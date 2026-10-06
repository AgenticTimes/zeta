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

## 5. 验证策略

每步：差分 285＋python_style＋official＋语料 40/40＋金用例全绿。
S1 额外：t425 PASS 实证＋del 读回 NameError 实证。
S3 额外：try/except NameError 捕获实证。
