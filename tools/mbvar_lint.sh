#!/usr/bin/env bash
# tools/mbvar_lint.sh — 门禁第 14 步（批次 346）：脚本里的 `$VAR` 紧跟多字节字符
#
# 为什么需要这一步（不是洁癖，是实测缺陷）：
#   macOS 自带 bash 3.2 解析 `$p（…` 这类「裸变量名紧跟多字节字符」时，会把多字节
#   字符的首字节并进变量名（实得变量名 `p\xef`），于是 `set -u` 当场报
#   `line N: p?: unbound variable` 并杀掉整个脚本。批次 346 之前的
#   `tools/ignore_rule_inventory.sh:68` 就死在这一行上——表现是**门禁第 13 步整步崩掉
#   而不是判红**：崩掉的步骤与通过的步骤在汇总里都只剩一句 rc，光看数字分不出来。
#   全仓实测 40 处同族写法、分布在 10 个脚本里；中文文案 + `set -u` 是本仓默认风格，
#   所以这不是偶发，是**必然复发**。
#
# 判据：`$NAME`（或位置参数 `$1`）之后紧跟的字节 ≥ 0x80 ⇒ 判红。
#   修法一律加花括号：`${NAME}`——语义完全等价，只消除解析歧义。
#   单引号内的 `$VAR` 不是变量引用，按引号状态机跳过，不误判。
#
# 用法：tools/mbvar_lint.sh [--full]
#   默认扫 tools/ 与 tests/ 下的 *.sh；--full 带上仓库内其余（不含 target/、.git/）。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

FULL=0
[[ "${1:-}" == "--full" ]] && FULL=1

# bash 3.2（macOS 自带）没有 mapfile —— 用 while read 收集（本脚本自己踩过一次）
FILES=()
if [[ $FULL -eq 1 ]]; then
  while IFS= read -r f; do FILES+=("$f"); done \
    < <(find . -name '*.sh' -not -path './target/*' -not -path './.git/*' | sort)
else
  while IFS= read -r f; do FILES+=("$f"); done \
    < <(find tools tests -name '*.sh' 2>/dev/null | sort)
fi

report=$(python3 - ${FILES[@]+"${FILES[@]}"} <<'PY'
import sys

ID_START = set(b'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_')
ID_CONT = ID_START | set(b'0123456789')

def scan(path):
    """逐字节扫（不能用 str.isalnum：它是 Unicode 感知的，会把 0xEF 之类
    当成字母吞进变量名，再把半个多字节字符拿去 decode ⇒ UnicodeDecodeError）。"""
    hits = []
    try:
        data = open(path, 'rb').read()
    except OSError:
        return hits
    for lineno, line in enumerate(data.split(b'\n'), 1):
        i, n, in_single, in_double = 0, len(line), False, False
        while i < n:
            b = line[i]
            if in_single:                        # 单引号内是字面量，整段跳过
                if b == 0x27:
                    in_single = False
                i += 1; continue
            if b == 0x5C:                        # 单引号外：反斜杠转义下一个字节
                i += 2; continue
            if b == 0x27:
                in_single = True; i += 1; continue
            if b == 0x22:                        # 双引号只切换状态，内容仍需检查
                in_double = not in_double; i += 1; continue
            # 注释放行（双引号内的 `#` 不是注释）
            if b == 0x23 and not in_double and (i == 0 or line[i - 1] in b' \t;|&('):
                break
            if b == 0x24:                        # $
                j = i + 1
                if j < n and line[j] in ID_START:
                    while j < n and line[j] in ID_CONT:
                        j += 1
                elif j < n and 0x30 <= line[j] <= 0x39:
                    while j < n and 0x30 <= line[j] <= 0x39:
                        j += 1
                else:
                    i += 1; continue
                name = line[i + 1:j].decode('ascii', 'replace')
                if j < n and line[j] >= 0x80:
                    hits.append((lineno, name,
                                 line.decode('utf-8', 'replace').strip()[:70]))
                i = j; continue
            i += 1
    return hits

total = 0
for f in sys.argv[1:]:
    hs = scan(f)
    if hs:
        for lineno, name, text in hs:
            print("  FAIL %s:%d  $%s 紧跟多字节字符 → 改写为 ${%s}  << %s"
                  % (f, lineno, name, name, text))
    else:
        print("  ok   %s" % f)
    total += len(hs)
print("__HITS__ %d" % total)
PY
)

echo "$report" | grep -v '^__HITS__'
hits=$(echo "$report" | sed -n 's/^__HITS__ \([0-9]*\)$/\1/p')
checked=$(echo "$report" | grep -cE '^  (ok|FAIL) ')
printf '汇总：%s 个脚本，%s 处违规\n' "$checked" "$hits"

[[ "${hits:-0}" -eq 0 ]] || exit 1
exit 0
