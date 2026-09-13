# PY-A test fixture: a directory package (__init__.py) imported by
# t44_package_import.z. Exercises package loading + module-internal
# cross-calls (per-module rename map).

def marker():
    return 7


def triple(x):
    return x * 3


def internal():
    # bare call to the package's own function -> pyfixturepkg__triple
    return marker() + triple(5)
