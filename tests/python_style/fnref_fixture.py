# PY-A: 回归夹具 —— 被导入模块的 `def` 要能当**值**读出来
# 语料形：strategies/code/jq_wufu_local.py:41-45 + :111-112
#   morning_routine = _strategy.morning_routine
#   for routine in [morning_routine, …]: routine(context)
# 形参照语料保留（arity=1），但**不打**它：动态槽里的 str 没有类型标记，打出来
# 是堆地址（#117/#118，另案），那会让本用例的 expect 逐跑不同。
def morning(_ctx):
    print("FNREF morning")


def buy(_ctx):
    print("FNREF buy")
