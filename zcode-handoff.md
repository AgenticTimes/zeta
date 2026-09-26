# zcode-handoff.md — 旁路 Agent（ZCode）任务交接

> 写给：下一个接管本车道的 ZCode 会话。
> 前任：本会话（批次 462–513，约 77 个提交）。
> 读完本文件即可接手，无需历史对话。

---

## 一、你是谁、在哪工作

| | 说明 |
|---|---|
| 角色 | **Agent-2 旁路**（反向车道：测试/普查/工具/文档 + parser 车道代码修复） |
| 工作区 | `/Users/meetai/source/zeta-bz`（git worktree，分支 `cleanup`） |
| 主树 | `/Users/meetai/source/zeta-src`（bootstrap 分支，**Agent-1 主线 qwen 的地盘**，你只读+按协议合并） |
| 推送 | `git push --force-with-lease agentic cleanup`（只推 cleanup；**bootstrap 的推送权在主线**） |
| 批次号段 | 514+ 续用（462–513 已用）；测试号段 t5xx 续用 |

**必读文件**（按序）：`AGENTS.md`（两班协作协议）→ `worktree.md`（状态板+台账）→ `roadmap2.md`（生效优先级源）→ `CONTEXT.md`（领域词汇——写文档/提交说明措辞对齐它）→ 本文件。

## 二、任务目标

1. **给主线建语义正确性普查网**（G.3 轴的进阶档）——已完成并稳态运转；
2. **普查发现 → 定性 → 移交主线 → 基线闸门盯防 → 修复后自动转绿** 的闭环——已跑通多轮；
3. **我车道内的代码修复**：parser（pattern.rs / stmt.rs 模式相关臂）、tests、tools、docs；
4. **不碰**：gen.rs、resolver.rs、codegen.rs、main.rs、runtime/*.c、pylib.rs、docs/ABI.md、tools/baselines/**（主线车道，除非台账报备且主线确认——先例：496 的 registry 数据行、464 的 std.rs GC 行）。

## 三、已建成的资产（都在 cleanup 分支，已推远端）

| 资产 | 位置 | 用途 |
|---|---|---|
| 随机普查生成器 | `tools/gen_random_diff.py` | 十五模式（numeric/str/stmts/list/dict/cmp/loop/fmt/slice/builtin/control/dmethod/nested/methods/class），`--seed` 可复现，`--mode` 选面；CPython 先验滤 bad_case |
| 差分判定器 | `tools/diff_test.py` | CPython oracle vs zeta，逐用例 verdict，基线闸门（match_min 只许升）；`DIFF_JOBS=1` 串行开关 |
| 差分基线 | `tools/baselines/diff_consistency.json` | match_min=316；**转好要 --bless 抬闸**，变差必查 |
| 排障工具增强 | `tools/parse_bisect.py` | 无分号方言回退定位（bisect_line_wise） |
| known-fail 钉子 | `tests/python_style/t501–t512` | 普查各族的最小判据，修复后自动 XPASS 转绿 |
| 移交简报 | `docs/HANDOFF-CENSUS-FAMILIES-2026-09-26.md` | **九+三族一屏**：最小复现/期望/现状/修复位置/闸门——主线修复的作业指导书 |
| 业务规则清单 | `docs/business-rules/pipeline-rules.md` | BR-xxx 编号，写台账直接引用 |
| 协作文档 | `AGENTS.md` / `worktree.md` / `roadmap2.md` / `CONTEXT.md` | 分工/台账/优先级/词汇 |

## 四、当前状态（2026-09-27，批次 525 收口）

- **主线**：qwen 在批次 452+（W1011 字段槽位出声 + 动态下标文本臂 + 元组槽位类型标记——**已修复我 505/506 发现的两个缺口**）。497–506 已并入，497–524 待合并。
- **cleanup 领先主线**：~7 提交（497–524 的台账 + 工具改动）。
- **差分一致率**：88.6%（match=406/458，缺口率稳态 12%）。基线已 bless 到 match_min=406。
- **门禁基线**：python_style 376 过/2 红（t231/t233 存量）/14 known-fail/2 XPASS；official 194/194；语料 40/40；jit 180/0。
- **已修复的普查族**：⑭ % 格式化（我 523 修）、① `/` 真除法（qwen 454 修）——XPASS 确认。

## 五、悬而未决（按优先级）

### A. 等主线修复的（有简报有闸门，不用催）
`docs/HANDOFF-CENSUS-FAMILIES-2026-09-26.md` 十二族：溢出回绕、`/` 真除法、sign 旗标、负数进制、Bool 性、None 打印、元组交换（#190）、容器比较、find 两参 shim、None 逻辑链、**类方法字符串族（⑫，最新最宽）**。每族闸门用例自动盯防，主线修好自动 XPASS。

### B. 你可以立刻做的
1. **钉 ⑫ 的 d1/d2**（简报里标了"未钉"）：按 t510 格式写 t513/t514，类方法字符串族进 known-fail；
2. **组合普查第三波**：类方法 × 推导式 × 循环 × 字符串的更多交互矩阵（先例：511/512 两战两捷，探针文件 /tmp/b440/b467/d1.z、d2.z）；
3. **新种子普查**：`for m in ...; gen_random_diff.py --mode $m --seed <新>` → 临时目录 → diff → 族分布读数（临时样本**不进库**）；
4. **XPASS 值守**：每次 python_style 若出现 XPASS = 主线修好了某族 → 摘钉 + 台账记批次。

### C. 别做的（裁定在案）
- #52 锚点归属（用户裁定不改——集合冻结、按需归属）；
- 解析器侧元组交换脱糖（两轮否证，#190 正解唯一在 gen.rs）；
- gen.rs/resolver/codegen/main.rs（车道）。

## 六、批次纪律（每批必须）

1. `git rebase bootstrap` **先于一切**；**rebase 后必须 `cargo build --release` 重建**（陈旧二进制会让你取到假读数——494b 的教训）；
2. 探针 → 定性 → 修复（我车道）或移交（主线车道带最小复现+修法草图）；
3. 门禁切片：动了 parser → python_style + cargo test；动了 tools → 该工具自身；改了行为 → 全量（≤10 提交一次， Discipline）；
4. **双提交**：代码批 + `worktree.md` 台账行（fix 与 docs 分开 commit）；
5. `git push --force-with-lease agentic cleanup`（rebase 后必须 force-with-lease）；
6. 提交说明措辞对齐 CONTEXT.md 词汇（响亮失败/静默错值/槽/位型……）。

## 七、踩过的坑（每条都有事故编号）

1. **rebase 后不重建 = 假读数**（494b：误报 str_repeat_left 回归，实为陈旧二进制）；
2. **GC 地址熵**：错误值里嵌堆地址的 mismatch detail 天然不可复现（482b 定性）——verdict 稳定即可，detail 已做 `<N>` 脱敏；
3. **转义层错位**：往工具里插含 `\n` 的代码时，bash heredoc → python → 目标文件要过两层转义——用 `chr(10)` 避开（508 事故）；
4. **并行套件偶发抖动**：重负载下 python_style 单例可能假红（507 的 t478）——复跑三遍再定性，静默复现才升级；
5. **gql 忘了 add 生成器本体**：474/476/477 曾只提交用例不提交生成器（可复现性漏洞，479a 补）——**新增模式时工具与用例同批提交**；
6. **tmp 固定路径竞态**：会话内 `export TMPDIR=/tmp/zeta-bz-tmp`（AGENTS.md 铁律）；
7. **两班并发跑全量门禁** = 机器饱和假红——错峰，或我方只跑切片。

## 八、与主线的协作事实（供参考）

- qwen 每 1–2 小时一批，合并勤劳（493–496、503–506、497–506 十批均已并入并代录台账）；
- 我方移交已落号：#188（py_format 两面 + find 余半）、#189（None 打印）、#190（元组交换，含两轮解析器否证备案）；
- 合并冲突史：worktree.md 曾发生双班并发编辑（424 期），规则已写进 §3——**重排/恢复共享文件必须保留对侧段落**；
- 台账惯例：每批一行五字段（批次/分支/主题/读数/状态），写进 `worktree.md` 对应 section。
