//! 批次 951：Return 语句发射体（869 零适配法自 gen.rs 迁出）。
//! 元组返回构造堆数组（StackArray 指针随帧死，调用方解构读死栈——
//! 见函数体内实测批注）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_return_stmt(&mut self, inner: &AstNode) {
            // `return (a, b)` must hand back a HEAP array. A StackArray is an
            // alloca: the pointer dies with the frame, so the caller's
            // `stack_array_get` destructuring read dead stack (measured:
            // `return d.iloc[0:0], 7` → `len(o)` SEGV). Build a real
            // `[cap|len]` array — `stack_array_get(arr, i)` reads
            // `((i64*)arr)[i]`, i.e. exactly the dynarray DATA pointer that
            // `zeta_dynarray_new`/`vec_push` hand out.
            let mut val = if let AstNode::Tuple(items) = inner {
                let mut vals = Vec::with_capacity(items.len());
                let mut tys = Vec::with_capacity(items.len());
                for it in items {
                    let vid = self.lower_expr(it);
                    tys.push(self.type_map.get(&vid).cloned().unwrap_or_else(Type::slot_fallback));
                    vals.push(vid);
                }
                let cap = self.next_id_with_lit(items.len() as i64);
                let h = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: "zeta_dynarray_new".to_string(),
                    args: vec![cap],
                    dest: h,
                    type_args: vec![],
                });
                self.exprs.insert(h, MirExpr::Var(h));
                self.type_map.insert(
                    h,
                    Type::DynamicArray(Box::new(tys.first().cloned().unwrap_or_else(Type::slot_fallback))),
                );
                // Mirror the ArrayLit lowering EXACTLY: every push targets the
                // ORIGINAL handle `h` (the runtime grows it and returns a new
                // data pointer, which the codegen tracks via the call's dest),
                // and each dest is registered as its own Var. Chaining the
                // handles (`cur = pushed`) made `vec_push` receive an
                // unregistered slot and SEGV inside `vec_push + 24`.
                for v in vals {
                    let sink = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "vec_push".to_string(),
                        args: vec![h, v],
                        dest: sink,
                        type_args: vec![],
                    });
                    self.exprs.insert(sink, MirExpr::Var(sink));
                    self.type_map.insert(
                        sink,
                        Type::DynamicArray(Box::new(tys.first().cloned().unwrap_or_else(Type::slot_fallback))),
                    );
                }
                self.type_map.insert(h, Type::Tuple(tys));
                self.tuple_slots.insert(h);
                h
            } else {
                self.lower_expr(inner)
            };
            let val = self.coerce_return_val(val);
            self.stmts.push(MirStmt::Return { val });
    }
}
