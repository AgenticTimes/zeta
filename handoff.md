# Handoff —— 让 REasyQuant 的「本地 wufu 策略」在 zeta 编译器下完整跑通

> 写给接手的人。读完这一份应该能在 30 分钟内跑起来、知道现在卡在哪、下一步该做什么。
> 详细逐批记录在仓库根目录的 `roadmap.md`（已有 121 个批次条目，**每批都有证据**）。

---

## 1. 目标与验收标准

**目标**：`~/source/quant/REasyQuant/strategies/code/jq_wufu_local.py --engine local`
（本地 poly wufu 策略）在 zeta 编译器下**完整编译、运行并产出回测指标**。

**验收基准**（CPython 补注册 `wufu` universe 后同一区间）：

```
final_value 994575.84 / return -0.5424 / trading_days 37
```

**当前状态：未达成**（还没产出 metrics）。编译与数据链路已基本打通，卡在「数据源排序」这一步，
最近一次表现是**挂住（timeout）**而不是崩溃。

---

## 2. 三套基线（必须全绿才允许提交）

| 口径 | 当前 | 说明 |
|---|---|---|
| 官方测试 | **194/194** | `./tools/run_all.sh` |
| python_style | **274 passed / 2 failed** | 存量失败：`t231_dict_set_cast_fromkeys`、`t233_listcomp_condition_capture`（本会话之前就红，非回归） |
| 语料解析 | **39/39 = 100%** | 扫 `REasyQuant/strategies/**/*.py` |

一键跑：

```bash
cd /Users/meetai/source/zeta-src
./tools/run_all.sh                      # 约 7 分钟，输出三行结论
bash tests/python_style/run.sh          # 只跑 python_style（快）
```

**红线**：这三条任一红了就先修再谈别的；编译器**不允许产生非法 IR 或静默错值**，
宁可响亮 abort 也不要静默桩。

---

## 3. 环境与命令速查

### 3.1 编译编译器本身

```bash
cd /Users/meetai/source/zeta-src
cargo build --release            # → target/release/zetac
./tools/build_runtime.sh         # 改过 runtime/*.c 后必须跑（重建 tokio_runtime.o / zeta_runtime_c.o）
```

- `ZETA_NO_OPT=1`：**同时关掉 LLVM -O3**。目前所有调试都用它（-O3 另有误编译，见 §7）。
- 改了 `pylib/*.z`（pandas/numpy shim）不用单独构建，编译用户程序时生效。

### 3.2 驱动（临时 harness，不入库）

文件：`~/source/quant/REasyQuant/strategies/code/_zeta_local_drv.py`（**未跟踪**，随用随改）

标准形态：

```python
import os, sys, json
os.environ.setdefault("REPLAYQUANT_LOCAL", "1")
import backend.strategy.wufu_constants          # 注册 wufu universe（必须）
from strategies.code.jq_wufu_local import run_backtest
r = run_backtest("2024-01-02", "2024-02-29", 1000000.0, engine="local")
print(json.dumps(r["metrics"], ensure_ascii=False))
```

编译 + 运行：

```bash
cd /Users/meetai/source/zeta-src
ZETA_NO_OPT=1 REPLAYQUANT_LOCAL=1 ./target/release/zetac \
  ~/source/quant/REasyQuant/strategies/code/_zeta_local_drv.py -o /tmp/wl/drvN
cd /tmp/wl && timeout 900 env REPLAYQUANT_LOCAL=1 ./drvN
```

- `QUANTGPT_CACHE_ONLY=1`：跳过联网拉取，只用本地缓存（调试时常开）。
- 退出码：`139`=段错误、`138`=总线错误、`134`=abort（含我们自己加的响亮护栏）、
  `124`=**timeout（挂住）**、`192/184/60`=zeta 程序的返回码异常（另有问题，见 §7）。

### 3.3 看崩溃栈

```bash
cd /tmp/wl
printf 'run\nbt 8\nquit\n' > c.txt && timeout 300 lldb -s c.txt ./drvN 2>&1 | grep -E 'frame #[0-9]'
```

条件断点（非常好用，例：抓 `py_df_loc` 收到 0 掩码那次）：

```bash
printf 'breakpoint set -n py_df_loc\nbreakpoint modify -c "$x1 == 0" 1\nrun\nbt 14\nquit\n' > c2.txt
```

### 3.4 运行期探针（env 门控，已内置）

```bash
ZT_PROBE_LOC=1 ./drvN          # vec_cmp / vec_not / df_loc / groupby / empty-like，带调用方符号
ZT_DEBUG_PQ=1  ./drvN          # parquet 列读取
```

`py_df_loc` / `py_df_empty_like` 的护栏还会打 `__builtin_return_address` + `dladdr` 调用方符号，
以及 `GC_base` 风格的句柄校验信息。

---

## 4. 当前卡点（接手点）

### 4.1 现象

最新 harness（`_ranked_fetch_sources`）**挂住**：

```python
from backend.datasrc.sources_selector import _ranked_fetch_sources
r = _ranked_fetch_sources("sh.159985")
print("ranked", len(r))
# → rc=124（timeout），没有输出
```

### 4.2 已知事实

1. 函数体尾部的排序是元凶候选：

   ```python
   return sorted(candidates, key=lambda s: (s != "tushare", -_source_score(s, bs_code)))
   ```

2. 隔离验证**都通过**（所以不是这些语义本身坏了）：
   - `_is_likely_index` 核心逻辑：`num 000300 XSHG 6 isdigit 1 / startswith 1 / a 1`
   - `lambda` 捕获形参 + `sorted(key=…)` + 元组键 + 闭包内调模块函数：`f 2` rc=0
   - `skip: frozenset[str] = frozenset()` 默认值 + `in skip`：`f 2` rc=0
3. 之前（批次 285）这条链是**崩溃**（`_is_likely_index + 136`），现在变成**挂住** ——
   说明最近两批（284 `df[掩码]`、286 私有名修饰）改变了路径。

### 4.3 本批已提交的改动（**未解决目标问题，但确有进展**）

批次 286 给「**闭包内调用模块私有函数**」加了兜底（`src/middle/mir/gen.rs`，23 行）：

- 改前：`_ranked_fetch_sources` 的 `lambda` 里调 `_source_score` ⇒ **链接失败**
  （`Undefined symbols: __source_score`；正确符号应是
  `backend_datasrc_sources_selector___source_score`）。
- 改后：**能链接**（`Compiled to …`），但 `_ranked_fetch_sources("sh.159985")` 仍然超时（rc=124）。
- 三套基线在该改动下仍全绿（见 §2）。

⇒ 说明「私有名修饰」只是这条链上的一个障碍，**根因还在更里面**。若判断该兜底是错的，
直接 `git checkout e04c5d9e -- src/middle/mir/gen.rs` 回退，不会影响别处。

`src/middle/mir/gen.rs` 里给「**闭包内调用模块私有函数**」加了兜底：

- 现象：`_ranked_fetch_sources` 的 `lambda` 里调 `_source_score` 时，链接报
  `Undefined symbols: __source_score`（而正确符号是
  `backend_datasrc_sources_selector___source_score`）。
- 兜底逻辑：裸调用 + 名字以 `_` 开头且 `current_module` 非空 ⇒
  拼 `format!("{}__{}", module.replace('.', "_"), method)` 再降级一次。
- 状态：**能编译通过**，但 `_ranked_fetch_sources` 仍然挂住 ⇒ 需要确认兜底是否生效、
  以及挂住点到底在 `sorted` 还是 `_source_score` 内部的 `_DEFAULT_SOURCE_SCORE[asset].get(...)`。

**下一步建议**（按顺序）：

1. 用 `ZETA_PROBE=1` 看闭包子 MirGen 的 `free` 与 `symbol_renames`，确认私有名替换是否命中；
2. 在 harness 里逐行复刻 `_source_score` 的体（`stats.get` / `_DEFAULT_SOURCE_SCORE[asset].get`），
   定位是字典访问、还是 `sorted` 的 key 调用循环；
3. 若挂住出现在 `sorted`：检查 `py_sorted_key` 的调用约定（闭包返回值是元组 ⇒ 元组比较的
   递归比较函数是否可能自旋）。

---

## 5. 本会话修复清单（232–286，全部已提交并 push）

> 每一条都在 `roadmap.md` 有 pre/post 证据；这里只列**根因级**的。

### 5.1 类型传播（都是"值对、类型丢"⇒ 静默错值）

| 批次 | 缺陷 | 症状 |
|---|---|---|
| 237 | 函数返回注解 `-> pd.DataFrame` 退化成 `map` | 帧列数恒 0 |
| 238 | `-> tuple[pd.DataFrame, int]` 元素类型丢失 | 解构出的帧被当整数 |
| 258/259 | `f64` 结构体字段存位模式、读回未 bitcast | `0.5` → `4.6e18` |
| **278** | **三元表达式的类型被后一处 I64 判断覆盖** | **wufu universe 全是 `4296191491.513180`** |
| 282 | 字符串向量比较走数值路径（`strtod`） | 日期区间恒空 ⇒ 缓存覆盖判断永远失败 |

### 5.2 语义（Python 语义错 ⇒ 静默错值）

| 批次 | 缺陷 | 症状 |
|---|---|---|
| **266** | **`not` 被当成 `~`**（`not parts` 变列表 ⇒ 恒真） | 清洗走错分支、结果变空 |
| **279** | **`x in (元组)` 恒为假**（元组是 `StackArray`，成员测试按动态数组读） | `normalize_to_jq` 失效 ⇒ 119 只全部取不到缓存 |
| 284 | `df[布尔掩码]` 被当列名 | 掩码下标崩溃 |
| 257 | `@dataclass` 字段默认值完全没生效 | `min_price 0 / drop_extreme False` |
| 267 | 方法调用带 kwargs 时**按位置追加** | 参数整体错位 |
| 243/245 | `column > 标量` 类型错、浮点字面量实参被读成 0 | 掩码全真/全假 |
| 249 | `mask_a \| mask_b` 变成**拼接** | 掩码 2N 长 |

### 5.3 运行期实现（缺实现/写坏）

| 批次 | 项 |
|---|---|
| 232 | `py_vec_clip`、`clip` 裸符号回退 |
| 233 | `df["col"] = 标量` 广播；`itertuples`/`iterrows` 行即 map |
| 240/241 | `py_vec_mul`（向量逐元素乘）、`py_vec_pct_change`、`py_vec_abs`、`[dynamic]str__*` 真符号 |
| 244 | **`groupby` 真分组**（`py_df_groupby` + `py_groupby_pairs` + MIR for 降级） |
| 247 | `df.iloc[0:0]` 空切片；`pd.concat` 缺返回注解 |
| 248 | `[dynamic]str__map`（`series.map(lambda)`） |
| 251/274 | **`vec_push` 返回值未回写**（多处，含 `if`/`for` 形式）⇒ 堆破坏/非确定性 |
| 255 | `return (a, b)` 改**堆数组**（原来返回栈 alloca ⇒ 悬垂） |
| 273 | **parquet 时间戳按数量级归一化**（微秒文件被当纳秒 ⇒ `1970-01-20`，892 行塌成 2 行） |
| 274 | 字符串键用 `GC_base` 证明（不再靠指针区间猜） |
| 282 | `py_vec_cmp_str`（字符串向量按 `strcmp` 比较，日期天然按时间序） |

---

## 6. 崩点演进（一眼看清走了多远）

```
get_universe → fetch_stocks → MarketDataFetcher.__init__ → _cache_path 垃圾
→ etf_listing 缓存丢弃 → DataFrame::copy(self==0)
→ 清洗函数内部（+2020 itertuples / +2472 clip / +2944 reset_index / +2884 len(out)）
→ 清洗函数整段跑完并返回
→ _load_cache / fetch_stocks 的 len(cached)（帧指针为 NULL / 缓存不覆盖）
→ 缓存链路打通（universe 119 只代码正确、loaded 118 of 119）
→ 数据源排序 _ranked_fetch_sources 的闭包键（批次 285–286，当前）
```

---

## 7. 踩坑清单（照做能省很多时间）

1. **`W1002` 是静默截断**：harness 行为反常时（尤其"无输出 + rc=0"）**先 grep `W1002`**。
   解析器遇到顶层不能处理的项会**丢弃其后所有内容**——我就差点把"被丢弃的复刻版"当成"通过"。
   ```bash
   ZETA_NO_OPT=1 .../zetac x.py -o /tmp/x 2>&1 | grep -i W1002
   ```
2. **`-O3` 会误编译**：IR 正确、汇编缺实参。**一律用 `ZETA_NO_OPT=1`**；
   收尾时才回 `-O3` 复核。
3. **`vec_push` 扩容会返回新指针**，忽略返回值 ⇒ 旧句柄写空/写坏；全仓已排过两轮，
   新增 C 助手时务必 `x = vec_push(x, v);`。
4. **`map_get` 按 intern 句柄索引**（不是内容）：改列名查询时别"直接用显示字符串"，
   会静默丢列（实测 `group a 0 0`）；必须 `map_str_key(display)` 或已有的
   `zt_safe_str_key`（后者用 `GC_base` 证明可安全解引用）。
5. **`StackArray` 是 alloca**：任何"跨函数返回"都不要让它逃逸。
6. **小字符串会打包进 64 位槽**（如 `sz.15998` = `0x38393935312e7a73`）：
   把它当地址解引用会 SEGV。
7. **不要改 REasyQuant 项目代码**：所有诊断用 harness + 运行期探针 + lldb。
8. **每批必须 commit + push**：`git push agentic bootstrap`（`origin` 是 https，会失败）。
9. **新增测试文件要 `git add -f`**（`*.z` 被 gitignore）；`runtime/*.o` 也要 `-f`。
10. **私有名的修饰只在“定义它的那个模块”内完成**：从别的模块调 `obj._private()` /
    `module._private` 会链接失败（`__private` 未定义）或拿到垃圾。探针里优先用公共 API。
11. **桩的规矩**：默认 `__attribute__((weak))`；与 libc 同名（如 `login`）必须强定义；
    去重**只能压消息不能压 raise**（否则第二次调用吞异常 ⇒ 返回 0 ⇒ 调用方解引用空对象）。

---

## 8. 关键文件地图

| 位置 | 作用 |
|---|---|
| `src/middle/mir/gen.rs` | MIR 降级主战场（三元类型、tuple 返回、`|`/`>` 分派、闭包、`for` 降级、方法调用实参绑定） |
| `src/middle/resolver/{resolver.rs,new_resolver.rs}` | 类型/注解归一化（`shim_class_normalize`）、模块全局类型、符号修饰 |
| `src/middle/types/mod.rs` | `Type::from_string`（`float`→F64 等） |
| `src/frontend/parser/{expr.rs,stmt.rs,top_level.rs}` | `not`/`in` 算子、dataclass 字段默认值、三元 |
| `src/backend/codegen/codegen.rs` | 结构体字段存取（f64 bitcast）、字段索引解析 |
| `runtime/py_additions.c` | **本会话改动最多**：`py_df_*`、`py_vec_*`、`py_not`、`zt_safe_str_key` |
| `runtime/tokio_runtime_stub.c` | 数组/map 原语（`vec_push` 护栏、`map_resolve` 链） |
| `runtime/parquet_min.c` | 自包含 parquet 读取（thrift/snappy/RLE/时间戳单位归一化） |
| `pylib/pandas.z` | pandas shim（DataFrame / loc / iloc / concat / groupby / 空帧规范化） |
| `tests/python_style/` | 回归用例（t2xx） |
| `roadmap.md` | **逐批记录 + 证据**（接手必读，尤其最近 30 批） |

---

## 9. 复现「已验证可用」的一串（自检环境是否正常）

把下面整段写进 harness 后编译运行；**输出必须和注释一致**（这是本会话验证过的基线状态）：

```python
import os, sys, json
os.environ.setdefault("REPLAYQUANT_LOCAL", "1")
import backend.strategy.wufu_constants                      # 必须先注册 universe
from backend.datasrc.market_data import MarketDataFetcher, get_universe
from backend.datasrc.data_cleaning import validate_and_repair_stock_ohlcv, MarketCleanConfig

codes = get_universe("wufu", date="2024-01-02")
print("universe", len(codes), codes[0], codes[1])
flush()

f = MarketDataFetcher()
c = f._load_cache("515170.XSHG")
print("cached", len(c), len(c.columns), c["trade_date"][0], c["trade_date"][891])
flush()

o, rep = validate_and_repair_stock_ohlcv(c, "515170.XSHG", MarketCleanConfig())
print("clean", len(o), len(o.columns), rep.output_rows)
flush()
```

实测输出（2026-xx 最新工作区）：

```
universe 119 sz.159985 sh.512070
cached 892 9 2022-05-05 2025-12-31
clean 892 9 892
```

> 说明：最后一步的 `clean 892 9 892` 是本会话的重要里程碑（曾经是 `clean 2 9 2`）。
> 打印日期那行能同时验证 parquet 时间戳单位（批次 273）与字符串比较（批次 282）。
>
> 退出码有时是 `184/192/60` 之类的怪值（zeta 程序返回值另有问题，不影响输出内容）。

### 9.1 探针写不出链的坑

从**自己的**模块里调另一个模块的**私有**方法/属性（`obj._cache_path(...)`、`MDS._parquet_cache`）
目前会**链接失败或拿到垃圾** —— 因为私有名的修饰（`模块__名字`）只在**那一个模块自己编译**时完成。
经验规则：

- 用**公共** API（`fetch_stocks` / `get_universe` / `validate_and_repair_stock_ohlcv`）；
- 项目自己要调用的私有名（`_load_cache`）在探针里**能用**（符号已由项目代码引入）；
- 不满足上述两条时，用 runtime 探针（`ZT_PROBE_LOC=1`）或 lldb 代替。

---

## 10. 提交与分支

- 远端：`git push agentic bootstrap`（分支 `bootstrap`；`origin` 是 https，会失败，别用）
- 最新提交：见 `git log --oneline -1`（本 handoff = 批次 286 提交之后的那一笔）
- 工作区干净（除 `.DS_Store` 与若干未跟踪目录 `.omo/`、`.ouroboros/`、`conversations/` 等，均无关）：
  批次 286 的闭包私有名兜底已随本 handoff 一起提交（见 §4.3）。

```bash
git log --oneline -10              # 看最近批次
git status --short                 # 看未提交内容
git diff src/middle/mir/gen.rs     # 看批次 286 兜底全文
```

## 11. 临时 harness 的当前内容

`~/source/quant/REasyQuant/strategies/code/_zeta_local_drv.py`（**未跟踪**，随用随改）当前装的是
**§9 的自检片段**（universe + 缓存 + 清洗三步，实测输出见 §9）。

两种常用替换：

```python
# A) 最小复现当前卡点（超时 rc=124）
from backend.datasrc.sources_selector import _ranked_fetch_sources
r = _ranked_fetch_sources("sh.159985")
print("ranked", len(r))

# B) 跑整个策略（真正目标）—— 见 §3.2
```

> 注意：这个文件在 `strategies/code/` 下，**会被语料基线扫到**（`corpus_baseline.py` 递归
> `strategies/**/*.py`）。要跑语料口径时把它移走（`mv` 到 `/tmp`），否则统计数会多一个文件。
