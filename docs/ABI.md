# Zeta ABI 合同

> **G.5（refactor.md）**：把"一个槽里到底能放什么、放取怎么对称、一次调用到底传什么"
> 写成合同。本文只描述**现行实现**并逐条锚定 `file:line` —— 不是愿望清单。
> 验收口径：§1 表穷尽（没有任何运行时探针假设了表外的表示，附 A 逐个反查）、
> §2/§3 每条规则有锚点、§3 的结论经过实测核对（§3.5）。
>
> 章节进度（G.5a-e 分次独立提交；**编号已为后续章节预留**）：
> §1 槽位值表示、§2 写/读对称 = G.5a（批次 305；**批次 318 补 R7**；
> **批次 319 落地 R7 诊断 ⇒ §2 现有 R1–R7**）；
> §3 调用约定与 struct/元组返回 = G.5b（批次 316；**批次 318 更正 M2/M3 两行并关闭附 B#4**）；
> §4 名字修饰、§5 类型布局、§6 跨边界假设 = G.5c（批次 317）——
> §1 表 #6/#7/#8 的几何常量已升格为明文的布局合同（§5 L1–L9）；
> 附录 audit 表 + "先改本文再改实现"的流程规则 = G.5d（批次 319 起了头：诊断侧）；
> 符号注册表 = G.5e。
> 验收反查与未决项放在 **附 A / 附 B**，不占用正式编号；附 A 覆盖 §1（§5 是其展开版），
> §3 的对应验收物是 §3.5 的 M1–M7（批次 318 由 4 条扩到 7 条）；
> §4–§6 的未决项已登记为附 B#5/#5′/#6/#7，#8/#9 来自批次 318/319。
>
> ⚠️ **锚点会随 `codegen.rs` 的行数移动**（批次 319 实测：本文件往 codegen 里插
> 74 行，全文 19 处 `codegen.rs:` 锚点一次性错位，需按插入点重映射后逐条复核）。
> 往该文件插代码的批次，**必须同批重生成这些锚点**，否则合同里的 `file:line` 变成假证据。
>
> ⚠️ `runtime/py_additions.c` 与 `runtime/tokio_runtime_stub.c` 由并发工作流持有：
> 本文**只引用，不修改**。

## 1. 槽位值表示表

前提（codegen.rs:1595-1599）：**静态可定的本地按自身 LLVM 类型开槽**
（`Type::F32`→`float`、`Type::F64`→`double`、其余一律 `i64`）。因此"8 字节槽"
指的是**除 f32/f64 之外的所有本地槽**，以及容器（vec 元素、map 值、struct 字段）
里那些没有静态类型的**裸字** —— 后者的合法内容才是本表的主题，也是
批次 291/297/298/299/300/301 六批连发的同一类 bug 的现场。

| # | 表示 | 槽内编码 | 写侧（产生处） | 读侧要求 | 混淆史（实测） |
|---|---|---|---|---|---|
| 1 | `i64` | 裸二补码 | `MirExpr::IntLit` | 原样 | — |
| 2 | `f64` | **IEEE-754 位模式**，不是裸整数 | ① 静态 `double` 槽：`Assign` 时 int 值走 `slot_sitofp`（codegen.rs:3293-3318）；② struct 字段是 64 位槽：存前 bitcast（codegen.rs:6177） | 从**裸容器字**读回必须是 `bitcast`，不能 `sitofp`（`slot_or_sitofp` codegen.rs:1702-1717，判据 `slot_read_ids` 60 / 1593） | 批次 298：裸存 → 读回 4.94e-322；批次 299：缺 `sitofp` → `500.0 + 50` 算成 `4.6e18`（5747 记录） |
| 3 | `bool` | **i64 的 0/1** | 比较结果立刻 `zext i1→i64`（`eq_ext`/`ne_ext`/…，codegen.rs:3748-3756） | 一律 `!= 0` | `Type::Bool → bool_type`（7357）只在 ABI 边界（参数/返回签名）出现；**i1 从不进槽** |
| 4 | `str`（指针形） | `.rodata` 或 GC 块首地址，`ptrtoint` 成 i64 | `MirExpr::StringLit`：私有 global 字节数组 + `ptrtoint`（codegen.rs:5667-5686） | 可直接解引用；`GC_base(h) != 0` ⇒ 堆串 | 与 #5 不可静态区分 ⇒ 见 R3 |
| 5 | `str`（packed 形） | 短 ASCII **按字节小端打进这 8 个字节本身**（实测样本 `"43601853"` = `0x3335383130363334`） | `vec<str>` 的元素槽（批次 291 实测；写侧仍是隐式决定，见附 B OPEN） | **必须先判形再解引用**；唯一的判形点在 `map_str_key`（py_additions.c:105-129）：`GC_base` 命中→按内容 FNV 哈希；否则 `u < 2^32 \|\| u >= 2^48`→判为 packed、原样当不透明键；再否则 `vm_read_overwrite` 探一页可读性 | 把 packed 当 `char*` 传给 `str_trim` → 约 25% 概率 SEGV（批次 297）；packed 之间比较走 `zt_packed_cstr_eq`（:1603） |
| 6 | `vec` 句柄 | 指向**数据**；`[cap \| len]` 头在 `base-16`；块 ≥ `16+cap*8` | `zeta_collect_vec_n` / `str_split` / df 列构造器（多数按元素数申请，故 cap 常 < 8） | `zt_dyn_vec_hdr`（py_additions.c:3469-3491）：`cap∈[1,2^28]`、`len ≤ cap`、块够大；**短 vec（cap<8）还必须是"紧块"** —— GC 向上取整的余量恰为 8 或 16，`have > need+16` 即判否 | 批次 301：`cap >= 8` 曾是唯一判据 ⇒ 1..7 元素列表被拒，`len()` 退化成对数据字做 `strnlen`（实测 3 元素报 0；1/4/8 报 5/0/8） |
| 7 | `map` 句柄 | **块首**；`w[0]=cap`（2 的幂、≥16、≤2^28）、`w[1]=used ≤ cap`；块 ≥ `16+cap*24`；`cap < 0` ⇒ growth forwarder（真表在 `w[1]`） | `MapNew` / `DictInsert` | `zt_dyn_is_map`（py_additions.c:3457-3467）+ `GC_size` 兜底 | 与 #8 撞车：map 把 PyJson 的 tag 当容量（见 #8） |
| 8 | `PyJson` | 16 字节 `[tag, payload]` **单元指针**；tag = `ZJ_NULL 0 / INT 1 / F64 2 / STR 3 / ARR 4 / OBJ 5 / BOOL 6`（tokio_runtime_stub.c:2550-2556，构造 `zj_make` :2560） | `json.loads` 一族 | 访问器按运行时 tag 分派（sum type；**不是**外层程序的动态分发） | `-> dict` 注解把 Json 当 map ⇒ 首字（tag ≤ 6）被当容量，`idx = hash & (cap-1)` 死循环（tokio_runtime_stub.c:190-196 记录；现行以"map 首字 ≥16、Json 首字 1..8"作值域区分并响亮报告） |
| 9 | 用户 `struct` | GC 句柄，字段各占一个 64 位槽 | `StructNew`（#2②） | 字段读 = 槽偏移 + 按字段静态类型还原（f64 见 #2） | 批次 300：未标注返回类型退化成 unit ⇒ 字段读成"空 variant 的第 0 字段"，**每个属性读都静默拿垃圾** |
| 10 | `PyDynamic`（预留） | boxed tag cell | — | — | B/T3 落地时更新本行。当前 dyn 值就是 #4–#8 那些**无标签字** —— 这正是 #6/#7 必须做几何判形的根本原因 |

## 2. 写/读对称规则

**R1 整数入浮点槽 = 值转换（`sitofp`），不是位保留。**
锚点 codegen.rs:3293-3318（`MirStmt::Assign` 的 `slot_sitofp`）。
反向不存在"浮点入整数槽"：浮点要么进 `double` 槽（值），要么进 64 位裸槽（位模式，R2）。
⚠️ 该断言的适用范围 = **赋值路径**。批次 318 实测：返回路径上确有"浮点写入、整数读出"
的同槽混型（M5 打印 `4612811918334230528`），另立新规则 **R7** 覆盖，不改动本条。

**R2 裸容器字里的 f64 出槽 = `bitcast` 回读。**
锚点 `slot_or_sitofp`（codegen.rs:1702-1717）——同一个 helper 二选一，判据是
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
锚点 codegen.rs:3748-3756。容器真值另有一套按长度的判定（批次 297），
两者不互相转换 —— "非空 dict" 是 `len>0`，不是 `!= 0`。

**R6 struct 字段一律 64 位槽；宽度≠8 的类型进/出槽各 bitcast 一次。**
锚点 codegen.rs:6177（存侧注释）+ #2 读侧。

**R7（批次 318 补）返回值也是一次跨槽写入：定义侧 LLVM 签名型 == 调用侧 dest 槽型。**
这条今天**正被违反**（批次 318 时零诊断，批次 319 起出声 ⇒ 见本节末），
它是 §2 的盲区而非新事实——R1/R2 只写了"赋值入槽"和
"容器字出槽"，没写"返回值入调用方槽"。实测两种形状（`/tmp/abi8/`，§3.5 M5/M6）：
`-> f64` + `return 7` ⇒ 槽按 `i64` 写、按 `double` 读 ⇒ **位保留而非 `sitofp`**，
即 **R1 的直接违例**出现在返回路径（打印 `0.000000`）；`-> i64` + `return 2.5` ⇒
槽按 `double` 写、按 `i64` 读 ⇒ 落在 R1 自述"反向不存在"的那一格（打印
`4612811918334230528`）⇒ **R1 的"反向不存在"这一断言对返回路径不成立，本批更正**。
违反 R7 的根因与修复见 §3.1 C2 与附 B#8（任务 #33）。

**诊断已落地（批次 319，任务 #33 第①步：只出声、不生成任何 IR）**：每个
`MirStmt::Call` 在 lowering 前先比"被调方 LLVM 返回型 vs 调用方 dest 槽的 MIR 型"，
不一致即 `warning: ABI return in call to \`f\`: callee returns float, caller's dest
slot is int`（锚点 codegen.rs:3327 调用点、:6960 实现），并在 `report_abi_coercions`
里按**独立计数器**汇总（`abi_ret_warn_count`，与 §3.2 的 `abi_warn_count` 分列——
两条规则、两个边界，合并会互相掩盖出现率）。覆盖面与边界如实写在这里，不粉饰：
① 只对 **Zeta 自定义函数**生效（`zeta_fn_names` 在 §3.1 的预声明环节登记）——C 运行期
符号的签名就是真值，槽型不合不属本缺陷；② 判据只覆盖 float↔int 两个方向，故 M7
那类"签名与槽一致、函数体内嵌套 return 被 `fptosi` 折算"**不在本诊断范围内**（那属
§3.2 表 #6/#7）；③ 单态化产物（`specialized_fns`）未登记 ⇒ 泛型调用暂不覆盖。
实测出现率（同一份二进制、逐文件单独捕获 stderr——必须自己扫，门禁日志读不出这个，
理由见附 B#9）：
`tests/unit-tests` **0/194**、`tests/python_style` 顶层 12 例 **0/12**、
真实语料 `~/source/quant/REasyQuant/strategies` **6/38 文件命中**。6 条命中抽查后
分两类，严重度不同：`ExecutionAdapter::nav_value/equity_value/available_cash`
（`-> float` 但函数体只有一句 docstring ⇒ 合成返回是 i64 零 ⇒ 读成 double 恰为
`0.0`，**数值上良性**；真问题是"调用为何绑到抽象桩"，那是分发缺陷不属 ABI）；
`get_volume_ratio_4` / `MOM_1` / `__closure_0_extract_metrics_from_analyzer_*`
（float 返回写进 int 槽 ⇒ 与 M5 同形的**垃圾值**）。逐文件定级归任务 #33 第②步。

## 3. 调用约定与 struct/元组返回（G.5b）

**先划边界**：本文的"调用约定"是三个不同的边界，混起来谈是本章之前的常见错误。
§3.1 zeta→zeta（同一 LLVM module 内的用户函数）、§3.2 实参强转全表、
§3.3 zeta→C 运行期、§3.4 间接调用与 Python 式形参折叠、§3.5 实测核对。
**本章只描述现行实现**；§3.2 的"判定"列是 G.5d 收敛的靶子，不是本批的行为变更。

### 3.1 zeta→zeta：签名字母表只有三个类型

**C1 参数与返回的 LLVM 类型只能是 `i64` / `f32` / `f64`。**
锚点：参数映射 codegen.rs:1467-1474（`Some(Type::F32)`→`f32`、`Some(Type::F64)`→`f64`、
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

**C2 返回类型有两个各自独立的来源，谁也不覆盖谁**（裁决层已于批次 318 定位，附 B#4 关闭）：
- **定义侧**（LLVM 签名）= `infer_fn_return_type`（:1376-1388）扫 `mir.stmts`，取
  **首条顶层 `Return`** 值在 `type_map` 里的类型，扫不到默认 `i64`（:1389）。
  声明的 `-> T` **完全不参与**——`Mir` 结构里根本没有 return_type 字段
  （`src/middle/mir/mod.rs`、`src/middle/mir/gen.rs` grep `return_type` **0 命中**）。
  ⚠️ **MIR 并非全平铺**：`If { then:[Return…] }` 是**嵌套 stmt，扫不到**。分支函数今天
  多数仍正确，是因为 MIR gen 会给带 `dest` 的 `If` **合成一条顶层尾 `Return{dest}`**，
  其 `type_map` 是各分支类型的合并值（实测 `/tmp/abi8/m3.z --dump-mir`：
  `If{…, dest: Some(7)}` + 顶层 `Return{val:7}` + `7: F64` ⇒ `define double @h`，
  `2.5/3.5` 全对）。**`dest: None` 时没有这条合成**（`m2b.z`：`If{dest:None}` 里的
  `return x`(F64) 被跳过，签名由顶层 `return 7`(I64) 定 ⇒ `define i64 @f`）
  ——这才是 :4520 注释"nested ones are never consulted"的确切适用面。
- **调用侧**（怎么读返回值）= MIR 生成期写进 **call dest 槽** 的 `type_map` 条目，
  它来自 resolver 的声明。实测（`/tmp/abi8/a.z --dump-mir`）：`-> i64` ⇒ main 里
  dest 槽是 `4: I64`，而被调函数自己的 MIR 里返回值是 `1: F64` —— **两份真相同时存在**。
⇒ 两者**没有一致性检查**。冲突时的形状**不是截断，是同槽混型的按位重解读**：
  - `-> i64` + 无条件 `return 2.5` ⇒ `define double @f()`（`a.ir`:1068、`ret double`:1071），
    调用点 `store double %12` 后 `load i64`（`a.ir`:1091-1092）⇒ 打印
    **`4612811918334230528`**（= 2.5 的 IEEE-754 位模式），rc=0、**零诊断**
    （批次 318 实测原貌；319 起该形状出 `warning: ABI return`，见 §2 R7 末）。
  - `-> f64` + `return 7` ⇒ `define i64 @g()`（`b.ir`:1070），调用点
    `call i64 @g()` 之后 `load double`（:1092-1094）⇒ 打印 `0.000000`
    （整数 7 的位模式是非规格化 double）。
  这正是 **§2 R7**（= R1 的对称性推到返回路径）所禁止的形状。
- §3.5 M2 当时报"截断成 `2`"，走的是**第三条路径**：那条用例的两个 `return` 分处分支，
  **末条顶层** `return 7` 把签名定成 i64，分支里的 `return x`（double）被 C3 的补偿点
  `fptosi` ⇒ `2`。据此写的"声明类型赢"是**误判**，本批更正：赢的是"首条顶层 Return 的
  推断"，与声明一致纯属巧合。

**C3 补偿发生在返回语句处：`ret_sitofp` / `ret_fptosi`（:4488-4530）。**
读的是**当前正在生成的函数自身**的 LLVM 签名
（:4488-4497 `builder.get_insert_block().get_parent()` → `get_return_type()`），
**不是**"从被调方签名反查"（此处为批次 318 的更正；原表述会让人以为调用侧也走同一条链）。
**补偿是有损的且不告警**；`abi_note` 只服务实参侧（§3.2），返回侧今天无任何诊断。
⇒ **G.5d 的修复靶心已明确**：让定义侧也服从声明（MIR 携带 return_type 到定义侧，
或 codegen 反查 resolver 声明来定签名），并在"声明 ≠ 首条顶层 Return 的推断"时
发一条与 §3.2 对称的诊断——今天这两种错法分别是**静默按位重解读**（C2）与
**静默有损转换**（C3）。

### 3.2 实参强转全表（`coerce_call_args`）

锚点 codegen.rs:6810-6935；告警器 `abi_note` 6937-6951（stderr 最多 8 条，
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

**C9 `zeta_call1(fptr, a)` 恰好一个 i64 实参**：声明 codegen.rs:1069，定义
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
固定为回归用例是 G.5d 的动作项）。全部用 `target/release/zetac <f> -o <bin>` AOT 实测；
M1–M4 在 `/tmp/abi3_*`（批次 316），M5–M7 在 `/tmp/abi8/`（批次 318，
同目录另有 `--emit-llvm` 落盘的 `a.ir`/`b.ir`/`m2b.ir`，本表引用的 IR 行号即出自它们）。

| # | 探针 | 结果 | 结论 |
|---|---|---|---|
| M1 | `struct Pair{a,b}` + `fn make_pair(x,y) -> Pair` + `fn take(p: Pair)` + 交错构造两个 Pair 后回读 | `42 / 99 / 4299 / 7 / 8 / 42` 全对 | struct 返回 = i64 句柄、**无别名污染**，C1 成立 |
| M2 | `-> f64` 首返 `7` 后返 `2.5`；`-> i64` 首返 `2.5` 后返 `7` | `7.000000 / 2.500000`；`2 / 7` | ⚠️ **本行原结论"声明类型赢"已于批次 318 更正为误判**（真机制见 M7 与 C2）：签名取自**顶层** `Return` 的推断，该用例里恰与声明同形。"float→int 返回静默有损、零告警"这一条**当时成立**（告警部分已于批次 319 补齐） |
| M3 | 返回只在 `if/else` 分支里、无末尾 `return` 的 `-> f64` | `2.500000 / 3.500000` | ⚠️ **机制已于批次 318 更正**：不是"C2 的'首条'含分支返回"——嵌套 `Return` 扫不到；是 MIR gen 为带 `dest` 的 `If` **合成**了顶层尾 `Return{dest}`，其 `type_map` 是分支类型的合并值（`7: F64`）。`dest: None` 时没有这条合成 ⇒ 见 M7 |
| M5 | `-> i64`，函数体**只有一条** `return 2.5`（`/tmp/abi8/a.z`） | 打印 **`4612811918334230528`**，rc=0，零诊断（319 起出声） | **两源冲突的真实形状 = 按位重解读，不是截断**：定义侧 `define double @f()`（`a.ir`:1068、`ret double 2.5`:1071），调用侧按声明读同一槽（`store double %12`:1091 → `load i64`:1092 → `println_i64`:1099）。那个整数就是 2.5 的 IEEE-754 位模式。⇒ 违 **§2 R7**（并暴露 R1"反向不存在"的断言对返回路径不成立） |
| M6 | `-> f64`，函数体只有一条 `return 7`（`/tmp/abi8/b.z`） | 打印 `0.000000` | M5 的对称方向：`define i64 @g()`（`b.ir`:1070）而调用点 `load double`（:1092-1094）⇒ 整数 7 的位模式是非规格化 double。**这是 R1 的直接违例**（整数入浮点槽必须 `sitofp`，此处按位保留）；M5/M6 合起来 ⇒ 声明与实现不一致时**两个方向都静默** |
| M7 | `-> i64`，`if x > 2.0: return x`（**无 else**）后接顶层 `return 7`（`/tmp/abi8/m2b.z`） | `r = 2`；IR 里是 `define i64 @f()` + `%ret_fptosi = fptosi double %6 to i64`（`m2b.ir`:1066、:1082-1083） | **这才是 M2 当年看到的"截断"**：`If{dest:None}` 里的 F64 返回对扫描不可见，签名由顶层 `return 7` 定成 i64，嵌套返回被 C3 补偿掉。与 M5 对照即可证明**"声明"从未参与定签名** |
| M4 | capybara COMPILER_BUGS #4 的复现：`fn get_pair() -> (i64, i64): return (42, 99)` + `let (a, b) = get_pair()` | `42 / 99`，rc=0 | **该 bug 今天不再复现**；其"返回局部指针"根因假设（COMPILER_BUGS.md:39、NOTES.md:25-28）被 M1/M4 证伪 —— 写侧本就 heap 分配（:6116、RELEASE_NOTES_v1.0.19.zeta:19）。"garbage fields"的**现行**来源是字段数解析失败时的 `("", 2)` 二字段兜底（:6336-6345 注释即记此事、:6380） |

⇒ **COMPILER_BUGS #4 应按"已不复现 + 残留风险另在"处理**，而不是继续按
"struct/元组返回是坏的"理解这套 ABI。

## 4. 名字修饰（G.5c 第一批）

**先分轴**——三条互相独立的命名轴，混谈是这类 bug 的共同形状：
**轴一** 源级名 → LLVM 符号名（编译器内）；**轴二** LLVM 符号名 → 链接符号
（LLVM 自己的 `.N` 改名 + Darwin 前导 `_`）；**轴三** 注册名 → C 实现
（registry 与 `__asm__` 魔法名）。

### 4.1 轴一的写侧：四种拼写，没有一种可逆

**N1 模块限定的规范形是 `<module 的点换成下划线>__<member>`。**
锚点 resolver.rs:1887、:2639；mir/gen.rs:388、:427、:539、:546、:3120。
构造就是两次 `replace`，**没有转义**：`__` 既是分隔符又可能出现在 member 里，
点号也会把 `a.b` 和 `a__b` 映到同一串。判"这是不是模块限定名"目前只有
`actual_name.contains("__")`（codegen.rs:2889）。⇒ 合同推论：**member 名含 `__`
即破坏可逆性**（无防护，登记附 B#5）。

**N2 泛型单态化形是 `<base>_inst_<T1_T2…>`**：specialization.rs:27
（`format!("{}_inst_{}", func_name, type_args.join("_"))`）+ codegen.rs
`mangle_function_name` :1346-1356。与 N1 共用 `_` 作类型名内部字符和分隔符，同样不可逆。

**N3 重载消歧形是 `<name>_<实参数>`，由 MIR 生成侧加**：gen.rs:10385、:11684
（`format!("{}_{}", func, arg_ids.len())`，注释 :10360-10366 自述"后缀只为区分重载，
因此读取侧必须能剥掉它"）。**读侧要靠剥后缀还原** ⇒ N3 的代价全在 §4.2 的瀑布里。

**N4 `str_*` 方法在符号层重写成 `host_str_*`**：codegen.rs:2603-2609
（前缀判定 + `resolve_string_method`）。这是一条**隐式命名改写**：源里写 `str_trim`，
链接期要找 `host_str_trim`。

### 4.2 轴一的读侧：`get_or_declare_function` 瀑布（313 行 / 实测 18 档）

**N5 一个调用点的符号名是一条有序尝试序列的结果，不是函数的输出。**
锚点 codegen.rs:2594-2906（函数体 313 行）。ARCHITECTURE-REVIEW-2026-09.md:102
记为"8 级"，本批逐档数出 **18 档**：

| 档 | 前提 | 尝试的名字 | 锚点 | 判性 |
|---|---|---|---|---|
| 1 | 名含 `::` | 精确限定名原样 | :2594-2602 | **承重墙**：注释原文要求"先试精确形，**不要**退回 mangle/裸方法名"——退回过，`a.column("code")[1]` 把一个 Vec 当 map 索引，SEGV（:2595-2599） |
| 2 | 名含 `::` | `::`→`__` | :2603-2609 | 承重墙（N6 的双态） |
| 3 | 名含 `::` | 裸方法名 + 实参数校验 | :2612-2618 | 事故现场：跨模块同名方法可被匹配（附 B#5′） |
| 4 | 名含 `::` | 裸方法名 + `_N` | :2620-2623 | 同 3 |
| 5 | — | `host_*` | :2626-2634 | N4 |
| 6 | — | 裸名 + 实参数校验 | :2636-2647 | 承重墙 |
| 7 | — | `name_N` | :2648-2656 | N3 的反向 |
| 8 | `type_args` 空 | 泛型默认实例化（参数全按 i64） | :2657-2664 | 承重墙 |
| 9 | `type_args` 非空 | `_inst_` mangle | :2665-2669 | N2 |
| 10 | 同上 | 剥 `_<数字>` 后 monomorphize | :2671-2683 | N3 反向 |
| 11 | 同上 | 剥尾缀再 `_inst_` | :2684-2696 | 注释点名 `oneshot::channel_0` |
| 12 | 同上 | 只用方法名 mangle（含再剥尾缀） | :2697-2716 | 事故现场 |
| 13 | 同上 | 限定路径 mangle + `::`→`__`（含再剥尾缀） | :2717-2741 | 事故现场 |
| 14 | 同上 | `host_*` / 裸名 | :2742-2751 | — |
| 15 | 汇流 | `name.<N>`（LLVM 改名） | :2753-2758 | 轴二，见 N8 |
| 16 | 汇流 | `name_N` 再试 | :2759-2766 | — |
| 17 | 汇流 | 剥尾缀 `_N` 试基名（**必须全数字**） | :2768-2792 | 承重墙：护栏注释原文——没有它 `to_string_f64` 曾被当成 `to_string`+后缀而静默改名（:2774-2777） |
| 18 | 全部落空 | **就地声明 extern** | :2796-2887 | 见 N6 |

**N6 瀑布的最后一档不是报错，是"猜一个签名出来"。**
锚点 :2883-2886 —— 兜底 extern 的签名一律是 `i64(i64, i64, …)`（参数个数 = 本调用点
实参数，变参位 `false`）。⇒ 合同级推论：**名字没对上时编译器不会说话**，它会发出一个
签名可能与定义不符的调用，错值再被 §3.2 的强转表静默"修好"。这就是
ARCHITECTURE-REVIEW:104 那条"静默错值链"的上游。
推论（收敛方向）：新增解析档位＝扩大错配面积；**响亮化（G.5d）优先于新增档位**。

**N7 `::` 与 `__` 双态是并存的事实，不是待修的笔误。**
存储侧写 `__`、调用点引用带 `::`（:2581-2582 注释原文），全仓有 4 处
`name.replace("::", "__")`：:2217、:2440、:2489（注释）、:2724。
但 capybara 的 bug 记录说的是**相反**的存储形：
"gen_mirs stores functions with `::` separator"（COMPILER_BUGS.md:31），且它观察到的
现象确实依赖顺序（:29、:33）。⇒ 本批判定：**两态都由 N5 的瀑布吸收**，
即"谁先被声明谁定形"；这条不是文档能修的，登记为 **G.5e 的靶心**
（收敛成"符号名一次定型，读侧只查表"）。

### 4.3 轴二：LLVM 的 `.N` 改名与 `.set` 别名表

**N8 LLVM 在同名冲突时把定义改名成 `name.N`，运行期靠 C 侧 `.set` 别名追认。**
读侧档位 :2753-2758；别名表 runtime/aliases.inc.c —— **恰好 66 条 `.set`**
（本批实测计数；行 4-69），形如
`".globl _print.13\n\t.set _print.13, _print2\n"`（aliases.inc.c:12），
由 tools/gen_from_registry.py:290 从 pylib/runtime_aliases.txt（69 行）生成，
经 tokio_runtime_stub.c:356-358 `#include "aliases.inc.c"` 进入编译单元。

**N9 `.N` 里的 N 不是 ABI，是"LLVM 在本 module 内第几次改名"的偶然计数。**
⇒ 别名表与**IR 发射顺序**是一对锁死件：src/main.rs:776-779 的注释原文——
HashMap 迭代顺序随机 ⇒ `print.N` 冲突改名和运行期别名表"从一次运行到下一次
在能用与不能用之间翻转"，:780 的 `all_mirs.sort_by(...)` 就是这把锁的钥匙。
**合同级：确定性发射序是 ABI 的一部分，不是代码风格。**
（stub:339、:343-344 的注释是这条的现场记录：`array_new_1` → `array_new.10`、
`print` → `print.N` 且 N 随 arity 1-6 变动。）

**N10 两类名字豁免于 `.N` 改名路径**：`zeta_*` 运行期分派名与含 `__` 的模块限定名
（:2861-2872 命中即直接复用已有声明）。注释列了豁免前真实产生的 4 个不可满足符号
（`__get_price_3`/`__get_price_7`/`__get_trade_days_2`/`__OrderCost_6`）——
默认参数让调用点实参数与声明实参数长期不一致，arity 后缀因此是**错的**。

### 4.4 轴三：非法标识符的 `__asm__` 魔法名

**N11 `[dynamic]<T>__<method>` 是类型打印器造出来的"符号名"，C 侧只能起别名接住。**
`Type::DynamicArray(inner)` 的 `display_name()` 就是 `[dynamic]{inner}`
（types/mod.rs:768）；分派侧用 `::` 形（pylib.rs:806
`dispatched_member("[dynamic]str::isin")`），落到 MIR 时是 `__` 形
（gen.rs:8713 `func: "[dynamic]str__map"`）；C 侧的实现叫 `zt_dyn_str_map`，
靠 `__asm__("_\\[dynamic\\]str__map")` 顶这个名字（py_additions.c:967）。
⇒ **每个动态方法都要人肉注册一个魔法名**（refactor.md:130 已把它列为承重墙），
且已经有只造了名、没注册实现的变体（gen.rs:8609/:8647/:8666/:12315 注释里的
`[dynamic]str__max/isna/pct_change`——链接期报 undefined 才算发现）。
**G.5e 目标**：把"方法名 → 符号"从字符串拼接换成注册表查表，未注册即编译期报错。

### 4.5 "新增一个运行期函数要改几处"：现状仍是手工同步，且有一份生成物从未接线

ARCHITECTURE-REVIEW:103 记的是"四份符号表手工同步"（①codegen 声明 ②gen.rs 分发
③C 实现 ④`.set` 别名，另加 JIT 的 `add_global_mapping`）。本批核对现状：

| 处 | 现状 | 锚点 |
|---|---|---|
| ① LLVM 声明 | **仍是手写**：codegen.rs 内 255 处 `add_function`（实测计数） | 例 codegen.rs:1069、1071、1078 |
| ①′ 生成物 | `runtime_decls_registry.rs`（294 处 `add_function`）+ `runtime_decls_core.rs`（61 处）由 `--emit`/`--emit-core` 生成，**两个入口函数从未被调用** | mod.rs:7、:9 只声明模块；`declare_registry_runtime_fns`/`declare_core_runtime_fns` callers **图内无边**（codegraph）+ grep 全仓仅定义处与一处注释（pylib.rs:685） |
| ② gen.rs 分发 | 手工 | 例 gen.rs:8713、:8965 |
| ③ C 实现 | 手工 | 例 py_additions.c:967、:3408 |
| ④ `.set` 别名 | 已生成（数据 `pylib/runtime_aliases.txt`，其 :1 注释自述"Generated/**edited by hand**"） | aliases.inc.c:1-2 |
| ⑤ JIT 绑定表 | 已生成**且已接线**（批次 315 用的就是它） | jit_mappings_gen.rs + jit.rs |

⇒ 结论（推翻 review 的一半）：**"单一生成器"只完成了 ④⑤ 两条，①′ 是并行的第二份
而未接管 ①**。这不是新发现的暗雷——机器判据早就登记了它：
`tools/baselines/dc_default.txt:2-3` 两条 `function ... is never used`
（`#![allow(dead_code)]` 屏蔽了 rustc 的报错，只有 `tools/dc_audit.sh` 绕开后才看得见）。
**G.5e 决策项**：要么接线（①′ 取代 ①），要么删除生成物并承认声明是手写的——
现状是最坏的一种：两份表并存、只有一份生效、且看起来已经收敛了。

**幽灵符号案例更新**：review:115 举的 `py_asdict_unexpanded`（原锚 registry.txt:208）
现已不是幽灵——条目在 :219（带 `stub=1`），C 侧有定义且**响亮失败**
（`py_stub_abort`，tokio_runtime_stub.c:1278-1282）。
但"**registry 是纯字符串、无校验**"这条仍然成立，逐条见 §6（本批下一段）。


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

## 5. 类型布局（G.5c 第二批）

这一章把 §1 表 #6/#7/#8 里"判形探针所依赖的几何"升格为**明文布局合同**。
用途有二：① 改任何容器布局前先照这张表（R4 的"改布局就是改本章"在此落地成常量表）；
② 轴 B（运行时探针）退役时按图索骥——探针删掉之后，这些不变量只剩文档在守。

**L1 唯一可信的边界是 GC 块本身。**
两个原语：`GC_base(p) == p` 读作"p 是某个块的起点"，`GC_size(p)` 读作"该块的可用字节数"
（py_additions.c:3459、:3476）。**GC 会把申请量向上取整**，实测余量恰为 8 或 16
（:3485-3487，cap 1..20 逐个量过）。
⇒ 合同：**任何"块应该多大"的判据必须写成 `size >= need` 或
`size - need ∈ {8,16}` 这种带余量的形式，不得写等号。**

**L2 `vec`：句柄 = 数据指针，头 `[cap|len]` 在 `base-16`，且头自己就是一个块首。**
- 判形侧：`hdr = (int64_t*)(h - 16)` → `GC_base(hdr) == hdr`（:3471-3472）、
  `cap ∈ [1, 2^28]`、`len ∈ [0, cap]`（:3474）、`need = 16 + cap*8`（:3475）、
  短 vec（cap<8）另需块是紧的（:3488）。
- **写侧三行不变量**（生产者必须照此）：`GC_malloc(16 + cap*8)` →
  `base[0] = cap; base[1] = 0` → **返回 `base + 2`**。
  锚点：`str_split`（:68、:82；空/极短路径也按 cap=8 开：:61）、
  `zeta_dynarray_new`（:2580、:2584）、`zeta_collect_vec_n`
  （:2708 `base[0] = len ? len : 8`；长度未知时从 `iter-16` 反读：:2706）。
- 读长度 = `((int64_t*)(data - 16))[1]`（切片侧 :191）。
⇒ 可测推论：**句柄永远不等于分配指针**，`GC_base(句柄) != 句柄` 恒成立。

**L3 `map`：句柄 = 块首；`[cap|used]` 在块首，桶区从 `+16` 起，每桶 24 字节。**
- `cap` 是 2 的幂、`∈ [16, 2^28]`（:3463）、`used ∈ [0, cap]`（:3465）、
  块 ≥ `16 + cap*24`（:3466）。
- 桶 = 3 个字 `[key | value | used]`，**`used` 只占第三个字的低字节**
  （`*(uint8_t*)(e + 16)`，:222-223；同一套地址算术 :3152、:3160）。
  `#define MAP_ENTRY_SIZE 24`（tokio_runtime_stub.c:169）、
  分配 `GC_malloc(16 + cap*MAP_ENTRY_SIZE)`（:178）。
- **扩容转发器**：旧块 `w[0] = MAP_MOVED`（= `-1`，stub:228）、`w[1] = 新块地址`
  （stub:184、:251）。判形侧承认它：`cap < 0` 时改看 `w[1] > 0x1000`（:3462）。
  历史事故（勿重犯）：把大表 `memcpy` 回旧的小块 = 堆破坏，注释原文记了
  "GC Warning: Failed to expand heap by … KiB" 与 50 KB 字典在 `zj_parse_value` 里崩
  （stub:186-189）。

**L4 `PyJson`：16 字节 `[tag, payload]` 单元，payload 指向 L2/L3 的句柄。**
tag 取值 `ZJ_NULL 0 / INT 1 / F64 2 / STR 3 / ARR 4 / OBJ 5 / BOOL 6`
（stub:2550-2556），分配 `GC_malloc(16); h[0]=tag; h[1]=payload`（:2561），
读侧 `((int64_t*)j)[0]` / `…[1]`（:2566）。`ARR` 的 payload 就是一个 L2 vec
（:2542；同一套 `16 + cap*8`，:2571-2574），`OBJ` 的 payload 就是一个 L3 map、
键按 `map_str_key` 归一（:2543）。

**L5 用户 `struct` / 元组：8 字节字段数组。**
struct 是 GC 块、字段各占一个 64 位槽，读侧用**非 packed** 的
`struct_type(&[i64; N], false)` 整块 load 再 `extract_value`
（codegen.rs:6426-6433），写侧是地址算术 `base + idx*8`（5411-5428）。
元组复用 L2 的 `[cap|len|elems…]`（:6630-6690）。

**L6 packed 短串：ASCII ≤7 字节按字节小端打进这 8 个字节本身，余下为 NUL。**
`if (n > 7) return 0;` + 逐字节比 `&packed`（py_additions.c:1605-1606）；
两形不可区分时先分形再 `strcmp`（:1617）；排序按字节逐个归一，可打印区间
`0x20..0x7f`（`u >> (8*i)`，:1633-1634）。

**L7 地址窗口假设＝平台合同（本表最脆的一行）。**
整套判形依赖"macOS arm64 的 `__PAGEZERO` 使 4 GiB 以下不可映射"（:117 注释原文），
具体落点 5 处：`u < 2^32 || u >= 2^48` 判 packed（:120）、小整数闸门
`h <= 0x1000`（:3458、:3470）、文本闸门 `h > 0x100000000`（:3539）、
用 `vm_read_overwrite` 探一页可读性而**不是** `msync`（:121-125，注释理由
"msync 只证明映射存在，不证明可读"）。
⇒ **换地址模型（x86-64 Linux、57-bit VA、CHERI）时这 5 处必须一起改**——
附 B#2 所说"脆弱"的具体形状就是这一行；移植前先读它。

**L8 "回看 `h-16`" 本身就是一次越界风险。**
读 `h-16` 前必须先证 `h ≥ 块首 + 16`：map 句柄**就是**块首，块若正好落在页起点，
`h-16` 落在 guard page（实测 SIGBUS in `map_insert+88`，driver batch 291）。
这段因果写在 tokio_runtime_stub.c:204-211，代码先取 `GC_base(h)` 再判偏移。

**L9 同尺寸≠同含义：24 字节块有两种互不相干的用途。**
argparse 的每条实参是裸三元组 `GC_malloc(24)` = `[dest | flag | default]`
（py_additions.c:1760，读回成三个 `char*`：:1789；解析器本体又是一个 cap=8 的 L2 vec：
:1751），而 L3 的桶也是 24 字节。
⇒ 合同推论：**尺寸不是判形依据**；判形只认 L1-L3 的字内容 + 块大小关系。
新增小对象时不要指望"这个尺寸没人用"。

## 6. 跨边界假设（G.5c 第三批）

### 6.1 一份表，两个解析器

`pylib/registry.txt`（525 行）自述语法在 :4-:17：
`F <module> <member> <symbol> args=… ret=… [handle=<Tag>]`（:7）、
`W <handle> <method> <symbol> args=<n> [ret_handle=<Tag>]`，**n 含 receiver**（:9，
:10 解释为"按 handle tag 分派方法"）、`X <symbol> args=… ret=…`（裸符号显式形状，
例 :99）。实测分布：**F 192 / W 93 / X 81 = 366 条可调用**，另有 M 29、A 1、N 1。
它被读两遍，**各写一套解析**：
① 编译期分发（Rust）——`include_str!` 把整表嵌进二进制（pylib.rs:22），按首字母
分派（`N` :115、`A` :129、`X` :185、`W` :216、字段 `ret_handle` :240）；
② 声明/别名/JIT 表（Python）——`kind not in ("F","W","X"): continue`
（gen_from_registry.py:39），W 的数字 arity 展开成 `["i64"] * n`（:53、:60-71）。
⇒ **两份实现同一语义、彼此不校验**。这是 G.5e（符号注册表）要收敛的正主。

### 6.2 类型 token 的实际权重：只有两个 token 有法律效力

- 实参侧合法 token 实测只有 `i64`（288 次）与 `f64`（59 次），加上 W 的裸数字 arity
  （`args=1`×60、`2`×24、`3`×6、`4`×3）与 45 条空 `args=`（另有 2 条是 :7/:9 的格式示例）。
- 返回侧写法有 11 种：`i64`172 / `f64`48 / `str`44 / `vec`17 / `vecstr`8 / `void`3 /
  `vecpath`2 / `vecmatch`2 / `vecjson`1 / `map`1。
- 但发射时**除 `f64`、`void` 之外一律 `i64_type`**（gen_from_registry.py:116-128）。
⇒ 那 75 个语义 token（`str`/`vec`/`vecstr`/`map`/…）**不产生任何约束**，只是写给人的
注释。同理 `handle=`（38 条）/`ret_handle=`（16 条）只喂 6.1 的 ① 侧分派。
**实参侧没有 handle 标记**（`arg_handle=` 全仓 0 命中）⇒ 一个 i64 实参到底该是 §1
的哪一行，注册表里**没有地方可写**。这既是 §1 全表存在的理由，也是它目前的缺口。

### 6.3 校验现状：只核"存不存在"，从不核签名

`tools/gen_from_registry.py` 的核对只做符号存在性：用 `nm -gU` 从
`zeta_runtime_c.o` + `tokio_runtime.o` 取"已定义"（:263 接受 `T`/`D`/`B`），
`missing = wanted - defined`（:278-279）。`tools/check_registry_symbols.sh:36`、`:45`
是同一判据的 shell 版（只提取 F 的第 4 列、X 的第 2 列）。
`tools/build_runtime.sh:15` 只重新生成四份产物、不检查；`--check` 是手动档
（口径见 validate.md:46）。
⇒ **没有任何判据比较过 `args=`/`ret=` 与 C 定义的实际签名，也没比过 arity。**
后果与 §3.2 相接：错配的实参由 `coerce_call_args` 静默兜住
（`strict_abi` 时首条即致命；默认不启用，见 §6.6）。
**G.5d 动作项**：把存在性判据升级为"符号 + 参数个数"判据（`nm` 拿不到 arity ⇒
需要 C 侧另出一份签名表，或从 registry 反推并逐一核对 `__asm__` 别名）。

### 6.4 打包/拆包责任：调用方负责塞进 i64，被调方负责读成形

这是 §3.3 C5 的具体化。合同：**打包责任全在编译器，拆包责任全在 C；
两边都不报错，只"读成形"。** 逐入口（C 签名 → 它假设收到 §1 的哪一行）：

| 入口 | C 签名锚点 | 它假设收到什么 | MIR 调用点 |
|---|---|---|---|
| `zeta_dynarray_new` | :2578 | 字面量 cap（真整数）—— ⚠️ 它内部 `if (cap < 8) cap = 8`（:2579），**所以字面量建出的 vec 永远 ≥8**，掩盖了下一行的判据缺陷 | 注册在 `pylib/runtime_core.txt:36`（`args=i64 ret=i64`） |
| `zeta_dyn_getitem` | :3427 | `base` 是 **L2 数据指针**或 **L3 map 句柄**；`key` 是索引，**负数按 `+len` 回卷**（:3433）；其 vec 判据写作 `cap >= 8`（:3432）⚠️ **与 L2 的 `cap >= 1`（:3474）不一致，且本批实测这是一个可观测的死循环**（`[1,2]+[3,4,5]` 的 cap=5 走不进 vec 臂 ⇒ 落到 `map_get` 的开放寻址环，`&(cap-1)` 在 cap=5 上不是掩码 ⇒ 永不停止；详见附 B#7） | gen.rs:12287；**手写**声明 codegen.rs:1071（2 参） |
| `zeta_dyn_len` | :3492 | 句柄**或**文本指针，**无标签**；读序 map→vec→文本（R4） | gen.rs:6412 |
| `zeta_dyn_contains` | :3510 | 4 元：容器 + 原键 + **map 归一键**（`map_str_key` 之值）+ `key_is_str` 选内容相等（:3506 原文） | gen.rs:8965 |
| `zeta_dyn_truth` | :3534 | map→已用槽数、vec→头长度、文本→首字节（:3529 原文）；文本分支先过 L7 的地址闸门（:3539） | gen.rs:3396 |

⚠️ 表中后四行在 Rust 侧**没有显式 extern 声明**（`zeta_dyn_getitem` 有，:1064），
其声明由 §4.2 第 18 档兜底生成为 `i64(i64×实参数)`。今天 arity 恰好对，但那是
**巧合级正确**：调用点少传一个参数时 §3.2 #9 会补一个零字，C 侧就把 0 当成
`key_is_str` 或 `hkey` 读下去 —— 这就是"打包/拆包责任"必须写死的原因。

### 6.5 "什么能被 JIT 绑定"有三份名单，互不校验

① `pylib/registry.txt`（AOT 链接期；生成器 :84-:86 的过滤写明"只有 `py_*` 进生成表，
`zeta_*`/`map_*` 仍手写在 `codegen::new`"）；
② `jit_mappings_gen.rs` 的 `JIT_MAPPINGS`（生成器 :231 注释自述它是 JIT 预检诊断的
**权威**）；③ `jit.rs` 的 `JIT_VEC_BINDINGS`（Rust 宿主函数，批次 315 立）。
⇒ 三者没有交集检查：**AOT 能链接的符号 JIT 未必能绑**（批次 315 的根因正是这个）。
现行补救是"预测而非探测"（`defined_in_process_image`）。
**G.5e 要把三张表合成一张。**

### 6.6 跨边界旋钮清单（开关与行为的对应关系）

| 旋钮 | 生效点 | 语义 |
|---|---|---|
| `ZETA_STRICT_ABI` / `--strict-abi` | codegen.rs:1333 读入 → 字段 `strict_abi`，6945-6950 用 | §3.2 的 `abi_note` 从告警变致命（CLI 侧 main.rs:529） |
| `ZETA_LENIENT_STUBS` | py_additions.c:3307（`py_stub_abort` 内） | 桩从 abort 退化成返回 0 |
| `ZETA_STRICT_STUBS` | py_additions.c:3314 | ⚠️ **假旋钮**：`(void)getenv(…)`，注释自述 "env is documentary"——读了但什么都不改变 |
| `ZETA_NO_OPT` | jit.rs `optimize_module` | 跳过 -O3 管线（仅诊断用） |
| `ZETA_JIT_MIN_OK` / `ZETA_JIT_TIMEOUT` | tools/jit_sweep.sh | JIT 门禁的跑通数下限 / 单程序超时 |

⇒ 合同推论：**环境变量要么有行为，要么删掉。** `ZETA_STRICT_STUBS` 属"看起来存在
开关"，下一个人设 `=1` 期望严格化时会被静默骗过（登记附 B#6）。

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
（§5 的 L1-L9 就是这张表的展开版；§3 的对应验收物是 §3.5 的 M1-M4。）

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
4. ~~**返回类型有两个候选来源，优先级未成文**~~
   ✅ **已定位 2026-09-22（批次 318）——答案是"没有优先级"：两源并行、互不知情。**
   原判断（"补偿点按声明办事 ⇒ 声明类型赢"）**是误判**，来源是把 M2 那条用例里
   "首条顶层 `Return` 恰好与声明同形"当成了声明在起作用。实测三点定死：
   ① `Mir` 无 return_type 字段 ⇒ 声明进不了定义侧（`infer_fn_return_type` 独占签名）；
   ② 声明**只**进调用侧：MIR gen 把 resolver 的声明类型写进 call 的 dest 槽
   （`--dump-mir` 实测 `-> i64` ⇒ `4: I64`，同程序被调 MIR 里返回值是 `1: F64`）；
   ③ 补偿点读的是**当前函数自身**的 LLVM 签名（:4488-4497
   `get_insert_block().get_parent()`），不是"被调签名反查"。
   后果比"静默截断"更糟：不一致时是**同槽混型的按位重解读**（M5：`-> i64` +
   `return 2.5` ⇒ 打印 `4612811918334230528`；M6 反向 ⇒ `0.000000`），
   截断只在"嵌套返回被顶层签名补偿"这一条路径上出现（M7）。
   ⇒ 修复项另立 **附 B#8**（本条只负责关闭"未定位"）。
5. **名字编码不可逆（§4.1 的根因）**：N1/N2/N3 三种拼写共用 `_` 与 `__`，
   **没有任何转义规则**，于是 `<module>__<member>`、`<base>_inst_<T>`、`<name>_<arity>`
   三族可以互相撞进同一串（`a__b` 既可能是"模块 a 的 b"，也可能是"模块 a_ 的 b"）。
   读侧唯一的判形手段是 `contains("__")`（codegen.rs:2889）——它只回答"有没有"，
   不回答"哪一段是分隔符"。⇒ 合同推论：**在加转义之前，任何"从符号名反推源级名"
   的代码都只能算启发式**，§4.2 那条 18 档瀑布就是它的体量证明。
   G.5e 动作项：要么给分隔符定转义形（如 `___` 或长度前缀），要么放弃反推、
   改为"声明期一次定型 + 读侧查表"。

   **5′ 跨模块裸名回退的歧义**（§4.2 档 3/4/12/13）：瀑布在限定名匹配失败后会退到
   **裸方法名**，而裸方法名在整个模块里是共享命名空间——命中哪个定义取决于
   谁先被声明。这与任务 #9 记的是同一类缺陷在两侧的投影（#9 记 resolver 侧的
   `py_member_aliases` 全局裸名表；本条记 codegen 侧的读回退）。**同根，不另立任务。**
6. **假旋钮**：`ZETA_STRICT_STUBS`（py_additions.c:3314 附近）只有注释，
   C 侧对该 env 的调用形如 `(void)getenv(...)`，**读了但什么都不改变**
   （对比 `ZETA_LENIENT_STUBS` :3307 确实改变 `py_stub_abort` 行为）。
   ⇒ 它出现在 §6.6 的表里是因为**下一个人会去设它**：设 `=1` 期望桩变严格，
   实际得到的是静默无变化。G.5d 动作项：二选一——接上行为（让 :3303 的 abort
   真的分严格/宽松两档）或删除该注释，别留"看起来存在的开关"。
7. **`zeta_dyn_getitem` 的 vec 判据偏旧 ⇒ 短 vec 动态下标死循环**（:3432 写 `cap >= 8`，
   L2 的 :3474 已改成 `cap >= 1` + 紧块）：这是批次 301"判据按值统一"漏掉的同一处的
   **第二个副本**。本批实测（`/tmp/abi5/p6.z`，非 `tests/`）：
   ```zeta
   def get1(d): return d[1]        # 参数未标注 ⇒ 走 zeta_dyn_getitem
   xs: list = [1, 2] + [3, 4, 5]   # py_array_concat：base[0] = n = 5（:2392）
   ```
   ⇒ 进程**挂死**，`sample` 1,719/1,719 个栈样本全在 `map_get+256`
   （`/tmp/abi5/p6.sample`）。机制与 §1 行 #8 记的 Json/map 撞车**完全同形**：
   vec 臂被拒后落到 `map_get`，它拿 `cap=5` 当 2 的幂容量做 `idx = hash & (cap-1)`，
   掩码失效 ⇒ `while(1)` 走不到空槽。
   对照组：同一段改成字面量 `[1, 2, 3]` 正常（`lit[1] = 2`），因为
   `zeta_dynarray_new` :2579 把 cap 抬到 8 —— **判据的漏洞被生产者的下限掩盖了，
   只在使用 `n ? n : 1` 的生产者上暴露**（`py_array_concat` :2392、`py_sorted_key` :1717、
   `py_builtin_map` :2084、`py_builtin_filter` :2094、`py_zip` :645、
   `zt_map_most_common` :294）。
   ⇒ 这不是"文档不一致"，是**已实测的运行期挂死**。修复属于 G.5d（把 :3432 收敛成
   一次 `zt_dyn_vec_hdr` 调用），已立任务跟踪；修完复跑 §3.5 探针 + §6.4 五入口。
8. **返回类型两源并行、无一致性检查**（附 B#4 定位之后的**修复项**，= 任务 #33）：
   定义侧签名只听 `infer_fn_return_type`，调用侧只听 MIR dest 槽里的声明类型
   （两者来源见 C2）。今天它**不是**"偶尔有损"，而是**能静默产出 IEEE 位模式垃圾**：
   M5 打印 `4612811918334230528`、M6 打印 `0.000000`，两者 rc=0（批次 318 时零诊断，
   319 起出声）
   —— 这是 **§2 R7**（第 2 章的对称表今天只覆盖"赋值入槽"和"容器字出槽"，
   返回路径是盲区）在两个方向上的违例：M6 直接违 R1（整数入浮点槽必须 `sitofp`），
   M5 落在 R1 自述"反向不存在"的那一格。
   两步修法（先响后改）：**① 已落地（批次 319），但落点与本条原计划不同**——原计划
   "定义期比对声明 vs 推断"要先送声明类型进定义侧（`Mir` 没有 return_type 字段）
   ⇒ 得动 MIR gen；实际改在**调用点**比"被调方签名型 vs dest 槽型"，零 MIR 结构改动，
   且告警恰好落在真正会出错的那条调用上（实现与三条覆盖边界见 §2 R7 末）。
   `strict_abi` 暂不因其致命：这是新告警、且真实语料已有 6 例命中，先积累判定数据再谈升级。
   **②** 比对通过后收敛为单一真相（建议签名服从声明、实际返回值走 C3 补偿，
   且补偿不再静默）。⚠️ 改签名来源会改变 `If{dest: None}` 那类程序的行为
   （M7 实测：签名被顶层 `return 7` 定成 i64、分支里的 F64 返回被 `fptosi`）
   ⇒ 必须全量门禁 + 把 M5/M6/M7 固化进 `tests/` 后再动。
9. **编译期诊断在门禁里读不出来**（批次 319 撞出，逐条实测两条 harness 路径）：
   - official：`tools/run_all.sh:67` 是 `… -o out >/dev/null 2>&1` ⇒ 编译期 stderr
     **整个丢掉**，连落盘都没有。
   - python_style：正面用例 `tests/python_style/run.sh:95` 把编译输出重定向到
     per-file 的 `$OUTDIR/$name.cc` ⇒ 事后翻得到，但**不聚合、不打印**，只有编译
     失败时才 `tail -1` 一行；负面用例 :87 同样是 `>/dev/null 2>&1`。
   ⇒ §3.2 的 `ABI coerce`、本批的 `ABI return`、批次 315 的 `[W3001]/[E4016]`
   这类诊断，在门禁汇总里一条都看不见。本批要拿"0/194、6/38"这两个数，只能另写
   逐文件捕获 stderr 的扫描（`/tmp/abi319/`）。
   **顺带一条好消息，它同时是给诊断改动的护栏**：`run.sh:127-133` 比对的是运行期
   **stdout**（`2>/dev/null`），编译期告警不参与判定 ⇒ 加告警天然不动 285 的计数口径。
   G.5d 动作项：门禁保留一份"编译期诊断"聚合输出（含命中文件名与计数），
   否则诊断做得再多也是往看不见的水里扔。
