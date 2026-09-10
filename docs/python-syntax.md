# Python-like 语法设计定稿（PY-1 ~ PY-5）

> 状态：已定稿（2026-09-10），实现按此文档执行。
> 红线：不引入动态类型/动态派发。所有 Python 化改动只发生在 **预处理 + parser 层**，
> AST 及以下（MIR/LLVM codegen）零改动（PY-3 除外，见 R5）。

## 0. 总体架构

```
源码 .z
  │
  ├─► indent_preprocess()   ← PY-1/PY-5：缩进块 → {}，唯一的新增 pass（字符串层，非 parser 层）
  │       │  （纯花括号风格文件 fast-path 原样透传，官方 227 测试零风险）
  ▼
nom parser（改动：关键字别名、分隔符放宽、Name[T]）
  │
  ▼
AST ──► MIR ──► LLVM（不改）
```

**决策变更（相对 roadmap 初版）**：放弃 Unicode 哨兵 `‹ ›` 方案，采用
**直接插入字面 `{`/`}`**。理由：预处理器工作在字符串层、插入点都在行首/行尾
（非字符串内部），与 parser 中已有的 `{}` 解析零冲突；哨兵方案需要动 12 处
块解析器，收益为零。

## R1 缩进块预处理（PY-1）

`src/frontend/indent.rs::indent_preprocess` 规则定稿：

1. **触发条件（fast path）**：仅当源码存在「非字符串内部、非注释内的行尾冒号」时启用
   （`looks_like_python_style`，需 string-aware）。纯花括号文件原样透传——
   这是官方 227 测试零回归的保证。tab 检查只在触发后进行（花括号文件里
   的 tab 缩进不受影响）。
2. **缩进计宽**：行首空格数。行首出现 tab → 编译错误（报错信息明确指向行号）。
3. **块开始**：行尾冒号（考虑字符串与 `//` 注释后判断）且下一非空非注释行缩进更深
   → **剥离行尾冒号**，行尾追加 ` {`，压栈「body 缩进 = 下一行缩进」。
   已含 `{` 的行不作为块头（混合风格基础）。
4. **块结束**：当前行缩进 < 栈顶 body 缩进 → 弹栈并在行首插入 `}`（可连弹多次）。
   同缩进 = 不开不关；更深缩进且非冒号行 = 继续同一块。
5. **空行 / 纯注释行不参与缩进记账**（不触发 dedent，不压栈）。
   ⚠️ 修复现存 bug：当前实现先做 dedent 再判注释，行首注释会把函数块错误关闭。
6. **字符串状态机**（PY-5 联动）：逐字符扫描行内引号状态（`'`/`"` + 转义 + 三引号区域）。
   仅在「非字符串」部分：识别行尾 `:`、剥离 `//` 注释、判定行首缩进。
   三引号字符串区域整段跳过（不判缩进、不插括号），直到闭引号。
7. **EOF 收尾**：栈未空 → 补 `}` **另起一行**（不得拼到末行行尾——末行可能是注释，
   `}` 会落入注释）。
8. **V1 限制**：冒号后必须换行 + 更深缩进（`if x: stmt` 单行块不支持）。
   多语句单行（`a = 1; b = 2`）本来就行，不在本规则内。

### 逻辑副作用清单（实现时必须验证）

- `lambda x: e` 单行表达式不以冒号结尾 → 不受影响；多行 lambda 不支持（V1）。
- 字典/struct 字面量多行书写：`{` 已在行内 → 不作为块头；其内部行以 `,` 结尾，
  不触发任何规则。行首缩进比外层深 → 不会 dedent，安全。
- `let s = "http://x"`：`//` 在字符串内，行尾判定用状态机 → 不误切。
- `match x:` + 缩进臂：预处理只负责外层 `{}`；臂本身仍是 `P => expr` 语法
  （见 R3 的臂分隔放宽）。

## R2 关键字别名（PY-2 / PY-4 交界）

| Python 写法 | Zeta 语义 | 实现点 | 优先级 |
|---|---|---|---|
| `elif cond:` | `else if cond` | `parse_if` 的 else 分支接受 `elif`（dedent 后形如 `} elif x {`） | P1 |
| `def f(...)` | `fn f(...)` | `parse_func` 头部 `alt((tag("fn"), tag("def")))` | P1 |
| `True` / `False` | `true` / `false` | `parse_bool` 加分支 | P1 |
| `lambda x: e` | `|x| e`（Closure） | `parse_closure` 加 `lambda` 前缀别名；**单行限制** | P1 |
| `and` / `or` / `not` | `&&` / `\|\|` / `!` | 二元/一元运算符层加 alias（优先级一致：not > and > or） | P2 |
| `None` | Option none 字面量 | 需类型上下文推断（runtime 有 `option_make_none`） | P2，另行设计 |
| `range(n)` / `range(a,b)` / `range(a,b,s)` | `0..n` / `a..b`（step P2） | parse 后 AST 重写 `range` 调用 → `AstNode::Range`（`for` 主用） | P1 |

不做的：`f-string` 插值（静态语言中 printf/format 更合适，[-] 降级）、
`class`（用 `struct`）、`self` 隐式参数（保留显式，`&mut self` 已有语义）。

## R3 混合块识别 + 分隔符放宽（PY-2）

1. **12 处块解析器不动**（V1 结论，见 §0 决策变更）。混合风格由预处理器
   规则 3/6 天然保证：花括号行不作为块头，预处理插入的 `}` 只关预处理开的块。
2. **struct/enum 成员换行分隔**：`separated_list0(ws(tag(",")), ...)` 放宽为
   逗号/分号 **可选**（`opt(alt((tag(","), tag(";"))))`），使
   ```
   struct Point:
       x: i64
       y: f64
   ```
   预处理成 `{}` 后可解析。逗号风格继续兼容。实现时必须回归验证
   struct 字面量/枚举字面量的逗号语义不受影响（它们走独立的 literal parser）。
3. **match 臂逗号可选**：`parse_match_expr` 中臂间 `,` 改为可选，使缩进风格的
   ```
   match x:
       1 => println_i64(1)
       _ => println_i64(0)
   ```
   可解析。安全性依据：臂 body 是完整 expr，换行后 `_ =>` 无法被 expr 续接。

## R4 `def` 冒号函数 + 返回类型（并入 R1/R2）

`def f(x: i64) -> i64:` = `fn f(x: i64) -> i64 {`，全部由 R1（预处理）+
R2（def 别名）组合覆盖，无独立工作项。

## R5 `Name[T]` 泛型语法（PY-3）

- **类型位置**：`parse_type_path` 在 path 后加 `opt(...)` 的 `[...]` 分支
  （与既有 `<...>` 并列）。归一化输出仍是 `Name<T, ...>` 字符串 →
  mangle 与 `<>` **自动统一**（`Vec[i64]` ≡ `Vec<i64>`），零 codegen 改动。
- **泛型函数声明**：`fn f[T: Ord](x: T)` → `parse_func` 的 generics 分支加 `[]` 形式，
  内部结构同一枚举。
- **冲突分析**：数组类型 `[T; N]` / 数组字面量 `[1, 2]` 都是**裸 `[` 开头**；
  `Name[T]` 的 `[` 前面是具名 path——在 `parse_type_path` 内部处理即可，二者不相遇。
- `where T: Ord` 约束检查是后续独立项（roadmap 原「待做」保留）。

## R6 stdlib 别名（PY-4）

现状盘点（实现=验证+补缺）：

| 名称 | 现状 | 动作 |
|---|---|---|
| `print(x)` | resolver 已有 `print` 签名（i64），print2~6 多参 runtime 已有 | 验证 E2E，补 f64/str 重载映射 |
| `println(x)` | resolver 已有 | 同上 |
| `len(x)` | gen.rs `len→array_len`、codegen `len→str_len` 已有 | 验证 array/vec/str 三态 |
| `range(...)` | 无 | R2 表：AST 重写 → Range |
| `def` | 无 | R2 表：parser alias |
| `lambda` | Closure 语法是 `\|x\| e` | R2 表；codegen 不完整则先 parse-only |
| `True/False/None` | 只有小写 true/false | True/False P1；None P2 |

## R7 三引号字符串（PY-5）

- parser 已完成（`parse_triple_quoted_string` 已接入 `parse_primary`，支持 `"""`/`'''`）。
- 剩余工作 = R1 规则 6 的预处理器状态机：三引号区域不触发 INDENT/DEDENT/
  冒号判定/注释剥离。
- f-string 降级不做。

## 测试资产

- `tests/python_style/*.z`：15 个用例，自包含期望输出（`// expect: ...` 注释），
  `run.sh` 编译→运行→逐行比对；支持 `// expect-error` 负面用例。
- 覆盖矩阵：缩进 fn/if/elif/while/loop/for+range/match、struct/enum 缩进定义、
  深嵌套 + 空行注释干扰、双向混合风格、无分号、`Name[T]`、print/len/def、
  lambda、三引号、tab 报错、多行字面量共存。
- 验收门槛：15/15 绿 + `/tmp/bench` 20/20 + 官方 227 通过率不低于 91.6%（207/226）。

## 实现顺序（每步一个 commit）

1. **PY-1b 预处理器修复**：R1 规则 2/3/5/6/7（冒号剥离、注释行、状态机、EOF 收尾、tab 报错）
2. **PY-2**：R2 的 elif/def/True/False + R3 的 2/3（分隔符放宽）
3. **PY-3**：R5 `Name[T]`
4. **PY-4**：R2 的 range/lambda + R6 验证补缺
5. **PY-5**：R7 状态机收尾
6. **回归**：三套测试全绿，roadmap 状态更新
