#!/usr/bin/env bash
# tools/jit_sweep.sh — JIT 模式（无 -o）的静默崩溃回归门禁（批次 315 / 任务 #26）。
#
# 为什么需要它：三套基线全走 `-o`（AOT），JIT 路径**一行都没覆盖**。于是
# `zetac file.z` 可以当场 SIGSEGV（rc=139、无一行诊断）而门禁全绿 —— 这正是
# 批次 315 修掉的东西。本脚本把"不许再出现静默崩溃"钉成可复跑判据。
#
# 判据（两条，缺一即红）：
#   1) segv == 0            —— JIT 崩一个都不许有（装载期/执行期都算）
#   2) ok   >= ZETA_JIT_MIN_OK（默认 163，批次 315 实测基线）
# 只报数不改动任何东西；跑过的程序会真的执行，故有 timeout 兜底。
#
# 用法：tools/jit_sweep.sh          # 全跑 + 判据
#       tools/jit_sweep.sh -v       # 额外列出每个文件的分类
#       ZETA_JIT_MIN_OK=200 tools/jit_sweep.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
TIMEOUT="${ZETA_JIT_TIMEOUT:-25}"
MIN_OK="${ZETA_JIT_MIN_OK:-163}"
VERBOSE=0
[ "${1:-}" = "-v" ] && VERBOSE=1

[ -x "$ZETAC" ] || { echo "no $ZETAC — run: cargo build --release" >&2; exit 2; }
command -v timeout >/dev/null || { echo "need coreutils timeout" >&2; exit 2; }

WORK=$(mktemp)


# 批次 461（并行化）：557 个文件逐个串行编译+运行是门禁第二大头（~10-15 分钟）。
# 文件之间零共享，按 ZETA_JIT_JOBS（默认核数，上限 8）分片成多个子进程并行，
# 各写各的分片文件避免追加交错；分类规则与串行版逐字相同。
JOBS="${ZETA_JIT_JOBS:-$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)}"
case "$JOBS" in ''|*[!0-9]*) JOBS=4 ;; esac
[ "$JOBS" -lt 1 ] && JOBS=1
[ "$JOBS" -gt 24 ] && JOBS=24
WORKD=$(mktemp -d /tmp/zeta_jitsweep.XXXXXX)
trap 'rm -rf "$WORK" "$WORKD"' EXIT

for f in tests/unit-tests/*.z tests/python_style/*.z; do
  [ -f "$f" ] && echo "$f"
done | awk -v d="$WORKD" -v j="$JOBS" '{ print > (d "/list_" (NR % j)) }'

w=0
while [ "$w" -lt "$JOBS" ]; do
  (
    [ -f "$WORKD/list_$w" ] || exit 0
    while IFS= read -r f; do
      rc=0
      msg=$(timeout "$TIMEOUT" "$ZETAC" "$f" 2>&1) || rc=$?
      # 顺序要紧：填桩后的 error[E4016] 是 rc=1，而"原本就红"的用例也可能是 rc=1，
      # 所以只有带 E4016 的才算 trap；rc=0 一律 ok（哪怕它打了 warning[E4016]）。
      case $rc in
        0) tag=ok ;;
        139) tag=segv ;;
        124) tag=timeout ;;
        *) if printf '%s' "$msg" | grep -q 'E4016'; then tag=trap; else tag=fail; fi ;;
      esac
      printf '%-8s %s\n' "$tag" "$f" >>"$WORKD/out_$w"
    done < "$WORKD/list_$w"
  ) &
  w=$((w + 1))
done
wait
cat "$WORKD"/out_* >>"$WORK" 2>/dev/null

[ "$VERBOSE" = 1 ] && cat "$WORK"

n() { awk -v t="$1" '$1==t{n++} END{print n+0}' "$WORK"; }
counts="ok=$(n ok) trap=$(n trap) fail=$(n fail) timeout=$(n timeout) segv=$(n segv)"
echo "jit sweep: $counts  (total $(wc -l <"$WORK" | tr -d ' ')，最小 ok=$MIN_OK)"

rc=0
[ "$(n segv)" -gt 0 ] && { echo "RED: $(n segv) 个文件在 JIT 下静默崩溃（SIGSEGV）"; rc=1; }
[ "$(n ok)" -lt "$MIN_OK" ] && { echo "RED: ok=$(n ok) < $MIN_OK —— 有程序从跑通变成跑不通"; rc=1; }
[ "$rc" = 0 ] && echo "GREEN: JIT 无静默崩溃，ok 未回退"
awk '$1=="segv"{print "  segv: " $2}' "$WORK"
exit "$rc"
