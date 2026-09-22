#!/usr/bin/env bash
# tools/py_module_search_inventory.sh —— PY-A 模块解析的"越界"值域（批次 343，任务 #70）
#
# 为什么需要它：`find_py_module_file`（src/middle/resolver/resolver.rs:2225）把**被编
# 译文件所在目录往上 5 级**连同它自己一起列进搜索基（顺序：source dir=0 → 祖先 1..5 →
# $ZETA_PYLIB → ~/.zeta/packages → 相对路径 pylib → build/stubs）。祖先链是为"工程根
# 相对的点号导入"留的（`from strategies.code.jq_shim import …` 从 strategies/code/ 里
# 编），但它同时意味着两件事：
#   1) 家目录里一个同名 `.z` 会**压过** `pylib` 的库面 —— 同一份编译器 + 同一份源码，
#      读数取决于检出位置上方躺了什么；
#   2) 深度本身就是判据的一部分 —— 本批实测（D 翼）：同一份内容，把被编译文件再往深挪
#      一层，模块从"解析成功"变成搜不到，且**不当场报错**。
# ⇒ 门禁读数随检出位置变化，而这件事此前只有开发者盯着 `PY-A: imported module … from
#   <path>` 那行才看得见（#70）。本批只做**零行为变更**的那一半：越界解析出声（W1005），
#   解析结果本身一个字不改。判据收紧（裸名不许跨祖先、点号名保留 —— wufu 靠它）留给
#   下一批，依据就是 E 段量到的分布。
#
# 两条本批踩过的坑，决定了下面所有断言的形状：
#   1) `W1005=0` **本身不是证据**。夹具第一轮就有三条"不该喊"是绿的，实际原因是文件写
#      在了不存在的目录里 ⇒ 编译当场失败 ⇒ 一条诊断都没有。所以除 D 翼那条明写"就是要
#      它解析不到"的，`expect` 一律先要求看见 `imported module` 那行才允许说"没喊"。
#   2) 候选与它的主文件**同生共死**。上一版把候选删掉后仍复用旧 `main.z`，于是"解析
#      成功"的断言全建成了一句空话。现在每档各有一个 `main_<档>.z`，只 import 本档
#      那一个模块。
#
# 六翼：
#   A 翼 —— 越界必喊：候选分别在越出 1 / 3 / 5 级的基上，各恰好 1 条 W1005，且报出的
#           级数与实际一致（喊的是"越了几级"，不是一个布尔）；
#   A2 翼 —— 边界：5 级是 cap 内最深一档 ⇒ 仍能链接出 `tag`；4 级不喊（B 翼）之外，
#           本翼只钉"级数最大那档确实解析到了"，防止把 cap 读成"4 级"；
#   B 翼 —— 不该喊的一个都不喊：同目录（0 级）、子包、$ZETA_PYLIB、仓库 `pylib` 相对基；
#   C 翼 —— 近优先且**只喊实际用的那个基**：同目录与祖先各一份 ⇒ 取同目录、0 条；
#           祖先 3 级与 5 级各一份 ⇒ 取 3 级、恰好 1 条且级数是 3；
#   D 翼 —— 负控制：判据不许被顺手放宽。候选在 cap 之外（6 级）⇒ **不该**喊 W1005
#           （没解析成功就喊，等于"任何 import 都喊"）。另锁"同一次编译两条 import 时
#           只有越界的那条喊"；
#   F 翼 —— 危害实锤：在越出 3 级的祖先上放一个**与库文件同名**的 `numpy.z`，实测
#           解析落点从 `pylib/numpy.z` 变成祖先那一份并喊 1 条 ⇒ 盘据 1 里"家目录压过
#           库面"不再是推论。（发射点因此只能有一处：`find_py_module_file` 被"探测"
#           与"真加载"两类调用点共用，第一版把告警写在搜索里，numpy 这种注册表+本地
#           库双身份的门 ⇒ 一次导入喊**两条**。F 翼是本批唯一直接测到这一条的档位。）
#   E 段 —— 全语料 W1005 计数，期望 0。
#
# 用法：./tools/py_module_search_inventory.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
[[ -x "$ZETAC" ]] || { echo "building zetac..." >&2; cargo build --release -p zetac -q; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
rc=0

# 所有 A/C/D 翼共用同一条深链，级数由"主文件在 L7"这一句推出，不靠数目录：
#   source dir = L7 ⇒ L6=1  L5=2  L4=3  L3=4  L2=5  （L1=6 已在 cap 之外，留给 D 翼）
CH="$TMP/a/L1/L2/L3/L4/L5/L6/L7"
mkdir -p "$CH"

# 写入前先建父目录：`> 不存在的路径` 只会留下一条 shell 报错和一个空断言（见文件头坑 1）。
mkmod()  { mkdir -p "$(dirname "$2")"; printf 'def tag() -> str:\n    return "%s"\n' "$1" > "$2"; }
mksrc()  { mkdir -p "$(dirname "$2")"; printf '%b' "$3" > "$2"; }
# 每档一个主文件，import 的模块名 = $1（本档唯一候选所在的名字）。
mkmain() { mksrc m "$CH/main_$1.z" "from $2 import tag\nprint(tag())\n"; }

# $1=要编译的 .z，$2..=该次编译的 env 赋值（形如 ZETA_PYLIB=...）
compile() { local f="$1"; shift; env "$@" "$ZETAC" --dump-mir "$f" -o "$TMP/o.o" 2>"$TMP/err" >"$TMP/out"; }
w1005()    { grep -c '\[W1005\]' "$TMP/err" || true; }
w1005_about() { grep '\[W1005\]' "$TMP/err" | grep -c "\`$1\`" || true; }
wlevel()   { grep -oE 'walking [0-9]+ level' "$TMP/err" | grep -oE '[0-9]+' | tr '\n' ','; }
resolved() { grep -oE "imported module \`[^\`]+\` from .*" "$TMP/err" | tail -1; }
dump_err() { sed -n 's/^/         /p' "$TMP/err" | head -6; }

# $1=说明 $2=期望条数。要求解析成功，否则"0 条"是空断言。
expect() {
  local n; n="$(w1005)"
  if [[ "$2" != "$n" ]]; then echo "  FAIL $1 —— 期望 $2 条，实得 $n 条"; dump_err; rc=1; return; fi
  if [[ -z "$(resolved)" ]]; then
    echo "  FAIL $1 —— 条数对上了但根本没解析成功（空断言，不是'不该喊'）"; dump_err; rc=1; return; fi
  echo "  ok   $1（W1005=$n）"
  echo "         $(resolved)"
}
# 只用于 D 翼：这次**就是要它解析不到**。$3=解析不到时本该出现的既有诊断。
expect_noresolution() {
  local n; n="$(w1005)"
  if [[ "$2" != "$n" ]]; then echo "  FAIL $1 —— 期望 $2 条，实得 $n 条"; dump_err; rc=1; return; fi
  if grep -q 'imported module' "$TMP/err"; then
    echo "  FAIL $1 —— 它居然解析成功了，这一档没测到 cap 之外"; dump_err; rc=1; return; fi
  if ! grep -q "$3" "$TMP/err"; then
    echo "  FAIL $1 —— 解析不到时的既有路径（$3）不见了"; dump_err; rc=1; return; fi
  echo "  ok   $1（W1005=$n，且仍走 $3 退化路径）"
}
# $1=说明 $2=期望级数串（wlevel 输出，含尾逗号）
want_level() {
  local g; g="$(wlevel)"
  if [[ "$2" == "$g" ]]; then echo "  ok   $1 → walking ${g%,}"; else
    echo "  FAIL $1 —— 期望 walking ${2%,}，实得 walking ${g%,}"; rc=1; fi
}

echo "== A 翼：越界必喊（级数由实测给出，不是数目录）=="
mkmain zzd_r5 zzd_r5; mkmod R5 "$TMP/a/L1/L2/zzd_r5.z"
compile "$CH/main_zzd_r5.z"; expect "候选在越出 5 级的基上 ⇒ 喊" 1; want_level "且级数报的就是 5" "5,"
mkmain zzd_r3 zzd_r3; mkmod R3 "$TMP/a/L1/L2/L3/L4/zzd_r3.z"
compile "$CH/main_zzd_r3.z"; expect "候选在越出 3 级的基上 ⇒ 喊" 1; want_level "且级数报的就是 3" "3,"
mkmain zzd_r1 zzd_r1; mkmod R1 "$TMP/a/L1/L2/L3/L4/L5/L6/zzd_r1.z"
compile "$CH/main_zzd_r1.z"; expect "候选在越出 1 级的基上 ⇒ 也喊（1 级也是越界）" 1; want_level "且级数报的就是 1" "1,"

echo "== A2 翼：'越界'不等于'失败'——同一份内容放在 5 级外和就地，MIR 逐字节相同 =="
mkmod SAME "$CH/zzd_sym.z"; mkmod SAME "$TMP/a/L1/L2/zzd_sym.z"; mkmain zzd_sym zzd_sym
compile "$CH/main_zzd_sym.z"; cp "$TMP/out" "$TMP/mir_near"
expect "先确认就地那一档不喊（否则下面比的是两份噪声）" 0
rm -f "$CH/zzd_sym.z"                       # 同目录那份会压住远端，必须删掉才测到越界
compile "$CH/main_zzd_sym.z"; cp "$TMP/out" "$TMP/mir_far"
if diff -q "$TMP/mir_near" "$TMP/mir_far" >/dev/null && [[ -s "$TMP/mir_far" ]]; then
  echo "  ok   就地 vs 越出 5 级 → 同一份 MIR（W1005 只是诊断，解析结果一字未改）"
else
  echo "  FAIL 两种落点的 MIR 不一致或为空 —— 本批的'零行为变更'不成立"
  diff "$TMP/mir_near" "$TMP/mir_far" | head -5 | sed -n 's/^/         /p'; rc=1
fi
want_level "且这一档确实走了远端" "5,"

echo "== B 翼：不该喊的一个都不喊 =="
mkmod SAMEDIR "$CH/zzd_same.z"; mkmain zzd_same zzd_same
compile "$CH/main_zzd_same.z"; expect "候选在被编译文件自己的目录（0 级）⇒ 不喊" 0
mkmod PKG "$CH/pkg/zzdpkg.z"
mksrc m "$CH/main_pkg.z" 'from pkg.zzdpkg import tag\nprint(tag())\n'
compile "$CH/main_pkg.z"; expect "候选在源目录的子包 ⇒ 不喊" 0
mkdir -p "$TMP/conf"; mkmod CONF "$TMP/conf/zzdconf.z"; mkmain zzdconf zzdconf
compile "$CH/main_zzdconf.z" "ZETA_PYLIB=$TMP/conf"
expect "候选在显式配置的 \$ZETA_PYLIB ⇒ 不喊" 0
mksrc m "$CH/main_json.z" 'import json\nprint(1)\n'
compile "$CH/main_json.z"
if [[ "$(w1005)" == 0 && -z "$(resolved)" ]]; then
  echo "  ok   内置注册表命中（json）⇒ 不喊（实测：注册表短路，连 disk 搜索都不进，所以一条 imported module 行都没有）"
else
  echo "  FAIL 注册表命中那一档不该出现解析行（W1005=$(w1005)，$(resolved)）"; dump_err; rc=1
fi
mksrc m "$CH/main_pylib.z" 'import numpy\nprint(1)\n'
compile "$CH/main_pylib.z"
expect "候选在仓库 pylib 相对基 ⇒ 不喊" 0
[[ "$(resolved)" == "imported module \`numpy\` from pylib/numpy.z" ]] \
  && echo "  ok   且落点确实是相对基（$(resolved) —— 相对路径，故 #343 选题期的 abspath 分类把它错记成祖先）" \
  || { echo "  FAIL 相对基那一档落点不对：$(resolved)"; rc=1; }

echo "== C 翼：近优先，且只喊实际用的那个基 =="
mkmod NEAR "$CH/zzd_dup.z"; mkmod MID "$TMP/a/L1/L2/L3/L4/zzd_dup.z"; mkmod FAR "$TMP/a/L1/L2/zzd_dup.z"
mkmain zzd_dup zzd_dup
compile "$CH/main_zzd_dup.z"; expect "同目录与祖先各一份 ⇒ 取同目录、不喊" 0
[[ "$(resolved)" == *"$CH/zzd_dup.z"* ]] \
  && echo "  ok   近者胜（$(resolved)）" \
  || { echo "  FAIL 近者胜未成立：$(resolved)"; rc=1; }
rm -f "$CH/zzd_dup.z"
compile "$CH/main_zzd_dup.z"; expect "祖先 3 级与 5 级各一份 ⇒ 仍只喊 1 条" 1
want_level "喊的是实际用的那个基（3，不是 5）" "3,"
[[ "$(resolved)" == *"$TMP/a/L1/L2/L3/L4/zzd_dup.z"* ]] \
  && echo "  ok   祖先之间也是近者胜" \
  || { echo "  FAIL 祖先近者胜未成立：$(resolved)"; rc=1; }

echo "== D 翼：负控制 —— 放宽到 cap 之外不许喊 =="
rm -f "$TMP/a/L1/L2/L3/L4/zzd_dup.z" "$TMP/a/L1/L2/zzd_dup.z" "$TMP/a/L1/zzd_out.z" "$TMP/a/L1/L2/zzd_sym.z"
mkmain zzd_out zzd_out
mkmod OUT "$TMP/a/L1/zzd_out.z"             # 相对 L7 越出 6 级 = 第 7 个基，cap 之外
compile "$CH/main_zzd_out.z"
expect_noresolution "候选在越出 6 级（cap 之外）⇒ 不该喊" 0 'unknown member'
# 同一次编译里只让越界的那条喊。
rm -f "$TMP/a/L1/zzd_out.z"
mkmod MIX "$TMP/a/L1/L2/L3/zzdmix.z"
mksrc m "$CH/main_mix.z" 'import numpy\nfrom zzdmix import tag\nprint(tag())\n'
compile "$CH/main_mix.z"
expect "一次编译两条 import ⇒ 只有越界那条喊" 1
[[ "$(w1005_about numpy)" == 0 ]] \
  && echo "  ok   相对基那条（numpy）一声不响" \
  || { echo "  FAIL numpy 被误喊（$(w1005_about numpy) 条）"; rc=1; }
[[ "$(w1005_about zzdmix)" == 1 ]] \
  && echo "  ok   喊的是越界那条（zzdmix）" \
  || { echo "  FAIL zzdmix 没被点名"; rc=1; }

echo "== F 翼：危害实锤——祖先那份同名文件压过库面基 =="
# 盘据 1 的形状：从 tests/python_style/ 出发 rank 4 就是 $HOME，而相对基 `pylib` 排在
# 祖先之后（实测 B 翼：numpy 平时落在 pylib/numpy.z）。⇒ 家目录里一个同名 .z 顶掉库面。
# 这里在 mktemp 树里复刻"祖先基 vs 库面基"的先后，不把文件往 $HOME 里放。
mksrc m "$CH/main_shadow.z" 'from numpy import tag\nprint(tag())\n'
compile "$CH/main_shadow.z"
expect "先确认无祖先候选时它落在库面基（否则 F 翼比的是两份噪声）" 0
[[ "$(resolved)" == *"pylib/numpy.z"* ]] \
  && echo "  ok   参照档落在相对基（$(resolved)）" \
  || { echo "  FAIL 参照档没落在相对基：$(resolved)"; rc=1; }
mkmod SHADOW "$TMP/a/L1/L2/L3/L4/numpy.z"      # 与库文件同名，放在越出 3 级的祖先上
compile "$CH/main_shadow.z"
expect "同名文件放在越出 3 级的祖先上 ⇒ 压过库面基并喊" 1
want_level "且级数是 3" "3,"
[[ "$(resolved)" == *"$TMP/a/L1/L2/L3/L4/numpy.z"* ]] \
  && echo "  ok   解析落点已从 pylib/numpy.z 变成祖先那一份" \
  || { echo "  FAIL 祖先那份没顶掉库面：$(resolved)"; rc=1; }
rm -f "$TMP/a/L1/L2/L3/L4/numpy.z"

echo "== E：全语料 W1005 计数（读数，期望 0）=="
total=0; files=0
for f in $(grep -lE "^(from|import) [A-Za-z_]" tests/python_style/*.z tests/official/*.z 2>/dev/null); do
  files=$((files + 1))
  n=$("$ZETAC" --dump-mir "$f" -o "$TMP/w.o" 2>&1 >/dev/null | grep -c '\[W1005\]' || true)
  total=$((total + ${n:-0}))
  [[ "${n:-0}" != 0 ]] && echo "  越界：$f（$n 条）"
done
echo "  含 import 的语料文件 $files 个，W1005 合计 $total 条"
if [[ "$total" != 0 ]]; then
  echo "  FAIL 语料里出现了新的越界解析 —— 要么把夹具挪进源目录，要么把这条登记成已知并写进 roadmap"
  rc=1
fi

echo
if [[ $rc -eq 0 ]]; then echo "pysrc: 全部断言通过"; else echo "pysrc: 有 FAIL（rc=1）"; fi
exit $rc
