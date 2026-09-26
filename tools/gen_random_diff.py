#!/usr/bin/env python3
"""tools/gen_random_diff.py —— G.3 进阶档第一块：随机算术表达式差分用例生成器。

refactor.md G.3 的进阶档："受限语法的随机程序生成 + CPython oracle"——
人工 curated 用例的覆盖面 = 想到的面；生成器把采样空间铺开，
让"还有多少语义是错的"从枚举变成度量。

v1 表达式文法（刻意收窄，绕开呈现层噪声，见 diff_test.py 已知边界 1）：
  - 整数字面量（含负数、绝对值 <= 2**48，避免 i64 溢出类已知缺口混入统计）
  - 二元算术：+ - * // %（** 除零会产出 bad_case 由 CPython 先验滤掉）
  - 移位/位运算：<< >> & | ^（移位量钳在 0..63，上界缺口单独立案不用生成器刷）
  - 比较：== != < <= > >=
  - 逻辑：and or not（Python 语义返回操作数，两边一致即可比）
  - 不含：/（真除法产生 float 打印差异）、字符串、容器、浮点

流程：生成候选表达式 → CPython 先验求值（报错 = 丢，不进分母）→
写 .dcase（python 形与 zeta 形同文，oracle 即 CPython 实跑输出）→
diff_test.py 判 match/mismatch。mismatch = zeta 侧真语义缺口，进批次读数。

用法：
  python3 tools/gen_random_diff.py --seed 20260926 --count 20   # 写 tests/diff/cases/
  固定种子 ⇒ 完全可复现；换种子 = 新一批采样，老用例不重生成。
"""
from __future__ import annotations

import argparse
import os
import random
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "tests", "diff", "cases")

STR_ALPHABET = ["a", "b", "ab", "ba", "abc"]
STR_METHODS_VAL = ["upper", "lower", "strip"]          # 返回字符串
STR_METHODS_INT = ["find", "count", "len"]             # 返回整数
STR_METHODS_BOOL = ["startswith", "endswith"]          # 返回布尔（Bool 性族采样）

BINOPS = ["+", "-", "*", "//", "%", "&", "|", "^"]
SHIFTS = ["<<", ">>"]
CMPS = ["==", "!=", "<", "<=", ">", ">="]
LOGICS = ["and", "or"]


def rand_int(rng: random.Random) -> int:
    v = rng.randint(0, 2**48 - 1)
    return -v if rng.random() < 0.35 else v


def gen_str_expr(rng: random.Random, depth: int = 0) -> str:
    """字符串表达式：拼接 / 整数重复 / 定长索引 / 比较 / 常用方法。
    刻意不含 split/sort（列表打印的 repr 差异属呈现层）与越界索引。"""
    if depth >= 2 or rng.random() < 0.35:
        q = rng.choice(['"', "'"])
        return q + rng.choice(STR_ALPHABET) + q
    kind = rng.random()
    if kind < 0.3:
        return f"{gen_str_expr(rng, depth+1)} + {gen_str_expr(rng, depth+1)}"
    if kind < 0.45:
        return f"{gen_str_expr(rng, depth+1)} * {rng.randint(1, 4)}"
    if kind < 0.55:
        s_var = gen_str_expr(rng, depth + 1)
        idx = rng.randint(0, 2)
        return f"({s_var})[{idx}]"
    if kind < 0.65:
        a = gen_str_expr(rng, depth + 1)
        b = gen_str_expr(rng, depth + 1)
        return f"({a} {rng.choice(['==', '!=', '<'])} {b})"
    m = rng.choice(STR_METHODS_VAL + STR_METHODS_INT + STR_METHODS_BOOL)
    if m == "len":
        return f"len({gen_str_expr(rng, depth+1)})"
    if m in STR_METHODS_BOOL:
        return f"{gen_str_expr(rng, depth+1)}.startswith({gen_str_expr(rng, depth+1)})"
    arg = ""
    if m == "replace" or m == "find":
        arg = f', {gen_str_expr(rng, depth+1)}'
    return f"({gen_str_expr(rng, depth+1)}).{m}({gen_str_expr(rng, depth+1)}{arg})"


def gen_expr(rng: random.Random, depth: int = 0) -> str:
    if depth >= 3 or rng.random() < 0.3:
        return str(rand_int(rng))
    kind = rng.random()
    if kind < 0.55:
        op = rng.choice(BINOPS)
    elif kind < 0.7:
        op = rng.choice(SHIFTS)
    elif kind < 0.85:
        op = rng.choice(CMPS)
    else:
        op = rng.choice(LOGICS)
    a = gen_expr(rng, depth + 1)
    b = gen_expr(rng, depth + 1)
    if op in SHIFTS:
        # 移位量钳 0..63：上界缺口（1<<64）已单独立案，生成器不重复刷它
        b = str(rng.randint(0, 63))
    expr = f"{a} {op} {b}"
    return f"({expr})" if rng.random() < 0.5 else expr


def python_eval(expr: str) -> str | None:
    """CPython oracle：返回 stdout；报错（除零/巨量移位等）返回 None = 丢弃。"""
    try:
        p = subprocess.run(
            [sys.executable, "-c", f"print({expr})"],
            capture_output=True, text=True, timeout=5,
        )
    except subprocess.TimeoutExpired:
        return None
    return p.stdout if p.returncode == 0 else None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, required=True, help="固定种子保证可复现")
    ap.add_argument("--count", type=int, default=20)
    ap.add_argument("--mode", choices=("numeric", "str"), default="numeric")
    ap.add_argument("--out", default=OUT_DIR)
    a = ap.parse_args()

    rng = random.Random(a.seed)
    written = 0
    tried = 0
    while written < a.count and tried < a.count * 6:
        tried += 1
        expr = gen_str_expr(rng) if a.mode == "str" else gen_expr(rng)
        expected = python_eval(expr)
        if expected is None:
            continue  # CPython 侧报错 = bad_case，不进分母，直接不写
        name = f"gen_{a.mode}_s{a.seed}_{written:03d}.dcase"
        body = (
            f"# @cat: {a.mode}\n"
            f"# @note: 随机生成（seed={a.seed} #{written}）—— CPython 先验通过，"
            f"oracle = 实跑输出\n"
            f"#@@ python\nprint({expr})\n"
            f"#@@ zeta\nprint({expr})\n"
        )
        with open(os.path.join(a.out, name), "w") as f:
            f.write(body)
        written += 1
    print(f"生成 {written}/{tried} 条（CPython 先验淘汰 {tried - written}）→ {a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
