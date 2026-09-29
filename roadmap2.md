# Zeta 编译器路线图 2（当前生效的优先级源）

> 建立：2026-09-25。首版依据：2026-09-24 用户裁定（refactor.md §9）+ 2026-09-25 三份新证据入册。
> **怎么读这份文件**：前一份 `roadmap.md`（1.4MB）是批次 1–409 的历史执行日志，已冻结、只读。
> 从批次 410 起，新的批次记录写在本文 §6；工作优先级也以本文为准。
> 新任务的唯一登记入口是 [backlog.md](backlog.md)（§4 里暂存的待登记项按其规则取空闲编号迁入）。
> 基线门禁（`tools/run_all.sh`）和批次纪律（每批独立提交 + 证据入册）不变。

---

## 0. 文档分工（哪份文件管什么）

| 文件 | 角色 |
|---|---|
| `roadmap.md` | 批次 1–409 的执行日志（历史归档，只读） |
| **`roadmap2.md`（本文件）** | 当前生效的路线图：主链、收尾顺序、批次 410 起的记录 |
| `backlog.md` | 新任务的唯一登记点（OPEN 数 ≤ 30，有记账） |
| `refactor.md` | 七轴重构计划与方法论（状态行待刷新，见 §4-8） |
| `docs/business-rules/pipeline-rules.md` | 业务规则与强制点证据源（可引用 BR-xxx 编号） |
| `docs/DEEPENING-OPPORTUNITIES-2026-09-25.md` | 深化候选 C1–C6（与 refactor.md 各轴的映射见其末节） |
| `docs/architecture-risk/architecture-review.md` | 扩展性风险 R1–R5（轴 D④ 的靶子数字以 R1 为准） |

## 1. 当前快照（2026-09-25，批次 409 收口后）

**主链走到哪了**：类型基础有三步——① 掩码类型 ✅（批次 398 完成）→ ② 跨函数签名表 ✅（批次 399 完成，顺手把 #33 的「无声明半」收敛了，全语料的 R7 判据从 5 行缩到 1 行）→ **③ 最小类型检查，这是下一步**。

**还挂着的一批格子**（按批次 408/409 §十一的盘点）：

- #33 还剩两种形态（声明与实现不一致时按位重读；一元负号 #115）
- #117（动态接收槽没有标记）
- #118
- 闭包参数侧（408 §十一b）
- p2 族（容器元素型）
- **409 (a)①②：`len(cache["k"])` 返回 0 / 参数冲突后 `len([1,2,3])` 返回 1** —— 这是下一批的默认候选，是 BR-R12/R13 证据链的延续；强制点在 `resolver.rs:1655-1735`

**悬而未决的事**：

- 406 §十(b)：C 运行时要不要接进 JIT（已升格为 G.5e 的前置问题，见 §2）
- acceptance 的非确定性判据还没立起来
- `run_all.sh:576` 的退出码口径有两种读法，没对齐（409 §十一f）

## 2. 主链（2026-09-24 裁定原文 + 2026-09-25 增补）

按顺序做，每格做完才进下一格：

```
类型基础③ 最小类型检查（未标注 = 动态值 + 动态值位置清单，2-3 天）   ← 当前在这
  ▼
#38 match 不进中间代码 + #45 格式化丢字（都在 gen.rs 同一段，联合重构，2-3 天）
  ▼
主线批次 301 恢复（现象：0 笔成交 → 选股数量分歧 → 追 final_value，2-4 天）
  ▼ 主线收尾后
F.4 全量迁移（把 parser/MIR/codegen 三处的定型特判拆掉）
B/T3-T5 值模型装箱（边界清单来自 ③；装箱正确性用 final_value 对齐来验证）
G.1 修复 + G.4 消灭降级
轴 C + 轴 D④（拆分 gen.rs；靶子数字以 R1 为准：lower_expr 单函数 10,371 行 / 占 gen.rs 71%）
G.5e 符号注册表（= C1 SymbolRegistry）
```

**增补一（G.5e 的前置问题）**：动 G.5e 之前必须先回答 **406 §十(b)（C 运行时接不接进 JIT）**——这个决定影响 JIT 要绑定几份接口，直接决定 SymbolRegistry 的 schema 要生成几个面。这个问题没回答前，G.5e 排在后面不动。

**增补二（清理期收尾顺序的插入，见 §3）**：CLI 降级链和 ABI.md 补记这一族，插在「截断收尾之后、回主线 301 之前」。

## 3. 清理期收尾顺序（2026-09-23 裁定的延伸）

1. **B 类解析器截断**——✅ **已清零**（批次 440 度量批实测：437 基线上 truncation inventory 0 W1002 / 238 文件；十族在 326–437 间被顺带清完）。清理期第一优先随之解除，转入 backlog S/M 收尾（#81–84 并批）。

2. **截断清零后、回主线前，把 C/D 类并成一个批次闭合**（跑快速门禁），新增三族（详见 §4）：
   - **管线降级链收敛**：borrow 检查结果被整段丢弃（P06）→ W0003 注册 + 一个错误码三种含义拆分（A5）→ CLI 和 lib.rs 两套失败语义对齐（A3）→ E4001 找不到 main 却返回成功（P07）。全部是零行为变更/只改报告，符合「报告先行原则」。
   - **CLI 合同小修族**：env 清单补上 `ZETA_NO_OPT`（A7）、重复 `-o` 参数预扫描取第一个 vs 循环取最后一个的分叉（A8）、bootstrap 吞掉 80 字节输出没人看见（A9）、bootstrap 链接没有 Windows 分派（A10）。挂到既有的 `tools/cli_semantics_check.sh` 门禁下。
   - **ABI.md 补记批次 399**（§4-6）：§3 里的 C2/C3/R7/M5–M7 还停在批次 320/387 时点的状态。

3. **S/M 级任务清零就回主线批次 301**，不等 5 天上限（09-23 裁定原文，不变）。

## 4. 新增登记项（待迁入 backlog.md，编号取空闲段）

| # | 项 | 强制点 / 证据 | 级别 | 来源 |
|---|-----|---------------|------|------|
| 1 | borrow 检查结果整段丢弃：失败置 `ok=false` 后从来没人读，还有 `let _ = ok;` 把告警按住 | `resolver/typecheck.rs:15-28`（BR-P06） | S | pipeline-rules P06 |
| 2 | W0003 没注册 + 一个错误码三种含义、两种严重级别（fallback 告警 / 非致命 typecheck（error 级打印）/ identity 告警） | `error_codes.rs:2162-2164`、`main.rs:789`、`typecheck.rs:57,528`（BR-A5） | S | pipeline-rules P05/A5 |
| 3 | CLI 和 lib.rs 是两套失败语义（`compile_and_run_zeta` 自称 primary API 但 main.rs 从来没调用过它；一边是 W 告警一边是硬错误） | `lib.rs:89-122` vs `main.rs`（BR-A3） | S | pipeline-rules A3 |
| 4 | E4001 找不到 main：打完 error 诊断还是 `Ok(())`（退出码 0），三处同病 | `main.rs:1026-1041/:1238-1244/:1324-1328`（BR-P07） | S | pipeline-rules P07 |
| 5 | CLI 合同五小项：env 清单漏了 `ZETA_NO_OPT`；重复 `-o` 预扫描取第一个 vs 循环取最后一个；bootstrap 吞 `--target`；bootstrap 静默残留 ≥80 字节输出；bootstrap 链接没有 Windows 分派 | `main.rs:527/:637-650/:1148-1155/:1216-1227`（BR-A7/A8/A9/A10/P12） | S×5 | pipeline-rules §1.2/附录A |
| 6 | ABI.md §3 补记批次 399：`infer_fn_return_type` 已委托给 `Mir::signature_ret_ty`（`mir.rs:45-58`），R7 收敛了「无声明半」（5 行→1 行）；C2/C3/M5–M7 相应收窄（M5 还在：声明与实现不一致时按位重读） | `codegen.rs:1400-1409`；backlog #33 行已记，ABI.md 本体没记 | S | pipeline-rules A14（精确化） |
| 7 | ✅ **已闭合（批次 468，旁路）**：锁步族由差分库看守（比 verifier 合适——verifier 管单级 IR 结构，管不了跨层语义一致）：负数 floordiv/mod 已在库（floordiv_neg×2 / mod_neg×3）；移位族补齐 numeric_shift（含负数右移，match 121）+ numeric_shift_overflow（1<<64 截断错值，已知 mismatch 记账）+ t506 known-fail。除零族 oracle 自报错 = bad_case 不适配 harness，维持登记 | `ctfe/value.rs:274-335` vs `codegen.rs:2020-2081`（BR-M04）；tests/diff/cases/ | S | ✅ |
| 8 | 轴 A 的表补两行：前端死代码四件套（proc_macro 845 行 / macro_expand_advanced 617 / borrow_enhanced 611 / identity_ownership ~470）；`runtime/memory_old+memory_enhanced` 孤儿文件 | `dc_default.txt` 基线占大头；refactor.md §1 表没点名 | S | 深化评审 C5。**⇒ 469 已补录**（refactor.md §1 表两行） |
| 9 | （排序注记，不是任务）refactor.md §9 ⑨′ 的 G.5d 状态行刷新：②(b) 已由批次 399 交付「无声明半」，指针应指向 backlog #33 | refactor.md ⑨′ vs backlog #68 行 | S | 本文件 §2。**⇒ 469 已补录**（refactor.md ⑨′ 状态段追加） |

## 5. 证据锚点怎么用

- 批次记录里引用规则时直接带 BR 编号（例如「BR-R13 参数冲突状态机，`resolver.rs:1671-1717`」），省得重新定位。
- 深化候选和 refactor.md 各轴的对应关系：C1→G.5e、C2→轴 F、C3→轴 D④、C4→G.4（扩容）、C5→轴 A、C6→G.5e 库面子模块。
- 数字以 2026-09-25 实测为准：gen.rs 共 14,644 行 / `lower_expr` 一个函数 10,371 行 / MirGen 结构体 45 个字段 + 15 个 `with_*` 构造器 / codegen 里 0 个 `Result` + 生产路径上 507 个 `.unwrap()`（出处 R1–R3）。

## 6. 批次记录（自批次 410 起追加于此）

### 批次 656（2026-09-29，主线）：`py_df_empty_like` 直接返回 map（帧套帧修复）

**现象**：t404 (`df[idx]` 行过滤) 在 `import pandas as pd` 下崩溃，报错 `map_keys was called on a value that is not a dict (first word="DataFrame")`。同一测试用 `from pandas import DataFrame` 则通过。

**根因**：`py_df_empty_like` 返回的是帧结构体（指针，首字段是 map），但 pylib 的 `loc`/`iloc` 方法直接把结果传给 `DataFrame(...)` 构造器。构造器期望的是 map，于是把帧结构体指针存进 `self.data`。后续 `self.data.keys()` 对帧结构体调 `map_keys`，而帧结构体的首字是 `"DataFrame"` 字符串标签 ⇒ 报错。

**修法**：
- `runtime/py_additions.c:1582-1610`：`py_df_empty_like` 直接返回 `nm`（map），不再包成帧结构体
- `runtime/py_additions.c:1903`：`py_df_loc` 的空选择分支直接 `return py_df_empty_like(frame)`，不再 `*(int64_t*)ef` 解包
- `pylib/pandas.z:189-198`：`loc` 方法minor重构，拆成两行便于阅读

**门禁读数**（快门禁，跳过 corpus/jit/diff 等）：
- python_style: 422 passed, 3 failed (t231, t233, t404), 4 known-fail, 1 xpass
- t404 在 `import pandas as pd` 下仍失败，`from pandas import DataFrame` 下通过——import 风格依赖行为，单独调查

**改动面**：runtime C + pylib（不影响 `src/middle`/`src/backend`，无需位移 A/B）

**提交**：`7ae93e44`

### 批次 657（2026-09-29，主线）：`module_renames_for` 方法回退剥模块前缀（`import pandas as pd` 的 `DataFrame(...)` 不再落 `zeta_platform_obj` 错布局）

**现象**：t404 (`df[idx]` 行过滤) 在 `import pandas as pd` 下崩溃（rc=139 / `map_keys` raise），同一测试用 `from pandas import DataFrame` 则通过（批次 656 记录末尾登记了这一线索）。

**根因**：`src/middle/resolver/resolver.rs` 的 `module_renames_for` 方法回退路径从 `func_name`（如 `"pandas__DataFrame::loc"`）取出 `head = "pandas__DataFrame"`（已带模块前缀的 mangled 名），却拿它去 `py_module_own_names`（存裸名 `"DataFrame"`）里查——永远查不到 ⇒ 方法体拿不到 symbol rename 表 ⇒ `DataFrame(result)` 落到 `gen.rs:10979-11006` 的大写 catch-all `zeta_platform_obj`（布局 `[name|a|b|c]`，name 串在 offset 0）⇒ `self.data` 读到的是 name 串而非 map ⇒ `map_keys` 段错误 / raise。

**修法**（`src/middle/resolver/resolver.rs:4249-4275`）：在查 `py_module_own_names` 之前，先剥掉 `head` 的模块前缀（遍历 `py_module_own_names` 的键，取 `"pandas__"` 前缀，`strip_prefix` 恢复裸名 `"DataFrame"`），再用裸名查。`import pandas as pd` 与 `from pandas import DataFrame` 两条路径现在产出相同的 LLVM IR（`loc` 方法内 `DataFrame(result)` → `call @pandas__DataFrame`）。

**附带**：
- t494 摘钉：batch 656 的 `py_df_empty_like` 直接返回 map 同时修好了非向量掩码空帧回退路径（`df.loc[7]` 现在返回空帧 `len=0`），`known-fail:` 标记移除
- `zeta_runtime_c.o` 补提交（batch 656 运行期改动后未重编入库）

**门禁读数**（快门禁）：
- official: compile 194/194, compile+link 191/194（存量 3 link-only 失败不变）
- python_style: 424 passed, 2 failed (t231, t233), 4 known-fail, 0 xpass
  - 较 batch 656 收尾：+1 pass（t404 修复）、t494 从 known-fail/xpass 转 pass
- compile-diagnostics: python_style 273 warning lines / 128 files（+34 lines vs 239 基线，新测试文件增加）
- dyn_binding: 4/0, comment_drift: 0

**改动面**：`src/middle/resolver`（下型/出码面）+ 测试标记 + 运行期 `.o`

**提交**：`d7f9a8fd`

### 批次 658（2026-09-29，主线）：`dict(x)` 动态参数走 `map__copy`（t231 转绿）

**现象**：t231 (`dict_set_cast_fromkeys`) 运行期 abort：`PY-A: _dict is NOT implemented in this build`。测试里的 `def copy_dict(x): return dict(x)` 在 `x` 为函数形参（类型 `PyDynamic`）时落到幽灵 `dict_1`。

**根因**：`src/middle/mir/gen.rs:9220-9248` 的 `dict(m)` 浅拷贝路径只在参数静态类型为 `map`/`dict` 时生效（`is_map` 判真 ⇒ `MapNew` + `py_map_update`）。当 `m` 是 `PyDynamic` 时，`is_map` 判假 ⇒ 整段跳过 ⇒ `dict(x)` 变成裸大写调用，最终解析为幽灵 `dict_1` ⇒ 链接期 `_dict` NOT implemented。

**修法**（`src/middle/mir/gen.rs:9248-9261`）：在 `if is_map { ... }` 后加 `else` 臂——动态/未知类型参数走 `map__copy(src)`（`runtime/py_additions.c:3718`，创建新 map + `py_map_update` 复制全部条目），语义与 Python `dict(m)` 浅拷贝一致。

**门禁读数**（快门禁）：
- official: compile 194/194, compile+link 191/194（不变）
- python_style: 425 passed, 1 failed (t233), 4 known-fail, 0 xpass
  - t231 转绿（存量红从 2 颗减到 1 颗）
- dyn_binding: 4/0, comment_drift: 0

**改动面**：`src/middle/mir/gen.rs`（下型/出码面）

**提交**：`d7da9915`

### 批次 659（2026-09-30，主线）：`collect_free_vars` 识别裸调用 callee 为自由变量（t233 转绿）

**现象**：t233 (`listcomp_condition_capture`) 运行期 abort：`PY-A: _condition is NOT implemented in this build`。测试里的 `[m for m in xs if condition(m)]` 中 `condition` 来自 tuple unpacking `for _, condition, enabled in steps`，是一个持有 lambda 的局部变量。

**根因**：`src/middle/mir/gen.rs:16613-16621` 的 `collect_free_vars` 在处理 `AstNode::Call` 时假设 method 名永远是静态符号（`let _ = method`），从不检查 callee 名是否是自由变量。当 `condition(m)` 是裸调用（`receiver = None`）且 `condition` 是闭包外的局部变量时，`condition` 不被收集为自由变量 ⇒ 闭包不通过 `zeta_env_get` 捕获它 ⇒ BATCH-294 的 `zeta_call1` 路径走不到（`name_to_id` 里没有 `condition`）⇒ 落到静态符号 `_condition` ⇒ 链接期 NOT implemented。

**修法**（`src/middle/mir/gen.rs:16617-16623`）：当 `receiver.is_none()` 且 `method` 不在 `bound` 集合时，将 `method` 加入 `free` 集合。这样 `lower_closure` 通过 `zeta_env_get` 捕获 `condition`，在子 MirGen 的 `name_to_id` 里注册为 `MirExpr::Var(slot_id)`，BATCH-294 的 `is_value_slot` 判真 ⇒ 发 `zeta_call1(condition_slot, m_arg)`。

**门禁读数**（快门禁）：
- official: compile 194/194, compile+link 191/194（不变）
- python_style: 426 passed, 0 failed, 4 known-fail, 0 xpass
  - t233 转绿（存量红从 1 颗减到 0 颗）
- dyn_binding: 4/0, comment_drift: 0

**改动面**：`src/middle/mir/gen.rs`（自由变量收集）

**提交**：`07e89027`

（暂空——下一批从 409 §十一 (a)①② 的隔离复现开始，记录格式沿用 roadmap.md 的四段证据纪律。）
