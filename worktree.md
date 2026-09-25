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

- **正向队列（Agent-1）**：roadmap2.md §2 主链的「下一批候选」，按实测损害量序。当前（批次 418 后）：
  **419＝晨间例程体崩点 A/B**（出码已钉死 ⇒ 前置具备；崩点 5/5 稳定在 134 行后 rc=139，末行 `[晨间] 计算流动性阈值`）
  → **`rows` 同二进制跨跑抖**（5 跑 4 值 12854–12858，运行期/库面侧非 codegen，是主线 301「数量不一致」最近的一格）
  → 名字→字段全局扫描选错 struct（先补一条 variant 为空、真打进 `codegen.rs:6535` 的夹具）→ #142（`gen.rs:3987` 读点未守卫）
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
| 418 | bootstrap | 出码非确定性两处落地：`codegen.rs:1606` alloca 序排序 + `:66` `struct_defs`→`BTreeMap`（代码批 `0f5bec05`，7 文件 +291/−161）；门禁第 16 步 `tools/emit_stable.sh` 上门；**两条负结果入册**：两个夹具盯不住源 B（把 BTreeMap 退回 HashMap 照样全过）、先前口误的 ABI 读数实为 **rc=2**（裸行号＝#52 的账） | **rc=1**（`/tmp/b418/gate418.log`，唯一红移到 `run_all.sh:605` 存量 `py_fail=2`）· emit_stable 2 夹具/违规 0 · 336/2/6/0 · 194/194+191 · 40/40 · jit 176/541 segv=0 · diff 120/130 92.3% bad 0 · clean_checkout rc=0 rev=`738618b8` · ABI 32/11/3（`--rebind` 77/77×2） | 代码批已提交 `0f5bec05`；本记录批随后推送 |
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
