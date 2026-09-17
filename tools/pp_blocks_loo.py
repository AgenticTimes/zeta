#!/usr/bin/env python3
"""在**预处理后文本**（ZETA_DUMP_PP 的产物）上做花括号感知的 leave-one-out。

为什么要有这个工具：当 `tools/blocks_loo.py` 在源码层面「单块无解」时，先用
`ZETA_DUMP_PP=<out> zetac <file>` 导出 PP，再把它**直接喂给编译器**：

    cp pp.txt pp_test.py && zetac pp_test.py -o /tmp/x

- 若**也失败** ⇒ 问题在 **parser 层**（与缩进预处理无关），本脚本可用；
- 若**通过** ⇒ 问题在预处理（`//` 括号污染、反斜杠折叠、单行体折叠等）。

本脚本按**花括号平衡**切块（不是按缩进），逐块整段删除；命中即打印该块。

用法: python3 tools/pp_blocks_loo.py <pp.txt>
"""
import subprocess, sys, os

ZETAC = os.environ.get("ZETAC", "./target/release/zetac")
PP = sys.argv[1]
lines = open(PP, encoding="utf-8").read().split("\n")


def delta(s):
    d = 0
    for c in s:
        if c in "([{":
            d += 1
        elif c in ")]}":
            d -= 1
    return d


# body = 第一个形如 "... {" 的块头之后，到最后一个单独的 "}"
close = max(i for i, l in enumerate(lines) if l.strip() == "}")
body_start = 1
body = lines[body_start:close]

blocks, i = [], 0
while i < len(body):
    d, j = delta(body[i]), i
    while d > 0 and j + 1 < len(body):
        j += 1
        d += delta(body[j])
    blocks.append((i, j + 1))
    i = j + 1


def ok(ls):
    open("/tmp/pp_blocks_probe.py", "w").write("\n".join(ls) + "\n")
    r = subprocess.run([ZETAC, "/tmp/pp_blocks_probe.py", "-o", "/tmp/pp_blocks_probe"],
                       capture_output=True, text=True)
    return "W1002" not in r.stderr


print(f"baseline ok? {ok(lines)}  blocks={len(blocks)}")
hit = None
for a, b in blocks:
    if ok(lines[:body_start + a] + lines[body_start + b:]):
        hit = (a, b)
        break
if hit:
    a, b = hit
    print(f"BLOCK fixes it: PP lines {body_start + a + 1}..{body_start + b}")
    for k in range(a, b):
        print("   ", lines[body_start + k])
else:
    print("没有任何单块能修好 —— 需要 2 块以上的交互（下一步做受控两两组合或字符级 ddmin）")
