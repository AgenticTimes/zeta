# Zeta 编译器类设计 · 各层类设计 / 类函数设计 / 调用链条（classdesign.md）

> 本文档是 [pyramid.md](pyramid.md)（编译器金字塔 0-7 层）与 [refactor.md](refactor.md)（七轴 A-G）的
> **代码映射层**：pyramid 说「每层必须做什么」，refactor 说「欠债在哪」，本文说
> 「**每层由哪些类承载、每个类有哪些函数、这些函数按什么链条互相调用**」。
>
> 数据来源：2026-09-22 对 `src/`（89,712 行 Rust）+ `runtime/`（8,456 行 C）+
> `pylib/registry.txt` 的实测清点。类名、函数签名、行号均为源码实证。
>
> **读法**：每层给四样东西——
> ① pyramid 层职责锚点；② **类设计**（类的职责 + 字段）；③ **类函数设计**（方法签名表 + 职责）；
> ④ **调用链条**（谁调谁，含跨层接口）。末尾标该层 refactor 轴债务。
>
> 签名沿用 Rust 源码原文（省略 `pub` 前缀与生命周期标注），行号 = 定义行。

---

## 0. 全局：端到端调用链

`src/main.rs:502` 的 `main()` 是唯一驱动，串起全部层。每一跳都是一个类的方法调用：

```text
main()                                   main.rs:502
 │
 ├─ scheduler::init_runtime()
 ├─ 参数解析（--dump-mir/--emit-llvm/--strict-abi/--report-untyped/--report-stubs/--no-link/--list-stubs）
 │
 ├─ ① fs::read_to_string(file)                              读源
 ├─ ② parse_zeta(code) -> IResult<&str, Vec<AstNode>>      top_level.rs:1590
 │      ↓ Vec<AstNode>
 ├─ ③ ensure_fully_parsed(remaining, code, file)           main.rs:391
 ├─ ④ const_eval::evaluate_constants(&asts)                const_eval.rs（CTFE）
 │      └─ 过滤 comptime-only FuncDef
 ├─ ⑤ Resolver::new()                                      resolver.rs:120
 ├─ ⑥ resolver.set_source_dir(path)                        resolver.rs:2142
 ├─ ⑦ resolver.expand_macros(&asts) -> Result<…, String>   resolver.rs:3361
 ├─ ⑧ resolver.register(ast) × N                           resolver.rs:183
 ├─ ⑨ resolver.infer_untyped_returns(&asts)                resolver.rs:1200
 ├─ ⑩ resolver.typecheck(&asts) -> bool                    （失败仅 W0003）
 ├─ ⑪ resolver.get_registered_funcs() -> Vec<AstNode>      resolver.rs:3475
 ├─ ⑫ resolver.lower_to_mir(ast) × N -> Mir                resolver.rs:3000
 ├─ ⑬ resolver.take_generated_closures() -> Vec<Mir>       resolver.rs:3165
 ├─ ⑭ resolver.collect_used_specializations(&asts)         resolver.rs:3211
 ├─ ⑮ resolver.monomorphize(key, ast) -> AstNode           resolver.rs:3298
 ├─ ⑯ resolver.record_mono(key, mir)                       resolver.rs:2129
 ├─ ⑰ all_mirs.sort_by(name)                               main.rs:782
 ├─ ⑱ [--dump-mir] Mir::dump_canonical()                   mir.rs:36
 ├─ ⑲ LLVMCodegen::new(&context, "module")                 codegen.rs:81
 ├─ ⑳ codegen.gen_mirs(&all_mirs)                          codegen.rs:1407
 ├─ ㉑ codegen.report_abi_coercions()                       codegen.rs:1342
 └─ ㉒ finalize_and_aot(&codegen, path, target)             jit.rs:256
        ├─ codegen.module.verify()
        ├─ 目标写盘
        └─ clang 链接 → 可执行
```

**五个「唯一接口」点**（pyramid 7.1「每阶段一个模块、依赖单向」的落点）：

| 阶段边界 | 接口类型 | 定义处 |
|---------|---------|--------|
| 文本 → AST | `Vec<AstNode>` | `src/frontend/ast.rs:28` |
| AST → 符号表 | `Resolver::register` | `resolver.rs:183` |
| AST → MIR | `Mir` | `src/middle/mir/mir.rs:6` |
| MIR → LLVM IR | `LLVMCodegen::gen_mirs` | `codegen.rs:1407` |
| IR → 运行期符号 | `PyMember.symbol`（registry 驱动） | `src/middle/pylib.rs:25` |

**反向依赖实测**：`backend → frontend` = 0（方向正常）；`frontend → middle` 有 5 处越层
（轴 D.2）。

> ℹ 本节是**跨层**链条（类 → 类）。每个类**自身**的方法调用链见 §9（类内调用链）。

---

## 1. 层 0：公理层（源语义 ≡ 目标语义）

**pyramid 锚点**：全程服从唯一公理，任何变换不得违反。

**类设计**：本层无 class —— 公理由**门禁工具**具现。

| 载体 | 职责 | 位置 |
|------|------|------|
| `run_all.sh` | 三基线门禁（官方 194 · python_style · 语料） | `tools/run_all.sh` |
| `mir_diff.sh` | 变换前后 MIR 字节 diff | `tools/mir_diff.sh` |
| `diff_test.py` | CPython 差分（参考实现作 oracle） | `tools/diff_test.py` |
| `OptLevel`（enum） | 优化级别维度 | `src/middle/optimization.rs:9` |

**调用链条**：

```text
改动 → run_all.sh ─┬─ 官方 194（compile-only）
                   ├─ python_style（端到端断言值）
                   └─ 语料 39（解析成功率）
     → mir_diff.sh（lowering 不变性）
     → diff_test.py（运行值等价）
```

**债务**：轴 E.1 —— 官方 194 只编译不运行，「公理」的证明力低于其声明。

---

## 2. 层 1：语言规范层

**pyramid 锚点**：语法规范 + 静态语义规范 + 动态语义规范；规范是「正确」的裁判。

### 2.1 类设计

| 类 | 职责 | 关键字段 |
|----|------|---------|
| `ErrorCodeRegistry` | 错误码规范（诊断 oracle） | 错误码 → 标题/建议/示例 | 
| `PyModule` | 运行期模块规范 | `name` / `aliases` / `members: Vec<PyMember>` / `noop` |
| `PyMember` | **单个符号的完整契约** | `name` / `symbol` / `args: Vec<String>` / `ret` / `handle` / `decl` / `stub` / `alias_of` |
| `PyMethod` | 句柄方法契约（含接收者） | `handle` / `method` / `symbol` / `arity` / `ret_handle` / `ret` / `decl` / `stub` |
| `PyHelper` | 编译器内部助手（`X` 行，不可 import） | `symbol` / `args` / `ret` |
| `StubSite` | 假值桩调用点记录 | 调用位置信息 |
| `registry.txt` | **规范本体**（纯文本 schema，代码由它生成） | 5 类行：`F`/`W`/`X`/`N`/`S` |

### 2.2 类函数设计

**`PyMember` / `PyModule` / `PyMethod` / `PyHelper`** —— 纯数据类，无方法；
能力由 `pylib` 的**自由函数族**提供（数据驱动设计：加库 = 改 txt，不改 Rust）：

```rust
// pylib.rs —— 查询族（编译期单例，OnceLock 缓存）
pub fn find_module(name: &str) -> Option<&'static PyModule>                    // :273
pub fn find_member(module: &str, member: &str) -> Option<&'static PyMember>    // :285
pub fn method_symbol(handle: &str, method: &str) -> Option<(&'static str, Option<&'static str>)>  // :297
pub fn method_by_unique_name(...)                                              // :307
pub fn known_module_names() -> Vec<&'static str>                               // :328
pub fn all_externs() -> Vec<(&'static str, Vec<&'static str>, &'static str)>   // :334  ← codegen 声明来源
pub fn packages_dir() -> std::path::PathBuf                                    // :373
pub fn is_noop_module(module: &str) -> bool                                    // :385  （__future__ 类）
pub fn method_ret(handle: &str, method: &str) -> Option<&'static str>          // :390
pub fn handle_op(op: &str, left: &str, right: &str) -> Option<(&'static str, &'static str)>  // :401  运算符分派
pub fn handle_tag(t: &str) -> Option<&'static str>                             // :452
// 桩管理族
pub fn stub_symbols() -> Vec<&'static str>                                     // :506
pub fn pylib_file_stubs() -> Vec<&'static str>                                 // :531
pub fn all_stub_symbols() -> Vec<&'static str>                                 // :563  （--list-stubs）
pub fn stub_sites() -> Vec<StubSite>                                           // :645
pub fn stub_call_match(target: &str) -> Option<(Vec<&'static str>, bool)>       // :660
pub fn lookup_declared_symbol(name: &str) -> Option<&'static str>              // :686
```

**`ErrorCodeRegistry`**：

```rust
registry.get(code) -> Option<&ErrorCodeInfo>     // error_codes.rs（--explain 用）
registry.all_codes() -> Vec<...>                 // 无参时列出全部
```

### 2.3 调用链条

```text
pylib/registry.txt
  ↓ include_str!                    pylib.rs:22（编译期内联）
parse_registry() → (Vec<PyModule>, Vec<PyHelper>)
  ├─→ all_externs() ────→ codegen 声明 extern（按 args/ret 精确生成，防 f64 被 fptosi 截断）
  ├─→ method_symbol() ──→ MirGen 方法分派（按 handle 精确匹配，取代名字猜测）
  ├─→ handle_op() ──────→ 二元运算符的运行期符号选择
  ├─→ handle_tag() ─────→ 句柄 → tag 编码
  └─→ all_stub_symbols() → --list-stubs / check_registry_symbols.sh 核对 nm
```

**债务**：E.3 —— `parse_registry` 无 JSON fixture 契约测试，规范与实现靠人工同步。

---

## 3. 层 2：前端层（职责一「读得准」）

### 3.1 缩进预处理（2.1 词法）

**类设计**：Python 式缩进需先转成显式块。

| 类/类型 | 职责 | 位置 |
|---------|------|------|
| `LineInfo` | 单行缩进信息（缩进宽度、是否续行） | `indent.rs:148` |
| `IndentError` | 缩进错误（不一致的 dedent 等） | `indent.rs` |

**类函数设计**：

```rust
pub fn indent_preprocess(input: &str) -> Result<Option<String>, IndentError>  // indent.rs:164  主入口
pub fn set_last_preprocess(text: String, origins: Vec<usize>)                 // :38   保存映射
pub fn clear_last_preprocess()                                                // :42
pub fn original_line_at(byte_offset: usize, fallback_source: &str) -> usize   // :48   变换后偏移 → 原行号
pub fn remaining_byte_offset(remaining: &str, fallback_source: &str) -> usize // :62
```

**`Position` / `Span` / `LocatedInput<'a>`**（诊断定位）：

```rust
pub fn position_from_offset(source: &str, offset: usize) -> Position   // location.rs:191
pub fn extract_line(source: &str, line_num: usize) -> Option<&str>     // :207
pub fn create_context(source: &str, span: Span) -> String              // :212  错误上下文渲染
pub fn with_location<'a, P, O>(...)                                    // :133  给组合子附位置
pub fn parse_error_diagnostic(...)                                     // :168
```

### 3.2 语法分析（2.2）

**类设计**：`AstNode` 是唯一 AST 表示（enum，`ast.rs:28`），
变体含 `Program` / `ConceptDef` / `ImplBlock` / `Method` / `FuncDef` / `ExternFunc` / …

**类函数设计**（nom 组合子，按文法层级分三档）：

```rust
// 顶层：top_level.rs
pub fn parse_zeta(input: &str) -> IResult<&str, Vec<AstNode>>        // :1590  ★ 唯一入口

// 表达式：expr.rs（3184 行）
pub fn parse_expr(input: &str) -> IResult<&str, AstNode>             // :3180  ★ 表达式入口
pub fn parse_full_expr(input: &str) -> IResult<&str, AstNode>        // :3193
pub fn parse_primary(input: &str) -> IResult<&str, AstNode>          // :1531  原子（字面量/调用/括号）
pub fn parse_lit(input: &str) -> IResult<&str, AstNode>              // :145
pub fn parse_string_lit(input: &str) -> IResult<&str, AstNode>       // :293
pub fn parse_condition(input: &str) -> IResult<&str, AstNode>        // :744
pub fn parse_match_expr(input: &str) -> IResult<&str, AstNode>       // :3088

// 词法原语：parser.rs
pub fn skip_ws_and_comments(input: &str) -> IResult<&str, ()>        // :39
pub fn ws<'a, P>(...)                                                // :65   空白包裹
pub fn parse_ident(input: &str) -> IResult<&str, String>             // :74
pub fn parse_member_ident(input: &str) -> IResult<&str, String>      // :147
pub fn line_comment / block_comment                                   // :16 / :35
pub fn kw_boundary<'a>(input: &'a str, word: &str) -> Option<&'a str> // :130  关键字边界
```

### 3.3 名称解析 + 类型检查（2.3 / 2.4）—— `Resolver`

**类设计**：`Resolver` 是**前端唯一的符号与类型所有者**（4,705 行）。
字段（`resolver.rs:35` 起）：

| 字段组 | 字段 | 职责 |
|--------|------|------|
| 符号表 | `funcs: HashMap<String, FuncSignature>` | 函数签名（参数类型 + 返回类型） |
| 符号表 | `registered_funcs: HashMap<String, AstNode>` | 已注册函数 AST |
| 符号表 | `impls: HashMap<(String,String), Vec<AstNode>>` | `impl` 块索引 |
| 类型 | `type_decls: HashMap<String, TypeDecl>` | 程序级类型声明（enum/alias） |
| 类型 | `associated_types: HashMap<(String,String), String>` | 关联类型绑定 |
| MIR 产出 | `cached_mirs` / `mono_mirs: HashMap<MonoKey, Mir>` | 降级结果 + 单态化实例 |
| 闭包 | `generated_closures: RefCell<HashMap<String, Mir>>` | 合成闭包 MIR 暂存 |
| 作用域 | `nonlocal_names` / `module_globals: RefCell<HashSet<String>>` | Python 作用域规则 |
| 模块 | `module_resolver: ModuleResolver` | `use` 路径 → 文件 |
| 模块 | `py_module_aliases` / `py_member_aliases` / `py_module_reexports` | `import X as a` 等别名映射 |
| 宏 | `macro_expander: MacroExpander` | 宏展开器 |
| 借用 | `borrow_checker: RefCell<BorrowChecker>` | 借用检查 |
| CTFE | `ctfe_consts: HashMap<String, ConstValue>` | 编译期常量 |

**类函数设计**（22 个 pub fn，按调用序列分组）：

```rust
// ── 构造与配置 ──
pub fn new() -> Self                                                    // :120
pub fn set_source_dir(&mut self, path: &std::path::Path)                // :2142  （use super:: 解析基准）
pub fn persist_specialization_cache(&self)                              // :174

// ── 阶段② 注册 ──
pub fn register(&mut self, ast: AstNode)                                // :183   ★ 符号表填充
pub fn expand_macros(&mut self, asts: &[AstNode]) -> Result<Vec<AstNode>, String>  // :3361
pub fn get_registered_funcs(&self) -> Vec<AstNode>                      // :3475  ★ 交给 MIR 的函数集

// ── 阶段③ 类型 ──
pub fn infer_untyped_returns(&mut self, _asts: &[AstNode])              // :1200  未注解返回推断
pub fn report_untyped_params(&self) -> Vec<(String, String)>            // :1176  （--report-untyped）
pub fn get_func_signature(&self, name: &str) -> Option<&(Vec<(String,Type)>, Type, bool)>  // :1164
pub fn get_all_func_signatures(&self) -> &HashMap<String, (Vec<(String,Type)>, Type, bool)> // :1171
pub fn module_global_types(&self) -> HashMap<String, Type>              // :1659  → 注入 MirGen
pub fn func_param_names(&self) -> HashMap<String, Vec<String>>          // :2113  → 注入 MirGen
pub fn is_nonlocal_name(&self, name: &str) -> bool                      // :3160  → 注入 MirGen
pub fn resolve_impl(&self, concept: &str, ty: &str) -> Option<Vec<AstNode>>  // :1156

// ── 阶段④ 降级 ──
pub fn lower_to_mir(&self, ast: &AstNode) -> Mir                         // :3000  ★ 组装 MirGen
pub fn take_generated_closures(&self) -> Vec<Mir>                        // :3165

// ── 阶段⑤ 单态化 ──
pub fn collect_used_specializations(...)                                // :3211
pub fn monomorphize(&self, key: MonoKey, ast: &AstNode) -> AstNode      // :3298
pub fn record_mono(&mut self, key: MonoKey, mir: Mir)                   // :2129
pub fn is_abi_stable(&self, key: &MonoKey) -> bool                      // :2125

// ── 其他 ──
pub fn py_module_init_symbol(&self, module: &str) -> Option<String>      // :2555  模块 `__init` 符号名
```

**⚠ 关键缺口**：`typecheck(&asts) -> bool` **不在此列表**——它是 `unified_typecheck.rs` 的自由函数
（`resolver.rs:2255` 附近经 `unified_typecheck::resolver()` 转发），且**失败只发 W0003 继续**。
pyramid 2.4 要求「类型检查是独立 pass」——**当前不满足**（轴 F）。

### 3.4 类型表示（2.4）

**类设计**：

| 类 | 职责 | 位置 |
|----|------|------|
| `Type`（enum，40+ 变体） | 全程序统一类型表示 | `types/mod.rs:94` |
| `TypeVar(u32)` | 类型变量（推断占位） | `types/mod.rs:40` |
| `TypeParam` | 类型参数（名 + kind + bound） | `:203` |
| `GenericContext` | 泛型上下文（参数表 + 父链） | `:237` |
| `Substitution` | 类型代换表 | `:1306` |
| `KindContext` / `LifetimeContext` / `AssociatedTypeContext` / `TypeFamilyContext` | 高级类型系统 | `types/{kind,lifetime,associated,family}.rs` |

**`Type` 的变体全集**（`mod.rs:94` 起）：`I8/I16/I32/I64/U8/U16/U32/U64/Usize` ·
`F32/F64` · `I32x4/I64x2/F32x4/V4I64`（SIMD）· `Bool/Char/Str/Range` ·
`Array(Box<Type>, ArraySize)` / `Slice` / `DynamicArray` / `Tuple` / `Ptr` / `Ref` /
`Named(String, Vec<Type>)` / `TraitObject` / `Function(Vec<Type>, Box<Type>)` /
`AsyncFunction` / `Variable(TypeVar)` / `Vector(Box<Type>, ArraySize)` /
`Constructor(String, Vec<Type>, Kind)` / `PartialApplication(Box<Type>, Vec<Type>)` / `Error`

**类函数设计**：

```rust
// ── 构造与转换 ──
pub fn from_string(s: &str) -> Type                                     // :283  ★ 注解字符串 → Type
pub fn display_name(&self) -> String                                    // :742   用户可读名
pub fn mangled_name(&self) -> String                                    // :854   ★ 符号修饰（4.3.d）

// ── TypeVar（:40 单独 impl）──
pub fn fresh() -> Self                                                  // :44    （AtomicU32 计数器，仅供推断占位）

// ── 查询谓词 ──
pub fn contains_vars(&self) -> bool                                     // :714
pub fn is_regular(&self) -> bool                                        // :1139  （数学性质）
pub fn is_integer(&self) -> bool                                        // :1177
pub fn is_floating_point(&self) -> bool                                 // :1193
pub fn is_vector(&self) -> bool                                         // :1225
pub fn as_vector(&self) -> Option<(&Type, usize)>                       // :1230
pub fn mathematical_properties(ty: &Type) -> Vec<String>                // :1199  （commutative 等）

// ── 泛型 ──
pub fn instantiate_generic(&self, type_args: &[Type]) -> Result<Type, String>   // :1006
pub fn instantiate_generic_with_bounds(...)                                      // :1807

// ── 概念/约束 ──
pub fn satisfies_concept(&self, ty: &Type, concept: &Type) -> bool      // :1963
pub fn satisfies_bound(&self, ty: &Type, bound: &TraitBound) -> bool    // :2027
pub fn concept_parent(concept: &Type) -> Option<Type>                   // :2013

// ── GenericContext ──
pub fn new() -> Self                                                    // :244
pub fn with_parent(parent: GenericContext) -> Self                      // :260
pub fn find_type_param(&self, name: &str) -> Option<&TypeParam>         // :268
pub fn add_type_param(&mut self, param: TypeParam)                      // :276

// ── Substitution ──
pub fn new() -> Self                                                    // :1312
pub fn apply(&self, ty: &Type) -> Type                                  // :1319  ★ 代换应用
pub fn unify(&mut self, t1: &Type, t2: &Type) -> Result<(), UnifyError>  // :1491  ★ 类型合一（推断核心）

// ── TypeParam ──
pub fn new(name: String) -> Self / with_kind(name, kind) / with_bound(self, bound)  // :211/:220/:229

// ── 派生宏 ──
pub fn handle_derive_attribute(attr: &str, type_name: &str) -> Result<Vec<String>, String>  // :1246
```

### 3.5 降糖与 CTFE（2.5）

| 类 | 职责 | 位置 |
|----|------|------|
| `MacroExpander` | 声明式宏展开 | `macro_expand.rs:37` |
| `AdvancedMacroExpander` | 增强宏（卫生 `HygieneLevel`、片段 `FragmentType`） | `macro_expand_advanced.rs:79` |
| `DeclarativeMacro` / `MacroPattern` / `MacroToken` | 宏定义表示 | `macro_expand.rs:14/21/28` |
| `ConstEvaluator`（ctfe） | 编译期求值器主体 | `ctfe/evaluator.rs:43` |
| `LegacyCompatConstEvaluator` | 旧兼容常量折叠 | `const_eval.rs:30` |

```rust
// ctfe/evaluator.rs —— 自由函数式入口 + 实例方法
pub fn evaluate_program(asts: &[AstNode]) -> CtfeResult<Vec<AstNode>>   // :1307  ★ 自由函数入口
pub fn eval_const_expr(expr: &AstNode) -> CtfeResult<ConstValue>        // :1301
                              // 实例方法
pub fn new() -> Self                                                    // :43
pub fn evaluate_program(&mut self, asts: &[AstNode]) -> CtfeResult<Vec<AstNode>>  // :52
pub fn eval_const_expr(&mut self, expr: &AstNode) -> CtfeResult<ConstValue>       // :580
pub fn try_eval_const_call(...)                                          // :1278
```

### 3.6 调用链条（前端内部）

```text
源文本
 └─ indent_preprocess(input)                      indent.rs:164
      ↓ 显式块标记文本
 └─ parse_zeta(&str)                             top_level.rs:1590
      └─ ws / skip_ws_and_comments                parser.rs:65/:39
      └─ parse_ident / parse_member_ident          parser.rs:74/:147
      └─ 顶层分派：FuncDef / ConceptDef / ImplBlock / ExternFunc
           └─ 语句 → stmt.rs 组合子
                └─ 表达式 → parse_expr(expr.rs:3180)
                     └─ parse_primary(:1531) → parse_lit(:145) / parse_string_lit(:293)
      ↓ nom many0 —— ⚠ 遇顶层不可处理项静默截断其后全部（W1002）
 └─ ensure_fully_parsed(remaining, code, file)    main.rs:391（把截断变硬错误）
 └─ resolver.expand_macros(asts)                  resolver.rs:3361
      └─ MacroExpander / AdvancedMacroExpander
 └─ resolver.register(ast) × N                    resolver.rs:183（填 funcs/impls/type_decls）
 └─ resolver.infer_untyped_returns               resolver.rs:1200
 └─ typecheck（unified_typecheck.rs，失败 W0003）
 └─ resolver.get_registered_funcs()               resolver.rs:3475
```

### 3.7 前端与 pyramid 差距

| pyramid 要求 | 现状 | 轴 |
|-------------|------|----|
| 类型检查是**独立 pass** | ✗ 无 `Resolver::typecheck` 方法；定型寄生 parser/MIR/codegen 三处 | **F** |
| 解析错误**恢复并继续** | ✗ `many0` + W1002 静默截断 | G.7 |
| 降糖**时点统一** | ⚠ 散布 resolver/MIR/codegen | F.1 |
| 前端不依赖中端 | ✗ 5 处 `frontend → middle` | D.2 |

**层内规模热点**：`resolver.rs` 4,705 行 + `new_resolver.rs` 2,160 行（**双轨未决断**，轴 A）。

---

## 4. 层 3：中端层

### 4.1 IR 类设计 —— `Mir`

**职责**：函数级唯一规范表示（`mir.rs:6`）。

| 字段 | 类型 | 职责 |
|------|------|------|
| `name` | `Option<String>` | 函数名（`None` = 匿名） |
| `param_indices` | `Vec<(String, u32)>` | 形参名 → 槽位 ID |
| `stmts` | `Vec<MirStmt>` | 语句序列 |
| `exprs` | `HashMap<u32, MirExpr>` | **表达式旁路表**（ID → 表达式） |
| `ctfe_consts` | `HashMap<u32, i64>` | CTFE 常量旁路表 |
| `type_map` | `HashMap<u32, Type>` | **类型旁路表**（ID → 类型） |
| `global_consts` | `HashMap<String, ConstValue>` | 全局常量 |
| `properties` | `Vec<String>` | 数学性质（commutative/associative/identity） |
| `generic_params` | `Vec<String>` | 泛型参数名 |
| `is_extern` | `bool` | FFI 声明标记 |

**类函数设计**：

```rust
pub fn dump_canonical(&self) -> String    // mir.rs:36  ★ 字节稳定文本（四本 HashMap 按 key 排序）
```

**`MirStmt`（enum，`mir.rs:101`）**：`Assign{..}` / `Call{func,args,dest,type_args}` /
`VoidCall{func,args}` / `Return{..}` / `SemiringFold{op,values,result}` / `ParamInit` /
`Consume` / `If{cond,then,else_}` / `TryProp` / …

**`MirExpr`（enum，`mir.rs:222`）**：`Var(u32)` / `IntLit(i64)` / `FloatLit(f64)` /
`StringLit(String)` / `FString(Vec<u32>)` / `ConstEval(i64)` / `TimingOwned(u32)` /
`Struct{..}` / `FieldAccess{..}` / `Syscall(u32, Vec<u32>)` / `As{..}` / `Range{..}` /
`Deref{..}` / `AddrOf{alloca_id}` / `BinaryOp{op: String, left, right}` / `StackArray{..}` / …

> ⚠ `BinaryOp { op: String }` —— pyramid 3.1 要求 typed op，业界（MLIR）为 typed enum。任务 **W1**。

### 4.2 降级器类设计 —— `MirGen`

**职责**：AST → MIR 的降级主战场（`gen.rs`，**13,636 行**）。

**字段**（`gen.rs:99` 起，节选）：`next_id` / `stmts` / `exprs` / `ctfe_consts` / `type_map` /
`name_to_id` / `global_consts` / `source_types` / `array_lit_lens` / `pointee_widths` /
`type_decls` / `shared_type_decls` / `source_file` / `argparse_kinds` / `param_defaults` /
`pending_closure_param_types` / `py_json_items_ids` / `last_closure_ret_ty` …

**类函数设计**（16 个 pub fn + 关键私有方法）：

```rust
// ── 构造（builder 模式：14 个 with_* 上下文注入）──
pub fn new() -> Self                                                                     // :231
pub fn with_global_consts(..)                                                            // :280
pub fn with_module_global_types(mut self, types: HashMap<String, Type>) -> Self          // :289
pub fn with_func_param_names(mut self, names: HashMap<String, Vec<String>>) -> Self      // :295
pub fn with_func_ret_types(..)                                                           // :300  ★ 返回类型（struct 返回重灾区）
pub fn with_nonlocal_names(mut self, names: HashSet<String>) -> Self                     // :316
pub fn with_module_globals(mut self, names: HashSet<String>) -> Self                     // :325
pub fn with_py_imports(..)                                                               // :331
pub fn with_py_user_modules(mut self, mods: HashSet<String>) -> Self                     // :342
pub fn with_symbol_renames(mut self, renames: HashMap<String, String>) -> Self           // :348  ★ 符号修饰（4.3.d）
pub fn with_type_decls(mut self, decls: HashMap<String, TypeDecl>) -> Self               // :803
pub fn with_current_module(mut self, module: String) -> Self                             // :809
pub fn with_source_file(mut self, path: Option<String>) -> Self                          // :814
pub fn with_argparse_kinds(mut self, kinds: HashMap<String, String>) -> Self             // :820
pub fn with_param_defaults(..)                                                           // :826

// ── 主入口 ──
pub fn lower_to_mir(&mut self, ast: &AstNode) -> Mir                                     // :834  ★
pub fn take_generated_mirs(&mut self) -> Vec<Mir>                                        // :13455 （闭包 MIR 取出）

// ── 私有降级链（调用顺序 = 降级顺序）──
fn lower_ast(&mut self, ast: &AstNode)                                                   // :1146
fn lower_ast_inner(&mut self, ast: &AstNode)                                             // :1157  语句分派
fn lower_expr(&mut self, expr: &AstNode) -> u32                                          // :3031  ★ 表达式分派
fn lower_closure(&mut self, params: &[String], body: &AstNode) -> String                 // :13184 闭包合成
fn lower_loop_else(&mut self, else_body: &[AstNode]) -> Vec<MirStmt>                     // :12969
fn lower_range_guard(..)                                                                 // :2989
fn lower_to_string(&mut self, id: u32) -> u32                                            // :2939  （print 路径）
fn lower_map_key(&mut self, id: u32) -> u32 / lower_map_key_typed(.., map_ty: Option<&Type>)  // :2849 / :2829
fn lower_multi_index_element(&mut self, item: &AstNode) -> u32                           // :12907
```

### 4.3 单态化类设计

| 类 | 职责 | 位置 |
|----|------|------|
| `MonoKey` | 实例化键（`func_name` + `type_args`） | `specialization.rs:7` |
| `MonoValue` | 实例化结果（`llvm_func_name` + `cache_safe`） | `specialization.rs:13` |

```rust
MonoKey::mangle() -> String      // 符号名生成（specialization.rs）
```

### 4.4 调用链条（降级）

```text
Resolver::lower_to_mir(&AstNode) -> Mir                resolver.rs:3000
 │  组装上下文：module_global_types() / func_param_names() / is_nonlocal_name()
 │              type_decls / source_file / argparse_kinds / param_defaults
 └─ MirGen::new() + with_*(×14)                        gen.rs:231–834
 └─ MirGen::lower_to_mir(ast) -> Mir                   gen.rs:834
      ├─ lower_ast(ast)                                gen.rs:1146
      │    └─ lower_ast_inner(ast)                     gen.rs:1157  语句分派
      │         ├─ 赋值/调用/返回 → MirStmt::Assign/Call/Return
      │         ├─ if/for/while   → MirStmt::If + 基本块
      │         └─ 表达式           → lower_expr(expr)  gen.rs:3031
      │              ├─ 字面量      → MirExpr::IntLit/FloatLit/StringLit
      │              ├─ 名称        → MirExpr::Var(id)（id 由 name_to_id 分配）
      │              ├─ 二元运算    → MirExpr::BinaryOp{op: String}  ⚠ 未 typed
      │              ├─ 方法调用    → pylib::method_symbol(handle, method) → MirExpr::Syscall
      │              ├─ 闭包        → lower_closure → 合成独立 Mir（gen.rs:13184）
      │              ├─ 推导式      → 脱糖为 If + StackArray
      │              └─ f-string    → MirExpr::FString(Vec<u32>)
      └─ take_generated_mirs()                          gen.rs:13455（闭包 MIR 取出）
```

### 4.5 中端与 pyramid 差距

| pyramid 要求 | 现状 | 轴 |
|-------------|------|----|
| 类型随 IR 携带，不靠旁路哈希表 | ✗ `type_map` 旁路表 + 14 个 `with_*` 注入 | **B/D** |
| typed op（非字符串） | ✗ `BinaryOp{op: String}` | W1 |
| 每级变换后有 verifier | ✗ 无 MIR verifier | G.6 |
| Lowering 分级、每步可验证 | ✗ AST 直落 MIR | 3.2 |
| 优化有分析支撑 | ⚠ 无独立优化 pass | C |

---

## 5. 层 4：后端层

### 5.1 类设计 —— `LLVMCodegen<'ctx>`

**职责**：MIR → LLVM IR 唯一后端（`codegen.rs`，7,571 行）。

**类函数设计**：

```rust
// ── 公共 API（4 个）──
pub fn new(context: &'ctx Context, name: &str) -> Self                    // :81   构造（建 module）
pub fn gen_mirs(&mut self, mirs: &[Mir])                                  // :1407 ★ 主入口
pub fn report_abi_coercions(&self)                                        // :1342 ★ ABI 折扣账本报告
pub fn type_to_llvm_type(&self, ty: &Type) -> inkwell::types::BasicTypeEnum<'ctx>  // :7374 ★ 布局落点

// ── 内部生成链（节选，按调用顺序）──
fn gen_fn(&mut self, mir: &Mir)                                           // :1535  单函数生成
fn mangle_function_name(&self, base_name: &str, type_args: &[Type]) -> String  // :1363  符号修饰
fn infer_fn_return_type(&self, mir: &Mir) -> BasicTypeEnum<'ctx>          // :1392  ★ 返回类型推断
fn void_decl_shadow(f: FunctionValue<'ctx>) -> bool                       // :1388
fn collect_slot_reads(..)                                                 // :1633
fn slot_or_sitofp(..)                                                     // :1702  ★ 槽位取值 + 浮点转换
fn collect_all_local_ids(&self, mir: &Mir) -> HashSet<u32>                // :1719
fn collect_ids_from_stmt_safe(..) / collect_ids_from_expr_safe(..)        // :1733 / :1920
fn is_operator(&self, name: &str) -> bool                                 // :2089
fn is_simd_operation(&self, name: &str) -> bool                           // :2151
fn simd_vector_type / simd_alloca_vec / simd_load_vec / simd_store_vec     // :2178–:2224
// 整数/浮点除法语义（Python 地板除 vs C 截断除）
fn build_floordiv_int(..) / build_floormod_int(..) / build_floordiv_float(..)  // :2009/:2042/:2074
```

### 5.2 模块级函数设计

```rust
// jit.rs —— 收尾与链接
pub fn finalize_and_aot<'ctx>(codegen: &LLVMCodegen<'ctx>, path: &Path, target_str: &str)
    -> Result<(), Box<dyn Error>>                                         // :256  ★
    // 内部：codegen.module.verify() → 目标写盘 → clang 链接
pub fn register_jit_mappings<'ctx>(module: &Module<'ctx>, ee: &ExecutionEngine<'ctx>)  // jit_mappings_gen.rs:158

// runtime_decls_*.rs —— @generated，由 registry 生成
pub fn declare_registry_runtime_fns<'ctx>(..)                             // runtime_decls_registry.rs:10
pub fn declare_core_runtime_fns<'ctx>(..)                                 // runtime_decls_core.rs:10

// monomorphize.rs —— 类型代换（后端侧的单态化）
pub fn create_substitution(type_vars: &[TypeVar], type_args: &[Type]) -> Substitution  // :17
pub fn substitute_type(ty: &Type, substitution: &Substitution) -> Type                 // :37
pub fn substitute_expr(expr: &MirExpr, s: &Substitution) -> MirExpr                    // :49
pub fn substitute_stmt(stmt: &MirStmt, s: &Substitution) -> MirStmt                    // :63
pub fn substitute_mir(mir: &Mir, substitution: &Substitution) -> Mir                   // :247
pub fn extract_type_vars(mir: &Mir) -> Vec<TypeVar>                                    // :279
```

### 5.3 调用链条

```text
LLVMCodegen::new(&context, "module")            codegen.rs:81
 └─ declare_registry_runtime_fns / declare_core_runtime_fns   （@generated extern 声明）
 └─ gen_mirs(&[Mir])                            codegen.rs:1407
      ├─ 确定性顺序（main.rs:782 已排序，防 .N 别名翻转）
      └─ 逐函数 gen_fn(mir)                     codegen.rs:1535
           ├─ infer_fn_return_type(mir)          :1392
           ├─ mangle_function_name(base, type_args)  :1363
           ├─ 语句：MirStmt → LLVM 指令
           │    └─ 槽位读取 slot_or_sitofp(..)   :1702（含 int→float 提升）
           └─ 表达式：MirExpr → LLVM IR
                ├─ Syscall → 按 PyMember.symbol 调用运行期函数
                ├─ FieldAccess → resolve_struct_field_index（⚠ 有 GLOBAL 字段名扫描兜底）
                └─ BinaryOp → 运算符分派（is_operator / simd_* / floordiv_*）
 └─ report_abi_coercions()                     codegen.rs:1342（--strict-abi 时 fail）
 └─ finalize_and_aot(&codegen, path, target)    jit.rs:256
      ├─ module.verify()                        （唯一的内部验证）
      ├─ 目标写盘（ZETA_NO_OPT=1 → -O0）
      └─ clang 链接 → 可执行
```

### 5.4 后端与 pyramid 差距

| pyramid 要求 | 现状 | 轴 |
|-------------|------|----|
| 4.3.a 调用点与定义同一份声明 | ⚠ registry extern 满足；用户函数/struct 返回靠 `with_func_ret_types` | G.5b |
| 4.3.b 值表示表穷尽、写/读对称 | ✗ 无表；`report_abi_coercions` 只**报告**（实测 133 处非白名单转换） | **B** |
| 4.3.c 布局假设升格为合同 | ⚠ `resolve_struct_field_index` 全局字段名扫描兜底 | G.5c |
| 4.3.d 单一名字修饰函数 | ⚠ `symbol_renames` 注入 + `mangle_function_name` + `aliases.inc.c` 三套并存 | G.5c |
| 4.3.e 合同治理 | ✗ 无 ABI 文档（`check_abi_anchors.py` 只做锚点校验） | G.5d |
| 全优化级别正确 | ✗ -O3 误编译（IR 对、汇编缺实参） | **G.1** |

---

## 6. 层 5：运行时层

### 6.1 类设计

| 类/模块 | 职责 | 位置 |
|--------|------|------|
| `DynamicArray` | 动态数组运行期表示 `[cap\|len\|data]` | `src/runtime/host.rs:682` |
| `host.rs` | Rust 侧宿主（I/O、内建、向量） | `src/runtime/host.rs`（942 行） |
| `runtime/tokio_runtime_stub.c` | **原语层**：vec / map / str / GC 交互 | 3,642 行 |
| `runtime/py_additions.c` | **PY-A shim**：pandas / numpy / datetime / json | 3,562 行 |
| `runtime/parquet_min.c` | 自包含 parquet（thrift/snappy/RLE/时间戳归一化） | 780 行 |
| `runtime/unavailable_stubs.c` | **响亮失败桩**（weak，未实现即 abort） | 232 行 |
| `runtime/aliases.inc.c` | LLVM `.N` 重命名别名表 | 70 行 |
| `zeta_src/*.z` | 自举源（51 文件 / 7,288 行） | `zeta_src/` |

### 6.2 类函数设计（C 原语族，签名即 ABI 契约）

```c
// ── 字符串族（str_* 为原语，host_str_* 为 MIR 直连入口）──
int64_t str_len(int64_t s)                                    // tokio_runtime_stub.c:44
int64_t str_concat(int64_t a, int64_t b)                      // :45
int64_t str_to_uppercase(int64_t s) / str_to_lowercase        // :52 / :59
int64_t str_trim(int64_t s)                                   // :66
int64_t str_starts_with(int64_t hay, int64_t needle)          // :76
int64_t str_ends_with(int64_t hay, int64_t needle)            // :80
int64_t str_contains(int64_t hay, int64_t needle)             // :86
int64_t str_replace(int64_t s, int64_t old_s, int64_t new_s)  // :90
// host_* 包装（MIR 侧调用名）
int64_t host_str_len(int64_t s)                               // :110
int64_t host_str_concat(int64_t a, int64_t b)                 // :130  ★ 含可读性守卫（批次 292 改为降级+告警）
int64_t host_str_to_uppercase / to_lowercase / trim           // :160–:162
int64_t host_str_starts_with / ends_with / contains / replace  // :163–:166

// ── 向量族 ──
int64_t vec_push(int64_t data_ptr, int64_t val)               // :30   ★ 扩容会返回新指针（忽略即堆破坏）

// ── 打印族 ──
void print_str(int64_t v) / println_str(int64_t v)            // :39 / :40
void print_i64 / println_i64 / print_f64 / println_f64        // 数值族
void print_bool(int64_t v)                                    // :38   ★ Python repr（True/False）

// ── map 族（含守卫）──
int zt_map_is_json_handle(int64_t h)                          // 首字 1..8 判 Json 句柄（防自旋）
int64_t map_new(void) / map_hash(int64_t key)
int64_t map_get_default(...) / zt_safe_str_key(...)
```

### 6.3 调用链条（生成代码 ↔ 运行时）

```text
zeta 源码 `s.upper()`
 └─ 前端：pylib::method_symbol("PyStr", "upper") → symbol 名
 └─ MIR：MirExpr::Syscall(id)  ← symbol 来自 PyMember.symbol
 └─ codegen：按 PyMember.args/ret 声明 extern 并调用
 └─ 链接期符号解析：
      ├─ runtime/tokio_runtime_stub.c  （原语：str_upper …）
      ├─ runtime/py_additions.c        （PY-A：py_df_* / py_vec_* / py_dt_*）
      └─ runtime/unavailable_stubs.c   （缺失 → weak stub → 响亮 abort）
```

### 6.4 运行时与 pyramid 差距

| pyramid 要求 | 现状 | 轴 |
|-------------|------|----|
| 5.1 每个值自带类型标签 | ✗ **i64 槽是万能源** | **B（核心）** |
| 5.2 值模型定后再选 GC | ⚠ Boehm 保守 GC（权宜）；保守 GC 与指针/整数混槽互斥 | B/T5 |
| 5.3 数据驱动方法表 | ⚠ registry 是数据驱动（对），仍有名字约定兜底 | B/T4 → W6 |
| 5.4 API 声明与实现分离 + 契约测试 | ⚠ 分离了，**无契约测试** | E.3 / G.5e |

**判形启发式族**（轴 B.1 要退役的债务）：`zt_map_is_json_handle`（首字 1..8）、
`zt_c_str_readable`（地址区间 + `vm_read_overwrite` 探页）、`GC_base` 句柄验证、
`StackArray` 逃逸检测、小字符串打包槽（`"sz.15998" = 0x38393935312e7a73`）——
每一个都是「值不带类型」逼出来的内存几何猜测。

---

## 7. 层 6：正确性层

### 7.1 类设计

| 类/工具 | 职责 | 位置 |
|--------|------|------|
| `module.verify()` | **唯一的内部验证**（LLVM 结构校验） | `jit.rs:261` 调用 |
| `Mir::dump_canonical` | IR 快照（字节稳定 golden） | `mir.rs:36` |
| `IdentityVerificationPass` | 身份类型验证 pass | `passes/identity_verification.rs:9` |
| `OptLevel` | 优化级别矩阵维度 | `optimization.rs:9` |
| `run_all.sh` | 三基线门禁（JSON 输出） | `tools/` |
| `diff_test.py` | CPython 差分（oracle） | `tools/` |
| `mir_diff.sh` | MIR 字节 diff | `tools/` |
| `asan_run.sh` | ASan/MSan/UBSan 常态化 | `tools/` |
| `opt_matrix.sh` | 全优化级别矩阵 | `tools/` |
| `check_abi_anchors.py` | ABI 锚点校验 | `tools/` |
| `parse_bisect.py` / `truncation_inventory.sh` | 最小复现 / 截断清单 | `tools/` |

### 7.2 调用链条（验证金字塔）

```text
pass 级      mir_diff.sh            同一输入两次编译，MIR 字节 diff
IR golden    --dump-mir → Mir::dump_canonical()       mir.rs:36
端到端       tests/python_style/run.sh   编译 + 运行 + 逐行比对 expect
差分         diff_test.py               zeta vs CPython 可观察行为
全级别       opt_matrix.sh              -O0..-O3 矩阵
未定义行为   asan_run.sh                ASan/MSan/UBSan
```

### 7.3 与 pyramid 差距

| pyramid 要求 | 现状 | 轴 |
|-------------|------|----|
| 每级 IR verifier，pass 后强制跑 | ✗ 只有末端 LLVM `module.verify()` | G.6 |
| 端到端断言**输出值** | ⚠ python_style 断言了；**官方 194 是 compile-only** | E.1 |
| 差分测试为核心指标 | ⚠ 有工具，未进门禁 | G.3 |
| 全优化级别门禁 | ✗ 全程 `ZETA_NO_OPT=1` 绕过 -O3 | **G.1** |
| 消灭「降级 + 警告」 | ✗ W0001（CTFE）/ W0002（宏）/ W0003（类型）三处降级继续 | G.4 |

---

## 8. 层 7：工程层

### 8.1 类设计

| 类/文件 | 职责 | 位置 |
|--------|------|------|
| `main()` | 驱动 + CLI 开关（1,200 行） | `src/main.rs:502` |
| `lib.rs` | crate 根，17 个 pub mod | `src/lib.rs:51` |
| `ErrorCodeRegistry` | 错误码（2,178 行） | `src/error_codes.rs` |
| `diagnostics.rs` | 结构化诊断（W2xxx 段目标） | `src/diagnostics.rs` |
| `src/lsp/` | LSP 服务（719 行，**未接线**） | `src/lsp/` |
| `gen_from_registry.py` | **代码生成器**（registry → 3 个 @generated） | `tools/` |
| `build_runtime.sh` | runtime .o 构建（`--gen` 重生成声明） | `tools/` |

### 8.2 调用链条（构建与生成）

```text
pylib/registry.txt
 └─ python3 tools/gen_from_registry.py --emit --emit-core --emit-jit --emit-aliases
      ├─→ codegen/runtime_decls_registry.rs   （extern 声明）
      ├─→ codegen/runtime_decls_core.rs       （核心声明）
      ├─→ codegen/jit_mappings_gen.rs         （JIT 映射）
      └─→ runtime/aliases.inc.c               （.N 别名表）
 └─ bash tools/build_runtime.sh
      ├─ clang -c runtime/tokio_runtime_stub.c → tokio_runtime.o
      ├─ clang -c runtime/py_additions.c       → zeta_runtime_c.o
      └─ clang -c runtime/unavailable_stubs.c  → zt_unavail.o
 └─ cargo build --release → target/release/zetac
```

### 8.3 与 pyramid 差距

| pyramid 要求 | 现状 | 轴 |
|-------------|------|----|
| 每阶段一模块、依赖单向 | ⚠ 5 处 frontend→middle；resolver 双轨 6,865 行 | A/D |
| 信息只有一个所有者 | ✗ 类型信息散在三处 | **F** |
| 名字 intern 化 | ✗ 未做 | C |
| 结构化诊断替代裸打印 | ✗ 63 处裸 `eprintln!` | E.4 |

---

## 9. 类内调用链（self-call chain）—— 每个类自身的调用流程

> 上一节是**跨层**链条（类 → 类）；本节是**类内**链条（方法 → 方法）。
> 数据由脚本扫描各类 `self.METHOD(` 调用边自动提取（仅统计同一类内的方法名），
> 渲染规则：`[n]` = 该方法的类内扇出；`⟲递归` = 回到调用路径上的祖先；
> `↑已展开` = 该子树已在本文档前文出现过（避免重复展开）。

### 9.1 全局类内调用统计

| 类 | 文件 | 方法数 | 有类内扇出的方法 | 扇出冠军 | 递归环 |
|----|------|-------|----------------|---------|--------|
| `Resolver` | `resolver.rs` | 37 | 7 | `register` → 5 | `register ⟲ load_user_python_module` |
| `MirGen` | `gen.rs` | 52 | 17 | **`lower_expr` → 22** | `lower_ast ⟲ lower_ast_inner ⟲ lower_expr` |
| `LLVMCodegen` | `codegen.rs` | 52 | 16 | `gen_stmt` → 14 | `gen_fn ⟲ monomorphize_function`、`gen_expr_safe ⟲ gen_expr` |
| `ConstEvaluator` | `ctfe/evaluator.rs` | 26 | 13 | `eval_const_expr` → 12 | `eval_const_expr ⟲ eval_*`（12 处回边） |
| `Type`/`Substitution`/`GenericContext` | `types/mod.rs` | 50 | — | 无类内扇出（纯函数式，走自由函数） |

**三个设计观察**：

1. **每个类都是「单一入口 + 深扇出」的树形**——`MirGen` 唯一入口 `lower_to_mir`（入度 0），
   `LLVMCodegen` 唯一入口 `gen_mirs`，`ConstEvaluator` 唯一入口 `evaluate_program`。
   这是 pyramid 7.1「每阶段一个模块」在**类粒度**上的体现。
2. **扇出高度集中**：`lower_expr`(22) / `gen_stmt`(14) / `eval_const_expr`(12) 三个方法
   吃掉了各类几乎全部子方法——它们就是 pyramid 2.5「禁止逐例特判」失效的地方
   （一个方法里塞 22 条特判分支）。
3. **递归都是语义必需**：`lower_ast ⟲`（嵌套语句）、`gen_expr_safe ⟲ gen_expr`
   （嵌套表达式）、`eval_const_expr ⟲ eval_*`（常量折叠递归）。无意外环。

### 9.2 `Resolver` 类内链

```text
lower_to_mir()                          ← 公共入口（组装 MirGen 上下文）
├─ func_param_names                      → 注入 with_func_param_names
├─ module_global_types                   → 注入 with_module_global_types
└─ module_renames_for                    → 注入 with_symbol_renames
```

```text
register()                              ← 公共入口（符号表填充）
├─ ast_to_const_value
├─ find_py_module_file
├─ load_user_python_module   [2]
│  ├─ find_py_module_file
│  └─ register  ⟲递归                    ← 模块导入递归注册（import 链）
├─ resolve_py_module_spec
└─ resolve_reexport_target
```

```text
expand_macros()                         ← 公共入口
└─ expand_macros_in_node                 （递归在函数内部，非方法调用）
```

**注意**：`lower_to_mir` 的类内扇出只有 3，且全部是**上下文查询**——
它本身不做降级，只组装 `MirGen` 的 14 个 `with_*`。真正的降级全在 `MirGen` 里。
这是 pyramid 7.1「信息唯一所有者」的反面：**Resolver 拥有信息，MirGen 消费信息，
中间靠 21 条注入缝合**。

### 9.3 `MirGen` 类内链（降级主链）

```text
lower_to_mir()                          ← 唯一入口（gen.rs:834）
├─ lower_ast   [1]                        gen.rs:1146
│  └─ lower_ast_inner   [9]               gen.rs:1157  语句分派
│     ├─ annotation_named_ty
│     ├─ lower_ast  ⟲递归                  ← 嵌套语句
│     ├─ lower_closure                    gen.rs:13184  闭包合成独立 Mir
│     ├─ lower_expr   [22]                gen.rs:3031  ★ 表达式分派（扇出冠军）
│     │  ├─ array_elem_is_str
│     │  ├─ enum_unit_variant_index
│     │  ├─ get_common_element_type
│     │  ├─ global_ty_of
│     │  ├─ i64_zero_id   [1]
│     │  ├─ is_array_like
│     │  ├─ lower_ast  ⟲递归
│     │  ├─ lower_closure
│     │  ├─ lower_map_key   [1]
│     │  ├─ lower_map_key_typed   [2]
│     │  ├─ lower_multi_index_element   [2]
│     │  ├─ lower_range_guard   [2]
│     │  ├─ lower_to_string   [1]
│     │  ├─ materialize_for_call   [1]
│     │  ├─ next_id                       ← 槽位分配（叶子）
│     │  ├─ next_id_with_lit   [1]
│     │  ├─ py_handle_of   [1]
│     │  ├─ py_member_call   [1]
│     │  ├─ py_member_target   [1]
│     │  ├─ py_struct_has_field
│     │  ├─ py_struct_type_of   [1]
│     │  └─ qualified_method_candidate
│     ├─ lower_loop_else   [1]
│     │  └─ lower_ast  ⟲递归
│     ├─ lower_map_key_typed   [2]
│     ├─ next_id
│     ├─ next_id_with_lit   [1]
│     └─ qualified_method_candidate
├─ next_id
└─ next_id_with_lit  ↑已展开
```

**读法**：主链 `lower_to_mir → lower_ast → lower_ast_inner → lower_expr` 四跳；
`lower_expr` 之下 22 个分支中，**11 个是 `py_*` / `qualified_method_candidate` 类特判**
（PY-A 兼容层），只有 `next_id` / `lower_closure` / `lower_map_key` 等是通用机制。
这正是轴 D.1「gen.rs 拆分」要切的刀口。

### 9.4 `LLVMCodegen` 类内链（生成主链）

```text
gen_mirs()                              ← 唯一入口（codegen.rs:1407）
├─ gen_fn   [5]                          codegen.rs:1535
│  ├─ collect_all_local_ids   [1]
│  │  └─ collect_ids_from_stmt_safe   [1]
│  │     └─ collect_ids_from_expr_safe
│  ├─ ensure_emittable_block
│  ├─ gen_stmt   [14]                    ★ 语句分派
│  │  ├─ build_floordiv_float / build_floordiv_int / build_floormod_int
│  │  ├─ coerce_call_args   [1]
│  │  │  └─ abi_note                      ← ABI 折扣记账点
│  │  ├─ cond_i1_from   [1]
│  │  │  └─ container_cond_i1
│  │  ├─ gen_expr_safe   [1]
│  │  │  └─ gen_expr   [9]                ★ 表达式分派
│  │  ├─ get_function
│  │  ├─ get_or_declare_function   [2]
│  │  │  ├─ mangle_function_name          ← 符号修饰点 ①
│  │  │  └─ monomorphize_function   [4]
│  │  ├─ handle_simd_operation   [9]
│  │  │  ├─ gen_expr_safe  ↑已展开
│  │  │  ├─ handle_simd_const
│  │  │  ├─ simd_alloca_vec / simd_load_vec / simd_store_vec
│  │  │  ├─ simd_binop   [2]
│  │  │  ├─ simd_trunc_val / simd_vector_type / simd_zext_i64
│  │  ├─ is_operator / is_simd_operation
│  │  ├─ load_local
│  │  ├─ note_return_slot_mismatch
│  │  └─ slot_or_sitofp                   ← 槽位取值 + int→float 提升
│  ├─ get_function
│  └─ infer_fn_return_type
├─ infer_fn_return_type
├─ is_generic_function
└─ monomorphize_function   [4]
   ├─ gen_fn  ↑已展开                     ⟲递归（实例化时递归生成）
   ├─ infer_fn_return_type
   ├─ mangle_function_name
   └─ substitute_mir   [1]
      └─ substitute_stmt
```

**读法**：主链 `gen_mirs → gen_fn → gen_stmt → gen_expr` 四跳，
与 `MirGen` 的降级链**结构同形**（都是「入口 → 语句 → 表达式」三段）。
`monomorphize_function ⟲ gen_fn` 是唯一的结构性递归——泛型实例化时递归生成函数体。

### 9.5 `ConstEvaluator` 类内链（CTFE）

```text
evaluate_program()                      ← 公共入口（evaluator.rs:52）
└─ transform_ast_node   [2]              :78   两遍：先注册，再变换
   ├─ eval_const_expr   [12]             :580  ★ 常量求值分派
   │  ├─ eval_array_literal   [1]        :974
   │  ├─ eval_assignment   [1]           :1091
   │  ├─ eval_binary_op   [1]            :728
   │  ├─ eval_block   [1]                :1043
   │  ├─ eval_for_loop   [1]             :1198
   │  ├─ eval_function_call   [4]        :773
   │  ├─ eval_if_expr_with_else   [2]    :993
   │  ├─ eval_let_binding   [1]          :1064
   │  ├─ eval_loop                       :1188
   │  ├─ eval_unary_op   [1]             :747
   │  ├─ eval_variable                   :758
   │  └─ eval_while_loop   [1]           :1138
   └─ transform_expr   [3]               :244
      ├─ eval_binary_op   [1]
      ├─ eval_function_call   [4]
      └─ eval_unary_op   [1]
```

**递归形态**：`eval_const_expr` 有 **12 条回边**——每个 `eval_*` 子方法内部
再调 `self.eval_const_expr(` 递归下降。这是全仓最纯粹的「递归下降求值器」形态。

### 9.6 `Type` / `Substitution` / `GenericContext` 类内链

**无类内扇出**——这三个类的全部方法（50 个）都是**纯函数式**：
不调 `self.OTHER()`，只对入参计算并返回。

```text
Type::from_string(s) -> Type            types/mod.rs:283
Type::mangled_name(&self) -> String     :854      ← 只读字段，无内部调用
Type::instantiate_generic(&self, args) -> Result<Type, String>   :1006
Substitution::apply(&self, ty) -> Type  :1319     ← 递归在自由函数里，非方法调用
Substitution::unify(&mut self, t1, t2) -> Result<(), UnifyError>  :1491
```

**这是本仓最健康的一个类族**：pyramid 5「纯函数、显式契约」的正面样本，
对比 `MirGen`（52 方法、17 个互相调用、22 扇出）——同一仓内两种设计范式的极端。

---

## 10. 调用流程链条（跨类 · 方法级全景）

> §0 是**链路图**，§9 是**类内**链表，本节是**跨类方法级**的逐步链条：
> 每一步标出「谁调谁」的具体函数名与定义处。
> `[n]` = 该调用点在同文件内的出现次数（源码 grep 实测）。

### 10.1 完整链条（55 步）

```text
[01] main()                                          main.rs:502
[02]  └─ scheduler::init_runtime()                   main.rs:503
[03]  └─ fs::read_to_string(file)                    main.rs:626   ×2
[04]  └─ parse_zeta(code) → Vec<AstNode>             main.rs:629   ×4  → top_level.rs:1590
[05]      ├─ parser::skip_ws_and_comments / ws       parser.rs:39/:65
[06]      ├─ parser::parse_ident / parse_member_ident   parser.rs:74/:147
[07]      ├─ top_level 组合子 → FuncDef/ConceptDef/ImplBlock/ExternFunc
[08]      ├─ stmt 组合子（if/for/while/try/match）
[09]      └─ parse_expr → parse_primary → parse_lit   expr.rs:3180/:1531/:145
[10]  └─ ensure_fully_parsed(remaining, code, file)  main.rs:391
[11]  └─ const_eval::evaluate_constants(&asts)       main.rs:634   ×2
[12]  └─ Resolver::new()                             main.rs:665   ×4
[13]  └─ resolver.set_source_dir(&file)              main.rs:667
[14]  └─ resolver.expand_macros(&asts)               main.rs:670   ×2
[15]      └─ macro_expand::parse_macro_rules / process_attributes   ×1/:2
[16]  └─ resolver.register(ast) × N                  main.rs:679   ×4
[17]      ├─ pylib::find_module(..)                  resolver.rs    ×4
[18]      ├─ pylib::find_member(..)                  resolver.rs    ×4
[19]      ├─ load_user_python_module ⟲ register        resolver.rs    （import 链递归）
[20]      └─ top_level::set_parsing_imported_module  resolver.rs    ×3
[21]  └─ resolver.infer_untyped_returns(&asts)        main.rs:683
[22]  └─ resolver.typecheck(&asts) → bool             main.rs:696   ×4（失败仅 W0003）
[23]  └─ resolver.get_registered_funcs()              main.rs:707   ×4
[24]  └─ resolver.lower_to_mir(ast) × N → Mir         main.rs:712   ×5  → resolver.rs:3000
[25]      └─ MirGen::new().with_*(×14)                resolver.rs:3116
[26]      └─ MirGen::lower_to_mir(ast)                gen.rs:834
[27]          └─ lower_ast → lower_ast_inner          gen.rs:1146 / :1157
[28]              └─ lower_expr                       gen.rs:3031   [22 扇出]
[29]                  ├─ pylib::find_member          gen.rs         ×9
[30]                  ├─ pylib::method_symbol         gen.rs         ×8
[31]                  ├─ pylib::method_ret / handle_tag  gen.rs       ×5 / ×5
[32]                  ├─ ctfe::eval_const_expr        gen.rs         ×1
[33]                  └─ lower_closure ⟲ lower_ast     gen.rs:13184
[34]  └─ resolver.take_generated_closures()           main.rs:721
[35]  └─ resolver.collect_used_specializations(&asts) main.rs:725   ×2
[36]  └─ resolver.monomorphize(key, ast) → AstNode    main.rs:760   ×2 → resolver.rs:3298
[37]  └─ resolver.lower_to_mir(mono_ast)              main.rs:761   （实例化体降级）
[38]  └─ resolver.record_mono(key, mir)               main.rs:762
[39]  └─ all_mirs.sort_by(name)                        main.rs:782
[40]  └─ Mir::dump_canonical()  [--dump-mir]          main.rs:795
[41]  └─ LLVMCodegen::new(&context, "module")        main.rs:808   ×4
[42]      └─ pylib::all_externs() → 声明 extern        codegen.rs:988
[43]  └─ codegen.gen_mirs(&all_mirs)                  main.rs:812   ×4 → codegen.rs:1407
[44]      └─ gen_fn                                     codegen.rs:1535   [5 扇出]
[45]          ├─ collect_all_local_ids → collect_ids_from_stmt_safe → …_expr_safe
[46]          ├─ gen_stmt                              codegen.rs        [14 扇出]
[47]          │   ├─ slot_or_sitofp                     槽位取值 + int→float 提升
[48]          │   ├─ gen_expr_safe → gen_expr ⟲           [9 扇出]
[49]          │   ├─ get_or_declare_function → mangle_function_name
[50]          │   │   └─ monomorphize_function ⟲ gen_fn   泛型实例化递归
[51]          │   ├─ coerce_call_args → abi_note        ★ ABI 折扣记账
[52]          │   └─ handle_simd_operation              [9 扇出，SIMD 家族]
[53]          └─ infer_fn_return_type
[54]  └─ codegen.report_abi_coercions()                 main.rs:813
[55]  └─ finalize_and_aot(&codegen, path, target)       main.rs:823  ×2 → jit.rs:256
         ├─ codegen.module.verify()                    jit.rs:261
         ├─ 目标写盘（ZETA_NO_OPT=1 → -O0）
         └─ clang 链接 → 可执行

（运行期） 生成代码 → runtime/tokio_runtime_stub.c（原语）
                    → runtime/py_additions.c（PY-A shim）
                    → runtime/unavailable_stubs.c（缺失→响亮 abort）
```

### 10.2 跨类调用边（源码 grep 实测）

| 调用方（层） | 被调符号 | 次数 | 用途 |
|-------------|---------|------|------|
| main（驱动） | `parse_zeta` | 4 | 解析入口 |
| main | `Resolver::new` / `LLVMCodegen::new` | 4 / 4 | 构造两大核心类 |
| main | `const_eval::evaluate_constants` | 2 | CTFE |
| main | `finalize_and_aot` | 2 | 收尾与链接 |
| main | `ErrorCodeRegistry::new` | 2 | `--explain` |
| main | `pylib::all_stub_symbols` / `stub_symbols` / `pylib_file_stubs` | 1 / 1 / 1 | `--list-stubs` |
| main | `indent::original_line_at` / `remaining_byte_offset` | 1 / 1 | 解析截断报错定位 |
| Resolver（前端） | `pylib::find_module` | 4 | 模块查询 |
| Resolver | `pylib::find_member` | 4 | 成员查询 |
| Resolver | `pylib::handle_tag` | 2 | 句柄→tag |
| Resolver | `pylib::handle_op` / `method_symbol` / `is_noop_module` | 1 / 1 / 1 | 运算符/方法/noop 模块 |
| Resolver | `top_level::set_parsing_imported_module` | 3 | 解析态标记（import 语义） |
| Resolver | `macro_expand::process_attributes` / `parse_macro_rules` | 2 / 1 | 宏展开 |
| Resolver | `MirGen::new()` | 1 | ★ **唯一的 Resolver→MirGen 接口**（`resolver.rs:3116`） |
| MirGen（中端） | `pylib::find_member` | **9** | ★ 方法/成员分派（最高） |
| MirGen | `pylib::method_symbol` | **8** | ★ 句柄方法→符号 |
| MirGen | `pylib::method_ret` / `handle_tag` | 5 / 5 | 返回类型 / tag |
| MirGen | `pylib::handle_op` / `find_module` / `method_by_unique_name` | 1 / 1 / 1 | 运算符/模块/唯一名 |
| MirGen | `ctfe::eval_const_expr` | 1 | 编译期常量折叠 |
| MirGen | `MirStmt::* / MirExpr::*`（构造） | **870** | MIR 节点构造（数据构造，非调用） |
| LLVMCodegen（后端） | `pylib::all_externs()` | 1 | ★ **唯一的 registry→codegen 声明接口**（`codegen.rs:988`） |
| LLVMCodegen | `inkwell::*`（LLVM 绑定） | 31+11+10+… | LLVM API 调用 |

### 10.3 层间调用契约（哪一层的哪个函数调哪一层）

| 边界 | 调用方函数 | 被调方函数 | 位置 |
|------|-----------|-----------|------|
| 前端 → 中端 | `Resolver::lower_to_mir` | `MirGen::new + with_* + lower_to_mir` | `resolver.rs:3116` / `:3000` |
| 中端 → 规范层 | `MirGen::lower_expr` | `pylib::find_member` / `method_symbol` / `method_ret` / `handle_tag` | `gen.rs`（共 28 处） |
| 中端 → 中端(CTFE) | `MirGen::lower_expr` | `ctfe::eval_const_expr` | `gen.rs` |
| 后端 → 规范层 | `LLVMCodegen::new` | `pylib::all_externs` | `codegen.rs:988` |
| 后端 → 中端 | `LLVMCodegen::gen_stmt` | `substitute_mir` / `extract_type_vars` | `codegen.rs:3021/:3070` |
| 后端 → 运行期 | `gen_expr`（Syscall） | `runtime/*.c` 的 `host_str_*` / `py_df_*` 等 | 链接期解析 |
| 中端 → 前端 | `Resolver::lower_to_mir` 内部查 `func_param_names` 等 | 同层调用，非跨层 | `resolver.rs` |

### 10.4 ⚠ 链条上的断链：@generated 声明文件是死代码

pyramid 设计的链条本应是：

```text
registry.txt → gen_from_registry.py → runtime_decls_registry.rs
            → declare_registry_runtime_fns() → codegen 声明 extern
```

**实测**：`declare_registry_runtime_fns`（`runtime_decls_registry.rs:10`， 35.1 KB）与
`declare_core_runtime_fns`（`runtime_decls_core.rs:10`， 8.1 KB）**全仓无任何调用点**
（只剩 `pylib.rs:685` 的一句文档注释提及）。

实际生效的声明路径是**另一条**：

```text
registry.txt → pylib::all_externs()   （codegen.rs:988）  → 逐条声明 extern
```

即：生成器变成了脱离主链的孤儿。这是 refactor.md 轴 A「死代码减法」与轴 D
「双轨决断」的又一个实例（约 43 KB 生成物 + `gen_from_registry.py --emit/--emit-core`
两个开关的维护成本）。

---

## 11. 类依赖与职责合理性评估

> 评估基于 §9-10 的实测调用图 + 本节的规模/引用量统计。
> 标准不凭偏好，每条判定附数据；结论分「合理 / 存疑 / 不合理」三档。

### 11.1 评估标准（6 条）

| # | 标准 | 含义 | 反模式信号 |
|---|------|------|-----------|
| C1 | **单一职责** | 一个类能用一句话描述 | 需要「以及」「或者」连接 |
| C2 | **依赖单向** | 层间依赖无环、方向一致 | 双向 import、越层引用 |
| C3 | **信息唯一所有者** | 每条信息只有一个类拥有 | 旁路表 + 多参数注入 |
| C4 | **依赖强度合理** | 依赖数据/接口，非实现细节 | 高强 × 远距 × 高波动 |
| C5 | **无投机抽象** | 代码被调用而非备着 | 定义了但零引用 |
| C6 | **可测** | 能脱离 I/O 单测 | 方法内直接做 I/O |

### 11.2 类规模与职责（量化）

| 类 | 行数 | 方法 | 字段行 | 类内扇出 | 一句话职责？ |
|----|------|------|--------|---------|-------------|
| `MirGen` | **13,636** | 52 | 47 | 17（冠军 `lower_expr` 22） | ✗ 「把 AST 降为 MIR **以及**处理 11 种 PY-A 特判」 |
| `LLVMCodegen` | **7,602** | 52 | 22 | 16（冠军 `gen_stmt` 14） | ✗ 「生成 LLVM IR **以及**处理 SIMD 家族」 |
| `Resolver` | 4,705 | 37 | 34（**19 个 RefCell**） | 7 | ✗ 「符号表 **以及**类型表 **以及**模块别名 **以及**宏 **以及**借用检查」 |
| `Type` + 10 个伴生类 | 2,366 | 54 | 6 | **0** | ✓ 「类型表示与运算」 |
| `ConstEvaluator` | 1,328 | 26 | 3 | 13（12 条回边） | ✓ 「递归下降常量求值」 |
| `indent`（自由函数） | 1,016 | 16 | 3 | — | ✓ 「缩进→显式块」 |
| `pylib`（数据+自由函数） | 845 | 8 | 35 | — | ✓ 「registry 的内存表示与查询」 |
| `Mir/MirStmt/MirExpr` | 288 | 1 | 10 | — | ✓ 「MIR 数据表示」 |

**判定：三个 God class（`MirGen` / `LLVMCodegen` / `Resolver`）违反 C1。**
合计 25,943 行，占全仓 Rust 的 **28.9%**。

### 11.3 依赖关系评估

| 依赖边 | 调用点 | 强度 | 距离 | 判定 |
|--------|-------|------|------|------|
| `Resolver → MirGen` | **1**（`resolver.rs:3116`） | 弱（只构造 + 注入） | 同层跨界 | ⚠ **存疑**：只 1 个调用点却传 14 个 `with_*` |
| `MirGen → pylib` | **30**（`find_member` 9 / `method_symbol` 8 / `method_ret` 5 / `handle_tag` 5 …） | 弱（查表） | 跨层（中→规范） | ✓ **合理**：数据驱动查表，无逆向依赖 |
| `Resolver → pylib` | 13（`find_module` 4 / `find_member` 4 / `handle_tag` 2 …） | 弱 | 跨层 | ✓ 合理 |
| `LLVMCodegen → pylib` | **1**（`all_externs`，`codegen.rs:988`） | 弱 | 跨层 | ✓ 合理（单一入口） |
| `frontend → middle` | 5 个文件 | 中 | 越层 | ✗ **不合理**：阻塞「先自举 frontend」（轴 D.2） |
| `backend → frontend` | **0** | — | — | ✓ 方向正常 |
| `Resolver → MirGen` 的 14 个 `with_*` | — | **强**（逐字段注入） | 同层 | ✗ **不合理**：C3 违反（信息没唯一所有者） |

**关键判断**：跨层调用本身**健康**（全部是「上→下、查表式、弱耦合」）；
真正的病在**同层内的信息注入**——`Resolver` 拥有 14 类信息，`MirGen` 靠 14 个
`with_*` 逐个接收。这是 C3 的硬伤（pyramid 7.1 的反面）。

### 11.4 死代码与多轨（C5 违反）

实测：以下类型/函数**在生产代码中零引用**（仅自身定义 + 自带测试）：

| 符号 | 行数 | 外部生产引用 | 备注 |
|------|------|------------|------|
| `EnhancedBorrowChecker` | 611 | **0**（只在自己文件的 `#[test]` 里） | 借用检查器第 2 套 |
| `AdvancedMacroExpander` | 617 | **0** | 宏展开器第 2 套 |
| `IdentityAwareBorrowChecker` | 470 | **0**（只在 `identity_ownership/tests.rs`） | 借用检查器第 3 套 |
| `declare_registry_runtime_fns` | 312 | **0**（仅 `pylib.rs:685` 一句注释） | @generated 孤儿 |
| `declare_core_runtime_fns` | 80 | **0** | @generated 孤儿 |
| `NewResolver`（struct） | — | **0** | 同模块 `InferContext` 是活的（typecheck_new 在用） |

**小计 ≥ 2,090 行零引用生产代码**（不含 `new_resolver.rs` 内 `NewResolver` 的那部分）。

**三处「多套实现」对照**：

```text
借用检查器：BorrowChecker(169 行) ✓接线  |  Enhanced(611) ✗死  |  IdentityAware(470) ✗死
           写了两套更「高级」的，都未采用，而接线的是最小的一套。

宏展开器：  MacroExpander(740) ✓接线        |  AdvancedMacroExpander(617) ✗死

类型检查器：unified_typecheck ✓接线         |  typecheck_new(690) ✓在链上（第一尝试）
                                         |  NewResolver(2,160) ✗死
```

**判定：C5 严重违反**——“备而不用的更高级实现”是本仓最系统性的坏味道：
共 1,698 行（两套借用检查器 + 一套高级宏展开器）写完后从未接线。

### 11.5 健康样本（同一仓内的对照）

| 设计 | 为何合理 |
|------|---------|
| `Type` / `Substitution` / `GenericContext` | 54 个方法、**零类内调用**、零 I/O——纯函数，每个都能单测 |
| `pylib` 数据驱动 | 加一个 Python 库 = 改 `registry.txt` 一行数据，不改 Rust 代码 |
| `Mir`/`MirStmt`/`MirExpr` | 纯数据类型，288 行，只 1 个方法（`dump_canonical`） |
| `ConstEvaluator` | 递归下降形态纯净（12 条回边全部语义必需），无上帝方法以外的职责 |
| 运行时 weak stub | 未实现 → 响亮 abort，把静默错值转成可观测失败（pyramid 5.4 的正确做法） |

**对照意义**：`types/mod.rs`（2,366 行但 0 类内耦合）与 `gen.rs`（13,636 行、17 个方法互调）
是**同一仓库里两种工程文化的极端**——说明问题不是语言或规模，而是**逐批特判的积累**。

### 11.6 综合结论

| 维度 | 判定 | 依据 |
|------|------|------|
| 依赖方向 | ✓ **基本合理** | 层间无环；`backend→frontend` = 0；跳层仅 5 处（集中在 identity 模块） |
| 依赖强度 | ✓ **合理** | 跨层调用均为查表式弱耦合（`pylib` 30 次、`types` 3 次） |
| 职责划分 | ✗ **不合理** | 三个 God class 占 28.9% 代码；均需「以及」才能描述 |
| 信息所有权 | ✗ **不合理** | `Resolver` 拥有 14 类上下文，靠 14 个 `with_*` 注入 `MirGen` |
| 抽象经济性 | ✗ **不合理** | 2,090 行零引用代码；三套借用检查器只用最小的一套 |
| 可测性 | ⚠ **分层差异大** | `Type`/`Mir` 可单测；`MirGen`/`LLVMCodegen` 必须整体驱动 |

**一句话**：**依赖关系是健康的，职责划分是失衡的**。
跨类/跨层的边少而弱（这是好消息）；问题全在**类内部的职责堆积**与**备而不用的抽象**。

### 11.7 改进优先级（按性价比）

| 优先 | 动作 | 依据 | 收益 | 风险 |
|------|------|------|------|------|
| **P0** | 删除 2,090 行零引用代码（两套备用借用器 + 高级宏展开器 + @generated 孤儿） | C5 实测零引用 | 减 2.3% 代码；减少「改错层」 | 极低（有 `mv` 到 `.trash/` 兜底） |
| **P1** | 把 14 个 `with_*` 收敛为一个 `LoweringContext` 结构体 | C3 违反；14 个入口 vs 1 个调用点 | 接口从 14 变 1，可测性↑ | 中（纯机械重构，需 MIR diff） |
| **P2** | `gen.rs` 按 §9.3 的三个切口拆分 | 13,636 行 / 22 扇出 | 单文件可读性 | 中（refactor D.1 已有方案） |
| **P3** | `Resolver` 拆符号表 / 类型表 / 模块别名三块 | 34 字段 + 19 RefCell | 职责清晰，可单测 | 高（改动面大，需先做 P1） |
| **P4** | 修 `frontend → middle` 5 处越层 | C2 | 自举可分阶段 | 中（identity 类型下沉） |

**P0 的理由**：零引用代码删除**不影响任何行为**，却能立即降低“改错层”的认知成本——
且小步提交可全量回滚（roadmap 硬规则：禁 `rm`，用 `mv` → `.trash/`）。

---

## 12. 跨层：ABI 与数据布局契约

pyramid 4.3 要求「一份书面 ABI 文档 + 单一实现点」。当前**无该文档**，契约散在 6 处：

| 契约面 | 实现点（类/函数） | 缺口 |
|--------|------------------|------|
| 4.3.a 调用约定 | `registry.txt` → `declare_registry_runtime_fns`（@generated） | 用户函数/struct 返回靠 `with_func_ret_types` 注入，无统一声明源 |
| 4.3.b 值表示 | **无表**；`LLVMCodegen::report_abi_coercions` 只报告 | 133 处非白名单转换；轴 B 就是补这张表 |
| 4.3.c 容器布局 | `DynamicArray` `[cap\|len\|data]`（host.rs:682）+ map 16 字节头与转发器 | 布局假设未升格为文档 |
| 4.3.d 名字修饰 | `Type::mangled_name`（types/mod.rs:854）+ `MirGen::with_symbol_renames`（gen.rs:348）+ `LLVMCodegen::mangle_function_name`（codegen.rs:1363）+ `aliases.inc.c` | **四套规则并存** |
| 4.3.e 合同治理 | `check_abi_anchors.py` | 无变更评审流程、无「ABI 影响」批次声明 |

**这是全仓最高风险区**：pyramid 判定「错一个字段偏移就是静默错值」，
而 4.3.b 的值表示表**整张不存在**。

---

## 13. 类关系总图

```text
┌─ 层 1 规范 ───────────────────────────────────────────────────────┐
│  registry.txt ──parse_registry──> PyModule/PyMember/PyMethod      │
│  pylib 函数族：find_module/find_member/method_symbol/all_externs  │
│                handle_op/handle_tag/all_stub_symbols              │
│  ErrorCodeRegistry                                                 │
└──────────────┬────────────────────────────────────────────────────┘
               │（codegen 声明 / MIR 分派 / 运行期符号）
┌─ 层 2 前端 ──▼────────────────────────────────────────────────────┐
│  indent_preprocess → parse_zeta → AstNode                         │
│  Resolver{ funcs, registered_funcs, impls, type_decls, … }         │
│    register / expand_macros / infer_untyped_returns / typecheck    │
│    lower_to_mir / monomorphize / take_generated_closures           │
│  Type / TypeParam / GenericContext / Substitution / KindContext    │
│  MacroExpander / ConstEvaluator                                    │
└──────────────┬────────────────────────────────────────────────────┘
               │ AstNode
┌─ 层 3 中端 ──▼────────────────────────────────────────────────────┐
│  MirGen{ stmts, exprs, type_map, name_to_id, … }  (13,636 行)     │
│    new + with_*(×14) → lower_to_mir → lower_ast → lower_expr       │
│    → lower_closure / lower_loop_else / lower_to_string            │
│  Mir{ name, stmts, exprs, type_map, … } + dump_canonical           │
│  MirStmt / MirExpr（enum）                                         │
│  MonoKey / MonoValue                                               │
└──────────────┬────────────────────────────────────────────────────┘
               │ Mir
┌─ 层 4 后端 ──▼────────────────────────────────────────────────────┐
│  LLVMCodegen{ module, ABI 账本 }                                   │
│    new / gen_mirs / report_abi_coercions / type_to_llvm_type       │
│    gen_fn / mangle_function_name / slot_or_sitofp / simd_* /       │
│    build_floordiv_* / infer_fn_return_type                         │
│  finalize_and_aot → module.verify() → clang                        │
│  declare_{registry,core}_runtime_fns (@generated)                  │
└──────────────┬────────────────────────────────────────────────────┘
               │ Syscall / extern 符号
┌─ 层 5 运行时 ─▼────────────────────────────────────────────────────┐
│  str_*/host_str_* | vec_push | map_* | print_* | py_df_*/py_vec_*  │
│  DynamicArray(host.rs:682) | unavailable_stubs（响亮桩）           │
│  zeta_src/*.z（51 文件自举源）                                     │
└────────────────────────────────────────────────────────────────────┘

层 6 正确性（横切）：run_all.sh / mir_diff.sh / diff_test.py / asan_run.sh / opt_matrix.sh
层 7 工程（横切）：main.rs 驱动 / gen_from_registry.py / build_runtime.sh
层 0 公理（横切）：三基线门禁 + 全优化级别
```

---

## 14. 层 ↔ 债务速查

| 层 | 达标度 | 主要缺口 | 轴 |
|----|-------|---------|----|
| 0 公理 | ⚠ | 官方 194 compile-only | E.1 |
| 1 规范 | ⚠ | registry 无契约测试 | E.3 / G.5e |
| 2 前端 | ✗ | **无 `Resolver::typecheck`**（非独立 pass）；W1002 静默截断 | **F** / G.7 |
| 3 中端 | ✗ | `type_map` 旁路 + 14 个 `with_*`；`BinaryOp{op: String}` | **B/D** / W1 / G.6 |
| 4 后端 | ⚠ | 4.3.b 值表示表不存在；**四套符号修饰规则并存** | **B** / G.1 / G.5 |
| 5 运行时 | ✗ | **值不带类型标签** → 判形启发式族 | **B（核心）** |
| 6 正确性 | ⚠ | 无 MIR verifier；三处降级继续 | G.1/G.3/G.4/G.6 |
| 7 工程 | ✗ | 越层依赖；gen.rs 13.6k 行；63 处裸 eprintln | A/C/D/E.4 |

**一句话**：每层职责在代码里**都有承载类**，但承载方式普遍是
「**旁路表 + 上下文注入 + 几何猜测**」而非「显式契约」——
轴 B（值模型）是总根，轴 F（类型检查独立 pass）是其编译期对偶。

---

*本文档为静态设计清点（类 + 函数 + 调用链），不含改造排程（排程见 refactor.md §9）。
行号对应 2026-09-22 工作区状态。*
