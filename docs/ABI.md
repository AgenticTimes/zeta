# Zeta ABI 合同

> **G.5（refactor.md）**：把"一个槽里到底能放什么、放取怎么对称、一次调用到底传什么"
> 写成合同。本文只描述**现行实现**并逐条锚定 `file:line` —— 不是愿望清单。
> 验收口径：§1 表穷尽（没有任何运行时探针假设了表外的表示，附 A 逐个反查）、
> §2/§3 每条规则有锚点、§3 的结论经过实测核对（§3.5）。
>
> 章节进度（G.5a-e 分次独立提交；**编号已为后续章节预留**）：
> §1 槽位值表示、§2 写/读对称 = G.5a（批次 305）；
> §3 调用约定与 struct/元组返回 = G.5b（批次 316）；
> §4 名字修饰、§5 类型布局、§6 跨边界假设 = G.5c（§1 表 #6/#7/#8 的几何常量届时
> 升格为明文的布局合同）；
> 附录 audit 表 + "先改本文再改实现"的流程规则 = G.5d；符号注册表 = G.5e。
> 验收反查与未决项放在 **附 A / 附 B**，不占用正式编号；附 A 目前只覆盖 §1，
> §3 的对应验收物是 §3.5 的 M1–M4。
>
> ⚠️ `runtime/tokio_runtime_stub.c` 由并发工作流持有：本文**只引用，不修改**。

## 1. 槽位值表示表

前提（codegen.rs:1576-1580）：**静态可定的本地按自身 LLVM 类型开槽**
（`Type::F32`→`float`、`Type::F64`→`double`、其余一律 `i64`）。因此"8 字节槽"
指的是**除 f32/f64 之外的所有本地槽**，以及容器（vec 元素、map 值、struct 字段）
里那些没有静态类型的**裸字** —— 后者的合法内容才是本表的主题，也是
批次 291/297/298/299/300/301 六批连发的同一类 bug 的现场。

| # | 表示 | 槽内编码 | 写侧（产生处） | 读侧要求 | 混淆史（实测） |
|---|---|---|---|---|---|
| 1 | `i64` | 裸二补码 | `MirExpr::IntLit` | 原样 | — |
| 2 | `f64` | **IEEE-754 位模式**，不是裸整数 | ① 静态 `double` 槽：`Assign` 时 int 值走 `slot_sitofp`（codegen.rs:3274-3299）；② struct 字段是 64 位槽：存前 bitcast（codegen.rs:6157） | 从**裸容器字**读回必须是 `bitcast`，不能 `sitofp`（`slot_or_sitofp` codegen.rs:1683-1698，判据 `slot_read_ids` :60 / :1574） | 批次 298：裸存 → 读回 4.94e-322；批次 299：缺 `sitofp` → `500.0 + 50` 算成 `4.6e18`（:5727 记录） |
| 3 | `bool` | **i64 的 0/1** | 比较结果立刻 `zext i1→i64`（`eq_ext`/`ne_ext`/…，codegen.rs:3728-3736） | 一律 `!= 0` | `Type::Bool → bool_type`（:7283）只在 ABI 边界（参数/返回签名）出现；**i1 从不进槽** |
| 4 | `str`（指针形） | `.rodata` 或 GC 块首地址，`ptrtoint` 成 i64 | `MirExpr::StringLit`：私有 global 字节数组 + `ptrtoint`（codegen.rs:5647-5666） | 可直接解引用；`GC_base(h) != 0` ⇒ 堆串 | 与 #5 不可静态区分 ⇒ 见 R3 |
| 5 | `str`（packed 形） | 短 ASCII **按字节小端打进这 8 个字节本身**（实测样本 `"43601853"` = `0x3335383130363334`） | `vec<str>` 的元素槽（批次 291 实测；写侧仍是隐式决定，见附 B OPEN） | **必须先判形再解引用**；唯一的判形点在 `map_str_key`（py_additions.c:105-129）：`GC_base` 命中→按内容 FNV 哈希；否则 `u < 2^32 \|\| u >= 2^48`→判为 packed、原样当不透明键；再否则 `vm_read_overwrite` 探一页可读性 | 把 packed 当 `char*` 传给 `str_trim` → 约 25% 概率 SEGV（批次 297）；packed 之间比较走 `zt_packed_cstr_eq`（:1603） |
| 6 | `vec` 句柄 | 指向**数据**；`[cap \| len]` 头在 `base-16`；块 ≥ `16+cap*8` | `zeta_collect_vec_n` / `str_split` / df 列构造器（多数按元素数申请，故 cap 常 < 8） | `zt_dyn_vec_hdr`（py_additions.c:3469-3491）：`cap∈[1,2^28]`、`len ≤ cap`、块够大；**短 vec（cap<8）还必须是"紧块"** —— GC 向上取整的余量恰为 8 或 16，`have > need+16` 即判否 | 批次 301：`cap >= 8` 曾是唯一判据 ⇒ 1..7 元素列表被拒，`len()` 退化成对数据字做 `strnlen`（实测 3 元素报 0；1/4/8 报 5/0/8） |
| 7 | `map` 句柄 | **块首**；`w[0]=cap`（2 的幂、≥16、≤2^28）、`w[1]=used ≤ cap`；块 ≥ `16+cap*24`；`cap < 0` ⇒ growth forwarder（真表在 `w[1]`） | `MapNew` / `DictInsert` | `zt_dyn_is_map`（py_additions.c:3457-3467）+ `GC_size` 兜底 | 与 #8 撞车：map 把 PyJson 的 tag 当容量（见 #8） |
| 8 | `PyJson` | 16 字节 `[tag, payload]` **单元指针**；tag = `ZJ_NULL 0 / INT 1 / F64 2 / STR 3 / ARR 4 / OBJ 5 / BOOL 6`（tokio_runtime_stub.c:2550-2556，构造 `zj_make` :2560） | `json.loads` 一族 | 访问器按运行时 tag 分派（sum type；**不是**外层程序的动态分发） | `-> dict` 注解把 Json 当 map ⇒ 首字（tag ≤ 6）被当容量，`idx = hash & (cap-1)` 死循环（tokio_runtime_stub.c:190-196 记录；现行以"map 首字 ≥16、Json 首字 1..8"作值域区分并响亮报告） |
| 9 | 用户 `struct` | GC 句柄，字段各占一个 64 位槽 | `StructNew`（#2②） | 字段读 = 槽偏移 + 按字段静态类型还原（f64 见 #2） | 批次 300：未标注返回类型退化成 unit ⇒ 字段读成"空 variant 的第 0 字段"，**每个属性读都静默拿垃圾** |
| 10 | `PyDynamic`（预留） | boxed tag cell | — | — | B/T3 落地时更新本行。当前 dyn 值就是 #4–#8 那些**无标签字** —— 这正是 #6/#7 必须做几何判形的根本原因 |

## 2. 写/读对称规则

**R1 整数入浮点槽 = 值转换（`sitofp`），不是位保留。**
锚点 codegen.rs:3274-3299（`MirStmt::Assign` 的 `slot_sitofp`）。
反向不存在"浮点入整数槽"：浮点要么进 `double` 槽（值），要么进 64 位裸槽（位模式，R2）。

**R2 裸容器字里的 f64 出槽 = `bitcast` 回读。**
锚点 `slot_or_sitofp`（codegen.rs:1683-1698）——同一个 helper 二选一，判据是
`slot_read_ids`（声明 :60，`collect_slot_reads` 填充 :1574，语义注释 :1613/:1682）。
> **代偿标注**：`slot_read_ids` 是 F 轴（类型检查独立 pass）落地前的补丁；
> F 落地后本条回到"按静态类型判"，该字段与其收集函数一并删除。

**R3 `str` 的指针/packed 只在 `map_str_key` 一处判形。**
锚点 py_additions.c:105-129（判形放这里的理由写在注释里：编译器 lowering 的
`map<str,_>` 访问与 C shim 侧的调用点**都汇流到这一个符号**）。
新增 str 消费点要么经它，要么自己按同一套三段判形做，**不得假设可解引用**。

**R4 句柄与裸整数在槽里不可静态区分 ⇒ 判形只能靠几何不变量。**
锚点 py_additions.c:3457-3491（#6/#7 的全部常量）。
推论（合同级）：**任何 GC 块布局或对齐的改动都是在改本章**；`len()` 一类必须把
"当文本读"放在最后（`zeta_dyn_len` py_additions.c:3492-3500 的注释即为此）。

**R5 bool 恒以 i64 的 0/1 参与槽操作。**
锚点 codegen.rs:3728-3736。容器真值另有一套按长度的判定（批次 297），
两者不互相转换 —— "非空 dict" 是 `len>0`，不是 `!= 0`。

**R6 struct 字段一律 64 位槽；宽度≠8 的类型进/出槽各 bitcast 一次。**
锚点 codegen.rs:6157（存侧注释）+ #2 读侧。

## 3. 调用约定与 struct/元组返回（G.5b）

**先划边界**：本文的"调用约定"是三个不同的边界，混起来谈是本章之前的常见错误。
§3.1 zeta→zeta（同一 LLVM module 内的用户函数）、§3.2 实参强转全表、
§3.3 zeta→C 运行期、§3.4 间接调用与 Python 式形参折叠、§3.5 实测核对。
**本章只描述现行实现**；§3.2 的"判定"列是 G.5d 收敛的靶子，不是本批的行为变更。

### 3.1 zeta→zeta：签名字母表只有三个类型

**C1 参数与返回的 LLVM 类型只能是 `i64` / `f32` / `f64`。**
锚点：参数映射 codegen.rs:1450-1457（`Some(Type::F32)`→`f32`、`Some(Type::F64)`→`f64`、
`_`→`i64`）；返回映射 :1458-1463 + `infer_fn_return_type` :1375-1390（同样只在
f32/f64/i64 中选）。
推论（合同级）：**struct、元组、串、容器、枚举一律以 i64 句柄/裸字传递与返回**；
LLVM 层不存在聚合返回 —— `sret`/`byval`/`struct_ret` 在 `src/` 命中 0。
写侧照此：struct 字面量 `runtime_malloc(fields*8)` + 字段各占 8 字节
（:6116 注释原文 "Allocate struct on HEAP to prevent dangling pointers"、:6117、
:6180-6181）；元组走 `StackArray` 的 `[cap|len|elems…]`、句柄 = `buf+16`（:6630-6690，
与 §1#6 同一几何）。读侧：字段访问 = `int_to_ptr` + **整块 load**
`struct_type(&[i64; N], false)`（:6406-6413，非 packed）+ `extract_value`（:6445 起）；
字段写 = 地址算术 `base + idx*8` 后 `store`（:5391-5408）。
⚠️ **另一套通用映射器不参与签名**：`type_to_llvm_type`（:7269）会把 `Type::Tuple`
映成真 LLVM struct（:7375-7381）。改签名时只认 :1450/:1458，认它就等着 ABI 不一致。

**C2 返回类型取自 MIR 里第一条 `Return` 的值的类型**（`infer_fn_return_type`
:1376-1388 扫 `mir.stmts`，**首条命中即返回**；扫不到则默认 `i64` :1389），
不是取自声明的 `-> T`。目前两者结果一致（§3.5 M1/M2/M3），但归一化发生在哪一层
本批未定位 ⇒ 登记附 B#4。
⚠️ 更准确的说法是：**这条规则的"谁是最终裁决者"尚未成文**。M2 的实测显示
**声明类型赢**（首返 `2.5` 而 `-> i64` 得 `2`），说明 :1376-1388 的推断不是终点；
在定位到那一层之前，C2 只能作为"读码所得 + 实测口径待补"来引用。

**C3 声明与首个返回不一致时，在返回语句处补偿：`ret_sitofp` / `ret_fptosi`**
（:4488-4530，`want_float` 由被调函数的签名反查 :4488-4497）。
**补偿是有损的且不告警**：`-> i64` 里 `return 2.5` → 静默变 `2`（M2 实测），
`abi_note` 不参与返回侧（它只服务实参，见 §3.2）。

### 3.2 实参强转全表（`coerce_call_args`）

锚点 codegen.rs:6790-6915；告警器 `abi_note` :6917-6931（stderr 最多 8 条，
`strict_abi` 时首条即致命）。调用点：:3766、:3782（运算符名兜底）、:4317（常规调用）、
:4474（void 调用兜底）、:4695（`DictInsert`→`map_insert`，传空 `arg_ids`）。

| # | 实参 → 形参 | 发出的指令 | 丢值？ | 告警 | 判定 |
|---|---|---|---|---|---|
| 1 | `i<n>` → `i<m>`，n<m | `arg_zext`（:6826-6830） | 否 | 无 | **保留** |
| 2 | `i<n>` → `i<m>`，n>m | `arg_trunc`（:6841-6844） | **是**（丢高位） | `narrow iA→iB`（:6832-6839） | **删除候选**（改诊断） |
| 3 | 同宽整数 | 原样（:6845-6846） | 否 | 无 | 保留 |
| 4 | int → float，且该实参来自容器裸槽 | `arg_slot_bits` **bitcast**（:6848-6862） | 否 | 无 | **保留**（与 §2 R2 对称，批次 299；判据 `slot_read_ids`） |
| 5 | int → float，其他来源 | `arg_sitofp`（:6863-6865） | 否 | 无 | 保留，但**应告警化**（值对、静默错） |
| 6 | float → int | `arg_fptosi`（:6875-6878） | **是**（小数；负数/OOB 为 LLVM poison） | `fptosi → iN`（:6871-6874） | **删除候选** |
| 7 | float ↔ ptr | **不转换**，原值照推（:6881-6885） | 值不匹配照样进 `build_call` | `float↔ptr (forbidden)` | **必改**：写着 forbidden 却不拒绝 |
| 8 | 其余一切（**含 ptr ↔ i64**） | 原样（:6886 `_ => push`） | 未知 | 无 | **删除候选**（今天的默认＝硬塞） |
| 9 | 实参少于形参 | 补 null/零常量（:6889-6910） | 静默补空 | 无 | **删除候选**（Python 侧应报错） |
| 10 | 实参多于形参 | `result.truncate(n_params)`（:6911-6913） | **静默丢实参** | 无 | **删除候选** |

**判定口径**：保留＝无损且语义正确；删除候选＝G.5d 收敛时改为诊断报错。
按"报告先行原则"（refactor.md §G 前言），**#2/#6/#7/#8/#9/#10 全部纳入 `abi_note`
是下一步，本批零行为变更**；#4/#5 需要的是让静默的 #5 也开口，而不是删掉它。
现行外部批评已登记在册：ARCHITECTURE-REVIEW-2026-09.md:104（"静默错值链"）、
:149（P0#2 要求收敛强转白名单）、validate.md:149（extern 声明写 i64 而收 f64 →
`fptosi` 截断，即 `abs(-2.5)`→nan 根因，复现 `tests/python_style/t27_builtins_fmt.z:18`）。

### 3.3 zeta→C 运行期

**C4 一律默认 C 调用约定**：`set_cc` / `CallConvention` / `byval` 在 codegen.rs 命中 0，
所以跨边界的一切都必须塞进 §3.1 的三类型字母表。

**C5 registry 生成的 extern 里句柄/串/容器参数一律声明为 `i64`**，只有真收
`double` 的才写 `f64`（@generated 产物 runtime_decls_registry.rs:18-24 起，
源头 pylib/registry.txt；C 侧对应 `int64_t`，例 py_additions.c:1785-1819 的
`py_argparse_*`、:2650 的 `zeta_env_get`）。
⇒ **打包/拆包责任**：调用方只保证"把值塞进一个 i64"，**类型解释全在被调方**。
每个 `W`/`F` 条目实际假设哪种表示（§1 的哪一行）由 §6（G.5c）逐条登记成表。

**C6 唯一的例外是 11 个手写 runtime 函数**（`option_*` / `host_result_*`）：它们以真
`ptr` 进出，故调用点前对**第 0 个实参**插 `inttoptr`（:4283-4291 名单 + :4303-4314），
调用后对返回插 `ptrtoint`（:4321-4326）。
⚠️ 两个事实要记住：`i == 0` 的限定意味着**第 2 个及以后的 ptr 形参不会被转换**，
以及 —— 新增 ptr 形参的 runtime 函数请走 C5 的 i64 约定，**不要扩这份名单**。

**C7 LLVM 层只有 3 处变参**：`zeta_collect_literals`（:1071-1073，注释说明"按 8 个固定
形参声明，多余交给 `coerce_call_args` 补/裁" ⇒ 实际受 §3.2#9/#10 支配）、
`printf`（:1099）、`::` 限定名 extern 兜底桩（:2492）。

### 3.4 间接调用与 Python 式形参折叠

**C8 闭包 V1 不捕获环境**：合成具名函数 `__closure_N` 直调，无环境结构体
（gen.rs:10310-10320 注释、:12930-12935）。"闭包值"就是函数地址（:12930）。

**C9 `zeta_call1(fptr, a)` 恰好一个 i64 实参**：声明 codegen.rs:1062，定义
py_additions.c:3408（把 `fptr` 强转成 `int64_t(*)(int64_t)`；**NULL → 返回 0**）。
分派守卫 gen.rs:10121-10141：`receiver.is_none()` 且 `arg_ids.len() == 1` 且名字不是
全局函数/闭包变量 ⇒ **0 参与 ≥2 参的间接调用永不走这条路**，也没有
`zeta_call2`/`zeta_callN`（grep 0 命中）。姊妹跳板 `zeta_call_fn_arg(i64,i64)`
（py_additions.c:3180；registry.txt:510 登记签名）对 NULL 是 **abort** —— 同一件事两种
失败方式，属 G.5d 收敛项。

**C10 默认值 / kwarg 在 MIR 期折叠成定长位置实参**，运行期不参与：
注入标记 `zeta_param_default(index, value)`（parser/top_level.rs:334-355）→
Resolver 收集 `param_defaults`（resolver.rs:613-637，kind 不匹配时 :629-636 告警）→
gen.rs `fill` 按**声明顺序**落槽：位置实参（:8043-8049）→ 关键字（:8050-8055）→
`**` 映射填未绑定槽（:8057-8068）→ 默认值（:8069-8078）。
C 侧 `zeta_param_default` 是**恒等 no-op**（py_additions.c:2666），存在只为让标记不成未定义符号。
现状记录（不是愿望）：**重复绑定静默后者覆盖**（:8052 `slots[i] = Some(v)` 无检查）、
**未知关键字变成多余位置实参**（:8053 `slots.push`）→ 再由 §3.2#10 静默裁掉；
只有"缺失绑定"会响（`warn_unbound` gen.rs:785-800，文案 "read 0 (Python would raise TypeError)"）。
Python 会抛 `TypeError` 的两件事，zeta 现在都不说话 ⇒ §3.2#9/#10 的删除候选就是为它们预备的。

**C11 被调方的 `*args`/`**kwargs` 没有收集语义**：解析器把它们压成一个不透明 i64 形参
（top_level.rs:71-82，注释原文 "real variadics need arg-tuple support"）；
**没有任何 C 函数把多余实参打包成元组/列表**。调用点 `f(*arr)` 仅在数组长度为字面量时
静态展开（gen.rs:8207-8243，"V1: static-size arrays compile-time unrolled"）；
日志调用里的 starred 实参明确**不展开**并告警（gen.rs:5677-5698）。
⇒ 合同推论：**任何依赖 callee 看见"全部实参"的写法目前都不成立**，写它就是写一个洞。

**C12 `PyArgNS` 是句柄类型，不是结构体**：registry.txt:440
（`W PyArgParser parse_args py_argparse_parse args=1 ret_handle=PyArgNS`）、
MIR 打类型 `Type::Named("PyArgNS")`（gen.rs:5300-5312）、消费端按字段访问分派到
`py_argparse_get_{i64,f64,bool,str}`（gen.rs:11062-11095）。
生命周期：GC 堆、进程级、无人释放（py_additions.c:1785-1819，值经 `GC_strdup` :1797）。

### 3.5 实测核对（G.5b 验收）

探针在 `/tmp`（**未入库**：新增 `tests/python_style/` 文件会动 285 这条计数，
固定为回归用例是 G.5d 的动作项）。全部用 `target/release/zetac <f> -o <bin>` AOT 实测。

| # | 探针 | 结果 | 结论 |
|---|---|---|---|
| M1 | `struct Pair{a,b}` + `fn make_pair(x,y) -> Pair` + `fn take(p: Pair)` + 交错构造两个 Pair 后回读 | `42 / 99 / 4299 / 7 / 8 / 42` 全对 | struct 返回 = i64 句柄、**无别名污染**，C1 成立 |
| M2 | `-> f64` 首返 `7` 后返 `2.5`；`-> i64` 首返 `2.5` 后返 `7` | `7.000000 / 2.500000`；`2 / 7` | **声明类型赢**（:4488-4497 由签名反查 `want_float` 后补偿）；**float→int 返回静默截断，零告警**。裁决层与 C2 的关系见附 B#4 |
| M3 | 返回只在 `if/else` 分支里、无末尾 `return` 的 `-> f64` | `2.500000 / 3.500000` | MIR 是平铺的，分支里的 `Return` 也在 `mir.stmts` 里 ⇒ C2 的"首条"含分支返回 |
| M4 | capybara COMPILER_BUGS #4 的复现：`fn get_pair() -> (i64, i64): return (42, 99)` + `let (a, b) = get_pair()` | `42 / 99`，rc=0 | **该 bug 今天不再复现**；其"返回局部指针"根因假设（COMPILER_BUGS.md:39、NOTES.md:25-28）被 M1/M4 证伪 —— 写侧本就 heap 分配（:6116、RELEASE_NOTES_v1.0.19.zeta:19）。"garbage fields"的**现行**来源是字段数解析失败时的 `("", 2)` 二字段兜底（:6336-6345 注释即记此事、:6380） |

⇒ **COMPILER_BUGS #4 应按"已不复现 + 残留风险另在"处理**，而不是继续按
"struct/元组返回是坏的"理解这套 ABI。

## 附 A. 探针反查（§1 是否穷尽）

| 探针 / 判形点 | 它假设的表示 | 表中行 |
|---|---|---|
| `map_str_key`（py_additions.c:105） | str ∈ {指针, packed}，且 packed 不可解引用 | #4 #5 |
| `zt_dyn_is_map`（:3457） | map = 块首 + 2 的幂容量 + `16+cap*24` 块 + growth forwarder | #7 |
| `zt_dyn_vec_hdr`（:3469） | vec = 数据指针 + `base-16` 头 + 紧块判据 | #6 |
| `zt_packed_cstr_eq`（:1603） | packed 的小端字节序 | #5 |
| `zeta_dyn_len`（:3492） | map → vec → 文本 的读序（无标签字的最后手段） | #6 #7 #4 |
| `zj_make` / `ZJ_*`（tokio_runtime_stub.c:2550,2560） | 16 字节 `[tag,payload]` 单元 | #8 |
| `ZT_PROBE_LOC` 系列以 `%lld` 打句柄 | 句柄就是可原样搬运的 i64 字 | #4 #6 #7 |

结论：**没有探针假设了表外的表示**；表中每一行都有至少一个探针或写侧锚点。

## 附 B. OPEN（只登记，不在本批决定）

1. **packed 的产生侧未成文**：判形只在读侧（R3），写侧由 `vec<str>` 生产者隐式
   决定。G.5d/e 需要给 str 槽定一个**可判**表示（tag 位，或统一走句柄）。
2. **PyJson 与 map 的区分靠值域**（首字 <16 vs ≥16 的 2 的幂），本质脆弱；
   响亮化目前仍在 C 侧。
3. ~~正式 §3（调用约定：参数顺序、struct 返回是拷贝还是指针、`zeta_call1` 的强转全表）
   = **G.5b**，其中 struct 返回若被认定不稳定，需当场选定一种写法并写死。~~
   ✅ **已落地 2026-09-21（批次 316）**：§3.1–§3.5 成文（C1–C12 + 强转全表 +
   M1–M4 实测）。struct/元组返回**不需要**"当场选定一种写法"——现行实现就是
   heap 句柄（§3.1 C1 写侧 :6116/:6117/:6180-6181），且 COMPILER_BUGS #4 的复现
   今天实测通过（M4）。§3.2 的"保留/白名单/删除"三档判定已给出，
   但**改动本身留给 G.5d**。
4. **返回类型有两个候选来源，优先级未成文**：`infer_fn_return_type`
   （:1376-1388）按 MIR **首条 `Return` 的值的类型**定 LLVM 返回类型，而补偿点
   （:4488-4497 反查被调签名得 `want_float`）实测按**声明的 `-> T`** 办事
   （M2：首返 `2.5` 而 `-> i64` ⇒ 得 `2`）。即"声明"确实进了签名，但**从哪条路径
   进、与 :1376 的推断谁覆盖谁**本批未定位 —— 定位前 C2 只能当"读码所得"引用。
   后果是可观测的：M2 的静默截断零告警（`abi_note` 只服务实参侧）。
   G.5d 动作项：先定位裁决层、再给返回侧补一条与 §3.2 对称的告警。
