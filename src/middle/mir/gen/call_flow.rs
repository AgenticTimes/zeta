//! 批次 861：Loop 表达式臂发射体（批 860 的迁移从未编译通过，本批重做）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// `loop { ... }` 表达式：结果槽缺省 0，`break EXPR` 写入。
    /// 批 859 前原臂逐字迁入（`id` 即调用方 lower_expr_node 的目的槽）。
    pub(super) fn lower_loop(&mut self, body: &[AstNode], id: u32) -> u32 {
        // Loop expression: result slot (default 0); break EXPR writes it.
        let result_id = self.next_id();
        self.exprs.insert(result_id, MirExpr::IntLit(0));
        self.type_map.insert(result_id, Type::I64);
        self.loop_value_stack.push(result_id);

        let stmts_before = self.stmts.len();
        for stmt in body {
            self.lower_ast(stmt);
        }
        let loop_stmts = self.stmts.split_off(stmts_before);
        self.loop_value_stack.pop();

        let cond_id = self.next_id();
        self.exprs.insert(cond_id, MirExpr::IntLit(1));
        self.type_map.insert(cond_id, Type::I64);
        self.stmts.push(MirStmt::While {
            cond: cond_id,
            pre_cond: vec![],
            body: loop_stmts,
            else_body: vec![],
        });

        self.exprs.insert(result_id, MirExpr::Var(result_id));
        self.type_map.insert(result_id, Type::I64);
        self.exprs.insert(id, MirExpr::Var(result_id));
        self.type_map.insert(id, Type::I64);
        result_id
    }

    /// `while cond { body } else { ... }` 语句（批 862 自 gen.rs 原臂逐字迁入）。
    pub(super) fn lower_while_stmt(
        &mut self,
        cond: &AstNode,
        body: &[AstNode],
        else_body: &[AstNode],
    ) {
        // Cond side-effects (e.g. `array_len` in `while j < len(xs)`)
        // must re-run every iteration *before* the condition load —
        // including on `continue`. Put them in `pre_cond`, not body.
        let stmts_before_cond = self.stmts.len();
        let cond_id = self.lower_expr(cond);
        let pre_cond = self.stmts.split_off(stmts_before_cond);

        let stmts_before_body = self.stmts.len();
        for stmt in body {
            self.lower_ast(stmt);
        }
        let body_stmts = self.stmts.split_off(stmts_before_body);

        let else_stmts = self.lower_loop_else(else_body);
        self.stmts.push(MirStmt::While {
            cond: cond_id,
            pre_cond,
            body: body_stmts,
            else_body: else_stmts,
        });
    }

    /// `for pattern in expr { body } else { ... }` 语句（批 863 自 gen.rs 原臂逐字迁入；
    /// 仅签名适配：pattern/expr 按 &AstNode 传入，四处 Box 解引用随改）。
    pub(super) fn lower_for_stmt(
        &mut self,
        pattern: &AstNode,
        expr: &AstNode,
        body: &[AstNode],
        else_body: &[AstNode],
    ) {
            // For now, implement simple desugaring for range-based for loops
            // for i in start..end { body } desugars to:
            // let mut i = start;
            // while i < end {
            //   body;
            //   i = i + 1;
            // }

            // Check if expr is a range expression (BinaryOp with ".." or AstNode::Range)
            let (start_expr, end_expr): (Box<AstNode>, Box<AstNode>) = match expr {
                AstNode::BinaryOp { op, left, right } if op == ".." => {
                    ((*left).clone(), (*right).clone())
                }
                AstNode::Range {
                    start,
                    end,
                    inclusive: _,
                } => ((*start).clone(), (*end).clone()),
                _ => {
                    // Collection iteration: for item in collection { ... }
                    // Clone needed data to avoid borrow conflicts
                    let body_clone = body.clone();
                    let pattern_clone = pattern.clone();
                    let expr_clone = expr.clone();

                    // Any collection expression is supported: materialize it
                    // into a slot first so the array expression is evaluated
                    // exactly once (a StackArray literal would otherwise be
                    // re-allocated on every reference).
                    {
                        let raw_id = self.lower_expr(&expr_clone);
                        // PY-A: `for k in d:` over a dict iterates its KEYS.
                        // The map layout has no array length, so the loop
                        // silently ran zero times before.
                        // `for k, grp in df.groupby(col)` — call the runtime
                        // grouping DIRECTLY with the call's own receiver/key
                        // (probed: going through the shim's `GroupBy` struct
                        // delivered key == 0, so every group collapsed).
                        let grp_call = match &expr_clone {
                            AstNode::Call {
                                receiver: Some(recv),
                                method,
                                args,
                                ..
                            } if method == "groupby" && !args.is_empty() => {
                                Some((recv.clone(), args[0].clone()))
                            }
                            _ => None,
                        };
                        let raw_id = if let Some((recv, key)) = grp_call {
                            let recv_id = self.lower_expr(&recv);
                            let key_id = self.lower_expr(&key);
                            let pid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_df_groupby".to_string(),
                                args: vec![recv_id, key_id],
                                dest: pid,
                                type_args: vec![],
                            });
                            self.exprs.insert(pid, MirExpr::Var(pid));
                            self.type_map.insert(
                                pid,
                                Type::DynamicArray(Box::new(Type::Tuple(vec![
                                    Type::Str,
                                    Type::Named("DataFrame".to_string(), vec![]),
                                ]))),
                            );
                            pid
                        } else if matches!(
                            self.type_map.get(&raw_id),
                            Some(Type::Named(n, _)) if n == "GroupBy"
                        ) {
                            let pid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_groupby_pairs".to_string(),
                                args: vec![raw_id],
                                dest: pid,
                                type_args: vec![],
                            });
                            self.exprs.insert(pid, MirExpr::Var(pid));
                            self.type_map.insert(
                                pid,
                                Type::DynamicArray(Box::new(Type::Tuple(vec![
                                    Type::Str,
                                    Type::Named("DataFrame".to_string(), vec![]),
                                ]))),
                            );
                            pid
                        } else if matches!(
                            self.type_map.get(&raw_id),
                            Some(Type::Named(n, _)) if n == "map"
                        ) {
                            let kid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "map_keys".to_string(),
                                args: vec![raw_id],
                                dest: kid,
                                type_args: vec![],
                            });
                            self.exprs.insert(kid, MirExpr::Var(kid));
                            self.type_map.insert(
                                kid,
                                Type::DynamicArray(Box::new(match self
                                    .type_map
                                    .get(&raw_id)
                                    .cloned()
                                {
                                    Some(Type::Named(_, args))
                                        if matches!(args.first(), Some(Type::Str)) =>
                                    {
                                        Type::Str
                                    }
                                    _ => Type::I64,
                                })),
                            );
                            kid
                        } else {
                            raw_id
                        };
                        let coll_slot = self.next_id();
                        self.exprs.insert(coll_slot, MirExpr::Var(coll_slot));
                        self.type_map.insert(
                            coll_slot,
                            self.type_map.get(&raw_id).cloned().unwrap_or_else(Type::slot_fallback),
                        );
                        self.stmts.push(MirStmt::Assign {
                            lhs: coll_slot,
                            rhs: raw_id,
                        });
                        let collection_id = coll_slot;
                        // Batch 110: `for c in "ab"` — Str is char*, not a
                        // vec header. array_len/array_get → hang / garbage.
                        let coll_is_str =
                            matches!(self.type_map.get(&collection_id), Some(Type::Str));
                        let (len_fn, get_fn) = if coll_is_str {
                            ("str_len", "str_get")
                        } else {
                            ("array_len", "array_get")
                        };
                        let len_id = self.next_id();
                        // NOTE: the id must be a Var (mutable slot), not an
                        // IntLit placeholder — codegen constant-folds a
                        // literal id and would drop the array_len result,
                        // making the loop bound a constant 0.
                        self.exprs.insert(len_id, MirExpr::Var(len_id));
                        self.stmts.push(MirStmt::Call {
                            func: len_fn.to_string(),
                            args: vec![collection_id],
                            dest: len_id,
                            type_args: vec![],
                        });
                        self.type_map.insert(len_id, Type::I64);

                        let start_id = self.next_id_with_lit(0);

                        // Create index variable name
                        let (var_name, is_simple_var) = if let AstNode::Var(n) = &pattern_clone
                        {
                            (n.clone(), true)
                        } else {
                            ("_i_".to_string(), false)
                        };
                        let index_var_id = self.next_id();
                        self.name_to_id.insert(var_name, index_var_id);
                        self.exprs.insert(index_var_id, MirExpr::Var(index_var_id));
                        self.type_map.insert(index_var_id, Type::I64);

                        // Initialize index = 0
                        self.stmts.push(MirStmt::Assign {
                            lhs: index_var_id,
                            rhs: start_id,
                        });

                        // while index < len
                        let cond_id = self.next_id();
                        self.exprs.insert(
                            cond_id,
                            MirExpr::BinaryOp {
                                op: "<".to_string(),
                                left: index_var_id,
                                right: len_id,
                            },
                        );
                        self.type_map.insert(cond_id, Type::Bool);

                        let stmts_before = self.stmts.len();

                        // Bind the pattern to collection[index].
                        let get_id = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: get_fn.to_string(),
                            args: vec![collection_id, index_var_id],
                            dest: get_id,
                            type_args: vec![],
                        });
                        // Carry the collection's element type to the
                        // loop item so `for k in d.keys(): print(k)`
                        // dispatches as a string (only pointer-shaped
                        // element types; F64 elements live as raw bits).
                        let elem_ty = if coll_is_str {
                            Some(Type::Str)
                        } else {
                            match self.type_map.get(&collection_id).cloned() {
                                Some(Type::DynamicArray(e))
                                | Some(Type::Array(e, _)) => match *e {
                                    Type::Str => Some(Type::Str),
                                    Type::Named(n, args) => Some(Type::Named(n, args)),
                                    _ => None,
                                },
                                _ => None,
                            }
                        };
                        match &pattern_clone {
                            AstNode::Var(item_name) => {
                                self.name_to_id.insert(item_name.clone(), get_id);
                                self.exprs.insert(get_id, MirExpr::Var(get_id));
                                self.type_map
                                    .insert(get_id, elem_ty.unwrap_or(Type::I64));
                            }
                            // `for k, v in pairs:` — destructure the element.
                            // Previously a Tuple pattern bound NO names at
                            // all, so k/v resolved to whatever was in scope
                            // (silently wrong values).
                            AstNode::Tuple(names) => {
                                self.exprs.insert(get_id, MirExpr::Var(get_id));
                                self.type_map.insert(get_id, Type::I64);
                                // A Vec<(k, v)> element carries per-position
                                // types, so `for k, c in most_common()` can
                                // type the key part as str.
                                let pair_tys = match self.type_map.get(&collection_id) {
                                    Some(Type::DynamicArray(e))
                                    | Some(Type::Array(e, _)) => match &**e {
                                        Type::Tuple(ts) => Some(ts.clone()),
                                        _ => None,
                                    },
                                    _ => None,
                                };
                                // Batch 288: a json.items() pair — the value
                                // half is a PyJson handle (the array element
                                // type is plain i64, see py_json_items_ids at
                                // the `.items()` lowering). BATCH-296: the
                                // KEY half is text — `py_map_items` hands
                                // back `zt_key_display(key)`, and a JSON
                                // object's keys are always strings. Typed as
                                // i64 it defeated content hashing on insert
                                // (`{k: v for k, v in d.items()}` stored raw
                                // pointers → every later `"512050.XSHG" in m`
                                // missed) and `str(k)` stringified the
                                // POINTER: the ETF listing cache came back as
                                // 1 entry, the 上市日 filter dropped nothing
                                // and 28 codes fell to the network.
                                let pair_tys = pair_tys.or_else(|| {
                                    if self.py_json_items_ids.contains(&raw_id) {
                                        Some(vec![
                                            Type::Str,
                                            Type::Named("PyJson".to_string(), vec![]),
                                        ])
                                    } else {
                                        None
                                    }
                                });
                                for (i, n) in names.iter().enumerate() {
                                    if let AstNode::Var(nm) = n {
                                        let idx_id = self.next_id_with_lit(i as i64);
                                        let part = self.next_id();
                                        self.stmts.push(MirStmt::Call {
                                            func: "stack_array_get".to_string(),
                                            args: vec![get_id, idx_id],
                                            dest: part,
                                            type_args: vec![],
                                        });
                                        self.name_to_id.insert(nm.clone(), part);
                                        self.exprs.insert(part, MirExpr::Var(part));
                                        let pty = pair_tys
                                            .as_ref()
                                            .and_then(|ts| ts.get(i).cloned())
                                            .unwrap_or(Type::I64);
                                        self.type_map.insert(part, pty);
                                    }
                                }
                            }
                            _ => {}
                        }

                        // t425: names this loop binds that are also module
                        // globals. Carved out of the env-first read while
                        // the body lowers; mirrored back into the env cell
                        // after the loop (Python leaks the last iterated
                        // value — the splice cannot cover this shape, the
                        // item bind is a Call, not an Assign).
                        let loop_globals: Vec<String> = if self.py_entry {
                            match &pattern_clone {
                                AstNode::Var(item_name) => {
                                    if self.module_globals.contains(item_name) {
                                        vec![item_name.clone()]
                                    } else {
                                        vec![]
                                    }
                                }
                                AstNode::Tuple(names) => names
                                    .iter()
                                    .filter_map(|n| match n {
                                        AstNode::Var(nm)
                                            if self.module_globals.contains(nm) =>
                                        {
                                            Some(nm.clone())
                                        }
                                        _ => None,
                                    })
                                    .collect(),
                                _ => vec![],
                            }
                        } else {
                            vec![]
                        };
                        for n in &loop_globals {
                            self.loop_var_active.insert(n.clone());
                        }

                        // i = i + 1 — advance BEFORE the user body.
                        //
                        // `continue` lowers to a jump to the loop's
                        // CONDITION block, which skips the rest of the
                        // body. With the increment at the END of the body
                        // that meant the index never moved and
                        //     for i in [1, 2, 3]:
                        //         if i == 2: continue
                        // looped forever (a hang — worse than a wrong
                        // value). Advancing here is invisible to the body:
                        // the user-visible name is bound to the ELEMENT
                        // (below), and only the internal index slot and the
                        // condition read `index_var_id`.
                        let inc_id = self.next_id();
                        let one_id = self.next_id_with_lit(1);
                        self.exprs.insert(
                            inc_id,
                            MirExpr::BinaryOp {
                                op: "+".to_string(),
                                left: index_var_id,
                                right: one_id,
                            },
                        );
                        self.type_map.insert(inc_id, Type::I64);
                        self.stmts.push(MirStmt::Assign {
                            lhs: index_var_id,
                            rhs: inc_id,
                        });

                        for stmt in body_clone {
                            self.lower_ast(stmt);
                        }

                        let body_stmts = self.stmts.split_off(stmts_before);
                        // PY-A: `for … else` — lowered AFTER the split so
                        // its statements don't land inside the loop body.
                        let else_stmts = self.lower_loop_else(else_body);
                        self.stmts.push(MirStmt::While {
                            cond: cond_id,
                            pre_cond: vec![],
                            body: body_stmts,
                            else_body: else_stmts,
                        });
                        // t425: env-first reads resume here — hand the
                        // env cell the loop's last bound value (and clear
                        // the carve-out). An empty collection leaves the
                        // item slot uninitialized; mirroring garbage is
                        // the same wrongness the slot read had before.
                        for n in &loop_globals {
                            self.loop_var_active.remove(n);
                            if let Some(&slot) = self.name_to_id.get(n) {
                                let (mirror, _key) = self.env_mirror(n, slot);
                                self.stmts.extend(mirror);
                            }
                        }
                    }
                    return;
                }
            };

            // Get variable name from pattern. A bare `_` (AstNode::Ignore)
            // is the usual "I don't need the index" spelling — it must still
            // drive the range loop. Before this the wildcard fell through to
            // the COLLECTION path, which iterated the Range as a collection
            // and ran the body zero times, silently (`for _ in range(3)`
            // printed nothing instead of looping).
            let range_var: Option<String> = match pattern {
                AstNode::Var(n) => Some(n.clone()),
                AstNode::Ignore => Some("__wildcard".to_string()),
                // `for i: usize in …` carries the annotation as a wrapper
                // around the name. Unwrapping it here keeps the range path;
                // previously the wrapper missed all three arms, the loop
                // fell through to the COLLECTION path, and a Range iterated
                // as an empty collection — body ran zero times, silently.
                AstNode::TypeAnnotatedPattern { pattern: inner, .. } => match &**inner {
                    AstNode::Var(n) => Some(n.clone()),
                    AstNode::Ignore => Some("__wildcard".to_string()),
                    _ => None,
                },
                _ => None,
            };
            if let Some(var_name) = &range_var {
                // Lower start and end expressions
                let start_id = self.lower_expr(&start_expr);
                let end_id = self.lower_expr(&end_expr);

                // Create loop variable
                let var_id = self.next_id();
                self.name_to_id.insert(var_name.clone(), var_id);
                self.exprs.insert(var_id, MirExpr::Var(var_id));
                self.type_map.insert(var_id, Type::I64);

                // PY-A (任务 #54): the induction counter is a SECOND slot.
                // Sharing `var_id` made the loop's bookkeeping observable:
                // `for k in range(3)` left k=3 (Python: 2, the last bound
                // value), and a body that wrote k advanced from *that* value
                // (`for k in range(3): k = 9` ran once and left k=10).
                let counter_id = self.next_id();
                self.exprs.insert(counter_id, MirExpr::Var(counter_id));
                self.type_map.insert(counter_id, Type::I64);

                // Initialize loop variable: let mut i = start
                self.stmts.push(MirStmt::Assign {
                    lhs: var_id,
                    rhs: start_id,
                });
                self.stmts.push(MirStmt::Assign {
                    lhs: counter_id,
                    rhs: start_id,
                });

                // Create a range iterator expression
                // For range start..end, we need to create an iterator
                // For now, we'll create a simple representation
                let range_id = self.next_id();
                self.exprs.insert(
                    range_id,
                    MirExpr::Range {
                        start: start_id,
                        end: end_id,
                    },
                );
                self.type_map.insert(range_id, Type::Range);

                // Save current statements to restore after loop body
                let stmts_before_body = self.stmts.len();

                // t425: the loop var's per-iteration value must win over
                // the env-first read while the body lowers (env-first
                // reads resume after the For, where the splice mirrors
                // have already kept the cell fresh — the per-iteration
                // `Assign{var_id, counter}` is mirrored like any assign).
                let var_is_global =
                    self.py_entry && self.module_globals.contains(var_name);
                if var_is_global {
                    self.loop_var_active.insert(var_name.clone());
                }

                // Generate loop body
                for stmt in body {
                    self.lower_ast(stmt);
                }

                // Get body statements
                let mut body_stmts = self.stmts.split_off(stmts_before_body);

                // PY-A (任务 #54): bind the user-visible name to the counter
                // at the TOP of every taken iteration — that is what Python
                // does (`for k in range(3)` binds 0, 1, 2 and the iterator
                // then stops without binding 3). Prepending after the split
                // keeps it invisible to the body's own statement list.
                body_stmts.insert(
                    0,
                    MirStmt::Assign {
                        lhs: var_id,
                        rhs: counter_id,
                    },
                );

                // PY-A: `for … else` — lowered after the split.
                let else_stmts = self.lower_loop_else(else_body);

                // Create For statement in MIR
                self.stmts.push(MirStmt::For {
                    iterator: range_id,
                    pattern: var_name.clone(),
                    var_id,
                    counter_id,
                    body: body_stmts,
                    else_body: else_stmts,
                });
                if var_is_global {
                    self.loop_var_active.remove(var_name);
                }
            }
    }
}
