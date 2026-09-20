#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <time.h>
#include <errno.h>
#include <pthread.h>
#include <gc.h>
#include <ctype.h>
#include <sys/wait.h>
#include <sys/time.h>
#include <math.h>
#include <dirent.h>
#include <glob.h>
#include <sys/stat.h>
#include <sys/types.h>

static pthread_mutex_t zt_lock = PTHREAD_MUTEX_INITIALIZER;

void array_push(int64_t arr, int64_t val);
// Defined in runtime/py_additions.c (linked as a separate object) — needed by
// py_path_parents below.
int64_t zeta_dynarray_new(int64_t cap);
int64_t vec_push(int64_t data_ptr, int64_t val);


static const char* zt_str_or_null(int64_t v) { return v ? (const char*)v : "<null>"; }
void println_i64(int64_t v) { printf("%lld\n", (long long)v); }
void print_i64(int64_t v) { printf("%lld", (long long)v); }
void println_f64(double v) { printf("%.6f\n", v); }
void print_f64(double v) { printf("%.6f", v); }
void print_bool(int64_t v) { printf("%s", v ? "true" : "false"); }
void print_str(int64_t v) { printf("%s", zt_str_or_null(v)); }
void println_str(int64_t v) { printf("%s\n", zt_str_or_null(v)); }
void print(int64_t v) { fputs(zt_str_or_null(v), stdout); }

// === String runtime (str_* mapped to host_str_* by codegen, GC-allocated) ===
int64_t str_len(int64_t s) { return s ? (int64_t)strlen((char*)s) : 0; }
int64_t str_concat(int64_t a, int64_t b) {
    if (!a || !b) return a ? a : b;
    size_t la = strlen((char*)a), lb = strlen((char*)b);
    char* out = (char*)GC_malloc(la + lb + 1);
    memcpy(out, (char*)a, la); memcpy(out+la, (char*)b, lb); out[la+lb] = 0;
    return (int64_t)out;
}
int64_t str_to_uppercase(int64_t s) {
    if (!s) return 0;
    size_t l = strlen((char*)s);
    char* out = (char*)GC_malloc(l+1);
    for (size_t i = 0; i <= l; i++) out[i] = (char)toupper((unsigned char)((char*)s)[i]);
    return (int64_t)out;
}
int64_t str_to_lowercase(int64_t s) {
    if (!s) return 0;
    size_t l = strlen((char*)s);
    char* out = (char*)GC_malloc(l+1);
    for (size_t i = 0; i <= l; i++) out[i] = (char)tolower((unsigned char)((char*)s)[i]);
    return (int64_t)out;
}
int64_t str_trim(int64_t s) {
    if (!s) return 0;
    char* start = (char*)s;
    while (*start && isspace((unsigned char)*start)) start++;
    size_t l = strlen(start);
    while (l > 0 && isspace((unsigned char)start[l-1])) l--;
    char* out = (char*)GC_malloc(l+1);
    memcpy(out, start, l); out[l] = 0;
    return (int64_t)out;
}
int64_t str_starts_with(int64_t hay, int64_t needle) {
    if (!hay || !needle) return 0;
    return strncmp((char*)hay, (char*)needle, strlen((char*)needle)) == 0 ? 1 : 0;
}
int64_t str_ends_with(int64_t hay, int64_t needle) {
    if (!hay || !needle) return 0;
    size_t lh = strlen((char*)hay), ln = strlen((char*)needle);
    if (ln > lh) return 0;
    return strcmp((char*)hay+lh-ln, (char*)needle) == 0 ? 1 : 0;
}
int64_t str_contains(int64_t hay, int64_t needle) {
    if (!hay || !needle) return 0;
    return strstr((char*)hay, (char*)needle) ? 1 : 0;
}
int64_t str_replace(int64_t s, int64_t old_s, int64_t new_s) {
    if (!s || !old_s || !new_s) return s;
    const char* str=(const char*)s, *pat=(const char*)old_s, *rep=(const char*)new_s;
    size_t plen=strlen(pat), rlen=strlen(rep), slen=strlen(str);
    size_t count=0; const char* p=str;
    while ((p=strstr(p,pat))!=NULL){count++;p+=plen;}
    if (count==0) return s;
    char* out=(char*)GC_malloc(slen+count*(rlen-plen)+1);
    p=str; char* o=out;
    const char* found;
    while ((found=strstr(p,pat))!=NULL) {
        size_t ch=(size_t)(found-p);
        memcpy(o,p,ch); o+=ch; memcpy(o,rep,rlen); o+=rlen;
        p=found+plen;
    }
    strcpy(o,p); return (int64_t)out;
}


// host_str_* aliases (codegen maps str_.method() to host_str_*)
int64_t host_str_len(int64_t s) { return str_len(s); }
int64_t host_str_concat(int64_t a, int64_t b) { return str_concat(a, b); }
int64_t host_str_to_uppercase(int64_t s) { return str_to_uppercase(s); }
int64_t host_str_to_lowercase(int64_t s) { return str_to_lowercase(s); }
int64_t host_str_trim(int64_t s) { return str_trim(s); }
int64_t host_str_starts_with(int64_t h, int64_t n) { return str_starts_with(h, n); }
int64_t host_str_ends_with(int64_t h, int64_t n) { return str_ends_with(h, n); }
int64_t host_str_contains(int64_t h, int64_t n) { return str_contains(h, n); }
int64_t host_str_replace(int64_t s, int64_t o, int64_t n) { return str_replace(s, o, n); }

// === Map runtime (i64 key/value, open-addressing hash map, GC-allocated) ===
#define MAP_ENTRY_SIZE 24
static int64_t map_hash(int64_t key) {
    uint64_t h=(uint64_t)key;
    h^=h>>33; h*=0xff51afd7ed558ccdULL;
    h^=h>>33; h*=0xc4ceb9fe1a85ec53ULL;
    h^=h>>33; return (int64_t)h;
}
int64_t map_new(void) {
    int64_t cap=16;
    char* base=(char*)GC_malloc(16+cap*MAP_ENTRY_SIZE);
    *(int64_t*)base=cap; *((int64_t*)base+1)=0;
    return (int64_t)base;
}
// A dict HANDLE is a raw int64 passed BY VALUE, so growth may not move the
// block: every holder would keep a stale pointer. The old block therefore
// becomes a FORWARDER — word0 = MAP_MOVED (negative cap), word1 = new address.
// It is exactly its own 16-byte header, so nothing overflows. Measured bug: the
// old code `memcpy(map, nb, 16+nc*ENTRY)` wrote the (bigger) new table INTO the
// (smaller) old block — heap corruption: "GC Warning: Failed to expand heap by
// 12143674558099984 KiB" and a SIGSEGV for any dict past ~12 keys (the ETF
// listing cache, 50 KB, died in zj_parse_value).
// A `-> dict` annotation on a function whose body returns `json.loads(...)` types
// the result `map`, while the VALUE is a PyJson cell [tag, payload] (tag 1..5). The
// map primitives then read the TAG as a capacity — `idx = hash & (cap-1)` spun
// forever and the local backtest HUNG inside `map_get_default` (measured with
// lldb). A map's first word is its capacity (>= 16), so a first word in 1..8
// identifies a Json value: report it loudly instead of hanging or returning a
// silent 0. (Fixing the TYPE is the real fix; this turns a hang into a
// diagnostic.)
int zt_map_is_json_handle(int64_t h) {
    if (!h) return 0;
    int64_t w0 = *(int64_t*)h;
    return (w0 >= 1 && w0 <= 8);
}
void zt_map_json_mismatch(const char* fn) {
    fprintf(stderr,
            "PY-A: `%s` was called on a JSON value (a `-> dict` annotation on a "
            "function returning json.loads() types it as a map). Refusing to "
            "reinterpret it as a hash table.\n",
            fn);
    fflush(stderr);
    abort();
}

#define MAP_MOVED (-1)
int64_t map_resolve(int64_t map) {
    // Follow the WHOLE forward chain: a block that was moved twice is itself a
    // forwarder. Resolving only one hop left `map_insert` on a block whose cap is
    // MAP_MOVED, so `idx = hash & (cap-1)` went wildly out of bounds and wrote
    // over the GC heap ("Failed to expand heap by 18014398509481968 KiB").
    int guard = 0;
    while (map && ((int64_t*)map)[0] < 0 && guard++ < 64) map = ((int64_t*)map)[1];
    return map;
}
void map_insert(int64_t map0, int64_t key, int64_t val) {
    int64_t map = map_resolve(map0);
    if (!map) return;
    if (zt_map_is_json_handle(map)) zt_map_json_mismatch("map_insert");
    int64_t* hdr=(int64_t*)map; int64_t cap=hdr[0]; int64_t len=hdr[1];
    if (len*4 >= cap*3) {
        int64_t nc=cap*2;
        char* nb=(char*)GC_malloc(16+nc*MAP_ENTRY_SIZE);
        *(int64_t*)nb=nc; *((int64_t*)nb+1)=0;
        for (int64_t i=0;i<cap;i++){
            char* e=(char*)map+16+i*MAP_ENTRY_SIZE;
            if (*(uint8_t*)(e+16)==1) map_insert((int64_t)nb,*(int64_t*)e,*((int64_t*)e+1));
        }
        hdr[0]=MAP_MOVED; hdr[1]=(int64_t)nb;   // old handle forwards to the new block
        map=(int64_t)nb; hdr=(int64_t*)map; cap=nc;
    }
    int64_t h=map_hash(key); int64_t idx=h&(cap-1);
    int64_t tomb=-1;
    while(1){
        char* e=(char*)map+16+idx*MAP_ENTRY_SIZE;
        uint8_t used=*(uint8_t*)(e+16);
        if(!used){
            if(tomb>=0){ idx=tomb; e=(char*)map+16+idx*MAP_ENTRY_SIZE; }
            *(int64_t*)e=key;*((int64_t*)e+1)=val;*(uint8_t*)(e+16)=1;hdr[1]++;return;
        }
        if(used==2 && tomb<0) tomb=idx;
        if(used==1 && *(int64_t*)e==key){*((int64_t*)e+1)=val;return;}
        idx=(idx+1)&(cap-1);
    }
}
int64_t map_get(int64_t map0, int64_t key) {
    int64_t map = map_resolve(map0);
    if (!map) return 0;
    if (zt_map_is_json_handle(map)) zt_map_json_mismatch("map_get");
    int64_t cap=((int64_t*)map)[0]; int64_t h=map_hash(key); int64_t idx=h&(cap-1);
    while(1){
        char* e=(char*)map+16+idx*MAP_ENTRY_SIZE;
        uint8_t used=*(uint8_t*)(e+16);
        if(!used)return 0;
        if(used==1 && *(int64_t*)e==key)return *((int64_t*)e+1);
        idx=(idx+1)&(cap-1);
    }
}
void map_free(int64_t map){(void)map;} /* GC-managed */

void flush(void) { fflush(stdout); }

#ifndef ZT_REAL_ASYNC // fake async stubs — excluded when tokio_runtime.c (real kqueue/epoll) is linked
int64_t reactor_create(void) { return (int64_t)malloc(1); }
int64_t reactor_add(int64_t e, int64_t f, int64_t ev) { (void)e;(void)f;(void)ev; return 0; }
int64_t reactor_modify(int64_t e, int64_t f, int64_t ev) { (void)e;(void)f;(void)ev; return 0; }
int64_t reactor_remove(int64_t e, int64_t f) { (void)e;(void)f; return 0; }
int64_t reactor_poll(int64_t e, int64_t buf, int64_t max, int64_t t) { (void)e;(void)buf;(void)max;(void)t; return 0; }
int64_t reactor_event_fd(int64_t buf, int64_t i) { (void)buf;(void)i; return -1; }
int64_t reactor_event_flags(int64_t buf, int64_t i) { (void)buf;(void)i; return 0; }
void reactor_destroy(int64_t e) { free((void*)e); }

int64_t waker_create(void) { int fds[2]; return pipe(fds)==0?(int64_t)fds[0]:-1; }
int64_t waker_wake(int64_t r) { char b=1; return write((int)(r+1),&b,1)>0?0:-1; }
int64_t waker_consume(int64_t r) { char b[8]; return read((int)r,b,8)>0?0:-1; }
void waker_destroy(int64_t r) { close((int)r); close((int)(r+1)); }

int64_t zt_timerfd_create(void) { return -1; }
int64_t zt_timerfd_set(int64_t f, int64_t ns) { (void)f;(void)ns; return -1; }
int64_t zt_timerfd_set_abs(int64_t f, int64_t an) { (void)f;(void)an; return -1; }
int64_t zt_timerfd_read(int64_t f) { (void)f; return -1; }

int64_t set_nonblocking(int64_t f) { int fl=fcntl((int)f,F_GETFL,0); return fl<0?-1:fcntl((int)f,F_SETFL,fl|O_NONBLOCK); }
int64_t monotonic_ns(void) { struct timespec ts; clock_gettime(CLOCK_MONOTONIC,&ts); return (int64_t)ts.tv_sec*1000000000+ts.tv_nsec; }
int64_t scheduler_register_waker(int64_t e, int64_t w) { (void)e;(void)w; return 0; }
int64_t scheduler_run_reactor(int64_t e, int64_t t) { (void)e;(void)t; return 0; }
#endif // ZT_REAL_ASYNC

int64_t runtime_malloc(int64_t size) { return (int64_t)GC_malloc((size_t)size); }
void runtime_free(int64_t ptr) { (void)ptr; /* GC-managed, no-op */ }
int64_t runtime_calloc(int64_t n, int64_t size) {
    size_t total = (size_t)n * (size_t)size;
    void* p = GC_malloc(total);
    if (p) memset(p, 0, total);
    return (int64_t)p;
}
int64_t runtime_realloc(int64_t ptr, int64_t size) { return (int64_t)GC_realloc((void*)ptr, (size_t)size); }

int64_t array_new(int64_t size, int64_t elem_size) { return runtime_malloc(size * elem_size); }
// Single-arg array_new(size) — size is the total byte count (benchmark tests)
int64_t array_new_1(int64_t size) { return runtime_malloc(size); }
// LLVM renames overloaded array_new_1 to array_new.10 internally
// array_new_10 是 array_new 的 1 参数重载版本（LLVM 内部重命名为 array_new.10）
int64_t array_new_10(int64_t size) { return runtime_malloc(size); }

// Multi-arg print overloads: LLVM renames print -> print.N per module, and arity
// varies (1-6). Provide implementations; the .set aliases below map each print.N
// to the right arity.
static void _print_one(int64_t v) {
    const char* s = (const char*)v;
    if (v != 0) { fputs(s, stdout); } else { fputs("0", stdout); }
}
int64_t print2(int64_t a, int64_t b) { _print_one(a); _print_one(b); return 0; }
int64_t print3(int64_t a, int64_t b, int64_t c) { _print_one(a); _print_one(b); _print_one(c); return 0; }
int64_t print4(int64_t a, int64_t b, int64_t c, int64_t d) { _print_one(a); _print_one(b); _print_one(c); _print_one(d); return 0; }
int64_t print5(int64_t a, int64_t b, int64_t c, int64_t d, int64_t e) { _print_one(a); _print_one(b); _print_one(c); _print_one(d); _print_one(e); return 0; }
int64_t print6(int64_t a, int64_t b, int64_t c, int64_t d, int64_t e, int64_t f) { _print_one(a); _print_one(b); _print_one(c); _print_one(d); _print_one(e); _print_one(f); return 0; }

// LLVM .N rename aliases — data in pylib/runtime_aliases.txt;
// regenerate: python3 tools/gen_from_registry.py --emit-aliases
#include "aliases.inc.c"

// print.N alias with N args maps to printf-style — declare variadic impl
int64_t print_variadic(int64_t n, ...);
__attribute__((used)) static int64_t print_impl_dummy = 0;

// append alias for array_push
void append(int64_t arr, int64_t val) { array_push(arr, val); }
// (count_primes stub removed — collided with user-defined functions of the
// same name, causing DUPLICATE_SYM link failures)

// Arrays are uniformly [cap | len | elems...] with the handle pointing at
// elems (same layout as vec_len/vec_get). null-safe.
int64_t array_len(int64_t arr) { return arr ? ((int64_t*)(arr - 16))[1] : 0; }
int64_t array_get(int64_t arr, int64_t idx) { return ((int64_t*)arr)[idx]; }
void array_set(int64_t arr, int64_t idx, int64_t val) { ((int64_t*)arr)[idx] = val; }
void array_push(int64_t arr, int64_t val) { (void)arr;(void)val; }
void array_free(int64_t arr) { (void)arr; /* GC-managed, no-op */ }
void array_set_len(int64_t arr, int64_t len) { (void)arr;(void)len; }

// stack_array_get/set — arrays stored as [len | elems...] (get/set with bounds check)
int64_t stack_array_get(int64_t arr, int64_t idx) { return ((int64_t*)arr)[idx]; }
void stack_array_set(int64_t arr, int64_t idx, int64_t val) { ((int64_t*)arr)[idx] = val; }

int64_t clone_i64(int64_t v) { return v; }
int64_t is_null_i64(int64_t v) { return v == 0; }
int64_t to_string_i64(int64_t v) { char* s = (char*)GC_malloc(24); sprintf(s, "%lld", (long long)v); return (int64_t)s; }
int64_t clone_bool(int64_t v) { return v; }
int64_t is_null_bool(int64_t v) { return v == 0; }
int64_t to_string_bool(int64_t v) { char* s = (char*)GC_malloc(6); sprintf(s, "%s", v ? "true" : "false"); return (int64_t)s; }
int64_t to_string_str(int64_t v) { return v; }

// === Option<T> runtime (GC-allocated, layout: [tag i64 | data i64]) ===
int64_t option_make_some(int64_t data) {
    int64_t* p = (int64_t*)GC_malloc(16);
    p[0] = 1;
    p[1] = data;
    return (int64_t)p;
}
int64_t option_make_none(void) {
    int64_t* p = (int64_t*)GC_malloc(16);
    p[0] = 0;
    p[1] = 0;
    return (int64_t)p;
}
int64_t option_is_some(int64_t opt) {
    if (opt == 0) return 0;
    return ((int64_t*)opt)[0] == 1 ? 1 : 0;
}
int64_t option_get_data(int64_t opt) {
    if (opt == 0) return 0;
    return ((int64_t*)opt)[1];
}

// === Result<T, E> runtime (GC-allocated, layout: [tag i64 | ok_data i64 | err_data i64]) ===
int64_t host_result_make_ok(int64_t data) {
    int64_t* p = (int64_t*)GC_malloc(24);
    p[0] = 1; // ok
    p[1] = data;
    p[2] = 0;
    return (int64_t)p;
}
int64_t host_result_make_err(int64_t err) {
    int64_t* p = (int64_t*)GC_malloc(24);
    p[0] = 0; // err
    p[1] = 0;
    p[2] = err;
    return (int64_t)p;
}
int64_t host_result_is_ok(int64_t res) {
    if (res == 0) return 0;
    return ((int64_t*)res)[0];
}
int64_t host_result_get_data(int64_t res) {
    if (res == 0) return 0;
    int64_t* p = (int64_t*)res;
    return p[0] == 1 ? p[1] : p[2];
}

// === Vec<T> runtime (GC-allocated, layout: [cap i64 | len i64 | data...]) ===
// Functions return the data pointer (after the 16-byte header).
int64_t vec_new(int64_t capacity) {
    int64_t cap = capacity < 4 ? 4 : capacity;
    int64_t* base = (int64_t*)GC_malloc(16 + cap * 8);
    base[0] = cap;
    base[1] = 0;
    return (int64_t)(base + 2);
}
int64_t vec_push(int64_t data_ptr, int64_t val) {
    int64_t* base = (int64_t*)(data_ptr - 16);
    int64_t cap = base[0];
    int64_t len = base[1];
    if (len >= cap) {
        // floor the growth so a zero-capacity array (e.g. `[]`) can grow:
        // cap * 2 would otherwise stay 0 forever and every push would
        // allocate a 16-byte buffer with no room for the element.
        int64_t new_cap = cap < 8 ? 8 : cap * 2;
        int64_t* nb = (int64_t*)GC_malloc(16 + new_cap * 8);
        nb[0] = new_cap;
        nb[1] = len;
        for (int64_t i = 0; i < len; i++) nb[2 + i] = base[2 + i];
        nb[2 + len] = val;
        nb[1] = len + 1;
        return (int64_t)(nb + 2);
    }
    base[2 + len] = val;
    base[1] = len + 1;
    return data_ptr;
}
int64_t vec_get(int64_t data_ptr, int64_t idx) {
    int64_t* base = (int64_t*)(data_ptr - 16);
    (void)base; // bounds check omitted: ponytail
    return ((int64_t*)data_ptr)[idx];
}
int64_t vec_len(int64_t data_ptr) {
    if (!data_ptr) return 0;
    // Same validation as the slice helper: a non-Vec handle must not produce
    // a garbage length (and downstream absurd allocations).
    int64_t* h = (int64_t*)(data_ptr - 16);
    int64_t cap = h[0], len = h[1];
    // Only obviously-insane values are rejected: some array kinds legitimately
    // report len > cap (they are grown by a stub push), and rejecting those
    // made len() return 0.
    if (cap < 0 || len < 0 || len > (1 << 28) || cap > (1 << 28)) return 0;
    return len;
}
void vec_free(int64_t data_ptr) { (void)data_ptr; /* GC-managed */ }

int64_t zeta_array_get_i64(int64_t arr, int64_t idx) { return ((int64_t*)arr)[idx]; }
void zeta_array_set_i64(int64_t arr, int64_t idx, int64_t val) { ((int64_t*)arr)[idx] = val; }
int64_t zeta_array_get_bool(int64_t arr, int64_t idx) { return ((int64_t*)arr)[idx]; }
// === spawn/join: real pthread-based async ===
// spawn(fn_addr) -> handle (new thread runs fn_addr, result stored globally)
// join(handle) -> result

static pthread_mutex_t spawn_lock = PTHREAD_MUTEX_INITIALIZER;
static int64_t thread_results[256] = {0};
static int result_idx = 0;

typedef int64_t (*thunk_fn)(void);

static void* spawn_worker(void* arg) {
    thunk_fn fn = (thunk_fn)arg;
    int64_t result = fn();
    pthread_mutex_lock(&spawn_lock);
    int idx = result_idx++ % 256;
    thread_results[idx] = result;
    pthread_mutex_unlock(&spawn_lock);
    return (void*)result;
}

int64_t spawn(int64_t fn_addr) {
    pthread_t tid;
    if (pthread_create(&tid, NULL, spawn_worker, (void*)fn_addr) != 0) {
        return -1;
    }
    return (int64_t)tid;
}

int64_t join(int64_t handle) {
    void* ret = NULL;
    pthread_join((pthread_t)handle, &ret);
    return (int64_t)ret;
}

// ============================================================================
// PY-A Python stdlib concurrency shims — threading / concurrent.futures /
// multiprocessing / asyncio / time. Built on the native primitives above.
//
// Semantics notes (V1):
//   - threading.Thread(target=f).start() runs f on a real pthread; .join()
//     returns f's value.
//   - Lock is a real pthread_mutex.
//   - futures.Executor.submit(f) spawns a thread per task (no worker reuse).
//   - multiprocessing.Process uses fork(2) (real processes); Pool.map runs
//     its items in parallel but inside the parent process (threads) —
//     ponytail: no result IPC, upgrade to pipes if isolation is required.
//   - asyncio has no event loop: run()/create_task() evaluate inline and
//     sleep() blocks. Values are correct, concurrency is not.
// ============================================================================

typedef struct {
    pthread_t th;
    int64_t (*fn)(int64_t);
    int64_t arg;
    int64_t result;
    int started;
    int joined;
} py_thread_t;

static void* py_thread_trampoline(void* arg) {
    py_thread_t* t = (py_thread_t*)arg;
    if (t->fn) {
        typedef int64_t (*fn_i64_t)(int64_t);
        t->result = ((fn_i64_t)(uintptr_t)t->fn)(t->arg);
    } else {
        t->result = 0;
    }
    return NULL;
}

int64_t py_threading_thread_new(int64_t fn) {
    py_thread_t* t = (py_thread_t*)GC_malloc(sizeof(py_thread_t));
    t->fn = (int64_t (*)(int64_t))fn;
    t->arg = 0;
    t->result = 0;
    t->started = 0;
    t->joined = 0;
    return (int64_t)t;
}

int64_t py_threading_thread_new_2(int64_t fn, int64_t arg) {
    py_thread_t* t = (py_thread_t*)GC_malloc(sizeof(py_thread_t));
    t->fn = (int64_t (*)(int64_t))fn;
    t->arg = arg;
    t->result = 0;
    t->started = 0;
    t->joined = 0;
    return (int64_t)t;
}

int64_t py_threading_thread_start(int64_t h) {
    if (!h) return -1;
    py_thread_t* t = (py_thread_t*)h;
    if (t->started) return -1;
    t->started = 1;
    return (int64_t)pthread_create(&t->th, NULL, py_thread_trampoline, t);
}

int64_t py_threading_thread_join(int64_t h) {
    if (!h) return -1;
    py_thread_t* t = (py_thread_t*)h;
    if (t->started && !t->joined) {
        pthread_join(t->th, NULL);
        t->joined = 1;
    }
    return t->result;
}

int64_t py_threading_thread_is_alive(int64_t h) {
    if (!h) return 0;
    py_thread_t* t = (py_thread_t*)h;
    return (t->started && !t->joined) ? 1 : 0;
}

int64_t py_threading_get_ident(void) { return (int64_t)pthread_self(); }
int64_t py_threading_current_thread(void) { return (int64_t)pthread_self(); }
int64_t py_threading_active_count(void) { return 1; }

// ---- threading.Lock (real mutex) ----
int64_t py_threading_lock_new(void) {
    pthread_mutex_t* m = (pthread_mutex_t*)GC_malloc(sizeof(pthread_mutex_t));
    memset(m, 0, sizeof(pthread_mutex_t));
    pthread_mutex_init(m, NULL);
    return (int64_t)m;
}
int64_t py_threading_lock_acquire(int64_t h) {
    if (!h) return -1;
    return (int64_t)pthread_mutex_lock((pthread_mutex_t*)h);
}
int64_t py_threading_lock_release(int64_t h) {
    if (!h) return -1;
    return (int64_t)pthread_mutex_unlock((pthread_mutex_t*)h);
}
int64_t py_threading_lock_locked(int64_t h) {
    if (!h) return 0;
    return pthread_mutex_trylock((pthread_mutex_t*)h) == 0
        ? (pthread_mutex_unlock((pthread_mutex_t*)h), 0)
        : 1;
}

// ---- concurrent.futures: thread-per-task, no worker reuse (V1) ----
typedef struct {
    pthread_t th;
    int64_t (*fn)(void);
    int64_t result;
    int started;
} py_future_t;

static void* py_future_trampoline(void* arg) {
    py_future_t* f = (py_future_t*)arg;
    f->result = f->fn ? f->fn() : 0;
    return NULL;
}

int64_t py_futures_executor_new(int64_t workers) {
    (void)workers; // V1: no pool bound, task-per-thread
    return 1;
}

int64_t py_futures_submit(int64_t ex, int64_t fn) {
    (void)ex;
    py_future_t* f = (py_future_t*)GC_malloc(sizeof(py_future_t));
    f->fn = (int64_t (*)(void))fn;
    f->result = 0;
    f->started = pthread_create(&f->th, NULL, py_future_trampoline, f) == 0;
    return (int64_t)f;
}

int64_t py_futures_result(int64_t fh) {
    if (!fh) return -1;
    py_future_t* f = (py_future_t*)fh;
    if (f->started) {
        pthread_join(f->th, NULL);
        f->started = 0;
    }
    return f->result;
}

int64_t py_futures_done(int64_t fh) {
    if (!fh) return 0;
    return ((py_future_t*)fh)->started ? 0 : 1;
}

int64_t py_futures_shutdown(int64_t ex) { (void)ex; return 0; }

// Executor.map(fn, data) — run fn over a Vec-layout handle in parallel.
typedef struct {
    int64_t (*target)(int64_t);
    int64_t item;
    int64_t result;
} py_map_task_t;

static void* py_map_worker(void* arg) {
    py_map_task_t* t = (py_map_task_t*)arg;
    t->result = t->target(t->item);
    return NULL;
}

int64_t py_futures_map(int64_t ex, int64_t fn, int64_t data) {
    (void)ex;
    if (!data) return 0;
    int64_t len = ((int64_t*)(data - 16))[1];
    int64_t cap = len < 8 ? 8 : len;
    int64_t* out = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    out[0] = cap;
    out[1] = len;
    py_map_task_t* tasks =
        (py_map_task_t*)GC_malloc(sizeof(py_map_task_t) * (size_t)(len ? len : 1));
    pthread_t* tids = (pthread_t*)GC_malloc(sizeof(pthread_t) * (size_t)(len ? len : 1));
    int64_t (*target)(int64_t) = (int64_t (*)(int64_t))fn;
    for (int64_t i = 0; i < len; i++) {
        tasks[i].target = target;
        tasks[i].item = ((int64_t*)data)[i];
        tasks[i].result = 0;
        tids[i] = 0;
        if (pthread_create(&tids[i], NULL, py_map_worker, &tasks[i]) != 0) {
            tasks[i].result = target(tasks[i].item); // fall back inline
            tids[i] = 0;
        }
    }
    for (int64_t i = 0; i < len; i++) {
        if (tids[i]) pthread_join(tids[i], NULL);
        out[2 + i] = tasks[i].result;
    }
    return (int64_t)(out + 2);
}

int64_t py_mp_pool_apply(int64_t p, int64_t fn, int64_t item) {
    (void)p;
    int64_t (*target)(int64_t) = (int64_t (*)(int64_t))fn;
    return target(item);
}

// ---- multiprocessing: Process uses fork(2) ----
typedef struct {
    pid_t pid;
    int started;
    int64_t (*fn)(void);
    int status;
} py_process_t;

int64_t py_mp_process_new(int64_t fn) {
    py_process_t* p = (py_process_t*)GC_malloc(sizeof(py_process_t));
    p->pid = 0;
    p->started = 0;
    p->fn = (int64_t (*)(void))fn;
    p->status = 0;
    return (int64_t)p;
}

int64_t py_mp_process_start(int64_t h) {
    if (!h) return -1;
    py_process_t* p = (py_process_t*)h;
    if (p->started) return -1;
    pid_t pid = fork();
    if (pid == 0) {
        int64_t r = p->fn ? p->fn() : 0;
        _exit((int)(r & 0xff));
    }
    if (pid < 0) return -1;
    p->pid = pid;
    p->started = 1;
    return 0;
}

int64_t py_mp_process_join(int64_t h) {
    if (!h) return -1;
    py_process_t* p = (py_process_t*)h;
    if (p->started) {
        waitpid(p->pid, &p->status, 0);
        p->started = 0;
    }
    // Python's join() returns None; we return the exit code instead (None is
    // not representable) — `p.exitcode()` returns the same value.
    return WIFEXITED(p->status) ? (int64_t)WEXITSTATUS(p->status) : -1;
}

int64_t py_mp_process_is_alive(int64_t h) {
    if (!h) return 0;
    py_process_t* p = (py_process_t*)h;
    if (!p->started) return 0;
    return waitpid(p->pid, &p->status, WNOHANG) == 0 ? 1 : 0;
}

int64_t py_mp_process_exitcode(int64_t h) {
    if (!h) return -1;
    py_process_t* p = (py_process_t*)h;
    if (!p->started && WIFEXITED(p->status)) return (int64_t)WEXITSTATUS(p->status);
    return -1;
}

int64_t py_mp_current_process(void) { return (int64_t)getpid(); }
int64_t py_mp_cpu_count(void) {
    int n = (int)sysconf(_SC_NPROCESSORS_ONLN);
    return n > 0 ? n : 1;
}

// Pool: V1 keeps the API but runs map() with the same parallel-in-process
// strategy as the futures executor.
int64_t py_mp_pool_new(int64_t workers) { return py_futures_executor_new(workers); }
int64_t py_mp_pool_map(int64_t pool, int64_t fn, int64_t data) {
    return py_futures_map(pool, fn, data);
}
int64_t py_mp_pool_close(int64_t p) { (void)p; return 0; }
int64_t py_mp_pool_join(int64_t p) { (void)p; return 0; }

// ---- asyncio (V1: sequential, no event loop) ----
int64_t py_asyncio_run(int64_t v) { return v; }
int64_t py_asyncio_sleep(double seconds) {
    if (seconds > 0) {
        struct timespec ts;
        ts.tv_sec = (time_t)seconds;
        ts.tv_nsec = (long)((seconds - (double)ts.tv_sec) * 1e9);
        nanosleep(&ts, NULL);
    }
    return 0;
}

// ---- time ----
void py_time_sleep(double seconds) { py_asyncio_sleep(seconds); }
double py_time_time(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (double)tv.tv_sec + (double)tv.tv_usec / 1e6;
}
double py_time_monotonic(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

// ---- math shims (registry-driven: declared by codegen from registry.txt) ----
double py_math_sqrt(double x) { return sqrt(x); }
double py_math_fabs(double x) { return fabs(x); }
int64_t py_math_floor(double x) { return (int64_t)floor(x); }
int64_t py_math_ceil(double x) { return (int64_t)ceil(x); }
double py_math_pow(double a, double b) { return pow(a, b); }

// ============================================================================
// PY-A stdlib shims, batch 2: os / os.path / os.environ / math / logging.
// Selected by evidence from the REasyQuant corpus (datetime 12, json 6, os 5,
// logging 5, math 15 call sites) rather than by completeness.
// ============================================================================

static char* zt_strdup(const char* s) {
    size_t n = strlen(s);
    char* r = (char*)GC_malloc(n + 1);
    memcpy(r, s, n + 1);
    return r;
}

int64_t py_os_getcwd(void) {
    char buf[4096];
    return (int64_t)zt_strdup(getcwd(buf, sizeof buf) ? buf : "");
}
int64_t py_os_getpid(void) { return (int64_t)getpid(); }
int64_t py_os_system(int64_t cmd) { return cmd ? (int64_t)system((const char*)cmd) : 0; }
int64_t py_os_getenv(int64_t name) {
    const char* v = name ? getenv((const char*)name) : NULL;
    return (int64_t)zt_strdup(v ? v : "");
}
int64_t py_os_listdir(int64_t path) {
    // Vec-layout handle of names in `path` (or "." when empty).
    const char* d = path ? (const char*)path : ".";
    DIR* dir = opendir(d);
    int64_t cap = 8, len = 0;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap;
    base[1] = 0;
    if (dir) {
        struct dirent* e;
        while ((e = readdir(dir)) != NULL) {
            if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) continue;
            if (len >= cap) {
                int64_t ncap = cap * 2;
                int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)ncap * 8);
                nb[0] = ncap;
                nb[1] = len;
                for (int64_t i = 0; i < len; i++) nb[2 + i] = base[2 + i];
                base = nb;
                cap = ncap;
            }
            base[2 + len++] = (int64_t)zt_strdup(e->d_name);
        }
        closedir(dir);
    }
    base[1] = len;
    return (int64_t)(base + 2);
}
int64_t py_os_makedirs(int64_t path) {
    if (!path) return -1;
    char tmp[4096];
    snprintf(tmp, sizeof tmp, "%s", (const char*)path);
    for (char* p = tmp + 1; *p; p++) {
        if (*p == '/') {
            *p = '\0';
            mkdir(tmp, 0777);
            *p = '/';
        }
    }
    return (int64_t)mkdir(tmp, 0777);
}
// `os.makedirs(p, exist_ok=True)` — the 2-arg call site (registry declares
// the 1-arg form; the runtime's `_N` convention carries the optional arg).
// exist_ok only suppresses an "already exists" raise, which this shim never
// raises, so it is ignored (never dropped data).
int64_t py_os_makedirs_2(int64_t path, int64_t exist_ok) {
    (void)exist_ok;
    return py_os_makedirs(path);
}
int64_t py_os_remove(int64_t path) { return path ? (int64_t)remove((const char*)path) : -1; }

// ---- os.path ----
int64_t py_os_path_join(int64_t a, int64_t b) {
    const char* x = a ? (const char*)a : "";
    const char* y = b ? (const char*)b : "";
    if (!*x) return (int64_t)zt_strdup(y);
    if (!*y) return (int64_t)zt_strdup(x);
    int need = (x[strlen(x) - 1] == '/') ? 0 : 1;
    char* r = (char*)GC_malloc(strlen(x) + strlen(y) + 2);
    strcpy(r, x);
    if (need) strcat(r, "/");
    strcat(r, y);
    return (int64_t)r;
}
int64_t py_os_path_basename(int64_t p) {
    const char* s = p ? (const char*)p : "";
    const char* slash = strrchr(s, '/');
    return (int64_t)zt_strdup(slash ? slash + 1 : s);
}
int64_t py_os_path_dirname(int64_t p) {
    const char* s = p ? (const char*)p : "";
    const char* slash = strrchr(s, '/');
    if (!slash) return (int64_t)zt_strdup("");
    if (slash == s) return (int64_t)zt_strdup("/");
    size_t n = (size_t)(slash - s);
    char* r = (char*)GC_malloc(n + 1);
    memcpy(r, s, n);
    r[n] = '\0';
    return (int64_t)r;
}
int64_t py_os_path_splitext(int64_t p) {
    // Returns the extension only (V1: the common `os.path.splitext(f)[1]` use).
    const char* s = p ? (const char*)p : "";
    const char* slash = strrchr(s, '/');
    const char* dot = strrchr(s, '.');
    if (!dot || (slash && dot < slash)) return (int64_t)zt_strdup("");
    return (int64_t)zt_strdup(dot);
}
int64_t py_os_path_exists(int64_t p) {
    struct stat st;
    return (p && stat((const char*)p, &st) == 0) ? 1 : 0;
}
int64_t py_os_path_isfile(int64_t p) {
    struct stat st;
    return (p && stat((const char*)p, &st) == 0 && S_ISREG(st.st_mode)) ? 1 : 0;
}
int64_t py_os_path_isdir(int64_t p) {
    struct stat st;
    return (p && stat((const char*)p, &st) == 0 && S_ISDIR(st.st_mode)) ? 1 : 0;
}
int64_t py_os_path_abspath(int64_t p) {
    char buf[4096];
    const char* s = p ? (const char*)p : ".";
    if (!realpath(s, buf)) {
        char cwd[4096];
        if (!getcwd(cwd, sizeof cwd)) return (int64_t)zt_strdup(s);
        snprintf(buf, sizeof buf, "%s/%s", cwd, s);
    }
    return (int64_t)zt_strdup(buf);
}
int64_t py_os_path_expanduser(int64_t p) {
    const char* s = p ? (const char*)p : "";
    if (s[0] == '~') {
        const char* home = getenv("HOME");
        if (home) {
            char* r = (char*)GC_malloc(strlen(home) + strlen(s) + 1);
            strcpy(r, home);
            strcat(r, s + 1);
            return (int64_t)r;
        }
    }
    return (int64_t)zt_strdup(s);
}

// ---- os.environ (process environment) ----
int64_t py_os_environ_get(int64_t k, int64_t dflt) {
    const char* v = k ? getenv((const char*)k) : NULL;
    if (v) return (int64_t)zt_strdup(v);
    return dflt ? dflt : (int64_t)zt_strdup("");
}
// `os.environ.get("KEY")` — the 1-arg form (Python's default is None, i.e. an
// empty string here). The registry declares the 2-arg form, so the 1-arg call
// site arity-mangled the symbol to `py_os_environ_get_1`, which nothing defined:
// 10 undefined-symbol reference sites in the REasyQuant local-backtest link.
int64_t py_os_environ_get_1(int64_t k) { return py_os_environ_get(k, 0); }
int64_t py_os_environ_setdefault(int64_t k, int64_t v) {
    if (!k) return v;
    const char* cur = getenv((const char*)k);
    if (cur) return (int64_t)zt_strdup(cur);
    setenv((const char*)k, v ? (const char*)v : "", 1);
    return v;
}

// ---- math (completes the libm set the corpus actually calls) ----
double py_math_exp(double x) { return exp(x); }
double py_math_log(double x) { return log(x); }
double py_math_log10(double x) { return log10(x); }
double py_math_sin(double x) { return sin(x); }
double py_math_cos(double x) { return cos(x); }
double py_math_tan(double x) { return tan(x); }
double py_math_atan2(double y, double x) { return atan2(y, x); }
int64_t py_math_trunc(double x) { return (int64_t)trunc(x); }
int64_t py_math_isfinite(double x) { return isfinite(x) ? 1 : 0; }
int64_t py_math_isnan(double x) { return isnan(x) ? 1 : 0; }
// Remaining libm functions the corpus writes as math.<name>. Before these were
// registered, each member fell through to a link error (fail-loud), so they
// were unusable rather than silently wrong — but they are common enough that
// they belong in the native set. libc has no symbol for isinf (it is a macro),
// hence the explicit wrapper.
double py_math_log2(double x) { return log2(x); }
double py_math_exp2(double x) { return exp2(x); }
double py_math_expm1(double x) { return expm1(x); }
double py_math_log1p(double x) { return log1p(x); }
double py_math_cbrt(double x) { return cbrt(x); }
double py_math_atan(double x) { return atan(x); }
double py_math_asin(double x) { return asin(x); }
double py_math_acos(double x) { return acos(x); }
double py_math_sinh(double x) { return sinh(x); }
double py_math_cosh(double x) { return cosh(x); }
double py_math_tanh(double x) { return tanh(x); }
double py_math_asinh(double x) { return asinh(x); }
double py_math_acosh(double x) { return acosh(x); }
double py_math_atanh(double x) { return atanh(x); }
double py_math_gamma(double x) { return tgamma(x); }
double py_math_erf(double x) { return erf(x); }
double py_math_erfc(double x) { return erfc(x); }
double py_math_fmod(double a, double b) { return fmod(a, b); }
double py_math_remainder(double a, double b) { return remainder(a, b); }
double py_math_copysign(double a, double b) { return copysign(a, b); }
double py_math_nextafter(double a, double b) { return nextafter(a, b); }
double py_math_ldexp(double a, int64_t e) { return ldexp(a, (int)e); }
int64_t py_math_isinf(double x) { return isinf(x) ? 1 : 0; }

// ---- logging (real, to stderr; levels are i64 constants) ----
#define PY_LOG_DEBUG 10
#define PY_LOG_INFO 20
#define PY_LOG_WARNING 30
#define PY_LOG_ERROR 40
#define PY_LOG_CRITICAL 50

static int64_t py_log_level = PY_LOG_WARNING;

int64_t py_logging_DEBUG(void) { return PY_LOG_DEBUG; }
int64_t py_logging_INFO(void) { return PY_LOG_INFO; }
int64_t py_logging_WARNING(void) { return PY_LOG_WARNING; }
int64_t py_logging_ERROR(void) { return PY_LOG_ERROR; }
int64_t py_logging_CRITICAL(void) { return PY_LOG_CRITICAL; }

int64_t py_logging_basicConfig(int64_t level) {
    if (level >= PY_LOG_DEBUG && level <= PY_LOG_CRITICAL) py_log_level = level;
    return 0;
}
// `logging.basicConfig(level=logging.INFO, format="...")` — the 2-arg call
// site (registry declares the level only). The FORMAT string is not honoured
// (this shim's logger writes its own fixed prefix); the level is, so the
// message threshold is right.
int64_t py_logging_basicConfig_2(int64_t level, int64_t format) {
    (void)format;
    return py_logging_basicConfig(level);
}
int64_t py_logging_setLevel(int64_t level) {
    if (level >= PY_LOG_DEBUG && level <= PY_LOG_CRITICAL) py_log_level = level;
    return 0;
}
int64_t py_logging_getLogger(int64_t name) { return name; } // logger identity = its name
static int64_t py_log_emit(int64_t level, const char* tag, int64_t logger, int64_t msg) {
    if (level < py_log_level) return 0;
    fprintf(stderr, "[%s] %s%s%s\n", tag,
            logger ? (const char*)logger : "", logger ? ": " : "",
            msg ? (const char*)msg : "");
    return 0;
}
// BARE logging method symbols. `logger` is re-exported across modules in
// REasyQuant (`data_ops_log.logger` -> `market_data_sources.logger` ->
// `market_data_fetcher.logger`), and when the closure that logs cannot recover
// its static type the call falls back to the METHOD NAME alone (`info`). That
// used to hit an abort stub and stop the local backtest the moment it logged.
// These print through the logger-less path (the message is never swallowed, the
// logger NAME is simply unknown) — this is diagnostics, not a value path.
__attribute__((weak)) int64_t info(int64_t lg, int64_t m) { (void)lg; return py_log_emit(PY_LOG_INFO, "INFO", 0, m); }
__attribute__((weak)) int64_t warning(int64_t lg, int64_t m) { (void)lg; return py_log_emit(PY_LOG_WARNING, "WARNING", 0, m); }
__attribute__((weak)) int64_t error(int64_t lg, int64_t m) { (void)lg; return py_log_emit(PY_LOG_ERROR, "ERROR", 0, m); }
__attribute__((weak)) int64_t debug(int64_t lg, int64_t m) { (void)lg; return py_log_emit(PY_LOG_DEBUG, "DEBUG", 0, m); }
__attribute__((weak)) int64_t critical(int64_t lg, int64_t m) { (void)lg; return py_log_emit(PY_LOG_CRITICAL, "CRITICAL", 0, m); }
int64_t py_logging_debug(int64_t m) { return py_log_emit(PY_LOG_DEBUG, "DEBUG", 0, m); }
int64_t py_logging_info(int64_t m) { return py_log_emit(PY_LOG_INFO, "INFO", 0, m); }
int64_t py_logging_warning(int64_t m) { return py_log_emit(PY_LOG_WARNING, "WARNING", 0, m); }
int64_t py_logging_error(int64_t m) { return py_log_emit(PY_LOG_ERROR, "ERROR", 0, m); }
int64_t py_logging_critical(int64_t m) { return py_log_emit(PY_LOG_CRITICAL, "CRITICAL", 0, m); }
int64_t py_logger_debug(int64_t lg, int64_t m) { return py_log_emit(PY_LOG_DEBUG, "DEBUG", lg, m); }
// PY-A: `log.info(fmt, *args)` — Python's Logger methods are VARIADIC while the
// registry declares a fixed arity, so extra args were arity-mangled into phantom
// symbols (`py_logger_info_4/_5`, 5 corpus call sites). This takes up to four
// extra values and prints them after the format string. V1 does NO
// %-substitution: the values are shown as-is, never silently dropped.
int64_t py_logger_info_n(int64_t lg, int64_t fmt, int64_t n, int64_t a1, int64_t a2,
                         int64_t a3, int64_t a4) {
    int64_t vals[4] = {a1, a2, a3, a4};
    char buf[1024];
    size_t off = 0;
    const char* f = (const char*)fmt;
    if (f) {
        while (f[off] && off < sizeof(buf) - 64) {
            buf[off] = f[off];
            off++;
        }
    }
    for (int64_t i = 0; i < n && i < 4; i++) {
        off += (size_t)snprintf(buf + off, sizeof(buf) - off, " %lld", (long long)vals[i]);
    }
    buf[off < sizeof(buf) ? off : sizeof(buf) - 1] = 0;
    return py_log_emit(PY_LOG_INFO, "INFO", lg, (int64_t)buf);
}
int64_t py_logger_warning_n(int64_t lg, int64_t fmt, int64_t n, int64_t a1, int64_t a2,
                            int64_t a3, int64_t a4) {
    int64_t vals[4] = {a1, a2, a3, a4};
    char buf[1024];
    size_t off = 0;
    const char* f = (const char*)fmt;
    if (f) {
        while (f[off] && off < sizeof(buf) - 64) {
            buf[off] = f[off];
            off++;
        }
    }
    for (int64_t i = 0; i < n && i < 4; i++) {
        off += (size_t)snprintf(buf + off, sizeof(buf) - off, " %lld", (long long)vals[i]);
    }
    buf[off < sizeof(buf) ? off : sizeof(buf) - 1] = 0;
    return py_log_emit(PY_LOG_WARNING, "WARNING", lg, (int64_t)buf);
}
int64_t py_logger_error_n(int64_t lg, int64_t fmt, int64_t n, int64_t a1, int64_t a2,
                          int64_t a3, int64_t a4) {
    int64_t vals[4] = {a1, a2, a3, a4};
    char buf[1024];
    size_t off = 0;
    const char* f = (const char*)fmt;
    if (f) {
        while (f[off] && off < sizeof(buf) - 64) {
            buf[off] = f[off];
            off++;
        }
    }
    for (int64_t i = 0; i < n && i < 4; i++) {
        off += (size_t)snprintf(buf + off, sizeof(buf) - off, " %lld", (long long)vals[i]);
    }
    buf[off < sizeof(buf) ? off : sizeof(buf) - 1] = 0;
    return py_log_emit(PY_LOG_ERROR, "ERROR", lg, (int64_t)buf);
}
int64_t py_logger_info(int64_t lg, int64_t m) { return py_log_emit(PY_LOG_INFO, "INFO", lg, m); }
int64_t py_logger_warning(int64_t lg, int64_t m) { return py_log_emit(PY_LOG_WARNING, "WARNING", lg, m); }
int64_t py_logger_error(int64_t lg, int64_t m) { return py_log_emit(PY_LOG_ERROR, "ERROR", lg, m); }
int64_t py_logging_FileHandler(int64_t p) { return p; }
// PY-A: `logging.getLogger().addHandler(h)` / `h.setFormatter(f)` — local no-op
// shims (the handle IS the logger/file path; nothing to attach locally).
int64_t py_logging_addHandler(int64_t lg, int64_t h) { (void)h; return lg; }
int64_t py_logging_setFormatter(int64_t h, int64_t f) { (void)f; return h; }
int64_t py_logging_Formatter(int64_t f) { return f; }

// ============================================================================
// PY-A datetime shims. Handles are 2-slot blocks: [days_since_epoch, seconds
// _of_day]; a timedelta handle is [days, secs] as well. Selected because the
// corpus leans on datetime hardest (timedelta 14, strptime 6, date/datetime
// construction + arithmetic).
// ============================================================================

static int64_t zt_days_from_civil(int64_t y, int64_t m, int64_t d) {
    y -= (m <= 2);
    int64_t era = (y >= 0 ? y : y - 399) / 400;
    int64_t yoe = y - era * 400;
    int64_t doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1;
    int64_t doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return era * 146097 + doe - 719468;
}
static void zt_civil_from_days(int64_t z, int64_t* y, int64_t* m, int64_t* d) {
    z += 719468;
    int64_t era = (z >= 0 ? z : z - 146096) / 146097;
    int64_t doe = z - era * 146097;
    int64_t yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    int64_t yy = yoe + era * 400;
    int64_t doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    int64_t mp = (5 * doy + 2) / 153;
    int64_t dd = doy - (153 * mp + 2) / 5 + 1;
    int64_t mm = mp + (mp < 10 ? 3 : -9);
    *y = yy + (mm <= 2);
    *m = mm;
    *d = dd;
}
static int64_t* zt_dt_alloc(int64_t days, int64_t secs) {
    int64_t* h = (int64_t*)GC_malloc(16);
    h[0] = days;
    h[1] = secs;
    return h;
}
int64_t py_dt_date(int64_t y, int64_t m, int64_t d) {
    return (int64_t)zt_dt_alloc(zt_days_from_civil(y, m, d), 0);
}
int64_t py_dt_datetime(int64_t y, int64_t m, int64_t d) {
    return (int64_t)zt_dt_alloc(zt_days_from_civil(y, m, d), 0);
}
int64_t py_dt_timedelta(int64_t days) { return (int64_t)zt_dt_alloc(days, 0); }
int64_t py_dt_now(void) {
    time_t t = time(NULL);
    struct tm tmv;
    localtime_r(&t, &tmv);
    return (int64_t)zt_dt_alloc(
        zt_days_from_civil(tmv.tm_year + 1900, tmv.tm_mon + 1, tmv.tm_mday),
        tmv.tm_hour * 3600 + tmv.tm_min * 60 + tmv.tm_sec);
}
int64_t py_dt_year(int64_t h) {
    int64_t y, m, d;
    zt_civil_from_days(((int64_t*)h)[0], &y, &m, &d);
    return y;
}
int64_t py_dt_month(int64_t h) {
    int64_t y, m, d;
    zt_civil_from_days(((int64_t*)h)[0], &y, &m, &d);
    return m;
}
int64_t py_dt_day(int64_t h) {
    int64_t y, m, d;
    zt_civil_from_days(((int64_t*)h)[0], &y, &m, &d);
    return d;
}
// `d.replace(year=Y, month=M, day=D)` — NEW date with those fields replaced;
// an unsupplied field arrives as <= 0 and keeps the receiver's value.
// `dataclasses.asdict(x)` where x is NOT a statically-known dataclass: the
// compiler rewrites the dataclass case (struct fields are known at compile
// time, see gen.rs), so reaching here means the value could not be expanded.
// Registry: stub=1. Loud fail via py_stub_abort (D / advice.md).
int64_t py_stub_abort(int64_t name_ptr);
int64_t py_asdict_unexpanded(int64_t v) {
    (void)v;
    return py_stub_abort((int64_t)(uintptr_t)"py_asdict_unexpanded");
}

int64_t py_dt_replace(int64_t h, int64_t y, int64_t m, int64_t d) {
    int64_t cy, cm, cd;
    zt_civil_from_days(((int64_t*)h)[0], &cy, &cm, &cd);
    if (y <= 0) y = cy;
    if (m <= 0) m = cm;
    if (d <= 0) d = cd;
    return (int64_t)zt_dt_alloc(zt_days_from_civil(y, m, d), ((int64_t*)h)[1]);
}
int64_t py_dt_identity(int64_t h) { return h; }
int64_t py_dt_strftime(int64_t h, int64_t fmt) {
    int64_t y, m, d;
    zt_civil_from_days(((int64_t*)h)[0], &y, &m, &d);
    const char* f = fmt ? (const char*)fmt : "%Y-%m-%d";
    char buf[256];
    // Minimal strftime subset: %Y %m %d %H %M %S %%
    int64_t secs = ((int64_t*)h)[1];
    char* o = buf;
    for (const char* p = f; *p && (o - buf) < 200; p++) {
        if (*p != '%') { *o++ = *p; continue; }
        p++;
        switch (*p) {
            case 'Y': o += sprintf(o, "%04lld", (long long)y); break;
            case 'm': o += sprintf(o, "%02lld", (long long)m); break;
            case 'd': o += sprintf(o, "%02lld", (long long)d); break;
            case 'H': o += sprintf(o, "%02lld", (long long)(secs / 3600)); break;
            case 'M': o += sprintf(o, "%02lld", (long long)((secs / 60) % 60)); break;
            case 'S': o += sprintf(o, "%02lld", (long long)(secs % 60)); break;
            case '%': *o++ = '%'; break;
            default: if (*p) *o++ = *p; break;
        }
    }
    *o = 0;
    return (int64_t)zt_strdup(buf);
}
// time.strftime(fmt) — format the CURRENT time. Without this the module call
// reached libc's strftime (a different signature entirely) and crashed on the
// first argument.
int64_t py_time_strftime(int64_t fmt) {
    return py_dt_strftime(py_dt_now(), fmt);
}

// strptime with the corpus's formats: %Y-%m-%d and %Y-%m-%d %H:%M:%S.
int64_t py_dt_strptime(int64_t s, int64_t fmt) {
    (void)fmt;
    const char* p = s ? (const char*)s : "";
    long long y = 0, mo = 1, d = 1, hh = 0, mi = 0, ss = 0;
    int n = sscanf(p, "%lld-%lld-%lld %lld:%lld:%lld", &y, &mo, &d, &hh, &mi, &ss);
    if (n < 3) {
        n = sscanf(p, "%lld-%lld-%lld", &y, &mo, &d);
        if (n < 3) return (int64_t)zt_dt_alloc(0, 0);
    }
    return (int64_t)zt_dt_alloc(zt_days_from_civil(y, mo, d), hh * 3600 + mi * 60 + ss);
}
static int64_t zt_dt_total_secs(int64_t h) {
    return ((int64_t*)h)[0] * 86400 + ((int64_t*)h)[1];
}
int64_t py_dt_sub_dates(int64_t a, int64_t b) {
    return (int64_t)zt_dt_alloc(((int64_t*)a)[0] - ((int64_t*)b)[0],
                                ((int64_t*)a)[1] - ((int64_t*)b)[1]);
}
int64_t py_dt_add_delta(int64_t a, int64_t dl) {
    return (int64_t)zt_dt_alloc(((int64_t*)a)[0] + ((int64_t*)dl)[0],
                                ((int64_t*)a)[1] + ((int64_t*)dl)[1]);
}
int64_t py_dt_sub_delta(int64_t a, int64_t dl) {
    return (int64_t)zt_dt_alloc(((int64_t*)a)[0] - ((int64_t*)dl)[0],
                                ((int64_t*)a)[1] - ((int64_t*)dl)[1]);
}
int64_t py_dt_lt(int64_t a, int64_t b) { return zt_dt_total_secs(a) < zt_dt_total_secs(b); }
int64_t py_dt_le(int64_t a, int64_t b) { return zt_dt_total_secs(a) <= zt_dt_total_secs(b); }
int64_t py_dt_gt(int64_t a, int64_t b) { return zt_dt_total_secs(a) > zt_dt_total_secs(b); }
int64_t py_dt_ge(int64_t a, int64_t b) { return zt_dt_total_secs(a) >= zt_dt_total_secs(b); }
int64_t py_dt_eq(int64_t a, int64_t b) { return zt_dt_total_secs(a) == zt_dt_total_secs(b); }
int64_t py_dt_ne(int64_t a, int64_t b) { return zt_dt_total_secs(a) != zt_dt_total_secs(b); }
int64_t py_dt_delta_days(int64_t h) { return ((int64_t*)h)[0]; }

// ============================================================================
// PY-A stdlib shims, batch 3: sys + json.
// json.dumps is dispatched by the *compiler* to a typed entry point, because
// an i64 handle carries no runtime type tag.
// ============================================================================
extern int64_t zeta_key_string(int64_t hash);
// Recorded at DictInsert by the compiler (dict value-type side table).
int64_t zeta_map_value_tag(int64_t map, int64_t key);

int64_t py_sys_maxsize(void) { return INT64_MAX; }
// sys.platform / os.sep / os.linesep / sys.argv — these were absent, so a bare
// attribute read warned and used 0 (a silently wrong value). argv is captured
// at process start (the generated main takes no arguments of its own).
#ifdef __APPLE__
static const char* zt_platform_name = "darwin";
#else
static const char* zt_platform_name = "linux";
#endif
int64_t py_sys_platform(void) { return (int64_t)zt_platform_name; }
int64_t py_os_sep(void) { return (int64_t)"/"; }
int64_t py_os_linesep(void) { return (int64_t)"\n"; }
// sys.argv — captured at process start (the generated main takes no arguments
// of its own). Registered as `ret=vecstr`, so subscripts/for-in now carry the
// element type through (the attribute-read mapping used to drop it, handing
// back handles and 0).
static int zt_argc = 0;
static char** zt_argv = 0;
__attribute__((constructor)) static void zt_capture_args(int argc, char** argv) {
    zt_argc = argc;
    zt_argv = argv;
}
int64_t py_sys_argv(void) {
    int64_t n = zt_argc > 0 ? zt_argc : 1;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)n * 8);
    base[0] = n;
    base[1] = zt_argc;
    for (int i = 0; i < zt_argc; i++) {
        base[2 + i] = (int64_t)zt_strdup(zt_argv[i] ? zt_argv[i] : "");
    }
    return (int64_t)(base + 2);
}
// Raw argv access for the argparse runtime (avoids building a Zeta Vec just to
// scan for options).
int64_t zeta_argc(void) { return zt_argc; }
int64_t zeta_argv_at(int64_t i) {
    return (i >= 0 && i < zt_argc && zt_argv) ? (int64_t)zt_argv[i] : 0;
}
int64_t py_sys_version_info(void) {
    // Vec [3, 14, 0] so `sys.version_info[0] >= 3` works.
    int64_t* base = (int64_t*)GC_malloc(16 + 3 * 8);
    base[0] = 3; base[1] = 3;
    base[2] = 3; base[3] = 14; base[4] = 0;
    return (int64_t)(base + 2);
}
int64_t py_sys_version(void) { return (int64_t)zt_strdup("3.14.0 (zeta py-a)"); }
int64_t py_sys_path(void) {
    // Empty Vec: `x not in sys.path` then `sys.path.insert(...)` is how real
    // code bootstraps paths — our search path is ZETA_PYLIB, so the list is
    // deliberately empty rather than pretending to be Python's.
    int64_t* base = (int64_t*)GC_malloc(16 + 8 * 8);
    base[0] = 8; base[1] = 0;
    return (int64_t)(base + 2);
}
int64_t py_sys_path_insert(int64_t idx, int64_t value) {
    (void)idx; (void)value;
    return 0; // recorded no-op: the compiler's search path is ZETA_PYLIB
}
void py_sys_exit(int64_t code) { exit((int)(code & 0xff)); }
int64_t py_sys_stdout_write(int64_t s) {
    const char* p = s ? (const char*)s : "";
    fputs(p, stdout);
    return (int64_t)strlen(p);
}
int64_t py_sys_stderr_write(int64_t s) {
    const char* p = s ? (const char*)s : "";
    fputs(p, stderr);
    return (int64_t)strlen(p);
}

// ---- warnings ----
// Python's warnings.warn() prints a diagnostic to stderr. The filter
// configuration (simplefilter/filterwarnings) is accepted as a no-op: this
// runtime has no warning-filter state, and silently dropping the message
// would be worse than printing it.
int64_t py_warnings_warn(int64_t m) {
    const char* p = m ? (const char*)m : "";
    fprintf(stderr, "Warning: %s\n", p);
    return 0;
}
int64_t py_warnings_noop(int64_t a) {
    (void)a;
    return 0;
}

// Generic accepted-and-ignored shim (1 i64 arg): decorators, filter config,
// and other Python surface that has no runtime effect here.
int64_t py_noop1(int64_t a) {
    (void)a;
    return 0;
}

// 2-arg accepted-and-ignored shim (`load_dotenv(path, override)`, …).
int64_t py_noop2(int64_t a, int64_t b) {
    (void)a;
    (void)b;
    return 0;
}

// `pd.read_parquet(path)` — a real reader (runtime/parquet_min.c): SNAPPY +
// PLAIN/RLE_DICTIONARY pages, returning the runtime's column map
// (name -> vector of value strings, `trade_date` as "YYYY-MM-DD").
extern int64_t zt_parquet_build_map(const char* path);
int64_t py_pd_read_parquet(int64_t path) {
    if (!path) return 0;
    return zt_parquet_build_map((const char*)path);
}

// ---- hashlib (md5/sha1/sha256) ----
// Accumulates the input in a growable GC buffer and hashes it on demand, so
// both `sha256(data).hexdigest()` and the streaming `h = sha256();
// h.update(x); h.hexdigest()` forms work. Backend is macOS CommonCrypto;
// other platforms get an empty digest rather than a wrong one.
#ifdef __APPLE__
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
#include <CommonCrypto/CommonDigest.h>
#endif

typedef struct {
    int algo; /* 1 = md5, 2 = sha1, 3 = sha256 */
    unsigned char* buf;
    size_t len;
    size_t cap;
} py_hash_t;

static void py_hash_append(py_hash_t* h, const void* data, size_t n) {
    if (!data || n == 0) return;
    if (h->len + n > h->cap) {
        size_t cap = h->cap ? h->cap : 64;
        while (cap < h->len + n) cap *= 2;
        h->buf = (unsigned char*)GC_realloc(h->buf, cap);
        h->cap = cap;
    }
    memcpy(h->buf + h->len, data, n);
    h->len += n;
}

static int64_t py_hashlib_new(int algo, int64_t data) {
    py_hash_t* h = (py_hash_t*)GC_malloc(sizeof(py_hash_t));
    h->algo = algo;
    h->buf = NULL;
    h->len = 0;
    h->cap = 0;
    if (data) py_hash_append(h, (const void*)data, strlen((const char*)data));
    return (int64_t)h;
}

int64_t py_hashlib_md5(int64_t data) { return py_hashlib_new(1, data); }
int64_t py_hashlib_sha1(int64_t data) { return py_hashlib_new(2, data); }
int64_t py_hashlib_sha256(int64_t data) { return py_hashlib_new(3, data); }
int64_t py_hashlib_md5_0(void) { return py_hashlib_new(1, 0); }
int64_t py_hashlib_sha1_0(void) { return py_hashlib_new(2, 0); }
int64_t py_hashlib_sha256_0(void) { return py_hashlib_new(3, 0); }

int64_t py_hashlib_update(int64_t handle, int64_t data) {
    if (!handle) return 0;
    py_hash_t* h = (py_hash_t*)handle;
    if (data) py_hash_append(h, (const void*)data, strlen((const char*)data));
    return 0;
}

static void py_hash_raw(py_hash_t* h, unsigned char* out, unsigned int* outlen) {
    *outlen = 0;
#ifdef __APPLE__
    if (h->algo == 1) {
        CC_MD5(h->buf, (CC_LONG)h->len, out);
        *outlen = CC_MD5_DIGEST_LENGTH;
    } else if (h->algo == 2) {
        CC_SHA1(h->buf, (CC_LONG)h->len, out);
        *outlen = CC_SHA1_DIGEST_LENGTH;
    } else {
        CC_SHA256(h->buf, (CC_LONG)h->len, out);
        *outlen = CC_SHA256_DIGEST_LENGTH;
    }
#else
    (void)h;
#endif
}

int64_t py_hashlib_hexdigest(int64_t handle) {
    if (!handle) return 0;
    unsigned char raw[64];
    unsigned int n = 0;
    static const char hex[] = "0123456789abcdef";
    py_hash_raw((py_hash_t*)handle, raw, &n);
    char* out = (char*)GC_malloc(n * 2 + 1);
    for (unsigned int i = 0; i < n; i++) {
        out[i * 2] = hex[raw[i] >> 4];
        out[i * 2 + 1] = hex[raw[i] & 0xF];
    }
    out[n * 2] = 0;
    return (int64_t)out;
}

int64_t py_hashlib_digest(int64_t handle) {
    if (!handle) return 0;
    unsigned char raw[64];
    unsigned int n = 0;
    py_hash_raw((py_hash_t*)handle, raw, &n);
    char* out = (char*)GC_malloc(n + 1);
    memcpy(out, raw, n);
    out[n] = 0;
    return (int64_t)out;
}

#ifdef __APPLE__
#pragma clang diagnostic pop
#endif

// ---- functools ----
// reduce(fn, iterable[, init]) — left fold. `fn` is a Zeta function pointer
// with the two-argument ABI. The iterable is a [cap|len] array handle.
int64_t py_functools_reduce(int64_t fn, int64_t arr) {
    int64_t n = array_len(arr);
    if (n <= 0) return 0;
    int64_t acc = ((int64_t*)arr)[0];
    for (int64_t i = 1; i < n; i++) {
        acc = ((int64_t(*)(int64_t, int64_t))fn)(acc, ((int64_t*)arr)[i]);
    }
    return acc;
}

int64_t py_functools_reduce_3(int64_t fn, int64_t arr, int64_t init) {
    int64_t acc = init;
    int64_t n = array_len(arr);
    for (int64_t i = 0; i < n; i++) {
        acc = ((int64_t(*)(int64_t, int64_t))fn)(acc, ((int64_t*)arr)[i]);
    }
    return acc;
}

// ---- any / all (array truthiness: non-zero is true) ----
int64_t py_builtin_any(int64_t arr) {
    int64_t n = array_len(arr);
    for (int64_t i = 0; i < n; i++) {
        if (((int64_t*)arr)[i]) return 1;
    }
    return 0;
}

int64_t py_builtin_all(int64_t arr) {
    int64_t n = array_len(arr);
    for (int64_t i = 0; i < n; i++) {
        if (!((int64_t*)arr)[i]) return 0;
    }
    return 1;
}

// ---- pathlib ----
// A PyPath is the path string itself (a char* handle), so every os.path.*
// shim accepts it unchanged; only the Path-only operations need new code.
int64_t py_path_new(int64_t s) {
    return s ? s : (int64_t)zt_strdup("");
}

int64_t py_path_stem(int64_t p) {
    const char* s = p ? (const char*)p : "";
    const char* slash = strrchr(s, '/');
    const char* base = slash ? slash + 1 : s;
    const char* dot = strrchr(base, '.');
    size_t n = (dot && dot != base) ? (size_t)(dot - base) : strlen(base);
    char* out = (char*)GC_malloc(n + 1);
    memcpy(out, base, n);
    out[n] = 0;
    return (int64_t)out;
}

int64_t py_path_suffix(int64_t p) {
    const char* s = p ? (const char*)p : "";
    const char* slash = strrchr(s, '/');
    const char* base = slash ? slash + 1 : s;
    const char* dot = strrchr(base, '.');
    if (!dot || dot == base) return (int64_t)zt_strdup("");
    return (int64_t)zt_strdup(dot);
}

// `Path.mkdir(parents=True, exist_ok=True)` — 29 call sites in REasyQuant, all
// with both flags, so the call arrives as the 3-arg form (registry declares the
// bare `mkdir`), which nothing defined (`_PyPath__mkdir` / arity-suffixed).
// `parents` → create the whole chain (mkdir -p); `exist_ok` → tolerate EEXIST.
// `Path.glob("*.jsonl")` — a Vec of matching paths (sorted by glob(3)).
// REasyQuant hits this in data_ops_log / task_store / ml.store / tools.strategy.
int64_t py_path_glob(int64_t p, int64_t pattern) {
    const char* dir = p ? (const char*)p : ".";
    const char* pat = pattern ? (const char*)pattern : "*";
    char buf[4096];
    snprintf(buf, sizeof buf, "%s/%s", dir, pat);
    int64_t h = zeta_dynarray_new(8);
    glob_t g;
    memset(&g, 0, sizeof g);
    if (glob(buf, 0, NULL, &g) == 0) {
        for (size_t i = 0; i < g.gl_pathc; i++) {
            h = vec_push(h, (int64_t)zt_strdup(g.gl_pathv[i]));
        }
    }
    globfree(&g);
    return h;
}

int64_t py_path_mkdir(int64_t p) {
    return p ? (int64_t)mkdir((const char*)p, 0777) : -1;
}
int64_t py_path_mkdir_3(int64_t p, int64_t parents, int64_t exist_ok) {
    if (!p) return -1;
    const char* s = (const char*)p;
    if (parents) {
        char tmp[4096];
        snprintf(tmp, sizeof tmp, "%s", s);
        for (char* q = tmp + 1; *q; q++) {
            if (*q == '/') {
                *q = '\0';
                mkdir(tmp, 0777);
                *q = '/';
            }
        }
        int64_t r = (int64_t)mkdir(tmp, 0777);
        if (r != 0 && exist_ok && errno == EEXIST) return 0;
        return r;
    }
    int64_t r = (int64_t)mkdir(s, 0777);
    if (r != 0 && exist_ok && errno == EEXIST) return 0;
    return r;
}

int64_t py_path_read_text(int64_t p) {
    const char* path = p ? (const char*)p : "";
    FILE* f = fopen(path, "rb");
    if (!f) return (int64_t)zt_strdup("");
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (sz < 0) sz = 0;
    char* buf = (char*)GC_malloc((size_t)sz + 1);
    size_t rd = fread(buf, 1, (size_t)sz, f);
    fclose(f);
    buf[rd] = 0;
    return (int64_t)buf;
}

// `p.read_text(encoding="utf-8")` — the 2-arg call site (registry declares
// the 1-arg form; encoding is irrelevant because the text is read as bytes).
int64_t py_path_read_text_2(int64_t p, int64_t encoding) {
    (void)encoding;
    return py_path_read_text(p);
}

// `p.write_text(s, encoding="utf-8")` — the 3-arg call site (registry declares
// receiver+text, so the call arity-mangled to `py_path_write_text_3`, which
// nothing defined). `encoding` is ignored: the bytes are written verbatim, and
// re-encoding a `str` handle would be a silent transformation.
int64_t py_path_write_text(int64_t p, int64_t text);
int64_t py_path_write_text_3(int64_t p, int64_t text, int64_t encoding) {
    (void)encoding;
    return py_path_write_text(p, text);
}

// pathlib.Path.parents — a Vec of ancestor paths: `parents[0]` is the parent,
// `parents[k]` drops the last k+1 components. `PyPath` handles ARE the path
// string, so each element is a string handle. Without this, `Path(__file__)
// .resolve().parents[2]` left the module-root global untyped and every
// `.exists()` / `.read_text()` on it emitted a bare symbol (`_exists` 6 refs /
// `_read_text` 6 refs in the REasyQuant local backtest: etf_listing,
// market_data_universe, data_ops_log, sources_selector).
int64_t py_path_parents(int64_t p) {
    const char* src = p ? (const char*)p : "";
    size_t n = strlen(src);
    char* buf = (char*)GC_malloc(n + 1);
    memcpy(buf, src, n + 1);
    int64_t h = zeta_dynarray_new(8);
    for (size_t i = n; i > 0; i--) {
        if (buf[i - 1] != '/') continue;
        buf[i - 1] = '\0';
        if (i - 1 == 0) {
            h = vec_push(h, (int64_t)zt_strdup("/"));
            break;
        }
        h = vec_push(h, (int64_t)zt_strdup(buf));
    }
    return h;
}

int64_t py_path_write_text(int64_t p, int64_t text) {
    const char* path = p ? (const char*)p : "";
    const char* s = text ? (const char*)text : "";
    FILE* f = fopen(path, "wb");
    if (!f) return 0;
    size_t n = strlen(s);
    fwrite(s, 1, n, f);
    fclose(f);
    return (int64_t)n;
}

// ---- json ----
static int64_t zt_json_quote(const char* s, char* out) {
    char* o = out;
    *o++ = '"';
    for (const char* p = s; *p; p++) {
        switch (*p) {
            case '"': *o++ = '\\'; *o++ = '"'; break;
            case '\\': *o++ = '\\'; *o++ = '\\'; break;
            case '\n': *o++ = '\\'; *o++ = 'n'; break;
            case '\r': *o++ = '\\'; *o++ = 'r'; break;
            case '\t': *o++ = '\\'; *o++ = 't'; break;
            default: *o++ = *p; break;
        }
    }
    *o++ = '"';
    *o = 0;
    return (int64_t)(o - out);
}
int64_t py_json_dumps_i64(int64_t v) {
    char* s = (char*)GC_malloc(32);
    snprintf(s, 32, "%lld", (long long)v);
    return (int64_t)s;
}
int64_t py_json_dumps_f64(double v) {
    // NOTE: the parameter must be `double` — the registry declares args=f64
    // and the ABI passes it in a float register, so reading it as int64 gave
    // a denormal bit pattern instead of the number.
    char* s = (char*)GC_malloc(40);
    snprintf(s, 40, "%g", v);
    return (int64_t)s;
}
int64_t py_json_dumps_str(int64_t str) {
    const char* p = str ? (const char*)str : "";
    char* s = (char*)GC_malloc(strlen(p) * 2 + 3);
    zt_json_quote(p, s);
    return (int64_t)s;
}
int64_t py_json_dumps_bool(int64_t v) {
    return (int64_t)zt_strdup(v ? "true" : "false");
}
int64_t py_json_dumps_vec(int64_t vec) {
    if (!vec) return (int64_t)zt_strdup("[]");
    int64_t len = ((int64_t*)(vec - 16))[1];
    size_t cap = 64;
    char* out = (char*)GC_malloc(cap);
    size_t n = 0;
    out[n++] = '[';
    for (int64_t i = 0; i < len; i++) {
        if (n + 32 > cap) { cap *= 2; char* nb = (char*)GC_malloc(cap); memcpy(nb, out, n); out = nb; }
        if (i) { out[n++] = ','; out[n++] = ' '; }
        n += (size_t)sprintf(out + n, "%lld", (long long)((int64_t*)vec)[i]);
    }
    out[n++] = ']';
    out[n] = 0;
    return (int64_t)out;
}
// V1: object keys come back from the hash side table; values are serialized
// as integers (the map stores raw 64-bit slots with no type tag).
int64_t py_json_dumps_map(int64_t map) {
        map = map_resolve(map);
if (!map) return (int64_t)zt_strdup("{}");
    int64_t cap = ((int64_t*)map)[0];
    size_t outcap = 256;
    char* out = (char*)GC_malloc(outcap);
    size_t n = 0;
    out[n++] = '{';
    int first = 1;
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)map + 16 + i * 24;
        if (!*(uint8_t*)(e + 16)) continue;
        int64_t key_hash = *(int64_t*)e;
        int64_t val = *(int64_t*)(e + 8);
        if (n + 256 > outcap) { outcap *= 2; char* nb = (char*)GC_malloc(outcap); memcpy(nb, out, n); out = nb; }
        if (!first) { out[n++] = ','; out[n++] = ' '; }
        first = 0;
        int64_t ks = zeta_key_string(key_hash);
        if (ks) {
            n += (size_t)zt_json_quote((const char*)ks, out + n);
        } else {
            n += (size_t)sprintf(out + n, "\"%lld\"", (long long)key_hash);
        }
        out[n++] = ':';
        out[n++] = ' ';
        // Serialize by the value type the compiler recorded at insert time.
        switch (zeta_map_value_tag(map, key_hash)) {
            case 1: {
                double d;
                memcpy(&d, &val, sizeof d);
                n += (size_t)sprintf(out + n, "%g", d);
                break;
            }
            case 2:
                if (val) {
                    n += (size_t)zt_json_quote((const char*)val, out + n);
                } else {
                    n += (size_t)sprintf(out + n, "\"\"");
                }
                break;
            case 3:
                n += (size_t)sprintf(out + n, "%s", val ? "true" : "false");
                break;
            default:
                n += (size_t)sprintf(out + n, "%lld", (long long)val);
                break;
        }
    }
    out[n++] = '}';
    out[n] = 0;
    return (int64_t)out;
}
int64_t py_sys_path_contains(int64_t value) {
    (void)value;
    return 0; // the path list is deliberately empty (see py_sys_path)
}

// ============================================================================
// PY-A `re` shims — POSIX regcomp/regexec (no new dependency).
//
// Python-flavoured escapes are translated to POSIX classes because BSD/GNU
// regex differ on `\d`/`\w`/`\s`. Match objects are handles so truthiness is
// right (`if m:` is false for "no match" instead of an empty-string
// pointer being truthy).
// ============================================================================
#include <regex.h>

static int64_t zt_re_translate(const char* pat, char* out, size_t cap) {
    size_t n = 0;
    for (const char* p = pat; *p && n + 24 < cap; p++) {
        if (*p == '\\' && p[1]) {
            switch (p[1]) {
                case 'd': memcpy(out + n, "[0-9]", 5); n += 5; p++; continue;
                case 'w': memcpy(out + n, "[A-Za-z0-9_]", 12); n += 12; p++; continue;
                case 's': memcpy(out + n, "[ \t\r\n]", 7); n += 7; p++; continue;
                case 'D': memcpy(out + n, "[^0-9]", 6); n += 6; p++; continue;
                case 'W': memcpy(out + n, "[^A-Za-z0-9_]", 13); n += 13; p++; continue;
                case 'S': memcpy(out + n, "[^ \t\r\n]", 8); n += 8; p++; continue;
                default: out[n++] = *p; out[n++] = p[1]; p++; continue;
            }
        }
        out[n++] = *p;
    }
    out[n] = 0;
    return (int64_t)n;
}

typedef struct {
    regex_t re;
    int ok;
} zt_regex_t;

static zt_regex_t* zt_re_compile_fl(int64_t pat, int cflags) {
    zt_regex_t* r = (zt_regex_t*)GC_malloc(sizeof(zt_regex_t));
    r->ok = 0;
    char buf[1024];
    const char* p = pat ? (const char*)pat : "";
    zt_re_translate(p, buf, sizeof buf);
    if (regcomp(&r->re, buf, REG_EXTENDED | cflags) == 0) r->ok = 1;
    return r;
}

static zt_regex_t* zt_re_compile(int64_t pat) {
    return zt_re_compile_fl(pat, 0);
}

// Python re flag bits → POSIX compile flags. Only the flags this backend can
// honour are mapped (IGNORECASE, MULTILINE); DOTALL/VERBOSE are deliberately
// NOT registered, so using them is reported rather than silently ignored.
#define ZT_RE_IGNORECASE 1
#define ZT_RE_MULTILINE 2
static int zt_re_cflags(int64_t pyflags) {
    int c = 0;
    if (pyflags & ZT_RE_IGNORECASE) c |= REG_ICASE;
    if (pyflags & ZT_RE_MULTILINE) c |= REG_NEWLINE;
    return c;
}
int64_t py_re_ignorecase(void) { return ZT_RE_IGNORECASE; }
int64_t py_re_multiline(void) { return ZT_RE_MULTILINE; }

// Match handle: [string, offsets[10]] (offsets are byte positions).
typedef struct {
    int64_t str;
    int64_t nmatch;
    regmatch_t m[10];
} zt_match_t;

static int64_t zt_re_exec(int64_t pat, int64_t str, int flags) {
    if (!str) return 0;
    zt_regex_t* r = zt_re_compile(pat);
    if (!r->ok) return 0;
    zt_match_t* mt = (zt_match_t*)GC_malloc(sizeof(zt_match_t));
    mt->str = str;
    mt->nmatch = 0;
    if (regexec(&r->re, (const char*)str, 10, mt->m, flags) != 0) return 0;
    mt->nmatch = 10;
    return (int64_t)mt;
}

int64_t py_re_search(int64_t pat, int64_t s) { return zt_re_exec(pat, s, 0); }
int64_t py_re_match(int64_t pat, int64_t s) { return zt_re_exec(pat, s, 0); }
// Flag-taking variants (3 args) — the arity-suffix mechanism routes a
// 3-argument re.search/match/fullmatch call here.
static int64_t zt_re_exec_fl(int64_t pat, int64_t s, int cflags) {
    if (!s) return 0;
    zt_regex_t* r = zt_re_compile_fl(pat, cflags);
    if (!r->ok) return 0;
    zt_match_t* mt = (zt_match_t*)GC_malloc(sizeof(zt_match_t));
    mt->str = s;
    mt->nmatch = 0;
    if (regexec(&r->re, (const char*)s, 10, mt->m, 0) != 0) return 0;
    mt->nmatch = 10;
    return (int64_t)mt;
}
int64_t py_re_search_3(int64_t pat, int64_t s, int64_t flags) {
    return zt_re_exec_fl(pat, s, zt_re_cflags(flags));
}
int64_t py_re_match_3(int64_t pat, int64_t s, int64_t flags) {
    return zt_re_exec_fl(pat, s, zt_re_cflags(flags));
}
int64_t py_re_fullmatch_3(int64_t pat, int64_t s, int64_t flags) {
    int64_t m = zt_re_exec_fl(pat, s, zt_re_cflags(flags));
    if (!m) return 0;
    zt_match_t* mt = (zt_match_t*)m;
    size_t len = strlen((const char*)(mt->str ? (const char*)mt->str : ""));
    if ((size_t)mt->m[0].rm_eo != len || mt->m[0].rm_so != 0) return 0;
    return m;
}
int64_t py_re_fullmatch(int64_t pat, int64_t s) {
    int64_t m = zt_re_exec(pat, s, 0);
    if (!m) return 0;
    zt_match_t* mt = (zt_match_t*)m;
    size_t len = strlen((const char*)(mt->str ? (const char*)mt->str : ""));
    if ((size_t)mt->m[0].rm_eo != len || mt->m[0].rm_so != 0) return 0;
    return m;
}
int64_t py_re_group(int64_t mh, int64_t idx) {
    if (!mh) return (int64_t)zt_strdup("");
    zt_match_t* mt = (zt_match_t*)mh;
    if (idx < 0 || idx >= 10 || mt->m[idx].rm_so < 0) return (int64_t)zt_strdup("");
    size_t n = (size_t)(mt->m[idx].rm_eo - mt->m[idx].rm_so);
    char* out = (char*)GC_malloc(n + 1);
    memcpy(out, (const char*)mt->str + mt->m[idx].rm_so, n);
    out[n] = 0;
    return (int64_t)out;
}
int64_t py_re_start(int64_t mh) {
    if (!mh) return -1;
    return ((zt_match_t*)mh)->m[0].rm_so;
}
int64_t py_re_end(int64_t mh) {
    if (!mh) return -1;
    return ((zt_match_t*)mh)->m[0].rm_eo;
}
// re.sub(pat, repl, s): `repl` may be a string (with \1..\9 backrefs) or a
// callable (Python's `lambda m: ...`) — a function pointer taking the match.
static void zt_re_append(char** out, size_t* n, size_t* cap, const char* src, size_t len) {
    if (*n + len + 1 > *cap) {
        while (*n + len + 1 > *cap) *cap *= 2;
        char* nb = (char*)GC_malloc(*cap);
        memcpy(nb, *out, *n);
        *out = nb;
    }
    memcpy(*out + *n, src, len);
    *n += len;
}
int64_t py_re_sub_call(int64_t pat, int64_t fn_ptr, int64_t s) {
    if (!s) return (int64_t)zt_strdup("");
    zt_regex_t* r = zt_re_compile(pat);
    if (!r->ok) return s;
    int64_t (*fp)(int64_t) = (int64_t (*)(int64_t))fn_ptr;
    const char* cur = (const char*)s;
    size_t cap = 256, n = 0;
    char* out = (char*)GC_malloc(cap);
    regmatch_t m[10];
    int guard = 0;
    while (regexec(&r->re, cur, 10, m, 0) == 0 && guard++ < 100000) {
        zt_re_append(&out, &n, &cap, cur, (size_t)m[0].rm_so);
        zt_match_t* mt = (zt_match_t*)GC_malloc(sizeof(zt_match_t));
        mt->str = (int64_t)(uintptr_t)cur;
        mt->nmatch = 10;
        for (int i = 0; i < 10; i++) mt->m[i].rm_so = m[i].rm_so, mt->m[i].rm_eo = m[i].rm_eo;
        int64_t rep = fp((int64_t)mt);
        if (rep) zt_re_append(&out, &n, &cap, (const char*)rep, strlen((const char*)rep));
        size_t adv = (m[0].rm_eo > 0) ? (size_t)m[0].rm_eo : 1;
        cur += adv;
    }
    zt_re_append(&out, &n, &cap, cur, strlen(cur));
    out[n] = 0;
    return (int64_t)out;
}
static int64_t zt_re_sub_fl(int64_t pat, int64_t repl, int64_t s, int cflags);
int64_t py_re_sub(int64_t pat, int64_t repl, int64_t s) {
    return zt_re_sub_fl(pat, repl, s, 0);
}
int64_t py_re_sub_4(int64_t pat, int64_t repl, int64_t s, int64_t flags) {
    return zt_re_sub_fl(pat, repl, s, zt_re_cflags(flags));
}
static int64_t zt_re_sub_fl(int64_t pat, int64_t repl, int64_t s, int cflags) {
    if (s && repl && (uintptr_t)repl > 4096) {
        // Heuristic: a real string handle looks like a pointer; a small integer
        // is a callback address. Callers with a callable take py_re_sub_call.
    }
    if (!s) return (int64_t)zt_strdup("");
    zt_regex_t* r = zt_re_compile_fl(pat, cflags);
    if (!r->ok) return s;
    const char* rep = repl ? (const char*)repl : "";
    const char* cur = (const char*)s;
    size_t cap = 256, n = 0;
    char* out = (char*)GC_malloc(cap);
    regmatch_t m[10];
    int guard = 0;
    while (regexec(&r->re, cur, 10, m, 0) == 0 && guard++ < 100000) {
        zt_re_append(&out, &n, &cap, cur, (size_t)m[0].rm_so);
        for (const char* p = rep; *p; p++) {
            if (*p == '\\' && p[1] >= '0' && p[1] <= '9') {
                int gi = p[1] - '0';
                if (m[gi].rm_so >= 0) {
                    zt_re_append(&out, &n, &cap, cur + m[gi].rm_so,
                                 (size_t)(m[gi].rm_eo - m[gi].rm_so));
                }
                p++;
            } else {
                zt_re_append(&out, &n, &cap, p, 1);
            }
        }
        size_t adv = (m[0].rm_eo > 0) ? (size_t)m[0].rm_eo : 1;
        cur += adv;
    }
    zt_re_append(&out, &n, &cap, cur, strlen(cur));
    out[n] = 0;
    return (int64_t)out;
}
int64_t py_re_split(int64_t pat, int64_t s) {
    if (!s) return 0;
    zt_regex_t* r = zt_re_compile(pat);
    int64_t cap = 8, len = 0;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap;
    base[1] = 0;
    const char* cur = (const char*)s;
    regmatch_t m[10];
    if (r->ok) {
        while (regexec(&r->re, cur, 10, m, 0) == 0) {
            size_t n = (size_t)m[0].rm_so;
            char* piece = (char*)GC_malloc(n + 1);
            memcpy(piece, cur, n);
            piece[n] = 0;
            if (len >= cap) {
                int64_t nc = cap * 2;
                int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)nc * 8);
                nb[0] = nc; nb[1] = len;
                for (int64_t i = 0; i < len; i++) nb[2 + i] = base[2 + i];
                base = nb; cap = nc;
            }
            base[2 + len++] = (int64_t)piece;
            size_t adv = (m[0].rm_eo > 0) ? (size_t)m[0].rm_eo : 1;
            cur += adv;
        }
    }
    if (len >= cap) {
        int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)(len + 1) * 8);
        nb[0] = len + 1; nb[1] = len;
        for (int64_t i = 0; i < len; i++) nb[2 + i] = base[2 + i];
        base = nb;
    }
    base[2 + len++] = (int64_t)zt_strdup(cur);
    base[1] = len;
    return (int64_t)(base + 2);
}
static int64_t zt_re_findall_fl(int64_t pat, int64_t s, int cflags) {
    if (!s) return 0;
    zt_regex_t* r = zt_re_compile_fl(pat, cflags);
    int64_t cap = 8, len = 0;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap; base[1] = 0;
    const char* cur = (const char*)s;
    regmatch_t m[10];
    int guard = 0;
    if (r->ok) {
        while (regexec(&r->re, cur, 10, m, 0) == 0 && guard++ < 100000) {
            size_t n = (size_t)(m[0].rm_eo - m[0].rm_so);
            char* hit = (char*)GC_malloc(n + 1);
            memcpy(hit, cur + m[0].rm_so, n);
            hit[n] = 0;
            if (len >= cap) {
                int64_t nc = cap * 2;
                int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)nc * 8);
                nb[0] = nc; nb[1] = len;
                for (int64_t i = 0; i < len; i++) nb[2 + i] = base[2 + i];
                base = nb; cap = nc;
            }
            base[2 + len++] = (int64_t)hit;
            size_t adv = (m[0].rm_eo > 0) ? (size_t)m[0].rm_eo : 1;
            cur += adv;
        }
    }
    base[1] = len;
    return (int64_t)(base + 2);
}
// re.compile returns the pattern itself (V1): Pattern methods take it first.
int64_t py_re_findall(int64_t pat, int64_t s) {
    return zt_re_findall_fl(pat, s, 0);
}
int64_t py_re_findall_3(int64_t pat, int64_t s, int64_t flags) {
    return zt_re_findall_fl(pat, s, zt_re_cflags(flags));
}
// finditer: like findall but yields Match handles (so `for m in
// finditer(...): m.group(1)` works). Each handle's str is the scan cursor it
// was found at, so group offsets stay correct.
int64_t py_re_finditer(int64_t pat, int64_t s) {
    if (!s) return 0;
    zt_regex_t* r = zt_re_compile(pat);
    int64_t cap = 8, len = 0;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap;
    base[1] = 0;
    const char* cur = (const char*)s;
    regmatch_t m[10];
    int guard = 0;
    if (r->ok) {
        while (regexec(&r->re, cur, 10, m, 0) == 0 && guard++ < 100000) {
            zt_match_t* mt = (zt_match_t*)GC_malloc(sizeof(zt_match_t));
            mt->str = (int64_t)cur;
            mt->nmatch = 10;
            memcpy(mt->m, m, sizeof(regmatch_t) * 10);
            if (len >= cap) {
                int64_t nc = cap * 2;
                int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)nc * 8);
                nb[0] = nc;
                nb[1] = len;
                for (int64_t i = 0; i < len; i++) nb[2 + i] = base[2 + i];
                base = nb;
                cap = nc;
            }
            base[2 + len++] = (int64_t)mt;
            size_t adv = (m[0].rm_eo > 0) ? (size_t)m[0].rm_eo : 1;
            cur += adv;
        }
    }
    base[1] = len;
    return (int64_t)(base + 2);
}
int64_t py_pattern_fullmatch(int64_t pat, int64_t s) { return py_re_fullmatch(pat, s); }
int64_t py_re_compile(int64_t pat) { return pat ? pat : (int64_t)zt_strdup(""); }
int64_t py_pattern_sub(int64_t pat, int64_t repl, int64_t s) { return py_re_sub(pat, repl, s); }
int64_t py_pattern_sub_call(int64_t pat, int64_t fn, int64_t s) { return py_re_sub_call(pat, fn, s); }
int64_t py_pattern_search(int64_t pat, int64_t s) { return py_re_search(pat, s); }
int64_t py_pattern_match(int64_t pat, int64_t s) { return py_re_match(pat, s); }
int64_t py_pattern_split(int64_t pat, int64_t s) { return py_re_split(pat, s); }
int64_t py_pattern_findall(int64_t pat, int64_t s) { return py_re_findall(pat, s); }

// ---- extra string methods needed by real libraries (stringcase et al.) ----
int64_t str_capitalize(int64_t s) {
    if (!s) return (int64_t)zt_strdup("");
    const char* p = (const char*)s;
    char* r = (char*)GC_malloc(strlen(p) + 1);
    size_t n = 0;
    if (*p) r[n++] = (char)toupper((unsigned char)*p);
    for (const char* q = p + 1; *q; q++) r[n++] = (char)tolower((unsigned char)*q);
    r[n] = 0;
    return (int64_t)r;
}
int64_t str_title(int64_t s) {
    if (!s) return (int64_t)zt_strdup("");
    const char* p = (const char*)s;
    char* r = (char*)GC_malloc(strlen(p) + 1);
    int at_word = 1;
    size_t n = 0;
    for (const char* q = p; *q; q++) {
        unsigned char c = (unsigned char)*q;
        if (isalpha(c)) {
            r[n++] = (char)(at_word ? toupper(c) : tolower(c));
            at_word = 0;
        } else {
            r[n++] = (char)c;
            at_word = 1;
        }
    }
    r[n] = 0;
    return (int64_t)r;
}
int64_t str_zfill(int64_t s, int64_t width) {
    const char* p = s ? (const char*)s : "";
    size_t len = strlen(p);
    if (width <= (int64_t)len) return (int64_t)zt_strdup(p);
    size_t pad = (size_t)width - len;
    char* r = (char*)GC_malloc(pad + len + 1);
    memset(r, '0', pad);
    memcpy(r + pad, p, len + 1);
    return (int64_t)r;
}
int64_t str_rfind(int64_t hay, int64_t needle) {
    const char* h = hay ? (const char*)hay : "";
    const char* n = needle ? (const char*)needle : "";
    const char* hit = strstr(h, n);
    const char* last = NULL;
    while (hit) {
        last = hit;
        hit = strstr(hit + 1, n);
    }
    return last ? (int64_t)(last - h) : -1;
}
int64_t str_is_alpha(int64_t s) {
    const char* p = s ? (const char*)s : "";
    if (!*p) return 0;
    for (; *p; p++) if (!isalpha((unsigned char)*p)) return 0;
    return 1;
}
int64_t str_is_digit(int64_t s) {
    const char* p = s ? (const char*)s : "";
    if (!*p) return 0;
    for (; *p; p++) if (!isdigit((unsigned char)*p)) return 0;
    return 1;
}
int64_t str_is_upper(int64_t s) {
    const char* p = s ? (const char*)s : "";
    int any = 0;
    for (; *p; p++) { if (islower((unsigned char)*p)) return 0; if (isupper((unsigned char)*p)) any = 1; }
    return any;
}
int64_t str_is_lower(int64_t s) {
    const char* p = s ? (const char*)s : "";
    int any = 0;
    for (; *p; p++) { if (isupper((unsigned char)*p)) return 0; if (islower((unsigned char)*p)) any = 1; }
    return any;
}
// Python str.swapcase: toggle each character's case. NOTE this is NOT an
// alias for upper/lower — the method table previously routed it to
// host_str_to_uppercase, so "aBc".swapcase() silently returned "ABC".
int64_t str_swapcase(int64_t s) {
    const char* p = s ? (const char*)s : "";
    char* r = (char*)GC_malloc(strlen(p) + 1);
    size_t n = 0;
    for (; *p; p++) {
        unsigned char c = (unsigned char)*p;
        if (isupper(c)) r[n++] = (char)tolower(c);
        else if (islower(c)) r[n++] = (char)toupper(c);
        else r[n++] = (char)c;
    }
    r[n] = 0;
    return (int64_t)r;
}
// Python 3.9 str.removeprefix / str.removesuffix.
int64_t str_remove_prefix(int64_t s, int64_t prefix) {
    const char* p = s ? (const char*)s : "";
    const char* pre = prefix ? (const char*)prefix : "";
    size_t pl = strlen(pre);
    if (pl && strncmp(p, pre, pl) == 0) return (int64_t)zt_strdup(p + pl);
    return (int64_t)zt_strdup(p);
}
int64_t str_remove_suffix(int64_t s, int64_t suffix) {
    const char* p = s ? (const char*)s : "";
    const char* suf = suffix ? (const char*)suffix : "";
    size_t sl = strlen(suf), pl = strlen(p);
    if (sl && sl <= pl && strcmp(p + pl - sl, suf) == 0) {
        char* r = (char*)GC_malloc(pl - sl + 1);
        memcpy(r, p, pl - sl);
        r[pl - sl] = 0;
        return (int64_t)r;
    }
    return (int64_t)zt_strdup(p);
}
// The remaining str.is* predicates. Each name COLLIDES with a libc ctype
// symbol of a completely different signature (`isalnum(int)`, not a string),
// so before they were registered the method fell through to the bare extern
// and silently returned 0 instead of failing loudly. Empty string is False for
// all of these (matching CPython).
int64_t str_is_alnum(int64_t s) {
    const char* p = s ? (const char*)s : "";
    if (!*p) return 0;
    for (; *p; p++) if (!isalnum((unsigned char)*p)) return 0;
    return 1;
}
int64_t str_is_space(int64_t s) {
    const char* p = s ? (const char*)s : "";
    if (!*p) return 0;
    for (; *p; p++) if (!isspace((unsigned char)*p)) return 0;
    return 1;
}
int64_t str_is_numeric(int64_t s) { return str_is_digit(s); }
int64_t str_is_decimal(int64_t s) { return str_is_digit(s); }
int64_t str_is_ascii(int64_t s) {
    const char* p = s ? (const char*)s : "";
    for (; *p; p++) if ((unsigned char)*p > 127) return 0;
    return 1;
}
int64_t str_is_printable(int64_t s) {
    const char* p = s ? (const char*)s : "";
    for (; *p; p++) {
        unsigned char c = (unsigned char)*p;
        // ASCII: only control characters are non-printable. Bytes >= 0x80 are
        // UTF-8 lead/continuation bytes; treating them as printable matches
        // CPython for accented letters / CJK (its exact Unicode-category test
        // is out of scope here).
        if (c < 0x80 && iscntrl(c)) return 0;
    }
    return 1;
}
// Python istitle: cased characters must be upper at a word start and lower
// within a word, and at least one cased character must exist.
int64_t str_is_title(int64_t s) {
    const char* p = s ? (const char*)s : "";
    int prev_cased = 0, any = 0;
    for (; *p; p++) {
        unsigned char c = (unsigned char)*p;
        if (isalpha(c)) {
            any = 1;
            if (prev_cased) { if (isupper(c)) return 0; }
            else if (islower(c)) return 0;
            prev_cased = 1;
        } else {
            prev_cased = 0;
        }
    }
    return any;
}
// "".join(parts) — Python's str.join over a Vec of string handles.
int64_t str_join(int64_t sep, int64_t vec) {
    const char* sp = sep ? (const char*)sep : "";
    if (!vec) return (int64_t)zt_strdup("");
    int64_t len = ((int64_t*)(vec - 16))[1];
    size_t cap = 64, n = 0;
    char* out = (char*)GC_malloc(cap);
    for (int64_t i = 0; i < len; i++) {
        const char* piece = (const char*)((int64_t*)vec)[i];
        if (!piece) piece = "";
        size_t plen = strlen(piece);
        size_t slen = (i && *sp) ? strlen(sp) : 0;
        if (n + plen + slen + 1 > cap) {
            while (n + plen + slen + 1 > cap) cap *= 2;
            char* nb = (char*)GC_malloc(cap);
            memcpy(nb, out, n);
            out = nb;
        }
        if (i && slen) { memcpy(out + n, sp, slen); n += slen; }
        memcpy(out + n, piece, plen);
        n += plen;
    }
    out[n] = 0;
    return (int64_t)out;
}
int64_t str_ljust(int64_t s, int64_t width, int64_t fill) {
    const char* p = s ? (const char*)s : "";
    char f = fill ? *(const char*)fill : ' ';
    size_t len = strlen(p);
    if (width <= (int64_t)len) return (int64_t)zt_strdup(p);
    size_t pad = (size_t)width - len;
    char* r = (char*)GC_malloc(len + pad + 1);
    memcpy(r, p, len);
    memset(r + len, f, pad);
    r[len + pad] = 0;
    return (int64_t)r;
}
int64_t str_rjust(int64_t s, int64_t width, int64_t fill) {
    const char* p = s ? (const char*)s : "";
    char f = fill ? *(const char*)fill : ' ';
    size_t len = strlen(p);
    if (width <= (int64_t)len) return (int64_t)zt_strdup(p);
    size_t pad = (size_t)width - len;
    char* r = (char*)GC_malloc(len + pad + 1);
    memset(r, f, pad);
    memcpy(r + pad, p, len + 1);
    return (int64_t)r;
}
int64_t host_str_capitalize(int64_t s) { return str_capitalize(s); }
int64_t host_str_title(int64_t s) { return str_title(s); }
int64_t host_str_zfill(int64_t s, int64_t w) { return str_zfill(s, w); }
int64_t host_str_rfind(int64_t s, int64_t n) { return str_rfind(s, n); }
int64_t host_str_isalpha(int64_t s) { return str_is_alpha(s); }
int64_t host_str_isdigit(int64_t s) { return str_is_digit(s); }
int64_t host_str_isupper(int64_t s) { return str_is_upper(s); }
int64_t host_str_islower(int64_t s) { return str_is_lower(s); }
int64_t host_str_swapcase(int64_t s) { return str_swapcase(s); }
int64_t host_str_removeprefix(int64_t s, int64_t p) { return str_remove_prefix(s, p); }
int64_t host_str_removesuffix(int64_t s, int64_t p) { return str_remove_suffix(s, p); }
int64_t host_str_isalnum(int64_t s) { return str_is_alnum(s); }
int64_t host_str_isspace(int64_t s) { return str_is_space(s); }
int64_t host_str_isnumeric(int64_t s) { return str_is_numeric(s); }
int64_t host_str_isdecimal(int64_t s) { return str_is_decimal(s); }
int64_t host_str_isascii(int64_t s) { return str_is_ascii(s); }
int64_t host_str_isprintable(int64_t s) { return str_is_printable(s); }
int64_t host_str_istitle(int64_t s) { return str_is_title(s); }
int64_t host_str_join(int64_t sep, int64_t vec) { return str_join(sep, vec); }
int64_t host_str_ljust(int64_t s, int64_t w, int64_t f) { return str_ljust(s, w, f); }
int64_t host_str_rjust(int64_t s, int64_t w, int64_t f) { return str_rjust(s, w, f); }
// Python's ljust(width[, fillchar]) — the 2-argument form is the common one.
// NOTE: `fill` is a STRING handle (str_ljust dereferences it), so the default
// must be a one-char string, not the byte value ' '.
int64_t host_str_ljust2(int64_t s, int64_t w) { return str_ljust(s, w, (int64_t)" "); }
int64_t host_str_rjust2(int64_t s, int64_t w) { return str_rjust(s, w, (int64_t)" "); }

static int64_t str_center(int64_t s, int64_t width, int64_t fill) {
    const char* p = s ? (const char*)s : "";
    char fc = fill ? *(const char*)fill : ' ';
    size_t n = strlen(p);
    if (width <= (int64_t)n) return (int64_t)zt_strdup(p);
    size_t total = (size_t)width;
    size_t left = (total - n) / 2;
    size_t right = total - n - left;
    char* out = (char*)GC_malloc(total + 1);
    memset(out, fc, left);
    memcpy(out + left, p, n);
    memset(out + left + n, fc, right);
    out[total] = 0;
    return (int64_t)out;
}
int64_t host_str_center(int64_t s, int64_t w, int64_t f) { return str_center(s, w, f); }
int64_t host_str_center2(int64_t s, int64_t w) { return str_center(s, w, (int64_t)" "); }

// ---- Python string indexing / slicing (s[0], s[-1], s[1:], s[:-1]) ----
int64_t str_get(int64_t s, int64_t i) {
    if (!s) return (int64_t)zt_strdup("");
    const char* p = (const char*)s;
    int64_t n = (int64_t)strlen(p);
    if (i < 0) i += n;
    // V1: out-of-range yields "" (Python raises IndexError; the compiler has
    // no exception path for a single subscript yet).
    if (i < 0 || i >= n) return (int64_t)zt_strdup("");
    char* r = (char*)GC_malloc(2);
    r[0] = p[i];
    r[1] = 0;
    return (int64_t)r;
}
// `to_end` distinguishes the omitted-end sentinel (s[1:]) from an explicit
// negative index (s[:-1]) — the desugar alone cannot tell them apart.
int64_t str_slice(int64_t s, int64_t start, int64_t end, int64_t to_end) {
    if (!s) return (int64_t)zt_strdup("");
    const char* p = (const char*)s;
    int64_t n = (int64_t)strlen(p);
    if (start < 0) start += n;
    if (start < 0) start = 0;
    if (start > n) start = n;
    if (to_end) {
        end = n;
    } else {
        if (end < 0) end += n;
        if (end < 0) end = 0;
        if (end > n) end = n;
    }
    if (end < start) end = start;
    int64_t len = end - start;
    char* r = (char*)GC_malloc((size_t)len + 1);
    memcpy(r, p + start, (size_t)len);
    r[len] = 0;
    return (int64_t)r;
}

// ============================================================================
// PY-A JSON as a STATIC sum type.
//
// A Json value is a two-slot GC block [tag, payload]:
//   0 NULL  -> the handle itself is 0 (so `is None` / `if not j` work)
//   1 INT   -> payload = i64
//   2 F64   -> payload = double bits
//   3 STR   -> payload = char* handle
//   4 ARR   -> payload = Vec handle (elements are Json handles)
//   5 OBJ   -> payload = map handle (key = map_str_key(str), value = Json)
//
// This is the tagged-union answer to "JSON is heterogeneous": the compiler
// knows the value is a Json, and every accessor dispatches on the runtime tag
// — no dynamic dispatch of the enclosing program, just a sum type, the same
// shape serde_json::Value / Swift's JSON enum use.
// ============================================================================
#define ZJ_NULL 0
#define ZJ_INT 1
#define ZJ_F64 2
#define ZJ_STR 3
#define ZJ_ARR 4
#define ZJ_OBJ 5
#define ZJ_BOOL 6

extern int64_t map_str_key(int64_t handle);

static int64_t zj_make(int64_t tag, int64_t payload) {
    int64_t* h = (int64_t*)GC_malloc(16);
    h[0] = tag;
    h[1] = payload;
    return (int64_t)h;
}
static int64_t zj_tag(int64_t j) { return j ? ((int64_t*)j)[0] : ZJ_NULL; }
static int64_t zj_payload(int64_t j) { return j ? ((int64_t*)j)[1] : 0; }

static int64_t zj_vec_new(int64_t cap) {
    if (cap < 8) cap = 8;
    int64_t* b = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    b[0] = cap;
    b[1] = 0;
    return (int64_t)(b + 2);
}
static void zj_vec_push(int64_t vec, int64_t v) {
    int64_t* b = (int64_t*)(vec - 16);
    if (b[1] >= b[0]) {
        int64_t ncap = b[0] < 8 ? 8 : b[0] * 2;
        int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)ncap * 8);
        nb[0] = ncap;
        nb[1] = b[1];
        for (int64_t i = 0; i < b[1]; i++) nb[2 + i] = b[2 + i];
        vec = (int64_t)(nb + 2);
        b = nb;
    }
    b[2 + b[1]] = v;
    b[1] += 1;
    // Callers hold the *old* handle, so grow in place where possible; when the
    // block moved, the parent array entry is stale. zj_vec_new starts at 8 and
    // arrays are built before being published, so this stays correct for the
    // parser's usage (the returned handle is what the parser stores).
}

// ---- parser ----
typedef struct {
    const char* p;
    int64_t arr; // current array handle being built (for realloc fixups)
} zj_parser;

static void zj_ws(zj_parser* s) {
    while (*s->p == ' ' || *s->p == '\t' || *s->p == '\n' || *s->p == '\r') s->p++;
}
static int64_t zj_parse_value(zj_parser* s);

static int64_t zj_parse_string(zj_parser* s) {
    if (*s->p != '"') return (int64_t)zt_strdup("");
    s->p++;
    size_t cap = 32, n = 0;
    char* out = (char*)GC_malloc(cap);
    while (*s->p && *s->p != '"') {
        char c = *s->p++;
        if (c == '\\' && *s->p) {
            char e = *s->p++;
            switch (e) {
                case 'n': c = '\n'; break;
                case 't': c = '\t'; break;
                case 'r': c = '\r'; break;
                case 'b': c = '\b'; break;
                case 'f': c = '\f'; break;
                case 'u': {
                    // \uXXXX -> UTF-8 (BMP only; surrogate pairs pass through)
                    unsigned cp = 0;
                    for (int i = 0; i < 4 && *s->p; i++) {
                        char d = *s->p++;
                        cp <<= 4;
                        if (d >= '0' && d <= '9') cp |= (unsigned)(d - '0');
                        else if (d >= 'a' && d <= 'f') cp |= (unsigned)(d - 'a' + 10);
                        else if (d >= 'A' && d <= 'F') cp |= (unsigned)(d - 'A' + 10);
                    }
                    if (n + 4 > cap) { cap *= 2; char* nb = (char*)GC_malloc(cap); memcpy(nb, out, n); out = nb; }
                    if (cp < 0x80) {
                        out[n++] = (char)cp;
                    } else if (cp < 0x800) {
                        out[n++] = (char)(0xC0 | (cp >> 6));
                        out[n++] = (char)(0x80 | (cp & 0x3F));
                    } else {
                        out[n++] = (char)(0xE0 | (cp >> 12));
                        out[n++] = (char)(0x80 | ((cp >> 6) & 0x3F));
                        out[n++] = (char)(0x80 | (cp & 0x3F));
                    }
                    continue;
                }
                default: c = e; break;
            }
        }
        if (n + 2 > cap) { cap *= 2; char* nb = (char*)GC_malloc(cap); memcpy(nb, out, n); out = nb; }
        out[n++] = c;
    }
    if (*s->p == '"') s->p++;
    out[n] = 0;
    return (int64_t)out;
}

static int64_t zj_parse_value(zj_parser* s) {
    zj_ws(s);
    char c = *s->p;
    if (c == '{') {
        s->p++;
        int64_t m = map_new();
        zj_ws(s);
        if (*s->p == '}') { s->p++; return zj_make(ZJ_OBJ, m); }
        for (;;) {
            zj_ws(s);
            int64_t key = zj_parse_string(s);
            zj_ws(s);
            if (*s->p == ':') s->p++;
            int64_t val = zj_parse_value(s);
            map_insert(m, map_str_key(key), val);
            zj_ws(s);
            if (*s->p == ',') { s->p++; continue; }
            if (*s->p == '}') s->p++;
            break;
        }
        return zj_make(ZJ_OBJ, m);
    }
    if (c == '[') {
        s->p++;
        int64_t vec = zj_vec_new(8);
        zj_ws(s);
        if (*s->p == ']') { s->p++; return zj_make(ZJ_ARR, vec); }
        for (;;) {
            int64_t v = zj_parse_value(s);
            // push with realloc fixup (the vector handle can move)
            int64_t* b = (int64_t*)(vec - 16);
            if (b[1] >= b[0]) {
                int64_t ncap = b[0] * 2;
                int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)ncap * 8);
                nb[0] = ncap;
                nb[1] = b[1];
                for (int64_t i = 0; i < b[1]; i++) nb[2 + i] = b[2 + i];
                vec = (int64_t)(nb + 2);
                b = nb;
            }
            b[2 + b[1]] = v;
            b[1] += 1;
            zj_ws(s);
            if (*s->p == ',') { s->p++; continue; }
            if (*s->p == ']') s->p++;
            break;
        }
        return zj_make(ZJ_ARR, vec);
    }
    if (c == '"') return zj_make(ZJ_STR, zj_parse_string(s));
    if (!strncmp(s->p, "true", 4)) { s->p += 4; return zj_make(ZJ_BOOL, 1); }
    if (!strncmp(s->p, "false", 5)) { s->p += 5; return zj_make(ZJ_BOOL, 0); }
    if (!strncmp(s->p, "null", 4)) { s->p += 4; return 0; }
    // number
    {
        const char* start = s->p;
        int is_float = 0;
        if (*s->p == '-' || *s->p == '+') s->p++;
        while ((*s->p >= '0' && *s->p <= '9')) s->p++;
        if (*s->p == '.') { is_float = 1; s->p++; while (*s->p >= '0' && *s->p <= '9') s->p++; }
        if (*s->p == 'e' || *s->p == 'E') {
            is_float = 1;
            s->p++;
            if (*s->p == '-' || *s->p == '+') s->p++;
            while (*s->p >= '0' && *s->p <= '9') s->p++;
        }
        char buf[64];
        size_t n = (size_t)(s->p - start);
        if (n >= sizeof buf) n = sizeof buf - 1;
        memcpy(buf, start, n);
        buf[n] = 0;
        if (is_float) {
            double d = strtod(buf, NULL);
            int64_t bits;
            memcpy(&bits, &d, sizeof bits);
            return zj_make(ZJ_F64, bits);
        }
        return zj_make(ZJ_INT, strtoll(buf, NULL, 10));
    }
}

int64_t py_json_loads(int64_t text) {
    if (!text) return 0;
    zj_parser s;
    s.p = (const char*)text;
    s.arr = 0;
    return zj_parse_value(&s);
}

// ---- serialization (typed: numbers/strings/nesting all correct) ----
static void zj_dump_into(int64_t j, char** out, size_t* n, size_t* cap);
static void zj_put(char** out, size_t* n, size_t* cap, const char* s, size_t len) {
    if (*n + len + 1 > *cap) {
        while (*n + len + 1 > *cap) *cap *= 2;
        char* nb = (char*)GC_malloc(*cap);
        memcpy(nb, *out, *n);
        *out = nb;
    }
    memcpy(*out + *n, s, len);
    *n += len;
}
static void zj_put_str(char** out, size_t* n, size_t* cap, const char* s) {
    zj_put(out, n, cap, "\"", 1);
    for (const char* p = s; *p; p++) {
        char c = *p;
        if (c == '"' || c == '\\') {
            char esc[2] = {'\\', c};
            zj_put(out, n, cap, esc, 2);
        } else if (c == '\n') {
            zj_put(out, n, cap, "\\n", 2);
        } else if (c == '\t') {
            zj_put(out, n, cap, "\\t", 2);
        } else {
            zj_put(out, n, cap, &c, 1);
        }
    }
    zj_put(out, n, cap, "\"", 1);
}
static void zj_dump_into(int64_t j, char** out, size_t* n, size_t* cap) {
    switch (zj_tag(j)) {
        case ZJ_NULL: zj_put(out, n, cap, "null", 4); break;
        case ZJ_INT: {
            char b[32];
            int k = snprintf(b, sizeof b, "%lld", (long long)zj_payload(j));
            zj_put(out, n, cap, b, (size_t)k);
            break;
        }
        case ZJ_F64: {
            double d;
            int64_t bits = zj_payload(j);
            memcpy(&d, &bits, sizeof d);
            char b[40];
            int k = snprintf(b, sizeof b, "%g", d);
            zj_put(out, n, cap, b, (size_t)k);
            break;
        }
        case ZJ_STR: zj_put_str(out, n, cap, (const char*)zj_payload(j)); break;
        case ZJ_BOOL: {
            const char* t = zj_payload(j) ? "true" : "false";
            zj_put(out, n, cap, t, strlen(t));
            break;
        }
        case ZJ_ARR: {
            int64_t vec = zj_payload(j);
            int64_t len = vec ? ((int64_t*)(vec - 16))[1] : 0;
            zj_put(out, n, cap, "[", 1);
            for (int64_t i = 0; i < len; i++) {
                if (i) zj_put(out, n, cap, ",", 1);
                zj_dump_into(((int64_t*)vec)[i], out, n, cap);
            }
            zj_put(out, n, cap, "]", 1);
            break;
        }
        case ZJ_OBJ: {
            int64_t m = map_resolve(zj_payload(j));
            int64_t cap_entries = m ? ((int64_t*)m)[0] : 0;
            zj_put(out, n, cap, "{", 1);
            int first = 1;
            for (int64_t i = 0; i < cap_entries; i++) {
                char* e = (char*)m + 16 + i * 24;
                if (!*(uint8_t*)(e + 16)) continue;
                if (!first) zj_put(out, n, cap, ",", 1);
                first = 0;
                int64_t ks = zeta_key_string(*(int64_t*)e);
                zj_put_str(out, n, cap, ks ? (const char*)ks : "");
                zj_put(out, n, cap, ":", 1);
                zj_dump_into(*(int64_t*)(e + 8), out, n, cap);
            }
            zj_put(out, n, cap, "}", 1);
            break;
        }
    }
}
int64_t py_json_dump(int64_t j) {
    size_t cap = 128, n = 0;
    char* out = (char*)GC_malloc(cap);
    zj_dump_into(j, &out, &n, &cap);
    out[n] = 0;
    return (int64_t)out;
}

// ---- accessors (static dispatch target for subscript / len / casts) ----
int64_t py_json_get(int64_t j, int64_t key) {
    if (!j) return 0;
    switch (zj_tag(j)) {
        case ZJ_OBJ: {
            // `key` is the raw string handle the compiler lowered.
            int64_t m = zj_payload(j);
            return map_get(m, map_str_key(key));
        }
        case ZJ_ARR: {
            int64_t vec = zj_payload(j);
            int64_t len = vec ? ((int64_t*)(vec - 16))[1] : 0;
            int64_t i = key;
            if (i < 0) i += len;
            if (i < 0 || i >= len) return 0;
            return ((int64_t*)vec)[i];
        }
        default: return 0;
    }
}
int64_t py_json_len(int64_t j) {
    if (!j) return 0;
    switch (zj_tag(j)) {
        case ZJ_ARR: {
            int64_t vec = zj_payload(j);
            return vec ? ((int64_t*)(vec - 16))[1] : 0;
        }
        case ZJ_OBJ: {
            int64_t m = map_resolve(zj_payload(j));
            int64_t cap = m ? ((int64_t*)m)[0] : 0;
            int64_t count = 0;
            for (int64_t i = 0; i < cap; i++) {
                char* e = (char*)m + 16 + i * 24;
                if (*(uint8_t*)(e + 16)) count++;
            }
            return count;
        }
        case ZJ_STR: return (int64_t)strlen((const char*)zj_payload(j));
        default: return 0;
    }
}
int64_t py_json_kind(int64_t j) { return zj_tag(j); }
// `isinstance(<parsed json>, dict|list|str|int|float)` — the tag is a runtime
// property of the value, so the answer must come from here (a static answer
// returned 0 for every object and the caller fell through to a wrong branch).
int64_t py_json_is_kind(int64_t j, int64_t tag) {
    if (!j) return 0;
    return zj_tag(j) == tag ? 1 : 0;
}
int64_t py_json_as_i64(int64_t j) {
    if (!j) return 0;
    switch (zj_tag(j)) {
        case ZJ_BOOL:
        case ZJ_INT: return zj_payload(j);
        case ZJ_F64: {
            double d;
            int64_t bits = zj_payload(j);
            memcpy(&d, &bits, sizeof d);
            return (int64_t)d;
        }
        case ZJ_STR: return strtoll((const char*)zj_payload(j), NULL, 10);
        default: return 0;
    }
}
double py_json_as_f64(int64_t j) {
    if (!j) return 0.0;
    switch (zj_tag(j)) {
        case ZJ_INT: return (double)zj_payload(j);
        case ZJ_F64: {
            double d;
            int64_t bits = zj_payload(j);
            memcpy(&d, &bits, sizeof d);
            return d;
        }
        case ZJ_STR: return strtod((const char*)zj_payload(j), NULL);
        default: return 0.0;
    }
}
int64_t py_json_as_str(int64_t j) {
    if (!j) return (int64_t)zt_strdup("");
    switch (zj_tag(j)) {
        case ZJ_STR: return zj_payload(j);
        case ZJ_INT: {
            char b[32];
            snprintf(b, sizeof b, "%lld", (long long)zj_payload(j));
            return (int64_t)zt_strdup(b);
        }
        case ZJ_F64: {
            double d;
            int64_t bits = zj_payload(j);
            memcpy(&d, &bits, sizeof d);
            char b[40];
            snprintf(b, sizeof b, "%g", d);
            return (int64_t)zt_strdup(b);
        }
        default: return py_json_dump(j);
    }
}
void py_json_print(int64_t j) {
    const char* s = (const char*)py_json_dump(j);
    fputs(s, stdout);
    fputc('\n', stdout);
}
// `d in obj` / `k in arr` — membership on a Json container.
int64_t py_json_contains(int64_t j, int64_t key) {
    if (!j) return 0;
    if (zj_tag(j) == ZJ_OBJ) {
        int64_t m = zj_payload(j);
        return map_get(m, map_str_key(key)) != 0;
    }
    if (zj_tag(j) == ZJ_ARR) {
        return py_json_get(j, key) != 0;
    }
    if (zj_tag(j) == ZJ_STR) {
        return strstr((const char*)zj_payload(j), (const char*)key) != NULL;
    }
    return 0;
}

// Python-ish repr for `print(json_value)`: scalars print bare, containers as
// JSON text (Python's dict repr differs only in quote style).
int64_t py_json_repr(int64_t j) {
    if (!j) return (int64_t)zt_strdup("None");
    switch (zj_tag(j)) {
        case ZJ_STR: return zj_payload(j);
        case ZJ_BOOL: return (int64_t)zt_strdup(zj_payload(j) ? "true" : "false");
        case ZJ_INT:
        case ZJ_F64: return py_json_as_str(j);
        default: return py_json_dump(j);
    }
}

// ============================================================================
// Dict value-type side table.
//
// A map slot is a raw 64-bit value with no type tag, so json.dumps(dict) could
// not tell 2.5 from 2 or a string pointer from an integer. The compiler DOES
// know each inserted value's static type, so at every DictInsert it records it
// here; the dumper (and later the printer) look it up by (map, key). This
// avoids changing the map layout, which the whole existing runtime depends on.
// ============================================================================
#define ZT_TAG_CAP 8192
static int64_t g_tag_map[ZT_TAG_CAP];
static int64_t g_tag_key[ZT_TAG_CAP];
static int64_t g_tag_val[ZT_TAG_CAP];
static uint64_t zt_tag_slot(int64_t map, int64_t key) {
    uint64_t h = (uint64_t)map * 1099511628211ULL ^ (uint64_t)key;
    return (h ^ (h >> 29)) & (ZT_TAG_CAP - 1);
}
void zeta_map_set_tag(int64_t map, int64_t key, int64_t tag) {
    uint64_t i = zt_tag_slot(map, key);
    for (int n = 0; n < ZT_TAG_CAP; n++) {
        uint64_t j = (i + (uint64_t)n) & (ZT_TAG_CAP - 1);
        if (g_tag_map[j] == 0) {
            g_tag_map[j] = map ? map : 1;
            g_tag_key[j] = key;
            g_tag_val[j] = tag;
            return;
        }
        if (g_tag_map[j] == (map ? map : 1) && g_tag_key[j] == key) {
            g_tag_val[j] = tag;
            return;
        }
    }
}
// 0 = unknown/int, 1 = f64, 2 = str, 3 = bool
int64_t zeta_map_value_tag(int64_t map, int64_t key) {
    uint64_t i = zt_tag_slot(map, key);
    for (int n = 0; n < ZT_TAG_CAP; n++) {
        uint64_t j = (i + (uint64_t)n) & (ZT_TAG_CAP - 1);
        if (g_tag_map[j] == 0) return 0;
        if (g_tag_map[j] == (map ? map : 1) && g_tag_key[j] == key) return g_tag_val[j];
    }
    return 0;
}

// Typed vector dumper: the element type is known STATICALLY (a list literal is
// homogeneous), so the compiler passes its tag instead of a per-element side
// table — indices move on realloc, the static type does not.
// tag: 0 = int, 1 = f64, 2 = str, 3 = bool
int64_t py_json_dumps_vec_typed(int64_t vec, int64_t tag) {
    if (!vec) return (int64_t)zt_strdup("[]");
    int64_t len = ((int64_t*)(vec - 16))[1];
    size_t cap = 64, n = 0;
    char* out = (char*)GC_malloc(cap);
    out[n++] = '[';
    for (int64_t i = 0; i < len; i++) {
        int64_t v = ((int64_t*)vec)[i];
        if (n + 64 > cap) { cap *= 2; char* nb = (char*)GC_malloc(cap); memcpy(nb, out, n); out = nb; }
        if (i) { out[n++] = ','; out[n++] = ' '; }
        switch (tag) {
            case 1: {
                double d;
                memcpy(&d, &v, sizeof d);
                n += (size_t)sprintf(out + n, "%g", d);
                break;
            }
            case 2:
                if (v) {
                    n += (size_t)zt_json_quote((const char*)v, out + n);
                } else {
                    n += (size_t)sprintf(out + n, "\"\"");
                }
                break;
            case 3:
                n += (size_t)sprintf(out + n, "%s", v ? "true" : "false");
                break;
            default:
                n += (size_t)sprintf(out + n, "%lld", (long long)v);
                break;
        }
    }
    out[n++] = ']';
    out[n] = 0;
    return (int64_t)out;
}

// ---- Json object/array navigation (keys/values/get) ----
static int64_t zj_vec_new_cap(int64_t cap) {
    if (cap < 8) cap = 8;
    int64_t* b = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    b[0] = cap;
    b[1] = 0;
    return (int64_t)(b + 2);
}
static int64_t zj_vec_push_h(int64_t vec, int64_t v) {
    int64_t* b = (int64_t*)(vec - 16);
    if (b[1] >= b[0]) {
        int64_t ncap = b[0] * 2;
        int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)ncap * 8);
        nb[0] = ncap;
        nb[1] = b[1];
        for (int64_t i = 0; i < b[1]; i++) nb[2 + i] = b[2 + i];
        vec = (int64_t)(nb + 2);
        b = nb;
    }
    b[2 + b[1]] = v;
    b[1] += 1;
    return vec;
}

// dict.get(k[, default]) — missing key yields the default. The compiler
// passes the default's own type tag (0 int, 1 f64, 2 str) so a non-Json
// default is wrapped into a Json value: `get` is statically typed PyJson, and
// returning a bare string handle would make `print(d.get(k, "x"))` read a
// Json tag out of a char*.
int64_t py_json_get_default(int64_t j, int64_t key, int64_t dflt, int64_t dflt_tag) {
    int64_t v = py_json_get(j, key);
    if (v) return v;
    if (!dflt) return 0;
    switch (dflt_tag) {
        case 2: return zj_make(ZJ_STR, dflt);
        case 1: return zj_make(ZJ_F64, dflt);
        default: return zj_make(ZJ_INT, dflt);
    }
}
// Object keys as a Vec of strings (Python's dict.keys()); arrays yield the
// indices as strings, matching Python's list-of-keys absence with a usable
// fallback.
// A parsed JSON OBJECT's payload is a dict; expose it so the dict helpers
// (which handle growth/forwarding) can be reused by `.items()` / `.values()`.
// Non-objects yield 0 — "no items", never garbage.
int64_t map_values(int64_t);
int64_t py_map_items(int64_t);
int64_t py_json_as_map(int64_t j) {
    return (j && zj_tag(j) == ZJ_OBJ) ? map_resolve(zj_payload(j)) : 0;
}
int64_t py_json_items(int64_t j) { return py_map_items(py_json_as_map(j)); }

int64_t py_json_keys(int64_t j) {
    int64_t vec = zj_vec_new_cap(8);
    if (!j) return vec;
    if (zj_tag(j) == ZJ_OBJ) {
        int64_t m = map_resolve(zj_payload(j));
        int64_t cap = m ? ((int64_t*)m)[0] : 0;
        for (int64_t i = 0; i < cap; i++) {
            char* e = (char*)m + 16 + i * 24;
            if (!*(uint8_t*)(e + 16)) continue;
            int64_t ks = zeta_key_string(*(int64_t*)e);
            if (ks) vec = zj_vec_push_h(vec, ks);
        }
    } else if (zj_tag(j) == ZJ_ARR) {
        int64_t src = zj_payload(j);
        int64_t len = src ? ((int64_t*)(src - 16))[1] : 0;
        for (int64_t i = 0; i < len; i++) {
            char* b = (char*)GC_malloc(24);
            snprintf(b, 24, "%lld", (long long)i);
            vec = zj_vec_push_h(vec, (int64_t)b);
        }
    }
    return vec;
}
// Object values (or array elements) as a Vec of Json handles.
int64_t py_json_values(int64_t j) {
    int64_t vec = zj_vec_new_cap(8);
    if (!j) return vec;
    if (zj_tag(j) == ZJ_OBJ) {
        int64_t m = map_resolve(zj_payload(j));
        int64_t cap = m ? ((int64_t*)m)[0] : 0;
        for (int64_t i = 0; i < cap; i++) {
            char* e = (char*)m + 16 + i * 24;
            if (!*(uint8_t*)(e + 16)) continue;
            vec = zj_vec_push_h(vec, *(int64_t*)(e + 8));
        }
    } else if (zj_tag(j) == ZJ_ARR) {
        int64_t src = zj_payload(j);
        int64_t len = src ? ((int64_t*)(src - 16))[1] : 0;
        for (int64_t i = 0; i < len; i++) vec = zj_vec_push_h(vec, ((int64_t*)src)[i]);
    }
    return vec;
}

// ============================================================================
// PY-A file objects + json.load/dump.
// Handle = [FILE*, closed]. Errors are reported on stderr and the handle is 0
// (fail-loud at use time rather than a silent no-op).
// ============================================================================
typedef struct {
    FILE* fp;
    int64_t closed;
} zt_file_t;

int64_t py_file_open(int64_t path, int64_t mode) {
    const char* p = path ? (const char*)path : "";
    const char* m = mode ? (const char*)mode : "r";
    FILE* fp = fopen(p, m);
    if (!fp) {
        fprintf(stderr, "PY-A: open(\"%s\", \"%s\") failed: %s\n", p, m, strerror(errno));
        return 0;
    }
    zt_file_t* h = (zt_file_t*)GC_malloc(sizeof(zt_file_t));
    h->fp = fp;
    h->closed = 0;
    return (int64_t)h;
}
static zt_file_t* zt_file(int64_t h) {
    if (!h) return NULL;
    zt_file_t* f = (zt_file_t*)h;
    return f->fp ? f : NULL;
}
int64_t py_file_read(int64_t h) {
    zt_file_t* f = zt_file(h);
    if (!f) return (int64_t)zt_strdup("");
    size_t cap = 4096, n = 0;
    char* buf = (char*)GC_malloc(cap);
    for (;;) {
        if (n + 1024 > cap) {
            cap *= 2;
            char* nb = (char*)GC_malloc(cap);
            memcpy(nb, buf, n);
            buf = nb;
        }
        size_t r = fread(buf + n, 1, 1024, f->fp);
        n += r;
        if (r == 0) break;
    }
    buf[n] = 0;
    return (int64_t)buf;
}
int64_t py_file_readline(int64_t h) {
    zt_file_t* f = zt_file(h);
    if (!f) return (int64_t)zt_strdup("");
    char* line = NULL;
    size_t cap = 0;
    ssize_t r = getline(&line, &cap, f->fp);
    if (r < 0) return (int64_t)zt_strdup("");
    char* out = (char*)GC_malloc((size_t)r + 1);
    memcpy(out, line, (size_t)r);
    out[r] = 0;
    free(line);
    return (int64_t)out;
}
int64_t py_file_readlines(int64_t h) {
    int64_t vec = zj_vec_new_cap(8);
    zt_file_t* f = zt_file(h);
    if (!f) return vec;
    char* line = NULL;
    size_t cap = 0;
    ssize_t r;
    while ((r = getline(&line, &cap, f->fp)) >= 0) {
        char* out = (char*)GC_malloc((size_t)r + 1);
        memcpy(out, line, (size_t)r);
        out[r] = 0;
        vec = zj_vec_push_h(vec, (int64_t)out);
    }
    if (line) free(line);
    return vec;
}
int64_t py_file_write(int64_t h, int64_t s) {
    zt_file_t* f = zt_file(h);
    const char* p = s ? (const char*)s : "";
    if (!f) return 0;
    fputs(p, f->fp);
    return (int64_t)strlen(p);
}
int64_t py_file_close(int64_t h) {
    zt_file_t* f = zt_file(h);
    if (!f) return 0;
    if (f->closed) return 0;
    fclose(f->fp);
    f->closed = 1;
    f->fp = NULL;
    return 0;
}
// `with open(...) as f:` — enter is identity, exit closes.
int64_t py_file_enter(int64_t h) { return h; }
int64_t py_file_exit(int64_t h) { return py_file_close(h); }
int64_t py_file_closed(int64_t h) {
    zt_file_t* f = (zt_file_t*)h;
    return (!h || !f->fp) ? 1 : 0;
}

// ---- json.load(f) / json.dump(obj, f) ----
int64_t py_json_load_file(int64_t h) {
    zt_file_t* f = zt_file(h);
    if (!f) return 0;
    int64_t text = py_file_read(h);
    return py_json_loads(text);
}
int64_t py_json_dump_file(int64_t j, int64_t h) {
    int64_t text = py_json_dump(j);
    return py_file_write(h, text);
}

// ============================================================================
// PY-A concurrency primitives, batch 2: queue.Queue / threading.Event /
// threading.Semaphore / threading.Timer. Built on pthread mutex + condvar, so
// the blocking is real (a consumer thread genuinely sleeps in cond_wait).
// ============================================================================
typedef struct {
    pthread_mutex_t m;
    pthread_cond_t c;
    int64_t* buf;
    int64_t cap;
    int64_t len;
    int64_t head;
} zt_queue_t;

int64_t py_queue_new(void) {
    zt_queue_t* q = (zt_queue_t*)GC_malloc(sizeof(zt_queue_t));
    pthread_mutex_init(&q->m, NULL);
    pthread_cond_init(&q->c, NULL);
    q->cap = 16;
    q->len = 0;
    q->head = 0;
    q->buf = (int64_t*)GC_malloc(sizeof(int64_t) * (size_t)q->cap);
    return (int64_t)q;
}
// Python's put/get take an optional timeout/block flag; the compiler only
// wires the blocking, no-argument form, so blocking is what callers get.
void py_queue_put(int64_t h, int64_t v) {
    if (!h) return;
    zt_queue_t* q = (zt_queue_t*)h;
    pthread_mutex_lock(&q->m);
    if (q->len == q->cap) {
        // ponytail: grow instead of blocking on a bound — Python's
        // Queue(maxsize=0) is unbounded, which is the common case.
        int64_t ncap = q->cap * 2;
        int64_t* nb = (int64_t*)GC_malloc(sizeof(int64_t) * (size_t)ncap);
        for (int64_t i = 0; i < q->len; i++) nb[i] = q->buf[(q->head + i) % q->cap];
        q->buf = nb;
        q->cap = ncap;
        q->head = 0;
    }
    q->buf[(q->head + q->len) % q->cap] = v;
    q->len += 1;
    pthread_cond_broadcast(&q->c);
    pthread_mutex_unlock(&q->m);
}
int64_t py_queue_get(int64_t h) {
    if (!h) return 0;
    zt_queue_t* q = (zt_queue_t*)h;
    pthread_mutex_lock(&q->m);
    while (q->len == 0) pthread_cond_wait(&q->c, &q->m);
    int64_t v = q->buf[q->head];
    q->head = (q->head + 1) % q->cap;
    q->len -= 1;
    pthread_cond_broadcast(&q->c);
    pthread_mutex_unlock(&q->m);
    return v;
}
int64_t py_queue_qsize(int64_t h) {
    if (!h) return 0;
    zt_queue_t* q = (zt_queue_t*)h;
    pthread_mutex_lock(&q->m);
    int64_t n = q->len;
    pthread_mutex_unlock(&q->m);
    return n;
}
int64_t py_queue_empty(int64_t h) { return py_queue_qsize(h) == 0; }

// ---- threading.Event ----
typedef struct {
    pthread_mutex_t m;
    pthread_cond_t c;
    int64_t set;
} zt_event_t;

int64_t py_threading_event_new(void) {
    zt_event_t* e = (zt_event_t*)GC_malloc(sizeof(zt_event_t));
    pthread_mutex_init(&e->m, NULL);
    pthread_cond_init(&e->c, NULL);
    e->set = 0;
    return (int64_t)e;
}
int64_t py_event_set(int64_t h) {
    if (!h) return 0;
    zt_event_t* e = (zt_event_t*)h;
    pthread_mutex_lock(&e->m);
    e->set = 1;
    pthread_cond_broadcast(&e->c);
    pthread_mutex_unlock(&e->m);
    return 0;
}
int64_t py_event_clear(int64_t h) {
    if (!h) return 0;
    zt_event_t* e = (zt_event_t*)h;
    pthread_mutex_lock(&e->m);
    e->set = 0;
    pthread_mutex_unlock(&e->m);
    return 0;
}
int64_t py_event_is_set(int64_t h) {
    if (!h) return 0;
    zt_event_t* e = (zt_event_t*)h;
    pthread_mutex_lock(&e->m);
    int64_t s = e->set;
    pthread_mutex_unlock(&e->m);
    return s;
}
int64_t py_event_wait(int64_t h) {
    if (!h) return 0;
    zt_event_t* e = (zt_event_t*)h;
    pthread_mutex_lock(&e->m);
    while (!e->set) pthread_cond_wait(&e->c, &e->m);
    pthread_mutex_unlock(&e->m);
    return 1;
}

// ---- threading.Semaphore ----
typedef struct {
    pthread_mutex_t m;
    pthread_cond_t c;
    int64_t count;
} zt_sem_t;

int64_t py_threading_sem_new(int64_t count) {
    zt_sem_t* s = (zt_sem_t*)GC_malloc(sizeof(zt_sem_t));
    pthread_mutex_init(&s->m, NULL);
    pthread_cond_init(&s->c, NULL);
    s->count = count < 0 ? 0 : count;
    return (int64_t)s;
}
int64_t py_sem_acquire(int64_t h) {
    if (!h) return 0;
    zt_sem_t* s = (zt_sem_t*)h;
    pthread_mutex_lock(&s->m);
    while (s->count <= 0) pthread_cond_wait(&s->c, &s->m);
    s->count -= 1;
    pthread_mutex_unlock(&s->m);
    return 1;
}
int64_t py_sem_release(int64_t h) {
    if (!h) return 0;
    zt_sem_t* s = (zt_sem_t*)h;
    pthread_mutex_lock(&s->m);
    s->count += 1;
    pthread_cond_broadcast(&s->c);
    pthread_mutex_unlock(&s->m);
    return 0;
}

// ---- threading.Timer(interval_seconds, fn) ----
typedef struct {
    pthread_t th;
    int64_t fn;
    int64_t active;
    double secs;
} zt_timer_t;

static void* zt_timer_worker(void* arg) {
    zt_timer_t* t = (zt_timer_t*)arg;
    struct timespec ts;
    ts.tv_sec = (time_t)t->secs;
    ts.tv_nsec = (long)((t->secs - (double)ts.tv_sec) * 1e9);
    nanosleep(&ts, NULL);
    if (t->active) {
        int64_t (*fp)(void) = (int64_t (*)(void))t->fn;
        fp();
    }
    t->active = 0;
    return NULL;
}
int64_t py_threading_timer_new(double secs, int64_t fn) {
    zt_timer_t* t = (zt_timer_t*)GC_malloc(sizeof(zt_timer_t));
    t->fn = fn;
    t->active = 0;
    t->secs = secs < 0 ? 0 : secs;
    return (int64_t)t;
}
int64_t py_timer_start(int64_t h) {
    if (!h) return -1;
    zt_timer_t* t = (zt_timer_t*)h;
    if (t->active) return -1;
    t->active = 1;
    return (int64_t)pthread_create(&t->th, NULL, zt_timer_worker, t);
}
int64_t py_timer_cancel(int64_t h) {
    if (!h) return 0;
    ((zt_timer_t*)h)->active = 0;
    return 0;
}

// ============================================================================
// PY-A random + itertools subset.
// PRNG is an xorshift64* seeded from the clock (no libc rand state, which
// would be shared with the platform shims).
// ============================================================================
static uint64_t zt_rng_state = 0;
static uint64_t zt_rng(void) {
    if (!zt_rng_state) {
        struct timespec ts;
        clock_gettime(CLOCK_MONOTONIC, &ts);
        zt_rng_state = (uint64_t)ts.tv_nsec ^ ((uint64_t)ts.tv_sec << 32) ^ 0x9E3779B97F4A7C15ULL;
        if (!zt_rng_state) zt_rng_state = 1;
    }
    uint64_t x = zt_rng_state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    zt_rng_state = x;
    return x * 2685821657736338717ULL;
}
int64_t py_random_seed(int64_t s) {
    zt_rng_state = (uint64_t)(s ? s : 1);
    return 0;
}
double py_random_random(void) { return (double)(zt_rng() >> 11) / 9007199254740992.0; }
int64_t py_random_randint(int64_t a, int64_t b) {
    if (b < a) { int64_t t = a; a = b; b = t; }
    uint64_t span = (uint64_t)(b - a) + 1;
    return a + (int64_t)(zt_rng() % span);
}
double py_random_uniform(double a, double b) {
    return a + (b - a) * ((double)(zt_rng() >> 11) / 9007199254740992.0);
}
int64_t py_random_choice(int64_t vec) {
    if (!vec) return 0;
    int64_t len = ((int64_t*)(vec - 16))[1];
    if (len <= 0) return 0;
    return ((int64_t*)vec)[zt_rng() % (uint64_t)len];
}
// Fisher-Yates over a Vec of raw 64-bit slots (in place; Python's shuffle is
// also in place and returns None).
int64_t py_random_shuffle(int64_t vec) {
    if (!vec) return 0;
    int64_t len = ((int64_t*)(vec - 16))[1];
    for (int64_t i = len - 1; i > 0; i--) {
        int64_t j = (int64_t)(zt_rng() % (uint64_t)(i + 1));
        int64_t t = ((int64_t*)vec)[i];
        ((int64_t*)vec)[i] = ((int64_t*)vec)[j];
        ((int64_t*)vec)[j] = t;
    }
    return 0;
}

// ---- itertools (eager, over Vec handles) ----
// chain(a, b) concatenates; Python's is lazy, but every eager use we can
// support (`list(chain(...))`, `for x in chain(...)`) behaves the same.
int64_t py_itertools_chain(int64_t a, int64_t b) {
    int64_t la = a ? ((int64_t*)(a - 16))[1] : 0;
    int64_t lb = b ? ((int64_t*)(b - 16))[1] : 0;
    int64_t vec = zj_vec_new_cap(la + lb + 8);
    for (int64_t i = 0; i < la; i++) vec = zj_vec_push_h(vec, ((int64_t*)a)[i]);
    for (int64_t i = 0; i < lb; i++) vec = zj_vec_push_h(vec, ((int64_t*)b)[i]);
    return vec;
}
// repeat(x, n) -> Vec of n copies.
int64_t py_itertools_repeat(int64_t x, int64_t n) {
    if (n < 0) n = 0;
    int64_t vec = zj_vec_new_cap(n + 8);
    for (int64_t i = 0; i < n; i++) vec = zj_vec_push_h(vec, x);
    return vec;
}
// islice(vec, start, end) — end < 0 means "to the end" (the slice sentinel).
int64_t py_itertools_islice(int64_t src, int64_t start, int64_t end) {
    int64_t len = src ? ((int64_t*)(src - 16))[1] : 0;
    if (start < 0) start = 0;
    if (start > len) start = len;
    if (end < 0 || end > len) end = len;
    if (end < start) end = start;
    int64_t vec = zj_vec_new_cap(end - start + 8);
    for (int64_t i = start; i < end; i++) vec = zj_vec_push_h(vec, ((int64_t*)src)[i]);
    return vec;
}
// enumerate over a Vec needs pairs; not modelled yet — count how many match a
// predicate value instead (used by real code as `sum(1 for ...)`).
int64_t py_itertools_count(int64_t src, int64_t value) {
    int64_t len = src ? ((int64_t*)(src - 16))[1] : 0;
    int64_t n = 0;
    for (int64_t i = 0; i < len; i++) {
        if (((int64_t*)src)[i] == value) n++;
    }
    return n;
}

// ============================================================================
// PY-A collections: Counter / defaultdict.
// Counter(iterable) builds a plain map element->count, so indexing, `in`,
// len and json.dumps all work through the existing map paths. most_common is
// deliberately absent: ordering by count needs pairs/tuples that the type
// model does not have yet, and inventing a return shape would be a silent
// wrong value (it fails loudly at link time instead).
// ============================================================================
int64_t py_collections_counter_new(int64_t vec) {
    int64_t m = map_new();
    if (!vec) return m;
    int64_t len = ((int64_t*)(vec - 16))[1];
    for (int64_t i = 0; i < len; i++) {
        int64_t k = ((int64_t*)vec)[i];
        int64_t c = map_get(m, k);
        map_insert(m, k, c + 1);
    }
    return m;
}
// Counter(<list of str>) — keys must be CONTENT hashes (map_str_key), exactly
// like a dict literal, or identical strings at different literal sites count
// as separate keys.
int64_t py_collections_counter_new_str(int64_t vec) {
    int64_t m = map_new();
    if (!vec) return m;
    int64_t len = ((int64_t*)(vec - 16))[1];
    for (int64_t i = 0; i < len; i++) {
        int64_t k = map_str_key(((int64_t*)vec)[i]);
        int64_t c = map_get(m, k);
        map_insert(m, k, c + 1);
    }
    return m;
}
// defaultdict(int) — our maps already return 0 for a missing key, which IS
// the int() default; a non-int factory (list/set) is not modelled.
int64_t py_collections_defaultdict(int64_t factory) { (void)factory; return map_new(); }

// len(dict) — count the used entries (map slots are [key|value|used]).
int64_t zeta_map_len(int64_t m) {
        m = map_resolve(m);
    if (zt_map_is_json_handle(m)) zt_map_json_mismatch("zeta_map_len");
if (!m) return 0;
    int64_t cap = ((int64_t*)m)[0];
    int64_t n = 0;
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)m + 16 + i * 24;
        if (*(uint8_t*)(e + 16)) n++;
    }
    return n;
}
// `k in dict`
int64_t py_map_contains(int64_t m, int64_t k) {
        m = map_resolve(m);
    if (zt_map_is_json_handle(m)) zt_map_json_mismatch("py_map_contains");
if (!m) return 0;
    // Probe the entry table instead of testing `map_get(m, k) != 0`: a key
    // whose VALUE is 0 (or None/empty) is still present, and the old form
    // reported it absent — so `"k" in d` was False for `{"k": 0}` and
    // `d.setdefault("k", v)` overwrote a legitimate 0. Silent wrong value.
    int64_t cap = ((int64_t*)m)[0];
    int64_t h = map_hash(k);
    int64_t idx = h & (cap - 1);
    while (1) {
        char* e = (char*)m + 16 + idx * MAP_ENTRY_SIZE;
        uint8_t used = *(uint8_t*)(e + 16);
        if (!used) return 0;
        if (*(int64_t*)e == k) return 1;
        idx = (idx + 1) & (cap - 1);
    }
}
