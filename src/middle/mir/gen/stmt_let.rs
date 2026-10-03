//! 批次 867：Let 语句臂发射体（自 gen.rs 原臂逐字迁入）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// `let pattern = expr`：nonlocal 名经 env 读写、普通名取 RHS 型、
    /// 带注解名做元素型/类名回填、元组模式逐位解构（原臂逐字）。
    pub(super) fn lower_let_stmt(&mut self, pattern: &AstNode, expr: &AstNode) {
            // Handle different pattern types
            match pattern {
                AstNode::Var(name) if self.nonlocal_names.contains(name) => {
                    // PY-A V3: nonlocal name — defining assignment stores
                    // through env; bind local slot to an env load.
                    let rhs_id = self.lower_expr(expr);
                    let key_id = self.env_store(name, rhs_id);
                    let slot_id = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_env_get".to_string(),
                        args: vec![key_id],
                        dest: slot_id,
                        type_args: vec![],
                    });
                    self.exprs.insert(slot_id, MirExpr::Var(slot_id));
                    let ty = self.env_slot_ty(name);
                    self.type_map.insert(slot_id, ty);
                    self.name_to_id.insert(name.clone(), slot_id);
                }
                AstNode::Var(name) => {
                    self.pending_closure_binding = Some(name.clone());
                    let rhs_id = self.lower_expr(expr);
                    self.pending_closure_binding = None;
                    let lhs_id = self.next_id();
                    self.stmts.push(MirStmt::Assign {
                        lhs: lhs_id,
                        rhs: rhs_id,
                    });
                    self.name_to_id.insert(name.clone(), lhs_id);
                    self.exprs.insert(lhs_id, MirExpr::Var(lhs_id));
                    // Copy type from RHS to LHS
                    if let Some(rhs_type) = self.type_map.get(&rhs_id) {
                        self.type_map.insert(lhs_id, rhs_type.clone());
                    } else {
                        self.type_map.insert(lhs_id, Type::I64);
                    }
                }
                AstNode::TypeAnnotatedPattern {
                    pattern: inner_pattern,
                    ty,
                } => {
                    // For type-annotated patterns, extract the inner pattern
                    // The type checking should have been done by the type checker
                    if let AstNode::Var(name) = &**inner_pattern {
                        let rhs_id = self.lower_expr(expr);
                        let lhs_id = self.next_id();
                        self.stmts.push(MirStmt::Assign {
                            lhs: lhs_id,
                            rhs: rhs_id,
                        });
                        self.name_to_id.insert(name.clone(), lhs_id);
                        self.exprs.insert(lhs_id, MirExpr::Var(lhs_id));
                        // Copy type from RHS to LHS
                        let rhs_ty = self.type_map.get(&rhs_id).cloned();
                        // `x: set[str] = set()` — the ANNOTATION is the only
                        // place the element type exists: `set()` lowers to an
                        // empty DynamicArray, so dropping the annotation left
                        // the element I64 ("unknown") and `"q" in c` compared
                        // HANDLES — every string counted as absent (measured:
                        // `fetched_codes: set[str] = set()` in fetch_stocks).
                        let refined = rhs_ty.clone().map(|t| {
                            match (&t, Self::annotation_elem_ty(ty)) {
                                (Type::DynamicArray(e), Some(el))
                                    if matches!(**e, Type::I64) =>
                                {
                                    Type::DynamicArray(Box::new(el))
                                }
                                // `set()` may come back wholly untyped
                                // (I64 / PyDynamic) — the annotation still
                                // says what the container holds.
                                (Type::I64, Some(el)) | (Type::PyDynamic, Some(el)) => {
                                    Type::DynamicArray(Box::new(el))
                                }
                                _ => t,
                            }
                        });
                        // `partial: pd.DataFrame | None = None` — the None
                        // initializer types the slot I64, and later rebinds
                        // do not refresh types, so `len(partial)` kept
                        // dispatching `array_len` on the DataFrame object
                        // (header read = 0 rows) and every fetch_stocks
                        // cache gate failed. The annotation names the real
                        // class; adopt it when the initializer is untyped.
                        let untyped_init = matches!(
                            refined,
                            None | Some(Type::I64) | Some(Type::PyDynamic)
                        );
                        let refined = if untyped_init {
                            self.annotation_named_ty(ty).or(refined)
                        } else {
                            refined
                        };
                        let refined = refined.unwrap_or(Type::I64);
                        self.type_map.insert(lhs_id, refined.clone());
                        if matches!(refined, Type::Named(_, _)) {
                            self.apply_dict_annotation(lhs_id, ty);
                        }
                    }
                    // Note: We could add runtime identity checking here if needed,
                    // but the type checker should have already validated the type.
                }
                _ => {
                    // For other pattern types, generate a simple assignment
                    // This is a simplification - in a full implementation,
                    // we would need to handle destructuring patterns
                    if let AstNode::Tuple(elements) = &pattern {
                        // Tuple destructuring: let (a, b) = expr;
                        // Lower as multiple field accesses.
                        let rhs_id = self.lower_expr(expr);
                        for (i, elem) in elements.iter().enumerate() {
                            if let AstNode::Var(name) = elem {
                                let elem_id = self.next_id();
                                // Access field i of the tuple
                                let field_id = self.next_id();
                                self.exprs.insert(field_id, MirExpr::IntLit(i as i64));
                                self.type_map.insert(field_id, Type::I64);
                                self.stmts.push(MirStmt::Call {
                                    func: "stack_array_get".to_string(),
                                    args: vec![rhs_id, field_id],
                                    dest: elem_id,
                                    type_args: vec![],
                                });
                                self.name_to_id.insert(name.clone(), elem_id);
                                self.exprs.insert(elem_id, MirExpr::Var(elem_id));
                                self.type_map.insert(elem_id, Type::I64);
                            }
                        }
                    } else {
                        let rhs_id = self.lower_expr(expr);
                        let lhs_id = self.next_id();
                        self.stmts.push(MirStmt::Assign {
                            lhs: lhs_id,
                            rhs: rhs_id,
                        });
                        self.exprs.insert(lhs_id, MirExpr::Var(lhs_id));
                        if let Some(rhs_type) = self.type_map.get(&rhs_id) {
                            self.type_map.insert(lhs_id, rhs_type.clone());
                        } else {
                            self.type_map.insert(lhs_id, Type::I64);
                        }
                    }
                }
            }
    }
}
