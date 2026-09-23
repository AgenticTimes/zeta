# Zeta 编译器架构优化方案（修订版 v2 —— 已按 `src/` 实测复核）

> 初版日期：2026-09-20（v1，对象错位，见下方核正声明）
> 修订日期：2026-09-20（v2，分析基座换为 `src/` Rust 编译器实实现）
> 主线：Python 语料编译器（roadmap 批次已至 274）；`zeta_src/` 自托管移植线（plan.z）自 5 月停摆，当前仅作 parse 回归语料。

---

## ⚠ 核正声明（v1 → v2）

v1 通篇引用 `zeta_src/` 下的 `.z` 文件作为「编译器架构」证据，**对象错位**：那些是停摆的自托管移植转写，不是编译 wufu 策略的真实现（`src/`，约 9.2 万行 Rust）。v1 中被实测推翻的断言：

| v1 断言 | 实测结论 |
|---|---|
| "type_map 是 `HashMap<u32, String>`，类型信息丢失" | **错**。真 MIR：`src/middle/mir/mir.rs:12` 为 `HashMap<u32, Type>`（结构化枚举）；String 类型问题只存在于 AST/resolver 参数注解层 |
| "param_types 一律 i64，f64 无区分" | **错**。`gen.rs` F64 出现 63 处；f64 参数签名、结构体字段位转换（批次 259）、`llvm.fabs/minnum` 内联均在用 |
| "Inkwell 与 IRGen 文本双轨、无 verifier" | **错**。`src/backend/codegen/` 只有 inkwell 结构化路径（codegen.rs/jit.rs/monomorphize.rs/runtime_decls_*），AOT 已含 verify；"双轨"是 zeta_src 移植未完成的形态 |
| "基线 python_style 260/263 · 语料 38/38" | **过时**。当前：官方 194/194 · python_style **273/276** · 语料 **39/39** |

v1 定性成立、已并入本版的结论：**无 Span**（`diagnostics.rs:132/145`、`lib.rs:793` 实测 `span: None`）、**Python 兼容侵入核心**（`gen.rs` 228 处 `py_` 特判）、**缺 MIR 优化 pass**（codegraph 证实 `optimize` 零调用）、**错误处理不统一**。

v1 的处置：作为「自托管移植线（plan.z）改进提案」存档于本文末尾附录，不再作为编译器主线的开工依据。

---

## 一、真实现状（`src/` 实测）

```
Python/.z 源码 → 缩进预处理 → nom parser → AST（类型注解为 String）
  → resolver（py_* 特判混入）→ MIR gen.rs（type_map: HashMap<u32, Type>，F64 已贯通）
  → codegen.rs（inkwell 结构化发射，AOT 含 verify）→ 链接（runtime_decls_registry）→ 执行
```

**已解决（v1 误判项，无需再做）**：MirType 贯穿、i64/f64 区分、IR verifier。

**确认存在的地基缺陷**：
1. **无 Span**：诊断系统 `span: None`，C1 只覆盖 W1002；所有链接/类型/codegen 错误无法定位源码行。
2. **py_ 特判债务**：`gen.rs` 228 处，且历次批次修复（含本文作者此前批次）仍在加深；分支间相互截走（批次 15-18 实证形状）。
3. **MIR 零优化**：`optimize` 函数存在但零调用者。
4. **诊断错误形态不统一**：print + 错误码与 Result 并存，panic 前值守卫靠逐点补丁。

**当前真正的阻塞（优先级高于一切地基改造）**：
- wufu-local 回测 4 个已定位运行期 bug：**批次 269**（清洗砍行）、**批次 270**（pd.concat sub-frame / map_get intern 句柄启发式）、**批次 271**（len(cached) 崩溃=帧 data 为 NULL，锁定 sh_515170 / sz_159509）、**批次 272/273/274** 收口后的残留验证
- 隐式字段合成缺口
- registry 双解析器（runtime_decls_registry vs 手工表）无契约测试
- t228 非确定性

---

## 二、任务拆分

> 规模：**S** ≤半天 · **M** 1-2 天 · **L** 3-5 天
> 纪律（沿用 roadmap 硬规则）：每任务完成即 commit + push 到 `agentic bootstrap`；合入前跑三基线（官方 194 · python_style 273/276 · 语料 39/39）+ `check_registry_symbols.sh`；大文件禁 `git checkout --`/`rm`，改 `mv` → `.trash/`。

### 阶段 0：wufu 运行期阻塞收口（最高优先，直接阻塞回测）

| ID | 任务 | 规模 | 依赖 | 验收标准 |
|----|------|------|------|----------|
| 0.1 | 批次 270/271 两个未修崩溃收口：map_get intern 句柄启发式替换为类型驱动（利用 MIR 现有 `Type`，非新增类型系统）；sub-frame data NULL 崩溃补前值守卫 + 结构化诊断 | L | - | wufu-local 回测跑过这两点；892→2 行清洗用例不回归；对崩溃输入 1000 例无 panic |
| 0.2 | 批次 269 清洗砍行残留：掩码计算路径全量回归（对已知 892 行语料逐段比对清洗前后行数） | M | 0.1 | 静默错值检测脚本入 `tools/`，输出行数差 ≠0 即失败 |
| 0.3 | 隐式字段合成缺口：盘点（哪些属性访问依赖合成、合成规则散落在哪），出清单 | S | - | 清单文档 + 每处 `py_` 归属标记 |
| 0.4 | t228 非确定性定位：固定 seed 下复现矩阵（同二进制重复执行 100 次 diff 输出） | M | - | 根因归类：map 迭代序 / HashMap 序 / 时间源 / 未初始化 |
| 0.5 | wufu-local 回测端到端：以上收口后完整跑一次，记录剩余阻塞项进 roadmap 新批次 | M | 0.1-0.4 | 回测产出可核对的收益曲线或明确的下一阻塞清单 |

### 阶段 1：registry 契约测试（防静默错值，小投入高杠杆）

| ID | 任务 | 规模 | 依赖 | 验收标准 |
|----|------|------|------|----------|
| 1.1 | 双解析器对拍：`runtime_decls_registry` 自动表 vs 手工/宿主表，导出符号集合 diff 测试（CI 每次构建跑） | M | - | 差异表非空即 fail，输出按模块分组的缺失符号 |
| 1.2 | 把 87 个历史未定义符号（批次 15 清单）固化为「必须命中注册表」用例矩阵（0/1/2/3 参 × 成员） | M | 1.1 | 批次 15-18 表格全部转成自动测试，链接期无裸名 |
| 1.3 | `check_registry_symbols.sh` 升级为契约测试入口并在 `run_all.sh` 内联 | S | 1.1 | 三基线脚本一条命令含 registry 契约 |

### 阶段 2：Span 分阶段挂载（地基改造第一优先，与 v1 阶段 1 合流）

| ID | 任务 | 规模 | 依赖 | 验收标准 |
|----|------|------|------|----------|
| 2.1 | `Span{file_id,start,end}` + `Spanned<T>`；nom token 层记录字节偏移 | S | - | token 流带 offset；parser 基准（benchmark_self_compile 等价物）耗时无回退 |
| 2.2 | AST 高频节点挂载（FuncDef/Call/Let/Assign），避免一次改全枚举 | M | 2.1 | `diagnostics.rs` 的 `span: None` 在四类错误路径变为 Some |
| 2.3 | 行列号计算（行表 + 二分）+ 诊断格式化器 `file:line:col` | S | 2.1 | 39 文件语料任意偏移 O(log n) 查行号 |
| 2.4 | W1002/E1002 改带精确行列（替代「丢多少行+起始文本」反推） | S | 2.2, 2.3 | 故意截断语句报错含起始位置 |
| 2.5 | 链接期未定义符号错误带调用点行号（直接消解 1.2 之前的手工探针需求） | M | 2.2, 2.3 | 复现裸名场景，报错指出调用行号 |
| 2.6 | MonoKey 排查：确认结构相等/哈希不受 span 字段污染（span 用 u32 紧凑编码，MonoKey 排除 span） | M | 2.2 | 单态化缓存命中率前后一致 |
| 2.7 | （后续）MIR stmt 透传 span，为 codegen 期诊断做准备 | M | 2.2 | 与 0.1 类崩溃诊断联动 |

### 阶段 3：py_ 特判债务收敛（大工程，必须在阶段 0-1 之后、且需设计评审）

| ID | 任务 | 规模 | 依赖 | 验收标准 |
|----|------|------|------|----------|
| 3.1 | 盘点：228 处 `py_` 分类（import 解析残留 / 成员调用分派 / 内建类型伪装 / 清洗 workaround），出归类文档 | S | - | 每处归属四类之一，标调用频次与截走风险 |
| 3.2 | 分支截走检测：对主 Call arm 建决策表测试（给定调用形态 → 期望命中 arm），先测不改 | M | 2.5 | 批次 15-18 的「应该一致却不一致」对能自动检出 |
| 3.3 | 冻结规则：新增 py 特性一律走归类文档中的指定挂载点，禁止新 Call arm 内联特判（写入 AGENTS/流程） | S | 3.1 | 流程文档 + review checklist 项 |
| 3.4 | 按类别逐个抽为独立 desugar/分派 pass（一类一 PR，273/276 基线不回退为硬门槛） | XL | 3.1-3.3 | 每类迁移后 py_ 计数净下降、三基线零回退 |
| 3.5 | python_style 缺口 3 例（273/276）在收敛中重评 | M | 3.4 | 276/276 或明确降级记录 |

**注**：v1 的「Python 兼容整体剥离为前端 desugar pass」方向仍然成立，但在 `src/` 落地即本阶段 3.4；因体量 XL 且与主线争抢 gen.rs，不设时间表，先做 3.1-3.3 止血。

### 阶段 4：MIR 优化 pass 接通（护栏先行）

| ID | 任务 | 规模 | 依赖 | 验收标准 |
|----|------|------|------|----------|
| 4.1 | golden snapshot 回归：语料选 10 个代表文件锁定 LLVM IR 输出（与既有 `mir_diff` 互补，形成 mir/ir 两级） | M | - | `tools/golden_ir/` diff 即回归报警 |
| 4.2 | 接通现有 `optimize`（零调用者）：先空跑（pass 列表为恒等），确认管线插入点与 verify 不冲突 | S | 4.1 | 恒等 pass 下 golden 逐字节等价 |
| 4.3 | 常量传播 pass：复用 MIR 已有 `Type` 与 CTFE 常量，直发字面量 | M | 4.2 | wufu 热点函数 IR 行数下降；结果与 4.1 一致 |
| 4.4 | DCE：未读取临时/死分支删除 | M | 4.3 | 同上护栏 |
| 4.5 | 小函数内联（阈值 stmt 数 ≤ N） | L | 4.4 | golden + 三基线全绿；回测数值不变 |

### 阶段 5：错误处理与 panic 审计（持续项）

| ID | 任务 | 规模 | 依赖 | 验收标准 |
|----|------|------|------|----------|
| 5.1 | `span: None` 残留路径审计：diagnostics.rs/lib.rs 全部构造点归类（哪些因无 span 数据、哪些是漏传） | S | - | 归类清单 + 逐批消除计划 |
| 5.2 | 统一诊断结构（错误码 + span + 建议），收敛 print+错误码路径 | L | 2.3 | 编译器对用户可见错误全部走同一结构 |
| 5.3 | panic 前值守卫系统化：`is_char_boundary` 类守卫 + NULL data 守卫固化为 lint/检查脚本（承接 0.1） | M | 0.1 | 语料 + fuzz 输入无 panic；新增守卫有对应回归测试 |

---

## 三、执行顺序与依赖

```
阶段0 (wufu收口) ──> 阶段1 (registry契约) ──┐
                    阶段2 (Span)  ←─ 与阶段1并行，2.5 反哺 1.2 的可读性
                              ├─> 阶段3.1-3.3 (止血/冻结，随时可做)
阶段4.1 (golden护栏) ──> 4.2-4.5 (optimize)   [须在 0.1 之后，防启发式改动无护栏]
阶段5 持续穿插
```

**推荐节奏**：`0 → (1 ∥ 2.1-2.5) → 4.1-4.2 → 3.1-3.3 → 2.6-2.7 / 4.3-4.5 → 3.4（择期）`

理由：
- 阶段 0 是唯一直接阻塞回测的工作，任何地基改造不得抢占；
- 阶段 1 registry 契约把批次 15-18 的手工排查成果固化为自动测试，防止 0.x/3.x 改动引起静默回退；
- 阶段 3.4（desugar 大迁移）体量 XL 且与批次修 bug 争抢 gen.rs 热点文件，默认只做 3.1-3.3 止血，全量迁移等回测阻塞清空后单独立项评审。

---

## 四、风险与回退

| 风险 | 缓解 | 回退 |
|------|------|------|
| 0.1 句柄启发式替换改变既有通过用例 | 改动前后对 39 语料 + 273 用例全跑；每处替换单独 commit | revert 单任务粒度 |
| Span 挂载使 AST clone 成本上升、污染 MonoKey 结构相等 | 2.6 专项验证；span u32 紧凑编码；MonoKey 排除 span | 只挂四类高频节点（2.2 范围封顶） |
| py_ 冻结规则（3.3）与紧急批次修 bug 冲突 | 例外走归类文档登记制，事后补挂载点 | 例外计数周报，超阈值即停下清理 |
| optimize 接通改变数值结果（回测收益曲线漂移） | 4.1 golden + 4.2 恒等空跑先行；每 pass 独立开关 env 门控 | 关 pass 即回原行为 |
| 与主线批次争抢文件 | 阶段 2 只动 parser/diagnostics，与 gen.rs 冲突面小；3.x 排批次间隙 | — |

---

## 五、函数级实施计划（三条合并价值线，2026-09-20 实测取证）

> 对应阶段 1/2/4 的函数级细化；所有 file:line 均经 grep 实测核对。
> 前提共识：批次 269-274 的 wufu 运行期收口（阶段 0）优先于本节，不在此展开。

### 取证的三个降本发现

1. **`src/frontend/parser/location.rs` 已有完整 `Position{offset,line,column}` / `Span` / `span_from()`，但全仓零使用者** —— Span 不是从零建，是「激活死代码 + 接线」。
2. **`src/middle/optimization.rs` 已实现 6 个 pass**（`dead_code_elimination:21`、`constant_folding:233`、`common_subexpression_elimination:285`、`strength_reduction:469`、`algebraic_simplification:483`、`optimize:490` O0-O3 管线）—— 缺的只是调用点：全仓唯一引用是 `compiler_config.rs:10` 导入 `OptLevel`（配置存了 level，管线从未跑）。
3. **registry 契约测试的数据源现成**：`src/middle/pylib.rs:334 all_externs()` 返回 registry.txt 全量 extern 表，可直接与两张生成表对拍。

### 线 A：接通 MIR optimize pass（最小线，先做）

接线点：`src/lib.rs:93 compile_and_run_zeta` → `src/lib.rs:150 codegen.gen_mirs(&mirs)`。

| 步 | 函数/位置 | 改动 | 测试 | 验收 |
|----|-----------|------|------|------|
| A1 | `src/lib.rs` `compile_and_run_zeta`（及 AOT 对应路径，同型 `gen_mirs` 调用点先 `grep -rn "gen_mirs" src` 全量定位） | 在 `gen_mirs(&mirs)` 前插入：`for mir in &mut mirs { optimization::optimize(mir, config.opt_level) }`；level 从 `compiler_config.rs` 现有 `OptLevel` 读取，默认 O0（=现状恒等） | 新单测 `optimization::tests::identity_o0_mir_unchanged`：构造样例 Mir，O0 前后 `format!("{:?}")` 逐字节等 | env `ZETA_OPT_LEVEL` 覆盖生效；三基线 273/276、194/194、39/39 零回退 |
| A2 | `dead_code_elimination`（optimization.rs:21）+ `mark_expr_used`（:206） | 不改代码，先补测试钉行为 | 单测：①被 Return 引用的 expr 不被删；②VoidCall 参数的 expr 不被删；③副作用 stmt（Call/VoidCall）永不算死 | 3 用例绿；**若发现按副作用判活有漏洞（如 DictInsert），先修再开 O1** |
| A3 | `constant_folding`（:233） | 折叠只在同型字面量上做（i64+i64），禁跨 f64/i64（防重蹈批次 259 位转换类错值） | 单测：`1+2→3`；`1.5+2` 不折叠；溢出走 wrapping 并记录 | 绿；对照 892 行时间戳用例（批次 273 回归钉）不漂移 |
| A4 | `optimize`（:490）O1 档 | O1 = 恒等 + const_fold + DCE（CSE/strength_reduction 留 O2+，逐个解锁） | golden：10 个代表文件的 MIR dump 前后 `mir_diff` | golden diff 仅出现在预期位置；数值结果 0 漂移 |
| A5 | 数值一致性总检 | 对 wufu-local 回测输入跑 O0 vs O1 | 收益曲线逐值 diff 脚本 | 完全一致才可默认开；否则维持 O0 默认并记录分歧样本 |

规模：A1-A3 各 S，A4 M，A5 M。回退：level 默认 O0，零行为变更。

### 线 B：registry 契约测试（防静默错值/裸名，小投入高杠杆）

三方数据源（实测）：
- 真相源：`pylib/registry.txt` → `pylib.rs:94 parse_registry()` → `:334 all_externs()`
- AOT 声明表：`src/backend/codegen/runtime_decls_registry.rs`（@generated by `tools/gen_from_registry.py --emit`）
- JIT 映射表：`src/backend/codegen/jit_mappings_gen.rs`（@generated by `--emit-jit`，源 `pylib/jit_mappings.txt`）

| 步 | 函数/位置 | 改动 | 测试 | 验收 |
|----|-----------|------|------|------|
| B1 | 新文件 `tests/registry_contract.rs` | 测试①「registry→声明」：`all_externs()` 的每个符号名 ∈ runtime_decls_registry.rs 中 `add_function("…")` 集（正则解析源文本即可，无需重构生成器） | 缺失集合 assert 为空；失败输出按模块分组 | 批次 15 的 87 个裸名场景变为编译期检出而非 ld 期 |
| B2 | 同文件 | 测试②「jit_mappings 覆盖」+ **双真相源显式化**：registry.txt 与 jit_mappings.txt 符号差集非空则 fail 并打印（首跑先量差集规模） | 差集为空或白名单文件 | 双解析器漂移从此有测试 |
| B3 | 同文件 | 测试③「生成表新鲜度」：重跑 `gen_from_registry.py --emit / --emit-jit` 后 `git diff --exit-code` | 非交互，CI 可跑 | 手改生成文件/忘再生成 = CI 红 |
| B4 | 同文件 | 测试④「宿主符号存在性」：`pub extern "C" fn py_*`/`host_*` 定义集 ⊇ 声明集（regex 扫 `src/runtime/`） | 未定义即报出符号名 | 链接期 undefined 提前到编译期 |
| B5 | `tools/run_all.sh` | 并入 `cargo test --test registry_contract` | — | 一条命令含契约层 |

规模：B1/B2 各 M，B3/B5 各 S，B4 M。回退：纯新增测试零行为变更；B2 首跑若大量差集，白名单冻结后逐批收敛。

### 线 C：Span 分阶段接线（激活 location.rs，诊断全线受益）

现状（实测）：`location.rs` 死代码；`diagnostics.rs` 机器齐全 —— `SourceSpan`(:35)、`span: Option<SourceSpan>`(:119)、`with_span`(:153)、`extract_context`(:179)、构造器 `parse_error/type_error/undefined_variable`(:455-467) 已收 span；但 `diagnostics.rs:132/145`、`lib.rs:793` 实际全传 `span: None`。AST（69 变体）无 span 字段。

| 步 | 函数/位置 | 改动 | 测试 | 验收 |
|----|-----------|------|------|------|
| C1 | `indent.rs` + `top_level.rs:1789 remaining_byte_offset`（已有偏移计算） | parse 错误带**字节偏移**（不动 AST）；`main.rs:108` W1002 改经行表换算 `file:line:col` | offset→行号矩阵单测；截断语句集成测试 | W1002/E1002 含精确行列 |
| C2 | 新纯函数 `offset_to_location(source, byte) -> Position`（放 location.rs，终结死代码） | 行表 + 二分；UTF-8 列按 char 计数（配 `is_char_boundary` 守卫，对齐批次 271 panic 纪律） | 边界：offset==len、多字节行中、CRLF | O(log n)；39 语料全文件无 panic |
| C3 | `ast.rs` 仅 4 类变体加 `span: Span`：FuncDef/Call/Let/Assign（parser 构造点用 `span_from(start_pos)` 填） | 匹配处 `..` 兜底；**MonoKey/PartialEq 必须排除 span**（先 `grep -n derive ast.rs` 确认现状） | ①二次 parse span 稳定；②单态化命中率前后一致（复用 A5） | 4 类节点真实 span；缓存零变化 |
| C4 | `mir.rs` `MirStmt` 加 `span: Option<Span>`（gen.rs lower 拷入）；`codegen.rs:2286` unresolved-declare-extern 处：非白名单前缀 → 带调用点行号诊断 | 白名单 = B1 声明集（两线合流点） | 拼错函数名 → 诊断含 `file:line:col` + 上下文波形 | 「裸名到 ld 才暴露」变编译期定位 |
| C5 | `lib.rs:793` 等 `span: None` 构造点逐批改 Some | 归类：数据可达未传=bug；上游无数据=登记 | 快照测试 | `grep -c "span: None"` 单调下降 |

规模：C1/C2 S，C3/C4 L，C5 持续。回退：C3 若 PartialEq 纠缠过深 → 降级为旁路 `SpanTable<node_id→Span>`（parse 期填充），C4 查表。

### 三线合流顺序

```
B1 ──> C4（白名单复用声明集）
A2,A3（护栏）──> A4（O1）──> A5（数值一致）──> 默认开 O1
C1,C2（零 AST 侵入）──> C3 ──> C4 ──> C5
```

开工序：**B1-B3（纯新增先行）→ A1-A3（默认 O0 合入）→ C1-C2 → C3 → A4/A5 → C4 → C5 穿插**。每步 commit + push `agentic bootstrap`；合入前三基线 + `check_registry_symbols.sh`；数值路径（A4/A5、C3 缓存）必跑 wufu-local 回测对照。

---

## 附录 A：v1 文档定位说明

v1 全部任务（MirType 贯穿、Runtime 三层拆分、.z 侧 IRGen verifier、自托管管线 crate 拆分等）**仅适用于 `zeta_src/` 自托管移植线（plan.z）**。该线自 5 月停摆、当前仅作 parse 语料，如重启再按 v1 附录原稿评估；在重启之前，不得据此对 `src/` 开工。

v1 三项与主线合流的内容已并入本版：
- v1 阶段 1.1-1.4（Span）→ 本版阶段 2
- v1 registry 相关（v1 未显式列出，核对意见补入）→ 本版阶段 1
- v1 5.2（golden snapshot）→ 本版阶段 4.1（与 `mir_diff` 互补为两级）

## 附录 B：v1 原文核对明细（存档）

- v1 引用证据本身属实但对象错误：`zeta_src/frontend/ast.z:34`、`zeta_src/middle/mir/gen.z:12`、`zeta_src/backend/codegen/ir_gen.z:40`、`zeta_src/main.z:100` 逐行核对无误 —— 它们是移植转写的形态，不代表编译器现状。
- v1 遗漏清单（本版已补入阶段 0）：批次 269-272 四个运行期 bug、隐式字段合成、registry 双解析器契约、t228 非确定性；v1 甚至引用了 270/271 作佐证却未将其列为待修任务——此为本版把阶段 0 置于一切之前的直接原因。
