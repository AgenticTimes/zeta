//! 历史缺陷的编译期回归测试
//!
//! 用途：把已经修好的缺陷在**进程内**复验住——`cargo test --test regression_history`
//! 一次编译、每条用例毫秒级，替代"每次都跑全量差分/python_style"。
//!
//! 覆盖边界（如实说明）：这里只收**编译期可观测**的缺陷（降形选错了运行期符号、
//! 类型标记丢了、参数被丢弃）。凡是只能从运行期打印值观测的缺陷（切片越界夹尾、
//! 字符串越界等），仍由 `tests/python_style/` 与 `tools/diff_test.py` 承担——
//! 那类需要真执行，挪不进毫秒级检查。
//!
//! harness 走到"降形"为止（解析 → 常量求值 → 宏展开 → 注册 → 类型检查 →
//! lower_to_mir），**不跑**单态化、`refine_param_types` 和后端。所以只有
//! 结论落在 `Mir`（符号名／槽位／类型标记）上的缺陷能进这里。
//!
//! 每条测试的期望值来源是**缺陷记录**（roadmap.md 批次节 + backlog.md 任务格 +
//! 改前二进制实拍），不是"现行输出是什么就写什么"。加新条目的步骤：
//! 1. 从 roadmap.md 找到那一批的根因站点与症状；
//! 2. 写一个能打到该站点的最小 `.z` 片段（三五行，别把整份语料搬进来）；
//! 3. 拿 `target/release/zetac --dump-mir <片段>` 确认形状，再把期望写成对
//!    `Mir` 结构的定向断言（只查那一处符号/类型，禁止整份 MIR 逐字节比对）；
//! 4. 把期望值改成症状值，确认这条测试会红（证明它不是空跑），改回后提交。

use zetac::frontend::ast::AstNode;
use zetac::frontend::parser::top_level::parse_zeta;
use zetac::middle::mir::mir::{Mir, MirExpr, MirStmt};
use zetac::middle::resolver::resolver::Resolver;
use zetac::middle::types::Type;

/// 源码 → 全部函数的 MIR（按名排序，保证读数可复现）。
/// 流程与 `src/main.rs` 的编译主干一致：解析 → 注册 → 类型检查 → 降形。
///
/// 降形走在大栈线程上：`MirGen::lower_expr_node` 是单个巨型函数，debug 构建里
/// 一层栈帧就有几十 KB，测试线程默认 2 MiB 会在 `format!` 这类嵌套表达式上溢栈
/// （批次 10012 实拍：`cargo test` 全threads 卡死、`has overflowed its stack`）。
/// 释放构建走主线程 8 MiB 没这个问题，所以这不是产品缺陷，是测试面的构建形态。
fn lower_all(src: &str) -> Vec<Mir> {
    let src = src.to_string();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || lower_all_inner(&src))
        .expect("起大栈线程失败")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

fn lower_all_inner(src: &str) -> Vec<Mir> {
    let (remaining, asts) = parse_zeta(src).unwrap_or_else(|e| panic!("解析失败: {e:?}"));
    assert!(
        remaining.trim().is_empty(),
        "解析没有吃满输入，剩余: {:?}",
        remaining.trim()
    );

    // 与 CLI 文件模式同序：解析后先走常量求值（`-2` 这类在这里折成字面量），
    // 少了这一步 harness 的读数会和真实编译不一致。
    let asts = zetac::middle::const_eval::evaluate_constants(&asts)
        .unwrap_or_else(|e| panic!("常量求值失败: {e}"));

    let mut resolver = Resolver::new();
    // 与 main.rs 主干同序：先展开宏，再注册展开后的 AST（少这一步，
    // `-2` 这类常量在 harness 里就不会折成字面量，读数与 CLI 不一致）。
    let expanded = resolver
        .expand_macros(&asts)
        .unwrap_or_else(|e| panic!("宏展开失败: {e}"));
    for ast in &expanded {
        resolver.register(ast.clone());
    }
    // 与 main.rs:837-842 同序：注册后、类型检查前跑一次返回类型推断，
    // `classify()`＋形参类形冲突（批次 400/652 那一层）就发生在这趟里。
    // 少了这一步，形参类型永远停在 `PyDynamic`，凡是"参数被钉成什么类型"的缺陷都观测不到。
    resolver.infer_untyped_returns(&expanded);
    let _ = resolver.typecheck(&expanded);

    let mut mirs: Vec<Mir> = resolver
        .get_registered_funcs()
        .iter()
        .map(|a| resolver.lower_to_mir(a))
        .collect();
    // 顶层语句由 parse_zeta 合成成 `main`（synthesize_implicit_main），
    // 它不在 get_registered_funcs 里，但模块级调用点就在那儿，两边都要收。
    for ast in &expanded {
        if let AstNode::FuncDef { name, .. } = ast {
            if !mirs.iter().any(|m| m.name.as_deref() == Some(name.as_str())) {
                mirs.push(resolver.lower_to_mir(ast));
            }
        }
    }
    for extra in resolver.take_generated_closures() {
        mirs.push(extra);
    }

    mirs.sort_by(|a, b| {
        a.name
            .as_deref()
            .unwrap_or("~anon")
            .cmp(b.name.as_deref().unwrap_or("~anon"))
    });
    mirs
}

fn mir<'a>(mirs: &'a [Mir], name: &str) -> &'a Mir {
    mirs.iter()
        .find(|m| m.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("没有降出函数 {name}（实得 {:?}）", mirs.iter().map(|m| &m.name).collect::<Vec<_>>()))
}

/// 函数体里所有 `Call`/`VoidCall` 的被调符号名（含嵌套块）。
fn call_symbols(m: &Mir) -> Vec<String> {
    fn walk(stmts: &[MirStmt], out: &mut Vec<String>) {
        for s in stmts {
            match s {
                MirStmt::Call { func, .. } => out.push(func.clone()),
                MirStmt::VoidCall { func, .. } => out.push(func.clone()),
                MirStmt::If { then, else_, .. } => {
                    walk(then, out);
                    walk(else_, out);
                }
                MirStmt::For { body, else_body, .. } | MirStmt::While { body, else_body, .. } => {
                    walk(body, out);
                    walk(else_body, out);
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&m.stmts, &mut out);
    out
}

/// 顶层 `Assign { lhs, rhs }` 对（按出现顺序）。
fn top_assigns(m: &Mir) -> Vec<(u32, u32)> {
    m.stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Assign { lhs, rhs, .. } => Some((*lhs, *rhs)),
            _ => None,
        })
        .collect()
}

/// 取整数字面量值：实参槽常是 `Var(id)` 一跳，字面量在把它赋进来的那行右侧。
fn int_lit(m: &Mir, id: u32) -> Option<i64> {
    let direct = match m.exprs.get(&id) {
        Some(MirExpr::IntLit(v)) => return Some(*v),
        Some(MirExpr::Var(_)) | None => None,
        _ => return None,
    };
    let rhs = m.stmts.iter().find_map(|s| match s {
        MirStmt::Assign { lhs, rhs, .. } if *lhs == id => Some(*rhs),
        _ => None,
    })?;
    match m.exprs.get(&rhs) {
        Some(MirExpr::IntLit(v)) => Some(*v),
        _ => direct,
    }
}

/// 找到把值赋进 `lhs` 槽那条语句的右侧表达式 id，跟着 `If`／`While` 的块走下去。
fn find_assign_rhs(stmts: &[MirStmt], lhs: u32) -> Option<u32> {
    for s in stmts {
        match s {
            MirStmt::Assign { lhs: l, rhs, .. } if *l == lhs => return Some(*rhs),
            MirStmt::While { body, else_body, .. } => {
                if let Some(r) = find_assign_rhs(body, lhs).or_else(|| find_assign_rhs(else_body, lhs)) {
                    return Some(r);
                }
            }
            MirStmt::If { then, else_, .. } => {
                if let Some(r) = find_assign_rhs(then, lhs).or_else(|| find_assign_rhs(else_, lhs)) {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

/// 批次 10002（移植 bootstrap 批次 810）／backlog 旧 #266③前身。
/// 症状（改前二进制 target/release/zetac.pre740、zetac.pre662 实拍）：
/// `xs.mean()` 走兜底臂发 `zeta_identity`、打印走 `println_i64`——
/// 把向量句柄当整数用（`statistics.mean([1.0, 2.0])` 打 10）。
/// 现树：静态向量接收者折叠成 `zeta_mean_vec`，目的槽标 F64。
#[test]
fn mean_fold_routes_static_vector_to_vector_fold() {
    let mirs = lower_all(
        "def f():
    xs = [1.0, 2.0]
    return xs.mean()

print(f())
",
    );
    let f = mir(&mirs, "f");

    let calls = call_symbols(f);
    assert!(
        calls.iter().any(|c| c == "zeta_mean_vec"),
        "静态向量的 mean 应折叠到 zeta_mean_vec，实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "zeta_identity"),
        "不应再走兜底臂 zeta_identity（那是改前的症状），实得调用: {calls:?}"
    );

    let dest = f
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func == "zeta_mean_vec" => Some(*dest),
            _ => None,
        })
        .expect("找得到 zeta_mean_vec 调用");
    assert_eq!(
        f.type_map.get(&dest),
        Some(&Type::F64),
        "折叠结果必须标 F64（目的槽 id={dest}）"
    );
}

/// 批次 10004（backlog 旧 #267）：收掉批次 10002 自己引入的回归。
/// 症状：折叠臂当时还接"类型未知"的接收者（PyDynamic／I64／None）并一律标 F64；
/// 语料形状 `def f(data): return data.rolling(w).mean()` 的接收者是对象句柄，
/// 标了 F64 之后经返回型回填污染调用点，调用点对返回的 Series 做字典下标
/// 就打到 codegen.rs:5462 的 `into_int_value()` 硬转（编译期崩溃）。
/// 现树：接收者不是静态向量时不折叠，结果保持句柄型（不是 F64）。
#[test]
fn mean_fold_abstains_when_receiver_type_is_unknown() {
    let mirs = lower_all(
        "def f(data):
    return data.rolling(3).mean()

print(f([1.0, 2.0, 3.0]))
",
    );
    let f = mir(&mirs, "f");

    assert_eq!(
        f.type_map.get(&1),
        Some(&Type::PyDynamic),
        "前置条件：接收者参数 id=1 静态类型未知（PyDynamic）"
    );
    let calls = call_symbols(f);
    assert!(
        !calls.iter().any(|c| c == "zeta_mean_vec"),
        "接收者类型未知时不得折叠（折叠了就会把句柄标成 F64），实得调用: {calls:?}"
    );

    let ret_val = f
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Return { val } => Some(*val),
            _ => None,
        })
        .expect("函数有返回");
    assert_ne!(
        f.type_map.get(&ret_val),
        Some(&Type::F64),
        "返回值 id={ret_val} 不得被标成 F64（那是批次 10002 引入、10004 收掉的污染）"
    );
}

/// 批次 10001（本树）／主树批次 813（backlog 旧 #268②）。
/// 症状：未标注返回型的 `def` 返回浮点时，调用点目的槽丢掉浮点型
/// （两张类型表以不同键存同一条信息，其中一张只有裸名）。
/// 现树：模块级 `x = half()` 的目的槽拿到 F64。
#[test]
fn unannotated_float_return_keeps_f64_at_module_call_site() {
    let mirs = lower_all(
        "def half():
    return 1.5

x = half()
print(x)
",
    );
    let callee = mir(&mirs, "half");
    assert_eq!(
        callee.signature_ret_ty(),
        Some(Type::F64),
        "前置条件：被调函数按函数体回填出 F64"
    );

    let main = mir(&mirs, "main");
    let dest = main
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func.starts_with("half") => Some(*dest),
            _ => None,
        })
        .expect("main 里有对 half 的调用");
    assert_eq!(
        main.type_map.get(&dest),
        Some(&Type::F64),
        "调用点目的槽 id={dest} 必须拿到浮点型（改前它是整数型，浮点值被当整数存）"
    );
}

/// 批次 10003（移植 bootstrap 批次 811）：负起点切片的归一化 +
/// "省略终点"哨兵与显式负终点分离。
/// 症状：编译期把负起点提前加 len（长度未知时算错），且省略终点与显式负终点
/// 混用同一个值，运行期分不出"到末尾"和"倒数第 N"。
/// 现树：起点原样传负数，省略终点传 i64::MIN 哨兵，归一化交给运行期。
#[test]
fn negative_slice_start_stays_negative_with_omitted_end_sentinel() {
    let mirs = lower_all(
        "def tail(xs):
    return xs[-2:]

print(tail([1, 2, 3, 4]))
",
    );
    let f = mir(&mirs, "tail");
    let (base, start, end) = f
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Call { func, args, .. } if func == "zeta_slice_vec" && args.len() == 3 => {
                Some((args[0], args[1], args[2]))
            }
            _ => None,
        })
        .expect("`xs[-2:]` 应降成 zeta_slice_vec(基座, 起点, 终点)");

    assert_eq!(int_lit(f, start), Some(-2), "负起点必须原样传负数，不在编译期提前加长度；本函数 MIR：\n{}", f.dump_canonical());
    assert_eq!(
        int_lit(f, end),
        Some(i64::MIN),
        "省略的终点必须是 i64::MIN 哨兵（与显式负终点区分开）"
    );
    assert_eq!(base, 1, "基座应是接收者本身（id=1）");
}

/// 批次 736（backlog 旧 #190）：元组交换腐蚀。
/// 症状：`a, b = b, a` 直写成两条 `a ← b`、`b ← a`，第二条读到的是已被覆盖的 a，
/// 两边最后同值。
/// 现树：先把两侧读进临时槽，再写回目的槽。
#[test]
fn tuple_swap_snapshots_before_writing_live_slots() {
    let mirs = lower_all(
        "def swap():
    a = 1
    b = 2
    a, b = b, a
    return a

print(swap())
",
    );
    let f = mir(&mirs, "swap");
    let assigns = top_assigns(f);

    // 由字面量初始化的槽＝活变量（a、b）；其余＝临时槽。
    let literal_ids: Vec<u32> = f
        .exprs
        .iter()
        .filter_map(|(id, e)| matches!(e, MirExpr::IntLit(_)).then_some(*id))
        .collect();
    let live: Vec<u32> = assigns
        .iter()
        .filter(|(_, rhs)| literal_ids.contains(rhs))
        .map(|(lhs, _)| *lhs)
        .collect();
    assert_eq!(live.len(), 2, "前置条件：a、b 两个活变量由字面量初始化，实得 {live:?}");
    let (a, b) = (live[0], live[1]);

    assert!(
        !assigns.contains(&(a, b)) && !assigns.contains(&(b, a)),
        "不得直写目的槽（`a ← b` / `b ← a` 同时存在就是腐蚀形），实得赋值: {assigns:?}"
    );

    let temps: Vec<u32> = assigns
        .iter()
        .map(|(lhs, _)| *lhs)
        .filter(|id| !live.contains(id) && !literal_ids.contains(id))
        .collect();
    let snap_to = |src: u32| {
        temps
            .iter()
            .any(|t| assigns.contains(&(*t, src)))
    };
    assert!(
        snap_to(b) && snap_to(a),
        "两侧都要先快照进临时槽再写回，实得赋值: {assigns:?}（临时槽 {temps:?}，活变量 {live:?}）"
    );
    assert!(
        assigns.contains(&(a, temps[0])) || assigns.contains(&(b, temps[0])),
        "写回的目的槽应取自临时快照，实得赋值: {assigns:?}"
    );
}

/// 批次 739：带注解的星号参数打断整个顶层项。
/// 症状：`parse_star` 不吃 `: 类型`，星号参数一旦带注解就报 W1002 并把整个
/// 顶层项丢掉（语料 310 文件里 10739 行丢行）。
/// 现树：两个参数都收进来（位置参数 + 星号参数），顶层项完整。
#[test]
fn star_param_with_annotation_keeps_both_params() {
    let mirs = lower_all(
        "def sum_all(first: int, *rest: int):
    total = first
    for x in rest:
        total = total + x
    return total

print(sum_all(1, 2, 3))
",
    );
    let f = mir(&mirs, "sum_all");

    let names: Vec<&str> = f.param_indices.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["first", "rest"],
        "带注解的星号参数不得被丢掉（丢掉时只剩 first，实得 {names:?}）"
    );
    let param_inits = f
        .stmts
        .iter()
        .filter(|s| matches!(s, MirStmt::ParamInit { .. }))
        .count();
    assert_eq!(param_inits, 2, "两个参数都应有 ParamInit，实得 {param_inits}");
}

/// 批次 737（`189e2716`，站点 `src/middle/mir/gen.rs` 的 `py_array_concat` 分支）：
/// 列表拼接的元素型合并。
/// 症状（缺陷记录原文）：元素型无条件取**左侧**，而 `out = []` 这类槽的元素型
/// 退化成 I64，于是 `out + [[1, 2]]` 拼完仍标 I64，读每一行打的是裸指针。
/// 现树规则：两侧之一是 I64、另一侧有真实型时取真实型；真冲突才保留左侧。
#[test]
fn concat_element_type_prefers_the_non_degenerate_side() {
    let mirs = lower_all(
        "xs = [1, 2]
ys = [1.5]
zs = xs + ys
print(zs)
",
    );
    let m = mir(&mirs, "main");

    let dest = m
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func == "py_array_concat" => Some(*dest),
            _ => None,
        })
        .expect("找得到 py_array_concat 调用");
    assert_eq!(
        m.type_map.get(&dest),
        Some(&Type::DynamicArray(Box::new(Type::F64))),
        "左侧 I64、右侧 F64 的拼接应取右侧真实型；改前取左侧退化成 \
         DynamicArray(I64)（槽 {dest} 实得 {:?}）",
        m.type_map.get(&dest)
    );
}

/// 批次 404（`c443c34a`，站点 `src/frontend/macro_expand.rs:191` 的 `expand_format`）：
/// `format!` 不再是桩。
/// 症状（roadmap.md:16231 记录）：`expand_format` 不看参数，整个宏无条件返回写死的
/// `StringLit("formatted string")`——语料 29 个文件 / 121 处 `format!(` 全打在同一个串上，
/// 编译成功、退出码 0、一声不出（`format!("value={}", 42)` 实测打 `[0]`）。
/// 现树规则：首参是字面量时按 `{}` 切段，每个值段发一次 `__fmtspec__`（下型到
/// `py_fmt_*`），与字面量段用 `+` 串接。
#[test]
fn format_bang_splits_each_value_segment_into_fmt_dispatch() {
    let mirs = lower_all(
        "n = 7
s = format!(\"value={} end={}\", n, 3)
print(s)
",
    );
    let m = mir(&mirs, "main");

    let calls = call_symbols(m);
    let value_segments = calls.iter().filter(|c| c.starts_with("py_fmt_")).count();
    assert_eq!(
        value_segments, 2,
        "两个 {{}} 孔应各发一次 py_fmt_* 派发（改前是 0 次），实得调用: {calls:?}"
    );

    let stub = m
        .exprs
        .values()
        .filter(|e| matches!(e, MirExpr::StringLit(s) if s == "formatted string"))
        .count();
    assert_eq!(
        stub, 0,
        "整个宏塌成写死串 \"formatted string\" 是改前的桩症状，不应出现在 MIR 里"
    );
}

/// 批次 738（`79be2515`，站点 `src/frontend/parser/top_level.rs` 的顶层项守卫）：
/// 保留字作普通标识符的**赋值形**不打断整份文件。
/// 症状（本批提交信息＋`kw_starts_declaration` 文档注释）：`struct`/`enum`/`trait`/
/// `mod`/`pub` 曾在 `DEFINITION_KEYWORDS` 里无条件命中，`struct = 5` 被当成一次失败的
/// 声明 ⇒ `many0` 就地停止，**后面的内容全被截掉**（W1002，按名逐个实测过）。
/// 现树规则：这五个名后面必须跟名字才算声明；`struct = 5` 落回 `parse_stmt`。
#[test]
fn reserved_word_assignment_forms_do_not_truncate_the_file() {
    let mirs = lower_all(
        "struct = 4
enum = 5
trait = 6
mod = 7
pub = 8
def after() -> i64:
    return 11

print(after())
",
    );
    // 截断时 `after` 这条声明根本进不来，所以先拿它做"文件确实吃满了"的正向证据。
    let after = mir(&mirs, "after");
    assert_eq!(
        after.exprs.values().filter(|e| matches!(e, MirExpr::IntLit(11))).count(),
        1,
        "五个保留字赋值之后的声明必须还在（`after` 体应返回字面量 11）"
    );

    let m = mir(&mirs, "main");
    let mut values: Vec<i64> = top_assigns(m)
        .iter()
        .filter_map(|(_, rhs)| int_lit(m, *rhs))
        .collect();
    values.sort();
    assert_eq!(
        values,
        vec![4, 5, 6, 7, 8],
        "五个保留字赋值形都该收进 main（截断时一个都没有），实得 {values:?}"
    );
}

/// 批次 652（`4a016fe8`，站点 `src/middle/resolver/resolver.rs` 的 `classify()`＋参数冲突）：
/// 同一个形参既接字符串、又接数组字面量时，两侧类型跨族必须让参数退化 `PyDynamic`、`len()` 发
/// `zeta_dyn_len`，绝不能只按字符串侧证据把参数判定成 Str 后发 `str_len`。
/// 症状（批次 652 记录）：`classify()` 缺 `ArrayLit`/`DynamicArrayLit` 分支，数组侧的类型证据
/// 对参数冲突检测不可见 ⇒ 参数被单方面判定成 Str ⇒ `len(cache["k"])` 返回 0、
/// `len([1,2,3])` 返回 1（静默错值，不报错）。
/// 形态说明（批次 10013 实测两处）：`classify()` 与形参类形冲突都发生在
/// `Resolver::infer_untyped_returns`（`resolver.rs:1361`，CLI 在 `main.rs:842` 调用），
/// 本 harness 原先不跑这一趟，三种写法（字典下标、字典 vs 数组、单侧数组）在撤掉数组分支前后
/// 读数一字不变 ⇒ harness 已补上该趟（与 CLI 同序）；实参形状取"同一形参既接字符串又接数组"，
/// 这才是记录里"数组证据被吞 ⇒ 参数单方面判成 Str"的那一侧。
/// 该批第 2 层修法（`zeta_dyn_len` 在 `codegen.rs:990` 的注册）属后端面，本套只走到降形，覆盖不到。
#[test]
fn len_on_param_shared_by_str_and_array_degrades_to_dynamic_route() {
    let mirs = lower_all(
        r#"def foo(x):
    return len(x)

r1 = foo("abcd")
r2 = foo([1, 2, 3])
print(r1)
print(r2)
"#,
    );
    let f = mir(&mirs, "foo");
    let (param_name, param_slot) = &f.param_indices[0];
    assert_eq!(param_name, "x", "读的是哪个形参要跟实现在册一致，实得 {param_name}");
    assert_eq!(
        f.type_map.get(param_slot),
        Some(&Type::PyDynamic),
        "字符串与数组字面量的跨族冲突应让共享形参退化 PyDynamic（改前按字符串侧判定成 Str），实得 {:?}",
        f.type_map.get(param_slot)
    );
    let calls = call_symbols(f);
    assert!(
        calls.iter().any(|c| c == "zeta_dyn_len"),
        "退化后的 `len()` 应发 `zeta_dyn_len`，实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "str_len"),
        "改前症状就是把动态参数当字符串取长度，`str_len` 不应出现，实得调用: {calls:?}"
    );
}

/// 批次 378（`0ee8b855`，站点 `src/middle/resolver/resolver.rs` 的 `expand_macros_in_node`）：
/// `match` 臂体、块臂里带 `ExprStmt` 外壳的语句、`let` 初值位、`x =` 右值位里的宏都要展开。
/// 症状（批次 378 记录＋夹具 `tests/python_style/t419_match_arm_macros.z`）：递归缺这四支，
/// 臂里的 `println!` 保持未展开的 `MacroCall`，而 MIR 生成对未展开宏是"悄悄跳过"
/// （`gen.rs:2888` 语句位、`:3242` 表达式位换成 `IntLit(0)`）⇒ 编译成功、退出码 0、
/// 臂里的副作用一条都不发。
/// 期望值来源：本夹具按记录里的四种形态写 8 个 `println!` 出现点，每个点展开成 2 个节点
/// （378 记录对批次 369 零效果的查证：" `println!` 展开是 2 个节点"）⇒ 16 次打印；
/// 改前这 8 个点全在臂里，一次都不会发（0）。
#[test]
fn macros_in_match_arms_and_value_positions_reach_mir() {
    let mirs = lower_all(
        r#"fn main() -> i64 {
    let n = 2
    let mut hits = 0
    let mut x = 0
    match n {
        1 => println!("one"),
        2 => println!("two"),
        _ => println!("other"),
    }
    match n {
        1 => println!("skip-me"),
        2 => {
            println!("in-block")
            hits = 5
        }
        _ => println!("skip-other"),
    }
    let a = match n {
        2 => {
            println!("let-side")
            7
        }
        _ => 0,
    }
    x = match n {
        2 => {
            println!("assign-side")
            9
        }
        _ => 0,
    }
    return 0
}
"#,
    );
    let m = mir(&mirs, "main");

    // 正向证据：每个臂里的字面量都作为字符串节点进了 MIR（宏未展开时整棵子树被跳过）。
    let literals: Vec<&str> = m
        .exprs
        .values()
        .filter_map(|e| match e {
            MirExpr::StringLit(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let missing: Vec<&str> = [
        "one",
        "two",
        "other",
        "skip-me",
        "in-block",
        "skip-other",
        "let-side",
        "assign-side",
    ]
    .iter()
    .copied()
    .filter(|lit| !literals.contains(lit))
    .collect();
    assert!(
        missing.is_empty(),
        "四种形态的臂宏都应展开并下发（改前一个都不在 MIR 里），缺: {missing:?}",
    );

    let calls = call_symbols(m);
    let prints = calls
        .iter()
        .filter(|c| c.starts_with("print_str") || c.starts_with("println_str"))
        .count();
    assert_eq!(
        prints, 16,
        "8 个 `println!` 出现点 × 每点 2 个展开节点 = 16 次打印（改前 0 次），全部调用: {calls:?}"
    );
}

/// 批次 654（`9ff68eec`，站点 `src/middle/mir/gen.rs` 的 `str % value` 分支）：
/// 左操作数是 Str 的 `%` 要发 `zeta_str_percent_fmt` 并把目的槽标成 Str，
/// 不能落进通用 `"%"` 调用。
/// 症状（批次 654 记录）：`op == "%"` 落入通用 `MirStmt::Call { func: "%" }`，被 codegen 的
/// `is_operator` 捕获后走 `build_floormod_int`——对字符串指针和值句柄做整数取模
/// ⇒ `"f=%s" % d["name"]` 编译成功、退出码 0，打出句柄整数 13 而不是 `f=abc`。
/// 形态说明（批次 10013 实测）：记录里 t401 的原始写法是函数形参接模板，本套进程内管线
/// 只跑到降形、不跑 `refine_param_types`，形参停在 `PyDynamic` 就让不了这一臂的 Str 条件
/// （那种写法在本 harness 里实得 `["zeta_raise", "%"]`）。这里改用顶层变量持字符串模板，
/// 左操作数由类型推断直接得到 Str；`--dump-mir` 走 CLI 时两种写法都发 `zeta_str_percent_fmt`。
/// 另外模板直接写成字面量时，展开阶段会先按说明符切段改走 `py_fmt_*`，也打不到这一分支。
#[test]
fn str_percent_value_routes_to_percent_fmt_not_integer_modulo() {
    let mirs = lower_all(
        r#"tmpl = "a=%s"
v = 42
r = tmpl % v
print(r)
"#,
    );
    let f = mir(&mirs, "main");
    let calls = call_symbols(f);
    let (dest, argc) = f
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Call {
                func,
                dest,
                args,
                ..
            } if func == "zeta_str_percent_fmt" => Some((*dest, args.len())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("`Str % 动态值` 应发 `zeta_str_percent_fmt`，实得调用: {calls:?}"));
    assert_eq!(argc, 2, "格式化调用应带（模板, 值）两个实参，实得 {argc}");
    assert!(
        !calls.iter().any(|c| c == "%"),
        "改前症状是落进通用 `%`（后端按整数取模处理指针×句柄），不应出现，实得调用: {calls:?}"
    );
    assert_eq!(
        f.type_map.get(&dest),
        Some(&Type::Str),
        "格式化结果槽应标成 Str（槽 {dest} 实得 {:?}）",
        f.type_map.get(&dest)
    );
}

/// 批次 376（`src/frontend/macro_expand.rs` 的 `expand_println` 无格式串那条叉）：
/// `println!(x)` 里打印器的名字不许按 x 的**语法形状**在展开期写死。
/// 症状（批次 376 记录）：单参数只要是 变量／整数字面量／调用结果 三种形状之一，展开器就
/// 直接发 `println_i64`（整数打印器），而展开期没有类型信息 ⇒ 字符串变量打出堆地址
/// （记录里 `4331907520`）、浮点变量被当整数截掉（`let g = 1.75` 打 `1`——不打地址、
/// 看着像一个合理整数）。名字写死那一笔按 `git log -S` 追到 v0.7.0 的 `13697ac8`，
/// 外面套的"按形状选"那条叉是 `139a7454`（改前文件 blame 实测，两笔隔 41 行、同函数不同段——
/// 376 记录把那条 "For now, simple expansion to a function call" 注释称作"同段"，本行按 blame 更正）。
/// 期望值来源：记录定位那一节说"带类型信息的分派本来就在 `gen.rs` 的单参 `println` 臂"
/// （按 `type_map` 选 `println_str`／`println_f64`／`println`），整数那一档由后端的
/// `println → println_i64` 映射接住，所以正确形态是 字符串→`println_str`、
/// 整数→保留 `println`，而 `println_i64` 这个名字不该由展开期发出来。
#[test]
fn println_without_format_string_dispatches_by_type_not_by_syntax_shape() {
    let mirs = lower_all(
        r#"def sv():
    return "from-call"

s = "hi"
n = 42
println!(s)
println!(n)
println!(sv())
"#,
    );
    let f = mir(&mirs, "main");
    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        2,
        "字符串变量与返回 str 的调用结果都该按类型拿到 `println_str`（改前两者都被展开期写成整数打印器），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝展开期把 变量／字面量／调用结果 三种形状统一写成 `println_i64`，这个名字不许出现在降形结果里，实得调用: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c == "println"),
        "整数变量该保留 `println` 交给后端按 `println → println_i64` 映射接住（记录里整数一档改前改后读数不变），实得调用: {calls:?}"
    );
}

/// 批次 377（同一函数的格式串那条叉）：`println!("A={}B", v)` 要在展开期把格式串按 `{}`
/// 切成"字面量段 + 值段"逐段发语句，最后补一条换行。
/// 症状（批次 377 记录，改前读数在批次 376 的构建上实拍）：改前 :130-135 把格式串
/// **整个剥掉**、只把值参传下去 ⇒ 字面量段必丢（`println!("A={}", a)` 只打 `1`）；
/// 多值合并成一次 `println` 调用，而 `println` 在 MIR 里只有单参分派、后端又映射到只吃
/// 一个参数的 `println_i64` ⇒ 第二个以后的值参被丢掉（`println!("C={}D={}", a, b)` 只打 `1`）。
/// 期望值来源：记录的修法那一节列出的节点序列——字面量段→`print_str(段)`、
/// 值段→`print(值, end="")`（按静态类型派发）、收尾 `print_str("\n")`。
/// 断言只钉"段有没有下发"与"每个值各发一次按型派发"，不钉打印调用总条数——
/// 值段那条 `end=""` 实参会另外落成一次空串 `print_str`（`gen.rs` 的 `has_end` 路径），
/// 那是实现细节、不是这一格的症状，总条数会随它变。
/// 第二站两个占位符实参按"字符串在前、浮点在后"排：浮点站第二个位置，
/// 改前"多占位符只发第一个值"那一支才会被这条用例抓到（变异 M3 实测）。
#[test]
fn println_format_string_splits_into_literal_and_value_segments() {
    let mirs = lower_all(
        r#"n = 7
g = 1.75
m = "ab"
println!("A={}B", n)
println!("C={}D={}", m, g)
"#,
    );
    let f = mir(&mirs, "main");

    // 正向证据①：五个字面量段（含收尾换行）都在 MIR 里（改前格式串被整个剥掉，一个都不在）。
    let literals: Vec<&str> = f
        .exprs
        .values()
        .filter_map(|e| match e {
            MirExpr::StringLit(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    let missing: Vec<&str> = ["A=", "B", "C=", "D=", "\n"]
        .iter()
        .copied()
        .filter(|lit| !literals.contains(lit))
        .collect();
    assert!(
        missing.is_empty(),
        "格式串的字面量段要逐段下发（改前整条被剥掉），缺: {missing:?}",
    );

    // 正向证据②：每个值段各自按静态类型派发。浮点值故意放在第二站的**第二个**占位符——
    // 改前多占位符只发第一个值，它排在前面时这条断言打不到。
    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "print_i64").count(),
        1,
        "第一站的整数值该发一次 `print_i64`（改前整站合并成一次 `println_i64`），实得调用: {calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "print_f64").count(),
        1,
        "第二站的浮点值也要各发一次——改前多占位符只打第一个值，这一条会丢，实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("println")),
        "带格式串的 `println!` 改后不再合并成一次 `println`（那样第二个值会丢），实得调用: {calls:?}"
    );
}

/// 批次 407（站点 `src/middle/resolver/resolver.rs` 的 `collect_calls`）：调用点证据的
/// 收集要下钻 `FString`，否则"唯一调用点写在插值里"的函数拿不到参数类型。
/// 症状（批次 407 记录）：`collect_calls` 的 match 原本有九条下钻臂，没有 `FString` 那条
/// ⇒ 插值里的调用整个不见 ⇒ 未注解形参停在 `"dyn"` → `unannotated_return_ty` 拿到
/// `PyDynamic` → 调用结果槽被 `.unwrap_or(Type::I64)` 定成 I64 → `lower_to_string` 发
/// `to_string_i64` ⇒ 堆指针被当十进制打印（记录 p1 实拍 `p=4367215168`，真值 `p=direct`），
/// 同一批 `t443` 的第四条期望里地址还会被喂进 `len` 参与算术（改前 `20`、改后 `4`）。
/// 期望值来源：记录的改前正证据是逐槽写明的——`rp` 体内 `type_map: 1: PyDynamic`、
/// `main` 里作用于调用结果的是 `to_string_i64`；同文件另外三处（`:1794` 与 `infer` 两支）
/// 都把 `FString` 当叶子直接答 `Str`，所以这条臂只是把不一致补齐。
/// 边界（不在本条覆盖内）：记录 §七 实拍嵌套 `def` 被提升成 `__closure_0_*` 名后仍拿不到
/// 证据（证据消费端只认三种名字解释），那一格本批未修。
#[test]
fn sole_callsite_inside_fstring_still_gives_param_type_evidence() {
    let mirs = lower_all(
        r#"def rp(s):
    return s

print(f"p={rp('direct')}")
"#,
    );
    let rp = mir(&mirs, "rp");
    let (param_name, param_slot) = &rp.param_indices[0];
    assert_eq!(param_name, "s", "读的是哪个形参要跟实现在册一致，实得 {param_name}");
    assert_eq!(
        rp.type_map.get(param_slot),
        Some(&Type::Str),
        "唯一调用点在插值里时，实参 `'direct'` 也该给形参提供 Str 证据（改前停在 PyDynamic），实得 {:?}",
        rp.type_map.get(param_slot)
    );

    let main = mir(&mirs, "main");
    let calls = call_symbols(main);
    // 调用点本身要还在（否则这条用例什么都没测到）；注册名去重后会带 `_1` 这类后缀，
    // 名字不是这一格的症状，所以只按前缀认。
    assert!(
        calls.iter().any(|c| c.starts_with("rp")),
        "调用点本身要还在（否则这条用例什么都没测到），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "to_string_i64"),
        "改前症状是把调用结果按整数转字符串（打出堆地址），`to_string_i64` 不应出现，实得调用: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c == "println_str"),
        "结果槽带上 Str 后打印该走 `println_str`，实得调用: {calls:?}"
    );
}

/// 常量比较的打印结果要按布尔走，不按整数 0/1 走（症状记录＝批次 327 任务 #45）。
/// 症状（批次 327 记录"一条 MIR 就够的证据"那一节实拍）：`print(3 == 4)` 的 MIR 是
/// `VoidCall{ "println_i64", [2] }` ＋ `exprs 2: IntLit(0)` ＋ `type_map 2: I64`，
/// 整条 MIR 里没有 BinaryOp ⇒ 常量折叠在 `gen.rs` 的下型与 print 分发**之前**就把比较吃成了
/// 整数 0/1，打印选到整数那一档（真值是 `False`，CPython 同输入打 `False`／`True`）。
/// 期望值来源：批次 327 记录（比较的结果必须是布尔，不是 0/1）＋ 批次 642 记录里
/// `(值, is_bool)` 那一臂的约定（顶层是布尔时渲染 `True`／`False`，不是十进制）。
///
/// 站点归属（批次 10015 实测更正，两条变异都留档）：本树里 `print(常量比较)` 这一形走的
/// 是 `src/middle/ctfe/evaluator.rs` 的 print 实参改写臂（`let rendered = if is_bool {…}`，
/// 现 :303-308，由批次 642 `adc0ffba` 引入），**不是** 327 新写的 `compare_int`——
/// 把 `compare_int` 的结果改回 `ConstValue::Int(cmp as i64)`（＝327 的改前状态）本用例读数
/// 一字不变（变异 M1 不红），在 `compare_int` 里加打印探针跑本用例，探针一次都不出。
/// 打这一臂（把 `True`／`False` 换成 `v.to_string()`）本用例立刻红（变异 M1b）。
/// `compare_int` 本身仍是活的，只是活在 const／comptime 折叠那条路上，那一形的用例另批补。
/// 边界（本条不覆盖）：批次 327 记录 §副作用面 里 `ConstValue::as_int()` 对 Bool 返回 None
/// 的三个消费者（求下标／重复计数／切分），记录自己写明"没为这条写用例"。
#[test]
fn constant_folded_comparison_yields_bool_not_int_zero() {
    let mirs = lower_all(
        r#"print(3 == 4)
print(2 == 2)
"#,
    );
    let f = mir(&mirs, "main");

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        2,
        "两条常量比较都该按布尔打成字符串（改前按整数打 0/1），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝折叠层把比较结果确定成整数类型，打印选到 `println_i64`，这个名字不许出现，实得调用: {calls:?}"
    );

    // 正向证据：下发的字面量是布尔的两个拼写，不是 0 和 1。
    let literals: Vec<&str> = f
        .exprs
        .values()
        .filter_map(|e| match e {
            MirExpr::StringLit(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    for want in ["False", "True"] {
        assert!(
            literals.contains(&want),
            "比较结果要以布尔字面量下发（真值 {want}），实得字符串字面量: {literals:?}"
        );
    }

    // 直接查"喂给打印器的那一格"：改前是 IntLit(0)／IntLit(1)，改后是两条布尔拼写。
    // 不查全部整数字面量——收尾那条无值语句本来也带一个 IntLit(0)，查它会误判。
    let printed: Vec<String> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::VoidCall { func, args } if func.starts_with("println") => {
                args.first().and_then(|a| match f.exprs.get(a) {
                    Some(MirExpr::StringLit(v)) => Some(v.clone()),
                    Some(other) => Some(format!("{other:?}")),
                    None => Some("<缺槽>".to_string()),
                })
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        printed,
        vec!["False".to_string(), "True".to_string()],
        "两条常量比较下发给打印器的实参要是布尔字面量（改前是 IntLit(0) 与 IntLit(1)）"
    );
}

/// 批次 447（2.2 名字绑定／#182 B 堆，站点 `src/middle/resolver/resolver.rs` 的
/// `impl_key_base` 与六处登记键）：泛型 `impl<K, V> Box2<K, V>` 的方法登记进名表时，
/// 键带着 impl 头部的**泛型原文**（`Box2<K, V>::new`），而调用点写的是裸类别名。
/// 症状（批次 447 记录）分两条后果：方法名是 `new` 时名表路由不命中、兜底臂又排除
/// `new` ⇒ 整条臂走完既不 `stmts.push` 也不 `exprs.insert` ⇒ 调用语句**被丢掉**
/// （夹具实拍 rc=133、stdout 空 ＋ 一条 W1010）；方法名是别的时落进"首字母大写＝句柄"
/// 的兜底，被改写成 `zeta_platform_obj(...)` ⇒ 一声不出地返回堆地址。
/// 期望值来源：记录的三条独立证据之①——定义侧把尖括号原样发进 LLVM 符号名
/// （`@"C2<K, V>::make"`），修法是把登记键截到第一个 `<` 之前 ⇒ 定义与调用两侧同串。
/// 非泛型的 `impl Plain` 是记录里的对照组（改前就绿），本条把它一起放进来当**前置条件**：
/// 泛型是这一格里唯一的变量。
#[test]
fn generic_impl_method_key_drops_angle_brackets_at_callsite() {
    let mirs = lower_all(
        r#"struct Inner { q: i64 }

pub struct Box2<K, V> {
    k: K,
    v: V,
    inner: Inner,
}

impl<K, V> Box2<K, V> {
    pub fn new() -> Self {
        Box2 { k: 5, v: 6, inner: Inner { q: 7 } }
    }

    pub fn tagged() -> i64 {
        3
    }
}

struct Plain { p: i64 }

impl Plain {
    pub fn new() -> Self {
        Plain { p: 13 }
    }
}

fn main() {
    let n = Plain::new()
    let b = Box2::new()
    let t = Box2::tagged()
    print(t)
}
"#,
    );

    // 前置条件（对照组）：非泛型的 impl 键一直是绑上的，所以"泛型"是唯一的变量。
    let plain = mir(&mirs, "Plain::new");
    assert!(
        !plain.stmts.is_empty(),
        "对照组：非泛型 `impl Plain` 的 `new` 要能降出体内语句，实得 {:?}",
        plain.stmts.len()
    );

    // 泛型方法的定义名不再带尖括号（改前登记键＝`Box2<K, V>::new`，
    // 记录证据①：那个键被原样发进 LLVM 符号名）。
    let names: Vec<&str> = mirs.iter().filter_map(|m| m.name.as_deref()).collect();
    assert!(
        names.contains(&"Box2::new"),
        "泛型 impl 的构造子该以裸类别名进名表（改前带尖括号 ⇒ 调用点永不命中），实得函数名: {names:?}"
    );
    assert!(
        names.contains(&"Box2::tagged"),
        "泛型 impl 的非构造子方法同理，实得函数名: {names:?}"
    );
    let bracketed: Vec<&str> = names.iter().copied().filter(|n| n.contains('<')).collect();
    assert!(
        bracketed.is_empty(),
        "降形结果里不许出现带泛型参数的名字（那是改前的登记键），实得: {bracketed:?}"
    );

    // 后果①：调用语句要真的下发（改前整条臂没有出口 ⇒ 既不 push 也不 insert）。
    let f = mir(&mirs, "main");
    let calls = call_symbols(f);
    for want in ["Box2::new", "Box2::tagged", "Plain::new"] {
        assert!(
            calls.iter().any(|c| c == want),
            "调用点 `{want}` 要出现在降形结果里（改前这一条被整条臂丢掉），实得调用: {calls:?}"
        );
    }
}

/// 批次 451（3.2 Lowering／返回标记，站点 `src/middle/resolver/resolver.rs` 的
/// `unannotated_return_ty`——本树现 :4581，识别条件在 :4950；记录写作内层 :3120-3140 ＋
/// 外层 :3337-3346）：
/// py `class` 脱糖出来的**无注解方法**，返回标记被解析层硬写成 `"i64"` ⇒
/// 中间层"按体恢复返回类型"的两道守卫（原本只认空注解）永远进不去 ⇒
/// 调用点目的槽被 `.unwrap_or(Type::I64)` 定成整数 ⇒ 运行期按整数打印一个 str 指针。
/// 症状（批次 451 记录根因链④ ＋ 靶夹具实拍）：改前 `4310769584`、改后 `a,b,`；
/// 同一段里 `print(len(b.parts))` 两侧都是 `2` ⇒ 值本身是好 str，坏的只有调用点那一格标记。
/// 期望值来源：记录的修法那一节——识别依据是接收者参数的类型拼写（脱糖写类别名、
/// 手写 `impl` 写 `"Self"`），命中"脱糖默认"就允许按体恢复；`out = "a"; return out`
/// 这种体在 `unannotated_return_ty` 的本地作用域表里判得出 Str。
/// 正对照：记录 §三 表明的"顶层 `def` 同形改前就是对的"，本条把它一起锁住防回潮。
/// 边界（本条不覆盖）：记录 §三 的 `@classmethod` 那一格已由批次 552（`dd6e81d7`）接走
/// （#195 前半已闭），本条不重复它；"写了注解而体不符"仍归 #33，记录有意不越权。
#[test]
fn py_class_method_str_return_marks_callsite_dest_as_str() {
    let mirs = lower_all(
        r#"class B:
    def r(self):
        out = "a"
        return out

def plain():
    out = "b"
    return out

print(B().r())
print(plain())
"#,
    );

    // 方法体自己的恢复：返回槽（局部变量 `out` 那一跳）该是 Str，不是被 "i64" 顶掉。
    let r = mir(&mirs, "B::r");
    let ret_val = r
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Return { val } => Some(*val),
            _ => None,
        })
        .expect("方法有返回值");
    assert_eq!(
        r.type_map.get(&ret_val),
        Some(&Type::Str),
        "无注解 py 方法的返回表达式该按体恢复成 Str（改前登记键写作 i64，被当成显式注解），实得 {:?}",
        r.type_map.get(&ret_val)
    );

    let f = mir(&mirs, "main");
    // 调用点目的槽：本批那一格的症状就在这一格（记录根因链④）。
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func == "B::r" || func == "plain_0" || func == "plain" => {
                Some(*dest)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        2,
        "前置条件：两个调用点都要降出来（少一个这条用例就什么都没测到），实得 {dests:?}"
    );
    for d in &dests {
        assert_eq!(
            f.type_map.get(d),
            Some(&Type::Str),
            "调用点目的槽 id={d} 该是 Str（改前是 I64 ⇒ 运行期按整数打印 str 指针，打出堆地址），实得 {:?}",
            f.type_map.get(d)
        );
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        2,
        "两个调用点都该按 Str 选打印器，实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝目的槽是 I64 ⇒ 按整数打印（打出堆地址），`println_i64` 不应出现，实得调用: {calls:?}"
    );
}

/// 批次 552（旁路 cleanup 车道编号，代码 `dd6e81d7`；roadmap 无此节，记录在提交信息里；
/// 站点 `src/middle/resolver/resolver.rs` 的
/// `unannotated_return_ty` 里 `cls`/`dyn` 那一档，现 :4951-4962）／backlog 旧 #195 前半。
/// 症状（552 记录 ＋ 夹具 `tests/python_style/t522_classmethod_return.z`）：批次 451 那一档
/// 只认"首参是 self 且类型是类别名"，而 `@classmethod` 的脱糖把 `cls` 留成普通未注解参数
/// ⇒ 判据看不见接收者，返回标记落回 i64 兜底 ⇒ 调用点按整数打印一个 str 指针。
/// 期望值来源：t522 的 `// expect:` 五行（a / 42 / b / c / 1）——`make`／`stat`／`inst`
/// 返回字符串，`num` 返回整数。
/// 边界（本条不覆盖）：552 记录登记在 #195 余项的"参数直传 `return s` 仍打指针"
/// （要调用点类型流进返回推断＝批次 628 那一族，本文件另有一条）与"float 返回打
/// 1.500000"的既有 print 方言。
#[test]
fn classmethod_return_recovers_str_at_callsite() {
    let mirs = lower_all(
        r#"class C:
    @classmethod
    def make(cls):
        out = "a"
        return out
    @classmethod
    def num(cls):
        return 42
    @staticmethod
    def stat():
        return "b"
    def inst(self):
        out = "c"
        return out

print(C.make())
print(C.num())
print(C.stat())
o = C()
print(o.inst())
"#,
    );

    // @classmethod 体自己的恢复：返回槽（局部变量 `out` 那一跳）该是 Str。
    // 改前 `cls` 那一档不认，返回标记被当成显式注解的 i64，恢复整条不启动。
    let make = mir(&mirs, "C::make");
    let ret_val = make
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Return { val } => Some(*val),
            _ => None,
        })
        .expect("classmethod 有返回值");
    assert_eq!(
        make.type_map.get(&ret_val),
        Some(&Type::Str),
        "无注解 @classmethod 的返回表达式该按体恢复成 Str（改前 cls 不是接收者拼写，被当成显式注解），实得 {:?}",
        make.type_map.get(&ret_val)
    );

    let f = mir(&mirs, "main");
    let dests: Vec<(String, u32)> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. }
                if matches!(func.as_str(), "C::make" | "C::num" | "C::stat" | "C::inst") =>
            {
                Some((func.clone(), *dest))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        4,
        "前置条件：四个调用点（classmethod 两形＋staticmethod＋实例方法）都要降出来，少一个这条用例就什么都没测到，实得 {dests:?}"
    );
    for (func, d) in &dests {
        // 期望值取自 t522 的 expect 行：make/stat/inst 是字符串，num 是整数 42。
        let want = if func.as_str() == "C::num" { Type::I64 } else { Type::Str };
        assert_eq!(
            f.type_map.get(d),
            Some(&want),
            "{func} 的调用点目的槽该是 {want:?}（改前 @classmethod 两格落 I64 ⇒ 运行期按整数打印 str 指针，打出堆地址），实得 {:?}",
            f.type_map.get(d)
        );
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        3,
        "三条返回字符串的调用点该选字符串打印器（改前 make/stat/inst 三格都被当成整数），实得调用: {calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_i64").count(),
        1,
        "只有 C::num 那一格按整数打印（真值 42），实得调用: {calls:?}"
    );
}

/// 批次 628（旁路 cleanup 车道编号，代码 `3200fc16`；记录在本树 `roadmap.md:24570`
/// 的并收录行内，无独立小节；
/// 站点 `src/middle/resolver/resolver.rs` 的
/// `refine_method_return_types` ＋ `collect_return_kinds` 的 `refinable` 参数转发臂；
/// 这趟由 `typecheck.rs:33` 接线）／backlog 旧 #195 余项。
/// 症状（628 记录 ＋ 夹具 `tests/python_style/t538_return_type_infer.z`）：
/// `def ident(self, w): return w` 的形参 `w` 在体内已被批次 627 精化成 Str，但注册到
/// `funcs` 的返回类型还停在 I64 ⇒ 调用点按 I64 选打印器，把串句柄地址打出来
/// （s35／s36 实拍）。
/// 期望值来源：t538 的 `// expect:` 三行（X / Hey! / hi moe）＝三个返回都是字符串。
/// 边界（本条不覆盖）：628 记录在册的保守面——容器返回（list/dict）不推断；
/// 混合型别投票与不可推断（PyDynamic）投毒弃权。
#[test]
fn method_return_inference_from_param_and_concat_marks_callsites_str() {
    let mirs = lower_all(
        r#"class Bag:
    def ident(self, w):
        return w
    def bang(self, word):
        return word + "!"
    def greet(self, name):
        return "hi " + name

b = Bag()
print(b.ident("X"))
print(b.bang("Hey"))
print(b.greet("moe"))
"#,
    );

    let f = mir(&mirs, "main");
    let dests: Vec<(String, u32)> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. }
                if matches!(func.as_str(), "Bag::ident" | "Bag::bang" | "Bag::greet") =>
            {
                Some((func.clone(), *dest))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        3,
        "前置条件：三个调用点都要降出来（少一个这条用例就什么都没测到），实得 {dests:?}"
    );
    for (func, d) in &dests {
        assert_eq!(
            f.type_map.get(d),
            Some(&Type::Str),
            "{func} 的调用点目的槽该是 Str（改前注册的返回类型停 I64 ⇒ 运行期按整数打串句柄地址），实得 {:?}",
            f.type_map.get(d)
        );
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        3,
        "三个调用点都该按 Str 选打印器（参数直传、拼接两侧、拼接左字面量三种证据各一条），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝返回类型停 I64 ⇒ 按整数打印串句柄，`println_i64` 不应出现，实得调用: {calls:?}"
    );
}

/// 批次 644（旁路 cleanup 车道编号，代码 `23a32829`；记录在该笔的提交信息里。
/// **批次编号与主树重合**：本树 `roadmap.md:25177` 的"批次 644"是另一批（主线度量批／#251），
/// 引用以哈希为身份；站点 `src/middle/ctfe/evaluator.rs` 的
/// `p642_collect_pattern_vars` 与 `p642_collect_assigned` 的 `For` 臂）／夹具
/// `tests/python_style/t518_module_global_env_first.z` 的 ⑤ 段（`pairs` 那一段；该夹具头注只编到 ⑤）。
/// 症状（644 记录）：批次 642 的"循环／分支内被赋过名的量不再折叠"收集器只认 `Var`
/// 模式，`for k2, v2 in pairs:` 的 Tuple 模式没被杀 ⇒ `k2`／`v2` 停在常量表里 ⇒
/// `print(k2)` 折成循环前的陈值 0、`print(v2)` 折成空串（expect 2／b 实拍）。
/// 期望值来源：t518 的 `// expect:` 六行（1 / a / 2 / b / 2 / b）——循环内两条与循环外
/// 两条都要读运行期的槽，`k2` 是整数、`v2` 是字符串。
/// 边界（本条不覆盖）：644 记录同时作废的"槽移位假说"（那是归因更正，不是行为面）。
#[test]
fn tuple_for_loop_pattern_kills_stale_consts_before_print_folds() {
    let mirs = lower_all(
        r#"pairs = [(1, "a"), (2, "b")]
k2 = 0
v2 = ""
for k2, v2 in pairs:
    print(k2)
    print(v2)
print(k2)
print(v2)
"#,
    );

    let f = mir(&mirs, "main");
    // 循环体内那两条 print 在 While 的 body 里，得跟着块走（只扫顶层会漏掉一半）。
    let mut printed: Vec<(String, u32)> = Vec::new();
    {
        fn walk(stmts: &[MirStmt], out: &mut Vec<(String, u32)>) {
            for s in stmts {
                match s {
                    MirStmt::VoidCall { func, args }
                        if matches!(func.as_str(), "println_i64" | "println_str") =>
                    {
                        out.push((func.clone(), args[0]));
                    }
                    MirStmt::While { body, else_body, .. } => {
                        walk(body, out);
                        walk(else_body, out);
                    }
                    MirStmt::If { then, else_, .. } => {
                        walk(then, out);
                        walk(else_, out);
                    }
                    _ => {}
                }
            }
        }
        walk(&f.stmts, &mut printed);
    }
    assert_eq!(
        printed.len(),
        4,
        "前置条件：循环内两条＋循环外两条 print 都要降出来，实得 {printed:?}",
    );

    // 症状那一格：被陈值折叠的 print 实参是字面量（改前四格全成 IntLit(0)／StringLit 空串），
    // 修法之后必须读运行期的槽。
    for (func, a) in &printed {
        let direct = f.exprs.get(a);
        assert!(
            !matches!(direct, Some(MirExpr::IntLit(_)) | Some(MirExpr::StringLit(_))),
            "{func} 的实参 id={a} 不该是折叠后的字面量（改前＝循环前陈值 0 与空串），实得 {direct:?}"
        );
        // 实参槽常是 `Var(id)` 一跳（本文件 `int_lit` 那条口径），字面量可能藏在赋右侧。
        let hopped = find_assign_rhs(&f.stmts, *a).and_then(|rhs| f.exprs.get(&rhs));
        if let Some(e) = hopped {
            assert!(
                !matches!(e, MirExpr::IntLit(_) | MirExpr::StringLit(_)),
                "{func} 的实参 id={a} 往回一跳也不该是字面量（改前陈值就藏在赋右侧），实得 {e:?}"
            );
        }
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_i64").count(),
        2,
        "两条 `print(k2)` 该按整数走（真值 1 与 2），实得调用: {calls:?}",
    );
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        2,
        "两条 `print(v2)` 该按字符串走（真值 a 与 b；改前四格都被折成陈字面量 ⇒ 全走字符串打印器），实得调用: {calls:?}",
    );
}

/// 批次 629（旁路 cleanup 车道编号，代码 `449471d4`，台账行 `worktree.md:337`；
/// 站点 `src/middle/resolver/resolver.rs` 的 `refine_field_element_types`，
/// 由 `src/middle/resolver/typecheck.rs` 在返回推断之后接线）／旧 #195 余项一族。
/// 症状（629 记录 ＋ 夹具 `tests/python_style/t539_field_element_refine.z`）：
/// `self.items = []` 是空字面量、没有元素证据，注册时按批次 594 的拼写默认落成 `list<i64>`
/// ⇒ `bg.items[0]` 的结果槽是 I64 ⇒ 运行期按整数读一个字符串句柄（打出地址）。
/// 修法是**投票**：从方法体里的 `self.<f>.append(<e>)` 站点取元素型别（`e` 是字面量或
/// 批次 627 已精化的参数），全体一致时把拼写从 `list<i64>` 改写 `list<str>`（只从默认改）。
/// 期望值来源：t539 的 `// expect:` 三行（2 / a / b）——`size()` 是整数，两次下标读是字符串。
/// 边界（本条不覆盖）：629 记录在册的保守面——混合元素型别投票弃权、显式注解不覆盖；
/// 以及那条链上的两个实现坑（裸 Call 语句站点、`ret_expr` 提升）本身不在这里复验。
#[test]
fn list_field_append_votes_str_element_at_index_callsite() {
    let mirs = lower_all(
        r#"class Bag:
    def __init__(self):
        self.items = []
    def add(self, x):
        self.items.append(x)
    def size(self):
        return len(self.items)

bg = Bag()
bg.add("a")
bg.add("b")
print(bg.size())
print(bg.items[0])
print(bg.items[1])
"#,
    );

    let f = mir(&mirs, "main");
    // (接收者槽, 结果槽)：两个下标读取。
    let gets: Vec<(u32, u32)> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, args, dest, .. } if func == "array_get" => Some((args[0], *dest)),
            _ => None,
        })
        .collect();
    assert_eq!(
        gets.len(),
        2,
        "前置条件：两次 `bg.items[i]` 都要降出来（少一个这条用例就什么都没测到），实得 {gets:?}"
    );
    for (recv, dest) in &gets {
        assert_eq!(
            f.type_map.get(recv),
            Some(&Type::DynamicArray(Box::new(Type::Str))),
            "字段槽 id={recv} 该被投票改写成 `list<str>`（改前停在默认的 `list<i64>` ⇒ 元素按整数读），实得 {:?}",
            f.type_map.get(recv)
        );
        assert_eq!(
            f.type_map.get(dest),
            Some(&Type::Str),
            "下标结果槽 id={dest} 该是 Str（改前随字段型别落 I64 ⇒ 运行期打串句柄地址），实得 {:?}",
            f.type_map.get(dest)
        );
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        2,
        "两次下标读都该按 Str 选打印器（真值 a 与 b），实得调用: {calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_i64").count(),
        1,
        "只有 `print(bg.size())` 那一格按整数打印（真值 2），实得调用: {calls:?}"
    );
}

/// 批次 630（旁路 cleanup 车道编号，代码 `01351a31`，台账行 `worktree.md:338`；
/// 站点 `src/middle/resolver/resolver.rs` 的 `refine_map_value_types` ＋
/// `refine_method_return_types` 里的 `self.<f>.get(k, <字面量>)` 那一面，
/// 接线顺序＝map 投票先于返回推断）／旧 #195 余项一族。
/// 症状（630 记录 ＋ 夹具 `tests/python_style/t540_map_value_refine_get.z`）：
/// `self.d = {}` 在批次 594 的拼写里是裸 `map`（没有值型别位）⇒ `fetch` 注册的返回类型
/// 停在 I64，调用方按整数选打印器，把串句柄地址打出来（记录实拍 s38 连纯字符串字典都败）。
/// 修法＝从 `self.<f>[<k>] = <v>` 站点投票出 `(键, 值)` 型别，一致时把裸 `map` 改写
/// `map<K, V>`；返回推断再读这个型别。默认字面量只在与投票值一致时才算证据，
/// 不一致（int 值＋str 默认＝真 union）投毒弃权。
/// 期望值来源：t540 的 `// expect:` 四行（x / missing / y / 1）——三个返回都是字符串，
/// `len(...)` 那一格才是整数。
/// 变异读数如实记：撤掉 `refine_map_value_types` 的接线后，目的槽回的是
/// `Named("PyJson")` 而不是记录实拍的 I64——因为批次 646/660 在这支后面又补了
/// "值型别未知＝真 union"的 PyJson／PyDynamic 支。用例照样红（不是 Str），
/// 但"红值＝记录症状值"只在 629/631 两条成立。
/// 边界（本条不覆盖）：union 跨函数边界＝记录留队的旧 #117 那格；带注解的 `map<K,V>` 不覆盖。
#[test]
fn map_field_value_votes_str_and_get_default_marks_callsites_str() {
    let mirs = lower_all(
        r#"class Cfg:
    def __init__(self):
        self.d = {}
    def put(self, k, v):
        self.d[k] = v
    def fetch(self, k):
        return self.d.get(k, "missing")

c = Cfg()
c.put("a", "x")
c.put("b", "y")
print(c.fetch("a"))
print(c.fetch("z"))
print(c.fetch("b"))
print(len(c.fetch("a")))
"#,
    );

    let f = mir(&mirs, "main");
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func == "Cfg::fetch" => Some(*dest),
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        4,
        "前置条件：四个 `c.fetch(...)` 调用点都要降出来（含 `len` 里那一个），实得 {dests:?}"
    );
    for d in &dests {
        assert_eq!(
            f.type_map.get(d),
            Some(&Type::Str),
            "调用点目的槽 id={d} 该是 Str（改前值型别无证据 ⇒ 注册返回停 I64 ⇒ 按整数打串句柄地址），实得 {:?}",
            f.type_map.get(d)
        );
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        3,
        "三条 `print(c.fetch(..))` 该按 Str 选打印器（真值 x / missing / y），实得调用: {calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_i64").count(),
        1,
        "只有 `print(len(c.fetch(\"a\")))` 那一格按整数打印（真值 1），实得调用: {calls:?}"
    );
}

/// 批次 631（旁路 cleanup 车道编号，代码 `15232a15`，台账行 `worktree.md:339`；
/// 站点 `src/middle/resolver/resolver.rs` 的 `refine_method_return_types` 两个调用面：
/// `return self.<m>(·)` 读 `funcs["C::m"].ret`、`return <plain>(·)` 读裸名键）／旧 #195 余项一族。
/// 症状（631 记录 ＋ 夹具 `tests/python_style/t541_return_chain_infer.z`）：
/// `def outer(self): return self.inner()` 里 `inner` 的返回已被批次 628 精化成 Str，
/// 但推断器不认 `Call` 形 ⇒ `outer` 注册返回停在 I64，调用方按整数打印串句柄地址
/// （记录实拍 s42）。修法＝扩两个调用面＋不动点迭代（≤4 轮，每轮重建 `funcs.ret` 快照），
/// callee 是 I64 或未知时投毒弃权。
/// 期望值来源：t541 的 `// expect:` 两行（deep / x）——委托方法与普通函数链各一条。
/// 覆盖面分工（变异实测）：这一条真正覆盖的是**委托面**（撤掉 `return self.<m>(·)`
/// 读 `funcs["C::m"].ret` 那一支，目的槽立刻回 `I64`＝记录症状）；`print(f())`
/// 那一格撤掉 `return <plain>(·)` 读裸名键后读数**不变**（24/24 照旧）。该支确系 631 所加
/// （`git log -S` 实跑到 `15232a15`），撤了不红说明这一形另有更早的推断在起作用——631 记录自陈
/// "普通函数链 s41 本就通（451/601 基础设施）"与此吻合。因此那一格只算回退保护（防止以后
/// 这一形被别的改动放松掉），不算 631 的证据。
/// 边界（本条不覆盖）：631 记录留队的 `str(dict)` 那一格（map 槽序非插入序）；
/// 容器返回链不推断＝628 的保守面。
#[test]
fn method_return_chain_delegation_marks_callsites_str() {
    let mirs = lower_all(
        r#"class C:
    def inner(self):
        return "deep"
    def outer(self):
        return self.inner()

def g():
    return "x"
def f():
    return g()

c = C()
print(c.outer())
print(f())
"#,
    );

    let f = mir(&mirs, "main");
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. }
                if func == "C::outer" || func == "f" || func == "f_0" =>
            {
                Some(*dest)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        2,
        "前置条件：委托方法那一格与普通函数链那一格都要降出来，实得 {dests:?}"
    );
    for d in &dests {
        assert_eq!(
            f.type_map.get(d),
            Some(&Type::Str),
            "调用点目的槽 id={d} 该是 Str（改前推断器不认 Call 形 ⇒ 注册返回停 I64 ⇒ 按整数打串句柄地址），实得 {:?}",
            f.type_map.get(d)
        );
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        2,
        "两个调用点都该按 Str 选打印器（真值 deep 与 x），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝委托链的返回停 I64 ⇒ 按整数打印，`println_i64` 不应出现，实得调用: {calls:?}",
    );
}

/// 批次 579（旁路 cleanup 车道编号，代码 `4ee26e8a`，台账行 `worktree.md:288`；
/// 站点 `src/middle/resolver/resolver.rs` 的 `unannotated_return_ty`（现 :4581）里那层嵌套
/// `infer`（现 :4635）的 `__contains__` 臂，现 :4716）／旧 ⑫ 注册期传播第一批。
/// 症状（579 记录 ＋ 差分夹具 `tests/diff/cases/class_tag_probe.dcase`）：
/// `t in self.tags` 在解析层脱糖成 `self.tags.__contains__(t)`，而方法返回推断的 Call 臂
/// 不认这个名字 ⇒ `has_tag` 的返回推不出来、停在 i64 ⇒ 调用点 `print` 打 `1` 而不是 `True`。
/// 修法＝补"`__contains__` 恒 Bool"臂（membership 在 Python 里恒为 Bool）。
/// 期望值来源：同一份源在 CPython 下的实拍真值 `True`／`False`（本批用 `python3` 现跑取），
/// 对应到编译期＝两个调用点目的槽都该是 `Bool`、打印走 `print_bool`。
/// 边界（本条不覆盖）：579 记录里同批留下的余项（`tag_str` 的 join 推断＝下面 580 那条、
/// `%(key)s=dict` 的 ⑭ 子形）；运行期打印文案（`True` 还是 `true`）也不在这里，只看类型标记与派发。
#[test]
fn membership_method_return_gives_bool_not_int_at_callsite() {
    let mirs = lower_all(
        r#"class Tagged:
    def __init__(self, name, tags):
        self.name = name
        self.tags = tags
    def has_tag(self, t):
        return t in self.tags

t = Tagged("srv", ["web", "db"])
print(t.has_tag("web"))
print(t.has_tag("cache"))
"#,
    );

    let f = mir(&mirs, "main");
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func == "Tagged::has_tag" => Some(*dest),
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        2,
        "前置条件：两个 `t.has_tag(..)` 调用点都要降出来，实得 {dests:?}"
    );
    for d in &dests {
        assert_eq!(
            f.type_map.get(d),
            Some(&Type::Bool),
            "调用点目的槽 id={d} 该是 Bool（改前 `__contains__` 推不出来 ⇒ 方法返回停在 I64 ⇒ 按整数打印 1），实得 {:?}",
            f.type_map.get(d)
        );
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "print_bool").count(),
        2,
        "两条 `print(t.has_tag(..))` 该按 Bool 选打印器（真值 True 与 False），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝返回停在 I64 ⇒ 按整数打印，`println_i64` 不应出现，实得调用: {calls:?}",
    );
}

/// 批次 580（旁路 cleanup 车道编号，代码 `988a553d`，台账行 `worktree.md:285`；
/// 站点同 :4716 那层嵌套 `infer` 里的字符串方法臂，现 :4744-4775，`m2 == "join"` 那一支）
/// ／旧 ⑫ 方法返回推断扩面。
/// 症状（580 记录 ＋ 同一份差分夹具 `class_tag_probe.dcase` 的第三格）：
/// `return ",".join(self.tags)` 里 `join` 不在推断表内 ⇒ 方法返回落空 ⇒ 停在 I64，
/// 调用点 `print` 把连接串的指针当整数打出来。修法＝`infer_global_ty` 的 Call 臂解构补绑 `args`，
/// 并给嵌套 `infer` 同步加 join 臂（Str 接收者上 join 恒返回 Str）。
/// 期望值来源：同一份源在 CPython 下的实拍真值 `web,db`，对应编译期＝目的槽 `Str`＋走 `println_str`。
/// 边界（本条不覆盖）：批次 587 在同一条臂上扩出的"未知接收者的字符串方法族"（`upper`／`strip` 等）
/// 只在臂的 `matches!` 表里出现，本条夹具打不到它，撤 587 那半不构成对本条的变异；
/// 运行期连接值仍归 `tests/diff/cases/class_tag_probe.dcase` 与差分步。
#[test]
fn str_join_method_return_marks_callsite_dest_as_str() {
    let mirs = lower_all(
        r#"class Tagged:
    def __init__(self, tags):
        self.tags = tags
    def tag_str(self):
        return ",".join(self.tags)

t = Tagged(["web", "db"])
print(t.tag_str())
"#,
    );

    let f = mir(&mirs, "main");
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func == "Tagged::tag_str" => Some(*dest),
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        1,
        "前置条件：`t.tag_str()` 的调用点要降出来，实得 {dests:?}"
    );
    for d in &dests {
        assert_eq!(
            f.type_map.get(d),
            Some(&Type::Str),
            "调用点目的槽 id={d} 该是 Str（改前 join 不在推断表里 ⇒ 方法返回落空停在 I64 ⇒ 按整数打连接串的指针），实得 {:?}",
            f.type_map.get(d)
        );
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        1,
        "`print(t.tag_str())` 该按 Str 选打印器（真值 web,db），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝连接串指针按整数打，`println_i64` 不应出现，实得调用: {calls:?}",
    );
}

/// 批次 592（旁路 cleanup 车道编号，代码 `d4750ffa`，台账行 `worktree.md:297`；
/// **编号与主树重合**：`worktree.md:184` 那行"批次 592"是 bootstrap 车道的另一批
/// （#167 余项①／`find_snippet_lines` 成因批）⇒ 引用以哈希 `d4750ffa` 为身份。
/// 站点同那层嵌套 `infer` 里的 `__collect__` 臂，现 :4728）／旧 #167 一族。
/// 症状（592 记录 ＋ 差分夹具 `tests/diff/cases/class_str_comprehension.dcase`）：
/// 列表推导 `[len(n) for n in self.names]` 脱糖成 `__collect__(iter, λ)`，推断器不认这个调用形
/// ⇒ 返回方法被否决成 I64 ⇒ `print` 打的是向量句柄而不是内容。修法＝补 `__collect__` 臂，
/// 元素型取 λ 体表达式（带 `if` 的筛形取 then 支），取不到时兜底 `DynamicArray(I64)`。
/// 期望值来源：同一份源在 CPython 下的实拍真值 `[1, 2, 1]`，对应编译期＝目的槽
/// `DynamicArray(I64)`，且 `print` 走向量打印路径（`py_json_dumps_vec_typed`）而不是整数打印器。
/// 边界（本条不覆盖）：元素型来自 λ 体推导的那半（本条夹具的元素是 `len(n)`＝整数，
/// 与兜底值同形，所以撤掉臂会红、但臂内取元素型那条线不能单独被本条区分）——
/// 字符串元素的推导形留在差分夹具里；`__collect__` 在 `infer_global_ty`（现 :2079）里的另一臂
/// 走的是全局变量路径，与本条的"方法返回"路径不同站点。
#[test]
fn list_comprehension_method_return_keeps_vector_shape_at_callsite() {
    let mirs = lower_all(
        r#"class Stats:
    def __init__(self):
        self.names = ["x", "yy", "z"]
    def summary(self):
        return [len(n) for n in self.names]

st = Stats()
print(st.summary())
"#,
    );

    let f = mir(&mirs, "main");
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func == "Stats::summary" => Some(*dest),
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        1,
        "前置条件：`st.summary()` 的调用点要降出来，实得 {dests:?}"
    );
    for d in &dests {
        assert_eq!(
            f.type_map.get(d),
            Some(&Type::DynamicArray(Box::new(Type::I64))),
            "调用点目的槽 id={d} 该保留向量形（改前推导式被否决成 I64 ⇒ print 打裸句柄），实得 {:?}",
            f.type_map.get(d)
        );
    }

    let calls = call_symbols(f);
    assert!(
        calls.iter().any(|c| c == "py_json_dumps_vec_typed"),
        "打印一个向量该走 `py_json_dumps_vec_typed` 那条路（真值 [1, 2, 1]），实得调用: {calls:?}",
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝向量句柄被当整数打，`println_i64` 不应出现，实得调用: {calls:?}",
    );
}

/// 批次 575（旁路 cleanup 车道编号，代码 `fe213d56`，台账行 `worktree.md:287`；
/// 站点＝`unannotated_return_ty` 那层嵌套 `infer` 里的 `AstNode::BinaryOp` 臂，
/// 现 :4667-4672——比较与成员运算在 Python 里恒为 Bool，故方法返回可推断）／旧 #195 一族。
/// 症状（575 记录自陈的余项 ＋ 差分夹具 `tests/diff/cases/class_tag_probe.dcase`）：
/// `def gt(self, a, b): return a > b` 里比较结果推不出类型 ⇒ 方法返回否决停在 I64
/// ⇒ 调用点按整数打布尔值（记录实拍：打 `1` 而不是 `True`）。
/// 期望值来源：同一份源在 CPython 下的实拍真值 `False / True / False`，
/// 对应编译期＝三个调用点目的槽都是 `Bool`，且打印走 `print_bool` 而不是整数打印器。
/// 覆盖面分工（变异实测）：撤掉这一支（整支删掉恒 Bool 那个 `if`）后**只有本条变红**，
/// 批次 10018 那条 579 用例照旧绿 ⇒ `t in self.tags` 走的是 579 的 `__contains__` 调用名臂，
/// 两条臂各自独立、不互为备份；红值＝目的槽回 `Some(I64)`，与 575 记录自陈的余项
/// （调用点"打 `1` 非 `True`"）是同一个症状。
/// 边界（本条不覆盖）：575 同批还放宽了运行期 `py_list_contains` 的内容比较
/// （`runtime/py_additions.c`，类字段串列表的元素是 rodata 字面量指针而非 GC 对象），
/// 那半只有真执行观测得到，仍归差分步；`and`／`or` 这类"按值选择"的运算本条臂刻意不接。
#[test]
fn comparison_method_return_marks_callsite_dest_as_bool() {
    let mirs = lower_all(
        r#"class Cmp:
    def gt(self, a, b):
        return a > b
    def neq(self, a, b):
        return a != b

c = Cmp()
print(c.gt(3, 5))
print(c.gt(9, 2))
print(c.neq(1, 1))
"#,
    );

    let f = mir(&mirs, "main");
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. }
                if func.starts_with("Cmp::gt") || func.starts_with("Cmp::neq") =>
            {
                Some(*dest)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        3,
        "前置条件：三个比较方法调用点都要降出来，实得 {dests:?}"
    );
    for d in &dests {
        assert_eq!(
            f.type_map.get(d),
            Some(&Type::Bool),
            "调用点目的槽 id={d} 该是 Bool（改前比较结果推不出 ⇒ 方法返回停 I64 ⇒ 按整数打布尔值），实得 {:?}",
            f.type_map.get(d)
        );
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "print_bool").count(),
        3,
        "三格都该按 Bool 选打印器（真值 False/True/False），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("println_i64")),
        "改前症状＝布尔值按整数打，整数打印器不应出现，实得调用: {calls:?}",
    );
}

/// 批次 587（旁路 cleanup 车道编号，代码 `a5f8280b`，台账行 `worktree.md:292`；
/// 站点＝同层 `infer` 里 580 那条 `join` 守卫被扩成的"未知接收者字符串方法族"表
/// （现 :4749-4775），加同批在语句位走查里补的裸 `Call` 解包与 `append` 元素型精化
/// （现 :4896））／旧 #195 一族。
/// 症状（587 记录 ＋ 差分夹具 `tests/diff/cases/func_return_str_list.dcase`）：
/// `for t in words: result.append(t.upper())` 里 `t` 是被迭代元素、型别未知，
/// 580 的表只认 `join` ⇒ `.upper()` 推不出 ⇒ `append` 的元素型精化拿不到 Str ⇒
/// 函数返回的 `vec` 元素停在 I64 占位 ⇒ `r[0]` 按整数打字符串指针（记录实拍"打地址"）。
/// 期望值来源：同一份源在 CPython 下的实拍真值 `2 / ALPHA`，对应编译期＝
/// `up_all` 调用点目的槽 `DynamicArray(Str)`、`r[0]` 的 `array_get` 目的槽 `Str`，
/// 且两格分别选整数与字符串打印器。
/// 覆盖面分工（变异实测）：本条同时覆盖 587 的两支——把守卫退回 580 的"只认 `join`"
/// （撤字符串方法族表 :4749-4775）与撤 `append` 的元素型精化（:4896 那行 `seen.insert`）
/// 都在**同一个断言**上变红、读数一字不差（`up_all` 的目的槽回 `DynamicArray(I64)`＝
/// 记录里"元素停在 I64 占位"那一处）⇒ 这两支在本条夹具上是同一条链的上下两环，
/// 本条不能把它们各自单独拆出来证明。
/// 边界（本条不覆盖）：587 还在同层 `infer` 的 `+` 运算上加了"一侧是 Str 即按拼接"的臂
/// （现 :4686，差分夹具 `class_method_str_concat`），本条夹具打不到那一支；
/// 580 那条 `join` 本体由批次 10018 的用例覆盖，本条只扩到表里的其余方法名。
#[test]
fn unknown_receiver_str_method_in_loop_keeps_vector_element_str_across_call() {
    let mirs = lower_all(
        r#"def up_all(words):
    result = []
    for t in words:
        result.append(t.upper())
    return result

r = up_all(["alpha", "beta"])
print(len(r))
print(r[0])
"#,
    );

    let f = mir(&mirs, "main");
    let call_dest = |want: &str| -> Vec<u32> {
        f.stmts
            .iter()
            .filter_map(|s| match s {
                MirStmt::Call { func, dest, .. } if func.starts_with(want) => Some(*dest),
                _ => None,
            })
            .collect()
    };

    let ret = call_dest("up_all");
    assert_eq!(
        ret.len(),
        1,
        "前置条件：`up_all([...])` 的调用点要降出来，实得 {ret:?}"
    );
    assert_eq!(
        f.type_map.get(&ret[0]),
        Some(&Type::DynamicArray(Box::new(Type::Str))),
        "`up_all` 的调用点目的槽该是 vec<Str>（改前 `.upper()` 推不出 ⇒ 元素停 I64 占位），实得 {:?}",
        f.type_map.get(&ret[0])
    );

    let elem = call_dest("array_get");
    assert_eq!(
        elem.len(),
        1,
        "前置条件：`r[0]` 的下标读要降出来，实得 {elem:?}"
    );
    assert_eq!(
        f.type_map.get(&elem[0]),
        Some(&Type::Str),
        "`r[0]` 的目的槽该是 Str（改前按整数打字符串指针），实得 {:?}",
        f.type_map.get(&elem[0])
    );

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        1,
        "`print(r[0])` 该按 Str 选打印器（真值 ALPHA），实得调用: {calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_i64").count(),
        1,
        "只有 `print(len(r))` 那一格按整数打印（真值 2），实得调用: {calls:?}"
    );
}

/// 批次 621（旁路 cleanup 车道编号，代码 `571636ba`，台账行 `worktree.md:329`；
/// 站点＝`inherit_class_members`（现 :2578 起）里 own-`__init__` 分支的收养守卫
/// 「有缺口＋有 `__baseargs__` 标记」双条件，现 :2688）／旧 #195 一族。
/// 症状（621 记录 ＋ 在册夹具 `tests/python_style/t531_own_init_base_args.z`）：
/// 子类自带 `__init__` 且用字面量调基初始化器（`Animal.__init__(self, "Rex")`）时，
/// 608 的收集循环拿实参的 **Var 名**当字段槽名，字面量造不出槽 ⇒ 合成 ctor 的
/// `Struct` 漏掉基布局字段、而位置写按 `Struct` 序走 ⇒ `d.name` 读到 `legs` 的槽、
/// 打出 4（记录 s16 实拍）。
/// 期望值来源：在册夹具 `t531` 的 `// expect:` 前两行（`Rex` / `4`），对应编译期＝
/// ctor 的 `Struct` 字段名按布局序为 `["name", "legs"]`、`name` 的初值槽型别是 `Str`
/// （621 同批把字段型别随字面量类别精化），且 `main` 两格分别选字符串与整数打印器。
/// 覆盖面分工（变异实测）：把双条件守卫改成恒 `continue`（＝改前行为）后本条变红，
/// 红在 `Struct` 字段名序那个断言、实得 `["legs"]`＝记录 s16 实拍的形状
/// （基字段 `name` 整个漏掉、位置写按 `Struct` 序错位）。
/// 边界（本条不覆盖）：`t531` 后半的多继承 `C(A, B)`（真值 7/5/9）走的是"无缺口"
/// 那条 continue，本条不查；运行期取值仍归 python_style。
#[test]
fn own_init_base_call_literal_rebuilds_ctor_struct_in_layout_order() {
    let mirs = lower_all(
        r#"class Animal:
    def __init__(self, name):
        self.name = name

class Dog(Animal):
    def __init__(self):
        Animal.__init__(self, "Rex")
        self.legs = 4

d = Dog()
print(d.name)
print(d.legs)
"#,
    );

    let mut found: Option<(&Mir, &Vec<(String, u32)>)> = None;
    for m in &mirs {
        for e in m.exprs.values() {
            if let MirExpr::Struct { variant, fields } = e {
                if variant.as_str() == "Dog" {
                    found = Some((m, fields));
                }
            }
        }
    }
    let (dog, fields) = found.unwrap_or_else(|| {
        panic!(
            "没有哪个函数体降出变体名为 Dog 的 `Struct`（实得 items {:?}）",
            mirs.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
        )
    });

    let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["name", "legs"],
        "ctor 的 `Struct` 该按合并布局补齐基字段（改前漏 `name`、位置写错位），实得 {names:?}"
    );

    let name_slot = fields
        .iter()
        .find(|(n, _)| n == "name")
        .map(|(_, s)| *s)
        .expect("`Struct` 里应有 name 字段");
    assert_eq!(
        dog.type_map.get(&name_slot),
        Some(&Type::Str),
        "`name` 的初值槽（id={name_slot}）该随字面量精化成 Str（改前基侧参数误推 i64 ⇒ 打指针），实得 {:?}",
        dog.type_map.get(&name_slot)
    );

    let f = mir(&mirs, "main");
    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_str").count(),
        1,
        "`print(d.name)` 该按 Str 选打印器（真值 Rex），实得调用: {calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_i64").count(),
        1,
        "只有 `print(d.legs)` 那一格按整数打印（真值 4），实得调用: {calls:?}"
    );
}

/// 批次 600（旁路 cleanup 车道编号，代码 `1b1946e9`，台账行 `worktree.md:466`；
/// **编号与主树重合**：`worktree.md:190` 那行"批次 600"是 bootstrap 车道的全量检查批
/// ⇒ 引用以哈希 `1b1946e9` 为身份。站点 `src/middle/resolver/resolver.rs:3826` 的
/// `refine_ctor_field_types`，调用点 `src/middle/resolver/typecheck.rs:24`）：
/// 构造器调用点实参的字面量型别要写回类别的字段型别表。
/// 症状（600 记录 ＋ 该批探针实拍）：`self.tag = tag` 脱糖后字段型停在解析期的 `i64`
/// 默认值，调用点 `Animal("Generic")` 的字符串实参没有被读回来 ⇒ 读字段的槽按整数选，
/// 运行期把字符串指针当整数打。修法＝类型检查开头补一趟 `refine_ctor_field_types`
/// （扫 `Assign(Var, Call(class-ctor, args))` 形、param→field 同名映射、只升级 `i64`
/// 默认档，拼写 Str/F64/Bool/`list<elem>`）；该批探针实拍 `Animal` fields 变成
/// `[("name","str")]`。
/// 形状说明（本批 CLI A/B 实拍，日志 `/tmp/b10020v2/ab.log`）：字段读发生在**方法体内**
/// （`return self.tag + " says hi"`）才吃到那张字段表；把类型检查里那一趟摘掉，四处
/// 方法体的字段读槽从 Str 变 I64。而模块级 `print(a.name)` 那种直接读法在摘臂前后
/// 读数一字不变（形参槽由调用点证据另一条路证成 Str），所以那种写法不能证明这一臂。
/// 期望值来源：同一份源在 CPython 下的真值 `Generic says hi`／`Rex barks`
/// （zeta 编译后运行逐字相同）＋ 编译期＝字段读槽 Str；撤臂实拍的 I64 就是改前形状。
/// 边界（本条不覆盖）：同批的 `list<elem>` 与 `Bool` 升级支；跨类继承字段的布局补齐
/// （批次 621 那条用例覆盖）；本形的运行期输出在撤臂前后相同，故本条钉的是类型标记面。
#[test]
fn ctor_field_table_upgrade_reaches_the_field_read_inside_methods() {
    let mirs = lower_all(
        r#"class Animal:
    def __init__(self, tag):
        self.tag = tag
    def speak(self):
        return self.tag + " says hi"

class Dog(Animal):
    def speak(self):
        return self.tag + " barks"

a = Animal("Generic")
d = Dog("Rex")
print(a.speak())
print(d.speak())
"#,
    );

    let mut reads = 0;
    for m in &mirs {
        let hits: Vec<u32> = m
            .exprs
            .iter()
            .filter_map(|(id, e)| match e {
                MirExpr::FieldAccess { field, .. } if field == "tag" => Some(*id),
                _ => None,
            })
            .collect();
        for id in hits {
            reads += 1;
            assert_eq!(
                m.type_map.get(&id),
                Some(&Type::Str),
                "`{}` 里 `self.tag` 的接收槽（id={id}）该拿到字段表升级后的 Str（撤掉那一趟＝停在解析期 i64 默认），实得 {:?}",
                m.name.clone().unwrap_or_default(),
                m.type_map.get(&id)
            );
        }
    }
    assert!(
        reads >= 2,
        "前置条件（正证据）：两个同名方法的字段读都要降出来，实得 {reads} 处"
    );
}

/// 批次 399（代码 `ee58b7d8`，站点 `src/middle/resolver/resolver.rs` 两处：
/// "按体恢复返回类型"内层 `infer` 的算术形状臂（现 :4676，任一浮点操作数 ⇒ F64），
/// 以及 `sig_params_snapshot`（现 :4557）作为覆盖层传进恢复（现 :2424、:5137））：
/// 未标注 `def` 的返回槽类型与 LLVM 签名不许有两个独立来源。
/// 症状（缺陷记录＝在册夹具 `tests/python_style/t435_crossfn_return_slot_type.z` 头部原文）：
/// `def scale(v): return v * 1.0` 的被调方签名已经是 `define double @scale(double)`，
/// 而调用点那个空槽的类型来自 resolver 的声明表——未标注 `def` 在那张表里是
/// `Tuple([])`（unit），于是 double 被写进 int 槽、按位重读成 `4602678819172646912`
/// （0.5 的 IEEE-754 位型；记录里另一格 `forward(1.25)` 打 `4608308318706860032`）。
/// 修法两条臂：体的算术形状（浮点操作数 ⇒ F64）与纯转发时参数的**调用点证据类型**
/// （参数在 AST 里恒为 `"dyn"`，而 `funcs` 表已按调用点把它证成 F64）。
/// 期望值来源：在册夹具 t435 的 `// expect:` 前两行（`scale=0.5`／`copied=0.5`）＋
/// 同一份源在 CPython 下的真值 `0.5`／`1.25`（zeta 编译后运行逐字相同），
/// 编译期＝两个调用点目的槽 F64 且走 `println_f64`。
/// 覆盖面分工（变异实测，日志 `/tmp/b10020v2/mutation_2.log`）：撤签名表覆盖层
/// （`sig_params_snapshot` 换成空表）红在 `forward` 那条断言，实得 `PyDynamic`＝记录里
/// "恢复落空"的形状；同一笔变异同时打红批次 407 那条用例（两条共用这一臂，不互为备份）。
/// 而**撤下浮点操作数那一支（算术形状臂）本套 33 条读数一字不变**——现树上 `scale` 的
/// 目的槽 F64 不由那一支决定（成因未定位，登记成余项）。所以 `scale` 那条断言只是把
/// 在册夹具 t435 的头两行读数在编译期锁住（防放松），不是对算术形状臂的覆盖证明。
/// 边界（本条不覆盖）：同批另外两处改动在 `mir.rs` 与 codegen（`signature_ret_ty` 与
/// `infer_fn_return_type` 的委托），本套只走到降形；记录点名的两格未收窄项
/// （声明与实现不一致 `-> i64` 却 `return 2.7`＝任务 #33；只有整数证据的 `/ % //`
/// 仍按整数走＝numeric 方言族）本条不查。
#[test]
fn crossfn_return_slot_and_forwarded_param_share_one_table() {
    let mirs = lower_all(
        r#"def scale(v):
    return v * 1.0

def forward(v):
    return v

n = scale(0.5)
m = forward(1.25)
print(n)
print(m)
"#,
    );

    let f = mir(&mirs, "main");
    let mut scale_dest: Vec<u32> = Vec::new();
    let mut forward_dest: Vec<u32> = Vec::new();
    for stmt in &f.stmts {
        if let MirStmt::Call { func, dest, .. } = stmt {
            if func.starts_with("scale") {
                scale_dest.push(*dest);
            } else if func.starts_with("forward") {
                forward_dest.push(*dest);
            }
        }
    }
    assert_eq!(
        (scale_dest.len(), forward_dest.len()),
        (1, 1),
        "前置条件：两个调用点都要降出来，实得 scale={scale_dest:?} forward={forward_dest:?}"
    );

    let d = scale_dest[0];
    assert_eq!(
        f.type_map.get(&d),
        Some(&Type::F64),
        "`scale(0.5)` 的目的槽 id={d} 该是 F64（在册夹具 t435 的头两行读数＝scale=0.5/copied=0.5；改前＝调用点空槽按声明表拿 unit、double 写进 int 槽按位重读成 4602678819172646912），实得 {:?}",
        f.type_map.get(&d)
    );
    let d = forward_dest[0];
    assert_eq!(
        f.type_map.get(&d),
        Some(&Type::F64),
        "`forward(1.25)` 的目的槽 id={d} 该从签名表拿到参数的调用点证据型 F64（改前＝AST 里参数恒为 dyn ⇒ 恢复落空 ⇒ 按整数打 1.25 的位型），实得 {:?}",
        f.type_map.get(&d)
    );

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_f64").count(),
        2,
        "两处 `print` 都该按浮点选打印器（真值 0.5／1.25），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝double 按位重读成整数打印，`println_i64` 不应出现，实得调用: {calls:?}",
    );
}

/// 批次 400（代码 `4ce47f9e`，站点 `src/middle/resolver/resolver.rs` 里
/// "这条参数还钉不钉得动"那道闸门：冲突计算现 :1825（`clash`），回退现 :1859-1873
/// （`conflicts.insert` ＋ 形参型回 `PyDynamic` ＋ AST 拼写回 `"dyn"`））：
/// 一条未标注参数在两个调用点收到**不同族**的实参时，不许被单侧证据钉死。
/// 症状（缺陷记录＝在册夹具 `tests/python_style/t436_dyn_param_conflicting_args.z`
/// 头部原文，改前图证也是原文）：`show("hi")` 那条调用点的证据把参数 `v` 全局钉成 str，
/// 于是 `show(3)` 的接收槽也判为 Str、走 `println_str` ⇒ 把整数 3 当 `char*` 解引用
/// ⇒ SIGSEGV（rc=139，整个程序连一行都没输出）。
/// 现树规则（同批记录）：i64 与 f64 算同族（钉 f64，整数实参在调用点加宽，两侧值都对）；
/// 跨族（str×数值、句柄×其余）不可合并 ⇒ 参数保持动态，编译期给一次警告。
/// 期望值来源：在册夹具的两行 `// expect:`（`3`／`14`）＋ CPython 同输入真值逐字相同；
/// 编译期＝`show` 的形参槽是 `PyDynamic`（记录："参数保持动态"）、两个调用点的实参槽
/// 一侧 Str 一侧 I64（正证据：两种族别确实都到了同一形参）、动态槽不被 `println_str`
/// 直接吃（改前形状＝`VoidCall { func: "println_str", args: [<被钉成 Str 的槽>] }`）。
/// 边界（本条不覆盖）：动态槽身上仍没有类型标记，str 值打印的是指针地址（在册夹具头
/// 自陈的残留缺口，故本条不回显 str 侧的值）；同族合并（i64×f64 钉 f64）那一支、
/// 以及 `--report-untyped` 的记录面（`ambiguous_dyn_params`）属 CLI 输出面，本条不查。
#[test]
fn conflicting_argument_kinds_across_callsites_leave_parameter_dynamic() {
    let mirs = lower_all(
        r#"def show(v):
    return v

def main() -> i64:
    show("hi")
    n = show(3)
    print(n)
    print(n + 11)
    return 0
"#,
    );

    let show = mir(&mirs, "show");
    let (pname, pslot) = show.param_indices[0].clone();
    assert_eq!(pname, "v", "前置条件：形参名要传进来");
    assert_eq!(
        show.type_map.get(&pslot),
        Some(&Type::PyDynamic),
        "冲突实参下的形参 `{pname}`（id={pslot}）该保持动态（改前＝被 str 侧证据钉成 Str），实得 {:?}",
        show.type_map.get(&pslot)
    );

    let f = mir(&mirs, "main");
    let sites: Vec<(u32, u32)> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, args, dest, .. } if func.starts_with("show") => {
                Some((args[0], *dest))
            }
            _ => None,
        })
        .collect();
    assert_eq!(sites.len(), 2, "两个调用点都要降出来，实得 {sites:?}");
    let arg_types: Vec<Option<&Type>> =
        sites.iter().map(|(a, _)| f.type_map.get(a)).collect();
    assert!(
        arg_types.contains(&Some(&Type::Str)) && arg_types.contains(&Some(&Type::I64)),
        "前置条件（正证据）：两个调用点的实参槽要一侧 Str、一侧 I64，实得 {arg_types:?}"
    );
    for (_, d) in &sites {
        assert_eq!(
            f.type_map.get(d),
            Some(&Type::PyDynamic),
            "调用点目的槽 id={d} 该随参数保持动态，实得 {:?}",
            f.type_map.get(d)
        );
    }

    for stmt in &f.stmts {
        if let MirStmt::VoidCall { func, args } = stmt {
            if func == "println_str" {
                for a in args {
                    assert_ne!(
                        f.type_map.get(a),
                        Some(&Type::PyDynamic),
                        "`println_str` 不许直接吃动态槽 id={a}（改前＝动态槽被钉成 Str 后按字符串解引用整数），实得调用: {func}"
                    );
                }
            }
        }
    }

    let calls = call_symbols(f);
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "println_i64").count(),
        1,
        "只有 `print(n + 11)` 那一格按整数打印（真值 14），实得调用: {calls:?}"
    );
}

/// 批次 524（代码 `adef6f4c`，站点 `src/middle/mir/mir.rs:68` 补的那一行
/// `Type::Str => Type::Str`）：函数签名返回型只有一个来源＝被调方自己降完的 MIR
/// （`Mir::signature_ret_ty`），那张映射里 `Str` 不许再被兜底臂吞掉。
/// 症状（提交信息原文）：`signature_ret_ty` 以 `_ => Type::I64` 兜底，返回字符串的函数
/// 被强制降成整数 ⇒ "LLVM 函数签名和调用方都把字符串指针当整数处理"（该批所属家族＝
/// 类方法打地址）。改法就一行：`F32`／`F64` 之后补 `Str` 自身一档。
/// 期望值出处：① 提交信息自陈"加 Type::Str 臂后 MIR 返回类型正确标为 Str"；
/// ② 同一份源在 CPython 下的真值 `hello`（zeta 编译后运行逐字相同，`cmp -s` SAME，
/// 读数 `/tmp/b10021/run_524.txt`）；③ 编译期＝`--dump-mir` 里 `return "hello"` 那个值槽
/// 读 `Str`（`/tmp/b10021/mir_524.txt`），与本套 `lower_all` 逐格一致。
/// 覆盖面分工（变异实测）：撤掉 `mir.rs:68` 那一行时，返回语句值槽在 `type_map` 里仍是 Str
/// （第一条断言＝正证据，不红），只有签名那一格回 `Some(I64)`＝记录症状值，所以本条钉的
/// 正是"签名不许比体低一档"这一处。限定名与裸名两份降形都查（`Greeter::greet`／`greet`）。
/// 边界（本条不覆盖）：该批自陈的残留"FieldAccess 结果在方法 MIR 的 type_map 中标为 I64"
/// 由旁路批次 600（`1b1946e9`）修掉，已由 10020 那条用例覆盖；签名发射到 LLVM 那一跳在
/// `src/backend/codegen/codegen.rs:1496`，本套只走到降形。
#[test]
fn string_returning_method_signature_is_not_degraded_to_int() {
    let mirs = lower_all(
        r#"class Greeter:
    def greet(self) -> str:
        return "hello"

g = Greeter()
s = g.greet()
print(s)
"#,
    );
    let methods: Vec<&Mir> = mirs
        .iter()
        .filter(|m| m.name.as_deref().unwrap_or("").ends_with("greet"))
        .collect();
    assert_eq!(
        methods.len(),
        2,
        "前置条件（正证据）：限定名与裸名两份降形都要在，实得 {:?}",
        mirs.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
    );
    for m in methods {
        let ret_val = m
            .stmts
            .iter()
            .find_map(|s| match s {
                MirStmt::Return { val } => Some(*val),
                _ => None,
            })
            .expect("方法体里要有一条 return，否则本条断言是空的");
        assert_eq!(
            m.type_map.get(&ret_val),
            Some(&Type::Str),
            "`{}` 的返回语句值槽 id={ret_val} 该是 Str（真值 `hello`；这是签名读的来源，\
             它不该是被兜底出来的），实得 {:?}",
            m.name.clone().unwrap_or_default(),
            m.type_map.get(&ret_val)
        );
        assert_eq!(
            m.signature_ret_ty(),
            Some(Type::Str),
            "`{}` 的签名返回型不许被兜底臂降成整数（改前＝`_ => I64` 把字符串指针当整数），实得 {:?}",
            m.name.clone().unwrap_or_default(),
            m.signature_ret_ty()
        );
    }
}

/// 批次 643（代码 `04d19eb4`，站点 `src/middle/ctfe/evaluator.rs:171` 的 `"//" | "floordiv"`
/// 那一档；同笔还改了三条差分夹具的 zeta 节拼写）：常量折叠的整数求值器认得哪些算子拼写，
/// 决定 `print(纯整数树)` 能不能在编译期塌成一个字符串常量。
/// 症状（提交信息原文）：zeta 词法把 Python 的 `//` 当行注释（`parser.rs:23 tag("//")`），
/// 差分夹具的操作数在 parse 阶段就被截断；方言面能表达的拼写是 indent 预处理器那条词算子
/// `floordiv`，而 i128 求值器只认 `"//"` ⇒ 折叠落空（该批实拍 v0＝被除数本身 27951846246271，
/// 不等于商 0）。修法＝加别名档 `"//" | "floordiv"`，语义按 Python 向下取整
/// （余数非零且两操作数异号时再减一，现 :175-186）。
/// 期望值来源：同样两道被除式在 CPython 下的真值 `3`／`-167045979064`（`python3` 现跑，
/// zeta 编译后运行 `cmp -s` 逐字相同；读数 `/tmp/b10021/pyout_643.txt`＋`run_643.txt`）＋
/// `--dump-mir` 里两处实参读 `StringLit("3")`／`StringLit("-167045979064")`
/// （`/tmp/b10021/mir_643b.txt`，与本套 `lower_all` 逐格一致）。
/// 覆盖面分工（变异实测）：撤掉 `"floordiv"` 别名后实参不再是折叠出的字符串常量、运行期
/// `floordiv` 调用回来 ⇒ 本头两条断言同时红，红值＝记录症状。
/// 边界（本条不覆盖）：除零那一支（`r == 0` 返回 `None`）是 643 之前就有的守卫，该批自陈余下
/// 2 条差分失败＝#117 union＋除零异常（在册）；`/`（truediv）自批次 642 起故意让 i128 求值器
/// 弃权（`evaluator.rs:187-189` 注释在册），本条不查。
#[test]
fn floordiv_word_operator_folds_at_compile_time() {
    let mirs = lower_all(
        r#"print(17 floordiv 5)
print(-1169321853448 floordiv 7)
"#,
    );
    let f = mir(&mirs, "main");
    let folded: Vec<&str> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::VoidCall { func, args } if func == "println_str" => args.first(),
            _ => None,
        })
        .filter_map(|id| match f.exprs.get(id) {
            Some(MirExpr::StringLit(s)) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        folded,
        vec!["3", "-167045979064"],
        "两处 `print` 的实参该在编译期塌成 CPython 真值那个十进制串（折叠落空＝实参不是字符串常量），实得 {folded:?}"
    );
    let calls = call_symbols(f);
    assert!(
        !calls.iter().any(|c| c == "floordiv"),
        "折叠成功后不该再有运行期 `floordiv` 调用（改前＝这一档不被识别、商留到运行期算），实得调用: {calls:?}"
    );
}

/// 批次 414（代码 `70b46b22`，站点 `src/frontend/parser/stmt.rs` 的 with 降形：
/// 共用骨架 `zeta_try_frame`（现 :1373）、handler 分支体现 :1729-1751）：
/// `with` 体抛异常时收尾的 `__exit__` 必须照跑，否则锁被永久扣住。
/// 症状（提交信息原文）：异常 longjmp 出 with 体，跳过块边界的收尾 exit ⇒ 互斥量一直被
/// 持有 ⇒ 下一次 acquire 永久阻塞（语料探针 `fetch_stocks` rc=124，栈停在
/// `py_threading_lock_acquire`；同一驱动 CPython 1.1 秒跑完）。在册夹具
/// `tests/python_style/t452_with_exception_releases_lock.z` 改前实拍 rc=124 零输出。
/// 修法＝把 with 体下进 try 帧，**两条分支都跑 exit**，handler 分支跑完再 `zeta_raise`
/// 原码（传播语义不变）；`return`／`break`／`continue` 三条边不经 try 帧，仍由
/// terminator 改写各自补一次 `zeta_try_end`，所以每条出口只释放一次。
/// 期望值来源：缺陷记录里那两处符号名（`py_threading_lock_release`／`zeta_raise`）
/// ＋ `--dump-mir` 形状实拍（`/tmp/b10021/mir_414.txt`：handler 分支序＝
/// `zeta_last_error` → `zeta_try_end` → `py_threading_lock_release` → `zeta_raise`；
/// 与本套 `lower_all` 的读数是同一条链，运行期真值仍由在册夹具的 `t452 [1, 4, 5]` 承担）。
/// 释放排在重抛**之前**是这条边的硬要求：排在之后＝那一支永远执行不到，等于没修。
/// 覆盖面分工（变异实测）：从 handler 分支删掉那次 exit 发射 ⇒ 本条的异常出口断言红，
/// 红值＝记录症状（这一支没有释放调用）。
/// 边界（本条不覆盖）：`return`／`break` 两条边的 terminator 改写、以及同批记录里
/// "留下的僵尸帧被下一次 raise 落点吃到"那一类（`load_metadata` SEGV）都要运行期才观测
/// 得到；本条只锁 with 异常出口这一条边的降形形状。
#[test]
fn with_body_exception_path_releases_lock_in_both_try_branches() {
    // 释放与重抛的**顺序**在本条里是断言对象，所以分支要按原序取符号名。
    fn funcs_in_order(stmts: &[MirStmt], out: &mut Vec<String>) {
        for s in stmts {
            match s {
                MirStmt::Call { func, .. } | MirStmt::VoidCall { func, .. } => {
                    out.push(func.clone())
                }
                MirStmt::If { then, else_, .. } => {
                    funcs_in_order(then, out);
                    funcs_in_order(else_, out);
                }
                _ => {}
            }
        }
    }

    let mirs = lower_all(
        r#"import threading

L = threading.Lock()

def raise_out():
    with L:
        raise ValueError("boom")
"#,
    );
    let f = mir(&mirs, "raise_out");

    let frame = f
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::If { then, else_, .. } => Some((then.clone(), else_.clone())),
            _ => None,
        })
        .expect("`with` 体该降出一个 try 帧（`zeta_try_setjmp` 的形状就是这条 If）");
    let mut flat = Vec::new();
    funcs_in_order(&f.stmts, &mut flat);
    assert!(
        flat.iter().any(|c| c == "zeta_try_enter") && flat.iter().any(|c| c == "zeta_try_setjmp"),
        "前置条件（正证据）：try 帧的两条建立调用都要在，否则本条断言打在别的分支上，实得调用: {flat:?}"
    );

    let mut then_funcs = Vec::new();
    funcs_in_order(&frame.0, &mut then_funcs);
    let mut else_funcs = Vec::new();
    funcs_in_order(&frame.1, &mut else_funcs);

    assert_eq!(
        then_funcs
            .iter()
            .filter(|c| *c == "py_threading_lock_release")
            .count(),
        1,
        "`with` 体的正常出口该恰好释放一次（同批纪律：每条出口不得双释放），实得分支调用: {then_funcs:?}"
    );
    assert_eq!(
        else_funcs
            .iter()
            .filter(|c| *c == "py_threading_lock_release")
            .count(),
        1,
        "异常出口（handler 分支）也该恰好释放一次（改前＝这一支没有 exit，锁被永久扣住），实得分支调用: {else_funcs:?}"
    );

    let release = else_funcs.iter().position(|c| c == "py_threading_lock_release");
    let reraise = else_funcs.iter().position(|c| c == "zeta_raise");
    assert!(
        matches!((release, reraise), (Some(r), Some(p)) if r < p),
        "handler 分支要按「先释放、再重抛」排（释放在重抛之后＝那一支跑不到，等于没修；\
         没有重抛＝传播被吞掉，在册夹具注释里的「`R` 里有 4 才算传到外层 except」那一格），实得分支调用: {else_funcs:?}"
    );
}
