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


def gen_list_case(rng: random.Random) -> str:
    """列表采样：字面量 / len / 定长索引 / append 后 len / 成员测试。"""
    elems = [rng.randint(-1000, 1000) for _ in range(rng.randint(2, 5))]
    lines = [f"v = {elems!r}"]
    lines.append("print(len(v))")
    lines.append(f"print(v[{rng.randrange(len(elems))}])")
    x = rng.randint(-1000, 1000)
    lines.append(f"v.append({x})")
    lines.append("print(len(v))")
    probe = rng.choice(elems + [x, 99999])
    lines.append(f"print({probe} in v)")
    return "\n".join(lines) + "\n"


def gen_dict_case(rng: random.Random) -> str:
    """字典采样：str 键字面量 / len / 键读 / 覆盖写后读 / 成员测试。"""
    keys = rng.sample(["a", "b", "c", "dd", "key1", "z"], rng.randint(2, 4))
    d = {k: rng.randint(-500, 500) for k in keys}
    items = ", ".join(f'"{k}": {v}' for k, v in d.items())
    lines = [f"d = {{{items}}}"]
    lines.append("print(len(d))")
    k0 = rng.choice(keys)
    lines.append(f'print(d["{k0}"])')
    k1 = rng.choice(keys)
    lines.append(f'd["{k1}"] = {rng.randint(-500, 500)}')
    lines.append(f'print(d["{k1}"])')
    probe = rng.choice(keys + ["missing"])
    lines.append(f'print("{probe}" in d)')
    return "\n".join(lines) + "\n"


def gen_cmp_case(rng: random.Random) -> str:
    """比较链采样：Python 链式比较（a<b<c 是 (a<b) and (b<c)）、一元 not、
    浮点比较（只打印布尔，不打印浮点——呈现层噪声）。"""
    lines = []
    for _ in range(rng.randint(2, 4)):
        shape = rng.random()
        if shape < 0.4:
            a, b, c = (rng.randint(-100, 1000) for _ in range(3))
            o1, o2 = rng.choice(CMPS), rng.choice(CMPS)
            lines.append(f"print({a} {o1} {b} {o2} {c})")
        elif shape < 0.65:
            x = rng.randint(-100, 100)
            lines.append(f"print(not {x})")
        elif shape < 0.85:
            fa = round(rng.uniform(-100, 100), 2)
            fb = round(rng.uniform(-100, 100), 2)
            o = rng.choice(CMPS)
            lines.append(f"print({fa} {o} {fb})")
        else:
            a, b = rng.randint(-50, 50), rng.randint(-50, 50)
            lines.append(f"print(-(-{a}) < {b}, {a} == {a})")
    return "\n".join(lines) + "\n"


def gen_loop_case(rng: random.Random) -> str:
    """循环积累采样：for-range/while 的计数器槽、累加变量类型流（M08 家族）。"""
    n = rng.randint(3, 12)
    lines = []
    shape = rng.random()
    if shape < 0.35:
        lines += ["s = 0", f"for i in range({n}):", "    s += i", "print(s)"]
    elif shape < 0.55:
        lines += ["t = 1", f"for k in range(1, {rng.randint(2, 7)}):", "    t = t * k", "print(t)"]
    elif shape < 0.75:
        m = rng.randint(1, 5)
        lines += ["acc = 0", f"n = {m}", "while n > 0:", "    acc += n", "    n -= 1", "print(acc)"]
    elif shape < 0.9:
        vals = [rng.randint(-50, 50) for _ in range(rng.randint(2, 5))]
        lines += ["tot = 0", f"for x in {vals!r}:", "    tot += x", "print(tot)"]
    else:
        start = rng.randint(0, 10)
        end = start + rng.randint(1, 9)
        step = rng.randint(1, 3)
        lines += [f"c = 0", f"for i in range({start}, {end}, {step}):", "    c += i", "print(c)"]
    return "\n".join(lines) + "\n"


FMT_VALS = [("x", "65"), ("n", "-42"), ("w", "123456"), ("f", "2.71828"), ("s", "ab")]
INT_TYPES = ["d", "x", "o", "b"]
FLOAT_TYPES = ["f", "e"]


def gen_fmt_case(rng: random.Random) -> str:
    """f-string 规格采样：进制/符号/宽度/补零/对齐填充/精度 的随机合法组合。
    ','（千分位，t505 钉）与 'c'（t504 钉）刻意排除——已在 known-fail。"""
    var, val = rng.choice(FMT_VALS)
    spec = ""
    is_float = var == "f"
    is_str = var == "s"
    if not is_str and rng.random() < 0.35:
        fill = rng.choice("*<>=^")
        spec += fill if fill in "<>^" else fill + rng.choice("<>^")
    elif rng.random() < 0.4:
        spec += rng.choice("<>^")
    if not is_str and rng.random() < 0.25:
        spec += "+" if val.startswith("-") is False else ""
    if not is_str and rng.random() < 0.3:
        spec += "0" if not spec.endswith(("<", ">", "^")) else ""
    if rng.random() < 0.6:
        spec += str(rng.randint(2, 10))
    if is_float and rng.random() < 0.5:
        spec += f".{rng.randint(1, 4)}"
    if is_str:
        if rng.random() < 0.4:
            spec += f".{rng.randint(1, 3)}"
    else:
        spec += rng.choice(FLOAT_TYPES if is_float else INT_TYPES)
    return f'{var} = {val}\nprint(f"{{{var}:{spec}}}")\n'


def gen_slice_case(rng: random.Random) -> str:
    """字符串切片采样：正/负/省略/步进切片 + 切片后取 len。字母表保证全匹配。"""
    s = "abcdef"
    lines = []
    for _ in range(rng.randint(3, 5)):
        shape = rng.random()
        if shape < 0.2:
            a, b = sorted(rng.sample(range(0, 7), 2))
            lines.append(f'print("{s}"[{a}:{b}])')
        elif shape < 0.4:
            lines.append(f'print("{s}"[:{rng.randint(0, 6)}])')
        elif shape < 0.6:
            lines.append(f'print("{s}"[{rng.randint(0, 6)}:])')
        elif shape < 0.8:
            a = rng.randint(-6, 0)
            lines.append(f'print("{s}"[{a}:])')
        else:
            lines.append(f'print("{s}"[1:6:{rng.choice([1, 2, 3])}])')
    lines.append(f'print(len("{s}"[1:4]))')
    return "\n".join(lines) + "\n"


def gen_builtin_case(rng: random.Random) -> str:
    """内建函数采样：sum/min/max/sorted/abs/round(int)/len 在随机容器与数值上。
    刻意避开 round(f, n) 的浮点打印呈现族与 str 相加的 repr 差异。"""
    vals = [rng.randint(-999, 999) for _ in range(rng.randint(2, 6))]
    lines = [f"v = {vals!r}"]
    lines.append(f"print(sum({vals!r}))")
    lines.append(f"print(min({vals!r}))")
    lines.append(f"print(max({vals!r}))")
    if rng.random() < 0.6:
        lines.append(f"print(sorted({vals!r}))")
    lines.append(f"print(abs({rng.choice(vals)}))")
    lines.append(f"print(round({rng.randint(-99, 99)} / 10))")
    lines.append(f"print(len({vals!r}) + sum({vals[:2]!r}))")
    return "\n".join(lines) + "\n"


def gen_control_case(rng: random.Random) -> str:
    """Python 特有控制流采样：for/while 的 else（无 break 才执行）、break、continue 的随机组合。"""
    n = rng.randint(3, 8)
    break_at = rng.randint(-1, n)          # -1 = 无 break
    use_while = rng.random() < 0.4
    skip = rng.randint(-1, n)              # -1 = 无 continue
    lines = ["s = 0"]
    if use_while:
        lines += [f"i = 0", f"while i < {n}:"]
    else:
        lines += [f"for i in range({n}):"]
    lines += ["    i2 = i" if False else "    s += i"]
    if skip >= 0:
        lines += [f"    if i == {skip}:", "        continue", "    s += 1"]
    if 0 <= break_at < n:
        lines += [f"    if i == {break_at}:", "        break", "    s += 2"]
    else:
        lines += ["    s += 2"]
    lines += ["else:", "    s += 100"]
    lines += ["print(s)"]
    return "\n".join(lines) + "\n"


DKEYS = ["alpha", "beta", "gamma", "dd", "k1", "z9"]


def gen_dmethod_case(rng: random.Random) -> str:
    """字典方法链采样：len / get 带缺省 / 成员测试 / 覆盖写后读——
    键池刻意小（覆盖写与命中/缺省两路都会被采到）。"""
    chosen = rng.sample(DKEYS, rng.randint(2, 4))
    pairs = ", ".join(f'"{k}": {rng.randint(-99, 99)}' for k in chosen)
    lines = [f"d = {{{pairs}}}"]
    for _ in range(rng.randint(3, 5)):
        op = rng.random()
        k = rng.choice(DKEYS)
        if op < 0.3:
            lines.append(f'print(d.get("{k}", {rng.randint(-99, 99)}))')
        elif op < 0.5:
            lines.append(f'print("{k}" in d)')
        elif op < 0.7:
            v = rng.randint(-99, 99)
            lines.append(f'd["{k}"] = {v}')
            lines.append(f'print(d["{k}"])')
        else:
            lines.append("print(len(d))")
    return "\n".join(lines) + "\n"


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




def gen_stmts_case(rng: random.Random) -> tuple[str, str] | None:
    """语句模式：3-5 条赋值链 + 汇总 print——让未标注变量的类型流经
    签名表/调用点证据（单表达式模式测不到的那条路径）。"""
    n = rng.randint(3, 5)
    lines = []
    names = []
    for i in range(n):
        rhs = gen_expr(rng, depth=2)
        name = f"v{i}"
        lines.append(f"{name} = {rhs}" if i == 0 else f"{name} = {rhs}")
        names.append(name)
    tail = " + ".join(names) if rng.random() < 0.5 else " + ".join(reversed(names))
    prog = "\n".join(lines) + f"\nprint({tail})\n"
    return prog, prog


def python_eval_program(text: str) -> str | None:
    """多行程序版 oracle：原样执行，不包 print。"""
    try:
        p = subprocess.run(
            [sys.executable, "-c", text],
            capture_output=True, text=True, timeout=5,
        )
    except subprocess.TimeoutExpired:
        return None
    return p.stdout if p.returncode == 0 else None


def write_stmts_case(a, rng, idx: int) -> bool:
    prog = gen_stmts_case(rng)
    if prog is None:
        return False
    text, _ = prog
    expected = python_eval_program(text)
    if expected is None:
        return False  # CPython 侧报错，不进分母
    # 溢出族过滤：链式乘法极易爆 i64（已知独立族），不滤会把其他缺口淹没
    try:
        if abs(int(expected.strip())) >= 2**62:
            return False
    except ValueError:
        pass
    name = f"gen_stmts_s{a.seed}_{idx:03d}.dcase"
    body = (
        f"# @cat: numeric\n"
        f"# @note: 随机赋值链（seed={a.seed} #{idx}）——变量类型流经签名表\n"
        f"#@@ python\n{text}\n"
        f"#@@ zeta\n{text}\n"
    )
    with open(os.path.join(a.out, name), "w") as f:
        f.write(body)
    return True


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, required=True, help="固定种子保证可复现")
    ap.add_argument("--count", type=int, default=20)
    ap.add_argument("--mode", choices=("numeric", "str", "stmts", "list", "dict", "cmp", "loop", "fmt", "slice", "builtin", "control", "dmethod"), default="numeric")
    ap.add_argument("--out", default=OUT_DIR)
    a = ap.parse_args()

    rng = random.Random(a.seed)
    written = 0
    tried = 0
    max_tries = a.count * (30 if a.mode in ("stmts", "list", "dict", "cmp", "loop", "fmt", "slice", "builtin", "control", "dmethod") else 6)
    while written < a.count and tried < max_tries:
        tried += 1
        if a.mode == "builtin":
            prog = gen_builtin_case(rng)
            expected = python_eval_program(prog)
            if expected is None:
                continue
            name = f"gen_builtin_s{a.seed}_{written:03d}.dcase"
            body = (
                f"# @cat: container\n"
                f"# @note: 随机内建函数（seed={a.seed} #{written}）\n"
                f"#@@ python\n{prog}\n"
                f"#@@ zeta\n{prog}\n"
            )
            with open(os.path.join(a.out, name), "w") as f:
                f.write(body)
            written += 1
            continue
        if a.mode == "slice":
            prog = gen_slice_case(rng)
            expected = python_eval_program(prog)
            if expected is None:
                continue
            name = f"gen_slice_s{a.seed}_{written:03d}.dcase"
            body = (
                f"# @cat: str\n"
                f"# @note: 随机字符串切片（seed={a.seed} #{written}）\n"
                f"#@@ python\n{prog}\n"
                f"#@@ zeta\n{prog}\n"
            )
            with open(os.path.join(a.out, name), "w") as f:
                f.write(body)
            written += 1
            continue
        if a.mode == "dmethod":
            prog = gen_dmethod_case(rng)
            expected = python_eval_program(prog)
            if expected is None:
                continue
            name = f"gen_dmethod_s{a.seed}_{written:03d}.dcase"
            body = (
                f"# @cat: container\n"
                f"# @note: 随机字典方法链（seed={a.seed} #{written}）\n"
                f"#@@ python\n{prog}\n"
                f"#@@ zeta\n{prog}\n"
            )
            with open(os.path.join(a.out, name), "w") as f:
                f.write(body)
            written += 1
            continue
        if a.mode == "control":
            prog = gen_control_case(rng)
            expected = python_eval_program(prog)
            if expected is None:
                continue
            name = f"gen_control_s{a.seed}_{written:03d}.dcase"
            body = (
                f"# @cat: control\n"
                f"# @note: 随机控制流（seed={a.seed} #{written}）——else/break/continue 组合\n"
                f"#@@ python\n{prog}\n"
                f"#@@ zeta\n{prog}\n"
            )
            with open(os.path.join(a.out, name), "w") as f:
                f.write(body)
            written += 1
            continue
        if a.mode == "fmt":
            prog = gen_fmt_case(rng)
            expected = python_eval_program(prog)
            if expected is None:
                continue
            name = f"gen_fmt_s{a.seed}_{written:03d}.dcase"
            body = (
                f"# @cat: str\n"
                f"# @note: 随机 f-string 规格（seed={a.seed} #{written}）\n"
                f"#@@ python\n{prog}\n"
                f"#@@ zeta\n{prog}\n"
            )
            with open(os.path.join(a.out, name), "w") as f:
                f.write(body)
            written += 1
            continue
        if a.mode == "loop":
            prog = gen_loop_case(rng)
            expected = python_eval_program(prog)
            if expected is None:
                continue
            name = f"gen_loop_s{a.seed}_{written:03d}.dcase"
            body = (
                f"# @cat: control\n"
                f"# @note: 随机循环积累（seed={a.seed} #{written}）\n"
                f"#@@ python\n{prog}\n"
                f"#@@ zeta\n{prog}\n"
            )
            with open(os.path.join(a.out, name), "w") as f:
                f.write(body)
            written += 1
            continue
        if a.mode == "cmp":
            prog = gen_cmp_case(rng)
            expected = python_eval_program(prog)
            if expected is None:
                continue
            name = f"gen_cmp_s{a.seed}_{written:03d}.dcase"
            body = (
                f"# @cat: truth\n"
                f"# @note: 随机比较链（seed={a.seed} #{written}）\n"
                f"#@@ python\n{prog}\n"
                f"#@@ zeta\n{prog}\n"
            )
            with open(os.path.join(a.out, name), "w") as f:
                f.write(body)
            written += 1
            continue
        if a.mode in ("list", "dict"):
            prog = gen_list_case(rng) if a.mode == "list" else gen_dict_case(rng)
            expected = python_eval_program(prog)
            if expected is None:
                continue
            name = f"gen_{a.mode}_s{a.seed}_{written:03d}.dcase"
            body = (
                f"# @cat: container\n"
                f"# @note: 随机容器（seed={a.seed} #{written}）—— CPython 先验通过\n"
                f"#@@ python\n{prog}\n"
                f"#@@ zeta\n{prog}\n"
            )
            with open(os.path.join(a.out, name), "w") as f:
                f.write(body)
            written += 1
            continue
        if a.mode == "stmts":
            written_stmts = write_stmts_case(a, rng, written)
            if written_stmts:
                written += 1
            continue
        # numeric / str 共用单表达式路径（472/474）
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
