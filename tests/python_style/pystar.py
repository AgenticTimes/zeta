# PY-A fixture: star import binds public names, skips underscore ones.
def alpha():
    return 11


def beta():
    return 31


def _hidden():
    return 99
