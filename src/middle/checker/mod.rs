//! 批次 911（轴 F 方案② P1 骨架）：统一类型检查器——SCCP 式 slot→型不动点。
//!
//! 设计稿：docs/f2-checker-design.md。
//! 批次 915 扩展：递归扫描嵌套块（If/Loop）＋字段访问型传播＋
//! 方法返回型（method_ret）＋Return 语句收集。
//! 批次 916 扩展：方法调用返回型传播（method_ret 查表）＋
//! 下标结果型传播（元素型/映射值型）。

pub mod constraint;
pub mod field_ty;
pub mod lattice;

use crate::frontend::ast::AstNode;
use crate::middle::checker::lattice::LatticeTy;
use crate::middle::types::Type;
use std::collections::HashMap;

/// 全程序唯一的类型环境（设计稿 §7）。
#[derive(Debug, Default, Clone)]
pub struct TypeEnv {
    /// 名字→格值（跨函数以名字为键；槽号每函数重排不可作键）。
    pub slots: HashMap<String, LatticeTy>,
    /// 函数→推断返回型（回写 func_ret_types 的来源）。
    pub fn_rets: HashMap<String, Type>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self::default()
    }

    /// 查表（miss＝Unknown）。
    pub fn get_slot(&self, name: &str) -> LatticeTy {
        self.slots.get(name).cloned().unwrap_or(LatticeTy::Unknown)
    }

    /// meet 合并：返回是否发生变化（变化 ⇒ 调用方把引用语句重新入列）。
    pub fn meet_slot(&mut self, name: &str, lat: LatticeTy) -> bool {
        let next = match self.slots.get(name) {
            Some(prev) => prev.meet(&lat),
            None => lat,
        };
        if self.slots.get(name) != Some(&next) {
            self.slots.insert(name.to_string(), next);
            true
        } else {
            false
        }
    }
}

/// 推断上下文：checker 需要访问的外部表。
pub struct InferCtx<'a> {
    /// 函数返回型表（来自 resolver 的 get_all_func_signatures）。
    pub ret_types: &'a HashMap<String, Type>,
    /// struct 名 → 字段表（来自 type_decls / shared_type_decls 合并）。
    pub type_decls: &'a HashMap<String, crate::middle::mir::r#gen::TypeDecl>,
    /// 模块全局名集合（env-first 读的判定）。
    pub module_globals: &'a std::collections::HashSet<String>,
}

/// 单函数体内的递归扫描＋约束传播（批 915 重构：递归进 If/Loop 等嵌套块）。
/// 批 918 扩展：Return 语句收集→fn_rets（P4 回灌替换的前提）。
/// 批 920 扩展：collect_fn_ret 递归收集＋变量槽／调用返回型。
pub fn infer_fn_body(env: &mut TypeEnv, fn_name: &str, body: &[AstNode], ctx: &InferCtx) {
    infer_fn_body_with_params(env, fn_name, &[], body, ctx);
}

/// 带参数注解版（批 931）：注解非空的参数先 meet 到参数槽，再扫函数体。
/// 参数槽已知后，体内的赋值边/二元运算/method_ret 查表都能吃到参数型
/// （checker_env 下游 mean 臂等消费点的真实增益面）。
pub fn infer_fn_body_with_params(
    env: &mut TypeEnv,
    fn_name: &str,
    params: &[(String, String)],
    body: &[AstNode],
    ctx: &InferCtx,
) {
    infer_fn_body_full(env, fn_name, params, None, body, ctx);
}

/// 带调用点证据版（批 934）：无注解参数位若证据一致 ⇒ 用证据型 meet。
pub fn infer_fn_body_full(
    env: &mut TypeEnv,
    fn_name: &str,
    params: &[(String, String)],
    evidence: Option<&[Option<Type>]>,
    body: &[AstNode],
    ctx: &InferCtx,
) {
    prime_param_slots(env, params, evidence);
    scan_stmts(env, body, ctx);
    // Return 语句收集→fn_rets（批 918：P4 回灌替换的前提）
    collect_fn_ret(env, fn_name, body, ctx);
}

/// 模块级调用点证据（批 934）：扫描全部函数体，对用户函数的无接收者调用
/// 收集字面量实参型 → 形参位候选。同一参数位多调用点型不一致 ⇒ None
/// （保守放弃——错证据比缺证据危害大）。
pub fn collect_param_evidence(
    funcs: &HashMap<String, AstNode>,
    body_rets: Option<&HashMap<String, Type>>,
    envs: Option<&HashMap<String, TypeEnv>>,
) -> HashMap<String, Vec<Option<Type>>> {
    // fn 名 → (候选列表, 冲突位列表)。冲突位单独记账：置 None 的候选
    // 不能被后续一致调用重新填充（None 兼"无证据"与"冲突"两义会漏记）。
    let mut table: HashMap<String, (Vec<Option<Type>>, Vec<bool>)> =
        HashMap::new();
    for def in funcs.values() {
        if let AstNode::FuncDef { name, body, .. } = def {
            let mut calls: Vec<&AstNode> = Vec::new();
            collect_calls(body, &mut calls);
            for c in calls {
                if let AstNode::Call {
                    receiver: None,
                    method,
                    args,
                    ..
                } = c
                {
                    if !funcs.contains_key(method) {
                        continue;
                    }
                    let (slot, conflicted) = table
                        .entry(method.clone())
                        .or_insert_with(|| {
                            (
                                vec![None; args.len().max(1)],
                                vec![false; args.len().max(1)],
                            )
                        });
                    // 当前函数名（Var 实参查调用函数自己的槽型，批 937）
                    let caller_name = match def {
                        AstNode::FuncDef { name, .. } => name.as_str(),
                        _ => "",
                    };
                    for (i, a) in args.iter().enumerate() {
                        if i >= slot.len() {
                            continue;
                        }
                        if conflicted[i] {
                            continue;
                        }
                        // 字面量优先；调用表达式 ⇒ body_rets 查返回型
                        //（批 936 二跳）；变量 ⇒ 调用函数推断后的槽型
                        //（批 937 三跳）；构造调用（首字母大写的注册名，
                        // py 类降糖的 ctor FuncDef 也在册）⇒ Named(类名)
                        //（批 946——scale(Point(), 2) 的 p 位由此定型）
                        let lit = constraint::literal_lattice(a)
                            .and_then(|l| l.known_ty())
                            .or_else(|| {
                                if let (
                                    AstNode::Call {
                                        receiver: None, method, ..
                                    },
                                    Some(br),
                                ) = (a, body_rets)
                                {
                                    br.get(method).cloned()
                                } else {
                                    None
                                }
                            })
                            .or_else(|| {
                                if let AstNode::Call {
                                    receiver: None,
                                    method,
                                    ..
                                } = a
                                {
                                    if method
                                        .chars()
                                        .next()
                                        .map_or(false, |c| c.is_uppercase())
                                        && funcs.contains_key(method.as_str())
                                    {
                                        return Some(Type::Named(
                                            method.clone(),
                                            vec![],
                                        ));
                                    }
                                }
                                None
                            })
                            .or_else(|| {
                                if let (
                                    AstNode::Var(vn),
                                    Some(env_map),
                                ) = (a, envs)
                                {
                                    env_map
                                        .get(caller_name)
                                        .and_then(|env| {
                                            env.get_slot(vn.as_str())
                                                .known_ty()
                                        })
                                } else {
                                    None
                                }
                            });
                        match (lit, &slot[i]) {
                            (Some(t), None) => slot[i] = Some(t),
                            // 与既有候选一致 ⇒ 保持
                            (Some(t), Some(prev)) if *prev == t => {}
                            // 两个都推得出但不一致 ⇒ 真冲突，永久放弃
                            (Some(_), Some(_)) => {
                                slot[i] = None;
                                conflicted[i] = true;
                            }
                            // 本调用点推不出 ⇒ 不动既有候选（批 943 修正：
                            // 此前"推不出"也被当冲突锁死，把可推断调用点
                            // 的候选作废）
                            (None, _) => {}
                        }
                    }
                }
            }
        }
    }
    table.into_iter().map(|(k, (cand, _))| (k, cand)).collect()
}

/// 顶层调用点证据并入（批 940）：顶层 `show(get_data())` 不在任何函数体
/// 里，collect_param_evidence 的函数体扫描收不到——单独扫顶层语句并调和。
/// 候选来源：字面量、调用表达式（body_rets 查）；变量实参保守不收
///（顶层 env 不在建计划阶段推断）。
fn merge_top_level_evidence(
    funcs: &HashMap<String, AstNode>,
    base: &HashMap<String, Vec<Option<Type>>>,
    body_rets: Option<&HashMap<String, Type>>,
    module_env: Option<&TypeEnv>,
    top_bodies: &[AstNode],
) -> HashMap<String, Vec<Option<Type>>> {
    if top_bodies.is_empty() {
        return base.clone();
    }
    let mut out = base.clone();
    let mut calls: Vec<&AstNode> = Vec::new();
    collect_calls(top_bodies, &mut calls);
    for c in calls {
        if let AstNode::Call {
            receiver: None,
            method,
            args,
            ..
        } = c
        {
            if !funcs.contains_key(method) {
                continue;
            }
            let slot = out
                .entry(method.clone())
                .or_insert_with(|| vec![None; args.len().max(1)]);
            for (i, a) in args.iter().enumerate() {
                if i >= slot.len() {
                    continue;
                }
                let lit = constraint::literal_lattice(a)
                    .and_then(|l| l.known_ty())
                    .or_else(|| {
                        if let (
                            AstNode::Call {
                                receiver: None,
                                method: m2,
                                ..
                            },
                            Some(br),
                        ) = (a, body_rets)
                        {
                            br.get(m2).cloned()
                        } else {
                            None
                        }
                    })
                    // 顶层 Var 实参 ⇒ 查 module_env 槽型（批 947：
                    // get(d, "a") 的 dd 位吃 d 的精化槽）
                    .or_else(|| {
                        if let (AstNode::Var(vn), Some(me)) = (a, module_env) {
                            me.get_slot(vn.as_str()).known_ty()
                        } else {
                            None
                        }
                    });
                match (lit, &slot[i]) {
                    (Some(t), None) => slot[i] = Some(t),
                    (Some(t), Some(prev)) if *prev == t => {}
                    (Some(_), Some(_)) => slot[i] = None,
                    (None, _) => {}
                }
            }
        }
    }
    out
}

/// cond 中的窄化提取（批 941；批 946 扩组合）：isinstance(x, T) ⇒
/// [(x, T 型)]。T 认两类：用户类名（type_decls 里在册 ⇒ Named(T)）与
/// 内建标量（int/float/str）。
/// 组合语义（批 946）：
/// - `A && B` 为真 ⇒ A、B 都真 ⇒ 两侧候选取并集；同一变量两侧不一致
///   ⇒ 该变量放弃（A 且 B 不可满足， meet 会造 Conflict）
/// - `A || B` 为真 ⇒ 至少一侧真 ⇒ 仅两侧候选完全一致才窄化
///   （异型并集格上不可表达）
/// - `not isinstance(...)` ⇒ then 内不成立 ⇒ 自然落空（不窄化）
fn narrow_from_cond(
    cond: &AstNode,
    env: &TypeEnv,
    ctx: &InferCtx,
) -> Vec<(String, Type)> {
    narrow_from_cond_with_decls(cond, ctx.type_decls)
}

/// 窄化提取的 gen 消费面（批 946）：gen 侧 If 语句臂在 then 降级前
/// push 覆盖层、降级后 pop——分支级槽型由此进入代码生成。
pub(crate) fn narrow_from_cond_with_decls(
    cond: &AstNode,
    type_decls: &HashMap<String, crate::middle::mir::r#gen::TypeDecl>,
) -> Vec<(String, Type)> {
    if let AstNode::BinaryOp { op, left, right } = cond {
        if op == "&&" || op == "||" {
            let lc = narrow_from_cond_with_decls(left, type_decls);
            let rc = narrow_from_cond_with_decls(right, type_decls);
            return if op == "&&" {
                // 并集；同变量冲突 ⇒ 弃该变量
                let mut out = lc.clone();
                for (n, t) in rc {
                    if let Some(slot) = out.iter_mut().find(|(m, _)| *m == n) {
                        if slot.1 != t {
                            out.retain(|(m, _)| *m != n);
                        }
                    } else {
                        out.push((n, t));
                    }
                }
                out
            } else {
                // 两侧完全一致才窄化
                if !lc.is_empty() && lc == rc {
                    lc
                } else {
                    vec![]
                }
            };
        }
    }
    if let AstNode::Call {
        receiver: None,
        method,
        args,
        ..
    } = cond
    {
        if method == "isinstance" && args.len() == 2 {
            if let (AstNode::Var(x), AstNode::Var(t)) = (&args[0], &args[1]) {
                let ty = if type_decls.contains_key(t.as_str()) {
                    Some(Type::Named(t.clone(), vec![]))
                } else {
                    match t.as_str() {
                        "int" => Some(Type::I64),
                        "float" => Some(Type::F64),
                        "str" => Some(Type::Str),
                        _ => None,
                    }
                };
                if let Some(ty) = ty {
                    return vec![(x.clone(), ty)];
                }
            }
        }
    }
    vec![]
}

/// 参数槽初始化（批 940 修正）：注解优先，但细化阶段的 "dyn" 动态标记
/// （from_string ⇒ PyDynamic）让位给调用点证据——证据是真实调用点的
/// 具体型，标记只是"没推断出"的占位。推断不出时标记维持动态语义。
fn prime_param_slots(
    env: &mut TypeEnv,
    params: &[(String, String)],
    evidence: Option<&[Option<Type>]>,
) {
    for (i, (pname, anno)) in params.iter().enumerate() {
        let ev_ty = evidence.and_then(|e| e.get(i)).and_then(|o| o.clone());
        let ann_lat = if anno.is_empty() {
            None
        } else {
            constraint::annotation_lattice(anno)
        };
        let known = ann_lat.as_ref().and_then(|l| l.known_ty());
        // 弱注解（批 943）：refine 给未注解参数写 "i64"/"dyn" 缺省（gen 的
        // codegen 签名面依赖字符串本身），checker 侧它们只是 ABI 缺省假设
        // 不是用户意图——调用点证据（真实实参型）优先。
        let weak = matches!(known, None | Some(Type::I64) | Some(Type::F64))
            || matches!(known, Some(Type::PyDynamic));
        if ev_ty.is_some() && weak {
            env.meet_slot(pname.as_str(), LatticeTy::known(ev_ty.unwrap()));
        } else if let Some(lat) = ann_lat {
            env.meet_slot(pname.as_str(), lat);
        } else if let Some(t) = ev_ty {
            env.meet_slot(pname.as_str(), LatticeTy::known(t));
        }
    }
}

/// 全局槽种子注入（批 942）：模块级顶层赋值推断出的槽型进函数 env，
/// skip 名单（函数参数）遮蔽——同名参数的型独立于全局。赋值/窄化只进
/// 函数 env，不回写模块 env（每次降级从种子重建）。
pub fn seed_module_slots(env: &mut TypeEnv, module_env: &TypeEnv, skip: &[String]) {
    for (name, val) in module_env.slots.iter() {
        if skip.iter().any(|p| p == name) {
            continue;
        }
        if env.get_slot(name) != *val {
            env.meet_slot(name, val.clone());
        }
    }
}

/// 模块级 checker 推断计划（批 938）：证据表＋并入 body_rets 的查表＋
/// 函数 env 缓存。lower_to_mir 每函数/闭包调用一次（batch 738 在册
/// 651 次）——这套编排只依赖全模块注册表，在入口惰性构建一次。
pub struct ModuleCheckerPlan {
    pub evidence: HashMap<String, Vec<Option<Type>>>,
    pub ret_map_full: HashMap<String, Type>,
    pub env_cache: HashMap<String, TypeEnv>,
    /// 模块级顶层赋值推断的槽型（批 942）：G = [1.0, 2.0] 的 G ⇒
    /// DynamicArray(F64)，函数 env 种子注入的来源。
    pub module_env: TypeEnv,
}

/// 惰性构建模块级推断计划（批 938）：字面量证据 → body_rets → 每函数
/// 推断缓存 env → 二跳/三跳证据。
pub fn build_module_checker_plan(
    funcs: &HashMap<String, AstNode>,
    ret_map: &HashMap<String, Type>,
    type_decls: &HashMap<String, crate::middle::mir::r#gen::TypeDecl>,
    module_globals: &std::collections::HashSet<String>,
    top_bodies: &[AstNode],
) -> ModuleCheckerPlan {
    let ev0 = collect_param_evidence(funcs, None, None);
    let body_rets =
        collect_module_body_rets(funcs, &ev0, ret_map, type_decls, module_globals);
    let mut lookup = body_rets.clone();
    for (k, v) in ret_map.iter() {
        lookup.entry(k.clone()).or_insert(v.clone());
    }
    let ctx = InferCtx {
        ret_types: &lookup,
        type_decls,
        module_globals,
    };
    let mut env_cache: HashMap<String, TypeEnv> = HashMap::new();
    for (fname, fdef) in funcs.iter() {
        if let AstNode::FuncDef { params, body, .. } = fdef {
            let mut fenv = TypeEnv::new();
            let f_ev = ev0.get(fname).map(|v| v.as_slice());
            infer_fn_body_full(&mut fenv, fname, params, f_ev, body, &ctx);
            env_cache.insert(fname.clone(), fenv);
        }
    }
    // 模块级槽推断（批 942）：顶层赋值语句的 Var lhs 建槽。批 947 起
    // 先于证据合并——顶层 Var 实参的证据查 module_env（写侧精化后的
    // 容器槽型由此过函数边界）。
    // 批 954 修正：只保留 module_globals 名单内的槽——扫描面里的赋值
    // 目标可能包括合成 main 体内部的**局部**变量（py 语料把用户顶层
    // 语句包装进 main），原实现把它们当全局种子注入到所有函数 env，
    // 造成 main 局部型跨函数泄漏（隐错型源）。
    let mut module_env = TypeEnv::new();
    scan_module_slots(&mut module_env, top_bodies, &ctx);
    // 批 954 补扫：py 语料把用户顶层语句包装进合成 main 体（top_bodies
    // 在该路径下只有函数定义）——main 的 body 也扫；retain 过滤保证
    // main 内部局部变量不入种子
    if let Some(AstNode::FuncDef { body: main_body, .. }) = funcs.get("main") {
        scan_module_slots(&mut module_env, main_body, &ctx);
    }
    module_env.slots.retain(|k, _| module_globals.contains(k));
    let evidence =
        collect_param_evidence(funcs, Some(&body_rets), Some(&env_cache));
    let evidence = merge_top_level_evidence(
        funcs,
        &evidence,
        Some(&body_rets),
        Some(&module_env),
        top_bodies,
    );
    let mut ret_map_full = body_rets;
    for (k, v) in ret_map.iter() {
        ret_map_full.entry(k.clone()).or_insert(v.clone());
    }
    ModuleCheckerPlan {
        evidence,
        ret_map_full,
        env_cache,
        module_env,
    }
}

/// 顶层语句的槽推断（批 942）：扫 Assign（lhs Var/Tuple）与字面量，
/// 建模块级槽型。复用 scan_stmts——If/For 等顶层控制流一并覆盖。
fn scan_module_slots(env: &mut TypeEnv, body: &[AstNode], ctx: &InferCtx) {
    scan_stmts(env, body, ctx);
}

/// 模块级 body 返回型收集（批 935）：对全部注册函数各建独立 env 推断
/// 一轮（参数注解/证据先入槽），收集各自的 fn_rets 成全局表。供 resolver
/// 并入 ctx 的 ret_types 查表——无注解函数调用的返回型由此闭环
/// （注解优先，调用方 or_insert 不覆盖）。
pub fn collect_module_body_rets(
    funcs: &HashMap<String, AstNode>,
    evidence: &HashMap<String, Vec<Option<Type>>>,
    ret_types: &HashMap<String, Type>,
    type_decls: &HashMap<String, crate::middle::mir::r#gen::TypeDecl>,
    module_globals: &std::collections::HashSet<String>,
) -> HashMap<String, Type> {
    let ctx = InferCtx {
        ret_types,
        type_decls,
        module_globals,
    };
    let mut out: HashMap<String, Type> = HashMap::new();
    for (name, def) in funcs {
        if let AstNode::FuncDef { params, body, .. } = def {
            let mut env = TypeEnv::new();
            let ev_slice = evidence.get(name).map(|v| v.as_slice());
            prime_param_slots(&mut env, params, ev_slice);
            scan_stmts(&mut env, body, &ctx);
            collect_fn_ret(&mut env, name.as_str(), body, &ctx);
            for (k, v) in env.fn_rets {
                out.entry(k).or_insert(v);
            }
        }
    }
    out
}

/// 递归收集函数体里所有无接收者 Call 节点（批 934）。
fn collect_calls<'a>(body: &'a [AstNode], out: &mut Vec<&'a AstNode>) {
    for stmt in body {
        match stmt {
            // 裸调用语句：顶层可执行语句包装进 main 体后，表达式语句
            // 是裸 Call（无 ExprStmt 包裹）——实测 main 体 heads=[Call,
            // Call, Assign, Call]，漏此臂则顶层调用点全部漏收（批 940）
            AstNode::Call { .. } => collect_calls_expr(stmt, out),
            AstNode::ExprStmt { expr } => collect_calls_expr(expr, out),
            AstNode::Assign(_, rhs) => collect_calls_expr(rhs, out),
            AstNode::Return(e) => collect_calls_expr(e, out),
            AstNode::If { then, else_, .. } => {
                collect_calls(then, out);
                collect_calls(else_, out);
            }
            AstNode::Loop { body } => collect_calls(body, out),
            AstNode::While { body, else_body, .. } => {
                collect_calls(body, out);
                collect_calls(else_body, out);
            }
            AstNode::For { body, else_body, .. } => {
                collect_calls(body, out);
                collect_calls(else_body, out);
            }
            AstNode::Unsafe { body } => collect_calls(body, out),
            _ => {}
        }
    }
}

fn collect_calls_expr<'a>(e: &'a AstNode, out: &mut Vec<&'a AstNode>) {
    match e {
        // 无接收者调用（用户函数候选）：收录并递归实参——嵌套调用
        //（print(check(p)) 里的 check(p)）此前漏收，批 943
        AstNode::Call {
            receiver: None,
            args,
            ..
        } => {
            out.push(e);
            for a in args {
                collect_calls_expr(a, out);
            }
        }
        AstNode::Call {
            receiver: Some(r),
            args,
            ..
        } => {
            collect_calls_expr(r, out);
            for a in args {
                collect_calls_expr(a, out);
            }
        }
        AstNode::FieldAccess { base, .. } => collect_calls_expr(base, out),
        AstNode::Subscript { base, index } => {
            collect_calls_expr(base, out);
            collect_calls_expr(index, out);
        }
        AstNode::BinaryOp { left, right, .. } => {
            collect_calls_expr(left, out);
            collect_calls_expr(right, out);
        }
        AstNode::UnaryOp { expr, .. } => collect_calls_expr(expr, out),
        _ => {}
    }
}

/// 从函数体收集全部 Return（含嵌套块）并推断返回型，记入 fn_rets。
/// 多个 return 全部已知且一致才记入；互相冲突则保守不记（宁缺勿错）。
fn collect_fn_ret(env: &mut TypeEnv, fn_name: &str, body: &[AstNode], ctx: &InferCtx) {
    let mut rets: Vec<&AstNode> = Vec::new();
    collect_returns(body, &mut rets);
    let mut known: Option<Type> = None;
    for e in &rets {
        if let Some(ty) = ret_expr_ty(e, env, ctx) {
            match &known {
                None => known = Some(ty),
                Some(prev) if *prev == ty => {}
                // 已知型互相冲突 ⇒ 放弃记录
                Some(_) => return,
            }
        }
    }
    if let Some(ty) = known {
        env.fn_rets.insert(fn_name.to_string(), ty);
    }
}

/// 递归收集函数体里所有 Return 的返回表达式。
fn collect_returns<'a>(body: &'a [AstNode], out: &mut Vec<&'a AstNode>) {
    for stmt in body {
        match stmt {
            AstNode::Return(e) => out.push(e),
            AstNode::If { then, else_, .. } => {
                collect_returns(then, out);
                collect_returns(else_, out);
            }
            AstNode::Loop { body } => collect_returns(body, out),
            AstNode::While { body, else_body, .. } => {
                collect_returns(body, out);
                collect_returns(else_body, out);
            }
            AstNode::For { body, else_body, .. } => {
                collect_returns(body, out);
                collect_returns(else_body, out);
            }
            AstNode::Unsafe { body } => collect_returns(body, out),
            _ => {}
        }
    }
}

/// 单个 return 表达式的型：字面量／已知槽变量／ret_types 查表。
fn ret_expr_ty(e: &AstNode, env: &TypeEnv, ctx: &InferCtx) -> Option<Type> {
    if let Some(lat) = constraint::literal_lattice(e) {
        return lat.known_ty();
    }
    if let AstNode::Var(name) = e {
        if let LatticeTy::Known(ty) = env.get_slot(name) {
            return Some(ty);
        }
        return None;
    }
    if let AstNode::Call { method, .. } = e {
        return ctx.ret_types.get(method).cloned();
    }
    // 一元运算 Return（批 968）：-x/~x 操作数槽为数值标量或 BigInt ⇒
    // 同型（复用批 921 的 is_numeric_ty 语义）；not ⇒ Bool
    if let AstNode::UnaryOp { op, expr } = e {
        match op.as_str() {
            "not" => return Some(Type::Bool),
            "-" | "~" => {
                if let AstNode::Var(n) = &**expr {
                    if let LatticeTy::Known(t) = env.get_slot(n.as_str()) {
                        if is_numeric_ty(&t) {
                            return Some(t);
                        }
                    }
                }
                return None;
            }
            _ => return None,
        }
    }
    // 列表字面量 Return ⇒ DynamicArray(元素型)（批 935 补形态）
    if let AstNode::ArrayLit(elems) = e {
        return uniform_elem_lat(elems)
            .map(|t| Type::DynamicArray(Box::new(t)));
    }
    // 字段访问 Return ⇒ 字段型（批 944——return v.px 的 getx 函数边界
    // 由此知道返回 f64；此前缺失使 x = getx(p) 的 x 槽落 I64，f64 位
    // 模式被整数乘出垃圾）
    if let AstNode::FieldAccess { base, field } = e {
        if let AstNode::Var(bn) = &**base {
            if let LatticeTy::Known(Type::Named(tn, _)) = env.get_slot(bn.as_str())
            {
                if let Some(crate::middle::mir::r#gen::TypeDecl::Struct {
                    fields,
                    ..
                }) = ctx.type_decls.get(tn.as_str())
                {
                    if let Some((_, ft)) = fields.iter().find(|(f, _)| f == field)
                    {
                        return Some(Type::from_string(ft));
                    }
                }
            }
        }
        return None;
    }
    None
}

/// 数值标量／BigInt（-、~ 一元运算保持同型的操作数面，批 921）。
/// 数值字面量（整数/浮点字面量节点，批 922）。
fn is_num_literal(e: &AstNode) -> bool {
    matches!(e, AstNode::Lit(_) | AstNode::FloatLit(_))
}

/// 数值标量／BigInt（-、~ 一元运算保持同型的操作数面，批 921）。
fn is_numeric_ty(ty: &Type) -> bool {
    matches!(
        ty,
        Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize
            | Type::F32
            | Type::F64
    ) || matches!(ty, Type::Named(n, _) if n == "BigInt")
}

fn scan_stmts(env: &mut TypeEnv, body: &[AstNode], ctx: &InferCtx) {
    for stmt in body {
        match stmt {
            // 赋值：lhs 为 Var 时传播型
            AstNode::Assign(lhs, rhs) => {
                if let AstNode::Var(name) = &**lhs {
                    propagate_assign(env, name, rhs, ctx);
                }
                // d[k] = v 写侧精化（批 947）：map 槽的键/值型按 gen 同一条
                // 首插规则精化（仅 I64 占位可被替换）；已钉槽写异型值 ⇒
                // 值型退化 PyDynamic（污染信号，静态 per-cell 语义——
                // batch 765 的 dict 级钉型已被否决，运行期格标签按格渲染）
                if let AstNode::Subscript { base, index } = &**lhs {
                    if let AstNode::Var(bn) = &**base {
                        if let LatticeTy::Known(Type::Named(n, targs)) =
                            env.get_slot(bn.as_str())
                        {
                            if n == "map" && targs.len() == 2 {
                                let val_ty = expr_known_ty(rhs, env, ctx);
                                let key_ty = match &**index {
                                    AstNode::StringLit(_) => Some(Type::Str),
                                    _ => expr_known_ty(index, env, ctx),
                                };
                                let old_key = targs[0].clone();
                                let old_val = targs[1].clone();
                                let new_key = match (&key_ty, &old_key) {
                                    (Some(Type::Str), Type::I64) => Type::Str,
                                    _ => old_key.clone(),
                                };
                                let new_val = match (&val_ty, &old_val) {
                                    (Some(v), Type::I64) if *v != Type::I64 => {
                                        v.clone()
                                    }
                                    (Some(v), old) if *v != old.clone() => {
                                        Type::PyDynamic
                                    }
                                    _ => old_val.clone(),
                                };
                                if new_key != old_key || new_val != old_val {
                                    env.slots.insert(
                                        bn.clone(),
                                        LatticeTy::known(Type::Named(
                                            "map".to_string(),
                                            vec![new_key, new_val],
                                        )),
                                    );
                                }
                            }
                        }
                    }
                }
                // 元组解包：x, y = pair ⇒ 逐分量传播（批 929）
                if let (AstNode::Tuple(elems), _) = (&**lhs, &**rhs) {
                    let src_ty = expr_known_ty(rhs, env, ctx);
                    if let Some(Type::Tuple(tys)) = src_ty {
                    for (i, e) in elems.iter().enumerate() {
                        if let (AstNode::Var(nm), Some(t)) = (e, tys.get(i)) {
                            env.meet_slot(nm.as_str(), LatticeTy::known(t.clone()));
                        }
                    }                    }
                }
                // rhs 内嵌套赋值（walrus 等）递归
                scan_expr(env, rhs, ctx);
            }
            // 控制流：递归进分支体。真分支在克隆 env 上扫描——cond 里的
            // isinstance(x, T) 窄化（x ⇒ Named(T)/内建标量）只在分支内生效；
            // 汇合时窄化槽不回流（分支后的 x 不保证是 T，批 941）。
            AstNode::If { cond, then, else_ } => {
                let narrowed = narrow_from_cond(cond, env, ctx);
                if narrowed.is_empty() {
                    scan_stmts(env, then, ctx);
                } else {
                    let mut then_env = env.clone();
                    for (name, ty) in &narrowed {
                        then_env.meet_slot(name.as_str(), LatticeTy::known(ty.clone()));
                    }
                    scan_stmts(&mut then_env, then, ctx);
                    for (name, val) in then_env.slots.iter() {
                        if narrowed.iter().any(|(n, _)| n == name) {
                            continue;
                        }
                        if env.get_slot(name) != *val {
                            env.meet_slot(name, val.clone());
                        }
                    }
                    scan_stmts(env, else_, ctx);
                }
            }
            AstNode::Loop { body, .. } | AstNode::Unsafe { body } => {
                scan_stmts(env, body, ctx);
            }
            // for 循环：迭代变量从序列元素型推断＋体递归（批 919）
            AstNode::For {
                pattern,
                expr,
                body,
                else_body,
            } => {
                if let AstNode::Var(name) = pattern.as_ref() {
                    if let Some(lat) = for_elem_lat(expr, env) {
                        env.meet_slot(name, lat);
                    }
                }
                // 元组模式：enumerate(arr)/zip(a,b)/Tuple 槽逐分量（批 930）
                if let AstNode::Tuple(elems) = pattern.as_ref() {
                    if let Some(tys) = tuple_iter_components(expr, env) {
                        for (i, e) in elems.iter().enumerate() {
                            if let (AstNode::Var(nm), Some(t)) = (e, tys.get(i)) {
                                env.meet_slot(nm.as_str(), LatticeTy::known(t.clone()));
                            }
                        }
                    }
                }
                scan_stmts(env, body, ctx);
                scan_stmts(env, else_body, ctx);
            }
            // while 循环：体递归（批 919；此前循环体整体漏扫）
            AstNode::While { body, else_body, .. } => {
                scan_stmts(env, body, ctx);
                scan_stmts(env, else_body, ctx);
            }
            AstNode::FuncDef { body, .. } => {
                scan_stmts(env, body, ctx);
            }
            // 表达式语句：递归进内嵌赋值
            AstNode::ExprStmt { expr } => {
                if let AstNode::Assign(lhs, rhs) = &**expr {
                    if let AstNode::Var(name) = &**lhs {
                        propagate_assign(env, name, rhs, ctx);
                    }
                }
            }
            _ => {}
        }
    }
}

fn scan_expr(_env: &mut TypeEnv, _expr: &AstNode, _ctx: &InferCtx) {
    // P2 后续扩展点：表达式内嵌套约束（当前不需要）
}

/// for 迭代序列的元素型（批 919；保守：只做有把握的形态，set 无元素型信息不推断）。
fn for_elem_lat(expr: &AstNode, env: &TypeEnv) -> Option<LatticeTy> {
    match expr {
        // range 家族 ⇒ I64（`a..b`、Range 节点、range() 调用）
        AstNode::BinaryOp { op, .. } if op == ".." => {
            return Some(LatticeTy::known(Type::I64));
        }
        AstNode::Range { .. } => return Some(LatticeTy::known(Type::I64)),
        AstNode::Call {
            receiver: None,
            method,
            ..
        } if method == "range" => return Some(LatticeTy::known(Type::I64)),
        _ => {}
    }
    if let AstNode::Var(src) = expr {
        return match env.get_slot(src) {
            LatticeTy::Known(Type::DynamicArray(e))
            | LatticeTy::Known(Type::Slice(e))
            | LatticeTy::Known(Type::Array(e, _)) => Some(LatticeTy::known((*e).clone())),
            // map 迭代产出键（对齐 gen/call_flow.rs map_keys 语义：Str 键映射保 Str，其余 I64）
            LatticeTy::Known(Type::Named(n, targs)) if n == "map" => {
                let key = if matches!(targs.first(), Some(Type::Str)) {
                    Type::Str
                } else {
                    Type::I64
                };
                Some(LatticeTy::known(key))
            }
            _ => None,
        };
    }
    None
}

/// for 元组模式的逐分量型：enumerate(seq)⇒(I64, elem)；zip(a,b)⇒逐序列
/// 元素型；序列槽本身为 Tuple ⇒ 原样（批 930）。
fn tuple_iter_components(expr: &AstNode, env: &TypeEnv) -> Option<Vec<Type>> {
    if let AstNode::Call {
        receiver: None,
        method,
        args,
        ..
    } = expr
    {
        match method.as_str() {
            "enumerate" => {
                let a0 = args.first()?;
                let e = for_elem_lat(a0, env)?.known_ty()?;
                return Some(vec![Type::I64, e]);
            }
            "zip" => {
                let mut tys = Vec::new();
                for a in args {
                    tys.push(for_elem_lat(a, env)?.known_ty()?);
                }
                return Some(tys);
            }
            _ => {}
        }
    }
    if let AstNode::Var(nm) = expr {
        if let LatticeTy::Known(t @ Type::Tuple(_)) = env.get_slot(nm) {
            return Some(match t {
                Type::Tuple(tys) => tys.clone(),
                _ => return None,
            });
        }
    }
    None
}

/// 赋值传播：确定 rhs 的型并 meet 到 lhs 名。
fn propagate_assign(env: &mut TypeEnv, name: &str, rhs: &AstNode, ctx: &InferCtx) {
    // 字面量
    if let Some(lat) = constraint::literal_lattice(rhs) {
        env.meet_slot(name, lat);
        return;
    }
    // 赋值边（名字→名字）
    if let AstNode::Var(src) = rhs {
        let src_lat = env.get_slot(src);
        if src_lat.is_known() {
            env.meet_slot(name, src_lat);
        }
        return;
    }
    // 二元运算传播（批 922 重构：同型双变量、变量与数值字面量、Str 拼接、比较⇒Bool）
    if let AstNode::BinaryOp { op, left, right } = rhs {
        // 比较运算 ⇒ Bool
        if matches!(
            op.as_str(),
            "==" | "!=" | "<" | ">" | "<=" | ">=" | "in" | "not in"
        ) {
            env.meet_slot(name, LatticeTy::known(Type::Bool));
            return;
        }
        // 双变量同型 ⇒ 同型
        if let (AstNode::Var(a), AstNode::Var(b)) = (&**left, &**right) {
            let la = env.get_slot(a);
            let lb = env.get_slot(b);
            if la.is_known() && la == lb && !matches!(la, LatticeTy::Conflict) {
                env.meet_slot(name, la);
            }
            return;
        }
        // 变量＋字面量：数值操作数保持变量型；Str 拼接 ⇒ Str（双向）
        for (v, other) in [(left, right), (right, left)] {
            if let AstNode::Var(a) = &**v {
                if let LatticeTy::Known(ty) = env.get_slot(a) {
                    if is_numeric_ty(&ty) && is_num_literal(other) {
                        env.meet_slot(name, LatticeTy::known(ty));
                        return;
                    }
                    if op == "+"
                        && ty == Type::Str
                        && matches!(&**other, AstNode::StringLit(_))
                    {
                        env.meet_slot(name, LatticeTy::known(Type::Str));
                        return;
                    }
                    if op == "+" && ty == Type::Str {
                        if let AstNode::Var(b) = &**other {
                            if env.get_slot(b) == LatticeTy::known(Type::Str) {
                                env.meet_slot(name, LatticeTy::known(Type::Str));
                                return;
                            }
                        }
                    }
                }
            }
        }
        return;
    }
    // FString ⇒ Str（批次 917 扩展：f-string 结果恒为文本）
    if let AstNode::FString(_) = rhs {
        env.meet_slot(name, LatticeTy::known(Type::Str));
        return;
    }
    // 一元运算（批 921）：not ⇒ Bool；-x/~x 操作数为数值标量或 BigInt ⇒ 同型
    if let AstNode::UnaryOp { op, expr } = rhs {
        match op.as_str() {
            "not" => {
                env.meet_slot(name, LatticeTy::known(Type::Bool));
            }
            "-" | "~" => {
                if let AstNode::Var(src) = &**expr {
                    if let LatticeTy::Known(ty) = env.get_slot(src) {
                        if is_numeric_ty(&ty) {
                            env.meet_slot(name, LatticeTy::known(ty));
                        }
                    }
                }
            }
            _ => {}
        }
        return;
    }
    // 调用返回（ret_types 查表）
    if let AstNode::Call {
        receiver,
        method,
        args,
        ..
    } = rhs
    {
        // 转换内建 ⇒ 目标型（批 929）
        let conv = match method.as_str() {
            "str" => Some(Type::Str),
            "int" => Some(Type::I64),
            "float" => Some(Type::F64),
            _ => None,
        };
        if let Some(t) = conv {
            env.meet_slot(name, LatticeTy::known(t));
            return;
        }
        // 标量 abs/min/max（批 932）：操作数槽已知同型 ⇒ 同型
        if matches!(method.as_str(), "abs" | "min" | "max") && receiver.is_none() {
            let operand_lats: Vec<LatticeTy> =
                args.iter().filter_map(|a| expr_known_ty(a, env, ctx)).map(LatticeTy::known).collect();
            if !operand_lats.is_empty()
                && operand_lats.iter().all(|l| l == &operand_lats[0])
                && is_numeric_ty(operand_lats[0].known_ty().as_ref().unwrap_or(&Type::I64))
            {
                env.meet_slot(name, operand_lats[0].clone());
                return;
            }
        }
        // len ⇒ I64；sorted ⇒ DynamicArray(元素型)（批 930）
        if method == "len" {
            env.meet_slot(name, LatticeTy::known(Type::I64));
            return;
        }
        if method == "sorted" {
            if let Some(a0) = args.first() {
                if let Some(LatticeTy::Known(e)) = for_elem_lat(a0, env) {
                    env.meet_slot(
                        name,
                        LatticeTy::known(Type::DynamicArray(Box::new(e))),
                    );
                }
            }
            return;
        }
        // 方法调用（receiver 已知型）：pop ⇒ 元素型；method_ret 查表。
        // 批 930 修正：这段原是独立臂，落在 ret_types 臂无条件 return 之后
        // ——批 916 落地起就不可达（死代码），现并入。
        if let Some(recv) = receiver {
            if method == "pop" {
                if let Some(LatticeTy::Known(e)) = for_elem_lat(recv, env) {
                    env.meet_slot(name, LatticeTy::known(e));
                    return;
                }
            }
            if let AstNode::Var(recv_name) = &**recv {
                if let LatticeTy::Known(Type::Named(tag, _)) =
                    env.get_slot(recv_name.as_str())
                {
                    if let Some(ret_handle) =
                        crate::middle::pylib::method_ret(tag.as_str(), method.as_str())
                    {
                        let ret_ty = match ret_handle {
                            "str" => Type::Str,
                            "f64" => Type::F64,
                            _ => Type::Named(ret_handle.to_string(), vec![]),
                        };
                        env.meet_slot(name, LatticeTy::known(ret_ty));
                        return;
                    }
                }
            }
        }
        if let Some(ty) = ctx.ret_types.get(method) {
            env.meet_slot(name, LatticeTy::known(ty.clone()));
            return;
        }
        // 构造调用：首字母大写且 type_decls 在册的类名 ⇒ Named(类名)
        //（批 943——p = Point() 的 p 槽由此知道自己是 Point）
        if method.chars().next().map_or(false, |c| c.is_uppercase())
            && ctx.type_decls.contains_key(method.as_str())
        {
            env.meet_slot(
                name,
                LatticeTy::known(Type::Named(method.clone(), vec![])),
            );
        }
        return;
    }
    // 字段访问：struct 已知 ⇒ 查字段型（批 915 扩展）
    if let AstNode::FieldAccess { base, field } = rhs {
        if let AstNode::Var(base_name) = &**base {
            let base_lat = env.get_slot(base_name);
            if let LatticeTy::Known(Type::Named(tn, _)) = &base_lat {
                if let Some(crate::middle::mir::r#gen::TypeDecl::Struct { fields, .. }) =
                    ctx.type_decls.get(tn)
                {
                    if let Some((_, ft)) = fields.iter().find(|(f, _)| f == field) {
                        let fty = Type::from_string(ft);
                        env.meet_slot(name, LatticeTy::known(fty));
                        return;
                    }
                }
            }
        }
    }
    // 下标（批 923 重构：切片⇒DynamicArray(e)，取元素⇒e，map⇒V）。
    // 批 947 修正：map 下标读必须先于 for_elem_lat 判定——后者是 for
    // 迭代语义（map ⇒ 键型），误用进下标读把 m["a"] 的**值**读成键型
    //（t485 实拍 v=Str ⇒ v[0] 槽标 Str ⇒ print 走 strlen SEGV）
    if let AstNode::Subscript { base, index } = rhs {
        let is_slice = matches!(&**index, AstNode::Range { .. })
            || matches!(&**index, AstNode::BinaryOp { op, .. } if op == "..");
        if let AstNode::Var(base_name) = &**base {
            let base_lat = env.get_slot(base_name);
            if !is_slice {
                if let LatticeTy::Known(Type::Named(n, targs)) = &base_lat {
                    if n == "map" {
                        if let Some(v) = targs.get(1) {
                            env.meet_slot(name, LatticeTy::known(v.clone()));
                            return;
                        }
                    }
                }
            }
            if let Some(LatticeTy::Known(seq_elem)) = for_elem_lat(base, env) {
                // 数组族序列：元素型已判定；切片保持动态数组形状
                let res = if is_slice {
                    Type::DynamicArray(Box::new(seq_elem))
                } else {
                    seq_elem
                };
                env.meet_slot(name, LatticeTy::known(res));
                return;
            }
        }
    }
    // 列表字面量：元素型可直接判定 ⇒ DynamicArray(元素型)（批 923）
    if let AstNode::ArrayLit(elems) = rhs {
        if let Some(lat) = uniform_elem_lat(elems) {
            env.meet_slot(
                name,
                LatticeTy::known(Type::DynamicArray(Box::new(lat))),
            );
        }
        return;
    }
    // 元组字面量：逐元素推断（批 923）
    if let AstNode::Tuple(elems) = rhs {
        let mut tys = Vec::new();
        let mut all = true;
        for e in elems {
            match expr_known_ty(e, env, ctx) {
                Some(t) => tys.push(t),
                None => {
                    all = false;
                    break;
                }
            }
        }
        if all {
            env.meet_slot(name, LatticeTy::known(Type::Tuple(tys)));
        }
        return;
    }
    // Cast ⇒ 目标注解型（批 923）
    if let AstNode::Cast { ty, .. } = rhs {
        env.meet_slot(name, LatticeTy::known(Type::from_string(ty)));
        return;
    }
    // 结构字面量 ⇒ Named(变体名)（批 943——p = Point { .. } 的 p 槽）
    if let AstNode::StructLit { variant, .. } = rhs {
        env.meet_slot(
            name,
            LatticeTy::known(Type::Named(variant.clone(), vec![])),
        );
        return;
    }
    // 字典字面量（批 933）：键取首个键型（Str 保 Str 否则 I64，对齐
    // map_keys/map 字面量语义）；值同型⇒该型、混型或推不出⇒PyDynamic、
    // 全 None⇒NoneValue、空⇒map[I64,I64]（对齐 gen/call_dict.rs 缺省）。
    if let AstNode::DictLit { entries } = rhs {
        let key_ty = match entries.first() {
            Some((k, _)) => match expr_known_ty(k, env, ctx) {
                Some(Type::Str) => Type::Str,
                _ => Type::I64,
            },
            None => Type::I64,
        };
        let mut val_ty: Option<Type> = None;
        let mut all_none = true;
        let mut mixed = false;
        for (_, v) in entries {
            match v {
                AstNode::NoneLit => {}
                _ => {
                    all_none = false;
                    match expr_known_ty(v, env, ctx) {
                        Some(t) => match &val_ty {
                            None => val_ty = Some(t),
                            Some(prev) if *prev == t => {}
                            _ => mixed = true,
                        },
                        // 有值推不出 ⇒ 不猜（PyDynamic 动态槽）
                        None => mixed = true,
                    }
                }
            }
        }
        let vty = if entries.is_empty() {
            Type::I64
        } else if all_none {
            Type::Named("NoneValue".to_string(), vec![])
        } else if mixed {
            Type::PyDynamic
        } else {
            val_ty.unwrap_or(Type::PyDynamic)
        };
        env.meet_slot(
            name,
            LatticeTy::known(Type::Named("map".to_string(), vec![key_ty, vty])),
        );
        return;
    }
}

/// 列表字面量的统一元素型：全部元素同型且可判定才给出（批 923）。
fn uniform_elem_lat(elems: &[AstNode]) -> Option<Type> {
    let mut ty: Option<Type> = None;
    for e in elems {
        let t = match e {
            AstNode::Lit(_) => Type::I64,
            AstNode::FloatLit(_) => Type::F64,
            AstNode::StringLit(_) => Type::Str,
            _ => return None,
        };
        match &ty {
            None => ty = Some(t),
            Some(prev) if *prev == t => {}
            _ => return None,
        }
    }
    ty
}

/// 表达式的已知型（元素级推断共用面，批 923）。
fn expr_known_ty(e: &AstNode, env: &TypeEnv, ctx: &InferCtx) -> Option<Type> {
    if let Some(lat) = constraint::literal_lattice(e) {
        return lat.known_ty();
    }
    if let AstNode::Var(n) = e {
        if let LatticeTy::Known(t) = env.get_slot(n) {
            return Some(t);
        }
    }
    if let AstNode::Call { method, .. } = e {
        return ctx.ret_types.get(method).cloned();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::AstNode;
    use std::collections::HashMap;

    fn var(n: &str) -> AstNode {
        AstNode::Var(n.to_string())
    }

    fn assign(lhs: &str, rhs: AstNode) -> AstNode {
        AstNode::Assign(Box::new(var(lhs)), Box::new(rhs))
    }

    /// 字面量赋值 ⇒ 槽型 Known(I64)。
    #[test]
    fn literal_assign_types_slot() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(&mut env, "test_fn", &[assign("x", AstNode::Lit(42))], &ctx);
        assert_eq!(env.get_slot("x"), LatticeTy::known(Type::I64));
    }

    /// 赋值边传播：b = a ⇒ b 与 a 同格。
    #[test]
    fn assign_edge_propagates() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign("a", AstNode::Lit(7)), assign("b", var("a"))],
            &ctx,
        );
        assert_eq!(env.get_slot("b"), env.get_slot("a"));
    }

    /// 冲突：同槽两种已知型 ⇒ Conflict。
    #[test]
    fn conflicting_writes_yield_conflict() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                assign("x", AstNode::Lit(1)),
                assign("x", AstNode::StringLit("s".to_string())),
            ],
            &ctx,
        );
        assert_eq!(env.get_slot("x"), LatticeTy::Conflict);
    }

    /// 调用返回型传播：ret_types 表里有 fn ⇒ 传播返回型。
    #[test]
    fn call_return_propagates() {
        let mut env = TypeEnv::new();
        let mut rets = HashMap::new();
        rets.insert("get_data".to_string(), Type::F64);
        let ctx = InferCtx {
            ret_types: &rets,
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign("y", AstNode::Call {
                receiver: None,
                method: "get_data".to_string(),
                args: vec![],
                type_args: vec![],
                structural: false,
            })],
            &ctx,
        );
        assert_eq!(env.get_slot("y"), LatticeTy::known(Type::F64));
    }

    fn call(method: &str, args: Vec<AstNode>) -> AstNode {
        AstNode::Call {
            receiver: None,
            method: method.to_string(),
            args,
            type_args: vec![],
            structural: false,
        }
    }

    /// for 循环：迭代变量从数组元素型推断（批 919）。
    #[test]
    fn for_iter_var_types_from_list() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "arr",
            LatticeTy::known(Type::DynamicArray(Box::new(Type::F64))),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::For {
                pattern: Box::new(var("x")),
                expr: Box::new(var("arr")),
                body: vec![],
                else_body: vec![],
            }],
            &ctx,
        );
        assert_eq!(env.get_slot("x"), LatticeTy::known(Type::F64));
    }

    /// for over range ⇒ 迭代变量 I64（批 919）。
    #[test]
    fn for_over_range_types_i64() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::For {
                pattern: Box::new(var("i")),
                expr: Box::new(call("range", vec![AstNode::Lit(10)])),
                body: vec![],
                else_body: vec![],
            }],
            &ctx,
        );
        assert_eq!(env.get_slot("i"), LatticeTy::known(Type::I64));
    }

    /// map 迭代产出键：Str 键映射保持 Str，其余 I64（对齐 map_keys 语义，批 919）。
    #[test]
    fn for_over_map_types_key() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "d",
            LatticeTy::known(Type::Named("map".to_string(), vec![Type::Str, Type::I64])),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::For {
                pattern: Box::new(var("k")),
                expr: Box::new(var("d")),
                body: vec![],
                else_body: vec![],
            }],
            &ctx,
        );
        assert_eq!(env.get_slot("k"), LatticeTy::known(Type::Str));
    }

    /// 循环体内的赋值也被扫描（批 919：For/While 体递归）。
    #[test]
    fn loop_body_assigns_scanned() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "arr",
            LatticeTy::known(Type::DynamicArray(Box::new(Type::Str))),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                AstNode::For {
                    pattern: Box::new(var("s")),
                    expr: Box::new(var("arr")),
                    body: vec![assign("n", AstNode::Lit(42))],
                    else_body: vec![],
                },
                AstNode::While {
                    cond: Box::new(AstNode::Lit(1)),
                    body: vec![assign(
                        "m",
                        AstNode::FloatLit("2.5".to_string()),
                    )],
                    else_body: vec![],
                },
            ],
            &ctx,
        );
        assert_eq!(env.get_slot("n"), LatticeTy::known(Type::I64));
        assert_eq!(env.get_slot("m"), LatticeTy::known(Type::F64));
    }

    /// return 变量：槽已知 ⇒ fn_rets 记入该型（批 920）。
    #[test]
    fn return_var_types_fn() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                assign("x", AstNode::Lit(5)),
                AstNode::Return(Box::new(var("x"))),
            ],
            &ctx,
        );
        assert_eq!(env.fn_rets.get("test_fn"), Some(&Type::I64));
    }

    /// 多个 return 一致 ⇒ 记入；不一致 ⇒ 不记（保守，批 920）。
    #[test]
    fn multiple_returns_agree_then_record() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                AstNode::If {
                    cond: Box::new(AstNode::Lit(1)),
                    then: vec![AstNode::Return(Box::new(AstNode::Lit(1)))],
                    else_: vec![],
                },
                AstNode::Return(Box::new(AstNode::Lit(2))),
            ],
            &ctx,
        );
        assert_eq!(env.fn_rets.get("test_fn"), Some(&Type::I64));
    }

    #[test]
    fn multiple_returns_conflict_skip() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                AstNode::If {
                    cond: Box::new(AstNode::Lit(1)),
                    then: vec![AstNode::Return(Box::new(AstNode::Lit(1)))],
                    else_: vec![],
                },
                AstNode::Return(Box::new(AstNode::StringLit("s".to_string()))),
            ],
            &ctx,
        );
        assert_eq!(env.fn_rets.get("test_fn"), None);
    }

    /// return 调用：ret_types 有 ⇒ 传播进 fn_rets（批 920）。
    #[test]
    fn return_call_types_fn() {
        let mut env = TypeEnv::new();
        let mut rets = HashMap::new();
        rets.insert("get_data".to_string(), Type::F64);
        let ctx = InferCtx {
            ret_types: &rets,
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::Return(Box::new(call("get_data", vec![])))],
            &ctx,
        );
        assert_eq!(env.fn_rets.get("test_fn"), Some(&Type::F64));
    }

    /// -x：操作数槽为数值型 ⇒ 结果同型（批 921）。
    #[test]
    fn unary_neg_propagates_operand_ty() {
        let mut env = TypeEnv::new();
        env.meet_slot("x", LatticeTy::known(Type::F64));
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign(
                "y",
                AstNode::UnaryOp {
                    op: "-".to_string(),
                    expr: Box::new(var("x")),
                },
            )],
            &ctx,
        );
        assert_eq!(env.get_slot("y"), LatticeTy::known(Type::F64));
    }

    /// not x ⇒ Bool（批 921）。
    #[test]
    fn unary_not_types_bool() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign(
                "flag",
                AstNode::UnaryOp {
                    op: "not".to_string(),
                    expr: Box::new(var("x")),
                },
            )],
            &ctx,
        );
        assert_eq!(env.get_slot("flag"), LatticeTy::known(Type::Bool));
    }

    /// x + 整数字面量：x 为数值 ⇒ 同型（批 922）。
    #[test]
    fn binop_var_int_lit_propagates() {
        let mut env = TypeEnv::new();
        env.meet_slot("count", LatticeTy::known(Type::I64));
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign(
                "next",
                AstNode::BinaryOp {
                    op: "+".to_string(),
                    left: Box::new(var("count")),
                    right: Box::new(AstNode::Lit(1)),
                },
            )],
            &ctx,
        );
        assert_eq!(env.get_slot("next"), LatticeTy::known(Type::I64));
    }

    /// x * 浮点字面量：x 为 F64 ⇒ 同型（批 922）。
    #[test]
    fn binop_var_float_lit_propagates() {
        let mut env = TypeEnv::new();
        env.meet_slot("total", LatticeTy::known(Type::F64));
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign(
                "scaled",
                AstNode::BinaryOp {
                    op: "*".to_string(),
                    left: Box::new(var("total")),
                    right: Box::new(AstNode::FloatLit("2.0".to_string())),
                },
            )],
            &ctx,
        );
        assert_eq!(env.get_slot("scaled"), LatticeTy::known(Type::F64));
    }

    /// Str + 字符串字面量 ⇒ Str（批 922）。
    #[test]
    fn binop_str_concat_types_str() {
        let mut env = TypeEnv::new();
        env.meet_slot("s", LatticeTy::known(Type::Str));
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                assign(
                    "t",
                    AstNode::BinaryOp {
                        op: "+".to_string(),
                        left: Box::new(var("s")),
                        right: Box::new(AstNode::StringLit("!".to_string())),
                    },
                ),
                assign(
                    "u",
                    AstNode::BinaryOp {
                        op: "+".to_string(),
                        left: Box::new(AstNode::StringLit("pre-".to_string())),
                        right: Box::new(var("t")),
                    },
                ),
            ],
            &ctx,
        );
        assert_eq!(env.get_slot("t"), LatticeTy::known(Type::Str));
        assert_eq!(env.get_slot("u"), LatticeTy::known(Type::Str));
    }

    /// 列表字面量全整数 ⇒ DynamicArray(I64)（批 923）。
    #[test]
    fn array_lit_int_types_dynamic_array() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign(
                "xs",
                AstNode::ArrayLit(vec![AstNode::Lit(1), AstNode::Lit(2)]),
            )],
            &ctx,
        );
        assert_eq!(
            env.get_slot("xs"),
            LatticeTy::known(Type::DynamicArray(Box::new(Type::I64)))
        );
    }

    /// 元组字面量 ⇒ Tuple(逐元素型)（批 923）。
    #[test]
    fn tuple_lit_types_tuple() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign(
                "pair",
                AstNode::Tuple(vec![AstNode::Lit(1), AstNode::StringLit("s".to_string())]),
            )],
            &ctx,
        );
        assert_eq!(
            env.get_slot("pair"),
            LatticeTy::known(Type::Tuple(vec![Type::I64, Type::Str]))
        );
    }

    /// 切片（下标为 Range）⇒ DynamicArray(元素型)（批 923）。
    #[test]
    fn slice_read_types_dynamic_array() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "arr",
            LatticeTy::known(Type::DynamicArray(Box::new(Type::F64))),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign(
                "part",
                AstNode::Subscript {
                    base: Box::new(var("arr")),
                    index: Box::new(AstNode::Range {
                        start: Box::new(AstNode::Lit(0)),
                        end: Box::new(AstNode::Lit(2)),
                        inclusive: false,
                    }),
                },
            )],
            &ctx,
        );
        assert_eq!(
            env.get_slot("part"),
            LatticeTy::known(Type::DynamicArray(Box::new(Type::F64)))
        );
    }

    /// Cast ⇒ from_string(目标型拼写)（批 923）。
    #[test]
    fn cast_types_target() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign(
                "f",
                AstNode::Cast {
                    expr: Box::new(AstNode::Lit(1)),
                    ty: "f64".to_string(),
                },
            )],
            &ctx,
        );
        assert_eq!(
            env.get_slot("f"),
            LatticeTy::known(Type::from_string("f64"))
        );
    }

    /// 元组解包赋值：x, y = pair ⇒ 逐分量按 Tuple 元素型传播（批 929）。
    #[test]
    fn tuple_unpack_propagates() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "pair",
            LatticeTy::known(Type::Tuple(vec![Type::I64, Type::Str])),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::Assign(
                Box::new(AstNode::Tuple(vec![var("x"), var("y")])),
                Box::new(var("pair")),
            )],
            &ctx,
        );
        assert_eq!(env.get_slot("x"), LatticeTy::known(Type::I64));
        assert_eq!(env.get_slot("y"), LatticeTy::known(Type::Str));
    }

    /// str()/int()/float() 转换内建 ⇒ 目标型（批 929）。
    #[test]
    fn conversion_builtins_types_target() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                assign("s", call("str", vec![AstNode::Lit(1)])),
                assign("i", call("int", vec![AstNode::FloatLit("2.5".to_string())])),
                assign("f", call("float", vec![AstNode::Lit(3)])),
            ],
            &ctx,
        );
        assert_eq!(env.get_slot("s"), LatticeTy::known(Type::Str));
        assert_eq!(env.get_slot("i"), LatticeTy::known(Type::I64));
        assert_eq!(env.get_slot("f"), LatticeTy::known(Type::F64));
    }
    /// 跨函数：调用点字面量实参 ⇒ 形参槽型（批 934）；冲突 ⇒ 不传。
    #[test]
    fn call_site_literals_type_params() {
        let mk_def = |name: &str, body: Vec<AstNode>| AstNode::FuncDef {
            name: name.to_string(),
            generics: vec![],
            lifetimes: vec![],
            params: vec![
                ("a".to_string(), String::new()),
                ("b".to_string(), String::new()),
            ],
            ret: String::new(),
            body,
            attrs: vec![],
            ret_expr: None,
            single_line: true,
            doc: String::new(),
            pub_: false,
            async_: false,
            const_: false,
            comptime_: false,
            where_clauses: vec![],
        };
        let g = mk_def(
            "g",
            vec![AstNode::Return(Box::new(var("a")))],
        );
        let mut registered = HashMap::new();
        registered.insert("g".to_string(), g);
        // 主调函数体：g(1.5, "s") 与 g(2.5, "t")——a 位一致 F64、b 位一致 Str
        let call_g = |x: AstNode, y: AstNode| {
            AstNode::ExprStmt {
                expr: Box::new(AstNode::Call {
                    receiver: None,
                    method: "g".to_string(),
                    args: vec![x, y],
                    type_args: vec![],
                    structural: false,
                }),
            }
        };
        let caller = mk_def(
            "caller",
            vec![
                call_g(AstNode::FloatLit("1.5".to_string()), AstNode::StringLit("s".to_string())),
                call_g(AstNode::FloatLit("2.5".to_string()), AstNode::StringLit("t".to_string())),
            ],
        );
        registered.insert("caller".to_string(), caller);
        // 冲突 caller2：g(1, ...)——a 位 I64 与 F64 冲突 ⇒ 双位放弃
        let caller2 = mk_def(
            "caller2",
            vec![call_g(AstNode::Lit(1), AstNode::StringLit("s".to_string()))],
        );
        registered.insert("caller2".to_string(), caller2);

        let ev = collect_param_evidence(&registered, None, None);
        let ev_g = ev.get("g").expect("g 的证据应存在");
        assert_eq!(ev_g[0], None, "a 位 F64/I64 冲突 ⇒ 放弃");

        // 只留一致调用点再验：a 位 F64、b 位 Str
        let mut registered2 = registered.clone();
        registered2.remove("caller2");
        let ev2 = collect_param_evidence(&registered2, None, None);
        let ev2_g = ev2.get("g").expect("g 证据");
        assert_eq!(ev2_g[0], Some(Type::F64));
        assert_eq!(ev2_g[1], Some(Type::Str));

        // 证据进推断：def g(a, b): x = a ⇒ a 槽 F64
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        let g_params = vec![
            ("a".to_string(), String::new()),
            ("b".to_string(), String::new()),
        ];
        infer_fn_body_full(
            &mut env,
            "g",
            &g_params,
            Some(&[ev2_g[0].clone(), ev2_g[1].clone()]),
            &[assign("x", var("a"))],
            &ctx,
        );
        assert_eq!(env.get_slot("x"), LatticeTy::known(Type::F64));
    }

    /// 跨函数二跳：实参为调用表达式 ⇒ 用 body_rets 查型作证据（批 936）。
    #[test]
    fn call_arg_evidence_via_body_rets() {
        let mk_def = |name: &str, params: Vec<(String, String)>, body: Vec<AstNode>| {
            AstNode::FuncDef {
                name: name.to_string(),
                generics: vec![],
                lifetimes: vec![],
                params,
                ret: String::new(),
                body,
                attrs: vec![],
                ret_expr: None,
                single_line: true,
                doc: String::new(),
                pub_: false,
                async_: false,
                const_: false,
                comptime_: false,
                where_clauses: vec![],
            }
        };
        // def get_data(): return [1.0, 2.0, 4.0]
        let get_data = mk_def(
            "get_data",
            vec![],
            vec![AstNode::Return(Box::new(AstNode::ArrayLit(vec![
                AstNode::FloatLit("1.0".to_string()),
                AstNode::FloatLit("2.0".to_string()),
                AstNode::FloatLit("4.0".to_string()),
            ])))],
        );
        // def show(data): pass —— 调用点 show(get_data())
        let show = mk_def(
            "show",
            vec![("data".to_string(), String::new())],
            vec![AstNode::ExprStmt {
                expr: Box::new(AstNode::Lit(0)),
            }],
        );
        let caller = mk_def(
            "caller",
            vec![],
            vec![AstNode::ExprStmt {
                expr: Box::new(AstNode::Call {
                    receiver: None,
                    method: "show".to_string(),
                    args: vec![AstNode::Call {
                        receiver: None,
                        method: "get_data".to_string(),
                        args: vec![],
                        type_args: vec![],
                        structural: false,
                    }],
                    type_args: vec![],
                    structural: false,
                }),
            }],
        );
        let mut registered = HashMap::new();
        registered.insert("get_data".to_string(), get_data);
        registered.insert("show".to_string(), show);
        registered.insert("caller".to_string(), caller);

        let ev0 = collect_param_evidence(&registered, None, None);
        let body_rets = collect_module_body_rets(
            &registered,
            &ev0,
            &HashMap::new(),
            &HashMap::new(),
            &Default::default(),
        );
        assert_eq!(
            body_rets.get("get_data"),
            Some(&Type::DynamicArray(Box::new(Type::F64)))
        );
        let ev1 = collect_param_evidence(&registered, Some(&body_rets), None);
        let ev_show = ev1.get("show").expect("show 证据");
        assert_eq!(
            ev_show[0],
            Some(Type::DynamicArray(Box::new(Type::F64)))
        );
    }

    /// 跨函数三跳：实参为变量 ⇒ 查调用函数推断后的槽型（批 937）。
    #[test]
    fn call_arg_evidence_via_caller_envs() {
        let mk_def = |name: &str, params: Vec<(String, String)>, body: Vec<AstNode>| {
            AstNode::FuncDef {
                name: name.to_string(),
                generics: vec![],
                lifetimes: vec![],
                params,
                ret: String::new(),
                body,
                attrs: vec![],
                ret_expr: None,
                single_line: true,
                doc: String::new(),
                pub_: false,
                async_: false,
                const_: false,
                comptime_: false,
                where_clauses: vec![],
            }
        };
        let get_data = mk_def(
            "get_data",
            vec![],
            vec![AstNode::Return(Box::new(AstNode::ArrayLit(vec![
                AstNode::FloatLit("1.0".to_string()),
                AstNode::FloatLit("2.0".to_string()),
            ])))],
        );
        let show = mk_def(
            "show",
            vec![("data".to_string(), String::new())],
            vec![AstNode::ExprStmt {
                expr: Box::new(AstNode::Lit(0)),
            }],
        );
        // def caller(): xs = get_data(); show(xs)
        let caller = mk_def(
            "caller",
            vec![],
            vec![
                assign("xs", AstNode::Call {
                    receiver: None,
                    method: "get_data".to_string(),
                    args: vec![],
                    type_args: vec![],
                    structural: false,
                }),
                AstNode::ExprStmt {
                    expr: Box::new(AstNode::Call {
                        receiver: None,
                        method: "show".to_string(),
                        args: vec![var("xs")],
                        type_args: vec![],
                        structural: false,
                    }),
                },
            ],
        );
        let mut registered = HashMap::new();
        registered.insert("get_data".to_string(), get_data);
        registered.insert("show".to_string(), show);
        registered.insert("caller".to_string(), caller);

        // 轮 1：字面量证据 → body_rets
        let ev0 = collect_param_evidence(&registered, None, None);
        let body_rets = collect_module_body_rets(
            &registered,
            &ev0,
            &HashMap::new(),
            &HashMap::new(),
            &Default::default(),
        );
        // 轮 2：每函数推断缓存 env（用 ev0；ctx 的查表并入 body_rets 后略——
        // 本测试 xs 的型走 ret_types 查表，需把 body_rets 并进查表表）
        let mut lookup = body_rets.clone();
        let ctx = InferCtx {
            ret_types: &lookup,
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        let mut envs: HashMap<String, TypeEnv> = HashMap::new();
        for (name, def) in &registered {
            if let AstNode::FuncDef { params, body, .. } = def {
                let mut env = TypeEnv::new();
                let ev_fn = ev0.get(name);
                let ev_slice = ev_fn.map(|v| v.as_slice());
                infer_fn_body_full(&mut env, name, params, ev_slice, body, &ctx);
                envs.insert(name.clone(), env);
            }
        }
        // 轮 3：带 envs 的证据
        let ev1 = collect_param_evidence(&registered, Some(&body_rets), Some(&envs));
        let ev_show = ev1.get("show").expect("show 证据");
        assert_eq!(
            ev_show[0],
            Some(Type::DynamicArray(Box::new(Type::F64)))
        );
    }

    /// plan 层：顶层调用点证据并入（批 940）。
    #[test]
    fn plan_merges_top_level_call_evidence() {
        let mk_def = |name: &str, params: Vec<(String, String)>, body: Vec<AstNode>| {
            AstNode::FuncDef {
                name: name.to_string(),
                generics: vec![],
                lifetimes: vec![],
                params,
                ret: String::new(),
                body,
                attrs: vec![],
                ret_expr: None,
                single_line: true,
                doc: String::new(),
                pub_: false,
                async_: false,
                const_: false,
                comptime_: false,
                where_clauses: vec![],
            }
        };
        let get_data = mk_def(
            "get_data",
            vec![],
            vec![AstNode::Return(Box::new(AstNode::ArrayLit(vec![
                AstNode::FloatLit("1.0".to_string()),
            ])))],
        );
        let show = mk_def(
            "show",
            vec![("data".to_string(), String::new())],
            vec![AstNode::ExprStmt {
                expr: Box::new(AstNode::Lit(0)),
            }],
        );
        let mut registered = HashMap::new();
        registered.insert("get_data".to_string(), get_data);
        registered.insert("show".to_string(), show);
        // 顶层：show(get_data())
        let top = vec![AstNode::ExprStmt {
            expr: Box::new(AstNode::Call {
                receiver: None,
                method: "show".to_string(),
                args: vec![AstNode::Call {
                    receiver: None,
                    method: "get_data".to_string(),
                    args: vec![],
                    type_args: vec![],
                    structural: false,
                }],
                type_args: vec![],
                structural: false,
            }),
        }];
        let plan = build_module_checker_plan(
            &registered,
            &HashMap::new(),
            &HashMap::new(),
            &Default::default(),
            &top,
        );
        let ev_show = plan.evidence.get("show").expect("show 证据");
        assert_eq!(
            ev_show[0],
            Some(Type::DynamicArray(Box::new(Type::F64))),
            "顶层 show(get_data()) ⇒ data 位证据 DynamicArray(F64)"
        );
    }

    /// return 字段访问：参数槽 Named(Point) ⇒ v.px 的字段型（批 944）。
    #[test]
    fn return_field_access_types_field() {
        let mut env = TypeEnv::new();
        let mut decls: HashMap<String, crate::middle::mir::r#gen::TypeDecl> =
            HashMap::new();
        decls.insert(
            "Point".to_string(),
            crate::middle::mir::r#gen::TypeDecl::Struct {
                fields: vec![("px".to_string(), "f64".to_string())],
                generics: vec![],
            },
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &decls,
            module_globals: &Default::default(),
        };
        // 参数证据：v ⇒ Named(Point)（模拟调用点 p = Point() 的三跳链）
        let evidence = vec![Some(Type::Named("Point".to_string(), vec![]))];
        infer_fn_body_full(
            &mut env,
            "getx",
            &[("v".to_string(), String::new())],
            Some(&evidence),
            &[AstNode::Return(Box::new(AstNode::FieldAccess {
                base: Box::new(var("v")),
                field: "px".to_string(),
            }))],
            &ctx,
        );
        assert_eq!(
            env.fn_rets.get("getx"),
            Some(&Type::F64),
            "return v.px ⇒ 字段型 F64 进 fn_rets"
        );
    }

    /// and 组合窄化：isinstance(x, T) and <其他> ⇒ then 内 x 窄化（批 946）。
    #[test]
    fn and_composed_isinstance_narrows() {
        let mut env = TypeEnv::new();
        let mut decls: HashMap<String, crate::middle::mir::r#gen::TypeDecl> =
            HashMap::new();
        decls.insert(
            "Point".to_string(),
            crate::middle::mir::r#gen::TypeDecl::Struct {
                fields: vec![("px".to_string(), "f64".to_string())],
                generics: vec![],
            },
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &decls,
            module_globals: &Default::default(),
        };
        let is_inst = |x: &str, t: &str| {
            AstNode::Call {
                receiver: None,
                method: "isinstance".to_string(),
                args: vec![var(x), var(t)],
                type_args: vec![],
                structural: false,
            }
        };
        // if isinstance(p, Point) and n > 0: y = p.px
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::If {
                cond: Box::new(AstNode::BinaryOp {
                    op: "&&".to_string(),
                    left: Box::new(is_inst("p", "Point")),
                    right: Box::new(AstNode::BinaryOp {
                        op: ">".to_string(),
                        left: Box::new(var("n")),
                        right: Box::new(AstNode::Lit(0)),
                    }),
                }),
                then: vec![assign(
                    "y",
                    AstNode::FieldAccess {
                        base: Box::new(var("p")),
                        field: "px".to_string(),
                    },
                )],
                else_: vec![],
            }],
            &ctx,
        );
        assert_eq!(
            env.get_slot("y"),
            LatticeTy::known(Type::F64),
            "and 左侧 isinstance ⇒ then 内 p 窄化 ⇒ 字段 F64"
        );
        assert_eq!(env.get_slot("p"), LatticeTy::Unknown, "汇合不回流");
    }

    /// and 双 isinstance：两个变量同时窄化（批 946）。
    #[test]
    fn and_double_isinstance_narrows_both() {
        let mut env = TypeEnv::new();
        let mut decls: HashMap<String, crate::middle::mir::r#gen::TypeDecl> =
            HashMap::new();
        for (cn, f) in [("Point", "px"), ("Size", "sw")] {
            decls.insert(
                cn.to_string(),
                crate::middle::mir::r#gen::TypeDecl::Struct {
                    fields: vec![(f.to_string(), "f64".to_string())],
                    generics: vec![],
                },
            );
        }
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &decls,
            module_globals: &Default::default(),
        };
        let is_inst = |x: &str, t: &str| {
            AstNode::Call {
                receiver: None,
                method: "isinstance".to_string(),
                args: vec![var(x), var(t)],
                type_args: vec![],
                structural: false,
            }
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::If {
                cond: Box::new(AstNode::BinaryOp {
                    op: "&&".to_string(),
                    left: Box::new(is_inst("a", "Point")),
                    right: Box::new(is_inst("b", "Size")),
                }),
                then: vec![
                    assign(
                        "u",
                        AstNode::FieldAccess {
                            base: Box::new(var("a")),
                            field: "px".to_string(),
                        },
                    ),
                    assign(
                        "v",
                        AstNode::FieldAccess {
                            base: Box::new(var("b")),
                            field: "sw".to_string(),
                        },
                    ),
                ],
                else_: vec![],
            }],
            &ctx,
        );
        assert_eq!(env.get_slot("u"), LatticeTy::known(Type::F64));
        assert_eq!(env.get_slot("v"), LatticeTy::known(Type::F64));
    }

    /// or 异型：isinstance(x, A) or isinstance(x, B) ⇒ 不窄化（并集
    /// 不可表达，批 946）。
    #[test]
    fn or_mixed_types_no_narrow() {
        let mut env = TypeEnv::new();
        let mut decls: HashMap<String, crate::middle::mir::r#gen::TypeDecl> =
            HashMap::new();
        for cn in ["Point", "Size"] {
            decls.insert(
                cn.to_string(),
                crate::middle::mir::r#gen::TypeDecl::Struct {
                    fields: vec![("px".to_string(), "f64".to_string())],
                    generics: vec![],
                },
            );
        }
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &decls,
            module_globals: &Default::default(),
        };
        let is_inst = |x: &str, t: &str| {
            AstNode::Call {
                receiver: None,
                method: "isinstance".to_string(),
                args: vec![var(x), var(t)],
                type_args: vec![],
                structural: false,
            }
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::If {
                cond: Box::new(AstNode::BinaryOp {
                    op: "||".to_string(),
                    left: Box::new(is_inst("x", "Point")),
                    right: Box::new(is_inst("x", "Size")),
                }),
                then: vec![assign(
                    "y",
                    AstNode::FieldAccess {
                        base: Box::new(var("x")),
                        field: "px".to_string(),
                    },
                )],
                else_: vec![],
            }],
            &ctx,
        );
        assert_ne!(
            env.get_slot("y"),
            LatticeTy::known(Type::F64),
            "or 异型 ⇒ x 不窄化 ⇒ 字段读推不出"
        );
    }

    /// return -v：操作数槽 F64 ⇒ 返回型 F64（批 968，复用 is_numeric_ty）。
    #[test]
    fn return_unary_neg_types_fn() {
        let mut env = TypeEnv::new();
        env.meet_slot("v", LatticeTy::known(Type::F64));
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "neg",
            &[AstNode::Return(Box::new(AstNode::UnaryOp {
                op: "-".to_string(),
                expr: Box::new(var("v")),
            }))],
            &ctx,
        );
        assert_eq!(env.fn_rets.get("neg"), Some(&Type::F64));
    }

    /// return !b：操作数槽 Bool ⇒ Bool（批 968，not ⇒ Bool 正确传播）。
    #[test]
    fn return_unary_not_types_bool() {
        let mut env = TypeEnv::new();
        env.meet_slot("b", LatticeTy::known(Type::Bool));
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "neg",
            &[AstNode::Return(Box::new(AstNode::UnaryOp {
                op: "not".to_string(),
                expr: Box::new(var("b")),
            }))],
            &ctx,
        );
        assert_eq!(
            env.fn_rets.get("neg"),
            Some(&Type::Bool),
            "not ⇒ Bool 正确传播"
        );
    }

    /// map 下标读 ⇒ 值型（非键型）——批 923 键值误用的回归锁
    ///（t485 实拍：v = m["a"] 的 v 曾被给键型 Str，v[0] 槽标 Str ⇒
    /// print 走 strlen SEGV）。
    #[test]
    fn map_subscript_read_yields_value_type() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "m",
            LatticeTy::known(Type::Named(
                "map".to_string(),
                vec![Type::Str, Type::I64],
            )),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign("v", AstNode::Subscript {
                base: Box::new(var("m")),
                index: Box::new(AstNode::StringLit("a".to_string())),
            })],
            &ctx,
        );
        assert_eq!(
            env.get_slot("v"),
            LatticeTy::known(Type::I64),
            "m[\"a\"] ⇒ 值型 I64（非键型 Str）"
        );
    }

    /// plan 层：顶层 Var 实参证据查 module_env 精化槽（批 947）。
    #[test]
    fn plan_top_level_var_evidence_uses_module_env() {
        let mk_def = |name: &str, params: Vec<(String, String)>, body: Vec<AstNode>| {
            AstNode::FuncDef {
                name: name.to_string(),
                generics: vec![],
                lifetimes: vec![],
                params,
                ret: String::new(),
                body,
                attrs: vec![],
                ret_expr: None,
                single_line: true,
                doc: String::new(),
                pub_: false,
                async_: false,
                const_: false,
                comptime_: false,
                where_clauses: vec![],
            }
        };
        // def get(dd, k): return dd[k]
        let get = mk_def(
            "get",
            vec![
                ("dd".to_string(), String::new()),
                ("k".to_string(), String::new()),
            ],
            vec![AstNode::Return(Box::new(AstNode::Subscript {
                base: Box::new(var("dd")),
                index: Box::new(var("k")),
            }))],
        );
        let mut registered = HashMap::new();
        registered.insert("get".to_string(), get);
        // 顶层：d = {}; d["a"] = 1.5; get(d, "a")
        let top = vec![
            assign("d", AstNode::DictLit { entries: vec![] }),
            AstNode::Assign(
                Box::new(AstNode::Subscript {
                    base: Box::new(var("d")),
                    index: Box::new(AstNode::StringLit("a".to_string())),
                }),
                Box::new(AstNode::FloatLit("1.5".to_string())),
            ),
            AstNode::ExprStmt {
                expr: Box::new(AstNode::Call {
                    receiver: None,
                    method: "get".to_string(),
                    args: vec![var("d"), AstNode::StringLit("a".to_string())],
                    type_args: vec![],
                    structural: false,
                }),
            },
        ];
        // d 是模块全局（resolver 的 module_globals 名单）——批 954 起种
        // 子只保留名单内的槽
        let mut globals = std::collections::HashSet::new();
        globals.insert("d".to_string());
        let plan = build_module_checker_plan(
            &registered,
            &HashMap::new(),
            &HashMap::new(),
            &globals,
            &top,
        );
        // d 的槽被写侧精化成 map[Str,F64]，顶层 Var 实参证据 ⇒ get.dd 位
        assert_eq!(
            plan.module_env.get_slot("d"),
            LatticeTy::known(Type::Named(
                "map".to_string(),
                vec![Type::Str, Type::F64]
            ))
        );
        let ev = plan.evidence.get("get").expect("get 证据");
        assert_eq!(
            ev[0],
            Some(Type::Named(
                "map".to_string(),
                vec![Type::Str, Type::F64]
            )),
            "顶层 get(d, \"a\") ⇒ dd 位证据 = d 的精化槽"
        );
    }

    /// d[k] = v 写侧值型精化（批 947）：d = {} 后写 f64 ⇒ 槽
    /// map[I64,F64]；异型二写 ⇒ 值型退化 PyDynamic（静态 per-cell 语义）。
    #[test]
    fn subscript_assign_refines_map_value() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        let sub_assign = |k: AstNode, v: AstNode| {
            AstNode::Assign(
                Box::new(AstNode::Subscript {
                    base: Box::new(var("d")),
                    index: Box::new(k),
                }),
                Box::new(v),
            )
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                assign("d", AstNode::DictLit { entries: vec![] }),
                sub_assign(
                    AstNode::StringLit("a".to_string()),
                    AstNode::FloatLit("1.5".to_string()),
                ),
                sub_assign(
                    AstNode::StringLit("b".to_string()),
                    AstNode::StringLit("s".to_string()),
                ),
            ],
            &ctx,
        );
        assert_eq!(
            env.get_slot("d"),
            LatticeTy::known(Type::Named(
                "map".to_string(),
                vec![Type::Str, Type::PyDynamic]
            )),
            "Str 键精化＋f64 首插精化；异型二写 ⇒ 值型 PyDynamic"
        );
    }

    /// 构造调用返回型：p = Point() ⇒ Named(Point)（type_decls 在册，批 943）。
    #[test]
    fn ctor_call_types_named() {
        let mut env = TypeEnv::new();
        let mut decls: HashMap<String, crate::middle::mir::r#gen::TypeDecl> =
            HashMap::new();
        decls.insert(
            "Point".to_string(),
            crate::middle::mir::r#gen::TypeDecl::Struct {
                fields: vec![],
                generics: vec![],
            },
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &decls,
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign("p", call("Point", vec![]))],
            &ctx,
        );
        assert_eq!(
            env.get_slot("p"),
            LatticeTy::known(Type::Named("Point".to_string(), vec![]))
        );
    }

    /// 全局槽种子注入：模块级槽型进函数 env，同名参数遮蔽（批 942）。
    #[test]
    fn module_slot_seeding_skips_params() {
        let mut module_env = TypeEnv::new();
        module_env.meet_slot(
            "G",
            LatticeTy::known(Type::DynamicArray(Box::new(Type::F64))),
        );
        module_env.meet_slot("x", LatticeTy::known(Type::I64));
        let mut env = TypeEnv::new();
        // 函数参数 x 遮蔽全局 x
        seed_module_slots(&mut env, &module_env, &["x".to_string()]);
        assert_eq!(
            env.get_slot("G"),
            LatticeTy::known(Type::DynamicArray(Box::new(Type::F64)))
        );
        assert_eq!(env.get_slot("x"), LatticeTy::Unknown, "同名参数遮蔽全局");
    }

    /// 控制流窄化：isinstance(x, T) 真分支内 x ⇒ Named(T)（字段访问/内建
    /// 窄化生效），分支汇合后窄化不回流（x 保持原状，批 941）。
    #[test]
    fn isinstance_narrows_in_then_branch() {
        let mut env = TypeEnv::new();
        let mut decls: HashMap<String, crate::middle::mir::r#gen::TypeDecl> =
            HashMap::new();
        decls.insert(
            "Point".to_string(),
            crate::middle::mir::r#gen::TypeDecl::Struct {
                fields: vec![("px".to_string(), "f64".to_string())],
                generics: vec![],
            },
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &decls,
            module_globals: &Default::default(),
        };
        let is_inst = |x: &str, t: &str| {
            AstNode::Call {
                receiver: None,
                method: "isinstance".to_string(),
                args: vec![var(x), var(t)],
                type_args: vec![],
                structural: false,
            }
        };
        // if isinstance(p, Point): y = p.px
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::If {
                cond: Box::new(is_inst("p", "Point")),
                then: vec![assign(
                    "y",
                    AstNode::FieldAccess {
                        base: Box::new(var("p")),
                        field: "px".to_string(),
                    },
                )],
                else_: vec![],
            }],
            &ctx,
        );
        assert_eq!(
            env.get_slot("y"),
            LatticeTy::known(Type::F64),
            "窄化后 p: Named(Point) ⇒ 字段 px ⇒ F64"
        );
        assert_eq!(
            env.get_slot("p"),
            LatticeTy::Unknown,
            "分支汇合后窄化不回流"
        );
    }

    /// isinstance(n, int) 真分支内 n ⇒ I64（内建标量窄化，批 941）。
    #[test]
    fn isinstance_builtin_narrows() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::If {
                cond: Box::new(AstNode::Call {
                    receiver: None,
                    method: "isinstance".to_string(),
                    args: vec![var("n"), var("int")],
                    type_args: vec![],
                    structural: false,
                }),
                then: vec![assign(
                    "m",
                    AstNode::BinaryOp {
                        op: "*".to_string(),
                        left: Box::new(var("n")),
                        right: Box::new(AstNode::Lit(2)),
                    },
                )],
                else_: vec![],
            }],
            &ctx,
        );
        assert_eq!(
            env.get_slot("m"),
            LatticeTy::known(Type::I64),
            "窄化 n: I64 ⇒ n*2 ⇒ I64"
        );
    }

    /// 模块级 body 返回型收集（批 935）：def f(): return 1.5 ⇒ f ⇒ F64。
    #[test]
    fn module_body_rets_collected() {
        let f = AstNode::FuncDef {
            name: "f".to_string(),
            generics: vec![],
            lifetimes: vec![],
            params: vec![],
            ret: String::new(),
            body: vec![AstNode::Return(Box::new(AstNode::FloatLit(
                "1.5".to_string(),
            )))],
            attrs: vec![],
            ret_expr: None,
            single_line: true,
            doc: String::new(),
            pub_: false,
            async_: false,
            const_: false,
            comptime_: false,
            where_clauses: vec![],
        };
        let mut registered = HashMap::new();
        registered.insert("f".to_string(), f);
        let rets = collect_module_body_rets(
            &registered,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &Default::default(),
        );
        assert_eq!(rets.get("f"), Some(&Type::F64));
    }

    /// 字典字面量：键 Str 保 Str 否则 I64；值同型⇒该型、混型⇒PyDynamic、
    /// 空⇒map[I64,I64]（对齐 gen/call_dict.rs 缺省，批 933）。
    #[test]
    fn dict_lit_types_map() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        let kv = |k: AstNode, v: AstNode| (k, v);
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                assign(
                    "d1",
                    AstNode::DictLit {
                        entries: vec![kv(
                            AstNode::StringLit("a".to_string()),
                            AstNode::Lit(1),
                        )],
                    },
                ),
                assign(
                    "d2",
                    AstNode::DictLit {
                        entries: vec![
                            kv(
                                AstNode::StringLit("a".to_string()),
                                AstNode::Lit(1),
                            ),
                            kv(
                                AstNode::StringLit("b".to_string()),
                                AstNode::StringLit("s".to_string()),
                            ),
                        ],
                    },
                ),
                assign("d3", AstNode::DictLit { entries: vec![] }),
            ],
            &ctx,
        );
        assert_eq!(
            env.get_slot("d1"),
            LatticeTy::known(Type::Named(
                "map".to_string(),
                vec![Type::Str, Type::I64]
            ))
        );
        assert_eq!(
            env.get_slot("d2"),
            LatticeTy::known(Type::Named(
                "map".to_string(),
                vec![Type::Str, Type::PyDynamic]
            ))
        );
        assert_eq!(
            env.get_slot("d3"),
            LatticeTy::known(Type::Named(
                "map".to_string(),
                vec![Type::I64, Type::I64]
            ))
        );
    }

    /// len(x) ⇒ I64 恒成立（批 930）。
    #[test]
    fn len_call_types_i64() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign("n", call("len", vec![var("xs")]))],
            &ctx,
        );
        assert_eq!(env.get_slot("n"), LatticeTy::known(Type::I64));
    }

    /// for i, x in enumerate(arr)：元组模式逐分量推断（批 930）。
    #[test]
    fn for_enumerate_tuple_pattern() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "arr",
            LatticeTy::known(Type::DynamicArray(Box::new(Type::F64))),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[AstNode::For {
                pattern: Box::new(AstNode::Tuple(vec![var("i"), var("x")])),
                expr: Box::new(call("enumerate", vec![var("arr")])),
                body: vec![],
                else_body: vec![],
            }],
            &ctx,
        );
        assert_eq!(env.get_slot("i"), LatticeTy::known(Type::I64));
        assert_eq!(env.get_slot("x"), LatticeTy::known(Type::F64));
    }

    /// arr.pop() ⇒ 元素型（批 930）。
    #[test]
    fn pop_types_element() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "xs",
            LatticeTy::known(Type::DynamicArray(Box::new(Type::Str))),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign(
                "last",
                AstNode::Call {
                    receiver: Some(Box::new(var("xs"))),
                    method: "pop".to_string(),
                    args: vec![],
                    type_args: vec![],
                    structural: false,
                },
            )],
            &ctx,
        );
        assert_eq!(env.get_slot("last"), LatticeTy::known(Type::Str));
    }

    /// 函数参数注解 ⇒ 参数槽型（批 931）；无注解参数不动。
    #[test]
    fn param_annotations_types_slots() {
        let mut env = TypeEnv::new();
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        let params = vec![
            ("data".to_string(), "f64".to_string()),
            ("n".to_string(), String::new()),
        ];
        infer_fn_body_with_params(
            &mut env,
            "f",
            &params,
            &[assign("x", var("data"))],
            &ctx,
        );
        assert_eq!(env.get_slot("data"), LatticeTy::known(Type::F64));
        assert_eq!(env.get_slot("x"), LatticeTy::known(Type::F64));
        assert_eq!(env.get_slot("n"), LatticeTy::Unknown);
    }

    /// 方法调用 method_ret 查表：接收者 Named(tag) ⇒ 表列返回型
    /// （批 916 臂此前不可达，批 930 并入后补此测试）。
    #[test]
    fn method_call_ret_table_propagates() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "d",
            LatticeTy::known(Type::Named("map".to_string(), vec![Type::Str, Type::I64])),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        // method_ret("map", "keys") 的表列返回型（有则断言 Known，无则跳过）
        if let Some(handle) = crate::middle::pylib::method_ret("map", "keys") {
            let expect = match handle {
                "str" => Type::Str,
                "f64" => Type::F64,
                other => Type::Named(other.to_string(), vec![]),
            };
            infer_fn_body(
                &mut env,
                "test_fn",
                &[assign(
                    "ks",
                    AstNode::Call {
                        receiver: Some(Box::new(var("d"))),
                        method: "keys".to_string(),
                        args: vec![],
                        type_args: vec![],
                        structural: false,
                    },
                )],
                &ctx,
            );
            assert_eq!(env.get_slot("ks"), LatticeTy::known(expect));
        }
    }

    /// abs(x) 操作数槽已知数值 ⇒ 同型；min/max 双同型 ⇒ 同型（批 932）。
    #[test]
    fn scalar_abs_minmax_propagates() {
        let mut env = TypeEnv::new();
        env.meet_slot("v", LatticeTy::known(Type::F64));
        env.meet_slot("a", LatticeTy::known(Type::I64));
        env.meet_slot("b", LatticeTy::known(Type::I64));
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[
                assign("m", call("abs", vec![var("v")])),
                assign("lo", call("min", vec![var("a"), var("b")])),
                assign("hi", call("max", vec![var("a"), var("b")])),
            ],
            &ctx,
        );
        assert_eq!(env.get_slot("m"), LatticeTy::known(Type::F64));
        assert_eq!(env.get_slot("lo"), LatticeTy::known(Type::I64));
        assert_eq!(env.get_slot("hi"), LatticeTy::known(Type::I64));
    }

    /// sorted(xs) ⇒ DynamicArray(元素型)（批 930）。
    #[test]
    fn sorted_types_dynamic_array() {
        let mut env = TypeEnv::new();
        env.meet_slot(
            "xs",
            LatticeTy::known(Type::DynamicArray(Box::new(Type::F64))),
        );
        let ctx = InferCtx {
            ret_types: &HashMap::new(),
            type_decls: &HashMap::new(),
            module_globals: &Default::default(),
        };
        infer_fn_body(
            &mut env,
            "test_fn",
            &[assign("s", call("sorted", vec![var("xs")]))],
            &ctx,
        );
        assert_eq!(
            env.get_slot("s"),
            LatticeTy::known(Type::DynamicArray(Box::new(Type::F64)))
        );
    }
}

#[cfg(test)]
mod tests_954 {
    use super::*;

    fn assign(lhs: &str, rhs: AstNode) -> AstNode {
        AstNode::Assign(
            Box::new(AstNode::Var(lhs.to_string())),
            Box::new(rhs),
        )
    }

    /// 批 954：module_env 种子只保留 module_globals 名单内的槽——
    /// 合成 main 体内部的局部变量不跨函数泄漏。
    #[test]
    fn module_env_filtered_to_declared_globals() {
        let mk_def = |name: &str, body: Vec<AstNode>| AstNode::FuncDef {
            name: name.to_string(),
            generics: vec![],
            lifetimes: vec![],
            params: vec![],
            ret: String::new(),
            body,
            attrs: vec![],
            ret_expr: None,
            single_line: true,
            doc: String::new(),
            pub_: false,
            async_: false,
            const_: false,
            comptime_: false,
            where_clauses: vec![],
        };
        // main 体：G = [1.0]（全局）＋ local = 5（main 局部，不应入种子）
        let main = mk_def(
            "main",
            vec![
                assign(
                    "G",
                    AstNode::ArrayLit(vec![AstNode::FloatLit("1.0".to_string())]),
                ),
                assign("local", AstNode::Lit(5)),
            ],
        );
        let mut registered = HashMap::new();
        registered.insert("main".to_string(), main);
        let mut globals = std::collections::HashSet::new();
        globals.insert("G".to_string());
        let plan = build_module_checker_plan(
            &registered,
            &HashMap::new(),
            &HashMap::new(),
            &globals,
            &[],
        );
        assert_eq!(
            plan.module_env.get_slot("G"),
            LatticeTy::known(Type::DynamicArray(Box::new(Type::F64)))
        );
        assert_eq!(
            plan.module_env.get_slot("local"),
            LatticeTy::Unknown,
            "main 局部变量不入全局种子"
        );
    }
}
