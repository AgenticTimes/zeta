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
# 判据形状（批次 344 的三翼 + 348 的两翼 + 349 的一翼 + 350 的一翼 = 七翼，缺一翼锁不住）：
#   阳性翼 —— 裸跑必须"被执行"。这一翼是探测器自己的对照：`ran()` 只认运行路径自己打的
#             `^Result: `（main.rs:930）。不能用"stdout 里有没有程序的输出"——转储文本里
#             就含源码与函数名，本批第一版因此把 17 行 MIR 读成"执行了 17 次"。
#   收窄翼 —— 五个只读入口（四标志 + env 旋钮）与它们的组合：被执行=0，**且**各自 rc=0
#             （否则"没执行"可能只是提前报错，判据就成了空的）。
#   不变翼 —— `-o` 那条路逐字不变：不执行、有产物；`--dump-mir` 的转储与 `-o` 无关
#             （本批只收窄 else 分支，不许动转储本身）。
#
# 期望值全部来自修复后实测，不是推测。批次 348 又把本文件的"三翼"扩到**五翼**：
#   翼 A —— `--emit-llvm f -o g` 从此**由标志决定产物**（g 是 IR 文本，不是链接好的可执行
#           文件；修复前实测 g = Mach-O 238,440 B 且 IR 只打到 stderr）。env 旋钮
#           ZETA_DUMP_IR 明确排除在外：它只多加一份 stderr 转储，不许改 `-o` 的含义。
#   翼 B —— 无输入时内置演示（CWD 相对的 examples/selfhost.z）**不许多被编译**：只读标志
#           走的就是这条路（修复前四个只读入口都编译了它）。
#   翼 C —— `--repl`（批次 349 / backlog #63）：标志要么生效要么**出声拒绝**，且会话
#           必须自己结束。修复前 `_dump_mir` 收下即弃、三个只读标志静默忽略、
#           EOF 之后无限打 `> `。这一翼每条断言都过 `head -c` 的保险丝（见翼 C 注释）。
#   翼 D —— 选项**词汇表**（批次 350 / backlog #80 ①）：解析器不认的选项必须点名，
#           多输入必须把两个都念出来，`-o`/`--target`/`--features` 掉尾缺值要说清缺的是谁，
#           而 `--help` 只许说真话 —— 它宣传的每个长选项都得真的被解析（漂移闸）。
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

# ── 批次 348 翼 A：`--emit-llvm` 与 `-o` 同在场时，`-o` 的产物就是 IR 文本 ──
# 修复前实测（roadmap 批次 348）：`--emit-llvm f -o g` 会**忽略** --emit-llvm —— IR 照样打
# 到 stderr，g 是链接好的 `Mach-O 64-bit executable`（238,440 B）。现在标志说话算数。
# 作用域只到显式标志：env 旋钮 ZETA_DUMP_IR 不许偷偷改变 `-o` 的含义（下面的对照翼钉这条）。
echo "== 批次 348 翼 A：--emit-llvm + -o ⇒ 产物是 IR 文本（env 旋钮除外）"
IRF="$TMP/ir.ll"
"$ZETAC" --emit-llvm "$SRC" -o "$IRF" >/dev/null 2>&1
# 这里一律不用 `grep -q`：本文件开着 pipefail，-q 命中就退 ⇒ 生产者吃到 SIGPIPE（rc=141），
# 判据会把"其实对"读成 FAIL（本批实测一次：rcC=1，两 locale 逐字相同的假红）。
if [[ $(head -n1 "$IRF" 2>/dev/null | grep -c '^; ModuleID') -ge 1 ]]; then
    echo "  ok   --emit-llvm -o 的产物首行是 IR 标记（$(wc -c < "$IRF" | tr -d ' ') 字节文本）"
else
    echo "  FAIL --emit-llvm -o 的产物不是 IR 文本：$(file -b "$IRF" 2>/dev/null | head -c 40)"
    rc=1
fi
want "--emit-llvm -o 不执行（IR 分支也不许跑）" 0 "$(ran "$ZETAC" --emit-llvm "$SRC" -o "$TMP/ir2")"
if [[ -e "$IRF.o" ]]; then echo "  FAIL 走 IR 分支却留下了 $IRF.o（说明还是跑了 AOT）"; rc=1; else
    echo "  ok   无 .o 残留（AOT/链接确实被跳过）"; fi
ENVF="$TMP/ir_env"
ZETA_DUMP_IR=1 "$ZETAC" "$SRC" -o "$ENVF" >/dev/null 2>&1
if [[ -x "$ENVF" ]]; then echo "  ok   ZETA_DUMP_IR=1 时 -o 仍是可执行文件（旋钮不改产物）"; else
    echo "  FAIL env 旋钮把 -o 的产物也改了 —— 越界"; rc=1; fi
MIRF="$TMP/mir_o"
"$ZETAC" --dump-mir "$SRC" -o "$MIRF" >/dev/null 2>&1
if [[ -x "$MIRF" ]]; then echo "  ok   --dump-mir 与 -o 并存时仍照常链接"; else
    echo "  FAIL --dump-mir -o 没产物（收窄过头）"; rc=1; fi

# ── 批次 348 翼 B：无输入时，只读标志不许触发内置演示 ──
# `zetac` 不带文件会读 **CWD 相对** 的 examples/selfhost.z，编译并在编译器进程里跑它
# （main.rs 里除 `-o` 之外第二处 `main.call()`）。探测器只认解析器自己打的
# `warning: [W1002] examples/selfhost.z:NN:` —— 修复前它在这四个入口都出现，
# 即"要个 dump 结果把演示程序编译了"。诊断文本自己含该文件名，所以**不能** grep 文件名，
# 必须钉 W1002 这条只有解析截断才会打的行（批次 344 同类自伤的复犯预防）。
echo "== 批次 348 翼 B：无输入 + 只读标志 ⇒ 内置演示一次都不许多编译"
fb_hit() { "$@" </dev/null 2>&1 1>/dev/null | grep -c 'W1002\] examples/selfhost.z'; }
fb_rc() { "$@" </dev/null >/dev/null 2>&1; echo $?; }
want "无输入裸跑 → 演示真被编译（阳性对照）" 1 "$(fb_hit "$ZETAC")"
for flag in --dump-mir --emit-llvm --report-stubs --report-untyped; do
    want "无输入 $flag → 演示没被编译" 0 "$(fb_hit "$ZETAC" "$flag")"
    want "无输入 $flag 仍出声（rc=1）" 1 "$(fb_rc "$ZETAC" "$flag")"
done
if [[ $("$ZETAC" --dump-mir </dev/null 2>&1 1>/dev/null | grep -c 'no input file') -ge 1 ]]; then
    echo "  ok   无输入的诊断说清了为什么判错"
else
    echo "  FAIL 无输入只读入口没有诊断（静默失败）"; rc=1
fi

# ── 批次 349 翼 C：`--repl` 的只读标志/旋钮要么生效要么出声，且 EOF 必须结束会话 ──
# 修复前实测（roadmap 批次 349）：
#   · `repl(_dump_mir)` 收下即弃 —— `--repl --dump-mir` 一个字节 MIR 都不打；
#   · `--repl --emit-llvm` / `--report-stubs` / `--report-untyped` 全部静默忽略；
#   · `--repl` 不在 argv[1] 时它被当成**输入文件名**，只有一行 `Os { code: 2, NotFound }`；
#   · EOF 之后 `read_line` 返回 0 被 `is_empty` 折成"空行 continue" ⇒ **无限打 `> `**
#     （实测 2 分钟 173 MB，只有 kill 停得下）。
# 保险丝：每条 repl 调用都过 `head -c $CAP`，**不依赖外部 timeout**（门禁不能假设
# 机器上有 coreutils）。取满 CAP 字节 = 会话没结束 = 判红，所以"没结束"本身是一个
# 可断言的读数，而不是一次挂死。
echo "== 批次 349 翼 C：--repl 要么说话要么出声（且自己会结束）"
CAP=4000
rp() { local in=$1; shift; printf '%s\n' "$in" | "$ZETAC" "$@" 2>&1 | head -c "$CAP"; }
rp_sig() { local sig=$1 in=$2; shift 2
    printf '%s\n' "$in" | "$ZETAC" "$@" 2>&1 | head -c "$CAP" | grep -c -- "$sig"; }
rp_rc() { local in=$1; shift
    # 保险丝在这里**同样**要有：`>/dev/null` 不是读者，跑飞的 repl 只会一直写。
    # `| head -c CAP` 取满就退出 ⇒ 写端下一次 flush 拿到 EPIPE，进程自己结束。
    # 注意：被这样杀掉的 rc 不再代表程序自己的判断（EPIPE→rc1 或 panic→rc101 都可能），
    # 所以每条 rc 断言都必须与它旁边那条"签名"断言**配对**读，单看 rc 会放过"没拒绝而是在转圈"。
    printf '%s\n' "$in" | "$ZETAC" "$@" 2>&1 | head -c "$CAP" >/dev/null
    echo "${PIPESTATUS[1]}"; }
short() { local label=$1 val=$2; if [[ ${#val} -lt $CAP ]]; then
        echo "  ok   $label → 会话自己结束（${#val} 字节 < 保险丝 ${CAP}）";
    else echo "  FAIL $label → 输出截在 $CAP 字节：EOF 之后还在转圈"; rc=1; fi; }
BARE=$(rp '6*7' --repl)
short "裸 --repl 会结束" "$BARE"
want "裸 --repl 仍真求值（6*7 → 42）" 1 "$(printf '%s' "$BARE" | grep -c '42')"
want "裸 --repl 不打 MIR（下一断言的对照）" 0 "$(printf '%s' "$BARE" | grep -c '== MIR')"
DUMP=$(rp '6*7' --repl --dump-mir)
short "--repl --dump-mir 会结束" "$DUMP"
want "--repl --dump-mir 打出 canonical MIR" 1 "$(printf '%s' "$DUMP" | grep -c '== MIR main ==')"
want "--repl --dump-mir 之后仍求值" 1 "$(printf '%s' "$DUMP" | grep -c '42')"
want "ZETA_DUMP_IR=1 --repl 打出 IR" 1 "$(printf '%s\n' '1+1' \
    | ZETA_DUMP_IR=1 "$ZETAC" --repl 2>&1 | head -c "$CAP" | grep -c "ModuleID = 'repl'")"
for flag in --emit-llvm --report-stubs --report-untyped; do
    want "--repl $flag 出声拒绝" 1 "$(rp_sig 'does not honour' '' --repl "$flag")"
    want "--repl $flag 判错（rc=1）" 1 "$(rp_rc '' --repl "$flag")"
done
want "$SRC --repl 位置错出声" 1 "$(rp_sig 'must be the first argument' '' "$SRC" --repl)"
want "$SRC --repl 位置错判错（rc=1）" 1 "$(rp_rc '' "$SRC" --repl)"

# ── 批次 350 翼 D：选项词汇表 ──
# 修复前实测（`_ => input = Some(args[i])` 把任何字串当输入文件名）：
#   ① `zetac --dump-mir2 f.z` 拼错的标志被后一个位置参数盖掉 ⇒ rc=0、程序照跑、一声不出；
#   ② 唯一那份 `--help` 在 `compiler_config.rs:234`，其调用者 `from_args`（:107）在图内
#      零调用者 ⇒ 它宣传的 16 个选项实测 16 个全被静默吞掉（含 `-h`/`--help` 自己）；
#   ③ 两个输入文件时前一个被静默丢弃；④ `-o`/`--target`/`--features` 掉尾无值被忽略。
# 口径承接批次 349：**拒绝类判据只钉文本**，rc 不给拒绝作证；每条"没执行"都要有
# `ran` 的正探针压着（同文件裸跑真执行），否则"没执行"可能只是提前报错。
echo "== 批次 350 翼 D：未知选项点名，且被吞的标志不再执行"
D="$TMP/d.out"
for flag in --dump-mir2 --emit-asm --incremental --verbose --report-stubs2; do
    "$ZETAC" "$flag" "$SRC" >"$D" 2>&1; d_rc=$?
    want "$flag 被点名（错误里有它自己）" 1 "$(grep -c "unrecognized option \`$flag\`" "$D")"
    want "$flag 在场时被编译的程序没执行" 0 "$(grep -c '^Result: ' "$D")"
    want "$flag 判错（rc=1）" 1 "$d_rc"
done
# 掉尾无值（此前静默走到"无输入"那条演示回退）
for flag in -o --target --features; do
    "$ZETAC" "$flag" </dev/null >"$D" 2>&1
    want "$flag 掉尾要说清缺值" 1 "$(grep -c 'expects a value' "$D")"
done
# 多输入文件：此前后一个静默盖掉前一个
"$ZETAC" "$SRC" "$SRC" >"$D" 2>&1
want "两个输入文件要点名两个" 1 "$(grep -c 'multiple input files' "$D")"
want "多输入时不执行" 0 "$(ran "$ZETAC" "$SRC" "$SRC")"
# --help 必须真出声，且位置无关（修复前 `--help` 自己也被当输入文件名）
want "--help 打出 Usage 行" 1 "$("$ZETAC" --help 2>&1 | grep -c '^Usage: zetac')"
want "文件 + --help 仍然只打帮助（放后面也管用）" 1 \
    "$("$ZETAC" "$SRC" --help 2>&1 | grep -c '^Usage: zetac')"
want "--repl --help 只打帮助（早于 --repl 的分支）" 1 \
    "$("$ZETAC" --repl --help </dev/null 2>&1 | grep -c '^Usage: zetac')"
# 漂移闸：帮助里宣传的每个长选项，解析器必须真的认得（在 main.rs 里以字面量出现）。
# 计数断言是这条闸自己的阳性对照 —— 抓不到选项就说明 --help 没出声，闸就成了空的。
HELP=$( "$ZETAC" --help 2>&1 | grep -oE '\-\-[a-z][a-z0-9-]*' | sort -u )
NH=$(printf '%s\n' "$HELP" | grep -c . )
want "--help 至少宣传 10 个长选项（漂移闸的前提）" 1 \
    "$( [ "$NH" -ge 10 ] && echo 1 || echo 0 )"
LIE=""
for o in $HELP; do
    grep -q -- "\"$o\"" src/main.rs || LIE="$LIE $o"
done
want "宣传的长选项没有一个是解析器不认的（帮助不说谎）" 0 "$( [ -z "$LIE" ] && echo 0 || echo 1 )"
if [[ -n "$LIE" ]]; then echo "       谎报项：$LIE"; fi

echo "cli_semantics: rc=$rc"
exit $rc
