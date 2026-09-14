---
updated: 2026-09-14T09:10:00.000Z
next: "finish work"
status: active
workflow: complex
---
# Work: 修关键字实参绑定 + 字符串下标/切片（+ 参数类型推断）

## Current Plan

### Stage 2: Implement
- [x] Read relevant code and existing patterns
- [x] Apply the smallest scoped change
- [x] Add/update tests when practical
- [x] Run relevant checks
- [x] Break multi-file work into safe slices
- [x] Consider spec-reviewer after delegated slices

### Stage 3: Validate
- [x] Define scenarios to run
- [x] Execute scenarios and document blockers
- [x] Record pass/fail evidence and open issues
- [x] Build a scenario matrix with expected evidence
- [x] Cover relevant success, error, and boundary paths

## Verification

| 套件 | 结果 |
|---|---|
| python_style | **53 passed, 0 failed**（新增 t51 kwargs、t52 字符串下标/切片、t53 参数推断） |
| 官方编译 | **194/194** |
| 官方运行退出码 vs 已提交基线 | **0 差异**（过程中出现过 test_while_loop 等 3 处回归，已定位并修复） |
| REasyQuant 语料 | 38/38 解析 |
| 真实第三方库 | stringcase 三个入口全对（snakecase/camelcase/pascalcase） |

## Evidence / Decisions

1. **关键字实参按名绑定**（commit 5084ddc5）：实参名原在 parser 被丢弃 → `f(b=2, a=1)` 静默算 201。
   parser 改为发 `__kwarg__` 标记；调用点用形参名重排（位置参数填前段、关键字按名落位）；
   未匹配名发 warning；registry/方法调用解包标记保持原行为。
2. **字符串下标/切片 + 参数类型推断**（commit 594ea0e4）：
   - `str_get`/`str_slice` + gen 按接收者类型分发；省略 end 哨兵改 `i64::MIN`
     （`s[:-1]` 折叠成 `Lit(-1)`，与哨兵无法区分 → 曾返回整串）
   - 参数类型推断：调用点证据（字面量 / 已知 str·f64 的值）→ 未标注参数定 str/f64；
     被调名三种解析；证据收集覆盖 return/let/assign/binaryop；6 轮传播；
     **只回写升级下标**（全量回写把数组参数写成 "array(...)"，造成 3 处官方回归）
3. **顺带修复**：
   - `[dynamic]T{}` 用无 header 的 `array_new` → `arr.push` 写到块外、`len()` 恒 0
     （test_while_loop 一直依赖旧的「意外行为」通过）；改用 `zeta_dynarray_new` + typed push 回写
   - slice/len 的 header 读取加合理性校验（原先非 Vec 句柄 → 巨额分配 → OOM 崩溃）

## Final Summary

P1 两项（kwargs 按名绑定、字符串下标/切片）完成，并补上使真实库可用的参数类型推断。
**首次跑通真实第三方库**（zorb 从 GitHub 装的 python-stringcase）。三套回归零差异。

## Open / Deferred

- [ ] `json.loads`（结果类型无法静态建模，仍故意不入表）
- [ ] 容器元素类型推断（`[i64]` 元素、dict value 类型）
- [ ] `re` flags/finditer/subn/Pattern 方法面；collections/itertools/random/pathlib 库覆盖
- [ ] `Thread(args=...)`、`with open(...)`
- [ ] `src/package/` 死代码处置、site-packages 直连
