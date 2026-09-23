#!/usr/bin/env bash
# tools/junk_swallow_inventory.sh —— W1004 判据的两侧 + 全语料读数
#
# 为什么需要它：stmt.rs 的语句兜底是"一个表达式就是一条语句"，所以任何解析器
# 没有规则的**前导词会被静默丢掉**，同一行的其余部分再被当第二条语句解析：
#   static mut counter: i64 = 0   → `static` 消失，剩下 `mut …` 当场中止（W1002 截断）
#   apple banana target: i64 = 7  → apple/banana 消失，target 被赋值
# 曾经在册的第三条 `import std::memory;`（绑到 `std`，`memory` 作为裸名语句消失）
# 已由批次 338 收进 `import` 的 `::` 翼，当前测得 0 处 W1004 —— 判据回归见
# tools/import_form_inventory.sh。留着这行是因为它标记了本判据的适用范围：
# 吞词发生在"解析器没有规则的前导词"上，规则补齐一处就少一处。
# W1004（批次 337）就在吞掉的那一刻说话。它的判据有排除项（隐式返回长得一样），
# 而"有排除项的判据"最容易在两翼上各翻一次：永远不打（判据写死了）和到处乱打
# （把隐式返回当 bug）。这两条各做成夹具，另加全语料计数当读数。
#
# 用法：
#   ./tools/junk_swallow_inventory.sh          # 只跑夹具断言（秒级）
#   ./tools/junk_swallow_inventory.sh --full    # 再加 official+python_style+corpus 计数
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ZETAC="${ZETAC:-$ROOT/target/release/zetac}"
[[ -x "$ZETAC" ]] || { echo "building zetac..." >&2; cargo build --release -p zetac -q; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
rc=0

# 夹具 1：应当**响**——两个前导词各响一次。
cat > "$TMP/swallow.z" <<'EOF'
fn f() -> i64 {
    apple banana target: i64 = 7
    return target
}
print(f())
EOF
# 夹具 2/3：应当**哑**——函数尾的隐式返回（裸名、裸字面值，含 `} else {` 那型）。
cat > "$TMP/ret-name.z" <<'EOF'
fn f() -> i64 {
    let target = 1
    target
}
print(f())
EOF
cat > "$TMP/ret-else.z" <<'EOF'
fn f() -> i64 {
    if 1 == 1 {
        let a = 2
        a
    } else {
        0
    }
}
print(f())
EOF

want() { # $1=描述 $2=期望 $3=实际
  if [[ "$2" == "$3" ]]; then echo "  ok   $1 → $3"; else echo "  FAIL $1 → 期望 ${2}，实得 $3"; rc=1; fi
}
hits() { "$ZETAC" "$1" -o "$TMP/o.o" 2>&1 | grep -c 'W1004'; }

echo "== 夹具（判据的两翼）"
want "前导词被吞 ⇒ 每个词响一次" 2 "$(hits "$TMP/swallow.z")"
want "尾表达式（裸名）⇒ 不响" 0 "$(hits "$TMP/ret-name.z")"
want "尾表达式（} else { 那型）⇒ 不响" 0 "$(hits "$TMP/ret-else.z")"
# 反证：吞词是真的发生了，不是只在诊断里——被吞那行仍然把值赋给了行尾的名字。
got=$("$ZETAC" "$TMP/swallow.z" 2>/dev/null | grep -v 'clang:' | head -1)
want "吞词后语句仍按'剩下部分'执行" 7 "$got"

if [[ "${1:-}" != --full ]]; then
  echo "junk_swallow: 夹具 rc=${rc}（--full 才有全语料计数）"; exit $rc
fi

echo "== 全语料 W1004 计数（读数；已知真阳性见 roadmap 批次 337）"
CORPUS_ROOT="${MIR_CORPUS:-$HOME/source/quant/REasyQuant/strategies}"
{
  find "$ROOT/tests/unit-tests" -name '*.z'
  ls "$ROOT"/tests/python_style/*.z 2>/dev/null
  find "$CORPUS_ROOT" -name '*.py' -not -path '*/.venv/*' 2>/dev/null
} | sort -u > "$TMP/files.txt"
total=0
while IFS= read -r f; do
  n=$(timeout 180 "$ZETAC" --dump-mir "$f" -o "$TMP/walk.o" 2>&1 >/dev/null | grep -c 'W1004' || true)
  total=$((total + n))
  [[ "$n" -gt 0 ]] && printf '  %-52s %s\n' "${f#"$ROOT"/}" "$n"
done < "$TMP/files.txt"
echo "  合计 $total 处 / $(wc -l < "$TMP/files.txt" | tr -d ' ') 文件"
echo "junk_swallow: 夹具 rc=${rc}（全语料只读数，不参与判定）"
exit $rc
