# Zeta 编译器：修改与验证方法

> 本文档记录本仓库的修改流程、验证工具链和口径约定。
> 适用分支：bootstrap（远端 agentic）。
> 最后更新：2026-09-13

---

## 1. 修改流程（每批一个 commit）

```
1. 探针复现     → 最小 .z 复现用例（/tmp/xx.z），确认当前行为
2. 定位         → pipeline_dump / indent_dump / IR dump / nm / lldb
3. 修复         → parser / gen.rs(MIR) / codegen.rs(LLVM) / runtime C
4. 验证复现用例 → zetac /tmp/xx.z -o /tmp/xx && /tmp/xx
5. 锁定回归     → 新用例进 tests/python_style/tNN_*.z（expect 注释）
6. 全量回归     → 三套测试（见 §3）
7. roadmap 更新 → 完成标记 + 根因记录
8. commit + push
```

### 提交规范

```bash
git add -A src/ runtime/ tokio_runtime.o tests/python_style/ roadmap.md
git add -f tests/python_style/*.z          # *.z 被 .gitignore 忽略，需 -f
git add -f runtime/py_additions.c          # 同上
git commit -m "feat(py-a): <一句话>          # feat(py-a) / fix(codegen) / docs / test
                                            # 正文：根因 + 修法 + 验证结果
<空行>
<详细说明：根因、修复点、影响范围>"
git push agentic bootstrap                  # origin 是 https 无凭据，用 agentic(SSH)
```

---

## 2. 诊断工具链

| 工具 | 用途 | 用法 |
|---|---|---|
| `zetac` | 编译 .z → 原生二进制 | `./target/release/zetac x.z -o x && ./x` |
| `pipeline_dump` | AST→注册→MIR 全链 dump | `./target/release/pipeline_dump x.z [-v/-ast]` |
| `indent_dump` | 预处理后的源码（缩进→{}） | `./target/release/indent_dump x.z` |
| IR dump | zetac 的 stderr 就是 LLVM IR | `./target/release/zetac x.z -o x 2>&1 \| awk '/define i64 @f/,/^}/'` |
| `nm` | 符号表（undefined 排查） | `nm binary \| grep xxx` |
| `lldb` | 运行期崩溃定位 | `lldb -b -o run -o "bt 3" ./x` |
| `ZETA_PROBE=1` | 源码内探针开关 | 部分函数有 env 探针 |
| `zorb` | 第三方库安装（最小闭环） | `zorb install <路径\|URL\|git>` / `list` / `remove` / `path`；安装到 `$ZETA_PACKAGES_DIR` 或 `~/.zeta/packages`，即 `import X` 的搜索目录 |

### 常用诊断模式

```bash
# 1) 解析停在哪儿（remaining = 未消费文本的起点）
./target/release/pipeline_dump x.z | head -1

# 2) 某函数的 MIR
./target/release/pipeline_dump x.z -v | grep -A10 "lowered f"

# 3) 某函数的 LLVM IR
./target/release/zetac x.z -o x 2>&1 | awk '/define i64 @f/,/^}/'

# 4) 未定义符号（链接失败）
./target/release/zetac x.z -o x 2>&1 | grep -A5 Undefined

# 5) C 级 runtime 验证（隔离编译器）
clang -c -O2 -I/opt/homebrew/include runtime/py_additions.c -o /tmp/add.o
# 写 C main 直接调 runtime 函数
```

---

## 3. 验证套件（三套全绿才可提交）

```bash
# 0) 第三方库安装（最小闭环，可选）
./target/release/zorb install ./mypkg     # 目录包需 __init__.py；也接受 X.py / URL / git
ZETA_PACKAGES_DIR=/tmp/pkgs ./target/release/zorb list   # 安装目录可覆盖（便于测试）

# 1) python_style（本仓库语法/语义回归，expect 注释自包含）
./tests/python_style/run.sh
#    输出: N passed, M failed, K known-fail

# 2) 官方套件（tests/unit-tests/ 为正本，/tmp/zeta_tests 为运行拷贝）
python3 - <<'EOF'
import subprocess, os, glob
def run(zetac, srcs, outdir, timeout=15):
    res = {}
    os.makedirs(outdir, exist_ok=True)
    for z in srcs:
        n = os.path.basename(z)[:-2]
        try:
            c = subprocess.run([zetac, z, '-o', f'{outdir}/{n}'],
                               capture_output=True, text=True, timeout=timeout)
            res[n] = c.returncode == 0
        except subprocess.TimeoutExpired:
            res[n] = False
    return res
cur = run('./target/release/zetac', sorted(glob.glob('/tmp/zeta_tests/*.z')), '/tmp/zt_out')
print(f"official: {sum(cur.values())}/{len(cur)}")
EOF
#    口径：当前基线 194/194 = 100%

# 3) 真实项目语料（REasyQuant）
python3 tools/corpus_baseline.py
#    口径：解析通过率（parse 层）38/38；编译通过 7/38（其余为外部库符号）
```

### 用例编写规范（tests/python_style/）

```python
// PY-A: <特性说明>                    # 头注释
// expect: 42                          # 按序声明每行期望输出
// expect-error                        # 负面用例（编译必须失败）
// known-fail: <原因>                  # 已知缺口（不计失败）
<纯 Python 语法代码>
```

runner (`run.sh`) 自动：编译→运行→逐行比对。**期望行必须与实际输出
逐字匹配**（f64 是 `%.6f` 格式如 `2.500000`）。

---

## 4. Runtime 修改流程（C 层）

```
1. 改 runtime/tokio_runtime_stub.c（新函数）或 tokio_runtime.c（异步）
2. 重编并合并（**只含这两个文件**）：
   clang -c -O2 -I/opt/homebrew/include -DZT_REAL_ASYNC runtime/tokio_runtime_stub.c -o /tmp/stub.o
   clang -c -O2 -I/opt/homebrew/include tokio_runtime.c -o /tmp/async.o
   ld -r /tmp/async.o /tmp/stub.o -o tokio_runtime.o
3. codegen.rs init 处加 LLVM 声明（签名必须与 C 一致，f64 参数尤其注意）
4. gen.rs 加分发（method 分发或 free-call 分发）

**不要**把 runtime/py_additions.c 并进 tokio_runtime.o —— 它归 `zeta_runtime_c.o`
（编译时两个 .o 一起链接）。混入会造成 ~166 个 duplicate symbol 链接失败。
```

**已知坑**：
- f64 参数的 extern 声明若写 i64 → fptosi 截断（abs(-2.5)→nan 的根因）
- 同名不同 arity 的运行时函数会被消歧逻辑加 `_N` 后缀 → zeta_ 前缀豁免
  （5 处改名点统一豁免）
- **方法名撞 libc**：`isalnum`/`isspace`/`strftime` 等若未入分发表，会静默链接到
  **同名 libc 函数**（签名不符 → 静默错值/崩溃），而非链接失败。新增 str 方法务必
  在 `gen.rs::str_method_symbol`（或 arity 特判）登记
- ~~StackArray 无 `[cap|len]` header~~ —— **2026-09-13 已统一**：数组句柄一律指向
  数据区，header `[cap|len]` 在 `handle-16`（与 vec_len/vec_get 同布局）。
  `array_len` 读 header（null 安全）；静态尺寸仍走编译期常量折叠。

---

## 5. 架构速查（改哪里）

| 层 | 文件 | 职责 |
|---|---|---|
| 预处理 | `src/frontend/indent.rs` | 缩进→`{}`；tab 归一化；字符串/三引号状态机 |
| Parser | `src/frontend/parser/{expr,stmt,top_level}.rs` | Python 语法→AST；desugar 大多在这层 |
| 语义/特化 | `src/middle/resolver/resolver.rs` | 符号收集；nonlocal/global 集合；闭包转存 |
| MIR 降级 | `src/middle/mir/gen.rs` | AST→MIR；方法分发/内置函数/闭包合成 全在这 |
| LLVM | `src/backend/codegen/codegen.rs` | MIR→IR；函数声明/改名豁免/类型适配 |
| Runtime | `runtime/py_additions.c` | 新增 Python 语义 C 函数（stdarg/Vec/map） |
| 合成 | `src/frontend/parser/top_level.rs` 末尾 | 隐式 main 合成；class 脱糖展开 |

### Python 特性 desugar 速查

| 语法 | 落点 | desugar 目标 |
|---|---|---|
| class | top_level.rs | struct + impl + 构造 fn |
| with | stmt.rs parse_with | `NAME = EXPR` 绑定 + body |
| try/except | stmt.rs | `_setjmp` 直调 + `zeta_raise` longjmp |
| nonlocal/global | stmt.rs | `zeta_nonlocal_decl` 标记 → env 路由 |
| comprehension 家族 | expr.rs | `__collect__`/`__collect_dict__` + lambda |
| lambda | expr.rs | Closure 节点 → 合成函数 `__closure_N` |
| starred `f(*a)` | gen.rs args 构建 | 编译期 unroll（V1 静态数组） |
| walrus `x := e` | expr.rs | Assign 节点（lower_expr Assign arm） |

---

## 6. 口径与红线

- **性能红线**：不引入动态类型/动态派发。所有 Python 兖容在
  预处理+parser+MIR 分发层，AST 结构不变（除 desugar 为既有节点组合）。
- **回归红线**：官方 194/194 不得下降；python_style 不得新增失败。
- **fail-open 禁止**：不支持的语法必须报错或记录，不得静默吞掉产生错值
  （class/f-string 曾犯过，已修）。
- ~~**布局债**：StackArray/DynamicArray 双布局不一致~~ —— **2026-09-13 已关闭**
  （stack/dynamic/切片/字面量统一 `[cap|len]` 布局）；新数组函数仍建议提供
  len 参数变体，以便静态尺寸走编译期常量折叠。
