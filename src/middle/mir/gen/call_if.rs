//! 批次 854：If 表达式发射体。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
PY-A If 表达式发射体（then/else 控制流）——原臂逐字。
pub(super) fn lower_if_expr(&mut self, cond: &Box<AstNode>, then: &Box<AstNode>, else_: &Option<Box<AstNode>>, dest: u32) {

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
    }

    AstNode::DictLit { entries } => {
        let map_id = id;
        self.stmts.push(MirStmt::MapNew { dest: map_id });
        let mut key_ty = Type::I64;
        // The VALUE type is tracked as the map's second type argument, so
        // `a = m["code"]` keeps DynamicArray(Str) instead of degrading to
        // I64 — `a[0]` then did a map_get on a Vec handle (SEGV, t193).
        let mut val_ty = Type::I64;
        let mut first_key = true;
        let mut first_val = true;
        // 批次 806：全 None 值判定（混型不动）
        let mut all_none_val = true;
        // 批次 808（混型 dict None 值格）：含 None 且含非 None ⇒
        // 混型——值型 PyDynamic（读侧 17638 格标签读随之点亮），
        // 字面量值写侧打格标签（8=None、4=文本、5=i64、6=f64 位、
        // 7=bool），print 位运行期按格渲染。非字面量值 tag 0 落
        // i64 缺省（残界）。纯字典零新增面。
        let has_none = entries.iter().any(|(_, v)| matches!(v, AstNode::NoneLit));
        let mixed = has_none && entries.iter().any(|(_, v)| !matches!(v, AstNode::NoneLit));
        for (k, v) in entries {
            // PY-A: `{**m, ...}` — merge m's entries into the literal.
            if let AstNode::Call {
                receiver: None,
                method,
                args: ka,
                ..
            } = k
            {
                if method == "zeta_dict_spread" && ka.len() == 1 {
                    let src_id = self.lower_expr(&ka[0]);
                    // Take the key kind from the source map so a
                    // spread-only literal (`{**a}`) is still
                    // string-keyed and `d.keys()` stays Vec<str>.
                    if first_key {
                        if let Some(Type::Named(n, params)) =
                            self.type_map.get(&src_id).cloned()
                        {
                            if n == "map" {
                                if let Some(kt) = params.first() {
                                    key_ty = kt.clone();
                                    first_key = false;
                                }
                            }
                        }
                    }
                    let scratch = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "py_map_update".to_string(),
                        args: vec![map_id, src_id],
                        dest: scratch,
                        type_args: vec![],
                    });
                    continue;
                }
            }
            let kid0 = self.lower_expr(k);
            if first_key {
                // Remember whether keys are strings: the map type carries
                // the key kind so `d.keys()` is typed Vec<str>.
                key_ty = self.type_map.get(&kid0).cloned().unwrap_or_else(Type::slot_fallback);
                first_key = false;
            }
            let kid = self.lower_map_key(kid0);
            let vid = self.lower_expr(v);
            if first_val {
                val_ty = self.type_map.get(&vid).cloned().unwrap_or_else(Type::slot_fallback);
                first_val = false;
            }
            // 批次 806（#113 携带读面）：全 None 值字典 ⇒ 值型
            // NoneValue（print 按型渲染 "None"，804 fromkeys 同款）。
            // 混型字典不动（首个值型 Wins 的旧约定，per-key 渲染另格）。
            if !matches!(v, AstNode::NoneLit) {
                all_none_val = false;
            }
            self.stmts.push(MirStmt::DictInsert {
                map_id,
                key_id: kid,
                val_id: vid,
            });
            // 批次 808：混型字典的字面量值打格标签（767 值标签大弧
            // 写侧；读侧格标签读由值型 PyDynamic 点亮，print 位按格
            // 渲染）。非字面量值不打（tag 0 落 i64 缺省＝残界）。
            if mixed {
                let tag: Option<i64> = match v {
                    AstNode::NoneLit => Some(8),
                    AstNode::StringLit(_) => Some(4),
                    AstNode::Lit(_) => Some(5),
                    AstNode::Bool(_) => Some(7),
                    AstNode::FloatLit(_) => Some(6),
                    _ => None,
                };
}
}
