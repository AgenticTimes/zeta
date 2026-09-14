# PY-A fixture: relative imports inside a package.
#   `from .sub import f` (submodule member) and `from . import extra`
#   (submodule object).
from .sub import double
from . import extra


def compute(n):
    return double(n) + extra.offset()
