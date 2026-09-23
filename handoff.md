# handoff.md — 交接文档（写给下一个接手的 AI：Qwen）

> 交接时间：2026-09-23 深夜。交接人：ZCode 会话（批次 362-367 为其产出，更早记录见 roadmap.md）。
> 工作方式要求（用户明确指示）：**文档和记录全部用朴实的大白话**，不要发明词、不要比喻、
> 不要"燃尽 / 翼 / 族 / 在途"这类说法。说"还没做完"就说还没做完。

---

## 0. 一分钟速览

- 项目：zeta 编译器（自研语言 + Python 兼容层 → LLVM）。当前唯一大目标：
  **让量化策略 `jq_wufu_local.py` 在本编译器上算出与 CPython 一致的结果**
  （final_value 994575.84 / return -0.5424 / 37 个交易日；现在程序能跑完，但算出 0）。
- 你接手时的进度：解析器静默丢代码的债已从 1,019 行清到 712 行（5 个文件完全恢复）；
  14 个字符串/Result 标准方法绑定已实现；官方测试历史上第一次 194 个全部编译+链接成功；
  python_style 296 通过 / 2 个历史遗留失败 / 4 known-fail / 1 xpass（t413，预期内）。
- **你第一件要做的事：修 for 循环体不执行**（见 §3，这是新发现的比丢代码更严重的缺陷）。
- 主线批次 301（修 final_value）处于冻结状态，等解析债和 for 循环修完再启动。

## 1. 环境与命令（照抄就能跑）

```bash
cd /Users/meetai/source/zeta-src        # 工作目录必须是仓库根，否则链接全挂
cargo build --release                   # 编译编译器（约 45 秒）
./tools/build_runtime.sh                # 改过 runtime/*.c 后必跑（重建两个 .o）
ZETA_NO_OPT=1 ./target/release/zetac 文件.z -o 输出名   # 编译用户程序（一律带 ZETA_NO_OPT=1）
bash tests/python_style/run.sh          # 快门禁（约 2-4 分钟）
./tools/run_all.sh                      # 全量门禁（约 7 分钟）
```

- 支持的调试环境变量：
  - `ZETA_PARSE_TRACE=1` —— 解析器在某条语句卡住时打印位置和剩余文本（批次 362 加的，定位丢代码问题全靠它）；
  - `ZETA_DUMP_PP=路径` —— 把缩进预处理后的文本写到该路径（看预处理改了什么；文件原样透传时不写出）；
  - `ZETA_STRICT_PARSE=1` —— 丢代码从警告变成致命错误（量"真实编译了多少"时用）。
- 看崩溃栈：`lldb`。注意输出的退出码含义：139 段错误、134 abort、124 超时。

## 2. 当前状态（交接时点）

| 项 | 数字/状态 |
|---|---|
| 已提交批次 | 367 第一部分（62f65255）；此前 362-366 全部已提交 |
| 官方测试 | 194/194 编译，**194/194 链接**（批次 365 起首次全链接） |
| python_style | 296 通过 / 2 失败（t231、t233，历史遗留）/ 4 known-fail / 1 xpass（t413，预期内） |
| 语料 | 39/39 解析通过 |
| 工作区 | 干净（只剩 .ouroboros/work.md 属另一会话） |
| 主线批次 301 | 冻结中（final_value 修复未开始） |

## 3. P0：for 循环体不执行（你第一件要做的事）

**复现**：

```zeta
fn main() -> i64 {
    for i in 0..3 {
        println!("x")
    }
    return 0
}
```

编译链接全部成功、退出码 0，但**一行 x 都不打**——循环体根本没执行。
对照：带类型注解的版本（`for i: usize in 0..5`）同样只算出 0
（回归测试 t413 以 known-fail 状态锁着这个错误值）。

**为什么排最前**：循环是最基础的控制流。任何依赖循环的程序（包括目标策略程序）
都会静默算错。它不属于丢代码问题——这个文件解析是完整的，错在运行期。

**嫌疑**：批次 332 改过 for/range 的归纳计数器（"独立成槽"）。先 `git show` 那一批，
再用最小文件 + `--dump-mir` 对比循环体是否在 MIR 里、是否被生成。

**验收**：`for i in 0..3 { println!("x") }` 打印 3 行；t413 的 known-fail 标记摘掉、
期望值改回 10；三基线不红。

## 4. P1：解析器丢代码剩余 712 行（8 个文件，卡点已全部探针定位）

| 文件 | 丢行 | 卡点（已探针确认） |
|---|---|---|
| benchmark_simd_vs_scalar | 357 | 函数体内 `static mut counter: u64 = 0`（需全局可变存储语义） |
| selfhost | 158 | `impl Parser for ZetaParser`（trait impl）+ trait 签名声明 + `concept` + match 字符模式 |
| quantum_basic | 85 | `use std::quantum::algorithms::ShorsAlgorithm;` 深路径 use |
| advanced_patterns_test | 62 | `Some(x @ 1) \| Some(x @ 2)` —— @绑定 + or 模式 |
| primezeta_usize_test | 36 | 解析已通（批次 367）；剩 typed 循环变量值 = 0，随 §3 P0 一并 |
| test_const_expression | 14 | `let arr: [usize; MAX + 1]` 数组类型注解（parse_type 不认识 `[T; N]`） |

按性价比从小到大做：test_const_expression → primezeta 剩余（随 P0）→ advanced_patterns →
quantum_basic → selfhost → benchmark。每个做完跑快门禁 + 语料。

## 5. 其他未完事项（P2，不着急）

- **#38**：match 表达式的结果槽恒为 I64（类型问题，归轴 F 最小版）。
- **unwrap 结果无静态类型**：`cs.nth(0).unwrap().len()` 派发错（t411 已用 is_empty 绕开）——根治归轴 F。
- **G.5d 尾部**：强转 6 档补诊断、registry 核签名等（见 backlog）。
- **主线批次 301**（P0+P1 的 S/M 项清零后启动）：0 成交根因 → universe/parquet 分歧
  （119→107 vs CPython 115→103，这是数值对齐的硬前提）→ final_value 对齐。
- **#42 已收口**（14/14 绑定落地，批次 363-365）；遗留观察：minimal_compiler 运行 rc=0
  但无输出（内嵌逻辑的运行期行为，下一层问题）。

## 6. 工作规则（用户明确要求 + 项目红线）

1. **文字用大白话**。不用黑话、不发明词、不打比喻。
2. **任务只登记进 backlog.md**；roadmap.md 只写执行记录（每批一段：现象/定位/修复/验证）。
3. **每批的固定流程**：最小复现 → 修 → 跑门禁 → 提交 → 更新 backlog。
4. **三基线是红线**：官方 194/194、python_style 只有 t231/t233 两个存量红、语料 39/39。
   变红先修再提交。
5. **提交前 `git status` 核对**，`git add` 写明确的文件清单
   （教训：批次 362 漏 add 了源码，靠补交 8aa318d8 才救回来）。
6. **批次号撞号**：可能有另一个会话在并行工作（365 就被两个会话同时用了）。
   开工前先 `git log --oneline -5` 看最新批次号，发现撞号就顺延。
7. 新测试文件要 `git add -f`（.gitignore 曾吞 `.z` 文件，批次 340/346 刚治理过）。
8. **backlog.md 是唯一任务登记表**（有界：新任务入表必须关闭一项旧任务）。

## 7. 老坑（照做能省很多时间）

- **一律 `ZETA_NO_OPT=1`**。批次 307 曾怀疑 -O3 前提反了，但结论未完全复核——
  在 G.1 修复档落地前不要开 -O3 当默认。
- **长跑一律 `timeout` 包裹**。
- **静默错值是最恶劣的失败类别**：编译成功、运行不崩、结果错——本项目历史上大部分
  批次修的都是它。宁可让程序响亮 abort，也不要给一个可能错的值。
- 改 C 运行时后必须 `./tools/build_runtime.sh`，并且把重建的两个 .o 一起提交
  （仓库跟踪它们）。
- 排查"丢代码"类问题：先 `ZETA_PARSE_TRACE=1` 看卡在哪条语句，
  再 `ZETA_DUMP_PP` 看预处理改了什么——两个仪器已经够用，不要回到逐个猜语法的老路。
- 定契约前先查运行时已有的机制（批次 364 定的"0 哨兵"Result 就因没查
  host_result_* 而被批次 365 推翻）。

## 8. 关键文件地图

| 文件 | 内容 |
|---|---|
| `roadmap.md` | 执行日志（每批一段：现象/定位/修复/验证），14,000+ 行，只追加 |
| `backlog.md` | **唯一任务登记表**（有界：新任务入表必须关闭一项旧任务） |
| `refactor.md` | 重构主计划（轴 A-G + 排程；最小架构弧已获用户批准） |
| `pyramid.md` | 编译器设计原则审计清单（树状，含 2026 前沿注解） |
| `docs/ABI.md` | 二进制接口合同（972 行，锚点核对器盯着它） |
| `docs/architecture_optimization_analysis.md` | 架构分析（2026-09-20） |
| `src/middle/mir/gen.rs` | MIR 生成（13.7k 行，最大单体，改动最频繁） |
| `src/backend/codegen/codegen.rs` | LLVM 代码生成（7.6k 行） |
| `src/frontend/parser/` | 解析器（parser.rs / expr.rs / stmt.rs / top_level.rs / pattern.rs） |
| `src/frontend/indent.rs` | 缩进预处理（PY-1） |
| `runtime/py_additions.c`、`runtime/tokio_runtime_stub.c` | C 运行时（方法绑定、判形、Result 单元） |
| `pylib/registry.txt` | Python 库绑定登记表（数据驱动的方法分发） |
| `tools/run_all.sh`、`tools/build_runtime.sh` | 门禁与运行时构建 |

## 9. 最后交代

- 你的前任（本会话）在批次 362-367 期间验证过的工作方式：
  最小复现 → 探针定位 → 最小修复 → 快门禁 → 提交 → 记录。
  这套节奏一天能推进 5-10 批，且不产生静默回归。
- 主线冻结是**用户决策**（先清债后修主线）。P0（for 循环）修完、
  §4 的 712 行清完，就按 §5 顺序恢复主线批次 301。
- 有拿不准的决策（比如闭包的运行时表示、Result 的表示），
  先查 backlog 和 roadmap 里已登记的契约；确实没有先例的，写清方案再动手。
