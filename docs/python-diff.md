# Python 支持面差距清单（2026-10-06）

**这份文件管什么**：把"作为一门现代高级编程语言，zeta 还差哪些"整理成一张分层清单——
架构面差什么、功能面差什么、按什么顺序补。每条带出处，并标清这条数是亲自复跑过的、还是
只读检索得到的。

**这份文件不管什么**：单个 Python 写法的修法站点。那份在
[`python-syntax-gaps-2026-10-06.md`](python-syntax-gaps-2026-10-06.md)（批次 10061 交付，
A9/B3/C16/D3 四条组，逐条给 `file:line`＋机制＋修法＋风险＋验收）。两份的分工：
那份是"逐条怎么修"，这份是"整个语言面缺在哪、先修哪一层"。第 4 节给交叉引用，避免同一个
缺陷两处各说各话。

分析对象＝`/Users/meetai/source/zeta-bz`（分支 `cleanup`，批次 10061 收尾态）。
`/Users/meetai/source/zeta-src/zeta_src` 是停摆的移植语料，不是分析对象。

---

## 1. 证据分级（本文件所有条目标记的含义）

| 标记 | 含义 |
|---|---|
| 【实测】 | 本会话里我亲自跑过命令读到（含读到的代码原文），出处可复现 |
| 【检索】 | 只读检索代理给的数或出处，我没有逐条复跑；引用时按"待复测"处理 |
| 【未找到证据】 | 检索落空。**不等于该功能不存在**，也不等于它可用；只是别当结论写 |

代码规模与在册量的实测底数（都出自本会话跑过的命令）：

| 数 | 值 | 取法 |
|---|---|---|
| `src/` 总行数 | 99,714 | `find src -name '*.rs' \| xargs wc -l` |
| `src/middle/mir/gen.rs` | 3,243 行 | `wc -l` |
| `py_[a-z_]*` 出现次数（`src/middle/mir/`） | 390 | `grep -rno 'py_[a-z_]*' src/middle/mir \| wc -l` |
| 错误码去重 | 201（`src/error_codes.rs` 共 2,225 行） | `grep -o '"[WEI][0-9]\{4\}"' src/error_codes.rs \| sort -u \| wc -l` |
| 差分语料 | 3,026 枚 `.dcase` | `ls tests/diff/cases/*.dcase \| wc -l` |
| python_style | 506 个条目（482 `.z` ＋ 16 `.py` ＋其余） | `ls tests/python_style \| wc -l` |
| 进程内回归钉 | 81 个 `#[test]` | `grep -c '#\[test\]' tests/regression_history.rs` |
| 标准库模块登记行数 | 29 | `grep -c '^M ' pylib/registry.txt` |
| 差分基线登记 | 2,846 条（judged 2,845） | `python3` 读 `tools/baselines/diff_consistency.json` |
| **在册语料与基线的差** | **180 枚 `.dcase` 不在基线**（盘上 3,026 − 基线 2,846；基线里没有"盘上已消失"的条目） | 同一份 json 的 `cases` 键集与 `ls tests/diff/cases/*.dcase` 求差 |

那 180 枚里含批次 10060 入库的 42 枚 `syn60b_*` 探针，以及种子 95012／95015 的生成用例。
**本文件只把"未进基线"当成实测事实记下**——它是没跑、跑了不计、还是判定按基线走，
要读 `tools/diff_test.py` 的口径才能定性，本批没测，所以不写成结论。

顺带纠一条过时注释：`src/frontend/parser/top_level.rs:23-26` 写形参默认值
"parsed and discarded"。实测 `def f(x=3, y="f9")` 的默认值走
`zeta_param_default`（MIR 里实拍到该调用），`f()` 打 `3 f9`、`f(7, "g")` 打 `7 g`，
与 CPython 一字相同【实测】。⇒ 默认参是生效的，注释错了。反倒是编译期那条
"the default for `y` has a kind that parameter type `dyn` cannot hold" 警告，在运行值
正确的情况下照样打【实测】——警告本身的可信度也要打折。

---

## 2. 架构面（四处，按"影响其他结论的可信度"排序）

### 2.1 不支持的写法不报错，而是改成能跑的东西 【实测】

三条静默通道，都能核实到代码原文：

| 通道 | 现象 | 站点 |
|---|---|---|
| 解析截断 | 碰到第一个不认的构造就停，文件剩余部分丢掉；只打 `warning: [W1002] ... DROPPED from the program`，退出码 0 | `src/main.rs:466-503`；同一循环的另一侧 `src/frontend/parser/top_level.rs:2535-2562`；逐项跳过恢复（W1003）要 `ZETA_PARSE_RECOVER=1` 才开（`top_level.rs:2601`） |
| 降级成 0 | 任何没有 lowering 路线的表达式打 `warning: [W1010] ... its slot reads 0`，槽写成 `IntLit(0)`＋`Type::I64` | `src/middle/mir/gen.rs:2448-2454` |
| 臂体直接丢 | `try` 的第二个及以后的 `except` 臂体被丢弃，**连警告都没有**（`if handler.is_empty() { handler = hbody; }`）；`except ValueError` 的类型名解析后被扔掉，异常类型根本不过滤 | `src/frontend/parser/stmt.rs:1478-1480`、`stmt.rs:1460-1479` |

同一族的其它静默点：装饰器整行吞（`top_level.rs:806-812`）、`del obj.attr` 写成 `Lit(0)`
（`stmt.rs:966-972`）、`match` 臂解析失败就 `break` 丢掉后续臂（`expr.rs:3437-3444`）、
`__init__` 里白名单外的语句全丢（`top_level.rs:1348-1356`）、`*args` 上的注解丢弃
（`top_level.rs:75-79`）。缓解项只有一处：`stmt.rs:775` 的 `warn_if_swallowed_prefix`。

**为什么这条排最前**：差分工具 `tools/diff_test.py:540-542`（本批写作时的行号）只在失败详情里
附 W1002 文本，判定看的是 stdout——stdout 恰好一致就记通过。于是"某一族全绿"这个读数本身不完全可信
（这是批次 10060 期间实测到的形状，具体有多少条落在这一档**未复测**）。

**批次 10063 已把这一档接进判定**：`run_zeta` 在退出码之后先扫编译 stderr，命中
W1002／W1003／W1004／W1010 任一条即返回新判定 `degrade` 并点名详情（码＋条数＋首行原文），
不再跑二进制。实测 `--only syn60b` 42 枚翻出 2 枚（都是 W1004，且这两枚本来就不在
2,846 条在册基线里）；窗口 3 抽样 303/303 一致＝该窗口没有用例翻面。
**在册 `match` 2,845 条里还有多少会翻成 `degrade`＝未测**（要全跑一遍才知道），
这一项已登进那份语法缺口文档的 §8 未证清单。

### 2.2 编译器自己没有错误通道，也没有崩溃兜底

- 全 `src` + `Cargo.toml` + `build.rs` 里 `catch_unwind`／`set_hook`／`panic = "abort"`
  命中 **0 处**【实测】。
- 后端 `.unwrap()` 522 处、`panic!` 8 处；`middle` unwrap 103／panic! 51；`frontend` 仅 8
  【检索】。`src/backend/codegen/codegen.rs:6409` 就是 `panic!("Unsupported binary
  operator…")`，`codegen.rs:7692` 不支持元素类型时回退"按数组处理"【检索】。
- 全树 `unimplemented!`／`todo!` 零命中【检索】——缺口不写成显式占位，全靠兜底臂与静默丢。

结果：一个没支持的写法只有两种结局——崩编译器，或者被 2.1 改成 0。中间那条"报错并指出
位置"的路不存在。

### 2.3 语言核心没有独立语义层，全靠前端的降级

- MIR 的枚举定义在 `src/middle/mir/mir.rs:154-279`（语句）与 `:282-342`（表达式）。
  异常、with、import、生成器、lambda、推导式、async、match **都没有对应节点**【检索】，
  分别降级成错误位轮询、`__collect__` 调用、`Call` 标记、`future_poll` 语句、if-else 链。
  `SemiringOp`（`mir.rs:345-348`）只有 Add／Mul。
- 类是 `class` → struct＋impl＋合成构造器（`top_level.rs:858` 起）；继承只取第一个基类，
  注释原文"V1 does not model MRO"（`top_level.rs:862-866`）；`super()` 写死第一基类
  （`top_level.rs:1194-1214`）【检索】。
- 运算与打印不走用户定义的 dunder：`__add__`／`__str__`／`__repr__`／`__call__` 在
  `call_binary.rs` 里零派发点，`gen/call_subscript.rs:19` 注明"runtime has no property
  objects"【检索】。已支持下标的只有 `__getitem__`／`__setitem__`／`__contains__`／`__len__`。
- 异常的运行期机制是全局错误码轮询＋setjmp/longjmp（`runtime/py_additions.c:3497`、`:3515`）
  【检索】，不是 LLVM landingpad，所以"按类型分发"这件事在数据结构层就没办法表达。

### 2.4 构建与目标平台绑死在一台机器上 【检索】

- `build.rs:127-128` 硬编码 `-L/opt/homebrew/opt/bdw-gc/lib -lgc`（macOS Homebrew 路径）。
- LLVM 21 绑定：`Cargo.toml:32-33` 的 inkwell 0.8.0(llvm21-1) ＋ llvm-sys 211
  `prefer-dynamic` ⇒ 用户机器要同版本动态 LLVM。
- `cfg!(target_os)` 全 `src` 仅 19 处，集中在 `src/std/env/mod.rs:254`、
  `src/package/zorb_integration.rs:76`；无 Windows 目标路径。
- CI 五个 workflow 全部 `runs-on` 自托管单机（`.github/workflows/ci.yml:17-71`）⇒ 没有
  跨平台矩阵，上面这些平台假设从没被第二台机器检验过。
- 内存走 Boehm GC（`py_additions.c` 里 `GC_malloc` 106 处、手写 `free` 2 处），双重释放／
  泄漏没有防护机制，只有 `tools/asan_run.sh` 一个脚本，**未实测跑过**。

---

## 3. 功能面（七处，按"能不能写真实程序"排序）

| # | 缺口 | 具体差在哪 | 证据 | 级别 |
|---|---|---|---|---|
| 3.1 | 异常体系 | 类型不过滤、多臂丢弃、裸 `raise` 重抛【未找到证据】、无 Python 级 traceback（出错打 C `backtrace()`） | `stmt.rs:1478-1480`＋`py_additions.c:2029/2313/3288` | 【实测】＋【检索】 |
| 3.2 | 迭代协议 | `yield` 无节点（W1004）；生成器表达式作实参被明确拒绝；多 `for` 子句的嵌套推导式语法只读一个 for＋一个 if；`iter` 只有 Rust trait 桩 | `src/frontend/parser/parser.rs:82-87`、`expr.rs:2372-2375`、`expr.rs:1175-1285` | 【检索】（`yield` 那条批次 10060 已实拍） |
| 3.3 | 集合方法面 | `set` 有 add／discard／remove／intersection／union（`union` 是批次 10063 接上的，走已有的 `py_vec_union`）；difference／symmetric_difference／issubset／issuperset／update 仍全无，且这五条在 `src/middle`、`runtime/*.c`、`pylib` 三处都查不到原语；`tuple` 无 count／index；`frozenset` 只有类型名、无构造器与方法 | `src/middle/mir/gen/call_set.rs:155-172`＋`call_class.rs:46`【实测】；其余【检索】 | 混合 |
| 3.4 | 字符串与字面量 | 无 bytes 字面量；str 缺 encode／decode／casefold／translate／maketrans／format_map；f-string 格式说明符只对 f64 生效；`{x=}` 退化成字面量文本 | `expr.rs:1694-1716`、`call_str.rs:14-88`（约 50 个方法在册）、`expr.rs:1566-1580`、`:1572` | 【检索】 |
| 3.5 | 内省函数 | setattr／hasattr／delattr／globals／locals／dir／vars／id／hash／input／eval／exec／iter／`frozenset` 构造器不在册；`getattr`／`next` 在 | 按 55 个常用 builtin 数，在册 38（`src/middle/mir/gen/call_*.rs` 派发表） | 【检索】，分母口径未复测 |
| 3.6 | 标准库 | 29 个模块（`pylib/registry.txt` 的 `M` 行）。深度不够的：`json.loads` 是真实递归下降但结果是静态标签联合、任意对象序列化不支持；`re` 是 POSIX ERE 子集（flag 只有 IGNORECASE／MULTILINE，命名组【未找到证据】）；`asyncio` 无事件循环；`collections` 只有 Counter／defaultdict；`itertools` 4 项；`random` 6 项。完全不存在的：statistics／string／io／subprocess／socket | `registry.txt:316-377`、`tokio_runtime_stub.c:2093-2133/3227`、`registry:60` | 【检索】 |
| 3.7 | 工具链 | 有：`--repl`（`main.rs:693`【实测】）、zorb 包安装器（自述无版本、无依赖解析、无 lock file）、LSP 骨架 719 行手写协议。没有：格式化器、面向用户的 linter、DWARF（`src/debugger` 828 行只是数据结构，后端无 debug-info 发射点）、`-O` 分档（`jit.rs:125` 硬编码 Aggressive，`jit.rs:76` 自记 "-O3 MISCOMPILE" 风险）。空壳两处：`--incremental` 只在 `compiler_config.rs:170` 置 bool、全 src 无消费者；`src/package` 2,265 行未接进 CLI 主干 | 见各处 | 【检索】＋`--repl`【实测】 |

并发单列一句：线程是真实现（`pthread_create`，`tokio_runtime_stub.c:739-774`；
Lock／Event／Semaphore／Timer／ThreadPoolExecutor；`fork` 在 `:922-943`）【检索】，
但 `tests/concurrency` 只有 9 个用例，GC 与多线程的交互没有专门测试【检索】——
"有线程能力"与"线程可用"之间缺的是验证，不是实现。

---

## 4. 与语法缺口文档的交叉引用（同一个缺陷只算一条，别两处各修各的）

| 本文件条目 | 那份文档里的编号 | 说明 |
|---|---|---|
| 3.2 迭代协议（`yield`） | A9 | 那份文档给 A9 的验收（第 15 批：夹具 `syn60b_gen_two_yield` 从红变绿＋`next()` 明确报错）**站不住**，见 §7 第一条 |
| 3.3 集合方法面（`union`） | B2 | 修法站点已给：复用 `py_vec_union`（`py_additions.c:1872`），零授权 |
| 3.1 异常体系 | A6（`try`/`else`） | A6 只写了 `else` 分支；多臂丢弃是同一族的更深一层 |
| 2.1 解析截断 | M-A0 | 差分工具把 stderr 的 W1002 计为失败，那份排第 1 批 |
| 2.4 / 2.2 | 无 | 这两块那份没覆盖（那份只管写法层面） |

---

## 5. 补的顺序（一个建议序列，不是选项菜单）

| 位次 | 做什么 | 为什么在这 | 验收方式 | 要不要授权 |
|---|---|---|---|---|
| 0 | 把三条静默通道改成明确报错＋非零退出：W1002／W1010 计入失败、第二个 `except` 臂丢弃要报错、装饰器被吞要警告 | 最小、零授权，而且是后面每条结论的地基——现在任何"通过"都可能只是没实现 | 差分工具对同一批用例的判定数变化；`ZETA_STRICT_PARSE=1` 与默认路径各跑一遍对比 | 不要 |
| 1 | 异常体系：类型过滤＋多臂保留＋Python 级 traceback | 决定这门语言能不能写真实程序；也让 2.1 第三条（臂体丢弃）有正经去处 | 一枚三臂 `except` 夹具（不同类型各走各路）在 CPython 与 zeta 两侧同值 | 要（动 `stmt.rs` 与运行期） |
| 2 | 迭代协议：`yield`／生成器／`iter` | 一处修好同时消掉 D2 的上游和一整族推导式问题 | 先另造一枚能观测 `list(g())` 结果的夹具——现成的 `syn60b_gen_two_yield` 只 `print(1)`，看不到 yield 出来的值（§7 第一条）；`sum(seq(4))` 的 `0`→`14` 属 D2，要 A9 修完才谈得上 | 部分（前端可零授权，运行期要授权） |
| 3 | 对象模型：MRO／属性描述符／dunder 协议 | 越晚越贵，但要你先裁定"走真语义层"还是"继续在 `class` 上去糖"——它动的是前端那层，也是那份文档 §7 第四项的一部分 | 一个带两级继承＋`super()`＋`__str__` 的夹具 | 要（方向未定） |
| 4 | DWARF 发射＋`-O` 分档 | 只改这两件就能把日常开发体验（崩了能定位、优化能关）拉起来，且不与上面冲突 | 用 `lldb` 打开一个崩在夹具里的进程能看到源文件名与行号；`-O0`／`-O3` 两侧同值 | 不要（`-O` 分档要碰 `jit.rs`） |
| 不做（现在） | bytes／str 编解码族、statistics 一类标准库补齐、LSP／formatter | 语义层与错误通道没立住之前补这些，等于在不确定的地基上加盖 | — | — |

---

## 6. 待裁定（这份清单新增的，不含那份文档 §7 已有的四项）

1. **对象模型走不走真语义层**（§5 第 3 位）。走＝MIR 要加节点，前端 `class` 去糖层会重做；
   不走＝MRO／属性／dunder 这三样基本按现状封顶，语言定位停在"脚本＋结构体"。
2. **平台假设要不要处理**（§2.4）。`build.rs` 的 Homebrew 硬路径＋动态 LLVM 绑定意味着现在
   只能在一种机器上构建。修法是加检测还是把 bdw-gc 静态化，取决于你要不要跨机器构建。
3. **`--incremental` 与 `src/package` 这两个空壳留不留**。放着是误导（用户会以为有增量编译），
   删是行为变更，接是真工作量。

---

## 7. 本批复测的更正，与仍待复测的数

**已复测并改正的**（不再留在"待测"里）：

- 错误码去重＝201（模式 `"[WEI][0-9]\{4\}"`，文件在 `src/error_codes.rs`；检索代理给 203 且路径写成 `src/middle/`）。
- 盘上语料＝3,026 枚 `.dcase`（与代理一致）；python_style＝506 个条目（代理给 511）；
  进程内回归钉＝81 个 `#[test]`（与代理一致；我记忆里的"62 条"已过时）。
- 标准库模块登记＝29 行 `^M `（与代理一致）。

**复测中发现的两条更正，涉及那份语法缺口文档**：

1. 那份文档 §6 第 15 批给 A9 的验收＝"在册夹具 `syn60b_gen_two_yield` 从红变绿＋`next()`
   明确报错"。实测该夹具（`tests/diff/cases/syn60b_gen_two_yield.dcase`）正文是
   `def g(): yield 1 / yield 2` 加一句 `print(1)`——**它从不观测 yield 出来的值**，头注自述
   "CPython 与 zetac 逐行一致"（也就是本来就是绿的），而且它不在 `diff_consistency.json`
   的 2,846 条基线里。⇒ 这条验收没有能失败的对象，要另造一枚读 `list(g())` 结果的夹具。
   那份文档的表不回改（批次 10061 的记录已入库），更正登记在这里。
2. 同一族：盘上 3,026 枚里有 **180 枚不在基线**（含批次 10060 入库的 42 枚 `syn60b_*`）。
   见 §1 表末两行。

**批次 10063 的行号复绑**（并入主树 49 笔＋本批改动之后按最后一轮实跑取的位置；
每条都是当场 `grep`/`sed` 实测，不是推算）：

- `src/main.rs:466-500` → `466-503`（`ensure_fully_parsed`，W1002 那行在 `:501`；
  `ZETA_STRICT_PARSE` 分支在 `:497`）。
- `src/middle/mir/gen.rs:2356-2380` → `2448-2454`（W1010 告警＋`IntLit(0)`／`Type::I64` 两行）。
- `tools/diff_test.py:521-523` → `540-542`（本批自己加的 19 行把它往后搬了）。
- `stmt.rs:975-980` → `966-972`（`del obj.attr` 的 `Lit(0)` 兜底臂）。
- `call_set.rs:156-161` → `155-172`（三个纯函数：`set_like_receiver` `:155`、
  `set_mutation_ok` `:161`、`set_new_set_ok` `:171`；发射臂 `:112-143`）。
- 复核未变：`mir.rs:154-279`（`MirStmt`）／`:282-342`（`MirExpr`）／`:345-348`（`SemiringOp`）、
  `stmt.rs:1480`（`if handler.is_empty()`）、`top_level.rs:2601`（W1003）、
  `top_level.rs:75-79`（`*args` 注解丢弃）、`call_binary.rs:784`（`|` 用 `py_vec_union`）、
  `py_additions.c:654/740`、`registry.txt:488/513`。
  另两处对得上但差一行：`parse_class` 入口实为 `top_level.rs:857`（§2.3 写 `:858`）；
  "V1 does not model MRO" 那句原文在 `:861-866`，因为它跨行断在 `V1 does`／`not model MRO`，
  按整串 `grep` 会以为这条引用是编的——**核对注释原文要按半个短语搜**。
- **本批未复核**（留在【检索】档）：`top_level.rs:2535-2562`（按 `DROPPED` 字样在
  `top_level.rs` 里 `grep` 不到，锚串可能本来就是错的）、`expr.rs:3437-3444`、
  `py_additions.c:2029/2313/3288`、`codegen.rs:6409`/`:7692`、`jit.rs:113`。

**仍待复测**（本文件里所有【检索】标记的来源，逐条待复跑）：

- 522／103／8 三组 `.unwrap()` 与 `panic!` 计数（分目录）。
- builtin "55 个数 38 在册"的分母口径；str 方法"约 50 个在册"。
- `py_` 特判数：那份文档引用 228（另一处写 264），本批实测 390（取法见 §1），检索代理给
  375——**四个数不同源**。要么统一到一条命令口径，要么每份文档各自写清自己的口径。
- MIR 里"有节点但后端未处理"的清单（`codegen.rs:6409`、`:7692`、`jit.rs:113`）。
- §2.1 第三条（多臂丢弃）目前影响哪几条在册用例——机制核实了，受害者没数。
- 那 180 枚未进基线的语料到底是"没跑"还是"跑了不计"——要读 `tools/diff_test.py` 的判定口径。

**本节存在的理由**：上面这些数进不了结论。要么复测后升到【实测】，要么就保持"未复测"的
写法，不许因为写进了文件就当作已核实。
