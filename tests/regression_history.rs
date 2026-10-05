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
    lower_pipeline(src, entry_path, true).0
}

/// 容许解析截断的变体（批次 10041 起）：解析没吃满输入时不 panic，把剩余文本当第二格返回。
/// 理由＝批次 325 那一族的失败模式是"第一个打不开的模式把文件余部整段丢掉"（W1002），
/// 走 `lower_with_source_dir` 时这类臂的红点全落在同一句前置断言上、形状断言根本不执行；
/// 有了"哪些函数进得了 MIR"这一格，各臂的集合才互不相同。
fn lower_all_allowing_truncation(src: &str) -> (Vec<Mir>, String) {
    lower_pipeline(src, None, false)
}

fn lower_pipeline(
    src: &str,
    entry_path: Option<&std::path::Path>,
    assert_full_parse: bool,
) -> (Vec<Mir>, String) {
    let (remaining, asts) = parse_zeta(src).unwrap_or_else(|e| panic!("解析失败: {e:?}"));
    if assert_full_parse {
        assert!(
            remaining.trim().is_empty(),
            "解析没有吃满输入，剩余: {:?}",
            remaining.trim()
        );
    }

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
    (mirs, remaining.to_string())
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

/// 批次 651（`21a006e9`）＝跨模块同名类的方法派发撞车——编译期回归钉。
///
/// 症状（651 记录原文＋本批实拍）：两个模块各有一个 `class Cfg`、方法同名（`show`）时，
/// 裸限定键 `Cfg::show` 被后注册的那个模块覆盖 ⇒ 两个调用点打到同一个方法体，
/// 打出来是同一个值（本夹具＝两行都 27 或都 11），而不是 CPython 的 11／27。
///
/// 修法＝注册 `ImplBlock` 时除裸名外再写入模块改写后的别名键（`m647a__Cfg::show`）到
/// `funcs` 与 `registered_funcs`（必须排在裸名注册之后，否则被覆盖），并把 `self` 形参型、
/// 构造器返回的 `StructLit` 变体名、`ret` 字段一起换成改写名，让派发在直取臂命中
/// （消费方＝`gen.rs` 的 `qualified_method_candidate`，先试改写键再走会撞车的尾部回退）。
///
/// 三侧真值（本批实测）：
/// - CPython＝`11`、`27`，rc=0（同三份文件的 `.py` 等价写法，逐行可译）；
/// - AOT 二进制＝`11`、`27`，rc=0（本批 AOT 编译运行实测，编译 rc=0）；
/// - `--dump-mir`＝`main` 段的构造调用目标 `m647a__Cfg`／`m647b__Cfg`、方法调用目标
///   `m647a__Cfg::show`／`m647b__Cfg::show`；两段的 `self` 槽型分别是
///   `Named("m647a__Cfg")`／`Named("m647b__Cfg")`；方法体里的加数常量分别 `1`／`7`；
///   构造段返回的 `Struct` 变体名与段名一字相同。
/// 另有两枚夹具检查同样的格子（调用点目标名＋构造段变体名）：第二枚＝不带 `__init__`
/// 的跨模块同名类（构造器由编译器合成），第三枚＝带基类的同名子类（基类写 `__init__`、
/// 子类显式调用它）。一次性探针实测＝这两枚都打到 `resolver.rs:2853` 那条合成构造器
/// 路径（探针打印 `ty` 是裸名 `Cfg`），`resolver.rs:2735` 那条四形都没到。
#[test]
fn cross_module_same_named_class_methods_keep_their_own_mangled_target() {
    let mirs = lower_multi(
        &[
            (
                "m647a.z",
                r#"class Cfg:
    def __init__(self):
        self.v = 10
    def show(self):
        return self.v + 1
"#,
            ),
            (
                "m647b.z",
                r#"class Cfg:
    def __init__(self):
        self.v = 20
    def show(self):
        return self.v + 7
"#,
            ),
            (
                "main.z",
                r#"from m647a import Cfg as CfgA
from m647b import Cfg as CfgB

a = CfgA()
b = CfgB()
print(a.show())
print(b.show())
"#,
            ),
        ],
        "main.z",
    );

    // ① 四个调用点（两处构造＋两处方法）的目标名，按出现顺序；筛掉的
    //    `zeta_module_decl`／`zeta_py_from`／`println_i64` 是导入与打印的接线。
    let main = mir(&mirs, "main");
    let sites: Vec<String> = call_symbols(main)
        .into_iter()
        .filter(|f| {
            f.ends_with("__Cfg") || f.ends_with("Cfg::show") || f == "Cfg::show" || f == "show"
        })
        .collect();
    let want_sites = [
        "m647a__Cfg",
        "m647b__Cfg",
        "m647a__Cfg::show",
        "m647b__Cfg::show",
    ];
    assert_eq!(
        want_sites.to_vec(),
        sites.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "跨模块同名类的四个调用点都该打到本模块的改写键（顺序＝构造 a、构造 b、方法 a、方法 b）；\
         651 之前别名键没进函数表，方法调用点会落到裸名 `Cfg::show`（＝后注册模块的那个体）。\
         左边＝期望，右边＝实得（含裸名即说明回退臂被走到）"
    );

    // ② 两个方法段的 `self` 槽型：各自模块的改写类名（漏一个＝接收者型回到裸 `Cfg`）。
    let mut recv: Vec<(&str, Option<Type>)> = Vec::new();
    for seg in ["m647a__Cfg::show", "m647b__Cfg::show"] {
        let m = mir(&mirs, seg);
        let slot = m
            .param_indices
            .first()
            .map(|(_, i)| *i)
            .unwrap_or_else(|| panic!("{seg} 要降出 self 形参槽位（实得 param_indices {:?}）", m.param_indices));
        recv.push((seg, m.type_map.get(&slot).cloned()));
    }
    let want_recv: Vec<(&str, Option<Type>)> = [
        ("m647a__Cfg::show", "m647a__Cfg"),
        ("m647b__Cfg::show", "m647b__Cfg"),
    ]
    .into_iter()
    .map(|(s, t)| (s, Some(Type::Named(t.to_string(), vec![]))))
    .collect();
    assert_eq!(
        want_recv, recv,
        "两个方法段的接收者槽型该分别是本模块的改写类名；651 之前 `self` 形参写的是裸名，\
         两段的接收者都指到同一个类（左＝期望，右＝逐格实得）"
    );

    // ③ 两个方法体里的加数常量：a 段是 1、b 段是 7（＝方法体没被换成对方的）。
    let mut addend: Vec<(&str, Option<i64>)> = Vec::new();
    for seg in ["m647a__Cfg::show", "m647b__Cfg::show"] {
        let m = mir(&mirs, seg);
        let lits: Vec<i64> = m
            .exprs
            .values()
            .filter_map(|e| match e {
                MirExpr::IntLit(v) => Some(*v),
                _ => None,
            })
            .collect();
        assert_eq!(
            lits.len(),
            1,
            "{seg} 段内该只有方法体那一个整数字面量（实得 {lits:?}）——多枚＝夹具形状变了，先核对夹具再谈归因"
        );
        addend.push((seg, lits.first().cloned()));
    }
    let want_addend: Vec<(&str, Option<i64>)> =
        vec![("m647a__Cfg::show", Some(1)), ("m647b__Cfg::show", Some(7))];
    assert_eq!(
        want_addend, addend,
        "两个方法段各自带的加数该是 1 和 7（＝派发没把 b 的方法体接到 a 的调用点上）；\
         撞车时两格读数会变成同一个值（左＝期望，右＝逐格实得）"
    );

    // ④ 两个构造段返回的 Struct 变体名＝改写类名（与段名一字相同）。
    let mut variant: Vec<(&str, Option<String>)> = Vec::new();
    for seg in ["m647a__Cfg", "m647b__Cfg"] {
        let m = mir(&mirs, seg);
        let found = m
            .exprs
            .values()
            .find_map(|e| match e {
                MirExpr::Struct { variant, .. } => Some(variant.clone()),
                _ => None,
            });
        variant.push((seg, found));
    }
    let want_variant: Vec<(&str, Option<String>)> = vec![
        ("m647a__Cfg", Some("m647a__Cfg".to_string())),
        ("m647b__Cfg", Some("m647b__Cfg".to_string())),
    ];
    assert_eq!(
        want_variant, variant,
        "构造器返回的 Struct 变体名该带模块前缀（651 之前两枚构造都返回裸 `Cfg`，\
         接收者型随之塌成同一个类＝方法派发撞车；左＝期望，右＝逐格实得）"
    );

    // ⑤⑥ 第二枚夹具＝不带 `__init__` 的类（构造器由编译器合成）。同样是跨模块同名
    //     类，检查合成构造器有没有带上模块前缀。撤 651 里两条 `ret_expr: Some(StructLit)`
    //     站点（`resolver.rs:2735`／`:2853`）后这一格仍不变（探针实测与台账见批次 10035）。
    let mirs2 = lower_multi(
        &[
            (
                "m647a.z",
                r#"class Cfg:
    def show(self):
        return self.v + 1
"#,
            ),
            (
                "m647b.z",
                r#"class Cfg:
    def show(self):
        return self.v + 7
"#,
            ),
            (
                "main.z",
                r#"from m647a import Cfg as CfgA
from m647b import Cfg as CfgB

a = CfgA()
b = CfgB()
print(a.show())
print(b.show())
"#,
            ),
        ],
        "main.z",
    );
    let main2 = mir(&mirs2, "main");
    let sites2: Vec<String> = call_symbols(main2)
        .into_iter()
        .filter(|f| {
            f.ends_with("__Cfg") || f.ends_with("Cfg::show") || f == "Cfg::show" || f == "show"
        })
        .collect();
    assert_eq!(
        want_sites.to_vec(),
        sites2.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "不带 `__init__` 的同名类同样该在四个调用点打到本模块的改写键（合成构造器那两条路径\
         ＝651 的另两处站点）（左＝期望，右＝实得）"
    );
    let mut variant2: Vec<(&str, Option<String>)> = Vec::new();
    for seg in ["m647a__Cfg", "m647b__Cfg"] {
        let m = mir(&mirs2, seg);
        let found = m
            .exprs
            .values()
            .find_map(|e| match e {
                MirExpr::Struct { variant, .. } => Some(variant.clone()),
                _ => None,
            });
        variant2.push((seg, found));
    }
    assert_eq!(
        want_variant, variant2,
        "合成构造器返回的 Struct 变体名也该带模块前缀（左＝期望，右＝逐格实得）"
    );
    // ⑦⑧ 第三枚夹具＝带基类的同名子类（子类自己写 `__init__` 并显式调用基类构造）。
    //     打这形的理由＝一次性探针实测：本形与第二枚（不带 `__init__`）都打到
    //     `resolver.rs:2853` 那条构造器合成路径（探针打印 `ty` 还是裸名 `Cfg`），
    //     而 `:2735` 那条一次都没到。检查的格子仍与第一枚相同：调用点目标名＋
    //     构造段变体名。
    let mirs3 = lower_multi(
        &[
            (
                "m647a.z",
                r#"class Base:
    def __init__(self, tag):
        self.tag = tag

class Cfg(Base):
    def __init__(self, x):
        Base.__init__(self, "Rex")
        self.legs = x

    def show(self):
        return self.legs + 1
"#,
            ),
            (
                "m647b.z",
                r#"class Base:
    def __init__(self, tag):
        self.tag = tag

class Cfg(Base):
    def __init__(self, x):
        Base.__init__(self, "Rex")
        self.legs = x

    def show(self):
        return self.legs + 7
"#,
            ),
            (
                "main.z",
                r#"from m647a import Cfg as CfgA
from m647b import Cfg as CfgB

a = CfgA(3)
b = CfgB(4)
print(a.show())
print(b.show())
"#,
            ),
        ],
        "main.z",
    );
    let main3 = mir(&mirs3, "main");
    let sites3: Vec<String> = call_symbols(main3)
        .into_iter()
        .filter(|f| {
            f.ends_with("__Cfg") || f.ends_with("Cfg::show") || f == "Cfg::show" || f == "show"
        })
        .collect();
    assert_eq!(
        want_sites.to_vec(),
        sites3.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "带基类的同名子类同样该在四个调用点打到本模块的改写键（左＝期望，右＝实得）"
    );
    let mut variant3: Vec<(&str, Option<String>)> = Vec::new();
    for seg in ["m647a__Cfg", "m647b__Cfg"] {
        let m = mir(&mirs3, seg);
        let structs: Vec<&String> = m
            .exprs
            .values()
            .filter_map(|e| match e {
                MirExpr::Struct { variant, .. } => Some(variant),
                _ => None,
            })
            .collect();
        assert_eq!(
            1,
            structs.len(),
            "{seg} 段内该只有构造器那一个 Struct 字面量（多个＝变体名的归因会随哈希顺序漂）"
        );
        variant3.push((seg, Some(structs[0].clone())));
    }
    assert_eq!(
        want_variant, variant3,
        "基类接管后构造器返回的 Struct 变体名也该带模块前缀（左＝期望，右＝逐格实得）"
    );
}

/// 循环之后那句 print 的**操作数形状**（批次 644 的观测点）：
/// - 值是从模块环境现读的 → `env_get("<名字>")`（陈值已被清除，读取留在 MIR 里）；
/// - 值被折成了字面量 → `StringLit("0")`／`IntLit(0)`（＝644 的症状：循环前那条赋值的陈值）；
/// - 其他（直接用一个槽的值）→ `var`。
///
/// 只看**顶层最后一个循环之后**（`for` 在 MIR 里降成 `While`）的第一条 `println_*`，
/// 避免把循环体内那条 print 读进来。
fn print_operand_shape_after_last_for(m: &Mir) -> String {
    let last_for = m
        .stmts
        .iter()
        .rposition(|s| matches!(s, MirStmt::For { .. } | MirStmt::While { .. }))
        .unwrap_or_else(|| {
            panic!(
                "main 顶层没有循环（for 在 MIR 里降成 While），顶层语句形如: {:?}",
                m.stmts
                    .iter()
                    .map(|s| format!("{s:?}").chars().take(40).collect::<String>())
                    .collect::<Vec<_>>()
            )
        });
    let operand = m.stmts[last_for + 1..]
        .iter()
        .find_map(|s| match s {
            MirStmt::VoidCall { func, args } if func.starts_with("println_") => args.first().copied(),
            _ => None,
        })
        .expect("循环之后没有 println 调用");
    let env_name = m.stmts.iter().find_map(|s| match s {
        MirStmt::Call { func, args, dest, .. }
            if func == "zeta_env_get" && *dest == operand =>
        {
            args.first().copied()
        }
        _ => None,
    });
    if let Some(nid) = env_name {
        return match m.exprs.get(&nid) {
            Some(MirExpr::StringLit(n)) => format!("env_get({n})"),
            _ => "env_get(名字读不出)".to_string(),
        };
    }
    match m.exprs.get(&operand) {
        Some(MirExpr::StringLit(s)) => format!("StringLit({s:?})"),
        Some(MirExpr::IntLit(i)) => format!("IntLit({i})"),
        Some(MirExpr::Var(_)) => "var".to_string(),
        Some(_) => "其他形状".to_string(),
        None => "槽缺失".to_string(),
    }
}

/// 来源＝批次 644（`23a32829`，2026-09-29，站点 `src/middle/ctfe/evaluator.rs`）。
///
/// 症状：批次 642 给循环／条件体加了"陈值清除"——体里被赋过值的名字要从 i128 常量表里
/// 删掉，之后的读取才不会被折成常量。那版的收集器 `p642_collect_assigned` 的 `For` 臂只认
/// `AstNode::Var` 形态的循环模式，`for k2, v2 in pairs:` 这种**元组模式**两个绑定名一个都没
/// 收进清除名单 ⇒ 循环前 `k2 = 0` 那条赋值的陈值留在表里 ⇒ 循环之后的 `print(k2)` 被折成 `0`
/// （644 记录里 t518 实拍＝打 `0`，应为 `2`）。
///
/// 修法：`For` 臂改走新增的 `p642_collect_pattern_vars`，递归收集模式里的每个绑定名。
///
/// 三侧真值（本批实测，`target/tmp_b10036/`）：
/// - CPython 侧：`f1`→`1 2 2`、`f2`→`7 9 9`、`f3`→`a b b`、`f4`→`7`、`f5`→`6`、`f6`→`2`；
/// - AOT 侧：六枚都同值、rc=0（`target/tmp_b10036/f{1..6}.bin`，仓根构建并运行）；
/// - `--dump-mir` 侧：六枚夹具循环后那句 print 的操作数分别是 `zeta_env_get("k2")`→`println_i64`、
///   `zeta_env_get("i")`→`println_i64`、`Var`→`println_str`（`f3` 打的是字符串绑定，值仍来自循环）、
///   `zeta_env_get("acc")`→`println_i64`（`f4`、`f5`）、`zeta_env_get("i")`→`println_i64`（`f6`）；
///   `ZETA_DBG_P642=1` 的清除名单：`f5`＝`["x", "acc"]`、`f6`＝`["i"]`（本批 CLI 读数，
///   只作归因证据，本套不断言它）。
///
/// 与旧用例的分工：本文件 :1347 的 `tuple_for_loop_pattern_kills_stale_consts_before_print_folds`
/// 是同来源（批次 644）的第一条，它的夹具里 `v2 = ""` 是字符串、进不了 i128 常量表，
/// 所以"只收元组第一个名字"这种坏法打不到它——本条的 `f3`（`v2 = 0`）与 `f4`（体内赋值）
/// 才是新增的两格；`f1`、`f2` 与旧用例同臂，写在这里是为了让四格读数一次可比。
#[test]
fn loop_pattern_bindings_kill_the_stale_int_const_before_later_prints() {
    // f1：元组模式，读**第一个**绑定名（644 记录里 t518 的原始形状）
    let f1 = r#"
pairs = [(1, "a"), (2, "b")]
k2 = 0
for k2, v2 in pairs:
    print(k2)
print(k2)
"#;
    // f2：裸名模式（644 之前就收集得到的形状）——用来区分"元组递归"和"收集器本身"
    let f2 = r#"
xs = [7, 9]
i = 0
for i in xs:
    print(i)
print(i)
"#;
    // f3：元组模式，读**第二个**绑定名——第一个名字收到、第二个漏掉时只有这一格会变
    let f3 = r#"
pairs = [(1, "a"), (2, "b")]
v2 = 0
for k2, v2 in pairs:
    print(v2)
print(v2)
"#;
    // f4：循环体里的普通赋值（不是循环模式）——这一格走收集器的 `Assign` 臂，
    // 与三枚模式格分开：模式收集整条撤掉时这一格照旧是绿的。
    let f4 = r#"
xs = [3, 4]
acc = 5
for x in xs:
    acc = acc + 1
print(acc)
"#;
    // f5：赋值在循环体的 if 分支里——收集器要多走一层 `If` 臂才收得到 acc
    let f5 = r#"
xs = [3, 4]
acc = 5
for x in xs:
    if x > 3:
        acc = acc + 1
print(acc)
"#;
    // f6：`while` 循环（不是 for）——走收集器的 `While` 臂
    let f6 = r#"
n = 2
i = 0
while i < n:
    i = i + 1
print(i)
"#;
    let readings: Vec<(&str, String)> = [
        ("元组模式·第一个绑定 k2", f1),
        ("裸名模式 i", f2),
        ("元组模式·第二个绑定 v2", f3),
        ("循环体内赋值 acc", f4),
        ("循环体内 if 分支赋值 acc", f5),
        ("while 体内赋值 i", f6),
    ]
    .iter()
    .map(|(label, src)| (*label, print_operand_shape_after_last_for(&mir(&lower_all(src), "main"))))
    .collect();

    assert_eq!(
        readings,
        vec![
            ("元组模式·第一个绑定 k2", "env_get(k2)".to_string()),
            ("裸名模式 i", "env_get(i)".to_string()),
            ("元组模式·第二个绑定 v2", "var".to_string()),
            ("循环体内赋值 acc", "env_get(acc)".to_string()),
            ("循环体内 if 分支赋值 acc", "env_get(acc)".to_string()),
            ("while 体内赋值 i", "env_get(i)".to_string()),
        ],
        "循环模式里的绑定名与体内（含 if／while 嵌套）被赋的值必须全部进陈值清除名单（批次 644／642）——读回字面量＝那条赋值没被清除"
    );
}

/// 批次 665（`f954223a`，2026-09-29，站点 `src/middle/ctfe/evaluator.rs` 的比较折叠臂，
/// 现 :143-164）：**带括号的比较是值比较，不是 Python 链式比较**。
///
/// 症状（665 记录＋本批实测）：改前那臂在**左操作数自己也是比较**时按 Python 链式语义折叠
/// （`a < b != c` ≡ `(a < b) and (b != c)`）。本批实测：真链式拼写在 parser 层就已被
/// `parse_comparison` 逐段折成 `&&`（`print(1 < 2 < 3)` 折与运行都答 True，
/// `print(3 < 2 < 1)` 都答 False，与 CPython 一字相同），到这一臂时外层是逻辑与而不是比较
/// ⇒ 启发式只对**带括号**的拼写生效；而运行期对带括号的形是"先算左再比"，
/// 于是同一份源码折叠与运行期各给一个值：`(-5 == 0) > -16` 折成 False，
/// 不折的路径答 True（`gen_numeric_s664202_001`）。665 把这一臂归回值语义：
/// 先算左、再算右、然后用外层算子比（`:161-163`）。
///
/// 观测点：`print(纯常量比较)` 走 `rewrite_big_print`（:254），它调 `eval_i128_tree`（:300），
/// 折出来的布尔由 :305-306 渲染成 `"True"`／`"False"` 字符串字面量下发给 `println_str`
/// ⇒ 折叠值在 MIR 里直接可读（`VoidCall{args:[N]}` 的 `exprs[N]`）。
///
/// 三侧真值（本批实拍，十一格全一致；夹具件在临时目录 `target/tmp_b10037/`，
/// `cargo clean` 会清掉，八行源码＋三行追加与本条夹具表的顺序一字相同）：
/// - CPython（`c8.py` 八行＋`c3.py` 三行追加）→ True True True True True False True
///   False｜True True True；
/// - 编译期折叠＋AOT 运行（`c8.z`／`c3.z`）同值（`rc=0`）；
/// - 强制走运行期（把操作数换成列表元素，`xs = [-5, 0, -16, 2, 1, 5, 3, 1, 0, 1]`，
///   `r8.z`／`r3.z` 同序）⇒ 同值 ⇒ 折叠与运行期在这一形上一致，正是 665 要的不变式。
///
/// 与既有那条的分工：`:940 constant_folded_comparison_yields_bool_not_int_zero` 钉的是
/// 顶层**裸比较**（`print(3 == 4)`）折出来的是布尔拼写而不是 0/1 ＝批次 642 的渲染臂；
/// 本条钉**嵌套比较**折出来的**值**＝批次 665 的折叠臂。两者红点不同形，互不备份。
///
/// 第八格 `print(1 < 2 == 1)` 钉的是链式拼写走 parser 那条路的结果：外层是 `&&`
/// 而不是比较 ⇒ 这一臂拿到的是"布尔与布尔比"，折成 False（与 CPython 同值）＝现状锁。
///
/// 本批同时更正了站点注释里那句"运行期不支持链式／真链式需要 parser 层节点"——
/// 与上面的实测相悖（`parse_comparison` 早就做收集-折叠，主树 `roadmap.md:846` 记为
/// 2026-09-12 完成，早于 665），注释按实测重写、行数不动。
#[test]
fn parenthesized_comparison_folds_with_value_semantics_not_as_a_python_chain() {
    // (标签, 夹具行, 期望)——前三列一起决定源码，源码与期望不会走偏。
    let cells: [(&str, &str, &str); 11] = [
        // 1..4：改前的链式启发折成 False，值语义折成 True＝区分格。
        (
            "括号比较作左操作数（665 症状形）",
            "print((-5 == 0) > -16)",
            "True",
        ),
        ("内层为假＋外层 < 5", "print((2 < 1) < 5)", "True"),
        ("内层为假＋外层 < 1", "print((3 < 2) < 1)", "True"),
        ("内层为假＋外层 != 1", "print((2 < 1) != 1)", "True"),
        // 5..8：两种折法给同一个值，当对照组（这一臂出别的毛病会先红在这里）。
        (
            "内层为真＋外层 > 0（两折法同值）",
            "print(((3 > 2) == 1) > 0)",
            "True",
        ),
        (
            "内层为假＋外层 == 1（两折法同值）",
            "print((2 < 1) == 1)",
            "False",
        ),
        (
            "内层为真＋外层 == 1（两折法同值）",
            "print((0 == 0) == 1)",
            "True",
        ),
        (
            "不带括号的比较串（parser 已折成 &&，启发式打不到）",
            "print(1 < 2 == 1)",
            "False",
        ),
        // 9..11：把 `apply` 闭包里 `>=`／`<=` 那两行也放进变异射程（8 格只用到
        // `<`、`>`、`==`、`!=` 四行）。
        (
            "内层 <= 为假＋外层 >= 0（区分格，钉 >= 那一行）",
            "print((2 <= 1) >= 0)",
            "True",
        ),
        (
            "内层 <= 靠相等为真＋外层 > 0（两折法同值，钉 <= 那一行）",
            "print((1 <= 1) > 0)",
            "True",
        ),
        (
            "内层 >= 为假＋外层 <= 0（区分格，钉 >= 与 <= 两行）",
            "print((1 >= 2) <= 0)",
            "True",
        ),
    ];
    // 每格单独降一趟 MIR。合在一起降时，没折的那几条会换一条发射路径，
    // "第 i 个 println 调用"与"第 i 格"就对不上号——批次 10037 的变异矩阵里
    // M4（把 `==` 从算子表里摘掉）实拿到"六条 True＋五条读不到"的清单，
    // 逐格落点全是猜的。逐格降低一趟 ⇒ 一格对一个调用。
    let got: Vec<(&str, String)> = cells
        .iter()
        .map(|(label, line, _)| {
            let mirs = lower_all(line);
            let f = mir(&mirs, "main");
            let operand = f
                .stmts
                .iter()
                .find_map(|s| match s {
                    MirStmt::VoidCall { func, args } if func.starts_with("println") => {
                        args.first().copied()
                    }
                    _ => None,
                });
            let shape = match operand.and_then(|id| f.exprs.get(&id)) {
                Some(MirExpr::StringLit(v)) => v.clone(),
                Some(other) => format!("{other:?}"),
                None => "<没有 println 调用>".to_string(),
            };
            (*label, shape)
        })
        .collect();
    let want: Vec<(&str, String)> = cells
        .iter()
        .map(|(label, _, v)| (*label, (*v).to_string()))
        .collect();

    assert_eq!(
        want, got,
        "嵌套比较的折叠值必须按值语义下发（批次 665）——前四格读回 False＝链式启发式又回来了；\
         读回非字符串形状＝这一臂没折，走了运行期"
    );
}

/// 批次 657（`d7f9a8fd`）：方法段的 MIR 名是 `Class::method`，它**不在**
/// `py_mangled_to_module`（那张表只收定义）里，所以 `module_renames_for` 早先给方法
/// 建不出改写表 ⇒ 方法体里的模块私有名留在裸名：函数 `make()` 落到 `make_0`
/// （链接期 `_make` 未定义），类 `Helper()` 落到 `zeta_platform_obj`（＝657 记的
/// "错误内存布局 ⇒ 崩在 `map_keys`"）。657 的兜底臂先把改写类名 `m657a__Holder`
/// 去掉模块前缀还原成裸名 `Holder`，再按裸名查所属模块、用该模块的自有名建表。
///
/// 三组夹具（各一次 `lower_multi`）：
/// - 主夹具：一个模块，方法体里分别引用模块私有函数（`get`）、另一个类（`build`）、
///   第三个类（`fresh`）、**自己所在的类**（`again`＝自有名表里含类名自身这一项，
///   变异 M3 的靶；本批第一版把这一形写成了 `fresh` 引用 `Node`，M3 实跑 61 条一字不变＝
///   阴性读数后才改正，见 roadmap 的教训段）；
///   格 8＝`main` 里五个带 `::` 的调用点符号串（`m657a__Holder::get,build,fresh` ＋
///   接收者方法 `m657a__Helper::val`／`m657a__Node::tag`）。
/// - 歧义夹具：两个模块各有一个同名类 `Shared` ⇒ `hits.len() == 1` 守卫让两个方法体
///   **都**留在裸名 `extra_0`＝未修的现状锁（将来改成按调用点模块消歧时这两格要同步改，
///   见 backlog #20005 余项）；
/// - 对照夹具：模块级函数 `top` 走的是另一条臂（`func_name` 直接命中定义表），
///   本批的变异都打不到它 ⇒ 只算防放松。
///
/// 端到端未修（记在 #20005 余项，本条不锁）：主夹具在 HEAD 上 AOT 仍链接失败，
/// `_make` 由裸名副本段 `_get`／`_get_inst_i64` 引用——那些副本的 `func_name` 是裸名
/// `get`（不含 `::`），兜底臂对它同样建不出表。
#[test]
fn method_bodies_recover_the_module_rename_table_from_the_mangled_class_name() {
    const M657A: &str = r#"def make():
    return 7


class Helper:
    def val(self):
        return 3


class Node:
    def tag(self):
        return 11


class Holder:
    def get(self):
        return make()

    def build(self):
        return Helper()

    def fresh(self):
        return Node()

    def again(self):
        return Holder()
"#;
    const MAIN_FX: &str = r#"from m657a import Holder

h = Holder()
print(h.get())
b = h.build()
print(b.val())
n = h.fresh()
print(n.tag())
"#;
    // CPython 对同一段（`m657a.py` ＋ `main.py`，批次 10038 实拍）：7 / 3 / 11。
    const M657C: &str = r#"def extra():
    return 5


class Shared:
    def take(self):
        return extra()
"#;
    const M657D: &str = r#"def extra():
    return 6


class Shared:
    def take(self):
        return extra()
"#;
    const MAIN_AMB: &str = r#"from m657c import Shared as Sc
from m657d import Shared as Sd

a = Sc()
b = Sd()
print(a.take())
print(b.take())
"#;
    const M657E: &str = r#"def make():
    return 7


def top():
    return make()
"#;
    const MAIN_CTL: &str = r#"from m657e import top

print(top())
"#;

    let fx = lower_multi(&[("m657a.z", M657A), ("main.z", MAIN_FX)], "main.z");
    let amb = lower_multi(
        &[("m657c.z", M657C), ("m657d.z", M657D), ("main2.z", MAIN_AMB)],
        "main2.z",
    );
    let ctl = lower_multi(&[("m657e.z", M657E), ("main3.z", MAIN_CTL)], "main3.z");

    // 段内第一个 `Call` 的被调符号名（`get`／`build`／`fresh`／`take`／`top` 都只有一条调用）。
    let call_of = |mirs: &[Mir], item: &str| -> String {
        let m = mir(mirs, item);
        m.stmts
            .iter()
            .find_map(|s| match s {
                MirStmt::Call { func, .. } => Some(func.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{item} 段内没有 Call（实得 stmts {} 条）", m.stmts.len()))
    };
    // 那条 `Call` 的目的槽类型：改写目标错了 ⇒ 这里跟着变（`zeta_platform_obj` 那臂
    // 的目的槽是 I64／没有类型），657 的内存布局症状在 MIR 面上的读数就在这一格。
    let dest_ty_of = |mirs: &[Mir], item: &str| -> String {
        let m = mir(mirs, item);
        let dest = m
            .stmts
            .iter()
            .find_map(|s| match s {
                MirStmt::Call { dest, .. } => Some(*dest),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{item} 段内没有 Call（取不到目的槽）"));
        match m.type_map.get(&dest) {
            Some(t) => format!("{t:?}"),
            None => format!("<目的槽 {dest} 没有类型>"),
        }
    };
    // 调用点侧：只收带 `::` 的被调符号（方法调用点），运行时内部调用（`zeta_*`、
    // `println_*`）不算，免得夹具形状一动就串格。
    let scoped_calls = |mirs: &[Mir], item: &str| -> String {
        call_symbols(mirs.iter().find(|m| m.name.as_deref() == Some(item)).expect("没有 main 段"))
            .into_iter()
            .filter(|f| f.contains("::"))
            .collect::<Vec<_>>()
            .join(",")
    };

    let got: Vec<(&str, String)> = vec![
        (
            "Holder::get 体内模块私有函数的改写目标",
            call_of(&fx, "m657a__Holder::get"),
        ),
        (
            "Holder::build 体内另一个类的构造目标",
            call_of(&fx, "m657a__Holder::build"),
        ),
        (
            "Holder::build 目的槽类型（657 的内存布局症状在 MIR 面上的读数）",
            dest_ty_of(&fx, "m657a__Holder::build"),
        ),
        (
            "Holder::fresh 体内引用第三个类（Node）的构造目标",
            call_of(&fx, "m657a__Holder::fresh"),
        ),
        (
            "Holder::fresh 目的槽类型",
            dest_ty_of(&fx, "m657a__Holder::fresh"),
        ),
        (
            "Holder::again 体内引用自己所在类的构造目标（变异 M3 的靶）",
            call_of(&fx, "m657a__Holder::again"),
        ),
        (
            "Holder::again 目的槽类型",
            dest_ty_of(&fx, "m657a__Holder::again"),
        ),
        (
            "main 里三个方法调用点＋两个接收者方法",
            scoped_calls(&fx, "main"),
        ),
        (
            "歧义夹具：m657c__Shared::take 体内调用（未修现状锁＝裸名）",
            call_of(&amb, "m657c__Shared::take"),
        ),
        (
            "歧义夹具：m657d__Shared::take 体内调用（未修现状锁＝裸名）",
            call_of(&amb, "m657d__Shared::take"),
        ),
        (
            "对照：模块级函数 top 走定义命中臂",
            call_of(&ctl, "m657e__top"),
        ),
    ];
    let want: Vec<(&str, String)> = vec![
        (
            "Holder::get 体内模块私有函数的改写目标",
            "m657a__make".to_string(),
        ),
        (
            "Holder::build 体内另一个类的构造目标",
            "m657a__Helper".to_string(),
        ),
        (
            "Holder::build 目的槽类型（657 的内存布局症状在 MIR 面上的读数）",
            r#"Named("m657a__Helper", [])"#.to_string(),
        ),
        (
            "Holder::fresh 体内引用第三个类（Node）的构造目标",
            "m657a__Node".to_string(),
        ),
        (
            "Holder::fresh 目的槽类型",
            r#"Named("m657a__Node", [])"#.to_string(),
        ),
        (
            "Holder::again 体内引用自己所在类的构造目标（变异 M3 的靶）",
            "m657a__Holder".to_string(),
        ),
        (
            "Holder::again 目的槽类型",
            r#"Named("m657a__Holder", [])"#.to_string(),
        ),
        (
            "main 里三个方法调用点＋两个接收者方法",
            "m657a__Holder::get,m657a__Holder::build,m657a__Helper::val,m657a__Holder::fresh,m657a__Node::tag"
                .to_string(),
        ),
        (
            "歧义夹具：m657c__Shared::take 体内调用（未修现状锁＝裸名）",
            "extra_0".to_string(),
        ),
        (
            "歧义夹具：m657d__Shared::take 体内调用（未修现状锁＝裸名）",
            "extra_0".to_string(),
        ),
        (
            "对照：模块级函数 top 走定义命中臂",
            "m657e__make".to_string(),
        ),
    ];

    assert_eq!(
        want, got,
        "方法段必须按改写类名还原裸类名、查到所属模块后建改写表（批次 657）——\
         第 1 格读回 `make_0`＝改写表又空了；第 2/3、4/5、6/7 格读回 `zeta_platform_obj`／\
         非 `Named` 类型＝657 那个错误内存布局的形（第 6/7 格只对\"跳过类名自身\"这种坏法敏感）；\
         第 9/10 格本该留在裸名 `extra_0`（两模块同名类的歧义未修，读到改写名＝守卫被放宽\
         而没做消歧，先核对修法再改期望）；第 11 格走另一条臂，红了说明整个改写表机制坏了"
    );
}

#[test]
// 批次 169（提交 `8383988f`）：注解写 `-> dict`、函数体却 `return json.loads(...)` 时，
// 登记的返回类型必须改成 `PyJson`。留着 `map`＝调用点按 map 原语去读 JSON 句柄
// （原症状：`stats.get(k, d)` 把 JSON 标签当容量读，运行期死转）。
// 站点＝`src/middle/resolver/resolver.rs:5196` 的 `is_dict_ret` 判定、
// `:5205` 的改写、以及 `:4524` 的 `returns_json_loads` 三趟走查
// （直接 `return`、`AstNode::Block` 递归、`If` 的 then／else 两侧）。
// 八格分工：1＝体内直接 return；2/3＝调用点目的槽＋赋值左侧（消费面）；
// 4＝`If` 的 then 侧；5＝`If` 的 else 侧；6＝try 块内（`Block` 那一支）；
// 7＝接收者不是 `json` 的同名方法（守卫必须只认 `json.loads`；本格的"真值"未修，
// 现按 `I64` 锁现状，见余项）；8＝返回字典字面量那格必须仍是 `map`（改写过宽会顶成 PyJson）。
fn annotated_dict_return_becomes_pyjson_when_the_body_returns_json_loads() {
    let src = r#"
import json


def load(txt) -> dict:
    return json.loads(txt)


def branchy(flag, txt) -> dict:
    if flag:
        return json.loads(txt)
    return {"a": 1}


def inelse(flag, txt) -> dict:
    if flag:
        return {"b": 2}
    else:
        return json.loads(txt)


def exp_then(flag, txt) -> dict:
    if flag:
        return json.loads(txt)
    else:
        return {"e": 5}


def try_ret(txt) -> dict:
    try:
        return json.loads(txt)
    except:
        return {"c": 3}


def other(obj, txt) -> dict:
    return obj.loads(txt)


d = load("{}")
b = branchy(True, "{}")
i = inelse(False, "{}")
e = exp_then(True, "{}")
t = try_ret("{}")
o = other("x", "{}")
"#;
    let mirs = lower_all(src);

    // 被调名含 `needle` 的第一个 `Call` 的目的槽（`If`/`For`/`While` 的块内也要找）。
    fn find_call_dest(stmts: &[MirStmt], needle: &str) -> Option<u32> {
        for s in stmts {
            match s {
                MirStmt::Call { func, dest, .. } if func.contains(needle) => return Some(*dest),
                MirStmt::If { then, else_, .. } => {
                    if let Some(d) = find_call_dest(then, needle) {
                        return Some(d);
                    }
                    if let Some(d) = find_call_dest(else_, needle) {
                        return Some(d);
                    }
                }
                MirStmt::For { body, else_body, .. } | MirStmt::While { body, else_body, .. } => {
                    if let Some(d) = find_call_dest(body, needle) {
                        return Some(d);
                    }
                    if let Some(d) = find_call_dest(else_body, needle) {
                        return Some(d);
                    }
                }
                _ => {}
            }
        }
        None
    }

    let dest_ty = |item: &str, needle: &str| -> String {
        let m = mir(&mirs, item);
        let dest = find_call_dest(&m.stmts, needle).unwrap_or_else(|| {
            panic!("{item} 段内没有被调名含 {needle} 的调用（实得 {:?}）", call_symbols(m))
        });
        match m.type_map.get(&dest) {
            Some(t) => format!("{t:?}"),
            None => panic!("{item} 的目的槽 {dest} 没有类型（type_map {} 项）", m.type_map.len()),
        }
    };

    // `d = load(...)` 左侧槽的类型：先按目的槽找到那条赋值，再读左侧。
    let assigned_lhs_ty = || -> String {
        let m = mir(&mirs, "main");
        let dest = m
            .stmts
            .iter()
            .find_map(|s| match s {
                MirStmt::Call { func, dest, .. } if func.starts_with("load") => Some(*dest),
                _ => None,
            })
            .unwrap_or_else(|| panic!("main 段没有 load 调用（实得 {:?}）", call_symbols(m)));
        let lhs = m
            .stmts
            .iter()
            .find_map(|s| match s {
                MirStmt::Assign { lhs, rhs, .. } if *rhs == dest => Some(*lhs),
                _ => None,
            })
            .unwrap_or_else(|| panic!("main 段没有把目的槽 {dest} 赋给左侧（顶层赋值 {} 条）",
                                      m.stmts.iter().filter(|s| matches!(s, MirStmt::Assign { .. })).count()));
        match m.type_map.get(&lhs) {
            Some(t) => format!("{t:?}"),
            None => panic!("main 的左侧槽 {lhs} 没有类型（type_map {} 项）", m.type_map.len()),
        }
    };

    let got = vec![
        ("load：体内 json.loads 的目的槽", dest_ty("load", "json_loads")),
        ("main：load(...) 调用点的目的槽", dest_ty("main", "load")),
        ("main：d = load(...) 的左侧槽", assigned_lhs_ty()),
        ("branchy：If 的 then 侧 json.loads 目的槽", dest_ty("branchy", "json_loads")),
        ("inelse：If 的 else 侧 json.loads 目的槽", dest_ty("inelse", "json_loads")),
        ("try_ret：try 块内 json.loads 目的槽", dest_ty("try_ret", "json_loads")),
        ("other：接收者非 json 的 loads 目的槽（未修现状锁）", dest_ty("other", "loads")),
        ("branchy：字典字面量那格仍是 map", {
            let m = mir(&mirs, "branchy");
            let mut lits = m
                .type_map
                .iter()
                .filter(|(_, t)| matches!(t, Type::Named(n, _) if n == "map"))
                .map(|(_, t)| format!("{t:?}"))
                .collect::<Vec<_>>();
            lits.sort();
            lits.join(",")
        }),
        ("main：branchy(...) 调用点的目的槽", dest_ty("main", "branchy")),
        (
            "main：inelse(...) 调用点的目的槽（显式 else 未修现状锁）",
            dest_ty("main", "inelse"),
        ),
        (
            "main：exp_then(...) 调用点的目的槽（显式 else 未修现状锁，另一侧）",
            dest_ty("main", "exp_then"),
        ),
        (
            "main：try_ret(...) 调用点的目的槽（try 侧未修现状锁）",
            dest_ty("main", "try_ret"),
        ),
        (
            "main：other(...) 调用点的目的槽（接收者守卫负向锁）",
            dest_ty("main", "other"),
        ),
    ];

    let want = vec![
        (
            "load：体内 json.loads 的目的槽",
            "Named(\"PyJson\", [])".to_string(),
        ),
        (
            "main：load(...) 调用点的目的槽",
            "Named(\"PyJson\", [])".to_string(),
        ),
        (
            "main：d = load(...) 的左侧槽",
            "Named(\"PyJson\", [])".to_string(),
        ),
        (
            "branchy：If 的 then 侧 json.loads 目的槽",
            "Named(\"PyJson\", [])".to_string(),
        ),
        (
            "inelse：If 的 else 侧 json.loads 目的槽",
            "Named(\"PyJson\", [])".to_string(),
        ),
        (
            "try_ret：try 块内 json.loads 目的槽",
            "Named(\"PyJson\", [])".to_string(),
        ),
        (
            "other：接收者非 json 的 loads 目的槽（未修现状锁）",
            "I64".to_string(),
        ),
        (
            "branchy：字典字面量那格仍是 map",
            "Named(\"map\", [Str, I64])".to_string(),
        ),
        (
            "main：branchy(...) 调用点的目的槽",
            "Named(\"PyJson\", [])".to_string(),
        ),
        (
            "main：inelse(...) 调用点的目的槽（显式 else 未修现状锁）",
            "Named(\"map\", [])".to_string(),
        ),
        (
            "main：exp_then(...) 调用点的目的槽（显式 else 未修现状锁，另一侧）",
            "Named(\"map\", [])".to_string(),
        ),
        (
            "main：try_ret(...) 调用点的目的槽（try 侧未修现状锁）",
            "Named(\"map\", [])".to_string(),
        ),
        (
            "main：other(...) 调用点的目的槽（接收者守卫负向锁）",
            "Named(\"map\", [])".to_string(),
        ),
    ];

    assert_eq!(
        want, got,
        "注解 `-> dict` 而体里 `return json.loads(...)` 的函数必须登记成 `PyJson`（批次 169）——\
         第 2/3/9 格是消费面，红了说明返回类型没传到调用点（第 9 格靠 `if` 的 then 侧递归，\
         第 2 格靠顶层 `return`）；第 10/11/12 格是未修现状锁：写了显式 `else:` 的函数\
         两侧都认不出（`json.loads` 放 then 侧与放 else 侧实测同样得到 `map`），`try:` 块内\
         也认不出，修好后这三格要改成 `PyJson`；第 13 格本该留在 `map`（接收者不是 `json`，\
         改写变宽＝把别的 `.loads` 也顶成 `PyJson`，这条是接收者守卫的负向锁）；\
         第 8 格读回空串＝字典字面量的 `map` 型被改写覆盖，169 的改写过头了；\
         第 7 格本该留在 `I64`（`other` 体内那次 `.loads` 的接收者是无型形参）"
    );
}

/// 批次 287（代码 `d8f69183`，站点 `src/frontend/parser/stmt.rs` 的 with 降形：
/// 终结符改写 `rewrite_with_exits`（现 :1308-1364）与发射器 `zeta_with_exit_stmt`
/// （现 :1272））：`with lock: return v` 死锁 + 静默错值。
/// 症状（记录原文）：desugar 只在 body「能走到尾」时追加 `__exit__` ⇒ 体内 `return`
/// 提前离开时锁永不释放（`_ranked_fetch_sources → py_threading_lock_acquire →
/// __psynch_mutexwait`，drv 挂住 rc=124）；同一形态还有第二坑（`top_level.rs` 把以
/// `return` 结尾的 Block 提升成 ret_expr）⇒ 返回值被吞、恒返 0。
/// 修法＝把体内 `return v` 就地改写成 `{ __with_ret_N = v; try_end; __exit__();
/// return __with_ret_N }`；`break`/`continue` 仅当绑定到 with **外层**循环时前置
/// 释放（降入循环体后 `at_loop_depth` 复位，防"体内 break 提前放锁"）。
/// 期望值来源：在册夹具 `tests/python_style/t287_with_lock_return.z` 的
/// `// expect: 42 / 42 / 9`（运行期真值由该夹具承担）＋本批 `--dump-mir` 实拍
/// （`/tmp/b10040/mir_v2.txt`、`/tmp/b10040/mir_head.txt`）与进程内逐函数事件轨迹
/// （HEAD 态 14 格＝`/tmp/b10040/trace_head.txt`；改前形状＝`arm_M1_return_arm_gone.txt`
/// 的 left 侧，那一臂就是把 `return` 那支改写撤掉＝287 改前）。槽号按首次出现顺序
/// 规范化（`s0`、`s1`…），所以下面每格读的是**事件次序与配对关系**，不是全局槽号。
/// 轨迹词表：`acq`＝加锁，`tend`＝弹 try 帧，`rel`＝释放锁，`raise`＝重抛，
/// `wsK<-sL`＝把返回槽 sL 写进 sK，`ret sK`＝返回 sK，`brk`/`cont`＝跳出／继续，
/// `if{…}else{…}`／`for{…}esle{…}`／`whl{…}esle{…}`＝分支与循环（`esle` 是 for/while
/// 的 else 支）。
/// 覆盖面分工（本批 14 臂变异矩阵实测，逐臂红点集见 roadmap 批次 10040）：
/// ① 撤 `return` 那支的整段改写（＝改前形状）、② 单撤那支里的释放发射、③ 单撤那支里
///   的弹帧——三臂红**同一组 9 格**（带 return 的九条轨迹），其余 5 格一字不动 ⇒ 三臂
///   算一条覆盖；④ `brk` 那支红 2 格、⑤ `cont` 那支红 1 格、⑥ If 整支不递归红 3 格、
///   ⑦ 只断 If 的 else_ 半支红 1 格、⑧⑨⑩ IfLet／Block／Loop 各自不递归分别红
///   `ret_in_iflet`／`ret_in_nested_with`／`ret_in_loop`（三格互不重叠）、
///   ⑪⑫ "降入循环体复位"那两处（for 体、while 体）各红 1 格，红的正是
///   "体内 break/continue 不该放锁"那两格、⑬⑭ for 的 else 支与 while 的 else 支
///   各自不递归分别红 `ret_in_for_else`／`break_in_while_else`。
/// 14 臂全部有红点：无阴性臂、无空跑格（每格至少被一臂打红）。
/// 未变异的一臂＝287 记录里的第二坑（`top_level.rs` 不再把以 `return` 结尾的 Block
/// 提升成 ret_expr），站点在本车道的在制文件里 ⇒ 第 6 格 `ret_after_assign` 对那一臂
/// 只算现状锁。
/// 与在册批次 414 那条（`with_body_exception_path_releases_lock_in_both_try_branches`）
/// 的分工：414 那条只走 `raise` 出口，本条走的是它头注里点名的未覆盖边界——
/// `return`／`break`／`continue` 三条提前退出边；两者互不备份。
/// 轨迹里的 `else{ tend rel raise }` 半段来自 414 的 handler 分支，撤那一臂本条也会
/// 跟着红——那一格算互备，不算本条的独立覆盖。
#[test]
fn with_body_terminators_release_the_lock_before_leaving() {
    use std::collections::{HashMap, HashSet};

    const REL: &str = "rel";
    const ACQ: &str = "acq";
    const TEND: &str = "tend";
    const RAISE: &str = "raise";

    /// 槽号规范化器：按首次出现顺序发号，读数与全局槽号解耦。
    struct Slots {
        map: HashMap<u32, String>,
        next: usize,
    }
    impl Slots {
        fn id(&mut self, v: u32) -> String {
            if let Some(l) = self.map.get(&v) {
                return l.clone();
            }
            let l = format!("s{}", self.next);
            self.next += 1;
            self.map.insert(v, l.clone());
            l
        }
    }

    /// 函数体内所有 `Return` 返回的槽（只有这些槽的写入才进轨迹）。
    fn ret_slots(stmts: &[MirStmt], out: &mut HashSet<u32>) {
        for s in stmts {
            match s {
                MirStmt::Return { val } => {
                    out.insert(*val);
                }
                MirStmt::If { then, else_, .. } => {
                    ret_slots(then, out);
                    ret_slots(else_, out);
                }
                MirStmt::For { body, else_body, .. } | MirStmt::While { body, else_body, .. } => {
                    ret_slots(body, out);
                    ret_slots(else_body, out);
                }
                _ => {}
            }
        }
    }

    /// 按序线性化成事件轨迹（深度优先，分支用花括号标出）。
    fn trace(stmts: &[MirStmt], out: &mut Vec<String>, slots: &mut Slots, rets: &HashSet<u32>) {
        for s in stmts {
            match s {
                MirStmt::Call { func, .. } | MirStmt::VoidCall { func, .. } => {
                    let tok = if func.contains("lock_release") {
                        REL
                    } else if func.contains("lock_acquire") {
                        ACQ
                    } else if func.contains("try_end") {
                        TEND
                    } else if func.contains("zeta_raise") {
                        RAISE
                    } else {
                        continue;
                    };
                    out.push(tok.to_string());
                }
                MirStmt::Assign { lhs, rhs } => {
                    if rets.contains(lhs) {
                        out.push(format!("w{}<-{}", slots.id(*lhs), slots.id(*rhs)));
                    }
                }
                MirStmt::Return { val } => out.push(format!("ret{}", slots.id(*val))),
                MirStmt::Break => out.push("brk".to_string()),
                MirStmt::Continue => out.push("cont".to_string()),
                MirStmt::If { then, else_, .. } => {
                    out.push("if{".to_string());
                    trace(then, out, slots, rets);
                    out.push("}else{".to_string());
                    trace(else_, out, slots, rets);
                    out.push("}".to_string());
                }
                MirStmt::For { body, else_body, .. } => {
                    out.push("for{".to_string());
                    trace(body, out, slots, rets);
                    out.push("}esle{".to_string());
                    trace(else_body, out, slots, rets);
                    out.push("}".to_string());
                }
                MirStmt::While { body, else_body, .. } => {
                    out.push("whl{".to_string());
                    trace(body, out, slots, rets);
                    out.push("}esle{".to_string());
                    trace(else_body, out, slots, rets);
                    out.push("}".to_string());
                }
                _ => {}
            }
        }
    }

    let mirs = lower_all(
        r#"import threading

L = threading.Lock()
M = threading.Lock()
flag = True

def ret_top():
    with L:
        return 7

def ret_in_if(c: bool):
    with L:
        if c:
            return 1
    return 2

def ret_in_elif(c: bool, d: bool):
    with L:
        if c:
            return 21
        elif d:
            return 22
        else:
            return 23
    return 24

def ret_in_while():
    with L:
        while flag:
            return 3
    return 4

def ret_in_for_else():
    with L:
        for i in range(2):
            pass
        else:
            return 5
    return 6

def ret_after_assign():
    x = 0
    with L:
        x = 8
        return x + 1

def ret_in_nested_with():
    with L:
        with M:
            return 11

def break_outer():
    for i in range(3):
        with L:
            break

def break_inner():
    with L:
        for i in range(3):
            break

def cont_outer():
    for i in range(3):
        with L:
            continue

def cont_inner():
    with L:
        while flag:
            continue

def ret_in_loop():
    with L:
        loop:
            return 31

def ret_in_iflet(o: Option<i64>) -> i64:
    with L:
        if let Some(v) = o:
            return 32
    return 33

def break_in_while_else():
    for i in range(3):
        with L:
            while flag:
                pass
            else:
                break
"#,
    );

    let traced = |name: &str| -> String {
        let f = mir(&mirs, name);
        let mut rets = HashSet::new();
        ret_slots(&f.stmts, &mut rets);
        let mut slots = Slots { map: HashMap::new(), next: 0 };
        let mut v = Vec::new();
        trace(&f.stmts, &mut v, &mut slots, &rets);
        v.join(" ")
    };

    let got = vec![
        ("ret_top：return 边的释放序", traced("ret_top")),
        ("ret_in_if：If 的 then 支里 return", traced("ret_in_if")),
        ("ret_in_elif：elif 链每一支的 return", traced("ret_in_elif")),
        ("ret_in_while：whl 体里 return", traced("ret_in_while")),
        ("ret_in_for_else：for 的 else 支里 return", traced("ret_in_for_else")),
        ("ret_after_assign：先赋值再 return（287 第二坑的形状）", traced("ret_after_assign")),
        ("ret_in_nested_with：嵌套 with 两层各放一次", traced("ret_in_nested_with")),
        ("break_outer：外层循环的 break 边要放锁", traced("break_outer")),
        ("break_inner：with 体内循环的 break 不放锁", traced("break_inner")),
        ("cont_outer：外层循环的 continue 边要放锁", traced("cont_outer")),
        ("cont_inner：with 体内循环的 continue 不放锁", traced("cont_inner")),
        ("ret_in_loop：loop 体里 return", traced("ret_in_loop")),
        ("ret_in_iflet：if let 支里 return", traced("ret_in_iflet")),
        ("break_in_while_else：whl 的 else 支里 break", traced("break_in_while_else")),
    ];
    let want = vec![
        ("ret_top：return 边的释放序", "acq if{ ws0<-s1 tend rel rets0 }else{ tend rel raise } ws2<-s3 rets2".to_string()),
        ("ret_in_if：If 的 then 支里 return", "acq if{ if{ ws0<-s1 tend rel rets0 }else{ } tend rel }else{ tend rel raise } rets2".to_string()),
        ("ret_in_elif：elif 链每一支的 return", "acq if{ if{ ws0<-s1 tend rel rets0 }else{ if{ ws0<-s2 tend rel rets0 }else{ ws0<-s3 tend rel rets0 } } }else{ tend rel raise } rets4".to_string()),
        ("ret_in_while：whl 体里 return", "acq if{ whl{ ws0<-s1 tend rel rets0 }esle{ } tend rel }else{ tend rel raise } rets2".to_string()),
        ("ret_in_for_else：for 的 else 支里 return", "acq if{ for{ }esle{ ws0<-s1 tend rel rets0 } tend rel }else{ tend rel raise } rets2".to_string()),
        ("ret_after_assign：先赋值再 return（287 第二坑的形状）", "acq if{ ws0<-s1 tend rel rets0 }else{ tend rel raise } ws2<-s3 rets2".to_string()),
        ("ret_in_nested_with：嵌套 with 两层各放一次", "acq if{ acq if{ tend rel ws0<-s1 tend rel rets0 }else{ tend rel raise } tend rel }else{ tend rel raise } ws2<-s3 rets2".to_string()),
        ("break_outer：外层循环的 break 边要放锁", "for{ acq if{ tend rel brk }else{ tend rel raise } }esle{ } rets0".to_string()),
        ("break_inner：with 体内循环的 break 不放锁", "acq if{ for{ brk }esle{ } tend rel }else{ tend rel raise } ws0<-s1 rets0".to_string()),
        ("cont_outer：外层循环的 continue 边要放锁", "for{ acq if{ tend rel cont }else{ tend rel raise } }esle{ } rets0".to_string()),
        ("cont_inner：with 体内循环的 continue 不放锁", "acq if{ whl{ cont }esle{ } tend rel }else{ tend rel raise } ws0<-s1 rets0".to_string()),
        ("ret_in_loop：loop 体里 return", "acq if{ whl{ ws0<-s1 tend rel rets0 }esle{ } tend rel }else{ tend rel raise } ws2<-s3 rets2".to_string()),
        ("ret_in_iflet：if let 支里 return", "acq if{ ws0<-s1 tend rel rets0 tend rel }else{ tend rel raise } rets2".to_string()),
        ("break_in_while_else：whl 的 else 支里 break", "for{ acq if{ whl{ }esle{ tend rel brk } tend rel }else{ tend rel raise } }esle{ } rets0".to_string()),
    ];
    assert_eq!(
        got, want,
        "with 体里的提前退出（return／绑到外层循环的 break／continue）每条边都要在离开前\
         弹帧＋放锁（改前＝desugar 只在体能走到尾时补 `__exit__`，这些边上锁永不释放 ⇒ 二次\
         调用挂死）；而 with 体内自己循环的 break/continue **不该**提前放锁（287 记录的 lock5 \
         探针：`locked()==1`）。上面每格是一个函数的规范化事件轨迹（槽号按首次出现发号）。"
    );
}

/// 批次 325（代码 `4f3e3833`，站点 `src/frontend/parser/pattern.rs` 的
/// `parse_char_lit`（现 :238-272）与 `parse_range_pattern`（现 :275-289））：
/// 范围模式族整族失效——两层缺陷叠着，所以"能解析"离"能对"只差一行读数。
/// 症状（记录原文）：① `parse_range_pattern` 的两端只吃 `parse_lit` ⇒
/// 字符字面量 `'a'..='z'` 直接解析失败，而解析失败的后果不是报错而是
/// [W1002]「第一个打不开的模式起、文件余部整段丢掉」（退出码仍为 0）；
/// ② `inclusive` 此前被 `inclusive: _` 丢弃 ⇒ `1..10` 与 `1..=10` 同义；
/// ③ 模式位的字符是**码点整数**（`'a'`＝97），不是字符串——单引号在本语言里
/// 还是普通字符串定界符（`s.split(',')` 依赖），所以只在模式位改语义。
/// 本条只钉得住 ①③（解析位＋转义表位）与 ② 的 inclusive 位；同批的下型两处
/// （`>=`/`<=` 的 Call dest 从没进 `exprs` ⇒ 静默回 i64 0、`x @ …` 多一层 Var）
/// 站点在 `gen.rs`（主线在重构该文件，本批不取），那两处的读数在本条里是现状锁。
/// 期望值来源：在册夹具 `tests/python_style/t306_range_pattern_guard.z` 的
/// `// expect: 1/2/3/0/111/222/-1/111/-1/7/-1`（运行期真值由该夹具承担，本批
/// AOT 实测同形）＋`/tmp/b10041/mir_head.txt` 的 `--dump-mir` 实拍＋变异矩阵
/// 左侧 `/tmp/b10041/arm_*.txt`（HEAD 态）。Rust 方言形状 ⇒ CPython 侧不适用。
/// 轨迹词表：`pi sK`＝形参入槽，`op(a,b) -> dK`＝`MirStmt::Call`（`op` 为被调符号，
/// 实参若已在 `exprs` 里是整数字面量就直接写字面值，否则按首次出现发槽号），
/// `sK <- v`＝赋值，`if(c){…}else{…}`＝分支，`ret sK`＝返回，
/// `<没进 MIR>`＝该函数根本没降出来（＝解析在该函数之前被截断）。
/// 四臂分工见台账：`inclusive` 位只改比较符不改函数清单，三枚端点位改函数清单。
/// `esc_start` 那格（转义端点在起点＋整数终点，且排在 `end_char` 之前）是用来把
/// "转义表位"与"终点字符位"分开的：少了这格时前者的坏格集是后者的真子集
/// （任何打掉转义表的输入形状也打掉终点字符位），两臂互相当不了备份。
#[test]
fn range_pattern_endpoints_and_inclusivity_reach_the_guard() {
    let (mirs, remaining) = lower_all_allowing_truncation(
        r#"
fn start_char(c: i64) -> i64 {
    match c {
        'a'..=5 => 1,
        _ => 0
    }
}
fn esc_start(c: i64) -> i64 {
    match c {
        '\n'..=5 => 4,
        _ => 0
    }
}
fn end_char(c: i64) -> i64 {
    match c {
        1..='z' => 2,
        _ => 0
    }
}
fn escapes(c: i64) -> i64 {
    match c {
        '\n'..='\t' => 7,
        _ => 0
    }
}
fn incl_int(x: i64) -> i64 {
    match x {
        1..=10 => 111,
        _ => -1
    }
}
fn excl_int(x: i64) -> i64 {
    match x {
        1..10 => 111,
        _ => -1
    }
}
fn char_letters(c: i64) -> i64 {
    match c {
        'a'..='z' => 1,
        '0'..='9' => 3,
        _ => 0
    }
}
fn binder(x: i64) -> i64 {
    match x {
        q @ 1..=10 => q,
        _ => -1
    }
}
fn two_arms(x: i64) -> i64 {
    match x {
        1..=3 => 10,
        8..=9 => 20,
        _ => 0
    }
}
"#,
    );

    #[derive(Default)]
    struct Slots {
        next: u32,
        ids: std::collections::HashMap<u32, u32>,
    }
    impl Slots {
        fn tok(&mut self, id: u32, m: &Mir) -> String {
            if let Some(MirExpr::IntLit(v)) = m.exprs.get(&id) {
                return format!("{v}");
            }
            if let Some(&n) = self.ids.get(&id) {
                return format!("s{n}");
            }
            let n = self.next;
            self.next += 1;
            self.ids.insert(id, n);
            format!("s{n}")
        }
    }

    fn trace(m: &Mir, stmts: &[MirStmt], slots: &mut Slots, out: &mut Vec<String>) {
        for s in stmts {
            match s {
                MirStmt::ParamInit { param_id, .. } => {
                    let t = slots.tok(*param_id, m);
                    out.push(format!("pi {t}"));
                }
                MirStmt::Call { func, args, dest, .. } => {
                    let mut a: Vec<String> = Vec::with_capacity(args.len());
                    for id in args {
                        a.push(slots.tok(*id, m));
                    }
                    let d = slots.tok(*dest, m);
                    out.push(format!("{func}({}) -> {d}", a.join(",")));
                }
                MirStmt::VoidCall { func, args, .. } => {
                    let mut a: Vec<String> = Vec::with_capacity(args.len());
                    for id in args {
                        a.push(slots.tok(*id, m));
                    }
                    out.push(format!("void {func}({})", a.join(",")));
                }
                MirStmt::Assign { lhs, rhs } => {
                    let l = slots.tok(*lhs, m);
                    let r = slots.tok(*rhs, m);
                    out.push(format!("{l} <- {r}"));
                }
                MirStmt::If { cond, then, else_, .. } => {
                    let c = slots.tok(*cond, m);
                    out.push(format!("if({c}){{"));
                    trace(m, then, slots, out);
                    out.push("}else{".to_string());
                    trace(m, else_, slots, out);
                    out.push("}".to_string());
                }
                MirStmt::Return { val } => {
                    let v = slots.tok(*val, m);
                    out.push(format!("ret {v}"));
                }
                MirStmt::Break => out.push("brk".to_string()),
                MirStmt::Continue => out.push("cont".to_string()),
                _ => out.push("其它语句".to_string()),
            }
        }
    }

    let cells = [
        ("start_char：端点是字符＋整数", "start_char"),
        ("esc_start：转义端点在起点＋整数终点", "esc_start"),
        ("end_char：端点是整数＋字符", "end_char"),
        ("escapes：转义端点 '\\n'..='\\t'", "escapes"),
        ("incl_int：`1..=10` 闭区间", "incl_int"),
        ("excl_int：`1..10` 开区间", "excl_int"),
        ("char_letters：两条字符臂", "char_letters"),
        ("binder：`q @ 1..=10` 绑定形", "binder"),
        ("two_arms：两条整数臂的先后", "two_arms"),
    ];

    let names: Vec<String> = mirs
        .iter()
        .map(|m| m.name.clone().unwrap_or_else(|| "~anon".to_string()))
        .collect();
    let mut got: Vec<(String, String)> = vec![
        ("进得了 MIR 的函数（按名排序）".to_string(), names.join(",")),
        (
            "解析是否被截断（剩余非空＝是）".to_string(),
            if remaining.trim().is_empty() { "否".to_string() } else { "是".to_string() },
        ),
    ];
    for (label, fname) in cells {
        let cell = match mirs.iter().find(|m| m.name.as_deref() == Some(fname)) {
            Some(m) => {
                let mut slots = Slots::default();
                let mut out: Vec<String> = Vec::new();
                trace(m, &m.stmts, &mut slots, &mut out);
                out.join(" ")
            }
            None => "<没进 MIR>".to_string(),
        };
        got.push((label.to_string(), cell));
    }

    let want: Vec<(String, String)> = vec![
        ("进得了 MIR 的函数（按名排序）".to_string(), "binder,char_letters,end_char,esc_start,escapes,excl_int,incl_int,main,start_char,two_arms".to_string()),
        ("解析是否被截断（剩余非空＝是）".to_string(), "否".to_string()),
        ("start_char：端点是字符＋整数".to_string(), "pi s0 >=(s0,97) -> s1 <=(s0,5) -> s2 &&(s1,s2) -> s3 if(s4){ s5 <- 1 }else{ if(1){ s5 <- 0 }else{ } } ret s5".to_string()),
        ("esc_start：转义端点在起点＋整数终点".to_string(), "pi s0 >=(s0,10) -> s1 <=(s0,5) -> s2 &&(s1,s2) -> s3 if(s4){ s5 <- 4 }else{ if(1){ s5 <- 0 }else{ } } ret s5".to_string()),
        ("end_char：端点是整数＋字符".to_string(), "pi s0 >=(s0,1) -> s1 <=(s0,122) -> s2 &&(s1,s2) -> s3 if(s4){ s5 <- 2 }else{ if(1){ s5 <- 0 }else{ } } ret s5".to_string()),
        ("escapes：转义端点 '\\n'..='\\t'".to_string(), "pi s0 >=(s0,10) -> s1 <=(s0,9) -> s2 &&(s1,s2) -> s3 if(s4){ s5 <- 7 }else{ if(1){ s5 <- 0 }else{ } } ret s5".to_string()),
        ("incl_int：`1..=10` 闭区间".to_string(), "pi s0 >=(s0,1) -> s1 <=(s0,10) -> s2 &&(s1,s2) -> s3 if(s4){ s5 <- 111 }else{ if(1){ unary_minus(1) -> s6 s5 <- s6 }else{ } } ret s5".to_string()),
        ("excl_int：`1..10` 开区间".to_string(), "pi s0 >=(s0,1) -> s1 <(s0,10) -> s2 &&(s1,s2) -> s3 if(s4){ s5 <- 111 }else{ if(1){ unary_minus(1) -> s6 s5 <- s6 }else{ } } ret s5".to_string()),
        ("char_letters：两条字符臂".to_string(), "pi s0 >=(s0,48) -> s1 <=(s0,57) -> s2 &&(s1,s2) -> s3 >=(s0,97) -> s4 <=(s0,122) -> s5 &&(s4,s5) -> s6 if(s7){ s8 <- 1 }else{ if(s9){ s8 <- 3 }else{ if(1){ s8 <- 0 }else{ } } } ret s8".to_string()),
        ("binder：`q @ 1..=10` 绑定形".to_string(), "pi s0 >=(s0,1) -> s1 <=(s0,10) -> s2 &&(s1,s2) -> s3 if(s4){ s5 <- s0 }else{ if(1){ unary_minus(1) -> s6 s5 <- s6 }else{ } } ret s5".to_string()),
        ("two_arms：两条整数臂的先后".to_string(), "pi s0 >=(s0,8) -> s1 <=(s0,9) -> s2 &&(s1,s2) -> s3 >=(s0,1) -> s4 <=(s0,3) -> s5 &&(s4,s5) -> s6 if(s7){ s8 <- 10 }else{ if(s9){ s8 <- 20 }else{ if(1){ s8 <- 0 }else{ } } } ret s8".to_string()),
    ];
    assert_eq!(
        got, want,
        "范围模式族的三处解析位（起点字符端点／终点字符端点／转义表）任一失效，\
         后果都不是报错而是「该函数起、文件余部整段丢掉」（W1002，退出码仍 0）；\
         `inclusive` 位失效则 `..=` 与 `..` 同义（比较符 `<`/`<=` 不分）。改前＝批次 325 \
         记录：`parse_range_pattern` 端点只吃 `parse_lit`、`inclusive: _` 被丢弃。"
    );
}

/// 批次 328（代码 `c7e4f7e8`，2026-09-22，站点 `src/middle/ctfe/value.rs:286-301` 的
/// `ConstValue::binary_op_int` 百分号臂，其中 328 新写的是 :294-299 那一段）／旧 numeric 族。
///
/// 症状（328 记录原文）：Rust 的取余跟**被除数**同号，Python 的取余跟**除数**同号
/// （`-7 % 3` 在 Python 是 `2`，在 Rust 是 `-1`）⇒ 折叠层把字面量取余折成错值。
/// 328 自陈这一处有两个实现点："字面量走 CTFE、变量走 codegen，所以两处都得改，
/// 只补一处会一个对一个错"。本条钉其中的**字面量点**（i64 常量层）。
///
/// 观测点：`const 名: int = 字面量 % 字面量` 的折叠结果落在 MIR 的 `global_consts` 表
/// （`src/middle/mir/mir.rs:13`，与批次 327 那条同格），值在进程内直接可读。
///
/// 期望值来源：CPython 现场实拍（`-7%3=2｜7%-3=-2｜-7%-3=-1｜8%3=2｜6%-3=0｜-6%3=0`）
/// ＋ HEAD 上 `--dump-mir` 同序读数（夹具 `/tmp/b10042/probe_const.z`，
/// 读数 `/tmp/b10042/probe_const.out`）——两侧六格一字相同。
///
/// 覆盖面分工（变异实测，日志 `/tmp/b10042/matrix_out.txt` 与
/// `/tmp/b10042/matrix_out_2b.txt`，逐臂原始输出 `arm_<臂名>_named_const_modulo.log`）：
/// - M1＝撤成 328 的改前写法（`Ok(left % right)`，截断语义）⇒ 本条红两格：
///   `NEG_LHS` 读回 `-1`、`NEG_RHS` 读回 `1`（＝CPython 的两个症状值形状），其余四格不变；
/// - M2＝只去掉 `r != 0 &&` 那道整除守卫 ⇒ 本条红一格：`EXACT_NEG_RHS`（`6 % -3`）
///   读回 `-3`。红格集与 M1 不相交＝这一臂里"补符号"和"整除时别补"是两件事，各有一格钉住；
/// - M6＝让这一臂直接失败（返回除错过）⇒ 六格全读回"表里没有这一项"，
///   即六格都依赖这一臂在跑（正证据；对照组那四格不是空跑）。
/// 同时刻 `print_argument_modulo...` 那条（下一条，i128 层）在 M1/M2/M6 下全部为绿
/// ⇒ 两层折叠互不备份，各写一条。
///
/// 本条不覆盖（记在 backlog #20005 余项）：328 的第二个实现点＝出码层
/// `build_floormod_int`（`src/backend/codegen/codegen.rs`，变量操作数走那里），
/// 后端是本车道不碰的改动面 ⇒ 不做变异，现由在册差分夹具
/// `tests/diff/cases/numeric_mod_dyn_neg.dcase` 在运行期承担。
#[test]
fn named_const_modulo_folds_with_the_divisors_sign() {
    let mirs = lower_all(
        r#"const NEG_LHS: int = -7 % 3
const NEG_RHS: int = 7 % -3
const BOTH_NEG: int = -7 % -3
const BOTH_POS: int = 8 % 3
const EXACT_NEG_RHS: int = 6 % -3
const EXACT_POS_RHS: int = -6 % 3
"#,
    );
    let f = mir(&mirs, "main");
    let names = [
        "NEG_LHS",
        "NEG_RHS",
        "BOTH_NEG",
        "BOTH_POS",
        "EXACT_NEG_RHS",
        "EXACT_POS_RHS",
    ];
    let got: Vec<(String, String)> = names
        .iter()
        .map(|n| {
            let v = match f.global_consts.get(*n) {
                Some(ConstValue::Int(v)) => v.to_string(),
                Some(other) => format!("{other:?}"),
                None => "<表里没有这一项>".to_string(),
            };
            ((*n).to_string(), v)
        })
        .collect();
    let want: Vec<(String, String)> = [
        ("NEG_LHS", "2"),
        ("NEG_RHS", "-2"),
        ("BOTH_NEG", "-1"),
        ("BOTH_POS", "2"),
        ("EXACT_NEG_RHS", "0"),
        ("EXACT_POS_RHS", "0"),
    ]
    .iter()
    .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
    .collect();

    assert_eq!(
        want, got,
        "`const` 折叠出来的取余必须跟除数同号（批次 328 的 value.rs 百分号臂）。改前症状＝跟着被除数同号：NEG_LHS 读回 -1、NEG_RHS 读回 1；去掉整除守卫则 EXACT_NEG_RHS 读回 -3。"
    );
}

/// 批次 642（旁路 cleanup 车道编号，代码 `adc0ffba`，2026-09-29，站点
/// `src/middle/ctfe/evaluator.rs:190-205` 的 `eval_i128_tree` 百分号臂，注释自陈
/// "Python modulo: sign of the divisor"）。**编号与本树 `roadmap.md:25112` 那节主树批次 642
/// 重合**（那节是 3.2 Lowering 名绑定族的已回退尝试）⇒ 引用以哈希 `adc0ffba` 为身份。
///
/// 症状：与批次 328 同一条语义，但在**另一层**——642 给 `print` 实参新写了 i128 树求值
/// （记录原文："Python 语义：floor 除、模取除数号、有界幂……"），这一层的算术器自己实现
/// 取余，不复用 `ConstValue::binary_op_int`。所以 328 只补了 i64 那一层，i128 这一臂
/// 坏与不坏在 MIR 上是另一个读数；两层互不备份。
///
/// 观测点：`print(纯字面量取余)` 走 `rewrite_big_print`，求值成功后把十进制拼写渲染成
/// 字符串字面量下发给 `println_str`（同批次 665 那条的观测点，`evaluator.rs:300-306`）
/// ⇒ 折叠值在 `VoidCall{args:[N]}` 的 `exprs[N]` 上直接可读。
/// 这一臂弃权时读回来的不是字符串而是 `IntLit(N)`（M5 实拍）——值仍然对，但那是 i64 层
/// 经普通下型发下来的整数槽，`println_str` 那一档没了 ⇒ 断言按**形状＋值**一起判，
/// "形状变了"本身就是失败信号。
///
/// 期望值来源：CPython 现场实拍（六格同下）＋ HEAD 上 `--dump-mir` 逐格读数
/// （`/tmp/b10042/probe_print.out`）。逐格单独降一趟 MIR 的理由与批次 10037 那条相同：
/// 合在一起降时未折叠的格会换发射路径，"第 i 个 println 调用"与"第 i 格"对不上号。
///
/// 覆盖面分工（变异实测，日志同上）：
/// - M3＝把这一臂撤成截断语义（`Some((m, false))`）⇒ 本条红两格：`print(-7 % 3)` 读回
///   `-1`、`print(7 % -3)` 读回 `1`，其余四格不变；
/// - M4＝只去掉 `m != 0 &&` 那道整除守卫 ⇒ 本条红一格：`print(6 % -3)` 读回 `-3`
///   （红格集与 M3 不相交）；
/// - M5＝让这一臂弃权（返回 `None`）⇒ 六格全红，实得值见上一段：形状从 `StringLit`
///   退成 `IntLit`、数值六格一字未变。
/// 三条臂下上一条（i64 层的 `named_const_modulo...`）全部为绿 ⇒ 两条各钉各的层。
///
/// 与既有两条的分工：批次 643 那条（`floordiv_word_operator_folds_at_compile_time`）
/// 钉的是同一函数里整除臂的**算子别名** `"//" | "floordiv"`；批次 665 那条
/// （`parenthesized_comparison_folds_with_value_semantics_not_as_a_python_chain`）钉的是
/// 同一函数里比较折叠臂。三条同函数不同臂，各自红各自的格子。
#[test]
fn print_argument_modulo_folds_with_the_divisors_sign() {
    let cells: [(&str, &str, &str); 6] = [
        ("负被除数／正除数（328 记录的症状形）", "print(-7 % 3)", "2"),
        ("正被除数／负除数", "print(7 % -3)", "-2"),
        ("两操作数皆负（两种语义同值，对照组）", "print(-7 % -3)", "-1"),
        ("两操作数皆正（对照组）", "print(8 % 3)", "2"),
        ("整除且除数为负（钉整除守卫）", "print(6 % -3)", "0"),
        ("整除且除数为正（对照组）", "print(-6 % 3)", "0"),
    ];
    let got: Vec<(&str, String)> = cells
        .iter()
        .map(|(label, line, _)| {
            let mirs = lower_all(line);
            let f = mir(&mirs, "main");
            let operand = f
                .stmts
                .iter()
                .find_map(|s| match s {
                    MirStmt::VoidCall { func, args } if func.starts_with("println") => {
                        args.first().copied()
                    }
                    _ => None,
                });
            let shape = match operand.and_then(|id| f.exprs.get(&id)) {
                Some(MirExpr::StringLit(v)) => v.clone(),
                Some(other) => format!("{other:?}"),
                None => "<没有 println 调用>".to_string(),
            };
            (*label, shape)
        })
        .collect();
    let want: Vec<(&str, String)> = cells
        .iter()
        .map(|(label, _, v)| (*label, (*v).to_string()))
        .collect();

    assert_eq!(
        want, got,
        "`print` 的 i128 折叠取余必须跟除数同号（批次 642 的 evaluator.rs 百分号臂）。读回 -1／1＝那一臂退回截断语义；读回非字符串形状＝这一臂弃权、走了运行期出码。"
    );
}

/// 10043 用：把"真除法在不在 MIR 里、打印走哪个 `println` 函数、实参是什么形状"压成一格字符串。
/// 扫描按 `lower_all` 排好的函数顺序（`f_0` 在 `main` 之前），所以"被调方体里的 div ＋
/// 调用点的 `println`"会落进同一串；多个 div 先排序再拼，免得逐次翻。
fn div_and_print_shape(mirs: &[Mir]) -> String {
    let mut divs: Vec<String> = Vec::new();
    let mut print: Option<(String, String)> = None;
    for m in mirs {
        let mut stack: Vec<&MirStmt> = m.stmts.iter().rev().collect();
        while let Some(s) = stack.pop() {
            match s {
                MirStmt::Call { func, dest, .. } if func == "/" => {
                    let t = match m.type_map.get(dest) {
                        Some(ty) => format!("{ty:?}"),
                        None => "<槽型缺失>".to_string(),
                    };
                    divs.push(format!("div->{t}"));
                }
                MirStmt::VoidCall { func, args } if func.starts_with("println") => {
                    if print.is_none() {
                        let arg = match args.first().and_then(|id| m.exprs.get(id)) {
                            Some(MirExpr::IntLit(v)) => format!("IntLit({v})"),
                            Some(MirExpr::FloatLit(v)) => format!("FloatLit({v})"),
                            Some(MirExpr::StringLit(v)) => format!("StringLit({v:?})"),
                            Some(MirExpr::Var(slot)) => match m.type_map.get(slot) {
                                Some(ty) => format!("Var({ty:?})"),
                                None => "Var(<槽型缺失>)".to_string(),
                            },
                            Some(other) => {
                                let d = format!("{other:?}");
                                d.split_whitespace().collect::<Vec<_>>().join(" ")
                            }
                            None => "<exprs 里没有这一项>".to_string(),
                        };
                        print = Some((func.clone(), arg));
                    }
                }
                MirStmt::If { then, else_, .. } => {
                    stack.extend(then.iter().rev().chain(else_.iter().rev()));
                }
                MirStmt::For { body, else_body, .. } | MirStmt::While { body, else_body, .. } => {
                    stack.extend(body.iter().rev().chain(else_body.iter().rev()));
                }
                _ => {}
            }
        }
    }
    divs.sort();
    let div = if divs.is_empty() {
        "no-div".to_string()
    } else {
        divs.join(",")
    };
    match print {
        Some((f, a)) => format!("{div} | {f} | {a}"),
        None => format!("{div} | no-print | -"),
    }
}

/// 批次 454（代码 `913e0a01`，2026-09-27，站点 `src/middle/ctfe/evaluator.rs:791-805` 的
/// `(AstNode::Lit(_), AstNode::Lit(_)) if op == "/"` 弃权臂；行号按本批最后一轮实跑取）。
///
/// 症状（记录原文）："Folding `7 / 2` to `Lit(3)` baked the truncated quotient into the AST
/// before MIR typing could see the expression. Leave the node alone: the lowering emits the
/// fdiv and the answer is 3.5."——`AstNode::Lit` 只装 i64，把真除法的商折进去就是把 3.5
/// 悄悄换成 3，而且是在 MIR 给这个表达式确定类型之前换的。
///
/// 观测点：真除法在 MIR 里是一条 `Call{func:"/"}`（`dest` 槽带类型），后面跟
/// `VoidCall{func:"println_f64", args:[dest]}`。这一臂被撤掉时第 1、2 格读回来的是
/// `no-div | println_i64 | IntLit(3)`／`IntLit(4)`，第 4 格读回 `no-div | println_i64 |
/// Var(I64)`——商在 AST 里就被折成 i64 字面量，MIR 里再没有除法调用，所以"没有 div 调用"
/// 本身就是失败信号。
///
/// 期望值来源：CPython 现场（`7 / 2`＝3.5、`8 / 2`＝4.0、`6 * 2`＝12、`3 + 4`＝7）＋
/// 本树 AOT 实拍（`/tmp/b10043/aot_c1` 打 `3.5`，rc=0）＋ `--dump-mir` 逐格读数
/// （`/tmp/b10043/f_c1_print_neg.out` 等，见 `/tmp/b10043/probe3.py`）。逐格单独降一趟 MIR 的
/// 理由同批次 10037／10042：合在一起降时未折叠的格会换发射路径，第 i 个 `println` 与第 i 格对不上号。
///
/// 覆盖面分工（六臂变异，汇总在 `/tmp/b10043/matrix_out.txt`，逐臂日志在同目录
/// `arm_M*_*.log`）：① 撤整臂（M1）＝第 1、2、4 格红，第 3 格与两格对照绿；② 把弃权条件
/// 从"只有 `/`"放宽到"所有字面量对"（M2）＝6 格一字不变——乘法／加法即便不在 AST 层折叠，
/// 下游仍折成同一个字面量，所以本批的格子测不出"弃权范围溢出"，两格对照只锁住当前发射形状，
/// **不**锁住"只有 `/` 被豁免"；③ 第 3 格（`x = 7 / 2` 再打印）在 M1 下不变＝赋值右侧根本不经
/// 这一臂，本条覆盖的是 `print` 参数与 `return` 表达式这两条会走到 AST 折叠的路径。
/// `const` 那一层（批次 454 没管）由下一条测试锁住；两条测试在任何一臂下都不互相变红。
///
/// 与既有用例的分工：批次 643 的 `floordiv_word_operator_folds_at_compile_time` 锁住词算子
/// `floordiv`（`.z` 里 `//` 是行注释，不是整除）；批次 10042 那两条锁住取余的两层折叠；
/// 454 的运行期真值由在册夹具 `t487`/`t488`/`t489` 承担，本条只锁编译期 MIR 形状。
#[test]
fn true_division_of_integer_literals_is_not_folded_at_the_ctfe_layer() {
    let cells: [(&str, &str, &str); 6] = [
        (
            "print 真除法（非整除；折成 3 就是静默错值）",
            r#"print(7 / 2)
"#,
            "div->F64 | println_f64 | Var(F64)",
        ),
        (
            "print 真除法（整除也要留浮点：Python 里 8/2 是 4.0）",
            r#"print(8 / 2)
"#,
            "div->F64 | println_f64 | Var(F64)",
        ),
        (
            "先赋给变量再打印：槽型跟着真除法走",
            r#"x = 7 / 2
print(x)
"#,
            "div->F64 | println_f64 | Var(F64)",
        ),
        (
            "函数返回真除法：被调方体里是 div，调用点目的槽是 F64",
            r#"def f():
    return 7 / 2
y = f()
print(y)
"#,
            "div->F64 | println_f64 | Var(F64)",
        ),
        (
            "对照组：乘法字面量仍然折叠（弃权臂只盖 `/`）",
            r#"x = 6 * 2
print(x)
"#,
            r#"no-div | println_str | StringLit("12")"#,
        ),
        (
            "对照组：加法字面量仍然折叠",
            r#"x = 3 + 4
print(x)
"#,
            r#"no-div | println_str | StringLit("7")"#,
        ),
    ];

    let got: Vec<(&str, String)> = cells
        .iter()
        .map(|(label, src, _)| (*label, div_and_print_shape(&lower_all(src))))
        .collect();
    let want: Vec<(&str, String)> = cells
        .iter()
        .map(|(label, _, v)| (*label, (*v).to_string()))
        .collect();

    assert_eq!(
        want, got,
        r#"两个整数字面量的 `/` 必须在 MIR 里留成真除法调用（批次 454 的 evaluator.rs 弃权臂）。读回 no-div | println_i64 | IntLit(3) 这类形状＝这一臂被撤，截断商在 MIR 确定类型之前就被折进 AST；第 3 格读回 no-div＝赋值右侧也开始经这一臂（本批实测不经，见文档头③）。"#
    );
}

/// 批次 454 同一条语义的**另一层**——`const` 项折叠走的是 `src/middle/ctfe/value.rs:274-285`
/// 的 `binary_op_int` 斜杠臂（`left.wrapping_div(right)`），发射点在 `src/middle/mir/gen.rs:3640-3660`
/// 往 `global_consts` 里写；454 那批只改了 AST 侧的 `ConstEvaluator`，这一层至今仍然折叠。
///
/// 这条是**现状锁**，不是"这一层已经按 454 改对"的证据：`const A: int = 7 / 2` 显式声明了
/// `int`，折成截断商 3 是自洽的；但它与 454 的立场（字面量 `/` 的商是浮点、不许折）是两套口径，
/// 差在哪一格算对，记录里没有裁定，所以只锁住"当前读数长这样"，改动时本条应红并强制重新取证。
///
/// 观测点：`global_consts` 里按名字可读的 `ConstValue::Int`。B／D 两格用来区分截断除与地板除：
/// `-7 / 2` 截断＝-3、地板＝-4；`-8 / 3` 截断＝-2、地板＝-3（Rust 的 `wrapping_div` 朝零截断）。
/// E 格（`8 / 0`）读回来是"表里没有这一项"，而且 `zetac` 整趟 rc=0、stderr 为空——除零在这里
/// 被静默丢掉，已登记为 `#20005` 余项。
///
/// 期望值来源：CPython 实算（7/2＝3.5 → 声明 int 取截断 3；-7/2＝-3.5；8/2＝4.0；-8/3＝-2.67）＋
/// 本树 `--dump-mir` 实拍（`/tmp/b10043/f_combined.out`：A=3、B=-3、C=4、D=-2，E 缺项；
/// 逐格单降的读数在 `/tmp/b10043/f_d*.out`，两问一致）。
///
/// 覆盖面分工（同一批六臂变异，逐臂日志在 `/tmp/b10043/arm_M*_*.log`）：把这一臂改成地板除
/// （`div_euclid`，M3）只有 B／D 红，读回 -4／-3；让这一臂一律报错（M4）则 A／B／C／D 四格
/// 一起变成缺项、E 格不变。两臂的坏格集是**包含关系**（M3 ⊂ M4），按构造 M3 拿不到独占格。
/// `binary_op_uint` 里另有一份逐字相同的斜杠臂，把它也改成报错（M6）后两条测试一字不变＝
/// 负数与普通整数字面量走的是有符号那一层，无符号那一层本批没覆盖。撤 AST 侧那一臂（M1、M2）
/// 时本条 5 格一字不变，反之撤本条这一臂时上一条 6 格一字不变＝两条测试互不备份、各自独立。
#[test]
fn const_declared_integer_division_folds_to_the_truncated_quotient() {
    let mirs = lower_all(
        r#"const A: int = 7 / 2
const B: int = -7 / 2
const C: int = 8 / 2
const D: int = -8 / 3
const E: int = 8 / 0
"#,
    );
    let f = mir(&mirs, "main");
    let names = ["A", "B", "C", "D", "E"];
    let want_vals = ["3", "-3", "4", "-2", "<表里没有这一项>"];
    let got: Vec<(String, String)> = names
        .iter()
        .map(|n| {
            let v = match f.global_consts.get(*n) {
                Some(ConstValue::Int(x)) => x.to_string(),
                Some(other) => format!("{other:?}"),
                None => "<表里没有这一项>".to_string(),
            };
            ((*n).to_string(), v)
        })
        .collect();
    let want: Vec<(String, String)> = names
        .iter()
        .zip(want_vals.iter())
        .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
        .collect();

    assert_eq!(
        want, got,
        "`const X: int = 字面量 / 字面量` 在 value.rs 的斜杠臂里折成**朝零截断**的商（批次 454 没管这一层；本条是现状锁）。读回 -4／-3＝这一臂改成地板除；读回缺项＝这一臂不再折叠；E 格若有值＝除零守卫被去掉，静默错值换了形状。"
    );
}
