#!/usr/bin/env python3
"""语料基线测量：L1 解析通过率追踪。

用法: python3 tools/corpus_baseline.py [语料目录...]
默认语料: REasyQuant strategies/

指标: parse_verdict 判定数 / total（"timeout" 单列）
- parse 通过 = 编译器走完 parse 阶段（后续链接失败也算 parse 通过）
- parse 失败 = "Parse failed"/"Parse error"/解析期 panic
"""
import subprocess, glob, os, sys

ZETAC = "target/release/zetac"
WORKDIR = "/tmp/corpus_baseline"
# 批次 755（backlog #260 余量）：30 秒是冷缓存下的贴地飞行——_drv_accept_409.py
# 实测 20.8s（热），负载下越过 30s ⇒ TimeoutExpired 一炸全批中止、连"解析通过"
# 行都不打（760 门禁当日两次 corpus total=0 的真因）。放宽到 90 秒，并把单文件
# 超时降级为该文件计败、继续跑完——测量器不该比被测物先脆。
TIMEOUT = 90


TIMEOUT_S = 90  # 单文件预算；主树口径＝90 秒（30 秒实测被击穿过，见批次 10008 记录）


def parse_verdict(path):
    """True/False＝解析判定；字符串 "timeout"＝预算内没跑完。

    单文件超时绝不能把整轮读数带走：改之前 `subprocess.run(timeout=30)` 的
    `TimeoutExpired` 没人接，异常直接抛出 ⇒ 连"解析通过 n/40"那一行都不印，
    调用方只能看到"缺行＝读数作废"，看不出是哪一文件、也看不出其余 39 条其实正常。
    """
    try:
        r = subprocess.run([ZETAC, path, "-o", os.path.join(WORKDIR, "probe")],
                           capture_output=True, text=True, timeout=TIMEOUT_S)
    except subprocess.TimeoutExpired:
        return "timeout"    out = (r.stderr or "") + (r.stdout or "")
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
    timeouts = []
    for f in sorted(files):
        v = parse_verdict(f)
        if v is True:
            ok += 1
        elif v == "timeout":
            timeouts.append(os.path.basename(f))
        else:
            fails.append(os.path.basename(f))
    print(f"语料: {len(files)} 文件")
    print(f"解析通过: {ok}/{len(files)} = {ok * 100 // max(len(files), 1)}%")
    if timeouts:
        print(f"超时（单文件预算 {TIMEOUT_S}s，单列、不计解析失败）: {' '.join(timeouts)}")
    if fails:
        print("解析失败:")
        for n in fails:
            print(f"  {n}")


if __name__ == "__main__":
    main()
