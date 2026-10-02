#!/usr/bin/env python3
"""tools/check_runtime_doublewrite.py — #211③ 的机械核对器（批次 801）。

背景：运行期 extern 符号必须"双写"——gen.rs 以字面量发 `func: "NAME"` 调用，
而符号进 LLVM 模块有两条正路：codegen.rs 手写 `module.add_function` 清单，
或 `crate::middle::pylib::all_externs()` 的清单驱动声明（codegen.rs:1086，
只纳 `py_` 前缀或尾数裸形如 `clip_2` 的 registry 符号）。两条都不满足时
`get_function` 兜底（codegen.rs:2900 附近）静默按"1 参、void"发 extern——
多余实参不装寄存器（批次 535 实拍：`py_df_set_value_tag` 三参调用对一参 void
声明 ⇒ 全 0 列 + SIGSEGV；批 655 实拍：`zeta_big_to_f64` 按 i64 返回截双）。

满足链（与 codegen.rs 实拍一致，逐条镜像）：
  S1 codegen.rs 手写 add_function（形态一：字面名+fn_type 参表；
     形态二：`for (name, arity) in [...]` 元组清单）。
  S2 all_externs()：pylib/registry.txt 的 F/W/X 行，`decl=0`/`stub=1` 除外，
     再按 `py_` 前缀或"尾数后缀"（ends_with_argc_suffix）过滤。
  S3 pylib/*.z 里的 `extern fn NAME`（随库源码同模块编译，进模块声明）。
  S4 src/ 下 `#[no_mangle] ... fn NAME`（Rust 侧实现；链接期同符号满足）。
  S5 `str_*` → `host_str_*` / `identity_host_str_*` 重映射臂（codegen.rs:2848）：
     裸名不在任何清单、但 host 形态在 ⇒ 调用前已换名，不算缺口。

判定面：
  A. MISSING（风险）——gen.rs 字面量发射、根 .o 里 C 有定义、S1–S5 全不满足。
     实拍补正（本批 IR 量）：这类符号实际落 get_or_declare_function 的
     **按需 extern 声明**（"create extern declaration matching the call
     site's arg count"，codegen.rs:3286 一带）——按调用元数发 i64(i64×N)，
     元数正确、全整型 coerce；t545 `@zeta_big_new(i64,i64)`、t526
     `@py_zip(i64,i64)` 实拍绿。残余风险面＝C 原型含 f64 而按需声明按 i64
     装参（截断类，批 655 同形）——本批对 MISSING×f64 原型求交为零
     （唯一的 py_vec_clip_f64 由 S1 手写声明满足），故 53 条按登记面入基线。
  B. ARITY_MISMATCH——声明 arity 与发射 arity 不符，且 `_N` 后缀消歧形态
     （codegen param_suffixed 查找 :3138；C 侧 `array_new_1`、
     `py_threading_thread_new_2` 实拍存在）也救不了。
  C. HAND-DUP——手写清单内部同名多 arity。

已核实并登记的风险进基线 tools/baselines/runtime_doublewrite.txt（仿锚点核对
--bless 惯例）：默认模式与基线逐项对照——新增⇒rc=1；基线里已消失（修掉了）
⇒提示 --bless 收面；集合一致⇒rc=0 并报在册数。`--bless` 重写基线。
rc=2 ⇒ 输入缺失（gen.rs/codegen.rs/registry.txt 或 .o 不可读）。

覆盖口径如实登记（不算缺口也不进基线）：UNKNOWN_ARITY（args 是变量）、
名不可数（format!/变量名发射）、`Name::space` 限定名（走链接面另行报错）。
"""

import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GEN_RS = ROOT / "src" / "middle" / "mir" / "gen.rs"
CODEGEN_RS = ROOT / "src" / "backend" / "codegen" / "codegen.rs"
REGISTRY_TXT = ROOT / "pylib" / "registry.txt"
PYLIB_Z = sorted((ROOT / "pylib").glob("*.z"))
BASELINE = ROOT / "tools" / "baselines" / "runtime_doublewrite.txt"
OBJS = [ROOT / "zeta_runtime_c.o", ROOT / "tokio_runtime.o"]


def c_defined_symbols():
    syms = set()
    for obj in OBJS:
        if not obj.exists():
            continue
        out = subprocess.run(["nm", "-gU", str(obj)], capture_output=True, text=True).stdout
        for line in out.splitlines():
            parts = line.split()
            if len(parts) >= 3 and parts[-2] in ("T", "D", "B", "W"):
                s = parts[-1]
                syms.add(s[1:] if s.startswith("_") and not s.startswith("__") else s)
    return syms


def parse_counted_vec(text, start):
    """text[start] 处是 '['；返回 (顶层逗号项数, '['']'之后位置)，深度配平。
    尾随逗号不另计一项（Rust 字面量允许 `[a, b,]`）。"""
    depth = 0
    items = 0
    i = start
    pending = False  # 已见到未计数的顶层内容
    while i < len(text):
        ch = text[i]
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
            if depth == 0:
                if pending:
                    items += 1
                return (items, i + 1)
        elif ch == "," and depth == 1:
            if pending:
                items += 1
            pending = False
        elif ch not in " \t\n" and depth == 1:
            pending = True
        i += 1
    return None, i


def ends_with_argc_suffix(sym):
    """镜像 pylib.rs:398——尾段 `_数字` 是 C 符号名的一部分。"""
    if "_" not in sym:
        return False
    base, digits = sym.rsplit("_", 1)
    return bool(base) and bool(digits) and digits.isdigit()


def handshake_decls():
    """S1：codegen.rs 手写清单 name -> arity；同名多 arity 记 HAND-DUP。"""
    src = CODEGEN_RS.read_text()
    decls = {}
    dup = []

    def record(name, arity):
        if name in decls and decls[name] != arity:
            dup.append((name, decls[name], arity))
        decls[name] = arity

    for m in re.finditer(r'add_function\(\s*"([^"]+)"', src):
        name = m.group(1)
        tail = src[m.end(): m.end() + 600]
        fm = re.search(r"fn_type\(\s*\[", tail) or re.search(r"fn_type\(\s*&\[", tail)
        if not fm:
            continue
        arity, _ = parse_counted_vec(tail, fm.end() - 1)
        if arity is None:
            continue
        record(name, arity)
    for m in re.finditer(r'\(\s*"([A-Za-z_][\w\[\]:]*)"\s*,\s*(\d+)(?:usize)?\s*\)', src):
        record(m.group(1), int(m.group(2)))
    return decls, dup


def manifest_externs():
    """S2：镜像 all_externs()——registry F/W/X 行，decl&&!stub，
    再过滤 `py_` 前缀或尾数后缀。返回 name -> arity。"""
    names = {}
    for raw in REGISTRY_TXT.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        kind = parts[0]
        flags = {}
        positional = []
        for tok in parts[1:]:
            if "=" in tok and tok.split("=", 1)[0] in (
                "args", "ret", "handle", "ret_handle", "decl", "stub", "alias-of"):
                k, v = tok.split("=", 1)
                flags[k] = v
            else:
                positional.append(tok)
        if flags.get("decl", "1") in ("0", "false", "no") or \
           flags.get("stub", "0") in ("1", "true", "yes"):
            continue
        if kind == "F" and len(positional) >= 3:
            sym = positional[2]
            args = flags.get("args", "")
            arity = 0 if args == "" else len(args.split(","))
        elif kind == "W" and len(positional) >= 3:
            sym = positional[2]
            arity = int(flags.get("args", "1"))
        elif kind == "X" and len(positional) >= 1:
            sym = positional[0]
            args = flags.get("args", "")
            arity = 0 if args == "" else len(args.split(","))
        else:
            continue
        if sym.startswith("py_") or ends_with_argc_suffix(sym):
            names[sym] = arity
    return names


def pylib_extern_fns():
    """S3：pylib/*.z 的 `extern fn NAME` 名集。"""
    names = set()
    for f in PYLIB_Z:
        for m in re.finditer(r'\bextern\s+fn\s+([A-Za-z_]\w*)', f.read_text()):
            names.add(m.group(1))
    return names


def rust_no_mangle_fns():
    """S4：src/ 下 `#[no_mangle]` 紧邻的 pub fn 名集（批 763 让位机制）。"""
    names = set()
    out = subprocess.run(
        ["grep", "-rA3", "no_mangle", str(ROOT / "src")],
        capture_output=True, text=True).stdout
    for m in re.finditer(r'fn\s+([A-Za-z_]\w*)', out):
        names.add(m.group(1))
    return names


def emit_sites():
    src = GEN_RS.read_text()
    lines_offsets = [0]
    for ln in src.splitlines(keepends=True):
        lines_offsets.append(lines_offsets[-1] + len(ln))

    def lineno(pos):
        lo, hi = 0, len(lines_offsets) - 1
        while lo < hi:
            mid = (lo + hi) // 2
            if lines_offsets[mid + 1] <= pos:
                lo = mid + 1
            else:
                hi = mid
        return lo + 1

    sites, unknown, interp = [], [], []
    for m in re.finditer(r'func:\s*"([^"]+)"(?:\.to_string\(\))?', src):
        line = lineno(m.start())
        name = m.group(1)
        win = src[m.end(): m.end() + 400]
        am = re.search(r"args:\s*(vec!\[|[\w.:()\[\] ]+?,)", win)
        if am and am.group(1).startswith("vec!"):
            arity, _ = parse_counted_vec(win, am.end() - 1)
            if arity is None:
                unknown.append((name, line))
            else:
                sites.append((name, arity, line))
        else:
            unknown.append((name, line))
    for m in re.finditer(r'func:\s*(?:format!\([^)]*\)|[^"\n,]{1,60}?\.to_string\(\)|[a-z_][\w.]*(_name|func_name)?\(\)?)\s*,', src):
        if m.group(0).find('"') >= 0:
            continue
        interp.append((m.group(0)[:40].strip(), lineno(m.start())))
    return sites, unknown, interp


def satisfied(name, decls, manifest, pylib_ext, rust_fns):
    """S1–S5 满足链判定。返回 (是否满足, 声明 arity 或 None)。"""
    if name in decls:
        return True, decls[name]
    if name in manifest:
        return True, manifest[name]
    if name in pylib_ext or name in rust_fns:
        return True, None
    if name.startswith("str_"):
        for cand in (f"host_{name}", f"identity_host_{name}"):
            if cand in decls or cand in manifest or cand in rust_fns:
                return True, None
    return False, None


def key_missing(name, arities):
    return f"MISSING|{name}|arity={','.join(str(a) for a in arities)}"


def key_mismatch(name, want, got):
    return f"MISMATCH|{name}|decl={want}|emit={','.join(str(g) for g in got)}"


def key_dup(name, a1, a2):
    return f"DUP|{name}|{a1}vs{a2}"


def main():
    bless = "--bless" in sys.argv
    for p in (GEN_RS, CODEGEN_RS, REGISTRY_TXT):
        if not p.exists():
            print(f"missing {p}", file=sys.stderr)
            return 2
    defined = c_defined_symbols()
    if not defined:
        print("两个根 .o 都不在（nm 无量）——先 bash tools/build_runtime.sh", file=sys.stderr)
        return 2
    decls, dup = handshake_decls()
    manifest = manifest_externs()
    pylib_ext = pylib_extern_fns()
    rust_fns = rust_no_mangle_fns()
    sites, unknown, interp = emit_sites()

    per_name = defaultdict(set)
    for name, arity, line in sites:
        per_name[name].add((arity, line))

    missing, mismatch = [], []
    for name, arities in sorted(per_name.items()):
        if "::" in name:
            continue
        ok, decl_arity = satisfied(name, decls, manifest, pylib_ext, rust_fns)
        got = sorted({a for a, _ in arities})
        lines = sorted({ln for _, ln in arities})
        if not ok:
            if name in defined:
                missing.append((name, got, lines))
            continue
        if decl_arity is not None and set(got) != {decl_arity}:
            # codegen 的 param-suffixed 查找（get_or_declare_function:3138）：
            # 元数不符时按 `NAME_<argc>` 另绑/另发，实拍 t62/t63 走
            # py_threading_thread_new_2。该形态符号在清单或 C 里存在 ⇒ 消解。
            if all((f"{name}_{a}" in decls or f"{name}_{a}" in manifest
                    or f"{name}_{a}" in defined) for a in got):
                continue
            mismatch.append((name, decl_arity, got, lines))

    keys = []
    for name, got, _ in missing:
        keys.append(key_missing(name, got))
    for name, want, got, _ in mismatch:
        keys.append(key_mismatch(name, want, got))
    for name, a1, a2 in dup:
        keys.append(key_dup(name, a1, a2))
    keys = sorted(set(keys))

    print(f"核对面：gen.rs 字面量发射点 {len(sites)} 处（静态 arity 可数）"
          f" + UNKNOWN_ARITY {len(unknown)} + 名不可数 {len(interp)}；"
          f"满足链 S1 手写 {len(decls)} / S2 清单 {len(manifest)} /"
          f" S3 pylib extern {len(pylib_ext)} / S4 Rust no_mangle {len(rust_fns)}；"
          f"C 定义 {len(defined)} 颗。")

    if bless:
        BASELINE.parent.mkdir(parents=True, exist_ok=True)
        BASELINE.write_text("\n".join(keys) + ("\n" if keys else ""))
        print(f"--bless：{len(keys)} 条风险登记写入 {BASELINE.relative_to(ROOT)} ⇒ rc=0")
        return 0

    for name, got, lines in missing:
        print(f"[MISSING] {name} 发射 arity={got} 行={lines[:6]}"
              f" ⇒ 满足链全空，落 get_or_declare_function 按需 i64 声明")
    for name, want, got, lines in mismatch:
        print(f"[MISMATCH] {name} 声明 arity={want} vs 发射 arity={got} 行={lines[:6]}")
    for name, a1, a2 in dup:
        print(f"[HAND-DUP] {name} 手写声明 arity {a1} vs {a2}")

    if not BASELINE.exists():
        print(f"基线不存在（首跑先 --bless 登记当前已核实风险面）⇒ rc=1")
        return 1
    base = {l.strip() for l in BASELINE.read_text().splitlines() if l.strip()
            and not l.startswith("#")}
    cur = set(keys)
    new = sorted(cur - base)
    stale = sorted(base - cur)
    for k in new:
        print(f"[新增风险] {k}")
    for k in stale:
        print(f"[基线过期] {k}（已修或已变——跑 --bless 收面）")
    if new or stale:
        print(f"判定：在册 {len(base)} 条，新增 {len(new)} / 待收面 {len(stale)} ⇒ rc=1")
        return 1
    print(f"判定：双写风险与基线一致（在册 {len(base)} 条，全部为已核实登记面）⇒ rc=0")
    return 0


if __name__ == "__main__":
    sys.exit(main())
