#!/usr/bin/env python3
"""语料基线测量：L1 解析通过率追踪。

用法: python3 tools/corpus_baseline.py [语料目录...]
默认语料: REasyQuant strategies/

指标: parse_ok / total
- parse 通过 = 编译器走完 parse 阶段（后续链接失败也算 parse 通过）
- parse 失败 = "Parse failed"/"Parse error"/解析期 panic
"""
import subprocess, glob, os, sys

ZETAC = "target/release/zetac"
WORKDIR = "/tmp/corpus_baseline"


def parse_ok(path):
    r = subprocess.run([ZETAC, path, "-o", os.path.join(WORKDIR, "probe")],
                       capture_output=True, text=True, timeout=30)
    out = (r.stderr or "") + (r.stdout or "")
    if "Linking failed" in out or "Compiled to" in out:
        return True   # parse 阶段通过（链接失败是下一阶段的事）
    if "Parse failed" in out or "Parse error" in out:
        return False
    return "panicked" not in out


def main():
    dirs = sys.argv[1:] or [
        os.path.expanduser("~/source/quant/REasyQuant/strategies"),
    ]
    os.makedirs(WORKDIR, exist_ok=True)
    files = []
    for d in dirs:
        files += glob.glob(os.path.join(d, "**", "*.py"), recursive=True)
    files = [f for f in files if ".venv" not in f]
    ok = 0
    fails = []
    for f in sorted(files):
        if parse_ok(f):
            ok += 1
        else:
            fails.append(os.path.basename(f))
    print(f"语料: {len(files)} 文件")
    print(f"解析通过: {ok}/{len(files)} = {ok * 100 // max(len(files), 1)}%")
    if fails:
        print("解析失败:")
        for n in fails:
            print(f"  {n}")


if __name__ == "__main__":
    main()
