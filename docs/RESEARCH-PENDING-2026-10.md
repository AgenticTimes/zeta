# 待裁三项的调研决策材料（2026-10-06，批 1013）

> 调研范围：动态语言值表示（NaN boxing / tagged pointer / 元素策略格）、
> 渐进类型边界语义（guarded / transient / monotonic）、Python AOT 编译器
> 生产先例（mypyc / Cython / CPython 3.11–3.14 特化与 JIT）。
> 结论先行：三项全部有主流先例背书的落地路径，不需要发明新机制。

## 项 1：轴 B 值标签（M3 起的动态层形态）

### 主流谱系（值表示三派）

| 派 | 代表 | 特点 |
|---|---|---|
| 全装箱 PyObject* | CPython（3.11→3.14 不改值表示） | Faster CPython 的提速全部来自 **PEP 659 特化自适应解释器＋内联缓存**（3.11）、Tier 2 uop（3.12/3.13）、copy-and-patch JIT（3.13/3.14 PEP 744）与 PEP 683 immortal objects——**不动 PyObject\* 本身** |
| NaN boxing | JavaScriptCore、LuaJIT（GC64） | 64 位 double 同时编码类型＋值 |
| 标签指针＋压缩指针 | V8（32 位压缩指针＋Smi 低 位标签） | cage 4GB 假设 |

### 关键主流形态：容器元素策略格（比逐元素 tag 更主流）

- **V8 elements kinds**：PACKED_SMI → PACKED_DOUBLE → PACKED_ELEMENTS 的
  **单向退化格**（越往下越慢、只降不升，升回需全量验证）。
- **PyPy list/dict strategies**：int/string/any 策略单向迁移，同思想。
- **zeta 已有同构先例**：字典值侧表（0=int 1=f64 2=str 3=bool）就是
  退化格的雏形——refactor §2 B.3 先例 3。

### 渐进类型边界语义三派（学界共识分法）

| 派 | 代表 | 成本结构 |
|---|---|---|
| guarded（深合同） | Typed Racket | 精确但边界可叠 coercion，"Is Sound Gradual Typing Dead?" 记录大量 >10x 配置 |
| transient（逐点检查） | Reticulated Python | 便宜的一次性检查＋模块级 blame |
| monotonic（单调引用） | Herman 空间高效、Sergey & Vitek 单调引用、Grift | **cast 抬类型不叠 coercion；untyped→typed 一次包装永久有效**——渐进类型性能问题的主流答案 |

### 对 zeta 的映射与裁定建议

refactor B.4 的分层混合 D 与主流完全同向。**M3 起的具体形态建议**：
1. 容器动态域用**元素策略格**（V8/PyPy 双验证），不做逐元素 tagged
   union——字典值侧表升级为容器头部的策略字＋单向迁移。
2. 边界包装采用 **monotonic 语义**（untyped→typed 一次包装、缓存，
   不叠 coercion）——py_getattr_dynamic＋zeta_callN（批 395/996 机械）
   就是包装点的执行器。
3. 静态可达处维持 TypeScript 式擦除（零运行期检查）——mypyc/Cython
   的 typed/untyped 双表示＋边界 coerce 是 AOT 生产先例。

**裁定请求**：批准按 M0（边界清单）→M1（bool 掩码）→M2（签名传播）
排批；M3 形态按上述"策略格＋monotonic 包装"修订立项文档第 5 节。

## 项 2：NoneValue over-claim 推断层

### 关键事实重估

原裁定（批 999）"需先建 opaque 动态成员调用机械"——**机械已存在**：
批 395 建 zeta_call0..4 蹦床，批 996 的 NoneValue 守卫已经是
py_getattr_dynamic＋zeta_callN 的动态成员调用（megamorphic helper 的
雏形）。推断层修（NoneValue claim → PyDynamic）的执行前提已满足。

### 主流形态（IC 分层）

V8/JSC 的调用点分派：未初始化 → 单态（tag 测试＋直调）→ 多态 →
**超态（megamorphic runtime helper）**。AOT 无 deopt 的等价物＝
"已知 tag 静态分派＋其余落 megamorphic helper"。zeta 的对应改造：
1. emit_* 分发守卫从 `tn == "NoneValue"` 泛化为
   `untyped/NoneValue 接收者 → py_getattr_dynamic + zeta_callN`
   （其余已知 Named 型维持静态分派）。
2. checker 端：部分证据的 NoneValue claim 降为 PyDynamic
   （"still unknown" 的既定拼写，types/mod.rs 注释原话）。

### 裁定建议

**批准一个小批实施**（预估 1 批）：改动面＝checker claim 降级＋守卫
泛化两处，护栏＝全量门禁。收益：类型不再说谎（print 渲染、下游定型
不再被空值标记污染），t73 同族（read_text 返回域）顺路受益。

## 项 3：backlog 老家族

### #36 selfhost 91 行（if let 构造子模式）

主流定式：ADT＝tagged union（discriminant ＋ payload）＋按 tag switch
＋字段投影——Rust match / Haskell case / Swift enum 的标准 lowering；
Python 阵营 PEP 634 match 也编译为 isinstance 链。zeta 缺的是**枚举
变体的标签字表示**（批 382 实证：构造子模式无语义、载荷无标签字）。

**裁定建议**：**与轴 B M3 合并立项**（同一表示层地基：tag word 的
静态枚举版与动态 cell 版共用 zj 格式与投影机制），不单独排批。
backlog 该项状态改"并入轴 B M3"。

### #26 JIT trap 族

已收口（批 1012：472→761 的 4→**0**）。余下 fail=1（integration_
all_features 的 map_insert 运行期异常）与 xabort=9（设计内响亮）各自
独立登记，不属于 trap 族。**该项可关闭**。

### ASan 夜航

已 CI 化（批 1011）。余量仅"首夜跑完后的红灯分诊"——等首个定时触
发即有数据。

## 汇总裁定单

| 项 | 建议 | 规模 |
|---|---|---|
| 轴 B | 批准立项；M0/M1/M2 立即排批；M3 形态修订为策略格＋monotonic | 7–10 批 |
| NoneValue | 批准小批（checker 降级＋守卫泛化） | 1 批 |
| #36 | 并入轴 B M3；backlog 关闭 | 0（随 M3） |
| #26 | 已收口，关闭 | 0 |

## 来源

- 值表示三派：[V8 tagging vs NaN boxing (SO)](https://stackoverflow.com/questions/63550957/why-does-v8-uses-pointer-tagging-and-not-nan-boxing)、[JSC/V8 值表示深度文](https://witch.work/en/posts/javascript-trip-of-js-value-tagged-pointer-nan-boxing)、[wingolog 经典综述](https://www.wingolog.org/archives/2011/05/18/value-representation-in-javascript-implementations)、[ExBoxing 混合式](https://medium.com/@kannanvijayan/exboxing-bridging-the-divide-between-tag-boxing-and-nan-boxing-07e39840e0ca)
- 元素策略格：[V8 elements kinds](https://v8.dev/blog/elements-kinds)、[PyPy list strategies](https://pypy.org/posts/2011/10/more-compact-lists-with-list-strategies-8229304944653956829.html)、[PyPy 优化文档](https://doc.pypy.org/interpreter-optimizations.html)
- 渐进类型：[Is Sound Gradual Typing Dead?（Takikawa et al.）](https://drops.dagstuhl.de)、[Reticulated transient 性能分析](https://www.researchgate.net)、[monotonic references（Sergey & Vitek, ESOP 2015）](https://www.researchgate.net)
- CPython 特化/JIT：[PEP 659](https://peps.python.org/pep-0659)、[PEP 744](https://peps.python.org/pep-0744)、[LWN JIT 分析](https://lwn.net/Articles/958350)、[3.14 tail-call](https://docs.python.org/3/whatsnew/3.14.html)、[nelhage 分析](https://blog.nelhage.com/post/cpython-tail-call)
- AOT 先例：[mypyc](https://github.com/mypyc/mypyc)、[mypyc C 代码生成（双函数边界 coerce）](https://github.com/python/mypy/wiki/Mypyc-C-Code-Generator)、[Cython 静态类型文档](https://cython.readthedocs.io)
