// Parses a real parquet file's footer with runtime/parquet_min.c and prints the
// columns. Run:
//   clang -O1 tests/runtime/parquet_meta_test.c runtime/parquet_min.c -o /tmp/pqmeta && \
//   /tmp/pqmeta ~/source/quant/REasyQuant/data/stocks/000300_XSHG.parquet
#include <stdint.h>
#include <stdio.h>
typedef struct {
    char name[96];
    int phys_type;
    int codec;
    int64_t num_values;
    int64_t total_compressed;
    int64_t data_page_offset;
    int64_t dict_page_offset;
} zt_pq_col;
typedef struct {
    int64_t num_rows;
    int64_t num_row_groups;
    int ncols;
    zt_pq_col cols[64];
} zt_pq_meta;
int zt_parquet_meta(const char* path, zt_pq_meta* m);
int main(int argc, char** argv) {
    if (argc < 2) { printf("usage: %s file.parquet\n", argv[0]); return 2; }
    zt_pq_meta m;
    if (!zt_parquet_meta(argv[1], &m)) { printf("PARSE FAILED\n"); return 1; }
    printf("rows=%lld rowgroups=%lld cols=%d\n", (long long)m.num_rows,
           (long long)m.num_row_groups, m.ncols);
    for (int i = 0; i < m.ncols; i++) {
        zt_pq_col* c = &m.cols[i];
        printf("  %-12s type=%d codec=%d n=%lld csize=%lld data_off=%lld dict_off=%lld\n",
               c->name, c->phys_type, c->codec, (long long)c->num_values,
               (long long)c->total_compressed, (long long)c->data_page_offset,
               (long long)c->dict_page_offset);
    }
    return 0;
}
