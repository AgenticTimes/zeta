# Zeta 编译器 Roadmap

> 状态图例：[ ] 待做 | [~] 进行中 | [x] 完成 | [-] 放弃/降级
> 工作区：`/Users/meetai/source/zeta-src`（bootstrap 分支 → `agentic` 远端）
> 测试资产：官方单测 `/tmp/zeta_tests`（227 文件）；回归套件 `/tmp/bench`（20 case）；**Python 风格套件 `tests/python_style/`（15 case，基线 1/15）**
> 当前通过率：**207/226 = 91.6%**
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

### PY-1 缩进块预处理【P1，进行中】
- [x] `indent.rs` + `parse_zeta` 接入（雏形，有 bug，见下）
- [ ] **修复：行尾冒号未剥离**（`fn main() -> i64: {` 无法解析 → `_main` 缺失，13/15 用例因此挂）
- [ ] **修复：注释行参与 dedent**（先 pop 再判注释 → 列 0 注释会错误关闭函数块）
- [ ] **修复：字符串不感知**（`print("http://x")` 被 `//` 误切注释、行尾 `:` 在字符串内误判块头）
- [ ] **修复：EOF 收尾 `}` 拼到末行行尾**（末行是注释时 `}` 落入注释内，须另起一行）
- [ ] tab 报错改为显式诊断（行号 + 消息），且仅在 python 风格触发（保护花括号文件的 tab）
- [ ] 缩进栈压「body 缩进 = 下一行缩进」语义保持；`:` 后必须换行（V1 无单行块）
- 预算：重写 `indent.rs` ~200 行，parser 零改动

### PY-2 关键字别名 + 分隔符放宽【P1】
- [ ] `elif` → `else if`（`parse_if` else 分支接受 `elif`；dedent 后形如 `} elif x {`）
- [ ] `def` → `fn`（`parse_func` 头部 alias，一行）
- [ ] `True`/`False` → `true`/`false`（`parse_bool` alias）
- [ ] struct/enum 成员换行分隔（逗号/分号可选 → `struct Point:` + 缩进字段可解析；逗号风格零破坏）
- [ ] match 臂间逗号可选（缩进臂 `0 => expr` 逐行书写；expr 不会续接下一臂，安全）
- [x] 12 处块解析器收敛——**不需要**（哨兵方案放弃，见架构总决策）

### PY-3 `Name[T]` 泛型语法【P1，可与 PY-1 并行】
- [ ] `parse_type_path` 加 `[...]` 分支（与 `<...>` 并列）；归一化输出 `Name<T>` → mangle 自动统一，零 codegen 改动
- [ ] `fn f[T: Ord](x: T)` 泛型函数声明 `[]` 形式（`parse_func` generics 分支）
- [ ] 裸 `[T]` 数组语义不变（数组类型/字面量是裸 `[` 开头，与 path 后的 `[` 不相遇）
- 预算：~40 行 + 测试

### PY-4 Python 风格内置别名【P1】
- [ ] `range(n)`/`range(a,b)` → AST 重写为 `0..n`/`a..b`（`AstNode::Range` 已存在，`for` 主用；step P2）
- [ ] `lambda x: e` → Closure（单行限制；codegen 依赖「closures/async codegen」待做项，测试标 known-fail）
- [ ] `print(x)` 多类型分发（i64/f64/str → println_i64/println_f64/print_str；print2~6 多参已有）——E2E 验证
- [ ] `len(x)` 分发（array/vec/str；gen.rs/codegen.rs 已有映射，验证 E2E）
- [ ] `and`/`or`/`not` 运算符别名【P2】；`None` 字面量【P2，需类型上下文】
- [x] `def`、`True`/`False` → 已挪至 PY-2

### PY-5 三引号字符串【P2，parser 已完成】
- [x] `"""..."""`/`'''...'''` 解析（`parse_triple_quoted_string` 已接入 `parse_primary`）
- [ ] 预处理器字符串状态机：三引号区域不触发缩进记账/冒号判定/`//` 注释剥离（与 PY-1 修复同一次重写完成）
- [-] f-string 插值——降级不做（静态语言用 printf/format 更合适）

### PY 验收标准（三套全绿）
- [ ] `tests/python_style/run.sh` **15/15**（含 1 负面用例；known-fail 单列不阻塞）
- [ ] 官方 227 测试通过率不低于 91.6%（`{}` 语法 100% 兼容，fast-path 透传保证）
- [ ] `/tmp/bench` 回归 20/20
- [ ] 混合风格双向可编译（fn `{}` + 内部缩进、fn `:` + 内部 `{}`）

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

## 执行顺序建议

1. **PY-1b 预处理器重写**（冒号剥离/注释行/字符串状态机/EOF 收尾/tab 诊断）— t01/t02/t07/t08/t09/t15 转绿
2. **PY-2**（elif/def/True/False + struct/enum 换行分隔 + match 臂逗号可选）— t04/t05/t06 转绿
3. **PY-3**（`Name[T]`）— t10 转绿
4. **PY-4**（range/lambda + print/len 验证补缺）— t03/t11/t12 转绿
5. **PY-5**（状态机收尾，随 PY-1b 大半完成）— t13 转绿
6. 回归三套测试 → 提交 → 回编译器修复线：std::quantum / DUPLICATE_SYM / NO_MAIN 批量

## 已知非阻塞

- stddev 不能走 `as i64` 中间步（截断为 0）
- 无 `-o` 模式 JIT 对简单 f64 程序 segfault（AOT 路径正常，低优）
- benchmark_simd_vs_scalar.z 源文件本身损坏（大量孤立 `}`），非编译器问题
