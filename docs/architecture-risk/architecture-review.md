# 架构扩展性与风险评审 — Zeta 编译器

> 场景技能：`architecture-visualization:risk-quality-reviewer`　格式技能：`graphviz`（risk-map DOT）
> 评审日期：2026-09-25　|　HEAD：`650bc37a`　|　范围：`src/`（真实编译器，172 `.rs` / 92,324 行）+ `runtime/` C + `zeta_src/`（移植语料）
> 性质：只读分析，零代码改动。所有数字来自本次实际跑过的命令；未测的写在 §四 里标为假设。
> 配套：`risk-map.dot`、`quality-scenarios.md`、`remediation-plan.md`

## 评审目标与口径

问题不是"代码写得干不干净"，而是：**再加一个语言构造 / 再加一个 Python 内建函数 / 再加一个编译 pass，代价是多少，会不会静默出错。**

因此每条发现都绑到一个扩展动作或一个质量属性，不做通用清单。严重度 = 损害量 × 触发频率；置信度单列，不与严重度混谈。

已有一份 `docs/ARCHITECTURE-REVIEW-2026-09.md`（09-19）和 `docs/architecture-health/architecture-health-report.md`（09-25，只管 drawio 图是否过期）。本文不重复其结论，并在 R8 记录它被实测推翻之处。

---

## 一、结论先行

按对"扩展"的阻碍程度排序，四条最该处理的：

1. **`lower_expr` 是一个 10,371 行的单函数**，占 `gen.rs`（14,644 行）的 71%。加任何表达式构造都得进这个函数。本月它已被 255 个 commit 触碰（全仓 789 个，占 32%），净增 11,560 行 = 全仓净增长的 72%。扩展成本已经集中到一个点上了。（置信度：高）
2. ** lowering 的默认行为是"静默吞掉"**：`gen.rs` 内 225 个 `_ =>` 兜底臂；语料侧 `gen.z` 更直白——未handled 语句 `_ => {}` 丢掉、未handled 表达式落成常量 `Lit(0)`。配合下一条，**不支持的构造会编译成功并产出错值**，而不是报错。（高）
3. **codegen 层没有任何错误传播通道**：`codegen.rs` 53 个函数、**0 个返回 `Result`**、507 个生产路径 `.unwrap()`（该文件无 `#[cfg(test)]`）。frontend/middle/runtime 都在用 `Result`（48/58/42 处）。唯一绝不该崩在用户输入上的一层，恰恰是唯一没有错误类型的一层。（高）
4. **`frontend ↔ middle` 双向成环**：5 个 frontend 文件 `use crate::middle`，12 个 middle 文件 `use crate::frontend`。想在两者之间插一个 pass、或单独测 borrow check，当前结构不支持。（高）
5. **整个验证面只有一台机器**：5 个 CI job 的 `runs-on` 逐字相同（`[self-hosted, galaxy, wsl2, x86-64]`）。上面四条能不能被发现，全押在这一台机器上；它停则整条流水线不再跑（不是变红，是不跑）。且 arm64 开发机无法复现门禁。（高）

一句话定位风险：**这个架构的"扩展"动作目前等于"往一个 10k 行函数里加 match 臂 + 在另外 3 个层各补一处 `py_` 特判 + 重新生成 registry"，而失败模式是产出错值而非报错。**

---

## 二、发现明细

### R1 [高] MIR lowering 单函数黑洞　`maintainability, testability`

| 测量 | 值 | 取证 |
|---|---|---|
| `MirGen::lower_expr` 起点 | `gen.rs:3527` | `grep -n 'fn lower_expr'` |
| 其后第一个函数 | `lower_multi_index_element` @ `gen.rs:13898` | awk 扫同缩进 `fn` |
| **单函数跨度** | **≈ 10,371 行** = gen.rs 的 71% | 13898 − 3527 |
| `gen.rs` 总行 / `fn` 数 | 14,644 / 73（同缩进 67） | `wc -l`, `grep -c` |
| `MirGen` 结构体字段 | 45 个，声明 `gen.rs:99-231` | AST 式扫描 |
| 上下文注入 `with_*` setter | 15 个（`gen.rs:284`–`:1022`） | `grep -c 'fn with_'` |
| 本月触碰 commit | 255（全仓 789，**32%**） | `git log --since=2026-09-01` |
| 本月净增行 | +11,560（+12,819 / −1,259），全仓净 +16,136 → **占 72%** | `--numstat` |
| 近 7 天净增 | +5,563（131 commits）≈ 795 行/天 | `--since=2026-09-18 --numstat` |
| churn 榜前三 | gen.rs 255 / codegen.rs 76 / resolver.rs 61 = 全部 commit 的 50% | `--name-only` |

交叉佐证：`architecture-health-report.md` 独立记到 gen.rs 14,644、codegen.rs 7,650，并记载**上一份审计期间 gen.rs 正在被编辑**（06:20 读到 14,645，06:27 变 14,644）。文件在几分钟内移动——这就是"加构造必须进这个函数"的直接后果。

**为什么是扩展性问题，不是代码风格问题**：45 字段 + 15 个 `with_*` setter 的 `MirGen` 是一个 context god-object；任何新构造都要在这一个函数里找到一个臂位，并与其余 225 个兜底臂共存（见 R2）。拆分前，"新语言特性"和"修一个 lowering bug"落同一批 commit，回归面无法隔离。

**验收判据**：拆分后，新增一个表达式构造的 commit 只应触碰 `mir/lower/` 下 1 个新模块 + 1 处枚举，而不是 `gen.rs`。

### R2 [高] 静默吞掉是 lowering 的默认语义　`reliability, correctness`

- `gen.rs` 内 `_ =>` 兜底臂 **225 个**；显式 "not implemented/unimplemented/not supported" 仅 **3 处**；`warn!`/`eprintln!` 31 处。→ 兜底远多于声明式失败，且告警是逐案手工加的。
- 移植语料侧更露骨，两处兜底直接把"不支持"变成"错值"：
  - `zeta_src/middle/mir/gen.z:250` — `_ => {}` 丢掉每一条未handled 语句
  - `zeta_src/middle/mir/gen.z:386-389` — `_ => { insert MirExpr::Lit(0); type_map.insert(id,"i64") }`，任何未handled 表达式**编译成常量 0**
  - `gen.z:170` — "Static array - not implemented yet"，静态数组落成空 `array_new`
- 仓库自己的历史在为这条红线打补丁：批次 405（`f4793586`）的 commit 标题是"getattr 的『未实现』误报：告警块搬到实现它的两条臂之后"。即：**告警点与实现点分离到会互相错位**，说明没有结构性契约，只有逐案调参。
- 项目记忆里的红线（"gen.rs 228 处 py_ 特判禁止新增"）现已到 **264**，超限 36 处——政策漂移见 R5。

**失败模式**：unsupported 构造 → 绿色 CI → 运行期错值。这与 backlog 记录的"链接失败/静默错值"类缺陷同源。
**验收判据**：任一 `_ =>` 兜底被命中时，要么 `bail!` 出诊断，要么显式记 `properties` 供下游拒绝；新增兜底臂需配一条"这条路径真跑过"的夹具。

### R3 [高] codegen 无错误类型　`reliability, usability`

| 模块 | `-> Result<` | 生产路径 `.unwrap()` |
|---|---|---|
| `src/frontend` | 48 | 11（全模块） |
| `src/middle` | 58 | 12（gen.rs） |
| **`src/backend`** | **2（全模块）** | **507（codegen.rs 单文件）** |
| `src/runtime` | 42 | 130（分散） |

`codegen.rs` 专项：53 个 `fn` 声明，**0 个返回 `Result`**，507 个 `.unwrap()`，且文件内无 `#[cfg(test)]`（故这 507 全是生产路径）。第二高的是 `distributed/cluster.rs` 34 个。

后果：codegen 内部任何前置假设不成立即 `panic`，用户拿到的是编译器崩溃栈而不是一条诊断。编译器把这一步做对的标准做法是 `Result`/`Diagnostic` 冒泡——而这一层根本没有那条通道。

### R4 [中高] `frontend ↔ middle` 依赖成环　`extensibility, maintainability`

方向统计（`use crate::<layer>`）：

```
src/frontend -> middle 13   |  src/middle  -> frontend 18
src/backend  -> middle 8    |  src/runtime -> middle   7
src/ml, src/distributed     -> 无跨层依赖
```

- frontend → middle 的 5 个文件：`src/frontend/borrow.rs`、`borrow_enhanced.rs`、`parser/identity_type.rs`、`identity_ownership.rs`、`identity_ownership/tests.rs`
- middle → frontend 的 12 个文件：`middle/mir/gen.rs`、`resolver/{resolver,new_resolver,typecheck,typecheck_new,unified_typecheck,module_resolver}.rs`、`ctfe/{evaluator,context,visitor}.rs`、`const_eval.rs`、`passes/identity_verification.rs`

middle→frontend（消费 AST）是正常的；**violation 在反向**：borrow check 属于前端语义，却依赖 middle 的类型/MIR。这使"前端独立编译/独立测试"和"在两阶段之间插 pass"都不成立。
**验收判据**：`src/frontend/**` 内 `use crate::middle` 归零，或将 `identity_ownership`/`borrow*` 正式并入 middle 并改名。

### R5 [中高] Python 兼容层特判散布 4 个编译器层　`extensibility`

`py_` 在全 `src/` 出现 **862 次**，分布：

| 文件 | 次数 | 性质 |
|---|---|---|
| `backend/codegen/runtime_decls_registry.rs` | 294 | **生成物**，带 `@generated ... DO NOT EDIT BY HAND`，真源 `pylib/registry.txt` |
| `middle/mir/gen.rs` | 264 | 手写 |
| `middle/resolver/resolver.rs` | 133 | 手写 |
| `backend/codegen/codegen.rs` | 57 | 手写 |
| `middle/pylib.rs` | 40 | 手写 |
| 其余（parser/stmt.rs 12、runtime/host.rs 9…） | ≤12 each | 手写 |

→ "加一个 Python 内建函数"的路径是 `registry.txt` → 重生成 → resolver 特判 → MIR lowering 特判 → codegen 特判，跨 4 层。
**这里是全仓护栏做得最好的一处**：CI 用 `python3 tools/gen_from_registry.py --emit*` + `git diff --exit-code` 强制生成物新鲜（`.github/workflows/ci.yml:75-82`），另有 `--check` 做 registry ↔ runtime `.o` 的 nm 对齐（`:86`）。生成物这条边有门；**手写的三处特判（gen.rs / resolver.rs / codegen.rs）没有任何一致性门。**

### R6 [高] 整条 CI 只有一台机器　`testability, availability`

- CI 共 5 个 job：`check`（`cargo check`+`clippy -D warnings`+`fmt`）、`test`（`cargo test --workspace`）、`zeta-selfhost`、`benchmark`、`baselines`（`ci.yml:16/27/37/54/70`）。
- **5 个 job 的 `runs-on` 完全相同：`[self-hosted, galaxy, wsl2, x86-64]`**（`ci.yml:17,28,38,55,71`，`grep -h runs-on | sort | uniq -c` → 5 次同一标签）。这不是 baselines 一个 job 的单点，**是整个验证面的单点**：那台机器停，check / clippy / 570 个单测 / 51 文件自举门 / 804 例语料门同时全部失效，且没有任何 job 会红——是整条流水线不再跑。
- 覆盖面：`tests/` 下 **804 个 `.z` 夹具**（python_style 332、unit-tests 198、primezeta 57、array-parsing 29…）+ 4 个 `.c`；Rust 侧 570 个 `#[test]`、158 个 `tests/**.rs`。
- 我一开始的假设是"530 例语料只在本地跑、CI 没接"——**该假设是错的**，`tools/run_all.sh` 确实在 CI（`ci.yml:90`）。随后我又把单点范围误写成"仅 baselines job"，`grep runs-on` 后更正为上表。
- 架构侧连带：runner 是 x86-64 WSL2，本评审机是 darwin/arm64 → **门禁无法在开发机复现**，且 `cargo build` 只在 x86-64 被验证过。
- `tools/run_all.sh` 内部有 7+ 子门（diff/knob/swallow/import/empty/jit/python），大量 `|| true` 与 `grep -c ... || true` 兜底 → 子门报 0 条时与"该门被跳过"难以区分（本次未逐条验证其 rc 语义，见 §四 A4；若成立，则与单点叠加成"唯一机器 + 唯一门 + 门可能静默"）。

### R7 [中] 停摆语料仍是 3 处运行期输入　`maintainability`

`zeta_src/` 事实：51 个 `.z` / 7,288 行，是 **Rust 逐句转写的伪 Rust**，不是可用的自举：`frontend/mod.z:2-5` 写 `pub mod ast;`、`frontend/parser/parser.z:2` 写 `#[derive(Clone, Debug, PartialEq)]`、`main.z:2-12` import 不存在的 `zeta::` crate、`main.z:48` `-> Result<(), Box<dyn std::error::Error>>`。`middle/mir/gen.z` 399 行 / 6 函数，对应 `gen.rs` 14,644 行 / 73 函数 → **行覆盖 2.7%、函数覆盖 8%**；`frontend/parser/unified_parser.z:13-64` 是 11 个 `// Placeholder` 全返回 `AstNode::Lit(0)`。最后实质提交 `a9372843`（2026-04-02），而 `src/` 在 2026-09-25。

但它**不是死代码**，三处在用它：
1. CI 全量编译它（`ci.yml:50` `./tools/selfhost_compile.sh`，4 个已知失败被钉住：`runtime/array.z`、`runtime/actor/{map.z,result.z}`、`runtime/xai.z`）
2. `--bootstrap` 收集它（`src/main.rs:1143` `collect(Path::new("zeta_src"), ...)`）
3. **`use crate::…` 被解析到它**（`src/middle/resolver/module_resolver.rs:123,164,170,178`）

第 3 条最危险：真实编译器的模块解析路径上挂着一份停摆伪代码。backlog #78 记录 `--bootstrap` 在 "Lowered 285 functions to MIR" 后栈溢出 rc=134。
**验收判据**：要么明确降级为"仅 parse 回归语料"并从 `module_resolver` 摘除，要么给出自举推进的可验证里程碑；当前"名义自举"状态会让每个新来者（含 agent）误判项目定位。

### R8 [中] 现有权威文档正在被实测推翻　`process risk`

`docs/ARCHITECTURE-REVIEW-2026-09.md`（09-19）§二断言"python_style（249 例）与官方 194 均未进 CI"——已由 `ci.yml:90` 推翻，接线发生在 `1e79439a`（2026-09-23，commit 标题正是"删掉两个说谎的步骤"）。同一文档 §三记 `src/` 220 个文件 / gen.rs 10,995 行，实测 172 个 / 14,644 行（gen.rs 6 天内 +3,649）。

仓库已经出现过"CI 步骤说谎要删"的先例，说明这类漂移在本项目是有代价的。文档需带上取证日期与口径，或改为可重跑脚本。

---

## 三、扩展性判定

| 扩展动作 | 当前路径 | 摩擦点 | 失败模式 |
|---|---|---|---|
| 加一个表达式构造 | `gen.rs:3527-13898` 内找 match 臂 | 10,371 行单函数 + 225 兜底臂 | R2 静默错值 |
| 加一个语句构造 | 同上 + `lower_ast_inner` | 同上 | `_ => {}` 丢语句 |
| 加一个 Python 内建 | registry.txt → 重生成 → resolver → gen.rs → codegen | 跨 4 层，3 处手写特判无门 | 链接失败 / 错符号（批次 406 实例） |
| 加一个 pass | 受阻 | R4 frontend↔middle 环 | 不支持 |
| 加一条 .z 语料用例 | `tests/` + `run_all.sh` | 只能信 x86-64 WSL2 那台机 | 本机不可复现 |
| 加一个错误诊断 | codegen 无处可加 | R3 无 `Result` | panic |

已经做对的、应当保留的护栏：registry 生成物 + `git diff --exit-code` 新鲜度门（`ci.yml:75-86`）、`clippy -D warnings`、逐批次回归记录（30 天 787 commit）、fail-loud 红线意识（`validate.md`）。**本评审没有建议动这些。**

---

## 四、假设与缺口（未测，不作为风险）

- **A1 编译期分阶段耗时不可知**：本次无运行期遥测。下一步：读 `tools/perf_baseline.py` + `benchmark` job（`ci.yml:54-67`）确认基线口径。
- **A2 依赖新鲜度/LLVM 版本约束未评估**：仓库有 `deny.toml`（存在，内容未读）。下一步：`cargo tree -d` + 读 `deny.toml` 白名单。
- **A3 符号/ABI 名字瀑布未独立验证**：09-19 文档把它列为 #1 风险（codegen 310 行名字瀑布 + C 运行时 66 条 `.set` 别名）。我未复核该行号与计数，仅 `runtime_decls_registry.rs` 侧确认了生成物机制。批次 406（`94007ada` "根文件 `_x` 的同模块调用被改成幽灵符号（整程序链接失败）"）是该风险仍在发作的旁证。下一步：定位并量测该名字瀑布。
- **A4 `run_all.sh` 子门的 rc 语义未逐条验证**：文件内多处 `|| true`；"某门 0 条断言"是否会让 job 静默通过，未证明。下一步：故意让一个子门 0 命中，看 job 是否仍绿。
- **A5 C 运行时边界未定**：`runtime/` 6 个 C 源，health 报告已把"哪些算 C 运行时"列为待决。

---

## 五、读法

`risk-map.dot`（`dot -Tsvg risk-map.dot > risk-map.svg`）把 R1–R8 叠在受影响的架构元素上，节点标签即取证锚点。`quality-scenarios.md` 给每个质量属性的可测场景；`remediation-plan.md` 按 ROI 排序，含 P0/P1/P2 与验收门。严重度与置信度在本文各条独立标注，未合并成单一分数——本仓无评分要求。
