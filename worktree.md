# worktree.md — 双 worker 状态板与提交台账

> 本文件是两个并行 agent 的**消息板 + 台账**。设计要点：worktree 侧的更新在合并前不出现在主线工作副本里，
> 但**分支内容立即可见**——所以主线侧用 `git show cleanup:worktree.md` 读最新状态板，旁路侧直接看自己副本。
> 两个台账 section 按块分离，两边各自只写自己的 section，同文件编辑可自动合并。

## 0. 角色与工作区

| | Agent-1 主线（qwen） | Agent-2 旁路（ZCode） |
|---|---|---|
| 工作区 | 主树 `/Users/meetai/source/zeta-src` | `../zeta-bz`（分支 `cleanup`） |
| 方向 | 正向：roadmap2.md 主链向前（当前 = 主线 301 根因链，损害量序） | 反向：债务尾巴向回收（见 §1 队列） |
| 分支 / 号段 | `bootstrap` / 批次 414–439、t45x–t49x | `cleanup` / 批次 440–459、t50x 起 |

## 1. 两条工作队列（一个正着修，一个倒着清）

**正向队列（Agent-1 负责）**：按 roadmap2.md §2 主链的「下一批候选」，实测损害量大的优先。当前（批次 424 后）：

- **头名＝`codegen.rs` 的 `("", 2)` 兜底 + 越界索引夹回 0（`src/backend/codegen/codegen.rs:6452` 起、`:6700-6708`）** —— 424 把「`.items()` 的接收者为什么是日期整数」这条链追到底：形参拿不到调用点证据 ⇒ 接收者类型未知 ⇒ 按两字段兜底读 ⇒ 第三字段夹回 0 ⇒ 下一跳从整数身上按指针读。**这一级是"缺证据"放大成内存不安全的现场**：`ZETA_DBG_FA=1` 全量编译语料，529 条 FieldAccess 里 **28 条走了夹回 0**（`portfolio`×8 的 base 是 `PyDynamic`＝动态 `context` 那一形；`positions` 另 7 条 base＝`I64`；`avg_cost`×5、`paused`×3、`context`/`routines`/`trading_dates`/`low_limit`/`closeable_amount`/`bar_types` 各 2）。两个方向：(a) 缺证据时**出声**（沿用 423 那一招——判形→点名→`zeta_raise`，423 直接换来 rc 139→0＋38 个名字）；(b) 真修＝"名字→字段"的全局扫描补 variant 判据（队列里那格，**t461 已经是它的第一个夹具成员**：variant 为空、真打进这条兜底）。
- → **第 2 格＝回调／函数值实参的参数证据**（＝头名的上游根因）：语料 MIR census —— 628 条目／625 个不同调用目标名，**55 个宿主发 `FuncAddr`**、被取地址的目标 **106 个**，其中 **103 个从不出现在任何调用点的 `func:` 里** ＝ 97 个 `__closure_N_*` ＋ **6 个具名日程例程**（`jq_wufu__morning_routine`／`afternoon_routine`／`buy_routine`／`sell_routine`／`reset_daily_flags`／`check_weak_period_daily`，正是 `run_daily(<fn>, time=…)` 注册的那批）⇒ **被调度执行的函数，形参永远拿不到调用点证据**，`context` 因此恒 `dyn`。尺子现成，判据要新立（从注册点实参反推形参类型）。
- → **423 头名（`.items()` 的接收者＝当天日期）本批结案为"机制换对了、语料那一格没够着"**：423 写的"强烈指向**槽位复用/串号**（#130/#131/#142 同族）"**判死** —— 全程没有槽位复用参与，改的只有类型证据通道。读数保留并加细：37 条 `py_map_items` 点名的句柄是 37 个互不相同的连续整数 `0x4d0c…0x4d46`、跳过 `0x4d10/11` 等周末 ⇒ 逐日 +1 的交易日序数（`0x4d0c`＝19724＝t461 里 `dt` 的来源）；`py_map_items` 语料 IR 25 站点／23 宿主、`check_a_share_weak_period` 内恰好 1 个（423 读数当场复现）。**未证的一格**：28 处夹回里究竟哪一处 emit 了这 37 次点名 —— 点名句仍只有原语名＋`dladdr` 返回位置，423 §八.2 那条「让点名句带上用户函数名」的前置工作仍未做。
- → **424 的语料位移＝0（三层正证据，写在这里防下一批误读成回归）**：`--emit-llvm` 两侧 md5 逐字节相同、MIR 唯一 hunk 是 #9 的别名掷硬币（同侧两跑差 2 个 hunk），运行期四跑读数逐字相同。够不着的原因：语料里 `jq_wufu` 不是根模块 ⇒ 调用点全限定、从不带 `_N` 后缀。
- → **422 那格「下一格」的归因作废**（读数保留）：`market_panel.py`／`backtrader_engine.py` 语料 define＝0 成立，但「函数体内 import 不进闭包」是错的——`bt_helpers` 在语料里的入口 `jq_wufu_local.py:158` 正写在 `_run_backtrader`（:154）体内，它 IR define＝**8** ⇒ 体内 import 会进闭包。剩下的问题是「同形两条 import 为什么一条进一条不进」（`jq_shim.py:1025` 是相对 `from .backtrader_engine import`），未证。
- → **第 3 格＝`str_trim + 24` 读 `x0=0x3532`**（419 批的崩点；423 之后这条已不是当前死点——守卫版跑完不再崩，但它仍在 #145 的账上：packed str 的生产者还没定位）。生产者三条负结果已入册，真修大概率要动 `runtime/py_additions.c` ⇒ **等用户授权**，未获授权前不作为头名。
- → **第 4 格＝#134 `GroupBy.__len__` 恒为 0 / #117 动态键 `.get(k, default)` 语料 7 种形态仍落 `_get`**（两个都不等授权）。
- → **`[dynamic]` 幽灵名族（422 已定价完，从头名摘掉）**：6 个具名桩的调用点全在晨间崩点**之后**或死代码里，语料实际跑到的点名行数为 0 ⇒ 位移 0。逐名读数（`copy`×2 在 `data_cleaning.py:302/304`、`dropna`/`ffill` 在 `bt_helpers.py:62/77`、`isin` 在 `:302`、`to_dict` 在 `jq_wufu.py:455`）**订正**了 419/420 的「`copy`×4 头名」与 §1 旧句「`ffill`/`isin` 归零」；真修＝掩码族（`zt_vec_cmp` 在 `runtime/py_additions.c:1013`，任务 #112）⇒ 等授权，不在语法层打补丁。
- → **`rows` 同一个二进制跨多次运行结果抖动**（跑 5 次出 4 种值 12854–12858；420 又录到 119/135 位置的抖动；归因在运行期/库侧，不在 codegen）。
- → **原「名字→字段的全局扫描选错 struct」那格已升为头名**（见本节第一条）：424 当场把坐标钉成 `codegen.rs:6452` 起的 `("", 2)` 兜底与 `:6700-6708` 的索引夹回（调试旋钮 `ZETA_DBG_FA` 在 `:6658`；418 记的 `:6535`、419 后记的 `:6665` 都已因两次加行移位，本行是重新量过的）→ #142（`gen.rs:3987` 读点无守卫）→ #134（`GroupBy.__len__` 恒 0）→ #117/#118 打印族 → #136 漏点族定价。

**反向队列（Agent-2 负责）**：从历史欠账尾巴往回收，队列固定，两个子队列依次吃：

1. **解析器截断十族，按丢行量降序**（docs/ABI.md 附 B#10，批次 325 时点状态）：`static mut` 局部声明（丢 357 行）→ `s[i..]` 开区段（230，任务 #39）→ selfhost（158，未定位，先出定价批）→ 函数体内 `use a::b::C`（85）→ 块体闭包实参（58）→ or 模式构造子（62）→ `for i: usize in`（36）→ 单段 `import pkg;`（19）→ 常量表达式数组（14）；
2. **backlog 里最老的 OPEN 项**（按编号升序，S/M 级优先）。

两条队列一个吃「当前损害」、一个吃「历史欠账」，文件面不相交（见 §2），天然无冲突。


## 2. 所有权矩阵（禁手清单）

| 资源 | Agent-1 | Agent-2 |
|---|---|---|
| `src/middle/**`、`src/backend/**`、`src/main.rs` | ✅ 独占 | 🚫 |
| `src/frontend/parser/pattern.rs`、模式解析相关 `stmt.rs` 臂 | 🚫 | ✅ 独占 |
| `runtime/*.c` | ✅（#117 若需 C 侧） | 🚫 |
| `tests/python_style` 号段 | t45x–t49x | t50x 起 |
| `docs/ABI.md`、`tools/baselines/**` | ✅（含 `--rebind`） | 🔒 只读 |
| `roadmap.md` 尾部追加 | ✅ | 🚫（写自己台账，合并时誊入） |
| `backlog.md` 登记行 | 各自只写自己批次产生的行 | 同左 |
| 推送 `agentic bootstrap` | ✅ 唯一 | 🚫 |

## 3. 合并协议（READY → MERGED）

1. 旁路侧每完成 **≥1 族（含该族测试转正 + 自己分支上的全量门禁 rc=0）**：在本文件 §5 台账把状态改为 `READY(族=…, 门禁=…)`，提交到 `cleanup`，然后**自行 rebase**：`git rebase bootstrap`（冲突只可能在自己独占的文件里）。
2. 主线侧每批开工前：`git show cleanup:worktree.md` 查 READY。两批之间的空窗执行合并：
   `git merge cleanup --ff-only`（旁路已 rebase ⇒ 保持线性历史）→ 誊记录进 roadmap.md（批次号按 440+ 归位）→ 本文件 §4 合并记录行 + 状态改 `MERGED` → `git push agentic bootstrap`。
3. 合并后旁路侧：`git rebase bootstrap`（拿到合并后的新 HEAD）继续下一族。
4. 冲突预期：设计上为零（所有权矩阵）；若出现，停在合并动作、在台账记 `CONFLICT(文件)` 等人裁决。

## 4. Agent-1 主线台账（qwen 维护；bootstrap）

| 批次 | 分支 | 主题 | 门禁读数 | 状态 |
|---|---|---|---|---|
| 413 | bootstrap | `dict[...]` 注解保留 + 四处解包；主线 301 第 1 步有答案（codes=0 非缺列） | 194/194 · 333/2/6 · 40/40 · diff 92.3% | MERGED（已推送） |
| 416 | bootstrap | 主线 301 第 3 步定位：0 成交病因＝模块 `def` 不进环境表、当值读回 0（修法当时判死回退，登记 known-fail） | 194/194 · 335/2/**7** · 40/40 · diff 92.3% | MERGED（已推送 `72462832`+`66d61724`） |
| 417 | bootstrap | `FuncAddr` 值读上门（`gen.rs:3890`/`:12290`，+23）+ **更正 416 的退化归因**：三格塌＝出码轮次（HEAD 重编照样塌），抖动在 MIR 之后的发射阶段 | **rc=1**（`gate417b.log:142`）· 336/2/6 · 194/194+191 · 40/40 · jit 176/365/541 · diff 92.3% · ABI 32/11/3（`--rebind`） | MERGED（已推送 `9b7246c7`+`7c0f16cb`） |
| 418 | bootstrap | 出码非确定性两处落地：`codegen.rs:1606` alloca 序排序 + `:66` `struct_defs`→`BTreeMap`（代码批 `0f5bec05`，7 文件 +291/−161）；门禁第 16 步 `tools/emit_stable.sh` 上门；**两条负结果入册**：两个夹具盯不住源 B（把 BTreeMap 退回 HashMap 照样全过）、先前口误的 ABI 读数实为 **rc=2**（裸行号＝#52 的账） | **rc=1**（`/tmp/b418/gate418.log`，唯一红移到 `run_all.sh:605` 存量 `py_fail=2`）· emit_stable 2 夹具/违规 0 · 336/2/6/0 · 194/194+191 · 40/40 · jit 176/541 segv=0 · diff 120/130 92.3% bad 0 · clean_checkout rc=0 rev=`738618b8` · ABI 32/11/3（`--rebind` 77/77×2） | 代码批已提交 `0f5bec05`；记录批已推送 `969f8ec5` |
| 419 | bootstrap | 动态接收者的裸成员名兜底绑到类别名 ⇒ struct 体套在 vec 句柄上崩（`resolver.rs:1014-1026` 双注册 × `get_or_declare_function` 两处裸名兜底）。判据换成问 MIR 清单（`class_method_members`）：是别名就改绑 `zeta_raise(1)` 跳板；**两条负结果入册**（一律抛 ⇒ t411 回归；`count_basic_blocks()>0` ⇒ 查幽灵名那刻别名还是 0 块声明，且只守一趟输出逐字节不变）。代码批 `92e05c11`（4 文件 +321/−155） | **rc=1**（`/tmp/b419g/gate419.log`，唯一红源仍是 `run_all.sh:605 py_fail=2`）· 337/2/6/0 ＝ `ls t*.z` 345 逐字对上 · 194/194+191 · 40/40 · jit 176/**542** segv=0 · diff 120/130 92.3% · clean_checkout rc=0 rev=`969f8ec5` · ABI 定位失败 4→**0**、漂移 32→**23**（`--rebind` 74 行/147 数 + `--bless-only` 点名 9 条；终态 23/11/10 rc=1）· **主线位移：语料 134→135 行 rc=139→139，新增行＝越过 try/except 的 `[WARNING] jq_shim: 动态池更新失败: 1`；新崩点 `str_trim+24` / `x0=0x3532`** | 代码批已提交 `92e05c11`；记录批已推送 `92e05c11`+`26504a95` |
| 420 | bootstrap | 动态接收者的 `.get(<str 键>)` 下到 `array_get`（load＝`base + key*8`，字符串键＝拿键地址当元素下标 ⇒ 确定性内存不安全，改前 rc=139 实拍）；改为复用静态路径的 `MirStmt::DictGet`（`gen.rs:10423-10446` +24 行，负对照整型键逐字节未变）。新用例 t456。**一条弯路负结果**：直接换名表符号 ⇒ LLVM verifier 拒 `map_get` 的 `i64(ptr,i64)`（只有 DictGet 那条发射路插 `build_int_to_ptr`）。代码批 `24514e84`（4 文件 +77 −24） | **rc=1**（`/tmp/b420/gate.txt`，唯一红源仍是 `run_all.sh:605 py_fail=2`）· 338/2/6/0 ＝ `ls t*.z` 346 逐字对上（净增 1＝t456 无回归）· 194/194+191 · 40/40 · jit 176/**543** segv=0 · diff 120/130 92.3% · clean_checkout rc=0 rev=`26504a95` · ABI 终态 23/11/10 rc=1 定位失败 0（`--rebind` 各 13 条，与 419 终态逐字相同、ABI.md 1032 行不变）· **主线位移 0（有正证据）**：改前二进制仍死在 `fetch_stocks`（`str_trim+24`），在本批改到的 6 个 IR 函数上游；语料 IR `@map_get` 225→239＝14 个调用点换写法 | 代码批已提交并推送 `24514e84`；记录批随后 |
| 421 | bootstrap | 动态接收者的 `.get(k, default)`（argc=3，名表零臂）下成 `map_get_default`：语料 IR `get_3` 32→0、`map_get_default` 27→61、LLVM bare `@get(` 34→0。**两笔代码提交**：`04a6aeeb`（gen.rs +39 / t457 / ABI 两侧，4 文件 +111/−24）+ `1bea38b9`（`receiver_has_typed_route` 判据 +24 / t458 / ABI 两侧，4 文件 **+72/−24**）。**第二笔是自己抓自己的回归**：第一版抢掉 batch 288 的 2 个 PyJson 活点（`py_json_get_default` 4→2）⇒ 运行期形状守卫喊 rc=134，t458 三进制实拍锁住（pre 绿 / post2 红 / post4 绿）。两条判据负结果入册：① 动态键放开＝静默打 default（宁留 abort）②`DataFrame::get` 那 2 点改前也落 bare `@get`（`pylib/pandas.z` 无 `def get`、`nm` 无 `_DataFrame::get`）⇒ 非回归、语义未判 | **rc=1**（`/tmp/b421/gate_guard.txt:150`，逐条排查后唯一红源＝`run_all.sh:605 py_fail=2` 常驻两例）· **340/2/6/0 ＝ 348**，与 `ls tests/python_style/t*.z` 348 逐字对上（净增 2＝t457+t458，零回归）· 194/194 compile（191/194 link，3 条 link-only 点名）· 语料 40/40 · jit 176/**545** segv=0 · diff 120/130 92.3% bad 0 · knob 23/swallow 6/import 22/empty_stmt 68/pysrc 42/cli_semantics 73/ignore_rules 19/mbvar 22 全 0 违规 · comment_drift 0 · emit_stable 2 夹具/0 · clean_checkout rc=0 rev=`04a6aeeb` · ABI 终态 23/11/10 rc=1 定位失败 0（两次 `--rebind` 各 11 行/13 行，ABI.md 仍 1032 行）· **主线位移 0（正证据）**：pre 135/135/120/135 rc=139 ↔ post4 同读数、末条日志同行 ⇒ 仍卡 `str_trim`（#145）· disclosure：门禁首行 W2003 喊 `tokio_runtime.o`（` M`，非本批产物）比 `.c` 新 ⇒ 运行期目标文件是改前的；注释订正后重编，`zetac` md5 逐字节不变（`b5f1abc6…`）⇒ 读数对应 `1bea38b9` 源码 | 代码批已提交 `04a6aeeb`+`1bea38b9`；记录批随后 |
| 422 | bootstrap | 幽灵桩**逐名点名**（2.3 可观测性）：419 的桩按 argc 共享 + 全仓抛掷恒 code 1 ⇒ 一次跑答不出"这个调用点抛没抛"。改为每个 ghost 一个 `zeta_dyn_missing_<名>_<argc>` → 新 `zt_dyn_member_missing`（`unavailable_stubs.c:81`，点名一次、**每次调用都抛**、不 abort ⇒ 419 契约一字未改；`codegen.rs:3064` + 两个调用点 `:2744`/`:2966`）。代码批 `8c5b9f79`（6 文件 +192/−85，含 `tokio_runtime.o` 重建与 ABI 两侧 36 行重绑）+ 新用例 t459（48 行，钉"去重只吞消息不吞抛掷"：5 抛/2 点名/6 行 stdout 逐字同改前）。**定价结论＝位移 0，该族从头名摘掉**：语料 6 桩点名后自报身份 —— `data_cleaning.py:302`(`isin`/`copy`)+`:304`(`copy`) 的宿主 `clean_market_ohlcv` 全 IR **calls=0**（`market_panel.py` define=0）、`bt_helpers.py:62`(`dropna`)/`:77`(`ffill`) 唯一调用方是 `__run_backtrader`（`backtrader_engine.py` define=0）、`jq_wufu.py:455`(`to_dict`) 2 个调用点全在**午间** routine；改后二进制两次跑 `rc=139 errlines=135` 且点名行 **0**。**三条订正**：①419/420"头名＝`copy`×4（`jq_wufu_local.py:166/190/248/301`）"作废（那 4 个绑 pylib 真定义，`call @"DataFrame::copy"` 语料 37 处，裸名 `@copy` 的 define 反而 0 调用点）②§1 旧句"`ffill`/`isin` 归零"不准（各 1 幽灵；`dropna`×15 是静态绑定数）③"bt_helpers.py:67 的 copy 是幽灵"订正为真绑定。方法坑入册：带 `::` 的 LLVM 符号在 IR 里带引号（`@"DataFrame::copy"`），`@[A-Za-z0-9_:]*` 搜族名＝假负。真修＝掩码族（`zt_vec_cmp` `py_additions.c:1013`，#112）⇒ 等授权 | **rc=1**（`/tmp/b422/gate_guard.txt:149`，唯一红源＝`run_all.sh:605 py_fail=2` 常驻两例）· **341/2/6/0 ＝ 349**，与 `ls tests/python_style/t*.z` 349 对上（净增 1＝t459，零回归）· 194/194 compile（191/194 link，3 条 link-only 同 421 逐字未动）· 语料 40/40 · jit **176 ok 未回退 / 546 total / trap 370** segv=0（+1/+1＝新用例；实拍归因：t459 无 `-o` rc=1，首个硬缺绑 `zeta_module_decl` E4016，未绑 36 符号清单与 t455 同一份 ⇒ #42 既有族，非本批新形态）· diff 120/130 92.3% bad 0 · knob 23/swallow 6/import 22/empty_stmt 68/pysrc 42/cli_semantics 73/ignore_rules 19/mbvar 22 全 0 违规 · comment_drift 0 · emit_stable 2 夹具/0 · clean_checkout rc=0 rev=`610ad28d` · 诊断 official 2/5、python_style **231 行/108 文件**（421＝226/107，+1 文件＝t459）· ABI 终态 **28/11/10 rc=1 定位失败 0**（改前对照在隔离 worktree 取 HEAD 自基线＝23/11/12 rc=2，那 2 条是 `aliases.inc.c` 生成文件不在裸检出里的既有红；本批 rebind 判搬家 36/拒改 38、改写 ABI.md 36 行 68 数，`codegen.rs` 漂移 13→18＝同形多命中拒猜，ABI.md 仍 1032 行）· disclosure：本次门禁首行**没有** W2003 ⇒ 三套基线链的就是本批重建的 `tokio_runtime.o`；`md5 zetac`＝`1aa05f8f…` 与 A/B 所用 `zetac_b422` 逐字节相同 | 代码批已提交 `8c5b9f79`，记录批随后（本行随记录批入库） |
| 423 | bootstrap | dict 原语先把**句柄判形**再用（G.5d）：`map_resolve`（`runtime/tokio_runtime_stub.c:264`）出口加 `zt_map_cap_ok`（首字＝2 的幂 ∧ `cap∈[16,2^30]`；`map_new` 从 16 起、`map_insert` 只翻倍 ⇒ 真表首字必是这个形状），不合格走 `zt_map_not_a_map` 点名 + `zeta_raise(1)`，**不再解引用**。通用探针 `zt_c_str_readable` 改名 `zt_ptr_readable`（9 个调用点跟着搬）。去重按 (原语名, 句柄) 只吞消息、不吞抛掷（422 合同）。代码批 `9c192241`（5 文件 **+135/−30**，含 `tokio_runtime.o` 162,772→**163,748** B、ABI 两侧 9 行 16 数重绑 + C9 行手工补正）+ 新用例 t460（53 行，改前 run=**139**/stdout 0 行实拍 → 改后 rc=0/stdout 四行逐字如 expect）。**主线 301 二十批来第一次位移（有正证据）**：语料两跑一致 `run 139 → 0`、stderr 135 → **357** 行、**38 条点名**（37 个 `py_map_items` 的接收者＝2024-01-02~02-29 的**交易日日期整数**，句柄 19724…19782 跳过周末 19728/19729；1 个 `map_get` 收 8 字节 ASCII 浮点文本串）／74 次抛掷 ⇒ 抛掷没被吞。**但 rc=0 ≠ 修好**：仍 `1000000 -> 0 (-100.00%)`、74 条 `[ERROR] … 失败: 1`。两条订正入册：① 421 的"位移 0 正证据"只比了 `errlines` 相同 ⇒ 判据换 `符号+偏移`＋rc（行数相同崩点可搬家）；② 本批上轮自记的"崩点从 `map_resolve+8` 搬到 `map_get+256`"**作废**（lldb 实拍打在前一代同名二进制上；当场复测：422 那份 `dc1d2e90` 崩在 `map_get+256`，守卫版 `3fe2b270` lldb 下 status=0）⇒ 只存在"同一份代码里两处未判形的解引用"。新坑：实拍文件与二进制同批产出并带 md5 | **rc=1**（`/tmp/b423/gate.rc`、日志 `/tmp/b423/gate.txt` 148 行，唯一红源＝`run_all.sh:605 py_fail=2` 常驻 `t231/t233`）· **342/2/6/0 ＝ 350**，与 `ls tests/python_style/t*.z` 350 对上（净增 1＝t460，零回归）· 194/194 compile（191/194 link，3 条 link-only 同 422 逐字未动）· 语料 40/40 · jit **176 ok 未回退 / 547 total / trap 371** segv=0（+1/+1＝新用例，仍是 #42 那一族既有形态）· diff 120/130 92.3% bad 0 · knob 23/swallow 6/import 22/empty_stmt 68/pysrc 42/cli_semantics 73/ignore_rules 19/mbvar 22 全 0 违规 · comment_drift 0 · emit_stable 2 夹具/0 · clean_checkout rc=0 rev=`300d9e2e` · 诊断 official 2/5、python_style **231 行/108 文件 ＝ 与 422 逐字相同**（t460 的点名是运行期 stderr，不产生编译期告警）· ABI 终态 **漂移 28 / 新 11 / 消失 10 / 定位失败 0，rc=1**（改前对照 37/11/10；`--rebind` 判搬家 38 项，`tokio_runtime_stub.c` 剩 1 条＝同行号不同长度区间引用被拒改）· disclosure：门禁首行无 W2003 ⇒ 三套基线链的是本批重建的 `.o`；`md5 target/release/zetac`＝`1aa05f8f…` **与 422 逐字节相同** ⇒ 位移纯在运行时侧，与 codegen 无关 | 代码批已提交 `9c192241`，记录批随后（本行随记录批入库） |
| 424 | bootstrap | **`.items()` 的接收者为什么是一个日期整数 —— 机制换对了、语料那一格没够着**（2.2 签名表＝调用点证据的绑定点）：`gen.rs:11461` 的 `suffixed` 给**每一条裸名调用**追加 `_实参个数`，而 `refine_param_types`（`src/main.rs:80`）只认「精确名」与「`模块__名`」两形 ⇒ 一条候选都找不着 ⇒ 未注解形参保持 `dyn` ⇒ codegen 的 FieldAccess 落 `("", 2)` 兜底、`codegen.rs:6700-6708` 又把越界索引**夹回 0** ⇒ 三字段结构体读成 word0、第二跳从整数身上按指针读＝SIGSEGV（`ZETA_DBG_FA` 两行实拍在案：`field=portfolio variant="" field_count=2 base_ty=Some(PyDynamic)` + `idx 2 >= count 2 -> fallback 0`）。修法＝`src/main.rs:183-207` +19 词干兜底（只在无精确条目时走、只认 `name == stem`、重载集 `show_1`/`show_2` 绑定一字未变）。代码批 `06c67cda`（4 文件 +74/−9）+ 新用例 t461（改前 compile=0 run=**139** stdout **0 行** → 改后 rc=0/四行，stdout 与 `python3` 当场对数逐字相同）。**家族尺子（两把）**：`jq_wufu.py` 单编成根模块 301 条目/2,518 条调用 → 44 个后缀目标/82 站点无条目，词干==裸条目 **32 个目标/46 站点/24 宿主**；语料整体 628 条目 → 87 个后缀目标（有精确同名条目的 **0 个**）、新绑 5、词干落限定条目尾的 6 个按边界**不绑**、76 个查无条目。**主线位移 0（三层正证据）**：`--emit-llvm` 两侧 md5 逐字节相同 `2997d4f5212568acccf6dcb72fb4abfd`（113,722 行、diff 0）；MIR 唯一 hunk 是 `BaoStockSource` 的模块前缀别名，而**同侧两跑差 2 个 hunk**＝#9 噪声底；运行期改前/改后各两跑读数逐字相同（357 行／PY-A 38＝37 `py_map_items`＋1 `map_get`／WARNING 107／`[晨间]` 37 逐日）。**语料侧这一族的规模第一次有数**：`ZETA_DBG_FA=1` 全量编译 529 条 FieldAccess 里 **28 条走了夹回 0**（`portfolio`×8 base＝`PyDynamic`＝动态 context 那一形、`positions` 另 7 条 base＝`I64`、`avg_cost`×5、`paused`×3…）；37 条点名的句柄是 37 个互不相同的连续整数 `0x4d0c…0x4d46`、跳过 `0x4d10/11` 等周末 ⇒ 逐日 +1 的交易日序数（`0x4d0c`＝19724＝夹具那个 `dt` 的来源），**但「28 处里哪一处 emit 了这 37 次点名」未证**（点名句仍无用户函数名）。**订正 423 §八.1**：「槽位复用/串号」判死。**头名换格**：① `("", 2)`＋索引夹回＝把「缺证据」放大成内存不安全的那一级（出声 vs 补 variant 判据两方向，423 的 precedent 是出声直接换来 rc 139→0）② 回调／函数值实参的参数证据（①的上游根因；census＝628 条目/625 个不同调用目标名、**55 个宿主发 `FuncAddr`**、被取地址的目标 **106 个**、其中 **103 个从不出现在任何调用点**＝97 闭包＋**6 个具名日程例程** `morning_routine`/`afternoon_routine`/`buy_routine`/`sell_routine`/`reset_daily_flags`/`check_weak_period_daily`，正是 `run_daily(<fn>, time=…)` 注册那批 ⇒ 被调度执行的函数形参恒 `dyn`）。尺子级负结果两条：`--dump-mir` 的打印（`main.rs:897`）在精化（`:905`）**之前**＝这条路照不到；emit-llvm 的 md5 基线必须同 cwd 同环境（漏 `REPLAYQUANT_LOCAL` 会得到另一颗 md5＝数据分支，不是非确定性） | **rc=1**（`/tmp/b424/gate.txt` + `gate.rc`，唯一红源仍是 `run_all.sh:605` 存量 `py_fail=2`＝t231/t233）· python_style **343/2/6/0 ＝ `ls t*.z` 351** 逐字对上（净增 1＝t461，零回归、known-fail 6 未动）· 194/194 compile（191/194 link，3 条 link-only 同 422/423 逐字未动）· 语料 40/40 · jit **ok=176 未回退 / total 548 / trap 372** segv=0（+1 total＝新用例进 sweep）· diff 120/130 92.3% bad 0 · comment_drift 0 · emit_stable 2 夹具/违规 0 · clean_checkout rc=0 rev=`ff5514e4`（＝门禁起跑时 HEAD，代码提交在其后）· 诊断 official 2 文件/5 行、python_style 231 行/108 文件＝与 422/423 逐字相同 · ABI 终态 **漂移 28 / 新 11 / 消失 10 rc=1 定位失败 0**（`--rebind` 改写 3 行/5 数：`main.rs` 五锚集体 +19，另 **1 条表格内裸引用手工成对搬** `main.rs:568`→`587`；ABI.md 仍 1032 行）· disclosure：本文件出现**同文件并发编辑**（§1 被整体重排、423 台账行被删、并新增 `## 5` 标题修 HEAD 里那张无名表格）⇒ 台账行按 `git checkout HEAD -- worktree.md` 复原后合并，`## 5` 标题保留 | 代码批已提交并推送 `06c67cda`；记录批随后（本行随记录批入库） |
| （续） | | | | |

## 5. Agent-2 旁路台账（ZCode 维护；cleanup）

| 批次 | 分支 | 主题（族 / 靶） | 门禁读数 | 状态 |
|---|---|---|---|---|
| 440（计划） | cleanup | 反向第一族：`static mut` 局部声明（丢行最多 357，判据已钉：`unsafe {}` 单喂可解析） | 待跑 | WORKING |
| （续） | | | | |

## 6. 旁路侧启动清单（一次性 / 每会话）

```bash
# 一次性（已完成则跳过）
git worktree add ../zeta-bz -b cleanup bootstrap
cd ../zeta-bz && cargo build --release
mkdir -p /tmp/zeta-bz-tmp

# 每会话
cd ../zeta-bz && export TMPDIR=/tmp/zeta-bz-tmp
git log bootstrap --oneline -3     # 查主线进度
git rebase bootstrap               # 拿到主线新批次（提交前）
# 构建/重门禁加 nice；全量门禁与主线错峰
```

## 7. 合并记录（主线侧维护）

| 日期 | merge 点 | 带入族/批次 | 备注 |
|---|---|---|---|
| （暂无） | | | |
