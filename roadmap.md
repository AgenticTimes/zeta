# Zeta 编译器 Roadmap

> 状态图例：[ ] 待做 | [~] 进行中 | [x] 完成 | [-] 放弃/降级
> 工作区：`/Users/meetai/source/zeta-src`（bootstrap 分支 → `agentic` 远端）
> 测试资产：官方单测 **`tests/unit-tests/`（194 文件，进 git 的正本）**；回归套件 `/tmp/bench`；**Python 风格套件 `tests/python_style/`（24 case：23 pass + 1 known-fail）**
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
- [x] std::quantum V1（2026-09-12）：QuantumCircuit/QubitState/Complex 占位对象
  （zeta_qc_new/measure/noop），quantum_basic 编译运行；真量子模拟仍待做
- [x] assert 内置（2026-09-12）：assert(cond, msg) → 失败时 zeta_assert_fail
- [x] `&mut` 引用参数（`6ba3ea8f`）
- [x] DUPLICATE_SYM（2026-09-12 完结）：① 删除 stub `count_primes`（恒返 0、
  与用户函数同名）；② **不透明方法兜底误抢 free 调用**——兜底分发原对
  receiver=None 的调用也生效，`count_primes(10)` 被抢成 `zeta_sieve_count(10)`
  （把 limit 当 handle 解引用 → 段错误）。修复：兜底仅限 method 形态
  （receiver.is_some()）
- [ ] NO_MAIN 库文件 main 包装器批量验证（test_loops/test_stability/test_suite/test_actual_issues 等，多为旧语法或测试套件文件）
- [ ] generic `where T: Ord` 约束检查（与 PY-3 泛型语法配套）
- [~] closures / async codegen 补齐（PY-4 lambda 依赖此项）
  - [x] `lambda x: e` 单行 Python 语法解析（`parse_python_lambda` → `AstNode::Closure`，置于 `parse_simple_ident` 前防被当变量名吞掉；`|x| e` 原语法不变；未提交）
  - [ ] 闭包值/调用 codegen（现有 `call_i64` 仅为恒等 stub；`generated_mirs` 是闭包函数发射通道；捕获环境待设计）
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
- [x] **`in` / `not in`**（2026-09-12）：解析期生成 `__contains__` 成员调用，字符串分发 host_str_contains；链式语境可组合（`a in b and c in d`）。数组/字典容器 V1 告警限界
- [x] **负索引 `arr[-k]`**（2026-09-12）：静态尺寸数组编译期改写 `n-k`；切片 `arr[1:3]` 仍待做（需 runtime slice 支持）
- [x] **切片 `arr[start:end]`**（2026-09-12）：end 排他（Python 语义）、省略形式 `[:e]`/`[s:]`；runtime `zeta_slice_vec` 返回 Vec 布局 handle（len/下标通用）；静态数组 end 哨兵编译期替换
- [x] **链式比较**（2026-09-12）：parse_comparison 重写为收集-折叠（`a < b < c` → `(a<b) && (b<c)`，边界操作数复用），is/is not/in/not in 均可入链
- [x] **元组解包全量**（2026-09-12）：`a, b = x, y` 并行赋值 + **`a, b = f()` 调用返回解包**（rhs 降级一次，stack_array_get 取元素）+ **`return a, b`** 逗号元组（match 臂内用单值形式——臂逗号是分隔符，元组需括号）
- [x] **字符串方法**（2026-09-12）：按 receiver 类型分发——upper/lower/trim/strip/lstrip/rstrip/contains/startswith/ends_with/replace/find/count/len/split（runtime `str_split` 返回 Vec 布局 handle）
- [x] **dict 下标/get/in**（2026-09-12，架构修复）：发现 **`lower_expr` 缺 DictLit 分支**（dict 字面量作为表达式时静默变 IntLit(0)，分支只在 lower_ast 语句级 match 里）——补齐并委托；字符串 key 内容哈希（`map_str_key` FNV-1a，同字面量不同 handle 的 key 归一）；`d.get(k)`/`k in d` 走 DictGet 同路径
- [x] **dict `.keys()`/`.values()`**（2026-09-12）：runtime `map_keys/map_values` 遍历开放寻址表 → Vec handle；`.get(k, default)` 双参形式待做
- [x] **try/except/finally/raise 完整语义**（2026-09-12）：setjmp/longjmp 方案打通——生成代码直调 `_setjmp(zt_slot)`（LLVM `returns_twice` 属性）+ runtime `zeta_raise` longjmp 到最近 try 帧（**跨函数立即中断**，真异常语义）。曾以为 longjmp 失效，实为 `except as e` 头解析漏前导空格 → e 绑定缺失 → else 块被判 undef 死代码整体删除。V1 限界：首个 except 捕获一切（类型过滤待做）、无 handler 时 abort
- [x] **装饰器 `@dec`**（2026-09-12）：解析消费忽略（def/class 前合法；语义改写待做）
- [x] **顶层 type alias**（2026-09-12）：`parse_type_alias` 强制分号是坏点——改可选；`type IntList = Vec<i64>` ✓
- [-] `global`/`nonlocal`、生成器/yield、async for —— 降级不做

## 执行顺序建议（下一步）

1. ~~f-string + 字符串值语义 + kqueue~~（2026-09-11 完成）
2. ~~class + 方法语义~~（2026-09-11 完成，commit `ad8532ab`）
3. ~~多参 print 修复 + 泛型多类型实例化~~（2026-09-11 完成，commit `24f17a57`/`ca06735e`）
4. closure codegen（解锁 t12 lambda；lambda 语法解析已完成，codegen 进行中）
5. ~~DUPLICATE_SYM~~ 完结（194/194）
6. 剩余深水区：closures 按引用捕获（V3 已实现 nonlocal 显式声明版；隐式
   读改写捕获待设计）、`where` 约束检查、WASM 后端、自举

## REasyQuant 真实项目实测（2026-09-13，未修改项目代码）

strategies/ 38 个策略编译：**7 个通过**，31 个失败——全部为外部库依赖边界，
非语法缺陷。失败文件的真·外部符号（扣除 runtime 已解析 prelude 与文件内
定义）去重后 **76 个**，分层决策：

| 层 | 符号举例 | 决策 |
|---|---|---|
| 聚宽平台 API（~35 个） | set_option / run_daily / order_target_value / get_current_data / get_fundamentals / query().filter().order_by().limit() | **接入不迁移**——宿主环境语义，由 REasyQuant 引擎以 shim .o 注入或经 IPC 调 Python 引擎 |
| Python 内建（~10 个） | list / int / float / min / sorted / sum / str.format / range | **迁移**——映射到既有 runtime（vec/map/to_string） |
| numpy 数学子集（~8 个） | arange / linspace / exp / pow / polyfit / diff / dropna / var | **迁移**——Zeta 数值 runtime（Vec + f64），polyfit = 最小二乘闭式解 |
| 用户函数前向引用（~15 个） | get_rank / iTrader / filter_*（定义在调用点之后或 arity 不匹配） | **编译器修复**——两遍 lower 或符号延迟绑定 |
| 日志（~6 个） | log.set_level / debug / info / basicConfig | **接入**——runtime logger（printf 归一） |

实测驱动的编译器修复（已落地）：keyword args、*args、参数位关键字名
（fn/open/high/low/set）、默认参数值、`~` 运算符、tab 归一化（行首 tab →
4 空格）、列表推导 `__collect__`、平台类构造 `zeta_platform_obj`、链式方法
identity 兜底、UTF-8 边界探针修复。

## 已知非阻塞

- stddev 不能走 `as i64` 中间步（截断为 0）
- 无 `-o` 模式 JIT 对简单 f64 程序 segfault（AOT 路径正常，低优）
- benchmark_simd_vs_scalar.z 源文件本身损坏（大量孤立 `}`），非编译器问题

## 库管理现状与 Python 库接入方案（2026-09-13 评估）

### 现状盘点：库管理"有名无实"

| 机制 | 现状 | 判定 |
|---|---|---|
| `use std::X` 模块解析 | Resolver 递归查找 build/stubs/std/X.z ✓ 文件存在 | 解析 ✓ |
| stdlib 桩（collections.z 等 12 个） | 桩内容是空壳 struct + no-op 方法（HashMap.insert 返回 None），无 runtime 支撑 | **有名无实**——`HashMap::new().insert(1,100)` 编译通过但链接失败（方法解析为裸名 extern `insert` 而非 `map_insert`） |
| zorb 包管理器（@scope/name） | 目录约定 + ~/.cache 缓存查找已写，但无 zorb 二进制、无包源 | **空架子** |
| `import numpy` / `import pandas`（Python 语法） | parse 后静默吞掉（PY-A import 容错），库符号全部落空 | **解析层假通过** |
| Python 库接入 | 无 FFI、无 CPython 嵌入、无 C ABI 映射 | **不存在** |

### 决策：Python 库"迁移还是接入"——按库分三类，不做全量兼容

全量兼容 pandas/numpy = 重写 CPython 生态（不可行）；嵌入 CPython = 拖入
解释器 + GIL，摧毁 AOT 定位。**正确路径是"用面驱动"**——REasyQuant 实测
31 个失败文件的外部符号去重后仅 76 个，其中 pandas/numpy 真实用面 ~30 个
（shift/groupby/rolling/polyfit/fillna...）。

| 层 | 内容 | 方案 | 工作量 |
|---|---|---|---|
| **L1 native**（本编译器最强项） | 数值计算：Vec/f64 数组上的 arange/linspace/diff/exp/log/polyfit/rolling/mean/std | 已实现一半（arange/diff/sorted/sum ✓）；补 exp/log/polyfit + f64 数组 layout 统一 | ~1 周 |
| **L2 mini-DataFrame** | 列名→Vec map（复用 map_str_key runtime）+ 策略实际用的 ~15 个方法（shift/rank/fillna/dropna/sort_values/rolling.mean） | 新 `dataframe.z` stdlib 桩 + native runtime（Map+Vec 组合） | ~2 周 |
| **L3 平台 API 边界** | jqdata 聚宽运行时（set_option/order_target_value/context.portfolio/get_price） | **接入不迁移**——runtime shim .o 由宿主（REasyQuant 回测引擎）提供实现，Zeta 二进制 extern 声明；本地 standalone 模式跑 no-op 桩（已部分落地） | shim ~1 周 |
| **L4 CPython FFI**（长期可选） | 真 pandas/numpy/scipy | Py_Initialize + PyObject 桥（类似 cffi 逆向）——仅在 L1/L2 覆盖不足时启用；优先级最低 | ≥1 月 |

### 关键架构原则

1. **平台 API 是环境不是库**：`set_option/run_daily/order` 的实现属于宿主
   （聚宽/REasyQuant 回测引擎），编译器只管 extern 声明 + 符号链接。策略
   AOT 二进制 = 纯计算核心，平台交互经 shim 边界回落宿主——与 C 程序
   链接 libc 的分工完全一致。
2. **用面驱动，不做全量兼容**：76 个符号里真正高频的是 ~20 个；先覆盖 80%
   调用点，剩余在真实策略报错时按需补。
3. **Python-only 语义降级**：groupby 多级索引、merge、时区等复杂语义在
   L2 不做——需要它们的策略留在 Python 引擎跑，Zeta 编译的是热路径。

## 通用 Python 编译：四层架构（2026-09-13 设计定稿）

目标重新定义——「所有 Python 都能被 Zeta 处理」的可达契约是：
**任何 .py 文件要么编译为原生，要么给出精确的不可编译原因，要么经桥接
回落 CPython**。全量动态语义原生编译不存在（需重写 CPython 对象模型），
业界（Nuitka/Mojo/mypyc）同此边界。

语料驱动排序（CPython 3.14 stdlib 155 文件 / 6702 函数的构造频率画像）：

| 构造 | 语料频次 | Zeta 现状 | 归属层 |
|---|---|---|---|
| try/except | 1412 | V1 error-state ✓ | L2 类型过滤 |
| class | 747 | ✓（struct 脱糖） | L1 |
| listcomp | 278 | ✓（__collect__） | L1 |
| with | 272 | ✗ 解析失败 | L1 解析（desugar → call+try） |
| starred `*args` 展开 | 268 | ✗ 调用点 | L1 |
| genexp `(x for x in y)` | 222 | ✗ | L2（迭代器协议） |
| yield/async def/await | 197/25/15 | ✗ | L4（协程状态机，深水区） |
| lambda | 130 | ✓（V2 捕获） | L1 |
| global | 65 | 部分（模块级 Assign→main） | L1（module 全局槽） |
| walrus `:=` | 59 | ✗ | L1（desugar） |
| dictcomp/setcomp | 39/11 | ✗ | L2 |
| nonlocal | 17 | ✓（V3 env） | L1 |

四层架构：
1. **L1 语法与静态语义**（100% Python 语法入库 + 静态子集原生编译）——
   parser 补 with/starred/walrus/genexp 解析；语义分类器（新 pass）对每个
   函数判定「静态可编译 / 依赖动态语义」，后者精确报错而非静默错译
2. **L2 运行时库**：stdlib 子集原生化（itertools 函数/collections/数据类）+
   迭代器协议（__iter__/__next__ 统一到 Vec/closure）
3. **L3 库边界**：C ABI FFI（extern 已有）+ zorb 包管理器实装（fetch 源码
   → 分类 → 编译或桩）
4. **L4 CPython 桥**（长期可选）：嵌入解释器处理动态残差，明确标注为
   「兼容边界」而非性能路径

验收方式改为语料驱动：以 CPython stdlib + REasyQuant 为基准语料，
逐层推进「解析通过率 100% → 分类覆盖率 → 静态函数原生编译率」三个指标。

### 下一步（按序）

1. L1 收尾：exp/log/polyfit runtime + f64 数组 layout 统一（StackArray/DynamicArray 双布局是当前最大技术债）
2. L2 mini-DataFrame：`dataframe.z` 桩 + native runtime（列存 Map+Vec）
3. 嵌套 def 方法分发修正（stub 方法解析为裸名 extern 的 bug——HashMap::new().insert() 应路由到 map_insert）
4. L3 shim 边界：REasyQuant 引擎侧提供 jq_shim.o（或确认现有 no-op 桩足够）
