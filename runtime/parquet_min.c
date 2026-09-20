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
