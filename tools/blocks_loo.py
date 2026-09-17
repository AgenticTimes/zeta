#!/usr/bin/env python3
"""块级 leave-one-out 定位「某个 def 为什么解析失败」。

用法:
    python3 tools/blocks_loo.py <file.py> <def 起始行号(1-based)> [--pairs]

做法（对应 roadmap §一 的手法 2/4）：
  1) 把该 def 整段抽出来拼成独立文件（+ 尾部一个 helper def，确保「后面还有内容」）；
  2) 按缩进切块（同一缩进的一段算一块），逐块整段替换为 `pass`；
  3) 若某一块去掉后能编译 ⇒ 该块内含拦路，**递归进这一块**再切；
  4) `--pairs` 额外做两两组合（本轮 `_run_nautilus` 单块/两两都无解，
     说明是 3 块以上的交互或全局性质，需要再换方法）。

注意（都是踩过的坑）：
  - 块必须**含完整子块**，只留块头 = 空块假象；
  - 不要把多行续行（行尾 `\\`）拆开；
  - 换回旧二进制测试后要 `touch` 源文件再 `cargo build`，否则 cargo 认为无需重建。
"""
import subprocess, re, sys, os

ZETAC = os.environ.get("ZETAC", "./target/release/zetac")
SRC, START = sys.argv[1], int(sys.argv[2])
PAIRS = "--pairs" in sys.argv
L = open(SRC, encoding="utf-8").read().split("\n")
i0 = START - 1
i1 = len(L)
for j in range(i0 + 1, len(L)):
    if re.match(r"^(def |class |@)", L[j]):
        i1 = j
        break
SIG, G = L[i0], "\n\ndef helper() -> int:\n    return 1\n"
PROBE = "/tmp/blocks_loo_probe.py"


def ind(s):
    t = s.lstrip()
    return len(s) - len(t)


def ok(lines):
    open(PROBE, "w").write(SIG + "\n" + "\n".join(lines) + G + "\n")
    r = subprocess.run([ZETAC, PROBE, "-o", PROBE[:-3]], capture_output=True, text=True)
    return "W1002" not in r.stderr


def ranges(lines, base):
    out, i = [], 0
    while i < len(lines):
        if not lines[i].strip() or ind(lines[i]) != base:
            i += 1
            continue
        j = i + 1
        while j < len(lines) and (not lines[j].strip() or ind(lines[j]) > base):
            j += 1
        out.append((i, j))
        i = j
    return out


body = L[i0 + 1:i1]
if ok(body):
    print("无法隔离复现（可能依赖前面的上下文）")
    sys.exit(0)
base = next(ind(l) for l in body if l.strip() and not l.strip().startswith("#"))
print(f"{os.path.basename(SRC)}::{SIG.strip()[:50]}  body={len(body)}  base_indent={base}")
while True:
    R = ranges(body, base)
    hit = None
    for a, b in R:
        trial = body[:a] + [" " * base + "pass"] + body[b:]
        if ok(trial):
            hit = (a, b)
            break
    if hit is None:
        print(f"  缩进 {base} 上没有任何**单块**能修好（块数={len(R)}）")
        if PAIRS:
            found = None
            for x in range(len(R)):
                for y in range(x + 1, len(R)):
                    (s1, e1), (s2, e2) = R[x], R[y]
                    trial = list(body)
                    trial[s1] = " " * base + "pass"
                    for k in range(s1 + 1, e1):
                        trial[k] = None
                    trial[s2] = " " * base + "pass"
                    for k in range(s2 + 1, e2):
                        trial[k] = None
                    if ok([l for l in trial if l is not None]):
                        found = (x, y)
                        break
                if found:
                    break
            print(f"  两两组合: {found}")
        break
    a, b = hit
    print(f"  BLOCK body[{a}:{b}] -> 递归")
    for k in range(a, min(b, a + 3)):
        print(f"      [{k}] {body[k]!r}")
    inner = body[a + 1:b]
    if not inner:
        print("      （单行，就是它）")
        break
    body = inner
    base = next((ind(l) for l in body if l.strip() and not l.strip().startswith("#")), None)
    if base is None:
        break
