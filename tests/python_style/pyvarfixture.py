# PY-A test fixture: a module-level VARIABLE imported by name (t273).
# `from pyvarfixture import D` used to bind an uninitialized slot (read 0).

D = {"a": 1}


def reg(k: str, v: i64) -> None:
    D[k] = v


def size() -> i64:
    return len(D)
