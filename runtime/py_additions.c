// runtime/py_additions.c — PY-A Python-compat runtime additions.
// Merged into tokio_runtime.o via: ld -r tokio_runtime.o py_additions.o
// (kept separate because tokio_runtime.c's epoll part does not compile on
// macOS; do not duplicate these symbols in tokio_runtime_stub.c)

#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <gc.h>
#include <stdlib.h>

// Python-style string equality (by content, not pointer)
int64_t str_eq(int64_t a, int64_t b) {
    if (!a || !b) { return a == b ? 1 : 0; }
    return strcmp((char*)a, (char*)b) == 0 ? 1 : 0;
}
int64_t host_str_eq(int64_t a, int64_t b) { return str_eq(a, b); }

// str(f64) — %g keeps Python's compact float repr ("2.5", not "2.500000")
int64_t to_string_f64(double v) {
    char* s = (char*)GC_malloc(32);
    snprintf(s, 32, "%g", v);
    return (int64_t)s;
}

// str_split(s, sep) — Python s.split(sep): returns a Vec-layout handle
// ([cap | len | data...]) whose elements are GC string handles.
int64_t str_split(int64_t s, int64_t sep) {
    if (!s) return 0;
    const char* text = (const char*)s;
    const char* pat = sep ? (const char*)sep : "";
    size_t plen = strlen(pat);
    if (plen == 0) {
        // V1: empty separator unsupported — return the whole string as [s]
        int64_t* base = (int64_t*)GC_malloc(16 + 8 * 8);
        base[0] = 8; base[1] = 1; base[2] = s;
        return (int64_t)(base + 2);
    }
    int64_t count = 1;
    for (const char* p = text; (p = strstr(p, pat)) != NULL; p += plen) count++;
    int64_t cap = count < 8 ? 8 : count;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap; base[1] = 0;
    const char* start = text;
    for (;;) {
        const char* hit = strstr(start, pat);
        size_t slen = hit ? (size_t)(hit - start) : strlen(start);
        char* piece = (char*)GC_malloc(slen + 1);
        memcpy(piece, start, slen);
        piece[slen] = 0;
        base[2 + base[1]] = (int64_t)piece;
        base[1] += 1;
        if (!hit) break;
        start = hit + plen;
    }
    return (int64_t)(base + 2);
}
int64_t host_str_split(int64_t s, int64_t sep) { return str_split(s, sep); }

// map_str_key — deterministic 64-bit FNV-1a content hash for string dict
// keys. The open-addressing map hashes/compares keys numerically; string
// handles differ per literal site, so content-keyed dicts must normalize.
int64_t map_str_key(int64_t handle) {
    if (!handle) return 0;
    const unsigned char* p = (const unsigned char*)handle;
    uint64_t h = 1469598103934665603ULL;
    while (*p) { h ^= (uint64_t)*p++; h *= 1099511628211ULL; }
    return (int64_t)(h ? h : 1);
}

// zeta_slice_vec(data, start, count) — Python arr[start:end]. `data` is a
// raw element pointer for stack arrays (count >= 0 given) or a Vec-layout
// handle for dynamic arrays (count < 0 → read len from the header). Returns
// a Vec-layout handle so len()/indexing work uniformly.
int64_t zeta_slice_vec(int64_t data, int64_t start, int64_t end) {
    if (!data) return 0;
    // Python semantics: end is EXCLUSIVE. end < 0 → slice to the end (reads
    // the Vec header; only valid for dynamic-array handles).
    int64_t n;
    if (end < 0) {
        n = ((int64_t*)(data - 16))[1] - start;
    } else {
        n = end - start;
    }
    if (n < 0) n = 0;
    int64_t cap = n < 8 ? 8 : n;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap; base[1] = n;
    for (int64_t i = 0; i < n; i++) base[2 + i] = ((int64_t*)data)[start + i];
    return (int64_t)(base + 2);
}

// map_keys / map_values — iterate the open-addressing table (entries at
// map+16, MAP_ENTRY_SIZE=24 bytes: [key | value | used]) into Vec handles.
int64_t map_keys(int64_t map) {
    if (!map) return 0;
    int64_t cap = ((int64_t*)map)[0];
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(cap ? cap : 8) * 8);
    base[0] = cap ? cap : 8; base[1] = 0;
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)map + 16 + i * 24;
        if (*(uint8_t*)(e + 16)) {
            base[2 + base[1]] = *(int64_t*)e;
            base[1] += 1;
        }
    }
    return (int64_t)(base + 2);
}
int64_t map_values(int64_t map) {
    if (!map) return 0;
    int64_t cap = ((int64_t*)map)[0];
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(cap ? cap : 8) * 8);
    base[0] = cap ? cap : 8; base[1] = 0;
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)map + 16 + i * 24;
        if (*(uint8_t*)(e + 16)) {
            base[2 + base[1]] = *((int64_t*)e + 1);
            base[1] += 1;
        }
    }
    return (int64_t)(base + 2);
}

// dict .get(k, default) — missing key returns the default (map_get returns 0)
#define MAP_ENTRY_SIZE 24 // matches tokio_runtime_stub.c map layout

// private copy of the stub's static mixxxer hash (not exported there)
static int64_t py_map_hash(int64_t key) {
    uint64_t h = (uint64_t)key;
    h ^= h >> 33; h *= 0xff51afd7ed558ccdULL;
    h ^= h >> 33; h *= 0xc4ceb9fe1a85ec53ULL;
    h ^= h >> 33;
    return (int64_t)h;
}

int64_t map_get_default(int64_t map, int64_t key, int64_t def) {
    if (!map) return def;
    int64_t cap = ((int64_t*)map)[0];
    int64_t h = py_map_hash(key);
    int64_t idx = h & (cap - 1);
    while (1) {
        char* e = (char*)map + 16 + idx * MAP_ENTRY_SIZE;
        uint8_t used = *(uint8_t*)(e + 16);
        if (!used) return def;
        if (*(int64_t*)e == key) return *((int64_t*)e + 1);
        idx = (idx + 1) & (cap - 1);
    }
}

// Python builtins: abs / min / max (i64 + f64 variants)
int64_t zeta_abs_i64(int64_t v) { return v < 0 ? -v : v; }
double zeta_abs_f64(double v) { return v < 0 ? -v : v; }
int64_t zeta_min_i64(int64_t a, int64_t b) { return a < b ? a : b; }
int64_t zeta_max_i64(int64_t a, int64_t b) { return a > b ? a : b; }
double zeta_min_f64(double a, double b) { return a < b ? a : b; }
double zeta_max_f64(double a, double b) { return a > b ? a : b; }

// sum(arr) — dynamic arrays read the Vec header; static arrays pass n
int64_t zeta_sum_vec(int64_t data) {
    if (!data) return 0;
    int64_t len = ((int64_t*)(data - 16))[1];
    int64_t acc = 0;
    for (int64_t i = 0; i < len; i++) acc += ((int64_t*)data)[i];
    return acc;
}
int64_t zeta_sum_n(int64_t data, int64_t n) {
    int64_t acc = 0;
    for (int64_t i = 0; i < n; i++) acc += ((int64_t*)data)[i];
    return acc;
}

// f-string format spec: zeta_fmt_f64(v, ".2f") → snprintf("%<spec>", v)
int64_t zeta_fmt_f64_spec(double v, int64_t spec) {
    char* s = (char*)GC_malloc(64);
    char fmt[40];
    fmt[0] = '%';
    const char* sp = spec ? (const char*)spec : "";
    size_t i = 0;
    for (; i < 30 && sp[i]; i++) fmt[1 + i] = sp[i];
    fmt[1 + i] = 0;
    snprintf(s, 64, fmt, v);
    return (int64_t)s;
}

// ── PY-A: try/except via error-state polling ────────────────────────
// `raise` records a global error code; the desugared try body wraps each
// statement in `if (zeta_last_error() == 0)` so raising skips the rest;
// the trailing `if (err != 0)` runs the handler. Robust — no setjmp.
static _Thread_local int64_t zt_error = 0;

int64_t zeta_clear_error(void) {
    zt_error = 0;
    return 0;
}
int64_t zeta_set_error(int64_t code) {
    zt_error = code;
    return code;
}
static int64_t py_poll_error(void) {
    return zt_error;
}

// ── PY-A: setjmp/longjmp exception machinery (V2 — real semantics) ──
#include <setjmp.h>
// `raise` longjmps to the innermost try frame IMMEDIATELY (interrupting the
// current function), unlike the error-state polling model.
static _Thread_local jmp_buf zt_jmps2[64];
static _Thread_local int64_t zt_jcodes2[64];
static _Thread_local int zt_jtop2 = -1;

int64_t zeta_try_enter(void) {
    if (zt_jtop2 + 1 >= 64) return -1;
    zt_jtop2++;
    zt_jcodes2[zt_jtop2] = 0;
    return zt_jtop2;
}
// Generated code calls _setjmp(zeta_try_slot()) directly; longjmp returns
// there a second time with the handler branch re-evaluating.
void* zeta_try_slot(void) {
    if (zt_jtop2 < 0) return 0;
    return (void*)&zt_jmps2[zt_jtop2];
}
int64_t zeta_try_end(void) {
    if (zt_jtop2 >= 0) zt_jtop2--;
    return 0;
}
int64_t zeta_raise(int64_t code) {
    if (zt_jtop2 >= 0) {
        zt_jcodes2[zt_jtop2] = code;
        // MUST pair with _setjmp (the no-signal-mask variant the generated
        // code declares); longjmp/_setjmp pairing is UB and silently no-ops
        // under some libSystem builds.
        _longjmp(zt_jmps2[zt_jtop2], 1);
    }
    fprintf(stderr, "Unhandled exception: code=%lld\n", (long long)code);
    exit(1);
}
int64_t zeta_last_error(void) {
    return zt_jtop2 >= 0 ? zt_jcodes2[zt_jtop2] : 0;
}

// assert(cond, msg) failure path: print and abort (Python AssertionError)
void zeta_assert_fail(int64_t msg) {
    fprintf(stderr, "AssertionError: %s\n", msg ? (const char*)msg : "");
    exit(1);
}

// BitArray ops on an opaque byte-buffer handle ([cap_bytes | data...] i64
// words). V1: bit i lives in word i/64, bit i%64. set_bit(v!=0 → 1).
int64_t zeta_bit_get(int64_t buf, int64_t i) {
    int64_t word = ((int64_t*)buf)[1 + i / 64];
    return (word >> (i % 64)) & 1;
}
int64_t zeta_bit_set(int64_t buf, int64_t i, int64_t v) {
    int64_t* words = (int64_t*)buf;
    if (v) words[1 + i / 64] |= (int64_t)1 << (i % 64);
    else words[1 + i / 64] &= ~((int64_t)1 << (i % 64));
    return 0;
}

// memory::BitArray::new(n) — [cap_bits | words...] zero-initialized
int64_t zeta_bitarray_new(int64_t nbits) {
    int64_t words = (nbits + 63) / 64;
    int64_t* buf = (int64_t*)GC_malloc((size_t)(1 + words) * 8);
    buf[0] = nbits;
    for (int64_t i = 0; i < words; i++) buf[1 + i] = 0;
    return (int64_t)buf;
}
// memory::DynamicArray::new(cap) — same [cap | len | data...] layout as the
// Vec runtime so vec_push/vec_get/vec_len work uniformly. Cap 0 → 8.
int64_t zeta_dynarray_new(int64_t cap) {
    if (cap < 8) cap = 8;
    int64_t* buf = (int64_t*)GC_malloc((size_t)(2 + cap) * 8);
    buf[0] = cap;
    buf[1] = 0;
    for (int64_t i = 0; i < cap; i++) buf[2 + i] = 0;
    return (int64_t)(buf + 2);
}

// memory::Sieve — V1 platform object: [limit | flags bytes...]
// zeta_sieve_new(limit) → handle; zeta_sieve_run(h) does a real sieve of
// Eratosthenes; zeta_sieve_count(h) counts unmarked >= 2.
int64_t zeta_sieve_new(int64_t limit) {
    if (limit < 0) limit = 0;
    char* buf = (char*)GC_malloc((size_t)limit + 1);
    for (int64_t i = 0; i <= limit; i++) buf[i] = 1;
    int64_t* h = (int64_t*)GC_malloc(16);
    h[0] = limit;
    h[1] = (int64_t)buf;
    return (int64_t)h;
}
int64_t zeta_sieve_run(int64_t h) {
    if (!h) return 0;
    int64_t limit = ((int64_t*)h)[0];
    char* is = (char*)((int64_t*)h)[1];
    for (int64_t p = 2; p * p <= limit; p++) {
        if (is[p]) {
            for (int64_t m = p * p; m <= limit; m += p) is[m] = 0;
        }
    }
    return 0;
}
int64_t zeta_sieve_count(int64_t h) {
    if (!h) return 0;
    int64_t limit = ((int64_t*)h)[0];
    char* is = (char*)((int64_t*)h)[1];
    int64_t c = 0;
    for (int64_t i = 2; i <= limit; i++) c += is[i];
    return c;
}

// std::quantum::QuantumCircuit V1 placeholder object: [qubits | reserved...]
// gate methods are no-ops; execute returns (handle, 0) as a unit-ish value.
int64_t zeta_qc_new(int64_t qubits) {
    int64_t* h = (int64_t*)GC_malloc(16);
    h[0] = qubits;
    h[1] = 0;
    return (int64_t)h;
}
int64_t zeta_qc_noop1(int64_t h) { return 0; }
int64_t zeta_qc_noop2(int64_t h, int64_t a) { return 0; }
int64_t zeta_qc_noop3(int64_t h, int64_t a, int64_t b) { return 0; }
int64_t zeta_qc_measure(int64_t h, int64_t q) { return 0; }
int64_t zeta_qc_execute(int64_t h) { return h; }
int64_t zeta_qc_is_normalized(int64_t h) { return 1; }
