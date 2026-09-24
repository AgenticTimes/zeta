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
> 附录 audit 表 + "先改本文再改实现"的流程规则 = G.5d（批次 319 起了头：诊断侧；
> **批次 320 补齐可核对性：附 B#9 关闭 + 锚点判据自动化**）；
> 符号注册表 = G.5e。
> 验收反查与未决项放在 **附 A / 附 B**，不占用正式编号；附 A 覆盖 §1（§5 是其展开版），
> §3 的对应验收物是 §3.5 的 M1–M7（批次 318 由 4 条扩到 7 条）；
> §4–§6 的未决项已登记为附 B#5/#5′/#6/#7，#8/#9 来自批次 318/319（**#9 已由批次 320 关闭**），
> 附 B 现有 10 条，第 10 条是批次 320 收门禁 stderr 后才看得见的解析器截断。
>
> ⚠️ **锚点会随 `codegen.rs` 的行数移动**（批次 319 实测：本文件往 codegen 里插
> 74 行，全文 19 处 `codegen.rs:` 锚点一次性错位，需按插入点重映射后逐条复核）。
> 往该文件插代码的批次，**必须同批重生成这些锚点**，否则合同里的 `file:line` 变成假证据。
> 判据已自动化（批次 320 / 任务 #35）：`./tools/check_abi_anchors.py` 解析本文全部
> `file:line`（含同行续写形态：一个带路径的锚点后跟裸 `:行号`，继承本行最近的路径），
> 核对可定位 / 未越界 / 未漂移三层，基线在 `tools/baselines/abi_anchors.tsv`；
> 改完代码跑一次，重映射后 `--bless`。**目前仍是本地一条命令，未进 CI**（任务 #37：
> 95 个锚点里 23 处落在下面那两个并发持有的 C 文件，先加作用域开关再挂硬门禁）⇒
> "同批改锚点"今天靠人，不靠门禁。
> 它**不**回答"这行是否仍是该规则的实现点"（见脚本头部的已知边界）。
>
> ⚠️ `runtime/py_additions.c` 与 `runtime/tokio_runtime_stub.c` 由并发工作流持有：
> 本文**只引用，不修改**。

## 1. 槽位值表示表

前提（codegen.rs:1599-1603）：**静态可定的本地按自身 LLVM 类型开槽**
（`Type::F32`→`float`、`Type::F64`→`double`、其余一律 `i64`）。因此"8 字节槽"
指的是**除 f32/f64 之外的所有本地槽**，以及容器（vec 元素、map 值、struct 字段）
里那些没有静态类型的**裸字** —— 后者的合法内容才是本表的主题，也是
批次 291/297/298/299/300/301 六批连发的同一类 bug 的现场。

| # | 表示 | 槽内编码 | 写侧（产生处） | 读侧要求 | 混淆史（实测） |
|---|---|---|---|---|---|
| 1 | `i64` | 裸二补码 | `MirExpr::IntLit` | 原样 | — |
| 2 | `f64` | **IEEE-754 位模式**，不是裸整数 | ① 静态 `double` 槽：`Assign` 时 int 值走 `slot_sitofp`（codegen.rs:3341-3366）；② struct 字段是 64 位槽：存前 bitcast（codegen.rs:6256） | 从**裸容器字**读回必须是 `bitcast`，不能 `sitofp`（`slot_or_sitofp` codegen.rs:1706-1721，判据 `slot_read_ids` 60 / 1593） | 批次 298：裸存 → 读回 4.94e-322；批次 299：缺 `sitofp` → `500.0 + 50` 算成 `4.6e18`（5782 记录） |
| 3 | `bool` | **i64 的 0/1** | 比较结果立刻 `zext i1→i64`（`eq_ext`/`ne_ext`/…，codegen.rs:3825-3833） | 一律 `!= 0` | `Type::Bool → bool_type`（7388）只在 ABI 边界（参数/返回签名）出现；**i1 从不进槽** |
| 4 | `str`（指针形） | `.rodata` 或 GC 块首地址，`ptrtoint` 成 i64 | `MirExpr::StringLit`：私有 global 字节数组 + `ptrtoint`（codegen.rs:5750-5769） | 可直接解引用；`GC_base(h) != 0` ⇒ 堆串 | 与 #5 不可静态区分 ⇒ 见 R3 |
| 5 | `str`（packed 形） | 短 ASCII **按字节小端打进这 8 个字节本身**（实测样本 `"43601853"` = `0x3335383130363334`） | `vec<str>` 的元素槽（批次 291 实测；写侧仍是隐式决定，见附 B OPEN） | **必须先判形再解引用**；唯一的判形点在 `map_str_key`（py_additions.c:105-129）：`GC_base` 命中→按内容 FNV 哈希；否则 `u < 2^32 \|\| u >= 2^48`→判为 packed、原样当不透明键；再否则 `vm_read_overwrite` 探一页可读性 | 把 packed 当 `char*` 传给 `str_trim` → 约 25% 概率 SEGV（批次 297）；packed 之间比较走 `zt_packed_cstr_eq`（:1609） |
| 6 | `vec` 句柄 | 指向**数据**；`[cap \| len]` 头在 `base-16`；块 ≥ `16+cap*8` | `zeta_collect_vec_n` / `str_split` / df 列构造器（多数按元素数申请，故 cap 常 < 8） | `zt_dyn_vec_hdr`（py_additions.c:3469-3491）：`cap∈[1,2^28]`、`len ≤ cap`、块够大；**短 vec（cap<8）还必须是"紧块"** —— GC 向上取整的余量恰为 8 或 16，`have > need+16` 即判否 | 批次 301：`cap >= 8` 曾是唯一判据 ⇒ 1..7 元素列表被拒，`len()` 退化成对数据字做 `strnlen`（实测 3 元素报 0；1/4/8 报 5/0/8） |
| 7 | `map` 句柄 | **块首**；`w[0]=cap`（2 的幂、≥16、≤2^28）、`w[1]=used ≤ cap`；块 ≥ `16+cap*24`；`cap < 0` ⇒ growth forwarder（真表在 `w[1]`） | `MapNew` / `DictInsert` | `zt_dyn_is_map`（py_additions.c:3457-3467）+ `GC_size` 兜底 | 与 #8 撞车：map 把 PyJson 的 tag 当容量（见 #8） |
| 8 | `PyJson` | 16 字节 `[tag, payload]` **单元指针**；tag = `ZJ_NULL 0 / INT 1 / F64 2 / STR 3 / ARR 4 / OBJ 5 / BOOL 6`（tokio_runtime_stub.c:2550-2556，构造 `zj_make` :2697） | `json.loads` 一族 | 访问器按运行时 tag 分派（sum type；**不是**外层程序的动态分发） | `-> dict` 注解把 Json 当 map ⇒ 首字（tag ≤ 6）被当容量，`idx = hash & (cap-1)` 死循环（tokio_runtime_stub.c:190-196 记录；现行以"map 首字 ≥16、Json 首字 1..8"作值域区分并响亮报告） |
| 9 | 用户 `struct` | GC 句柄，字段各占一个 64 位槽 | `StructNew`（#2②） | 字段读 = 槽偏移 + 按字段静态类型还原（f64 见 #2） | 批次 300：未标注返回类型退化成 unit ⇒ 字段读成"空 variant 的第 0 字段"，**每个属性读都静默拿垃圾** |
| 10 | `PyDynamic`（预留） | boxed tag cell | — | — | B/T3 落地时更新本行。当前 dyn 值就是 #4–#8 那些**无标签字** —— 这正是 #6/#7 必须做几何判形的根本原因 |
| 11 | 用户 `enum` | **两制**：全单元枚举 = 裸判别值（#1）；只要有一个变体带载荷，整个枚举一律 `[tag, p0, …]` GC 块（与 #8/#9 同形） | ① 载荷形 `Token::Ident(5)`、② 单元形 `Shape::Point`（同枚举内）：`src/middle/mir/gen.rs:3730` 起的变体路径 + `MirExpr::Struct`（`enum_is_boxed` :3313 决定用哪一制）；③ `Some(v)`/`Ok(v)`/`Err(v)` 复用 Option/Result 运行时布局（gen.rs:7008，`option_make_some` 同形） | 判据必须按同一制读：块形读 slot 0 比 tag（`boxed_tag_guard` gen.rs:3371 → `Deref{pointee_width:8}`），全单元形 `== 判别值`；载荷绑定读 slot 1+k。**没注册的构造子名不许当判据** —— 恒不匹配（gen.rs 结构模式 `else` 分支），因为无 tag 可比 | 批次 396：写侧三处都漏了标签字 ⇒ `Some(7)` 被判成"None"（`option_is_some` 拿载荷 7 和 1 比），带载荷臂"恒匹配 + 绑 0"使 `Token::Ident(n)` 对任何值都进第一条臂；单元形写在裸名当值的路径后面 ⇒ 被下成 `FuncAddr`，`_Color__Green` 只有声明没有定义 |

## 2. 写/读对称规则

**R1 整数入浮点槽 = 值转换（`sitofp`），不是位保留。**
锚点 codegen.rs:3341-3366（`MirStmt::Assign` 的 `slot_sitofp`）。
反向不存在"浮点入整数槽"：浮点要么进 `double` 槽（值），要么进 64 位裸槽（位模式，R2）。
⚠️ 该断言的适用范围 = **赋值路径**。批次 318 实测：返回路径上确有"浮点写入、整数读出"
的同槽混型（M5 打印 `4612811918334230528`），另立新规则 **R7** 覆盖，不改动本条。

> 锚点源码：src/backend/codegen/codegen.rs
**R2 裸容器字里的 f64 出槽 = `bitcast` 回读。**
锚点 `slot_or_sitofp`（codegen.rs:1706-1721）——同一个 helper 二选一，判据是
`slot_read_ids`（声明 :60，填充 :1597，消费 :1712，语义注释 :1703-1705/:1632）。
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
锚点 codegen.rs:3825-3833。容器真值另有一套按长度的判定（批次 297），
两者不互相转换 —— "非空 dict" 是 `len>0`，不是 `!= 0`。

**R6 struct 字段一律 64 位槽；宽度≠8 的类型进/出槽各 bitcast 一次。**
锚点 codegen.rs:6256（存侧注释）+ #2 读侧。

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
slot is int`（锚点 codegen.rs:3375 调用点、:7039 实现），并在 `report_abi_coercions`
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

**R8（批次 385 补）模块全局在运行期只有一份存储：env 表里按名字存的那个 64 位词；每一条写路径都必须写到它。**
一个模块级名字在编译期有两个落点：使用它的那个函数体里的局部槽（带静态类型），和
C 运行时里按名字查的那一格（`zeta_env_get`/`zeta_env_set`，runtime_decls_core.rs:51-52，
一个格子只装一个 64 位词）。**函数体里没有这个名的槽时，读走 env**（gen.rs:3725 起），
所以只写局部槽的写路径等于把这次赋值丢掉。三条写路径现在是这样：
① `=` —— 写局部槽 + 镜像 env（gen.rs:3725 起，本规则的第一处实现）；
② `global` 声明的名字、以及批次 384 抬到模块作用域的 `static` —— 这些名字**没有**局部槽
（读写一律走 env，gen.rs:1649 与 :1781），所以是"只写 env"，与 ① 不是一条规则；
③ `+=`（`AssignOp`）—— 批次 385 之前只看局部槽，`total += 5` 连调两次打回 `5 / 5`，
现在与 ① 对称（gen.rs:3725 起）。
**上面这些 `=`/`+=` 的段号是批次 385 写的；批次 387 在同一个文件前面插入了两个公共入口，
其后的行号整体前移**，新号：① 的两处镜像 gen.rs:1738（传 `existing`）与 :1793（传 `new_id`）、
② 的 `nonlocal` 写 :1718 与 :1824、③ 的镜像 :1857（传 `slot_id`）；八条写点从批次 387 起
一律走 `env_store`（:428），四条原先硬写 `Type::I64` 的读点一律走 `env_slot_ty`（:485）。
**三条路径镜像的都是"刚写过的那个槽"，不是原来那条表达式**（`=`：gen.rs:3725 传
`existing`、gen.rs:3725 传 `new_id`；`+=`：gen.rs:3725 传 `slot_id`）。理由实测：折叠出的加法表达式里就带着这个槽
自己（同一函数里 `total += 1; total += 2`），而 codegen 在每个使用点重新求值该表达式 ⇒
第二次写被算了两遍 —— 中间态实拍：函数自己返回 20，env 里是 22；`=` 同形（`acc = acc + 3;
acc = acc + 4`）返回 7、env 里 11。
**env 只装一个 64 位词，这条限制本身没有消失**（`zeta_env_get(i64)->i64` /
`zeta_env_set(i64, i64)`，runtime_decls_core.rs:51-52），消失的是"两头各说各话"。
**批次 387 起，写侧与读侧共用一个判据**：`env_store`（gen.rs:428 起）与 `env_slot_ty`
（gen.rs:485 起）都问同一句 `global_ty_of(name)` ⇒ 一格要么两头按数值，要么两头按位。
实测三档：
① 整数精确；
② 容器句柄作为一个词存回取回都对，因为读侧把声明类型带给了槽（`global_ty_of`，gen.rs:3500）
—— 顶层 `data = [10, 20, 30]`，`len(data)` 在外层和别的函数里都是 `3 / 3`（`/tmp/b385/p7.z`）；
③ **声明是浮点的名字存位模式**：`env_store` 对"声明 `F32`/`F64` 且值也是浮点"的名字，先把值
落进一个槽，再 `AddrOf` + `Deref{pointee_width: 8}` 从那个 `double` 的地址读出裸词传给
`zeta_env_set` —— 与读侧同一个重解读，也与容器写侧同形（codegen.rs:4782 `dict_f64_bits`、
:5503 `field_fbits`、:6792 `elem_bits`），不需要新的 C 运行时符号。改前实拍：
`ratio = 2.5` 在别的函数里打回 `0.000000`（写侧 `fptosi` 存成 2，2 的位模式是 1e-323）；
改后 `t426_f64_env_roundtrip.z` 为 `2.500000 / 2.500000`，`global` 写与 `+=` 写两形见
`t427_f64_env_write_paths.z`（`3.500000 / 3.750000 / 3.750000`，与 `python3` 逐字相同）。
**没有声明类型的那一族仍然截断**（闭包里的 `nonlocal`：`gen.rs:1718` 一侧，实拍
`/tmp/b386/pA.z` 的 `read= 2`，两条 `fptosi` 告警仍在）—— 它的读侧按 `I64` 走，写侧存位
模式会打成十几亿量级的整数垃圾，那不是修复。收掉这一格要给 env 带类型，是轴 B/F 的活。
另一半没闭合的：`x: float` 的模块全局被**整数字面量**赋值时，值不是浮点 ⇒ 走原样的整数存储，
读侧再按 `F64` 重解读 —— 这一形本批未改也未测，登记在此。
**本规则还没覆盖的一格（实测、未修）**：模块体自己（合成出来的 `main`）读它声明过的
模块全局用的是那条 `=` 留下的局部槽 —— 读侧先看这个函数已有的槽（gen.rs:3452，在 gen.rs:3725
的 env 读之前），命中就直接返回，不会再从 env 刷新 ⇒ 别的函数写过之后，主程序里
读回的还是声明时的值（`tests/python_style/t423_static_mut_persistent.z` 末行 `top=0`
就是这一格，最小复现 `/tmp/b385/p4.z`）。

## 3. 调用约定与 struct/元组返回（G.5b）

**先划边界**：本文的"调用约定"是三个不同的边界，混起来谈是本章之前的常见错误。
§3.1 zeta→zeta（同一 LLVM module 内的用户函数）、§3.2 实参强转全表、
§3.3 zeta→C 运行期、§3.4 间接调用与 Python 式形参折叠、§3.5 实测核对。
**本章只描述现行实现**；§3.2 的"判定"列是 G.5d 收敛的靶子，不是本批的行为变更。

### 3.1 zeta→zeta：签名字母表只有三个类型

> 锚点源码：src/backend/codegen/codegen.rs
**C1 参数与返回的 LLVM 类型只能是 `i64` / `f32` / `f64`。**
锚点：参数映射 codegen.rs:1467-1474（`Some(Type::F32)`→`f32`、`Some(Type::F64)`→`f64`、
`_`→`i64`）；返回映射 :1480-1484 + `infer_fn_return_type` :1392-1405（同样只在
f32/f64/i64 中选）。
推论（合同级）：**struct、元组、串、容器、枚举一律以 i64 句柄/裸字传递与返回**；
LLVM 层不存在聚合返回 —— `sret`/`byval`/`struct_ret` 在 `src/` 命中 0。
写侧照此：struct 字面量 `runtime_malloc(fields*8)` + 字段各占 8 字节
（:6211 注释原文 "Allocate struct on HEAP to prevent dangling pointers"、:6212-6215、
:6279-6280）；元组走 `StackArray` 的 `[cap|len|elems…]`、句柄 = `buf+16`（:6740-6745、
:6814-6815，与 §1#6 同一几何）。读侧：字段访问 = `int_to_ptr` + **整块 load**
`struct_type(&[i64; N], false)`（:6501-6509，非 packed）+ `extract_value`（:6544-6547）；
字段写 = 地址算术 `base + idx*8` 后 `store`（:5489-5510）。
⚠️ **另一套通用映射器不参与签名**：`type_to_llvm_type`（:7422）会把 `Type::Tuple`
映成真 LLVM struct（:7528-7534）。改签名时只认 :1467-1480，认它就等着 ABI 不一致。

> 锚点源码：src/backend/codegen/codegen.rs
**C2 返回类型有两个各自独立的来源，谁也不覆盖谁**（裁决层已于批次 318 定位，附 B#4 关闭）：
- **定义侧**（LLVM 签名）= `infer_fn_return_type`（:1392-1405）扫 `mir.stmts`，取
  **首条顶层 `Return`** 值在 `type_map` 里的类型，扫不到默认 `i64`（:1412）。
  声明的 `-> T` **完全不参与**——`Mir` 结构里根本没有 return_type 字段
  （`src/middle/mir/mod.rs`、`src/middle/mir/gen.rs` grep `return_type` **0 命中**）。
  ⚠️ **MIR 并非全平铺**：`If { then:[Return…] }` 是**嵌套 stmt，扫不到**。分支函数今天
  多数仍正确，是因为 MIR gen 会给带 `dest` 的 `If` **合成一条顶层尾 `Return{dest}`**，
  其 `type_map` 是各分支类型的合并值（实测 `/tmp/abi8/m3.z --dump-mir`：
  `If{…, dest: Some(7)}` + 顶层 `Return{val:7}` + `7: F64` ⇒ `define double @h`，
  `2.5/3.5` 全对）。**`dest: None` 时没有这条合成**（`m2b.z`：`If{dest:None}` 里的
  `return x`(F64) 被跳过，签名由顶层 `return 7`(I64) 定 ⇒ `define i64 @f`）
  ——这才是 :4617-4618 注释"nested ones are never consulted"的确切适用面。
- **调用侧**（怎么读返回值）= MIR 生成期写进 **call dest 槽** 的 `type_map` 条目，
  它来自 resolver 的声明。实测（`/tmp/abi8/a.z --dump-mir`）：`-> i64` ⇒ main 里
  dest 槽是 `4: I64`，而被调函数自己的 MIR 里返回值是 `1: F64` —— **两份真相同时存在**。
> 锚点源码：外部转储（/tmp 下 `--emit-llvm` 落盘的 .ir，非仓内文件，不入锚点核对）
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

> 锚点源码：src/backend/codegen/codegen.rs
**C3 补偿发生在返回语句处：`ret_sitofp` / `ret_fptosi`（:4595-4629）。**
读的是**当前正在生成的函数自身**的 LLVM 签名
（:4585-4594 `builder.get_insert_block().get_parent()` → `get_return_type()`），
**不是**"从被调方签名反查"（此处为批次 318 的更正；原表述会让人以为调用侧也走同一条链）。
**补偿是有损的且不告警**；`abi_note` 只服务实参侧（§3.2），返回侧今天无任何诊断。
⇒ **G.5d 的修复靶心已明确**：让定义侧也服从声明（MIR 携带 return_type 到定义侧，
或 codegen 反查 resolver 声明来定签名），并在"声明 ≠ 首条顶层 Return 的推断"时
发一条与 §3.2 对称的诊断——今天这两种错法分别是**静默按位重解读**（C2）与
**静默有损转换**（C3）。

### 3.2 实参强转全表（`coerce_call_args`）

> 锚点源码：src/backend/codegen/codegen.rs
锚点 codegen.rs:6889-7014；告警器 `abi_note` :7012-7026（stderr 最多 8 条，
`strict_abi` 时首条即致命）。调用点：:3859、:3875（运算符名兜底）、:4410（常规调用）、
:4567（void 调用兜底）、:4792（`DictInsert`→`map_insert`，传空 `arg_ids`）。

> 锚点源码：src/backend/codegen/codegen.rs
| # | 实参 → 形参 | 发出的指令 | 丢值？ | 告警 | 判定 |
|---|---|---|---|---|---|
| 1 | `i<n>` → `i<m>`，n<m | `arg_zext`（:6923-6930） | 否 | 无 | **保留** |
| 2 | `i<n>` → `i<m>`，n>m | `arg_trunc`（:6940-6943） | **是**（丢高位） | `narrow iA→iB`（:6931-6939） | **删除候选**（改诊断） |
| 3 | 同宽整数 | 原样（:6944-6946） | 否 | 无 | 保留 |
| 4 | int → float，且该实参来自容器裸槽 | `arg_slot_bits` **bitcast**（:6948-6959） | 否 | 无 | **保留**（与 §2 R2 对称，批次 299；判据 `slot_read_ids`） |
| 5 | int → float，其他来源 | `arg_sitofp`（:6960-6963） | 否 | 无 | 保留，但**应告警化**（值对、静默错） |
| 6 | float → int | `arg_fptosi`（:6975-6978） | **是**（小数；负数/OOB 为 LLVM poison） | `fptosi → iN`（:6969-6974） | **删除候选** |
| 7 | float ↔ ptr | **不转换**，原值照推（:6980-6984） | 值不匹配照样进 `build_call` | `float↔ptr (forbidden)` | **必改**：写着 forbidden 却不拒绝 |
| 8 | 其余一切（**含 ptr ↔ i64**） | 原样（:6985 `_ => push`） | 未知 | 无 | **删除候选**（今天的默认＝硬塞） |
| 9 | 实参少于形参 | 补 null/零常量（:6988-7009） | 静默补空 | 无 | **删除候选**（Python 侧应报错） |
| 10 | 实参多于形参 | `result.truncate(n_params)`（:7010-7011） | **静默丢实参** | 无 | **删除候选** |

**判定口径**：保留＝无损且语义正确；删除候选＝G.5d 收敛时改为诊断报错。
按"报告先行原则"（refactor.md §G 前言），**#2/#6/#7/#8/#9/#10 全部纳入 `abi_note`
是下一步，本批零行为变更**；#4/#5 需要的是让静默的 #5 也开口，而不是删掉它。
现行外部批评已登记在册：ARCHITECTURE-REVIEW-2026-09.md:104（"静默错值链"）、ARCHITECTURE-REVIEW-2026-09.md:149
（P0#2 要求收敛强转白名单）、validate.md:151（extern 声明写 i64 而收 f64 →
`fptosi` 截断，即 `abs(-2.5)`→nan 根因，复现 `tests/python_style/t27_builtins_fmt.z:18`）。

**#6 的一档已在源头消失（批次 397）**：浮点接收者的方法调用（`x.sqrt()` 一类）不再下成裸名让
未知 extern 兜底声明成 `i64(i64, …)`，而是按 `pylib` 注册表（`pylib::find_member`）取符号与声明
签名，路由后实参本来就是 `double(double)` ⇒ 不进 `fptosi` 档、也不发 `abi_note`。
**这不等于 #6 收敛**：仓内声明成 i64 形参的被调者（如 `zeta_qc_new`）照旧截小数，official 集内
仍有实例；而 `abs` / `round` 这类**表里没有同名成员**的写法仍走整数路径（`(-4.5).abs()` 打 `4`，
改前改后逐字同值）⇒ 上面那条 `abs(-2.5)` 的旧账**本批未清**。整数接收者与"形参落定 i64 的仓内
函数"两档的正解都在 refactor 类型基础那一格（跨函数可查的参数/返回类型表），不在本表内解决。

### 3.3 zeta→C 运行期

**C4 一律默认 C 调用约定**：`set_cc` / `CallConvention` / `byval` 在 codegen.rs 命中 0，
所以跨边界的一切都必须塞进 §3.1 的三类型字母表。

> 锚点源码：runtime/py_additions.c
**C5 registry 生成的 extern 里句柄/串/容器参数一律声明为 `i64`**，只有真收
`double` 的才写 `f64`（@generated 产物 runtime_decls_registry.rs:18-24 起，
源头 pylib/registry.txt；C 侧对应 `int64_t`，例 py_additions.c:1791-1825 的
`py_argparse_*`、:2650 的 `zeta_env_get`）。
⇒ **打包/拆包责任**：调用方只保证"把值塞进一个 i64"，**类型解释全在被调方**。
每个 `W`/`F` 条目实际假设哪种表示（§1 的哪一行）由 §6（G.5c）逐条登记成表。

> 锚点源码：src/backend/codegen/codegen.rs
**C6 唯一的例外是 10 个手写 runtime 函数**（`option_*` / `host_result_*`）：它们以真
`ptr` 进出，故调用点前对**第 0 个实参**插 `inttoptr`（:4381-4392 名单 + :4396-4404），
调用后对返回插 `ptrtoint`（:4418-4426）。
⚠️ 两个事实要记住：`i == 0` 的限定意味着**第 2 个及以后的 ptr 形参不会被转换**（:4396），
以及 —— 新增 ptr 形参的 runtime 函数请走 C5 的 i64 约定，**不要扩这份名单**。

> 锚点源码：src/backend/codegen/codegen.rs
**C7 LLVM 层只有 3 处变参**：`zeta_collect_literals`（:1086-1088，注释说明"按 8 个固定
形参声明，多余交给 `coerce_call_args` 补/裁" ⇒ 实际受 §3.2#9/#10 支配）、
`printf`（:1114-1115）、`::` 限定名 extern 兜底桩（:2906-2910）。

### 3.4 间接调用与 Python 式形参折叠

**C8 闭包 V1 不捕获环境**：合成具名函数 `__closure_N` 直调，无环境结构体
（gen.rs:10498-10508 注释、:13985-13990 "The closure value is then the function address
(i64)"）。

**C9 `zeta_call<argc>(fptr, a…)` 逐 arity 一个跳板（0..4）**：`zeta_call1` 声明
codegen.rs:1069，定义 py_additions.c:3414；`zeta_call0/2/3/4` 声明
codegen.rs:1074-1077，定义 tokio_runtime_stub.c:3792/3798/3804/3810。五者同一形状：
把 `fptr` 强转成 `int64_t(*)(i64 × argc)`，实参与返回值一律 i64，**NULL → 返回 0**。
分派守卫 gen.rs:10871-10895：`receiver.is_none()` 且 `arg_ids.len() <= 4` 且名字不是
全局函数/闭包变量 ⇒ **arity ≥5 的间接调用仍走裸符号路径**（实测
`let f = add5; f(1,2,3,4,5)` → `Undefined symbols "_f"`），也没有 `zeta_callN`：变参 C
函数不能把动态实参表转发给任意函数指针，所以只能逐 arity 各写一个。
返回值恒为 I64：被跳板调的那一方若声明返回字符串/浮点，调用方拿到位模式（G.5d 收敛项）。
姊妹跳板 `zeta_call_fn_arg(i64,i64)`
（py_additions.c:3186；registry.txt:510 登记签名）对 NULL 是 **abort** —— 同一件事两种
失败方式，属 G.5d 收敛项。JIT（无 -o）绑定不了这族：`pylib/jit_mappings.txt` 里 `call`
0 命中，实测 `error[E4016]: 'zeta_call0' has no binding in JIT mode`（`zeta_call1` 同）。

**C10 默认值 / kwarg 在 MIR 期折叠成定长位置实参**，运行期不参与：
注入标记 `zeta_param_default(index, value)`（parser/top_level.rs:341-362）→
Resolver 收集 `param_defaults`（resolver.rs:619-643，kind 不匹配时 :633-640 告警）→
> 锚点源码：src/middle/mir/gen.rs
gen.rs `fill` 按**声明顺序**落槽：位置实参（:8751-8757）→ 关键字（:8758-8763）→
`**` 映射填未绑定槽（:8764-8776）→ 默认值（:8777-8786）。
C 侧 `zeta_param_default` 是**恒等 no-op**（py_additions.c:2672），存在只为让标记不成未定义符号。
> 锚点源码：src/middle/mir/gen.rs
现状记录（不是愿望）：**重复绑定静默后者覆盖**（:8760 `slots[i] = Some(v)` 无检查）、
**未知关键字变成多余位置实参**（:8761 `slots.push`）→ 再由 §3.2#10 静默裁掉；
只有"缺失绑定"会响（`warn_unbound` gen.rs:993-1008，文案 "read 0 (Python would raise TypeError)"）。
Python 会抛 `TypeError` 的两件事，zeta 现在都不说话 ⇒ §3.2#9/#10 的删除候选就是为它们预备的。

**C11 被调方的 `*args`/`**kwargs` 没有收集语义**：解析器把它们压成一个不透明 i64 形参
（top_level.rs:71-82，注释原文 "real variadics need arg-tuple support"）；
**没有任何 C 函数把多余实参打包成元组/列表**。调用点 `f(*arr)` 仅在数组长度为字面量时
静态展开（gen.rs:8915-8951，"V1: static-size arrays compile-time unrolled"）；
日志调用里的 starred 实参明确**不展开**并告警（gen.rs:6348-6369）。
⇒ 合同推论：**任何依赖 callee 看见"全部实参"的写法目前都不成立**，写它就是写一个洞。

**C12 `PyArgNS` 是句柄类型，不是结构体**：registry.txt:440
（`W PyArgParser parse_args py_argparse_parse args=1 ret_handle=PyArgNS`）、
MIR 打类型 `Type::Named("PyArgNS")`（gen.rs:5971-5983）、消费端按字段访问分派到
`py_argparse_get_{i64,f64,bool,str}`（gen.rs:11980-12013）。
生命周期：GC 堆、进程级、无人释放（py_additions.c:1791-1825，值经 `GC_strdup` :1803）。

### 3.5 实测核对（G.5b 验收）

探针在 `/tmp`（**未入库**：新增 `tests/python_style/` 文件会动 285 这条计数，
固定为回归用例是 G.5d 的动作项）。全部用 `target/release/zetac <f> -o <bin>` AOT 实测；
M1–M4 在 `/tmp/abi3_*`（批次 316），M5–M7 在 `/tmp/abi8/`（批次 318，
同目录另有 `--emit-llvm` 落盘的 `a.ir`/`b.ir`/`m2b.ir`，本表引用的 IR 行号即出自它们）。

> 锚点源码：外部转储（/tmp 下 --emit-llvm 落盘的 .ir，非仓内文件，不入锚点核对）
| # | 探针 | 结果 | 结论 |
|---|---|---|---|
| M1 | `struct Pair{a,b}` + `fn make_pair(x,y) -> Pair` + `fn take(p: Pair)` + 交错构造两个 Pair 后回读 | `42 / 99 / 4299 / 7 / 8 / 42` 全对 | struct 返回 = i64 句柄、**无别名污染**，C1 成立 |
| M2 | `-> f64` 首返 `7` 后返 `2.5`；`-> i64` 首返 `2.5` 后返 `7` | `7.000000 / 2.500000`；`2 / 7` | ⚠️ **本行原结论"声明类型赢"已于批次 318 更正为误判**（真机制见 M7 与 C2）：签名取自**顶层** `Return` 的推断，该用例里恰与声明同形。"float→int 返回静默有损、零告警"这一条**当时成立**（告警部分已于批次 319 补齐） |
| M3 | 返回只在 `if/else` 分支里、无末尾 `return` 的 `-> f64` | `2.500000 / 3.500000` | ⚠️ **机制已于批次 318 更正**：不是"C2 的'首条'含分支返回"——嵌套 `Return` 扫不到；是 MIR gen 为带 `dest` 的 `If` **合成**了顶层尾 `Return{dest}`，其 `type_map` 是分支类型的合并值（`7: F64`）。`dest: None` 时没有这条合成 ⇒ 见 M7 |
| M5 | `-> i64`，函数体**只有一条** `return 2.5`（`/tmp/abi8/a.z`） | 打印 **`4612811918334230528`**，rc=0，零诊断（319 起出声） | **两源冲突的真实形状 = 按位重解读，不是截断**：定义侧 `define double @f()`（`a.ir`:1068、`ret double 2.5`:1071），调用侧按声明读同一槽（`store double %12`:1091 → `load i64`:1092 → `println_i64`:1099）。那个整数就是 2.5 的 IEEE-754 位模式。⇒ 违 **§2 R7**（并暴露 R1"反向不存在"的断言对返回路径不成立） |
| M6 | `-> f64`，函数体只有一条 `return 7`（`/tmp/abi8/b.z`） | 打印 `0.000000` | M5 的对称方向：`define i64 @g()`（`b.ir`:1070）而调用点 `load double`（:1092-1094）⇒ 整数 7 的位模式是非规格化 double。**这是 R1 的直接违例**（整数入浮点槽必须 `sitofp`，此处按位保留）；M5/M6 合起来 ⇒ 声明与实现不一致时**两个方向都静默** |
| M7 | `-> i64`，`if x > 2.0: return x`（**无 else**）后接顶层 `return 7`（`/tmp/abi8/m2b.z`） | `r = 2`；IR 里是 `define i64 @f()` + `%ret_fptosi = fptosi double %6 to i64`（`m2b.ir`:1066、:1082-1083） | **这才是 M2 当年看到的"截断"**：`If{dest:None}` 里的 F64 返回对扫描不可见，签名由顶层 `return 7` 定成 i64，嵌套返回被 C3 补偿掉。与 M5 对照即可证明**"声明"从未参与定签名** |
| M4 | capybara COMPILER_BUGS #4 的复现：`fn get_pair() -> (i64, i64): return (42, 99)` + `let (a, b) = get_pair()` | `42 / 99`，rc=0 | **该 bug 今天不再复现**；其"返回局部指针"根因假设（COMPILER_BUGS.md:39、NOTES.md:25-28）被 M1/M4 证伪 —— 写侧本就 heap 分配（codegen.rs:6211、RELEASE_NOTES_v1.0.19.zeta:19）。"garbage fields"的**现行**来源是字段数解析失败时的 `("", 2)` 二字段兜底（codegen.rs:6399-6404 注释即记此事、兜底点在 codegen.rs:6477-6480） |

⇒ **COMPILER_BUGS #4 应按"已不复现 + 残留风险另在"处理**，而不是继续按
"struct/元组返回是坏的"理解这套 ABI。

## 4. 名字修饰（G.5c 第一批）

**先分轴**——三条互相独立的命名轴，混谈是这类 bug 的共同形状：
**轴一** 源级名 → LLVM 符号名（编译器内）；**轴二** LLVM 符号名 → 链接符号
（LLVM 自己的 `.N` 改名 + Darwin 前导 `_`）；**轴三** 注册名 → C 实现
（registry 与 `__asm__` 魔法名）。

### 4.1 轴一的写侧：四种拼写，没有一种可逆

**N1 模块限定的规范形是 `<module 的点换成下划线>__<member>`。**
锚点 resolver.rs:2002、:2803；mir/gen.rs:392、:635、:747、:754、:3694。
构造就是两次 `replace`，**没有转义**：`__` 既是分隔符又可能出现在 member 里，
点号也会把 `a.b` 和 `a__b` 映到同一串。判"这是不是模块限定名"目前只有
`actual_name.contains("__")`（codegen.rs:2935，与同一行的
`actual_name.starts_with("zeta_")` 共用一个分支——"运行期分派名"和"模块限定名"在一条判据里混为一谈）。
⇒ 合同推论：**member 名含 `__`
即破坏可逆性**（无防护，登记附 B#5）。

**N2 泛型单态化形是 `<base>_inst_<T1_T2…>`**：src/middle/specialization.rs:27
（`format!("{}_inst_{}", func_name, type_args.join("_"))`）+ `mangle_function_name`
src/backend/codegen/codegen.rs:1371-1389（`_inst` 后逐个拼 `_` 加类型短名）。
与 N1 共用 `_` 作类型名内部字符和分隔符，同样不可逆。

**N3 重载消歧形是 `<name>_<实参数>`，由 MIR 生成侧加**：gen.rs:11160、:12693
（`format!("{}_{}", func, arg_ids.len())`）。同处注释分三段：
加后缀的理由（src/middle/mir/gen.rs:11134-11136）、三类**不加**后缀的名字
（`zeta_*`、含 `__`、含 `::`，src/middle/mir/gen.rs:11137-11147）、以及"读侧剥后缀会误伤
名字自带的下划线"的现场记录（src/middle/mir/gen.rs:11148-11154：`DataFrame::reset_index`
被剥成 `DataFrame::reset`，返回类型查不到 ⇒ 结果 typed I64 ⇒ `b.columns` 成了一次假字段读）。
**读侧要靠剥后缀还原** ⇒ N3 的代价全在 §4.2 的瀑布里。

**N4 `str_*` 方法在符号层重写成 `host_str_*`**：codegen.rs:2649-2655
（前缀判定 + `resolve_string_method`）。这是一条**隐式命名改写**：源里写 `str_trim`，
链接期要找 `host_str_trim`。

### 4.2 轴一的读侧：`get_or_declare_function` 瀑布（313 行 / 实测 18 档）

**N5 一个调用点的符号名是一条有序尝试序列的结果，不是函数的输出。**
锚点 codegen.rs:2636-2948（函数体 313 行）。ARCHITECTURE-REVIEW-2026-09.md:102
记为"8 级"，本批逐档数出 **18 档**：

> 锚点源码：src/backend/codegen/codegen.rs
| 档 | 前提 | 尝试的名字 | 锚点 | 判性 |
|---|---|---|---|---|
| 1 | 名含 `::` | 精确限定名原样 | :2659-2667 | **承重墙**：注释原文要求"先试精确形，**不要**退回 mangle/裸方法名"——退回过，`a.column("code")[1]` 把一个 Vec 当 map 索引，SEGV（:2664-2668） |
| 2 | 名含 `::` | `::`→`__` | :2664-2670 | 承重墙（N6 的双态） |
| 3 | 名含 `::` | 裸方法名 + 实参数校验 | :2675-2683 | 事故现场：跨模块同名方法可被匹配（附 B#5′） |
| 4 | 名含 `::` | 裸方法名 + `_N` | :2684-2689 | 同 3 |
| 5 | — | `host_*` | :2691-2699 | N4 |
| 6 | — | 裸名 + 实参数校验 | :2700-2712 | 承重墙 |
| 7 | — | `name_N` | :2713-2721 | N3 的反向 |
| 8 | `type_args` 空 | 泛型默认实例化（参数全按 i64） | :2722-2729 | 承重墙 |
| 9 | `type_args` 非空 | `_inst_` mangle | :2731-2734 | N2 |
| 10 | 同上 | 剥 `_<数字>` 后 monomorphize | :2735-2748 | N3 反向 |
| 11 | 同上 | 剥尾缀再 `_inst_` | :2749-2761 | 注释点名 `oneshot::channel_0` |
| 12 | 同上 | 只用方法名 mangle（含再剥尾缀） | :2762-2781 | 事故现场 |
| 13 | 同上 | 限定路径 mangle + `::`→`__`（含再剥尾缀） | :2782-2806 | 事故现场 |
| 14 | 同上 | `host_*` / 裸名 | :2807-2816 | — |
| 15 | 汇流 | `name.<N>`（LLVM 改名） | :2818-2823 | 轴二，见 N8 |
| 16 | 汇流 | `name_N` 再试 | :2824-2831 | — |
| 17 | 汇流 | 剥尾缀 `_N` 试基名（**必须全数字**） | :2829-2855 | 承重墙：护栏注释原文——没有它 `to_string_f64` 曾被当成 `to_string`+后缀而静默改名（:2839-2841） |
| 18 | 全部落空 | **就地声明 extern** | :2857-2947 | 见 N6 |

**档位漂移实测（批次 335）**：这 18 行的旧锚点全部来自批次 317，实测整体偏移
**+59…+62 行**（非恒定 ⇒ 加一个常数修不了，只能逐档重数），且档位 1-5 的旧锚点落在
`get_or_declare_function` **函数体之外**（函数定义从 codegen.rs:2636 才开始）。

**N6 瀑布的最后一档不是报错，是"猜一个签名出来"。**
锚点 codegen.rs:2948-2951 —— 兜底 extern 的签名一律是 `i64(i64, i64, …)`（参数个数 = 本调用点
实参数，变参位 `false`）。⇒ 合同级推论：**名字没对上时编译器不会说话**，它会发出一个
签名可能与定义不符的调用，错值再被 §3.2 的强转表静默"修好"。这就是
docs/ARCHITECTURE-REVIEW-2026-09.md:104 那条"静默错值链"的上游。
推论（收敛方向）：新增解析档位＝扩大错配面积；**响亮化（G.5d）优先于新增档位**。

> 锚点源码：src/backend/codegen/codegen.rs
**N7 `::` 与 `__` 双态是并存的事实，不是待修的笔误。**
存储侧写 `__`、调用点引用带 `::`（:2646-2647 注释原文）。`name.replace("::", "__")` 实测有
**9 处**（旧文档记"4 处"是错的，且它列的第 4 个锚点其实是一行注释）：
读侧查表 :2278、:2501、:2536、:2552、:2664，mangle 之后再拼 :2789、:2799，
extern 声明 :2874、:2877——**9 处全部落在 codegen.rs 这一份文件里**（全仓 grep 无第 10 处）。
但 capybara 的 bug 记录说的是**相反**的存储形：
"gen_mirs stores functions with `::` separator"（COMPILER_BUGS.md:31），且它观察到的
现象确实依赖顺序（COMPILER_BUGS.md:29、COMPILER_BUGS.md:33）。⇒ 本批判定：**两态都由 N5 的瀑布吸收**，
即"谁先被声明谁定形"；这条不是文档能修的，登记为 **G.5e 的靶心**
（收敛成"符号名一次定型，读侧只查表"）。

### 4.3 轴二：LLVM 的 `.N` 改名与 `.set` 别名表

**N8 LLVM 在同名冲突时把定义改名成 `name.N`，运行期靠 C 侧 `.set` 别名追认。**
读侧档位 codegen.rs:2818-2823；别名表 runtime/aliases.inc.c —— **恰好 66 条 `.set`**
（本批实测计数；行 4-69），形如
`".globl _print.13\n\t.set _print.13, _print2\n"`（aliases.inc.c:12），
由 `emit_aliases_inc`（tools/gen_from_registry.py:288-292）从 pylib/runtime_aliases.txt（69 行）生成，
经 tokio_runtime_stub.c:356-358 `#include "aliases.inc.c"` 进入编译单元。

> 锚点源码：src/main.rs
**N9 `.N` 里的 N 不是 ABI，是"LLVM 在本 module 内第几次改名"的偶然计数。**
⇒ 别名表与**IR 发射顺序**是一对锁死件：src/main.rs:868-871 的注释原文——
HashMap 迭代顺序随机 ⇒ `print.N` 冲突改名和运行期别名表"从一次运行到下一次
在能用与不能用之间翻转"，:872 的 `all_mirs.sort_by(...)` 就是这把锁的钥匙。
**合同级：确定性发射序是 ABI 的一部分，不是代码风格。**
（runtime/tokio_runtime_stub.c:339 与 :343-344 的注释是这条的现场记录：`array_new_1` → `array_new.10`、
`print` → `print.N` 且 N 随 arity 1-6 变动。）

> 锚点源码：src/backend/codegen/codegen.rs
**N10 两类名字豁免于 `.N` 改名路径**：`zeta_*` 运行期分派名与含 `__` 的模块限定名
（:2919-2937 命中即直接复用已有声明；豁免判据在 :2935）。注释列了豁免前真实产生的 4 个不可满足符号
（`__get_price_3`/`__get_price_7`/`__get_trade_days_2`/`__OrderCost_6`，:2932-2934）——
默认参数让调用点实参数与声明实参数长期不一致，arity 后缀因此是**错的**。

### 4.4 轴三：非法标识符的 `__asm__` 魔法名

**N11 `[dynamic]<T>__<method>` 是类型打印器造出来的"符号名"，C 侧只能起别名接住。**
`Type::DynamicArray(inner)` 的 `display_name()` 就是 `[dynamic]{inner}`
（types/mod.rs:768）；分派侧用 `::` 形（pylib.rs:899
`dispatched_member("[dynamic]str::isin")`），落到 MIR 时是 `__` 形
（gen.rs:9455 `func: "[dynamic]str__map"`）；C 侧的实现叫 `zt_dyn_str_map`，
靠 `__asm__("_\\[dynamic\\]str__map")` 顶这个名字（py_additions.c:967）。
⇒ **每个动态方法都要人肉注册一个魔法名**（refactor.md 的"承重墙清单"第 130 行已把它列为承重墙；
该文件是仓库根的活文档、未入库，因此不进锚点核对），
且已经有只造了名、没注册实现的变体（gen.rs:9351、:9370、:9389、:9408、:9468、:13367 注释里的
`[dynamic]str__max`、`[dynamic]i64__all`、`str__isna`、`str__pct_change`、`str__normalize`
——链接期报 undefined 才算发现）。
**G.5e 目标**：把"方法名 → 符号"从字符串拼接换成注册表查表，未注册即编译期报错。

### 4.5 "新增一个运行期函数要改几处"：现状仍是手工同步，且有一份生成物从未接线

docs/ARCHITECTURE-REVIEW-2026-09.md:103 记的是"四份符号表手工同步"（①codegen 声明 ②gen.rs 分发
③C 实现 ④`.set` 别名，另加 JIT 的 `add_global_mapping`）。本批核对现状：

| 处 | 现状 | 锚点 |
|---|---|---|
| ① LLVM 声明 | **仍是手写**：codegen.rs 内 255 处 `add_function`（实测计数） | 例 codegen.rs:1069、:1079、:1088 |
| ①′ 生成物 | `runtime_decls_registry.rs`（294 处 `add_function`）+ `runtime_decls_core.rs`（61 处）由 `--emit`/`--emit-core` 生成，**两个入口函数从未被调用** | codegen/mod.rs:7、:9 只声明模块；`declare_registry_runtime_fns`/`declare_core_runtime_fns` callers **图内无边**（codegraph）+ grep 全仓仅定义处与一处注释（pylib.rs:778） |
| ② gen.rs 分发 | 手工 | 例 gen.rs:9455、:9707 |
| ③ C 实现 | 手工 | 例 py_additions.c:967、:3414 |
| ④ `.set` 别名 | 已生成（数据 pylib/runtime_aliases.txt:1 的注释自述"Generated/**edited by hand**"，即"生成物同时被人手改"） | aliases.inc.c:1-2 |
| ⑤ JIT 绑定表 | 已生成**且已接线**（批次 315 用的就是它） | jit_mappings_gen.rs + jit.rs |

⇒ 结论（推翻 review 的一半）：**"单一生成器"只完成了 ④⑤ 两条，①′ 是并行的第二份
而未接管 ①**。这不是新发现的暗雷——机器判据早就登记了它：
`tools/baselines/dc_default.txt:2-3` 两条 `function ... is never used`
（`#![allow(dead_code)]` 屏蔽了 rustc 的报错，只有 `tools/dc_audit.sh` 绕开后才看得见）。
**G.5e 决策项**：要么接线（①′ 取代 ①），要么删除生成物并承认声明是手写的——
现状是最坏的一种：两份表并存、只有一份生效、且看起来已经收敛了。

**幽灵符号案例更新**：docs/ARCHITECTURE-REVIEW-2026-09.md:115 举的 `py_asdict_unexpanded`
原锚在 pylib/registry.txt 第 208 行——该号现已被另一条目（`filterwarnings`）占用，
即条目向下漂了 11 行；它现在在 registry.txt:219（带 `stub=1`），所以**已不是幽灵**：
C 侧有定义且**响亮失败**
（`py_stub_abort`，tokio_runtime_stub.c:1278-1282）。
但"**registry 是纯字符串、无校验**"这条仍然成立，逐条见 §6（本批下一段）。


> 锚点源码：runtime/py_additions.c
| 探针 / 判形点 | 它假设的表示 | 表中行 |
|---|---|---|
| `map_str_key`（py_additions.c:105） | str ∈ {指针, packed}，且 packed 不可解引用 | #4 #5 |
| `zt_dyn_is_map`（:3457） | map = 块首 + 2 的幂容量 + `16+cap*24` 块 + growth forwarder | #7 |
| `zt_dyn_vec_hdr`（:3469） | vec = 数据指针 + `base-16` 头 + 紧块判据 | #6 |
| `zt_packed_cstr_eq`（:1609） | packed 的小端字节序 | #5 |
| `zeta_dyn_len`（:3492） | map → vec → 文本 的读序（无标签字的最后手段） | #6 #7 #4 |
| `zj_make` / `ZJ_*`（tokio_runtime_stub.c:2697、:2550） | 16 字节 `[tag,payload]` 单元 | #8 |
| `ZT_PROBE_LOC` 系列以 `%lld` 打句柄 | 句柄就是可原样搬运的 i64 字 | #4 #6 #7 |

结论：**没有探针假设了表外的表示**；表中每一行都有至少一个探针或写侧锚点。

## 5. 类型布局（G.5c 第二批）

这一章把 §1 表 #6/#7/#8 里"判形探针所依赖的几何"升格为**明文布局合同**。
用途有二：① 改任何容器布局前先照这张表（R4 的"改布局就是改本章"在此落地成常量表）；
② 轴 B（运行时探针）退役时按图索骥——探针删掉之后，这些不变量只剩文档在守。

**L1 唯一可信的边界是 GC 块本身。**
两个原语：`GC_base(p) == p` 读作"p 是某个块的起点"，`GC_size(p)` 读作"该块的可用字节数"
（py_additions.c:3465、:3482）。**GC 会把申请量向上取整**，实测余量恰为 8 或 16
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
（codegen.rs:6501-6508），写侧是地址算术 `base + idx*8`（5446-5463）。
元组复用 L2 的 `[cap|len|elems…]`（:6630-6690）。

**L6 packed 短串：ASCII ≤7 字节按字节小端打进这 8 个字节本身，余下为 NUL。**
`if (n > 7) return 0;` + 逐字节比 `&packed`（py_additions.c:1611-1612）；
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
（py_additions.c:1766，读回成三个 `char*`：:1795；解析器本体又是一个 cap=8 的 L2 vec：
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
| `zeta_dyn_getitem` | :3427 | `base` 是 **L2 数据指针**或 **L3 map 句柄**；`key` 是索引，**负数按 `+len` 回卷**（:3433）；其 vec 判据写作 `cap >= 8`（:3432）⚠️ **与 L2 的 `cap >= 1`（:3474）不一致，且本批实测这是一个可观测的死循环**（`[1,2]+[3,4,5]` 的 cap=5 走不进 vec 臂 ⇒ 落到 `map_get` 的开放寻址环，`&(cap-1)` 在 cap=5 上不是掩码 ⇒ 永不停止；详见附 B#7） | gen.rs:13339；**手写**声明 codegen.rs:1079（2 参） |
| `zeta_dyn_len` | :3492 | 句柄**或**文本指针，**无标签**；读序 map→vec→文本（R4） | gen.rs:7043 |
| `zeta_dyn_contains` | :3510 | 4 元：容器 + 原键 + **map 归一键**（`map_str_key` 之值）+ `key_is_str` 选内容相等（:3506 原文） | gen.rs:9707 |
| `zeta_dyn_truth` | :3534 | map→已用槽数、vec→头长度、文本→首字节（:3529 原文）；文本分支先过 L7 的地址闸门（:3539） | gen.rs:4005 |

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
| **Rust 侧全部布尔旋钮**（批次 336） | diagnostics.rs:515 `env_flag`，27 个读点收敛于此后 | 值为 `0` / `false` / `no` / `off` / 空 ⇒ **关**；其余非空值 ⇒ 开；未设置 ⇒ 关。此前 27 点全是 `env::var(… ).is_ok()`＝**存在即开**，写 `=0` 得到的是"开" |
| `ZETA_STRICT_ABI` / `--strict-abi` | codegen.rs:1341 读入 → 字段 `strict_abi`，6976-6981 用 | §3.2 的 `abi_note` 从告警变致命（CLI 侧 main.rs:568） |
| `ZETA_LENIENT_STUBS` | py_additions.c:3313（`py_stub_abort` 内） | 桩从 abort 退化成返回 0 |
| `ZETA_STRICT_STUBS` | py_additions.c:3320 | ⚠️ **假旋钮**：`(void)getenv(…)`，注释自述 "env is documentary"——读了但什么都不改变 |
| `ZETA_NO_OPT` | jit.rs `optimize_module` | 跳过 -O3 管线（仅诊断用） |
| `ZETA_JIT_MIN_OK` / `ZETA_JIT_TIMEOUT` | tools/jit_sweep.sh | JIT 门禁的跑通数下限 / 单程序超时 |

⇒ 合同推论：**环境变量要么有行为，要么删掉。** `ZETA_STRICT_STUBS` 属"看起来存在
开关"，下一个人设 `=1` 期望严格化时会被静默骗过（登记附 B#6）。

**收敛半径的边界（批次 336 实测）**：`env_flag` 只管得到 Rust 侧。C 运行时仍有 4 个
`getenv` 站点——py_additions.c:2943（`ZETA_PROBE`）、py_additions.c:3313 与
unavailable_stubs.c:83（`ZETA_LENIENT_STUBS`）、py_additions.c:3320（假旋钮）——它们仍是
"非空即开"。⇒ 同一个 `ZETA_LENIENT_STUBS=0` 在 Rust 侧读作关、在 C 侧读作开；两侧语义
不一致这件事本身比单个错值更坏，修法是在 runtime 侧加一个与 `env_flag` 同表意的
`zt_env_flag()`（或公共头），而不是在文档里写"注意 C 侧不一样"。已登记为 OPEN。

### 6.7 进程入口的返回值就是退出码：两种方言在这里相反

`main` 的返回槽交给 clang 的 crt ⇒ **它是什么值，进程就 exit 几**。这条边界不在
zeta↔C 的任何一张签名字母表里（§3.1 只管参数），所以此前只有"尾表达式即返回值"这一条
Rust 式建模在起作用，Python 侧的对应规则（模块尾值只被 REPL 回显、**丢弃**）从未接线：
`l = [3,5,7]; sum(l)` 输出正确却 rc=15（任务 #55）。

现行合同（批次 333 立，行号见 roadmap 批次 333）：

| 事实 | 载体 |
|---|---|
| "这个 `main` 带模块体"由 parser 判定，写成 FuncDef 的属性标记 `PY_ENTRY_ATTR`（`py_entry`） | `top_level.rs` 的 `synthesize_implicit_main`：合成 main 无条件打标；用户的 main 只有在**真的合并进模块语句**或**源里出现过 `if __name__ == "__main__"` guard** 时才打标 |
| 标记在 MIR 侧的唯一消费者 | `MirGen::py_entry`（`lower_to_mir` 逐 item 复位，`MirGen` 是复用对象） |
| 交回值统一改写 | `force_entry_returns`：入口体内每个 `MirStmt::Return`（含 `If`/`For`/`While` 的嵌套语句向量）的槽改指同一个常量 0，**副作用保留** |

三个容易踩的点，写在这里是因为它们各自埋过一次：
① 判据**不能**挂在"缩进预处理器是否触发"上（该谓词是 `!changed`，无缩进的 python 文件算作
花括号方言——任务 #51 对 `//` 记过同一条，批次 334 已改掉，见 §6.8），否则本族所有 flat 复现件都漏掉；
② 到达 `Return` 的路由有 **4 条**（尾值合成、`lower_expr` 的 Return 档、parser 把尾表达式
提升进 `FuncDef::ret_expr`、`ExprStmt` 包着的 Return），逐点打特例判据必漏；
③ 新属性必须走 `process_attributes` 的元数据档，否则会掉进 W5001 未知属性告警——那个计数
是门禁基线的一部分。

⇒ 合同推论：**Rust 式 `fn main() -> i64 { 42 }` 仍返回 42**（它既没有模块体也没有 guard），
所以这条规则不是"入口一律返回 0"，而是"带模块体的入口不交回尾值"。要主动设退出码的
**正路是 `sys.exit(n)`**：`pylib/registry.txt:294` 已把 `sys exit` 接到 `py_sys_exit`
（运行时 `exit(code & 0xff)`），实测 `sys.exit(3)` 与裸 `exit(3)` 都给出 rc=3、与 CPython 一致
——本批一度以为"这条通道不存在"，是因为只查了 MIR 下型点没查注册表（登记在 roadmap 批次 333）。

### 6.8 "这是 python 源码"不能从"有没有缩进"推出来

`indent_preprocess` 的 `changed` 只说明一件事：**本文件需要缩进→大括号改写**。两种相反的
方言都能让它为假——花括号源码本来就不需要改写，**只有顶层语句的 flat python 脚本也不需要**。
把它当方言谓词用的代价各出现过一次：① 入口尾值（§6.7 陷阱 ①，第一版修复因此完全惰性）；
② `//`：平铺文件里整除被当行注释吃掉，`print(x // y)` 静默丢掉后半截（W1002 截断），
而 `print(7 // 2)` 与 `return 1  // Success` 走的是同一条分支。

现行合同（批次 334 立）：`changed==true` 的文件里 `//` 一律按运算符改写（缩进本身就是方言
证据，这条不变）；`changed==false` 时**逐处**判——`indent.rs:590` 带一个跨行携带的 `(`/`[`
净深度走完文件，`indent.rs:653` 只在深度 > 0 处改写，其余原样留作注释。`{` 故意不计入深度，
否则花括号方言的 `if x {  // note` 会被吃掉注释。

判据成立靠的是语料实测而不是直觉：official 语料 194 个文件里"行内已有代码、后面跟 `//`"
共 **209 处，全部在深度 0** ⇒ 一处都不会被误改写。曾考虑的替代判据被同一份实测否决：
"看 `//` 右侧像不像操作数"命中 36/209（`// Success`、`// SIMD`、`// Percentage` 都是单个词）。

⇒ 剩余洞（已记账，不在合同内）：深度 0 的平铺整除 `t = 7 // 2` 仍按注释处理，且失败形态是
**静默错值**（`t` 拿到 7）而非截断 ⇒ `tests/python_style/t406_flat_floordiv_bare_assignment.z`
（known-fail）。闭它需要的是**文件级**方言证据，不是更强的行内启发式——`;` 也不行：
official 语料 194 个文件里 **115 个一个分号都没有**，其中 54 个照样用 `//` 写行内注释
（批次 328 当年记的是 44，口径是"代码后跟 `//` 且不在字符串里"；这里的 115/54 用
`';' not in src` 与"非行首的 `\\S.*//`"两条粗判据复测）。

## 附 A. 探针反查（§1 是否穷尽）

| 探针 / 判形点 | 它假设的表示 | 表中行 |
|---|---|---|
| `map_str_key`（py_additions.c:105） | str ∈ {指针, packed}，且 packed 不可解引用 | #4 #5 |
| `zt_dyn_is_map`（:3457） | map = 块首 + 2 的幂容量 + `16+cap*24` 块 + growth forwarder | #7 |
| `zt_dyn_vec_hdr`（:3469） | vec = 数据指针 + `base-16` 头 + 紧块判据 | #6 |
| `zt_packed_cstr_eq`（:1603） | packed 的小端字节序 | #5 |
| `zeta_dyn_len`（:3492） | map → vec → 文本 的读序（无标签字的最后手段） | #6 #7 #4 |
| `zj_make` / `ZJ_*`（tokio_runtime_stub.c:2697、:2550） | 16 字节 `[tag,payload]` 单元 | #8 |
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
   读侧唯一的判形手段是 `contains("__")`（codegen.rs:2935）——它只回答"有没有"，
   不回答"哪一段是分隔符"。⇒ 合同推论：**在加转义之前，任何"从符号名反推源级名"
   的代码都只能算启发式**，§4.2 那条 18 档瀑布就是它的体量证明。
   G.5e 动作项：要么给分隔符定转义形（如 `___` 或长度前缀），要么放弃反推、
   改为"声明期一次定型 + 读侧查表"。

   **5′ 跨模块裸名回退的歧义**（§4.2 档 3/4/12/13）：瀑布在限定名匹配失败后会退到
   **裸方法名**，而裸方法名在整个模块里是共享命名空间——命中哪个定义取决于
   谁先被声明。这与任务 #9 记的是同一类缺陷在两侧的投影（#9 记 resolver 侧的
   `py_member_aliases` 全局裸名表；本条记 codegen 侧的读回退）。**同根，不另立任务。**
6. **假旋钮**：`ZETA_STRICT_STUBS`（py_additions.c:3320 附近）只有注释，
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
9. ✅ **编译期诊断在门禁里读不出来**（批次 319 撞出 → **批次 320 关闭**）：
   - official 侧原状：`tools/run_all.sh` 编译时 `… -o out >/dev/null 2>&1` ⇒ 编译期
     stderr **整个丢掉**，连落盘都没有（批次 319 记的那条 run_all.sh 锚点当时在 67 行，已随本批改动漂到 76 行——正是这个工具存在的理由）。
   - python_style 侧原状（**本批更正批次 319 的一处判断**）：正面用例
     `tests/python_style/run.sh:96` 把编译输出重定向到 per-file 的 `$OUTDIR/$name.cc`，
     319 据此写的是"事后翻得到"——**错**。`run.sh:16` 有 `trap 'rm -rf "$OUTDIR"' EXIT`，
     `OUTDIR` 是 `mktemp -d` ⇒ 跑完即整目录删除，外部永远捞不回来。聚合**必须**发生在
     run.sh 内部、trap 之前，这也是本批改动落点选择的直接原因。
   ⇒ 现况：两套 harness 各出一行 `compile-diagnostics:`（official 逐文件留档到
   `$OFFICIAL_DIAG`，默认 `/tmp/zeta_official_diag.txt`；python_style 在删目录前聚合
   并打印去重后的 top 10），数字同时进 `/tmp/zeta_baseline.json` 的
   `compile_diagnostics` 字段 ⇒ CI artifact 里也拿得到。
   **口径**：只统计 `warning:` / `PY-A:`，排除 `clang: warning:`（链接器抱怨 `-no-pie`，
   实测 194/194 全有 —— 不排除的话真信号会被同名噪声埋掉）。**不参与退出码**：
   护栏仍然是 `run.sh:136-142` 比对运行期 stdout ⇒ 诊断再多也不动 285/194 的计数口径。
   - **第一份读数（批次 320，此前不可见）**：official **13/194 文件、18 行**
     （= 12×`[W1002]` + 2×`ABI coerce` + 1×其汇总 + 2×`PY-A: imported` + 1×`PY-A: unknown`；
     `ABI return` **0 行** ⇒ 批次 319 报的 0/194 这次是**在有捕获的前提下**复现的）；
     python_style **82 文件、191 行**（大头：31×"defaults `errors` 的 kind 装不进 `dyn`
     参数型 ⇒ 被强转"、14×`ABI coerce in call to zeta_env_set … fptosi`、
     12×numpy 双份 shim 提示）。
   - ⚠️ 这份读数额外撞出一个**独立缺陷**，已另登记为 **附 B#10**（不是 ABI 问题，
     但只有把 stderr 收回来才看得见 —— 这就是本项非做不可的证明）。
   - **批次 322 顺带修掉本项自己的一个取证缺陷**（`tools/run_all.sh:196`）：写进
     `zeta_baseline.json` 的 `jit.ok` 一直**不是测量值**。提取式
     `sed -E 's/.*ok=([0-9]+).*/\1/'` 的前缀 `.*` 是贪婪的，而 sweep 那行末尾还带
     阈值（`…，最小 ok=163`）⇒ 它跳过真读数、抓到阈值。实测同一行：贪婪式给 163、
     锚定式给 **170**。后果：JSON 里的 `jit.ok` 恒等于 `MIN_OK`，任何跨批次比对这个
     字段的动作都**结构上不可能**发现回退（批次 321 把 ok 从 163 推到 170，
     JSON 里却看不到）。已改为 `s/^jit sweep: ok=([0-9]+) .*/` 并复验
     （`{“ok”: 170}`）。CI 只把该 JSON 当 artifact 上传、没有断言读它
     （`.github/workflows/ci.yml:103-113`）⇒ 判据没被污染，被污染的是留存的证据。
10. **`official: 194/194` 里的用例程序被解析器就地截断**（批次 320 收门禁 stderr 时
   撞出，任务 #36；**不是 ABI 缺陷**，登记在此只因为它是"诊断看不见"的直接代价）：
   这些文件编译成功、也被计入 194/194，但每个都带一条
   `[W1002] … N line(s) at the end of the input were NOT parsed … DROPPED from the program`
   —— 解析器遇到第一个不认识的顶层条目就停下，**后面的整段程序不进 AST**。
   批次 320 首测：12 文件 / 丢 **1,805 行**（占该 12 文件 2,041 行的 **88%**）。
   **批次 321 已修掉其中一族**（`x @ 1..=10` 绑定模式，见下），现余 **11 文件 / 1,749 行
   / 该 11 文件合计 1,992 行 ⇒ 仍 88%**；`test_advanced_patterns.z` 全文 49 行现已全部进 AST。
   **批次 323 又修掉一族**（`s[i..]` 开区段下标，见下）⇒ 丢行 **1,749→1,222**、
   仍是 11 文件（minimal_compiler 单文件 757→230，截断点 `:45`→`:572`）。
   **批次 324 再修一族**（`r#"…"#` 原始字符串，见下）⇒ 丢行 **1,222→1,037**、文件 **11→9**
   （test_suite 127 与 bootstrap_validation_test 58 两处截断点整体消失）。
   **批次 325 只推进一族的一部分**（`'a'..='z'` 字符范围模式，见下）⇒ 丢行 **1,037→1,019**、
   仍 9 文件（advanced_patterns_test 单文件 80→62，同一截断点内还有下一族）。
   **后果**：① 官方基线对这批文件只覆盖了程序前缀，"194/194"不能读成
   "194 个程序全部编译通过"；② 任何"某语法已支持"的结论若来自这批文件，证据无效。
   - ⚠️ **批次 320 的措辞有一处错，此处更正**：当时写"首条未解析文本集中在
     `@` 模式、`impl`、`match` 体这几族"——`impl P {}`、`impl P for Q {}`、
     `1 | 2 | 3` 或模式、`unsafe {}`、`if let Some((p, q))`、嵌套 `} else if` 链
     **单独喂全都解析**（批次 321 逐个最小用例实测）。W1002 报的是**顶层条目首行**，
     病因在条目内部，按首行措辞归类必然归错。为此新增 `tools/parse_bisect.py`：
     从截断行起按语句边界逐前缀回喂编译器，第一个重新触发 W1002 的行即病因行。
   - **分类表（11 文件，逐行 = 最小用例正测 + `parse_bisect.py` 定位）**：

     | 构造 | 丢行 | 文件 | 判据 |
     |---|---|---|---|
     | `s[i..]` **开区段下标**（双边 `s[i..j]` 可解析） | ~~757~~ → **230**（批次 323 已修，见下） | minimal_compiler:401 `self.input[self.pos..].starts_with(pattern)` | 批次 322 三条最小对照用例：`s[i..j].starts_with(…)` W1002=0 ／ `s[i..].starts_with(…)` W1002=1 ／ `s[i..].len()` W1002=1。⚠️ 本行原标"`match` 作表达式"，批次 321 探针已否证（`match` 作语句同样失败），322 重隔离 ⇒ 登记为任务 #39 |
     | `static mut` 局部声明 | 357 | benchmark_simd_vs_scalar:11 | `unsafe {}` 单独喂可解析 |
     | ~~`r#"…"#` 原始字符串~~ | ~~127+58~~ → **0**（批次 324 已修，见下） | test_suite:6、bootstrap_validation_test:18 | 单喂 `let s = r#"…"#;` 触发 |
     | ~~`'a'..='z'` 字符范围模式~~ | ~~80~~ → **62**（批次 325 已修解析，下一族见下） | advanced_patterns_test:32 | 整型范围模式可解析 ⇒ 差在字符字面量；批次 325 补 `parse_char_lit` 后同一截断点内改判为 **or 模式里的构造子模式**（`Some(x @ 1) \| Some(x @ 2)`） |
     | `use a::b::C;` 在函数体内 | 85 | quantum_basic:75 | 单喂触发 |
     | 带块体的闭包实参 `f(\|\| { … })` | 58 | integration_all_features:48 | 单喂触发 |
     | `for i: usize in 0..10`（带类型标注的循环变量） | 36 | primezeta_usize_test:31 | 单喂触发 |
     | `import pkg;`（单段、无 `::`） | 19 | integration_test_program:5 | 单喂触发 |
     | `[usize; MAX + 1]` / `[0; MAX + 1]`（常量表达式数组） | 14 | test_const_expression:6 | 单喂触发 |
     | 未定位 | 158 | selfhost:57 | bisect 落在 `} else if ch.is_digit(10) {` 分支首条语句，但嵌套 else-if 最小用例通过 ⇒ **OPEN** |
   - **修法归属**：G.1/G.3（解析器 + 用例重构），不在 ABI 范围；优先级按上表丢行量。
     已关闭的一族：`x @ 1..=10` —— `src/frontend/parser/pattern.rs:22` 的 `alt()` 里
     `parse_struct_pattern`（`src/frontend/parser/pattern.rs:48`，对裸路径**故意**
     返回 `Ok(Var)`，见 `src/frontend/parser/pattern.rs:135`）排在
     `parse_bind_pattern`（`src/frontend/parser/pattern.rs:46`）之前，于是 `@` 右侧永不消费 ⇒ 整条 `fn` 连文件余部被丢。
     绑定模式前判后，截断文件 12→11、丢行 1,805→1,749，四套基线不动
     （official 194/194、python_style 285/2/4/0、corpus 39/39、jit segv=0）。
     该族修复顺带**暴露**了第二个缺口：`print(x)` 在 MIR 里发 `println_str`
     （`src/middle/mir/gen.rs:8640`），而 JIT 表里只有 `println_i64` ⇒ 新解析出的代码
     一执行就 E4016 填桩；补 `pylib/jit_mappings.txt` 七条后 jit ok **163→170**。
   - **批次 322 关掉第二族：字符串字面量作 match 模式**（`parse_lit` 只吃数字，
     模式里 `"+"` 停在引号处 ⇒ 臂拿不到 `=>`）。接线时有**两层**缺陷，第二层是
     批次 321 那张表教出来的"补完语法还得核语义"才抓到的：
     ① `src/frontend/parser/pattern.rs` 的 `alt()` 加 `parse_string_lit`
     （`:60`，`parse_lit` 之后；`parse_simple_pattern` 的 `alt()` 同步加，否则
     or 模式 `"a" | "b"` 内的子模式仍不消费）；
     ② 字符串臂若照抄整型臂的 `MirStmt::Call{func:"=="}` ⇒ 编译 0 诊断、
     **五条 expect 全部落到 `_ =>`**。原因：`==` 在 IR 里被声明为
     External `i64(i64,i64)`（`src/backend/codegen/codegen.rs:1165`），两个
     `Str` 操作数走这条路比的是**指针**。必须下成 `MirExpr::BinaryOp`
     （与 `op == "+"` 同形，后端 `codegen.rs:5938` 有 str 比较分支）。
     同形修正也 applied 到 or 模式子臂（`src/middle/mir/gen.rs` 的
     `AstNode::OrPattern` 分支）。
     **本族在 11 文件 / 1,749 行里恢复了 0 行** —— 上表把 minimal_compiler 的
     757 行标成"match 作表达式"是错的，其真因是 `s[i..]`（任务 #39）；
     字符串模式族此前根本不在这张表里，因为官方 194 个文件没有一个用得上它。
     收益是能力面 + 一条断言正确臂选择的回归用例
     （`tests/python_style/t303_match_string_pattern.z`，python_style **285→286**）。
     口径变化：`str ==` 依赖 `host_str_eq`，它定义在并发持有的
     `runtime/py_additions.c:43`（⚠️ 只引用，不改），而 `runtime/*.c` 从不进
     zetac 镜像（无 `build.rs`，见批次 315）⇒ JIT 侧无绑定 ⇒ t303 在 sweep 里
     计为 trap。总观测：total 485→486、ok 仍 170、trap 315→316、segv=0，
     且 E4016 有指名诊断（非静默）⇒ 属 G.5e"JIT 绑定三张表归一"的输入项。
   - **批次 323 关掉第三族：`s[i..]` 开区段下标**（任务 #39）。两层：
     ① 解析：`src/frontend/parser/expr.rs:2422` 新增 `slice_sep`（`:` 与 `..` 二选一，
     且拒绝 `...` 的前两字符），`src/frontend/parser/expr.rs:2440` 的起始界分支改走它。
     **为何不在 `parse_expr` 里修**：优先级链是
     `parse_additive → parse_shift → parse_range → parse_unary`，range 比加法**更紧**，
     所以 `parse_expr` 无法在 `..` 前停下 ⇒ 双边 `s[i..j]` 早已被 range 分支吃掉，
     只有**缺一侧界**时才落到切片分支。代价同源于该优先级：点号形式的起始界只吃
     `parse_unary`/`parse_postfix` 级操作数，`s[a[i]+1..]` 仍不可解析（已写进用例头）。
     ② 下型：`Box::new(v)` / `String::new()` 此前在 `PathCall` 里**什么都不发**
     （无语句、无 `exprs` 条目）⇒ 幽灵 id。经 `let` 读回是垃圾值，但作为 **struct 字面量字段值**
     会当场崩编译器：`src/backend/codegen/codegen.rs:6255` 无条件索引 `exprs[field_id]`。
     触发链 `tests/unit-tests/minimal_compiler.z:129` → 崩点 `codegen.rs:6255`（rc=101）。
     修法：`Box::new` 走**恒等**下型（`Box<T>` 槽与其内值同为 64 位句柄，
     `src/middle/mir/gen.rs:12592`），`String::new()` 下成空串字面量（`:12596`；
     行号随批次 325 的 `lower_range_guard` 插入漂移，已按锚点核对重 cite）。
     回归用例 `tests/python_style/t304_open_ended_slice.z`（5 条 expect，含 `Box::new` 作字段值）。
     恢复量：official 丢行 **1,749→1,222**（单文件 757→230）。python_style **286→287**，
     jit total 486→487 / trap 316→317（t304 用 `str_slice`，JIT 侧无绑定，同 t303 那族）、ok 仍 170。
   - **批次 324 关掉第四族：`r#"…"#` 原始字符串**。`parse_raw_string_lit`
     （`src/frontend/parser/expr.rs:364`）旧版只认 `r"…"` / `r'…'`：`alt((tag("r\""), tag("r'")))`
     在 `r#` 处直接失败 ⇒ 整条 `fn` 连同文件余部被 W1002 丢掉。这一族的共同形状是
     **把一整段被测程序嵌进一个多行字面量**（test_suite.z:6 与 bootstrap_validation_test.z:18
     都是 bootstrap 自举用例），哈希形存在的理由就是这个多行场景。
     规则按 Rust 语义实现：结束符 = 一个 `"` 后跟与开头**等量**的 `#`，内部不做转义。
     回归面三条已进用例（`tests/python_style/t305_raw_string_hash.z`）：旧形 `r"…"`/`r'…'`
     不变、字面里的 `"` 不误结束、以 `r` 开头的标识符（`rate = 5`）不被该分支吃掉。
     恢复量：丢行 **1,222→1,037**、截断文件 **11→9**；python_style **287→288**。
   - **批次 325 推进第五族：范围模式（`'a'..='z'` / `1..=10` / `x @ 1..=10`）——
     解析只是一半，另一半是"这一族此前从未活着过"**。
     ① 解析：`src/frontend/parser/pattern.rs:215` 新增 `parse_char_lit`，接进
     `parse_range_pattern` 的两端（`:239`、`:242`）与 `parse_simple_pattern` 的
     `alt()`（`:295`）。**刻意的不对称**：单引号在本语言里也是普通字符串定界符
     （`s.split(',')` 依赖它），所以 `'x'` 只在**模式位**解释为码点整数——槽位里
     char 本来就是整数，表达式位仍是 1 字符串。
     ② 下型 A（**整族此前无条件失效**）：`>=` / `<=` 两条 `MirStmt::Call` 的 dest
     **从没进过 `exprs`**，而 `gen_expr_safe`（`src/backend/codegen/codegen.rs:5689`）
     对缺失 id 静默回 `i64 0` ⇒ `&&` 永远看到 `0 && 0` ⇒ **任何范围臂都不可能命中**，
     全部落到 `_ =>`，编译 0 诊断。这一条与字符字面量无关，整型范围同样中招
     （`match 5 { 1..=10 => 111, _ => 222 }` → 222），是这段下型代码写下来起就没对过。
     ③ 下型 B（`x @ 1..=10` 专有）：条件被 `Var(inner_cond_id)` 多包了一层，
     而那一格从无写入 ⇒ `-O` 把"读未初始化 alloca"降成 `brk #0x1` ⇒ **SIGTRAP、
     一行输出都没有**。修法是把 guard 的 dest 直接交给上层读（`gen.rs:11719`）。
     ④ `inclusive` 字段此前被 `inclusive: _` 丢掉 ⇒ `..` 一直按 `..=` 运行。
     现已区分（`gen.rs:3451`）。official 194 文件里**没有一处**用排他范围作模式
     （`..` 全是 for 循环与切片，走 `MirExpr::Range` 那条独立路径）⇒ 语义变更零回归面。
     ⑤ 顺带按轴 A 去重：两处逐字相同的 30 行下型合并为 `lower_range_guard`
     （`src/middle/mir/gen.rs:3429`）。
     **恢复量只有 18 行**（advanced_patterns_test 80→62，截断点仍在 `:40`）：同一个
     未解析条目里还有下一族 —— **or 模式中的构造子模式** `Some(x @ 1) | Some(x @ 2)`
     （最小用例 `s2.z` 单喂即触发；`Some(x @ 4..=6)` 单独喂**可解析** ⇒ 缺口精确在
     or 的分支是构造子时，`AstNode::OrPattern` 的下型分支也只认 Lit/StringLit/`_`）。
     读数：`tests/python_style/t306_range_pattern_guard.z`（11 条 expect，断言**臂选择**
     而非"能编译"）⇒ python_style **288→289**；jit total 488→**489**、ok 168→**169**
     （t306 全整数、JIT 侧干净）、trap 仍 320、segv=0；official compile **194/194**、
     compile+link 仍 191/194、corpus 39/39、诊断计数逐条与批次 324 相同。
     性能：`perf_baseline.py --diff` 判定阶段 ir total **+0.6%**（阈值 10%）。
     ⚠️ 同族未修的相邻两点已登记：`q @ _`（绑定里的通配）仍返回不匹配；
     "读一个没有 `exprs` 条目的 id"这一整类静默失效属 **任务 #41** 家族。
11. **`official` 那一个数把"编译器接不接受这段源码"和"程序能否链上完整运行时"混成了同一件事**
   （批次 323 撞出，口径变更就地登记）：修完 `s[i..]` 后 minimal_compiler 多解析 527 行，
   official 从 194/194 掉到 **193/194** —— 但 `zetac` 本身没有报错，失败在 gcc 链接：
   `Undefined symbols … _chars _is_alphanumeric _is_digit _is_empty _is_whitespace _iter _nth
   _parse _push_str _to_string _unwrap _unwrap_or`（12 个未绑定的 std 方法，全部来自刚恢复的代码）。
   **后果**：只要判据仍是"编译+链接全过"，解析恢复就被运行时完整度**封顶**——每恢复一行只要引用
   一个还没绑定的方法，就报成"编译器不支持这段语法"，而这批语料（self-host 编译器）离可运行
   还差整个 std 表面。⇒ 门禁改为分两段量：
   - `src/main.rs:574` + `:926` 新增 `--no-link`（出 `.o` 即止）；
   - `tools/run_all.sh:101` 只在"整链失败"时补跑一次 `--no-link` 做归因，
     日志两行：`official: compile N/194, compile+link M/194` 与逐文件的
     `### <name> — 缺运行时绑定: <符号名…>`（明细 `$OFFICIAL_LINK_DIAG`，默认
     `/tmp/zeta_official_link.txt`）；JSON 加 `official.compile` 字段。
   - **判据（`tools/run_all.sh:575`）从 `pass==total` 改为 `compile==total`**，
     compile+link 与缺绑定清单照样打印 ⇒ 不是"把门禁绿过去"：该缺口从"一个红色计数"
     变成"指名到符号的登记表"，且真实编译失败仍然致命。本批读数：**compile 194/194、
     compile+link 193/194**。
   ⚠️ 另一条本批实测的**取证卫生**：`tools/run_all.sh:10` 与 `tools/parse_bisect.py:158`
   都指向 **`target/release/zetac`**。改了编译器而没 `cargo build --release`，门禁与 bisect
   量的就是旧二进制 —— 本批因此得到一次假"无变化"读数（bisect 仍报 45/757，直接跑新编译器已 572/230）。
   两次矛盾读数出现时，先怀疑测量（批次 321 立的规矩，此处第三次应验）。
   - **批次 324 用同一个口径量到这一族的可预测代价，读数以"能归因"为准**：修完原始字符串后
     `compile+link` 从 193/194 掉到 **191/194**（新登记的 `test_suite` 缺 `_to_string`、
     `bootstrap_validation_test` 缺 `_to_string`+`_unwrap_or_else`；`_to_string` 在 3 个
     link-only 文件里**全部出现** ⇒ 它是补齐顺序上的第一优先）。同批
     **jit ok 170→168**，逐文件可归因（`tools/jit_sweep.sh -v`）：这两个文件此前"JIT 跑通"
     是因为 `main` 整个被截断掉、它其实什么都没跑；现在真去执行恢复的代码 ⇒ 撞
     `to_string` 无 JIT 绑定 ⇒ 计 trap。也就是说 ok 的下降不是回退，而是**这两个数过去虚高**；
     trap 317→320 里剩的 +1 是新用例 t305（模块级 `let` ⇒ `zeta_module_decl`/`zeta_env_set`
     无 JIT 绑定，与 t304 同族）。硬判据 `compile==total` 仍 194/194，`ok>=163` 仍绿。

