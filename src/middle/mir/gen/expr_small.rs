//! 批次 891：lower_expr_node 剩余小臂（869 零适配法迁出）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {

    pub(super) fn lower_bigint_lit(&mut self, text: &String, id: u32) -> u32 {
            // Batch 647: a beyond-i64 literal lowers to the zeta_big
            // runtime handle ([lo|hi] 16-byte GC block, batch 641),
            // statically typed Named("BigInt") — arithmetic on it
            // routes through the big family, print/str render via
            // zeta_big_to_string.
            let v: i128 = text.parse().unwrap_or(0);
            let lo = self.int_slot(v as i64);
            let hi = self.int_slot((v >> 64) as i64);
            self.stmts.push(MirStmt::Call {
                func: "zeta_big_new".to_string(),
                args: vec![lo, hi],
                dest: id,
                type_args: vec![],
            });
            self.exprs.insert(id, MirExpr::Var(id));
            self.type_map
                .insert(id, Type::Named("BigInt".to_string(), vec![]));
    id
    }

    pub(super) fn lower_range_expr(
        &mut self,
        start: &Box<AstNode>,
        end: &Box<AstNode>,
        inclusive: &bool,
        id: u32,
    ) -> u32 {
            let start_id = self.lower_expr(start);
            let end_id = self.lower_expr(end);
            let dest = self.next_id();

            // Range expression for for loops
            self.exprs.insert(
                dest,
                MirExpr::Range {
                    start: start_id,
                    end: end_id,
                },
            );
            self.type_map.insert(dest, Type::Range);
            return dest;
    id
    }

    pub(super) fn lower_cast(&mut self, expr: &Box<AstNode>, ty: &String, id: u32) -> u32 {
            let expr_id = self.lower_expr(expr);
            let target_type = Type::from_string(ty);
            self.type_map.insert(id, target_type.clone());
            // Create As expression node
            self.exprs.insert(
                id,
                MirExpr::As {
                    expr: expr_id,
                    target_type,
                },
            );
    id
    }

    pub(super) fn lower_unsafe_expr(&mut self, body: &Vec<AstNode>, id: u32) -> u32 {
            // Evaluate the last expression in an unsafe block as the result
            if let Some(last) = body.last()
                && let AstNode::ExprStmt { expr } = last
            {
                return self.lower_expr(expr);
            }
            // Fallback: evaluate the whole body as statements
            for stmt in body {
                self.lower_ast(stmt);
            }
            self.exprs.insert(id, MirExpr::IntLit(0));
            self.type_map.insert(id, Type::I64);
    id
    }

    pub(super) fn lower_timing_owned(
        &mut self,
        ty: &String,
        inner: &Box<AstNode>,
        id: u32,
    ) -> u32 {
            // Timing-owned: wrap the inner expression.
            let inner_id = self.lower_expr(inner);
            self.exprs.insert(id, MirExpr::TimingOwned(inner_id));
            if let Some(ty) = self.type_map.get(&inner_id) {
                self.type_map.insert(id, ty.clone());
            } else {
                self.type_map.insert(id, Type::I64);
            }
    id
    }

}