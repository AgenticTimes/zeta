# 存储与值表示统一架构

> 批 1026 立项。覆盖在册余量：t425 共享格、del 读回余项、缺名读 UnboundLocal 面、
> 轴 B M3 slice 2+。核心目标：**消除双存储和值表示双义这两类结构性缺陷的根因。**

## 1. 问题：一个名字两份存储

zeta 编译器里，模块全局变量有**两个存放位置**：
- **槽**（alloca）：声明作用域内的直读直写，O(1)
- **env**（哈希表）：跨函数读写的镜像

写侧有镜像（批 391），读侧按作用域分派（py_entry 门控，批 545）——
但两个位置的值可以不一致（t425 实拍：main 读到旧槽 0，peek() 读到 env 10）。

**同类缺陷**（根因相同——"值没有唯一存放点"）：

| 面 | 症状 | 在册 |
|---|---|---|
| t425 | main 读 total 拿旧槽值 0，peek() 读 env 拿新值 10 | 385 起在册 |
| del 读回 | del x 后主程序体读 x 仍拿旧槽值（env 已删但槽未清） | 998 残留 |
| UnboundLocal | fn 内 if 分支赋值、另一分支读取——zeta 拿 0，CPython UnboundLocalError | 未独立登记 |

## 2. 问题：动态值没有类型标签

i64 槽承载：整数、浮点位模式、字符串指针、容器句柄——**值不带类型**。
静态类型传不到的分派点只能运行期"猜形状"（B.1 探针族 16 处 GC_base 引用）。

**同类缺陷**：
| 面 | 症状 |
|---|---|
| keyfn 双桥 | f64 数组 bitcast / i64 数组 sitofp——每个消费点必须静态知道槽语义 |
| df[掩码] vs df[列名] | I64 槽冒充 Bool 掩码，三分派靠猜 |
| dict[str, Any] 值 | 写入点可见的类型记入侧表，动态变化即失效 |

## 3. 目标架构

### 3a. 模块全局：env 为唯一存放点

```
写：全部走 zeta_env_set（env_mirror 已有）
读：全部走 zeta_env_get（消除槽读取路径）
```

- 槽变成**可选缓存**（首次读缓存、写后失效），不参与语义
- del 清 env + 墓碑 ⇒ 后续 env_get raise ⇒ NameError ✓
- 跨函数读写一致 ✓（env 是唯一真相源）
- UnboundLocal：非函数局部名在 env 中不存在 ⇒ NameError ✓

**迁移成本**：改 call_var.rs 的 Var 读路由——`module_globals.contains(name)`
命中时从"槽优先/env 兜底"翻转为"纯 env 读"。风险：性能（每次读一次
哈希查找）＋依赖槽类型推断的下游 face（tuple 元素型等 545 门控）。

**性能缓解**：env_get 底层已按内容哈希（map_str_key）；热点路径后续可加
读取缓存（首次 env_get 后缓存指针，写侧失效）。V1 不做缓存，语义正确
优先。

### 3b. 动态值：编译期定型＋运行期 tag 双轨

- **静态可达**：维持现有类型传播（M2 fixpoint＋func_ret_types 表）
- **动态边界**：`dict[str, Any]` 写入/读出打 ZJ tag（M3 slice 1 已做
  py_map_items 解包；slice 2 补标量装箱）
- **探针退役**：每迁一域删一段（B.1 表 −3 起步）

### 3c. 编译期未定义检测

MIR-gen 数据流追踪：每个函数体内，变量首次使用是否在赋值之前。
未定义读 ⇒ zeta_name_error（与 1022 同机制，扩展到函数局部面）。

## 4. 实施里程碑

| 步 | 内容 | 规模 | 风险 |
|---|---|---|---|
| S1 | 模块全局读翻转：module_globals 命中的 Var 读从槽切 env | 1 批 | 中（t425/545 门控面） |
| S2 | 编译期 defined-before-use 数据流 | 1 批 | 低（纯新增，不改既有路径） |
| S3 | UnboundLocal/缺名读 raise 接入 | 1 批 | 低 |
| S4 | 轴 B M3 slice 2（标量装箱＋monotonic） | 2-3 批 | 高（ABI 面） |

### S1 完成状态（批 1027 校正：主体已由历史批次落地，残量改列 S4）

S1 的翻转主体**早已在库**：批 545 完成 py_entry 模块体 env-first 读（t425
摘钉）、批 391 完成写侧镜像、批 967 完成 keyfn 特化副本 FuncAddr、批 998
完成 del 墓碑 raise、批 1022 完成全表未缺名读 NameError。批 1027 补齐
墓碑读的 NameError 消息本体（原裸 zeta_raise 只打 Unhandled exception）。

批 1027 实测：t425 出 5 ✓；模块体/函数体 del 读回均打 NameError 并
rc=1 ✓；try/except 捕获后继续执行 ✓。

**三门控重新归类**（call_var.rs `env_first` 剩余闸门，不再是"待翻残量"）：

| 门控 | 归类 | 依据 |
|---|---|---|
| loop_var_active | **语义必需，保留** | 循环期间槽是新值、env 是旧值（条目绑定是 Call 不走 391 镜像）；循环结束才镜像末值（call_flow.rs）。翻转反而错 |
| Named 类实例 | S4 依赖 | env 单元往返丢"哪个构造调用造的我"（t464 双同类字段 transpose 实测）；typed cell 带出处后可翻 |
| 槽类型≠cell 类型 | S4 依赖 | env 读按声明型回填，粗于模块体绑定即降级（tuple 元素型→DynamicArray(I64) 实测读垃圾）；typed cell 后可翻 |

**S1 真正的未竟面**（登记 backlog，不属本批）：Named 全局被 del 后，
模块体读回走 Named 门控拿旧槽实例而非 NameError——静态不可知名字会被
del，需 typed cell 墓碑位（S4）才能既保 t464 出处又保 del 语义。

### S2 完成状态（批 1028 落地，a7a0712b）

编译期 defined-before-use 已实现：降低器降低函数体前预扫普通赋值目标
（`body_assigned_names`），读侧命中且程序顺序未绑定 ⇒ 运行期抛错——
函数内 UnboundLocalError、模块级 NameError（文案与 CPython 一致）。
与 1022 的全表未命中判定互补：1022 盖"名字哪都没有"，本批盖"名字
在本体有赋值但读在赋值前"（含与模块全局同名的局部遮蔽读，CPython
判局部、zeta 此前静默回落读全局）。

实测四面对齐 CPython：函数先读后赋抛错、已绑定读出值、条件真路径
不误伤（保守判定不做分支合并，只按程序顺序）、For 目标先读抛错。
门禁 rc=0（302/302＋50/50＋18/18＋40/40）。

**刻意排除面**（发散在册，不扩大）：AssignOp 目标不收集——函数内
`全局 += 1` 在 zeta 走 env 读改写（CPython 应 UnboundLocal）；Static
标记（已提升模块级格）；闭包子 MirGen 自扫自的体。

## 5. 验证策略

每步：差分 285＋python_style＋official＋语料 40/40＋金用例全绿。
S1 额外：t425 PASS 实证＋del 读回 NameError 实证。
S3 额外：try/except NameError 捕获实证。
