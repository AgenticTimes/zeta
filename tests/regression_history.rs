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
use zetac::middle::ctfe::value::ConstValue;
use zetac::middle::mir::mir::{Mir, MirExpr, MirStmt, SemiringOp};
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

/// 多模块夹具：把 `files` 写进一个临时目录，`entry` 当入口编译。入口里的
/// `from <mod> import <名>` 会走 `Resolver::register` → `load_user_python_module`
/// （**从磁盘读**），这是批次 154/159 那一族（模块前缀键、再导出名）唯一的到达路径。
/// 降形完之后临时目录删掉——MIR 已经在返回值里。
fn lower_multi(files: &[(&str, &str)], entry: &str) -> Vec<Mir> {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "zeta_regression_history_{}_{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("建临时目录失败: {e}"));
    for (name, src) in files {
        std::fs::write(dir.join(name), src)
            .unwrap_or_else(|e| panic!("写 {name} 失败: {e}"));
    }
    let entry_path = dir.join(entry);
    let src = std::fs::read_to_string(&entry_path).expect("入口文件读不出来");
    // 降形同样走大栈线程（和 lower_all 一个理由），临时目录由子线程收尾删除。
    let worker = {
        let dir2 = dir.clone();
        let entry_path2 = entry_path.clone();
        move || {
            let mirs = lower_with_source_dir(&src, Some(&entry_path2));
            let _ = std::fs::remove_dir_all(&dir2);
            mirs
        }
    };
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(worker)
        .expect("起大栈线程失败")
        .join()
        .unwrap_or_else(|e| {
            let _ = std::fs::remove_dir_all(&dir);
            std::panic::resume_unwind(e)
        })
}

fn lower_all_inner(src: &str) -> Vec<Mir> {
    lower_with_source_dir(src, None)
}

fn lower_with_source_dir(
    src: &str,
    entry_path: Option<&std::path::Path>,
) -> Vec<Mir> {
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
    // 与 main.rs:826 同序：文件模式在注册前把源码目录交出去，模块加载才找得到
    // 同目录的 `X.z`／`X/__init__.z`（少了这一步，多模块夹具只会得到空模块表）。
    if let Some(p) = entry_path {
        resolver.set_source_dir(p);
    }
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

// 批次 460（旁路 cleanup 车道，代码 `060b0ea3`，站点 `src/frontend/parser/pattern.rs:37`
// 的引用模式臂＋`parse_ref_pattern`（现 :109））。
// 症状（记录原文）：模式位置的 `&value` 不被识别 → `parse_match_arm` 失败 →
// **整个 `fn` 连同其后顶层项被丢**（W1002）；来源是 `zeta_src/runtime/array.z` 的 `array_get`。
// 修法＝按槽位模型剥掉 `&` 直接绑定内层（i64 word 无移动/借用之分），`&mut` 带词边界检查、
// 递归支持 `&&p`。
// 真值来源（本批取的三侧）：在册夹具 `tests/python_style/t501_ref_pattern_match.z` 的
// `// expect: 42 / 0 / 5` ＋改后二进制实拍（`/tmp/b10022/run_t501_ref_pattern_match.txt`＝
// 42、0、5）＋`--dump-mir`（`/tmp/b10022/mir_f460.txt`：`option_is_some` → `option_get_data`
// → `Assign{lhs: 结果槽, rhs: get_data 的 dest}`）。CPython 侧不适用——`fn`/`match`/`Option`
// 是 zeta 的 Rust 方言拼法，没有 Python 对照写法。
// 本条不锁运行期取值（42/0/5 由夹具承担），锁编译期两格：① 函数与后续顶层项都还在；
// ② `&` 模式绑的是内层载荷（`option_get_data` 的目的槽流进返回槽），不是引用本身。
#[test]
fn reference_pattern_arm_keeps_function_and_binds_inner_payload() {
    fn assigns_in_order(stmts: &[MirStmt], out: &mut Vec<(u32, u32)>) {
        for s in stmts {
            match s {
                MirStmt::Assign { lhs, rhs } => out.push((*lhs, *rhs)),
                MirStmt::If { then, else_, .. } => {
                    assigns_in_order(then, out);
                    assigns_in_order(else_, out);
                }
                MirStmt::For { body, else_body, .. } | MirStmt::While { body, else_body, .. } => {
                    assigns_in_order(body, out);
                    assigns_in_order(else_body, out);
                }
                _ => {}
            }
        }
    }

    let mirs = lower_all(
        r#"fn pick(o: Option<i64>) -> i64 {
    match o {
        Some(&value) => value,
        None => 0,
    }
}
fn pick_mut(o: Option<i64>) -> i64 {
    match o {
        Some(&mut value) => value,
        None => 7,
    }
}
fn after_ref_pattern(o: Option<i64>) -> i64 {
    return 99;
}
"#,
    );

    // ① 三个顶层项都在（改前症状就是 `pick` 之后的项被带走）
    let names: Vec<&str> = mirs.iter().filter_map(|m| m.name.as_deref()).collect();
    for want in ["pick", "pick_mut", "after_ref_pattern"] {
        assert!(
            names.contains(&want),
            "引用模式不该把函数丢掉（缺一个＝改前那条 W1002 又回来了），实得顶层项: {names:?}"
        );
    }

    // ② 两条臂（`&value`／`&mut value`）都按「取内层载荷 → 写进返回槽」降
    for name in ["pick", "pick_mut"] {
        let f = mir(&mirs, name);
        let mut calls = Vec::new();
        fn collect(stmts: &[MirStmt], out: &mut Vec<(String, u32)>) {
            for s in stmts {
                match s {
                    MirStmt::Call { func, dest, .. } => out.push((func.clone(), *dest)),
                    MirStmt::If { then, else_, .. } => {
                        collect(then, out);
                        collect(else_, out);
                    }
                    _ => {}
                }
            }
        }
        collect(&f.stmts, &mut calls);
        let payload = calls
            .iter()
            .find(|(c, _)| c == "option_get_data")
            .unwrap_or_else(|| panic!("`{name}` 里没取载荷（实得调用: {calls:?})"))
            .1;
        assert!(
            calls.iter().any(|(c, _)| c == "option_is_some"),
            "`Some(..)` 臂头该先做 is_some 测试（正证据：这条臂真跑了），实得调用: {calls:?}"
        );

        let mut asg = Vec::new();
        assigns_in_order(&f.stmts, &mut asg);
        let ret = f
            .stmts
            .iter()
            .find_map(|s| match s {
                MirStmt::Return { val } => Some(*val),
                _ => None,
            })
            .unwrap_or_else(|| panic!("`{name}` 没有返回语句"));
        assert!(
            asg.iter().any(|(lhs, rhs)| *lhs == ret && *rhs == payload),
            "`&`／`&mut` 该剥掉、把内层载荷（槽 {payload}）赋进返回槽 {ret}，实得赋值: {asg:?}"
        );
    }
}

// 批次 371（主线，代码 `0fff1809`，站点 `src/frontend/parser/pattern.rs:66-80`：
// 在 alt 之后统一 `many0` 收集 `| 后续模式`，非空即折成 `AstNode::OrPattern`）。
// 症状（记录原文）：`A | B`、`"x" | "y"` 这类臂头过去**只有左边被消费**——
// `parse_struct_pattern` 对裸路径必定成功（pattern.rs:135 的 Var 兜底），所以非字面量开头的链
// 根本拿不到剩余的 `| …`；后果是 `advanced_patterns_test` 丢 62 行。
// 编译期结论：一条 or 臂折成**一次** `"||"` 调用、两个实参各是一个被或进来的分支测试
// （`--dump-mir` 实拍＝`/tmp/b10022/mir_f371.txt`：`Call{func:"||", args:[7,9], dest:11}` 与
// `Call{func:"||", args:[14,16], dest:18}`，`type_map` 里 7/9/14/16 都是 `Bool`）。
// 运行期真值仍由 official 的 advanced_patterns 夹具承担，本条不重复锁。
#[test]
fn or_pattern_arm_head_lowers_to_one_disjunction_per_arm() {
    let mirs = lower_all(
        r#"fn alt_head(x: Option<i64>) -> i64 {
    match x {
        Some(1) | Some(2) => 10,
        Some(3) | Some(4) => 20,
        _ => 30,
    }
}
fn after_or_chain(x: i64) -> i64 {
    return 77;
}
"#,
    );

    let names: Vec<&str> = mirs.iter().filter_map(|m| m.name.as_deref()).collect();
    assert!(
        names.contains(&"after_or_chain"),
        "or 臂后面的顶层项不该被丢（改前＝臂头只吃左边，剩余文本没人消费），实得顶层项: {names:?}"
    );
    let f = mir(&mirs, "alt_head");

    let mut disjunctions: Vec<Vec<u32>> = Vec::new();
    fn collect(stmts: &[MirStmt], out: &mut Vec<Vec<u32>>) {
        for s in stmts {
            match s {
                MirStmt::Call { func, args, .. } if func == "||" => out.push(args.clone()),
                MirStmt::If { then, else_, .. } => {
                    collect(then, out);
                    collect(else_, out);
                }
                _ => {}
            }
        }
    }
    collect(&f.stmts, &mut disjunctions);

    assert_eq!(
        disjunctions.len(),
        2,
        "两条 or 臂该各折成一次 `||`（改前＝只有左边被消费，一条都折不出来），实得: {disjunctions:?}"
    );
    for args in &disjunctions {
        assert_eq!(
            args.len(),
            2,
            "or 链该恰好带两个被或进来的分支，实得: {args:?}"
        );
        for id in args {
            assert_eq!(
                f.type_map.get(id),
                Some(&Type::Bool),
                "槽 {id} 该是一个分支测试的结果（Bool）——正证据：`||` 两侧真的各算了一次比较"
            );
        }
    }
}

// 批次 367（主线，代码 `62f65255`，站点 `src/frontend/indent.rs:488-496`：
// `find_inline_colon` 里「冒号之后的代码已有本语句自己的 `{`」这条守卫）。
// 症状（记录原文）：`for i: usize in 0..10 {` 的类型注解冒号曾被行内冒号规则当成 Python 单行块，
// 改写成语法错误的 `for i { usize in 0..10 { }`，`primezeta_usize_test` **丢 36 行**。
// 本批实测的发射路径liveness（10021 立的规矩：改写类站点先测哪条路径真经过它）：
// 该 fixture 的 `calc` 走 `rewrite_inline_body` → `is_header_start`（首词 `for` 在
// HEADER_KEYWORDS）→ `find_inline_colon`，撤掉守卫后改写真发生、`calc` 与 `after_typed_for`
// 一起消失（变异 M3 实拍＝本批验证表）。
// 边界：循环体真跑那半属批次 368（`var_id`／`counter_id` 分离那条，站点在 `gen.rs`，
// 本套按 10014 的规矩避开），本条只锁「注解冒号不改写」＝循环形状与后续顶层项还在。
#[test]
fn typed_for_variable_annotation_is_not_rewritten_as_one_line_block() {
    let mirs = lower_all(
        r#"fn calc() -> i64 {
    let mut sum = 0
    for i: usize in 0..5 {
        sum = sum + i
    }
    return sum
}
fn after_typed_for() -> i64 {
    return 55;
}
"#,
    );

    let names: Vec<&str> = mirs.iter().filter_map(|m| m.name.as_deref()).collect();
    assert!(
        names.contains(&"after_typed_for"),
        "带类型注解的 for 行不该把后面的顶层项带走（改前＝改写成语法错误、静默丢行），实得顶层项: {names:?}"
    );

    let f = mir(&mirs, "calc");
    let (iterator, pattern, body) = f
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::For {
                iterator,
                pattern,
                body,
                ..
            } => Some((*iterator, pattern.clone(), body.clone())),
            _ => None,
        })
        .expect("`for i: usize in 0..5 { … }` 该降出一条 For（改前那行被改写成 `for i { … }` 就没有循环了）");
    assert_eq!(
        pattern, "i",
        "循环变量名该是注解前的 `i`（`usize` 是类型不是名字的一部分）"
    );
    match f.exprs.get(&iterator) {
        Some(MirExpr::Range { start, end }) => {
            let lo = f.exprs.get(start);
            let hi = f.exprs.get(end);
            assert!(
                matches!(lo, Some(MirExpr::IntLit(0)))
                    && matches!(hi, Some(MirExpr::IntLit(5))),
                "`0..5` 该原样进 Range 的两个端点（实得 start={lo:?} end={hi:?}）"
            );
        }
        other => panic!("迭代器槽 {iterator} 该是 `Range`，实得 {other:?}"),
    }
    assert!(
        body.iter().any(|s| matches!(
            s,
            MirStmt::SemiringFold {
                op: SemiringOp::Add,
                ..
            }
        )),
        "循环体不该是空的（改写一旦发生，`sum = sum + i` 就掉出了本语句），实得 body: {body:?}"
    );
}

/// 批次 370（主线 `ceb704c9`）：数组类型注解的长度位收表达式。
///
/// 症状（记录原文口径）：`parse_zeta_array` 的长度位只认 `digit1 | parse_ident`，
/// `[usize; MAX + 1]` 在这里失败 → 整个 `fn` 被拒 → W1002 丢掉其后顶层项
/// （`tests/unit-tests/test_const_expression.z` 丢 14 行，含 `fn main`）。
///
/// 期望值来源：在册 official 夹具 `test_const_expression.z` 的 AOT 退出码（本批实拍
/// 退出码 0＝`arr[50]` 的值，`/tmp/b10023/out370`）＋Python 等价写法
/// `arr=[0]*(MAX+1); arr[50]` 也给 0；缺陷记录里那句"整个 fn 被拒"。
/// 两个接缝都钉：
/// 1. **解析器接缝**（`pub fn parse_array_type`）——长度位是表达式时返回的类型串
///    逐字为 `"[i64; MAX + 1]"`（改前这条直接 Err，所以红点落在这一句本身）；
/// 2. **降形接缝**——带该注解的 `fn` 与它后面的顶层项都还在。
///
/// 覆盖面分工与边界：见批次 10023 的台账（变异实测后写）。
#[test]
fn array_type_size_slot_accepts_const_expression() {
    let parsed = zetac::frontend::parser::parser::parse_array_type("[i64; MAX + 1]")
        .unwrap_or_else(|e| panic!("长度位是表达式时应解析成功（改前在这里 Err），实得 {e:?}"));
    assert!(
        parsed.0.trim().is_empty(),
        "整个类型注解该被吃满，剩余: {:?}",
        parsed.0
    );
    assert_eq!(
        parsed.1, "[i64; MAX + 1]",
        "长度位原文（去空白）该原样带进类型串"
    );

    let mirs = lower_all(
        r#"fn fill(buf: [i64; MAX + 1]) -> i64 {
    return 3
}
fn after_array_expr() -> i64 {
    return 4
}
"#,
    );
    let fill = mir(&mirs, "fill");
    assert!(
        fill
            .param_indices
            .iter()
            .any(|(n, id)| n == "buf" && fill.type_map.contains_key(id)),
        "`buf` 该作为形参登记，实得 param_indices: {:?}",
        fill.param_indices
    );
    assert!(
        mirs.iter().any(|m| m.name.as_deref() == Some("after_array_expr")),
        "带数组注解的 `fn` 后面那个顶层项不该被丢"
    );
}

/// 批次 362（主线；源码笔 `8aa318d8`，文档笔 `5921ae0e`）：路径关键字加词边界。
///
/// 症状（记录原文口径）：`tag("self")` 没有词边界，`self_compile_test` 被截成
/// 路径 `self` ＋ 剩下的 `_compile_test` ⇒ 整条调用解析失败、**其后所有语句静默丢弃**
/// （`minimal_compiler.z` 因此丢 230 行）。
/// 本批实拍补一条口径：该笔的**源码**是隔了一笔才提交的（`5921ae0e` 的提交说明写了
/// 两处改动、实际只提了文档），所以引用这一格要带 `8aa318d8`。
///
/// 期望值来源：在册夹具 `tests/python_style/t408_self_prefix_ident.z` 的
/// `// expect: 10 / 14 / 16`（本批用当前二进制实拍同值，`/tmp/b10023/out408`）
/// ＋形状侧 `--dump-mir`（`/tmp/b10023/mir_f362h.txt`：实参槽 3 由
/// `Assign { lhs: 3, rhs: 1 }` 从形参绑来，调用 `args: [3]`）。
/// **CPython 侧不适用**：`fn`／`-> i64` 是 zeta 的 Rust 方言拼法。
///
/// 读到的现行事实（写给下批省一趟）：用户函数的调用点符号带**实例后缀**
/// （`make` 在 MIR 里是 `make_1`、`bb()` 是 `bb_0`），所以断言按前缀取，别写死全名。
#[test]
fn self_prefixed_local_stays_one_identifier_in_call_args() {
    let mirs = lower_all(
        r#"fn make(x: i64) -> i64 {
    return x * 2
}
fn uses_self_prefix(a: i64) -> i64 {
    let self_compile_test = a
    let doubled = make(self_compile_test)
    return doubled
}
fn after_self_prefix(b: i64) -> i64 {
    return b + 2
}
"#,
    );
    let u = mir(&mirs, "uses_self_prefix");

    // 形参 → self 前缀局部：这条 Assign 是症状的正面证据（它存在说明
    // `let self_compile_test = a` 整条被解析了）。
    assert!(
        top_assigns(u).contains(&(3, 1)),
        "`let self_compile_test = a` 该是把形参槽 1 赋进槽 3，实得顶层赋值: {:?}",
        top_assigns(u)
    );

    let mut calls = u.stmts.iter().filter_map(|s| match s {
        MirStmt::Call { func, args, .. } => Some((func.clone(), args.clone())),
        _ => None,
    });
    let (callee, args) = calls
        .next()
        .expect("`make(self_compile_test)` 该降出一次调用（改前整条失败）");
    assert!(
        calls.next().is_none(),
        "函数体里只该有这一次调用，多出来的＝解析错位"
    );
    assert!(
        callee.starts_with("make"),
        "被调名该是 `make`（带实例后缀），实得 {callee}"
    );
    assert_eq!(
        args,
        vec![3],
        "实参该是那个 self 前缀局部（槽 3），不是 `self` 路径本身"
    );
    assert!(
        mirs.iter().any(|m| m.name.as_deref() == Some("after_self_prefix")),
        "`fn` 不该因为这一条语句被整块丢掉"
    );
}

/// 批次 334（主线 `3f7504aa`，任务旧 #51）：flat 文件里 `//` 按**行内括号深度**判定。
///
/// 症状（记录原文口径）：`//`→`floordiv` 的重写挂在 `normalize_blocks` 的
/// `changed` 后面，而 flat（无缩进）文件不需要改写 ⇒ `changed` 为假，整个重写被跳过，
/// `print(x // y)` 的后半截被当行注释吃掉（W1002 截断，"编译成功"却一个字不打）。
/// 修法＝无缩进证据时逐处判：该列上 `(`/`[` 未闭合 ⇒ 在表达式内部 ⇒ 是运算符；
/// **深度跨行携带**。
///
/// 期望值来源：在册夹具 `tests/python_style/t403_floor_div_inside_call.z` 的
/// `// expect: 3` ＋ CPython 现跑（本批夹具 `print(\n len(xs) // 2\n)` 两侧都打 2，
/// `/tmp/b10023/out334b`）＋形状侧 `--dump-mir`（`/tmp/b10023/mir_f334b.txt`：
/// `Call { func: "floordiv", args: [18, 22], dest: 23 }` → `VoidCall { println_i64, [23] }`）。
///
/// 夹具故意取**跨行调用**那一形：`//` 被吃掉时剩下的 `print(` ＋ `)` 仍然配平，
/// 前置断言（:48）不会红，所以红点只能落在下面这几句形状断言上——
/// 与批次 10022 那三条"红点在 :48、形状断言没执行到"的格子互补。
///
/// 覆盖面分工与边界：见批次 10023 的台账（变异实测后写）。
#[test]
fn flat_file_double_slash_inside_open_bracket_is_the_operator() {
    let mirs = lower_all("xs = [1, 2, 3, 4]\nprint(\n    len(xs) // 2\n)\n");
    let main = mir(&mirs, "main");

    let folds: Vec<(Vec<u32>, u32)> = main
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, args, dest, .. } if func == "floordiv" => {
                Some((args.clone(), *dest))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        folds.len(),
        1,
        "`//` 在未闭合的 `(` 里该是整除运算符（改前它被当注释吃掉，一处都不剩），实得: {folds:?}"
    );
    let (args, dest) = folds[0].clone();
    assert_eq!(args.len(), 2, "整除该有两个实参，实得 {args:?}");
    assert_eq!(
        int_lit(main, args[1]),
        Some(2),
        "除数该是字面量 2（t403 那条形如 `x // y`，本夹具取真值 2）"
    );
    assert!(
        main.stmts.iter().any(|s| matches!(
            s,
            MirStmt::VoidCall { func, args } if func == "println_i64" && args == &[dest]
        )),
        "整除结果槽 {dest} 该直接喂给打印（喂错槽＝把被吃掉的那半截当没有）"
    );
}

/// 批次 327（代码 `e4e5591e`，站点 `src/middle/ctfe/value.rs` 的 `ConstValue::binary_op`
/// 整数两臂：比较器先问＝现 :179 与 :185，配套新增的 `compare_int`（现 :248）／
/// `compare_uint`（现 :261）；记录＝`roadmap.md:10541`，标题把病因写成"比较结果的 `1/0`
/// 不在下型层，在**常量折叠**层"）／旧 #45 一族。
///
/// 症状（记录原文）：`ConstValue::binary_op` 的 Int/Int、UInt/UInt 两臂把算术器结果
/// **无条件** `.map(ConstValue::Int)`，而算术器 `binary_op_int` 里兼任了
/// `== != < <= > >=` 六臂、返回 `(left == right) as i64` ⇒ 折叠层把比较**定型**成整数，
/// `print(3 == 4)` 打 `0`。
///
/// 本条补的是批次 10015 登记的那条余项（"327 的 const／comptime 折叠形状"）：那批实测
/// 撤掉 `compare_int` 的结果臂后 18 条读数一字不变——现树上 `print(3 == 4)` 走的是批次 642
/// `adc0ffba` 的 print 实参改写臂（`ctfe/evaluator.rs:303-308`），`compare_int` 只活在常量位。
/// 所以取**具名 `const` 绑定**那一形：折叠结果不进表达式树，而是落进 MIR 的
/// `global_consts` 表（`src/middle/mir/mir.rs:13`），比较器的产物形状在那里能直接读到。
///
/// 期望值来源：三份真值同批实拍（产物在 `/tmp/b10024/`）。
/// ① 折叠表＝`--dump-mir`：`EQ: Bool(false)`／`NE: Bool(true)`；撤掉比较器臂
///    （改回把比较结果包成 `ConstValue::Int`）后同位置是 `Int(0)`／`Int(1)`＝症状值。
/// ② 运行期＝同一份源编译后跑打 `False`，CPython
///    `EQ = (3 == 4); NE = (3 != 4); print(EQ == NE)` 逐字相同。
/// ③ 编译期形状＝`EQ == NE` 在 MIR 里保留为 `BinaryOp`、槽型 `Bool`、走 `print_bool`。
///    这条通路正是 327 记录"副作用面"那一格自陈的新形状："(Bool, Int) 混合算术在 CTFE 里
///    不再是 Int/Int ⇒ 折叠失败，`transform_expr` 的 `_ =>` 分支保留 BinaryOp 走动态路径"，
///    当时只锁在差分夹具 `truth_fold_arith`／`truth_fold_logic`，本条是它第一次进进程内。
///
/// 覆盖面分工（变异实测，日志 `/tmp/b10024/mutation.log`）：变异 M1＝把 `value.rs:179` 与 `:185`
/// 两行从 `ConstValue::Bool(cmp)` 改回 `ConstValue::Int(cmp as i64)`（＝327 记录里的改前状态，
/// 比较结果重新被折叠层定型成整数）⇒ 全套 45 条**只有本条红**，红点＝折叠表那一断言
/// （`global_consts` 读数），实得 `Some(Int(0))` 正是症状值；本文件那条
/// `constant_folded_comparison_yields_bool_not_int_zero`（现 :889，
/// 走批次 642 `adc0ffba` 的 print 实参改写臂）不红。两条分工＝:889 那条判"print 分发认得布尔"，
/// 本条判"折叠层把比较存成 Bool"——327 新写的比较臂第一次有单独红点（10015 那次变异动的是
/// `compare_int` 函数体内返回值，打不到任何条目，见 :879-885 头注）。
/// 边界（本条不覆盖）：M1 一次改了两行（Int/Int ＋ UInt/UInt 成对），但红点只由 Int/Int 那一支
/// 产生——夹具两个操作数都是有符号字面量，`compare_uint`（现 :261）这一形没被走到，UInt 侧
/// 仍是未钉住的半边（与 :886-887 那格 `as_int()` 消费者缺口不同的一条）。
#[test]
fn const_folded_comparison_is_stored_as_bool_not_int() {
    let mirs = lower_all(
        r#"const EQ: bool = 3 == 4
const NE: bool = 3 != 4
print(EQ == NE)
"#,
    );

    let f = mir(&mirs, "main");

    // ① 折叠层产物：比较器给 Bool，不给 Int。
    for (name, want) in [("EQ", false), ("NE", true)] {
        let want_cv = ConstValue::Bool(want);
        assert_eq!(
            f.global_consts.get(name),
            Some(&want_cv),
            "具名 `const` 绑定 `{name}` 该以 `ConstValue::Bool({want})` 存进 global_consts\
             （改前症状＝折叠层把比较定型成整数，同位置是 `Int({})`），实得 {:?}",
            if want { 1 } else { 0 },
            f.global_consts.get(name)
        );
    }

    // ③ 正证据：两个 Bool 常量之间的比较没被折叠掉，而是留在 MIR 里走动态路径。
    let cmp_dests: Vec<u32> = f
        .exprs
        .iter()
        .filter_map(|(id, e)| match e {
            MirExpr::BinaryOp { op, .. } if op == "==" => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(
        cmp_dests.len(),
        1,
        "前置条件：`EQ == NE` 该以 `BinaryOp` 留在 MIR 里（被折掉＝这条动态通路没跑到），实得 {cmp_dests:?}",
    );
    let d = cmp_dests[0];
    assert_eq!(
        f.type_map.get(&d),
        Some(&Type::Bool),
        "比较结果槽 id={d} 该标 Bool（下型臂：比较运算恒 Bool），实得 {:?}",
        f.type_map.get(&d)
    );
    let calls = call_symbols(f);
    assert!(
        calls.iter().any(|c| c == "print_bool"),
        "`print(EQ == NE)` 该走布尔打印器（真值 False），实得调用: {calls:?}",
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝比较结果按整数打，`println_i64` 不应出现，实得调用: {calls:?}",
    );
}

/// 批次 592（旁路 cleanup 车道编号，代码 `d4750ffa`，台账行 `worktree.md:297`；
/// **编号与主树重合**：`worktree.md:184` 那行"批次 592"是 bootstrap 车道的另一批
/// ⇒ 引用以哈希 `d4750ffa` 为身份。站点＝`unannotated_return_ty` 那层嵌套 `infer`
/// 里的 `__collect__` 臂，现 :4728，元素型取 λ 体表达式那一行＋兜底
/// `unwrap_or(Type::I64)`）／旧 #167 一族。
///
/// 症状（592 记录 ＋ 在册差分夹具 `tests/diff/cases/class_str_comprehension.dcase`）：
/// 列表推导脱糖成 `__collect__(iter, λ)`，推断器不认这个调用形 ⇒ 返回方法被否决成 I64
/// ⇒ `print` 打的是向量句柄而不是内容。
///
/// 本条与批次 10018 那条余项的关系（三次变异后照实改写）：那条余项要求"单独判臂内『取 λ 体
/// 表达式当元素型』那一行"（现 :4739）。本条把元素从 `len(n)`（与兜底同形）换成字面量 `"x!"`
/// （`Str`，与兜底 `I64` 不同形）以后，那一行**仍然打不到**——三次变异的读数见下面的覆盖面分工。
/// 所以本条钉的不是那条余项，而是 592 修完之后的半成品状态：被调方体内推导式结果槽已带 `Str`
/// 标记，调用点目的槽仍读 `DynamicArray(I64)`。那条余项按原样继续登记。
/// 元素取字面量而非 `n + "!"` 是实测选择：`n` 在推导器里读成 `I64`，拼接支（批次 587：一侧 Str、
/// 另一侧 Str 或未知才算拼接）因此拒推 ⇒ 元素型仍落兜底。
///
/// 期望值来源：同一份源在 CPython 下的实拍真值 `['x!', 'x!', 'x!']`（三个元素都是 str）。
/// 编译期形状侧 `--dump-mir` 读**被调方那一段**（`== MIR Stats::tags ==`：
/// `Call { func: "zeta_collect_vec_n", dest: 2 }` → `Return { val: 2 }` →
/// `type_map: 2: DynamicArray(Str)`），进程内读数与 CLI 逐格相同。
/// 注意取段要取到下一个 `== MIR ` 为止：同一次 dump 里 `main` 段的 `7:` 与被调方段的 `2:`
/// 不是同一格，用 `sed '/== MIR main ==/,$p'` 这类"打到文件末尾"的取法会把后者读成前者的
/// （本批实拍踩到）。
///
/// 覆盖面分工（变异实测，日志 `/tmp/b10024/mutation{,2,3,4}.log`；产物 `/tmp/b10024/`）。
/// 三次变异、两种元素形状都跑了：
/// ① M2＝把元素那一行（`resolver.rs:4739`）改成恒走兜底 ⇒ **45 条读数一字不变**；元素换成
///    `n + "!"` 的第一版夹具同样不变。⇒ 那一行在进程内打不到，余项仍开着。
/// ② M2d＝撤 `infer_global_ty` 里另一条同名 `__collect__` 臂（现 :2079）的元素推导（:2102 硬编
///    `Bool`）⇒ **45 条读数一字不变**（该臂在此形下是否被走到未单独取证，这条只算阴性结果）。
/// ③ M2c＝撤整条 592 臂（现 :4728-4741）⇒ **2 条红**：本条的调用点槽那一断言（下面标着
///    "现状锁"那一格，实得 `Some(I64)`＝592 记录写的改前症状"向量被否决成标量"）＋
///    `list_comprehension_method_return_keeps_vector_shape_at_callsite`
///    （现 :1733，同一症状）。⇒ 本条确实跑到这条臂，但判据落在**形状层**（`DynamicArray` 有没有），
///    与本文件那条 `len(n)` 用例共用同一条臂、同一个红点。
/// 结论：本条相对既有 592 用例的增量＝把"被调方槽 `Str` ＋ 调用点槽 `I64` 元素"这一对**同时**钉住
/// （既有那条只读调用点的向量形，元素本来就是 `I64`）。那个调用点 `I64` 不是 ①②任一处的元素推导
/// 给的（两处硬编都不动它），成因站点未定位＝本批开出的新余项。
///
/// 边界（本条不覆盖）两格：
/// ① **调用点目的槽**（`main` 段 `Stats::tags` 那条 `Call` 的 `dest`）仍是兜底值
///    `DynamicArray(I64)`——恢复出来的返回型没传到 caller 槽。本条把这个读数按"现状锁"钉住
///    （修好后它会红，届时要连注释一起改），它不是 592 那条缺陷的症状值：592 的症状是
///    向量整个被否决成标量 `I64`，那一条由本文件那条 `len(n)` 用例覆盖。
/// ② 运行期取值仍是**指针地址**而非字符串内容（编译后跑实拍
///    `[4299448288, 4299448272, 4299448256]`，地址随 ASLR 变）＝向量元素身上的类型标记还没接上，
///    与批次 400 用例头注自陈的残留缺口同一条，故本条不回显运行期读数。
#[test]
fn comprehension_element_marker_is_str_in_the_callee_but_not_at_the_call_site() {
    let mirs = lower_all(
        r#"class Stats:
    def __init__(self):
        self.names = ["x", "yy", "z"]
    def tags(self):
        return ["x!" for n in self.names]

st = Stats()
print(st.tags())
"#,
    );

    let g = mir(&mirs, "Stats::tags");
    let collect: Vec<u32> = g
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func == "zeta_collect_vec_n" => Some(*dest),
            _ => None,
        })
        .collect();
    assert_eq!(
        collect.len(),
        1,
        "前置条件：推导式要降成 `zeta_collect_vec_n` 一处，实得 {collect:?}"
    );
    let d = collect[0];
    let want = Type::DynamicArray(Box::new(Type::Str));
    assert_eq!(
        g.type_map.get(&d),
        Some(&want),
        "被调方里推导式结果槽 id={d} 该是 `DynamicArray(Str)`（元素是字面量 `\"x!\"`）。\
         注意：这个读数不由 592 臂内那一行元素推导给出——把 :4739 改成恒兜底、把 :2102 硬编 `Bool` \
         都不动它（覆盖面分工 ①②），给出它的站点未定位。兜底值是 `DynamicArray(I64)`，\
         与本文件那条 `len(n)` 夹具同形，实得 {:?}",
        g.type_map.get(&d)
    );
    assert!(
        g.stmts
            .iter()
            .any(|s| matches!(s, MirStmt::Return { val } if *val == d)),
        "这一形返回的就是推导式结果槽 {d}（不返回它＝本条读的不是那条臂的产物）"
    );

    // 现状锁（边界①）：调用点目的槽仍是兜底值，不是被调方那个 DynamicArray(Str)。
    let f = mir(&mirs, "main");
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func == "Stats::tags" => Some(*dest),
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        1,
        "前置条件：`st.tags()` 的调用点要降出来，实得 {dests:?}"
    );
    let cd = dests[0];
    assert_eq!(
        f.type_map.get(&cd),
        Some(&Type::DynamicArray(Box::new(Type::I64))),
        "现状锁：调用点目的槽 id={cd} 现在仍读兜底的 `DynamicArray(I64)`（元素标记没传到 caller 槽，\
         与运行期打指针地址同一条缺口；元素的来源见覆盖面分工，撤整条臂时这一格读到 `Some(I64)`）。 \
         这条断言会在传过去那一天变红——那时把它改成 `Str` \
         并删掉本条注释里这句现状锁，实得 {:?}",
        f.type_map.get(&cd)
    );

    let calls = call_symbols(f);
    assert!(
        calls.iter().any(|c| c == "py_json_dumps_vec_typed"),
        "打印一个向量该走 `py_json_dumps_vec_typed` 那条路，实得调用: {calls:?}",
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝向量句柄被当整数打，`println_i64` 不应出现，实得调用: {calls:?}",
    );
}

/// 批次 399（代码 `ee58b7d8`，站点两条：`src/middle/resolver/resolver.rs` 里"按体恢复
/// 返回类型"那层嵌套 `infer` 的算术形状臂（任一操作数是浮点 ⇒ F64，现 :4676-4682），
/// 以及 `src/middle/mir/mir.rs:61` 的 `Mir::signature_ret_ty`——记录把它写成
/// "the ONE entry saying what this function's `ret` carries, so the callee's LLVM signature
/// and the caller's destination slot stop being decided by two different rules"）／旧 #45 一族。
///
/// 症状（记录原文）：未标注 `def` 的返回槽类型与 LLVM 签名有两个独立来源 ⇒
/// `def scale(v): return v * 1.0` 的被调方签名已是 `define double @scale(double)`，
/// 调用点那个空槽却按声明表拿 unit ⇒ double 写进 int 槽、按位重读成
/// `4602678819172646912`（0.5 的 IEEE-754 位型）。
///
/// 本条补的是批次 10023 登记的那条余项：本文件那条 399 用例（`scale`／`forward` 两形）
/// **撤下算术形状臂后整套读数一字不变**（成因未定位）——它钉住的是 `sig_params_snapshot`
/// （现 :4557）那条"纯转发时取参数的调用点证据型"的臂。本条先把参数彻底拿掉
/// （`def bonus(): return 10 * 1.5` 没有形参 ⇒ 签名表覆盖层没有可给的证据），
/// 结果这条**同样**打不到算术形状臂（本批变异 M3 实拍：撤掉整支 45 条读数一字不变），
/// 于是把断言挪到那层嵌套 `infer` 的**下游入口** `signature_ret_ty` 上——
/// 这一形里被调方签名与调用点目的槽都从它取，撤掉它的浮点支会红（覆盖面分工见下）。
/// 算术形状臂那一支的余项**仍未销**，且现在多一条实测边界：有参、无参两形都不由它决定。
///
/// 期望值来源：同一份源在 CPython 下的真值 `15.0`，zeta 编译后运行逐字相同（打 `15.0`）；
/// 形状侧 `--dump-mir` 读 `Call { func: "bonus_0", args: [], dest: 2 }` →
/// `type_map: 2: F64` → `VoidCall { println_f64, [2] }`；被调方体内
/// `SemiringFold { op: Mul, values: [2, 3], result: 4 }` 的 `result` 槽也是 F64。
///
/// 覆盖面分工（变异实测，日志 `/tmp/b10024/mutation{,2}.log`）：
/// ① M3＝撤下 resolver 的算术形状臂整支（现 :4673-4680，"任一操作数是浮点 ⇒ F64"，
///    保留后面的 Str 拼接支）⇒ **45 条读数一字不变**。⇒ 无参形同样打不到那一支；
///    本条对 399 的覆盖不在这里，10023 那条余项继续开着。
/// ② M3b＝撤 `src/middle/mir/mir.rs:61` `signature_ret_ty` 的浮点保留支
///    （`Type::F32 => Type::F32`／`Type::F64 => Type::F64` 两行改成 `=> Type::I64`）
///    ⇒ **2 条红**：本条（红点＝`signature_ret_ty()` 那一断言，实得 `Some(I64)`）＋
///    `unannotated_float_return_keeps_f64_at_module_call_site`（现 :271，批次 10001（本树）／
///    主树批次 813，backlog 旧 #268②）。本文件那条 399 用例
///    `crossfn_return_slot_and_forwarded_param_share_one_table`（现 :2109）不红。
/// ⇒ 本条覆盖＝399 记录里"被调方签名与调用点目的槽是同一个读数"那一格（`signature_ret_ty`），
/// 相对 :271 那条的增量＝**无参形**（那条的 `scale` 有形参，证据还能从签名表覆盖层取）；
/// 与 :271 那条共用同一个入口，因此互为同臂红点、不互为备份。不覆盖＝resolver 的算术形状臂（①）。
#[test]
fn float_operand_in_body_sets_the_call_slot_when_the_function_has_no_params() {
    let mirs = lower_all(
        r#"def bonus():
    return 10 * 1.5
print(bonus())
"#,
    );

    let f = mir(&mirs, "main");
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func.starts_with("bonus") => Some(*dest),
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        1,
        "前置条件：`bonus()` 的调用点要降出来，实得 {dests:?}"
    );
    let d = dests[0];
    assert_eq!(
        f.type_map.get(&d),
        Some(&Type::F64),
        "目的槽 id={d} 该是 F64：这个 `def` 没有形参，签名表覆盖层给不出证据，\
         浮点形状只能由被调方的返回读数决定（真值 15.0；改前症状＝double 按位重读成整数），实得 {:?}",
        f.type_map.get(&d)
    );

    let calls = call_symbols(f);
    assert!(
        calls.iter().any(|c| c == "println_f64"),
        "`print(bonus())` 该按浮点选打印器（真值 15.0），实得调用: {calls:?}",
    );
    assert!(
        !calls.iter().any(|c| c == "println_i64"),
        "改前症状＝浮点值按整数位型打，`println_i64` 不应出现，实得调用: {calls:?}",
    );

    // 被调方自身：体的算术形状同样要把结果槽标成 F64（签名与目的槽不许有两个来源）。
    let g = mir(&mirs, "bonus");
    let folds: Vec<u32> = g
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::SemiringFold { op, result, .. } if *op == SemiringOp::Mul => Some(*result),
            _ => None,
        })
        .collect();
    assert_eq!(
        folds.len(),
        1,
        "前置条件：`10 * 1.5` 要降成折叠运算，实得 {folds:?}",
    );
    let r = folds[0];
    assert_eq!(
        g.type_map.get(&r),
        Some(&Type::F64),
        "被调方里乘法结果槽 id={r} 该是 F64，实得 {:?}",
        g.type_map.get(&r)
    );

    // 399 立的那条"唯一入口"：被调方签名与调用点目的槽共用这一条读数（撤掉它的浮点支＝
    // 回到"两个来源"的形状，签名按 int 发、目的槽仍是浮点位型）。
    assert_eq!(
        g.signature_ret_ty(),
        Some(Type::F64),
        "`bonus` 的 `signature_ret_ty()` 该给 F64（返回槽 id={r} 的型在这里被读成签名），实得 {:?}",
        g.signature_ret_ty()
    );
    assert_eq!(
        g.signature_ret_ty(),
        f.type_map.get(&d).cloned(),
        "被调方签名与调用点目的槽必须是同一个读数（399 的症状正是这两个来源分家）：\
         签名={:?} 目的槽={:?}",
        g.signature_ret_ty(),
        f.type_map.get(&d)
    );
}

/// 批次 10024 新开的未修项 ②（`roadmap.md` 的 10024 段「新开两条未修」第二格；
/// 站点＝`src/middle/mir/gen.rs` 里 `global_consts.get(name)` 那个 `match` 的 `_ =>` 兜底臂，
/// 本树 HEAD `7b80115a` 上是 :4935-4939，插入 `MirExpr::Var(id)` ＋ `Type::I64`；
/// `ConstValue::Bool` 在这个 `match` 里**没有自己的臂**）／#20005 余项。
///
/// 症状（10024 记录原文）：`const X: bool = …` 的读取槽按 I64 ⇒ `print(X)` 打 `1` 而不是 `False`。
/// 折叠层已经给出正确答案（批次 327 `e4e5591e` 的比较臂把结果存成 `ConstValue::Bool`），
/// 丢型发生在**读取**那一层：`MirExpr::Var(id)` ＋ `Type::I64` ⇒ 打印分发选中 `println_i64`。
///
/// 本条是**现状锁**（缺陷未修，锁住当前读数并写明它错在哪），不是正确性锁：
/// 期望值来源三份真值同批实拍（产物在 `/tmp/b10025/`）。
/// ① CPython `T = 3 != 4; F = 3 == 4; print(T); print(F)` ⇒ `True` / `False`
///    （`/tmp/b10025/cpython_truth.txt`）。
/// ② 运行期＝同一份源 `zetac /tmp/b10025/c1.z` 直接执行打 `1` / `0`，并带一条
///    `error[W0003]: Typecheck failed (non-fatal)` 提示（rc=0，非致命）⇒ 与 ① 逐字不同＝症状实拍。
/// ③ 编译期形状＝`--dump-mir`：`global_consts` 是 `T: Bool(true)` / `F: Bool(false)`，
///    而两个打印槽是 `Var(2)`/`Var(6)` ＋ `type_map: 2: I64, 6: I64` ＋
///    两次 `VoidCall { func: "println_i64" }`。
///    取段按 `== MIR <名> ==` 切到下一个 `== MIR ` 为止（10024 记的坑：整段尾巴会串到被调方）。
///
/// 覆盖面分工（变异实测，日志 `/tmp/b10025/m1_suite.log`／`m2_suite.log`）：
/// M1＝把这条兜底臂插入的槽型从 `Type::I64` 换成 `Type::Str` ⇒ 全套 46 条**只有本条红**，
/// 翻红的是「打印槽清单」那一条前置断言（实得 `[]`＝`println_i64` 不再被发出）。这一笔的
/// 作用是**站点归因**：夹具的读取确实打在这条 `_ =>` 臂上，而不是 `global_consts` 没命中时
/// 的「普通变量」那一支——两支插入的形状逐字相同（`Var(id)` ＋ `Type::I64`），只看改前读数分不开。
/// M2＝修复方向实拍：在同一个 `match` 里补一条 `ConstValue::Bool(b)` 臂
/// （`IntLit(if *b {1} else {0})` ＋ `Type::Bool` ＋ `return id`）⇒ 同样只有本条红，
/// 而 `--dump-mir` 的读取变成 `type_map: 2: Bool` ＋ `VoidCall { func: "print_bool" }`
/// （`/tmp/b10025/c1.m2.mir.txt`）。运行期那半（`print_bool` 是否真打 `True`／`False`）
/// 本批**未取**——要重编 release 二进制后执行才知道。
///
/// 边界（本条不覆盖）：① 夹具只走具名 `const` 的读取两形，`print(3 != 4)` 那种直接形不在本条；
/// ② `ConstValue` 共七个变体（`Int`／`UInt`／`Bool`／`Array`／`IntArray`／`Unit`／`String`，
/// `src/middle/ctfe/value.rs:7-22`），这个 `match` 只写了 `Int`／`String`／`Array` 三臂
/// （静态读臂所得，未逐一支变异）⇒ `UInt`／`IntArray`／`Unit` 与 `Bool` 同样落到这条兜底，
/// 本条只钉住 `Bool` 一支的现状，其余三支的读取型未锁。
#[test]
fn bool_const_reads_as_int64_at_the_use_site_current_state() {
    let mirs = lower_all(
        r#"const T: bool = 3 != 4
const F: bool = 3 == 4
print(T)
print(F)
"#,
    );

    let f = mir(&mirs, "main");

    // 正证据（已修的那半）：折叠层给的是 Bool，不是 1/0。
    for (name, want) in [("T", true), ("F", false)] {
        assert_eq!(
            f.global_consts.get(name),
            Some(&ConstValue::Bool(want)),
            "`{name}` 该以 `ConstValue::Bool({want})` 存进 global_consts（327 的产物），实得 {:?}",
            f.global_consts.get(name)
        );
    }

    // 现状锁（未修的那半）：每一次 `print(布尔常量)` 都按整数下发。
    let slots: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::VoidCall { func, args } if func == "println_i64" => {
                args.first().copied()
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        slots,
        vec![2, 6],
        "前置条件：两个打印各一次 `println_i64`（真值应为 `True`／`False`，现在按整数下发），\
         实得槽 {slots:?}",
    );
    for slot in slots {
        assert!(
            matches!(f.exprs.get(&slot), Some(MirExpr::Var(i)) if *i == slot),
            "读取槽 {slot} 该是 `Var({slot})`（布尔常量没被折成字面量，走的是 `_ =>` 兜底那一支），实得 {:?}",
            f.exprs.get(&slot)
        );
        assert_eq!(
            f.type_map.get(&slot),
            Some(&Type::I64),
            "现状锁：读取槽 {slot} 现在标 I64 ⇒ 打印分发拿到整数 ⇒ 打 `1`/`0`。\
             这一格是要被改掉的那格（改成 Bool 才会走 `print_bool`），改动它时本断言必须显式翻红。实得 {:?}",
            f.type_map.get(&slot)
        );
    }

    let calls = call_symbols(f);
    assert!(
        !calls.iter().any(|c| c == "print_bool"),
        "`print(T)`／`print(F)` 现在**不**走布尔打印器（＝缺陷所在）；如果这条红了，\
         说明读取型已被修好，请把本用例的正确性期望改成 True/False 那一侧。实得调用: {calls:?}",
    );
}

/// 批次 184（主线 `e44a238f`，站点＝`infer_unannotated_returns` 那层嵌套 `infer` 里的
/// `AstNode::DictLit` 臂——按**首条**键值推出 `map<K, V>` 的两个类型参数，
/// 本树 HEAD `e53c0147` 上是 :4697-4710；同名臂另有一处在 `infer_global_ty`（现 :2002），
/// 那一处本批五枚夹具形状的 `--dump-mir` 差异都是 0 行，未钉住，见覆盖面分工）／旧 #45 相邻族。
///
/// 症状（记录原文）：`_get is NOT implemented in this build`——全局类型表里它是
/// `Named("map", [])`（**没有类型参数**）⇒ 下标后的值类型 I64 ⇒ `.get(source, 0.5)` 掉出 map 分派
/// ⇒ 裸 `get` ⇒ 撞未实现桩停机。
///
/// 期望值来源：三份真值同批实拍（产物在 `/tmp/b10026/`）。
/// ① CPython 同形源（类方法返回字典字面量＋`h.cfg().get("a", 0)`）打 `1`；
/// ② 运行期＝同一份源 `-o` 编译后执行打 `1`（`s3bin`，rc=0）⇒ 与 ① 逐字相同；
/// ③ 编译期＝`--dump-mir` 的 `main` 段 `type_map` 里三处 `map` 槽（id 7 / 13 / 17）都是
///    `Named("map", [Str, I64])`（`s3_baseA.mir`），且调用面出现 `map_str_key`＋`py_map_contains`、
///    没有裸 `get`。
///    取段按 `== MIR <名> ==` 切，别打到文件末尾（10024 记的那格）。
///
/// 覆盖面分工（变异实测，日志 `/tmp/b10026/m_d4699_suite.log`／`m_d2004_suite.log`）：
/// ① 撤本条那一臂（把 `:4697` 的 `match entries.first()` 换成恒 `None`）⇒ 47 条**只红本条**，
///    红值 `Some(Named("map", []))`＝记录里的症状值。
/// ② 撤另一处同名臂 `infer_global_ty`（现 :2002）⇒ 47 条读数一字不变（阴性结果；先在 CLI 侧
///    对五枚夹具形状跑过 A/B，那一臂的差异行数也全为 0）。⇒ 本条只钉住 :4697 这一处，
///    :2002 那处的症状面（全局字典变量直读）在本套夹具里没有对应形状。
/// ③ ① 的红点落在"三处 `map` 槽带类型参数"那一行（末轮实跑＝`:3402:9`）：前置条件在它之前已先通过、
///    `map_str_key` 那条正证据在它之后未执行到 ⇒ 这两处只是**防放松**，不写成"已证有效"。
#[test]
fn dict_literal_return_carries_map_type_args_at_the_callsite() {
    let mirs = lower_all(
        r#"class Holder:
    def cfg(self):
        return {"a": 1, "b": 2}

h = Holder()
print(h.cfg().get("a", 0))
"#,
    );

    let f = mir(&mirs, "main");

    // 前置条件：`h.cfg()` 这个调用点真的在 main 里（没被内联掉／没被丢掉）。
    let cfg_calls: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func.starts_with("Holder::cfg") => Some(*dest),
            _ => None,
        })
        .collect();
    assert!(
        !cfg_calls.is_empty(),
        "前置条件：`main` 里要有 `Holder::cfg` 的调用点（没有＝这条链没跑到，用例是空的），实得 {:?}",
        f.stmts.len()
    );

    // ③ 症状格：`map` 型槽必须带两个类型参数（首条键值推出来的 Str／I64）。
    let map_slots: Vec<u32> = f
        .type_map
        .iter()
        .filter(|(_, t)| matches!(t, Type::Named(n, _) if n == "map"))
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(
        map_slots.len(),
        3,
        "前置条件：`main` 段该有三处 `map` 型槽（返回槽＋链上两处接收者），实得 {map_slots:?}",
    );
    for id in &map_slots {
        assert_eq!(
            f.type_map.get(id),
            Some(&Type::Named(
                "map".to_string(),
                vec![Type::Str, Type::I64],
            )),
            "`map` 槽 id={id} 该带首条推出来的两个类型参数 `<Str, I64>`（184 的症状正是退成\
             `Named(\"map\", [])` ⇒ 下标值型落 I64 ⇒ `.get` 掉出 map 分派），实得 {:?}",
            f.type_map.get(id)
        );
    }

    // 正证据：`.get` 仍在 map 面上分派，没有掉成裸 `get` 桩。
    let calls = call_symbols(f);
    assert!(
        calls.iter().any(|c| c == "map_str_key"),
        "`get(\"a\", 0)` 该走 map 的键面（`map_str_key`），实得调用: {calls:?}",
    );
    assert!(
        !calls.iter().any(|c| c == "get" || c.starts_with("get_")),
        "184 的症状是掉成裸 `get`／`get_3` 撞未实现桩停机，这类符号不该出现，实得调用: {calls:?}",
    );
}
/// 全局变量的环境写入／读回槽位：`zeta_env_set(名字槽, 值槽)` 与
/// `zeta_env_get(名字槽) -> 目的槽`，只收"名字槽的表达式是 `StringLit(name)`"的那些。
/// 返回（写入的值槽列表, 读回的目的槽列表），按语句出现顺序。
fn env_slots(m: &Mir, name: &str) -> (Vec<u32>, Vec<u32>) {
    let named = |slot: u32| matches!(m.exprs.get(&slot), Some(MirExpr::StringLit(s)) if s == name);
    let mut writes = Vec::new();
    let mut reads = Vec::new();
    for s in &m.stmts {
        match s {
            MirStmt::VoidCall { func, args } if func == "zeta_env_set" && args.len() == 2 => {
                if named(args[0]) {
                    writes.push(args[1]);
                }
            }
            MirStmt::Call { func, args, dest, .. } if func == "zeta_env_get" && args.len() == 1 => {
                if named(args[0]) {
                    reads.push(*dest);
                }
            }
            _ => {}
        }
    }
    (writes, reads)
}

/// 按符号名取出该调用的实参槽列表（只看顶层语句，本套夹具的调用都是顶层的）。
fn call_args(m: &Mir, sym: &str) -> Vec<Vec<u32>> {
    m.stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, args, .. } if func == sym => Some(args.clone()),
            MirStmt::VoidCall { func, args } if func == sym => Some(args.clone()),
            _ => None,
        })
        .collect()
}

/// 批次 172（主线 `9e9c16c1`，站点＝`infer_global_ty` 里带 `method == "__collect__"` 守卫的
/// 那条臂，本树 HEAD `e53c0147` 上是 `src/middle/resolver/resolver.rs:2079`）。
///
/// 症状（记录原文）：列表推导式赋给全局变量时，全局类型表推不出它的类型 ⇒ 变量读回
/// （`zeta_env_get`）整段消失，长度与下标都直接拿写入槽算 ⇒ 非确定性段错误。
///
/// 期望值来源：三份真值同批实拍（产物在 `/tmp/b10027/`）。
/// ① CPython 同形源（`def to_bs(c): return "B" + c`／`CODES = [to_bs(c) for c in ["a","b"]]`）
///    打 `2` 和 `Ba`；
/// ② 运行期＝同一份源 `-o` 编译后执行打 `2` 和 `Ba`（`f172bin`，rc=0）⇒ 与 ① 逐字相同；
/// ③ 编译期＝`--dump-mir` 的 `main` 段：`zeta_env_set` 的值槽（id 16）与两次 `zeta_env_get`
///    的目的槽（id 21／28）都是 `DynamicArray(Str)`（`f172_base.mir`），`len` 打在读回槽上
///    走 `vec_len`、下标打在读回槽上走 `array_get`。
///    取段按 `== MIR <名> ==` 切，别打到文件末尾（10024 记的那格）。
///
/// 覆盖面分工（变异实测，`main` 段差异行数；臂＝在 HEAD 源码上原地改那一支再重编）：
/// ① 撤本条那一臂（`:2079` 的守卫改成 `method == "__collect__" && false`）⇒ 只有本夹具的
///    `main` 段变（82 行／全文件 120 行）：两次 `zeta_env_get` 消失、`CODES` 的名字槽消失、
///    读回槽 21／28 的 `DynamicArray(Str)` 没了；另两枚夹具 0 行。
/// ② 撤同名另一处臂（`infer_unannotated_returns` 里的 `__collect__` 臂，现 :4728）＝未变异；
///    本条只钉 `:2079` 这一处。
/// ③ 全局字典字面量那一族由 10026 的第 47 条钉住（站点＝`infer_unannotated_returns` 的
///    `DictLit` 臂），两条款形状不同（列表推导式／字典字面量返回），不互为备份。
/// ④ 红点末轮实跑落在 `tests/regression_history.rs:3503:5`（前置条件那一行：读回槽空）⇒
///    它之后的症状格与 `vec_len`／`array_get` 正证据**未执行到**，那两处只算**防放松**，
///    不写成"已证有效"。
#[test]
fn global_list_comprehension_keeps_element_type_at_env_reads() {
    let mirs = lower_all(
        r#"def to_bs(c):
    return "B" + c

CODES = [to_bs(c) for c in ["a", "b"]]
print(len(CODES))
print(CODES[0])
"#,
    );

    let f = mir(&mirs, "main");
    let (writes, reads) = env_slots(f, "CODES");

    assert_eq!(
        (writes.len(), reads.len()),
        (1, 2),
        "前置条件：全局 `CODES` 该有一次 `zeta_env_set` 写入＋两次 `zeta_env_get` 读回\
         （172 的症状正是读回整段消失），实得 写入槽 {writes:?}／读回槽 {reads:?}",
    );

    // 症状格：写入槽与每一个读回槽都带元素型 `DynamicArray(Str)`。
    for (what, slot) in std::iter::once(("写入", writes[0]))
        .chain(reads.iter().map(|&r| ("读回", r)))
    {
        assert_eq!(
            f.type_map.get(&slot),
            Some(&Type::DynamicArray(Box::new(Type::Str))),
            "`CODES` 的{what}槽 id={slot} 该是 `DynamicArray(Str)`（推导式元素按 `to_bs` 的返回型推出来），\
             实得 {:?}",
            f.type_map.get(&slot)
        );
    }

    // 正证据：`len`／下标真的打在**读回槽**上，不是绕过读回直接用写入槽。
    let lens = call_args(f, "vec_len");
    let gets = call_args(f, "array_get");
    assert_eq!(
        lens.first().map(|a| a.first().copied()),
        Some(Some(reads[0])),
        "`len(CODES)` 该把 `zeta_env_get` 的读回槽交给 `vec_len`（172 的症状是这一步直接用写入槽），\
         实得 {lens:?}"
    );
    assert_eq!(
        gets.first().map(|a| a.first().copied()),
        Some(Some(reads[1])),
        "`CODES[0]` 该把 `zeta_env_get` 的读回槽交给 `array_get`，实得 {gets:?}"
    );

    // 运行期形状：两个打印各走一次自己的打印机（元素是 Str，长度是 I64）。
    assert_eq!(call_args(f, "println_i64").len(), 1, "`print(len(CODES))` 该有一次 `println_i64`");
    assert_eq!(call_args(f, "println_str").len(), 1, "`print(CODES[0])` 该有一次 `println_str`");
}

/// 批次 300（主线 `610b8db5`，站点＝`module_global_types_uncached` 里"未标注 `def` 的返回类型
/// 从自身 `return` 反推"的补偿块，本树 HEAD `e53c0147` 上是 `resolver.rs:2425-2462`，
/// 被变异的循环在 :2445）。
///
/// 症状（记录原文）：模块级全局变量赋成**未标注函数**的调用结果（`pf = make(…)`）时，
/// 全局类型表里它带 `Tuple([])`（＝没有返回型）⇒ 之后每一次读取都按空结构体处理。
///
/// 期望值来源：三份真值同批实拍（产物在 `/tmp/b10027/`）。
/// ① CPython 同形源（`def tags(): return ["a", "b"]`／`T = tags()`）打 `2` 和 `a`；
/// ② 运行期＝同一份源 `-o` 编译后执行打 `2` 和 `a`（`f171bin`，rc=0）⇒ 与 ① 逐字相同；
/// ③ 编译期＝`--dump-mir` 的 `main` 段：`zeta_env_set` 的值槽（id 5）与两次 `zeta_env_get`
///    的目的槽（id 10／17）都是 `DynamicArray(Str)`（`f171_base.mir`）。
///
/// 覆盖面分工（变异实测，`main` 段差异行数）：
/// ① 撤本条那一臂（:2445 的 `fn_rets.iter_mut()` 改成 `.iter_mut().take(0)`）⇒ 只有本夹具的
///    `main` 段变（82 行／全文件 120 行）：两次 `zeta_env_get` 消失、`T` 的名字槽消失、
///    读回槽 17 从 `DynamicArray(Str)` 退成 `I64`；172 那枚夹具 0 行。
/// ② 记录里的原形状（`pf = make(…)` ＋ `print(pf.tag)`，类实例）在这**三臂**变异下差异行数都是 0
///    ⇒ 那个形状在本套里打不到这一支（10024 记的"登记形状必须实测"再次成立）；本条改用
///    列表返回那一形，`pf.tag` 那形仍未锁。
/// ③ 同块的批次 600 补充（`::` 限定的类方法也进补偿）与 171 的 `fn_rets.get(method)` 那一臂
///    （现 :2232）本批未变异；:2232 那臂在三枚夹具形状下差异行数均为 0（阴性，未钉住）。
/// ④ 红点末轮实跑落在 `tests/regression_history.rs:3583:5`（前置条件那一行：读回槽空）⇒
///    它之后的症状格与 `vec_len`／`array_get` 正证据**未执行到**，那两处只算**防放松**，
///    不写成"已证有效"。
#[test]
fn global_assigned_from_unannotated_call_keeps_return_type_at_env_reads() {
    let mirs = lower_all(
        r#"def tags():
    return ["a", "b"]

T = tags()
print(len(T))
print(T[0])
"#,
    );

    let f = mir(&mirs, "main");
    let (writes, reads) = env_slots(f, "T");

    assert_eq!(
        (writes.len(), reads.len()),
        (1, 2),
        "前置条件：全局 `T` 该有一次 `zeta_env_set` 写入＋两次 `zeta_env_get` 读回\
         （300 的症状是返回型推不出⇒读回整段消失），实得 写入槽 {writes:?}／读回槽 {reads:?}",
    );

    for (what, slot) in std::iter::once(("写入", writes[0]))
        .chain(reads.iter().map(|&r| ("读回", r)))
    {
        assert_eq!(
            f.type_map.get(&slot),
            Some(&Type::DynamicArray(Box::new(Type::Str))),
            "`T` 的{what}槽 id={slot} 该带未标注函数自身 `return` 反推出的 `DynamicArray(Str)`\
             （300 之前这一格是 `Tuple([])`／退化成 `I64`），实得 {:?}",
            f.type_map.get(&slot)
        );
    }

    let lens = call_args(f, "vec_len");
    let gets = call_args(f, "array_get");
    assert_eq!(
        lens.first().map(|a| a.first().copied()),
        Some(Some(reads[0])),
        "`len(T)` 该把读回槽交给 `vec_len`，实得 {lens:?}"
    );
    assert_eq!(
        gets.first().map(|a| a.first().copied()),
        Some(Some(reads[1])),
        "`T[0]` 该把读回槽交给 `array_get`，实得 {gets:?}"
    );

    assert_eq!(call_args(f, "println_i64").len(), 1, "`print(len(T))` 该有一次 `println_i64`");
    assert_eq!(call_args(f, "println_str").len(), 1, "`print(T[0])` 该有一次 `println_str`");
}
/// 批次 154（主线 `b3007ef9` ＋追加 `244ca889`）。站点两处，都在
/// `src/middle/resolver/resolver.rs`（本树 HEAD `e53c0147` 上）：
/// ① 模块前缀剥离臂 :2318-2336（`for pfx in &prefixes { g.strip_prefix(pfx) }`）；
/// ② 全局类型表双键写入臂 :2397-2402（裸名＋`<prefix><name>` 两个键都写）。
///
/// 症状（记录原文）：以下划线开头的全局名拼成 mangled 键是 `datasrc___ROOT`（三个下划线），
/// 用 `rsplit_once("__")` 剥前缀会把前导下划线一起吃掉 ⇒ 剥出来 `PROJECT_ROOT` 这类裸名，
/// 与 walk 从源码 AST 取的 `_ROOT` 不匹配 ⇒ 整张模块全局类型表为空（追加那一笔是同族第二格：
/// 再导出的名字在消费模块里读的是 mangled 键，而表里只有裸名）。类型表的格子没了 ⇒
/// 变量读回槽定成 `I64` ⇒ `print(_ROOT)` 发 `println_i64` 而不是 `println_str`。
///
/// 期望值来源（三份真值同批实拍，产物在 `/tmp/b10028/`）：
/// ① CPython 同形源（`_ROOT = "abc"`＋`print(_ROOT)`，两处消费点）打两行 `abc`；
/// ② 编译期＝`--dump-mir`：`datasrc__show`／`consumer__use_it` 两段各有一次
///    `zeta_env_get("datasrc___ROOT") -> 目的槽`，目的槽类型 `Str`，打印走 `println_str`
///    （`f154a_base.mir`／`f154b_base.mir`）；
/// ③ 运行期（`-o` 编译后执行）**与 ① 不一致**：只打一行 `abc`。原因见下面覆盖面分工 ⑤
///    那条本批新发现的缺陷 ⇒ 本条的期望值取 ①② 两侧，不写成"三方一致"。
///
/// 覆盖面分工（变异实测，差异行数按夹具的 `--dump-mir` 全文统计；臂＝在 HEAD 源码上
/// 原地改那一支再重编）：
/// ① 撤①号臂（把前缀剥离改成 `g.rsplit_once("__")`）⇒ `f154a` 8 行／`f154b` 16 行差异，
///    `datasrc__show` 与 `consumer__use_it` **两段同时** `println_str`→`println_i64`、
///    目的槽 `Str`→`I64`。
/// ② 撤②号臂（把双键写入改成只留 `let _ = pfx;`，即不写 mangled 键）⇒ 差异行数与症状
///    **一字相同**（8/16，同两段同两处）。
/// ③ ⇒ **两臂是一条链，不互为备份**：本批只写一条用例，撤任一臂都打红它，
///    所以这条钉不住"只有其中一臂坏"那种形状，余项里写清楚。
/// ④ 多模块路径本批新（`lower_multi`）：`from datasrc import _ROOT` 要求磁盘上真有
///    `datasrc.z`，走 `register` → `load_user_python_module`；harness 里补的那一步是
///    `resolver.set_source_dir(entry_path)`，与 `src/main.rs:826` 文件模式同序。
///    之前 49 条全是单文件夹具，模块前缀那一族（154／159）到不了。
/// ⑤ 本批顺带**新发现的缺陷（未修，不属于本条收编范围）**：双模块入口的 `main` 段里
///    `consumer__use_it` 这个调用点整条没了（`consumer__init`、两次 `zeta_py_from`、
///    `datasrc__init`、`datasrc__show` 都在，就是少 `consumer__use_it`），
///    AOT 运行因此只打一行而 CPython 打两行；只 import `consumer` 的那枚（`only_consumer.z`）
///    AOT 不打任何输出。登记在 #20005 余项内。
/// ⑥ 进程内变异复验（末轮实跑）：c154a 与 c154b 都是 50 条只红本条这一条，红点与红值
///    一字相同——`tests/regression_history.rs:3782:9`，
///    `夹具A/datasrc__show 的读回槽 id=5 …实得 Some(I64)`（right 侧 `Some(Str)`）。
///    红点落在读回槽类型那一行＝前置条件（读回次数）在撤臂时不变，这条的正证据是实质断言；
///    `println_str`／`println_i64` 两格在红点之后＝未执行到，只算防放松。
#[test]
fn underscore_global_reexport_keeps_str_type_at_module_env_reads() {
    // 夹具 A：main 只 import `show`，`show` 在自己模块里读 `_ROOT`（单跳）。
    let a = lower_multi(
        &[
            (
                "datasrc.z",
                r#"_ROOT = "abc"

def show():
    print(_ROOT)
"#,
            ),
            (
                "main.z",
                r#"from datasrc import show

show()
"#,
            ),
        ],
        "main.z",
    );
    // 夹具 B：`consumer` 自己不定义 `_ROOT`，从 `datasrc` 再导出后读（154 追加那一格）。
    let b = lower_multi(
        &[
            (
                "datasrc.z",
                r#"_ROOT = "abc"

def show():
    print(_ROOT)
"#,
            ),
            (
                "consumer.z",
                r#"from datasrc import _ROOT

def use_it():
    print(_ROOT)
"#,
            ),
            (
                "main.z",
                r#"from consumer import use_it
from datasrc import show

show()
use_it()
"#,
            ),
        ],
        "main.z",
    );

    for (label, mirs, func) in [
        ("夹具A/datasrc__show", &a, "datasrc__show"),
        ("夹具B/datasrc__show", &b, "datasrc__show"),
        ("夹具B/consumer__use_it", &b, "consumer__use_it"),
    ] {
        let f = mir(mirs, func);
        // mangled 键：模块前缀 `datasrc__` ＋源码裸名 `_ROOT`，三个下划线。
        let (writes, reads) = env_slots(f, "datasrc___ROOT");
        assert_eq!(
            (writes.len(), reads.len()),
            (0, 1),
            "前置条件：{label} 里 `_ROOT` 该有一次 `zeta_env_get` 读回、零次写入\
             （写入在 `datasrc__init` 段），实得 写入槽 {writes:?}／读回槽 {reads:?}",
        );

        let dest = reads[0];
        assert_eq!(
            f.type_map.get(&dest),
            Some(&Type::Str),
            "{label} 的读回槽 id={dest} 该带模块全局类型表里的 `Str`\
             （154 之前整张表为空，这一格是 `I64`），实得 {:?}",
            f.type_map.get(&dest),
        );

        let strs = call_args(f, "println_str");
        assert_eq!(
            strs.first().map(|a| a.first().copied()),
            Some(Some(dest)),
            "`print(_ROOT)` 该把读回槽 {dest} 交给 `println_str`，{label} 实得 {strs:?}",
        );
        assert_eq!(
            call_args(f, "println_i64").len(),
            0,
            "154 的症状是把 `Str` 读成 `I64`⇒发 `println_i64`，{label} 不该有该符号，\
             实得调用: {:?}",
            call_symbols(f),
        );
    }
}

/// 批次 402（主线 `34bd4324`；本批只取落在 `typecheck_new.rs` 的那半臂）。
/// 站点＝`src/middle/resolver/typecheck_new.rs:123`（`string_to_type` 签名表里的
/// `"Str" => return Type::Str`，本树 HEAD `ddf8343c` 上）。
///
/// 症状（记录原文＋本批实拍）：`Str` 是 Zeta 自己的字符串类型拼写（`tests/unit-tests/selfhost.z`
/// 的 `fn parse(input: Str)` 用的就是它），但签名表只写了 python 侧的小写 `str`／`string` ⇒
/// `Str` 落到通用名分支，回来是 `Named("Str")` 这个**假类** ⇒ 调用点据返回值把目的槽 typed 成
/// 非 `Str`，打印走 `println_i64`（打的是 `char*` 的数值）。
/// 另一半臂在 `src/middle/mir/gen.rs:1142`（形参槽那格，撤了会让 `s[i]` 下成 `map_get`）
/// ——`gen.rs` 是主树在重构的文件，本车道按约定不碰，那半形状的覆盖登记在余项里。
///
/// 期望值来源（三侧同批实拍，产物 `/tmp/b10029/`）：
/// ① CPython 同形源（去掉 `Str` 注解：`def first(s): return s[0]` ＋ `print(first("abc"))`）打 `a`；
/// ② 运行期＝同一份源 `-o` 编译后执行打 `a`（`f402bin`，rc=0）⇒ 与 ① 相同；
/// ③ 编译期＝`--dump-mir` 的 `main` 段：`Call { func: "first_1", dest: 2 }` 的目的槽 2 是 `Str`，
///    打印走 `println_str(2)`（`f402_base.mir`）。取段按 `== MIR <名> ==` 切到下一个 `== MIR `。
///
/// 覆盖面分工（`--dump-mir` 全文差异行数 × 进程内 50→52 条实跑）：
/// ① 撤本条那一臂（删 `:123` 那行）⇒ 只有 f402 变（行数见台账），f155 那枚 0 行；
///    进程内红点＝本条，症状格＝目的槽类型（`Str` 变 `Named("Str")`／`I64`，实得值见台账）。
/// ② 批次 155 那臂（同文件 :112）由下一条用例钉住，两臂同函数不同拼写臂、形状不互为备份。
/// ③ `gen.rs:1142` 那半臂（形参槽 ⇒ `s[i]` 走 `str_get` 而不是 `map_get`）本批未变异：
///    本条不断形参槽那一格，`s[0]` 的下标分派只作为 f402 的 `first` 段现状记录。
#[test]
fn str_spelling_in_signature_table_keeps_callsite_dest_str() {
    let mirs = lower_all(
        r#"def first(s: Str) -> Str:
    return s[0]

print(first("abc"))
"#,
    );
    let f = mir(&mirs, "main");

    // 调用点带实例后缀（`first` → `first_1`），按被调名取前缀＋纯数字后缀。
    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. }
                if func == "first"
                    || func
                        .strip_prefix("first_")
                        .map_or(false, |x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit())) =>
            {
                Some(*dest)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        1,
        "前置条件：`main` 段该有且只有一次对 `first` 的调用（打不出调用点就等于没测到这条臂），\
         实得 {dests:?}"
    );
    let dest = dests[0];

    assert_eq!(
        f.type_map.get(&dest),
        Some(&Type::Str),
        "`first(...)` 调用点目的槽 id={dest} 该按签名表里的 `Str` 拼写定成 `Type::Str`\
         （402 之前 `Str` 不在表里⇒落通用名分支成假类 `Named(\"Str\")`，这一格退成非 Str），\
         实得 {:?}",
        f.type_map.get(&dest)
    );

    let strs = call_args(f, "println_str");
    assert_eq!(
        strs.first().map(|a| a.first().copied()),
        Some(Some(dest)),
        "`print(first(\"abc\"))` 该把目的槽 {dest} 交给 `println_str`，实得 {strs:?}"
    );
    assert_eq!(
        call_symbols(f)
            .iter()
            .filter(|c| c.as_str() == "println_i64")
            .count(),
        0,
        "402 的症状是返回值按整数读⇒发 `println_i64`（打出 `char*` 的数值），实得调用: {:?}",
        call_symbols(f)
    );
}

/// 批次 155（主线 `f08da4bc`；本批只取落在 `typecheck_new.rs` 的那半臂）。
/// 站点＝`src/middle/resolver/typecheck_new.rs:112`（`string_to_type` 里裸名
/// `"dict" => return Type::Named("map", …)` 那条归一化臂）。
///
/// 症状（记录原文）：`Type::from_string` 在批次 153 已把 `dict` 归一成 `map`，但**签名解析走的是
/// 另一条路**（`typecheck_new::string_to_type`），那条路没补 ⇒ 调用点把 `-> dict` 的返回值当
/// 非 map 处理 ⇒ 之后的 `.get(...)`／`.values()` 落到 opaque 兜底发裸符号（REasyQuant 的
/// `_load_split_factors() -> dict` 之后 `.get(stock_code)` 一共 7 处）。
/// 另一半臂（`dict.get` 返回值取 map 的值类型）在 `gen.rs`，本车道不碰，登记在余项里。
///
/// 期望值来源（三侧同批实拍，产物 `/tmp/b10029/`）：
/// ① CPython 同形源（`def m(): return {"a": 1}` ＋ `v = m()` ＋ `print(v.get("a", 0))`）打 `1`；
/// ② 运行期＝同一份源 `-o` 编译后执行打 `1`（`f155bin`，rc=0）⇒ 与 ① 相同；
/// ③ 编译期＝`--dump-mir` 的 `main` 段：`Call { func: "m_0", dest: 4 }` 的目的槽与存进环境的
///    那格都是 `Named("map", …)`，`.get("a", 0)` 的判存在走 `py_map_contains`＋键面 `map_str_key`
///    （`f155_base.mir`）。
///
/// 覆盖面分工（`--dump-mir` 差异行数 × 进程内实跑）：
/// ① 撤本条那一臂（:112 的 `Named("map")` 改成 `Named("dict")`，＝不做归一化）⇒ 只有 f155 变，
///    f402 那枚 0 行；进程内红点＝本条的症状格（目的槽的 map 名）。
/// ② 批次 402 那臂（同文件 :123 的 `Str` 拼写）由上一条用例钉住；两臂是同一个
///    `string_to_type` 函数里的**不同拼写臂**，撤各自只红自己那条 ⇒ 互为独立覆盖。
/// ③ 155 记录里的另外两臂（泛型 `dict[K, V]` 与 `lt(dict, …)`，现 :289／:363 那两处
///    `if tn == "dict" { "map" }`）本批未变异＝夹具走的是裸名 `-> dict`，那两形未锁，登记余项。
#[test]
fn bare_dict_return_annotation_normalizes_to_map_at_callsite() {
    let mirs = lower_all(
        r#"def m() -> dict:
    return {"a": 1}

v = m()
print(v.get("a", 0))
"#,
    );
    let f = mir(&mirs, "main");

    let dests: Vec<u32> = f
        .stmts
        .iter()
        .filter_map(|s| match s {
            MirStmt::Call { func, dest, .. }
                if func == "m"
                    || func
                        .strip_prefix("m_")
                        .map_or(false, |x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit())) =>
            {
                Some(*dest)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        dests.len(),
        1,
        "前置条件：`main` 段该有且只有一次对 `m` 的调用，实得 {dests:?}"
    );
    let dest = dests[0];

    // 症状格：签名解析把裸名 `dict` 归一成 `map` 之后，调用点目的槽才是 map 名。
    let got = match f.type_map.get(&dest) {
        Some(Type::Named(n, _)) => n.clone(),
        other => {
            panic!(
                "`m()` 调用点目的槽 id={dest} 该是 `Type::Named(\"map\", …)`\
                 （155 之前签名路径没做 dict→map 归一化），实得 {other:?}"
            )
        }
    };
    assert_eq!(
        got,
        "map".to_string(),
        "`-> dict` 注解在签名解析路径也要归一化成 `map`，实得命名类型 {got}"
    );

    // 正证据：`.get(k, d)` 的键面走 map 族分派（不是裸 `get`）。撤臂时这一格在红点之后，
    // 所以它只算防放松。
    let calls = call_symbols(f);
    assert!(
        calls.iter().any(|c| c.starts_with("map_")),
        "`v.get(\"a\", 0)` 该在 map 面上分派（`map_str_key`），实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "get" || c.starts_with("get_")),
        "155 的症状是落到 opaque 兜底发裸 `get`／`_get`，这类符号不该出现，实得调用: {calls:?}",
    );
}

/// 批次 10031（移植主树批次 150 `41e15672`）：Python 标量注解拼写 `float`／`int`
/// 在四处型串解析臂上的别名。
/// 症状（缺陷记录原文）：`float` 留作不透明命名型时——
/// ① 每个这样的形参拿到整数调用约定（调用点各插一条 fptosi）；
/// ② `-> float` 的函数只有在 `infer_fn_return_type` 恰好看见浮点字面量时才返回 f64，
///    否则发出非法 IR（整型函数 `ret double`），整个编译在链接前中止；
/// ③ 即便被调方签名已是 double，调用点仍把结果记成 i64，`print(fee(2.0))` 打的是
///    f64 的位模式（4611686018427387904）。
/// 现树规则：`float`→F64、`int`→I64，形参槽、签名返回型、调用点目的槽、打印分派四格一致。
///
/// 覆盖面分工（逐臂撤除，`cargo test --test regression_history` 实测，批次 10031）：
/// ① 撤 `gen.rs:1498` 的 `|| pt_str == "float"` ⇒ 红在第一格（形参 `amount` 槽读回 I64）。
/// ② 撤 `types/mod.rs:320` 的 `"int" => Type::I64` ⇒ 红在第二格（形参 `rate` 槽读回 `Named("int")`）。
/// ③ 撤 `typecheck_new.rs:140` 的 `"float" => return Type::F64` ⇒ 红在第三格
///    （调用点目的槽读回 `Named("float")`＝记录里"f64 位模式"那一格）。
///    三臂各红各的格 ⇒ 互为独立覆盖。
/// ④ 阴性读数（不入账为已覆盖）：撤 `new_resolver.rs:440-441` 整对、单撤 `typecheck_new.rs:139`
///    的 `int`、单撤 `types/mod.rs:321` 的 `float` ⇒ 53 条读数一字不变；
///    第三格（`signature_ret_ty`）在③红点之前仍为 F64，说明它不取自这四臂，本条只算防放松。
#[test]
fn python_scalar_annotation_aliases_reach_param_and_callsite_slots() {
    let mirs = lower_all(
        "def fee(amount: float, rate: int) -> float:
    return amount * 2.0

x = fee(1.5, 2)
print(x)
",
    );

    // ① 形参槽：注解拼写 `float`／`int` 解析成真实标量型，不是不透明命名型。
    let fee = mir(&mirs, "fee");
    let slot_of = |want: &str| {
        let hit = fee.param_indices.iter().find(|(n, _)| n == want).map(|(_, id)| *id);
        hit.unwrap_or_else(|| {
            panic!(
                "形参 {want} 应登记在 param_indices，实得 {:?}",
                fee.param_indices
            )
        })
    };
    let amount = slot_of("amount");
    let rate = slot_of("rate");
    assert_eq!(
        fee.type_map.get(&amount),
        Some(&Type::F64),
        "形参 `amount: float` 的槽 id={amount} 该是 F64（改前是不透明命名型，落到整数调用约定），实得 {:?}",
        fee.type_map.get(&amount)
    );
    assert_eq!(
        fee.type_map.get(&rate),
        Some(&Type::I64),
        "形参 `rate: int` 的槽 id={rate} 该是 I64，实得 {:?}",
        fee.type_map.get(&rate)
    );

    // ② 签名返回型：`-> float` 解析成 F64。
    assert_eq!(
        fee.signature_ret_ty(),
        Some(Type::F64),
        "`-> float` 的签名返回型该是 F64（改前是不透明命名型）"
    );

    // ③ 调用点目的槽与被调方签名同读数；打印按 f64 分派，不打位模式。
    let main = mir(&mirs, "main");
    let dest = main
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func.starts_with("fee") => Some(*dest),
            _ => None,
        })
        .expect("main 里有对 fee 的调用");
    assert_eq!(
        main.type_map.get(&dest),
        Some(&Type::F64),
        "调用点目的槽 id={dest} 该与被调方签名同为 F64（改前记成 i64，打的是 f64 位模式）"
    );
    let calls = call_symbols(main);
    assert!(
        calls.iter().any(|c| c.starts_with("println_f64")),
        "`print(x)` 该按 F64 分派到 `println_f64`，实得调用: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("println_i64")),
        "同一格里不该出现整数打印分派，实得调用: {calls:?}"
    );
}

/// 批次 10032（移植主树批次 208 `ac7e9f56`）：PEP 604 联合注解取第一个非 `None` 成员。
/// 症状（缺陷记录原文）：整串 `pd.DataFrame | None` 解析失败⇒类型退化，
/// `ParquetCache.load(...) -> pd.DataFrame | None` 回来是个 map，于是 `df["col"] = v`
/// 编成对着 DataFrame 结构体指针做 `DictInsert`（实测崩在 `map_insert`）。
/// 现树规则：`Type::from_string` 与 `new_resolver::parse_type_string` 都跳过 `None`
/// 取首个成员⇒目的槽拿到类名，赋值分派到该类的 `__setitem__`。
/// 实测口径（本批变异研究）：夹具走的是**方法体返回值推型**（`return Cache()`），
/// 移除 208 的三处注解分支（`types/mod.rs` 的联合分支、`new_resolver.rs` 的联合分支、
/// `gen.rs` 的点号名分派分支）后 55 条结果一字不变⇒ 本条锁的是**这套分派现状**
/// （目的槽拿到类名＋发 `__setitem__` 而不是 `DictInsert`），不是那三处注解分支的锁；
/// 那三处里能直接调到的两处由 `src/middle/types/mod.rs` 与
/// `src/middle/resolver/new_resolver.rs` 的模块内单元测试锁定。
#[test]
fn union_annotation_takes_first_non_none_member_for_setitem_dispatch() {
    let mirs = lower_all(
        r#"class Cache:
    def load(self, path: str) -> Cache | None:
        return Cache()
    def __setitem__(self, k, v):
        print("set", k, v)

c = Cache()
d = c.load("a")
d["col"] = 1
print("done")
"#,
    );
    let main = mir(&mirs, "main");

    let dest = main
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func.ends_with("load") => Some(*dest),
            _ => None,
        })
        .expect("main 里有对 load 的调用");
    let got = match main.type_map.get(&dest) {
        Some(Type::Named(n, args)) if args.is_empty() => n.clone(),
        other => panic!(
            "`c.load(\"a\")` 调用点目的槽 id={dest} 该是类名 `Named(\"Cache\", [])`\
             （208 之前整串 `Cache | None` 解析失败退化成 map），实得 {other:?}"
        ),
    };
    assert_eq!(
        got,
        "Cache".to_string(),
        "联合注解 `Cache | None` 应取第一个非 None 成员 `Cache`，实得 {got}"
    );

    // 症状格：`d["col"] = 1` 走类的 `__setitem__`，不是对着结构体指针的 map 写入。
    let calls = call_symbols(main);
    assert!(
        calls.iter().any(|c| c.ends_with("__setitem__")),
        "`d[\"col\"] = 1` 该分派到 `Cache::__setitem__`，实得调用: {calls:?}"
    );
    assert!(
        !main
            .stmts
            .iter()
            .any(|s| matches!(s, MirStmt::DictInsert { .. })),
        "208 的症状就是发 `DictInsert`（对着结构体指针做 map 写入），这条语句不该出现"
    );
}

/// 批次 10032 第二条：联合注解里的标量拼写（`float | None`）在 MIR 槽面上的现状。
/// 症状来源：批次 150（`41e15672`）的 `float`／`int` 别名缺失＝形参退化；
/// 批次 208（`ac7e9f56`）的联合串整串解析失败＝类型退化。叠加时 `amount: float | None`
/// 既不能留整串也不能退化。
/// 实测口径：这枚夹具的形参槽由调用点实参推型填成 F64，移除 150 的别名两行
/// （`new_resolver.rs:440-441`）与 208 的联合分支后结果一字不变⇒ 本条是现状锁；
/// 那两行别名改由 `new_resolver.rs` 的模块内单元测试（直接调 `parse_type_string`）锁，
/// 即 10031 余项①问的"别名分支要换什么形状才打得到"＝直接调用，不是这套夹具。
#[test]
fn union_scalar_member_reaches_param_slot() {
    let mirs = lower_all(
        "def fee(amount: float | None) -> float:
    return 2.0

x = fee(1.5)
print(x)
",
    );
    let fee = mir(&mirs, "fee");
    let amount = fee
        .param_indices
        .iter()
        .find(|(n, _)| n == "amount")
        .map(|(_, id)| *id)
        .expect("形参 amount 应登记在 param_indices");
    assert_eq!(
        fee.type_map.get(&amount),
        Some(&Type::F64),
        "形参 `amount: float | None` 的槽 id={amount} 该取联合首个非 None 成员并解析成 F64\
         （留整串＝解析失败，退化＝不透明命名型），实得 {:?}",
        fee.type_map.get(&amount)
    );

    let main = mir(&mirs, "main");
    let dest = main
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::Call { func, dest, .. } if func.starts_with("fee") => Some(*dest),
            _ => None,
        })
        .expect("main 里有对 fee 的调用");
    assert_eq!(
        main.type_map.get(&dest),
        Some(&Type::F64),
        "调用点目的槽 id={dest} 该同为 F64"
    );
    let calls = call_symbols(main);
    assert!(
        calls.iter().any(|c| c.starts_with("println_f64")),
        "`print(x)` 该按 F64 分派，实得调用: {calls:?}"
    );
}

/// 批次 10033（移植主树批次 153 `2ed6da43`）：`self.m = {}` 的字典字段不再被类型化成 i64。
/// 症状（缺陷记录原文）：`parse_class` 的字段类型推断只认字面量／bool／浮点／字符串／数组，
/// `DictLit` 落到兜底 `i64` ⇒ 字段上的 map 操作全部失配：`.get`／`.values` 发裸符号（链接失败）、
/// `k in self.m` **恒返回 0**（不报错的静默错值）。
/// 两处站点＝`src/frontend/parser/top_level.rs` 的字段类型推断（本车道该文件在制，本批不改它、
/// 也不做它的变异，因此这一处只算现状锁）＋`src/middle/types/mod.rs` 的 `Type::from_string`
/// 把 `dict`／`dict[K, V]` 归一化成 `map`（该处由本批的模块内单元测试直接锁）。
/// 与 10029（来源批次 155）那条 `-> dict` 注解归一化＝症状同形而站点不同（那条在
/// `typecheck_new.rs` 的 `string_to_type`），两条互不备份。
#[test]
fn dict_field_literal_init_reaches_map_slots() {
    let mirs = lower_all(
        r#"class Store:
    def __init__(self):
        self.m = {}

    def put(self, k, v):
        self.m[k] = v

    def probe(self, k):
        if k in self.m:
            print("hit")
        else:
            print("miss")

s = Store()
s.put("a", 1)
s.probe("a")
s.probe("z")
print(s.m.get("a", 0))
print(len(s.m.values()))
"#,
    );
    // 三侧真值：CPython 打 `hit/miss/1/1`；AOT 二进制 rc=0 同四行；MIR 面＝下面三格。
    // ① 有 `MapNew` 的那一段（`self.m = {}` 的构造）里，目的槽该是 map，不是 i64。
    let ctor = mirs
        .iter()
        .find(|m| {
            m.name.as_deref().map(|n| n.starts_with("Store")).unwrap_or(false)
                && m.stmts.iter().any(|s| matches!(s, MirStmt::MapNew { .. }))
        })
        .expect("构造段里有 `MapNew`（`self.m = {}`）");
    let dest = ctor
        .stmts
        .iter()
        .find_map(|s| match s {
            MirStmt::MapNew { dest } => Some(*dest),
            _ => None,
        })
        .expect("MapNew 带目的槽");
    match ctor.type_map.get(&dest) {
        Some(Type::Named(n, _)) if n == "map" => {}
        other => panic!(
            "`self.m = {{}}` 的字段槽 id={dest} 该是 `Named(\"map\", …)`\
             （153 之前这格＝I64，字段上的 map 操作因此全部失配），实得 {other:?}"
        ),
    }

    // ② 症状格（静默错值那一格）：`k in self.m` 该发 map 的成员判定调用，不是恒 0。
    let probe = mirs
        .iter()
        .find(|m| m.name.as_deref().map(|n| n.ends_with("probe")).unwrap_or(false))
        .expect("降出 probe 段");
    let calls = call_symbols(probe);
    assert!(
        calls.iter().any(|c| c.contains("map_contains")),
        "`k in self.m` 该走 map 成员判定（153 之前恒返回 0＝静默错值），实得调用: {calls:?}"
    );

    // ③ `.values()` 该发 map 方法分支，不是裸 `_values`（153 之前＝链接期找不到符号）。
    let main = mir(&mirs, "main");
    let mcalls = call_symbols(main);
    assert!(
        mcalls.iter().any(|c| c.contains("map_values")),
        "`len(s.m.values())` 该发 map 的 values 分支，实得调用: {mcalls:?}"
    );
    assert!(
        !mcalls.iter().any(|c| c == "_values"),
        "153 的症状之一就是发裸符号 `_values`（链接失败），这个调用名不该出现"
    );
}

/// 批次 627（旁路 cleanup 车道旧编号，代码 `3ba4dd07`；夹具
/// `tests/python_style/t537_method_param_refine.z`；落点
/// `src/middle/resolver/resolver.rs` 的 `refine_method_param_types` ＋
/// `p627_record` ＋ `collect_p627_in_expr`，接线在 `typecheck.rs:29`）／backlog 旧 #195。
/// 症状（627 记录原文＋t537 实拍）：没有类型注解的方法形参在函数表里停在 I64
/// （生成器那一侧的拼写是 PyDynamic），`g.greet("World")` 的字串实参没有回流到
/// 方法体 ⇒ 体内 f-string 的这个部件按整数去 stringify 一个字符串句柄，
/// 打出来是 "Hello, " 加一串地址，而不是 "Hello, World!"。
/// 修法＝resolver 扫模块级调用点，记 `Class::method` → [(形参位置, 类型)]，
/// 交给生成器按位置覆写 `type_map`；守卫＝现状是 I64 或 PyDynamic 才精化，
/// 写了真注解的形参永不覆盖。
/// 期望值来源＝三侧真值：CPython 侧打 "Hello, World!" 和 "hey!"；AOT 侧同值
/// （本批实跑 `target/tmp_b10034/t627b.bin`，rc=0）；MIR 侧 `--dump-mir` 里
/// `Greeter::greet`／`Greeter::tag`／`Greeter::shout` 三段的形参槽都是 `Str`
/// （本批读数）。
/// 三条形参各由 `refine_method_param_types` 里一条语句形态臂精化：
/// `print(g.greet(…))`＝`collect_p627_in_expr` 内对实参的递归（实参下沉）、
/// `g.tag(…)`＝模块级裸 `Call` 语句臂、`s = g.shout(…)`＝赋值右值臂。
/// 变异时各红自己那一格＝独立覆盖。另有两支在本夹具下是阴性（撤臂后 57 条一字
/// 不变、未锁）：`ExprStmt` 臂（`g.tag(…)` 实测降成裸 `Call`，不走表达式语句那一支）
/// 和 `g = Greeter(...)` 的局部类别表那一行（接收者类别由
/// `module_global_types_at` 直接给出，不依赖该表）。读数见批次 10034 台账。
/// 边界（本条不覆盖）：同名的裸段 `greet` 与实例化段 `greet_inst_i64` 里
/// 同一形参槽仍是 PyDynamic（本批 `--dump-mir` 读数）＝"后端取哪一份 MIR"
/// 的另一格，本条不断言；调用点目的槽为 Str 那一格由 628 的用例
/// `method_return_inference_from_param_and_concat_marks_callsites_str` 管，互不备份。
#[test]
fn unannotated_method_params_take_the_argument_type_from_call_sites() {
    let mirs = lower_all(
        r#"class Greeter:
    def __init__(self, prefix):
        self.prefix = prefix
    def greet(self, name):
        return f"{self.prefix}, {name}!"
    def tag(self, name):
        return f"[{name}]"
    def shout(self, word):
        return word + "!"

g = Greeter("Hello")
print(g.greet("World"))
g.tag("XSHG")
s = g.shout("hey")
print(s)
"#,
    );

    // 三段的形参槽各由一条语句形态臂精化（段名＋形参名）。逐格先收成一张表再一次
    // 比较：单格 assert 在循环里只报第一格，某一支变异打掉两格时读不出打在谁身上。
    let mut readings: Vec<(String, Option<Type>)> = Vec::new();
    for (seg, param) in [
        ("Greeter::greet", "name"),
        ("Greeter::tag", "name"),
        ("Greeter::shout", "word"),
    ] {
        let m = mir(&mirs, seg);
        let slot = m
            .param_indices
            .iter()
            .find(|(n, _)| n == param)
            .map(|(_, i)| *i)
            .unwrap_or_else(|| {
                panic!(
                    "{seg} 要降出形参 `{param}` 的槽位（实得 param_indices {:?}）",
                    m.param_indices
                )
            });
        readings.push((format!("{seg}#{param}"), m.type_map.get(&slot).cloned()));
    }
    let expected: Vec<(String, Option<Type>)> = readings
        .iter()
        .map(|(k, _)| (k.clone(), Some(Type::Str)))
        .collect();
    assert_eq!(
        expected, readings,
        "三段形参槽都该按调用点的字面量实参精化成 Str；627 之前这格停 I64／PyDynamic，\
         体内把字符串句柄按整数转成串＝打地址（左＝期望，右＝逐格实得）"
    );
}
