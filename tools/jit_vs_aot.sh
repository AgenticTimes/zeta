#!/usr/bin/env bash
# tools/jit_vs_aot.sh — 批次 774：JIT↔AOT 值级对照门禁。
#
# jit_sweep 只按 rc 分档，双侧 rc=0 而 stdout 分歧（静默值差）不在其判据内；
# 本工具补这一格：逐文件比 JIT（剥 main.rs 的 `Result:` 回显）与 AOT 的 stdout，
# 并把"AOT 绿而 JIT 红"钉成独立红项（JIT 侧把能跑的程序跑崩＝762/764 族形状）。
#
# 分桶：
#   vsame     双侧 rc=0 且掩码后 stdout 相同
#   vswitch   双侧 rc=0 但掩码后 stdout 不同          —— 红项
#   aot0_jitred AOT rc=0 而 JIT rc!=0                 —— 红项
#   jit0_aotred  JIT rc=0 而 AOT rc!=0（单测程序用退出码当哨兵＝#55 家族，信息档）
#   known     在册设计差（见 KNOWN 数组，逐条带理由）
#   bothred / nobuild / timeout  设计内红或不可构建，跳过
#
# 掩码：连续 9 位以上数字（堆地址随 ASLR 逐次变＝坑 册记在册）归一为 <addr>。
# 用法：tools/jit_vs_aot.sh [-v]；ZETA_JVA_TIMEOUT 单例上限（默认 25s）。
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1
ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
TIMEOUT="${ZETA_JVA_TIMEOUT:-25}"
VERBOSE=0
[ "${1:-}" = "-v" ] && VERBOSE=1
command -v timeout >/dev/null || { echo "need coreutils timeout" >&2; exit 2; }

# 在册设计差：JIT 进程 argv=[zetac,file.z]（sys.argv 多一元），AOT 是二进制自身。
is_known() {
  case "$1" in
    tests/python_style/t107_sys_argv.z) return 0 ;;
    *) return 1 ;;
  esac
}

WORKD=$(mktemp -d)
trap 'rm -rf "$WORKD"' EXIT
JOBS="${ZETA_JVA_JOBS:-$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)}"
case "$JOBS" in ''|*[!0-9]*) JOBS=4 ;; esac
[ "$JOBS" -lt 1 ] && JOBS=1

ls tests/unit-tests/*.z tests/python_style/*.z \
  | awk -v d="$WORKD" -v j="$JOBS" '{ print > (d "/list_" (NR % j)) }'

w=0
while [ "$w" -lt "$JOBS" ]; do
  (
    [ -f "$WORKD/list_$w" ] || exit 0
    bin="$WORKD/b_$w"
    while IFS= read -r f; do
      if is_known "$f"; then printf 'known   %s\n' "$f" >> "$bin"; continue; fi
      jmsg=$(timeout "$TIMEOUT" "$ZETAC" "$f" 2>/dev/null); jrc=$?
      ob="$WORKD/$(printf %s "$f" | tr /.- __).exe"
      if ! timeout 60 "$ZETAC" -o "$ob" "$f" >/dev/null 2>&1; then
        printf 'nobuild %s\n' "$f" >> "$bin"; rm -f "$ob"; continue
      fi
      amsg=$(timeout "$TIMEOUT" "$ob" 2>/dev/null); arc=$?
      rm -f "$ob"
      mask() { printf '%s\n' "$1" | grep -v '^Result: ' | sed -E 's/[0-9]{9,}/<addr>/g'; }
      if [ "$jrc" = 124 ] || [ "$arc" = 124 ]; then printf 'timeout %s\n' "$f" >> "$bin"; continue; fi
      if [ "$jrc" = 0 ] && [ "$arc" = 0 ]; then
        if [ "$(mask "$jmsg")" = "$(mask "$amsg")" ]; then
          printf 'vsame   %s\n' "$f" >> "$bin"
        else
          printf 'vswitch %s\n' "$f" >> "$bin"
        fi
      elif [ "$jrc" != 0 ] && [ "$arc" = 0 ]; then
        printf 'aot0jitred %s\n' "$f" >> "$bin"
      elif [ "$jrc" = 0 ] && [ "$arc" != 0 ]; then
        printf 'jit0aotred %s\n' "$f" >> "$bin"
      else
        printf 'bothred %s\n' "$f" >> "$bin"
      fi
    done < "$WORKD/list_$w"
  ) &
  w=$((w + 1))
done
wait
cat "$WORKD"/b_* >> "$WORKD/all" 2>/dev/null

[ "$VERBOSE" = 1 ] && cat "$WORKD/all"

n() { awk -v t="$1" '$1==t{c++} END{print c+0}' "$WORKD/all"; }
echo "jit_vs_aot: vsame=$(n vsame) vswitch=$(n vswitch) aot0jitred=$(n aot0jitred) jit0aotred=$(n jit0aotred) known=$(n known) bothred=$(n bothred) timeout=$(n timeout) nobuild=$(n nobuild) (total $(wc -l < "$WORKD/all" | tr -d ' '))"
rc=0
# 分母守卫：处理数 < 应处理数＝空壳/漏跑，不许拿"0 红项"报 GREEN（首跑实证 split 假负）
exp=$(ls tests/unit-tests/*.z tests/python_style/*.z | wc -l | tr -d ' ')
tot=$(wc -l < "$WORKD/all" | tr -d ' ')
[ "$tot" -lt "$exp" ] && { echo "RED: 已处理 $tot < 应处理 $exp —— 分母缺口，读数作废"; rc=1; }
[ "$(n vswitch)" -gt 0 ] && { echo "RED: $(n vswitch) 例双侧 rc=0 而 stdout 分歧"; awk '$1=="vswitch"{print "  vswitch: " $2}' "$WORKD/all"; rc=1; }
[ "$(n aot0jitred)" -gt 0 ] && { echo "RED: $(n aot0jitred) 例 AOT 绿而 JIT 红"; awk '$1=="aot0jitred"{print "  aot0jitred: " $2}' "$WORKD/all"; rc=1; }
[ "$rc" = 0 ] && echo "GREEN: 值级对照无静默分歧、无 AOT绿JIT红"
exit "$rc"
