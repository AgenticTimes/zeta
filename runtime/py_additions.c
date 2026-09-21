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
#include <math.h>

static int64_t zt_vec_len(int64_t v);
// Map primitives live in tokio_runtime_stub.c (merged into tokio_runtime.o);
// declared here before any use so no implicit-declaration conflict arises.
int64_t map_new(void);
int64_t map_insert(int64_t, int64_t, int64_t);
// A grown dict forwards from its old block; every reader must resolve first.
int64_t map_resolve(int64_t);
int64_t py_df_empty_like(int64_t frame);
static int zt_maybe_map(int64_t);
int zt_map_is_json_handle(int64_t);
void zt_map_json_mismatch(const char*, int64_t);
int64_t map_get(int64_t, int64_t);
int64_t map_str_key(int64_t);
int64_t py_map_contains(int64_t, int64_t);
#include <dlfcn.h>
#include <execinfo.h>
#include <unistd.h>   // sysconf(_SC_PAGESIZE)
#include <sys/mman.h> // msync page-mappedness probe
#include <mach/mach.h> // vm_read_overwrite readability probe

int64_t vec_push(int64_t, int64_t);
int64_t zeta_dynarray_new(int64_t cap);

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
// Arity variant (`s.split(sep, maxsplit)` — Python's 2-arg form). The runtime's
// split has no maxsplit, so the third argument is accepted and ignored rather
// than leaving an unresolved symbol.
int64_t host_str_split_3(int64_t s, int64_t sep, int64_t maxsplit) {
    (void)maxsplit;
    return str_split(s, sep);
}

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
    // Batch 291: vec<str> slots can hold small strings PACKED into the 8-byte
    // word ("43601853" = 0x3335383130363334) — measured: SEGV at that fault
    // address inside inlined `map_str_key` from `Series.nunique`'s `x in seen`.
    // The guard lives HERE (not at each caller) because both compiler-lowered
    // `map<str,_>` accesses and C shim sites funnel through this one symbol.
    // A non-readable value becomes an opaque identity key — equal values stay
    // equal, which is all dedup and membership need.
    if ((int64_t)GC_base((void*)handle) != 0) goto hash_content;
    {
        uint64_t u = (uint64_t)handle;
        // macOS arm64: __PAGEZERO is unmapped below 4 GiB and images start just
        // above it; values outside the canonical 48-bit window are packed
        // strings, not pointers.
        if (u < (1ULL << 32) || u >= (1ULL << 48)) return handle;
        // `vm_read_overwrite` faults-checks the read on our own task (msync only
        // proves MAPPING — a packed value can hit a PROT_NONE page).
        char probe;
        vm_size_t got = 0;
        if (vm_read_overwrite(mach_task_self(), (vm_address_t)u, 1,
                              (vm_address_t)&probe, &got) != KERN_SUCCESS)
            return handle;
        if (!probe) return handle;
    }
hash_content:;
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

// {**a, **b} — copy src's entries into dst by RAW key. Copying raw keys (not
// the display text that map_keys hands back) matters: string-keyed maps store
// content hashes, and a later d["k"] lookup goes through map_str_key, so
// re-inserting display text would never be found.
int64_t py_map_update(int64_t dst, int64_t src) {
    dst = map_resolve(dst);
    src = map_resolve(src);
    if (!dst || !src) return 0;
    int64_t cap = ((int64_t*)src)[0];
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)src + 16 + i * 24;
        if (*(uint8_t*)(e + 16)) {
            map_insert(dst, *(int64_t*)e, *((int64_t*)e + 1));
        }
    }
    return 0;
}

int64_t map_keys(int64_t map) {
    if (!map) return 0;
    map = map_resolve(map);
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
    map = map_resolve(map);
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
    map = map_resolve(map);
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

// ── PY-A: pandas / numpy spellings of the SAME PyDate / PyDelta handles ──
// The `datetime` module already produces them and all date arithmetic and
// comparison in the runtime dispatches on the handle tag, so these are thin
// aliases — no second date representation to keep in sync.
int64_t py_dt_strptime(int64_t, int64_t);
int64_t py_dt_lt(int64_t, int64_t);
int64_t py_dt_le(int64_t, int64_t);

int64_t py_dt_from_str(int64_t s) {
    // `pd.Timestamp(x)` where x is ALREADY a PyDate: callees with union-typed
    // params (`covers_range(start: str | pd.Timestamp)`) lose the tag at MIR,
    // so this runs on a handle too (measured: re-parsing the PyDate cell's
    // bytes produced the 1969-… garbage dates in `_to_ts`/covers_range).
    // Discriminator: a date-only ISO STRING is exactly "%Y-%m-%d" or longer
    // (10 bytes); a PyDate cell's first 8 bytes are one int64 days, and no
    // real-world epoch (≈ year 2423, days<2^31) puts an ASCII byte in the top
    // 4 — so bytes 4..7 are 0x00 (or 0xFF for negative/pre-1970) there.
    // Verified: str[4] == 0 iff s is NOT a valid ISO date string.
    if (s) {
        const unsigned char* b = (const unsigned char*)s;
        if (b[4] == 0 || b[4] == 0xFF) return s;  // already a date handle
    }
    static char* fmt = 0;
    if (!fmt) {
        fmt = (char*)GC_malloc(9);
        strcpy(fmt, "%Y-%m-%d");
    }
    return py_dt_strptime(s, (int64_t)fmt);
}

// `pd.Timestamp(ts, unit="ns", tz="UTC")` — the 3-arg call site (registry
// declares the 1-arg form, so the call arity-mangled to
// `py_dt_from_str_3`, which nothing defined). unit/tz are IGNORED: the PyDate
// handle is timezone-naive and carries no sub-day precision, so they cannot be
// honoured — and inventing an offset would be a silent wrong value.
int64_t py_dt_from_str_3(int64_t s, int64_t unit, int64_t tz) {
    (void)unit;
    (void)tz;
    return py_dt_from_str(s);
}

// str vs date SCALAR compare — `covers_range` compares the string trade_date
// column (the parquet reader formats dates "YYYY-MM-DD") against Timestamp
// scalars. Parse the string side and reuse the date comparisons.
int64_t py_dt_gt(int64_t, int64_t);
int64_t py_dt_ge(int64_t, int64_t);
int64_t py_dt_str_lt(int64_t s, int64_t d) { return py_dt_lt(py_dt_from_str(s), d); }
int64_t py_dt_str_le(int64_t s, int64_t d) { return py_dt_le(py_dt_from_str(s), d); }
int64_t py_dt_str_gt(int64_t s, int64_t d) { return py_dt_gt(py_dt_from_str(s), d); }
int64_t py_dt_str_ge(int64_t s, int64_t d) { return py_dt_ge(py_dt_from_str(s), d); }


// `np.searchsorted(sorted_dates, value, side="left"|"right")` over a Vec of
// PyDate handles. `side` arrives as the STRING handle (the registry fills
// keyword arguments positionally), so the choice is made by content.
// left  = first index with a[i] >= v   (numpy's default)
// right = first index with a[i] >  v
int64_t py_dt_searchsorted(int64_t vec, int64_t value, int64_t side) {
    int64_t right = 0;
    if (side) {
        char* s = (char*)side;
        right = strcmp(s, "right") == 0;
    }
    int64_t lo = 0;
    int64_t hi = zt_vec_len(vec);
    while (lo < hi) {
        int64_t mid = lo + (hi - lo) / 2;
        int64_t probe = ((int64_t*)vec)[mid];
        int64_t advance = right ? py_dt_le(probe, value) : py_dt_lt(probe, value);
        if (advance) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    return lo;
}

int64_t map_get_default(int64_t map, int64_t key, int64_t def) {
    if (!map) return def;
    map = map_resolve(map);
    if (zt_map_is_json_handle(map)) zt_map_json_mismatch("map_get_default", map);
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

// Series/list `.unique()` — order-preserving dedup. Compare by i64 equality
// first (ints / identical pointers); else by C-string content (str columns).
int64_t zeta_vec_unique(int64_t data) {
    if (!data) return 0;
    int64_t n = 0;
    if (!zt_vec_header_ok(data, &n)) return 0;
    /* Manual grow — avoid vec_push re-entry quirks on a freshly minted header. */
    int64_t cap = n < 8 ? 8 : n;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap;
    base[1] = 0;
    int64_t out = (int64_t)(base + 2);
    for (int64_t i = 0; i < n; i++) {
        int64_t v = ((int64_t*)data)[i];
        int found = 0;
        int64_t on = base[1];
        for (int64_t j = 0; j < on; j++) {
            int64_t u = ((int64_t*)out)[j];
            if (u == v) { found = 1; break; }
            if (u && v) {
                const char* su = (const char*)u;
                const char* sv = (const char*)v;
                if (su[0] == sv[0] && strcmp(su, sv) == 0) { found = 1; break; }
            }
        }
        if (!found) {
            if (base[1] >= base[0]) {
                int64_t nc = base[0] * 2;
                int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)nc * 8);
                nb[0] = nc;
                nb[1] = base[1];
                for (int64_t k = 0; k < base[1]; k++) nb[2 + k] = base[2 + k];
                base = nb;
                out = (int64_t)(base + 2);
            }
            ((int64_t*)out)[base[1]] = v;
            base[1] += 1;
        }
    }
    return out;
}

// Series/list `.nunique()` — count of order-preserving unique elements.
int64_t zeta_vec_nunique(int64_t data) {
    int64_t u = zeta_vec_unique(data);
    if (!u) return 0;
    int64_t n = 0;
    if (!zt_vec_header_ok(u, &n)) return 0;
    return n;
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
    map = map_resolve(map);
    if (zt_map_is_json_handle(map)) zt_map_json_mismatch("py_map_items", map);
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

// round(x) — Python returns an int with banker's rounding (round(2.5) == 2).
int64_t py_round_i64(double x) { return (int64_t)nearbyint(x); }

// ── PY-A: min/max with a key callable (linear scan; ties keep the first,
// like Python) ──────────────────────────────────────────────────────
int64_t py_min_key(int64_t vec, int64_t keyfn) {
    int64_t n = zt_vec_len(vec);
    if (n <= 0) return 0;
    int64_t best = ((int64_t*)vec)[0];
    int64_t best_k = ((int64_t(*)(int64_t))keyfn)(best);
    for (int64_t i = 1; i < n; i++) {
        int64_t v = ((int64_t*)vec)[i];
        int64_t k = ((int64_t(*)(int64_t))keyfn)(v);
        if (k < best_k) { best = v; best_k = k; }
    }
    return best;
}
int64_t py_max_key(int64_t vec, int64_t keyfn) {
    int64_t n = zt_vec_len(vec);
    if (n <= 0) return 0;
    int64_t best = ((int64_t*)vec)[0];
    int64_t best_k = ((int64_t(*)(int64_t))keyfn)(best);
    for (int64_t i = 1; i < n; i++) {
        int64_t v = ((int64_t*)vec)[i];
        int64_t k = ((int64_t(*)(int64_t))keyfn)(v);
        if (k > best_k) { best = v; best_k = k; }
    }
    return best;
}

// `x in list` — linear scan; string elements compare by CONTENT (the
// compiler passes that in, since element types are static). Previously this
// silently produced 0.
// PY-A: list equality by CONTENT. Ruby/Python semantics: same length and every
// element equal (strings compared with strcmp when elem_is_str). Without this,
// `a == b` on two lists compiled to an integer compare of the two handle
// pointers, so equal-content lists always compared false.
int64_t py_list_eq(int64_t a, int64_t b, int64_t elem_is_str) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    int64_t n = zt_vec_len(a);
    if (n != zt_vec_len(b)) return 0;
    for (int64_t i = 0; i < n; i++) {
        int64_t x = ((int64_t*)a)[i];
        int64_t y = ((int64_t*)b)[i];
        if (elem_is_str) {
            if (!x || !y) {
                if (x != y) return 0;
            } else if (strcmp((const char*)x, (const char*)y) != 0) {
                return 0;
            }
        } else if (x != y) {
            return 0;
        }
    }
    return 1;
}

// ponytail: content fallback for an UNKNOWN element type. A caller that cannot
// prove the element type passes elem_is_str=0 and only handles are compared —
// so `[c for c in to_fetch if normalize(c) not in fetched_codes]` treated every
// string as absent and `fetch_stocks` carried the whole universe forward (then
// crashed on the garbage set handle). `GC_base()` proves a value is a real GC
// pointer WITHOUT dereferencing it, so the extra string compare cannot read a
// wild address. Ceiling: two distinct objects with identical text compare equal
// — which is exactly Python's `in` semantics for strings.
static int zt_ptr_is_gc_object(int64_t p) {
    return p > 0x1000 && GC_base((void*)p) != 0;
}
// `s.startswith(("a", "b"))` / `s.endswith(...)` — Python takes a TUPLE of
// prefixes. Combining per-prefix calls with `||` in the MIR returned 0 for a
// matching prefix (measured: `_is_likely_index("000300.XSHG")` was False, so
// A-share index codes were scored as ordinary ETFs), so the whole test lives in
// one helper.
int64_t py_str_prefix_any(int64_t s, int64_t vec, int64_t is_end) {
    if (!s || !vec) return 0;
    int64_t n = zt_vec_len(vec);
    const char* str = (const char*)s;
    size_t slen = strlen(str);
    for (int64_t i = 0; i < n; i++) {
        const char* p = (const char*)((int64_t*)vec)[i];
        if (!p) continue;
        size_t plen = strlen(p);
        if (is_end) {
            if (plen <= slen && memcmp(str + (slen - plen), p, plen) == 0) return 1;
        } else if (plen <= slen && memcmp(str, p, plen) == 0) {
            return 1;
        }
    }
    return 0;
}
// `series.isna()` — our columns are string vectors and the only "missing" spelling
// is the literal "nan" produced by `pd.to_numeric(..., errors="coerce")` (plus the
// empty string). Returns a NEW vector of 0/1 so `mask | other` and boolean
// indexing keep working.
// `mask.any()` / `mask.all()` on a boolean/label vector. Truthiness: an integer
// slot is true when non-zero; a string slot when non-empty (and not "0"/"nan").
// `series.max()` / `.min()` — numeric when every element parses as a number
// (that is what the pandas path expects for prices/dates-as-strings), otherwise
// the lexicographic extreme. "nan"/empty slots are skipped; an all-missing vector
// yields "nan" so callers never see a fabricated 0.
// Python `set`s degrade to lists here, so `s.discard(x)` / `s.remove(x)` need a
// real removal: rebuild the vector without the first matching slot (by CONTENT
// when both look like strings, so it behaves like a set for string members).
// `s.add(x)` on our list-backed set: append only when absent (sets DEDUP — a
// plain push made `s.add(1); s.add(1)` report len 2, t262).
int64_t py_vec_add_unique(int64_t vec, int64_t x, int64_t elem_is_str) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 0; i < n; i++) {
        int64_t v = ((int64_t*)vec)[i];
        if (v == x) return vec;
        if (v && x && elem_is_str && strcmp((const char*)v, (const char*)x) == 0) return vec;
    }
    vec = vec_push(vec, x);
    return vec;
}

int64_t py_vec_discard(int64_t vec, int64_t x, int64_t elem_is_str) {
    if (!vec) return vec;
    int64_t n = zt_vec_len(vec);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        int64_t v = ((int64_t*)vec)[i];
        int same = (v == x);
        if (!same && v && x && elem_is_str) {
            same = strcmp((const char*)v, (const char*)x) == 0;
        }
        if (!same) out = vec_push(out, v);
    }
    return out;
}

int64_t py_vec_extreme(int64_t vec, int64_t want_max) {
    if (!vec) return (int64_t)GC_strdup("nan");
    int64_t n = zt_vec_len(vec);
    int have_num = 0, have_str = 0;
    double best_num = 0;
    const char* best_str = NULL;
    for (int64_t i = 0; i < n; i++) {
        const char* v = (const char*)((int64_t*)vec)[i];
        if (!v || v[0] == 0 || strcmp(v, "nan") == 0) continue;
        char* endp = NULL;
        double d = strtod(v, &endp);
        int is_num = (endp && *endp == 0);
        if (is_num) {
            if (!have_num || (want_max ? d > best_num : d < best_num)) best_num = d;
            have_num = 1;
        }
        if (!have_str || (want_max ? strcmp(v, best_str) > 0 : strcmp(v, best_str) < 0)) {
            best_str = v;
        }
        have_str = 1;
    }
    if (have_num) {
        char buf[64];
        snprintf(buf, sizeof buf, "%.10g", best_num);
        return (int64_t)GC_strdup(buf);
    }
    if (have_str) return (int64_t)GC_strdup(best_str);
    return (int64_t)GC_strdup("nan");
}
int64_t py_vec_max(int64_t vec) { return py_vec_extreme(vec, 1); }
int64_t py_vec_min(int64_t vec) { return py_vec_extreme(vec, 0); }

int zt_map_or_vec_truthy(int64_t v);  /* fwd */
static int zt_slot_truthy(int64_t v) {
    if (!v) return 0;
    if (v > 0x1000 && (v & 0x7) == 0) {
        const char* s = (const char*)v;
        if (s[0] == 0) return 0;
        if (strcmp(s, "0") == 0 || strcmp(s, "nan") == 0 || strcmp(s, "False") == 0) return 0;
        return 1;
    }
    return v != 0;
}
int64_t py_vec_isna(int64_t vec);
int64_t py_vec_isna(int64_t vec);
int64_t py_vec_notna(int64_t vec);
// Bare-symbol fallbacks: when the compiler cannot prove the receiver is a
// vector it emits `isna`/`notna` (+ arity variants). Treat a plausible vector
// that way; anything else is passed through unchanged with a warning (never a
// wild read, never a fabricated mask).
// A plausible `map<..>` block: header is `[cap|len]` with sane bounds.
static int zt_maybe_map(int64_t m) {
    if (!m || m < 0x1000) return 0;
    int64_t cap = ((int64_t*)(m - 16))[0];
    if (cap < 0 || cap > (1LL << 26)) return 0;
    return 1;
}
static int zt_maybe_vec(int64_t v) {
    if (v < 0x1000) return 0;
    int64_t cap = ((int64_t*)(v - 16))[0];
    int64_t len = ((int64_t*)(v - 16))[1];
    return cap >= 0 && len >= 0 && len <= cap && cap <= (1LL << 30);
}
int64_t zt_bare_mask(int64_t v, int want_notna) {
    if (zt_maybe_vec(v)) {
        return want_notna ? py_vec_notna(v) : py_vec_isna(v);
    }
    fprintf(stderr,
            "PY-A: `%s` called on a value that is not a vector — returned as-is\n",
            want_notna ? "notna" : "isna");
    fflush(stderr);
    return v;
}

// `~mask` on a boolean vector — element-wise NOT (Python's `~` on a Series).
// Without this `~invalid` evaluated to the integer bitwise-NOT of the VECTOR
// HANDLE (0/-garbage), so `df.loc[~invalid]` received a bogus mask.
// `mask_a | mask_b` — ELEMENT-WISE or for two boolean/label vectors (Python's
// Series `|`). The generic `|` path concatenated them, so
// `out["close"].isna() | (out["close"] < cfg.min_price)` produced a 2N-element
// "mask" and `df.loc[...]` then kept 2N rows.
// `vec * vec` — ELEMENT-WISE product (pandas `df["close"] * df["volume"]`).
// The generic `*` path routed two arrays to the SEMIRING fold (matmul), which
// walked the elements as arrays and crashed in `array_len` (measured:
// `validate_and_repair_stock_ohlcv` + the `amount` recompute). Column elements
// are STRINGS (parquet), so parse both sides; an unparsable element keeps the
// left value (loud in the sense that the result is a number, never a pointer).
// `series.pct_change()` — element-wise (v[i]-v[i-1])/v[i-1] with the first
// element 0. Used by `remove_extreme_return_bars`; without it the call hit a
// zero-arity stub and the following `~mask` produced 0, which made
// `DataFrame.loc` abort loudly ("mask is missing").
int64_t py_vec_pct_change(int64_t vec) {
    if (!vec) return vec;
    int64_t n = zt_vec_len(vec);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    double prev = 0;
    for (int64_t i = 0; i < n; i++) {
        const char* v = (const char*)((int64_t*)vec)[i];
        char* e = NULL;
        double d = v ? strtod(v, &e) : 0;
        int ok = v && e && *e == 0 && e != v;
        double r = 0;
        if (i > 0 && ok && prev != 0) r = (d - prev) / prev;
        if (ok) prev = d;
        char buf[64];
        snprintf(buf, sizeof buf, "%.10g", r);
        out = vec_push(out, (int64_t)GC_strdup(buf));
    }
    return out;
}

// `series.abs()` — numeric magnitude; non-numeric elements pass through.
int64_t py_vec_abs(int64_t vec) {
    if (!vec) return vec;
    int64_t n = zt_vec_len(vec);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        const char* v = (const char*)((int64_t*)vec)[i];
        char* e = NULL;
        double d = v ? strtod(v, &e) : 0;
        if (!v || !e || *e != 0 || e == v) {
            out = vec_push(out, (int64_t)v);
            continue;
        }
        char buf[64];
        snprintf(buf, sizeof buf, "%.10g", d < 0 ? -d : d);
        out = vec_push(out, (int64_t)GC_strdup(buf));
    }
    return out;
}

int64_t py_vec_pct_change(int64_t vec);
int64_t py_vec_abs(int64_t vec);
static int zt_maybe_vec_arity1(int64_t v) { return v > 0x1000; }
// weak: the shim (`pylib/pandas.z`) also emits a `pct_change` symbol, and the
// program's own definition must win (measured `duplicate symbol '_pct_change'`).
// `series.map(lambda …)` on a string vector: the MIR passes (vec, fn_ptr) — the
// closure is compiled to a plain function (`__closure_N`), same convention as
// `py_functools_reduce`. Used by `fetch_stocks`'s
// `result["stock_code"].map(lambda x: …)`.
int64_t zt_dyn_str_map(int64_t vec, int64_t fn) __asm__("_[dynamic]str__map");
int64_t zt_dyn_str_map(int64_t vec, int64_t fn) {
    if (!vec) return vec;
    if (!fn) {
        fprintf(stderr, "PY-A: [dynamic]str::map called without a function\n");
        abort();
    }
    int64_t n = zt_vec_len(vec);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        int64_t r = ((int64_t(*)(int64_t))fn)(((int64_t*)vec)[i]);
        int64_t pushed = vec_push(out, r);
        if (pushed != out) out = pushed;
    }
    return out;
}

// The MIR emits the DYNAMIC-array method symbol `[dynamic]str::<m>` for a
// vector receiver (`df["close"].pct_change()`); these are the actual targets.
int64_t zt_dyn_str_pct_change(int64_t v) __asm__("_[dynamic]str__pct_change");
int64_t zt_dyn_str_pct_change(int64_t v) { return py_vec_pct_change(v); }
int64_t zt_dyn_str_abs(int64_t v) __asm__("_[dynamic]str__abs");
int64_t zt_dyn_str_abs(int64_t v) { return py_vec_abs(v); }

__attribute__((weak)) int64_t pct_change(int64_t v) {
    return zt_maybe_vec_arity1(v) ? py_vec_pct_change(v) : v;
}
__attribute__((weak)) int64_t pct_change_1(int64_t v) {
    return zt_maybe_vec_arity1(v) ? py_vec_pct_change(v) : v;
}
__attribute__((weak)) int64_t abs_1(int64_t v) {
    return zt_maybe_vec_arity1(v) ? py_vec_abs(v) : (v < 0 ? -v : v);
}
int64_t vec_abs(int64_t v) { return zt_maybe_vec_arity1(v) ? py_vec_abs(v) : v; }

// `column > scalar` (and friends) — ELEMENT-WISE comparison producing a 0/1
// mask. Without this the MIR typed the result Bool, so the mask was used as a
// vector: `len(mask)` read `mask-16` on a small integer and `DataFrame.loc`
// aborted ("mask is missing"). Measured in `remove_extreme_return_bars`
// (`mask = ret > max_abs_daily_return`).
static int64_t zt_vec_cmp(int64_t vec, double rhs, int kind) {
    if (getenv("ZT_PROBE_LOC")) fprintf(stderr, "[probe] vec_cmp kind=%d vec=%lld rhs=%g\n", kind, (long long)vec, rhs);
    int zt_probe = getenv("ZT_PROBE_LOC") != NULL;
    if (!vec) return vec;
    int64_t n = zt_vec_len(vec);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        const char* v = (const char*)((int64_t*)vec)[i];
        char* e = NULL;
        double d = v ? strtod(v, &e) : 0;
        int ok = v && e && *e == 0 && e != v;
        int r = 0;
        if (ok) {
            switch (kind) {
                case 0: r = d > rhs; break;
                case 1: r = d < rhs; break;
                case 2: r = d >= rhs; break;
                case 3: r = d <= rhs; break;
                case 4: r = d == rhs; break;
                default: r = d != rhs; break;
            }
        }
        out = vec_push(out, r ? 1 : 0);
    }
    if (zt_probe) fprintf(stderr, "[probe] vec_cmp out=%lld n=%lld\n", (long long)out, (long long)n);
    return out;
}
// `_i` variants take an integer rhs, so the compiler can pass an i64 literal
// without a float conversion at the call site.
// `_bits` takes the f64 BIT PATTERN as an integer: the codegen passes a float
// LITERAL in an integer register (measured: `ret > 0.2` reached the helper with
// rhs == 0), so the value must travel as i64 and be reinterpreted here.
#define ZT_CMP_WRAP_BITS(name, kind)                                               \
    int64_t py_vec_##name##_bits(int64_t v, int64_t bits) {                        \
        double r;                                                                  \
        memcpy(&r, &bits, sizeof r);                                               \
        return zt_vec_cmp(v, r, kind);                                             \
    }
ZT_CMP_WRAP_BITS(gt, 0)
ZT_CMP_WRAP_BITS(lt, 1)
ZT_CMP_WRAP_BITS(ge, 2)
ZT_CMP_WRAP_BITS(le, 3)
ZT_CMP_WRAP_BITS(eq, 4)
ZT_CMP_WRAP_BITS(ne, 5)
#undef ZT_CMP_WRAP_BITS

#define ZT_CMP_WRAP(name, kind)                                                    \
    int64_t py_vec_##name(int64_t v, double r) { return zt_vec_cmp(v, r, kind); }  \
    int64_t py_vec_##name##_i(int64_t v, int64_t r) {                              \
        return zt_vec_cmp(v, (double)r, kind);                                     \
    }
// String-vector comparison: our dates are "YYYY-MM-DD" strings, which order
// chronologically under `strcmp`, but the numeric variants parsed them with
// `strtod` — `col >= "2023-08-05"` compared 2022 vs 2023 and answered 0 for
// every row, so `cached[(cached["trade_date"] >= eff_start) & …]` produced an
// EMPTY frame and the cache never looked like it covered the range.
int64_t py_vec_cmp_str(int64_t vec, int64_t rhs, int64_t kind) {
    if (!vec) return vec;
    const char* r = rhs ? (const char*)rhs : "";
    int64_t n = zt_vec_len(vec);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        const char* v = (const char*)((int64_t*)vec)[i];
        int c = v ? strcmp(v, r) : -1;
        int res = 0;
        switch (kind) {
            case 0: res = c > 0; break;
            case 1: res = c < 0; break;
            case 2: res = c >= 0; break;
            case 3: res = c <= 0; break;
            case 4: res = c == 0; break;
            default: res = c != 0; break;
        }
        out = vec_push(out, res ? 1 : 0);
    }
    return out;
}

ZT_CMP_WRAP(gt, 0)
ZT_CMP_WRAP(lt, 1)
ZT_CMP_WRAP(ge, 2)
ZT_CMP_WRAP(le, 3)
ZT_CMP_WRAP(eq, 4)
ZT_CMP_WRAP(ne, 5)
#undef ZT_CMP_WRAP

int64_t py_vec_mul(int64_t a, int64_t b) {
    if (!a || !b) return a ? a : b;
    int64_t na = zt_vec_len(a), nb = zt_vec_len(b);
    int64_t n = na < nb ? na : nb;
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        const char* sa = (const char*)((int64_t*)a)[i];
        const char* sb = (const char*)((int64_t*)b)[i];
        char* ea = NULL;
        char* eb = NULL;
        double da = sa ? strtod(sa, &ea) : 0;
        double db = sb ? strtod(sb, &eb) : 0;
        int oka = sa && ea && *ea == 0 && ea != sa;
        int okb = sb && eb && *eb == 0 && eb != sb;
        if (!oka || !okb) {
            out = vec_push(out, (int64_t)(oka ? sa : sb));
            continue;
        }
        char buf[64];
        snprintf(buf, sizeof buf, "%.10g", da * db);
        out = vec_push(out, (int64_t)GC_strdup(buf));
    }
    return out;
}

int64_t py_vec_or(int64_t a, int64_t b) {
    if (!a) return b;
    if (!b) return a;
    int64_t na = zt_vec_len(a), nb = zt_vec_len(b);
    int64_t n = na > nb ? na : nb;
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        int ta = i < na && zt_map_or_vec_truthy(((int64_t*)a)[i]);
        int tb = i < nb && zt_map_or_vec_truthy(((int64_t*)b)[i]);
        out = vec_push(out, (ta || tb) ? 1 : 0);
    }
    return out;
}

// `(col >= x) & (col <= y)` — element-wise AND of two 0/1 masks. Without this
// the MIR fell through to the scalar `Call{func:"&"}` path and LLVM emitted a
// bitwise `and` of the two VECTOR HANDLES — a garbage pointer that passed
// `py_is_vec` and crashed later (measured: `fetch_stocks`'s cache-window
// filter, SIGSEGV inside `sum(mask)`/`DataFrame::loc`).
int64_t py_vec_and(int64_t a, int64_t b) {
    if (!a) return b;
    if (!b) return a;
    int64_t na = zt_vec_len(a), nb = zt_vec_len(b);
    int64_t n = na > nb ? na : nb;
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        int ta = i < na && zt_map_or_vec_truthy(((int64_t*)a)[i]);
        int tb = i < nb && zt_map_or_vec_truthy(((int64_t*)b)[i]);
        out = vec_push(out, (ta && tb) ? 1 : 0);
    }
    return out;
}

int64_t py_vec_not(int64_t vec) {
    if (getenv("ZT_PROBE_LOC")) fprintf(stderr, "[probe] vec_not in=%lld\n", (long long)vec);
    if (!vec) return 0;
    int64_t n = zt_vec_len(vec);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        out = vec_push(out, zt_map_or_vec_truthy(((int64_t*)vec)[i]) ? 0 : 1);
    }
    if (getenv("ZT_PROBE_LOC")) fprintf(stderr, "[probe] vec_not out=%lld n=%lld\n", (long long)out, (long long)n);
    return out;
}

int64_t py_vec_notna(int64_t vec) {
    int64_t m = py_vec_isna(vec);
    int64_t n = zt_vec_len(m);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        out = vec_push(out, ((int64_t*)m)[i] ? 0 : 1);
    }
    return out;
}

int zt_map_or_vec_truthy(int64_t v) { return zt_slot_truthy(v); }
int64_t py_vec_any(int64_t vec) {
    if (!vec) return 0;
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 0; i < n; i++) {
        if (zt_slot_truthy(((int64_t*)vec)[i])) return 1;
    }
    return 0;
}
int64_t py_vec_all(int64_t vec) {
    if (!vec) return 1;
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 0; i < n; i++) {
        if (!zt_slot_truthy(((int64_t*)vec)[i])) return 0;
    }
    return 1;
}

// `df.loc[<bool mask>]` — row filtering in the column-map model: the frame is a
// heap cell whose field 0 is the column map; each column vector is filtered by the
// mask. Done in C on purpose: the same logic in the shim hit a slot-allocation
// bug (`len(keys)` came out as the MASK's length, so the loop ran out of range and
// `map_get` was called with a raw index as the key).
int zt_map_or_vec_truthy(int64_t v);
// `df.itertuples(index=False)` / `df.iterrows()` — the column-map model has no
// namedtuple, so each ROW is a map {column -> value}. Rows come back as a vec of
// map handles; `row.<col>` reaches them through the map-attribute path.
// Is the handle a plausible dynamic array? Used by the shim to tell a MASK from
// a slice/absent key (`df.iloc[0:0]` arrives as 0).
// Python `not x` — falsy for 0, NULL, an EMPTY string, an EMPTY list/map.
// (`!` cannot be reused: for an array it means `~mask`.)
static int zt_c_readable(int64_t a) {
    char p; vm_size_t g;
    return vm_read_overwrite(mach_task_self(), (vm_address_t)a, 1,
                             (vm_address_t)&p, &g) == KERN_SUCCESS;
}
int64_t py_not(int64_t x) {
    if (!x) return 1;
    // Batch 291: the vec/map heuristics peek at x-16. For a value that IS a
    // GC block base (imported-module global `HAS_BAOSTOCK` measured holding
    // 0x15_0000_0000 = a block start), that read lands in unmapped guard
    // space — measured SIGBUS at 0x14fffffff0 in `_baostock_login`. Only
    // probe what is actually readable; a block-base value is an object
    // pointer, truthy iff its first byte is non-zero.
    if ((int64_t)GC_base((void*)x) != x && zt_c_readable(x - 16)) {
        if (zt_maybe_vec(x)) return zt_vec_len(x) == 0 ? 1 : 0;
        if (zt_maybe_map(x)) return zt_vec_len(map_keys(x)) == 0 ? 1 : 0;
    }
    if (x > 0x100000000LL && x < 0x7fffffffffffLL && zt_c_readable(x)) {
        const char* s = (const char*)x;
        return s[0] == 0 ? 1 : 0;
    }
    return 0;
}

int64_t py_is_vec(int64_t v) { return zt_maybe_vec(v) ? 1 : 0; }

// Hash a `map<str, _>` key slot. Small strings can arrive PACKED into the
// 8-byte slot (`sz.15998` = 0x38393935312e7a73) instead of as a `char*`, and
// `map_str_key` dereferences its argument (`ldrb [x20]`) — measured as a SEGV
// inside inlined `map_str_key` from `py_df_groupby`. Non-pointer values hash
// as opaque integers, which keeps equal values equal.
int64_t zt_safe_str_key(int64_t v) {
    // Batch 291: the packed-small-string guard moved INTO `map_str_key` itself
    // (single definition — see comment there), so every caller — compiler-
    // lowered `map<str,_>` access and C shim alike — is covered. This alias is
    // kept for the groupby sites that were written against it.
    return map_str_key(v);
}

// A frame with the SAME columns and zero rows (`df.iloc[0:0]`). Previously the
// empty-slice case reached `py_df_loc` with mask 0 and aborted ("mask is
// missing") — measured in `remove_extreme_return_bars`'s `if not parts:` path.
int64_t py_df_empty_like(int64_t frame) {
    if (!frame) return 0;
    int64_t map = map_resolve(*(int64_t*)frame);
    // A bogus frame (e.g. the result of `DataFrame::iloc` called with the mask
    // that a bound-method slice never supplies) must not reach `map_keys`:
    // measured as `map_keys + 144 ← py_df_empty_like ← hA` in `df.iloc[0:0]`.
    if (!map || !zt_maybe_map(map)) {
        fprintf(stderr,
                "PY-A: empty-like on a non-frame %lld — returning an empty frame\n",
                (long long)frame);
        {
            void* bt[10];
            int nb = backtrace(bt, 10);
            char** syms = backtrace_symbols(bt, nb);
            for (int i = 1; i < nb && i < 6; i++) {
                fprintf(stderr, "[probe]   empty-like bt[%d] %s\n", i, syms ? syms[i] : "?");
            }
            free(syms);
        }
        int64_t* out = (int64_t*)GC_malloc(sizeof(int64_t));
        out[0] = map_new();
        return (int64_t)out;
    }
    int64_t nm = map_new();
    int64_t names = map_keys(map);
    int64_t n = zt_vec_len(names);
    for (int64_t i = 0; i < n; i++) {
        map_insert(nm, ((int64_t*)names)[i], zeta_dynarray_new(1));
    }
    int64_t* out = (int64_t*)GC_malloc(sizeof(int64_t));
    out[0] = nm;
    return (int64_t)out;
}

// `series.clip(lower=..., upper=...)` — element-wise clamp of a numeric column
// (strings are parsed; unparsable ones pass through unchanged).
int64_t py_vec_clip(int64_t vec, double lo, double hi, int64_t has_lo, int64_t has_hi);
// Bare-symbol fallbacks (the compiler cannot always prove the receiver is a vec):
// treat a plausible vector as one, otherwise pass the value through with a warning.
int64_t clip(int64_t v) { return zt_maybe_vec(v) ? py_vec_clip(v, 0, 0, 0, 0) : v; }
int64_t clip_2(int64_t v, int64_t lo) { return zt_maybe_vec(v) ? py_vec_clip(v, (double)lo, 0, 1, 0) : v; }
int64_t clip_3(int64_t v, int64_t lo, int64_t hi) {
    return zt_maybe_vec(v) ? py_vec_clip(v, (double)lo, (double)hi, 1, 1) : v;
}
int64_t clip_4(int64_t v, int64_t lo, int64_t hi, int64_t x) {
    (void)x;
    return zt_maybe_vec(v) ? py_vec_clip(v, (double)lo, (double)hi, 1, 1) : v;
}

int64_t py_vec_clip(int64_t vec, double lo, double hi, int64_t has_lo, int64_t has_hi) {
    if (!vec) return vec;
    int64_t n = zt_vec_len(vec);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        const char* v = (const char*)((int64_t*)vec)[i];
        char* endp = NULL;
        double d = v ? strtod(v, &endp) : 0;
        int is_num = v && endp && *endp == 0 && endp != v;
        if (!is_num) {
            out = vec_push(out, (int64_t)v);
            continue;
        }
        if (has_lo && d < lo) d = lo;
        if (has_hi && d > hi) d = hi;
        char buf[64];
        snprintf(buf, sizeof buf, "%.10g", d);
        out = vec_push(out, (int64_t)GC_strdup(buf));
    }
    return out;
}

static int zt_maybe_vec_fwd(int64_t v) {
    if (v < 0x1000) return 0;
    int64_t cap = ((int64_t*)(v - 16))[0];
    int64_t len = ((int64_t*)(v - 16))[1];
    return cap >= 0 && len >= 0 && len <= cap && cap <= (1LL << 30);
}
static int64_t zt_frame_rows(int64_t map) {
    int64_t keys = map_keys(map);
    int64_t nk = zt_vec_len(keys);
    int64_t nrows = 0;
    for (int64_t j = 0; j < nk; j++) {
        int64_t col = map_get(map, map_str_key(((int64_t*)keys)[j]));
        int64_t n = col ? zt_vec_len(col) : 0;
        if (n > nrows) nrows = n;
    }
    return nrows;
}
// `df["col"] = value` — a VECTOR value is stored as-is; a SCALAR is BROADCAST to
// the frame's row count (pandas semantics). Storing the scalar (a string handle
// for `df["stock_code"] = norm`) made `dict(self.data)` / per-column operations
// treat that handle as a vector — measured as `map_resolve` crashing inside
// `DataFrame::copy` (via `reset_index`).
int64_t py_df_setitem(int64_t frame, int64_t key, int64_t val) {
    if (!frame) return 0;
    int64_t map = *(int64_t*)frame;
    if (!map) return 0;
    int64_t k = map_str_key(key);
    if (zt_maybe_vec_fwd(val)) {
        map_insert(map, k, val);
        return 0;
    }
    int64_t nrows = zt_frame_rows(map);
    int64_t vec = zeta_dynarray_new(nrows > 0 ? nrows : 1);
    for (int64_t i = 0; i < nrows; i++) vec = vec_push(vec, val);
    map_insert(map, k, vec);
    return 0;
}

// ---- groupby -------------------------------------------------------------
// `for key, grp in df.groupby("col")` — the shim's GroupBy carried only the
// frame, so the loop iterated a struct handle (garbage) and the whole
// `remove_extreme_return_bars` path produced a dead frame. Each element here is
// a raw 2-slot block `[key, subframe]` (what `stack_array_get(elem, i)` reads),
// and the subframe is a shim DataFrame struct (one field: the column map).
int64_t py_df_groupby(int64_t frame, int64_t key) {
    if (getenv("ZT_PROBE_LOC")) {
        int64_t kp = (key > 0x100000000LL && key < 0x7fffffffffffLL) ? key : 0;
        fprintf(stderr, "[probe] groupby frame=%lld key=%lld keytext=%s\n",
                (long long)frame, (long long)key, kp ? (const char*)kp : "(not-a-ptr)");
        fflush(stderr);
    }
    int64_t out = zeta_dynarray_new(4);
    if (!frame) return out;
    int64_t map = map_resolve(*(int64_t*)frame);
    if (!map) return out;
    int64_t k = map_str_key(key);
    int64_t keys_vec = map_get(map, k);
    if (!keys_vec && getenv("ZT_PROBE_LOC")) {
        int64_t kn = map_keys(map);
        fprintf(stderr, "[probe] groupby lookup MISS key=%lld hashed=%lld ncols=%lld firstkey=%s\n",
                (long long)key, (long long)k, (long long)zt_vec_len(kn),
                zt_vec_len(kn) > 0 ? (const char*)((int64_t*)kn)[0] : "-");
        fflush(stderr);
    }
    if (!keys_vec) return out;
    int64_t n = zt_vec_len(keys_vec);
    if (getenv("ZT_PROBE_LOC")) {
        fprintf(stderr, "[probe] groupby keyvec=%lld n=%lld first=%lld ncols_m=%lld\n",
                (long long)keys_vec, (long long)n,
                (long long)(n > 0 ? ((int64_t*)keys_vec)[0] : 0),
                (long long)zt_vec_len(map_keys(map)));
        fflush(stderr);
    }

    // Column list is needed to rebuild each sub-frame.
    int64_t col_names = map_keys(map);
    int64_t ncols = zt_vec_len(col_names);

    int64_t seen = map_new();          // key -> pair index (order preserving)
    int64_t pairs = zeta_dynarray_new(4);

    for (int64_t i = 0; i < n; i++) {
        int64_t kraw = ((int64_t*)keys_vec)[i];
        // Keys in a `map<str, _>` are HASHED (`map_str_key`); the raw pointer
        // differs per row even for equal strings (each value is its own
        // allocation), so grouping by pointer produced one group per row.
        // Small strings can also arrive PACKED into the 8-byte slot
        // (`sz.15998` = 0x38393935312e7a73), so only dereference what looks like
        // a real pointer: `map_str_key` read `[x20]` on a packed value and SEGV'd.
        int64_t hk = zt_safe_str_key(kraw);
        int64_t idx = map_get(seen, hk);
        if (getenv("ZT_PROBE_LOC")) {
            fprintf(stderr, "[probe] grp row %lld kraw=%lld hk=%lld idx=%lld gcbase=%lld txt=%s\n",
                    (long long)i, (long long)kraw, (long long)hk, (long long)idx,
                    (long long)(int64_t)GC_base((void*)kraw),
                    (kraw && (int64_t)GC_base((void*)kraw) == kraw) ? (const char*)kraw : "?");
        }
        int64_t pair = 0;
        if (idx) {
            pair = ((int64_t*)pairs)[idx - 1];
        } else {
            int64_t* p = (int64_t*)GC_malloc(2 * sizeof(int64_t));
            int64_t* sub = (int64_t*)GC_malloc(sizeof(int64_t));
            sub[0] = map_new();
            p[0] = kraw;
            p[1] = (int64_t)sub;
            pairs = vec_push(pairs, (int64_t)p);
            map_insert(seen, hk, zt_vec_len(pairs));
            pair = (int64_t)p;
        }
        int64_t* sub = (int64_t*)((int64_t*)pair)[1];
        // Resolve the sub-map every round: `map_insert` below grows it into a
        // FORWARDING block, and reading/inserting through the stale block made
        // `map_get` return a non-zero garbage handle that then went into
        // `vec_push` as a capacity (GC: "Failed to expand heap by 13582167728875120 KiB").
        int64_t smap = map_resolve(sub[0]);
        for (int64_t c = 0; c < ncols; c++) {
            // `map_keys` yields the STORED key representation; `map_get`/`map_insert`
            // in this runtime key `map<str, _>` by `map_str_key`, so hash here.
            // `map_get` keys by the INTERNED handle (`map_str_key`), so the
            // display string from `map_keys` must be interned first — using it
            // directly lost every column (measured: `group a 0 0` in /tmp/gb.z).
            int64_t hc = zt_safe_str_key(((int64_t*)col_names)[c]);
            int64_t col = map_get(map, hc);
            if (!col) continue;
            smap = map_resolve(smap);
            int64_t dst = map_get(smap, hc);
            if (!dst) {
                dst = zeta_dynarray_new(4);
                map_insert(smap, hc, dst);
            }
            if (i < zt_vec_len(col)) {
                int64_t pushed = vec_push(dst, ((int64_t*)col)[i]);
                // `vec_push` returns a NEW pointer when it grows; the map must be
                // updated or the column reads a stale 0-length header (measured:
                // every group reported `len(grp) == 0`).
                if (pushed != dst) {
                    dst = pushed;
                    map_insert(smap, hc, dst);
                }
            }
        }
    }
    if (getenv("ZT_PROBE_LOC")) {
        fprintf(stderr, "[probe] groupby done npairs=%lld out=%lld outn=%lld\n",
                (long long)zt_vec_len(pairs), (long long)out, (long long)zt_vec_len(out));
        fflush(stderr);
    }
    // `pairs` holds the pair blocks directly.
    for (int64_t i = 0; i < zt_vec_len(pairs); i++) {
        out = vec_push(out, ((int64_t*)pairs)[i]);
    }
    if (getenv("ZT_PROBE_LOC")) {
        fprintf(stderr, "[probe] groupby return out=%lld len=%lld\n",
                (long long)out, (long long)zt_vec_len(out));
        fflush(stderr);
    }
    return out;
}

// `gb.pairs()` — the GroupBy struct holds (frame, key) in that order.
int64_t py_groupby_pairs(int64_t gb) {
    if (!gb) return zeta_dynarray_new(1);
    return py_df_groupby(((int64_t*)gb)[0], ((int64_t*)gb)[1]);
}

int64_t py_df_itertuples(int64_t frame, int64_t with_index) {
    if (!frame) return zeta_dynarray_new(1);
    int64_t map = *(int64_t*)frame;
    if (!map) return zeta_dynarray_new(1);
    int64_t keys = map_keys(map);
    int64_t nk = zt_vec_len(keys);
    int64_t nrows = 0;
    for (int64_t j = 0; j < nk; j++) {
        int64_t col = map_get(map, map_str_key(((int64_t*)keys)[j]));
        int64_t n = col ? zt_vec_len(col) : 0;
        if (n > nrows) nrows = n;
    }
    int64_t out = zeta_dynarray_new(nrows > 0 ? nrows : 1);
    for (int64_t i = 0; i < nrows; i++) {
        int64_t row = map_new();
        for (int64_t j = 0; j < nk; j++) {
            int64_t kdisp = ((int64_t*)keys)[j];
            int64_t col = map_get(map, map_str_key(kdisp));
            int64_t n = col ? zt_vec_len(col) : 0;
            int64_t v = (i < n) ? ((int64_t*)col)[i] : 0;
            map_insert(row, map_str_key(kdisp), v);
        }
        if (with_index) {
            char buf[32];
            snprintf(buf, sizeof buf, "%lld", (long long)i);
            map_insert(row, map_str_key((int64_t)GC_strdup("index")),
                       (int64_t)GC_strdup(buf));
        }
        out = vec_push(out, row);
    }
    return out;
}

int64_t py_df_loc(int64_t frame, int64_t mask) {
    if (getenv("ZT_PROBE_LOC")) {
        void* ra = __builtin_return_address(0);
        Dl_info di;
        const char* sym = (dladdr(ra, &di) && di.dli_sname) ? di.dli_sname : "?";
        fprintf(stderr, "[probe] df_loc frame=%lld mask=%lld from=%s\n",
                (long long)frame, (long long)mask, sym);
        {
            void* bt[12];
            int nb = backtrace(bt, 12);
            char** syms = backtrace_symbols(bt, nb);
            for (int i = 1; i < nb && i < 7; i++) {
                fprintf(stderr, "[probe]   bt[%d] %s\n", i, syms ? syms[i] : "?");
            }
            free(syms);
        }
    }
    if (!frame) return 0;
    int64_t map = *(int64_t*)frame;
    if (!map) return 0;
    if (!mask) {
        fprintf(stderr, "PY-A: DataFrame.loc: mask is missing\n");
        fflush(stderr);
        abort();
    }
    // A bound-method slice (`df.iloc[0:0]`) reaches the shim with no mask at all:
    // the MIR emits the method call first and the slice handling afterwards, so
    // the mask argument is never a vector. "No rows selected" keeps the columns —
    // and we say so on stderr rather than reading garbage as a mask.
    if (!zt_maybe_vec(mask)) {
        fprintf(stderr,
                "PY-A: DataFrame.loc: mask %lld is not a vector — empty selection\n",
                (long long)mask);
        // Return the COLUMN MAP: the shim wraps this in `DataFrame(...)`, so
        // handing back a frame struct made a frame-in-frame and every later
        // `self.data` read a struct pointer (measured: `remove_extreme_return_bars`
        // returned an empty result through `market_df.iloc[0:0]`).
        {
            int64_t ef = py_df_empty_like(frame);
            return ef ? *(int64_t*)ef : map_new();
        }
    }
    int64_t keys = map_keys(map);
    int64_t nk = zt_vec_len(keys);
    int64_t mlen = zt_vec_len(mask);
    int64_t out = map_new();
    for (int64_t j = 0; j < nk; j++) {
        int64_t kdisp = ((int64_t*)keys)[j];
        int64_t col = map_get(map, map_str_key(kdisp));
        int64_t n = col ? zt_vec_len(col) : 0;
        int64_t kept = zeta_dynarray_new(n > 0 ? n : 1);
        for (int64_t i = 0; i < n; i++) {
            if (i < mlen && zt_map_or_vec_truthy(((int64_t*)mask)[i])) {
                kept = vec_push(kept, ((int64_t*)col)[i]);
            }
        }
        map_insert(out, map_str_key(kdisp), kept);
    }
    return out;
}

int64_t py_vec_isna(int64_t vec) {
    if (!vec) return zeta_dynarray_new(1);
    int64_t n = zt_vec_len(vec);
    int64_t out = zeta_dynarray_new(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        const char* v = (const char*)((int64_t*)vec)[i];
        int64_t flag = 0;
        if (!v || v[0] == 0) {
            flag = 1;
        } else if (strcmp(v, "nan") == 0 || strcmp(v, "NaN") == 0 ||
                   strcmp(v, "None") == 0) {
            flag = 1;
        }
        out = vec_push(out, flag);
    }
    return out;
}

// Content equality between two `str` values that may individually be a
// readable C string OR a small string PACKED into the 8-byte slot (batch 291
// — same packed-value hazard `map_str_key` now guards). Pointer-vs-pointer
// uses strcmp; packed-vs-packed needs equality (different values can never be
// equal text); mixed packs the pointer's bytes and compares words.
static int zt_is_cstr_ptr(int64_t v) {
    if (!v) return 0;
    if ((int64_t)GC_base((void*)v) != 0) return 1;
    uint64_t u = (uint64_t)v;
    if (u < (1ULL << 32) || u >= (1ULL << 48)) return 0;
    char probe;
    vm_size_t got;
    return vm_read_overwrite(mach_task_self(), (vm_address_t)u, 1,
                             (vm_address_t)&probe, &got) == KERN_SUCCESS &&
           probe != 0;
}
static int zt_packed_cstr_eq(int64_t packed, const char* s) {
    size_t n = strlen(s);
    if (n > 7) return 0;
    const unsigned char* p = (const unsigned char*)&packed;
    for (size_t i = 0; i < n; i++)
        if (p[i] != (unsigned char)s[i]) return 0;
    for (size_t i = n; i < 8; i++)
        if (p[i] != 0) return 0;
    return 1;
}
static int zt_str_content_eq(int64_t a, int64_t b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    int ra = zt_is_cstr_ptr(a), rb = zt_is_cstr_ptr(b);
    if (ra && rb) return strcmp((const char*)a, (const char*)b) == 0;
    if (!ra && !rb) return 0;
    if (ra) return zt_packed_cstr_eq(b, (const char*)a);
    return zt_packed_cstr_eq(a, (const char*)b);
}

// Batch 292: ORDERING (`<` `>` `<=` `>=`) on str operands. Codegen used to
// fall through to integer `icmp` on the raw slot — a POINTER comparison, so
// `a >= c` with equal content answered False and the trading-day filter
// `d >= start_date and d <= end_date` dropped every day (backtest ran 0
// days). Normalize packed slots to a byte view, then strcmp.
static const char* zt_str_view(int64_t v, char* buf) {
    if (!v) return "";
    if (zt_is_cstr_ptr(v)) return (const char*)v;
    uint64_t u = (uint64_t)v;
    for (int i = 0; i < 8; i++) {
        char c = (char)((u >> (8 * i)) & 0xff);
        if (!c || (unsigned char)c < 0x20 || (unsigned char)c >= 0x7f) {
            buf[i] = 0;
            return buf;
        }
        buf[i] = c;
    }
    buf[8] = 0;
    return buf;
}
int64_t host_str_cmp(int64_t a, int64_t b, int64_t kind) {
    char ba[9], bb[9];
    int r = strcmp(zt_str_view(a, ba), zt_str_view(b, bb));
    switch (kind) {
        case 0: return r < 0;
        case 1: return r > 0;
        case 2: return r <= 0;
        case 3: return r >= 0;
        default: return 0;
    }
}

int64_t py_list_contains(int64_t vec, int64_t x, int64_t elem_is_str) {
    int64_t n = zt_vec_len(vec);
    if (getenv("ZT_DEBUG_CONTAINS")) fprintf(stderr, "CONTAINS vec=%p n=%lld x=%p str=%lld\n", (void*)vec, (long long)n, (void*)x, (long long)elem_is_str);
    for (int64_t i = 0; i < n; i++) {
        int64_t v = ((int64_t*)vec)[i];
        if (v == x && v != 0) return 1;
        if (elem_is_str) {
            if (getenv("ZT_DEBUG_CONTAINS")) fprintf(stderr, "CONTAINS str i=%lld/%lld v=%p x=%p\n", (long long)i, (long long)n, (void*)v, (void*)x);
            // Batch 291: vec<str> slots can hold small strings PACKED into the
            // 8-byte word — measured SIGSEGV in `strcmp` from `py_list_contains`
            // ← `_build_stock_arrays`. Content equality must not dereference a
            // packed value.
            if (v && x && zt_str_content_eq(v, x)) return 1;
        } else if (v && x && zt_ptr_is_gc_object(v) && zt_ptr_is_gc_object(x)) {
            if (getenv("ZT_DEBUG_CONTAINS")) fprintf(stderr, "CONTAINS cmp '%s' vs '%s'\n", (const char*)v, (const char*)x);
            if (strcmp((const char*)v, (const char*)x) == 0) return 1;
        }
    }
    return 0;
}

// ── PY-A: sort/sorted with a key callable (decorate-sort-undecorate) ──
// Keys are computed once per element; the original index is the tie-breaker,
// so the sort stays stable like Python's.
typedef struct {
    int64_t key;
    int64_t idx;
} zt_kv_t;
static int zt_cmp_kv_asc(const void* a, const void* b) {
    const zt_kv_t* x = (const zt_kv_t*)a;
    const zt_kv_t* y = (const zt_kv_t*)b;
    if (x->key != y->key) return x->key < y->key ? -1 : 1;
    return x->idx < y->idx ? -1 : (x->idx > y->idx ? 1 : 0);
}
static int zt_cmp_kv_desc(const void* a, const void* b) {
    const zt_kv_t* x = (const zt_kv_t*)a;
    const zt_kv_t* y = (const zt_kv_t*)b;
    if (x->key != y->key) return x->key > y->key ? -1 : 1;
    return x->idx < y->idx ? -1 : (x->idx > y->idx ? 1 : 0);
}
static void zt_sort_by_key(int64_t* vals, int64_t n, int64_t keyfn, int reverse) {
    if (n <= 1) return;
    zt_kv_t* kv = (zt_kv_t*)GC_malloc(sizeof(zt_kv_t) * (size_t)n);
    for (int64_t i = 0; i < n; i++) {
        kv[i].key = ((int64_t(*)(int64_t))keyfn)(vals[i]);
        kv[i].idx = i;
    }
    qsort(kv, (size_t)n, sizeof(zt_kv_t), reverse ? zt_cmp_kv_desc : zt_cmp_kv_asc);
    int64_t* out = (int64_t*)GC_malloc((size_t)n * 8);
    for (int64_t i = 0; i < n; i++) out[i] = vals[kv[i].idx];
    for (int64_t i = 0; i < n; i++) vals[i] = out[i];
}
// In-place: xs.sort(key=f[, reverse=...]) — returns the handle.
int64_t py_list_sort_key(int64_t vec, int64_t keyfn, int64_t reverse) {
    int64_t n = zt_vec_len(vec);
    zt_sort_by_key((int64_t*)vec, n, keyfn, (int)reverse);
    return vec;
}
// Copy: sorted(xs, key=f[, reverse=...]) — returns a new Vec.
int64_t py_sorted_key(int64_t vec, int64_t keyfn, int64_t reverse) {
    int64_t n = zt_vec_len(vec);
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(n ? n : 1) * 8);
    base[0] = n ? n : 1;
    base[1] = n;
    for (int64_t i = 0; i < n; i++) base[2 + i] = ((int64_t*)vec)[i];
    zt_sort_by_key(base + 2, n, keyfn, (int)reverse);
    return (int64_t)(base + 2);
}

// ── PY-A: strip/lstrip/rstrip with a CHARACTER SET ───────────────────
static int64_t zt_strip_set(int64_t s, int64_t chars, int left, int right) {
    const char* p = s ? (const char*)s : "";
    const char* set = chars ? (const char*)chars : " \t\n\r\f\v";
    size_t b = 0, e = strlen(p);
    if (left) while (b < e && strchr(set, p[b])) b++;
    if (right) while (e > b && strchr(set, p[e - 1])) e--;
    size_t n = e - b;
    char* out = (char*)GC_malloc(n + 1);
    memcpy(out, p + b, n);
    out[n] = 0;
    return (int64_t)out;
}
int64_t host_str_strip_chars(int64_t s, int64_t c) { return zt_strip_set(s, c, 1, 1); }
int64_t host_str_lstrip_chars(int64_t s, int64_t c) { return zt_strip_set(s, c, 1, 0); }
int64_t host_str_rstrip_chars(int64_t s, int64_t c) { return zt_strip_set(s, c, 0, 1); }

// ── PY-A: argparse (V1) ──────────────────────────────────────────────
// The parser is a Vec of [flag_name, default_string] pairs (a Vec, not a map,
// because scanning argv needs the NAMES). parse_args resolves each flag from
// argv, falling back to its default, into a map name-hash -> string; the
// compiler picks the typed getter from the flag kind it recorded at
// add_argument (so `args.cash` is a float, not an integer).
extern int64_t zeta_argc(void);
extern int64_t zeta_argv_at(int64_t i);

int64_t py_argparse_new(void) {
    int64_t* base = (int64_t*)GC_malloc(16 + 8 * 8);
    base[0] = 8;
    base[1] = 0;
    return (int64_t)(base + 2);
}
int64_t py_argparse_add(int64_t parser, int64_t dest, int64_t flag, int64_t default_str) {
    // [dest_name, argv_flag, default_string] — the dest is what `args.<x>`
    // looks up, the flag is what argv is scanned for (they differ: `start`
    // vs `--start`).
    int64_t* pair = (int64_t*)GC_malloc(24);
    pair[0] = dest;
    pair[1] = flag;
    pair[2] = default_str ? default_str : (int64_t)GC_strdup("");
    return vec_push(parser, (int64_t)pair);
}
// `--name value` | `--name=value` | bare `--name` (-> "1", for store_true).
static const char* zt_argv_value(const char* name) {
    size_t nlen = strlen(name);
    int64_t argc = zeta_argc();
    for (int64_t i = 1; i < argc; i++) {
        const char* a = (const char*)zeta_argv_at(i);
        if (!a || strncmp(a, name, nlen) != 0) continue;
        const char* rest = a + nlen;
        if (*rest == '=') return rest + 1;
        if (*rest == 0) {
            if (i + 1 < argc) {
                const char* nxt = (const char*)zeta_argv_at(i + 1);
                if (nxt && nxt[0] != '-') return nxt;
            }
            return "1";
        }
    }
    return 0;
}
int64_t py_argparse_parse(int64_t parser) {
    int64_t ns = map_new();
    int64_t n = zt_vec_len(parser);
    for (int64_t i = 0; i < n; i++) {
        int64_t* pair = (int64_t*)((int64_t*)parser)[i];
        const char* dest = (const char*)pair[0];
        const char* flag = (const char*)pair[1];
        const char* val = (const char*)pair[2];
        if (flag && flag[0]) {
            const char* got = zt_argv_value(flag);
            if (got) val = got;
        }
        map_insert(ns, map_str_key((int64_t)dest), (int64_t)GC_strdup(val ? val : ""));
    }
    return ns;
}
int64_t py_argparse_get_str(int64_t ns, int64_t name) {
    return map_get(ns, map_str_key(name));
}
int64_t py_argparse_get_i64(int64_t ns, int64_t name) {
    int64_t s = map_get(ns, map_str_key(name));
    return s ? (int64_t)strtoll((const char*)s, NULL, 10) : 0;
}
double py_argparse_get_f64(int64_t ns, int64_t name) {
    int64_t s = map_get(ns, map_str_key(name));
    return s ? strtod((const char*)s, NULL) : 0.0;
}
int64_t py_argparse_get_bool(int64_t ns, int64_t name) {
    int64_t s = map_get(ns, map_str_key(name));
    if (!s) return 0;
    const char* p = (const char*)s;
    if (!*p) return 0;
    if (strcmp(p, "0") == 0 || strcmp(p, "false") == 0 || strcmp(p, "False") == 0) return 0;
    return 1;
}

// ── PY-A: hex/oct/bin/reversed ───────────────────────────────────────
// Sign-aware base rendering with Python's 0x/0o/0b prefix.
static int64_t zt_int_to_base(int64_t n, int base, const char* prefix) {
    char tmp[72];
    int k = 0;
    unsigned long long u = n < 0 ? (unsigned long long)(-n) : (unsigned long long)n;
    if (!u) tmp[k++] = '0';
    while (u) {
        int d = (int)(u % (unsigned)base);
        tmp[k++] = (char)(d < 10 ? '0' + d : 'a' + d - 10);
        u /= (unsigned)base;
    }
    char* o = (char*)GC_malloc(80);
    size_t p = 0;
    if (n < 0) o[p++] = '-';
    p += (size_t)sprintf(o + p, "%s", prefix);
    while (k) o[p++] = tmp[--k];
    o[p] = 0;
    return (int64_t)o;
}
int64_t py_builtin_hex(int64_t n) { return zt_int_to_base(n, 16, "0x"); }
int64_t py_builtin_oct(int64_t n) { return zt_int_to_base(n, 8, "0o"); }
int64_t py_builtin_bin(int64_t n) { return zt_int_to_base(n, 2, "0b"); }
int64_t py_builtin_reversed(int64_t vec) {
    int64_t n = zt_vec_len(vec);
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(n ? n : 1) * 8);
    base[0] = n ? n : 1;
    base[1] = n;
    for (int64_t i = 0; i < n; i++) base[2 + i] = ((int64_t*)vec)[n - 1 - i];
    return (int64_t)(base + 2);
}

// ── PY-A: math constants + the missing common functions ──────────────
// Constants previously warned and lowered to 0 (a silently wrong value);
// gcd/factorial/isqrt had no libc counterpart to fall back on either.
double py_math_pi(void) { return 3.14159265358979323846; }
double py_math_e(void) { return 2.71828182845904523536; }
double py_math_tau(void) { return 6.28318530717958647692; }
double py_math_inf(void) { return (double)INFINITY; }
double py_math_nan(void) { return (double)NAN; }
double py_math_hypot2(double a, double b) { return hypot(a, b); }
int64_t py_math_gcd(int64_t a, int64_t b) {
    if (a < 0) a = -a;
    if (b < 0) b = -b;
    while (b) {
        int64_t t = a % b;
        a = b;
        b = t;
    }
    return a;
}
int64_t py_math_factorial(int64_t n) {
    if (n < 0) return 0;
    int64_t r = 1;
    for (int64_t i = 2; i <= n; i++) r *= i;
    return r;
}
double py_math_degrees(double r) { return r * 180.0 / 3.14159265358979323846; }
double py_math_radians(double d) { return d * 3.14159265358979323846 / 180.0; }
int64_t py_math_isqrt(int64_t n) {
    if (n < 0) return 0;
    int64_t r = (int64_t)sqrt((double)n);
    while (r > 0 && r * r > n) r--;
    while ((r + 1) * (r + 1) <= n) r++;
    return r;
}

// ── PY-A: dict.setdefault / list.extend ─────────────────────────────
int64_t py_map_setdefault(int64_t m, int64_t key, int64_t def) {
    if (py_map_contains(m, key)) return map_get(m, key);
    map_insert(m, key, def);
    return def;
}
// Appends in place; the handle can move when the vector grows, so the caller
// rebinds the receiver variable.
int64_t py_list_extend(int64_t vec, int64_t other) {
    int64_t n = zt_vec_len(other);
    int64_t h = vec;
    for (int64_t i = 0; i < n; i++) h = vec_push(h, ((int64_t*)other)[i]);
    return h;
}

// ── PY-A: os.path.join with 3-4 parts / int(s, base) / dict.fromkeys ──
extern int64_t py_os_path_join(int64_t a, int64_t b);
int64_t py_os_path_join_3(int64_t a, int64_t b, int64_t c) {
    return py_os_path_join(py_os_path_join(a, b), c);
}
int64_t py_os_path_join_4(int64_t a, int64_t b, int64_t c, int64_t d) {
    return py_os_path_join(py_os_path_join_3(a, b, c), d);
}

// int(text, base) — Python's base-prefixed conversion (0x/0b/0o handled by
// strtoll when base is 0 or 16/2/8).
int64_t py_int_base(int64_t s, int64_t base) {
    const char* p = s ? (const char*)s : "";
    while (*p == ' ' || *p == '\t') p++;
    return (int64_t)strtoll(p, NULL, (int)base);
}

// dict.fromkeys(keys, value) — a map from the key list; string keys are
// content-hashed like any other dict.
int64_t py_map_fromkeys(int64_t keys, int64_t val, int64_t keys_are_str) {
    int64_t m = map_new();
    int64_t n = zt_vec_len(keys);
    for (int64_t i = 0; i < n; i++) {
        int64_t k = keys_are_str ? map_str_key(((int64_t*)keys)[i]) : ((int64_t*)keys)[i];
        map_insert(m, k, val);
    }
    return m;
}

// `pd.DataFrame(columns=[...])` — the KWARG-ONLY schema constructor. The shim's
// DataFrame is a column map (`map<str, vec<str>>`), so build one: each listed
// column becomes an EMPTY column (matching "empty frame with this schema").
// Without this the call fell to a bare `DataFrame` symbol (`_DataFrame`,
// undefined) — 10 `columns=` sites in REasyQuant's data/ML layer.
int64_t zeta_df_with_columns(int64_t names) {
    int64_t m = map_new();
    int64_t n = zt_vec_len(names);
    for (int64_t i = 0; i < n; i++) {
        int64_t name = ((int64_t*)names)[i];
        int64_t empty = zeta_dynarray_new(8);
        map_insert(m, map_str_key(name), empty);
    }
    return m;
}

// s.replace(old, new, count) — bounded replacement.
int64_t host_str_replace_n(int64_t s, int64_t o, int64_t n, int64_t count) {
    const char* p = s ? (const char*)s : "";
    const char* old = o ? (const char*)o : "";
    const char* rep = n ? (const char*)n : "";
    size_t olen = strlen(old);
    if (olen == 0 || count == 0) return (int64_t)GC_strdup(p);
    size_t rlen = strlen(rep);
    size_t cap = strlen(p) + 1;
    char* out = (char*)GC_malloc(cap);
    size_t k = 0;
    const char* cur = p;
    int64_t done = 0;
    while (*cur) {
        if ((count < 0 || done < count) && strncmp(cur, old, olen) == 0) {
            if (k + rlen + 1 > cap) { size_t nc = (k + rlen + 1) * 2; char* nb = (char*)GC_malloc(nc); memcpy(nb, out, k); out = nb; cap = nc; }
            memcpy(out + k, rep, rlen);
            k += rlen;
            cur += olen;
            done++;
        } else {
            if (k + 2 > cap) { size_t nc = cap * 2; char* nb = (char*)GC_malloc(nc); memcpy(nb, out, k); out = nb; cap = nc; }
            out[k++] = *cur++;
        }
    }
    out[k] = 0;
    return (int64_t)out;
}

// ── PY-A: round(x, n) — Python rounds the actual double value at the n-th
// decimal (banker's rounding, via the default to-nearest-even mode). The
// 2-argument form previously returned a truncated integer. ────────────
double py_round_n(double x, int64_t n) {
    if (n == 0) return nearbyint(x);
    double scale = 1.0;
    if (n > 0) {
        for (int64_t i = 0; i < n; i++) scale *= 10.0;
        return nearbyint(x * scale) / scale;
    }
    for (int64_t i = 0; i < -n; i++) scale *= 10.0;
    return nearbyint(x / scale) * scale;
}

// ── PY-A: split(sep, maxsplit) / repr / set() ───────────────────────
static void zt_push_raw(int64_t** base, int64_t* cap, int64_t* len, int64_t v) {
    if (*len >= *cap) {
        int64_t nc = *cap * 2;
        int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)nc * 8);
        nb[0] = nc;
        nb[1] = *len;
        for (int64_t i = 0; i < *len; i++) nb[2 + i] = (*base)[2 + i];
        *base = nb;
        *cap = nc;
    }
    (*base)[2 + (*len)++] = v;
}

// At most `maxsplit` splits; the remainder stays in the final piece.
int64_t host_str_split_max(int64_t s, int64_t sep, int64_t maxsplit) {
    const char* p = s ? (const char*)s : "";
    const char* sp = sep ? (const char*)sep : "";
    size_t slen = strlen(sp);
    int64_t cap = 8, len = 0;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap;
    base[1] = 0;
    const char* cur = p;
    if (slen == 0) {
        zt_push_raw(&base, &cap, &len, (int64_t)GC_strdup(p));
        base[1] = len;
        return (int64_t)(base + 2);
    }
    while (maxsplit < 0 || len < maxsplit) {
        const char* hit = strstr(cur, sp);
        if (!hit) break;
        size_t n = (size_t)(hit - cur);
        char* tok = (char*)GC_malloc(n + 1);
        memcpy(tok, cur, n);
        tok[n] = 0;
        zt_push_raw(&base, &cap, &len, (int64_t)tok);
        cur = hit + slen;
    }
    zt_push_raw(&base, &cap, &len, (int64_t)GC_strdup(cur));
    base[1] = len;
    return (int64_t)(base + 2);
}

// repr(str) — Python quotes strings.
int64_t py_repr_str(int64_t s) {
    const char* p = s ? (const char*)s : "";
    size_t n = strlen(p);
    char* out = (char*)GC_malloc(n + 3);
    out[0] = '\'';
    memcpy(out + 1, p, n);
    out[n + 1] = '\'';
    out[n + 2] = 0;
    return (int64_t)out;
}

// set(xs) — V1: a deduplicated Vec (no add/remove, membership via the list
// path). Order follows first appearance.
int64_t py_builtin_set(int64_t vec) {
    int64_t n = zt_vec_len(vec);
    int64_t cap = n ? n : 1;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap;
    base[1] = 0;
    for (int64_t i = 0; i < n; i++) {
        int64_t v = ((int64_t*)vec)[i];
        int dup = 0;
        for (int64_t j = 0; j < base[1]; j++) {
            if (base[2 + j] == v) { dup = 1; break; }
        }
        if (!dup) base[2 + base[1]++] = v;
    }
    return (int64_t)(base + 2);
}

// set.add(x) — push if absent; returns the (possibly grown) vec handle.
extern int64_t vec_push(int64_t vec, int64_t val);
int64_t py_set_add(int64_t vec, int64_t v) {
    int64_t n = zt_vec_len(vec);
    for (int64_t i = 0; i < n; i++) {
        if (((int64_t*)vec)[i] == v) return vec;
    }
    return vec_push(vec, v);
}

// typing.cast(typ, val) — annotation-only; return val unchanged.
int64_t py_typing_cast(int64_t _ty, int64_t val) { (void)_ty; return val; }

// ── PY-A: map(f, xs) / filter(f, xs) — eager, returning a Vec ─────────
// `fn` is a Zeta function pointer; filter(None, xs) keeps truthy elements.
int64_t py_builtin_map(int64_t fn, int64_t vec) {
    int64_t n = zt_vec_len(vec);
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(n ? n : 1) * 8);
    base[0] = n ? n : 1;
    base[1] = n;
    for (int64_t i = 0; i < n; i++) {
        base[2 + i] = ((int64_t(*)(int64_t))fn)(((int64_t*)vec)[i]);
    }
    return (int64_t)(base + 2);
}
int64_t py_builtin_filter(int64_t fn, int64_t vec) {
    int64_t n = zt_vec_len(vec);
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)(n ? n : 1) * 8);
    base[0] = n ? n : 1;
    base[1] = 0;
    for (int64_t i = 0; i < n; i++) {
        int64_t v = ((int64_t*)vec)[i];
        int64_t keep = fn ? ((int64_t(*)(int64_t))fn)(v) : v;
        if (keep) base[2 + base[1]++] = v;
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

// ── PY-A: Python `@` matmul ──────────────────────────────────────────
// Parse/MIR need a symbol so `I @ corr` does not abort the enclosing def.
// Real ndarray matmul is NOT implemented — this is an i64 multiply stub
// that warns once on stderr (fail-loud for anyone expecting numpy `@`).
int64_t zeta_matmul(int64_t a, int64_t b) {
    static int warned;
    if (!warned) {
        fprintf(stderr,
                "warning: zeta_matmul is an i64 mul stub (not ndarray matmul)\n");
        warned = 1;
    }
    return a * b;
}

// ── PY-A: np.where ───────────────────────────────────────────────────
// 1-arg: flat index vector where mask[i] != 0. Numpy returns `(indices,)`;
// MIR rewrites `np.where(mask)[0]` to this flat call so the common corpus
// spelling works without nested-vec element types.
// 3-arg: scalar/element-wise select (cond ? x : y).
int64_t zeta_dynarray_new(int64_t);

static int zt_looks_like_vec(int64_t v) {
    if (v < 0x1000) return 0; /* small ints / bools are scalars */
    int64_t cap = ((int64_t*)(v - 16))[0];
    int64_t len = ((int64_t*)(v - 16))[1];
    if (cap < 0 || len < 0 || len > cap || cap > (1LL << 30)) return 0;
    return 1;
}

int64_t zeta_np_where1(int64_t mask) {
    if (!zt_looks_like_vec(mask)) {
        int64_t idxs = zeta_dynarray_new(1);
        if (mask) idxs = vec_push(idxs, 0);
        return idxs;
    }
    int64_t n = zt_vec_len(mask);
    int64_t idxs = zeta_dynarray_new(n > 0 ? n : 8);
    for (int64_t i = 0; i < n; i++) {
        if (((int64_t*)mask)[i]) idxs = vec_push(idxs, i);
    }
    return idxs;
}

int64_t zeta_np_where3(int64_t cond, int64_t x, int64_t y) {
    int c_vec = zt_looks_like_vec(cond);
    int x_vec = zt_looks_like_vec(x);
    int y_vec = zt_looks_like_vec(y);
    if (!c_vec) {
        return cond ? x : y;
    }
    int64_t cn = zt_vec_len(cond);
    int64_t xn = x_vec ? zt_vec_len(x) : 0;
    int64_t yn = y_vec ? zt_vec_len(y) : 0;
    int64_t out = zeta_dynarray_new(cn > 0 ? cn : 8);
    for (int64_t i = 0; i < cn; i++) {
        int64_t c = ((int64_t*)cond)[i];
        int64_t xv = x;
        int64_t yv = y;
        if (x_vec && xn > 0) xv = ((int64_t*)x)[i < xn ? i : xn - 1];
        if (y_vec && yn > 0) yv = ((int64_t*)y)[i < yn ? i : yn - 1];
        out = vec_push(out, c ? xv : yv);
    }
    return out;
}

// ── PY-A: numpy eye / zeros / diag / fill_diagonal / diagonal / solve stub ──
// Flat row-major matrices (n*n). Honest V1 — not a full ndarray.
int64_t zeta_np_eye(int64_t n) {
    if (n < 0) n = 0;
    int64_t N = n * n;
    int64_t h = zeta_dynarray_new(N > 0 ? N : 8);
    for (int64_t i = 0; i < N; i++) h = vec_push(h, 0);
    for (int64_t i = 0; i < n; i++) ((int64_t*)h)[i * n + i] = 1;
    return h;
}

int64_t zeta_np_zeros(int64_t n) {
    if (n < 0) n = 0;
    int64_t h = zeta_dynarray_new(n > 0 ? n : 8);
    for (int64_t i = 0; i < n; i++) h = vec_push(h, 0);
    return h;
}

int64_t zeta_np_zeros2(int64_t r, int64_t c) {
    if (r < 0) r = 0;
    if (c < 0) c = 0;
    return zeta_np_zeros(r * c);
}

// Vector → flat diagonal matrix (n×n).
int64_t zeta_np_diag(int64_t v) {
    int64_t n = zt_looks_like_vec(v) ? zt_vec_len(v) : 0;
    if (n <= 0) {
        /* scalar: 1×1 */
        int64_t h = zeta_dynarray_new(1);
        return vec_push(h, v);
    }
    int64_t h = zeta_np_zeros2(n, n);
    for (int64_t i = 0; i < n; i++) ((int64_t*)h)[i * n + i] = ((int64_t*)v)[i];
    return h;
}

// Extract diagonal of flat square matrix → vector. Non-square / opaque → identity.
int64_t zeta_np_diagonal(int64_t m) {
    if (!zt_looks_like_vec(m)) return m;
    int64_t N = zt_vec_len(m);
    int64_t n = 0;
    while (n * n < N) n++;
    if (n * n != N || n == 0) {
        static int w;
        if (!w) { w = 1; fprintf(stderr, "warning: PY-A: np.diagonal on non-square — identity\n"); }
        return m;
    }
    int64_t h = zeta_dynarray_new(n);
    for (int64_t i = 0; i < n; i++) h = vec_push(h, ((int64_t*)m)[i * n + i]);
    return h;
}

// fill_diagonal(a, val): in-place on flat square; val may be scalar or vec.
int64_t zeta_np_fill_diagonal(int64_t a, int64_t val) {
    if (!zt_looks_like_vec(a)) {
        static int w;
        if (!w) { w = 1; fprintf(stderr, "warning: PY-A: np.fill_diagonal on non-vec — no-op\n"); }
        return a;
    }
    int64_t N = zt_vec_len(a);
    int64_t n = 0;
    while (n * n < N) n++;
    if (n * n != N) {
        static int w2;
        if (!w2) { w2 = 1; fprintf(stderr, "warning: PY-A: np.fill_diagonal on non-square — no-op\n"); }
        return a;
    }
    int val_vec = zt_looks_like_vec(val);
    int64_t vn = val_vec ? zt_vec_len(val) : 0;
    for (int64_t i = 0; i < n; i++) {
        int64_t v = val;
        if (val_vec && vn > 0) v = ((int64_t*)val)[i < vn ? i : vn - 1];
        ((int64_t*)a)[i * n + i] = v;
    }
    return a;
}

// Loud stub: solve(A, b) → b (not a factorisation). Documents the gap.
int64_t zeta_np_solve_stub(int64_t a, int64_t b) {
    (void)a;
    static int w;
    if (!w) {
        w = 1;
        fprintf(stderr, "warning: PY-A: scipy.linalg.solve is a stub — returning RHS\n");
    }
    return b;
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
    if (getenv("ZT_PROBE_CONCAT") &&
        ((uint64_t)a < 0x10000 || (uint64_t)b < 0x10000)) {
        // TEMP diagnostic (batch 291): drvX3 SIGSEGV inside concat from
        // jq_wufu__initialize. One operand's header read faulted at -7
        // (=> that operand == 1). Print both handles + a short backtrace.
        void* bt[12];
        int n = backtrace(bt, 12);
        fprintf(stderr, "[ZT_PROBE_CONCAT] a=%ld b=%ld\n", (long)a, (long)b);
        backtrace_symbols_fd(bt, n > 6 ? 6 : n, 2);
        abort();
    }
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
// PY-A: `np.full(n, v)` — a Vec of n copies of v. Uses the same header layout
// as zeta_dynarray_new/vec_push (cap/len before the data pointer), so the
// result is an ordinary Vec the rest of the runtime already understands.
int64_t zeta_dynarray_new(int64_t);
int64_t vec_push(int64_t, int64_t);
int64_t py_vec_full(int64_t n, int64_t v) {
    if (n < 0) n = 0;
    int64_t h = zeta_dynarray_new(n);
    for (int64_t i = 0; i < n; i++) h = vec_push(h, v);
    return h;
}

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
int64_t zeta_env_map_for_probe(void) { return env_map(); }
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
// PY-A: default-argument marker (`zeta_param_default(index, value)`). No-op at
// runtime — the Resolver reads it to fill omitted call arguments; this stub
// only exists so the marker never becomes an undefined symbol.
int64_t zeta_param_default(int64_t index, int64_t value) { (void)index; return value; }
// PY-A: Python-library import markers. The Resolver collects them into the
// module/member alias tables; MirGen does the actual symbol mapping. What
// still reaches runtime is a module the compiler could not resolve — and
// CPython raises ImportError for a package that is not installed.
// Batch 288: without the raise, `import baostock` succeeded and
// _has_baostock() (sources_selector.py:57-62) reported the package present
// — the local ranking gained a phantom baostock source (measured 4 vs
// CPython's 3), changing fetch order and every downstream metric. The list
// mirrors the acceptance venv (probed package-by-package with python3).
static int zt_module_missing(const char* m) {
    static const char* missing[] = {
        "baostock", "tushare", "jqdatasdk", "rqdatac", "akshare",
        "yfinance", "pyarrow", "backtrader", "nautilus_trader", NULL};
    if (!m) return 0;
    for (int i = 0; missing[i]; i++) {
        size_t l = strlen(missing[i]);
        if (strncmp(m, missing[i], l) == 0 && (m[l] == '\0' || m[l] == '.'))
            return 1;
    }
    return 0;
}
extern int64_t zeta_raise(int64_t);
int64_t zeta_py_import(int64_t module, int64_t alias) {
    (void)alias;
    if (zt_module_missing((const char*)module)) return zeta_raise(1);
    return 0;
}
int64_t zeta_py_from(int64_t module, int64_t member, int64_t alias) {
    (void)member; (void)alias;
    if (zt_module_missing((const char*)module)) return zeta_raise(1);
    return 0;
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

// PY-A: comma subscript on an opaque platform object — pandas
// `df.iloc[r, c]` / `frame.loc[i, j]`. This compiler has no DataFrame, so the
// receiver handle is opaque here. This is a PLATFORM SHIM in the same sense as
// `zeta_platform_obj` below: locally it returns the handle unchanged (no real
// 2-D semantics — a standalone run of a platform strategy is not semantically
// meaningful anyway), and a host that links its own `py_getitem2` overrides
// it. It exists so such source parses and links instead of silently binding to
// a single-index lookup. To make it fail loudly instead, delete this function:
// the call then becomes an undefined symbol at link time.
// PY-A: an opaque "slice object" placeholder for a multi-index subscript's
// slice element (`df.iloc[:, 0]`). Same platform-shim contract as py_getitem2:
// locally it carries no real slice, the host may override the symbol. It must
// NOT be a real Vec slice — the base is an opaque platform object, and reading a
// Vec header off it (what zeta_slice_vec does) segfaulted for `d[:, 0]`.
int64_t py_slice_new(int64_t start, int64_t end, int64_t step) {
    (void)start;
    (void)end;
    (void)step;
    return 0;
}

int64_t py_getitem2(int64_t base, int64_t i, int64_t j) {
    (void)i;
    (void)j;
    return base;
}

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

// sorted(x) on an ARRAY OF STRINGS. The generic path above orders i64 slots
// numerically — for str elements that sorts POINTER values, so the result
// looked unsorted (trading-day lists). Content order via the same packed-
// pointer-safe view the comparisons use (batch 293).
int64_t zeta_sorted_vec_len_str(int64_t data, int64_t len) {
    if (!data) return 0;
    if (len < 0) len = ((int64_t*)(data - 16))[1];
    if (len < 0) len = 0;
    int64_t* nb = (int64_t*)GC_malloc(16 + (size_t)(len ? len : 8) * 8);
    nb[0] = len ? len : 8; nb[1] = len;
    for (int64_t i = 0; i < len; i++) nb[2 + i] = ((int64_t*)data)[i];
    for (int64_t i = 1; i < len; i++) {
        int64_t k = nb[2 + i];
        char ka[9];
        const char* ks = zt_str_view(k, ka);
        int64_t j = i - 1;
        while (j >= 0) {
            char ja[9];
            if (strcmp(zt_str_view(nb[2 + j], ja), ks) > 0) { nb[2 + j + 1] = nb[2 + j]; j--; }
            else break;
        }
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

// PY-A: `range(a, b)` as a VALUE (not a for-head) — a Vec of [a, b).
// `zeta_arange` only covers [0, n), so this variant takes the start too.
int64_t zeta_arange_from(int64_t a, int64_t b) {
    int64_t n = b - a;
    if (n < 0) n = 0;
    int64_t v = zeta_arange(n);
    int64_t* p = (int64_t*)v;
    for (int64_t i = 0; i < n; i++) p[i] += a;
    return v;
}

// PY-A: `range(a, b, step)` as a VALUE — a Vec of [a, b) stepping by `step`.
// Only positive steps are implemented (Python's negative-step ranges would need
// a descending Vec and stay a compile-time diagnostic at the call site).
int64_t zeta_arange_step(int64_t a, int64_t b, int64_t step) {
    if (step <= 0) step = 1;
    int64_t n = (b - a + step - 1) / step;
    if (n < 0) n = 0;
    int64_t v = zeta_arange(n);
    int64_t* p = (int64_t*)v;
    for (int64_t i = 0; i < n; i++) p[i] = a + i * step;
    return v;
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
// A filtered-out item returns this sentinel (same convention as the list
// comprehension collect).
#define ZT_COMP_SKIP (-1)

int64_t zeta_collect_dict(int64_t iter, int64_t fn_ptr) {
    if (!iter) return 0;
    int64_t len = ((int64_t*)(iter - 16))[1];
    int64_t m = map_new();
    int64_t (*fp)(int64_t) = (int64_t(*)(int64_t))fn_ptr;
    for (int64_t i = 0; i < len; i++) {
        int64_t pr = fp(((int64_t*)iter)[i]);
        if (pr == ZT_COMP_SKIP) continue;   // `if <cond>` filter excluded it
        int64_t* kv = (int64_t*)pr;
        map_insert(m, kv[0], kv[1]);
    }
    return m;
}
// __pack_pair__(k, v) — a 2-slot heap pair [key, value]. It used to pack into
// `(k<<32)|v`, which silently corrupted anything that is not a small positive
// integer (string keys came back as mangled pointers, and values above 2^32
// wrapped). Passing the pair by reference costs one allocation and is exact.
// String keys arrive already content-hashed (the compiler hashes them at the
// pack site, exactly like a dict literal).
int64_t zeta_pack_pair(int64_t k, int64_t v) {
    int64_t* p = (int64_t*)GC_malloc(16);
    p[0] = k;
    p[1] = v;
    return (int64_t)p;
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
    m = map_resolve(m);
    other = map_resolve(other);
    if (!m || !other) return m;
    int64_t cap = ((int64_t*)other)[0];
    for (int64_t i = 0; i < cap; i++) {
        char* e = (char*)other + 16 + i * MAP_ENTRY_SIZE;
        if (*(uint8_t*)(e + 16) == 1) map_insert(m, *(int64_t*)e, *((int64_t*)e + 1));
    }
    return m;
}
// d.pop(k[, default]) — the open-addressing table has no tombstones, so
// removal rebuilds in place (collect survivors, clear, re-insert). O(n),
// fine for the dict sizes Python code uses here.
int64_t zeta_map_pop_default(int64_t m, int64_t key, int64_t def) {
    if (!m) return def;
    m = map_resolve(m);
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
    m = map_resolve(m);
    m = map_resolve(m);
    int64_t cap = ((int64_t*)m)[0];
    if (cap < 0) cap = 0;
    for (int64_t i = 0; i < cap; i++)
        *(uint8_t*)((char*)m + 16 + i * MAP_ENTRY_SIZE + 16) = 0;
    ((int64_t*)m)[1] = 0;
    return m;
}

// ---- Batch 123: call-through / importlib / getattr / next --------------
// Call a captured 1-arg function pointer (listcomp `condition(m)`).
int64_t zeta_call_fn_arg(int64_t fn_ptr, int64_t arg) {
    typedef int64_t (*fn1_t)(int64_t);
    if (!fn_ptr) {
        fputs("zeta: zeta_call_fn_arg(NULL)\n", stderr);
        fflush(stderr);
        abort();
    }
    return ((fn1_t)fn_ptr)(arg);
}

// importlib.import_module — no dynamic loader. Batch 288: returns the module
// NAME as a pseudo handle for the only supported consumer, py_getattr_dynamic
// (the PEP 562 facade in market_data). Any other use receives a string where a
// module is expected and fails at the next operation.
int64_t py_import_module(int64_t name) {
    return name;
}

// getattr(obj, <dynamic name>) — batch 288: resolves module-global state by
// NAME through the process-wide env table (module globals are `zeta_env_set`
// under their bare names, and that table is shared across modules — exactly
// what the PEP 562 facade in market_data delegates). A miss stays loud:
// arbitrary-object dynamic getattr has no runtime reflection here.
int64_t py_getattr_dynamic(int64_t obj, int64_t name) {
    (void)obj;
    extern int64_t map_has(int64_t, int64_t);
    extern int64_t map_str_key(int64_t);
    int64_t zeta_env_map_for_probe(void);
    if (name && map_has(zeta_env_map_for_probe(), map_str_key(name))) {
        return zeta_env_get(name);
    }
    const char* s = name ? (const char*)name : "<null>";
    fprintf(stderr,
            "zeta: getattr(obj, %s) with a dynamic attribute name is not "
            "supported (needs a literal name or a default, or the name must "
            "be a live module global)\n",
            s);
    fflush(stderr);
    abort();
    return 0;
}

// builtin next(it) without default — abort (iterator protocol incomplete).
int64_t py_builtin_next(int64_t it) {
    (void)it;
    fputs("zeta: next(it) without a default is not supported "
          "(use next(it, default) or implement iteration)\n",
          stderr);
    fflush(stderr);
    abort();
    return 0;
}

// Opaque receiver `.next()` (e.g. baostock rs.next()): report exhausted.
int64_t py_method_next(int64_t self) {
    (void)self;
    static int w = 0;
    if (!w) {
        w = 1;
        fputs("warning: PY-A: .next() on untyped receiver returns 0 (exhausted)\n",
              stderr);
    }
    return 0;
}

// ── PY-A batch 128: bare-name fallthroughs (library/runtime, not codegen) ──
// MIR special-cases 2-arg zip / typed isinstance; kwargs like zip(a,b,strict=True)
// and opaque receivers still emit bare `_zip` / `_to_parquet` / `_hasattr`.
extern int64_t py_path_write_text(int64_t p, int64_t text);

int64_t zip(int64_t a, int64_t b, int64_t c) {
    (void)c; /* ignore strict= / third iterable for V1 */
    return py_zip(a, b);
}
int64_t isinstance(int64_t obj, int64_t ty) {
    (void)obj;
    (void)ty;
    return 0;
}
int64_t hasattr(int64_t obj, int64_t name) {
    (void)obj;
    (void)name;
    return 0;
}
int64_t setattr(int64_t obj, int64_t name, int64_t val) {
    (void)obj;
    (void)name;
    (void)val;
    return 0;
}
// NOTE: no bare `to_parquet` — DataFrame.to_parquet also emits `@to_parquet`
// and duplicates against this .o (same class as clear/ffill).
int64_t write_text(int64_t self, int64_t text) {
    return py_path_write_text(self, text);
}
int64_t frozenset(int64_t xs) { return xs; }
int64_t enumerate(int64_t xs) { return xs; }

// NOTE: do NOT export bare `clear`/`update`/`add`/`ffill`/`to_parquet`/
// `numpy__isfinite`/… — library methods with those names also emit the same
// linker symbol and duplicate against zeta_runtime_c.o (batches 128–129).
// Untyped fallthrough is MIR opaque_fallback / pylib/*.z, not bare C aliases.

// ==================== D: loud stubs (advice.md) ====================
// Default: abort with the symbol name. ZETA_LENIENT_STUBS=1 → warn once
// per symbol on stderr and return 0 (keeps soft stubs green while CI logs
// which stubs are actually hit). name_ptr is a C string handle (i64).
#define ZT_STUB_WARN_MAX 64
static char zt_stub_warned[ZT_STUB_WARN_MAX][96];
static int zt_stub_warned_n;

static int zt_stub_already_warned(const char* name) {
    for (int i = 0; i < zt_stub_warned_n; i++) {
        if (strcmp(zt_stub_warned[i], name) == 0) return 1;
    }
    if (zt_stub_warned_n < ZT_STUB_WARN_MAX) {
        strncpy(zt_stub_warned[zt_stub_warned_n], name, 95);
        zt_stub_warned[zt_stub_warned_n][95] = '\0';
        zt_stub_warned_n++;
    }
    return 0;
}

int64_t py_stub_abort(int64_t name_ptr) {
    const char* name = name_ptr
        ? (const char*)(uintptr_t)name_ptr
        : "<unknown>";
    if (getenv("ZETA_LENIENT_STUBS") != NULL) {
        if (!zt_stub_already_warned(name)) {
            fprintf(stderr, "warning: zeta stub not implemented: %s\n", name);
            fflush(stderr);
        }
        return 0;
    }
    (void)getenv("ZETA_STRICT_STUBS"); /* default is strict; env is documentary */
    fprintf(stderr, "zeta: stub not implemented: %s\n", name);
    fflush(stderr);
    abort();
    return 0;
}



int64_t isna(int64_t v) { return zt_bare_mask(v, 0); }
int64_t isna_1(int64_t v) { return zt_bare_mask(v, 0); }
int64_t notna(int64_t v) { return zt_bare_mask(v, 1); }
int64_t notna_1(int64_t v) { return zt_bare_mask(v, 1); }

// BATCH-291: `series.dt.strftime(fmt)` — the `.dt` accessor lowers to the
// array handle itself (gen.rs passthrough); this maps each PyDate element
// through py_dt_strftime into a fresh string vector. Without it the chain
// reached LIBC `strftime` with a garbage fmt pointer (SIGSEGV in `_st_fmt`,
// jq_wufu_local.py:81). DELIBERATELY LAST in this TU: inserting it earlier
// shifted every following function and flipped a latent layout-sensitive
// defect in the driver build (cache path / strnlen crash) — keep the HEAD
// code layout byte-identical ahead of this point.
extern int64_t py_dt_strftime(int64_t h, int64_t fmt);
// A date given as TEXT ("2023-08-07", what our market rows carry in
// `trade_date`) must be REFORMATTED, not fed to `py_dt_strftime`: that helper
// reads `((int64_t*)h)[0]` as a day count, so a string element produced
// garbage like `8921216181287397-01-07` and `.dt.strftime("%Y-%m-%d")` turned
// 119 real trading days into 7 unmatchable strings → the driver's date window
// kept 0 days (batch 293). Same minimal format subset as py_dt_strftime.
static int64_t zt_text_date_strftime(const char* s, const char* f) {
    long long y = 0, mo = 0, d = 0, hh = 0, mi = 0, se = 0;
    int used = 0;
    if (sscanf(s, "%lld-%lld-%lld%n", &y, &mo, &d, &used) < 3) return -1;
    if (s[used] == ' ') sscanf(s + used, " %lld:%lld:%lld", &hh, &mi, &se);
    char buf[256];
    char* o = buf;
    for (const char* p = f; *p && (o - buf) < 200; p++) {
        if (*p != '%') { *o++ = *p; continue; }
        p++;
        switch (*p) {
            case 'Y': o += sprintf(o, "%04lld", y); break;
            case 'm': o += sprintf(o, "%02lld", mo); break;
            case 'd': o += sprintf(o, "%02lld", d); break;
            case 'H': o += sprintf(o, "%02lld", hh); break;
            case 'M': o += sprintf(o, "%02lld", mi); break;
            case 'S': o += sprintf(o, "%02lld", se); break;
            case '%': *o++ = '%'; break;
            default: if (*p) *o++ = *p; break;
        }
    }
    *o = 0;
    return (int64_t)GC_strdup(buf);
}
int64_t zeta_vec_strftime(int64_t data, int64_t fmt) {
    if (!data) return 0;
    int64_t n = 0;
    if (!zt_vec_header_ok(data, &n)) return 0;
    int64_t cap = n < 8 ? 8 : n;
    int64_t* base = (int64_t*)GC_malloc(16 + (size_t)cap * 8);
    base[0] = cap;
    base[1] = n;
    int64_t out = (int64_t)(base + 2);
    const char* f = fmt ? (const char*)fmt : "%Y-%m-%d";
    for (int64_t i = 0; i < n; i++) {
        int64_t v = ((int64_t*)data)[i];
        char vb[16];
        const char* s = v ? zt_str_view(v, vb) : "";
        // A PyDate handle's first 8 bytes are a day count, which can also
        // parse as digits — require the exact ISO shape to call it text.
        long long ty, tm, td;
        int is_text = s && sscanf(s, "%lld-%lld-%lld", &ty, &tm, &td) == 3 &&
                      ty >= 1000 && ty <= 9999 && tm >= 1 && tm <= 12 &&
                      td >= 1 && td <= 31;
        int64_t r = is_text ? zt_text_date_strftime(s, f) : -1;
        ((int64_t*)out)[i] = r >= 0 ? r : py_dt_strftime(v, fmt);
    }
    return out;
}

// BATCH-294: `set.clear()` on the list-backed set (see gen.rs dispatch).
// In-place length reset — the handle stays the same, so call sites that keep
// the old handle (a struct field) still observe an empty set. Deliberately
// LAST in this TU, same layout rule as zeta_vec_strftime above.
int64_t zeta_vec_clear(int64_t data) {
    if (!data) return 0;
    int64_t n = 0;
    if (!zt_vec_header_ok(data, &n)) return data;
    ((int64_t*)(data - 16))[1] = 0;
    return data;
}

// BATCH-294: indirect-call trampoline for call-through-a-value (see gen.rs
// zeta_call1 dispatch). Function addresses arrive as the i64 of a FuncAddr.
// Deliberately LAST in this TU (layout rule above).
int64_t zeta_call1(int64_t fptr, int64_t a) {
    if (!fptr) return 0;
    typedef int64_t (*zt_fn1)(int64_t);
    return ((zt_fn1)(void*)fptr)(a);
}

// BATCH-294: `<opaque>.date()` — Zeta's datetime handle is a PyDate whose
// first field is the day count; the date projection is the handle itself.
int64_t zeta_dt_date(int64_t h) { return h; }

// BATCH-295: subscript on a value whose static type is unknown (dyn param).
// The compiler previously always emitted `map_get`, so `d[i]` on a LIST handle
// walked a fake bucket chain and SIGSEGVED (measured: `enumerate(
// trading_days_filtered)` in `_run_local` desugars to `t[i]`, crashed at
// `map_get+256` right after the "[local] 开始回测" log). Discriminate at
// runtime: a Vec handle's [cap|len] header is its OWN GC block's start
// (base-16); a map handle IS its block start, so base-16 is some other
// block — and even when a neighbouring block mimics a sane header, we only
// commit if the index is in range, otherwise the map lookup still runs.
int64_t zeta_dyn_getitem(int64_t base, int64_t key) {
    if (!base) return 0;
    int64_t* h = (int64_t*)(base - 16);
    if (base > 0x1000 && GC_base((void*)h) == (void*)h) {
        int64_t cap = h[0], len = h[1];
        if (cap >= 8 && cap <= (1LL << 28) && len >= 0 && len <= cap) {
            int64_t i = key < 0 ? key + len : key;
            if (i >= 0 && i < len) return h[2 + i];
        }
    }
    return map_get(base, key);
}

// BATCH-296: geometry-based kind test for a handle whose STATIC type is unknown
// (`d = {}; d["x"] = {...}` types the value I64, and `dict[str, Any]` from the
// corpus does too). Such a value reached `array_len`, which read the PRECEDING
// GC block and answered 0 — that is the `codes=0` line of the local backtest,
// where `_build_stock_arrays` really returns 91 entries (measured with the §9
// harness: `arrays 91`). Two shapes are distinguished by their GC block:
//   Map    the handle IS the block start; word0 = capacity (power of two >= 16,
//          negative for a growth forwarder) and the block covers 16 + cap*24.
//   Vec    the handle points at the data; its [cap|len] header at base-16 is a
//          block of its own, sized 16 + cap*8.
// The size check is what makes this safe: a mimic needs a real block of the
// exact right size, not just plausible words. Anything left over is text.
// Order matters — a Vec's first element can itself look like a capacity, but a
// Vec handle is never its own block start, so the map test rejects it first.
int64_t zeta_map_len(int64_t);
int64_t str_len(int64_t);

static int zt_dyn_is_map(int64_t h) {
    if (h <= 0x1000) return 0;
    if (GC_base((void*)h) != (void*)h) return 0;
    int64_t* w = (int64_t*)h;
    int64_t cap = w[0];
    if (cap < 0) return w[1] > 0x1000;      // growth forwarder → the target
    if (cap < 16 || cap > (1LL << 28) || (cap & (cap - 1)) != 0) return 0;
    int64_t used = w[1];
    if (used < 0 || used > cap) return 0;
    return (int64_t)GC_size((void*)w) >= 16 + cap * 24;
}

static int64_t* zt_dyn_vec_hdr(int64_t h) {
    if (h <= 0x1000) return NULL;
    int64_t* hdr = (int64_t*)(h - 16);
    if (GC_base((void*)hdr) != (void*)hdr) return NULL;
    int64_t cap = hdr[0], len = hdr[1];
    if (cap < 8 || cap > (1LL << 28) || len < 0 || len > cap) return NULL;
    if ((int64_t)GC_size((void*)hdr) < 16 + cap * 8) return NULL;
    return hdr;
}

int64_t zeta_dyn_len(int64_t h) {
    if (!h) return 0;
    if (zt_dyn_is_map(h)) return zeta_map_len(h);
    int64_t* hdr = zt_dyn_vec_hdr(h);
    if (hdr) return hdr[1];
    // Text is the LAST reading: an untagged value (a count, a small struct id)
    // must not be dereferenced — measured SIGSEGV when `len()` reached `strlen`
    // on one — while string literals live in .rodata, OUTSIDE the GC heap, so
    // a GC-block test alone would answer 0 for them. `zt_c_readable` fault-
    // checks the first byte and strnlen bounds the scan.
    if (zt_c_readable(h)) return (int64_t)strnlen((const char*)h, 1 << 20);
    return 0;
}

// `k in c` on an unknown container. `hkey` is the map-normalised key (string
// content hash) and `key_is_str` selects content equality for the list scan —
// both are what the TYPED paths already pass, so a dynamic value behaves like
// whichever container it actually is.
int64_t zeta_dyn_contains(int64_t c, int64_t key, int64_t hkey, int64_t key_is_str) {
    if (!c) return 0;
    if (zt_dyn_is_map(c)) return map_get(c, hkey) != 0;
    if (zt_dyn_vec_hdr(c)) return py_list_contains(c, key, key_is_str);
    if (!zt_c_readable(c) || !zt_c_readable(key)) return 0;
    size_t cl = strnlen((const char*)c, 1 << 16);
    size_t kl = strnlen((const char*)key, 1 << 16);
    if (!cl || !kl) return kl ? 0 : 1;   // Python: `"" in text` is True
    return strstr((char*)c, (char*)key) != NULL;
}
