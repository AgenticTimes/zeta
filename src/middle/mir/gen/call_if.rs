//! 批次 858：If 表达式发射体（原臂逐字迁入——零替换法）。

use super::fold_env_condition;
use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_if_expr(
        &mut self,
        cond: &Box<AstNode>,
        then: &Vec<AstNode>,
        else_: &Vec<AstNode>,
        dest: u32,
    ) -> u32 {
                // If expression - generate control flow with destination
                let cond_id = self.lower_expr(cond);
                let dest_id = self.next_id();

                // The destination's type must come from the BRANCHES: typing it
                // I64 unconditionally made `prefix = "sh" if c else "sz"` hold a
                // string handle in an I64 slot — `len(prefix)` then did
                // `array_len(handle)` and produced garbage, `f"{prefix}.{left}"`
                // printed `<address>.<code>`, and the whole wufu universe came out
                // as `4296191491.513180`. That is why `get_universe("wufu")`
                // returned junk codes and `_load_cache` failed for all of them.
                let ast_branch_ty: Option<Type> = {
                    let ast_ty = |n: &AstNode| -> Option<Type> {
                        match n {
                            AstNode::StringLit(_) | AstNode::FString { .. } => Some(Type::Str),
                            AstNode::FloatLit(_) => Some(Type::F64),
                            AstNode::Bool(_) => Some(Type::Bool),
                            AstNode::Lit(_) => Some(Type::I64),
                            // A comprehension that keeps its element as-is
                            // (`[d for d in days if cond]`) has a VAR tail —
                            // resolve it from the enclosing type map (batch
                            // 293: the trading-day filter typed I64, so the
                            // driver's date window matched 0 days).
                            AstNode::Var(name) => self
                                .name_to_id
                                .get(name.as_str())
                                .and_then(|i| self.type_map.get(i))
                                .cloned(),
                            _ => None,
                        }
                    };
                    let tail_of = |blk: &[AstNode]| -> Option<Type> {
                        // The ternary's arms arrive as BLOCKS (`Block { body }`),
                        // so unwrap down to the last real statement.
                        let mut tail = blk.last();
                        while let Some(AstNode::Block { body }) = tail {
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
                self.exprs.insert(dest_id, MirExpr::Var(dest_id));
                self.type_map
                    .insert(dest_id, ast_branch_ty.clone().unwrap_or(Type::I64));

                // Helper function to process block
                fn process_block(
                    mir_gen: &mut MirGen,
                    block: &[AstNode],
                    dest: u32,
                ) -> Vec<MirStmt> {
                    if block.is_empty() {
                        // Empty block - assign 0
                        let zero_id = mir_gen.next_id_with_lit(0);
                        return vec![MirStmt::Assign {
                            lhs: dest,
                            rhs: zero_id,
                        }];
                    }

                    // Save current statements
                    let saved_stmts = std::mem::take(&mut mir_gen.stmts);

                    // Generate block statements
                    for s in block {
                        mir_gen.lower_ast(s);
                    }

                    // Take generated statements
                    let mut block_stmts = std::mem::take(&mut mir_gen.stmts);

                    // Restore original statements
                    mir_gen.stmts = saved_stmts;

                    // Capture last expression value if block produces value
                    if let Some(last_stmt) = block_stmts.last() {
                        match last_stmt {
                            MirStmt::Assign { lhs, .. } => {
                                // Block ends with assignment - use that value
                                block_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *lhs,
                                });
                            }
                            MirStmt::If {
                                dest: Some(if_dest),
                                ..
                            } => {
                                // Block ends with if expression - use its destination
                                block_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *if_dest,
                                });
                            }
                            MirStmt::Return { val } => {
                                // Block ends with return - can't assign to dest
                                // (function returns, dest unused)
                            }
                            MirStmt::Call {
                                dest: call_dest, ..
                            }
                            | MirStmt::SemiringFold { result: call_dest, .. }
                            | MirStmt::DictGet {
                                dest: call_dest, ..
                            }
                            | MirStmt::MapNew { dest: call_dest }
                            | MirStmt::StructNew {
                                dest: call_dest, ..
                            } => {
                                // Block ends with function call - use its result
                                block_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *call_dest,
                                });
                            }
                            _ => {
                                // No value-producing statement - assign 0
                                let zero_id = mir_gen.next_id_with_lit(0);
                                block_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: zero_id,
                                });
                            }
                        }
                    } else {
                        // Block has AST nodes but no MIR statements were generated
                        // (e.g. bare Var/Lit/BinaryOp expressions in statement position)
                        // Lower the last AST node as an expression to capture its value
                        if let Some(last_ast) = block.last() {
                            // PY-A: unwrap `ExprStmt` — lowering the WRAPPER node
                            // is not an expression lowering at all, so the value
                            // came back untyped (i64) and a string branch of a
                            // ternary was printed as a pointer.
                            let inner = match last_ast {
                                AstNode::ExprStmt { expr } => expr.as_ref(),
                                other => other,
                            };
                            let val_id = mir_gen.lower_expr(inner);
                            block_stmts.push(MirStmt::Assign {
                                lhs: dest,
                                rhs: val_id,
                            });
                        } else {
                            // Shouldn't reach here (empty block handled above), but fallback
                            let zero_id = mir_gen.next_id_with_lit(0);
                            block_stmts.push(MirStmt::Assign {
                                lhs: dest,
                                rhs: zero_id,
                            });
                        }
                    }

                    block_stmts
                }

                // Process then and else blocks
                let then_stmts = process_block(self, then, dest_id);
                let else_stmts = process_block(self, else_, dest_id);

                // PY-A: `dest_id` was typed i64 up front, so a conditional
                // expression whose branches yield STRINGS
                // (`"big" if a > 3 else "small"`) stored a string pointer in an
                // i64 slot — `print` then showed a number instead of the text.
                // Take the type from the branch values instead (they agree in
                // every real case; disagreement keeps the i64 fallback).
                let branch_ty = |stmts: &[MirStmt]| -> Option<Type> {
                    stmts.iter().rev().find_map(|s| match s {
                        MirStmt::Assign { rhs, .. } => self.type_map.get(rhs).cloned(),
                        _ => None,
                    })
                };
                if let Some(ty) = branch_ty(&then_stmts).or_else(|| branch_ty(&else_stmts)) {
                        if !matches!(ty, Type::I64)
                        || branch_ty(&else_stmts).map_or(true, |t| matches!(t, Type::I64))
                    {
                        self.type_map.insert(dest_id, ty);
                    }
                }
                // The AST-level inference is AUTHORITATIVE and must win: the
                // statement-level pass above derives I64 for
                // `s = "sh" if c else "sz"` (both arms are plain Assigns), which
                // made `len(s)` emit `array_len` on a string handle and
                // `f"{prefix}.{code}"` print `<address>.<code>` for every code in
                // the wufu universe (measured: `4296191491.513180`).
                if let Some(ast_ty) = ast_branch_ty {
                    self.type_map.insert(dest_id, ast_ty);
                }

                // Create If statement with destination
                self.stmts.push(MirStmt::If {
                    cond: cond_id,
                    then: then_stmts,
                    else_: else_stmts,
                    dest: Some(dest_id),
                });

                return dest_id;
            
        dest
    }

    /// `if cond { … } else { … }` 语句／表达式位（批 864 自 gen.rs 原臂逐字迁入；
    /// 含 fold_env_condition 编译期环境开关、表达式 if 的值槽捕获与分支型回填）。
    pub(super) fn lower_if_stmt(&mut self, cond: &AstNode, then: &[AstNode], else_: &[AstNode]) {
            // PY-A: compile-time env switch (see fold_env_condition).
            if let Some(taken) = fold_env_condition(cond) {
                let branch: &[AstNode] = if taken { then } else { else_ };
                for s in branch {
                    self.lower_expr(s);
                }
                return;
            }
            let cond_id = self.lower_expr(cond);

            // Check if this is expression if (branches produce values) or statement if
            // Simple heuristic: if any branch contains return, treat as statement
            let mut is_statement_if = false;
            let mut then_has_return = false;
            let mut else_has_return = false;

            // Scan branches for returns/breaks/continues (statement-only)
            for s in then.iter() {
                if let AstNode::Return(_) | AstNode::Break(_) | AstNode::Continue(_) = s {
                    then_has_return = true;
                    is_statement_if = true;
                    break;
                }
            }
            for s in else_.iter() {
                if let AstNode::Return(_) | AstNode::Break(_) | AstNode::Continue(_) = s {
                    else_has_return = true;
                    is_statement_if = true;
                    break;
                }
            }

            let dest_id = if is_statement_if {
                // Statement if: no destination needed
                None
            } else {
                // Expression if: create destination
                let id = self.next_id();

                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
                Some(id)
            };

            // Generate then block in isolated context
            // Generate then block inline (no isolated context)
            let mut then_stmts = vec![];
            if !then.is_empty() {
                // Save current statements
                let saved_stmts = std::mem::take(&mut self.stmts);

                // Generate block statements directly in current context
                for s in then {
                    self.lower_ast(s);
                }

                // Take the generated statements
                then_stmts = std::mem::take(&mut self.stmts);

                // Restore main statements
                self.stmts = saved_stmts;

                // For expression if, capture the last value
                if let Some(dest) = dest_id
                    && !then_has_return
                {
                    if let Some(last_stmt) = then_stmts.last() {
                        match last_stmt {
                            MirStmt::Assign { lhs, .. } => {
                                // Add assignment to dest
                                then_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *lhs,
                                });
                            }
                            MirStmt::If {
                                dest: Some(if_dest),
                                ..
                            } => {
                                // Block ends with if expression - use its destination
                                then_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *if_dest,
                                });
                            }
                            MirStmt::Call {
                                dest: call_dest, ..
                            }
                            | MirStmt::SemiringFold { result: call_dest, .. }
                            | MirStmt::DictGet {
                                dest: call_dest, ..
                            }
                            | MirStmt::MapNew { dest: call_dest }
                            | MirStmt::StructNew {
                                dest: call_dest, ..
                            } => {
                                // Block ends with function call - use its result
                                then_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *call_dest,
                                });
                            }
                            _ => {
                                // No value-producing statement found
                                let zero_id = self.next_id_with_lit(0);
                                then_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: zero_id,
                                });
                            }
                        }
                    } else if let Some(last_ast) = then.last() {
                        // then_stmts is empty but then block has AST nodes
                        // Lower the last AST as an expression for its value
                        let val_id = self.lower_expr(last_ast);
                        then_stmts.push(MirStmt::Assign {
                            lhs: dest,
                            rhs: val_id,
                        });
                    }
                }
            }

            // Generate else block inline
            let mut else_stmts = vec![];
            if !else_.is_empty() {
                // Save current statements
                let saved_stmts = std::mem::take(&mut self.stmts);

                // Generate block statements directly in current context
                for s in else_ {
                    self.lower_ast(s);
                }

                // Take the generated statements
                else_stmts = std::mem::take(&mut self.stmts);

                // Restore main statements
                self.stmts = saved_stmts;

                // For expression if, capture the last value
                if let Some(dest) = dest_id
                    && !else_has_return
                {
                    if let Some(last_stmt) = else_stmts.last() {
                        match last_stmt {
                            MirStmt::Assign { lhs, .. } => {
                                // Add assignment to dest
                                else_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *lhs,
                                });
                            }
                            MirStmt::If {
                                dest: Some(if_dest),
                                ..
                            } => {
                                // Block ends with if expression - use its destination
                                else_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *if_dest,
                                });
                            }
                            MirStmt::Call {
                                dest: call_dest, ..
                            }
                            | MirStmt::SemiringFold { result: call_dest, .. }
                            | MirStmt::DictGet {
                                dest: call_dest, ..
                            }
                            | MirStmt::MapNew { dest: call_dest }
                            | MirStmt::StructNew {
                                dest: call_dest, ..
                            } => {
                                // Block ends with function call - use its result
                                else_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *call_dest,
                                });
                            }
                            _ => {
                                // No value-producing statement found
                                let zero_id = self.next_id_with_lit(0);
                                else_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: zero_id,
                                });
                            }
                        }
                    } else if let Some(last_ast) = else_.last() {
                        // else_stmts is empty but else block has AST nodes
                        // Lower the last AST as an expression for its value
                        let val_id = self.lower_expr(last_ast);
                        else_stmts.push(MirStmt::Assign {
                            lhs: dest,
                            rhs: val_id,
                        });
                    }
                }
            }

            // Expressions are already in self.exprs (generated inline)
            // No need to merge or update next_id

            // An expression-if's dest starts I64; a str-valued branch then
            // leaks a pointer-typed slot (batch 293: `[d for d in days if
            // cond]` desugars to `if cond { d } else { -1 }`, the dest
            // stayed I64, so `trading_days_filtered` was DynamicArray(I64)
            // and the driver's date filter pointer-compared to 0 days).
            // Trust the branch that carries the comprehension element; the
            // -1 skip sentinel needs no type.
            if let Some(dest) = dest_id {
                let branch_val = |stmts: &Vec<MirStmt>| -> Option<u32> {
                    stmts.last().and_then(|s| match s {
                        MirStmt::Assign { lhs, rhs } if *lhs == dest => Some(*rhs),
                        _ => None,
                    })
                };
                let refined = branch_val(&then_stmts)
                    .or_else(|| branch_val(&else_stmts))
                    .and_then(|v| self.type_map.get(&v).cloned());
                if let Some(t) = refined {
                    self.type_map.insert(dest, t);
                }
            }

            self.stmts.push(MirStmt::If {
                cond: cond_id,
                then: then_stmts,
                else_: else_stmts,
                dest: dest_id,
            });
    }
}