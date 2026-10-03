//! 批次 852：UnaryOp 发射体（负号/位非/not——原臂逐字）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_unary_op(
        &mut self,
        op: &String,
        expr: &Box<AstNode>,
        id: u32,
    ) -> u32 {
            // Handle unary operators like ! (not)
            // Python `not` is LOGICAL negation — it must NOT become `!`,
            // because the `!` branch treats an ARRAY operand as `~mask`
            // (element-wise NOT). `if not parts:` on a 1-element list became
            // `py_vec_not(parts)` → a list, which is truthy, so the code took
            // the wrong branch and the whole cleaning result came out empty.
            // Lower it to the runtime helper instead (falsy = 0 / empty array).
            if op == "not" {
                let expr_id = self.lower_expr(expr);
                // BATCH-297: a parsed-JSON value is a TAGGED cell, so the
                // geometry reader in `py_not` cannot tell `[]` from `[1]` —
                // ask the JSON accessor first (it answers 0/1, which the
                // regular `py_not` negates unchanged).
                let subject =
                    if matches!(self.type_map.get(&expr_id), Some(Type::Named(n, _)) if n == "PyJson")
                    {
                        let tj = self.emit_call("py_json_truth", vec![expr_id], Type::I64);
                        tj
                    } else {
                        expr_id
                    };
                self.emit_call_into(id, "py_not", vec![subject], Type::Bool);
                return id;
            }
            let op: &str = op;
            let expr_id = self.lower_expr(expr);
            let dest = self.next_id();

            // PY-A fix: negative float literals fold at MIR — the
            // i64 unary_minus path corrupts their bit pattern (→ NaN).
            if op == "-" {
                if let AstNode::FloatLit(v) = &**expr {
                    self.exprs.insert(dest, MirExpr::FloatLit(-v.parse::<f64>().unwrap_or(0.0)));
                    self.type_map.insert(dest, Type::F64);
                    return dest;
                }
            }
            if op == "&mut" || op == "&" {
                // Address-of: pass the variable's location, not its value.
                // For a simple variable, use its alloca address (ptrtoint in codegen).
                if let AstNode::Var(name) = &**expr {
                    if let Some(&addr_alloca) = self.name_to_id.get(name.as_str()) {
                        self.exprs.insert(dest, MirExpr::AddrOf { alloca_id: addr_alloca });
                        self.type_map.insert(dest, Type::I64);
                        return dest;
                    }
                }
                // Non-variable operand: fall through to passing its value
                // (rvalues have no address; the callee mutation is discarded).
                self.exprs.insert(dest, MirExpr::Var(expr_id));
                self.type_map.insert(dest, Type::I64);
                return dest;
            }
            if op == "!" {
                // A VECTOR operand is Python's `~mask` (element-wise NOT): the
                // parser/lowering maps `~` onto `!`, so `df.loc[~invalid]` used
                // to call the INTEGER `!` on the vector handle — `loc` then got
                // mask == 0 (measured: "DataFrame.loc: mask is missing").
                if matches!(
                    self.type_map.get(&expr_id),
                    Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                ) {
                    self.stmts.push(MirStmt::Call {
                        func: "py_vec_not".to_string(),
                        args: vec![expr_id],
                        dest,
                        type_args: vec![],
                    });
                    // Batch 398: `~` keeps the operand's element — a Bool mask
                    // stays a Bool mask, an index list stays an index list.
                    let elem = match self.type_map.get(&expr_id) {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => (**e).clone(),
                        _ => Type::I64,
                    };
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(elem)));
                } else {
                    // Logical NOT operator
                    let stmt = MirStmt::Call {
                        func: "!".to_string(),
                        args: vec![expr_id],
                        dest,
                        type_args: vec![],
                    };
                    self.stmts.push(stmt);
                }
            } else if op == "*" {
                // Pointer dereference - use Deref MirExpr with pointee width
                let pointee_width = self.pointee_widths.get(&expr_id).copied().unwrap_or(8);
                self.exprs.insert(
                    dest,
                    MirExpr::Deref {
                        addr_id: expr_id,
                        pointee_width,
                    },
                );
                self.type_map.insert(dest, Type::I64);
                // SKIP the common Var(dest) insert below — the Deref expr is used inline
                // by gen_expr_safe, so we don't need an alloca to load from.
                return dest;
            } else if op == "~"
                && matches!(
                    self.type_map.get(&expr_id),
                    Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                )
            {
                // `~mask` on a mask VECTOR (Python's `df.loc[~invalid]`):
                // element-wise NOT. The integer bitwise-NOT of the handle made
                // the mask garbage (measured: `loc` received mask == 0).
                self.stmts.push(MirStmt::Call {
                    func: "py_vec_not".to_string(),
                    args: vec![expr_id],
                    dest,
                    type_args: vec![],
                });
                let elem = match self.type_map.get(&expr_id) {
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => (**e).clone(),
                    _ => Type::I64,
                };
                self.type_map
                    .insert(dest, Type::DynamicArray(Box::new(elem)));
            } else if op == "-" {
                // PY-A fix: floating-point operands must NOT go through
                // the i64 unary_minus runtime (bit-pattern negation →
                // NaN for negatives like -2.5). 0 - x via BinaryOp keeps
                // the float type; ints keep the dedicated symbol.
                let is_float = matches!(
                    self.type_map.get(&expr_id),
                    Some(Type::F64) | Some(Type::F32)
                );
                if is_float {
                    let zero_id = self.next_id();
                    self.exprs.insert(zero_id, MirExpr::FloatLit(0.0));
                    self.type_map.insert(zero_id, Type::F64);
                    self.exprs.insert(
                        dest,
                        MirExpr::BinaryOp {
                            op: "-".to_string(),
                            left: zero_id,
                            right: expr_id,
                        },
                    );
                } else if matches!(
                    self.type_map.get(&expr_id),
                    Some(Type::Named(n, _)) if n == "BigInt"
                ) {
                    /* Batch 658: a BigInt handle negates through the
                       big family — the i64 unary_minus bit-flips the
                       POINTER, and the print face then derefs garbage
                       (measured rc=139 on a loop-assigned big; the
                       static shape only survived via the ctfe fold). */
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_big_neg".to_string(),
                        args: vec![expr_id],
                        dest,
                        type_args: vec![],
                    });
                } else {
                    // Unary minus - use special function name to avoid conflict with binary minus
                    self.stmts.push(MirStmt::Call {
                        func: "unary_minus".to_string(),
                        args: vec![expr_id],
                        dest,
                        type_args: vec![],
                    });
                }
            } else {
                // Other unary operators (unary plus?, etc.)
                let stmt = MirStmt::Call {
                    func: op.to_string(),
                    args: vec![expr_id],
                    dest,
                    type_args: vec![],
                };
                self.stmts.push(stmt);
            }

            self.exprs.insert(dest, MirExpr::Var(dest));
            // Preserve the operand's type — unary_minus on f64 should still be f64.
            // The old code hardcoded Type::I64, which broke float casts like `b as i64`.
            let op_ty = self.type_map.get(&expr_id).cloned().unwrap_or_else(Type::slot_fallback);
            self.type_map.insert(dest, op_ty);
            return dest;
    }
}
