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

## 铁律（两边的红线）

- `bootstrap` 的 push 权只在主线侧；旁路侧的 rebase 与合并由主线侧在批间窗口执行。
- `docs/ABI.md` 与 `tools/baselines/**` 在旁路侧**只读**（含 `check_abi_anchors.py --rebind`）；旁路的清单更新在合并时由主线侧代录。
- 全量门禁（`tools/run_all.sh`）两侧**不并发**；旁路侧构建与重门禁加 `nice`。
- `/tmp` 固定路径有竞态（`/tmp/zeta_baseline.json`、`/tmp/zt_*.o`）：旁路侧会话 `export TMPDIR=/tmp/zeta-bz-tmp`。
- 批次纪律不变：fix + docs 双提交、独立可回滚、禁攒批捆提交、门禁读数入册。

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
