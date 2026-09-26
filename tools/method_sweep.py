#!/usr/bin/env python3
"""tools/method_sweep.py — str/dict/list 方法可用性全面扫描（批次 528）。
用法：python3 tools/method_sweep.py
输出：缺 shim/实现/运行期错误的方法清单。
"""
import subprocess, os, sys, tempfile

ZETAC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                     "target", "release", "zetac")

str_methods = [
    "upper", "lower", "strip", "lstrip", "rstrip", "title", "capitalize",
    "swapcase", "isalpha", "isdigit", "isalnum", "islower", "isupper",
    "isspace", "istitle", "splitlines", "isascii", "isdecimal",
    "center(10)", "zfill(8)", "count('a')", "find('a')", "rfind('a')",
    "index('a')", "replace('a','b')", "startswith('a')", "endswith('a')",
    "split(',')", "rsplit(',',1)", "partition(',')", "rpartition(',')",
    "format()", "expandtabs()", "removeprefix('a')", "removesuffix('a')",
]
list_methods = [
    "append(1)", "extend([1])", "insert(0,1)", "pop()", "remove(1)",
    "clear()", "count(1)", "index(1)", "reverse()", "sort()", "copy()",
]
dict_methods = [
    "get('a')", "get('a',1)", "keys()", "values()", "items()",
    "pop('a')", "popitem()", "setdefault('a',1)", "update({'b':2})",
    "clear()", "copy()",
]


def test(src: str) -> str:
    with tempfile.NamedTemporaryFile(mode="w", suffix=".z", delete=False) as f:
        f.write(src)
        tmp = f.name
    try:
        r = subprocess.run([ZETAC, tmp, "-o", tmp + ".bin"],
                           capture_output=True, text=True, timeout=30)
        if "Undefined" in r.stderr:
            return "LINK_FAIL"
        if r.returncode != 0:
            return "COMPILE_ERR"
        p = subprocess.run([tmp + ".bin"], capture_output=True, text=True, timeout=10)
        if p.returncode != 0:
            return f"RUNTIME_ERR rc={p.returncode}"
        return "OK"
    except subprocess.TimeoutExpired:
        return "TIMEOUT"
    finally:
        for suffix in ("", ".bin", ".bin.o"):
            try:
                os.unlink(tmp + suffix)
            except OSError:
                pass


def main() -> int:
    print("=== str methods ===")
    for m in str_methods:
        r = test(f'"abc".{m}')
        if r != "OK":
            print(f"  {m}: {r}")
    print("=== list methods ===")
    for m in list_methods:
        src = f"v = [1, 2, 3]\nv.{m}\nprint(len(v))\n"
        r = test(src)
        if r != "OK":
            print(f"  {m}: {r}")
    print("=== dict methods ===")
    for m in dict_methods:
        src = f'd = {{"a": 1, "b": 2}}\nr = d.{m}\nprint(len(d))\n'
        r = test(src)
        if r != "OK":
            print(f"  {m}: {r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
