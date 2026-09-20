// Minimal SNAPPY decoder check (parquet's default compression).
// Run:  cd <repo> && clang -O1 tests/runtime/snappy_test.c \
//          <(sed -n '/static int64_t zt_snappy_varint/,/^}/p;/^int64_t zt_snappy_uncompress/,/^}/p' runtime/py_additions.c) -o /tmp/snt && /tmp/snt
// (or simply compile the two functions together with this file)
#include <stdint.h>
#include <string.h>
#include <stdio.h>
int64_t zt_snappy_uncompress(const char*, int64_t, char*, int64_t);
int main(void) {
    // "abcabcabc" (9) : literal "abc" then copy len=6 offset=3
    unsigned char comp[] = {9, 0x08, 0x61, 0x62, 0x63, 0x09, 3};
    char out[64]; memset(out, 0, sizeof out);
    int64_t n = zt_snappy_uncompress((const char*)comp, sizeof comp, out, sizeof out);
    printf("n=%lld out=%.*s ok=%d\n", (long long)n, (int)(n > 0 ? n : 0), out, n == 9 && strncmp(out, "abcabcabc", 9) == 0);
    // 4-byte literal
    unsigned char c2[] = {4, 0x0c, 'w', 'x', 'y', 'z'};
    char o2[16]; int64_t n2 = zt_snappy_uncompress((const char*)c2, sizeof c2, o2, sizeof o2);
    printf("n2=%lld ok=%d\n", (long long)n2, n2 == 4 && strncmp(o2, "wxyz", 4) == 0);
    // malformed: copy with offset 0 must fail
    unsigned char c3[] = {5, 0x0a, 0, 0};
    char o3[8]; printf("bad=%lld\n", (long long)zt_snappy_uncompress((const char*)c3, sizeof c3, o3, sizeof o3));
    return 0;
}
