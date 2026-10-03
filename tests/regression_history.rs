//! 历史缺陷回归钉（编译期层面）
//!
//! 用途：把已经修好的缺陷在**进程内**钉住——`cargo test --test regression_history`
//! 一次编译、每条用例毫秒级，替代"每次都跑全量差分/python_style"。
//!
//! 覆盖边界（如实说明）：这里只钉**编译期可观测**的缺陷（降形选错了运行期符号、
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
fn lower_all(src: &str) -> Vec<Mir> {
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
