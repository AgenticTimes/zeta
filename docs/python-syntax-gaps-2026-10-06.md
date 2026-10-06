# Python 语法支持面：缺口清单与实现方案（2026-10-06）

> **来源**：批次 10060 的实测（163 枚读数，分五轮；台账 `roadmap.md:33363`；81 枚用例已入库
> `tests/diff/cases/syn60_*.dcase`／`syn60b_*.dcase`）。本文不做新测量，只做两件事：
> ① 把每条缺口追到代码站点并说明为什么；② 给出可分批执行的实现方案、风险与验收方式。
>
> **口径**：期望值一律来自 CPython 现场运行；`zetac` 编译成二进制后执行，stdout 逐行比对。
> 本文所有 `file:line` 在撰写时逐一实读核对过（核对命令见 §1.3）。行号会随注释编辑搬家，
> **动身之前请按同一锚点文本重新定位一次**，不要直接抄行号。
>
> **本文不包含**：性能问题、标准库缺失清单、`src/frontend/indent.rs` 预处理器缺陷。

---

## 0. 一页速览

| 组 | 条数 | 机制 | 主要站点 | 要不要额外授权 | 建议顺序 |
|---|---|---|---|---|---|
| A | 9 | 解析器不认该写法，把该行之后整段程序丢掉 | `src/frontend/parser/*` | 主车道在制面，动身前要报备 | 第 3、6、7、15 批 |
| B | 3 | 运行期明确报"这个方法名编译器不认识" | `src/middle/mir/gen/call_set.rs`／`call_dispatch.rs` | B2 不要；B1/B3 要（`runtime/*.c`） | 第 2、11、13 批 |
| C | 16 | 认了，但值或呈现不对（静默错，退出码 0） | `call_print.rs`、`gen.rs`、`stmt_assign.rs`、`call_field.rs` | 打印链要（`call_print.rs`） | 第 4–14 批（按链走，见 §4） |
| D | 3 | 跑出的二进制停住，`kill -9` 也收不掉——**D2 已读到链（arity 缺陷），D1/D3 挂死点未定位** | D2：`call_num.rs:287`；D1：`py_zip`；D3：未定 | D2 不要；D1/D3 的改动候选要 | 第 10 批（10a 零授权可提前） |
| M-A0 | 1 | 判定口径：解析失败看起来像"通过" | `tools/diff_test.py` | 不要 | **第 1 批** |

四组不是四个独立问题：C 组 16 条实际聚成 6 条链（§4）；D 组 3 条本文读了三条 MIR，
**是三条不同的链**——D2 已经读到发射点少发一个参数（§5），D1/D3 的挂死点仍未定位。
A 组 9 条彼此独立（都是解析表少一项）。

---

## 1. 动手前先读的三节

### 1.1 已有资产（不要重复造）

| 资产 | 用途 | 位置 |
|---|---|---|
| `ZETA_STRICT_PARSE=1` | 把解析截断从警告升级为致命错误 `E1002`，用来量"这个程序真编译了多少" | `src/main.rs:497` |
| `tools/parse_bisect.py` | 把 W1002 从"停在哪个顶层条目"细化到"哪一行的哪个构造" | `tools/parse_bisect.py`（说明见 `validate.md:49`） |
| `tools/truncation_inventory.sh` | 一次跑完官方递归 + 语料，按丢弃行数排序 | `validate.md`、`refactor.md` G.7a |
| 10 族构造分类表 | 此前一批（批次 321）已经分类过的**Rust 方言**截断族清单 | `docs/ABI.md` 附 B#10 |
| 差分语料 + 判定器 | CPython 对照，基线只升不降 | `tests/diff/cases/`、`tools/diff_test.py` |
| Python 化语法的设计定稿 | PY-1～PY-5 的分层红线（改动只在预处理 + parser 层） | `docs/python-syntax.md` |
| 历史缺陷单元测试 | 毫秒级复验，全局清单只减不增 | `tests/regression_history.rs`（现 62 条） |

本文的 A 组是**Python 写法**，与附 B#10 那 10 族 Rust 方言写法不重叠；但方法完全沿用：
先 `ZETA_STRICT_PARSE=1` 复现 → `parse_bisect.py` 定行 → 单构造最小用例入库 → 改 parser。

### 1.2 一条必须写进所有验收标准的纪律

解析失败的现有效果（实测 7 枚，手工复跑 4 枚核对）：

1. `zetac` 退出码 **0**；
2. stderr 有 `warning: [W1002] N line(s) at the end of the input were NOT parsed …`；
3. 产出的二进制能跑、退出码 **0**、stdout **0 字节**。

也就是说"这个语法不支持"在只看退出码和 stdout 的口径下**看起来像通过**。站点：
`src/main.rs:466-502`（`ensure_fully_parsed`；默认警告、`ZETA_STRICT_PARSE` 才致命，
注释里写明理由：官方套件 11 个文件有不可解析尾部，直接改成致命会把 194/194 翻红）。

### 1.3 本文行号的核对方式

```bash
# 逐条重取锚点（示例：A1 一元正号、B2 并集方法臂、map 形状闸门）
grep -n '&mut' src/frontend/parser/expr.rs | head -3
grep -n 'method == "intersection"' src/middle/mir/gen/call_set.rs
grep -n 'zt_map_cap_ok' runtime/tokio_runtime_stub.c
```

---

## 2. A 组：解析器不认的 9 种写法

难度分三档：**一档**＝运算符/参数表里少一项，改动局限在一个函数内；**二档**＝要在 AST 里加位点
并接上 MIR 降级；**三档**＝要引入新语义（只有 A9）。

### 一档（4 条）

#### A1 一元正号 `+x`

- **实测**：`print(+x)` → 空输出（同一探针里 `-x` 单写是逐行一致的）。
- **站点**：`src/frontend/parser/expr.rs:1359 parse_unary`，运算符表在 `:1379`：
  `opt(alt((tag("&mut"), tag("&"), tag("-"), tag("*"))))`。`+` 不在表内，
  于是 `+` 未被消费，整个表达式解析失败。
- **方案**：`:1379` 的 `alt` 里加 `tag("+")` 并在 `:1387` 的分支里让 `+` 走恒等
  （直接返回内层表达式，不发 `UnaryOp`）——比发一个新的 `"pos"` 运算符更省，因为下游
  MIR/后端不需要多认一个运算符。
- **风险**：`+` 也是二元运算符；`parse_unary` 只在表达式开头走到，`a + b` 由
  `parse_additive` 处理，两者不冲突。仍要加一枚 `print(1 + 2)` 与一枚 `print(+1)` 同文件的用例，
  确认没把二元加号吃进一元分支。
- **验收**：`cargo test -p zetac --lib parser` + 单构造 `.dcase`（`syn60b_unary_plus`）。

#### A2 幂赋值 `x **= 3`

- **实测**：空输出；`+= -= *= /= //= %= &= |= ^= <<= >>=` 都逐行一致。
- **站点**：`src/frontend/parser/stmt.rs:600-613` 的 `alt` 表——这是全仓唯一的 `AssignOp`
  产生点（在 `:664` 使用）。表里有 `*=` 没有 `**=`，也没有 `//=`；
  `tag("*=")` 无法匹配 `**=`（第二个字符就是 `*`），整条 `alt` 失败。
- **方案**：两步。① 表里按**先长后短**加 `ws(tag("**="))` 与 `ws(tag("//="))`，
  放在 `*=`／`/=` 之前（次序错＝`**=` 会被读成 `*=` 加一个悬空 `*`）。
  ② `:661` 的 `op.trim_end_matches('=')` 之后要给 `**` 与 `//` 接上降级：
  现有 `//` 已在表达式侧有落点（`syn60b_floordiv_call` 是绿的），照它的运算符名复用，
  `**` 同理复用二元 `**`（`expr.rs:1400` 起有专门的 `**` 优先级层）。
- **验收**：`syn60b_pow_assign`、`syn60b_floordiv_assign` 两枚单构造入库。

#### A8 推导式里两个 `if` 连写

- **实测**：`[x for x in rows if x > 1 if x < 5]` → 空输出；单 `if` 逐行一致。
- **站点**：`src/frontend/parser/expr.rs:1240-1246`（`parse_listcomp_full`，函数起于 `:1181`）：

  ```rust
  let (input, cond) = if let Ok((rest, _)) = ws(tag("if")).parse(input) {
      let (rest, c) = ws(parse_full_expr).parse(rest)?;
      (rest, Some(c))
  } else {
      (input, None)
  };
  let (input, _) = ws(tag(close_str)).parse(input)?;
  ```

  只吃**一个** `if`（`:1177` 的注释就写着 `(if cond)?`）。第二个 `if` 留在流里，
  `:1246` 的收尾括号匹配失败 ⇒ 整个推导式失败。字典推导同样形状在 `expr.rs:981-984`。
- **方案**：把单条 `if` 改成 `many0` 收集条件列表，然后**嵌套复用** `:1261-1268` 已经在造的
  `AstNode::If`（外层第一个条件最内层）。AST 与 MIR 零改动，改动只在 `:1240` 这一段。
- **验收**：`syn60b_comp_double_if`；再加一枚"两个 `if` + 一个 `else` 分支不介入"的负向形状。

#### A5 lambda 带默认参数

- **实测**：`f = lambda x, y=2: x*y` → 空输出；`lambda x: x*x` 逐行一致。
- **站点**：`src/frontend/parser/expr.rs:675-702`（`parse_lambda`）。它**自己手写**了一个参数循环：

  ```rust
  let (rest, name) = ws(parse_ident).parse(cur)?;   // 只收裸标识符
  ...
  } else { ... return Err(nom::Err::Error(...)); }  // expr.rs:697
  ```

  收完 `y` 后流里剩 `=2: x*y`，既不是 `,` 也不是 `:` ⇒ 在 `:697` 硬失败。
  与之对照：`def` 的参数解析在 `src/frontend/parser/top_level.rs:38 parse_param_full`，
  **那里支持默认值**（`:110`／`:129`）。`|x|` 闭包（`expr.rs:715`）同样只收裸标识符。
- **方案**：把 `parse_param_full` 提升为 `pub(crate)` 并让 `parse_lambda` 调用它，
  消掉三份各自实现（`def`／`lambda`／`|x|`）；默认值要一路带到
  `AstNode::Closure` 的参数字段，再由 `src/middle/mir/gen/lower_closure.rs` 接。
  如果一次性改三处风险太大，最小步是只在 `parse_lambda` 的循环里加"可选 `= 表达式`"
  并**先丢弃默认值**——但那样 `f(1)` 会运行期报参数不够，属于把静默错改成明确报错，
  可接受，**前提是台账与注释都写清"默认值尚未生效"**。本文推荐做全，不推荐半修。
- **风险**：AST 字段新增会波及闭包降级与调用点填参（`call_dispatch.rs:2801-2946` 的 `fill`
  位置绑定），这条链上历史有批次 752（`*args` 位置收集）与批次 800（闭包体丢体）两次事故。

### 二档（4 条）

#### A3 仅位置参数标记 `/` 与 A4 仅关键字参数标记 `*`

- **实测**：`def k(a, /, b): …` 与 `def h(a, *, flag): …`（含带默认值）都空输出。
- **站点**：`src/frontend/parser/top_level.rs:134`
  `alt((parse_self, parse_star, parse_kw_param, parse_regular)).parse(input)`，
  参数列表在 `:262-269` 用 `separated_list0` 组装。
  - `parse_star`（`:89-102`）要求 `*` 之后**必须**跟标识符（`:93 ws(parse_ident)`），
    所以裸 `*` 不匹配；
  - `parse_regular`（`:106-113`）只收标识符，裸 `/` 不匹配；
  - 两者都是：`separated_list0` 收到 `a` 就停，`delimited` 接着要 `)` 而流里是 `,` ⇒ 整个 `def` 失败。
- **方案**：`parse_param_full` 的返回值现在是 `(name, marker, default)` 三元组
  （见 `:100` 的 `parse_star` 构造），天然可以承载"这里有一个 `/` 或 `*` 分隔位"：
  ① 在 `alt` 里加两个"标记项"分支，返回 `("", "/", None)`／`("", "*", None)` 这样的哨兵；
  ② 在 `def` 的返回类型标注处（`:283-287` 是 `->`）旁边把标记存进 AST 的函数节点；
  ③ 语义侧：仅关键字 ⇒ 位置实参数量不得超过 `/` 之前的个数；仅位置 ⇒ `/` 之前的名字不接受
  `name=` 传参。落点在 `call_dispatch.rs:2801-2946` 的 `fill`（同文件已有 `*args`／`**kwargs`
  的位置绑定逻辑可参照，批次 752 的产物）。
- **分步**：A3/A4 拆两批做，先做 `*`（只关键字，语义更简单、现有 `parse_star` 已经认识 `*`），
  再做 `/`。**第一批只做到"能解析、按普通参数处理"**，把"传参方式错了要报错"留第二批，
  并在用例里明确写"当前只验证能跑通"，避免把半成品当完成。

#### A6 `try/except/else:` 的 `else` 分支

- **实测**：空输出。`try/except` 有读数；`try/finally` 能解析但体不执行（那是 C11，另一条链）。
- **站点**：`src/frontend/parser/stmt.rs:1436 parse_try_stmt`。except 循环在 `:1446-1484`，
  `:1449` 见不到 `except` 就 `break`，然后**只**探测 `finally`（`:1497-1514`），
  全程没有一处读 `else` 关键字；`:1515` 之后直接返回。残留的 `else {…}` 回到
  `parse_stmt` 的分派表（`stmt.rs:1849-1889`）无处可去 ⇒ 所在块整体丢弃。
  补充结构事实：这里**没有 try 的 AST 节点**，是 parser 内的纯脱糖
  （`:1269` 注释 `desugars (parser-only, no AST change)`），`:1593` 返回 `AstNode::Block`。
- **方案**：与既脱糖风格一致的最小改动：在 except 循环之后、`finally` 探测之前加
  `else` 分支识别，并把该分支体包成"只有异常状态码为 0 才执行"的条件块
  （`:1572` 已经有一个用于 handler 的 `else_branch` 概念，`:1401 zeta_try_frame` 提供了
  运行时帧；`e = zeta_last_error()` 的取码方式见 `:1573-1579`）。
  即：**复用现有的 `zeta_last_error()==0` 作条件**，不新增 AST 节点。
- **验收**：`syn60b_try_else`（无异常时 `else` 打印、有异常时不打印，两枚函数）。
- **注意**：`else` 与 C11 的 `finally` 是同一条脱糖链的两个缺口，建议同批或相邻批处理，
  否则修完 `else` 后 `try/except/else/finally` 全写仍然只有一半对，用例不好写。

#### A7 `match` 的序列模式 `case ['quit']:`

- **实测**：空输出。`case 1:`、`case _:` 逐行一致。
- **站点**：Python 的 `case` 臂由 `src/frontend/parser/expr.rs:3480 parse_match_arm` 处理，
  模式本体交给 `expr.rs:3489 → src/frontend/parser/pattern.rs:20-72 parse_pattern`。
  该函数已有的分支：裸 `_`（`:28`）、`&pat`（`:42`）、元组 `(…)`（`:44`／`:127`）、
  `x@pat`（`:52`／`:292`）、结构体/路径（`:54`／`:141`）、范围（`:56`／`:275`）、
  或模式 `|`（`:58`／`:307` + `:79 many0`）、数字字面量（`:60`）、字符串字面量（`:66`）、
  `true/false`（`:68-69`）、变量（`:71`）。**没有 `[` 开头的序列模式分支**；
  以 `[` 开头时所有分支都失败（`:71` 的 `parse_ident` 直接拒），错误按 `pattern.rs:39-41`
  注释描述的机制向外传播成整段丢弃。
- **方案**：三步。① `src/frontend/ast.rs` 的模式节点区（`enum AstNode` 在 `:28`，
  模式变体集中在 `:247-372`）加一个序列模式变体（元素为子模式列表）；
  ② `pattern.rs` 加 `[ pat, pat, … ]`（含 `*rest` 尾元素是可选的第二步）分支，
  放在 `:66` 字符串分支之前、按首字符 `[` 判定；
  ③ MIR 降级：`src/middle/mir/gen.rs` 里参照元组模式 `(…)` 现有的降级，
  序列模式＝长度相等 + 逐元素子模式匹配。
- **风险**：③ 落在 `gen.rs`——主车道在制面（用户裁定"尽量绕开这个 gen.rs"）。
  **本条在拿到 gen.rs 工作面许可之前不动**；可以先做①②（能解析、匹配失败即明确报错），
  把③留在同一编号的余项里。

### 三档（1 条）

#### A9 `yield`（生成器函数）

- **实测**：`def g(): yield 1; yield 2` → 编译给 W1004 警告（"print 被拆成独立语句"那一族），
  `list(g())` 实得 `2`，期望 `[1, 2]`。
- **站点与事实**：`yield` **根本不是关键字**——关键字表在 `src/frontend/parser/parser.rs:82-87`
  （含 `return`／`break`／`continue`／`match`／`defer`），里面没有 `yield`；
  `src/frontend/ast.rs:28` 的 `enum AstNode` 没有 Yield 变体；MIR 侧无降级。
  于是 `yield` 被当成一个普通变量名（`AstNode::Var("yield")`），后面的 `1` 变成下一条语句，
  触发 `warn_if_swallowed_prefix`（判定在 `src/frontend/parser/stmt.rs:775`，
  打印在 `:807-810`，调用点 `:59` 与 `:757`）——这就是 W1004 与错值的来源。
- **方案（这是本文里唯一需要新语义的一条，建议单列一期）**：
  1. **最小可用**：把 `yield` 加进关键字表（`parser.rs:82`），函数体含 `yield` 时
     按**急切收集**处理——把 `yield expr` 脱糖成"往函数私有的收集数组里 push"，
     函数返回该数组。这与现有推导式的 `__collect__` 急切脱糖
     （`expr.rs:1276`、`call_dispatch.rs:4964 zeta_collect_vec_n`）是同一思路，
     能覆盖 `list(g())`／`for x in g()`／`sum(g())` 常见三种消费形态。
     **明确代价**：语义不是惰性的（`next(g())`、无限生成器不成立），
     必须在函数注释、用例头、本文余项三处写清"急切收集，非惰性"。
  2. **惰性生成器**：需要生成器的运行时表示（挂起帧），跨 AST/MIR/运行期三层，
     且与 C11、A6 的 setjmp 帧机制相互影响。本文不建议在当前排期里做，
     列为独立议题请用户裁定。
- **不做**：把 `yield` 继续当普通标识符。现在的行为是"静默错值 + 一条无关的 W1004"，
  加关键字后至少能给出"该函数没有 yield 位点"这类明确错误。

---

## 3. B 组：运行期明确报"这个方法名不认识"

三条都有指名（`Unhandled exception: code=1`），不是静默错，因此优先级按"改动面 × 是否需授权"排。

### B2 集合并集 `s.union(t)` —— **建议第一个真修**（零授权、零新增原语）

- **实测**：`dynamic receiver has no member [dynamic]i64::union`。
- **事实**：C 原语**已经存在**——`runtime/py_additions.c:1872 py_vec_union(a, b, elem_is_str)`
  （批次 879 落的产品），但全仓只有一个调用点：`|` 运算符
  `src/middle/mir/gen/call_binary.rs:784`。**没有任何一处把 `.union(` 当方法名接住**
  （`src/middle/mir/gen/` 里 grep `"union"` 只命中 `call_class.rs:102` 的单元测试）。
- **站点（要照抄的配方）**：批次 807 修 `intersection` 的三处——
  ① MIR 方法臂 `src/middle/mir/gen/call_set.rs:107-135`（守卫
  `method=="intersection" && set_like_receiver(receiver_ty) && set_intersection_ok(...)`，
  算 `elem_is_str` 标志、发 `MirStmt::Call{func:"py_vec_intersect", args:[recv,arg,flag]}`、
  返回型继承接收者）；② 分类表入口 `call_class.rs:46 "intersection" => CallClass::SetIntersection`；
  ③ C 侧原语（本次**不需要**）。`pylib/registry.txt` 在这条配方里没参与。
- **方案**：`call_set.rs` 加一个与 `:107-135` 同形的 `union` 臂，`func` 填 `py_vec_union`，
  `call_class.rs:46` 加 `"union" => CallClass::SetUnion`。**只改 `src/middle`，不碰 `runtime/*.c`，
  不碰 registry，不碰 `gen.rs`。**
- **顺手能记的量**：`difference`／`issubset`／`issuperset`／`isdisjoint`／`symmetric_difference`
  在 `src/middle`、`runtime/*.c`、`pylib` 三处**都查不到**（本文实查）。
  本批**只做 `union`**：其余五条需要新 C 原语＝要授权，别混进这一批。
- **验收**：`cargo test -p zetac --lib call_set` + `syn60b_set_union`（`s|t` 与 `s.union(t)` 同文件，
  证明两条路读到同一个值）。

### B1 列表按下标删 `del xs[0]`

- **实测**：`del d[k]` 逐行一致，`del xs[0]` 抛 `dynamic receiver has no member [dynamic]i64::__delitem__`。
- **链路（四处站点）**：
  1. 脱糖：`src/frontend/parser/stmt.rs:907`（注释 `V1 only del obj[key] → obj.__delitem__(key)`），
     实际构造在 `:944-948`；
  2. 字典臂（走得通）：`src/middle/mir/gen/call_dispatch.rs:5077-5116`，
     守卫 `receiver_ty … Type::is_map()`，`("__delitem__", 2) => Some("zeta_map_pop")`（`:5116`），
     C 侧 `runtime/py_additions.c:4746 zeta_map_pop`；
  3. **列表臂缺失**：`call_dispatch.rs:5772-5795` 的动态数组方法表只有
     `push/len/unique/nunique/sum/strftime`；其余落到 `:5791-5792` 的兜底
     `format!("{}::{}", rty_name, method)`，而 `Type::DynamicArray(inner)` 的名字来自
     `src/middle/types/mod.rs:812` `[dynamic]{}::…` ⇒ 拼出编译器不认识的方法名；
  4. 兜底怎么变成交错：`src/backend/codegen/codegen.rs:3386` 把它绑到抛停桩
     （白名单 `dyn_runtime_bound` 在 `:3499-3510`，只有 `map/pct_change/abs/nth`），
     桩在 `runtime/unavailable_stubs.c:92 zt_dyn_member_missing` 打印观测到的那行，然后
     `zeta_raise(1)`。
- **方案**：两步，且**必须同批**（只做第①步会得到"能编过但什么都不删"的静默错，
  那是把明确报错换成更坏的东西）。
  ① `runtime/py_additions.c` 新增 `py_vec_delitem(vec, i)`（读头 `h[0]=cap, h[1]=len`，
  负索引按 `+len` 回卷，越界**响亮报错**，成员前移）——**要用户授权**（改 `runtime/*.c`）；
  ② `call_dispatch.rs:5772-5795` 的表里加 `("__delitem__", 2)` → `py_vec_delitem`，
  并把该符号加进 `pylib/registry.txt` 或显式 extern 声明（参照 §5 的第 18 档风险）。
- **验收**：`syn60b_del_index`（删首、删尾、删中间三枚函数）+ 越界负向用例（`// expect-error`）。

### B3 `sorted(元组列表, key=…)` —— 与 C13 是同一条链

- **实测**：`sorted(pairs, key=lambda p: p[1])`（`pairs=[('a',3),('b',1),('c',2)]`）抛
  ``map_get` was called on a value that is not a dict`。
- **链路**：
  1. 元组**没有独立的值类型**：`src/middle/mir/gen/call_expr_lit.rs:326-368 lower_tuple_expr`
     造 `MirExpr::StackArray{…}`，型记 `Type::Tuple` 并登记进 `tuple_slots`（`:366-367`）；
     `tuple(xs)` 在动态槽上是恒等（`call_dispatch.rs:2272-2283`，注释 "A tuple IS a dynamic
     array in the value model"）；运行期没有 `tuple_get`/`zeta_tuple` 下标原语
     （只有 `zeta_tuple_repr`，`py_additions.c:1514`）；
  2. lambda 里的 `p` 是无型 I64 槽 ⇒ 拿不到 `Type::Tuple`，因此
     `src/middle/mir/gen/call_subscript.rs:149-224` 的快速路径（`tuple_slots`/`pair_slots`/
     `Named("tuple")` → `stack_array_get`）不匹配；
  3. 落到 `call_subscript.rs:496-510` 发 `zeta_dyn_getitem`；
  4. `runtime/py_additions.c:5116-5137 zeta_dyn_getitem`：元组是栈数组指针、**没有 GC 头**，
     `GC_base(base-16)==base-16`（`:5118`）判不出来，文本闸门（`:5136`）也不成立，
     于是 `:5137 return map_get(base, key);` ⇒ `runtime/tokio_runtime_stub.c:337-356 zt_map_not_a_map`
     打印观测到的那行。
- **方案（编译侧优先，运行期另批）**：
  ① **最小且不碰运行期**：在 `sorted(key=…)` 的降级里把 key 函数的形参按**列表元素型**定型，
  使 `p[1]` 走进 `call_subscript.rs:149-224` 的 `stack_array_get` 臂。落点：
  `src/middle/mir/gen/call_builtin.rs:383`（`sorted` 的 2/3 参分支，符号选择在 `:722-723`）
  与 `lower_closure.rs` 的形参定型。这与批次 10001/813 修过的"调用点目的槽丢型"是同一族方法。
  ② **根修要授权**：给元组一个带 GC 头的堆表示，或在 `zeta_dyn_getitem` 前加"栈数组"闸门。
  ② 影响面大（`zeta_dyn_getitem` 在 40 文件语料里有 938 个调用点，注释里写明
  `:5112-5114`），本文不建议与①同批。
- **同链的 C13**：`sorted(dict.items())` 顺序整个反过来。已读到的实现是
  `runtime/py_additions.c:449/463/477 zt_map_sorted + qsort` 与 `:4260 zeta_sorted_vec_len`
  （按 i64 槽插入排序）／`:4287 zeta_sorted_vec_len_str`（按内容比较），
  而 `:4280-4285` 的注释自己承认通用路径"sorts POINTER values"。
  **这条要改比较器＝改 `runtime/*.c`＝要授权**；两个候选方向（改 C 比较器 vs
  把语义搬到 `pylib` 库面并让 `call_builtin.rs` 不再截获 `sorted`）
  仍等用户裁定（§7 第 2 项）。

---

## 4. C 组：认了但值/呈现不对（16 条 → 6 条链）

### 链 1：容器与元组的呈现（C4、C16，牵连 C8、C14）

- **实测**：2 元组打成 `(1, 2)` 正确；1/3/混合元素元组、以及**函数返回的元组**打成堆地址
  （`print(t)` → `4300165072`）；`t[1:3]`、`tuple([4,5])` 降级成列表形状；
  `(1,2)+(3,)` → 地址；`tup()` 返回容器再打印 → 地址。
- **站点**：`src/middle/mir/gen/call_print.rs:544-555` 只对 **arity==2** 有特例：

  ```rust
  if let Some(Type::Tuple(ts)) = self.type_map.get(arg_id).cloned() {
      if ts.len() == 2 { ... "py_print_pair" ... continue; }
  }
  ```

  其余全部落到 `:681-683` 的兜底 `_ => { if is_last { "println_i64" } else { "print_i64" } }`
  ⇒ 打句柄值＝堆地址。通用渲染器 `zeta_tuple_repr`（`runtime/py_additions.c:1514`）**只**接在
  `to_string` 路径上（`src/middle/mir/gen.rs:1917-1943`），打印分派从来不看它。
  函数返回的元组：`src/middle/mir/gen/stmt_return.rs:29-38` 把返回值降成 dynarray 句柄
  （临时型 `DynamicArray`，`:60` 再把 `Tuple` 贴回），当调用点目的槽没带 `Type::Tuple` 时
  打印又走 `println_i64`。**"Tuple 型在调用边界具体在哪一步丢"这一格：站点未定位**（本文只
  确认了症状点 `call_print.rs:681`）。
- **方案**：
  ① 打印侧通用化——`call_print.rs` 的 Tuple 分支去掉 `ts.len()==2` 限制，
  任意长度走 `zeta_tuple_repr`（已有 C 函数，**不需要新原语**），
  拿到文本后走 `println_str`；同时把 `to_string` 路径 `gen.rs:1917-1943` 的调用方式作为参照。
  ② 返回槽保住 `Tuple` 型——这一半要先做定位（方法见 §8），拿到站点再改。
- **风险**：`call_print.rs` 是打印链，**改动面最宽**（差分语料 3026 枚几乎每枚都调 print）。
  需要用户授权（§7 第 3 项），并且必须按 `--bless` 流程走："基线只升不降"，
  呈现变化会让成批用例的期望输出搬家，**不许静默删用例**（`refactor.md` ⑨ 的裁定）。
- **C8／C14 归在这一链的理由与边界**：`print('x', end='!')` 单写正确、复合形态出 `x!<null>`；
  循环变量在 `for ch in 'ab': print(ch, end=' ')` 后多出 `<null>`。两者都带
  `<null>` 字形 ⇒ 与"最后一项用 `println_*`、非末项用 `print_*`"这层选择（`:681-683`）
  同形。**但触发条件本文未定位**（单构造绿、复合红），只登记读数，见 §8。

### 链 2：集合不去重（C1）

- **实测**：`{1,2,2,3}` 的 `len` 得 4（期望 3）；`{True,1}` 得 2；集合推导
  `{w[0] for w in words}` 得 `['a','a','b']`。`s.add(重复)` 表现正确。
- **站点**：`src/frontend/parser/expr.rs:1071-1075` 的注释就是决定本身：

  ```rust
  // PY-A: `{a, b, c}` is a SET literal, not a dict. Zeta has no set type, and
  // the existing set comprehension already lowers to a list, so a set literal
  // does the same (duplicates are NOT collapsed — same accepted limitation as
  // setcomp).
  ```

  `:1115` 返回 `AstNode::ArrayLit(items)`，**从不调用去重原语**。
  去重能力已经存在：`add()` 走 `src/middle/mir/gen/call_set.rs:24-34` 发
  `py_vec_add_unique`（`runtime/py_additions.c:1412`）。
- **方案**：集合字面量/集合推导的降级里，把"一次性 `ArrayLit`"改成
  "建空 vec + 逐元素 `py_vec_add_unique`"——**复用已有原语，零 `runtime/*.c` 改动**。
  落点两处：`src/middle/mir/gen/call_expr_lit.rs` 的数组字面量臂（要区分是 set 语义的入口）
  与推导式的 `__collect__` 出口（`call_dispatch.rs:4964`）。
  因为 parser 层把 set 字面量已经塌成 `ArrayLit`（`:1115`），**需要先给 set 语义留一个可辨识的
  形状**（最小做法：在 AST 侧保留一个 `Call{method:"__set_lit__"}` 之类的标记节点，
  而不是继续用 `ArrayLit`）——这一小步是本条的真正成本。
- **验收**：`syn60b_set_dedup`（字面量/推导/`add` 三种形态同文件，三者 `len` 必须一字相同）。

### 链 3：`None` 与类型对象（C2、C3）

- **C2 实测**：`d.get(缺失键)` 得 `0`，期望 `None`。
  **站点**：`call_dispatch.rs:4990-5005` 把 `d.get(k)` 发成 `MirStmt::DictGet`，
  注释原样写着 `Python d.get(k) (missing key → 0)`；运行期
  `runtime/py_additions.c:4065`：`if (!dflt) return zt_cell_make(ZJ_INT, 0);`。
  而 `None` 在运行期**只是呈现**——`call_print.rs:18-29` 用标签 8 打字符串 `"None"`，
  没有任何 `None` 值对象。于是"缺键返回 0"和"缺键返回 None"在下游不可区分。
- **C3 实测**：`type(1) is int` 得 `False`（`isinstance(1, int)` 正确）。
  **站点两处**：① `is` 在 parser 里被折成 `==`——
  `src/frontend/parser/expr.rs:2507-2509` `found_op = Some("==".to_string())`；
  ② `type(x)` 折叠成**类型名字符串**——`call_dispatch.rs:3209-3224`
  （注释：`fold it to the Python type NAME as a string… "type(x) is int"-style code… works`）。
  于是 `"int" == int` 是字符串与类名格的按内容比较 ⇒ `False`；
  `isinstance` 走静态类型检查（`call_dispatch.rs:2171`）⇒ `True`。
- **方案（分两步，第 1 步小、第 2 步大）**：
  1. **小步**：在 `expr.rs` 的比较层识别"`is` + 右操作数是内置类型名（`int/float/str/bool/list/dict`）"
     这一形状，直接折成 `isinstance(左, 右)` 的既有落点（`call_dispatch.rs:2171`）。
     改动在 parser 一处分支；风险是 `x is y`（对象同一性）与 `x is int` 形状要分得开——
     用"右操作数是这个名字表里的裸标识符"作守卫，并把不在表内的 `is` 保持现状。
  2. **大步（另议，需授权）**：引入运行期 `None` 单例，让 `dict.get` 缺键、
     无返回值函数、`is None` 三处共用。这会改 §1 里 ABI.md §3.3 的打包合同
     （值附带类型标记那一系列），不是加一个分支的事。
     **在此之前，`d.get(缺)` 的 `0` 只能做成"现状锁"用例，不能声称修好。**
- **验收**：小步 `syn60b_type_is_int`（`type(v) is T` 五型 + `x is y` 同一性负向形状各一枚）。

### 链 4：特殊方法、类属性、异常对象、finally（C9、C10、C12、C11）

- **C9（`__str__`/`__eq__` 不被咨询）**：`print(a)` 打地址、`a == b` 恒 `False`、`len(a)` 正确。
  **站点**：dunder 派发只有两处存在——`__len__`
  （`src/middle/mir/gen/call_len.rs:44-46 qualified_method_candidate(n, "__len__")`）与
  `__setitem__`；全 `src/middle` grep `__str__`／`__eq__` **没有任何派发点**
  （`call_binary.rs:1361-1365` 的 `map__eq` 是字典专用）。结构体句柄打印落
  `call_print.rs:681`（`println_i64`），`==` 落 i64 句柄比较。
  **方案**：仿 `call_len.rs:44-46` 的形状，为打印（`call_print.rs` 的类型分派入口）与
  `==`（`call_binary.rs`）**各加一个 dunder 咨询臂**：先查接收者类里有没有
  `__str__`/`__eq__`，有就发限定名调用，没有保持现状。打印那一半与链 1 同面 ⇒ 要同一份授权。
- **C10（写实例属性落到类格）**：`c.shared = 9` 之后 `print(c.shared, C.shared)` 得 `1 1`（期望 `9 1`）。
  **站点两侧不对称**：写侧 `src/middle/mir/gen/stmt_assign.rs:14-26` **只有基名是类名时**才写全局
  `{Class}__{attr}`，实例写走 `StructFieldStore`（`:436-443`）；
  读侧 `src/middle/mir/gen/call_field.rs:54-64`：

  ```rust
  let gname = format!("{}__{}", tn, field);
  ...
  if !is_struct_field && self.module_globals.contains(&gname) {
      return self.lower_expr(&AstNode::Var(gname));
  }
  ```

  类体里的 `shared = 1` 被脱糖成全局 `C__shared`（`src/frontend/parser/top_level.rs:1063-1070`），
  而 `shared` 不在结构体字段表里 ⇒ `c.shared` 读回来永远是类值。
  **方案**：读侧改成"实例属性优先"——先按结构体字段读，字段确实不存在才落类全局。
  真正的形状是**类属性也要能按实例名读**这一档：需要一个类属性快照随实例结构体走的设计，
  属结构级改动（`@staticmethod` 返回 str 打成地址那一格也在这一档，本文未定位）。
- **C12（异常没有对象）**：`except KeyError as e` 之后 `type(e).__name__` 得 `0`、
  `print(e)` 打地址。**站点**：绑定在 `src/frontend/parser/stmt.rs:1573-1579`
  （`e = zeta_last_error()`），运行期 `runtime/py_additions.c:3550-3552` 返回的只是
  `zeta_raise(code)` 的**小整数码**（`:3539`），全 `py_additions.c` 里没有异常类/名字。
  **方案**：给异常加最小对象（码 + 类名 + 消息，一个 GC 块），
  `zeta_last_error` 之外加 `zeta_last_error_name`，绑定改用它。跨 parser/运行期两面，
  **要授权**（`runtime/*.c`）。可与 A6 的 `else` 分支同批（同一条 setjmp 脱糖链）。
- **C11（`finally` 体不执行）**：`fin\n5` 实得 `5`。**站点**：`finally` 体确实发射了，
  但是作为 try 块之后的**兄弟语句**——`src/frontend/parser/stmt.rs:1591-1592`
  `out.extend(finally_body);`。当 try 体里有 `return` 时，后端注释直接写明后果：
  `src/backend/codegen/codegen.rs:3940-3947`（"a `return` inside the try leaves those
  statements in an already-terminated basic block … park them in a fresh
  predecessor-less block"，`ensure_emittable_block` 在 `:3956-3964`）⇒ 落进不可达块，永不执行。
  **方案**：`finally` 不能靠直线代码位置表达，必须在**每个出口**（正常结束、`return`、
  异常落点）之前发射 `finally` 体。落点 = `parse_try_stmt` 的脱糖改造（`stmt.rs:1436-1600`）
  加 `zeta_try_frame`（`:1401`），与 A6/C12 是同一条链的三个缺口。
  **建议 A6 + C11 + C12 合并成一批"try 语句三缺口"**，否则中间态没有可写的验收用例。

### 链 5：格式规格（C6、C7）

- **C7 实测**：`f"{s!r}"` 打出原样 `s!r`（期望 `'hi'`）。
  **站点**：`src/frontend/parser/expr.rs:1566-1572` f-string 分段：

  ```rust
  let (expr_text, spec) = match split_format_spec(inner) { ... }; // 只按 ':' 切
  let base_expr = match parse_full_expr(expr_text) {
      Ok((rem, e)) if rem.trim().is_empty() => e,
      _ => AstNode::StringLit(expr_text.to_string()),   // 解析不了就当字符串字面量
  ```

  `split_format_spec`（`:1608-1633`）**只认 `:`**，`!r/!s/!a` 从不识别；
  `s!r` 解析失败后按 `:1572` 退成字符串字面量——这就是"打出 `!r`"的机制。
  **方案**：`split_format_spec` 同时按 `:` 与 `!`（且 `!` 后跟 `s/r/a` 且再跟 `:` 或段尾）切段，
  把转换标记带进段落；`!r`/`!s` 的实现可复用已有的 repr/str 落点
  （`to_string` 路径 `gen.rs:1917-1943`；`!a` 不做，登记余项）。
  **退成字符串字面量这一条本身也该改**：解析不了的分段应当**明确报错**，
  而不是静默打原文（本仓库红线"宁可响亮失败"）。
- **C6 实测**：`"{:>6}|{:<6}".format(a, b)` 的宽度/对齐**全丢**；f-string `{y:^9.3}` 得 `3.14`
  （期望 `  3.142  `）。而 `:>6`、`:.2f`、`:05` 是对的。
  **站点（前半）**：`.format` 的规格是**明知故丢**——`src/middle/mir/gen.rs:3060-3071`：

  ```rust
  let (field, _had_spec) = match inner.find([':', '!']) { ... };
  if _had_spec { ... eprintln!("warning: PY-A: str.format format-specs ({:.2f} / {:<0} / {!r}) \
      are ignored — the value is kept, the presentation is not"); }
  ```

  同一次 `find([':', '!'])` 顺带把 `!r` 剥掉。
  **f-string 侧**规格是**真接的**：`call_builtin.rs:186-197` → `py_fmt_f64/i64/str`，
  C 侧 `zt_parse_spec`（`py_additions.c:814-837`）解析 fill/align/width/sign/`0`/`,`/精度/类型，
  补齐走 `zt_fmt_pad`。
  **`{y:^9.3}` 为什么得 `3.14`：站点未定位**（按已实现的解析器应当产出 `  3.142  `）；
  最可能的分岔是 `call_builtin.rs:191-197` 的类型通道——非 F64 落 `py_fmt_i64`/`py_fmt_str`，
  而 `py_fmt_str`（`py_additions.c:1036-1042`）按**字符**截断。定位方法见 §8。
- **方案**：`.format` 复用 f-string 已经通的 `__fmtspec__` 通道
  （即把 `gen.rs:3060-3071` 丢弃规格改成转投 `call_builtin.rs:186-197` 同一发射形状）。
  改完要顺手摘掉那条 `OnceLock` 警告，并把"忽略规格"的现状用例（如果有）改期望。
- **验收**：`syn60b_format_spec_width`、`syn60b_format_align`、`syn60b_fstr_repr_flag` 三枚；
  每枚都要带一枚"规格为空"的对照，防止把默认呈现改动误判成修好。

### 链 6：星号解包（C5）—— 本批新定位到站点的一条

- **实测（本文重跑，`/tmp/b10061/c5a.z`、`c5b.z`）**：
  `ys = [*xs, 4]`（`xs=[1,2,3]`）→ `len(ys)` 得 `2`、`ys` 打 `[1, 4]`（期望 `4`／`[1, 2, 3, 4]`）；
  `first, *rest = [1, 2, 3]` → `first` 得 `1` 正确、`rest` 得 `2`（期望 `[2, 3]`）。
  两枚都**没有 W1002**（不是解析拒绝），c5a 的 MIR 里有确凿的一格：

  ```
  22: Deref { addr_id: 21, pointee_width: 8 }
  ```

  且 `ys` 只发了两次 `vec_push`——与"取到 `xs[0]` 再补 `4`"完全对应。
- **站点与根因**：`*` 在 `src/frontend/parser/expr.rs:1379` 是**Rust 的指针解引用**运算符
  （表里 `&mut & - *` 四个），Python 的"可迭代解包"没有任何表示：
  - `[*xs, 4]`：元素 `*xs` 解成 `Deref{pointee_width:8}` ⇒ 读句柄首个槽＝`xs[0]`。
  - `first, *rest = …`：LHS 元素也用 `parse_unary` 解析
    （`src/frontend/parser/stmt.rs:504` 与 `:530`），`:569/:579` 组装成
    `AstNode::Tuple(items)` → `Assign`；降级侧 `src/middle/mir/gen/stmt_assign.rs:125-167`
    **只认 `AstNode::Var` 目标**（`:126 if let AstNode::Var(name) = l`），
    `UnaryOp` 目标那一格被静默跳过、没有任何诊断。**`rest` 读出 `2` 的具体来源本文未定位**（§8）。
- **方案**：新增解包形状，两侧各一处。
  ① 列表字面量里的 `*expr`：在 `src/frontend/parser/expr.rs` 的数组字面量元素解析里
  优先识别"星号 + 可迭代"并发一个显式的展开节点（字典侧已有同类先例：
  `expr.rs:1027-1038 zeta_dict_spread`、`call_dict.rs:38`），
  降级侧按元素逐个 push（复用 `vec_push`）。
  ② 赋值目标里的 `*name`：`stmt.rs:504/530` 处保留星号信息（不要让它折进 `parse_unary`），
  `stmt_assign.rs:114-168` 的解构臂加"星号目标＝收集剩余元素成 vec"分支，
  同时把 `:126` 的静默跳过改成明确报错（未知目标形状应当报错，不该继续丢）。
- **风险**：`*` 一名两用（解引用 vs 解包）必须在 parser 早期分岔；
  分岔依据可以是"当前是否在列表字面量元素/赋值目标位置"——两处位置都已有独立的解析入口，
  不需要全局上下文。**动身前必须报备**：`stmt.rs`、`expr.rs`、`stmt_assign.rs` 三个文件
  与主车道在制的 parser 面重叠。

### C15（`nonlocal` 累加在复合形态里得 `0`）

单构造绿、同文件后面还有容器打印时红。**触发条件未定位**，`nonlocal` 的站点本文未查。
登记在 §8，不与链 1 混写（同形不等于同因）。

---

## 5. D 组：跑出的二进制停住 —— 三条读数：D2 已读到链，D1/D3 挂死点**未定位**

- **挂死的三枚（夹具原文照抄，均取自 `/tmp/b10060/`）**：
  - **D1** `iso3/builtin_zip.z`：`print(list(zip([1, 2], 'ab')))`
  - **D2** `iso5/sum_gen.z`：`def seq(n):` / `    for i in range(n):` / `        yield i * i` /
    `print(sum(seq(4)))`（同一函数体的 `probes/fn_generator_yield.z` 多一行
    `print(list(seq(3)))`，也挂）。
    **注意**：本轮**没有**测过生成器表达式写法 `sum(x*x for x in [1,2,3])`——
    在册语料里只有列表推导形态 `sum([i * i for i in range(4)])`
    （`tests/diff/cases/control_list_comp_sum.dcase`），裸 genexp 传 `sum` 的形状从未入库。
  - **D3** 三枚同族：`iso5/pair_slice_tuple.z`（`print(t[1:3], tuple([4, 5]))`）、
    `iso5/pair_concat_tuple.z`（`print((1,) + t, tuple([4, 5]))`）、
    `iso4/t_line2.z`（三项合写），`t = (1, 2, 3)`。
  三族都进入 `ps -o stat` 为 `U`/`UNE` 的状态，`timeout` 与 `kill -9` 都收不掉
  （#20006 同形）。本文复查时实测：`/tmp/b10060/` 下这类进程**现存 12 颗**
  （`ps -eo pid,stat,etime,comm | grep /tmp/b10060/`，2026-10-06 读数：
  `probes/bi_core` ×3（pid 3633、6785、11043）、`probes/cont_tuple_literal` ×2
  （7899、12011）、`probes/fn_generator_yield` ×2（9224、13184）、`iso3/builtin_zip`（24875）、
  `iso4/t_line2`（36713）、`iso5/pair_slice_tuple`（38130）、`iso5/pair_concat_tuple`（38486）、
  `iso5/sum_gen`（38815）——D1/D2/D3 三枚各有一颗在列，取栈时优先用这三颗；
  进程号会被回收，动手前要先复取）。
- **不挂的对照读数（同一批实测，本文复查逐枚读出 stdout）**：这一组是定界的关键，
  每枚都给 zeta 实得与 CPython 期望。
  | 对照夹具 | zeta 实得 | CPython | 说明 |
  |---|---|---|---|
  | `iso3/tup_call.z` `print(tuple([4, 5]))` | `[4, 5]` | `(4, 5)` | `tuple()` 单写只错呈现，不挂 |
  | `iso5/pair_slice_concat.z` `print(t[1:3], (1,) + t)` | `[2, 3] 8697335712` | `(2, 3) (1, 1, 2, 3)` | 不含 `tuple()`，第二个参数打成句柄数字，不挂 |
  | `iso/gen_yield.z`、`iso4/list_of_gen.z` `print(list(g()))` | `2` | `[1, 2]` | 两元素 `yield` 生成器过 `list()` 只错值，不挂 |
  | `iso5/sum_list.z` `print(sum([0, 1, 4]))` | `5` | `5` | 列表形态 `sum` 正确 |
  | `iso5/max_gen.z` `print(max(seq(4)))` | `0` | `9` | **与 D2 同一函数体、只差 `max`/`sum`** ⇒ 不挂但错值 |
  ⇒ 从读数能收窄出两条形状规律（**只是规律，不是机制**）：
  ① D3 的挂点需要 `tuple([4, 5])` **和**同一条 `print` 里的第二个容器表达式
  （`t[1:3]` 或 `(1,)+t`）；两个因素各自单独出现都不挂。
  ② D2 的挂点跟在被 `sum(...)` 消费之后，同形状的 `max(...)` 不挂（得 `0`）。
- **本批留存的读数（本文复查）**：上述挂死夹具的 `run.out` 与 `run.err` **都是 0 字节**。
  `run.err` 为空有判据意义（对照 `probes/bi_sorted_key.run.err`＝运行期点名信息的写法，见 B3），
  说明**挂死前一条运行期诊断都没打出来**。`run.out` 为空**不能**说明第一行 print 没执行——
  stdout 重定向到文件是块缓冲，进程没正常退出就不落盘。
  另需记法上一条防误读：`/tmp/b10060/` 下"空 out ＋空 err"共 **25 项**，其中只有这 8 个名字
  有活着的挂死进程——其余是**解析截断**后正常退出的空输出（§1.2 那条纪律）。
  所以"两个文件都是 0 字节"不能当挂死判据，判据只有 `ps -o stat`。
- **三条链本文各自读了 MIR（`--dump-mir` 只编译不执行，零挂死风险；读数存
  `/tmp/b10061/d1.mir`／`d2.mir`／`d3.mir`）**，结论是**三条不同链，不能共用一条归因**：
  - **D1（`zip`）**：发射 `zeta_dynarray_new` → `vec_push`×2 → `py_zip(args:[4,11])`
    → `py_print_pairs(args:[2,13,14])`。`py_zip`（`runtime/py_additions.c:1050`）
    的 `:1056 base[0] = n ? n : 1;` ⇒ **cap=2、绕过 `zeta_dynarray_new` 的钳位**，
    这条在链上是实拍的。
  - **D2（`sum(生成器函数)`）**：发射 `seq_1(args:[5])` → **`zeta_sum_n(args:[3])`——只发 1 个实参**
    → `println_i64`；而 `seq` 的函数体里 `Return { val: IntLit(0) }`（`yield` 被丢掉，
    stderr 实拍 `warning: [W1004] …:3: \`yield\` became a stand-alone statement…`，见 A9）。
    C 侧 `zeta_sum_n(int64_t data, int64_t n)`（`py_additions.c:740`）＝
    `for (i=0;i<n;i++) acc += ((int64_t*)data)[i];`。
    **这条是三条里唯一本文能读完且缺陷可点名的**：少发的第二个参数 `n` 取调用约定的残值，
    而 `data` 此时是 `0` ⇒ 从地址 0 起按残值长度逐格读。
    发射点＝`src/middle/mir/gen/call_num.rs:287` 的
    `(_, SumSeq::LegacyBare) => ("zeta_sum_n", false, None)`；同表 `SumSeq::Dynamic` 那支
    用的是 1 参版 `zeta_sum_vec`（`:654`，`if (!data) return 0;` 自带空值保护，
    registry:488 在册）。
    **`zeta_sum_n` 在册（`pylib/registry.txt:513 X zeta_sum_n args=i64,i64 ret=i64`）
    ⇒ 这不是第 18 档猜签名，是发射点自己少发一个参数。**
  - **D3（`tuple([4,5])` 进多参数 `print`）**：发射只到 `zeta_dynarray_new` → `vec_push`×2
    → `print_i64` / `print_str` / `py_json_dumps_vec_typed` ——**全都走带钳位的正规构造器**，
    本节原先写的"拼接/切片构造器给 D3 cap=5"**在本枚 MIR 里找不到支撑，作废**。
    D3 的挂点本文未定位。
- **两条已实测存在、但与挂死的因果未证的站点**：
  1. 正规 vec 构造器**有钳位**：`runtime/py_additions.c:3636 zeta_dynarray_new`，
     `:3637 if (cap < 8) cap = 8;`。
  2. 绕过钳位直写头字的构造点确实存在（`:517`、`:1056`、`:1100`、`:2583`、`:2599`、
     `:2729`、`:2898` 一批 `base[0] = n ? n : 1;`），D1 命中的是 `py_zip` 那一处；
     `zeta_collect_vec_n`（`:4145`，`:4149 base[0] = len ? len : 8;`）**不在这三条链上**
     ——它属于裸生成器表达式（`expr.rs:2264 parse_call_genexp` → `:2297 "__collect__"` →
     `call_dispatch.rs:4964`），而那个写法本轮从未测过（见上面对 D2 的更正）。
  3. 消费侧闸门**不一致**（这条缺陷是真的，但与挂死的因果**未证**）：
     `zeta_dyn_getitem`（`:5116`）内联判据写作 `:5120 if (cap >= 8 && …)`，
     而同一文件的正式 vec 判形 `zt_dyn_vec_hdr`（`:5169-5190`）接受 `cap>=1` 并要求"紧块"
     （`:5188 if (cap < 8 && have > need + 16) return NULL;`，注释 `:5178-5187` 写明批次 301
     就是为短 vec 改的）。⇒ `zeta_dyn_getitem` 是**这一族里唯一还在用旧 `cap>=8` 闸门的入口**，
     短 vec 到它这里会漏下去，落到 `:5137 return map_get(base, key);`。
- **为什么"漏进 map_get 就自旋"这条推测不能当结论写**：`map_get`
  （`runtime/tokio_runtime_stub.c:435-446`）第一步是 `map_resolve(map0)`（`:436`），
  而 `map_resolve`（`:357-380`）已经在出口做形状检查——

  ```c
  if (!zt_map_is_json_handle(map) && !zt_map_cap_ok(w0))
      zt_map_not_a_map(fn, map, w0);
  ```

  `zt_map_cap_ok`（`:334-336`）＝`cap>=16 ∧ cap<=2^30 ∧ 是 2 的幂`。
  所以 cap=2/3/5 的短 vec 进 `map_get` 会**先被点名并抛停**，不是自旋。
  这条抛停路径**实拍存在**：`/tmp/b10060/probes/bi_sorted_key.run.err`
  （177 字节，`PY-A: map_get was called on a value that is not a dict
  (handle=0x102449fd0, first word=2) — raising instead of dereferencing it as a hash table.`
  ＋ `Unhandled exception: code=1`；**那个 `first word=2` 正是短 vec 的头字**）。
  而四枚挂死探针**没有**这条信息 ⇒ 挂死点是**另一条路径**，本文未定位。
- **文档里那条死循环记录已经过期一档**：`docs/ABI.md:761` 记"cap=5 走不进 vec 臂 ⇒ 落到
  `map_get` 的开放寻址环 ⇒ 永不停止（附 B#7）"，`docs/ABI.md:52` 记 批次 423 起
  `map_resolve` 出口"另有一道粗判形 ⇒ 不合格即点名 + `zeta_raise(1)`，**不再解引用**"。
  两句合起来读：那个自旋入口已被批次 423 关掉，**剩下的未闭面就是
  `zeta_dyn_getitem:5120` 的旧 `cap>=8` 判据**（它把短 vec 判成"不是 vec"）。
- **下一步该做的不是改代码，是取栈（零新风险）**：机器上已有历史遗留的挂死进程
  （`backlog.md:1163` #20006 记了存量）。在其中一颗上跑
  `sample <pid> 1` 或 `lldb -p <pid> -o "bt -all" -b -o detach`，
  栈顶是 `map_get`/`map_has`（＝判据漏下去）、`zt_dyn_is_text` 的 `vm_read_overwrite`/`msync`
  探针（`py_additions.c:33-35`，＝内核态读别人内存，能解释 `U` 态）、
  还是 `abort`（＝#20006 那条收尾不完的路）——一次读数就能把这三档分开。
  **取栈的优先级现在是 D1/D3**：D2 上面已经读到调用点、并且知道 `data=0`＋`n=残值`，
  再取栈只是为了确认它停在哪个入口。**在拿到栈之前，本节 D1/D3 的任何"机制"写法都只算候选。**
- **候选动作（等取栈结果择一，都要授权因为都落在 `runtime/*.c`）**：
  ① D1 专用：`py_zip:1056` 改走 `zeta_dynarray_new`
  （**必须同时按新 cap 重算 `GC_malloc` 大小**，否则写出越界）；
  同一族还有六处同形写法（本文 `grep -n 'base\[0\]' runtime/py_additions.c` 实测：
  `:517`、`:1100`、`:2583`、`:2599`、`:2729`、`:2988`，外加 `:4149` 的
  `base[0] = len ? len : 8;`——短长度照样绕过钳位），
  但**没有一处实测在这三条链上**，动之前要各自先找到靶形；
  ② `zeta_dyn_getitem:5120` 的内联判据删掉，改调已有的 `zt_dyn_vec_hdr`（`:5169`）——
  一处改动就把这一族剩下的判据分岔消掉；
  ③ 若栈落在 `zt_dyn_is_text`：文本闸门要先过地址可读性再决定读内容（`py_additions.c:33-35`
  那两个探针的用法要收窄）。
- **验证方式**：改后重跑 D1/D2/D3 三枚，期望从"挂死"变成"值对"或"打出明确诊断"；
  取栈这一步本身不改仓库，可以单独成批（主体＝一次带读数的定位记录）。
- **附带：同一条 `LegacyBare` 发射路径的第二个症状（本文已实测，不再按候选写）**：
  `iso5/max_gen.z` 与 D2 只差函数名（`max` 代 `sum`），**不挂**、得 `0`，CPython 得 `9`。
  两条 MIR 并排读：`max` 那支发的是 `py_builtin_max(args:[3])`（1 参），
  `sum` 那支发的是 `zeta_sum_n(args:[3])`（1 参）。C 侧
  `py_builtin_max(int64_t vec)`（`py_additions.c:3331`）**正好是 1 参** ⇒ 少发参数没被发现；
  `zeta_sum_n(int64_t data, int64_t n)`（`:740`）是 2 参 ⇒ 少发一个就是把残值当长度用。
  站点＝`src/middle/mir/gen/call_num.rs:287`
  `(_, SumSeq::LegacyBare) => ("zeta_sum_n", false, None)`。
  发射点选 `zeta_sum_n` 却填 `None` 长度参数，是这一行的自相矛盾，不是编译器的猜测。
- **第 18 档（本组的上游风险，已记在 `docs/ABI.md:515/:521`）**：
  `src/backend/codegen/codegen.rs:3396`/`:3432`/`:3456`/`:3533` 对没有声明的符号按实参个数猜签名
  （`(0..args_count).map(|_| self.i64_type.into())`），`ABI.md:521` 原文"瀑布的最后一档不是报错，
  是'猜一个签名出来'"。本文实测：`py_zip`、`py_print_pairs`、`py_builtin_max`
  **都不在** `pylib/registry.txt` 或 `pylib/runtime_core.txt` 里
  （`grep -n 'py_zip\|py_print_pairs\|py_builtin_max' pylib/registry.txt pylib/runtime_core.txt`
  返回 0 行）⇒ 它们的原型来自这一档：arity 今天恰好对，少传一个参数时 C 侧就把 0 当
  `key_is_str`/`hkey` 读下去。**对照：`zeta_sum_n` 反而是在册的**
  （`pylib/registry.txt:513`），所以 D2 那条少发参数不属于这一档。
  **动作**：`py_zip`、`zeta_dyn_getitem`、`py_vec_union`、`py_vec_delitem`（新）四个符号
  一律登记进 `pylib/registry.txt` 或用显式 extern，别留猜测。

---

## 6. 实现顺序（一批一个可验收单元）

排序依据＝（读者能立刻少踩的坑）×（零授权优先）×（一条链不要拆成两半）。
每批都按 AGENTS.md 的口径只跑改到的模块内部单元测试 + 编译零错误，
`bash tools/sample_gate.sh <批次号>` 抽样，十批界再全局逐个用例跑。

| 批 | 内容 | 改动面 | 要授权 | 验收（除内部单元测试外） | 主要风险 |
|---|---|---|---|---|---|
| 1 | **M-A0**：差分判定把 stderr 的 W1002 计为该用例失败（单列一档，不改分母、不改基线） | `tools/diff_test.py` | 否 | 自建一枚含 `+x` 的临时用例必须变红；`syn60` 族 81 枚读数不变 | 语料里已存在的 12 个官方文件有 W1002 ⇒ 只在 `tests/diff/cases/` 面强制，`run_all.sh` 口径另附说明 |
| 2 | **B2**：`.union(` 方法臂（复用 `py_vec_union`）+ `py_zip`/`zeta_dyn_getitem`/`py_vec_union` 登记进 registry | `call_set.rs`、`call_class.rs`、`pylib/registry.txt` | 否 | `syn60b_set_union`；`--dump-mir` 前后对照只多一条 `Call` | 返回型继承接收者的约定（照 `call_set.rs:133`） |
| 3 | **A1 + A2 + A8**（三个"表里少一项"） | `expr.rs:1379`、`stmt.rs:600-613`、`expr.rs:1240` | 报备 parser 面 | `syn60b_unary_plus`／`pow_assign`／`comp_double_if`；每枚带一个二元同形对照 | `**=` 与 `//=` 的表次序；`//` 的降级名要与在册的 `syn60b_floordiv_call` 一致 |
| 4 | **链 5 + C7**：`.format` 规格转投 f-string 通道；`split_format_spec` 认 `!r/!s`；解析不了的分段改报错 | `gen.rs:3060-3071`、`expr.rs:1566-1633`、`call_builtin.rs:186-197` | 否 | `syn60b_format_spec_width`／`format_align`／`fstr_repr_flag`，各带"空规格"对照 | `gen.rs` 是主车道在制面（**批 4 开工前必须先拿到该面许可或推后**） |
| 5 | **链 2**：set 语义可辨识形状 + 字面量/推导走 `py_vec_add_unique` | `expr.rs:1071-1115`、`call_expr_lit.rs`、`call_dispatch.rs:4964` | 报备 | `syn60b_set_dedup`（三形态 `len` 相同） | AST 形状改动会牵动 `{**a}` 判别（`expr.rs:1080`） |
| 6 | **A3/A4 第一步**：`*`／`/` 能解析（暂按普通参数），默认值与传参校验留余项 | `top_level.rs:89-134/262-269`、`call_dispatch.rs:2801-2946` | 报备 | `syn60b_kwonly_marker`（只验证能跑通，注释写清未生效部分） | 批次 752/800 的闭包与位置绑定链 |
| 7 | **A5**：`parse_param_full` 提 `pub(crate)`，`lambda`/`\|x\|` 共用；默认值一路带到 `Closure` | `expr.rs:675-715`、`lower_closure.rs` | 报备 | `syn60b_lambda_default`（显式传参 + 用默认值两种调用） | 半修＝把静默错换成运行期报错，必须同批做完 |
| 8 | **try 三缺口**：A6 `else` + C11 `finally` 每个出口发射 + C12 异常最小对象 | `stmt.rs:1401-1600`、`codegen.rs:3940-3964`、`py_additions.c:3539-3552` | **是**（`runtime/*.c`） | `syn60b_try_else`／`try_finally_return`／`except_as_name` | setjmp 帧与 `zeta_last_error` 是既有依赖方（`pylib` 与老用例要同批改，见记忆"修掉巧合会揭出依赖它的绿用例"） |
| 9 | **链 1 第一步**：Tuple 打印去 arity==2 限制，改走已有 `zeta_tuple_repr` | `call_print.rs:544-555/681-683` | **是**（打印面） | `syn60b_tuple_print_arity`（1/2/3/混合）；`--bless` 前后差分成清单 | 呈现改动会成批换期望输出，**不许静默删用例** |
| 10 | **D 组拆两步。10a（零授权、可插到第 3 批位置做）**：D2 的 arity 正修＝`call_num.rs:287` 的 `SumSeq::LegacyBare` 改指 1 参版 `zeta_sum_vec`。10b（要授权）：在遗留挂死进程上取栈，把 D1/D3 的三档候选分开 → 按结果择一：`py_zip` 补钳位 **或** `zeta_dyn_getitem` 改调已有的 `zt_dyn_vec_hdr` **或** 收窄 `zt_dyn_is_text` 的读探针 | 10a：`src/middle/mir/gen/call_num.rs:287`（纯 Rust 发射侧）。10b：取栈不改仓库；改动候选 `py_additions.c:1050-1056/5116-5137/33-35` | 10a 不要；10b 的改动候选要 | 10a 验收＝D2 从挂死变成打出 `0`（**值仍错，CPython 是 `14`；不许写成"D2 修完"——A9 的 `yield` 缺口在上游**）＋ `cargo test -p zetac --lib gen` 全绿；10b 验收＝读数能落在三档中的某一档并写进 §5 | 10a：`LegacyBare` 的语义是不是真要 1 参版要逐条读同表四支；10b：分配大小必须与 cap 同步、938 个 `zeta_dyn_getitem` 调用点的影响面 |
| 11 | **B1**：`py_vec_delitem` + 列表臂 + registry | `py_additions.c`、`call_dispatch.rs:5772-5795` | **是** | `syn60b_del_index`（首/尾/中）+ 越界负向 | 与 D 组同文件，别同一批动两处运行期 |
| 12 | **链 3 小步**：`is 类型名` 折成 `isinstance` | `expr.rs:2507-2509`、`call_dispatch.rs:2171` | 否 | `syn60b_type_is_int`（五型 + 同一性负向） | `x is y` 与 `x is int` 的守卫表 |
| 13 | **B3 编译侧**：`sorted(key=…)` 形参按元素型定型，使元组下标走 `stack_array_get` | `call_builtin.rs:383/722-723`、`call_subscript.rs:149-224`、`lower_closure.rs` | 否 | `syn60b_sorted_key_tuple` | 与 10001/813 同族：目的槽定型要一路核到运行期读数 |
| 14 | **链 6**：`*expr` 列表展开 + `*name` 目标收集（静默跳过改报错） | `expr.rs`（数组字面量）、`stmt.rs:504/530/569-586`、`stmt_assign.rs:114-168` | 报备 | `syn60b_star_spread_lit`／`star_target` | `*` 一名两用要在早期分岔 |
| 15 | **A9 第一步（急切收集）**：`yield` 进关键字表 + 收集脱糖；惰性另议 | `parser.rs:82-87`、`ast.rs`、`expr.rs`、`call_dispatch.rs:4964` | 报备 | `syn60b_gen_two_yield`（已在库）从红变绿 + `next()` 明确报错 | 语义非惰性；三处必须写清边界 |

`A7`（序列模式）与 `B3/C13` 的根修（比较器 / 元组堆表示）不在表内：
前者要 `gen.rs` 工作面，后者要用户裁定方向（§7）。

---

## 7. 需要用户裁定的四项

1. **`runtime/*.c` 改动许可**（批 8、10、11，以及 A9 的惰性方向）：AGENTS.md 要求改运行期先授权。
2. **`sorted` 一族两个方向选哪个**：① 改 C 比较器
   （`py_additions.c:4260 zeta_sorted_vec_len` 按 i64 槽插入排序、`:4287 …_str` 按内容，
   `:4280-4285` 注释承认通用路径按指针排序）；② 把语义搬到 `pylib` 库面
   （`pylib/numpy.z`/`pandas.z` 已是"包装既有 C 原语"的路子，`registry.txt` 只留无库实现的原语），
   并让 `call_builtin.rs` 不再截获 `sorted`。**方向未定 ⇒ 批 13 只做编译侧定型，不碰排序本身。**
3. **打印面（`call_print.rs`）与 `gen.rs` 是否可由本车道动**：批 4、9 的前置。
4. **A 类动身前主车道 parser 在制面如何协调**：`expr.rs`/`stmt.rs`/`top_level.rs`/`pattern.rs`
   全部重叠（跨车道改文件要提前报备，经验教训 #7）。
另注（不算本文待裁项，只是提醒下一批开工前要看）：分支 `wip/739-745-parser-lambda`（`93ba5a90`）
是否重新落地；十批界全局跑落在 10061（上次全量 10051），语料分母现为 3026，用前重取。

---

## 8. 未定位 / 未证清单（不许写成结论）

| 项 | 已有证据 | 缺什么 | 定位方法 |
|---|---|---|---|
| C4 后半 | `call_print.rs:681` 症状点确认；`stmt_return.rs:29-38/60` 的贴回逻辑读到了 | `Tuple` 型**具体在哪一步**从目的槽消失 | 在 `stmt_return.rs:60` 与 `call_print.rs:544` 各插一次性 `eprintln!` 打印 `type_map` 读到的型，四枚夹具（1/2/3 元组 + 函数返回）读 stderr，`git show HEAD:` 还原并核 md5 |
| C6 后半 | `zt_parse_spec` 支持 `^`/宽/精度；`call_builtin.rs:186-197` 三通道存在 | `{y:^9.3}` 得 `3.14` 的落点（疑非 F64 通道，但**未实测**） | 同夹具 `--dump-mir` 取发射的函数名（`py_fmt_f64` 还是 `py_fmt_str`），再读该 C 函数 |
| C5 `rest=2` | LHS 用 `parse_unary`（`stmt.rs:504/530`）、降级只认 `Var`（`stmt_assign.rs:126`）都确认 | `rest` 这个**未绑定名字**为什么读出 `2` | 在 `stmt_assign.rs:125` 循环前一次性打印 `litems` 形状清单；再看 `rest` 走的是 `zeta_env_get` 还是同名槽复用 |
| C8／C14 | 单构造绿、复合红的两处读数；`<null>` 字形与 `print_*`/`println_*` 分层同形 | 触发条件（哪一类后续语句会让前一条 print 多出 `<null>`） | 二分：固定 `print('x', end='!')` 为前段，逐条换后段（容器打印／循环／函数定义／多参数 print），每次 `--dump-mir` 看多发了哪条 `VoidCall` |
| C15 | 单构造绿、复合形态得 `0` | 触发条件与 `nonlocal` 站点 | 同 C8 的二分法；先 grep `nonlocal` 在 `src/frontend`/`src/middle` 的落点 |
| C10 后半 | 读写两侧不对称已确认（`stmt_assign.rs:14-26/436-443` vs `call_field.rs:54-64`） | `@staticmethod` 返回 str 打成地址 | 单独一枚夹具（只调静态方法并打印返回值），走链 1 的打印面还是类属性面要先分清 |
| D1／D3 挂死点 | 已证：D1 链上有绕过钳位的 `py_zip:1056`（cap=2）；D3 链上**没有**短头构造点（MIR 只发 `zeta_dynarray_new`＋`vec_push`＋`py_json_dumps_vec_typed`）；两者 `run.err` 均 0 字节 ⇒ 挂死前没打运行期诊断 | **挂死点本身**。"短 vec 漏进 `map_get` 自旋"这条推测已被 §5 反证（`map_resolve` 出口的形状闸门会先点名抛停，实拍见 `bi_sorted_key.run.err`）；#20006 是否同一个因也未证；机器上 `~/Library/Logs/DiagnosticReports/`（817 份）**没有**这 8 个挂死二进制的 `.ips` ⇒ 也不能默认"崩在崩溃报告路径上" | 在**已存在的挂死 pid** 上 `sample`/`lldb -p … bt -all`（零新风险）；不要在改前重跑会挂死的夹具 |
| D2 挂死链（本文已读完） | MIR 实拍：`seq` 返回 `IntLit(0)`（`yield` 被丢，W1004）→ `zeta_sum_n` 只发 1 个实参、在册签名要 2 个 ⇒ `data=0`、`n=`残值 | 残值 `n` 具体多大、是否真读到未映射页 ⇒ 只有取栈/`lldb` 能定；A9 的 `yield` 缺口是上游独立一条 | 10a 改指 `zeta_sum_vec` 后重跑：从挂死变打 `0`＝链已通、值仍错（不许写成修完） |
| B3 根修 | 元组无独立值类型（`call_expr_lit.rs:326-368`）、`zeta_dyn_getitem` 认不出栈数组（`:5116-5137`） | 给元组 GC 头会影响多少调用点 | `codegraph` 取 `zeta_dyn_getitem`/`map_get` 的调用点清单 + 语料 `--dump-mir` 里 `StackArray` 出现数（`nm` 核符号存在） |
| `max(生成器)=0` | `call_num.rs:287` 发 1 参、`py_additions.c:740` 要 2 参（两侧都读到） | 少传的那个参数是否真在调用点缺失 | `--dump-mir` 取 `zeta_sum_n`/`zeta_max_n` 的 `args` 长度，与 C 签名逐条对照（这是第 18 档猜测签名的直接后果） |

---

## 9. 这批测量本身的三条方法（写下来给后续批次复用）

1. **单构造必须单独入库**：一次解析失败会丢掉**所在行之后的整段程序**（`main.rs:466-502`），
   所以复合探针里"红"的构造，单独写可能是绿的。批次 10060 实测有 24 个这种形状
   （台账 `roadmap.md:33412`）。不拆开就会把"已支持"记成"不支持"，也会把缺口算小。
2. **挂死风险的驱动写法**：`subprocess.run(timeout=…)` 会被不可中断态的进程钉死
   （第一版驱动就卡在`bi_core`），改成"后台 `Popen` + 每 0.15 秒 `poll()` +
   读 `ps -o stat=` + 到点放弃继续"，并把 stdout/stderr 直接重定向到文件
   （顺带避开 64KB 管道写满的死锁）。74 枚 2 分钟跑完，两次全量读数一字相同。
3. **挂死的链要"只编译不执行"地读**：`zetac --dump-mir <文件>` 只走到发射，
   不会启动那个收不掉的进程，所以挂死夹具的调用链照样能读（本文 D1/D2/D3 三枚的
   MIR 就是这么做到的，存 `/tmp/b10061/`）。要判断"少发了几个参数"，
   就看 `Call { func: ..., args: [...] }` 的 `args` 长度，与 `pylib/registry.txt`
   里在册的 `args=i64,i64` 逐个对——在册符号能对，不在册的（第 18 档）连对的对象都没有。
