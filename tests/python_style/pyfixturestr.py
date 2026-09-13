# PY-A test fixture: untyped parameters + string ops, imported by t53.
# Python functions carry no annotations; the parameter type is inferred from
# call-site evidence, which is what lets `s[0]` / `s.lower()` dispatch as str.


def lower(s):
    return str(s).lower()


def first(s):
    return s[0]


def tail(s):
    return s[1:]


def snake_like(s):
    return lower(first(s)) + "_" + lower(tail(s))
