//! 批次 875：小语句臂清扫（ExprStmt／ConstDef／IfLet／Await——869 零适配法）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_exprstmt(&mut self, expr: &AstNode) {
            if let AstNode::Return(inner) = expr {
                let val = self.lower_expr(inner);
                self.stmts.push(MirStmt::Return { val });
            } else {
                let expr_id = self.lower_expr(expr);
                // For expression statements that are values (not just side effects),
                // we need to capture the value. Create a temporary assignment.
                // This will be optimized away if not needed.
                let temp_id = self.next_id();
                self.stmts.push(MirStmt::Assign {
                    lhs: temp_id,
                    rhs: expr_id,
                });
                // Store the temp ID for implicit return to find
                self.exprs.insert(temp_id, MirExpr::Var(temp_id));
                // The temp must CARRY the expression's type. Typing it I64
                // unconditionally erased F64 from every expression-statement
                // arm of a ternary: `return p*(1+self.s) if c else ...`
                // (`CostModel.fill_price`) then inferred an i64 signature, so
                // the early `return price` compiled to `fptosi double->i64`
                // and the caller's bitcast produced 2.5e-323 — i.e. the whole
                // local backtest had `total = 0` and `final_value -> 0`.
                let temp_ty = self
                    .type_map
                    .get(&expr_id)
                    .cloned()
                    .unwrap_or(Type::slot_fallback());
                self.type_map.insert(temp_id, temp_ty);
            }
    }

    pub(super) fn lower_constdef_stmt(&mut self, name: &String, value: &AstNode) {
            // Register local constant values so Var(name) references can resolve them
            // CTFE should have already evaluated `value` to a Lit/Bool by this point
            match value {
                AstNode::Lit(n) => {
                    self.global_consts.insert(
                        name.clone(),
                        crate::middle::ctfe::value::ConstValue::Int(*n),
                    );
                }
                AstNode::Bool(b) => {
                    self.global_consts.insert(
                        name.clone(),
                        crate::middle::ctfe::value::ConstValue::Bool(*b),
                    );
                }
                AstNode::StringLit(s) => {
                    self.global_consts.insert(
                        name.clone(),
                        crate::middle::ctfe::value::ConstValue::String(s.clone()),
                    );
                }
                _ => {
                    // Try CTFE evaluation at MIR gen time
                    if let Ok(val) = crate::middle::ctfe::eval_const_expr(value) {
                        self.global_consts.insert(name.clone(), val);
                    }
                }
            }
    }

    pub(super) fn lower_iflet_stmt(
        &mut self,
        pattern: &AstNode,
        expr: &AstNode,
        then: &[AstNode],
        else_: &[AstNode],
    ) {
            // Desugar: if let <pat> = <expr> { then } [else { else_ }]
            match pattern {
                AstNode::Var(name) => {
                    // Simple binding — always matches.
                    let expr_id = self.lower_expr(expr);
                    let lhs_id = self.next_id();
                    self.stmts.push(MirStmt::Assign {
                        lhs: lhs_id,
                        rhs: expr_id,
                    });
                    self.name_to_id.insert(name.clone(), lhs_id);
                    self.exprs.insert(lhs_id, MirExpr::Var(lhs_id));
                    if let Some(ty) = self.type_map.get(&expr_id) {
                        self.type_map.insert(lhs_id, ty.clone());
                    } else {
                        self.type_map.insert(lhs_id, Type::I64);
                    }
                    for stmt in then {
                        self.lower_ast(stmt);
                    }
                }
                AstNode::Ignore => {
                    // Wildcard — always matches, discard value.
                    self.lower_expr(expr);
                    for stmt in then {
                        self.lower_ast(stmt);
                    }
                }
                _ => {
                    // Complex pattern: lower expr (side effects), always run then.
                    self.lower_expr(expr);
                    for stmt in then {
                        self.lower_ast(stmt);
                    }
                }
            }
    }

    pub(super) fn lower_await_stmt(&mut self, body: &AstNode) {
            // Await statement: evaluate the inner expression and poll.
            let fut_id = self.lower_expr(body);
            let pr_id = self.next_id();
            let zero_id = self.next_id_with_lit(0);
            let stored_fut = self.next_id();
            self.exprs.insert(stored_fut, MirExpr::Var(stored_fut));
            self.type_map.insert(stored_fut, Type::I64);
            self.stmts.push(MirStmt::Assign {
                lhs: stored_fut,
                rhs: fut_id,
            });
            self.exprs.insert(pr_id, MirExpr::Var(pr_id));
            self.type_map.insert(pr_id, Type::I64);

            let mut body_stmts = vec![];
            body_stmts.push(MirStmt::Call {
                func: "future_poll".to_string(),
                args: vec![stored_fut],
                dest: pr_id,
                type_args: vec![],
            });
            let mut then_stmts = vec![];
            then_stmts.push(MirStmt::Break);
            let cond_id = self.next_id();
            self.exprs.insert(
                cond_id,
                MirExpr::BinaryOp {
                    op: "!=".to_string(),
                    left: pr_id,
                    right: zero_id,
                },
            );
            self.type_map.insert(cond_id, Type::Bool);
            body_stmts.push(MirStmt::If {
                cond: cond_id,
                then: then_stmts,
                else_: vec![],
                dest: None,
            });
            let true_id = self.next_id_with_lit(1);
            self.stmts.push(MirStmt::While {
                cond: true_id,
                pre_cond: vec![],
                body: body_stmts,
                else_body: vec![],
            });
    }

    /// await 表达式位（批 886 原臂逐字迁入；结果写 id 槽）。
    pub(super) fn lower_await_expr(&mut self, body: &AstNode, id: u32) -> u32 {
            // Await expression: poll the sub-future until ready, then extract value.
            // Generates:
            //   let __fut = <body>;           // create sub-future
            //   while true {
            //       let __pr = future_poll(__fut);
            //       if __pr != 0 {               // Ready
            //           result = future_result(__fut);
            //           break;
            //       }
            //   }
            //   return result;
            let fut_id = self.lower_expr(body);
            // Create IDs
            let pr_id = self.next_id();
            let result_id = self.next_id();
            let zero_id = self.next_id_with_lit(0);

            // Store the sub-future pointer
            let stored_fut = self.next_id();
            self.exprs.insert(stored_fut, MirExpr::Var(stored_fut));
            self.type_map.insert(stored_fut, Type::I64);
            self.stmts.push(MirStmt::Assign {
                lhs: stored_fut,
                rhs: fut_id,
            });

            // poll result
            self.exprs.insert(pr_id, MirExpr::Var(pr_id));
            self.type_map.insert(pr_id, Type::I64);

            // result
            self.exprs.insert(result_id, MirExpr::Var(result_id));
            self.type_map.insert(result_id, Type::I64);

            // While loop body
            let mut body_stmts = vec![];

            // pr = future_poll(stored_fut)
            body_stmts.push(MirStmt::Call {
                func: "future_poll".to_string(),
                args: vec![stored_fut],
                dest: pr_id,
                type_args: vec![],
            });

            // if pr != 0 { result = future_result(stored_fut); break; }
            let mut then_stmts = vec![];
            then_stmts.push(MirStmt::Call {
                func: "future_result".to_string(),
                args: vec![stored_fut],
                dest: result_id,
                type_args: vec![],
            });
            then_stmts.push(MirStmt::Break);

            let cond_id = self.next_id();
            self.exprs.insert(
                cond_id,
                MirExpr::BinaryOp {
                    op: "!=".to_string(),
                    left: pr_id,
                    right: zero_id,
                },
            );
            self.type_map.insert(cond_id, Type::Bool);

            body_stmts.push(MirStmt::If {
                cond: cond_id,
                then: then_stmts,
                else_: vec![],
                dest: None,
            });

            // While(true) loop
            let true_id = self.next_id_with_lit(1);
            self.stmts.push(MirStmt::While {
                cond: true_id,
                pre_cond: vec![],
                body: body_stmts,
                else_body: vec![],
            });

            // Store result back to the expression ID so it's accessible
            // via load_local(id) later (e.g., in a return statement).
            self.stmts.push(MirStmt::Assign {
                lhs: id,
                rhs: result_id,
            });

            // Register result as the expression value
            self.exprs.insert(id, MirExpr::Var(result_id));
            if let Some(ty) = self.type_map.get(&fut_id) {
                self.type_map.insert(id, ty.clone());
            } else {
                self.type_map.insert(id, Type::I64);
            }
            return id;
    }


    /// 块表达式位（批 886 原臂逐字迁入；结果写 id 槽）。
    pub(super) fn lower_block_expr(&mut self, body: &Vec<AstNode>, id: u32) -> u32 {
            // Block expression: lower body statements, capture last value.
            if body.is_empty() {
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
                return id;
            }
            // Save and isolate block stmts
            let saved_stmts = std::mem::take(&mut self.stmts);
            // Lower all but the last as statements
            for stmt in &body[..body.len() - 1] {
                self.lower_ast(stmt);
            }
            // Lower the last as an expression (block value)
            let last = &body[body.len() - 1];
            // If last is an ExprStmt, unwrap it
            let val_id = match last {
                AstNode::ExprStmt { expr } => self.lower_expr(expr),
                // PY-A: statement-form last (Assign/Let/Return…) — lower
                // as a statement; the block value falls back to the last
                // produced value or 0.
                AstNode::Return(_) => self.i64_zero_id(), // Return handled by closure fn tail
                AstNode::Assign(_, _) | AstNode::Let { .. } => {
                    self.lower_ast(last);
                    self.i64_zero_id()
                }
                other => self.lower_expr(other),
            };
            let block_stmts = std::mem::take(&mut self.stmts);
            self.stmts = saved_stmts;
            // Forward all block stmts
            for s in block_stmts {
                self.stmts.push(s);
            }
            // Assign the block result to the block's local slot
            self.stmts.push(MirStmt::Assign {
                lhs: id,
                rhs: val_id,
            });
            // Store the block result (reference own alloca so gen_expr_safe loads from it)
            self.exprs.insert(id, MirExpr::Var(id));
            if let Some(ty) = self.type_map.get(&val_id) {
                self.type_map.insert(id, ty.clone());
            } else {
                self.type_map.insert(id, Type::I64);
            }
            return id;
    }

}