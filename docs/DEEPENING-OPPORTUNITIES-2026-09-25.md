# 深化机会报告 — Zeta 编译器（Deepening Opportunities）

> 日期：2026-09-25　|　方法：`improve-codebase-architecture` 流程（git 热区 → deepening 走查子代理 → 候选筛选）
> 词汇：**module**（有 interface 与 implementation 之物）、**interface**（caller 正确使用所需的一切）、**depth**（每单位 interface 撬动的行为量）、**seam**（不改此处代码即可改变行为的位置）、**deletion test**（删掉后复杂度在调用点重现 = 挣饭吃，消失 = pass-through）。
> 证据基础：本轮 deepening 走查（子代理实测 file:line）+ [业务规则清单](business-rules/pipeline-rules.md)（BR-xxx 锚点）+ [ARCHITECTURE-REVIEW-2026-09](ARCHITECTURE-REVIEW-2026-09.md)（P0/P1 编号，不重复其结论）+ [架构风险评审](architecture-risk/architecture-review.md)。
> 配套可视化报告：`$TMPDIR/architecture-review-<timestamp>.html`（运行时生成）。

---

## 候选总览

| # | 候选 | 推荐强度 | 一句话 |
|---|------|---------|--------|
| C1 | 符号绑定：名字瀑布 → SymbolRegistry 深模块 | **Strong** | "调用绑到哪个符号"一个概念散布在 6 个模块，各自带启发式 |
| C2 | 类型判定：四层启发式叠层 → 一个 `type_of` 模块 | **Strong** | 类型答案要跨 4 个模块拼；HM 基建是有 implementation 无 caller 的假想 seam |
| C3 | gen.rs：五个语义域从 `lower_expr` 拆成内部 pass | **Strong** | 近 120 提交 22 次改同一个文件；语义域共享一个 match 一个状态机，互不可测 |
| C4 | 失败降级：五个告警-继续点 → CompilePolicy 模块 | Worth exploring | "该不该停"散布在 5 个 call site + 4 个 env 开关 + 2 套失败语义 |
| C5 | 删除死重：deletion test 的一次性收成 | **Strong** | ~5000–6000 行零 caller 代码，deletion test 逐项判死 |
| C6 | pylib 绑定：三方重叠 → 单一 PyBinding 表 | Worth exploring | 优先级规则只存在于注释 |

---

## C1 · 符号绑定：名字瀑布 → SymbolRegistry

**Files**：`src/backend/codegen/codegen.rs`（`get_or_declare_function` 18 档瀑布 `codegen.rs:2640-2952`；`get_function` 8 档 `:2275-2367`）、`runtime/tokio_runtime_stub.c`（`.set` 别名）、`pylib/registry.txt`、`src/backend/codegen/jit.rs`（`JIT_MAPPINGS`）、`src/lib.rs:154-620`（手写 `add_global_mapping`）、`runtime_decls_registry.rs`（生成物，未接线）

**Problem**：符号绑定模块是 shallow 的——caller（gen.rs 的 Call lowering、codegen 的名字解析）必须知道的 interface 事实包括：瀑布 18 档的**顺序**、`name_N` 后缀的双向约定（gen_mirs 加、codegen 剥）、`zeta_*`/含 `__` 名**永不**加后缀的豁免、`str_*`→`host_*` 前缀改写、`name.<N>` 用调用点实参数而非 LLVM 计数的近似。interface 几乎与 implementation 等宽。同一个 seam 目前有 **6 个平行 adapter**（AOT 瀑布 / C `.set` / registry.txt / JIT 生成表 / JIT 手写表 / 未接线生成物），其中 AOT 与 JIT 两套清单可各自演化（BR-A19）。

**Solution**：一个 SymbolRegistry 模块，interface 收敛为 `resolve(name, arity, type_args) -> SymbolEntry{signature, storage}`；瀑布降级为它的 implementation 细节；Rust 声明、C 头、registry 条目、JIT 映射由一份 schema 生成。

**Before/After**：

```
Before: 6 个平行 adapter，caller 记 18 档顺序      After: 1 个深模块，1 个 interface
┌────────┐┌────────┐┌────────┐┌────────┐          ┌──────────────────────────┐
│AOT瀑布 ││C .set  ││registry││JIT×2  │   ──►    │  SymbolRegistry          │
│18档顺序││66条别名││.txt    ││生成+手写│          │ interface: resolve(name, │
└───┬────┘└───┬────┘└───┬────┘└───┬────┘          │  arity, type_args)       │
    └─────────┴────┬────┴─────────┘               │ ════════════════════════ │
                   │ caller 必须知道全部豁免       │ (schema 生成 4 个面；     │
                   ▼                              │  瀑布变成内部实现细节)    │
              静默错绑定/链接失败                  └──────────────────────────┘
```

**Wins**：locality：绑定错误集中一处 · leverage：一份 schema 生成 4 个面 · 删 3 份平行映射表 · tests hit one interface（今天唯一的闸门是 shell 级 `check_registry_symbols.sh`）

**对照**：与 ARCHITECTURE-REVIEW P0#1 同源（引用不重复）；本报告补充的增量证据：codegen 无 `#[cfg(test)]`、`runtime_decls_*.rs` 是未接线的第 6 份平行物、AOT/JIT 双轨演化风险（BR-A19）。

---

## C2 · 类型判定：四层启发式叠层 → `type_of` 深模块

**Files**：`src/middle/resolver/resolver.rs`（字符串归一 `:104-141`、6-pass 证据 `:1213-1774`、句柄表 `:2028-2062`、三层覆盖 `:3241-3350`）、`src/main.rs:80-287`（`refine_param_types`）、`src/middle/resolver/typecheck.rs:353-427`（万物 i64）、`src/middle/types/`（5400 行 HM 基建）

**Problem**："这个表达式/参数是什么类型"没有一个 module 拥有答案——要跨 4 个模块按固定顺序拼。类型 interface 无任何直接单测：resolver.rs 无 `#[cfg(test)]`，tests/unit 里 `Resolver` 只被当道具传给 BorrowChecker，没走 `register` + 类型判定；唯一有效的测试是整程序 golden。`types/` 的 HM 基建是**有 implementation 无 caller 的假想 seam**（one adapter = hypothetical seam）：`family.rs`/`associated.rs` 零引用，`unify` 仅新轨道内部消费，而新轨道对推不动的节点整体静默跳过（BR-R01/R02/R04）。

**Solution**：单一 `type_of(node) -> Type` interface；未标注 → 显式 `PyDynamic` 而非静默 i64。HM 基建二选一：接成唯一 engine（第二个 adapter 到位，seam 变真），或删除（deletion test：无 caller → 复杂度不重现 → pass-through，删除即集中）。

**Before/After（cross-section）**：

```
Before: 答案穿 4 层薄带                  After: 1 层厚带
┌─ register 字符串归一 ────── h-8 ─┐     ┌──────────────────────────┐
├─ 6-pass 调用点证据 ──────── h-8 ─┤     │ type_of(node) -> Type    │
├─ lower_to_mir 签名恢复 ──── h-8 ─┤ ──► │ ════════════════════════ │
├─ refine_param_types (MIR后) h-8 ─┤     │ 深 implementation：归一/  │
└─ 兜底：万物 i64（静默）──────────┘     │ 证据/恢复仍是内部细节    │
  caller 无法回答"strict 下会怎样"        └──────────────────────────┘
```

**Wins**：locality：类型 bug 集中一个模块 · "interface is the test surface" 成立（今天不成立）· 未标注参数从静默 i64 变显式 PyDynamic · 决策点：删 or 接 5400 行假想 seam

---

## C3 · gen.rs：五个语义域 → 内部 pass 群

**Files**：`src/middle/mir/gen.rs`（14,739 行；近 120 提交中 **22 次**改动，热区第一）

**Problem**：真值判定（`:4030-4100`）、二元运算符重写（`:4004` 起同一 arm）、f-string（`:3981-4001`）、下标（写 `:1758` / 读 `:9341-9351`）、模块全局 mirror（`:523-616`）五个语义域挤在 `lower_expr` 一个 match 里，共享同一个 `stmts/type_map/next_id` 状态机——**互不可测**。每修一个批次都要回到同一个 ~8000 行函数找插入点：locality 最差的地方恰是改动最频繁的地方。此外 MirGen 的构造 interface 过宽：**14 个 `with_*` builder**，caller 必须喂满，否则绑定静默出错（BR-R17 的 14 表克隆即源于此）。

**Solution**：外部 seam 不动（`lower_to_mir(fn) -> Mir`），内部按语义域拆 5 个 pass——**internal seams**，各自可独立驱动测试；14 个 `with_*` 收敛为一个 Context 结构传入。

**Before/After（call-graph collapse）**：

```
Before: 一个 match 吞五个域            After: 外部 seam 不变，内部 5 个可测 pass
┌─ lower_expr (~8000 行 match) ─┐     ┌─ MirGen  (interface 不变) ───┐
│  真值 / 运算符 / f-string /   │     │ ┌truthy┐┌ops┐┌fstr┐┌idx┐┌glb┐│
│  下标 / 全局 mirror           │ ──► │ │ pass ││pass││… 内部灰色，│
│  共享 stmts/type_map/next_id  │     │ └──────┘└────┘  各自可测 ───┘│
│  无 #[cfg(test)]，仅 golden   │     │ Context×1 取代 with_*×14     │
└───────────────────────────────┘     └──────────────────────────────┘
```

**Wins**：locality：22/120 提交的修改面集中 · 5 个语义域可独立测试 · 构造 interface 从 14 收敛为 1 · 外部 caller 零迁移

---

## C4 · 失败降级：五个告警-继续点 → CompilePolicy

**Files**：`src/main.rs:384-795`（W1002/W0001/W0002/W0003 + borrow 静默）、`src/lib.rs:109-122`（同一失败的另一套硬错误语义）、`src/diagnostics.rs`（`env_flag` 7 键）、`src/error_codes.rs`（W0003 未注册、一码三义）

**Problem**："这个失败该不该让编译停止"没有 owner：5 个 call site 各自裁决，4 个 env 开关（`ZETA_STRICT_PARSE/ABI/RUNTIME_DIR`、`ZETA_NO_OPT`）各自接线，CLI 与 lib.rs 是**两套失败语义**；W0003 以 error 级打印却非致命。测试只能靠退出码反推（而 E4001 报错 rc=0，退出码也不可信，BR-P07）。

**Solution**：一个 CompilePolicy 模块，interface = `decide(Diagnostic) -> Fatal | Warn`。CLI / lib.rs / REPL 三个 caller 对同一诊断的不同裁决，恰好证明这个 **seam 是真实的**（两三个 adapter 已存在）；env 开关降级为 policy 的参数。

**Wins**：strict 语义一个模块内可测 · 一套失败语义 · 退出码恢复信用 · 新诊断不再自选严重级别

---

## C5 · 删除死重（deletion test 收成）

**Files**（全部经 deletion test 验证零 caller）：`src/middle/optimization.rs`（603 行，唯一"caller"是 compiler_config 的 import）、`src/compiler_config.rs`（373 行）、`src/frontend/{proc_macro, macro_expand_advanced, borrow_enhanced, identity_ownership}.rs`（~2500 行）、`src/backend/codegen/monomorphize.rs` 自由函数、`unified_typecheck.rs` 门面结构 + `TypeCheckMigrator`、`src/middle/types/{family,kind,associated}.rs`、`src/runtime/{memory_old, memory_enhanced}.rs`、`runtime_decls_{registry,core}.rs`、`Mir.ctfe_consts` 字段

**Problem**：~5,000–6,000 行零 caller 代码随每个产物编译、随每次导航被读（AI-navigability 直接受损——每个死模块都要走一遍才能确认是死的）；`#![allow(dead_code)]` 关掉了编译器的免费检查。

**Solution**：删除（或 `#[cfg(test)]` 隔离有保留价值者）。deletion test 逐项判死：无 caller → 复杂度不重现 → 全部 pass-through。

```
Before ████████████████████████░░░░ ~6% 死重随产物编译   After ████████████████████████ 编译器重新帮盯
```

**Wins**：编译器免费检查恢复（去掉 allow(dead_code)）· AI 导航面 -6% · 一小时级成本 · 为 C1–C3 清出地基

---

## C6 · pylib 绑定：三方重叠 → 单一 PyBinding 表

**Files**：`pylib/*.z`、`pylib/registry.txt`（F/W 行）、`src/middle/mir/gen.rs` MIR 拦截（`np.where` 多参、`np.zeros` 元组展开、"registered members win"）、`src/middle/pylib.rs`（`find_member` 递归/唯一性闸门）

**Problem**："pandas.read_parquet 绑到哪"由三方共同决定，优先级规则只写在注释里（`resolver.rs:502-513` 的双源合并规则无结构化记录，BR 漂移项）；加一个内建要同步 3–4 处。

**Solution**：绑定查询收敛为一个 PyBinding 模块（可作为 C1 的库面子模块）；gen.rs 的语义特判变成表数据。

**Wins**：优先级从注释进代码 · 加内建 = 改一处 · fail-loud 校验有单一落点

---

## Top recommendation

**先 C5（删死重），主攻 C1（SymbolRegistry）。**

C5 一小时级：去掉 `#![allow(dead_code)]` 后编译器开始免费帮盯，AI 导航面立减 ~6%，且为后续所有深化清场。C1 是"链接失败/静默错值"类缺陷的最大单一来源（四份历史幽灵符号事故都在注释里），而 **AOT 与 JIT 两套绑定清单的并存证明这个 seam 是真实的**（two adapters = a real seam）——现在的问题只是 6 个 adapter 互不认识。C3 紧随其后（改动频率最高的文件可测性为零）。

---

## 与既有文档的关系

- ARCHITECTURE-REVIEW-2026-09.md（09-19）：P0#1 ≈ C1、P0#2 ≈ C2 的一部分、P1#5 ≈ C2、P1#6 ≈ C3、P1#11 ≈ C5。本报告以 deepening/interface 视角重组并补充了测试性证据（哪些 seam 今天完全没有 interface 级单测）。
- docs/architecture-risk/（09-25）：扩展性风险视角（R#、场景 S#），与深化候选互补，未发现矛盾。
- docs/business-rules/pipeline-rules.md（09-25）：本报告 BR-xxx 引用的规则与强制点来源。
