//! 批次 860：Loop 无限循环发射体。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_loop(&mut self, body: &Box<AstNode>, dest: u32) -> u32 {
            // Loop expression: result slot (default 0); break EXPR writes it.
            let result_id = self.next_id();
            self.exprs.insert(result_id, MirExpr::IntLit(0));
            self.type_map.insert(result_id, Type::I64);
            self.loop_value_stack.push(result_id);

            let stmts_before = self.stmts.len();
            for stmt in body {
                self.lower_ast(stmt);
            }
            let loop_stmts = self.stmts.split_off(stmts_before);
            self.loop_value_stack.pop();

            let cond_id = self.next_id();
            self.exprs.insert(cond_id, MirExpr::IntLit(1));
            self.type_map.insert(cond_id, Type::I64);
            self.stmts.push(MirStmt::While {
                cond: cond_id,
                pre_cond: vec![],
                body: loop_stmts,
                else_body: vec![],
            });

            self.exprs.insert(result_id, MirExpr::Var(result_id));
            self.type_map.insert(result_id, Type::I64);
            self.exprs.insert(id, MirExpr::Var(result_id));
            self.type_map.insert(id, Type::I64);
            return result_id;
        dest
    }
}
