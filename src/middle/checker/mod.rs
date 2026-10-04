//! 批次 911（轴 F 方案② P1 骨架）：统一类型检查器——SCCP 式 slot→型不动点。
//!
//! 设计稿：docs/f2-checker-design.md。流程：
//! 1. 预填（参数注解／已知返回型／调用点实参型）
//! 2. 顺序扫函数体，每条语句产出约束入工作列表
//! 3. 弹出 Work → meet 更新槽型；变化 ⇒ 引用该槽的语句重新入列
//! 4. 双清空 ⇒ 不动点
//!
//! P1 骨架范围：TypeEnv＋逐语句顺序扫描＋赋值边与字面量（保守子集）；
//! 调用返回与运算结果约束在 P2 接入。**尚未接线**——不进任何现有管线。

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

/// 单函数体内的顺序扫描＋赋值边传播（P1 保守子集）：
/// - `x = <字面量>` ⇒ meet(x, 字面量格值)
/// - `x = <名字>`   ⇒ meet(x, 该名当前格值)（赋值边传播）
/// - 其他表达式不产约束。
pub fn infer_fn_body(env: &mut TypeEnv, body: &[AstNode]) {
    for stmt in body {
        if let AstNode::Assign(lhs, rhs) = stmt {
            if let AstNode::Var(name) = &**lhs {
                if let Some(lat) = constraint::literal_lattice(rhs) {
                    env.meet_slot(name, lat);
                    continue;
                }
                if let AstNode::Var(src) = &**rhs {
                    let src_lat = env.get_slot(src);
                    if src_lat.is_known() {
                        env.meet_slot(name, src_lat);
                    }
                    continue;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::AstNode;

    fn var(n: &str) -> AstNode {
        AstNode::Var(n.to_string())
    }

    fn assign(lhs: &str, rhs: AstNode) -> AstNode {
        AstNode::Assign(Box::new(var(lhs)), Box::new(rhs))
    }

    /// 批次 911：字面量赋值 ⇒ 槽型 Known(I64)。
    #[test]
    fn literal_assign_types_slot() {
        let mut env = TypeEnv::new();
        infer_fn_body(&mut env, &[assign("x", AstNode::Lit(42))]);
        assert_eq!(env.get_slot("x"), LatticeTy::known(Type::I64));
    }

    /// 赋值边传播：b = a ⇒ b 与 a 同格。
    #[test]
    fn assign_edge_propagates() {
        let mut env = TypeEnv::new();
        infer_fn_body(
            &mut env,
            &[assign("a", AstNode::Lit(7)), assign("b", var("a"))],
        );
        assert_eq!(env.get_slot("b"), env.get_slot("a"));
    }

    /// 冲突：同槽两种已知型 ⇒ Conflict（不静默选边）。
    #[test]
    fn conflicting_writes_yield_conflict() {
        let mut env = TypeEnv::new();
        infer_fn_body(
            &mut env,
            &[
                assign("x", AstNode::Lit(1)),
                assign("x", AstNode::StringLit("s".to_string())),
            ],
        );
        assert_eq!(env.get_slot("x"), LatticeTy::Conflict);
    }

    /// Unknown ⊕ Known ＝ Known（单位元，传播不受未知槽阻断）。
    #[test]
    fn unknown_meets_known_stays_known() {
        let mut env = TypeEnv::new();
        env.meet_slot("x", LatticeTy::known(Type::I64));
        assert_eq!(env.get_slot("x"), LatticeTy::known(Type::I64));
    }
}
