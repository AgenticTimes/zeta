//! 批次 841：无接收者内建函数族第一片（map/filter/chr/ord/divmod/dict）。
//! if 串联臂逐字迁入；zip/any/all/enumerate/list/float/sorted 表段在
//! gen.rs 原位（批 842 续迁）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// 无接收者内建族第一片执行者。命中返回 Some(dest)；None 落链。
    pub(super) fn lower_builtin_1(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
                    // PY-A: map(f, xs) / filter(f, xs) — now that a bare function
                    // name is a FuncAddr this can call back into it. Eager (V1):
                    // the result is a Vec, not an iterator.
                    if receiver.is_none()
                        && (method == "map" || method == "filter")
                        && args.len() == 2
                    {
                        let f = self.lower_expr(&args[0]);
                        let xs = self.lower_expr(&args[1]);
                        let func = if method == "map" {
                            "py_builtin_map"
                        } else {
                            "py_builtin_filter"
                        };
                        self.stmts.push(MirStmt::Call {
                            func: func.to_string(),
                            args: vec![f, xs],
                            dest: dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map
                            .insert(dest, Type::DynamicArray(Box::new(Type::I64)));
                        return Some(dest);
                    }
                    // PY-A: chr(n) / ord(s) / divmod(a, b) / dict() — previously
                    // bare externs (link failure) or missing entirely.
                    if receiver.is_none() && method == "chr" && args.len() == 1 {
                        let a = self.lower_expr(&args[0]);
                        self.emit_call_into(dest, "py_builtin_chr", vec![a], Type::Str);
                        return Some(dest);
                    }
                    if receiver.is_none() && method == "ord" && args.len() == 1 {
                        let a = self.lower_expr(&args[0]);
                        self.emit_call_into(dest, "py_builtin_ord", vec![a], Type::I64);
                        return Some(dest);
                    }
                    // divmod(a, b) → (a // b, a % b) as a tuple, so
                    // `q, r = divmod(a, b)` unpacks via the call-return path.
                    // PY-A: the quotient must be FLOOR division — with `/` here the
                    // true-division fix would have made `divmod(7, 2)` answer 3.5.
                    if receiver.is_none() && method == "divmod" && args.len() == 2 {
                        let q = AstNode::BinaryOp {
                            op: "floordiv".to_string(),
                            left: Box::new(args[0].clone()),
                            right: Box::new(args[1].clone()),
                        };
                        let r = AstNode::BinaryOp {
                            op: "%".to_string(),
                            left: Box::new(args[0].clone()),
                            right: Box::new(args[1].clone()),
                        };
                        return Some(self.lower_expr(&AstNode::Tuple(vec![q, r])));
                    }
                    // dict() with no arguments is an empty map.
                    if receiver.is_none() && method == "dict" && args.is_empty() {
                        return Some(self.lower_expr(&AstNode::DictLit { entries: vec![] }));
                    }
                    // `dict(m)` is a SHALLOW COPY in Python, not an alias: mutating
                    // the result must not touch the source. Only done when the
                    // argument is statically a map — an unknown argument keeps the
                    // loud diagnostic (copying an unknown handle would corrupt data).
                    if receiver.is_none() && method == "dict" && args.len() == 1 {
                        let src_id = self.lower_expr(&args[0]);
                        let is_map = matches!(
                            self.type_map.get(&src_id),
                            Some(t) if t.is_map()
                        );
                        if is_map {
                            let fresh = self.next_id();
                            self.stmts.push(MirStmt::MapNew { dest: fresh });
                            self.exprs.insert(fresh, MirExpr::Var(fresh));
                            self.type_map
                                .insert(fresh, Type::Named("map".to_string(), vec![]));
                            // The VALUE of `dict(m)` is the new map, not
                            // py_map_update's return (which is 0 — using it as the
                            // dest made `dict(d)` an empty map).
                            let sink = self.emit_call("py_map_update", vec![fresh, src_id], Type::I64);
                            self.exprs.insert(dest, MirExpr::Var(fresh));
                            self.type_map.insert(dest, Type::Named("map".to_string(), vec![]));
                            return Some(dest);
                        } else {
                            // Dynamic/unknown argument: emit `map__copy(src)` which
                            // works on any handle (creates new map + copies entries).
                            // This fixes t231 where `dict(x)` with `x: PyDynamic`
                            // fell through to ghost `dict_1`.
                            self.emit_call_into(dest, "map__copy", vec![src_id], Type::Named("map".to_string(), vec![]));
                            return Some(dest);
                        }
                        /* Batch 661 (cleanup) reconciliation: the dynamic route
                           above is mainline 658's `map__copy` — functionally
                           equivalent to this lane's `py_dict_ctor` (both funnel
                           non-dict handles through map_resolve's loud guard =
                           TypeError parity), so ONE route is kept. The runtime
                           `py_dict_ctor` stays as unwired infrastructure. */
                    }
        None
    }
}
