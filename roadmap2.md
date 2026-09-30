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

### 批次 660（2026-09-30，主线）：`.get(k, default)` 的「已知并集」写回 `PyDynamic`（`class_dict_field_get` 转绿）＋十倍位全量门禁批

**层号＝4.3.b（类型基础③／返回位），任务＝harness #256。** 这是批次 400 那条「证据冲突 ⇒ 退成动态」规则（当时落在**参数位**）在**返回位**的成员。同时是本十日位的全量门禁批（上次在册＝批次 650）。

**现象（在册红例）**：`tests/diff/cases/class_dict_field_get.dcase`——`self.data.get(key, "missing")` 三行 print 里走默认值那一行打堆地址（改前二进制 md5 `5d08320cc073679a0e22498a6108ca2c` 实拍 `10 / 4300696640 / 20`，run_rc=0＝**静默错值**），CPython 真值 `10 / missing / 20`。

**根因**（`src/middle/resolver/resolver.rs` 本批实读）：py 类方法的返回类型由 `refine_method_return_types`（`:2991`）＋ `collect_return_kinds`（`:3110`）这台引擎决定。在 `.get(k, default)` 面上，字段值类型 `vt` 查 `map_vals`：`self.data = {}` 没有 `map<k,v>` 注解 ⇒ `vt = None` ⇒ 旧判据没有 `(None, Some(_))` 这一支、落到 `_ => None` ⇒ 这条返回被当**毒票**推成 `PyDynamic` ⇒ 写回闸门「全票 PyDynamic＝弃权」⇒ `funcs` 保留解析层硬写的 **i64** ⇒ 调用点按 `println_i64` 打 `char*`。

**修法**＝把「我推不出来」和「证据表明这是真并集」分成两档：
1. `resolver.rs:3201` 起：`(None, Some(_))` ⇒ `Some(Type::PyDynamic)`——字段值类型静态不可知 ⇒ 返回值就是「命中值 ∪ 默认值」的真并集，这是**证据**不是**缺口**；
2. `collect_return_kinds` 加 `dyn_faces: &mut usize`（`:3118`）只统计这条 `.get` 面（callee 回退那条 filter 只放行 `Str|F64|Bool`，所以 `Some(PyDynamic)` 的 face 唯一对应 `.get`）；
3. 写回闸门（`:3080`）加一支：`dyn_faces == rets.len()` 且全票都是 `PyDynamic` ⇒ 写 `Type::PyDynamic`；毒票仍按批次 628 的口径弃权。
调用点无需新代码——`PyDynamic` 返回在批次 653 已端到端可渲染（`zeta_dyn_to_string` + `println_str`）。代码笔 `f3fa96f2`（1 文件 **+26/−4**）。

**否决的另一条改法**：把 `.get(k, default)` 按默认值字面量直接钉成 `Str`（`classify` 那台引擎上的更宽方案，只对本例第一行 `10`/第三行 `20` 是错的）——那会把该例现在**打对**的两行整数变成垃圾。按在册纪律「修掉巧合会揭出依赖它的绿用例」否决。

**收益读数（本批主产出）**：`class_dict_field_get` mismatch → match ⇒ 差分 **576 → 577**（96.0% → **96.2%**）、`per_cat.container 225 → 226`、mismatch 23 → 22，逐案闭合＝转好恰这一条／转差 0 条。闸门代录笔 `9eef3f27`。

**一、十倍位全量门禁（17 步逐项；存件 `/tmp/b660/gate_full.log` 179 行，独占机器、`GATE_RC=` 末行戳记）**

| 步骤 | 本次（660） | 与 650 在册对照 |
|---|---|---|
| official | compile **194/194**、compile+link **191/194**；link-only 3 条名单逐字未动（`integration_all_features`／`quantum_basic`／`selfhost`） | 相同 |
| 诊断面 official | 5/194 文件、**21 行** | 相同 |
| python_style | **426 passed / 0 failed / 4 known-fail / 0 xpass** | 650＝420/2/6/0 ⇒ 两条存量红（`t231`、`t233`）已由 658/659 收干，known-fail 6→4 |
| 诊断面 python_style | **273 行 / 128 文件** | 650＝271/126 ⇒ **+2 行/+2 文件，未逐项归因**（登记；651–659 快门禁未跑该步） |
| corpus | 40 文件、解析 **40/40 = 100%** | 相同 |
| jit sweep | **ok=176 trap=451 fail=0 timeout=0 segv=0**（total 627，最小 ok=163）⇒ `GREEN` | 650＝178/447/0/0/**625** ⇒ 输入 +2、**ok −2**。**归因＝不是本批**：`ZETAC=` 各指 pre/post 两颗各跑一遍，两侧逐字相同（`ok=176 trap=451 total=627`）⇒ 这一档在 651–659 之间发生且当时跳过了 jit 步；**具体哪两条从 ok 变 trap 未逐案定位**（登记） |
| truth | **43/43** | 相同 |
| 真值面分族 | str 105/105、container **226/227**、numeric 113/134、control 90/91 | container +1＝本批那一格，其余相同 |
| diff | **match=577 judged=600 rate=96.2% bad_case=1**（总用例 601） | 650＝576/600/96.0%/1 |
| 坏用例 | `del_undefined_var`（参考侧 `NameError: name 'x' is not defined`） | 相同 |
| knob/swallow/import/empty_stmt | 23 / 6 / 22 / 68 条断言，FAIL 全 0 | 相同 |
| pysrc/cli_semantics/ignore_rules | 42 / 87 / 19 条，FAIL 全 0 | 相同 |
| mbvar/emit_stable/dyn_binding/comment_drift | 25 脚本违规 0／2 夹具违规 0／4 条不一致 0／复述 0 处 | 相同 |
| clean_checkout | rc=0（3s，rev=`67613a9e`） | 相同（rev 搬家＝本批 HEAD） |

⇒ **整趟 `GATE_RC=0`**（650 那次是 1，红源就是 `t231`/`t233` 两条存量红）。**顺带一条台账过时**：AGENTS.md「门禁节奏」一节里那句「`GATE_RC=1` 的存量红源两侧相同＝`t231_dict_set_cast_fromkeys`、`t233_listcomp_condition_capture`」自本批起不再成立（该文件不在本批所有权动作里，只登记不改）。

**二、主线 301 位移 A/B＝0（正证据）**

- 两颗同在仓内 `target/release/`（坑 52/68）：`zetac.pre660` md5 `5d08320cc073679a0e22498a6108ca2c`（＝659 终态那颗；原存件被重建覆盖后按在册止损配方在隔离 worktree `/tmp/b660/pre_wt`@`67613a9e` 重建并**撞上在册 md5**＝正证据）、`zetac.post660` md5 `b7c061c482a35d8f0f8289acea03a14b`（＝当前 `target/release/zetac`）。
- 口径：cwd `~/source/quant/REasyQuant`、`REPLAYQUANT_LOCAL=1`、驱动相对源路径 `strategies/code/_drv_accept_409.py`、**串行独占**、n＝**13/侧**；脚本 `/tmp/b660/ab660.sh`、表 `/tmp/b660/ab660.tsv`（26 行）。
- 产物：`acc660_pre.bin`／`acc660_post.bin` 各 **686,752 B**、compile_rc **0/0**、md5 互异（`56f8832f…`/`82bcea64…`）⇒ 尺寸相同不是判据（坑：产物尺寸当位移判据在 439 已作废）。
- 读数：两侧 26 发**全部** rc=1、stdout **0** 行、stderr **903** 行、`vstack` 命中 0、`成交` 命中 0、末行 `Unhandled exception: code=<N>`；归一后 stderr（`0x..`→`<A>`、数字→`<N>`）取 md5 ⇒ **26 发全落同一颗哈希 `617c614b7b29d72516b9152d46e40317`**。
- 编译诊断面：`diag_pre.txt`／`diag_post.txt` 归一后 **218 行逐字相同**（warning 404／含 `error` 行 19／`PY-A:` 251 两侧同）。
- ⇒ **零位移**（运行面＋编译面各有正证据）。分档如实：这趟两侧都停在 644 §三 在册的「行情缓存命中 0 只 → `Unhandled exception`」档，与 569/575/581 的三档不同档 ⇒ 结论只覆盖这条路径；**主线 301「0 笔成交」症状本批未变**。

**三、锚点面＝本批零重绑义务**

- 改后：漂移 **68**／新 **0**／消失 **4**（基线 306 条），待归属 98 条/88 种、声明为仓外 14 条、共用同键 38 条，rc=1。
- 对照在隔离 worktree `/tmp/b660/pre_wt`（HEAD 自基线）：漂移 **68**／新 0／消失 **6**。
- 逐项核过（不是只比计数）：两份 `[漂移]` **集合 68 条逐字相同**（`diff` 为空）⇒ **本批净 +22 行没把任何一条原本正确的引用推离**，零重绑义务。
- 消失 6 vs 4 的差＝pre_wt 缺未跟踪生成物 `runtime/aliases.inc.c`（`.gitignore` 屏蔽，在册坑 71）带出的 2 条定位失败＋2 条消失＝**worktree 假 delta**，不是本批 damage。
- 唯一内容级差异＝已漂移的 `resolver.rs:4217` 那条引用在新树下指向了另一行（本批 +22 行把它推远）——它改前就在那 68 条存量里，属 **#167** 的账。

**四、事故与自纠（入册以免重犯）**

第一笔改动打到了**错误的引擎**：PY-A 有两台独立的返回推断引擎，`classify`＋`infer_untyped_returns`（`:1335`）只服务模块级无注解 def——它的入口闸门是 `ret.is_empty() || ret == "()"`，而 py 类方法带着解析层硬写的 i64 默认，**根本进不去**。改完编译通过、夹具读数一个字没变（仍是地址）才发现打错。回退四行后用 `git diff --stat src/` 复验工作树为空，再重做。⇒ 教训：**同族两台引擎要先确认哪台在服务这条拼写**，判据＝改后读数不变即打错点。

**五、登记的三个残口（本批刻意未动、也未量成员数）**

1. `self.<f>.get(key, <int 字面量>)`：默认值是整数时 `dt` 也取不到 ⇒ 落 `(None, None)` ⇒ 仍是毒票弃权 ⇒ **同族另一头还红着**；
2. 模块级 `def f(d): return d.get(k, "x")`：走的是另一台引擎（`infer_untyped_returns`），本批未接；
3. 无注解字段 × 默认值类型推不出（`(None, None)`）整档弃权。

**账务**：代码 `f3fa96f2` → 代录 `9eef3f27` → 本记录批（＋worktree 台账行）。下一次全量门禁＝**批次 670**。存件 `/tmp/b660/`（`gate_full.log`、`ab660.sh`/`ab660.tsv`/`runs/*`、`compile_{pre,post}.log`、`diag_{pre,post}.txt`、`compile_diag.tsv`、`anchors_{head,post}.txt`＋两份漂移集合、`t660.z`、`bless.py`、`pre_build.log`、`pre_wt/`）。

（暂空——下一批从 **#257 的两条分歧归因** 开始，记录格式沿用 roadmap.md 的四段证据纪律。）

### 批次 661（2026-09-30，主线）：`fn` 从 Python 源的保留字表摘掉 —— 收 W1002 整文件截断（2.2／共享关键字表）

**pyramid 层归位＝2.2 前端解析**（`parser.rs::parse_ident` 的共享保留字表），缺陷族＝**W1002 静默截断**（尾丢），harness 任务＝**#258**。顺带在同一改动面收掉一条本批中途自己引入的回归（#258 的 ②）。

**一、根因（不是症状）**

`parse_ident`（`src/frontend/parser/parser.rs:83`）把 `fn` 列为保留字 ⇒ 任何**读取**名为 `fn` 的变量的语句（`fn(x)`、`fn = x`、`for fn in tasks:`）在词法层就失败。顶层是 `many0(parse_top_level_entry)`，第一项失败即停止 ⇒ 该定义**连同其后整个文件**被丢弃，而 `ensure_fully_parsed`（`src/main.rs:478-502`）只发 W1002 警告、程序照旧 rc=0 编译。与 `where`（批次 328）、`impl`、`type`（PY-A）同一族、同一个理由、同一个修法形状：**保留字只在声明位置认**。

**二、最小复现与改前实拍**

- 夹具 `tests/python_style/t544_py_fn_is_identifier.z`（三形：`def call_twice(fn): fn(); fn()`＋模块级 `fn = 4`＋类别方法里 `for fn in tasks`，尾行 `print("tail-reached")` 作截断哨）。
- 改前二进制（`target/release/zetac.pre661`，md5 `b7c061c482a35d8f0f8289acea03a14b`）实拍为红：`tests/python_style/t544_py_fn_is_identifier.z:12: 33 line(s) … were NOT parsed`（存件 `/tmp/b661/err_pre661.txt`）；改后 `zetac` md5 `4c3892a6c0a0bcc6f1f16a45abe11bc6` 下 5 行 expect 逐字命中。
- 语料成员实拍：`strategies/code/_drv_probe661.py:12` 改前 W1002 **54 行**（`/tmp/b661/probe_pre.err`）⇒ 改后该文件**不再打 W1002**（`/tmp/b661/probe_post.err`）。

**三、位移 A/B＝−520 行（正证据，语料全项目口径）**

两颗二进制同在仓内 `target/release/`（坑 52/68）；表＝`/tmp/b661/w1002_ab.txt`、合计行 `/tmp/b661/w1002_ab.log`（脚本 `/tmp/b661/w1002_ab.sh`，cwd `~/source/quant/REasyQuant`）。分母＝该仓 310 个 `.py`（`-not -path` 排除 `.venv` 等，**不是**门禁 corpus 步的 40 文件口径）里的 8 个截断成员：

| 成员 | pre | post |
|---|---|---|
| `tests/test_data_service_facade.py` | 35 | **0** |
| `tests/test_expression_facade.py` | 46 | **0** |
| `tests/test_expression_parser.py` | 407 | **0** |
| `tests/test_feishu_notifier.py` | 210 | **178** |
| `tests/test_jq_shim.py` | 571 | 571 |
| `backend/datasrc/split_factors.py` | 97 | 97 |
| `backend/datasrc/dividend_factors.py` | 190 | 190 |
| `backend/engines/jq_shim.py` | 1092 | 1092 |
| **合计** | **2648** | **2128**（**−520**） |

⇒ 4 个成员改善、**0 个变差**；其余 4 个截断成员的病因不是 `fn`（另登 #259）。

**四、本批中途引入并已收口的回归（#258②）**

摘掉保留字后，垃圾表达式可以**跨过换行**吃掉下一项的声明关键字：`!!!\n\nfn second() {…}` 读成 `not not not fn`，`fn second` 就此埋葬。实测在册夹具 `t259_parse_sync_recover.z`（`// env: ZETA_PARSE_RECOVER=1`）：

- pre＝`W1003` 报在**第 9 行**、打印 `1 2`；
- 只改 `fn` 的中转二进制（`c9925b4df09202177cbeaf379c7ac773`）＝`W1003` 推到**第 11 行**、`Undefined symbols: "_second"` / `Linking failed`；
- 修法＝`ends_on_item_keyword`（`top_level.rs`）识别「消费文本正好止于单独一行定义关键字」的项并**拒绝该入口项**（边界回到作者写的换行处），`DEFINITION_KEYWORDS` 同时提到模块作用域供两处共用。
- 复验＝终态二进制下 t259 行为与 pre **逐字相同**（W1003 在第 9 行、`1 2` 照打）。

**五、快门禁（committed 树 `72ed9fac`，二进制 `4c3892a6…`，`/tmp/b661/gate3.log`）**

| 步 | 读数 | 与 660 在册底对比 |
|---|---|---|
| official | compile **194/194**、compile+link **191/194** | 相同（缺运行时绑定仍 3 个：`integration_all_features`、`quantum_basic`、`selfhost`，`/tmp/zeta_official_link.txt`） |
| 诊断面 official | **5 文件 / 21 行** | 相同 |
| 诊断面 python_style | **273 行 / 128 文件** | 相同 |
| python_style | **427 passed / 0 failed / 4 known-fail / 0 xpass** | 660＝426/0/4/0 ⇒ **＋1＝本批新夹具 t544**，其余格不动 |
| dyn_binding / comment_drift | 4 条断言不一致 0 / 复述 0 处 | 相同 |
| **GATE_RC** | **0** | 相同 |

**corpus 步另有一句（工具项，不入本批损害）**：`tools/corpus_baseline.py:19` 的 `timeout=30` 是**硬编码**（`ZETA_CORPUS_TIMEOUT` 对它无效，实测抬 120 仍按 30 抛 `TimeoutExpired`）。用同一份清单与判定、把预算抬到 180s 的旁路脚本（`/tmp/b661/corpus_run.py`）实测 **42/42 解析通过、W1002 丢行合计 0**；其中 2 个是我自己塞进语料目录的探针（`_drv_probe661.py`、`_drv_probe661b.py`，两支都是 OK／W1002=0），删除后**分母回到 40**，与 660 在册的「40 文件、40/40」一致。越界的是 4 个大文件（`jq_wufu_local` 25.5s、`wufu_bt` 35.0s、`wufu_v1` 35.3s、`wufu_v2` 33.4s），**不是本批推过去的**：对 `wufu_bt`/`wufu_v1` 两颗二进制各 2 发计时＝pre **27.8 / 27.3 / 31.0 / 30.9**、post **29.4 / 30.6 / 31.0 / 30.5**（`/tmp/b661/tt.log`）⇒ **改前那一侧也越 30s**，是同一侧方差卡在预算边界（在册坑 84：同侧方差不入账）。⇒ 登工具项 **#260**。

**六、锚点面＝零重绑义务**：漂移 **68**／新 **0**／消失 **4**（基线 306 条），与 660 的 `/tmp/b660/anchors_post.txt` 逐项核过＝两份 `[漂移]` **集合逐字相同**（`diff` 为空），且 68 条里没有一条落在 `src/frontend/parser/**`（本批唯一 `.rs` 改动面）⇒ 本批 `src/**` 净 +61 行（+69/−8）没推离任何引用。

**七、#257 的第一份读数（本批只量、未归因）**

探针 5 步在改后二进制上第一次**真跑到**（改前它整段被 W1002 丢掉）。cwd `~/source/quant/REasyQuant`、`REPLAYQUANT_LOCAL=1`，存件 `/tmp/b661/257_{zeta,oracle}.{out,err}`：

| 步 | zeta（`4c3892a6`） | CPython oracle |
|---|---|---|
| 1 `read_parquet` | OK，`rows 892 cols 9` | OK，同 |
| 2 赋列 | OK，`after assign rows 892` | OK，同 |
| 3 `to_datetime` | OK，`date coerced` | OK，同 |
| 4 `load_metadata` | OK 但 **`meta size 0`** | OK，**`meta size 2`** |
| 5 `validate_and_repair_stock_ohlcv` | **`RAISE … 1`** | OK，**`repaired rows 892`** |

⇒ 两条分歧：① 缓存元数据读成空（stderr 另有 `load_metadata failed for data/stocks/000300_XSHG.parquet: 1`）；② 清洗函数抛异常、异常载荷是 `1`。同一趟 stderr 还有一句线索：`zt_col_as_text was called on a value that is not a dict (handle=0x104d86e47, first word=7881706469483372868)`，而 `7881706469483372868` 小端 8 字节解码＝ ASCII **`DataFram`**（`python3 -c` 实测）⇒ 那个接收者是**类名字符串**而不是 map。**归因未做**，留在 #257。

**八、登记的独立缺陷（本批刻意未动，成员数已实测）**

1. **`m6` 形仍截断**＝`def t(cb: dyn): for x in [1]: cb(x)` 之后的 `print("z")` 被丢（`/tmp/b661/m6.py` 改后 W1002 **7 行**；同批兄弟形 `m1`–`m5` 全 0）⇒ 不是 `fn` 那条词法闸门，另一条成因（#261）。
2. **保留字族的剩余成员＝10 名**：同名探针在终态二进制上仍各丢 2 行＝`mut let pub mod use trait struct enum dyn box`；已收口为 0 的 10 名＝`fn impl crate ref where self super is do pass`（`/tmp/b661/kw_*.py` × `target/release/zetac` 逐名实测）⇒ 下一批按这张表挑损害量最大的名（#262）。
3. **4 个语料成员仍截断且成因不是 `fn`**（571／190／97／1092 行，`/tmp/b661/w1002_ab.txt`）（#259）。
4. **corpus 工具的 30s 硬编码预算**（#260）。

**账务**：代码 `72ed9fac`（`parser.rs` +12/−1、`top_level.rs` +57/−7、`t544` 新夹具 +39/−0）→ 本记录批。下一次全量门禁＝**批次 670**。存件 `/tmp/b661/`（改前二进制**在仓内** `target/release/zetac.pre661`（坑 52：A/B 两颗须同目录，故不放 /tmp）、`gate.log`/`gate2.log`/`gate3.log`、`w1002_ab.sh`/`w1002_ab.txt`/`w1002_ab.log`、`corpus_run.py`/`corpus_post_final.{log,txt}`、`tt.sh`/`tt.log`、`err_pre661.txt`、`probe_{pre,post}.err`、`t259_*` 两颗对照、`257_{zeta,oracle}.{out,err}`、`kw_*.py`、`m1–m6.py`、`corpus_files.txt`(310)/`corpus_fn_files.txt`(10)/`ab_files.txt`(8)）。语料目录里我那两支探针已删（`_drv_probe661.py` 文本存 `/tmp/b661/probe661_kept.py`，#257 复用）。
