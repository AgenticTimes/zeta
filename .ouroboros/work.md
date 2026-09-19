---
updated: "2026-09-17T20:24:52.058Z"
next: "done"
status: "done"
workflow: complex
---
# Work: 占位符符号 py_asdict_unexpanded 改为响亮失败

## Current Plan

Work through generated stages in order by default. Do not delete required stage steps; mark non-applicable steps with evidence and rationale.

### Stage 1: Implement — C 侧实现 py_asdict_unexpanded（打印原因 + abort），重建对应 runtime 对象；顺带核对 range/getattr/dict/object 的来源

- [x] Read relevant code and existing patterns
- [x] Apply the smallest scoped change
- [x] Add/update tests when practical
- [x] Run relevant checks
- [x] Break multi-file work into safe slices
- [x] Consider spec-reviewer after delegated slices

### Stage 2: Validate — 实测（未定义符号数下降 / 走到该路径时信息可读）+ 三套基线（官方 194/194、python_style 全绿、语料逐文件对拉无回退）+ roadmap + commit + push

- [x] Define scenarios to run
- [x] Execute scenarios or document blockers
- [x] Record pass/fail evidence and open issues
- [x] Build a scenario matrix with expected evidence
- [x] Cover relevant success, error, and boundary paths

## Open Questions

## Verification

- 改写实测：getattr(date(2020,5,6),"year") → Compiled 但输出 18388（应为 2020）⇒ 静默错值，回滚
- 回滚后复核：同一示例恢复为 Linking failed（回到既有的响亮失败）
- python_style: 159 passed, 0 failed（回滚后复跑）
- 官方 tests/unit-tests: 194/194（本轮净改动为 0，基线未动）
## Evidence / Decisions

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- 试了 `getattr(obj, "字段")` 的编译期改写（字符串字面量 → FieldAccess，走既有 struct/handle 分派）。想法本身是对的，但**实测会变成静默错值**：链接通过，而 `getattr(date(2020,5,6), "year")` 返回 **18388**（应为 2020）
- 原因：`d` 是**未标注形参** ⇒ 接收者类型未知 ⇒ FieldAccess 退化成结构体字段读/map 取。⇒ 该改写只有在接收者类型**静态已知**时才安全
- 按本项目红线处理：**无证据 + 引入静默错值 ⇒ 立即回滚**（本会话第五次回滚），并把「正确写法」记进 roadmap（先查 type_map，已知 handle/struct 才改写，否则继续走未实现诊断）
- 边界确认：`getattr(a, <计算出的名字>)` 仍走编译期指名诊断 ✓；上一批 `object()` 的修复不受影响 ✓
- 本轮净改动 0（只有 roadmap 文档）；度量保持：python_style 159/159、官方 194/194、语料解析 37/38、未定义符号 83
- 结论再一次指向同一个洞：`getattr` 安全改写 / `*_N` 消除 / `xs[0]` 返 0 / `in` 恒 0 —— 共同依赖都是**参数/接收者的静态类型**，即参数类型推断
- commit 99a66b20（roadmap），已 push 到 agentic/bootstrap
## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- 修掉内建幽灵符号里的一个真实项：object() —— Python 的裸 object 正是运行时已有的不透明平台句柄，改发 zeta_platform_obj(0,0,0,0)；此前退化成对裸名 object 的自由调用 ⇒ 链接期未定义符号（语料 3 处）
- 对两个**无法降级**的内建（getattr、非循环用法的 range）按红线**不做语义假的桩**，改为在编译期打一条指名诊断（"builtin `X` is not implemented in this form (it would link against an undefined symbol named `X`)"）；符号照旧发射、行为不变，但用户能立刻定位
- 度量：语料未定义符号去重 **84 → 83**（object 归零；getattr 2 / range 3 仍在但现在有指名提示）；解析 37/38 不回退
- 红线：python_style **158 → 159**（新增 t159_object_builtin.z，pre-fix 用 git stash 验证 FAIL、post-fix PASS）；官方 **194/194**
- 复用批次二十四结论：**不用同名 C 桩兜底**（range/getattr/object 都是普通标识符，会与用户自定义同名函数撞符号）
- commit 434eddf6（修复）+ 1721f74a（roadmap），已 push 到 agentic/bootstrap
## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **试了 \`getattr(obj, "field")\` 的编译期改写，实测会变成静默错值，已回滚**。
- 实测：改写后链接通过，但 \`getattr(date(2020,5,6), "year")\` 返回 **18388**（应为 2020）。
  原因 = \`d\` 是**未标注形参** ⇒ 接收者类型未知 ⇒ FieldAccess 退化成结构体字段读/map 取。
- 结论：该改写**只在接收者类型静态已知时**安全；换成「先查 type_map 是否已知 handle/struct，
  不认识就继续走未实现诊断」可行，但属另一批。按规矩**无证据 + 引入静默错值 ⇒ 回滚**。
- 这又一次指向同一个洞：\`getattr\` 安全改写、\`*_N\` 消除、\`xs[0]\` 返 0、\`in\` 恒 0 —— 共同依赖是
  **参数/接收者的静态类型**（参数类型推断）。
- 边界确认：\`getattr(a, <计算名字>)\` 仍走编译期指名诊断 ✓；\`object()\` 修复不受影响 ✓。
- 度量（本轮净改动 0）：python_style **159/159**、官方 **194/194**、语料解析 37/38、未定义符号 83。
- commit \`99a66b20\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- ① registry 占位符巡检：**已清零**。用批次二十三的办法（registry 符号名 ∩ nm 运行时库）全量跑：带符号条目 292 个、两个运行时对象已定义 587 个、**不在库里的条目 = 0** ⇒ py_asdict_unexpanded 修完后没有别的幽灵占位符
- ② 内建降级成幽灵符号（新发现）：b = range(5) / range(1,10,2) → 未定义 _range；b = getattr(a,"x") → _getattr；b = object() → _object（dict() 正常）。性质=未实现的内建**不发编译期错误，而发同名自由调用** ⇒ 链接期出幽灵符号（响亮但零信息）
- 已排除一条捷径并记录理由：把 range/getattr/object 做成同名 C 桩 —— 三个都是极普通标识符，用户代码自定义同名函数会撞符号（DUPLICATE_SYM 老问题），且无法加前缀绕过（发射的就是裸名）⇒ 必须走编译器侧降级/报错
- 本轮无源码改动（巡检 + 分析），三套基线保持：python_style 158/158、官方 194/194、语料解析 37/38、未定义符号去重 84
- 过程踩了两个 shell/git 坑并修好：bash 双引号串里的反引号被命令替换吞掉（roadmap 两处字面量被吃，已追加修正提交）；「已 push 后再 amend」导致 push 被拒，用 git reset --soft agentic/bootstrap 后补一个修正提交解决
- commit f27daa41（巡检+分析）+ 55fd9c6f（修正），已 push 到 agentic/bootstrap
## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **试了 \`getattr(obj, "field")\` 的编译期改写，实测会变成静默错值，已回滚**。
- 实测：改写后链接通过，但 \`getattr(date(2020,5,6), "year")\` 返回 **18388**（应为 2020）。
  原因 = \`d\` 是**未标注形参** ⇒ 接收者类型未知 ⇒ FieldAccess 退化成结构体字段读/map 取。
- 结论：该改写**只在接收者类型静态已知时**安全；换成「先查 type_map 是否已知 handle/struct，
  不认识就继续走未实现诊断」可行，但属另一批。按规矩**无证据 + 引入静默错值 ⇒ 回滚**。
- 这又一次指向同一个洞：\`getattr\` 安全改写、\`*_N\` 消除、\`xs[0]\` 返 0、\`in\` 恒 0 —— 共同依赖是
  **参数/接收者的静态类型**（参数类型推断）。
- 边界确认：\`getattr(a, <计算名字>)\` 仍走编译期指名诊断 ✓；\`object()\` 修复不受影响 ✓。
- 度量（本轮净改动 0）：python_style **159/159**、官方 **194/194**、语料解析 37/38、未定义符号 83。
- commit \`99a66b20\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **object() 修好**：Python 的裸 object 正是运行时已有的不透明平台句柄 → 改发 \`zeta_platform_obj(0,0,0,0)\`（此前是对裸名 \`object\` 的自由调用 ⇒ 链接期未定义）。未定义符号去重 **84 → 83**（object 归零）。
- **getattr / 非循环 range**：无法降级（动态属性查找 / 无运行时表示），按红线**不做语义假的桩**；改为**编译期指名诊断**，让用户立刻看到是谁干的（符号照旧发射，行为不变）。
- 新增 t159_object_builtin.z —— pre-fix FAIL（Linking failed）、post-fix PASS；python_style **158 → 159**；官方 **194/194**；解析 37/38 不回退。
- 复用了批次二十四的结论：**不用同名 C 桩兜底**（会与用户自定义同名函数撞符号）。
- commit \`434eddf6\`（修复）+ \`1721f74a\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **试了 \`getattr(obj, "field")\` 的编译期改写，实测会变成静默错值，已回滚**。
- 实测：改写后链接通过，但 \`getattr(date(2020,5,6), "year")\` 返回 **18388**（应为 2020）。
  原因 = \`d\` 是**未标注形参** ⇒ 接收者类型未知 ⇒ FieldAccess 退化成结构体字段读/map 取。
- 结论：该改写**只在接收者类型静态已知时**安全；换成「先查 type_map 是否已知 handle/struct，
  不认识就继续走未实现诊断」可行，但属另一批。按规矩**无证据 + 引入静默错值 ⇒ 回滚**。
- 这又一次指向同一个洞：\`getattr\` 安全改写、\`*_N\` 消除、\`xs[0]\` 返 0、\`in\` 恒 0 —— 共同依赖是
  **参数/接收者的静态类型**（参数类型推断）。
- 边界确认：\`getattr(a, <计算名字>)\` 仍走编译期指名诊断 ✓；\`object()\` 修复不受影响 ✓。
- 度量（本轮净改动 0）：python_style **159/159**、官方 **194/194**、语料解析 37/38、未定义符号 83。
- commit \`99a66b20\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- 已修：占位符符号 py_asdict_unexpanded —— registry 把 dataclasses.asdict 指向这个**到处都不存在**的符号，于是编译期重写不适用时，整个程序在**链接期**失败且报错无意义（语料 3 处）
- 修法：runtime/tokio_runtime_stub.c 实现该符号 —— 打印可读原因（"dataclasses.asdict() on a value that is not a statically known dataclass is not supported"）+ abort()。于是同一文件其他代码可正常链接；只有真走到未展开路径才失败；**绝不返回假值**（响亮失败 ≠ fail-open）。同步重建 tracked 的 tokio_runtime.o
- 度量：语料未定义符号去重 **85 → 84**，py_asdict_unexpanded 引用归零；解析 37/38 不回退；链接通过仍 1/38（其余缺 pandas/平台符号）
- 红线：python_style **158/158**；官方 **194/194**（两者在改动后均复测）
- 方法沉淀（写进 roadmap）：检查「registry 指向不存在符号」的可复用办法 = registry.txt 里的符号名去 nm -g --defined-only 运行时库里查，差集即占位符，一律给「会响的实现」
- 过程教训：heredoc 里跟 `&& python3 - <<'PY'` 会让 python 代码被当成 commit message 吞掉（本轮踩了一次，已 amend 修正为规范消息并单独提交 roadmap）
- commit 91dfb3bd（修复，amend 后）+ 73e1f51f（roadmap），已 push 到 agentic/bootstrap
## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **试了 \`getattr(obj, "field")\` 的编译期改写，实测会变成静默错值，已回滚**。
- 实测：改写后链接通过，但 \`getattr(date(2020,5,6), "year")\` 返回 **18388**（应为 2020）。
  原因 = \`d\` 是**未标注形参** ⇒ 接收者类型未知 ⇒ FieldAccess 退化成结构体字段读/map 取。
- 结论：该改写**只在接收者类型静态已知时**安全；换成「先查 type_map 是否已知 handle/struct，
  不认识就继续走未实现诊断」可行，但属另一批。按规矩**无证据 + 引入静默错值 ⇒ 回滚**。
- 这又一次指向同一个洞：\`getattr\` 安全改写、\`*_N\` 消除、\`xs[0]\` 返 0、\`in\` 恒 0 —— 共同依赖是
  **参数/接收者的静态类型**（参数类型推断）。
- 边界确认：\`getattr(a, <计算名字>)\` 仍走编译期指名诊断 ✓；\`object()\` 修复不受影响 ✓。
- 度量（本轮净改动 0）：python_style **159/159**、官方 **194/194**、语料解析 37/38、未定义符号 83。
- commit \`99a66b20\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **object() 修好**：Python 的裸 object 正是运行时已有的不透明平台句柄 → 改发 \`zeta_platform_obj(0,0,0,0)\`（此前是对裸名 \`object\` 的自由调用 ⇒ 链接期未定义）。未定义符号去重 **84 → 83**（object 归零）。
- **getattr / 非循环 range**：无法降级（动态属性查找 / 无运行时表示），按红线**不做语义假的桩**；改为**编译期指名诊断**，让用户立刻看到是谁干的（符号照旧发射，行为不变）。
- 新增 t159_object_builtin.z —— pre-fix FAIL（Linking failed）、post-fix PASS；python_style **158 → 159**；官方 **194/194**；解析 37/38 不回退。
- 复用了批次二十四的结论：**不用同名 C 桩兜底**（会与用户自定义同名函数撞符号）。
- commit \`434eddf6\`（修复）+ \`1721f74a\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **试了 \`getattr(obj, "field")\` 的编译期改写，实测会变成静默错值，已回滚**。
- 实测：改写后链接通过，但 \`getattr(date(2020,5,6), "year")\` 返回 **18388**（应为 2020）。
  原因 = \`d\` 是**未标注形参** ⇒ 接收者类型未知 ⇒ FieldAccess 退化成结构体字段读/map 取。
- 结论：该改写**只在接收者类型静态已知时**安全；换成「先查 type_map 是否已知 handle/struct，
  不认识就继续走未实现诊断」可行，但属另一批。按规矩**无证据 + 引入静默错值 ⇒ 回滚**。
- 这又一次指向同一个洞：\`getattr\` 安全改写、\`*_N\` 消除、\`xs[0]\` 返 0、\`in\` 恒 0 —— 共同依赖是
  **参数/接收者的静态类型**（参数类型推断）。
- 边界确认：\`getattr(a, <计算名字>)\` 仍走编译期指名诊断 ✓；\`object()\` 修复不受影响 ✓。
- 度量（本轮净改动 0）：python_style **159/159**、官方 **194/194**、语料解析 37/38、未定义符号 83。
- commit \`99a66b20\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **① registry 占位符巡检：已清零**（用批次二十三记下的办法全量跑）：registry 带符号条目 **292 个**、两个运行时对象已定义 **587 个**、**不在库里的条目 = 0** ⇒ \`py_asdict_unexpanded\` 修完后没有别的幽灵占位符。
- **② 内建降级成幽灵符号**：\`b = range(5)\`/\`range(1,10,2)\` → \`_range\`；\`b = getattr(a,"x")\` → \`_getattr\`；\`b = object()\` → \`_object\`（\`dict()\` 正常）。性质=**未实现的内建不发编译期错误、而发同名自由调用** ⇒ 链接期幽灵符号。
- **已排除的捷径**：把 \`range\`/\`getattr\`/\`object\` 做成同名 C 桩 —— 三个都是极普通标识符，用户自定义同名函数会**撞符号**（DUPLICATE_SYM 老问题），且无法加前缀绕过（发射的就是裸名）。⇒ 必须走**编译器侧**降级/报错。
- 度量：本轮无源码改动（巡检 + 分析），三套基线保持：python_style **158/158**、官方 **194/194**、语料解析 37/38、未定义符号 84。
- **过程教训**：\`bash -c\` 的双引号字符串里写反引号会被**命令替换**吞掉（本轮 roadmap 有两处字面量被吃掉，已修正并提交）；另外「已 push 后再 amend」会导致 push 被拒（要 reset --soft 到远端再补一个修正提交）。
- commit \`f27daa41\` + \`55fd9c6f\`，已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **试了 \`getattr(obj, "field")\` 的编译期改写，实测会变成静默错值，已回滚**。
- 实测：改写后链接通过，但 \`getattr(date(2020,5,6), "year")\` 返回 **18388**（应为 2020）。
  原因 = \`d\` 是**未标注形参** ⇒ 接收者类型未知 ⇒ FieldAccess 退化成结构体字段读/map 取。
- 结论：该改写**只在接收者类型静态已知时**安全；换成「先查 type_map 是否已知 handle/struct，
  不认识就继续走未实现诊断」可行，但属另一批。按规矩**无证据 + 引入静默错值 ⇒ 回滚**。
- 这又一次指向同一个洞：\`getattr\` 安全改写、\`*_N\` 消除、\`xs[0]\` 返 0、\`in\` 恒 0 —— 共同依赖是
  **参数/接收者的静态类型**（参数类型推断）。
- 边界确认：\`getattr(a, <计算名字>)\` 仍走编译期指名诊断 ✓；\`object()\` 修复不受影响 ✓。
- 度量（本轮净改动 0）：python_style **159/159**、官方 **194/194**、语料解析 37/38、未定义符号 83。
- commit \`99a66b20\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **object() 修好**：Python 的裸 object 正是运行时已有的不透明平台句柄 → 改发 \`zeta_platform_obj(0,0,0,0)\`（此前是对裸名 \`object\` 的自由调用 ⇒ 链接期未定义）。未定义符号去重 **84 → 83**（object 归零）。
- **getattr / 非循环 range**：无法降级（动态属性查找 / 无运行时表示），按红线**不做语义假的桩**；改为**编译期指名诊断**，让用户立刻看到是谁干的（符号照旧发射，行为不变）。
- 新增 t159_object_builtin.z —— pre-fix FAIL（Linking failed）、post-fix PASS；python_style **158 → 159**；官方 **194/194**；解析 37/38 不回退。
- 复用了批次二十四的结论：**不用同名 C 桩兜底**（会与用户自定义同名函数撞符号）。
- commit \`434eddf6\`（修复）+ \`1721f74a\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

- **试了 \`getattr(obj, "field")\` 的编译期改写，实测会变成静默错值，已回滚**。
- 实测：改写后链接通过，但 \`getattr(date(2020,5,6), "year")\` 返回 **18388**（应为 2020）。
  原因 = \`d\` 是**未标注形参** ⇒ 接收者类型未知 ⇒ FieldAccess 退化成结构体字段读/map 取。
- 结论：该改写**只在接收者类型静态已知时**安全；换成「先查 type_map 是否已知 handle/struct，
  不认识就继续走未实现诊断」可行，但属另一批。按规矩**无证据 + 引入静默错值 ⇒ 回滚**。
- 这又一次指向同一个洞：\`getattr\` 安全改写、\`*_N\` 消除、\`xs[0]\` 返 0、\`in\` 恒 0 —— 共同依赖是
  **参数/接收者的静态类型**（参数类型推断）。
- 边界确认：\`getattr(a, <计算名字>)\` 仍走编译期指名诊断 ✓；\`object()\` 修复不受影响 ✓。
- 度量（本轮净改动 0）：python_style **159/159**、官方 **194/194**、语料解析 37/38、未定义符号 83。
- commit \`99a66b20\`（roadmap），已 push。

## Final Summary

- **修掉本会话反复撞到的那条主线的真正断点**：MIR 形参落表只认 f64/bool/str/泛型 T，
  其余一律 I64 ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄属性/方法全退化。
- **两处叠加修法**：① MIR 形参落表识别库句柄标签（覆盖显式标注）；② resolver 调用点推断
  覆盖无标注形参（两种调用形状：dotted 与 from-import 裸名），AST 回写加 Named 分支。
- **实测**：`def f(d: PyDate): return d.year` 18388 → **2020**；无标注 `d.month` → **5**。
- 红线：python_style **160/160**（新增 t160，pre-fix 用 stash 验证 FAIL）；官方 **194/194**；
  语料解析 37/38 不回退（这是语义修复，未定义符号 83 不变）。
- **过程教训（第三次同类）**：`bash -c` **双引号**串里的反引号会被命令替换执行掉（commit message
  被吃两次）；写消息/文档一律用 `python3 - <<'PYEOF'` 带引号 heredoc 或 `git commit -F <file>`。
- commit `1678bdc8`（修复）+ `18d7fb4a`（roadmap），已 push。

## Final Summary

## Open / Deferred

- **参数类型推断（未标注形参）现在是唯一的真正瓶颈**：`getattr` 的安全改写、`*_N` 消除、`xs[0]` 返 0、`in` 早期恒 0、`date.replace` 之外的 handle 方法分派 —— 共同依赖都是「参数/接收者的静态类型」。建议下一轮直接开它
- `getattr(obj, "字段")` 的编译期改写：**可做但必须先查 type_map**（识别出已知 handle/struct 才改写，否则继续走未实现诊断）——本轮实测不查类型会变静默错值（18388），已回滚
- `range` 作为值：可考虑物化到 Vec（复用已有 `zeta_arange`），但需先定 1/2/3 参语义
- `_N` 族第二成因（注册表多签名）：logger.info 变参 / json.dumps 关键字 / getLogger 零参
- pandas/numpy 面（33 符号 / 92 调用点）、平台 API 宿主 shim（16 / 65）、跨文件链接（17 / 72）
- 运行时对象构建与跟踪（结构性）：tokio_runtime.o 被跟踪 / zeta_runtime_c.o 未跟踪 ⇒ fresh clone 只有一半运行时
- 工程：无宿主 shim 构建脚本；无端到端验收（与 Python 回测收益对齐）
