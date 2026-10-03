//! 批次 854：Loop 无限循环发射体。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
PY-A Loop 无限循环发射体——原臂逐字。
pub(super) fn lower_loop(&mut self, body: &Box<AstNode>, dest: u32) {
                    tail = body.last();
                }
                match tail {
                    Some(AstNode::ExprStmt { expr }) | Some(AstNode::Return(expr)) => {
                        ast_ty(expr)
                    }
                    Some(AstNode::Assign(_, rhs)) => ast_ty(rhs),
                    _ => None,
                }
            };
            let t = tail_of(then);
            let e = tail_of(else_);
            match (t, e) {
                (Some(a), Some(b)) if a == b => Some(a),
                // Mixed arms: a filtered comprehension is
                // `if cond { ELEMENT } else { -1 }` — the i64 sentinel
                // must not drag the result type to I64 (batch 293:
                // this made every str comprehension a DynamicArray
                // (I64), and the driver's trading-day filter then
                // pointer-compared to 0 days).
                (Some(Type::I64), Some(b)) => Some(b),
                (Some(a), Some(Type::I64)) => Some(a),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                _ => None,
            }
        };
        // Create destination for expression result
}
}
