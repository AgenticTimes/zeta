#!/usr/bin/env bash
# tools/import_form_inventory.sh —— `import` 的形状值域（批次 338）
#
# 为什么需要它：`import` 走 python 那条规则，而 python 规则只认「a.b [as c]」这种
# 名字，且**不吞结尾的 `;`**。两个后果都实测过（修复前）：
#   import std;        → 剩下一个裸 `;` 当顶层项，没人解析得动 ⇒ 文件余部整段丢弃
#                        （真实用例：integration_test_program.z 因此丢了 19 行，含整个 fn main）
#   import std::memory;→ 绑成 std（错模块；日志会打 PY-A: imported module std），
#                        memory 掉成一条空语句（W1004 响）
# 而 `::` 路径在这个方言里**早有正解**：use std::memory;（top_level.rs 的
# parse_use_statement，含 ::{a, b} 群形）。所以本批不是新造语法，而是把 `import` 的
# `::` 那一翼接到 use 的同一份文法上（stmt.rs 调 top_level.rs::parse_use_targets），
# 不留第二个实现点。
#
# 判据分三翼，缺一翼锁不住：
#   等值翼 —— import 的每种 :: 形状，MIR 必须与 use 的对应形状逐字节相同；
#   中立翼 —— 结尾有没有 `;`，都不许改变 MIR、不许截断文件（python 那一翼也一样）；
#   尾巴翼 —— :: 之后跟 use 文法不认的东西（`as`、`=`、`;`）时必须**退回** python 腿。
#
# 尾巴翼是本批自己踩出来的：接到 use 文法的第一版把整条语句交给了它，结果
# `import std::memory as m;` 从"静默错绑但整个文件在"（0 诊断 / 171 行 MIR）退化成
# "W1002 吃掉整个 fn main"（39 行）—— 因为 `use` 没有 `as` 规则，被提交的尾巴成了
# 没人认的顶层项，`many0` 就此停下。逐形状量过 13 个尾巴形状（含空尾巴）后定了退回判据，
# 读数表在 roadmap 批次 338。**退回不等于修好**：`as` 那一族现在仍是静默错绑
# （工具把这条如实锁成读数，见"诚实读数"那行），正解要给 AstNode::Use 加别名字段。
#
# 期望值全部来自实测（修复后跑的），不是推测；三套语料的门禁读数见 roadmap 批次 338。
# 注意：PY-A 那几条依赖**模块搜索的起点**（同一份 `import numpy as np`，在仓库根会打
# 出 2 条 PY-A、在 /tmp 下一条都没有）——所以本脚本先 cd 到仓库根，读数才可复现。
#
# 用法：./tools/import_form_inventory.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
[[ -x "$ZETAC" ]] || { echo "building zetac..." >&2; cargo build --release -p zetac -q; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
rc=0
BODY=$'\nfn main() -> i64 {\n    let a = memory::DynamicArray::new(4)\n    return 0\n}\n'

fx() { printf '%s%s' "$1"$'\n' "$BODY" > "$TMP/$2.z"; }          # $1=首行语句 $2=夹具名
mir() { "$ZETAC" --dump-mir "$1" -o "$TMP/o.o" 2>/dev/null | sed '/^Compiled to /d'; }
diag() { "$ZETAC" --dump-mir "$1" -o "$TMP/o.o" 2>&1 >/dev/null \
         | grep -oE 'W100[0-9]|PY-A' | sort | uniq -c | tr -d ' \n'; }
same() { if diff -q <(mir "$2") <(mir "$3") >/dev/null; then echo "  ok   $1"; else
    echo "  FAIL $1 —— MIR 不同：$(diff <(mir "$2") <(mir "$3") | head -4 | tr '\n' ' ')"; rc=1; fi; }
want() { if [[ "$2" == "$3" ]]; then echo "  ok   $1 → ${3:-（空）}"; else
    echo "  FAIL $1 → 期望 ${2:-（空）}，实得 ${3:-（空）}"; rc=1; fi; }

# ── 等值翼的夹具：每个 import 形状配一个 use 参照 ──
fx 'import std::memory;'                   imp_path
fx 'use std::memory;'                      ref_path
fx 'import std::memory::capability;'       imp_deep
fx 'use std::memory::capability;'          ref_deep
fx 'import std::memory::{capability, thing};' imp_grp
fx 'use std::memory::{capability, thing};'    ref_grp
# ── 中立翼的夹具：同一形状的带分号/不带分号两版 ──
fx 'import std'          imp_nosemi
fx 'import std;'         imp_semi
fx 'import std.mem'      dot_nosemi
fx 'import std.mem;'     dot_semi
fx 'import os.path'      py_nosemi
fx 'import os.path;'     py_semi
fx 'import std::memory, ml;'  imp_mixed    # :: 段 + python 逗号列
fx 'import numpy as np'       py_as
fx 'import numpy as np;'      py_as_semi
# ── 尾巴族夹具：`::` 路径之后还能跟什么 ──
# 共享文法只在"剩下的文本还能被顶层循环认领"时才提交：`use` 没有 `as` 规则
# （`AstNode::Use` 没有装别名的字段），`=`/`;` 也开不了任何顶层项。这些形状一旦
# 被提交，尾巴就成了没人认的顶层项 ⇒ `many0` 停下 ⇒ 整个文件余部被丢掉。
fx 'import std::memory as m'  tail_as
fx 'import std::memory = 2'   tail_eq
fx 'import std::memory(1)'    tail_call
fx 'import std::memory[0]'    tail_index
fx 'import std::memory { x }' tail_block
fx 'import std::memory, ml'   tail_comma
fx 'import std::memory'       imp_ns
fx 'use std::memory'          ref_ns

echo "== 等值翼：import 的 :: 形状必须与 use 编译到同一份 MIR"
same "单段 ::（绑末段 memory，不再导入 std）" "$TMP/imp_path.z" "$TMP/ref_path.z"
same "多段 ::（绑 capability）"               "$TMP/imp_deep.z" "$TMP/ref_deep.z"
same "群形 ::{a, b}"                          "$TMP/imp_grp.z"  "$TMP/ref_grp.z"
want "import std::memory; 不再有任何诊断" "" "$(diag "$TMP/imp_path.z")"
# 反证「绑错模块」这条真发生过：import std; 会发出 std 的初始化调用；
# 接上 use 文法的 import std::memory; 不该有。
want "import std; 仍导入 std（std__init 计数）" 2 "$(mir "$TMP/imp_semi.z" | grep -c std__init)"
want "import std::memory; 不再导入 std（同上）" 0 "$(mir "$TMP/imp_path.z" | grep -c std__init)"

echo "== 中立翼：结尾分号不许改变形状，也不许截断文件"
same "import std == import std;"        "$TMP/imp_nosemi.z" "$TMP/imp_semi.z"
same "import std.mem == import std.mem;" "$TMP/dot_nosemi.z" "$TMP/dot_semi.z"
same "import os.path == import os.path;" "$TMP/py_nosemi.z"  "$TMP/py_semi.z"
same "import numpy as np == import numpy as np;" "$TMP/py_as.z" "$TMP/py_as_semi.z"
want "import std; 不截断"                ""      "$(grep -o W1002 <(diag "$TMP/imp_semi.z"))"
want "import std::memory, ml; 既不截断也不吞词" "1PY-A" "$(diag "$TMP/imp_mixed.z")"

echo "== 尾巴族：:: 之后跟 use 文法不认的东西时，必须退回而不是提交（提交=吃文件）"
notrunc() { want "$1 不截断" "" "$(grep -o W1002 <(diag "$TMP/$2.z"))"; }
notrunc "import a::b as c" tail_as
notrunc "import a::b = 2"  tail_eq
notrunc "import a::b(1)"   tail_call
notrunc "import a::b[0]"   tail_index
notrunc "import a::b { x }" tail_block
notrunc "import a::b, c"   tail_comma
# 退回不等于修好：`as` 这一族仍是静默错绑（PY-A 在、无 W1004），已登记 OPEN 1。
want "import a::b as c 退回后仍是错绑（诚实读数）" "1PY-A" "$(diag "$TMP/tail_as.z")"
same "裸 ::（不带分号）与 use 等值" "$TMP/imp_ns.z" "$TMP/ref_ns.z"

echo "== 别改坏 python 那一翼（无 :: 的形状仍走 python 标记，实测口径）"
want "import os.path; → 只有一条 PY-A（unknown 模块）"     "1PY-A" "$(diag "$TMP/py_semi.z")"

echo "== 真实用例：截断点必须已经走过 import 那几行（只许更好，不许退回）"
real="$ROOT/tests/unit-tests/integration_test_program.z"
trunc_line=$(timeout 180 "$ZETAC" --dump-mir "$real" -o "$TMP/r.o" 2>&1 >/dev/null \
  | sed -nE 's/.*\[W1002\].*:([0-9]+):.*/\1/p' | head -1)
if [[ -z "$trunc_line" ]]; then
  echo "  ok   该档已无 W1002（本批之后又有人修掉了 fn main 的形状）"
elif [[ "$trunc_line" -ge 8 ]]; then
  echo "  ok   截断点在 :$trunc_line（≥8：import 那 4 行已被完整认领，剩下的另案）"
else
  echo "  FAIL 截断点退回 :$trunc_line —— import 的尾巴又开始吃文件了"; rc=1
fi

echo "import_form: rc=$rc"
exit $rc
