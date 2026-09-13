# PY-A test fixture: a user Python module imported by t42_user_module.z
# (loaded from disk by `import pyfixture` — the same-directory search root)

def double(x):
    return x * 2


def helper():
    # bare call to the module's own function: exercises the per-module
    # rename map (must resolve to pyfixture__double, not a global `double`)
    return double(4) + 1
