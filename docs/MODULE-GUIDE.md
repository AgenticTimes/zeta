# MODULE-GUIDE.md — Zeta 各模块核心功能与伪代码导览

> 日期：2026-09-19　|　配套：[ARCHITECTURE-REVIEW-2026-09.md](ARCHITECTURE-REVIEW-2026-09.md)（问题证据）、[advice.md](../../advice.md)（优化方案）
>
> 伪代码是按调研快照**重构的控制流骨架**，非逐行对应；方括号标注真实位置，⚠ 指向分析报告中的问题编号。

---

## 0. 总管线与模块状态总表

```
.z / .py 源码
   │
   ▼ [indent.rs]        Python 缩进 → 花括号（文本级预处理）
   ▼ [top_level.rs]     parse_zeta：nom many0 递归下降 → Vec<AstNode>
   ▼ [ctfe/evaluator]   evaluate_constants：comptime 求值，ConstDef → 字面量
   ▼ [resolver.rs]      register：宏展开 + 符号注册 + Python import 处理
   ▼ [typecheck*.rs]    三段式检查（新系统 infer → fallback 旧检查）⚠失败仅告警
   ▼ [mir/gen.rs]       lower_to_mir：AST → MIR（嵌套语句树 + 表达式旁表）
   ▼ [resolver.rs]      monomorphize：泛型特化 ⚠替换表自映射
   ▼ [codegen.rs]       MIR → LLVM IR（inkwell 结构化构建）
   ▼ [jit.rs]           finalize_and_aot：verify → O3 → 写 .o
   ▼ [main.rs]          gcc 链接 zeta_runtime_c.o + tokio_runtime.o（Boehm GC）
   ▼
可执行文件
```

| 模块 | 路径 | 核心功能 | 状态 |
|---|---|---|---|
| 入口驱动 | `src/main.rs`、`src/lib.rs` | CLI、管线编排、JIT 模式 | 在用 |
| 缩进预处理 | `src/frontend/indent.rs` | Python 缩进/续行/单行复合体 → 花括号 | 在用 |
| 解析器 | `src/frontend/parser/{parser,expr,stmt,top_level}.rs` | 递归下降 → AST | 在用 |
| AST | `src/frontend/ast.rs` | 64 变体单一 enum，无 span，类型是字符串 | 在用 |
| 宏展开 | `src/frontend/macro_expand.rs` | macro_rules 风格展开 | 在用 |
| 借用检查 | `src/frontend/borrow.rs` | 轻量借用检查 | 在用（边缘） |
| 死代码群 | proc_macro / macro_expand_advanced / borrow_enhanced / identity_ownership / location | 历史尝试残留 | 死代码 |
| 常量求值 | `src/middle/ctfe/evaluator.rs` | AST 树解释器，编译期折叠 | 在用 |
| 符号解析 | `src/middle/resolver/resolver.rs` | 注册、内建表、Python import/mangling、单态化壳 | 在用（巨石） |
| 类型检查 | `resolver/typecheck*.rs`、`new_resolver.rs`、`unified_typecheck.rs` | 新旧双轨 | ⚠双轨陪跑 |
| 类型基础 | `src/middle/types/` | HM unify/kind/family/identity | ⚠基本未消费 |
| Python 注册表 | `src/middle/pylib.rs` + `pylib/registry.txt` | Python 面 → C shim 符号绑定 | 在用 |
| MIR | `src/middle/mir/mir.rs` | 嵌套语句树 + `HashMap<u32, MirExpr>` | 在用 |
| MIR 生成 | `src/middle/mir/gen.rs` | AST→MIR（含全部 Python 语义特判） | 在用（巨石） |
| MIR 优化 | `src/middle/optimization.rs` | DCE/CSE/折叠等 5 pass | ⚠零调用 |
| LLVM 后端 | `src/backend/codegen/codegen.rs` | MIR→LLVM IR、名字瀑布、强转 | 在用（巨石） |
| AOT/JIT | `src/backend/codegen/jit.rs` | 目标码发射、JIT 符号映射 | 在用 |
| 单态化工具 | `src/backend/codegen/monomorphize.rs` | MIR 级类型替换 | 与 codegen 重复 |
| C 运行时 | `runtime/tokio_runtime_stub.c` | print/str/map/vec/线程等 ~452 个符号 | 在用（AOT） |
| C 运行时 | `runtime/py_additions.c` | Python 兼容层 ~253 个符号 | 在用（AOT） |
| 异步底座 | `tokio_runtime.c`（根目录） | epoll/kqueue reactor | 未接通 |
| 实验件 | `runtime/capybara_runtime.c` | io_uring/TCP | 独立实验 |
| Python 库 | `pylib/numpy.z`、`pandas.z` | 最小真实现 + 明示桩 | 在用 |
| 自举语料 | `zeta_src/`（51 文件） | Rust 代码转写 | 仅 parse 回归语料 |

---

## 1. 入口驱动（`main.rs` / `lib.rs`）

**核心功能**：CLI 解析、按顺序编排上述管线、native 与 wasm 两种链接、JIT 模式（`lib.rs::compile_and_run_zeta`，用 Rust actor 运行时 + `add_global_mapping` 注册符号）。

```python
# main.rs 管线伪码
def zetac(path, out, run):
    src        = read(path)
    asts       = parse_zeta(src)                      # → §2
    asts       = evaluate_constants(asts)             # → §3.1
    r          = Resolver.new()
    r.register(asts.clone())                          # → §3.2  ⚠整树 clone
    r.typecheck_all()                                 # 失败 → W0003 告警，继续编 ⚠
    mirs     = { f.name: r.lower_to_mir(f) for f in r.funcs }   # 每函数 clone 14 张全局表
    for key in r.collect_used_specializations():      # → §3.5 单态化
        mirs[key] = r.lower_to_mir(monomorphize(key))
    mirs = sort_by_name(mirs)                         # 修 HashMap 迭代序 → print.N 竞态
    mod  = LLVMCodegen.new()                          # 声明 240 个运行时符号 → §4.1
    mod.gen_mirs(mirs)                                # → §4.2
    print_to_stderr(mod)                              # ⚠P2-13 无条件 dump IR
    obj  = finalize_and_aot(mod, Aggressive + default<O3>)   # → §4.3
    if run:
        gcc(obj, "zeta_runtime_c.o", "tokio_runtime.o", "-lgc", "-no-pie")
        exec(binary)
```

---

## 2. 前端

### 2.1 缩进预处理（`indent.rs`，919 行）

**核心功能**：无独立词法器；Python 缩进在**文本层**转换成花括号，之后解析器只面对花括号方言。`//` 地板除被改写为单词 `floordiv` 避免与注释混淆。

```python
def indent_preprocess(src):
    src = tabs_to_4_spaces(src)                 # tab 缩进 → 4 空格
    src = fold_backslash_continuations(src)     # 反斜杠续行折叠为一行
    src = fold_oneline_compound(src)            # `if x: stmt` → `if x { stmt }`
    src = rewrite_floordiv(src)                 # 非注释 `//` → ` floordiv `
    if already_braced(src): return src          # 花括号风格源码直通

    out, stack = "", [0]
    for line in src.lines():
        ind = count_leading_spaces(line)
        if ind > stack.top:  out += "{"; stack.push(ind)
        while ind < stack.top: out += "}"; stack.pop()
        out += line.strip() + (" {" if line.endswith(":") else "")
    return out + "}" * (len(stack) - 1)
# 已知问题：Box::leak 泄漏预处理文本 ⚠前端⑤；无行号映射（advice C1 的落点）
```

### 2.2 解析器骨架（`parser/`，合计 ~7,300 行）

**核心功能**：nom 组合子直接在 `&str` 上递归下降。四层分工：`parser.rs` 是基础设施（空白跳过、关键字表、类型字符串解析），`top_level.rs` 是入口与顶层项，`stmt.rs` 是语句，`expr.rs` 是表达式（最大）。

```python
# top_level.rs — 入口与错误恢复现状
def parse_zeta(src):
    pre = Box::leak(indent_preprocess(src))     # 泄漏一份源码副本
    asts, _rest = many0(parse_top_level_item)(pre)
    #                              ^ 第一个失败项处静默停止，尾部全部丢弃 ⚠前端②
    synthesize_implicit_main(asts)              # Python 模块级语句 → 合成 fn main
    return asts

def parse_top_level_item():                     # 分发：def/class/impl/import/@/let/...
    ...

# expr.rs — 表达式优先级：9 层手写阶梯（每层 50–90 行近似重复）⚠前端③
def parse_expr():    return parse_ternary()     # A if C else B（Python 三元）
def parse_ternary(): ...
def parse_logical_or():
    l = parse_logical_and()
    while peek_nospace("or") or (skip_ws(); peek("or")):
        r = parse_logical_and(); l = BinaryOp("or", l, r)
    return l
# ... parse_logical_and → parse_comparison → parse_bitwise_and/xor/or
#     → parse_additive → parse_shift → parse_multiplicative → parse_unary

# parse_primary 分发（expr.rs 尾部）：
#   字面量 / 标识符 / 括号 / list·dict·set 推导 / genexp / walrus :=
#   f-string（含相邻字符串隐式拼接）/ lambda / 比较链 a<b<c
```

### 2.3 AST（`ast.rs`，345 行）

```python
# 单一巨型 enum，64 变体，约 250–264 字节/节点 ⚠前端③
# 表达式/语句/定义/模式混在一起；无 span 字段；类型是 String
AstNode =
  | FuncDef { name, params: Vec<(String,String)>, ret: String, body, is_async,
              generics, where_clauses, ..., defaults, decorators }   # 15 个字段
  | ClassDef | StructDef | ConceptDef | ImplBlock | ConstDef | UseDecl
  | If { cond, then, elif_chains, else_ } | While { body, else_body }   # Python for/else
  | For { pat, iter, body, else_body } | Try { body, handlers, finally }
  | Assign { lhs, rhs } | AnnAssign { target: Var|FieldAccess|Subscript, ty, rhs }
  | BinaryOp { op: String, l, r } | Call { callee, args } | MemberAccess | Subscript
  | Import | FromImport | Raise | With | Global | Nonlocal | Del | ...
```

### 2.4 宏与检查（`macro_expand.rs` / `borrow.rs`）

```python
# macro_expand.rs（resolver 调用）
def expand_macros(funcs):                      # 对每个 FuncDef
    for attr in attrs if attr is macro_rules:
        rules = parse_macro_rules(attr)
        body   = apply_rules(body, rules)      # 模式匹配替换，重建 AST
        # 实现为逐字段 clone 重建（10+ 个 String/Vec.clone）⚠前端⑤
    return rewritten

# borrow.rs：轻量借用/可变冲突检查，typecheck 阶段跑一遍；错误主要提示性
```

### 2.5 诊断（`diagnostics.rs`）

```python
Diagnostic = { severity, code, span, message, help, note, suggestions }
diag_error!(code, msg, span=None)   # ⚠所有调用点 span 均为 None（前端①）
```

---

## 3. 中端

### 3.1 常量求值（`ctfe/evaluator.rs`，1,368 行）

**核心功能**：AST 树解释器。在 resolver 之前跑，把 `ConstDef` 原地替换为字面量；支持 comptime 函数调用（递归解释函数体）。

```python
def evaluate_constants(asts):
    for node in asts:
        if node is ConstDef:
            v = eval_const_expr(node.expr)
            replace(node, literal(v))              # AST → AST 改写
        if node is FuncDef and node.is_comptime:
            interp.register(node)

def eval_user_function_call(f, args):              # comptime 函数解释执行
    env = bind(f.params, args)
    for stmt in f.body: exec_stmt(stmt, env)       # 递归，recursion_depth 上限
    return env["ret"]
# ⚠gen.rs:14-113 另在 MIR 生成期读真实环境变量剪 if 分支（可复现性隐患）
```

### 3.2 符号解析（`resolver/resolver.rs`，3,718 行）

**核心功能**：注册一切符号；处理 Python import（别名/mangling/再导出）；内建函数表；单态化壳与特化缓存持久化。

```python
def register(asts):                                # ~934 行单函数 ⚠中端②
    for node in asts:
        case FuncDef:
            expand_macros(node)                    # → §2.4
            registered_func_defs[name] = node.clone()      # ⚠前端⑤ 整树 clone
            record(params, ret, generics, defaults)         # 类型存字符串
        case StructDef | ClassDef: 注册类型表 + 方法表 + 合成构造器
        case PyImport | FromImport:
            walk_py_import(ret_expr)               # try 块内 import 也扫（批次 126 修）
            py_module_aliases[alias] = module      # 17 张 Python RefCell 表之一 ⚠中端②
            mangling: mod__f / mod__Class__method
    register_builtin_functions()                   # ~930 行内建表

def resolve_call(name):                            # 名字解析；链路含
    ...                                            # module_renames_for / py_member_aliases
                                                   # / find_member（registry 线性扫描）
def monomorphize(key):                             # resolver.rs:2289-2349
    subst = zip(key.type_args, key.type_args)      # ⚠中端④ 自映射恒等，真替换推给 codegen
    ast = registered[key.func].clone()             # 全量 clone + 重 lower
    清空 generics 与调用点 type_args（仅 5 种节点有替换分支）
    return ast

def load_user_python_module(path):                 # 磁盘加载 .py → 手工 mangling
    合成 <module>init()，用 zeta_env_get/set 哨兵模拟幂等   # 模块系统 = 字符串改名 ⚠中端②
```

### 3.3 类型检查双轨（`typecheck.rs` 编排）

```python
def typecheck(node):
    borrow.check(node)                             # ① 借用
    r = typecheck_unified(node)                    # ② 新系统（HM）infer
    if failed(r) and r is not Mismatch:
        silently_skip(node)                        # ⚠中端① 错误静默吞掉
        return Fallback
    old_check_node(node)                           # ③ 旧系统：只查参数个数等
    identity.verify(node)                          # ④ identity 校验
    # 真正把关的是最弱的 ③；types/ 的 5,400 行 HM 基础设施基本只被 ② 陪跑 ⚠中端①
```

### 3.4 Python 注册表（`pylib.rs`，336 行 + `registry.txt`）

```python
def parse_registry(text):                          # include_str! 编译期嵌入
    for line in text:
        "M mod"                       → modules[mod] = {}
        "F mod member sym args=… ret=…" → modules[mod][member] = Fn(sym, abi, ret, handle)
        "W handle method sym"         → handle_methods[handle][method] = sym
        "X sym"                       → externs[sym] = Fn(...)      # 仅保证 ABI 声明
        "A a b"                       → aliases[a] = b
def all_externs():  return externs ∪ F/W 符号       # 供 codegen 生成 extern 声明
# ⚠幽灵符号：registry.txt:208 py_asdict_unexpanded 指向不存在的 C 符号（advice Q2）
```

### 3.5 MIR 数据结构与生成（`mir/mir.rs` + `gen.rs`）

```python
# mir.rs — 不是 CFG/SSA：嵌套语句树 + 表达式旁表 ⚠中端③
Mir = { stmts: Vec<MirStmt>, exprs: HashMap<u32, MirExpr>, type_map, ctfe_consts, ... }
MirStmt = If{cond_id, then: Vec<MirStmt>, els: Vec<MirStmt>}
        | While{cond_id, body} | For{iter_id, pat, body, else_body}
        | Assign{lhs, expr_id} | Return{expr_id} | Break | Continue
        | Call{func: String, args} ...             # op/func/字段名全是 String ⚠中端③
MirExpr = IntLit(i64) | StrLit | FloatLit(f64) | BinaryOp{op: String, l, r}
        | Call{func: String, args} | MemberAccess | StructNew{variant, fields} ...

# gen.rs — AST→MIR（10,995 行文件；lower_expr 约 7,970 行单 match ⚠中端②）
def lower_to_mir(fndef):
    g = MirGen(clone(14 张 resolver 全局表))        # ⚠中端⑤ 每函数全表克隆
    g.lower_ast_inner(fndef.body)                  # ~1,571 行：语句→MirStmt
def lower_expr(e):
    match e:
        IntLit → 建表达式，登记 type_map
        Call   → if builtin: 映射内建/运行时符号
                 elif py_member_call 可解析: 查 F/W 注册表发 shim 调用
                      (registered members win；np.where 多参在此拦截)
                 elif 局部函数: zeta_call_fn / 闭包 __closure_N
                 else: 发裸名（链接期 undef 的源头之一）
        MemberAccess → handle 属性（"PyDate"/"PyPath" 字符串标签）或方法绑定
        BinaryOp → 处理 floordiv / matmul / 短路
        # 216 处 py_ 特判、208 处 PY-A 注释都在这个 match 里
```

### 3.6 MIR 优化（`optimization.rs`，599 行）— **零调用**

```python
def optimize(mirs, level):                          # 全仓库无调用点；opt_level 无消费者 ⚠中端③
    passes = [dead_code_elimination, constant_folding, cse,
              strength_reduction(空操作), algebraic_simplification(空循环)]
    for _ in (level==O3 ? 10 : 1) 轮: run(passes)

def dce(mir):                                       # ⚠bug：嵌套体结果被丢弃
    mark(roots); for stmt in mir.stmts:
        if stmt is If: tmp = Mir{exprs:{}}; dce(clone(stmt.then))   # 结果不写回
    sweep(unmarked)

def cse(mir):                                       # ⚠bug：FloatLit 值不进 key
    seen = {}
    for e in exprs:
        key = match e { FloatLit(_) => "FloatLit", ... }   # 第二个浮点字面量错换第一个
        if key in seen: replace(e, seen[key])
        else: seen[key] = e
```

---

## 4. 后端

### 4.1 LLVM 代码生成（`codegen/codegen.rs`，6,865 行，inkwell/LLVM 21）

**核心功能**：MIR → LLVM IR，结构化构建（非文本拼接）。`new()` 硬编码声明 240 个运行时符号；核心是 `gen_mirs → gen_fn → gen_stmt/gen_expr`；名字解析靠瀑布；参数强转静默。

```python
def new():                                          # codegen.rs:71 起
    for sym in 240 个运行时函数: module.add_function(sym, abi)   # advice A2 生成化目标

def gen_mirs(mirs):
    for f in mirs: gen_fn(f)
    for g in eager_generics: monomorphize_function(g)  # 急切实例化（advice I3 改按需）

def gen_fn(f):
    entry = append_basic_block(f.name)
    for stmt in f.stmts: gen_stmt(stmt)             # 嵌套树直接递归（Break/Continue 在此处理）

def gen_stmt(s):                                    # ~2,172 行单函数 ⚠后端②
    case If:     cond_br(gen_expr(cond), then_bb, else_bb); 递归两臂
    case While:  loop_bb/cond_bb/end_bb; With: body falls_through 才追加 __exit__
    case Try:    zeta_try_enter/exit + landing 块（jmp_buf）
    case Return: f64→i64 bitcast（与既有 i64→f64 对称，批次 127）

def gen_expr(e):                                    # ~880 行
    match e: ... 大量查 locals / fns / specialized_fns 缓存

def get_or_declare_function(name, argtys):          # ~310 行名字瀑布 ⚠后端①（advice A4）
    try 顺序:
      ① fns 缓存 → ② 裸名 → ③ "::"→"__" mangle → ④ 裸方法名+arity 校验
      → ⑤ name_N 后缀 → ⑥ _inst_ 泛型 mangle → ⑦ 剥 _N 尾缀
      → ⑧ name.N（LLVM 冲突自动改名 → C 端 66 条 .set 别名追认）
      → ⑨ host_ 前缀映射 → ⑩ 兜底 extern 声明（链接期才报错）

def coerce_call_args(args, params):                 # codegen.rs:6208 ⚠后端②
    for (a, p) in zip(args, params):
        if type(a) != type(p): 自动 zext / trunc / sitofp / fptosi / bitcast
        # 静默强转：abs(-2.5)→nan 类错值的来源；advice B1 改白名单矩阵

def monomorphize_function(f, type_args):            # codegen 层按需实例化
    substitute_mir(mir, 位置替换 TypeVar(i) → type_args[i])
    缓存进 fns / specialized_fns（mangled 名 name_inst_i64_f64）
```

### 4.2 AOT / JIT（`jit.rs`，737 行）

```python
def finalize_and_aot(module, path):
    module.verify()
    tm = TargetMachine(OptimizationLevel::Aggressive)
    LLVMRunPasses(module, "default<O3>")            # 新 PM；优化全托 LLVM ⚠中端③
    write_to_memory_buffer → 写 <out>.o
def jit_run(module):
    EE = 执行引擎; for sym in ~90 个运行时符号: add_global_mapping(sym, C 地址)   # 手工清单
```

---

## 5. C 运行时

### 5.1 `tokio_runtime_stub.c`（3,331 行，~452 个导出符号）

**核心功能**：AOT 链接的全量符号库——print/str/map/vec/Option/Result、pthread spawn/join、threading/futures/asyncio/argparse shim、os/math/time。

```c
// Vec：[cap i64 | len i64 | elems...]，句柄指向数据区，header 在 handle-16
void* vec_new(n):   h = GC_malloc(16 + n*8); h->cap = h->len = n
void* vec_push(v,x): if len==cap: realloc 2x; elems[len++] = x        // 边界检查被注释 ⚠
i64  vec_get(v,i):  return elems[i]                                   // "bounds check omitted"

// Map：开放寻址，entry 24B [key i64 | val i64 | used u8]
i64 map_get(m, k):  i = FNV1a(k) & mask;  线性探测
                    if k 是字符串: 存内容哈希; g_keystr 旁路表(8192槽)找回键文本 ⚠碰撞
void map_insert(m,k,v): 命中 tombstone(used==2) 复用（批次 127）

// 线程：spawn → pthread_create；结果写全局环形数组
i64 spawn(fn, env): tid = counter++ % 256; thread_results[tid] = ...   // 256 并发后覆盖 ⚠
// asyncio：run/create_task 内联求值、sleep 直接阻塞（"Values are correct, concurrency is not"）

// 66 条别名追认 LLVM .N 改名（advice A3 数据化目标）
__asm__(".set _print.11, _println_i64");
```

### 5.2 `py_additions.c`（2,190 行，~253 个导出符号）

**核心功能**：Python 兼容层——f-string 规格、zip/sorted/min-max/key、strip 字符集、argparse、math 常量、numpy 部分函数、try/except、闭包 env、listcomp 收集器、dict/list 方法补全、importlib/getattr 占位。

```c
// f-string："f{x:>8.2f}" → 解析 align/fill/width/prec 并格式化
void py_format(fmt, val, out);

// try/except：Thread_local jmp_buf 栈，深度上限 64
i64 zeta_try_enter(void): if depth >= 64 return -1;  // ⚠静默不保护
    jmp_bufs[depth++] = setjmp point
void zeta_try_exit_ok(void): depth--;                // 异常: longjmp 到 handler

// 闭包自由变量：进程级 g_env 槽位，只增不清 ⚠
void* env_get(i64 slot);  i64 env_put(void* v);

// zip/sorted/min/max：GC 分配中间容器；sorted = qsort + 可调用比较
```

### 5.3 `tokio_runtime.c`（根目录，228 行）— 未接通的真异步底座

```c
// Linux: epoll + timerfd；macOS: kqueue + EVFILT_TIMER
// waker = pipe；scheduler_run_reactor 轮询就绪事件 → 调 waker
// 与 stub 的 asyncio shim 未打通（后端⑤：两头不靠）
```

---

## 6. 库层

### 6.1 `pylib/pandas.z`（334 行）— 列映射 DataFrame

```python
# 模型：一列 = map<str, vec>；不是行级存储
class DataFrame:
    def __init__(self, data): self.cols = data          # map<str, vec>
    def __getitem__(self, key): return self.cols[key]   # 真实现
    def dropna(self): return self.copy()                # 桩：恒等
    def isin(self, _):  return [1]*len(self)            # 桩：全 1 掩码 ⚠D 响亮化目标
    def groupby(self, _): return GroupBy(空)            # 桩
# 注释里记录了"绕编译器 bug 写法规约"：形参勿叫 name、方法必须写返回注解、
# 负切片 SEGV —— 库层正确性依赖编译器缺陷规避清单 ⚠测试体系③
```

### 6.2 `pylib/registry.txt`（497 行）

```
M  pandas                     # 声明模块（共 29 个）
F  pandas read_parquet py_pandas_read_parquet args=… ret=…   # 成员→C shim（190 条）
W  PyPath exists py_path_exists                              # 句柄方法分派（89 条）
X  py_member_call             # codegen 可能发射的辅助 shim（63 条）
A  futures concurrent.futures # 别名
# include_str! 嵌入 → 改 registry 要重编编译器（advice 第 7 节风险表）
```

### 6.3 `zeta_src/`（51 文件，7,288 行）

名义自举（目录结构镜像 Rust 编译器），实为 Rust 代码逐句转写（引用不存在的 `zeta::` 模块）；当前角色 = parse 管线回归语料。

---

## 7. 一次调用的端到端走读（示例：`print(df.head(2))`）

```python
1. indent.rs      : 缩进 → {}，行号映射（未来 C1 在此建）
2. top_level.rs   : 顶层解析出 FuncDef；模块级语句合成 main
3. ctfe           : 无 comptime → 直通
4. resolver       : import pandas → py_module_aliases；df 类型未知 → 无标注
5. typecheck      : 新系统 skip（df 无类型信息）→ 旧系统查参数个数 → 通过
6. mir/gen        : MemberAccess(head) → py_member_call(df, "head", [2])
                    → registry W 表 → py_df_head（handle 分派）
7. codegen        : get_or_declare_function("py_df_head") 瀑布命中注册表
                    → call i64 (i64, i64)；coerce 无需（全 i64）
8. jit.rs         : default<O3> → 写 .o
9. 链接           : gcc main.o zeta_runtime_c.o tokio_runtime.o -lgc -no-pie
10. 运行          : py_df_head 读 map 列头 2 行 → println 输出
```
