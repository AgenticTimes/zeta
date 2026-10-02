#!/usr/bin/env bash
# tools/sample_gate.sh — 本树（cleanup）每批必跑的抽样检查，按批号轮转 10% 窗口。
#
# 用法：bash tools/sample_gate.sh <批次号>
# 四步：差分 10% 轮转／python_style 10% 轮转／official 10% 轮转／语料全跑
#
# 为什么要有这个脚本（批次 10005，实拍事故）：本树原先没有门禁脚本，10004 那批手搓
# 检查时把 `run_one.sh` 当成在仓库根（实际在 tests/python_style/），12 次调用全部
# 什么也没跑，verdict 文件是空的，读数看着像"12 例都没判定"而不像"没跑到"——
# 空输出把"没命中"伪装成了一次正常读数。所以这里对每一步都加分母守卫：
# 分母为 0、verdict 缺失或空，一律判红并单独计数，绝不当成通过。
#
# rc 口径：0＝四步都过；1＝任一步红（含分母守卫触发）；2＝用法错误／件不在。
set -uo pipefail

B=${1:-}
if [ -z "$B" ]; then
  echo "用法: bash tools/sample_gate.sh <批次号>" >&2
  exit 2
fi
W=$(( B % 10 ))
ROOT=$(cd "$(dirname "$0")/.." && pwd)
ZETAC="$ROOT/target/release/zetac"
RUN_ONE="$ROOT/tests/python_style/run_one.sh"
WORK=$(mktemp -d "/tmp/zeta_gate_${B}.XXXXXX")
export ZETA_STRICT_RUNTIME_DIR=1

rc_total=0
[ -x "$ZETAC" ] || { echo "缺可执行件: $ZETAC（先 cargo build --release）"; exit 2; }
[ -f "$RUN_ONE" ] || { echo "缺单用例工具: $RUN_ONE"; exit 2; }
echo "批次 $B 窗口 $W（ROOT=$ROOT）"
echo "被测件 md5: $(md5 -q "$ZETAC")  运行期 .o md5: $(md5 -q "$ROOT/zeta_runtime_c.o")"

# --- ① 差分 10% 轮转 ---
python3 "$ROOT/tools/diff_test.py" --sample "10:$W" > "$WORK/diff.log" 2>&1
rc_diff=$?
judged=$(grep -oE 'judged=[0-9]+' "$WORK/diff.log" | tail -1 | cut -d= -f2)
match=$(grep -oE 'match=[0-9]+' "$WORK/diff.log" | tail -1 | cut -d= -f2)
bad=$(grep -oE 'bad_case=[0-9]+' "$WORK/diff.log" | tail -1 | cut -d= -f2)
dif=$(( ${judged:-0} - ${match:-0} ))
# 红/绿只看读数，不看 diff_test 的 rc（批次 10007 改口径，两处都是实测）：
# `--sample` 模式下 `tools/diff_test.py:307` 只会因"参考侧跑不出真值的坏用例"置 rc=2，
# 真正的分歧要走到基线比对才置 rc=1，而 :317 的 `return rc` 在抽样分支提前退出够不到它
# ⇒ rc 对分奇毫无区分力：坏用例 rc=2（旧口径误判红），有分歧 rc=0（旧口径误判绿）。
# 坏用例不计红＝主树批次 756 已裁定的口径，本树此前未跟。
if [ "${judged:-0}" -eq 0 ]; then
  echo "① 差分: 分母为 0＝读数作废（抽样一条也没跑到，rc=$rc_diff）"; rc_total=1
elif [ "$dif" -gt 0 ]; then
  echo "① 差分: 红 ${match:-0}/${judged} 不一致 ${dif} 条（rc=$rc_diff）"; rc_total=1
elif [ "${bad:-0}" -gt 0 ]; then
  echo "① 差分: ${match}/${judged} 一致；另 ${bad} 条坏用例（参考侧跑不出真值，不计红，rc=$rc_diff）"
else
  echo "① 差分: ${match}/${judged} 一致 rc=0"
fi

# --- ② python_style 10% 轮转 ---
ps_ok=0; ps_total=0; ps_bad=""; ps_missing=""; ps_known_n=0; ps_known=""; ps_xpass=""
for z in "$ROOT"/tests/python_style/t*.z; do
  [ -e "$z" ] || continue
  b=$(basename "$z" .z)
  h=$(( $(printf '%s' "$b" | cksum | awk '{print $1}') % 10 ))
  [ "$h" -eq "$W" ] || continue
  ps_total=$(( ps_total + 1 ))
  d="$WORK/ps_$b"
  mkdir -p "$d"
  timeout 150 bash "$RUN_ONE" "$z" "$d" > /dev/null 2>&1
  if [ ! -s "$d/verdict" ]; then
    ps_missing="$ps_missing $b"
  elif grep -q "^PASS" "$d/verdict"; then
    ps_ok=$(( ps_ok + 1 ))
  elif grep -q "^KNOWN-FAIL" "$d/verdict"; then
    ps_known_n=$(( ps_known_n + 1 )); ps_known="$ps_known $b"
  elif grep -q "^XPASS" "$d/verdict"; then
    ps_xpass="$ps_xpass $b"
  else
    ps_bad="$ps_bad $b"
  fi
done
# 钉住的夹具（`// known-fail:`）不计红：run_one.sh 对带标记的用例只在值符合预期时给
# XPASS、其余给 KNOWN-FAIL（批次 304 口径），所以 KNOWN-FAIL＝"已知缺口照旧"，
# XPASS＝"预期已达成、标记可摘"——两者都不是本批引入的失败。真红仍然只有
# FAIL、缺 verdict 与分母为 0 三类（批次 10008 加这段；窗口 7 首跑实拍 t450 被当红）。
if [ "$ps_total" -eq 0 ]; then
  echo "② python_style: 分母为 0＝读数作废"; rc_total=1
elif [ -n "$ps_missing" ]; then
  echo "② python_style: 缺 verdict 判定文件（工具没跑到底）:$ps_missing"; rc_total=1
elif [ -n "$ps_bad" ]; then
  echo "② python_style: ${ps_ok}/$(( ps_total - ps_known_n )) PASS，非 PASS:$ps_bad"; rc_total=1
else
  echo "② python_style: ${ps_ok}/$(( ps_total - ps_known_n )) PASS" \
       "$([ "$ps_known_n" -gt 0 ] && printf '（另 %d 条钉住 KNOWN-FAIL 不计红:%s）' "$ps_known_n" "$ps_known")" \
       "$([ -n "$ps_xpass" ] && printf '★ 可摘标（XPASS）:%s' "$ps_xpass")"
fi

# --- ③ official 10% 轮转 ---
of_ok=0; of_total=0; of_link=0; of_fail=""
for z in "$ROOT"/tests/unit-tests/*.z; do
  [ -e "$z" ] || continue
  b=$(basename "$z" .z)
  h=$(( $(printf '%s' "$b" | cksum | awk '{print $1}') % 10 ))
  [ "$h" -eq "$W" ] || continue
  of_total=$(( of_total + 1 ))
  out=$(timeout 150 "$ZETAC" "$z" -o "$WORK/of_$b" 2>&1 || true)
  if echo "$out" | grep -q "Compiled to"; then
    of_ok=$(( of_ok + 1 ))
  elif echo "$out" | grep -q "Linking failed"; then
    of_ok=$(( of_ok + 1 )); of_link=$(( of_link + 1 ))
  else
    of_fail="$of_fail $b"
  fi
  rm -f "$WORK/of_$b"
done
if [ "$of_total" -eq 0 ]; then
  echo "③ official: 分母为 0＝读数作废"; rc_total=1
elif [ -n "$of_fail" ]; then
  echo "③ official: ${of_ok}/${of_total} 编译通过（含链接缺绑定 ${of_link}），失败:$of_fail"; rc_total=1
else
  echo "③ official: ${of_ok}/${of_total} 编译通过（含链接缺绑定 ${of_link}，chronic 口径不计红）"
fi

# --- ④ 语料全跑 ---
python3 "$ROOT/tools/corpus_baseline.py" > "$WORK/corpus.log" 2>&1
co_line=$(grep -E "解析通过" "$WORK/corpus.log" | tail -1)
if [ -z "$co_line" ]; then
  echo "④ 语料: 没读到"解析通过"一行＝读数作废"; rc_total=1
else
  co_ok=$(echo "$co_line" | grep -oE '[0-9]+/[0-9]+' | cut -d/ -f1)
  co_all=$(echo "$co_line" | grep -oE '[0-9]+/[0-9]+' | cut -d/ -f2)
  if [ "${co_all:-0}" -eq 0 ]; then
    echo "④ 语料: 分母为 0＝读数作废"; rc_total=1
  elif [ "${co_ok:-0}" -lt 40 ]; then
    echo "④ 语料: ${co_ok}/${co_all}（低于满数，逐条归因见 ${WORK}/corpus.log；不据此判红——本树在册失败已登记 backlog）"
  else
    echo "④ 语料: ${co_ok}/${co_all} 满数"
  fi
fi

echo "----"
echo "sample_gate 批次 $B 窗口 $W: rc=$rc_total（明细目录 $WORK）"
exit "$rc_total"
