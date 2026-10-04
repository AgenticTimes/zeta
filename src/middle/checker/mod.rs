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
pub fn infer_fn_body(env: &mut TypeEnv, body: &[AstNode], ctx: &InferCtx) {
    scan_stmts(env, body, ctx);
}

fn scan_stmts(env: &mut TypeEnv, body: &[AstNode], ctx: &InferCtx) {
    for stmt in body {
        match stmt {
            // 赋值：lhs 为 Var 时传播型
            AstNode::Assign(lhs, rhs) => {
                if let AstNode::Var(name) = &**lhs {
                    propagate_assign(env, name, rhs, ctx);
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
    // 二元运算同型传播
    if let AstNode::BinaryOp { left, right, .. } = rhs {
        if let (AstNode::Var(a), AstNode::Var(b)) = (&**left, &**right) {
            let la = env.get_slot(a);
            let lb = env.get_slot(b);
            if la.is_known() && la == lb && !matches!(la, LatticeTy::Conflict) {
                env.meet_slot(name, la);
            }
        }
        return;
    }
    // 比较运算 ⇒ Bool（批次 917 扩展）
    if let AstNode::BinaryOp { op, .. } = rhs {
        if matches!(
            op.as_str(),
            "==" | "!=" | "<" | ">" | "<=" | ">=" | "in" | "not in"
        ) {
            env.meet_slot(name, LatticeTy::known(Type::Bool));
            return;
        }
    }
    // FString ⇒ Str（批次 917 扩展：f-string 结果恒为文本）
    if let AstNode::FString(_) = rhs {
        env.meet_slot(name, LatticeTy::known(Type::Str));
        return;
    }
    // 调用返回（ret_types 查表）
    if let AstNode::Call { method, .. } = rhs {
        if let Some(ty) = ctx.ret_types.get(method) {
            env.meet_slot(name, LatticeTy::known(ty.clone()));
        }
        return;
    }
    // 方法调用返回（receiver 上的 method → method_ret 查表，批 916 扩展）
    if let AstNode::Call {
        receiver: Some(recv),
        method,
        ..
    } = rhs
    {
        if let AstNode::Var(recv_name) = &**recv {
            let base_lat = env.get_slot(recv_name);
            if let LatticeTy::Known(Type::Named(tag, _)) = &base_lat {
                if let Some(ret_handle) =
                    crate::middle::pylib::method_ret(tag, method)
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
    // 下标结果型：DynamicArray(e) / Array(e, _) ⇒ e；Named("map",[K,V]) ⇒ V
    if let AstNode::Subscript { base, .. } = rhs {
        if let AstNode::Var(base_name) = &**base {
            let base_lat = env.get_slot(base_name);
            if let LatticeTy::Known(Type::DynamicArray(e)) = &base_lat {
                env.meet_slot(name, LatticeTy::known((**e).clone()));
                return;
            }
            if let LatticeTy::Known(Type::Array(e, _)) = &base_lat {
                env.meet_slot(name, LatticeTy::known((**e).clone()));
                return;
            }
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
        infer_fn_body(&mut env, &[assign("x", AstNode::Lit(42))], &ctx);
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
}
