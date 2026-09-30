# AGENTS.md — Agent 工作约定（每次会话开工前必读）

本仓库常驻**两个并行 agent**，靠 worktree 分支 + 文件所有权隔离协作。开工前先确认自己是谁、在哪个工作区。

## 两个 worker

| | Agent-1 主线（qwen） | Agent-2 旁路（ZCode） |
|---|---|---|
| 工作区 | 仓库主树 `/Users/meetai/source/zeta-src` | `../zeta-bz`（git worktree，分支 `cleanup`） |
| 方向 | **正向**：按 roadmap2.md「下一批候选」的损害量序向前推进（当前 = 主线 301 根因链） | **反向**：从债务尾巴向回收（截断族按丢行量降序 → backlog 最老 OPEN 项） |
| 分支 | `bootstrap`（**唯一推送者**） | `cleanup`（只在自己的分支提交） |
| 批次号段 | 414–439 | 440–459（合并时按实际时间归位誊入 roadmap.md） |
| 测试号段 | t45x–t49x | t50x 起 |

## 工具偏好

- 查看/理解代码时，**优先用 `codegraph`**（符号级结构查询、调用图、影响面），比 grep + 读文件更快且上下文更准。

## 语言纪律（2026-09-30 用户裁定：禁止黑话，禁止英语专有词直译中文）

写台账、提交信息、汇报，一律用平实中文。两条硬规矩：

1. **禁止自造行话**。不说"跑电池""撒布""钓中""抬闸"这类圈内黑话，直接说做的事：跑差分测试、批量生成用例、发现输出不一致的用例、把通过数固化成新基线。
2. **禁止英语词直译**。`test battery` 直译成"电池"、`poison` 直译成"毒票"这类做法是错的。有通行中文术语的用中文；没有通行译法的**直接用英文原词**（如 MIR、LLVM、`--bless` 命令参数），首次出现附一句解释——不许硬造直译词。

踩过的实例（新写文档对照自查）：

| 错误写法 | 应写成 |
|---|---|
| 跑电池／电池跑到一半 | 跑差分测试／差分测试跑到一半 |
| 撒布 | 批量生成（用例） |
| 钓中／钓真 bug | 发现了一个输出不一致的真实缺陷 |
| 毒票 | 无法推断返回类型的分支 |
| 抬闸 | 把基线提高并固化 |
| 错窗采样 | 读错了结构偏移位置的内存字 |

边界：仓库既有术语（CONTEXT.md 词汇表、历史台账行）不回改；命令名、文件名、代码符号原样保留；`known-fail`／`mismatch` 等已在测试脚本里作为字面量输出的词可原样引用。

## 开工三步

1. 读 `roadmap2.md`（生效优先级源）→ 本文件 → `worktree.md`（状态板与台账）。
2. 确认角色与号段；查看对侧进度：
   - 主线侧：`git show cleanup:worktree.md`（读旁路分支上的状态板）与 `git log cleanup --oneline -5`
   - 旁路侧：直接看自己工作区副本 + `git log bootstrap --oneline -3`
3. 只在 `worktree.md` 所有权矩阵授权的范围内动文件。

## 每批收尾时必做（两班都一样）

每批提交完（此刻工作区必干净），主线侧顺手做一次同步：

```bash
git log cleanup --oneline -3     # 看一眼旁路做了什么
git merge cleanup                # 有分叉就是一个小合并提交，文件面不相交所以不冲突
git push agentic bootstrap
```

旁路侧在主线合并后：`git rebase bootstrap`（拿到新代码继续干活）。
只有一条安全规则：**不在批次中间合并**——合并永远发生在"刚提交完自己批次"之后。
## 提交节奏（2026-09-29 用户裁定：提交代码为主，文档附带）

**一批的"主体"必须是真实代码改动**（`src/**`、`runtime/**`、`tools/**`、`tests/**`）。`roadmap.md`／`worktree.md`／`backlog.md` 三本台账只是**随批附带的记录**，它本身不构成推进——一笔只动这三个 md 的提交等于零推进。

四条判据（都有事故实证）：

1. **顺序固定：实现 → 验证 → 先提交代码 → 再补记录。** 记录批要小，不要在等门禁的几十分钟里让工作树保持脏。事故：批次 645–651 连续 7 批只写台账（最近 40 笔提交仅 4 笔含代码），同期 `src/` 一行未改。
2. **同一族连续 2 批零代码 ⇒ 下一批必须是真修批**，除非有实测障碍（需用户授权件、需方言裁决），且障碍要写进该批记录。事故：650 立了这条规矩，651 自己违反了它。
3. **不许用"这一回合做不完真修"当理由改挑做得完的记录批**——那是挑舒适区（记录批零风险、锚点核对 4 秒必过），不是约束。真修做不完就把它停在已验证的干净状态上，别拿记录批凑数。
4. **真修失败要留证并如实报"零落地"**：补丁存 `/tmp/b<NNN>/patch_<NNN>.diff`、红读数存 `gate_regress.log`，回退后必须复验被测二进制 md5 等于在册的改前值（事故实证：批次 642、652 两次真修尝试都打过再回退，回退不复验＝可能拿旧二进制读数当新改动画）。

汇报推进时用 commit hash + `git diff --stat` 当证据；只叙述"正在推进/在等门禁"不算。

## 用 codegraph 理解代码（同上裁定：多用）

结构问题（谁调用它／改了会炸到哪／符号落在哪）**先查图再 grep**，grep 只作文本证据；结论必须带 `file:line`。

- **改完代码先 `codegraph sync` 再查询**，否则查到的是旧图（这是最常见的错因）。
- 定位修法前：`codegraph callers <符号>` 定生死（空结果先 `query` 确认符号存在再下"零调用"结论）、`codegraph impact <符号>` 量半径、`codegraph affected <文件>` 取受影响测试集——用它们替代"grep + 读整个文件"，省下的上下文留给真改动点。
- 跨语言／宏生成的边可能缺（Rust→C extern、`runtime/*.c` 互调）：结论写"图内无边"，要绝对结论时补一次 grep 复核。
- 查询异常先 `codegraph status` 看索引健康（文件数骤降＝索引坏了），再考虑重建索引。

## 铁律（两边的红线）

- `bootstrap` 的 push 权只在主线侧；旁路侧的 rebase 与合并由主线侧在批间窗口执行。
- `docs/ABI.md` 与 `tools/baselines/**` 在旁路侧**只读**（含 `check_abi_anchors.py --rebind`）；旁路的清单更新在合并时由主线侧代录。
- 全量门禁（`tools/run_all.sh`）两侧**不并发**；旁路侧构建与重门禁加 `nice`。
- `/tmp` 固定路径有竞态（`/tmp/zeta_baseline.json`、`/tmp/zt_*.o`）：旁路侧会话 `export TMPDIR=/tmp/zeta-bz-tmp`。
- 批次纪律不变：fix + docs 双提交、独立可回滚、禁攒批捆提交、门禁读数入册。

## 门禁节奏（2026-09-26 用户裁定：全量每 10 批一次）

**每批只跑与本批改动相关的步骤；全量门禁每 10 批跑一次**（批次号走到 10 的整数倍那批的收尾：440、450…）。跑子集不是"少验收"，是把同一批该跑的步骤挑对：全量 17 步里与本批改动面无关的步骤，跑了也只是重复已知读数。

按改动面路由（左＝本批动了什么，右＝必跑）：

| 改动面 | 必跑步骤 |
|---|---|
| `src/middle/**`、`src/backend/**`（下型/出码） | `official` + `python_style` + 新增/改动的夹具（pre/post 两颗二进制）+ **主线 301 位移 A/B** |
| `src/frontend/**`（解析） | `official` + `python_style` + `corpus`（`tools/corpus_baseline.py`） |
| `tools/**`、门禁自身 | 该工具自己的判据脚本 + `python_style`（防把套件跑废） |
| `runtime/*.c` | `python_style` + `official` + 位移 A/B（改运行期＝跨界面，不信单套件） |
| `docs/**`（含 `docs/ABI.md`） | `tools/check_abi_anchors.py`（只读核对；行号搬家要 `--rebind`） |
| 夹具/用例文本 | `python_style` 单步即可 |

三条护栏（缺一条就会被判"子集跑得不对"）：

1. **位移 A/B 不算门禁**：它是本批的损害量读数，任何动 `src/middle`/`src/backend`/`runtime` 的批次都要跑，不因"非全量"而免。
2. **快门禁期间不并发取读数**：门禁的 `corpus` 步有单文件 30s 预算（`ZETA_CORPUS_TIMEOUT`），并发编译会把一个文件顶超时、并把整步读数吃成 0/0（实测：批次 438 的 10 分钟全量白跑）。
3. **每 10 批的那次全量独占机器**，并把 17 步逐项读数入册；两次全量之间攒下的子集读数不足以证明"没回归"，所以全量批不许并到别的批里收尾。

### 实测耗时（2026-09-26，同一颗 `zetac`，独占机器，逐项戳记 `/tmp/b438/gate_timed.log`、`gate_fast.log`）

| 步骤 | 全量里耗时 | 归属 |
|---|---|---|
| official + 诊断面 | 29–34s | 几乎每批都跑（它是唯一能看编译期回归的步） |
| python_style（352 例） | 174–242s | 动下型/出码/运行期/用例都跑 |
| corpus（40 文件） | 168s | 只有动解析才跑 |
| jit sweep（557 例） | 60s | 只有动解析/发射管线的批跑 |
| truth + diff | 83s | 真值面，全量专属 |
| pysrc / empty_stmt / clean_checkout / import / knob / swallow / sem | 46/20/6/4/4/–/3s | 工具与判据面，全量专属 |
| **合计** | **666s = 11'06"** | |

`src/middle`/`src/backend` 批的快门禁＝`run_all.sh` 已有的 `--skip-*` 开关组合（**不需要新参数，也不动这个脚本**——它的 `:10/:103/:198/:628` 被 `docs/ABI.md` 以裸行号引用，改正文会静默把那些引用挪走，#52）：

```bash
bash tools/run_all.sh --skip-corpus --skip-jit --skip-diff --skip-knob \
  --skip-swallow --skip-import --skip-empty --skip-clean --skip-pysrc \
  --skip-sem --skip-ignore --skip-mbvar --skip-emit-stable
```

实测 **208s = 3'28"**（省下 458s），且保留步的读数与全量逐项相同（official 194/194·191/194、python_style 353 passed/2 failed/6 known-fail/0 xpass、239 warning 行/113 文件、dyn_binding 4 条不一致 0）。`GATE_RC=1` 的存量红源两侧相同＝`t231_dict_set_cast_fromkeys`、`t233_listcomp_condition_capture`（快门禁不改变 rc）。

**口径更新（批次 438 收尾之后，合并树实测）**：`python_style` 与 jit sweep 两步已在批次 461（旁路）并行化，同一条配方在合并树上＝**139s**、python_style **354 passed**/2/6/0（＋1 过＝旁路的 `t501_ref_pattern_match.z`，`ls t*.z` 361→362），其余逐项同上 ⇒ 上面 208s 与表内 `python_style`/`jit sweep` 两格是 461 之前的口径。读数明细见 roadmap.md「旁路并入（2026-09-26）」节末。

批内另外三条省时间的规矩：**先 `--emit-llvm` 单模块比对**再上整程序 A/B（批次 438 实测：一处字段布局的位移在单模块 IR 里两行就看清了，整程序 A/B 一次要 12 对编译）；**某侧确定性失败就别 n≥6**（同一条链接错误重跑六遍只是同一个结论）；`--dump-mir` 的大夹具先测同侧噪声底再按 `== MIR item ==` 归位数真实改动点。


## 文档地图

| 文件 | 角色 |
|---|---|
| `roadmap2.md` | 生效优先级源（M1–M4 收敛阶梯见其 §1–§2） |
| `worktree.md` | 双 worker 状态板 + 所有权矩阵 + 提交台账 + 合并协议 |
| `roadmap.md` | 批次执行日志（主线侧追加；旁路记录在自己的台账，合并时誊入） |
| `backlog.md` | 任务唯一登记点（OPEN ≤ 30） |
| `refactor.md` | 七轴重构计划与方法论 |
| `CONTEXT.md` | 领域词汇表（测试与文档措辞对齐它） |
| `docs/business-rules/` | 业务规则与强制点坐标（BR-xxx 可直接引用） |
| `docs/TEST-DESIGN-2026-09-25.md` | 测试用例设计与 seam 约定 |

## 旁路批次 462–528 经验教训（必读——每条都有事故实证）

| # | 教训 | 事故批次 |
|---|------|---------|
| 1 | **rebase 后必须 `cargo build --release` 强制重建**——cargo 增量检查可能漏检 rebase 带来的源文件变更，陈旧二进制会产出假读数（494b 误报回归、507b 三连假象，同一坑踩两次） | 494b/507b |
| 2 | **共享文件（worktree.md/backlog.md）重排或恢复时必须保留另一班写的段落**——按 HEAD 复原会把对侧行丢掉（424 期实测） | 424 |
| 3 | **并行套件假红**：重负载下 python_style 单例可能超时假红——复跑三遍再定性，静默复现才升级 | 507 |
| 4 | **GC 地址熵**：错误值里嵌堆地址的 mismatch detail 天然不可复现（每次编译地址不同）——verdict 稳定即可，detail 已做 `<N>` 脱敏 | 482b |
| 5 | **往工具插含 `\n` 的代码时注意转义层数**：bash heredoc → python → 目标文件要过两层转义，用 `chr(10)` 避开 | 508 |
| 6 | **新增模式时生成器与用例同批提交**——只提交用例不提交生成器会造成可复现性漏洞 | 479a |
| 7 | **跨车道修文件须报备**——496 修 registry.txt（数据行）、464 修 runtime/std.rs（5 行）均提前报备且合并正常；不报备会撞车 | 464/496 |
| 8 | **rebase 冲突的 --ours/--theirs 方向**：rebase 时 --ours = bootstrap（主线），--theirs = cleanup（旁路）——搞反会取到旧值（484 账目事故） | 484 |

## 差分普查体系（旁路建成，主线可用）

- 生成器：`tools/gen_random_diff.py`（十五模式 × `--depth` shallow/normal/deep × `--seed` 可复现）
- 判定器：`tools/diff_test.py`（CPython oracle vs zeta，基线闸门 match_min 只升不降，`DIFF_JOBS=1` 串行开关）
- 方法扫描：`tools/method_sweep.py`（58 方法可用性清单，当前 8 缺 shim）
- 移交简报：`docs/HANDOFF-CENSUS-FAMILIES-2026-09-26.md`（十三族一屏，含修法草图）
- known-fail 钉子：`tests/python_style/t501–t513`（修复后自动 XPASS 转绿）
