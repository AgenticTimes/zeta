# Zeta 编译器 Roadmap

> 状态图例：[ ] 待做 | [~] 进行中 | [x] 完成 | [-] 放弃/降级
> 工作区：`/Users/meetai/source/zeta-src`（bootstrap 分支 → `agentic` 远端）
> 测试资产：官方单测 `/tmp/zeta_tests`（226 文件）；回归套件 `/tmp/bench`；**Python 风格套件 `tests/python_style/`（23 case：22 pass + 1 known-fail）**
> 当前通过率：官方 **199/226**（超基线 198，零回归）；python_style **22/23**
> 新目标（2026-09-11）：**基本能编译 Python**——PY-A 兼容层推进中
> 语法设计定稿：**`docs/python-syntax.md`（实现以此为准）**

## 定位决策（2026-09 已确认）

**「强化 Python」**：Python 级书写体验 + 静态编译性能（Swift/typed-Python 定位）。

| 取 Python | 取 Go | 保留 Zeta 内核（不取舍） |
|---|---|---|
| 缩进块（`:` + indent，`{}` 可选兼容） | `:=` 式类型推断 | match/枚举（穷尽性检查） |
| 无分号 | 简单包/模块模型 | 泛型（monomorphization）+ trait |
| `print()`/`len()` 风格 stdlib 别名 | GC 内存模型 | `&mut` 显式可变性 |
| `def` 关键字别名 → `fn` | 编译期常量 | Option/Result 错误处理 |
| 列表/字典字面量（已有） | | LLVM 后端 + 真线程并发 |

**红线**：不引入动态类型/动态派发——性能目标是 ≈C（串行 10^8 已 0.58s），动态化即前功尽弃。
泛型 + trait 是性能与抽象底座（编译期 monomorphization，非 Python 擦除式），保留不弱化。

## Python 化语法改造（设计定稿 2026-09-10，规则见 `docs/python-syntax.md`）

**架构总决策**：Python 化全部落在 **预处理（字符串层）+ parser 层**，AST 以下零改动。
**放弃 Unicode 哨兵 `‹ ›` 方案**——改为直接插入字面 `{`/`}`（插入点在行首/行尾，
与 parser 零冲突，12 处块解析器全部不动）。
注释保持 `//`（`#` 与属性语法 `#[...]` 冲突，不做）。

### PY-1 缩进块预处理【P1，完成】
- [x] `indent.rs` 全量重写：字符串状态机 + 三引号区域 + 块头关键字门控（`fn/def/if/elif/else/for/while/loop/match/struct/enum/impl/trait/concept/unsafe/comptime/mod`，或 `let … = <if|match|loop|unsafe|comptime> …:`）
- [x] 修复：行尾冒号剥离（`_main` 缺失根因）、注释行/空行/三引号续行不参与记账、`"http://x"` 不被 `//` 误切、EOF 收尾另起一行、行尾注释保留
- [x] 行首 tab → `IndentError::TabIndent`（仅代码行、仅 python 风格触发）
- [x] 单测 14 个全绿（indent.rs 内嵌）；花括号文件 100% 透传（wrapped type ascription 验证）
- [ ] P2：tab 错误显式诊断接 error_codes（现 nom Failure 无消息载体）

### PY-2 关键字别名 + 分隔符放宽【P1，完成】
- [x] `elif` → `else if`（stmt/expr 两处 parse_if 重构出 parse_if_tail 供 elif 链复用）
- [x] `def` → `fn`（词边界保护，`default` 不受影响）
- [x] `True`/`False` → `true`/`false`（parse_primary 首位 + 词边界）
- [x] struct/enum 成员 `,`/`;` 可选 → 缩进定义体可解析；逗号风格零破坏
- [x] match 臂间逗号可选
- [x] 12 处块解析器收敛——不需要（哨兵方案放弃）

### PY-3 `Name[T]` 泛型语法【P1，语法层完成】
- [x] `parse_type_path` 加 `[...]` 分支（嵌套感知，支持 `Map[str, Vec[i64]]` 混用）；归一化 `Name<T>` → mangle 统一
- [x] `fn f[T](x: T)` 声明 `[]` 形式（struct/enum/impl 泛型同步获得）
- [x] 裸 `[T]` 数组语义不变（验证）
- [ ] **既有缺口（新发现，非 PY-3 语法问题）**：泛型函数 T 参数实例化 E2E 不可用（`fn id<T>(x: T) -> T` 最简用例失败，`<>` `[]` 同样）；顶层 `type X = Y` alias 编译失败；`where T: Ord` 约束检查待做

### PY-4 Python 风格内置别名【P1，完成】
- [x] `range(n)`/`range(a,b)` → AST 重写为 `Range` 节点（端点排他 = Python 语义，实测验证；step 不支持）
- [x] `print(x)` 单参按类型分发 i64/f64/str → `println_i64/println_f64/println_str`（Python 语义带换行；修复运行时 `print` 符号为 fputs 字符串-only 导致整数段错误的 bug）；多参保持 print.N legacy 路径
- [x] `len(x)` 分发：字面量尺寸数组 → **编译期常量**；str → `str_len`；其余 → array_len 桩
- [x] `println(变量)` 修复：按参数类型分发（原无条件 `println_i64` 把字符串句柄当整数打印）
- [x] `lambda` → known-fail（依赖 closures codegen 待做项，t12 标记）
- [ ] P2：`and`/`or`/`not` 别名；`None` 字面量；`range` step
- [ ] **既有缺口（新发现）**：DynamicArray 的 `len()` 运行时桩 `array_len` 恒返 0；多参 print 只输出首参

### PY-5 三引号字符串【P2，完成】
- [x] `"""..."""`/`'''...'''` 解析（既有）+ 预处理器字符串状态机（三引号区域不触发缩进记账/冒号判定/注释剥离，起始行/续行语义对齐 Python）
- [-] f-string 插值——降级不做

### PY 验收标准（三套全绿）
- [x] `tests/python_style/run.sh` **14/15 pass**（+1 known-fail：t12 lambda 依赖 closures codegen）
- [x] 官方 226 测试零回归（198 → 199 通过；if-true 折叠修复使 test_complex_control 类用例受益）
- [x] `/tmp/bench` 285 文件编译对比零回归（基线 56 可编译 = 当前 56）
- [x] 混合风格双向可编译（t08）
- 期间修复的既有编译器 bug：**① if 布尔字面量条件 CTFE 折叠内联 terminator**（一基本块双 ret）；
  **② 用户枚举 unit 变体 match 第一臂必中**（模式当变量绑定无条件命中，现注册程序级 type_decls + 判别值比较）

## 原 Roadmap（编译器 bug 修复 + 官方对齐）

### 已完成

- [x] Float/double 全管线（IntLit/FloatLit 拆分、类型感知 load_local、fptosi/sitofp）— commit `b4f94066` 等
- [x] spawn/join 真实 pthread 并行（2×50M: 0.29s ≈ C）；串行 10^8 modulo 0.58s ≈ C 0.59s
- [x] Struct 字面量 f64 字段 bitcast — `52876537`
- [x] Boehm GC 集成（std.rs GC_malloc、C stub 全 GC、-lgc 链接）
- [x] Vec/Option/Result/String/HashMap C stub 运行时（GC-backed）— `4fe6f1e4`、`120bd3ee`、`655ca5a6`
- [x] array_new.10 LLVM 重命名 alias（.set 方案）— `e92fab30`
- [x] 函数参数类型从签名推导（f64 参数→double LLVM 参数）— `c96ea326`
- [x] Quant E2E：full_quant2.z sharpe 0.0599 / maxDD 0.0473（对齐 Python）
- [x] **struct `&mut self` 方法语义**：新增 `MirStmt::StructFieldStore`，`self.count += 1` 写穿堆指针；test_struct_method.z → 3 — `8c57a711`
- [x] 调用点参数类型强转 `coerce_call_args()`（int↔float、位宽）+ DictInsert 修复 — `367df251`
- [x] bootstrap `-o` 链接路径补 `-lgc` + `tokio_runtime.o`
- [x] **struct f64 字段比较 panic 修复**：BinaryOp 混合 int/float 操作数自动 sitofp 提升 — `ddc15293`
- [x] **loop/while break terminator 修复**：body 以 Break/Continue 结尾时不再发射回边 — `ddc15293`
- [x] runtime print2~print6 多参数实现 + print.N alias 按参数个数映射 — `ddc15293`
- [x] 单测通过率 190/227 (83%) → **207/229 (90.4%)**

## 进行中

- [x] **Enum match 修复**（`40eb0a1e`）：
  - `parse_match_arm` 支持 `return` 语句作为 arm body（`parse_return` 改 pub）
  - gen.rs Match arm body 是 Return 时发射 `MirStmt::Return`（原来静默返回 0）
  - `AstNode::Ignore` wildcard 支持（`_` 模式原来落到恒假）
  - test_match.z → 102 ✓、classify 1/2/0 ✓
- [x] runtime stub `.N` 后缀 alias 全套（print.11~103、array_new.11~36、stack_array_get.4 等 63 个）+ 补 stack_array_get/set 实现 — `40eb0a1e`/`ddc15293`
- [x] `;` 多余分号解析修复（parse_block_body 跳过空语句）— `40eb0a1e`
- [x] main 入口返回类型泛化验证：u64/i32/i64 均可（无需改 codegen）
- [x] **官方单测 190/227 (83%) → 205/226 (90.7%)**，回归 20/20 绿 — `ddc15293`
- [x] const 无类型注解语法 `const F = 55` 解析支持（parse_const ty 改 opt）— `98c17aa0`（CTFE 函数调用 const 仍待修）
- [x] **const/ctfe 语义**：`const F = compute()` 在 `compute` 标 `comptime` 时正确求值为 55（非 bug，测试用例未标 comptime 导致）
- [x] **loop 表达式带值**：`let r = loop { break 42; }` → 42；loop_value_stack + Break(Some) 赋值 + ret_expr 提升；test_simple_break.z → 42、test_loops.z → 97 — `7f4ddc21`
- [x] **&mut 引用参数**：`&mut x` 表达式降为 AddrOf（alloca ptrtoint），`fn f(c: &mut T)` + `*c` 读写穿透；test_while_fixes.z → ALL PASSED — `6ba3ea8f`
- [x] 官方单测 **205/226 (90.7%) → 207/226 (91.6%)**

## 待做（按价值排序）

- [ ] CTFE `const F = compute()` 未标 comptime 时静默求值错误（标了则正确）— 语义对齐待定
- [ ] std::quantum 模块：quantum_basic.z 的 cnot/execute/h/is_normalized UNDEF（Python 化改造期间挂起）
- [x] `&mut` 引用参数（`6ba3ea8f`）
- [ ] DUPLICATE_SYM：prime_counter_fixed.z / simplest_prime_counter.z（自编译符号冲突）
- [ ] NO_MAIN 库文件 main 包装器批量验证（test_loops/test_stability/test_suite/test_actual_issues 等，多为旧语法或测试套件文件）
- [ ] generic `where T: Ord` 约束检查（与 PY-3 泛型语法配套）
- [ ] closures / async codegen 补齐（PY-4 lambda 依赖此项）
- [ ] WASM 后端（官方宣传项）
- [ ] 自举（selfhost.z 依赖完整 stdlib，长期目标）

## PY-A Python 兼容层（2026-09-11，目标：基本能编译 Python）

### 已完成（本批）
- [x] `#` 注释（parser `line_comment` + 预处理器字符串状态机；`#[` 保留给属性）
- [x] `pass`（no-op 语句）
- [x] `and`/`or`/`not`（词边界守卫的运算符别名）+ `is`/`is not`（→ `==`/`!=`）
- [x] `None` 字面量（V1 降为 0；`is None`/`== None` 均可用）
- [x] `import x` / `from x import y` 容错解析（V1 消费不处理，模块映射后补）
- [x] `if __name__ == "__main__":` 主守卫（语句级解包 + **模块顶层语句合成隐式 `fn main`**）
- [x] **裸赋值隐式声明**（`x = 5` 无 let —— Python 核心语义；原静默错值）
- [x] **泛型函数 T 参数实例化修复**：`is_generic_function` 改用声明的 `Mir.generic_params`
  （旧启发式把「type_map 含 Type::Variable」的函数——包括泛型函数的调用者——误判为泛型而
  从不发射）；gen_mirs 第一遍后急切实例化（默认替换，参数落 i64）；
  `fn id<T>(x: T) -> T` + `id(3)` E2E 可用（t10 转绿）
- [x] **无类型注解参数**（`def f(x):` Python 常态）——原 parse_param 强制要求 `: type`，
  整个函数解析失败且函数体泄漏为顶层语句；现类型可选默认 i64（调用点强转适配 f64）
- [x] 顶层赋值/let 不再静默丢弃（收集进隐式 main）
- 官方回归：199=199 持平；integration_test_program 由坏转好
- 实测（真实 Python 脚本）：过程式脚本（while 内 early-return、递归、`#` 注释、主守卫）✓；
  `len(数组参数)` 仍 0（动态数组无长度头，见缺口清单）

### Python 对齐缺口清单（按对「编译真实 Python 文件」的影响排序）
- [x] **class**（2026-09-11）：`class Foo:` → struct + `def method(self, ...)` → impl 方法（self 隐式参数）；构造 `__init__`（字段从 `self.x = 字面量` 提取类型，`self.x = 参数` → 构造参数直传）；继承 `class A(B):` 显式报错；t21_class E2E（commit `ad8532ab`）
- [x] **f-string**（2026-09-11）：发现既有 `AstNode::FString`/`MirExpr::FString` 基础设施但 parser 从未产出——补 `parse_fstring`（字面量/`{expr}` 切分、`{{}}` 转义）+ 非 Str 部件 `to_string_*` 分发 + `str()` 内置
- [x] **字符串值语义**（2026-09-11，架构修复）：`==`/`!=` 按内容比较（新 runtime `str_eq`）、`+` 拼接。根因有二：① 运算符三轨降级不统一（`+`→SemiringFold/比较→BinaryOp/其余→Call），字符串 `+` 改道 BinaryOp 统一走 codegen 分发；② get_or_declare 数字后缀剥离丢「后缀须全数字」守卫，`to_string_f64` 被当 `to_string`+_f64 消歧后缀静默改名（符号谜团根源）
- [x] **多参 print**（2026-09-11）：`print("a", x, "b")` 空格分隔全输出。gen.rs 对**任意参数量**逐参类型分发（print_str/print_i64/print_f64）+ 参数间空格 + 末参走 println_* 带换行——完全绕开 print.N C alias 表（该表只出首参、且是 LLVM 重命名后缀的脆碰运表）；t22_multi_print 回归（commit `24f17a57`）+ MIR 发射顺序确定性排序（`6f68cc72`）
- [x] **泛型多类型实例化**（2026-09-11）：同一泛型函数按调用点参数类型推断实例化。① gen.rs：泛型参数携带 `Type::Variable(TypeVar(gidx))`（原为 I64 硬编码，替换无从谈起）；调用点无显式 type args 时按实参类型推断并替换声明返回类型（dest 槽位与实例一致）；② resolver：`string_to_generic_type` 按声明泛型列表解析参数/返回（原每次 fresh var，参数与返回互不相同，substitution 永不命中）；③ codegen：`monomorphize_function` 先替换后签名（f64 参数不再强转 i64 stub）、嵌套实例化时保存/恢复 gen_fn 状态（原 clobber 调用方 locals）、`get_or_declare_function` 推断路径查 `generic_defs`。`id(3)`→i64 实例、`id(3.5)`→f64 实例、`pair(2.5,1)`→f64 2.5；t23 回归（commit `ca06735e`）
- [x] **kqueue 移植**（2026-09-11）：tokio_runtime.c 原只有 epoll、macOS 无法重建 AOT runtime（链的是陈旧预编译对象）；现 `#ifdef __APPLE__` kqueue（reactor/waker/EVFILT_TIMER，事件位值沿用 epoll 编码）；`runtime/py_additions.c` 增量符号合并（ld -r）构建流程入册
- [ ] `in` 成员运算（数组/字符串/字典 contains runtime）；`not in`
- [ ] 负索引 `arr[-1]`、切片 `arr[1:3]`
- [ ] 链式比较 `a < b < c`（解析期脱糖）
- [ ] 元组解包 `a, b = f()`；多返回值
- [ ] 字符串方法全覆盖（`.upper()` 等 host_str_* 映射到方法调用语法）
- [ ] dict 方法（`.keys()`/`.values()`/`.get()` 默认值）
- [ ] try/except 映射（Zeta 有 Result/try-prop，语义对齐需设计）
- [ ] 装饰器 `@dec`（V2，可先解析忽略）
- [ ] 顶层 type alias（既有坏点）+ 顶层变量（模块级 `x = 5` → 全局）
- [-] `global`/`nonlocal`、生成器/yield、async for —— 降级不做

## 执行顺序建议（下一步）

1. ~~f-string + 字符串值语义 + kqueue~~（2026-09-11 完成）
2. ~~class + 方法语义~~（2026-09-11 完成，commit `ad8532ab`）
3. ~~多参 print 修复 + 泛型多类型实例化~~（2026-09-11 完成，commit `24f17a57`/`ca06735e`）
4. closures codegen（解锁 t12 lambda）
5. std::quantum / DUPLICATE_SYM / NO_MAIN 批量

## 已知非阻塞

- stddev 不能走 `as i64` 中间步（截断为 0）
- 无 `-o` 模式 JIT 对简单 f64 程序 segfault（AOT 路径正常，低优）
- benchmark_simd_vs_scalar.z 源文件本身损坏（大量孤立 `}`），非编译器问题
