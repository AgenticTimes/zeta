// ============================================================================
// Minimal PARQUET reader — enough for the REasyQuant local cache files written
// by pandas/pyarrow (SNAPPY compression, PLAIN and RLE_DICTIONARY pages,
// one or more row groups).
//
// File layout: "PAR1" | column chunks | FileMetaData (thrift compact) | u32 len | "PAR1"
// No external library is available at runtime, so the thrift compact protocol and
// the page decoders live here. SNAPPY + the RLE/bit-packed hybrid are in
// py_additions.c.
// ============================================================================
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int64_t zt_snappy_uncompress(const char* in, int64_t in_len, char* out, int64_t out_cap);
int64_t zt_rle_bitpack_decode(const char* in, int64_t in_len, int bit_width, int64_t* out,
                              int64_t out_cap);

// ---- thrift compact protocol ------------------------------------------------
typedef struct {
    const unsigned char* p;
    const unsigned char* end;
    int64_t last_fid;
} tp_t;

static int64_t tp_uvarint(tp_t* t) {
    int64_t v = 0, sh = 0;
    while (t->p < t->end && sh < 64) {
        unsigned char b = *t->p++;
        v |= (int64_t)(b & 0x7f) << sh;
        sh += 7;
        if (!(b & 0x80)) break;
    }
    return v;
}
static int64_t tp_zigzag(tp_t* t) {
    int64_t u = tp_uvarint(t);
    return (u >> 1) ^ -(u & 1);
}
static int tp_field(tp_t* t, int* fid, int* ftype) {
    if (t->p >= t->end) return 0;
    unsigned char b = *t->p++;
    if (b == 0) return 0;
    *ftype = b & 0x0f;
    int delta = (b >> 4) & 0x0f;
    if (delta == 0) {
        *fid = (int)tp_zigzag(t);
    } else {
        *fid = (int)(t->last_fid + delta);
    }
    t->last_fid = *fid;
    return 1;
}
static void tp_skip(tp_t* t, int type);
static void tp_skip_struct(tp_t* t) {
    int64_t saved = t->last_fid;
    t->last_fid = 0;
    int fid, ftype;
    while (tp_field(t, &fid, &ftype)) tp_skip(t, ftype);
    t->last_fid = saved;
}
// list header: high nibble = size (15 => varint), low nibble = elem type
static int tp_list(tp_t* t, int64_t* size, int* etype) {
    if (t->p >= t->end) return 0;
    unsigned char b = *t->p++;
    *etype = b & 0x0f;
    int64_t n = (b >> 4) & 0x0f;
    if (n == 15) n = tp_uvarint(t);
    *size = n;
    return 1;
}
static void tp_skip(tp_t* t, int type) {
    switch (type) {
        case 1: case 2: break;
        case 3: t->p += 1; break;
        case 4: case 5: case 6: (void)tp_uvarint(t); break;
        case 7: t->p += 8; break;
        case 8: {
            int64_t n = tp_uvarint(t);
            t->p += n;
            break;
        }
        case 9: case 10: {
            int64_t n;
            int et;
            tp_list(t, &n, &et);
            for (int64_t i = 0; i < n; i++) tp_skip(t, et);
            break;
        }
        case 11: {
            int64_t n = tp_uvarint(t);
            if (n > 0 && t->p < t->end) {
                unsigned char kv = *t->p++;
                int kt = (kv >> 4) & 0x0f, vt = kv & 0x0f;
                for (int64_t i = 0; i < n; i++) {
                    tp_skip(t, kt);
                    tp_skip(t, vt);
                }
            }
            break;
        }
        case 12: tp_skip_struct(t); break;
        default: break;
    }
    if (t->p > t->end) t->p = t->end;
}

// ---- parquet metadata -------------------------------------------------------
#define ZT_PQ_MAX_COLS 64
typedef struct {
    char name[96];
    int phys_type;        // 0 BOOLEAN 1 INT32 2 INT64 3 INT96 4 FLOAT 5 DOUBLE 6 BYTE_ARRAY
    int codec;            // 0 UNCOMPRESSED 1 SNAPPY 2 GZIP 3 LZO 4 BROTLI 5 LZ4 6 ZSTD
    int64_t num_values;
    int64_t total_compressed;
    int64_t data_page_offset;
    int64_t dict_page_offset;   // 0 when absent
} zt_pq_col;

typedef struct {
    int64_t num_rows;
    int64_t num_row_groups;
    int ncols;
    zt_pq_col cols[ZT_PQ_MAX_COLS];
} zt_pq_meta;

static void tp_read_binary(tp_t* t, char* out, int cap) {
    int64_t n = tp_uvarint(t);
    int k = 0;
    for (int64_t i = 0; i < n && k < cap - 1; i++) {
        if (t->p + i < t->end) out[k++] = (char)t->p[i];
    }
    out[k] = 0;
    t->p += n;
    if (t->p > t->end) t->p = t->end;
}

// ColumnMetaData (ColumnChunk field 3)
static void pq_read_colmeta(tp_t* t, zt_pq_col* c) {
    int fid, ftype;
    int64_t saved = t->last_fid;
    t->last_fid = 0;
    while (tp_field(t, &fid, &ftype)) {
        switch (fid) {
            case 1: c->phys_type = (int)tp_zigzag(t); break;
            case 3: {  // path_in_schema: list<string>, keep the LAST element
                int64_t n;
                int et;
                tp_list(t, &n, &et);
                for (int64_t i = 0; i < n; i++) tp_read_binary(t, c->name, sizeof c->name);
                break;
            }
            case 4: c->codec = (int)tp_zigzag(t); break;
            case 5: c->num_values = tp_zigzag(t); break;
            case 6: (void)tp_zigzag(t); break;
            case 7: c->total_compressed = tp_zigzag(t); break;
            case 9: c->data_page_offset = tp_zigzag(t); break;
            case 11: c->dict_page_offset = tp_zigzag(t); break;
            default: tp_skip(t, ftype); break;
        }
    }
    t->last_fid = saved;
}

// ColumnChunk
static void pq_read_colchunk(tp_t* t, zt_pq_col* c) {
    int fid, ftype;
    int64_t saved = t->last_fid;
    t->last_fid = 0;
    while (tp_field(t, &fid, &ftype)) {
        if (fid == 3 && ftype == 12) {
            pq_read_colmeta(t, c);
        } else {
            tp_skip(t, ftype);
        }
    }
    t->last_fid = saved;
}

// FileMetaData: 2 schema, 3 num_rows, 4 row_groups
static void pq_read_footer(tp_t* t, zt_pq_meta* m) {
    int fid, ftype;
    while (tp_field(t, &fid, &ftype)) {
        if (fid == 3) {
            m->num_rows = tp_zigzag(t);
        } else if (fid == 4 && ftype == 9) {
            int64_t nrg;
            int et;
            tp_list(t, &nrg, &et);
            m->num_row_groups = nrg;
            for (int64_t g = 0; g < nrg; g++) {
                int gid, gtype;
                int64_t saved = t->last_fid;
                t->last_fid = 0;
                while (tp_field(t, &gid, &gtype)) {
                    if (gid == 1 && gtype == 9) {
                        int64_t ncol;
                        int cet;
                        tp_list(t, &ncol, &cet);
                        for (int64_t i = 0; i < ncol; i++) {
                            if (m->ncols < ZT_PQ_MAX_COLS) {
                                zt_pq_col* c = &m->cols[m->ncols];
                                memset(c, 0, sizeof *c);
                                pq_read_colchunk(t, c);
                                m->ncols++;
                            } else {
                                tp_skip(t, cet);
                            }
                        }
                    } else {
                        tp_skip(t, gtype);
                    }
                }
                t->last_fid = saved;
            }
        } else {
            tp_skip(t, ftype);
        }
    }
}

// Reads the footer metadata. Returns 1 on success.
int zt_parquet_meta(const char* path, zt_pq_meta* m) {
    FILE* f = fopen(path, "rb");
    if (!f) return 0;
    if (fseek(f, 0, SEEK_END) != 0) { fclose(f); return 0; }
    long len = ftell(f);
    if (len < 12) { fclose(f); return 0; }
    unsigned char trailer[8];
    if (fseek(f, len - 8, SEEK_SET) != 0) { fclose(f); return 0; }
    if (fread(trailer, 1, 8, f) != 8) { fclose(f); return 0; }
    if (memcmp(trailer + 4, "PAR1", 4) != 0) { fclose(f); return 0; }
    int64_t flen = (int64_t)trailer[0] | ((int64_t)trailer[1] << 8) |
                   ((int64_t)trailer[2] << 16) | ((int64_t)trailer[3] << 24);
    if (flen <= 0 || flen > len - 12) { fclose(f); return 0; }
    unsigned char* buf = (unsigned char*)malloc((size_t)flen);
    if (!buf) { fclose(f); return 0; }
    if (fseek(f, len - 8 - (long)flen, SEEK_SET) != 0) { free(buf); fclose(f); return 0; }
    size_t got = fread(buf, 1, (size_t)flen, f);
    fclose(f);
    if (got != (size_t)flen) { free(buf); return 0; }
    memset(m, 0, sizeof *m);
    tp_t t = {buf, buf + flen, 0};
    pq_read_footer(&t, m);
    free(buf);
    return m->ncols > 0;
}

// ============================================================================
// Page decoding
//
// PageHeader (thrift): 1 type, 2 uncompressed_page_size, 3 compressed_page_size,
// 5 data_page_header{1 num_values, 2 encoding, 3 def_level_encoding,
// 4 rep_level_encoding}, 7 dictionary_page_header{1 num_values, 2 encoding}.
// The page BODY is compressed with the column's codec (SNAPPY here).
//
// DataPageV1 body = [rep levels] [def levels] [values]; each level section is a
// 4-byte LE length followed by RLE/bit-packed data (bit width = ceil(log2(max+1))).
// ============================================================================
#define ZT_PQ_PAGE_DATA 0
#define ZT_PQ_PAGE_INDEX 1
#define ZT_PQ_PAGE_DICT 2
#define ZT_PQ_PAGE_DATA_V2 3

typedef struct {
    int type;
    int num_values;
    int encoding;
    int def_encoding;
    int rep_encoding;
    int dict_num_values;
    int64_t uncompressed_size;
    int64_t compressed_size;
    int64_t header_size;   // bytes consumed by the header
} zt_pq_page;

static void pq_read_data_page_header(tp_t* t, zt_pq_page* p) {
    int fid, ftype;
    int64_t saved = t->last_fid;
    t->last_fid = 0;
    while (tp_field(t, &fid, &ftype)) {
        switch (fid) {
            case 1: p->num_values = (int)tp_zigzag(t); break;
            case 2: p->encoding = (int)tp_zigzag(t); break;
            case 3: p->def_encoding = (int)tp_zigzag(t); break;
            case 4: p->rep_encoding = (int)tp_zigzag(t); break;
            default: tp_skip(t, ftype); break;
        }
    }
    t->last_fid = saved;
}
static void pq_read_dict_page_header(tp_t* t, zt_pq_page* p) {
    int fid, ftype;
    int64_t saved = t->last_fid;
    t->last_fid = 0;
    while (tp_field(t, &fid, &ftype)) {
        switch (fid) {
            case 1: p->dict_num_values = (int)tp_zigzag(t); break;
            case 2: p->encoding = (int)tp_zigzag(t); break;
            default: tp_skip(t, ftype); break;
        }
    }
    t->last_fid = saved;
}
static int pq_read_page_header(tp_t* t, zt_pq_page* p) {
    memset(p, 0, sizeof *p);
    const unsigned char* start = t->p;
    t->last_fid = 0;
    int fid, ftype, ok = 0;
    while (tp_field(t, &fid, &ftype)) {
        switch (fid) {
            case 1: p->type = (int)tp_zigzag(t); ok = 1; break;
            case 2: p->uncompressed_size = tp_zigzag(t); break;
            case 3: p->compressed_size = tp_zigzag(t); break;
            case 5: pq_read_data_page_header(t, p); break;
            case 7: pq_read_dict_page_header(t, p); break;
            case 8: pq_read_data_page_header(t, p); break;
            default: tp_skip(t, ftype); break;
        }
    }
    p->header_size = (int64_t)(t->p - start);
    return ok;
}

static int zt_bit_width(int64_t maxv) {
    int w = 0;
    while ((int64_t)1 << w <= maxv) w++;
    return w;
}

// Decompresses `comp` into `out` (size `uncomp`), honouring the codec.
// Returns the number of bytes produced, or -1.
int64_t zt_pq_decompress(int codec, const char* comp, int64_t comp_len, char* out,
                         int64_t out_cap) {
    if (codec == 0) {
        if (comp_len > out_cap) return -1;
        memcpy(out, comp, (size_t)comp_len);
        return comp_len;
    }
    if (codec == 1) return zt_snappy_uncompress(comp, comp_len, out, out_cap);
    return -1;  // gzip/zstd/... not needed for this project's cache files
}

// ============================================================================
// Column value extraction. Returns a malloc'd array of int64 slots (one per
// value): doubles are bit-cast, INT64 timestamps stay as-is, BYTE_ARRAY becomes
// a pointer to a NUL-terminated copy. `max_def` > 0 means the column is
// OPTIONAL, so each value is preceded by a definition level (0 = null).
// ============================================================================
int64_t zt_pq_read_column(const char* path, const zt_pq_col* col, int max_def,
                          int64_t* out, int64_t out_cap) {
    FILE* f = fopen(path, "rb");
    if (!f) return -1;
    if (fseek(f, 0, SEEK_END) != 0) { fclose(f); return -1; }
    long len = ftell(f);
    // Start from the dictionary page when present (it precedes the data pages).
    int64_t off = col->dict_page_offset ? col->dict_page_offset : col->data_page_offset;
    int64_t remaining = col->total_compressed;
    int64_t produced = 0;
    // Dictionary values (only for BYTE_ARRAY / numerics; stored as raw slots).
    int64_t* dict = NULL;
    int64_t dict_n = 0;
    int64_t* dict_idx = NULL;
    while (remaining > 0 && off < len && produced < out_cap) {
        if (fseek(f, (long)off, SEEK_SET) != 0) break;
        unsigned char hdr[4096];
        size_t got = fread(hdr, 1, sizeof hdr, f);
        if (got == 0) break;
        tp_t t = {hdr, hdr + got, 0};
        zt_pq_page pg;
        if (!pq_read_page_header(&t, &pg)) break;
        int64_t body_off = off + pg.header_size;
        int64_t csize = pg.compressed_size;
        if (csize <= 0 || body_off + csize > len) break;
        char* comp = (char*)malloc((size_t)csize);
        if (!comp) break;
        if (fseek(f, (long)body_off, SEEK_SET) != 0) { free(comp); break; }
        if (fread(comp, 1, (size_t)csize, f) != (size_t)csize) { free(comp); break; }
        char* body = (char*)malloc((size_t)pg.uncompressed_size + 8);
        if (!body) { free(comp); break; }
        int64_t blen = zt_pq_decompress(col->codec, comp, csize, body,
                                        pg.uncompressed_size);
        free(comp);
        if (blen < 0) { free(body); break; }
        int64_t pos = 0;
        if (pg.type == ZT_PQ_PAGE_DICT) {
            int64_t dn = pg.dict_num_values;
            if (col->phys_type == 5) {          // DOUBLE
                if (dict) free(dict);
                dict = (int64_t*)malloc(sizeof(int64_t) * (size_t)(dn > 0 ? dn : 1));
                dict_n = dn;
                for (int64_t i = 0; i < dn; i++) {
                    double d;
                    memcpy(&d, body + pos, 8);
                    pos += 8;
                    int64_t bits;
                    memcpy(&bits, &d, 8);
                    dict[i] = bits;
                }
            } else if (col->phys_type == 6) {   // BYTE_ARRAY
                if (dict) free(dict);
                dict = (int64_t*)malloc(sizeof(int64_t) * (size_t)(dn > 0 ? dn : 1));
                dict_n = dn;
                for (int64_t i = 0; i < dn; i++) {
                    int32_t n;
                    memcpy(&n, body + pos, 4);
                    pos += 4;
                    char* s = (char*)malloc((size_t)n + 1);
                    memcpy(s, body + pos, (size_t)n);
                    s[n] = 0;
                    pos += n;
                    dict[i] = (int64_t)s;
                }
            } else {                            // INT64 etc.
                if (dict) free(dict);
                dict = (int64_t*)malloc(sizeof(int64_t) * (size_t)(dn > 0 ? dn : 1));
                dict_n = dn;
                for (int64_t i = 0; i < dn; i++) {
                    int64_t v;
                    memcpy(&v, body + pos, 8);
                    pos += 8;
                    dict[i] = v;
                }
            }
        } else if (pg.type == ZT_PQ_PAGE_DATA || pg.type == ZT_PQ_PAGE_DATA_V2) {
            int64_t n = pg.num_values;
            int64_t defs[4096];
            int64_t ndef = 0;
            if (max_def > 0) {
                if (pg.type == ZT_PQ_PAGE_DATA) {
                    int32_t lvl_len;
                    memcpy(&lvl_len, body + pos, 4);
                    pos += 4;
                    ndef = zt_rle_bitpack_decode(body + pos, lvl_len, zt_bit_width(max_def),
                                                 defs, 4096);
                    pos += lvl_len;
                } else {
                    // DataPageV2: levels are uncompressed and length-prefixed in
                    // the header; not produced by this project's writer.
                    ndef = -1;
                }
                if (ndef < 0) { free(body); break; }
            }
            if (pg.encoding == 0) {              // PLAIN
                for (int64_t i = 0; i < n && produced < out_cap; i++) {
                    if (max_def > 0 && i < ndef && defs[i] < max_def) {
                        out[produced++] = 0;     // null
                        continue;
                    }
                    if (col->phys_type == 5) {
                        double d;
                        if (pos + 8 > blen) break;
                        memcpy(&d, body + pos, 8);
                        pos += 8;
                        memcpy(&out[produced++], &d, 8);
                    } else if (col->phys_type == 6) {
                        int32_t sl;
                        if (pos + 4 > blen) break;
                        memcpy(&sl, body + pos, 4);
                        pos += 4;
                        char* s = (char*)malloc((size_t)sl + 1);
                        memcpy(s, body + pos, (size_t)sl);
                        s[sl] = 0;
                        pos += sl;
                        out[produced++] = (int64_t)s;
                    } else {
                        int64_t v;
                        if (pos + 8 > blen) break;
                        memcpy(&v, body + pos, 8);
                        pos += 8;
                        out[produced++] = v;
                    }
                }
            } else if (pg.encoding == 8 || pg.encoding == 2) {  // RLE_DICTIONARY
                int bitw = (unsigned char)body[pos++];
                int64_t idx[8192];
                int64_t ni = zt_rle_bitpack_decode(body + pos, blen - pos, bitw, idx, 8192);
                if (ni < 0) { free(body); break; }
                for (int64_t i = 0; i < n && produced < out_cap; i++) {
                    if (max_def > 0 && i < ndef && defs[i] < max_def) {
                        out[produced++] = 0;
                        continue;
                    }
                    if (i >= ni || idx[i] >= dict_n || !dict) break;
                    out[produced++] = dict[idx[i]];
                }
            }
        }
        free(body);
        off = body_off + csize;
        remaining -= pg.header_size + csize;
        if (pg.type == ZT_PQ_PAGE_INDEX) break;
    }
    fclose(f);
    if (dict) free(dict);
    if (dict_idx) free(dict_idx);
    return produced;
}

// ============================================================================
// SNAPPY decoder — parquet's default compression (pandas `to_parquet` writes
// SNAPPY + PLAIN/RLE_DICTIONARY). Self-contained so `pd.read_parquet` can be
// implemented without an external library.
//
// Format: varint(uncompressed length), then elements. Each element starts with a
// tag byte whose low 2 bits select the kind:
//   0 literal : length-1 in bits 2..7; 60..63 mean the length-1 is stored in the
//               next 1..4 little-endian bytes.
//   1 copy    : length 4 + bits 2..4, offset = (bits 5..7)<<8 | next byte.
//   2 copy    : length 1 + bits 2..7, offset = LE16.
//   3 copy    : length 1 + bits 2..7, offset = LE32.
// ============================================================================
static int64_t zt_snappy_varint(const unsigned char* p, int64_t n, int64_t* consumed) {
    int64_t v = 0, shift = 0, i = 0;
    while (i < n && shift < 64) {
        unsigned char b = p[i++];
        v |= (int64_t)(b & 0x7f) << shift;
        shift += 7;
        if (!(b & 0x80)) break;
    }
    if (consumed) *consumed = i;
    return v;
}

// Returns the uncompressed size written, or -1 on malformed input / overflow.
int64_t zt_snappy_uncompress(const char* in, int64_t in_len, char* out, int64_t out_cap) {
    if (!in || in_len <= 0 || !out || out_cap <= 0) return -1;
    const unsigned char* src = (const unsigned char*)in;
    int64_t hdr = 0;
    int64_t want = zt_snappy_varint(src, in_len, &hdr);
    if (want < 0 || want > out_cap) return -1;
    int64_t i = hdr, n = 0;
    unsigned char* dst = (unsigned char*)out;
    while (i < in_len) {
        unsigned char tag = src[i++];
        int kind = tag & 3;
        if (kind == 0) {
            int64_t len = (tag >> 2) + 1;
            if (len > 60) {
                int64_t extra = len - 60;
                if (i + extra > in_len) return -1;
                int64_t v = 0;
                for (int64_t k = 0; k < extra; k++) v |= (int64_t)src[i + k] << (8 * k);
                i += extra;
                len = v + 1;
            }
            if (i + len > in_len || n + len > want) return -1;
            memcpy(dst + n, src + i, (size_t)len);
            i += len;
            n += len;
        } else {
            int64_t len, offset;
            if (kind == 1) {
                if (i + 1 > in_len) return -1;
                len = 4 + ((tag >> 2) & 7);
                offset = ((int64_t)(tag >> 5) << 8) | src[i];
                i += 1;
            } else if (kind == 2) {
                if (i + 2 > in_len) return -1;
                len = 1 + (tag >> 2);
                offset = (int64_t)src[i] | ((int64_t)src[i + 1] << 8);
                i += 2;
            } else {
                if (i + 4 > in_len) return -1;
                len = 1 + (tag >> 2);
                offset = (int64_t)src[i] | ((int64_t)src[i + 1] << 8) |
                         ((int64_t)src[i + 2] << 16) | ((int64_t)src[i + 3] << 24);
                i += 4;
            }
            if (offset <= 0 || offset > n || n + len > want) return -1;
            // Byte-by-byte: `offset` may be smaller than `len` (overlapping copy,
            // which snappy allows and exploits for runs).
            for (int64_t k = 0; k < len; k++) {
                dst[n] = dst[n - offset];
                n++;
            }
        }
    }
    return (n == want) ? n : -1;
}

// ============================================================================
// Parquet RLE / bit-packed hybrid decoder — used for definition levels and for
// dictionary indices (RLE_DICTIONARY encoded pages). Stream layout:
//   varint header; header&1 -> bit-packed run of (header>>1)*8 values,
//   otherwise an RLE run of (header>>1) copies of one value stored in
//   ceil(bit_width/8) little-endian bytes. Bit-packed values are packed
//   LSB-first, continuing across byte boundaries.
// Returns the number of values written, or -1 when the input is malformed.
// ============================================================================
int64_t zt_rle_bitpack_decode(const char* in, int64_t in_len, int bit_width,
                              int64_t* out, int64_t out_cap) {
    if (!in || in_len <= 0 || bit_width < 0 || bit_width > 32 || !out) return -1;
    const unsigned char* src = (const unsigned char*)in;
    int64_t i = 0, n = 0;
    const int byte_width = (bit_width + 7) / 8;
    while (i < in_len) {
        int64_t hdr = 0, sh = 0;
        while (i < in_len && sh < 64) {
            unsigned char b = src[i++];
            hdr |= (int64_t)(b & 0x7f) << sh;
            sh += 7;
            if (!(b & 0x80)) break;
        }
        if (hdr & 1) {
            int64_t count = (hdr >> 1) * 8;
            if (bit_width == 0) {
                for (int64_t k = 0; k < count && n < out_cap; k++) out[n++] = 0;
                if (n >= out_cap) return n;
                continue;
            }
            int64_t bits = count * bit_width;
            int64_t bytes = (bits + 7) / 8;
            if (i + bytes > in_len) return -1;
            int64_t bitpos = 0;
            for (int64_t k = 0; k < count; k++) {
                int64_t v = 0;
                for (int b = 0; b < bit_width; b++) {
                    int64_t bp = bitpos + b;
                    int64_t byte = i + bp / 8;
                    int bit = (int)(bp % 8);
                    if (bp / 8 >= bytes) return -1;
                    if (src[byte] & (1u << bit)) v |= (int64_t)1 << b;
                }
                bitpos += bit_width;
                if (n < out_cap) out[n++] = v;
                if (n >= out_cap) { i += bytes; goto done; }
            }
            i += bytes;
        } else {
            int64_t count = hdr >> 1;
            if (i + byte_width > in_len) return -1;
            int64_t v = 0;
            for (int b = 0; b < byte_width; b++) v |= (int64_t)src[i + b] << (8 * b);
            i += byte_width;
            for (int64_t k = 0; k < count; k++) {
                if (n >= out_cap) goto done;
                out[n++] = v;
            }
        }
    }
done:
    return n;
}