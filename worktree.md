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

## 1. 两条工作队列（正着 / 倒着）

- **正向队列（Agent-1）**：roadmap2.md §2 主链的「下一批候选」，按实测损害量序。当前（批次 422 后）：
  **头名＝`market_panel.py` / `backtrader_engine.py` 两个模块进不了编译闭包（语料 IR define 数＝0）** —— 422 点名的副产品：`clean_market_ohlcv`（3 个幽灵桩的宿主）全 IR **calls=0**，唯一调用方就在 `market_panel.py`；`_add_pandas_data` 的唯一调用方是 `__run_backtrader`，而 `backtrader_engine.py` define＝0 ⇒ 晨间/本地回测链路少了哪些模块、是不是同一格拦路，**未定价**（判据现成：`--emit-llvm` 的 `^define` 计数 × 模块名，配 `--dump-mir` 项数对照）
  → **第 2 格＝`str_trim + 24` 读 `x0=0x3532`**（419 的崩点，420/421/422 三批 A/B 复测仍在 135 行、**位移 0 有正证据**：改前二进制仍死在 `fetch_stocks`。生产者三条负结果入册，真修大概率要动 `runtime/py_additions.c` ⇒ **等用户授权**，未获授权前不作为头名）
  → **第 3 格＝#134 `GroupBy.__len__` 恒 0 / #117 动态键 `.get(k, default)` 语料 7 形仍落 `_get`**（都不等授权）
  → **`[dynamic]` 幽灵名族（422 定价完，从头名摘掉）**：6 个具名桩点全在晨间崩点**之后**或死代码里，语料跑点名行 0 ⇒ 位移 0。逐名读数（`copy`×2 在 `data_cleaning.py:302/304`、`dropna`/`ffill` 在 `bt_helpers.py:62/77`、`isin` 在 `:302`、`to_dict` 在 `jq_wufu.py:455`）**订正** 419/420 的"`copy`×4 头名"与 §1 旧句"`ffill`/`isin` 归零"；真对齐＝掩码族（`zt_vec_cmp` 在 `runtime/py_additions.c:1013`，任务 #112）⇒ 等授权，不在语法面补
  → **`rows` 同二进制跨跑抖**（5 跑 4 值 12854–12858，420 又一次读数 119/135 位置的抖；运行期/库面侧非 codegen）
  → 名字→字段全局扫描选错 struct（先补一条 variant 为空、真打进 `codegen.rs:6665` 的夹具；418 记的 `:6535` 已因 418/419 两次加行移到这一格）→ #142（`gen.rs:3987` 读点未守卫）
  → #134（`GroupBy.__len__` 恒 0）→ #117/#118 打印族 → #136 漏点族定价。
- **反向队列（Agent-2）**：从债务尾巴向回收，队列固定，两条子队列依次：
  1. **截断十族按丢行量降序**（docs/ABI.md 附 B#10，批次 325 状态）：`static mut` 局部声明 357 → `s[i..]` 开区段 230（任务 #39）→ selfhost 158（未定位，先出定价批）→ 体内 `use a::b::C` 85 → 块体闭包实参 58 → or 模式构造子 62 → `for i: usize in` 36 → 单段 `import pkg;` 19 → 常量表达式数组 14；
  2. **backlog 最老 OPEN 项**（按编号升序，S/M 优先）。
- 两条队列一个吃「当前损害」、一个吃「历史欠账」，文件面不相交（见 §2），天然无冲突。

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
