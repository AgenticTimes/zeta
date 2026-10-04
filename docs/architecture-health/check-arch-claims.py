#!/usr/bin/env python3
"""Architecture claim checker for docs/diagrams/zeta-*.drawio.

Each row encodes one falsifiable assertion made in a diagram node label.
Run from the repository root:  python3 docs/architecture-health/check-arch-claims.py
Exit code = number of FAILing claims (0 means every diagram assertion still holds).
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".")

FILES = {
    "gen.rs": "src/middle/mir/gen.rs",
    "resolver.rs": "src/middle/resolver/resolver.rs",
    "codegen.rs": "src/backend/codegen/codegen.rs",
    "ast.rs": "src/frontend/ast.rs",
    "mir.rs": "src/middle/mir/mir.rs",
    "types/mod.rs": "src/middle/types/mod.rs",
}


def lines(rel):
    p = ROOT / rel
    return 0 if not p.exists() else sum(1 for _ in p.open("rb"))


def enum_body(rel, enum_name):
    """Return (start_line, end_line) 1-based, inclusive of the braces."""
    src = (ROOT / rel).read_text(encoding="utf-8", errors="replace").split("\n")
    start = next(i for i, l in enumerate(src) if re.search(r"\benum\s+%s\b" % enum_name, l))
    depth = 0
    for i in range(start, len(src)):
        depth += src[i].count("{") - src[i].count("}")
        if depth == 0 and i > start and "{" in "".join(src[start : i + 1]):
            return start + 1, i + 1
    return start + 1, len(src)


def enum_variants(rel, enum_name):
    a, b = enum_body(rel, enum_name)
    body = (ROOT / rel).read_text(encoding="utf-8", errors="replace").split("\n")[a:b]
    # Rustfmt puts one variant per line at 4-space indent; ',' terminates bare variants.
    return [re.match(r"^    ([A-Z][A-Za-z0-9_]*)", l).group(1)
            for l in body if re.match(r"^    [A-Z][A-Za-z0-9_]*[, <({]", l)]


def struct_fields(rel, struct_name):
    src = (ROOT / rel).read_text(encoding="utf-8", errors="replace").split("\n")
    start = next((i for i, l in enumerate(src) if re.search(r"\bstruct\s+%s\b" % struct_name, l)), None)
    if start is None:
        return None
    depth = 0
    out = []
    for i in range(start, len(src)):
        depth += src[i].count("{") - src[i].count("}")
        m = re.match(r"^(?:pub )?([a-z_][a-z0-9_]*)\s*:", src[i].strip())
        if m and depth >= 1:
            out.append(m.group(1))
        if depth == 0 and i > start:
            break
    return out


C_DEF = re.compile(r"^[A-Za-z_][A-Za-z0-9_ \t\*]*?\b([A-Za-z_][A-Za-z0-9_]*)\s*\(")
C_KW = {"if", "for", "while", "switch", "return", "sizeof"}


def c_definitions(rel):
    p = ROOT / rel
    if not p.exists():
        return 0
    src = re.sub(r"/\*.*?\*/", "", p.read_text(encoding="utf-8", errors="replace"), flags=re.S)
    src = re.sub(r"//[^\n]*", "", src)
    ls = src.split("\n")
    n = 0
    for i, l in enumerate(ls):
        m = C_DEF.match(l)
        if not m or m.group(1) in C_KW:
            continue
        for j in range(i, min(i + 6, len(ls))):
            if "{" in ls[j]:
                n += 1
                break
            if ";" in ls[j]:
                break
    return n


def z_count(rel_glob):
    out = subprocess.run(
        ["bash", "-c", "find '%s' -name '*.z' | wc -l" % (ROOT / rel_glob)],
        capture_output=True, text=True,
    )
    return int(out.stdout.strip() or 0)


def symbol_exists(pattern, globs=("*.rs", "*.c")):
    args = ["grep", "-rlE"]
    for g in globs:
        args += ["--include=" + g]
    args += [pattern, str(ROOT / "src"), str(ROOT / "runtime")]
    r = subprocess.run(args, capture_output=True, text=True)
    return bool(r.stdout.strip())


results = []


def check(kind, claim, actual, ok, source):
    results.append((ok, kind, claim, actual, source))


# --- numeric claims (core-classes diagram) -----------------------------------
lc = lines(FILES["gen.rs"])
check("line-count", "gen.rs 13,785 lines", lc, abs(lc - 13785) / 13785 <= 0.01, FILES["gen.rs"])
lc = lines(FILES["resolver.rs"])
check("line-count", "resolver.rs 4.7k lines", lc, abs(lc - 4700) / 4700 <= 0.05, FILES["resolver.rs"])
lc = lines(FILES["codegen.rs"])
check("line-count", "codegen.rs 7.6k lines", lc, abs(lc - 7600) / 7600 <= 0.05, FILES["codegen.rs"])

v = len(enum_variants(FILES["ast.rs"], "AstNode"))
check("variants", "AstNode 40+ variants", v, v >= 40, FILES["ast.rs"])
v = len(enum_variants(FILES["types/mod.rs"], "Type"))
check("variants", "Type 50+ variants", v, v >= 50, FILES["types/mod.rs"])

c = c_definitions("runtime/tokio_runtime_stub.c") + c_definitions("runtime/py_additions.c")
check("c-functions", "C runtime 462 functions", c, abs(c - 462) / 462 <= 0.05,
      "runtime/tokio_runtime_stub.c + runtime/py_additions.c")

z = z_count("tests/unit-tests")
check("corpus", "official 198 .z files", z, z == 198, "tests/unit-tests/**/*.z")

# --- attribution claims -------------------------------------------------------
rf = struct_fields(FILES["resolver.rs"], "Resolver") or []
for want in ("fns", "classes", "fn_rets", "func_ret_types"):
    check("field", "Resolver owns field `%s`" % want,
          "present" if want in rf else "absent (%d fields: %s...)" % (len(rf), ", ".join(rf[:6])),
          want in rf, FILES["resolver.rs"])

mf = struct_fields(FILES["mir.rs"], "Mir") or []
for want in ("stmts", "exprs", "param_indices", "globals"):
    check("field", "Mir owns field `%s`" % want,
          "present" if want in mf else "absent", want in mf, FILES["mir.rs"])

gf = struct_fields(FILES["gen.rs"], "MirGen") or []
for want in ("type_map", "exprs", "func_ret_types", "type_decls"):
    check("field", "MirGen owns field `%s`" % want,
          "present" if want in gf else "absent", want in gf, FILES["gen.rs"])

# --- symbol claims (pipeline-flow + call-chain) ------------------------------
for pat in [
    r"\bindent_preprocess\b", r"\bparse_zeta\b", r"\bparse_top_level_entry\b",
    r"\bparse_block_body\b", r"\bparse_full_expr\b", r"\bResolver::register\b|\bfn register\b",
    r"\blower_to_mir\b", r"\bstr_method_symbol\b", r"\bgen_mirs\b", r"\bfn get_function\b",
    r"\bunannotated_return_ty\b", r"\bhost_str_to_string\b", r"\bhost_result_is_ok\b",
    r"\bzeta_call1\b", r"\bW1002\b", r"\bZETA_PARSE_TRACE\b", r"\bPyDynamic\b",
]:
    check("symbol", "symbol %s" % pat, "found" if symbol_exists(pat) else "NOT FOUND",
          symbol_exists(pat), "src/ runtime/")

# ============================================================================
fails = [r for r in results if not r[0]]
width = max(len(r[2]) for r in results)
print("claim".ljust(width), "| verdict | actual")
print("-" * (width + 34))
for ok, kind, claim, actual, source in results:
    print(claim.ljust(width), "| %-7s | %s" % ("PASS" if ok else "FAIL", actual))
print("-" * (width + 34))
print("%d claims checked, %d PASS, %d FAIL" % (len(results), len(results) - len(fails), len(fails)))
sys.exit(len(fails))
