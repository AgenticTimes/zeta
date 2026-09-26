# 业务规则提取（Business Rules Extraction）

> 方法：business-rules-extraction 工作流（分层地图 → 并行深提取 → 源码验证 → 强制点矩阵 → 流程追踪 → 文档化）。
> 所有规则的"强制点"均定位到 file:line；抽取时的行号为快照，随主线演进会漂移。

## 索引

| 文档 | 范围 | 规则数 | 日期 |
|------|------|--------|------|
| [pipeline-rules.md](pipeline-rules.md) | 全系统管线级：编排降级链（BR-P）、前端（BR-F）、解析/类型（BR-R）、CTFE/MIR（BR-M）、后端 codegen（BR-B）、运行时/pylib（BR-T）+ 补提取（BR-L 词法/降糖、BR-C 容器/并发、BR-DG 诊断/语法缺口）+ 附录 A（Declared But Not Enforced）+ 附录 B（流程追踪） | ~138（85 主提取 + 53 补提取） | 2026-09-25 |
| [COVERAGE.md](COVERAGE.md) | 对照 pyramid.md 功能树的覆盖度矩阵：补提取后 ✅15 / 🟡6 / ⭕B 类未实现 6 / ➖ 有意不做 3（A 类缺口已填平） | — | 2026-09-25 |

## 核心结论速览

1. **失败降级链是本仓库最重要的系统级规则**：W1002/W0001/W0002/W0003 + borrow 静默丢弃五个降级点使主 CLI 管线几乎不可能因前端/中端失败而停止——正确性押在链接期 undefined symbol 这道非强制性防线上（pipeline-rules.md §1.1）。
2. **fail-loud 红线在 pylib/import 侧守得住、在管线侧守不住**：未知 Python 成员"绝不静默 no-op"（BR-T02），但解析截断、类型失败、borrow 错误全部告警继续。
3. **四层启发式叠层构成实际类型系统**：字符串归一 → 6-pass 调用点证据 → 签名恢复 → refine_param_types；HM 基础设施基本未消费（BR-R04）。
4. **死开关/死代码约占声明面的重要比例**：优化器 5 pass、`-O0..-O3`、`compiler_config.rs`、双轨 typecheck 陪跑链、`types/{family,kind,associated}` 等——汇总见附录 A（24 项）。
5. **最要紧的一处文档漂移**：ABI.md 未记录批次 399 的签名单一事实源改造（附录 A14），影响其后 R7/M5/M6 三节结论。

## 复用说明

- 提取下一层（如 MIR 层专项深挖）时，以 pipeline-rules.md 对应 BR 段为起点、MODULE-GUIDE.md 为权威对照。
- 新规则按既有编号段续编（BR-P/F/R/M/B/T-xxx），并在本索引登记。
