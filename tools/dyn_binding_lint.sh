#!/usr/bin/env bash
# tools/dyn_binding_lint.sh — 门禁第 17 步（批次 428）：`[dynamic]` 运行期绑定两侧一致
#
# 为什么需要这一步：`codegen.rs` 里 `dyn_runtime_bound` 有一份"哪些
# `[dynamic]<接收者>::<成员>` 真的由 C 运行期实现"的例外表。批次 428 的判据是
# 「不在这张表里的 ghost ⇒ 抛异常」，于是这张表两侧都会腐烂：
#   · 运行期**新增**一个 `__asm__("_[dynamic]…")` 绑定而 Rust 表没跟上 ⇒
#     一条能用的绑定被判成"成员缺失"，当场把可用的调用点改成抛异常（回归）；
#   · Rust 表写了运行期**没有**的名字 ⇒ 那条 ghost 仍旧走到 extern declare，
#     链接期把整份二进制拖没（就是 428 要治的那个病）。
# 两种腐烂都不会有任何测试变红——Rust 表是字符串常量，C 标签是汇编符号名，
# 中间没有类型、没有引用、没有编译期关系。所以只能当场核对。
#
# 判据：集合相等（双向）。
#   C 侧 = `runtime/*.c` 里的 `__asm__("_[dynamic]<接收者>__<成员>")` 标签，
#          **排除** `runtime/unavailable_stubs.c`：那 124 条是批次 156 为了让本地
#          二进制**能链接**而写的弱桩，每条一被调用就 abort 自己的名字，
#          它们不是"运行期真实现了这个成员"。
#   Rust 侧 = `DYN_RUNTIME_BINDINGS` 常量里的字面量。
#   归一：去掉 C 标签的前导 `_`，把接收者后的第一个 `__` 换成 `::`
#         （接收者类型名里没有 `__`，成员名可能有 dunder，所以按第一个切）。
#
# 用法：tools/dyn_binding_lint.sh

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

report=$(python3 - "$ROOT" <<'PY'
import glob, os, re, sys

root = sys.argv[1]
SPLIT = re.compile(r'^_\[dynamic\]([^\]]+?)__(.+)$')

# ── C 侧：真实现（排除弱桩台账）──
c_labels = {}
for path in sorted(glob.glob(os.path.join(root, 'runtime', '*.c'))):
    if os.path.basename(path) == 'unavailable_stubs.c':
        continue
    with open(path, encoding='utf-8', errors='replace') as f:
        for lineno, line in enumerate(f, 1):
            for m in re.finditer(r'__asm__\("(_\[dynamic\][^"]*)"\)', line):
                sp = SPLIT.match(m.group(1))
                if not sp:
                    print('  FAIL runtime/%s:%d 标签形状不认识: %s'
                          % (os.path.basename(path), lineno, m.group(1)))
                    continue
                c_labels.setdefault('[dynamic]%s::%s' % (sp.group(1), sp.group(2)),
                                    '%s:%d' % (os.path.basename(path), lineno))

# ── Rust 侧：例外表 ──
src = os.path.join(root, 'src', 'backend', 'codegen', 'codegen.rs')
text = open(src, encoding='utf-8', errors='replace').read()
block = re.search(r'const DYN_RUNTIME_BINDINGS: &\[&str\] = &\[(.*?)\];', text, re.S)
rust = {}
if block is None:
    print('  FAIL codegen.rs 里找不到 DYN_RUNTIME_BINDINGS 常量（判据失效）')
else:
    for m in re.finditer(r'"([^"]+)"', block.group(1)):
        name = m.group(1)
        rust[name.replace('__', '::', 1)] = name

print('__C__ %d' % len(c_labels))
print('__R__ %d' % len(rust))

for name in sorted(set(c_labels) | set(rust)):
    if name in c_labels and name in rust:
        print('  ok %s （C %s ⇔ Rust 表）' % (name, c_labels[name]))
    elif name in c_labels:
        print('  FAIL %s 由 C 实现（%s）但 Rust 例外表没登记 ⇒ 会被判成"成员缺失"抛异常'
              % (name, c_labels[name]))
    else:
        print('  FAIL %s 在 Rust 例外表里但没有对应的 __asm__ 标签 ⇒ 仍会走到 extern declare 并拖没链接'
              % name)
PY
)

printf '%s\n' "$report" | grep -vE '^__[CR]__ ' || true
c_n=$(printf '%s\n' "$report" | sed -n 's/^__C__ \([0-9]*\)$/\1/p'); c_n=${c_n:-0}
r_n=$(printf '%s\n' "$report" | sed -n 's/^__R__ \([0-9]*\)$/\1/p'); r_n=${r_n:-0}
fails=$(printf '%s\n' "$report" | grep -c '^  FAIL ' || true); fails=${fails:-0}

if [[ $fails -ne 0 ]]; then
  echo "dyn_binding: C 侧 ${c_n} 条标签 / Rust 表 ${r_n} 条，不一致 ${fails}（期望 0）" >&2
  exit 1
fi
echo "dyn_binding: C 侧 ${c_n} 条标签 / Rust 表 ${r_n} 条，两侧一致（期望相等）"
