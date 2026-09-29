// 批次 641（旁路 cleanup）：i128 值表示＋运算族的 C 判据（22 条 numeric
// mismatch 的代表运算 vs CPython oracle）。编译行（仓根执行）：
//   bash tools/build_runtime.sh
//   clang -O1 -Iruntime tests/big_test.c zeta_runtime_c.o tokio_runtime.o \
//     -L/opt/homebrew/lib -lgc -Wl,-rpath,/opt/homebrew/lib -o /tmp/big_test
// 值表示＝16 字节 GC 块 [lo|hi] 句柄（640 施工序第①步）；gen 接入＝642+。
#include <stdio.h>
#include <string.h>
#include <stdint.h>
int64_t zeta_big_new(int64_t, int64_t);
int64_t zeta_big_from_i64(int64_t);
int64_t zeta_big_shl(int64_t, int64_t);
int64_t zeta_big_mul(int64_t, int64_t);
int64_t zeta_big_or(int64_t, int64_t);
int64_t zeta_big_neg(int64_t);
int64_t zeta_big_add(int64_t, int64_t);
int64_t zeta_big_cmp(int64_t, int64_t);
int64_t zeta_big_to_string(int64_t);

static int fails = 0;
static void chk(const char* what, int64_t str, const char* want) {
    const char* g = (const char*)str;
    if (strcmp(g, want) != 0) { printf("FAIL %s: got %s want %s\n", what, g, want); fails++; }
    else printf("ok %s = %s\n", what, g);
}
int main() {
    chk("-35442324074144 << 62",
        zeta_big_to_string(zeta_big_shl(zeta_big_from_i64(-35442324074144LL), 62)),
        "-163448870393302300717529144754176");
    chk("2**100", zeta_big_to_string(zeta_big_shl(zeta_big_from_i64(1), 100)),
        "1267650600228229401496703205376");
    chk("1<<64", zeta_big_to_string(zeta_big_shl(zeta_big_from_i64(1), 64)),
        "18446744073709551616");
    chk("3037000499^2",
        zeta_big_to_string(zeta_big_mul(zeta_big_from_i64(3037000499LL), zeta_big_from_i64(3037000499LL))),
        "9223372030926249001");
    {
        int64_t sh = zeta_big_shl(zeta_big_from_i64(-35442324074144LL), 62);
        int64_t orv = zeta_big_or(zeta_big_from_i64(61180469571415LL), sh);
        chk("61180469571415 | (-B<<62)", zeta_big_to_string(orv), "-163448870393302300656348675182761");
        int64_t c = zeta_big_cmp(orv, zeta_big_from_i64(56012436744467LL));
        printf("%s cmp(or<rhs)=%lld (want -1)\n", c == -1 ? "ok" : "FAIL", (long long)c);
        if (c != -1) fails++;
    }
    chk("add cross-sign", zeta_big_to_string(zeta_big_add(
        zeta_big_from_i64(-35442324074144LL), zeta_big_from_i64(61180469571415LL))),
        "25738145497271");
    chk("neg", zeta_big_to_string(zeta_big_neg(zeta_big_from_i64(-7201409416254496LL))),
        "7201409416254496");
    chk("to_string zero", zeta_big_to_string(zeta_big_from_i64(0)), "0");
    chk("-2**100", zeta_big_to_string(zeta_big_neg(zeta_big_shl(zeta_big_from_i64(1), 100))),
        "-1267650600228229401496703205376");
    chk("1<<103", zeta_big_to_string(zeta_big_shl(zeta_big_from_i64(1), 103)),
        "10141204801825835211973625643008");
    printf(fails ? "RESULT: %d FAILS\n" : "RESULT: ALL OK\n", fails);
    return fails != 0;
}
