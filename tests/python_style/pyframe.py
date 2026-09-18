# PY-A test fixture: 一个「库」里的类，方法体里有局部变量 + 链式读取（见 t187）。
#   键类型必须写成 `lt(map, str)`（Zeta 的泛型拼写，不是 `<...>`）。


class Frame:
    def __init__(self, d: lt(map, str)):
        self.d = d

    def n_rows(self):
        names = list(self.d.keys())
        if len(names) == 0:
            return 0
        return len(self.d[names[0]])

    def n_columns(self):
        return len(self.d)
