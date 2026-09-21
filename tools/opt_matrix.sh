#!/usr/bin/env bash
# tools/opt_matrix.sh — G.1（refactor.md）：同一套用例在**不同优化级别**下各跑一遍，
# 把"随级别翻转"的用例列出来。只读工具：不改源文件、不动 target/。
#
# 用法：
#   tools/opt_matrix.sh                 跑 O3 与 NO_OPT 两级
#
# 为什么只有这两级（优化开关的真实拓扑，逐条锚定）：
#   * 唯一的运行期开关是 `ZETA_NO_OPT`（存在性检查，值无所谓）：
#       - jit.rs:24-26  —— 有值就 **跳过 `default<O3>` IR 管线**；
#       - jit.rs:168-172 —— 同时把 TargetMachine 的代码生成级别降到 None。
#     ⇒ 它一次动**两半**（IR 优化 + 后端 ISel），所以"NO_OPT 下对、O3 下错"只说明
#     问题在这两半之一，**不能**据此二分到具体 pass。要单变量二分需要一个只改
#     管线字符串（jit.rs:29 `"default<O3>"`）的旋钮——那是改热文件，G.1 修复档的事。
#   * `src/compiler_config.rs` 解析 `-O0/-O1/-O2/-O3`（:121-124）写进
#     `config.opt_level`（:22），但**全仓无人读它**，且 `pub mod compiler_config`
#     （lib.rs:52）之外再无 `compiler_config` 引用 ⇒ 这四个命令行标志当前是**死码**，
#     拿它们做矩阵会得到四份完全相同的结果。
#   * `middle::optimization::optimize(mir, level)`（optimization.rs:490）按级别分发，
#     但零调用点 ⇒ MIR 层优化器同样未接线（roadmap 还债③）。
#
# 判据：两级之间 verdict 集合有任何差异即 exit 1。
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 1

LEVELS="O3 NO_OPT"
while [ $# -gt 0 ]; do
    case "$1" in
        --levels) LEVELS="$2"; shift 2 ;;
        -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

WORK="${OPT_MATRIX_DIR:-$(mktemp -d /tmp/zeta_opt_matrix.XXXXXX)}"
mkdir -p "$WORK"

for lv in $LEVELS; do
    case "$lv" in
        # bash 的 `env -u` 与 `unset` 对子进程等价：这里显式清掉，避免继承
        O3)     unset ZETA_NO_OPT ;;
        NO_OPT) export ZETA_NO_OPT=1 ;;
        *)      echo "unknown level: $lv (可用：O3 NO_OPT)" >&2; exit 2 ;;
    esac
    log="$WORK/$lv.raw"
    tests/python_style/run.sh > "$log" 2>&1
    rc=$?
    # 只留"用例 → 判定"，忽略计数行与细节文本（细节里带时间/路径会假报差异）
    grep -oE '^(PASS|FAIL|XPASS|KNOWN-FAIL|BAD)  +[A-Za-z0-9_.-]+' "$log" \
        | awk '{print $1" "$2}' | sort > "$WORK/$lv.verdicts"
    echo "$lv: rc=$rc verdicts=$(wc -l < "$WORK/$lv.verdicts" | tr -d ' ')"
done

if [ $(printf '%s\n' $LEVELS | wc -l | tr -d ' ') -lt 2 ]; then
    echo "单级：无对照可言"; exit 0
fi

first=$(printf '%s\n' $LEVELS | head -1)
flips=0
echo "----------------------------------------"
for lv in $LEVELS; do
    [ "$lv" = "$first" ] && continue
    if ! cmp -s "$WORK/$first.verdicts" "$WORK/$lv.verdicts"; then
        echo "差异 $first vs $lv："
        diff "$WORK/$first.verdicts" "$WORK/$lv.verdicts" | grep -E '^[<>]' | sort -k2
        flips=$((flips + $(diff "$WORK/$first.verdicts" "$WORK/$lv.verdicts" | grep -cE '^[<>]')))
    else
        echo "$first == $lv（$(wc -l < "$WORK/$lv.verdicts" | tr -d ' ') 个用例判定逐项相同）"
    fi
done
echo "----------------------------------------"
echo "opt matrix: levels=[$LEVELS]  flips=$flips"
echo "明细：$WORK/<level>.raw（verdicts=归一化后的 用例→判定）"

if [ "$flips" != 0 ]; then exit 1; fi
exit 0
