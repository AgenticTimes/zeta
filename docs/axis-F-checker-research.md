# 轴 F 调研：先进 checker 的实现方式（批次 907，2026-10-04）

> 服务于 F.2 checker 设计。zeta 的场景＝动态语言（Python）子集 → AOT 原生码，
> 源码大多无注解。按此场景选了四个最相关的先例：Codon（同类 AOT）、Numba
> （同类推断问题）、TypeScript（最大规模的渐进类型 checker）、渐进类型/双向
> 检查的学术口径。

## 一、Codon（MIT exaloop）——与 zeta 同类的 Python 子集 AOT 编译器

管线：parser → **TypecheckVisitor**（AST 访问器）→ Codon IR → LLVM。
- 类型检查与 IR 构造**合并在同一遍访问**：每个表达式在访问时被赋一个
  `TypePtr`，`unify()` 在遍历中传播约束；类型检查的输出直接就是 IR 构造的输入。
- **没有独立的"类型检查 pass"**——推断寄生在翻译访问器里（与 zeta 现状同构！）
  区别在 Codon 把它做得有条理：统一的表达式类型表示＋集中式 unify。
- 全程序 AOT 类型检查，不支持全动态语义（不受支持的动态特性直接报错或要求改写）。

对 zeta：现状（判断寄生在 lower 里）不是原罪——Codon 证明了这条路能走通，
关键是**统一的表达式类型表示＋集中的传播点**，而不是"推断"与"生成"分离。

## 二、Numba——独立推断 pass ＋ "未知"一等公民

管线：bytecode → interpreter.py（Numba IR，SSA 形式）→ **typeinfer.py（抽象
解释/数据流，迭代到不动点）**→ LLVM。
- 类型推断是**独立的第二个 pass**，在 IR 上做数据流分析、迭代到不动点；
  函数参数与变量的型由局部推断决定。
- 推断到不动点仍未知的值落 **object 型（动态）**，运行期派发（typeguard）——
  "未知"是一等公民，与 zeta 的 PyDynamic＋893 形状分派**同构**。

对 zeta：892 链路图五环节的断点（返回型传播、调用点型回灌、print 分派各自
为政）正是**数据流不动点迭代**要系统性解的问题——zeta 现在是"顺序单向
推断＋四处回灌补丁"，Numba 是"IR 上迭代到不动点、一处收敛"。

## 三、TypeScript——最大规模的渐进类型 checker

管线：parser → **binder**（一遍建符号表/作用域链）→ **checker**（懒式按需
定型＋记忆化）→ emitter。
- **没有独立的"类型检查 pass"**——checker（单文件 4.7 万行）在任何人要类型
  或诊断时**按需计算并缓存**（getSymbolLinks/getTypeLinks 缓存、getWidenedType、
  resolveName 等入口）。
- 循环推断（递归函数返回型）用**循环标记**而非多轮不动点。
- 类型环境＝作用域符号表链，全程序唯一。
- TS 7 原生版（Go 重写）保持同一设计——说明该架构在原生高性能场景成立。

对 zeta：892 实验的反复断裂（回灌放宽→分派又错）本质是**没有懒式＋记忆化的
统一类型查询入口**——三个型表（func_ret_types/type_map/body_ret_tys）各查各的。

## 四、学术口径

- **双向检查**（Pierce & Turner 2000 "Local Type Inference"；Dunfield &
  Krishnaswami 2019/2022 综述）：synthesis（向上综合）与 checking（向下验证）
  两模式，动态语言编译器的实用主流；高阶型推断靠它落地。
- **渐进类型**（Siek & Taha 2006）：未知是一等公民型；带格转换在边界插入
  ——zeta 的 PyDynamic＋形状分派即其工程化。
- **Typed Racket**（Tobin-Hochstadt & Felleisen）：动态语言上实用渐进类型的
  最完整先例。

## 五、共同教训（四先例一致）

1. **类型环境全程序唯一**（TS 作用域链／Numba SSA 型表／Codon 统一 TypePtr）
   ——refactor.md §10 轴 F 判据原话，无先例例外。
2. **"未知"是一等公民**（Numba object／TS unknown／渐进 ？）——zeta 的
   PyDynamic＋形状分派（893）方向正确，但传播链必须完整。
3. **推断与消费的接口要单一**——Codon 在遍历中 unify、TS 在 checker 入口
   按需计算、Numba 在 IR 上不动点——都是"一处收敛"，没有先例把类型表
   拆成三张各查各的（zeta 现状）。
4. **循环推断**：TS 用循环标记（lazily），Numba 用不动点迭代——两条路都有
   先例；zeta 的 prime_body_ret/priming 机制是不动点迭代的雏形。

## 六、对 zeta F.2 的两条路线

**路线 A（Codon 式——改造现有）**：把类型推断做进 lower 访问器（现状），
引入统一表达式类型表示＋集中传播。优点：增量可做、不建新 pass。缺点：
寄生问题只解决一半，gen.rs 仍需携类型环境。
**路线 B（Numba/TS 混合——推荐）**：resolver 后、gen 前插独立数据流
迭代 pass，产出全程序唯一 slot→Type 表（不动点）；gen 现场判断换查表
（词汇表已就位）；表外未知落 PyDynamic＋形状分派（893 工作子集即其
运行期配套）。优点：五环节断点的源头对齐，892 链路图整体作废重排为
"查表"。缺点：推断口径差异需灰度（全量差分 2846 例为裁判）。

**建议**：路线 B。首步＝在 resolver.lower_to_mir 之前对函数体做一遍
"slot→型"不动点迭代（现 body_ret_tys/prime 机制已是其雏形——把它从
"F32/F64/PyDynamic 专用"推广为通用型传播）。五环节链路图（批 892）
的①③④三环随之自然收敛。



## 七、补充调研：Nim／Go／Rust／LuaJIT（2026-10-04 追加）

### Nim（静态，编译到 C）
- 管线：lex/parse → **sem.nim 语义检查 pass**（semExpr/semTypeNode 定型、
  semfold 常量折叠、vm 编译期求值）→ C 生成。
- 类型检查是**独立语义 pass**，产出带型 AST 供 codegen 消费；1.4 的增量
  编译（IC）围绕 sem pass 缓存带型 AST。
- **对 zeta**：sem pass → 带型 AST → codegen＝路线 B 的直系先例（判据原话
  "类型环境全程序唯一"在此成立）。

### Go（静态，自举）
- 管线：noder（syntax→nodes）→ **types2（类型检查）** → unified IR →
  irgen → walk → SSA。
- **关键先例**：Go 1.18 起把编译器内手搓的类型检查器**替换为 go/types 的
  移植版 types2**——消灭编译器与标准库两套类型系统各自漂移的问题。
  unified IR 让导入包与编译包走同一表示。
- **对 zeta**：这正是轴 F 的终极形态——**杀掉手搓重复、移植唯一判定**。
  896–904 的命名合同（is_map/is_dynamic/is_untyped/slot_fallback）就是
  "可移植词汇表"的雏形；F.2 的目标＝把它们背后的判定逻辑集中成一个
  zeta 版 types2。

### Rust（静态，自举，两级检查）
- 管线：AST→HIR → **typeck pass**（FnCtxt＋InferCtxt＋trait solving，按 item
  产出 **TypeckResults** 表）→ MIR → **MIR typecheck**（借用检查内的第二级
  型检查）→ 优化。
- 两个先例：①typeck 产出 **TypeckResults 表**、下游阶段只查表不重推
  （＝路线 B 的产出物形态）；②**两级检查**（HIR 源级型检查＋MIR 降级后
  的二级核验）——zeta 未来可在轴 G 加 MIR 级核验器（现登记项）。
- 推断与检查分离：rustc_infer（推断上下文）与 rustc_hir_typeck（检查驱动）
  是两个 crate——"推断机制"与"检查驱动"分家，机制可被多驱动复用。

### LuaJIT（动态 → JIT，无静态检查器）
- **完全没有静态类型检查**：解释器跑通用字节码；热路径 trace 记录时按
  **运行期类型特化**（ADD 按实际型分裂成 ADDLN/ADDIV），guard 校验假设、
  失败走 side trace 或经 snapshot 退回解释器。
- parser 的 Dyndata 是**词法/语法期的成长数组记账**（表达式回填表、常量表），
  与类型无关（899 目录名混淆澄清：Dyndata ≠ 型机制）。
- **对 zeta**：LuaJIT 证明动态源可以在**无静态检查器**的前提下达到原生速
  度——条件是运行期特化＋guard。zeta 的 893 落地形（PyDynamic＋形状分派）
  即同一思想的 AOT 版；**真正不可静态判定的面走运行期守卫是被 LuaJIT 验证
  过的终局方案**，#279 方案②的格标签就是 zeta 版 guard。

### 补充结论

1. 静态语言（Nim/Go/Rust）一致采用：**独立类型 pass → 带型表 → 下游查表**
   （路线 B 有三个静态语言先例）；Go 的 types2 移植是"杀手搓重复"的终极形态。
2. 动态 → AOT（zeta/LuaJIT）一致采用：**可静态判定的静态定，不可判定的
   运行期守卫**——zeta 的 893 落地形方向被 LuaJIT 长期验证。
3. Rust 的两级检查（HIR typeck ＋ MIR typecheck）提示 zeta 的终局可加
   MIR 级核验器（轴 G 登记项）。
4. 之前的四先例（Codon/Numba/TS/渐进类型）结论不变，本批补充的四个静态
   语言先例使路线 B 的先例证据从"动态语言推断系统"扩展到"静态语言主流
   编译器"——**先例证据完备**。

## 来源

- Codon：官方文档（Compilation Flow）、USENIX 2023 论文（Shajii et al.）、
  github.com/exaloop/codon
- Numba：官方 Architecture 文档、numba/core/{interpreter,typeinfer}.py、
  arXiv 2021 综述
- TypeScript：github.com/microsoft/TypeScript/wiki（Architectural Overview）、
  src/compiler/checker.ts、TS 7 原生移植报道
- 学术：Pierce & Turner 2000；Dunfield & Krishnaswami 2019/2022；
  Siek & Taha 2006；Typed Racket 系列
- 补充：Nim 编译器内部（sem.nim/semfold.nim/vm.nim，IC 1.4 发布说明）；
  Go cmd/compile 架构（noder/types2/unified IR，types2 移植 CL）；
  rustc-dev-guide（type-checking/traits/resolution 章，FnCtxt/InferCtxt/
  TypeckResults）；LuaJIT 内部（trace/guard/snapshot，lj_record，SO 与
  Cornell CS6120 讲义）
