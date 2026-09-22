#!/usr/bin/env bash
# tools/cli_semantics_check.sh —— "要 dump 不等于要运行"（批次 344 / backlog §2-A #28）
#
# 缺陷本体（修复前实测）：`src/main.rs` 里决定"要不要在编译器进程里 JIT 并执行被编译
# 程序"的**只有一个条件** —— 有没有给 `-o`。`if let Some(out) = output { 编译 } else {
# finalize_and_jit + main.call() }`。于是所有"只读输出"标志在无 -o 时都会把程序跑一遍：
#   zetac --dump-mir x.z / --emit-llvm x.z / --report-stubs / --report-untyped /
#   ZETA_DUMP_IR=1 zetac x.z   —— 全都执行 x，而 --help 写的是 "instead of binary"。
# 代价分两档，都实测过（roadmap 批次 344）：
#   小用例（official）差 0–1 ms —— 程序本身什么都不干；
#   真语料（quant strategies，3 文件 × 3 轮）ir 口径 597.5 → 388.6 ms，**−35%**，
#   且修复前 9/9 轮都是 fail:rc1（执行程序撞 E4016 桩 → exit(1)），修复后 9/9 轮 ok。
#   ⇒ 批次 313 那份 `ir` 基线（41,655 ms/12 文件）与本批之后**不同口径**，不可直比。
#
# 判据形状（三翼，缺一翼锁不住）：
#   阳性翼 —— 裸跑必须"被执行"。这一翼是探测器自己的对照：`ran()` 只认运行路径自己打的
#             `^Result: `（main.rs:930）。不能用"stdout 里有没有程序的输出"——转储文本里
#             就含源码与函数名，本批第一版因此把 17 行 MIR 读成"执行了 17 次"。
#   收窄翼 —— 五个只读入口（四标志 + env 旋钮）与它们的组合：被执行=0，**且**各自 rc=0
#             （否则"没执行"可能只是提前报错，判据就成了空的）。
#   不变翼 —— `-o` 那条路逐字不变：不执行、有产物；`--dump-mir` 的转储与 `-o` 无关
#             （本批只收窄 else 分支，不许动转储本身）。
#
# 期望值全部来自修复后实测，不是推测。顺带如实记录一条**本批不修**的 CLI 事实：
# `--emit-llvm x -o y` 会忽略 --emit-llvm，y 是链接好的可执行文件（不是 IR），见脚本尾。
#
# 用法：./tools/cli_semantics_check.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
[[ -x "$ZETAC" ]] || { echo "building zetac..." >&2; cargo build --release -p zetac -q; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
rc=0
SRC="$ROOT/tests/unit-tests/test_minimal.z"
[[ -f "$SRC" ]] || { echo "夹具缺失：$SRC —— 判据不能建在不存在的文件上" >&2; exit 9; }

# 被执行了吗 = 运行路径自己打的 `^Result: `，别的一个字都不信
ran() { "$@" 2>/dev/null | grep -c '^Result: '; }
want() { if [[ "$2" == "$3" ]]; then echo "  ok   $1 → ${3:-0}"; else
    echo "  FAIL $1 → 期望 ${2:-0}，实得 ${3:-0}"; rc=1; fi; }
rc0() { "$@" >/dev/null 2>&1; local n=$?; echo "$n"; }

echo "== 阳性对照：裸跑必须在执行（探测器没信号，下面的 0 就全是假的）"
want "zetac <f> 裸跑 → 执行" 1 "$(ran "$ZETAC" "$SRC")"

echo "== 收窄：只读入口一个都不许执行（且各自 rc=0）"
want "--dump-mir 不执行"   0 "$(ran "$ZETAC" --dump-mir "$SRC")"
want "--emit-llvm 不执行"  0 "$(ran "$ZETAC" --emit-llvm "$SRC")"
want "--report-untyped 不执行" 0 "$(ran "$ZETAC" --report-untyped "$SRC")"
want "--report-stubs 不执行"   0 "$(ran "$ZETAC" --report-stubs "$SRC")"
want "ZETA_DUMP_IR=1 不执行"   0 "$(ran env ZETA_DUMP_IR=1 "$ZETAC" "$SRC")"
want "四个只读标志同给也不执行" 0 "$(ran "$ZETAC" --dump-mir --emit-llvm --report-stubs --report-untyped "$SRC")"

echo "== 反空转：这些 0 不是因为提前失败或转储没出来"
want "--dump-mir rc=0" 0 "$(rc0 "$ZETAC" --dump-mir "$SRC")"
want "--emit-llvm rc=0" 0 "$(rc0 "$ZETAC" --emit-llvm "$SRC")"
want "--report-stubs rc=0" 0 "$(rc0 "$ZETAC" --report-stubs "$SRC")"
want "--report-untyped rc=0" 0 "$(rc0 "$ZETAC" --report-untyped "$SRC")"
want "--dump-mir 有转储" 1 "$("$ZETAC" --dump-mir "$SRC" 2>/dev/null | grep -c '^== MIR ' | sed 's/^[1-9].*/1/')"
want "--emit-llvm 有 IR" 1 "$("$ZETAC" --emit-llvm "$SRC" 2>&1 1>/dev/null | grep -c '; ModuleID' | sed 's/^[1-9].*/1/')"

echo "== 不变：-o 那条路（编译/链接）逐字照旧"
want "<f> -o out 不执行" 0 "$(ran "$ZETAC" "$SRC" -o "$TMP/a.out")"
if [[ -x "$TMP/a.out" ]]; then echo "  ok   -o 产物仍是可执行文件"; else
    echo "  FAIL -o 没产物（或没执行位）"; rc=1; fi
want "--dump-mir -o 不执行" 0 "$(ran "$ZETAC" --dump-mir "$SRC" -o "$TMP/b.out")"
want "--emit-llvm -o 不执行" 0 "$(ran "$ZETAC" --emit-llvm "$SRC" -o "$TMP/c.out")"

strip() { grep -vE '^(Result: |Compiled to )'; }
"$ZETAC" --dump-mir "$SRC" 2>/dev/null | strip > "$TMP/mir_no.txt"
"$ZETAC" --dump-mir "$SRC" -o "$TMP/d.out" 2>/dev/null | strip > "$TMP/mir_with_o.txt"
if cmp -s "$TMP/mir_no.txt" "$TMP/mir_with_o.txt"; then
    echo "  ok   MIR 转储与 -o 无关（$(wc -l < "$TMP/mir_no.txt" | tr -d ' ') 行逐字相同）"
else
    echo "  FAIL MIR 转储随 -o 变了：$(diff "$TMP/mir_no.txt" "$TMP/mir_with_o.txt" | head -4 | tr '\n' ' ')"
    rc=1
fi

echo "  注   --emit-llvm <f> -o g 今天会忽略 --emit-llvm：g 是链接好的可执行文件而非 IR"
echo "       （$(file -b "$TMP/c.out" 2>/dev/null | head -c 40)）—— 另案，本批不修"

echo "cli_semantics: rc=$rc"
exit $rc
