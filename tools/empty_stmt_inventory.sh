#!/usr/bin/env bash
# tools/empty_stmt_inventory.sh —— 裸 `;` 的值域（批次 339，任务 #67）
#
# 为什么需要它：顶层语句循环是 `many0(顶层项)`，而**没有任何规则认得一个裸 `;`**，
# 于是它成为"第一个解析不动的顶层项" ⇒ 循环当场停下 ⇒ **文件余部整段丢弃**（W1002）。
# 修复前实测（每个形状后面都挂着 `fn tail_marker()` + `fn main()`，用批次 338 的
# 二进制跑本脚本反证，读数见 roadmap 批次 339）：13 个位置里
#   **11 个直接截断**（首项 `;` / `;;` / `;;;` / 带缩进的 `;` / 注释后 / 表达式语句
#   后 / fn 定义后 / def 定义后 / use 后 / import 后 / mod 体内）——MIR 从 35 行掉到
#   23 行，整个程序没了，只剩合成的空 main；
#   **1 个出声但不丢文本**（文件末尾的 `;`：后面本来没东西，MIR 不变，白响一条）；
#   **1 个完全中立**（`x = 1;` 之后的 `;`——赋值规则把分号吃了）；块内的 `;` 也早就
#   中立（B 翼锁的就是"别把它改坏"）。
# ⇒ 一个字符能不能删掉整个程序，取决于"前面恰好跑了哪条规则"。本批闭合这条值域：
#   `;` 是**顶层空项**，与块内空语句同一语义，且只在一个地方实现
#   （top_level.rs::parse_top_level_entry —— 文件循环、两处 `mod { … }` 体、可选的
#   恢复循环共用它，不留第二个实现点）。
#
# 跟手收益：批次 338 给 parse_python_import 打的"自己吃掉结尾分号"补丁由此成为
# 冗余（同一条规则不该有两个落点）。删掉之后实测：22 条 import 断言仍绿，且顶层/
# def 体内/fn 体内三种 `import os;` 都不截断 —— 这三条在本批 E 段另锁一遍。
#
# 四翼 + 一段回归锁，缺一翼锁不住：
#   中立翼（A/B）—— 同一程序加不加裸 `;` 必须逐字节同一份 MIR；
#   截断翼（A/B）—— 加不加都必须看得见后面的 `fn tail_marker()`，且不许出 W1002；
#   两路翼（C）  —— 默认路径与 ZETA_PARSE_RECOVER=1 必须给出同一份 MIR 且都不出声
#                  （修复前同一个 `;` 是"默认截断 / 恢复出声"，两路各说各话）；
#   负控制（D）  —— 接受空项不等于放宽吞词判据：`!!!` 仍然截断，`q;` 与 `static mut …`
#                  仍然响 W1004（批次 337 的判据与本批共用语句兜底，最容易顺手关掉）。
#   E 段 —— "删掉 338 补丁后 import 三档不坏"。它**两版二进制都绿**：锁的是"删了不
#           坏"，不是"修前坏"，不要拿它当反证。（反证读数：批次 338 的二进制跑本脚本
#           = 29 条断言 FAIL / 39 条 ok，A 翼 23、C 翼 5、D 翼 1、B/E 翼 0。）
#
# 期望值全部来自实测（修复后跑的），不是推测。全语料读数（truncation_inventory
# 修复前后对照）记在 roadmap 批次 339。
#
# 用法：./tools/empty_stmt_inventory.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
[[ -x "$ZETAC" ]] || { echo "building zetac..." >&2; cargo build --release -p zetac -q; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
rc=0
# 顶层形状后面一律挂这段：`kept` 拿 `== MIR tail_marker ==` 当"文件没被截断"的证据。
TAIL=$'\nfn tail_marker() -> i64 {\n    return 7\n}\n\nfn main() {\n    print(tail_marker());\n}\n'

fx()   { printf '%b' "$2" > "$TMP/$1.z"; }                       # %b：\n 展开成真换行
mir()  { "$ZETAC" --dump-mir "$1" -o "$TMP/o.o" 2>/dev/null | sed '/^Compiled to /d'; }
rmir() { ZETA_PARSE_RECOVER=1 "$ZETAC" --dump-mir "$1" -o "$TMP/o.o" 2>/dev/null | sed '/^Compiled to /d'; }
diag() { "$ZETAC" --dump-mir "$1" -o "$TMP/o.o" 2>&1 >/dev/null | grep -oE 'W100[0-9]' | sort -u | tr '\n' ','; }

same() { if diff -q <(mir "$2") <(mir "$3") >/dev/null; then echo "  ok   $1"; else
    echo "  FAIL $1 —— MIR 不同：$(diff <(mir "$2") <(mir "$3") | head -4 | tr '\n' ' ')"; rc=1; fi; }
kept() { if ! grep -q '^== MIR tail_marker ==' <(mir "$2"); then
    echo "  FAIL $1 —— tail_marker 不在 MIR 里（文件被截断）"; rc=1; return; fi
  if grep -q '\[W1002\]' <("$ZETAC" --dump-mir "$2" -o "$TMP/o.o" 2>&1 >/dev/null); then
    echo "  FAIL $1 —— 仍报 W1002"; rc=1; return; fi
  echo "  ok   $1"; }
want() { if [[ "$2" == "$3" ]]; then echo "  ok   $1 → ${3:-（无诊断）}"; else
    echo "  FAIL $1 → 期望 ${2:-（无诊断）}，实得 ${3:-（无诊断）}"; rc=1; fi; }

# pair 名字 不加;的源码 加裸;的源码 —— 一次上三条断言（中立 + 两版截断证据）
pair() {  # $1=说明 $2=不加 ; 的源码 $3=加裸 ; 的源码 $4=夹具名前缀
  fx "${4}_no" "$2"; fx "${4}_yes" "$3"
  same "中立翼：$1（加/不加逐字节同一份 MIR）" "$TMP/${4}_no.z" "$TMP/${4}_yes.z"
  kept "截断翼：$1（加了 ; 尾巴仍在）"         "$TMP/${4}_yes.z"
  kept "截断翼：$1（参照版尾巴也在）"          "$TMP/${4}_no.z"
}

echo "== A 翼：顶层裸 ; 的 13 个位置 =="
pair "首项单个 ;"       "$TAIL"                     ";$TAIL"                      a_first
pair "首项 ;;"          "$TAIL"                     ";;$TAIL"                     a_double
pair "首项 ;;;"         "$TAIL"                     ";;;$TAIL"                    a_triple
pair "带缩进的顶层 ;"   "$TAIL"                     "  ;$TAIL"                    a_indent
pair "注释之后"         "# c$TAIL"                  "# c\n;$TAIL"                 a_comment
pair "赋值语句之后"     "x = 1;$TAIL"               "x = 1;\n;$TAIL"              a_assign
pair "表达式语句之后"   "print(1);$TAIL"            "print(1);\n;$TAIL"           a_expr
pair "fn 定义之后"      "fn f() {\n    return 1\n}$TAIL" "fn f() {\n    return 1\n}\n;$TAIL" a_fn
pair "def 定义之后"     "def f(a):\n    return a$TAIL" "def f(a):\n    return a\n;$TAIL" a_def
pair "use 之后"         "use std::io;$TAIL"         "use std::io;\n;$TAIL"        a_use
pair "import 之后"      "import os;$TAIL"           "import os;\n;$TAIL"          a_import
pair "mod 体内"         "mod m {\n    fn g() {\n        return 1\n    }\n}$TAIL" \
                        "mod m {\n    ;\n    fn g() {\n        return 1\n    }\n}$TAIL" a_mod
pair "文件末尾"         "$TAIL"                     "${TAIL}\n;\n"                a_eof

echo "== B 翼：块内 / 控制流体内（修复前已中立，这一翼防的是把它改坏）=="
# B 翼自带 fn main，所以只挂 tail_marker 这一个标记函数（挂 $TAIL 会重复定义 main）。
TAILFN=$'\nfn tail_marker() -> i64 {\n    return 7\n}\n'
pair "块首"             'fn main() {\n    print(tail_marker());\n}\n'"$TAILFN" \
                        'fn main() {\n    ;\n    print(tail_marker());\n}\n'"$TAILFN" b_head
pair "块内语句之后"     'fn main() {\n    print(tail_marker());\n}\n'"$TAILFN" \
                        'fn main() {\n    print(tail_marker());\n    ;\n}\n'"$TAILFN" b_mid
pair "块内 ;;"           'fn main() {\n    print(tail_marker());\n}\n'"$TAILFN" \
                         'fn main() {\n    ;;\n    print(tail_marker());\n}\n'"$TAILFN" b_double
pair "if 体首条是 ;"    'if 1:\n    print(2)\n'"$TAILFN" \
                        'if 1:\n    ;\n    print(2)\n'"$TAILFN" b_if
pair "while 体首条是 ;" 'while 0:\n    print(2)\n'"$TAILFN" \
                        'while 0:\n    ;\n    print(2)\n'"$TAILFN" b_while

echo "== C 翼：两路（默认 / ZETA_PARSE_RECOVER=1）必须同 MIR 且都不出声 =="
for n in a_first a_import a_use a_expr a_mod b_head; do
  f="$TMP/${n}_yes.z"
  if ! diff -q <(rmir "$f") <(mir "$f") >/dev/null; then
    echo "  FAIL $n —— 两路 MIR 不同：$(diff <(rmir "$f") <(mir "$f") | head -4 | tr '\n' ' ')"; rc=1; continue
  fi
  d="$(ZETA_PARSE_RECOVER=1 "$ZETAC" --dump-mir "$f" -o "$TMP/o.o" 2>&1 >/dev/null | grep -oE 'W100[0-9]' | sort -u | tr '\n' ',')"
  want "两路同 MIR 且恢复路径不出声（$n）" "" "$d"
done

echo "== D 翼：负控制 —— 接受空项不等于放宽吞词判据 =="
fx n_junk   "!!!$TAIL"
fx n_q      'q;\nprint(1)\n'
fx n_static 'static mut counter: i64 = 0\nprint(1)\n'
fx n_semi   ";$TAIL"
want "junk 三连叹号仍然截断（W1002 还在响）"  "W1002,"       "$(diag "$TMP/n_junk.z")"
want "裸 ; 一声不出（不该有 W100x）"          ""             "$(diag "$TMP/n_semi.z")"
want "q; 仍然响 W1004（337 吞词判据未失效）"  "W1004,"       "$(diag "$TMP/n_q.z")"
# 期望值来自实测（修复前后同一条：W1002 是 337 那批就有的读数，不是本批新增）
want "static mut 仍然响 W1002+W1004"          "W1002,W1004," "$(diag "$TMP/n_static.z")"

echo "== E：批次 338 的分号补丁已删（同一规则不留两个落点），import 三档另锁 =="
fx e_top "import os;\nprint(1);$TAIL"
fx e_def "def f():\n    import os;\n    return 1\n$TAIL"
fx e_fn  "fn g() {\n    import os;\n    return 1\n}$TAIL"
kept "顶层 import os; 之后不截断" "$TMP/e_top.z"
kept "def 体内 import os; 之后不截断" "$TMP/e_def.z"
kept "fn 体内 import os; 之后不截断" "$TMP/e_fn.z"
fx   e_nosemi "import os$TAIL"
fx   e_semi   "import os;$TAIL"
same "import 结尾有没有 ; 都不改变 MIR" "$TMP/e_nosemi.z" "$TMP/e_semi.z"

echo
if [[ $rc -eq 0 ]]; then echo "empty_stmt: 全部断言通过"; else echo "empty_stmt: 有 FAIL（rc=1）"; fi
exit $rc
