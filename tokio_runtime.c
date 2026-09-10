// tokio_runtime.c — Zeta AOT async runtime.
// epoll + timerfd on Linux, kqueue + EVFILT_TIMER on macOS (PY-A: the
// previous version was epoll-only and could not be rebuilt on macOS; the
// checked-in tokio_runtime.o was a stale prebuilt object).
//
// Event flag encoding follows epoll bit values, which the generated code and
// the Rust reactor (src/runtime/reactor.rs) already use:
//   EPOLLIN=0x001, EPOLLOUT=0x004, EPOLLERR=0x008, EPOLLHUP=0x010

#include <stdint.h>
#include <unistd.h>
#include <fcntl.h>
#include <string.h>
#include <time.h>
#include <pthread.h>

#ifdef __APPLE__
#include <sys/event.h>
#define ZT_KQUEUE 1
#else
#include <sys/epoll.h>
#include <sys/timerfd.h>
#include <sys/socket.h>
#define ZT_EPOLL 1
#endif

// ── reactor ─────────────────────────────────────────────────────────────

#ifdef ZT_EPOLL
static struct epoll_event zt_buf[1024];
static pthread_mutex_t zt_lock = PTHREAD_MUTEX_INITIALIZER;

int64_t reactor_create(void) { return epoll_create1(EPOLL_CLOEXEC); }
int64_t reactor_add(int64_t e, int64_t f, int64_t ev) {
    struct epoll_event ee; memset(&ee,0,sizeof(ee)); ee.data.fd=(int)f;
    if (ev&1) ee.events|=EPOLLIN; if (ev&2) ee.events|=EPOLLOUT;
    ee.events|=EPOLLERR|EPOLLHUP;
    return epoll_ctl((int)e, EPOLL_CTL_ADD, (int)f, &ee);
}
int64_t reactor_modify(int64_t e, int64_t f, int64_t ev) {
    struct epoll_event ee; memset(&ee,0,sizeof(ee)); ee.data.fd=(int)f;
    if (ev&1) ee.events|=EPOLLIN; if (ev&2) ee.events|=EPOLLOUT;
    ee.events|=EPOLLERR|EPOLLHUP;
    return epoll_ctl((int)e, EPOLL_CTL_MOD, (int)f, &ee);
}
int64_t reactor_remove(int64_t e, int64_t f) { return epoll_ctl((int)e, EPOLL_CTL_DEL, (int)f, NULL); }
#else
// kqueue: epoll bit-value flags translated to EVFILT_READ/WRITE filters.
// epoll registers IN/OUT in ONE event; kqueue needs one kevent per filter.
static struct kevent zt_buf[1024];
static pthread_mutex_t zt_lock = PTHREAD_MUTEX_INITIALIZER;

static int zt_filter_add(int kq, int fd, int16_t filter) {
    struct kevent ev;
    EV_SET(&ev, (uintptr_t)fd, filter, EV_ADD, 0, 0, NULL);
    return kevent(kq, &ev, 1, NULL, 0, NULL);
}
static int zt_filter_del(int kq, int fd, int16_t filter) {
    struct kevent ev;
    EV_SET(&ev, (uintptr_t)fd, filter, EV_DELETE, 0, 0, NULL);
    return kevent(kq, &ev, 1, NULL, 0, NULL); // ENOENT ignored by caller
}

int64_t reactor_create(void) {
    int kq = kqueue();
    if (kq >= 0) fcntl(kq, F_SETFD, FD_CLOEXEC);
    return kq;
}
int64_t reactor_add(int64_t e, int64_t f, int64_t ev) {
    int r = 0;
    if (ev & 1) r |= zt_filter_add((int)e, (int)f, EVFILT_READ);
    if (ev & 2) r |= zt_filter_add((int)e, (int)f, EVFILT_WRITE);
    return r;
}
int64_t reactor_modify(int64_t e, int64_t f, int64_t ev) {
    // kevent EV_ADD on an existing ident+filter updates it; stale filters are
    // removed first so a dropped flag actually stops reporting.
    zt_filter_del((int)e, (int)f, EVFILT_READ);
    zt_filter_del((int)e, (int)f, EVFILT_WRITE);
    return reactor_add(e, f, ev);
}
int64_t reactor_remove(int64_t e, int64_t f) {
    zt_filter_del((int)e, (int)f, EVFILT_READ);
    zt_filter_del((int)e, (int)f, EVFILT_WRITE);
    return 0;
}
#endif

int64_t reactor_poll(int64_t e, int64_t buf, int64_t max, int64_t t) {
#ifdef ZT_EPOLL
    pthread_mutex_lock(&zt_lock);
    int n = epoll_wait((int)e, zt_buf, (int)max, (int)t);
    if (buf && n > 0) for (int i = 0; i < n && i < max; i++) {
        ((int64_t*)buf)[i*2]=zt_buf[i].data.fd; ((int64_t*)buf)[i*2+1]=zt_buf[i].events;
    }
    pthread_mutex_unlock(&zt_lock); return n;
#else
    pthread_mutex_lock(&zt_lock);
    struct timespec ts, *tsp = NULL;
    if (t >= 0) { ts.tv_sec = t/1000; ts.tv_nsec = (t%1000)*1000000; tsp = &ts; }
    int n = kevent((int)e, NULL, 0, zt_buf, (int)max, tsp);
    if (buf && n > 0) for (int i = 0; i < n && i < max; i++) {
        int64_t flags = 0;
        if (zt_buf[i].filter == EVFILT_READ)  flags |= 0x001; // EPOLLIN
        if (zt_buf[i].filter == EVFILT_WRITE) flags |= 0x004; // EPOLLOUT
        if (zt_buf[i].flags & EV_EOF)         flags |= 0x010; // EPOLLHUP
        ((int64_t*)buf)[i*2]   = (int64_t)zt_buf[i].ident;
        ((int64_t*)buf)[i*2+1] = flags;
    }
    pthread_mutex_unlock(&zt_lock); return n;
#endif
}

int64_t reactor_event_fd(int64_t buf, int64_t i) { return ((int64_t*)buf)[i*2]; }
int64_t reactor_event_flags(int64_t buf, int64_t i) { return ((int64_t*)buf)[i*2+1]; }
void reactor_destroy(int64_t e) { close((int)e); }

// ── waker ───────────────────────────────────────────────────────────────

int64_t waker_create(void) {
    int fds[2];
#ifdef ZT_EPOLL
    if (pipe2(fds, O_CLOEXEC | O_NONBLOCK) < 0) return -1;
#else
    // macOS has no pipe2: pipe() + explicit fcntl setup
    if (pipe(fds) < 0) return -1;
    for (int i = 0; i < 2; i++) {
        fcntl(fds[i], F_SETFD, FD_CLOEXEC);
        int fl = fcntl(fds[i], F_GETFL, 0);
        fcntl(fds[i], F_SETFL, fl | O_NONBLOCK);
    }
#endif
    return fds[0];
}
int64_t waker_wake(int64_t r) { char b=1; return write((int)(r+1),&b,1)>0?0:-1; }
int64_t waker_consume(int64_t r) { char b[8]; return read((int)r,b,8)>0?0:-1; }
void waker_destroy(int64_t r) { close((int)r); close((int)(r+1)); }

// ── timers ──────────────────────────────────────────────────────────────
// Linux: timerfd. macOS: EVFILT_TIMER on a dedicated kqueue (periodic by
// default, matching timerfd semantics; data reports the expiry count).

int64_t zt_timerfd_create(void) {
#ifdef ZT_EPOLL
    return timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC|TFD_NONBLOCK);
#else
    int kq = kqueue();
    if (kq >= 0) fcntl(kq, F_SETFD, FD_CLOEXEC);
    return kq;
#endif
}
int64_t zt_timerfd_set(int64_t f, int64_t ns) {
#ifdef ZT_EPOLL
    struct itimerspec s; memset(&s,0,sizeof(s));
    s.it_value.tv_sec=ns/1000000000; s.it_value.tv_nsec=ns%1000000000;
    return timerfd_settime((int)f,0,&s,NULL);
#else
    struct kevent ev;
    EV_SET(&ev, 1, EVFILT_TIMER, EV_DELETE, 0, 0, NULL);
    kevent((int)f, &ev, 1, NULL, 0, NULL); // reset any previous interval
    EV_SET(&ev, 1, EVFILT_TIMER, EV_ADD|EV_ENABLE, NOTE_NSECONDS, (uint64_t)ns, NULL);
    return kevent((int)f, &ev, 1, NULL, 0, NULL);
#endif
}
int64_t zt_timerfd_set_abs(int64_t f, int64_t an) {
#ifdef ZT_EPOLL
    struct itimerspec s; memset(&s,0,sizeof(s));
    s.it_value.tv_sec=an/1000000000; s.it_value.tv_nsec=an%1000000000;
    return timerfd_settime((int)f,TFD_TIMER_ABSTIME,&s,NULL);
#else
    // Absolute monotonic deadline → relative one-shot
    struct timespec ts;
    int64_t now = clock_gettime(CLOCK_MONOTONIC,&ts)
        ? -1 : (int64_t)ts.tv_sec*1000000000+ts.tv_nsec;
    int64_t rel = an - now; if (rel < 0) rel = 0;
    struct kevent ev;
    EV_SET(&ev, 1, EVFILT_TIMER, EV_DELETE, 0, 0, NULL);
    kevent((int)f, &ev, 1, NULL, 0, NULL);
    EV_SET(&ev, 1, EVFILT_TIMER, EV_ADD|EV_ENABLE|EV_ONESHOT, NOTE_NSECONDS, (uint64_t)rel, NULL);
    return kevent((int)f, &ev, 1, NULL, 0, NULL);
#endif
}
int64_t zt_timerfd_read(int64_t f) {
#ifdef ZT_EPOLL
    uint64_t v; return read((int)f,&v,8)>0?(int64_t)v:-1;
#else
    struct kevent ev;
    int n = kevent((int)f, NULL, 0, &ev, 1, &(struct timespec){0,0});
    return n > 0 ? (int64_t)ev.data : -1;
#endif
}

// ── misc ────────────────────────────────────────────────────────────────

int64_t set_nonblocking(int64_t f) {
    int fl=fcntl((int)f,F_GETFL,0); return fl<0?-1:fcntl((int)f,F_SETFL,fl|O_NONBLOCK);
}
int64_t monotonic_ns(void) {
    struct timespec ts; return clock_gettime(CLOCK_MONOTONIC,&ts)?-1:(int64_t)ts.tv_sec*1000000000+ts.tv_nsec;
}

int64_t scheduler_register_waker(int64_t e, int64_t w) {
#ifdef ZT_EPOLL
    struct epoll_event ev; ev.events=EPOLLIN|EPOLLERR|EPOLLHUP; ev.data.fd=(int)w;
    return epoll_ctl((int)e,EPOLL_CTL_ADD,(int)w,&ev);
#else
    return zt_filter_add((int)e, (int)w, EVFILT_READ);
#endif
}
int64_t scheduler_run_reactor(int64_t e, int64_t t) {
#ifdef ZT_EPOLL
    pthread_mutex_lock(&zt_lock);
    int n=epoll_wait((int)e,zt_buf,1024,(int)t);
    pthread_mutex_unlock(&zt_lock);
    if(n<=0)return n; int c=0;
    for(int i=0;i<n;i++) if(zt_buf[i].events&EPOLLIN)
        {char b[8];read((int)zt_buf[i].data.fd,b,8);c++;} return c;
#else
    pthread_mutex_lock(&zt_lock);
    struct timespec ts, *tsp = NULL;
    if (t >= 0) { ts.tv_sec = t/1000; ts.tv_nsec = (t%1000)*1000000; tsp = &ts; }
    int n = kevent((int)e, NULL, 0, zt_buf, 1024, tsp);
    pthread_mutex_unlock(&zt_lock);
    if(n<=0)return n; int c=0;
    for(int i=0;i<n;i++) if(zt_buf[i].filter==EVFILT_READ)
        {char b[8];read((int)zt_buf[i].ident,b,8);c++;} return c;
#endif
}
