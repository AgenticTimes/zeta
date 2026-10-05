# F.2 设计稿：统一类型检查器（路线 B：SCCP 式 slot→型不动点）

批次 910 设计定稿（2026-10-04）。前置：docs/axis-F-checker-research.md
（四先例＋四语言＋SCCP/LLM/渐进三项）、批 892 五环节链路图。

## 1. 目标与范围

在 resolver 降型之前，对每个函数体做一遍**slot→型不动点迭代**，产出
全程序唯一的 `TypeEnv`（槽→型表）；gen.rs 的现场判断逐步换查表。
驱动算法＝SCCP 式稀疏工作列表（Wegman-Zadeck 1991；LLVM
SparsePropagation.h 的格函数可插拔先例）。

本设计稿覆盖：类型格、约束来源、求解器、与现有管线的四个对接点、
灰度切换计划。

## 2. 类型格（Lattice）

```
        ⊥ Known(multiple)      ← 冲突（同槽两种已知型）
              │  meet
        Known(Type)            ← 具体型：I64/F64/Str/Bool/Named(map..)/vec…
              │  meet
        ⊤ PyDynamic            ← 未知（= PyDynamic，运行期形状分派）
```

- 格值 `LatticeTy`：`enum LatticeTy { Unknown, Known(Type), Conflict }`。
- meet(a, b)：Unknown⊕x＝x；x⊕x＝x；Known(a)⊕Known(b≠a)＝Conflict。
- Conflict 的运行期含义：保持 PyDynamic＋形状分派（893 落地形），
  绝不静默选边——890 变体 A 的教训（句柄标 F64 毒化字典面）。
- 型的相等性＝`Type` 的结构相等（现 type_map 语义）。

## 3. 约束来源（五条，全部来自现有 AST/状态，无需新信息）

| 约束 | 来源 | 例子 |
|---|---|---|
| 字面量 | Lit/Bool/StrLit/FloatLit/NoneLit | `x = 1` ⇒ x: I64 |
| 赋值边 | Assign(lhs Var, rhs) | lhs_slot ⇒ rhs_slot（同格传播） |
| 调用返回 | func_ret_types[fn]（已存在） | `y = f()` ⇒ y: ret(f) |
| 运算结果 | 二元/一元的常见型规则（保守子集） | `a + b` ⇒ meet(a, b) 的数值闭包 |
| 注解 | 参数/let 注解（已有 parse 产物） | `codes: set[str]` ⇒ DynamicArray(Str) |

刻意不入约束的：mean/len 等方法调用返回型（901 前的现场判断）——
这些留在 gen.rs 现场臂（它们依赖运行期形状），checker 只覆盖
槽位型传播。这一刀把 892 链路图的②③④三环从"逐函数回灌"改为
"checker 统一求解"，⑤（print 分派）与①（方法路由）不动。

## 4. 求解器（SCCP 工作列表）

```rust
// src/middle/checker/mod.rs（新模块）
pub struct TypeEnv {
    /// 槽（按名字建——跨函数以名字为键，槽号每函数重排不可作键）
    slots: HashMap<String, LatticeTy>,
    /// 函数返回型（推断结果，回写 func_ret_types 的来源）
    fn_rets: HashMap<String, Type>,
}

pub struct Checker<'a> {
    ast: &'a AstNode,               // 函数体
    env: TypeEnv,
    worklist: Vec<Work>,            // 待重估的语句下标
}
```

求解循环（每函数）：
1. 预填：参数注解／已知 func_ret_types／调用点实参型（627 机制产出）。
2. 顺序扫函数体，每条语句产出约束、入工作列表。
3. 弹出 Work：按格 meet 更新槽型；若槽型变化，把所有引用该槽的
   语句重新入列（def-use 邻接＝赋值/读的变量名索引）。
4. 双清空（工作列表空且无变化）⇒ 不动点。

终止性：格高度有限（每槽 ≤ 格高），meet 单调 ⇒ 必收敛；
实测函数体规模（≤百语句级）毫秒内。

## 5. 四个对接点（全部是现有机制，逐一替换）

| 现有机制（将被替代） | 对接点 | 批次 |
|---|---|---|
| body_ret_tys 登记＋回灌（resolver :5607/:5535，只 F32/F64/PyDynamic） | checker 的 fn_rets 直接写 func_ret_types（含 Str/元组等全部已知型） | P2 |
| prime_body_ret 预热（调用前降一遍只取 body 型） | checker 不动点天然与降型顺序无关 ⇒ 预热删除 | P2 |
| mean 臂的 PyDynamic 特判（879/893 系） | 查表得到 v 的型：vec ⇒ 折叠、未知 ⇒ mean_to_string（893 语义保留） | P3 |
| 896 的 slot_fallback 兜底（34 处） | 查表命中 ⇒ Known；查表 miss（真未知）⇒ 仍 slot_fallback（不删，语义收窄） | P4 |

渐进保证：checker 求解失败的槽（无约束/冲突）＝原行为
（slot_fallback/PyDynamic），因此每一步切换都可独立回退。

## 6. 灰度切换计划（每步独立验证：编译＋lib＋差分 100%＋python_style）

- P1（本批 910 骨架）：checker 模块＋格＋求解器单测
  （纯逻辑：赋值链/调用返回/冲突——不接线）。
- P2：接 mean 面（t813/t10004/t10002 三夹具＋全量差分）。
- P3：接 fromkeys/set[str] 注解面（879 系夹具回归验证）。
- P4：全面替换回灌；删除 prime_body_ret；三夹具＋全量回归。

## 7. 类设计（关键类型与职责）

```
src/middle/checker/
├── mod.rs        // Checker/TypeEnv/求解循环（本设计 §4）
├── lattice.rs    // LatticeTy＋meet（纯函数，含 6 个合同单测）
└── constraint.rs // 语句→约束的提取（match ast 逐臂，可测）

TypeEnv:
  slots: HashMap<String, LatticeTy>   // 名字→格值（跨函数以名字为键）
  fn_rets: HashMap<String, Type>      // 函数→推断返回型
  fn get_slot(&self, name) -> LatticeTy   // 查表（miss=Unknown）
  fn meet_slot(&mut self, name, LatticeTy) // meet＋变更报告

Checker:
  fn check_fn(&mut self, body: &[AstNode]) // 单函数不动点
  fn check_program(&mut self, asts)        // 跨函数（调用边传播）
```

## 8. 风险与对策

- 推断口径漂移（最大风险）：checker 结果与现状不一致 ⇒ 逐面差分
  实测（2846 例裁判）；不一致面按 892 方法定位（chain map）。
- 跨函数递归：不动点迭代天然处理（初值 Unknown，逐轮收紧）；
  与 prime 的差异＝不再依赖降型顺序。
- dict 键值缺省（map_kv 的 I64 缺省是语义性）：不属于槽位传播，
  checker 不碰。
