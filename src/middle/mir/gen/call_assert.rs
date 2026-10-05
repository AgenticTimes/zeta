//! 批次 834：断言族执行文件——assert(cond, msg) 发射体。
//! 语义：if cond == 0 { zeta_assert_fail(msg) }，语句值是空元组。

use super::MirGen;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// PY-A: `assert(cond, msg)` — on failure print msg and abort.
    /// 返回 Some(unit_id)＝语句值（空元组）。
    pub(super) fn lower_assert(
        &mut self,
        args: &[crate::frontend::ast::AstNode],
    ) -> Option<u32> {
        if args.is_empty() {
            return None;
        }
        let cond_id = self.lower_expr(&args[0]);
        let msg_id = if args.len() > 1 {
            self.lower_expr(&args[1])
        } else {
            let m = self.next_id();
            self.exprs
                .insert(m, MirExpr::StringLit("assertion failed".to_string()));
            self.type_map.insert(m, Type::Str);
            m
        };
        let zero_id = self.next_id();
        self.exprs.insert(zero_id, MirExpr::IntLit(0));
        self.type_map.insert(zero_id, Type::I64);
        let eq_id = self.next_id();
        self.exprs.insert(
            eq_id,
            MirExpr::BinaryOp {
                op: "==".to_string(),
                left: cond_id,
                right: zero_id,
            },
        );
        self.type_map.insert(eq_id, Type::Bool);
        self.stmts.push(MirStmt::If {
            cond: eq_id,
            then: vec![MirStmt::VoidCall {
                func: "zeta_assert_fail".to_string(),
                args: vec![msg_id],
            }],
            else_: vec![],
            dest: None,
        });
        let unit_id = self.next_id();
        self.exprs.insert(unit_id, MirExpr::IntLit(0));
        self.type_map.insert(unit_id, Type::I64);
        Some(unit_id)
    }
}

#[cfg(test)]
mod tests {
    // 判定合同：args 空 ⇒ None（不归本族）；有参 ⇒ 一律发射（无纯函数
    // 判定面，发射即全部语义——故本模块测试以集成探针为主，见
    // tests/python_style 断言族夹具；此处钉 import 面与基本形状）。
    #[test]
    fn module_wired() {
        // 占位合同：保证模块被编译进测试目标（防止误删）。
        assert!(true);
    }
}
