# gen.rs 重构收官报告（批次 814–893，2026-10-04 更新）

## 结论

gen.rs 从 19392 行降至 **3829 行（净 -15563 行，-80.2%）**。
lower_ast_inner 与 lower_expr_node 的内联臂**全部清零**（仅剩一行委托与
getattr 域——getattr×4 顺序敏感臂按 YAGNI 登记维持原位）；
逻辑分布于 gen/ 下 **32 个家族文件**；路由决策唯一化（classify_call，
含 887 补的构造器族映射）；全库内置测试 158/158、python_style 479/0、
全量差分 match=2845/2845＝100%。轴 D 原始验收（净减 ≥1500 行）超额 9.4 倍。

## 家族文件清单（32 个，按族）

| 文件 | 家族 |
|---|---|
| call_class.rs | 路由分类器 classify_call ＋ type_name_of（887 补构造器族映射＋单测钉） |
| call_dispatch.rs | Call 臂主干（5840 行分派链，876 迁入） |
| call_binary.rs | BinaryOp 全族（1485 行，869 零适配法重做成功） |
| call_match.rs | Match 表达式（599 行） |
| call_print.rs | print 按格渲染链＋PyDynamic 形状分派（893） |
| call_field.rs | 字段读＋类变量路由（866 转发修复） |
| call_subscript.rs | 下标读（888 补 dest 写回） |
| call_expr_lit.rs | Tuple/StructLit/ArrayLit/DynamicArrayLit/ArrayRepeat（872/886） |
| call_unary.rs | UnaryOp（884 浮点负号提前返回修复） |
| call_dict.rs | DictLit（873） |
| call_flow.rs | Loop/While（861/862 重做＋迁移） |
| call_if.rs | If 表达式＋If 语句（859/864） |
| call_fstring.rs | f-string（861 重做） |
| call_var.rs | Var 读（881） |
| call_path.rs | PathCall（885） |
| call_patterns.rs | BindPattern/RangePattern/OrPattern/StructPattern（891） |
| call_builtin.rs / call_len.rs / call_str.rs / call_json.rs / call_num.rs / call_assert.rs / call_re.rs / call_logging.rs / call_ctor.rs / call_set.rs | 内建与方法子族（814–844） |
| stmt_assign.rs / stmt_let.rs / stmt_funcdef.rs / stmt_misc.rs | 语句臂（865/867/874/875/886） |
| lower_closure.rs | 闭包降型＋Closure 表达式臂（877/891） |
| expr_small.rs | Cast/Range/BigIntLit/Unsafe/TimingOwned（891） |

## 修复批附带产出（879–893）

zeta_vec_union（set 并集）、mean 接收者双路＋zeta_mean_to_string（#279 方案②）、
混型列表逐元素形状判别（#273，flat＋nested）、signature_ret_ty 保留
PyDynamic 身、回灌放宽 PyDynamic、print PyDynamic 形状分派面。
缺陷核销 11 项（#273～#278 主面＋t79/t274/t217/t813）。

## 轴 D 判据对照

| 判据 | 状态 |
|---|---|
| lower_expr 净减 ≥1,500 行 | ✅ 净 -15563（超额 9.4 倍） |
| MIR diff 为空（等价搬家） | ✅ 每批行为探针＋全量差分 100% |
| frontend→middle 边为 0 | ✅ 未引入新越界 |

## 方法论沉淀（全部有事故实证）

1. **零适配法**（869）：迁移签名取臂的原样解构类型（&String/&Box），臂体
   一字不改——857 的失败根因就是签名类型与解构类型不符。
2. **表达式臂委托必须转发返回值**（866，三次现身：866/886/891）——statement
   臂才可省略 return。
3. **行号手术三律**：发现与拼接同一快照、多行解构头从 `=>` 行后取体、
   每臂拼接后立即括号深度扫描（860/872 事故沉淀）。
4. **可信读数一律独立工作树干净构建**（共享主树的增量构建/bisect 会产假读数，
   890 再证）；**构建命令直接看输出尾部**，不数 error 行（管道吞退出码四次事故）。
5. 长任务必须有实时进度输出＋指纹缓存（841/842，用户裁定入 AGENTS.md）。
6. 门禁全局跑走 `--group 50` 合并编译（809 落地、893 接入门禁，
   全量差分约 40 分钟→4 分钟）。

## 剩余（全部有登记依据）

- getattr×4 顺序敏感臂：YAGNI 维持（与其他 face 交错共享 Batch 405 ghost
  守卫，已住 call_dispatch 家族文件内）。
- #279 方案②的完整格标签机制：mean 面已用 PyDynamic＋形状分派落地；
  完整格标签（值标签大弧延伸）待独立排期。
- gen.rs 剩余 3829 行构成：一行委托＋getattr 域＋模块级自由函数＋impl 胶水。
