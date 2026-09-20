#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef struct { char name[96]; int phys_type; int codec; int64_t num_values; int64_t total_compressed; int64_t data_page_offset; int64_t dict_page_offset; } zt_pq_col;
typedef struct { int64_t num_rows; int64_t num_row_groups; int ncols; zt_pq_col cols[64]; } zt_pq_meta;
int zt_parquet_meta(const char*, zt_pq_meta*);
int64_t zt_pq_read_column(const char*, const zt_pq_col*, int, int64_t*, int64_t);
int main(int argc, char** argv) {
    zt_pq_meta m;
    if (!zt_parquet_meta(argv[1], &m)) { printf("meta fail\n"); return 1; }
    for (int i = 0; i < m.ncols; i++) {
        if (strcmp(m.cols[i].name, argv[2]) != 0) continue;
        static int64_t out[4096];
        int64_t n = zt_pq_read_column(argv[1], &m.cols[i], 1, out, 4096);
        printf("col=%s n=%lld\n", argv[2], (long long)n);
        if (m.cols[i].phys_type == 5) {
            for (int k = 0; k < 5 && k < n; k++) { double d; memcpy(&d, &out[k], 8); printf("  %d %.4f\n", k, d); }
        } else if (m.cols[i].phys_type == 6) {
            for (int k = 0; k < 5 && k < n; k++) printf("  %d %s\n", k, (char*)out[k]);
        } else {
            for (int k = 0; k < 5 && k < n; k++) printf("  %d %lld\n", k, (long long)out[k]);
        }
        return 0;
    }
    printf("col not found\n");
    return 1;
}
