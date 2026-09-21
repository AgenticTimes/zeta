# Zeta ABI 合同

> **G.5a（refactor.md）**：把"一个槽里到底能放什么、放取必须怎么对称"写成合同。
> 本章只描述**现行实现**并逐条锚定 `file:line` —— 不是愿望清单。
> 验收口径：§1 表穷尽（没有任何运行时探针假设了表外的表示，附 A 逐个反查）、
> §2 每条规则有锚点。
>
> 章节规划（G.5a-e 分次独立提交；**编号已为后续章节预留**）：
> §1 槽位值表示、§2 写/读对称（本批，G.5a）；
> §3 调用约定与 struct 返回 = G.5b；§4 名字修饰、§5 类型布局、§6 跨边界假设
> = G.5c（§1 表 #6/#7/#8 的几何常量届时升格为明文的布局合同）；
> 附录 audit 表 + "先改本文再改实现"的流程规则 = G.5d；符号注册表 = G.5e。
> 本批的验收反查与未决项放在 **附 A / 附 B**，不占用 §3 起的正式编号。
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
3. 正式 §3（调用约定：参数顺序、struct 返回是拷贝还是指针、`zeta_call1` 的强转全表）
   = **G.5b**，其中 struct 返回若被认定不稳定，需当场选定一种写法并写死。
