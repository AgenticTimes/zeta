#!/usr/bin/env python3
"""tools/gen_diff_cases.py —— 受限语法随机程序生成（refactor.md G.3 进阶 / 任务 #49 第一段）

为什么需要它：`tests/diff/cases/` 的 111 条用例是人 curated 的，覆盖面 = 想到的面
（diff_test.py 的"已知边界 3"自己写着这件事）。这批要把它变成**采样空间**。

三条硬约束，缺一条这套东西就只会产噪音：
  1) **只生成结果必为 int/bool/str/container 的表达式** —— 浮点会撞上 `%.6f` 的呈现层
     差异（#48），随机程序在那边只会产出 200 条同一个原因的 mismatch。
     所以 `/`、`//`、浮点字面量整族都不在语法里（#48/#51 已登记，不需要随机程序再发现一遍）。
  2) **绝对值可静态界定** —— 每个 int 表达式携带一个"值域上界"，超限的构造直接拒绝。
     否则随机乘法会撞 i64 溢出，那与任意精度整数是同一个已知族（#48）。
  3) **必然停机** —— while 只生成"`i = 0` + `while i < N` + 体内 `i = i + 1`"这一种计数器形状。

产出的是**合法的 `.dcase`**（两份渲染同文本；`# @note:` 里带 seed，可复现），
生成与判定分离：本脚本先写临时目录 + 现场跑一遍分类，人看完读数再 --promote 进库。
"""

from __future__ import annotations

import argparse
import importlib.util
import random
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CASE_DIR = ROOT / "tests" / "diff" / "cases"

# 复用 harness 的执行/判定层，避免两套口径（它自己负责 compile/runtime/mismatch 的分层）。
_spec = importlib.util.spec_from_file_location("diff_test", ROOT / "tools" / "diff_test.py")
diff_test = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(diff_test)

MAX_ABS = 10**6  # 约束 2：任何 int 表达式的 |值| 上界，远低于 i64
INT_LITS = ["0", "1", "2", "3", "5", "7", "12"]
STR_LITS = ['"ab"', '"x"', '"zeta"']


class Int:
    """int 表达式：文本 + 静态值域上界（约束 2 的载体）。"""

    def __init__(self, text: str, bound: int) -> None:
        self.text, self.bound = text, bound


class Bool:
    def __init__(self, text: str) -> None:
        self.text = text


class Str:
    def __init__(self, text: str, bound: int) -> None:
        self.text, self.bound = text, bound


class List:
    def __init__(self, text: str, length: int) -> None:
        self.text, self.length = text, length


class Scope:
    """一次生成的符号表：变量名 -> 表达式对象。长度/值域都跟着走。"""

    def __init__(self) -> None:
        self.ints: dict[str, Int] = {}
        self.bools: dict[str, Bool] = {}
        self.strs: dict[str, Str] = {}
        self.lists: dict[str, List] = {}

    def fresh(self, kind: str) -> str:
        n = len(self.ints) + len(self.bools) + len(self.strs) + len(self.lists)
        return f"v{kind}{n}"


def gen_int(rng: random.Random, sc: Scope, depth: int) -> Int:
    choices = ["lit", "var", "add", "sub", "mul", "mod", "pow", "neg",
               "cond", "call", "len", "sum", "minmax", "index"]
    if depth <= 0:
        choices = ["lit"] + (["var"] if sc.ints else [])
    op = rng.choice(choices)
    if op == "lit":
        t = rng.choice(INT_LITS)
        return Int(t, int(t))
    if op == "var" and sc.ints:
        v = rng.choice(sorted(sc.ints))
        return Int(v, sc.ints[v].bound)
    if op == "add":
        a, b = gen_int(rng, sc, depth - 1), gen_int(rng, sc, depth - 1)
        if a.bound + b.bound > MAX_ABS:
            return a
        return Int(f"({a.text} + {b.text})", a.bound + b.bound)
    if op == "sub":
        a, b = gen_int(rng, sc, depth - 1), gen_int(rng, sc, depth - 1)
        if a.bound + b.bound > MAX_ABS:
            return a
        # 减出负数是地板取模唯一的信号来源（正数 `%` 与 C 的 srem 同值），
        # 所以减法的值域上界要保留，别退化成 `abs(...)`
        return Int(f"({a.text} - {b.text})", a.bound + b.bound)
    if op == "mul":
        a, b = gen_int(rng, sc, depth - 1), gen_int(rng, sc, depth - 1)
        if a.bound * b.bound > MAX_ABS:
            return a
        return Int(f"{a.text} * {b.text}", a.bound * b.bound)
    if op == "mod":
        a = gen_int(rng, sc, depth - 1)
        k = rng.randint(1, 9)
        # 除数带符号才测得到地板语义：正数走字面量，负数走 `(0 - k)`（同一批改的两条路）
        rhs = str(k) if rng.random() < 0.5 else f"(0 - {k})"
        return Int(f"{a.text} % {rhs}", k)
    if op == "pow":
        base = gen_int(rng, sc, depth - 1)
        e = rng.randint(0, 3)
        if base.bound**e > MAX_ABS:
            return base
        return Int(f"{base.text} ** {e}", base.bound**e)
    if op == "neg":
        a = gen_int(rng, sc, depth - 1)
        return Int(f"abs(0 - {a.text})", a.bound) if rng.random() < 0.5 else Int(f"-{a.text}", a.bound)
    if op == "cond":
        c, a, b = gen_bool(rng, sc, depth - 1), gen_int(rng, sc, depth - 1), gen_int(rng, sc, depth - 1)
        return Int(f"({a.text} if {c.text} else {b.text})", max(a.bound, b.bound))
    if op == "call":
        a = gen_int(rng, sc, depth - 1)
        return Int(f"inc({a.text})", a.bound + 1)
    if op == "len":
        l = rng.choice(sorted(sc.lists))
        return Int(f"len({l})", sc.lists[l].length)
    if op == "sum":
        l = rng.choice(sorted(sc.lists))
        return Int(f"sum({l})", sc.lists[l].length * 13)
    if op == "minmax":
        # 参数只能是 int：bool 进 min/max 是 #50（已知红），不在随机空间里重复发现
        a, b = gen_int(rng, sc, depth - 1), gen_int(rng, sc, depth - 1)
        return Int(f"{'min' if rng.random() < 0.5 else 'max'}({a.text}, {b.text})", max(a.bound, b.bound))
    if op == "index" and sc.lists:
        name = rng.choice(sorted(sc.lists))
        ln = sc.lists[name].length
        i = rng.randrange(ln) if rng.random() < 0.7 else -rng.randrange(1, ln + 1)
        return Int(f"{name}[{i if i >= 0 else f'0 - {-i}'}]", 12)
    return Int(rng.choice(INT_LITS), 12)


def gen_bool(rng: random.Random, sc: Scope, depth: int) -> Bool:
    choices = ["cmp", "eq", "not", "andor", "in"]
    if sc.bools:
        choices.append("var")
    if depth <= 0:
        choices = ["cmp"] + (["var"] if sc.bools else [])
    op = rng.choice(choices)
    if op == "var":
        v = rng.choice(sorted(sc.bools))
        return Bool(v)
    if op == "cmp":
        a, b = gen_int(rng, sc, depth - 1), gen_int(rng, sc, depth - 1)
        return Bool(f"{a.text} {rng.choice(['<', '>', '<=', '>='])} {b.text}")
    if op == "eq":
        a, b = gen_int(rng, sc, depth - 1), gen_int(rng, sc, depth - 1)
        return Bool(f"{a.text} {rng.choice(['==', '!='])} {b.text}")
    if op == "not":
        return Bool(f"not ({gen_bool(rng, sc, depth - 1).text})")
    if op == "andor":
        a, b = gen_bool(rng, sc, depth - 1), gen_bool(rng, sc, depth - 1)
        return Bool(f"({a.text} {rng.choice(['and', 'or'])} {b.text})")
    if op == "in" and sc.lists:
        name = rng.choice(sorted(sc.lists))
        return Bool(f"{gen_int(rng, sc, depth - 1).text} in {name}")
    if sc.strs:
        s = rng.choice(sorted(sc.strs))
        return Bool(f"{s} {rng.choice(['==', '!='])} {rng.choice(STR_LITS)}")
    return Bool(f"{gen_int(rng, sc, depth - 1).text} > 0")


def gen_str(rng: random.Random, sc: Scope, depth: int) -> Str:
    op = rng.choice(["lit", "var", "cat"])
    if op == "var" and sc.strs:
        v = rng.choice(sorted(sc.strs))
        return Str(v, sc.strs[v].bound)
    if op == "cat" and depth > 0:
        a, b = gen_str(rng, sc, depth - 1), gen_str(rng, sc, depth - 1)
        return Str(f"{a.text} + {b.text}", a.bound + b.bound)
    t = rng.choice(STR_LITS)
    return Str(t, len(t) - 2)


def gen_program(rng: random.Random) -> tuple[str, str]:
    """返回 (cat, 程序文本)。程序 = 固定头（inc + 三个容器）+ 若干随机语句。"""
    sc = Scope()
    body: list[str] = []
    kinds: list[str] = []

    # 固定头：让 `inc(...)`、`len(l)`、`sum(l)` 的下标/长度都有静态依据
    head = [
        "def inc(x) -> int:",
        "    return x + 1",
        "",
        "l = [3, 5, 7]",
        "s = \"ab\"",
        "d = {\"a\": 1, \"b\": 2}",
    ]
    sc.lists["l"] = List("l", 3)
    sc.strs["s"] = Str("s", 2)

    for _ in range(rng.randint(4, 7)):
        pick = rng.choice(["print_int", "print_bool", "print_str", "assign_int",
                           "assign_bool", "append", "if", "loop", "for", "dict"])
        if pick == "print_int":
            body.append(f"print({gen_int(rng, sc, 3).text})")
            kinds.append("numeric")
        elif pick == "print_bool":
            body.append(f"print({gen_bool(rng, sc, 2).text})")
            kinds.append("truth")
        elif pick == "print_str":
            body.append(f"print({gen_str(rng, sc, 2).text})")
            kinds.append("str")
        elif pick == "assign_int":
            n = sc.fresh("i")
            rhs = gen_int(rng, sc, 3)
            body.append(f"{n} = {rhs.text}")
            sc.ints[n] = Int(n, rhs.bound)
        elif pick == "assign_bool":
            n = sc.fresh("b")
            body.append(f"{n} = {gen_bool(rng, sc, 2).text}")
            sc.bools[n] = Bool(n)
        elif pick == "append":
            body.append(f"l.append({gen_int(rng, sc, 2).text})")
            sc.lists["l"] = List("l", sc.lists["l"].length + 1)
            kinds.append("container")
        elif pick == "if":
            c = gen_bool(rng, sc, 2)
            body.append(f"if {c.text}:")
            body.append(f"    print({gen_int(rng, sc, 2).text})")
            body.append("else:")
            body.append(f"    print({gen_int(rng, sc, 2).text})")
            kinds.append("control")
        elif pick == "loop":
            # 约束 3：只允许这一种计数器形状
            n = rng.randint(2, 4)
            body.append("i = 0")
            body.append(f"while i < {n}:")
            body.append("    print(i * 2)")
            body.append("    i = i + 1")
            sc.ints["i"] = Int("i", n)
            kinds.append("control")
        elif pick == "for":
            n = rng.randint(1, 4)
            body.append(f"for k in range({n}):")
            body.append(f"    print(k + {gen_int(rng, sc, 1).text})")
            sc.ints["k"] = Int("k", n + 12)
            kinds.append("control")
        elif pick == "dict":
            which = rng.choice(["len", "in", "index"])
            if which == "len":
                body.append("print(len(d))")
            elif which == "in":
                body.append(f'print({"\"a\"" if rng.random() < 0.5 else "\"z\""} in d)')
            else:
                body.append(f'print(d[{"\"a\"" if rng.random() < 0.5 else "\"b\""}])')
            kinds.append("container")
    cat = max(set(kinds), key=kinds.count) if kinds else "numeric"
    return cat, "\n".join(head + body) + "\n"


def promote(src_dir: Path, names: list[str]) -> None:
    """把随机生成目录里的用例挑进 `tests/diff/cases/`（库里已有同名则跳过）。"""
    for n in names:
        src = src_dir / n
        if not src.exists():
            print(f"[E2001] 没有生成过 {n}（seed/outdir 对不上？）")
            continue
        dst = CASE_DIR / n
        if dst.exists():
            print(f"跳过（库里已有同名）：{n}")
            continue
        shutil.copy2(src, dst)
        print(f"promote {n}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--seed", type=int, default=20260922)
    ap.add_argument("--count", type=int, default=60)
    ap.add_argument("--outdir", default=str(Path(tempfile.gettempdir()) / "zeta_gen_cases"))
    ap.add_argument("--promote", metavar="NAME", nargs="*", default=None,
                    help="把 --outdir 里已生成的用例按名字复制进 tests/diff/cases（默认不写库）")
    args = ap.parse_args()

    out = Path(args.outdir)
    if args.promote is not None:
        promote(out, args.promote or sorted(p.name for p in out.glob("gen*.dcase")))
        return 0

    rng = random.Random(args.seed)
    out.mkdir(parents=True, exist_ok=True)
    for p in out.glob("gen*.dcase"):
        p.unlink()

    workdir = Path(tempfile.mkdtemp(prefix="zeta_gen_run."))
    tally: dict[str, int] = {}
    rows: list[tuple[str, str, str]] = []
    try:
        for i in range(1, args.count + 1):
            cat, src = gen_program(rng)
            name = f"gen{i:03d}_{cat}"
            path = out / f"{name}.dcase"
            path.write_text(
                f"# @cat: {cat}\n"
                f"# @note: 随机生成 seed={args.seed} #{i}（受限语法：无 / // 浮点，值域有界，必停机）\n"
                f"#@@ python\n{src}#@@ zeta\n{src}",
                encoding="utf-8",
            )
            case = diff_test.parse_case(path)
            try:
                ref = diff_test.run_ref(case["python"], workdir)
            except diff_test.BadCase as e:
                verdict, detail = "bad_case", str(e)
            else:
                verdict, got, detail = diff_test.run_zeta(case["zeta"], name, workdir)
                if verdict == "ok":
                    verdict, detail = ("match", "") if got == ref else ("mismatch", first_diff(ref, got))
            tally[verdict] = tally.get(verdict, 0) + 1
            rows.append((name, verdict, detail))
    finally:
        shutil.rmtree(workdir, ignore_errors=True)

    for name, verdict, detail in rows:
        if verdict != "match":
            print(f"{verdict:9} {name}  {detail[:110]}")
    print("\n读数:", " ".join(f"{k}={v}" for k, v in sorted(tally.items())), f"（共 {args.count}，seed={args.seed}）")
    print(f"用例目录：{out}")
    return 0


def first_diff(ref: list[str], got: list[str]) -> str:
    for n, (a, b) in enumerate(zip(ref, got), 1):
        if a != b:
            return f"#{n} 期望 {a!r} 实得 {b!r}"
    if len(ref) != len(got):
        return f"行数 {len(ref)} vs {len(got)}"
    return ""


if __name__ == "__main__":
    sys.exit(main())
