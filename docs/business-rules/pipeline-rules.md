# Zeta 编译器业务规则清单 —— 全系统管线级

> 日期：2026-09-25　|　范围：`src/`（Rust 编译器）、`runtime/` + `tokio_runtime.c`（C 运行时）、`pylib/`（Python 兼容层）、`tools/`（一致性闸门）
> 方法：6 路并行分层提取（入口管线 / 前端 / 解析类型 / CTFE+MIR / 后端 codegen / 运行时 pylib）→ 源码抽验（10/10 通过）→ 强制点矩阵 → 端到端流程追踪。
> 权威对照基准：[MODULE-GUIDE.md](../MODULE-GUIDE.md)、[ARCHITECTURE-REVIEW-2026-09.md](../ARCHITECTURE-REVIEW-2026-09.md)、[ABI.md](../ABI.md)。
>
> **状态图例**：`Confirmed` = 强制点与消费点均在源码定位核实；`Corrected` = 经代码核验修正了文档/直觉声明；`New` = 流程追踪新发现。
> **强制度**：`硬` = 代码强制（分支/报错）；`软` = 仅告警/约定/注释；`死` = 声明但无消费点（见附录 A）。

---

## 0. 规则分布地图

| 层 | 目录 | 规则类型 | BR 段 |
|----|------|---------|-------|
| 入口/管线层 | `src/main.rs`、`lib.rs` | 模式切换、失败降级、链接矩阵、环境开关 | BR-P |
| 前端层 | `src/frontend/` | 源码改写、解析截断、隐式合成、关键字表 | BR-F |
| 解析/类型层 | `src/middle/resolver/`、`types/` | 类型归一、证据收敛、mangling、双轨回退 | BR-R |
| CTFE/MIR 层 | `src/middle/ctfe/`、`mir/` | 常量折叠条件、Python 语义特判、槽位不变量 | BR-M |
| 后端层 | `src/backend/codegen/` | 名字瀑布、强转矩阵、ABI、优化级别 | BR-B |
| 运行时/pylib | `runtime/`、`pylib/`、`tools/` | 符号绑定契约、桩边界、GC 约定、一致性闸门 | BR-T |

---

## 1. 管线编排与降级规则（BR-P）—— 本清单核心

**系统级总规则（流程追踪结论，New）**：主 CLI 管线存在五个"失败→告警→继续编"降级点（W1002 解析截断、W0001 CTFE、W0002 宏、W0003 typecheck、borrow 检查静默丢弃）。**一个解析丢 757 行、类型检查失败、宏展开失败的程序，仍然会被降级链一路送过 codegen 和链接**。编译失败在这一层几乎不可能发生；正确性押在"链接期 undefined symbol"这最后一道非强制性防线上。

### 1.1 失败降级链

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| P01 | 解析尾部残留：默认 W1002 告警继续；`ZETA_STRICT_PARSE=1` 才升 E1002 致命。不升 fatal 的理由（注释原文）：官方套件 11 个文件有不可解析尾巴（一个丢 757 行整个 `impl Parser`），fatal 会破坏 194/194 回归底线 | `src/main.rs:384-432`（消费 `main.rs:717`、`:1065`） | nom `many0` 停在第一个失败顶层项，尾部静默丢弃；默认仅告警 | 软（默认）/硬（strict） | Confirmed ✅抽验 |
| P02 | CTFE 失败：W0001 告警，使用未求值的原始 AST 继续。注意：`lib.rs::compile_and_run_zeta` 同一失败是硬错误——"同一编译"两套失败语义 | `src/main.rs:719-748` vs `src/lib.rs:114-115` | 告警继续，不终止 | 软 | Confirmed ✅抽验 |
| P03 | comptime 函数剔除豁免：返回类型文本以 `[` 开头者保留——CTFE 无法物化数组常量，漏删曾致链接期 undefined `generate_residues` | `src/main.rs:724-741` | 静默，直到链接失败才暴露 | 软（不变量靠注释守） | Confirmed ✅抽验 |
| P04 | 宏展开失败：W0002 告警，回退**未展开 AST 克隆**继续编译（lib.rs 侧为硬错误） | `src/main.rs:755-761` vs `src/lib.rs:109-111` | 告警 + 整树 clone 回退 | 软 | Confirmed ✅抽验 |
| P05 | typecheck 失败绝不阻断：W0003 "non-fatal — proceeding with unresolved types" 后照常对每个注册函数 `lower_to_mir`。该码经 `diagnostic_from_code` 以 **error 级别打印**且**未注册**（`error_codes.rs:2162-2164`："W0001-W0003 are still unregistered"），同一个 W0003 在 `typecheck.rs:57,528` 还承载另外两种语义 | `src/main.rs:786-795`；下游 `main.rs:797-807` | error 标签 + 非致命行为 + 一码三义 | 软 | Confirmed ✅抽验 |
| P06 | borrow 检查结果被整段丢弃：失败仅置局部 `ok=false`，`let _ = ok;` 掩盖未读，函数返回值只由 typecheck 决定——borrow 错误零出口 | `src/middle/resolver/typecheck.rs:15-28` | 完全静默 | 死（疑似 bug） | Confirmed ✅抽验 |
| P07 | JIT 找不到 `main`：E4001 诊断打印后仍 `Ok(())`——**退出码 0**，调用方无法从 rc 感知（文件/bootstrap/REPL 三处同病） | `src/main.rs:1026-1041`、`:1238-1244`、`:1324-1328` | error 语义 + rc=0 | 软 | Confirmed |

### 1.2 模式切换与 CLI 契约

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| P08 | 是否给 `-o` 是 AOT/JIT 的**唯一**分界：有 `-o` 落盘链接，无 `-o` 且非探针就地 JIT 执行 `main()`（batch 344 前 `--dump-mir x.z` 会执行 x） | `src/main.rs:915`、`:1022-1023` | 分流 | 硬 | Confirmed |
| P09 | probe_only 四旗联合定义 `dump_mir\|\|dump_ir\|\|report_untyped\|\|report_stubs`；只读旗 + 无输入文件 = 硬错误（防隐式编译执行 selfhost.z） | `src/main.rs:578`、`:1052-1059` | Err 终止 | 硬 | Confirmed ✅抽验 |
| P10 | 只有显式 `--emit-llvm` 可把 `-o` 产物重定向为 IR 文本；`ZETA_DUMP_IR` 环境变量无此权力（特权分离，batch 348） | `src/main.rs:544-548`、`:908-930` | IR 进 stderr，`-o` 产物照常链接 | 硬 | Confirmed ✅抽验 |
| P11 | `--help` 位置无关、全局最高优先（修复前会落到输入文件臂编译并执行该文件）；`--repl` 必须是 argv[1] 且与 `--emit-llvm/--report-stubs/--report-untyped` 互斥；未知 `-` 参数与第二输入文件硬拒绝（batch 350 前 typo 静默变成文件名 rc=0） | `src/main.rs:537-542`、`:614-634`、`:683-701` | Err 终止 | 硬 | Confirmed |
| P12 | `--bootstrap` 只继承**循环前预扫描**的第一个 `-o`，`--target` 恒为 `"native"`——`zetac --bootstrap --target wasm32` 的 target 被静默忽略 | `src/main.rs:637-650` | 静默忽略，无告警 | 软 | New（流程追踪） |
| P13 | 裸 `zetac` = 编译并 JIT 执行 `examples/selfhost.z`（typecheck 结果 `let _ =` 忽略）；文件缺失才报错 | `src/main.rs:1060-1062` | 隐式行为 | 软 | Confirmed ✅抽验 |

### 1.3 确定性与单态化不变量

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| P14 | MIR 发射前按函数名排序：HashMap 迭代序随机曾使 LLVM `print.N` 冲突改名与 C 侧 `.set` 别名表"时好时坏"；`--dump-mir` 输出 canonical 文本供 `tools/mir_diff.sh` 逐字节比较 | `src/main.rs:868-877` | 移除即非确定性编译产物 | 硬（不变量） | Confirmed ✅抽验 |
| P15 | 硬编码 `add<i64>` 种子特化：无条件保证 `MonoKey{add,[i64]}` 存在并注入 used_specs——与用户程序无关 | `src/main.rs:817-834` | 静默 | 软（历史遗留） | Confirmed |
| P16 | 未标注参数类型传播：3 轮定点迭代；只传播 handle 类（Str/Named/vec）、只覆盖缺失/I64 槽；全体调用点不一致则保留；空体 class（≥2 方法）视为接口豁免（批次 295 的 SIGSEGV 防线） | `src/main.rs:80-287`（调用 `:891`） | 违反即静默错类型 → 运行期 SIGSEGV（历史案例） | 软 | Confirmed |

### 1.4 链接与优化级别

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| P17 | target 分派：`wasm32`/`wasm32-wasi` → wasm-ld（`wasm-ld`→`wasm-ld-21` 降级链，`--no-entry --export-all --allow-undefined`）；**其余一切值（含非法值）静默按 native** | `src/main.rs:932-960` | `--target foo` 不报错 | 硬（分派）/软（非法值） | Confirmed |
| P18 | native 链接平台矩阵：Windows `-lmsvcrt -lkernel32`；否则 `-lc -lgc -L/opt/homebrew/opt/bdw-gc/lib -no-pie`（Homebrew 路径硬编码，非该布局平台链接必败） | `src/main.rs:970-983` | 链接失败 Err | 硬 | Confirmed ✅抽验 |
| P19 | 运行时对象优先级：`zeta_runtime_c.o` > `zeta_runtime.o` > `libzeta.a`（缺失静默跳过）；`tokio_runtime.o` 独立探测、存在即链 | `src/main.rs:986-1009` | 缺运行时 → 链接期 undefined symbol 才暴露 | 软 | Confirmed ✅抽验 |
| P20 | 运行时对象定位："ambiguity is announced, never resolved quietly"——cwd 副本与 `ZETA_RUNTIME_DIR` 覆盖不一致时 W2002 告警仍用 cwd；`ZETA_STRICT_RUNTIME_DIR=1` 才强制；exe 向上最多 4 层兜底 | `src/main.rs:440-492` | 告警继续 | 软（strict 硬） | Confirmed |
| P21 | 优化级别恒为 `Aggressive` + `default<O3>` 硬编码（CLI 无 `-O*`）；唯一开关 `ZETA_NO_OPT=1`（且该键未列入 usage 的 Environment knobs 清单）；注释警告 NO_OPT 产物 ~5x 大且慢，勿用于发布 | `src/backend/codegen/jit.rs:78-80`、`:124`、`:147`、`:293-314` | env 降级 | 硬（默认）/软（env） | Confirmed |
| P22 | 环境开关真值语义：`env_flag` 除 `""/0/false/no/off` 外一切非空值（大小写不敏感）为真；消费键 7 个：`ZETA_DUMP_IR / ZETA_STRICT_ABI / ZETA_STRICT_PARSE / ZETA_STRICT_RUNTIME_DIR / ZETA_NO_OPT / ZETA_PARSE_RECOVER / ZETA_LENIENT_STUBS(C侧)` | `src/diagnostics.rs:515-523` | — | 硬 | Confirmed |

---

## 2. 前端层规则（BR-F）

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| F01 | tab 归一化：缩进 tab 按 4 列 stop 展开为空格（`PY-A` 归一化先于一切处理）；归一化文本经 `Box::leak` 泄漏进程生命周期（与预处理输出构成双重泄漏） | `src/frontend/indent.rs:165-204`、`parser/top_level.rs:1628-1629` | 静默重写 | 硬 | Confirmed |
| F02 | **"行首 tab 硬错误"规则已死**：PY-A 归一化使 `TabIndent` 错误分支不可达；测试 `tab_indent_is_error`（`indent.rs:1085-1091`）名字说 error、断言却是 `is_ok()`——名实相反 | `src/frontend/indent.rs:834-857`、`:934-936` | 分支不可达 | 死 | Corrected |
| F03 | 单行复合体改写 `if x: stmt` → `if x { stmt }` 刻意收窄：行必须以块关键字开头（`HEADER_KEYWORDS` 22 词 + 5 前缀修饰）；冒号后在深度 0 且行内无 `{`；非 `::`/`:=`；只递归一层（双层单行体的"响亮失败"实际是静默截断 + W1002） | `src/frontend/indent.rs:138-142`、`:326-439`、`:547-551` | 改写放弃 → 冒号残留 → many0 截断 | 硬 | Confirmed |
| F04 | `//`→`floordiv` 双模式：方言已证明（发生过缩进改写）时一切 CODE 位 `//` 都是运算符；方言未证明时仅括号深度 >0 才算（`{` 不计入深度以保住 `if x { // note`）。**已知漏洞（batch 334 OPEN，测试钉死）**：平坦 Python 文件 `t = 7 // 2` 被当注释吞参数 | `src/frontend/indent.rs:238`、`:590-714`、`:1033-1038` | 静默吞表达式 | 软 | Confirmed |
| F05 | 花括号源码直通：`normalize_blocks` 无改动 → 保留原 `&str`；注意 `changed` 判据是"需要缩进改写"而非"是 Python"——平坦 Python 走直通，方言不可由此判别 | `src/frontend/indent.rs:226-256` | 分流 | 硬 | Confirmed |
| F06 | 解析截断语义：`many0` 在第一个失败顶层项静默停止返回前缀（一个裸 `;` 曾截断 12/13 位置的文件尾部）；`ZETA_PARSE_RECOVER=1` 才启用逐项恢复循环 | `parser/top_level.rs:2122-2148`、`:1591-1615` | 尾部丢弃（接 P01） | 硬（截断）/软（告警） | Confirmed |
| F07 | 隐式 main 合成：定义白名单（FuncDef/StructDef/EnumDef/ImplBlock/TypeAlias/ConstDef/Use/ConceptDef/ExternFunc/ModDef）留顶层，**其余一切顶层语句成为隐式 main 函数体**；纯定义库合成空 main（`py_entry` 标记，防函数尾值变进程退出码）；已定义 main 时模块级语句前插、裸 `main()` 自调用丢弃、`__main__` 卫语句拆接；解析被导入模块时卫语句存活为运行时检查（BATCH-290） | `parser/top_level.rs:1952-2118`、`:2040-2097`、`:1705-1718` | 静默改写用户程序结构 | 硬 | Confirmed |
| F08 | 关键字表：33 个保留字；**刻意不保留** `where`（numpy.where）、`impl`、`type`、`and/or/not`；内建类型名不作保留以支持 `u64::MAX`；成员/方法名永不是关键字（`np.where` 曾丢整个 445 行文件）；关键字边界用只跳行首空白的检查（`return_value = 1` 曾被解析为 `return _value = 1`） | `parser/parser.rs:81-155` | 误判即解析错位 | 硬 | Confirmed |
| F09 | 泛型尖括号 vs 小于号：`parse_type_args` 要求整个 `<...>` 是完整类型列表，前缀匹配即拒绝回溯——否则 `a[b.c < d]` 静默丢文本；方括号版同规则 | `parser/parser.rs:646-672`、`:969-990` | 静默丢弃 | 硬 | Confirmed |
| F10 | `#[cfg(...)]` 求值：`feature` 对照 `--features`；`target_*` 对照 `std::env::consts`；**未知键 → false，裸 `test`/`debug_assertions` → 恒 false** | `src/frontend/cfg.rs:60-114` | 静默剔除 | 硬 | Confirmed |
| F11 | borrow 检查刻意极浅：不递归函数体/分支体（容器一律 `_ => true`），只有模块级语句链被走查——以此把误报面限制住；`SpeculativeState` 只写不读，"并发安全"实为空壳 | `src/frontend/borrow.rs:79-97`、`:142-166` | 漏报为主 | 软 | Confirmed |
| F12 | 死代码群（编译进产物、零调用）：`proc_macro.rs`(845 行)、`macro_expand_advanced.rs`(617)、`borrow_enhanced.rs`(611)、`identity_ownership`(470+tests) | `src/frontend/mod.rs:3-11` 仅声明 | 无行为 | 死 | Confirmed |

---

## 3. 解析/类型层规则（BR-R）

**本层总评（流程追踪结论）**：真正的"类型系统"是四层启发式叠层——register 期字符串归一 → 6-pass 调用点证据 → lower_to_mir 签名恢复 → MIR 后 refine_param_types；HM unify（`types/`）只在新轨道内闭环，而新轨道"能推则推、推不动整体静默回退"，加上管线级 W0003 非致命——**该层编译失败几乎不可能发生**。

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| R01 | 三段式 typecheck：新系统 infer → 三态判定 → fallback 旧 `check_node`。**仅 `UnifyError::Mismatch` 判 Failure**；OccursCheck 等一律降级 Fallback | `resolver/typecheck.rs:15-77`、`unified_typecheck.rs:266-297` | Fallback → W0003 → 逐节点旧检查 | 软 | Confirmed |
| R02 | 新系统对推不动的节点**静默跳过**（"Old system will handle it"，零输出）；只要一个节点成功就跑 solve——新系统几乎从不让编译失败 | `resolver/typecheck_new.rs:67-95` | 完全静默 | 软 | Confirmed |
| R03 | 旧检查器"万物皆 I64"默认：Var/Lit/未知调用/Subscript/整个 match 兜底全部 `Type::I64`——这是 Fallback 路径"几乎总能通过"的原因，也是一大类静默错值的根因 | `resolver/typecheck.rs:353-427` | 静默给错型 | 软 | Confirmed |
| R04 | types/ HM 基础设施基本未消费（验证成立）：`family.rs`/`associated.rs` 零引用；`Kind` 仅 1 处；`unify` 仅被新轨道调用——真正把关的是字符串相等 + 手写兼容表 | `src/middle/types/` 全目录 | 编译进产物但不可达 | 死 | Confirmed |
| R05 | types_compatible 宽松整型互通：i64 可赋给任意无符号及 i8/i16/i32（允许截断，注释自认 "unsafe for negative values but matches common practice"）；`is_copy` 把 str 视为 Copy | `resolver/typecheck.rs:460-516` | 有损但不报错 | 软 | Confirmed |
| R06 | 内建符号表硬编码（~930 行）：`print`/`println` 参数是 `Type::I64`；`==`/`!=`/`<` 等运算符被当函数名注册；extern 指针一律 i64 ABI；ExternFunc 一律注册 `async=true` | `resolver/resolver.rs:3903-4830` | arity/类型检查依据此表 | 硬（表） | Confirmed |
| R07 | import 走查完备性不变量：import 降标为标记 Call 后，register 必须走进 If 的 cond/then/else 与 FuncDef 的 ret_expr（try/except 产物）——漏走查即链接期裸符号（多次实测） | `resolver/resolver.rs:295-402` | 链接失败 | 硬（不变量） | Confirmed |
| R08 | 模块解析三态：registry 命中即用（registry 成员优先，本地文件是"补充"并发 warning）→ 磁盘加载 → 外部 shim（成员按名解析、无类型检查）；未知模块/成员必须告警，"never silently swallowed, which could compile into a program that reads garbage" | `resolver/resolver.rs:496-537` | eprintln 告警 + 继续 | 软（fail-loud） | Confirmed |
| R09 | 模块搜索顺序：被编译文件目录及其 **0..=5 级祖先**（防裸名匹配 `/a.py`）→ `$ZETA_PYLIB` → `~/.zeta/packages` → 内置 pylib → `build/stubs`；靠 rank>0 祖先命中必须 W1005 出声 | `resolver/resolver.rs:2362-2447` | 位置敏感告警 | 软 | Confirmed |
| R10 | mangling 不变量：`<module with _ for .>__<name>`；合成 `<prefix>init()` 用 `zeta_env_get` 哨兵保证模块体只跑一次；同 canonical 路径二次加载别名到首名（防双前缀未定义符号）；循环导入需在解析前插入 `py_user_modules` | `resolver/resolver.rs:2558`、`:2428-2508`、`:2642-2692` | 不带前缀 → 两模块共享全局槽（`_PROJECT_ROOT` 三下划线陷阱实测） | 硬（约定） | Confirmed |
| R11 | 相对导入按 `__package__` 锚点：1 点 = 包内，每多 1 点去一层包尾；越界 fail-loud 告警后丢弃 | `resolver/resolver.rs:2293-2349` | 告警 + continue | 软 | Confirmed |
| R12 | 返回类型证据收敛：固定 **6 个 pass**（每轮传播一层调用深度）；返回类型仅当未标注且在 funcs 表才推断；**单一证据独占**才钉型；f-string 插值内的调用点必须下钻收集（批次 407——不下钻则插值里的调用贡献零证据，参数保持 i64 打出指针） | `resolver/resolver.rs:1213-1774`（f-string `:1548-1553`） | 静默错型/打印指针 | 软 | Confirmed |
| R13 | 参数冲突状态机：真冲突（跨族 str×数值、handle×其它；i64/f64 同族不算）→ 参数**永久**降 `PyDynamic`，funcs 签名表与 registered AST 文本**双边回写**，告警仅首次；`--report-untyped` 单独标出 | `resolver/resolver.rs:1655-1735` | 静默降级 + 一次告警 | 软 | Confirmed |
| R14 | 全局类型推断唯一性闸门：类构造 `__<Class>` 后缀**唯一**命中才采纳且先于 fn_rets（防合成 ctor 遮蔽）；函数返回**所有候选一致**才采纳；列表元素取第一个元素类型；`__collect__` 恒 DynamicArray | `resolver/resolver.rs:1784-1953` | 多命中放弃（50% SIGSEGV 防线，注释实测） | 软 | Confirmed |
| R15 | 返回类型三层覆盖：声明 map/dict 但体是 `json.loads` → 强制 `Named("PyJson")`（map 原语把 JSON tag 当 capacity 会死循环，实测 HANG）；dict 推导取 K/V；未注解 unit 从 body return 反推（批次 300） | `resolver/resolver.rs:3241-3350`、`:2874` | 防死循环/静默指针 | 软 | Confirmed |
| R16 | AST 级 monomorphize 的替换表是**自映射恒等**（`type_args.zip(type_args)`），只做"清泛型标记 + Self 替换"；真正泛型替换在 codegen 侧 `monomorphize_function`（MIR/TypeVar 级）——两套机制并存，与 `specialization.rs` 磁盘缓存构成三条并行线；`resolver.rs:1-5` 自称 "Single source of truth" 与此矛盾 | `resolver/resolver.rs:3546-3606` ✅抽验、`backend/codegen/codegen.rs:3015-3099` | no-op 替换 | 死（疑似 bug） | Confirmed |
| R17 | 每次 `lower_to_mir` 克隆 14 张全局表（逐函数 × 全局状态复制；每函数一个 MirGen、程序级状态靠快照传递） | `resolver/resolver.rs:3241-3392`、`main.rs:798-807` | 性能/一致性成本 | 硬（结构） | Confirmed |
| R18 | 参数默认值与声明类型冲突（字符串默认 vs 非 Str 参数）只告警照收——"`def f(x = \"s\")` would pass a pointer as an i64"；argparse 类型表跨函数全局收集（add_argument 在模块级、`args.<flag>` 在函数里） | `resolver/resolver.rs:598-745` | 告警 + coerce | 软 | Confirmed |
| R19 | 特化缓存暗坑：`load_specialization_cache` 每条插 `Mir::default()` 空占位，未被覆盖的占位以 `name: None → "anon"` 合入 final_mirs——多个占位互相覆盖且可能向 codegen 发射空 MIR | `resolver/resolver.rs:166-178`、`main.rs:859-866` | 静默 | 软（疑似 bug） | Confirmed |

---

## 4. CTFE / MIR 层规则（BR-M）

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| M01 | ConstDef 只有求值为 **Int/Bool** 才被字面量替换；Float/String/Array 或失败一律保留原 AST + W6001，推迟到 MIR gen 时二次 CTFE（仅收 Int/Bool/String），再失败以运行时表达式落地 | `ctfe/evaluator.rs:80-124`、`mir/gen.rs:2860-2890` | 告警 + 推迟 | 软 | Confirmed |
| M02 | if 常量条件折叠被"分支含终止符"禁止——把 Return/Break 内联进块中会产生双终止符 LLVM basic block（历史 bug 源） | `ctfe/evaluator.rs:372-413` | 禁止折叠 | 硬（不变量） | Confirmed |
| M03 | CTFE 预算：递归深度 1024、循环迭代 10,000,000；无限 `loop` 一律不支持；comptime 调用仅当所有实参无变量引用才急切求值（失败静默吞掉） | `ctfe/evaluator.rs:865`、`:1146`、`:27-40` | Err → 上层按 M01 保留 AST | 硬 | Confirmed |
| M04 | **跨层锁步约束**：CTFE 整数语义 = Python（`%` 取除数符号、floordiv 向下取整、移位量 0..64），注释要求与 codegen 的 `build_floormod_int` "stay in lock-step"——两条实现靠注释约束同步，无共享代码或测试锚点 | `ctfe/value.rs:274-335` vs `codegen.rs:2020-2081` | 编译期/运行期算术不一致 | 软（约定） | Confirmed |
| M05 | 比较运算必须产出 Bool 常量，绝不能折叠成 Int(0/1)——返回 Int(0) 曾使 `print(3 == 4)` 打 0，且下游 Int→I64→println_i64 链无法恢复布尔性 | `ctfe/value.rs:242-258` | 错误输出 | 硬（不变量） | Confirmed |
| M06 | MIR 四个 `HashMap<u32,_>` 共享同一 id 空间；**未注册 id 的隐式行为是读作 i64 0**（`gen_expr_safe` 兜底）——漏注册即静默 0 值，无诊断 | `mir/mir.rs:6-26`、`codegen.rs:5689-5695` | 静默 0 值 | 软 | Confirmed |
| M07 | `signature_ret_ty` 单一事实源（批次 399）：首个顶层 Return 的槽类型决定 LLVM 签名，float 保留、其余走 i64 word；嵌套 return 不参与（`def scale(v): return v * 1.0` 曾打出 0.5 的 IEEE 位型整数） | `mir/mir.rs:29-58` | 调用方按 int 槽接 float | 硬 | Confirmed |
| M08 | `for` 的 `counter_id` 与 `var_id` 必须是两个槽：Python 语义 `for k in range(3)` 循环后 k==2（最后被**绑定**值）；var_id 每个被采纳迭代顶部写一次，只有 counter 驱动条件与自增；DCE 显式保活 counter | `mir/mir.rs:193-208`、`gen.rs:2729-2790`、`optimization.rs:110-135` | k 循环后为 3（任务 #54 实测） | 硬（不变量） | Confirmed |
| M09 | Python 真值判定：`and`/`or` 必须短路，"哪边为真"按容器长度问（`zeta_dyn_truth`），不是 `!= 0`——`cost or CostModel()` 曾把 0.0 位型读成"空"使全部 fee() 变 0；用户类实例恒真；`df is not None and len(df)>0` 曾对 None 句柄跑 len → SIGBUS | `gen.rs:3966-4060` | SIGBUS/静默错值 | 硬 | Confirmed |
| M10 | `is None` 在 AST 层已被擦除为 `== 0`（parser 做的），因此 `==` 对零字面量必须排除出元素级掩码路径（identity_sentinel 豁免，批次 398）——否则非空句柄被判 None 走错分支 | `gen.rs:4494-4512` | 走错分支 | 硬 | Confirmed |
| M11 | 二元运算按操作数类型重写（巨石 else-if 链）：str+ → 拼接；数组+数组 → concat；str×int → repeat；`|` 对 set **降级为 concat**；掩码对 → `py_vec_or`（曾对标量句柄做 LLVM and → SIGSEGV）；落通用臂 = 对句柄裸整数运算 = 垃圾值 | `gen.rs:4144-4790` | SIGSEGV/垃圾值 | 硬 | Confirmed |
| M12 | 模块全局写规则单点化（批次 385/386/391）：body lowered 完成后整体 splice env mirror，**mirror 读槽不读 rhs**——`xs.remove(3)` 后 `peek()` 打 3 而 `print(xs)` 打 `[1,2]` 类"一名两答"的修复 | `gen.rs:499-542` | 一名两答 | 硬（不变量） | Confirmed |
| M13 | 嵌套 def 提升双规则：`fn_depth > 1` 走 closure 子生成器发布为独立合成函数（防 Return 混流双终止符）；提升臂**必须**把 `last_closure_ret_ty` 记入 `closure_ret_tys`——缺失则调用方按 I64 槽接字符串打出堆地址（批次 408 修复） | `gen.rs:2075-2108`、`:9463`、`:11141` | 打印堆地址 | 硬（不变量） | Confirmed |
| M14 | 内建缺参不报错：少于形参且无默认值 = Python TypeError 的降级——缺参**读 0** + 每处一次性 stderr 警告（"We cannot fail the build here"） | `gen.rs:993-1005` | 静默读 0 | 软 | Confirmed |
| M15 | MIR 优化 5 pass（DCE/CSE/折叠/强度削减/代数化简）**全部零调用**（grep 证实）；`strength_reduction`/`algebraic_simplification` 连实现都是空壳；`-O0..-O3` 只写进死配置字段无任何读取方；其中 DCE 丢嵌套处理结果、CSE 把所有 FloatLit 折成同一 key——**将来接线前必须先修** | `src/middle/optimization.rs` 全文 | 无行为 | 死 | Confirmed ✅抽验 |
| M16 | `Mir.ctfe_consts` 是幽灵字段：全仓库无一处 insert，gen/monomorphize 只搬空表；`dump_canonical` 每次打印空段——真正干活的是 resolver 的 `ctfe_consts` | `mir/mir.rs:11` | 死字段 | 死 | Confirmed |

---

## 5. 后端 codegen 规则（BR-B）

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| B01 | 符号解析是有序瀑布（18 档）不是查表：`::` 精确名 → `::`→`__` → 裸方法名+arity 校验 → `name_N` → `host_*` → `_inst_` 泛型 → 剥全数字尾缀 → `name.<N>` → 泛型默认实例化 → **兜底"猜签名" extern**（一律 i64 fn_type）——最后一级是静默生成可能错误的调用 | `codegen.rs:2640-2952` | 命中错误符号（历史 SEGV）或链接失败 | 硬（顺序） | Confirmed |
| B02 | 同名去重守卫：`zeta_*` 与含 `__` 的名字**永不**加 `_N` 后缀——否则产生链接器永不可满足的符号（4 个历史幽灵符号 `__get_price_3` 等记录在注释） | `codegen.rs:2919-2946` | 链接失败 | 硬 | Confirmed |
| B03 | 强转矩阵（coerce_call_args 十档）：宽化 zext 静默、窄化 trunc 告警、int→float 分 bitcast（容器槽，位保留）与 sitofp（值转换）、float→int fptosi 告警、**float↔ptr 告警"forbidden"但原值照推**；告警预算 stderr 最多 8 条，`ZETA_STRICT_ABI` 下 fatal 类首条致命 | `codegen.rs:6889-7030` | 静默强转（`abs(-2.5)→nan` 类错值来源） | 软（strict 硬） | Confirmed |
| B04 | 同槽两制：int 写入浮点槽 = sitofp 值转换；容器字读出 = bitcast 位保留（判据 `slot_read_ids` 收集 8 个读函数名 + Assign 不动点传播）——判错方向即出 4.6e18 类垃圾值（0.5→4602678819172646912 实测记录） | `codegen.rs:3344-3367`、`:1637-1721` | 垃圾值 | 硬 | Confirmed |
| B05 | 实参少补零、多则静默截断（Python TypeError 场景零诊断） | `codegen.rs:6988-7012` | 静默 | 软 | Confirmed |
| B06 | struct = 堆分配（`runtime_malloc(fields.len()*8)`）、每字段一个 64 位槽、非 packed 整块 load；f64 字段按位模式 bitcast 读回；恒定 C 调用约定，唯一 ptr 边界是 10 个手写函数且只转第 0 参 | `codegen.rs:6214-6232`、`:4380-4423` | — | 硬（ABI） | Confirmed |
| B07 | 约 240 个运行时声明是 `new()` 里 **233 处手写** `add_function`，内含 **8 对同名重复声明**（`array_push` 一处返回 i64 一处 void），依赖 LLVM 自动 `.1` 改名——即 `.N` 追认机制的自造实例；生成物 `runtime_decls_registry.rs`(294)/`runtime_decls_core.rs`(61) 无任何调用者 | `codegen.rs:81-1346` | `.N` 不确定性 +1 | 软 | Confirmed |
| B08 | `_setjmp` 挂 `returns_twice` 且**首次声明时无条件向 stderr 打两行 PROBE**（无 env_flag 门控，对比 `:1440` 的 `ZETA_PROBE` 门控）——现存唯一的无条件 stderr 副作用（原"无条件 dump 全量 IR"已被 Q1/batch 348 修复为条件化） | `codegen.rs:3379-3412` | 每次编译输出噪音 | 软 | Confirmed |
| B09 | JIT 符号可解析性"**预测而非探测**"：候选 = `JIT_VEC_BINDINGS` 前缀 ∨ `jit_mappings_gen::JIT_MAPPINGS` ∨ `dlopen(NULL)` 进程镜像；探测式查询会触发 MCJIT 惰性 finalize 段错误（`EXC_BAD_ACCESS at 0xb0` 实测）；未解析 extern 替换为自报陷阱 `zeta_jit_missing_symbol`（E4016 + exit(1)），不拒绝编译；`runtime/*.c` 不链入 zetac，`py_*` 在 JIT 模式一律不可解析 | `jit.rs:24-73`、`:143-160`、`:183-254` | 陷阱函数 + exit(1) | 硬 | Confirmed |
| B10 | wasm 走 AOT 专用分支（triple `wasm32-unknown-unknown`、features `+bulk-memory,+simd128`），JIT 直接拒绝 wasm；codegen 内 0 处 wasm 分支——目标差异全部收敛在 jit.rs | `jit.rs:111-113`、`:263-295` | Err | 硬 | Confirmed |
| B11 | 字段索引解析失败静默降级 `("", 2)` 二字段替身或 field 0；`resolve_struct_field_index` 遍历 HashMap（迭代序不确定）→ 同名字段跨 struct 时索引非确定——与"确定性发射序是 ABI 的一部分"精神相悖 | `codegen.rs:6403-6535`、`:5702-5719` | 静默读垃圾 | 软 | Confirmed |
| B12 | 泛型判定 = 显式 `generic_params`（旧启发式按 type_map 曾"silently never emitted"）；无显式实参的调用点急切实例化（防悬空 extern） | `codegen.rs:1493-1512`、`:3000-3012` | 链接失败 | 硬 | Confirmed |

---

## 6. 运行时 / pylib 规则（BR-T）

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| T01 | registry 是编译期内嵌数据（`include_str!`）：**加库 = 改数据文件而非 Rust**（改后须重编编译器）；行格式 M/A/F/W/X/N 六指令；`ret` 缺省 i64、`decl` 缺省 true、`stub` 缺省 false；字段不足静默 `continue` 跳过该行 | `src/middle/pylib.rs:22`、`:98-263` | 静默丢行 | 硬 | Confirmed |
| T02 | **fail-loud 红线**：未知模块成员导入必须响亮失败——"a silent no-op import could otherwise compile into a program that reads garbage" | `pylib.rs:3-7`、`resolver.rs:291-293` | 编译期告警（非致命） | 软（理念硬） | Confirmed |
| T03 | `N` 模块（`__future__`）：导入被接受、零绑定、零校验、零告警 | `pylib.rs:50-53`、`:478-480` | noop | 硬 | Confirmed |
| T04 | 桩双轨：`py_stub_abort` 默认 abort()；`ZETA_LENIENT_STUBS=1` 降级为每符号告警一次（去重表 64 条，表满即静默）并返回 0；`ZETA_STRICT_STUBS` 是 documentary 空操作 | `runtime/py_additions.c:3290-3323` | abort → 运行期崩溃 | 硬 | Confirmed |
| T05 | `soft:` 前缀桩返回假值不 abort（DataFrame.dropna/isin 等恒等/全 1 掩码）；硬桩必须显式调 `py_stub_abort`；把 soft 桩改成 identity→i64 曾让下游 `.columns` 静默变 0 | `pylib/pandas.z:16`、`:127-286` | 静默假值 | 软 | Confirmed |
| T06 | pandas 是"列映射模型"：一列 = 一个列表；表达不了的（sort_values/iloc 行级操作）**响亮失败**；`loc/iloc` 掩码非向量时返回**同列空帧而非 0**（返回裸 0 曾让调用方对 0 调 `.copy()` 崩溃）；时间类型单表示（Timestamp/Timedelta/datetime64 全是 handle 别名，防撕裂 handle_op 分派） | `pylib/pandas.z:1-14`、`:168-188`、`registry.txt:453-471` | 响亮失败（设计） | 硬 | Confirmed |
| T07 | asyncio V1 无事件循环：run/create_task = 恒等（内联求值）、sleep 阻塞；**`gather` 刻意缺席以 fail-loud**（"Values are correct, concurrency is not"）；ThreadPoolExecutor 与 ProcessPoolExecutor 同一构造器——"池"是假的（线程 pthread、进程 fork） | `registry.txt:59-67`、`:36-37`、`tokio_runtime_stub.c:607-841` | 进程池语义静默变线程语义 | 软 | Confirmed |
| T08 | GC 全线约定：Rust 侧 `std_malloc = GC_malloc`、`std_free` 是 no-op；C 侧 Python 兼容层一切堆分配走 `GC_malloc` 绝不 malloc/free（104+ 处）；链接必须 `-lgc` | `src/runtime/std.rs:19-70`、`tokio_runtime_stub.c`、`py_additions.c` | 混用 libc free = UB | 硬 | Confirmed |
| T09 | tokio_runtime.o = `ld -r` 三合一：根目录 `tokio_runtime.c`（编译通过才并入，**失败静默降级 stub-only**，一条 stderr note）+ `tokio_runtime_stub.c` + `unavailable_stubs.c`；kqueue/epoll 双后端共用同一套 epoll 位值事件编码（跨语言 ABI 契约） | `tools/build_runtime.sh:27-48`、`tokio_runtime.c:6-8` | 静默降级 | 软 | Confirmed |
| T10 | 一致性闸门：`check_registry_symbols.sh` 要求 registry 每个 F/W/X 符号（除 decl=0）在两个 .o 的 `nm -gU` 中可见——**stub=1 且 decl 缺省也要求 C 里有 abort 包装器**；退出码 0/1/2 三态 | `tools/check_registry_symbols.sh:29-54` | exit 1 阻断 | 硬 | Confirmed |
| T11 | 平台数据源 API 一律"源不可用"软返回 0（保住本地缓存回退），且用 **WEAK 符号**防撞名——裸符号 `login` 曾被 libc `login(3)`(utmp) 满足导致 `EXC_BAD_ACCESS` | `runtime/unavailable_stubs.c:17-40` | 返回 0 继续跑 | 硬 | Confirmed |
| T12 | 包目录约定：`$ZETA_PACKAGES_DIR` 优先，否则 `~/.zeta/packages`；`zorb install` 与 import loader **共用同一函数**防漂移；pylib 定位不依赖 cwd（向上 4 级 + W1006） | `pylib.rs:370-455` | 静默 | 硬 | Confirmed |
| T13 | 库层被编译器缺陷反向约束（已文档化的契约）：pandas.z 方法形参勿叫 `name`（否则该方法根本不生成）；map 键形参必须注解 `str`（否则按指针探测哈希静默取不到值）；frames 必须标注 `[DataFrame]` | `pylib/pandas.z:6-11`、`:407-410` | 静默错值/段错误 | 软（写法规约） | Confirmed |
| T14 | 生效的 `runtime_malloc` 在 `host.rs`（委托 GC）；`memory_old.rs`/`memory_enhanced.rs` 是孤儿文件不在模块树；`memory.rs` 的实现被整块注释（解开即与 host.rs 撞 no_mangle 符号） | `src/runtime/host.rs:13-19`、`mod.rs:2-29` | 无行为 | 死（边界） | Confirmed |

---

## 7. 词法 / 字面量 / 降糖规则（BR-L）—— 2026-09-25 补提取（覆盖缺口 COVERAGE §2.1/2.2/2.5）

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| L01 | 字符串 blanking：簿记前把字符串内容抹成裸引号对，串内冒号/花括号/`//`/`#` 不参与缩进与注释判定；`#[` 是属性不是注释 | `indent.rs:860-921`、`:881` | 防误判前提（`print("http://x")` 不误开块） | 硬（不变量） | Confirmed |
| L02 | 三引号在**簿记视图**统一 blank 成 `"""`（真实程序文本不动，floordiv 改写路径刻意保留真实引号）；三引号内反斜杠跳过、内容行零簿记 | `indent.rs:885-891`、`:946-962` | 关闭 run 缺失 → 该行永不作块头 | 硬 | Confirmed |
| L03 | 行内单引号字符串未闭合 = 行级降级：该行永不被判块头，语法错误留给解析器 | `indent.rs:908-913` | 静默交后端报错 | 软 | Confirmed |
| L04 | 反斜杠续行折叠：代码位行尾 `\` 把下一行去前导空白拼成逻辑行；续行缩进无意义；三引号内不折叠 | `indent.rs:279-312` | 静默重写 | 硬 | Confirmed |
| L05 | 字符串转义白名单只有 `\" \' \\ \n \t \r` 六种；**未知转义（含 `\u` `\x` `\0`）静默保留两个原字符** | `parser/expr.rs:315-336` | `\q` 输出字面 `\q`，无诊断 | 软 | Confirmed |
| L06 | 进制字面量 `0x/0o/0b` 均支持且允许下划线；**解析溢出或无有效位 `unwrap_or(0)` 静默变 0** | `expr.rs:159-196` | 17 个 F 的十六进制编译为 0 | 软（静默错值） | Confirmed |
| L07 | 十进制整数一律 i64 槽；溢出静默 0；类型后缀（`u8` 等）**只消费不校验范围**（`42u8` 超 255 仍 42） | `expr.rs:199-240` | 静默 | 软 | Confirmed |
| L08 | 浮点 = `数字[.数字][e[+-]数字]`；指数仅在后面真有数字时才消费——`1e`+标识符保持 `1` 和 `e`（注释自记 `v = 1e8` 曾编成 `v = 1`） | `expr.rs:41-143` | 防截断守卫 | 硬 | Confirmed |
| L09 | `None` 降为 `Lit(0)`（自注释 "pragmatic V1"）——`is None` 擦除为 `== 0`（BR-M10）的词法上游 | `expr.rs:1456-1472` | `None` 参与算术即 0 | 硬 | Confirmed |
| L10 | 原始字符串支持 `r"…" r'…' r#"…"#` 三形式、无任何转义；**`r"""…"""` 不支持且会被吃成空 `r""` + 残余 → 解析截断**（无测试钉死） | `expr.rs:364-407`、alt 序 `:1684-1699` | 截断（接 F06） | 软（缺口） | Confirmed |
| L11 | f-string 结构：`{{`/`}}` 转义；`{expr:spec}` 在引号/括号外首个冒号切分为 `__fmtspec__(expr, spec)`；**内部表达式解析失败 → 静默降级为字面文本 StringLit** | `expr.rs:1487-1623`（降级 `:1562`） | `f"{a.b.c}"` 解析失败把原文当字面输出 | 软 | Confirmed |
| L12 | 相邻字符串隐式拼接：跳过任意空白（可跨行）后下一原子仍是字符串即拼接；含 FString 时合并为 FString | `expr.rs:1638-1682` | `"a" x` 不拼接 | 硬 | Confirmed |
| L13 | import 降标双路径：点路径 → `zeta_py_import` 标记 Call（缺省别名 = 末段）；`::` 路径 → use 语义，但 tail 是 `as`/`=`/`;` 三种形状时**拒绝提交回退 python 路径**（防 many0 卡死丢整文件；实测 `import std::memory as m;` 曾 0 诊断错绑） | `parser/stmt.rs:941-1064`（拒绝表 `:982-984`） | 提交错误形状 = 错绑 | 硬（降级回退） | Confirmed |
| L14 | 调用降标：`k=v` → `__kwarg__(name, value)` 包装（lookahead 排除 `==`）；`**d` → `zeta_kwargs_unpack` 标记，**必须在表达式回退前检测**（漏检则双重解引用、callee 静默收 0） | `expr.rs:2342-2393` | 修复前 `f(b=2,a=1)` 静默算成 `f(2,1)` | 硬 | Confirmed |
| L15 | 推导式四种 kind（list/genexp/set/dict）**全部降标为同一个建 Vec 的 `__collect__`**；过滤条件编译成 `if cond { expr } else { -1 }`（**-1 哨兵 = 跳过**）；裸 genexp 作唯一实参不支持（`sum(x for x in y)` 须写 `sum([...])`） | `expr.rs:1165-1274`、限制 `:2362-2365` | dict/set 推导在该层语义不可区分（漂移 4） | 硬 | Confirmed |
| L16 | 三元 `A if C else B` 降为语句形 If；右结合同 Python；else 分支首部一元负号特判包 UnaryOp（五个语料文件的修复） | `expr.rs:3095-3194` | 形状不符回退普通解析 | 硬 | Confirmed |
| L17 | 参数默认值 → 体首 `zeta_param_default(序号, 值)` prologue（extern 不生成）；dataclass 无 `__init__` 字段默认值搬上合成构造器时 **marker 索引含 self 偏移 -1**；`list()`/`dict()` 零参默认归一为字面量（否则链接期 undefined `_list`） | `top_level.rs:341-365`、`:1244-1277` | 修复前 `add(5)` 的 b 静默用 0 | 硬 | Confirmed |
| L18 | 函数体内 `static` 提升为模块级赋值；**同名第二处 → W1009 告警且原地不提升**（不共享单元格）；体内 `use` 提升为模块级导入 + Ignore 占位（仅 `in_body` 路径） | `top_level.rs:1784-1894` | 冲突告警 + 保持局部语义 | 软 | Confirmed |
| L19 | `__main__` 卫语句拆接细节（与 F07 互补）：裸 `main()` 自调用**丢弃**（前插后无限递归，实测 SEGV）；解析被导入模块时卫语句存活为运行时检查、语句进 `__zeta_module_body__` 载体 | `top_level.rs:1720-1757`、`:2048-2071` | 静默改写程序结构 | 硬 | Confirmed |
| L20 | 宏展开：`println!` 字面段保留 + 占位符数≠实参数-1 → Err（batch 377 前 `println!("A={}",1)` 只打 1）；`format!` **非字面模板 → 硬编码 `"formatted string"` 字面量**（静默错误输出，注释以"语料没写过"自辩）；`vec!` → ArrayLit；未知宏 → Err → W0002 回退（P04） | `macro_expand.rs:86-269`（桩 `:224-226`） | 静默错误文本 | 软 | Confirmed |
| L21 | 属性白名单（`derive/test/inline/py_entry/must_use/cfg/allow/deny/warn/repr`）之外 → **W5001 告警并继续**；`py_entry` 豁免计数（诊断基线） | `macro_expand.rs:686-746` | 软失败：属性被忽略 | 软 | Confirmed |

**漂移疑点（LX）**：① f-string 注释自称"V1 无 format spec"但 `split_format_spec` 已实现（`expr.rs:1485` vs `:1553`）；② `indent.rs:24-25` "V1 limitations: single-line blocks not supported" 与已实现的 `fold_inline_bodies` 矛盾；③ `r"""` 缺口无测试钉死（L10）；④ 推导式注释声称四种 kind、实际单一产物——`{k: v for …}` 是否真产出 dict 存疑，值得 S1 验证；⑤ `format!` 非字面模板静默回退建议钉测试；⑥ 数字溢出 `unwrap_or(0)` 双胞胎（L06/L07）与 F04 同属静默错值家族。

---

## 8. 运行时容器与并发规则（BR-C）—— 2026-09-25 补提取（覆盖缺口 COVERAGE §5.2，正确性要害区）

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| C01 | **`vec_get` 零边界检查**（注释自认 "bounds check omitted: ponytail"）：负索引读回**头部字段**（idx=-1 → len、-2 → cap），正越界读相邻 GC 块 | `tokio_runtime_stub.c:480-484` | 静默返回垃圾值（看似有值实为错值） | 软（缺陷） | Confirmed |
| C02 | `vec_push` 增长合同：新容量 `cap<8?8:cap*2`（地板 8 防 `[]` 永不增长）、**返回新数据指针**——调用方必须链式接住 | `tokio_runtime_stub.c:463-475` | 丢弃返回值 = 元素留在旧块**静默丢数据** | 硬（合同） | Confirmed |
| C03 | `vec_push` 头部护栏：cap/len 不构成合法 `[cap\|len]` 头 → 打印调用者符号后 **abort**（防野句柄变成 petabyte 级 GC 申请掩盖真错） | `tokio_runtime_stub.c:450-462` | 响亮失败 | 硬 | Confirmed |
| C04 | `vec_len` 合理性窗口 2^28：超窗判"非 Vec"**静默返回 0**——错型句柄使循环零次执行、结果集静默为空 | `tokio_runtime_stub.c:485-496` | 静默 0 | 软（缺陷） | Confirmed |
| C05 | `array_*` 半残族：`array_push`/`array_set_len` 是**刻意 no-op**（元素静默蒸发）；`array_get/set` 无边界检查且 `:378` 注释声称有检查（**注释失实**） | `tokio_runtime_stub.c:371-380` | 静默蒸发/读写邻块 | 软（缺陷） | Confirmed |
| C06 | 短 vec 判据现行规则："cap<8 仅当块是紧的（GC 取整余量 ∈ {8,16}）才接受"；void 版 `zj_vec_push` 搬移后调用者旧句柄失步，靠"解析期先建后发布"用法约定兜底 | `py_additions.c:3475-3495`、`stub.c:2713-2735` | 拒收 → 静默 0（#32 挂死的族谱） | 软 | Confirmed |
| C07 | map 75% 载荷扩容 + **MAP_MOVED 转发器**（旧块降级为转发、永久存活）——map 永不"满"；把新表拷回旧块 = 堆破坏（~12 键实测 SIGSEGV） | `tokio_runtime_stub.c:243-253` | 违规 = 堆破坏 | 硬（不变量） | Confirmed |
| C08 | map 转发链守卫 **64 跳**：超限返回仍是转发器的块，负 cap 参与取模 = 全盘越界写（单跳版曾写穿 GC 堆） | `tokio_runtime_stub.c:229-237` | 链损坏时越界/SIGSEGV，错误离根因极远 | 软（缺陷） | Confirmed |
| C09 | 线性探测**缺席 = 0 不可分辨**（`map_get` 无法区分"缺席"与"存了 0"，`map_has` 为此存在）；表永不满靠只增不减兜底（无删除操作） | `tokio_runtime_stub.c:254-295` | 缺席读静默 0 | 软 | Confirmed |
| C10 | tombstone（used==2）复用路径存在但**全运行时无任何代码写入**——纯防御性死路径；未来加删除而不更新查找/迭代会把死键当活键（提前登记的坑） | `tokio_runtime_stub.c:259-266` | 当前不可触发 | 死路径 | Confirmed |
| C11 | map 键 = 64 位 FNV-1a **内容哈希本身**；str 键必须经 `map_str_key` 归一（裸指针查 = 恒缺席静默 0）；哈希相撞是文档化上限（后者取值覆盖前者） | `py_additions.c:93-146` | 未归一查 = 静默 0 | 硬 | Confirmed |
| C12 | 两张 8192 槽旁路表**满表静默**：`g_keystr` 首串胜出（第 8193 个不同键的 dumps 键名退化为裸哈希数字）；类型标签表满则 f64 被 dumps 成 int（2.5→2） | `py_additions.c:101-158`、`stub.c:3114-3150` | 静默错值（显示层） | 软（缺陷） | Confirmed |
| C13 | `str_eq` 对 packed 内联串**无判形防护**：packed 槽直入 strcmp 按小端字节当地址解引用，约 25% 概率 SEGV——崩溃点远离真实根因；packed 安全路径只有 `zt_str_content_eq` | `py_additions.c:39-42` vs `:1619-1627` | SEGV（响亮但远端） | 软（缺陷） | Confirmed |
| C14 | str 排序视图遇**控制字符/高位字节即截断**——含非 ASCII 的字符串排序与区间过滤静默错序（历史现场：日期比较全假、回测 0 天） | `py_additions.c:1629-1659` | 静默错序 | 软（缺陷） | Confirmed |
| C15 | `host_str_concat` 野参降级为 ""继续拼接，警告去重**上限 8 次**后完全无声 | `tokio_runtime_stub.c:130-158` | 静默缺一段文本 | 软（缺陷） | Confirmed |
| C16 | 并发返回值：`join` 走 `pthread_join` retval；`thread_results[256]` 是**只写环形遗迹**（第 257 个 spawn 覆盖第 1 个的槽）——今天无人读，是未来改动的高危陷阱 | `tokio_runtime_stub.c:506-534` | 当前不可触发；改读即静默拿旧值 | 死路径 | Confirmed |
| C17 | `join` 无守卫：spawn 失败句柄（-1）、重复 join 都静默返回 0 **冒充线程结果** | `tokio_runtime_stub.c:530-534` | 静默 0 | 软（缺陷） | Confirmed |
| C18 | **无 GIL 真并行**（裸 pthread、每任务一线程无池上限），map/vec 原语**非线程安全**——两线程同时 `map_insert` 同一 map = 转发器竞争/静默丢键/堆损坏 | `tokio_runtime_stub.c:536-550` | 数据竞争面 | 软（缺陷） | Confirmed |
| C19 | futures 语义偏差：`done()` **倒置**（已完成未取结果报 0 → 忙等死循环）；建线程失败 `result()` **静默 0 冒充计算结果**；`Executor.map` 忽略 workers、单元素建线程失败回退内联 | `tokio_runtime_stub.c:676-747` | 静默错值/性能静默塌缩 | 软 | Confirmed |
| C20 | multiprocessing 用 fork：子进程结果 `& 0xff` 后 `_exit`——**结果 8 位回绕**（300→44）；`Pool.map` 实为父进程内线程，无 IPC | `tokio_runtime_stub.c:772-797` | 静默截断 | 软（缺陷） | Confirmed |
| C21 | Thread 单参解包启发式：任何 ≥0x1000 且 `[-16]` 处形似 `cap>0 && len==1` 的实参被拆成首元素——**真传 1 元素向量时收到的是元素** | `tokio_runtime_stub.c:582-599` | 类型静默错位 | 软（缺陷） | Confirmed |
| C22 | **try 深度上限 64 的静默破坏**：第 65 层 `zeta_try_enter` 返回 -1 无人检查、不压帧，codegen 仍对 `zeta_try_slot()`（返回第 64 层的 buf）做 setjmp → 覆写父帧、处理完错弹父层——后续 raise 跳进陈旧栈，全静默 | `py_additions.c:2508-2523`、`codegen.rs:3378-3385` | UB/SEGV，无声 | 软（缺陷） | Confirmed |
| C23 | 异常模型收窄：单一 i64 错误码（无类型/对象）；**首个 except 捕获一切**，`except ValueError` 按类型分派 handler 静默走错分支；`_setjmp`/`_longjmp` 必须配对（错配部分 libSystem 下**静默空转**） | `py_additions.c:2528-2541`、`stmt.rs:1324-1326` | 走错 handler / raise 不跳 | 软 | Confirmed |
| C24 | 闭包 env 是**进程级单例**：键 = 名字内容哈希，同名跨作用域静默互踩（按引用共享槽），槽位只增不清、GC 永不回收 | `py_additions.c:2640-2661` | 同名覆盖 + 单调泄漏（设计使然） | 硬 | Confirmed |
| C25 | f-string 规格解析细则：`','` 被吃掉但**千分位不实现**（`{1234567:,d}` 静默无逗号）；type 后垃圾字符忽略；`'c'`/`'n'` **静默按十进制输出**（`{65:c}` 打 65）；未知 type 强转 'f'；无精度浮点 `%.6f` 复刻 Python 默认 | `py_additions.c:516-618` | 静默偏差 | 软 | Confirmed |

---

## 9. 诊断体系与语法缺口（BR-DG）—— 2026-09-25 补提取（覆盖缺口 COVERAGE §2.2/7.3）

| BR-ID | 规则 | 强制点 | 违规/触发行为 | 强制度 | 状态 |
|-------|------|--------|---------------|--------|------|
| DG01 | 诊断**双出口双轨制**：结构化通道（`diag_error!`/`diag_warning!` 宏 19 处 → 线程局部 Reporter，可聚合）vs 裸 `eprintln!`（12 处 / 11 个码：W1002–W1006/W1008/W1009/W2001/W2002/E1002/E4016）——格式、去向、可测性全部不同，判别规则只在"写在管线哪一段" | `lib.rs:784-809`、`main.rs:428` | 门禁只能抓 stderr 文本数 eprintln 侧 | 软 | Confirmed |
| DG02 | 编号空间三源错位：注册表 **176 条全是 E 码**；实际发射 22 个不同码中**仅 5 个既发射又注册**（≈23%）；**全部 16 个 W 码 `--explain` 必失败**（含最高频的 W1002）；死代码 `borrow_enhanced.rs` 还用 E4001/E4002（Codegen 族）发借错——分类法目前只是注册表的愿望清单 | `error_codes.rs`、`main.rs:583-598` | `--explain W1002` → "Unknown error code" | 软（缺陷） | Confirmed |
| DG03 | `common` 表 **9 个警告常量声明后零发射点**（W1001/W3002/W3003/W3004/W4001/W8001/W8002/W9001/W9002）——引用它们写代码会编译通过但承诺了不存在的告警 | `error_codes.rs:2166-2211` | 死常量 | 死 | Confirmed |
| DG04 | **span 恒 None**：宏体硬编码，带 span 的机制（`with_span`、`extract_context` 下划线快照、`location.rs`）全链死代码；生产诊断**唯一带行号**的机制是 C1 缩进映射，且只报到顶层条目首行（条目内部真因行不报——`parse_bisect.py` 为此存在） | `lib.rs:791/:809`、`main.rs:400-411` | `Diagnostic::format` 的 `-->` 分支永不执行 | 死/软 | Confirmed |
| DG05 | `diagnostic_from_code` **无视码前缀恒定 `Severity::Error`**——任何 W 码经它构造都打 error 标签（P05 的机制根源）；suggestion 唯一附加点在它查注册表；注册表 `example` 字段 176 条全 None（死字段） | `error_codes.rs:2112-2126` | W 码 error 标签 + 零建议 | 硬（现状） | Confirmed |
| DG06 | 工具判据：`parse_bisect.py` 从截断行起按"合法切点"（任意深度 `;` 收尾行 / 深度回到 1 且 `}` 收尾且下一行非 else）逐前缀回喂，**第一条让 W1002 重新出现的行即病因行**；`truncation_inventory.sh` 只读报告恒 exit 0，不阻断任何东西 | `tools/parse_bisect.py:12-14/:87-111` | rc=2 = 有截断未定位 | 硬（工具） | Confirmed |
| DG07 | **语法缺口十族共同不变量**：以下构造全部"应支持但截断"——不发射 unsupported 诊断，解析停在首个不认识的顶层条目、其后整段不进 AST，仅一条 W1002 报**条目首行**（误导性定位），文件仍编译成功**计入 194/194 基线（假绿）**；`ZETA_STRICT_PARSE=1` 才致命。各族现状（以 docs/ABI.md 附 B#10 批次 325 为准）：`s[i..]` 开区段残 230 行（原"match 表达式"族被批次 322 否证改判，任务 #39）；`static mut` 局部 357；`r#"…"#` **已修**（批次 324，归 0）；or 模式构造子 `Some(x @ 1) \| Some(x @ 2)` 62（批次 325 修字符字面量后改判）；体内 `use a::b::C` 85；块体闭包实参 58；`for i: usize in` 36；单段 `import pkg;` 19；常量表达式数组 14；selfhost 158 **病因未定位**（bisect 判据失配的唯一族） | `docs/ABI.md` 附 B#10、`main.rs:418-427` | 静默丢 N 行 + 基线假绿 | 软（缺口） | Confirmed |

**漂移疑点（DG）**：① `error_codes.rs:2163` 注释只承认 "W0001-W0003 未注册"，实际 W0004/W6001/W3001 同样未注册或语义错位——注释边界画窄了；② E1002 确认第三发射方（strict-parse "未解析尾巴"）；③ 任务简报常写"注册表 ~150 条"，实测 176 条且全 E 码——"注册表 ≈ 全部诊断"高估覆盖约 4 倍。

---



| # | 项 | 证据 | 分类 |
|---|-----|------|------|
| A1 | `compiler_config.rs` 整个模块是死配置：`CompilerConfig`/`BuildConfig`/`PGOConfig` 零外部引用；help 自称 v0.3.38 广告 16 个未接线的选项（`-O2` 会得到 unrecognized option）——两份 help 只有一份是活的 | `src/compiler_config.rs` 全文、`main.rs:502-505` 注释自证 | 死配置 |
| A2 | 优化器 5 pass + `-O0..-O3` 开关：见 M15 | `src/middle/optimization.rs` | 死开关 |
| A3 | `lib.rs:89-90` 声称 `compile_and_run_zeta` 是 "primary public API used by the bootstrap `main.rs`"——main.rs 从未调用它（仅 tests 消费），且两者降级语义不同（W 告警 vs 硬错误） | `src/lib.rs:89-122` vs `src/main.rs` | 文档漂移 |
| A4 | `error_codes.rs` 的 `common` 常量模块与注册表逐条错位（`UNEXPECTED_TOKEN="E1001"` vs 注册表 E1001="General parse error"…整模块像旧版码表）；W 码不在注册表内、`common` 常量零消费 | `src/error_codes.rs:2129-2212` | 死代码 + 漂移 |
| A5 | W0003 一码三义两种严重级别：fallback 告警 / "Typecheck failed (non-fatal)"（以 error 级打印）/ identity 告警 | `typecheck.rs:57,528`、`main.rs:789` | 命名冲突 |
| A6 | E1002 一码三义：注册表 "Unexpected token"、strict-parse 的 "unparsed tail"、`common::UNTERMINATED_STRING` | `error_codes.rs`、`main.rs:425` | 命名冲突 |
| A7 | usage 的 Environment knobs 清单漏掉 `ZETA_NO_OPT`（清单 6 个、实际 7 个键）；且 env 清单无 `cli_semantics_check.sh` 式的漂移保障 | `main.rs:527-528` vs `jit.rs:293-303` | 文档漂移 |
| A8 | `-o` 预扫描与循环在重复 `-o` 时分叉：`position()` 取第一个、循环 last-wins——`zetac -o a -o b --bootstrap` 用 a，普通编译用 b（恰是注释声称已对齐的路径） | `main.rs:637-643` | 隐式分叉 |
| A9 | bootstrap 部分解析告警阈值 80 字节形成静默盲区：残留 ≥80 字节既不报 "Partial:" 也不报错，与 "ambiguity is announced" 宣示相悖 | `main.rs:1148-1155` | 静默盲区 |
| A10 | bootstrap 链接分支无 Windows 平台库分派（硬编码 Homebrew 路径 + 不链兜底库）——Windows 上 bootstrap 必败 | `main.rs:1216-1227` | 隐式行为 |
| A11 | 双轨 typecheck 陪跑死代码：`UnifiedTypeChecker`/`TypeCheckStrategy`/`TypeCheckMigrator`（默认 `use_new_system: false`）零消费者；`resolver/concept_check.rs`、`types/{family,associated,kind}.rs` 同 | `unified_typecheck.rs:38-80`、`typecheck_new.rs:405-455` | 死代码 |
| A12 | `const_eval.rs` 的 legacy 兼容层（`legacy_evaluate_constants` 等）仓库内无调用方 | `src/middle/const_eval.rs:15-67` | 死代码 |
| A13 | `monomorphize.rs`（backend）自由函数全仓无调用者——与 resolver 侧、codegen 自带方法构成第 6 份平行单态化实现，ABI.md §4.5 清单未登记它 | `backend/codegen/monomorphize.rs:17-339` | 死代码 |
| A14 | **ABI.md 批次 399 漂移（最重要）**：ABI.md C2/附 B 断言"定义侧签名 = 扫首条顶层 Return、声明不参与、两源并行"；代码已改读 `Mir::signature_ret_ty()` 与调用侧同源（"cannot drift apart again"）——R7 诊断的存在前提与 M5/M6 复现性需重估，ABI.md 章节进度未记录批次 399 | `ABI.md` vs `codegen.rs:1400-1409` | 文档漂移 |
| A15 | MODULE-GUIDE P2-13 "无条件 dump IR" 已过时：IR dump 已条件化（`if dump_ir && !ir_to_file`，Q1/batch 348）；现存的无条件 stderr 副作用只剩 `_setjmp` PROBE 两行（B08） | `MODULE-GUIDE.md` §4.1 vs `main.rs:908-911` | 文档漂移（已修复项） |
| A16 | registry.txt 自相矛盾：`json.loads` 注释说"stays unlisted and fails loudly"，随后注册了 `F json loads py_json_loads`；`W PyExecutor submit` 行 symbol 位置重复两次（解析器忽略裸 token，"侥幸无害"） | `pylib/registry.txt:306-308`、`:351`、`:38` | 注释漂移 |
| A17 | pandas.z 连续两个 `def columns(self)`（:294 与 :297），前者被遮蔽成死代码且无告警 | `pylib/pandas.z:294-302` | 死代码 |
| A18 | `math.log` 是 libm 真实现、`numpy.log` 是硬桩 abort——同名数学函数跨模块语义裂缝，注册表面无统一说明 | `registry.txt:115` vs `numpy.z:54-56` | 语义裂缝 |
| A19 | JIT 绑定双轨：`lib.rs:154-620` 手写 add_global_mapping 清单与 `jit_mappings_gen.rs`（生成物）并存；codegen 注释要求新函数加 `pylib/jit_mappings.txt`，但手写清单无数据源约束——两份清单可各自演化 | `src/lib.rs:154-620`、`jit.rs:149-150` | 双轨风险 |
| A20 | `runtime/tokio_runtime_stub.o`（48 个 T 符号）是陈旧产物，build 脚本写 /tmp、根目录 tokio_runtime.o（647 T）才是链接真身——检入的 .o 无人引用极易误导 | `runtime/` vs `tools/build_runtime.sh:27-45` | 陈旧产物 |
| A21 | reactor 符号不在 registry 的 F/W/X 中：`tokio_runtime.c` 编译失败静默降级 stub-only 不被 `check_registry_symbols.sh` 把关 | `tools/check_registry_symbols.sh` 范围 | 闸门盲区 |
| A22 | `--list-stubs`/`--report-stubs` 的文件扫描面硬编码只有 numpy.z 与 pandas.z——新增 pylib/*.z 的 `# stub:` 标记会静默漏报 | `src/middle/pylib.rs:628-631` | 扫描面写死 |
| A23 | `indent.rs` 头注释与测试名宣称的 "tab 硬错误" 已死（F02）；`borrow.rs` 头注释宣称 affine/ownership + 并发安全，实际浅检查 + SpeculativeState 空壳（F11） | `indent.rs:8-9`、`borrow.rs` 头注释 | 注释漂移 |
| A24 | `resolver.rs:2276-2292` 三段连续 doc comment 堆在 `resolve_py_module_spec` 头上，实际描述的是后面三个不同函数；行号自指注释（引用 `:1794`、`:2937`）任何插入都会再漂（提交 130fab71 专门修过一次） | `resolver.rs:2276-2292`、`:1548-1553` | 注释漂移 |

---

## 附录 B：流程追踪证据（Phase 4C）

### B.1 端到端降级链（文件模式，主入口 `zetac <file>`）

```
read + BOM 剥离 (main.rs:711)
 → parse_zeta：indent 预处理(F01-F05) → many0 截断(F06)
 → ensure_fully_parsed：W1002 告警继续 / ZETA_STRICT_PARSE→E1002 (P01)
 → CTFE：失败 W0001 用原始 AST 继续 (P02)；成功则剔 comptime（数组返回豁免 P03）
 → expand_macros：失败 W0002 回退未展开 AST (P04)
 → register（内建表 R06 / import 走查 R07 / mangling R10）
 → typecheck：borrow 丢弃(P06) → 三段式(R01/R02) → 失败 W0003 继续 (P05)
 → 逐函数 lower_to_mir（14 表克隆 R17）
 → refine_param_types 3 轮 (P16) → add<i64> 种子 (P15)
 → collect_used_specializations → monomorphize（自映射 R16）
 → 合并 final_mirs（缓存占位 "anon" 风险 R19）→ 按名排序 (P14)
 → LLVMCodegen：240 声明(B07) → 名字瀑布(B01) → 强转矩阵(B03)
 → dump_ir 条件输出 (P10) → finalize_and_aot：Aggressive+O3 (P21)
 → 链接：wasm-ld | gcc 矩阵 (P17-P19)
   ⟶ 无 -o：finalize_and_jit（预测式符号解析 B09，缺 main = E4001 + rc=0 P07）
```

**追踪发现（New 类）**：
- (a) **输入被静默覆盖**：`--bootstrap` 吞掉 `--target`（P12）；重复 `-o` 在 bootstrap 路径取首值（A8）。
- (b) **行为与名称不符**：W0003 以 error 级打印却非致命（P05）；E4001 是 error 却 rc=0（P07）；双层单行体"响亮失败"实为静默截断（F03）。
- (c) **错误被静默吞掉**：borrow 结果（P06）、bootstrap ≥80 字节残留（A9）、CTFE 急切求值失败（M03）、JIT 探测式查询改为陷阱（B09，设计如此）。

### B.2 入口矩阵

| 入口 | 触发 | 管线差异 |
|------|------|---------|
| `zetac <file>` | 默认 | 全管线 + AOT/JIT 分流（P08） |
| `zetac`（裸） | 无参数 | 编译执行 `examples/selfhost.z`，typecheck 忽略（P13） |
| 只读旗（`--dump-mir/--emit-llvm/--report-*/`） | probe_only | 禁止执行；无输入文件 = 硬错误（P09） |
| `--repl` | argv[1] | 整行包进 `fn main() -> i64`，恒 JIT native；typecheck 失败每行 E2001 continue |
| `--bootstrap` | 预扫描后 return | 遍历 `zeta_src/` 全部 .z；无 Windows 链接分派（A10） |
| JIT（lib.rs） | 编程 API | 与 CLI 降级语义不同：CTFE/宏/typecheck 全是硬错误（A3） |

### B.3 抽验记录

本清单 10 条承重规则在产出前由主 agent 逐条重读源码验证（标注 ✅抽验）：P01/P02/P03/P04/P05/P06（降级链全环）、P09/P10/P13（模式切换）、P14/P18/P19（确定性/链接）、R16（自映射 subst）、M15（优化器零调用）。全部与代理报告一致，零修正。

---

## 覆盖度声明

- 本清单主体 ~85 条核心规则（自 ~230 条候选中按管线级视野筛选）+ 2026-09-25 补提取 53 条（BR-L 词法/降糖 21、BR-C 容器/并发 25、BR-DG 诊断/语法缺口 7，填补 [COVERAGE.md](COVERAGE.md) 的 A 类缺口）；完整候选明细（含每条的原文引用与历史事故注释）保留在提取过程记录中。
- `gen.rs` 在调查期间被并发修改（777KB→773KB），M 层行号以最后校准为准，建议按注释文本/函数名重定位。
- 未覆盖：`src/lsp/`、`src/ml/`、`src/distributed/`、`src/package/`、`src/debugger/`（初步判断为未接线/人造语料模块，ARCHITECTURE-REVIEW §3 已归入"其他"）；`zeta_src/` 按 MODULE-GUIDE 定位为 parse 回归语料，未提取语义规则。
