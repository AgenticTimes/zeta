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

### 批次 169 后的诊断（「行情请求 0 只」根因）

隔离探针（导入真模块）：

```
wufu n 0            ← get_universe("wufu") 空
keys 2              ← UNIVERSES 里确实有 2 个条目
k 4307507541 / k 4307496678    ← 键是**数字**（裸句柄），不是内容哈希
```

⇒ `market_data_universe.UNIVERSES[name] = codes`（在 `register_universe` 内）写入时
`UNIVERSES` 被判成 **I64**（不是 `map`）⇒ 键未内容哈希 ⇒ `name in UNIVERSES` 查不到
⇒ 股票池为空 ⇒ 全链路「0 只」。

最小复现（同文件 / 跨模块 / 带 `dict[str, list[str]]` 注解）都**正常**（`n=2`、`bin=1`），
说明这是**批次 167 遗留的时机问题**在同一项目规模下暴露：
`module_global_types()` 探针显示 `globals=1 prefixes=[]`（应为数百），
即 mdu 的函数在被降级时，它自己的模块全局名集合还没登记完。

⇒ 批次 170 的第一个任务：修 `module_globals` / `module_global_types` 的**登记时机**
（把 Python 模块的加载与登记挪到「降级之前」），修好后 `UNIVERSES` 恢复 `map`，
键走内容哈希，股票池非空。

### 批次一百七十（2026-09-19）：跨模块模块级列表的 `in`（语料 **48/48 → 50/50**）

链接期铁证：`Undefined symbols: _backend_strategy_wufu_constants__WUFU_INDEX_BS_CODES.__contains__`
—— `x not in mod.LIST`（跨模块模块级 List，值走 env 读、`type_map` 无条目）⇒ `receiver_ty=I64`
⇒ 成员判定落到「唯一 `__contains__` 定义」兜底 ⇒ 发出不存在的符号。

修法：`__contains__` 分支内用 `receiver_global_key()`（`Var(n)`→n；`mod.NAME`→
`<module with . as _>__NAME`）+ `global_ty_of()` **就地**恢复容器类型。

**只在本分支生效**（第一次改动通用 `receiver_ty` ⇒ `df.sort_values(...)` 变
`_map__sort_values` 幽灵，已回退并收窄）。

**driver 现状**：链接 ✓、universe 非空（探针 579/827），仍在 `run_backtest` 开头几条语句
（universe 过滤之后、`fetch_stocks` 之前）崩溃，输出被崩溃吞掉。

### 下一队列（批次一百七十一）

1. `run_backtest` 开头段逐句 `flush()` 二分（universe 过滤 → `MarketDataFetcher()` →
   `warmup_start_of` → `fetch_stocks`）
2. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环

### 批次一百七十一/一百七十二（2026-09-19）：列表推导式常量类型 ⇒ 消灭非确定性段错误

**症状**：driver 每次运行 ~50% SIGSEGV；`MallocScribble=1` 100%；lldb（关 ASLR）不崩但给错值。
崩溃报告落在 `py_map_fromkeys` ← `wufu_constants._register_into_datasrc`。

**根因**：`WUFU_BS_CODES = [jq_to_bs(c) for c in WUFU_JQ_CODES]` 在 AST 里是
`Call{receiver: Some(Var("…JQ_CODES")), method: "__collect__", args: [Closure{…}]}`，
`infer_global_ty` 无此分支 ⇒ 常量无类型 ⇒ I64 ⇒ `LIST + LIST` 走 `SemiringFold`（数值加）
⇒ `dict.fromkeys(<垃圾整数>)` ⇒ `py_map_fromkeys` 野读。

**修法**：`infer_global_ty` 增 `__collect__`：元素类型取闭包体/迭代对象的类型，
**推不出也返回 `DynamicArray`**（推导式永远是列表）。另：`module_global_types` 把 `funcs` 的
**返回类型表**穿透给 `infer_global_ty`，Call 分支先查用户函数返回类型。

**实测**（`MallocScribble=1`，4/4 稳定）：`UNIVERSES=2`、`BS=115`、`IDX=4`、
**`get_universe("wufu")=119`** ✓；driver 日志由「行情请求 **0** 只」变为「**119** 只」。

### 下一队列（批次一百七十三）

1. 区间显示 `1969-08-04`：日期参数被丢成 0/epoch
2. rqdatac 回退之后的崩溃栈
3. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环

### 批次一百七十三/一百七十四（2026-09-19）：推导式元素类型 + f-string 内联条件

| # | 缺陷 | 铁证 | 修法 |
|---|---|---|---|
| 173 | 推导式元素类型停在 I64 | `INFER WUFU_BS_CODES: DynamicArray(I64)` | `infer_global_ty` 的 Call 分支先查用户函数返回类型（裸名 → `__<name>` 后缀匹配，多个候选**类型一致**时采用）⇒ `DynamicArray(Str)` |
| 174 | f-string 内联条件表达式片段按 i64 串化 | `jq_to_bs("510300.XSHG")` = `4339988730.510300`（17 字符，**整个 wufu 池 119 个代码全是垃圾**） | f-string 片段若类型为 I64/PyDynamic 且 AST 是「两分支都是字符串字面量」的条件 ⇒ 先标 `Str` 再 `lower_to_string` |

实测：`jq_to_bs("510300.XSHG")` → **`sh.510300`** ✓（9 字符）。

**运行状态**：数据层已经拿着正确的 **119 只**跑源选择/回退循环。

### 下一队列（批次一百七十五）

1. `区间 1969-08-04 ~ `：`fetch_stocks` 内日期 f-string 仍异常（同一类问题？）
2. 源循环之后的崩溃栈
3. `py_pd_read_parquet` 真实实现（2636 parquet / 669 MB）——跑出指标的最后一环

### 批次一百七十五（2026-09-19）：裸日志方法符号 + 语料口径修正

`fetch_stocks` 内 `_log` 闭包的 `logger.info(msg)`：`logger` 是**跨模块再导出**的模块全局
（`data_ops_log.logger` → `market_data_sources.logger` → `market_data_fetcher.logger`），
闭包拿不到静态类型 ⇒ 退化成**裸方法名 `info`** ⇒ 撞 abort 桩 ⇒ 本地回测第一次写日志就停机。

lldb：`info` ← `__closure_0` ← `MarketDataFetcher::fetch_stocks + 956`。

修法：运行期补上 `info`/`warning`/`error`/`debug`/`critical` 裸符号（走无 logger 名的路径，
消息照常打印）。

**语料口径修正**：`tools/corpus_baseline.py` 扫 `REasyQuant/strategies`，
我放在 `strategies/code/` 的临时 harness 被计入 ⇒ 数字被抬高。清掉 11 个废弃 harness 后：

```
语料: 38 文件 / 38/38 = 100%      ← 项目真实语料
官方 194/194、python_style 271/274 不变
```

**下一处崩点**：`py_list_contains + 28` ← `__closure_48` ← `zeta_collect_vec_n + 112`
← `MarketDataFetcher::fetch_stocks + 3380`（`fetch_stocks` 里的推导式 `in`，
类型已知但运行期句柄是野值）。

### 下一队列（批次一百七十六）

1. `fetch_stocks` 推导式里 `in` 的野句柄（Scribble 下 100%）
2. `区间 1969-08-04 ~ `：日期显示异常
3. `py_pd_read_parquet` 真实实现——跑出指标的最后一环

### 批次一百七十六（2026-09-19）：空列表 `append` 的元素类型 + 裸日志符号 weak

| # | 缺陷 | 铁证 | 修法 |
|---|---|---|---|
| 1 | `lst = []` + `lst.append(x)` 元素类型停在 I64（=未知） | `x in lst` 传 `elem_is_str=0`，`py_list_contains` 比**句柄地址** ⇒ 字符串永远「不在」⇒ `h(["a","b","c"])` 返回 3 ✗（应 2） | `vec_push` 落点：接收者为 `DynamicArray(I64)` 且追加值类型 ≠ I64 ⇒ 细化槽位类型（单向 I64→具体） |
| 2 | 上一批的裸日志符号与程序自身定义冲突 | `duplicate symbol '_warning'/_error'/_info'`（REasyQuant 的 `_LogAdapter` 方法就落成裸名） | 运行期那几个符号改 `__attribute__((weak))`，程序自身定义优先 |

**连带收益**：python_style **271 → 272 passed / 2 failed**（`t207_empty_append_str` 转通过）。

**下一处崩点**：

```
py_list_contains + 28 ← __closure_92 ← zeta_collect_vec_n + 112 ← MarketDataFetcher::fetch_stocks + 3360
```

即 `fetch_stocks` 里 `[c for c in to_fetch if self._normalize_stock_code(c) not in fetched_codes]`
这类推导式的 `fetched_codes`（函数局部，类型仍未知）⇒ `vec` 句柄野值。

### 下一队列（批次一百七十七）

1. `push`/`+` 之外的列表来源也要细化元素类型；`in` 的 `elem_is_str==0` 走**运行期内容比较兜底**
2. `py_list_contains` 入口对 0/不可用句柄做守卫（响亮诊断，不野读）
3. `区间 1969-08-04 ~ `：日期显示异常
4. `py_pd_read_parquet` 真实实现——跑出指标的最后一环

### 批次一百七十七（2026-09-19）：`set |= {...}` 的按位或

`|=` 脱糖 → `x = x | y`，两个「集合」（V1 降级为 DynamicArray）走了**按位或** ⇒ 垃圾句柄
⇒ 集合恒空 ⇒ `c not in fetched_codes` 恒真 ⇒ `fetch_stocks` 带着整池继续 ⇒ 最后在
`py_list_contains` 野读崩溃。

修法：`op == "|"` 且任一侧是 array（或 `Named("set")`）⇒ `py_array_concat`；
`py_list_contains` 在 `elem_is_str == 0` 时用 `GC_base()` 证明是 GC 对象后按内容比较
（不解引用，安全；`ZT_DEBUG_CONTAINS=1` 诊断）。

**仍未收敛**：函数参数/局部集合的并集与「参数列表的 `in`」仍错（模块级字面量正确）；
且 `py_list_contains` 新分支未被执行到 ⇒ 下一批先确认链接的运行期是否最新
（`cargo build` 让内嵌的 `zeta_runtime_c.o` 同步）。

### 批次一百七十八（2026-09-19）：容器注解参与元素类型

`TypeAnnotatedPattern` 丢弃注解（`ty: _`）⇒ `c: set[str] = set()` 的元素类型停在「未知」
⇒ `"q" in c` 传 `elem_is_str=0` ⇒ 比句柄 ⇒ 字符串恒「不在」
（`fetch_stocks` 的 `fetched_codes` 链）。

修法：保留注解，新增 `annotation_elem_ty`（`list[str]`/`set[str]`/`frozenset[int]` 等），
RHS 为 `DynamicArray(I64)`/`I64`/`PyDynamic` 时按注解细化。

**未收敛**：最小复现 `c: set[str] = set(); c |= {"q"}; "q" in c` 仍为 0 ——
MIR 显示 `py_builtin_set` 结果仍是 `DynamicArray(I64)`（细化没落到该槽位），
`in` 取到的是细化前的类型。下一批：在 `py_builtin_set` 落点直接用注解。

### 批次一百七十九/一百八十（2026-09-19）：`|=` 类型刷新 + 元组前缀 `startswith`

| # | 缺陷 | 铁证 | 修法 |
|---|---|---|---|
| 179 | `x \|= y` 写回已存在槽位时不刷新静态类型 | `c: set[str] = set(); c \|= {"q"}; "q" in c` → 0 ✗（槽位仍是 `set()` 的 `DynamicArray(I64)`） | `AssignOp` 对 `Var` 目标走专门路径：先降级合并表达式，把非 I64/PyDynamic 的类型写回槽位，再 Assign |
| 180 | `startswith(<tuple>)` 恒 0 | `"000300".startswith(("000","399"))` → 0 ✗ | 元组/数组字面量实参展开成多个单前缀调用并 `\|\|` 合并 |

实测：`f()`→1 ✓、`f(["a","b","c"])` 集合并集→2 ✓、`startswith(("000","399"))`→1 ✓。

**遗留**：项目内 `_is_likely_index("000300.XSHG")` 仍为 0（隔离用例已修）；
崩点 `_is_likely_index + 52`（`EXC_BAD_ACCESS at 0x0`）待下一批单测该函数。

### 批次一百八十一（2026-09-19）：元组前缀 `startswith` 下沉到运行期助手

MIR 里「多个单前缀调用 + `||`」在隔离用例正确、项目里仍返回 0 ⇒ 改为整体下沉：
`py_str_prefix_any(s, vec, is_end)` 一次调用完成任一前缀匹配。

**项目铁证**：`_is_likely_index("000300.XSHG")` → 1 ✓、`("sh.510300")` → 0 ✓、
`("399001.XSHE")` → 1 ✓（此前恒 0 ⇒ 指数被当普通 ETF；`_is_likely_index + 52` 的
`EXC_BAD_ACCESS at 0x0` 一并消失）。

**运行状态**：**已无崩溃**，但 `fetch_stocks` 的源回退循环不收敛（119 → 238 → …，>900s）。
下一批：`to_fetch`/`fetched_codes` 为何不缩减 + `区间 1969-08-04 ~ `。

### 批次一百八十二（2026-09-19）：`bs.login()` 撞 libc `login(3)`

`baostock` 不在 registry ⇒ `bs.login()` 落到裸符号 `login` ⇒ 链接器用 **libc `login(3)`**
（utmpx）满足 ⇒ `EXC_BAD_ACCESS at 0x8`（`getutmpx` ← `login` ← `_baostock_login`）。

修法：stub 里 weak 定义 `login`/`logout`（说明 + `zeta_raise`），平台源失败走调用方 try/except。

**新崩点**：裸成员 `_get`（未类型化接收者的 `.get(...)`）⇒ abort 桩。
**仍未收敛**：`fetch_stocks` 源回退循环 119 → 238 → …；`区间 1969-08-04 ~ `。

### 批次一百八十三（2026-09-19）：裸成员 `get` 的定位（未落地，已回退）

崩点：`PY-A: _get is NOT implemented` → lldb：`get` ← `_source_score + 368`。

MIR：`_source_score` 里 `zeta_env_get(_DEFAULT_SOURCE_SCORE)` → subscript → **`get_3`**（裸 + 元数后缀）。
全局类型表里它是 `Named("map", [])`（**无类型参数**）⇒ 下标后的值类型 I64 ⇒ `.get(...)`
掉出 map 分派 ⇒ 裸 `get`。

试过「`DictLit` 按首条推导 `map<K, V>`」（表里确实变成
`map<Str, map<Str, F64>>` ✓），但**引入回归**：`MarketDataFetcher.__init__` 的
`py_os_path_join` 又以 0x1 崩溃 ✗ ⇒ 已 `git checkout` 回退。
下一批要用更窄的方式（例如只在「值本身是 DictLit」时递归，或在 `infer_global_ty`
里给 DictLit 单独加一个 `map<Str, map<...>>` 变体）重试。

### 下一队列（批次一百八十四）

1. nested dict 的值类型（`_DEFAULT_SOURCE_SCORE[asset].get(...)`）——窄化重试
2. `fetch_stocks` 源回退循环不收敛（119 → 238 → …）
3. `区间 1969-08-04 ~ `：日期显示异常
4. `py_pd_read_parquet` 真实实现——跑出指标的最后一环

### 批次一百八十四（2026-09-19）：`DictLit` 推导 `map<K, V>`（修掉裸 `get`）

`infer_global_ty` 的 `DictLit` 按首条推键/值类型 ⇒
`_DEFAULT_SOURCE_SCORE -> map<Str, map<Str, F64>>` ⇒ `[asset].get(source, 0.5)` 回到 map 路径，
`_get` 未实现桩不再触发。

**并纠正批次 183 的误判**：那时把偶发的 `MarketDataFetcher.__init__` 崩溃当成回归而回退；
本次同产物连跑 3 次全部越过该点（3/3 rc=124 超时）⇒ 那是一次 flake。

**运行状态**：3/3 卡在 `fetch_stocks` 源回退循环（119 → 238 → …）。

### 批次 185 观察（源回退循环不收敛）

driver 日志序列：`行情请求 119 只` → `待拉取 119 只` → `[baostock] 批量拉取 119 只`
→ `待拉取 238 只`（= 2×119）→ `[source-priority] …：238 只`。

探针（同一 driver 语境）：

```
u 119 / f 119 / u2 119        ← get_universe 与过滤结果都是 119，且重复调用不累积
UNIVERSES -> Named("map", [Str, DynamicArray(Str)])   ← 全局类型表正确
```

⇒ `get_universe`/`register_universe` 没有重复注册；问题在 `fetch_stocks` **之后**
（第二次进入的 238 从哪来）。下一批用 lldb 在 `fetch_stocks` 打断点、打印入参长度与调用栈，
确认是「同一函数体被执行两次」还是「调用方传了 238」。

**附带观察**：探针里 `u[i]` 用 **F64 路径**打印成 `4366971587.515880`（应为字符串
`sh.515880`）⇒ `列表下标结果的打印分发` 仍有类型丢失（不影响 `in` 的成员判定：
实测 `u[0] in WUFU_INDEX_BS_CODES` 为 0，与「该码不是指数」一致）。

### 批次一百八十六（2026-09-19）：`login` 强符号（weak 不生效）+ 循环翻倍的精确定位

weak 的 `login` 仍输给 libc（`getutmpx` ← `login(3)` ← `_baostock_login`）⇒ 改**强定义**，
崩溃消失。

**精确定位**：第二次「待拉取 238 只」**没有**配套的「行情请求 …」⇒ 同一次
`fetch_stocks` 调用内，缓存扫描 for 循环的**循环体执行了两遍**（`to_fetch` 翻倍）。
下一批：把该循环（含 `try/except` + `continue`）做成最小复现。

### 批次 187 结论（关键路径重排）

排查「源回退循环不收敛」时确认：**每条路径都回到同一个根** ——
`MarketDataFetcher._load_cache()` → `_parquet_cache.load()` → `pd.read_parquet()`
→ 运行期 `py_pd_read_parquet` 仍是 `return 0` 的桩 ⇒ `cached = None`
⇒ 每个代码都进 `to_fetch` ⇒ 触发平台/回退链（本实现里平台不可用）⇒ 循环。

已排除（最小复现均正常）：

| 形状 | 结果 |
|---|---|
| `try/except` + `continue` 的 for 循环 | ✓ 不多跑 |
| `try` 块之后接 for 循环 | ✓ 不多跑 |
| 注解空列表 `out: list[str] = []` 与形参别名 | ✓ 不增长 |
| 缓存扫描循环的简化复刻 | ✓ `n 3 f 3` 一次 |

⇒ **关键路径就是 `py_pd_read_parquet`（真实 parquet 读取）**：它一旦可用，
缓存命中 ⇒ `to_fetch` 收敛 ⇒ 直接进入回测（本地数据 2636 个 parquet / 669 MB）。
随后仍待处理：`区间 1969-08-04 ~ `、列表下标的打印分发。

### 下一队列（批次 188）

1. **`py_pd_read_parquet` 真实实现**（先支持本项目缓存表的 schema：
   `trade_date/stock_code/open/high/low/close/volume/amount`，产出「列名 → 向量」的 DataFrame 表示）
2. 源回退循环收敛验证（有了缓存命中后）
3. `区间 1969-08-04 ~ ` 日期显示；列表下标打印分发

### 批次 193/194（2026-09-20）：parquet 完全可用 + 缓存路径的定位

**已完成**：`pd.read_parquet` 在 zeta 程序里完全可用（9 列 × 892 行，值与 pyarrow 一致）：

```
nk 9 / k close…stock_code / hasdate 1 / cols 9 / rows 892 / d0 2022-05-05
```

（修法是给 **MIR 调用点**的 `match ret` 也加上 `"map"` 分支；上一批只改了解析器那处。）

**缓存仍 0 命中的定位**（探针）：

```
path `gv/stocks/000300_XSHG.parquet     ← 前缀是 2 字节垃圾（应为 /Users/.../data）
exists 0 / loaded 0
root /Users/meetai/source/quant/REasyQuant   ✓（模块内部算出来的 _PROJECT_ROOT 正确）
f.cache_dir → 4370300384   ← **字段读取按 i64 分派**（值是句柄，类型丢失）
mf.__file__ → len 1        ← 跨模块读模块属性 `__file__` 得到垃圾
```

⇒ 下一批三条并进：
1. **结构体字段的静态类型**（`self.cache_dir: str` 读出后应仍是 Str；现在按 i64 打印/分派）
2. `f"{safe}.parquet"` / `os.path.join(...)` 在 `_cache_path` 里产生垃圾前缀的原因
3. 跨模块 `mod.__file__`

### 批次 195 补充定位（最小复现 /tmp/mm12）

给 `pkg/liby.py` 的 init 加一句打印后：

```
liby init         ← 模块 init **确实执行了**（V 已写入 env）
（随后 Trace/BPT trap，rc=133，show() 的打印没有出现）
```

⇒ 不是 init 顺序、也不是 env 键问题（MIR 里读写键都是 `pkg_liby__V`）。
陷阱发生在 `pkg.user.show()` 进入处附近（`EXC_BREAKPOINT`，非 `zt_unavailable` 的 abort），
下一批：反汇编 `pkg_user__show` 的头几条指令 + 核对 `import pkg.user; pkg.user.show()`
是否被解析成了别的符号。

### 批次 196/197（2026-09-20）：`_cache_path` 垃圾前缀已缩到最小

探针（真实 `MarketDataFetcher`）：

```
cd_eq 1                                  ← f.cache_dir 与正确路径**相等**
p `gv/stocks/000300_XSHG.parquet         ← ✗ 仍是 2 字节垃圾前缀
plen 32 / exists 0
```

⇒ `cache_dir` 本身正确，但 ctor 里那句 `self.stock_cache_dir = os.path.join(self.cache_dir,
"stocks")` 产出了垃圾。

**两字段的最小复现（/tmp/fld3.z）却是对的**：

```
eq_cd 1 / eq_scd 1        ← 值全对（只是 print 走了 i64 路径）
```

⇒ 与字段**数量/顺序**有关（真实类字段更多）。另外确认：结构体字段读取的**静态类型是 i64**
（`len(f.cache_dir)` 得 0、`print` 出数字），值是对的。

下一批：把 `MarketDataFetcher.__init__` 的字段按顺序逐个加进最小复现，定位是第几个字段
开始串位（或直接看 ctor 的 MIR 里 `Struct` 的字段顺序与 `FieldAccess` 的索引）。

### 批次 202 定位（元组字面量的真实根因，**暂不回填**）

`ParquetCache.load` 崩溃的地址给了决定性线索：

```
EXC_BAD_ACCESS at 0x61645f6564617274      = "trade_date" 的 8 个字节（小端）
  py_list_contains + 152（strcmp）
```

源码是 `date_cols: tuple[str, ...] = ("trade_date",)`，而**解析器把 `(x,)` 当普通括号表达式**
（`parse_tuple_or_paren` 里 `opt(tag(","))` 吃掉了尾逗号却不记录）⇒ `date_cols` 退化成**字符串本身**：

```
t = ("trade_date",)
t[0]      → "t"          （读的是字符串首字符）
len(t)    → 10           （是字符串长度）
for x in t → 逐字符迭代
"trade_date" in t → 1    （子串匹配，恰好为真）
```

于是 `for col in date_cols:` 里 `col` 是**字符**，`array_get` 把字符串前 8 字节当成整数
传进 `strcmp` ⇒ 正是那个崩溃地址。

**试过并已回退**：① 让解析器保留尾逗号（`(x,)` → `Tuple`）；② 元组元素物化进槽位。
两个改动本身语义正确，但会让**单元素元组的表示**暴露出来（`(x,)[0]` 得到 0、
`x in (y,)` 比较句柄），官方 194→193、python_style 272→271（t62_thread_args）、
语料 39→36 ⇒ **必须先把元组的运行时表示（元素读取 / 成员判定 / 迭代）修对**，
再回填这两处。已 `git checkout` 回退，基线复测 194/194、272/274、39/39 全绿。

### 下一队列（批次 203）

1. **元组表示**：StackArray 的元素读取（`t[0]`）、`len(t)`、`for x in t`、`x in t`
   —— 修对之后再回填「尾逗号 → Tuple」与「元素物化」
2. 之后：`ParquetCache.load` 走通 ⇒ 缓存命中 ⇒ `to_fetch` 收敛 ⇒ 进入回测

### 批次 203 调查（单元素元组的修复面比预想大，**本轮全部回退**）

把「`(x,)` → Tuple」（解析器保留尾逗号）落地后，逐个修被暴露的缺口：

| 缺口 | 现象 | 修法 | 结果 |
|---|---|---|---|
| 元组下标走 DictGet | `("a",)[0]` → 0 | 下标降级加 `Type::Tuple` 分支 → `stack_array_get` | ✓ `f 1 / g 1 / h 1`（len/下标/迭代 全对） |
| Thread 单元素 args | `Thread(work, args=(41,))` → 输出句柄 4312338417 ✗ | `_2` 只需一个参数 ⇒ 单元素传元素本身 | 未生效（该 kwarg 走的不是那条分支） |

但**连带回归**（必须同时修好才算数）：

- 官方 194 → **193**
- python_style 272 → **270**（`t35_comprehensions2`、`t62_thread_args`）
- 语料 39 → **36**

⇒ 单元素元组一旦变成真元组，会牵动：`*args`/`kwargs` 展开、推导式的打包/解包、
以及语料里若干 `(x,)` 用法。这是一块**需要专门一轮**的表示层工作（元组元素读取、
迭代、成员判定、与 args 的互操作），不能顺手带过。

**已全部回退**，基线复测：官方 194/194、python_style **272**/2、语料 **39/39** ✓。
（保留的结论：崩溃地址 `0x61645f6564617274` = "trade_date" 的 8 字节，根因确定无误。）

### 批次 204 结果（单元素元组修好，基线全绿）

| 口径 | 现在 |
|---|---|
| 官方 | **194/194** |
| python_style | **272 passed / 2 failed** |
| 语料解析 | **39/39 = 100%** |

四步：① 解析器保留尾逗号（并修正空元组 `()` 的 panic：只有「恰好 1 项且无尾逗号」才解括号）；
② 元组下标加 `Type::Tuple` 分支走 `stack_array_get`；③ 新增 `tuple_slots` 区分真元组
（dict 推导式结果会漏出 Tuple 注解）；④ 运行期 `py_threading_thread_new_2` 对「长度恰为 1
的运行时向量」拆包（`Thread(f, args=(41,))`）。

**新崩点**（缓存读取继续推进）：`map_insert + 280` ← `_load_cache + 216`：
写 `df["stock_code"] = norm` 时 map 句柄或其 `cap` 是野值（地址形如 0x1900… ，像栈地址）。
已先修一处明显的（列名是栈上的 `c->name` 直接交给 `map_str_key`，而后者会把句柄登记进
哈希侧表 ⇒ 悬垂指针）；仍崩，下一批查 DataFrame shim 的 `self.data` 取值。

### 批次 206 定位（缓存链：**同一段代码，模块级能跑、方法内崩**）

逐条复刻 `ParquetCache.load` 与 `_load_cache` 的语句，**在 driver 语境（模块级）全部成功**：

```
exists 1 / df cols 892 / assigned 892 / dt ok
norm 000300.XSHG / meta_n 0 / meta_src 0 / repair_ok 1
_parquet_cache.load(path) → loaded 1
```

但 `MarketDataFetcher._load_cache("000300.XSHG")`（**方法内**）仍然 Bus error，且崩在
`map_insert + 280` ← `_load_cache + 220`（该函数里唯一一次 map 写入是 `df["stock_code"] = norm`）
⇒ 说明 `df`（`_parquet_cache.load(path)` 的返回值）在**方法语境**下是野值。

下一批：对比「模块级调用」与「方法内调用」的 MIR（重点看
`-> pd.DataFrame | None` 这种**联合注解**的返回值处理，以及 `df["col"] = v` 的接收者取值）。

### 批次 209 排除法（`load_metadata` 在 `_load_cache` 内崩）

| 假设 | 实验 | 结果 |
|---|---|---|
| 嵌套 try 本身有问题 | `outer{try: inner{try: raise}}` | ✓ 正常（inner caught） |
| 函数内局部导入 + try | `inner{try: import json as pq; pq.loads(bad)}` | ✓ 正常 |
| C 桩的 `zeta_raise` 穿过嵌套 try | `inner{try: _rqdatac_init()}` | ✓ 正常（`r 0`，项目自己捕获） |
| **模块级**调用 `_parquet_cache.load_metadata(path)` | 同 driver 语境 | ✓ 正常（打印 warning，`meta_n 0`） |

⇒ 同一函数、同一参数，**模块级调用正常，`_load_cache` 内调用崩**
（`load_metadata + 184` ← `_load_cache + 232`）。区别只在调用链深度/所处的 try 栈
（`fetch_stocks` 内部已有多层 try+setjmp）。下一批：数一数 `fetch_stocks → _load_cache →
load_metadata` 这条链上的 `_setjmp` 帧数，与「模块级」对比（怀疑是**深层 setjmp/longjmp** 的已知脆弱点）。

### 批次 210 补充：最小复现与当前崩帧

`/tmp/stubtry.z`（最小复现）：

```python
def f():
    try:
        import pyarrow.parquet as pq
        t = pq.read_table("/tmp/x")
        return 1
    except Exception as e:
        print("caught"); return 0        # ✓ 正常（caught / f 0）

def g():
    try:
        import pyarrow.parquet as pq
        t = pq.read_table("/tmp/x")
        m = t.schema.metadata or {}      # ← 崩
        return len(m)
    except Exception as e:
        print("caught2"); return 0
```

修复后 `f` 正常、`g` 崩在 **`g + 104`**（lldb），`_read_table_2` 已是 **T（我们的桩）**。
⇒ 帧弹栈已修好（longjmp 落在 `g` 自己的帧里），剩下的问题是
**`t.schema.metadata or {}` 这句在 `t` 为 0 时被求值**：`or` 的右操作数被**过早求值**，
且左操作数的两次解引用（+184/+188）没有空值保护。下一批：
① 看 `g + 104` 的指令；② 决定是给 `or` 加短路，还是在 `t.schema` 这条链上加保护。

### 批次 211（2026-09-20）：平台桩去重压掉了 raise（系统性修复）

`zt_unavailable_soft` 的去重写成 `if (warned[i] == what) return 0;` ⇒ **第二次调用直接返回 0、不抛异常**
⇒ 调用方走进空对象解引用（`t.schema.metadata`、`DataFrame::n_rows`）。

修法：去重只跳过打印，`zeta_raise(1)` 每次执行。
最小复现 `/tmp/stubtry.z` 由「两个函数里第二个崩」变为 **两行都正常**：
`caught / f 0 / caught2 / g 0 / rc=0`。

driver 崩点前移到 `DataFrame::n_rows + 12` ← `__len__` ← `fetch_stocks + 1400`（对 None 调 len）。

### 批次 215 定位（缓存返回的 DataFrame 为空 ⇒ 缩到「方法调用静默消失」）

复刻 `validate_and_repair_stock_ohlcv` 的调用形态后发现：函数的早退分支
`if df is None or df.empty: return pd.DataFrame(), report` 被命中（返回空 DataFrame），
即**参数位置上的 DataFrame** 的 `self.data` 取不到。继续缩：

```python
class C:
    def __init__(self, d) -> None: self.data = d
    def n(self) -> int: return len(self.data)

c = C([1, 2, 3])
print("direct", c.n())      # ✓ 打印 3
def use(x) -> int:
    return x.n()            # ← 形参 x 未注解
print("via", use(c))        # ✗ **整条语句静默消失**（rc=0，无任何输出）
```

⇒ 这是「**未注解形参上的方法调用让整条语句消失**」的静默失败（比崩更糟）。
下一批：看 `use` 的 MIR（`x.n()` 是否被丢/改成幽灵后连 print 一起丢），
并修「静默丢语句」这条红线（编译器不得静默丢失语句）。

### 批次 217（2026-09-20）：注解形参修好后的连锁推进

修好「注解形参类型」后，`validate_and_repair_stock_ohlcv` 真正开始执行，随之暴露一串
「列上的方法调用」幽灵符号（`_notna`、`[dynamic]str__isna`、`[dynamic]str__max`、
`_set__add`、`py_noop2_3` …）。本轮补齐：

- MIR：map 下标（`df["close"]`）取 `map<K,V>` 的 **V** 作为结果类型（此前恒 I64 ⇒ 列方法全落幽灵）
- MIR：`.isna/.notna/.any/.all/.max/.min` 于向量接收者；`set.add`（去重）/`discard`/`remove`
- 运行期：`py_vec_isna/notna/any/all/extreme/add_unique/discard` + 裸 `isna*/notna*` 安全回退
  （先验证是不是向量，不是就原样返回并告警）+ `py_noop2_3`/`py_noop3`

**新崩点**：`DataFrame::copy + 24` ← `validate_and_repair_stock_ohlcv + 808`
（`out = df.copy()` 里的 `self.data` 字段取值 —— 结构体字段索引在「基类型是形参」时仍会走
`variant=""`/`field_count=2` 的兜底）。下一批：把字段索引按**声明类型**查（此前试过一版会
`ExtractOutOfRange`，需要同时把 field_count 对齐）。

### 批次 219 定位（`DataFrame::copy` 的 `self` 是 0）

反汇编 `DataFrame::copy`（lldb）：

    +8 : str x0,[sp,#0x10]      ; self
    +12: bl map_new             ; dict() 新建 map
    +20: ldr x8,[sp,#0x10]      ; x8 = self
    +24: ldr x1,[x8]            ; ← 崩：self 为 0
    +28: bl py_map_update

⇒ `df.copy()` 的**接收者**是 0。但把同一形态缩到最小却完全正常：

    class 无关的最小复现：
    def cp(d: pd.DataFrame) -> int:
        e = d.copy()
        return len(e)
    → direct_copy 892 ✓ / fn_copy 892 ✓

即「注解形参 + `.copy()`」本身没问题。真实函数的前置语句是

    cfg = cfg or MarketCleanConfig()
    report = OhlcvRepairReport(input_rows=len(df))
    if df is None or df.empty: return pd.DataFrame(), report
    out = df.copy()                                   ← 崩

下一批：把 `OhlcvRepairReport(input_rows=len(df))` 这类**结构体构造**加进最小复现，
怀疑构造过程把 `df` 所在槽位/寄存器写坏（或默认参数 `cfg = cfg or MarketCleanConfig()`
的求值顺序问题）。

### 批次 219 补充（最小复现仍正常）

把真实函数的前置语句（含 `cfg: MarketCleanConfig | None = None` 默认参数、
`cfg = cfg or MarketCleanConfig()`、`OhlcvRepairReport(input_rows=len(df))`）逐条搬进最小复现：

    f 892 ✓（不再崩）

⇒ 形态本身都正常。下一批改用**调用真实函数**并显式传第三参
（`validate_and_repair_stock_ohlcv(df, norm, MarketCleanConfig())`）对比默认参数路径；
同时打印 `len(df)` **函数内**（通过一个包装函数）来确认形参 `df` 在真实函数里是否已经是 0。

### 批次 220（2026-09-20）：把 `DataFrame::copy` 的 self==0 缩到「函数内特有的槽位失效」

| 实验 | 结果 |
|---|---|
| 直接调 `validate_and_repair_stock_ohlcv(df, norm)`（harness） | 崩（`DataFrame::copy + 24`，self==0） |
| 显式传第三参 `MarketCleanConfig()` | 崩（同上）⇒ 与默认参数无关 |
| 把函数体全部语句**内联**到 harness（含 `MarketCleanConfig()` / `OhlcvRepairReport(input_rows=len(df))` / `df.copy()` / `df.empty`） | **全部正常**（`copy 9 892`） |
| `df.empty` 之后再 `df.copy()`（怀疑 empty 写坏实参） | 正常（`b_empty 0 / len 892 / c_copy 9 892`） |
| 函数内 `try:` 包住调用 | 仍崩 ⇒ 与 try 无关 |

函数自身 MIR 检查：

- `df` 形参 = slot **1**，`ParamInit{param_id:1}`，**没有任何语句写 slot 1**（脚本统计 0 处）
- `DataFrame::__len__(1)`、`DataFrame::empty(1)` 都传入 slot 1 且**正常返回**
- 紧接着 `DataFrame::copy(1)` 却让被调方拿到 self==0

⇒ 值在「`empty` 之后、`copy` 之前」被清零。这段区间里的语句只有
`report = OhlcvRepairReport(input_rows=len(df))` 与 `cfg = cfg or MarketCleanConfig()`
（MIR 里还有一处 `Assign{lhs:3,…}` + `zeta_env_set` —— 对**形参**做 env 写，值得怀疑）。
下一批：把这两条按**同样的顺序**放进一个「只有函数边界不同」的复现里逐步二分
（例如把 harness 的语句包进一个 `def g(d, norm)` 而不是顶层）。

### 批次 221：调用点**没有装载实参**（反汇编铁证）

`validate_and_repair_stock_ohlcv` 里 `df.copy()` 的调用点（lldb 反汇编）：

    +796: bl map_get                 ; 上一条语句的结果落在 x0
    +800: str x0,[sp,#0x410]
    +804: bl DataFrame::copy         ; ← **没有重新装载 x0**
    +808: str x0,[sp,#0x470]
    +816: bl DataFrame::empty        ; ← 同样没有装载实参（拿的是 copy 的返回值）

⇒ 不是「值被清零」，而是**该调用点根本没写实参寄存器**：`copy`/`empty` 收到的是上一条语句
（`map_get`）残留在 x0 里的东西。MIR 是 `Call{func:"DataFrame::copy", args:[1]}`（形参 1 = df），
而 codegen 在这个位置**跳过了 `locals[1]` 的装载**。

同时也能解释为什么内联复现都正常：那段代码在**函数内**才走到这条发射路径。
下一批：在 codegen 的 `MirStmt::Call` 实参发射里找出「参数 id 不在 `locals` 时静默跳过」的分支
（`locals.get(id)` 为 None ⇒ 应当回退 `gen_expr_safe(id)` 或响亮报错，绝不能发一个**无实参**的调用）。

### 批次 221 补充：可疑的「1 参数 / void 返回」extern 声明

`codegen.rs` 里有一处兜底（约 2291 行附近，位于「未解析函数的通用处理」分支）：

    if name.contains("::") || self.module.get_function(name).is_none() {
        let void_type = self.context.void_type();
        let fn_type = void_type.fn_type(&[self.i64_type.into()], false);   // 固定 1 参 + void 返回
        self.module.add_function(name, fn_type, Some(Linkage::External));
        return f;
    }

凡是名字里带 `::` 的调用（`DataFrame::copy`、`DataFrame::empty`…）在**定义尚未出现时**
都会拿到这个「1 参数、void 返回」的声明 —— 与真正定义（含 `self` 的 1 参、返回句柄）**不一致**，
于是实参装载/返回值处理都可能错位（正是反汇编里「调用点没装载实参」的形态）。

`get_or_declare_function`（另一处，2416 行）已经改成**先查定义**再声明；这处旧兜底应同规则处理
（或直接删掉，交给 2416 那处）。

下一批：给这处兜底加上「先查 `name` / mangled / fns 缓存」的前置检查（与 2416 对齐），
再跑 driver 看 `DataFrame::copy` 的调用点是否变成正确的实参装载。

### 批次 223：IR 正确、汇编却缺实参（pipeline 猜想）

`--emit-llvm` 的 IR（同一份 driver，`ZETA_NO_OPT=1`）：

    merge:
      %250 = load i64, ptr %78, align 4          ; %78 = alloca，entry 里 `store i64 %0, ptr %78`
      %251 = call i64 @"DataFrame::copy"(i64 %250)   ; ✓ 实参在
      …
    define i64 @"DataFrame::copy"(i64 %0) { … }     ; 全模块只有这一个定义，没有第二份 declare

而同一处**机器码**（lldb 反汇编，刚重新编译过的 drv400）：

    +788: ldr x8,[sp,#0x4d0]
    +792: ldr x0,[x8]
    +796: bl map_get
    +800: str x0,[sp,#0x530]
    +804: bl DataFrame::copy        ; ← x0 是 map_get 的残留，**没有 load %78 那一步**

⇒ IR 完全正确、汇编与该 IR 不一致。最可能的解释：**产出目标文件的模块与 `--emit-llvm` 打印的
模块不是同一份**（例如 dump 发生在某个 fixup/优化之后，而 .o 用的是更早的模块），
或存在一次**寄存器复用/参数装载被跳过**的后端预处理。

下一批：在 codegen 里把「生成 .o 之前的最终 IR」也打印一份（同一路径、紧邻 dump），
比较两处 IR 是否一致；若一致 ⇒ 定位到 LLVM 之后的那次变换；若不一致 ⇒ 找到被跳过的 fixup。

### 批次 224：-O0 下的下一个崩点 = `.loc[<布尔掩码>]`（列映射模型缺失）

反汇编（-O0 产物，`validate_and_repair_stock_ohlcv` 内）：

    +1364: bl map_get              ; 第二个参数是 `(x == 0)` 的 **布尔**（不是列名）
    +1368: str x0,[sp,#0x7f0]
    +1372: ldr x0,[sp,#0x7f0]
    +1376: bl DataFrame::copy      ; ← 对 map_get 的结果（很可能是 0）调 copy ⇒ 崩

即源码里的 `out = out.loc[~invalid].copy()`：`.loc[<布尔掩码>]` 在本实现里被当成 **map 下标**
（键是布尔值），取不到就返回 0，随后 `.copy()` 解引用 0。

`pylib/pandas.z` 开头已注明「`sort_values`/`iloc` 是行级操作，列映射模型表达不了（响亮失败）」，
但这条路径既没响亮失败、结果也不对。

下一批：给 `DataFrame.loc(mask)` 一个**真实的按行过滤**实现（列映射模型完全可以做：
对每列向量按掩码取子集；`df.columns` 不变），或至少改成**响亮 abort**，绝不返回 0 让调用方崩。

### 批次 225 补充：`_parquet_cache.load` 返回的帧「0 列却有 892 行」

探针（driver 语境）：

    exists 1
    load_none 0                    ← 拿到了帧
    cols 0 / rows 892              ← **列数 0，行数 892**（不一致）

而 `_load_cache("000300.XSHG")` 最终返回 **None**（内部 `if df is None: return None` 或 except）。

`len(df)` 走 `n_rows()`（`list(self.data.keys())` 为空应得 0），却给出 892 ⇒ 说明 `len(df)`
与 `df.columns` 看到的**不是同一个字段/路径**；而 `.loc[mask]` 新实现里 `out = {}` +
逐列 append + `DataFrame(out)` 是重点怀疑对象（字典构建/字段写入）。

下一批：在最小复现里跑一遍 `.loc[mask]` 并同时打印 `len(df.columns)` 与 `len(df)`，
定位是「`DataFrame(out)` 的 `data` 字段没写进去」还是「`n_rows()` 读错了字段」。

### 批次 225 补充 2：`.loc` 里 `self.data` 读到的是 **PyJson**（字段布局不一致）

最小复现（`/tmp/loc.z`，只有 `pd.DataFrame({...})` + `df.loc([1,0,1])`）：

    /tmp/loc  →  abort
    lldb:
      #3 zt_map_json_mismatch + 60     ← 运行期护栏（响亮，符合红线）
      #4 map_get + 208
      #5 DataFrame::loc + 256          ← 就在新写的 loc 里
      #6 main

即 `loc` 内部对 `self.data` 取 map_get 时，**该值被自己的护栏判成 PyJson** ⇒ 说明
`DataFrame` 结构体里 `self.data` 读到的**不是**构造函数写入的那个 map。

与 Python 的差异点：`_load_cache` 里有 `df.attrs["source"] = meta["source"]` ——
Python 允许运行时给对象加属性，而本实现的结构体字段是**编译期固定**的；
字段索引/字段数在这类「动态加字段」后可能不再一致（批次 218 的按声明类型查表正是为此）。

下一批：打印 `DataFrame` 结构体的字段清单（构造处 vs 读取处）以及 `self.data` 的字段索引，
确认是不是 `attrs` 之类的动态字段把它挤偏了。

### 批次 226：`.loc` 的最小复现与 IR 对照

`/tmp/loc2.z`（无 `.loc`）：`cols 2 / rows 3 / a0 1` ✓
`/tmp/loc.z`（加 `df.loc([1,0,1])`）：**abort** 于护栏 `zt_map_json_mismatch ← map_get ← DataFrame::loc + 256`

IR 对照：

- `DataFrame::n_rows` 正确读 `{ i64 }` 的字段 0 ⇒ `map_keys` ✓
- `DataFrame::loc` 里出现 `%map_ptr11 = inttoptr i64 %92 to ptr` / `map_get(ptr %map_ptr11, i64 %93)`，
  其中 `%92 = load i64, ptr %54` —— 需要确认 `%54` 是 `self.data` 还是新建的 `out` 字典；
  护栏判定「首字 1..8（像 JSON tag）」⇒ 该句柄**不是**普通 map。

下一批：把 `loc` 的 MIR/IR 里每个 `map_get` 的 map 来源标出来（`self.data` vs `out`），
确认是哪一处拿到了 JSON 句柄；必要时把 `loc` 的返回改成 `self.copy()` + 覆盖列，避开新建字典。

### 批次 227 补充：`loc` 的 IR 与 `n_rows` 完全同形，但运行期 `self.data` 是 0

IR 对照（同一模块）：

    DataFrame::loc   : inttoptr i64 %65 → load { i64 } → extractvalue 0 → map_ptr → map_get   ✓
    DataFrame::n_rows: inttoptr i64 %17 → load { i64 } → extractvalue 0 → map_ptr → map_keys  ✓

⇒ 字段读取的**编译形态已一致**（批次 197/218 的修法生效）。但运行期 `map_get` 的 map 是 0/坏值：
最小复现 `/tmp/loc3.z`（`pd.DataFrame({...})` 后**直接** `df.loc([1,0,1])`，中间没有任何语句）：

    df = pd.DataFrame({"a": [...], "b": [...]})
    e  = df.loc([1, 0, 1])          ← segfault in map_get ← DataFrame::loc
    （同一份帧先 `df.columns` / `len(df)` 都是对的）

⇒ 帧对象在 ctor 之后、`loc` 之前的某一刻丢失了 `data` 字段（怀疑 `DataFrame(...)` 的
StructNew 落在**栈**上，或 `zeta_map_set_tag`/env 写入把它挤掉）。
下一批：检查 `DataFrame(...)` 构造点的 IR（StructNew 是堆还是栈）与两次调用之间对该对象的写入。

### 批次 228：断在 `DataFrame::loc` 入口 —— **实参本身就不对**

lldb（`breakpoint DataFrame::loc`）：

    x0 = 0x0000000100385f10      ← 内容看着像一串字符串指针（像 map/vec 的数据区）
    x1 = 0x0000000000000002      ← **mask 竟然是整数 2**（应为 `[1,0,1]` 的向量句柄）

即调用方把「2」当第二个实参传进来了（像元数/步长之类的标量），而第一个实参不是 DataFrame 结构体。
IR 侧却完全正常：`pandas__DataFrame` 走 `runtime_malloc(8)` + 存字段 0 + 返回堆指针；
`main` 里 `%103=call pandas__DataFrame → store %63 → load → store %18 → %116=load %18 →
call DataFrame::loc(%116, %117)`。

⇒ 下一批：核对 `df.loc([1,0,1])` 这条链上 `%117` 的来源（IR 里它应是向量句柄），
以及为什么运行时成了 2；同时确认 `pd.DataFrame({...})` 的返回值在运行时到底是结构体还是 map。

### 批次 228 补充：`[1,0,1]` 的 vec_push 回写把向量槽写坏了（IR 铁证）

`main` 的 IR（`e = df.loc([1,0,1])` 这段）：

    %106 = call i64 @zeta_dynarray_new(i64 3)
    store i64 %106, ptr %26
    %107 = load i64, ptr %26
    %108 = call i64 @vec_push(i64 %107, i64 1)     ; 推入第一个元素 ✓
    %109 = load i64, ptr %2                        ; ✗✗ 从**无关的 alloca** 取值
    store i64 %109, ptr %26                        ; ✗✗ 把向量槽覆盖成那个值
    %110 = load i64, ptr %26                       ; 此时 %26 已不是向量
    %111 = call i64 @vec_push(i64 %110, i64 0)     ; 往垃圾里推
    …
    %115 = load i64, ptr %64
    store i64 %115, ptr %26                        ; 最终 mask 槽 = 垃圾
    %117 = load i64, ptr %26
    %118 = call i64 @"DataFrame::loc"(i64 %116, i64 %117)

而同一段 MIR 是正确的：

    Call { func: "vec_push", args: [22, 18], dest: 23 }
    Assign { lhs: 22, rhs: 23 }        ; ✓ 回写的是 push 的 dest

⇒ **MIR 对、IR 错**：`Assign{lhs: 22, rhs: 23}` 在 codegen 里被编译成「从另一个 alloca 取值」，
于是每次 `vec_push` 之后向量变量都被写坏。这也解释了批次 222/223 里
`DataFrame::copy` 实参为 0 的形态（同一个「Assign 的 rhs 取错槽」家族）。

下一批：定位 codegen 里 `MirStmt::Assign` 的 rhs 取值路径（`gen_expr_safe(rhs)` 为什么
会落到别的 alloca），并核对 `locals` 与 `exprs` 的 id 是否在同一编号空间。

### 批次 228 补充 2：MIR/IR 复核后**都正确**（把 debug 打印加进 `loc` 后连打印都没到）

复核结论修正：

- `[1,0,1]` 的 MIR 是 `zeta_dynarray_new → vec_push×3（每次 `Assign{h, sink}` 回写） → exprs[id]=Var(h)` ✓
  （IR 里的 `%109 = load ptr %2` 其实是**同一个槽**的 alloca：IR 的 `%N` 编号与 MIR 的 id 不是一套）
- `pandas__DataFrame` 的 IR 是 `runtime_malloc(8)` + 存字段 0 + 返回堆指针 ✓
- `main` 里 `pandas__DataFrame → df 槽 → DataFrame::loc(df, mask)` ✓

把探针加进 `loc` 的**第一行**（`print("LOC …" + str(len(self.data)))`）后**连打印都没出现**，
且崩点仍是 `map_get`：

⇒ 崩溃发生在 `len(self.data)` 这一句 —— 即**进入 `loc` 后读 `self.data` 就是坏的**。
但同一帧对象在 `main` 里刚被 `df.columns` / `len(df)` 正常使用过（`/tmp/loc2.z` ✓）。

下一批（不必再猜 IR）：把 `main` 里 `pandas__DataFrame` 的返回值**立刻**用 `print(len(df))` 验证，
再把 `loc` 的探针换成「先不读 `self.data`，只打印入参 mask」——二分到底是 **self 坏**还是 **mask 坏**，
然后顺着「谁在两次调用之间改了这个对象」查（重点：`zeta_map_set_tag` 的侧表、env 写入、
以及 `DataFrame(...)` 的字段是否被 GC 移动/复用）。

### 批次 231（2026-09-20）：`~mask` ⇒ `!` 的归并（loc 掩码恢复）

`out.loc[~invalid]` 的 `~invalid` 被降级成 `Call{func:"!"}`（整数逻辑非）⇒ mask=0 ⇒
响亮断言 `DataFrame.loc: mask is missing`。修法：`!` 分支先看类型，向量走 `py_vec_not`。

崩点推进到 `validate_and_repair_stock_ohlcv + 2020`（-O0 产物），这是本会话在清洗函数里
到达的最深位置。

### 批次 235：`_parquet_cache.load` 返回的帧「892 行 / 0 列」，`.loc(...).copy()` 之后却有 9 列

探针（driver 语境，真实文件）：

    df = _parquet_cache.load(path)
    in  892 rows / 0 cols        ← ✗ 列数为 0
    out = df.loc(<全 1 掩码>).copy()
    out 892 rows / 9 cols        ← ✓ 9 列回来了

同一方法 `columns`（→ `column_names()` → `list(self.data.keys())`）在两处给出 0 和 9
⇒ **`load` 返回的那个帧的 `data` 是空的**（或字段读取取到了空 map），而 `loc` 走的是
「C 里按 `map_keys` 重建」的路径，所以拿到的是真数据。

另外验证：`list(d.keys())` / `len(d)` / `len(ks[0])` 本身都正确（`ks_len 2 / map_len 2 / ks0 1`）。

下一批：查 `ParquetCache.load` 的返回路径 —— `df = pd.read_parquet(path)`（9 列 ✓）之后，
`for col in date_cols: if col in df.columns: df[col] = pd.to_datetime(df[col])` 与
`return df` 之间，`df` 的 `data` 是不是被某次 `__setitem__`/`to_datetime` 换成了空 map。

### 批次 236：**函数返回值** `-> pd.DataFrame` 退化成 `map`（本族总根因）

最小复现（同一程序内对照，均为 -O0 产物）：

    d = pd.read_parquet(PATH)          # 模块级：rp 892 9      ✓
    e = pd.DataFrame({"a":["1","2"]})  # 模块级：ctor 2 1     ✓
    def inside(path) -> i64: d = pd.read_parquet(path); return len(d.columns)
    inside(PATH)                       # inside_rp 9          ✓ 函数**内部**是对的
    def mk() -> pd.DataFrame: d = pd.DataFrame(...); return d
    m = mk(); len(m.columns)           # mk 2 0               ✗ 传出去就成了 map

⇒ 函数**内部**一切正常；**跨函数返回**后，调用方拿到的值按 **map** 处理：
`m.columns` 生成的是 `map_get_default(m, "columns", …)`（MIR 实测）而不是
`DataFrame::columns` ⇒ 列数恒 0、`m.data` 也变成 map 取值（恒空）。

对照：`pd.DataFrame(...)` 构造器在 MIR 里是 `Named("DataFrame")`（gen.rs 4711 显式标注），
所以模块级写得对；**只有「用户函数声明 `-> pd.DataFrame`」这条注解→类型的路**丢成了 map。

这条覆盖了此前所有「方法内/深链取到坏值」的现象（`copy` / `loc` / `reset_index` /
`load` / `array_len` 各一次都是它的不同表象）。

下一批：定位用户函数**返回注解**的解析路径（`pd.DataFrame` 这种**点号限定名**），
把 `DataFrame` 解析到 shim 的结构体（而不是退化成 `map`）。

### 批次 238：`-> tuple[pd.DataFrame, int]` 的元素类型（解构后 frame 类型丢失）

`remove_extreme_return_bars` 返回 `tuple[pd.DataFrame, int]`；该类型是
`Named("tuple", [Named("DataFrame"), I64])`，而解构代码只匹配 `Type::Tuple(ts)`
⇒ 掉到 `_ => Type::I64` 默认分支。于是解构出来的帧被当整数：
`len(out.columns)` → map 取值（恒 0）、`out["a"]` → 对结构体句柄做 map 下标（SEGV）。

修法两处：
1. `resolver.rs::shim_class_normalize` 递归进 `Type::Tuple`（此前只进 `Named`/`DynamicArray`）；
2. `gen.rs` 解构分支同时接受 `Named("tuple", ts)`。

回归用例 `tests/python_style/t274_tuple_return_dframe.z`（期望 2 / 2 / 1）。
度量：官方 194/194、python_style **274**/2、语料 39/39 全绿。

崩点：`DataFrame::n_rows + 16 ← __len__ ← validate_and_repair_stock_ohlcv + 2912`
（`len(out)` 现在派发到**正确**的方法，只是 `out` 的值仍坏 —— 下一批继续）。

### 批次 239：清洗函数崩溃点二分到**一行** —— `out["close"] * out["volume"]`

把 `validate_and_repair_stock_ohlcv` 的函数体整段复刻到 harness（同样签名
`(df: pd.DataFrame, stock_code: str, cfg: MarketCleanConfig | None = None)`），逐步
print 后得到精确断点：

    A 892 9   B … C … D … E 892 9         ← 前面全部正常
    E1 vol_fillna 892 / E2 vol_clip 892 / E3 set 892 9 / E4 pre 892 9
    E4a close 892 vol 892                  ← 两个向量都能取长度
    prod = out["close"] * out["volume"]    ← ✗ 崩在 array_len + 4

即 **两个都已就绪的向量相乘** 时踩坏指针：`array_len` 拿到的实参不是数组句柄
（字符串句柄被当数组 → `arr-16` 越界）。parquet 读出来的列元素是**字符串**，
所以「向量 × 向量」必须先按数值解析，不能对元素取 `array_len`。

对照：`out["volume"].fillna(0).clip(lower=0)` 与本步无关（E1/E2/E3 全绿）。

位置：`probe + 3316`（drv629）。下一批：修「向量 × 向量（字符串元素）」这一条。

### 批次 240：**向量 × 向量**改为逐元素乘（不再走 matmul/semiring）

`c = a * b`（两个字符串向量）此前落到 `SemiringFold { op: Mul }`（矩阵乘），
它把元素当数组走 ⇒ `array_len` 读字符串句柄的 `arr-16` ⇒ 崩。pandas 语义是**逐元素**。

修法：
- 运行期 `py_vec_mul(a, b)`：逐元素 `strtod` 两边相乘，解析不出的原样保留；
- MIR：`op == "*"` 且两侧都是数组 ⇒ `py_vec_mul`（放在 `else if op == "*" || op == "@"` **之前**）。

最小复现 `/tmp/vm.z`（`a=["1.5","2.5"]; b=["3.0","4.0"]; c=a*b`）：
修复前 SEGV，修复后 `2 / 10` ✓。

顺带（同一函数 `remove_extreme_return_bars` 的下一处）：
`grp["close"].pct_change().abs()` 也缺运行期实现 —— 已加 `py_vec_pct_change` /
`py_vec_abs` 与 weak 回退 `pct_change`/`pct_change_1`/`abs_1`（shim 自己也发 `pct_change`
符号 ⇒ 必须 weak，否则 `duplicate symbol '_pct_change'`）。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 241：`[dynamic]str::` 方法（`pct_change` / `abs`）与结果类型

- 运行期补上动态数组方法的真符号：`_[dynamic]str__pct_change` / `_[dynamic]str__abs`
  （此前只有一个 `pct_change` weak 回退，链接时 `Undefined symbols: _[dynamic]str__pct_change`）。
- MIR：接收者是数组时，`pct_change` / `abs` 直接走 `py_vec_pct_change` / `py_vec_abs`
  并把结果标成 `DynamicArray(Str)`（此前被标成 I64 ⇒ `c[2]` 是整数下标、`print_str` 崩）。

验证 `/tmp/pc4.z`：

    a = ["1.0", "2.0", "4.0"]
    c = a.pct_change()   → len 3 / c[2] = 1     ✓（(4-2)/2）
    b = a.abs()          → len 3                ✓

**驱动进展（重要）**：`validate_and_repair_stock_ohlcv` **整段跑完**，
崩点从 `_load_cache` 移到 `MarketDataFetcher::fetch_stocks + 3324`
（栈里已经没有清洗函数）⇒ 数据层首次完整通过一只标的的清洗。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 242：两处新发现（清洗路径分叉 + try 返回值）

1. **`remove_extreme_return_bars` 仍是崩点**：`drv641`（显式 `MarketCleanConfig()`，
   `drop_extreme_bars=True`）崩在 `validate_and_repair_stock_ohlcv + 2888`（= `len(out)`），
   而 driver（用项目自身配置）已越过清洗函数 ⇒ 说明项目默认配置**关掉了**极端 bar 清洗，
   所以我们看到的 `fetch_stocks + 3324` 是**另一处**问题（`cached is not None and len(cached) > 0`，
   反汇编确认：`cset ne` + `cset gt` 正是这两个条件）。
2. **`try` 内 `return <局部帧>`**：探针 `/tmp/un3.z` 输出正确（`try_ret 2 2`）但**退出码为 1**
   —— 隐式返回 0 被写成 1，另记一笔（不影响本批判定）。

下一批优先级：(a) `remove_extreme_return_bars`（`vec > 标量` 掩码链，`/tmp/pc.z` 已复现）；
(b) `fetch_stocks` 里 `len(cached)` 的坏帧来源。

### 批次 243：`column > 标量` 逐元素比较（掩码不再是 Bool）

`mask = ret > max_abs_daily_return` 此前结果类型是 **Bool**，掩码被当向量用：
`len(mask)` 读 `mask-16`（小整数减 16 ⇒ 越界）⇒ 段错误；`DataFrame.loc` 则响亮
abort（"mask is missing"）。

修法：MIR 在比较运算两侧**恰好一侧**是数组时改走运行期
`py_vec_{gt,lt,ge,le,eq,ne}[_i](vec, scalar)`（`_i` 变体吃 i64 字面量，避免调用点做浮点转换），
结果标成 `DynamicArray(I64)`。

验证：
- `/tmp/cmp.z`：`a = ["1.0","2.0"]; m = a > 0.5` → `m 2` ✓（修复前 SEGV）
- `/tmp/pc.z` 整条链：`ret 4 / abs 4 / mask 4 / not 4` ✓

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

崩点仍在 `validate_and_repair_stock_ohlcv + 2884`（`report.output_rows = len(out)`，
`out` 来自 `remove_extreme_return_bars`）：说明掩码链已通，问题落在它内部更后面
（`groupby` 迭代 / `concat` / `iloc[0:0]` 等）或该函数返回值本身。

### 批次 244：`for k, grp in df.groupby(col)` 真分组（GroupBy 不再是空壳）

`DataFrame.groupby` 此前返回 `GroupBy(self.copy())`，`GroupBy` 只有 `mean/sum` 两个桩、
**没有迭代协议** ⇒ `for _code, grp in df.groupby("stock_code")` 迭代的是结构体句柄
（垃圾），`remove_extreme_return_bars` 于是产出死帧。

修法（三处）：
1. **运行期** `py_df_groupby(frame, key)`：按 `map_str_key` 分组（**必须哈希**——
   每个字符串值都是独立分配，按指针分组会一组一行），每组重建子帧，返回
   「`[key, subframe]` 两槽块」的向量（`stack_array_get(elem, i)` 正好读这两槽）；
   `py_groupby_pairs(gb)` 解包 GroupBy 的 (frame, key)。
   两处坑：列的 `map_get`/`map_insert` 也要用 `map_str_key`；`vec_push` **扩容时会返回新指针**，
   必须回写 map（否则列指向旧的 0 长度头，表现为「每组 `len(grp)==0`」）。
2. **shim**：`GroupBy.__init__(frame, key)`；`DataFrame.groupby(self, by, sort=True) -> GroupBy`。
3. **MIR 的 for 降级**：迭代对象类型是 `Named("GroupBy")` 时改调 `py_groupby_pairs`，
   并把元素类型标成 `Tuple([Str, Named("DataFrame")])`（这样 `grp["close"]` / `grp.loc[...]`
   才按 DataFrame 派发）。

验证 `/tmp/gb.z`（`df` 2 列 4 行、两个 code）：

    group a 2 2 / group b 2 2 / groups 2 parts 2     ✓
    （修复前：group a 0 0，且迭代 4 次 —— 每组一行）

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**剩余**：`remove_extreme_return_bars` 现在走到 `grp.loc[~mask]` 时 `py_df_loc` 响亮 abort
（"mask is missing"）。单独复刻该链条（`pct_change → abs → > 标量 → sum → ~ → loc`）在同一
harness 里**全部通过**（`ret 892 / mask 892 / sum 891 / not 892 / loc 1 9`），
⇒ 该函数**内部**某处的静态类型没落到实处（怀疑 `grp["close"]` 的 `lt(vec,str)` 或
`cfg.max_abs_daily_return` 取值），下一批用运行期探针（env 门控打印）定位。

### 批次 245：浮点**字面量**在运行期助手实参里变成 0

`mask = ret > 0.2` 走 `py_vec_gt(vec, 0.2)`，但运行期探针显示 **rhs=0**：
codegen 把浮点字面量放进**整数寄存器**，而 C 侧声明是 `double` ⇒ ABI 不匹配读成 0。
后果：掩码变成「全部 > 0」⇒ `mask.sum() == 891/892`，`~mask` 全 0，`DataFrame.loc`
响亮 abort（"mask is missing"）。

修法：MIR 在 RHS 是 `FloatLit` 且另一侧是数组时，改走
`py_vec_{gt,lt,ge,le,eq,ne}_bits(vec, <f64 的位模式>)`，C 侧 `memcpy` 还原 double。
（变量形式的浮点实参仍走原 `double` 版本。）

验证 `/tmp/cmp3.z`：`sum 1`（只有 1.0→2.0 这一处变化超过 0.5）、`not 3` ✓。
修复前是 `sum 891`。

另外运行期加了 `ZT_PROBE_LOC=1` 门控探针（`vec_cmp` / `vec_not` / `df_loc`），
用来在真实调用链里确认哪个助手被调用、实参是多少 —— 本轮正是靠它定位的。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**剩余**：`remove_extreme_return_bars` 仍在 `grp.loc[~mask]` 处拿到 mask=0：
探针显示 `vec_not in=<有效指针>` 之后 `df_loc mask=0` ⇒ `~mask` 的结果没有被写进
`loc` 读的那个槽位（嵌套表达式 `grp.loc[~mask]` 的临时值未物化），下一批查这条。

### 批次 246：`~mask` 结果丢失 —— IR 正确，问题在第二次执行

faithful 复刻（`df["stock_code"] = "000300.XSHG"` 之后调用函数体）后稳定复现：

    inloop 892 892 892 0        ← 第一次迭代（0 是**正确**的：真实日收益都 < 0.2）
    vec_not in=… out=… n=892 / df_loc mask=… / appended 1 892   ✓
    vec_not in=… out=… **n=1**   ← 第二次（输入只有 1 个元素）
    df_loc mask=0               ← ✗ 响亮 abort

同时核对了 LLVM IR（`define i64 @reb` 内）：

    %146 = load i64, ptr %17      ; mask
    %147 = call i64 @py_vec_not(i64 %146)
    store i64 %147, ptr %71
    %148 = load i64, ptr %49      ; grp
    %149 = load i64, ptr %71      ; ~mask ✓
    %150 = call i64 @"DataFrame::loc"(i64 %148, i64 %149)

⇒ **IR 完全正确**（存了也读了），但运行期第二次执行时 `loc` 读到 0，
而且那次 `py_vec_not` 的输入只有 1 个元素 ⇒ 现场是「第二次执行时槽位/实参错位」。
（`ZT_PROBE_LOC=1` 探针留在运行期，后续继续用。）

### 批次 247：两处修复 —— `iloc[0:0]` 空切片 + `pd.concat` 缺返回注解

1. **`df.iloc[0:0]`**：切片不是向量，shim 的 `iloc(mask: lt(vec,i64))` 收到 0 ⇒ 直接进
   `py_df_loc` 响亮 abort。运行期新增 `py_is_vec` / `py_df_empty_like`，shim 的 `iloc`
   在 key 不是向量时返回**同列的空帧**（`remove_extreme_return_bars` 的 `if not parts:` 分支）。
2. **`pd.concat` 缺返回注解**：未注解 ⇒ 返回类型默认 I64 ⇒ 调用方
   （`return pd.concat(parts, …), removed`）把帧当整数，`c 0 0 / d 0 0`。
   补上 `-> DataFrame` 后：`c 3 2 / d 2 2` ✓。
   （同一族：shim 里**函数都必须写返回注解**，批次 99 的老规矩。）

诊断手段（本轮靠它定位）：运行期探针加 `__builtin_return_address` + `dladdr`；
再到 lldb 里给 `py_df_loc` 加 `$x1 == 0` 条件断点，拿到完整栈
`py_df_loc ← DataFrame::loc ← DataFrame::iloc ← reb + 932` —— 一眼看出是 `iloc[0:0]`。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**新阻塞**：链接报 `Undefined symbols: _[dynamic]str__map`（`fetch_stocks` 里
`result["stock_code"].map(lambda x: self._normalize_stock_code(str(x)))`）。
`pd.concat` 注解修好后 receiver 被正确判成「字符串向量」⇒ 派发到 `[dynamic]str__map`；
运行期还没有这个符号。它吃的是 **lambda（可能带闭包环境）**，需要先确认编译器传参
约定（`(vec, fn)` 还是 `(vec, fn, env)`）再实现，避免静默错值。

### 批次 248：`[dynamic]str__map`（`series.map(lambda …)`）

`pd.concat` 注解修好后，`fetch_stocks` 里
`result["stock_code"].map(lambda x: self._normalize_stock_code(str(x)))` 的 receiver
被正确判成字符串向量 ⇒ 派发到 `_[dynamic]str__map`，运行期缺这个符号（链接失败）。

MIR 实证：`[dynamic]str::map(<vec>, <closure>)` 只传 **两个** 实参，闭包被编译成顶层
函数 `__closure_0`（`param_indices: [("v", 1)]`）⇒ ABI 是 `(vec, fn_ptr)`，与
`py_functools_reduce` 同一约定。

运行期新增 `zt_dyn_str_map`（asm 名 `_[dynamic]str__map`）：逐元素 `fn(x)`，
按 `vec_push` 的返回值回写句柄；`fn == 0` 时**响亮 abort**（绝不静默产出垃圾）。

验证 `/tmp/mapz.z`：`a.map(lambda v: len(v))` → `len 2` ✓。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**崩点**：回到 `validate_and_repair_stock_ohlcv + 2884`（`report.output_rows = len(out)`），
这次是 `map_keys + 60` —— `out` 的 map 是垃圾。`concat` 本身已在 `/tmp/cc.z` 验证正确
（`c 3 2 / d 2 2`），所以嫌疑落在 `remove_extreme_return_bars` 的返回值经
**tuple 解构**交给调用方这一段，下一批查。

### 批次 249：`|` 的判据放宽到「任一侧是向量」（掩码 2N 的根因）

探针实证（`drv701`，真实 `validate_and_repair_stock_ohlcv`）：

    vec_cmp kind=1 out=… n=892            ← `out["close"] < cfg.min_price`
    vec_not in=…  out=… **n=1784**        ← 2 × 892！
    df_loc frame=… mask=<1784 元素向量>   ← 随后 n_rows 崩

`invalid = out["close"].isna() | (out["close"] < cfg.min_price)`：`isna()` 的静态类型是
`DynamicArray(I64)`（批次 241 之后），**不是 Bool**，所以批次 234 的 `boolish` 判据没命中，
`|` 掉回「向量拼接」⇒ 掩码变成 2N。

修法：判据放宽为「**任一侧是 DynamicArray/Array** 就逐元素 OR」（Python 里只有 `+` 拼接）。

修后探针：`vec_not out n=892` ✓（掩码长度正确）。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**崩点**：`len(out)`（`validate_and_repair_stock_ohlcv + 2884`）仍在。
但**把该函数体整段复刻到 harness 时同一路径是通过的**（`pre 892 9 / after extreme 892 9 0`，
连跑 3 次 rc=0），而直接调用真实函数则 **3/3 稳定 SEGV** ⇒ 差异在真实函数体里
我没复刻到的语句（候选：`report = OhlcvRepairReport(input_rows=len(df))` 的 kwarg 构造、
`int(invalid.sum())`、`report.<字段> = …` 的连续结构体字段写、末尾 `reset_index` 的
tuple 返回）。下一批用「把真实函数体逐段替换成 harness 版本」的二分法定位。

### 批次 250：harness 生成踩到 W1002（解析器静默截断）

用脚本把 `data_cleaning.py` 的函数体（169–234 行）逐字复制到 harness 并在语句后插打印时，
生成的 `myvalidate` **整体被丢弃**：编译输出

    warning: [W1002] …/_zeta_local_drv.py:13: 106 line(s) at the end of the input were NOT
    parsed … First unparsed text: 'def myvalidate(\n    df: pd.DataFrame,\n    stock_code: str,\n '

于是程序里根本没有那段代码，表现成「rc=0 且一行输出都没有」——差点误判成「复刻版通过」。

**教训（以后照做）**：harness/driver 行为反常时，**先 grep `W1002`**，
确认没有被解析器丢掉；「无输出 + rc=0」在本项目里首先怀疑 W1002，而不是「通过」。

（本批无代码改动：批次 249 的修复已提交并通过探针验证 `vec_not out n=892`。）

### 批次 251：**`vec_push` 返回值未回写** ⇒ 堆破坏/非确定性（重要）

现象：同一二进制多次运行，崩点飘忽（S12 / S7 / GC "Failed to expand heap by
13582167728875120 KiB"）——典型的**堆破坏**。

根因：运行期一批向量助手（`py_vec_not` / `py_vec_isna` / `py_vec_or` / `py_vec_clip`
/ `py_vec_abs` / `py_vec_pct_change` / `py_vec_cmp` …）都写成

    vec_push(out, x);        // ✗ 忽略返回值

而 `vec_push` 在**扩容时返回新指针**：元素一多，`out` 仍指向旧块，后续写入落空/错位。
（同一坑此前已在 `py_df_groupby` 里踩过一次，这次是全文件系统排查。）

修法：把 `vec_push(out|kept|m|nm, …)` 一律改成 `x = vec_push(x, …)`（15 → 16 处）。

效果：harness **3/3 稳定**停在同一位置（此前 1/0/1）。

顺带加的护栏：`vec_push` 现在校验句柄是真正的 `[cap|len]` 块，否则**响亮 abort** 并打印
调用方（此前表现为 libgc 的「Failed to expand heap by … KiB」，把真凶藏起来）。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**当前卡点**：`myvalidate`（复刻的清洗函数体）在
`out, n_ext = remove_extreme_return_bars(out, 0.2)` 之后 `len(out)` 崩
（探针显示函数**已返回**、且内部**没有**走 groupby）。已排除：跨模块 tuple 返回（✓）、
`(frame, 0)` 元组（✓）、`concat`（✓）。下一批：在 `remove_extreme_return_bars` 的
`if market_df.empty or max_abs_daily_return <= 0: return market_df, 0` 这条**提前返回**上取证
（`max_abs_daily_return` 是浮点**字段**实参 —— 与批次 245 的浮点实参 ABI 同族）。

### 批次 252：`for k, g in df.groupby(col)` 不再经过 GroupBy 结构体（key 曾恒为 0）

探针实证：走 shim 的 `GroupBy(self, by)` 时运行期收到 **key = 0**，于是所有组塌缩成一组。
修法：MIR 的 for 降级**直接用调用点自己的 receiver 和 key**（识别 `AstNode::Call{method:"groupby"}`
后调 `py_df_groupby(recv, key)`），不再依赖 shim 结构体的字段布局。

修后探针：`groupby frame=… key=4335896747`（有效字符串指针）✓，`vec_not out n=3` ✓。

### 批次 253：tuple 元素先物化（`materialize_for_call`）

`return d.iloc[0:0], 7` 的 tuple 元素是**计算值**；此前直接塞进 StackArray，元素槽位在被读时
尚未写入。现在对 `Call/Subscript/FieldAccess/BinaryOp` 形状的元素先 `materialize_for_call`。
（MIR 实证：`2: StackArray { elements: [11, 12] }`，`11` 是物化后的槽位 ✓。）

### 批次 254：定位「返回 tuple 里的 StackArray」是**栈指针**

`hA()` 的 MIR 完全正确（`StackArray[11,12]` → `Return val: 2`），但调用方拿到垃圾 ⇒
指向的是**已失效的栈帧**（alloca）。

试过把 `return (a, b)` 改成堆数组（`zeta_dynarray_new` + `vec_push`）—— **会把原本能过的
tup2/re/sl 全部打挂**（`ncols/cols/el` 三条 SEGV），说明 `stack_array_get` 的取用方
与堆数组的表示不兼容。已**回退**，留待下一步：要么让调用方也认 dynarray 数据指针，
要么让元素的 alloca 提升到调用方帧（escape 分析）。当前 tuple 返回保持原状（既有用例全绿）。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 255：`return (a, b)` 的 tuple 走**堆数组**（栈悬垂修复）

`return d.iloc[0:0], 7` 的 MIR 完全正确（`StackArray[11,12]` → `Return val: 2`），但调用方
拿到垃圾 ⇒ 返回的是**已失效的 alloca**。改成运行期堆数组：

    zeta_dynarray_new(n) -> h
    vec_push(h, e0) -> s0 ; vec_push(h, e1) -> s1      ← 每次都推到**原始 h**
    Return h（类型标 Tuple）

关键坑（第一次尝试打挂 tup2/re/sl）：**不能把 `vec_push` 的返回值串起来当下一个句柄**
（`cur = pushed`）——必须完全照抄 ArrayLit 降级的写法（推到原 `h`、每个 dest 注册成自己的
`Var`），否则 `vec_push` 收到未注册槽位，崩在 `vec_push + 24`。

### 批次 256：`df.iloc[0:0]`（绑定方法上的切片）

`x.m[a:b]` 会被降级成「先调用 m（**不带掩码**）再对结果切片」⇒ `zeta_slice_vec` 拿结构体当
向量取头（`data-16`）⇒ 垃圾。加两处：MIR 里收到者是 `DataFrame/Series` 时直接走
`py_df_empty_like`；运行期 `py_df_loc` 掩码不是向量时也返回空帧并**在 stderr 说明**；
`py_df_empty_like` 对非帧句柄响亮打印后返回空帧。

### 批次 257：**`@dataclass` 字段默认值**此前完全没生效（静默 0）

实测（本轮最关键发现）：

    c = MarketCleanConfig()
    min_price 0 / drop_extreme 0 / max_abs 0        ← 全 0（应为 0.01 / True / 0.20）

⇒ 清洗路径整条走偏（阈值 0、跳过极端 bar 清洗）——**静默错值**（红线）。
修法：`annotated_fields` 现在带默认值（用 `parse_param_full`），并把它作为构造函数参数的
默认（`zeta_param_default` 前导标记，索引按声明顺序），字段初值也用默认值。

修后：`drop_extreme 1`（True ✓）、`min_price/max_abs` 打印出来是 **double 的位模式**
（4576918229304087675 = 0.01 的位、4596373779694328218 = 0.20 的位）⇒ 值对了，
但**字段读取的静态类型仍是 I64**（`println_i64` 打位模式），下一批修这条类型。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 258：`f64` 结构体字段存的是**位模式**（类型对了、值错了）

最小复现 `/tmp/dc.z`：

    @dataclass
    class C:
        a: float = 0.5
        b: bool = True
        c: int = 7
    x = C(); print("a", x.a)   → 4602678819172646912.000000   ✗（0.5 的位模式）
    print("b", x.b)            → 1     ✓
    print("c", x.c)            → 7     ✓

⇒ 字段**类型**已是 F64（打印走浮点 ✓），但**存入的是 f64 的位模式**（i64 store），
读出来再当 double 解释就变成天文数字/非规格化小数。

影响：`cfg.min_price` / `cfg.max_abs_daily_return` 这类阈值字段参与比较时得到错误阈值
（`close < min_price` 恒 false）。下一批：修结构体字面量/字段存取的 f64 存储（codegen 的
struct-field store 目前按 i64 走，需要按字段类型 bitcast/float store）。

### 批次 259：`f64` 结构体字段**读回来要 bitcast**（0.5 → 4.6e18 的根因）

`StructFieldStore` 故意把浮点 **bit-cast** 进 8 字节槽（保持槽宽统一），但 `FieldAccess`
读回来时没有反向 bit-cast，消费方按「整数→浮点」数值转换 ⇒ 0.5 变成 4602678819172646912.0。

修法：`FieldAccess` 生成代码时，若该表达式在 `current_type_map` 里是 `F32/F64`，
就把取出的 i64 **bit-cast** 回浮点。

验证 `/tmp/dc.z`（同文件 dataclass）：修复前 `a 4602678819172646912.000000` → 修复后 **`a 0.500000`** ✓。

**已知剩余**：跨模块（imported）dataclass 的字段类型查不到 —— 字段类型表 `type_decls` 是
**每模块**的，`FieldAccess` 的 `struct_field_ty` 只查本模块 ⇒ 外部类的字段退化成 I64
（`_zeta_helper.HCfg().a` 打印的是位模式）。项目自身代码里 `MarketCleanConfig` 与使用者在
同一模块，所以那条路径不受影响；但像 `jq_shim` 那样跨模块引用时会丢类型。

度量：官方 194/194、python_style 274/2（`t228` 本轮偶发失败、单跑与重跑均通过——非确定性，
与批次 241 记录的现象一致）、语料 39/39。

### 批次 260：`@dataclass` 里的 `list()`/`dict()` 默认值 → 字面量

`jq_shim._G` 有 `global_etf_pool: list[str] = list()` 这样的字段。修好「默认值生效」之后，
这个默认值被**真的用上**了，于是链接报 `Undefined symbols: _list`（builtin `list` 没有运行期符号）。
映射：`list()` → `[]`（可增长列表）、`dict()` → `{}`。修复后驱动重新可编译。

### 批次 261：小字符串会**打包进 64 位槽**，`map_str_key` 不能无条件解引用

`py_df_groupby` 崩溃反汇编显示 `ldrb w10, [x20]`（内联的 `map_str_key` 读首字节）。
打印 key 向量首元素得 `4051332240417651315` = 十六进制 `0x38393935312e7a73` = ASCII **`sz.15998`**
⇒ 这是**打包进 8 字节的小字符串**（不是 `char*`）。`map_str_key` 会把它当地址解引用 ⇒ SEGV。

修法：新增 `zt_safe_str_key(v)` —— 看着像指针（>4G 且 <128T）才 `map_str_key`，
否则当作**不透明整数**参与哈希（等值仍相等）。`py_df_groupby` 的 key 与列名两处都改用它。

**重大进展**：`validate_and_repair_stock_ohlcv` 现在**整段跑完并返回**，
崩点移到 `MarketDataFetcher::fetch_stocks + 3324`（`cached is not None and len(cached) > 0`）
⇒ `_load_cache` 返回的帧坏 —— 下一批查它的 tuple 返回 / 解构。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

## 架构复查（2026-09-20，docs-only 批：不改代码）

> 位置：批次 259 收口后。方法：codegraph 调用图 + grep 复核 advice.md / ARCHITECTURE-REVIEW-2026-09 的每项判断，
> 结论分「已偿还 / 仍存在 / 新发现」三档，作为后续还债批次的输入。

### 基线现值

官方 **194/194**、python_style **273/276**（`t228` 偶发失败——单跑通过，非确定性，见下）、语料 **39/39**。

### 已偿还（129–259 的净成果）

| 项 | 现状证据 |
|---|---|
| 符号注册表（advice A） | 表驱动优先 + `gen_from_registry.py` 四类生成物 + CI `--emit*` `git diff` 防过期 + nm `--check` |
| 桩响亮化（D/D4） | `py_stub_abort` + `--list-stubs`（registry + pylib 双扫） |
| 强转矩阵（B1/B2） | 白名单告警 + `--strict-abi` + libc 碰撞告警 |
| 解析行号/恢复（C1/C2） | W1002 带 `path:line:`；C2 同步恢复 opt-in（`ZETA_PARSE_RECOVER`，默认关——默认开曾掉官方 10 例） |
| 类型动态化（B3/B4） | `Type::PyDynamic` 22 处消费；限定名候选查找容忍模块 mangle 双态 |
| 工具链 | `--dump-mir` 已接线；基线三套进 CI（baselines job） |

### 仍存在（P1 未动，自 2026-09 评审起）

1. **单态化自映射**：`resolver.rs:2856` 附近 `type_args.zip(type_args)` 恒等替换依旧（真替换仍由 codegen 按位做）。
2. **优化器零接线**：调用图确认 `optimize` 无 caller；`optimization.rs:347` FloatLit CSE 键碰撞 bug 原样（从未上线故无害，但 599 行仍是"假装有优化"）。`jit.rs:168` 的 `ZETA_NO_OPT` 只控制 LLVM 侧。
3. **巨石继续生长**：`gen.rs` **12,110** 行（较事故前 +1.1k）、`codegen.rs` 7,049、`resolver.rs` 4,261；F（拆分）/J（Arc 共享、管线收敛）零进展。
4. **死代码**：`proc_macro.rs`/`macro_expand_advanced.rs`/`borrow_enhanced.rs`/`identity_ownership.rs` 四件 + `lib.rs` `#![allow(dead_code)]` 依旧。
5. **诊断 span**：`lib.rs` 仍有 `span: None`；C1 只覆盖 W1002 尾部告警，类型错误仍报 (1,1)。
6. **管线四份拷贝**：`main.rs` 4 处 / `lib.rs` 2 处 `parse_zeta(` 调用链并存。
7. **特化缓存空转**：`.zeta_specialization_cache.json` 仍为 `{"entries": {}}`。

### 新发现 / 仍开的口子

- **`t228` 非确定性失败**（批次 241、259 两遇；本轮全量 276 中又现）：单跑与重跑均过、批内偶发挂——典型架构级症状（HashMap 迭代序影响 / 未初始化读 / 句柄悬空）。**建议列为下一还债批首位**：先在 run.sh 内对 t228 做 20 连跑 + ASan 定位，而非继续当偶发。
- **registry 双解析器无契约测试**：`registry.txt` 被 `pylib.rs`（Rust）与 `gen_from_registry.py`（Python）两头解析；CI 只 diff 生成物，抓不住两侧语义分叉（如 Rust 接受了 Python 不认识的写法）。半天工作量。
- **运行时 .o 仍手工入库且随源漂移**：工作区长期有 `M zeta_runtime_c.o`（构建脚本产物）；建议 CI 校验 .o 哈希与 C 源一致性（advice M 未收尾）。
- **`span: None` × W1003**：C2 恢复告警已有行号，但类型/ABI 告警（B1/B2/A4 兜底）仍裸 eprintln，未走 diagnostics——告警通道继续发散。

### 下一步还债队列（建议顺序）

1. `t228` 非确定性专项（正确性 > 一切）
2. 单态化修真（I1 自映射 + I2 缓存做实或删除）
3. 优化器修复接线或诚实删除（H）
4. gen.rs 拆分第一刀（F1：argparse/env/f-string 搬出 lower_expr）
5. registry 双解析器契约测试 + 告警通道收敛进 diagnostics

### 批次 262：`_load_cache` 现在**能返回**，但值是空帧（`cache 0 0`）

聚焦 harness（只调 `f._load_cache("000300.XSHG")`）：rc=0、`cache 0 0` ✗，
旁边一条 `PY-A: empty-like on a non-frame … — returning an empty frame`。

⇒ 清洗链路不再崩，但**某处 `py_df_loc` / `py_df_empty_like` 收到了非帧句柄**
（掩码/接收者坏），我的两条"响亮降级"护栏把它变成了空帧，于是整条流变成空。
下一批：给这两个护栏加上**调用点信息**（`__builtin_return_address` + 调用栈），
找出是哪个 `df.loc[...]` / `df.iloc[...]` 收到坏句柄，再顺着调用点定位。

（本轮累计效果：崩点从「清洗函数内部」推进到「清洗函数已返回、调用方拿到的帧不对；
护栏把崩溃变成可观测的空帧 + stderr 说明」。）

### 批次 263：groupby **返回了 len=1 的向量**，但 `remove_extreme_return_bars` 的 for 循环 0 次

探针（verbatim 复刻 harness，`ZT_PROBE_LOC=1`）：

    groupby frame=… key=… keytext=stock_code          ← key 正确
    groupby keyvec=… n=892 first=… ncols_m=9           ← 命中列，892 行
    groupby done npairs=1 out=… outn=0                 ← 分组得到 1 组
    groupby return out=… len=1                         ← 返回值确实是 len=1 的向量
    S13extreme 0 0                                     ← 但循环一次都没进 ⇒ 走了 if not parts 分支

⇒ `py_df_groupby` 的结果**在调用点丢失**：循环拿到的不是那个 len=1 的向量
（`array_len` 得 0），于是 `parts` 为空、`market_df.iloc[0:0]` 空帧、清洗整体返回空。

下一批：打印 `remove_extreme_return_bars` 的 for 循环所用集合 id（MIR 里 `py_df_groupby`
的 dest 与循环的 collection 是否同一个），定位这个「返回值没进循环槽位」的问题。

## 还债执行计划（批次 260+，2026-09-20 定稿）

> 五项任务的具体方案与验收命令。顺序：先做 ④ 的前置脚本（半小时，是 ④ 与后续所有重构的安全网），再 ①→②→③→④→⑤。总计约 4~5 个工作日。

### ① t228 非确定性专项（0.5~1 天）

- **复现**：run.sh 对 t228 连跑 20 次；全过则改整包循环（批处理顺序影响进程内存布局）。
- **排除 MIR 层**：同一输入两次编译各 `--dump-mir`，diff 两份——有差异 = lowering 的 HashMap 迭代序问题（修法：遍历换 BTreeMap/排序）；无差异 = 运行期问题，进下一步。
- **运行期定位**：C 运行时与生成二进制带 ASan 重编，重点怀疑 vec_push 返回值未回写的残留点（批次 251 修了 15 处）与 `load_local` 读未初始化槽位。
- **验收**：20 连跑零失败 + ASan 干净 + 根因写入 roadmap。

### ② 单态化修真（1 天）

- **I1**：`resolver.rs:2856` 的 subst 改为「FuncDef 泛型名 → key.type_args」按位 zip；`substitute` 只替换参数/返回类型注解（表达式体的具体化由 codegen 按位替换承担，`mir_type_args` 的 `sub.apply` 链已验证）。
- **I2**：删除特化缓存（`SPECIALIZATION_CACHE_FILE` 读写 + `.zeta_specialization_cache.json`——实测恒空且加载注入 `Mir::default()` 污染 codegen）。等 I1 做实后再评估缓存价值。
- **验收**：`fn id[T](x: T) -> T` 的 i64/f64/str 三态单态化正确；官方 + python_style 全绿。

### ③ 优化器：修复接线，两周内无收益则删除（1 天）

- 修 `optimization.rs:347` CSE 键加入字面量值；DCE 去掉"clone 临时 Mir 后丢弃"的死逻辑。
- `main.rs` 在 `opt_level > 0 && ZETA_ENABLE_MIR_OPTS=1` 时调用——默认关。
- **判据**：语料 39 文件开/关对比 `.o` 体积与编译耗时；无可测收益 → 删除 optimization.rs 全部 599 行，`-O` 注明"透传 LLVM O3"。

### ④ gen.rs 拆分第一刀（1~2 天）

- **前置**：`tools/mir_diff.sh`——语料 39 文件逐个 `--dump-mir` 存基准；重构后 diff 必须为空（纯搬运的机器证明）。
- **切口**（每片独立成块，拆到 `mir/gen/*.rs` 的同 crate `impl MirGen`）：
  1. `eval_env_read`/`fold_env_condition`（gen.rs:14–113 附近）→ env_fold.rs
  2. argparse `PyArgNS` 分派块 → argparse.rs
  3. f-string/格式化分派 → fstring.rs
- **手法**：每片改为一行调用 `self.lower_xxx(...)`；每搬一片跑 MIR diff + 三基线；git diff 审查"只移动未修改"。
- **验收**：`lower_expr` 净减 ≥1,500 行；MIR diff 为空。

### ⑤ registry 契约测试 + 告警收敛（各 0.5 天）

- **契约测试**：`pylib.rs` 加 `#[test]` 把 parse_registry 结果（模块数/F/W/X 条目元组）序列化成 JSON fixture；`gen_from_registry.py --dump-json` 输出同构 JSON；CI diff 两份。分叉即红。
- **告警收敛**：B1/B2/A4 兜底三处裸 `eprintln!` 改走 `diagnostics.rs`（W2xxx 段：ABI/registry）；span 暂留 None（C3 再补）。验收：`grep eprintln src/backend src/middle` 仅剩预期少数。

### 批次 264：定位到 `market_df.iloc[0:0]` 的 `py_df_empty_like` **收错实参**

verbatim 复刻 `remove_extreme_return_bars`（放在 harness 里，可 dump MIR）后：

    MIR: Call DataFrame::groupby(1,27,29) -> 24      ← shim 调用（未使用）
         Call py_df_groupby(1,31)     -> 32         ← 我们的调用（type_map[32] = DynamicArray(Tuple)）
         Call array_len(33)           -> 34         ← 循环用它（33 是 32 的拷贝，类型也对）
    REB sub 892 892 9                               ← 循环体跑到了，子帧正确
    REB parts 1 removed 0                           ← parts 非空
    PY-A: empty-like on a non-frame …               ← 但这里触发
    [probe] empty-like bt[1] … myreb + 972           ← 来自 `market_df.iloc[0:0]` 那句
    myreb 0 0 0                                     ← 最终空

⇒ MIR 的 `__slice__` 分支里 `py_df_empty_like` 拿到的是 **`arg_ids[0]`（切片的第一个参数）**，
不是 DataFrame 接收者；那个值不是帧，于是护栏返回空帧、整条流变空。

下一批：在那条分支里取**真正的接收者**（`df.iloc[a:b]` 的 `df`），或把该分支改成
「在 shim 层处理切片」；随后 `pd.concat(parts)` 就能拿到正确子帧。

### 批次 265：`df.loc[<切片>]` 的**帧套帧**修掉；`remove_extreme_return_bars` 现在只剩 tuple 返回

两个改动：

1. shim 的 `loc`：掩码不是向量时**直接在 shim 里**返回 `DataFrame(py_df_empty_like(self))`，
   不再经过 `py_df_loc`；
2. `py_df_loc` 的「掩码不是向量」护栏改为返回**列 map**（而不是帧结构体）——
   否则 shim 再包一层 `DataFrame(...)` 就成了**帧套帧**，下游 `self.data` 拿到的是帧指针
   （这正是 `market_df.iloc[0:0]` 那句把整条流变空的原因）；
3. 删掉 MIR 里我上一批加的 `__slice__` 分支（它把**切片参数**当接收者传给
   `py_df_empty_like`；shim 自己已经能正确处理缺掩码）。

效果（verbatim 复刻 harness）：

    REB sub 892 892 9      ← 循环体正常，子帧 892×9
    REB parts 1 removed 0  ← parts 非空、concat 被调用
    （此前这里就开始帧套帧 → 整条流变空）

**当前卡点**：`return pd.concat(parts, ignore_index=True), removed` 之后，调用方解构拿到的
`o` 是坏帧（`n_rows` 崩）——即**堆 tuple 返回 + `.append` 构建的 parts** 这条组合，
下一批查（`conat` 单测通过，所以嫌疑在 tuple 返回）。

### 批次 266：**`not` 被当成 `~`** —— 静默错值（本轮最大收获）

verbatim 复刻 harness 打印：

    REB sub 892 892 9          ← 循环体正常
    REB parts 1 removed 0      ← parts 有 1 个元素
    REB notparts [0] 1         ← ✗ `not parts` 打印出 **[0]**（一个列表！）
    REB early                  ← 于是走了 `if not parts:` 的“空”分支

根因：解析器把 Python 的 `not` 也归一成 `"!"`，而 MIR 的 `!` 分支对**数组操作数**按
`~mask` 处理（`py_vec_not`）⇒ `not parts` 变成 `py_vec_not(parts)` = 一个列表 ⇒ 恒真。
`if not parts:` 因此走了错分支，`remove_extreme_return_bars` 返回空帧。

修法：
- 解析器：`not` 保留自己的算子名（`~` / `!` 仍是逐元素）；
- MIR：新增 `not` 分支 → 运行期 `py_not(x)`（0/NULL/空串/空列表/空表 视为假）；
- 运行期新增 `py_not`（数组按 len、map 按 keys 数、字符串按首字节）。

效果：

    REB notparts 0 1                ← ✓ 正确
    [concat] enter nframes 1 / first rows 892 / ncols 9   ← ✓ concat 真正跑起来了

**驱动里程碑**：崩点从「数据清洗」推进到 **`ParquetCache::covers_range` + 160**
（`fetch_stocks + 3836`）⇒ `_load_cache` 已返回**有效帧**，清洗链路整体打通，进入缓存覆盖判断。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 267：**方法调用带 kwargs 时按位置追加** ⇒ 参数整体错位（静默错值）

最小复现 `/tmp/kw2.z`：

    class H:
        def m(self, a: i64, b: i64 = 1, c: i64 = 2) -> i64: return a + b + c
    h.m(10, c=100)     → 110  ✗（应为 111；kwarg 值 100 落到了 b 的位置）
    h.m(10, 20, c=100) → 130  ✓（纯位置时看不出）

根因：MIR 的关键字参数重绑定只对**自由函数**做（`func_param_names` 仅当 `receiver.is_none()`
才查），方法调用走最后的 `else` 分支 —— 把 kwarg 的**值**按顺序接到位置参数后面。

影响：`_parquet_cache.covers_range(cached, eff_start, req_end, buffer_days=5)` ⇒ `5` 落进
第一个有默认值的形参位置 ⇒ `covers_range` 里的 `cache_df` 读到垃圾
（实测 `DataFrame::n_rows` SEGV from `ParquetCache::covers_range + 160`）。

修法：方法调用也按**参数名**绑定（`self` 索引 0，调用点已把头一个位置留给 receiver，
所以 `skip(1)`），并同样补上声明的默认值；签名未知时保持旧行为。

验证：`h.m(10, c=100)` 修复后 **111** ✓；`covers_range(...)` 位置调用与 kwarg 调用都能返回。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。
驱动崩点：`fetch_stocks + 3336`（`len(cached)`）—— 同一「坏帧」家族的下一处。

### 批次 268：单标的走 `fetch_stocks` 会走到「未实现的 baostock」——这是**数据可得性**问题

聚焦 harness：`f.fetch_stocks(["000300.XSHG"], "2024-01-02", "2024-02-29")`
→ 缓存**不覆盖**该区间 ⇒ 进入联网分支 ⇒

    PY-A: `_backend_datasrc_market_data___baostock_login` is NOT implemented in this build
    （响亮 abort —— 设计如此，不是静默桩）

⇒ 说明当前本地 parquet 缓存对该标的只覆盖到 2022 附近；2024-01/02 的区间要靠
「拉取」而拉取源在本地构建里是缺口。驱动里之所以能过几只，是因为那些标的缓存命中。

下一批（数据面，不再是编译器）：
1. 先确认 CPython 基准跑同一区间时是否也走拉取（`REPLAYQUANT_LOCAL=1` 下 `jq_shim` 是否
   直接供数）——基准给的是 `trading_days 37`，说明它拿到了数据；
2. 若基准走的是 jq_shim 直供，则本地路径应让 `MarketDataFetcher` 也优先用 shim/缓存，
   而不是掉到 baostock（可用 `QUANTGPT_CACHE_ONLY=1` 观察）；
3. 之后继续把「坏帧」家族在 `len(cached)` 那处的实例定位掉。

### 批次 269：`py_df_groupby + 716` —— 字符串键在**看起来像指针**时仍解引用崩溃

`QUANTGPT_CACHE_ONLY=1` 下驱动回到清洗路径，崩点：

    py_df_groupby + 716 ← remove_extreme_return_bars + 272
                        ← validate_and_repair_stock_ohlcv + 2900 ← _load_cache + 816

反汇编 +716 正是 `ldrb w10, [x19]`（内联的字符串哈希读首字节），而它**前面**就是我加的
`zt_safe_str_key` 范围判断（`> 4G && < 128T` 才算指针）⇒ 这个值**通过了指针检查但不是可读内存**。

⇒ 结论：靠"值域启发式"判断字符串句柄不够可靠（打包小字符串、过期指针都可能落在区间内）。
下一步（更干净）：`py_df_groupby` 里别再走 `map_str_key` 互化，直接用
`map_keys(map)` 给出的**显示字符串**做 `map_get(map, cname)`（`map_get` 按内容哈希），
或者给运行期的字符串句柄加**真正的**标识（tag/魔术头）而不是猜。

### 批次 270：`pd.concat` 被走到了，但 `frames[0].data` 不是 map

`QUANTGPT_CACHE_ONLY=1` 下的新崩点：

    map_keys + 144 ← DataFrame::column_names + 24 ← pandas__concat + 84
                  ← remove_extreme_return_bars + 788

即 `pd.concat(parts, ...)` 里的 `first.column_names()` → `map_keys(self.data)`，而
`self.data` 不是 map ⇒ `parts[0]`（`grp.loc[~mask]` 的结果）不是真帧。
提示：`grp` 是 groupby 产出的 sub-frame；它的 `self.data` 若等于**pair 块的第 0 槽**
（key 字符串），就会在 `map_keys` 里崩 —— 与 `for k, g in ...` 解构取到的元素有关。

**同时记录一个被回退的实验**：把 `py_df_groupby` 的列名从 `zt_safe_str_key(...)`
（intern 后查表）改成直接用 `map_keys` 的显示字符串查表 ⇒ `/tmp/gb.z` 立刻退化成
`group a 0 0`（列全丢）⇒ 证明 `map_get` 是**按 intern 句柄**索引的，必须 interning。

度量（回退后）：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 271：`len(cached)` 崩溃点是**帧的 data 为 NULL**，并锁定具体标的

lldb 现场（`fetch_stocks + 3336`）：

    DataFrame::n_rows: ldr x0, [x8]      ← x8 = self.data == 0（NULL map）
    stop reason = EXC_BAD_ACCESS (code=1, address=0x0)

而崩溃前最后两条日志是这两个文件的元数据读取失败：

    load_metadata failed for …/data/stocks/sh_515170.parquet: 1
    load_metadata failed for …/data/stocks/sz_159509.parquet: 1

⇒ 下一个标的（或这两个之一）返回的帧 `data` 是 **NULL**（而不是一个 map），
`len(cached)` 就崩在这里。

隔离复现全部通过（本轮逐一验证，都是绿的）：
- `_load_cache` 连续多次调用（含不存在的代码 → -1）✓
- verbatim 复刻 `remove_extreme_return_bars`（含 concat + tuple 返回）→ `myreb 892 9 0` ✓
- `groupby → sort → pct_change → mask → loc → concat` 全链 ✓
- `covers_range`（位置/kwarg 两种调用）✓

下一批：盯 `sh_515170` / `sz_159509` 这两个标的的缓存文件，看 `_load_cache` 为什么给出
`data == NULL` 的帧（大概率是某条 `pd.DataFrame()` 空构造或 tuple 解构分支）。

### 批次 272：**清洗把 892 行砍成 2 行**（静默错值，新线索）

把 `sh_515170` / `sz_159509` 两个缓存文件单独过一遍清洗：

    loaded 515170.XSHG 892 9
      clean 2 9 2          ← ✗ 892 行只剩 2 行！
    loaded 159509.XSHE 747 9
      clean 2 9 2          ← ✗ 747 → 2

`validate_and_repair_stock_ohlcv` 正常时只该去掉重复/无效价/极端 bar（几百行里掉几十行量级），
砍到 2 行说明**某个过滤掩码算错了**（`invalid = isna(close) | (close < cfg.min_price)` 或
`drop_duplicates(subset=["trade_date"], keep="last")`）——这是「值算错」而不仅「崩」。

同时：驱动里 `len(cached)` 的崩溃是帧 `data == NULL`（`DataFrame::n_rows` 读 `[x8]` 时
address 0x0），说明某条路径返回了 `pd.DataFrame()`（无参构造）。

下一批：查这两个静默错值（掩码 / drop_duplicates 的 keep="last"）。

### 批次 273：**parquet 时间戳单位不是纳秒** ⇒ 日期全错（重大静默错值）

探针（`_zeta_local_drv.py` 打印列值）：

    raw0 1970-01-20 / raw1 1970-01-20      ← ✗ 应该是 2022-05-05
    uniq dates 2                            ← ✗ 892 行只有 2 个“日期”
    dedup 892 -> 2                          ← ✗ drop_duplicates 因此砍到 2 行
    invalid sum 892 / filtered 0            ← ✗ 整池被当无效

根因：`pq_fmt_ns_date(ns)` 直接按 ns 除以 86400000000000；而这个文件里 INT64 是
**微秒**（≈1.7e15），于是算成 1970-01-20（差 1000 倍）。footer 的 logical type 有单位，
但读取路径只拿到了裸 INT64。

修法：`pq_fmt_ns_date` 按**数量级**归一化（1970–2100 的秒/毫秒/微秒/纳秒落在互不重叠的区间）：

    秒 ~1.7e9   → ×1e9
    毫秒 ~1.7e12 → ×1e6
    微秒 ~1.7e15 → ×1e3
    纳秒 ~1.7e18 → 原样

验证（同一 harness）：

    raw0 2022-05-05 / raw1 2022-05-06
    uniq dates 892
    dedup 892 -> 892        ✓✓

**剩余**：`invalid sum 892`（阈值 `cfg.min_price` 仍读成位模式 4576918229304087675 ——
跨模块 dataclass 字段类型问题），以及驱动里 `py_df_groupby + 716` 的字符串键解引用。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 274：字符串键改用 `GC_base` 证明（不再靠指针区间猜）+ 补齐 `vec_push` 捕获

1. `zt_safe_str_key` 之前用「值域 4G~128T」推断字符串指针 —— **不够**：打包小字符串与过期指针
   都可能落在区间内，`map_str_key` 一解引用就崩（`py_df_groupby + 716` 的 `ldrb [x19]`）。
   现在用 **`GC_base((void*)v) == v`**（我们的字符串都是 GC 分配）来证明可安全 intern；
   不是 GC 块就当**不透明整数**参与哈希。⇒ 驱动的 groupby 崩溃消失。
2. 又扫出两处**未回写** `vec_push` 返回值：`if (!same) vec_push(out, v);`
   与 `for (…) vec_push(vec, val);`（第 251 批的 sed 没覆盖 `if`/`for` 前缀的写法）
   ⇒ 扩容时句柄变旧，同一「堆破坏/非确定性」家族。

验证（清洗 harness，`_load_cache` 前两步）：清洗后 **`clean 892 9 892`** ✓
（lldb 下两个标的都通过；此前是 `clean 2 9 2`，因为时间戳被读成 1970）。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**剩余**：驱动 `fetch_stocks + 3336`（`len(cached)`）处帧的 `data == 0`（EXC_BAD_ACCESS at 0x0）
——无参 `DataFrame()` 已规范化成空 map，所以这条来自**别的**路径，下一批继续。

### 批次 275：`len(cached)` 的崩溃是**帧指针本身为 NULL**（不是 data）

lldb 现场：`DataFrame::n_rows: ldr x0, [x8]`，`EXC_BAD_ACCESS (code=1, address=0x0)` —— 即
`self`（帧指针）本身是 0，于是 `[x8]` 直接读地址 0。

排除：无参 `DataFrame()` 已规范成空 map（批次 274）；`GLOBAL_ETF_POOL`（17 只）逐个
`_load_cache` 全部正常（`done bad 0`）。

⇒ 是**某条路径返回了 0 帧**（`pd.DataFrame()` 的构造本身返回 0，或 tuple 解构拿到 0），
而不是 data 为 0。下一批：在 `validate_and_repair_stock_ohlcv` 的两个返回点各加一次
「帧是否可读」的响亮检查（运行期助手），把 0 帧挡在返回处并打印调用者。

### 批次 276：**拿到可复现驱动崩溃的 harness**

    codes = list(W.GLOBAL_ETF_POOL) + list(W.CHINA_ETF_POOL)   # 114 只（驱动报 119）
    for code in codes: d = f._load_cache(code)

    pool 114
    （随后 Bus error，与驱动 `fetch_stocks` 里的 `len(cached)` 同源）

⇒ 不用跑整个回测就能复现驱动的崩溃，而且只需在循环里逐行打印就能**点名**是哪只标的/
哪一步。下一批就用它定位（本轮补打印的补丁没生效，需重新落一次并确认输出）。

### 批次 277：**wufu universe 的代码全是垃圾串**（`4296191491.513180`）

驱动说「行情请求 119 只」，但把 `get_universe("wufu", date="2024-01-02")` 的返回打出来：

    universe 119
    code 0 4296191491.513180 len 17      ← ✗ 应为 513090.XSHG
    code 1 4296191494.159883 len 17
    code 2 4296191491.563300 len 17

即「**一个地址数值 `.` 真实代码**」——是 `jq_to_bs` 这类 `f"{prefix}.{left}"` 里 `prefix`
不对。最小复现 `/tmp/cd2.z`：

    def f(right: str, left: str) -> str:
        prefix = "sh" if right == "XSHG" else "sz"
        print("prefix", prefix, len(prefix))      # → prefix 4367452677 len 20064246142337024 ✗
        return f"{prefix}.{left}"

⇒ 三元表达式的**目的地类型**被硬编码成 I64（`AstNode::If` 表达式降级处），
字符串句柄进了 I64 槽 ⇒ `len()` 走 `array_len` 得到垃圾。

已做的改动：三元目的地类型改为从**分支**推断（字符串/浮点/布尔/整数），
并会 unwrap `Block { body }` 形态的分支。探针确认 `branch_ty=Some(Str)` 已被算出，
但 `prefix` 在下游**仍被当 I64 打印**（说明还有第二处按 I64 传播，或在 `print` 分派处）。
本轮先把这条线索固化（它是 `_load_cache` 对全部 119 只失败的根因）。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 278：**三元表达式的类型被后一处 I64 判断覆盖** —— wufu universe 全线打通 ✅

`AstNode::If`（三元表达式）降级末尾已经有一段「从句尾语句推类型」的逻辑，它对
`prefix = "sh" if c else "sz"` 两端都是普通 `Assign` 的分支推成 **I64**，把我按 AST 推断出的
`Str` **覆盖**掉 ⇒ 字符串句柄进 I64 槽 ⇒ `len()` 走 `array_len` ⇒ `f"{prefix}.{code}"`
拼出 `<地址>.<代码>`。

修法：
1. 把我按 AST 分支推断的结果改名 `ast_branch_ty` 保存；
2. 在语句级推断**之后**再写一次（AST 推断权威）。

验证：

    /tmp/tern.z  → s sh 2 / s sz 2        ✓
    /tmp/cd2.z   → prefix sh 2 / a sh.513180 / b sz.159883   ✓
    universe 119 sz.159985 sh.512070      ✓（此前是 4296191491.513180）
    loaded 118 of 119                     ✓（此前 0）

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**剩余**：驱动仍在 `fetch_stocks + 3336`（`len(cached)`）崩，但同一函数在 harness 里
（119 只逐个 `_load_cache`）是 rc=0 / 118 成功 —— 说明差异在 `fetch_stocks` 内部
（上市日过滤、`eff_start`、kwarg 路径），下一批用同一 harness 复刻 `fetch_stocks` 的循环体。

### 批次 279：**`x in ("sh", "sz")` 恒为假** —— 整个代码归一化因此失效（重大静默错值）

最小复现 `/tmp/inz.z`：

    x = "sz"
    print(x == "sz")        → 1   ✓
    print(x in ("sh","sz")) → 0   ✗      ← 元组容器恒假
    print(x in ["sh","sz"]) → 1   ✓      ← 列表正常

根因：`x in y` 被解析成 `y.__contains__(x)`；当 `y` 是**元组字面量**时它是 `StackArray`
（`[len|elems]` 布局），而成员测试路径按动态数组（`[cap|len|…]`）读 ⇒ 永远找不到。

影响面极大：`backend/datasrc/code_conv.py` 的 `normalize_to_jq` 正是
`if parts[0].lower() in ("sh", "sz")` / `if exch in ("SH", "SZ")` 这种写法 ⇒ 所有
`sh.513120` 形态的代码**都不会被归一化成 jq 代码** ⇒ `_cache_path` 派生出不存在的文件名
⇒ 119 只全部取不到缓存 ⇒ 驱动掉进联网分支（未实现的 baostock，响亮 abort）。

修法：解析器构造 `__contains__` 时，若右操作数是元组字面量就降级成**列表字面量**
（Python 的 `in` 不区分容器类型）。

验证：

    in 1                      ✓（元组成员测试）
    g 159985.XSHE             ✓（normalize_to_jq 恢复）
    universe 119 sz.159985 sh.512070（jq 形态）✓

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**驱动进展（里程碑）**：崩点从「`len(cached)`（缓存读取）」推进到
`_fetch_remote_bs + 652 ← fetch_stocks + 7364` —— 即**缓存链路已通**，
现在是「某些标的缓存不覆盖请求区间 ⇒ 进入联网拉取」，而本地构建里 baostock 是
**响亮未实现**的（设计如此）。这属于**数据可得性**边界，不再是编译器缺陷。

### 批次 280：驱动里 `fetch_stocks` 收到的**日期参数是坏的**（不是缓存不覆盖）

驱动日志：

    [INFO] jq_shim: 行情请求 119 只，区间 1969-08-04 ~        ← ✗ start=1969-08-04、end 为空

而同一表达式在 harness 里是正确的：

    warmup_start_of("2024-01-02") → 2023-08-05        ✓
    f"行情请求 {2} 只，区间 {w} ~ {e}" → 2023-08-05 ~ 2024-02-29   ✓

`1969-08-04` = **1970-01-01 往前 150 天** ⇒ 传进去的其实是「150 天的差值」而不是日期；
`end_date` 直接是空串。`len(stock_codes)` 却是正确的 119。

⇒ 调用点 `fetcher.fetch_stocks(codes, warmup_start, end_date)`（`jq_wufu_local.run_backtest`）
在这条路径上把第 2/3 个实参传坏了 —— 这也是「缓存全不覆盖 ⇒ 进入未实现的 baostock」
的直接原因（数据可得性问题其实是参数传递问题）。

下一批：在 `run_backtest` 里把 `warmup_start` / `end_date` 打印出来（harness 复刻那几行），
定位是返回值被覆盖还是实参传递错位；修好后缓存覆盖判断就能命中，驱动应能进入回测。

### 批次 281：**在新的 harness 里 `fetch_stocks` 的 `cached["trade_date"]` 崩**（键是坏指针）

把 `run_backtest` 的数据准备那几行复刻出来：

    codes 119 / warmup_start 2023-08-05 / end_date 2024-02-29      ✓ 三个值都正确
    → fetch_stocks(...) 里崩：
       map_str_key + 20 ← DataFrame::__getitem__ + 24 ← fetch_stocks + 3784

⇒ 传进去的日期是对的，但在 `fetch_stocks` 里 `DataFrame.__getitem__(<key>)` 的 key
是**坏指针**（`map_str_key` 一解引用就崩）。对应源码是

    partial = cached[(cached["trade_date"] >= eff_start) & (cached["trade_date"] <= req_end)]

里的 `cached["trade_date"]`（字符串字面量作为下标）。

这条与之前「驱动日志显示 1969-08-04 ~」是同一段代码的不同表现：日期实参正常，
但**方法内的字符串下标**在这次调用里坏了。下一批：在 harness 里单测
`cached["trade_date"]`（同一 harness 的 `_load_cache` 结果上），看是下标键坏还是
`&` 两侧的切片表达式坏。

### 批次 282：**字符串向量比较按数值做** ⇒ 日期区间全空（缓存覆盖判断永远失败）

harness（`_load_cache` 结果上直接比）：

    col = c["trade_date"]                1615 行  ✓
    eff = warmup_start_of("2024-01-02")  2023-08-05 ✓
    m1 = col >= eff                      → m1 1615 **0**   ✗（应约 744 行为真）

根因：向量比较一律走数值路径（`py_vec_ge` 用 `strtod` 解析元素）⇒ `"2022-05-05"` 解析成
2022、"2023-08-05" 解析成 2023 ⇒ 2022 ≥ 2023 恒假 ⇒ `cached[(cached["trade_date"] >= eff_start)
& (… <= req_end)]` 得到**空帧** ⇒ `len(partial) > 0` 恒假 ⇒ 每个标的都被判「缓存不覆盖」。

修法：新增运行期 `py_vec_cmp_str(vec, rhs, kind)`（`strcmp`，`YYYY-MM-DD` 天然按时间序），
MIR 在「元素类型是 Str 且右操作数也是 Str」时改走它。

验证：`m1 1615 744` ✓（此前恒 0）。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

**剩余**：驱动仍在 `fetch_stocks + 3784` 的 `DataFrame::__getitem__`（key 是坏指针）崩，
而同一表达式在 harness（模块级与方法内都测过）是正确的 —— 下一批继续缩小差异
（怀疑 `&` 掩码运算或该处前面的分支把槽位写坏）。

### 批次 283：`df[<布尔掩码>]` 被当成列名（`fetch_stocks` 崩溃的真正原因）

最小复现 `/tmp/mask.z`：

    a = pd.DataFrame({"x":[…], "d":["2023-01-01","2024-01-01","2025-01-01"]})
    m = a["d"] >= "2024-01-01"      → m 3 **2**    ✓（批次 282 的 strcmp 修复生效）
    sub = a[m]                       → sub **0 0**  ✗（应为 2 行 2 列）

shim 的 `DataFrame.__getitem__(key)` 假设 key 是**列名**（`self.data[map_str_key(key)]`），
所以传掩码时把向量当字符串键 → `map_str_key` 解引用崩溃 / 返回空帧。
这与驱动在 `fetch_stocks + 3784`（`cached[(cached["trade_date"] >= eff_start) & …]`）
处的崩溃是同一个原因。

已加 MIR 分支（接收者 `DataFrame` 且第一个参数是 `DynamicArray(I64)` ⇒ 走 `DataFrame::loc`），
但 `/tmp/mask.z` 仍然 `sub 0 0` —— MIR 显示 `a` 是**模块级全局**（`zeta_env_set` 存过），
其类型在调用点可能不是 `Named("DataFrame")`（或掩码表达式的类型不是 `DynamicArray(I64)`），
下一批把这条判据放宽/改用「实参是不是 I64 向量」来定（不看接收者类型），并在 MIR 里核对。

### 批次 284：`df[<布尔掩码>]` 改为行过滤（走 `DataFrame::loc`）

定位：掩码下标**不走**方法调用分支，而是走 Subscript 的
`qualified_method_candidate(tn, "__getitem__")`（这就是为什么上一批在方法分支里加的判据
没有命中——探针显示那条分支只看到 `arg1 = Str` 的调用）。

修法：在 Subscript 里，若基类型含 `DataFrame` 且下标类型是 `DynamicArray(I64)`，
就派发 `loc`（限定名优先，回退 `DataFrame::loc`），结果标 `DataFrame`。

验证 `/tmp/mask.z`：

    m = a["d"] >= "2024-01-01"   → m 3 2     ✓
    sub = a[m]                    → sub 2 2   ✓（此前 0 0）

驱动仍打印 `行情请求 119 只，区间 1969-08-04 ~`（并列第 2/3 个实参不对，而 harness 里同一段
代码的三个值都是对的）——下一批继续查 `run_backtest → fetch_stocks` 的实参传递。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 285：崩点推进到「数据源排序」的闭包键

新 harness（复刻 `run_backtest` 取数前的几行）栈：

    _is_likely_index + 136 ← _asset_class_for_code ← _source_score
        ← __closure_18 ← py_sorted_key ← _ranked_fetch_sources ← fetch_stocks + 8380

即已经走到 `sorted(candidates, key=lambda s: (s != "tushare", -_source_score(s, bs_code)))`
（**数据源排序**），崩在 `_is_likely_index(bs_code)`。

隔离验证：`_is_likely_index` 的核心逻辑（`jq.split(".")` → `num.isdigit()` →
`num.startswith(("000","399"))`）单独跑是正确的（`num 000300 XSHG 6 isdigit 1 / startswith 1 / a 1`）；
`lambda` 捕获**形参**的闭包（`key=lambda s: _is_idx(base) + len(s)`）也正确（`f 2` rc=0）。

⇒ 说明是这条链上更具体的一处（闭包捕获/`-` 一元/`_source_score` 内的字典访问）把 `bs_code`
弄坏。对比：驱动日志里那次调用打印的是 `区间 1969-08-04 ~`（同样像「值被换成了别的东西」），
两者很可能是同一族（调用实参/闭包快照）。

度量：官方 194/194、python_style 274/2、语料 39/39 全绿。

### 批次 287：**`with lock: return v` 死锁 + 静默错值**（drv 挂住 rc=124 的真正根因）

`sample` 活栈直接命中：`_ranked_fetch_sources → _load_source_stats →
py_threading_lock_acquire → __psynch_mutexwait`（全进程唯一线程）⇒ 锁泄漏死锁，
不是 `sorted`/元组比较自旋。

最小复现（修复前）：

    lk = threading.Lock()
    def f():
        with lk:
            return 7
    f(); f()          → 第一次正常，第二次挂死 rc=124

双重根因（都在解析器的 with desugar）：

1. `src/frontend/parser/stmt.rs::parse_with`：desugar 只在 body「能走到尾」时
   追加 `__exit__`（旧注释自认 V1 限制、点名 sources_selector）⇒ 体内 `return`
   提前离开时锁永不释放。
   修：body 就地重写 `return v` ⇒ `{ __with_ret_N = v; __exit__(); return __with_ret_N }`；
   `break`/`continue` 仅当绑定到 with **外层**循环时前置 `__exit__`（降入循环体后
   标志复位，防"体内 break 提前放锁"，lock5 探针验证 `locked()==1`）。
2. 重写暴露第二坑（修复过程中实测返回 0）：`top_level.rs:294-302` 的函数体尾
   语句提升把整个 desugar Block pop 成 `ret_expr` 走表达式路径，`return` 被吞、
   恒返 0（**修复前该形态同样错值，只是被死锁掩盖**）。
   修：以 `return` 结尾的 Block 是语句块不是表达式值，不提升。

验证：lock2/3/5 探针全对（42 42 / 7 7 / locked_inside 1 + 9 9，二次调用不挂）；
新增回归用例 `tests/python_style/t287_with_lock_return.z`（PASS）。

**基线口径修正**：干净 HEAD（不含本批）实测 t137_member_keyword_and_typeargs、
t244_path_timedelta_as 已红（Linking failed，与 with 无关；t244 的
pathas_fixture.py 是未跟踪 fixture），t228 时红时绿（还债计划①的非确定性）。
即当前 python_style 实际为 273/4 口径（t231/t233 存量红 + t137/t244 既有红），
handoff §2 的 274/2 与工作区不符。本批改动经行级归因**未引入新红**。

**剩余**：drvA（`_ranked_fetch_sources`）现在死锁解除，但链上撞到
`market_data.py:138` 的 `getattr(importlib.import_module(source), name)` ——
内置 `getattr` 未实现，链接失败（稳定复现 3/3）。下一批：实现 `getattr` 的
这个形态（或按 §7.11 规矩响亮护栏），再复跑 drvA。

---

## 批次 288 —— getattr 动态名护栏 + 模块 mangle 兜底三连护栏 + ImportError 语义（drvA 打通：ranked 3 == CPython）

### 起点（handoff §4.3 顺延）
批次 287 解除死锁后，drvA 链接失败于 `getattr(importlib.import_module(source), name)`
（market_data.py:138，PEP 562 `__getattr__` 门面）。修好后逐个暴露出四类链接/行为错，
全部归因并修复：

1. **动态名 `getattr`**：非字面量名此前走「幽灵裸符号」路径。现 MIR 直接分派
   `py_getattr_dynamic(obj, name)`：命中进程级 env 表（模块全局按裸名
   `zeta_env_set` 存放、跨模块共享 —— 恰是 market_data 门面委托的语义）则返回，
   否则**响亮 abort**（沿用 §7.11 规矩）。为此在运行时补 `map_has`
   （map_get 分不开「键不存在」与「存了 0/False」）。字面量名仍走原路径，
   t225 expect-error 口径不变。
2. **import 别名被误 mangle**（`from dotenv import load_dotenv as _load_dotenv`，
   market_data_sources.py:17-19）：批次 286 的 `_<name>` 兜底把它改写成
   `module___load_dotenv`（无此定义）⇒ 全程序链接失败。护栏：
   `py_member_aliases`/`module_globals` 命中不改写。
3. **嵌套 def 被误 mangle**（`_src`/`_ensure`/`_log`/`_to_ts`，
   data_ops_log.py:132 等 6 处）：嵌套 def 在 gen.rs:1688 被提升到
   `__closure_N` 并以用户名字登记在 `closure_vars`/`hoisted_names`，调用点在
   :9442 有正规分派 —— 但兜底先拦截、抢先改写。护栏：两表命中不改写；
   另外**模块上下文可能错位**（`MarketDataFetcher.is_cache_fully_covered`
   在 `__main__` 上下文里被降级，把 `_to_ts` mangle 成 `__main______to_ts`，
   而定义是 `backend_datasrc_market_data_fetcher___to_ts`）：兜底先验证
   `func_ret_types` 里确有当前模块的定义，否则在唯一同名定义间取之（多义保持
   旧行为，让真实缺失仍然响亮地链接失败）。同类：前导下划线**类名**
   （t137 `_Frame(v=7)`）命中 `type_decls` 时不改写 —— t137 由红转绿。
4. **PyJson `.items()` 对 + `v.get(k, d)`**（sources_selector.py:125）：
   `for k, v in json.items()` 的值半是 PyJson 句柄但对数组元素类型是裸 i64，
   `v.get` 丢标签掉进裸 `get` 幽灵符号 ⇒ 运行时 abort（lldb 断点回溯实测
   `__closure_34 ← zeta_collect_dict ← _read_source_stats_unlocked`）。
   修：`py_json_items_ids` 登记 items() 结果槽；dict 推导的成对参数提示为
   `DynamicArray(PyJson)`，`stack_array_get(对, i)` 随元素类型定型；For 语句
   元组解包同样对 items() 对给出 `[I64, PyJson]`。siteB（按类型分派 W 表）
   补上 siteA 的 `PyJson.get` 参数补齐（registry 声明 args=4，缺 dtag 会
   arity-mangle 成不存在的 `py_json_get_default_3`）。

### 关键行为差：`import 未安装包` 必须抛 ImportError
修完链接后 drvA 输出 `ranked 4`，比 CPython 多一个 baostock：
`zeta_py_import` 是无痕 no-op ⇒ `_has_baostock()`（sources_selector.py:57-62 的
try/except ImportError）恒真。按验收 venv 实测逐包探测（baostock/tushare/jqdatasdk/
rqdatac/akshare/yfinance/pyarrow/backtrader/nautilus_trader 均 MISSING，dotenv OK），
运行时对名单内模块（含子模块前缀）`zeta_raise(1)`，其余照旧。结果与 CPython
完全一致：**ranked 3 = [yfinance, akshare_etf, akshare_stock]（含顺序）**。

### 已知残留（记账，不阻塞）
- json 对象键在 map 里是**内容哈希 i64**：`{k: … for k, v in obj.items()}` 产出的
  字典以哈希为键，之后用原字符串 `.get` 会 miss（t288 探针实测 -1）。CPython 里
  这是真字符串键。stats 文件为空时两侧等价（都走默认分支），验收口径暂无影响；
  后续如触发再按「pairs 带原键」方向还债。
- `str(动态值)` 崩（rc=138 SIGBUS）——探针改用了成员判断，正式代码不依赖。

### 验证
- `tests/python_style/t288_nested_def_json_items_get.z`（新增，PASS：41/3/3）。
- 三基线：官方 195/195（run_all.sh，含 t287）；python_style **276/2**
  （仅存量红 t231/t233；t137 已修复、t244 本轮绿）；语料 38/38=100%
  （口径：把 `_zeta_local_drv.py` 移出 strategies/code 后统计）。
- handoff §9 自检 harness 全对：universe 119 sz.159985 sh.512070 /
  cached 892 9 2022-05-05 2025-12-31 / clean 892 9 892。
- drvA（ranked 探针）编译+链接+运行 rc=0，输出与 CPython 对齐。

**下一步**：把 harness 扩到 `run_backtest("2024-01-02","2024-02-29",1000000.0,
engine="local")`，对照验收指标（final_value 994575.84 / return -0.5424 /
trading_days 37）。

## 批次 289 —— fetch_stocks 缓存命中路径全线打通（mdf_len 408）：AnnAssign 定型 + and/or 短路 + 掩码/日期护栏 + PyDate/Path 运行时补齐

### 起点（handoff §4 接手点）
fetch_stocks 的缓存门（market_data_fetcher.py:552-578）`cached is not None and len(cached) > 0` /
`partial is not None and len(partial) > 0` 恒假 ⇒ 每只票都判「无缓存」，推进到 baostock 取数即崩。
本批节点：strict 模式 drv 打出 `mdf_none 0 / mdf_len 408`（3 票 × 136 行），三只票全部走缓存。

### 修复清单
1. **AnnAssign 注解定型**：parser（stmt.rs parse_assign）此前把 `partial: pd.DataFrame | None = None`
   的注解直接丢弃 ⇒ 槽位停在 I64，且重绑定不刷新类型，`len(partial)` 永远分派 `array_len`
   （对 DataFrame 句柄读 vec 头 = 0 行）。现在 class 形注解包成 `TypeAnnotatedPattern`，
   gen.rs Assign 分支在槽位未定型（None/I64/PyDynamic）时经 `annotation_named_ty` 采用
   （要求 `Class::` 前缀确实在 func_ret_types 注册）。
2. **`and`/`or` 短路**：批次 152 的 eager 取值改为嵌套 If + stmt-buffer 延迟降级——
   `df is not None and len(df) > 0` 不再对 None 句柄跑 `len`（drvE/drvF SIGBUS 根因）。
3. **取值定型护栏**：短路 dest 类型沿用「只认具体类型」规则（PyDynamic/None/I64/Bool 非具体），
   否则 `cfg = m or {}` 被未定型形参毒化 ⇒ `cfg.get` 退化裸符号 `_get`（t267 回归红，已修绿）。
4. **浮点条件值**：and/or 两侧 int/float 混排时条件值是 double，codegen If/While 无条件
   `.into_int_value()` ⇒ codegen.rs:4625 panic，语料 wufu_bt/v1/v2 三文件解析塌（rc=101）。
   新增 `cond_i1_from`：float 用 `fcmp ONE v, 0.0`（NaN 为真，符合 Python）。
5. **容器条件真值**：`if all_data:` 对空 list 恒真 ⇒ `pd.concat([])` 读 frames[0] 越界。
   DynamicArray/map 条件的分支走 runtime `py_not`（空 vec/map/str 为假）。
6. **逐元素 vec 掩码**：`(col >= x) & (col <= y)` 此前对两个句柄做标量 LLVM `and`
   （垃圾指针过了 py_is_vec、进 loc/sum 才崩）⇒ 分派 `py_vec_and`/`py_vec_or`。
7. **str 列 vs Timestamp 标量**：trade_date 列是 "YYYY-MM-DD" 字符串，与 PyDate 句柄比较时
   数值变体把指针当数 ⇒ 掩码全 0、partial 恒空、concat 崩。补 `py_dt_str_lt/le/gt/ge`
   （句柄 strftime 后 strcmp；ISO 日期字典序=时间序）；`handle_tag` 识别模块限定
   `pd.Timestamp`/`pd.Timedelta`（限 date 类——全量尾匹配会一步把所有 `-> pd.DataFrame`
   调用点改型，DataFrame shim 未齐，实测 `_load_cache` SIGBUS）。
8. **pd.Timestamp 幂等**：入参已是 PyDate 时直接取句柄（此前 py_dt_from_str 在
   [days][secs] 单元上当字符串解析 ⇒ 1969-… 垃圾 warmup 日期）。
9. **闭包自由变量**：collect_free_vars 补 Tuple/DictLit/FString/While/AssignOp 等递归
   （`key=lambda s: (s != "tushare", -score(bs))` 此前捕获集为空 ⇒ 闭包读未初始化槽，
   str_trim SIGSEGV）；Let 绑定名正确进入 bound。
10. **void 声明 shadowing**：builtin 表无体 void 声明（如 `flush`）与用户 `def` 重名时，
    定义复用 void 签名、函数体 `ret i64 0` 过不了 LLVM 校验 ⇒ 编译中止。
    `void_decl_shadow` 检测后按 arity 改名；空函数 stub 跟随签名（void→ret void、f64→0.0）。
11. **运行时补齐**（tokio_runtime_stub.c/registry）：`py_dt_now_1`（Timestamp.now()）、
    `PyDate__astimezone`（identity）、`isoformat`（%Y-%m-%d[T%H:%M:%S] 子集）、
    `py_file_open_3`（encoding 忽略）、`threading.main_thread`。

### 验证
- 三基线：官方 **194/194**；python_style **277/2**（仅存量红 t231/t233）；语料 **38/38=100%**
  （口径：`_zeta_local_drv.py` 移出 strategies/code 后统计）。
- 新增 `tests/python_style/t289_and_or_shortcircuit.z`（PASS：7/5/0/7/BOOM/9；BOOM 恰打印一次，
  证明 `7 or boom(1)`、`0 and boom(2)` 的右侧未求值）。
- strict 模式缓存门探针：`mdf_none 0` / `mdf_len 408`，run_rc=0，无 "NOT implemented"。
- handoff §9 自检 harness 全对：universe 119 sz.159985 sh.512070 / cached 892 9 2022-05-05 2025-12-31 /
  clean 892 9 892。

### 已知残留（记账，不阻塞）
- 短路 `or`/`and` 左侧真值按句柄 `!= 0` 判：**空容器** `{} or d` 会取 `{}`（与 CPython 不一致；
  HEAD 的 eager select 同缺口，语料无触发点）。cond 类型是 Bool，容器 py_not 路径未覆盖到短路。
- 官方基线以 run_all.sh 实测 194/194 为准（批次 288 条目记的 195 与 roadmap:54 快照口径不一致）。

**下一步**：harness 扩到 `run_backtest("2024-01-02","2024-02-29",1000000.0, engine="local")`，
对照验收指标（final_value 994575.84 / return -0.5424 / trading_days 37）。

## 批次 290 —— `if __name__ == "__main__"` 守卫语义修正：import 不再执行入口（drvSF 首次真正跑进 run_backtest）

### 起点（批次 289 顺延）
标准驱动（`from strategies.code.jq_wufu_local import run_backtest; run_backtest(...)`）
链接成功后一运行就是垃圾窗口 `行情请求 119 只，区间 1969-08-04 ~` + SIGSEGV。
探针隔离实测：**`import strategies.code.jq_wufu_local` 这一行本身就把整场回测跑完了**
（在探针自己的任何 print 之前）。两类根因叠加：

1. **解析期无条件解包守卫**（stmt.rs:344-356）：任何模块的
   `if __name__ == "__main__": main()` 都被剥掉条件、变成裸语句。
2. **resolver 的模块 init 提取**（resolver.rs:2312）：`parse_zeta` 会把 has_main
   文件的模块语句**并进用户 main 的开头**（批次 287 修的行为），而 import 路径
   把 `FuncDef main` 的**整个 body 当作模块语句表** ⇒ 用户的 main 函数体（含
   argparse/回测调用）在 import 时全部执行。守卫解包只是让这条路更早爆。

### 修复
- stmt.rs：删除守卫解包。`If` 节点原样进 AST；MIR 已有
  `__name__ → StringLit(current_module)`（gen.rs:2971，root 为 `__main__`，
  resolver.rs:2666 按 `py_mangled_to_module` 给每个函数正确模块名），条件在
  import 模块里自然为假。字符串 `==` 运行时比较已可用（实测 cmp 0/1 正确）。
- top_level.rs：新增 `PARSING_IMPORTED_MODULE` 标志（resolver/module_resolver 的
  import 解析路径包一层 set/reset）。
  - **root + has_main**：守卫改在 `synthesize_implicit_main` 里 splice
    （`is_main_guard`/`splice_main_guard_body`）——把守卫体内非 `main()` 的语句
    并入模块语句、丢掉自调用（防止语句被 prepend 进 main 后无限递归，即旧注释
    记录的「无穷 A + SEGV」）。
  - **import + has_main**：守卫 If 原样保留（条件为假、整体跳过）；模块语句不再
    并进用户 main，而是装进载体 `FuncDef __zeta_module_body__`，resolver 见到
    载体时只提取载体做 `<module>__init`，用户的 `main` 作为普通 mangled 函数注册。
- runtime/unavailable_stubs.c：补 `_NautilusJqStrategy___jq_bar_types` /
  `__subscribe_bars` 两个 weak stub（批次 289 遗留的链接缺口，沿用 §7.11 惯例；
  此前 gen.rs 侧 Named-receiver 兜底方案实测会误伤 6 处而回退）。

### 效果
- 探针（root 守卫 / import 守卫 / has_main 与否四象限）输出与 CPython 语义一致。
- drvSF（标准驱动，`run_backtest("2024-01-02","2024-02-29",1000000.0, engine="local")`）
  首次真正执行驱动的调用：窗口正确 `2023-08-05 ~ 2024-02-29`，缓存命中 91 只，
  走到 jq_wufu_local.py:81 `market_df["trade_date"].dt.strftime(...)` 才崩
  （lldb：`_st_fmt` 收到垃圾 fmt 指针，`.dt` 访问器链未接 `py_dt_strftime`）。

### 验证
- 新增 `tests/python_style/t290_main_guard.z`（+ `guardmain_fixture.py`）：
  import 带守卫的 has_main 模块不得打印，root 守卫照常执行（ENTRY/ROOT/3）。
- 四门禁全绿：官方 194/194；python_style **278/2**（仅存量红 t231/t233；
  注：并行跑驱动的编译与套件会产生假失败——串行复测为准）；语料 38/38=100%
  （口径：驱动移出 strategies/code）；§9 harness 逐字对上
  （universe 119 sz.159985 sh.512070 / cached 892 9 2022-05-05 2025-12-31 / clean 892 9 892）。

**下一步**：批次 291 —— Series `.dt.strftime` 访问器链（`df[col].dt.strftime(fmt).unique()`），
随后处理「合计 12375 行，0 只标的」的计数疑点与 LocalBackend 主循环。

## 环境观测（2026-09-21 12:24，非批次记录）：进程卡死导致 agent 静默 3 小时

**触发**：用户反馈「qoder 推进 roadmap 好像没在动」——最后提交 09-21 05:56（批次 290），
未提交文件 mtime 停在 09:08，之后 3 小时零写入、零编译进程。

**发现**：`/tmp/dg2`（zetac 编译产物，Mach-O arm64，字符串含 `ZETA_PROBE`、
`zeta_matmul is an i64 mul stub` 警告）**单核 100% CPU 空转 25 小时 20 分**：

- 启动 09-20 约 11:00（PPID=1，已脱离父进程），累积 CPU 时间 1497 分钟
- RSS 仅 3 MB —— **纯空转，不是在计算**（正常编译器测试不会 25 小时只占 3MB）
- 挂在 tty s007，cwd `/private/tmp`
- 占用整机 load 的 1/3（load avg 3.4 / 10 核）

**处理**：`kill 97330`（SIGTERM 即退出，验证确为挂起而非长任务）。处置后 CPU 峰值进程
从 100% 降至 25%（仅剩 Qoder 界面与 WindowServer）。

**教训（对后续批次）**：
1. `/tmp` 下的 zeta 测试二进制若卡死不会自己退出，且脱离父进程后无人回收 →
   **长跑探针必须带超时**（`timeout 300 /tmp/xxx`），禁止裸跑无超时二进制。
2. 判断 agent 是否在工作：看**提交时间 + 未提交文件 mtime + 是否有编译进程**，
   而不是只看 App 是否开着（Qoder 窗口开着 ≠ agent 在干活；其子进程只有
   Electron 标准组件时说明没有 worker 在跑）。
3. 无进展时的第一检查项：`ps -Ao pid,%cpu,etime,comm -r | head`，
   找 `etime` 很长且 `%cpu` 高的孤儿进程。

**批次 291 起点不受影响**：series `.dt.strftime` 访问器链（`df[col].dt.strftime(fmt).unique()`）
仍是下一步，本次仅环境清理，无代码改动。

## 批次 291（2026-09-21，**接管自 Qoder 的未提交工作**）：bool 打印语义 + `.dt.strftime` 突破

### 起点
Qoder 于 09-21 05:56 提交批次 290 后继续工作到 09:08（8 个文件、~500 行未提交改动），
随后进程静默（见文末「环境观测」：`/tmp/dg2` 卡死 25 小时）。本批接管这批工作，
先判定其状态再决定去留。

**盘点结果**：改动方向正确（`.dt.strftime` 访问器链 + `timezone.utc` 注册 +
`@classmethod` cls 形参 + 构造器实参传递），但**引入 1 个回归**：
`t229_method_defaults_isin` 由绿转红（干净基线 278/2 → 带改动 277/3）。

### 本批修复（两处，均为 print 的类型分派缺口）

1. **`print(bool)` 走 i64 回退**（`src/middle/mir/gen.rs` 分派表）
   - 现象：`print(1 == 1)` 输出 `1`、`print(True)` 输出 `true`，CPython 应为 `True`/`False`。
   - 根因 A：分派表只有 Str/F64/PyPath，缺 `Type::Bool`，bool 落进 `_ => println_i64`。
   - 根因 B：BinOp 的 `op_type` 兜底是 `Type::I64`——**整型比较**（`1 == 1`、`i < n`）
     结果被标成 I64；只有浮点比较分支才修正为 Bool。于是即使补了分派表，
     比较表达式仍然拿不到 Bool 类型。
   - 修法：① 整型比较的结果类型改判 `Type::Bool`；② 分派表加 `Type::Bool => "print_bool"`。
     **注意排除 `&&`/`||`**——它们在 Python 里是取值语义，有独立的 op_type 重算段，
     误纳入会让 38 个用例转红（实测）。
2. **`print_bool` 输出小写**（`runtime/tokio_runtime_stub.c`）
   - `printf("%s", v ? "true" : "false")` → `"True" : "False"`（Python repr）。
   - 改 C 源码后**必须重跑 `./tools/build_runtime.sh`**，否则 `.o` 不更新、改动不生效
     （本批踩过：改了 C 没重建 runtime，一度以为修复无效）。

### 测试期望同步（35 个用例）
`print(bool)` 由 1/0 变为 True/False 后，35 个用例的 expect 行是**旧 bug 的行为快照**，
按 CPython 语义更新（`1→True`、`0→False`）。其中两处需人工判断：

- `t229`：`Series(["a","a","b"]).nunique()` 期望 `3` → **`2`**。
  注释原文「str 列 map 成员偶发按指针，可能计 3」= 当初把 bug 固化成期望值；
  CPython pandas 实测为 2（唯一值数），Qoder 的改动恰好修好了它。
- `t114_argparse_argv`：期望 `1` → `True`（`store_true` 的存在性判定）。

### 效果
- **三基线全绿**：官方 194/194；python_style **278/2**（仅存量红 t231/t233）；语料 **39/39=100%**。
- **批次 290 卡点突破**：drvSF 不再崩在 `jq_wufu_local.py:81` 的
  `market_df["trade_date"].dt.strftime(...)`（`.dt` 访问器链已通）。
- 崩点推进到：`MarketDataFetcher::_load_cache` 的 `host_str_concat`
  （`ZT-DIAG host_str_concat a=0x102c4e9a7[0] b=0x18[?]`，
   `b=0x18` 不是合法字符串句柄 ⇒ 拼接参数里有一个未初始化/被截断的值）。
  发生在 baostock 批量拉取之后、缓存写入路径上。

### 验证
- `./tools/run_all.sh --json-only` → 194/194 · 278/2 · 39/39。
- bool 打印探针：`print(True)`/`print(1==1)`/`print(1<2)`/`print("x" in "abc")`
  输出与 CPython 逐行一致。
- drvSF：编译通过 → 运行到 `_load_cache` 字符串拼接崩（`Abort trap: 6`），
  崩溃栈 `#2 MarketDataFetcher::_load_cache+0x24c`。

**下一步**：批次 292 —— `_load_cache` 的 `host_str_concat` 参数 `b=0x18` 来源
（疑似小字符串打包槽 / 未初始化句柄，参见 handoff.md §7.6）。

## 批次 292（2026-09-21）：`_load_cache` no longer aborts —— drvSF **首次跑完整场回测**

### 起点（批次 291 顺延）
drvSF 突破 `.dt.strftime` 后，崩在 `MarketDataFetcher::_load_cache+0x24c`：
`ZT-DIAG host_str_concat a=0x102c4e9a7[0] b=0x18[?]`，`b=0x18`（=24）不是合法 char*。

### 破案：abort 的制造者是诊断探针本身
批次 291 留下的探针 `zt_concat_probe` **只报不修、且以 abort 收尾**。它把
「非法参数」这一可降级事件升级成进程终止，**掩盖了真实故障点**——
abort 时程序本来还能继续跑。

### 修复
`host_str_concat`（`runtime/tokio_runtime_stub.c`）改为**降级 + 一次性告警**：

- 非法一侧强转为 `""`，让字符串构建继续，暴露下一个真正的失败；
- 保留 `backtrace + dladdr` 调用链与野生指针值（前 8 次），便于追溯来源；
- 移除 `zt_concat_probe` 的 abort 路径（文件头加 `#include <execinfo.h>`，
  因为该函数现在在文件前部使用 backtrace）。

### 效果（drvSF 里程碑）
**程序不再崩溃，第一次从头跑到尾、退出码 0**：

```
[INFO] jq_shim: [baostock] 批量拉取 28 只...
ZT-WARN host_str_concat bad arg a=0x103026e07[0] b=0xdb6d000102ffcbcc[?]
  #0 host_str_concat+0xfc
  #1 MarketDataFetcher::_load_cache+0x24c
[WARNING] 0baostock login failed: 4376219488
[INFO] jq_shim: 合计 12375 行，0 只标的
[INFO] strategies.code.jq_wufu_local: [local] 开始回测: 4345527160 ~ 4345527171（0 交易日）
[INFO] strategies.code.jq_wufu_local: [local] 回测完成: 1000000 -> 0 (-100.00%)
```

### 新暴露的待办（排序即下一步顺序）

1. **`b=0xdb6d000102ffcbcc` 形态变了**：从小整数 `0x18` 变成
   **小字符串打包槽**（handoff.md §7.6：`"sz.15998" = 0x38393935312e7a73`）。
   `0xdb6d000102ffcbcc` 高位字节含 `\x00\x01` —— 疑似**句柄与整数字段被混装**，
   或 concat 的一个参数收的是打包后的 str 而非指针。仍需定位 `_load_cache` 里
   哪次拼接传入它。
2. **`0baostock login failed: <4.3e9>`**：异常消息提取仍是整数句柄
   （见批次 291 探针：`str(ValueError("boom"))` → `4341190592`）。
   **Exception→str 未实现**，导致所有 `f"... {e}"` 打印句柄而非消息。
   这是「0 只标的」类静默失败难以定位的元凶之一。
3. **「合计 12375 行，0 只标的」**：行数对、标的数 0 —— 校验/分组路径把 code 列丢了。
4. **日期 `4345527160 ~ 4345527171`**：日期实参被当整数透传（非 PyDate），
   与 2/3 可能同源（类型传播丢失）。
5. `交易成本: 佣金万0 | 滑点0.00%` —— 成本配置读成 0（`CostModel.from_jq` 路径）。

### 验证
- `./tools/run_all.sh --json-only` → **194/194 · 278/2 · 39/39**（三基线保持全绿）。
- drvSF：编译通过 → 运行至结束（退出码 0），不再 abort。

**下一步**：批次 293 —— 优先做 2（Exception→str），因为它把后续所有静默失败
都变成不可读；然后按 1 → 3 → 4 推进。

## 批次 293（2026-09-21）：推导式/三元表达式的**元素类型**不再塌成 I64

### 起点
批次 292 待办 4：`[local] 开始回测: 4345527160 ~ 4345527171（0 交易日）`。
日期是整数句柄、交易日 0 —— 都指向「str 值落进 I64 槽」。

### 三处类型丢失（`src/middle/mir/gen.rs`）
1. **表达式型 `If` 的 dest**：以 I64 起型，str 分支写进来后槽仍是 I64。
   `[d for d in days if cond]` 脱糖成 `if cond { d } else { -1 }` →
   `trading_days_filtered` 是 `DynamicArray(I64)`，日期比较退化成指针比较，
   窗口匹配 **0 天**。修法：dest 按携带元素的那条分支重定型（`-1` 哨兵无需类型）。
2. **三元臂的类型**：`tail_of` 只认字面量；臂尾是 `Var` 时用 `name_to_id` +
   `type_map` 解析。混合臂（`元素` vs `-1` 哨兵）不再被 I64 拖走。
3. **`sorted()` 的元素类型**：原先硬编码 `DynamicArray(I64)`，现从源数组元素
   类型推导；元素是 `str` 时改调 `zeta_sorted_vec_len_str`
   （`runtime/py_additions.c`，按字符串比较排序）。

### 效果
`[local] 开始回测: … ~ …（**37 交易日**）` —— 与 CPython 的 trading_days 一致；
`行情请求 4 只，区间 2023-08-05 ~ 2024-02-29` 日期以字符串正确打印（批次 292 是句柄）。

## 批次 294（2026-09-21）：三个弱符号桩被真实实现顶掉 + 构造器字段定型

- `x.clear()`（list 承载的 set）→ `zeta_vec_clear`（原地 len=0，句柄保持有效；
  此前落到 `_clear` abort 桩，`PositionLedger._today_buys.clear()` 直接终止进程）。
- `<opaque>.date()` → `zeta_dt_date`（Zeta 的 datetime 句柄就是 PyDate，
  `.strftime`/比较读同一 shape，投影即句柄本身）。
- **按值调用**：`for routine in [morning_routine, …]: routine(context)` —— 被调
  名是持有函数地址的局部变量，静态符号会链到 `_routine` abort 桩；改走 C 跳板
  `zeta_call1`（仅在无同名全局函数、且不是闭包时）。
- `parse_class`（`src/frontend/parser/top_level.rs`）把 `self.field = 首字母大写(...)`
  定为 `Named(cls)`，构造出来的字段不再匿名。

## 批次 295（2026-09-21）：`context.portfolio.positions` 读通 —— 全 37 日回测跑完不崩

### 起点
`_parity_snapshot+452` SIGSEGV（rc=139）：`[s for s, p in context.portfolio.positions.items()]`。

### 根因链（三层，逐层实测）
1. **管线顺序**：whole-program refine 在 per-function MIR 之后，`.positions` 在
   lowering 期被当成普通字段读，类型 I64。
2. **属性 vs 字段**：`positions` 是 `@property`，运行时值要靠方法调用产生；
   字段读回落到 2 字段 stand-in → 把 `LocalBackend` 句柄当 map 用。
3. **Protocol 桩**：`ExecutionBackend.positions` 的体是 `...` → `ret i64 0`，
   即使调对了也返回 0。

### 修复
- `refine_param_types`（`src/main.rs`）重写为 3 轮不动点：
  - `is_interface_method`：无 Call/VoidCall/If/For/While/DictInsert 的体即桩
    （**`Assign` 必须排除**：Protocol 体会 lower 成 `ParamInit + Assign + Return`，
    误判会让 `ExecutionBackend total=6 stubs=0`）；`total>=2 && stubs==total` 的类 = Protocol。
  - `overridable`：`None | I64 | PyDynamic | Variable | Named(∈protocols)` 可被覆盖；
    `worth`：接口类型的实参**不得**参与共识（接口只说明「是某个具体对象」，不说明是哪个）。
  - Phase 1 调用点→形参；**Phase 2（新）**构造器 `MirExpr::Struct{variant,fields}`
    的字段类型共识 → 覆盖 `FieldAccess` 节点类型（`self.portfolio = LocalBackend(...)`
    这类字段由此获得具体类型）。
- `gen_expr` 的 `MirExpr::FieldAccess`（codegen）：接收者类若有一个**零参同名方法**，
  这是 `@property` 读，必须 `build_call`（`prop_read`），而不是按 struct 字段读；
  接收者类沿 `Var`/`FieldAccess` 链上溯 6 层并用 `struct_{variant}_` 前缀校验中间字段真实存在。
- `.items()` 在未知接收者上不再 identity 链式传递：`("items", 1) => ("py_map_items", "vec")`
  （identity 会让推导式收集器把 MAP 句柄当 Vec 头读，`zeta_collect_vec_n+40` 崩溃）。
- 修饰名属性回退：`struct_backend…__LocalBackend` 找不到时重试 `LocalBackend::positions`。
- `zeta_dyn_getitem(base, key)`（`runtime/py_additions.c`）：按 GC 头区分 Vec/Map，
  兜底 `map_get`，替代 enumerate 路径上的 `map_get` 误用。

### 本批收尾的**回归修复**（python_style 278/2 → 277/3 → 278/2）
`t129_lambda_binding` 的 `f = lambda s: s.upper()` 打印堆指针。
定位：批次 294 的 `zeta_call1` 守卫抢先吃掉了 `f("ab")`（`f` 是持有闭包地址的
局部槽 → `is_value_slot` 成立 → 返回类型写死 I64），而 `closure_vars` 路径
（`closure_ret_tys` 记录 lambda 体真实类型）在它之后。
修法：`zeta_call1` 路径加 `&& !self.closure_vars.contains_key(method)`；
按值调用（`routine(context)`）保持，闭包返回类型恢复。
**排除项（实测）**：`refine_param_types` 关掉（临时注释 + 重建）t129 仍红 → 非 refine 所致；
`git show HEAD:gen.rs` 覆盖后 t129 绿 → 回归在工作树 gen.rs 内。

### 验证（串行门禁）
- `./tools/build_runtime.sh` → `ok: tokio_runtime.o (624 T) + zeta_runtime_c.o`
- `./tools/run_all.sh` → official **194/194** · python_style **278 passed, 2 failed**
  （仅存量红 t231/t233）· 语料 **38/38**（口径：驱动移出 `strategies/code`；在位时 39/39）
- handoff §9 自检 harness 逐字对上：
  `universe 119 sz.159985 sh.512070` / `cached 892 9 2022-05-05 2025-12-31` / `clean 892 9 892`
- 驱动 `drvN36`：`ZETA_NO_OPT=1 REPLAYQUANT_LOCAL=1` 编译 → 37 日回测全程 **rc=0**

### 仍挡验收的待办（按可操作性排序）
1. **`合计 12375 行，0 只标的` / `Loaded 12375 records for 0 stocks`**：行数对、
   按 code 分组后为 0 → 分组键列丢失；这直接导致 final_value=0。
2. **`%d rows, codes=%d, trading_days=%d 12375 0 119`**：`%`-格式串未替换实参，
   实参被追加到尾部（str-format 未实现，非 f-string）。
3. **`[PARITY] 4397413760 | target=… | holdings=… | ranked=-`**：句柄代替字符串/对象
   打印（与 Exception→str 同源）。
4. `load_metadata failed for …parquet: 1` ×N（尾部数字疑似错误码未转消息）。
5. `交易成本: 佣金万0 | 滑点0.00%` —— `CostModel.from_jq` 读成 0。
6. `getattr ... is not implemented in this form` ×13，全部走默认值。

**下一步**：批次 296 —— 打待办 1（分组键列丢失）：它同时解释「0 只标的」和
final_value=0，是当前唯一阻塞数值验收（994575.84 / -0.5424）的项。

## 批次 296（2026-09-21）：动态值的运行时分发 + json.items() 推导键内容哈希 —— **行情链路首次全缓存命中**

### 起点（批次 295 待办 1）
`合计 12375 行，0 只标的` / `Loaded 12375 records for 0 stocks`：行数对、按 code 分组后为 0。

### 破案：不是「分组键列丢失」，是**读不懂动态值**
三条独立证据把根因从 DataFrame 分组挪到了类型系统：
1. `_build_stock_arrays` 的返回值存在 `dict[str, Any]` 里，取出后编译器只能给 **I64** 槽。
   在此槽上 `len(e)` 走 `array_len` ⇒ 读前一个 GC 块头 ⇒ 返回 **0**（`codes=0`，实测真值 91）。
2. `"x" in e` 在 I64 接收者上根本不识别，编译期只留一句 warning，运行期恒 **False**。
3. 真正的「0 只」来自上游：`etf_listing._listing_dates_cached()` 的
   `{str(k): str(v) for k, v in data.items()}` —— **推导式里 pair 的两个槽都按整对
   (`DynamicArray(Named("PyJson"))`) 定型**，槽 0 不是 `Str` ⇒ `__pack_pair__` 的
   `Type::Str` 判据永不成立 ⇒ 不发 `map_str_key` ⇒ 字典按指针建键。
   1724 条上市日塌成 1 条 ⇒ 上市日过滤不生效 ⇒ 119 只全请求 ⇒ 28 只走网络（离线）失败
   ⇒ 只剩 91 只。

### 修复
**runtime（`runtime/py_additions.c`，仍保持本 TU 末尾）**
- `zt_dyn_is_map` / `zt_dyn_vec_hdr`：只用 GC 几何判形状（`GC_base` 必须是块首、
  Map 的 `cap` 是 ≥16 的 2 的幂且 `GC_size ≥ 16+cap*24`；Vec 的 `base-16` 处 `[cap|len]`
  且 `GC_size ≥ 16+cap*8`）。Map 的负 `cap` 是扩容转发指针，按 `w[1]` 目标再判。
- `zeta_dyn_len`：map → `zeta_map_len`，vec → 头里的 `len`，**最后**才试文本
  （`zt_c_readable` 用 `vm_read_overwrite` 探首字节 + `strnlen` 限长）。
  第一版直接落到 `strlen` 上，非句柄的 I64 值 ⇒ SIGSEGV（`main+680`）；
  改用「先判 GC 块」后又发现**字符串字面量在 .rodata、不在 GC 堆**（`str len 0`），
  最终定为可读性探测。
- `zeta_dyn_contains`：map 用内容哈希 `map_get`，vec 用 `py_list_contains`，
  否则 `strstr` 子串（并保住 Python 的 `"" in text == True`）。

**mir（`src/middle/mir/gen.rs`）**
- `len(x)`：`None | I64 | PyDynamic` 槽改发 `zeta_dyn_len`（原先的 `array_len` 回退是错的）。
- `k in c`：同一批槽型改发 `zeta_dyn_contains`，把编译期 warning 变成运行期真判。
- json.items() 的 pair：for 循环解构路径把槽 0 定型 `Str`（`zt_key_display` 还回文本键）；
  推导式路径的闭包参数提示改成 `DynamicArray(Tuple([Str, PyJson]))`，并在
  `stack_array_get` 槽读处**按下标取元组分型**。
  坑：下标只能从 **AST 字面量**拿，`self.exprs[lowered_id]` 已是 `Var`（首版按 `IntLit`
  匹配 ⇒ 恒落 I64，实测 `to_string_i64(指针)`）。
- `series.map(fn)` 在列向量上预分派 `[dynamic]str__map`，返回类型取
  `closure_ret_tys`（此前定型 I64 ⇒ `s2[0]` 打印堆指针、`nunique()` 落回 Series 桩读坏头）。
- `df["col"].nunique()`（列即 vec）走 `zeta_vec_nunique`（此前 `Series::nunique` 把
  vec 头当 `self.data`，2 元素 str 列上 SIGBUS `nunique+72`）。
- `str(<已经是 str>)` 改发 `zeta_identity`：原先只登记 `Var(arg)` 别名、没有产出语句，
  模块级 `x = str("ab")` 从**未写过的 alloca** 取值 ⇒ `puts(0)` SEGFAULT。

### 效果（drv39，`ZETA_NO_OPT=1 REPLAYQUANT_LOCAL=1`，rc=0）
- `上市日过滤: 119 → 107 只`（跳过项带真实上市日，如 `588170.XSHG：上市日 2025-04-08`）
- `缓存命中 107 只；待拉取 0 只` → `合计 13402 行，107 只标的` → `Loaded 13402 records for 107 stocks`
- `本地数据注入完成: … 13402 107 119`：**网络拉取路径彻底消失**，全离线跑通
- 回测 37 日全程 rc=0

### 验证（串行门禁）
- `./tools/build_runtime.sh` → `ok: tokio_runtime.o (624 T) + zeta_runtime_c.o`
- `./tools/run_all.sh` → official **194/194** · python_style **278 passed, 2 failed**
  （仅存量红 t231/t233）· 语料 **39/39**（驱动在位；移出则 38/38）
- 新增回归 `tests/python_style/t291_dyn_len_contains_json_comp.z`（dyn `len`/`in` +
  json.items() 推导键 lookup，断言与 items() 迭代顺序无关）→ 套件 **279 passed, 2 failed**
- handoff §9 自检 harness 逐字对上：
  `universe 119 sz.159985 sh.512070` / `cached 892 9 2022-05-05 2025-12-31` / `clean 892 9 892`

### 仍挡验收的待办（排序即下一步顺序）
1. **`ranked=-` 每日为空**：`_parity_snapshot` 的
   `getattr(g, "ranked_etfs_result", [])` 是三参 + 字面量名形态，编译器只报
   `not implemented in this form` 后落到默认值 ⇒ 快照读到空。需先分辨
   「选股真为空」与「只是 getattr 读丢」：把三参 getattr 实现为
   `属性存在则取值、否则取 default`，再看 ranked/target。
2. **`%`-格式串未替换实参**：`本地数据注入完成: market=%d rows, codes=%d, trading_days=%d 13402 107 119`。
3. **句柄代替字符串/对象打印**：`开始回测: 4365142248 ~ 4365142259`、
   `target=4368798336`、`holdings=4368801056` —— f-string/`str()` 对 Str 值仍打指针。
4. `回测完成: 1000000 -> 0 (-100.00%)`：final_value 归零（持仓估值或市值链未通）。
5. bool 打印成 `1/0`、`交易成本: 佣金万0 | 滑点0.00%`（`CostModel.from_jq` 读成 0）、
   `os.chdir` 空操作、`open(path, encoding=…)` 把 kwarg 错映射成 mode ⇒ `Invalid argument`、
   动态值上的 `.keys()` 链到未定义 `_keys`、`.items()` 迭代顺序与插入序相反。

**下一步**：批次 297 —— 打待办 1（三参 `getattr`）+ 待办 3（Str 值在 f-string 里打指针）。
这两条决定 `ranked`/`target` 能否还原成可读文本，是从「日志能看」走到「数值能核」的最短路径。

---

## 批次 297 —— 分支条件的 Python 真值（`if x` / `not x` / `x or y` / `x and y`）

### 症状
`jq_wufu_local` 里所有「空容器」判定都是错的：`if not g.merged_etf_pool:` 永不短路、
`ranked = getattr(g, "ranked_etfs_result", []) or []` 恒取空的一侧、
`if pool:` 在空列表上为真。日志上表现为 `ranked=-`（空）而数据链路已通。

### 根因
容器在 i64 槽里是**句柄**，句柄恒非零 ⇒ 编译器把条件降成 `icmp ne 0` 就等于
「只要有地址就为真」。空 list/dict/str 的地址照样非零。
静态类型又帮不上忙：`dict[str, Any]` 的元素、动态参数、json 取值都落在
`I64`/`PyDynamic`/无类型 槽里，编译器只看到「整数」。

### 修复
**runtime（`runtime/py_additions.c`，仍在 TU 末尾）**
- 新增 `zeta_dyn_truth(h)`：**单一**的「未知值 Python 真值」定义 ——
  map 形状 → 计数、vec 形状 → 头里的 `len`、否则可读性探测后的首字节、再否则 `!= 0`。
  `py_not` 改为 `zeta_dyn_truth(x) ? 0 : 1`，于是 `not x`、`if x:`、`while x:` 共用同一套语义。
- 新增 `py_json_truth(j)`：`json.loads` 的值是**打 tag 的单元**
  （`word0=ZJ_*`、`word1=载荷`，见 stub 里的 `zj_make`），几何探针读不出长度
  （首词是 4 ⇒ 被当成非空字符串）。按 kind 分派：null→假、int/bool/float→值、
  str/list/dict→`py_json_len`。

**codegen（`src/backend/codegen/codegen.rs::container_cond_i1`）**
- 判定表从「只有 `DynamicArray`/`map`」扩到 `Array`/`Str`/`I64`/`PyDynamic`/无类型
  （都发 `py_not`，比较 `== 0`）；`map|dict|set|frozenset` 同路；
  `Named("PyJson")` 单列，发 `py_json_truth` 并比较 `!= 0`（helper 答的是「真」不是「空」）。
- `Bool`/`F64` 保持原样（浮点在 `cond_i1_from` 里已有自己的位比较路径）。

**mir（`src/middle/mir/gen.rs`）**
- `or`/`and`：左操作数先过 `zeta_dyn_truth`（`PyJson` 走 `py_json_truth`），
  再用结果 `!= 0` 决定取哪一侧；`Bool`/`F64` 直连，不付这次调用。
- `not x`：操作数是 `PyJson` 时先发 `py_json_truth` 再 `py_not`（`py_not(0/1)` 恰好是逻辑取反）。

### 验证（串行门禁）
- `./tools/build_runtime.sh` → `ok: tokio_runtime.o (624 T) + zeta_runtime_c.o`；`cargo build --release` → 17.47s
- `./tools/run_all.sh` → official **194/194** · python_style **280 passed, 2 failed**
  （仍只有存量红 t231/t233）· 语料 **39/39**（驱动在位、探针已删）
- 新增回归 `tests/python_style/t292_container_truth.z`：字面量空 list/dict/str 的
  `if`/`not`、`x or y` 与 `x and y` 的**取值**语义、json 值的
  `0/false/""/[]/{}` 判定、以及整数 `or/and` 不受影响 ——
  五行断言与 CPython 逐字相同（`469 / 7 / 5 / 10 / 3`）。
- handoff §9 自检 harness 逐字对上（批次 297 之后仍然成立）：
  `universe 119 sz.159985 sh.512070` / `cached 892 9 2022-05-05 2025-12-31` / `clean 892 9 892`
- 驱动 drv43 与 drv39 输出逐行等价（数据链路未变），`ranked=-` 依旧 ⇒ **不是**真值判定问题。

### 过程中钉死的两件事（写下来，别再花 turn 找）
1. **`zetac` 的工作目录必须是 `zeta-src`**（`pylib/*.z` 相对 cwd 解析）。
   在项目目录里编译会看到 `warning: PY-A: unknown member DataFrame in Python module pandas`
   和一堆 `_DataFrame`/`_load_cache`/`_get_universe` 链接失败 —— 那**不是**编译器退化，
   是 shim 没加载。主文件放在项目内（`strategies/code/…`）解析模块，cwd 仍在 zeta-src。
2. **驱动有约 25% 概率崩在 `str_trim`**（`~/Library/Logs/DiagnosticReports/drvN37-…1618.ips`
   早于本批次，故为**存量**崩点，非 297 引入）：
   `str_trim+24 ← backend_datasrc_code_conv__normalize_to_jq+16 ← MarketDataFetcher::fetch_stocks+5336`，
   崩时的实参是 0x3932 / 0x3132 / 0x3633392e30 —— 小端 ASCII 分别是 `29`、`12`、`0.936`，
   即**短字符串被打进 8 字节槽里（packed）当指针传给了 `str.strip()`**。
   `lldb` 下 6 次不崩（堆布局变了），别把它当偶发。

**下一步**：批次 298 —— 收掉这个 packed-string 崩点（`code.strip()` 前把 packed 槽
还原成真 `char*`，或在 `str_trim` 一侧判形），它同时是「25% 概率崩」和
`target=/holdings=/ranked=` 打指针（同一批短码字符串）的近邻。

## 批次 298 —— 未标注字段的静态类型 + 赋值侧的 int↔float 槽位一致

acceptance 的最后一环是期末估值 `final_value += pos.total_amount * price`，
这条链上两处静态类型是错的：

### 崩点与根因
1. **未标注的 `self.x = <expr>` 没有类型**。`parse_class` 只采信显式注解，
   于是 `self._cash = 500.0` 的字段在 `type_decls` 里是 `i64`，
   读出来是**double 的位模式**（`500.0` → `4619567317775286272`）。
2. **赋值侧不做 int→float 的槽位一致**。float 字段/float 容器槽按约定存**位模式**
   （`f64_as_i64`），但 `MirStmt::Assign` 与 `BinaryOp` 提升把 i64 值直接写进去，
   读侧再 `bitcast` 就成了天文数字。

### 改动
**parser（`src/frontend/parser/top_level.rs`）**
- 无注解的 `self.x = <expr>` 按表达式推字段类型：`Lit(Float)` / `float(...)` /
  `self.x = <float 字段>` 推 `f64`；`self.x = v or Default()` 穿透到 `v`。

**codegen（`src/backend/codegen/codegen.rs`）**
- `MirStmt::Assign`：目标是 float 槽而值是整数时发 `sitofp`（写入侧与读取侧对齐）。
- `SemiringFold`：累加器与操作数都改走 `slot_or_sitofp`（`sef_acc_sitofp` /
  `sef_val_sitofp`），并把 `values[i]` 的 expr id 传进 `gen_expr`，
  否则 `FieldAccess` 操作数拿不到自己的静态类型 ——
  `self._cash = self._cash + total` 曾编译成 `sitofp(4619567317775286272) + total`。

### 验证
- official 194/194 · python_style 281 passed, 2 failed（存量红 t231/t233）· 语料 38/38
- 新增回归 `tests/python_style/t298_field_float_infer.z`（`// expect: 63`）：
  未标注 float 字段的读回、`cash -= p.avg_cost` 的减法、`total += p.x * 2.0` 的累加。

## 批次 299 —— 字典推导的键类型 + 容器槽 float 的来历判定

### 崩点
`LocalBackend.positions` → `PositionLedger.positions` 是一个带过滤条件的字典推导：
```python
{c: Position(...) for c, p in self._positions.items() if p["amount"] > 0}
```
推导产物的**键**静态类型是 `i64`（`self._p = {}` 无注解 ⇒ `map[PyDynamic, I64]`），
于是 `d["AAA"]` 拿 packed 短码当整数哈希去查 ⇒ SEGV；同一条链上
`p.total_amount == 2000.0` 恒假（float 字段读出来是位模式，比较前没 bitcast）。

### 改动
**mir（`src/middle/mir/gen.rs`）**
- `items/keys/values/most_common` 的视图元素类型：`Type::Named` 的实参缺省或是
  `PyDynamic` 时**当作未标注**，键回落到 `Str`、值回落到 `I64`，
  这样推导侧能对字面量键做内容哈希。
- `stack_array_get` 的槽类型判定同时接受**裸 `Tuple(..)` 参数**（推导的元组目标
  在 hint 里存的是元素类型整体，不是 `DynamicArray(elem)`）。
- `struct_field_ty`：`type_decls` 的键是**模块 mangle 过的**
  （`backend_strategy_wufu_backend__Position`），而部分路径用裸名 ——
  补上「裸名 → mangled」的正向候选并 `sort()` 保证确定性。

**codegen（`src/backend/codegen/codegen.rs`）**
- 新增 `pub slot_read_ids: HashSet<u32>` + `collect_slot_reads`：
  在 `emit_function` 里扫出「从容器槽读出来的值」的 expr id
  （`map_get / dict_get / array_get / vec_get / stack_array_get / zeta_dyn_getitem /
  map_get_default / map_get_or`），穿过 `Assign` 做不动点扩张。
- `fn slot_or_sitofp(v, id, name)`：**来历决定 bitcast 还是 sitofp** ——
  容器槽读出来的整数是位模式（bitcast），算术产生的整数是数值（sitofp）。
- `coerce_call_args` 增加 `arg_ids`，`(IntValue, FloatType)` 这条边按同一规则选
  `arg_slot_bits` / `arg_sitofp`。
- `BinaryOp` 的混合精度提升走 `slot_or_sitofp`，并且**操作数必须带自己的 expr id**
  （此前 `gen_expr(.., None)` 让 `FieldAccess` 丢掉了静态类型）。

### 验证
- official **194/194** · python_style **282 passed, 2 failed**（只有存量红 t231/t233）
  · 语料 **38/38**（驱动移开）· handoff §9 harness 逐字：
  `universe 119 sz.159985 sh.512070` / `cached 892 9 2022-05-05 2025-12-31` / `clean 892 9 892`
- 新增回归 `tests/python_style/t299_dictcomp_key_float_slot.z`（`// expect: 1023`）：
  无注解 dict 字段上做推导（含过滤式）、`d["AAA"]` 按键取回、`len(d.values())`、
  `p.total_amount * 2.0` 累加、`cash -= p.avg_cost`、过滤推导的 `len`/取值、
  成员判定、`int(p.total_amount)` 求和；O0/O2/NO_OPT 三档结果一致。
- 真实模块的估值链：drv62 打印
  `P code 510880.XSHG amt 1000.000000 cost 1.500000` /
  `P code 159509.XSHE amt 2000.000000 cost 1.250000`
  （批次前是 `P code 4302104953 amt 4652007308841189376`）。

### 尝试后回退的部分
`runtime/py_additions.c` 里 `zeta_collect_dict` 的 `GC_base((void*)k) == k` 重哈希
**没有生效**（字符串句柄不是自己的 GC base），且对 struct 键是隐患 —— 已回退。

**下一步**：批次 300 —— acceptance 仍然打印 `回测完成: 1000000 -> 0 (-100.00%)` /
`AZ JSON <pointer>`。三个待打的点按优先级：
1. `run_backtest` 内部 0 笔成交、`available_cash` 读到 0、`final_holdings` 为空 ——
   新线索是**对「函数返回的对象」再调方法会静默崩溃**（`/tmp/wl/drv_L4crash.py`）。
2. f-string 与 `%s` 的字符串参数来历（打指针、不同短串塌成同一个字），
   它同时挡住 `_parity_snapshot` 的可观测性与 `json.dumps` 的验收输出。
3. universe 分歧：zeta `119 → 107` vs CPython `115 → 103`，伴随
   `load_metadata failed … : 1` 的 parquet 回退。

---

## 批次 300 —— 未标注 `def` 的返回类型（函数返回值上的方法调用链）

### 崩点
`/tmp/wl/drv_L4crash.py` 在 L4 一行之前还先崩在
`PY-A: '_values' is NOT implemented in this build`；即使跳过那一行，
`pf.available_cash` 读到的是垃圾。最小复现 `/tmp/wl/z300/b4.z`：
函数返回一个类实例，模块级 `pf = make(1000.0)`，然后 `pf.available_cash` /
`pf.positions.values()` —— 前者打 0、后者链接到 `_values` 幽灵符号。

### 根因
**未标注返回值的 `def` 在签名表里注册成 `Type::Tuple(vec![])`，也就是 unit。**
Python 的「没有注解」在这个 parser 里写成 `ret == "()"`（不是空串 —— 用
`ZETA_PROBE300` 实测出来的，第一次修复因为守卫条件写成 `!ret.trim().is_empty()`
而完全没生效）。unit 顺着调用点传下去后：

- `pf.<attr>` 的字段读编译成 `extractvalue {i64,i64}, 0`，即**读一个空 variant 的
  第 0 个字段** —— 静态类型是 unit 时每一个属性读都静默拿到垃圾；
- `pf.positions.values()` 因为接收者不是已知 struct，退化成**按方法名命名的自由调用**
  `@values`，链接到 `runtime/unavailable_stubs.c:190` 的弱桩并 abort。

### 改动
**`src/middle/resolver/resolver.rs`**
- 新增关联函数 `unannotated_return_ty(defs, name, classes, fn_rets) -> Option<Type>`：
  只有当签名为 unit 时介入，扫描该 `def` **自己 body 里的 `return`**，
  要求所有候选类型存在、彼此相等且非 unit 才返回（不一致就保持原样，宁缺毋滥）。
  内部 `infer` 覆盖：`Var`（局部赋值表）/ `StringLit|FString → Str` /
  `FloatLit → F64` / `Lit → I64` / `BoolLit → Bool` /
  `StructLit{variant} → class_ty` / `DictLit → Named("map",[k,v])` /
  `ArrayLit → DynamicArray(elem)` / `Call{receiver:None,method} →` 先按**构造调用**
  查类名（批次 291 的既有优先级：ctor 压在函数返回类型之前），再查 `fn_rets`。
  `walk` 递归 `Block / If(then,else_) / For / While`，收集 `Assign(Var, _)` 到局部表，
  并识别 `zeta_py_from(module, member, alias)` 形式的 import 别名，
  这样 `return` 一个从别的模块改 named 的类也能解析。
  两个名字查找（def 与 class）都带**唯一后缀兜底**：`type_decls` 的键是模块 mangle 过的
  （`backend_strategy_wufu_backend__Position`），而调用点用裸名；兜底只在候选唯一时命中，
  并且先把候选 `sort()` —— HashMap 顺序不确定，不能靠遍历顺序做决定。
- 两处接入点：`lower_to_mir`（约 2990 行，先算好 `class_names`（排序）与 `decl_rets`
  再进 `map` 闭包），和 `module_global_types`（约 2063 行，`classes.sort()` 之后
  拿 `declared` 快照覆盖 `fn_rets`）。模块级变量的类型来自后者，缺了它 `pf` 仍是 unit。

### 验证（串行门禁）
- `cargo build --release` 干净；`./tools/run_all.sh` → official **194/194** ·
  python_style **283 passed, 2 failed**（仍只有存量红 t231/t233）· 语料 **38/38**
- handoff §9 自检逐字对上：`universe 119 sz.159985 sh.512070` /
  `cached 892 9 2022-05-05 2025-12-31` / `clean 892 9 892`
- 最小复现 `/tmp/wl/z300/b4.z`：修复前 `cash 0.000000` + 链接失败，修复后
  `cash 1000.000000` / `n 1`
- 新增回归 `tests/python_style/t300_unannotated_func_ret.z`（`// expect: 15`）：
  未标注 `def make(...)` 返回 struct、`@property` 上的过滤字典推导、
  `len(pf.positions.values())`、`"AAA" in pf.positions`、`pf.positions["AAA"] == 1.0`
  —— 与 CPython 输出逐字相同（15）。
- 真实模块：drv303 打印 `L4 after trade 121956.000000 1`
  （批次前在 `_values` 桩上 abort），`execute_trade` / `available_cash` /
  `positions.values()` 三条链同时恢复。

**下一步**：批次 301 —— acceptance 依旧 `回测完成: 1000000 -> 0 (-100.00%)`，
末行打 `4390899200`（指针）而不是 JSON。已确认的两条：
1. **`getattr` 有编译期 `error:`**：acceptance 编译过程中
   `error: builtin \`getattr\` is not implemented in this form (it would link against an
   undefined symbol named \`getattr\`)` 出现十几次，而 `error` 并没有终止编译 ⇒
   这些调用点返回垃圾/0，`x or []` 于是恒为 `[]`。`_parity_snapshot` 的
   `getattr(g, "target_etfs_list", [])` 正落在这条路上，是 0 笔成交的头号嫌疑。
2. `[PARITY]` 行的 date/target/holdings 全部打指针
   （`4395333088`、`target=4374146667` 且不同日期共享同一个字），即 f-string/`%s`
   的字符串来历仍未接上；它同时挡住验收用的 `json.dumps` 输出。

---

## 批次 302（refactor 立即档 ①／T0）—— `--dump-mir` 变成可字节比对的重构安全网

> 编号说明：301 号由主线预占（批次 300 文末「下一步」），本节点走 refactor 轨
> （`refactor.md` §9 立即档第一波 ①），直接取 302 避免撞号。

### 为什么这是第一刀
`refactor.md` B.5 把 T0 排在一切 MIR 工作之前：**没有"变没变"的判据，任何 MIR 层
重构都无法自证是纯搬运**。而实测表明此前的 `--dump-mir` 完全不能当判据用 ——
同一份输入编译两次，jq_wufu.py 的 dump 差 **161324 行**、299 个函数块里 **284 个不同**。

### 三处不确定性（各自独立，缺一即全抖）
1. **四个 HashMap arena 的 Debug 顺序**（`exprs`/`type_map`/`ctfe_consts`/
   `global_consts`）—— 主体噪声，占 161324 行的绝大多数；
2. **函数输出顺序** —— `all_mirs` 取自 `final_mirs.values()`（HashMap），
   此前 dump 打在 `sort_by(name)` **之前**；
3. **合成闭包符号名** —— 来自进程全局 `static AtomicU32`，取值取决于函数被
   遍历的顺序（HashMap 随机）。实测同一文件里 `__closure_0` 一次是 `code`
   lambda、下一次是 `codes` lambda。

### 改动
**`src/middle/mir/mir.rs`** —— 新增 `Mir::dump_canonical()`：函数头 +
`param_indices/properties/generic_params/is_extern` + `stmts:` + 四个 arena
经内部 `render_entries` **按 key 排序**逐条 `{:#?}`（续行缩进 4）。
泛型约束写作 `K: Ord + Display`（第一版用 `DisplayKey<K>` 适配器和
`impl Display for DisplayKey<'_, String>`/`<'_, u32>`，K 未受约束编译不过 —— 删）。

**`src/main.rs`** —— 删除 `sort_by` 之前那段 `eprintln!("== MIR {} == …{:#?}")`；
改到 `all_mirs.sort_by(...)` **之后**、走 **stdout**（告警仍留 stderr），
`dump_mir` 分支只剩 `for m in &all_mirs { print!("{}", m.dump_canonical()); }`。

**`src/middle/mir/gen.rs`** —— 闭包命名换成「所属作用域名 + 序号 + 命名空间哈希」：
- 新增字段 `closure_ns: String` / `closure_seq: usize`（`new()` 里零值初始化；
  `lower_to_mir` 入口把 `closure_ns` 设为该 `FuncDef`/`ExternFunc` 的名字，
  模块级则退回 `current_module`；每函数进入时 `closure_seq = 0`）；
- `lower_closure` 的 `closure_name = __closure_{n}_{bare}_c{hash8}`。形状的两条硬约束
  都来自 codegen 的符号瀑布（`codegen.rs:2675/2689/2708/2731`）：**不能含内部
  `__`**（会被当 `module__fn` 解析，定义侧被丢 ⇒ 第 1 次尝试
  `___closure__backend_datasrc_split_factors__load_split_factors__0` 链接失败），
  **末段不能全数字**（arity 剥离会重命名引用而不重命名定义 ⇒ 第 2 次尝试
  `___closure_closure_load_split_factors_cf3b8248_0_37b76d63` 链接失败）；
  故序号在前、`_c<哈希>` 收尾，裸名相同但模块不同的父作用域靠 fnv 哈希区分；
- 撞名不再静默：`thread_local MINTED` 登记表，同名被两个作用域铸出时打
  `warning: [W2001] … minted by two scopes`；
- 子 MirGen 继承 `closure_ns = 本闭包名`、`closure_seq = 0`。

### 顺带修掉的真 bug（不只是改名）
`lower_closure` 结尾只 `self.generated_mirs.push(mir)`，而 `build_mir` 搬走的是
stmts/exprs/type_map —— **child 自己的 `generated_mirs` 从不并回**。闭包体内再
lowering 出来的合成函数因此只有引用没有定义。补 `std::mem::take(&mut child.generated_mirs)`
并入。行为差异实测（`/tmp/wl/z302/nd.py`，嵌套 `def` 三层）：
批次 300 二进制「编译成功」但运行到 `PY-A: '___closure' is NOT implemented in this
build` 后 abort；现在打印 `11`，与 CPython 一致。已固化为
`tests/python_style/t302_nested_def_in_closure.z`（`// expect: 11`）。
本地 wufu 驱动上同一缺陷表现为链接期
`Undefined symbols: ___closure_0_closure_0_load_split_factors_… referenced from
___closure_0_load_split_factors_…`。

### 新工具 `tools/mir_diff.sh`
`snapshot [dir]` / `diff [dir]`，`--file <path>`（可重复）、`--runs N`、
`MIR_CORPUS`、`MIR_DIFF_DIR`（默认 `/tmp/zeta_mir_baseline`）、`MIR_DIFF_SHOW=1`。
语料发现口径与 `tools/corpus_baseline.py` 一致。snapshot **连跑 N 次**，任一次
不一致即标 `UNSTABLE` —— 这是设计要点：不稳定文件必须自己现形，否则一次抖动就能
把「纯搬运」的结论伪装成真的。diff 只在 `changed>0` 时退 1；新增/删除文件与
UNSTABLE 只报告不退 1（主线会把驱动换进换出语料）。bash 3.2 兼容
（无 `mapfile`）、`timeout` 可选、`set -e` 下不用 `[[ … ]] && cmd` 短路。
正反两向实测：干净快照 `same=2 changed=0 rc=0`；把 `m1.py` 的 `return a+b` 改成
`return a+b+3` 后 `changed=1 rc=1` + `CHANGED: m1.py`。

### 验证（串行门禁）
- `cargo build --release -p zetac` 干净；`./tools/build_runtime.sh` ok
- `./tools/run_all.sh` → official **194/194** · python_style **284 passed,
  2 failed**（新增 t302 通过；仍只有存量红 t231/t233）· 语料 **39/39**
- 本地驱动 `ZETA_NO_OPT=1 REPLAYQUANT_LOCAL=1 zetac …/\_zeta_local_drv.py -o /tmp/wl/drv310`
  → `Compiled to /tmp/wl/drv310`，rc=0（闭包改名期间曾链接失败，现已恢复）
- **稳定性度量**：`MIR_SNAPSHOT_RUNS=3 ./tools/mir_diff.sh snapshot` →
  `total=39 stable=34 unstable=5`。5 个不稳定文件（`_zeta_local_drv` /
  `jq_wufu_local` / `wufu_bt` / `wufu_v1` / `wufu_v2`）**每个只剩 2 行翻转**，
  且是同一个根因（下节）；此前是 161324 行。

### 残余不确定性的定位（下一个节点，不进本批）
`class BaoStockSource` 在 `backend/datasrc/market_data_sources.py:56` 与
`backend/datasrc/sources.py:99` 各定义一次 ⇒ 两个模块限定名都是合法值，`self`
槽类型取自一张**按裸名全程序建键**的映射（`Resolver::py_member_aliases`，
resolver.rs:67），其构建遍历 HashMap ⇒ 后写者胜、谁后写随机。
判别依据：翻转出现在 `--dump-mir` 中，而 dump 早于 `refine_param_types`
（main.rs:660 vs :668）⇒ 随机源在 resolver 侧而非类型细化侧。
这不是"dump 还不够好看"，而是**生成的程序自身可能拿错 `self` 类型**
⇒ 修法 = 撞名响亮告警（W2002）+ 键带模块限定，独立成批。

### 附记：主线自检 h9 链接失败的归因（不改主线代码）
`/tmp/wl/h9.py` 链接期缺 `_validate_and_repair_stock_ohlcv`（Mach-O 前导下划线，
即 LLVM 侧的 `validate_and_repair_stock_ohlcv`）。**非本批引入**：批次 300 之前的
二进制（`/tmp/wl/zetac_head`）编译同一文件同样失败且更差（`_get_universe` 也缺）；
同一文件 3 次编译失败点完全一致 ⇒ 不是本批关心的随机性。**也不是解析截断**
（h9 的编译日志里没有任何 W1002，`data_cleaning.py` 全文解析通过）。
探针实测到的形状：该函数的 `__ret_tuple` 全局常量**已生成**（并触发
`重名全局常量：_validate_and_repair_stock_ohlcv__ret_tuple_2207，后者被跳过`），
但 `final_mirs` 里没有它的函数块；同时 `_M` 里该模块的拼写是
**`backend.datasrc_data_cleaning`（末段分隔符是点）**，而调用点查的是全下划线形式
⇒ 归因为**模块名 mangling 不一致**，与 `resolve_symbol` 的
`module_mangle`（`resolver.rs:1085`，`replace('.', "_")`）不同源。
留给主线批次 301（任务 #7），本批只登记不修。

**下一步**：refactor §9 立即档 ③ —— G.8a 假值桩标记 + `--report-stubs`
（registry 条目加 `stub=` 标记，编译期列出本次实际调用到的桩；零行为变更、冷文件）。

---

## 批次 303（refactor 立即档 ②+③ —— G.7a 盘点清单 + G.8a `--report-stubs`）

两件事同属"零行为变更的响亮化"：编译器已经会告警，缺的是**把告警变成可规划的
输入**。一个是截断告警要逐个编译才看得见，一个是假值桩只说"存在"不说"我这次
用上了"。

### G.7a 收尾：`tools/truncation_inventory.sh`（新）
`ensure_fully_parsed` 早已打 `[W1002] <N> line(s) at the end of the input were
NOT parsed …`，但 G.7a 验收要求的是**盘点清单**（受影响文件 + 被丢内容），
一次一个文件的告警给不了。脚本按 `--dump-mir` 跑一遍官方套件 + 语料（发现口径
与 `tools/corpus_baseline.py` / `mir_diff.sh` 一致），逐行提取
`计数 / 文件 / 断点位置 / 首段被丢文本`，按丢弃行数从大到小排。
只读报告：**永远 exit 0**，门禁仍是 `run_all.sh`。

实测（237 文件 = 官方目录递归 198 `.z`（门禁集是顶层 194，另有 `simd/`+`const/` 4 个）
+ 语料 39 `.py`）：**12 个文件命中，共 1805 行被丢，语料 0 命中**。前三名
`minimal_compiler.z` 757、`benchmark_simd_vs_scalar.z` 357、`selfhost.z` 158。ARCHITECTURE-REVIEW §P0#3 记的是"11 文件"，按现测为 12。
⇒ G.7b（顶层同步恢复）的范围从此可逐文件核对，而不是估。

踩坑记录（同类脚本会再踩）：macOS 的 BSD sed **不支持 BRE 量词 `\+`**，
`s/…\([0-9]\+\) line/…/` 静默不匹配、字段落 `?`；写 `[0-9][0-9]*`。

### G.8a：`--report-stubs` —— 本次编译真正踩到的假值桩
**标记侧（已有）**：`--list-stubs` 给 18 个（registry 1 + pylib `# stub:` 17），
即 §4.4 清单。**缺的是接线**：清单说"存在"，没说"你这个程序踩了几个"。

- **`src/middle/pylib.rs`**：新增 `StubSite{marker, exact, member}` 与
  `stub_call_shape / dispatched_member / stub_sites / stub_call_match`。
  marker→MIR 调用目标三种形状（`numpy.vstack`→`numpy__vstack`、
  `pandas.DataFrame.dropna`→`DataFrame::dropna`、单段名即自身，`soft:` 前缀先剥）；
  接收者静态类型未知时按**成员名**命中，并撤销 codegen 的消歧后缀
  （`[dynamic]str::isin`、`isin_inst_i64`、`isin__2` → `isin`）。
  宁多报不漏报：成员名命中单独标注 `name-dispatched, may over-report`。
  一处真 bug 在测试里抓到：`isin__2` 的前缀切完还剩尾分隔符 ⇒ `trim_end_matches('_')`。
- **`src/main.rs`**：`report_stub_calls(&all_mirs)` 递归 `If/For/While`（含
  `pre_cond`、`else_body`）收集 `Call/VoidCall` 的 `func`，在
  `refine_param_types` 之后调用；**只写 stderr**（stdout 归 `--dump-mir`），
  命中为空时一行都不打。

验收实测（三向）：
- 不调桩 ⇒ 零输出：`tests/python_style/t302_nested_def_in_closure.z`；
- 精确命中：`t210_df_tail_dropna.z` → `soft:pandas.DataFrame.dropna  x1  soft: fake value  as \`DataFrame::dropna\``；
- 真实语料：`l1_fixed_pool_momentum.py` → `numpy.log x1 (aborts at run time)` +
  `soft:pandas.DataFrame.dropna x2`，名字与 §4.4 清单逐字一致。

### 验证（串行门禁）
- `cargo build --release -p zetac` 干净；`cargo test --release --lib pylib` →
  **8 passed**（4 个 G.8a 新测：形状映射、消歧后缀还原、exact/name 分类、
  "每个已登记 marker 都有可命中形状"）
- `./tools/run_all.sh` → official **194/194** · python_style **284 passed,
  2 failed**（仍只有存量红 t231/t233）· 语料 **39/39** ⇒ 三基线数字与批次 302
  逐项相同，"零行为变更"成立（`--report-stubs` 默认关，报告只写 stderr）

**下一步**：立即档 ④（§5.2 known-fail 包：机制已在，用例待补）。

---

## 批次 304（refactor 立即档 ④ —— §5.2 known-fail 包）

### 为什么先改机制，再补用例
§5.2 说"known-fail 机制已在、用例为零"。**机制其实半只脚是错的**：旧路径只做
"编译 rc==0 ⇒ XPASS（可摘除标记）"。而 §5.2 点名的坏形态（元组 `in`、`%s`、
`df[掩码]`）**全都编译通过** —— 它们错在值上。也就是说：一旦给它们登记
known-fail，套件立刻告诉你"标记可以摘了"，等于用门禁把已知错值伪装成待清理。
所以本批先把判定改成**按 expect 的值判定**，再登记用例。

### `tests/python_style/run.sh`
- 新增 `verdict ok|bad <name> <detail>`：四个出口（expect-error / 编译失败 /
  expect-abort / 值比对）统一走它，known≠0 时分别落到
  `XPASS / KNOWN-FAIL`，否则仍是 `PASS / FAIL`；普通用例的逐字输出格式不变。
- known-fail 用例**忽略** `expect-error` 与 `expect-abort`：拒编和 abort 就是缺口
  本身，不是契约。
- 语义：值与 expect 逐字相同才 XPASS（= 真的修好了、标记该摘）；其余一律
  KNOWN-FAIL，单列不计通过率、不影响 `run.sh` 退出码（不破门禁）。

### 5 条用例（t4xx 段，§5.2 指定）
| 文件 | 现状（实测） | 参照 |
|---|---|---|
| `t401_percent_s_from_dict.z` | `"f=%s" % d["name"]` 打 **13**（句柄整数），同行 f-string 正确 | CPython `f=abc` |
| `t402_tuple_variable_membership.z` | `2 in t`（元组变量）恒 False，编译期有 W 告警但值照旧错 | CPython `True` |
| `t403_floor_div_inside_call.z` | `print(x // y)` ⇒ `[W1002] … 1 line(s) NOT parsed`，整个 print 被丢，程序**不打字仍 rc=0** | CPython `3` |
| `t404_df_row_index_via_var.z` | `idx=[0,2]; len(df[idx])` → **1**，而字面量 `len(df[[1,3]])` → 2（批次 284 的判据在经变量时不成立） | 方言意图（真 pandas 此处 KeyError） |
| `t405_hard_stub_aborts_loudly.z` | **正向**：`pd.date_range` → `zeta: stub not implemented`，rc=134 | G.8 响亮侧 |

t403 是 G.7b 的头号靶子，也是本批唯一的新增语言缺口发现：`//` 在**括号内**（调用
参数、元组、列表同理）截断解析，写成 `z = x // y` 则正常；`.py` 与 `.z` 同表现
（不是方言差异）。语料现有 3 处 `//` 全在赋值 RHS ⇒ 与"G.7a 盘点：语料 0 命中"
一致，主线暂不受影响。

### 验证
- 新语义正反两向：4 条 known-fail 全部 `KNOWN-FAIL`（带原因），临时探针
  `t406_xpass_probe`（把已修好的 t302 标上 known-fail）→ `XPASS … 可摘除标记`，
  随后删除探针。
- `./tools/run_all.sh`：official 194/194 · python_style **285 passed, 2 failed,
  4 known-fail, 0 xpass**（存量红仍只有 t231/t233；+1 pass 即 t405）· 语料 39/39
- **口径提醒**：`run.sh` 必须从仓库根跑。从 `tests/python_style/` 里跑，31 个
  pandas/numpy 用例因 cwd 找不到 `pylib/*.z` 而红（实测 254 passed / 33 failed）
  —— 与本批改动无关，是既有前提。

**下一步**：立即档 ⑤ —— G.5a 槽位值表示表（`docs/ABI.md`，文件尚不存在）。

---

## 批次 305（refactor 立即档 ⑤ —— G.5a `docs/ABI.md` 槽位值表示表）

### 这一章要消灭的复发条件
批次 291/297/298/299/300/301 六连发是**同一个 bug 的六个面**：一个 8 字节槽里到底
放的是值、位模式、指针还是句柄，全靠读侧运行时猜。猜错的表现形式是静默错值
（4.94e-322、`500.0+50` 算成 4.6e18、持仓恒为 0）或约 25% 概率的 SEGV。
G.5a 的要求是**只写现行实现、每条锚定 `file:line`** —— 不是愿望清单，否则下一章
立刻和代码分家。

### `docs/ABI.md`（新建，纯文档，零行为变更）
- **§1 槽位值表示表**：10 行（i64 / f64 位模式 / bool 作 i64 0-1 / str 指针 /
  str packed / vec 句柄 / map 句柄 / PyJson / 用户 struct / PyDynamic 预留），
  每行给"槽内编码 + 写侧产生处 + 读侧要求 + 混淆史（实测）"。
- **§2 写/读对称规则 R1–R6**：整数入浮点槽走 `slot_sitofp`、裸容器字里的 f64
  出槽走 `bitcast`（并标注 `slot_read_ids` 是 F 轴落地前的**代偿**，届时删除）、
  str 判形只在 `map_str_key` 一处、句柄/裸整数不可静态区分故判形只能靠几何不变量
  （推论：**改 GC 块布局就是改本章**）、bool 恒以 0/1 进槽、struct 字段恒 64 位槽。
- **附 A 探针反查表**（不占用正式 §3 起的编号）：7 个判形点逐个对表行
  （`map_str_key`、`zt_dyn_is_map`、
  `zt_dyn_vec_hdr`、`zt_packed_cstr_eq`、`zeta_dyn_len`、`zj_make`/`ZJ_*`、
  `ZT_PROBE_LOC`）。验收口径"§1 穷尽"= 没有探针假设了表外的表示，且表中每行
  至少有 1 个探针或写侧锚点 —— 两条都成立。
- **附 B OPEN 三条只登记不决定**：packed 的**产生侧**未成文（判形全在读侧）、
  PyJson 与 map 靠值域区分本质脆弱、调用约定与 struct 返回留给 G.5b。
- 章节规划按 G.5a–e 分次落：本批 §1/§2（+ 附 A 验收表、附 B OPEN），§3 调用约定=G.5b，
  §4/§5/§6 修饰·布局·跨边界=G.5c，audit 附录+流程规则=G.5d，符号注册表=G.5e。
  ⚠️ 初稿把验收表/OPEN 写成 §3/§4，与预留编号撞车，落笔后已改为附 A/附 B。

### 两处落地时的自我纠正
1. 初稿把前提写成"每个槽都是 i64"。读 codegen.rs:1576-1580 后否定：静态可定的
   本地按自身 LLVM 类型开槽（`F32`→`float`、`F64`→`double`），所以"8 字节槽"
   只覆盖**其余本地槽 + 容器里的裸字**。改前提后 R1 才自洽。
2. 初稿头注把 §5 归给 G.5d；照 refactor.md 的 G.5c 正文重读后修正为上表的映射。

### 锚点核查（19 处逐条 sed 验证）
`codegen.rs` :60 / :1574 / :1576 / :1613 / :1683 / :3274-3299(`slot_sitofp` :3295) /
:3728 / :5647 / :5727 / :6157 / :7283；`py_additions.c` :105 / :1603 / :3457 /
:3469 / :3492；`tokio_runtime_stub.c` :190 / :2550-2556(`ZJ_*` 全 7 个) / :2560。
**`tokio_runtime_stub.c` 由并发工作流持有：本章只引用、不修改。**

### 验证
- `./tools/run_all.sh`（2026-09-21T17:12:49Z）：official **194/194** ·
  python_style **285 passed, 2 failed, 4 known-fail, 0 xpass**（存量红仍只有
  t231/t233）· 语料 **39/39 = 100%** —— 与批次 304 三项数字逐项相同。
- 本批未动 `src/`、`runtime/`、`tests/`，只有 `docs/ABI.md`（新）+ `roadmap.md`
  ⇒ "零行为变更"由基线数字复证，而非仅由改动面推断。

**下一步**：立即档 ⑥ G.2（sanitizer 化：ASan/MSan 下的可复现崩溃现场）。

---

## 批次 306（refactor 立即档 ⑥ —— G.2 sanitizer 常态化 + 还债① t228）

**本批性质**：只读工具 + 链接器入口的**加法**，不动 `runtime/`、不动 `src/middle`、
`src/backend`。落 G.2 之前先量它到底能覆盖什么，结果比原设想小，所以本批的交付
一半是工具、一半是**把覆盖边界写成可复现的口径**（免得下一个"零命中"被读成"没有内存错误"）。

### 交付物
1. **`tools/asan_run.sh`（新，只读）**：把 `runtime/*.c` 编成 ASan 插桩目标文件放进
   `/tmp/zeta_asan_rt`，用 `ZETA_RUNTIME_DIR` + `ZETA_EXTRA_LDFLAGS=-fsanitize=address`
   让 zetac 链接插桩版，逐用例编译并运行 `tests/python_style/t*.z`，输出命中清单。
   - 仓库根的 `zeta_runtime_c.o` / `tokio_runtime.o` 是 **git 跟踪产物**：本脚本只往
     `/tmp` 写，绝不覆盖它们。
   - **插桩目标文件必须与链接器同一个 driver 产出**。实测：Homebrew clang 的 .o +
     `/usr/bin/gcc`（=Apple clang 17）链接 → `Undefined symbols:
     ___asan_version_mismatch_check_v8`。所以 `ASANC="${ZETA_ASAN_CC:-gcc}"`。
   - `ASAN_OPTIONS=detect_leaks=0` 必带：GC 保留的块会被泄漏检查全部误报。
   - `--selftest`：阳性对照 + **盲区对照**写进脚本（见下），`--strict` 供 nightly 用。
2. **`src/main.rs` 三处加法**：`extra_ld_flags()`（:496，读 `ZETA_EXTRA_LDFLAGS`；
   **未设置时链接命令与历史逐字相同**）、两处链接点注入（:903 / :1109）、
   `find_runtime_obj` 的 **[W2002]**（:458 —— cwd 副本压过 `ZETA_RUNTIME_DIR` 时告警，
   `ZETA_STRICT_RUNTIME_DIR=1` 时改为要求覆盖版，:463）。
3. **`tools/run_all.sh` [W2003]**（:38-49）：跑基线前比对 `.o` 与 `runtime/*.c` 的
   mtime。原因：`run_all.sh` 只做 `cargo build -p zetac`，**从不重编 C 运行期**，
   改了 `.c` 忘了 `build_runtime.sh` 时，三条绿色数字描述的是旧运行期。
   ⚠️ **这条改动目前只在我本机生效**：`.gitignore:124` 的 `run_*` 是无锚定模式（本意
   挡仓库根的 `murphy_*/zeta_*/test_*/build_*` 之类脚本），把 `tools/run_all.sh` 一起
   挡住了 ⇒ 该文件从未入库（`git ls-files tools/run_all.sh` 空）。
   **连带后果（本批发现，未处理）**：`.github/workflows/ci.yml:106` 的 baselines job
   直接 `./tools/run_all.sh` —— 干净 checkout 上该文件不存在。self-hosted 常驻 runner
   留着未跟踪的本地副本，所以这个失败在 CI 上是隐形的。
   修法是一行 `git add -f tools/run_all.sh`，但**不擅自做**：常驻 runner 的工作区里
   有一份同名未跟踪文件，检出新增同名文件会以 "untracked working tree file would be
   overwritten" 直接失败，需要先确认 runner 侧怎么处置。⇒ 登记为 OPEN，见下。
4. **`tools/mir_diff.sh`**（:58）：`find … || true`。

### 实测数字（可复跑）
- **语料级 ASan 扫描**：`asan sweep: 291 program(s)  0 ASan-hit  2 crash  287 ok
  2 exit!=0  0 compile-fail`（两次独立复跑同数）。2 crash = 存量红 t231/t233；
  2 exit!=0 = t209 rc=1、t48 rc=5 —— **zetac 生成的 main 直接拿表达式值当退出码**，
  非零退出不是崩溃信号，故单列 EXIT 桶而不计入 CRASH。
- **阳性/盲区对照**（`tools/asan_run.sh --selftest`）：
  `malloc-overflow=detected (rc=134)  GC-overflow=silent (rc=0)` ——
  同样写 `p[72]`/`q[72]` 越界，`malloc(64)` 上 ASan 报 heap-buffer-overflow，
  `GC_malloc(64)` 上**完全静默且退出 0**。Boehm GC 只向系统要大块、块内相邻分配
  之间没有 redzone ⇒ **vec/map/struct 的内部越界本工具看不见**。
- **t228 非确定性专项**：`/tmp/t228chk/t228` 连跑 **200 次，全部 rc=0，输出 200 次
  同一指纹**（`shasum` 前 8 位 `3ba82abe`），且输出与 7 条 `// expect` 逐行相同；
  `MIR_SNAPSHOT_RUNS=20` 下 t228 单独 snapshot 给出
  `mir_diff snapshot: total=1 stable=1 unstable=0`（20 次 `--dump-mir` 字节一致，
  命令：`MIR_CORPUS=/tmp/zeta_empty_corpus MIR_SNAPSHOT_RUNS=20 ./tools/mir_diff.sh
  snapshot --file tests/python_style/t228_df_dedup_itertuples.z /tmp/zeta_t228_only`）；
  ASan 下另跑 20 轮编译+执行无命中。**⇒ 今天复现不出该用例的非确定性。**

### 顺手量出来的一个发现：语料层 MIR **不是** 20 次稳定
同一把命令放宽到整个语料（39 文件 + t228）时：
`mir_diff snapshot: total=40 stable=35 unstable=5` ——
UNSTABLE = `_zeta_local_drv.py`（主线驱动本身）、`jq_wufu_local.py`、`wufu_bt.py`、
`wufu_v1.py`、`wufu_v2.py`。口径要说清：T0（批次 302）用默认 `MIR_SNAPSHOT_RUNS=2`，
本批用 20，捕获概率差一个量级 ⇒ 这**不一定是 302 之后引入的回归**，更可能是
2 次抽样漏掉的既有抖动。影响：`mir_diff` 作为"纯代码移动"的保险绳，在这 5 个文件上
**只有 2 次抽样级别的可信度**。这正是任务 #9（跨模块同名裸名别名表歧义）的形态，
`wufu_*` 就是它的样本 ⇒ 记为 #9 的输入，不在本批追。

### 三个"静默缺陷"是本批用起来才暴露的
1. `mir_diff.sh` 在 `set -e` 下被 `find` 的失败中止了整个分组管道 ⇒ `--file` 传入的
   文件**无声消失**，脚本转而报 "no input files"。这个工具的存在意义就是"给一个文件
   也能 diff"，此前从没这么用过。
2. `run_all.sh` 从不重编 C 运行期（[W2003] 的来源）。
3. `find_runtime_obj` 先看 cwd 再看 `ZETA_RUNTIME_DIR` ⇒ 从仓库根跑 ASan 扫描时，
   **插桩版会被仓库根的未插桩 .o 悄悄顶掉**，而"0 命中"看起来完全正常。
   修法不是改优先级（那会破坏既有工作目录假设），而是加 [W2002] + strict 开关。

### 自我纠正（过程中写下又推翻的）
- 曾从"`malloc(8)` 越界在 -O1 下 ASan 静默"推出"本机 ASan 不工作"。错：那是死存储
  消除，-O0 有报告。结论已在写进任何文档前改掉，`--selftest` 固定用 `-O0`。
- 曾把 macOS 的 `Abort trap: 6` 归因于"`timeout` 给自己重发信号"。实测：子进程被信号
  杀死时 **bash 自己**打这一行，`timeout --foreground`、子 shell、`2>/dev/null` 都躲不掉；
  最终做法是把脚本自身 stderr 收进 `notices` 文件、结束后只回显非作业提示行
  （主循环 `done 2>"$LOG/notices.log"`；`--selftest` 分支同样处理，否则它自己漏一行）。
- 首轮 t228 连跑打印 "200 1" 是假的：`/tmp/t228x` 不存在，200 次重定向全部失败 ⇒
   rc=1 全进"失败"桶。`mkdir -p` 后才是上面的 200/200。

### 覆盖范围（诚实口径，写进脚本头部）
插桩的只有 **C 运行期**；`zetac` 生成的目标代码**不**插桩（需要 LLVM 的 ASan pass，
且前提是 IR 里指针/i64 不混用 —— 那是轴 B/F 的活）。看得见：运行期 C 的栈/全局越界、
非 GC 堆（malloc/strdup）的 overflow 与 use-after-free、把整数当指针解引用
（正好是批次 291/297 那一类）。看不见：GC 块内部越界（实测见上）。
⇒ **要覆盖后者需要 G.2b**：给 vec/map/struct 分配加自带 canary（`ZT_CONTAINER_GUARDS`），
在 push/insert/grow 处检查；本仓 libgc 无 `GC_first_obj`/`GC_next_obj`，堆遍历这条路不通，
`-DGC_DEBUG` 只是廉价近似。

### t228 结论：从"还债首位"降级
不是"查完了没问题"，而是**"现有工具查不动它"**：唯一残留的嫌疑（GC 块内越界 /
未初始化读）恰好落在实测盲区里。⇒ 排到 **G.2b 落地后再判**；§8 表①、§9 表 G.2 行、
立即档 ⑥ 均已按此改口径。

### 验证
- `./tools/run_all.sh`（2026-09-21T17:50:18Z）：official **194/194** ·
  python_style **285 passed, 2 failed, 4 known-fail, 0 xpass**（存量红仍只有
  t231/t233）· 语料 **39/39 = 100%** —— 与批次 305 三项数字逐项相同。
- [W2003] 本次未触发（`zeta_runtime_c.o`/`tokio_runtime.o` 23:51 均新于四个 `runtime/*.c`）。
- `src/main.rs` 改于门禁开始前（01:23 本地），门禁覆盖的就是本批要提交的代码。

**下一步**：立即档 ⑦ G.1（-O3 诊断，只读）。G.2b（canary）作为独立批次排在其后。

---

## 批次 307（refactor 立即档 ⑦ —— G.1 诊断档：优化级别对照矩阵）

**本批性质**：只读诊断 + 一个新工具。**未动** `src/`、`runtime/`、`tests/`。
G.1 的修复档（单变量旋钮、t06/t31 两根因）留给与主线协调的时点，不在诊断批里做。

### 结论先说
**"-O3 误编译"这个前提，方向要反过来。** 今天语料里没有任何"−O3 把对的编错"的现行；
有的是 2 例**只有 -O3 才正确**的用例，也就是 **`-O3` 正在掩盖生成 IR 自身的不自洽**：

| 用例 | -O3 | ZETA_NO_OPT=1 | 机制（实测） |
|---|---|---|---|
| `t06_struct_enum` | PASS | FAIL：Linking failed | 模块只有 1 个 `define`（main）；`@Color__Green` 是 `declare i64 @Color__Green()`（IR:1169）**从无定义**，却被 `store i64 ptrtoint (ptr @Color__Green to i64), ptr %6, align 4`（IR:1095）取地址当值存（枚举变体=函数指针）。-O3 DCE 掉这条死存储才链接得过 ⇒ 真实符号缺失 `_Color__Green` |
| `t31_builtins2` | PASS | FAIL：立即崩 | rc=138（SIGBUS），10/10 复现、stdout 零字节、无 stderr；链接通过 ⇒ 崩在生成代码本身，机制未定 |
| `t231`/`t233` | FAIL | FAIL | 存量红，与优化级别无关 |

### 优化开关的真实拓扑（这是"为什么矩阵只有两级"的答案）
* 唯一开关 `ZETA_NO_OPT`（存在即生效）**双联动**：jit.rs:24-26 跳过 `default<O3>` IR
  管线（管线字符串在 :29）+ jit.rs:168-172 把 TargetMachine 降到 `None`。
  ⇒ 用它无法二分到"IR pass"还是"后端 ISel"。
* JIT 路径另有硬编码 `OptimizationLevel::Aggressive`（jit.rs:70、:88）。
* **`-O0/-O1/-O2/-O3` 命令行是死码**：`compiler_config.rs` 解析（:121-124）写进
  `config.opt_level`（:22），全仓无人读；`CompilerConfig` 只出现在自身文件与
  lib.rs:52 ⇒ 整个文件不可达。真实 argv 循环 main.rs:587 白名单里没有 `-O*`，
  落到 `_ => input = Some(...)`（:614）被当输入文件 ⇒
  `zetac x.z -O0` 实测 `Error: Os { code: 2, kind: NotFound }`（:624，错误不提文件名）。
* **顺带量到同一循环的静默行为**：第二个位置参数**覆盖** `input` ⇒
  `zetac a.z b.z` 只编 b.z 且不提示。属 G.7"静默吃输入"家族，本批只登记。
* **MIR 层优化器也未接线**：`middle::optimization::optimize(mir, level)`
  （optimization.rs:490）零调用点 ⇒ 还债③"删 599 行"的前提成立，它本来就没在跑。

### 假设核对
* **假设 2（noundef/nonnull/noalias 声明不符、persistent TBAA 写错）→ 静态否证**：
  `src/` 全仓 `tbaa|noalias|noundef|nonnull` 零命中；t228 样本的实际 IR（6109 行）里
  这四个各 0 次。对齐只有 `align 4`(1919) 与 `align 8`(1179) 两档，无超额声明
  （超额才是危险方向）。**但 README.md:19 的 "persistent TBAA metadata" 宣称的实现不存在**
  ⇒ 文档-实现不符，要么删要么按 G.5 的"现状/愿望"分栏改写。
* **假设 1（未初始化槽读）→ 未否证，且新增了可查的形态**：t06 证明存在"依赖 -O3 DCE
  才合法的存储"；t31 的 -O0 立即 SIGBUS 是同一类嫌疑的更强信号。
* **轴 B 量化**（同一 6109 行样本）：`inttoptr` 110、`ptrtoint` 63、`alloca i64` 1172、
  `alloca double` 3 ⇒ "指针/i64 混用"有数了，这也是 G.2 生成码侧插桩做不了的原因。

### 工具：`tools/opt_matrix.sh`
两级各跑一遍 `tests/python_style/run.sh`，把 verdict 归一化成"用例→判定"后对比，
**exit 码 = 翻转数**。`--levels "O3 NO_OPT"` 可改级别集。脚本头部把上面的拓扑与
"为什么只有两级"逐条锚定，避免下一个人以为 `-O3` 矩阵已经能做。
验收口径据此修正为：**翻转数 = 0** 进 CI（不是"两级各自三基线绿"——-O0 侧今天就红）。

### 验证
- 矩阵：`O3: verdicts=287`、`NO_OPT: verdicts=287`，`flips=4`（=2 例 × 两行 diff）。
- `./tools/run_all.sh`（2026-09-21T18:19:45Z）：official **194/194** ·
  python_style **285 passed, 2 failed, 4 known-fail, 0 xpass** · 语料 **39/39 = 100%**
  —— 与批次 306 三项数字逐项相同（本批未动 `src/`，这是预期的复证而非巧合）。
- 矩阵跑在门禁**之前**，两者未并发（矩阵会调 291 次 zetac，与 run_all 的语料阶段互斥）。

**下一步**：⑧ 轴 A 死代码（`compiler_config.rs` 整文件、`middle::optimization::optimize`
及其 pass 群正是轴 A 的现成条目）；G.1 修复档三件（单变量旋钮 / t06 枚举变体定义 /
t31 -O0 SIGBUS 根因）作为独立批次，需与主线协调动 codegen 的时点。

---

## 批次 308（refactor 立即档 ⑧ —— 轴 A 死代码第一块：从未参与编译的孤儿簇）

**删了什么**（8 个 git 跟踪文件，全部 clean，工作区无并发编辑）：

| 目录 | 行数 | 文件 |
|---|---|---|
| `src/paradigm/` | 274 | mod.rs |
| `src/holographic/` | 440 | mod.rs |
| `src/temporal/` | 413 | mod.rs |
| `src/consciousness/` | 405 | mod.rs + types.rs |
| `src/reality/` | 51 | mod.rs + physics_engine.rs |
| `src/meta/` | 41 | mod.rs |
| **合计** | **1,632**（见下） | 8 |

> 口径说明：`git diff --cached --stat` 报 **1,632 行删除**，而我逐目录
> `find … | xargs cat | wc -l` 得 1,624，差 **8 = 每个文件 1 行**——这 8 个文件都
> 没有行尾换行，`cat` 拼接时每个文件的最后一行与下一个文件的首行合并成一行。
> **以 git 的 1,632 为准**；下面的"删除前后行数"同用 `cat` 口径，故差值也是 1,624。

**为什么这是轴 A 最干净的第一块**：这六个目录**从未被 `pub mod` 声明过**
（lib.rs 无、`src/**/mod.rs` 无、全仓 `^\s*(pub )?mod (holographic|temporal|consciousness|
reality|meta|paradigm)\s*;` **零命中**）⇒ 它们根本不进编译单元。因此删除
**不需要动 `src/lib.rs`** —— 而 lib.rs 此刻正被并发工作流改着（`M src/lib.rs`），
凡是"要删 lib.rs 里一行声明"的块（`crate::ml` + `crate::distributed` 6,226 行）
本批都做不了，只能等它干净。

**引用复核（按 §1 的口径含 bin/、tests/、examples/、.github/）**：
`tests/` `examples/` `benches/` 对 `holographic|temporal::|consciousness|crate::reality|
crate::meta|crate::paradigm` **零命中**；`Cargo.toml` 的 `[[test]]/[[bin]]/path=` 无一指向
这些目录；`src/paradigm/mod.rs:12-15` 里那些 `crate::holographic::init()` 只是**同簇的
死文本**（它自己也没被声明）。活跃侧只有 `src/paradigm_simple.rs`（lib.rs:70 声明、
`tests/unit/test_paradigm.rs:8` 调 `run_demo()`）——它带的是**自己内联的同名模块**
（`paradigm_simple.rs:7/41/80/143`），与磁盘上的死目录是两份拷贝，不受影响。

### 一处口径纠正
refactor.md §1 表里这块写的是 **1,309 行**——那只数了 holographic/temporal/
consciousness/reality 四个（440+413+405+51=1,309，逐目录 `wc -l` 已复算）。
同簇的 `src/paradigm/`（274）与 `src/meta/`（41）同样未声明、同样零风险，
本批**一并删除**，实际 **-1,632 行**（git 口径；`cat` 口径 1,624，差异见上表下的说明）。

### 顺带钉住一个我自己的度量卫生缺陷（重要）
本批第一次**不带管道**地跑 `./tools/run_all.sh`，拿到 **rc=1**；而此前几批的记录里
写的是"exit code 0"。真相：门禁的绿色判据是 `run_all.sh:134` 的 `py_fail -ne 0 ⇒ rc=1`，
而 t231/t233 两级都红的存量红**一直在** ⇒ **run_all.sh 从批次 302 起每次都是 1**，
之前那次"0"来自 `./tools/run_all.sh | tail -25` —— 管道的退出码是 `tail` 的。
⇒ 后果不严重（我记录的是 JSON 里的三项数字，它们是真的），但**"门禁绿了"这句话
在此前几批里不成立**。以后：门禁一律 `>文件 2>&1; rc=$?` 直读退出码，
并把它与 JSON 三项一起记。**这不是本批引入的回归**（本批三项数字与批次 307 逐项相同）。

### 验证
- 删除前后 `find src -name '*.rs' | xargs cat | wc -l`：92,219 → **90,595**
  （差 1,624 = `cat` 口径；git 报 1,632，说明见上）。
- `cargo build --release -p zetac -q` rc=0、无新告警。
- `./tools/run_all.sh > /tmp/gate_308.txt 2>&1`（2026-09-21T18:27:00Z）：official
  **194/194** · python_style **285 passed, 2 failed, 4 known-fail, 0 xpass**（红的仍是
  t231/t233）· 语料 **39/39 = 100%** —— 与批次 306/307 三项逐项相同；
  脚本 rc=1 的原因见上节（存量红触发的既有判据，非本批回归）。

**下一块**（按"不碰脏文件"排序）：`new_resolver` 双轨死管道 3,115 行
（`new_resolver.rs` 2,199 + `typecheck_new.rs` 690 + `type_cache.rs` 226，
只需改 `src/middle/resolver/mod.rs:5` 一行，该文件当前 clean）。

---

## 批次 309（refactor 立即档 ⑧ 续 —— 轴 A 第二块用"可编译性反证"验，结果推翻了一半的表）

**方法改变**：§1 表里的证据都是 grep 式的（"仅 `resolver/mod.rs:5` 一行声明"）。grep 只能
证明"没人这么写"，不能证明"删了还能编"。本批改用**反证**：真删 + `cargo check`，
让编译器来当引用检查器。代价极低（三文件在 HEAD 干净，`git checkout --` 即回滚）。

### 结果 1：`new_resolver` + `typecheck_new` **不是死码**，表的"待删 3,115 行"作废
删掉 `new_resolver.rs`(2,199) + `typecheck_new.rs`(690) 并去掉两行 `pub mod` 后
`cargo check -p zetac` **失败**，错误分三类：
* `error[E0432]: unresolved import super::typecheck_new` × 4 与
  `unresolved import crate::middle::resolver::typecheck_new` × 1 —— 引用者是
  `src/middle/resolver/unified_typecheck.rs`（:87、:112、:269、:289 四处
  `use super::typecheck_new::NewTypeCheck`），而 `unified_typecheck` 又被
  `src/middle/resolver/typecheck.rs:8` 引进来 ⇒ **在编译图的活路径上**。
* `error[E0599]: no method named string_to_generic_type found for &mut Resolver` × 2
  与 `error[E0624]: method string_to_type is private` × 4 —— 说明 `Resolver` 的这两个
  方法实现就在 `new_resolver.rs` 里，被外部（含 `src/middle/types/mod.rs`）调用。
⇒ 已**完整回滚**（`git status --short src/middle/resolver/` 为空、`cargo check` rc=0）。
⇒ §1 表这一行从"待删 3,115"改成"前提被否"；真要减这块，得先做**调用点手术**
（把 `unified_typecheck` 的双轨分支收敛到单轨），那是重构不是删除。

### 结果 2：`type_cache.rs`（226 行）**确为死码，已删**
同样反证：删文件 + 去掉 `resolver/mod.rs` 的 `pub mod type_cache;` ⇒
`cargo check -p zetac` **rc=0、零 error**（全仓引用侧也只有它自己文件头那行注释）。
`git status` 显示改动面 = `M src/middle/resolver/mod.rs` + `D src/middle/resolver/type_cache.rs`，
不含并发工作流的 `src/lib.rs`。

### 顺带把口径钉牢：为什么"pub 不等于活"这句这次不成立
`type_cache` 是 `pub`，lib crate 的 `pub` 项理论上可被集成测试用（`zetac::...`），
`cargo check` 只编 lib 不编 tests ⇒ 单看 rc=0 不够。补了两道：
`grep -rn 'type_cache' src/ tests/ tools/` 除自身文件外**零命中**；三基线复跑数字不变。

### 验证
- `cargo check -p zetac` rc=0（删 type_cache 后）。
- `./tools/run_all.sh > /tmp/gate_309.txt 2>&1`（2026-09-21T18:34:38Z）**直读退出码**：
  rc=1 —— 原因是批次 308 记过的既有判据（`run_all.sh:134` 要求 `py_fail==0`，而
  t231/t233 存量红一直在），非本批回归。三项数字 official **194/194** ·
  python_style **285 passed, 2 failed, 4 known-fail, 0 xpass** · 语料 **39/39 = 100%**
  —— 与批次 306/307/308 逐项相同。

**轴 A 剩余**：`crate::ml` + `crate::distributed`（6,226 行，表里证据是 grep 式，
**同样需要用本批的反证法重验**，且删它要动正被并发工作流改的 `src/lib.rs` ⇒ 阻塞）。
新增一条更值得做的候选：`new_resolver.rs` 里只有 `string_to_type` /
`string_to_generic_type` / `InferContext` 等少数项被外部引用 ⇒ **文件内**的
未引用部分是真正的低垂果实（但需要逐项反证，不是一次删除）。

---

## 批次 310（refactor 立即档 ⑧ 续 —— 轴 A 第三块：给"死代码"装上机器判据，顺手清掉 new_resolver 的假 API）

### 先说本批真正的产出：一条被掩掉的整类缺陷
`src/lib.rs:7` 与 `src/main.rs:7` 各有一行 **`#![allow(dead_code)]`**。
⇒ 全仓 dead_code 告警平时**一条都不输出**，"cargo check 干净"在这仓里**不等于**"没有死代码"。
这也解释了批次 308/309 为什么要靠 grep 猜、为什么 §1 表的行数敢写"1.6 万"——判据本身是关的。

绕开它不需要改 `src/lib.rs`（正被并发工作流持有，本批一行未动）：
`RUSTFLAGS="--force-warn dead_code"` 在命令行上强制打开该 lint，优先级高于源码里的 allow。

### 新增判据工具：`tools/dc_audit.sh`（可重复、带基线）
- 默认：打印本仓 `src/` + `tests/` 的命中清单（`file:line<TAB>消息`）与总数、按文件排行。
- `--snapshot` / `--diff`：基线快照与"只允许减少、新增即 rc=1"。
- 当前基线：**116 条**（`/tmp/zeta_dc_baseline.txt`）。

### 两次踩坑（都已写进脚本注释，避免下一批复发）
1. **缓存会伪造"零死代码"**：cargo 命中缓存的那次运行不重发 warning ⇒ 看着像 0 命中。
   脚本里 `touch src/lib.rs`（只改 mtime，不碰内容）强制重编 zetac 一个 crate，
   再加自检：输出里没有 `Checking zetac` 就 `[E2002]` 直接 rc=3，不产出清单。
2. **`= note: requested on the command line with --force-warn dead-code` 不能当过滤器**：
   实测 116 条里只有 **23** 条带这条 note，用它过滤会静默丢掉 80%。改成"消息措辞"过滤
   （`(is|are) never (used|read|constructed|called)`），并与每条 warning 的**第一个** `-->` 配对。
   （另外：路径判据必须 `^(src|tests)/`，因为依赖 crate 的绝对路径里也含 `/src/`，
   第一版因此把 110 这个噪声数当成了基线。）

### 用新判据清掉的第一块：`new_resolver.rs` 的 pub 面
文件 2,199 行，但外部真正引用它的只有 `typecheck_new.rs:48/61/69/84/440` 五处 ⇒
活的 API 只有 `InferContext::{new, add_function, infer, take_substitution, solve}`。
做法（批次 309 的"可编译性反证" + 本批的强制 lint）：先把候选 pub 项临时降为私有，
`--force-warn dead_code` 立刻点名 4 个真死项，删之；再把"确认无外部引用"的项**留在私有**：

| 项 | 处置 | 判据 |
|---|---|---|
| `enum Constraint::Bound`（:15）+ `solve()` 里的 Bound 分支（原 :1730-1734） | **删** | "variant `Bound` is never constructed" ⇒ 该分支不可达，`satisfies_bound` 那段是死路径 |
| `InferContext::substitution()`（原 :1710） | **删** | method never used（`take_substitution` 才是外部用的那个） |
| `InferContext::final_type()`（原 :1915） | **删** | method never used |
| `fn type_check()` 自由函数（原 :1937） | **删** | function never used —— 注意：§1 表说的"入口函数"其实是它，入口从未接线 |
| `Constraint` / `lookup` / `declare` / `constrain` / `enter_generic_scope` / `exit_generic_scope` / `infer_generic_call` / `register_builtin_generics` | **降为私有**（保留实现） | 文件内有用到（不报死），但 `grep -rn 'new_resolver::'` 证明外部零引用 ⇒ 不是 API |

净变化：`new_resolver.rs` **-39 行**（`git diff --stat`：8 insertions / 47 deletions），
文件内 pub 项从 16 个降到 6 个（1 个结构体 + 5 个方法）。

### 为什么"降私有"算架构收益而不是顺手美化
`pub` 在这个 crate 里 = "对外可达"，编译器据此永不判死 ⇒ 只要 API 面虚胖，轴 A 的
可审计性就是零。降私有之后这些项**永久纳入 dead_code 判据**，下一批能自动发现它们变死。

### 下一批的现成靶子（都是编译器点名的私有死项，位置精确）
`src/middle/ctfe/evaluator.rs:991`、`src/middle/mir/gen.rs:214`（5 个 async 相关字段）、
`src/middle/resolver/resolver.rs:115`、`src/middle/resolver/typecheck.rs:559`、
`src/middle/types/mod.rs:1372`；按文件量最大的是 `src/bin/zorb.rs`(12)、`src/main.rs`(9)、
`src/lsp/protocol.rs`(7)、`src/ml/` 目录 17。

### 验证
- `cargo check -p zetac --tests` rc=0；`cargo test -p zetac --lib new_resolver` **7 passed / 0 failed**
  （文件内 `mod tests` 用私有项仍可访问，子模块能看父模块私有项，故未受可见性收紧影响）。
- `./tools/run_all.sh > /tmp/gate_310.txt 2>&1`（2026-09-21T18:50:38Z）**直读退出码**：
  rc=1 = 批次 308 记过的既有判据（`run_all.sh:134` 要 `py_fail==0`，t231/t233 存量红）。
  三项数字 official **194/194** · python_style **285 passed, 2 failed, 4 known-fail, 0 xpass** ·
  语料 **39/39 = 100%** —— 与批次 307/308/309 逐项相同 ⇒ 零基线位移。
- `./tools/dc_audit.sh --diff` → "无新增命中"，且 `src/middle/resolver/new_resolver.rs` 命中数归零。

---

## 批次 311（refactor 立即档 ⑧ 续 —— 轴 A 第四块：判据说"死"，还要判"该不该删"）

批次 310 的判据只回答"有没有引用"，不回答"删了是不是在掩盖缺陷"。本批把 `src/middle/`
的 5 条命中逐条读实现后分成两类，**只删 (a) 类**：

- **(a) 被近亲取代 / 重复挂载** ⇒ 删，无信息损失；
- **(b) 已实现但从未接线的能力** ⇒ **不删**，它记录的是一个真实缺口，删掉就等于把缺陷埋了。

### (a) 类：删了 48 行
1. `src/middle/ctfe/evaluator.rs` 原 990-1029 的 `eval_if_expr`（**40 行**）。
   判据：同文件 `:1033` 的 `eval_if_expr_with_else` 才是活路径 —— `:610` 的
   `AstNode::If { cond, then, else_ } => self.eval_if_expr_with_else(...)` 用的是后者。
   前者签名带 `else_branch: Option<&AstNode>`，是被"else 是多语句块"这一版取代的旧形状。
2. `src/middle/resolver/resolver.rs` 的 `identity_inference` + `capability_inferencer`
   两个字段 + `Resolver::new()` 里两处初始化（**8 行**）。
   判据："fields ... are never read"（只构造于原 :136-137 / :157-158，全仓无读取点）；
   **真正在用**的那份在 `src/middle/passes/identity_verification.rs:13`（`:275` 有调用）。
   ⇒ 这是同一能力的**重复挂载**，不是未接线能力。
   疑点排除：`identity` 特性（`Cargo.toml` 里是空 flag，`identity = []`）不会让它变活 ——
   `ZETA_DC_FEATS=--all-features` 下这两项**同样**判死（见下）。

### (b) 类：3 条命中判"不删"，其中一条是缺陷证据
- **`src/middle/types/mod.rs:1372 unify_array_size`（49 行）—— 本批最有价值的发现。**
  活路径 `unify` 的 Array 分支在 `:1569` 内联实现，语义**比死掉的这个更弱**：
  * 内联版：`size1 != size2 && !(size 是 Literal(0)) ⇒ Err`，且注释自己承认
    "Allow size 0 as a wildcard (for type inference) — This is a hack to support array subscripting"；
  * 死版：`ConstParam` 同名才统一、`ConstParam` 与 `Literal` 互容、`Expr` 同名才统一。
  ⇒ 内联版对 `[T; N]`（N 为 const 参数）与 `Literal` 的组合会**误判不可统一**。
  删掉 `unify_array_size` = 把这条已知的 const-generics 缺口一起销毁。改为登记任务，
  与轴 B / 任务 #22 的双轨收敛合并处理（同一个"两套实现、活的那套更弱"的模式）。
- `src/middle/mir/gen.rs:214` 的 `async_state_ptr` / `async_segment_count` / `is_async_fn` /
  `async_saved_vars` + `closure_counter`（5 个 never-read 字段）：删字段要连带删**写入侧**，
  而 `closure_counter` 属 PY-A 闭包命名链路、`async_*` 属被放弃的 async 状态机 lowering 路线
  —— 归入批次 312 单独判（且 `gen.rs` 是主线最热文件，改动面要最小）。
- `src/middle/resolver/typecheck.rs:559` 的 `infer_identity_type` / `get_required_capabilities`：
  旧 track 里的 identity 能力推断未接线，与 `unify_array_size` 同属"保留哪一轨"的决断 ⇒ 并入 #22。

### 判据本身升级：dead_code 是按 cfg 配置算的
`tools/dc_audit.sh` 加 `ZETA_DC_FEATS`（如 `--all-features`）。实测：

| 配置 | 命中数（本批删除后） | 差异 |
|---|---|---|
| 默认特性 | **114** | — |
| `--all-features` | 116 | 恒多 2 条，都在 `src/integration/`（`coordination.rs:86` field `callbacks`、`type_context.rs:78` fields `stack`/`context`），只有开 `integration` 特性才进编译图 |

⇒ **基线必须按 cfg 配置分别存**（`ZETA_DC_BASELINE=...`）。另记一次误读：第一次量
`--all-features` 得到 52 条，是**缓存运行**（cargo 不重发 warning），脚本里的
`Checking zetac` 自检就是为拦这个。

### 顺带撞出来的门禁盲区（登记为任务 #23）
三套基线从不跑 `cargo test`，Rust 侧单测无人监管。实测 `cargo test -p zetac --lib`：
并行跑 **SIGABRT**（signal 6，报不出失败用例）；`--test-threads=1` → **135 passed / 1 failed**，
失败点 `src/frontend/indent.rs:944`（`header_colon_stripped_with_trailing_comment`）。
该文件相对 HEAD **零改动**（本批 `git diff --name-only` 只有 3 个文件），且是纯函数测试
⇒ 属 HEAD 上的存量失败，非本批引入。

### 验证
- `cargo check -p zetac --tests` rc=0，零 `error`。
- `./tools/dc_audit.sh --diff` → **无新增命中**，总数 116 → **114**（本批删的正好 2 条）；
  `src/middle/ctfe/evaluator.rs`、`src/middle/resolver/resolver.rs` 命中归零。
- `./tools/run_all.sh > /tmp/gate_311.txt 2>&1`（2026-09-21T19:00:29Z）直读退出码：rc=1 =
  既有判据（`run_all.sh:134` 要 `py_fail==0`，t231/t233 存量红）。三项数字
  official **194/194** · python_style **285 passed, 2 failed, 4 known-fail, 0 xpass** ·
  语料 **39/39 = 100%** —— 与批次 309/310 逐项相同 ⇒ 零基线位移。

---

## 批次 312（refactor 立即档 ⑧ 续 —— 轴 A 第五块：清掉 `src/bin/` 的一次性调试残渣，顺带撞出"CI 里根本不存在的性能门禁"）

### 为什么这批不只是"少 1,009 行"
`src/bin/*.rs` 由 cargo **自动发现**成 bin target，每次 `cargo build --release` 都编；
更要紧的是它们**直接调编译器内部 API**（`parse_simd_type`、`skip_ws_and_comments0`、
`parse_full_expr`、`Type::from_string`…）⇒ 只要它们还在，这些签名就动不得。
轴 B/C 的每次重构都在给这 11 个从来没人跑的文件付税。

### 判据（三条同时成立才删）
1. 头部语义 = 一次性版本对齐/单点 print 调试（如 `//! Analyze v0.5.0 syntax issues`、
   `//! Performance benchmarking for v0.3.24`），**不是** `#[test]`、不带断言、不进 CI；
2. 全仓引用为零：`grep -rl <name>` 排除 `.git`/`target`/`conversations` 后只剩
   `.codegraph/codegraph.db`（索引自身的陈旧条目）；
3. `cargo check -p zetac --all-targets` 删后 rc=0（`--all-targets` 才真编 bin+test+example，
   只 `cargo check` 会漏 —— 批次 309 的教训延续）。

**保留**（有明确用途，不参与删除）：`zorb.rs`(315，包安装器 V1，头部带 usage)、
`zeta-lsp.rs`(50，LSP 入口)、`indent_dump.rs`(14) 与 `pipeline_dump.rs`(59)（带
`cargo run --release --bin …` 用法注释、且 `validate.md` 在引用）。

删除清单（11 个，**1,009 行**）：`analyze_v0_5_0_syntax_issues` 266、`performance_benchmark` 257、
`real_v0_5_0_compatibility` 160、`primezeta_compatibility_test` 82、`direct_array_test` 67、
`test_array_parse` 44、`debug_simd` 44、`test_vector_from_string` 33、`debug_parse_type2` 26、
`debug_parse_if` 17、`debug_ws` 13。
（`performance_benchmark` 被删的理由：它是 v0.3.24 时代对 `parse_zeta` 打**内置字符串**的一次性
计时，**不是** ⑪ 要的"端到端 `time zetac` 前后对照"基线 —— 后者本批另立工具，见下。）

### `mir/gen.rs` 的 5 个 never-read 字段（15 行）：判成 (a) 类，删
批次 311 把这条留给"要连带删写入侧"的怀疑；实测**根本没有写入侧** ——
`async_state_ptr`/`async_segment_count`/`is_async_fn`/`async_saved_vars`/`closure_counter`
在 gen.rs 里的全部出现 = 4 处声明 + 1 处 `closure_counter` 声明 + `new()` 里的 5 行初始化
（`grep -n` 命中行号 214/216/218/220/226 + 282-288，无第三处）⇒ 只写常量、无人读的**纯残留**。
`closure_counter` 的身份已确认被取代：活的命名在 `:13138 self.closure_seq` + `:13170`
的 `__closure_{n}_{bare}_c{hash:08x}`（T0/B.5 为消除 HashMap 随机序而改），
其文档注释"monotonic counter for synthetic closure function names"读起来像还在用 ⇒ 删掉最省事。
"从不被读也从不被写的字段不可能影响 MIR"这一条比字节 diff 更强，故未做 `--dump-mir` 前后比对
（三基线 + 语料 39/39 仍照常复跑，见验证）。

### 撞出来的更大一块：`Benchmarks` 这个 workflow 引用的东西**仓里根本没有**
`.github/workflows/benchmarks.yml`（触发：push 到 main/dev + **每天 02:00 UTC cron** + 手动）里：
- `:106` `cargo build --bench compiler_bench`、`:148` `--bench runtime_bench` ⇒ **`benches/` 目录不存在**
  （`ls benches` → No such file or directory）；
- `:190/:199/:205/:209` `cargo build|run --bin regression_test` ⇒ **`src/bin/regression_test.rs` 不存在**；
- 而 `Cargo.toml:94` 明明有 `criterion = { version = "0.8.1", features = ["html_reports"] }`，
  且全仓**没有 `[[bench]]` 段** ⇒ 依赖付了、harness 没接。
⇒ **refactor ⑪"性能基线测量"在 CI 里是一个看起来存在、实际每天必然失败的作业**。
这既是假接线（同 #15 的 `run_all.sh` 未入库、G.8 的桩），也解释了为什么"性能没基线"这件事
一直没被当成缺陷 —— 门禁本身是幽灵。
（注：仓内 `criterion` 是 dev-dependency，而 `[[bench]]` + `harness = false` 要改
`Cargo.toml` —— 该文件正被并发工作流持有（当前未提交改动 -24/+2），所以本批**不改 Cargo.toml**，
⑪ 先走"仓内工具脚本"路线：`tools/perf_baseline.sh` 直接量端到端 `zetac` 编译耗时。）

### 实测收益（可抽查的数）
- `touch src/bin/*.rs && /usr/bin/time -p cargo build --release --bins`：
  删前 15 个 bin = **real 1.46s / user 7.78s**；删后 4 个 bin = **real 0.77s / user 3.26s**
  ⇒ CPU 时间 **-58%**（多核下墙钟差被并行掩盖，user 才是可信量）。
- `./tools/dc_audit.sh`（默认特性）命中 **114 → 101**（-13，`--diff` 无新增命中）；
  `src/bin/` 目录命中从 27 降到 12，全部集中在保留的 `zorb.rs`。

### 验证
- `cargo check -p zetac --all-targets` rc=0（`/tmp/chk312.txt` 内 error 计数 0）。
- `./tools/run_all.sh > /tmp/gate_312.txt 2>&1`（2026-09-21T19:12:20Z）直读退出码：rc=1 =
  既有判据（`run_all.sh:134` 要 `py_fail==0`）。三项 official **194/194** ·
  python_style **285 passed, 2 failed, 4 known-fail, 0 xpass** · 语料 **39/39 = 100%**
  —— 与批次 310/311 逐项相同 ⇒ 零基线位移。

## 批次 313（refactor 立即档 ⑪ —— 性能基线：先把"能不能判"这件事量清楚）

### 为什么不是 CI
批次 312 已证 `.github/workflows/benchmarks.yml` 是幽灵作业（`benches/` 与
`src/bin/regression_test.rs` 都不存在，`Cargo.toml:94` 付了 criterion 却没有 `[[bench]]`），
而补 `[[bench]]` 要改正被并发工作流持有的 `Cargo.toml` ⇒ ⑪ 走仓内脚本
`tools/perf_baseline.py`（294 行，新增）。

### 口径设计（三处判据都是**量出来的**，不是设计时拍的）
1. **成功判据必须分阶段**：照抄 `corpus_baseline.py` 的"`Compiled to`/`Linking failed`
   都算走完"在 `--emit-llvm` 这条路径上直接失效 —— 首版因此报 `failed_runs: 12/12`，
   数字全不可信。现按阶段判：`aot` 认 `Compiled to`(ok) / `Linking failed`(link-fail，
   编译阶段走完、链接是本机环境问题)；`ir` 用"LLVM AssemblyWriter 一定以
   `attributes #N = {…}` 收尾"作为**IR 打印已完成**的证据（`_ir_printed()`）。
   min 只在可信托里取；提前报错退出的轮次剔除并计入 `bad_runs`([W3001])，
   link-fail / 打印后才崩 计入 `noted_runs`([N3001])。
2. **暖机**：未暖机时"冷态首采基线"与"随后的稳态测量"差 +38%（同一份二进制，
   aot total 27,689 → 36,320 ms），`--diff` 会稳定报假回退 ⇒ 每次调用先丢弃一轮
   每文件每阶段各一次（`PERF_WARMUP=0` 仅供调试）。
3. **判定统计量**：同一份二进制暖机后做 5 次独立调用，三种口径的复现性是
   `ir total` 41,655/41,016/42,288/42,460/41,132 ms ⇒ 散布 **3.5%**（可判）；
   `ir 逐文件中位` +0.6/+7.9/+8.1% ⇒ 12 个样本的中位数被个别长尾单次抖动（最大 +24%）
   推动，比 total 更抖（弃用为判据）；`aot total` 30,186/36,421/32,451/33,206/33,067 ms
   ⇒ 跨调用漂 **20.7%**（含 clang 链接）⇒ 只打印作参考。
   ⇒ 默认 `PERF_GATE_STAGES=ir` + 阈值 10%；aot 侧要变可判需 A/B 交错（任务 #27）。
   另外 `--diff` 在暖机**之前**先校验 `corpus/n_files/repeat/阶段` 与基线一致，
   不一致直接 `[E3001]` 退出 —— 否则"文件数变少 ⇒ total 变小"会被读成通过。

### 基线数值（2026-09-21T20:14Z 采，`/tmp/zeta_perf_baseline.json`）
- 语料 `~/source/quant/REasyQuant/strategies` 取 sorted 前 12 个 `.py`
  （排除 `.venv` 与我方临时 `_zeta_local*` driver），`PERF_REPEAT=3` 取 min。
- `aot` total **30,185.8 ms**，逐文件 min 的中位数 419.5 ms；
  `ir` total **41,654.7 ms**，中位数 627.5 ms。
- 体量集中度：`ir` 前三 `wufu_bt.py` 11,782.8 / `wufu_v1.py` 11,637.0 /
  `jq_wufu_local.py` 11,212.8 ms = **83.1%**（`aot` 同口径 84.3%）
  ⇒ 轴 C 的"编译最慢的东西"就是这三棵，优化收益评估按 total 加权即按它们加权。

### 顺手撞出的真缺陷（→ 任务 #26）
`--emit-llvm` **每次**都在打印完 IR 之后 SIGSEGV（rc=139；本轮 36/36 次 `ir` 测量全崩，
连 2 行最小程序 `x = 1` 也崩）。lldb：`EXC_BAD_ACCESS (address=0x0)`、`frame #0: 0x0`
（跳到空函数指针），崩溃在 `src/main.rs:816 codegen.module.print_to_stderr()` 之后的退出
路径；对照同文件走 `-o`（aot）不崩 ⇒ 只在这条 dump 且跳过 `finalize_and_aot` 的路径上。
该 flag 自 batch 13x 的 `3b79889f` 就存在（未做历史复跑验证），非本批次引入。
影响面：任何用 `--emit-llvm` 做门禁/CI 的脚本都会被非 0 退出码误导。

### 验证
- 工具自证：`--diff`（未改任何编译器代码，同一份二进制）在 ir 判据上给出
  -1.3%~+1.9%，5 次全部 rc=0 ⇒ 判据不会自己造回退；口径不一致/缺基线/未知模式
  三类误用均 `[E3001]` rc=2（已实测触发）。
- 代码零改动 ⇒ `./tools/run_all.sh > /tmp/gate_313.txt 2>&1`（2026-09-21T20:32:36Z→
  20:35:37Z）直读退出码 rc=1 = 既有判据（`run_all.sh:134` 要 `py_fail==0`）。三项
  official **194/194** · python_style **285 passed, 2 failed, 4 known-fail, 0 xpass** ·
  语料 **39/39 = 100%** ⇒ 与批次 310/311/312 逐项相同，零基线位移。

### OPEN
- 任务 #27：`aot` 阶段跨调用漂 20.7%，性能门禁目前**只能判 ir 口径**；解法首推
  基线二进制/候选二进制同轮 A/B 交错（把漂移抵消给两边）。
- 任务 #26：`--emit-llvm` 退出路径空指针。
- 任务 #15：基线只能躺在 `/tmp`（仓内 `.gitignore` 的 `run_*` 会吞掉这类文件），
  机器负载一变就失效 ⇒ 与本项同因，先修 #15 才能把 perf/dc 基线入库。
  （**批次 314 已修**：dc 基线入库 `tools/baselines/`；perf 基线仍留 `/tmp`，原因改为本机路径依赖，见批次 314）

## 批次 314（任务 #15 —— 第二个幽灵门禁：CI 跑的 `tools/run_all.sh` 从来没被跟踪过）

### 为什么这比"少一个文件"严重
批次 313 查出 `benchmarks.yml` 引用不存在的 bench（幽灵门禁）。本批是同一缺陷类的**第二例，
而且更隐蔽**：文件**存在、能跑、每天本地都在跑**，只是 git 里没有它。
`.github/workflows/ci.yml:106` 在 `baselines` job 里执行 `./tools/run_all.sh`，但
`.gitignore:124` 是 `run_*`（CRLF 行），它同时匹配仓库根和任意子目录 ⇒
`git add tools/run_all.sh` 会被静默忽略，`checkout@v4` 出来的工作树里根本没有这个文件。
⇒ **三套基线（官方 194 / python_style / 语料 39）在 CI 上从未真正跑过**，
那个 job 只能以"找不到文件"失败。`git ls-files --error-unmatch tools/run_all.sh` →
"路径规格未匹配任何 git 已知文件"（本批实测），而它调用的
`tools/build_runtime.sh` / `tests/python_style/run.sh` / `tools/corpus_baseline.py`
（run_all.sh:43/:76/:98）**都已跟踪** ⇒ 唯一缺的就是入口本身。

### 修法：定向反包含，不放宽 `run_*`
`run_*` 在根目录挡的是 mypy/scratch 类临时脚本，删掉它会放开一大片。所以只在它后面插一行
`!tools/run_all.sh`（4 行：3 行说明 + 1 行规则），并逐条验证判据没有连带变化：

| 判据 | 期望 | 实测 |
|---|---|---|
| `git check-ignore -v tools/run_all.sh` | 命中 `!tools/run_all.sh` | `.gitignore:128:!tools/run_all.sh` ✅ |
| `run_scratch.sh` / `tools/run_scratch.sh` | 仍被 `run_*` 挡 | `:124` ✅ |
| `zeta_probe.o` | 仍被 `*.o:145` 挡 | ✅ |
| 与 HEAD 逐行 diff | **纯插入 4 行，零改动** | `@@ -124,0 +125,4 @@` 只有 `+` ✅ |

⚠️ 本仓 `.gitignore` 是 **CRLF 且含 7 个 NUL 字节**（`file` 报 data、`grep` 报 Binary）
⇒ 只能按字节读写编辑，插入行也补成 CRLF 与全文件一致；用文本模式重写会破坏内容。
NUL 计数改前改后都是 7。

### 顺带把 dc 基线入库（"只许减少"的判据要能持久）
批次 310 的 `tools/dc_audit.sh` 基线默认在 `/tmp/zeta_dc_baseline.txt`，注释里当时写着
"基线落 /tmp 是因为 #15 未修"。#15 一修，这个借口就没了 ⇒
- 新建 `tools/baselines/dc_default.txt`（**101 条**命中，默认特性口径）；
- `dc_audit.sh` 的 `BASE` 默认值改为仓内路径，注释同步（`--all-features` 仍指到
  `tools/baselines/dc_allfeat.txt`，换特性配置=换基线文件这条不变）。
- 理由：dc 命中清单只由**源码树**决定，不含机器路径 ⇒ 可入库、跨机可比。
  perf 基线**相反**，JSON 里写着 `corpus=~/source/quant/...` 与本机文件数 ⇒ 仍留 `/tmp`，
  `tools/perf_baseline.py` 文档串已改成这个新理由（原文错误地归因给 #15）。
- ⚠️ 入库基线必须回答"你是在脏工作树上采的吧"：是，工作树带着并发工作流**未提交**的
  `src/blockchain/**` 删除。核对结论是**不影响**——HEAD 的 `src/lib.rs:52` 是
  `#[cfg(feature = "blockchain")]` + `Cargo.toml` `default = []` ⇒ 该模块在 HEAD 的默认特性
  编译图里同样不存在；实测基线清单里 blockchain 命中 **0 条**（`grep -c blockchain` = 0）。
  ⇒ 这份 101 条在干净 HEAD 上成立，没把别人的未提交状态烤进仓内文件。

### 同型问题顺手记录（本批**不改**，避免替 CI 编造测试内容）
`ci.yml:61` 的 "Run known-good tests" 步骤引用 `tests/test_hello.z` / `test_values.z` /
`test_basic.z` —— 三个文件在 `tests/` 下**不存在也不曾被跟踪**（`ls tests/*.z` 无匹配）。
它外面套着 `if [ -f "$f" ]`（:62），所以这一步不是失败而是**静默空转**：日志里会打
"Self-host compilation verified"，看起来像跑过 JIT 冒烟测试，实际一个用例都没执行。
这比 313 那类"引用不存在 ⇒ 直接红"更糟，因为它产出的是**假绿**。
真实冒烟语料在 `tests/unit-tests/`（194 个 `.z`）与 `tests/smoke/`。

### 验证
- 代码零改动 ⇒ `./tools/run_all.sh > /tmp/gate_314.txt 2>&1` 直读退出码
  （2026-09-21T20:55:39Z→20:58:35Z）rc=1 = 既有判据（`run_all.sh:134` 要 `py_fail==0`）。
  三项 official **194/194** · python_style **285 passed, 2 failed, 4 known-fail, 0 xpass**
  · 语料 **39/39 = 100%** ⇒ 与批次 310~313 逐项相同，零基线位移。
- `./tools/dc_audit.sh --diff` rc=0："无新增命中（基线 tools/baselines/dc_default.txt：101 条）"
  ⇒ 入库基线可用，且缓存自检（`Checking zetac`）通过。
- `bash -n tools/dc_audit.sh` 通过；`tools/run_all.sh` 模式 755（CI 用 `./` 直接执行）。

### OPEN
- 幽灵门禁还剩第三例未处理：`benchmarks.yml` 的 5 处引用（批次 313 已记录，未修）。
- `ci.yml:61` 的假绿冒烟步骤（本批记录，未修）⇒ 修法是接到 `tests/unit-tests/` 真实语料，
  而不是新建那三个文件。
- 任务 #26 / #27 / #22 / #24 不变。

> **补录说明（批次 317 时自查）**：下面 315/316/317 三条是**先提交、后补日志**。
> 批次 314 之后我把"批次记录"写进了未跟踪的 `refactor.md`（G.5 各章落地明细），
> 忘了这份跟踪在库的 `roadmap.md` 才是批次流水账 ⇒ 本仓库的权威批次日志出现了
> 三批空洞。这属批次 314 刚记的"判据不在库里 = 假绿"的同类：**记录不在权威位置，
> 等于没记**。以后每批以 `roadmap.md` 为准（`refactor.md` 只放计划与审计表）。

## 批次 315（任务 #26 —— JIT 静默跳空指针：根因纠正 + 未绑符号就地填桩）

### 先纠正批次 313 的根因判断
313 记的是"`--emit-llvm` 退出路径 SIGSEGV"。实测根因**与 `--emit-llvm` 无关**：
CLI 无 `-o` 时一律走 `finalize_and_jit` 并在编译器进程内 `main.call()`，
`--emit-llvm` 只是顺带打印 IR。真正的洞是仓里没有 `build.rs` ⇒ `runtime/*.c`
从不在 zetac 镜像里（`nm` 反查 `zeta_env_set`/`println_str` 均 0 命中），而 JIT 只绑
`pylib/jit_mappings.txt`(131 条) + `vec_*` ⇒ Python 式模块级绑定所调用的
`zeta_env_get`/`zeta_nonlocal_decl` 等**无地址**，调用即跳到 0（rc=139、零诊断）。
**三套基线全走 `-o`（AOT），JIT 路径一行都没覆盖过** —— 这才是它能静默到今天的理由。

### 三个非显然的决定（每条都被实测推翻过首版）
1. **填桩而非拒编译**：`jit.rs::trap_unresolved_symbols` 给"被引用 + 非 `llvm.*` +
   绑不到"的声明填 `zeta_jit_missing_symbol(name)` → `error[E4016]` + `exit(1)`，
   必须在 `create_jit_execution_engine`（该处拷贝 module）之前。A/B 全跑 485 文件抓到
   "静态拒绝编译"版**误杀 3 个今天能跑通的文件**（`-O3` 后仍留在 IR 里的调用可能是
   动态死代码）⇒ 判"能不能跑"不能只看 IR 里有没有这条 call。
2. **只预测、不探测**：探针 `ee.get_function_address()` 自己在
   `MCJIT::finalizeLoadedModules → RuntimeDyldImpl::resolveRelocations` 里踩空
   （lldb：`EXC_BAD_ACCESS at 0xb0`）⇒ 不许问引擎要地址；"绑了什么"与"能否绑"收敛到
   同一张表（`JIT_MAPPINGS` 提为 `pub const`、`vec_*` 前缀表化 `JIT_VEC_BINDINGS`）。
3. **`dlopen(NULL)+dlsym` 而非 `RTLD_DEFAULT`**：后者是宏、无可移植拼写，且**错句柄
   不报错、只是什么都找不到** ⇒ 静默退化成"全都绑不到"的假阳工厂。换后本机同数。

### 实测（判据：`segv==0` 且 `ok>=163`）
- 新增 `tools/jit_sweep.sh` 并接进 `run_all.sh` 第 4 步（`--skip-jit` 可关；缺 coreutils
  `timeout` 时**喊话跳过**，不做批次 314 刚记的那种幽灵门禁）。
- 159 ok / 326 SIGSEGV → **163 ok / 322 精确报错 / 0 SIGSEGV**；`comm` 证 ok 集合
  **零回退**（4 个原 SIGSEGV 反转为跑通，崩点在装载期重定位）。E4016 入注册表。
- 四步门禁：194/194 · 285 passed/2 failed/4 known-fail/0 xpass · 39/39 · jit GREEN；
  `dc_audit --diff` 仍 101 条无新增。

### OPEN
- 任务 #28：`--emit-llvm` 不带 `-o` 时仍会**执行**被编译的程序（CLI 语义缺陷，本批未动）。
- 任务 #29：`ci.yml:61` 的"JIT 冒烟"引用 3 个不存在的 `tests/test_*.z` + `if [ -f ]`
  ⇒ 静默空转还打 verified（假绿）。应改接 `tools/jit_sweep.sh`，并在 Linux 上实测 ok 基线。

## 批次 316（refactor ⑨ 第一段 / G.5b —— `docs/ABI.md` §3 调用约定成文）

- §3.1–§3.5：规则 **C1–C12** + `coerce_call_args` **实参强转全表**（10 档，逐档标
  丢值/告警/判定）+ §3.5 四个**实测**探针 M1–M4。纯文档，编译器代码零改动。
- **了结 capybara COMPILER_BUGS #4**（"struct 返回垃圾字段"）：其"返回局部指针"
  根因假设被 M1/M4 **证伪** —— 写侧本就 heap 分配，现行实现是 heap 句柄（§3.1 C1）；
  M4 直接复现原 bug 用例，今天 `42 / 99` rc=0。残余风险改记为**字段数解析失败时的
  `("", 2)` 二字段兜底**（gen.rs:6336-6345、:6380）。
- 强转表给出"保留 / 白名单 / 删除候选"三档判定，**改动本身留给 G.5d**
  （#2/#6/#7/#8/#9/#10 纳入 `abi_note`）。
- M2 是唯一实测到"静默有损"的一条：`-> i64` 而首返 `2.5` ⇒ 得 `2`、零告警
  ⇒ **声明类型赢**，但裁决层未定位 ⇒ 附 B#4 立项。

## 批次 317（refactor ⑨ 第二段 / G.5c —— §4 名字修饰 + §5 类型布局 + §6 跨边界假设）

### 写了什么（`docs/ABI.md`，纯文档，编译器代码零改动）
- **§4 名字修饰 N1–N11**：先分三轴（源级名→LLVM 符号名 / LLVM→链接符号 / 注册名→C 实现），
  混谈是这类 bug 的共同形状。写侧四种拼写**没有一种可逆**（`<mod>__<member>`、
  `_inst_`、`_<arity>`、`host_str_*`）；读侧逐档数出 `get_or_declare_function` 的
  **18 档 / 313 行**瀑布（codegen.rs:2575-2887）。**N6 是本批最重要的结论**：
  瀑布最后一档不是报错，是就地声明 `i64(i64×实参数)` 的 extern（:2883-2886）
  ⇒ 名字没对上时编译器不会说话，ARCHITECTURE-REVIEW:104 的"静默错值链"上游在此。
  §4.5 把"新增一个运行期函数要改几处"做成表，结论：**"单一生成器"只完成了两条，
  生成物那份声明从未接管手写那份**。
- **§5 类型布局 L1–L9**：§1 表 #6/#7/#8 的几何常量升格为明文合同（vec = 数据指针 +
  `base-16` 的 `[cap|len]` 头 + 短 vec 紧块判据；map = 块首 + 24 字节桶 + `MAP_MOVED`
  forwarder；PyJson = 16 字节 `[tag,payload]`），每条给写侧/读侧双锚点。
- **§6 跨边界假设 6.1–6.6**：一份 registry 两个解析器；类型 token 只有 `f64`/`void`
  有法律效力（其余一律 i64）；校验**只核符号存在、从不核签名**；打包/拆包责任逐入口
  写死；"什么能被 JIT 绑定"有**三份互不校验**的名单；旋钮清单里有一个假旋钮。
- 附 A 恢复并标明覆盖范围；附 B 增 **#5** 编码不可逆、**#5′** 跨模块裸名回退
  （与任务 #9 同根，不另立）、**#6** `ZETA_STRICT_STUBS` 只读不用（py_additions.c:3314
  `(void)getenv(...)`，注释自述 "env is documentary"）、**#7**。

### 写合同的过程本身就是审计（三处计划前提被实测推翻）
① "瀑布八级"不成立 → 18 档，且真正的靶心是"消灭猜签名"而非"消灭瀑布"；
② 66 条 `.set` 已在**生成物** `runtime/aliases.inc.c`（旧锚 stub:228-294 过期），
   而幽灵符号 `py_asdict_unexpanded` 已闭链接期缺口（registry.txt:219 →
   响亮桩 tokio_runtime_stub.c:1278-1282）；
③ "四处手工同步"里声明那一份其实有**两份**：`runtime_decls_registry.rs`(294 处
   `add_function`) + `runtime_decls_core.rs`(61 处) 的入口函数从未被调用，现役仍是
   手写 255 处 —— 两条 `never used` 早就躺在 `tools/baselines/dc_default.txt:2-3`。

### 附 B#7 = 本批唯一的实测新缺陷（→ 任务 #32，优先级高于其余 G.5d 项）
`zeta_dyn_getitem`（py_additions.c:3427）的 vec 判据仍写 `cap >= 8`（:3432），
而批次 301 只把 `zt_dyn_vec_hdr` 那份改成了 `cap >= 1` + 紧块（:3474、:3488）
⇒ **同一判据的第二份副本**。复现：`def get1(d): return d[1]` + `xs: list = [1,2] + [3,4,5]`
⇒ 进程**挂死**，`sample` 1,719/1,719 栈样本全在 `map_get+256`。机制与 §1 行 #8 记的
Json/map 撞车**完全同形**：cap=5 走不进 vec 臂 ⇒ 落到 `map_get` 的开放寻址环，
`idx = hash & (cap-1)` 在 cap=5 上不是掩码 ⇒ `while(1)` 永不停止。
**我自己初稿判错过一次**：先写"今天不可观测为 bug"，依据是 3 例探针全对 ——
但那 3 例都走 `zeta_dynarray_new`（:2579 `if (cap < 8) cap = 8`），旧判据被生产者的
下限**掩盖**；换 `base[0] = n ? n : 1` 的生产者（`py_array_concat` :2392、
`py_sorted_key` :1717、`py_builtin_map` :2084、`py_builtin_filter` :2094、
`py_zip` :645、`zt_map_most_common` :294）即暴露。对照组：同一段用字面量
`[1,2,3]` 正常（`lit[1] = 2`）、不走动态路径也正常（`len = 5 / idx = 2`）⇒ 定位到
"动态下标 + 短 vec"这一交叉点。探针留在 `/tmp/abi5/`（`p4/p5/p6/p7` + `p6.sample`），
**故意不进 `tests/`**：那 3 例依赖"参数未标注"这一非合同行为，固化前先由任务 #32
定判据（G.5d 再决定固化到哪一段，不动 285 的计数口径）。

### 验证
- 编译器代码零改动 ⇒ 未复跑三套基线（纯文档口径同批次 316；313 那次跑了，因为它新增了
  工具脚本）。**本批另有一条不跑的理由，写清楚**：当前工作树被并发工作流改动
  （`src/blockchain/*` 删除、`src/lib.rs`、`Cargo.toml`、`runtime/py_additions.c`），
  此刻复跑会把别人的中间态算进我的基线位移，读数不可归因。
- 结构自检：`grep -n '^#' docs/ABI.md` 六章 + 两附录在位、编号连续；附 B 引用
  （#2/#4/#5/#5′/#6/#7）全部有对应条目；G.5 审计表 §4–§6 三行 ❌/⚠️→✅，
  完备性快照的"§4–§6 缺口判断仍然有效"一句已随之改写。

### OPEN
- 任务 #32（本批新增，见上）。
- 附 B#4（返回类型最终裁决层）、附 B#6（假旋钮）留在 G.5d。
- 轴 A 关联：`runtime_decls_*.rs` 的 355 处未接线声明 ⇒ 要么接线要么删，
  归 **G.5e 第一个动作**（已写进 refactor.md 的 G.5e 条目）。


## 批次 318（refactor ⑨ 第三段 / G.5d 前置 —— 附 B#4 裁决层定位并关闭，顺带推翻批次 316 的 M2）

### 本批做什么
批次 317 把"返回类型有两个候选来源、谁覆盖谁未定位"留在附 B#4。本批**只做定位**：
不动 codegen 一行，把结论落成合同（commit f79bad19，仅 `docs/ABI.md`）。
定位之所以是 G.5d 的**前置**而不是 G.5d 的一部分：修复动作（补告警、收敛真源）
要先知道"该改哪一处"，而批次 316 给出的答案本身是错的 —— 按错答案动手会改错地方。

### 裁决层 = 没有裁决层
- 被调方的 LLVM 签名**只**来自 `infer_fn_return_type`（codegen.rs:1375-1389）：
  扫 MIR **首条顶层** `Return` 的值的 `type_map` 条目。
- 声明的 `-> T` **只**到达调用方的 dest 槽（gen.rs 的函数声明进 `type_map`，
  调用点按它分配目的槽），从未参与被调方签名的构造。
- 二者之间**没有任何一致性检查**。grep 证据：`struct Mir` 无 `return_type` 字段
  （全仓 0 命中）⇒ 声明类型在 IR 层根本没有承载位，谈不上"覆盖"。
- `--dump-mir` 同程序对拍：调用方 `4: I64`（来自声明）vs 被调方 `1: F64`
  （来自首返 `2.5`）⇒ 两条来源在 MIR 里就并存，谁也不覆盖谁。

### 推翻批次 316 的 M2（本批 P0）
M2 原结论"**声明类型赢**"（`-> i64` + 首返 `2.5` ⇒ 得 `2`）**错**。实测
（`/tmp/abi8/a.z`，`a.ir`:1068 `define double @f` / :1071 `ret double` /
:1090-1092 caller 按 `i64` alloca+bitcast 读同槽 / :1099 printf）⇒ 打印
**`4612811918334230528`**，rc=0，零告警。即"赢"的是首返推断，声明侧只是**换了
一种读法**去读同一份位型。新增三行实测：
- **M5**：`-> i64`、首返 `2.5` ⇒ `4612811918334230528`（位型重解释，非截断）。
- **M6**：反向（`-> f64`、首返整数）⇒ 打印 `0.000000`（`b.ir`:1070/1092/1094/1102）。
- **M7**：`-> i64` 且 `return` 在 `if` 分支内、`If{dest: None}` ⇒
  `define i64 @f` + `ret_fptosi`（`m2b.ir`:1066/:1082-1083）⇒ 得 `r = 2`。
  这才是 M2 当初看到"截断"的真实现场 —— 与 M5 是**两条不同路径**，M2 把它们混成了
  一条，所以结论看起来对得上样例却对不上机制。

### 同时更正 M3 的机制描述
M3 说"`infer_fn_return_type` 的首条**含分支**返回"—— 机制说反了。首条顶层
`Return` 是 **MIR gen 合成的** `Return{val: If.dest}`（分支汇合类型），并非分支体内
那条；而 `If{dest: None}` 时体内 `return` 对推断**不可见**（:4508-4527 的注释自证：
"`infer_fn_return_type` only scans top-level returns, so nested ones are never
consulted"）。`m3.z` 实测：合成的 `Return{val: 7}` + `7: F64` ⇒ `define double @h`，
2.5/3.5 打印正确。⇒ 推断的可见性边界是"**顶层且已汇合**"，比原描述窄。

### 合同侧改动
- 新增 **R7**：返回值也是一次跨槽写入 ⇒ 定义侧 LLVM 签名型 == 调用侧 dest 槽型；
  今天正被违反且**零诊断**（`abi_note` 只服务实参侧）。
- **R1 限定适用范围** = 赋值路径，其"反向不存在"断言就地标注由 R7 覆盖（不改原文）。
- **C2** 重写为"两个各自独立的来源，谁也不覆盖谁（裁决层已定位，附 B#4 关闭）"。
- **C3** 更正：补偿点读的是**当前正在生成的函数自身**的签名
  （:4488-4497 `builder.get_insert_block().get_parent()`），不是被调方 ——
  原描述会让人误以为"按声明办事"。这一条同时给出 G.5d 的**修复靶心**。

### 验证
- 纯文档（`docs/ABI.md`，+83/-23）⇒ 未复跑三基线；不跑的第二条理由同批次 317：
  工作树被并发工作流改动（`runtime/*.c`、`src/lib.rs`、`Cargo.toml`、`src/blockchain/*`），
  读数不可归因。
- 结构自检：`grep -n '^#'` 六章 + 两附录在位；`sed -n '/^## 附 B/,$p' | grep -c '^[0-9]\.'`
  = 8；文中 `附 B#N` 引用（#2/#4/#5/#5′/#6/#7/#8）全部有对应条目；修掉了新 C2 标题里
  嵌套加粗导致的渲染错位。

### OPEN
- **任务 #33（本批新增，附 B#8）**：返回侧混型的两步修复 ——
  ①先补一条与 §3.2 对称的**返回侧告警**（不改行为，只出声）；
  ②再把签名来源收敛到单一真源。**注意 ②会改变 M7 类程序的行为**（嵌套 return
  从"看不见"变成"看得见"）⇒ 必须跑全部门禁后再落，且要与 285 计数口径对账。
- 探针仍留 `/tmp/abi5`、`/tmp/abi8`，**故意不进 `tests/`**（固化判据等 G.5d；
  动 `tests/` 会移 285 的口径）。

## 批次 319（refactor ⑨′ 第一段 / G.5d ① —— R7 返回侧诊断落地：把批次 318 写的合同变成会出声的检查）

### 选型理由
G.5d 的四个候选里只有任务 #33 ① 在脏工作树下可做：#32（`zeta_dyn_getitem` 短
vec 挂死）与附 B#6（假旋钮 `ZETA_STRICT_STUBS`）都要改 `runtime/py_additions.c`
（并发工作流持有），而 ① 只落在 `codegen.rs` 这个干净文件里。且它是 G.5d 后续
所有工作的**前置** —— 没有诊断，"改签名来源会不会移行为"这个问题无法度量。

### 落点与附 B#8 原计划不同（本批 P1）
原计划 ① 是"定义期比对声明 vs 推断"⇒ 要先把声明类型送进定义侧，而 `struct Mir`
没有 `return_type` 字段（批次 318 已 grep 定案）⇒ 得动 MIR gen。实际改在**调用点**
（`codegen.rs:3327` `MirStmt::Call` 分支首句）比"被调方 LLVM 返回型 vs dest 槽型"：
零 MIR 结构改动，且告警恰好落在真正会出错的那条调用上（一个定义可能对应多条
类型不同的调用，逐条报比在定义处报一次更有用）。实现 `:6960` `note_return_slot_mismatch`。

### 三条诚实的覆盖边界（写进 R7 正文，不是脚注）
1. **只对 Zeta 定义的函数生效** —— C 运行时符号的签名就是唯一真相，那边 float/int
   不一致不是本缺陷，故新增 `zeta_fn_names` 集合在预声明处登记（`:1481`）。
2. **只覆盖 float↔int** —— M7 那类"签名与槽一致、但嵌套 return 走 `fptosi`"不在
   范围内（§3.2 表 #6/#7 那一档），诊断不会假装看见它。
3. **泛型未覆盖** —— 单特化的 `specialized_fns` 没登记进 `zeta_fn_names`。

计数用独立的 `abi_ret_warn_count`（不复用 `abi_note` 的那个）：两条规则两个边界，
合并后看不出各自的触发率。

### 调用点名字的坑（本批真实踩坑）
第一次不落：调用侧 `func` 带 MIR 的 arity 后缀（`f` 被调成 `f_0`），而 `fns` 的
key 是预声明用的 `actual_name`（`f`）。改成候选拼写表（精确名 → `::` 基名 →
`base_N` → 剥尾部 `_数字`），**精确匹配优先**，避免把 `f_1` 这种合法函数名抢走。

### 实测曝光面（口径见下一节，别复用批次 318 的"零"）
- `tests/unit-tests` **0/194**、`tests/python_style` 顶层 12 文件 **0/12**
- 真实语料 **6/38 文件命中** —— 这才是非零的真实暴露面。已分诊两类：
  - (A) `ExecutionAdapter::nav_value/equity_value/available_cash`：`-> float` 但函数体
    只有 docstring ⇒ 合成 `i64` 零值被当 double 读 = `0.0`。数值上无害，**真问题是
    dispatch 绑到了抽象桩**，那是语义/绑定问题不是 ABI 问题，另立。
  - (B) `get_volume_ratio_4`、`MOM_1`、`__closure_0_extract_metrics_from_analyzer_cfb9e263a`：
    float 返回落进 int 槽 ⇒ **M5 型真垃圾**，与本诊断同源。
- `strict_abi` 本批**故意不把它升成 fatal**：新告警 + 语料 6 处命中，先把判断数据攒够。

### 行为保持 + 零基线位移
输出逐字节未变（`4612811918334230528` / `0.000000` / `r = 2` / `2.500000 3.500000`
四例对拍）。门禁在最终二进制上复跑（`/tmp/gate_319.txt`，rc=1 为既有口径）：
official **194/194** · python_style **285 passed / 2 failed / 4 known-fail / 0 xpass**
（failed 仍是 `t231_dict_set_cast_fromkeys`、`t233_listcomp_condition_capture`，与本批无关）
· corpus **39/39** ⇒ 三项读数与批次 318 完全一致，零位移。
未复跑：`perf_baseline --diff`（改动 = 每条 Call 语句 6 次哈希查表，远在 10% 阈下）、
`jit_sweep` —— 被问到时要按"未测"回答，不要当"已测通过"。

### 两个工具/文档缺陷（一个是本批自己撞出来的）
1. **`dc_audit.sh --diff` 按 `file:line:col` 比对 ⇒ 幽灵命中**。本批误报
   `codegen.rs:2482` 为新增，实为既有基线命中 `get_function_with_types`(`:2463`)
   被 +19 行漂移推过去的。改成按 **文件+消息** 比对（`dc_key()`）。双向验证：
   当前树 101 命中 / rc=0 / 无幽灵；篡改基线（`ZETA_DC_BASELINE` 去掉 codegen 若干行）
   仍能报出新增项 ⇒ 检测能力没被改坏。另确认 `dc_audit`/`perf_baseline` 不在
   `run_all.sh` 与 `.github/workflows` 的调用链里 ⇒ 改它不可能污染已记录的门禁读数。
2. **新增附 B#9：编译期诊断在门禁里读不到**。我一开始报"全量套件零命中"，随后发现
   日志里**一条 `warning:` 都没有** —— 因为压根没收。逐条核实口径：
   `tools/run_all.sh:67` 编译 official 时 `>/dev/null 2>&1`（stderr 全丢）；
   `tests/python_style/run.sh:95` 把正例编译输出写进逐文件 `.cc`（保留但不聚合，
   只在编译失败时 `tail -1`）、`:87` 负例同样丢弃；判定比的是**运行期 stdout**
   （`:127-133`）⇒ **编译期告警不影响判定结果**（这条对后续诊断工作是护栏，
   也是"改诊断安全"的依据）。测量本身已用重跑 `a.z` 复现来验证捕获方法有效，
   不是拿旧日志充数。立为任务 #34。

### 文档侧：锚点整体重映射（并把它变成立刻生效的规矩）
`codegen.rs` 插入 74 行 ⇒ ABI.md 里 19 个 `codegen.rs:` 锚点全部失效，按插入点
（+7/+9/+17/+19/+20/+74）机械重映射，29 个数字 token 改动、前缀计数仍是 19。
逐条抽查落点：1595/1599 alloca 类型规则、3293 `MirStmt::Assign`、6177 struct 字段
bitcast 注释、1702 `slot_or_sitofp`、3748/3756 比较、5667 `StringLit`、6810
`coerce_call_args`、1069 `zeta_call1`、2594 `get_or_declare_function`、6426 struct
type、1333 `strict_abi`。头部新增 ⚠️：**任何往该文件插代码的批次必须在同批重算锚点**
（任务 #35）—— 合同文档的引用一旦漂移就没人再抽查，等于把 §1–§6 全部退回"愿望清单"。
另把 R7/C2/M2/M5 里"零诊断/零告警"的措辞改成"当时成立 / 319 起出声"（批次 318 的
结论不能被本批的改动静默改写）。附 B#8 的 ① 就地标注已落地并说明落点差异。

### 中途回退一次（记录方法，避免下次靠运气）
第一版重映射脚本在设置文件上下文的分支里只吐出 `:NNNN`，**把 `codegen.rs` 前缀删了**
（`grep -c 'codegen.rs:'` 返回 0 才发现）。处置：坏文件留在 `/tmp/abi_319_broken.md`
备查 → `git checkout HEAD -- docs/ABI.md`（HEAD=批次 318，锚点完好）→ 修正脚本
（保留 `pre = m.group(1)+':'`）重跑 → 再在新行距上重贴批次 319 的正文，
这样新引的 `:3327`/`:6960` 不会被二次平移。

### 提交
`8ea470de`（`codegen.rs` +74 纯插入 / `docs/ABI.md` / `tools/dc_audit.sh`，
未 push：批次 315 起未获推送授权）。

## 批次 320（refactor ⑨′ 第二段 / G.5d ⑦⑧ —— 把"诊断"变成门禁读得到的东西：任务 #34 + #35）

### 选型理由
批次 319 落地的 R7 诊断有一个尴尬事实：**它进了门禁就没人能看见**。G.5d 剩下的候选里，
①（强转 6 档补诊断）和 ⑤（探针固化进 tests/）都要以"能测量告警触发面"为前提，
而 #32/假旋钮两项仍阻塞在并发持有的 `runtime/py_additions.c`。所以本批做**测量本身**：
先把诊断收进来，后面每一批诊断改动才有读数。这正是批次 319 自己踩的坑
（第一版"全量零命中"是在空集上测的，见附 B#9）。

### 改动一：门禁聚合编译期诊断（任务 #34 / 关闭附 B#9）
- `tools/run_all.sh`：official 段由 `>/dev/null 2>&1` 改成 **stderr 逐文件落盘**
  （`/tmp/zt_out/$n.diag`）→ 带告警的文件汇总进 `$OFFICIAL_DIAG`
  （默认 `/tmp/zeta_official_diag.txt`），并打印 `compile-diagnostics: official 13/194 …`。
- `tests/python_style/run.sh`：聚合**必须写在 run.sh 内部、EXIT trap 之前** ——
  `$OUTDIR` 是 `mktemp -d` 且 `run.sh:15` 有 `trap 'rm -rf'`，外部永远捞不回来。
  这里更正批次 319 附 B#9 的一条判断：当时写"per-file `.cc` 事后翻得到"是**错的**，
  真实情况是跑完即删，所以"不聚合"比"聚合不全"更严重。
- 两份计数同时进 `/tmp/zeta_baseline.json` 的 `compile_diagnostics` 字段 ⇒ CI 的
  artifact 里也拿得到（ci.yml 的 baselines job 本来就把这份 JSON 上传）。
- **口径**：只数 `warning:` 与 `PY-A:`，**排除 `clang: warning:`**（链接器抱怨 `-no-pie`，
  实测 194/194 全有；不排除，聚合结果 194/194 命中、真信号全被埋）。
- **不参与退出码**：判据仍是"编译成功数 / 运行期 stdout 比对"，护栏与批次 319 记的一致
  ⇒ 加诊断天然不动 194 与 285 的口径。
- 输出重复问题顺手处理：python_style 失败时 `tail -20 "$py_log"` 已经会带出聚合块，
  故 run_all 只在它通过时自己打印（两处都印是纯噪声）。

### 第一份读数（此前完全不可见）
- official：**13/194 文件、18 行** = 12×`[W1002]` + 2×`ABI coerce` + 1×其汇总
  + 2×`PY-A: imported module` + 1×`PY-A: unknown Python module`。
  `ABI return`（批次 319 的新诊断）**0 行** ⇒ 319 报的 0/194 这次是在**有捕获的前提下**复现的，
  不再是空集测量。
- python_style：**82 文件、191 行**。大头依次：31×"`errors` 的默认值 kind 装不进
  `dyn` 参数型 ⇒ 被强转"、14×`ABI coerce in call to zeta_env_set … fptosi`、
  33+12× pandas/numpy 双份 shim 提示、12×`ABI coerce:` 汇总。
  ⇒ 这份清单就是 G.5d ① 的输入：**§3.2 标"删除候选"的 6 档里，今天真在响的只有
  `ABI coerce` 那一族**，其余的候选诊断该按这个顺序补。

### 改动二：锚点自动核对（任务 #35）
`tools/check_abi_anchors.py` —— 解析 `docs/ABI.md` 里全部 `file:line`，判据三层：
①可定位（裸名按 `git ls-files` 后缀唯一匹配；多候选时用**车轮判据**：只有
"行数 ≥ 锚点行号"的候选算数，仍多解则报歧义并要求文档改写路径）②未越界
③**未漂移**（锚点所指那一行的文本与 `tools/baselines/abi_anchors.tsv` 逐字比）。
第三层才是批次 319 那类事故的直接解药：往 codegen.rs 插 74 行，锚点当场指向别的句子，
人肉抽查才有救——脚本一跑就报"漂移 N 处 + 新 + 消失"，rc=1。
- 支持同行续写形态（`resolver.rs:1887、:2639`）：裸 `:行号` 继承**本行**最近的路径，
  不跨行继承（否则 IR 转储里的 `:1068` 会被绑到上一段的真路径上）。95 个锚点里
  14 个来自续写形态，逐条人工核过绑定正确。
- 本批顺带把文档里唯一有歧义的锚点写死：`mod.rs:7` → `codegen/mod.rs:7`
  （38 个 `mod.rs` 候选里只有这一个能唯一判定）。
- 负向对照（改坏基线再看能不能抓到）：把 codegen.rs:1595 的快照文本篡改成一行假话
  + 塞一条不存在的 `src/main.rs:999` ⇒ 报 `[漂移] 1 / [消失] 1`、rc=1 ✓；
  正常树 ⇒ 95 个可解析 / 0 定位失败 / 漂移 0，rc=0。
- ⚠️ **未接进 CI，理由要说清**：95 个锚点里有 **23 处**落在并发工作流持有的
  `runtime/py_additions.c`（16）与 `runtime/tokio_runtime_stub.c`（7），硬门禁会把别人的
  批次卡成"文档问题"。接线的前提是先给脚本加"只核我持有的文件为硬判据、C 侧降级为报告项"
  的作用域开关 —— 已登记为**任务 #37**，不在本批偷偷上一条会误伤的判据。
- 已知边界（写进脚本头部，别当"已覆盖"）：它只回答"锚点还指着当初那行吗"，
  不回答"那行是否仍是该规则的实现点"；符号整段搬走且原位置留同样文本的情况测不出。

### 本批最大的收获是它撞出来的缺陷（附 B#10 / 任务 #36）
`official: 194/194` 里有 **12 个用例编译成功但程序被解析器就地截断**：
每个都带 `[W1002] … N line(s) … NOT parsed … DROPPED from the program`
——解析器遇到第一个不认识的顶层条目就停，后面整段不进 AST。
丢弃量合计 **1,805 行 / 这 12 个文件共 2,041 行 ⇒ 88% 的内容从未被编译**。
最差：`minimal_compiler` 800 丢 757、`benchmark_simd_vs_scalar` 364 丢 357、
`test_suite` 129 丢 127、`advanced_patterns_test` 100 丢 98、`selfhost` 179 丢 158。
未解析文本集中在 `fn test_at_patterns()`（`@` 模式）、`impl X for Y`、`match` 体几族。
⇒ ①"194/194"不能读成"194 个程序全部编译通过"；②凡以这批文件为"某语法已支持"证据的
结论一律无效。**这就是本批非做不可的证明**：诊断看不见时，连"基线在测什么"都会读错。

### 验证
- 门禁四项在最终状态复跑（`/tmp/gate_320b.txt`，**rc=1** 为既有口径：`run_all.sh` 的绿判据
  要求 `py_fail==0`）：official **194/194** · python_style **285 passed / 2 failed /
  4 known-fail / 0 xpass**（failed 仍是 `t231_dict_set_cast_fromkeys`、
  `t233_listcomp_condition_capture`）· corpus **解析通过 39/39 = 100%** ·
  `jit sweep: ok=163 trap=322 fail=0 timeout=0 segv=0 (total 485)`
  ⇒ 与批次 318/319 的读数逐位一致，**零基线位移**（本批不动编译器代码，改的是 harness 与工具）。
- `tools/dc_audit.sh --diff`：101 条，无新增命中 ✓。
- 新文件 `tools/check_abi_anchors.py`、`tools/baselines/abi_anchors.tsv` 已核
  **不被 .gitignore 吞掉**（批次 314 的 `run_*` 教训：入库前先 `git check-ignore -v`）。
- 结构自检：`bash -n tools/run_all.sh`、`bash -n tests/python_style/run.sh` 均通过；
  `run.sh` 的最后一句仍是 `[ "$fail" -eq 0 ]` ⇒ 退出码语义未动。

### OPEN
- 附 B#9 关闭、#34/#35 关闭（#35 的 CI 接线留作用域开关做完再接）。
- 新增 **任务 #36（附 B#10）**：12 个官方用例的解析器截断 —— 要么把这些构造接进 parser，
  要么按现状拆分并在 known-fail 段立住；**不许静默删用例**（那会把缺口藏得更深）。
- G.5d 剩余：① 强转 6 档补诊断（现在有了读数顺序）、② #33(b) 单一真源（⚠️ 改 M7 行为）、
  ③ #32 挂死 + ④ 假旋钮（均阻塞在并发文件）、⑤ M1–M7 探针固化 t4xx、⑥ registry 核签名。

## 批次 321（任务 #36 第一段 —— 解析器截断：修掉一族、给余下十族立判据）

### 选型理由
#36 是批次 320 刚登记的、后果最大且**无阻塞**的一项（不碰并发的两个 C 文件）。
它的难点不在改代码而在**归类**：W1002 只报到顶层条目首行，8/11 报 `fn` 头、
2 报 `impl` 头、1 报 `import` 行，病因却在条目内部。批次 320 因此留下一个错判
（"集中在 `@` 模式、`impl`、`match` 体这几族"），本批先立判据再动手。

### 改动一：`x @ 1..=10` 绑定模式不再截断整份文件（`78653481`）
- 根因：`parse_pattern` 的 `alt()` 里 `parse_struct_pattern` 排在
  `parse_bind_pattern` **之前**，而它对裸路径**故意返回 `Ok`**
  （`src/frontend/parser/pattern.rs:109` → `AstNode::Var`）⇒ `x @ 1..=10`
  只吃掉 `x`，`@ …` 留在输入里，arm 等不到 `=>`，整条 `fn` 连文件余部一起被丢。
- 修法：绑定模式先判（`parse_simple_pattern` 同步），并把这条不变量写进注释。
- 判据（先 repro 后动手）：`match x { 1..=10 => … }` 整型范围模式**本来就能解析**
  ⇒ "match 不支持"是假的；只有带 `@` 的 arm 触发 W1002。修后同一 repro 归零。
- 读数：截断文件 12→**11**、丢行 1,805→**1,749**；`test_advanced_patterns.z`
  全文 49 行第一次全部进 AST。四套基线不动。

### 改动二：JIT 绑上 `io.rs` 的 print 家族，ok 163→170（`2b1edd2e`）
这是改动一**顺带暴露**的缺口，不是本批新增的风险：`test_advanced_patterns.z`
第一次执行到 `println!("{}", s)`，MIR 对它发的是 `println_str`
（`src/middle/mir/gen.rs:7932`、`:7810`），而 `pylib/jit_mappings.txt` 里只有
`println_i64` 一个别名 ⇒ E4016 填桩 ⇒ jit ok **163→162**。
- 关键判断：**不能读成回归**。符号一直躺在 zetac 镜像里
  （`src/runtime/io.rs:89` 带 `#[unsafe(no_mangle)]`），只是从没进过表。
- 动作：补 7 条（`print_i64/print_bool/print_str/println/println_bool/println_str/flush`），
  `tools/gen_from_registry.py --emit-jit` 重生成 → 138 条。
  `println_f64/print_f64` **故意不补**：只有 C 侧实现，仓里没 build.rs（批次 315 根因）。
- 结果：jit ok **170**、trap 322→**315**、fail=0 timeout=0 segv=0（+7 个程序从"跑不通"变"跑得通"）。

### 改动三：`tools/parse_bisect.py` —— 把"解析器停在这"变成"就是这一行"
逐语句前缀回喂编译器（补 `}`*d 配平），**第一个**重新触发 W1002 的行即病因行。
两个不显然的约束（都被实测教过）：
1. 切点必须合法：在 `} else if` 之前切一刀，补上的 `}` 造出悬空 else 链 ⇒ 假病因
   （selfhost 第一次报 55，实为 57 之后）；规则=任意深度以 `;` 收尾，或深度 1 且
   以 `}` 收尾且下一行不以 `else` 开头。
2. **不许把成员 fn 直接抠出来单测**：保留 4 空格缩进的片段会被缩进敏感路径判死，
   当时得出"impl 单独能解析"的错结论。dedent 后同一成员立刻复现失败。

### 本批产出的分类表（11 文件 / 1,749 行，逐条最小用例正测）
写在 **docs/ABI.md 附 B#10**（单文件交付：结论进既有文档，不另起文件）。
按丢行量排序，前三：`match` 作表达式 757、`static mut` 局部 357、`r#"` 原始字符串 185。
仅 `selfhost`（158）未定位，登记为 OPEN。

### 本批最大的收获：**测量纪律**（比代码修复更值）
本批有两次"同一输入两次读数相反"（一次报 impl 能解析、一次报不能）。
先怀疑解析器非确定性，实测 `wi.z` 连跑 3 次 = 3 次一致 ⇒ **不是编译器抖，是我读数错**：
两处都出在 shell 循环里 `grep -c W1002 $z.err` 读到了上一轮的同名 `.err`。
⇒ 复测规则：任何"翻转结论"的读数必须**换文件名**重跑，或先 `rm -f *.err`。
这条也解释了为什么改动二差点被当成回归——第一次读数就是在一个不可信的循环里拿的。

### 验证
- 门禁 `/tmp/gate_321b.txt`：official **194/194** · python_style **285 passed / 2 failed /
  4 known-fail / 0 xpass**（口径未移）· corpus **39/39 = 100%** · jit **ok=170 trap=315
  fail=0 timeout=0 segv=0**（`GREEN: JIT 无静默崩溃，ok 未回退`）。
  退出码仍恒 1（`py_fail=2` 的存量判据），与本批无关。
- 诊断侧第一份对比读数：official 有告警的文件 13→**12**、告警行 18→**17**。
- `./tools/parse_bisect.py --all` → `/tmp/t321_class.txt`（分类表原始输出）。

### OPEN
- **selfhost 的 158 行**：bisect 指向 `selfhost.z:57`（`} else if ch.is_digit(10) {`
  分支内首条语句），但嵌套 else-if 的最小用例通过 ⇒ 病因未定，下批继续缩。
- 其余 10 族按丢行量排序待接进解析器（`match` 作表达式一族性价比最高：一次修 757 行）。
- 本批**未**测：`tools/perf_baseline.py --diff`（已连续三批没跑，登记在此以免被当成测过）。
- **锚点工具自己也被抓一次**（批次 321 实测，值得记）：本批给 `pattern.rs` 插了 7 行
  注释，`docs/ABI.md` 里新写的 `pattern.rs:40/:44/:109` 三处**当场**变成注释行。
  第一版 `--bless` 把这三条假锚点烤进了基线（工具的"文本相同"判据在 bless 时是自身
  的受害者）。发现方式：bless 后逐条读基线的文本列，看到 `// pattern below. …` 才暴露。
  ⇒ 规则：**bless 前必须逐条核对新锚点的文本列是不是代码行**；且注释里不要写行号
  （`pattern.rs` 的注释已从 "line 109" 改成按符号名指代，那行号是它自己插行漂掉的）。

## 批次 322（任务 #36 第二段 —— 字符串字面量 match 模式；附带的两处更正）

### 选型理由
批次 321 的表把 757 行那族标成"`match` 作表达式"，而 321 自己的探针已经测出
"`match` 作语句同样失败" ⇒ 那一行是这张表里唯一还带未证判据的行。本批就从它开始：
一次只换一个变量喂最小用例（321 定的测量纪律），把判据重做一遍。

### 探针矩阵（`/tmp/t322`，每条都换名重跑，避免 321 那类读旧 `.err` 的错）
| 用例 | 内容 | W1002 |
|---|---|---|
| q2 | 整型模式 + 单行逗号分隔 + 调用体 | 0 |
| q5 | 整型模式 + 换行分隔 + 字面量体 | 0 |
| q1/q3/q4/q6/q7/q9 | **模式位是字符串字面量**（体是调用/整字面量/串字面量、单行/多行皆试） | 1 |
| q8 | `if s == "+"`（字符串出现在**表达式**位） | 0 |
⇒ 判据收敛到一处：**字符串字面量作 match 模式**。`parse_lit`
（`src/frontend/parser/expr.rs:145`）从 FloatLit/hex/十进制往下走到底都不认引号，
模式位因此停在 `"+"` ⇒ 臂取不到 `=>` ⇒ 整条 `fn` 连文件余部被丢。
顺带否证了批次 321 表的写法：**757 行那族与 match 无关**，真因是
`s[i..]` 开区段下标（`s[i..j]` 可解析、`s[i..].starts_with(…)` 与 `s[i..].len()`
均失败）⇒ 新登记任务 #39，`docs/ABI.md` 附 B#10 该行已按此更正。

### 改动一：`tools/run_all.sh` 的 `jit.ok` 一直是阈值不是读数（门禁取证缺陷）
同一份 run 里出现两个矛盾读数：正文 `jit sweep: ok=170 …`，JSON `{"ok": 163}`。
按 321 立的规矩先怀疑测量而不是编译器：`sed -E 's/.*ok=([0-9]+).*/\1/'` 前缀贪婪，
而那一行末尾还带阈值 `…，最小 ok=163` ⇒ 抓到最后一个 `ok=`。同一行实测：
贪婪 163 / 锚定 170。后果是 `zeta_baseline.json` 的 `jit.ok` **恒等于 MIN_OK**，
跨批次比对这个字段结构上不可能发现回退（321 把 ok 从 163 推到 170，JSON 里看不见）。
改为 `s/^jit sweep: ok=([0-9]+) .*/`，复验 JSON 落 170。
CI 只上传该 JSON 不做断言（`ci.yml:103-113`）⇒ 污染的是留存证据，不是退出码。

### 改动二：字符串字面量作 match 模式（两层缺陷，第二层是 321 那课教出来的）
1. 解析：`src/frontend/parser/pattern.rs:60` 加 `parse_string_lit`（`parse_lit` 之后），
   `parse_simple_pattern`（`:252`）同步加 —— 只加前者则 `"a" | "b"` 的子模式仍不消费。
   `parse_string_lit`（`src/frontend/parser/expr.rs:293`）由私有改 `pub`。
2. 下型：**照抄整型臂会得到静默错值**。整型臂发 `MirStmt::Call{func:"=="}`，
   而 `==` 在 IR 里是 External `i64(i64,i64)`（`src/backend/codegen/codegen.rs:1157`）
   ⇒ 两个 `Str` 比的是**指针**，五条 expect 全落到 `_ =>`，编译 0 诊断。
   改发 `MirExpr::BinaryOp`（与 `op == "+"` 同形，后端 str 分支 `codegen.rs:5855`）；
   `AstNode::OrPattern` 分支同形修正（其尾部无条件 `Var(sub_pat_id)` 会烤掉
   内联 BinaryOp，故加 `matches!(… BinaryOp{..})` 守卫）。
   实测五条 expect 全对：plus / minus / ab-or-cd / ab-or-cd / other。
   回归用例 `tests/python_style/t303_match_string_pattern.z`（断言 stdout，不动 194 口径）。

### 本批的负结果（必须记，否则下一批又会照错表干活）
字符串模式族在 11 文件 / 1,749 行里**恢复 0 行**：官方 194 个文件此前没有一个用得上
这个构造（它根本不在表里），而 757 行那族的真因是 `s[i..]`。⇒ 能力面 +1、
一条断言臂选择的用例 +1，丢行总数不变（11 / 1,749 与批次 321 逐项相同）。

### 顺带登记（不在本批做）
任务 **#38**：py 风格 `def` + 臂内 `return "…"` 打印**地址**。最小 repro
`def f(x): match x { 1 => return "one", _ => return "other" }; print(f(1))`
→ `4368704480`。根因是 `AstNode::Match` 分支末尾无条件
`self.type_map.insert(id, Type::I64)`（本批时 `gen.rs:10984`）：带返回类型的 `fn`
被签名盖住所以看着对，py 模式无签名 ⇒ 按位重解读 ⇒ 与 附 B#8 / 任务 #33 同族。
t303 因此刻意写成 `fn` 形，并在文件头写明原因。

### 验证
`./tools/run_all.sh`（cwd=库根、不接管道）真退出码 1（`py_fail==2` 是常驻判据）：
official **194/194** · python_style **286**/2/4/0（285→286 即新增 t303）·
corpus 解析通过 **39/39** · jit sweep **ok=170 trap=316 fail=0 timeout=0 segv=0**
（total 485→**486** 是 t303 进池；ok 不变）。t303 在 JIT 侧计 trap 的原因已查明并写进
附 B#10：`str ==` 依赖 `host_str_eq`，定义在并发持有的 `runtime/py_additions.c:43`
（只引用不改），无 `build.rs` ⇒ JIT 永不绑定 ⇒ E4016 有指名诊断、非静默；
属 G.5e"JIT 绑定三张表归一"的输入项。锚点：100→**104**（5 条因插行重 cite、
4 条本批新增），bless 前逐条核对文本列均为代码行，复跑 rc=0。
未测：`tools/perf_baseline.py --diff`（已连续**四**批没跑，见下 OPEN）。

### OPEN
- 757 行真因 `s[i..]`（任务 #39）；selfhost 158 行仍未定位；余下各族按丢行量待接。
- 任务 #38（match 结果槽恒 I64）。
- 新增任务 #40：`.gitignore:114` 的裸 `*.z` 会吞掉**新建**的测试用例 —— 本批 t303
  落盘后 `git status` 完全不出现，靠 `git check-ignore -v` 才看得见，只能 `git add -f`
  （与已跟踪的 313 个 `tests/python_style/*.z` 一致）。后果：任何新用例若忘了 -f，
  本地读到 286、干净克隆读到 285，且无人报错 —— 与任务 #15 同族（判据/用例不在库里）。
  没动 `.gitignore`：加 `!tests/**/*.z` 会让一批历史忽略文件突然出现在别人的
  `git status` 里，属共享配置改动，留给用户定。
- 性能基线已连续 319/320/321/322 四批未跑。

## 批次 323（任务 #39 —— 最大那一族 `s[i..]`：恢复 527 行，并把"编译"和"链接"分成两个数）

### 选型理由
批次 322 把判据钉在**开区段下标**上（双边 `s[i..j]` 可解析、`s[i..]` 不可），这是
`附 B#10` 表里最大的一族（minimal_compiler 757 行 = 全部丢行的 43%）。
按 321 立的规矩干活：先最小对照用例锁死判据，再动解析器。

### 改动一：解析器认 `s[a..]` / `s[..b]` / `s[..]`
`src/frontend/parser/expr.rs:2265` 新增 `slice_sep`（`:` 与 `..` 二选一，并拒绝
`...` 的前两字符），`src/frontend/parser/expr.rs:2283` 的起始界分支改为
"先探分隔符：是分隔符 ⇒ 起始界缺省；否则解析起始界再要求分隔符"。
**为什么不能在 `parse_expr` 那一侧修**：优先级链
`parse_additive → parse_shift → parse_range → parse_unary` 里 range 比加法**更紧**，
`parse_expr` 遇到 `i+1..` 会一路吃掉 `..` 再回头找右界而失败 ⇒ 它结构上无法
"停在 `..` 之前"。双边形式因此从来就不经过这里（被通用下标的 range 分支吃掉），
落到切片分支的**必然**是缺一侧界的点号形式。
继承的代价一并写明：点号形式的起始界只吃 `parse_unary`/`parse_postfix` 级操作数，
所以 `s[a[i]+1..]` 仍不可解析（同一优先级所致，非本批遗漏，已写进用例头与附 B#10）。

### 改动二：解析通了才暴露的第二层 —— `Box::new` 把编译器崩掉
`minimal_compiler.z` 恢复后 `zetac` 以 rc=101 崩在
`src/backend/codegen/codegen.rs:6176`（`exprs[field_id]` 无条件索引）。
定位路径（每步都可复跑）：前缀截断 bisect → 逐 `fn` 隔离 → 体内逐行扫 →
v4/v5/v6/v9/v12~v15 最小矩阵；决定性一次是 MIR dump：
`Struct{fields:[("a",3)]}` 而 `exprs` 里没有 3 ⇒ `Box::new(expr)` 作 **struct 字段值**
才触发，作 `let`/实参只读到垃圾值（首批 7 个 repro 全编译通过，方向错过两轮）。
根因：`PathCall` 里 `Box::new` / `String::new` **一条语句、一个 `exprs` 条目都不发**，
返回的 id 是幽灵。修法按同族透明处理：`src/middle/mir/gen.rs:11647` 让 `Box::new(v)`
恒等下型（`Box<T>` 槽与其内值同为 64 位句柄，无需分配/释放），`:11651` 让
`String::new()` 下成空串字面量。**没有**顺手扩到别的 `T::method`（见下"顺带登记"）。

### 口径变更：`official` 一个数混了两件事，本批拆成 compile / compile+link
拆完两层，official 从 194/194 掉到 193/194 —— 但编译器没报错，是 **gcc 链接**失败：
恢复的 527 行引用了 12 个从未绑定的 std 方法
（`chars nth unwrap unwrap_or to_string push_str is_empty is_digit is_alphanumeric
is_whitespace iter parse`）。只要判据仍是"编译+链接全过"，解析恢复就被运行时完整度
**封顶**：每多恢复一行，只要引用一个还没绑定的方法，就报成"编译器不支持这段语法"。
⇒ `src/main.rs:535`+`:824` 加 `--no-link`（出 `.o` 即止）；
`tools/run_all.sh:81` 仅在整链失败时补跑一次做归因，日志打
`official: compile N/194, compile+link M/194` + 逐文件
`### <name> — 缺运行时绑定: <符号名>`；判据 `tools/run_all.sh:221` 改盯 `compile==total`。
compile+link 照样打印、缺绑定指名到符号 ⇒ 该缺口从"一个红计数"变成"一张登记表"，
真实编译失败仍然致命。不是删用例、也不是把门禁绿过去。

### 本批的负结果 + 一次测量陷阱（记下来省下一批）
1. `parse_bisect.py --all` 在解析器改完后仍报 45/757（"零恢复"），与直接跑新编译器的
   572/230 矛盾。按 321 的规矩先怀疑测量：`tools/run_all.sh:10` 与
   `tools/parse_bisect.py:158` 都写死 `target/release/zetac` ⇒ 没 `cargo build --release`
   就是在量旧编译器。重建后读数一致。**这条对门禁同样成立**，是卫生事实不是偶发。
2. minimal_compiler 内部还剩两处 W1002 未恢复：`s[pos+1..]`（加法作起始界，见改动一
   的继承代价）与 `a[...]` 一族（既有缺口，非本批引入）。

### 顺带登记
任务 **#41**：`T::static_method(...)` 的 `PathCall` 分支同族缺口 —— **已定义**类的静态方法
实测 `P::new(10)` → `0`（名字小写 `new` 时整条下型消失，幽灵 id）、
`P::build(10)` / `P::make(10)` → 打印**地址**（路由到 `zeta_platform_obj`）。
本批只补了 `Box::new`/`String::new` 两个透明构造，没扩这一族：扩了会把"静默丢条目"
变成"链接期缺符号"，改动半径与判据都要单独立项。

### 验证
`cargo build --release` 后 `./tools/run_all.sh`（cwd=库根、不接管道）真退出码 1
（`py_fail==2` 常驻）：**official compile 194/194、compile+link 193/194**（唯一 link-only
= minimal_compiler，12 个符号已登记）· python_style **287**/2/4/0（286→287 即新增 t304）·
corpus 解析通过 **39/39** · jit sweep **ok=170 trap=317 fail=0 timeout=0 segv=0**
（total 486→487 是 t304 进池；ok 不变，trap +1 的原因是 `str_slice` 在 JIT 侧无绑定，
与 t303 同族）· compile-diagnostics official **12/194 文件、17 行**（与批次 322 末次读数一致）。
丢行量：`./tools/truncation_inventory.sh` → 11 文件 **1,749→1,222**、237 文件里 11 命中不变。
回归用例 `tests/python_style/t304_open_ended_slice.z`（5 条 expect：`s[i..]`、`s[..5]`、
`s[..]`、`s[6..]`、`Box::new` 作 struct 字段值），需 `git add -f`（任务 #40）。
锚点：104→**114**（净 +10：新增 11 条真新锚点，另 5 条只是因插行重 cite；消失 6 =
那 5 条旧行号 + 1 条历史行号改写为散文、以免留下指向别处的假锚点）。
bless 前逐条核对了 16 条"新锚点"的文本列，确认每一行都是其正文声称的那条代码，
复跑 rc=0（`漂移 0 / 新 0 / 消失 0`）。
性能基线：**本批 §OPEN 记的五批欠账已在批次 324 补跑**（补跑读数见批次 324，非本批读数）。

### OPEN
- 任务 #36 余 11 文件 / **1,222 行**：benchmark_simd_vs_scalar 357、minimal_compiler 230、
  selfhost 158（仍未定位）、test_suite 127、quantum_basic 85、advanced_patterns 80、
  integration_all_features 58、bootstrap_validation 58、primezeta_usize 36、
  integration_test_program 19、test_const_expression 14。
- 下一族的顺序问题：self-host 语料的丢行现在**同时**受解析器和运行时完整度约束，
  恢复前先跑一次 `--no-link` 看它掉进哪一类，能省一批无效改动。
- 任务 #41（`T::static_method` + `codegen.rs:6176` 缺表达式条目时应当报错而不是崩）。
- 任务 #38（match 结果槽恒 I64）、#40（裸 `*.z` 吞新用例）、#37、#33。
- 性能基线已连续 319/320/321/322/323 五批未跑 → **批次 324 已补跑**（读数在那一批登记）。

## 批次 324（任务 #36 第四族 —— `r#"…"#` 原始字符串：185 行，外加一次"ok 虚高"更正）

### 先还的账：性能基线（连续五批未跑）
`python3 tools/perf_baseline.py --diff`：**ir total 41,655→39,721 ms（−4.6%，逐文件中位 −14.1%）、
aot 30,186→29,212 ms（−3.2%）**，未过 10% 回退阈值 ⇒ 批次 321~323 的解析器/MIR 改动没有
把编译拖慢。顺带量到两件事实：该 harness 的 36 个语料文件里 **30 个在 aot 阶段 link-fail**
（与 附 B#11 同一族缺绑定，不是性能问题），且 `ir:fail:rc1 × 36` 被 W3001 排除在 min 之外。

### 选型理由
按批次 323 立的顺序规则（动一族之前先判断它掉在"解析"还是"运行时"），选 **185 行的原始字符串族**：
`test_suite.z` 127 + `bootstrap_validation_test.z` 58，两处的形状完全同类 ——
**把一整段被测程序嵌进一个多行字面量**做自举测试，正是哈希形存在的理由；
而它只要求字面量能力，不牵连缺绑定的方法面。

### 改动
`src/frontend/parser/expr.rs:364` `parse_raw_string_lit`：旧版第一行是
`alt((tag("r\""), tag("r'")))`，遇 `r#` 直接失败 ⇒ 整条 `fn` 连文件余部被 W1002 丢掉。
改成先吃 `r`、再数 `#` 的个数：0 个 ⇒ 走旧形（`r"…"` / `r'…'`，行为一字未动）；
≥1 个 ⇒ 要求 `"`，结束符为"一个 `"` 后跟与开头等量的 `#`"，内部不做转义（原始串的语义）。

### 回归面（三条，都进用例）
① 旧形 `r"no\esc"` / `r'sq'` 不变；② 字面量内的 `"` 不得提前结束（`r#"x"y"#` → `x"y`）；
③ **以 `r` 开头的标识符不许被这个分支吃掉**（`rate = 5` 仍正常）—— 这是重写入口判定最容易踩的坑。
用例 `tests/python_style/t305_raw_string_hash.z`（7 条 expect，含 `m.len()==5` 断言多行内容
真的带换行进来了）。

### 读数里的一处"过去虚高"（必须这样记，否则下批会当成回退）
- official **compile 194/194**（硬判据不动）；`compile+link` **193→191**：新恢复的两个文件
  进了 link-only 名单（`test_suite` 缺 `_to_string`、`bootstrap_validation_test` 缺
  `_to_string`+`_unwrap_or_else`）。`_to_string` 在 3 个 link-only 文件里**全部出现** ⇒ 任务 #42
  的补齐顺序上它是第一优先。
- **jit ok 170→168**，逐文件可归因（`tools/jit_sweep.sh -v`）：掉的正是这两个文件，而它们
  过去"JIT 跑通"的原因是 `fn main` 压根在被截断的尾巴里（`test_suite.z:128`、
  `bootstrap_validation_test.z:59`，截断点分别是 `:4`、`:18`）⇒ 过去跑的是一段空程序。
  现在 main 真存在、真执行，撞 `to_string` 无 JIT 绑定 ⇒ 计 trap。
  即：**这两个数过去虚高，不是编译器回退**；trap 317→320 的另一个 +1 是新用例 t305
  （模块级 `let` ⇒ `zeta_module_decl`/`zeta_env_set` 无 JIT 绑定，与 t304 同族）。
- compile-diagnostics official 12→**10 文件 / 15 行**（两条 W1002 消失）·
  corpus 39/39 · python_style **287→288** · jit total 487→488。
- 丢行：**1,222→1,037**、截断文件 **11→9**（`tools/truncation_inventory.sh` 逐行核对）。
- 锚点 114→**115**：`expr.rs:2239/2257` 因本次插行漂到 `:2265/:2283`（重 cite，净条数不变），
  本批正文新增 `expr.rs:364` 一条；bless 前核对新锚点文本列，复跑 rc=0。

### OPEN
- 任务 #36 余 9 文件 / 1,037 行：benchmark_simd_vs_scalar 357（`static mut` 局部）、
  minimal_compiler 230、selfhost 158（仍未定位）、quantum_basic 85（函数体内 `use`）、
  advanced_patterns_test 80（`'a'..='z'` 字符范围模式）、integration_all_features 58
  （块体闭包实参）、primezeta_usize 36（带类型标注的循环变量）、
  integration_test_program 19（单段 `import`）、test_const_expression 14（常量表达式数组）。
- 下一族建议按"缺绑定半径"排序再选：`static mut` 那 357 行若引用 `get_time` 一类未绑定符号，
  恢复后只会进 link-only 名单；`'a'..='z'`（80 行）是纯解析 ⇒ 半径最小。
- 任务 #42（12+2 个 std 方法绑定）、#41、#38、#40、#37、#33。


## 批次 325（任务 #36 第五族 —— 范围模式：解析只是表象，底下是"这一族从未活着过"）

### 选型理由（并记一次判断失误）
批次 324 的 OPEN 写着"`'a'..='z'`（80 行）是纯解析 ⇒ 半径最小"。**这个判断错了**，
而且是按"丢行量+看起来只差一个引号"排的序，没先跑最小语义探针。实际三层缺陷叠加，
解析只是第一层（详见下）。记录在此是为了把规则补全：**动一族之前除了判断它掉在
"解析"还是"运行时"，还得问"这一族的语义有没有被断言过"** —— 范围模式能解析 22 行
却一行都没恢复过，说明它从未被任何 expect 覆盖。

### 改动一：解析层（`src/frontend/parser/pattern.rs`）
`parse_range_pattern` 的两端原本只吃 `parse_lit` ⇒ 字符字面量在模式位解析失败 ⇒
`advanced_patterns_test.z:32` 起整条 `fn` 连文件余部被 [W1002] 丢弃。新增
`parse_char_lit`（`:201`），接进两端（`:239`、`:242`）与 `parse_simple_pattern`
的 `alt()`（`:295`，理由同批次 322：只加前者的话 or 模式内的子模式仍不消费）。
**刻意的不对称**：单引号在本语言里也是普通字符串定界符（`s.split(',')` 依赖它），
故 `'x'` 只在**模式位**取码点整数 —— 槽位里 char 本来就是整数，表达式位仍是 1 字符串。
本仓没有 `CharLit` AST 节点，`parse_char_lit` 直接产 `AstNode::Lit(codepoint)`，
`\n`/`\r`/`\t`/`\0` 与 `\\`/`\'`/`\"` 五个转义形按整数处理。

### 改动二：下型层 A —— 范围臂**从来没命中过**（与字符字面量无关）
`match 5 { 1..=10 => 111, _ => 222 }` → **222**，编译 0 诊断。根因不在算子语义
（批次 322 那一课这次不适用：`>=`/`<=`/`&&` 在 `MirStmt::Call` 路径上是内联
`icmp sge/sle/and`，`codegen.rs:3763-3782`），而在 **dest 从没进 `exprs`**：
`gen_expr_safe`（`src/backend/codegen/codegen.rs:5606`）对缺失 id 静默回 `i64 0`
⇒ `&&` 永远看到 `0 && 0`。实测证据是同一条 MIR 里 `exprs`/`type_map` 两个表都
没有 `>=`/`<=` 的 dest 格（只有 `cond_id → Var(and_id)` 那一条）。
**范围模式的整型形与字符形共用这段下型 ⇒ 整族此前无条件失效**，也解释了
批次 321 为什么"修完 `x @ 1..=10` 的解析"一行都没恢复。
顺带：`inclusive` 字段被 `inclusive: _` 丢掉 ⇒ `..` 一直按 `..=` 运行。现已区分。
**回归面为零**：official 194 文件里没有一处用排他范围作模式（`..` 全是 for 循环
与切片，走 `MirExpr::Range` 那条独立下型路径）。

### 改动三：下型层 B —— `x @ 1..=10` 是 SIGTRAP 而不是错值
条件被多包了一层 `Var(inner_cond_id)`，而那一格从无写入 ⇒ LLVM 把"读未初始化
alloca"判为不可达并降成 `brk #0x1`：**main 的第一条指令就是 trap，一行输出都没有**
（`otool -tv` 见 `main: brk #0x1`，`nm` 见该 `.o` 只有 368 字节、零未定义符号）。
这是批次 324 那类"虚高/静默"读数的新亚种：**trap 而不是错值**，所以连"打印出来
不对"的机会都没有。修法是让 guard 的 dest 直接被上层读（`gen.rs:10886`）。

### 改动四：按轴 A 去重
上述两处原本各有 30 行逐字相同的下型 ⇒ 合并为 `lower_range_guard`
（`src/middle/mir/gen.rs:2989`），两处调用（`:10886`、`:10911`）。
分配顺序刻意保持与旧代码一致（`ge`/`le` 先于 `lower_expr(start/end)`），
以免 MIR 编号无谓漂移。

### 本批的负结果（必须记）
范围模式族只恢复 **18 行**（advanced_patterns_test 80→62，截断点仍在 `:40`）：同一个
未解析条目里还有下一族 —— **or 模式中的构造子模式** `Some(x @ 1) | Some(x @ 2)`
（最小用例单喂触发；`Some(x @ 4..=6)` 单独喂**可解析** ⇒ 缺口精确在"or 的分支是构造子"，
`AstNode::OrPattern` 的下型分支也只认 Lit/StringLit/`_`）。已登记，不在本批做。
相邻两点同样不在本批：`q @ _`（绑定里的通配）仍判不匹配；"读一个没有 `exprs`
条目的 id"这一整类静默失效属任务 #41 家族。

### 验证
`./tools/run_all.sh`（cwd=库根、不接管道）真退出码 1（`py_fail==2` 是常驻判据）：
official **compile 194/194**（硬判据）· compile+link 仍 191/194 ·
python_style **289**/2/4/0（288→289 即新增 t306）· corpus 解析通过 **39/39** ·
jit sweep **ok=169 trap=320 fail=0 timeout=0 segv=0**（total 488→489、ok 168→169，
**两处增量都且仅等于 t306**）· 编译诊断计数与批次 324 逐条相同（official 10 文件/15 行、
python_style 82 文件/191 行）。
新用例 `tests/python_style/t306_range_pattern_guard.z` 断言的是**臂选择**（11 条 expect：
字符范围三类+边界、整型 `..=` 与 `..` 的边界差、`q @ 1..=10 => q` 的绑定值），
不是"能编译" —— 修复前它解析通过、编译 0 诊断、臂全部落到 `_ =>`。
性能：`tools/perf_baseline.py --diff` 判定阶段 **ir total +0.6%**（阈值 10%，逐文件中位
−1.1%；aot 仅参考 +12.5%，含链接跨调用漂移，见任务 #27）。
锚点：bless 后复跑 `check_abi_anchors.py` rc=0（本批 gen.rs 插删导致 18 条整体漂移，
逐条核对文本列仍是同一代码行）。

### OPEN
- 任务 #36 余 **9 文件 / 1,019 行**：benchmark_simd_vs_scalar 357（`static mut` 局部）、
  minimal_compiler 230、selfhost 158（仍未定位）、quantum_basic 85（函数体内 `use`）、
  **advanced_patterns_test 62（or 模式中的构造子模式，本批新判据）**、
  integration_all_features 58（块体闭包实参）、primezeta_usize 36（带类型标注的循环变量）、
  integration_test_program 19（单段 `import`）、test_const_expression 14（常量表达式数组）。
- 下一族选择按本批改过的规则走：先跑最小语义探针确认"解析之外还有没有下型/运行时的坑"，
  再按缺绑定半径排序。`static mut`（357）与 `use`-in-fn（85）都是"解析一步 + 后续未知"，
  单看丢行量不足以定优先级。
- 任务 #42（12+2 个 std 方法绑定，`_to_string` 第一优先）、#41、#38、#40、#37、#33。

## 批次 326（refactor 立即档 ⑩ / G.3 起步 —— 第一次能**量出**"还有多少语义是错的"）

### 选型理由
立即档执行序 ①–⑬ 里唯一还挂着"立即起步"且从未动过的是 **⑩ G.3 差分 harness**
（`grep -rn 'diff_test|差分|oracle'` 全仓 0 处实现，只有 refactor.md/pyramid.md 里的
要求 ⇒ 不是重复劳动）。为什么插到任务 #36 的第六族之前：#36 已连做五族，每族都在收
"解析器吃代码"，而**语义正确性至今一个数字都没有**——`// expect:` 的期望值是人照着
自己对 zeta 的想象手写的，它只能防回归，结构上不可能发现"和 Python 不一致"。
G.3 换的是期望值的**来源**（参考实现现场产出），不是再加一套断言。

### 改动一：harness `tools/diff_test.py`（新，334 行）
- 一文件两份（`tests/diff/cases/*.dcase`，`#@@ python` / `#@@ zeta` 两段）：两份**必须
  相邻**，拆开存放迟早漂移。扩展名刻意不用 `.z`（`.gitignore:114` 的裸 `*.z`，
  任务 #40 记的那把坑），所以 `git add` 不需要 `-f`。
- 期望值 = `python3` 跑 python 段的 stdout，逐行比、只归一化尾部空行
  （与 `tests/python_style/run.sh:136` 同口径）。**手写期望一律不算数**。
- 五态判定：`match` / `mismatch`（真语义缺口）/ `compile` / `runtime` / `bad_case`
  （参考侧自己跑不出真值）。坏用例**排除出分母**并单独 rc=2——把"用例写坏了"和
  "实现错了"混进同一个数，等于两个都没测。
- 判据两条硬的：① 基线里记为 `match` 的用例不许变差（逐用例）；② `match` 绝对数
  不许低于 `match_min`。比率只作展示——用绝对数才不会因为"新加了一条难用例"
  就把闸门抬高、也不会惩罚扩充覆盖面。**退出码 precedence：回归优先于坏用例**
  （曾经的 match 退化成 bad_case 同时命中两条，此时报 1；run_all.sh 里 rc=2 只喊话）。
- `compile` 判定指名缺的符号（`host_str_find` 这种），复用批次 323 立的口径：
  不指名就会让"还没绑定"冒充"编译器不支持"。

### 改动二：104 条 curated 语义片段（refactor.md 的 100 条是下限）
分布 truth 20 / str 22 / container 20 / numeric 22 / control 20。
**第一份读数：match 85 / judged 104 ⇒ 差分一致率 81.7%**；
分相 truth 15/20、str 19/22、container 20/20、numeric 11/22、control 20/20。
两句必要的诚实话：
1. container / control 满贯**不判**为"这两类实现好"，判为"这两类测得浅"——生成器、
   异常、嵌套函数、kwargs、字符串方法长尾一条都没碰。满分类别是覆盖度警报，
   这条写进了任务 #49 的判据里。
2. 19 条不一致按根因分四摊，逐摊立项（#45 比较结果类型、#46 除法族四样、
   #47 静默垃圾、#48 方言裁决）+ 1 条缺绑定进 #42。

### 改动三：接进门禁第 5 步（`tools/run_all.sh`）
`--skip-diff` 可关、JSON 多一个 `diff` 字段、rc=1 判红、rc=2 只喊话不判红
（参考侧差异不是编译器缺陷；Linux/mac 的环境差也不该判红编译器——但**静默忽略**
才是批次 314 记的幽灵门禁，所以喊话必须出声）。代价实测 +约 35s（104 × 编译 0.08s
+ 两侧各一次运行），比 jit_sweep 那一步便宜。

### 负对照两条（都实测过，不然"闸门"只是形容词）
- **A**：把 `container_sum` 的 zeta 段改成 `sum([1, 2, 4])` ⇒ `rc=1`，
  打出 `container_sum（match → mismatch: 首个差异行 #1: 期望 '6' 实得 '7'）`
  **并且** `match=84 低于基线 match_min=85` 两条都响。
  **第一次做错了**：两侧同源文本，一次 `str.replace` 把期望和实现同时改了，结果仍
  match ⇒ 只改 `#@@ zeta` 段才成立。这条不是笔误，是"一文件两份"的真实失效模式
  （全局替换会连期望一起改），记录在此作为该格式的已知代价。
- **B**：把 `container_min` 的 python 段写成未定义名 ⇒ `bad_case` 出声、比率按
  judged=103 重算，且同一用例同时算回归（ precedence 见上）。复原后 `rc=0`。
- harness 也**自己响过一次**：加缺符号登记时忘了定义 `UNDEF_SYM_RE`，
  该用例被记成 `bad_case: harness 异常: NameError...` 而不是被静默跳过——
  这正是"harness 自身的洞必须响"那条设计在起作用（`rc=2`，不污染缺口清单）。

### 本批最脏的两条读数（都带"静默"二字）
- `print(3 * "ab")` → 打 **13093038960**，另一次跑是 13007317872 ⇒ 打的是**堆指针**。
  `"ab" * 3` 则正常 ⇒ `int * str` 没有对称下型，落到 `println_i64` 上，零诊断。
- `print("n=%d" % 42)` → 一次 8、一次 0 ⇒ **同一程序两次读数不同**，
  这是读到未初始化槽的硬证据（若只是"格式化没实现"应当稳定打某个值）。
  与任务 #41/#33/附 B 的"按位重解读"同根，进 #47。
- `print(7 // 2)` → 整条 print 被 [W1002] 吞掉（`//` 在本语言是行注释，
  Python 的整除算子与之词法冲突）。这是任务 #36 那一族在**差分库**里的第一例。

### 验证
- `./tools/run_all.sh`（前台，五步全跑）**rc=1**（既有 `py_fail==2` 判据，非本批引入）：
  `official: compile 194/194`、`compile+link 191/194`（link-only 明细三条同批次 325）、
  `python_style: 289 passed, 2 failed, 4 known-fail, 0 xpass`、`corpus 39/39`、
  `jit sweep: ok=169 trap=320 fail=0 timeout=0 segv=0（total 489）`、
  编译诊断 official 10 文件/15 行 + python_style 82 文件/191 行——
  **前四步与批次 325 逐数字相同**（本批零 Rust 改动，这些数不该动；动了才说明
  改门禁改出了副作用）。`diff` 字段：`{"match": 85, "judged": 104, "rate_pct": 81.7,
  "bad_case": 0, "skipped": 0}`。
- 基线入库 `tools/baselines/diff_consistency.json`（104 条逐用例判定 + `match_min=85`）。
- 性能**未跑，并写明理由**：本批不动 `src/`，MIR 与 IR 无位移可测 ⇒ 无事可测（不是漏跑）。
  若后续批次修 #45–#47 中任一条，须按批次 325 口径补 perf `--diff` 与锚点核对。
- 锚点**确实漂了 3 条**，漂因是本批改了 `tools/run_all.sh`（净 +37/−1：`--skip-diff`
  两处 +2 行，第 5 步与其 JSON 字段 +29 行）：`:81→:83`、`:176→:178`、`:221→:252`
  （合同正文里对这三行的引用逐条重引，每条都先用 `sed -n 'Np'` 核对文本与基线快照
  逐字相同，再 `--bless`）。
  这条本身就是任务 #37 想要的东西：**锚点作用域不止 `src/`**，门禁脚本同样是合同引用点，
  而它比 `src/` 更容易被"只是加个步骤"的改动带走。
- 一条测量侧事实（不是仓库事实，但会骗人）：同一命令丢后台跑**出现过只到第 1 步就
  以 rc=0 结束**的截断（该 rc 与脚本控制流不可能自洽：第 2 步起必有出声行），
  前台复跑五步齐全 ⇒ 门禁读数一律以前台运行为准，后台只用于等待通知。

### OPEN
- 新立 #45（比较结果该是 Bool 却打 1/0，5 条）、#46（除法族四样：`/` 非真除法、
  `//` 撞行注释、`%` 非 floor 语义、幂优先于一元负）、#47（int*str 打指针、
  `str % int` 非确定值）、#48（浮点 `%.6f` 与任意精度整数**需要一次方言裁决**——
  修法落在 `runtime/tokio_runtime_stub.c:36`，该文件并发工作流持有、本侧只读，
  所以这是一个决策而不是一个 patch）、#49（G.3 进阶：随机程序生成器）。
- ⑩ 从 ⬜ 变 🟡：起步档（curated 库 + 一致率 + 门禁）已落地，"一致率进 CI 读数
  序列"与随机生成档未完。G.3 未闭合 ⇒ refactor.md §10 轴 G 判据里的
  "差分一致率 ≥N%" 仍只有一行历史读数，不构成趋势。
- 任务 #36 余 9 文件 / 1,019 行的选族规则不变（批次 325 改过的：先跑最小语义探针，
  还得问"这一族的语义有没有被断言过"）。本批的差分库对这条有直接帮助：
  `//` 那一例说明差分库能替 #36 预先回答"解析通了以后语义对不对"。


---

## 批次 327（任务 #45 闭包 —— 比较结果的 `1/0` 不在下型层，在**常量折叠**层）

### 先记一次判断失误（这条比修复本身值钱）
批次 326 立 #45 时我把根因写成"比较运算产物的 type_map 是 I64 而不是 Bool ⇒ print 选到
`println_i64`"。**错的**，而且错法是这批反复出现的那一种：从"差分读数 + print 分发的代码形状"
倒推，没有先看 MIR。实际探针 `/tmp/g3/cmp1.z`：`print(a == b)`（变量）、`c = (3 == 4); print(c)`、
`e = 1 != 2; print(e)` **早就打 True/False** —— `src/middle/mir/gen.rs:4129`（批次 291）
`_ if is_cmp && !matches!(op.as_str(), "||" | "&&") => Type::Bool` 一直在正常工作。
补一条规则：**差分库回答"哪里不一致"，不回答"哪里错"**；定位仍须 `--dump-mir` 看产物形状。

### 一条 MIR 就够的证据
`print(3 == 4)`（`--dump-mir`）：`stmts: VoidCall{ "println_i64", [2] }`、`exprs: 2: IntLit(0)`、
`type_map: 2: I64` —— **整条 MIR 里没有 BinaryOp**。⇒ 常量折叠在 MIR 之前就把比较吃成了整数
字面量，`gen.rs` 的下型与 print 分发根本没被走到。同一份 MIR 里 `print(a == b)` 则有
`type_map: 17: Bool`（走 print_bool）。两条路的分岔点不在这个文件里。

### 改动：`src/middle/ctfe/value.rs`（+39 / −14）
`ConstValue::binary_op` 的 Int/Int、UInt/UInt 两臂把算术器结果**无条件** `.map(ConstValue::Int)`，
而算术器 `binary_op_int` 里兼任了 `== != < <= > >=` 六臂、返回 `(left == right) as i64`
（原 :284-289）—— 折叠层把比较**定型**成了整数。
现在：新增 `compare_int`（:248）/`compare_uint`（:261），只在比较算子上返回 `Option<bool>`；
两臂先问比较器（:179、:185）⇒ 产物是 `ConstValue::Bool`；算术器里那六臂删掉
（一份语义只留一个实现点，避免"改了一处另一处还折成 Int"）。
**下游一行没改**：`transform_expr` 早就写着 `Ok(ConstValue::Bool(result)) => AstNode::Bool(result)`
（`src/middle/ctfe/evaluator.rs:262、:287`），`AstNode::Bool` 下型为 Bool，print 分发
`gen.rs:7939` 命中 `print_bool` —— 折叠层一直有能力产出布尔，只是没人产。

### 副作用面：这次改的是"什么被折、什么不折"，不只是值
- `(Bool, Int)` 混合算术在 CTFE 里不再是 Int/Int ⇒ 折叠失败，`transform_expr` 的 `_ =>` 分支
  **保留 BinaryOp 走动态路径**（`_ =>` 也吞掉 Err，所以这不是硬错误、编译器不会因此报错）。
  实测 `(3 == 4) + 1` → 1、`(2 < 5) * 10` → 10、`not (1 == 2)` → True、
  `(1 == 1) and (2 == 2)` → True，与 CPython 逐字相同 ⇒ 新用例 `truth_fold_arith`、
  `truth_fold_logic` 专门锁这条"折叠退出后落到动态路径"的新通路。
- 链式比较 `1 < 2 < 3`：内层折成 `Bool(true)`，外层 `(Bool, Lit)` 不再折叠 ⇒ 动态路径打 True。
  此前是"折两次得 Int(1)"，值也恰好对（Python `True < 3` 为 True）—— 那是运气，现在走的是
  与 Python 同构的那条。
- 布尔当下标 `a[1 == 1]`：折叠不再产出 Int ⇒ 动态路径，实测 8 / 7 与 CPython 同（`/tmp/g3/edge.z`）。
- `ConstValue::as_int()` 对 Bool 返回 None，消费者三处（求下标 :669、:1109、重复计数 :696 与
  切分 :1228/:1232）此前能吃到折叠后的 0/1，现在只能让折叠退出。**没为这条写用例**（构造不出
  真实语料里出现的形态），记在此以免被当成已覆盖。

### 验证
- 门禁五步（前台）：`official compile 194/194`、`compile+link 191/194`（link-only 三条明细同批次
  325/326）、`python_style: 289 passed, 2 failed, 4 known-fail, 0 xpass`（失败仍是存量的
  `t231_dict_set_cast_fromkeys`、`t233_listcomp_condition_capture`，非本批引入）、
  `corpus 39/39`、`jit sweep ok=169 segv=0（total 489）`、
  **`diff test: match=92 judged=107 rate=86.0% bad_case=0`**（批次 326：85/104=81.7%）。
  编译诊断 official 10 文件/15 行、python_style 82 文件/191 行 —— 与批次 326 逐数字相同。
  ⇒ **前四步零位移，本批的位移只在 diff 一步**，这正是批次 326 建这一步想要的读法。
- 分类读数：truth 15/20 → **22/23**、str 19/22、container 20/20、numeric 11/22、control 20/20。
- 基线 `tools/baselines/diff_consistency.json` 复 bless：total 104→107、match_min 85→92、
  `bad_case=0`。三条新用例都是**加**进来的（含一条故意留红，见 OPEN），
  满足 #36/#39 那条"不许用静默删用例关闭任务"。
- 锚点：120 个可解析 / 漂移 0 / 新 0 / 消失 0（`value.rs` 不是 ABI.md 的引用点，本批也没动门禁脚本）。
- `cargo test --release --lib`：只有存量那条 `frontend::indent::tests::header_colon_stripped_with_trailing_comment`
  （任务 #23），`middle::ctfe` / `const_eval` 无失败。
- 性能 `python3 tools/perf_baseline.py --diff` 一共三次读数：改后 **ir total +6.2%**、复跑 **+9.1%**
  （aot +29.8% / +33.6%）。因为两次都在朝阈值走，做了**同负载手工对照**：把 `value.rs` 退回
  `HEAD` 重新构建再测 ⇒ **未改动的代码 ir total +11.8%、aot +39.4%**（当时 `load averages 4.6`，
  10 核）。结论两条：① 本批的位移不可归因（它是编译期多一个分支，IR 形状无变化，且对照读数
  比它更差）；② **更要紧**：未改动代码在当前负载下已经越过 `PERF_REGRESS_PCT=10` ⇒ 这个阈值低于
  本机噪声地板，门禁会假红（见 OPEN）。

### OPEN
- 新立 **#50**：`m = min(1 == 2, 3 == 4); print(m)` → zeta `0`，Python `False`（`max(True, False)` 同族）。
  这是**内建函数返回值下型**，与折叠是两条路。差分用例 `truth_builtin_min.dcase` 已入库并
  **留在 mismatch** —— 判据看的是 match 绝对数 + 逐用例回归，不是比率，所以登记一条已知红的
  用例不污染门禁，反而把它变成可回归的读数。同时记下 min/max 在 `gen.rs` 有四处分发
  （:6573、:6801、:6842、:8661），修之前先按轴 A 的口径确认哪几处是活的，别给死副本补类型。
- perf 阈值从"以后再说"升级成"已经会咬人"：要么按 `tools/perf_baseline.py --calibrate` 量一次
  本机地板再定 `PERF_REGRESS_PCT`，要么把判定改成"同负载对照"（本批用的就是那个手工做法，
  但它应该是脚本能力而不是每次人肉复现）。与 #27（aot 跨调用漂移 20%）同源，建议并到 #27。
- 差分库剩 15 条不一致按族：numeric 11（#46 除法族 / #48 方言裁决）、str 3（#47 + `host_str_find`
  缺绑定归 #42）、truth 1（#50）。numeric 是唯一还成族的，下一批优先它。
- 任务 #36 余 9 文件 / 1,019 行不变；⑩ 仍 🟡（G.3 起步档在，随机生成档与"一致率进 CI 读数序列"未完）。

## 批次 328（numeric 族拆半 —— `%` 要**两个**实现点，`**` 缺的是**一层优先级**而不是一个特例）

### 这一批只拿三条：`%` 的符号、`**` 的优先级、以及"修一半不算修"的判据
批次 327 之后 numeric 剩 11 红。按族拆：`%` 与 `**` 是**纯整数语义**，修了就对；`//` 挂在方言判据上
（→ #51）；`/` 真除、浮点 repr、任意精度整数是同一次裁决（→ #48）。本批不动后两族，理由在 OPEN 里
用实测数据说清楚，不是"挑软柿子"。

### 改动 ①：`%` 取**除数**的符号 ⇒ 折叠层与下沉层各一份
Python 的 `%` 结果符号跟除数（`-7 % 3 == 2`、`7 % -3 == -1`），Rust/LLVM 的 `srem` 跟被除数。
补一处不够，因为**字面量和变量走的是两条不同的路**：
- `src/middle/ctfe/value.rs:286` —— `binary_op_int` 的 `"%"` 臂。`print(-7 % 3)` 这类全字面量的表达式
  在这一层就被折掉，根本到不了 codegen。
- `src/backend/codegen/codegen.rs:2042` 新增 `build_floormod_int`（`srem` + "余数非零且与除数异号则加除数"
  的 select，无分支），两个整数分发点 `:3776`、`:6091` 都改走它。`:3776` 是带类型的算术分发，
  `:6091` 是另一条表达式路径 —— 只改一个点的话 `a % -b` 与 `x % y` 会一个对一个错。
这条结构与 `//` 的 `build_floordiv_int`（:2009）**同构**：floordiv 早就是"折叠层 + 下沉层"两份实现，
`%` 是第二个落进这个模式的算子。两份实现必须互指（`build_floormod_int` 的文档注释 :2041 已写明镜像
`ConstValue::binary_op_int` 的 `"%"` 臂），否则下一次只改一处。
实测：`-7 % 3`→2、`7 % -3`→-1、`-7 % -3`→-2、`0 % 5`→0，动态版 `a % -b`→-1、`(0-9) % 4`→3 ——
逐条与 CPython 相同。UInt 臂（value.rs:350）**不需要**改：`ConstValue::UInt` 恒非负，截断即地板。

### 改动 ②：`**` —— 先记一次判断失误
**第一版是个补丁，不是修复。** 我在 `parse_unary` 里特判"`-` 后面紧跟 `**`"，把整条乘除链抢来当指数，
探针 14/14 与 CPython 逐字相同 ⇒ 我以为这条修完了。换一批**不含一元负号**的形状，立刻现形（旧二进制）：

| 表达式 | CPython | 第一版 zeta | 为什么 |
|---|---|---|---|
| `2 * 3 ** 2` | 18 | **36** | `**` 在 `parse_multiplicative` 的算子表里 ⇒ 循环左结合地折成 `(2*3)**2` |
| `2 ** 3 % 5` | 3 | **8** | 同一张表 ⇒ 指数位调 `parse_multiplicative`，把 `% 5` 吞成 `2 ** (3%5)` |

规则沉淀：**补一个特例形状 ≠ 补一层优先级**。判据升级成 —— 每条语义修复都要再测一组
"同一层、换个入口形状"的探针，尤其要测**不含被特判的那个 token** 的形状。

**真正的结构**：给 `**` 独立一层。
- `src/frontend/parser/expr.rs:1267` 新增 `parse_power`：底数 = `parse_postfix`，指数 = `parse_unary`
  （⇒ 右结合 `2 ** 3 ** 2` = 512、负指数 `2 ** -3`、以及"一元负号弱于 `**`"三件事同时成立）。
- `:1245` —— `parse_unary` 的"无前缀算子"分支从 `parse_postfix` 改指 `parse_power`。
  `parse_postfix` 全仓只有这一个调用点（实测 grep），所以改这一处就覆盖了整条链。
- `:2744` —— `parse_multiplicative` 的算子表去掉 `**`，右结合特判分支删掉。
- **副作用是净删除**：第一版那 30 行特判整块移除，"一元负号 vs 幂"从两处变一处。
实测 `tests/diff/cases/numeric_pow_mixed_tier.dcase` + 23 行探针（`/tmp/g3/prec2.z`）**全绿**：
`a * 3 ** 2`、`2 ** 3 % 5`、`17 % 2 ** 3`、`-2 % (0 - 3) ** 2`、`f(2) ** 2`、`lst[1] ** 2`、
`len(d1) ** 3`、`-2 ** 2 ** 3`→-256、`(1+2) ** 2`→9。

共享 `**` 这个词元的两族**没被打坏**（这是本批唯一真正的外部性风险）：`{**a, **b}`
（`tests/python_style/t121_dict_spread.z`）与 `f(**d)`（`t120_kwargs_unpack.z`）在 python_style 仍绿，
因为字典展开在 `parse_dict_entry`（expr.rs:904）里先于表达式层就吃掉 `**`；新用例把
`d2 = {**d1, "b": 2}` 与幂混排形状一起锁进差分库，防的就是下一次有人再把 `**` 塞回算子表。

### 验证
- 门禁五步（前台，改完 `%` 与 `**` 之后各跑一次）：`official 191/194`（compile 194/194，link-only 三条
  明细同批次 325/326/327）、`python_style 289 passed / 2 failed / 4 known_fail / 0 xpass`（失败仍是存量的
  `t231_dict_set_cast_fromkeys`、`t233_listcomp_condition_capture`）、`corpus 39/39`、
  `jit ok=169 segv=0（total 489）`、**`diff match=99 judged=111 rate=89.2% bad_case=0`**。
  编译诊断 official 10 文件 / 15 行、python_style 82 文件 / 191 行 —— **前四步与批次 327 逐数字相同**，
  位移只在 diff 一步。
- 分类读数：numeric 11/22 → **18/26**，其余 truth 22/23、str 19/22、container 20/20、control 20/20 全不位移。
- 基线 `tools/baselines/diff_consistency.json` 复 bless：total 107→111、match_min 92→99。
  四条用例全是**加**进来的（`numeric_mod_dyn_neg`、`numeric_pow_dyn_prec`、`numeric_pow_paren_base`、
  `numeric_pow_mixed_tier`），满足 #36/#39"不许用静默删用例关闭任务"。
- 锚点：**这批第一次真撞到行号漂移** —— codegen.rs 插 35 行（:2037 起）又删 4 行（:6056 起）⇒ 表内
  14 个 codegen 锚点位移 +35/+31；expr.rs 净 +30 ⇒ 2 个位移。按"先重引再 bless"的口径做：
  逐个用**内容等值**（不是靠算术）定位新行号、改写 `docs/ABI.md` 的 19 处引用，再复采基线 ⇒
  最终 `120 个可解析 / 漂移 0 / 新 0 / 消失 0`、`锚点全部对上`。
- **但"漂移 0"不等于"ABI.md 的证据都还指得对"**：核对器只认 `path:行号` 形态，文档里另有一批
  **裸行号**引用（`（7357）`、`6937-6951`、"5411-5428"）它看不见。手工把 HEAD 版 ABI.md 的每个四位数
  与 HEAD 版 codegen.rs 逐行对内容，筛出 5 条确实是行号的（并排除两条假阳性：`42 / 99 / 4299 / 7 / 8 / 42`
  里的 4299 是测试值、`tokio_runtime_stub.c:2550,2560` 属于另一个文件），全部按内容重定位后改写：
  `5747→5782`、`7357→7388`、`6937-6951→6968-6982`、`6945-6950→6976-6981`、`5411-5428→5446-5463`。
  ⇒ 这是 #35 那件工具的**已知盲区**，已立任务跟踪（裸行号要么改成 `path:行号` 形态，要么让核对器
  连裸数字一起扫并按最近的 `path:` 归属）。
- `cargo test --release --lib -- --test-threads=1`：`135 passed / 1 failed`，失败仍是存量那条
  `frontend::indent::tests::header_colon_stripped_with_trailing_comment`（任务 #23）⇒ 无新增。
  注：并行跑仍会因 `Exclusion ranges overlap`（外部依赖（LLVM 侧），`src/` 内无此字串）SIGABRT，也是 #23 记的存量。
- 性能 `--diff`：本批 **ir total +11.7%**（> 10% ⇒ `[E3002]`）。这次没有停在"应该是噪声"：
  ① `--calibrate` 给同一份二进制连测两轮 ⇒ **ir total 级地板 1.2%**（所以同一时刻的抖动解释不了 11.7%）；
  ② **同负载对照**：把 `value.rs`/`codegen.rs`/`expr.rs` 三个文件退回 `HEAD` 重新构建再测 ⇒
  **未改动的代码 ir total +10.7%、aot +47.5%**（本批读数：ir +11.7%、aot +25.9%）—— 未改动版本**同样越阈值**，
  且 aot 比本批差 22 个百分点。
  ⇒ 结论比批次 327 收紧一档：**问题不是 `PERF_REGRESS_PCT=10` 太小**（同二进制地板只有 1.2%），
  而是"基线是别的时刻测的"。加大阈值没救，必须把判定改成**成对对照**（同负载重测基线，或像本批这样
  做一次 HEAD 重建对照），归 #27。

### OPEN
- **浮点 `%` 仍是截断**：`codegen.rs:3704` 用 `build_float_rem`（C `fmod`）。实测 `(0 - x) % y`
  → zeta `-1.000000` / CPython `2.0`，`x % (0 - y)` → `1.000000` / `-2.0`。符号修复与 `build_floormod_int`
  同构、可独立做，但**改了也读不出来** —— 期望值是 `2.0`，实得 `2.000000`，用例照样 mismatch。
  所以并进 #48（浮点 repr 裁决），不单开 bug 单；这条同时说明 #48 是 numeric 剩下的**唯一闸门**。
- `//`（#51）：判据现在挂在"文件里有没有缩进块"上（`indent.rs:219-258` 的 `normalize_blocks` 早退），
  flat python 文件永远拿不到地板除。blast radius 已量：**44 个 official 文件既没有分号又用 `//` 写行尾
  注释，扫描到 158 行** ⇒ 用"没有分号"当方言信号会把 `return i  // Should return 5` 改成算术，
  用 `{`/`}` 也不行（python 字典要花括号、`if x {  // comment` 行括号不闭合）。下一刀只能走"括号/字符串
  上下文扫描"或显式 `--dialect` 开关，且必须先有全量门禁复跑。
- `str_percent_fmt` 两次读数 **24 → 26**（同一份二进制、同一用例）⇒ #47 的"打的是非确定值（地址派生）"
  加强：不是稳定错值，是会随进程变的垃圾。
- 差分库剩 12 条不一致按族：numeric 8（floordiv 3 → #51；truediv / float_add / float_promote /
  mod_float / big_int 5 → #48）、str 3（#47 + `host_str_find` 缺绑定归 #42）、truth 1（#50）。
- 任务 #36 余 9 文件 / 1,019 行不变；⑩ 仍 🟡。
## 批次 329（任务 #49 第一段 —— 把差分库从"想到的面"换成"采样空间"，60 条采样一次性约出**三个新族**）

### 这一批只拿三条：一台随机程序生成器、三个新红族、以及"比退出码才看得见"的那一类

差分库到上一批为止 111 条全部是人 curated 的 —— `diff_test.py` 自己的"已知边界 3"就写着
"覆盖面 = 想到的面"。这批把 G.3 的进阶档补上：**受限语法随机程序生成**（refactor.md ⑩ 的第二半）。

### 改动：`tools/gen_diff_cases.py`（新增，347 行）

三条硬约束，缺一条这套东西只会产噪音（这是设计，不是保守）：
1. **只生成结果必为 int/bool/str/container 的表达式** —— `/`、`//`、浮点字面量整条语法都不生成。
   理由：浮点会撞上 `%.6f` 呈现层差异（#48）、`//` 撞方言判据（#51），随机程序在那边只会把
   同一个已知原因复制成 200 条 mismatch，采样预算全废。**已登记族不再重复发现**。
2. **绝对值可静态界定** —— 每个 `Int` 表达式携带 `bound`，构造会超限的直接退化回操作数
   （`add/sub/mul/pow` 四条都做这个检查，`MAX_ABS = 10**6`）。否则随机乘法撞 i64 溢出，
   那与任意精度整数同族（#48）。
3. **必然停机** —— `while` 只生成 `i = 0` / `while i < N` / 体内 `i = i + 1` 一种计数器形状。

判定层**不重写**：`importlib` 直接载 `tools/diff_test.py` 的 `parse_case/run_ref/run_zeta/BadCase`，
所以"生成侧的 verdict"和"门禁里的 verdict"是同一把尺。产物是合法 `.dcase`（`# @note:` 带 seed+序号，
可复现），默认写临时目录，人看完读数再 `--promote NAME…` 挑进库 —— **生成与入库分离**，
避免"跑一次脚本就把 60 条未 triage 的程序灌进基线"。

### 采样读数（seed=20260922，60 条）

`match=43 mismatch=11 runtime=6 bad_case=0 compile=0`。
**0 条 bad_case** 是这套语法成立的前提（参考侧 CPython 全部跑出真值）；**0 条 compile** 说明
随机程序不再暴露"解析不了"，暴露的全是语义差 —— 这正是上一批之后该看的层次。
17 条非一致全部归到**三个族**，且都不是已登记族：

| 族 | 采样命中 | 最小复现（已入库） | 实测 |
|---|---|---|---|
| 动态负下标 | 8 条（第 13/28/36/39/42/47/52/57） | `container_neg_dyn_index.dcase` | 期望 7/7/7/7/7/5，实得 7/3/3/3/7/8 |
| 同上·嵌套档 | 归族后加探 | `container_neg_index_chain_crash.dcase` | `m[0-1]`（元素是容器句柄）单层即 SIGSEGV（rc=139），字面量 `m[-1][-1]`/`m[-1]` 正常 |
| for 尾值 | 3 条（第 1/14/34） | `control_for_var_after_loop.dcase` | `for k in range(3)` 结束后 k=3（Python 2）；**同程序 while 档正确** |
| 尾表达式当退出码 | 6 条（第 6/27/30/31/32/44） | `control_tail_value_exit_code.dcase` | 输出逐字正确、rc=15；`len(l)` 尾 → rc=3，`l.append(12)` 尾 → rc=112 |

机制层面只有第一族定位到实现点：**判据挂在 AST 形状上而不是值上** ——
`gen.rs:12072-12114` 的负下标修正只匹配 `AstNode::UnaryOp{op:"-", expr: Lit(k)}`
（`Type::Array(..Literal)` 档 :12074、`Type::DynamicArray` 档 :12082），于是
- `l[-1]` 是 `UnaryOp` → 走 `vec_len - 1` ✓
- `l[0 - 1]` 在 MIR 里仍是 `BinaryOp`（**批次 329 原文写"被 CTFE 折成负字面量"是错的**，
  `--dump-mir` 实测：`Call{func:"-", args:[IntLit 0, IntLit 1]}` → `Call{func:"array_get", args:[.., 21]}`，
  正负都不折 —— 纠正与后续见批次 330）→ 不匹配形状判据 → 原始负下标交给运行时读越界 ✗
- `l[k]`（`k = 0 - 1`，`print(k)` 打出正确的 `-1`）是 `Var` → 同样不匹配 ✗

这与批次 328 那条"补一个特例形状 ≠ 补一层优先级"是同一类错的另一个方向：**按形状匹配，
值域就漏**；按值匹配才闭合。后两族本轮只到"最小复现 + 读数"，实现点未定位，不在 roadmap 里猜。

### 为什么"退出码"这一族值得单独一条用例

6 条采样全部判 `runtime` 而**输出逐字正确**：只看 stdout 的对照（官方套件、python_style 的
口径都是"退出码 0 才算过"，但差分侧若只比输出就会漏）会把这类当成绿。`control_tail_value_exit_code.dcase`
的意义是把"比退出码"这件事钉在差分库里。roadmap:9143 早就顺手记过一次（t209 rc=1 / t48 rc=5），
当时没立任务 ⇒ 一直没修；这次正式登记 #55，并把"记过但没登记"当作流程漏洞写进来。

### 入库与验证

- 新用例 12 条 = 红 4（上表）+ 绿锁 8（`gen009/016/019/022/043/046/056/058` 的生成程序原样 promote，
  按构造标签去重覆盖 `% ** 三元 while for append dict 索引 abs/inc/min-max/sum/布尔代数`）。
  43 条绿里没有全 promote：随机程序共享同一固定头，信息密度低于人工用例，采 8 条锁形状即可。
- 基线：`diff_consistency.json` 111 → **123 条，match 99 → 107，rate 87.0%，match_min=107**。
  红档进库不破门禁 —— 门禁是**每条不退化 + 绝对 match 地板**，不是比率地板，所以"明知是红也进库"
  仍然合法且必要（这 4 条就是本批的产出）。
- 分类读数：numeric 18/26（不变）、str 19/22 → **20/23**、truth 22/23（不变）、
  container 20/20 → **24/26**、control 20/20 → **23/25**。旧库这三类**全绿**，所以新库 16 条红里
  有 4 条是本批刚钉的族、12 条是上一批就登记在案的 #47/#48/#50/#51/#42。
- 五步门禁：official **194/194 编译、191/194 链接**、python_style **289 过 / 2 败（存量口径 `py_fail==2`）/ 4 KNOWN-FAIL / 0 XPASS**、
  语料 **39/39**、jit **169/489**、diff **107/123** —— 前四步与批次 328 逐项相同（本批**未碰 `src/`**，
  `git status` 可核），只有 diff 步因入库 12 条而变化。
- `check_abi_anchors.py`：120 锚点漂移 0 / 新 0 / 消失 0（未碰 `docs/ABI.md`，也未碰任何被锚定的源文件）。
- `cargo test --lib` 未重跑：本批改动全在 `tools/` 与 `tests/diff/cases/`，无 Rust 源变更。

### OPEN
- **生成器的语法边界就是它的盲区**，逐条写清楚免得被当"已覆盖"：固定头（只有 `inc()`、`l`、`s`、`d`
  四个符号）、无嵌套容器、无 dict 值参与算术、无用户函数互调、无浮点、无 `//`。
  下一批要扩就**按族扩**（一次只放一族，放 `//`/`/` 就会淹进 #48/#51 的已知红，等于白采样）。
- #53 优先：实现点已定位（`gen.rs:12072-12114` 按形状匹配），修法是把判据换成"值为负就修正"，
  并对动态下标加 runtime `idx < 0 ? len + idx : idx`；它同时管住错值档和嵌套崩溃档，是这批三个新族里
  唯一已经能动手的。
  **批次 330 修正**：前半句被实测否掉并回退（负下标在 MIR 里基本不是字面量，按值匹配抓不到东西），
  后半句（select）才是主路径 —— 三个候选修法与其阻塞点见批次 330。
- #55 影响面最大但实现点未定位：**任何以表达式收尾的脚本退出码都不是 0**。官方套件的口径是
  "退出码 0 才算过"，所以这类在 official 侧表现为失败而不是静默；未定位前不猜。
- #54 待定位：`while` 尾值正确、`for range` 尾值多 1 ⇒ 差异在 range  lowering 的归纳变量写回，
  与批次 328 那条"判据挂在形状上"不同，这次是**写入时机**问题，需要单独一批。
- numeric 剩 8 条红不变（floordiv 3 → #51；truediv/float_add/float_promote/mod_float/big_int 5 → #48）。
- 任务 #49 只闭前半：**"一致率进 CI 读数序列"未做** —— 现在 `diff` 读数只进 `run_all.sh` 的
  `/tmp/zeta_baseline.json`，没有跨批次序列；生成器也还只是人工采样工具，没进门禁。⑩ 仍 🟡。
- 任务 #36 余 9 文件 / 1,019 行不变。
## 批次 330（#53 的机制纠正 —— 负下标缺的不是"按值匹配"，是**运行时侧没人做归一化**）

### 先说结论：批次 329 那句"被 CTFE 折成负字面量"是推断，且是错的

上一批写 #53 时只看了 `gen.rs` 的匹配形状，没跑 `--dump-mir` 就替 CTFE 编了一步"折叠"。实测反驳：

```
l = [3, 5, 7] 的两种下标写法，MIR 尾部：
  print(l[-1])    → Call{func:"vec_len", args:[bid]} … Call{func:"array_get"}   ← 形状判据命中，修正生效
  print(l[0 - 1]) → Call{func:"-", args:[19, 20]} → Call{func:"array_get", args:[13, 21]}
                    （19=IntLit(0)、20=IntLit(1)）
  print(l[3 - 1]) → 同样保留 Call{func:"-"}                                       ← 正数也不折
```

也就是说 `0 - 1` **根本没被折叠**，它以运行时二元运算的身份进 `array_get`。所以"按形状匹配漏掉负字面量"
这个描述虽然方向对（形状判据确实窄），但**漏的类别不是字面量，而是任何非常量下标** —— 那正是
`gen.rs` 这一层按定义管不到的部分。

### 试过并回退：把判据从形状换成值

`gen.rs:12072-12114` 改成 `neg_index: Option<i64>`（同时接受 `Lit(k<0)` 与 `UnaryOp{-, Lit(k>0)}`，
并把 DynamicArray 档从 `vec_len - k` 改写成 `vec_len + v`）→ 编译通过、`l[-1]` 仍然 7，
但 `l[0 - 1]` / `l[k]` / `l[0 - 2]` **读数一字不变**（7/3/3/3/7/8 → 完全同值）。
原因如上：这条臂没有触发点。**已 `git checkout` 回退**，`src/` 与 HEAD 一致，
门禁 `diff 107/123 rate=87.0%` 复跑读数"无回归"，`array_get` 三处定义未动。

教训写死在这里：**改判据之前先 `--dump-mir` 看真实 IR 形状**。批次 329 那条机制句如果先跑 dump，
就不会把推断写成结论 —— 与锚点核对同一个道理：没有可抽查证据的句子不算证据。

### #53 的三个候选修法（含各自的阻塞点，按可闭合性排序）

| # | 修法 | 位置 | 覆盖 | 状态 |
|---|---|---|---|---|
| 1 | codegen 在下标调用点前插 `select(idx<0, idx+len, idx)` | `src/backend/codegen/codegen.rs`（`build_select` 惯例见 :2067，长度取 `vec_len`） | AOT + JIT 同一处 | **唯一能单点闭合的路**；与批次 328 的 `build_floormod_int`（:2042）同构 |
| 2 | 运行时归一化 `idx < 0 → idx + len` | AOT：`runtime/tokio_runtime_stub.c:372`；JIT：`src/runtime/array.rs:167`、`src/runtime/host.rs:766` | 三处都得改 | AOT 那一份在并发工作流手里（禁改）⇒ 只改 Rust 两份 = 只修 JIT，**AOT 依旧错**，正是批次 328 记下的"两个实现点"陷阱 |
| 3 | 让 list 下标走已有 `zeta_dyn_getitem` | `runtime/py_additions.c:3429` 已经做 `key < 0 ? key + len : key` | 一处 | 该文件同样禁改；且它内嵌 `cap >= 8` 旧判据副本 = 任务 #32 的短 vec 盲区，3 元素 list 会被判成"非 vec"落回 `map_get` ⇒ **除非 #32 先修，这条不成立** |

下一批按 #1 做：只在 codegen 一处，AOT/JIT 同覆盖，不碰并发持有的文件。

### 顺带闭合的一件事

批次 329 的红档 `container_neg_index_chain_crash`（嵌套 list 负下标 SIGSEGV）与本族同源 ——
负下标喂给 `((int64_t*)arr)[idx]` 读到头部之前的指针。批次 330 实测把它**从链式缩到单层**：
`print(m[0-1])`（`m = [[1,2],[3,4]]`）单独一行即 rc=139、无输出，而字面量档 `print(m[-1])` 正常
—— 崩溃不需要"第二次解引用"，先前那句"链式才炸"是未测的推断，已随用例一起改小。
所以 #1 落地应同时让这两条一起转绿（diff match 107 → 109 的预期），**不能只测其中一条就宣布修好**。

---

## 批次 331（#53 落地：下标判据从"AST 形状"换成"值域"，读写两侧一处闭合）

批次 330 的结论是"缺口不在折叠、在运行时没有归一化"，并选定候选 #1（codegen 侧 select）。
本批兑现它，并把**写侧**一起收了 —— 因为两侧原来是同一条按形状匹配的判据，只收一半就是
批次 328 已经记过的那类错（补一个特例形状 ≠ 闭合值域）。

### 改了什么（两个实现点，各一处）

| 点 | 位置 | 内容 |
|---|---|---|
| MIR 侧 | `src/middle/mir/gen.rs:3068` `emit_norm_index`、`:3086` `normalize_subscript_index` | 新 MIR 词 `norm_index(len, idx)`；读侧调用点 `:12238`、写侧调用点 `:1431` 共用同一个 helper |
| IR 侧 | `src/backend/codegen/codegen.rs:3608` | 把 `norm_index` 内联成 `select(idx<0, len+idx, idx)`，照 `:2074` floormod 的 `build_select` 惯用法 |

批次 329/330 引的那段按形状匹配的档，因批次 331 的 102 行插入已整体下移，现在位于
`gen.rs:12161-12203`（`let index: Box<AstNode> = match (..)`，起点 `:12160` 的
`negative_dyn_index`）—— 旧编号读起来像失效引用，这里给一次对账。上表行号是**当前工作树**的编号
（批次 332 在 `gen.rs:2449-2508` 净插 27 行，`:3068` 之后所有行号 = 本批原编号 +27；`codegen.rs` 同理 +9）。

为什么放 codegen 而不是运行时（批次 330 三候选里 #1 的理由，落地后复核成立）：
`MirExpr` 没有 Select/Ternary 变体，值级 select 只能在 IR 构造点做；放这里 AOT 与进程内 JIT
共用同一份实现，**不需要往 `pylib/jit_mappings.txt` 加符号**，也不碰并发工作流持有的两个 C 文件。

判据的形状：`len` 按容器分档取 —— `DynamicArray` 走运行时 `vec_len(base)`，定长数组把编译期
字面量长度当参数传进去；**静态可知的非负字面量下标不加任何指令**（`l[0]`、`l[2]` 这类热路径零成本）。

### 读数（AOT `-o`，与 CPython 对照）

| 档 | 期望 | 修复前 | 修复后 |
|---|---|---|---|
| `container_neg_dyn_index`（6 print） | 7/7/7/7/7/5 | 7/3/3/3/7/8 | 全对 |
| `container_neg_index_chain_crash` | `[3, 4]` ×2 | rc=139、无输出 | 全对 |
| **新钉写侧档** `container_neg_dyn_index_assign` | 9 / `[3, 5, 9]` / `[3, 42, 9]` | 7 / `[3, 5, 7, 0, 0, 0, 0, 0, 0]` | 全对 |
| 定长数组 `a[0-1]`（`[i64; 3]`） | 30 | 30（形状档已对） | 30，正向档 `a[1]`/`a[j]` 不变 |

写侧修复前那一条值得单独记：`l[0-1] = 9` 的值**没落进数据区**，容器的**长度反倒被改成 9**
（打回 `[3, 5, 7, 0, 0, 0, 0, 0, 0]`、`l[2]` 仍是 7）。读侧是"读到垃圾"，写侧是"静默改结构"，
所以它必须和读侧同批收，并且单独钉一条用例。

顺带复测（**没有为它们改判据**，实测已对，登记免得被当"未覆盖"）：
元组 `t[0-1]` → 3、字符串 `s[0-1]` → `c`、形参里的 `a[0-1]`（`def f(a)` + `f(l)`）→ 7。

### 验证

- 差分：**107 → 110 / 124**（新增 1 条写侧用例，container 27/27 全绿），基线 `--bless` 到
  `match_min=110`、88.7%。批次 330 写的预期是 107 → 109（两条一起转绿）；实际 109 是"只修读侧"
  的数，本批把写侧一起收了就多一条 —— 两条红档确实**同批**转绿，没有只测其中一条。
- 五步门禁（`./tools/run_all.sh`，rc=1 是存量 `py_fail==2` 判据）：official **194/194 编译、
  191/194 链接**、python_style **289 过 / 2 败 / 4 KNOWN-FAIL / 0 XPASS**（失败仍是存量的
  `t231_dict_set_cast_fromkeys`、`t233_listcomp_condition_capture`）、语料 **39/39**、
  jit **ok=169 / trap=320 / fail=0 / segv=0**（下限 163）、diff 110/124。前四步与批次 329 逐项相同。
- 性能（成对同负载，任务 #27 的口径）：一段下标热循环（100000 次 `m[n % 8]` + while 计数 + dict
  写入三档）用**新旧两个编译器**各编一份，二进制交替各跑 3 轮，**6 次全部 0.70s**（10ms 分辨率下
  无可测差），两份程序输出逐字相同（300000/450000/100000）。这不是门禁读数，只是"别把 select
  加在正数下标上"这条设计的事后核对。
- 锚点：本批在被锚定的两个文件里插了 102 行 ⇒ `docs/ABI.md` 36 处 `file:line` 重映射，按 diff
  位移算出新行号后**逐字比对**确认 48/48 条内容对上才 `--bless`，最终 120 锚点漂移 0 / 新 0 / 消失 0。
  **本批踩到的一次流程错，记下来**：我先 bless 了一次锚点基线，随后又继续改代码，第二次 relocate
  就拿"相对 HEAD 的 diff"去移一套"已含第一批插入"的行号 ⇒ 29 条逐字比对 FAIL。把 `docs/ABI.md` 与
  `tools/baselines/abi_anchors.tsv` 回退到 HEAD 重做才对。规则：**代码改完再 bless 锚点，中间不许有第二次 bless**。

### OPEN

- `ArraySize` 非 `Literal`（Param/Const）与 `is_array_param`（切片形参）两档**未实测**，不宣称覆盖。
  下一批要扩就先测这两档，再谈别的。
- 区间/切片下标（`l[0-2:]` 一类）在另一条 lowering 路径上，本批没碰。
- relocate + 逐字校验两个脚本目前还是 `/tmp` 临时件；等 #52（锚点工具看不见裸行号）有结论时
  一起并进 `tools/`，否则同一份 102 行插入下批还要再手抖一次。

---

## 批次 332（#54 落地：`for`/`range` 的归纳计数器从用户变量里分出来）

批次 329 把这条族钉成已知红（#54），当时的读数是"`for k in range(3)` 跑完 `k==3`，而 `while`
计数器结束值与 Python 一致"。本批不补尾值 store，而是**在 MIR 词表里给归纳计数器一个自己的槽**
—— 尾值从此是控制流的结构性后果，不是补丁。

### 改了什么（一处词表 + 一处构造 + 四处消费点）

| 层 | 位置 | 内容 |
|---|---|---|
| MIR 词表 | `src/middle/mir/mir.rs:172`（`MirStmt::For`） | 新字段 `counter_id: u32`，与 `var_id` 并列；doc-comment 记了两者为何不能共用 |
| MIR 构造 | `src/middle/mir/gen.rs:2449-2461` | **全仓只有这一个 `range` 版 `For` 构造点**（实测 grep 确认）：新开 `counter_id` 槽、注册 `MirExpr::Var` + `Type::I64`，起始值同时写进两个槽 |
| MIR 构造 | `src/middle/mir/gen.rs:2492-2497` | 在 body 切分之**后**往头部插 `Assign{lhs: var_id, rhs: counter_id}` —— 每次真正进入迭代才绑定，"跑完不绑定第 n 个值"因此是自然的 |
| MIR 构造 | `src/middle/mir/gen.rs:2508` | `For` push 带上 `counter_id` |
| IR 降低 | `src/backend/codegen/codegen.rs:5315`、`:5357`、`:5423-5436` | 解构出 `counter_id`；`for.cond`/`for.inc` 的 load/store 全走 `counter_ptr`，`var_id` 只剩 body 里那条绑定 |
| IR 槽收集 | `src/backend/codegen/codegen.rs:1853`、`:1862` | `collect_ids_from_stmt_safe` 两个槽都插。**必须**：For 降低用 `self.locals.get(..).unwrap()`，槽没被收集就是编译器 panic |
| IR 替换 | `src/backend/codegen/codegen.rs:3262` | `substitute_stmt` 原样透传（`Substitution` 是类型变量→类型，id 不该被动） |
| DCE | `src/middle/optimization.rs:115`、`:121` | `used.insert(*counter_id, true)` —— 计数器的读写方是 codegen 的循环机器，不是这条语句里任何表达式，不显式保活会被判无用 |
| DCE/CSE | `src/middle/optimization.rs:450` | CSE 的 `For` 臂加 `counter_id: _` |
| 单态化 | `src/backend/codegen/monomorphize.rs:164`、`:171` | 透传 |

`MirStmt` derive 了 `Debug`，所以 `--dump-mir` 自动带出新字段，无需改格式化代码。

### 读数（AOT `-o` 后与 CPython 逐条对照，12 档）

"修复前"列只填**本批真实回测过**的档；没回测的写"未回测"，不拿推断顶替读数。

| 档 | 程序形状 | 期望 | 修复前 | 修复后 |
|---|---|---|---|---|
| p1 | `for k in range(3)` 后读 `k` | 2 | **3** | 2 ✓ |
| p2 | `for k in range(3): k = 9` 后读 `k` | 9 | **10**（且迭代被改写） | 9 ✓ |
| p3 | 动态起点 `range(2,5)` 尾值 | 4 | **5** | 4 ✓ |
| p4 | 嵌套 `range(2)`/`range(3)` 两个尾值 | 2 / 1 | **3 / 2** | 2 / 1 ✓ |
| p5 | 集合式 `for k in [1,5,7]` 尾值 | 7 | 7（本来就对） | 7 ✓ |
| p6 | `break` 后读 `k` | 2 | 2（本来就对） | 2 ✓ |
| p7 | `for k in range(0)` 后读 `k` | NameError | 0 | 0（**不在本批范围**，见下） |
| p8 | `for k in range(4): print(k); k = 100` | 0..3 然后 100 | 未回测 | 全对 ✓ |
| p9 | 循环内 `continue` + 尾值 | 0/2/3 然后 3 | 未回测 | 全对 ✓ |
| p10 | `for … else` 里读 `k` | else tail: 2 | 未回测 | 对 ✓ |
| p11 | 累加后同时读 `total` 和 `i` | 10 / 4 | 未回测 | 10 / 4 ✓ |
| p12 | 通配 `for _ in range(3)` | 1/1/1 | 未回测 | 对 ✓ |

p5/p6 的"本来就对"是有原因的，值得记下来免得下批又去"修"它：`for x in <集合>` 在 gen.rs 里
本来就被下型成带内部索引槽的 `While`（元素在 body 里绑定），所以**这条路径一开始就是对的**，
本批只动了 `range` 路径 —— 也就正好是批次 329 观测到的"只有 range 族红"。

p7 单独说明判据边界：空区间时 Python 抛 `NameError`，zeta 留 0。这是"未绑定变量读取"一族
（缺诊断），不是"尾值取错"一族，**本批没有宣称修它，也没有为它改判据**。

### 钉用例

- `tests/diff/cases/control_for_var_after_loop.dcase`（改）：批次 329 的档，note 改成
  "原为红、批次 332 转绿、此后是回归钉"，body 扩了动态起点档和嵌套档。
- `tests/diff/cases/control_for_var_written_in_body.dcase`（新）：p8 + p9 两档。按批次 331 的
  教训（**补一个特例形状 ≠ 闭合值域**），尾值档和"body 内改写循环变量"档必须同批各钉一条 ——
  后者在旧实现里不只是尾值错，是**迭代本身**被改写（p2 只跑一轮）。

### #53 的闭包复采（同一批顺手做，因为改的就是判据覆盖面）

用随机生成器对两个 seed 各采 60 条回测：

| seed | match | mismatch | runtime |
|---|---|---|---|
| 20260922 | 54 | **0** | 6 |
| 20261005 | 53 | 1 | 6 |

批次 331 的闭包轨迹是 43 → 50 → 54；20260922 上唯一的残留是 6 条 `exit 112`，那全是 #55
（尾表达式当退出码），与下标无关。**#53 到这里才算按值域闭掉**。

### 顺带查出并修掉的一个方法论 bug：生成器的分类不可复现

跨批次对 seed=20260922 的读数时，同一个 seed 两次跑给出的**文件名和 `# @cat:` 标签不同**
（实测 12 条里 4 条换标签），而程序正文逐字相同。根因在 `tools/gen_diff_cases.py:263`：
众数用 `max(set(kinds), key=kinds.count)`，平票时胜出者取决于 `set` 的迭代顺序，而那个顺序随
`PYTHONHASHSEED` 变。改成 `max(dict.fromkeys(kinds), …)` 按首次出现顺序裁决。

影响范围要说准：**被钉进库里的用例正文一直可复现**，批次 329/331 的族归属结论站得住；
不可复现的是"随机目录里第 N 条属于哪一族"这个标签，所以那几批按标签做的跨批次对照读数需要
重新采一次才可比（本批已重采，见上表）。

### 验证

- 差分：**112 / 126、88.9%**（container 27/28、control 25/26，两条 `for` 档全绿；新增 2 条用例
  把分母从 124 抬到 126）。基线 `--bless` 到 `match_min=112`。
  注：五步门禁里那次 diff 读数打的是 `112/125`，因为门禁跑在第二条 `.dcase` 落地之前 —— 当前
  工作树重跑是 `112/126`，两者不是同一个分母，别当成不一致。
- 五步门禁（`./tools/run_all.sh`，rc=1 是存量 `py_fail==2` 判据）：official **194/194 编译、
  191/194 链接**、python_style **289/2/4/0**（失败仍是存量的 `t231_dict_set_cast_fromkeys`、
  `t233_listcomp_condition_capture`）、语料 **39/39**、jit **ok=169 / trap=320 / fail=0 / segv=0**
  （下限 163）、diff 见上。前四步与批次 331 **逐项相同** —— 改了 `MirStmt::For` 的词表而四个
  既有基线一位没动，说明新增字段在所有旧 lowering 路径上都是透传。
- 性能（任务 #27 的成对同负载口径）：300 万次迭代的 `for i in range(3000000): t = t + i`，
  新旧两个编译器各编一份、二进制交替各 3 轮，**6 次全部 1.25s**（10ms 分辨率下无可测差），
  即 body 头部那条 `var_id = counter_id` store 被 LLVM 管线吃掉了。
  **对照档证明这份比较是活的**：语义真有差的那条程序（`d.z`，尾值 3 vs 2）两份二进制
  `cmp -l` 差 **52 字节**，现在还能在 `/tmp/b332/d_old.bin` / `d_new.bin` 上复核。
  诚实标注：`perf_for` 那对二进制的"只差 3 字节元数据"读数当时是实测的（`--emit-llvm` 因为 #28
  产出的是 Mach-O 而非文本 IR，只能走 `cmp -l`），但**那两份二进制已随清理丢失**，此数暂不可复核。
- 锚点：`gen.rs` 净插 27 行、`codegen.rs` 净插 9 行 ⇒ `docs/ABI.md` **36 处** `file:line` 重映射
  （共 43 处改写），按 `git diff -U0` 位移算新行号后**逐字比对 48/48 对上**才 bless，
  最终 120 锚点漂移 0 / 新 0 / 消失 0。**本批只 bless 了一次**（遵守批次 331 定的规则）。
- 结构核对：`MirStmt::For` 的全部消费点（codegen 降低 + 槽收集 + substitute、optimization DCE + CSE、
  monomorphize、gen 构造）都在这批的表里，编译器改完能跑通五步门禁本身就说明没有遗漏的解构点
  —— Rust 的穷尽匹配会拒绝漏臂，这是"改词表"这类重构便宜的地方。

### OPEN

- **#56（本批新查出）**：动态负下标在 `and` 条件的三元式里读回 0 —— `l[0-3] if (k<=k and 7<3)
  else (k+7)` 期望 7、实得 0，而**同一表达式去掉 `and`**（`l[0-3] if (7<3) else (k+7)`）是对的。
  已钉成 `tests/diff/cases/container_neg_index_in_and_ternary.dcase`（4 档最小化，**故意留红**）。
  批次 331 的判据在 `normalize_subscript_index` 里按容器取 `len`，这条路上 `len` 的来源与三元式
  条件求值顺序的交互**尚未定位到实现点** —— 只登记读数，不写结论。
- #55（尾表达式 → 退出码）现在是 seed 20260922 上唯一残留的红族（6 条 runtime +
  `control_tail_value_exit_code.dcase`），下一批的默认候选。
- p7 那族（空区间/未绑定变量读取）缺的是**诊断**而不是值修复，与 #36 的"静默截断"同属
  "错得不吭声"一类，需要单独裁决。
- 批次 331 遗留的两档未实测项（`ArraySize::Param/Const`、`is_array_param` 切片形参）仍然没碰。

---

## 批次 333（#55 落地：带模块体的入口函数不交回尾值，改写点收成一个）

批次 332 把 #55 登记为"下一批默认候选"。本批兑现它，并按批次 331 的教训把判据放在**值域**上
（"入口体内所有 Return"），而不是按语句形状打第四个特例。

### 改了什么（三个实现点）

| 点 | 位置 | 内容 |
|---|---|---|
| parser 判定 | `src/frontend/parser/top_level.rs:1742` `PY_ENTRY_ATTR`；打标两处：`:1913`（合成 main）、`:1891`+`:1894`（用户的 main，判据 `!merged.is_empty() \|\| saw_main_guard`）；guard 证据 `:1755` 声明、`:1799`/`:1821` 两处赋值 | "这个 `main` 带模块体"只有 parser 知道，MIR 侧不再猜 |
| MIR 消费 | `src/middle/mir/gen.rs:158` 字段、`:257` 字面量初始化、`:881-883` 逐 item 复位并按属性判取、`:1128-1131` 调用点、`:841-861` `force_entry_returns` | 递归改写 `Return` 的槽（含 `If::then/else_`、`For::body/else_body`、`While::body/else_body`），**副作用保留、只换交回的那个槽** |
| 属性不报警 | `src/frontend/macro_expand.rs:671`（`process_attributes` 的元数据档，照 `inline`/`must_use`） | 否则会掉进 W5001 未知属性告警，那个计数是门禁基线 |

为什么不做成"每个 push 点各判一次"：到达 `Return` 的路由实测有 **4 条** —— 尾值合成
`gen.rs:1115`、`lower_expr` 的 Return 档 `:1817`、parser 把函数尾表达式提升进
`FuncDef::ret_expr`（`top_level.rs:284` 提升、`gen.rs:1879-1881` 落地）、`ExprStmt` 包着的
Return `:2105`。**第三条是这批现场才发现的**：q12 打了标记、`py_entry` 也取到了，退出码仍是 7，
因为那条 `Return` 根本不经尾值合成。`match` 臂内 `return`（`:11101`）走的是表达式路径，本批
未实测（登记 OPEN）。

`py_entry` 的判据为什么不能挂在方言谓词上（任务 #51 那条）在本批第二次得到印证：第一版实现就是
用"缩进预处理器是否触发"当"是不是 python"，结果 **flat 的 q1–q4、q11 全部走花括号分支，整个修复
是惰性的**。`PY_ENTRY_ATTR` 的注释里写明了这段，避免下一个人再挂回去。

### 值域读数（21 档探针，AOT `-o` 后跑真进程比 rc + stdout）

HEAD 列是**同仓 HEAD 编译器**编出来的二进制实测（临时 `target/release/zetac_head`，量完即删），
不是推断。期望列：python 档取 CPython 3 实跑，`q6`/`q15` 是花括号方言对照档（无 CPython 对照）。

| 档 | 形状 | 期望 rc | HEAD | 本批 |
|---|---|---|---|---|
| q1 `l=…;print(len(l));sum(l)` | 合成 main 尾值 | 0（stdout 3） | 15 | 0 ✅ |
| q2 尾 `len(l)` | 同上 | 0 | 3 | 0 ✅ |
| q3 尾 `l.append(12)` | 方法调用尾值 | 0 | 112 | 0 ✅ |
| q4 `k=9;print(k);k` | 裸变量尾值 | 0（stdout 9） | 0 | 0 ✅（HEAD 已对，登记） |
| q5 `def main():…return 7` + `main()` | 合并 main 显式 return | 0（stdout 1） | 7 | 0 ✅ |
| **q6** `fn main() -> i64 { 42 }` | **花括号对照** | 42 | 42 | 42 ✅ 未受影响 |
| q7 `def f(): 42` + `print(f())` | 非入口函数尾值 | rc 0 | 0（stdout 42） | 0（stdout 42）✅ 保持 |
| q8 尾值在 `if/else` 分支里 | 尾值合成的 `If{dest}` 档 | 0 | 15 | 0 ✅ |
| q9 尾值 `i*100`（while 之后） | Assign 档 | 0（stdout 3） | 44 | 0 ✅ |
| q10 只有 `print("hello")` | 无值尾 | 0 | 0 | 0 ✅（HEAD 已对） |
| q11 尾值 `d["a"]` | DictGet 档 | 0 | 1 | 0 ✅ |
| q12 guard-only + main 尾值 | **`ret_expr` 提升路由** | 0（stdout in main） | 7 | 0 ✅ |
| q13 guard-only + `if` 内 return | 嵌套 `If` 分支 Return | 0 | 9 | 0 ✅ |
| q14 guard-only + `for` 内 `if` return | 嵌套 `For::body` Return | 0 | 5 | 0 ✅ |
| **q15** `fn main() { if …{return 7} return 4 }` | **花括号对照（嵌套）** | 7 | 7 | 7 ✅ 未受影响 |
| q16 模块体 + `main()`，`if` 内 return | 合并 main + 嵌套分支 | 0 | 8 | 0 ✅ |
| q17 非入口函数 `if/else` 尾值 | 对照：函数尾值仍返回 | rc 0 | 0（stdout 21） | 0（stdout 21）✅ 保持 |
| q18 `sys.exit(3)` | **主动设退出码的正路** | 3（stdout before） | 未测 | 3 ✅ |
| q19 裸 `exit(3)` | 同上 | 3 | 未测 | 3 ✅ |
| q20 `for … else: return 4`（入口内） | `For::else_body` 递归档 | 0 | 未测 | 0 ✅ |
| q21 `while … else:` + 嵌套 `if` return | `While::else_body` 递归档 | 0（stdout done） | 未测 | 0 ✅ |

`q18`/`q19` 是**否证档**：本批一度据此登记 OPEN 说"修完 #55 后没有任何途径能设非 0 退出码"，
实测发现结论错了 —— `pylib/registry.txt:294` 早已把 `sys exit` 接到 `py_sys_exit`
（运行时 `exit(code & 0xff)`），rc=3 与 CPython 一致。见下文"自查 2"。

### 验证

- 差分：**112/126 → 115/128**（88.9% → 89.8%，`match_min` bless 到 115）。新增 2 条 pin
  （`control_exit_code_guard_only_main`、`control_exit_code_return_in_branch`），另有 1 条**存量红转绿**：
  `control_tail_value_exit_code`（批次 331 立的那条）。用 HEAD 编译器复跑同一子集做对照：
  control 类 **25/28 → 28/28**，三条红正是 #55 家族（`exit 7` / `exit 5` / `exit 15`），
  跑法 `ZETAC=<abs>/target/release/zetac_head python3 tools/diff_test.py --only control`
  （注意 `ZETAC` 必须是绝对路径，相对路径会让 28 条全被判成"参考侧跑不出真值"的假 bad_case）。
- 五步门禁（`./tools/run_all.sh`，rc=1 是存量 `py_fail==2` 判据）：official **194/194 编译、
  191/194 链接**、python_style **289 过 / 2 败 / 4 KNOWN-FAIL / 0 XPASS**（仍是存量的
  `t231_dict_set_cast_fromkeys`、`t233_listcomp_condition_capture`）、语料 **39/39**、
  jit **ok=169 / trap=320 / fail=0 / segv=0**（下限 163）、diff 115/128。**诊断基线一项未变**
  （official 10 文件/15 行、python_style 82 文件/191 行）⇒ 新属性确实没漏进 W5001。
- 性能（任务 #27 的成对同负载口径）：`while` 100000 次 `m[n % 8]` + 内层 8 次 dict 写入，
  新旧两个编译器各编一份，二进制交替各 3 轮：**95/96、93/94、94/93 ms**（10ms 内无可测差），
  两份输出逐字相同（350000 / 8）。这条不是门禁读数，是"入口只执行一次，改写不落在环里"的事后核对。
- 锚点：`gen.rs` 净 +49、`top_level.rs` +37/-2、`macro_expand.rs` +5 ⇒ `docs/ABI.md` **29 处**
  `file:line` 重映射，按 `git diff -U0` 位移算新行号后**逐字比对 48/48 对上**才 bless（120 条，
  漂移 0 / 新 0 / 消失 0）。**随后只改了文档**（新增 §6.7，带 1 条新锚点 `pylib/registry.txt:294`），
  代码未再动，所以本批实际有**两次 bless**：第一次是代码位移重映射（120），第二次只把 §6.7 的
  新锚点收进基线（121）。批次 331 定的"中间不许有第二次 bless"针对的是"bless 完又改代码"，
  这里没有代码变更 ⇒ 基线与代码是自洽的；把这点写明而不是藏起来。

### 本批的两处自查（都记成规则）

1. **同一条判据的两个调用点，我只补了一个。** `is_main_guard` 在 `synthesize_implicit_main` 里有
   两处调用（`Block` 展开分支 `:1798`、顶层语句分支 `:1820`）；`saw_main_guard = true` 先只加在
   前者 ⇒ q12/q13/q14 仍红。定位靠临时 `ZETA_DBG55` 探针（打印"merge: guard=false"，一眼看出
   谓词没被走到，探针已删）。这是批次 331 那条"补一个特例形状 ≠ 闭合值域"在本批内部的重演，
   只不过这次漏的是**同一判据的第二个调用点**。
2. **登记 OPEN 前先跑一条最小探针。** 见 `q18`/`q19`：结论从"通道不存在"改成"通道在注册表里"，
   只因为最初只 grep 了 MIR 下型点。规则：**"某某没接线"这类负命题，必须至少有一条实测的
   反向探针撑着**，否则只能写成"未查到接线点"。

### docs/ABI.md §6.7（本批新增，跨边界合同）

`main` 的返回槽即进程退出码 —— 这条边界不在 §3.1 的签名字母表里，此前只被 Rust 式"尾值即返回值"
建模管着，Python 侧对应规则（模块尾值丢弃）从未接线。§6.7 写清合同三行（判定在 parser、
消费点唯一、改写递归）+ 三条易踩点（方言谓词、4 条路由、属性须走元数据档），并给出
`sys.exit` 这条设退出码的正路。**本批有意不加 `file:line` 锚点到那张表**（只在 §6.7 末尾留了
`pylib/registry.txt:294` 这一条），行号证据集中放在本节，免得文档与代码行号两处同时腐坏。

### OPEN

- `__zeta_module_body__` 载体（被 import 模块的模块体，`top_level.rs:1857-1873`）**故意未打标、也未实测**。
  它由 resolver 当 import 初始化器执行，"退出码"语义在此不适用，但"该不该打标"没有读数支撑 ——
  下批要动 `synthesize_implicit_main` 就先补这条的实测。
- 非入口函数的尾值仍是返回值（`q7` 打 42、`q17` 打 21，CPython 都是 `None`）。这是"尾表达式即返回值"
  的整体建模，牵一面语料（与 #48 的方言裁决同类），**不在 #55 范围内**，本批只保证它没被改动。
- `match` 臂内的 `return`（`gen.rs:11101` 那条 `vec![MirStmt::Return{..}]`）**未实测**：
  它是表达式路径、且 `match` 臂在入口函数里的写法尚未在语料里出现。
- #56（`and` 条件三元式里的动态负下标）仍**故意留红**；批次 331 的两档未实测项
  （`ArraySize::Param/Const`、`is_array_param` 切片形参）仍未碰。
- 下一批默认候选：**#51**（方言判据挂在"有没有缩进"上）—— 本批的 `PY_ENTRY_ATTR` 绕开了它，
  但 `//` 整除那条路还瞎着，且现在有了第二个实测后果（flat python 文件既用不了 `//`、
  也曾经让 #55 的第一版修复完全失效）。

## 批次 334（#51 落地：`//` 的方言判据从"有没有缩进"换成"该列括号是否未闭合"）

批次 333 点名的默认候选。同一处根因（`changed` 被当方言谓词）第二次咬人，这次是
`src/frontend/indent.rs` 的 `//`→`floordiv` 改写整个挂在 `changed==true` 上，
于是**所有 flat python 文件**用不了整除。

### 改了什么（三处，全在 `src/frontend/indent.rs`）

| 点 | 位置 | 内容 |
|---|---|---|
| 方言证据换成行内证据 | `:227-244`（`if !changed` 分支的新增段） | 无缩进文件里逐处判：改写后文本与原文件不同才返回 `Some(text)`，并照旧喂 `estimate_line_origins`（C1 行号映射）；未改写时保持原有的 `Ok(None)`/折叠文本两条出口（`:247-256` 一字未动） |
| 扫描器带跨行括号深度 | `:581` `rewrite_floordiv_lines`、`:644` 判据、`:693-697` 深度累加 | 深度只算 `(` 与 `[`，**`{` 故意不算**；跨行携带、每行末钳到 0 |
| python 路径收敛成同一个函数 | `:266`（原 `rewrite_floordiv_line` 就地循环） | `only_in_brackets=false` ⇒ 行为与批次 333 之前逐字相同（缩进=方言证据，`t138` 的 `return a // b` 继续可用） |

### 判据是量出来的，不是想出来的

`/tmp/b334/scan.py` 镜像 `rewrite_floordiv_line` 的字符串/三引号/`#`/`//` 处理，把两套语料里
"行内已有代码、后面跟 `//`"的每一处抓出来（official = `tests/unit-tests/*.z`，与门禁第一步同集合）：

| 候选判据 | official 命中（209 处候选） | python_style 命中（5 处候选） | 结论 |
|---|---|---|---|
| `(`/`[` 净深度 > 0（本批采用） | **0 处 / 0 文件** | 3 处（`t138:31`、`t138:36`、`t403:11`） | 语料内零误伤，采纳 |
| 同上，但深度只在行内累计（不跨行） | 0 处 | 同样 3 处 | 两语料无差异 ⇒ 选**跨行携带**（多解锁 `print(` 换行续写的形状，成本为 0） |
| 看 `//` 右侧像不像操作数（≤1 词） | **36 处**：`return 1  // Success`、`let v = f(a)  // SIMD`、`scalar_avg * 100 / cache_avg  // Percentage` | — | **否决**：会静默改写 36 条花括号语料 |
| 文件里没有 `;` ⇒ 判为 python | — | — | **否决**：194 个 official 文件里 **115 个一个分号都没有**，其中 54 个还写行内 `//` 注释（复测口径见 §6.8 末注；批次 328 记的 44 是"代码后跟 `//` 且不在字符串里"的更严口径） |

### 探针（`/tmp/b334/`，`indent_dump` 看预处理、`zetac -o` 看运行）

| # | 形状 | 期望 | HEAD | 本批 |
|---|---|---|---|---|
| p1 | `print(7 // 2)` / `print(-7 // 2)`（flat） | 3 / -4 | 不改写（`//` 当注释） | 3 / -4 ✓ |
| p2 | `print(` 换行 `x // 10)` | 1 | 不改写 | 1 ✓（跨行携带） |
| p3 | 花括号 + `return 1;  // Success`、`if r == 3 { return 0;  // Success` | 原样 | 原样 | 原样 ✓（零改写，`Ok(None)`） |
| p4 | `t = a // b`（flat、括号平衡） | 3 | 静默 `t=7` | **仍静默 7**（登记 OPEN，pin 成 `t406` known-fail） |
| p5 | `# …https://x//y` 注释 + `print(len("https://x//y"))` + `print( 7 // 2 )` | 12 / 3 | 后半截丢 | 12 / 3 ✓（字符串、`#` 注释不动） |
| n1 | `print(max(1, 7 // 2))` | 3 | — | 3 ✓（嵌套调用） |
| n2 | `print((7 // 2) * 3)` | 9 | — | 9 ✓（内层括号） |
| n3 | `print(len(s) // 3)` | 1 | — | 1 ✓（函数调用作左操作数） |
| n4 | `print(` 换行 `-9 // 2)` | -5 | — | -5 ✓（负操作数 + 跨行） |
| p6 | `print(7.5 // 2.0)` | CPython `3.0` | — | `3.000000` —— **未做成用例**：差在浮点 repr，属 #48 的方言裁决，混进来会把两个问题记成一件事 |

### 验证

- **单元**（`cargo test --lib`，新增 5 条，均绿）：`flat_floordiv_inside_a_call_is_the_operator`、
  `flat_floordiv_depth_carries_across_a_multiline_call`、`flat_brace_style_trailing_comment_is_never_the_operator`
  （3 个形状）、`flat_url_in_string_or_hash_comment_survives`、
  `flat_floordiv_at_depth_zero_is_still_a_comment`（**故意 pin 住洞**，防它以后被当成新行为）。
  同一次跑里 `header_colon_stripped_with_trailing_comment` 红 —— 已用 HEAD 版 `indent.rs` 单独复跑
  确认**同样红**（就是任务 #23 记的那条存量失败，不是本批引入）。
- **差分**：3 条 `numeric_floordiv*` 由 mismatch 转 match；新增 2 条（`nested_call`、`multiline_call`）
  直接 match。`--only floordiv` 子集 3/3。基线 bless：`115/128 89.8%` → **`120/130 92.3%`**，
  mismatch 12→9，numeric 档 `18/26` → `23/28`，`bad_case=0`。
- **python_style**：`t403_floor_div_inside_call` 摘掉 `// known-fail:` 标记（头部注释重写说明
  原因，程序体 `x = 7` / `y = 2` / `print(x // y)` 三行一字未动）⇒
  `289/2/4/0` → **`290 passed, 2 failed, 4 known-fail, 0 xpass`**（失败仍是 `t231`/`t233` 两条存量）；
  known-fail 计数不变是因为同时补进 `t406`（新增洞）。编译告警 `191 行 / 82 文件` →
  **`190 / 81`**，正是 t403 那条 W1002 截断告警消失。
- **official**：`compile 194/194`、`compile+link 191/194`、link-only 3 个文件同一批符号，
  诊断 `10 文件 / 15 行` —— **四项全不变**（这是"零误伤"的运行时背书，不只看扫描器）。
- **corpus** `39/39`、**jit** `ok=169 trap=320 fail=0 segv=0`（下限 163）—— 均不变。
- **锚点**：`check_abi_anchors` 漂移 0 / 消失 0；新增 2 条（`indent.rs:581`、`:644`）解析到的
  正是目标行，基线 `121` → **`123`** 已 bless。
- 门禁 `./tools/run_all.sh` rc=1 —— 唯一红项仍是 `py_fail==2` 的既有判据（设计如此）。
- **性能未测**。理由写在自检里，不拿"应该不影响"充当读数。

### 自检（本批踩到 / 差点踩到）

1. **差点把"扫描器统计"当成"编译器行为"交付**。`[A] official 命中 0` 只证明*文本层*没误伤，
   不证明 194 个文件编译结果不变 —— 所以 official 四项单独复跑了一遍，并把"必须跑全量门禁"
   从判据升成实际动作。
2. **不在洞上盖新洞，也不顺手扩面**。p4 的形状（深度 0 的平铺整除）本批闭不了：闭它要
   文件级方言证据，而行内启发式已全被实测否决。做法是给洞建 known-fail 用例（`t406`）+
   单元 pin（`flat_floordiv_at_depth_zero_is_still_a_comment`）+ §6.8 记"剩余洞不在合同内"，
   而不是再猜一条判据把 official 的 36 处单词尾注释赌进去。
   浮点那半（p6）同理**没有**做成用例 —— 那是 #48 的 repr 裁决，混进来会把两件事记成一件事。

### docs/ABI.md §6.8（本批新增）

"这是 python 源码"不能从"有没有缩进"推出来：写清 `changed` 的真实语义、两种方言都为假的成因、
现行合同（缩进文件一律改写 / 无缩进文件按深度逐处判、`{` 不计入深度）、判据的实测依据
（209 处全在深度 0）与被否决的替代判据（RHS 词形 36/209、`;` 缺失 115/194），
并把 §6.7 陷阱 ① 从"未闭合的兄弟问题"改成指向 §6.8。

### OPEN

- **深度 0 的平铺整除**（`t = 7 // 2`）仍是静默错值 ⇒ `t406` known-fail。要闭需要文件级
  方言证据（例如 `--dialect` 旋钮或"含 `def`/`elif`/`lambda`/`import` 即 python"的显式谓词），
  本批有意不做：那是一条新的**方言开关**决策，属 refactor.md 的 G.5e 家族，不该塞进一条 bug 修复。
- `rewrite_floordiv_lines` 的深度跨行携带在"文件本身括号不闭合"时会一路 > 0，此时行尾注释被
  当运算符改写。official 语料 0 命中；但这是**结构性**风险，不是零风险。若哪天出现
  "解析失败的 flat 文件报出一堆莫名其妙的未定义名字"，先回这里查。
- 批次 333 的 OPEN 全部继承（`__zeta_module_body__` 未打标未实测、非入口函数尾值、
  `match` 臂内 `return`、`While::pre_cond` 未递归、#56 故意留红、批次 331 两档未实测项）。
- 下一批默认候选：**#52**（锚点核对器看不见"裸行号"引用 —— 本批 §6.8 就写了
  "`:687-694`" 这种裸区间，正是它覆盖不到的形态）；或 **#32**（短 vec 动态下标死循环的
  `cap>=8` 旧判据副本）。

## 批次 335（#52 落地：锚点核对加第 4 层"可归属"，并把 ABI.md §1–§4 的裸行号全部绑上路径）

批次 334 点名的默认候选。第 4 层判据一句话：**文档里每个 `:NNNN` 形态的行号引用都必须绑到一个
路径**，绑不上的进 `«待归属»` 棘轮行（同一张 TSV，只许降不许升）。设计动机不是"缺功能"，而是
"已核对"这句话的可信度：批次 334 报"123 个锚点全对上"，而同一份文档里飘着的裸行号一个都没被数过。

### 判据是量出来的（四种归属方式逐一实测）

| 候选 | 实测 | 结论 |
|---|---|---|
| 跨行就近（裸 `:NNNN` 绑"往上最近的 `path:`"） | §3.4 的 `:8052` 会挂到 `py_additions.c`、§3.3 的 `:4283` 同理 | **否决**：挂错的锚点读起来像已核对过，比没有锚点更坏 |
| 同行就近（采用） | §3.5 有一行同时含 `NOTES.md:25-28` 和三个 `:6xxx` 代码行号 | 采纳，但**只有代码/数据文件参与继承**（`.md`/`.z` 不设 `last_cited`），否则 `:6116` 挂到一份 61 行的笔记上 |
| 裸区间 `6968-6982` 也扫、按同行最近路径绑 | 同行有路径时仍误绑：`批次 333-334`、年份、计数与行号区间同形 | **否决自动绑**：一律进待归属，逼人改写 |
| ASCII 逗号列表 `:2550,2560` 当多锚点 | 与千分位 `1,719`、`80→62` 同形 | **否决**：判不出来 ⇒ 改文档写法（`:2560、:2550`） |
| 显式作用域行 `> 锚点源码：<path>`（空行/标题结束） | 18 档表、6 行探针表逐格写路径不可读 | 采纳；仓外证据写 `> 锚点源码：外部转储（… 不入锚点核对）` —— "核不了但有人显式说过"与"没人想过"是两件事，前者计入"声明为仓外 14 条" |

### 改了什么

| 点 | 位置 | 内容 |
|---|---|---|
| 第 4 层：可归属 | `tools/check_abi_anchors.py`（模块 docstring 判据 1-4 + `collect()`：`ANCHOR_RE`/`CONT_RE`/`SCOPE_RE`/`BARE_RANGE_RE`/`EXEMPT_RES`/`PENDING`/`EXTERNAL`） | 裸引用扫描、同行继承、作用域声明、批次号与 ISO 日期先剔除（等长空格保列号）、待归属棘轮、`--pending` 输出文档行号 |
| **修一条恒假判据** | `tools/check_abi_anchors.py:245` | `SRC_EXT.match(rel)` → `.search(rel)`。模式以 `\.` 开头，`match` 从 0 位起锚 ⇒ 对 `src/…/codegen.rs` 恒假 ⇒ "同行继承只认代码文件"从未生效，所有靠同行继承的续写锚点全落进待归属。**修法之前它给出的"已通过"是在空集上测的** |
| ABI.md §1–§4 重锚 | `docs/ABI.md:36-465` | 每处引用对当前源码重读后再落锚；§4 的 18 档表、N6-N11、§4.5 两张表全部改绑（见下表）。§5+ 有意不动，留给下一批 |
| 基线 | `tools/baselines/abi_anchors.tsv` | 123 条 → **240 锚点 + 84 种（93 处）待归属 = 324 行**；`--bless` 后 `漂移 0 / 新 0 / 消失 0`，rc=0 |

### 归属计数（可复现）

```
git show HEAD:docs/ABI.md > /tmp/abi_head.md   # HEAD=批次 334
./tools/check_abi_anchors.py --doc /tmp/abi_head.md --pending   # → 待归属 202 处 / 178 种
./tools/check_abi_anchors.py --pending                          # → 待归属 93 处 / 84 种
```

§1–§4（含 §4.5 两张表）**109 → 0**；整篇 202 → 93。剩下的 93 处按节分布 = 下一批的队列：
§5 类型布局 38、§6 跨边界 29、附 A 4、附 B 及以后 22。

分桶口径（两版都用**各自文件**的节起始行，不拿现行边界去切旧版 —— 本轮先这么错过一次，
把 §1–§4 多算了 21 处）：`grep -n "^## " docs/ABI.md` 给节行，`--pending` 每行第一列是文档行号。
旧版 §5 起于 431 ⇒ §1-2 = 3 / §3-4 = 106 / §5+ = 93 = 202；新版 §5 起于 467 ⇒ §1-4 = 0 /
§5+ = 93。**§5+ 那一组集合级未变**（`comm` 双向差为空：不多一处、不少一处）。

### 重锚时发现的是"内容级"错误，不是行号漂移

| 旧写法 | 实测 | 处理 |
|---|---|---|
| §3.1 `:4488-4497`、§3.5 `:6116` 一类裸区间 | 与源码差 90+ 行，且解析不到路径 | 改 `codegen.rs:4581-4590` / `codegen.rs:6211` 形态 |
| §3.2 表格 10 行的锚点 | 整体落后约 93 行（`abi_note` 告警器旧写 `6968-6982`） | 逐行重绑 `:6919-6926 … :7006-7007`，标题锚 `:6885-7010` 复算仍对 |
| §3.2 尾 `validate.md:149` | 该行是空行 —— 被"锚点行内容为空 ⇒ 定位失败"判据抓到 | → `:151`（`:149` 处曾是 P0#2 白名单，已在上一批改为节引用） |
| §3.3 C6"11 个手写 runtime 函数" | 清单实际 6+4 | → 10，并逐处给锚点 |
| §4.1 判模块限定名"只有 `contains("__")`" | `codegen.rs:2931` 是 `starts_with("zeta_")` 与 `contains("__")` 的二合一 | 改文；顺带记下"分派名与限定名共用一条判据"这一事实 |
| §4.1 `mangle_function_name :1346-1356` | 函数在 `:1363-1381` | 改锚 |
| §4.1 N3"注释 `:10360-10366` 自述后缀只为区分重载" | **能解析、但挂错**——那 7 行是 `DataFrame::column` 的限定名注释；讲后缀的是 `gen.rs:10548-10568` | 拆成三段（理由 `:10548-10550` / 三类豁免 `:10551-10561` / 剥后缀误伤事故 `:10562-10568`） |
| §4.2 18 档表全部锚点 | 偏移 **+59…+62 不恒定**，且档位 1-5 的旧锚落在 `get_or_declare_function` **函数体之外**（函数从 `:2636` 才开始） | 逐档重数改锚 `:2655-2663 … :2857-2947`，并新增"档位漂移实测"一段说明为何不能加常数修 |
| §4.2 N7"全仓有 4 处 `replace("::", "__")`" | `grep -rn src/` = **9 处，且全在 codegen.rs**；旧列表里第 4 个锚点还是行注释 | 改数 + 9 处逐给锚点 |
| §4.2 N9 `:780 的 all_mirs.sort_by` | `sort_by` 在 `src/main.rs:782`（778-781 是注释） | 改锚 |
| §4.3 N8 读侧档位 `:2753-2758` / N10 `:2861-2872` | 实际 `:2814-2819` / `:2915-2933`（四符号清单在 `:2928-2930`） | 改锚 |
| §4.4 ghost 清单 `gen.rs:8797/:8647/:8666/:12315` | 只有 `8797` 对；真实 6 处是 `:8797、:8816、:8835、:8854、:8914、:12535` | 改锚 + 补 `i64__all`/`normalize` 两名；**顺带暴露** `/` 分隔的裸 `:NNN` 完全不进判据 |
| §4.5 `例 codegen.rs:1069、1071、1078` | 1078 是注释行，`add_function` 在 1080；且 `、1071` 无冒号 ⇒ 隐形 | → `:1069、:1071、:1080` |
| §4.5 `原锚 registry.txt:208 … 条目在 :219` | 208 行现在是 `filterwarnings`，`py_asdict_unexpanded` 在 **219**（漂 11 行） | 改写为"原锚在…第 208 行，该号已被另一条目占用"，不把历史锚当现行锚；`:219` 显式给路径 |
| §4.4 `refactor.md:130` | 根目录活文档不在 `git ls-files` ⇒ 第 1 层"无解"，rc=2 | 改为"第 130 行 + 注明未入库、不入锚点核对"，并把这条边界写进脚本 docstring |
| §3.1 C2 的 `/tmp` IR 转储 | 仓外证据 | `> 锚点源码：外部转储（/tmp 下 --emit-llvm 落盘的 .ir，非仓内文件，不入锚点核对）` |

### 自检（脚本四条 rc 全路径实测 + 恒假判据的回归证据）

- `漂移/新增/消失/待归属增` 四条判据分开测：干净 rc=0；文档里加一条 `:9999` → `[待归属+1] :9999` rc=1；
  删掉表格一行（锚点消失）→ `[消失] codegen.rs:2803` rc=1；写 `nope/nothere.rs:12` → rc=2。
- **SRC_EXT 恒假的回归证据**（夹具 `/tmp/b335/t.md`，含 4 条正/负例）：把 `.search` 临时改回
  `.match` ⇒ 夹具"同行继承正例" `:1392-1405` 落回待归属（4 处 / 7 可解析）；改回 `.search`
  ⇒ 3 处 / 8 可解析。三条负例在两种实现下都留在待归属（裸区间、`.md` 之后的 `:6116`、
  笔记后的 `:1363-1382`）——即"修好之后仍然不放过该放过的"。
- 每条新锚都过 `--list` 逐条读指向的句子。第 4 层只保证"路径解析得开 + 行没漂"，
  **不保证"作者没把两个文件的行号混进同一段"**；本轮就是靠 `--list` 才发现 `:10360-10366`
  与 `gen.rs:8647` 这类"能解析、但挂错"的引用（脚本永远抓不到它们）。

### 门禁读数

`./tools/run_all.sh > /tmp/b335/gate.log 2>&1; echo "run_all rc=$?"`（ts
`2026-09-22T11:07:50Z`）**直读退出码 ⇒ rc=1**，唯一原因是既有判据
`tools/run_all.sh:253`（`py_fail != 0` ⇒ rc=1；批次 336 加第 6 步后该判据移到 `:280`）：
2 条失败仍是批次 326 以来的
`t231_dict_set_cast_fromkeys` / `t233_listcomp_condition_capture`，**不是本批新增**。

| 步 | 读数 | vs 批次 334 |
|---|---|---|
| official | compile **194/194**，compile+link 191/194（3 条 link-only：缺运行时绑定） | 相同 |
| python_style | 290 passed / **2 failed** / 4 known-fail / 0 xpass | 相同 |
| corpus | 39/39 | 相同 |
| jit sweep | ok=169 trap=321 fail=0 timeout=0 segv=0（total 490，门槛 ok≥163） | 相同 |
| diff | match=120 judged=130 rate=**92.3%** bad_case=0 | 相同 |
| 诊断 | official 10 文件 / 15 行；python_style 81 文件 / 190 行 | 相同 |

**为什么"五步全同"是本批的预期结果**：本批只动 `docs/ABI.md`、`tools/check_abi_anchors.py`、
`tools/baselines/abi_anchors.tsv`、`roadmap.md` —— 没有一条在编译链上（门禁只在
`target/release/zetac` 缺失时才 `cargo build`，本批未触发）。所以这份读数同时是一次
**负向自检**：文档层批次不该动二进制，动了才说明改错了地方。

`./tools/check_abi_anchors.py`（无参数，直读）⇒ **rc=0**，`漂移 0 / 新 0 / 消失 0 /
定位失败 0`；基线 `tools/baselines/abi_anchors.tsv` = 240 锚点行 + 84 种待归属 = 324 行。

### OPEN

- **§5+ 的 93 处待归属未核**（§5 38 / §6 29 / 附 A 4 / 附 B+ 22）。已棘轮化 ⇒ 不会更糟，
  但"没核过"这句话要一直显形。下一批按节推。
- **隐形引用形态仍靠人**：`stub:339`（冒号前是单词字符）、`、1071`（没有冒号）、
  `/:8647`（冒号前是斜杠）三种形状既不核对也不计数，本轮只在 docstring 里立了写法合同。
  要机器堵住，需要一条对账判据："正文里 `:\d{3,4}` 出现次数 == 可解析 + 待归属"，
  差值即隐形量。登记为 #52 的收尾项，不塞进本批。
- 锚点核对**仍未接 CI**（任务 #37：需要先给脚本加 src-only 作用域开关，否则会去核未跟踪的活文档）。
- 批次 334 的 OPEN 全部继承（深度 0 平铺整除 `t406`、括号不闭合 flat 文件的跨行深度风险、
  批次 333 那五项、#56 故意留红、批次 331 两档未实测项）。
- 下一批默认候选：**§5 类型布局的 38 处待归属**（同一把尺子往下推，做完 ABI.md 才是整篇可核）；
  或 **#32**（短 vec 动态下标死循环的 `cap>=8` 旧判据副本）。

## 批次 336（G.7b 从"有实现"推到"有读数"，并把 27 个反了的布尔旋钮收进一个实现点）

### 选题过程：两个测量把"修一个形状"换成了"回答一个问题"

1. 起点是 #36 / G.7 的截断清点。`tools/truncation_inventory.sh`（official 递归 198 + corpus 39 = 237 档）
   按丢行数排序 ⇒ 9 档共 **1,019 行**没进 AST（按档分布见下一节的表）。对最大那档做逐行二分，崩点打到
   函数体里的 `static mut counter: i64 = 0`。
2. **在修这个形状之前先量这个形状属于谁**：`static` 在 zeta 里**根本不是关键字**——
   `parse_stmt`（`src/frontend/parser/stmt.rs:1590`）的 21 臂 `alt` 里没有 `static`，
   `parse_type_path`/`parse_var_type` 也没有 `"static"` 这个 `tag`；而 `name: ty = expr;`
   本身是合法声明 ⇒ `static counter: i64 = 0` 被读成"声明叫 `static` 的东西"，
   `static` 被丢弃、`counter` 静默绑定，**不报错也不进 AST**。`static mut …` 则因"名字前有两个
   标识符"而整条失败。⇒ 结论：给 `static`/`static mut` 补特例只是"补一个形状"，闭合不了值域；
   真正的缺口是**方言关键字表**没有一处声明谁是关键字（登记为 OPEN，见下）。

   > **批次 337 实测更正本条的两处机制描述**（原文保留，因为它推出的结论仍成立）：
   > ① "不进 AST"只对了一半——`static counter: i64 = 0` **进的是 AST**，只是形状变了：
   >   `--dump-mir` 给 `Assign{lhs: Var(counter), rhs: IntLit(0)}` ＋ `Return{val: …}`，
   >   即"声明"被降级成一次普通赋值，`static` 这个词消失。
   > ② 中止整条的不是"两个标识符"，而是**第二个词不能开表达式**。任意多个裸词都能被吃掉：
   >   `apple banana cherry target: i64 = 7` 照常解析并把 7 赋给 `target`（三词各成一条无副作用
   >   语句）。`static mut …` 崩是因为 `mut` 不是表达式起手式 ⇒ `parse_block_body` 失败 ⇒
   >    enclosing `fn` 失败 ⇒ 顶层 `many0` 停下 ⇒ W1002 丢尾。
   >   ⇒ 所以本表 `benchmark_simd_vs_scalar.z` 那一档的"崩点 = 体内 `static mut counter`"要读成
   >   "**崩点是 `mut`，而 `static` 在崩之前已被静默吞掉**"；后者是批次 337 的 W1004 覆盖的东西。
   >   （`parse_stmt` 的行号随本批插入移动到 `stmt.rs:1626`。）

### 读数：解析恢复能不能默认打开——现在不能，代价是量出来的

G.7b 的跳过-重同步（`parse_zeta_impl_recover`，`src/frontend/parser/top_level.rs:1958`，
由 `:1930` 的 `ZETA_PARSE_RECOVER` 选路）**代码早就在，但从没人测过它值不值默认**。
`./tools/knob_probe.sh` 的 B 段（official 194 档逐个编译，直读退出码）：

| 口径 | 关态（默认） | 开态（`ZETA_PARSE_RECOVER=1`） |
|---|---|---|
| rc 分布 | **191 × rc=0 / 3 × rc=1** | **185 × rc=0 / 9 × rc=1** |
| 丢进行 AST 的行 | 1,019 行 / 9 档 | 0 档报 W1002 |
| 0→非0（恢复引入的失败） | — | **6** |
| 非0→0（恢复修好的） | — | **0** |

那 6 档：`benchmark_simd_vs_scalar`、`integration_all_features`、`primezeta_usize_test`、
`quantum_basic`、`selfhost`、`test_const_expression`。另 3 档双态皆红 = 既有的 link-only
（缺运行时绑定，附 B#10 口径）。

被丢的 1,019 行按档分布（`./tools/truncation_inventory.sh` 直读，W1002 的 `<file>:<line>` 是
**截断点**不是文件末尾；最后一列只抄清点器给的那一行的原文，**除两档外未逐档定位崩点**——
崩点归因是下一批的活，这里不装成已做过）：

| 丢行数 | 档 | 截断点 | 该行原文（inventory 输出） |
|---|---|---|---|
| 357 | `benchmark_simd_vs_scalar.z` | `:9` | `fn get_time() -> u64 {` —— 本批唯一做过逐行二分的档，崩点是体内 `static mut counter` |
| 230 | `minimal_compiler.z` | `:572` | `fn main() -> i64 {`（该档双态皆红 ⇒ 恢复收益不体现在 rc 上） |
| 158 | `selfhost.z` | `:22` | `impl Parser for ZetaParser {` |
| 85 | `quantum_basic.z` | `:73` | `fn test_shors_algorithm() -> i32 {` |
| 62 | `advanced_patterns_test.z` | `:40` | `fn test_multiple_patterns() {` —— 崩点已在批次 326 定为 or 模式里的构造子模式（任务 #43） |
| 58 | `integration_all_features.z` | `:46` | `fn distributed_test() {` |
| 36 | `primezeta_usize_test.z` | `:29` | `fn test_for_loop() -> i64 {` |
| 19 | `integration_test_program.z` | `:5` | `;`（上一行以裸分号结尾） |
| 14 | `test_const_expression.z` | `:4` | `fn test_array() -> usize {` |

**判读**：恢复把 1,019 行全部收了回来，然后把它们引用的符号变成"引用了但没定义"——6 档的失败
形态全是链接期 `Undefined symbols`（不是编译错误）。**取证卫生**：首版我用 `grep '^error'`
数差异，报出"0 差异"，因为这 6 档的报错行首字母是大写 `E`（`Error: "Linking failed"`）⇒
判据错了会得出反向结论；改直读退出码后才看见 6 档回归。另：`truncation_inventory.sh` 只过滤
W1002，所以"开态 0 处 W1002"不等于"开态没问题"，它只是 W1003 换了个名字。

### 结论（G.7b-2）：默认打开的前置条件是一件事，不是六个补丁

被跳过的项目现在**在 AST 里不存在** ⇒ 下游引用它的文件必然链接失败。要"默认开"，先让跳过
不改变链接结果：**给被跳过的顶层项产出占位/签名声明**（只声明、不定义，或定义成 `--report-stubs`
能看见的假值桩）。这条前置满足前，默认保持关，`ZETA_PARSE_RECOVER` 作为诊断用旋钮。
本次不动代码去凑这个数——那会把"恢复"变成"造出六个更假的符号"。

### 一个反了的旋钮 = 27 个：`env_flag` 闭合布尔值域

顺带从 `ZETA_PARSE_RECOVER` 查它的读法，读到的是**全仓一致的语义倒置**：
27 处旋钮读点全是 `std::env::var("ZETA_X").is_ok()` = **存在即开** ⇒ 写 `ZETA_X=0` 的人
得到的是"开"。三条实测反例（批次改前）：

| 旋钮 | 写法 | 改前实际 | 改后 |
|---|---|---|---|
| `ZETA_PARSE_RECOVER` | `=0` | 打出 W1003（恢复被启用） | 关（W1002） |
| `ZETA_STRICT_PARSE` | `=0` | E1002 致命 rc=1 | rc=0 |
| `ZETA_STRICT_ABI` | `=0` | ABI 强转升级为失败 rc=1 | rc=0 |

修法是一个实现点 + 27 处原地替换（**行号不变**，`git diff --numstat` 六档全是 N/N 对称）：

| 文件 | 站点 |
|---|---|
| `src/middle/mir/gen.rs` | 9（`:321 :5114 :5747 :6001 :6785 :8576 :13410 :13519 :13552`） |
| `src/backend/codegen/codegen.rs` | 5（`:1333 :1436 :6484 :6528 :6535`） |
| `src/middle/resolver/resolver.rs` | 5（`:184 :1979 :2006 :2031 :2101`） |
| `src/main.rs` | 4（`:424 :463 :509 :530`） |
| `src/backend/codegen/jit.rs` | 2（`:78 :304`） |
| `src/frontend/parser/top_level.rs` | 2（`:1930 :2011`） |

实现点 `src/diagnostics.rs:515`：假值拼写 = `"" / "0" / "false" / "no" / "off"`（先 `trim`
再小写），其余非空值为开，未设置为关。**两处故意不改**（A4 点名，防止"全仓已收敛"的误读）：
`src/diagnostics.rs:225` 的 `NO_COLOR`（外部约定就是"存在即生效"）、`src/std/env/mod.rs:104`
的 `env::var(name).is_ok()`（那是**被编译语言**的"变量是否存在" API，不是编译器旋钮）。
字符串旋钮（`ZETA_PACKAGES_DIR`/`ZETA_PYLIB`/`ZETA_RUNTIME_DIR`/`ZETA_EXTRA_LDFLAGS`/
`ZETA_DUMP_PP`）与 CTFE 里的 `getenv`（`gen.rs:43`）不在值域问题里，未动。

### 工具项：把"我测过一次"变成"门禁每周测"

| 点 | 位置 | 内容 |
|---|---|---|
| 新工具 | `tools/knob_probe.sh` | **A 段 23 条断言**：假值全拼写（`0/off/OFF/" false "/no/NO/空/两空格`）+ 未设置 ⇒ 必须关，真值（`1/true/on/2`）⇒ 必须开；`ZETA_STRICT_PARSE`、`ZETA_STRICT_ABI` 各测 `=0/=1/未设置` 三档 rc；A3 夹具若造不出 coerce 告警就**显式 skip 并说明"别当已覆盖"**，不许静默通过。B 段 = 上面那张 194 档对照表，只读数不判定 |
| 门禁第 6 步 | `tools/run_all.sh:214-235`（`--skip-knob`、JSON `knob` 键、判据 `:289`） | 只跑 A 段（秒级）。放门禁的理由：断言测的是**值域**，抽样测不出"下一个旋钮又用 `is_ok`" |
| 负向自检 | `ZETAC=/bin/true ./tools/run_all.sh --skip-其余五步` | ⇒ `knob: 13 条断言，FAIL 10（rc=1）`，整门禁 rc=1 ⇒ 这一步**抓得住**，不是恒真判据 |
| 契约修正 | `tools/opt_matrix.sh:9` | 原文写"唯一的运行期开关是 `ZETA_NO_OPT`（存在性检查，**值无所谓**）"——"值无所谓"正是本批修掉的缺陷 ⇒ 改为"假值拼写视为关" |
| 文档 | `docs/ABI.md §6.6` | 表首加一行"Rust 侧全部布尔旋钮 ⇒ `diagnostics.rs:515`"；表后记下**收敛半径的边界**（见 OPEN 第 1 条） |

**自伤记录（锚点核对器当场抓到）**：往 `run_all.sh` 插 31 行 ⇒ `docs/ABI.md` 的 3 个
`run_all.sh` 锚点全部漂移，`./tools/check_abi_anchors.py` 逐条报出行号与新旧内容：
`:83→:85`、`:178→:180`、`:252→:279`。改文档重绑后再 `--bless`。这条正是这个工具存在的理由——
**在别的批次里，这类漂移是静默的。**

### 门禁读数

`./tools/run_all.sh > /tmp/b336/gate_final.log 2>&1; echo "run_all rc=$?"`（**直读退出码，不经
管道**；ts `2026-09-22T11:46:47Z`）⇒ **rc=1**，唯一原因仍是既有判据 `tools/run_all.sh:280`
（`py_fail != 0`）：那 2 条还是批次 326 以来的 `t231_dict_set_cast_fromkeys` /
`t233_listcomp_condition_capture`。

| 步 | 读数 | vs 批次 335 |
|---|---|---|
| official | compile **194/194**，compile+link 191/194（3 条 link-only：缺运行时绑定） | 相同 |
| python_style | 290 passed / **2 failed** / 4 known-fail / 0 xpass | 相同 |
| corpus | 39/39 | 相同 |
| jit sweep | ok=169 trap=321 fail=0 timeout=0 segv=0（total 490，门槛 ok≥163） | 相同 |
| diff | match=120 judged=130 rate=**92.3%** bad_case=0 | 相同 |
| **knob（本批新增）** | **23 条断言 / FAIL 0** | 无上一格 |
| 诊断 | official 10 文件 / 15 行；python_style 81 文件 / 190 行 | 相同 |

**与批次 335 的"全同"不同义**：335 只动文档，全同是预期的负向自检；本批**动了二进制**
（`cargo build --release` 于 19:28:53，晚于所有 `src/*.rs` 的 mtime ⇒ 门禁跑的就是本批产物），
所以这份"五步全同"是**行为中立的证据**：27 处改读法没有改变任何一档在默认旋钮下的编译/运行/
差分结果 —— 与"值域只在写 `=0` 时才可观测"这一判断一致。
`./tools/check_abi_anchors.py` ⇒ **rc=0**，`漂移 0 / 新 0 / 消失 0 / 定位失败 0`；
基线 `tools/baselines/abi_anchors.tsv` = 324 → **327 行**（锚点 240 → 243，84 种待归属不变）。
`--numstat` 的 **7 增 4 删**逐项对得上：7 = §6.6 三个新锚（`diagnostics.rs:515`、
`py_additions.c:2937`、`unavailable_stubs.c:83`）+ `codegen.rs:1333` 改内容 + `run_all.sh`
三个移位重绑（`:85`/`:180`/`:279`）；4 = `codegen.rs:1333` 旧内容 + `run_all.sh` 三个旧行号。

### OPEN（本批欠的、和量出来的）

1. **C 侧 4 个 `getenv` 站点没收敛**：`runtime/py_additions.c:2937`（`ZETA_PROBE`）、
   `:3307` 与 `runtime/unavailable_stubs.c:83`（`ZETA_LENIENT_STUBS`）、`py_additions.c:3314`
   （假旋钮）仍是"非空即开"⇒ 同一个 `ZETA_LENIENT_STUBS=0` 在 Rust 侧读作关、在 C 侧读作开。
   修法是 runtime 侧一个与 `env_flag` 同表意的 `zt_env_flag()`，不是写文档提醒。
   本批不动的**具体**理由：`py_additions.c` 正被并发工作流改（`git status` 显示 `M`，按约只引用不改），
   而单独改 `unavailable_stubs.c` 会让 `zeta_runtime_c.o` 变陈旧 ⇒ 门禁 `[W2003]` 喊话、
   整批读数换成旧运行期。已写进 `docs/ABI.md §6.6`。
2. **`static` 不是关键字 ≠ "缺 static 支持"，是"未知修饰符被静默吃掉"这一族**：
   `static counter: i64 = 0` 不报错、不进 AST、还绑定了 `counter`。要按值域闭合，需要
   一条"声明形态里出现非白名单修饰词 ⇒ 出声"的判据（候选实现点：`parse_let` /
   `parse_assign` 之前），而不是给 `static` 加一个臂。#36 的姊妹项。
3. **G.7b-2**：被跳过项的占位/签名声明（本批的落地结论，见上）。做完它，"恢复默认开"才
   从"6 档回归"变成可评估项，#36 那 1,019 行也才有出口。
4. **`[W1003]` 不带文件名**：`top_level.rs:2006-2009` 打的是 `warning: [W1003] :{line}: …`
   （冒号前该是路径的位置是空的）⇒ 多文件程序里跳过了哪一档无从定位；对照 W1002 是
   `main.rs` 的 `ensure_fully_parsed` 打的，那里手上有文件名。缺的是**管道**不是信息：
   行号已经走 `indent.rs:33` 的 `LAST_PP` 线程局部（`set_last_preprocess` 在 `:38`）传到解析器，
   把当前文件路径并进去（换元组第三元，或平行一个 `LAST_PP_PATH`）即可，
   写入点与 `set_last_preprocess` 同一处。
5. **"实现点收敛"不等于"判据收敛"**：27 个站点走同一个 `env_flag`，所以假值语义由实现点统一保证；
   但 A 段只对**外显行为可测的 3 个旋钮**（`PARSE_RECOVER`/`STRICT_PARSE`/`STRICT_ABI`）逐值域断言，
   其余 24 站点（`ZETA_PROBE` 6 处、`ZETA_DBG_FA` 3 处、`ZETA_NO_OPT` 2 处…）没有逐个的对外断言。
   下一个"对外可测"的旋钮出现时应补进 A 段，别把这次的 23 条读成"27 个都测了"。
6. 批次 335 的 OPEN 全部继承（§5+ 的 93 处待归属、隐形引用对账判据 `:\d{3,4}`、锚点核对接 CI=#37、
   `/tmp` 下三个一次性脚本未进 `tools/`）；更早的继承项（深度 0 平铺整除 `t406`、批次 333 那五项、
   #56 故意留红、批次 331 两档未实测）不变。

### 下一批默认候选

- **G.7b-2 的占位声明**（OPEN 3）：修掉 6 档回归 ⇒ 恢复可默认开 ⇒ 直接接 #36 的 1,019 行，
  是本批读数唯一指向的那一步。
- 或 **§5 类型布局的 38 处待归属**（批次 335 队列，纯文档、可与代码批次并行）。
- 或 **OPEN 2 的"未知修饰符出声"**：它同时是 `static`、`thread_local`、`extern` 一类方言词的入口。


---

## 批次 337（把"前导词被静默吞掉"从推断变成读数，再给它一条会响的判据 W1004）

### 选题过程：一批的结论里有一条是错的，纠错顺路把机制挖到了底

批次 336 的 OPEN 2 说"未知修饰符被静默吃掉"。开工前先按纪律跑了一条最小探针，结果
**336 写下的机制描述有两处不成立**（已在原文下方以引用块更正，原文保留）：

| 336 的说法 | 337 实测 | 证据 |
| --- | --- | --- |
| `static counter: i64 = 0` "不报错也**不进 AST**" | 它**进 AST**，只是降级成一次普通赋值 | `--dump-mir`：`Assign{lhs: Var(counter), rhs: IntLit(0)}` ＋ `Return{val: …}`（体内声明变成写槽） |
| `static mut …` 因"名字前有两个标识符"而整条失败 | 前导的**任意多个**裸词都能被吞掉；中止整条的是**第二个词不能开表达式** | `apple banana cherry target: i64 = 7` 正常解析并把 7 赋给 `target`；而 `static mut counter: i64 = 0` 的 W1002 报的是 `First unparsed text: 'mut counter: i64 = 0'` —— 崩点是 `mut`，`static` 早在崩之前就被吃了 |

⇒ 这条更正本身就是本批的选题：**"崩点"和"丢词"是两个缺陷**，批次 336 只看得见前者。

### 机制：吞词发生在两处兜底，且形状与"合法的函数尾隐式返回"完全相同

`static` / `apple` / `import` 都不是关键字，于是它们走进同一对兜底：

- `src/frontend/parser/stmt.rs:679` `parse_expr_stmt` —— `parse_stmt`（`:1626`）21 臂 `alt`
  的最后一臂，`parse_full_expr` 能吃下一个裸标识符就算一条语句；
- `src/frontend/parser/stmt.rs:58` `parse_block_body` —— `parse_stmt` 整条失败后再试一次表达式。

两条都**不留任何痕迹**：被吞的词成一条无副作用的 `ExprStmt`，同一行剩下的文本再由下一轮循环
当成**另一条语句**解析。实测（`zetac` 直接跑，取程序 stdout）：

| 输入（函数体内一行） | 编译诊断 | 程序输出 | 读法 |
| --- | --- | --- | --- |
| `apple banana cherry target: i64 = 7` | 无 | `7` | 三个词消失，赋值照做 |
| 同上，但 `return apple` | 无 | `100` | `apple` 从未被赋值 ⇒ 吞掉的确实是前导词 |
| `static mut counter: i64 = 0` | W1002（整个 `fn` 连同文件余部丢弃） | — | `mut` 开不了表达式 ⇒ `parse_block_body` 失败 ⇒ 顶层 `many0` 停下 |

### 判据选型：第一版判据被自己的读数否证

先按"表达式语句无副作用 ⇒ 出声"做了一版探针（`AstNode::Var`/`Lit`），跑 237 档（official 198 + 语料 39）：
**40 命中 / 24 文件**（official 19 档 + 语料 5 档；21 个裸名 + 19 个裸字面值）。逐条对回源码后：

- **36 条是合法惯用法** —— 函数（或 `if`/`else` 分支）尾部的隐式返回：`final_demo.z:22 value`、
  `quantum_basic.z:31 0`、`test_if_else_fixed.z:12 result  // Implicit return`（用例注释自己写明是隐式返回）。
- 只有 2 族是真吞词（**4 处命中**）：`benchmark_simd_vs_scalar.z:11` 的 `static`、以及
  `integration_test_program.z:3-4` / `memory_model_test.z:4` 的 `import std::…`。

⇒ "无副作用"单独**不是**判据，用它等于给 24 个文件里 36 处正确代码刷告警。
改成两翼：**无副作用 ＋ 该表达式之后仍有文本，且那些文本不是缩进预处理器补的定界符**
（`}`、`} else {` 会被拼在尾表达式同一行 —— 这也是那 36 条隐式返回被排除的原因；加上这一翼后
的实测排除量正好是 **30 条后接 `}` ＋ 6 条后接 `} else {`**，与"排除 36 留 4"逐条对上）。

### 读数：判据的假阳性面（探针两档跑 237 档，定稿判据跑 533 档）

前两张表是**探针阶段**（`ZETA_PROBE_JUNK`，跑完即删）在 237 档上的读数；第三张是接进门禁的
W1004 在 533 档上的读数（`junk_swallow_inventory.sh --full`，可复跑）。

| 判据 | 范围 | 命中 | 分类 |
| --- | --- | --- | --- |
| ① 只看"无副作用" | 237 档 | **40 / 24 文件** | 21 裸名 + 19 裸字面值 |
| ② ①＋"后接文本非预处理器定界符" | 同上 40 条 | **4 真信号** | 排除 30 条（后接 `}`）＋ 6 条（后接 `} else {`） |
| W1004（=②，已接线） | **533** 文件 | **4 处 / 3 文件** | official 4；python_style 296 档 **0**；语料 39 档 **0** |

4 处逐条：`benchmark_simd_vs_scalar.z:11`（`static`）、`integration_test_program.z:3`、`:4`、
`memory_model_test.z:4`（三条都是 `import std::X;` 丢掉的那一段）。⇒ 判据可以默认打开，不需要旋钮。

⚠️ 探针阶段有一档读数**不能要**，已剔除：①在语料 5 个 `.py` 上报了 5 条 `yield` 命中、行号全是
`:729`，而这 5 档实际行数只有 70/379/166/128/157 ⇒ **行号是假的**（`LAST_PP` 的行映射对不上当前
文件，见 OPEN 4）。命中形状本身成立（都被 ② 的 `}` 排除），但**按文件/按行归属不可信**，
所以语料那一档在上表里只记 W1004 的 0，不记探针的 5。

⚠️ 顺带撞出一个**独立的语义缺陷**（不是判据问题，登记为任务 #64）：
`import std::memory;` 走的是 python-import 那条路，它吃到 `std` 就停 ⇒
实际效果是 **导入了 `std`**（日志里真有一句 `PY-A: imported module \`std\` from build/stubs/std.z`）
**并把 `memory` 当裸名语句丢掉**。文件照样编译成功。本批让它出声，语义要等 `import a::b;` 的正解。

### 实现

| 落点 | 内容 |
| --- | --- |
| `src/frontend/parser/stmt.rs:700` | `warn_if_swallowed_prefix(expr, stmt_start, rest)` —— 单点判据，剥掉 `{}()`,、`else` 后仍有余文才出声 |
| `stmt.rs:58`、`stmt.rs:683` | 两处兜底各接一行（第二处的 `rest` 取 `;` 之后，否则每条 `;` 结尾的语句都会误报） |
| 诊断文案 | `warning: [W1004] :{line}: \`static\` became a stand-alone statement while 'mut counter: u64 = 0' was parsed as the next one — a word on this line is being ignored` |
| `src/error_codes.rs:2167-2169` | 码表对齐：`W1002` 从 `UNNECESSARY_PARENTHESES`（**全仓零引用**，且这个码实际早被截断诊断占用）改为 `PARSE_TRUNCATED_INPUT`，新增 `PARSE_RECOVERY_SKIP`=W1003、`PARSE_SWALLOWED_WORD`=W1004 |

行号只有 `:{line}:`、没有文件名 —— 和 W1003 同一个已知缺口（`original_line_at` 靠
`indent.rs:34` 的 `LAST_PP` 线程局部，那里只有文本没有路径）。合并进已有的"诊断缺文件名"
OPEN，不在本批半解：`parse_zeta` 有 5 个调用方（`lib.rs:95`、`lib.rs:832`、
`resolver.rs:2379`、`module_resolver.rs:639`、`bin/pipeline_dump.rs:10`），
只给其中一部分塞路径会让跨模块警告**带着错误的文件名**。

### 工具与门禁：判据的两翼都要有回归网

`tools/junk_swallow_inventory.sh`（新，86 行）—— 4 条夹具断言：

| 断言 | 期望 | 为什么必须测这一翼 |
| --- | --- | --- |
| `apple banana target: i64 = 7` | W1004 ×**2** | 每个被吞的词各响一次（只测一个词，就把"逐词响"过判成"一行响一次"） |
| 尾表达式是裸名 | 0 | 判据翻成"永远不响"时抓不住 |
| 尾表达式在 `} else {` 那型里 | 0 | 排除项（定界符剥离）被人删掉时，那 36 处隐式返回会一起变噪声 |
| 同一夹具**运行** | 输出 `7` | 反证：吞词是真发生了，且语句仍按"剩下的部分"执行 —— 不是只在诊断里造形状 |

`--full` 才走 533 档全语料计数（只报读数，不参与判定，和 `knob_probe.sh` 的 B 段同口径）。
门禁 `tools/run_all.sh` 加**步骤 7**（`:239-260`，判据 `:318`，`--skip-swallow` 在 `:30`），
JSON 多一个 `swallow {checked, failed, skipped}`。

### 门禁读数（批次 336 → 337）

| 步骤 | 336 | 337 | 说明 |
| --- | --- | --- | --- |
| official 编译 | 194/194 | **194/194** | |
| official 编译+链接 | 191/194 | **191/194** | 缺绑定的 3 档不变（#42） |
| python_style | 290 过 / 2 判红 / 4 已知失败 / 0 xpass | **同左** | 判据 `py_fail != 0 ⇒ rc=1`（`:307`）⇒ 门禁 rc=1 是**预期读数** |
| 语料 | 39/39 | **39/39** | |
| jit sweep | ok=169 trap=321 fail=0 timeout=0 segv=0（总 490） | **同左** | |
| diff | match=120 judged=130 92.3% 坏用例 0 | **同左** | |
| knob | 23 断言 / FAIL 0 | **23 / 0** | |
| swallow | —（本批新增） | **4 断言 / FAIL 0** | |
| 诊断 official | 10 文件 / 15 行 | **10 文件 / 19 行** | +4 行＝W1004 那 4 处；文件数不变，因为 3 个受害文件本来就有 W1002/PY-A 行 |
| 诊断 python_style | 81 文件 / 190 行 | **81 / 190** | 与"该套 0 真信号"一致 |
| 锚点核对 | rc=0，基线 327 | rc=0，基线 **327**（条数不变） | 本批漂移见下 |

与批次 336 的"全同"不同义：本批**动了二进制**（新增一条诊断），所以这份全同样是
"行为中立"的证据 —— 唯一预期中的变化就是 official 诊断行数 15→19，且能逐条对上。

### 自伤记录：又是自己改的行把锚点撞漂移了

`tools/run_all.sh` 插入步骤 7 让 ABI.md 的 3 条引用失配（`:85→:87`、`:180→:182`、`:279→:306`）。
批次 336 刚记过同一形状，这次的教训只有一条有效：**改 `run_all.sh` 之前先算它下游有几条锚点**。
先重锚 ABI.md、复跑到"漂移 0 / 新 3 / 消失 3"，代码定稿后再 `--bless`，最后 `rc=0`。

### OPEN（本批新增／推进）

1. **`import a::b;` 的语义正解**（任务 #64）：现在它导入 `a` 并丢掉 `b`，本批只是让它出声。
   正解要么 `::` 段按 `use` 的语义绑定末段，要么当场判错。
2. **W1004 是否进 `ZETA_STRICT_PARSE`**：本批**故意不进**。它指的是一个编译器语法缺口
   （`static`/`import a::b` 这类方言词没被任何规则认领），在补齐认领之前把它做成致命错误，
   等于因为编译器的债判用户的刑。要接，得先有 #64 那类正解。
3. **码表与发出点没有编译期绑定**（本批撞出来的架构问题）：`error_codes.rs` 的 `W1xxx` 常量
   全仓零引用，实际发出的码都是字面串 —— 所以 W1002 早就和 `UNNECESSARY_PARENTHESES` 撞车而无人发现。
   要么让发出点引用常量，要么把码表降级为纯文档，别留着当"看起来是真的"的假登记处。
4. **`LAST_PP` 的行映射会带着上一个文件的行号**（本批读数撞出来的取证缺陷，任务 #65）：
   探针在 5 个语料 `.py` 上报的行号全是 `:729`，而这 5 档实际只有 70/379/166/128/157 行
   （`wufu_v1.py` 128 行却报 729）。**同一套代码在 official 上是对的**：对照探针 `lm1.z` 把吞词放在
   `:2`、后面还有 8 行代码，W1004 报的正是 `:2` ⇒ 缺陷特定于语料那条路径（py 模式/模块解析），
   不是行映射通用坏掉。嫌疑形状两处（**尚未定到哪一条分支**）：
   `indent.rs:62-78` 的 `remaining_byte_offset` 假设 `remaining` 是 pp 缓冲的**后缀**，
   不匹配时退到 `saturating_sub`；`indent.rs:48-57` 的 `original_line_at` 把偏移夹进缓冲后
   直接查 `origins` —— 那里没有"这份行表属于哪个文件"的记录（线程局部，只存文本＋行表）。
   后果：任何**按行**的语料级读数不可信，本批因此只保留了按文件计数、且把语料那一档剔除。
5. 承接批次 336 未动的：#60（C 侧 `zt_env_flag`）、#61（占位/签名声明）、#63（`--repl` 的
   `_dump_mir` 收下即弃）、#52 尾巴（93 条待归属 / 裸行号引用）、W1003+W1004 共用的缺文件名。

### 下一批默认候选

- **#61 G.7b-2 的占位/签名声明**：唯一能把 6 档回归清零、让解析恢复默认打开的一步。
- 或 **#64 `import a::b;`**：本批留下的最直接的一条真语义 bug，且已有出声的诊断当验收线。
- 或 **§5 类型布局 93 条待归属**（纯文档，可与代码批次并行）。

## 批次 338（#64 落地：`import` 的 `::` 翼并入 `use` 的文法，顺带量出"结尾分号"这个家族还剩几个成员）

### 选题：批次 337 的 OPEN 1 说"导入 a 丢掉 b"，实测发现那是**两个缺陷叠在一行**

337 只给了出声的诊断（W1004），没给语义。开工先按纪律跑值域探针（`/tmp/b338/`，
修前二进制 = `git show HEAD:` 的两个 parser 文件重建，存为 `/tmp/b338/zetac_before`；
修后 = `zetac_after`，与最终重建产物 `cmp` 逐字节相同），结果**"绑错名字"只是较小的一半**：

| 形状（修前） | 实测 | 证据 |
| --- | --- | --- |
| `import std::memory;` | 绑到 `std`（MIR 里出现 `Call std__init` 2 处），`memory` 落成一条裸名语句 | `/tmp/b338/err_t3.txt`、W1004 逐词计数 |
| `import std;`（带分号） | **文件被截断**：MIR 166 行，去掉分号是 195 行 | `/tmp/b338/mir_t1.txt` vs `mir_n_semi.txt` |
| `import a::b::{c, d};` | 从 `::` 起整条尾巴未解析 ⇒ W1002 丢 6 行 | `err_ref_grp.txt`：`First unparsed text: '::{capability, dyn};'` |
| `import a::b as c;` | 同上形状 ⇒ W1002 丢 6 行 | `err_ref_as.txt`：`First unparsed text: 'as m;'` |
| 真实用例 `integration_test_program.z` | 3–6 行的 4 条 `import` 只认领了前 2 行，**文件余部 19 行全丢**（含整个 `fn main`），而门禁仍把它算作"编译通过" | 修前该档 W1002 丢 19 行 |

⇒ 一条"看起来只是名字绑错"的 OPEN，底下压着**吃掉整个文件**的机制。`parse_python_import`
的循环解析完目标后**不吃结尾的 `;`**，`many0` 拿到的下一条顶层项就是一个裸 `;`，没有任何规则
认它 ⇒ 循环停下 ⇒ 文件余部静默丢弃。分号丢文本、`::` 丢尾巴，是同一个函数里的两处，本批一起收。

### 机制与收敛点：不写第二套 `::` 文法

`::` 路径在 zeta 里**只有一种既有语义**，就是 `use` 那套（`parse_use_statement` →
群形 `::{a, b}` → `AstNode::Use{path}`，末段进作用域）。所以本批的写法是把 `use` 的尾巴
原样搬成一个共享函数，而不是在 `import` 里复刻：

- `src/frontend/parser/top_level.rs:141` 新增 `pub(crate) fn parse_use_targets(path, input)`
  —— 群形 `::{…}` ＋ 可选 `;` ＋ `Use` 节点构造，全部从 `parse_use_statement`（`:132`）
  逐字搬出；`:132` 瘦成"关键字 → `parse_path` → 交尾巴"。
- `src/frontend/parser/stmt.rs:913`（`parse_python_import` 的目标循环内）：先试 `parse_path`，
  只有**段数 > 1** 才走 `parse_use_targets`。判据写成段数而非"看到 `::`"，是因为单段
  `import std` 必须继续走 python 那条腿（模块加载），不能被 `use` 抢走。
- 同处的**提交判据**（`stmt.rs:927-931`）：`use` 的尾巴解析成功**且**剩下的文本不是
  `as`/`=`/`;` 开头时才提交，否则退回 python 腿。这一条是本批初版欠的账，见下节"尾巴翼"。
- `src/frontend/parser/stmt.rs:992`：循环出口吃掉本条语句的 `;`。

### 值域：`import` 的三种翼各有各的语义，验收线按翼分开设

| 翼 | 形状 | 判据 | 修后实测 |
| --- | --- | --- | --- |
| 等值翼 | `import a::b;` / `a::b::c` / `::{a, b}`（带与不带 `;` 两版） | 与 `use` **逐字节同一份 MIR**（`--dump-mir` 稳定），且不留任何诊断 | 4 条 `same` 全过；`import std::memory;` 诊断为空；反证 `std__init` 计数 `import std;`=2 / `import std::memory;`=0 |
| 中立翼 | 带/不带 `;` | 两种写法 MIR 必须相同，且都不截断 | 4 条 `same` 全过（含 `import numpy as np` 与 `…;`），`import std;` W1002 为空 |
| 尾巴翼 | `::` 之后跟 `use` 文法不认的东西 | **必须退回 python 腿，不许提交**（提交=吃掉文件余部） | 6 条"W1002 为空"全过 ＋ 1 条诚实读数（`as` 族退回后仍是静默错绑，见 OPEN 1） |
| python 翼 | `import a.b`、`import x as y`、逗号列表 | **不许被本批改动**：仍走 `zeta_py_*` 标记、仍只出一条 PY-A | `import os.path;` → `1PY-A`；`import std::memory, ml;` → `1PY-A`（既不截断也不吞词） |

**尾巴翼不是设计出来的，是本批自己踩出来的**：接上文法的初版把整条语句交给了
`parse_use_targets`，而 `use` 没有 `as` 规则（`AstNode::Use{path}` 装不下别名），
于是 `import std::memory as m;` 的尾巴成了没人认的顶层项 ⇒ `many0` 停下。逐形状量了
13 个尾巴（含空尾巴），判据 = W1002 有没有从 0 变 1：

| `::` 之后的尾巴 | 修前（只接文法、无退回判据） | 加退回判据后 | 读法 |
| --- | --- | --- | --- |
| （空）`import std::memory` | W1002=0 | W1002=0 | 等值翼生效，绑 `memory` |
| ` as m` | **W1002=0 → 1，MIR 171 行 → 39 行** | 0（171 行） | 退回线；`=`、`;` 同理由 |
| ` = 2` | **0 → 1（242 行 → 39 行）** | 0（242 行） | |
| ` (1)` / ` [0]` / ` { x }` / ` foo` / `bar` | 0 → 0 | 0 | 提交是**更好**的（修前 ` (1)` 那一族本来就 W1002=1） |
| ` ,` / ` , ml` | 0 → 0 | 0 | 逗号续列表 |
| ` if 1` | 1 → 1 | 1 | 两边都截断；修前"保住"的 167 行其实是**错绑的 std 模块体**，不是更多程序 |
| ` ::{capability} as c` | 1 → 1 | 1 | 群形加 `as` 仍无人认（同 OPEN 1） |

⇒ 一句纪律的实证：**"修好一条形状"和"没弄坏值域里其余形状"是两次测量**，第二次是本批
初版漏掉的那次。退回判据的白名单（`as`/`=`/`;`）逐条来自上表，不是猜的；也**没有**去做
"提交后试解析余部，能parse才提交"那种更通用的形式——`parse_top_level_item` 会因为
**后面某个顶层项自己语法不支持**而失败（它有个 fail-loud 臂，`top_level.rs:1565` 的
`DEFINITION_KEYWORDS` 及其上方注释），那种情况下不提交反而更糟。真正的通用判据要先能区分
"余部开不了项"与"余部自己坏掉"，登记为 OPEN 3。

一条**判据设计上的坑**也记在这里以免被重踩：`import numpy as np` 在仓库根目录报 2 条 PY-A、
在 `/tmp` 报 0 条 —— PY-A 的措辞取决于模块搜索根（`build/stubs`、`pylib`）相对 cwd 的位置。
中立翼因此锁 MIR 而不是锁诊断条数，这条约束写进了工具头部注释
（`tools/import_form_inventory.sh:1-30`）。

**语义验收的诚实边界**：本批能证到的是降级层 —— 绑谁、吃掉多少文本、发出哪些诊断。
"末段真的在作用域里可用"在这一层观察不到：这个方言里未知名字按名解析，
对照组（不 import 直接用 `memory::capability::new`）编译结果相同。所以等值翼只声称
"与 `use` 走同一条文法、同一份 AST 构造"，不声称运行期可见性已验证（承接 #22/#33）。

### 修前/修后总读数（同一台机、只差这两个文件，official 194 档逐个 `--dump-mir`）

| 读数 | 修前 | 修后 | 说明 |
| --- | --- | --- | --- |
| official 编译 | 194/194 | **194/194** | |
| official 编译+链接 | 191/194 | **191/194** | 缺绑定的 3 档不变（#42） |
| 含 W1002 档数 | 9 | **9** | 不变 —— 那 9 档各有各的崩点，本批只挪动了其中一个的位置 |
| W1002 报告的**被丢弃行数合计** | 1,019 | **1,016** | −3，全部来自 `integration_test_program.z`（19 → 16）：`import` 那 4 行已被完整认领，剩下的截断点从 `:5` 推到 `:8` 的 `fn main() {`（另案） |
| W1004 行 | 4 | **1** | −3：`integration_test_program.z` 的 2 处 `import std::…` 吞词、`memory_model_test.z` 的 1 处；剩 `benchmark_simd_vs_scalar.z` 的 `static`（批次 337 的 #61 一路） |
| official 诊断行合计 | 19 | **15** | 与上两行逐条对上（3×W1004 + 1 条 PY-A 随截断点消失） |
| python_style | 探针口径 194 行诊断 / 13 行丢弃 / 0 W1004 | **完全相同** | 该套无 `::` 形状 ⇒ 中立证据；门禁口径同样是 **81 档 / 190 行**（两个数只差在探针把逐文件的 `warning:` 与 `PY-A:` 并计） |
| 门禁其余步骤 | corpus 39/39、jit ok=169/490、diff 120/130 92.3% 坏 0、knob 23/0、swallow 4/0 | **同左** | 行为中立 |
| 门禁 import 步骤 | —（本批新增） | **22 断言 / FAIL 0**（JSON `import_form {checked:22, failed:0}`） | 四翼齐：等值 6 + 中立 6 + 尾巴 8 + python 1 + 真实用例 1 |

`integration_test_program.z` 那一档是本批最有价值的证据：修前它**在门禁里是"编译通过"的**，
却把 19 行连整个 `fn main` 一起丢了；修后仍丢 16 行，但截断点已经走过 `import`，
剩下的那句 `fn main() {` 才是真崩点。工具为此锁了一条**只许更好**的单调线
（`tools/import_form_inventory.sh` 末段：截断行号 ≥8 才放行，退回即 FAIL）。

### 顺手量出的家族读数：`import` 只是"不吃结尾分号"这一族的第三个成员

按"带 `;` 与不带 `;` 的 MIR 是否相同"扫了 14 个语句形状（`/tmp/b338/semi_probe.sh`，
判据 = 加了 `;` 是否新增 W1002）。中立：`let`、赋值、调用、`import`、`from … import`、
`global`、`nonlocal`、`assert`、`type`、`const`、`use`。**不中立 3 个**：

| 形状 | 修后实测 | 已有 `opt(tag(";"))` 的规则（对照） |
| --- | --- | --- |
| `pass;` | 无分号 0 / 有分号 **1** | —— |
| `del x;` | 无分号 0 / 有分号 **1** | `return`（`stmt.rs:660`）、`global`（`:1481`）、`nonlocal`（`:1514`） |
| `fn f() { … };` | 无分号 0 / 有分号 **1** | |

⇒ 本批在 `stmt.rs:992` 逐规则吃分号，**没有闭合这个值域**（同一纪律：补一个特例形状 ≠ 闭合值域）。
真正的收敛点不在再多写三个 `opt(tag(";"))`，而是让 `parse_top_level_item`
（`top_level.rs:1533`）认一条"空项 = 裸 `;`"：一处改动覆盖全部四个调用点（`:1449`、`:1466`、
`:1945`、`:1997`），并让 `import` 那处 `:974` 变成冗余而非必要。登记为任务 #67。

### 跟手更正：本批的插入把批次 337 写下的 4 条行号引用弄漂了

锚点核对器只扫 `docs/ABI.md`，**roadmap 内部的引用不在它管辖内**（这正是任务 #52 记的盲区）。
所以本批动 `run_all.sh` 之后，337 那节有 4 处自述失效，逐条更正如下（原文保留）：

| 337 的写法 | 现在实测 | 漂因 |
| --- | --- | --- |
| 步骤 7 块在 `:239-260` | `:241-262` | 本批在 `:20`（`SKIP_IMPORT=0`）与 `:32`（`--skip-import`）各插一行 ⇒ 其后的块号 +2 |
| `--skip-swallow` 在 `:30` | `:31`（`SKIP_SWALLOW=0` 在 `:19`） | 只吃到 `:20` 那一行 ⇒ +1（本批的 `--skip-import` 在它后面） |
| swallow 判据 `:318` | `:344` | +26 = 上面 2 行 ＋ 步骤 8 块 22 行 ＋ JSON 里 2 行，逐块对上 |
| 工具 `junk_swallow_inventory.sh`（新，86 行） | **89 行** | 本批改了它的头部注释（见下） |

同一处头部注释还有一条**事实更正**：337 举的三个吞词例子里，
`import std::memory;` "绑到 `std`，`memory` 作为裸名语句消失"这一条已被本批移除，
当前实测该档 **0 处 W1004**。注释已改成"曾在册、本批收走、回归看
`tools/import_form_inventory.sh`"，因为 W1004 的适用范围本身就写着"解析器没有规则的前导词"
—— 规则补齐一处，判据就该少一处，这条边界要跟着事实走。
夹具断言复跑：4 条全过（`2 / 0 / 0 / 输出 7`）。

### 锚点自伤（连续第二批，形状完全相同）

本批动了 `tools/run_all.sh`（步骤 8 块 `:264-284`、JSON `:306-307`、判据 `:345`、
`--skip-import` 在 `:32`、`SKIP_IMPORT=0` 在 `:20`）与 `top_level.rs`（插入 `parse_use_targets`），
`check_abi_anchors.py` 报**漂移 4**：`run_all.sh:87→:89`、`:182→:184`、`:306→:332`，
以及 `top_level.rs:334-355→:341-362`（ABI.md:254 那条**范围**引用也在账上，不只是 `run_all.sh`）。
重锚后 `漂移 0 / 新 4 / 消失 4`，文档定稿再 `--bless`。
批次 336、337、338 连续三次同一形状 ⇒ 上一批写的教训"改 `run_all.sh` 之前先算它下游有几条锚点"
仍然只是被重复抄写、没有被执行；本轮的实际变化是**把第 4 条锚点（范围引用）也纳入了同一次核对**，
下次改动前该做的是"先跑 `python3 tools/check_abi_anchors.py` 看谁会漂"，而不是改完再修。

### OPEN（本批新增／推进）

1. **`import a::b as c;` 完全没修**（本批实测，且是从"会截断"退回"静默错绑"换来的）：
   `use` 文法没有 `as` 规则 ⇒ 提交会吃掉文件余部（下表"尾巴翼"），退回则保住整个文件但
   仍绑成 `a`。退回后的实测读数是 `1PY-A`、**W1004 也不响**（337 的判据管前导词，
   这种"尾巴被丢掉"的形状不在它的覆盖里）。正解要给 `AstNode::Use`（`ast.rs:204`，
   目前只有 `path: Vec<String>`）加别名字段并接到 resolver，不是改判据。
   工具里那条"诚实读数"断言就是钉住这个未修状态的。而 `use` 那一侧**没人退回**：
   `use std::memory as m;` 实测 W1002、丢 2 行、MIR 只剩 39 行 —— 同一条语法缺口，
   两种拼法现在一个保文件一个吃文件。正解（给 `AstNode::Use` 加别名字段）能一次收掉两笔。
2. **`::` 与 `.` 两种拼写的模块加载语义现在真的分叉了**（本批造成的新边界，已量）：
   `import std.memory;` 走 python 腿 ⇒ 4 处 `__init` 调用 + `PY-A: imported module …`；
   `import std::memory;` 走 `use` 腿 ⇒ **0 处 init、0 条诊断**，与 `use std::memory;` 逐字节相同。
   拼写即语义是可辩护的，但"import 却不加载模块"是不是用户期望，需要一次方言裁决（与 #48 同族）。
3. **尾巴翼的退回判据是白名单，不是通用形式**（本批实测过通用写法的代价）：
   `kw_boundary("as") || '=' || ';'` 三个成员各对应一条量过的形状。通用判据应是
   "提交后余部仍能被顶层循环认领"，但 `parse_top_level_item` 会因为**后面某个项自己语法不支持**
   而失败（`top_level.rs:1565` 的 fail-loud 臂），那时不提交反而更糟 —— 要先能区分
   "余部开不了项"和"余部自己坏掉"。这条区分没做。
4. **`;` 家族的三个残留成员**（`pass;` / `del x;` / `fn …;`，任务 #67）：见上表，收敛点已指名。
5. **`error_codes.rs` 的码表常量仍零引用**（承接批次 337 OPEN 3）：本批新加的 PY-A/W1002 行为
   照旧是字面串，编译期绑定问题未动。
6. 承接未动的：#65（`LAST_PP` 行映射串号）、#61（占位/签名声明）、#60（`zt_env_flag`）、
   #63（`--repl` 的 `_dump_mir` 收下即弃）、#52 尾巴（93 条待归属 / 裸行号）。

### 下一批默认候选

- **#67 裸 `;` 作为顶层空项**：一处改动收掉三个实测成员，并让本批 `stmt.rs:992` 的逐规则补丁变成冗余。
- **#61 占位/签名声明**：让解析恢复可默认打开（`benchmark_simd_vs_scalar.z` 那唯一一条 W1004 在此路上）。
- 或 **OPEN 1（`import a::b as c;`）**：要先给 `AstNode::Use` 加别名字段，半径比看起来大，
  但它同时收掉 `use a::b as c;`（同一份文法，此前一直没人管）。

---

## 批次 339（#67 落地：裸 `;` 收进顶层文法当空项，顺带删掉批次 338 为它打的逐规则补丁）

### 选题：批次 338 的 OPEN 4 数出"; 家族还剩三个成员"，实测家族是 13 个位置里的 12 个

338 为了让 `import distributed;` 不吃掉文件余部，在 `parse_python_import` 末尾加了一条
`opt(ws(tag(";")))`（`stmt.rs:974`）。那是**逐规则补丁**：同一字符的生死取决于"哪条规则
恰好先跑到"。开工前先把值域量全（`tools/empty_stmt_inventory.sh` 的 A 翼 13 个位置，
修前基线 = 批次 338 的二进制）：

| 顶层位置（`;` 单独占一行/一列） | 修前 | 修后 |
|---|---|---|
| 首项单个 `;` / `;;` / `;;;` | **截断**（文件余部全丢，W1002） | 中立 |
| 带缩进的顶层 `;` | **截断** | 中立 |
| 注释之后、表达式语句之后 | **截断** | 中立 |
| `fn` 定义之后、`def` 定义之后 | **截断** | 中立 |
| `use` 之后、`import` 之后 | **截断** | 中立 |
| `mod { … }` 体内 | **截断**（连 `fn g` 一起丢） | 中立 |
| 文件末尾 | **仍报 W1002** | 不出声 |
| 赋值语句之后 | 中立（赋值自己吃掉 `;`） | 中立（不变） |
| 块内 / `if` 体 / `while` 体首条 | 中立（空语句早已支持） | 中立（不变） |

⇒ 13 个位置里 12 个会被一个字符删掉整个程序，而**块内**同一字符一直是合法空语句。
338 数的三个（`pass;`、`del x;`、`fn … { };`）都在这 12 个里，且修后全部关闭（逐条实测）。

### 改动：一个实现点、四个调用点、一处删除

| 文件 | 行 | 改 |
|---|---|---|
| `src/frontend/parser/top_level.rs` | `:1608` | 新增 `parse_top_level_entry` = `alt((use, 顶层项, value(vec![], tag(";"))))` —— 空项就是空项，不给它造 AST 节点 |
| 同上 | `:1447`、`:1461` | `mod { … }` 两处体内循环改用它（此前是 `alt((use, item))` 的手抄两份） |
| 同上 | `:1963` | 文件主循环 `many0(ws(…))` 改用它（此处 −4 行） |
| 同上 | `:1999` | **恢复路径**（`ZETA_PARSE_RECOVER=1`）也改用它：此前它把"先试 `use`、再试顶层项"**手写了两遍**（两个 `match` 臂），现在合成一个臂（删掉重复的 14 行，`AstNode::Skip` 的过滤留在调用点，行为不变）。修前同一个 `;` 是"默认路径截断 / 恢复路径出声"，两路各说各话 |
| `src/frontend/parser/stmt.rs` | `:987` | **删掉** 338 那条 `opt(ws(tag(";")))`，留注释指名规则的唯一落点。同一条规则不留两个落点 |
| `tools/empty_stmt_inventory.sh` | 新建 137 行 | 四翼锁（中立/截断/两路/负控制），68 条断言 |
| `tools/run_all.sh` | `:288`、`:373` | 第 9 步接进门禁（判据在脚本内部，门禁只认退出码）+ `--skip-empty` |
| `tests/python_style/t407_empty_stmt.z` | 新建 | 语义层用例（MIR 层由脚本锁，这里锁"跑起来到底打什么"） |

净账：`top_level.rs` +4 行（2,087→2,091 = 删 24 行重复的 `alt`/恢复段〔3+3+4+14〕、加 26 行新函数
（17 行 doc 注释 + 8 行实现 + 空行）与 2 行调用点注释），`stmt.rs` ±0 行（1,702 行不变）——**换到的是 12 个截断
位置归零 + 一条规则只剩一个落点**。

### 反证：把锁拿到修前二进制上跑，它必须咬

只把上面那两个 parser 文件退回 HEAD（= 批次 338 状态）重新 `cargo build --release -p zetac`，
得到 `/tmp/zetac_before`；语料其余部分两版完全相同 ⇒ 差异只可能是本批引入的。

| 读数 | 修前 | 修后 |
|---|---|---|
| `empty_stmt_inventory.sh` | **rc=1，29 FAIL / 39 ok**（A 翼 23、C 翼 5、D 翼 1，**B 翼 0、E 翼 0**） | **rc=0，0 FAIL / 68 ok** |
| `t407_empty_stmt.z` 运行期 | stdout **空**（W1002 丢 25 行、整个程序没了，而**退出码 0**） | 打印 `1`/`2`/`4`，无诊断 |

B 翼（块内）与 E 翼（338 的 import 用例）修前就 0 FAIL —— 这两翼锁的不是本批的缺陷，
是"别把它改坏"和"删掉补丁后那条规则仍然成立"，所以它们**必须**不咬；一起咬说明夹具在测别的东西。
D 翼那条 FAIL 是负控制的正面证据：接受空项**没有**顺手放宽吞词判据（`q;` 仍响 W1004、
`static mut …` 仍响 W1002+W1004、`!!!` 仍截断）。

### 三套门禁读数（批次 338 → 339）

`./tools/run_all.sh > /tmp/b339_run_all.log 2>&1; echo "run_all rc=$?"`（直读退出码；
ts `2026-09-22T14:03:29Z`）⇒ **rc=1**，唯一原因仍是既有判据 `tools/run_all.sh:360`
（`py_fail != 0`），那两条还是批次 326 以来的存量 `t231_dict_set_cast_fromkeys` /
`t233_listcomp_condition_capture`。

| 步 | 读数 | vs 批次 338 |
|---|---|---|
| official | compile **194/194**，compile+link 191/194（3 条 link-only：缺运行时绑定，#42） | 相同 |
| python_style | **291** passed / 2 failed / 4 known-fail / 0 xpass | +1 = 新增 t407；失败仍是那两条存量 |
| corpus | **39/39** | 相同 |
| jit sweep | ok=**170** trap=321 fail=0 timeout=0 **segv=0**（total 491，门槛 ok≥163） | ok/total 各 +1（t407 进 `tests/python_style/*.z` 的枚举） |
| diff | match=120 judged=130 rate=**92.3%** bad_case=0 | 相同 |
| knob / swallow / import | 23 / 4 / 22 条断言，FAIL 全 0 | 相同 |
| **empty_stmt（本批新增）** | **68 条断言 / FAIL 0** | 无上一格 |
| compile-diagnostics official | 9 档 / 15 行，W1002 **丢行合计 1,016** | **与 338 修后逐字相同** |
| compile-diagnostics python_style | 81 档 / 190 行 | 相同 |

最后一行要说清楚，否则容易被读成"本批没效果"：**三套语料里没有任何一个真实文件在顶层写裸 `;`**
（grep 整仓 `.z`：整行只有 `;` 的文件只有新建的 t407）。所以那 9 档各有各的崩点、一个都没被挪动
—— 本批的价值不在语料分数，在"少一类地雷 + 少一个重复落点"。它同时是**恢复默认打开（#61）**
的前置：以前一个裸 `;` 就能让整个文件消失，那种状态下把恢复打开是不安全的。

### 自伤记录：锚点连续第四批被自己撞漂移（同形状）

往 `run_all.sh` 插 25 行 + 文件头那 2 行 `--skip-empty` ⇒ `docs/ABI.md` 的 3 条锚点漂移：
`:89→:91`、`:184→:186`、`:332→:359`（前两条是文件头那 2 行推的，第三条是本批那 25 行推的）。
重绑 + `--bless`，核对器现报 **243 条 / 0 漂移**。

批次 336、337、338、339 **连续四批同一形状**，而 336 写下的教训（"改 `run_all.sh` 之前先算它
下游有几条锚点"）至今只是被反复抄写、没有被执行。这次把它改成做掉：核对器已经同时打印"基线内容"
和"现在内容"（本次三条都是它当场打出来的），说明"锚点搬家"是**可判定**的。正解是给
`tools/check_abi_anchors.py` 加 `--rebind`：拿基线那段内容在目标文件里搜，**唯一命中**才改写行号，
零命中/多命中仍判漂移。登记为下一批默认候选（本批只做了人工重绑 + `--bless`）。

### 跟手更正：roadmap 内部的行号引用（核对器看不见这一类）

`tools/check_abi_anchors.py` 只扫 `docs/ABI.md`，roadmap 里的行号引用在它管辖之外（任务 #52
记的就是这个盲区）。本批逐条按内容重定位，**分两类记**——把别人的债算在自己头上同样是造假。

一、**本批造成的漂移**（`top_level.rs` 净 +4 行，但删除发生在插入点之前，所以引用是往回漂的）：

| 引用（写出它的批次） | HEAD 里 | 本批后 | 说明 |
|---|---|---|---|
| `parse_top_level_item`（338：`:1533`） | `:1533` | **`:1527`** | −6：`mod` 两处体内循环各收掉 3 行 |
| fail-loud 臂的 `DEFINITION_KEYWORDS`（338：`:1565`） | `:1565` | **`:1559`** | 同上 |
| `parse_zeta_impl_recover`（336：`:1958`） | `:1965` | **`:1981`** | 336 写下后被 337/338 推 +7，本批再推 +16 |
| W1003 打印（336：`:2006-2009`） | `:2014-2017` | **`:2018-2021`** | 同上 |
| `mod` 体内手抄的两条 `alt`（338：`:1449`、`:1466`） | 同 | **已删除** | 收敛进 `parse_top_level_entry` |
| 文件循环/恢复循环的 `alt`（338：`:1945`、`:1997`） | 同 | **已删除** | `:1997` 那处即本批删掉的 14 行重复段 |

二、**前几批遗留、本批逐条核对时才暴露**（`stmt.rs` 一侧本批净 0 行，漂动不是本批造成的）：
`parse_expr_stmt` 写作 `:679` 实在 **`:680`**、`warn_if_swallowed_prefix` 写作 `:700` 实在
**`:701`**、336 那句"`parse_stmt` 的行号随本批插入移动到 `stmt.rs:1626`"实在 **`:1670`**。
仍然逐条对得上、因此原文不动的：`stmt.rs:58`、`:660`、`:683`、`:913`、`:927-931`、`:1481`、
`:1514`（那三处是**语句级**规则自吃结尾分号，与顶层空项不冲突；本批只删 `import` 那一份，
因为它的动机原本是"防截断"，而防截断现在归顶层文法管），以及 `top_level.rs:141`。
随删除失效的只有 338 的 `stmt.rs:992`；指向它的三句历史表述保留原文，"改动"表里已指名它由
`top_level.rs:1608` 取代。

### OPEN

1. **`fn proto();`（无体签名声明）仍截断** —— 本批实测：修后 diag=W1002、`tail_marker` 不在 MIR。
   崩点不在 `;`，在顶层文法根本没有"签名声明"这一臂（= #61）。`;` 一族收干净之后，这是该路上
   唯一与 `;` 相关的残留，也是 #61 的第一个可验收盘据。
2. **`.gitignore:114` 的裸 `*.z` 本批当场咬到**（#40）：t407 建好后 `git status` 完全不显示它。
   若非人为查一次 `check-ignore`，本批会提交一个**不含用例的"用例批次"**，而门禁在作者机器上仍报
   291 passed。已 `git add -f` 入本批。根因未修：那段的注释是"competition 构建产物"，同段还有
   `*.c`/`*.a`/`*.lib`，收窄成带前缀的段内模式即可 —— 一行改动，且它是门禁可复现性的前提。
3. **`mod` 作用域下的函数调用打地址**（t407 注释里绕开的那条）：`print(holder::inner())` 实测打
   `4342763456`。与本批**无关**，反证 = 同一程序在修前/修后两版二进制下的 `--dump-mir` 逐字节相同
   （只差 `-o` 路径那行 `Compiled to …` 尾注）。独立缺陷，未编号。
4. **`import x as y;` 的 `as` 静默错绑**（承接 338 OPEN 1）：正解是给 `AstNode::Use` 加别名字段，
   不是改判据。
5. 承接未动的：#65（`LAST_PP` 串号）、#61（占位/签名声明）、#60（`zt_env_flag`）、
   #63（`--repl` 的 `_dump_mir` 收下即弃）、#52 尾巴（93 条待归属 / 裸行号）。

### 下一批默认候选

- **锚点核对器加 `--rebind`**：把"连续四批自伤"从抄写变成做掉；顺带能覆盖 #52 的裸行号盲区。
- **#40 `.gitignore` 裸 `*.z` 收窄**：一行，且门禁计数的可复现性押在它上面（本批差点踩空）。
- **#61 占位/签名声明**：OPEN 1 已经把它的第一个可验收盘据量好了（`fn proto();`）。

---

## 批次 340（#40 落地：`.gitignore` 的裸 `*.z` / `*.c` / `test_*` 吃掉的是**手写的源文件**，本批把它改成可复现）

### 选题：339 的 OPEN 2 说"一行改动，且它是门禁可复现性的前提"——实测前提比这更硬

批次 339 建 `tests/python_style/t407_empty_stmt.z` 时，`git status` 完全不显示新建的用例，
根因是 `.gitignore:114` 的裸 `*.z`（#40，批次 322 发现）。当时只 `git add -f` 把文件捞回来，
债留给下一批。开工前先把值域量全 —— **哪些手写文件因为这几条规则从未进过仓库**：

| 文件 | 盘上 | HEAD 树里 | `git log --all -- <该文件>` | 谁依赖它 |
|---|---|---|---|---|
| `pylib/numpy.z` | 95 行 / 2,807 B | **不在** | 空（从未提交过） | `src/middle/pylib.rs:536` 的 `include_str!`（**编译期**）＋ `import numpy` 的 PY-A 运行时加载 |
| `tests/python_style/t124_ternary.z` | 1,080 B | **不在** | 空 | `tests/python_style/run.sh` 的用例计数（PY-A 三元式） |
| `tests/unit/test_float_e2e.z` | 328 B | **不在** | 空 | 无工具引用 `tests/unit/`（门禁只读 `tests/unit-tests/`，见 `tools/run_all.sh:83`） |

计数侧证据（一律对 **HEAD 树**比，不对索引比 —— 索引里已经有本批暂存的文件）：
`git ls-tree -r HEAD --name-only pylib` 里 `.z` 只有 1 个（`pandas.z`），盘上 2 个；
`tests/unit` 是 40 对 41；`tests/python_style` 在干净检出里 296 个 `.z`，在作者机器上 297 个。

### 实测：干净 checkout **连编译都过不了**，不止"计数少 4 个"

上一轮我只做了"把两个文件移开再跑一次"（→ 287 passed / 5 failed）。那量到的是**运行期**那条
机制；`include_str!` 是**编译期**的，移文件不影响已建好的二进制，所以那次读数低估了灾情。
本批改用真检出：`git worktree add --detach /Users/meetai/wt340b HEAD`（HEAD = 36533049，
即批次 339 的提交），只把 `zetac` 用符号链接指过来、`ZETA_RUNTIME_DIR` 指回主目录 —— 也就是
**把工具链钉死，只让"入库与否"这一个变量动**：

| 状态 | `cargo check` | `python_style` 读数 |
|---|---|---|
| 干净 checkout（= 本批之前任何人 / CI 拿到的仓库） | **rc=101**，首错 `/tmp/wt340b_check.log:240`：`error: couldn't read src/middle/../../pylib/numpy.z`: No such file or directory --> `src/middle/pylib.rs:536:13` | **287 passed / 5 failed**（`t212` `t227` `t230` + 存量 `t231` `t233`），`tests/python_style` 里根本没有 t124 |
| 同一检出 + 本批 4 个文件的内容（先把文件拷进去量的，为的是在提交前就知道数字） | **rc=0**（`Finished dev profile in 4.31s`） | **291 passed / 2 failed**（只剩存量 `t231` `t233`） |
| **真·干净检出 `801fc78a`**（提交后另开 `/Users/meetai/wt340c`） | **rc=0**（`Finished … in 4.39s`，`/tmp/wt340c_check.log`） | **291 passed / 2 failed**（`/tmp/wt340c_py.log`，failed 名单同上） |
| 作者机器（改动前后都是这份） | rc=0 | 291 passed / 2 failed |

第二行与第三行互相核对过：那 4 个文件在模拟检出里的 blob 哈希与提交里逐字节相同
（`git hash-object` 对 `git rev-parse 801fc78a:<path>` → `1343daca` / `dfb6d0cf` / `0af325a6` /
`718e77da`），所以后读数既属于那份模拟，也属于真克隆。

三条 E0282（`pylib.rs:539` / `:540` / `:541`）是同一个错的类型推断级联（`include_str!` 失败后
`src` 无类型），不是第二个缺陷。CI 侧同一件事：`.github/workflows/ci.yml:33` 的第一步是
`cargo test --workspace`、`:35` 是 `cargo build --release`，两者都撞这条 —— 见 OPEN 4。
`include_str!` 缺文件是硬错误不是警告，这一点另用一行程序独立复现（`/tmp/isl/main.rs` →
`error: couldn't read /tmp/isl/./nope.z`），免得有人以为它会退化成空串。
`numpy.z` 的两条依赖机制也分别钉住：编译期 `pylib.rs:536`，运行期 `resolver.rs:2251`
把 `pylib` 放进 PY-A 搜索路径。

**交叉验证**：真检出的 287/5 与上一轮"移开两个文件"的 287/5 是同一份 failed 名单，逐字相同 ——
两条独立路径给出同一个数，说明这 4 个用例的差额全部归因 `.gitignore`，不来自 worktree 环境。

### 改动

| 文件 | 改了什么 |
|---|---|
| `.gitignore:116-123` | `*.c`(:115) 后加 `!*.c` + `runtime/aliases.inc.c`，即把"按扩展名忽略"收窄成"按名字忽略那一个再生文件" |
| `.gitignore:131-139` | `test_*`(:130) 后加 `!pylib/*.z` + `!tests/**/*.z`，只对手写源所在的两棵树开负例 |
| `pylib/numpy.z`、`tests/python_style/t124_ternary.z`、`tests/unit/test_float_e2e.z` | 首次入库（三者 `git log --all` 原本为空） |
| `roadmap.md` | 本节 |

净账：`.gitignore` 2,994 → 4,154 字节、**+17 行 / −0 行**（hunk 头 `@@ -115,2 +115,10 @@` 与
`@@ -122,2 +130,11 @@`；内容是注释 13 行（6 + 7）加 4 条规则），其余文件逐字节不动。
**NUL 字节改前改后都是 7 个**（本批没引入损坏，也没修，见 OPEN 3）。

### 反证 / 边界：放开的是手写源，产物一个没漏出来

- `find . -name '*.c'`（排除 `target/`、`.git/`）共 11 个，其中未跟踪的**只有** `runtime/aliases.inc.c`
  一个，改后仍被忽略（`git check-ignore -v` → `.gitignore:123` 命中它自己的名字）；另外 10 个
  早已跟踪，而 ignore 规则对已跟踪文件无效 —— 所以 `!*.c` 带来的 `git status` 噪声 = **0**。
  它挡住的是"以后往 `runtime/` 放手写 .c"这一整类，而不是今天某个具体文件。
- `aliases.inc.c` 该不该入库：不该。它是 `@generated`，`tools/build_runtime.sh:15`（`--gen` →
  `gen_from_registry.py --emit-aliases`）能再生 —— 保持忽略是对的，本批只是给它换了条不牵连全仓的规则。
- 产物侧一个都没放开：`find … -name '*.z' | git check-ignore --stdin` 在改动后仍被忽略的只剩
  `build/stubs/**`，加上根级三个陈旧 scratch（`test_neg.z` 83 B、`test_match_simple.z` 178 B、
  `test_match.z` 530 B，mtime 09-07/09-08，且 `tests/unit-tests/` 下有同名跟踪版本）—— 三者按
  scratch 处理，故意不动。
- 改后 `tests/` 与 `pylib/` 两棵树里**已无任何"被忽略且未跟踪"的 `.z`/`.py`**（同一条 sweep 为空）。
- 新建用例从此免 `-f`：`git check-ignore -v tests/python_style/t999_probe.z` → `!tests/**/*.z`（:139）。
- 反向边界（本批**没**收干净的部分）：裸 `*.z`(:114) 还在，`src/probe.z`、`tools/probe.z`、
  `docs/examples/probe.z`、`examples/probe.z` 四个探针实测全部仍命中 `:114` —— 见 OPEN 1。
- 门禁 9 步读数逐项不变（下表），因为它测的是作者机器上一直在跑的这份工作树；本批动的是
  "别人克隆下来会看到什么"。

### 门禁读数（`./tools/run_all.sh`，直接读退出码：`run_all rc=1`）

| 步 | 读数 |
|---|---|
| official | compile **194/194**，compile+link **191/194**（3 条 link-only = #42） |
| corpus（self-host） | parse_ok **39/39** |
| python_style | **291 passed / 2 failed**，4 known-fail，0 xpass |
| jit sweep | ok=170 trap=321 fail=0 timeout=0 segv=0（total 491，最小 ok=163） |
| diff | match=120 judged=130 rate=92.3% bad_case=0 |
| knob / swallow / import / empty_stmt | 23/0、4/0、22/0、**68/0** |
| 编译期诊断 | official 9 文件 / 15 行；python_style 81 文件 / 190 行 |
| 锚点 | 243 条可解析 / 定位失败 0 / **漂移 0**（基线 243 条），待归属 93 条 |

`rc=1` 的唯一来源仍是存量 `py_fail=2`（判据 `tools/run_all.sh:360`，`t231` / `t233`）。

**连续四批的锚点自伤在本批断了**，但别把它读成"我变小心了"：本批一行都没碰 `run_all.sh` 和
`docs/ABI.md`，插入点不在任何锚点的下游，所以漂移为 0 是**运气不是纪律**。`--rebind` 仍然排
下一批默认候选第一位。

### 意外收获：同一份代码，仅仅检出位置不同，门禁读数就差 3 个用例

第一次我把 worktree 放在 `/tmp/wt340b`，`python_style` 读到 **284 passed / 8 failed** —— 多出的
三条正是 `t75_re_flags` / `t76_re_pattern` / `t90_re_escape`，日志 `/tmp/wt340b_py.log:347-348`
当场给出原因：`PY-A: imported module re from /tmp/re.z`。机制在 `resolver.rs:2236-2245`：
被编译文件所在目录**连同最多 6 层祖先**都在 PY-A 的搜索路径里（注释自陈 "and so do its
ANCESTORS … Depth is capped so a plain name can never match `/a.py`"）。而 `/tmp` 根上躺着
**328 个**陈旧 `.z` 探针（`ls /tmp/*.z | wc -l`），其中 `/tmp/re.z` 是个 pandas 探针，与 `re`
毫无关系，却把内置 shim 盖掉了。把 worktree 挪到 `/Users/meetai/wt340b`（祖先目录里 `.z` 计数
为 0）之后才拿到上表那两份 287/5 与 291/2。

这条不是本批引入的，但它让"我机器上门禁是绿的"这句话又弱了一截：**读数对目录布局敏感**，
且盖掉时只有 `warning:` 一级出声。登记为 OPEN 2。

### 复核本批的一处自我更正

`.gitignore` 在 git 眼里是**二进制**（就是那 7 个 NUL），`git diff --numstat` 对它输出
`-	-	.gitignore`，所以本批这 17 行在常规 diff 里读不出内容。上面"改动"表里的行号与文本，
证据来自 `python difflib` 对 `git show HEAD:.gitignore` 与盘上文件的逐行比对（两段 hunk：
`@@ -115,2 +115,10 @@` 与 `@@ -122,2 +130,11 @@`）。写这一句是因为：**如果没人记下，
"改动无法被 review"这件事本身就成了造假的温床。**

### OPEN

1. **裸 `*.z`(:114) 仍在**，本批只在 `tests/`、`pylib/` 两处开负例。`src/`、`tools/`、
   `docs/examples/`、`examples/` 下新手写 `.z` 依旧静默消失（四个探针实测）。正解是把那一段
   （注释自称 "Competition binary…"）改成带前缀的产物模式再删裸规则，前提是先清点的确哪些
   产物靠它 —— 今天的答案是"只有 `build/stubs/**` 和根级 scratch"，但那是**当前**盘上的读数。
2. **PY-A 祖先搜索（`resolver.rs:2236-2245`）让门禁读数随检出位置变化**：本批实测差 3 个用例
   （284/8 vs 287/5）。修法二选一 —— 祖先搜索在越过仓库根时**出声**（现在只有一条 `warning:`，
   而且这次是打在聚合诊断里、没人逐条读），或者干脆不越出根。顺带：`tools/` 的临时探针不该往
   `/tmp` 根上扔（328 个存量）。
3. **`.gitignore` 里那 7 个 NUL**（`:153-155`，`PERFORMANCE_*.md` 那行尾部混进一段 UTF-16 编码的
   `zeta/`）：HEAD 版本同样 7 个，**前置于本批**。后果见上一节 —— 这个文件的所有改动对 git、
   对锚点核对器、对 code review 全部不可见。修它要重写这 3 行，且文件是 CRLF，单独一批做。
4. **CI 到底有没有在干净 clone 上跑过？** `ci.yml:33` 第一步就该红（实测 `cargo check` rc=101）。
   要么这些 job 没接/没跑，要么 runner 复用了带这些文件的目录。本批看不到运行记录，不猜结论。
   可验收动作：把"干净检出 + `cargo check`"变成门禁的一步（见下一批候选）。配方本批已经在手：
   `git worktree add --detach <路径，且**不能放在 /tmp**> <commit>` + 符号链接 `zetac` +
   `ZETA_RUNTIME_DIR` 指回构建产物所在目录 —— 不重建 LLVM 依赖，4 秒量完。
5. `tests/unit/test_float_e2e.z` 入库 ≠ 进门禁：没有任何工具引用 `tests/unit/`（run_all 读的是
   `tests/unit-tests/`，`tools/run_all.sh:83`）。它只是 41 个同类里的第 41 个，随规则放开一起回来。
6. 承接未动的：#61（占位/签名声明，`fn proto();` 盘据已量好）、#65（`LAST_PP` 串号）、
   #60（`zt_env_flag`）、#63（`--repl` 的 `_dump_mir`）、#52 尾巴（93 条待归属 / 裸行号）、
   `mod` 作用域函数调用打地址（339 OPEN 3）、`import x as y;`（339 OPEN 4）。

### 下一批默认候选

- **锚点核对器加 `--rebind`**（339 就登记了，本批仍然没做）：连续四批自伤被第五批的"没碰文件"
  掩盖，不等于债已还。顺带覆盖 #52 的裸行号盲区。
- **门禁加一步"干净检出可编译"**：在临时目录 `git worktree add` + `cargo check`，专治本批这类
  "作者机器绿、克隆下来红"的债；同时能逼 OPEN 2 的选址问题被正式解决。
- **#61 占位/签名声明**：`fn proto();` 的可验收盘据已经在 339 OPEN 1 里量好。

---

## 批次 341（#69 落地：锚点核对器加 `--rebind` —— "这只是搬家"从记性问题改成可判定动作）

### 选题盘据：连续四批、13 条手工重绑，动作完全同形

| 批次 | 记录位置 | 当场漂移 | 具体搬家 |
|---|---|---|---|
| 336 | 本文件 `:11481` "自伤记录（锚点核对器当场抓到）" | 3 | `run_all.sh:83→:85`、`:178→:180`、`:252→:279` |
| 337 | `:11671` "又是自己改的行把锚点撞漂移了" | 3 | `:85→:87`、`:180→:182`、`:279→:306` |
| 338 | `:11837` "锚点自伤（连续第二批，形状完全相同）" | 4 | `:87→:89`、`:182→:184`、`:306→:332` ＋ `top_level.rs:334-355→:341-362` |
| 339 | `:11962` "锚点连续第四批被自己撞漂移（同形状）" | 3 | `:89→:91`、`:184→:186`、`:332→:359` |

合计 **13 条**，每条的动作也都是同一步：读核对器打印的"基线内容 vs 现在内容" → 在被改的文件里
找到那句话 → 改文档里的数字 → 再 `--bless`。前三步是**机械判定**，不是判断：核对器已经把内容
打印出来了，"搬家"的定义就是"这句在文件里唯一命中"。批次 336 写下的教训（"改 `run_all.sh`
之前先算它下游有几条锚点"）被后面三批各抄一遍、执行 0 次 —— 它要求的是记性，而四批共同缺的
正是记性。把判定交给工具，记性就不再是判据的一部分。

暴露面用 `--list` 量，不靠估：`docs/ABI.md` 挂在 `tools/run_all.sh` 上的锚点共 **4 条**
（`:186` jit ok 提取式在 `ABI.md:806`、`:91` `--no-link` 归因在 `:951`、`:359` official 判据在
`:955`、`:10` 编译器路径在 `:959`）。在文件头插行会让这 4 条同时漂。口径要说清：336/337/339
每批漂的 3 条都是 `run_all.sh` 的，338 的第 4 条是 `top_level.rs` 的区间引用 ——
`:10` 那条是批次 323 之后才进池的，所以"同样一次插行"今天的半径是 4，不是当年的 3。

### 改动（只动工具自己一个文件，`+178 / −2`，546 行）

| 位置 | 内容 |
|---|---|
| `tools/check_abi_anchors.py:23-29` | 用法块补 `--rebind`（含 `--dry`）＋ 一段"它治哪一种漂移、判据是什么" |
| `:182` | 新类型 `Pos` —— 文档行号 / 解析到的文件 / 起止行 / 两组数字在**原始文档行**里的列区间 |
| `:234`、`:248` | `collect` 的两条正则各取 `m.span()`（区间右端点单独取），在 `:282` 落进 `positions` |
| `:298` | `find_snippet_lines()`：与 `collect` **同源判据**（同一个 `normalize`、同区间长度）逐行找锚点现在的位置 |
| `:311` | `rebind()`：搬家 → 改文档 → 刷基线；六条拒改分支（本批量到 4 条，见反证矩阵） |
| `:342` | `refuse()`：拒改时连 `(文件,行号)` 一起记下，供刷新基线时原样保留 |
| `:407-408` | 同一文档行内**从右往左**替换列区间，否则前面那个数字会被后面那次改写顶掉 |
| `:417` | 刷新基线 = 重采 ∪ 拒改条目的旧内容 —— 不是无条件重采（见下文那个被反证抓到的缺陷） |
| `:451` | `--dry` 单用（不带 `--rebind`）当场 rc=2 拒收："收下即弃的开关"正是任务 #63 那一类缺陷 |
| `:506` | 分发点在基线存在性检查**之后**：无基线时仍先报 `[E1001]`，不允许对着空基线重绑 |

列区间而不是"再匹配一次文本"是刻意的：`strip_exempt` 抹豁免片段时抹的是**等长空格**，
所以正则给的列号与原始文档行一一对得上，改写可以精确到数字，不会顺手改掉同形的批次号。

### 实测①：happy path（造一次 336~339 的形状，然后一条命令收口）

| 步 | 命令 | 读数 |
|---|---|---|
| 1 | 往 `tools/run_all.sh` 头部插 5 行注释 → `./tools/check_abi_anchors.py` | **rc=1**，`漂移 4 / 新 0 / 消失 0`（基线 243 条） |
| 2 | `./tools/check_abi_anchors.py --rebind --dry` | rc=0，`漂移 4 → 判定搬家 4 / 拒改 0`，打印"文档未改动" |
| 3 | `./tools/check_abi_anchors.py --rebind` | rc=0，同判定 + `已改写 docs/ABI.md（4 行 / 4 个数字）` + `基线已随之刷新（243 个锚点；定位失败 0 条）` |
| 4 | 再 `./tools/check_abi_anchors.py` | **rc=0**，`漂移 0 / 新 0 / 消失 0`，243 条、93 待归属 / 84 种、14 仓外**全部不变** |

`:10→:15`、`:91→:96`、`:186→:191`、`:359→:364` 四条都是"内容逐字相同，全文件唯一命中"。
原来 13 次里的每一次都要人读三段内容再数偏移，现在是一次命令；`--dry` 用来在改之前先看判定。
步 2 的 rc=0 与步 1 的 rc=1 不矛盾：`--dry` 的退出码只表"判定结果"，不表"文件已对上"（它什么都没改）。

### 实测②：反证矩阵（这几条不通，`--rebind` 就是自动造假证据的机器）

| 反证 | 构造 | 期望 | 实测 |
|---|---|---|---|
| A 就地改写 | 把锚点那句 `ZETAC=…release…` 换成 `…debug…` | 零命中 ⇒ 拒改、不许洗白 | `[拒改] tools/run_all.sh:10 → 零命中：那段内容已不在 …（就地改写？必须人工重读合同）`；同一次里其余 3 条照搬；复跑 **rc=1 / 漂移 1** |
| B 多命中 | 文件末尾再放一份逐字同形句，并在头部插 2 行顶掉锚点位置 | 唯一性不成立 ⇒ 拒改 | `[拒改] :10 → 2 处命中（12, 381）：唯一性不成立，不猜`；`ABI.md:959` 那条**仍写 `:10`**、未被自动改；复跑 rc=1，且打印出"基线=原句 / 现在=注释行" |
| C 锚点落在空行 | 把 `:10` 那行清空（不漂，直接变成不可解析） | 报"定位失败 + 消失"而不是搬家 | `--rebind`：`[拒改] :10 → 文档不再产生这条锚点（消失），rebind 不处理`，且**不写文档**（`edits` 为空即返回）；复跑 **rc=2 / 消失 1** 原样在 |
| E 同键不同长度 | 在 `tools/parse_bisect.py:158` 上方插 1 行，并在 `ABI.md:959` 把同一锚点写成 `:158`＋`:158-160` 两种长度 | 区间长度不一致 ⇒ 拒改（改了必留一条继续漂） | `[拒改] tools/parse_bisect.py:158 → 同一 (文件,行号) 在文档里有**不同长度**的区间引用`；不写文档，复跑 **rc=1 / 漂移 1** 原样在 |
| D 老路径零回归 | `--bless` 写到临时基线，与 HEAD 版 `cmp` | 逐字节一致 | `cmp` 无输出 ⇒ 327 行相同（`collect` 改成 7 元组没动快照内容）；`--list`/`--pending` 的计数也仍是 243 / 93 / 14 |

`rebind()` 一共有 6 条拒改分支，上表量到 4 条。另两条按构造不该可达，**没有实测**：
"文档里已找不到这条引用"（漂移键来自快照，快照必然带位置）与"内容命中自身所在行"
（命中在原地就不叫漂移）—— 它们是防御性断言，写在这里是为了下次出现时能被认出是工具缺陷。

### 反证抓到的缺陷：第一版会把 rc=1 洗成 rc=0

第一版（`rebind()` 末尾无条件重采基线）在多命中构造上的读数是：`[拒改] :15 → 3 处命中`
**紧接着**复跑 `漂移 0 / 锚点全部对上`、rc=**0**。原因：文档里的 `:15` 已经指着 `cd "$ROOT"` 了，
重采把这个错误配对当成新的事实写进基线 ⇒ 核对器从此认为这条合同"逐字核对过"。
**这比不做更坏**：漂移至少还会响。修法是 `:417` 那次合并 —— 只重采真正搬过家的条目，
拒改的把旧内容原样塞回去，让它继续报错。
修后同一类构造（上表 B 行）：`已改写 3 行`（可证明的那三条）＋ `拒改的 1 条原样保留 →
核对器会继续报错` ＋ 复跑 rc=1 / `漂移 1`，并把"基线=原句 / 现在=注释行"两行照样打出来；
A、C、E 三行复跑同样还在报错（A `rc=1 / 漂移 1`、C `rc=2 / 消失 1`、E `rc=1 / 漂移 1`）。

happy path 从头到尾都是绿的 —— 这个缺陷没有任何别的来源，只有把"必须拒"的场景构造出来才看得见。

### 跟手更正：339/340 写的"顺带覆盖 #52 的裸行号盲区"不成立

`:12018` 与 `:12172` 两处都写了"`--rebind` 顺带能覆盖 #52 的裸行号盲区"。做完才看清是错的：
`--rebind` 的前提是"有路径 ⇒ 能定位文件 ⇒ 有快照可比"，而 #52 的债恰恰是**裸行号没有路径**，
在 `collect` 阶段就落进"待归属"、连快照都不产生，谈不上重绑。且核对器**故意**不扫 `roadmap.md`
/ `refactor.md` 这类活文档（模块 docstring 末尾："锚点不指向移动中的文档"），所以本批也没法拿它
给自己收口。#52 的 93 条待归属 / 84 种原样继承，一行都没少。

### 操作自伤记录：在后台门禁执行期间 `git checkout` 了它自己

第一次把门禁放后台，随后为了做下一个控制立刻跑了 `git checkout -- tools/run_all.sh` ⇒
bash 是**边读边执行**的，文件被换掉后它从旧偏移继续读，读到
`./tools/run_all.sh: line 151: syntax error near unexpected token 'fi'`、rc=2，
而 `/tmp/zeta_baseline.json` 停在 `ts=2026-09-22T14:34:16Z`（**上一批的读数**）。
只看 JSON 会得到"这批也跑过了"的假读数。教训一条：**门禁在跑就不要动 `tools/run_all.sh`**，
要复位等它结束 —— 与批次 321 立、写在 `docs/ABI.md:962` 的那条规矩（"两次矛盾读数出现时，先怀疑测量"）同源，只是这次的测量扰动是我自己造的。
下面的门禁读数是重跑的那一次。

### 净账

一句话：**"这只是搬家"从需要记性的教训变成了 `--rebind` 的一次判定，而它的第一版被自己的反证
抓出会把 rc=1 洗成 rc=0** —— 收口 1 项登记两批的债（#69），更正 2 处 overstated 声明（#52 那句），
新增 1 条操作纪律（门禁跑着别动被执行的脚本）。编译器、运行时、语料一行未动。

### 边界（它不承诺什么，别当"已覆盖"）

1. 只回答"这句话还在不在文件里"，不回答"这句话是不是那条合同"。整段搬走且原地留下同样文本
   ⇒ 依旧测不出来（模块 docstring 里那条老边界，`--rebind` 没有改变它）。
2. 不处理区间内部被改一个字的情况：判据是**整段逐字相同**，段内任何改动都算零命中 ⇒ 人工。
   338 那条 `top_level.rs:334-355→:341-362` 如果当时段内也改了，本工具只会拒，不会猜。
3. 不接门禁、不接 CI：实测 `grep -n check_abi_anchors tools/*.sh .github/workflows/*.yml` **零命中**
   ⇒ 与任务 #37 同一个缺口：漂移只让核对器自己 rc=1，没有任何自动化的东西看它。
   `--rebind` 会写工作树，更不该悄悄进 CI（要进的是"只计数不改"的那一半）。
4. 首次 bless 就锁死错误行的锚点，本工具永远看不见它 —— 它治的是漂移，不是错绑。

### 门禁读数

`./tools/run_all.sh > /tmp/b341_gate2.log 2>&1; echo "run_all rc=$?"`（**直读退出码，不经管道**；
JSON `ts=2026-09-22T15:14:30Z` —— 这个时间戳同时证明第一次那条被搅断的运行**没有**覆盖基线）
⇒ **rc=1**，唯一原因仍是既有判据 `tools/run_all.sh:360`（`py_fail != 0`）：那 2 条还是
`t231_dict_set_cast_fromkeys` / `t233_listcomp_condition_capture`。
顺带把本批的主题落在这个文件上钉实：`py_fail` 判据在 `:360`，而 ABI.md 引用的 official 判据在
`:359` —— 相邻两行，一条在锚点池子里、一条不在。插 5 行会把前者推到 `:364`（实测），这就是
`--rebind` 判"搬家"的那个样本。

| 步 | 读数 | 与批次 340 |
|---|---|---|
| official | compile 194/194，compile+link 191/194 | 相同 |
| python_style | pass 291 / fail 2 / known-fail 4 / xpass 0 | 相同 |
| corpus | 解析通过 39/39 = 100% | 相同 |
| jit sweep | ok=170 trap=321 fail=0 timeout=0 segv=0（总 491） | 相同 |
| diff | match=120 judged=130 92.3% 坏用例 0 | 相同 |
| knob | 23 断言 / FAIL 0 | 相同 |
| swallow | 4 断言 / FAIL 0 | 相同 |
| import_form | 22 断言 / FAIL 0 | 相同 |
| empty_stmt | 68 断言 / FAIL 0 | 相同 |
| 诊断 official / python_style | 9 文件 15 行 ／ 81 文件 190 行 | 相同 |
| 锚点核对 | rc=0，243 条 / 漂移 0 / 93 待归属 / 14 仓外 | 相同 |

九步全同是本批应有的形状：**一行编译器、运行时、语料代码都没动**，改的是 `tools/` 下的一个
核对脚本。它同时也是反面证据 —— 门禁不跑锚点核对（见 OPEN 1），所以"锚点漂了"这件事
从来不影响这九个数，只影响 `docs/ABI.md` 的可信度。


### OPEN（本批新增／推进）

1. **锚点核对（含漂移计数）没有接线**：#37 的老债，本批把 `grep` 证据补实了（工具与 CI 双零命中）。
   可验收动作：门禁加一步只读核对，把 `漂移 / 新 / 消失 / 待归属` 四个数写进 `zeta_baseline.json`。
   注意 `--rebind` 不在其中 —— 它会改文件。
2. **`--dry` 的退出码语义**：现在它只表"判定结果"（无拒改即 0），即使文档仍有漂移。
   要不要让它跟随"文件当前是否已对上"是口径选择，本批不定，先记下来。
3. 两条按构造不可达的拒改分支未实测（见反证矩阵下方）。若哪天出现，说明 `collect` 与 `rebind`
   的键口径分家了 —— 那是工具缺陷，不是新增用法。
4. 继承未动：#61（占位/签名声明，`fn proto();` 盘据已量好）、#65（`LAST_PP` 串号）、
   #60（`zt_env_flag`）、#63（`--repl` 的 `_dump_mir`）、#52 尾巴（93 条待归属 / 84 种）、
   #70（PY-A 祖先 6 层搜索让读数随检出位置变化）、#71（门禁加"干净检出可编译"）、
   #72（裸 `*.z` 仍吞 `src/`、`tools/`、`docs/examples/`；`.gitignore` 的 7 个 NUL 使它不可 review）、
   `mod` 作用域函数调用打地址（339 OPEN 3）、`import x as y;`（339 OPEN 4）。

### 下一批默认候选

- **门禁加一步"干净检出可编译"**（#71）：配方在 340 OPEN 4 已经在手（`git worktree add --detach`
  放在**非 `/tmp`** 位置 + 符号链接 `zetac` + `ZETA_RUNTIME_DIR`，约 4 秒）。本批又给它添一条理由：
  门禁少一步，`cargo check rc=101` 那种"作者机器绿、克隆下来红"就只能靠人偶然撞上。
- **OPEN 1：把只读核对接进门禁**（承接 #37）—— 本批把工具补到"漂移可自动修"，但"漂移能不能被看见"
  仍然取决于有人记得跑它。
- **#61 占位/签名声明**：`fn proto();` 的可验收盘据在 339 OPEN 1。

---

## 批次 342（#71 落地：门禁加第 10 步"干净检出可编译"，并第一次把重绑交给 `--rebind`）

### 选题盘据

1. **340 OPEN 4 的那笔债有一整个批次的暴露史**：`pylib/numpy.z` 被 `include_str!` 引用却
   从未入库（`src/middle/pylib.rs:536`），前九步全绿了不知多少批，直到人偶然在别的目录里
   编译才撞上 `rc=101`。九步里没有一步会读 HEAD 的检出——它读的都是**带未提交改动的工作树**。
2. 代价先量再做（不是猜）：`git worktree add --detach` 一份 30 M 检出 + `cargo check`
   共享主树 `target/` ⇒ 本批选题阶段在一个临时工作树（`wt342`，量完已 `git worktree remove`）
   上实测 **冷 4.34 s / 热 0.9 s**，而主树
   `cargo check --offline` 之后仍是 0 s 级（共享没把主树缓存改脏）。给干净检出配**独立**
   target 则是先重建 7.0 G 依赖树 —— 那是"永远跑不进门禁"的形状，所以选址与 target 策略
   是这道题的两个真问题，本批两个都按实测定。
3. 341 的 `--rebind` 只在自己的反证矩阵里跑过。改 `tools/run_all.sh` 会让锚点池子里
   4 条中的 3 条必漂 ⇒ 这是它能拿到的第一次**生产**验收，不接受"看起来能用"。

### 改动

| 位置 | 内容 |
|---|---|
| `tools/run_all.sh:3` | usage 行加 `[--skip-clean]` |
| `:22`、`:36` | `SKIP_CLEAN=0` + flag 解析 |
| `:313-378` | 第 10 步：选址校验 → 在主树里解析提交 → `worktree add`/`checkout --detach` → 干净性断言 → `CARGO_TARGET_DIR=$ROOT/target cargo check --offline --locked -q` |
| `:323-325` | 三个 env 覆盖口：`ZETA_CLEAN_WT`（默认 `$HOME/zeta-clean-checkout`）、`ZETA_CLEAN_TARGET`（默认主树 `target`）、`ZETA_CLEAN_REF`（默认 `HEAD`） |
| `:374-375` | 失败输出：`head -20` + `tail -5`（不是别处的 `tail -30`，理由见"反证抓到的缺陷"） |
| `:404-405` | JSON 新键 `clean_checkout`：`rc` / `secs` / `rev` / `skipped` |
| `:448` | 判据：`SKIP_CLEAN=0 && clean_rc != 0 ⇒ rc=1`（93~99 这类"没测成"同样判红） |

净账：`tools/run_all.sh` **+75 / −1**，编译器、运行时、语料一行未动。

### 实测①：这一步的耗时形状（同一台机器、共享 target）

| 场景 | 读数 |
|---|---|
| 首次（target 里还没有该路径的指纹；本批选题阶段的临时工作树） | 检出 30 M + `cargo check` **4.34 s**，rc=0 |
| 复用一个已登记的工作树（本批常态） | 整步 **≤1 s**，`secs` 记 0 |
| `git worktree remove` 后让步骤自己重建 | 整步 **1 s**，rc=0，工作树 30 M，`status --porcelain` 0 行 |
| 同一天主树 `cargo check --offline -q`（第 10 步刚跑完） | rc=0，**0 s**（连测两次） ⇒ 共享没把主树缓存挤脏 |

### 实测②：反证矩阵（每一步都只跑第 10 步，判据行 `:448`）

| # | 构造 | 期望 | 实测 |
|---|---|---|---|
| N1 | `ZETA_CLEAN_REF=36533049`（批次 339 的**真实历史 HEAD**：`pylib/numpy.z` 当时根本不在树里） | 判红，且根因是"文件没读到" | `clean_checkout: rc=101（1s，rev=36533049）`，首行 `error: couldn't read \`src/middle/../../pylib/numpy.z\`` → `src/middle/pylib.rs:536:13`，gate rc=1 |
| N2 | `ZETA_CLEAN_WT=relative/wt` | 拒 | `rc=99`（必须绝对路径） |
| N3 | `ZETA_CLEAN_WT=$ROOT/inner-wt` | 拒 | `rc=98`（在仓库内会借用主树文件） |
| N4 | 先 `mkdir /Users/meetai/not-a-worktree-342` 并放 `sentinel.txt`，再指过去 | 拒，且**不碰**别人的目录 | `rc=96`，事后哨兵文件内容 `KEEPME` 原样、目录只多回它自己 |
| N5 | `ZETA_CLEAN_REF=no-such-ref-342` | 拒 | `rc=93`（在主树里解不出提交） |
| N6 | 在已登记的工作树里放一个未跟踪文件 `zz_probe_342.md` | 拒绝给读数 | `rc=94`（"检出 f4e6c7ba 后仍不干净，本步骤只 checkout，不 clean"） |
| N7 | `--skip-clean` | 静默跳过、不参与判定 | 无步骤行输出，JSON `{"rc":0,"secs":0,"rev":"","skipped":1}`，gate rc=0 |
| N8 | `CARGO_HOME=/tmp/emptych342`（干净 runner 的模拟） | 必须**响亮**地红在环境上 | `rc=101`：`error: no matching package named \`argon2\` found … you're using offline mode (--offline)` |
| 复位 | 默认参数 | 绿 | `rc=0` |

N1 是本批要的那一颗子弹：它不靠我改任何东西，直接把"检出不可编译"的历史事故重放进门禁。
其余七颗是护栏本身——它们若判绿，第 10 步就成了一个假的安全感来源。

### 反证抓到的缺陷（写在前，因为它是"实现先错了一版"的实录）

1. **陈旧检出假绿（真缺陷，已修）**。第一版复用路径写的是 `git -C "$WT" checkout --detach HEAD`，
   而 `HEAD` 在工作树里解的是**工作树自己的** HEAD。实测：先把工作树手动 detach 到
   `a4c25a22`（只改 roadmap 的祖先提交，能编）再跑步骤 ⇒ 它报 `rc=0（0s）` 判绿，而工作树
   仍停在 `a4c25a22`、主树 HEAD 是 `f4e6c7ba`。也就是说门禁会**无限重复测上一次那个提交**。
   修法：提交一律在主树里解析（`:339` `git -C "$ROOT" rev-parse --verify --quiet "${REF}^{commit}"`），
   `worktree add` 与 `checkout` 都用这个 sha，并把 `rev` 打进读数和 JSON（`:368`、`:404`），
   让"测的是哪个提交"变成可抽查的字段。复测：同一陈旧状态下改后 → `rev=f4e6c7ba`、工作树被
   移回、rc=0。
2. **失败输出把根因挤出了窗口**。沿用别处的 `tail -30` 时，N1 的 30 行里只有 4 条
   `error[E0282]: type annotations needed`，`grep -c "couldn't read"` = **0** —— 恰恰看不见
   真正那句。cargo 的根因在**开头**（级联在后），所以这里改成 `head -20` + `tail -5`
   （`:374-375`）；N1 复测首行即 `couldn't read … pylib/numpy.z`。

### 锚点：`--rebind` 的第一次生产使用（341 的实战验收）

改 `tools/run_all.sh` 后：`tools/run_all.sh:10` **没漂**（新增行在它下面），其余 3 条全漂 ——
这正是"搬家"的形状。

```
$ python3 tools/check_abi_anchors.py --rebind --dry
rebind：漂移 3 条 → 判定搬家 3 条 / 拒改 0 条
  [搬家] tools/run_all.sh:91 → :93   [搬家] :186 → :188   [搬家] :359 → :430
```

落笔前先把三个目标行逐字读出来核对（`sed -n '93p;188p;430p'`，三行与基线文本一一对上），
再 `--rebind`：改写 `docs/ABI.md` 3 行 / 3 个数字，基线随之刷新，复跑纯核对
`漂移 0 / 新 0 / 消失 0`，rc=0。339 起连续四批、共 13 次的手工重绑，本批第一次交给工具；
341 的"能判定搬家"这句话至此有了非自造样本的证据。

### 操作自伤记录（读数差点用错，两条）

1. **zsh 不分词**：`S="--skip-corpus --skip-official …"; bash tools/run_all.sh $S` 在 zsh 下把
   整串当**一个**参数，九个 `--skip-*` 全部失效 ⇒ 我以为是"只跑第 10 步"的 N1、N1b 实际各跑了
   一遍**全门禁**（约 7 分钟）。识别证据：JSON 里 `official_not_measured=0`、`import_form
   checked=22`。后果是 N1 那条 `gate rc=1` 的读数含义被我说过头（它是"全门禁 + 第 10 步红"，
   不是"只第 10 步红"）；本批结论已改用重跑后的那条（同节 N1 行）。往后凡"把一个变量里的多个
   flag 传进命令"，一律 `bash -c '…'` 包一层。
2. **管道尾 rc**：一次 `… | head -2; echo rc=$?` 读到的是 `head` 的 rc。这条纪律 341 刚写过，
   本批又踩了一次；所幸那次读数（stale demo）只用来判定"有没有动工作树"，没进任何结论。

### 边界（它不承诺什么）

1. 只判 `cargo check`（Rust 侧编译），不判链接、不判运行、不判 `.z` 语料能否解析。它的靶心是
   "文件没入库"这一类；`#42` 那 12 个缺运行时绑定的东西它看不见——那归 official 步（判据在 `:430`）。
2. **快是借来的**：共享主树 `target/` + 本机 `~/.cargo` 缓存。N8 实测干净环境会红在依赖解析上
   ⇒ 要接 CI（#37）必须先给 runner 缓存或去掉 `--offline`，否则红的是环境不是代码。
3. 与主树并发构建会争 cargo 的 target 文件锁（串行等待，本批未测时长）。
4. 它测 **HEAD**，未提交改动永不参与 —— 这是用途不是缺陷，但"我本地能编"仍要看前九步。
5. 工作树常驻 `$HOME/zeta-clean-checkout`（30 M），会被本步骤自己 `checkout --detach` 移动，
   不要在里面干活。选址必须在仓库外：`#70` 那条 PY-A 往上 6 级祖先搜索会让放在 `$ROOT` 之下的
   检出"借到"主树的 `.z`（`:318-319` 记了这条约束的来源）。
6. 零删除命令：全过程只用 `git worktree add` / `git checkout`，没有 `git clean`、没有 `rm -rf`
   指向任何非本脚本创建的路径；同名但未登记的目录直接拒（N4）。

### 门禁读数

`bash tools/run_all.sh > /tmp/batch342_gate.log 2>&1`（**直读退出码**，日志末行 `GATE_RC=1`），
JSON `ts=2026-09-22T15:43:26Z` ⇒ **rc=1**，唯一原因仍是既有判据 `tools/run_all.sh:431`
（`py_fail != 0`）：`t231_dict_set_cast_fromkeys` / `t233_listcomp_condition_capture`。

| 步 | 读数 | 与批次 341 |
|---|---|---|
| official | compile 194/194，compile+link 191/194 | 相同 |
| python_style | pass 291 / fail 2 / known-fail 4 / xpass 0 | 相同 |
| corpus | 解析通过 39/39 | 相同 |
| jit sweep | ok=170 trap=321 fail=0 timeout=0 segv=0（总 491） | 相同 |
| diff | match=120 judged=130 92.3% 坏用例 0 | 相同 |
| knob | 23 断言 / FAIL 0 | 相同 |
| swallow | 4 断言 / FAIL 0 | 相同 |
| import_form | 22 断言 / FAIL 0 | 相同 |
| empty_stmt | 68 断言 / FAIL 0 | 相同 |
| **clean_checkout（新增）** | **rc=0，secs=0，rev=f4e6c7ba，skipped=0** | 本批新增 |
| 诊断 official / python_style | 9 文件 15 行 ／ 81 文件 190 行 | 相同 |
| 锚点核对 | rc=0，243 条 / 漂移 0 / 93 待归属 / 14 仓外 | 相同（重绑后） |

前九步全同是本批应有的形状：动的只有 `tools/run_all.sh` 与由它引起的 3 个锚点数字。

### OPEN（本批新增／推进）

1. **只读锚点核对仍未接进门禁**（341 OPEN 1 / #37）——本批**评估后仍不做**，理由是可验收的：
   门禁任何一次给 `run_all.sh` 加行都会让锚点先漂后修，把核对放进去会让门禁红在本批自己
   身上（顺序死结）；而 `--rebind` 已把"修"降到 2 个命令。真正缺的是 CI 侧那一步只读核对，
   它必须先解决 N8 的环境依赖。
2. **`secs` 是整秒**，热路径读数就是 `0`，与"没跑"同形。目前靠 `rev` 非空 + `rc` 区分
   （唯一"没跑"形态是 N7 的 `skipped=1`）。要不要换毫秒，本批未定。
3. **N1 型事故只能事后拦**：本步骤判"HEAD 可编译"，但 HEAD 是**提交后**才形成的——门禁跑在
   提交前时，它测到的是上一个提交。真正的防线是提交后再跑一次（本批纪律里已有"门禁复跑后
   才提交"，但没有任何东西强制它）。
4. 继承未动：#70（PY-A 祖先 6 层搜索让读数随位置变化）、#72（裸 `*.z` 仍吞 `src/`、`tools/`、
   `docs/examples/`；`.gitignore` 的 7 个 NUL 使它不可 review）、#61、#65、#60、#63、
   #52 尾巴（93 条待归属 / 84 种）、#42（12 个 link-only std 绑定）、`mod` 作用域函数调用打地址、
   `import x as y;`。

### 下一批默认候选

- **#70 祖先 6 层搜索**：本批给它添了新场景——干净检出常驻 `$HOME`，哪天想在那里面跑语料
  （不只是 `cargo check`），位置敏感性立刻生效；先量"搜索路径命中顺序"再定判据。
- **#72 `.gitignore` 收窄的后半**：`*.z` 仍吞 `src/`、`tools/`、`docs/examples/` 下的手写源，
  且那 7 个 NUL 使它无法 review（CRLF 文件，只能字节级改）。
- **#61 占位/签名声明**：`fn proto();` 的可验收盘据在 339 OPEN 1。

一句话：**门禁从九步变十步，"作者机器绿、克隆下来红"这一类第一次有了自动读数；而这一步
自己在反证下先暴露出"会重复测上一个提交"的假绿，修完才让 `rev` 成为可抽查字段 ——
341 的 `--rebind` 也在本批完成了第一次生产使用（3 条漂移、2 命令收口）。**



