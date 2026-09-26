# Zeta 编译器路线图 2（当前生效的优先级源）

> 建立：2026-09-25。首版依据：2026-09-24 用户裁定（refactor.md §9）+ 2026-09-25 三份新证据入册。
> **怎么读这份文件**：前一份 `roadmap.md`（1.4MB）是批次 1–409 的历史执行日志，已冻结、只读。
> 从批次 410 起，新的批次记录写在本文 §6；工作优先级也以本文为准。
> 新任务的唯一登记入口是 [backlog.md](backlog.md)（§4 里暂存的待登记项按其规则取空闲编号迁入）。
> 基线门禁（`tools/run_all.sh`）和批次纪律（每批独立提交 + 证据入册）不变。

---

## 0. 文档分工（哪份文件管什么）

| 文件 | 角色 |
|---|---|
| `roadmap.md` | 批次 1–409 的执行日志（历史归档，只读） |
| **`roadmap2.md`（本文件）** | 当前生效的路线图：主链、收尾顺序、批次 410 起的记录 |
| `backlog.md` | 新任务的唯一登记点（OPEN 数 ≤ 30，有记账） |
| `refactor.md` | 七轴重构计划与方法论（状态行待刷新，见 §4-8） |
| `docs/business-rules/pipeline-rules.md` | 业务规则与强制点证据源（可引用 BR-xxx 编号） |
| `docs/DEEPENING-OPPORTUNITIES-2026-09-25.md` | 深化候选 C1–C6（与 refactor.md 各轴的映射见其末节） |
| `docs/architecture-risk/architecture-review.md` | 扩展性风险 R1–R5（轴 D④ 的靶子数字以 R1 为准） |

## 1. 当前快照（2026-09-25，批次 409 收口后）

**主链走到哪了**：类型基础有三步——① 掩码类型 ✅（批次 398 完成）→ ② 跨函数签名表 ✅（批次 399 完成，顺手把 #33 的「无声明半」收敛了，全语料的 R7 判据从 5 行缩到 1 行）→ **③ 最小类型检查，这是下一步**。

**还挂着的一批格子**（按批次 408/409 §十一的盘点）：

- #33 还剩两种形态（声明与实现不一致时按位重读；一元负号 #115）
- #117（动态接收槽没有标记）
- #118
- 闭包参数侧（408 §十一b）
- p2 族（容器元素型）
- **409 (a)①②：`len(cache["k"])` 返回 0 / 参数冲突后 `len([1,2,3])` 返回 1** —— 这是下一批的默认候选，是 BR-R12/R13 证据链的延续；强制点在 `resolver.rs:1655-1735`

**悬而未决的事**：

- 406 §十(b)：C 运行时要不要接进 JIT（已升格为 G.5e 的前置问题，见 §2）
- acceptance 的非确定性判据还没立起来
- `run_all.sh:576` 的退出码口径有两种读法，没对齐（409 §十一f）

## 2. 主链（2026-09-24 裁定原文 + 2026-09-25 增补）

按顺序做，每格做完才进下一格：

```
类型基础③ 最小类型检查（未标注 = 动态值 + 动态值位置清单，2-3 天）   ← 当前在这
  ▼
#38 match 不进中间代码 + #45 格式化丢字（都在 gen.rs 同一段，联合重构，2-3 天）
  ▼
主线批次 301 恢复（现象：0 笔成交 → 选股数量分歧 → 追 final_value，2-4 天）
  ▼ 主线收尾后
F.4 全量迁移（把 parser/MIR/codegen 三处的定型特判拆掉）
B/T3-T5 值模型装箱（边界清单来自 ③；装箱正确性用 final_value 对齐来验证）
G.1 修复 + G.4 消灭降级
轴 C + 轴 D④（拆分 gen.rs；靶子数字以 R1 为准：lower_expr 单函数 10,371 行 / 占 gen.rs 71%）
G.5e 符号注册表（= C1 SymbolRegistry）
```

**增补一（G.5e 的前置问题）**：动 G.5e 之前必须先回答 **406 §十(b)（C 运行时接不接进 JIT）**——这个决定影响 JIT 要绑定几份接口，直接决定 SymbolRegistry 的 schema 要生成几个面。这个问题没回答前，G.5e 排在后面不动。

**增补二（清理期收尾顺序的插入，见 §3）**：CLI 降级链和 ABI.md 补记这一族，插在「截断收尾之后、回主线 301 之前」。

## 3. 清理期收尾顺序（2026-09-23 裁定的延伸）

1. **B 类解析器截断**（#36 一族。现状：9 个文件 / 1,019 行被截丢，批次 325 后的数据，以 docs/ABI.md 附 B#10 为准）——不变，仍是清理期的唯一最优先。

2. **截断清零后、回主线前，把 C/D 类并成一个批次闭合**（跑快速门禁），新增三族（详见 §4）：
   - **管线降级链收敛**：borrow 检查结果被整段丢弃（P06）→ W0003 注册 + 一个错误码三种含义拆分（A5）→ CLI 和 lib.rs 两套失败语义对齐（A3）→ E4001 找不到 main 却返回成功（P07）。全部是零行为变更/只改报告，符合「报告先行原则」。
   - **CLI 合同小修族**：env 清单补上 `ZETA_NO_OPT`（A7）、重复 `-o` 参数预扫描取第一个 vs 循环取最后一个的分叉（A8）、bootstrap 吞掉 80 字节输出没人看见（A9）、bootstrap 链接没有 Windows 分派（A10）。挂到既有的 `tools/cli_semantics_check.sh` 门禁下。
   - **ABI.md 补记批次 399**（§4-6）：§3 里的 C2/C3/R7/M5–M7 还停在批次 320/387 时点的状态。

3. **S/M 级任务清零就回主线批次 301**，不等 5 天上限（09-23 裁定原文，不变）。

## 4. 新增登记项（待迁入 backlog.md，编号取空闲段）

| # | 项 | 强制点 / 证据 | 级别 | 来源 |
|---|-----|---------------|------|------|
| 1 | borrow 检查结果整段丢弃：失败置 `ok=false` 后从来没人读，还有 `let _ = ok;` 把告警按住 | `resolver/typecheck.rs:15-28`（BR-P06） | S | pipeline-rules P06 |
| 2 | W0003 没注册 + 一个错误码三种含义、两种严重级别（fallback 告警 / 非致命 typecheck（error 级打印）/ identity 告警） | `error_codes.rs:2162-2164`、`main.rs:789`、`typecheck.rs:57,528`（BR-A5） | S | pipeline-rules P05/A5 |
| 3 | CLI 和 lib.rs 是两套失败语义（`compile_and_run_zeta` 自称 primary API 但 main.rs 从来没调用过它；一边是 W 告警一边是硬错误） | `lib.rs:89-122` vs `main.rs`（BR-A3） | S | pipeline-rules A3 |
| 4 | E4001 找不到 main：打完 error 诊断还是 `Ok(())`（退出码 0），三处同病 | `main.rs:1026-1041/:1238-1244/:1324-1328`（BR-P07） | S | pipeline-rules P07 |
| 5 | CLI 合同五小项：env 清单漏了 `ZETA_NO_OPT`；重复 `-o` 预扫描取第一个 vs 循环取最后一个；bootstrap 吞 `--target`；bootstrap 静默残留 ≥80 字节输出；bootstrap 链接没有 Windows 分派 | `main.rs:527/:637-650/:1148-1155/:1216-1227`（BR-A7/A8/A9/A10/P12） | S×5 | pipeline-rules §1.2/附录A |
| 6 | ABI.md §3 补记批次 399：`infer_fn_return_type` 已委托给 `Mir::signature_ret_ty`（`mir.rs:45-58`），R7 收敛了「无声明半」（5 行→1 行）；C2/C3/M5–M7 相应收窄（M5 还在：声明与实现不一致时按位重读） | `codegen.rs:1400-1409`；backlog #33 行已记，ABI.md 本体没记 | S | pipeline-rules A14（精确化） |
| 7 | G.6 的第一条不变量：CTFE 的 floordiv/floormod 语义和 codegen 的 `build_floormod_int/floordiv_int` 必须步调一致——目前纯靠注释人工同步，没有共享代码也没有测试锚点 | `ctfe/value.rs:274-335` vs `codegen.rs:2020-2081`（BR-M04） | S | pipeline-rules M04 |
| 8 | 轴 A 的表补两行：前端死代码四件套（proc_macro 845 行 / macro_expand_advanced 617 / borrow_enhanced 611 / identity_ownership ~470）；`runtime/memory_old+memory_enhanced` 孤儿文件 | `dc_default.txt` 基线占大头；refactor.md §1 表没点名 | S | 深化评审 C5 |
| 9 | （排序注记，不是任务）refactor.md §9 ⑨′ 的 G.5d 状态行刷新：②(b) 已由批次 399 交付「无声明半」，指针应指向 backlog #33 | refactor.md ⑨′ vs backlog #68 行 | S | 本文件 §2 |

## 5. 证据锚点怎么用

- 批次记录里引用规则时直接带 BR 编号（例如「BR-R13 参数冲突状态机，`resolver.rs:1671-1717`」），省得重新定位。
- 深化候选和 refactor.md 各轴的对应关系：C1→G.5e、C2→轴 F、C3→轴 D④、C4→G.4（扩容）、C5→轴 A、C6→G.5e 库面子模块。
- 数字以 2026-09-25 实测为准：gen.rs 共 14,644 行 / `lower_expr` 一个函数 10,371 行 / MirGen 结构体 45 个字段 + 15 个 `with_*` 构造器 / codegen 里 0 个 `Result` + 生产路径上 507 个 `.unwrap()`（出处 R1–R3）。

## 6. 批次记录（自批次 410 起追加于此）

（暂空——下一批从 409 §十一 (a)①② 的隔离复现开始，记录格式沿用 roadmap.md 的四段证据纪律。）
