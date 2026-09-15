// runtime/py_additions.c — PY-A Python-compat runtime additions.
// Merged into tokio_runtime.o via: ld -r tokio_runtime.o py_additions.o
// (kept separate because tokio_runtime.c's epoll part does not compile on
// macOS; do not duplicate these symbols in tokio_runtime_stub.c)

#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <gc.h>
#include <stdlib.h>
#include <stdarg.h>
#include <ctype.h>

static int64_t zt_vec_len(int64_t v);

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
// Side table hash -> string pointer. Dict keys are stored as content hashes,
// which is great for lookup and terrible for serialization: json.dumps(dict)
// needs the key text back. Recording the first string seen for a hash makes
// keys recoverable (a hash collision would pick the earlier string — a
// documented 64-bit-FNV ceiling, not a silent corruption).
#define ZT_KEYSTR_CAP 8192
static int64_t g_keystr_hash[ZT_KEYSTR_CAP];
static int64_t g_keystr_ptr[ZT_KEYSTR_CAP];

int64_t map_str_key(int64_t handle) {
    if (!handle) return 0;
    const unsigned char* p = (const unsigned char*)handle;
    uint64_t h = 1469598103934665603ULL;
    while (*p) { h ^= (uint64_t)*p++; h *= 1099511628211ULL; }
    h = h ? h : 1;
    uint64_t idx = h & (ZT_KEYSTR_CAP - 1);
    for (int i = 0; i < ZT_KEYSTR_CAP; i++) {
        uint64_t j = (idx + (uint64_t)i) & (ZT_KEYSTR_CAP - 1);
        if (g_keystr_hash[j] == 0) {
            g_keystr_hash[j] = (int64_t)h;
            g_keystr_ptr[j] = handle;
            break;
        }
        if ((uint64_t)g_keystr_hash[j] == h) break;
    }
    return (int64_t)h;
}

// String recorded for a dict key hash (0 when unknown, e.g. non-string keys).
int64_t zeta_key_string(int64_t hash) {
    if (!hash) return 0;
    uint64_t idx = (uint64_t)hash & (ZT_KEYSTR_CAP - 1);
    for (int i = 0; i < ZT_KEYSTR_CAP; i++) {
        uint64_t j = (idx + (uint64_t)i) & (ZT_KEYSTR_CAP - 1);
        if (g_keystr_hash[j] == 0) return 0;
        if (g_keystr_hash[j] == hash) return g_keystr_ptr[j];
    }
    return 0;
}

// zeta_slice_vec(data, start, count) — Python arr[start:end]. `data` is a
// raw element pointer for stack arrays (count >= 0 given) or a Vec-layout
// handle for dynamic arrays (count < 0 → read len from the header). Returns
// a Vec-layout handle so len()/indexing work uniformly.
// Validate a Vec-layout header before trusting it. A handle that is NOT a Vec
// (e.g. a string, or a value whose static type was unknown to the compiler)
// used to yield a garbage capacity and an absurd allocation -> OOM instead of
// a diagnosable failure.
static int zt_vec_header_ok(int64_t data, int64_t* len_out) {
    if (!data) return 0;
    int64_t* h = (int64_t*)(data - 16);
    int64_t cap = h[0], len = h[1];
    // len > cap happens for array kinds grown by a stub push; only reject
    // obviously-insane values (this guard exists to stop absurd allocations
    // from a non-Vec handle, not to police the invariant).
    if (cap < 0 || len < 0 || len > (1 << 28) || cap > (1 << 28)) return 0;
    *len_out = len;
    return 1;
}

int64_t zeta_slice_vec(int64_t data, int64_t start, int64_t end) {
    if (!data) return 0;
    // Guard the header read below.
    {
        int64_t probe = 0;
        if (!zt_vec_header_ok(data, &probe)) return 0;
    }
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
// String keys are stored as content hashes; recover the original text for
// display (falls back to the raw key for non-string keys).
static int64_t zt_key_display(int64_t key) {
    int64_t s = zeta_key_string(key);
    return s ? s : key;
}

int64_t map_keys(int64_t map) {
    if (!map) return 0;
    int64_t cap = ((int64_t*)map)[0];
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(cap ? cap : 8) * 8);
    base[0] = cap ? cap : 8; base[1] = 0;
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)map + 16 + i * 24;
        if (*(uint8_t*)(e + 16)) {
            base[2 + base[1]] = zt_key_display(*(int64_t*)e);
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

// collections.Counter(...).most_common([n]) — a Vec of (key, count) pairs in
// descending count order. Each pair is a 2-slot handle, so
// `for k, c in counter.most_common():` destructures it. Keys go through
// zt_key_display, so string keys come back as the original text.
static int64_t zt_map_most_common(int64_t map, int64_t limit) {
    if (!map) return 0;
    int64_t cap = ((int64_t*)map)[0];
    if (cap < 0) cap = 0;
    int64_t* keys = (int64_t*)GC_malloc((size_t)(cap ? cap : 1) * 8);
    int64_t* vals = (int64_t*)GC_malloc((size_t)(cap ? cap : 1) * 8);
    int64_t n = 0;
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)map + 16 + i * 24;
        if (*(uint8_t*)(e + 16)) {
            keys[n] = zt_key_display(*(int64_t*)e);
            vals[n] = *((int64_t*)e + 1);
            n++;
        }
    }
    for (int64_t i = 1; i < n; i++) {
        int64_t kk = keys[i], vv = vals[i];
        int64_t j = i - 1;
        while (j >= 0 && vals[j] < vv) {
            keys[j + 1] = keys[j];
            vals[j + 1] = vals[j];
            j--;
        }
        keys[j + 1] = kk;
        vals[j + 1] = vv;
    }
    if (limit > 0 && limit < n) n = limit;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(n ? n : 1) * 8);
    base[0] = n ? n : 1;
    base[1] = n;
    for (int64_t i = 0; i < n; i++) {
        int64_t* pair = (int64_t*)GC_malloc(16);
        pair[0] = keys[i];
        pair[1] = vals[i];
        base[2 + i] = (int64_t)pair;
    }
    return (int64_t)(base + 2);
}
int64_t py_map_most_common(int64_t map) { return zt_map_most_common(map, 0); }
int64_t py_map_most_common_2(int64_t map, int64_t limit) {
    return zt_map_most_common(map, limit);
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

// ── PY-A: Python format specs (f"{x:>8.2f}", f"{n:05d}", f"[{s:^7}]") ──
// The naive "prepend '%'" trick only works for a bare `.2f`; anything with
// fill/align/zero-padding produced an invalid C conversion and printed the
// spec text itself. Parse the Python spec and pad manually instead.
typedef struct {
    char fill;
    char align; /* 0 = default */
    char type;  /* 0 = none */
    int width;
    int zero;
    int has_precision;
    int precision;
    int numeric;
} zt_fmt_t;

static void zt_parse_spec(const char* s, zt_fmt_t* f) {
    f->fill = ' '; f->align = 0; f->type = 0; f->width = 0;
    f->zero = 0; f->has_precision = 0; f->precision = 0; f->numeric = 0;
    const char* p = s ? s : "";
    if (p[0] && p[1] && (p[1] == '<' || p[1] == '>' || p[1] == '^')) {
        f->fill = p[0]; f->align = p[1]; p += 2;
    } else if (*p == '<' || *p == '>' || *p == '^') {
        f->align = *p; p++;
    }
    if (*p == '+' || *p == '-' || *p == ' ') p++;
    if (*p == '#') p++;
    if (*p == '0') { f->zero = 1; p++; }
    while (*p >= '0' && *p <= '9') { f->width = f->width * 10 + (*p - '0'); p++; }
    if (*p == ',') p++;
    if (*p == '.') {
        p++;
        f->has_precision = 1;
        while (*p >= '0' && *p <= '9') { f->precision = f->precision * 10 + (*p - '0'); p++; }
    }
    if (*p) f->type = *p;
}

static int64_t zt_fmt_pad(const char* body, zt_fmt_t* f) {
    size_t n = strlen(body);
    if (f->width <= (int64_t)n) return (int64_t)GC_strdup(body);
    size_t pad = (size_t)f->width - n;
    char al = f->align;
    if (!al) al = f->numeric ? '>' : '<';
    char fill = f->fill;
    size_t left = 0, right = 0;
    if (f->zero && !f->align && f->numeric) {
        fill = '0';
        left = pad;
    } else if (al == '>') {
        left = pad;
    } else if (al == '<') {
        right = pad;
    } else {
        left = pad / 2;
        right = pad - left;
    }
    char* out = (char*)GC_malloc((size_t)f->width + 1);
    // Zero padding goes after a leading sign, not before it.
    if (fill == '0' && n > 0 && (body[0] == '-' || body[0] == '+')) {
        out[0] = body[0];
        memset(out + 1, fill, left);
        memcpy(out + 1 + left, body + 1, n - 1);
        out[1 + left + n - 1] = 0;
        return (int64_t)out;
    }
    memset(out, fill, left);
    memcpy(out + left, body, n);
    memset(out + left + n, fill, right);
    out[left + n + right] = 0;
    return (int64_t)out;
}

int64_t py_fmt_i64(int64_t v, int64_t spec) {
    zt_fmt_t f;
    zt_parse_spec(spec ? (const char*)spec : "", &f);
    f.numeric = 1;
    char body[160];
    char t = f.type ? f.type : 'd';
    if (t == 'b') {
        unsigned long long u = (unsigned long long)v;
        char tmp[160];
        int k = 0;
        if (!u) tmp[k++] = '0';
        while (u && k < 159) { tmp[k++] = (char)('0' + (u & 1)); u >>= 1; }
        for (int a = 0, b = k - 1; a < b; a++, b--) { char c = tmp[a]; tmp[a] = tmp[b]; tmp[b] = c; }
        tmp[k] = 0;
        return zt_fmt_pad(tmp, &f);
    } else if (t == 'x' || t == 'X' || t == 'o') {
        snprintf(body, sizeof body, t == 'x' ? "%llx" : (t == 'X' ? "%llX" : "%llo"),
                 (unsigned long long)v);
    } else if (t == 'f' || t == 'F' || t == 'e' || t == 'E' || t == 'g' || t == 'G') {
        char cfmt[24];
        snprintf(cfmt, sizeof cfmt, f.has_precision ? "%%.%d%c" : "%%%c", f.precision, t);
        snprintf(body, sizeof body, cfmt, (double)v);
    } else {
        snprintf(body, sizeof body, "%lld", (long long)v);
    }
    return zt_fmt_pad(body, &f);
}

int64_t py_fmt_f64(double v, int64_t spec) {
    zt_fmt_t f;
    zt_parse_spec(spec ? (const char*)spec : "", &f);
    f.numeric = 1;
    char body[160];
    char t = f.type ? f.type : 'f';
    if (t != 'f' && t != 'F' && t != 'e' && t != 'E' && t != 'g' && t != 'G') t = 'f';
    char cfmt[24];
    if (f.has_precision) {
        snprintf(cfmt, sizeof cfmt, "%%.%d%c", f.precision, t);
    } else if (t == 'f' || t == 'F') {
        snprintf(cfmt, sizeof cfmt, "%%.6%c", t); /* Python's default float repr */
    } else {
        snprintf(cfmt, sizeof cfmt, "%%%c", t);
    }
    snprintf(body, sizeof body, cfmt, v);
    return zt_fmt_pad(body, &f);
}

int64_t py_fmt_str(int64_t s, int64_t spec) {
    zt_fmt_t f;
    zt_parse_spec(spec ? (const char*)spec : "", &f);
    const char* p = s ? (const char*)s : "";
    char* body = (char*)p;
    if (f.has_precision) {
        size_t n = strlen(p);
        if ((int64_t)n > f.precision) {
            body = (char*)GC_malloc((size_t)f.precision + 1);
            memcpy(body, p, (size_t)f.precision);
            body[f.precision] = 0;
        }
    }
    return zt_fmt_pad(body, &f);
}

// ── PY-A: zip(a, b) ─────────────────────────────────────────────────
// A Vec of 2-slot pair handles, length = min(len(a), len(b)); combined with
// the `for x, y in ...` destructure this covers the common zipped loop.
int64_t py_zip(int64_t a, int64_t b) {
    int64_t na = a ? ((int64_t*)(a - 16))[1] : 0;
    int64_t nb = b ? ((int64_t*)(b - 16))[1] : 0;
    int64_t n = na < nb ? na : nb;
    if (n < 0) n = 0;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(n ? n : 1) * 8);
    base[0] = n ? n : 1;
    base[1] = n;
    for (int64_t i = 0; i < n; i++) {
        int64_t* pair = (int64_t*)GC_malloc(16);
        pair[0] = ((int64_t*)a)[i];
        pair[1] = ((int64_t*)b)[i];
        base[2 + i] = (int64_t)pair;
    }
    return (int64_t)(base + 2);
}

// ── PY-A: sorted(xs, reverse=True) — sort then reverse in place ──────
extern int64_t zeta_sorted_vec_len(int64_t vec, int64_t len);
int64_t py_sorted_vec_rev(int64_t vec, int64_t len, int64_t rev) {
    int64_t out = zeta_sorted_vec_len(vec, len);
    if (rev && out) {
        int64_t n = ((int64_t*)(out - 16))[1];
        for (int64_t i = 0, j = n - 1; i < j; i++, j--) {
            int64_t t = ((int64_t*)out)[i];
            ((int64_t*)out)[i] = ((int64_t*)out)[j];
            ((int64_t*)out)[j] = t;
        }
    }
    return out;
}

// ── PY-A: d.items() — Vec of (key, value) pairs, keys via the hash side
// table so string keys come back as text. ────────────────────────────
int64_t py_map_items(int64_t map) {
    if (!map) return 0;
    int64_t cap = ((int64_t*)map)[0];
    if (cap < 0) cap = 0;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(cap ? cap : 1) * 8);
    base[0] = cap ? cap : 1;
    base[1] = 0;
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)map + 16 + i * 24;
        if (*(uint8_t*)(e + 16)) {
            int64_t* pair = (int64_t*)GC_malloc(16);
            pair[0] = zt_key_display(*(int64_t*)e);
            pair[1] = *((int64_t*)e + 1);
            base[2 + base[1]] = (int64_t)pair;
            base[1] += 1;
        }
    }
    return (int64_t)(base + 2);
}

// ── PY-A: integer power `2 ** 10` ────────────────────────────────────
// A negative exponent would be a float in Python; this returns 0 for that
// case rather than pretending (float power goes through libm).
int64_t zeta_pow_i64(int64_t base, int64_t exp) {
    if (exp < 0) return 0;
    int64_t r = 1;
    while (exp > 0) {
        if (exp & 1) r *= base;
        base *= base;
        exp >>= 1;
    }
    return r;
}

// ── PY-A: re.escape + list.index/count ───────────────────────────────
int64_t py_re_escape(int64_t s) {
    const char* p = s ? (const char*)s : "";
    size_t n = strlen(p);
    char* out = (char*)GC_malloc(n * 2 + 1);
    size_t k = 0;
    for (size_t i = 0; i < n; i++) {
        char c = p[i];
        if (strchr(".^$*+?()[]{}|\\", c)) out[k++] = '\\';
        out[k++] = c;
    }
    out[k] = 0;
    return (int64_t)out;
}

// ── PY-A: strided slices `s[::-1]`, `a[::2]` ─────────────────────────
static int64_t zt_vec_len(int64_t v);
// INT64_MIN marks an omitted bound; with a negative step an omitted start
// means "from the end". Step 0 yields an empty result.
int64_t str_slice_step(int64_t sh, int64_t start, int64_t end, int64_t step) {
    const char* p = sh ? (const char*)sh : "";
    int64_t n = (int64_t)strlen(p);
    if (step == 0) return (int64_t)GC_strdup("");
    int64_t st, en;
    if (start == INT64_MIN) {
        st = (step > 0) ? 0 : n - 1;
    } else {
        st = start;
        if (st < 0) st += n;
    }
    if (end == INT64_MIN) {
        en = (step > 0) ? n : -1;
    } else {
        en = end;
        if (en < 0) en += n;
    }
    char* out = (char*)GC_malloc((size_t)n + 1);
    size_t k = 0;
    if (step > 0) {
        if (st < 0) st = 0;
        if (en > n) en = n;
        for (int64_t i = st; i < en; i += step) out[k++] = p[i];
    } else {
        if (st > n - 1) st = n - 1;
        if (en < -1) en = -1;
        for (int64_t i = st; i > en; i += step) {
            if (i >= 0 && i < n) out[k++] = p[i];
        }
    }
    out[k] = 0;
    return (int64_t)out;
}

int64_t zeta_slice_vec_step(int64_t vec, int64_t start, int64_t end, int64_t step) {
    int64_t n = zt_vec_len(vec);
    int64_t cap0 = n > 0 ? n : 1;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap0 * 8);
    base[0] = cap0;
    base[1] = 0;
    if (step == 0) return (int64_t)(base + 2);
    int64_t st, en;
    if (start == INT64_MIN) {
        st = (step > 0) ? 0 : n - 1;
    } else {
        st = start;
        if (st < 0) st += n;
    }
    if (end == INT64_MIN) {
        en = (step > 0) ? n : -1;
    } else {
        en = end;
        if (en < 0) en += n;
    }
    if (step > 0) {
        if (st < 0) st = 0;
        if (en > n) en = n;
        for (int64_t i = st; i < en; i += step) base[2 + base[1]++] = ((int64_t*)vec)[i];
    } else {
        if (st > n - 1) st = n - 1;
        if (en < -1) en = -1;
        for (int64_t i = st; i > en; i += step) {
            if (i >= 0 && i < n) base[2 + base[1]++] = ((int64_t*)vec)[i];
        }
    }
    return (int64_t)(base + 2);
}

// ── PY-A: string/list operators Python adds on top of arithmetic ─────
// `"-" * 40`, `[0] * 3`, `[1] + [2]` — without these the numeric operators
// ran on the handles and produced garbage values.
int64_t host_str_repeat(int64_t s, int64_t n) {
    const char* p = s ? (const char*)s : "";
    if (n < 0) n = 0;
    size_t len = strlen(p);
    size_t total = len * (size_t)n;
    char* out = (char*)GC_malloc(total + 1);
    for (int64_t i = 0; i < n; i++) memcpy(out + (size_t)i * len, p, len);
    out[total] = 0;
    return (int64_t)out;
}

static int64_t zt_vec_len(int64_t v) {
    return v ? ((int64_t*)(v - 16))[1] : 0;
}

int64_t py_array_concat(int64_t a, int64_t b) {
    int64_t na = zt_vec_len(a), nb = zt_vec_len(b);
    int64_t n = na + nb;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(n ? n : 1) * 8);
    base[0] = n ? n : 1;
    base[1] = n;
    for (int64_t i = 0; i < na; i++) base[2 + i] = ((int64_t*)a)[i];
    for (int64_t i = 0; i < nb; i++) base[2 + na + i] = ((int64_t*)b)[i];
    return (int64_t)(base + 2);
}

int64_t py_array_repeat(int64_t a, int64_t n) {
    int64_t len = zt_vec_len(a);
    if (n < 0) n = 0;
    int64_t total = len * n;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(total ? total : 1) * 8);
    base[0] = total ? total : 1;
    base[1] = total;
    for (int64_t k = 0; k < n; k++) {
        for (int64_t i = 0; i < len; i++) {
            base[2 + k * len + i] = ((int64_t*)a)[i];
        }
    }
    return (int64_t)(base + 2);
}

// ── PY-A: chr(n) / ord(s) ────────────────────────────────────────────
int64_t py_builtin_chr(int64_t n) {
    char* s = (char*)GC_malloc(2);
    s[0] = (char)(n & 0xFF);
    s[1] = 0;
    return (int64_t)s;
}
int64_t py_builtin_ord(int64_t s) {
    const char* p = s ? (const char*)s : "";
    return (int64_t)(unsigned char)p[0];
}

// ── PY-A: max(xs) / min(xs) — the 1-argument form over an i64 array ──
int64_t py_builtin_max(int64_t vec) {
    int64_t n = vec ? ((int64_t*)(vec - 16))[1] : 0;
    if (n <= 0) return 0;
    int64_t best = ((int64_t*)vec)[0];
    for (int64_t i = 1; i < n; i++) {
        int64_t v = ((int64_t*)vec)[i];
        if (v > best) best = v;
    }
    return best;
}
int64_t py_builtin_min(int64_t vec) {
    int64_t n = vec ? ((int64_t*)(vec - 16))[1] : 0;
    if (n <= 0) return 0;
    int64_t best = ((int64_t*)vec)[0];
    for (int64_t i = 1; i < n; i++) {
        int64_t v = ((int64_t*)vec)[i];
        if (v < best) best = v;
    }
    return best;
}

// ── PY-A: s.split() with no separator — split on whitespace runs, dropping
// empty fields (Python semantics). The 2-arg form uses host_str_split. ──
int64_t host_str_split_ws(int64_t s) {
    const char* p = s ? (const char*)s : "";
    int64_t cap = 8, len = 0;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap;
    base[1] = 0;
    while (*p) {
        while (*p && isspace((unsigned char)*p)) p++;
        if (!*p) break;
        const char* start = p;
        while (*p && !isspace((unsigned char)*p)) p++;
        size_t n = (size_t)(p - start);
        char* tok = (char*)GC_malloc(n + 1);
        memcpy(tok, start, n);
        tok[n] = 0;
        if (len >= cap) {
            int64_t nc = cap * 2;
            int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)nc * 8);
            nb[0] = nc;
            nb[1] = len;
            for (int64_t i = 0; i < len; i++) nb[2 + i] = base[2 + i];
            base = nb;
            cap = nc;
        }
        base[2 + len++] = (int64_t)tok;
    }
    base[1] = len;
    return (int64_t)(base + 2);
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

// ── PY-A: closure free-variable environment ──────────────────────────
// A dedicated map whose keys are CONTENT-hashed string names. Slots are
// process-lifetime (GC-backed) — closures capture by reference to the slot,
// so reads see the latest writes from any scope.
int64_t map_new(void);
int64_t map_insert(int64_t, int64_t, int64_t);
int64_t map_get(int64_t, int64_t);
int64_t map_str_key(int64_t);

static int64_t g_env = 0;

static int64_t env_map(void) {
    if (!g_env) g_env = map_new();
    return g_env;
}
int64_t zeta_env_get(int64_t name_handle) {
    return map_get(env_map(), map_str_key(name_handle));
}
void zeta_env_set(int64_t name_handle, int64_t v) {
    map_insert(env_map(), map_str_key(name_handle), v);
}

// nonlocal declaration marker — no runtime effect (the env routing happens
// at the variable's read/write sites), but keeps the call linkable.
int64_t zeta_nonlocal_decl(int64_t name) { return name; }
// PY-A: module-global marker — no-op; the resolver/gen route reads/writes
// through the env instead. Present so the synthesized marker call links.
int64_t zeta_module_decl(int64_t name) { return name; }
// PY-A: Python-library import markers — no-ops. The Resolver collects them
// into the module/member alias tables; MirGen does the actual symbol mapping.
int64_t zeta_py_import(int64_t module, int64_t alias) { (void)module; (void)alias; return 0; }
int64_t zeta_py_from(int64_t module, int64_t member, int64_t alias) {
    (void)module; (void)member; (void)alias; return 0;
}

// PY-A: list comprehension collector — iter is a Vec-layout handle
// ([cap|len|data...]); fn_ptr is the address of a generated closure taking
// one i64 and returning i64 (-1 = skip). Returns a new Vec-layout handle.
int64_t zeta_collect_vec_n(int64_t iter, int64_t fn_ptr, int64_t len_override) {
    if (!iter) return 0;
    int64_t len = len_override;
    if (len < 0) len = ((int64_t*)(iter - 16))[1];
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(len ? len : 8) * 8);
    base[0] = len ? len : 8;
    base[1] = 0;
    int64_t (*fp)(int64_t) = (int64_t(*)(int64_t))fn_ptr;
    for (int64_t i = 0; i < len; i++) {
        int64_t v = fp(((int64_t*)iter)[i]);
        if (v != -1) {
            base[2 + base[1]] = v;
            base[1] += 1;
        }
    }
    return (int64_t)(base + 2);
}

// identity for chainable method fallbacks (returns the receiver handle)
int64_t zeta_identity1(int64_t h) { return h; }
int64_t zeta_identity2(int64_t h, int64_t a) { return h; }

// chainable identity — receiver handle passes through for unknown opaque
// method calls (pandas-style chaining fallback)
int64_t zeta_identity(int64_t h) { return h; }

// ── PY-A: JoinQuant platform API shims (REasyQuant strategies) ──────
// These platform functions have no local AOT meaning; they exist so strategy
// sources link and run as standalone binaries. Behavior: log-and-return-0,
// with option setters recording into a global dict-like handle.
int64_t set_option(int64_t a, int64_t b) { return 0; }
int64_t set_benchmark(int64_t a) { return 0; }
int64_t set_slippage(int64_t a) { return 0; }
int64_t set_order_cost(int64_t a, int64_t b) { return 0; }
int64_t set_universe(int64_t a) { return 0; }
int64_t set_limit_mode(int64_t a) { return 0; }
int64_t set_yesterday_position(int64_t a) { return 0; }
int64_t log_set_level(int64_t a, int64_t b) { return 0; }
int64_t run_daily(int64_t a, int64_t b) { return 0; }
int64_t run_monthly(int64_t a, int64_t b) { return 0; }
int64_t run_weekly(int64_t a, int64_t b) { return 0; }
int64_t run_interval(int64_t a, int64_t b) { return 0; }
int64_t get_current_data(int64_t a) { return 0; }
int64_t get_all_securities(int64_t a) { return 0; }
int64_t get_trade_days(int64_t a, int64_t b) { return 0; }
int64_t get_stock_list(int64_t a) { return 0; }
int64_t attribute_history(int64_t a, int64_t b, int64_t c, int64_t d) { return 0; }
int64_t get_price(int64_t a, int64_t b, int64_t c, int64_t d) { return 0; }
int64_t order_target_value(int64_t a, int64_t b) { return 0; }
int64_t order_target(int64_t a, int64_t b) { return 0; }
int64_t order_value(int64_t a, int64_t b) { return 0; }
int64_t order_shares(int64_t a, int64_t b) { return 0; }

// platform class constructor — opaque handle [class_name | args...]
int64_t zeta_platform_obj(int64_t name, int64_t a, int64_t b, int64_t c) {
    int64_t* h = (int64_t*)GC_malloc(32);
    h[0] = name;
    h[1] = a;
    h[2] = b;
    h[3] = c;
    return (int64_t)h;
}

// ── PY-A: Python builtins migration ─────────────────────────────────
// list(x) — Vec handle passthrough (arrays and Vecs are already handles)
int64_t zeta_list(int64_t x) { return x; }
// int(x) / float(x) — conversions across i64/f64/str
int64_t zeta_int_i64(int64_t v) { return v; }
int64_t zeta_int_f64(double v) { return (int64_t)v; }
int64_t zeta_int_str(int64_t s) {
    if (!s) return 0;
    return (int64_t)strtoll((const char*)s, NULL, 10);
}
double zeta_float_i64(int64_t v) { return (double)v; }
double zeta_float_f64(double v) { return v; }
double zeta_float_str(int64_t s) {
    if (!s) return 0;
    return strtod((const char*)s, NULL);
}
// round(x[, ndigits]) — Python banker's rounding approximation (half-away)
int64_t zeta_round_f64(double v, int64_t nd) {
    double m = 1;
    for (int64_t i = 0; i < nd; i++) m *= 10;
    return (int64_t)(v * m + (v >= 0 ? 0.5 : -0.5)) / (int64_t)m;
}
double zeta_floor_f64(double v) { return v >= 0 ? (double)(int64_t)v : (double)(int64_t)(v - 0.9999999999); }
// str.cast → StringLit handle (already Str)
// sorted(x) — Vec handle sort ascending (i64)
int64_t zeta_sorted_vec_len(int64_t data, int64_t len) {
    if (!data) return 0;
    if (len < 0) len = ((int64_t*)(data - 16))[1];
    if (len < 0) len = 0;
    // insertion sort in place on the copy — allocate new vec first
    int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)(len ? len : 8) * 8);
    nb[0] = len ? len : 8; nb[1] = len;
    for (int64_t i = 0; i < len; i++) nb[2 + i] = ((int64_t*)data)[i];
    for (int64_t i = 1; i < len; i++) {
        int64_t k = nb[2 + i];
        int64_t j = i - 1;
        while (j >= 0 && nb[2 + j] > k) { nb[2 + j + 1] = nb[2 + j]; j--; }
        nb[2 + j + 1] = k;
    }
    return (int64_t)(nb + 2);
}
// numpy subset: arange(n) / linspace(a, b, n)
int64_t zeta_arange(int64_t n) {
    int64_t cap = n < 8 ? 8 : n;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap; base[1] = n;
    for (int64_t i = 0; i < n; i++) base[2 + i] = i;
    return (int64_t)(base + 2);
}
int64_t zeta_linspace_i64(int64_t a, int64_t b, int64_t n) {
    int64_t cap = n < 8 ? 8 : n;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap; base[1] = n;
    for (int64_t i = 0; i < n; i++) base[2 + i] = a + (b - a) * i / (n > 1 ? n - 1 : 1);
    return (int64_t)(base + 2);
}
// diff(vec) → len-1 vec
int64_t zeta_diff_n(int64_t data, int64_t len) {
    if (!data) return 0;
    if (len < 0) len = ((int64_t*)(data - 16))[1];
    if (len < 2) { int64_t* e = (int64_t*)GC_malloc(16 + 8); e[0]=8; e[1]=0; return (int64_t)(e+2); }
    int64_t cap = len - 1 < 8 ? 8 : len - 1;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap; base[1] = len - 1;
    for (int64_t i = 0; i < len - 1; i++)
        base[2 + i] = ((int64_t*)data)[i + 1] - ((int64_t*)data)[i];
    return (int64_t)(base + 2);
}
// log.set_level(level, name) / logger.debug/info — no-op logging shims
int64_t zeta_log_noop2(int64_t a, int64_t b) { return 0; }
int64_t zeta_log_noop1(int64_t a) { return 0; }

// __collect_literals__(e1..en, COUNT, lambda) — variadic map over literal
// elements. Returns Vec handle of mapped results. NOTE: varargs after two
// fixed params; the generated call must pass (e1..en, count, fn_ptr) —
// actually simpler ABI: (count, fn_ptr, e1..en).
int64_t zeta_collect_literals(int64_t count, int64_t fn_ptr, ...) {
    int64_t (*fp)(int64_t) = (int64_t(*)(int64_t))fn_ptr;
    int64_t cap = count < 8 ? 8 : count;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap; base[1] = 0;
    va_list ap;
    va_start(ap, fn_ptr);
    for (int64_t i = 0; i < count; i++) {
        int64_t v = va_arg(ap, int64_t);
        int64_t mapped = fp(v);
        if (mapped != -1) {
            base[2 + base[1]] = mapped;
            base[1] += 1;
        }
    }
    va_end(ap);
    if (getenv("ZETA_PROBE")) {
        fprintf(stderr, "PROBE collect_literals count=%lld elems:", (long long)count);
        for (int64_t i = 0; i < base[1]; i++)
            fprintf(stderr, " %lld", (long long)base[2 + i]);
        fprintf(stderr, "\n");
    }
    return (int64_t)(base + 2);
}

// __collect_dict__(iter, fn_ptr) — fn returns packed (k<<32)|v pairs; the
// dict is a fresh platform map handle. V1: k/v both i64.
int64_t zeta_collect_dict(int64_t iter, int64_t fn_ptr) {
    if (!iter) return 0;
    int64_t len = ((int64_t*)(iter - 16))[1];
    int64_t m = map_new();
    int64_t (*fp)(int64_t) = (int64_t(*)(int64_t))fn_ptr;
    for (int64_t i = 0; i < len; i++) {
        int64_t pair = fp(((int64_t*)iter)[i]);
        map_insert(m, pair >> 32, pair & 0xFFFFFFFF);
    }
    return m;
}
// __pack_pair__(k, v) — pack two i64 into one i64 (V1: k high, v low)
int64_t zeta_pack_pair(int64_t k, int64_t v) {
    return (k << 32) | (v & 0xFFFFFFFF);
}

// ── PY-A: Python list methods ────────────────────────────────────────
// index/count/insert/remove/pop/sort/reverse. Vec layout: the handle points
// at the data, header [cap|len] at handle-16 (vec_push/vec_len layout).
// In-place ops mutate the handle's data; `insert` grows and returns a
// possibly-new handle, which the caller rebinds to the receiver variable.
int64_t vec_push(int64_t data_ptr, int64_t val);

static double zt_bits_f64(int64_t b) {
    double d;
    memcpy(&d, &b, 8);
    return d;
}

int64_t zeta_list_index(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 0; i < n; i++)
        if (((int64_t*)vec)[i] == v) return i;
    return -1;
}
int64_t zeta_list_count(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec), c = 0;
    for (int64_t i = 0; i < n; i++)
        if (((int64_t*)vec)[i] == v) c++;
    return c;
}
int64_t zeta_list_index_str(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 0; i < n; i++)
        if (str_eq(((int64_t*)vec)[i], v)) return i;
    return -1;
}
int64_t zeta_list_count_str(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec), c = 0;
    for (int64_t i = 0; i < n; i++)
        if (str_eq(((int64_t*)vec)[i], v)) c++;
    return c;
}
int64_t zeta_list_index_f64(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec);
    double needle = zt_bits_f64(v);
    for (int64_t i = 0; i < n; i++)
        if (zt_bits_f64(((int64_t*)vec)[i]) == needle) return i;
    return -1;
}
int64_t zeta_list_count_f64(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec), c = 0;
    double needle = zt_bits_f64(v);
    for (int64_t i = 0; i < n; i++)
        if (zt_bits_f64(((int64_t*)vec)[i]) == needle) c++;
    return c;
}

// `xs.insert(i, v)` — grow then shift right. Returns the (possibly new) handle.
int64_t zeta_list_insert(int64_t vec, int64_t idx, int64_t v) {
    int64_t n = zt_vec_len(vec);
    if (idx < 0) idx += n;
    if (idx < 0) idx = 0;
    if (idx > n) idx = n;
    int64_t nh = vec_push(vec, 0);
    for (int64_t i = n; i > idx; i--) ((int64_t*)nh)[i] = ((int64_t*)nh)[i - 1];
    ((int64_t*)nh)[idx] = v;
    return nh;
}
static void zt_list_shift_out(int64_t vec, int64_t idx, int64_t n) {
    for (int64_t j = idx; j < n - 1; j++) ((int64_t*)vec)[j] = ((int64_t*)vec)[j + 1];
    ((int64_t*)(vec - 16))[1] = n - 1;
}
// Returns the (unchanged) handle: the statement form rebinds the receiver to
// the call result, so returning 0 here would clobber the list.
int64_t zeta_list_remove(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 0; i < n; i++)
        if (((int64_t*)vec)[i] == v) { zt_list_shift_out(vec, i, n); return vec; }
    return vec;
}
int64_t zeta_list_remove_str(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 0; i < n; i++)
        if (str_eq(((int64_t*)vec)[i], v)) { zt_list_shift_out(vec, i, n); return vec; }
    return vec;
}
int64_t zeta_list_remove_f64(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec);
    double needle = zt_bits_f64(v);
    for (int64_t i = 0; i < n; i++)
        if (zt_bits_f64(((int64_t*)vec)[i]) == needle) { zt_list_shift_out(vec, i, n); return vec; }
    return vec;
}
int64_t zeta_list_pop(int64_t vec) {
    int64_t n = zt_vec_len(vec);
    if (n <= 0) return 0;
    int64_t v = ((int64_t*)vec)[n - 1];
    ((int64_t*)(vec - 16))[1] = n - 1;
    return v;
}
int64_t zeta_list_pop_at(int64_t vec, int64_t idx) {
    int64_t n = zt_vec_len(vec);
    if (idx < 0) idx += n;
    if (idx < 0 || idx >= n) return 0;
    int64_t v = ((int64_t*)vec)[idx];
    zt_list_shift_out(vec, idx, n);
    return v;
}
int64_t zeta_list_sort(int64_t vec) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 1; i < n; i++) {
        int64_t k = ((int64_t*)vec)[i], j = i - 1;
        while (j >= 0 && ((int64_t*)vec)[j] > k) { ((int64_t*)vec)[j + 1] = ((int64_t*)vec)[j]; j--; }
        ((int64_t*)vec)[j + 1] = k;
    }
    return vec;
}
int64_t zeta_list_sort_str(int64_t vec) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 1; i < n; i++) {
        int64_t k = ((int64_t*)vec)[i], j = i - 1;
        while (j >= 0 && strcmp((char*)((int64_t*)vec)[j], (char*)k) > 0) {
            ((int64_t*)vec)[j + 1] = ((int64_t*)vec)[j];
            j--;
        }
        ((int64_t*)vec)[j + 1] = k;
    }
    return vec;
}
int64_t zeta_list_sort_f64(int64_t vec) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 1; i < n; i++) {
        int64_t kb = ((int64_t*)vec)[i];
        double k = zt_bits_f64(kb);
        int64_t j = i - 1;
        while (j >= 0 && zt_bits_f64(((int64_t*)vec)[j]) > k) {
            ((int64_t*)vec)[j + 1] = ((int64_t*)vec)[j];
            j--;
        }
        ((int64_t*)vec)[j + 1] = kb;
    }
    return vec;
}
int64_t zeta_list_reverse(int64_t vec) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 0, j = n - 1; i < j; i++, j--) {
        int64_t t = ((int64_t*)vec)[i];
        ((int64_t*)vec)[i] = ((int64_t*)vec)[j];
        ((int64_t*)vec)[j] = t;
    }
    return vec;
}

// ── PY-A: dict update / pop / clear ──────────────────────────────────
// d.update(other) — copy every entry of `other`, overwriting on collision.
// Keys are already content-hashed in the stored maps, so copy them verbatim.
int64_t zeta_map_update(int64_t m, int64_t other) {
    if (!m || !other) return m;
    int64_t cap = ((int64_t*)other)[0];
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)other + 16 + i * MAP_ENTRY_SIZE;
        if (*(uint8_t*)(e + 16)) map_insert(m, *(int64_t*)e, *((int64_t*)e + 1));
    }
    return m;
}
// d.pop(k[, default]) — the open-addressing table has no tombstones, so
// removal rebuilds in place (collect survivors, clear, re-insert). O(n),
// fine for the dict sizes Python code uses here.
int64_t zeta_map_pop_default(int64_t m, int64_t key, int64_t def) {
    if (!m) return def;
    int64_t cap = ((int64_t*)m)[0];
    if (cap < 0) cap = 0;
    int64_t* ks = (int64_t*)GC_malloc((size_t)(cap ? cap : 1) * 8);
    int64_t* vs = (int64_t*)GC_malloc((size_t)(cap ? cap : 1) * 8);
    int64_t n = 0, found = 0, out = def;
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)m + 16 + i * MAP_ENTRY_SIZE;
        if (!*(uint8_t*)(e + 16)) continue;
        int64_t k = *(int64_t*)e, v = *((int64_t*)e + 1);
        if (k == key) { out = v; found = 1; continue; }
        ks[n] = k; vs[n] = v; n++;
    }
    if (!found) return def;
    for (int64_t i = 0; i < cap; i++)
        *(uint8_t*)((char*)m + 16 + i * MAP_ENTRY_SIZE + 16) = 0;
    ((int64_t*)m)[1] = 0;
    for (int64_t i = 0; i < n; i++) map_insert(m, ks[i], vs[i]);
    return out;
}
int64_t zeta_map_pop(int64_t m, int64_t key) { return zeta_map_pop_default(m, key, 0); }
int64_t zeta_map_clear(int64_t m) {
    if (!m) return m;
    int64_t cap = ((int64_t*)m)[0];
    if (cap < 0) cap = 0;
    for (int64_t i = 0; i < cap; i++)
        *(uint8_t*)((char*)m + 16 + i * MAP_ENTRY_SIZE + 16) = 0;
    ((int64_t*)m)[1] = 0;
    return m;
}
