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
