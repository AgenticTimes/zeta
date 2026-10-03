//! 批次 870：Match 表达式臂发射体（857 稳定化时内联回 gen.rs 的 600 行
//! 臂，本批按 869 的零适配法迁出）。

use super::MirGen;
use crate::frontend::ast::{AstNode, MatchArm};
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_match_expr(
        &mut self,
        scrutinee: &Box<AstNode>,
        arms: &Vec<MatchArm>,
        id: u32,
    ) -> u32 {
            // Lower the scrutinee expression
            let scrutinee_id = self.lower_expr(scrutinee);

            // Generate if-else chain for match arms
            let result_id = id;
            // PY-A (#38 ①): the result slot used to be typed `I64` no matter
            // what the arms returned, so `let s = match 1 { 1 => "hello", _ =>
            // "world" }; print(s)` printed the string handle as a number
            // (measured: `4370672064`). Collect each arm's type instead; see
            // the unification below the chain.
            let mut arm_value_tys: Vec<Type> = Vec::new();

            // We'll build the match as a series of if-else statements
            // Start from the last arm and work backwards
            let mut else_branch = Vec::new();

            for arm in arms.iter().rev() {
                // Generate condition based on pattern
                let cond_id = self.next_id();

                match &*arm.pattern {
                    AstNode::Lit(pattern_value) => {
                        // For literal patterns, generate equality check
                        let pattern_id = self.next_id();
                        self.exprs.insert(pattern_id, MirExpr::IntLit(*pattern_value));
                        self.type_map.insert(pattern_id, Type::I64);

                        // Create equality comparison: scrutinee == pattern
                        // This creates a call to the "==" operator
                        self.emit_call_into(cond_id, "==", vec![scrutinee_id, pattern_id], Type::Bool);
                    }
                    AstNode::StringLit(pattern_value) => {
                        // String-literal pattern. Deliberately **not** the
                        // `MirStmt::Call{func:"=="}` shape the integer arm
                        // above uses: the `==` operator is declared
                        // External as `i64(i64,i64)`, so calling it on two
                        // `Str` operands compares their *addresses* — always
                        // false, and every arm silently fell through to `_`.
                        // `BinaryOp` is what `op == "+"` lowers to, and the
                        // backend routes it to the str-compare branch.
                        let pattern_id = self.next_id();
                        self.exprs
                            .insert(pattern_id, MirExpr::StringLit(pattern_value.clone()));
                        self.type_map.insert(pattern_id, Type::Str);

                        self.exprs.insert(
                            cond_id,
                            MirExpr::BinaryOp {
                                op: "==".to_string(),
                                left: scrutinee_id,
                                right: pattern_id,
                            },
                        );
                        self.type_map.insert(cond_id, Type::Bool);
                    }
                    AstNode::Var(var_name) if var_name == "_" => {
                        // Wildcard pattern - always true
                        self.exprs.insert(cond_id, MirExpr::IntLit(1));
                        self.type_map.insert(cond_id, Type::Bool);
                    }
                    AstNode::Ignore => {
                        // `_` wildcard pattern — always true
                        self.exprs.insert(cond_id, MirExpr::IntLit(1));
                        self.type_map.insert(cond_id, Type::Bool);
                    }
                    AstNode::Var(var_name) => {
                        // `None` / `Err` written bare are the same arm as
                        // `Option::None` / `Result::Err`; the checks below
                        // key on the qualified spelling. Left bare, a `None`
                        // arm fell through to "capture variable, always
                        // matches" and swallowed every value.
                        let var_name: &str = match var_name.as_str() {
                            "None" => "Option::None",
                            "Err" => "Result::Err",
                            other => other,
                        };
                        // Check if this is an enum variant name like Option::None
                        if var_name == "Option::None" || var_name == "Result::Err" {
                            // For enum variant without data, check if it matches
                            let check_func = if var_name == "Option::None" {
                                "option_is_some" // We'll invert this
                            } else if var_name == "Result::Err" {
                                "host_result_is_ok" // We'll invert this
                            } else {
                                // Should not happen
                                self.exprs.insert(cond_id, MirExpr::IntLit(0));
                                self.type_map.insert(cond_id, Type::Bool);
                                continue;
                            };

                            // Call the runtime function to check the variant
                            let check_result_id = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: check_func.to_string(),
                                args: vec![scrutinee_id],
                                dest: check_result_id,
                                type_args: vec![],
                            });
                            self.exprs
                                .insert(check_result_id, MirExpr::Var(check_result_id));
                            self.type_map.insert(check_result_id, Type::Bool);

                            // Invert the check (None is not Some, Err is not Ok)
                            let inverted_id = self.emit_call("!", vec![check_result_id], Type::Bool);

                            self.exprs.insert(cond_id, MirExpr::Var(inverted_id));
                            self.type_map.insert(cond_id, Type::Bool);
                        } else if let Some(tag) = self.enum_unit_variant_index(var_name) {
                            // Unit-variant pattern of a registered enum
                            // (e.g. `Color::Red`).
                            let boxed = self
                                .enum_variant_of(var_name)
                                .map(|(enum_name, _, _)| self.enum_is_boxed(&enum_name))
                                .unwrap_or(false);
                            let cond = if boxed {
                                // The value is a `[tag, …]` block: read the tag.
                                self.boxed_tag_guard(scrutinee_id, tag)
                            } else {
                                // All-unit enum: the value IS the discriminant,
                                // so a bare `scrutinee == tag` stays correct.
                                let cond = self.next_id();
                                let pattern_id = self.next_id();
                                self.exprs.insert(pattern_id, MirExpr::IntLit(tag));
                                self.type_map.insert(pattern_id, Type::I64);
                                self.emit_call_into(cond, "==", vec![scrutinee_id, pattern_id], Type::Bool);
                                cond
                            };
                            self.exprs.insert(cond_id, MirExpr::Var(cond));
                            self.type_map.insert(cond_id, Type::Bool);
                        } else if let Some((_, tag, _)) = self.enum_variant_of(var_name) {
                            // Same spelling without a payload list
                            // (`Token::Eof | Token::BraceClose` as a pattern,
                            // or a unit arm of an enum the parser wrote as a
                            // bare path) for a *boxed* enum whose value block
                            // carries the tag in slot 0.
                            let cond = self.boxed_tag_guard(scrutinee_id, tag);
                            self.exprs.insert(cond_id, MirExpr::Var(cond));
                            self.type_map.insert(cond_id, Type::Bool);
                        } else if var_name == "_" {
                            // Wildcard pattern - always true
                            self.exprs.insert(cond_id, MirExpr::IntLit(1));
                            self.type_map.insert(cond_id, Type::Bool);
                        } else {
                            // Regular variable binding pattern - always matches
                            // Add binding to name_to_id so the arm body can reference it
                            self.name_to_id.insert(var_name.to_string(), scrutinee_id);
                            self.exprs.insert(cond_id, MirExpr::IntLit(1));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                    }
                    AstNode::StructPattern {
                        variant,
                        fields,
                        rest: _,
                    } => {
                        // The bare Python-style spelling of a builtin
                        // constructor means the same arm (`Some(n)` ==
                        // `Option::Some(n)`), and the tests below key on the
                        // qualified name.
                        let variant: &str = match variant.as_str() {
                            "Some" => "Option::Some",
                            "None" => "Option::None",
                            "Ok" => "Result::Ok",
                            "Err" => "Result::Err",
                            other => other,
                        };
                        // Handle enum variant patterns like Option::Some(x) or Result::Ok(val)
                        // Check if this is an enum variant pattern
                        if variant.starts_with("Option::") || variant.starts_with("Result::") {
                            // Generate condition to check the variant
                            let check_func = if variant == "Option::Some" {
                                "option_is_some"
                            } else if variant == "Option::None" {
                                // For None, we check if it's not Some
                                "option_is_some" // We'll invert this below
                            } else if variant == "Result::Ok" {
                                "host_result_is_ok"
                            } else if variant == "Result::Err" {
                                // For Err, we check if it's not Ok
                                "host_result_is_ok" // We'll invert this below
                            } else {
                                // Unknown variant, treat as false
                                self.exprs.insert(cond_id, MirExpr::IntLit(0));
                                self.type_map.insert(cond_id, Type::Bool);
                                continue;
                            };

                            // Call the runtime function to check the variant
                            let check_result_id = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: check_func.to_string(),
                                args: vec![scrutinee_id],
                                dest: check_result_id,
                                type_args: vec![],
                            });
                            self.exprs
                                .insert(check_result_id, MirExpr::Var(check_result_id));
                            self.type_map.insert(check_result_id, Type::Bool);

                            // For None and Err, we need to invert the check
                            let final_check_id =
                                if variant == "Option::None" || variant == "Result::Err" {
                                    let inverted_id = self.emit_call("!", vec![check_result_id], Type::Bool);
                                    inverted_id
                                } else {
                                    check_result_id
                                };

                            // Set up bindings for field patterns
                            for (_field_name, field_pattern) in fields {
                                if let AstNode::Var(var_name) = field_pattern {
                                    // Extract the field value from the enum
                                    let field_id = self.next_id();
                                    let extract_func = if variant == "Option::Some" {
                                        "option_get_data"
                                    } else if variant == "Result::Ok"
                                        || variant == "Result::Err"
                                    {
                                        "host_result_get_data"
                                    } else {
                                        // No data to extract
                                        continue;
                                    };

                                    // Call runtime function to extract the data
                                    self.stmts.push(MirStmt::Call {
                                        func: extract_func.to_string(),
                                        args: vec![scrutinee_id],
                                        dest: field_id,
                                        type_args: vec![],
                                    });
                                    self.exprs.insert(field_id, MirExpr::Var(field_id));
                                    self.type_map.insert(field_id, Type::I64);
                                    self.name_to_id.insert(var_name.clone(), field_id);
                                }
                            }

                            self.exprs.insert(cond_id, MirExpr::Var(final_check_id));
                            self.type_map.insert(cond_id, Type::Bool);
                        } else if let Some((_, tag, _)) = self.enum_variant_of(variant) {
                            // A payload-carrying variant of a registered
                            // user enum (`Token::Ident(n)`, `Ast::Lit(n)`).
                            //
                            // This used to be the `else` branch below, which
                            // bound every field name to a literal `0` and
                            // set the condition to "always true" — so
                            // `match t { Token::Ident(n) => n + 1, _ => 900 }`
                            // took the first arm for ANY value and printed 1
                            // (measured). The block that such a variant now
                            // builds is `[tag, p0, …]` (see `enum_is_boxed`),
                            // so the test reads slot 0 and each binding reads
                            // slot 1+k.
                            let cond = self.boxed_tag_guard(scrutinee_id, tag);
                            for (k, (_field_name, field_pattern)) in
                                fields.iter().enumerate()
                            {
                                if let AstNode::Var(var_name) = field_pattern {
                                    let slot =
                                        self.deref_slot(scrutinee_id, 8 * (k as i64 + 1));
                                    self.name_to_id.insert(var_name.clone(), slot);
                                }
                            }
                            self.exprs.insert(cond_id, MirExpr::Var(cond));
                            self.type_map.insert(cond_id, Type::Bool);
                        } else {
                            // Nothing registered this variant name, so there is
                            // no tag to compare against and no slot to read the
                            // payload from: the arm CANNOT be tested.
                            //
                            // This used to set the condition to "always true"
                            // and bind every field to `0`, which made an
                            // untestable arm claim a match on any value —
                            // `match 5 { Foo(n) => n, _ => 7 }` printed 0
                            // (measured) instead of falling through. Fail
                            // closed instead: the arm never matches, so the
                            // value goes to the arms that CAN answer, and the
                            // wildcard or a later arm decides.
                            for (_field_name, field_pattern) in fields {
                                if let AstNode::Var(var_name) = field_pattern {
                                    // Register the names so the arm body still
                                    // resolves; it is unreachable either way.
                                    let field_id = self.next_id();
                                    self.name_to_id.insert(var_name.clone(), field_id);
                                    self.exprs.insert(field_id, MirExpr::IntLit(0));
                                    self.type_map.insert(field_id, Type::I64);
                                }
                            }
                            self.exprs.insert(cond_id, MirExpr::IntLit(0));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                    }
                    AstNode::TypeAnnotatedPattern {
                        pattern: inner_pattern,
                        ty: _,
                    } => {
                        // For type-annotated patterns, extract the inner pattern
                        // The type checking should have been done by the type checker
                        match &**inner_pattern {
                            AstNode::Var(var_name) => {
                                // Regular variable binding pattern - always matches
                                // Add binding to name_to_id so the arm body can reference it
                                self.name_to_id.insert(var_name.clone(), scrutinee_id);
                                self.exprs.insert(cond_id, MirExpr::IntLit(1));
                                self.type_map.insert(cond_id, Type::Bool);
                            }
                            _ => {
                                // For other inner patterns, treat as always false for now
                                self.exprs.insert(cond_id, MirExpr::IntLit(0));
                                self.type_map.insert(cond_id, Type::Bool);
                            }
                        }
                    }
                    AstNode::OrPattern(patterns) => {
                        // Or pattern: any sub-pattern matches.
                        // Lower each sub-pattern and OR their conditions.
                        let mut or_cond = None;
                        for sub_pat in patterns {
                            let sub_pat_id = self.next_id();
                            // Lower the sub-pattern as a match condition on scrutinee.
                            // Re-use the scrutinee_id — sub-pattern checks reference it.
                            let sub_lit_id = self.next_id();
                            match sub_pat {
                                AstNode::Lit(val) => {
                                    self.exprs.insert(sub_lit_id, MirExpr::IntLit(*val));
                                    self.type_map.insert(sub_lit_id, Type::I64);
                                    self.stmts.push(MirStmt::Call {
                                        func: "==".to_string(),
                                        args: vec![scrutinee_id, sub_lit_id],
                                        dest: sub_pat_id,
                                        type_args: vec![],
                                    });
                                }
                                AstNode::StringLit(val) => {
                                    // Same reason as the top-level string
                                    // arm: `Call "=="` has an i64 signature
                                    // and would compare pointers.
                                    self.exprs
                                        .insert(sub_lit_id, MirExpr::StringLit(val.clone()));
                                    self.type_map.insert(sub_lit_id, Type::Str);
                                    self.exprs.insert(
                                        sub_pat_id,
                                        MirExpr::BinaryOp {
                                            op: "==".to_string(),
                                            left: scrutinee_id,
                                            right: sub_lit_id,
                                        },
                                    );
                                }
                                AstNode::Var(name) if name == "_" => {
                                    // Wildcard always matches
                                    self.exprs.insert(sub_pat_id, MirExpr::IntLit(1));
                                    self.type_map.insert(sub_pat_id, Type::Bool);
                                }
                                _ => {
                                    // Fallback for other sub-patterns
                                    self.exprs.insert(sub_pat_id, MirExpr::IntLit(0));
                                    self.type_map.insert(sub_pat_id, Type::Bool);
                                }
                            }
                            // The arms above that lower through a
                            // `MirStmt::Call` leave `sub_pat_id` as a
                            // register to load; the string arm already wrote
                            // an inline condition expr there, which this
                            // must not clobber.
                            if !matches!(self.exprs.get(&sub_pat_id), Some(MirExpr::BinaryOp { .. }))
                            {
                                self.exprs.insert(sub_pat_id, MirExpr::Var(sub_pat_id));
                            }
                            self.type_map.insert(sub_pat_id, Type::Bool);

                            if let Some(prev) = or_cond {
                                // OR the conditions: prev || sub_pat
                                let or_result_id = self.emit_call("||", vec![prev, sub_pat_id], Type::Bool);
                                or_cond = Some(or_result_id);
                            } else {
                                or_cond = Some(sub_pat_id);
                            }
                        }
                        let cond_val = or_cond.unwrap_or_else(|| {
                            let default = self.next_id();
                            self.exprs.insert(default, MirExpr::IntLit(1));
                            self.type_map.insert(default, Type::Bool);
                            default
                        });
                        self.exprs.insert(cond_id, MirExpr::Var(cond_val));
                        self.type_map.insert(cond_id, Type::Bool);
                    }
                    AstNode::BindPattern {
                        name,
                        pattern: inner,
                    } if matches!(&**inner, AstNode::Ignore) => {
                        // PY-A: `q @ _` — the inner pattern is the wildcard, so
                        // the arm matches everything and the binding is the
                        // whole point. Without this case the fallback below
                        // lowered `_` as a *value*, and `lower_expr` gives a
                        // wildcard `IntLit(0)`; the arm became
                        // `scrutinee == 0`, so `match 4 { q @ _ => q + 1 }`
                        // caught nothing and returned the match result slot
                        // nothing ever wrote. The `=> true` reading is the one
                        // the bare-`_` arm above already uses (#38 ⑤).
                        self.name_to_id.insert(name.clone(), scrutinee_id);
                        self.exprs.insert(cond_id, MirExpr::IntLit(1));
                        self.type_map.insert(cond_id, Type::Bool);
                    }
                    AstNode::BindPattern {
                        name,
                        pattern: inner,
                    } => {
                        // x @ pattern: bind name to scrutinee, then match inner pattern.
                        self.name_to_id.insert(name.clone(), scrutinee_id);
                        // Check the inner pattern
                        let inner_cond_id = self.next_id();
                        let cond_val = match &**inner {
                            AstNode::RangePattern {
                                start,
                                end,
                                inclusive,
                            } => {
                                // x @ start..end: bind, then guard the range. The
                                // guard ends in a stored `Call` dest and has to be
                                // read through THAT id — an extra `Var(inner_cond_id)`
                                // hop read an alloca nothing ever wrote, and `-O`
                                // lowers that load to `brk #0x1` (SIGTRAP).
                                self.lower_range_guard(scrutinee_id, start, end, *inclusive)
                            }
                            _ => {
                                // Other inner patterns: match by lowering.
                                let inner_id = self.lower_expr(inner);
                                self.stmts.push(MirStmt::Call {
                                    func: "==".to_string(),
                                    args: vec![scrutinee_id, inner_id],
                                    dest: inner_cond_id,
                                    type_args: vec![],
                                });
                                inner_cond_id
                            }
                        };
                        self.exprs.insert(cond_val, MirExpr::Var(cond_val));
                        self.type_map.insert(cond_val, Type::Bool);
                        self.exprs.insert(cond_id, MirExpr::Var(cond_val));
                        self.type_map.insert(cond_id, Type::Bool);
                    }
                    AstNode::RangePattern {
                        start,
                        end,
                        inclusive,
                    } => {
                        let and_id =
                            self.lower_range_guard(scrutinee_id, start, end, *inclusive);
                        self.exprs.insert(cond_id, MirExpr::Var(and_id));
                        self.type_map.insert(cond_id, Type::Bool);
                    }
                    AstNode::Tuple(elements) => {
                        // Tuple pattern: match each element (simplified: always true for now)
                        // In a full implementation, we'd destructure and match each element.
                        // For now, bind elements by index position.
                        for (i, elem) in elements.iter().enumerate() {
                            if let AstNode::Var(name) = elem {
                                let field_id = self.next_id();
                                self.name_to_id.insert(name.clone(), field_id);
                                self.exprs.insert(field_id, MirExpr::IntLit(0));
                                self.type_map.insert(field_id, Type::I64);
                            }
                        }
                        self.exprs.insert(cond_id, MirExpr::IntLit(1));
                        self.type_map.insert(cond_id, Type::Bool);
                    }
                    _ => {
                        // For now, treat other patterns as always false
                        self.exprs.insert(cond_id, MirExpr::IntLit(0));
                        self.type_map.insert(cond_id, Type::Bool);
                    }
                }

                // Handle guard clause if present
                let final_cond_id = if let Some(ref guard) = arm.guard {
                    // Lower the guard expression
                    let guard_id = self.lower_expr(guard);

                    // Create AND condition: pattern_matches && guard_condition
                    let and_cond_id = self.emit_call("&&", vec![cond_id, guard_id], Type::Bool);

                    and_cond_id
                } else {
                    cond_id
                };

                // Now lower the arm body (after establishing pattern bindings)
                // If the arm body is a `return` statement, emit a Return
                // (lower_expr has no Return arm and would fabricate 0).
                // PY-A: anything this lowering pushes into `self.stmts` belongs
                // to THIS arm — before the drain below it was left in the
                // enclosing function, so every arm's side effects ran instead of
                // only the matched one. Measured on `match 2 { 1 => n+=1,
                // 2 => n+=10, _ => n+=100 }`:改前退出码 111（三条都跑），改后 10。
                let arm_body_start = self.stmts.len();
                let mut then_branch: Vec<MirStmt> = Vec::new();
                if let AstNode::Return(inner) = &*arm.body {
                    let ret_val = self.lower_expr(inner);
                    if let Some(ty) = self.type_map.get(&ret_val).cloned() {
                        arm_value_tys.push(ty);
                    }
                    then_branch.push(MirStmt::Return { val: ret_val });
                } else if let AstNode::Block { body } = &*arm.body {
                    // Batch 758 (#38①a): case-block arms lower by STATEMENT
                    // semantics — a `return` inside the arm block is a
                    // function return (lower_expr has no Return arm and
                    // fabricated 0; measured: `case 1: return "one"` fell
                    // through to the fn default and the caller read 0).
                    // Statements push into self.stmts and the drain below
                    // moves them into THIS arm's branch, in body order —
                    // the Return stays last. The last expression value
                    // feeds the result slot (a promoted match IS the fn
                    // tail value), but a `return` wins: no slot write
                    // after it (an Assign past the Return was a terminator
                    // in the middle of the basic block, measured).
                    let mut last_val: Option<u32> = None;
                    let mut returned = false;
                    for st in body {
                        match st {
                            AstNode::Return(inner) => {
                                let ret_val = self.lower_expr(inner);
                                if let Some(ty) = self.type_map.get(&ret_val).cloned() {
                                    arm_value_tys.push(ty);
                                }
                                then_branch.push(MirStmt::Return { val: ret_val });
                                returned = true;
                                last_val = None;
                            }
                            AstNode::ExprStmt { expr } => {
                                last_val = Some(self.lower_expr(expr));
                            }
                            other => {
                                self.lower_ast(other);
                                last_val = None;
                            }
                        }
                    }
                    if !returned {
                        if let Some(v) = last_val {
                            if let Some(ty) = self.type_map.get(&v).cloned() {
                                arm_value_tys.push(ty);
                            }
                            then_branch.push(MirStmt::Assign {
                                lhs: result_id,
                                rhs: v,
                            });
                        }
                    }
                } else {
                    let arm_body_id = self.lower_expr(&arm.body);
                    if let Some(ty) = self.type_map.get(&arm_body_id).cloned() {
                        arm_value_tys.push(ty);
                    }
                    then_branch.push(MirStmt::Assign {
                        lhs: result_id,
                        rhs: arm_body_id,
                    });
                };
                let arm_body_end = self.stmts.len();
                if arm_body_end > arm_body_start {
                    let mut arm_stmts = self.stmts.split_off(arm_body_start);
                    arm_stmts.append(&mut then_branch);
                    then_branch = arm_stmts;
                }

                let if_stmt = MirStmt::If {
                    cond: final_cond_id,
                    then: then_branch,
                    else_: else_branch,
                    dest: None,
                };

                // For the next iteration, the current if becomes the else branch
                else_branch = vec![if_stmt];
            }

            // The final else_branch contains the complete if-else chain
            // Add it to statements
            if !else_branch.is_empty() {
                // The chain starts with the first arm's if statement
                self.stmts.extend(else_branch);
            }

            self.exprs.insert(id, MirExpr::Var(id));
            // Unify: every arm has to store a value of the same type. A
            // `return` arm leaves the slot unwritten, and mixed types have no
            // single answer, so both cases keep the old `I64`.
            let all_same = arm_value_tys.len() == arms.len()
                && arm_value_tys.first().map_or(false, |first| {
                    arm_value_tys.iter().all(|t| t == first)
                });
            let result_ty = if all_same {
                arm_value_tys[0].clone()
            } else {
                Type::I64
            };
            self.type_map.insert(id, result_ty);
            id
    }
}
