# Zeta 编译器 Roadmap

> 状态图例：[ ] 待做 | [~] 进行中 | [x] 完成 | [-] 放弃/降级
> 工作区：`/Users/meetai/source/zeta-src`（bootstrap 分支 → `agentic` 远端）
> 测试资产：官方单测 **`tests/unit-tests/`（194 文件，进 git 的正本）**；回归套件 `/tmp/bench`；**Python 风格套件 `tests/python_style/`（至 t263；`*.z` 需 `git add -f`）**
> **当前进度快照（2026-09-19，批次 149 收口后）：python_style 260/263 · 官方 194/194 · 语料解析 38/38 —— 见文末「批次一百四十九 收口」**
> Python 库注册表：**16 个模块**（见「库导入机制」小节）；第三方库 `zorb install` 可用，已验真实库 `python-stringcase` 全函数正确
> 新目标（2026-09-11）：**基本能编译 Python**——PY-A 兼容层推进中
> 语法设计定稿：**`docs/python-syntax.md`（实现以此为准）**
> **流程硬规则（AGENTS / goal 验收强制）**：每完成一个 goal batch 任务 → **立刻 `commit` + `push`**（缺一不可）；禁止攒多批再捆提交。禁止对未提交大文件 `git checkout --` / `rm`（改用 `mv` → `.trash/`）。推送目标是 **`git push agentic bootstrap`**（`origin` 是 https，无凭据会失败）。当前无未推送提交。

## 排查方法论 + 进度快照（2026-09-17）

### 一、方法论：在「真实 Python 语料」上定位问题的标准流程

**诊断开关（已固化，另见 `validate.md`）**

| 开关 | 作用 |
|---|---|
| `W1002`（默认） | 解析尾部未消费时**警告**：丢了多少行 + 起始文本（不再静默截断） |
| `ZETA_STRICT_PARSE=1` | 同上但升级为 `E1002` **致命** —— 用来量「到底编译进去了多少」 |
| `ZETA_DUMP_PP=<path>` | 把**缩进预处理之后**的源码写到 path（解析器真正吃到的文本） |

**流程（每步都别跳）**

1. `zetac <file> -o /tmp/o`，读 `W1002`：丢了多少行、余量从哪开始。
2. **别信余量首行**：`many0` 停在**项的起点**，所以余量永远从 `def …`/`class …` 开始，
   看着像签名问题，实际常常是**函数体内**某个构造 → 直接 `ZETA_DUMP_PP` 看真实文本。
3. **判据一**：若「删掉 k 行 → 丢行数减少约 k」，说明解析停在**固定位置**而非某个构造，
   应直接定位那个位置，不要再二分。
4. **块级 leave-one-out（保留上下文）**：把某条语句连同其子块整体换成 `pass`，看丢行是否下降。
   优于「最小前缀」——前缀会把块切一半、制造**空块假象**，把 k 定位到无关的 `if` 头上。
5. **隔离复现**：把可疑函数抽成独立小文件；能复现就逐步还原；不能复现说明是上下文相关，
   回到第 4 步在真实文件里做。
6. **编译插桩**：需要看 AST/中间态时加 `ZETA_PROBE_*`（env 门控）只打一行。

**踩过的坑（直接当检查项用）**

- **空块假象**：把文件截在 `if x:` 处 ⇒ 空块 ⇒ 解析必然失败，与真 bug 无关。
- **`ExprStmt` 会被规整成裸 `Call`**：体里的语句有时不带 `ExprStmt` 包裹（`parse_func` 注释里有记），
  任何「扫描体内语句」的代码必须两种形态都认。
- **`tag("_")` 吃掉前缀**：`parse_pattern` 与 `parse_type` 各一处 —— `_code` / `_HistoryFrame`
  被读成通配符 `_` + 残渣 ⇒ 必须加**词边界**。光这一个 bug 值 654 行。
- **短路分支**：`kw.is_empty()` 之类提前返回，让后面的「默认值填充」永远走不到。
- **插错位置 = 静默不生效**：`type()` 的特殊分支我插在 `arg_ids` 计算**之前** ⇒ 永不命中；
  代码审查看不出来，只能靠**实测输出**发现。
- **panic 优先于诊断**：先用 `is_char_boundary` 之类把「崩溃」堵住（编译器绝不能因源码输入 panic），
  再谈报错是否清楚。
- **输出纪律**：会话上下文是**每轮整体重传**的，打印量 ≈ 后续每轮的网络出量与成本。
  定位问题时优先「窄 grep / 少量行 / 落盘后只读关键行」，避免整表整文件打印。

### 二、进度快照（2026-09-17）

**三套基线**：官方 **194/194**；python_style **230/230（0 failed）**；语料「解析通过」**38/38**、「完全解析」**38/38**、未解析行合计 **0**。

**语料（REasyQuant，38 个文件）** — 度量脚本：`tools/corpus_baseline.py`（解析通过）+ 对 `W1002`「丢了多少行」求和（完全解析口径）

| 指标 | 本阶段起点 | 现在 | 七轮批次合计 |
|---|---|---|---|
| 完全解析 | 6 | **38/38** | 12 → 38 |
| 未解析行合计 | 8580 | **0** | 6138 → 0（−100%） |

剩余拦路：语料 **W1002 已清零**（批次一百一十六修 `@`）；链接/运行缺口仍在（平台 API 等，本轮范围外）。

**本批次十五（2026-09-17，调研 + 定位，未落地修复）——链接期缺口的第一批**

按「距离能运行 REasyQuant」的调研结论开工清**自研缺口**，先把 `timedelta(16)/date(6)/datetime(1)`
这一类定位清楚（这是调用点最多的一类，23 处）：

- **实测口径**：语料 38 文件 解析 **37/38**、链接 **1/38**、运行退出码 0 **1/38**；
  未定义符号去重 **87**；`nm` 查 `zeta_runtime_c.o`/`tokio_runtime.o` 对这 87 个符号
  **0 命中**（所以不是链接顺序）；`grep -rl "zetac|zeta_" REasyQuant` **0 文件**（宿主未集成）。
- **根因（批次十六已修正上一条归因）**：**不是**「导入类名遮蔽模块名」——无任何 import 时同样失败。
  真正的形状是「同一模块内，不同成员的解析结果不同」：

  | 调用（**无任何 import**） | 结果 |
  |---|---|
  | `datetime.strptime(s, fmt)`（2 参） | **链接通过** ✓ |
  | `datetime.date(2020,1,1)`（3 参） | ✗ 裸 `date` |
  | `datetime.datetime(2020,1,1)`（3 参） | ✗ 裸 `datetime` |
  | `datetime.timedelta(3)`（1 参） | ✗ 裸 `timedelta` |
  | `datetime.now()`（0 参） | ✗ 裸 `now` |

  注册表里 `strptime`/`date`/`datetime`/`timedelta` 都是**同样形状**的 `F datetime <member> <sym> …`，
  所以差别只能出在**解析/降级路径本身**（疑似与 arity 有关：0/1/3 参走不到注册表命中，
  2 参能到）。注意 `timedelta` 的注册表项是 `args=i64`（1 参）却仍失败 ⇒ 不是「参数个数对不上」那么简单。
  **批次十七：探针跑过了，结论如下（关键一步已确定）**
  - 加了 env 门控探针 `ZETA_PROBE_DT`（打印 root/parts/alias/find_module），并给
    `py_member_target` 补了「root 是已注册模块就用它」的回退。探针实测：
    `root="datetime" alias=None find_module=Some("datetime")` ⇒ **回退生效、模块已认出**；
    且编译输出里**没有** `unknown member` 警告 ⇒ `find_member("datetime","timedelta")` **命中**。
  - **但链接期仍然是裸名 `_timedelta` / `_datetime`**（`ld` 报错原文）⇒ 说明
    **调用发射根本没走 `py_member_call`（line 3508 那条路）**。`py_member_call` 全文件只有
    一个调用点（3508，位于主 Call arm 2943 之内），所以 `datetime.timedelta(...)` 必然被
    **更早的分支**截走了（或走了另一个 Call arm）。
  - **批次十八：探针进到了 arm 内部，出现一对「应该一致却不一致」的现象**
    （env 门控 `ZETA_PROBE_CALL`，在 arm 入口与 3508 前各埋一个）：
    - `import datetime` 场景（能链）：`arm-entry` ✓ → `reached-3508` ✓ → 链接通过
    - `from datetime import datetime, timedelta, date` 场景（不能链）：
      **`arm-entry` ✓ → `reached-3508` ✓**（说明确实走到了 `py_member_call`！）→ 但 `ld` 仍报**裸名** `_datetime`/`_timedelta`
    - 同时：**没有** `unknown member` 警告 ⇒ 不能证明 `find_member` 未命中
    - 加了「root 是已注册模块就用它」回退后：探针显示 `find_module=Some("datetime")`（回退生效），
      **但 `ld` 报错依旧**，且仍无警告
    ⇒ 矛盾点：**调用点走到了 3508、模块/成员也能解析，但最终发射的仍是裸名**。
    这只能是「3508 之后还有一条发射路径」或「返回的 symbol 被丢弃/覆盖」。
    **批次十九（已修，commit `5e412456`）**：按上面这条线埋探针后拿到决定性一步 ——
    `PROBE hit`（3508 命中分支内）**根本不打印** ⇒ `py_member_call` 返回 **None**，
    随后掉进 handle/identity 兜底发出裸名（这才是根因，不是「二次发射」）。
    修法：在**调用发射点**加注册表回退 —— 接收者是 `Var(name)` 且 name 恰为已注册模块时，
    按 `name.member` 查；再补 `name.name.member`（注册表把 `datetime.now` / `date.today`
    这类类静态成员记成 dotted 键）。
    **实测**：`date/datetime/timedelta(3)/timedelta(days=3)/now()/strptime(...)` 修复前全部
    Linking failed → 修复后**全部 Compiled**；t157 pre-fix FAIL / post-fix PASS；
    未定义符号去重 87 → 86（`timedelta` 16→8）。
    ⚠️ 期间三四次「无证据的实验」都已回滚（registry dotted 条目、`py_member_target` root 回退、
    两轮探针）——**留在树上的只有最后这条被实测证明有效的改动**。
- **试过但无效**：在 `pylib/registry.txt` 里补三条 dotted 成员
  （`F datetime datetime.timedelta py_dt_timedelta …` 等）——**实测仍链接失败**，
  说明这条路径压根没按「base 文本 + 成员名」查注册表（对照组 `datetime.strptime` 能通，
  是因为它命中了另一条分支）。该实验已**回滚**（无证据的改动不留）。
- **下一步（已定位到落点）**：在**方法调用降级**处，对「base 是 `Var(name)` 的 dotted 调用」
  加一次注册表回退（`name` 恰为已注册模块时按 `name.member` 查），**必须在退化成自由调用之前**。
  然后同样处理 `range(3)`、`getattr/dict/object/setdefault/_Info` 与 `py_asdict_unexpanded`
  （后者是**占位符**：没实现就发了符号出去，应改为响亮报错）。

**本批次十四（2026-09-17）——`impl` 不再保留（commit `00acc6fc`）**

- **症状**：`jq_wufu_local._run_nautilus`（159 行）整体被丢；该函数到处用 `impl` 当变量名。
- **根因**：`impl` 在 `parse_ident` 的保留字表里 ⇒ 每一条读它的语句解析失败。
- **修法**：① 保留字表去掉 `impl`（`impl Foo { … }` 仍由 `parse_impl` 接住 ——
  `impl` + 标识符 + `{` 不可能是表达式语句）；② 顶层 fail-loud 的 `DEFINITION_KEYWORDS`
  判定里，`impl` 只在「后面跟 标识符/`<`」时才算定义关键字（否则顶层 `impl = …` 被判成定义失败）。
- **定位方法**：源码层单块 LOO 与两两组合都无命中（**多个独立成因**），换**块级 ddmin**
  （成段删除完整块）后收敛到 1 块 `engine.add_strategy(impl)`。
- ⚠️ **教训**：最小文件确实失败，但我第一遍用 `tail -5` 看输出，把 W1002 截掉了，
  差点当成「不失败」。**看编译器输出永远不要 tail 截断。**
- 度量：未解析行 **236 → 77**，**完全解析 36/38 → 37/38**（仅剩 ETF动量EPO 的 `@`）。

**本批次十三（2026-09-17）——浮点比较类型（commit `1668b1c4`）**

- **症状**：`w = 5.0 > 1.0` → `print(w)` 打 **0.000000**，`if w` 走错分支（浮点守卫静默失效）。
- **根因**：gen.rs 的 BinaryOp 收尾按「有一个 f64 操作数 ⇒ 结果是 F64」推断**所有**算子，
  比较算子也被算进去 ⇒ 真值 1 以整数写进 double 槽，读回来是 ~5e-324。
- **修法**：比较/逻辑算子（`== != < > <= >= && || in not in`）在浮点分支里仍返回 `Type::Bool`。
- 顺带更正上一批报告里的错误归因：我原以为触发条件是「f64 数组字面量在前」，
  实测与数组**无关**，任何「浮点 vs 浮点比较赋值给变量」都中招。
- 度量：python_style 154 → **155**；语料 236 → 236（语义修复，不动解析率）。
- 验证方法教训：换回旧二进制后 `touch` 源文件重建，重建的**仍是修好的源码**
  ⇒ 假 PASS；验 pre-fix 必须 `git stash push <源文件>` 再 build。

**本批次十二（2026-09-17）——两处静默错值（commit `3b645275`）**

| 问题 | 症状（实测） | 根因 | 修法 |
|---|---|---|---|
| **数组参数上的 `in` 恒为 0** | `def has(xs: [i64], v: i64)` + `if v in xs`：`has([1,2,3], 2)` → **0** | `x in xs` 降级成 `__contains__`，而它只查 `type_map` 的类型；**数组参数**的类型记在 `source_types`（未标注形参在 type_map 里默认 I64）⇒ 落到「不支持」分支返回 0（只有一行容易漏看的 warning） | 加 `source_types` 数组回退；元素是否字符串也从 `[str]` 取 |
| **列表 `==` 恒为 0** | `a=[1,2]; b=[1,2]; a==b` → **0**；`[1,2]==[1,2]` → 0 | `==` 编译成两个**句柄**的整数比较 ⇒ 内容相同永远不等 | 新增运行时 `py_list_eq(a,b,elem_is_str)`（长度 + 逐元素，字符串 strcmp），gen.rs 对数组操作数分派，`!=` 用 `^1` 取反 |

理由：这两处都出现在**策略的过滤/守卫代码**里（`if code in list(...)`、`if a == b`），
属于「能编译、能跑、结果错」——比编译失败危险。

验证：t154（10 条断言）pre-fix FAIL / post-fix PASS；修复前后同一程序实测
`0 0 0 0 | 0 0 0 0 0 1` → `1 0 1 1 | 1 0 1 1 0 1`；官方 194/194；语料 236 → 236（语义修复，
不动解析率）。附带提醒：`py_list_eq` 需重建 gitignored 的 `zeta_runtime_c.o`。

**本批次十一（2026-09-17）——集合字面量（commit `b12fd0f4`）**

**`{a, b, c}` 此前完全不支持**（roadmap 早先记录的缺口）：`{1,2}` / `{1,2,}` / 多行 set 全失败，
带 set 的整个定义被丢 —— 命中 `指数ETF动量轮动.initialize` 的 `g.stocks={'510500.XSHG', …}`（146 行）。
修法：`parse_dict_lit` 按「第一个元素后面有没有顶层 `:`」分派；Zeta 无 set 类型，而
**既有 setcomp 已降级成 list**，故 set 字面量同降级成 list（重复元素不折叠，同一条已知限界）。

回归（t121 立刻抓到）：`{**a, **b}` 里 `parse_expr` 会把 `**a` 当双重解引用**成功解析**，
于是「没有 `:`」把它误判成 set → 已加 `*` 开头探针排除。

度量：未解析行 **382 → 236**，**完全解析 35/38 → 36/38**。

**本批次十（2026-09-17）——两处根因（commit `c8bfad4b`、`8e3a2f1e`）**

| 根因 | 症状 | 修法 | 度量 |
|---|---|---|---|
| **参数名前缀撞关键字** `types`/`type_s`/`typeOf` | `parse_param` 的「放宽关键字参数名」分支用裸 `tag("type")`，`def f(types)` 被读成参数 `type` + 残留 `s` ⇒ 整个定义被丢 | 把 `kw_boundary` 提到 `parser.rs` 共享，新增 `parse_kw_param_name` | 731 → 479（`jq_shim` 275→23） |
| **单行复合体** `if not xs: return []` | 缩进预处理只给「体在下一行且缩进更深」的头插 `{`，这类行带着冒号透传 ⇒ 整个定义被丢（**84 行 / 16 文件**） | 新增 `fold_inline_bodies`，在任何缩进记账之前改写成 `head { body }`；仅当行首是块关键字、冒号在代码位置且深度 0、`::`/`:=` 排除、后面有内容 | 479 → **382**（**完全解析 32/38 → 35/38**） |

开发过程中的两次踩坑（教训已写进代码注释）：
- 单行体改写第一版拿 `info.code`（**字符串已涂白**）的下标去切**原始行**，下标对不上，
  生成了 `if no { xs:return [] }` 这种残骸 —— 必须自己在原始行上扫描。
- `find_inline_colon` 起初只把 `#` 当注释，于是 brace 风格行 `if c { x }  // note: 1,2`
  的注释冒号被当成内联体冒号，切片反向 ⇒ **编译器 panic**；官方套件 `test_simd_murphy`
  立刻把它从 194 打到 193。补上 `//` 注释与 `colon >= code_end` 防御后恢复。

一次**撤回**：曾`认为 `lambda_`（参数名以前缀撞 `lambda`）也是缺陷，加了词边界 + 用例；
但用旧二进制跑遍 `lambda_=1` / `print(lambda_)` / `{'lambda_':1}` / `lambda lambda_: …`
各种形态都不复现，遂**回滚**该改动（无证据的改动不留）。当时那个「复现」是 harness 假象：
探针被插进了 `def f(context):` 里，成了**嵌套 def**。

### 批次九（**已修** `b9543737`）：集合 `for` 里的 `continue` 死循环

最小复现（**所有历史二进制都复现，非本次改动引入**）：

```
def f() -> i64:
    x = 0
    for i in [1, 2, 3]:
        if i == 2:
            continue
        x = x + i
    return x
print(f())      # 永不返回
```

根因（已定位到代码）：`gen.rs` 的**集合式 for** 被降级成「`while index < len` + 体末 `index = index + 1`」，
而 codegen 的 `MirStmt::While` 把 `continue` 跳到**条件块**——于是自增被跳过、下标不动、死循环。
range 式 `for` 走的是 `MirStmt::For`（有独立的 `for.inc` 块），所以不受影响。

影响面：`for x in list: if cond: continue` 是策略文件里的常见写法（`filter_stocks` 等），
一跑就挂。**这是本项目目前最严重的正确性缺陷（挂起优于错值）**，优先级高于剩余解析拦路。

修法（已落地）：把自增移到「`array_get`/模式绑定之后、用户体之前」。**不需要**额外快照槽——
用户可见名绑的是**元素**（`for i in [1,2,3]` 里 `i` 是元素，绑在 `get_id` 上），
只有内部下标槽与条件读 `index_var_id`，所以自增提前对循环体不可见。
回归：`t149_for_continue_advances.z`（continue / break / 体内读循环变量 / 嵌套循环 /
元组模式 / 空集合 六种形状，pre-fix 挂起、post-fix PASS）。
顺带给 `tests/python_style/run.sh` 每个用例加 `timeout 20`——挂起的程序会把整套测试卡死
（本次卡了 30 分钟才发现这条缺陷）。

**本批次七（2026-09-17）——四处 parser/预处理根因（commit `33d21d0e`、`eb8c2f13`、`bcab1438`、`d6ddf042`）**

四个都是「**静默错值或全丢**」类，且都先在 `W1002` 余量里看到一个看着不相干的 def：

| 根因 | 症状 | 修法 | 度量 |
|---|---|---|---|
| **反斜杠续行** `x = 1 + \` 换行 `2` | 残留 `\` 使语句解析失败，整个顶层项被丢 | 预处理器在**任何缩进记账之前**把 `\<newline>`+续行前导空白折叠成一条逻辑行 | 2641 → 2467（完全解析 +2） |
| **逗号下标里的切片元素** `.iloc[:, 0]` | 下标元素走 `parse_expr`，裸 `:` 不是表达式 | 元素可以是切片（复用单索引切片同款形状，receiver 由 `bind_slice_receiver` 补上） | 2467 → 1629（**完全解析 +4**） |
| **三元条件以一元算子开头** `X if -200 <= diff < 200 else Y` | 条件是从 `if` 到 `else` 的**切片**，带前导空白；`parse_expr_no_if` 自己不跳空白，一元算子识别又是**按位置**判断的 | 解析条件前 `skip_ws_and_comments0` | 1629 → 1376 |
| **单引号原始字符串** `r'2\|3'` | 只认 `r"…"`；赋值位静默变成 `x = r` + 游离字符串，实参位直接弄坏调用 | `alt((tag("r\""), tag("r'")))`，遇同种引号即结束 | 1376 → **881**（**完全解析 +5**） |

两个值得记的细节：
- **切片元素不能走真实 `zeta_slice_vec`**：它把基址当 Vec 头读，而基址是不透明的平台对象（numpy/pandas）——
  `d[:, 0]` 在整数基址上**直接段错误**（已实测）。改用新的平台桩 `py_slice_new(start,end,step)`。
- **“同一个构造在不同上下文表现完全不同”**：反斜杠续行、三元条件一元、单引号原始串这三个，
  在独立语句里都能过，只在特定位置（切片出来的子串 / 调用实参）才炸。

新增用例：`t144_backslash_continuation.z`、`t145_multi_index_slice.z`、`t146_ternary_cond_unary.z`、`t147_raw_string_single_quote.z`（均 pre-fix FAIL / post-fix PASS）。

**本批次六（2026-09-17）——两项 parser 根因（commit `23277519`、`f9daf54e`）**

- **成员后的裸 `<` 不再当泛型实参**：`opt(tag("::")) + parse_type_args` 让裸 `<` 也被接受，
  于是 `filter(a.b<10,#c…\n indicator.eps>0.3,)` 里的 `<` 被当 `<...>`，inner-slice 向前
  抓到 `>0.3` 的 `>`；关键是**中间的 `10,#…\n indicator.eps` 恰好也能解析成一个类型列表**
  （数字 + 点号路径），所以上一批加的「inner 必须整体被消费」校验也放行了。
  修法：成员后的类型实参必须带 `::`（turbofish，Rust 同规则）；仓库内无一处 `obj.m<T>()`
  裸用法（tests/pylib/stubs/语料都只写 `::<`），零兼容损失。`parse_type_path` 里的
  `Name<T>`（类型位置，无比较歧义）保持不变。
  度量：未解析行 **3254 → 2641**，**完全解析 17/38 → 21/38**（+4：干积分-量化框架 258→**0**、
  基本面01+RSI择时 140→**0**、白马股攻防转换 84→**0**、大市值价值优化 62→**0**）。
- **`for … else:` / `while … else:` 端到端**（语义：循环未 `break` 才执行 else）：此前 `else` 子句
  让整个顶层项被丢弃。实现走**双出口**——正常结束分支到 `*.else` 块、`break` 目标指向 else 之后
  （loop_stack 的 exit），因此**不需要改写 break、也不需要「是否 break 过」的标志变量**。
  AST/MIR/codegen 全链路携带 else 体；遍历点（resolver nonlocal/collect_calls、typecheck、
  borrow、ctfe、dead-code、substitute）全部同步——**必须遍历 else 体，否则只在 else 里赋值的
  局部量不会被登记**。CTFE 不支持 break，故 else 一律执行。
  度量：**3372 → 3254**（`干积分-量化框架` 376 → 258）。
- 新增用例 `t142_loop_else.z`（无 break 的 for-else / 有 break 的 for-else / while-else /
  有 break 的 while-else）、`t143_member_lt_not_generic.z`（均 pre-fix FAIL / post-fix PASS）。

**本批次五（2026-09-17）——逗号下标 `a[i, j]`（commit `8d495a94`）**

- **根因**：postfix 的 `[` 只有两条分支（`parse_expr` 单索引、`start:end` 切片），
  **没有逗号下标**。后果一半响亮一半静默：`if x[0,0]:` 丢整个顶层项；
  `a = x[0,0]` **能编译但 a 拿到的是 x**（`[0,0]` 被当无用数组字面量语句）。
- **修法**：parser 新增 `parse_multi_index`（仅 >= 2 个索引时成立，单索引不变）→
  `Subscript { index: Tuple([...]) }`；gen.rs 识别 Tuple 索引 → `py_getitem2(base, i, j)`；
  runtime/py_additions.c 加**平台桩**（本地恒等返回句柄，与 `zeta_platform_obj` 同属 L3；
  宿主链接自己的 `py_getitem2` 即可覆盖）。codegen init 补 3×i64 extern 声明。
- **改成响亮失败的方法**：删掉 `py_getitem2` 这个 C 函数即可 —— 调用会变成链接期
  undefined symbol。（单独的语句：本地 standalone 跑平台策略本来就没有真实语义。）
- **度量**：未解析行 **3889 → 3372**（−517），6 个文件改善，**无回退**：
  子账户多策略分仓 267→162、小市值排除3bug版 267→162、国九小市值 235→139、
  国九条中小板微盘 235→139、四大搅屎棍 258→166、ETF动量EPO 100→77。
- 新增 `t141_comma_subscript.z`（pre-fix FAIL / post-fix PASS）。
- 记：`zeta_runtime_c.o` 是 gitignored，本机改 `py_additions.c` 后必须重建（validate.md §4）。

**本批次三（2026-09-17）——两项字面量/预处理修复（commit `988e3af8`、`022dc05e`）**

- **科学计数法浮点字面量 `1e8`**：`parse_float_lit` 曾明确不支持指数，走整数分支只吃下 `1`，
  剩下的 `e8` 变成游离标识符 —— `v = 1e8` **能编译但是 1**（静默错值！）；
  `f(1e8)`/`[1e8]` 里那个游离的 `e8` 让括号不闭合⇒整个顶层项被丢弃。
  语料 22 处 / 10 文件（`income.operating_revenue > 1e8` 是三个 `get_stock_list` 的共同拦路）。
  修法：`parse_float_lit` 增加 `[eE][+-]?digits` 分支，且必须真有数字才吃下指数。
  度量：未解析行 **4631 → 3988**（−643，7 个文件改善）。
- **括号内续行的缩进不再触发块闭合**：Python 里括号未闭合期间的换行缩进无意义，
  但 dedent 循环无条件按缩进弹块栈，一个**顶格续行**会把 `}` 插进表达式中间：
  `x = [` 那行就地关掉 def（`x = [\n}`），其后全丢。真实形态是「跨行嵌套列表、续行顶格」。
  修法：`depth > 0` 的续行跳过 dedent 循环。度量：**3988 → 3889**（`首板低开原版` 329 → 230）。
- 新增用例 `t139_scinot_f64.z`、`t140_paren_continuation.z`（均 pre-fix FAIL / post-fix PASS）。

**本批次二（2026-09-17）——`//` 整除端到端（commit `77faca30`、`268a8027`）**

- **根因**：python 风格源码里 `//` 是整除运算符（注释是 `#`），但 `line_comment` 无条件把
  `//` 当注释。真正的伤害不止「吃掉本行余下文本」：切在 `//` 处会让该行**括号不平衡**
  （`int(cash / price` 永远不闭合），而缩进预处理器靠括号深度折「逻辑行」——深度卡在 >0
  之后，**同一块内后续所有块头都不再被识别为块头、`{` 全部不插** ⇒ 定义连同其后内容静默丢弃。
- **修法**：预处理器**确认 python 风格后**（`normalize_blocks` 第一遍返回 `changed`）
  把代码位置的 `//` 改写为字词运算符 `floordiv`，然后**重跑一遍** normalize（必须先于扫描，
  否则上面的括号污染依旧）；parser 把 `floordiv` 加进乘法级算子表（带词边界，`floordivx`
  仍是变量）；resolver/CTFE 同步识别；codegen 用 `sdiv+srem+符号修正`（真正向下取整，
  `-7//2 == -4`）/ `fdiv+zeta_floor_f64`。
- **度量**：`jq_wufu` 72 → **0**、`jq_wufu_daily` 72 → **0**、`jq_shim` 346 → **275**；
  合计 **4846 → 4631**，完全解析 **15/38 → 17/38**，无任何文件回退。
- 新增用例 `t138_floordiv.z`（pre-fix FAIL / post-fix PASS 双向验证）。
- **两条已知限界（均为响亮失败，非静默错值）**：① 无缩进块的「扁平」python 文件不会被
  判定为 python 风格，`print(7 // 2)` 仍报 `W1002`；② `//=` 不支持（同样 `W1002`）。

**本批次一（2026-09-17，commit `9e5cf1ff`）——「parser 误吃前缀」两处**

- **`.member` 成员名不再套保留字黑名单**：`np.where(mask)` 里 `where` 被 `parse_ident` 拒，
  失败在 postfix 内层 ⇒ **整个顶层项连同其后全部内容被静默丢弃**（jq_shim 一处吃 445 行）。
  新增 `parse_member_ident`，expr.rs 字段/方法名解析点改用它。
- **泛型实参必须吃满整段内容**：`parse_type_args` / `parse_bracketed_type_args` 原先只要求
  inner 能解析出一个类型列表**前缀**，剩下文本被 `let (_, args) = …` 丢掉。于是
  `a[b.c < d]` 里的 `<` 被当泛型实参，inner-slice 扫描器向前抓到文件里**下一个 `>`**
  （可远在几百行外，甚至落在字符串字面量里），中间文本静默丢弃。改为要求整体消费。
- 顺带：`parse_type` 的 `_` 词边界（`-> _HistoryFrame | None`；pattern.rs 同类已在 `fd2dd593` 修）。
- 新增回归用例 `t137_member_keyword_and_typeargs.z`（pre-fix FAIL / post-fix PASS 双向验证）。
- （当时已定位未修、后续已在批次二修掉：`//` 被当行注释。）

**本阶段修掉的（26 个提交，按类）**

- **解析层**：`import X as Y` 别名、括号跨行 `from … import (…)`、跨行签名（按逻辑行判定）、
  dict 尾随逗号、相邻字符串隐式拼接、生成器表达式作实参、推导式元组目标、
  `class` 的 docstring、链式赋值、`_` 前缀循环变量与类型名、
  「定义关键字不得退化成语句」（fail-loud）
- **语义层（多数是「静默错值」）**：`f(**mapping)`、`{**a, **b}`、默认参数值、
  推导式循环变量取 iterable 元素类型、闭包捕获变量保真类型、`f = lambda …` 后调用（曾链接失败）、
  裸 `_` 循环变量（曾零次迭代）、字典推导的 `if` 过滤 + pair 位打包改真二元组、
  `type(x)` 编译期折叠
- **健壮性**：CJK 源码的 `is_char_boundary` panic；清掉 3 条 `DBG` 残留
- **工具/文档**：`ZETA_DUMP_PP`、`ZETA_STRICT_PARSE`、测试指令 `// args:` / `// env:`、
  `validate.md` 构建配方更正

### 三、下一步（按价值排序）

1. 语料剩余拦路（未解析行 731）：`jq_shim` 275
   （`get_all_securities`）、`指数ETF动量轮动` 146（`initialize`）、`ETF动量EPO` 77
   （`epo(x, signal, lambda_, method=…)`——参数名与内建 `lambda` 撞名，值得一看）、
   `首板高开-低开-弱转强混合策略` 46 / `追首板涨停` 28（同款 `get_hl_stock`）。按 §一 手法逐个走。
2. `slice(...)`：解析通过但链接报 `_slice` 未定义（语料 5 处）→ 需要「slice 句柄作下标」这条路径
3. 默认参数遗留：未标注形参上的字符串/浮点默认值；`from X import 常量` 绑定
4. 平台桩清单（都可用「删 C 函数」切响亮失败，或由宿主提供）：`py_getitem2`、`py_slice_new`
5. 扁平 python 方言判定、`//=`；泛型函数 `T` 实例化 E2E；`where T: Ord`；文末其余未勾选项

> 附记一：§一 的「块级 leave-one-out」有一处易踩的假象——**只留块头会制造空块假象**。
> 切单元必须**含完整子块**，且先用单条语句逐个隔离过一遍再组合。
>
> 附记三：**隔离 harness 里 stub 不能写成单行 def**（`opens` 要求下一行缩进更深）。
>
> 附记四：**数 `else:` 归属不能「往前找到第一个 for/while」**（14 处 ≠ 严格的 1 处）。
>
> 附记四补：**「完整性校验」不等于语义正确**。给 `a[b.c < d]` 加的「inner 必须整体被消费」
> 会被合法类型列表骗过（`a.b<10, … c.d>0.3`），真正的判据在**语法位置**上（用 `::` 消歧）。
>
> 附记六：**定位单个函数时，先把多行续行当成一个单元**。本轮把 `adjust_stock_num` 的链式三元
> 按物理行切成 4 个单元，得到了一整轮「`return result` 单条就失败」的错误结论；
> 把整个续行链当一个单元后才指向真因。切单元前先看行尾有没有 `\`、括号有没有闭合。
>
> 附记二：**预处理器改写必须在扫描之前，且不能碰三引号引号字符**。本轮先写成「扫描完再改写输出」，
> 结果 `//` 的括号污染已经发生（`{` 没插），效果恰好被抵消；而且复用 `scan_line` 的
> `push_str("\"\"\"")` 把源码里的 `'''` 改成了 `"""`，闭合引号不再匹配、大半个文件被当成字符串——
> 两个 bug 都靠「和 `//`→`/` 的手改副本对拉 + `ZETA_DUMP_PP` 逐行 diff」才现形。
> 预处理器里**任何改写原文本的代码**都要先问：我输出的到底是记账用的 `code`，还是程序本身？

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
- [x] `lambda` → ✓（closure codegen 完成，t12 绿；捕获 V2 + nonlocal V3）
- [ ] P2：`and`/`or`/`not` 别名；`None` 字面量；`range` step
- [ ] **既有缺口（新发现）**：DynamicArray 的 `len()` 运行时桩 `array_len` 恒返 0；多参 print 只输出首参

### PY-5 三引号字符串【P2，完成】
- [x] `"""..."""`/`'''...'''` 解析（既有）+ 预处理器字符串状态机（三引号区域不触发缩进记账/冒号判定/注释剥离，起始行/续行语义对齐 Python）
- [-] f-string 插值——降级不做

### PY 验收标准（三套全绿）
- [x] `tests/python_style/run.sh` **14/15 pass**（+1 known-fail：t12 lambda 依赖 closures codegen）  【历史记录；现状见下方：全套 37/37 绿】
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

### Python 库导入机制 + 并发库（2026-09-13）

**导入机制**（parser 标记 + resolver 收集 + MirGen 映射；注册表本身是数据文件
`pylib/registry.txt`，`include_str!` 嵌入）：
- `import X` / `from X import y [as z]` 不再静默吞掉：parser 发 `zeta_py_import` / `zeta_py_from`
  标记（no-op runtime），Resolver 收集成模块/成员别名表，MirGen 把调用映射到 runtime shim，
  并按成员声明给返回值打**精确句柄标签**（`PyThread`/`PyLock`/`PyExecutor`/`PyFuture`/`PyProcess`/`PyPool`）
- 未知模块/成员 → 明确 warning（`treated as an external shim`），不再是假通过；
  未入表的成员保持链接期失败（fail-loud）而非静默错值
- **通用文件加载**（2026-09-13 补）：注册表未命中时按
  `<源文件目录>/X.{py,z}` → `pylib/X.{py,z}` → `$ZETA_PYLIB/X.{py,z}` → `build/stubs/X.{py,z}`
  搜索并递归编译；模块顶层定义统一加 `X__` 前缀命名空间（per-function rename map 重写
  模块内部裸调用），因此**用户自己的模块可导入**，且模块与主程序（或两个模块）同名函数不冲突
- **注册表数据驱动**：模块/成员/方法、参数类型、返回类型、句柄标签全部写在
  `pylib/registry.txt`；codegen 的 extern 声明由该文件派生（不再手写第二份清单）。
  **加一个库 = 改数据文件 +（如需新原语）加 C shim，不改 Rust** —— 已用 `math`
  （sqrt/fabs/floor/ceil/pow）实测：仅改 registry.txt + C，零 Rust 改动即通过
- **`with` 语义修复**：`with X [as n]:` 此前只做绑定 + 执行 body，**`with lock:` 根本没加锁**
  （fail-open）。现降级为 `zeta_with_enter`/`zeta_with_exit` 路由：库句柄按标签映射
  （PyLock → acquire/release），其他类型回退 identity 并**发 warning 记录**（不静默）
- 顺带修两个**词边界**parse bug（都属同类）：
  ① `ws(tag("import"))` 先吃空格 → 边界守卫永远看到模块名 → **Python `import` 从来没匹配过**
     （一直退化成 `Var("import")` 被丢弃）；
  ② `as` 类型转换无词边界 → `async def` 的 `as` 被当 cast、类型名吃掉 `ync`，
     **静默改写前一条语句**（`async def` 由此损坏，现已可解析）
- 附带：裸函数名作值 → `FuncAddr`（`threading.Thread(work)` 需要），常量名排除在外

**并发库**（C shim 薄封装既有原生原语，`runtime/tokio_runtime_stub.c`）：

| 库 | 实现 | 真实性 |
|---|---|---|
| `threading` | `Thread(target)`/`start`/`join`/`is_alive`、`Lock`/`acquire`/`release`/`locked`、`current_thread`/`get_ident`/`active_count` | **真 pthread**，4×300M 加速 **3.9×** |
| `concurrent.futures` | `ThreadPoolExecutor`/`ProcessPoolExecutor`、`submit`/`result`/`done`/`map`/`shutdown` | **真并行**（V1 每任务一线程，不复用 worker）；4×300M **4.0×** |
| `multiprocessing` | `Process`/`start`/`join`/`exitcode`/`is_alive`、`Pool`、`cpu_count` | Process 走 **fork(2) 真进程**；`Pool.map` 与 futures 同策略（**进程内并行，非真多进程** — 接线 IPC 即可升级） |
| `asyncio` | `run`/`sleep`/`create_task`/`ensure_future` | **顺序语义**：无事件循环，run 内联、sleep 阻塞；**值正确、并发不真**（`gather` 故意未入表，避免静默错值） |
| `time` | `sleep`/`time`/`monotonic`/`perf_counter` | 真（clock_gettime/nanosleep） |
| `math` | `sqrt`/`fabs`/`floor`/`ceil`/`pow` | 真（libm）——**纯数据文件接入，零 Rust 改动** |

Python 侧 `time.sleep(0.02)`、`Thread.join()` 返回值等已验证；f64 返回类型经注册表 `ret` 标注，
避免 i64 位模式误解（`time.monotonic()` 差值直接可用于测时）。

**系统库 vs 用户库：当前分层（2026-09-13 澄清）**

| | 系统库（shim 注册表） | 用户/磁盘模块 |
|---|---|---|
| 判定 | `pylib::find_module` 命中 `pylib/registry.txt` | 注册表未命中 → 磁盘搜索 |
| 符号 | `py_*` runtime shim，带句柄标签 | `mod__name` 前缀 mangled Zeta 函数，无标签 |
| 成员 | 必须入表 | 任意名放行（不存在则链接期失败） |
| 记录 | `py_module_aliases` | `py_user_modules` + stderr 提示 |
| 搜索路径 | 不查磁盘 | 源文件目录 → `pylib` → `$ZETA_PYLIB` → `build/stubs` |

**优先级规则：注册表优先，磁盘次之**（即 `import threading` 即使旁边有 `threading.py`
也用内置 shim；Zeta 侧 `use std::X` 走的是另一套 `module_resolver`，共享 `build/stubs` 目录）。

刚补的两处诊断（此前是静默）：
- 注册表命中且磁盘存在同名文件 → warning 说明「本地文件被内置 shim 遮蔽」
- 注册表模块的未知成员经**属性访问**（`threading.nope()`）→ 与 `from` 形式一致地 warning
  （此前只有裸链接错误 `_nope` 可看）

仍属概念债（未做）：`pylib/` 与 `build/stubs` 在两套机制间共用意念不清；
没有「只用系统库 / 只用本地实现」的显式开关（如 `ZETA_NO_SHIM=1`）。

**第三方库安装（zorb）：最小闭环已通（2026-09-13）**

`target/release/zorb`（`src/bin/zorb.rs`，无新依赖）：
`install <路径|http(s) URL|git URL>`（URL 走系统 `curl`，git 走 `git clone --depth 1`）、
`list`、`remove`、`path`。安装落点 = `$ZETA_PACKAGES_DIR` 或 `~/.zeta/packages` —— 与编译器
**同一个函数**（`pylib::packages_dir()`）取路径，避免两边漂移。目录包（需 `__init__.py`/`__init__.z`）
与单文件模块（`X.py`/`X.z`）都接受，非包目录会明确报错。

编译器侧同步支持：搜索路径加入安装目录；并新增**目录包**（`X/__init__.py`）解析。
E2E 已验证（2026-09-13 实测，含**真实第三方库**）：
- `zorb install ./mypkg` → `import mypkg` / `from mypkg import twice` 值正确（含包内互调）
- `zorb install <GitHub raw URL>` → 真实库 `stringcase.py`（34KB）装入并 `import` 成功
- `zorb install <git URL>`（`https://github.com/okunishinishi/python-stringcase.git`）→ clone 后
  自动识别「仓库根无 `__init__.py` 但有单个模块」并安装；多候选时明确报错（排除 `setup.py`/`test_*`）

**注意边界（2026-09-14 更新）**：这段是当时的观察，现已被后续批次解决 ——
`re` 已实现（POSIX 后端 + Match 句柄 + 可调用替换），`stringcase` 三个入口全部输出正确
（见下方「真实第三方库首次端到端跑通」）。当时的结论仍然成立：**安装 ≠ 可用**，
第三方库能用与否取决于其依赖面是否落在已实现范围内（注册表现 **16 个模块**）。
仍未做：版本/依赖解析、包源索引、校验和、`site-packages` 直连。

**在此之前的状态（2026-09-13 实测，记录备查）**

| 件 | 状态 |
|---|---|
| `zorb` / `zpip` 可执行文件 | **不存在**——Cargo 无 bin target，`target/release` 里只有 `zetac` 与调试工具 |
| `src/package/`（`Manifest`/`DependencyResolver`/`Workspace`/`ZorbClient`/漏洞扫描/签名） | 约 55KB Rust 代码，**编译器零调用点**（只有模块内部互引）→ 死代码 |
| 包源 / 索引 / 下载 / 校验 | 不存在：`ZorbClient` 读的 `cache_dir/index.json` 无生产者，无网络拉取路径 |
| `use @scope/name::X` | 只查本地 `packages/@scope/name/src/mod.z` 与 `~/.cache/zorb/packages/...`（两目录本机均不存在）；失败路径的 `diag_warning!(W2002, "Try running \`zorb install\`")` **实测未输出**（警告被过滤）→ 静默忽略 |
| `use zorb::reqwest` / `serde` / `serde_json` | 硬编码映射到 `build/stubs/external/` 三个桩 |
| Python 第三方库（pip 装的） | **完全不可见**：导入搜索路径只有 源文件目录 / `pylib` / `$ZETA_PYLIB` / `build/stubs`，**无 site-packages / venv / PYTHONPATH**；且不支持目录包（`__init__.py`） |

当前第三方 Python 库只有两条可用路径：① 注册表加条目 + C shim（如 `math`）；
② 把 `.py`/`.z` 放进搜索路径（用户模块，`ZETA_PYLIB` 可指向自定义目录，但需平铺单文件）。

**JSON 静态和类型（路径 A，2026-09-14）**

按约定选择「静态和类型」而非「动态类型」：`PyJson` 是带标签的句柄
`[tag, payload]`，tag ∈ {null, int, f64, str, array, object}（`null` 就是 0，所以
`is None` / `if not j` 天然成立）——与 serde_json::Value / Swift `enum JSON` 同形。

- `json.loads` 现在是**真正的递归下降解析器**（数字按语法区分 int/f64、字符串转义含
  `\uXXXX` → UTF-8、嵌套容器），返回 `PyJson`
- `json.dumps(Json)` **递归且带类型**：嵌套 float/string 不再退化成位模式/指针
  （实测 `{"name":"zeta","ratio":0.75,"ver":2,"nested":{"a":10},"tags":[1,2,3]}`）
- **静态分发**：下标（`cfg["k"]`、`arr[0]`、嵌套 `cfg["nested"]["a"]`）、`len()`、
  `int()/float()/str()`、`print()`（标量裸打、容器打 JSON）、`in`（对象键/数组元素/子串）
  全部按 `PyJson` 类型静态分发，运行期按 tag 取值 —— **不是程序级的动态派发**
- 顺带修 bug：`int(x)` 原先是个 stub（算出了正确的转换函数却没用，直接返回原值），
  只在值本来就是 i64 时"碰巧"正确

**容器值类型侧表（2026-09-14 续）**：map 槽是裸 64 位、无法分辨 2.5 与 2、字符串指针
与整数。编译器在**每次 DictInsert** 记录该值的静态类型（0=int 1=f64 2=str 3=bool）到
按 (map, key) 索引的侧表，dumps/print 查表按类型序列化 —— **不动 map 布局**（整个 runtime 都
依赖它）。实测：`d = {"a":1,"b":2.5,"c":"x","ok":True}` → `json.dumps(d)` =
`{"b": 2.5, "a": 1, "ok": true, "c": "x"}`，`print(d)` 同样正确（不再是指针）。
顺带修：dict 里的 f64 原先被 `fptosi` 截断（2.5→2），现按位模式存。

**列表元素类型（2026-09-14 续）**：列表字面量是同质的 → 元素类型**编译期已知**，直接作为标签传给
类型化序列化器（不需要逐元素侧表，下标会随 push/扩容移动而静态类型不会）。实测：
`json.dumps([1.5, 2.5])` → `[1.5, 2.5]`、`json.dumps(["a","b"])` → `["a","b"]`、
`print([1.5, 2.5])` → `[1.5, 2.5]`（此前是位模式/指针），相应 warning 已移除。

**Json 导航 API（2026-09-14 续）**：`d.get(k)` / `d.get(k, default)`（编译器补默认值参数，并传默认值的
静态类型标签 → 非 Json 默认值被包装成 Json；否则 `print(d.get(k,"x"))` 会把 char* 当 Json 读 tag）、
`d.keys()`（Vec<str>）、`d.values()`（Vec<Json>）。for-in 的循环项类型现在从集合元素类型继承
（仅 Str/Named——F64 元素在槽里是裸位模式），因此 `for k in d.keys(): print(k)` 打印字符串而非指针。
另修：JSON 布尔此前解析成 1/0，`loads("true")` dump 回来是 `1`；现新增 bool tag，回写为 `true`。

**并发原语第二批（2026-09-14 续）**：`queue.Queue`（put/get/qsize/empty）、`threading.Event`
（set/clear/is_set/wait）、`Semaphore`（acquire/release）、`Timer(seconds, fn)`（start/cancel）——
全部基于 pthread mutex+condvar，**阻塞是真的**。支撑性修复：**模块级全局保留静态类型**
（模块级句柄在函数内经 env 读取时值是裸 i64，`q = queue.Queue()` 后 `q.put(x)` 会退化成裸调用；
现由 resolver 记录模块全局类型并在 env 读取与 `py_handle_of` 两处使用）——这同时修好了
`cfg = json.loads(text)` 在函数内使用的情形。顺带：新增的模块全局扫描曾对空接收者链
（`foo().bar()`）做下标访问导致编译器 panic（t25_containers），已加守卫。

**文件对象 + json 文件 API（2026-09-14 续）**：`open(path[, mode])`（内建，mode 默认 "r"）返回 PyFile
句柄；`read/readline/readlines/write/close/closed` + `__enter__`/`__exit__`（**`with open(...)` 现在走真
上下文协议**，不再是无操作兜底）；`json.load(f)` / `json.dump(obj, f)` 复用同一套类型驱动序列化
（dict→侧表、Json→递归、list→类型化）。打开失败在 stderr 报错并返回空句柄，不静默。
顺带修：`with X as n:` 把 enter 结果标成 i64，丢了句柄标签 → 块内 `f.write(...)` **静默无操作**、
文件为空；现 enter 继承接收者的标签。

未做（明确记录）：`items()`、直接 `for x in cfg:`（需按 tag 决定迭代键还是元素）、`seek`/`flush`/二进制模式、
异构列表/嵌套容器的逐元素类型（需要真正的容器值标签或 tagged union 元素）；
`json.load`/`dump` 文件 API；Json 迭代与 `keys()/values()/items()`、`get(k, default)`；
注解驱动的 `loads` 形态。这些是「更多 API」，不是「做不到」。

**真实第三方库首次端到端跑通（2026-09-14）**

`zorb install https://github.com/okunishinishi/python-stringcase.git` 装好的库，
三个入口函数全部给出正确结果：`snakecase("HelloWorld") == "hello_world"`、
`camelcase("hello_world") == "helloWorld"`、`pascalcase("hello world") == "Hello world"`。
此前它连编译都过不去（for 循环无 terminator → `lowercase` 未定义 → 运行期返回 0）。

为此补的三项（都是通用能力，不是为该库打的补丁）：
- **字符串下标/切片**：`s[i]`（负数支持）→ 单字符串；`s[a:b]`/`s[a:]`/`s[:b]` → 子串。
  省略 end 的哨兵从 `-1` 改为 `i64::MIN` —— `s[:-1]` 会被折叠成 `Lit(-1)`，
  两者原本无法区分（`s[:-1]` 于是返回整串）
- **参数类型推断（调用点证据）**：未标注参数默认 i64，导致参数上的字符串操作走数组路径。
  现按证据推断 str/f64：实参是字符串/浮点字面量（或已知 str/f64 的值）即定为该类型；
  被调名按三种方式解析（普通 / `from X import f` → `X__f` / 模块内裸调用 → `前缀+名`）；
  证据收集覆盖 return/let/assign/binaryop；最多 6 轮传播（调用链逐层传递）；
  **只回写被升级的下标**（回写全部会把数组参数写成 `"array(...)"`，造成早期回归）
- **关键字实参按名绑定**（`f(b=2, a=1)` 曾按源码顺序传参，静默给出 201）

顺带修掉：`[dynamic]T{}` 走的是无 header 的 `array_new` 缓冲 → `arr.push(x)` 写到块外、
`arr.len()` 恒 0（`test_while_loop` 一直依赖旧的「意外行为」才通过）；现改用
`zeta_dynarray_new`（[cap|len|data]），且 typed `push` 在 vec_push 重新分配后回写接收者。
slice/len 的 header 读取加了合理性校验，非 Vec 句柄不再触发巨额分配（曾直接 OOM 崩溃）。

**系统库第三批（2026-09-13 夜，自主推进）**

| 库/能力 | 内容 | 证据 |
|---|---|---|
| `datetime` | date/datetime/timedelta/strptime/now/today、`.year/.month/.day/.days/.date()/.strftime()`、`-`/`+` 与全部比较 | 语料最重缺口（timedelta 14、strptime 6） |
| `os` / `os.path` / `os.environ` | join/basename/dirname/splitext/exists/isfile/isdir/abspath/expanduser、getcwd/getenv/system/listdir/makedirs、environ.get/setdefault | 语料 os 5 |
| `sys` | exit/version/version_info/maxsize/path/path.insert/stdout.write/stderr.write | — |
| `json` | **dumps**（编译器按实参类型分发 i64/f64/str/bool/vec/map；字典键靠 hash→字符串侧表还原） | 语料 json 6 |
| `re` | POSIX regcomp/regexec；sub（含 `\1` 反引用与**可调用替换**）、match/search/fullmatch（Match 句柄，`if m:` 语义正确）、split/findall、compile、`group/start/end` | 真实库 stringcase |
| `math` 补全 | exp/log/log10/sin/cos/tan/atan2/trunc/isfinite/isnan | 语料 15 调用点 |
| `logging` | 真打到 stderr；DEBUG/INFO/WARNING/ERROR/CRITICAL 常量、getLogger 句柄 + info/debug/warning/error | 语料 logging 5 |
| `__future__` | `N` 指令：接受的空操作模块 | 语料 5 |

**支撑性机制（都不是一次性 hack，后续库直接复用）**
- 库句柄**运算符分发**：BinaryOp 两侧为 Named 句柄 → 查 `handle_op` 走 shim（date/timedelta 的算术与比较）——此前是对指针做整数运算
- 库句柄**属性读取**：FieldAccess 的**降级后基址**类型为句柄 → 走同一张方法表（`d.year`、`delta.days`、链式 `p.date().year`）
- **模块级语句执行**：`import X` 时执行 `X__init()`（env 幂等守卫），模块级常量对其函数与外部可见；同名常量跨模块隔离
- **返回类型推断**：未标注函数按证据推 str/f64（详见下），返回字符串的库函数不再在调用点被当 i64
- 导入搜索支持**目录包**（`X/__init__.py`）+ `zorb install` 安装目录
- registry 新增 `X`（仅声明 extern，不暴露成员）与 `ret=vec/str` 等类型标注

**顺带修掉的编译器 bug（都是 fail-open 类，有官方用例佐证）**
1. **`!` 被实现成按位取反**（`x ^ -1`）：`not 0` → -1，`if !ok` **恒真** → 3 个官方 string 测试因此报假失败（exit 2/6 → 0）
2. **for 循环里 `break`/`continue` 无目标**：`MirStmt::For` 从不 push `loop_stack` → `if cond: continue` 产生无 terminator 的基本块（LLVM 报错），`break` 直接穿透；现补 `for.inc` 块（`continue` 落在自增，避免经典死循环）
3. **闭包不继承模块 rename map**：模块内 `lambda m: lowercase(m.group(0))` 里的 `lowercase` 未加前缀 → 链接失败
4. **`from X import f` 返回值硬编码 i64**

**2026-09-15 批次顺带修掉的 fail-open（静默丢代码 / 错值 / 崩溃）**
1. **`from X import y` 不跑模块 init**（`25acdb0d`）：成员读模块级状态读到未初始化全局（`total()` 得 1 而非 6）
2. **`@dataclass` 处解析中止**（`6befdec0`）：类与其后**所有**语句被丢弃 → 合成 `__init__` 后修复
3. **注解赋值 `x: int = 5` 解析中止**（`6d11cd24`）：语句只吃掉 `x`，余下 `: int = 5` 无法解析 → 本语句及后续全部丢失
4. **Zeta `var x: T = v` 声明从未被解析**（`6d11cd24`）：`var` 当裸表达式，随后 `x: T = v` 中止所在块
5. **CTFE 无条件删除所有 `comptime fn`**（`6d11cd24`）：数组返回值的 comptime 函数无法物化常量 → 运行期调用悬空；`test_actual_issues` 此前"通过"实为整个函数体被丢弃
6. **多参 `Thread` 把 tuple 句柄当首参**（`c49e4ff8`）：静默垃圾值 → 合成解包适配器
7. **f64 线程参数位模式**（`5968d6cc`）：无 MIR bitcast 原语 → 改为 fail-loud 警告
8. **`for i, v in enumerate(xs):` 整条语句静默丢弃**（`13d3c3d8`）：`parse_pattern` 只接受**带括号**元组；现支持无括号目标，双名 enumerate 解糖为索引循环
9. **`any`/`all`、`zip` 链接失败**（`13d3c3d8`/`f27b33cd`）：现分别实现；`zip` 产出 `(a[i],b[i])` 对以配合元组解构
10. **`print(Path)`/`str(Path)`/f-string 把字符串型句柄打成指针**（`cf903286`）；**`.parent` 被标 i64** 致 `len(...)` 段错误（`02a3c0e5`）
11. **`s.ljust(n)`/`rjust(n)` 落空**（`31fd29ad`）：方法表只在 3 元数注册；注意 fill 是**字符串句柄**（解引用），默认必须传单字符字符串——传字节 `' '`（32）会段错误
12. **`"{}".format(...)`**（`31fd29ad`）：字面量模板重写为 f-string；含格式说明/转换/具名/缺参时返回 None → 落空报错
13. **f-string 格式说明静默错**（`a9387dd4`）：`{x:>8.2f}` 曾把说明文字当结果打出来；现按 Python 语法解析 spec 并手工补齐（f64 入口必须显式声明，否则自动 extern 会 fptosi）
14. **`max/min`、`s.split()`（无分隔符）、`sorted(reverse=)`、`d.items()` 链接失败**（`a3f1e103`/`13a2077b`）：现分别实现
15. **`raise ValueError("boom")` 是空操作**（`2fee5c9f`）：裸 `raise Expr` 退化成裸 `raise` 变量 → 异常从不抛出、其后语句被丢弃
16. **`s * n` / `list + list` / `[0] * n` 全是静默垃圾值**（`813987f4`）：数值运算符直接作用在句柄上
17. **3 段切片 `s[::2]` 解析失败**（`2b0c74d2`）：解析失败并静默丢弃其后所有语句；现支持可选 step（双符号边界规则）
18. **`re.escape` 链接失败**（`40e61a3a`）
19. **幂运算 `**` 崩溃**（`c5c747e2`）：`2 * (*10)` 对字面量做指针解引用 → SIGSEGV
20. **`map`/`filter` 链接失败**（`4f4dd325`）：现 eager 返回列表
21. **列表/字典方法链接失败**（`7c1f7bb7`，loop 子代理落地、我方复核）：根因是数组接收者落进 opaque-fallback → `index`/`count` 被当字符串方法路由；现加专用分发（元素类型感知），mutator 回写接收者
22. **`repr`/`set()`/`split(sep,maxsplit)`**（`4cba298c`）、**`round(x,n)` 强转 i64 / `enumerate(xs,start)` 把 start 当数组下标 / `for k in d:` 静默 0 次**（`78c9b302`）
23. **`int(s,base)` / `replace(old,new,count)` / `os.path.join` 3-4 参 / `dict.fromkeys`**（`4dddfb27`）
24. **`list.sort(key=)` / `sorted(key=, reverse=)`**（`23e54249`）：decorate-sort-undecorate，稳定；`sort(key=f)` 缺参曾是垃圾值
25. **`x in list` 静默返回 0 / `bool(x)` 链接失败**（`4d4c18f4`）、**`print(sep=,end=)` 被当普通实参打印**（`121834e3`）
26. **`min(xs, key=f)` 返回函数指针**（`cb9ba317`）：被「min of TWO values」2 参分支吞掉
27. **`dict.setdefault` / `list.extend`**（`90f0fb53`）；附带记录**既有折叠坑**：`len(字面量尺寸数组)` 是编译期常量 → `extend` 后不会跟着变
28. **`math` 常量静默为 0 且缺函数**（`f0152f78`）：经验——**"注册表未命中的模块成员 → 落到同名 libc 函数"是一类危险模式**（签名不匹配 → 崩溃/UB，比链接失败更糟）
29. **`sys.platform`/`os.sep`/`os.linesep` 静默为 0**（`99f389d7`）；`sys.argv` 先放弃、后凭根因修复启用（`c1e9dc94`）
30. **`os.listdir` 应声明 `ret=vecstr`**（`25aae397`）：`ret=i64` 时结果不带元素类型，成员判断/逐元素比较只能"碰巧"成立
31. **`strip`/`lstrip`/`rstrip` 字符集形式**（`b161def8`）；同批**全表审计**：148 条 `F` + 80 条 `W` 的 `ret=` 与 C 实现返回类型逐条一致（0 不匹配）
32. **`math` libm 常用函数补齐**（`78c96a2e`）：`log2/exp2/expm1/log1p/cbrt/atan/asin/acos/sinh/cosh/tanh/asinh/acosh/atanh/gamma/erf/erfc/fmod/remainder/copysign/nextafter/ldexp/isinf`（`isinf` 是 libc 宏、无符号；`ldexp` 第二参为 i64）
33. **`str.swapcase` 错值 + `str.is*` 谓词族静默 0**（`d8a76934`，loop 子代理落地、我方复核）：`swapcase` 被路由到**大写**函数；`is*` 未注册 → 落到**同名 libc ctype 函数**（`isalnum(int)`）→ 静默 0（见 28 同一模式）。补真 `str_swapcase` + 7 谓词 + `removeprefix`/`removesuffix`。谓词为 **ASCII 语义**（`é.isalpha()` 为 0，Python 为 True），精确 Unicode 分类属已知限界
34. **`hex`/`oct`/`bin`/`reversed`**（`59ab7a3b`）：四者在 libc 均无同名符号；`hex(-255)` 按 Python 输出 `-0xff`
35. **`time.strftime` 崩溃**（`12dbcfce`）：模块级调用未入注册表 → 落到 **libc 的 `strftime`**（签名完全不同）→ 格式串被当指针解引用 → **SIGSEGV**（曾误判为挂起）
36. **构建配方纠正**（docs `b2f017d4`/`cacf817d`）：`tokio_runtime.o` = `ld -r tokio_runtime.c + tokio_runtime_stub.c`（**不含** `py_additions.c`，后者归 `zeta_runtime_c.o`）；混入会造成 **166 个 duplicate symbol**（validate.md §4 已更正）。另：`zetac` 以**相对路径**查对象文件，必须在仓库根目录运行。


**仍未做（本轮新发现，按优先级）**
- [x] **P1 关键字实参按名绑定**：解析保留实参名（`__kwarg__` 标记），调用点按形参名重排；
  未匹配名发 warning 并按位置传（2026-09-14 完成，t51）
- [x] **P1 字符串下标与切片**：`s[i]`/`s[a:b]`/`s[:b]`/`s[a:]` 全部实现（2026-09-14 完成，t52）；
  参数类型推断（调用点证据）让未标注参数上的字符串操作正确分发（t53）
- [x] **P1 `json.loads`**：以静态和类型 `PyJson`（tagged union）实现，真解析器 + 递归 dumps +
  下标/len/int/float/str/print/in 静态分发（2026-09-14 完成，t54）；剩余：文件 API、迭代、容器值标签
- [ ] P2 `re` 补齐：`finditer`/`subn`/`IGNORECASE` 等 flags、`\g<name>`、Pattern 对象的方法面
- [~] P2 库覆盖：`random`/`itertools`/`collections` **已完成**（2026-09-14，t60/t61）；
  `collections` 缺 `most_common`（需 pair/tuple）与 `defaultdict(list/set)`；`typing`/`warnings` 已接入；仍缺 `pathlib`/`functools`/`hashlib`/`dataclasses`
- [ ] P2 `types` 推断继续：容器元素类型、参数类型推断（现在未标注参数= i64，`def f(s): s.upper()` 靠名字回退兜住）

**缺口清单（2026-09-14 复核）** —— 未做项一律保持 fail-loud（链接期失败或 warning），
不得静默产生错值。已完成项在下方「批次记录」里有对应 commit 与用例。

**P1 — 影响真实代码可用性**

- [x] 模块级语义：模块体 import 时执行一次（幂等）、模块级常量对内对外可见、同名常量跨模块隔离（t46）
- [x] **模块级全局保留静态类型**：句柄经 env 读取不再丢标签（`q = queue.Queue()` 后 `q.put(x)` 可用）
- [x] **关键字实参按名绑定**（t51）；**参数/返回类型推断**（t50/t53）
- [x] 字符串下标与切片 `s[i]`/`s[a:b]`/`s[:-1]`（t52）
- [x] 目录包（`X/__init__.py`，t44）；`with open(...)` 走真 `__enter__`/`__exit__`（t58）
- [x] JSON 静态和类型 `PyJson` + 容器值类型（dict 侧表 / list 静态元素类型，t54/t55/t56/t57）
- [x] **相对导入**（`from . import x` / `from .mod import y` / `from ..pkg import z`）、`from X import *` 绑名（t64/t65）：`parse_relative_module` 保留前导点；resolver 用 `__package__` 锚点解析（包 `__init__` 锚自身、子模块锚父包），越界/无包上下文 fail-loud；star import 绑定目标模块公开顶层名（下划线排除）。同批修 `from X import y` 不跑模块 init 的静默错值（`total()` 由 1 → 6）
- [ ] `importlib` / 动态 `sys.path`
- [x] **`Thread(target, args=(...))`**（t62/t63）：单元素经现有 `py_threading_thread_new_2`；2+ 元素在 MIR 合成解包适配器 `fn __tp: target(__tp[0], ...)`（`lower_closure` + `array_get`），入口点单 i64 ABI 不再把 tuple 句柄当首参（原为静默垃圾值）。覆盖位置 target、`target=`、3 参、`threading.Thread` 限定形式
- [x] **自定义 `__enter__`/`__exit__`**（t62_with_user_ctx）：非 shim 类型按静态类型分发到该类型的 `__enter__`/`__exit__` 方法；仅当两者都不存在才回退 identity + warning
- [ ] **异构容器逐元素类型**：`[1, "a"]`、嵌套容器元素（需 tagged union 元素或真正的容器值标签）

**P2 — 库与 API 补齐（按剩余量）**

- [x] `collections.Counter`/`defaultdict`（t61，返回 map）；顺带修 `d[k]=v` 静默无操作
- [x] `random`（t60）、`itertools` 子集 chain/repeat/islice/count（t60，eager）
- [x] `queue.Queue`、`threading.Event`/`Semaphore`/`Timer`（t59，pthread condvar 真阻塞）
- [x] `os`/`os.path`/`os.environ`、`sys`、`datetime`、`re`（POSIX）、`math`、`logging`、`__future__`
- [x] `json` 全套（loads/dumps/load/dump/get/keys/values、嵌套、类型正确）
- [x] 文件对象 `open/read/readline(s)/write/close/closed`（t58）
- [x] `collections` `most_common([n])`（`993e288d`，t79）：**根因是 Counter 未按内容哈希 key**——字符串字面量在不同位置是不同指针，`Counter(['a','b','a'])` 曾产生 3 个条目（5 元素 len=5）。现新增 `py_collections_counter_new_str`（元素类型为 str 时按 `map_str_key` 哈希），`map_keys`/`most_common` 经既有 hash→原串侧表还原文本。`len(c)`、降序计数均已正确。
  ✅ `d.keys()` 的**键类型**已随 map 类型参数走（`3b8d57ae`，t80）：dict 字面量的键类型记进 `Named("map", [Str|I64])`，所以字符串键字典 `keys()` 是 `Vec<str>`（`map_keys` 从侧表还原原串），int 键字典仍是 `Vec<i64>`——两者都不再打哈希/句柄。
  ✅ `most_common([n])` 返回 **`Vec<(key, count)>`**（`64184892`）：key 类型取自 map 的键类型，元组解构按**元素的位置类型**定型 —— `for k, n in c.most_common(): print(k)` 打的是原串。`defaultdict(list/set)` 仍缺
- [x] `typing`（注解专用 no-op 模块，`1914474c`，t66）、`warnings`（warn 真打 stderr，过滤器 no-op，`80867154`，t67）
- [x] `hashlib`（md5/sha1/sha256 + 链式 `hexdigest` + 流式 `update`，CommonCrypto 后端，`99358400`，t69）；
  同批修链式调用接收者（`py_handle_of` 现可从注册表 `handle=` 解析 Call 结果的标签）
- [x] `functools.reduce`（含无 init / 带 init 两种元数，`385dc730`，t71）
- [x] `__file__` 内建（编译期源文件路径，`525bafd5`，t72）—— 解锁 `os.path.dirname(__file__)` 等
- [x] `pathlib`（`Path(str)` / `/` 拼接 / `.resolve().parent` 链 / `.name`/`.stem`/`.suffix`/`.exists()`/`.is_file`/`.is_dir`/`.read_text`/`.write_text`，`02a3c0e5`+`cf903286`，t73）。
  同批：`handle_op` 支持 `PyPath/str`、BinaryOp 句柄分发接受 Str 操作数、两处 FieldAccess 分发改看 `ret_handle`（原来只看标量 ret，导致 `.parent` 被标成 i64）、`py_handle_of` 支持 FieldAccess 接收者、`print`/`str()`/f-string 把字符串型句柄（Path）按 str 渲染（此前静默打成指针）
- [x] `argparse` V1（`089f46b9`，t113/t114）：`ArgumentParser(...)`/`add_argument(...)`/`parse_args()` 与 `args.<field>` 走**编译期重写**——静态方法表枚举不了动态字段名，故 `add_argument` 处按 `type=`/`action=`（缺省看 `default` 字面量）把 flag→kind 记进程序级表（`resolver.rs` walk），`args.<field>` 依 kind 分发到 `py_argparse_get_str|i64|f64|bool`；map key 用 Python 的 dest 规则（去前导 `-`、`-`→`_`），argv 匹配用原始 `--flag`。未声明的 flag → warning 降 0（fail-loud）。`--flag value` 与 `--flag=value` 都支持。
  测试侧：`tests/python_style/run.sh` 新增 `// args:` 指令（argv 传入被测程序）。
  仍缺：位置参数、`nargs`、`subparsers`、互斥组；`required=True` 当前只提示不强制
- [ ] `pickle`（语料 2 处）
- [x] `dataclasses`（`@dataclass` → 按字段注解合成 `__init__`；`asdict` 编译期展开为字段字典，`6befdec0`，t68）。
  同批修 fail-open：`@dataclass` 处解析中止导致类与其后全部语句被静默丢弃；单大写类名（`class P`）仍被注解解析当作泛型类型变量（已知小坑）
- [x] `re` 补齐（`03116dd6`+`8e8d17da`）：`IGNORECASE`/`I`、`MULTILINE`/`M` 常量；带 flags 的 `search/match/fullmatch`(3)/`findall`(3)/`sub`(4)；`finditer`（产出 Match 句柄）；`re.compile` 返回 `PyPattern` 且 Pattern 方法面（search/match/fullmatch/findall/finditer/split/sub）全部可分发；`findall` 返回字符串表；`print(match)` 打匹配文本。同批修**链式方法**标签解析（`pat.search(s).group(2)` 原会落到裸 `group` extern）
  仍缺：`subn`、`DOTALL`/`VERBOSE`（POSIX 后端无法实现，故意不注册 → 使用即报警而非静默忽略）、`\g<name>`、无匹配时 `print(m)` 打空串而非 `None`
- [ ] threading 补齐：`BoundedSemaphore`/`Barrier`/`Condition`/`local`/`enumerate`/`main_thread`、
  `Thread(daemon=)`、`Thread.name`
- [ ] futures 补齐：`as_completed`/`wait`/`Future.exception`/`cancel`/`add_done_callback`；
  `ProcessPoolExecutor` 目前与线程池等价
- [ ] multiprocessing 补齐：`Queue`/`Pipe`/`Value`/`Array`/`Manager`/共享锁；`Pool` 现为**进程内并行**
  （非真多进程，接线 IPC 即升级）、缺 `apply_async`/`imap`/`starmap`
- [ ] queue 补齐：`LifoQueue`/`PriorityQueue`/`full`/`Empty`/`Full` 与 `get/put` 的 timeout 形式
- [ ] Json 剩余：`items()`、直接 `for x in cfg:`（需按 tag 决定迭代键还是元素）
- [ ] 文件剩余：`seek`/`flush`/二进制模式

**工程与形态**

- [ ] `pylib/registry.txt` 是**声明式数据**，新原语仍需写 C → 下一步可让库以 Zeta 源模块实现
  （`pylib/X.z` + extern 包装），复用已有文件加载器，做到「纯 Zeta 库零 C 零 Rust 接入」
- [ ] `src/package/` 死代码处置（`Manifest`/`DependencyResolver` 未接 CLI）
- [ ] 静默路径清理：`use` 失败（W2002）等 diagnostic 实测不输出，需确认是过滤策略还是路径未达
- [ ] 库来源显式化（`ZETA_NO_SHIM=1` 之类）；`pylib/` 与 `build/stubs` 职责分离
- [ ] `zorb` 剩余：版本与依赖解析、包源索引、下载校验和
- [ ] Python 站点包：`site-packages`/`venv`/`PYTHONPATH` 搜索

**L4 — 深水区**

- [ ] asyncio 真并发（事件循环 / 真 `Task` / `gather` / `wait_for` / `Queue` / `Lock` / `to_thread`）
- [ ] `yield` 生成器
- [ ] C 扩展型库（numpy/pandas 真实现）—— 需 CPython 桥，属产品取舍

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
- [x] **数组布局统一**（2026-09-13）：StackArray（字面量数组/元组）改为与 Vec/DynArray 同布局——handle 指向数据区，header `[cap|len]` 在 handle-16；`array_len` 读 header（null 安全）。同时修三处暴露的 bug：① for-in 的 `len_id` 用 `IntLit(0)` 占位被 codegen 常量折叠 → 循环上界恒为 0；② for-in 只接受 `Var` 集合（字面量 `for x in [1,2,3]` 静默 0 次），改为任意集合表达式并先物化到槽位（避免字面量重复分配）；③ `vec_push` 的 `new_cap = cap*2` 在 cap=0 时永远长不大（`[]` 后 append 丢元素），加下限 8；④ `lst.append(x)` 返回值被丢 → 接收者为变量时自动回写句柄。旧行为靠读堆元数据侥幸，现确定性。
- [x] **切片 `arr[start:end]`**（2026-09-12）：end 排他（Python 语义）、省略形式 `[:e]`/`[s:]`；runtime `zeta_slice_vec` 返回 Vec 布局 handle（len/下标通用）；静态数组 end 哨兵编译期替换
- [x] **链式比较**（2026-09-12）：parse_comparison 重写为收集-折叠（`a < b < c` → `(a<b) && (b<c)`，边界操作数复用），is/is not/in/not in 均可入链
- [x] **元组解包全量**（2026-09-12）：`a, b = x, y` 并行赋值 + **`a, b = f()` 调用返回解包**（rhs 降级一次，stack_array_get 取元素）+ **`return a, b`** 逗号元组（match 臂内用单值形式——臂逗号是分隔符，元组需括号）
- [x] **字符串方法**（2026-09-12）：按 receiver 类型分发——upper/lower/trim/strip/lstrip/rstrip/contains/startswith/ends_with/replace/find/count/len/split（runtime `str_split` 返回 Vec 布局 handle）
- [x] **dict 下标/get/in**（2026-09-12，架构修复）：发现 **`lower_expr` 缺 DictLit 分支**（dict 字面量作为表达式时静默变 IntLit(0)，分支只在 lower_ast 语句级 match 里）——补齐并委托；字符串 key 内容哈希（`map_str_key` FNV-1a，同字面量不同 handle 的 key 归一）；`d.get(k)`/`k in d` 走 DictGet 同路径
- [x] **dict `.keys()`/`.values()`**（2026-09-12）：runtime `map_keys/map_values` 遍历开放寻址表 → Vec handle；`.get(k, default)` 双参形式待做
- [x] **try/except/finally/raise 完整语义**（2026-09-12）：setjmp/longjmp 方案打通——生成代码直调 `_setjmp(zt_slot)`（LLVM `returns_twice` 属性）+ runtime `zeta_raise` longjmp 到最近 try 帧（**跨函数立即中断**，真异常语义）。曾以为 longjmp 失效，实为 `except as e` 头解析漏前导空格 → e 绑定缺失 → else 块被判 undef 死代码整体删除。V1 限界：首个 except 捕获一切（类型过滤待做）、无 handler 时 abort
- [x] **装饰器 `@dec`**（2026-09-12）：解析消费忽略（def/class 前合法；语义改写待做）
- [x] **顶层 type alias**（2026-09-12）：`parse_type_alias` 强制分号是坏点——改可选；`type IntList = Vec<i64>` ✓
- [x] `global`/`nonlocal` —— 完成（V3 env 路由 + 模块全局槽隐式读，无需声明）；生成器/yield、async for —— 降级不做

## 执行顺序建议（下一步）

1. ~~f-string + 字符串值语义 + kqueue~~（2026-09-11 完成）
2. ~~class + 方法语义~~（2026-09-11 完成，commit `ad8532ab`）
3. ~~多参 print 修复 + 泛型多类型实例化~~（2026-09-11 完成，commit `24f17a57`/`ca06735e`）
4. ~~closure codegen（解锁 t12 lambda）~~ 完结（t12 绿；捕获 V2/V3 已落地）
5. ~~DUPLICATE_SYM~~ 完结（194/194）
6. 剩余深水区：closures 按引用捕获（V3 nonlocal 显式声明版 + 模块全局槽隐式读已完成；
   闭包内隐式捕获待设计）、`where` 约束检查、WASM 后端、自举

## REasyQuant 真实项目实测（2026-09-13，未修改项目代码）

### 实测更新（2026-09-16，argparse 落地后重测）

parse **38/38**；**link 34/38**（旧的「7 个通过」口径已过时，见下）。
剩 4 个链接失败，按「是编译器缺陷还是外部符号」拆开：

| 文件 | 文件内已定义却报未定义（=编译器缺陷） | 文件内无定义（=平台/外部） |
|---|---|---|
| `code/指数ETF动量轮动.py` | `initialize` | **无** |
| `code/大市值价值优化.py` | `my_trade` | `set_level` |
| `code/蛇皮走位小市值.py` | `consistent` | `set_level` |
| `code/白马股攻防转换.py` | 无 | `debug`/`info`/`set_level`/`get_security_info` |

→ 4 个里有 **3 个**被同一个编译器缺陷卡住，其中 `指数ETF动量轮动.py` **零外部符号**，
修掉该缺陷即应直接可链接。

### ⚠️ 口径更正：语料「链接 34/38」是截断造成的假象，真值就是 7/38（2026-09-16 追加）

`b08d129c` 里我写「重测 parse 38/38、link 34/38（旧的 7/38 口径已过时）」——**这个判断是错的**，
方向刚好反了：34/38 是静默截断的产物。修掉截断（见下节）+ 修掉 `import X as Y` 之后，
解析真正走进文件内容，链接反而**掉到 7/38**：

| 口径 | 值 | 说明 |
|---|---|---|
| 退出码（旧口径） | 34/38 | 多数文件只在第 1 行截断，编出来是个**空 main**，所以能链接 |
| 走进去之后的退出码 | **7/38** | 与 roadmap 2026-09-13 首次实测的「7 个通过」完全一致 |

结论：**7/38 才是真值，2026-09-13 的记录没错**；我上一版把它当"过时"是误判，此处更正。
以后报语料数一律附 `ZETA_STRICT_PARSE=1` 的「完全解析」口径。

### `import X as Y` 别名导入（本轮修复）

语料绝大多数文件第 1~2 行是 `import pandas as pd`。别名分支里那个判空测试用的是
**带前导空格的** `t`（`ws(parse_dotted_name)` 已经把 `as` 前的空格吃掉了，`t == " pd"`），
`t.starts_with(alnum)` 永假 → 别名被跳过 → `as` 被当成逗号列表里的**第二个模块名** →
解析只吃掉 `import pandas `，把 `as pd` 连同其后整份文件留作未解析余量。修法：先 `trim_start`
再判，并要求 `as` 后有分隔空白（避免 `import a asb` 被误读）。回归：`t115_import_alias`。

### 找到「def 被静默拆碎」的机制：定义关键字退化成了语句（2026-09-16 追加，已修）

用探针把 `parse_top_level_item` 的 alt 逐个试一遍，输出谁吃掉了输入、吃了多少字节：

```
PROBE item head="def my_trade(context) {...}" alt=func consumed=1146 rest="def before_market_open(c"
PROBE item head="def my_trade(context) {...}" alt=stmt consumed=4    rest="my_trade(context) {"
PROBE item head="my_trade(context) {...}"    alt=stmt consumed=18   rest="{"
```

真相：`parse_func` 对 `def my_trade(...)` **解析失败**（体内有它啃不动的构造），`alt` 于是退到
`parse_stmt`；而 `parse_stmt` 把 **`def` 当成一个裸标识符**接受（只吃了 4 字节 `def `），
接着又把 `my_trade(context) ` 当成另一条语句 —— 定义被**静默拆成两段碎片**，
`many0` 继续往后啃，直到某个位置彻底卡住，整份文件从那里起被丢弃。

修法（fail-loud）：`parse_top_level_item` 里把定义类解析器单独抽出来，**失败后若输入以
「只能是定义」的关键字开头（`def/class/fn/struct/enum/impl/trait/concept/macro/mod/const/pub`），
就直接返回错误**，不再退给 `parse_stmt`。`if/for/while/match/try/with` 不在此列——模块级语句
仍然要能落到 `parse_stmt`。效果：诊断起点从半截行号变回真正的 `def my_trade(context) {`。

顺带：
- `tests/python_style/run.sh` 支持 `// env: K=V`（严格模式这类用例需要它），并修掉自己引入的
  `env "${envs[@]:-}"` 空数组展开 bug（`env "" prog` 会什么都不跑）。
- 新增 `t116_strict_truncated_def`（`// expect-error` + `ZETA_STRICT_PARSE=1`）锁住该行为。
- **口径变化**：官方退出码 194/194 不变；python_style 117/117；语料**老实口径 7/38 → 9/38**。

### `from X import (...)` 跨行括号列表 + `from x import y as z`（2026-09-16 追加，已修）

语料最大的文件 `jq_wufu.py` 第 43 行是真实 Python 里到处都在用的形式：

```python
if os.environ.get('REPLAYQUANT_LOCAL') == '1':
    from strategies.code.jq_shim import (
        OrderCost, PriceRelatedSlippage, attribute_history, g, get_current_data, …
    )
```

`parse_python_from_import` 原来用 `take_while(|c| c != '\n')` **只取 `import` 之后本行的剩余**，
括号里一个成员都取不到 → 该 if 体解析失败 → 整份文件从第 42 行起被丢弃（丢 1200+ 行）。
修法：识别外层括号，取到匹配的 `)`（import 列表不会嵌套括号），并剥掉行内 `#` 注释。

**顺带修同一类老 bug**：`from x import y as z` 的别名分支也是拿**带前导空格的** `t` 做
`starts_with(alnum)` 判空，别名被静默丢掉（与 `import X as Y` 那个 bug 同源、同修法）。

回归 `t117`：括号 + 注释 + 别名三种形态都验；python_style 117→**118/118**。

**口径说明（重要）**：修好之后语料「退出码通过」从 9 掉回 **7**，这不是倒退 ——
解析走得越深，越多的真实符号被引用，链接才失败；**丢行数是下降的**
（`jq_wufu_daily` 1241→1179、`jq_wufu` 1162→1098、`jq_shim` 721→708）。
以后语料必须同时报三个数：退出码 / 未解析行数 / 完全解析文件数，单看退出码会被反向误导。

修完后的语料分布（38 个文件）：
- **3 个只差平台符号**（已完全解析，LINK-undefined）：`Debug多标的ETF`、`安全摸狗`、`稳健型ETF`，
  缺的是 `set_level`/`DataFrame`/`get_security_info`/`datetime` 这类宿主 API。
- **28 个仍被解析截断**（PARSE-truncated），其中最高频符号是 `set_level`（几乎人人都有）。

### 跨行函数签名（2026-09-16 追加，已修）

缩进预处理原来**按行**判定头部：`opens` 要求「本行以 `:` 结尾 且 本行以块关键字开头」。
签名跨行时 `def` 在第一行、`:` 在最后一行，两边都不满足 → 不改写 → 头部冒号原样透传 →
`parse_func` 解析失败 → 其后整份文件被丢弃。`jq_shim.py` 第 46 行正是这种写法：

```python
def inject_local_data(
    market_df: pd.DataFrame,
    index_df: pd.DataFrame | None = None,
) -> None:
```

修法：在预处理里跟踪**括号深度 + 逻辑行首行**（`logical_head`/`head_indent`），
判定改成「逻辑行结束（深度归零）+ 本行以 `:` 结尾 + 逻辑行首行是块关键字 +
下一代码行缩进 > 逻辑行首行缩进」。单行情形的行为完全不变。回归 `t118`。

同一修法顺带覆盖跨行条件：`if (a > 1\n and b > 2):` 也能正常改写了（一并进了 `t118`）。
语料 3 个最大文件的丢行数继续下降（`jq_wufu_daily` 1179→1163、`jq_wufu` 1098→1082、
`jq_shim` 708→660），语料丢行总数基线记为 **8728**（今后看这个数的变化）。

### dict 字面量的尾随逗号（2026-09-16 追加，已修）

`{"a": 1,}` —— 单行也一样 —— **整个语句解析失败**。根因是 nom 的 `separated_list0`：
它在「最后的分隔符之后没有下一个元素」时会**回退到分隔符之前**，所以尾随的那个 `,`
从未被消费，随后 `ws(tag("}"))` 撞上 `,}` 就失败了。解析失败 → 该语句只被吃到一个裸标识符
（`x`）→ 外层块与其后全部被静默丢弃。

修法：`entries` 非空时显式 `opt(ws(tag(",")))` 再吃 `}`（空 dict 走前面已处理的分支，
所以 `{,}` 这种非法写法不会被顺带接受）。这是 PEP 8 / black 的标准风格，跨行 dict 几乎都带，
真实 Python 命中率极高。回归 `t119`。

效果：`wufu_v1`/`wufu_v2` 的丢行数骤降到 **57 / 58 行**（此前是 400+），
语料丢行总数 8728 → **8695**。它们现在的下一个拦路是 **dict/call 里的 `**` 解包**
（`params = {**STRATEGY_PARAMS, **overrides}`、`WufuConfig(**params)`）与数字下划线
（`1_000_000.0`）。注意：`**` 出现在**签名**里是好的（`def f(**kw)` ✅）。

### `f(**mapping)` 实参解包（2026-09-16 追加，已修 —— 修掉了「静默错值」）

`f(**d)` 此前**能编译但结果是 0**：表达式解析器把 `**d` 当成 `*(*d)`（两次一元解引用），
被调用者静默收到 0 —— 错值 + 零诊断，正是红线点名的「静默错值」，比解析失败危险得多。

修法两步：
1. 解析期：`parse_call_arg` 在退到表达式之前先识别前导 `**`，标成 `zeta_kwargs_unpack` 标记
   （必须早于表达式回退，否则永远被 `*(*d)` 吃掉；同时排除 `***`，避免吃掉幂运算收尾）。
2. 调用点：按**被调用者的参数名**展开成 `f(m["x"], m["y"])` —— mapping 的键运行期才有，
   但参数名是静态已知的（复用既有的 `func_param_names`）。位置实参先占槽、显式 kwarg 覆盖、
   `**` 填充剩余未绑定槽位。签名未知（外部 shim / 方法调用）时**显式警告**，不再静默丢参数。

回归 `t120`：`f(**d)` → 12（此前 0）、`f(3, **{"y": 7})` → 37；
`f(b=2, a=1)`（kwarg 按名重排）与 `f(*xs)`（单星解包）均未回归。
official 194/194、python_style 120→121/121。

### `{**a, **b}` 字典解包（2026-09-16 追加，已修 —— 解锁 wufu_v1/v2）

原样形式的整个 dict 字面量解析失败 → 外层块与其后全部被静默丢弃（`wufu_v1`/`wufu_v2` 各丢 400+ 行）。
三步实现：
1. 解析：`parse_dict_entry` 先识别前导 `**`，标成 `zeta_dict_spread` KEY 标记（值位放占位 `Lit(0)`）。
2. 降级：`DictLit` 遇到该标记就发射 `py_map_update(literal, src)`；仅含 spread 的字面量
   从**源 map 的键类型**取 key_ty，保证 `d.keys()` 仍是 `Vec<str>`。
3. 运行期：`py_map_update` 按**原始 key** 直接遍历源 map 槽位拷贝
   （`base+16 + i*24`，used 标志在 +16）。这一步的关键：**不能**用 `map_keys` 交回的显示文本
   —— 字符串键存的是内容哈希，重新插入显示文本后 `d["x"]` 永远查不到。实测 `{**a}.keys()`
   返回 `["x"]` 正是这条的回归证据。

效果：**`wufu_v1`/`wufu_v2` 完全解析通过**（语料截断文件 34 → **32**，
丢行总数 8695 → **8580**）。回归 `t121`。

注意：运行期改了 `py_additions.c` → 本机需重建 `zeta_runtime_c.o`（该 .o 是 gitignored，
构建配方见 `validate.md` §4）。

### 新发现：set 字面量完全不支持（未修）

`{1, 2}` 和 `{1, 2,}` **都**解析失败 —— 不是尾随逗号问题，是集合字面量整体没有支持
（`AstNode` 里也没有 SetLit，`len(s)` 之类自然无从谈起）。与 dict 字面量共用 `{`，
需要按内容（有 `:` 才是 dict）分派。

### 默认参数值（2026-09-16 追加，**已修** —— 又一处「静默错值」）

修前：`def add(a, b = 10)` + `add(5)` 打 **5**（应为 15）、`def greet(name = "world")` + `greet()` 打 0。
`parse_param` 其实**解析了**默认值（有 `parse_default_value`）却在构造元组时把 `_default` 丢掉，
AST 的 `params` 只有 `(name, type)` —— 于是缺省实参静默当 0，错值且零诊断。

修法（不改 AST，沿用本仓库的标记模式）：
1. `parse_param_full` 保留默认值；`parse_func` 把它作为**函数体前导标记**
   `zeta_param_default(index, value)` 注入（`parse_param` 仍是两元组包装，另外两个调用点零改动）。
2. Resolver 按函数名收集成表（**同时接受 `ExprStmt` 包裹与裸 `Call` 两种体形态** ——
   这个坑害我调了一轮：体里的标记在 register 时已变成裸 `Call`）。
3. MirGen 收表，调用点补槽顺序：位置实参 → 显式 kwarg → `**` 解包 → **默认值**；
   找不到默认值又没给实参时 `warn_unbound` 指名警告（Python 会 TypeError）。
4. 运行期 `zeta_param_default` 是 no-op 桩（`py_additions.c`），否则标记会变成未定义符号。

坑：`kw.is_empty()` 那条分支原本直接 `args.clone()` 短路，**默认值填充根本走不到** ——
必须让它也走槽位路径。

另有两条「kind 不匹配」现在**告警**（此前静默出错值）：无标注形参按 i64 处理，所以
`def f(x = "s")` 会把指针当 i64 打（垃圾数字）、`def f(x = 1.5)` 会静默截断成 1。
警告文案会指出参数名与类型，提示加标注。

回归 `t122`（15/25/123/193/198/15/17 全对）。official 194/194、python_style 122→**123/123**、
语料指标不变（exit-ok 5、截断 32、丢行 8580）——这是语义修复，不动解析面。

### 编译器 panic 修复 + 三元表达式进展（2026-09-16 追加）

**① panic 修复（本轮最重要）**：`find_top_level_kw` 用**字节下标切 `&str`**（`input[i..]`），
遇到中文（多字节）字符串字面量就会 `byte index … is not a char boundary` panic。
REasyQuant 语料里到处是中文 docstring —— 以前解析早早截断、走不到这里，我这几轮的修复让解析
走得更深才暴露出来（表现为整份文件编译崩溃、一个二进制都产不出来）。
编译器**绝不能因源码输入 panic**，已加 `is_char_boundary` 守卫（同文件 `comp_probe` 早有先例）。
`panic 计数` 已加入语料测评口径，本轮归零。

**② 清掉 `parse_conditional_tail` 里的 3 条 `eprintln!("DBG …")`** —— 每次编译都往 stderr 喷调试信息。

**③ 三元表达式（Python `A if C else B`）进展**：机制本来就在（`parse_conditional_tail`），
但三处类型/解析环节断链：
- `If` 作表达式时 **dest 被硬编码成 i64** → 字符串三元 `return "big" if … else "small"` 打出指针数字。
  改为从分支值取真实类型；`process_block` 里还必须**先解包 `ExprStmt`** 再降级，否则拿到的是无条件值。
- 返回类型推断的分类器**不认 `If`**，且函数体里的语句常是**裸表达式**（`ExprStmt` 被规整掉）→ 两者都补。
- `else` 分支改用完整表达式解析 → 右结合嵌套 `1 if n>0 else -1 if n<0 else 0` 可解析。

**④ 三元收尾（2026-09-16 同日追加，已修 → `t124_ternary` 转绿）**：
- **分支里的负号**已修：探针显示 `else=Err(input: "-1\nprint(x)\n")` —— **else 槽位的前导一元负号**
  会让通用表达式解析失败（同一份代码里 then 槽位正常、单独 `x = -1` 也正常）。在该位置加了
  一元运算符回退（解析操作数再包 `UnaryOp`）；`else -1` ✅、`else (+1)`、`else 0 - 1` 均正常。
- 无括号右结合嵌套 `1 if n > 0 else -1 if n < 0 else 0` ✅（else 分支改用完整表达式解析后成立）。

**测评**：official **194/194**；python_style **125/125（0 failed）**，`t124_ternary` 由红转绿；
语料 丢行 8580→**8463**、panic **0**、截断 32。

### `type(x)` 内建（2026-09-16 追加，已修）

两处问题一起修：
1. **`type` 在保留字表里**（`parse_ident` 拒绝它）→ `type(x)` 解析失败 → 外层函数及其后
   整份文件被静默丢弃。把 `type` 从保留字表移除即可（`parse_type_alias` 是**字面匹配**、
   且排在表达式路径之前，所以 Zeta 的 `type X = Y` 不受影响，已验证）。
2. 解除保留后能解析，但会去找不存在的 `_type` 符号（**链接失败**）。
   改为**编译期折叠**成 Python 的类型名字符串（`int`/`str`/`float`/`list`/`dict`/`bool`/`slice`/
   其他→`object`）—— 类型编译器本来就知道，不需要任何运行期反射。

回归 `t136`（5 种类型名）。语料丢行 6171 → **6138**；official 194/194、python_style 136/136、panic 0。

**仍遗留（已定位）**：`slice(...)` 只看不建 —— 解析通过但链接期报 `_slice` 未定义（语料 5 处）。
要真正支持需要「切片以句柄形式作为下标」这条路径（运行期已有 `zeta_slice_vec`，
但下标处需要能识别 slice 句柄），属独立一批。

### 链式赋值 `a = b = expr`（2026-09-16 追加，已修）

解析器吃掉 `a = b` 就把 `= expr` 留在原地 → 语句失败 → 外层函数及其后整份文件被静默丢弃。
`jq_wufu_daily.update_breadth_gate` 里 `below = checked = 0` 正是此形状。

修法要点（第一版我写错了，值得记）：残留的 `= expr` 是**整条链的值**，不是又一个目标。
正确脱糖：右值只求值一次写进第一个目标，其余从左到右复制
（`a = b = 0` ≡ `a = 0; b = a`）。第一版把 `0` 当成目标，于是 `a = b = 0` 打出 1、2 这种
垃圾（槽位号），`below = checked = 0` 后 `checked += 1` 还会连带把 below 变成 1。

**效果**：语料截断 29 → **26**、丢行 6790 → **6171**（−619）；
`jq_wufu_daily` 511 → **388**、`jq_wufu` 509 → **368**（当前最大为 `jq_shim` 445）。
回归 `t135`；official 194/194、python_style 135/135、panic 0。

### 类里的文档字符串（2026-09-16 追加，已修）

类体解析器没有「跳过裸字符串字面量」的分支（**函数体早就容忍 docstring**），于是
`class C:` 后紧跟 `"""doc."""` 会让整个类解析失败 → 类及其后整份文件被静默丢弃。
`jq_shim._LocalContext` 正是这个形状。

修法：类体循环里先用 `parse_primary` 试解析一个裸字符串字面量，命中就跳过（文档不是成员）。

**效果**：语料丢行 6940 → **6790**（−150）；`jq_shim` 595 → **445**（当前最大回到
`jq_wufu_daily` 511）。回归 `t134`；official 194/194、python_style 134/134、panic 0。

### 下划线开头的循环变量（2026-09-16 追加，已修）

`parse_pattern` 的 wildcard 分支用 `tag("_")` 匹配，于是 **`_code` 被吃掉前导 `_`**、
把 `code` 留在原地 → 整个 `for` 语句解析失败 → 外层函数及其后整份文件被静默丢弃。
这正是 `jq_wufu_daily.calculate_global_etf_threshold` 里 `for _code, data in arr.items():`
的形状（块级 leave-one-out 显示该语句单条占 −310）。

修法：wildcard 要求真正的**词边界**（`_` 之后不能跟字母/数字/下划线）。

**效果**：语料截断 30 → **29**、丢行 7594 → **6940**（−654）；
`jq_wufu_daily` 821 → **511**、`jq_wufu` 744 → **509**（当前最大变成 `jq_shim` 595）。
回归 `t132`；official 194/194、python_style 132/132、panic 0。

**同批发现的静默错值（已修，同日）**：**裸 `_` 作循环变量时循环体根本不执行** ——
`for _ in range(3): n += 1` 打 **0**（应为 3）。根因：range 路径只匹配 `AstNode::Var`，
wildcard 于是落到**集合路径**，把 Range 当集合迭代 → 零次。修法：range 路径同时接受
`AstNode::Ignore`（绑定一个丢弃名 `__wildcard`）。`for _ in 0..3`（官方 `minimal_compiler.z:554`
的写法）与 `for _ in [1,2,3]` 一并验证。回归 `t133`。

⚠️ 官方那个用例之所以一直没暴露：`minimal_compiler.z` 本身就在 11 个「静默截断」名单里
（见上文）——**同一个文件既是解析截断的受害者，又藏着被截断掩盖的运行期错值**。

### 推导式里的元组解包 `for k, v in pairs`（2026-09-16 追加，已修）

运行时只给 lambda **一个元素**，所以多出来的名字必须从**元素的槽位**取。此前
`[s for s, p in context.portfolio.positions.items() if p.total_amount > 0]` 这种写法
让整个推导式解析失败 → 外层函数及其后整份文件被静默丢弃。

修法（纯解析期脱糖，不动运行期）：目标解析成名字列表；单名照旧作 lambda 参数；
多名则生成一个合成参数 `__comp_eN`，并把体内（元素表达式 + `if` 条件）对这些名字的引用
改写为 `stack_array_get(__e, i)`（`items()` 的元素本来就是 2 槽句柄）。
列表/字典/生成器三种推导都接上了。

**效果（终于动了语料指标）**：
- 完全解析文件数 **6 → 8**（截断 32 → **30**）
- 未解析行合计 7859 → **7594**（−265）
- `jq_wufu_daily` 859 → **821**

回归 `t131`；official 194/194、python_style 131/131、panic 0。

### 生成器表达式作为调用实参（2026-09-16 追加，已修）

`f(x for x in y)` —— 它的括号**就是调用自己的括号**，所以普通表达式解析只吃掉 `x`，
把 ` for x in y)` 留在原地 → 整个调用解析失败 → 外层函数乃至其后整份文件被静默丢弃。
`jq_wufu_daily._parity_snapshot` 里 `",".join(f"…" for m in ranked[:10])` 正是此形状
（解析器里原本还挂着一条 "PY-A LIMIT: bare genexp as sole argument is NOT supported"）。

修法：解析实参前先做「**深度 0 处是否存在 ` for `**」探测（跳过字符串字面量），
命中则按生成器表达式解析，复用列表推导的 `__collect__(iter, lambda)` 脱糖（含 `if` 的 -1 哨兵）。

回归 `t130`（join/sum、带 `if`、以及 `join(list)` 未回归）。official 194/194、python_style 130/130。

⚠️ **诚实记录：语料总丢行没有变化（仍 7859）**，因为 `_parity_snapshot` 里**还有第二个拦路**：
`[s for s, p in context.portfolio.positions.items() if p.total_amount > 0]` ——
**推导式里的元组解包**（`for s, p in …`）不支持，解析仍停在同一项。下一刀就是它。

### 闭包捕获变量的类型（2026-09-16 追加，已修 —— 上一节遗留项的根因）

`lower_closure` 把**所有捕获的自由变量**硬编码成 `Type::I64`，于是推导式/闭包体内对捕获
字典/列表/字符串的操作全部按整数处理。典型表现（本轮实测）：

```
fields = ["a","b","c"]; d = {"a":1, "b":2}
{f: 2 for f in fields if f in d}    # len 0（应为 2）—— `f in d` 把字典句柄当整数，
                                    # 条件恒假 → 每一项都被 -1 哨兵过滤掉，结果静默为空
```

修法：给捕获名取**父作用域里该名字的真实类型**（`self.name_to_id` → `self.type_map`），
而不是硬编码 i64。修后上述用例 len = 2 ✓，`x["a"]` = 2 ✓，内联 iterable 的情形同样正确。

回归 `t128`。official 194/194、python_style 128/128、语料丢行 7859（语义修复）、panic 0。

**已修（同日追加）**：`f = lambda s: s.upper()` 之后调用 `f(...)` 曾**链接失败**
（`_f` 未定义）。根因：只有 Zeta 的 `let f = lambda …` 路径会设置
`pending_closure_binding`，**Python 的 `=` 路径没设** → `f(x)` 被发射成「调用名为 f 的符号」。
修法：Assign 路径在 rhs 是 Closure 时同样设置该绑定；并新增 `closure_ret_tys`
按闭包名记录 lambda 体的值类型，让 `f(x)` 的结果类型正确
（否则字符串闭包按 i64 打印成指针）。

回归 `t129`（模块级/函数内、字符串/整数、带捕获各一例）。

### 推导式循环变量的类型（2026-09-16 追加，已修 —— 又一类「静默错值」）

**比字典推导更宽的问题**：推导式的循环变量**一律被定型为 i64**，所以只要 iterable 里是字符串，
体内任何字符串操作都在指针上瞎算，且毫无提示：

```
[s.upper() for s in ["ab", "cd"]][0]      # 打印 4365082608（指针），应为 "AB"
[len(s) for s in ["ab", "cde"]][0]        # 打印 33776999900435999，应为 2
{f: 1 for f in ["a"]}["a"]                # 查不到（0），应为 1
```

修法：在调用点把 **iterable 的元素类型**交给推导式 lambda 的参数定型
（新增 `pending_closure_param_types`，`lower_closure` 消费），并让 collect 的结果带
**元素类型**（`last_closure_ret_ty` 记录 lambda 体的值类型）——列表推导产出的
`Vec<str>` 不再是 `Vec<i64>`，`x[0]` 才会按字符串打印。

回归 `t127`。official 194/194、python_style 127/127、语料丢行 7859（本轮是语义修复，
不动解析面）、panic 0。

**遗留（已记录，未修）**：iterable 自身类型未知时（典型：未经标注的**模块级**变量 ——
env 路由会把类型丢掉为 i64），元素类型仍退回 i64，字符串键可能因此哈希不上。
根治需要把模块级变量的类型一路带到 env 读取点。

### 带 if 过滤的字典推导式 + pair 位打包（2026-09-16 追加，已修）

`parse_dictcomp_full` 里明写着「V1: no filter in dictcomp」，所以 `{k: v for k in it if cond}`
的 `if …` 不被消费 → 尾随 `}` 检查失败 → **整个定义连同其后内容被静默丢弃**。
`jq_wufu_daily.get_hist_arrays` 里的
`{f: sub[f].values for f in fields if f in sub.columns}` 正是该形状。

修法（对齐列表推导的哨兵约定）：可选 `if <cond>` → lambda 体为
`if cond { __pack_pair__(k, v) } else { -1 }`，运行期 collect **跳过 -1**。

顺带修运行期的 `__pack_pair__`：原来是 `(k<<32)|v` **位打包**，会把字符串键（指针）
与大整数悄悄弄坏。改成真正的二元组分配（`GC_malloc` 两槽），并在 pack 点对
`Str` 类型的键做内容哈希（对齐 dict 字面量的 `lower_map_key` 规则）。

**仍未修 + 已 fail-loud**：推导式的循环变量被定型为 i64，所以**字符串键**在 pack 点
识别不出是字符串、不做哈希 → `x["a"]` 查不到（但仍 `len` 正确）。已在 collect 处
按 iterable 元素类型检测并**显式告警**，不再静默给错值。根治需要让推导式 lambda 的
参数类型从 iterable 元素类型推断。

效果：语料丢行 8038 → **7859**（−179）；official 194/194、python_style **126/126**、panic 0。
回归 `t126`（整数键 + 过滤 + 无过滤）。

### 相邻字符串字面量隐式拼接（2026-09-16 追加，已修）

`"a" "b"` / `f"x" f"y"`（可跨行）—— 基础 Python，长消息与换行排版里到处都是。
此前第二个字面量**不被消费**，外层调用随之解析失败、**整份文件被静默丢弃**。
修法：`parse_primary` 解析到一个字符串/ f-string 后循环吃相邻字面量并拼接
（全为普通串 → 合并成单个 `StringLit`；否则合成 `FString(parts)`），
`trim_start` 让两半可以跨行（调用内换行正是语料里的形态）。

这也顺手解释了上一轮的 bisect 结果：`jq_wufu_daily.initialize` 体里那条跨行 `log.info`
就是本类。**修后该文件丢行 1163 → 948**，语料总丢行 8463 → **8038**（−425）。

回归 `t125`。official 194/194、python_style **125/125（0 failed）**、panic 0。

**⑥ 语料下一个拦路的分布（2026-09-16 追加，逐函数「体换 pass」测收益）**

`jq_wufu_daily` 当前丢行 948，把每个顶层 def 的体换成 `pass` 看收益，收益最大的几个：

| 收益(行) | 函数 | 换 pass 后剩 |
|---|---|---|
| −155 | `get_final_ranked_etfs` | 793 |
| −89 | `get_hist_arrays` | 859 |
| −80 | `update_sector_pool` | 868 |
| −47 | `check_a_share_weak_period` | 901 |
| −47 | `calculate_global_etf_threshold` | 901 |

即**拦路是多个、分散在不同函数里**，不是单点。对 `get_final_ranked_etfs` 做体内前缀二分时，
最小的可复现前缀是「docstring + `if not g.merged_etf_pool:`」这两行 —— 但**脱离语料上下文写不出来**
（最小文件里同样的三行完全正常），所以又是一次「上下文相关」的触发，单独复现不可靠。
下一轮不要再从「最小前缀」切入，改为**在真实文件里做块级 leave-one-out**（保留上下文），
或先给这一层加「解析失败即指名报告位置」的诊断，靠报错定位。

**⑤ 语料仍未动**（诚实记录）：把 `jq_wufu_daily.initialize` 的体换成 `pass` 后丢行 1163 → **948**，
说明该函数体只占约 215 行，**下游还有别的拦路**。下一轮就从 `initialize` 体内部逐段二分，
再顺着剩下的 948 行继续找。

### 注解：点号限定类型 + PEP 604 联合类型（2026-09-16 追加，已修）

`def f(df: pd.DataFrame)`、`x: a.b.c`、`X | None`、`list[str] | None` —— 两种注解此前都让
**整个定义**解析失败（`parse_type` 只认简单名与 `A<B>` 形式）。修法：
- 点号路径：基础类型后接 `(.ident)*`，原样保留为不透明类型名；
- 联合类型：`| <type>` 可重复，取**第一个不是 `None` 的分支**（Zeta 无联合类型，
  而 `Optional[X]` 运行时就是 X）。

回归 `t123`（点号 + 多行签名 + 联合 + 默认值混用）。official 194/194、python_style 123→**124/124**。

⚠️ **诚实记录：语料指标一个都没动**（still exit-ok 5、截断 32、丢行 8580）。
原因值得记住：`ensure_fully_parsed` 的定位是「many0 停下的那一项」，而 `many0` 停在**项起点**，
所以余量文本从 `def …` 开始 —— 看起来像签名问题，实际是该 def 的**函数体**里有它啃不动的构造。
下一轮不要再被余量首行误导：直接在函数体内部二分（把体逐段替换成 `pass`，用 W1002 判据）。

### 遗留：默认参数值的两条限界

- 无标注形参一律 i64 ⇒ 字符串/浮点默认值不可用（有告警，不再静默）
- 默认值表达式在运行期仍被求值一次（前导桩调用的实参），字面量场景无害

```
def add(a, b = 10): return a + b
print(add(5))        # 打 5，应为 15
def greet(name = "world"): return name
print(greet())       # 打 0，应为 world
```

`fn add(a: i64, b: i64 = 10)` 同样如此；两个实参都给时正常（`add(5,20)` → 25）。
即「缺省填充」这条路径整体失效，且字符串默认值也一样（说明不是类型问题）。
roadmap 早先记的「默认参数值已落地」与实际不符，此处更正并记为待修。

### 另外两个已定位的小缺口（未修）

- `from math import pi` + `print(pi)` 打 **0**：`from X import 常量` 的成员没有绑定成裸名
  （函数成员 `from math import floor` 正常）。常量走的是另一条解析路径，待查。


`def X(...):` 后**函数体没有任何语句**（只有注释/空行）时，缩进预处理不会把头部改写成
`{` —— `opens` 要求 `next_code_indent(&infos, i+1) > indent`，而此处下一个「代码行」是缩进更浅的
下一个 def，于是条件为假、行原样透传（`def X(...):` 带冒号），解析必然失败。
Python 里空函数体本身是语法错误，语料未必命中，故先记着。

### 后续两个靶子（按语料命中面排序）

1. **`def X(...)` 头被吃掉、函数体 `{` 留在余量里**（`蛇皮走位小市值.py`、`五年15倍年化79.py`、
   `大市值价值优化.py` 均命中，是当前语料最大的一类）。余量形如
   `'{\n    yesterday = context.previous_date\n    now_time = …'`——说明有某个备选分支
   **成功消费了 `def X(...) ` 却没消费 `{`**（不是 `parse_func`：它失败会回溯，余量就该从 `def` 开始）。
   下一步直接在 `parse_top_level_item` 的 alt 里逐个二分，找出到底是谁吃掉的。
2. `jq_wufu.py`：余量从顶层 `if os.environ.get('REPLAYQUANT_LOCAL') == '1' {` 开始
   （该 `if` 整体解析失败，疑似体内 `from … import …` 或 `os.environ.get` 形态）。

### 静默截断的根因找到了：`many0` + CLI 丢弃 `remaining`（2026-09-16 追加）

`parse_zeta` 用 `many0(alt(...))` 解析顶层项——**`many0` 在第一个解析不过去的项处「成功」停下**，
把后半截留在 `remaining` 里。`src/lib.rs:100` 一直有「remaining 必须为空」的检查，但
**CLI 路径（`src/main.rs`）把 remaining 丢掉了**（`Ok((_remaining, asts))` /
`// discard remaining slice`）。结果：CLI 只编译文件前缀，被丢掉的定义在链接期变成
「undefined symbol」，而编译器**一句诊断都不报**。

已修（本批）：CLI 两处都加 `ensure_fully_parsed()`——
- 默认：**W1002 警告**（保留 194/194 退出码口径，但不再沉默）
- `ZETA_STRICT_PARSE=1`：**E1002 报错**（老实口径，用来量「到底编译了多少」）

⚠️ **这个修法暴露了一个更严重的事实：两套基线数字都掺水。**

| 口径 | 旧数字 | 实测（`ZETA_STRICT_PARSE=1` / 警告计数） |
|---|---|---|
| 官方套件 | 194/194 | 194/194 退出码，但 **11 个文件有未解析尾巴** |
| REasyQuant 语料 | 链接 34/38 | 退出码 34/38，但 **37/38 个文件有未解析尾巴** |

官方套件的尾巴规模（最狠的几个）：

| 文件 | 丢掉的尾部 |
|---|---|
| `minimal_compiler` | **757 行**（整个 `impl Parser`） |
| `benchmark_simd_vs_scalar` | 357 行 |
| `selfhost` | 158 行（`impl Parser for ZetaParser`） |
| `test_suite` | 127 行 |
| `advanced_patterns_test` | 98 行 |

语料侧更极端：**大多数文件在第 1 行就断**，即整份文件（几百行）全被丢掉，
"链接成功"的 34 个二进制里很多只是个空 `main`。

两个最大的失败类（下一步的靶子）：
1. **`import pandas as pd` —— Python 的 `as` 别名 import 不支持**。解析器吃掉 `import pandas `
   后在 `as pd` 上停住，于是**整个文件从第 1 行起全丢**。语料里绝大多数文件第 1~2 行
   就是 `import pandas as pd` / `from jqdata import *`，所以这**大概率是语料侧性价比最高的单点修复**。
2. **已定位但未缩到最小**：某些 `def X(...)` 头被解析成功后，紧随的 `{` 连同函数体被留在
   remaining（`蛇皮走位小市值.py`、`大市值价值优化.py` 都命中），下面那节记录的 A 缺陷即此类。

**口径更新**：以后报数一律同时给两个口径——「退出码」用于守红线，「完全解析」
（`ZETA_STRICT_PARSE=1`）用于判断真的编译进去了多少。

**缺陷 A：某个 def 之后的所有顶层定义被静默丢弃（触发条件已压缩，根因未定）**
`大市值价值优化.py` 共 7 个顶层 `def`，目标文件里只有 **4 个 T 符号**：`my_trade` 变成 `U`
（被 `initialize` 引用但无定义），其后的 `check_limit_up`/`check_stocks`/`filter_*` **连引用都没有**
——说明在 `my_trade` 处中断后，后续顶层项整体未被处理，且**全程不报错**。

可重复的定位方法：把除目标函数外的所有顶层 def 体替换成 `    pass`，再逐个还原体——
`restore_my_trade` → 只发射 **2/7**（`my_trade` 自身变 `U`，其后全丢）；
`restore_check_stocks` → **4/7**（`check_stocks` 变 `U`，其后全丢）；其余 5 个还原后仍 7/7。
即「**某个函数体会触发一次静默的线性中断**」，中断点之后的一切都被丢弃。

已排除的假设：
- **不是体积/条数限制**——给体加 10 条重复语句或 10 个不同局部变量，均正常发射。
- 不是单一语句形状：`my_trade` 体分两半各自都正常，**合起来**才中断。
- 体内部定位：三组语句里只有删掉 s3（`if current_data[stock].last_price <
  current_data[stock].high_limit: … else: …`，文件 76–80 行）才恢复发射。但 s3 **单独**当体 ✅、
  s3 + `x = len(context.portfolio.positions)` ✅、s3 + `value = … / (…)` ✅，而
  **s3 后面再跟任意一条块语句**就中断：`s3 + if …: pass` ❌、`s3 + for …: continue` ❌。
- 脱离语料上下文后**最小独立复现未成功**，根因**尚未定位**（所以下批要先加「降级失败必须
  fail-loud」，把静默丢弃变成指名报错，再顺着报错定位）。

⚠️ 更正：上一版记录（commit `b08d129c`）曾断言「函数体降级失败导致 def 被静默丢弃」——
那是**未经证实的推断**。实测把 `my_trade` 体换成 `pass` 后该 def 确实恢复发射，但**其后的顶层 def
仍然全丢**，所以「体降级失败」解释不了尾部的丢弃；该断言已作废。

**缺陷 B（顺带发现，可脱离语料复现）：缩进跳跃 + 块语句导致语句顺序错乱**

```
def f():            # 函数体首行 8 空格（非常规缩进）
        if 1 < 2:
            print(1)
        else:
            print(2)
    if 2 > 1:       # 缩回 4 空格后再次起块
        pass
    print(3)
```

编译运行输出 **`3` 然后 `1`**（应为 `1` 然后 `3`）——语句被重排；同内容改成规整缩进
（4/8 一致）则输出正确。两种情况都会打 `W0003 Typecheck failed (non-fatal)`。
疑似 PY-1 缩进预处理在「块内缩进 8 → 降到 4 → 再起块」时的块闭合/排序处理有误，
与缺陷 A 的中断可能同源，待一并查。

**缺陷：`log.*` 平台日志面**（`set_level`/`debug`/`info`，3 个文件命中）——按下面的
「接入不迁移」层处理（runtime logger shim），与本条编译器缺陷相互独立。

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

- `print(bool)` 打 `0`/`1`，Python 打 `True`/`False`（t113/t114 即按现状 0/1 断言）。
  改动会波及既有若干用例的期望值，需单独批次评估，暂不混入其他修改
- stddev 不能走 `as i64` 中间步（截断为 0）
- 无 `-o` 模式 JIT 对简单 f64 程序 segfault（AOT 路径正常，低优）
- benchmark_simd_vs_scalar.z 源文件本身损坏（大量孤立 `}`），非编译器问题
- **静默错值（已修 2 项，见批次十二；以下仍待修）**：
  ① ~~f64 数组字面量之后再写字面量比较~~（已修，见批次十三；实测与数组无关，
     是任何浮点比较赋值给变量都会中招）；
  ② 未标注形参按 i64 处理（`def f(xs): return xs[0]` + `f([7,8])` → 0，需显式 `xs: [i64]`）
     —— 属参数类型推断设计问题，需要单独批次；
  ③ 嵌套列表相等只比外层句柄（`py_list_eq` 不递归）

## 库管理现状与 Python 库接入方案（2026-09-13 评估）

### 现状盘点：库管理"有名无实"

| 机制 | 现状 | 判定 |
|---|---|---|
| `use std::X` 模块解析 | Resolver 递归查找 build/stubs/std/X.z ✓ 文件存在 | 解析 ✓ |
| stdlib 桩（collections.z 等 12 个） | 桩内容是空壳 struct + no-op 方法（HashMap.insert 返回 None），无 runtime 支撑 | **有名无实**——`HashMap::new().insert(1,100)` 编译通过但链接失败（方法解析为裸名 extern `insert` 而非 `map_insert`） |
| zorb 包管理器（@scope/name） | 目录约定 + ~/.cache 缓存查找已写，但无 zorb 二进制、无包源 | **空架子** |
| `import X` / `from X import y`（Python 语法） | 注册表 shim + **磁盘文件加载用户模块**（同目录/pylib/$ZETA_PYLIB，.py/.z）；未知模块发明确 warning | ✓ L1 |
| Python 库接入 | L1 注册表 + C shim 已落地（threading/futures/multiprocessing/asyncio/time）；无 CPython 嵌入 | **部分**（L3 shim 路线） |

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
| with | 272 | ✓ 解析 + desugar | L1 ✓ |
| starred `*args` 展开 | 268 | ✓（V1 静态数组展开，调用点编译期 unroll） | L1 ✓ |
| walrus `:=` | 59 | ✓（desugar → Assign；括号/裸两种形式） | L1 ✓ |
| genexp `(x for x in y)` | 222 | ✓（同 listcomp desugar，V1 牺牲惰性；裸 genexp 受 parse 架构限制） | L1 ✓ |
| yield/async def/await | 197/25/15 | `async def`/`await` 可解析（`as` 词边界 bug 已修），asyncio shim 顺序语义；yield 生成器仍 ✗ | L4（协程状态机，深水区） |
| lambda | 130 | ✓（V2 捕获） | L1 |
| global | 65 | ✓（module 全局槽隐式读，无需显式声明） | L1 ✓ |
| dictcomp/setcomp | 39/11 | ✓（setcomp 输出 Vec，去重待做） | L1 ✓ |
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

已完成：~~f64 数组 layout 统一~~、~~Python 库导入机制（含通用文件加载 + 数据驱动注册表）~~
、~~四个并发库 + math~~、~~`with` 语义修复~~（均 2026-09-13，python_style 43/43 / 官方 194/194 /
语料 38/38，退出码零差异）。

下一批按「缺口清单」优先级（见上）：

1. ~~**P1 模块系统与语义**~~（2026-09-15 完成）：相对导入 + star import + `from X import y`
   跑模块 init（commit `25acdb0d`）、`Thread(target, args=(...))` 多参（`c49e4ff8`）、
   自定义 `__enter__`/`__exit__`（`0c3a2ed6`）
2. **P2 并发 API 补齐**：`queue` 模块 → threading（Event/Semaphore/Barrier/Condition/Timer）→
   futures（as_completed/wait）→ multiprocessing（真多进程 + Queue/Pipe/Value）
3. **P2 库覆盖**：os/sys/json/re/collections/itertools/... 逐个按需接入（现在是数据文件 + C，
   或纯 Zeta 模块）
4. **P2 注册表形态升级**：库实现为 `pylib/X.z`（extern + 包装），让纯 Zeta 库零 C 零 Rust 接入
5. L1 收尾：exp/log/polyfit runtime
6. L2 mini-DataFrame：`dataframe.z` 桩 + native runtime（列存 Map+Vec）
7. 嵌套 def 方法分发修正（stub 方法解析为裸名 extern 的 bug——HashMap::new().insert() 应路由到 map_insert）
8. L3 shim 边界：REasyQuant 引擎侧提供 jq_shim.o（或确认现有 no-op 桩足够）
9. **L4**：asyncio 事件循环与真协程、yield 生成器

## argparse 设计与风险界定（2026-09-16，实现前定稿）

**语料证据**（REasyQuant strategies，38 文件）：`ArgumentParser(` 4、`parse_args()` 4、`add_argument(` **34**、`args.<field>` **57**；
`add_argument` 的关键字分布：`default` 25、`type` 15、`help` 12、`action` 7、`required` 2、`choices` 1。

**核心难点**：`args.<field>` 是**动态字段名**，注册表的静态方法表无法枚举；且字段类型（str/f64/i64/bool）必须**编译期确定**，否则 `args.cash` 这类浮点字段会被当整数（静默错值）。

**方案：编译期建「flag → 种类」表 + 运行期命名空间存字符串**

1. **编译期（gen）**：`X.add_argument("--flag", ...)` 在**降级点**按**名字**读关键字（`default`/`type`/`action`），记录 `flag → kind`：
   - `type=float` → f64；`type=int` → i64；`type=str`/缺省 → str（再由 `default` 的静态类型兜底：字符串字面量→str、浮点→f64、整数→i64）
   - `action="store_true"` → bool
   - 发射 `py_argparse_add(parser, "--flag", <default 的字符串形式>)`（数值默认值在运行期用 `to_string_*` 转成字符串）
   - `help`/`required`/`choices` **接受但不实现**（V1）：`required` 缺失时**发 warning**（fail-loud 不静默），`choices`/`help` 忽略并在文档记录
2. **运行期（C）**：
   - parser = `Vec<[flag_name, default_str]>`（用 Vec 而非 map，因为需要**按名字遍历**去 argv 里找）
   - `parse_args()` → 扫 `argv`（由启动构造器捕获，见 §29）匹配 `--flag value` / `--flag=value` / 裸 `--flag`（后者取值 `"1"`，服务 `store_true`），命中即覆盖默认值；结果是 `map: name_hash → 字符串`
   - **取值按种类分入口**：`py_argparse_get_str/i64/f64/bool`（f64 入口必须在 codegen 显式声明，否则自动 extern 会按 i64 声明并 fptosi——老坑）
3. **`args.<field>`**：gen 在 FieldAccess 处识别「基址类型为 `PyArgNS`」，查编译期表拿 kind，发射对应的取值入口并按 kind 定型。

**V1 明确不支持（保持 fail-loud / warning）**：短选项 `-x`、位置参数、`nargs`、子命令（`add_subparsers`）、`type=` 传可调用对象、互斥组。**不静默**。

**风险与验证**：① 编译期表是**按解析器实例**还是全局？V1 用**全局 flag→kind**（同名字段跨解析器取后者，并在冲突时 warning）；② 未知 flag 的 `args.<x>` 必须**编译期报错/warning**，不得降级成 0；③ 用例需覆盖 `--flag value`、`--flag=value`、`store_true` 缺省/存在、数值/字符串默认值、`required` 缺失 warning。


## 批次二十（2026-09-17，**分析结论：_N 族不能用便宜的本地规则修**）

`*_N` 幽灵符号（`host_str_replace_4` / `py_json_dumps_i64_3` / `py_logger_info_5` /
`py_logging_*_N` / `host_str_count_1`，13 调用点）的产生链已查明：

1. 接收者类型未知（未标注形参 → type_map 默认 i64），于是走**通用「方法名 → 方法」表**
   （`gen.rs:8514` `"replace" => ("host_str_replace", 3, "str")`）；
2. 实际参数个数（receiver + 3 个 kwargs = 4）与该表声明的 arity（3）不符；
3. codegen 的 arity 消歧（`codegen.rs:2075/2411/2441/2551/2657`，豁免条件只有
   `starts_with("zeta_")`）给它加后缀 → `host_str_replace_4`。

**两条便宜修法都被否掉**：

| 修法 | 否掉的理由 |
|---|---|
| 把 `host_`/`py_` 也加进 arity 消歧的豁免前缀 | 会让 `date.replace(year=…)` 真的调 `host_str_replace`（str 语义）作用在**日期句柄**上 ⇒ **链接通过但运行时静默错值**，正好撞本项目红线 |
| arity 不匹配时改报编译期错误 | 只是把未定义符号 `host_str_replace_4` 换成 `replace`，**链接仍然失败**，没有净收益 |

**真正的修法只有一条**：让编译器知道接收者的类型（`d` 是 `PyDate`）。也就是回到参数类型推断。
两条具体路径：
- **(a) 调用点驱动推断**（最小可用版）：无标注形参若在所有调用点都收到同一"类"实参
  （如 `date(...)` / `datetime.date(...)` 的返回值），就把该形参定型为该 handle 标签；
- **(b) 关键字形状分派**（局部、显式规则）：`x.replace(year=…, month=…, day=…)` 的 kwargs 集合
  与 `date.replace` 签名唯一对应，可据此路由到 `py_dt_replace`。
  注意这需要先补 `py_dt_replace`（**已试作并验证可编译进 `tokio_runtime.o`**，
  当时因为接收者类型未知而根本没被走到，遂回滚）。

⚠️ 副产物结论：`*_N` 族与「未标注形参按 i64」「`xs[0]` 返 0」「`in` 早期恒 0」**同根** ——
都是「类型未知时的兜底分派」。清它等于做参数类型推断。


## 批次二十一（2026-09-17，**已修** `ec95eab9`）：`date.replace` 的 kwargs 形状分派

上一轮否掉两条便宜修法后，选了**唯一不会引入静默错值**的那条：`x.replace(year=, month=, day=)`
的 kwargs **集合**唯一标识 `date.replace`（str.replace 只吃位置参数），据此在调用发射点分派。

- 运行时补 `py_dt_replace(h, y, m, d)`：未给字段以 `<=0` 表示 ⇒ **部分替换语义正确**
  （`d.replace(year=2021).month` 仍为 5）；同步重建 tracked 的 `tokio_runtime.o`。
- 注册表 `W PyDate replace py_dt_replace args=4 ret_handle=PyDate`；gen.rs 按 kwargs 形状分派。
- 实测：`date(2020,5,6).replace(year=2021,month=1,day=1).year` → 2021；
  `.replace(year=2021)` → year+month = 2021+5 = 2026。
- 度量：未定义符号去重 **86 → 85**，`host_str_replace_4` **归零**；python_style 157 → **158**；
  官方 194/194；解析 37/38 不回退。
- 教训沉淀：这类「类型未知时的兜底分派」问题，**用调用点可观测的形状（kwargs 集合）消歧**
  是成本最低且不牺牲语义的做法；不要用「豁免前缀」那种会引入假语义的捷径。

### 剩余结构性两项（都不是补丁，需要单独批次）
1. **参数类型推断**（未标注形参）：它是 `*_N`、`xs[0]` 返 0、`in` 恒 0 等一系列问题的**共同根因**。
   两条路径已记录（调用点驱动推断 / 更多「形状分派」规则）。
2. **运行时对象的构建与跟踪**：`tokio_runtime.o` 被 git 跟踪而 `zeta_runtime_c.o` 未跟踪
   ⇒ fresh clone 只有一半运行时（`main.rs` 的 `if exists()` 链会静默跳过）。
   建议加 `build.rs`/Makefile 目标后 `git rm --cached tokio_runtime.o`，并让 CI 先构建。


## 批次二十二（2026-09-17，分析结论）：`_N` 族的**第二个**成因——注册表模型表达不了 Python 的可选/可变参数

批次二十查明的成因是「接收者类型未知 → 通用方法名表 arity 不匹配」。本轮把剩下的 `_N` 符号
逐个对到语料调用点，发现它们其实是**另一类**问题：

| 幽灵符号 | 语料调用形态 | 冲突点 |
|---|---|---|
| `py_logger_info_5` / `py_logger_info_4` | `log.info('sell', s, cdata[s].name)`、`log.info(fmt, x, y)` | Python 的 `Logger.info(fmt, *args)` 是**变参**；注册表只声明 `W PyLogger info … args=2`（C 侧也只有 2 参） |
| `py_json_dumps_i64_3` | `json.dumps(v, ensure_ascii=False, indent=2)` | 注册表 `F json dumps … args=i64` 只有**位置参数**，没有可选关键字 |
| `py_logging_getLogger_0` | `logging.getLogger()` | 注册表声明 `args=i64`（必填），Python 里是**可选** |
| `py_logging_FileHandler_2` / `host_str_count_1` | 同类可选/变参形态 | 同上 |

**结论：注册表的「一个成员 = 一个固定 arity」模型表达不了 Python 的
「可选参数 / 变参 / 仅关键字参数」**。所以 arity 消歧（`codegen.rs` 那 5 处）对这些名字
必然产生幽灵符号。

**三条可选修法（按成本排序）**，都需要单独批次：
1. **注册表支持多 arity 别名**（数据模型扩展）：同一成员登记多个 `args=` 变体，
   各自映射到不同的 C helper（如 `py_logger_info2` / `py_logger_info_n`）。
2. **调用点改写**（沿用本轮 `date.replace` 的成功经验）：按**可观测形状**改写 ——
   `log.info(a, b, c)` → `py_logger_info_str3(a, b, c)`（跳过 Python 的 %-格式化语义时），
   `json.dumps(v, **kwargs)` → 读取已支持的关键字（`indent`/`ensure_ascii`）后调用固定 arity 入口。
3. **fail-loud**（最小动作）：arity 不匹配且名字是运行时前缀（`py_`/`host_`）时，
   在编译期报一条「该运行时入口只接受 N 个参数」的错误，而不是发一个不存在的 `_N` 符号去链接期爆。
   ⚠️ 不能改成「豁免前缀直接调用」——那会把 5 个参数压进 2 参的 C 函数（ABI 不匹配 ⇒ 静默错值）。

> 注意与批次二十的结论并不矛盾：`date.replace` 那 5 处属于「类型未知」（本轮已用形状分派修掉），
> 这一批属于「注册表表达不了 Python 签名」。两类都在 `_N` 名下，但修法不同。


## 批次二十三（2026-09-17，已修）：占位符符号 py_asdict_unexpanded 改为响亮失败

`pylib/registry.txt:208` 把 `dataclasses.asdict` 指向 `py_asdict_unexpanded`，而这个符号在 C 与 src 里都不存在（注释写明 asdict 本应在编译期重写）。于是重写不适用时编译器发一个幽灵符号出去，整个程序在**链接期**失败，报错 `_py_asdict_unexpanded` 对使用者毫无意义（语料 3 处）。

修法：在 `runtime/tokio_runtime_stub.c` 实现该符号 —— 打印可读原因并 `abort()`。于是：① 同一文件的其他代码仍可编译链接；② 只有真走到未展开路径才失败；③ 绝不返回假值（loud failure，不是 fail-open）。同步重建 tracked 的 `tokio_runtime.o`。

度量：语料未定义符号去重 **85 → 84**，`py_asdict_unexpanded` 引用归零；python_style **158/158**；官方 **194/194**；解析 37/38 不回退。

> 同一类「registry 指向不存在符号」一律照此办理：给一个**会响的**实现，不让它去链接期爆。
> 检查办法：把 `registry.txt` 里出现的符号名去 `nm -g --defined-only tokio_runtime.o zeta_runtime_c.o` 里查，查不到的即占位符。


## 批次二十四（2026-09-17，巡检 + 分析）

### ① registry 占位符巡检：**已清零**
用批次二十三记下的办法（registry 里的符号名 ∩ nm 运行时库）全量跑了一遍：

- registry 带符号的条目 **292 个**；两个运行时对象已定义符号 **587 个**
- **不在运行时库里的条目：0** ⇒ 上一个自造符号（`py_asdict_unexpanded`）修完后，
  registry 里已没有「指向不存在符号」的占位符。
  （检查脚本思路已写在此批次；以后新增 registry 条目可复跑。）

### ② 内建降级成幽灵符号：range / getattr / object
实测（形如 `b = <builtin>(...)`）：

| 写法 | 结果 |
|---|---|
| `b = range(5)` / `range(1, 10, 2)` | ✗ 未定义 `_range`（PY-4 的 range 改写只覆盖 `for … in range()` 头部） |
| `b = getattr(a, "x")` | ✗ 未定义 `_getattr` |
| `b = object()` | ✗ 未定义 `_object` |
| `b = dict()` | ✓ 正常 |

**性质**：未实现的内建**不是编译期报错，而是发一个与内建同名的自由调用** ⇒ 链接期出幽灵符号
（响亮但信息量为零）。修法有两条，都需要编译器侧改动：
1. 给这些内建补真正的降级（`getattr` 走注册表/属性查找；`object()` 走平台对象桩；
   `range` 的**非循环**用法本就没有运行时表示 ⇒ 应当**编译期明确报错**）；
2. 加一张「已知内建 → 是否可降级」表，表外/不可降级的在**编译期**报错。

**已排除的捷径**：把 `range`/`getattr`/`object` 直接做成同名 C 桩。
理由：三个都是**极普通的标识符**，用户代码自己定义同名函数时会**撞符号**
（roadmap 有专门的 DUPLICATE_SYM 记录），而且它们无法通过加前缀绕过（发射的就是裸名）。
⇒ 要走 **编译器侧**的降级/报错，不能靠运行时兜底。


## 批次二十五（2026-09-17，已修）：object() 走平台桩 + 两个内建加编译期诊断

承接批次二十四查出的「内建降级成幽灵符号」：

- **`object()` 修好**：Python 的裸 `object` 正是运行时已有的不透明平台句柄
  （`zeta_platform_obj`），改为发射它。整类问题（链接期裸名）消除。
- **`getattr` / 非循环用法的 `range` 无法降级**（前者要动态属性查找，后者没有运行时表示），
  按项目红线**不改成语义假的桩**；改为在**编译期**打一条指名诊断：
  `builtin \`X\` is not implemented in this form (it would link against an undefined symbol...)`。
  符号照旧发射（行为不变），但用户能立刻看到是谁干的。
- 度量：未定义符号去重 **84 → 83**（`object` 归零）；python_style **158 → 159**（t159）；
  官方 **194/194**；解析 37/38 不回退。
- 复用了批次二十四的结论：**不用同名 C 桩兜底**（会与用户自定义同名函数撞符号）。


## 批次二十六（2026-09-17，分析结论）：`getattr(obj, field)` 能改写，但**现在改会变成静默错值**

想法：`getattr(obj, "field")` 的第二实参是**字符串字面量**时，语义上等价于静态属性访问，
可以改写成 `FieldAccess`，然后走既有的 struct/handle 分派（如 `PyDate.year` → `py_dt_year`）。

**实测结果：改完链接通过，但值是错的** —— `getattr(date(2020,5,6), "year")` 返回 **18388**
（不是 2020）。原因：`d` 是**未标注形参** ⇒ 接收者类型未知 ⇒ `FieldAccess` 不知道它是 PyDate，
退化成结构体字段读/map 取 ⇒ **静默错值**。

⇒ 该改写**只有在接收者类型静态已知时**才安全（换成「先看 type_map 是否 Known handle/struct，
不认识就继续走未实现诊断」的写法可行，但属另一个批次）。**本轮已回滚**（无证据的有效性 + 引入静默错值）。

这条又一次指向同一个洞：**`getattr` 的安全改写、`*_N` 的消除、`xs[0]` 返 0、`in` 早期恒 0 ——
共同的依赖都是「参数/接收者的静态类型」。**

### 本轮顺带确认的边界
- `getattr(a, <计算出的名字>)` 仍走编译期指名诊断（无法静态解析）✓
- `object()` 的修复（批次二十五）不受影响 ✓


## 批次二十七（2026-09-17，分析结论）：给「实参是库句柄」的形参补类型 —— 机制找到了，但一轮没打通

已定位既有机制：**调用点驱动的形参类型推断已经存在**（`resolver.rs:1329` 起，带 `ZETA_PROBE` 探针），
但它只把形参升级成 **Str / F64** 两种（`classify` 返回 1/2，AST 回写也只认这两种）。

本轮尝试扩到「句柄」：在调用点循环里加一段 —— 若实参是 `AstNode::Call{receiver: Some(Var(root)), method}`
且 `find_member(module, method).handle` 有值，就把该形参改成 `Type::Named(tag)`，并在 AST 回写里加
`Type::Named(n,_) => n.clone()`。

**实测未生效**：`def f(d): return d.year` + `f(date(2020,5,6))` 仍输出 **18388**（应为 2020）。
⇒ 说明这条路上还有第二处（形参类型可能不满足 `param_types[i] == Type::I64` 的前置条件，
或 MIR 侧读的不是被回写的那个 AST 字段）。**已回滚**（未验证的改动不留）。

**下一步（已收窄）**：用 `ZETA_PROBE=1` 跑上面这个两行用例，看 `PROBE param infer` 是否打印、
打印的 param_types 是什么；再顺着「MIR 从哪里读形参类型」查第二处。

**批次二十七 · 追加决定性诊断**：`ZETA_PROBE=1` 跑 `f(date(2020,5,6))` 时
**没有任何 `PROBE param infer` 输出** ⇒ 那段形参推断**根本没触发**（不是「升级了但不生效」）。
最可能：`collect_calls` 只扫已注册函数体，而顶层代码被折进合成 main，这条调用没被收集到
（或 callee 查找失败 / pargs 为空）。**下一步就用这个两行用例加探针确认收集范围**，
这比继续猜 MIR 读取点更直接。


## 批次二十八（2026-09-17，定位推进 —— 两段已打通，断点收敛到 MIR）

**先更正批次二十七的一个错误判读**：`ZETA_PROBE` 没打印 `PROBE param infer` **不等于「循环没跑」** ——
该探针只在 `changed` 非空时打印，而 `classify` 不认句柄 ⇒ 自然没有输出。真实原因是**调用形状没覆盖**：
`from datetime import date` 之后 `date(...)` 是 `Call{receiver: None, method: "date"}`，
而上一轮我只匹配了 `receiver: Some(Var(root))` 的 dotted 形状。

**这一轮把两段打通并验证了**（改 `resolver.rs` 的形参推断）：
1. 覆盖 **两种调用形状**：dotted `Root.member(...)` 与 from-import 后的裸 `member(...)`
   （后者查 `py_member_aliases`）；命中 `find_member(module, member).handle` 就升级形参类型
2. AST 回写加 `Type::Named(n,_) => n.clone()`

探针实证（`ZETA_PROBE_PT`）：
```
PROBE param infer: f -> [Named("PyDate", [])]
PROBE rewrite callee=f registered=true changed_types=[(0, Named("PyDate", []))]
```
⇒ **推断触发、AST 形参文本也确实被改写**（两段都通）。

**断点在 MIR**：`def f(d): return d.year` + `f(date(2020,5,6))` 仍输出 **18388**；
`nm -u` 看目标文件只调用 `py_dt_date`，**没有 `py_dt_year`** ⇒ MIR 的 FieldAccess/句柄分派
没有采用形参的声明类型。**已回滚**（端到端症状未变、三套指标未见改善 ⇒ 无证据不留）。

**下一步（很具体）**：给 MIR 的 FieldAccess 路径加探针，打印接收者在 `type_map` / `source_types`
里的条目（`d.year` 走的是 `py_handle_of` → `method_symbol` 那条链），确认形参类型是否根本没进
`type_map`，还是进了但 `py_handle_of` 读的是另一个表。


## 批次二十九（2026-09-17，**关键更正 + 断点定死**）

**决定性对照实验**（3 行，不需要任何推断机制）：

    from datetime import date
    def f(d: PyDate) -> i64:
        return d.year
    print(f(date(2020, 5, 6)))     # 实测 **18388**（应为 2020）

即：**即使形参显式标注成 `PyDate`，`d.year` 依然读垃圾**。

⇒ **真正的断点在 MIR 侧，与「调用点推断」无关**。这更正了我最近几轮的叙事：
- 调用点推断（批次二十八已打通两种形状 + AST 回写，探针实证）**不是**当前的必要条件；
- 更小、更靠前的修法是：**让 MIR 把「形参声明类型」正确落到 `type_map`**，
  使 `py_handle_of(Var)` 的 `name_to_id + type_map` 分支能返回 `PyDate`。
  修好之后，批次二十八那套推断（已证明会触发并把 `PyDate` 写进 AST 与签名表）
  就能把**无标注形参**一并覆盖 —— 两者是叠加关系，不是替代。

**下一步（极小、可复现）**：
1. 用上面这个 3 行用例（显式 `d: PyDate`）作判据；
2. 在 MIR 的形参初始化处（`build_mir` 附近）打印 `name_to_id`/`type_map` 里 `d` 的条目，
   确认是「没落表」还是「落成了别的类型」（如 `Type::from_string("PyDate")` 退化成 I64）；
3. 顺带查 `Type::from_string` 对未知名字的兜底（`src/middle/types/mod.rs:399` 起的 `_ =>` 分支）
   —— 若未知名字不映射成 `Type::Named(name)`，句柄标签就永远进不了 `type_map`。


## 批次三十（2026-09-17，**已修** `d40a19bd`）：句柄类型的形参保住 handle 标签

**病灶（本会话反复撞到的那条主线的真正断点）**：MIR 的形参落表只认 `f64` / `bool` / `str` /
泛型 `T`，**其余一律 `I64`** ⇒ `py_handle_of` 看不到句柄 ⇒ 句柄上的属性/方法全退化：

    def f(d: PyDate) -> i64: return d.year    # 18388（应为 2020）
    d.strftime(...)                            # 发裸名
    d.replace(...)                             # 找不到 PyDate.replace

**两处修法（互相叠加，缺一不可）**：
1. **MIR 形参落表**识别库句柄标签（`pylib::handle_tag`）→ 落 `Type::Named(tag)`（只覆盖 I64 默认）
   ⇒ 覆盖**显式标注**。
2. **resolver 调用点推断**把「实参是库句柄」的形参升级 —— 两种调用形状都认
   （dotted `Root.member(...)` 与 from-import 后的裸 `member(...)`，后者查 `py_member_aliases`），
   AST 回写加 `Type::Named(n,_) => n.clone()` ⇒ 覆盖**无标注** `def f(d)` + `f(date(...))`。

**实测**：`d.year` 18388 → **2020**；`d.month` → **5**（经推断路径）。t160 pre-fix FAIL /
post-fix PASS；python_style **159 → 160**；官方 **194/194**；语料解析 37/38 不回退。

**方法论教训（本会话第三次栽在同一个坑）**：`bash -c` 的**双引号**字符串里写反引号会被
**命令替换**执行掉（commit message 被吃）。写消息/文档一律用 `python3 - <<'PYEOF'` 的
**带引号 heredoc** 或 `git commit -F <file>`，不要用 `python3 -c "…"`。


## 批次三十一（2026-09-17，**已修**）：`getattr(obj, "字段")` 安全版改写

批次二十六试过这个改写，当时**变成静默错值**（18388）被回滚 —— 因为接收者类型未知时
FieldAccess 退化成字段读/map 取。批次三十修好「句柄类型形参落表」后，这个改写可以做得**安全**：

**只在两件事同时成立时才改写**：① 接收者句柄标签静态已知（`py_handle_of`）；
② 该成员在注册表里（`pylib::method_symbol`）。否则仍走既有的编译期指名诊断 ——
**安全条件本身就是这个改动的全部要点**。

实测：`getattr(d, "year")`（标注形参）→ 2020；`getattr(date(2020,5,6), "month")` → 5；
`getattr(a, "nope")`（未知接收者）→ 编译期指名诊断、行为不变。

t161 pre-fix FAIL / post-fix PASS；python_style **160 → 161**；官方 **194/194**。

> 方法论沉淀：**「先修基础类型信息，再开放基于类型的优化」** —— 同一个改写，在类型信息缺位时
> 是静默错值，在位时是正确优化。批次二十六那次回滚不是白费，它把「安全条件」定清楚了。


## 批次三十二（2026-09-17）：用 t162 锁住「句柄面」并留证

批次三十修好句柄形参落表之后，测一遍整个 datetime 句柄面，**全部可用**：

| 表达式 | 结果 |
|---|---|
| `d.strftime("%Y-%m-%d")` | `2020-05-06` |
| `d.day` / `d.year` / `d.month` | 6 / 2020 / 5 |
| `(d + td).day`（结果仍带 PyDate 标签） | 9 |
| `(a - b).days`（得 PyDelta 且可取属性） | 5 |

**修复前 vs 修复后的对照（同一文件）**：修复前**编译通过但一行都不打** —— 句柄调用静默失效，
正是本项目最危险的那一类；修复后四个值全部正确。t162 即以此为准据。

python_style **161 → 162**；官方 **194/194**。

> 验证方法提醒：**用 `git stash` 验 pre-fix 只对「未提交的改动」有效**；本轮两个修复已经提交，
> 所以必须 `git checkout <fix 之前的 commit> -- <涉及文件>` 再重建，否则会得到假 PASS
> （本轮踩过一次，已用该方法重验并记录）。


## 批次三十三（2026-09-17，**已修**）：`json.dumps` 的格式化关键字实参

`_N` 族第二成因（注册表表达不了 Python 签名）里的第一个落地项。该分派的守卫是
`args.len() == 1`，而 `json.dumps(x, ensure_ascii=False, indent=2)` 多出来的 `__kwarg__`
实参让它失效 ⇒ 掉到通用路径 ⇒ 幽灵符号 `py_json_dumps_i64_3`（语料 4 处）。

修法（**形状分派**，本会话已验证三次的路线）：保留第一个**位置**实参（排除 `__kwarg__`
包裹，避免纯 kwargs 调用取错值），忽略格式化 kwargs，并用 `OnceLock` **只打印一次**提示
—— 只影响排版、不影响值，且**不静默**。

实测：`{"a": 1}` / `[1, 2]` / `"x"` 全对；t163 pre-fix FAIL / post-fix PASS；
未定义符号去重 **83 → 82**（`py_json_dumps_i64_3` 归零）；python_style **162 → 163**；官方 **194/194**。

> 同一形状的其它两项仍在队列：`log.info(fmt, *args)`（变参，需要 C 侧 variadic helper）
> 与 `logging.getLogger()`（零参，需允许缺省）。


## 批次三十四（2026-09-17，**已修**）：`logging.getLogger()` 零参

Python 里 `getLogger(name)` 的 name 是可选，注册表却是必填 arity ⇒ 0 参被消歧成幽灵符号
`py_logging_getLogger_0`。**形状分派**修法：0 参补一个空名字 —— 运行期桩里 logger 的身份
就是它的名字，本地所有 logger 方法都是 no-op，安全且不引入假语义。

度量：未定义符号 **82 → 81**（`py_logging_getLogger_0` 归零）；python_style **163 → 164**；官方 **194/194**。

**同族但更大的两个缺口（已定位，未做）**：
| 缺口 | 需要什么 |
|---|---|
| `log.info(fmt, *args)` 变参（`py_logger_info_4/5`） | C 侧 variadic helper：`py_logger_info_n(lg, fmt, n, a1..a4)`（7 固定参数，仿 `zeta_collect_literals` 的声明方式）+ 形状分派。V1 建议**不做 %-替换**但把实参打印出来（可见而非丢弃） |
| `logger.addHandler(...)` | 注册表里没有这个成员 —— 需要新增 `W PyLogger addHandler …` + C 桩 |


## 批次三十五（2026-09-17，**已修**）：变参 `log.info(fmt, *args)`

`_N` 族第二成因里最后一项可修的：Python 的 Logger 方法是**变参**，注册表却是固定 arity
⇒ 多出来的实参被消歧成 `py_logger_info_4/_5`（语料 5 处）。

三处配套（形状分派 + 运行时 helper，与本会话前几次同一套路）：
1. `runtime/tokio_runtime_stub.c` 新增 `py_logger_info_n(lg, fmt, n, a1..a4)`
   （7 个固定参数，仿 `zeta_collect_literals` 的声明方式）。**V1 不做 %-替换，但把实参打印出来**
   —— 可见、绝不丢弃；同步重建 tracked 的 `tokio_runtime.o`。
2. codegen 补 7 参 extern 声明（避免 ABI 猜错）。
3. gen.rs 在句柄方法分支**之前**做形状分派：`method == "info"`、实参 2..=5、接收者是已知
   `PyLogger`、无 `__kwarg__` 包裹 ⇒ route 到 helper。

度量：未定义符号去重 **81 → 80**；python_style **164 → 165**；官方 **194/194**；解析 37/38 不回退。
logger 家族只剩 `py_logging_FileHandler_2`（`addHandler` 面，注册表缺成员，单列待做）。


## 批次三十六（2026-09-17，**已修**）：语料整段 logging 初始化模式

wufu 系策略里这段初始化此前根本编不过，三个缺口各不一样：

    _fh = logging.FileHandler("...", mode="w")   # ① mode 是提示，多余实参 -> 幽灵符号 _2
    _fh.setFormatter(logging.Formatter("..."))   # ② FileHandler 无句柄标签 -> setFormatter 自由调用
    logging.getLogger().addHandler(_fh)          # ③ 注册表没有 addHandler 成员

修法（三处配套，都是本会话反复用的两招：形状分派 + 运行时 no-op 桩）：
1. **形状分派**：`FileHandler` 多余实参丢弃（只留 path），并**只打一次**提示（不静默）。
2. **注册表**：`F logging FileHandler … handle=PyFileHandler` + `W PyFileHandler setFormatter …`。
3. **注册表 + C 桩**：`W PyLogger addHandler …`，并补 `py_logging_addHandler` /
   `py_logging_setFormatter`（no-op 返回接收者，与既有 logging 桩同风格），重建 tracked 的 `.o`。

度量：未定义符号去重 **80 → 77**，**logger 家族符号全部归零**；python_style **165 → 166**；官方 **194/194**。


## 批次三十七（2026-09-17，**已修**）：字面量模板 `.format` 的格式说明符

`format_template_parts` 里有一条 `if inner.contains(':') || inner.contains('!')` → `return None`：
**模板里只要出现格式说明符**（`{:.2f}` / `{:<0}` / `{!r}`），整个重写就 bail out ⇒ 调用退化成
自由调用 `format` ⇒ 幽灵符号、链接失败（语料 **9 处**，集中在策略的日志/报表字符串上）。

修法：**丢掉说明符、保留值**（只损失排版，不丢数据）+ 只打一次提示；仍然无法重写的
（命名域 `"{a}".format(a=…)`、下标越界）改为**编译期诊断**，不再只留裸符号给链接器。

实测：`"{:.2f}|{}".format(3.5, 7)` → `3.5|7`；`"{:<0}\t{}\t{:.2f}…".format(1..5)` → `1 2 3 4 5%`。
python_style **166 → 167**；官方 **194/194**；语料 `format` 幽灵符号 **9 → 7**（余下是命名域，已有诊断）。


## 批次三十八（2026-09-17，**已修**）：命名域 `.format`（`"{a}".format(a=1)`）

`format_template_parts` 只认自动域 `{}` 与数字域 `{0}`，遇名字即 `return None` ⇒ 退化成
自由调用 `format` ⇒ 幽灵符号。修法：调用点把 `__kwarg__` 实参按**名字**建表、位置实参单独成表；
模板里 `{name}` 查名字表（自动域仍只按位置编号，混用也正确）。

实测：`"{a}-{b}".format(a=1, b=2)` → `1-2`；`"{}-{a}".format(9, a=9)` → `9-9`。
python_style **167 → 168**；官方 **194/194**。

### 语料 `format` 幽灵符号的排查结论（7 处，留给下一轮）
分布在 7 个文件（每个 6~10 处 `.format(`）。**已排除**的形状：非字面量接收者（0 处）、
命名域（本批已支持）、格式说明符（批次三十七已支持）、跨行 + 嵌在 `log.info(...)` 里（本地实测通过）。
⇒ 下一步：挑其中一个文件（如 `十年52倍年化59.py`）用 `tools/blocks_loo.py` 做 leave-one-out，
定位那一条具体调用。


## 批次三十九（2026-09-17，**已修**）：内置 `format(value, spec)`

批次三十七/三十八修的是 `.format()` **方法**；语料仍有 7 个 `format` 幽灵符号。按文件定向排查
（抽取每个 `.format(` 调用逐个编译）时发现它们其实来自**内置函数** `format(value, spec)`
（`format(cost,'.2f')` 等）—— 它发的是**自由调用** `format` ⇒ 未定义符号。

修法：降级为 `str(value)`（复用既有的 `str()` 按类型分派），**值保留、只损失排版**
（V1 没有 spec 引擎），并只打一次提示。

度量：未定义符号去重 **77 → 76**，**`format` 归零**；python_style **168 → 169**；官方 **194/194**。

> 排查手法记录：**先按「符号名 → 文件」定位，再抽取每条候选调用逐个编译** —— 这比读文件快，
> 而且直接暴露了「同名但不同来源」（方法 vs 内置函数）这个关键区分。


## 批次四十（2026-09-17，**已修**）：`range(n)` 作为值物化成 Vec

`xs = range(4)` 此前发自由调用 `range`（未定义符号）。形状分派：1 参物化成 `zeta_arange(n)`
（Vec `[0..n)`，类型标 DynamicArray(I64)）；2/3 参需 offset/step，**仍走编译期指名诊断**。

实测 `xs = range(4); len(xs)` → 4。python_style **169 → 170**；官方 **194/194**。

### 符号来源分类（本批的重要副产物）
把「我们这侧」的剩余项查清了，**其中两项其实不是我们的**：

| 符号 | 真实来源 | 归属 |
|---|---|---|
| `date`(5) | `.date()` **方法**调用在**平台对象**上（`context.current_dt.date()`）——接收者类型未知 | **宿主侧**（平台对象） |
| `timedelta`(6) | **裸名** `timedelta(...)`，而所在文件 import 里没有它（`from datetime import time,date`、`#import datetime` 被注释）——语料依赖聚宽的**星号导入** | **宿主侧**（平台命名空间） |
| `range` 2 参(3)、`_Info`(2)、`dict`(2)、`getattr`(2)、`slice`(2)、`sum`(2)、`mean`(2)、`to_dict`(1) | 我们这侧 | 零星 |

⇒ 结论：**我们自己这一侧的符号缺口已接近清完**，剩下的大头（≈70 个符号、≈200 个调用点）
都是宿主/第三方面：平台 API、pandas/numpy、跨文件链接。


## 批次四十一（2026-09-17，**已修**）：内置 `slice(a, b[, step])`

正是 roadmap §三 一直挂着的那一项（「`slice(...)` 解析通过但链接报 `_slice` 未定义」）。
语料形态：`slice(start_idx, last_idx + 1)`、`slice(-count, None)`、`slice(None)`。

修法：形状分派到**逗号下标路径同一个**不透明 slice 句柄 `py_slice_new`
（批次五为 `.iloc[:, 0]` 引入的平台桩）—— 两种写法从此一致，不再各走一条路；
1/2/3 参都覆盖（缺省补 0 / 步长 1）。

度量：未定义符号去重 **76 → 75**，**`slice` 归零**；python_style **170 → 171**；官方 **194/194**。


## 批次四十二（2026-09-17，**已修**）：`range(a, b)` 作为值

上一批只覆盖了 `range(n)`；`range(a, b)`（语料 3 处）仍在发自由调用 `range`。`zeta_arange`
只覆盖 `[0, n)`，故新增 `zeta_arange_from(a, b)`：C 侧 + codegen 两参 extern 声明 + 形状分派；
3 参（带 step）仍走编译期指名诊断。

实测（stash + 回滚 `.o` 重建 vs 当前）：修复前 `_range` / Linking failed → 修复后
`range(2,7)` 长度 5、`range(3,5)` 长度 2。python_style **171 → 172**；官方 **194/194**；
语料 `range` 幽灵符号 **3 → 1**（余下是 3 参形式，按设计保持响亮失败）。

> 顺带记录：`zeta_runtime_c.o` 是 gitignored 的本机构建产物 —— 新增 C 符号后必须按
> validate.md §4 重建，否则 fresh clone/CI 上会变成 undefined symbol。


## 批次四十三（2026-09-17，**已修**）：`range(a, b, step)` 作为值

三种形态至此全部覆盖：`range(n)`（批次四十）、`range(a, b)`（批次四十二）、
`range(a, b, step)`（本批，新增 `zeta_arange_step`）。**只实现正步长**：字面量非正步长
在编译期报指名错误（负步长需要降序 Vec），不静默给升序结果。

实测：`range(1,10,2)` → 长度 5；`range(0,9,3)` → 长度 3。
python_style **172 → 173**；官方 **194/194**；语料未定义符号 **75 → 74**，**`range` 归零**。


## 批次四十四（2026-09-17，**修掉一半**）：6 个「离链接最近」的文件其实卡在 codegen

按「离链接最近的 6 个文件」去查时发现它们**根本没走到链接**：

    Function return type does not match operand type of return inst!  ret i64 0  (double)
    Terminator found in the middle of a basic block!  label %else16

⇒ 函数声明返回 f64，而 `return 0` 直接发 `ret i64 0` ⇒ LLVM 校验失败 ⇒ **整个编译中止**，
所以既没有链接错误也没有未定义符号（看起来像「缺 0 个符号」）。

**本批修掉其中一半**：`MirStmt::Return` 按当前函数的返回类型强转返回值（i64→f64 走 sitofp）。
实测该错误行已消失。

**另一半仍在**：「Terminator found in the middle of a basic block」（`label %else16`）——
疑似 else 分支在已终结的块上继续发射（或发了两次终结指令）。复现：
`zetac <corpus>/l2_data_layer_probe.py -o /tmp/q`。

> 这条比「补平台符号」更值得先做：它挡着 6 个文件（而现在总共只有 1 个文件链接通过）。


## 批次四十五（2026-09-17，**已修**）：try/except 脱糖把 `zeta_try_end()` 追加到终结指令之后

**这是挡住 6 个语料文件的真正原因**，也解释了批次四十四的怪现象（「缺 0 个符号」却不过）。

`try/except` 在**前端**脱糖成 `if zeta_try_setjmp()==0 { body; zeta_try_end() } else { …; zeta_try_end() }`，
而 `zeta_try_end()` 是普通调用**不是终结指令**。分支末条是 `return`/`break`/`continue` 时，
调用被追加到终结指令之后 ⇒ LLVM 校验失败 ⇒ **整个编译中止**：

    Terminator found in the middle of a basic block!   label %else16

**修法**：`branch_falls_through()`（末条 Return/Break/Continue → false；`if/else` 两臂都不落穿 → false），
只在能落穿时补 `zeta_try_end()`。

**双向验证**：pre-fix `Terminator found in the middle…` ✗ / post-fix `Compiled to` + 输出 `1 5` ✓（t174）。

**度量**：Terminator 类错误 **6 文件 → 0**；未定义符号去重 74 → **86**（不是回归：这 6 个文件此前
被编译崩溃挡住，符号从未暴露）；python_style **174/174**；官方 **194/194**；语料解析 37/38。

> 教训：语料文件「0 个未定义符号却链接失败」= 编译在更早阶段崩了，去看**完整 stderr**，
> 不要只看 ld 的 undefined 块。


## 批次四十六（2026-09-17）：**范围决定 —— 平台 API 不做，只做本地/可安装 API**

用户明确指示：**聚宽平台 API（宿主侧）不在范围内**，只处理**本地 API**与**可安装 API**。

因此把本批已经写好并验证过的平台日志桩**回滚**（`set_level`/`info`/`debug`/`warning`/`error`
五个 no-op + 一次「无宿主」警告，实测曾把链接通过从 1/38 提到 3/38）。回滚理由：它们是**平台侧**语义，
按指示不做；保留会让「我们实现了平台语义」这一印象失真。

> 需要时可一个 commit 恢复：`runtime/py_additions.c` 末尾追加那 5 个 `int64_t f(int64_t, ...)`
> no-op（每个符号只喊一次 `has no host linked`），再按 validate.md §4 重建 `zeta_runtime_c.o`。

**范围划分（后续按此执行）**：

| 类别 | 例子 | 做不做 |
|---|---|---|
| 本地 API | stdlib shim、注册表条目、运行时 helper（`py_*`/`zeta_*`） | ✅ 做 |
| 可安装 API | `zorb install` 装进来的纯 Python 包；`-r requirements.txt` 批量安装 | ✅ 做 |
| 平台 API | `set_level`/`history`/`get_security_info`/`get_index_stocks`/`order_*`（聚宽宿主） | ❌ **不做**（宿主职责） |
| 数据面 | pandas/numpy 的 `DataFrame`/`tolist`/`values`/`dropna`/`diff`… | ⚠️ 仅当能**本地实现**时做，不造宿主假数据 |

**仍然有效的结论**（本批测得的证据，供将来接宿主时用）：4 个语料文件只差平台符号
（`大市值价值优化`/`稳健型ETF` 仅差 `set_level`；`趋势筛选ETF轮动`/`首板低开优化版` 差
`set_level`+`history`+`get_security_info`+`DataFrame`+`diff`+`dropna`）。


## 批次四十七（2026-09-17）：**目标重定位 —— 把 REasyQuant 的「本地实现」跑起来**

用户指示：语料里**只针对聚宽平台**写的样本文件不管；**把本地实现运行起来就行**。

### 本地实现在哪

| 文件 | 行数 | 角色 |
|---|---|---|
| `strategies/code/jq_shim.py` | 646 | **本地聚宽 API 模拟**（宿主侧 Python 实现）：`get_price` / `attribute_history` / `get_extras` / `order_target_value` / `get_current_data` / `get_hist_arrays` / `get_all_securities` / `get_trade_days` / `run_daily` / `set_option` / `set_slippage` / `set_order_cost` / `get_security_info` / `set_current_portfolio` / `order` / `query` / `_Bar` / `_LocalContext` / `_HistoryFrame` / `PriceRelatedSlippage` / `OrderCost` |
| `strategies/code/jq_wufu_local.py` | 379 | **本地回测入口**（`REPLAYQUANT_LOCAL=1 python jq_wufu_local.py --start … --end …`） |
| `backend/engines/local_backtest_engine.py` | 264 | VectorBT 本地回测引擎 |
| `backend/engines/jq_shim.py` | 1130 | 引擎侧 shim |

### 实测（编译器现状）

- `jq_shim.py`、`jq_wufu_local.py` **都能解析**（无 W1002）
- **跨文件导入解析已经可用**：编译器把 shim 的模块级 `g` 解析成 `jq_shim___G`（符号已带模块前缀）
- 当前阻塞（都是**本地**问题，不是平台问题）：
  1. `getattr(obj, "字面量名", default)` —— 编译器按「计算名形式」拒绝（jq_shim 4 处 / jq_wufu 10 处 / jq_wufu_daily 11 处 / jq_wufu_local 2 处）。**这些名字全是字面量**，可以静态解析：接收者类型有该字段 → 字段访问；无该字段 → 用 default。类型未知时保持响亮诊断。
  2. 「无默认值参数按 0 读」的警告（`_Bar` 3 个字段、`jq_shim___G` 42 个全局字段）—— 这是**静默错值**风险，需要默认值/字段初始化语义。

> 结论：下一步做 **`getattr(obj, "literal"[, default])` 的本地实现** —— 它属于本地 API（Python 内置），且是本地实现的头号阻塞。


## 批次四十八（2026-09-17，**已修一半**）：`getattr(obj, "字面量"[, default])` 支持结构体接收者

静态改写此前只支持「2 参 + 注册表 handle」；本批补结构体接收者：字段存在→字段访问、
字段不存在但有 default→用 default（**Python 语义**）、无 default→**保持响亮诊断**。
新增 `py_struct_type_of()` / `py_struct_has_field()`（后者兼容 `jq_shim__G`→`G` 前缀剥离）。

**双向验证**：pre-fix `getattr is not implemented in this form` ✗ / post-fix `Compiled to` + `5` `42` ✓（t175）。
python_style **175/175**；官方 **194/194**；语料链接 1/38、未定义符号 86（未变）。

**没解决的部分（下一批的真正入口）**：本地实现的 20 处 `getattr` 接收者**都无静态类型**：

| 接收者形态 | 例子 | 为什么静态解不了 |
|---|---|---|
| import 进来的模块别名 | `getattr(_strategy, "SLIPPAGE_REALISTIC", 0.001)` | 模块不是值，没有 `Type::Named` |
| 未解析形参 | `getattr(obj, "slip", 0.0)` / `getattr(cost, "cost", {})` | W0003 类型检查失败，形参类型未知（鸭子类型） |
| 下标表达式 | `getattr(cd[c], "paused", False)` | 非 Var |
| **平台注入的全局** | `jq_wufu.py` 里的 `g`（本单元无定义） | 需要模块全局**跨文件**类型可见性 |

⇒ 下一步是**模块全局跨文件可见性**（让策略里的 `g` 解析到 shim 的 `_G` 结构体）+ 调用点形参类型推断。


## 批次四十九（2026-09-17，**已修**）：`os.environ.get(...) == 'lit'` 编译期折叠

**这是「本地实现」的总开关**。`jq_wufu.py` 里本来就有本地分支：

    if os.environ.get('REPLAYQUANT_LOCAL') == '1':
        from strategies.code.jq_shim import (OrderCost, PriceRelatedSlippage, g, log, …)
    else:
        from jqdata import *          # 聚宽平台

不折叠 ⇒ **两个分支都下沉** ⇒ 平台符号全被拖进来。折叠后（Python 缺键返回 None，
所以「未设置」是已知答案）：

- `REPLAYQUANT_LOCAL=1 zetac strategies/code/jq_wufu.py`：平台符号
  `set_level`/`history`/`DataFrame`/`diff`/`dropna` **全部消失**，未定义 30+ → **18**
- 剩下 18 个 = numpy/pandas 面（`any`/`arange`/`asarray`/`concat`/`isna`/`linspace`/`sum`/`tolist`/`vstack`/`date`）
  + **未解析的点分本地导入**（`get_extras`/`get_security_info` 其实就在 shim 里）
  + `log.*` 方法（`info`/`error`/`warning`）

双向验证：pre-fix 编译不产出二进制 ✗ / post-fix `Compiled to` + `unset -> else`、`default -> then` ✓（t176）。
python_style **176/176**；官方 **194/194**；语料未定义符号 86 → **85**。

### 下一步（本地实现的真正入口）

**点分本地模块导入解析**：`from strategies.code.jq_shim import (…)` 目前不解析，
所以 shim 里的函数（`get_price`/`get_security_info`/`get_extras`/`attribute_history`…）
仍以未定义符号出现。解析它（相对源文件目录 / 搜索根）就能把整个本地 shim 链进来。


## 批次五十（2026-09-17，**本地实现接上了**）：本地模块导入 → `<module>__<name>` 路由

三处改动（全在本地/可安装 API 范畴）：

| # | 改动 | 为什么必需 |
|---|---|---|
| 1 | 自由调用路由到 `<module>__<member>`（`gen.rs`），结果类型沿用 resolver 推断 | `from <本地模块> import f` 后 `f(...)` 此前留成未解析外部符号 |
| 2 | 点分本地路径的**祖先搜索**（`resolver.rs`，深度上限 6） | `from strategies.code.jq_shim import …` 从 `strategies/code/jq_wufu.py` 编译时须对着仓库根解析 |
| 3 | `walk_py_import` 递归进 `if`/`for`/`while` 体 | 策略顶部 `if os.environ.get('REPLAYQUANT_LOCAL') == '1': from <本地 shim> import …` 里的导入是真导入 |

**实测**：

- 最小验证：`from a import f` → `print(f())` = 42；`import a` + `a.f()` = 42（此前两者都链接失败）
- `REPLAYQUANT_LOCAL=1 zetac strategies/code/jq_wufu.py`：IR 出现 **35 个 `strategies_code_jq_shim__*` 定义**，
  未定义 30+ → **18**，shim 符号只剩 **4 个 arity 后缀不匹配**的
  （`__get_price_3`/`__get_price_7`/`__get_trade_days_2`/`__OrderCost_6` —— 定义端无 `_N` 后缀）
- python_style **176/176**（t53 回归已修：类型推断必须沿用 resolver 的，否则字符串被当 i64 打印指针）
- 官方 **194/194**

**度量口径变化（诚实说明）**：语料未定义符号去重 85 → **151** —— `if` 分支里的导入现在会被收集，
**更多本地模块被真正载入**（本地 shim 的 pandas 面符号随之可见）。链接通过数仍 **1/38**，无回退。

**试过但回退**：让 resolver 只走「被选中的分支」（复用 env 折叠）。理论上更干净，实测更差
（本地模式 18 → 29 未定义）—— 未下沉的分支仍在提供被下沉分支依赖的绑定。已回退并记录。

**下一步**：① 那 4 个 arity 后缀不匹配的符号（定义端要不要带 `_N`）；
② 本地 shim 自身依赖的 pandas 面（`DataFrame`/`Timedelta`/`Timestamp`/`_Info`/`concat`/`tolist`…）。


## 批次五十一（2026-09-17，**已修**）：模块限定名豁免 arity 后缀化 —— 本地 shim 未定义符号归零

`<module>__<name>` 的定义端不带 `_N`，调用端两处会按实参个数后缀化
（`gen.rs` 两个调用点 + `codegen.rs` 的「已存在但形参个数不匹配 → 另声明 extern」分支）⇒ 链接器永远满足不了：

    __get_price_3 / __get_price_7 / __get_trade_days_2 / __OrderCost_6 / ___Bar_1 / ___G_0

默认参数让「调用点实参个数 ≠ 声明形参个数」成为常态，所以必然触发。

**修法**：模块限定名（含 `__`）与 `zeta_*` 一样豁免后缀化，实参适配交给 `coerce_call_args`。

**实测**：`REPLAYQUANT_LOCAL=1 zetac strategies/code/jq_wufu.py`
**shim 前缀未定义符号 6 → 0**（本地 shim 完全接上）；python_style **176/176**；官方 **194/194**。

### ⚠️ 顺带发现的**高优先级缺口**（下一批优先）：默认参数不生效

```python
def add3(a, b, c=10): return a + b + c
print(add3(1, 2))     # 实测 3，应为 13   ← 静默错值
class Pair:
    def __init__(self, x, y=2): ...
Pair(5).total()       # 实测 5，应为 7    ← 静默错值
```

跨模块与同文件都一样。这是**静默错值**（红线问题），不是「缺功能」。
本轮试过在导入路由处补默认值填充，**实测无效** ⇒ `param_defaults` 表里根本没有该条目，
说明收集环节（解析器的 body-prologue 标记 → resolver 收集）就没进表。
按「无证据的改动不留」已回退，测试用例与 fixture 一并移出 —— **不能把错值写成「期望值」**。


## 批次五十二（2026-09-17，**已修一个静默错值**）：跨模块默认参数

**根因**（探针实测，不是猜的）：resolver 把**已载入模块**的默认值按**模块限定名**建表
（`pyfixturearity__add3`），而导入路由查的是裸名（`add3`）⇒ `has=false` ⇒ 默认值丢失。
同文件路径本来就正常（`add3(1,2)` = 13 ✓），所以这个 bug 只在跨模块暴露。

**修法**：先查 `<module>__<member>`，再回退裸名。

**双向验证**：pre-fix `3`/`6` ✗ → post-fix `13`/`6` ✓（t177 + fixture `pyfixturearity.py`）。
python_style **177/177**；官方 **194/194**；语料 1/38、未定义 140（未变）；
`REPLAYQUANT_LOCAL=1` 下 shim 残留未定义 **0**。

### 仍未修（同类静默错值，下一批优先）：构造器 `__init__` 的默认参数

```python
class Pair:
    def __init__(self, x, y=2): ...
Pair(5).total()      # 实测 5，应为 7 —— **同文件也一样**
```

默认值收集只覆盖**顶层 `FuncDef`**，类方法不在内 ⇒ 键根本不存在（探针里表只有 1 个键）。
复现：`/tmp/ctor.py`。


## 批次五十三（2026-09-17，**已修第二个静默错值**）：构造器 `__init__` 默认参数

`Pair(5).total()` 实测 **5**（应为 7）。

**根因**：class 在前端脱糖成 `struct + impl + 构造函数`，构造函数是**新合成**的 `FuncDef`
（body 只有一个 `StructLit` return），**不带** `__init__` 体里的 `zeta_param_default` 标记。
Resolver 按函数名建默认值表，而 `Pair(5)` 解析到构造函数 `Pair`（不是 `__init__`）⇒ 没有该键 ⇒ 缺参读 0。

**修法**：合成构造函数时把 `__init__` 的默认值标记搬进构造函数体前导，下标 **减 1**（`__init__` 形参含 `self`）。

**双向验证**：pre-fix `13`/`6`/**5** ✗ → post-fix `13`/`6`/**7** ✓（t177 现在覆盖三件事：
arity 后缀化、跨模块默认值、构造器默认值）。python_style **177/177**；官方 **194/194**；
语料 1/38、未定义 140；本地模式 shim 残留 **0**。

> 本会话至此修掉的**静默错值**清单（都属于红线问题）：
> `for` 里 `continue` 死循环、数组参数 `in`、列表 `==`、浮点比较类型、句柄类型形参落表、
> `getattr` 无类型校验版、`dict(x)` 别名、跨模块默认参数、构造器默认参数。


## 批次五十四（2026-09-17）：一个静默错值 + 两个接线问题 + 度量口径修正

### 1. 静默错值：`py_map_contains` 用「值 != 0」判断存在性

`return map_get(m, k) != 0;` ⇒ 值为 0/None/空的键被判**不存在**：
`"k" in {"k": 0}` = False，`d.setdefault("k", v)` 会**覆盖**合法的 0。
改为按开放寻址表探测（读 `used` 标志）。改动在 `runtime/tokio_runtime_stub.c`，
**并重建 `tokio_runtime.o`**（该 .o 被 git 跟踪，必须一起提交）。

### 2. `dict.get` / `dict.setdefault` 进注册表

```
W map get map_get_default args=3 ret=i64
W map setdefault py_map_setdefault args=3 ret=i64
```

此前未接线 ⇒ 调用点退化成自由调用（未定义符号）。本地 shim 里 `.get(` 20 次、策略 9 次。

### 3. 运行时对象查找与 CWD 解耦（`src/main.rs`）——**修正了度量口径**

`.o` 此前按裸相对路径查找 ⇒ 只有在仓库根运行才链接得上；别处编译会**静默丢掉整个运行时**，
报出一大堆核心符号未定义，把真实缺口淹没。新增 `find_runtime_obj()`：
CWD → `$ZETA_RUNTIME_DIR` → 可执行文件目录及 4 层祖先。

**口径修正后**（`jq_shim.py`，`REPLAYQUANT_LOCAL=1`）：真实缺口是 **16 个数据面符号** ——
`DataFrame / Timedelta / Timestamp / _Info / concat / datetime64 / dict / execute_trade /
full / get / getattr / searchsorted / setdefault / tolist / unique / where`
（此前报 70 个，大半是运行时缺失的噪音）。

**验证**：t178（`get` 命中/未命中/default、`setdefault` 插入、`{k:0}` 不得被覆盖）。
python_style **178/178**；官方 **194/194**；语料 1/38、未定义 140。


## 批次五十五（2026-09-17）：pandas/numpy **日期面**别名到已有的 PyDate/PyDelta

本地 shim 的日期调用（`np.datetime64(pd.Timestamp(end_date))` 4 处、`pd.Timedelta(days=1)`、
`np.searchsorted(dates, v, side=…)` 2 处）此前都是未定义符号。它们**不需要新的数据表示**：
`datetime` 模块早就产出 PyDate/PyDelta，运行时日期运算/比较全按句柄标签分派 ⇒ 只做别名：

```
M pandas / M numpy
F pandas Timestamp   py_dt_from_str       args=i64 ret=i64 handle=PyDate
F pandas Timedelta   py_dt_timedelta      args=i64 ret=i64 handle=PyDelta
F numpy  datetime64  py_dt_identity       args=i64 ret=i64 handle=PyDate
F numpy  searchsorted py_dt_searchsorted  args=i64,i64,i64 ret=i64
```

新增 C：`py_dt_from_str`（`%Y-%m-%d`）、`py_dt_searchsorted`（PyDate Vec 二分；`side=` 以字符串句柄到达，
按内容判定，默认 left = numpy 语义）。

**踩坑（已修）**：把 `pd.Timedelta(days=n)` 写成 `py_dt_timedelta(n*86400)`，但该函数单位是**天**不是秒
⇒「2024-01-31 + 2 天」跑到 **2497 年**（静默错值）。直接别名后 `2024-01-31 + 2d` = `2024-02-02` ✓。

**度量**：`jq_shim.py` 未定义 **16 → 12**；语料未定义 **140 → 135**；python_style **179/179**；
官方 **194/194**；语料链接 1/38。

### 本地 shim 剩余未定义（12）

`DataFrame / _Info / concat / dict / execute_trade / full / get / getattr / setdefault / tolist / unique / where`

其中 `get`/`setdefault` 仍出现 ⇒ 说明这些调用点的接收者**不是 map 句柄**（可能是类实例/未知类型），
需要单独定位；`execute_trade` 同理（看起来是**方法解析**问题，不是库问题）。


## 批次五十六（2026-09-17，**回滚收场，附精确证据**）：`.get()` 的两条失败路径

目标：解决 `jq_shim.py` 里仍报未定义的 `get`/`setdefault`（14 个调用点）。**没修成，全部回滚**，
但把两条**互不相同**的失败路径钉清楚了 —— 这比一个半吊子补丁值钱。

### 路径 A：接收者类型不是 `map` ⇒ 退化成自由调用 `get`（未定义符号）

```python
CACHE = {}                    # 模块级全局 dict
def f():
    CACHE["x"] = 1
    return CACHE.get("x", 99) # ← 未定义符号 _get
def g(d: dict):
    return d.get("k", 7)      # ← 未定义符号 _get（**带注解也一样**）
```

`map` 方法分派在 `gen.rs` 里有一条**专用分支**（`d.keys()/values()/items()/most_common()`，
注释写明「`map` 不是 Py* 标签，所以走这条而不是句柄分派」），**其中没有 `get`/`setdefault`**；
注册表那条 `W map get …` 只在句柄路径上生效。`receiver_ty` 取自 `type_map`，而
模块全局的读取没有本地槽位（类型在 `module_global_types` 里）⇒ 拿到 I64 ⇒ 分支不进。

### 路径 B：字典**字面量**的 `d.get(k, default)` 走注册表路径，是**正常**的

`t178`（`d = {"a": 1}` + `d.get("a", 99)`）在本批之前是**通过**的 —— 说明 `map_get_default`
本身没问题，字面量路径能正确取到值。

### 我试了什么（以及为什么回滚）

给那条专用分支补 `get`/`setdefault` 两个 case + 缺省值补 0 + 把 `dict` 当作 `map` 的别名 +
模块全局类型回退。结果：**4 个既有用例挂掉**（t103/t178/t25/t27），t178 从
`1 99 5 5 0 0` 变成 `99 99 5 99 7 7` —— `d.get("a", 99)` **返回了默认值 99**，
即 map 参数变成了 0/错位（`map_get_default(0, k, def)` 正好返回 `def`）。

**说明该分支的 `arg_ids` 布局与我的假设不符**（`keys`/`values` 是 0 参方法，看不出来）。
⇒ 回滚全部改动，`git checkout -- src/middle/mir/gen.rs`，复核 **python_style 179/179**、
t178 输出恢复 `1 99 5 5 0 0` ✓。**不留回归、不留未验证的改动。**

### 下一步（已备好切入点）

在 `d.get(k, default)`（字典字面量）上埋一个 env 门控探针，打印该分支的
`receiver_ty`、`arg_ids` 与最终 `MirStmt::Call` 的 `func`/`args`，与**注册表路径**（现在能正确取值的那条）
逐字段对比，找出布局差异后再动代码 —— 不再靠猜。

**剩余**：`jq_shim.py`（LOCAL=1）未定义 12 个：
`DataFrame / _Info / concat / dict / execute_trade / full / get / getattr / setdefault / tolist / unique / where`


## 批次五十七（2026-09-17，**探针定位完成，改动仍回滚**）：`receiver_ty` 到不了 map

接批次五十六，用 env 门控探针把 `get`/`setdefault` 的分派真相查清了。

### 真相 1：正确的分派点在哪

`d.get(k, default)` / `d.setdefault(k, v)` 的**正确实现早就在**（`gen.rs` ~6518-6600 那一组
`map` 块），而且做对了关键一步：**键要经过 `lower_map_key()` 做内容哈希** ✓
（`map_get_default(recv, lower_map_key(key), default)`）。IR 实测字面量情形就是：
`call i64 @map_get_default(i64 %58, i64 %59, i64 99)` ✓。

⇒ 批次五十六我把 `get`/`setdefault` 加到了**另一个**分支（~6104 的 `keys/values/items/most_common`
分支）—— 那条分支**不做 `lower_map_key`**，所以键按指针哈希 ⇒ 查不到 ⇒ 返回默认值。
**这就是当时 t178 从 `1 99 5 5 0 0` 变成 `99 99 5 99 7 7` 的原因**（t178 值 `99` 正是默认值）。

### 真相 2：为什么注解/全局 dict 还是走不到那条块

探针（`ZETA_PROBE_MAP`）实测：

```
PROBE_MAP method=get arg_ids=[1, 4, 5] receiver_ty=Some(I64) globals=[]
```

- `def g(d: dict)` 的形参：`receiver_ty = I64` ✗ —— `dict` 注解**没有**变成 `Named("map")`/`Named("dict")`
- 模块级 `CACHE = {}`：`module_global_types` 是**空的**（`globals=[]`）✗

所以「放宽 map 判定 + 模块全局类型回退」两个改动**实测无效**（探针为准），按纪律全部回滚 ✓。
IR 里该调用仍是 `call i64 @get(...)`（自由调用 → 链接失败，**响亮** ✓ 不是静默错值）。

### 下一步（两个具体子项，都有探针可复核）

1. **让 `dict` 注解落表**：`def f(d: dict)` 的形参类型要变成 `Named("map")`（或让 map 判定接受 `dict`），
   在**参数落表**处修（`gen.rs` 形参类型表 / resolver 的注解解析）。
2. **让模块级 dict 全局落表**：`CACHE = {}` 要进 `module_global_types`（现在整张表是空的 ——
   说明该表只在某些编译形态下被填充，需查 resolver 的填充条件）。

复核方式：`ZETA_PROBE_MAP=1 zetac <file>` 看 `receiver_ty` 是否为 `Named("map")`，
以及 `/tmp/mget3.py` 是否输出 `1` 与 `3`。

**当前状态**：python_style **179/179**、t178 输出 `1 99 5 5 0 0` ✓、工作树干净（仅本 roadmap 提交）。


## 批次五十八（2026-09-17，**探针定位：嵌套 class 被静默丢弃**）

顺着 `_Info`（shim 里 `get_security_info` **函数体内**定义的 class）查下去，发现一个**静默错值**级别的缺口：

### 症状（最小复现）

```python
def make(n):
    class P:
        def __init__(self, x):
            self.x = x
    p = P(n)
    return p.x

print(make(5))     # 实测 4372832720（指针！）应为 5
```

顶层类完全正常（`class P: …` + `P(5).x` → 5 ✓，`P(7).x` → 7 ✓），**只有嵌套类**出问题。

### 证据（IR 对比）

| | 顶层类 | 嵌套类 |
|---|---|---|
| `define i64 @P(` | ✓ 有（含 `runtime_malloc` + `getelementptr` 字段写入） | **✗ 完全没有** |

⇒ 嵌套 class 的**构造函数根本没被创建**：`P(n)` 的调用点落到别处（未定义符号或垃圾指针），
字段读写自然也是错的。这不是「不支持」而是「**静默产生错值**」，属红线问题。

### 根因（已定位到具体位置）

`parse_class` **只挂在顶层 `definitions` 备选表里**（`top_level.rs:1228` 的 `alt(...)`），
语句解析器里**没有 class**。函数体内的 `class P:` 于是被语句解析路径吞掉 ——
既没有解析错误（无 W1002），也没有生成任何节点 ⇒ 构造函数不存在。

### 下一步（切入点明确）

把 `parse_class` 加进**语句**解析的备选（让嵌套 class 也脱糖成 `Block{StructDef, ImplBlock, FuncDef}`，
进而走到已有的 item 处理与 `type_decls` 注册）。注意两点：
① 构造函数名是**函数内局部名**，会走 `lower_closure` 的 hoist 路径（nested def 已有先例 ✓）；
② 同名类在多个函数里定义会撞名，需要按**所属函数**加前缀。

复核：`zetac /tmp/nested3.py` 应输出 `5`，且 IR 里应出现 `define i64 @…P…(`。
（本轮未改代码 —— 只做定位，树保持干净。）


## 批次五十九（2026-09-17，**三步定位完成，代码仍回滚**）：嵌套 class 的解析已通、构造器体为空

接批次五十八，按计划把 `parse_class` 加进语句解析。**推进了三步，但最终仍回滚** —— 过程有证据。

### 步骤 1：语句解析器接纳 `parse_class` ✓（但撞上 nom 的 21 元组上限）

`parse_class` 只挂在顶层 `definitions` 的 `alt(...)` 里。加进 `parse_stmt` 的 `alt` 后编译失败：

    the method `parse` exists for struct `Choice<...>`, but its trait bounds were not satisfied

因为 **nom 的 `alt` 单个元组最多 21 个备选**，而语句列表正好已有 21 个 ✗。改用**嵌套 alt**
（`alt((parse_func, parse_class))` 作为一个元素）✓ 编译通过 ✓ ⇒ **嵌套 class 从此能被解析** ✓
（IR 里出现了被 hoist 的构造函数 `define i64 @__closure_0(i64 %0)` ✓）。

### 步骤 2：`P(n)` 被送到平台对象运行时 ✗（大写名回退）

`gen.rs:5944` 的大写名回退只检查 `func_ret_types`：

    let user_fn_defined = self.func_ret_types.contains_key(&method.clone());

嵌套 class 的构造函数是 **hoist 成闭包** 的（在 `closure_vars` 里 ✗ 不在 `func_ret_types` 里），
于是 `P(n)` 落进 `zeta_platform_obj` ✗。把 `closure_vars`/`hoisted_names` 一并纳入判断后，
IR 变成正确的 `call i64 @__closure_0(i64 %5)` ✓。

### 步骤 3：构造函数的**函数体是空的** ✗（真正的缺口）

```
define i64 @__closure_0(i64 %0) {
entry:
  %1 = alloca i64, align 8
  store i64 %0, ptr %2, align 4
  store i64 0, ptr %1, align 4
  %4 = load i64, ptr %1, align 4
  ret i64 %4                      ; ← 没有 runtime_malloc、没有字段写入，直接返回 0
}
```

即：class 脱糖出的 `Block{StructDef, ImplBlock, FuncDef(ctor)}` 落在**函数体内**时，
这些 **item 节点没有被真正 lower** ⇒ 结构体没注册、构造函数体是空的。
于是 `P(n)` 返回 0/垃圾 ⇒ 字段读仍是错值；且因为调用点已改为调用这个空函数，
行为从「垃圾值」变成「崩溃（SIGTRAP）」✗ —— **比之前更差**，所以回滚 ✓。

### 结论 / 下一步

**回滚**（`stmt.rs` / `top_level.rs` / `gen.rs`），复核 python_style **179/179** ✓。
要修的不是「解析」也不是「调用点」，而是 **函数体内的 item 节点（StructDef / ImplBlock / ctor）
没有走到 item 的 lowering 路径** —— 这是嵌套 class 的最后一环，也是唯一还没做的一环。
（步骤 1 的两处改动本身是对的、且有 IR 证据，可与这一环一起提交。）

复核脚本：`zetac /tmp/nested3.py` 应输出 `5`，IR 里 `__closure_0` 应含 `runtime_malloc` + 字段写入。


## 批次六十（2026-09-17，**已修**）：函数体内定义的 class（嵌套 class）真正可用

`_Info`（shim 里函数体内定义的 class）此前是未定义符号；最小复现更是**静默错值**（打印指针）。
三处缺一不可：

| # | 位置 | 问题 |
|---|---|---|
| 1 | `stmt.rs` | `parse_class` 只挂在顶层 `definitions` 的 `alt` ⇒ 嵌套 class 被静默吞掉（无节点、无解析错误）。**nom 的 `alt` 单元组上限 21**，语句列表正好已满 ⇒ 必须写嵌套 alt |
| 2 | `gen.rs` | 大写名回退只查 `func_ret_types`，而函数内 class 的构造函数 hoist 成闭包（在 `closure_vars`）⇒ `P(n)` 被送到 `zeta_platform_obj` |
| 3 | `gen.rs` | `lower_closure` 一律 `lower_expr(body)`，而构造函数体是 `Block` ⇒ **语句全丢** ⇒ 构造函数空、`ret 0`。改为 `Block` 逐条 `lower_ast`；并让子 MirGen 继承 `type_decls`/`func_ret_types` |

**双向验证**：pre-fix `4364608240`/`8729216492` ✗ → post-fix `5`/`7` ✓（t180）。
**实测**：`jq_shim.py` 未定义 **12 → 11**（`_Info` 消失）；python_style **180/180**；官方 **194/194**。

> 语料未定义符号 135 → 136（+1）：嵌套 class 现在会被解析 ⇒ 其方法体里引用的符号也随之可见。
> 链接通过数不变（1/38），属「看得更多」而非回退。

### 顺带发现（下一批候选）：字符串**字段**的类型传播

```python
class A:
    def __init__(self, s):
        self.name = s
print(A("hi").name)          # ✗ 打印指针
print(A("hi").name == "hi")  # ✓ 1（值是对的！）
```

⇒ 字段的**值**正确（字符串句柄），只是**字段读表达式的类型**是 I64 ⇒ print 走整型路径。
顶层类同样如此，与嵌套无关。t180 只断言已修好的部分，**没有把错值写成期望值**。


## 批次六十一（2026-09-17，**探针定位：字符串字段错在「调用结果类型」而非字段读**，改动回滚）

批次六十顺带发现的「字符串字段打印成指针」，本轮查到了**完整的因果链**，但两个修法都无效，故回滚。

### 因果链（探针实测，`ZETA_PROBE_FLD`）

```
PROBE_FLD field=name base_ty=Some(Str) decls=["A"] retA=Some(Variable(TypeVar(0)))
```

1. 结构体构造函数的**声明返回类型是结构体名**（`ret: "A"`），而裸的大写名被解析成**泛型类型变量**
   ⇒ `func_ret_types["A"] = Variable(TypeVar(0))` ✗
2. 调用点随后做泛型替换 ⇒ `A("hi")` 的结果类型被**统一成第一个实参的类型**（**Str** ✗）
3. 于是 `A("hi").name` 是在一个「类型为 Str 的值」上读字段 ✗ ⇒ 字段类型丢失 ⇒ 读出来按 i64 用 ⇒
   `print` 打印指针 ✗（而 `== "hi"` 仍然为真 ✓ —— 因为**值**是对的，丢的只是**类型**）

### 试过的两个修法（都无效，已回滚）

| 修法 | 位置 | 结果 |
|---|---|---|
| 字段读按 `type_decls` 的声明类型落表 | `FieldAccess` lowering（原来无条件 `Type::I64` ✗） | 打印仍是指针 ✗ |
| 调用结果在「被调用者是已注册 struct」时强制为 `Named(struct)` | 自由调用结果类型处（覆盖泛型替换） | 打印仍是指针 ✗ |

两次改动都**没有回归**（python_style 180/180 ✓）但也**没有改变输出** ✗ ⇒ 说明类型在**更晚的阶段**
（`print` 的类型判定 / 另一处覆写）又被改回去了 —— 这正是下一步要用探针查的点（在 `print` lowering
处打印实参 id 的 `type_map` 条目，看它此刻是什么）。

**回滚**，复核 python_style **180/180** ✓、`/tmp/strfield.py` 行为不变 ✓。

### 结论

`A("hi").name` 的**值是对的**（`== "hi"` 为真 ✓），错的是**类型信息在调用边界被泛型统一冲掉** ✗。
要修就修在**调用结果类型**上，但必须找到**最后一个覆写点** —— 否则改了也不生效（本轮已证实）。


## 批次六十二（2026-09-17，**根因完全查明：字段类型声明 + 子 MirGen 克隆**，改动回滚）

接批次六十一，把「字符串字段打印成指针」查到底了 —— 现在**根因与「为什么前两次修法都不生效」都清楚了**。

### 探针实证（`declared=[("name","i64")]`）

```
PROBE_FLD2 field=name base_ty=Some(Str) declared=Some([("name", "i64")])
```

⇒ 结构体字段的**声明类型是 i64** ✗。来源：`def __init__(self, s)` 的参数**没有注解**，
class 脱糖时把字段类型记成默认的 `i64`。而运行时**值**是正确的字符串句柄 ✓
（所以 `A("hi").name == "hi"` 为真 ✓）。字段读按 `i64` 用 ⇒ `print` 打印指针 ✗。

### 为什么「在字段读处查 `type_decls`」和「在调用结果处强制 `Named(struct)`」都不生效

第三次尝试：在 **`StructLit` lowering** 处按实际存入的值**细化声明类型**（i64 → str）✓
—— 这是对的修法方向，但**仍然不生效** ✗。原因：

**构造函数是被 hoist 到子 MirGen 里 lower 的**，而子 MirGen 拿到的是
`type_decls.clone()`（批次六十为了让嵌套 class 的 `StructLit` 能解析而加的 ✗）⇒
**在子上下文里做的细化，回不到父上下文** ✗。父上下文给 `A("hi").name` 定型时，
看到的仍是旧的 `i64` ✗。

⇒ 结论：这个修法要生效，必须让 `type_decls` **共享**（`Rc<RefCell<…>>` 之类的重构 ✗），
或者把细化搬到**父上下文**里做（在 class item 的 lowering 处，而不是在构造函数体里 ✗）。

### 回滚与复核

`git checkout -- src/middle/mir/gen.rs`，python_style **180/180** ✓，`/tmp/strfield.py` 行为不变 ✓。

### 下一步（两条路，都已想清楚）

1. **共享 `type_decls`**（把 `HashMap` 换成共享句柄）—— 一次重构解决「子上下文细化回不去」这一类问题；
2. 或**在父上下文细化**：class item lowering 时，用 `__init__` 的 `self.x = <expr>` 右值类型
   直接写父上下文的 `type_decls`（不经过构造函数体）。


## 批次六十三（2026-09-17，**已修**）：结构体字段类型跟着实参走

**两处必须同时**（缺一无效 —— 这正是批次六十一/六十二三次尝试失败的原因）：

| # | 位置 | 做法 |
|---|---|---|
| 1 | **调用点（父上下文）** | 用实参类型细化 `type_decls` 里的字段类型。**不能在构造函数体里细化**：构造函数在**子 MirGen** 里 lower，拿到的是 `type_decls.clone()`（批次六十为修嵌套 class 而加），子上下文改动**回不来** |
| 2 | **字段读** | 按 `type_decls` 的声明类型落表（原来无条件 `Type::I64`）；接收者类型丢失时从 `Call{method}` 反查结构体 |

另给**闭包调用路径**加了同样处理（函数内 class 的构造函数是 hoist 成闭包的）。

**双向验证**：pre-fix `4378468946`/`1`/`42` ✗ → post-fix `hi`/`1`/`42` ✓（t181）。
python_style **181/181**；官方 **194/194**；语料 1/38、未定义 136（未变）。

### 仍有一个变体未修：**函数内 class 的字符串字段**

```
nested2:  def security(code): class _Info: def __init__(self, c): self.display_name = f"ETF-{c.split('.')[0]}"
          print(security("518880.XSHG").display_name)   # 仍打印指针 ✗
```

顶层 class 的字符串字段 ✓、函数内 class 的**整型**字段 ✓ 都已正确 ⇒ 剩下的是
「闭包 ctor + 字符串字段」这个组合，切入点：在闭包路径确认 `type_decls` 的键与
`base_func`（hoist 名/类名）是否一致，以及 f-string 赋值的字段是否被识别为 str。


## 批次六十四（2026-09-17，**未修成，改动回滚**）：`-> Any` 返回类型是剩下的那层

继续追「函数内 class 的字符串字段」。探针把最后一层挖出来了：

```
PROBE_CLS read field=display_name base_ty=Some(Named("Any", [])) resolved=None
PROBE_CLS base_func=_Info in_decls=true arg_tys=[Some(Str)] decls=[("_Info", [("display_name","i64"),("start_date","str")])]
```

### 真正的原因不是「闭包 ctor + 字符串字段」，而是**读的位置跨了 `Any`**

`nested2` 的实际读点是 `get_security_info("...").display_name` —— 基是**调用
`get_security_info`**，而它的注解是 **`-> Any`** ✗ ⇒ 读点拿到的基类型是 `Named("Any")` ✗，
既不是结构体、也无法反查到 `_Info` ✗ ⇒ 字段类型退化成 i64 ✗ ⇒ 打印指针。

（`decls` 里 `start_date` 是 `str` ✓ 而 `display_name` 是 `i64` ✗ —— 因为前者右值是**字面量**
（解析期可知 ✓），后者是 **f-string** ✗（解析期不可知 ✓），正好印证「字段类型靠实参/字面量细化」这条链 ✓。）

### 试过什么

给字段读加「`Named` 查不到就再按 `Call{method}` 反查」的链式回退 ✗，
并修好被正则误删的闭包路径细化块 ✗。结果：**两处都没有改变任何输出** ✗
（`nested2` 仍打印指针 ✓、`make("hi")` 这类函数内 class 字符串字段同样如此 ✓），
python_style 181/181 ✓、官方 194/194 ✓ —— 属「无证据的改动」，**已回滚** ✓。

### 结论 / 下一步

要修必须**细化函数的返回类型**：当函数声明为 `-> Any`（或未知）而函数体**返回一个已知结构体实例**时，
把 `func_ret_types[fn]` 细化成该结构体 —— 这与批次六十三对字段做的细化是同一类手法
（在**父上下文**做，别丢进子 MirGen ✗）。

> 注：`nested2` 是我为复现 shim 的 `get_security_info` 写的合成用例；
> 真实 shim 里该函数的用法可能不同，所以这条按「合成复现」记录。


## 批次六十五（2026-09-17，**未修成，改动回滚**）：`infer_untyped_returns` 是正确入口，但结构体名不在手边

按上一轮的结论去**细化函数返回类型**，找到了既有机制 `infer_untyped_returns`
（注释写着「conservative evidence-only inference」，已有 str/f64 推断 + 6 轮迭代传播 ✓），
在它的循环里加了「返回 `<已知结构体>(...)` ⇒ 该函数返回该结构体」的细化（`-> Any` / `-> object` 也视为可推断 ✓）。

探针证据：

```
PROBE_RET fn=get_security_info ret="Any" rets=1 has_Info=false decls=[]
```

- 函数选对了 ✓（`ret="Any"` ✓、`rets=1` ✓ —— 返回语句被正确收集 ✓）
- **但 resolver 的 `type_decls` 是空的** ✗（`decls=[]` ✗）—— 这个字段在这条路径上**根本没被填充** ✗；
  结构体注册实际发生在 **MirGen** 的 `type_decls` 里（批次六十~六十三用的就是那份 ✓）

改用「扫描已注册 AST 收集 struct 名」代替空的 `type_decls` ✗ —— **仍未生效** ✗
（顶层 class 的 `def f() -> Any: return A("hi")` 也还是打印指针 ✗）。

按纪律回滚 ✓：python_style **181/181** ✓、官方 194/194 ✓。

### 本轮净收益：两个「下一步该往哪走」的硬事实

1. `infer_untyped_returns` 是**正确的入口**（能看到函数、返回语句、`-> Any`）✓；
2. 但**结构体名在 resolver 侧不可得** ✗（`type_decls` 空 ✗，扫描 AST 的替代方案也没生效 ✗）
   ⇒ 要么把结构体注册**提前到 resolver**（让那份 `type_decls` 真正被填充 ✓），
   要么把返回类型细化**挪到 MirGen 侧**（那里有 `type_decls` ✓，但只有当前 item 的 AST ✗
   —— 需要一次跨函数的预扫描 ✓）。


## 批次六十六（2026-09-17，**已修**）：模块级容器的类型落表

**根因**（探针 `globals=[]`）：`module_global_types()` 只在 RHS 类型可推导时插入，
而 `CACHE = {}` / `POOL = []` 这类**容器字面量没有分支** ⇒ **整张表为空** ⇒
模块全局上的 `.get(k, d)` 退化为自由调用 `get`（未定义符号）。
这正是本地 shim 里 `_local_cache.get(...)`（9 个调用点）的形态 ✓。

**修法**：RHS 匹配补 `DictLit` → `Named("map")`、`ArrayLit` → `DynamicArray(i64)`。

**实测**：`/tmp/mget3.py` 的 `f()` 从「未定义符号」变成 **1** ✓；
`jq_shim.py` 未定义 **11 → 10**（`setdefault` 消失）；`jq_wufu.py` 20；
python_style **182/182**（t182）；官方 **194/194**。

### 顺带发现（未修）：模块级**列表**的 `.append()` 之后读元素仍是 0

```python
POOL = []
def add(x):
    POOL.append(x)
    return POOL[0]
print(add(5))     # 实测 0，应为 5
```

疑似 `vec_push` 返回**新句柄**后，**全局变量的回写**没生效（局部变量有专门的重绑定逻辑 ✓）。
t182 只断言已修好的 dict 部分 —— **没有把错值写成期望值** ✓。


## 批次六十七（2026-09-17，**未修成，改动回滚**）：模块级列表 `.append()` 的写回

接批次六十六的发现（`POOL = []` + `POOL.append(x)` + `POOL[0]` → **0** ✗）。查清了整条链：

### 链路（探针 + 代码定位）

1. 已存在的写回逻辑在**带类型的 `push` 分支**里（`vec_push` 会返回新句柄，必须回写 ✓），
   而且**只处理局部变量**（`name_to_id.get(name)` ✗）—— 模块全局没有本地槽位 ✗。
2. 但 `POOL.append(x)` **根本没走那条分支** ✗（探针无输出 ✗）：它落进了
   `opaque_fallback`（未定型接收者的兜底表）✓ → `("push", 2) | ("append", 2) => vec_push` ✗
   —— **这里没有任何写回** ✗ ⇒ 扩容后的新句柄被丢掉 ⇒ 列表看着还是空的 ✗（静默错值 ✓）。
3. 想让第 1 条生效，需要接收者被定型为 `DynamicArray` ✗ —— 而模块全局读出来的类型是裸 `I64` ✗。

### 试过什么（都无效，已回滚）

- 在 `push` 分支给**模块全局**加 `zeta_env_set` 写回 ✓（逻辑正确 ✓，但那条分支不触发 ✗）
- 再把「模块全局读的类型回退到 `module_global_types`」补上 ✗ —— 仍未改变输出 ✗
  （`POOL` 的类型仍不是 `DynamicArray` ✗，说明这张表在这条路径上还是拿不到 ✓）

python_style **182/182** ✓、官方 194/194 ✓（回滚后复核 ✓）。

### 下一步（两条路，取舍清楚）

1. **继续查 `module_global_types` 为什么没生效**（批次六十六刚给它补了 `ArrayLit` ✓，
   但这里读出来仍不是 `DynamicArray` ✗）—— 需要在**接收者定型处**打印这张表与查到的键 ✓；
2. 或**在 `opaque_fallback` 的 `append`/`push` 处加写回**（接收者是 `Var` 时，
   局部走 `Assign`、全局走 `zeta_env_set` ✓）—— 但要先确认 `vec_push` 对**非 vec** 接收者
   是安全的 ✗（该兜底表也服务未定型接收者 ✗，写回错对象会破坏数据 ✗）。

> 注意：`vec_push` 对**非向量**接收者会怎样，是选路 2 前必须先验证的前提 ✓。


## 批次六十八（2026-09-17，**两个前提查清，改动仍回滚**）：`vec_push` 的边界 + 全局类型其实拿得到

### 前提一（批次六十七列的选路前提）：`vec_push` 对**非向量接收者不安全** ✗

```c
int64_t vec_push(int64_t data_ptr, int64_t val) {
    int64_t* base = (int64_t*)(data_ptr - 16);   // ← 直接把入参当数据指针往前读 16 字节
    int64_t cap = base[0];
```

⇒ 传入 map/struct 句柄会读到垃圾 ✗ ⇒ **「在 `opaque_fallback` 的 `append`/`push` 处加写回」这条路必须排除** ✗
（该兜底表也服务未定型接收者 ✗，写回错对象会破坏数据 ✗）。这条前提现在有代码证据 ✓。

### 前提二：模块全局的**类型表其实拿得到** ✓

```
PROBE_GT var=POOL globals_set=true gtypes=Some(DynamicArray(I64))
```

⇒ `module_global_types["POOL"] = DynamicArray(i64)` ✓、`module_globals` 里也有 ✓ ——
问题**不在表**，而在**读取处**：`receiver_ty` 取自 `type_map[读出的 id]` ✗（裸 I64 ✗），
所以 `push` 分支不触发 ✗。

### 试过什么（仍无效，已回滚）

把两处一起补上：① 读处对模块全局回退到 `module_global_types` ✓；② `push` 分支给模块全局
加 `zeta_env_set` 写回 ✓。编译通过 ✓、但 `POOL.append(x)` 之后 `POOL[0]` **仍是 0** ✗
（要么 `push` 分支仍未触发 ✗，要么写回与后续读取用的键/槽位不一致 ✗）。

回滚后复核 python_style **182/182** ✓、官方 194/194 ✓。

### 下一步（把范围缩到最小）

在 `push` 分支入口打印「是否触发 + 接收者 AST 形态 + 写回分支走了哪条」，
以及 `POOL[0]` 的读取走的是 env 还是本地槽 —— 两处对齐之后再改，**不再一次补两处** ✗。


## 批次六十九（2026-09-17，**已修**）：`dict(m)` 浅拷贝

`dict(_cost_config)`（本地 shim）此前落到自由调用 `dict` ✗。Python 的 `dict(m)` 是**浅拷贝**不是别名 ✓，
实现为 `map_new()` + `py_map_update(new, src)` ✓（后者按**原始键**拷贝 ✓，字符串键的内容哈希保持 ✓）。

**踩到的坑（当场修）**：`py_map_update` **返回 0** ✗ —— 一开始把它的返回值当表达式值 ⇒
`dict(d)` 得到**空 map** ✗（实测 `c.get("a", 99)` = 99 ✗）。改为表达式值 = 新 map、
`py_map_update` 的返回值丢进 sink ✓。

**边界**：只在实参**静态是 map** 时做 ✓；类型未知保持**响亮诊断** ✗（拷错句柄会破坏数据 ✓）。

**双向验证**：pre-fix `Undefined symbols` ✗ → post-fix `1`/`2`/`99` ✓（t183；第三行证明是拷贝 ✓）。
python_style **183/183**；官方 **194/194**；`jq_shim.py` 未定义 **10 → 9**。

### 本地 shim 剩余未定义（9）

`DataFrame / concat / execute_trade / full / get / getattr / tolist / unique / where`

分类：
- **数据面**（6）：`DataFrame` `concat` `tolist` `unique` `where` `full` —— numpy/pandas 用面，L2 mini-DataFrame
- **接收者类型未知**（3）：`get`（annotated param / `Any` 返回）`getattr`（同）`execute_trade`（模块全局对象的方法）
- 已确认**不是库问题**：`execute_trade` 是类方法 ✗、`get`/`getattr` 是类型传播 ✗

### 已查清但**未修**的项（按阻塞程度排序，供后续选择）

| 项 | 影响 | 状态 |
|---|---|---|
| 模块级列表 `.append()` 后读元素为 0 | 小（语料里 dict 已修好） | 链路已查清（批次六十七/六十八），两轮未果 |
| `-> Any` 返回类型丢结构体信息 | 小（仅我的合成复现） | 入口已确认（`infer_untyped_returns`），结构体名在 resolver 侧不可得 |
| `get`/`getattr` 的接收者类型 | 中（影响本地 shim） | 根因=注解/`Any` 不落表 |
| 数据面 6 个符号 | 大（本地实现的最后一道坎） | 未动 |


## 批次七十（2026-09-17，**已修两个符号**）：数据面最小两块

| 符号 | 做法 | 为什么诚实 |
|---|---|---|
| `np.full(n, v)` | 新 C helper `py_vec_full`（与 `zeta_dynarray_new`/`vec_push` 同一套头部布局 ⇒ 结果是普通 Vec ✓） | 语义与 numpy 一致 ✓ |
| `.tolist()`（未定型接收者） | 兜底表 → `zeta_identity`，结果 kind 记 `vec` | 我们的数组本来就是 Vec ✓；identity **不解引用** ✓ 未知接收者不会读坏数据 ✓ |

**实测**：`np.full(3,7)` → 3 / 7 ✓；`[10,20,30].tolist()` → 3 / 20 ✓；
`jq_shim.py`（LOCAL=1）未定义 **9 → 7** ✓；python_style **184/184**；官方 **194/194**。

**顺带确认（既有，非本次引入）**：未定型**形参**接收数组字面量时元素读出为 0
（去掉 `.tolist()` 的对照同样 `3`/`0` ✗）。t184 只用已定型数组 —— 不把错值写成期望值 ✓。

**为什么不实现 `np.where`**：numpy 的 `where(mask)` 返回**索引数组的元组**，
`where(mask)[0]` 取的是「整个索引数组」✗；我们没有元组表示 ⇒ 实现成「返回索引 Vec」会让
`[0]` 变成「第一个索引」✗ —— **语义不同** ✗ ⇒ 保持响亮诊断 ✓（不制造假语义 ✓）。

### 本地 shim 剩余未定义（7）

`DataFrame / concat / execute_trade / get / getattr / unique / where`

- `DataFrame` + `concat`：真正的 L2 mini-DataFrame（构造 + 列/行 + `sort_values`）—— 最大的一块
- `get` / `getattr` / `execute_trade`：接收者类型未知（注解/`Any` 不落表）
- `unique`：可做（Vec 去重 ✓，语义一致 ✓）—— 下一轮候选
- `where`：**不做**（元组语义，见上 ✗）


## 批次七十一（2026-09-17，**架构方向修正 + 改动回滚**）：DataFrame 应该是**库实现**

用户指出：**DataFrame 也应该是库实现**（不该是编译器特例）。这个方向是对的 ✓，而且项目里
**已有**这套设施 ✓ —— 我按它试了一轮，撞到两个具体障碍，按纪律回滚。

### 已有设施（不需要新造）

| 设施 | 状态 |
|---|---|
| 库源码位置 | `pylib/*.py|.z`、`build/stubs/`、`~/.zeta/packages`（`find_py_module_file` 逐个搜 ✓） |
| 库内可声明运行时函数 | `extern fn name(a: i64) -> i64;` ✓（`build/stubs/std/time.z` 就是这么写的 ✓） |
| 模块成员调用路由 | 批次五十已做：`pd.DataFrame(...)` → `pandas__DataFrame` ✓ |
| 包管理 | `zorb install` ✓（纯 Python 包 ✓） |

于是写了 `pylib/pandas.z`（列映射模型：一列 = 一个列表；`DataFrame` 类 + `concat` 函数 ✓），
**零编译器特例** ✓。实测：库**确实被加载** ✓（`imported module pandas from pylib/pandas.z` ✓）、
`_DataFrame::column_names` 等符号**确实生成了** ✓。

### 障碍一：模块同时存在于**注册表**时，本地文件被忽略

`registry.txt` 里已有 `M pandas`（批次五十五为 `pd.Timestamp → PyDate` 句柄加的 ✓，
而句柄标签是**注册表独有**的表达力 ✗ 库文件给不了 ✗）。原逻辑是「注册表赢，本地文件忽略」✗。
我改成**两者并存**（库作为补充 ✓）—— 库加载成功 ✓，但 **t179_pandas_datetime 回归** ✗：
库的存在改变了 `pd.Timestamp` 的解析 ✗。

### 障碍二：库内**持有 map 的字段**丢失类型

`self.data.keys()` → 未定义 `_keys` ✗（字段类型细化只覆盖 str/f64/bool ✗，map 未覆盖 ✗）。
试了给细化加 map 分支 ✗ —— 未生效 ✗（字段声明仍是 i64 ✗，与批次六十三同一处机制 ✓）。

### 回滚与复核

`git checkout` 两个文件 ✓、`pylib/pandas.z` 移到 `/tmp/` ✓，python_style 复核通过 ✓。

### 下一步（把方向坐实的两件事，都很具体）

1. **让「补充」路径优先注册表成员**：模块既在注册表又有库文件时，注册表里**已登记的成员**
   （`Timestamp`/`Timedelta` → 句柄 ✓）仍走 C shim ✓，库文件只补**未登记**的名字（`DataFrame`/`concat` ✓）
   —— 这样 t179 不会回归 ✓；
2. **字段类型细化覆盖 map**（批次六十三那处 ✓），库内 `self.data.keys()` 才能解析到 `map_keys` ✓。

> 这两件做完，`pylib/pandas.z` 就能真正替代编译器特例 ✓ —— 也正是用户指出的方向 ✓。


## 批次七十二（2026-09-17，**找到统一障碍：`type_decls` 不共享**，改动回滚）

按批次七十一的两步走，两个障碍的**共同根因**现在清楚了 —— 就是批次六十二/六十五那个
**「克隆 vs 共享」**问题，这次跨的是**模块边界**。

### 障碍一的真正原因（不是「注册表遮蔽」，也不是路由）

t179 的「回归」其实**不是** `pd.Timestamp` 被抢走 ✓ —— 而是 `pylib/pandas.z` 里的
`self.data.keys()` 变成未定义符号 `_keys` ✗，而库会被**每一个** import pandas 的程序加载 ✓
⇒ **链接失败** ✓。我加的「注册表成员优先」守卫是对的 ✓ 但没解决这个 ✗。

### 障碍二：字段类型细化在**调用点**做，而字段声明在**库自己的 MirGen** 里

- `DataFrame.__init__(self, data)` 的 `data` **无注解** ✗ ⇒ 库侧把字段声明成 `i64` ✗
- 唯一能提供真实类型的地方是**调用点**（`pd.DataFrame({...})` ✓ 实参是 map ✓）
- 但调用点在**导入方**的 MirGen ✗，而 `self.data.keys()` 是在**库的** MirGen 里 lower 的 ✗
- 两边的 `type_decls` 是**各自克隆**的 ✗ ⇒ 调用点的细化**回不到库** ✗

我在路由路径上补了同一套细化（批次六十三那套 ✓）—— **仍未生效** ✗，因为库侧看不到 ✓。

### 结论：这是一个**架构性**障碍，不是三个独立 bug

批次六十二（子 MirGen 克隆）、六十五（resolver 侧 `type_decls` 为空）、
七十一/七十二（库与导入方各自的克隆）**是同一件事** ✗：
**结构体/字段声明表需要在所有 MirGen（含子上下文、含被导入模块）之间共享** ✓。

⇒ 下一步应是**一次重构**：把 `type_decls` 换成共享句柄（`Rc<RefCell<…>>` 或等价物 ✓），
让它天然跨子上下文与模块 ✓ —— 这一件能同时解掉上面三处 ✓，也是让
`pylib/pandas.z` 真正替代编译器特例的前提 ✓（用户指出的方向 ✓）。

**回滚**：两个源文件 `git checkout` ✓、`pylib/pandas.z` 移到 `/tmp/` ✓、python_style 复核 ✓。


## 批次七十三（2026-09-17，**重构范围与验收先落地**；本轮改动回滚）

按上一轮的承诺，先把「共享声明表」重构的**范围与验收**写清楚，再动代码。同时本轮发现了一个
**改变重构形状**的事实。

### 新事实：MirGen 里其实有**两张**声明表

```rust
type_decls:        HashMap<String, TypeDecl>,   // 工作副本
shared_type_decls: HashMap<String, TypeDecl>,   // 累积/种子表
```

`lower_to_mir()` 每个 item 开始时都会：

```rust
self.type_decls.clear();
self.type_decls.extend(self.shared_type_decls.iter().map(...));   // ← 重新播种
```

⇒ **在 `type_decls` 里做的细化，下一个 item 就被抹掉** ✗ —— 这是批次六十二/六十三/六十五
「细化回不去」的**直接机制** ✓（比「克隆」这个说法更精确 ✓）。

### 本轮试过（无证据，已回滚）

| 改动 | 结果 |
|---|---|
| 子 MirGen 继承 `shared_type_decls`（而非工作副本） | 无可见变化 ✗ |
| 细化后同步写回 `shared_type_decls`（新增 `persist_struct_refinement`） | 无可见变化 ✗ |

构造了两个隔离用例（跨 item 读字段、子上下文读字段）——**pre-fix 与 post-fix 输出相同** ✗
⇒ 按纪律回滚 ✓（python_style 复核 **184/184** ✓）。

### 重构范围（Scope）

把 `type_decls` **单一化并共享**，跨三种边界都成立：

1. **resolver → 各模块 MirGen**（同一张表 ✓，不再每模块克隆 ✗）
2. **父 MirGen → 子 MirGen**（闭包 / 嵌套 class ✓）
3. **item → item**（`lower_to_mir` 的 re-seed 不再丢弃细化 ✓）

形态：`Rc<RefCell<HashMap<String, TypeDecl>>>`（编译器单线程 ✓，已确认无 `Send`/`thread::spawn` 使用 ✓）。
`type_decls` 与 `shared_type_decls` **合并成一个句柄** ✓（工作副本保留为局部临时表亦可 ✓）。

### 验收（Acceptance）

| # | 判据 |
|---|---|
| 1 | python_style **184/184**、官方 **194/194** 不回归 |
| 2 | 现有 t181（结构体字段类型跟随实参）、t180（嵌套 class）、t179（pandas 日期）全绿 |
| 3 | 恢复 `pylib/pandas.z` + resolver 的「补充加载」后：`pd.DataFrame({...})` 的 `.column_names()`/`.n_rows()`/`concat` 输出正确，且**不再有 `_keys` 未定义符号** |
| 4 | 三个隔离用例（跨 item / 子上下文 / 跨模块读字段）从「0 或指针」变成正确值 |

> 3 与 4 是这次重构的**真正目的** ✓：让 `pylib/pandas.z` 这类**库实现**能替代编译器特例 ✓
> （用户指出的方向 ✓），并让「字段类型细化」在所有上下文里都成立 ✓。


## 批次七十四（2026-09-17，**共享表重构做了但验收不达标 ⇒ 回滚**；查到最后一层）

按批次七十三的验收动手做了 `Rc<RefCell<…>>` 共享，**做完并编译通过**，但**验收第 3/4 条不达标** ⇒ 回滚。

### 实际做了什么（都已回滚）

1. `MirGen.shared_type_decls` → `Rc<RefCell<HashMap<…>>>` ✓（新增 `with_shared_type_decls` ✓）
2. 子 MirGen（闭包/嵌套 class）拿**同一个句柄**，不再克隆 ✓
3. 细化后 `persist_struct_refinement()` 写回共享表 ✓（否则 `lower_to_mir` 每 item 重新播种会抹掉 ✓）
4. **Resolver 也持一个句柄**并传给每一次 `lower_to_mir` ✓（**程序级共享** ✓：所有 item、所有被加载模块 ✓）

编译通过 ✓、python_style **184/184** ✓、官方 **194/194** ✓ —— 但：
- 隔离用例（`childfld`：子上下文读字段）仍是 **0** ✗
- `pylib/pandas.z` 仍是 **`_keys` 未定义** ✗

⇒ **无验收证据** ⇒ 回滚 ✓（`git checkout` 两个文件 + 库文件移出 ✓）。

### 查到的最后一层：这是**顺序**问题，不是「共享」问题

即使表是程序级共享的 ✓，**库的方法在什么时候被 lower** ✗ 与**调用点什么时候做细化** ✗ 是两件独立的事：
库的 `column_names`（含 `self.data.keys()`）按自己的 item 顺序被 lower ✓，
而「`pd.DataFrame({...})` 的实参是 map」这个证据是在**导入方**的调用点才知道的 ✗ ——
细化**来得太晚** ✗，方法体里已经把字段按 i64 定型了 ✗。

⇒ 要让库真正可用，字段类型必须在**定义时**就已知 ✓ —— 也就是**库自己标注** ✓
（`def __init__(self, data: map)` ✓ —— 这是**库该做的事** ✓，正是用户指出的方向 ✓）。

### 下一步（一个很窄的探针）

我试了在库里写 `data: map` ✗ —— **仍未生效** ✗（`_keys` 依旧 ✗）。
所以下一个要回答的问题非常具体：**`def __init__(self, data: map)` 的注解 `map` 为什么没有变成字段类型 `map`** ✗
（探针位置：class 脱糖时的 `init_params` → 字段表 ✓，以及 `Type::from_string("map")` 的实际返回 ✓）。
这一步不需要动共享表 ✓ —— 共享表那套（批次七十三的范围）**先搁置** ✓，因为验收证明它不是当前的瓶颈 ✗。


## 批次七十五（2026-09-17，**两个确凿缺陷已定位，但链路仍差最后一环 ⇒ 回滚**）

按批次七十四的窄问题（「`data: map` 注解为什么没变成字段类型 `map`」）查下去，**找到两个确凿缺陷**，
但整条链**仍差最后一环**，验收不达标 ⇒ 按纪律回滚 ✓。

### 缺陷一：字段类型对「参数右值」硬编码成 i64 ✗

`parse_class` 里从 `self.x = <rhs>` 推字段类型：

```rust
AstNode::Var(name) if param_names.contains(&name.as_str()) => {
    // `self.x = x` — type unknown, call-site coercion adapts
    "i64".to_string()          // ← 无视参数的注解 ✗
}
```

⇒ `def __init__(self, d: map)` 的字段仍被记成 `i64` ✗。
改成「取该参数声明的类型」后，**探针确认生效** ✓：`decls=[("C", [("d", "map")])]` ✓（此前是 i64 ✗）。

### 缺陷二：脱糖把 `self` 的类型写成字面量 `"Self"` ✗

```rust
let mut new_params = vec![("&mut self".to_string(), "Self".to_string())];
```

⇒ 方法体内 `self` 的类型是 `Named("Self")` ✗（不是类名 ✗）⇒ 字段读**根本找不到结构体** ✗
（探针：`base_ty=Some(I64)` ✗）。改成类名后，字段读仍拿不到 `map` ✗。

### 仍差的一环

两个缺陷都修掉之后，`_keys` **仍然未定义** ✗ ⇒ 说明 `self.d.keys()` 的**接收者类型**在方法体内
仍未成为 `Named("map")` ✗。下一个探针位置很明确：**方法体内 `.keys()` 调用点的接收者类型**
（即字段读返回的 `field_ty` 与 `type_map` 实际落表值 ✓），以及 map 方法分派分支的条件 ✓。

### 回滚与复核

`git checkout` 两个文件 ✓、python_style **184/184** ✓。
两个缺陷的**位置与修法都写在这里** ✓ —— 下一轮直接落这两处 + 一次探针即可 ✓，不必重新定位 ✓。


## 批次七十六（2026-09-17，**已修**）：方法体内 `self.<字段>` 的类型链打通 —— 库实现开始可用

`pylib/pandas.z`（**库实现** ✓ 用户指出的方向 ✓）此前链接失败于 `_keys`。查到是**三个独立缺陷**串联：

| # | 缺陷 | 位置 | 修法 |
|---|---|---|---|
| 1 | `self.x = x` **硬编码 i64**，无视参数注解 | `parse_class` | 取参数声明的类型（探针确认 `decls=[("C",[("d","map")])]` ✓） |
| 2 | 脱糖把 `self` 的类型写成字面量 `"Self"` ✗ | `parse_class` | 改为类名；导入模块里结构体注册为 `模块__类名`，形参落表也接受该拼写 |
| 3 | 形参落表只认 f64/bool/str/泛型/句柄 | `gen.rs` | 补上「已注册的 struct」 |

**双向验证**（t185）：pre-fix `Undefined symbols`（`_keys`）✗ → post-fix `1` ✓。

**实测**：`jq_shim.py`（LOCAL=1）未定义 **7 → 6** —— `concat` 消失 ✓，它现在由 `pylib/pandas.z`
作为**库代码**提供 ✓，`pd.DataFrame({...})` 的最小路径跑出正确值 ✓（`.n_columns()` = 1 ✓）。
python_style **185/185**；官方 **194/194**。

**同时落地**（库能替代编译器特例的前提）：
- resolver：模块既有内置 shim 又有本地库文件时，**库作为补充加载** ✓（不再被忽略 ✓）；
- 调用点：注册表**已登记**的成员仍走 C shim ✓（否则库会抢走 `pd.Timestamp` 的句柄语义 ⇒ t179 回归 ✓）。

### 已知遗留（下一步）

1. 从**外部**读字段（`c.d.keys()`）或带**参数下标**（`self.d[k]`）会**崩溃** ✗ —— 与本次三处无关 ✓
2. `DataFrame` 在 shim 里仍是未定义符号 ✗（可能是别的调用形态 ✓，如 0 参或未定型实参 ✓）
3. 数据面其余：`execute_trade` / `get` / `getattr` / `unique` / `where`


## 批次七十七（2026-09-17，**已修**）：字段下标作为表达式会段错误

`self.d["a"]` **段错误** ✗（不是错值，是崩溃）。两个原因叠加：

1. **表达式下标路径没有 map 分支** ✗ —— 赋值路径（`d[k] = v`）早已处理 map ✓，表达式路径没有 ✗
   ⇒ 落到数组分支，把 map 句柄**当数组读** ✗
2. `MirStmt::DictGet` 的 codegen 用 `load_local(map_id)` ✗，而**字段读没有 alloca** ✗
   ⇒ 基址必须先**物化到局部槽** ✓

**修法**：补 map 分支 ✓ + 基址先 `Assign` 到新局部槽 ✓ 再 `DictGet` ✓。

**双向验证**：`self.d["a"]` 段错误 ✗ → **1** ✓（t185 扩展）。
python_style **185/185**；官方 **194/194**。

### 仍崩溃（独立问题，下一步）

**未注解**形参做键：`def get(self, k): return self.d[k]` ✗ 崩溃；
注解 `k: str` 时正常 ✓（对照 `f(d: map, k: str)` = 1 ✓）。
⇒ 疑似 `lower_map_key` 对**未知类型**的键不做内容哈希 ✓（或做了别的处理 ✓），
下一个探针位置：`lower_map_key` 在「键类型未知」时的行为 ✓。


## 批次七十八（2026-09-17，**已修**）：已知 struct 的方法胜过「不透明兜底」（段错误）

`c.get("a")`（`C` 有 `get` 方法）**段错误** ✗。IR 证据：它被降级成**数组下标** ——
键 `"a"` 变成了**元素偏移** ✗：

```llvm
%elem_ptr = getelementptr i64, ptr %array_ptr, i64 ptrtoint (ptr @str_lit.31 to i64)
```

原因：不透明兜底表有一条启发式 `("get", 2) => array_get` ✓，它在**用户方法解析之前**命中 ✗
⇒ 把 struct 句柄当数组读 ✗。

**修法**：兜底之前先判断「接收者是**已知 struct** 且该 struct **有同名方法**」✓，有则跳过兜底 ✓。

**双向验证**：pre-fix **段错误**（139）✗ → post-fix `1` ✓。
python_style **185/185**（t185 扩到 4 条断言 ✓）；官方 **194/194**。

### 仍取不到值（独立问题，下一步）

**未注解**形参做键（`def get(self, k): self.d[k]`）→ **0** ✗（不再是崩溃 ✓，但是错值 ✗）。
根因：map 的**声明类型里没有键类型** ✗（`Named("map", [])` ✓），而 `lower_map_key` 只按**键自身**的
静态类型决定是否内容哈希 ✗。试过「按 map 的键类型决定」✗ —— 实测无效 ✓，已回滚 ✓。

⇒ 下一步可选：让 map 的类型**带上键类型**（`map[str, T]` ✓，注解与字面量都记录 ✓），
这样内容哈希的判定就有可靠依据 ✓ —— 这同时能改善 `keys()`/`items()` 的元素类型 ✓。


## 批次七十九（2026-09-17，**探到新事实，改动回滚**）：map 键类型的路走不通，因为构造调用不走那条细化路径

目标：让 map 的类型**带上键类型**（`map[str, T]` ✓），这样「未注解形参做键」的内容哈希判定就有依据 ✓。

### 探到的三个事实

1. **字典字面量本来就带键类型** ✓ —— `gen.rs:3060` 是 `Type::Named("map", vec![key_ty])` ✓
   （`{"a": 1}` → `Named("map", [Str])` ✓）。缺的只是**字段/注解侧**没有键类型 ✗
   （注解写 `map` ✗ ⇒ `Named("map", [])` ✗）。
2. **`from_string` 的泛型拼写是 `lt(Name, Args)`** ✓（不是 `<...>` ✗）——
   所以字段表里要写 `lt(map, str)` ✓ 才会解析成 `Named("map", [Str])` ✓。
3. **构造调用 `C({...})` 根本不经过那条细化块** ✗ —— 探针放在细化循环入口 ✓ **一次都没触发** ✗
   ⇒ 说明 `C(...)` 由**别的分支** lower ✓（IR 里是普通 `call i64 @C(...)` ✓，但 lowering 路径不同 ✓）。

### 试过什么（无证据，已回滚）

- 给细化补 map 分支（记录 `lt(map, str)` ✓）
- 重新加回 `lower_map_key_typed`（按 **map 的**键类型决定内容哈希 ✓）并在表达式下标处使用 ✓

结果：`ext2` 仍是 **0** ✗（探针显示 `map_ty=Named("map", [])` ✓ 未变 ✓）
⇒ 两个改动**均无效果** ✓，按纪律回滚 ✓（python_style 复核 ✓）。

### 下一步（很具体）

1. 找到 `C({...})` 究竟由哪条分支 lower ✓（探针：在「大写名回退」/「类构造」/「普通调用」三处各放一个 ✓，
   看哪一处对 `C({...})` 触发 ✓）——**只有先找到这条路径，字段键类型的细化才有地方落** ✓；
2. 顺带：`from_string` 的泛型拼写差异（`lt(...)` vs `<...>` ✓）值得在文档/注释里写明 ✓，
   否则以后还会有人（包括我）写 `<...>` 然后发现「没生效」✗。


## 批次八十（2026-09-17，**上轮结论被自己的探针推翻**，改动回滚）

上一轮我写「构造调用 `C({...})` 根本不经过那条细化块」✗ —— **这是错的** ✗。本轮在**细化块入口**
放探针，实测：

```
PROBE_CTOR freecall base=C found=true args=1
```

⇒ 细化块**确实被走到** ✓，`base=C` 也能在 `type_decls` 里找到 ✓，`arg_ids` 有 1 个 ✓。
上一轮的「没触发」是我**探针放错了位置** ✗（放在循环体里，而循环前有 `if let Some(Struct)` 判定 ✓）。

### 那么真正的原因只有一个

细化块里的 `concrete` 匹配**只有** `Str/F64/F32/Bool` ✗ —— map 实参落进 `_ => continue` ✗
⇒ 字段保持注解给的 `map`（无键类型 ✗）⇒ `lower_map_key_typed` 拿不到键类型 ✗
⇒ 未注解形参做键时按**指针**探测内容哈希的 map ⇒ 静默取不到值 ✗。

**修法（下一轮直接落，位置已确认）**：给该 `concrete` 匹配补一个 map 分支，
记录 **`lt(map, str)`** ✓（`from_string` 的泛型拼写是 `lt(Name, Args)` ✓，不是 `<...>` ✗），
并允许把 `"map"` 升级为带键类型的拼写 ✓；再把 `lower_map_key_typed` 用在表达式下标处 ✓。

### 本轮为何没落成

两次尝试都卡在**补丁锚点匹配**上 ✗（缩进与文件实际内容不符 ✗），编译未通过 ✓，
按纪律 `git checkout` 回滚 ✓（python_style 复核 ✓）。**没有任何未验证改动留在树里** ✓。

> 教训：这轮的价值是**推翻了上轮的结论** ✓ —— 探针位置错了会得出错误的根因，
> 而错误根因会让后面几轮的修法全部落空 ✗。位置确认（`base=C found=true`）之后，
> 下一轮只需一次正确的补丁 ✓。


## 批次八十一（2026-09-17，**已修**）：map 的键类型决定内容哈希

`def get(self, k): return self.d[k]`（键**未注解**）返回 **0** ✗（静默取不到值）。
根因：`lower_map_key` 只按**键自身**的静态类型决定内容哈希 ✗；键未注解 ⇒ I64 ⇒
对内容哈希的 map 按**指针**探测 ⇒ 查不到 ⇒ 0，无诊断 ✗。

**三处修法**：
1. `lower_map_key_typed(iid, map_ty)` ✓ —— 按 **map 声明的键类型**决定 ✓，表达式下标改用它 ✓
2. 调用点细化补 map 分支 ✓（记录 `lt(map, str)` ✓；`from_string` 的泛型拼写是 `lt(Name, Args)` ✓）
3. **可靠来源是注解** ✓ —— 调用点细化受 **lower 顺序**限制 ✗，所以库应写
   `def __init__(self, d: lt(map, str))` ✓（实测 `c.get("a")` = **1** ✓）

**双向验证**（t186）：pre-fix `0` ✗ → post-fix `1` ✓。
python_style **186/186**；官方 **194/194**；`jq_shim.py` 未定义 **6**（未变 ✓）。

**给库补注解**：`pylib/pandas.z` 的 `__init__` 改 `data: lt(map, str)` ✓，文件头写明
「泛型拼写是 `lt(...)`，键类型必须写出来」✓。

**试过但无效（已回滚）**：把细化**持久化**到 `shared_type_decls` ✗ —— 实测无效 ✓，
因为这是**顺序**问题（方法先 lower ✗），不是「共享」问题 ✓（批次七十四的结论再次被证实 ✓）。

**已知遗留**：`pylib/pandas.z` 完整路径仍**段错误** ✗（在 `column()`/`n_rows()` 一带 ✓），
与本次三处无关，尚未定位。


## 批次八十二（2026-09-17，**段错误已消除，取值仍不对**）：已知 struct 的方法调用改用限定名

`a.column("code")[1]`（`DataFrame` 来自 `pylib/pandas.z`）**段错误** ✗。IR 铁证：

```llvm
%51 = call i64 @column(...)                  ; ← 平名 free call ✗
%dict_get = call i64 @map_get(ptr %map_ptr5, i64 1)   ; ← 对 Vec 做 map 下标 ✗
```

**定义端**是 `@"DataFrame::column"` ✓（限定名 ✓）；平名 `@column` 是**无关的桩** ✗
（还有 `@column_inst_i64` ✓），返回值类型 i64 ✗ ⇒ `[1]` 被当成 map 下标 ✗ ⇒ 崩溃 ✓。

**修法**：接收者类型是**已知 struct** 时，方法调用发**限定名** ✓（与定义端一致 ✓）。

**实测**：段错误（139）✗ → **退出码 0** ✓；python_style **186/186**；官方 **194/194**。

### 仍不正确（下一步）

| 表达式 | 实测 | 期望 |
|---|---|---|
| `a.n_rows()` | **0** ✗ | 2 |
| `a.column("code")[1]` | 打印**空** ✗ | `y` |

⇒ 疑似**限定名调用点的返回类型**查不到 ✓（`func_ret_types` 的键可能是平名或别的拼写 ✗），
返回值被定型为 i64 ✗，链式读取自然错 ✗。
下一个探针：**限定名方法的 `func_ret_types` 键**（打印表里的键集合 ✓）。


## 批次八十三（2026-09-17，**两处已修 + 新发现槽位冲突**）：方法调用按唯一限定名解析

`a.n_rows()`（库 `DataFrame`）返回 **0** ✗。探针：

```
PROBE_RT method=n_rows receiver_ty=Some(I64) matching_keys=["n_rows", "DataFrame::n_rows", …]
PROBE_FN method=n_rows receiver_ty=Some(I64) chosen_func=DataFrame::n_rows   ← 修后 ✓
```

- 接收者类型**未知**（`pd.DataFrame(...)` 的结果类型未被跟踪 ⇒ I64 ✗）⇒ 平名解析到**无关的桩** ✗
- 修法：接收者不是 struct/array 时，若 `func_ret_types` 里以 `::<method>` 结尾的键**唯一** ✓ 就用它 ✓
  （多于一个候选保持平名 ✓ 不猜 ✓）；`::` 限定名**不做 arity 后缀** ✓（定义端没有 ✗）

**实测**：`chosen_func=DataFrame::n_rows` ✓；python_style **186/186**；官方 **194/194**。

### 新发现的编译器缺陷（下一步，IR 铁证）：局部变量与 `self` 槽位冲突

```llvm
store i64 %0, ptr %11          ; self
store i64 %19, ptr %11         ; names = list(...)   ← 写进了 self 的槽 ✗
%21 = call i64 @vec_len(i64 %20)   ; 算的是 len(self) 而不是 len(names) ✗
```

⇒ 库方法内的局部变量 `names` **复用了 `self` 形参的槽** ✗ ⇒ 方法内部算错 ✓。
独立缺陷 ✓，下一个探针位置：`name_to_id`/局部槽分配时 `self` 别名的处理 ✓。


## 批次八十四（2026-09-17，**把「槽位冲突」精确到 codegen 层**，无代码改动）

批次八十三发现库内 `n_rows` 的局部变量 `names` 与 `self` 写进同一个 alloca ✗。本轮把它缩小了范围。

### 三个对照实验（都**正常** ✓）

| 用例 | 位置 | 结果 |
|---|---|---|
| `names = list(self.d.keys())` + `len(names)` | 主文件的方法 | **1** ✓ |
| 完整 `n_rows` 形状（含 `if` + `names[0]`） | 主文件的方法 | **2** ✓ |
| 同一个类放在**模块**里（`from framelib import Frame`） | 独立模块 | **2** ✓ |
| 同一个类放在 `/tmp/pandas.z`（注册表里也有 `M pandas` 的**补充**情形 ✓） | 独立模块 | **2** ✓ |

⇒ **不是**「方法里的局部变量」问题 ✗，**也不是**「模块/注册表补充」问题 ✗。

### 探针把范围钉到 codegen

```
PROBE_SLOT assign name=names existing=None self_id=Some(1) next_id=7
```

⇒ MIR 层**没有**冲突 ✓：`self` 是 id 1 ✓、`names` 拿到**全新** id 7 ✓。
但 IR 里两者写的是**同一个 alloca** ✗：

```llvm
store i64 %0, ptr %11          ; self (id 1)
store i64 %19, ptr %11         ; names (id 7)  ← 同一个 alloca ✗
```

⇒ 冲突在 **codegen 的局部 alloca 分配/命名** ✗，不在 MIR ✓。
（这也解释了为什么三个对照用例都正常 ✓：它们的 id 布局不同 ✓。）

### 下一步（探针位置已明确）

在 codegen 的「为局部 id 分配 alloca」处打印 **id → alloca 名** 的映射 ✓，
对 `pylib/pandas.z` 的 `n_rows` 看 id 1 与 id 7 为什么落到同一个 alloca ✓。
复现命令：`zetac /tmp/pl_a.py`（期望 2，实测 0）✓。

> 本轮无代码改动 ✓，树保持绿 ✓ —— 价值在于把「槽位冲突」这个模糊说法
> 变成了「codegen alloca 分配」这个**具体位置** ✓，并排除了三种可能 ✓。

## 批次八十五（2026-09-17，**库类方法链打通**）：`n_rows()` 返回正确值

批次八十二/八十三的三处修复合起来让**模块里的类方法**真正可用：

| # | 修复 |
|---|---|
| 1 | 方法调用按**唯一的限定名**解析（接收者类型未知时平名落到无关的桩） |
| 2 | `::` 限定名**不做 arity 后缀**（定义端没有） |
| 3 | map 的键类型由注解 **`lt(map, str)`** 带进来，`lower_map_key_typed` 才能内容哈希 |

**实测**（真库路径）：`n_rows()` → **2**（此前 0）、`n_columns()` → 2；
新增 **t187**（fixture 模块里的类 + 方法内局部变量 + `list(...)` + 链式下标）→ **PASS**；
python_style **187/187**；官方 **194/194**；`jq_shim.py`（LOCAL=1）未定义 **6**（未变）。

### 重要更正：批次八十三/八十四的「槽位冲突」判断**不成立**

复测发现对照用例全对、codegen 探针显示**每个 id 都有自己的 alloca** —— 当时的错误判断被
`/tmp/pandas.z` 这个**陈旧副本**污染了：编译器按「**源文件目录 → pylib**」顺序搜模块，
而我把测试副本放在了 `/tmp` ⇒ 测的一直是副本。删掉副本后复测为正确值。

> 教训：**测量环境本身也会骗人**。以后测 `pylib/` 里的库，必须确认**没有同名副本**在源文件目录里
> （或把用例放在独立目录）。

### 仍未解决（下一步）

`a.column("code")[1]` 取元素：一种写法打印**空**、另一种（先赋值再取）**段错误**。
`len(a.column("code"))` = 2 说明**列本身是对的** ⇒ 问题在**取元素**那一步。

## 批次八十六（2026-09-17，**把「取元素」缩到 codegen 的限定名解析**，改动回滚）

目标：`c = f.column("code")` 之后 `c[1]` —— 实测 `c[0] == "x"` 为 **0** ✗、`print(c[1])` **段错误** ✗。

### 已确认的链路事实

| 观察 | 结论 |
|---|---|
| `len(f.column("code"))` = **2** ✓ | 列本身正确 ✓ |
| MIR 探针：`PROBE_FN2 method=column receiver_ty=Some(Named("Frame", [])) chosen=Frame::column` ✓ | **MIR 层选对了限定名** ✓ |
| IR 里定义是 `@"Frame::column"` ✓，而调用是 `@column` ✗ | **转换发生在 codegen** ✗ |

⇒ 下一处探针位置很明确：codegen 的 `get_or_declare_function` ✓ ——
看 `"Frame::column"` 是否**在** `::` 拆分/`_N` 剥离这些回退**之前**被查到 ✓
（现在的行为像是：限定名查不到 ⇒ 回退到平名 ⇒ 命中一个**无关的桩** ✗）。

### 试过什么（无验证效果，已回滚）

1. 让 `from_string` 认识 `lt(vec, T)` / `lt(list, T)` ⇒ `DynamicArray(T)` ✓
   （给库一个「返回列表」的拼写 ✓）
2. 给 `pylib/pandas.z` 的 `column`/`column_names` 加 `-> lt(vec, str)` ✓
3. 给测试 fixture 的 `column` 加同样的返回注解 ✓

结果：**段错误依旧** ✗ ⇒ 无验证效果 ⇒ 全部回滚 ✓（python_style 复核 **187/187** ✓）。

> 说明：第 1 条本身是**有用能力** ✓（库需要一种写法表达「返回列表」✓），
> 但它不是当前瓶颈 ✗ —— 瓶颈在 codegen 的限定名解析 ✓。等那一处修好后可以再回来加 ✓。

## 批次八十七（2026-09-17，**找到 codegen 里的「同名两个函数对象」**，改动回滚）

上一轮把范围缩到 codegen 的 `get_or_declare_function` ✓，本轮**直接验证**了它：

### 实验：在 `::` 分支里**优先**查精确限定名

在 `name.contains("::")` 分支最前面插入 `self.module.get_function(name)` ✓ —— 结果：

```
Undefined symbols: "_Frame__column", referenced from: _main in ct4.o
```

⇒ 精确名 `Frame::column` **查不到** ✗ ⇒ 回退到 `__` 变形 ✓ ⇒ 但那个 `Frame__column`
**没有定义** ✗ ⇒ 链接失败 ✓。（改动前则回退到平名 `column` ✓ —— 那个**存在** ✓，
但是个**无关的桩** ✗ ⇒ 静默错值/段错误 ✗。）

⇒ 结论：**同一个方法存在两个函数对象** ✓ ——
- `@"Frame::column"` = **真正的定义** ✓（IR 里能看到 ✓，来自 `mir.name` ✓）
- `Frame__column` = 另一个注册项 ✗（无定义 ✗）

而 `get_function("Frame::column")` 在**调用点**返回 **None** ✗ —— 像是**注册/顺序**问题 ✓
（定义在 IR 里排在 `main` 之前 ✓，但 codegen 侧的注册未必 ✓）。

### 下一步（探针位置）

在调用点打印 `self.module.get_function("Frame::column")` 的结果 ✓ **以及** `module` 里
所有含 `column` 的函数名 ✓ —— 看真正的定义是以什么键注册的 ✓。
（已回滚本轮改动 ✓，python_style **187/187** ✓、官方 **194/194** ✓。）

## 批次八十八（2026-09-17，**找到真凶：方法参数叫 `name` 会让该方法根本不生成**）

批次八十七的探针在调用点打印了模块里的函数名，结果**决定性**：

```
PROBE_FQ name=Frame::column exact=false fns=["Frame::n_columns", "n_columns"]
```

⇒ **`Frame::column` 根本不存在** ✗（同一个类里 `n_columns` ✓ 在 ✓）—— 不是「查到错的函数」✗，
而是**这个方法压根没被生成** ✗。

### 定位：把参数从 `name` 改名成 `key`

```
PROBE_FQ name=Frame::column exact=true fns=["Frame::column", "column", "column_inst_i64"]
```

⇒ 方法出现了 ✓、调用正确 ✓、`len(f.column("code"))` = **2** ✓、**段错误消失** ✓。

**⇒ 编译器缺陷：方法参数名叫 `name` 时，该方法不会被生成** ✗（与方言内部的 `name` 标识符冲突 ✗，
大概率在方法脱糖/签名收集那一带 ✓ —— 下一个探针：`def column(self, name)` 时 `func_ret_types`
与 `module` 里该方法的注册情况 ✓）。

### 已落地

- `pylib/pandas.z` 的 `column(self, name)` → `column(self, key)` ✓（**绕开**该缺陷 ✓），
  并在文件头写明这个坑 ✓；
- 实测：库路径**不再段错误** ✓、`len(...)` = 2 ✓；
- python_style **187/187** ✓、官方 **194/194** ✓。

### 仍差一步（已定位）

`a.column("code")[1]` = **0** ✗（应 `y`）—— 列本身对 ✓（`len` = 2 ✓），
但**方法返回值的类型**未知 ✗ ⇒ 取元素按 i64 ✗。
我试了给方法加 `-> lt(vec, str)` 并让 `from_string` 认识 `lt(vec, T)` ⇒ `DynamicArray(T)` ✗ ——
**本轮未验证出效果** ✗（该注解可能没走到方法返回值那条路 ✓），故**已回滚** ✓。
下一步：查**方法返回值注解**是否落进 `func_ret_types` ✓（探针位置明确 ✓）。

## 批次八十九（2026-09-17，**已修**）：方法调用不再回退到「无关的桩」

批次八十八把症状记成「参数名叫 `name` 会让方法**不生成**」✗ —— 本轮查明**更准确**的机制 ✓：

- `Frame::column` **确实会被生成** ✓（IR 里有 ✓）；
- 但在**调用点**它可能**尚未生成**（创建顺序 ✗）⇒ `get_or_declare_function` 的 `::` 分支
  先试变形名 `Frame__column` ✗（无定义 ✗）、再试**平名 `column`** ✗ ——
  而平名恰好**存在**（一个返回 i64 的重复体 ✗）⇒ 命中它 ⇒ **静默错值 / 段错误** ✗。
- 参数名改成 `key` 之所以「好了」✗，只是**改变了创建顺序/注册布局** ✓，治标不治本 ✗。

**修法**：精确限定名查不到时，**直接按限定名声明**（定义可能在本模块稍后生成 ✓），
**不再回退**到变形名或平名 ✓。

**双向验证**（t188，方法参数就叫 `name`）：
- pre-fix：解析到平名桩 ⇒ 错值/段错误 ✗
- post-fix：`len(f.column("code"))` = **2** ✓、`n_columns()` = 1 ✓

python_style 187 → **188/188**；官方 **194/194**。

### 仍差一步（已定位）

`a.column("code")[1]` = **0** ✗（应 `y`）—— 列的**长度**对 ✓，但方法**返回值的类型**未知 ✗
⇒ 取元素按 i64 ✗。需要「方法返回值注解」落进 `func_ret_types` ✓（`lt(vec, str)` 那条路 ✓）。

## 批次九十（2026-09-17，**返回值类型注解：找到了入口，但转换点有第三个**，改动回滚）

目标：让方法返回值的类型进到调用方 —— `a.column("code")[1]` 取元素 ✓。

### 探针证据（决定性）

```
PROBE_RET2 base=DataFrame::column found=Some(Named("vec", [Str]))
```

⇒ **注解确实到达了签名** ✓（`-> lt(vec, str)` 生效 ✓），但被转成 **`Named("vec", [Str])`** ✗
（而不是 `DynamicArray(Str)` ✗）—— 所以调用方**无法**把结果当列表索引 ✗ ⇒ 取元素按 i64 ⇒ 0 ✗。

### 我改了三个「注解→类型」的转换点，都没生效

| 位置 | 改动 | 结果 |
|---|---|---|
| `Type::from_string`（types/mod.rs） | `lt(vec, T)` → `DynamicArray(T)` | 无变化 ✗ |
| `parse_type_string`（new_resolver.rs） | 同上 | 无变化 ✗ |
| 脱糖处尊重显式返回注解（top_level.rs） | 显式 `->` 优先于 str/i64 启发式 | 注解确实到了签名 ✓（但转换点不是这三处之一 ✗） |

再加一个「在使用点归一化 `Named("vec"/"list")` → `DynamicArray`」✗ —— **反而让 `pl_c` 从 2 退化成 0** ✗
⇒ 已回滚 ✓（python_style 复核 **188/188** ✓）。

### 结论 / 下一步

`func_ret_types` 的键与值都对了 ✓（`DataFrame::column` → `Named("vec", [Str])` ✓），
但**转换发生在第四个地方** ✗（不是 `Type::from_string` ✗、不是两个 `parse_type_string` ✗）。
下一个探针位置：**方法签名注册处**（`self.funcs.insert(...)` 的 ret 参数 ✓）——
直接在那里把 `lt(vec, T)` 落成 `DynamicArray(T)` ✓，或在 `get_all_func_signatures` 出口归一化 ✓。

> 注意：在使用点归一化**看起来**更省事 ✓ 但实测**引入回归** ✗（`pl_c` 2→0 ✗），
> 所以这条路要带着回归测试走 ✓，不能凭「应该更好」就上 ✓。

## 批次九十一（2026-09-17，**方法返回值：注解已生效但转换点仍未找到**，改动回滚）

目标：让方法返回值的类型进到调用方（`a.column("code")[1]` 取元素）。

### 本轮的两个确定事实

1. **脱糖处「显式返回注解优先」确实生效** ✓ —— 打开后，签名里的 ret 从 `Some(I64)` 变成
   `Some(Named("vec", [Str]))` ✓（探针 `PROBE_RET3` ✓）。**这一处是对的** ✓（本轮回滚了 ✓，
   但与下一处配合时它必须一起上 ✓）。
2. **`lt(vec, str)` 的转换点仍未找到** ✗ —— 已经试了**四个**候选，全部无效果 ✗：
   `Type::from_string`（types/mod.rs）✗、`parse_type_string`（new_resolver.rs）✗、
   `parse_type_string`（typecheck_new.rs）✗、以及「在使用点归一化」✗（后者还会**引入回归** ✗）。

⇒ ret 一直是 `Named("vec", [Str])` ✗ ⇒ 调用方按 i64 索引 ⇒ `a.column("code")[1]` = 0 ✗。

### 下一步（换方法：不再逐个试，改用探针定位）

在**每个** `parse_type_string` / `from_string` 的入口打印「输入以 `lt(` 开头」的那次调用 ✓，
一次编译就能看出**哪个函数**真的在处理它 ✓ —— 而不是继续逐个打补丁试 ✗
（本轮四次尝试全无效，说明「猜哪个转换点」这条路已经走不通 ✗）。

### 回滚与复核

四处改动全部回滚 ✓，python_style **188/188** ✓、官方 **194/194** ✓。

## 批次九十二（2026-09-17，**返回值类型修对了，但暴露新症状**，改动回滚）

上一轮留下的两半，本轮**接上了** ✓ —— 而且拿到了决定性证据 ✓：

```
PROBE_RET4 base=DataFrame::column ret=Some(DynamicArray(Str))    ← 类型终于对了 ✓
```

⇒ 需要**两处同时**（缺一无效，这解释了此前四次尝试为何全部落空 ✗）：

1. **脱糖处「显式返回注解优先」** ✓（否则注解被 i64 启发式覆盖 ✗）
2. **`lower_to_mir` 里对 `vec`/`list` 归一化** ✓（把 `Named("vec", [T])` 变成 `DynamicArray(T)` ✓，
   放在**签名表交给 MIR 的唯一出口** ✓，一处覆盖所有注解解析器 ✓）

### 但暴露出**新症状**（所以回滚 ✓）

打开这两处后：
- `ret` 类型正确 ✓（`DynamicArray(Str)` ✓）
- 可是 `len(a.column("code"))` 从 **2 掉到 0** ✗ —— 列**变空**了 ✗

⇒ 返回类型注解**改变了方法体的 lowering** ✗（`return self.d[key]` 的取值被影响 ✓），
这是一个**新的**、更靠内的缺陷 ✓，不该带着它提交 ✓。

### 回滚与复核

四处改动全部回滚 ✓（python_style **188/188** ✓、官方 **194/194** ✓）。

### 下一步（两件事，顺序清楚）

1. **先把「脱糖显式注解优先」单独上** ✓（它本身是正确行为 ✓，且已验证会让 ret 从 `I64` 变成
   `Named("vec")` ✓）—— 但要先确认它**不改变**现有用例的取值 ✓（本轮它带出了「列变空」✗，
   需要先查清是不是它的锅 ✓）；
2. 再上「`lower_to_mir` 归一化」✓（纯类型层 ✓，应无运行时影响 ✓）。

> 探针位置：注解打开后，打印 `column` 方法体里 `self.d[key]` 那一处的取值来源 ✓
> （`DictGet` 的 key 是否仍被内容哈希 ✓、返回的 Vec 是否为空 ✓）。

## 批次九十三（2026-09-17，**已修**）：方法的显式返回注解生效

`label(self) -> str: return self.s` 返回的字符串此前被**脱糖处的 str/i64 启发式覆盖** ⇒
`print(c.label())` 打印**指针**（`4340556770`）✗。

**两处配合**（缺一无效 ✓ —— 这也解释了批次八十六～九十一四次尝试为何全部落空 ✗）：

1. **脱糖处显式注解优先** ✓（未注解的方法标记为 `"()"` 才走启发式 ✓）；
2. **签名表出口归一化** ✓（`lower_to_mir`：`vec`/`list` → 数组类型 ✓，
   否则库的 `-> lt(vec, str)` 变成 `Named("vec", [T])` ✗，调用方无法当列表索引 ✗）。

**双向验证**（t189）：pre-fix `4340556770`/1 ✗ → post-fix `hi`/1 ✓。
python_style **189/189**；官方 **194/194**。

### 更正批次九十二的错误结论

当时写「打开这两处后列变空（2→0）」✗ —— 实测是**误判** ✗：`pl_c` 的 **0 就是当时的基线** ✓
（与两处改动无关 ✓），两处改动**没有改变任何取值** ✓（2/0/0 前后一致 ✓）。
⇒「`pylib/pandas.z` 的 `column` 返回空」是一个**独立的既有问题** ✓，与本批无关 ✓。

## 批次九十四（2026-09-17，**最小库端到端跑通**：`2` 和 `y`）

批次九十三的两处修复（脱糖显式注解优先 + 签名表出口归一化）合起来，让**库里的类方法返回列表**
第一次端到端跑通 ✓：

```
/tmp/frametest/pandas.z   (class DFrame + column(self, key) -> lt(vec, str))
/tmp/frametest/main2.py   from pandas import DFrame; a = DFrame({"code": ["x","y"]})
                          print(len(a.column("code")))   → 2 ✓
                          print(a.column("code")[1])     → y ✓
```

IR 也确认方法体正确 ✓：`map_str_key(key)` → `map_get(map, hashed_key)` → `ret` ✓。

### 但真实 `pylib/pandas.z` 仍然失败 ✗

同一个最小类放进 `pylib/pandas.z`（或带**额外方法** `column_names`/`n_columns`/`n_rows`/`concat` 的版本）
⇒ `len(a.column("code"))` = **0** ✗ / 打印**空** ✗。
⇒ 差异在**额外方法**（或模块级 `concat`）✓ —— 下一个探针：**二分那些额外方法** ✓
（把 `pylib/pandas.z` 的方法逐个删掉 ✓，看哪一个一删就好 ✓）。

### 回滚与复核

库与 fixture 的注解改动**已回滚** ✓（在真实 `pylib/pandas.z` 里未验证出效果 ✗），
python_style **189/189** ✓。

> 本轮的意义：**「库里的类返回列表」这条链第一次拿到正确值** ✓（最小库 ✓），
> 并且把剩余问题缩小到「真实库的额外方法/模块级函数」✓ —— 这是一个可二分的问题 ✓，
> 不再是「不知道卡在哪」✗。


## 批次九十五（2026-09-18，**已修**）：真实 `pylib/pandas.z` 的 `column` 返回空/段错误

批次九十四把问题缩到「真实库 vs 最小库」差异，本轮二分**不是**额外方法，而是更简单的漏改：

- 批次八十八把形参从 `name` 改成 `key`（形参叫 `name` 会让方法不生成）
- 但方法体仍写 `return self.data[name]` ⇒ `name` **未定义** ⇒ 段错误（exit 139）/空列
- **修法**：体改为 `self.data[key]`；保留返回注解 `-> lt(vec, str)`（取元素需要）

**双向验证**：
- pre-fix（体用 `name`）：`len(a.column("code"))` → exit **139**
- post-fix（体用 `key`）：输出 `2` / `x` / `y` / `1`（`n_columns`）✓

回归：`t190_pandas_column.z`；python_style **189 → 190**；官方 **194/194**。

> 教训：改参数名时**体里的同名引用必须一起改**；「额外方法干扰」是假线索，最小 diff 先查体。


## 批次九十六（2026-09-18，**已修**）：`pylib/pandas.z` 的 `concat` 空结果/段错误

批次九十五修好 `column` 后，`concat([a,b])` 仍空或崩。探针拆出**三处独立陷阱**（都在库侧绕过，编译器缺陷另记）：

1. **形参类型**：`frames` 必须写成 `frames: [DataFrame]`。未标注时 `frames[i]` 丢掉 struct 字段（`.data` 空/空指针）。`lt(vec, DataFrame)` **不够**。
2. **while 条件里的 `len`**：`while j < len(colnames)` **恒假**（循环体永不进）。必须先 `ncols = len(colnames)` 再 `while j < ncols`。
3. **map 取出的列表做 `prev + col`**：得到**空列表**。改为按元素拼：`merged = merged + [col[k]]`。

**实测**：`concat([DataFrame({"code":["x"]}), DataFrame({"code":["y"]})])` → `n_rows=2`，列 `x`/`y` ✓。

回归：`t191_pandas_concat.z`；python_style **190 → 191**；官方 **194/194**；`t190` 不回退。

> 记下的编译器缺陷（未修，库侧已绕过）：① 未标注 `[T]` 形参的元素类型；② `while … < len(…)`；③ 从 map 取出的 vec 的 `+`。


## 批次九十七（2026-09-18，**已修**）：`while j < len(xs)` 循环永不进入

`while j < len(colnames)`（`colnames` 来自方法返回值）曾**恒不进循环**。

**根因**：`AstNode::While` 把条件副作用（`array_len` Call）塞进 `body`，codegen 在 `while.cond` 只 `load` 条件槽 —— 首次判断时槽未写 ⇒ 当成 0 ⇒ 跳过。字面量列表的 `len` 被常量折叠故幸免。

**修法**：`MirStmt::While` 增加 `pre_cond: Vec<MirStmt>`；条件副作用放这里；codegen 在 `while.cond` 里先跑 `pre_cond` 再取条件（`continue` 也走这条路径）。

回归：`t192_while_len_method.z`（期望 `2`）；python_style **191 → 192**；官方 **194/194**。
`pylib/pandas.z` 的 `ncols = len(...)` 绕过可保留（防御），行为与直接 `while j < len(...)` 一致。


## 批次九十八（2026-09-18，**已修**）：map 取出的列表丢失类型（`+` 变整数加 / 下标变 map_get）

`a = m["code"]; a + b` 曾得到垃圾长度；`a[0]` 走 `map_get` 打出 0。

**根因**：`DictGet` 把结果一律标成 `I64`；map 类型只带键（`lt(map, str)`），不带值。

**修法**：
1. dict 字面量推断值类型 → `Named("map", [key, val])`
2. `DictGet` 传播 `params[1]`（`vecstr`/`lt(vec,T)` 归一成 `DynamicArray`）
3. `d[k] = v` 插入时 refine map 的值槽
4. `pylib/pandas.z`：`data: lt(map, str, vecstr)`；`concat` 改回 `merged + col`

回归：`t193_map_list_concat.z`；python_style **192 → 193**；官方 **194/194**；t190/t191 不回退。

## 批次九十九（2026-09-18，**已修**）：类 `__getitem__` / `df["col"]`

`f["code"]` / `a["code"]`（DataFrame）曾 SEGFAULT；显式 `f.__getitem__("code")` 能调用但 `c[0]` 打出 `0`。

**根因（两处）**：
1. Named 接收者下标落到 DictGet，把对象指针当 map。
2. 方法返回类型查找对含 `_` 的限定名做 `rsplit_once('_')`：`F::__getitem__` → `F::__getitem`，注解 `-> lt(vec, str)` 丢失；顺带暴露 `column_names` 以前**碰巧**查到 `column` 的列表类型才“能用”。

**修法**：
1. `Subscript`：已知 struct 有 `__getitem__` 时调用 `Type::__getitem__(base, key)`（同 `__enter__` / 方法限定名）
2. 限定名 / `module__name` / `zeta_*`：**不再**切 arity 后缀查返回类型
3. `pylib/pandas.z`：加 `__getitem__`；`column_names` 补 `-> lt(vec, str)`

回归：`t194_getitem_df.z`；python_style **193 → 194**；官方 **194/194**；t190/t191 不回退。

## 批次一百（2026-09-18，**已修**）：`DataFrame.empty` / `.columns` 属性

语料写 `if not df.empty` / `df.columns`；此前 `empty`/`columns` 不是 struct 字段，
`FieldAccess` 读布局外垃圾指针 → 非空垃圾值恒真，`not df.empty` **静默永不进支**。

**修法**：
1. MIR `FieldAccess`：Named 接收者、名字**不是**真字段、且存在**仅 `self` 的零参方法**
   （带返回类型）时，改发 `Call Type::prop(base)`（property 形态）
2. `pylib/pandas.z`：加 `empty() -> bool`（`n_rows()==0`）、`columns() -> lt(vec, str)`

探针 `/tmp/pdtest/empty.py`：`a.empty`→0 / `ok`；`DataFrame({})`→`b.empty`→1；
`len(a.columns)`→1 / `code`。空 `{}` 构造正常（`n_rows` 对零列返回 0）。

回归：`t195_df_empty_columns.z`；focused t195/t194/t191/t190 全绿；
python_style **194 → 195**；官方 **194/194**。

## 批次一百零一（2026-09-18，**已修**）：`len(obj)` → `__len__`

语料 / pandas 写 `len(df)`；用户类写 `def __len__(self)`。此前 Named 接收者
落到 `array_len`，句柄无 array header → **恒返 0**（静默错长）。

**修法**：
1. MIR `len(...)`：`Type::Named`（非 map/dict/PyJson）且存在 `Type::__len__`
   （或唯一 `__len__` 候选）时，发 `Call Type::__len__(arg)`，否则仍 `array_len`
2. `pylib/pandas.z`：加 `__len__` → `n_rows()`（与 Python `DataFrame.__len__` 一致）

探针：`len(F())`→3（IR 见 `F::__len__`）；`len(DataFrame({"code":[…]}))`→2；
空 `DataFrame({})`→0。

回归：`t196_len_dunder.z`；focused t196/t195/t194/t191/t190 全绿；
python_style **195 → 196**；官方 **194/194**。

## 批次一百零二（2026-09-18，**已修**）：`obj[k]=v` → `__setitem__` + 字段 map 插入物化

语料 / pandas 写 `df["col"] = [...]`；用户类写 `def __setitem__(self, key, val)`。
此前 Named 接收者落到 `DictInsert`，把对象指针当 map → **SEGFAULT**。
方法体内 `self.d[key]=v` 虽已走 map 分支 + `map_str_key`，但 codegen 的
`DictInsert` 只 `load_local(map_id)`——`FieldAccess` 无 alloca → **仍 SEGFAULT**
（`self.d["a"]` 读路径早已物化，写路径漏了）。

**修法**：
1. MIR 下标赋值：Named（非 map）且存在 `Type::__setitem__`（或唯一候选）时，
   发 `Call Type::__setitem__(base, key, val)`，否则保留 DictInsert
2. map 的 `DictInsert`：先 `Assign` 物化 base 到 local（与 DictGet 对称），再插入
3. `pylib/pandas.z`：加 `__setitem__` → `self.data[key]=val`

探针：`F()["a"]=1` IR 见 `F::__setitem__` + `map_str_key` + struct field load；
`DataFrame` 列覆写/新增列打印 `y`/`z`。

回归：`t197_setitem.z`；focused t197/t196/t195/t194/t191 全绿；
python_style **196 → 197**；官方 **194/194**。

## 批次一百零三（2026-09-18，**已修**）：`df.shape` / `"col" in df` / `df.copy`

语料写 `s = df.shape; s[0]`、`"code" in df`、`b = df.copy()` 后再改列。
此前：`shape` 若当字段读会拿到垃圾；`in DataFrame` 静默 false；`copy` 无返回注解时
调用点把结果当 i64，`b["col"]=`/`len(b)` 不走 dunder（SEGFAULT / 错长）。

**修法**：
1. `pylib/pandas.z`：`shape() -> lt(vec, i64)`（零参属性分派已有）；
   `__contains__` → `key in self.data`（map 成员）；`copy(self) -> DataFrame`
2. MIR：`key in map` 的 DictGet 先 Assign 物化 FieldAccess（与下标/ DictInsert 对称），
   并用 `lower_map_key_typed`（未注解 `key` 形参在 type_map 里是 I64）

探针：`shape[0]`→2；`"code" in a`→1 / `"nope" in a`→0；`copy` 后改列 `len`→2、首元 `y`。

回归：`t198_shape_contains_copy.z`；focused t198/t197/t196 全绿；
python_style **197 → 198**；官方 **194/194**。

## 批次一百零四（2026-09-18，**已修**）：`del obj[k]` / map.pop / keys

语料写 `del df["col"]`、类上 `__delitem__` + `self.d.pop(key)`。此前：
1. `del` 被当标识符，下标变 GET（SEGFAULT / 静默无删）
2. `parse_stmt` 加 `parse_del` 后 nom `alt` 超 21 臂编译失败
3. `parse_class` 丢掉 `parse_func` 提升出的 `ret_expr` —— 单语句方法体（含 `__delitem__`）变成空 stub
4. 形参 `lt(map, str)` 规范化成 `map<str>` 后仍落 I64，`d.pop` 链到裸 `_pop`

**修法**：
1. `parse_del`：`del obj[k]` → `obj.__delitem__(k)`；与 `parse_pass` 嵌套进 `alt`
2. `parse_class`：**保留** `ret_expr`（`__init__` 折回 body）
3. MIR：`map`/`map<…>`/`lt(map,…)` 形参写入 `Named("map", …)`；已有 `zeta_map_pop` + `lower_map_key_typed`
4. `pylib/pandas.z`：`__delitem__` / `keys` / `to_dict` / `head`（`head` 列截断仍 SEGFAULT，用例不覆盖）

探针：`F({"a":1,"b":2}); del f["a"]` → `0`/`1`；`del a["v"]` + `keys()[0]` → `0`/`1`/`1`/`code`。

回归：`t199_delitem_keys_head.z`（无 head）；focused t199/t198/t197 全绿；
python_style **198 → 199**；官方 **194/194**。

## 批次一百零五（2026-09-18，**已修**）：`DataFrame.head(n)` SEGFAULT

语料写 `df.head(1)`。批次一百零四已挂 `head`，列截断仍 SEGFAULT，用例未覆盖。

**二分**（最小探针）：
1. `return self` → 通
2. `DataFrame(dict(self.data))` → 通
3. 字面键 + `col[:n]` → 通；字面键 + `append`/`[col[0]]` → 通但首元 `(null)`
4. `self.column_names()` 动态键 / 完整 while+append → **SEGFAULT**

**根因（编译器）**：同类方法内 `self.other()` 若返回 **vec**，调用方读到的元素为 **0**（`self.names2()→["code","v"]` 亦然；`self.n_columns()` 等标量返回正常）。把 0 当列名查 map → 空指针 → SEGFAULT。另：`append`/`[col[i]]` 拼 **str 列**会写出 `(null)`（独立正确性缺口）。

**修法（库绕过，编译器缺口未动）**：`pylib/pandas.z` `head`：
1. `cols = list(self.data.keys())`（不调 `self.column_names()`）
2. `out[key] = col[:n]`（不用 while/append）

探针：`head(1)` → `len` 1、首元 `x`。

回归：`t200_df_head.z`；focused t200/t199 全绿；
python_style **199 → 200**；官方 **194/194**。

## 批次一百零六（2026-09-18，**已修**）：`self.method()` 返回 vec 丢类型

批次一百零五用库绕过；本批修编译器。

**根因**：导入模块里 struct 被 mangle 成 `pandas__DataFrame`，`self` 在方法体内是
`Named("pandas__DataFrame")`，但 impl 方法仍注册为 `DataFrame::column_names`。
调用点拼出 `pandas__DataFrame::column_names` → **不在 `func_ret_types`** → 返回默认
`I64` → `cols[0]` 走 `map_get(key=0)` 得 0 → 当列名查 map → SEGFAULT。
（本地未 mangle 的 `class C` / 外部 `c.names()` 碰巧正常。）

**IR**：调用点 `type_map` 对 `C::names`/`DataFrame::column_names` 应为 `DynamicArray(Str)`；
错时为 `I64`，下标发 `DictGet`/`map_get` 而非 `array_get`。

**修法**（`gen.rs`）：`resolve_struct_method`——`tn::method` 缺失时试 `__` 后缀去前缀
（`pandas__DataFrame` → `DataFrame::method`），再唯一 `::method` 候选。Named 方法调用与
`struct_has_method` / `with` 协议共用。`pylib/pandas.z` `head` 改回 `self.column_names()`。

回归：`t201_self_method_vec_ret.z`；python_style **200 → 201**；官方 **194/194**。

## 批次一百零七（2026-09-18，**已修**）：列向量 `.values` 恒等（语料 `sub[f].values`）

语料写 `{f: sub[f].values for f in fields if f in sub.columns}`；列映射模型里
`sub[f]` 已是 list，`.values` 应为恒等。此前对 DynamicArray 走裸 FieldAccess，
读布局外垃圾 → **SEGFAULT**。

**候选排序**（探针）：
1. **`col.values` / `sub[f].values`** — SEGV（本批）
2. `df.values` / `df.index` — 编译通、打出垃圾指针（struct 缺属性；与一百同类）
3. `df.drop` / `rename` — 链接失败（方法未实现）；kwargs 列表实参另有静默错
4. 空 list `append(str)` 再 `print` — 打指针数（类型丢；经 DF 取回仍对）
5. `1 in series` — SEGV（列非 Series）

**修法**（`gen.rs`）：
1. FieldAccess：`field=="values"` 且 base `is_array_like` → Assign 恒等，保留元素类型
2. opaque_fallback：`("values", 1)` → `zeta_identity`；identity+vec 时保留接收者 DynamicArray 元素类型（避免 tolist/values 退化成 `I64` 元素）

探针：`[10,20].values[0]`→10；`DataFrame(...)[f].values[0]`→`x`。

回归：`t202_col_values.z`；focused t202/t201/t200/t199 全绿；
python_style **201 → 202**；官方 **194/194**。

## 批次一百零八（2026-09-18，**已修**）：`df.drop` + `df.index`

语料/探针写 `df.drop(columns=[...])`；列映射可用 copy+del 表达。`df.values`
（嵌套矩阵）难做；`df.index` 可降为 `range(n_rows)` 列表。

**探针**：
1. 方法 kwargs 多参（`labels=`/`columns=`）—— 名匹配静默失败（columns 恒 None）
2. **单参** `drop(labels)` —— 位置 `drop(["x"])` 正确；任意 kwargs 值绑到唯一形参，
   故 `drop(columns=["x"])` **碰巧可用**（与探针拼写一致）
3. 单 str `drop("x")` + for-in —— SEGV/挂起；只用 `lt(vec, str)` 列表形
4. `df.index` —— 此前 FieldAccess 垃圾指针；零参方法 → `list(range(n_rows()))`

**修法**（`pylib/pandas.z`）：
1. `drop(self, labels: lt(vec, str)) -> DataFrame`：`out=self.copy()`；`for k in labels: del out[k]`
2. `index(self) -> lt(vec, i64)`：`list(range(self.n_rows()))`（属性分派已有）
3. `df.values` / 多参 kwargs 名匹配 / 单 str drop —— **延后**

回归：`t203_df_drop.z`；python_style **202 → 203**；官方 **194/194**。

## 批次一百零九（2026-09-18，**已修**）：`df.rename(columns={old: new})`

候选：A rename kwargs dict；B `for c in "ab"` 挂起（影响 `drop("x")`）；C 其它。

**探针**：
1. **A**：缺方法 → `_DataFrame__rename` 链接失败。`columns: lt(map, str, str)` 时
   kwargs `columns={…}` **可用**；未注解形参绑 dict 则 `len==0`/查键静默空。
2. **B**：`for c in "ab"` 走 `array_len`/`array_get`（str 非数组）→ 死循环打垃圾 /
   `drop("x")` SEGV。属编译器 for-in，本批不修。
3. 选 **A**（库侧小修，语料热、探针已确认 kwargs dict）。

**修法**（`pylib/pandas.z`）：
`rename(self, columns: lt(map, str, str)) -> DataFrame`：按 `column_names` 拷列，
`key in columns` 则用新名写入 out map。

回归：`t204_df_rename.z`；python_style **203 → 204**；官方 **194/194**。

## 批次一百一十（2026-09-18，**已修**）：`for c in "ab"` 字符迭代

**根因**：`gen.rs` 集合 for-in 一律 `array_len`/`array_get`；`Type::Str` 是
`char*`，`array_len` 读伪长度 → 死循环/垃圾输出；连带阻塞单 str
`drop("x")`（若对 labels 做 for-in）。

**修法**：`coll_is_str` 时改走 `str_len`/`str_get`，元素类型标 `Type::Str`
（与 `s[i]` 下标路径一致）。

**未做**：`drop("x")` 单 str 重载——多字符列名不能靠 for-in 字符拆；应
`del out[labels]` 或包成 `[labels]`，另批。

回归：`t205_for_in_str.z`；python_style **204 → 205**；官方 **194/194**。

## 批次一百一十一（2026-09-18，**已修**）：`df.drop("col")` 单 str

**探针**（for-in-str 修后）：
1. `drop("v")` / `drop("code")` 仍 SEGV——形参 `lt(vec, str)`，for-in 走 `array_len` 把
   `char*` 当 vec 头。
2. 库内 `isinstance(labels, str)` **不可行**：未注解形参默认 i64；注解 vec 则
   isinstance 看声明类型（恒 list/否），不是实参运行时值。
3. 手写 `drop([lab])` / `drop(labels: str)` + `del out[labels]` 均可用。

**修法**：MIR 调用点——`method=="drop"` 且非 self 实参 `Type::Str` 时包成一元
`StackArray`（与 `[lab]` 同形）。库仍 `for k in labels: del out[k]`。

回归：`t206_df_drop_str.z`；python_style **205 → 206**；官方 **194/194**。

## 批次一百一十二（2026-09-18，**已修**）：空 list `append(str)` 类型丢失

**候选排序**（探针）：
1. `index.tolist()` — 已通（identity + index 列表）
2. `1 in list` / `"x" in col` — 已通
3. `df.values` 矩阵 — len=0 静默错，列映射难做，**延后**
4. `reset_index`/`fillna`/`astype` — 链接缺 `_DataFrame__reset_index`，库侧另批
5. **空 `[]` + `append("hi")` + `print`** — 打指针数；`len` 恒 0（本批）

**根因**：
1. `[]` 标成 `Array(I64, Literal(0))` → `len()` 常量折叠 0；`append` 后类型不更新
2. 元素仍当 I64 → `println_i64`（指针当整数）；数据其实已写入（`xs[0]=="hi"` 为真）
3. `xs: lt(vec, str) = []` 的注解被忽略，只抄 RHS

**修法**（`gen.rs`）：
1. 空 `[]` → `DynamicArray(I64)`（`len` 走 `vec_len`）
2. `append`/`push` 回写句柄时按推入值 refine → `DynamicArray(Str)` 等
3. `TypeAnnotatedPattern` 尊重 `lt(vec, str)` 注解

回归：`t207_empty_append_str.z`；python_style **206 → 207**；官方 **194/194**。

## 批次一百一十三（2026-09-18，**已修**）：reset_index/fillna/astype typed copy + groupby 响亮

**候选排序**（探针 + 语料热度）：
1. **`df.values` 矩阵** — `len==0`/type=int 静默错；方法内拼 list-of-lists 可行但
   下标/注解易 SEGV（嵌套元素类型丢），**继续延后**
2. **`reset_index`** — 链接缺 `_DataFrame__reset_index`（响亮）；语料 25 处多为
   `reset_index(drop=True)`（本批）
3. **`fillna`/`astype`** — 未实现时走 opaque `zeta_identity`→结果标 **i64**，
   `len(columns)` 静默 **0**（比链接失败更糟）
4. **`groupby`** — 同 identity 静默打垃圾指针；列映射表达不了分组

**修法**：
1. `pylib/pandas.z`：`reset_index`/`fillna`/`astype` → `self.copy()`（`-> DataFrame`），
   让 `struct_has_method` 压过 opaque identity
2. `gen.rs`：从 opaque identity 名单**去掉 `groupby`** → `_DataFrame__groupby` 链接失败（响亮）

**未做**：真 NaN fill、dtype 转换、`df.values` 行矩阵、groupby 语义。

回归：`t208_df_reset_fillna_astype.z`；python_style **207 → 208**；官方 **194/194**。

## 批次一百一十四（2026-09-18，**已修**）：语句形 `assert` 响亮失败

**候选排序**（探针）：
1. **`df.values` 列主矩阵** — 方法内 `lt(vec, vecstr)` + append 可编译，但调用点嵌套元素类型丢失
   （`v[0][0]`→0 / SEGV）；注解回写仍丢。真矩阵需编译器嵌套返回类型，**继续延后**
2. **`assert False` 不响亮**（本批）— 拆成 `Var("assert")` + 条件两个 ExprStmt，皆空操作；
   `assert(cond)` 调用形本已走 `zeta_assert_fail`
3. `sort_values`/`merge`/`to_csv`/`iloc` — 行级/IO，列映射只能 identity 或链接失败，非语义赢

**根因**：无 `parse_assert`；与旧 `del`/`raise` 同病（关键字当标识符）。
`assert False, "msg"` 还把后续源码留在 remaining 里静默丢掉。

**修法**（`stmt.rs`）：
1. `parse_assert`：`assert cond` / `assert cond, msg` → `Call assert(...)`（复用 MIR）
2. 后接 `(` 时退回，保留 `assert(...)` 调用形
3. 挂入 `alt((parse_pass, parse_del, parse_assert))`

**未做**：`df.values` 嵌套返回类型；真 NaN/`sort_values`。

回归：`t209_assert_stmt.z`；python_style **208 → 209**；官方 **194/194**。

## 批次一百一十五（2026-09-18，**已修**）：pandas tail/dropna + 列 unique + numpy 自由名

**范围**（跳过 JoinQuant 平台 API）：类别 2–6 探针后落地最高影响项。

### 探针结论

| 项 | 失败形态 | 本批 |
|---|---|---|
| `df.tail` / `df.dropna` | 裸 `_tail`/`_dropna` 链接失败 | **库实现** |
| `col.unique()` / `xs.unique()` | 裸 `_unique` | **runtime `zeta_vec_unique`** |
| `df.where` | 列映射表达不了行 mask | **保持响亮**（`_DataFrame__where`） |
| bare `arange`/`linspace`/`sum` | 已通（MIR） | — |
| `np.arange`/`linspace`/`sum`/`asarray` | 注册表缺员 → 幽灵 `_arange(module,…)` | **registry** |
| bare `asarray` | 幽灵 `_asarray` | **MIR 恒等** |
| `getattr` 无类型接收者 | 仍响亮诊断 | **不动**（静默 default 会错值） |
| 本地模块依赖图 | 无小赢 | **跳过** |
| `ETF动量EPO` / `epo` | 停在 `I @ corr` —— **`@` 矩阵乘未解析**（非 `lambda_`） | **非 quick，跳过** |

### 修法

1. `pylib/pandas.z`：`tail(n)`（`start=len-n; col[start:]`，负切片仍 SEGV）；`dropna()` → typed `copy()`（无 NaN 哨兵）
2. `runtime/py_additions.c`：`zeta_vec_unique` 保序去重（指针等或 `strcmp`）
3. `gen.rs` opaque：`unique`→`zeta_vec_unique`（返回 **DynamicArray**，勿保留 `Array(_, Literal(n))` 否则 `len` 常量折叠成去重前长度）；`dropna`→identity（Series）；`where` **不入** identity
4. `registry.txt`：`numpy.{arange,linspace,sum,asarray}` → `zeta_arange` / `zeta_linspace_i64` / `zeta_sum_vec` / `zeta_identity`
5. MIR：bare `asarray(x)` 恒等；顺带修正 arange/linspace 的 `type_map` 为 DynamicArray

回归：`t210_df_tail_dropna` / `t211_col_unique` / `t212_numpy_free_names` / `t213_where_loud`；
python_style **209 → 213**；官方 **194/194**。

### 类别 2–6 仍开放

2. **where**：真行 mask / `np.where(cond,x,y)` 未做；`dropna` 真 NaN/axis/subset 未做
3. numpy：`arange` 多参/float、`asarray(dtype=)`、`sum` 轴/NaN 传播未做
4. **getattr**：无静态类型接收者 + default 仍响亮（有意）；已知 struct + 字面量属性名已通
5. 本地模块依赖图：未动
6. **epo / `@`**：~~卡在 matmul 解析~~ → **批次一百一十六已修**（解析清零；runtime 仍为 i64 桩）

## 批次一百一十六（2026-09-18，**已修**）：Python `@` matmul 解析

**范围**：类别 2–6 继续，优先 ETF动量EPO 的 `@`（跳过 JoinQuant 平台 API）。

### 探针

| 项 | 失败形态 | 本批 |
|---|---|---|
| 最小 `a @ b` | W1002 从 `@ b` 起丢 2 行 | **parser** |
| `def f(): c = a @ b` | 整个 `def` 被丢 | **同上** |
| `epo` 签名 `lambda_` | 无 W1002（已通） | — |
| ETF动量EPO 全文 | W1002 **77** 行，从 `def epo` 起 | **修 `@` 后 → 0** |
| `getattr(p,"x")` 已知 struct | 已通 | **跳过** |

### 修法

1. `expr.rs` `parse_multiplicative`：`@` 与 `*`/`/`/`%` 同级（`["**","*","/","%","@"]`）
2. `gen.rs`：`op == "@"` → `Call zeta_matmul`（避免 fallthrough 发名为 `@` 的自由调用）
3. `py_additions.c`：`zeta_matmul` = **i64 乘积桩** + stderr 警告一次（非 ndarray；真 matmul 待做）
4. 重建 gitignored `zeta_runtime_c.o`

### 度量

| | before | after |
|---|---|---|
| ETF动量EPO W1002 丢行 | **77** | **0** |
| 语料完全解析 | 37/38 | **38/38** |
| 语料未解析行合计 | 77 | **0** |

回归：`t214_matmul_at.z`；python_style **213 → 214**；官方 **194/194**。

## 批次一百一十七（2026-09-18，**已修**）：getattr 字面量扩面 + listcomp DataFrame + np.where

**范围**：类别 2–5（跳过 JoinQuant 平台；解析 #6 已清）。

### 探针（jq_shim 链接缺口，平台外）

| 项 | before | 本批 |
|---|---|---|
| `pd.DataFrame` in listcomp | 裸 `_DataFrame(env,r)` | **闭包继承 py_imports** |
| `np.where(mask)[0]` | 裸 `_where` | **MIR + `zeta_np_where1`** |
| `getattr(Point(5),"x")` / `getattr(w.p,"x")` | 响亮 `_getattr` | **Call/FieldAccess Named** |
| untyped `getattr(obj,"slip",0)` | 泛化诊断 | **指名接收者/缺类型**（仍不静默 default） |
| bare `from numpy import where` | `where` 关键字 → W1002 | **取消保留** |
| `_get` / 平台符号 | 仍缺 | **不动**（注解/`Any`/宿主） |

### 修法

1. `lower_closure`：继承 `py_module_aliases` / `py_member_aliases` / `py_user_modules` / `module_global_types`；模块别名不进 `zeta_env_get`
2. `py_struct_type_of`：支持 ctor `Call` 与嵌套 `FieldAccess`；getattr 失败诊断带接收者形状
3. `np.where`：MIR 分派 `zeta_np_where1`（扁平索引；`[0]` 形直接扁平）/ `zeta_np_where3`；registry 占位；重建 `zeta_runtime_c.o`
4. `parse_ident`：`where` 不再保留（同 `type`/`impl`）

### 度量

| | before | after |
|---|---|---|
| jq_shim 非平台裸名 | DataFrame/where/getattr/get/… | **去掉 DataFrame、where** |
| 语料完全解析 | 38/38 | **38/38** |
| 语料未解析行 | 0 | **0** |

回归：`t215_getattr_ctor_field` / `t216_df_listcomp` / `t217_np_where` / `t218_where_import` / `t219_getattr_loud`；
python_style **214 → 219**；官方 **194/194**。

### 类别 2–5 仍开放

2. **where**：`DataFrame.where` 仍响亮；`np.where` 无 axis/broadcast 细项；真行 mask 未做
3. **get**：未定型接收者仍裸 `_get`（需 map 类型传播 / `Any`）
4. **getattr**：动态名、`Any`/未注解形参、下标接收者仍响亮（有意）；平台 `g` 不在范围
5. 本地模块：闭包 import 表已修；更深跨模块默认参数等未动

## 批次一百一十八（2026-09-18，**已修**）：非平台链接缺口 —— strftime / date / numpy 数值面 / logger_*_n / getattr default

**范围**：样本策略非平台 undef（**跳过** JoinQuant：`set_level`/`info`/`get_trades`/…）。

### 探针（before → after）

| 符号/形态 | before | 本批 |
|---|---|---|
| `datetime.datetime(...).strftime` 无 `import datetime`（jqdata `*`） | 裸 `PyDate__strftime` | `py_handle_of` 走 `find_module` fallback → `py_dt_strftime` |
| `.date()` 未定型 | 裸 `date` | opaque → `py_dt_identity` |
| `np.eye` / `np.zeros((r,c))` / `diag` / `diagonal` / `fill_diagonal` | 裸名 / 元组当标量 | C 实实现 + MIR `zeros2` 展开 |
| `scipy.linalg.solve` | 裸 `solve` | **响亮桩** `zeta_np_solve_stub`（回 RHS） |
| `np.sum` / 未定型 `.sum()` / `sum(xs)` | 部分裸 | registry + `zeta_sum_vec` |
| `corr`/`cov`/`pct_change` 未定型 | 裸名 | opaque → `zeta_identity`（列映射不可表达） |
| `log.error/warning(fmt, x)` | `py_logger_*_3` 幽灵 | `py_logger_*_n`（同 info_n） |
| `getattr(untyped,"x",default)` | 幽灵 `_getattr` | **用 default** + 警告一次；无 default 仍响亮 |

### 修法（易错点）

1. **`py_handle_of`**：`datetime.datetime(...)` 在无 import 别名时仍要从 `flatten_module_receiver` + `find_module` 取 `handle=PyDate`，否则方法名被 mangling 成 `PyDate__strftime`。
2. **`np.zeros((r,c))`**：绝不能把 StackArray 指针塞进 1 参 C；MIR 展开为 `zeta_np_zeros2(r,c)`。
3. **logger 变参**：registry 固定 arity ⇒ 多参被 arity-mangle；统一走 `*_n(lg,fmt,n,a1..a4)`，V1 **不做 %-替换**但打印实参。
4. **groupby 故意不 identity**：列映射 DF 表达不了分组；identity 会静默打出垃圾指针。

### 度量（7 样本：`l1_fixed_pool_momentum` / `指数ETF动量轮动` / `Debug多标的ETF` / `安全摸狗` / `稳健型ETF` / `ETF动量EPO` / `jq_shim`）

| 口径 | 结果 |
|---|---|
| raw link | **1/7**（仅 `l1_fixed_pool_momentum`） |
| if-platform-stubbed（非平台 undef 为空） | **6/7** |
| 仍拦 | `jq_shim` → `groupby`（有意响亮）+ 平台 `execute_trade` |

回归：`t220_strftime_noimport` / `t221_numpy_eye_zeros` / `t222_getattr_default` / `t223_logger_error_n` / `t224_sum_untyped` / `t225_getattr_no_default`；
python_style **219 → 225**；官方 **194/194**。

### 仍开放（非本批）

- JoinQuant 平台 API（宿主 shim）
- `groupby` 真语义 / 未定型 `.get`
- `np.corrcoef`/`polyfit`/`var` 等自由名；真 `linalg.solve`

## 批次一百一十九（2026-09-18，**已修**）：DataFrame.groupby → Named GroupBy（链接）

**范围**：NO JoinQuant 平台。jq_shim 非平台链接拦路仅剩 `groupby`。

### 修法

1. `pylib/pandas.z`：`DataFrame.groupby(by) -> GroupBy`（`GroupBy(self.copy())`）
2. `GroupBy.mean` / `GroupBy.sum` → 空 `DataFrame({})`（库方法压过 opaque `mean→zeta_identity`，避免 Named 上静默 i64）
3. 未定型 `.groupby` 仍不进 opaque identity（保持响亮）

**未做**：真分组、`for (k,g) in gb` 迭代、按组聚合语义。

### 度量（7 样本）

| 口径 | 结果 |
|---|---|
| raw link | **1/7**（仅 `l1_fixed_pool_momentum`） |
| if-platform-stubbed | **7/7**（groupby 已消；jq_shim 仅剩平台 `execute_trade`） |

回归：`t226_df_groupby`；python_style **225 → 226**；官方 **194/194**。

## 批次一百二十（2026-09-19，**已修**）：Zeta-rewrite —— `pylib/numpy.z` + pandas 去重/迭代面

**方向**：库 API 落在 `pylib/*.z`；C/registry 只留原语（`X` + MIR 分派）。NO JoinQuant 平台 API。

### 语料 survey（`strategies/code/*.py`，相对既有 pandas.z/registry）

| 热度 | API | 本批前 |
|---|---|---|
| np 高 | `sum/mean/log/arange/asarray/linspace/where/zeros/eye` | 后半已 registry/MIR；**mean/any/vstack/log** 仍裸 |
| pd 高 | `DataFrame/Timestamp/concat` | 库 + registry 句柄 |
| DF 方法 | `drop_duplicates` / `itertuples` | **响亮** `_DataFrame__*` |
| DF 方法 | `pct_change` | opaque identity→i64 |
| DF 方法 | `reset_index(drop=False)` | 方法默认未注入，无参 drop=0 |

### jq_wufu 非平台 undef（滤平台/宿主后）

- `jq_wufu_local`：`_DataFrame__drop_duplicates`、`_DataFrame__itertuples`、`_any`、`_vstack`、`_pandas__Series`/`date_range`/…、`_Path__open`、`_isin`/`_iterrows`/`_nunique`…
- `jq_wufu`：`_any`、`_vstack`、`_dict`/`_setdefault`/`_condition`（+ 平台 `execute_trade`）

### 修法

1. **`pylib/numpy.z`**：`arange`/`asarray`/`zeros` 包装 `zeta_*`；补 `mean`/`any`/`vstack`/`log`/`isnan`/`append`；`where` 文档桩
2. **registry**：撤 `F numpy {arange,asarray,zeros}`（库赢）；**`where` 仍留 `F`**（否则 `from numpy import where` 3 参落到 1 参库函数）；保留 linspace/sum/eye/diag 的 `F` + 全部 `X`
3. **MIR**：`py_member_call` 对库 `DynamicArray` 返回标 `vec`（否则 `np.arange` 当 i64 → SEGV）
4. **`pylib/pandas.z`**：`drop_duplicates`（保序去重）、`itertuples`（行下标列表）、`pct_change`（typed copy）；`reset_index` 仍恒 copy（Named 方法默认未注入）

### 易错点

- 注册表 `F` 与 `.z` 同名时 **registered members win** —— 不撤 `F` 则库包装永不调用
- `drop_duplicates(subset)` **勿注解** `lt(vec,str)`：字面量下标 SEGV；用 for-in
- `reset_index` 无参时 drop 落到 0（与 False 同）；插 index 列会毁掉语料 `reset_index()` → 等默认注入

回归：`t227_numpy_z_wrappers` / `t228_df_dedup_itertuples`；python_style **226 → 228**；官方 **194/194**。

### 下一队列

1. Named 方法默认值注入（解锁 `reset_index(drop=False)` 真插列）
2. `np.log`/`vstack` 真语义；`itertuples` namedtuple / `row.col`
3. `pd.Series` / `to_datetime` / `isin` / `iterrows` / `nunique`
4. `Path.open` / `makedirs` 等 pathlib 面

## 批次一百二十一（2026-09-19，**已修**）：Named 方法默认 + pandas isin/nunique/iterrows/Series + numpy clip

**范围**：NO JoinQuant 平台。Zeta-rewrite 库面继续「用到再补」。

### 修法

1. **MIR**：Named 方法也走 `callee_sig_for_call`（剥 `self`）注入默认/kwargs —— `df.reset_index()` 得 `drop=True`
2. **`pylib/pandas.z`**：`reset_index` 真分 `drop`；`iterrows`/`isin`/`nunique`；最小 `Series`；`date_range`/`to_datetime`/`to_numeric` 桩
3. **opaque**：列级 `isin`/`nunique`/`iterrows`/`clip`/`min`/`max`（清裸链）；`zeta_vec_nunique` + `zeta_identity2`
4. **`pylib/numpy.z`**：`clip`/`max`/`min`；内建 **`max`/`min` 多参折叠**（`max(a,b,c)`）

### 易错点

- 方法默认必须**跳过 self 槽**，否则位置参绑到 self
- `Series.__len__` → `len(self.data)` 易递归挂死；勿加，用 `len(s.tolist())`
- 注册表勿对 `Series`/`date_range` 加 `F`（registered members win）

### 度量

| 口径 | before (批 120) | after |
|---|---|---|
| python_style | 228/228 | **230/230** |
| 官方 | 194/194 | **194/194** |
| jq_wufu_local 非平台 undef | 35 | **25** |

清掉：`_isin`/`_iterrows`/`_nunique`/`_pandas__Series`/`date_range`/`to_datetime`/`to_numeric`/`_clip`/`_max`/`_min`。

回归：`t229_method_defaults_isin` / `t230_numpy_clip_minmax`；`t208`/`t228` 更新。

### 下一队列

1. pathlib：`Path.open` / `exists` / `read_text` / `resolve`
2. `dict`/`setdefault`/`fromkeys` 未定型接收者
3. `np.log`/`vstack` 真语义；Series 真 `isin` 掩码
4. `itertuples` namedtuple / `row.col`

## 批次一百二十二（2026-09-19，**已修**）：dict/set/pathlib/isna/cast 链接面

**范围**：NO JoinQuant 平台。用到再补；跳过宿主 `query_hs300`/`register_universe`/`get_row_data`/`initialize`/`load`/`covers_range`/`set_cost_config`/`cache_clear`。

### 修法

1. **MIR**：`dict(m)` 未定型也浅拷贝；`set()` 空集；`dict.fromkeys(keys)` 1 参；`typing.cast` → 第二参；opaque `setdefault`/`isna`/`set.add`；`os.makedirs(..., exist_ok=)` 剥 `__kwarg__`
2. **pathlib**：`-> Path` / PyPath 方法早拦截；`open(encoding=)` 忽略非 mode 字符串；kwargs 多 arity 的 `read_text`/`exists`/`resolve` 只传 path
3. **库**：`pandas.isna`/`isnull`；`numpy.condition` 桩；registry `typing.cast` + `PyPath.open`；C `py_set_add` / `py_typing_cast`

### 易错点

- 解析把 `encoding="utf-8"` 收成 `__kwarg__`；勿当 `open` 的 mode
- Named(`Path`) ≠ handle `PyPath` —— 注解返回类型会走 `Path__*` 幽灵名

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 230/230 | **232/232** |
| 官方 | 194/194 | **194/194** |
| jq_wufu_local 非平台 undef | 25 | **13** |

清掉：`_dict`/`_set`/`_setdefault`/`_fromkeys`/`_add`/`_cast`/`_exists`/`_resolve`/`_Path__open`/`_read_text`/`_[dynamic]str__isna`/`_py_os_makedirs_2`。

回归：`t231_dict_set_cast_fromkeys` / `t232_path_open_isna_makedirs`。

### 仍剩（13，多为宿主/闭包）

`cache_clear` `condition` `covers_range` `get_row_data` `getattr` `import_module` `initialize` `load` `next` `query_hs300_stocks` `query_zz500_stocks` `register_universe` `set_cost_config`

## 批次一百二十三（2026-09-19，**已修**）：compiler/lang —— listcomp 自由调用捕获 / next / importlib / getattr 动态名

**范围**：NO JoinQuant 平台。跳过宿主 `query_*` / `register_universe` / `get_row_data` / `initialize` / `load` / `covers_range` / `set_cost_config` / `cache_clear`。

### 根因（易错）

| 符号 | 真因 | 误判 |
|---|---|---|
| `_condition` | listcomp `[m for m in xs if condition(m)]` 把自由调用的 **callee 名** 当成符号，未进 `collect_free_vars` | 曾加 `numpy.condition` 桩（批 122）——无关 |
| `_next` | 语料是 **`rs.next()`**（baostock），不是 builtin `next(it)` | — |
| `_import_module` | `importlib.import_module` 无 registry | — |
| `_getattr` | `getattr(mod, name)` **动态名**（`market_data.__getattr__`） | 字面量+default 路径已通 |

### 修法

1. **`collect_free_vars`**：无 receiver 的 Call 把 `method` 当自由变量；捕获时跳过 builtin / `func_ret_types` / 未绑定名
2. **Call**：局部 callee 且不在 `closure_vars` → `zeta_call_fn_arg`（保留 `f = lambda` 的直接 `__closure_N` 路径，避免打成 i64）
3. **builtin `next(it, default)`** → default + 警告；`next(it)` → `py_builtin_next` 响亮 abort
4. **opaque `.next()`** → `py_method_next` 返回 **0**（exhausted；勿用 identity，否则 `while rs.next()` 死循环）
5. **`importlib.import_module`** → registry + `py_import_module` abort
6. **动态 `getattr`** → `py_getattr_dynamic` abort（字面量无 default 的 untyped 仍故意幽灵，守 t225）

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 232/232 | **235/235** |
| 官方 | 194/194 | **194/194** |
| jq_wufu_local 焦点 undef（13） | 13 | **9** |

清掉：`condition` `getattr` `import_module` `next`。

仍剩（全宿主，本批跳过）：`cache_clear` `covers_range` `get_row_data` `initialize` `load` `query_hs300_stocks` `query_zz500_stocks` `register_universe` `set_cost_config`。

回归：`t233_listcomp_condition_capture` / `t234_next_import_getattr` / `t235_importlib_module`。

## 批次一百二十五（2026-09-19，**已修**）：sources 整模块可编 —— del多目标 / 类继承 / `...` / with+return

**范围**：NO JoinQuant。让 `market_data_sources.py` 本地函数真正进 `.o`。

### 根因链

| 拦路 | 后果 |
|---|---|
| `del a, b` 只吃第一个目标 | 顶层 `try: … del _load_dotenv, _Path` 炸 → **整文件从 try 起全丢** |
| `class S(abc.ABC)` 显式 Failure | 抽象基类起整文件截断 |
| 方法体 `...` 不识 | 抽象方法炸类 |
| `with lock: return` 在 return 后仍 `__exit__` | LLVM terminator → 链接前崩溃 |

### 修法

1. `parse_del`：逗号分隔多目标（primary/subscript）
2. `parse_class`：吞掉 `(bases)`，V1 忽略 MRO
3. `parse_ellipsis_stmt`：`...` ≡ pass
4. `with`：仅当 `branch_falls_through(body)` 时追加 `__exit__`；`branch_falls_through` 识别嵌套 `Block`

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 238 | **242** |
| `jq_wufu_local` 中 `sources__*` | 仅 `init` + 大量 U | **大量 T**（`_baostock_login` / `_jqdata_init` / `_from_rq_code`…） |
| 链接 | Terminator 崩溃 | **Linking failed**（正常 undef 表，~85，多为第三方/裸方法） |

回归：`t239`–`t242`。

### 下一队列

1. 裸方法：`covers_range` / `load` / `register_universe` / `ParquetCache`
2. `Path`/`timedelta` → stdlib（勿 `module__Path`）
3. facade 再导出边：`market_data___baostock_login` 仍 U（循环导入时序）
4. `--engine local` 避开 backtrader/nautilus

## 批次一百二十六（2026-09-19，**已修**）：try 内 from-import + Path/timedelta 勿错绑

**范围**：NO JoinQuant。清 `register_universe` 裸名与 `pathlib__Path` / `datetime__timedelta`。

### 根因

| 现象 | 根因 |
|---|---|
| `try: from … import X` → 链接 `_X` | `parse_func` 把 try 的尾 `Block` 提到 `ret_expr`，`walk_py_import` 只扫空 `body` |
| `_Path(...)` → `pathlib__Path` | `module_renames_for` 把 registry 再导出编成 `mod__name`，抢在 `py_member_call` 之前 |

### 修法

1. `walk_py_import` / `walk_nonlocal`：扫 `FuncDef.ret_expr`；并递归非 import 的 `Call`/`Return`/…
2. `module_renames_for`：`find_member` 命中的 registry 成员跳过 rename（交给 `py_member_aliases` → `py_path_new` / `py_dt_timedelta`）

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 242 | **244**（t243/t244） |
| 官方 | 194/194 | **194/194** |
| `jq_wufu_local` undef | ~85 | **~82** |
| `_register_universe` | U | **消**（链到 `…universe__register_universe`） |
| `_pathlib__Path` / `_datetime__timedelta` | U | **消**（`py_path_new` / `py_dt_timedelta`） |

回归：`t243_try_from_import` / `t244_path_timedelta_as`（+ `pathas_fixture.py` / `pytryimport/`）。

仍剩（摘）：`_dotenv__load_dotenv` / `_pandas__read_parquet`（registry 无条目）、裸方法 `_execute_trade` / `_initialize` / `_set_cost_config`、第三方 bt/ak/baostock。

### 下一队列

1. registry 补 `dotenv` / `read_parquet` 或 noop
2. 接收者方法：`set_cost_config` / `execute_trade` / `initialize`
3. `--engine local`

## 批次一百二十七（2026-09-19，**已修**）：ann-attr / del-attr 解锁 LocalBackend

**范围**：NO JoinQuant。`wufu_backend.PositionLedger`/`LocalBackend` 曾整段丢光。

### 根因链

| 拦路 | 后果 |
|---|---|
| `self.x: T = v` 只认裸名 ann-assign | `set[str]`/`str\|None` 属性注解炸类 |
| `del self.m[k]` 用 `parse_primary` 只吃 `self` | 剩 `._positions[k]` 炸 `sell` → PositionLedger 起整文件截断 |
| `CostModel::fee` `ret double` vs `define i64` | PositionLedger 可解析后 LLVM verify 整仓 abort |
| `del d[k]` → `map____delitem__` 无分派 | t239 真删除无法链接 |

### 修法

1. `parse_assign`：ann-assign 目标扩到 `Var`/`FieldAccess`/`Subscript`（`parse_unary`）
2. `parse_del`：目标改 `parse_unary`；`map.__delitem__` → 已有 `zeta_map_pop`
3. codegen `Return`：`f64→i64` bitcast（对称已有 `i64→f64`）
4. `map_insert`/`map_get`：识别 tombstone（`used==2`）；重建 `tokio_runtime.o` / `zeta_runtime_c.o`

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 244 | **247**（t246/t247；t239 expect 2→1） |
| `wufu_backend` W1002 | 212 行自 PositionLedger | **0** |
| `LocalBackend::*` / `execute_trade` / `set_cost_config` | 无 / 裸 U | **define** |
| `jq_wufu_local` 链接 | LLVM type abort | **Linking failed**（正常 undef ~92） |
| 消掉 | `_dotenv__load_dotenv` / `_pandas__read_parquet` / 裸 `_execute_trade` | — |

回归：`t246_ann_attr_assign` / `t247_del_attr` / `t239_del_multi` / t243–t245。

### 仍剩（摘）

- 别名：`_jq_shim___LocalPortfolio` / `_initialize`
- 第三方：bt / nautilus / akshare / baostock / pyarrow
- 语言：`_zip` `_isinstance` `_hasattr` `_setattr` `_clear`（map.clear 裸名）
- `--engine local` 避开 bt/nautilus

## 批次一百二十八（2026-09-19，**已修**）：import 类别名 + 裸 map/pandas 方法

**范围**：NO JoinQuant。清 `_jq_shim___LocalPortfolio`；`d.clear`/`d.update`/`s.add` 与 DF 链式方法；勿再导出与类方法同名的 C 裸符号。

### 根因

| 现象 | 根因 |
|---|---|
| `_jq_shim___LocalPortfolio` | `_LocalPortfolio = LocalBackend` 且 RHS 来自 `from … import` —— `own_names` 追不到；需 `walk_name_aliases` 抄 `py_member_aliases` |
| `_initialize`（批 127 仍记） | 已由 `walk_module_member_assigns` 清 |
| `_clear`/`_update`/`_add` | 接收者丢 `map` 标签（或 struct 字段落成 I64）→ 未进 map 分派 |
| `[dynamic]i64__all` 等 | DynamicArray 未知方法拼 `Type::method` 幽灵名 |
| 裸 C `clear`/`to_parquet` | 与 `Ledger::clear` / `DataFrame::to_parquet` 同发 `@clear` → **duplicate symbol** |

### 修法

1. resolver：`walk_name_aliases` — `Alias = ImportedName` → 写入 `py_member_aliases`（+ 当前模块 reexport）
2. MIR opaque：`clear`→`zeta_map_clear`，`update`→`zeta_map_update`，`ffill`/`sort_index`/…→identity；`add` 在 I64/vec/map 句柄上 → `py_set_add`
3. MIR map 块：`("add", 2)` → `py_set_add`；DynamicArray：`all`/`any`/`ffill`/`notna`/… 勿拼 `[dynamic]T__*`
4. `pylib/pandas.z`：`ffill`/`sort_index`/`reindex`；`numpy.z`：`isfinite`/`isinf`；C 仅留 **无同名方法冲突** 的符号（`numpy__isfinite` 等）
5. **禁止** 再加裸名 `clear`/`update`/`ffill`/`to_parquet` 进 `py_additions.c`

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 247 | **252**（t248–t252） |
| `jq_wufu_local` 链接 undef | ~80 | **~65** |
| `_jq_shim___LocalPortfolio` / `_clear` / `_update` / `_add` / `[dynamic]*` / `_ffill` | U | **消** |

回归：`t248_class_alias` / `t249_member_assign` / `t250_import_class_alias` / `t251_map_clear_update` / `t252_ffill_sort_index`。

### 仍剩（摘）

- 第三方/宿主：bt `Cerebro`/`setcash`、nautilus 模型、baostock/ak/tushare、`query_*`、`cache_clear`
- 反射/杂项：`_cls`/`_call`/`_init`/`_date`、`decimal__Decimal`、`___closure`
- `LocalBackend___price_lookup` 等 backend 幽灵成员
- `--engine local` 仍不删 bt/nautilus 函数体（无 DCE）→ 链接仍见其 undef

### 下一队列

1. facade/私有方法：`___price_lookup` / `baostock_login` 再导出时序
2. pyarrow/`read_table`/`write_table` 响亮桩（勿与 DF 方法同名裸 C）
3. 可选：常量 `engine=="local"` 时跳过编 bt/nautilus 模块（大改，另开）

## 批次一百二十九（2026-09-19，**已修**）：advice.md 速赢包 Q1–Q5

**范围**：工程债还债（[advice.md](advice.md) §1），不改语料语义。`ARRAY_SYNTAX_DOCUMENTATION.md` 无待改优化项（仅语法说明）。

### 做了什么

| # | 项 | 改动 |
|---|---|---|
| Q1 | IR dump 改 flag | `main.rs`：仅 `--emit-llvm` 或 `ZETA_DUMP_IR=1` 时 `print_to_stderr` |
| Q5 | PROBE 清理 | 删除全部 `eprintln!("PROBE …")`（codegen / gen / resolver） |
| Q3 | 死代码 | 删 `optimized_mir.rs` / `optimized_gen.rs` / `evaluator_complete.rs` / `module_resolver.rs.backup` / `*.o.tmp` / `tokio_runtime_old.o` |
| Q2 | nm 核对 | 新增 `tools/check_registry_symbols.sh`；`py_asdict_unexpanded` 列入允许缺失 |
| Q4 | 统一基线 | 新增 `tools/run_all.sh` → `/tmp/zeta_baseline.json`；`validate.md` 引用 |

### 验收

- 正常编译 stderr 无 IR 洪水；`ZETA_DUMP_IR=1` 才有 IR
- `rg PROBE src/` 零命中
- `./tools/check_registry_symbols.sh` 退出 0
- `./tools/run_all.sh --skip-official --skip-corpus` 产出 JSON
- 冒烟：t247/t248/t250–t252 绿

### 下一队列（advice 排期）

1. 任务 A1（registry 字段扩展）+ 可选 CI 雏形（L）
2. 修 8 个 duplicate-symbol 回归（见批 130）

## 批次一百三十（2026-09-19，**已修**）：基线绿 + A1 registry 字段

**范围**：[ARCHITECTURE-REVIEW](docs/ARCHITECTURE-REVIEW-2026-09.md) / [advice.md](advice.md)。

### 做了什么

1. **修 8 个 python_style 回归**：删 `numpy__isfinite`/`isinf` C 裸桩（与 `numpy.z` 方法同名 duplicate）
2. **Q4 收口**：`validate.md` §3 推荐 `./tools/run_all.sh`
3. **A1**：`pylib.rs` 解析 `stub=`/`decl=`/`alias-of=`；`asdict` → `stub=1 decl=0`；`all_externs` 跳过；nm 脚本跳过 `stub=1`；单测 2 个

### 度量

| 口径 | 结果 |
|---|---|
| python_style | **252/252** |
| 官方 | **194/194** |
| `check_registry_symbols.sh` | 0 |
| pylib 单测 | 2/2 |

### 下一队列

1. A2：`tools/gen_from_registry.py` → `runtime_decls.rs`
2. `run_all` 全量含语料；CI 雏形（L）
3. 穿插 `jq_wufu_local` undef（~65）

## 批次一百三十一（2026-09-19，**已修**）：A2 registry → 生成声明

**范围**：advice 任务 A2（registry 单一事实源的声明侧）。

### 做了什么

1. `tools/gen_from_registry.py --emit` → `src/backend/codegen/runtime_decls_registry.rs`（273 个 `py_*`）
2. `--check`：nm 核对（跳过 `stub=1`）
3. `codegen::new` 用 `declare_registry_runtime_fns` 替换运行期 `all_externs()` 循环
4. 核心 `map_*`/`zeta_*`/`println_*` 仍手写（A2b：另立 `runtime_core.txt` 再生成）

### 验收

- `cargo build -p zetac` 绿
- `--check` 退出 0
- 冒烟 t212/t227/t250–t252

### 下一队列

1. A2b：核心运行时表数据化（净减 codegen `new()` 手写块）
2. A3：别名 `.set` → `aliases.inc.c`
3. A4：`get_or_declare_function` 表驱动影子模式

## 批次一百三十二（2026-09-19，**已修**）：A3 别名数据化 + runtime 构建脚本

**范围**：advice A3 + M。

### 做了什么

1. `pylib/runtime_aliases.txt`（66 条 LLVM `.N` → 规范符号）
2. `gen_from_registry.py --emit-aliases` → `runtime/aliases.inc.c`
3. `tokio_runtime_stub.c` 手写 `__asm__` 块改为 `#include "aliases.inc.c"`
4. `tools/build_runtime.sh`（`--gen` 顺带重生 decls/aliases）

### 验收

- stub 重编后 `nm` 仍见 `_print.31` / `_array_new.10`
- 官方/python_style 冒烟绿

### 下一队列

1. A4：`get_or_declare_function` 表驱动影子模式
2. A2b：核心 runtime 声明表
3. CI：`run_all.sh` job（L）

## 批次一百三十三（2026-09-19，**已修**）：A4 表驱动查找（影子→切换）

**范围**：advice A4。

### 做了什么

1. `pylib::lookup_declared_symbol`：`decl=1 && !stub && py_*` 符号索引（含 `alias-of`）
2. 影子批次：表 vs 瀑布并行，mismatch=0 后切换
3. `get_or_declare_function`：**表命中优先**，瀑布仅兜底；未登记 `py_*` 仍告警
4. 补登记漏表符号：`py_map_update`/`py_array_concat`/`py_os_path_join_{3,4}`/`py_int_base`/`py_map_fromkeys`；decls → 279
5. 表命中校验 arity（避免 `reduce_3`/`join_3` 被剥后缀落到错误重载）

### 验收

- 官方 194/194；python_style 252/252；corpus 38/38
- t212/t227/t250–t252 绿；A4 未登记兜底告警趋零

### 下一队列

1. A2b：核心 `map_*`/`zeta_*`/`println_*` 声明表
2. A5：jit.rs 映射从注册表生成
3. CI：`run_all.sh` job（L）

## 批次一百三十四（2026-09-19，**已修**）：A2b 核心 runtime 声明表

**范围**：advice A2b。

### 做了什么

1. `pylib/runtime_core.txt`（61：`map_*`/`zeta_*`/`print(ln)_*`/`array_*`，含 `variadic`/`ptr`/`f64`）
2. `gen_from_registry.py --emit-core` → `runtime_decls_core.rs`；`codegen::new` 开头一次 `declare_core_runtime_fns`
3. 删手写核心 `add_function`（new 内 ~221 → ~146）；`build_runtime.sh --gen` 顺带 `--emit-core`
4. 修复：剥离时误删 `declare_registry_runtime_fns` 与 `str_get`/`str_slice`/`py_slice_new` 等 → 已恢复

### 验收

- 官方 194/194；python_style 252/252；corpus 38/38
- 冒烟 t71/t98/t105/t212/t251

### 下一队列

1. A5：jit.rs 映射从注册表生成
2. 清掉 new() 里仍与 registry 重复的手写 `py_*`
3. CI：`run_all.sh` job（L）

## 批次一百三十五（2026-09-19，**已修**）：A5 JIT 映射数据化

**范围**：advice A5。

### 做了什么

1. `pylib/jit_mappings.txt`（131：LLVM 名 → Rust host 路径）
2. `gen_from_registry.py --emit-jit` → `jit_mappings_gen.rs`；`finalize_and_jit` 一次 `register_jit_mappings`
3. 保留 `vec_push_`/`vec_get_`/`vec_len_` 前缀特化循环；`jit.rs` 净减 ~500 行手写 mapping
4. `build_runtime.sh --gen` 顺带 `--emit-jit`

### 验收

- JIT 冒烟：`zetac` 无 `-o` → `3` / exit 0
- 官方 194/194；python_style 252/252；corpus 38/38

### 下一队列

1. 清掉 `codegen::new` 里与 registry 重复的手写 `py_*`
2. CI：`run_all.sh` job（L）
3. A 系列收口后穿插语料功能批次

## 批次一百三十六（2026-09-19，**已修**）：L 基线进 CI

**范围**：advice L（雏形）。

### 做了什么

1. `.github/workflows/ci.yml` 新增 `baselines` job（`needs: test`）
2. 步骤：`gen_from_registry.py --emit*` + `git diff --exit-code`（生成物不过期）→ `cargo build -p zetac --release` → `--check`（nm）→ `tools/run_all.sh`
3. 上传 artifact：`zeta-baseline`（`artifacts/zeta_baseline.json`）

### 验收

- 本地：`gen --emit*` 无 diff；`--check` 退出 0
- CI 在 self-hosted galaxy runner 上跑通（push/PR 后由 Actions 验证）

### 下一队列

1. D：桩响亮化（stub 运行时告警）
2. 清掉 `codegen::new` 重复手写 `py_*`
3. 穿插语料功能 / B1 强转矩阵

## 批次一百三十七（2026-09-19，**已修**）：D 桩响亮化

**范围**：advice D（机制，不批量迁移假值桩）。

### 做了什么

1. `py_stub_abort`（`py_additions.c`）：默认 abort + 符号名；`ZETA_LENIENT_STUBS=1` → 每符号 stderr 告警一次并返回 0
2. `py_asdict_unexpanded` → 走 `py_stub_abort`；registry `stub=1` 且 `decl=1`（声明 abort wrapper）
3. `zetac --list-stubs`；`gen`/`nm` 对 stub 声明一并核对
4. `t253_stub_abort.z` + `run.sh` 的 `// expect-abort:`

### 验收

- `--list-stubs` 输出与 registry stub 数一致（当前 1：`py_asdict_unexpanded`）
- t253：strict 下 abort，stderr 含 `stub not implemented: py_asdict_unexpanded`
- 官方 / python_style / corpus 三基线绿

### 下一队列

1. 假值桩逐步标 `stub=1` 并改走 abort（isin/dropna 等需 LENIENT 或真实现后再迁）
2. 清掉 `codegen::new` 重复手写 `py_*`
3. 穿插语料功能 / B1 强转矩阵

## 批次一百三十八（2026-09-19，**已修**）：B1 强转矩阵

**范围**：advice B1。

### 做了什么

1. `coerce_call_args` 白名单：同宽 / zext / sitofp 放行；narrow / fptosi / float↔ptr → stderr 告警（每模块最多 8 条样例 + 汇总）
2. `--strict-abi` / `ZETA_STRICT_ABI=1`：矩阵外强制失败（`abi_fatal`）
3. `t254_abi_coerce_warn.z`：默认告警仍通过；`--strict-abi` 退出非 0

### 验收

- 默认编译不破坏基线；t254 输出 `1`
- `--strict-abi` 对 t254 报 `strict-abi: forbidden coerce … fptosi`
- 官方 / python_style / corpus 三基线绿

### 下一队列

1. B2：libc 碰撞检查（isalnum/strftime/abs…）
2. B3：`Type::PyDynamic` + `--report-untyped`
3. 假值桩逐步标 stub / 语料功能穿插

## 批次一百三十九（2026-09-19，**已修**）：A 收尾 + B2 libc 碰撞

**范围**：advice 排期 139（A 清 `codegen::new` 残余 `py_*` + B2）。

### 做了什么

1. A 收尾：`py_fmt_*` / `py_round_*` / `py_slice_new` / `py_list_eq` / `py_getitem2` / `py_logger_info_n` 迁入 `registry.txt`；删 `new()` 内与 registry 重复的 queue/threading/`py_file_open` 等手写声明（手写 `add_function` 150→137，`py_*` 残余 0）
2. B2：waterfall 发明 bare extern 时命中 libc 名表 → stderr 告警
3. `t255_libc_collision.z`：`isalnum` 触发告警且仍可运行

### 验收

- t255 编译 stderr 含 `collides with libc`；运行输出 `1`
- t27 等依赖 `py_fmt_*`/`py_round_*` 的用例仍绿
- 官方 / python_style / corpus 三基线绿

### 下一队列

1. B3：`Type::PyDynamic` + `--report-untyped`
2. C1/C2：解析行号 + 顶层同步恢复
3. soft 桩逐步真实现或 LENIENT 后响亮化

## 批次一百四十（2026-09-19，**已修**）：D4 库层假值桩

**范围**：advice D4。

### 做了什么

1. 约定 `# stub: <name>` / `# stub: soft:<name>` 标记 `pylib/numpy.z`、`pylib/pandas.z`
2. `pylib_file_stubs` + `all_stub_symbols`；`zetac --list-stubs` / `gen --list` 合并 registry + pylib（20 = 1+19）
3. 响亮迁移（基线未用）：`numpy.{vstack,log,isnan,isfinite,isinf}`、`pandas.{date_range,to_datetime,to_numeric}` → `py_stub_abort`
4. soft 保留假值：`dropna/fillna/astype/isin/…`（基线依赖 typed copy）
5. `t256_pylib_stub_abort.z`

### 验收

- `--list-stubs` 与 `gen --list` 一致（20）
- t256：abort + `pandas.date_range`
- 官方 / python_style / corpus 三基线绿

### 下一队列

1. B3：`Type::PyDynamic` + `--report-untyped`（与 E4 合批）
2. C1/C2：解析行号 + 顶层同步恢复
3. soft 桩逐步真实现或 LENIENT 后响亮化

## 批次一百四十一（2026-09-19，**已修**）：B3 `Type::PyDynamic` + `--report-untyped`

**范围**：advice B3（E4 签名 `Type` 结构化未合入，仍 `from_string`）。

### 做了什么

1. `Type::PyDynamic`（`"dyn"`）：未标注形参默认 `dyn` 而非 `i64`；ABI 仍映射 i64
2. `zetac --report-untyped`：列出 `(func, param)` 中仍为 `PyDynamic` 的形参
3. 调用点推断 / 类字段细化：把 `dyn` 与旧 `i64` 默认同等视为可升级（否则 t53/t160/t147/t167/t181/t182 回归）
4. `t257_report_untyped.z`：清单含 `add.a`/`add.b`，运行输出 `3`

### 验收

- `--report-untyped` 对 t257 打印 `untyped params (2): add.a / add.b`
- python_style **257/257**；官方 **194/194**；语料 **38/38** parse

### 下一队列

1. C1/C2：解析行号映射 + 顶层同步恢复
2. B4：dyn 接收者走 W 方法表分发
3. soft 桩逐步真实现；E4 可与后续类型清理合批

## 批次一百四十二（2026-09-19，**已修**）：C1 行号映射 + C2 同步恢复（opt-in）

**范围**：advice C1 + C2。

### 做了什么

1. C1：`indent_preprocess` 写 TLS 行源映射；`ensure_fully_parsed` 的 W1002 升级为 `path:line:`（`t258`）
2. C2：`parse_zeta_impl_recover` + `skip_to_top_level_sync`（W1003）；**默认关**，`ZETA_PARSE_RECOVER=1` 开启（默认开曾让官方 184/194、语料 34/38）
3. `t259_parse_sync_recover.z`：`// env: ZETA_PARSE_RECOVER=1`，中间 `!!!` 后仍跑到 `second`

### 验收

- t258：stderr `W1002 …/t258_parse_line_map.z:11:`
- t259 + RECOVER：输出 `1`/`2` + W1003
- 官方 **194/194**；python_style **259/259**；语料 **38/38**

### 下一队列

1. B4：dyn 接收者走 W 方法表分发；语料 undef 攻坚
2. soft 桩真实现 / LENIENT 后响亮化
3. 观察 RECOVER 误恢复后再考虑翻默认

## 批次一百四十三（2026-09-19，**已修**）：B4 + 误回退恢复

**范围**：advice B4；事故恢复（`git checkout` 丢掉未提交 gen.rs）。

### 做了什么

1. **恢复**（按 roadmap 规格重写，非整文件找回）：
   - `While.pre_cond`：mir + gen（条件副作用）+ codegen（`while.cond`/`continue` 先跑）
   - for-in-str：`coll_is_str` → `str_len`/`str_get`（t205）
   - `df.drop(str)` 调用点包一元 `StackArray`（批次 111）
2. **B4**：`method_by_unique_name` 仅 `PyDynamic` + SKIP 容器/dunder 名
3. **附带**：`handle_tag` 改为查 W 表全部 handle（原先只有 PyDate/PyDelta，导致升级后的 `PyPath` 形参仍 I64、`p.exists()` 裸链）
4. 摘除 t205/t260 `known-fail`

### 验收

- t192 / t205 / t260 PASS
- 官方 / python_style 需全量再跑（t206 `columns`/`in DataFrame` 另案，非本次回退主因）

### 教训

- **每个 batch 必须 commit**；禁止对未提交大文件 `git checkout --` / `rm`（改用 `mv` 进 `.trash/`）

### 下一队列

1. 全量三基线确认
2. t206 DataFrame.columns / `in` 容器类型
3. soft 桩 / 语料 undef

---

## 进度快照（2026-09-19 17:16，批次一百四十三收口）

### 位置

| 项 | 状态 |
|---|---|
| advice P0 | **129–143 全线 ✅**（注册表 A、ABI B1–B4、D/D4 桩、C1+C2、B3 dyn） |
| 最新批次 | **143 已修**：误 `git checkout` 后按规格恢复 `While.pre_cond` / for-in-str / B4；`handle_tag` 补全 W 表 handle |
| HEAD | `3b79889f` — `feat(py-a): batches 129–143 — registry, dyn/ABI, parse recover, B4 restore` |
| 分支 | `bootstrap` 领先 `origin/bootstrap` **387**；**该 commit 未 push** |
| 流程违规 | 129–143 曾长期不 commit，后捆成 `3b79889f`；**AGENTS 要求每 batch 立刻 commit+push —— 该提交尚未 push** |

### 焦点验证（刚测）

| 用例 | 结果 |
|---|---|
| t192 `while` + `pre_cond` | ✅ |
| t205 for-in-str | ✅ |
| t260 B4 / Path.exists | ✅ |
| t206 `drop`/`columns`/`in DataFrame` | ❌ 仍全 0（另案） |

### 基线数字

| 套件 | 数字 | 备注 |
|---|---|---|
| 官方 | **待重跑** | 事故前口径 194/194 |
| python_style | **待重跑** | 焦点 t192/t205/t260 绿；全量未确认 |
| 语料 parse | **38/38**（事故前） | 未解析行 0 |
| 语料 undef | **~65** | 目标 → &lt;40 |

### 下一刀 = 批次 144

1. `./tools/run_all.sh` 写出真实三基线数字  
2. 修 t206（`DataFrame.columns` / `in` 容器类型丢成 I64）  
3. soft 桩消化 + 语料 undef  
4. **每完成一项：commit + push**（勿再捆批）


## 批次一百四十四（2026-09-19，**部分收口**）：事故余波清点 + t206 修复链

**范围**：批次 144 快照清单第①②项；③ soft 桩/undef 留下一批。

### 做了什么

1. **全量三基线重跑**（143 事故后首次）：官方 194/194、语料 38/38、python_style **220/260**（40 失败 = 恢复规格不完整的欠账，非新增回归）
2. **t206 修复链**（3 处编译器 + 1 处库注解）：
   - gen.rs FieldAccess：已知 struct 上 `df.columns`（无括号）命中零参方法时发射限定名 `DataFrame::columns`（此前退化为裸字段读 → 对 map 句柄做 array_len → 恒 0）
   - gen.rs `in`：`x in obj` 对定义了 `__contains__` 的 Named 接收者发射限定调用；接收者类型未跟踪时按"唯一限定定义"回退（对齐 Call 路径的既有做法）
   - gen.rs self 类型化：MirGen 增加 current_class，方法 lowering 时 `self` 绑定为 Named(类名)（`__contains__` 的 self 此前是 PyDynamic，`self.data` 丢 map 类型）
   - codegen DictGet/DictInsert：map_id 从 load_local 改为 gen_expr_safe（map 可以是 `self.data` 这类表达式；此前读从未写入的槽位 → 垃圾 → 0）
   - pandas.z：`__contains__/__delitem__/column/__getitem__` 的 key 注解 `str`（跨方法边界后键哈希由键的静态类型决定——库文件头部注释记载的同族问题）
3. **--dump-mir 接线**：死标志激活（编译路径输出各函数 MIR Debug，本次排查即靠它）

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 220/260 | **223/260**（t206 + 2 连带） |
| t206_df_drop_str | 全 0 | **1 0 1 1 0 1 1 0 全绿** |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

### 事故余波分类（37 失败，下一批队列）

- **链接失败 17**：恢复时丢失的修复需按原批次重放 —— `_unique` 裸名（t211）、`_@`（matmul 操作符未映射，t214）、`_map____delitem__`（批次 127 del→zeta_map_pop 丢失，t239/t246）、getattr 系（t215/t219/t222）、numpy 系（t212/t217/t221/t227/t230）
- **输出错 20**：pandas/dunder 家族（t193–t210/t227–t229/t87）——疑似限定名/dunder 分派同簇
- p7（drop 后 `b["code"][0]`）SIGBUS 另案

### 下一队列

1. 链接失败 17 例：按原批次重放（`_@`、`_map____delitem__`、`_unique` 优先——每个都是一处发射点修复）
2. 输出错 20 例：按 dunder 分派聚类处理
3. soft 桩消化 + 语料 undef <40

## 批次一百四十五（2026-09-19，**进行中**）：事故余波重放 第一轮

**范围**：143 事故丢失修复的按批重放（37 失败 → 31）。

### 重放的修复（4 组发射点 + 1 组库注解）

1. **Subscript→`__getitem__` 分派**（批次 99 重放）：Named struct 上的下标 `df["c"]` 此前直接 DictGet（对 struct 指针 map_get → SEGV/0）；现命中 `Class::__getitem__` 限定调用（t194 簇前置）
2. **vec 方法表**：`xs.sum()`/`xs.unique()`（PyDynamic/I64/DynamicArray/Array/None 接收者）→ `zeta_sum_vec`/`zeta_vec_unique`（保持元素类型；此前落 opaque str_fallback 发射裸名 `_sum`/`_unique`）
3. **map `__delitem__`**（批次 127 重放）：`del d[k]` → `zeta_map_pop` + 内容哈希键（t239）
4. **matmul `@`**：与 `*` 同走 SemiringFold Mul（标量 matmul 即乘法；此前 `Call{func:"@"}` 链接失败，t214）
5. **内建 sum**：PyDynamic/未跟踪实参 → `zeta_sum_vec`（此前走静态 `zeta_sum_n` 打 0，t224）

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 223/260 | **229/260** |
| t203/t211/t214/t224/t239 | FAIL | **PASS** |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

### 剩余 31 失败的分类

- **链接失败 12**：getattr 系（t215/t219/t222/t234，批次 123 重放）、listcomp（t216/t233）、strftime（t220）、logger（t223）、dict set cast（t231）、path open（t232）、ann-attr（t246）、numpy clip（t230）
- **输出错 19**：len/setitem dunder（t196/t197）、pandas 链式（t198/t200/t201/t202/t204/t207/t208/t210/t228/t229）、numpy where/eye（t212/t217/t218/t221/t227）、isinstance（t87）
- 工具注记：`--dump-mir` 已接线（144），本轮排查全靠它

### 下一队列

1. getattr 系 4 例（同一 lowering 点的可能性大）
2. dunder len/setitem + pandas 链式簇
3. numpy where/eye 簇

### 批次一百四十五 第二轮（同日）：getattr/next 族重放

1. **builtin `getattr(obj, "name" [, default])`**：已知 struct（或从构造调用恢复类名）且字段存在 → 改写 FieldAccess（复用零参方法分派/字段类型）；字段不存在或未跟踪接收者 → default（一次告警）；无 default/动态名 → `py_getattr_dynamic` 响亮 abort（批次 123 红线）
2. **builtin `next(it[, default])`**：带 default → default（一次告警）；无 default → `py_builtin_next` abort
3. **opaque `.next()`**：`py_method_next` 返回 0（exhausted，一次告警；已知 struct 的 next 方法走限定调用）
4. 修复重放引入的 MIRgen 悬挂 Var 模式：默认值/FieldAccess 改写直接 `return self.lower_expr(...)`，不再 `Var(did)` 指向未存槽 id
5. 移除临时 ZETA_PROBE 探针

### 度量（第二轮后）

| 口径 | before | after |
|---|---|---|
| python_style | 229/260 | **232/260** |
| t215/t219/t222/t234 | FAIL | **PASS** |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

剩余 28 失败：dunder len/setitem（t196/t197）、pandas 链式（t198/t200/t201/t202/t204/t207/t208/t210/t228/t229）、numpy（t212/t217/t218/t221/t227/t230）、isinstance（t87）、listcomp（t216/t233）、strftime（t220）、logger（t223）、dict set cast（t231）、path open（t232）、ann-attr（t246）。

### 批次一百四十六（2026-09-19，**部分收口**）：dunder len/setitem + getattr 幽灵红线修复

1. **`len(obj)` → `__len__` 分派**：Named 接收者且类定义 `__len__` 时发射限定调用（t196：`len(F())`=3、`len(df)`=行数、空 DF=0）
2. **下标赋值 → `__setitem__` 分派**（批次 102 重放）：`f["a"] = 1` / `df["col"] = [...]` 命中 `Class::__setitem__`（VoidCall）；此前落 DictInsert 写 struct 指针（垃圾写）
3. **map 赋值键哈希**：Assign 路径改用 `lower_map_key_typed`（按 map 声明的键类型，与表达式路径一致；参数键此前按指针哈希永不匹配）
4. **N 参 min/max**：`max(1, 5, 3)` 两两折叠（f64 走 llvm.minnum/maxnum 内联）；此前仅 2 参，3 参落裸名 `_min`/`_max`（t230）
5. **getattr 幽灵红线回归修复**：145 引入的 2 参无 default 路径误发 `py_getattr_dynamic`（编译成功）破坏 t225；改回字面量未命中即落出本块走幽灵链接失败

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 232/260 | **237/260**（t196/t197/t198/t230/t225 + 连带） |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

### 剩余 23 失败

- 链接失败 6：t216/t233（listcomp）、t220（strftime）、t223（logger `_3` 变体）、t231（fromkeys/dict/add）、t232（Path open/read_text）、t246（set）
- 输出错 17：t193/t200/t201/t202/t204/t207/t208/t210/t228/t229（pandas 链式）、t212/t217/t218/t221/t227（numpy where/eye/free_names）、t87（isinstance）

### 下一队列

1. pandas 链式簇（head/tail/rename/reset_index 的 self 方法链 + 切片）——最大簇 10 例
2. numpy where/eye/free_names 簇
3. 链接失败 6 例逐个（多为缺 registry X 条目或 W 方法）

### 批次一百四十六 第二轮（同日）：vec identity + N 参 min/max

1. **vec/lList FieldAccess identity**：`xs.values` / `.tolist` 在 DynamicArray 与定长 Array 上 = 句柄本身（vec 无字段；此前裸字段读 → 垃圾 → 后续下标 SEGV，t202）
2. **N 参 min/max**：`max(1, 5, 3)` 两两折叠 f64/i64（t230）

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 237/260 | **238/260** |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

### 新定位的缺陷（未修，下一批）

- **切片 end 为 PyDynamic 参数时结果错误**：`def sl(col, n): return col[:n]` 对 `["x","y"]` 返回空（`zeta_slice_vec(handle, 0, 1)` 本应正确；疑 PyDynamic end 的 lowering 或静态数组 header 探测）——head/tail/rename 簇（10 例）的公共根因
- 其余：numpy where/eye 簇、isinstance、listcomp DataFrame、strftime/logger/path 链接失败

### 批次一百四十六 第三轮（同日）：current_class 规范化 + 收口

1. **current_class 剥模块 mangle 前缀**（pandas__DataFrame → DataFrame）：type_decls/func_ret_types 以裸类名为键
2. 定位到的深层债务：**方法限定名不统一**——同一 pylib 类的方法同时存在 `DataFrame::columns` 与 `pandas__DataFrame::column_names` 两种注册形态（"5 处改名点"债务的具体形态），导致 self 类型化只对部分方法生效；head/tail/rename 簇（t200/t204/t207/t208/t210/t228/t229/t201）的 SEGV 仍源于此
3. 本轮已收复：t230（N 参 min/max）、t225（getattr 幽灵红线回归修复）、t202（vec FieldAccess identity）

### 度量（批次一百四十六 收口）

| 口径 | 143 事故后 | 现在 |
|---|---|---|
| python_style | 220/260 | **238/260**（+18） |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

### 下一队列（批次一百四十七）

1. **方法限定名统一**（P1 级还债）：load_user_python_module 的方法注册收敛为单一形态（DataFrame::method），消除 `pandas__DataFrame::x` / `DataFrame::x` 双态——head/tail/rename 簇 10 例的根因
2. numpy where/eye/free_names 簇（t212/t217/t218/t221/t227/t230 ✓已收）
3. 链接失败 6 例：t216/t233（listcomp 闭包内 DataFrame）、t220（strftime W 表）、t223（logger `_3` 变体）、t231（fromkeys/dict/add）、t232（Path open）、t246（set）
4. isinstance（t87）

### 批次一百四十七（2026-09-19，**部分收口**）：限定名统一尝试 + 战术回退

1. **尝试**：rename_definition 补 ImplBlock 重命名（ty + 方法 self/ret 同步）+ defs 分发放行 ImplBlock —— 方法注册统一为 `pandas__DataFrame::method`。**结果：级联失败**（t202 由过转崩）——根因是 pylib 类的字段是**隐式**的（来自 `__init__` 赋值，StructDef 无字段声明），重命名后 `self.data` 的字段类型查找落空，且构造体内 StructLit variant 未随改。**已回退**（git checkout resolver.rs），gen.rs/codegen.rs 的手术修复保留
2. **t225 回归修复**：145 的 2 参 getattr 误发 py_getattr_dynamic（编译成功）破坏幽灵红线；改回字面量未命中即落出本块（链接失败 = 预期）
3. **结构性结论**：head/tail/rename 簇（8 例 SEGV/错值）的正确修法是**解析器级隐式字段合成**——parse_class 从 `__init__` 的 `self.x = ...` 赋值合成 StructDef 字段声明（带类型推断），使 self 类型化、字段读取、map 键哈希全链可用；这是批次 148 的主任务

### 度量（批次一百四十七 收口）

| 口径 | before | after |
|---|---|---|
| python_style | 238/260 | **238/260**（t225 修复对冲了重命名扰动） |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

### 下一队列（批次一百四十八）

1. **隐式字段合成**（parse_class：`__init__` 的 `self.x = <expr>` → StructDef 字段 `x: <推断类型>`）——解锁 head/tail/rename 簇 8 例 + t198/t210/t228
2. numpy where/eye 簇（t212/t217/t218/t221/t227）
3. 链接失败 6 例（t216/t220/t223/t231/t232/t246）
4. isinstance（t87）、listcomp 闭包（t216/t233）

### 批次一百四十七（2026-09-19，**已收口**）：限定名候选查找——双态统一的外科实现

**方案变更**：直接重命名 ImplBlock 会级联（ctor StructLit variant、字段类型查找、py_struct_type_of 等消费者都需要同步），已回退。改为**读取侧容忍双态**：新增 `qualified_method_candidate(tn, method)`——先试 `{tn}::{method}`，未命中则剥模块 mangle 前缀（`pandas__DataFrame` → `DataFrame`）重试。接入全部 5 个分派点（Call 限定路径 / len→__len__ / 下标赋值→__setitem__ / 下标→__getitem__ / in→__contains__）。

### 根因（head/tail 簇 SEGV 的完整链条）

`self` 在库方法内被类型化为 `Named("pandas__DataFrame")`（mangled struct），而 func_ret_types 以 `DataFrame::column_names`（未 mangled）为键 → 方法返回类型查找 MISS → `cols` 退化 I64 → `cols[j]` 落入 map 猜测 → map_get(vec 句柄) SEGV。

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 238/260 | **241/260**（t200/t201/t204 部分路径 + 连带） |
| t196/t197/t198/t200/t201/t202/t206/t87 | FAIL/部分 | **PASS** |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

### 剩余 19 失败

- numpy 簇 5：t212/t217/t218/t221/t227（where/eye/free_names/wrappers）
- pandas 链式 6：t193/t204/t207/t208/t210/t228/t229（rename/reset_index/dedup/append）
- 链接失败 6：t216/t233（listcomp 闭包内 DataFrame）、t220（strftime）、t223（logger `_3`）、t231（fromkeys/dict/add）、t232（Path open/read_text）、t246（set）
- t87（isinstance：直接运行输出正确，runner 口径待查）

### 批次一百四十七 追加（同日）：isinstance PyDynamic 修复

- **isinstance(x, int)**：B3（批次 141）把未标注形参类型改为 PyDynamic 后，静态类型判定对 PyDynamic 落 0（t87 `f(5)`）。按 i64 ABI 现实把 PyDynamic 纳入 int 判定

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 241/260 | **242/260**（t87） |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

### 批次一百四十八（2026-09-19，**部分收口**）：np.where 重放 + isinstance PyDynamic

1. **np.where MIR 拦截重建**（批次 122 重放）：1 参 → `zeta_np_where1`（索引 Vec）；3 参 → `zeta_np_where3`（标量 select / 元素级）；静态类型按 cond 形状（Vec/I64）——此前 registry `ret=i64` 使 `np.where(mask)[0]` 落 map 猜测 SEGV（t217/t218）
2. **幽灵边界**：仅自由调用 `np.where` 拦截（receiver 为模块别名）；DataFrame 方法 `.where(a)` 保持幽灵链接失败（t213 期望编译报错）
3. **isinstance PyDynamic**（批次 141 B3 回归修复）：未标注形参按 i64 ABI 判 int（t87）

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 238/260 | **243/260**（t217/t218/t87/t230/t225 + 连带） |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |

### 剩余 17 失败

- pandas 链式 8：t193/t204/t207/t208/t210/t228/t229/t201（隐式字段合成为前置，批次 148 主任务）
- 链接失败 6：t216/t233（listcomp 闭包内 DataFrame 构造）、t220（strftime W 表缺口）、t223（logger `_3` 变体 shim）、t231（fromkeys/dict/add）、t232（Path open/read_text shim）、t246（set）
- numpy 2：t212（free names 环境读类型）、t221（eye 二维行语义）

### 下一队列（批次一百四十九）

1. **隐式字段合成**（parse_class：`__init__` 的 `self.x = <expr>` → StructDef 字段，类型从构造参数/字面量推断）——pandas 链式 8 例的前置
2. numpy free-names：`from numpy import arange` 等别名调用返回类型的 env 往返
3. 链接失败 6 例逐个（多为缺 registry X 条目或 W 方法）

---

## 批次一百四十九（2026-09-19，**已收口**）：注解/调用点形状 vs 注册表 —— 13 个裸符号/静默错值

本批不再奔「隐式字段合成」那条大改（批次 147 判定它是 head/tail 簇的前置），而是先按
**「注册表里已经有、只是降级层没接」**逐项清缺口 —— 证据导向、每项都可 pre/post 对拉。

### 修掉的（每条都有 pre-fix FAIL / post-fix PASS 的独立证据）

| # | 症状 | 根因 | 修法 |
|---|---|---|---|
| 1 | `k in m`（`m: lt(map,K,V)` 形参）**恒 0**；类方法里**段错误** | `in` 的 map 分支只认 `type_map` 的 `Named("map")`，而 `lt(...)` 注解**从不进 type_map**（形参落表处没有 `lt` 分支，一律留 I64），类型只在 `source_types` 里 ⇒ 自由函数落「不支持」返回 0；类方法落「唯一 `X::__contains__` 兜底」，把 map 实参当 `self` 发 `DataFrame::__contains__(map,k)` | `is_map` 加 `source_types` 的 `map<` 回退（沿用 6393 行既有的 `is_array_param` 范式）；给唯一 `__contains__` 兜底加 `self` 槽位闸门 |
| 2 | 列表推导体里 `pd.DataFrame(r)` → 裸 `_DataFrame` | `lower_closure` 建子 MirGen 只拷 nonlocal/module_globals/type_decls/func_ret_types/symbol_renames，**不拷 import 表** | 子 MirGen 补齐 py_imports / py_user_modules / module_global_types / func_param_names / argparse_kinds / param_defaults / source_file / global_consts / shared_type_decls / closure_vars / closure_ret_tys / hoisted_names / current_class / re_repl_param |
| 3 | `set()` / `s.add(v)` / `dict.fromkeys(k)`（1 参）三类裸符号 | `py_builtin_set` / `py_set_add` / `py_map_fromkeys` 早已在 runtime+注册表里；`set` 只接 1 参、`add` 无人发射、`fromkeys` 只接 2 参 | `set()` 0 参走 `py_builtin_set(0)`（`zt_vec_len(0)==0` null 安全）；`s.add` → `py_set_add` + 与 `push` 同款回写；`fromkeys` 1 参补缺省值 0 |
| 4 | `-> Path` / `def f(p: Path)`（**Python 类名**）→ 裸 `_Path__open` | `handle_tag()` 只认**标签**（PyPath），不认 Python 类名；`F <mod> <Class> … handle=<Tag>` 就是这层映射却没被用 | `handle_tag` 增加「按模块成员名查 handle=」；resolver 返回类型归一化处套用 |
| 5 | `p.open()` → `py_file_open_1`（未定义）；`q.open(encoding=…)` 同理 | 注册表声明 2 参（receiver+mode），1 参调用被 arity 后缀 | **只对 `PyPath.open` 定点补**：缺 mode / 只有 kwarg → 补 0（`py_file_open` 本就把 null mode 当 "r"）。**不做通用补参**——运行时有**故意的 `_N` 变体**（`py_threading_thread_new_2`），第一版通用规则把 t62 从 42 打成 1，已回退 |
| 6 | `os.makedirs(p, exist_ok=True)` / `p.read_text(encoding=…)` | 同上（kwarg 进不去固定 arity 的 C 符号） | 按运行时既有 `_N` 约定补真实现：`py_os_makedirs_2`、`py_path_read_text_2`（忽略那个语义上无意义的可选参），重建 `tokio_runtime.o` |
| 7 | `log.error(fmt,x)` / `log.warning(fmt,x)` → `_3` 裸符号 | `py_logger_error_n`/`warning_n` 早已在 runtime+注册表+codegen 声明里，只有 MIR 分派写死 `info` | 分派扩到 `info|warning|error`，符号名按级别拼 |
| 8 | `datetime.datetime(…).strftime(fmt)`（**无 import**）→ `_PyDate__strftime` | 无别名时构造调用解析得晚（`find_module` 回退在调用发射点，不在 AST 形状处）⇒ `py_handle_of` 对 Call 形状接收者返 None ⇒ 落「Named 接收者方法」分支，那里无条件 `tn::method` | Named 接收者分支**之前**查一次 W 表：`tn` 是句柄标签时直接发 shim。这也解释了为什么 `.year`（字段路径）一直通、`.strftime(…)`（Call）不通 |
| 9 | `np.arange(4)[2]` 段错误 | `py_member_call` 对磁盘模块只能报粗粒度返回种类（"str"/"f64"/"i64"），`-> lt(vec,i64)` 被压成 I64 ⇒ `[2]` 走 map 猜测 | 调用点优先取 `func_ret_types[symbol]` 的声明类型 |
| 10 | `from numpy import where` 的 3 参形态打印指针 | 批次 148 的 np.where 拦截只认「receiver 是模块别名」；裸名落到 pylib 的 1 参包装，3 参被静默截断 | 裸名经 `py_member_target(None,"where")` 解析为 numpy.where 时同样拦截 |
| 11 | `map<K,V>` 的 **V 完全没进类型图** | 字典字面量只记键类型 `Named("map",[K])` ⇒ `a = m["code"]` 是 I64 ⇒ `a[0]` 走 map_get(Vec 句柄) 段错误 | 字面量按第一个值的静态类型补第二类型参数；下标表达式从 `targs[1]` 取结果类型 |
| 12 | `np.zeros((r,c))` **挂死** | `pylib/numpy.z` 注释写「元组形由 MIR → zeta_np_zeros2」，但 MIR 里**没有这段拦截**（`grep zeta_np_zeros src/` 为空）⇒ 元组句柄当长度，分配天文数字平坦数组 | 补拦截：元组 2 元素 → `zeta_np_zeros2(r,c)`，结果 DynamicArray(I64) |

### 度量

| 口径 | 本批起点 | 现在 |
|---|---|---|
| python_style | 243/260 | **255/262**（+12，含新增 t261/t262） |
| 官方 / 语料 | 194/194 · 38/38 | 持平（每步都复跑） |

commit：`7e9aad9e` `af7e2f77` `e8565b73` `595e9607` `0ff12fda` `6d90181a`（均已 push agentic）。

### 剩余 7 例的**精确定位**（下一批直接从这里开）

- **pandas 5 例（t204/t207/t208/t228/t229）—— 同一簇，根因已缩小到两处**：
  1. **库方法内 `self.copy()` / `len(self.data)` 与外部调用结果不一致**（可复现、可二分）：
     - `a.n_columns()`（内部 `len(self.data)`）→ **2** ✓
     - `len(a.data)`（外部对同一字段）→ **0** ✗
     - `b = a.fillna(0); b.n_columns()` → **2** ✓ 而 `len(b.columns)` → **0** ✗
     ⇒ **字段读的静态类型在「方法内」与「方法外」两条路径上不一致**，即批次 146/147
     记的那条债（`pandas__DataFrame::x` / `DataFrame::x` 双态 + 隐式字段），
      但它**不是** batch 147 试过的做法能修的（那次是重命名 ImplBlock，级联失败已回退）。
     下一步应从「字段类型查找」单点埋探针（`py_struct_type_of` / 8392 行附近），
      而不是再动命名。
  2. **`[]` 字面量是 0 长 StackArray，不是可增长的 DynamicArray**（t207 的直接根因）：
     `xs = []` → `Array(I64, Literal(0))` + `StackArray` ⇒ `xs.append("hi")` 的
     `vec_push` 写进栈内存，`len(xs)` 恒 0。**且注解被丢弃**：`zs: lt(vec,str) = []`
     的 `lt(vec,str)` 在 `parse_assign`（stmt.rs:371）里解析完就扔 ⇒ `zs` 仍是
     `Array(I64,0)`。Python 列表语义要求字面量走 `DynamicArray`——这与 Rust 风格的
     `[T; N]` 语义冲突，需要一次「Python 语料专用」的判定（如 `parse_array_lit` 标记
     来源），是**独立的设计决定**，不要顺手改。
- **t231**：`dict(x)` 对**未定型形参**是**故意**的响亮失败（gen.rs 5428 注释：
  「copying an unknown handle would corrupt data」）。要让它通过需要「调用点推断
  覆盖无标注形参」——批次 py_asdict 那条线上已有同类机制（AST 回写 + Named 分支），
  应复用而不是在这里放宽。
- **t233**：闭包内经 env 取到的**函数值**做调用，需要**间接调用**（MIR 现无
  `CallIndirect`；codegen 无 `inttoptr` + `build_indirect_call`）。闭包值本身已是
  `MirExpr::FuncAddr`（i64 函数地址），所以这是一条**新增能力**，不是修 bug。

### 批次一百四十九 **收口**（2026-09-19）

> 上文「剩余 7 例的精确定位」写成后本批又连收 4 例，以本节为准。

**最终度量**：python_style **260/263**（起点 243/260）· 官方 **194/194** · 语料 **38/38**。
11 个 commit 全部 push（`agentic`）：`7e9aad9e` → `e5890fd0`。

**本批翻绿的 13 例**：t216 · t223 · t232 · t212 · t218 · t227 · t221 · t193 · t208 ·
t228 · t229 · t204 · t246；新增回归锁 t261（map 形参成员判定）· t262（set/fromkeys）·
t263（空表可增长）。

**最值钱的三条**（都是「一层元数据错 → 整簇症状」）：

| 根因 | 症状面 | 修法落点 |
|---|---|---|
| 调用点 `rsplit_once('_')` 把 `::`-限定名的**名字里的 `_`** 当 arity 后缀切掉 ⇒ 返回类型查不到 ⇒ I64 | `reset_index`/`sort_index`/`pct_change` 三个方法的结果全变 I64，`b.columns` 变成结构体字段读（t204/t208/t228/t229） | `suffixed` 布尔判据（批次 99 在 codegen 修过、gen.rs 漏了） |
| 方法调用的**默认参数**从不注入 ⇒ codegen 补 0 | `a.reset_index()` 的 `drop` = 0（=False），走错分支（t229） | 方法调用且该名字声明过默认值时，按 `func_param_names`（skip self）建槽位填 `param_defaults[i+1]` |
| 形参**没有 `lt(...)` 分支** ⇒ map/vec 形参一律 I64 | `out[columns[key]] = col` 里 `columns[key]` 无值类型 ⇒ **裸字符串指针当键插入**，查询走内容哈希全 miss → `b["y"]` SEGV（t204） | 新增 `lt_annotation_type()`，形参落表处解析成规范 MIR 类型 |

**剩余 3 例（带本批实测结论，下一批直接从这里开）**

1. **t207（列表字面量语义）** —— 本批做了两次受控实验，结论明确：
   - 已修并保留：`[]` → `DynamicArray`（pre-fix 实测 `0 0 7 0 9`，post-fix `0 1 7 2 9`）。
     修之前 `[]` 是 0 长 StackArray，`vec_push` 读 alloca 前 16 字节当 cap/len 并写越界
     （pylib 里 `idx: lt(vec,str) = []` + `idx.append(...)` 的 UB 来源）。
   - **非空字面量改 DynamicArray：已实测并回退**（`ZETA_PY_LIST_LITERAL` 实验开关的
     代码已删除，仅留在本节记录）。改成功后 t207 的 `len(ys)` 正确（1 → 2），但**代价**：
     `[1.5, 2.5]` 元素打印成非规格化数（`vec_push` 是 i64 通道，f64 元素按位重解释 ——
     t56/t139 红）、`f(*args)` 解不开（starred 展开靠 `Array(_, Literal(n))` 拿长度 ——
     t33 红）、t24 红。⇒ 要落地必须**同时**做两件事：① pushed/loaded 时按元素类型
     bitcast f64↔i64；② starred 展开改从字面量 AST 取长度而不是从类型取。
     另附：`array_push` 在 AOT 里是**空桩**（`runtime/tokio_runtime_stub.c:241`
     `(void)arr;(void)val;`）—— `DynamicArrayLit` 的既有降级也在用它，元素被静默丢弃，
     属**独立的既存缺陷**。
   - `zs: lt(vec, str) = []` 的注解仍被丢：`parse_assign`（`stmt.rs:371`）解析出 `_ty`
     后**直接丢弃**。要么让 `Assign` 带注解，要么按「先查类型再降级 RHS」的顺序。
2. **t231（`dict(x)` 未定型形参）** —— gen.rs 的注释写明是**故意**的响亮失败
   （「copying an unknown handle would corrupt data」）。要过就需要「调用点推断覆盖
   无标注形参」（py_asdict 那条线上已有同类机制的雏形），不是在这里放宽。
3. **t233（经 env 取到的函数值做调用）** —— 闭包值已经是 `MirExpr::FuncAddr`（i64
   函数地址），缺的是 **间接调用**：MIR 无 `CallIndirect`，codegen 无
   `inttoptr` + `build_indirect_call`。这是**新增能力**而不是修 bug；顺带也会让
   `g = f; g(1)`（当前裸 `_g` 链接失败）可用。

### 下一队列（批次一百五十，按价值/前置排序）

本批把「注册表里已有、降级层没接」这一类清完后，剩下 3 例都不是单点，各自带**前置改造**。
建议顺序（1 是 2 的前置）：

1. **列表字面量语义落地（t207）—— 两个前置必须一起做，否则会引发已实测的回归**
   - ① `vec_push`/`array_get` 是 i64 通道，f64 元素必须按元素静态类型 **bitcast f64↔i64**
     （否则 `[1.5, 2.5]` 打印成非规格化数 —— 实测 t56/t139 红）。
   - ② `f(*args)` 的展开目前靠 `Type::Array(_, ArraySize::Literal(n))` 取长度，字面量改成
     DynamicArray 后长度信息丢失（实测 f(*args) 变 0 参 —— t33 红）。须改从**字面量 AST**
     取长度，而不是从类型取。
   - 前置缺陷（独立于本项，值得顺手清）：**`array_push` 在 AOT 是空桩**
     （`runtime/tokio_runtime_stub.c:241` `(void)arr;(void)val;`），而既有
     `DynamicArrayLit` 降级仍在用它 ⇒ 该语法的元素被**静默丢弃**（当前无测试覆盖）。
   - 另：`zs: lt(vec, str) = []` 的注解仍被丢 —— `parse_assign`（`stmt.rs:371`）解析出
     `_ty` 后直接扔掉。要么让 `Assign` 带注解字段，要么「先定类型再降级 RHS」。
2. **`dict(x)` 未定型形参（t231）** —— 现为**故意**的响亮失败（gen.rs 注释：
   「copying an unknown handle would corrupt data」）。要过需要「调用点推断覆盖无标注
   形参」；py_asdict 那条线上已有同类机制雏形（AST 回写 + Named 分支），复用而不是在此放宽。
3. **间接调用（t233）** —— 闭包值已是 `MirExpr::FuncAddr`（i64 函数地址），缺 MIR
   `CallIndirect` + codegen `inttoptr` / `build_indirect_call`。属**新增能力**；顺带解锁
   `g = f; g(1)`（当前裸 `_g` 链接失败）。

**方法论沉淀（本批反复用到，优先复用）**：症状是「未定义 `_Xxx` / 整簇段错误」时，
先按「**注册表里有没有、只是降级层没接**」排查，而不是先动类型系统 —— 本批 13 例里
11 例是这一类。判据：`grep <symbol> pylib/registry.txt runtime/*.c src/backend/codegen/runtime_decls_registry.rs`
三处都在 ⇒ 缺口在 MIR 分派；三处都没有 ⇒ 才是真缺口。

---

## 批次一百五十（2026-09-19）：目标下沉到「跑起 REasyQuant 本地回测」

**目标锚点**（用户 2026-09-17 定的范围，见 memory）：只做**本地/可安装** API，
不碰聚宽平台 API；验收 = 把 `strategies/code/jq_wufu_local.py` 用 zetac 编译起来，
`--engine local` 能产出回测结果。

**复现命令**（不修改 REasyQuant 项目代码）：

```bash
REPLAYQUANT_LOCAL=1 ./target/release/zetac \
  ~/source/quant/REasyQuant/strategies/code/jq_wufu_local.py -o /tmp/wl/wufu_local
# 数未定义符号：
grep -A200 'Undefined symbols' <log> | grep -oE '^\s+"_[^"]+"' | sort -u | wc -l
```

### 本批修掉：编译中止（连链接都到不了）

`jq_wufu_local.py` 此前不是「链接失败」而是**编译中止**：

```
Error: "Function return type does not match operand type of return inst!
  ret double 0.000000e+00 / i64"
```

| # | 根因 | 修法 |
|---|---|---|
| 1 | `infer_fn_return_type` 只扫**顶层** `Return`；`if` 体内的 `return 0.0` 看不见 ⇒ 函数声明 i64 却发出 `ret double` ⇒ LLVM 校验失败 ⇒ 整个编译中止。codegen 的 Return 只有 `i64→f64` 单向补齐，缺反向 | 补 `f64→i64`(fptosi)：**声明是 i64 就按 i64 返回**（源码契约），编译器不再产生非法 IR |
| 2 | Python 拼写 `float`/`int` 没映射：`-> float` 被当不透明 Named ⇒ 被调用方签名 double、**调用点按 i64 取结果** ⇒ `print(fee(2.0))` 打出位模式 `4611686018427387904`（静默错值）；`amount: float` 拿 i64 ABI（79 条 ABI coerce 警告） | 三处类型解析器都要补（只补一处不生效）：`new_resolver::parse_type_string` · `Type::from_string` · `typecheck_new::string_to_type`（Resolver 的 `parse_type_string` 委托到它）+ gen.rs 形参落表的 `f64` 判据 |

回归锁：**t264**（pre-fix 实测编译中止）。度量：python_style 260/264 → **261/264**，
官方 194/194 · 语料 38/38 持平。

### 现在的状态：能编译到链接期，**未定义符号 89 个 / 219 个引用点**

> ⚠️ 本行原写「56 个」——那是 `grep -A200 'Undefined symbols'` **截断在 200 行**
> 少数了 33 个（该块有 219 行引用明细）。正确口径与影响见批次 151。下表的分桶
> 因此也漏了几个高频项（最典型的是 `_py_os_environ_get_1`，10 处引用，全库第一）。

按「谁来修、在不在 local 路径上」分桶：

| 桶 | 符号（示例） | 归属 | local 路径需要？ |
|---|---|---|---|
| A. 未运行引擎被整图编译 | `Cerebro` `adddata` `backend_strategy_backtrader_backend__BacktraderBackend` `backend_strategy_nautilus_backend__NautilusBackend___context_factory` `...___price_lookup` `getvalue` `from_int` `from_str` `decimal__Decimal` `__jq_bar_types` | `_run_backtrader` / nautilus 引擎。**惰性 import 也在函数体内被编译**，于是把链接拖死 | **不需要**——但**必须先解决「要不要链」**：`--engine` 是运行期参数，编译器不知道只跑 local。两条路：① 编译期按不可用引擎折叠（`_run_backtrader/_run_nautilus` 整体裁掉）；② 给这些外部依赖提供**响亮失败**的桩 |
| B. 导入模块的私有成员 / 函数内延迟 import | `backend_datasrc_market_data___baostock_login` `..._logout` `backend_datasrc_adjustment__anchor_to_reference` `backend_datasrc_fund_adj_adjustment___fetch_fund_adj` `backend_datasrc_calibration__DataCalibrator___reference_loader` | 源里都已定义（`market_data.py:612` / `adjustment.py:131` / `fund_adj_adjustment.py:61`），但**函数内** `from .market_data import _baostock_login` 后再裸名调用 ⇒ 调用点被 mangle、定义侧没发射 | **需要**（本地行情靠 baostock/本地 parquet，不走平台） |
| C. 嵌套 def / 匿名闭包 | `___closure`（闭包名丢了编号）、`LocalBackend::_price_lookup` 这类嵌套 def 的 mangle 名 | `_price_lookup(code, _d)` 是**函数内嵌套 def**，被当方法调用 | **需要**（`LocalBackend._price_lookup` 在 local 路径上） |
| D. vec/Series 接收者的方法分派落到「类型名当限定名」 | `[dynamic]str__median` `[dynamic]str__isna` `[dynamic]str__notna` `DataFrame__abs` `DataFrame__median` | `df["x"].median()` / `.isna()` / `.notna()` / `.abs()`：接收者静态类型是 DynamicArray/DataFrame，分派把它 display_name 当限定名 → 发一个谁也定义不了的符号 | **需要**（data 层 adjustment/split_factors/fundamental_data 大量使用） |
| E. `-> Any` 接收者的方法分派 | `Any__filter` | 与 memory 里已记的「`Named("Any")` 反查不到结构体声明」同源 | 需要（要确认具体调用点） |
| F. 库/注册表缺口（可安装或纯 Python） | `PyDate__to_pydatetime`（`jq_wufu_local.py:279` 直接用）· `PyDate__tz_convert` · `decimal__Decimal` · `getsignal`（stdlib `signal`）· `cache_clear`（`functools.lru_cache` 包装）· `encode`（str 方法）· `get_loc`/`get_level_values`（pandas 索引面）· `clip`/`all`/`any`（Series 面） | 逐项补 W 表 / registry / pylib | 多数**需要** |
| G. 平台 API（**不做**，按用户范围） | `all_instruments` `fund_daily` `index_components` `index_daily` `fund_etf_hist_em` `fund_etf_category_sina` `auth` `download` | 聚宽平台。**不补齐**，必须在本地路径上换成 baostock/本地 parquet 数据源 | 明确不做 |

### 下一队列（批次一百五十一，建议顺序）

1. **A 桶先做**：不解决「未运行引擎也被链接」，后面每修一个符号都会被 A 桶噪声淹没；
   而且 A 桶里有 10+ 个符号，做掉之后真实缺口才看得清。建议先做**编译期折叠**
   （`--engine` 的取值是运行期参数，所以要按「引擎不可用」而不是按 engine 值折叠：
   `backtrader`/`nautilus` 在本机不是可安装的纯 Python 依赖）。
2. **C 桶（嵌套 def / 闭包命名）**：这是纯编译器缺陷（符号名丢了编号 = 发错名），
   影响面比一个用例大，且 pre-fix/post-fix 可用最小复现锁定。
3. **D 桶（vec/Series 方法分派）**：一次修好 `median/isna/notna/abs` 四类，
   data 层立刻少一大片。
4. **B 桶（私有成员 + 函数内 import）**：与「模块内私有名导出」一体。
5. **F 桶**：逐个小口补（`to_pydatetime` 优先——`jq_wufu_local.py:279` 在 local 路径上直用）。
6. 至此才谈「链接成功 → 运行 → 出回测收益」。

### 方法论沉淀

- **「编译中止」优先于「链接失败」**：`Function return type does not match` 这类
  LLVM 校验错误会让整个编译停住，症状看起来像「编译器坏了」，实际是一条
  `ret` 未按声明类型转换。排查入口：`--emit-llvm 2>ir.ll` 后扫
  `define i64 @f` 内是否有 `ret double`。
- **同一条类型别名要补多处**：`float` 只在 `Type::from_string` 补了**不生效** ——
  签名走 `Resolver::parse_type_string` → `typecheck_new::string_to_type`。
  改类型别名前先 `grep -rn 'fn parse_type_string\|fn string_to_type\|fn from_string'`。
- 未定义符号清单要**按归属分桶**再动手，否则会在「明确不做的平台 API」上浪费预算。

---

## 批次一百五十一（2026-09-19）：裸 `try/finally` 是一等语法 —— 又一个非法 IR

**症状**（`backend/datasrc/market_data_universe.py` 独立编译）：

```
Error: "Terminator found in the middle of a basic block! label %entry"
```

**最小复现**（5 行，pre-fix 必现）：

```python
def f(n: i64) -> i64:
    try:
        return 3
    finally:
        print(9)
```

发出的 IR：

```llvm
entry:
  ret i64 3                        ; try 体里的 return
  call void @println_i64(i64 9)     ; finally 体 —— 落在终止指令之后
  ...
  ret i64 %arr_handle
```

**根因（在 parser 的回退路径，不在 codegen）**：`parse_try_stmt` 在没有 `except`
时**直接 Err**（`if !saw_except { return Err(...) }`），外层于是用**普通块**把
`finally { … }` 收下 ⇒ 它的语句成了 try 体之后的**同级语句**；try 体里一旦有
`return`，`finally` 的指令就排在 `ret` 后面 ⇒ LLVM 拒绝整个模块。

**修法（两处，互为补充）**

1. **根因**：`saw_except` 检查移到 finally 解析**之后**，条件放宽为
   `!saw_except && finally_body.is_empty()` ⇒ `try/finally` 走与
   `try/except/finally` 同一套 setjmp/If 降级（`finally` 落在 If 的 merge 块之后）。
2. **不变量**：codegen 新增 `ensure_emittable_block()` —— 发射语句前若当前基本块
   已有终止指令，就把该语句放进一个**新建的无前驱块**。任何 MIR 生产者都不可能
   再产出非法 IR；语句仍可见（不静默删）。在函数主语句循环显式调用。

**度量**：python_style 261/265 → **262/265**（新增 t265，pre-fix 实测 Terminator）
· 官方 194/194 · 语料 38/38 持平。

### ⚠️ 两条必须记住的**方法论**结论（本批踩出来的）

1. **「独立编译」与「整体编译」会给出不同的结论 —— 不能互相代替**。
   本缺陷在 `market_data_universe.py` **独立**编译时中止，而在 wufu local
   **整体**编译里**根本不出现**（pre-fix 日志 Terminator 计数 = 0）。实测：
   未定义符号清单 pre/post 完全一致（56 个，仅闭包编号因全局计数器而不同），
   `_backend_datasrc_market_data_universe__get_universe` 两个版本都已定义。
   ⇒ 这个修复是**红线类健壮性**修复，**对 `--engine local` 的符号集合没有净影响**。
   （我在上一轮口头汇报里说过它「卡住 local 路径的 get_universe」，那是**过度推断**，
   特此更正：结论要以 pre/post 符号清单对拉为准，不能从「单文件编译失败」推到
   「整体链接失败」。）
2. **覆盖盲区**：三套基线都不编译 `backend/**` 下的**模块文件**（语料是
   `strategies/`，官方单测与 python_style 是自包含用例）。所以「194/194 + 38/38 +
   wufu local 能到链接期」**不蕴含**「没有非法 IR」。本缺陷正是从这个盲区漏出来的。
   建议：把 `backend/**/*.py` 的**独立 compile-only** 加进 `tools/run_all.sh`
   （判据只看「有没有 codegen Error」，不看链接），成本低、能抓这类整模块级缺陷。

**已知遗留（未修）**：`try` 体内 `return` / 异常仍然**不执行 `finally`**（只有正常
完成路径会执行）。Python 语义要求每条退出路径都跑到 —— 需在 desugar 层把 finally
体插到每个 `Return` 之前（或引入 goto），是独立改动。本批只保证「不产生非法 IR +
正常路径语义正确」。语料里 `try: … return codes finally: _baostock_logout()` 这类
**清理型 finally 会被跳过**（资源泄漏，不是错值），应排在后续批次。

### 下一队列（批次一百五十二，按「离跑通 local 的距离」重排）

1. **`getattr` 39 处**（`error: builtin getattr is not implemented in this form`）——
   local 路径整体编译里出现 **39 次**，是目前**最大的一块**；每次都是一个
   「静默走默认值 / 幽灵符号」的隐患。先按调用点分类（`g` 对象字段 vs 其他）。
2. **间接调用**（t233 同源）：解掉 `LocalBackend._price_lookup`（local 路径上）
   + nautilus 的 `__context_factory`/`__price_lookup`。MIR 需 `CallIndirect`，
   闭包值已是 `FuncAddr`。
3. **`finally` 语义**（本节遗留）：清理型 finally 被跳过。
4. **A 桶**：backtrader/nautilus —— 库桩（一个 `py_unavailable` C 函数 + N 条
   registry）或编译期裁剪，二选一。
5. **D 桶**（`[dynamic]str__median/isna/notna`、`DataFrame__abs/median`）→
   **B 桶**（私有成员 + 函数内 import）→ **E/F 桶**逐个小口。
6. 最后才谈「链接成功 → 运行 → 出回测收益」。

### 批次一百五十一 追加（同日）：arity 后缀裸符号 + 未定义符号口径更正

**口径更正**：`grep -A200 'Undefined symbols'` **截断在 200 行**，而该块有 **219 行**
引用明细 ⇒ 少数 33 个符号。正确数法：

```python
python3 - <<'PY'
import re; t=open(LOG).read(); blk=t[t.index('Undefined symbols'):]
print(len(set(re.findall(r'^\s+"([^"]+)"', blk, re.M))))   # 符号数
PY
# 引用点总数：把每个符号名下面 "… in wufu_local.o" 的行计数
```

**实测**：wufu local **89 符号 / 219 引用点 → 88 / 210**。

**本批修掉的 2 个（合计 12 处引用）——「调用点形状 ≠ registry 声明 arity」**

| 调用 | registry | 调用点 arity | 之前 | 现在 |
|---|---|---|---|---|
| `os.environ.get(K)` | `args=i64,i64` | 1（Python default 可省） | 未定义 `py_os_environ_get_1`（**10 处引用，全库第一**） | C 侧 `py_os_environ_get_1(k) → py_os_environ_get(k, 0)` |
| `pd.Timestamp(s, unit=…, tz=…)` | `args=i64` | 3 | 未定义 `py_dt_from_str_3`（2 处） | C 侧 `py_dt_from_str_3(s,unit,tz)`：**忽略** unit/tz（PyDate 句柄本就不带时区/日内精度，编造偏移=静默错值） |

沿用运行时既有 `_N` 约定（同批次 149 的 `py_os_makedirs_2` / `py_path_read_text_2`）：
**C 侧补真实现**，不在 MIR 里丢参数、不返假值。回归锁 **t266**（`// env:` 设值 +
未设键打空行 + Timestamp 的 year/month）。python_style **263/266**，
官方 194/194 · 语料 38/38 持平。

**关键施工结论：修一个会露出下一个 ⇒ 剩余缺口必须迭代到不动点。**

本轮修掉 2 个，同时**新出现** `_backend_datasrc_market_data__fetch_stock_data`：
`os.environ.get` 通了之后，原本半途放弃的函数编译到了下一处缺口。
成因是**项目侧既有隐患**：`market_data_sources.py:60` 写
`from .market_data import _baostock_login, _baostock_logout, fetch_stock_data`，
而 facade `market_data.py` **并没有** re-export `fetch_stock_data`
（CPython 只在真的走到那条 source 时才 ImportError）。
⇒ 不能按一次性清单估工；每轮以「符号数 + 引用点数」两个指标收敛，
并留意**新出现的符号**（不是回归，是原先被前一个错误遮住的下一层）。

---

## 批次一百五十二（2026-09-19）：`and`/`or` 取值语义 —— Python 语义的静默错值

**症状**（REasyQuant 本地回测里 7 处 `_get` 幽灵符号的根因之一）

```python
def from_jq(cls, config: dict | None) -> CostModel:
    cfg = config or {}
    return cls(slippage=float(cfg.get("slippage", 0.0)), …)   # ← 裸 `_get`
```

**根因**：Python 的 `x or d` 值是 `x`（真值时）或 `d`（`and` 对称），但 codegen 把它
实现成**布尔**运算（`icmp ne 0` → `and`/`or` → `zext`）⇒ 结果恒 0/1：

| 表达式 | pre-fix | 正确（Python） |
|---|---|---|
| `0 or 7` | 1 | 7 |
| `3 or 7` | 1 | 3 |
| `0 and 7` | 0 | 0 |
| `3 and 7` | 1 | 7 |

⇒ `cfg = m or {}` 之后 `cfg` 是布尔 1，`cfg.get(k,d)` 把 1 当 map 句柄；
`t = s or "x"` 之后 `.upper()` 丢字符串。

**修法（两处必须同时改）**

1. **codegen**：`or` → `select(truthy(left), left, right)`；`and` →
   `select(truthy(left), right, left)`。两侧都是 i64，select 后仍 i64，ABI 不变。
2. **MIR 结果类型**：同类型操作数取该类型；一侧是不透明默认（I64/PyDynamic/Bool）
   时取另一侧的具体类型 ⇒ `cfg` 是 map（`.get()` 走 `W map get map_get_default`），
   而 `if a or b:` 仍是 Bool。

> ⚠️ **只做 (2) 会从「链接失败」变成「段错误」**（类型说是 map、值却是布尔 1）——
> 实测踩到并回退，两处一起落地才成立。这是本项目「只改类型不改值 = 静默错值」的
> 又一例，值得当检查项。

**度量**：python_style 263/266 → **264/267**（新增 t267）· 官方 194/194 · 语料 38/38
**持平**（含语义变更也未回退）· wufu local **88 符号 / 208 引用点**（`_dict` 5→3，
`CostModel::from_jq` 的 3 处 `_get` 消失）。

### 本轮另外两个**已定位到根因**的符号家族（下一批直接开工）

**家族 B1：`from <文件模块> import <名字>` 在模块**尚未加载**时就被解析 ⇒ 永久外部桩**

日志行号是铁证（`out4.log`）：

```
line 18  unknown member `_baostock_login` in Python module `backend.datasrc.market_data`
line 20  unknown member `fetch_stock_data`  in …`backend.datasrc.market_data`
line 21  unknown member `fetch_benchmark_returns` in …
line 22  unknown member `get_universe`      in …
line 58  unknown member `_FETCH_SOURCE_NAMES` in …
line 76  PY-A: imported module `backend.datasrc.market_data` from …/market_data.py   ← 才加载
```

**6 个名字全部在 facade 加载之前被解析**（18/20/21/22/58 < 76），于是 `known=false`
→ 警告 + 记成 external shim → 调用点发裸 mangle 符号（`_baostock_login` 6 处引用）。
成因是**循环 import**：`market_data` → `market_data_sources` →（函数内延迟 import）
`market_data`，加载器遇到「正在加载中」就放弃 ⇒ `is_user` 仍为 false。
修法方向：from-import 的**名字解析推迟到目标模块加载完成之后**（占位/两遍解析），
或让 `load_user_python_module` 对循环可达（排队待解析）。收益 ≈ 6 名字 / 14 引用点。

**家族 B2：模块级函数被**选择性发射**（不是解析失败）**

`wufu_local.o` 里 `backend_datasrc_adjustment__compute_split_factors` /
`detect_split_dates` / `__init` **有**（T），而 `anchor_to_reference` /
`apply_qfq_adjustment` **完全没有**（连别的名字下也没有，`nm | grep -i anchor` 为空），
但调用点要的是 `_backend_datasrc_adjustment__anchor_to_reference`（2 处引用）。
⇒ 定义侧压根没发射，不是 mangle 不一致。**注意**：`adjustment.py` **独立**编译时
有 `[W1002] … 163 line(s) … NOT parsed`（起点 `def apply_qfq_adjustment(`），
而 wufu local **整体**编译里 W1002 计数 = **0** —— 又一次「独立 vs 整体结论不同」。
下一步：先用 `--dump-mir`/`nm` 判定 `anchor_to_reference` 是「未发射」还是「发射到
别的名字」，再决定是发射条件（可达性）还是解析截断。

### 下一队列（批次一百五十三）

1. **家族 B1**（循环 import 的名字解析时机）—— 完整定位、收益明确（6 名字/14 引用）
2. **家族 B2**（选择性发射 / 独立 vs 整体不一致）—— 需先做上面的判定实验
3. **`getattr` 39 处**：最大的单块，先按调用点分类
4. **间接调用**：`LocalBackend._price_lookup`（local 路径）+ nautilus 两个
5. 库桩（A 桶）/ D 桶 vec-Series 分派 / 未类型化接收者的 `.get/.values/.cls/.date`
   家族（`_get` 7 · `_values` 7 · `_cls` 7 · `_date` 7 —— 注意 `_cls`/`_date` 多来自
   **classmethod 的 `cls` 与装饰器被忽略**这条既有债务）
6. **把 `backend/**/*.py` 独立 compile-only 加进 `tools/run_all.sh`**（覆盖盲区，
   本批已第二次靠它发现差异）

---

## 批次一百五十三（2026-09-19）：字典字段类型化 + `Path.parents`

### 1. `self.x = {}` 字段被类型化成 i64（含**一处静默错值**）

`parse_class` 的字段类型推断只认字面量/bool/浮点/字符串/数组，`AstNode::DictLit`
落在 `_ => "i64"` ⇒ 字典字段是 I64：

| 写法 | 之前 | 现在 |
|---|---|---|
| `self.m.get(k, d)` | 裸 `_get`（链接失败） | `W map get map_get_default` |
| `self.m.values()` | 裸 `_values`（链接失败） | map 方法分支 |
| **`k in self.m`** | **恒返回 0 —— 静默错值** | 正确 1/0 |

第三行是本批最重要的一条：它**不报错**，只把成员判定恒判为假。
（独立最小复现：只留 `k in self.m`，pre-fix 编译通过并打印 0，post-fix 打印 1。）

修法：字段推断补 `DictLit => "map"`；`Type::from_string` 把 `dict[K,V]` / 裸 `dict`
归一化成 `map`（所有 map 消费点都按名字 `"map"` 分派）。
实测消掉 `_PositionLedger::clear_today_buys` / `on_trading_day` 两处 `_values`。
回归锁 **t268**。

### 2. `Path.parents[N]` 新能力（+ 4 处 W 表结果类型补全）

`W PyPath parents` 此前不存在 ⇒ `Path(__file__).resolve().parents[2]` 断链：

- `.parents` 落 opaque 兜底 = **静默空值**（t269 pre-fix：`Compiled to` 但零输出）
- 后面接 `.exists()` / `.read_text()` 则变成裸符号（`_exists` 7 / `_read_text` 6）

修法：C `py_path_parents` → Vec（元素即路径字符串句柄）+ registry 新结果种类
`ret=vecpath` + `vecpath → DynamicArray(Named("PyPath"))` 补进 **4 处** W 表结果类型映射
（4283 句柄分派 / 7037 B4 唯一名 / 7618 Named 接收者 / 8488+8558+8616 FieldAccess）。
**少了任何一处**，`[N]` 拿到的都是 I64 ⇒ 后续方法又发裸符号 —— 本批是逐处定位出来的，
这也是「同一条类型映射散落在多处」这条债的又一实例（同批次 152 的 `float` 别名）。
回归锁 **t269**（`b a tmp` / `4` / 链式 `b`）。

### ⚠️ 3. 由此**新定位**的一个独立机制：模块全局类型推断（未修）

REasyQuant 里 `_exists` / `_read_text` 的**主体**调用点**没有**因此减少（7→7 / 6→6）。
逐层追到根因（MIR 铁证）：

```
_LISTING_CACHE = _PROJECT_ROOT / "data" / "universe" / "etf_listing.json"
→ MIR: zeta_env_get(...) 的类型是 I64  →  call exists_1        # 不是 PyPath
```

即**解析器侧的 `module_global_types` 推断**没有覆盖
`Path(...).resolve().parents[2]` 与 `/` 运算符链 ⇒ 全局记为「未定型」⇒
方法调用发裸符号。这与 MIR 层类型（本批修的）**无关**，是独立的一块。
受影响符号：`_exists`(7) / `_read_text`(6) / `_is_file` / `_mkdir` 等涉及
`_PROJECT_ROOT` 派生全局的调用点。**下一批优先做这个**（收益 13+ 引用点，
且是「模块级常量链」这一整类）。

### 度量（本批累计）

| 口径 | 批次 152 后 | 现在 |
|---|---|---|
| python_style | 264/267 | **266/269**（+t268/t269） |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |
| wufu local 未定义符号 | 88 / 208 | **88 / 208**（本批两条修复都不在这条链上，见 §3） |

### 下一队列（批次一百五十四）

1. **模块全局类型推断**（§3）—— 本批新定位，收益 13+ 引用点，是「常量链」整类
2. **家族 B1**（循环 import 的名字解析时机）
3. **`getattr` 39 处**
4. **间接调用**（`LocalBackend._price_lookup`）
5. 库桩（A 桶）/ D 桶 vec-Series 分派 / `_cls`·`_date` 家族
6. 工具：`backend/**/*.py` 独立 compile-only 进 `tools/run_all.sh`

### 批次一百五十三 追加：模块级「常量链」推断（**部分达成**，附精确剩余点）

**修掉的（有独立证据）**

| 项 | 之前 | 现在 |
|---|---|---|
| `_ROOT = Path(__file__).resolve().parents[2]` 派生路径链 | 全局落空 ⇒ I64 ⇒ `.exists()` 发裸符号 | 递归 `infer_global_ty()` 解析（Var 别名 / `/` 走 `handle_op` / `Subscript` 取元素 / `FieldAccess` 取 W 表属性 / `Call` 的 handle 链**与**按名 import 的成员调用） |
| 多模块编译下 `module_globals` 存 mangled 名 | walk 用裸名判断 ⇒ **整表空**（探针 defs=448/globals=372/**typed=0**） | 补 `bare_globals`（`rsplit_once("__")` 去前缀） |
| `cache_path: Path` 形参的静态类型是 `Named("Path")` | W 表按 `PyPath` 键 ⇒ `Path::write_text` 未定义 | Named 接收者分派先 `handle_tag(tn)` 归一再查 W 表 |
| W 表结果类型映射散在 MIR 4 处 | 批次 153 为 `vecpath` 逐处补过 | 新增 `method_result_ty()` 作为**单点映射**（resolver 侧） |

**证据**：t270（新增）pre-fix `Undefined symbols` → post-fix `x.json / 0 / 0`；
单模块编译探针显示 `_PROJECT_ROOT`/`_LISTING_CACHE`/`_ETF_UNIVERSE_CACHE` 均为
`Named("PyPath")`，**etf_listing 独立编译的 `_exists`/`_read_text` 归零**。
度量：python_style **267/270** · 官方 194/194 · 语料 38/38 ·
**wufu local 88/208 → 87/204**（`_debug` 消失，**无新增符号**）。

**⚠️ 未达成（不要误读为已修）**：**整体**编译里那张表**仍然是空的**
（同一探针：defs=448 / globals=372 / **typed=0**），所以 etf_listing 等模块的
`_exists` / `_read_text` 在 wufu-local 链接里**仍在**。已逐一排除：mangled 名字（已修）、
`Path(...)` 接收者（已修）、`parents`（批次 153 已加）、`/` 运算符（已验）。
最小复现都能命中（单模块 ✓、「import 一个模块」✓），**只有全量模块图不行** ⇒
下一批从「全量编译里 `<mod>__init` 的函数体为什么没被 `walk` 到」开刀
（建议：在 `module_global_types` 里对每个 def 打点，看 448 个 def 中 `<mod>__init`
的在不在、body 是否为空）。

### 批次一百五十四（2026-09-19）：模块全局类型表在全量编译下为空（**已修**）+ 2 个 `Path` 面补齐

**决定性一步**：批次 153 追加里我加的 `bare_globals` 用 `rsplit_once("__")` 剥模块前缀，
对**以下划线开头**的全局名是错的 ——

```
prefix = "backend_datasrc_etf_listing__"   # 结尾两个下划线
name   = "_PROJECT_ROOT"                   # 开头一个下划线
拼接   → "...etf_listing" + "___" + "PROJECT_ROOT"   # 连成三个下划线
rsplit_once("__") → "PROJECT_ROOT"          # 前导下划线被吃掉
```

walk 查的是源码裸名 `_PROJECT_ROOT` ⇒ 仍不匹配 ⇒ **整表为空**
（探针：defs=448 / globals=372 / **typed=0**；单模块编译正常，因为裸名直接进 globals）。
修法：按**已知模块前缀**（`py_loaded_modules` 逐个 `<module with _ for .>__` 做
`strip_prefix`）剥离，不再猜分隔符。

**顺带补齐的 Path 面**（都是「接收者类型修好后**才暴露**出来的下一层」）：

| 调用 | 之前 | 现在 |
|---|---|---|
| `p.write_text(s, encoding="utf-8")` | `py_path_write_text_3` 未定义 | C `_3`（忽略 encoding：字节原样写） |
| `d.mkdir(parents=True, exist_ok=True)` | `_PyPath__mkdir` 未定义（**corpus 29 处，全是这个形态**） | `W PyPath mkdir` + C `_3`（parents→建链，exist_ok→容忍 EEXIST） |

**度量**

| 口径 | before | after |
|---|---|---|
| wufu local 未定义符号 | 87 / **204** 引用点 | **87 / 194**（`_exists` 7→**2** · `_read_text` 6→**2**） |
| python_style / 官方 / 语料 | 267/270 · 194/194 · 38/38 | 持平 |

累计（本会话）：**89 符号 / 219 引用点 → 87 / 194**。

**方法论沉淀（第三次同类）**：修好一层类型后，**下一层的裸符号会浮现**，
且往往形态不同 —— 本批依次暴露 `py_path_write_text_3` → `_PyPath__mkdir`。
每修一层都要重新取符号清单（`grep -A400 'Undefined symbols'` 后再解析，
**不要用 `-A200`：会截断**）。剩余 `_exists`(2)/`_read_text`(2) 的调用点
（`market_data_universe` 的 f-string 路径、`nautilus_engine._make_equity`、
`ParquetCache::save`）留待下批逐个看。

### 批次一百五十四 追加：`_exists`/`_read_text` 清零 + `Path.glob`

**再导出的全局名读的是 mangled 键**：`market_data_universe.py` 自己**不定义**
`_PROJECT_ROOT`，而是从 `.market_data_sources` import；它的读取走
`backend_datasrc_market_data_sources___PROJECT_ROOT`，而表里只有 walk 从源码 AST
取到的裸名 ⇒ 查不到 ⇒ 该模块仍是 I64 ⇒ `cache_path.exists()` 发裸符号。
修法：walk **两个键都写**（裸名 + `<prefix><name>`；前缀从 module body 的注册名
`<mod>__init` 尾部剥 `init` 得到）。

**`Path.glob`**：`_LOG_DIR.glob("*.jsonl")` 等（data_ops_log / task_store /
ml.store / tools.strategy）→ `_PyPath__glob`。补 C `py_path_glob`（`glob(3)` → 路径
字符串 Vec）+ `W PyPath glob … ret=vecpath`。

**度量**：wufu local **87/194 → 85/186**；`_exists` **2→0** · `_read_text` **2→0**；
python_style 267/270 · 官方 194/194 · 语料 38/38 持平。
累计（本会话）：**89 符号 / 219 引用点 → 85 / 186**。

**本批的完整修复链（每一步都是上一步的直接后果，形态各不相同）**：

```
bare_globals 前缀剥离（rsplit_once("__") 吃掉前导下划线）
  → py_path_write_text_3（类型修好后 write_text 变 3 参）
  → _PyPath__mkdir（29 处 corpus 调用，W 表从未有 mkdir）
  → 再导出全局的 mangled 键（同名跨模块读取走的是 mangled 名）
  → _PyPath__glob（W 表缺口）
```

⇒ 施工纪律：**每修一层都必须重新取符号清单核对**（`grep -A400 'Undefined symbols'`，
**不要 `-A200`**），不能复用上一轮的结论。

### 批次一百五十五（2026-09-19）：`-> dict` 注解归一化 + `dict.get()` 值类型

| 项 | 根因 | 修法 |
|---|---|---|
| `def f() -> dict:` 之后 `.get(...)` 发裸符号（`_get` 7 引用） | **签名**解析走 `typecheck_new::string_to_type`，**没做** `dict → map`（批次 153 只补了 `Type::from_string`） | 裸名 / 泛型 `dict[...]` / `lt(dict,…)` 三处都归一（**同一别名散落 N 个解析器** —— 本会话第三次，前两次：`float`、`vecpath`） |
| `dict.get(k[,d])` 返回类型写死 I64 | map 分支两处（2 参 DictGet / 3 参 `map_get_default`）都 `insert(id, Type::I64)` ⇒ str 值字典打印出**裸句柄数字**、比较虽对但 `print` 是错值 | 取接收者 `map<K,V>` 的 **V** 类型（缺省 I64） |

**关键的施工教训（响亮 → 静默 的退化，已拦下）**：只做第一项时，
`dict[str,str].get()` 会**编译通过但打印句柄数字** —— 从「链接失败（响亮）」变成
「静默错值」。**第二项是第一项的前置条件，必须同批落地**。这也是红线
「不支持的语法必须报错，不得静默吞掉产生错值」的正面样本：类型修好之前，
是**链接**在替我们兜底。

**度量**：python_style **268/271**（+t271）· 官方 194/194 · 语料 38/38 ·
wufu local **85/187 → 85/185**（`_get` 8→7 · `_setdefault` 3→2）。

### 批次一百五十五 附：本轮**尝试并回退**的一项（留给下批）

**模块内私有函数/方法体的 `symbol_renames` 缺失**（`_to_ts` 8 引用 ·
`_baostock_login` 6 · `_baostock_logout` 4 = 同一族）：

- `module_renames_for(func_name)` 用 `py_mangled_to_module[func_name]` 查模块，
  而**方法**的 MIR 名是 `Class::method`（裸类名）⇒ 查不到 ⇒ 整张 rename 表为空 ⇒
  方法体内调用同模块的私有函数时发出**裸名**，而定义侧是 `<prefix>_to_ts` ⇒ undefined。
- 我加了「按类名在 `py_module_own_names` 里唯一匹配」的回退：**MIR 侧确实生效**
  （dump 显示方法体内已改成 `backend_datasrc_market_data_fetcher___to_ts`），
  但**度量没动**，反而多了 2 个幽灵（`_nautilus_trader_model_objects__Money`、
  `_pd.Timestamp__date` —— 来自 reexports 表在新上下文里被应用），故**已回退**。
- 剩余未覆盖的 3 个子形态（下批从这里开）：
  1. 方法体的 **free 镜像/trampoline**（`_fetch_stocks` / `_fetch_stocks_inst_i64`）——
     它们没有 rename 表；
  2. **模块级自由函数**（`_is_cache_fully_covered`）—— 同样发裸名；
  3. **同名文件被加载成两个模块**（`jq_shim` 与 `strategies.code.jq_shim`）⇒
     类名匹配命中 2 个模块 ⇒ 我的「唯一匹配」直接放弃（探针实测 hits=["jq_shim",
     "strategies.code.jq_shim"]）。
  ⇒ 收益可观（18 引用点 / 3 个符号），但需要把 rename 挂到**镜像生成**与
  **模块级函数**两条路径上，而不是在查找侧兜。

---

## 批次一百五十六（2026-09-19）：本地回测入口**首次链接成功** + 模块级语句不再被丢弃

**目标锚点**：用户要求 `strategies/code/jq_wufu_local.py --engine local` **完全编译运行**。

### 1. 「有 `def main` 就不跑模块级语句」——本地入口因此静默失效（**已修**）

`synthesize_implicit_main` 一见 `def main` 就 `return asts`（early return）⇒ 模块级
语句**从不收集、从不执行**。`jq_wufu_local.py` 的本地模式全靠模块级语句：

```python
if os.environ.get('REPLAYQUANT_LOCAL') == '1':
    import jq_wufu as _strategy
    from strategies.code.jq_shim import (get_cost_config, get_price, ...)
```

⇒ 静默不生效、`_strategy` 未绑定 ⇒ 回测在打印第一行日志后崩。

修法：不再 early return，把模块级语句**前置进用户的 `main`**。
⚠️ 关键细节：`if __name__ == "__main__": main()` 在 parse 期已被解开成裸 `main()`
调用 ⇒ 前置会**自我递归**（实测无限 `A` 输出后 SEGV）⇒ 该调用必须丢弃。
回归锁 **t272**（pre-fix 实测只打 `B 10`：A 丢失、X 读到垃圾）。

### 2. 85 个未定义符号 → 0：`runtime/unavailable_stubs.c`（**生成 + weak**）

对「模块图里被调用、无人定义」的符号各给一个**响亮 abort**（打印自己的名字；
`ZETA_LENIENT_STUBS=1` 降级为 warning + 返回 0，便于一次跑出全部未实现路径）。
`tools/build_runtime.sh` 并进 `tokio_runtime.o`。

> ⚠️ **必须是 weak 符号**：runtime 对象链进**每个**程序，`_add`/`_get`/`_dict`/`_init`/
> `_date` 这类名字与普通程序定义撞车 —— 实测 strong 版本把官方套件从 194 打到
> **188**（`1 duplicate symbols`）。加 `__attribute__((weak))` 后程序自身定义优先。
>
> ⚠️ 生成时注意 **macOS 名字约定**：`ld` 报的名字带前导下划线（`_Cerebro`），
> C 里要写 `Cerebro`（再被编译器加一个下划线）；`[dynamic]str__isna` 这类非法标识符
> 走 `__asm__("<linker 名>")`。

### 3. 当前运行状态（实测）

```bash
$ REPLAYQUANT_LOCAL=1 zetac .../strategies/code/jq_wufu_local.py -o /tmp/wl/wufu_local
rc=0  →  489 KB 可执行文件
$ ./wufu_local --start 2024-01-02 --end 2024-02-29
[INFO] 获取数据...        # 首次有输出（此前零输出）
Trace/BPT trap: 5 (rc=133)
```

**lldb 精确定位**：

```
frame #0: backend_datasrc_market_data__init + 2380  →  brk #0x1
frame #1: run_backtest + 92
frame #2: main + 1032
```

崩点紧跟该模块**模块级 try/except** 的 `zeta_try_end`；该 try 的 import
（`from .market_data_universe import WUFU_BS_CODES, …` — 名字实际不在那个模块，
真实 Python 下必然抛 ImportError 并被 `except ImportError: pass` 接住）⇒ 走的是
**setjmp 返回非 0（longjmp）那条路**。**已排除**：IR 非法（then/else 都 `br` 到 merge，
函数以 `ret i64 0` 结尾）、`_setjmp` 缺 `returns_twice`（IR 里声明带
`#3 = { returns_twice }`）⇒ 结论是 **-O 阶段把 longjmp 返回路径优化成不可达**。
最小复现 `try: raise … except: pass` **不复现**，触发条件与函数体规模/上下文相关。

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 268/271 | **269/272**（+t272） |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |
| wufu local | 链接失败 | **链接成功**；运行到 `获取数据...` 后崩在上述已知点 |

### 下一队列（批次一百五十七）

1. **longjmp 返回路径被 -O 判为不可达**（本批精确定位）——最小复现要靠**堆大函数体**
   （已试小函数/纯 raise 不复现）；可先用「把 `zeta_try_*` 相关函数标记
   `optnone`/在 `finalize_and_aot` 关掉该函数的优化」验证方向
2. 数据层：`py_pd_read_parquet` 目前是**返回 0 的桩**（`runtime/tokio_runtime_stub.c:1273`）
   ⇒ 即使崩点修好，"运行出回测结果"还需要**真实 parquet 读取**（2636 个 parquet /
   669MB 在 `REasyQuant/data/`）+ pandas 面（`Series.max/min/abs/isna/median/clip` 等）
3. `unavailable_stubs.c` 的 85 条要按「运行真正撞到哪条」逐条替换为真实现
   （lenient 模式一次性跑出全部命中）
4. 家族 B1（循环 import 名字解析）、`getattr` 39 处、间接调用（`LocalBackend._price_lookup`）

### 批次一百五十六 追加：-O3 误编译**已证实** + 下一个真阻塞（`from mod import VAR`）

**1. `ZETA_NO_OPT=1` 诊断开关**（`jit.rs::optimize_module` 直接返回）——只差是否跑 O3，
同一个二进制：

| 构建 | 运行结果 |
|---|---|
| 正常（O3） | `[INFO] 获取数据...` → **SIGTRAP**（`brk #0x1`） |
| `ZETA_NO_OPT=1` | `[INFO] 获取数据...` → `Unhandled exception: code=4372430400`（**可读诊断**，rc=1） |

⇒ 批次 156 排除「IR 非法」「`_setjmp` 缺 returns_twice」后，本条**证实** O3 把 longjmp
返回路径优化成了不可达。（顺带发现：未优化时还会多出 **22** 个未定义符号 —— 这些路径
被 O3 当死代码删了，已一并补桩。）

**2. 下一个真阻塞：`from <模块> import <变量>` 坏掉**（2 文件最小复现）

```python
# regmod.py
D = {"a": 1}
def size() -> i64: return len(D)
def reg(k: str, v: i64) -> None: D[k] = v
# use.py
from regmod import D, reg, size
print(size()); reg("b", 2); print(size())
```

| 变体 | 实测 |
|---|---|
| `from regmod import reg, size`（**不**导入变量） | `1 2` ✓（跨模块改写模块全局 dict 正常） |
| `from regmod import D, reg, size` | **SEGV** ✗ |
| `from regmod2 import D` + `len(D)` | **0**（应为 1）✗ |

⇒ 函数导入正常、**变量导入拿到 0/坏槽**。`jq_wufu_local` 的
`from backend.strategy.wufu_constants import WUFU_INDEX_BS_CODES`（模块级列表）
正是这种用法 ⇒ 本地回测必然踩到，也是 `get_universe("wufu")` 抛
`Unknown universe` 的链条末端。

**下一队列（批次一百五十七）**：
1. **修 `from mod import VAR` 的绑定**（最小复现已就位；优先于其它）
2. -O3 误编译 longjmp 返回路径（可用 `ZETA_NO_OPT` 对比 + 堆大函数体的最小复现）
3. 数据层 `py_pd_read_parquet` 真实实现（2636 个 parquet/669MB 在 REasyQuant/data/）
4. 85+22 条桩按「运行真正撞到哪条」逐条替换（`ZETA_LENIENT_STUBS=1` 一次跑全）

---

## 批次一百五十七（2026-09-19）：`from mod import VAR` + `pd.DataFrame(columns=…)` + **CPython 参考基准**

### 1. `from <模块> import <变量>`（**已修**，t273）

**函数**导入有解析路径（调用走 `py_member_call` → `<mod>__<func>`），**变量**导入没有：
`AstNode::Var` 的降级只有 `symbol_renames`（对**根文件的 main** 是空表）与 nonlocal 两条分支
⇒ `D` 绑成未定义的新槽位（MIR 里是自引用 `8: Var(8)`）⇒ `len(D)` = 0；当 dict 用则 SEGV。

| 2 文件最小复现 | 之前 | 现在 |
|---|---|---|
| `from regmod import reg, size`（不导入变量） | `1 2` ✓ | ✓ |
| `from regmod import D, reg, size` | **SEGV** | `1 2 2` ✓ |
| `from regmod2 import D` + `len(D)` | **0** | `1` ✓ |

修法：`Var` 降级里新增一条 —— 命中 `py_member_aliases` 且目标模块在 `py_user_modules`
时改写为该模块的 env 全局 `<mod>__<member>`（loader 已把每个模块级绑定存进 env）。
局部名优先，不改变遮蔽语义。

### 2. `pd.DataFrame(columns=[…])`（**已修**，t274）

通用路径丢掉 kwarg 名、把值当 `data` 传 ⇒ 列映射里存进一个 Vec ⇒ `len(df)` /`.columns` SEGV。
修法（三步都必要，见 commit `8c0b4cf4`）：识别 kwargs-only → 按 `columns=` 建
「名字 → 空列」dict **字面量**（字面量列表是 StackArray，走运行时 helper 会读成长度 0）
→ 用**原 receiver** 重入普通路径（直接发自由调用会拿不到返回类型 ⇒ `e.columns` 退化成
字段读 + `array_len` = 0）。

### 3. ⭐ **CPython 参考基准**（本批最重要的产出）

**CPython 跑项目文档里的入口也失败**，错误与我们的检查一致：

```bash
$ REPLAYQUANT_LOCAL=1 .venv/bin/python strategies/code/jq_wufu_local.py --start 2024-01-02 --end 2024-02-29
ValueError: Unknown universe: wufu. Available: ['small_scale', 'hs300', 'csi500', …]
```

⇒ `get_universe("wufu")` 需要 `backend.strategy.wufu_constants` **先被 import**
（wufu universe 由它的模块级 `_register_into_datasrc()` 注册），而入口在第 67 行调用
`get_universe` **之后**才 import 它 —— **项目自身的缺陷，不是编译差异**。

补上注册再跑（`REasyQuant/_zeta_local_drv.py`，本项目侧的**临时对照程序**，不入库）：

```json
{"initial_cash": 1000000.0, "final_value": 994575.84, "return": -0.5424,
 "trading_days": 37, "engine": "local",
 "final_holdings": [{"code":"159509.XSHE","amount":260900,"avg_cost":1.2804},
                    {"code":"510880.XSHG","amount":115300,"avg_cost":2.8972},
                    {"code":"515220.XSHG","amount":264500,"avg_cost":1.2087}]}
```

**验收目标由此明确**：编译后的程序跑同一段代码（含注册步骤）应给出**同样的指标**。

### 度量

| 口径 | before | after |
|---|---|---|
| python_style | 269/272 | **271/274**（+t273/t274） |
| 官方 / 语料 | 194/194 · 38/38 | 持平 |
| 对照程序（driver）编译 | 链接失败 | 仍差 `_DataFrame`（个别调用形状未覆盖） |

### 下一队列（批次一百五十八）

1. **对照程序剩余的 `_DataFrame`**：`data_cleaning.code_coverage_stats` 里
   `pd.DataFrame(columns=[…])` 的 MIR 仍是 `DataFrame_2 args 9,10`（两个位置参数）——
   与 t274 通过的形状不同，需 dump 出这两个 id 的来源再定；`nav.py` 的
   `pd.DataFrame(index=…, dtype=…)` 同族
2. -O3 误编译 longjmp 返回路径（`ZETA_NO_OPT=1` 可对照）
3. 数据层 `py_pd_read_parquet` 真实实现（2636 parquet / 669MB）——**运行出真实结果的前置**
4. 107 条桩按「运行真正撞到哪条」逐条替换（`ZETA_LENIENT_STUBS=1` 一次跑全）

---

## 批次一百五十九（2026-09-19）：把「wufu universe 注册丢失」查到底（语料 **38/38 → 42/42**）

目标：让 local wufu 回测**跑起来**。本批不追新符号，而是把「`get_universe("wufu")` 抛
`Unknown universe`」这条链逐层拆开——**四层，每层都是独立缺陷**。

### 链条与修法

| # | 层 | 症状/证据 | 修法 |
|---|---|---|---|
| 1 | `list(<map>)` 返回 map 本身 | `WUFU_JQ_CODES = list(dict.fromkeys(…))` 拿到 map ⇒ 下游 `WUFU_BS_CODES` 为空 | map/dict 接收者 → `map_keys(x)`，元素类型取键类型（`list(x)` 的「透传」只对数组成立） |
| 2 | 模块级 str 列表被记成 `DynamicArray(I64)` | `dict.fromkeys(CODES + ["y"])` 的 `keys_are_str=false` ⇒ 键**未做内容哈希** ⇒ 去重丢失 | `infer_global_ty` 的 `ArrayLit` 取**第一个元素**的类型 |
| 3 | 函数内**相对** import 不触发目标 module init | MIR 里没有 `market_data_universe__init`；marker 携带的是**原始相对 spec**（`..datasrc.market_data_universe`），init 发射处拿它查 `py_user_modules` ⇒ 查不到 ⇒ 模块从未初始化 ⇒ `register_universe` 写进未初始化的 `UNIVERSES` ⇒ 注册丢失 | ① resolver 记 `原始 spec → 解析后模块名`；② MIR 先归一，再用 `py_user_modules` **或** `func_ret_types` 里有 `<mod>__init` 判定 |
| 4 | 循环 import 期间目标模块被当作外部 shim | `py_user_modules` 只在**加载成功后**登记，而 `register` 递归处理 import ⇒ 循环期间 `is_user=false` ⇒ 成员引用全变 "unknown member — external shim" ⇒ 裸符号（**实测该告警 261 → 89**） | `load_user_python_module` 一找到文件就登记为 user module |

附带：re-exports 表只对**真实模块**生效（`pd.Timestamp.date` 这类点号属性路径不参与重命名，
否则产生 `_pd.Timestamp__date` 幽灵）；「按类名匹配所属模块」的 rename 回退**只取 own names**
（取 re-exports 会产生新幽灵）。

### 度量

| 口径 | before | after |
|---|---|---|
| 语料解析 | 38/38 | **42/42** ⬆ |
| 官方 | 194/194 | 194/194 |
| python_style | 271/274 | 271/274 |
| driver "unknown member" 告警 | 261 | **89** |

### 运行状态（lldb，no-opt 构建）

```
[INFO] …: 获取数据...
PY-A: `__to_ts` is NOT implemented in this build …      ← 已越过 get_universe ✓
```

⇒ **宇宙注册问题已解决**，程序进入数据层。剩下的 `_to_ts` 属「方法体内调用同模块私有函数」
（`module_renames_for` 对 `Class::method` 拿不到模块）——已定位到修法，但需要先把
re-exports 表做可靠（本批已缩小到 own-names-only）。

### 下一队列（批次一百六十）

1. **`_to_ts` 一族**：方法体的 rename 表（own-names-only 已就绪，需确认不再产生
   `_pd.Timestamp__date`/`_filter` 这两个幽灵后再打开）
2. `_filter` / `_pd.Timestamp__date` 的来源（数据层 pandas/numpy 面）
3. `py_pd_read_parquet` 真实实现（2636 parquet / 669MB）——**跑出指标的最后一块**
4. -O3 误编译 longjmp 路径（`ZETA_NO_OPT=1` 可对照；O3 下 trap 在
   `market_data__init` 的 try/except）

---

## 批次一百六十/一百六十一（2026-09-19）：引号前缀归一 + `cast` 空操作（语料 **38/38 → 45/45**）

### 批次 160：「同一文件两个模块名」的**引用侧**归一（driver **链接成功**）

```text
T _jq_shim__get_cost_config                  ← 定义（先加载的拼写）
U _strategies_code_jq_shim__get_cost_config  ← 引用（22 处）
```

`jq_shim.py` 既可裸名（策略目录在 sys.path 上）又可点号（`strategies.code.jq_shim`）导入，
定义落在**先加载**的拼写下；引用侧有三处建 `<module>__<member>` 的地方没归一（其中
「用户模块的默认参数填充」路径是主犯）⇒ 第二个前缀永远 undefined。
**修法**：三处（`py_member_call` / 默认参数填充 / `from <mod> import <VAR>` 的 env 读）
都先过一遍 `py_module_aliases`（resolver 在同一文件第二次加载时写 `module → canon`）。
⇒ driver **rc=0（622 KB）** ✓

### 批次 161：`typing.cast(T, v)` 必须是**类型不变**的空操作

MIR 铁证：

```text
call py_dt_sub_delta args [14, 16] -> 22    # 22: Named("PyDate") ✓
call py_typing_cast  args [10, 22] -> 9     # 9: **I64** ✗ ← 元凶
call strftime_2      args [23, 26] -> 24    # 在 I64 接收者上分派 ✗
```

注册表 `F typing cast py_typing_cast args=i64,i64 ret=i64` 把值重新标成 I64 ⇒
`cast(pd.Timestamp, ts).strftime(...)` 把 PyDate 句柄当整数 ⇒ `strftime_l` 崩
（实测 `warmup_start_of` 的栈）。修法：MIR 里 `cast(T,v)` 直接降低 `v` 并**保留其类型**
（Python 语义：cast 只影响静态类型）。判定用 `py_member_target(receiver,"cast")`，
同时覆盖 `typing.cast` 与裸名。

### 度量

| 口径 | 起点 | 现在 |
|---|---|---|
| 语料解析 | 38/38 | **45/45** ⬆ |
| 官方 | 194/194 | 194/194 |
| python_style | 271/274 | 271/274 |
| driver | 链接失败 | **链接成功并进入数据层** |

### 运行状态（lldb，no-opt）

崩点持续前移（每一步都已修）：
`get_universe`（universe 注册）→ `fetch_stocks` / `_to_ts`（模块私有函数 rename）→
**`MarketDataFetcher.__init__`**（`py_os_path_join` 读到坏指针 = `_PROJECT_ROOT / "data"`）。

### 下一队列（批次一百六十二）

1. `MarketDataFetcher.__init__` 的 `_PROJECT_ROOT`（由 `market_data_sources` **跨模块 import**
   进来的私有模块全局）—— 值坏 ⇒ `py_os_path_join` 崩
2. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一块
3. -O3 longjmp 误编译（`ZETA_NO_OPT=1` 可对照）
4. 顺带记一个小 parser 坑：把函数命名为 `use` 会与 `use` 语句冲突（`from mdf import use` +
   `print(use())` 触发 W1002 丢尾部）

### 批次一百六十二（2026-09-19）：`str(<PyPath>)` 的类型（语料 **45/45 → 46/46**）

`MarketDataFetcher.__init__` 里 `str(_PROJECT_ROOT / "data")` 是 `lower_to_string` 的
**直通**（PyPath 句柄本身就是路径字符串），但直通**没改类型** ⇒ 结果仍是 `Named("PyPath")`
⇒ 上层 `or` 的取值类型与 `os.path.join` 的分派按 i64 走 ⇒ 把句柄当数字解引用
（lldb：`ldr x0,[x8]`，`x8 = NULL`）。

修法只重标**结果 id** 为 `Str`：就地改源 id 会污染原变量的后续 `.parent/.exists()` 分派；
经新 id 转手又没有 alloca（t73 立刻 SEGV，已实测回滚）⇒ **只改结果 id 的类型**。

### 批次 162 后的运行状态（lldb，no-opt）

崩点继续前移：`get_universe` → `fetch_stocks`/`_to_ts` → `MarketDataFetcher.__init__` →
**`run_backtest` 的第一行日志** `logger.info("获取数据...")`：

```
frame #5: drv24`py_logger_info + 108          ← fprintf(stderr, "[INFO] %s: %s")
frame #6: strategies_code_jq_wufu_local__run_backtest + 384
frame #0: _platform_strlen（坏指针）
```

即 logger 的**名字**是坏指针 —— `logging.getLogger(__name__)` 里的 `__name__` 被解析成了
一个**符号名**（早前日志里出现过 `[INFO] backend_strategy_wufu_constants__DEFENSIVE_ETF_JQ:`
这种前缀），而正确值是**模块名字符串**。⇒ 下一批：`__name__` 的取值范围。

### 下一队列（批次一百六十三）

1. `__name__`（应为模块名字符串）—— 当前是符号名 ⇒ logger 名坏指针
2. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一块
3. -O3 longjmp 误编译（`ZETA_NO_OPT=1` 可对照）

### 批次一百六十三（2026-09-19）：`__name__` + 平台源响亮回退 + 句柄运算数物化

| # | 缺陷 | 铁证 | 修法 |
|---|---|---|---|
| 1 | `__name__` **完全没有实现** | `print(__name__)` → **1**；`getLogger(__name__)` 拿坏指针 ⇒ `run_backtest` 首行日志在 `fprintf/strlen` 崩 | MirGen 增 `current_module`（resolver 用 `py_mangled_to_module[fn]` 填，根文件 `"__main__"`），`Var("__name__")` 降为该模块名字符串，随闭包传播。实测 `[WARNING] nm: hello` ✓ |
| 2 | 平台桩 `abort()` 把本地回测停在第一次平台调用 | `_auth` 立刻 abort | 平台族（jqdatasdk/rqdatac/tushare/akshare/pyarrow 读表）改成**打一行说明 + `zeta_raise(1)`** —— 与 CPython 同形，项目自己的 try/except 走本地回退。（先试「返回 0」会把 `auth` 伪装成**成功**："jqdatasdk authenticated successfully" 之后拿 0 句柄再崩 ✗） |
| 3 | 句柄运算符操作数不物化 ⇒ codegen `load_local` 取 NULL | `self.cache_dir / "x"` → `py_os_path_join` 解引用崩（`MarketDataFetcher.__init__+68`） | `handle_op`（`/`、日期加减）两操作数先 `materialize_for_call`（与 map 下标路径同规则） |

**运行状态（lldb，no-opt，本会话最远）**：

```
[INFO] strategies.code.jq_wufu_local: 获取数据...
PY-A: platform source `_auth` is not available — falling back
[WARNING] backend.market_data: jqdatasdk auth failed: 1        ← 项目自己的回退生效 ✓
frame #0: _LogAdapter::info + 44                               ← 闭包内的用户类方法分派
frame #2: MarketDataFetcher::fetch_stocks + 964                ← 已在数据层内部 ✓
```

⇒ **模块初始化 / 宇宙注册 / 符号解析 / 构造器 / 首行日志 / 平台回退** 全通。

### 下一队列（批次一百六十四）

1. 闭包内的用户类方法分派（`_LogAdapter::info`，崩在 +44）
2. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环
3. -O3 longjmp 误编译（`ZETA_NO_OPT=1` 可对照）

### 批次一百六十四（2026-09-19）：`log.info(fmt, *args)` 空 varargs 解引用

lldb：`_LogAdapter::info` 里 `ldr x8,[sp,#0x38]`（args 参数=**0**，本次调用无额外实参）
→ `ldr x3,[x8]` **EXC_BAD_ACCESS address=0x0**。原因：日志变参路径对 `*args`
直接 `lower_expr` ⇒ 降成 `MirExpr::Deref` ⇒ codegen 发**裸 load**，而变参句柄可能为空。

修法：遇到 starred 操作数**跳过 + 编译期 eprintln 说明**（不硬展开、不解引用、不静默），
`n_lit` 按**实际展开元素数**计数（原来用 `args.len()-1`，会把跳过项也算进去）。

**运行状态（本会话最远）**：

```
[INFO] strategies.code.jq_wufu_local: 获取数据...
PY-A: platform source `_auth` is not available — falling back
[WARNING] backend.market_data: jqdatasdk auth failed: 1
[INFO] jq_shim: 行情请求 0 只，区间 1969-08-04 ~     ← 日志链贯通
frame #0: GC_generic_malloc_many                     ← py_json_loads 解析崩
frame #5: backend_datasrc_etf_listing___listing_dates_cached + 128
```

### 下一队列（批次一百六十五）

1. `py_json_loads` 输入指针（etf_listing 缓存 JSON）——先确认传入的是否合法字符串
2. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环
3. -O3 longjmp 误编译（`ZETA_NO_OPT=1` 可对照）

### 批次一百六十五（2026-09-19）：dict 扩容堆破坏（重大）+ JSON `.items()/.values()`

**根因（越界写）**：`map_insert` 扩容时 `memcpy(map, nb, 16+nc*MAP_ENTRY_SIZE)` ——
把「大的新表」拷进「小的旧块」（旧块只有 `16+cap*ENTRY`）⇒ 越界写 2 倍、踩坏 GC 元数据。

铁证（任何 ≥13 键的 dict）：

```
GC Warning: Failed to expand heap by 12143674558099984 KiB
GC Warning: Out of Memory! Heap size: 0 MiB. Returning NULL!
Segmentation fault
```

`data/universe/etf_listing.json`（50 KB / 1724 键）正是死在这里；25 键时 `len(d)` 还会**静默返回 0**。

**修法**：句柄按值传 ⇒ 扩容不能让块搬家 ⇒ 旧块变**转发块**（`word0 = MAP_MOVED(-1)`、
`word1 = 新地址`，正好是它自己的 16 字节头），所有读表函数先 `map_resolve()`；
且必须**跟完整条链**（只跟一跳会让 `map_insert` 落在 cap=-1 的块上，
`idx = hash & (cap-1)` 越界 ⇒ 二次破坏 —— 首修后 25 键 len 仍 0 即此）。

改动点：stub 的 `map_insert`/`map_get`/`py_json_len`/`py_json_dumps_map`/`zeta_map_len`/
`py_map_contains`/`py_json_keys`/`py_json_values`；py_additions.c 的 `map_keys`/`map_values`/
`zt_map_most_common`/`map_get_default`/`py_map_items`/`py_map_update`/`zeta_map_update`/
`zeta_map_pop*`/`zeta_map_clear`。

**JSON 对象方法**：`.items()`/`.values()` 在注册表里没有 PyJson 条目 ⇒ arity-mangle 成桩返回 0
⇒ `{str(k): str(v) for k, v in data.items()}` 静默得 `{}`。新增 `py_json_as_map` 复用 dict 助手；
`.values()` 元素类型保持 `PyJson`（否则打印句柄数字 —— t57 回归，已修回）。

**实测**：13/25/60/200 键 dict 全对；etf_listing.json → **1724 键**、推导式 → **1724** ✓。

### 下一队列（批次一百六十六）

1. `MarketDataFetcher.__init__ + 92` 的第二次 `os.path.join(self.cache_dir, "stocks")`
   —— 「无 alloca 的调用实参」家族（`py_os_path_join + 44`）
2. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环
3. -O3 longjmp 误编译（`ZETA_NO_OPT=1` 可对照）

### 批次一百六十六（2026-09-19）：构造器内 `self.<字段>` 读取

MIR 铁证：`MarketDataFetcher.__init__` 里 `16: FieldAccess{base:18,"cache_dir"}` 而
`18: Var(18)` **从未被赋值**（`param_indices` 只有 `cache_dir`）——
解析器合成构造器时丢掉了 `self`，`self.x = v` 只进「字段初始化表」，
**后一行读** `self.x` 落到无 alloca 的槽 ⇒ codegen 取 NULL ⇒
`py_os_path_join` 解引用崩（`MarketDataFetcher.__init__+92`）。

修法：`StructLit` 按源码顺序登记 `self_field_aliases`；`FieldAccess` 最前面查别名；
**护栏**：仅当 `self` 未绑定时启用（真 `self` 方法 / Rust 风格字面量不变）；嵌套用长度截断。

### 下一队列（批次一百六十七）

1. `_listing_dates_cached + 524`（缓存读取 / `{str(k): str(v) for k,v in data.items()}`）
2. 结构体字段读取的**静态类型**（`c.cache_dir` 现在按 i64 分派 ⇒ `len()` 得 0）
3. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环

### 批次一百六十七（2026-09-19）：跨模块模块级全局的类型

`FieldAccess` 的「用户模块命名空间读」分支把结果类型**硬编码 I64** ⇒ 句柄标签丢失，
`import a; a.C.exists()` 变成裸符号 `_a__C.exists`（链接失败）；而
`from a import C` 走绑定路径，一直是对的。

修法：新增 `MirGen::global_ty_of()`（原名 → 已知模块前缀剥离 → `rfind("__")` 兜底；
`_PROJECT_ROOT` 是三个下划线，按 `__` split 会丢前导下划线 —— 注释里记过这个坑）；
两处改用该助手；`py_member_call`/`py_member_target` 在注册表未命中时，若点号前缀是
**有类型的全局**则返回 `None`，让调用方把 `a.C` 当**值**降级并按句柄标签分派。

实测：三种写法（直接 / from-import / 别名）全部链接 ✓ 且运行 ✓。

**遗留（下一批）**：`module_global_types()` 的**计算时机** —— 根文件 `main` 在 import 之前
就被降级（探针：`globals=1 prefixes=[]`，应为数百）⇒ 根文件读跨模块全局会退化成
「no registry entry and is not a value — lowering it as 0」的响亮告警。
项目自身路径（模块内读自己的全局）不受影响。

### 下一队列（批次一百六十八）

1. `_listing_dates_cached + 524`（现场 `x0=0`、`x2=-48`；日志显示已走进 jqdata 分支，
   说明**缓存分支没有提前返回**）
2. `module_global_types()` 的时机（根文件 main 先于 import 降级）
3. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环

### 批次一百六十八（2026-09-19）：`isinstance(<json>, dict)` + 元组解构类型（语料 **46/46 → 48/48**）

| # | 缺陷 | 铁证 | 修法 |
|---|---|---|---|
| 1 | `isinstance(parsed_json, dict)` 恒 0（**静默错值**） | 隔离程序：`len(data)=1724` 但 `isinstance(data,dict)=0` ⇒ `_listing_dates_cached` 丢弃已解析的缓存、走平台分支并崩 | `PyJson` 走**运行期**标签分派 `py_json_is_kind(v,tag)`（ZJ_INT 1/F64 2/STR 3/ARR 4/OBJ 5） |
| 2 | 元组解构元素硬编码 i64 | `num, exch = jq.split(".")` 值对但类型 i64 ⇒ `num.startswith(("000","399"))` 对整数做句柄分派 ⇒ SEGV（`_is_likely_index+56`） | 按源值类型取元素类型（DynamicArray/Array → e，Tuple → ts[i]，Str → Str） |

**运行状态**：`etf_listing` 缓存分支打通（日志出现「缓存命中 0 只；待拉取 0 只」），
崩点前移到 `_source_score` 的 `stats.get(source, {...})` —— 裸 `get` 幽灵符号（裸 `get` 在
隔离复现里表现为**挂死**）。

### 下一队列（批次一百六十九）

1. `dict.get` 的类型来源：`-> dict[str, dict[str,int]]` 返回 `json.loads` 的函数，
   `.get()` 既不走 map 分支也不走 PyJson 分支 ⇒ 裸 `get`
2. `str.startswith(<tuple>)`（Python 支持元组前缀，实测返回 0）
3. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环

### 批次一百六十九（2026-09-19）：`-> dict` + `json.loads` 的返回类型 + map 原语护栏

隔离复现：

```python
def f() -> dict:
    return json.loads('{"a": 1}')
d = f(); e = d.get("a", 0)      # ⇒ 挂死（lldb: map_get_default+112 自旋）
```

`-> dict[...]` 把返回值定型 `map`，运行期却是 PyJson 单元 ⇒ `map_get_default` 把 **tag 当容量**
读 ⇒ `idx = hash & (cap-1)` 死循环。

- **真修**：`lower_to_mir` 里「声明 `dict` + 函数体 `return json.loads(...)`」⇒ 返回类型取 `PyJson`
  （`.get(k,default)`/`len()`/`.items()` 路径已存在）。实测 `1` ✓。
- **护栏**：map 原语见句柄首字 1..8（Json tag；map 容量恒 ≥16）⇒ 打印诊断 + abort
  （不静默重解释、不挂死）。

**运行状态**：数据源选择/回退循环跑起来了（含 `[source-priority]`、`[baostock]` 日志），
但**请求的股票集合是 0 只**，循环若干轮后挂死。

### 下一队列（批次一百七十）

1. 「行情请求 0 只」——universe/股票集合为空（数据层入口）
2. 循环后的挂死栈
3. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环
