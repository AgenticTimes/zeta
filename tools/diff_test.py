#!/usr/bin/env python3
"""tools/diff_test.py —— CPython 差分测试 harness（refactor.md G.3 起步 / 立即档 ⑩）

为什么需要它：`// expect:` 断言的是**我以为是什么**——写用例的人照着自己对
zeta 的想象填期望值，于是它只能防回归，永远量不出"语义与 Python 不一致"。
CPython 才是这门语言的规范本体（pyramid 1.2 的豁免理由就是"由 G.3 的行为性代偿"），
所以期望值必须由参考实现现场产出：同一段语义写两份（python 形 / zeta 形），
`python3` 跑前者 stdout 即真值，`zetac` 跑后者与之逐行比对。

这第一次让"还有多少语义是错的"变成一个数字（一致率），而不是一种感觉。

用例格式（tests/diff/cases/*.dcase，一文件两份，相邻不许拆开放——两份会漂移）：

    # @cat: truth            # 五类之一：truth|str|container|numeric|control
    # @note: 空列表真值       # 一句话说明这条在测什么语义点
    #@@ python
    print(1 if [] else 0)
    #@@ zeta
    print(1 if [] else 0)

  两份**必须同语义**；python 形是唯一权威期望，手写期望值一律不算数。
  扩展名用 `.dcase` 而非 `.z`：`.gitignore:114` 的裸 `*.z` 会把新建用例吞掉
  （任务 #40 记的同一把坑），差分用例尤其不能依赖 `git add -f`。

判定分层（坏用例不配进分母）：
  match      两份 stdout 逐行相等
  mismatch   zeta 跑通了但值不同  —— 真语义缺口，本 harness 的存在理由
  compile    zeta 编译/降级失败  —— 能力缺口
  runtime    zeta 运行期非 0/超时 —— 崩点
  bad_case   python 参考侧自身报错 —— 用例写坏了，记 rc=2 并**排除出分母**

判据（对齐 tools/jit_sweep.sh 的"计数不回退"口径，不用比率当闸门）：
  硬闸门 1：基线里记为 match 的用例不许变差（逐用例比对，位置无关）。
  硬闸门 2：match 绝对数 >= 基线 `match_min`（新增用例失败不罚，
            把已通过的改坏必罚）。比率只做展示指标。
  ./tools/diff_test.py --bless    # 采集/刷新 tools/baselines/diff_consistency.json
  ./tools/diff_test.py            # 核对；回归 rc=1，坏用例 rc=2
  退出码 precedence：**回归优先**——曾经 match 的用例退化成 bad_case 同时命中两条，
  此时报 1（run_all.sh 里 rc=2 只喊话不判红，判 1 才会拦住"覆盖度静默消失"）。

已知边界（不是缺陷，别当"已覆盖"）：
  1) 呈现层与语义层混在一根尺子上：`%.6f` 的浮点打印、`None`/容器的 repr 差异
     会记成 mismatch，而它们不是"算术/控制流错了"。所以逐类读数（按 @cat）比
     总比率可读，且**别为了拉高比率去改期望值**——要改的是实现。
  2) 只覆盖 print 到 stdout 的可观察行为；异常类型、迭代器惰性、GC 时机测不到。
  3) 用例是人 curated 的，覆盖面=想到的面。refactor.md G.3 的进阶档（受限语法
     随机程序生成）才是把"想到的面"变成"采样空间"的那一步。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CASE_DIR = ROOT / "tests" / "diff" / "cases"
BASELINE = ROOT / "tools" / "baselines" / "diff_consistency.json"
ZETAC = Path(os.environ.get("ZETAC", ROOT / "target" / "release" / "zetac"))
CATEGORIES = ("truth", "str", "container", "numeric", "control")
RUN_TIMEOUT = 20
# 链接器报缺符号的行形：`  "_host_str_find", referenced from:`（与 run_all.sh 的
# 缺绑定登记同一口径：必须指名，否则"还没绑定"会冒充"编译器不支持"）。
UNDEF_SYM_RE = re.compile(r'"_([A-Za-z0-9_]+)"')


class BadCase(Exception):
    """用例本身写坏了（参考侧跑不出真值）——不许混进语义缺口里。"""


def parse_case(path: Path) -> dict:
    """把一个 .dcase 拆成 {cat, note, python, zeta}。段落标记必须各出现一次。"""
    lines = path.read_text(encoding="utf-8").splitlines()
    cat, note = "", ""
    sections: dict[str, list[str]] = {}
    cur = None
    for ln in lines:
        stripped = ln.strip()
        if stripped.startswith("#@@"):
            cur = stripped[3:].strip()
            if cur in sections:
                raise BadCase(f"段落标记 `#@@ {cur}` 重复")
            sections[cur] = []
            continue
        if cur is None:
            if stripped.startswith("# @cat:"):
                cat = stripped[len("# @cat:"):].strip()
            elif stripped.startswith("# @note:"):
                note = stripped[len("# @note:"):].strip()
            continue
        sections[cur].append(ln)
    missing = {"python", "zeta"} - set(sections)
    if missing:
        raise BadCase(f"缺少段落 {sorted(missing)}")
    extra = set(sections) - {"python", "zeta"}
    if extra:
        raise BadCase(f"未知段落 {sorted(extra)}")
    if cat not in CATEGORIES:
        raise BadCase(f"@cat 必须是 {CATEGORIES}，实为 {cat!r}")
    return {
        "cat": cat,
        "note": note,
        "python": "\n".join(sections["python"]) + "\n",
        "zeta": "\n".join(sections["zeta"]) + "\n",
    }


def norm(out: str) -> list[str]:
    """与 tests/python_style/run.sh 同口径：只归一化尾部空行，其余逐字比。"""
    lines = out.split("\n")
    while lines and lines[-1].strip() == "":
        lines.pop()
    return lines


def run_ref(src: str, name: str, workdir: Path) -> list[str]:
    wd = workdir / name
    wd.mkdir(parents=True, exist_ok=True)
    workdir = wd
    p = workdir / "ref.py"
    p.write_text(src, encoding="utf-8")
    try:
        r = subprocess.run(
            [sys.executable, "-B", str(p)],
            capture_output=True, text=True, timeout=RUN_TIMEOUT, cwd=str(workdir),
        )
    except subprocess.TimeoutExpired:
        raise BadCase("参考侧（python3）超时")
    if r.returncode != 0:
        tail = (r.stderr or "").strip().splitlines()
        raise BadCase(f"参考侧退出 {r.returncode}: {tail[-1] if tail else '无 stderr'}")
    return norm(r.stdout)


def run_zeta(src: str, name: str, workdir: Path) -> tuple[str, list[str], str]:
    """返回 (verdict, stdout行, 详情)。verdict ∈ matchable 的三态之一。"""
    # 并行安全：每个用例一个子目录（zetac 的中间产物落在 cwd，共享目录会互相踩）
    wd = workdir / name
    wd.mkdir(parents=True, exist_ok=True)
    z = wd / f"{name}.z"
    z.write_text(src, encoding="utf-8")
    binp = wd / name
    try:
        c = subprocess.run(
            [str(ZETAC), str(z), "-o", str(binp)],
            capture_output=True, text=True, timeout=60, cwd=str(wd),
        )
    except subprocess.TimeoutExpired:
        return "compile", [], "zetac 超时（>60s）"
    if c.returncode != 0:
        return "compile", [], compile_detail(c.stderr)
    try:
        r = subprocess.run(
            [str(binp)], capture_output=True, text=True, timeout=RUN_TIMEOUT,
            cwd=str(workdir),
        )
    except subprocess.TimeoutExpired:
        return "runtime", [], f"超时（>{RUN_TIMEOUT}s）"
    if r.returncode != 0:
        tail = (r.stderr or "").strip().splitlines()
        return "runtime", [], f"exit {r.returncode}: {tail[-1] if tail else '无 stderr'}"
    return "ok", norm(r.stdout), ""


def load_baseline(path: Path) -> dict | None:
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        print(f"基线 {path} 解析失败: {e}", file=sys.stderr)
        return None


def main() -> int:
    ap = argparse.ArgumentParser(description="CPython 差分测试 harness（G.3）")
    ap.add_argument("--bless", action="store_true", help="采集/刷新基线")
    ap.add_argument("--only", metavar="SUBSTR", help="只跑名字含该子串的用例（调试用，不参与基线判定）")
    ap.add_argument("--verbose", "-v", action="store_true", help="打印每条 mismatch/compile 的实际输出")
    ap.add_argument("--json", metavar="PATH", help="把本次读数另存一份 JSON")
    args = ap.parse_args()

    if not ZETAC.exists():
        print(f"缺 {ZETAC}；先 cargo build --release", file=sys.stderr)
        return 2
    cases = sorted(CASE_DIR.glob("*.dcase"))
    if args.only:
        cases = [c for c in cases if args.only in c.name]
    if not cases:
        print(f"{CASE_DIR} 下没有 .dcase 用例", file=sys.stderr)
        return 2

    workdir = Path(tempfile.mkdtemp(prefix="zeta_diff."))
    results: dict[str, dict] = {}
    per_cat: dict[str, list[int]] = {c: [0, 0] for c in CATEGORIES}  # [match, judged]
    bad = []
    def judge(path: Path) -> tuple[str, dict]:
        name = path.stem
        rec = {"cat": "?", "verdict": "bad_case", "detail": ""}
        try:
            case = parse_case(path)
            rec["cat"] = case["cat"]
            ref = run_ref(case["python"], name, workdir)
            verdict, got, detail = run_zeta(case["zeta"], name, workdir)
            if verdict != "ok":
                rec.update(verdict=verdict, detail=detail)
            elif got == ref:
                rec["verdict"] = "match"
            else:
                diff = first_diff(ref, got)
                rec.update(verdict="mismatch", detail=f"首个差异行 #{diff[0]}: 期望 {diff[1]!r} 实得 {diff[2]!r}")
        except BadCase as e:
            rec.update(verdict="bad_case", detail=str(e))
        except Exception as e:  # harness 自身的洞必须响，不许静默记成缺口
            rec.update(verdict="bad_case", detail=f"harness 异常: {type(e).__name__}: {e}")
        return name, rec

    try:
        from concurrent.futures import ThreadPoolExecutor
        raw = os.environ.get("DIFF_JOBS", "0") or os.cpu_count() or 4
        workers = max(1, min(8, int(raw)))
        with ThreadPoolExecutor(max_workers=workers) as ex:
            for name, rec in ex.map(judge, cases):
                results[name] = rec
                if rec["verdict"] != "bad_case":
                    slot = per_cat.setdefault(rec["cat"], [0, 0])
                    slot[1] += 1
                    if rec["verdict"] == "match":
                        slot[0] += 1
    finally:
        os.system(f"rm -rf {shlex.quote(str(workdir))}")
    bad = sorted(n for n, r in results.items() if r["verdict"] == "bad_case")

    judged = len(cases) - len(bad)
    match = sum(1 for r in results.values() if r["verdict"] == "match")
    rate = (match / judged * 100) if judged else 0.0
    by_verdict: dict[str, int] = {}
    for r in results.values():
        by_verdict[r["verdict"]] = by_verdict.get(r["verdict"], 0) + 1

    for name in sorted(results):
        rec = results[name]
        if rec["verdict"] == "match":
            continue
        print(f"{rec['verdict']:9} {name} [{rec['cat']}] {rec['detail']}")
        if args.verbose and rec["verdict"] in ("mismatch", "compile", "runtime"):
            print(f"    note: {parse_case(CASE_DIR / (name + '.dcase'))['note']}")
    for cat in CATEGORIES:
        m, n = per_cat.get(cat, [0, 0])
        print(f"  {cat:10} {m}/{n}" if n else f"  {cat:10} （无用例）")

    doc = {
        "total": len(cases),
        "judged": judged,
        "match": match,
        "rate_pct": round(rate, 1),
        "match_min": match,
        "by_verdict": by_verdict,
        "per_cat": {c: {"match": v[0], "judged": v[1]} for c, v in per_cat.items()},
        "cases": {k: {"cat": v["cat"], "verdict": v["verdict"]} for k, v in sorted(results.items())},
    }
    if args.json:
        Path(args.json).write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")

    print(
        f"diff test: match={match} judged={judged} rate={rate:.1f}% "
        f"bad_case={len(bad)}（总用例 {len(cases)}）"
    )

    if args.bless:
        BASELINE.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
        print(f"基线已写入 {BASELINE}（match_min={match}，比率 {rate:.1f}%）")
        return 2 if bad else 0

    base = load_baseline(BASELINE)
    rc = 0
    if bad:
        print(f"坏用例 {len(bad)} 条（参考侧跑不出真值，已排除出分母）: {' '.join(bad)}")
        rc = 2
    if args.only:
        print("--only 子集跑：不与基线比对（子集里「消失」的用例不是回归），判定只到本次读数")
        return rc
    if base is None:
        print(f"无基线 {BASELINE}——只出读数，不判定。先 --bless", file=sys.stderr)
        return rc
    regress = []
    for name, rec in sorted(base.get("cases", {}).items()):
        if rec.get("verdict") != "match":
            continue
        now = results.get(name)
        if now is None:
            regress.append(f"{name}（基线 match，本次用例文件消失）")
        elif now["verdict"] != "match":
            regress.append(f"{name}（match → {now['verdict']}: {now['detail']}）")
    if regress:
        print("一致用例回归：")
        for r in regress:
            print(f"  {r}")
        rc = 1
    floor = int(base.get("match_min", 0))
    if match < floor:
        print(f"match={match} 低于基线 match_min={floor}")
        rc = 1
    improved = [
        n for n, r in sorted(results.items())
        if r["verdict"] == "match" and base.get("cases", {}).get(n, {}).get("verdict") not in (None, "match")
    ]
    if improved:
        print(f"较基线转好 {len(improved)} 条（记得 --bless 抬高闸门）: {' '.join(improved)}")
    if rc == 0:
        print("差分一致率无回归")
    return rc


def first_diff(ref: list[str], got: list[str]) -> tuple[int, str, str]:
    for i in range(max(len(ref), len(got))):
        a = ref[i] if i < len(ref) else "（无此行）"
        b = got[i] if i < len(got) else "（缺此行）"
        if a != b:
            return (i + 1, a, b)
    return (0, "", "")


def compile_detail(stderr: str) -> str:
    """链接失败的光看末行是 `Error: "Linking failed"`——指名不了缺什么。
    批次 323 在 run_all.sh 里定过同一条口径：缺运行时绑定必须登记**符号名**，
    否则"编译器不支持"会冒充"还没绑定"（这两件事的修法完全不同）。"""
    lines = (stderr or "").strip().splitlines()
    tail = lines[-1] if lines else "无 stderr"
    miss = sorted({m.group(1) for ln in lines for m in [UNDEF_SYM_RE.search(ln)] if m})
    if miss:
        return f"缺运行时绑定: {', '.join(miss)}"
    w1002 = [ln for ln in lines if "W1002" in ln]
    if w1002:
        return f"{tail}（解析截断，见 W1002）"
    return f"exit: {tail}"


if __name__ == "__main__":
    sys.exit(main())
