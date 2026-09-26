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

实测 **208s = 3'28"**（省下 458s），且保留步的读数与全量逐项相同（official 194/194·191/194、python_style 352 passed/2 failed/6 known-fail/0 xpass、238 warning 行/112 文件、dyn_binding 4 条不一致 0）。`GATE_RC=1` 的存量红源两侧相同＝`t231_dict_set_cast_fromkeys`、`t233_listcomp_condition_capture`（快门禁不改变 rc）。

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
