# Zeta 架构优化分析

> 2026-09-20，基于 HEAD d30274fe（bootstrap 分支，roadmap 批次 283）。
> 只读分析，不改代码。所有结论附可抽查证据（file:line 或验证命令）。

## 0. 现状快照

| 维度 | 数据 | 来源 |
|---|---|---|
| Rust bootstrap 编译器 | 220 文件 / 94,712 行 | `find src -name '*.rs' \| xargs wc -l` |
| Zeta 自举源码 | 51 文件 / 7,288 行 | `find zeta_src -name '*.z'` |
| C 运行期 | ~7,900 行（py_additions.c 独占 3,104） | `wc -l runtime/py_additions.c` |
| 基线 | 官方 194/194、python_style 274/2、语料 39/39 | roadmap.md 批次 279-283 度量行 |

src/middle/ 32.6k 行占全仓 1/3，其中 `mir/gen.rs` 单文件 **12,313 行**，是全仓最大单体。

## 1. 核心矛盾：架构债在自举期被指数放大

项目北极星是"Zeta 编译 Zeta、甩掉 Rust bootstrap"（README）。当前架构与之冲突的根问题只有一个：

> **类型信息在编译期不可靠传播、在运行期完全缺失，导致每个语义分派点都要在 MIR/解析器/运行期三层各自"猜"类型。**

证据链（近期批次全部命中同一根因）：

- 批次 279：`in` 分派只看语法形态，MIR 无容器布局信息 → 修在解析器降级元组字面量（src/frontend/parser/expr.rs:2340-2352）。**残留**：元组变量的成员测试仍恒 0——HEAD 实测 `y = ("a","b"); "a" in y` → 0（静默错值，非响亮失败），且编译时打出误导警告 "`in` membership is only supported for strings/dicts in V1"。
- 批次 282：向量比较一律走 `strtod` 数值路径（runtime/py_additions.c:787, 1053-1054）→ 日期字符串比较恒假 → 修法是 MIR 里查 `type_map[vec_id] == DynamicArray(Str)` 才改派 strcmp（src/middle/mir/gen.rs:3580-3609）。
- 批次 283：bool 掩码与 i64 向量同型不可分（gen.rs:3607 掩码粗类型为 `DynamicArray(I64)`，布尔语义丢失）→ `df[mask]` 被当列名 → MIR 判据 `Named("DataFrame") && DynamicArray(I64)`（gen.rs:11127-11158）。HEAD 实测已修好（限定名/非限定名两种导入形态 `a[m]` 均返回正确 2 行），**roadmap 停在"判据未命中"是过时状态，驱动崩点应已解除，值得优先复跑验证**。
- 运行期侧：int64 句柄"到底是什么"靠指针区间启发式（runtime/py_additions.c:1139 `x > 0x100000000 && x < 0x7fffffffffff`）和 GC_base 证明（:1154-1159）探测；pylib/pandas.z 头部注释本身就是一份脆弱性清单（形参叫 `name` 会吞掉方法、`__getitem__` 假定 key 恒为列名 :35-37）。

**结论**：只要"值不带运行时类型标签、静态 type_map 不跨作用域可靠传播"这两个缺口在，批次就会以"最小复现 → 加一个特判分支"的节奏继续消耗，且每个特判都留下形态盲区（如 279 的元组变量）。这是架构问题，不是实现进度问题。

## 2. 优化轴 A — 死重量清理（成本最低，先做）

| 项 | 规模 | 证据 |
|---|---|---|
| new_resolver 双轨死管道 | 3,115 行 | `wc -l src/middle/resolver/{new_resolver,typecheck_new,type_cache}.rs`；全流水线仅用 `Resolver`（src/main.rs:307,584,703,786；src/lib.rs:108,869），new_resolver 只在 mod.rs:5 声明、types/mod.rs:319 一条注释提及，零调用 |
| `crate::ml` / `crate::distributed` | 6.2k 行 | 核心零引用（`grep -rn "crate::ml\|crate::distributed" src \| grep -v 自身目录` 为空），却无条件编译（src/lib.rs:62,65） |
| blockchain 模块 | 5.6k 行 + solana-sdk 等重依赖 | 非默认 feature（Cargo.toml:106），默认构建外无人引用 |
| 孤儿模块 | 1,309 行 | src/holographic、temporal、consciousness、reality 从未在 lib.rs 声明（grep 无命中），永不编译；挂在 lib.rs:72 的 paradigm_simple.rs 是演示代码 |
| 根目录/源码树杂物 | — | b3ir.log、tokio_runtime.c/.o、zeta_runtime_c.o、test_*.z 在根；src/ 下 40 个 .z/.rs 测试杂file + 构建产物（src/target_check/、src/rmetaxtFyon/full.rmeta） |

净收益：编译依赖图缩小、`cargo build` 时间下降、roadmap 调试时少 3 千行"这代码还有人用吗"的判定负担。**风险**：删除前需逐项确认 `bin/`、examples、CI 无引用（上表的 grep 已覆盖 src/，还需覆盖 tests/ 与 workflows/）。

## 3. 优化轴 B — 类型系统根治（终止换坑式批次的唯一出路）

三个缺口对应三个动作，按依赖顺序：

1. **动态值携带运行时 tag**（影响：高）。给 Dynamic/PyDynamic 值加单字节类型标签（str / i64 / bool_vec / i64_vec / df_handle / dict_handle…），运行期按 tag 分派，替换 py_additions.c 全部指针区间探测（:1139）与 GC_base 猜测（:1154-1159, :707-712）。全库现无任何 tagging 方案（grep NaN-boxing/type tag 无命中；`Type::PyDynamic` src/middle/types/mod.rs:181 只是编译期占位）。roadmap.md:621 曾有一次"静态标签教训"，做之前先读该条。
2. **MIR 区分 bool 掩码与 i64 向量**（影响：高，是 1 的子集）。批次 282 的掩码产物（gen.rs:3607）带布尔语义标记，则 283 类判据从"猜接收者名字"变成"读标记"，`df[掩码]`/`df[列名]`/`df[int索引]` 三分派不再依赖 `Named(tn).contains("DataFrame")` 字符串匹配。
3. **type_map 跨作用域可靠传播**（影响：中）。当前以 ExprId 为键的扁平表（gen.rs:108）+ 跨函数靠 mangle 名字符串剥前缀匹配（gen.rs:835, :967），未标注形参默认记 I64（t154 注释）。修向：形参/全局槽位类型进符号表而非名字匹配。

验收建议：以批次 279 残留（元组变量 `in`）为第一块试金石——它是当前最小、最干净的"类型信息缺失"复现。

## 4. 优化轴 C — 编译流水线性能（影响自举速度）

- **1,354 处 `.clone()`**，热点 gen.rs 304 处、resolver.rs 164 处、codegen.rs 75 处。无 rustc-hash / 无符号 intern：`gen.rs:105-148` 一个结构体即 8 张 `HashMap<String,_>`；middle+backend 共 201 处 String 键 HashMap。
  修向（低风险、可分批）：① 引入 `rustc-hash`，String 键表换 `FxHashMap`；② 符号 intern（名字→u32），type_map/名字匹配随之脱字符串；③ clone 热点用 `&str`/Cow 消除。
- **线性 scope 查找**：src/middle/ctfe/context.rs:82,113,176,189 逐层 `scopes.iter().rev()`，深嵌套 O(深度)，可加 name→层级索引。
- 收益量化需要基线：先测 `time zetac zeta_src/main.z` 当前编译耗时，作为 C 轴各步的前后对照。

## 5. 优化轴 D — 层次与可维护性

- **gen.rs 12,313 行拆分**：roadmap.md"还债执行计划（批次 260+）"已定稿五项任务（含 gen.rs 拆分、单态化、契约测试），本分析与它不冲突——**A/B/C 轴应与该计划合流排程**，避免两套改进互相踩。建议顺序：还债计划前置脚本（其 ④ 的安全网）→ 本文轴 A 清理 → 轴 B tag 专项。
- **前端越层依赖 middle**：frontend 7 个文件直接 `use crate::middle::...`（src/frontend/identity_ownership.rs:7-9、parser/identity_type.rs:17 等）。自举时 Zeta 侧要按阶段切编译器，这些边会让"先自举 frontend"不可能独立进行。修向：把 identity 类型定义下沉到共享 types 层，或定义阶段间接口。backend→frontend 为 0，方向正常。
- **resolver 双轨决断**（轴 A 的架构半边）：要么明确 new_resolver 是继任者并把主 resolver 的逻辑迁过去，要么删除。当前"3 千行挂着不接线"是最差状态——每次改语义都要检查是否两边都要改。

## 6. 优化轴 E — 测试架构（把"全绿"变成有意义的信号）

结构性缺口：**官方 194 用例 compile-only**（tools/run_all.sh:41-52 只 `zetac x.z -o`，不跑），所以批次 266/267/272/273/279/282 六种"静默错值"在 194 全绿下照样漏过。真正跑值的是 python_style（`// expect:` 快照，276 用例）和语料 39 文件。

已确认的回归空白（python_style 全目录 grep）：
- `in` 元组：t100_in_bool.z 只覆盖 `in [list]`；
- `df[bool_vec]` 掩码：无任何用例（t213/t217 只沾 where）；
- 字符串向量比较：仅 t154 沾边。

修向：
1. 每个修复批次落一个 expect 快照用例是硬门禁（批次 279/282/283 三个都欠着；HEAD 已实测 282/283 行为正确，用例可以立即补而不会红）。
2. 已知坏形态（如元组变量 `in`）用 `// known-fail:` 单列进 python_style——run.sh 已支持该标记且 XPASS 会报出来，等于免费拿到"修复自动转正"机制。
3. CI（.github/workflows/ci.yml:33, :106）已跑三基线，缺的是运行值覆盖比例，不是缺 CI。

## 7. 优先级矩阵

| 轴 | 收益 | 成本 | 风险 | 建议时点 |
|---|---|---|---|---|
| A 死重量清理 | 中-高（-9k 行、-3 重依赖） | 0.5-1 天 | 低（逐项 grep 复核即可） | **立即** |
| E.1 补 279/282/283 快照用例 | 高（堵复发） | 半天 | 无 | **立即**（HEAD 已实测可过） |
| 复跑驱动验证 283 修复 | 高（roadmap 主战场解锁） | 半天 | 无 | **立即** |
| B 运行时 tag | 高（终止换坑式批次） | 3-5 天 | 中（ABI/运行期面广） | 下一个 roadmap 专项 |
| 还债计划①-⑤（roadmap 已定稿） | 高 | 4-5 天 | 中 | 与 B 交织执行 |
| C intern/FxHashMap | 中（编译提速） | 2-3 天分批 | 低-中 | B 之后（intern 与 tag 触点重叠） |
| D gen.rs 拆分 + 前端下沉 | 中 | 含在还债计划 | 中 | 随还债计划 |

## 8. 一句话总结

架构层面的优化不是"把 Rust 代码写得更快"，而是：**先用半天做完的减法（轴 A/E）止住熵增，再把资源集中到"值带类型标签"这一个根因（轴 B）上，让自举调试从"每批一个特判"回到"每批收敛一类问题"**——其余（性能、拆分）跟着已定稿的还债计划走即可。
