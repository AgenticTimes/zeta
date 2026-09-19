# advice.md — Zeta 优化实施方案与任务拆分

> 日期：2026-09-19　|　配套分析：[docs/ARCHITECTURE-REVIEW-2026-09.md](docs/ARCHITECTURE-REVIEW-2026-09.md)（问题证据与业界调研在彼处，本文只讲"怎么改"）
>
> 行号引用为撰写时快照，执行时以实际代码为准。

---

## 〇、进度对账（2026-09-19 更新，批次一百四十三后）

> roadmap 已推进至批次一百四十三；本节为 advice 的对账与修订，后续章节保留原文供追溯。

### 已落地（全部见 roadmap 批次一百二十九—一百四十二）

| advice 任务 | 状态 | 落点 |
|---|---|---|
| Q1–Q5 速赢包 | ✅ | 批次 129（IR dump 改 flag、PROBE 清零、死代码第一批、nm 核对脚本、run_all.sh） |
| 任务 A 符号注册表（A1/A2/A2b/A3/A4/A5） | ✅ 全套 | 批次 130–135：`registry.txt` + `runtime_core.txt` + `runtime_aliases.txt` + `jit_mappings.txt` 四份数据 → `gen_from_registry.py` 单一生成器 → `runtime_decls_*.rs` / `aliases.inc.c` / `jit_mappings_gen.rs`；A4 影子模式 mismatch=0 后切换；`codegen::new` 手写声明 240→146；`jit.rs` 净减 ~500 行 |
| L 基线进 CI | ✅ | 批次 136：`baselines` job（生成物 `git diff --exit-code` 防过期 + nm `--check` + `run_all.sh`，artifact 上传） |
| M 运行时构建脚本 | ✅ | 批次 132：`tools/build_runtime.sh --gen` 一键重生全部生成物 |
| D 桩响亮化（机制） | ✅ | 批次 137：`py_stub_abort` + `ZETA_LENIENT_STUBS` + `--list-stubs` + t253（当前 registry 层 stub 仅 1 个） |
| B1 强转矩阵 | ✅ | 批次 138：白名单放行 / 矩阵外告警（每模块 8 条封顶）/ `--strict-abi` 硬失败 / t254 |
| A 收尾 + B2 libc 碰撞 | ✅ | 批次 139：`codegen::new` 手写 `py_*`→0；waterfall libc 名表告警 / t255 |
| D4 库层假值桩 | ✅ | 批次 140：`# stub:` 扫 pylib；loud→`py_stub_abort`；soft 保留；`--list-stubs` 20=1+19 / t256 |
| B3 `PyDynamic` + `--report-untyped` | ✅ | 批次 141：`Type::PyDynamic`/`dyn`；未标注形参可升级；`t257`；E4 未合入 |
| C1 行号 + C2 同步恢复 | ✅ | 批次 142：W1002 `path:line:`；C2 默认关 / `ZETA_PARSE_RECOVER=1`；t258/t259 |
| B4 dyn → 唯一 W 分发 | ✅ | 批次 143：`method_by_unique_name` + PyDynamic-only；误 `checkout` 后按规格恢复 pre_cond/for-in-str；`handle_tag` 补全 W handle |

**基线现值**：python_style 焦点 t192/t205/t260 ✅（t206 columns/`in` 另案）；官方/全量 python_style **待重跑确认**；语料 **38/38** parse、`jq_wufu_local` 未定义符号 **~65**。

### 对账发现的四个新问题（本版新增任务的来源）

1. ~~**D 有覆盖缺口**~~：**D4 ✅**（批次 140）— loud 已 abort；soft 假值仍标记在 `--list-stubs`（`soft:` 前缀），待真实现。
2. **gen.rs 分发仍硬编码**：`np.where` 等 MIR 拦截散在 `lower_expr`；registry 已是数据源，方法分发可同样数据化（G3 的前置，且是"名字瀑布"在分发侧的残留）。
3. ~~**P0 里唯一完全未动的正确性项是 C**~~：**C1+C2 ✅**（批次 142）— 行号已通；恢复默认关（误恢复曾砸官方/语料），待观察后翻默认。
4. **P1 全线未动**：`monomorphize` 自映射仍在（resolver.rs:2305 附近）、优化器仍零调用（无 `ZETA_ENABLE_MIR_OPTS`）、`gen.rs` 11003 行（还略涨了）。

---

## 0. 总原则（所有任务共同遵守的护栏）

1. **三套基线是唯一验收标准**：官方套件（compile-only，当前 194/194 口径）、python_style（当前 249 例）、语料基线（jq_wufu_local 38 文件 parse 数 + 未定义符号去重计数）。每个任务开始前先跑基线把数字写进 roadmap，结束后对照。
2. **行为变更一律挂开关**：env 或 CLI flag，默认值保证基线不回退；观察 2–3 个批次无碍后再翻默认值。这延续了 `ZETA_STRICT_PARSE` 的既有做法。
3. **先修"错"再修"慢"**：静默错值/静默丢失（P0）→ 架构债（P1）→ 编译性能与工程效率（P2）。运行期性能不在本计划内（已托付 LLVM O3，路线正确）。
4. **还债与功能交替**：按 roadmap 既有节奏，每完成 1 个还债任务可穿插 2–3 个语料功能批次。近期批次的试错成本（如 `datetime.timedelta` 花 5 个批次定位发射路径）正是欠债的利息，还债任务的收益会直接体现在功能批次的推进速度上。
5. **先度量后动手**：P1-E（类型系统）这类"二选一"决策，先用 1 个批次拿数据再拍板。
6. **结构核查用 codegraph**（仓库有 `.codegraph` 索引，CLI：`callers`/`callees`/`impact`/`affected`/`node`）：还债批次验收除三基线外加**结构验收**——"优化器零调用"用 `callers optimize` 确认、拆 `gen.rs`（F1）前先 `impact lower_expr` + `affected` 定位受影响测试；grep 只作文本证据，调用关系以图为准。

---

## 1. 第一周速赢包（✅ 已全部落地：批次一百二十九 + 一百三十）

| # | 任务 | 改动点 | 预估 | 验收 |
|---|---|---|---|---|
| Q1 | IR dump 改 flag | `main.rs:390` 无条件 `print_to_stderr` → 仅 `--emit-llvm` 或 `ZETA_DUMP_IR=1` 时输出 | 30 min | 三基线绿；正常编译 stderr 干净 |
| Q2 | nm 符号核对脚本 | 新建 `tools/check_registry_symbols.sh`：从 `pylib/registry.txt` 提取全部 F/W/X 符号，`nm` 核对存在于 `zeta_runtime_c.o`/`tokio_runtime.o` | 2 h | **抓出 `py_asdict_unexpanded` 等幽灵符号**并修正条目（registry.txt:208）；脚本零输出 = 通过 |
| Q3 | 死代码删除第一批 | 只删"零引用且无测试依赖"的：`src/middle/mir/optimized_mir.rs`、`optimized_gen.rs`、`ctfe/evaluator_complete.rs`、`resolver/module_resolver.rs.backup`、根目录 `*.o.tmp` 两个 0 字节残留、`tokio_runtime_old.o` | 1 h | `cargo build/test` 绿；基线绿 |
| Q4 | 统一基线脚本 | 新建 `tools/run_all.sh`：官方（拷 /tmp 编译计退出码）+ python_style（复用现有 run.sh）+ `tools/corpus_baseline.py`，输出统一 JSON 汇总（作为单一事实源，消除 158/160/230/247/249 口径漂移） | 0.5 天 | 一条命令产出三套数字；README/validate.md 引用它 |
| Q5 | 调试探针清理 | 删除 codegen.rs:1387/2283/3125 等处 `eprintln!("PROBE ...")` | 30 min | grep "PROBE" 零命中；基线绿 |

**合并收益**：一个批次做完，此后每个批次都有了防回归护栏（Q4）和干净的工作台。

---

## 2. P0 —— 正确性任务（按优先级执行）

### 任务 A：符号/ABI 单一注册表（✅ 已全部落地：批次一百三十—一百三十五；A 收尾 ✅ 批次一百三十九）

**问题回顾**：新增一个运行时函数要手工同步 4–5 处——codegen `new()` 的 240 个 `add_function` 声明（codegen.rs:71 起）、gen.rs 分发、C 实现、必要时 stub 里 66 条 `__asm__.set` 别名（tokio_runtime_stub.c:228–294）、JIT 模式 jit.rs 的 ~90 个 `add_global_mapping`。`get_or_declare_function`（codegen.rs:2415–2727）是 ~310 行的名字解析瀑布。

**核心思路：registry.txt 已经是数据源（F/W/X 行已带 `args=/ret=`），把它变成真正的单一事实源，其余全部生成。**

**方案**：

1. **扩展 registry.txt 条目字段**（保持现有格式兼容，新增可选键）：

   ```
   F pathlib Path py_path_new args=i64 ret=handle:PyPath decl=1
   W   PyPath exists py_path_exists args=i64 ret=i64 decl=1
   X   py_asdict_unexpanded args=i64 ret=i64 stub=1        # stub=1 = 未实现
   ```

   - `decl=0`：codegen 不自动声明（内部函数）
   - `stub=1`：未实现，配合任务 D 响亮化
   - `alias-of=<sym>`：取代 C 文件里的 `.set` 别名（记录"LLVM `.N` 改名追认"关系为数据）

2. **写生成器 `tools/gen_from_registry.py`**（纯脚本即可，仓库无 build.rs，保持零构建开销）：
   - 输出 ①：`src/backend/codegen/runtime_decls.rs`（`declare_runtime_fns(&Module)`，替换手写 240 条）
   - 输出 ②：`runtime/aliases.inc.c`（`__asm__.set` 别名块，替换手写 66 条，`#include` 进 stub）
   - 输出 ③：`--check` 模式 = Q2 的 nm 核对（生成器与核对器同源）
   - CI 中跑 `gen_from_registry.py --check`：重新生成 + `git diff --exit-code`，保证生成物不过期

3. **改造 `get_or_declare_function`**：表驱动优先，瀑布降级为兜底——
   查找顺序：① 注册表精确命中（含 W 方法表）→ ② 已注册用户函数/特化 mangled 名 → ③ 现有启发式瀑布（**每落到 ③ 都发一次编译期告警**，语料上告警数应为 0，不为 0 即暴露漏登记）。

4. **影子模式迁移**（风险控制）：先让表驱动与旧瀑布并行运行一个批次，diff 两者结果，确认一致后再删瀑布中的对应分支。

**任务拆分**：

| # | 子任务 | 预估 | 验收 |
|---|---|---|---|
| A1 | registry 条目字段扩展 + pylib.rs 解析（Q2 脚本并入 `--check`） | 0.5 天 | 幽灵符号清零；pylib.rs 单测覆盖新字段 |
| A2 | 生成器产出 runtime_decls.rs，codegen `new()` 收敛为一次调用 | 1 天 | codegen.rs 净减 ~240 行；三基线绿 |
| A3 | 别名块数据化生成 aliases.inc.c，stub 重建 | 0.5 天 | stub 重编命令进 tools/ 脚本；基线绿 |
| A4 | get_or_declare_function 表驱动化（影子模式 → 切换） | 1–2 天 | 语料 undef 计数不升；编译期"落到兜底"告警 = 0 |
| A5 | jit.rs 映射从注册表生成 | 0.5 天 | JIT smoke 测试绿 |

---

### 任务 B：静默强转与类型伪装治理（B1 ✅ 批次 138；B2 ✅ 批次 139；B3 ✅ 批次 141；B4 ✅ 批次 143）

**问题回顾**：`coerce_call_args`（codegen.rs:6208 起）对任何参数不匹配自动 zext/truncate/sitofp/fptosi（validate.md 记录的 `abs(-2.5)→nan` 即此类）；方法名撞 libc（`isalnum/isspace/strftime`）静默链接到 libc；**未标注形参恒按 i64** 是 roadmap 反复撞墙的总根因（批次 122/123 的 `_condition`、`getattr`、`xs[0]` 返 0、`in` 恒 0 等）。

**方案**：

1. **强转白名单矩阵**（`--strict-abi` 下矩阵外报错，默认告警+计数，不破坏基线）：

   | 源 → 目标 | 处理 |
   |---|---|
   | i64 → f64 | sitofp，允许 |
   | i64 → i32 及以下 | 窄化，告警 |
   | f64 → i64 | bitcast 仅保留在既有对称点，其余告警 |
   | f64 ↔ 指针句柄 | **禁止** |
   | 同型 | 直通 |

2. **libc 碰撞检查**：codegen 内置一张常见 libc 符号表（isalnum/isspace/strftime/abs/rand…），当 extern 兜底声明的名字命中该表时告警"将与 libc 链接冲突，建议改名或显式注册"。

3. **未标注参数显式化**（杠杆最大的一步）：新增 `Type::PyDynamic`（或字符串 `"dyn"`），未标注形参/返回解析为 dyn 而非 i64；**ABI 层 dyn 仍映射 i64/f64 双槽不动**（避免动运行时），但类型层可追踪"这是猜的"。第一步只做两件事：
   - 编译期输出"未标注参数清单"（`--report-untyped`），对照语料逐个补注解——这直接消灭 roadmap 里"四处找转换点"的试错模式；
   - `py_member_call` 分发时 dyn 接收者不再按 i64 猜，而是查 W 方法表。
   - 后续批次再逐域收窄 dyn（先语料策略层，再 pylib）。

**任务拆分**：

| # | 子任务 | 依赖 | 预估 | 验收 |
|---|---|---|---|---|
| B1 | 强转矩阵 + `--strict-abi` + 告警计数 | A1（用注册表签名比对） | 1 天 | 三基线绿；语料告警清单产出并归档 |
| B2 | libc 碰撞检查 | 无 | 0.5 天 | 对 `isalnum` 类构造回归用例（t2xx）验证告警出现 |
| B3 | `Type::PyDynamic` 引入 + `--report-untyped` | 无 | 2–3 天 | 语料补注解后 undef 计数显著下降（预期消掉 `_zip`/`_isinstance`/`_hasattr`/`_setattr`/`_clear` 一类裸名批次） |
| B4 | dyn 接收者走 W 方法表分发 | B3 | 1–2 天 | `rs.next()`、`query_*` 类语料焦点用例转 T |

---

### 任务 C：解析器错误恢复 + 位置信息

**问题回顾**：`many0` 在第一个失败项静默截断（top_level.rs:1553–1575）；官方套件 11 个文件有未解析尾巴（一个丢 757 行），`main.rs:71-104` 自述无法报行号；AST 无 span，诊断基础设施（diagnostics.rs）全部 `span: None`。

**方案（分三档，性价比递减，前两档必做）**：

1. **C1 行号映射（半天）**：`indent.rs` 文本预处理时已有逐行对应关系——维护 `byte_offset → 源行号` 映射表（注意预处理会折叠续行/单行复合体，需映射回原行），`ensure_fully_parsed` 用剩余输入的 offset 查表，把"尾部 N 行没解析"告警升级为 `file:line:` 精确告警。
2. **C2 顶层同步恢复（1–2 天）**：`parse_zeta_impl` 循环改造：

   ```
   loop {
       match parse_top_level_item(input) {
           Ok(rest, item) => items.push(item),
           Err => {
               let (rest, err) = skip_to_sync(input, SYNC_KEYWORDS); // def/class/import/from/@/try/# 行首
               errors.push(err /* 带 C1 的行号 */);
               if rest.is_empty() || rest.len() == input.len() { break; }  // 防死循环
               input = rest;
           }
       }
   }
   ```

   错误收集进 diagnostics，默认**不致命**（行为同今天，但恢复后能解析出更多项、错误有行号）；`ZETA_STRICT_PARSE=1` 时致命。预期官方套件 11 个文件的"尾巴丢失"大幅减少（错误项之后的内容不再陪葬——批次 125 `del` 多目标炸整文件的根因链即属此类）。
3. **C3 函数级 SpanTable（1 天，可选先行）**：不追求给 64 变体 AST 全部加 span（churn 太大），先建 `HashMap<函数名, (起始行, 结束行)>`（indent.rs 预处理时天然知道块边界），resolver/typecheck 报错时按当前函数名查表填充 span。这一步就能让类型错误的报错从 `(1,1)` 变成 `file:line`，覆盖 80% 的诊断价值。全量节点 span 留给远期。

**验收**：三基线绿；官方套件尾丢告警数从 11 下降；语料 parse 数 ≥ 37/38 不回退；新增 t2xx 用例：含一个语法错误的文件能恢复出后续函数并给出行号。

---

### 任务 D：桩响亮化（机制 ✅ 137；D4 ✅ 批次一百四十）

**问题回顾**：与项目 fail-loud 红线冲突的假值桩——`numpy.isnan` 恒 False、`vstack` 返回第一块、`isin` 全 1、`dropna/fillna/astype` 恒等、`date_range` 空列表——链接通过但语义为假。`.ouroboros/goal.md` 已定了 `py_asdict_unexpanded` 的响亮失败模式，本任务将其推广为机制。

**方案**：

1. registry 条目 `stub=1`（任务 A 引入的字段）+ `pylib/*.z` 内对应函数体改为调用 `py_stub_abort("<sym>: not implemented")`（新增运行时函数，打印符号名后 abort——模仿现有 `py_builtin_next` 模式）。
2. **过渡期双模式**：`ZETA_STRICT_STUBS=1`（默认）→ abort；`ZETA_LENIENT_STUBS=1` → 运行期 stderr **每符号只告警一次** + 返回现值。语料若真触发某个桩，会在 CI 日志里显形，而不是静默错值。
3. `zetac --list-stubs`：编译期打印桩清单，方便在 roadmap 里跟踪"桩 → 真实现"的消化进度。

**任务拆分**：D1 registry/`pylib.z` 标记迁移（0.5 天）；D2 `py_stub_abort` 运行时 + 双模式（0.5 天）；D3 `--list-stubs`（0.5 天）。验收：三基线绿；`--list-stubs` 输出与 registry stub 数一致；t2xx 用例验证 strict 模式下桩调用 abort 且带符号名。

**✅ D1–D3 已落地（批次一百三十七）**。新增：

**D4 库层假值桩迁移（对账新增，1–1.5 天）**：`pylib/*.z` 内的假值桩不在 registry，`--list-stubs` 看不见它们。方案：① 约定 `// stub:` 注释标记 `.z` 内桩函数，`gen_from_registry.py --list` 增扫 pylib 并与 `--list-stubs` 合并输出；② 逐个迁移 `py_stub_abort`（`isin/dropna/astype/vstack/date_range` 等）——先核对语料是否触发，触发者走 `ZETA_LENIENT_STUBS` 或优先真实现。验收：`--list-stubs` 输出 = registry stub + pylib 桩总数；基线绿。

---

## 3. P1 —— 架构还债任务

### 任务 E：类型系统双轨清账（先度量，再二选一）

**现状**：`types/` 约 5,400 行 HM 基础设施陪跑；实际把关的是旧 `check_node`；`typecheck_new.rs` 错误静默吞掉。

**方案**：

1. **E1 度量批次（1 天，必做）**：给 `typecheck_unified` 加计数器——每节点标记 `Inferred / FellBack / Skipped`，编译结束打印覆盖率汇总。拿到数据后决策：
   - **覆盖率可用（建议阈值：语料上 ≥ 60% 节点 Inferred）→ 接通路线**：E2 把 FellBack/Skipped 的错误从静默改为 diagnostics 输出（默认 warning）；E3 逐步让新系统对"参数个数/返回类型"先变成硬检查，旧 `check_node` 退役。
   - **覆盖率不可用 → 删除路线**：删 `new_resolver.rs`/`typecheck_new.rs`/`unified_typecheck.rs`/`type_cache.rs`（约 3,200 行），`types/` 只保留被字符串解析用到的最小部分，HM 部分归档。**不要维持现状**——双轨的维护成本已经体现在近期批次的返工里。
2. **E4 FuncDef 签名结构化（独立可做）**：`params: Vec<(String, String)>` 第二个 String 改为解析好的 `Type`，消除各处 `Type::from_string` 重复解析。此项与 B3（PyDynamic）同一改动面，建议合批。

### 任务 F：拆解巨石（纯机械，风险低，可持续穿插）

**方法**：不做逻辑改写，只"搬家"——把 `lower_expr`（gen.rs:2627–10596，约 7,970 行）的 match 分支按关注点抽成独立函数/文件，每抽一个关注点跑一次基线。建议顺序（从耦合最小开始）：

| 批次 | 抽出内容 | 预估 |
|---|---|---|
| F1 | argparse 处理、`eval_env_read`/env 折叠（gen.rs:14–113）、f-string 格式化 | 1–2 天 |
| F2 | 闭包/自由变量捕获（含批次 123 的 `collect_free_vars` 逻辑）、import mangling（`load_user_python_module` 相关分发） | 2 天 |
| F3 | 容器方法分发（str/list/map 的 W 表调用）、`py_member_call` 及其别名链 | 2 天 |
| F4 | `gen_stmt`（codegen.rs:3084–5255，2,172 行）拆 with/try/for-while/match 四块 | 2 天 |

同时做 **F-R**：`Resolver` 30 个字段中 17 个 Python `RefCell<HashMap>` 收进 `PyContext` 子结构（1 天）——这一步直接为任务 J 铺路。

### 任务 G：MIR 字符串化治理（分三段，前两段便宜）

1. **G1 intern**：`op/func/字段名` 的 `String` → `Arc<str>`（或引入 interner `SymbolId(u32)`），消除 clone 热点（gen.rs 378 处 to_string/format! 的大头）。
2. **G2 op 枚举**：`BinaryOp { op: String }` 的 op 是封闭小集合，改 `enum MirBinOp`；match 穷尽性顺便让编译器盯着。
3. **G3 callee 枚举**：`Call.func` → `Callee { User(SymId), Runtime(SymId), Dynamic }`——**依赖任务 A 完成后才有意义**，否则与名字瀑布重复建设。

### 任务 H：优化器修复并接通（或删除）

**方案**：推荐"修复 + 默认关闭接入"，给未来留钩子：

1. 修 CSE FloatLit key 碰撞（optimization.rs:348–351：key 加入字面量值）；
2. DCE 嵌套体处理结果写回（或直接删掉嵌套处理——先只做顶层标记-清扫，行为可预期）；
3. `main.rs` 在 `opt_level > 0` 且 `ZETA_ENABLE_MIR_OPTS=1` 时调用 `optimize()`，默认关；打开后跑三基线 + 语料二进制 diff，确认无回归再翻默认；
4. 若两周内 MIR 优化对语料无可测收益（LLVM O3 已覆盖），**诚实删除** 599 行——保留 `-O` 参数解析但注明"透传 LLVM"。

### 任务 I：单态化补真

1. **I1 修自映射**（resolver.rs:2289–2349）：`type_args.zip(type_args)` → `func.generics.zip(key.type_args)`；
2. **I2 修缓存加载**：`load_specialization_cache` 不再插 `Mir::default()` 占位（resolver.rs:168），`record_mono` 时写入真 MIR；`.zeta_specialization_cache.json` 要么做实要么删除（当前内容 `{"entries":{}}`，先按删除处理，A1 之后再评估做实价值）；
3. **I3 eager_generics 改按需**（codegen.rs:1447–1453）：只实例化被调用到的特化，对比二进制体积；
4. **I4 删除 monomorphize.rs 与 codegen.rs 重复的 ~200 行 substitute**（保留 codegen 版）。

### 任务 J：克隆/遍历放大治理

1. **J1**：`register` 存 `Arc<AstNode>`（resolver.rs:183–185 处 clone 消除）；`expand_macros` 只重建被改写的节点；
2. **J2**：`Resolver::lower_to_mir` 每函数 clone 14 张全局表 → `MirGen` 持 `Arc<PyContext>`/全局表引用（依赖 F-R）；
3. **J3**：`main.rs` 四份管线拷贝收敛为 `fn compile(config) -> Result` 单函数（lib.rs 已有 `compile_and_run_zeta` 可作蓝本）；
4. **J4**：`Box::leak` 预处理文本改 owned/`Arc<str>`（top_level.rs:1397），LSP 场景不再累积泄漏。

### 任务 K：死代码全量清理（Q3 之后的第二批）

删除清单（执行前逐个 grep 复核引用）：`frontend/proc_macro.rs`(845)、`frontend/macro_expand_advanced.rs`(617)、`frontend/identity_ownership{.rs,/}`(~639)、`frontend/parser/location.rs`(237，或被 C3 吸收)、`frontend/borrow_enhanced.rs`(611，注意被 tests/memory-management 引用——把依赖它的测试一并迁移或删除)、parser.rs 三个零调用 helper。随后**分模块移除 `#![allow(dead_code)]`**，让编译器此后盯着新死代码。合计净减 ~3,100 行。

---

## 4. P2 —— 工程效率任务

| # | 任务 | 方案 | 预估 |
|---|---|---|---|
| L | 基线进 CI | GitHub Actions 新 job：cargo build → `tools/run_all.sh`（Q4 产物），JSON 汇总作为 artifact 上传；python_style runner 加 `--filter` 与 `xargs -P` 并行（注意每用例已用 mktemp 独立 CWD，并行安全；`.zeta_specialization_cache.json` 写 CWD 的问题在 I2 处理） | 1 天 |
| M | 运行时 .o 构建脚本化 | `tools/build_runtime.sh` 固化 validate.md:129–141 的手工命令；CI 校验 .o 哈希与 C 源一致（防止源改了 .o 忘重编） | 0.5 天 |
| N | 文档与定位诚实化 | README 定位改为"Python→原生 AOT 编译器（量化语料驱动），系统语言自举为远期方向"；CI 的 `zeta_src/*.z` glob 改递归或注明"仅 parse 回归语料"；roadmap/goal/validate 的基线数字统一引用 Q4 的 JSON | 0.5 天 |

---

## 5. 排期建议（✅ 128–138 已按此执行完毕；下表为修订后的 139 起）

> 批次粒度延续现有习惯（一天 1–2 批很常见）；表列为纯还债批次，中间按"护栏原则 4"穿插语料功能批次（当前主线：undef ~65 → <40）。依赖：B4 依赖 B3；G3 依赖 A（✅）；J2 依赖 F-R。

| 批次 | 内容 | 预估 |
|---|---|---|
| 129–138 | Q1–Q5、A1–A5、L、M、D1–D3、B1 | ✅ 已完成 |
| 139 | A 收尾（清 `codegen::new` 残余手写 `py_*`）+ B2 libc 碰撞检查 | ✅ 已完成 |
| 140 | D4 库层假值桩迁移（`--list-stubs` 合并扫 pylib） | ✅ 已完成 |
| 141 | B3：`Type::PyDynamic` + `--report-untyped`（E4 未合入，仍 `from_string`） | ✅ 已完成 |
| 142 | C1 行号映射 + C2 顶层同步恢复（`ZETA_PARSE_RECOVER` opt-in） | ✅ 已完成 |
| 143 | B4 + 误 checkout 恢复（pre_cond / for-in-str / handle_tag） | ✅ 已完成（t206 另案） |
| 144 | 全量三基线确认；t206 DataFrame.columns/`in`；soft 桩 / 语料 undef | 下一刀 |
| 145 | E1 类型覆盖率度量 → 接通/删除决策 | 1 天 |
| 146 | F1 + F-R：gen.rs 拆分第一批（argparse/env/f-string）+ PyContext | 2 天 |
| 147 | H：优化器修 FloatLit/DCE + `ZETA_ENABLE_MIR_OPTS` 接线 | 1 天 |
| 148 | I：单态化修自映射 + 缓存做实/删除 | 1 天 |
| 149 | K：死代码第二批（proc_macro 等 ~3,100 行）+ N 文档诚实化 | 1 天 |
| 150+ | J（Arc 共享、管线收敛）、G1–G3、F2–F4、C3 | 持续 |

---

## 6. 依赖关系

```
Q2 ──► A1 ──► A2 ──► A3 ──► A4 ──► A5 ──► G3
        │      │
        │      └────► B1(签名比对)   C1 ──► C2 ──► (C3)
        └────► D(stub 字段)          E1(独立) ──► E2/E3 或删除
F-R ──► J2                          B3(独立) ──► B4
Q4 ──► L(CI)                        F1..F4(独立, 机械)
I 独立 / H 独立 / K 依赖 Q3 / G1-G2 独立
```

---

## 7. 风险清单与缓解

| 风险 | 缓解 |
|---|---|
| A4 表驱动改变符号解析顺序 → undef 回归 | 影子模式并行一个批次；A4 验收标准就是"语料 undef 计数不升" |
| C2 恢复行为让官方 11 文件爆出新错误 | 默认只告警不致命（STRICT 才致命）；先跑一遍看错误总量再决定是否翻默认 |
| B1/B3 收紧后暴露既有静默错值（`abs(-2.5)` 类） | 这正是目的；按 roadmap 习惯逐个开 t2xx 用例修复，`--strict-abi` 开关兜底 |
| D 桩告警在语料运行时刷屏 | 每符号只告警一次；strict/lenient 双模式 |
| E 删除路线不可逆 | E1 数据先行；删除前打 tag 存档分支 |
| F 拆分引入复制粘贴笔误 | 纯搬家不重写；每步基线对照 + `git diff --stat` 审查"只移动未修改" |
| registry 改动要求重编编译器（include_str!） | A2 后由生成器 + CI diff 兜底；远期可把 registry 移出 include_str! 改运行期/编译期文件读取 |

---

## 8. 如果只做三件事

1. ~~任务 A（符号注册表）~~ **✅ 已完成**（批次 130–135）——影子模式 mismatch=0 的事实证明了方案成立；
2. ~~任务 B3（`Type::PyDynamic`）~~ **✅ 已完成**（批次 141）——未标注 = `dyn`；`--report-untyped` 可清单化；调用点推断对 `dyn` 与旧 `i64` 同等升级；
3. ~~任务 C2（解析同步恢复）+ C1（行号）~~ **✅ 已完成**（批次 142）——W1002 带 `path:line:`；C2 经 `ZETA_PARSE_RECOVER=1` 启用（默认关，防误恢复砸基线）；
4. ~~D4（库层假值桩迁移）~~ **✅ 已完成**（批次 140）——`--list-stubs` 单一口径；loud abort / soft 假值分列。

下一步杠杆：~~**B4**~~ ✅ → **全量三基线** + **t206** + **soft 桩消化** + **语料 undef ~65→<40**（穿插 P1：E1 / F1）。

原第三件（Q4 + L 基线进 CI）✅ 已完成（批次 129/136），护栏已生效——本对账的每个数字都出自 `run_all.sh` 口径。
