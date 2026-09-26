# 普查缺口族移交简报（主线修复用）

> 日期：2026-09-26　|　来源：随机差分普查（十四模式 234 例，机制见 docs/TEST-DESIGN 与 diff_test.py）
> 用途：每族一屏——最小复现 / CPython 期望 / zeta 现状 / 疑似修复位置 / 闸门用例。
> **全部闸门用例已进 diff 基线与 python_style known-fail：修好即自动转绿，无需手工对账。**
> 优先级建议按本表顺序：①②③ 是同一片"打印/格式化"邻域，可并批；④⑤⑥⑦ 各自独立。

## ① `/` 真除法缺失（连带 round 错值，11 例闸门）

```python
print(7 / 2)        # CPython 3.5        zeta 3（整数除法）
print(round(66/10)) # CPython 7 (6.6)    zeta 6（66/10 已是 6）
```
- 疑似位置：gen.rs 的 `/` 分派（整数操作数走了 floormod 路径）；round 本身已验证正确（含银行家舍入 6.5→6 / 7.5→8）。
- 闸门：`numeric_truediv` + `gen_builtin_s24680_*` 10 例。
- 注意：改真除法会让整数除法结果变 float——下游 `//` 与 `%` 不受影响，但既有代码若依赖 `/` 取整需要排查（语料 `//` 用法已全量在案）。

## ② f-string sign 旗标不渲染（4 例闸门，#188①）

```python
n = 123456
print(f"{n:+d}")    # CPython +123456    zeta 123456
print(f"{2.718:+.3f}")  # CPython +2.718  zeta 2.718
```
- 疑似位置：`py_additions.c` py_format 的规格解析——sign 被跳过但从不输出（CT35"跳过 sign"实为吞掉）。
- 闸门：`t508_fmt_sign_flag` + `gen_fmt_s2468_002/005/007/009`。

## ③ 负数进制型（7 例闸门，#188②）

```python
print(f"{-42:b}")   # CPython -101010    zeta 64 位补码全幅
```
- 疑似位置：同 py_format——负数 + b/o/x 型需要"符号 + 绝对值"路径（CPython 语义）。
- 闸门：`t509_fmt_negative_binary` + `gen_fmt_s2468_001/003/008/010/012/014/015`。

## ④ 逻辑链中比较结果丢 Bool 性（5 例闸门，#117 近亲）

```python
print(1 and 2 == 3)   # CPython False      zeta 0
print(min(3, 1, 2) == 1)  # 组合形态同理
```
- 疑似位置：gen.rs 的 and/or 结果类型采纳（M11"只采纳两侧一致的具体类型"）——比较产生的 Bool 经过 and/or 后落回 i64 位型打印。
- 闸门：`t512_bool_print_in_logic` + `gen_numeric_s20260926_001/008/010/013` + `truth_builtin_min`。

## ⑤ None 打印成 0（2 例闸门，#189）

```python
print(3 and None)   # CPython None       zeta 0
```
- 根源：None 词法降级 Lit(0)（L09 决策）+ print 无 None 渲染。
- 闸门：`t507_none_logic_print`。

## ⑥ 元组交换顺序腐蚀（1 例闸门，#190）

```python
a = 1; b = 2
a, b = b, a
print(a, b)         # CPython 2 1        zeta 2 2
```
- 根因已两轮实验定位：gen.rs 的 Assign(Tuple, Tuple) lowering **顺序赋值**，右值引用左目标时腐蚀。解析器侧两轮脱糖已试并否证（见 #190 行——模块级全局收集是结构性障碍）。
- 修法：lowering 先把 RHS 各元素求值入临时、再从临时绑定。
- 闸门：`t510_tuple_swap`。

## ⑦ 溢出回绕（~12 例闸门，需先做设计裁定）

```python
print(99999999999999999999999)   # CPython 任意精度   zeta 0（字面量静默 0）
print(2 ** 100)                  # 位回绕垃圾
```
- 两条路线二选一（属用户/主线裁定）：(a) 响亮报错（字面量超 i64 编译错误；运算溢出无法静默兜住但可插检查）；(b) 登记为已知限制并在文档明示。
- 闸门：`t503_big_literal_overflow` + `numeric_big_int` + `gen_numeric` 溢出例 + `numeric_shift_overflow`。

## ⑧ 浮点打印 %.6f（3 例闸门，呈现族——优先级最低）

```python
print(0.1 + 0.2)    # CPython 0.30000000000000004   zeta 0.300000
print(4.0)          # CPython 4.0                   zeta 4.000000
```
- 位置：py_format 浮点默认 `%.6f`——CPython 用最短往返 repr。修法 = 换 repr 算法（如 Grisu/Ryu 的 C 实现）或接受偏差并文档化。
- 闸门：`numeric_float_add/promote/mod_float`。

## ⑩ 容器比较族（批次 500 定性，5 例闸门）

```python
print([1, 2] < [1, 3])    # CPython True     zeta False
print((1, 2) < (1, 3))    # CPython True     zeta False
print((1, 2) == (1, 3))   # CPython False    zeta False ✓
d1 = {"a": 1}; d2 = {"a": 1}
print(d1 == d2)           # CPython True     zeta False
print([3] > [1, 9])       # CPython True     zeta True ✓
```
- 形状边界：list/tuple 的 `<`/`<=` 错（疑似句柄比较）、dict 的 `==` 错；`>` 与 list `==` 对——不对称。
- 疑似位置：gen.rs 比较分派对容器操作数缺元素级字典序路径（py_list_cmp shim 缺或未接）。
- 闸门：`container_cmp_order` + `list_cmp_order`。

## ⑪ 元组解包的字符串绑定无类型（批次 506 定性，一条修法治三触发）

```python
pairs = [(1, "a"), (2, "b")]
for k, v in pairs:
    print(v)            # CPython a / b     zeta 堆地址
```
- 根因：`Assign(Tuple, ·)` 同一条 lowering 路径（for 目标解包、dict 推导、
  顺序赋值）不给解包出的字符串变量类型标记 ⇒ 打印/取 len 全打地址。
- 实证矩阵：单目标循环 ✓、int 解包 ✓、**字符串解包 ✗**；505 的过滤 dict 推导、
  #190 的交换、本例——三种触发同根。
- 修法：gen.rs 的 Tuple 解包绑定为每个变量写类型标记（一次修，三族全绿）。
- 闸门：`tuple_unpack_str_type` + `container_dict_comp_filter` + `t510`。

## ⑫ 类方法上下文的字符串全族（批次 513 探针 d1/d2，**未钉——下一手**）

```python
class Greet:
    def name(self): return "world"
    def hello(self): return "hello " + self.name()
g = Greet()
print(g.hello())   # CPython "hello world"   zeta 堆地址
```
- 范围比 506 更宽：不止循环积累——**类方法里任何字符串表达式**（列表推导
  [len(n) for ...]、跨方法拼接 self.name()）都打地址；普通函数/模块级同形
  全对。int 值正常。
- 临时探针文件：/tmp/b440/b467/d1.z、d2.z（未入库未钉 known-fail——
  接手第一件事：按 t510 格式钉住后移交）。

## ⑬ 字典方法族（批次 515 扫描，2 例闸门）

```python
d = {"a": 1, "b": 2}
print(list(d.values()))   # CPython [1, 2]（插入序）   zeta [2, 1]
print(list(d.items())[0]) # CPython ('a', 1)          zeta 堆地址
```
- values() 顺序 = 哈希序非插入序（map 存储未维护插入链）；items() 元素 =
  元组解包同根（⑪ 家族）。**其余 13 个方法**（rstrip/lstrip/strip(chars)/
  replace 三参/center/zfill/split-maxsplit/list.remove/extend/count/
  dict.pop/len）实测全对——方法面覆盖度高。
- 闸门：`container_dict_methods`。

## ⑭ % 格式化操作符整体缺失（批次 511 定性，5 例闸门——最高频缺口）

```python
print("n=%d" % 42)          # CPython n=42     zeta 18（垃圾）
print("%s-%s" % ("a", "b")) # CPython a-b      zeta 地址
print("v=%.2f" % 3.14159)   # CPython v=3.14   zeta 1.507200
print("x=%x" % 255)         # CPython x=ff     zeta 70
print("p=%d%%" % 50)        # CPython p=50%    zeta 15
```
- 根因：字符串左操作数的 `%` 没接格式化路径——落到通用位运算/哈希分支。
- 修法建议：gen.rs 比较层 `%` 分派加 str 左操作数臂 → py_format 机器
  **已在**（f-string 规格实现主体扎实——批次 486 基础 11 探针全对），
  只缺 `%` 操作符的路由。
- 闸门：`str_percent_format_family` + `t513_percent_format` + `str_percent_fmt`。

## ⑨ str.find 两参形式缺 shim（1 例 compile 闸门，#188 余半）

```python
print("abcd".find("c", 2))   # CPython 2    zeta 链接期缺 find
```
- 一参形式已修（496：registry 补 `W str find host_str_find args=2`，shim 本就在）。
- 余：C 侧加 `host_str_find3(hay, needle, start)` 三参 shim + registry 行。
- 闸门：`gen_str_s314159_003`。

---

## 附：普查族以外、同属"静默错值"曝光面的既有钉子

- t401–t405、t450（dict[str,Any] 几何判形）、t450 系——known-fail 15 条总账见 python_style 每次门禁输出。
- 随机普查的 mismatch **verdict 全部自动盯防**：任何一族被修好，对应用例 XPASS 并提示摘钉；被改坏则 rc=1。
