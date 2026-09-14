# Tasks: feat-py-module-system

## T1: 相对导入（parser）
- [x] `parse_relative_module`（前导点 + 可选点分名，保留点前缀）—— `src/frontend/parser/stmt.rs`
- [x] `parse_python_from_import` 改用 `parse_relative_module`
- [x] 验证：t64_relative_import GREEN

## T2: 相对导入（resolver）
- [x] `py_module_pkg`（模块 → `__package__`：`__init__` 锚自身、子模块锚父包）
- [x] `py_current_module` 上下文（`load_user_python_module` 内 save/restore）
- [x] `resolve_py_module_spec`：按 level 解析 `.`/`..`；越界/无上下文 fail-loud
- [x] `from . import submod` 绑定子模块（存在时）而非包成员
- [x] 验证：t64_relative_import GREEN

## T3: star import
- [x] parser 发 `zeta_py_star` 标记（替代原静默 star）
- [x] resolver 枚举目标模块公开顶层名并绑定（下划线排除）
- [x] gen：`zeta_py_star` 为编译期 no-op（不产生 runtime 调用），并跑模块 init
- [x] 验证：t65_star_import GREEN；`_hidden` 未绑定 → 链接失败（fail-loud）

## T4: `from X import y` 跑模块 init
- [x] gen 的 init 发射条件加入 `zeta_py_from`（原仅 `zeta_py_import`）
- [x] 实测：pyfixturemod 的 `total()` 由 1 → 6（`mod-init` 先打印）

## T5: 自定义 `__enter__`/`__exit__`
- [x] gen 的 `with` 路由：无 Py* 标签时查接收者静态类型，分发到该类型 `__enter__`/`__exit__`
- [x] 仅当两者都不存在才回退 identity + warning
- [x] 验证：t62_with_user_ctx（RED 0/0 → GREEN 1/6）；t32/t41 无回归

## T6: `Thread(target, args=(...))`
- [x] 单元素沿用 `py_threading_thread_new_2`（既有，t62_thread_args）
- [x] 2+ 元素：MIR 合成解包适配器 `fn __tp: target(__tp[0], ...)` + 打包 tuple 句柄
- [x] 覆盖位置 target / `target=` / 3 参 / `threading.Thread` 限定形式
- [x] 验证：t63_thread_args_multi GREEN（原为静默垃圾值）

## T7: 三套回归 + 提交
- [x] python_style 66/66
- [x] 官方 194/194（运行退出码零差异）
- [x] 语料 38/38
- [x] 原子提交：`0c3a2ed6`、`c49e4ff8`、`25acdb0d`（+ 先前批 `bb5d0ca1`/`17994b15`）
