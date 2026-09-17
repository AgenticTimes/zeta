# PY-A test fixture: 调用点实参个数与声明形参个数不一致（默认参数），
# 用来回归「模块限定名不该被 arity 后缀化」+「跨模块默认参数要填上」（含构造器）——见 t177。


def add3(a, b, c=10):
    return a + b + c


class Pair:
    def __init__(self, x, y=2):
        self.x = x
        self.y = y

    def total(self):
        return self.x + self.y
