# Goal: 占位符符号 py_asdict_unexpanded 改为响亮失败

Goal Status: assumed
Workflow: complex

## Objective

继续推进 roadmap：收拾「占位符符号」这一类最小项 —— `pylib/registry.txt:208` 把 `dataclasses.asdict` 指向 `py_asdict_unexpanded`，这个符号在 C 与 src 里都不存在（注释说明 asdict 本应在编译期重写；重写不适用时就会发一个幽灵符号去链接期爆）。把它改成**响亮失败**：C 侧实现该符号，打印明确原因并 abort（像 zeta_assert_fail 那样），使同一文件的其他代码仍可编译链接，而真正走到未展开路径时立刻得到可读的失败信息 —— 不是返回假值。顺带核对 `range`(3)/`getattr`(2)/`dict`(2)/`object`(2) 这几个内建面符号的来源，能顺手收的一并处理，不能的记入 roadmap。

## Routing Decision

- Operation: New Ouroboros Work
- Workflow: complex
- Rationale:
  - 用户「继续推进 roadmap」；上一轮我把「py_asdict_unexpanded 占位符应改响亮报错」列为剩余小项之一，它是本轮预算内最小可验证的一项
  - 选它的理由：单点、纯 C 侧、无 Rust 关键路径改动、可用「未定义符号数 + 三套基线」验证，符合「不静默错值」的红线
  - complex：跨 registry/C/runtime 对象重建，且要守 194/194 与逐文件不回归

## Intent Stages

- [ ] implement — C 侧实现 py_asdict_unexpanded（打印原因 + abort），重建对应 runtime 对象；顺带核对 range/getattr/dict/object 的来源
- [ ] validate — 实测（未定义符号数下降 / 走到该路径时信息可读）+ 三套基线（官方 194/194、python_style 全绿、语料逐文件对拉无回退）+ roadmap + commit + push

## Success Criteria

### User-Specific
- [ ] `py_asdict_unexpanded` 不再是未定义符号，且被走到时打印可读原因并中止（非静默返回）
- [ ] 未定义符号去重数低于 85
- [ ] 官方 194/194；python_style 不新增失败；语料解析 37/38 不回退
- [ ] roadmap 回填 + commit + push

### Stage-Derived
- [ ] Requested behavior works as described
- [ ] Relevant checks/tests pass or limitations are documented
- [ ] Scenario results are recorded with evidence
- [ ] Pass/fail/blockers are clearly separated
- [ ] Scenario matrix covers expected success and failure paths

## Constraints

- 不得降低官方 194/194 退出码口径
- python_style 不得新增失败（当前 158/158）
- 只做「响亮失败」类改动，不引入静默错值（不返回假值假装成功）
- 改 runtime/*.c 后必须重建 gitignored 的 zeta_runtime_c.o 或 tracked 的 tokio_runtime.o（对应文件）
- Follow existing project patterns
- Do not refactor unrelated code
- Record important evidence and decisions in work.md
- Default to read-only validation; do not fix issues unless an implement stage exists

## Out of Scope

- 注册表多签名（_N 第二成因）—— 数据模型扩展，另批
- 参数类型推断、pandas/numpy 面、平台 shim、跨文件链接、运行时对象构建跟踪
