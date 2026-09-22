#!/usr/bin/env python3
"""tools/parse_bisect.py —— 把 "解析器在这里停了" 变成 "就是这一行的这个构造"。

为什么需要它：`[W1002]` 只报到**顶层条目**的首行（批次 321 修完绑定模式后，official
基线仍有 11 个被截断的文件：8 个报 `fn` 头、2 个报 `impl` 头、1 个报 `import` 行），
而真正的病因在条目**内部某一条语句**。按 W1002 的措辞去猜构造会猜错——批次 321 实测：
`impl P {}`、`impl P for Q {}`、`1 | 2 | 3` 或模式、`unsafe {}`、`if let Some((p, q))`、
`} else if` 嵌套链 **单独喂都能解析**，所以 `minimal_compiler`（丢 757 行）/
`selfhost`（丢 158 行）的病因不在 impl 头上（前者由 `func: match op {…}` 即
"match 当结构体字段值"触发，见 docs/ABI.md 附 B#10 的分类表）。

判据：从截断行开始，一条语句一条语句地喂前缀（补右花括号使其配平），
**第一条**让 W1002 重新出现的行就是病因行——顺序扫描保证先命中的是真凶，
括号计数把字符串里的花括号算错最多导致某行被跳过（不测），不会误报更早的行。

用法：
    ./tools/parse_bisect.py tests/unit-tests/selfhost.z
    ./tools/parse_bisect.py --all            # 扫 official 目录，逐文件给病因行
退出码：0=找到病因行  1=无截断（无需诊断）  2=有截断但扫不出病因（要人工看）
"""
import argparse
import os
import re
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
W1002 = re.compile(
    r"\[W1002\]\s+\S+?\.z:(?P<line>\d+):\s+(?P<n>\d+) line\(s\) at the end"
)


def strip_noise(line: str) -> str:
    """去掉行内注释与字符串/字符字面量内容，只留下真正参与配平的括号。"""
    out = []
    i = 0
    while i < len(line):
        c = line[i]
        if c == "/" and line[i + 1 : i + 2] == "/":
            break
        if c in "\"'":
            q = c
            i += 1
            while i < len(line) and line[i] != q:
                i += 2 if line[i] == "\\" else 1
            i += 1
            continue
        out.append(c)
        i += 1
    return "".join(out)


def compile_text(zetac: str, text: str) -> str:
    """编译一段源码，返回 stderr（W1002 是 warning，不影响退出码）。"""
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "p.z")
        with open(src, "w") as f:
            f.write(text)
        try:
            p = subprocess.run(
                [zetac, src, "-o", os.path.join(d, "p.out")],
                capture_output=True,
                text=True,
                timeout=60,
            )
        except subprocess.TimeoutExpired:
            return "TIMEOUT"
        return p.stderr


def find_truncation(zetac: str, path: str):
    with open(path, errors="replace") as f:
        lines = f.read().splitlines()
    err = compile_text(zetac, "\n".join(lines) + "\n")
    m = W1002.search(err)
    if not m:
        return None, lines, None
    return (int(m.group("line")), int(m.group("n"))), lines, err


def bisect(zetac: str, lines: list, start: int, end: int):
    """在顶层条目 lines[start-1 : end]（0-based 闭开区间）内找第一条病因行。

    只在**合法切点**试前缀，否则会把"前缀本身不完整"误读成病因（批次 321 踩过：
    在 `} else if` 之前切一刀，补上的 `}` 造出一条悬空 else 链，报出假病因行）。
    合法切点 = ①任意深度上以 `;` 收尾的行（一条完整语句，补 `}`*d 即配平），
    或 ②深度回到 1 且以 `}` 收尾、且**下一行不以 `else` 开头**。
    """
    item = lines[start - 1 : end]
    d = 0
    for i, raw in enumerate(item):
        stripped = strip_noise(raw)
        d += stripped.count("{") - stripped.count("}")
        if d < 0:
            return None  # 条目在预期之前闭合
        body = stripped.strip()
        if d < 1 or not body:
            continue
        cut = False
        if body.endswith(";"):
            # 任何深度上以 `;` 收尾的都是一条完整语句，补 `}`*d 能配平外层块。
            cut = True
        elif body.endswith("}") and d == 1:
            nxt = ""
            for j in range(i + 1, len(item)):
                t = item[j].strip()
                if t and not t.startswith("//"):
                    nxt = t
                    break
            cut = not nxt.startswith("else")
        if not cut:
            continue
        prefix = "\n".join(item[: i + 1]) + "}" * d + "\n"
        if "W1002" in compile_text(zetac, prefix):
            return start + i, raw.strip(), i
    return None


def item_end(lines: list, start: int) -> int:
    """返回该顶层条目的结束行（0-based 开区间上界）。"""
    d = 0
    seen = False
    for i in range(start - 1, len(lines)):
        s = strip_noise(lines[i])
        d += s.count("{") - s.count("}")
        seen = seen or "{" in s
        if d <= 0 and (seen or s.strip().endswith(";")):
            return i + 1
    return len(lines)


def report_all(zetac: str, directory: str) -> int:
    rc = 0
    for name in sorted(os.listdir(directory)):
        if not name.endswith(".z"):
            continue
        path = os.path.join(directory, name)
        trunc, lines, _ = find_truncation(zetac, path)
        if not trunc:
            continue
        start, n = trunc
        end = item_end(lines, start)
        hit = bisect(zetac, lines, start, end)
        rc = 1
        if hit:
            ln, txt, _ = hit
            print(f"{name}:{start} 丢{n}行 病因行 {ln}: {txt[:70]}")
        else:
            print(f"{name}:{start} 丢{n}行 未定位（条目首行本身即不可解析？看 {lines[start-1].strip()[:60]}）")
            rc = 2
    return rc


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("file", nargs="?")
    ap.add_argument("--zetac", default=os.path.join(ROOT, "target/release/zetac"))
    ap.add_argument("--all", action="store_true", help="扫 tests/unit-tests/ 全部文件")
    ap.add_argument("--dir", default=os.path.join(ROOT, "tests/unit-tests"))
    a = ap.parse_args()

    if a.all:
        return report_all(a.zetac, a.dir)
    if not a.file:
        ap.error("要么给 file，要么给 --all")

    trunc, lines, _ = find_truncation(a.zetac, a.file)
    if not trunc:
        print(f"{a.file}: 无截断（整个文件都进了 AST）")
        return 1
    start, n = trunc
    end = item_end(lines, start)
    hit = bisect(a.zetac, lines, start, end)
    print(f"{a.file}:{start} 之后 {n} 行未解析（条目 = 第 {start}..{end} 行）")
    if hit:
        print(f"病因行 {hit[0]}: {hit[1]}")
        return 0
    print("未定位到病因行：条目首行本身就不可解析，或括号计数被字符串骗过")
    return 2


if __name__ == "__main__":
    sys.exit(main())
