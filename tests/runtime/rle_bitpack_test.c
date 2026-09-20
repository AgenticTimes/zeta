#include <stdint.h>
#include <stdio.h>
int64_t zt_rle_bitpack_decode(const char*, int64_t, int, int64_t*, int64_t);
int main(void) {
    // bit_width=1: RLE run of 8 ones -> header=(8<<1)=16, value byte 1
    unsigned char a[] = {16, 1};
    int64_t out[64];
    int64_t n = zt_rle_bitpack_decode((const char*)a, sizeof a, 1, out, 64);
    printf("rle n=%lld all1=%d\n", (long long)n, n == 8 && out[0] == 1 && out[7] == 1);
    // bit_width=2: bit-packed run of 8 values 0,1,2,3,0,1,2,3 -> header=(1<<1)|1=3
    // packed LSB-first: 00 01 10 11 ... -> bytes 0xE4, 0xE4
    unsigned char b[] = {3, 0xE4, 0xE4};
    int64_t o2[16];
    int64_t n2 = zt_rle_bitpack_decode((const char*)b, sizeof b, 2, o2, 16);
    printf("bp n=%lld v=%lld,%lld,%lld,%lld ok=%d\n", (long long)n2,
           (long long)o2[0], (long long)o2[1], (long long)o2[2], (long long)o2[3],
           n2 == 8 && o2[0]==0 && o2[1]==1 && o2[2]==2 && o2[3]==3 && o2[4]==0);
    // bit_width=0 RLE: dictionary with a single entry
    unsigned char c[] = {6};
    int64_t o3[8];
    int64_t n3 = zt_rle_bitpack_decode((const char*)c, sizeof c, 0, o3, 8);
    printf("bw0 n=%lld v0=%lld ok=%d\n", (long long)n3, (long long)o3[0], n3 == 3 && o3[0] == 0);
    return 0;
}
