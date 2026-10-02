//! 批次 816：集合家族的调用降级（gen.rs Call 巨臂拆家族第一刀）。
//! 按业界模板（rustc/Go/Swift 均按构造种类分文件）：一个家族一个文件、
//! 一张显式路由表。子模块可访问父模块 MirGen 私有字段。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// 集合族：add/discard/remove（写回接收者）与 intersection（返回新句柄）。
    /// 命中返回 Some(dest)；None = 不归本族，链上后续继续。
    pub(super) fn lower_set_family(
        &mut self,
        receiver: Option<&AstNode>,
        receiver_ty: &Type,
        method: &str,
        arg_ids: &[u32],
        dest: u32,
    ) -> Option<u32> {
                if matches!(method, "add" | "discard" | "remove")
                    && (matches!(receiver_ty, Type::DynamicArray(_) | Type::Array(_, _))
                        || matches!(receiver_ty, Type::Named(n, _) if n == "set" || n == "frozenset")
                        || (!matches!(receiver_ty, Type::Str)
                            && matches!(receiver_ty, Type::I64 | Type::PyDynamic)))
                {
                    if method == "add" && arg_ids.len() == 2 {
                        let elem_is_str = matches!(self.type_map.get(&arg_ids[1]), Some(Type::Str));
                        let flag = self.next_id();
                        self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                        self.type_map.insert(flag, Type::I64);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_add_unique".to_string(),
                            args: vec![arg_ids[0], arg_ids[1], flag],
                            dest: dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        // vec_push may reallocate and returns the new handle — write
                        // it back so a growing set is not silently lost.
                        if let Some(AstNode::Var(name)) = receiver.as_ref().map(|r| &**r) {
                            if let Some(&slot) = self.name_to_id.get(name) {
                                self.stmts.push(MirStmt::Assign { lhs: slot, rhs: dest });
                            }
                        }
                        self.type_map.insert(dest, receiver_ty.clone());
                        return Some(dest);
                    }
                    if arg_ids.len() == 2 {
                        let elem_is_str = matches!(
                            self.type_map.get(&arg_ids[1]),
                            Some(Type::Str)
                        );
                        let flag = self.next_id();
                        self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                        self.type_map.insert(flag, Type::I64);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_discard".to_string(),
                            args: vec![arg_ids[0], arg_ids[1], flag],
                            dest: dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        if let Some(AstNode::Var(name)) = receiver.as_ref().map(|r| &**r) {
                            if let Some(&slot) = self.name_to_id.get(name) {
                                self.stmts.push(MirStmt::Assign { lhs: slot, rhs: dest });
                            }
                        }
                        self.type_map.insert(dest, receiver_ty.clone());
                        return Some(dest);
                    }
                }
                // Batch 807: `sa.intersection(sb)` on a list-backed set. The
                // receiver's element type is what reached the ghost name
                // (`[dynamic]i64::intersection` / `[dynamic]str::…`), so batch
                // 428's rule turned the call site into a raise — measured 9 of
                // the 40 real strategy files writing
                // `list(set(temp).intersection(set(stockList)))`.
                // The result type INHERITS the receiver: a hard-coded I64 would
                // read a str column's handles as integers, swapping a loud raise
                // for a silent wrong value (this repo's worst class).
                if method == "intersection"
                    && arg_ids.len() == 2
                    && receiver.is_some()
                    && (matches!(receiver_ty, Type::DynamicArray(_) | Type::Array(_, _))
                        || matches!(receiver_ty, Type::Named(n, _) if n == "set" || n == "frozenset")
                        || (!matches!(receiver_ty, Type::Str)
                            && matches!(receiver_ty, Type::I64 | Type::PyDynamic)))
                {
                    let elem_of_str = |t: Option<&Type>| match t {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            matches!(**e, Type::Str)
                        }
                        Some(Type::Str) => true,
                        _ => false,
                    };
                    let elem_is_str =
                        elem_of_str(Some(receiver_ty)) || elem_of_str(self.type_map.get(&arg_ids[1]));
                    let flag = self.next_id();
                    self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                    self.type_map.insert(flag, Type::I64);
                    self.stmts.push(MirStmt::Call {
                        func: "py_vec_intersect".to_string(),
                        args: vec![arg_ids[0], arg_ids[1], flag],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    // The helper returns a FRESH vector and never mutates the
                    // receiver, so no write-back here (unlike add/discard above).
                    self.type_map.insert(dest, receiver_ty.clone());
                    return Some(dest);
                }
        None
    }
}
