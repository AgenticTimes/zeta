//! 批次 911（轴 F 方案② P1 骨架）：语句→约束提取。
//!
//! 从函数体 AST 提取"变量名→格值/传播边"约束（设计稿 §3 的五条来源）。
//! 本片实现字面量与注解两类（可纯逻辑测试）；赋值边与调用返回在
//! Checker::check_fn 的顺序扫描里产出（它们依赖扫描期状态）。

use crate::frontend::ast::AstNode;
use crate::middle::checker::lattice::LatticeTy;
use crate::middle::types::Type;

/// 字面量/注解能直接给出的格值（保守子集——其余语句产传播边）。
pub fn literal_lattice(expr: &AstNode) -> Option<LatticeTy> {
    match expr {
        AstNode::Lit(n) => Some(LatticeTy::known(Type::I64)),
        AstNode::Bool(b) => Some(LatticeTy::known(if *b {
            Type::I64
        } else {
            Type::I64
        })),
        AstNode::FloatLit(s) => Some(LatticeTy::known(Type::F64)),
        AstNode::StringLit(_) => Some(LatticeTy::known(Type::Str)),
        AstNode::NoneLit => Some(LatticeTy::known(Type::Named(
            "NoneValue".to_string(),
            vec![],
        ))),
        _ => None,
    }
}

/// 注解串 → 型（委托 Type::from_string，dict 已归一成 map）。
pub fn annotation_lattice(ty: &str) -> Option<LatticeTy> {
    Some(LatticeTy::known(Type::from_string(ty)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_int_yields_i64() {
        assert_eq!(
            literal_lattice(&AstNode::Lit(42)),
            Some(LatticeTy::known(Type::I64))
        );
    }

    #[test]
    fn literal_string_yields_str() {
        assert_eq!(
            literal_lattice(&AstNode::StringLit("s".to_string())),
            Some(LatticeTy::known(Type::Str))
        );
    }

    #[test]
    fn literal_float_yields_f64() {
        assert_eq!(
            literal_lattice(&AstNode::FloatLit("2.5".to_string())),
            Some(LatticeTy::known(Type::F64))
        );
    }

    #[test]
    fn none_yields_nonevalue() {
        assert_eq!(
            literal_lattice(&AstNode::NoneLit),
            Some(LatticeTy::known(Type::Named(
                "NoneValue".to_string(),
                vec![]
            )))
        );
    }

    #[test]
    fn call_yields_no_constraint() {
        let call = AstNode::Call {
            receiver: None,
            method: "foo".to_string(),
            args: vec![],
            type_args: vec![],
            structural: false,
        };
        assert_eq!(literal_lattice(&call), None);
    }

    #[test]
    fn annotation_known_spelling_yields_type() {
        // from_string 已支持的拼写：裸 "i64"/"str"/"dict"（dict 归一成 map）。
        let lat = annotation_lattice("str");
        assert_eq!(
            lat.and_then(|l| l.known_ty()),
            Some(Type::Str),
            "str 注解应有 Str 型"
        );
    }
}
