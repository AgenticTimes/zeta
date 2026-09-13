# PY-A test fixture: module body side effect + module-level constant,
# imported by t46_module_semantics.z
print("mod-init")
LIMIT = 5


def get():
    return LIMIT


def total():
    return get() + 1
