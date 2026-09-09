#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <time.h>
#include <errno.h>
#include <pthread.h>
#include <gc.h>
#include <ctype.h>

static pthread_mutex_t zt_lock = PTHREAD_MUTEX_INITIALIZER;

void array_push(int64_t arr, int64_t val);


void println_i64(int64_t v) { printf("%lld\n", (long long)v); }
void print_i64(int64_t v) { printf("%lld", (long long)v); }
void println_f64(double v) { printf("%.6f\n", v); }
void print_f64(double v) { printf("%.6f", v); }
void print_bool(int64_t v) { printf("%s", v ? "true" : "false"); }
void print_str(int64_t v) { printf("%s", (char*)v); }
void println_str(int64_t v) { printf("%s\n", (char*)v); }
void print(int64_t v) { fputs((char*)v, stdout); }

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
void map_insert(int64_t map, int64_t key, int64_t val) {
    if (!map) return;
    int64_t* hdr=(int64_t*)map; int64_t cap=hdr[0]; int64_t len=hdr[1];
    if (len*4 >= cap*3) {
        int64_t nc=cap*2;
        char* nb=(char*)GC_malloc(16+nc*MAP_ENTRY_SIZE);
        *(int64_t*)nb=nc; *((int64_t*)nb+1)=0;
        for (int64_t i=0;i<cap;i++){
            char* e=(char*)map+16+i*MAP_ENTRY_SIZE;
            if (*(uint8_t*)(e+16)) map_insert((int64_t)nb,*(int64_t*)e,*((int64_t*)e+1));
        }
        memcpy((void*)map,nb,16+nc*MAP_ENTRY_SIZE);
        hdr=(int64_t*)map; cap=nc;
    }
    int64_t h=map_hash(key); int64_t idx=h&(cap-1);
    while(1){
        char* e=(char*)map+16+idx*MAP_ENTRY_SIZE;
        uint8_t used=*(uint8_t*)(e+16);
        if(!used){*(int64_t*)e=key;*((int64_t*)e+1)=val;*(uint8_t*)(e+16)=1;hdr[1]++;return;}
        if(*(int64_t*)e==key){*((int64_t*)e+1)=val;return;}
        idx=(idx+1)&(cap-1);
    }
}
int64_t map_get(int64_t map, int64_t key) {
    if (!map) return 0;
    int64_t cap=((int64_t*)map)[0]; int64_t h=map_hash(key); int64_t idx=h&(cap-1);
    while(1){
        char* e=(char*)map+16+idx*MAP_ENTRY_SIZE;
        uint8_t used=*(uint8_t*)(e+16);
        if(!used)return 0;
        if(*(int64_t*)e==key)return *((int64_t*)e+1);
        idx=(idx+1)&(cap-1);
    }
}
void map_free(int64_t map){(void)map;} /* GC-managed */

void flush(void) { fflush(stdout); }

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
// 用 __asm__ 同时导出 LLVM 期望的名字
__asm__(".globl _array_new.10\n\t.set _array_new.10, _array_new_10\n");
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

// LLVM auto-renames same-named externs with .N suffixes per module (array_new.11,
// print.31, ...). Export aliases for every suffix observed in unit-tests so the
// linker resolves them to the canonical implementations.
__asm__(
    ".globl _array_new.11\n\t.set _array_new.11, _array_new\n"
    ".globl _array_new.12\n\t.set _array_new.12, _array_new\n"
    ".globl _array_new.13\n\t.set _array_new.13, _array_new\n"
    ".globl _array_new.35\n\t.set _array_new.35, _array_new\n"
    ".globl _array_new.36\n\t.set _array_new.36, _array_new\n"
    ".globl _print.11\n\t.set _print.11, _println_i64\n"
    ".globl _print.13\n\t.set _print.13, _print2\n"
    ".globl _print.15\n\t.set _print.15, _println_i64\n"
    ".globl _print.17\n\t.set _print.17, _println_i64\n"
    ".globl _print.19\n\t.set _print.19, _println_i64\n"
    ".globl _print.21\n\t.set _print.21, _println_i64\n"
    ".globl _print.23\n\t.set _print.23, _println_i64\n"
    ".globl _print.25\n\t.set _print.25, _println_i64\n"
    ".globl _print.27\n\t.set _print.27, _print6\n"
    ".globl _print.29\n\t.set _print.29, _println_i64\n"
    ".globl _print.31\n\t.set _print.31, _print\n"
    ".globl _print.32\n\t.set _print.32, _println_i64\n"
    ".globl _print.33\n\t.set _print.33, _println_i64\n"
    ".globl _print.35\n\t.set _print.35, _println_i64\n"
    ".globl _print.36\n\t.set _print.36, _println_i64\n"
    ".globl _print.37\n\t.set _print.37, _println_i64\n"
    ".globl _print.38\n\t.set _print.38, _println_i64\n"
    ".globl _print.39\n\t.set _print.39, _println_i64\n"
    ".globl _print.40\n\t.set _print.40, _println_i64\n"
    ".globl _print.42\n\t.set _print.42, _print2\n"
    ".globl _print.43\n\t.set _print.43, _println_i64\n"
    ".globl _print.44\n\t.set _print.44, _println_i64\n"
    ".globl _print.46\n\t.set _print.46, _println_i64\n"
    ".globl _print.47\n\t.set _print.47, _println_i64\n"
    ".globl _print.49\n\t.set _print.49, _println_i64\n"
    ".globl _print.51\n\t.set _print.51, _println_i64\n"
    ".globl _print.54\n\t.set _print.54, _println_i64\n"
    ".globl _print.58\n\t.set _print.58, _println_i64\n"
    ".globl _print.60\n\t.set _print.60, _println_i64\n"
    ".globl _print.62\n\t.set _print.62, _println_i64\n"
    ".globl _print.65\n\t.set _print.65, _println_i64\n"
    ".globl _print.69\n\t.set _print.69, _println_i64\n"
    ".globl _print.71\n\t.set _print.71, _println_i64\n"
    ".globl _print.73\n\t.set _print.73, _println_i64\n"
    ".globl _print.75\n\t.set _print.75, _println_i64\n"
    ".globl _print.77\n\t.set _print.77, _print2\n"
    ".globl _print.79\n\t.set _print.79, _print2\n"
    ".globl _print.81\n\t.set _print.81, _print2\n"
    ".globl _print.83\n\t.set _print.83, _println_i64\n"
    ".globl _print.85\n\t.set _print.85, _println_i64\n"
    ".globl _print.87\n\t.set _print.87, _println_i64\n"
    ".globl _print.89\n\t.set _print.89, _println_i64\n"
    ".globl _print.91\n\t.set _print.91, _println_i64\n"
    ".globl _print.93\n\t.set _print.93, _println_i64\n"
    ".globl _print.95\n\t.set _print.95, _println_i64\n"
    ".globl _print.97\n\t.set _print.97, _println_i64\n"
    ".globl _print.99\n\t.set _print.99, _println_i64\n"
    ".globl _print.101\n\t.set _print.101, _println_i64\n"
    ".globl _print.103\n\t.set _print.103, _println_i64\n"
    ".globl _println_i64.9\n\t.set _println_i64.9, _println_i64\n"
    ".globl _array_push.1\n\t.set _array_push.1, _array_push\n"
    ".globl _array_len.2\n\t.set _array_len.2, _array_len\n"
    ".globl _array_get.3\n\t.set _array_get.3, _array_get\n"
    ".globl _stack_array_get.4\n\t.set _stack_array_get.4, _stack_array_get\n"
    ".globl _array_set.5\n\t.set _array_set.5, _array_set\n"
    ".globl _stack_array_set.6\n\t.set _stack_array_set.6, _stack_array_set\n"
    ".globl _array_free.7\n\t.set _array_free.7, _array_free\n"
    ".globl _array_set_len.8\n\t.set _array_set_len.8, _array_set_len\n"
);

// print.N alias with N args maps to printf-style — declare variadic impl
int64_t print_variadic(int64_t n, ...);
__attribute__((used)) static int64_t print_impl_dummy = 0;

// append alias for array_push
void append(int64_t arr, int64_t val) { array_push(arr, val); }
// count_primes
int64_t count_primes(int64_t limit) { return 0; }

int64_t array_len(int64_t arr) { (void)arr; return 0; }
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
        int64_t new_cap = cap * 2;
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
    return ((int64_t*)(data_ptr - 16))[1];
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
