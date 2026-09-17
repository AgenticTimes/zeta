# Zeta 编译器 Roadmap

> 状态图例：[ ] 待做 | [~] 进行中 | [x] 完成 | [-] 放弃/降级
> 工作区：`/Users/meetai/source/zeta-src`（bootstrap 分支 → `agentic` 远端）
> 测试资产：官方单测 **`tests/unit-tests/`（194 文件，进 git 的正本）**；回归套件 `/tmp/bench`；**Python 风格套件 `tests/python_style/`（158 case 全绿，逐用例 20s 超时）**
> 当前通过率（2026-09-17 实测，validate.md §3 口径）：官方 **194/194**（运行退出码与基线零差异）；python_style **156/156**；REasyQuant 语料**完全解析 37/38**、未解析行合计 **77**（`ZETA_STRICT_PARSE` 口径，退出码见 §二）
> Python 库注册表：**16 个模块**（见「库导入机制」小节）；第三方库 `zorb install` 可用，已验真实库 `python-stringcase` 全函数正确
> 新目标（2026-09-11）：**基本能编译 Python**——PY-A 兼容层推进中
> 语法设计定稿：**`docs/python-syntax.md`（实现以此为准）**

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

**三套基线**：官方 **194/194**；python_style **156/156（0 failed）**；语料「解析通过」**38/38**、「完全解析」**37/38**、未解析行合计 **77**。

**语料（REasyQuant，38 个文件）** — 度量脚本：`tools/corpus_baseline.py`（解析通过）+ 对 `W1002`「丢了多少行」求和（完全解析口径）

| 指标 | 本阶段起点 | 现在 | 七轮批次合计 |
|---|---|---|---|
| 完全解析 | 6 | **37/38** | 12 → 37 |
| 未解析行合计 | 8580 | **77** | 6138 → 77（−6061，−99%） |

剩余拦路（2 个文件）：`jq_wufu_local` 159（`_run_nautilus`）、`ETF动量EPO` 77（`epo(…)`）。

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
