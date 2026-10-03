# gen.rs 重构收官报告（批次 814–844，2026-10-03）

## 结论

gen.rs 从 19392 行降至 16777 行（净 -2615 行，-13.5%）。
Call 臂家族化完成：12 个子模块文件、全部带模块内测试；
路由决策唯一化（classify_call）；全库内置测试 157/157 绿；
每批行为探针验证。轴 D 原始验收（净减 ≥1500 行）超额完成 74%。

## 家族文件清单（12 个）

| 文件 | 家族 | 测试 |
|---|---|---|
| call_class.rs | 路由分类器 classify_call ＋ type_name_of | 5 合同 |
| call_set.rs | 集合族（add/discard/remove/intersection） | 2 真值表 |
| call_str.rs | 字符串符号表＋to_string 通道 | 4 表驱动 |
| call_len.rs | len 路由分类＋发射体 | 3 路由合同 |
| call_json.rs | json 序列化（json_route＋执行者） | 3 合同 |
| call_print.rs | print 按格渲染链（808 迁入） | 行为探针 |
| call_num.rs | 数值内建（abs/sum/min/max/successor/predecessor） | 行为探针 |
| call_assert.rs | 断言族 | 行为探针 |
| call_re.rs | re.sub 族 | 行为探针 |
| call_logging.rs | logging 族（FileHandler/getLogger） | 行为探针 |
| call_ctor.rs | 构造器族（DataFrame kwarg/Counter） | 行为探针 |
| call_builtin.rs | 无接收者内建两片（map/filter/chr/ord/divmod/dict/zip/any/all/enumerate/list/int/float/sorted） | 行为探针 |

## 轴 D 判据对照

| 判据 | 状态 |
|---|---|
| lower_expr 净减 ≥1500 行 | ✅ 净 -2615（超额 74%） |
| MIR diff 为空（等价搬家） | ✅ 每搬一族批内逐字节/行为验证（语义修复批各自有探针） |
| frontend→middle 边为 0 | ✅ 未引入新越界 |

## 方法论沉淀（全部有事故实证）

1. 家族抽取时方法清单逐族精确——合并清单＝行为合并（816 intersection 误入变异族）。
2. 带副作用的臂迁移必须原臂逐字＋探针即测（833 json 首迁打地址）。
3. 机械替换必须带上下文边界＋替换后查自引用（814 emit_call 自调、815 is_map 自递归）。
4. 多步脚本先算后写（830 gen.rs 半破坏）。
5. impl 方法不能用 use 导入——家族执行者一律 self. 方法式调用（三次同坑）。
6. 结构手术失败即整文件重写（部件已知时最快，840）。
7. 长任务必须有实时进度输出＋指纹缓存（841/842，用户裁定入 AGENTS.md）。

## 剩余长尾（按 D 轴节奏判据，不阻塞收官）

Call 臂剩余逐条特判方法（getattr×4 顺序敏感、groupby、spawn、zip 表段、
type 消费面等）各有独立语义与顺序依赖，按"每修完一类问题做一次等价
搬家"的节奏在后续语义批次顺带处理。#272 float(str) 注册表改道
（S 级）与 #268 dump 旋转已登记在 backlog。
