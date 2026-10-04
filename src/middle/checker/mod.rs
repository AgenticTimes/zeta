//! 批次 911（轴 F 方案② P1 骨架）：统一类型检查器——SCCP 式 slot→型不动点。
//!
//! 设计稿：docs/f2-checker-design.md。
//! 批次 915 扩展：递归扫描嵌套块（If/Loop）＋字段访问型传播＋
//! 方法返回型（method_ret）＋Return 语句收集。
//! 批次 916 扩展：方法调用返回型传播（method_ret 查表）＋
//! 下标结果型传播（元素型/映射值型）。

pub mod constraint;
pub mod lattice;

use crate::frontend::ast::AstNode;
use crate::middle::checker::lattice::LatticeTy;
use crate::middle::types::Type;
use std::collections::HashMap;

/// 全程序唯一的类型环境（设计稿 §7）。
#[derive(Debug, Default)]
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
    scan_stmts(env, body, ctx);
    // Return 语句收集→fn_rets（批 918：P4 回灌替换的前提）
    collect_fn_ret(env, fn_name, body, ctx);
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
            // 控制流：递归进分支体
            AstNode::If { then, else_, .. } => {
                scan_stmts(env, then, ctx);
                scan_stmts(env, else_, ctx);
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
    // 下标（批 923 重构：切片⇒DynamicArray(e)，取元素⇒e，map⇒V）
    if let AstNode::Subscript { base, index } = rhs {
        let is_slice = matches!(&**index, AstNode::Range { .. })
            || matches!(&**index, AstNode::BinaryOp { op, .. } if op == "..");
        if let AstNode::Var(base_name) = &**base {
            let base_lat = env.get_slot(base_name);
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
