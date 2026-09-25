# PY-A: 回归夹具 —— 模块读自己的 `__file__`
# 语料形：backend/datasrc/market_data_sources.py:146
#   `_PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent`
import os

print("FIXTURE", os.path.basename(__file__), os.path.basename(os.path.dirname(__file__)))
