# Proposal: Python 模块系统与语义 P1 收尾

## Why

roadmap「下一步（按序）」第 1 项 = **P1 模块系统与语义**，其子项多数已完成
（模块级语句执行、顶层常量导出、目录包、`with open(...)`），只剩三处 fail-loud /
降级缺口，全部影响真实 Python 代码可用性：

1. **相对导入**（`from . import x` / `from .mod import y` / `from ..pkg import z`）
   与 `from X import *` 绑名——当前相对导入无法解析，星号导入未绑名。
2. **`Thread(target, args=(...))`**——带参目标不支持（`FuncAddr` 零参），
   而 `threading.Thread(work, args=(n,))` 是并发代码的常见写法。
3. **自定义 `__enter__`/`__exit__`**——非 shim 类型仍走 identity 兜底（只发 warning），
   `with` 对用户自定义上下文管理器不生效。

这三项都是「本该工作却静默降级/失败」的收尾，属于 PY-A「基本能编译 Python」目标的 P1 缺口。

## What Changes

### 1. 相对导入 + 星号导入
- parser 保留前导 `.`（level）信息，`from . import x` / `from .mod import y` / `from ..pkg import z`
- resolver 依据**当前模块的包上下文**把相对名解析为绝对模块路径
- `from X import *`：把模块的**公开顶层定义**绑入当前命名空间

### 2. `Thread(target, args=(...))`
- `threading.Thread` 支持 `args=` 元组：把目标函数 + 实参打包，线程启动时以该实参调用
- 复用现有 `FuncAddr` 机制并扩展为可携带绑定实参（或新增 args-tuple 路径）

### 3. 自定义 `__enter__`/`__exit__`
- `with X:` 在 shim 标签未命中时，尝试调用用户定义的 `__enter__`/`__exit__` 方法
- 仅当两者都不存在时才回退 identity（保留 warning）

## Impact

- Affected specs: Python 兼容层（PY-A）模块系统
- Affected code:
  - `src/frontend/parser/`（import/from 解析、星号导入）
  - `src/middle/resolver/`（模块/成员别名表、相对名解析、公开定义收集）
  - 模块搜索/加载（`pylib::find_module` 等）
  - `src/middle/mir/gen.rs`（`FuncAddr`、Thread 调用、`with` 路由）
  - `runtime/*.c`（如 Thread 需新增带参启动原语）
- 回归红线：python_style 61/61 不得下降；官方 194/194 运行退出码零差异；语料 38/38

## Out of Scope

- `importlib` / 动态 `sys.path` 操作
- 真多进程 multiprocessing、asyncio 事件循环
- WASM 后端、自举
