# Goal: 接管 refactor 七轴推进（batch 346+）

Goal Status: assumed
Workflow: complex

## Objective

接管 zeta-src 的 refactor 推进（原由 Qoder 执行，已停约 2 小时）。按 refactor.md 七轴计划（A 死代码减法 / B 类型标签 / C 编译性能 / D 层次可维护 / E 测试架构 / F 类型检查器独立 / G 正确性工程）持续推进，每批次 commit + push 到 agentic/bootstrap。现状：Qoder 已完成到 batch 345，工作区留有 64 项未提交改动（含 src/blockchain/* 36 个删除 + docs/ABI.md + tools/run_all.sh，diff 合计 -15066 行）。

## Routing Decision

- Operation: New Ouroboros Work
- Workflow: complex
- Rationale:
  - 用户明确要求接管并持续推进 refactor.md 的七轴计划
  - refactor.md 已有完整排程（§9 总排程与依赖）+ 完成判据（§10），目标清晰
  - 当前有明确的接手点：Qoder 未提交的轴 A 减法工作 + backlog.md 登记项

## Intent Stages

- [ ] understand — 接手盘点：读 refactor.md §9 排程 + backlog.md 登记项，验证当前工作区可编译与三基线状态，确定 Qoder 未提交改动的完整性与归属
- [ ] implement — 按 refactor 轴推进：每批次一个 commit（代码/文档分开），修复 + 新增回归用例 + 跑门禁
- [ ] validate — 每批三基线全绿 + MIR diff 不变性（重构类改动）+ roadmap/backlog 证据登记，push 到 agentic/bootstrap

## Success Criteria

### User-Specific
- [ ] 每批次 commit + push 成功（代码/文档分开提交）
- [ ] 三基线持续全绿：官方 194 + python_style 278/2 + 语料 100%
- [ ] Qoder 未提交的 64 项改动得到妥善处置（验证后提交或保留说明）
- [ ] refactor.md 的轴按排程推进，每步独立提交独立回滚
- [ ] 每批有证据登记（roadmap.md 或 backlog.md）

### Stage-Derived
- [ ] Relevant code/docs were read and important claims are backed by evidence
- [ ] Explanation identifies responsibilities, call paths, and key risks
- [ ] Key dependencies and caller/callee relationships are documented
- [ ] Requested behavior works as described
- [ ] Relevant checks/tests pass or limitations are documented
- [ ] Scenario results are recorded with evidence
- [ ] Pass/fail/blockers are clearly separated
- [ ] Scenario matrix covers expected success and failure paths

## Constraints

- 每批次 commit + push（git push agentic bootstrap）
- 三基线红线：官方 194 + python_style 存量 2 红 + 语料 100%
- 全程 ZETA_NO_OPT=1；长跑 timeout 包裹
- 禁 rm 删除代码，用 mv → .trash/
- 禁止代码与文档混合提交（refactor 原则 2）
- 接手前先保住 Qoder 的未提交工作，不得丢弃
- Do not ask the user questions that can be answered from code or docs
- Record important evidence and decisions in work.md
- Follow existing project patterns
- Do not refactor unrelated code
- Default to read-only validation; do not fix issues unless an implement stage exists

## Out of Scope

- 推翻 Qoder 已完成的批次
- 未经验证就丢弃其未提交工作
- 修改 classdesign.md（我的分析文档）
