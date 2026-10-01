#!/usr/bin/env bash
# tools/sample_gate.sh — 批次 755（2026-10-01 用户裁定：停跑全套测试）后的每批门禁配方。
#
# 用法：sample_gate.sh <批次号> [--no-corpus] [--no-official] [--no-python] [--no-diff]
#
# 每批默认跑：
#   1) 差分 10% 轮转抽样        tools/diff_test.py --sample 10:<批号%10>（2026-09-30 裁定沿用）
#   2) python_style 10% 轮转    ZETA_PY_SAMPLE=<批号%10> tests/python_style/run.sh（755 新增钩子）
#   3) official 10% 轮转        tests/unit-tests/*.z 按同名哈希抽 1/10 逐个编译
#   4) corpus 全跑              40 个真实策略文件（~2 分钟；754 刚修复 40/40，不抽样）
#
# 规则与差分抽样同规：窗口＝批号 %10，十批轮转逼近全覆盖；红了由调用方按需跑
# 全族定位；抽样豁免基线比对。十批界不再有独占全量门禁（760 全量已裁停）。
# 本脚本不改 tools/run_all.sh（docs/ABI.md 以裸行号引用它）；run_all.sh 保留给
# 按需全族定位用。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
N=${1:?用法: sample_gate.sh <批次号> [--no-corpus] [--no-official] [--no-python] [--no-diff]}
shift || true
case "$N" in ''|*[!0-9]*) echo "批次号必须是数字"; exit 2 ;; esac
W=$((N % 10))

run_corpus=1; run_official=1; run_python=1; run_diff=1
for a in "$@"; do
    case "$a" in
        --no-corpus)  run_corpus=0 ;;
        --no-official) run_official=0 ;;
        --no-python)  run_python=0 ;;
        --no-diff)    run_diff=0 ;;
        *) echo "未知参数: $a"; exit 2 ;;
    esac
done

rc=0

if [ "$run_diff" -eq 1 ]; then
    echo "── 差分抽样 窗口 $W ──────────────────────────"
    python3 "$ROOT/tools/diff_test.py" --sample "10:$W" | tail -2 || rc=1
fi

if [ "$run_python" -eq 1 ]; then
    echo "── python_style 抽样 窗口 $W ─────────────────"
    ZETA_PY_SAMPLE=$W "$ROOT/tests/python_style/run.sh" || rc=1
fi

if [ "$run_official" -eq 1 ]; then
    echo "── official 抽样 窗口 $W ─────────────────────"
    ok=0; total=0
    for z in "$ROOT"/tests/unit-tests/*.z; do
        b=$(basename "$z" .z)
        h=$(( $(printf '%s' "$b" | cksum | awk '{print $1}') % 10 ))
        [ "$h" -eq "$W" ] || continue
        total=$((total + 1))
        if "$ROOT/target/release/zetac" "$z" -o "/tmp/zeta_sample_gate_$b" >/dev/null 2>&1; then
            ok=$((ok + 1))
        else
            echo "official 编译失败: $b"
            rc=1
        fi
        rm -f "/tmp/zeta_sample_gate_$b"
    done
    echo "official: $ok/$total compiled (窗口 $W)"
fi

if [ "$run_corpus" -eq 1 ]; then
    echo "── corpus 全跑 ───────────────────────────────"
    python3 "$ROOT/tools/corpus_baseline.py" 2>&1 | grep '解析通过' | tail -1 || rc=1
fi

echo "sample_gate 批 $N:rc=$rc"
exit "$rc"
