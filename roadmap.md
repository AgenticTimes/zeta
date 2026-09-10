# Zeta 编译器 Roadmap

> 状态图例：[ ] 待做 | [~] 进行中 | [x] 完成 | [-] 放弃/降级
> 工作区：`/Users/meetai/source/zeta-src`（bootstrap 分支 → `agentic` 远端）
> 测试资产：官方单测 `/tmp/zeta_tests`（226 文件）；回归套件 `/tmp/bench`；**Python 风格套件 `tests/python_style/`（15 case：14 pass + 1 known-fail）**
> 当前通过率：官方 **199/226**（基线 198/226，编译+运行口径 198/198 零回归）；python_style **14/15**
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

## 执行顺序建议（PY 线已完成，下一步）

1. ~~PY-1 ~ PY-5~~（2026-09-11 完成，python_style 14/15）
2. 编译器修复线：**泛型函数 T 参数实例化**（PY 线最大新发现缺口）→ 顶层 type alias → closures codegen（解锁 t12）
3. std::quantum / DUPLICATE_SYM / NO_MAIN 批量
4. P2 语法糖：and/or/not、None、range step、tab 诊断接 error_codes

## 已知非阻塞

- stddev 不能走 `as i64` 中间步（截断为 0）
- 无 `-o` 模式 JIT 对简单 f64 程序 segfault（AOT 路径正常，低优）
- benchmark_simd_vs_scalar.z 源文件本身损坏（大量孤立 `}`），非编译器问题
