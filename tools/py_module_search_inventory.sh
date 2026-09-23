#!/usr/bin/env bash
# tools/py_module_search_inventory.sh —— PY-A 模块解析的"越界"值域（批次 343，任务 #70）
#   ＋ 库面基的位置独立性（批次 351，任务 #76 ② = 本文件 G 翼）
#
# 为什么需要它：`find_py_module_file_ranked`（src/middle/resolver/resolver.rs:2238）把**被编
# 译文件所在目录往上 5 级**连同它自己一起列进搜索基（顺序：source dir=0 → 祖先 1..5 →
# $ZETA_PYLIB → ~/.zeta/packages → 库面基 pylib → build/stubs）。祖先链是为"工程根
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
# 七翼：
#   A 翼 —— 越界必喊：候选分别在越出 1 / 3 / 5 级的基上，各恰好 1 条 W1005，且报出的
#           级数与实际一致（喊的是"越了几级"，不是一个布尔）；
#   A2 翼 —— 边界：5 级是 cap 内最深一档 ⇒ 仍能链接出 `tag`；4 级不喊（B 翼）之外，
#           本翼只钉"级数最大那档确实解析到了"，防止把 cap 读成"4 级"；
#   B 翼 —— 不该喊的一个都不喊：同目录（0 级）、子包、${ZETA_PYLIB}、仓库 `pylib`（cwd 拼法）；
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
#   G 翼 —— 批次 351（#76 ②）：库面基 `pylib` 不再随 CWD 消失。修前实测：同一份编译器
#           + 同一份绝对路径源码，仓库根 rc=0 / 1045 行 MIR，`/tmp` 下 rc=1、
#           `_arange` undefined / 158 行。三档：G1 两个 CWD 同一份产物；G2 cwd 里有同名
#           `pylib` ⇒ 喊 1 条 W1006 且**两个基都搜**（cwd 优先次序不变）；G3 负控制 ——
#           把编译器拷进一棵没有 `pylib` 的裸树 ⇒ 必须退回修前的样子并出声。G3 存在的
#           理由：它钉住"库面确实解析到了"，否则 G1 的"两次逐字相同"可以靠两份残骸蒙过。
#   E 段 —— 全语料 W1005 计数，期望 0；同一趟编译顺带数 W1006，也期望 0。批次 352 补
#           了口径自证：语料是数组，每个目录先自证"顶层有 .z 且含 import"，再要两个下限
#           （文件数 ≥50、真解析到模块的文件数 ≥20）——此前 `tests/official/*.z` 匹配
#           0 个文件而报错被 `2>/dev/null` 吞掉，"全语料"三批以来只有 python_style 一档。
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
# 每档一个主文件，import 的模块名 = ${1}（本档唯一候选所在的名字）。
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
  echo "  ok   ${1}（W1005=${n}）"
  echo "         $(resolved)"
}
# 只用于 D 翼：这次**就是要它解析不到**。$3=解析不到时本该出现的既有诊断。
expect_noresolution() {
  local n; n="$(w1005)"
  if [[ "$2" != "$n" ]]; then echo "  FAIL $1 —— 期望 $2 条，实得 $n 条"; dump_err; rc=1; return; fi
  if grep -q 'imported module' "$TMP/err"; then
    echo "  FAIL $1 —— 它居然解析成功了，这一档没测到 cap 之外"; dump_err; rc=1; return; fi
  if ! grep -q "$3" "$TMP/err"; then
    echo "  FAIL $1 —— 解析不到时的既有路径（${3}）不见了"; dump_err; rc=1; return; fi
  echo "  ok   ${1}（W1005=${n}，且仍走 $3 退化路径）"
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
# 盘据 1 的形状：从 tests/python_style/ 出发 rank 4 就是 ${HOME}，而相对基 `pylib` 排在
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

echo "== G 翼：库面基不随 CWD 消失（批次 351，#76 ②）=="
# 盘据形状（351 修之前实测）：同一份编译器 + 同一份**绝对路径**源码，仓库根 rc=0 /
# MIR 1045 行，`/tmp` 下 rc=1、`_arange` undefined、MIR 158 行 —— 因为搜索基里那条
# `pylib` 是**裸相对路径**（修前 resolver.rs:2274 那条 `bases.push(PathBuf::from("pylib"))`），
# 库面在不在取决于在哪儿起编译器。
# 修法是照抄 src/main.rs:440 `find_runtime_obj`（它早就为运行时 .o 治过同一个病）：
# cwd 写法仍然第一（所以仓库根的读数逐字不变），其后追加从可执行文件往上 4 级的基。
#
# 这一翼自带的负控制是 G3 档：把编译器拷进一棵没有 `pylib` 的光树 ⇒ 同一个源文件
# 立刻退回"库面消失"那副样子。⇒ G1 的"两次输出逐字相同"不可能靠"本来就没有库面"
# 蒙过去：那样两档比的是两份不同的产物，而这里要求的是一份完整的。
G_SRC="$TMP/g/rep.z"
mkdir -p "$TMP/g" "$TMP/elsewhere"
mksrc g "$G_SRC" 'import numpy as np\nxs = np.arange(5)\nprint(len(xs))\n'
# $1=起编译的目录，$2=源文件（绝对路径）
gm() { ( cd "$1" && "$ZETAC" --dump-mir "$2" -o "$TMP/g.o" ) >"$TMP/g.out" 2>"$TMP/g.err"; }
gres() { grep -oE "imported module \`numpy\` from .*" "$TMP/g.err" | tail -1; }
g1006() { grep -c '\[W1006\]' "$TMP/g.err" || true; }

echo "-- G1：仓库根 vs 一个没有 pylib 的目录 ⇒ 同一份产物"
gm "$ROOT" "$G_SRC"
if [[ "$(g1006)" != 0 ]] || [[ -z "$(gres)" ]]; then
  echo "  FAIL 仓库根参照档本身就不干净（W1006=$(g1006) 条，落点=${gres:-（无）}）—— 下面比的是噪声"; rc=1
fi
cp "$TMP/g.out" "$TMP/g.mir.root"
LINES=$(grep -c . "$TMP/g.mir.root")
if [[ "$LINES" -gt 500 ]]; then
  echo "  ok   参照产物完整（$LINES 行 MIR；修前在别的 CWD 只编得出 158 行残骸）"
else
  echo "  FAIL 参照产物只有 $LINES 行 —— 两档都在比残骸，G1 是空断言"; rc=1
fi
gm "$TMP/elsewhere" "$G_SRC"
if [[ "$(g1006)" != 0 ]]; then echo "  FAIL 换个 CWD 不该多出告警（W1006=$(g1006)）"; rc=1; fi
if ! grep -q 'imported module `numpy`' "$TMP/g.err"; then
  echo "  FAIL 换 CWD 后库面仍然消失了（stderr 里没有 imported module 行）"; rc=1
elif diff -q "$TMP/g.mir.root" "$TMP/g.out" >/dev/null && [[ -s "$TMP/g.out" ]]; then
  echo "  ok   仓库根 vs 裸目录 → 同一份 MIR、同样 rc=0（落点：$(gres)）"
else
  echo "  FAIL 同一个源文件在两个 CWD 下产物不一致 —— 库面仍随 CWD 变化"
  diff "$TMP/g.mir.root" "$TMP/g.out" | head -5 | sed -n 's/^/         /p'; rc=1
fi

echo "-- G2：cwd 里有一个同名 \`pylib\` ⇒ 歧义出声（W1006），两个基都搜"
mkdir -p "$TMP/decoy/pylib"
mkmod HELLO "$TMP/decoy/pylib/zzd_decoy.z"
mksrc d "$TMP/decoy/main.z" 'from zzd_decoy import tag\nimport numpy as np\nprint(tag())\nprint(len(np.arange(3)))\n'
( cd "$TMP/decoy" && "$ZETAC" --dump-mir main.z -o "$TMP/g2.o" ) >"$TMP/g2.out" 2>"$TMP/g2.err"
d1006=$(grep -c '\[W1006\]' "$TMP/g2.err" || true)
if [[ "$d1006" == 1 ]]; then
  echo "  ok   两个不同的 pylib ⇒ 恰好 1 条 W1006（不是 0 条静默择优）"
else
  echo "  FAIL 歧义档 W1006=$d1006 条，期望 1 条"; sed -n '1,4p' "$TMP/g2.err" | sed -n 's/^/         /p'; rc=1
fi
grep -q 'imported module `zzd_decoy` from pylib/zzd_decoy.z' "$TMP/g2.err" \
  && echo "  ok   cwd 那份仍然优先（相对落点逐字未变）" \
  || { echo "  FAIL cwd 基的优先次序变了"; sed -n 's/^/         /p' "$TMP/g2.err" | head -4; rc=1; }
if grep -q 'imported module `numpy`' "$TMP/g2.err"; then
  echo "  ok   库面没被 cwd 那份顶掉：$(grep -oE 'imported module `numpy` from .*' "$TMP/g2.err" | tail -1)"
else
  echo "  FAIL 只搜了 cwd 的 pylib ⇒ 两个基都搜没成立"; rc=1
fi

echo "-- G3 负控制：可执行文件旁边也没有 pylib ⇒ 必须出声，旧退化路径仍在"
mkdir -p "$TMP/solo/bin" "$TMP/solo/work"
cp "$ZETAC" "$TMP/solo/bin/zetac"
mksrc s "$TMP/solo/work/s.z" 'import numpy as np\nfrom zzd_alpha import a\nprint(1)\n'
# 必须调**拷贝出来的那一个**：第一版在这里写了 "$ZETAC"，于是档位跑的是仓库编译器，
# 它顺着自己的树找到了 pylib ⇒ 库面照样解析到、W1006 一条没有 —— 三条 FAIL 全建在
# 错误的二进制上。P0 那条断言钉的就是这一点：告警里得出现这棵裸树的路径。
# 匹配尾段 `solo/bin/zetac` 而不是全路径 —— 仓库二进制里没有这一段，而 current_exe
# 可能把 $TMP 的 /var 前缀写成 /private/var，全路径匹配会假红。
( cd "$TMP/solo/work" && ZETA_PYLIB= "$TMP/solo/bin/zetac" --dump-mir s.z ) >"$TMP/g3.out" 2>"$TMP/g3.err"
n1006=$(grep -c '\[W1006\]' "$TMP/g3.err" || true)
grep -q "solo/bin/zetac" "$TMP/g3.err" \
  && echo "  ok   P0 喊的是这棵裸树里的编译器自己（告警里有 solo/bin/zetac ⇒ 档位没跑回仓库二进制）" \
  || { echo "  FAIL P0 告警里没有裸树路径 —— 这一档跑的不是拷贝的编译器，下面三条断言全部无效"; rc=1; }
if [[ "$n1006" == 1 ]]; then
  echo "  ok   哪儿都没有 pylib ⇒ 喊 1 条（一次编译两条 import 也只一条：每进程一次，不是每 import 一次）"
else
  echo "  FAIL 皆无档 W1006=$n1006 条，期望 1 条"; sed -n '1,6p' "$TMP/g3.err" | sed -n 's/^/         /p'; rc=1
fi
grep -q 'unknown member' "$TMP/g3.err" \
  && echo "  ok   既有的退化诊断仍在（新告警是补充，不是替换）" \
  || { echo "  FAIL 既有的 unknown member 路径不见了"; rc=1; }
if grep -q 'imported module' "$TMP/g3.err"; then
  echo "  FAIL 裸树里它居然解析到了库面 —— G1 的「相同」就是两份噪声"; sed -n 's/^/         /p' "$TMP/g3.err" | head -4; rc=1
fi
if grep -q "$ROOT/pylib" "$TMP/g3.err"; then
  echo "  FAIL 皆无档报出的路径里出现了仓库 pylib —— 搜索基没跟着可执行文件走"; rc=1
else
  echo "  ok   告警报的是「找过了哪儿」（裸树里那两个落点），没把仓库那份混进来"
fi
[[ -s "$TMP/solo/bin/zetac" ]] || { echo "  FAIL 拷贝出来的编译器是空的"; rc=1; }
echo "== E：全语料越界/库面基计数（读数，W1005 与 W1006 都期望 0）=="
# 同一趟编译同时数两个码 ⇒ 不为 W1006 再跑一遍全语料。W1006 在仓库根的语料上必须
# 是 0：既没有"两个 pylib 打架"（歧义），也没有"哪儿都找不到"（失踪）。
#
# 语料是**数组**，每个目录先自证非空再进循环。343 起的写法是一行
# `grep -lE "^(from|import) …" tests/python_style/*.z tests/official/*.z 2>/dev/null`，
# 而 `tests/official/` **从来没存在过**（official 语料的真路径是 `tests/unit-tests/`，
# 194 个 = 门禁第 1 步那个 194）⇒ 那条 glob 匹配 0 个文件、`No such file or directory`
# 被 `2>/dev/null` 吃掉，于是三批以来"全语料"实际只有 python_style 一档（117 个）。
# 接进 unit-tests 后是 119 个，但新增的 2 个含 import 的文件走的是 `import std::memory;`
# 这种 Rust 拼法，**一条 PY-A 解析都不产生** —— 接它的价值不在信号，在"清单不再靠一条
# 没人核对的 glob"。
CORPUS_DIRS=(tests/python_style tests/unit-tests)
# 两条下限：语料文件数、以及"真的走到 PY-A 模块解析"的文件数（实测 119 / 55）。下限取
# 实测的一半再低一档，目的是抓住"glob 又空了 / 编译集体失败"，不是钉死精确数。
IMPORT_RE='^[[:space:]]*(from|import) [A-Za-z_]'
corpus=()
for d in "${CORPUS_DIRS[@]}"; do
  dz=$(find "$d" -maxdepth 1 -name '*.z' 2>/dev/null | grep -c . ); dz=${dz:-0}
  if [[ "$dz" == 0 ]]; then
    echo "  FAIL 语料目录 $d 顶层没有 .z —— 这一档匹配 0 个文件，'全语料'是空的（343 的 tests/official 就是这么静默的）"
    rc=1; continue
  fi
  hits=()
  while IFS= read -r hf; do hits+=("$hf"); done < <(grep -lE "$IMPORT_RE" "$d"/*.z 2>/dev/null)
  if [[ ${#hits[@]} -eq 0 ]]; then
    echo "  FAIL 语料目录 $d 有 $dz 个 .z，却没有一个含 import/from —— 它给 E 段贡献 0 信号：要么接错语料，要么把它从清单里删掉"
    rc=1; continue
  fi
  echo "  ok   语料目录 $d 自证非空（顶层 .z=${dz}，含 import=${#hits[@]}）"
  corpus+=("${hits[@]}")
done
if [[ ${#corpus[@]} -ge 50 ]]; then
  echo "  ok   语料合计 ${#corpus[@]} 个文件（下限 50；343~351 实际只有 117 个，因为那条空 glob 的报错被吞了）"
else
  echo "  FAIL 语料合计只有 ${#corpus[@]} 个（下限 50）—— 有语料目录又空了，下面的 0 是空断言"
  rc=1
fi
total=0; w6_total=0; files=0; res_hit=0
# `${corpus[@]+…}`：本脚本开了 `set -u`，bash 3.2 下空数组展开成 `"${corpus[@]}"` 会直接
# 报 unbound variable 而**崩在判据之前**——那时脚本非 0，但打不出"哪条红了"。
for f in ${corpus[@]+"${corpus[@]}"}; do
  files=$((files + 1))
  "$ZETAC" --dump-mir "$f" -o "$TMP/w.o" >/dev/null 2>"$TMP/w.err"
  n=$(grep -c '\[W1005\]' "$TMP/w.err" || true)
  m=$(grep -c '\[W1006\]' "$TMP/w.err" || true)
  r=$(grep -c 'PY-A: imported module' "$TMP/w.err" || true)
  total=$((total + ${n:-0}))
  w6_total=$((w6_total + ${m:-0}))
  [[ "${r:-0}" != 0 ]] && res_hit=$((res_hit + 1))
  [[ "${n:-0}" != 0 ]] && echo "  越界：${f}（$n 条）"
  [[ "${m:-0}" != 0 ]] && echo "  库面基：${f}（$m 条）"
done
echo "  含 import 的语料文件 $files 个（其中 $res_hit 个解析到模块），W1005 合计 $total 条，W1006 合计 $w6_total 条"
if [[ $res_hit -ge 20 ]]; then
  echo "  ok   语料里 $res_hit 个文件真的走到 PY-A 模块解析（下限 20）—— 上面那个 0 是'搜遍了没越界'，不是'根本没在搜'"
else
  echo "  FAIL 只有 $res_hit 个文件解析到模块（下限 20）—— 编译侧坏了或语料被换掉了，W1005=0 是空断言"
  rc=1
fi
if [[ "$total" != 0 ]]; then
  echo "  FAIL 语料里出现了新的越界解析 —— 要么把夹具挪进源目录，要么把这条登记成已知并写进 roadmap"
  rc=1
fi
if [[ "$w6_total" != 0 ]]; then
  echo "  FAIL 语料里出现了 W1006 —— 门禁在仓库根跑，歧义和失踪都说明搜索基被写坏了"
  rc=1
fi

echo
if [[ $rc -eq 0 ]]; then echo "pysrc: 全部断言通过"; else echo "pysrc: 有 FAIL（rc=1）"; fi
exit $rc
