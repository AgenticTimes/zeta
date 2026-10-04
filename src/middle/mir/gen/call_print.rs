//! 批次 828：print 家族的按格渲染（767/808 值标签大弧 print 位）。
//! 路由 8=None 词/4=文本/5=i64/6=f64 位/7=bool 词，全不中落 i64；
//! 自底向上 else-if（len 链同构）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
/// 批次 827：print 位按格渲染链抽为独立方法（808 混型 dict None 值格）。
/// 路由 8=None 词/4=文本/5=i64/6=f64 位/7=bool 词，全不中落 i64；
/// 自底向上 else-if（767 len 链同构）。返回 true＝已发射（调用方 continue）。
pub(super) fn emit_tagged_print(&mut self, arg_id: u32, tag_slot: u32, is_last: bool) -> bool {
                        let nl_id = self.int_slot(is_last as i64);
                        let (s_pr, s_prn) = ("print_str", "println_str");
                        let (i_pr, i_prn) = ("print_i64", "println_i64");
                        let none_str = self.next_id();
                        self.exprs
                            .insert(none_str, MirExpr::StringLit("None".to_string()));
                        self.type_map.insert(none_str, Type::Str);
                        let layers: Vec<(i64, MirStmt)> = vec![
                            (
                                8,
                                MirStmt::VoidCall {
                                    func: (if is_last { s_prn } else { s_pr }).to_string(),
                                    args: vec![none_str],
                                },
                            ),
                            (
                                4,
                                MirStmt::VoidCall {
                                    func: (if is_last { s_prn } else { s_pr }).to_string(),
                                    args: vec![arg_id],
                                },
                            ),
                            (
                                5,
                                MirStmt::VoidCall {
                                    func: (if is_last { i_prn } else { i_pr }).to_string(),
                                    args: vec![arg_id],
                                },
                            ),
                            (
                                6,
                                MirStmt::VoidCall {
                                    func: "zeta_print_f64_word".to_string(),
                                    args: vec![arg_id, nl_id],
                                },
                            ),
                            (
                                7,
                                MirStmt::VoidCall {
                                    func: "zeta_print_bool_word".to_string(),
                                    args: vec![arg_id, nl_id],
                                },
                            ),
                        ];
                        let mut acc: Vec<MirStmt> = vec![MirStmt::VoidCall {
                            func: (if is_last { i_prn } else { i_pr }).to_string(),
                            args: vec![arg_id],
                        }];
                        for (tag, then_stmt) in layers.iter().rev() {
                            let lit = self.next_id();
                            self.exprs.insert(lit, MirExpr::IntLit(*tag));
                            self.type_map.insert(lit, Type::I64);
                            let cond = self.next_id();
                            self.exprs.insert(
                                cond,
                                MirExpr::BinaryOp {
                                    op: "==".to_string(),
                                    left: tag_slot,
                                    right: lit,
                                },
                            );
                            self.type_map.insert(cond, Type::I64);
                            acc = vec![MirStmt::If {
                                cond,
                                then: vec![then_stmt.clone()],
                                else_: std::mem::take(&mut acc),
                                dest: None,
                            }];
                        }
                        self.stmts.extend(acc);
                        return true;
    true
}

    /// 批次 832：print 家族执行者（sep/end 抽取、多参循环、按格渲染、
    /// Bool 尾行补换行——全部从 gen.rs print 臂原样迁入）。
    pub(super) fn lower_print(&mut self, args: &[AstNode], dest: u32) {
                // PY-A: `sep=` / `end=` keywords. They used to be lowered
                // like ordinary arguments, so `print(1, 2, sep="-")` printed
                // "1 2 -" and `end=""` appended a stray value.
                let mut sep_expr: Option<AstNode> = None;
                let mut end_expr: Option<AstNode> = None;
                let mut positional: Vec<&AstNode> = Vec::new();
                for a in args {
                    if let AstNode::Call {
                        receiver: None,
                        method: m,
                        args: ka,
                        ..
                    } = a
                    {
                        if m == "__kwarg__" && ka.len() == 2 {
                            if let AstNode::StringLit(n) = &ka[0] {
                                match n.as_str() {
                                    "sep" => {
                                        sep_expr = Some(ka[1].clone());
                                        continue;
                                    }
                                    "end" => {
                                        end_expr = Some(ka[1].clone());
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    positional.push(a);
                }
                // Batch 598: `print(d.get(k, D))` on a map-typed receiver —
                // the result is an int-or-str UNION a static slot cannot
                // type (batch 576 typed it Str and the hit path deref'd
                // garbage). Split at statement level: each branch prints
                // its own statically typed value. k and D are evaluated
                // eagerly (CPython: D lazy — registered corner). Sole-
                // positional-arg shape only; sep/end keep the generic path.
                if positional.len() == 1 && sep_expr.is_none() && end_expr.is_none() {
                    if let AstNode::Call {
                        receiver: Some(recv),
                        method: m,
                        args: gargs,
                        ..
                    } = positional[0]
                    {
                        if m == "get" && gargs.len() == 2 {
                            let recv_id = self.lower_expr(recv);
                            // 批次 901：收敛到 is_map 唯一判定（Named("dict") 幻影型已证）。
                            let recv_is_map = self
                                .type_map
                                .get(&recv_id)
                                .map_or(false, Type::is_map);
                            if recv_is_map {
                                let k_id = self.lower_expr(&gargs[0]);
                                // Key normalization — the WRITE side
                                // (`DictInsert`) stores `map_str_key(k)`
                                // (content-hash for text, identity for
                                // ints); the probe must hash identically
                                // or every present key reads absent
                                // (t178).
                                let knorm = self.emit_call("map_str_key", vec![k_id], Type::I64);
                                // Existence via py_map_contains DIRECTLY —
                                // the receiver is verified map-typed above;
                                // routing through `__contains__` dispatch
                                // re-derived the receiver's type and could
                                // fall to a dyn/str fallback that answered
                                // False for present keys whose value is 0
                                // (t178: `{"z": 0}` — batch 598).
                                let cond_id = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: "py_map_contains".to_string(),
                                    args: vec![recv_id, knorm],
                                    dest: cond_id,
                                    type_args: vec![],
                                });
                                self.exprs
                                    .insert(cond_id, MirExpr::Var(cond_id));
                                self.type_map.insert(cond_id, Type::Bool);
                                // hit branch: `d[k]` — typed by the map's
                                // declared value type.
                                let outer = std::mem::take(&mut self.stmts);
                                let hit_ast = AstNode::Subscript {
                                    base: recv.clone(),
                                    index: Box::new(gargs[0].clone()),
                                };
                                let hit_id = self.lower_expr(&hit_ast);
                                let hit_ty = self.type_map.get(&hit_id).cloned();
                                let mut then_stmts = std::mem::take(&mut self.stmts);
                                self.stmts = outer;
                                // miss branch: `D` — its own static type.
                                let outer2 = std::mem::take(&mut self.stmts);
                                let miss_id = self.lower_expr(&gargs[1]);
                                let miss_ty = self.type_map.get(&miss_id).cloned();
                                let mut else_stmts = std::mem::take(&mut self.stmts);
                                self.stmts = outer2;
                                let pf = |t: &Option<Type>| match t {
                                    Some(Type::Str) => "println_str",
                                    Some(Type::F64) | Some(Type::F32) => "println_f64",
                                    _ => "println_i64",
                                };
                                then_stmts.push(MirStmt::VoidCall {
                                    func: pf(&hit_ty).to_string(),
                                    args: vec![hit_id],
                                });
                                else_stmts.push(MirStmt::VoidCall {
                                    func: pf(&miss_ty).to_string(),
                                    args: vec![miss_id],
                                });
                                self.stmts.push(MirStmt::If {
                                    cond: cond_id,
                                    then: then_stmts,
                                    else_: else_stmts,
                                    dest: None,
                                });
                                let unit = self.next_id();
                                self.exprs.insert(unit, MirExpr::IntLit(0));
                                self.type_map.insert(unit, Type::Tuple(vec![]));
                                return;
                            }
                        }
                    }
                }
                // Batch 659: `print(d[k])` on a map-typed variable
                // whose value type refined to Str (651/652 混合值型
                // map 的毒化面) — the generic path renders EVERY read
                // through that single dispatch, so an int word behind
                // the str refinement went to println_str and strlen'd
                // the raw word (measured rc=139 with no output; str
                // reads printed fine, which is why the crash looked
                // shape-dependent). The 535 per-key tag side table
                // knows the real kind: render via zeta_map_get_render
                // (653 infra, unwired) and print the STRING. Three
                // guards keep the 653 lesson honored: Var receiver
                // only (no double-eval, .get calls untouched), render
                // only in this consumption position (bare `x = d[k]`
                // keeps the generic path), and only when the static
                // value type is Str (pure int/container maps keep
                // their working dispatches).
                if positional.len() == 1 && sep_expr.is_none() && end_expr.is_none() {
                    if let AstNode::Subscript { base, index } = positional[0] {
                        let base_map_str = if let AstNode::Var(vname) = &**base {
                            self.name_to_id
                                .get(vname.as_str())
                                .map_or(false, |&sid| {
                                    self.type_map.get(&sid).map_or(false, |ty| {
                                        // 批次 901（轴 F）：收敛到 is_map 唯一判定。
                                        ty.is_map()
                                            && matches!(
                                                ty,
                                                Type::Named(_, params)
                                                    if matches!(params.last(), Some(Type::Str))
                                            )
                                    })
                                })
                        } else {
                            false
                        };
                        if base_map_str {
                            let recv_id = self.lower_expr(base);
                            let k_id = self.lower_expr(index);
                            let r_id = self.emit_call("zeta_map_get_render", vec![recv_id, k_id], Type::Str);
                            self.stmts.push(MirStmt::VoidCall {
                                func: "println_str".to_string(),
                                args: vec![r_id],
                            });
                            let unit = self.next_id();
                            self.exprs.insert(unit, MirExpr::IntLit(0));
                            self.type_map.insert(unit, Type::Tuple(vec![]));
                            return;
                        }
                    }
                }
                let mut arg_ids = vec![];
                for a in &positional {
                    // Batch 655 (mainline): `print(3 and None)` /
                    // `print(None or 5)` — AND/OR whose statically-selected
                    // operand is a None literal renders "None" (value repr
                    // stays 0). Must be checked BEFORE the batch-635 handler
                    // below, which intercepts all AND/OR BinaryOps.
                    if self.and_or_select_is_none(a) {
                        let nid = self.next_id();
                        self.exprs
                            .insert(nid, MirExpr::StringLit("None".to_string()));
                        self.type_map.insert(nid, Type::Str);
                        arg_ids.push(nid);
                        continue;
                    }
                    // Batch 659 (cleanup): per-arg map-subscript render — a
                    // mixed map's int word must not reach the println
                    // dispatch through the str refinement
                    // (`print(d["n"], d["s"])` segvs the int side).
                    if let AstNode::Subscript { base, index } = a {
                        if let Some(r_id) = self.mapsub_render_str(base, index) {
                            arg_ids.push(r_id);
                            continue;
                        }
                    }
                    // Batch 635: `print(L and R)` with an int left and a
                    // Bool right — the union cannot share one println
                    // dispatch (the falsy branch prints an int, the
                    // truthy branch prints True/False; a single I64 dest
                    // rendered the Bool side as 1). Split at statement
                    // level like the batch-598 `.get` shape: each branch
                    // prints its own operand with its own dispatch.
                    // Guards: sole positional arg, no sep/end, right
                    // operand Bool-typed, left I64/PyDynamic-typed
                    // (Str/F64 lefts already adopt their concrete type).
                    if let AstNode::BinaryOp { op, left, right } = &**a {
                        if (op == "&&" || op == "||")
                            && positional.len() == 1
                            && sep_expr.is_none()
                            && end_expr.is_none()
                        {
                            let saved3 = std::mem::take(&mut self.stmts);
                            let left_id = self.lower_expr(left);
                            let lt = self.type_map.get(&left_id).cloned();
                            let right_id = self.lower_expr(right);
                            let rt = self.type_map.get(&right_id).cloned();
                            let mut right_stmts = std::mem::take(&mut self.stmts);
                            self.stmts = saved3;
                            /* Batch 648: the RIGHT operand may also be
                               a NoneLit (`3 and None` in CPython prints
                               None) — the None face renders it. */
                            let right_none = matches!(
                                &**right,
                                AstNode::NoneLit
                            );
                            let split_worthy = (matches!(rt, Some(Type::Bool))
                                || right_none)
                                && lt.as_ref().map_or(true, |t| t.is_untyped());
                            if split_worthy {
                                let truth_id =
                                    if matches!(lt, Some(Type::Bool)) {
                                        left_id
                                    } else {
                                        let tid = self.emit_call("zeta_dyn_truth", vec![left_id], Type::I64);
                                        tid
                                    };
                                let cond_id = self.next_id();
                                let zero_id = self.next_id_with_lit(0);
                                self.exprs.insert(
                                    cond_id,
                                    MirExpr::BinaryOp {
                                        op: "!=".to_string(),
                                        left: truth_id,
                                        right: zero_id,
                                    },
                                );
                                self.type_map.insert(cond_id, Type::Bool);
                                // `&&`: truthy left selects the RIGHT
                                // operand (Bool — True/False via
                                // to_string_bool + println_str); falsy
                                // prints the LEFT (int). `||` mirrors.
                                if right_none {
                                    /* None side: print the literal "None"
                                       (a NoneLit operand lowers to I64 0,
                                       which would render as an integer). */
                                    let nid = self.next_id();
                                    self.exprs
                                        .insert(nid, MirExpr::StringLit("None".to_string()));
                                    self.type_map.insert(nid, Type::Str);
                                    right_stmts.push(MirStmt::VoidCall {
                                        func: "println_str".to_string(),
                                        args: vec![nid],
                                    });
                                } else {
                                    let cid = self.next_id();
                                    self.exprs.insert(cid, MirExpr::Var(cid));
                                    self.type_map.insert(cid, Type::Str);
                                    right_stmts.push(MirStmt::Call {
                                        func: "to_string_bool".to_string(),
                                        args: vec![right_id],
                                        dest: cid,
                                        type_args: vec![],
                                    });
                                    right_stmts.push(MirStmt::VoidCall {
                                        func: "println_str".to_string(),
                                        args: vec![cid],
                                    });
                                }
                                let left_print = MirStmt::VoidCall {
                                    func: "println_i64".to_string(),
                                    args: vec![left_id],
                                };
                                let (then_b, else_b) = if op == "&&" {
                                    (right_stmts, vec![left_print])
                                } else {
                                    (vec![left_print], right_stmts)
                                };
                                self.stmts.push(MirStmt::If {
                                    cond: cond_id,
                                    then: then_b,
                                    else_: else_b,
                                    dest: None,
                                });
                                continue;
                            }
                            // Not split-worthy: discard the speculative
                            // operand buffers (they live in right_stmts)
                            // and lower the node plainly.
                            arg_ids.push(self.lower_expr(a));
                            continue;
                        }
                    }
                    // Batch 647 (cleanup): a BigInt operand renders via
                    // zeta_big_to_string — pushed as a Str arg id like
                    // the NoneLit face.
                    if matches!(a, AstNode::BigIntLit(_)) {
                        let lhs0 = self.lower_expr(a);
                        let sid = self.emit_call("zeta_big_to_string", vec![lhs0], Type::Str);
                        arg_ids.push(sid);
                        continue;
                    }
                    if matches!(a, AstNode::NoneLit) {
                        // Batch 624: `print(None)` literal face renders
                        // "None" (value representation stays 0).
                        let nid = self.next_id();
                        self.exprs
                            .insert(nid, MirExpr::StringLit("None".to_string()));
                        self.type_map.insert(nid, Type::Str);
                        arg_ids.push(nid);
                    } else if let AstNode::Var(x) = a {
                        /* Batch 654: a NoneVar name (last top-level
                           assignment was `x = None`) renders "None". */
                        if self.none_vars_gen.contains(x) {
                            let nid = self.next_id();
                            self.exprs
                                .insert(nid, MirExpr::StringLit("None".to_string()));
                            self.type_map.insert(nid, Type::Str);
                            arg_ids.push(nid);
                        } else {
                            // 批次 806（#113 携带读面）：Var 槽带 NoneValue
                            // 型（`v = f()`，f 纯 None 返回）——按型渲染，
                            // 与非 Var 实参的 804 臂同款。
                            let aid = self.lower_expr(a);
                            // 批次 903（轴 F kind 3）：收敛到 is_none_value 唯一判定。
                            if self
                                .type_map
                                .get(&aid)
                                .map_or(false, Type::is_none_value)
                            {
                                let nid = self.next_id();
                                self.exprs
                                    .insert(nid, MirExpr::StringLit("None".to_string()));
                                self.type_map.insert(nid, Type::Str);
                                arg_ids.push(nid);
                            } else {
                                arg_ids.push(aid);
                            }
                        }
                    } else {
                        let aid = self.lower_expr(a);
                        // 批次 804（#113 fromkeys 格）：值型 NoneValue 的
                        // 读——按型渲染 "None"（654 NoneVar 按名渲染的按型
                        // 同款；fromkeys None 结果型 map[K, NoneValue] 让
                        // d["k"] 读带上该型）。
                        // 批次 903（轴 F kind 3）：同上收敛。
                        if self
                            .type_map
                            .get(&aid)
                            .map_or(false, Type::is_none_value)
                        {
                            let nid = self.next_id();
                            self.exprs
                                .insert(nid, MirExpr::StringLit("None".to_string()));
                            self.type_map.insert(nid, Type::Str);
                            arg_ids.push(nid);
                        } else {
                            arg_ids.push(aid);
                        }
                    }
                }
                let n = arg_ids.len();
                // Separator (default one space).
                let space_id = match &sep_expr {
                    Some(e) => self.lower_expr(e),
                    None => {
                        let s = self.next_id();
                        self.exprs.insert(s, MirExpr::StringLit(" ".to_string()));
                        self.type_map.insert(s, Type::Str);
                        s
                    }
                };
                // With an explicit `end`, no argument may bake in the
                // newline (the println_* family appends one).
                let has_end = end_expr.is_some();
                let end_id = end_expr.as_ref().map(|e| self.lower_expr(e));
                for (i, arg_id) in arg_ids.iter().enumerate() {
                    if i > 0 {
                        self.stmts.push(MirStmt::VoidCall {
                            func: "print_str".to_string(),
                            args: vec![space_id],
                        });
                    }
                    let is_last = !has_end && i + 1 == n;
                    // 批次 808（混型 dict None 值格）：PyDynamic 实参带格
                    // 标签 ⇒ 运行期按格渲染链（767 len 链同构、自底向上
                    // else-if）。8=None、4=文本、5=i64、6=f64 位、7=bool；
                    // 0/未知落 i64（静态缺省同形）。非混型字典无标签读、
                    // 无本链，零新增面。
                    // 批次 808/827：PyDynamic 实参带格标签 ⇒ 按格渲染
                    // 链（抽为 emit_tagged_print，路由 8/4/5/6/7，
                    // 0/未知落 i64）。非混型字典无标签读、零新增面。
                    if self.type_map.get(arg_id).map_or(false, Type::is_dynamic) {
                        if let Some(tag_slot) = self.slot_tags.get(arg_id).cloned() {
                            if self.emit_tagged_print(*arg_id, tag_slot, is_last) {
                                continue;
                            }
                        }
                    }
                    // A Json value prints by its tag (scalars bare,
                    // containers as JSON text) — converting to a string
                    // first keeps it on the existing print path.
                    // A list prints by its (static) element type, Python
                    // repr differs only in spacing/quoting.
                    // 批次 557: a list of PAIRS prints as CPython tuple
                    // text `[(1, 'x'), (2, 'y')]` — per-position string
                    // flags from the static pair types (py_print_pairs).
                    // Without this the arg fell into the int-vec dump and
                    // printed raw element words.
                    // a BARE pair value (`print(e[0])`) — same spelling
                    // for a single pair.
                    // Batch 647: a BigInt-typed value (literal or the
                    // result of big arithmetic) renders its decimal value.
                    // 批次 893（#279 方案②）：PyDynamic 槽的 print 按运行期
                    // 形状分派（zeta_dyn_to_string：文本原样、map/vec 结构化、
                    // 整数十进制）——未知型均值（zeta_mean_to_string）等"值可能
                    // 是文本也可能是句柄"的槽在此收敛。原先落 println_i64，把
                    // 文本句柄按整数打（t813 实拍）。
                    if self.type_map.get(arg_id).map_or(false, Type::is_dynamic) {
                        let sid = self.emit_call("zeta_dyn_to_string", vec![*arg_id], Type::Str);
                        let f = if is_last { "println_str" } else { "print_str" };
                        self.stmts.push(MirStmt::VoidCall {
                            func: f.to_string(),
                            args: vec![sid],
                        });
                        continue;
                    }
                    if matches!(
                        self.type_map.get(arg_id),
                        Some(Type::Named(n, _)) if n == "BigInt"
                    ) {
                        let sid = self.emit_call("zeta_big_to_string", vec![*arg_id], Type::Str);
                        let f = if is_last { "println_str" } else { "print_str" };
                        self.stmts.push(MirStmt::VoidCall {
                            func: f.to_string(),
                            args: vec![sid],
                        });
                        continue;
                    }
                    if let Some(Type::Tuple(ts)) = self.type_map.get(arg_id).cloned() {
                        if ts.len() == 2 {
                            let kid = self.next_id_with_lit(matches!(ts[0], Type::Str) as i64);
                            let vid = self.next_id_with_lit(matches!(ts[1], Type::Str) as i64);
                            let sid = self.emit_call("py_print_pair", vec![*arg_id, kid, vid], Type::Str);
                            let f = if is_last { "println_str" } else { "print_str" };
                            self.stmts.push(MirStmt::VoidCall {
                                func: f.to_string(),
                                args: vec![sid],
                            });
                            continue;
                        }
                    }
                    if let Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) =
                        self.type_map.get(arg_id)
                    {
                        if let Type::Tuple(ts) = &**e {
                            if ts.len() == 2 {
                                let k_str =
                                    matches!(ts[0], Type::Str) as i64;
                                let v_str =
                                    matches!(ts[1], Type::Str) as i64;
                                let kid = self.next_id_with_lit(k_str);
                                let vid = self.next_id_with_lit(v_str);
                                let sid = self.emit_call("py_print_pairs", vec![*arg_id, kid, vid], Type::Str);
                                let f = if is_last {
                                    "println_str"
                                } else {
                                    "print_str"
                                };
                                self.stmts.push(MirStmt::VoidCall {
                                    func: f.to_string(),
                                    args: vec![sid],
                                });
                                continue;
                            }
                        }
                    }
                    if let Some(tags) = match self.type_map.get(arg_id) {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            Some(Self::elem_tag_string(e))
                        }
                        _ => None,
                    } {
                        // Batch 633: nested lists carry a recursive tag
                        // string ("4,0" = list of int-lists) and route to
                        // the recursive runtime; flat forms keep the typed
                        // dumps call unchanged.
                        let (fname, tags_expr): (&str, MirExpr) = if tags.len() == 1 {
                            (
                                "py_json_dumps_vec_typed",
                                MirExpr::IntLit(tags.as_bytes()[0].wrapping_sub(b'0') as i64),
                            )
                        } else {
                            (
                                "py_json_dumps_vec_nested",
                                MirExpr::StringLit(tags.clone()),
                            )
                        };
                        let sid = self.next_id();
                        let tid = self.next_id();
                        self.exprs.insert(tid, tags_expr);
                        if fname.ends_with("_typed") {
                            self.type_map.insert(tid, Type::I64);
                        } else {
                            self.type_map.insert(tid, Type::Str);
                        }
                        self.stmts.push(MirStmt::Call {
                            func: fname.to_string(),
                            args: vec![*arg_id, tid],
                            dest: sid,
                            type_args: vec![],
                        });
                        self.exprs.insert(sid, MirExpr::Var(sid));
                        self.type_map.insert(sid, Type::Str);
                        let f = if is_last { "println_str" } else { "print_str" };
                        self.stmts.push(MirStmt::VoidCall {
                            func: f.to_string(),
                            args: vec![sid],
                        });
                        continue;
                    }
                    // A dict prints by its recorded value tags (Python's
                    // repr differs only in quote style).
                    if matches!(
                        self.type_map.get(arg_id),
                        Some(Type::Named(name, _)) if name == "map"
                    ) {
                        let sid = self.emit_call("py_json_dumps_map", vec![*arg_id], Type::Str);
                        let f = if is_last { "println_str" } else { "print_str" };
                        self.stmts.push(MirStmt::VoidCall {
                            func: f.to_string(),
                            args: vec![sid],
                        });
                        continue;
                    }
                    let printed_id = if matches!(
                        self.type_map.get(arg_id),
                        Some(Type::Named(name, _)) if name == "PyJson"
                    ) {
                        let sid = self.emit_call("py_json_repr", vec![*arg_id], Type::Str);
                        sid
                    } else if matches!(
                        self.type_map.get(arg_id),
                        Some(Type::Named(name, _)) if name == "PyMatch"
                    ) {
                        // Print the matched text rather than the raw handle.
                        let gid = self.next_id();
                        self.exprs.insert(gid, MirExpr::IntLit(0));
                        self.type_map.insert(gid, Type::I64);
                        let sid = self.emit_call("py_re_group", vec![*arg_id, gid], Type::Str);
                        sid
                    } else if self.type_map.get(arg_id).map_or(false, Type::is_dynamic) {
                        // Batch 653 (#117): dynamic values have no runtime type tag,
                        // so the print dispatch can't tell str from int from map.
                        // Convert to string first using GC-geometry probes.
                        let sid = self.emit_call("zeta_dyn_to_string", vec![*arg_id], Type::Str);
                        sid
                    } else {
                        *arg_id
                    };
                    let func = match self.type_map.get(&printed_id) {
                        Some(Type::Str) => {
                            if is_last { "println_str" } else { "print_str" }
                        }
                        Some(Type::Named(n, _)) if n == "PyPath" => {
                            if is_last { "println_str" } else { "print_str" }
                        }
                        Some(Type::F64) | Some(Type::F32) => {
                            if is_last { "println_f64" } else { "print_f64" }
                        }
                        // Batch 291: bool must print Python-style (True/False),
                        // not as the i64 fallback (1/0). print_bool exists in the
                        // runtime since the beginning but was never selected —
                        // `print(1 == 1)` printed `1`. No println_bool: the newline
                        // comes from the `print_str("\n")` emitted below for is_last.
                        Some(Type::Bool) => "print_bool",
                        _ => {
                            if is_last { "println_i64" } else { "print_i64" }
                        }
                    };
                    self.stmts.push(MirStmt::VoidCall {
                        func: func.to_string(),
                        args: vec![printed_id],
                    });
                    if matches!(self.type_map.get(&printed_id), Some(Type::Bool)) && is_last {
                        let nl = self.next_id();
                        self.exprs.insert(nl, MirExpr::StringLit("\n".to_string()));
                        self.type_map.insert(nl, Type::Str);
                        self.stmts.push(MirStmt::VoidCall {
                            func: "print_str".to_string(),
                            args: vec![nl],
                        });
                    }
                }
                // `print(..., end=X)` — emit the terminator ourselves.
                if let Some(e) = end_id {
                    self.stmts.push(MirStmt::VoidCall {
                        func: "print_str".to_string(),
                        args: vec![e],
                    });
                }
                let unit_id = self.next_id();
                self.exprs.insert(unit_id, MirExpr::IntLit(0));
                self.type_map.insert(unit_id, Type::Tuple(vec![]));
                self.type_map.insert(dest, Type::Tuple(vec![]));
                return;

        let _ = dest;
    }
}
