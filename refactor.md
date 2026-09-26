# 重构执行计划（refactor.md）

> 本文档是重构主计划，与 [roadmap.md](roadmap.md)（批次主线）、[advice.md](advice.md)
> （工程债清单）、[handoff.md](handoff.md)（接手指南）并列。
> 论证来源：[docs/architecture_optimization_analysis.md](docs/architecture_optimization_analysis.md)
> （2026-09-20，批次 283 时点）+ 2026-09-21 复核与专业编译器视角评估
> （HEAD e2ae14d2，批次 297 已提交、298/299 在途）。
> 所有数字均为 2026-09-21 实测，论断附 file:line 或验证命令，可逐条抽查。
>
> **专业评估的三个根本缺失**（本计划轴 B/F/G 就是它们的答案）：
> ① 没有统一值模型（值不带类型 ⇒ 运行期取证）→ 轴 B；
> ② 类型检查不是独立 pass（定型逻辑寄生在 parser/MIR/codegen 三处）→ 轴 F；
> ③ 正确性工程缺失（-O3 误编译被绕过、静默错值常态化、无差分测试）→ 轴 G。
>
> **执行总原则**：
> 1. 主线（jq_wufu final_value 对齐）不因重构中断——凡标注"立即"的项都在提交间隙做；
> 2. 每步独立提交、独立回滚；混合提交禁止；
> 3. 三基线（官方 194/194 · python_style 存量 2 红 · 语料 100%）是所有改动的红线；
> 4. 全程 `ZETA_NO_OPT=1`（轴 G.1 根治 -O3 后废止本条），长跑 `timeout` 包裹。

---

## 0. 现状快照与债务地图

| 维度 | 数据 | 来源 |
|---|---|---|
| Rust 编译器 | 96,886 行（删除 blockchain 后约 91k） | `find src -name '*.rs' \| xargs wc -l` |
| 最大单体 | `src/middle/mir/gen.rs` **13,525 行**（批次 283 时 12,313，一天 +9.8%） | `wc -l` |
| C 运行期 | 8,443 行，其中 `py_additions.c` 3,549（批次 283 时 3,104） | 实测 |
| Zeta 自举源 | 51 文件 / 7,288 行 | `find zeta_src -name '*.z'` |
| 增速 | 批次 283→297+在途（约一天）：gen.rs +1,212 行、py_additions.c +445 行，全为特判净堆积 | git diff stat |

债务按七轴组织，互相依赖关系见 §9 排程：

| 轴 | 内容 | 规模 | 状态 |
|---|---|---|---|
| A 死代码减法 | 双轨 resolver、ml/distributed、孤儿模块、blockchain、杂物 | ~1.6 万行 | blockchain 已删（9-21）；其余未动 |
| **B 类型标签战役** | 统一值模型：值不带类型 ⇒ 运行期探针类 + 编译期侧信道墙 | 见 §2 | 未动（**核心**） |
| C 编译性能 | clone/HashMap/intern | 见 §3 | 未动 |
| D 层次与可维护性 | gen.rs 拆分、前端越层、双轨决断 | 见 §4 | 未动 |
| E 测试架构 | compile-only 盲区、known-fail、契约测试 | 见 §5 | 部分成型 |
| **F 类型检查器独立** | 定型逻辑从前端/MIR/后端三处剥离，收敛为一个双向检查 pass | 见 §6 | 未动 |
| **G 正确性工程** | -O3 根因、sanitizer、差分、消灭降级、ABI 合同、MIR verifier、**解析截断、假值桩** | 见 §7 | 未动 |

---

## 1. 轴 A：死代码减法（立即，0.5~1 天，风险低）

**在提交间隙执行**——每个块独立一个 commit，逐项 grep 复核全仓引用（含 `bin/`、`examples/`、
`tests/`、`.github/`）后删除，删完 `cargo build` + 三基线。

| 块 | 行数 | 证据（2026-09-21 复核） | 状态 |
|---|---|---|---|
| ~~blockchain + 加密依赖~~ | ~~5,603 + 14 个依赖~~ | src/blockchain、tests/blockchain-smart-contracts、lib.rs:52、Cargo.toml features/deps | ✅ **2026-09-21 已删**（cargo build 绿 + python_style 281/2 验证无回归） |
| `new_resolver` 双轨死管道 | ~~**3,115**~~ | new_resolver.rs 2,199 + typecheck_new.rs 690 + type_cache.rs 226；全流水线仅用 `Resolver`，new_resolver 仅 `resolver/mod.rs:5` 一行声明 | ⚠️ **前提被否（批次 309，可编译性反证）**：删前两文件后 `cargo check` 失败 —— `unified_typecheck.rs:87/112/269/289` 四处 `use super::typecheck_new::NewTypeCheck`（而 `unified_typecheck` 被 `typecheck.rs:8` 引入，在活路径上），且 `Resolver::string_to_type`/`string_to_generic_type` 的实现就在 `new_resolver.rs`、被 `src/middle/types/mod.rs` 等调用。⇒ **不是删除任务，是"双轨收敛"重构任务**。唯 `type_cache.rs`(226) 反证通过、已删 |
| `crate::ml` + `crate::distributed` | **6,226** | `grep -rln "crate::ml\|crate::distributed" src` 排除自身目录为空；却无条件编译（lib.rs 模块声明） | 待删 |
| ~~`new_resolver.rs` 的假 API 面~~ | -39 行 + 可见性收紧 | 批次 310：`RUSTFLAGS=--force-warn dead_code` 点名 `Constraint::Bound`（never constructed ⇒ `solve()` 里那个分支不可达）、`substitution()`、`final_type()`、自由函数 `type_check()`（"入口函数"从未接线）；外部引用实测只有 `typecheck_new.rs:48/61/69/84/440` 五处 | ✅ **批次 310 已删/已收**：4 项删除 + 8 项降为私有，pub 项 16→6；`cargo test --lib new_resolver` 7/7，三基线零位移 |
| **判据本身**：`#![allow(dead_code)]` 关掉了整类告警 | — | `src/lib.rs:7` + `src/main.rs:7` 各一行 ⇒ "cargo check 干净"在本仓**不等于**没有死代码（批次 308/309 只能靠 grep 猜的根因） | ✅ **批次 310 已装判据**：`tools/dc_audit.sh`（`--force-warn` 绕过 allow，不改 lib.rs）采到 **116 条**命中（批次 312 删 bin 后 **101 条**，批次 314 入库 `tools/baselines/dc_default.txt`），`--diff` 模式可做"只许减少"的门禁；⬜ 待决：这两行 allow 是否摘掉（要动并发工作流持有的 `src/lib.rs`） |
| 孤儿模块 holographic/temporal/consciousness/reality | ~~1,309~~ **实为 1,632** | 目录存在但从未在 lib.rs 声明，永不编译 | ✅ **2026-09-21 已删（批次 308）**：口径纠正——原表只数了这四个（440+413+405+51=1,309），同簇的 `src/paradigm/`(274) 与 `src/meta/`(41) 同样未声明，一并删除 ⇒ -1,624 行；删前复核全仓 `^\s*(pub )?mod (holographic\|temporal\|consciousness\|reality\|meta\|paradigm)\s*;` **零命中**、`tests//examples//benches/` 零引用、`Cargo.toml` 无 path 指向；`paradigm_simple.rs`（活代码，lib.rs:70 + tests/unit/test_paradigm.rs:8）带的是**自己内联的同名模块**，不受影响 |
| 根目录/源码树杂物 | — | b3ir.log、tokio_runtime.c/.o、zeta_runtime_c.o、test_match*.z 在根；`tools/__pycache__/` 入库 | 待清（⚠️ `.o` 与 `Cargo.toml`/`src/lib.rs` 同属并发工作流，不可代提交） |
| `src/bin/` 一次性调试残渣 | **1,009** | 11 个文件头部自称"v0.5.0/v0.3.24 兼容性分析 / 单点 print 调试"，非 `#[test]`、无断言、CI 零引用、全仓 grep 零引用；且直接调内部 API（`parse_simd_type`、`skip_ws_and_comments0`、`parse_full_expr`）⇒ 卡住轴 B/C 的签名重构 | ✅ **批次 312 已删**：bin 数 15→4（保留 `zorb`/`zeta-lsp`/`indent_dump`/`pipeline_dump`），`cargo build --release --bins` CPU 时间 7.78s→3.26s（-58%）；同批删 `mir/gen.rs` 5 个纯残留字段 15 行 |
| **⑪ 性能基线是幽灵门禁** | — | `.github/workflows/benchmarks.yml:106/:148` 要 `--bench compiler_bench`/`runtime_bench` 但 **`benches/` 目录不存在**；`:190/:199/:209` 要 `--bin regression_test` 但 **`src/bin/regression_test.rs` 不存在**；`Cargo.toml:94` 有 criterion 依赖、全仓无 `[[bench]]` 段 ⇒ 依赖付了 harness 没接；该 workflow 每天 02:00 UTC 定时无条件失败 | 🟡 **批次 313 走仓内脚本路线实现**：`tools/perf_baseline.py`（不碰 `Cargo.toml`）。两阶段口径 `aot`(端到端，含 clang 链接) / `ir`(`--emit-llvm`，轴 B/C 改 IR 体量最灵敏)； **判定默认只压 `ir` 的 total**（实测 5 次独立调用散布 3.5%），`aot` 跨调用漂 20.7% 只作参考， 且必须先暖机（冷/热差会把基线读成 +38% 假回退）——三个口径全是量出来的； 带 `--calibrate` 复测噪声地板。基线：`ir` 41,655 ms / `aot` 30,186 ms（12 文件， 前 3 大文件占 83%）。⬜ CI 侧 `[[bench]]`+`harness=false` 仍待 `Cargo.toml` 解锁； ⬜ aot 侧要变可判需 A/B 交错（任务 #27） |
| **`--emit-llvm` 打印完成后 SIGSEGV** | — | 批次 313 采基线时发现：`zetac <任意文件> --emit-llvm` **每次都** rc=139（128+11），连 2 行最小程序也崩；lldb 证据 `EXC_BAD_ACCESS (address=0x0)`、`frame #0: 0x0`（跳到空函数指针），崩溃点在 `src/main.rs:816 print_to_stderr()` **之后**的退出路径（stderr 末尾是 AssemblyWriter 最后的 `attributes #N`，IR 完整）。对照：同文件走 `-o`（aot）不崩 ⇒ 只在这条 dump 且跳过 `finalize_and_aot` 的路径上 | ⬜ 待修（任务 #26）：任何拿 `--emit-llvm` 做门禁/CI 的脚本都会被非 0 退出码误导；`--flag` 自 batch 13x 的 `3b79889f` 就在，非本批次引入（未做历史复跑） |
| **第二个幽灵门禁：CI 调的 `tools/run_all.sh` 从未被 git 跟踪** | — | 与 ⑪ 同型但更隐蔽（文件存在、本地天天跑，只是不在 git 里）。`.gitignore:124` 是无斜杠的 `run_*` ⇒ 匹配任意层级，把 `tools/run_all.sh` 一起吞了；而 `ci.yml:106` 的 `baselines` job 就是 `./tools/run_all.sh` ⇒ `checkout@v4` 出来的工作树里没有这个文件，**三套基线（官方 194 / python_style / 语料 39）在 CI 上从未真正跑过**。实测 `git ls-files --error-unmatch tools/run_all.sh` = 未匹配任何已知文件；而 run_all.sh 依赖的 `tools/build_runtime.sh`(:43)、`tests/python_style/run.sh`(:76)、`tools/corpus_baseline.py`(:98) **都已跟踪** ⇒ 缺的只有入口 | ✅ **批次 314 已修**：`.gitignore` 定向插 `!tools/run_all.sh`（不放宽 `run_*`；该文件是 CRLF + 7 个 NUL 的"data"，按字节改写并核对与 HEAD 之间是**纯插入 4 行**）。四条判据实测无连带变化：check-ignore 命中新豁免行、`run_scratch.sh`（根与 tools/ 下）仍被 :124 挡、`zeta_probe.o` 仍被 :145 挡。同批把 dc 基线入库（101 条，并核对"在脏工作树上采的"这条质疑：HEAD 的 blockchain 是 `#[cfg(feature)]` + `default = []`，基线里 blockchain 命中 0 条 ⇒ 干净 HEAD 同样成立）。⬜ `ci.yml:61` 的 JIT 冒烟步骤引用 3 个**不存在**的 `tests/test_hello.z`/`test_values.z`/`test_basic.z`，外面套 `if [ -f "$f" ]`(:62) ⇒ 不失败而是**静默空转并打出 verified 字样**（假绿，比直接红更糟）；本批只记录未修，修法是指向 `tests/unit-tests/` 真实语料而非凭空造文件 |

| 前端死代码四件套 `proc_macro`(845) / `macro_expand_advanced`(617) / `borrow_enhanced`(611) / `identity_ownership`(~470) | **~2,543** | 四文件仅 `frontend/mod.rs` 声明、全仓零其他引用（2026-09-26 复核）；`tools/baselines/dc_default.txt` 内有大头 | 待过堂：按本表判据 (a)/(b) 逐个判——`borrow_enhanced` 属"(b) 从未接线的能力"（其 lifetime 消费链 `types/lifetime.rs` 本身不在主管线），删前先立任务；其余三个倾向 (a) 直接删（roadmap2 登记 8） |
| `runtime/memory_old.rs` + `memory_enhanced.rs` 孤儿文件 | 不在构建图 | 不在 `runtime/mod.rs` 模块树、永不编译；`memory.rs` 的 no_mangle 版本被整块注释（解开即与 `host.rs` 撞符号） | 待删（零风险：不参与编译，删前 grep 引用即可） |

**方式**：独立分支（`feat/infra-cleanup`，沿用 `feat/**` CI 约定）执行，等主线干净提交点合并。

**下一批的现成靶子（批次 311 由编译器点名，非 grep 猜测）** —— 判据 `./tools/dc_audit.sh`
（批次 314 起基线**入库** `tools/baselines/dc_default.txt`，默认特性 **101 条** / 44 文件；
`ZETA_DC_FEATS=--all-features` 另存一份 ⇒ **基线要按 cfg 配置分别存**）：
`src/bin/zorb.rs`(12) · `src/main.rs`(9) · `src/runtime/async_advanced.rs`(8) ·
`src/lsp/protocol.rs`(7) · `src/ml/`(17) · `src/distributed/`(11)。

**删除判据（批次 311 定，别只盯"有没有引用"）**：dead_code 判死分两类，**只删 (a)**：
- **(a) 被同文件近亲取代 / 重复挂载** ⇒ 删（例：`ctfe/evaluator.rs` 的旧 `eval_if_expr`
  被 `:1033 eval_if_expr_with_else` 取代；`resolver.rs` 那两个 identity 字段是
  `passes/identity_verification.rs:13` 那份的重复挂载）——已删 **48 行**。
- **(b) 已实现但从未接线的能力** ⇒ **不删 + 开任务**：它本身就是缺陷证据。现例
  `types/mod.rs:1372 unify_array_size`（活的 `unify:1569` Array 分支语义更弱，见任务 #24）、
  `resolver/typecheck.rs:559` 的 identity 能力推断（并入 #22）、
  `mir/gen.rs:214` 的 `async_*` 五字段（删字段要连带删写入侧，批次 312 单独判）。

**门禁盲区（批次 311 撞出，任务 #23）**：三套基线从不跑 `cargo test`；
`cargo test -p zetac --lib` 实测并行 SIGABRT、单线程 135 passed / 1 failed
（`frontend/indent.rs:944`，HEAD 存量失败，与本批无关）。

---

## 2. 轴 B：类型标签战役（核心，主线收尾后启动）

### B.0 问题定义：i64 槽是万能源，值不带类型

编译器的 ABI 里，一个 64 位槽可以承载：整数、浮点位模式、字符串指针、
list/dict 容器句柄、用户类 struct 句柄、JSON tag 单元。**值本身不携带任何类型信息**，
于是在静态类型传不到的地方，每个语义分派点只能在运行期"猜形状"。

复发的模式固定为两句话：**值是对的，类型丢了**。批次证据（全部见 roadmap 各批四段记录）：

| 批次 | 症状 | 断点位置 |
|---|---|---|
| 279（残留） | 元组变量 `"a" in y` 恒 0 | 元组容器形态，静态动态都不知道 |
| 282 | 日期字符串比较恒假 | MIR 只看 `DynamicArray(Str)` 才改派 strcmp |
| 283 | `df[bool掩码]` 被当列名 | 掩码与 i64 向量同型，布尔语义丢失 |
| 296 | `len(e)` 读前一 GC 块头得 0 | `dict[str, Any]` 元素落在 I64 槽 |
| 296 | 推导式 pair 槽 0 不是 Str | 字典按指针建键，1724 条塌成 1 条 |
| 297 | `if not g.merged_etf_pool:` 永不短路 | 容器句柄恒非零 ⇒ `icmp ne 0` 恒真 |
| 298（在途） | `amount` 读回 4.94e-322 | 整数存进 F64 槽没做 sitofp |
| 298（在途） | `self._cash + total` = 4.6e18 | SemiringFold 拿不到操作数静态类型 |
| 299（在途） | `positions.values()` 读 struct 字段出垃圾 | dict 值类型停在声明态 i64 |

**增速**：见 §0 快照。根因不除，这个斜率不会变。

### B.1 运行期判形启发式（要退役的债务）

`runtime/py_additions.c` 的探针类。每个条目都有一段"第一版 SIGSEGV → 改判形"的踩雷史
（注释即证据），**它们不是设计，是幸存者**：

| 探针 | 判形方法 | 已知盲区 / 踩雷史 | 位置 |
|---|---|---|---|
| `zt_dyn_is_map` | `GC_base(h)==h` + cap 为 ≥16 的 2 的幂 + `GC_size ≥ 16+cap*24`；负 cap 按扩容转发追 `w[1]` | 需要真实 GC 块恰好等大；map/vec 二义时靠先验排除 | py_additions.c:3457 |
| `zt_dyn_vec_hdr` | `base-16` 头 `[cap\|len]` 自成块 + `GC_size ≥ 16+cap*8` | 同上，只验几何不验语义 | :3469 |
| `zeta_dyn_len` | map→计数、vec→头 len、**最后**才试文本 | 第一版 `strlen` 对非句柄 SIGSEGV；字符串字面量在 .rodata 不在 GC 堆 ⇒ GC 块测试答 0 | :3479 |
| `zeta_dyn_contains` | map→内容哈希、vec→列表扫、否则 strstr | 空串语义特判 `"" in text == True` | :3497 |
| `zeta_dyn_truth` | map→计数、vec→头 len、文本→首字节、否则 `!=0` | 指针区间猜测 `0x100000000..0x7fffffffffff`；int 5 靠 GC_base 兜底不进探针 | :3521 |
| `py_json_truth` | 按 ZJ_* kind 分派 | **JSON tag 单元几何探针读不出**（首词是小整数 tag，被误判非空串）——探针族对带 tag 值失明的实锤 | :3543 |
| `zeta_dyn_getitem` | vec 头验界则下标，否则 `map_get` | 假桶链 SIGSEGV（`enumerate` 反式，:3420 注释） | :3427 |
| `zt_slot_truthy` | 8 字节对齐猜测 + 可读首字节 + `"0"/"nan"/"False"` 黑名单 | 对齐≠指针；黑名单是补丁不是语义 | :860 |
| `zt_maybe_map` / `zt_maybe_vec` | 弱化的几何猜 | `zt_maybe_vec_arity1` 退化为 `v > 0x1000` | :878/:884/:960 |
| `zt_c_readable` | `vm_read_overwrite` 探 1 字节 | 只证可读不证是串 | :1204 |
| 方法名 mangle 分派 | `_[dynamic]str__map` 等 3 个 `__asm__` 符号 | 每个动态方法要人肉注册一个魔法名 | :967-989 |

**基线度量**（退役验收的对照数字，2026-09-21 实测）：

```bash
grep -c  "GC_base"  runtime/py_additions.c runtime/tokio_runtime_stub.c   # 11 + 2
grep -c  "0x100000000\|0x7fffffffffff" runtime/*.c                        # 2
grep -c  "__asm__"  runtime/py_additions.c                                # 3
```

### B.2 编译期类型传播断点（要接线的债务）

1. **type_map 按 ExprId 平铺**（`src/middle/mir/gen.rs:103`，`HashMap<u32, Type>`）：
   每个函数一个新 `MirGen`，跨函数边界类型全断；恢复靠 **mangle 名字符串匹配**
   （gen.rs:462、:505，`global_ty_of("mod__name")` 逐前缀试）。
2. **侧信道补丁墙**——MirGen 结构体上每一条旁路都是一个已修断点的化石
   （gen.rs:110-155 一带）：`array_lit_lens`、`pointee_widths`、
   `py_json_items_ids`（批次 288）、`pending_closure_param_types`、
   `last_closure_ret_ty`、`last_dict_pair_ty`（批次 299）。**这个列表还在每批变长**。
3. **未标注形参默认 I64**，靠调用点证据推断、最多 6 轮传播
   （roadmap「真实第三方库首次端到端跑通（2026-09-14）」段）——6 轮打不住的链路就是漏网值。
4. **`Type::PyDynamic`**（`src/middle/types/mod.rs:181`，B3 批次 141 引入）：
   只是编译期占位（"ABI still i64"），运行期没有任何对应物。B4 的 W 方法表
   （`src/middle/pylib.rs:453` `handle_tag`）已能按标签精确分发**库句柄**，
   但用户数据流经 `dict[str, Any]` / 动态形参时标签照样丢。
5. **在途 298/299 的特判**（codegen.rs 槽 sitofp / SemiringFold expr_id、
   top_level.rs `or` 左支定型 / 内建转换定型、gen.rs dict 插入细化 / 三元 temp 携带 /
   用户类恒真）——每一处都是断点的坐标。**这批改动的判定表就是 B.4 迁移时的需求清单。**

### B.3 三个可泛化的先例（本战役不是从零发明）

1. **ZJ_* tagged cell（运行期 tag 已在役）** — `runtime/tokio_runtime_stub.c:2550-2564`：
   `zj_make(tag, payload)` 分配 16 字节 GC 块，`word0 = kind`（7 种：NULL/INT/F64/STR/ARR/OBJ/BOOL）、
   `word1 = 载荷`。`json.loads` → `json.dumps` → `py_json_truth`/`py_json_len` 全链路按 tag
   分发，是全库**唯一不需要启发式的动态值域**。批次 297 的教训反面印证：探针对它失明，
   只能另开 `py_json_truth` 专用通道——**带 tag 的值和探针式值在同一个 ABI 里互不兼容**。
2. **registry 句柄标签（编译期精确分发已在役）** — `pylib/registry.txt`
   `handle=<Tag>` / `ret_handle=<Tag>` + `src/middle/pylib.rs:453 handle_tag`：
   `t.start()`、`f.result()` "dispatch exactly instead of by guessing"（registry.txt:11 原话）。
   threading/queue/Path/Date 全家因此不踩猜坑。
3. **字典值侧表（受限标签已在役）** — roadmap 2026-09-14 段：每次 `DictInsert` 把值的
   静态类型（0=int 1=f64 2=str 3=bool）记入按 (map, key) 索引的侧表，dumps/print 查表——
   **当时刻意"不动 map 布局"**，代价是侧表只覆盖写入点可见的静态类型，类型动态变化即失效。

而 roadmap.md:642 有一条**明确推迟的欠条**：
> 未做（明确记录）：异构列表/嵌套容器的逐元素类型（**需要真正的容器值标签或 tagged union 元素**）

本战役就是还这笔债：把先例 1 的 16 字节 cell 从 JSON 域推广为动态值的**通用表示**，
把先例 2 的标签分发从库句柄推广到用户数据流。

### B.4 方案对比与选型

| 方案 | 做法 | 覆盖 | 迁移成本 | 主要风险 | 结论 |
|---|---|---|---|---|---|
| A 全量 tagged cell | 一切 PyDynamic 值都按 16 字节 (tag, payload) 传递 | 根治 | 极大（每个 i64 边界都要装箱/拆箱） | GC/ABI 全改，等价重写运行期 | 不选 |
| B GC 块头加 tag 字 | 每个容器块头塞一个 tag word | 根治（容器） | 中 | map 扩容转发（负 cap）、vec `base-16` 头假设遍布 runtime，改块头会让运行时多处出错 | 不选 |
| C 纯编译期 | 只修 type_map 传播 + 掩码语义，不打运行期 tag | 静态可达处 | 小 | `dict[str, Any]`、动态形参、json 值**本质动态**，静态永远覆盖不了（批次 296/297 已证） | 不够 |
| **D 分层混合（选）** | D1 静态层修传播；D2 动态层**只给边界值**打 tag，复用 ZJ 16 字节格式 | 边界全覆盖 | 分批可控 | 边界点找不全 ⇒ 保留探针兜底，渐进不爆炸 | **✓** |

**选 D 的理由（全部来自批次证据）**：断点集中在少数边界（`dict[str, Any]` 写入/读出、
未标注形参实参、json 取值、DataFrame 单元格读取、推导式闭包参数），批次 282-299 无例外；
ZJ 格式可直接推广；探针保留为兜底，每迁一域删一段，三基线全程不红。

### B.5 迁移路线（T0-T5，每步可独立提交、可独立回滚）

**T0 前置**（半小时，与轴 D④ 共用，可先做）—— ✅ **2026-09-21 已完成（roadmap 批次 302）**
- `tools/mir_diff.sh`：语料 39 文件逐个 `--dump-mir` 存基准（设计已在 roadmap 还债计划④）。
  任何 MIR 层重构以"diff 为空"自证纯搬运。
- 实现口径（三处缺一不可，前两处是"字节稳定"的真身）：
  ① `Mir::dump_canonical()`（mir.rs）——四个 HashMap arena（exprs/type_map/
  ctfe_consts/global_consts）按键排序后输出，此前直接 `{:#?}`，同一份输入两次
  编译有 284/299 个函数块不同、161324 行 diff；
  ② 打印点移到 `all_mirs`  emission 排序**之后**并走 stdout（main.rs），告警留 stderr；
  ③ 合成闭包符号改为「所属作用域名 + 序号 + 命名空间哈希」（gen.rs
  `closure_ns`/`closure_seq`），替掉进程全局 `AtomicU32`——序号原本取决于
  HashMap 随机的 lowering 访问顺序；命名形状受 codegen 符号瀑布约束
  （无内部 `__`、末段不得全数字），W2001 告警兜住撞名。
- 顺带修真 bug：`lower_closure` 从不并回 `child.generated_mirs` ⇒ 闭包内再定义
  的函数只有引用没有定义（修复前 `tests/python_style/t302_nested_def_in_closure.z`
  跑到 `___closure` 桩 abort，本地驱动表现为链接期 undefined symbol）。
- 现状：`MIR_SNAPSHOT_RUNS=3` 下 34/39 文件逐字节稳定；余 5 个（wufu 家类 + 驱动）
  各只剩 2 行翻转，同一个根因 ⇒ 登记为 §9 后续项「跨模块同名类」：
  `class BaoStockSource` 在 `backend/datasrc/market_data_sources.py:56` 与
  `backend/datasrc/sources.py:99` 各定义一次，两个模块限定名都是合法值，而
  `self` 槽的类型取自一张**按裸名全程序建键**的映射（`Resolver::py_member_aliases`，
  resolver.rs:67），构建时遍历 HashMap ⇒ 后写者胜、谁后写随机。实测 wufu_v1
  325329 行里恰好这一对互换（两个函数块的 `type_map` slot 1 各持一名）。
  定位线索：翻转出现在 `--dump-mir` 里，而 dump 早于 `refine_param_types`
  （main.rs:660 vs :668）⇒ 随机源在 resolver 侧，不是收尾的类型细化。
  **这不只是 dump 的不确定性，是生成程序自身会拿错 `self` 类型**；
  修法是撞名响亮告警（W2002）+ 键改为模块限定。
- 试金石：**元组变量 `in`**（批次 279 残留）——最小、最干净、同时踩静态与动态两侧。

**T1 静态层：bool 掩码语义**（1 天，收益立即可单独发版）
- MIR 为掩码产物引入 `DynamicArray(Bool)`（或专用掩码 type）：
  掩码粗定型点（gen.rs:2017 一带）、行过滤分派判据（gen.rs:8546
  "A `DynamicArray(I64)` argument is a mask"）、codegen 真值判定表
  `container_cond_i1`（codegen.rs:5418）全部改为读类型。
- 效果：`df[掩码]` / `df[列名]` / `df[int索引]` 三分派从"猜接收者名字"变"读类型"。
- 回归：python_style 补 `df[bool_vec]` 用例（当前全目录零覆盖）。

**T2 静态层：跨函数签名表**（1-2 天，**与轴 F.2 同一改动面，合并执行**）
- resolver 把函数签名记入符号表，替换 gen.rs:462/:505 的 mangle 名匹配；
  未标注形参从"默认 I64"变为"记 PyDynamic"。
- 与 advice.md E4（FuncDef 签名结构化）合批（advice.md:203 已建议）。

**T3-T5 动态层**：见 §6.4（依赖轴 F 产出的 PyDynamic 边界点清单）。

---

## 3. 轴 C：编译性能（B/F 之后，触点重叠）

| 项 | 现状（2026-09-21 实测） | 修向 |
|---|---|---|
| `.clone()` 密度 | middle+backend+frontend 共 **1,028** 处 | 热点改 `&str`/Cow；先测基线再动手 |
| String 键 HashMap | gen.rs **29** 张 + resolver.rs **38** + codegen.rs 5 | `rustc-hash` FxHashMap 替换（低风险、可分批） |
| 符号无 intern | 名字匹配全走 String（B.2.1 的 mangle 匹配即受害者） | 名字→u32 intern；**与 B/T2、F 同改动面，故排后** |
| CTFE 线性 scope 查找 | `src/middle/ctfe/context.rs:82`、`:189` 逐层 `iter().rev()` | name→层级索引 |

**前置度量**（作为各步前后对照）：`time zetac` 语料 39 文件 + `zeta_src/main.z` 自举源。

---

## 4. 轴 D：层次与可维护性

1. **gen.rs 拆分**（= 还债计划④，1-2 天）：前置 `mir_diff.sh`（T0）；
   切口三刀——env fold（`eval_env_read`/`fold_env_condition`）、argparse `PyArgNS` 分派块、
   f-string/格式化分派，拆到 `mir/gen/*.rs` 同 crate `impl MirGen`；
   每搬一片跑 MIR diff + 三基线。验收：`lower_expr` 净减 ≥1,500 行、diff 为空。
2. **前端越层依赖**：5 个 frontend 文件直接 `use crate::middle`（identity_ownership.rs、
   parser/identity_type.rs、borrow.rs、borrow_enhanced.rs 及其测试）——自举时
   Zeta 侧要按阶段切编译器，这些边让"先自举 frontend"不可能。修向：identity 类型
   下沉共享 types 层。backend→frontend 为 0，方向正常。
3. **双轨 resolver 决断**：轴 A 已删即了结；若中途决定保留 new_resolver 为继任者，
   必须先接线再谈，"3 千行挂着不接线"是最差状态。

---

## 5. 轴 E：测试架构（把"全绿"变成有意义的信号）

1. **官方 194 是 compile-only**（只编译不运行）——批次 266/272/273/282/296 的静默错值
   全靠驱动与 python_style 抓到，194 全绿照样漏。修向：主线收尾后给官方用例补运行值
   断言（**不放在主线期间做**——会即时改变门禁数字 194/194，干扰主线门禁）。
2. **known-fail 机制转正**：已知坏形态（元组变量 `in`、`df[bool_vec]` 零覆盖等）用
   run.sh 已支持的 `// known-fail:` 标记进 python_style——不破门禁、修复自动转正。
   编号用 **t4xx 段**（现有用例已用到 t298，主线每批继续占号），并在分支描述声明占段。
   **进度（批次 304）**：✅ 已完成，但**先修了机制**——旧路径只看"编译 rc==0"，
   而 §5.2 点名的坏形态全部编译通过（错在值），登记即报 XPASS「可摘除标记」，
   等于把已知错值伪装成待清理。现按 expect 的**值**判定：逐字相同才 XPASS，
   其余（错值/拒编/abort）记 KNOWN-FAIL；known 用例的 expect-error/expect-abort
   被忽略（拒编与 abort 是缺口，不是契约）。首批 5 条：`t401 %s 打句柄整数`、
   `t402 元组 in 恒 False`、`t403 括号内 // 截断解析`、`t404 df[行下标变量] 只 1 行`、
   `t405 hard 桩响亮 abort`（正向）。顺带产出的新缺口：`//` 在括号内让解析截断
   （赋值 RHS 正常、`.py`/`.z` 同表现），已作为 G.7b 首选靶子钉在 t403。
3. **registry 契约测试**（= 还债计划⑤前半）：`pylib.rs` 加 `#[test]` 序列化 parse_registry
   结果为 JSON fixture，`tools/gen_from_registry.py --dump-json` 输出同构 JSON，CI diff。
4. **告警收敛**（= 还债计划⑤后半）：backend+middle 现存 **63 处**裸 `eprintln!`
   （实测），改走 `diagnostics.rs` W2xxx 段。
5. **保持既有纪律**：python_style 每批一条 expect 快照（276→282 已成型）、三基线门禁、
   roadmap 四段证据。

---

## 6. 轴 F：类型检查器独立成 Pass（新，专业评估缺失②）

### F.0 为什么独立成轴

当前**没有类型检查这个编译阶段**——定型逻辑寄生在三处，互相不知道对方：
① 解析器里（`parse_class` 字段定型，top_level.rs:1078 一带，含 298 在途的 `or` 左支/
内建转换规则——**解析器在做语义分析**）；② MIR lowering 里（gen.rs type_map 全部写入点
+ 六条侧信道）；③ 后端里（codegen.rs `container_cond_i1` 真值判定表:5418、slot bitcast
回读 `slot_read_ids`）。后果：同一表达式在三层可能得到三个不同类型答案，谁先谁错全看路径。
专业对照：类型检查必须是**独立 pass**——输入 AST+符号表，输出全程序统一的类型环境。

### F.1 审计：现有定型点清单（checker 的需求来源，0.5 天）

| 定型点 | 现状 | 去向 |
|---|---|---|
| `parse_class` 字段定型（top_level.rs:1078+） | 解析时按 RHS 语法猜类型（`or` 左支、内建转换、字典字面量……） | 迁入 checker（parser 只产 AST） |
| gen.rs `type_map` 写入点 | lowering 边走边记，函数边界断 | checker 产出统一类型环境，lowering 只读 |
| 调用点证据推断（≤6 轮） | 未标注形参默认 I64，逐轮传播 | checker 按调用图 SCC 做不动点（替代"6 轮"） |
| codegen `container_cond_i1`（codegen.rs:5418） | 按类型查表发 py_not/py_json_truth | 读 checker 类型，删表 |
| codegen `slot_read_ids`/slot sitofp（298 在途） | 追踪"哪个槽存过位模式" | 位模式追踪 = 类型信息，应来自 checker 而非 codegen 自查 |
| MirGen 六条侧信道（B.2.2） | 逐个断点的旁路 | 逐条删除，查 checker 输出 |

### F.2 checker 设计（2-3 天）

- 新模块 `src/middle/typecheck/`：`fn check(program: &Ast, signatures: &mut SymbolTable) -> TypeEnv`。
- **双向检查**（bidirectional）：synth 模式（字面量/调用/内建 → 从下往上）+ check 模式
  （赋值/实参/返回 → 期望类型从上往下）。Python 子集的参数标注喂 check 模式，其余 synth。
- **未标注 = `PyDynamic`（gradual），永远不是 I64**——这一条消灭"默认 I64"整类问题。
- 跨函数：签名表 + 调用图 SCC 不动点（递归函数/互递归收敛），取代 mangle 名匹配与 6 轮传播。
- 前置：**B/T2 签名表**（同一改动面，合并执行，见 §2 B.5）。

### F.3 gradual 边界语义（1 天）

- 规则表：`dyn → static` 必须经过**边界点**（插入 unbox/运行时检查，即轴 B/T3 的装箱点）；
  `static → dyn` 自由（box）。
- 类型不匹配（如 f64 槽接 i64 值）= **编译错误**，不再静默降级。
- F 产出的"PyDynamic 流动边界清单"**就是** B/T3 的边界包装点需求来源——两个轴在此握手。

### F.4 迁移拆除（2-3 天，每步 MIR diff + 三基线）

按依赖序，每步独立提交：
1. 字段定型迁出 parser（top_level.rs 清空定型分支）；
2. gen.rs 六条侧信道逐条替换为查 TypeEnv（每删一条跑门禁）；
3. 调用点证据 6 轮传播删除（签名表不动点接管）；
4. codegen `container_cond_i1`/`slot_read_ids` 改读类型（判定表删除）。

---

## 7. 轴 G：正确性工程（新，专业评估缺失③）

### G.0 为什么

编译器正确性必须**对所有优化级别成立**。现状：`-O3` 误编译被当成环境事实绕过
（handoff §7："IR 正确、汇编缺实参……一律用 ZETA_NO_OPT=1"）。专业视角这是最响的警报：
LLVM 只在 IR 违反其规则时才"误编译"——优化级别只是把潜伏 UB 显影。"IR 正确"这个结论
本身存疑：IR 层面的 UB（未初始化读、别名违规、属性不一致）在 `opt` 眼里 IR 依然"合法"。
同时**静默错值是本项目主要 bug 类别**（批次 266/272/273/282/296/298 全是"编译成功、算出错值"），
违背"宁可报错、不可错译"的职业操守。

### G.1 -O3 误编译 root cause（诊断 1 天，可立即做；修复 1-2 天，需协调）

- **复现**：取一个 -O3 下错、-O0 下对的语料文件，llvm-extract/手工裁剪最小化。
- **假设清单**（按概率排序）：
  1. **未初始化槽读**：全 i64 alloca 模型下，部分控制流路径未 store 就 load——-O0 偶然读 0，
     -O3 寄存器分配后为任意值（还债①独立怀疑过 "load_local 读未初始化槽位"）。
     先跑 MemorySanitizer/ASan 的 -O0 构建抓现行。
  2. **调用点/定义属性不一致**：noundef/nocapture/nonnull 声明与实际不符，或 README 提到的
     "persistent TBAA metadata" 写错（TBAA 错 = 别名分析错 = -O3 才暴露）。
  3. **noalias 违规**：参数标了 noalias 但实际被别名。
- **工具**：`opt -O2 --verify-each` 逐 pass 二分定位；`llvm-dis` 对比 -O0/-O3 IR；
  `lli` 直接执行 IR 隔离 JIT/链接变量；INKWELL 版本与 LLVM 行为差异排查。
- **验收**：run_all.sh 支持 `OPT_LEVEL` 矩阵（-O0/-O1/-O2/-O3 各跑一遍）全绿进 CI；
  默认 O3 下驱动 37 日 rc=0；handoff §7 的 ZETA_NO_OPT 条目删除。
  **多级别门禁是持续机制，不是一次性验收**——修完不进矩阵 = 下次 -O3 变更再炸一次。
- **约束**：诊断阶段只读（不动热文件），与主线并行；修复动 codegen 需与主线协调时点。

> ✅ **诊断档实现 2026-09-21（批次 307）**——只读，新增 `tools/opt_matrix.sh`。结论比
> 原设想更尖锐，先记事实：
>
> 1. **优化开关的真实拓扑只有一个，而且是双联动的**：`ZETA_NO_OPT`（存在即生效）同时
>    跳过 `default<O3>` IR 管线（jit.rs:24-26、管线字符串在 :29）**和**把 TargetMachine
>    降到 `OptimizationLevel::None`（jit.rs:168-172）。⇒ 用它做的对照**无法**把问题二分
>    到"IR 层 pass"还是"后端 ISel"。真正的 `-O0/-O1/-O2/-O3` 矩阵需要先有一个只改管线
>    字符串的旋钮（动热文件，属修复档）。
> 2. **命令行 `-O0/-O3` 是死路，而且是响亮的死路**：`src/compiler_config.rs` 解析它们
>    （:121-124）写进 `config.opt_level`（:22），但全仓**无人读** `opt_level`，
>    `CompilerConfig` 除自身文件外只在 lib.rs:52 出现 ⇒ 整个文件不可达。真实 argv 处理
>    在 main.rs:587 的白名单循环里，`-O0` 不在白名单 ⇒ 落到 `_ => input = Some(...)`
>    （:614）被当**输入文件**，实测 `zetac x.z -O0` 得到
>    `Error: Os { code: 2, kind: NotFound }`（:624 的 `read_to_string(&file)?`，
>    错误里连文件名都不提）。同一循环对**第二个位置参数静默覆盖** `input`
>    ⇒ `zetac a.z b.z` 只编 b.z，无提示。
> 3. **MIR 层优化器同样未接线**：`middle::optimization::optimize(mir, level)`
>    （optimization.rs:490）按 `OptLevel` 分发，**零调用点**（grep 复核：只有
>    compiler_config.rs:10 引它的 `OptLevel` 类型）。roadmap 还债③"删 599 行"的前提
>    成立——它本来就没在跑。
> 4. **矩阵实测（287 个非 known-fail 用例 × 2 级）：2 例随级别翻转，方向与假设相反**：
>    `t06_struct_enum`、`t31_builtins2` 在 **-O3 通过、NO_OPT 失败**；t231/t233 两级都红。
>    ⇒ 今天没有"−O3 把对的编错"的现行，反而是 **-O3 在掩盖 IR 自身不自洽**：
>    - `t06`：模块里只有 1 个 `define`（main），`@Color__Green` 是
>      `declare i64 @Color__Green()`（IR:1169）**从无定义**，却被
>      `store i64 ptrtoint (ptr @Color__Green to i64), ptr %6, align 4`（IR:1095）
>      取地址存成值（枚举变体=函数指针）。-O3 把这条死存储 DCE 掉才链接得过；
>      -O0 保留 ⇒ `Undefined symbols: _Color__Green`，"Linking failed"。
>    - `t31`：-O0 下**立即 SIGBUS**（rc=138，10/10 复现、stdout 零字节、无 stderr），
>      -O3 下输出 5 行全对。链接是通的 ⇒ 崩在生成的代码本身，机制未定（下一步）。
> 5. **假设 2（属性/TBAA 声明不符）静态否证**：`src/` 全仓 `tbaa|noalias|noundef|nonnull`
>    **零命中**，`--emit-llvm` 的实际 IR 里 `tbaa/noalias/noundef/nonnull` 各 **0 次**
>    （t228 样本 6109 行）。⇒ 不存在"我们声明了 LLVM 却据此误优化"的通道；
>    但 README.md:19 宣称的 "persistent TBAA metadata" **实现里不存在** ⇒ 文档-实现不符，
>    该句要么删要么进 G.5 的"愿望/现状"分栏。对齐方面：IR 里 `align` 只有 4 与 8 两档
>    （1919/1179 次），无超额声明（over-aligned 才是危险方向）。
> 6. **轴 B 前提量到了数**：同一个 6109 行样本里 `inttoptr` 110 次、`ptrtoint` 63 次、
>    `alloca i64` 1172 处（`alloca double` 仅 3 处）⇒ "指针与 i64 混用"不是修辞，
>    这也是 G.2 生成码侧插桩做不了的直接量化理由。
>
> **验收尾径修正**：原计划"OPT_LEVEL 矩阵全绿进 CI"里，"全绿"当前定义为
> **翻转数 = 0**（`tools/opt_matrix.sh` 以 flips 作 exit 码），而不是"两级各自三基线绿"
> —— 因为 -O0 侧今天本来就是红的（2 例），把它当基线会误伤主线。

### G.2 sanitizer 常态化（0.5-1 天，立即做，= 还债①扩展）

- 新增 `tools/asan_run.sh`：运行时 C 与生成代码都带 `-fsanitize=address` 构建跑语料 + 驱动；
  进 CI（nightly）。
- **t228 非确定性**：20 连跑 + ASan 定位（= 还债计划①，直接执行不再排队）。

> ✅ **部分实现 2026-09-21（批次 306）**——工具与清单已就位，但**覆盖范围比原设想小**，
> 这条差异必须先记下来，否则"0 命中"会被读成"没有内存错误"：
>
> 1. **插桩只到运行期 C**，没到生成代码（后者要 LLVM 的 ASan pass + IR 里指针/i64 不混用
>    ⇒ 轴 B/F 的地盘）。原设想"运行时 C **与生成代码**都带 ASan"这一半**未做**。
> 2. **GC 盲区（实测，非推断）**：`GC_malloc(64)` 后写 `q[72]` → ASan 静默、rc=0；
>    同样越界写在 `malloc(64)` 上 → rc=134 + `heap-buffer-overflow`。
>    zeta 的 vec/map/struct **全部**住在 Boehm GC 堆里 ⇒ ASan 恰好看不见最热的那一类
>    （批次 291/297/298/301 都是这类）。⇒ 需要 **G.2b：容器自带 canary
>    （`ZT_CONTAINER_GUARDS`，按头里的 cap/len 算出用户区末尾再校验）**，
>    检查点挂在 push/insert/grow。注意没有现成的堆遍历可用：本机
>    `/opt/homebrew/include/gc/gc.h` 无 `GC_first_obj`/`GC_next_obj`（实测 grep 零命中），
>    `-DGC_DEBUG` 宏替换是 10 分钟成本的备选。
> 3. **验收从"零命中"改成"零命中 + 阳性对照在位"**：`--selftest` 每次现场重编上述两个
>    越界并断言 `malloc=detected / GC=silent`，工具坏了或口径变了会立刻露出来。
> 4. **还债①（t228）结论：今天不可复现**——200 连跑输出指纹 1 个、rc 全 0；
>    同一输入 20 次 `--dump-mir` 字节稳定（MIR 层已排除）；ASan 构建 20 轮 0 命中。
>    但第 2 条的盲区使"堆内越界/未初始化读"**未**被排除 ⇒ 降级为"G.2b 实现后再判"，
>    不再占用还债首位。
> 5. **同一次 `MIR_SNAPSHOT_RUNS=20` 顺带量到：语料层不是 20 次稳定**——
>    `total=40 stable=35 unstable=5`（`_zeta_local_drv.py`、`jq_wufu_local.py`、
>    `wufu_bt.py`、`wufu_v1.py`、`wufu_v2.py`）。T0（批次 302）只按默认 2 次抽样验收，
>    所以这更像既有抖动而非新回归；但它限定了 T0 的效力：**这 5 个文件上
>    `mir_diff` 只有"2 次抽样一致"级别的可信度**。形态与 §9 任务项"跨模块同名裸名
>    别名表歧义"一致 ⇒ 作为该项目的输入，不在本批追。
>
> 顺带被新用法翻出的三处静默缺陷（都已修，属本批）：`find_runtime_obj` 里
> `ZETA_RUNTIME_DIR` 会输给 cwd 同名 `.o`（从仓库根跑必然发生 ⇒ 静默链到未插桩运行期，
> 足以把"0 命中"变成假结论），现在冲突时打 `[W2002]` 且 `ZETA_STRICT_RUNTIME_DIR=1`
> 以 override 为准；`mir_diff.sh` 在 `CORPUS_ROOT` 不存在时因 `set -e` 丢掉 `--file` 输入
> 且谎报"no input files"；`run_all.sh` 从不重建 C 目标文件（改了 `runtime/*.c` 不重编就
> 拿旧运行期测基线），现在 `runtime/*.c` 比 `.o` 新会打 `[W2003]`。


### G.3 CPython 差分测试 harness（起步 2 天，持续扩充）

- `tools/diff_test.py`：**语义片段库**起步——真值/字符串/容器/数值/控制流，每条 snippet
  以 CPython 实际输出为期望（不是手写期望），先人工 curated **100 条**。
- 进阶：受限语法的随机程序生成 + CPython oracle，一致性率作为指标进 CI/roadmap。
- 与轴 E 关系：python_style 的 `// expect:` 是它的子集；差分 harness 是"生成器版"，
  两者共用运行器。
- **验收判据从"三基线绿"升级为"三基线绿 + 差分一致率 N%"**——首次可回答
  "还有多少语义是错的"。

### G.4 消灭"降级 + 警告"失败模式（1 天，依赖 F.3）

- 盘点 gen.rs 全部"类型未知 → 落 I64 + warning"分支（grep warning 分支逐一登记）。
- 政策：**未知 = PyDynamic + 边界 tag（轴 B），永远不是静默 I64**。
- 验收：降级点 `grep -c` 归零，或残留点逐条进显式 allowlist 并给 roadmap 理由。

### G.5 ABI 合同（pyramid 4.3，可立即做；总量 2-3 天，按 G.5a-d 四步独立提交）

**背景**：zeta 不是没有 ABI，而是有一套**无人写下的 ABI**——批次 298 槽 sitofp、
`slot_read_ids` 位模式回读、packed-string 25% 崩、capybara struct 返回垃圾
（COMPILER_BUGS #4）与 `::`/`__` 双态（#3）全是同一类合同缺失。G.5 = 写
`docs/ABI.md`（合同本体，六章）+ 收敛实现点（单一所有者）。**补丁文化是果，
缺这份合同是因。**

**完备性审计（2026-09-21 实测）**：结论 = **不完备**。现有材料全部是"分析/事故注释"
形态，没有任何一处是"合同"形态（即"违反此规定 = bug"的成文规则）：
> 本表是**审计当时**的快照；批次 305 已把 §1/§2 从 ❌ 变成 ✅（合同已存在），
> §3 调用约定的缺口判断已在批次 316 关闭（同样是 ❌→✅，见该行），
> §4–§6 的缺口判断已在批次 317 关闭（三行全部 ❌/⚠️→✅，其中"已有材料"列里
> 记的行号有三处过期，已在本表就地更正）；§3 的**唯一残余缺口**（附 B#4）已在
> 批次 318 关闭，且关闭方式是推翻批次 316 的一条实测结论（M2）——本表 §2/§3 两行
> 已随之更新。**六章缺口判断现已全部关闭**，剩下的都是"已定位、待修"（任务 #32/#33）。
> **批次 319 起，#33 从"待修"变成"修了一半"**：R7 的返回侧诊断已完成（(a) 完成、
> (b) 单一真源未动），并新增两项 G.5d 输入（#34 门禁读不到编译诊断、#35 锚点自动核对）。
> **批次 320 把这两项输入关掉**：门禁现在聚合编译期诊断（第一份读数 official
> 13/194 文件、python_style 82/285 文件），锚点漂移有脚本可查（95 个锚点、基线入库）。
> 顺带撞出 **附 B#10 / 任务 #36**：`official 194/194` 里 12 个用例的程序被解析器
> 就地截断（丢 88% 的行）⇒ "194/194"这个数本批起要按"编译成功的文件数"读，
> 不能读成"194 个程序完整编译通过"。

| 章 | 完备度 | 已有材料（可汇总进 docs/ABI.md） | 缺口 |
|---|---|---|---|
| §1 槽位值表示 | ✅ **已成文（批次 305）** 10 行表 + 附 A 探针反查证明穷尽 | ARCHITECTURE-REVIEW §4.3"无标签 i64/f64 值模型"段；packed 串规则散在 py_additions.c:1228、:1603（`zt_packed_cstr_eq`）注释 | ~~表示表本体~~；"packed 何时发生"**仍在附 B OPEN**（写侧由 `vec<str>` 生产者隐式决定，未成文） |
| §2 写/读对称 | ✅ **已成文（批次 305，批次 318 补 R7）** R1–R7，逐条 file:line | 298 修复注释（codegen.rs:56-57、:5725、:6157"struct 字段按 64 位槽存，浮点 bitcast 入槽"） | ~~规则表 + 行号锚定~~（含"`slot_read_ids` = F 轴前代偿"的删除条件）；**批次 318 新增 R7**（返回值也是一次跨槽写入：定义侧签名型 == 调用侧 dest 槽型），R1 的"反向不存在"就地限定为**赋值路径** |
| §3 调用约定 | ✅ **已成文（批次 316，批次 318 更正 C2/C3 + 补 M5–M7）** C1–C12 + 强转全表 + §3.5 七个实测探针 | ~~`zeta_call1` 两行注释~~ → 现在是 §3.3/§3.4 的成文规则；validate.md 的 abs(-2.5)→nan 批评已落到 §3.2 表 #6 | ~~返回类型的最终裁决层未定位（附 B#4）~~**已定位并关闭（批次 318）：结论是"没有裁决层"**——被调方签名只来自 `infer_fn_return_type`，声明 `-> T` 只到调用方 dest 槽，二者无一致性检查（`struct Mir` 无 `return_type` 字段）。**顺带推翻批次 316 的 M2"声明类型赢"**（实为位型重解释，M5 打印 `4612811918334230528`）。残余缺口从"未定位"变成"已定位但未修"⇒ 任务 #33；强转全表的"删除候选"实现 = G.5d |
| §4 名字修饰 | ✅ **已成文（批次 317）** N1–N11 + §4.5"新增一个运行期函数要改几处"表 | ARCHITECTURE-REVIEW §4.3 的记录已逐条复核并**更正三处过期**：瀑布实为 **18 档 / 313 行**（`get_or_declare_function` codegen.rs:2575-2887，非"八级 / :2415-2727"）；66 条 `.set` 现在**生成物** `runtime/aliases.inc.c`（非 stub:228-294）；幽灵符号已闭链接期缺口（registry.txt:219 现指向**故意的响亮桩** tokio_runtime_stub.c:1280） | 合同已写；**瀑布收敛 = G.5e**（N6 兜底 extern 会自己猜签名，是"静默错值链"上游） |
| §5 类型布局 | ✅ **已成文（批次 317）** L1–L9（§1 表 #6/#7/#8 的几何常量升格为明文） | py_additions.c 探针注释（vec 头 base-16、map 块 16+cap*24、ZJ 单元 16B）——每条已给写侧/读侧双锚点 | ~~单点成文~~；残余：短 vec 紧块判据仍有**第二份未收敛**（见 §6 行） |
| §6 跨边界假设 | ✅ **已成文（批次 317）** 6.1–6.6（打包/拆包责任逐入口写死） | registry.txt（525 行 F/W）+ 两个解析器（`src/middle/pylib.rs` / `tools/gen_from_registry.py`）+ 三份 JIT 绑定名单 | ~~打包/拆包未定义~~；~~无校验~~已如实成文为"**只核符号存在、从不核签名**"（§6.3，类型 token 除 `f64`/`void` 外无法律效力）；**本批实测出一个真实缺陷**：`zeta_dyn_getitem` :3432 仍写 `cap >= 8`（批次 301 只改了 :3474 那份）⇒ 短 vec 动态下标**挂死**（`map_get` 的 `&(cap-1)` 环），已立任务 #32 |

#### G.5a 槽位值表示 + 写/读对称规则（0.5-1 天，最热，先做）

docs/ABI.md §1-2，直接消灭批次 298 整类 bug 的复发条件：

**§1 槽位值表示表**——一个 8 字节槽的全部合法内容与编码，对着 `py_additions.c`
探针族反查（每个探针假设的表示都必须在表中）：

| 表示 | 编码 | 证据/事故 |
|---|---|---|
| i64 | 裸二补码 | — |
| f64 | IEEE-754 **位模式**（非裸整数） | 批次 298：裸存读回 4.94e-322 |
| bool | 0/1 | — |
| str | 指针（.rodata 或 GC），**或 packed 内联**——何时打包必须显式成文 | 批次 297：packed 当指针传给 str_trim，25% 崩 |
| vec / map / 用户 struct | GC 句柄 | `zt_dyn_is_map` 判形依赖此约定 |
| PyJson | 16 字节 tag 单元指针 | tokio_runtime_stub.c:2550（ZJ_*） |
| PyDynamic（预留） | boxed tag cell | B/T3 实现时更新本章 |

**§2 写/读对称规则**：整数值入浮点槽必须 `sitofp`（codegen `Assign`，298 修复点）；
浮点位模式出槽必须 bitcast 回读（`slot_read_ids`）——每条规则锚定现行实现行号，
并标注"`slot_read_ids` 是 F 轴实现前的代偿，届时删除"。
**验收**：§1 表穷尽（无探针假设了表外的表示）；§2 每条有 file:line 锚点。

> ✅ **已完成 2026-09-21（批次 305）**——`docs/ABI.md` §1/§2 建成，验收两条均成立：
> ① **附 A** 探针反查表逐条核对 7 个判形点（`map_str_key` :105、`zt_dyn_is_map` :3457、
> `zt_dyn_vec_hdr` :3469、`zt_packed_cstr_eq` :1603、`zeta_dyn_len` :3492、
> `zj_make`/`ZJ_*` stub :2550/:2560、`ZT_PROBE_LOC`），**无探针假设表外表示**；
> ② R1–R6 每条带锚点，19 处 `file:line` 已逐条 sed 复核。（批次 318 起 §2 为 **R1–R7**，
> 新增的 R7 覆盖返回路径；R1–R6 原文与锚点未动。）
> 两处与本草稿图的差异，以文档为准：**(a)** 表示从 8 行扩到 **10 行**（str 的指针形
> 与 packed 形必须分行——两者不可静态区分，是 291/297 的分叉点；vec/map/struct 也
> 各自单列，因判形常量不同）；**(b)** 前提修正——**并非"每槽都是 i64"**，
> 静态可定的本地按自身 LLVM 类型开槽（codegen.rs:1576-1580，`F32`→`float`、
> `F64`→`double`），故"8 字节槽"只覆盖其余本地槽 + 容器里的裸字。
> "packed 何时发生"未按原计划在本章写成文规则，而是如实登记为附 B OPEN
> （产生侧目前由 `vec<str>` 生产者隐式决定，无对称写规则可锚）。

#### G.5b 调用约定 + struct 返回（0.5-1 天）

docs/ABI.md §3，对应 capybara COMPILER_BUGS #4。**六章中唯一接近零章**——
codegen.rs 里没有任何关于 struct 返回/参数传递约定的注释，`zeta_call1` 只有两行：
先行 harvest `coerce_call_args` 的现行强转行为（zext/truncate/sitofp/fptosi 全表），
逐条判定"保留/进收敛白名单/删除"：
- zeta→zeta：参数顺序与传递位置、返回值位置；
- **struct/元组返回语义**：先如实记录现行实现（指针？拷贝？——NOTES 怀疑返回局部
  变量指针）；若实现本身不稳定，标记为 **OPEN 决策**并当场选定一个
  （caller-alloc + sret 或隐式指针）写死，不再逐批漂移；
- zeta→C 运行期（`py_*` shim）：寄存器/栈约定、闭包 env 传递（`zeta_call1`）；
- Python 式默认参数/kwarg 的实参折叠规则（PyArgNS）。
**验收**：用 capybara NOTES 的 struct 返回最小复现对 §3 做一次实测核对。

> ✅ **已完成 2026-09-21（批次 316，commit 1f78d9d7）**——docs/ABI.md §3.1–§3.5
> 建成：C1–C12 十二条规则 + `coerce_call_args` 十行强转全表（逐条给
> 保留/白名单/删除候选判定）+ §3.5 四个实测探针（M1 struct 返回、M2 返回类型
> 裁决、M3 分支内 return、M4 = COMPILER_BUGS #4 复现）。验收成立：M4 打印
> `42 / 99`、rc=0。
> **本草稿图有两处需要更正，以文档为准**：
> ① "NOTES 怀疑返回局部变量指针"这条根因假设**被证伪**——写侧一直是 heap 分配
> （codegen.rs:6116 注释原文 "Allocate struct on HEAP to prevent dangling
> pointers"），#4 今天不复现；现行真实风险换成两件事：字段数解析失败时的
> `("", 2)` 二字段兜底（:6336-6345/:6380）与 M2 的静默有损返回（`-> i64` +
> `return 2.5` → ~~`2`~~ 零告警；**批次 318 更正为 `4612811918334230528`**——
> "有损"是对的，"损失成截断"是错的，见下一条）。
> ② "若实现不稳定就当场选定 sret/caller-alloc 写死"这一支**不需要**：现行约定
> 明确（聚合体一律 i64 句柄，LLVM 层无 sret/byval——`src/` grep 命中 0），
> 漂移的不是实现而是文档。
> 顺手了结的另一件：`infer_fn_return_type`（:1376-1388，按 MIR 首条 return 推）与
> 补偿点（:4488-4497，按签名反查）在 M2 下表现不一致——~~**声明类型赢**，即推断
> 不是最终裁决者~~。这一层未定位，登记为 docs/ABI.md **附 B#4**（不在本批猜）。
> ⚠️ **本段两处结论已被批次 318 推翻**：① 裁决层已定位，答案是"**没有裁决层**"
> （`struct Mir` 无 `return_type` 字段，grep 0 命中）；② "声明类型赢"**错**——
> M2 的 `2` 来自另一条路径（`If{dest: None}` 时体内 return 对推断不可见，M7），
> 而 `-> i64` + 顶层 `return 2.5` 实际打印 `4612811918334230528`（M5，位型重解释）。
> 附 B#4 随之关闭，残余风险改记任务 #33。详见 roadmap.md 批次 318。
> 强转收敛（#2/#6/#7/#8/#9/#10 改诊断）与探针固化进 `tests/` 都是 G.5d 的动作项；
> 本批**零行为变更**，探针留在 /tmp 未入库（往 tests/python_style 加文件会动
> 285 这条计数）。纯文档改动，未复跑三套基线。

#### G.5c 名字修饰 + 类型布局 + 跨边界假设（0.5-1 天）

> ✅ **已完成 2026-09-21（批次 317，commit d004230e，任务 #31）**：`docs/ABI.md`
> §4（N1–N11 + §4.5）、§5（L1–L9）、§6（6.1–6.6）、附 A 恢复 + 附 B#5/#5′/#6/#7。
> **起草计划里三处前提被实测推翻/更正**（写合同的过程本身就是审计）：
> ① "瀑布八级"不成立——逐档数是 **18 档 / 313 行**，且**最后一档不是报错**而是就地
> 声明 `i64(i64×实参数)` 的 extern（:2883-2886）⇒ G.5e 的靶心从"消灭瀑布"精确化为
> "消灭猜签名"；② "四处手工同步"里**声明那一份其实有两份**——生成物
> `runtime_decls_registry.rs`/`runtime_decls_core.rs`（294+61 条 LLVM 声明）**从未接线**，
> 现役仍是 255 条手写声明，二者已在 `tools/baselines/dc_default.txt:2-3` 里；
> ③ 幽灵符号例子过期（见上表）。**新发现的真实缺陷**：附 B#7 ——
> `zeta_dyn_getitem` :3432 的 `cap >= 8` 是批次 301 收敛时漏掉的**第二份判据**，
> 实测 `[1,2]+[3,4,5]` 的动态下标**挂死**（1,719/1,719 栈样本在 `map_get+256`）。
> 我初稿据 3 例探针写成"今天不可观测为 bug"，**判据错了**：那 3 例全走
> `zeta_dynarray_new`（:2579 把 cap 抬到 8）因而天然满足旧判据；换用 `n ? n : 1`
> 的生产者（concat/sorted/map/filter/zip/most_common）即暴露。已改为实测结论并立
> **任务 #32**。纯文档改动，未复跑三套基线。

docs/ABI.md §4-6：
- **§4 名字修饰**：`mod__name`、`::` vs `__` 双态（capybara #3）、
  `_[dynamic]str__map` 魔法符号——逐条标注"承重墙"还是"事故"；**汇总
  ARCHITECTURE-REVIEW §4.3 的现成记录**（瀑布八级、66 条 `.set` 别名、
  四处手工同步）成文；其工业化（消灭瀑布）单列为 G.5e；
- **§5 类型布局**：struct 字段顺序与对齐、vec 头 `[cap|len]` 在 base-16、
  map 块 `16+cap*24`、ZJ 单元 16 字节——**py_additions.c 全部探针的几何假设
  在此升格为明文合同**（轴 B 退役探针时按图索骥）；
- **§6 跨边界假设**：registry.txt 每个 F/W 条目假设的实参表示；
  `py_additions.c` 每个 `zeta_dyn_*` 入口假设收到什么——打包/拆包责任在调用方
  还是被调方，逐条写死。
**验收**：§5 的每条几何假设都能在 py_additions.c 找到引用行号；
§6 覆盖 registry.txt 全部条目类别。

#### G.5d 收敛 audit + 流程规则（0.5 天，唯一动代码的步骤）

- 对 §1-6 每条决策 grep 全部实现点，**>1 处的收敛为单一所有者**（其余改为调用它
  或引用文档）；每收敛一条跑三基线；
- 写入流程：**今后每出一个"值对类型错"bug，先修 docs/ABI.md、再修单一实现点**；
  roadmap 批次记录模板增加一行"ABI 影响：无/§x"。
- **输入已由 G.5b/c 备好（批次 316/317），逐条对应**：
  ① §3.2 强转全表里标"删除候选"的 6 档（#2/#6/#7/#8/#9/#10）补诊断；
  ② ~~附 B#4"返回类型最终裁决层未定位"——先定位再给返回侧补一条与 §3.2 对称的告警~~
     **定位已完成（批次 318，附 B#4 关闭）：结论是"根本没有裁决层"**，
     故本项从"先定位再补告警"简化为两步修复，已立**任务 #33**：
     (a) ✅ **已完成（批次 319）**：与 §3.2 对称的**返回侧告警**（不改行为、只出声，
         覆盖 R7 的违反）——但**落点与本条原计划不同**：原计划在定义期比"声明 vs 推断"
         ⇒ 要先把声明类型送进定义侧（`Mir` 无 `return_type` 字段）得动 MIR gen；
         实际改在**调用点**（codegen.rs:3327 调用、:6960 实现）比"被调方 LLVM 返回型
         vs dest 槽型"⇒ 零 MIR 结构改动，且告警落在真正会出错的那条调用上。
         覆盖边界（三条，已写进 ABI.md R7）：只对 Zeta 定义的函数（`zeta_fn_names`）、
         只 float↔int（M7 型不在范围）、单特化泛型未登记。实测曝光：`tests/unit-tests`
         0/194、python_style 顶层 0/12、**真实语料 6/38**（两类：抽象桩绑定 vs M5 型真垃圾）。
     (b) 再把签名来源收敛到单一真源。⚠️ **(b) 会改变 M7 类程序的行为**
     （`If{dest: None}` 下嵌套 return 从"不可见"变"可见"）⇒ 必须跑全部门禁再落，
     并与 285 的计数口径对账。靶心已给：C3 更正后确认补偿点读的是
     **当前正在生成的函数自身**签名（:4488-4497 `get_insert_block().get_parent()`）。
     🟡 **状态（2026-09-26 刷新，批次 469 旁路补录）**：(b) 的"无声明半"已由批次 399
     交付（= 类型基础②）：`infer_fn_return_type` 委托 `Mir::signature_ret_ty`
     （`mir.rs:45-58`），与调用侧 dest 槽同源；全语料 R7 尺子 5 行→1 行。
     **剩余两形**：① 声明与实现不一致仍按位重读（覆盖层真声明赢）；② 一元负号
     两源不同源（#115）——状态以 backlog #33 行为唯一登记点，本节不再逐批更新。
  ③ 附 B#7 = **任务 #32（已实测的运行期挂死，优先级最高）**；⚠️ 阻塞：修复点在
     `runtime/py_additions.c:3432`，该文件由并发工作流持有（批次 317 实测发现）。
  ④ 附 B#6 假旋钮 `ZETA_STRICT_STUBS` 二选一（接行为或删注释）；⚠️ 同样落在并发持有文件。
  ⑤ §3.5 的 M1–M7（批次 318 由 4 个探针扩到 7 个）与 `/tmp/abi5`、`/tmp/abi8`
     两组探针**固化进 tests/**（新开 t4xx 段，勿动 285 的计数口径）；
  ⑥ §6.3 的"只核存在"升级为"核存在 + 核签名/arity"（`tools/check_registry_symbols.sh`）。
  ⑦ ✅ **已关闭（批次 320 / 任务 #34 / 附 B#9）**：门禁读不到编译期诊断——
     原状：`run_all.sh` 的 official 段 `>/dev/null 2>&1` 把 stderr 整个丢掉，
     `tests/python_style/run.sh:95` 只写逐文件 `.cc` 且 `run.sh:15` 的
     `trap 'rm -rf "$OUTDIR"' EXIT` 跑完即删（**批次 319 写的"事后翻得到"是错的**）。
     现况：两套 harness 各出 `compile-diagnostics:` 行 + official 明细落
     `$OFFICIAL_DIAG`，计数进 baseline JSON ⇒ CI artifact 可见；口径排除
     `clang: warning:`（194/194 全有的 `-no-pie` 噪声），**不参与退出码**
     （护栏仍是判定比运行期 stdout ⇒ 加诊断不动 194/285 的计数）。
     **第一份读数**：official 13/194 文件 18 行、python_style 82 文件 191 行、
     新诊断 `ABI return` 0 行（这次是有捕获的测量）。
     ⇒ 后续 ①⑤ 的动手顺序现在有据可依：真正在响的是 `ABI coerce` 那一族
     （python_style 14×`zeta_env_set` + 语料若干），其余候选档可以缓。
  ⑧ ✅ **工具已完成（批次 320 / 任务 #35）**：`codegen.rs` 一插行，ABI.md 的锚点全漂移
     （批次 319 手工重映射 + 一次脚本删掉前缀的回退）。新判据
     `tools/check_abi_anchors.py` = 可定位 / 未越界 / **未漂移**（基线
     `tools/baselines/abi_anchors.tsv`，100 个锚点当前全对上（批次 321 由 95 增至 100）；篡改基线的负向对照 rc=1）。
     ⚠️ **未进 CI**：23 处锚点在并发持有的两个 C 文件里，先加作用域开关再挂硬门禁
     = 任务 #37。⇒ "同批改锚点"目前靠人，不靠门禁，别在文档里写成已被门禁拦住。
  ⑨ **批次 320 撞出的新缺陷（附 B#10 / 任务 #36，属 G.1/G.3 而非 ABI）**：
     `official: 194/194` 里 **12 个用例编译成功但程序被解析器就地截断**
     （`[W1002] … DROPPED from the program`）——合计丢 **1,805 行 / 这 12 文件 2,041 行
     = 88% 内容从未被编译**（`minimal_compiler` 800 丢 757、`benchmark_simd_vs_scalar`
     364 丢 357、`test_suite` 129 丢 127）。~~未解析文本集中在 @ 模式 / impl X for Y /
     match 体几族~~ **⇒ 批次 321 更正：这个归类是错的**。W1002 只报顶层条目首行，
     病因在条目内部；逐条最小用例实测后 `impl P {}`、`impl P for Q {}`、或模式 `1|2|3`、
     `unsafe {}`、`if let Some((p,q))`、嵌套 `} else if` **全部能解析**。
     ⇒ ① 194/194 不能读成"194 个程序全部编译通过"；
     ② 以这批文件为"某语法已支持"的证据一律无效。修法二选一，**不许静默删用例**。
     这一条正是 ⑦ 非做不可的证明：诊断看不见时，连"基线在测什么"都会读错。
  ⑨′ **批次 321 已推进 #36 第一段**：修掉 `x @ 1..=10` 绑定模式（`alt()` 里
     `parse_struct_pattern` 对裸路径故意返回 Ok，抢在 `parse_bind_pattern` 前）
     ⇒ 截断文件 12→11、丢行 1,805→1,749；并新增 `tools/parse_bisect.py`
     把 W1002 从"停在哪个条目"升级为"哪一行的哪个构造"，据此建了 **10 族构造的
     分类表**（进 docs/ABI.md 附 B#10，丢行量排序：match 作表达式 757 >
     static mut 局部 357 > r#" 原始字符串 185 > 字符范围模式 80 > 函数体内 use 85 >
     块体闭包实参 58 > 带标注 for 36 > 单段 import 19 > 常量表达式数组 14 >
     selfhost 158 未定位）。顺带暴露并修掉 JIT 缺 `println_str` 等 7 条 print 家族
     ⇒ **jit ok 163→170**（第一次有批次把 ok 基线**往上**推）。
**验收**：audit 表（决策 → 实现点数）进 docs/ABI.md 附录，残留多处者逐条给理由。

#### G.5e 延伸项：符号注册表工业化（= ARCHITECTURE-REVIEW P0#1，1-2 周，单列排后）

- 一份 schema 同时生成 Rust 声明 + C 头 + registry 条目（顺带消灭四处手工同步与
  幽灵符号——~~registry.txt:208 的 `py_asdict_unexpanded` 即纯字符串表无校验的后果~~
  **例子过期（批次 317 复核）**：条目现在 :219 且 C 侧有定义（响亮桩
  tokio_runtime_stub.c:1278-1282），链接期缺口已闭；**同类缺口换了形状**——
  生成物 `runtime_decls_registry.rs`（294 处 `add_function`）+
  `runtime_decls_core.rs`（61 处）**两个入口函数从未被调用**，现役仍是 codegen.rs
  手写的 255 处声明，两条 `never used` 早就躺在 `tools/baselines/dc_default.txt:2-3`
  ⇒ **G.5e 第一个动作 = 接线或删掉生成物，别让"单一生成器"停在半成品状态**）；
  `get_or_declare_function` 的 **18 档 / 313 行**瀑布（正锚 codegen.rs:2575-2887，
  旧记的"310 行 / :2415-2727"已过期）退化为哈希查找，**靶心从"消灭瀑布"精确化为
  "消灭第 18 档"**——兜底 extern 就地猜 `i64(i64×实参数)` 签名（:2883-2886，ABI.md N6）
  才是"静默错值链"的上游；用 `nm` 自动核对 registry 符号在 .o 中真实存在（**已有，
  但它只核存在、从不核签名** = ABI.md §6.3，G.5e 要补的是签名/arity 判据）。
- 与 G.5 的关系：**G.5a-d 出合同（§4 写清现行规则），G.5e 把 §4 的实现工业化**——
  先有合同再动实现，顺序不可倒。**§4 已于批次 317 成文 ⇒ 本项的前置已满足。**

**G.5 总验收**：六章在位、每条带锚点；codegen 相关注释引用章节号；G.5d 逐条过门禁；
`OPT_LEVEL` 矩阵（G.1）与三基线保持全绿。

### G.6 MIR verifier（pyramid 6.1，1-2 天，新模块）

- 新增 `src/middle/mir/verifier.rs`：结构不变量检查——def-before-use、
  块终结符完整性（capybara NOTES 的"for 循环无 terminator"类 bug 一秒可抓）、
  每个临时恰好一次赋值、type_map 引用的 ExprId 必须存在。
- 挂接：`--dump-mir` 时强制运行 + debug 构建常开；红 = 编译器自身 bug，立即 abort
  （呼应"宁可响亮失败"红线）。
- 与 mir_diff.sh 分工：**diff 管"变没变"（重构安全网），verifier 管"合法不合法"
  （正确性闸门）**——互补不互替。
- 验收：现有语料 39 文件全部通过 verifier；人为注入一个坏 MIR 能红。

### G.7 解析静默截断根治（pyramid 2.2；= ARCHITECTURE-REVIEW P0#3）

**为什么 P0**：many0 静默截断让编译器**静默丢弃用户代码**——官方套件 11 文件受影响、
无 span、无错误恢复（grep 证实 parser.rs 零恢复机制）。这比错译更恶劣：用户以为编过了，
程序实际缺一块。

**优先级设计——"报告"与"行为"分离**：同步恢复会翻出真语言缺口、可能打破 194/194 门禁，
必须协调；报告化不改行为，可立即做。

- **G.7a 截断响亮报告（立即，0.5 天，行为零变更）**：解析器在丢弃输入处打 stderr 告警
  （字节数/偏移/近似行号）——exit code 与解析结果不变，194/194 门禁不动；顺带产出
  受影响文件与被丢内容的盘点清单。
  验收：官方 11 受影响文件全部在清单中现形；三基线数字零变化。
  **进度（2026-09-21 实测盘点）**：告警侧已完成 —— `src/main.rs` 的
  `ensure_fully_parsed` 打 `[W1002] <N> line(s) … 起始文本片段`，行号经
  `indent::remaining_byte_offset` + `original_line_at` 映射回原始源；
  `ZETA_STRICT_PARSE=1` 升级为 `E1002` 致命。
  实测：**官方套件 12 个文件触发 W1002**（清单见 `tools/truncation_inventory.sh`），
  **本地语料 39 文件零触发**（含驱动编译全日志 grep 无 W1002）—— 即
  "静默吃代码"目前只咬官方自测，不咬主线语料。
  盘点清单已脚本化（批次 303）：`truncation_inventory.sh` 一次跑完官方递归 198 `.z`
  + 语料 39，按丢弃行数排序输出 `计数/文件/断点/首段被丢文本`；实测合计
  **1805 行被丢**，前三名 `minimal_compiler.z` 757 · `benchmark_simd_vs_scalar.z`
  357 · `selfhost.z` 158。G.7b 的范围从此逐文件可核，不用估。✅ **报告侧已完成**
- **G.7b 顶层同步恢复 + span（主线收尾后协调，1-2 天，行为变更）**：顶层项解析失败
  跳到下一个顶层关键字继续（rust-analyzer 式弹性解析）；indent.rs 文本层行号映射进
  诊断（与 E.4 共用 diagnostics.rs）。
  验收：受影响文件逐个重新评估——翻出的每个缺口要么修、要么进显式不支持清单；
  错误信息带行号。

### G.8 假值桩响亮化（pyramid 5.4；= ARCHITECTURE-REVIEW P0#4）

**为什么 P0**：`isnan` 恒 False、`isin` 全 1、`vstack` 返回首块、`dropna/fillna/astype`
恒等、`date_range` 空列表——链接通过但语义为假，与"宁可响亮失败"红线直接冲突，
正确性押在"语料恰好不触发"上。

- **G.8a 桩标记 + 调用报告（立即，0.5 天，行为零变更，冷文件）**：registry 条目加
  `stub=` 标记（按 ARCHITECTURE-REVIEW §4.4 清单）；编译期 `--report-stubs` 列出
  本次编译实际调用到的桩。
  验收：报告与 §4.4 假值桩清单一致；不调用桩的程序零告警。
  **进度（批次 303）**：✅ **已完成**。`--list-stubs` 给 18 个标记（registry 1 +
  pylib `# stub:` 17 = §4.4 清单），`--report-stubs` 从 **MIR 调用点**反查真正踩到的
  桩（`pylib::stub_call_match`：marker→`numpy__vstack` / `DataFrame::dropna` /
  单段名三种形状，静态类型未知的接收者按成员名命中并撤销 codegen 消歧后缀
  `[dynamic]str::isin`、`isin_inst_i64`、`isin__2`，标注
  `name-dispatched, may over-report`——宁多报不漏报）。报告只写 stderr、空则一字
  不发。三向实测：`t302`（不调桩）零输出 · `t210_df_tail_dropna` →
  `soft:pandas.DataFrame.dropna x1` · 语料 `l1_fixed_pool_momentum.py` →
  `numpy.log x1` + `soft:pandas.DataFrame.dropna x2`，名称与 §4.4 逐字一致。
  **G.8b 仍待协调**：报告已能列出"哪些程序踩了哪个桩"，逐桩评估的输入齐了。
- **G.8b 桩响亮 abort（协调时点，0.5 天，行为变更）**：标记桩运行期显式 abort
  （沿用 `py_asdict_unexpanded` 已定模式）。**必须逐桩评估后再开**——主线语料可能
  正踩着某个桩，直接响亮化会把 37 日驱动打崩；每个桩要么补真实现、要么经主线确认标死。
  验收：phase 2 后调用未实现语义 = 响亮 abort 而非假值；三基线在桩清零/显式标记后
  恢复全绿。

---

## 8. roadmap 还债执行计划（批次 260+ 定稿五项）的归并

roadmap.md:7588 已定稿的五项，与本计划的对应关系（**不另立两套排程**）：

| 还债项 | 内容 | 归并到 |
|---|---|---|
| ① t228 非确定性专项 | 20 连跑 + dump-mir diff + ASan（0.5~1 天） | **§7 G.2**（升级为 sanitizer 常态化）。批次 306 实测：200 连跑 0 失败、MIR 20 次编译字节稳定、ASan 20 轮零命中 ⇒ 今天复现不出；残留嫌疑恰是 ASan 在 GC 堆上的盲区 ⇒ **降为"G.2b 实现后再判"，退出还债首位** |
| ② 单态化修真 | subst 按位 zip + 删特化缓存（1 天） | 独立；与 F.2 签名处理相邻 |
| ③ MIR 优化器决断 | CSE 键修字面量、两周无收益删 599 行（1 天） | 独立；**不在主线期间做**（动 main.rs，与 ZETA_NO_OPT 调试约定耦合；G.1 根治后重新评估） |
| ④ gen.rs 拆分第一步 | mir_diff.sh 前置 + 分三段搬（1~2 天） | 本计划 T0 + §4.1 |
| ⑤ 契约测试 + 告警收敛 | 各 0.5 天 | 本计划 §5.3 + §5.4 |

**ARCHITECTURE-REVIEW-2026-09 P0 四项归并**（另一来源的 P0，同样不另立排程）：

| 评审 P0 | 内容 | 归并到 |
|---|---|---|
| P0#1 符号四处追认 | 单一符号注册表 | G.5e |
| P0#2 静默类型伪装 | coerce_call_args / libc 撞名 | G.4 + G.5b（强转全表 harvest） |
| P0#3 解析静默截断 | 同步恢复 + span | **G.7** |
| P0#4 假值桩 | stub 标记 + 响亮化 | **G.8** |

---

## 9. 总排程与依赖

```
【当前判断（2026-09-24）】一个一个修语法缺口的办法已经到头：
   最近 5 个批次（375/381/386/390/392）全部只做定位、没改代码，结论都指向同两个根源——
   值身上没有类型标记、函数之间查不到类型。丢代码检查剩 2 个文件 / 176 行，
   全部卡在这两个根源上；另外新发现 3 个文件也在丢代码（共 936 行），
   用的还是已经会修的那套办法，说明老办法到此为止。
   ⇒ 下一步改成：先把类型这块缺的基础能力补上（4-6 天），再回主线。
   ▼
【下一步：补类型基础，三件事】
   ├─ ① for 循环条件里的真假值掩码有了自己的类型，不再靠猜（1 天）
   ├─ ② 每个函数的参数和返回类型统一记进一张表，函数之间能查到（1-2 天）
   └─ ③ 最小类型检查：没写类型的参数一律按"动态值"处理，
       并记录动态值都出现在哪些位置（2-3 天）
   ▼
【回主线前先修两件挡路的事】（都在 gen.rs 同一段，分开改会返工）
   #38 match 语句不进中间代码（已有 7 个确认的错例）；
   #45 格式化输出丢字——println!("A={}", 1) 只打 1，
   恢复主线后打印 final_value 时会直接踩上
   ▼
主线批次 301 恢复（先查 0 笔成交的原因 → 再查选股数量不一致 → 最后对 final_value，约 2-4 天）
   │  这时类型检查已经在了，再遇到类型问题就是"表里加一行"，不用三层打补丁
   ▼ 主线修完（有了算对数的标准程序做对照）
   ├─ 类型检查补全（拆掉解析器/中间层/生成代码里三处临时判断）
   ├─ 给值加类型标记（动态值装箱，探针逐个退役）
   ├─ 拆分 1.4 万行的 gen.rs + 换更快的哈希表
   └─ 三件要等主线的修复：-O3 处理 / 函数体 use / 假桩响亮化
```

| 项 | 收益 | 成本 | 风险 | 时点 |
|---|---|---|---|---|
| 轴A 死代码 | -1.6 万行、依赖图缩小 | 0.5~1 天 | 低（逐项 grep 复核） | **立即** |
| §5.2 known-fail 包 | 堵复发 | 半天 | 无（不破门禁） | ✅ **已完成 2026-09-21（批次 304）** |
| T0 mir_diff.sh | 一切 MIR 重构的安全网 | 半小时 | 无 | ✅ **已完成 2026-09-21（批次 302）** |
| G.1 诊断（只读） | 定位 -O3 UB 真因 | 1 天 | 无（不动热文件） | ✅ **诊断已完成 2026-09-21（批次 307）**：结论是**方向相反**——今天没有"-O3 编错"的现行，反倒是 2 例只有 -O3 才对（`t06` 调用无定义的 `@Color__Green`、`t31` -O0 立即 SIGBUS）；假设 2（TBAA/属性）静态否证。修复档（单变量旋钮 + 这两例）待与主线协调 |
| G.2 sanitizer + t228 | 抓内存问题现行 | 0.5~1 天 | 低 | ✅ **部分实现 2026-09-21（批次 306）**：C 侧插桩 + 命中清单 + 阳性/盲区自对照；**生成码侧未做**（需 LLVM ASan pass，前置=轴 B/F），GC 块内越界另立 **G.2b**（canary） |
| G.5 ABI 合同（G.5a-e） | 同类 ABI bug 绝迹的制动器；审计=§3 接近零、§4 有分析无合同 | a-d 2~3 天；e 符号注册表 1-2 周单列 | 低（a-c 纯文档）/ 中（d、e） | ✅ **a 已完成 2026-09-21（批次 305）** / 其余**立即**（e 排后） |
| G.6 MIR verifier | 每 pass 合法性闸门 | 1~2 天 | 低-中（新模块） | 随批次穿插（T0 后即可） |
| G.7 解析静默截断（G.7a/b） | 静默吃代码绝迹 | a 0.5 天；b 1-2 天 | 低（a 零行为变更）/ 中（b 翻出真缺口动门禁） | ✅ **a 已完成 2026-09-21（批次 303）** / 协调（b） |
| G.8 假值桩响亮化（G.8a/b） | fail-loud 红线落实 | a+b 各 0.5 天 | 低（a 零行为变更）/ 中（b 可能崩主线驱动） | ✅ **a 已完成 2026-09-21（批次 303）** / 协调（b） |
| G.3 差分 harness | 语义一致性可度量 | 2 天起步 | 无（新文件） | **立即起步** |
| B/T1 掩码语义 | 停掉一类猜坑 | 1 天 | 低 | **现在（下一步第一件）** |
| B/T2 + F.2 签名表 | 跨函数类型地基 | 2 天 | 中 | **现在（下一步第二件）** |
| F 核心（检查器最小版：未标注=PyDynamic + 边界清单） | 主线类型洞从"三层补丁"变"签名表加一行" | 2-3 天 | 中（每步门禁） | **现在（下一步第三件）** |
| #38 + #45 联合重构（gen.rs:10676 段） | 主线 301 打印 final_value 的直接前置；selfhost 91 行的堵点 | 2-3 天 | 中（同段重构避免返工） | 弧后、主线 301 前 |
| F.4 全量迁移 | 拆除 parser/MIR/codegen 三处定型特判 | 2-3 天 | 中 | 主线收尾后 |
| B/T3-T5 值模型 | 终止换坑式批次 | 5~7 天 | 中（探针兜底渐进） | 主线收尾后（边界来自 F 核心） |
| G.1 修复 + G.4 | O3 恢复默认；静默错值绝育 | 2-3 天 | 中（动 codegen） | 主线后协调 |
| 还债②③ | 单态化/减法 | 1~2 天 | 低-中 | 随批次穿插 |
| 轴C + §4.1 | 编译提速、可维护 | 3~4 天分批 | 中 | B/F 之后 |

**排序逻辑**：F（静态收敛）在 B/T3-T5（动态装箱）**之前**——因为 F 的 PyDynamic 边界
清单就是装箱点的需求来源，先有清单再装箱，避免边装箱边找边界。轴 G 诊断类全部可并行，
修复类需要协调。
**2026-09-23 修订（用户裁定：先补类型基础再回主线）**：F 的**最小核心**（签名表 + 检查器最小版）
从"主线后"提前到"主线 301 前"——依据是批次 298-300 的实证：主线剩余障碍（0 成交、
universe 分歧）与已修的三个批次同为"值对、类型丢"类，带检查器上主线，剩余洞的修复
从"三层补丁"降为"签名表加一行"，且 gen.rs 不再变大。**F.4 全量迁移与 B/T3-T5 装箱
保持主线后不变**——装箱需要 final_value 对齐当正确性对照，没有对照就不动它。

**报告先行原则**（G.7/G.8 的拆分依据）：凡会改变行为/门禁数字的修复，一律先落
"报告化"半步（零行为变更、冷文件、立即做），让问题显形；行为变更半步等主线收尾
协调执行——194/194 门禁与主线驱动不能被并行工作中途打崩。

**立即档执行序（2026-09-21 重排）**——立即档经多轮追加累计约 10 天量，必须有先后，
否则"都立即"等于没有优先级：

- **第一波（每项 ≤1 天、零行为变更、独立提交；干完后"静默吃代码/假值/ABI 盲区"
  三类问题全部显形）**：
  ① ✅ T0 mir_diff.sh（0.5h，解锁一切 MIR 工作；批次 302）→ ② ✅ G.7a 截断报告
  （批次 303：告警在位 + `tools/truncation_inventory.sh` 清单，12 文件/1805 行）→
  ③ ✅ G.8a 桩标记 + `--report-stubs`（批次 303）→ ④ ✅ §5.2 known-fail 包
  （批次 304：判定改为按值 + 首批 5 条）→ ⑤ ✅ G.5a 槽位值表示表
  （批次 305：`docs/ABI.md` §1 十行表 + §2 R1–R6 + §3 探针反查，19 处锚点逐条复核）
- **第二波（半天~1 天粒度，按依赖排序）**：
  ⑥ ✅ G.2 sanitizer 部分实现（批次 306：ASan 侧工具链就位、MSan 未做、生成码侧未做）
  → ⑦ ✅ G.1 诊断（批次 307：`tools/opt_matrix.sh`，2 例随级别翻转且方向与假设相反）→
  ⑧ 轴 A 进行中（批次 308：孤儿簇 -1,632；批次 309：反证推翻"3,115 待删"、实删 `type_cache.rs` 226；
    批次 310：装上机器判据 `tools/dc_audit.sh`（绕过 `lib.rs:7`/`main.rs:7` 的
    `#![allow(dead_code)]`，基线 116 条）+ `new_resolver.rs` 假 API 面 -39/可见性收紧；
    批次 311：判据说"死"还要判"该不该删"——(a) 被同族实现取代/重复挂载 ⇒ 删，
    (b) 已实现但从未接线 ⇒ 留 + 立任务（是缺陷证据）；按此删 ctfe `eval_if_expr` 与
    resolver 两个未用字段 -48，并把 `unify` 的 Array 臂弱于死代码这件事立成任务 #24；
    批次 312：`src/bin/` 11 个一次性调试残渣 -1,009 行（bin 15→4，`cargo build --release
    --bins` CPU 7.78s→3.26s）+ `mir/gen.rs` 5 个残留字段 -15 ⇒ 判据命中 114→101；
    余 `crate::ml`+`crate::distributed`(6,226) **阻塞**：删除要改正被并发工作流持有的 `src/lib.rs`）
  → ⑨ 🟡 G.5b ✅（批次 316：`docs/ABI.md` §3.1–§3.5，C1–C12 + 强转全表 + M1–M4
    实测；了结 capybara COMPILER_BUGS #4 的"返回局部指针"根因假设——被 M1/M4 证伪，
    残余风险改记为 `("", 2)` 字段兜底 + M2 静默有损返回；附 B 新增 #4"返回类型
    最终裁决层未定位"）；批次 317：§4/§5/§6 成文（N1–N11 + §4.5 同步表 /
    L1–L9 / 6.1–6.6，附 B 增 #5 编码不可逆、#5′ 跨模块裸名回退=任务 #9 同根、
    #6 假旋钮 `ZETA_STRICT_STUBS`、#7）⇒ **G.5c ✅**；#7 是本批唯一实测发现且
    推翻了我自己的初判：写合同过程中量到 `zeta_dyn_getitem` :3432 的 `cap >= 8`
    是批次 301 漏判的第二份 ⇒ 短 vec 动态下标**挂死**（任务 #32）；
    批次 318（G.5d 前置）：附 B#4 定位并关闭 ⇒ **没有裁决层**（被调方签名只来自
    `infer_fn_return_type`，声明 `-> T` 只到调用方 dest 槽，`struct Mir` 无
    `return_type` 字段），据此**推翻批次 316 的 M2"声明类型赢"**（实为位型重解释
    `4612811918334230528`；M2 看到的截断现场另有其人 = M7 的 `If{dest: None}`）+
    更正 C3 的读取对象 + 新增 §2 R7 →
  ⑨′ 🟡 G.5d 动代码（**批次 319 起诊断侧、批次 320 起可测量侧**）：
    批次 319 = 任务 #33 (a) R7 返回侧告警实现（codegen.rs:3327 调用 / :6960 实现，
    零 MIR 结构改动、行为逐字节不变、三基线零位移，实测曝光 0/194 官方 + 语料 6/38）；
    批次 320 = 任务 #34/#35 关闭（门禁聚合编译诊断 + 锚点自动核对），
    **第一份读数**：official 13/194 文件 18 行、python_style 82 文件 191 行、`ABI return` 0 行，
    并据此撞出任务 #36（12 个官方用例被解析器截断 88% 内容）。
    剩余输入见上文 G.5d 条目 ①②(b)③④⑤⑥⑧的 CI 半边 —— 强转 6 档补诊断（顺序已按读数定）、
    #33(b) 单一真源（⚠️ 会改 M7 类行为）、#32 挂死 + 假旋钮**阻塞**
    （`runtime/py_additions.c` 并发持有）、探针固化 t4xx、registry 核签名、#37 锚点接 CI →
  ⑩ G.3 差分 harness 起步 →
  ⑪ 🟡 性能基线（批次 313：`tools/perf_baseline.py` 实现并采下第一份基线
    `ir` 41,655 ms / `aot` 30,186 ms；三处口径全部实测确定——必须暖机、判定只压
    `ir` total(散布 3.5%)、`aot` 漂 20.7% 降为参考；撞出 `--emit-llvm` 退出路径
    SIGSEGV = 任务 #26，aot 侧要可判需 A/B 交错 = 任务 #27）
  → ⑫ ✅ 判据持久化（批次 314，任务 #15）：⑪ 的幽灵门禁有**第二例**——`ci.yml:106`
    的 `baselines` job 跑 `./tools/run_all.sh`，而 `.gitignore:124` 的 `run_*`（无斜杠 ⇒
    匹配任意层级）把它一起吞了 ⇒ **三套基线在 CI 上从未真正跑过**。已定向插
    `!tools/run_all.sh` 豁免（不放宽 `run_*`；`.gitignore` 是 CRLF+7 NUL 的"data"，
    按字节改并核对与 HEAD 纯插入 4 行；4 条 ignore 判据实测无连带变化）。
    同批把 ⑧ 的 dc 基线入库 `tools/baselines/dc_default.txt`（101 条；HEAD 上
    blockchain 是 `#[cfg(feature)]` 且 `default = []`，清单里 0 条 ⇒ 脏树采集不影响可移植）；
    perf 基线含 `corpus=~/…` 本机路径，**仍留 /tmp**（文档串已改成这个理由）。
    ⬜ 顺手记录未修：`ci.yml:61` 的"JIT 冒烟"引用 3 个**不存在**的 `tests/test_*.z`，
    外面套 `if [ -f ]` ⇒ 静默空转还打 verified，属**假绿**（比红更糟）
  → ⑬ ✅ JIT 静默崩溃根治（批次 315，任务 #26）：⑪ 撞出的"`--emit-llvm` 退出路径
    SIGSEGV"**根因被纠正**——那不是 emit 的问题：`--emit-llvm -o x.ll` rc=0 正常，
    而 CLI 无 `-o` 时**一律**走 `finalize_and_jit` + 在编译器进程内 `main.call()`，
    `--emit-llvm` 只是顺带打了 IR。真正的洞：仓里**没有 build.rs**（`find` 0 命中）
    ⇒ `runtime/*.c` 从不在 zetac 镜像里（`nm` 反查 `zeta_env_set`/`println_str` 均
    0 命中），而 JIT 只绑 `pylib/jit_mappings.txt`(131 条) + `vec_*` ⇒ Python 式模块级
    绑定调用的 `zeta_env_get/zeta_env_set/zeta_nonlocal_decl` 无地址 ⇒ rc=139 **零诊断**。
    三处实测教训（都推翻过本批的中间版本）：
    (a) **不能问引擎要地址**——探针 `ee.get_function_address()` 自己在
        `MCJIT::finalizeLoadedModules → RuntimeDyldImpl::resolveRelocations` 里踩空
        （lldb：`EXC_BAD_ACCESS at 0xb0` in `pthread_mutex_lock`，frame#6 即探针）；
    (b) **静态判"引用了未绑符号 ⇒ 拒绝编译"会误杀**——A/B 全跑（485 文件 × 两版二进制）
        抓到 3 个**今天能跑通**的文件被判死：`-O3` 后仍留在 IR 里的调用可能是**动态死代码**；
        另有 `dlsym(NULL,…)` 恒 NULL 叠加假阳（macOS 的 `RTLD_DEFAULT` 是宏 `-2`，
        `libc` 不在 apple 目标导出它）；判"进程镜像里有没有这个符号"因此改用
        `dlopen(NULL)+dlsym`——POSIX 里它就是"本进程已加载的全部镜像"，与 MCJIT 自己
        的宿主查找同一口径。`RTLD_DEFAULT` 作为宏没有可移植拼写（`libc` 在 apple 目标
        根本不导出、在 linux 又把它拼成 `NULL`），而**错句柄不报错、只是什么都找不到**
        ⇒ 会静默退化成"全都绑不到"的假阳工厂（Linux CI 上就是把门禁判红的那一刀）；
        换掉后本机实测同数（ok=163 / segv=0）；
    (c) 实现形态因此是**填桩而非拒绝**：`jit.rs::trap_unresolved_symbols` 给"被引用 +
        非 `llvm.*` + 绑不到"的声明就地填自报名桩体（`zeta_jit_missing_symbol(name_ptr)`
        → 打 `error[E4016]` + `exit(1)`），且必须在 `create_jit_execution_engine`（该处
        拷贝 module）之前；编译期先给一条 `warning[E4016]` 列全桩名。
    判据 = 新增 `tools/jit_sweep.sh`（全量 JIT 扫，把"0 SIGSEGV + ok 数不回退"钉成门禁，
    因三套基线全走 `-o`，**JIT 路径原本一行都没覆盖**）：
    前 159 ok / **326 SIGSEGV** → 后 163 ok / 322 精确报错 / **0 SIGSEGV**，
    `comm` 证 ok 集合零回退，且 4 个原 SIGSEGV 反转为跑通（其崩点是**装载期**重定位，
    填桩后符号全可解析）。同批把"绑了什么"与"能否绑"收敛到同一张表：`JIT_MAPPINGS`
    提为 `pub const`（生成器同步；条目指针保持 `*const ()`，const eval 不许 ptr→int），
    `vec_*` 三个前缀表化 `JIT_VEC_BINDINGS`，注册与判据同读一表防漂移。
    接线：`run_all.sh` 加第 4 步（`--skip-jit` 可关；缺 coreutils `timeout` 时**喊话**
    跳过而非静默跳过——静默跳过就是 ⑫ 记的幽灵门禁），JIT 判据从此和其余三套同处一个
    门禁。`--skip-jit` 与全量四步各复跑一次：official 194/194、python_style 285/2/4/0、
    corpus 39/39、jit ok=163/segv=0（脚本 rc=1 仍是 py_fail=2 的既有判据，非本批引入）。
    被否掉的中间版本不留痕：`main.rs::jit_unresolved_calls`（静态拒绝版）已整段
    删除、`main.rs` 回到 HEAD，`jit_symbol_resolvable` 随之从 `pub` 收回私有。
- **容量约定**：立即档是主线冲刺期的低强度并行轨，按每周 2-3 个提交间隙消化
  （约 3-4 周清完）；主线提前收尾时，未清完的立即档项让位于主链
  （B/T1 → T2+F.2 → F → B/T3-T5 → G.1 修复）。

**SOTA 调研新增任务（W 系，2026-09-21 登记，设计注解见 pyramid.md 各🔬 前沿节点）**：

| W | 任务 | 触发时点 | 归属 |
|---|---|---|---|
| W1 | MIR `op: String` → typed enum（业界 typed-op 标准，低成本高价值） | 还债③决断**前**先做 | 轴 D/还债③ |
| W3 | 自举二进制 PGO（LLVM PGO + BOLT/Propeller） | 自举里程碑 + 轴 C 后期 | 轴 C |
| W5 | 装箱表示 microbenchmark（ZJ-cell vs NaN-box vs tagged-ptr，aarch64 实测装箱/拆箱/分派） | B/T3 的设计输入 | 轴 B |
| W6 | 热调用点单态 inline cache（PEP 659 验证的路线） | B/T5 后性能专项 | 轴 B 后续 |
| W9 | query 式增量编译（salsa 路线） | 远期（自举后） | 轴 C/§7.2 |

**SOTA 不采纳清单**（前沿有、经评估明确不做）：e-graph 中端（Cranelift aegraph——规模
不配，还债③判据不变）；Cranelift 双后端（维护两套后端不合算）；free-threading（单线程
AOT 阶段无关）；copy-and-patch JIT（PEP 744 路线，对 AOT 编译器不适用）；MLIR 式分层
IR（已在显式不做清单，W1 是其最小补救）。

**任务登记迁移（2026-09-22，债务清理期）**：编号任务与 OPEN 项的**唯一登记点 =
[backlog.md](backlog.md)**（有界，登记规则见其文件头）；roadmap.md 回归纯执行日志、
不再新增任务。清理期边界与处理顺序见 backlog.md §0/§3——主线批次 301 冻结至退出条件
触发（S/M 项清零或满 5 个工作日）。
**处理顺序调整（2026-09-23 晚，用户裁定）**：剩余清理时间**全部优先投 B 类解析器截断**
（#36 一族——它是唯一直接影响编译结果正确性的债：还在丢用户代码）；诊断码族按
backlog §3 第 4 条以一行修法收尾即闭；C 类 S 项允许并批一次闭合、跑快速门禁；
**S/M 项清零即回主线批次 301，不等到 5 日上限**。

**显式不做（pyramid 有要求，本计划有意豁免——写明而非遗漏）**：
- **4.1/4.2 指令选择与寄存器分配**：委托 LLVM，自建即重复造轮子；
- **3.1 IR 分层（高层 IR）**：保留 AST→MIR 一跳，D.4 只做物理拆分不改结构；自举优先，
  B/F 实现后重估——这是本计划最大的结构性取舍；
- **1.2 动态语义书面规范**：由 G.3 的 CPython oracle 行为性代偿，不另写规范文档；
- **5.2 GC 策略选型**：B 轴消灭"指针整数混槽"后再评估，当前保守 GC 不动。

## 10. 各轴完成判据

- **轴 A**：§1 表中行数归零，`cargo build` 时间下降有前后数字；
- **轴 B**：B.1 基线度量归零、MirGen 侧信道六条删除、试金石（元组 `in`）与 `df[bool_vec]` 转正；
- **轴 C**：`time zetac` 前后对照 ≥ 可测收益，否则不合并；
- **轴 D**：`lower_expr` 净减 ≥1,500 行且 MIR diff 为空；frontend→middle 边为 0；
- **轴 E**：官方测试含运行值断言、known-fail 集合非空且随修复自动收缩、契约测试进 CI；
- **轴 F**：parser/gen/codegen 三处定型逻辑删除（F.1 表清零）、类型环境全程序唯一、
  `f64 槽接 i64` 类错误变成编译错误而非运行期错值；
- **轴 G**：`OPT_LEVEL` 矩阵（-O0..-O3）全绿进 CI（handoff §7 ZETA_NO_OPT 条目废止）、
  ASan 夜航无红、差分一致率 ≥N%（N 随片段库增长重定）、gen.rs 降级点归零、
  `docs/ABI.md` 在位且被 codegen 注释引用、MIR verifier 常开且现有语料全过、
  截断与假值桩的报告化在位（G.7a 清单、G.8a `--report-stubs`）、
  终态（G.7b/G.8b）后静默截断与假值桩清零。

## 11. 一句话总结

**先用"立即"档止住熵增（轴 A + T0 + known-fail + G 的全部只读诊断），然后按
"F 收敛静态类型（产出边界清单）→ B 实现值模型（按清单装箱、退役探针）→ G.1/G.4
根治正确性（O3 恢复默认、静默错值绝育）"的主链推进，让批次节奏从"每批收一个坑"
回到"每批收一类坑"，并让"还有多少语义是错的"第一次变得可度量（G.3）。**
