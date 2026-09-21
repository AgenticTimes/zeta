#!/usr/bin/env python3
"""性能基线测量：zetac 编译耗时的前后对照（refactor ⑪ / 轴 C 的判据工具）。

为什么是仓内脚本而不是 CI bench：
  `.github/workflows/benchmarks.yml` 里那个 "Benchmarks" 作业是**幽灵**——它
  :106/:148 要 `cargo build --bench compiler_bench|runtime_bench`，而 `benches/`
  目录根本不存在；:190/:199/:209 要 `--bin regression_test`，`src/bin/regression_test.rs`
  也不存在；`Cargo.toml:94` 付了 criterion 依赖却没有 `[[bench]]` 段。
  补那条路要改正被并发工作流持有的 `Cargo.toml`，故 ⑪ 先用本脚本量真实负载。

测什么（两个阶段，口径不同，别混着看）：
  aot = `zetac <f> -o probe`   走完 前端→MIR→LLVM -O3→.o→gcc 链接（端到端用户视角）
  ir  = `zetac <f> --emit-llvm`（module 打到 **stderr**，见 main.rs:816 print_to_stderr）
                              只到**未优化** IR 打印（批次 307：--emit-llvm 打的是
                              优化前的 module）⇒ 轴 B/C 改 IR 体量时看这条最灵敏。
                              批次 313 实测：打印完成后进程在退出路径 SIGSEGV(rc=139)。

统计口径（三条都是批次 313 在本机量出来的，不是设计时假设）：
  1. 每个 (阶段, 文件) 取 **min**（`PERF_REPEAT`，默认 3）。min 而不是 mean：
     干扰只会让它变慢，取小值最接近真机时。
  2. **先暖机**（丢弃一轮"每文件每阶段各一次"）：批次 313 未暖机时，冷态首采的基线
     与随后的稳态测量差 +38%（aot total 27,689 → 36,320 ms，同一份二进制），
     会把机器状态读成回退。暖机后 ir 的散布降到 3.5%，但 aot 仍漂到 20%（见 3）。
     `PERF_WARMUP=0` 仅供调试。
  3. **判定 = 主阶段(`PERF_GATE_STAGES`，默认 `ir`) 的 total_ms 变化**，阈值
     `PERF_REGRESS_PCT`（默认 10%）。同一份二进制暖机后连做 4 次独立调用实测：
       ir total   41,655 / 41,016 / 42,288 / 42,460 / 41,132 ms ⇒ 散布 **3.5%** ⇒ 可判 ✅
       ir 逐文件中位  +0.6% / +7.9% / +8.1%                 ⇒ 12 个样本的中位数被个别
          长尾文件单次抖动(实测最大 +24%)推动，比 total 更抖 ⇒ 只作参考 ❌
       aot total  30,186 / 36,421 / 32,451 / 33,206 / 33,067 ms ⇒ 含 clang 链接，跨调用漂
          **20.7%** ⇒ 默认不参与判定（任务 #27：A/B 交错才能消掉机器负载） ❌
     ⇒ 阈值 10% 高于 ir 的 3.5% 地板，但对 aot 无效。
  提前报错退出的轮次会从 min 里**剔除**并计入 bad_runs（[W3001]）；link-fail 与
  ir 的"IR 打印完成后才崩"属于编译走完、退出路径有缺陷，计入 noted_runs（[N3001]）。
  噪声本身可用 `--calibrate` 复测（同码连测 `PERF_CALIB_TIMES` 轮看散布）。

用法：
  python3 tools/perf_baseline.py                     # 打印当前测量
  python3 tools/perf_baseline.py --snapshot          # 另存基线（默认 /tmp/zeta_perf_baseline.json）
  python3 tools/perf_baseline.py --diff              # 与基线比对，回退超阈值则 rc=1
  python3 tools/perf_baseline.py --calibrate         # 同码连测 N 轮，量噪声地板
  PERF_CORPUS=<dir> PERF_FILES=16 PERF_STAGES=ir PERF_GATE_STAGES=ir,\\
  PERF_REPEAT=5 PERF_WARMUP=0 python3 tools/perf_baseline.py

基线默认落在 /tmp 而不是仓内：仓根 `.gitignore` 的 `run_*` 等规则会把这类文件吞掉
（同一根因见任务 #15：tools/run_all.sh 也未被跟踪）。要长期留存需先修那条忽略规则。
"""
import glob
import json
import os
import statistics
import subprocess
import sys
import time

ZETAC = "target/release/zetac"
WORKDIR = "/tmp/zeta_perf_probe"
BASELINE = os.environ.get("ZETA_PERF_BASELINE", "/tmp/zeta_perf_baseline.json")
CORPUS = os.environ.get("PERF_CORPUS", os.path.expanduser("~/source/quant/REasyQuant/strategies"))
N_FILES = int(os.environ.get("PERF_FILES", "12"))
STAGES = (os.environ.get("PERF_STAGES", "aot,ir")).split(",")
# 判定只压在主阶段上；批次 313 用实测噪声地板定的这个默认值见 --calibrate 说明。
GATE_STAGES = os.environ.get("PERF_GATE_STAGES", "ir").split(",")
TIMEOUT = 60


def die(msg, rc=2):
    print(f"[E3001] {msg}", file=sys.stderr)
    sys.exit(rc)


def collect_files():
    if not os.path.isdir(CORPUS):
        die(f"语料目录不存在：{CORPUS}（用 PERF_CORPUS 指定；**空清单不等于通过**）")
    fs = glob.glob(os.path.join(CORPUS, "**", "*.py"), recursive=True)
    # .venv 是第三方包；_zeta_local* 是我方临时 driver，不属于语料且会被清理
    fs = [f for f in fs if ".venv" not in f and "_zeta_local" not in os.path.basename(f)]
    fs = sorted(fs)[:N_FILES]
    if not fs:
        die(f"{CORPUS} 下找不到 .py 语料，测不出任何数据")
    return fs


def _ir_printed(r):
    """LLVM AssemblyWriter 把 attribute 组写在 module 最末尾，因此
    '以 attributes #N = {...} 结尾' 就是 --emit-llvm 那条打印已经跑完的证据。"""
    tail = ((r.stdout or "") + (r.stderr or "")).rstrip()
    return tail.endswith("}") and "\nattributes #" in tail[-4000:]


def run_once(stage, src, out):
    """返回 (耗时 ms, 状态)。状态是分阶段的，混用会得出错误结论：
      aot 走真链接，本机环境链接失败是常态；'Linking failed' 说明**编译阶段本身走完**
          ⇒ 记 link-fail（可信），不是提前退出。
      ir  的 --emit-llvm 在批次 313 实测：**每次**都在打印完 IR 之后于进程退出路径
          SIGSEGV（rc=139，lldb: frame#0 = 0x0 空指针调用，发生在 main 返回之后）。
          IR 已完整 ⇒ 计时有效，单独记 torn:*-crash-after-dump（另立缺陷任务）。
    """
    if stage == "aot":
        cmd = [ZETAC, src, "-o", out]
    elif stage == "ir":
        cmd = [ZETAC, src, "--emit-llvm"]
    else:
        die(f"未知阶段 {stage}")
    t0 = time.perf_counter()
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return (TIMEOUT * 1000.0, "timeout")
    ms = (time.perf_counter() - t0) * 1000.0
    txt = (r.stdout or "") + (r.stderr or "")
    rc = r.returncode
    if stage == "ir":
        complete = _ir_printed(r)
        if rc == 0:
            return (ms, "ok" if complete else "fail:ir-truncated")
        if complete:
            sig = -rc if rc < 0 else rc
            return (ms, f"torn:dump后退出崩溃(sig{sig})")
        return (ms, f"fail:rc{rc}")
    if rc < 0:
        return (ms, f"crash:sig{-rc}")
    if "Compiled to" in txt:
        return (ms, "ok")
    if "Linking failed" in txt:
        return (ms, "link-fail")
    return (ms, f"fail:rc{rc}")


TRUSTWORTHY = ("ok", "link-fail")


def trustworthy(st):
    return st in TRUSTWORTHY or st.startswith("torn:")


def warm_up(files):
    """丢弃一轮"每文件每阶段各一次"的测量，让机器进入持续负载态再取数。
    批次 313 实测原因：冷态首采的基线比随后的稳态**快 30%**（快照 27,689 ms vs
    紧接着三轮稳态 35,846/36,165/36,320 ms，轮间散布仅 0.9%）——
    不暖机就把基线取在冷态，`--diff` 会稳定地报出 +38% 的假回退。
    ⚠️ 暖机只解决"冷/热"这一层：aot 暖机后跨调用仍漂 20%（含 clang 链接），
    所以判定默认只压 ir 阶段（见 统计口径 3 与任务 #27）。"""
    for stage in STAGES:
        for f in files:
            run_once(stage, f, os.path.join(WORKDIR, "warm"))


def measure(files, repeat):
    res = {}
    bad = {}
    noted = {}
    for stage in STAGES:
        per = {}
        for f in files:
            runs = []
            statuses = []
            for _ in range(repeat):
                ms, st = run_once(stage, f, os.path.join(WORKDIR, "probe"))
                runs.append(ms)
                statuses.append(st)
            for st in statuses:
                d = noted if trustworthy(st) else bad
                k = st if st in TRUSTWORTHY else f"{stage}:{st}"
                d[k] = d.get(k, 0) + 1
            good = [ms for ms, st in zip(runs, statuses) if trustworthy(st)]
            per[os.path.relpath(f, CORPUS)] = round(min(good or runs), 1)
        res[stage] = {"per_file": per, "total_ms": round(sum(per.values()), 1),
                      "median_of_min_ms": round(statistics.median(per.values()), 1)}
    return res, bad, noted


def _deltas(base_stage, cur_stage, key="per_file"):
    """[(文件, 相对变化分数)]，只比双方都有的文件；0.1 = +10%。"""
    b = base_stage[key]
    return [(k, (v - b[k]) / b[k]) for k, v in cur_stage[key].items() if k in b and b[k] > 0]


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    if mode and mode not in ("--snapshot", "--diff", "--calibrate"):
        die(f"未知模式 {mode}（可用：--snapshot / --diff / --calibrate）")
    if mode == "--diff" and not os.path.exists(BASELINE):
        die(f"基线 {BASELINE} 不存在——先 --snapshot（缺基线时比对无意义，不能当作通过）")
    repeat = int(os.environ.get("PERF_REPEAT", "3"))
    os.makedirs(WORKDIR, exist_ok=True)
    if not os.path.exists(ZETAC):
        die(f"{ZETAC} 不存在——先 cargo build --release（测的不是缓存里的旧二进制）")
    files = collect_files()
    base = None
    if mode == "--diff":
        # 口径不一致的比对没有意义，而且"文件数变少 ⇒ total 变小"会被读成"没有回退"
        # ⇒ 必须在暖机（花掉真金白银的编译时间）**之前**判掉
        base = json.load(open(BASELINE))
        for k, want in (("corpus", CORPUS), ("n_files", len(files)), ("repeat", repeat)):
            if base.get(k) != want:
                die(f"基线的 {k}={base.get(k)!r} 与本次 {want!r} 不一致"
                    f"（口径不同的 total 不可比，不能当作通过）")
        for s in STAGES:
            if s not in base.get("stages", {}):
                die(f"基线里没有阶段 {s}（本次 PERF_STAGES={STAGES}）——换口径请重取基线")
    if os.environ.get("PERF_WARMUP", "1") == "1":
        warm_up(files)
        print(f"[warm] 已丢弃一轮暖机数据（{len(files)} 文件 × {len(STAGES)} 阶段）",
              file=sys.stderr)

    if mode == "--diff":
        cur, bad, noted = measure(files, repeat)
        worst = 0.0
        for stage in STAGES:
            b, c = base["stages"][stage], cur[stage]
            td = (c["total_ms"] - b["total_ms"]) / b["total_ms"] * 100.0
            ds = _deltas(b, c)
            med = statistics.median([d for _, d in ds]) * 100.0 if ds else 0.0
            # 判据 = **主阶段的 total_ms 变化**。批次 313 在本机实测三种口径的复现性：
            #   ir total（5 次暖机后的独立调用）41,655 / 41,016 / 42,288 / 42,460 /
            #        41,132 ms ⇒ 散布 3.5% ⇒ 可判 ✅
            #   ir 逐文件中位  +0.6% / +7.9% / +8.1%   ⇒ 12 个样本的中位数被个别长尾
            #        文件单次抖动（实测最大 +24%）推动，比 total 抖 ⇒ 只作参考 ❌
            #   aot total      30,186 / 36,421 / 32,451 / 33,206 / 33,067 ms ⇒ clang 链接，
            #        跨调用漂移 20.7%，只能作参考、不能作门禁 ❌（任务 #27）
            # ⇒ 默认只判 ir（GATE_STAGES），阈值 10% 高于 3.5% 的地板。
            gated = stage in GATE_STAGES
            if gated:
                worst = max(worst, td)
            print(f"{stage:4s} {'判据' if gated else '仅参考'}=total 变化 {td:+5.1f}%"
                  f"   （total {b['total_ms']:.0f} → {c['total_ms']:.0f} ms；"
                  f"逐文件中位 {med:+.1f}%）")
            for k, dlt in sorted(((k, d * 100.0) for k, d in ds), key=lambda kv: -abs(kv[1]))[:3]:
                print(f"        位移最大 {k}  {dlt:+.1f}%")
        report(bad, noted)
        thr = float(os.environ.get("PERF_REGRESS_PCT", "10"))
        print(f"      （判定阶段 {GATE_STAGES} 的 total；aot 因含链接跨调用漂 20%，"
              f"用 PERF_GATE_STAGES=aot,ir 可强行纳入）")
        if worst > thr:
            print(f"[E3002] {GATE_STAGES} 的 total 相对基线回退 {worst:+.1f}% > {thr}%"
                  f"（若怀疑是机器负载，先跑 --calibrate 看噪声地板）")
            return 1
        print(f"未超过回退阈值 {thr}%（判定阶段 total 最大变化 {worst:+.1f}%）")
        return 0

    if mode == "--calibrate":
        # 同一份二进制连测 N 轮 ⇒ 轮间散布就是**噪声地板**：任何低于它的
        # "回退"都不能算回退。批次 313 实测：未暖机时第 1→2 轮就飘 28%(aot total)，
        # 那是冷/热态差而非随机噪声 ⇒ 必须先暖机，再看地板是否降到 ~1%。
        times = int(os.environ.get("PERF_CALIB_TIMES", "2"))
        passes = []
        for i in range(times):
            cur, bad, noted = measure(files, repeat)
            passes.append(cur)
            print(f"第 {i+1}/{times} 轮：", {s: passes[-1][s]["total_ms"] for s in STAGES},
                  file=sys.stderr)
        report(bad, noted)
        print(f"\n噪声地板（同一份二进制连测 {times} 轮；判据口径 = 主阶段 total）：")
        floor = 0.0
        for s in STAGES:
            t = [p[s]["total_ms"] for p in passes]
            f_total = (max(t) - min(t)) / min(t) * 100.0
            meds = [statistics.median([d for _, d in _deltas(passes[0][s], p[s])]) * 100.0
                    for p in passes[1:]]
            f_med = max((abs(m) for m in meds), default=0.0)
            if s in GATE_STAGES:
                floor = max(floor, f_total)
            print(f"  {s:4s} total 级 {f_total:5.1f}%{'（判定口径）' if s in GATE_STAGES else ''}"
                  f"   逐文件中位级 {f_med:5.1f}%   total 序列 {[round(x) for x in t]}")
        print(f"⇒ 判定口径下本机地板 {floor:.1f}%；PERF_REGRESS_PCT 必须显著高于它"
              f"（建议 {max(2 * floor, 10.0):.0f}%），否则门禁会把机器负载读成回退。")
        return 0

    cur, bad, noted = measure(files, repeat)
    doc = {"corpus": CORPUS, "n_files": len(files), "repeat": repeat,
           "stages": cur, "bad_runs": bad, "noted_runs": noted}
    print(json.dumps(doc, indent=2, ensure_ascii=False))
    report(bad, noted)
    if mode == "--snapshot":
        json.dump(doc, open(BASELINE, "w"), ensure_ascii=False)
        print(f"基线已写入 {BASELINE}", file=sys.stderr)
    return 0


def report(bad, noted):
    if bad:
        print("[W3001] 不可信的测量轮次（提前退出会『变快』，这些轮次已被排除在 min 之外）：",
              file=sys.stderr)
        for k, n in sorted(bad.items()):
            print(f"    {k} × {n}", file=sys.stderr)
    if noted:
        print("[N3001] 已计入 min 但需要知道的退出路径异常：", file=sys.stderr)
        for k, n in sorted(noted.items()):
            print(f"    {k} × {n}", file=sys.stderr)


if __name__ == "__main__":
    sys.exit(main())
