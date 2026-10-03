//! 批次 847：BinaryOp 全族发射体（算术/比较/逻辑/字符串/容器运算）。
//! 臂体逐字迁入；签名直接收 op/left/right/id——零文本替换，行为等价。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt, Type};

impl MirGen {
    pub(super) fn lower_binary_op(
        &mut self,
        op: &str,
        left: &AstNode,
        right: &AstNode,
        id: u32,
    ) -> u32 {
                // `x in ("sh", "sz")` — a TUPLE literal on the right. Tuples are
                // `StackArray`s with a `[len|elems]` layout, which the membership
                // path reads as a dynamic array ⇒ every membership test against a
                // tuple answered 0 (`"sz" in ("sh","sz")` → 0, while the same value
                // in a LIST worked). `code_conv.normalize_to_jq` is built on such
                // tuples, so `sh.513120` never normalized and every wufu code
                // failed to resolve to a cache file.
                if matches!(op.as_str(), "in" | "not in") {
                    if let AstNode::Tuple(items) = &**right {
                        let rewritten = AstNode::BinaryOp {
                            op: op.clone(),
                            left: left.clone(),
                            right: Box::new(AstNode::ArrayLit(items.clone())),
                        };
                        return self.lower_expr(&rewritten);
                    }
                }
                // PY-A: `and`/`or` must SHORT-CIRCUIT. The old path lowered
                // BOTH sides eagerly and let codegen `select` between the two
                // values — so `df is not None and len(df) > 0` ran `len(df)`
                // even when df was the None handle (crash measured in
                // `MarketDataFetcher::fetch_stocks`, drvE/drvF SIGBUS). The
                // value-select typing below is preserved: dest takes the whole
                // result via a nested If whose branches splice the deferred
                // lowering (same stmt-buffer swap the if-expression uses).
                if matches!(op.as_str(), "&&" | "||") {
                    let left_id = self.lower_expr(left);
                    let dest = self.next_id();
                    let zero_id = self.next_id_with_lit(0);
                    let cond_id = self.next_id();
                    // BATCH-297: WHICH side to take is a truthiness question,
                    // and `!= 0` is not the same answer for a container: an
                    // empty list/dict/string handle is non-zero, so
                    // `ranked = getattr(g, "ranked_etfs_result", []) or []`
                    // kept the empty side and `pool or fixed_pool` never fell
                    // through. `zeta_dyn_truth` asks the value (GC geometry →
                    // length, anything else → `!= 0`), so an int answers the
                    // same as before. Floats keep their own rule (bits != 0).
                    let left_ty = self.type_map.get(&left_id).cloned();
                    // A USER-CLASS instance is always truthy in Python (our
                    // dataclasses define no `__bool__`/`__len__`) and it lives in
                    // an i64 slot as a heap pointer, so `!= 0` is the whole
                    // question. `zeta_dyn_truth` instead probes the object's first
                    // word as a length: `self._cost = cost or CostModel()` read
                    // `slippage` (0.0 bits) as "empty", silently swapped in a
                    // default model, and every `fee()` became 0 — the ledger's
                    // cash barely moved.
                    let is_object = matches!(
                        &left_ty,
                        Some(Type::Named(n, _))
                            if !matches!(
                                n.as_str(),
                                "map" | "dict" | "set" | "frozenset" | "PyJson" | "str"
                            )
                    );
                    let truth_id =
                        if matches!(left_ty, Some(Type::F64) | Some(Type::Bool)) || is_object {
                            left_id
                        } else {
                        // A JSON cell needs its own reader (tag + payload), the
                        // geometry probes cannot see through the tag word.
                        let func = if matches!(&left_ty, Some(Type::Named(n, _)) if n == "PyJson") {
                            "py_json_truth"
                        } else {
                            "zeta_dyn_truth"
                        };
                        let tid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: func.to_string(),
                            args: vec![left_id],
                            dest: tid,
                            type_args: vec![],
                        });
                        self.exprs.insert(tid, MirExpr::Var(tid));
                        self.type_map.insert(tid, Type::I64);
                        tid
                    };
                    self.exprs.insert(
                        cond_id,
                        MirExpr::BinaryOp {
                            op: "!=".to_string(),
                            left: truth_id,
                            right: zero_id,
                        },
                    );
                    self.type_map.insert(cond_id, Type::Bool);

                    let saved_stmts = std::mem::take(&mut self.stmts);
                    let right_id = self.lower_expr(right);
                    let mut right_stmts = std::mem::take(&mut self.stmts);
                    self.stmts = saved_stmts;

                    let left_then = vec![MirStmt::Assign { lhs: dest, rhs: left_id }];
                    let right_then_assign = {
                        right_stmts.push(MirStmt::Assign { lhs: dest, rhs: right_id });
                        right_stmts
                    };
                    let (then_b, else_b) = if op == "&&" {
                        // a and b: falsy a selects a; truthy a evaluates b.
                        (right_then_assign, left_then)
                    } else {
                        (left_then, right_then_assign)
                    };
                    // Value semantics typing (batch 152, kept through the
                    // short-circuit rewrite): only adopt a CONCRETE operand
                    // type — an untyped param (PyDynamic/None/I64/Bool) must
                    // not poison the dest, or `cfg = m or {}` would type cfg
                    // PyDynamic and `cfg.get` would dispatch to `_get`.
                    let lt = self.type_map.get(&left_id).cloned();
                    let rt = self.type_map.get(&right_id).cloned();
                    // t512: the chain RETURNS one of its operands, so when the
                    // left side is a compile-time constant the selection is
                    // decidable and the result takes the SELECTED side's type
                    // (`1 and 2 == 3` is False — a bool; typing it I64 made
                    // print render 0). Unknown truthiness keeps the batch-152
                    // rule: a wrong Bool claim would misrender an int result.
                    let lhs_truth: Option<bool> = match self.exprs.get(&left_id) {
                        Some(MirExpr::IntLit(v)) => Some(*v != 0),
                        Some(MirExpr::FloatLit(v)) => Some(*v != 0.0),
                        Some(MirExpr::StringLit(s)) => Some(!s.is_empty()),
                        _ => None,
                    };
                    let selected_ty = match (op.as_str(), lhs_truth) {
                        ("&&", Some(false)) | ("||", Some(true)) => lt.clone(),
                        ("&&", Some(true)) | ("||", Some(false)) => rt.clone(),
                        _ => None,
                    };
                    let dest_ty = if let Some(t) = selected_ty {
                        t
                    } else {
                        let concrete = |t: &Option<Type>| match t {
                            Some(Type::I64) | Some(Type::PyDynamic) | Some(Type::Bool) | None => {
                                None
                            }
                            other => other.clone(),
                        };
                        match (concrete(&lt), concrete(&rt)) {
                            (Some(a), Some(b)) if a == b => a,
                            (Some(a), _) => a,
                            (_, Some(b)) => b,
                            _ => match (lt, rt) {
                                (Some(a), Some(b)) if a == b => a,
                                _ => Type::I64,
                            },
                        }
                    };
                    self.type_map.insert(dest, dest_ty);
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.stmts.push(MirStmt::If {
                        cond: cond_id,
                        then: then_b,
                        else_: else_b,
                        dest: Some(dest),
                    });
                    return dest;
                }
                let left_id = self.lower_expr(left);
                let right_id = self.lower_expr(right);
                let dest = self.next_id();
                /* Batch 647: BigInt arithmetic — either operand carrying the
                   Named("BigInt") type routes the whole op through the
                   zeta_big family (the other side is boxed from I64). The
                   result stays a BigInt handle; comparisons yield Bool. */
                let big_arm = matches!(
                    self.type_map.get(&left_id),
                    Some(Type::Named(n, _)) if n == "BigInt"
                ) || matches!(
                    self.type_map.get(&right_id),
                    Some(Type::Named(n, _)) if n == "BigInt"
                );
                if big_arm {
                    if matches!(op.as_str(), "%" | "floordiv") {
                        /* Batch 658: `%`/`//` on handles route through the
                           runtime family with Python floor semantics; the
                           result stays a BigInt handle (big // 3 is far
                           beyond i64). A zero divisor raises inside
                           zeta_big_mod/zeta_big_floordiv (the 554 guard
                           below never runs on this path). */
                        let box_side = |side: u32, g: &mut Self| -> u32 {
                            match g.type_map.get(&side) {
                                Some(Type::Named(n, _)) if n == "BigInt" => side,
                                _ => {
                                    let b = g.next_id();
                                    g.stmts.push(MirStmt::Call {
                                        func: "zeta_big_from_i64".to_string(),
                                        args: vec![side],
                                        dest: b,
                                        type_args: vec![],
                                    });
                                    g.exprs.insert(b, MirExpr::Var(b));
                                    g.type_map.insert(
                                        b,
                                        Type::Named("BigInt".to_string(), vec![]),
                                    );
                                    b
                                }
                            }
                        };
                        let bl = box_side(left_id, self);
                        let br = box_side(right_id, self);
                        self.stmts.push(MirStmt::Call {
                            func: if op == "%" {
                                "zeta_big_mod".to_string()
                            } else {
                                "zeta_big_floordiv".to_string()
                            },
                            args: vec![bl, br],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map
                            .insert(dest, Type::Named("BigInt".to_string(), vec![]));
                        return dest;
                    }
                    if op == "/" {
                        /* Batch 655: `/` is TRUEDIV — Python answers with
                           a float (2**100 / 3 = 4.2e+29); the old abstain
                           left dest unlowered and every such division
                           printed 0. Both sides convert through the
                           128-bit decimal string (strtod = correctly
                           rounded); a zero divisor still raises like the
                           batch-554 guard. */
                        /* Only the BigInt side converts (boxing a float
                           slot through from_i64 would bit-reinterpret it);
                           int/float sides flow to the codegen float-div
                           path, which sitofps them natively. The zero
                           sentinel matches the divisor's converted kind —
                           an unconverted int divisor compares against
                           IntLit(0) like the batch-554 guard. */
                        let mut to_f = |side: u32, g: &mut Self| -> u32 {
                            if matches!(
                                g.type_map.get(&side),
                                Some(Type::Named(n, _)) if n == "BigInt"
                            ) {
                                let f = g.next_id();
                                g.stmts.push(MirStmt::Call {
                                    func: "zeta_big_to_f64".to_string(),
                                    args: vec![side],
                                    dest: f,
                                    type_args: vec![],
                                });
                                g.exprs.insert(f, MirExpr::Var(f));
                                g.type_map.insert(f, Type::F64);
                                f
                            } else {
                                side
                            }
                        };
                        let fl = to_f(left_id, self);
                        let fr = to_f(right_id, self);
                        let div_is_float = matches!(
                            self.type_map.get(&fr),
                            Some(Type::F64) | Some(Type::F32)
                        );
                        let zero = self.next_id();
                        if div_is_float {
                            self.exprs.insert(zero, MirExpr::FloatLit(0.0));
                            self.type_map.insert(zero, Type::F64);
                        } else {
                            self.exprs.insert(zero, MirExpr::IntLit(0));
                            self.type_map.insert(zero, Type::I64);
                        }
                        let cond_id = self.next_id();
                        self.exprs.insert(
                            cond_id,
                            MirExpr::BinaryOp {
                                op: "==".to_string(),
                                left: fr,
                                right: zero,
                            },
                        );
                        self.type_map.insert(cond_id, Type::Bool);
                        let code_id = self.next_id_with_lit(2);
                        self.stmts.push(MirStmt::If {
                            cond: cond_id,
                            then: vec![MirStmt::VoidCall {
                                func: "zeta_raise".to_string(),
                                args: vec![code_id],
                            }],
                            else_: vec![],
                            dest: None,
                        });
                        self.exprs.insert(
                            dest,
                            MirExpr::BinaryOp {
                                op: "/".to_string(),
                                left: fl,
                                right: fr,
                            },
                        );
                        self.type_map.insert(dest, Type::F64);
                        return dest;
                    }
                    let box_side = |side: u32, g: &mut Self| -> u32 {
                        match g.type_map.get(&side) {
                            Some(Type::Named(n, _)) if n == "BigInt" => side,
                            _ => {
                                let b = g.next_id();
                                g.stmts.push(MirStmt::Call {
                                    func: "zeta_big_from_i64".to_string(),
                                    args: vec![side],
                                    dest: b,
                                    type_args: vec![],
                                });
                                g.exprs.insert(b, MirExpr::Var(b));
                                g.type_map.insert(
                                    b,
                                    Type::Named("BigInt".to_string(), vec![]),
                                );
                                b
                            }
                        }
                    };
                    let bl = box_side(left_id, self);
                    let br = box_side(right_id, self);
                    let is_cmp = matches!(
                        op.as_str(),
                        "<" | ">" | "<=" | ">=" | "==" | "!="
                    );
                    if is_cmp {
                        let c = self.emit_call("zeta_big_cmp", vec![bl, br], Type::I64);
                        let zero = self.next_id_with_lit(0);
                        let ord = match op.as_str() {
                            "<" => "<",
                            ">" => ">",
                            "<=" => "<=",
                            ">=" => ">=",
                            "==" => "==",
                            _ => "!=",
                        };
                        self.exprs.insert(
                            dest,
                            MirExpr::BinaryOp {
                                op: ord.to_string(),
                                left: c,
                                right: zero,
                            },
                        );
                        self.type_map.insert(dest, Type::Bool);
                        return dest;
                    }
                    let sym = match op.as_str() {
                        "+" => "zeta_big_add",
                        "-" => "zeta_big_sub",
                        "*" => "zeta_big_mul",
                        "<<" => "zeta_big_shl",
                        ">>" => "zeta_big_shr",
                        "|" => "zeta_big_or",
                        "&" => "zeta_big_and",
                        "^" => "zeta_big_xor",
                        _ => {
                            self.type_map.insert(dest, Type::I64);
                            return dest;
                        }
                    };
                    self.stmts.push(MirStmt::Call {
                        func: sym.to_string(),
                        args: vec![bl, br],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::Named("BigInt".to_string(), vec![]));
                    return dest;
                }
                /* Batch 650: SHIFT ARITHMETIC IS ALWAYS BIG. Python's `<<`
                   is the grow-an-integer operator — the result is exact for
                   every shift count, so the raw i64 shl (poison at n ≥ 64,
                   silent wrap below) is never the right lowering. Both
                   operands box through the zeta_big family and the dest is
                   a BigInt handle: variable shift counts included (the
                   batch-649 literal-only gate is subsumed; a loop-internal
                   `y = 3 << 64` now prints the true value every iteration).
                   `>>` is arithmetic in zeta_big_shr — 0 for a ≥ 0, -1 for
                   a < 0 at n ≥ 64, exact below. Float/string receivers keep
                   their previous (garbage-in) behavior out of scope. */
                if matches!(op.as_str(), "<<" | ">>") {
                    let bl = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_big_from_i64".to_string(),
                        args: vec![left_id],
                        dest: bl,
                        type_args: vec![],
                    });
                    self.exprs.insert(bl, MirExpr::Var(bl));
                    self.type_map
                        .insert(bl, Type::Named("BigInt".to_string(), vec![]));
                    self.stmts.push(MirStmt::Call {
                        func: if op == "<<" { "zeta_big_shl" } else { "zeta_big_shr" }.to_string(),
                        args: vec![bl, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::Named("BigInt".to_string(), vec![]));
                    return dest;
                }
                /* Batch 645: a LITERAL zero divisor on `/` makes the 554
                   raise unconditional — the division never executes, so its
                   result slot types I64 (the except branch's integer write
                   then dominates the variable's render;
                   except_division_zero: y printed -1.0 instead of -1). */
                let dead_div = op == "/"
                    && matches!(self.exprs.get(&right_id), Some(MirExpr::IntLit(0)));

                // ZeroDivisionError (except_division_zero, batch 554): CPython
                // raises on every `/` whose divisor is zero; the inline div
                // silently answered inf (float) or UB (sdiv). Materialize the
                // divisor (the check would otherwise evaluate it a second
                // time — codegen re-evaluates at each use), raise through the
                // zeta_raise trampoline (bare `except:` catches it; uncaught
                // → loud exit), then divide.
                if op == "/" || op == "%" || op == "floordiv" {
                    // '%' and 'floordiv' raise on a zero divisor too (CPython:
                    // "integer division or modulo by zero") — and srem by zero
                    // is LLVM UB, same as sdiv. Integer ops only: the guard's
                    // zero literal stays an IntLit.
                    let rslot = match self.exprs.get(&right_id) {
                        Some(MirExpr::Var(_)) => right_id,
                        _ => {
                            let f = self.next_id();
                            self.exprs.insert(f, MirExpr::Var(f));
                            let ty =
                                self.type_map.get(&right_id).cloned().unwrap_or_else(Type::slot_fallback);
                            self.type_map.insert(f, ty);
                            self.stmts.push(MirStmt::Assign {
                                lhs: f,
                                rhs: right_id,
                            });
                            f
                        }
                    };
                    let float_div = op == "/"
                        && (matches!(
                            self.type_map.get(&rslot),
                            Some(Type::F32) | Some(Type::F64)
                        ) || matches!(
                            self.type_map.get(&left_id),
                            Some(Type::F32) | Some(Type::F64)
                        ) || matches!(self.exprs.get(&rslot), Some(MirExpr::FloatLit(_))));
                    let zero_id = if float_div {
                        let z = self.next_id();
                        self.exprs.insert(z, MirExpr::FloatLit(0.0));
                        self.type_map.insert(z, Type::F64);
                        z
                    } else {
                        self.next_id_with_lit(0)
                    };
                    let cond_id = self.next_id();
                    self.exprs.insert(
                        cond_id,
                        MirExpr::BinaryOp {
                            op: "==".to_string(),
                            left: rslot,
                            right: zero_id,
                        },
                    );
                    self.type_map.insert(cond_id, Type::Bool);
                    let code_id = self.next_id_with_lit(2);
                    self.stmts.push(MirStmt::If {
                        cond: cond_id,
                        then: vec![MirStmt::VoidCall {
                            func: "zeta_raise".to_string(),
                            args: vec![code_id],
                        }],
                        else_: vec![],
                        dest: None,
                    });
                }

                // PY-A: operator dispatch on library handles (datetime
                // date/timedelta arithmetic and comparisons). Without it the
                // operands were treated as plain integers, silently producing
                // garbage for `d1 - d2` / `d < today`.
                let tag_name = |t: Option<Type>| match t {
                    Some(Type::Named(n, _)) => Some(n),
                    Some(Type::Str) => Some("str".to_string()),
                    _ => None,
                };
                if let (Some(lt), Some(rt)) = (
                    tag_name(self.type_map.get(&left_id).cloned()),
                    tag_name(self.type_map.get(&right_id).cloned()),
                ) {
                    if let Some((sym, kind)) = crate::middle::pylib::handle_op(op, &lt, &rt) {
                        // The codegen's call-arg path loads an operand from its
                        // LOCAL slot; a FieldAccess / call result has no alloca, so
                        // `self.cache_dir / "x"` (or `_PROJECT_ROOT / "data"` where
                        // the global came through a field) loaded NULL and
                        // `py_os_path_join` dereferenced it — measured SEGV at
                        // `MarketDataFetcher.__init__+68`. Materialize both
                        // operands into fresh slots first (same rule the map
                        // subscript path already follows).
                        let left_id = self.materialize_for_call(left_id);
                        let right_id = self.materialize_for_call(right_id);
                        self.stmts.push(MirStmt::Call {
                            func: sym.to_string(),
                            args: vec![left_id, right_id],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map.insert(
                            dest,
                            match kind {
                                "date" => Type::Named("PyDate".to_string(), vec![]),
                                "delta" => Type::Named("PyDelta".to_string(), vec![]),
                                "path" => Type::Named("PyPath".to_string(), vec![]),
                                _ => Type::I64,
                            },
                        );
                        return dest;
                    }
                }

                // Batch 779 (#203⑥ 第一格)：list / scalar 逐元素除——Python 语义
                // （[10.0, 20.0] / 2 == [5.0, 10.0]）。此前走标量 sdiv/句柄算术，
                // 元素读回是位垃圾。C 侧 zeta_vec_div_scalar（元素 f64 位、标量
                // double 形参），标量为整型时先 sitofp。
                // 只收 DynamicArray（堆 vec，[cap|len|elems] 头）——定长
                // 栈数组（Array(F64, N) 字面量形）没有 dyn 头，C 侧原样返回
                // 会别名原列表；其逐元素算术待值模型统一（#203 行内在册）。
                // Str 元素列排除：文本列的除法走 zt_col_arith 的文本 coerc 路
                // （455 批），zeta_vec_div_scalar 按 f64 位读 char* 会出垃圾
                // （t490 half[1]=1073888190 实拍）。
                if op == "/"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::DynamicArray(_))
                    )
                    && !matches!(
                        self.type_map.get(&left_id),
                        Some(Type::DynamicArray(e)) if matches!(**e, Type::Str)
                    )
                    && !matches!(
                        self.type_map.get(&right_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(..))
                    )
                {
                    let r_f64 = if matches!(
                        self.type_map.get(&right_id).cloned(),
                        Some(Type::F64)
                    ) {
                        right_id
                    } else {
                        let f = self.emit_call("zeta_float_i64", vec![right_id], Type::F64);
                        f
                    };
                    // 元素型出声：int 元素列表走真除（CPython [10,20]/2 =
                    // [5.0, 10.0]），f64 位元素直除。
                    let elem_i64 = matches!(
                        self.type_map.get(&left_id),
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _))
                            if matches!(**e, Type::I64)
                    );
                    let elem_flag = self.next_id_with_lit(elem_i64 as i64);
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_vec_div_scalar".to_string(),
                        args: vec![left_id, r_f64, elem_flag],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map.insert(
                        dest,
                        Type::DynamicArray(Box::new(Type::F64)),
                    );
                    return dest;
                }
                if op == ".." {
                    // Range expression for for loops
                    self.exprs.insert(
                        dest,
                        MirExpr::Range {
                            start: left_id,
                            end: right_id,
                        },
                    );
                    self.type_map.insert(dest, Type::Range);
                } else if (op == "==" || op == "!=")
                    && self.is_array_like(&left_id)
                    && self.is_array_like(&right_id)
                {
                    // PY-A: list equality. `a == b` on two lists used to compile
                    // to a plain integer compare of the two HANDLES, so two
                    // equal-content lists always compared 0 — a silent wrong
                    // answer in every `if a == b` guard.
                    // Batch 398: the guard used to be `||`, which also swallowed
                    // `column == scalar` — exactly the shape the element-wise
                    // mask path below exists for. Measured: `e = df["code"] ==
                    // "d"; len(df[e])` printed 0 where pandas gives 1, because
                    // `py_list_eq` compared a vector against a bare string
                    // pointer (`zt_vec_len` on the scalar) and handed back a
                    // Bool, not a mask. List-vs-list stays here; vector-vs-
                    // scalar falls through to `py_vec_cmp_str`/`py_vec_*`.
                    let elem_is_str = self
                        .array_elem_is_str(&left_id)
                        || self.array_elem_is_str(&right_id);
                    let flag = self.next_id();
                    self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                    self.type_map.insert(flag, Type::I64);
                    let eq_id = self.emit_call("py_list_eq", vec![left_id, right_id, flag], Type::Bool);
                    if op == "!=" {
                        let one = self.next_id_with_lit(1);
                        self.exprs.insert(
                            dest,
                            MirExpr::BinaryOp {
                                op: "^".to_string(),
                                left: eq_id,
                                right: one,
                            },
                        );
                    } else {
                        self.exprs.insert(dest, MirExpr::Var(eq_id));
                    }
                    self.type_map.insert(dest, Type::Bool);
                } else if op == "+"
                    && (matches!(self.type_map.get(&left_id), Some(Type::Str))
                        || matches!(self.type_map.get(&right_id), Some(Type::Str)))
                {
                    // PY-A: string concatenation — route through BinaryOp so
                    // the codegen string dispatch (host_str_concat) handles it,
                    // instead of the numeric SemiringFold adder.
                    // Batch 659: either side being a Str-refined map subscript
                    // renders through the per-key tag table first — the raw
                    // word would reach host_str_concat as a bogus pointer
                    // (measured ZT-WARN + truncated output).
                    let mut l_use = left_id;
                    let mut r_use = right_id;
                    if let AstNode::Subscript { base, index } = &**left {
                        if let Some(r) = self.mapsub_render_str(base, index) {
                            l_use = r;
                        }
                    }
                    if let AstNode::Subscript { base, index } = &**right {
                        if let Some(r) = self.mapsub_render_str(base, index) {
                            r_use = r;
                        }
                    }
                    self.exprs.insert(
                        dest,
                        MirExpr::BinaryOp {
                            op: op.clone(),
                            left: l_use,
                            right: r_use,
                        },
                    );
                    self.type_map.insert(dest, Type::Str);
                } else if op == "+"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    )
                    && matches!(
                        self.type_map.get(&right_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    )
                {
                    // PY-A: `[1, 2] + [3, 4]` — list concatenation. Previously
                    // the numeric adder ran on the two handles (garbage).
                    // Batch 737: the element type prefers the NON-degenerate
                    // side — `out = []` types its element I64, so
                    // `out + [[1, 2]]` used to keep I64 and every read of the
                    // row printed a raw pointer. I64-vs-real matches take the
                    // real element; true conflicts keep the left.
                    let le = match self.type_map.get(&left_id).cloned() {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => Some(*e),
                        _ => None,
                    };
                    let re = match self.type_map.get(&right_id).cloned() {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => Some(*e),
                        _ => None,
                    };
                    let elem = match (le, re) {
                        (Some(l), Some(r)) => {
                            if l == Type::I64 && r != Type::I64 { r } else { l }
                        }
                        (Some(l), None) => l,
                        (None, r) => r.unwrap_or(Type::I64),
                        _ => Type::I64,
                    };
                    self.stmts.push(MirStmt::Call {
                        func: "py_array_concat".to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(elem)));
                } else if op == "|"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::Named(n, _)) if n == "set"
                    )
                    || (op == "|"
                        && (matches!(
                            self.type_map.get(&left_id),
                            Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                        ) || matches!(
                            self.type_map.get(&right_id),
                            Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                        )))
                {
                    // Python sets DEGRADE TO LISTS here, so `s |= {...}` is a
                    // UNION. Without this the bitwise-or ran on two HANDLES and
                    // produced garbage: the set stayed empty, `c not in
                    // fetched_codes` was always true, and the garbage handle
                    // afterwards crashed `py_list_contains` (measured in
                    // `fetch_stocks`). Approximated by concatenation — membership
                    // stays correct, duplicates survive (dedup needs runtime
                    // content comparison; tracked in the roadmap).
                    // Prefer whichever side carries a REAL element type: `set()`
                    // reports I64 = "unknown", and taking that side made
                    // `c |= {"q"}` a DynamicArray(I64) — so `"q" in c` compared
                    // HANDLES afterwards and every string counted as absent.
                    let elem_of = |t: Option<Type>| match t {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => Some(*e),
                        _ => None,
                    };
                    // A boolean/label mask PAIR must be OR-ed element-wise, not
                    // concatenated: `isna(col) | (col < x)` produced 2N elements and
                    // `df.loc[...]` kept 2N rows.
                    // Either side being a VECTOR is enough: Python's `|` on two
                    // Series is element-wise OR (only `+` concatenates), and
                    // `isna(col)` is typed `DynamicArray(I64)`, not Bool — so the
                    // `|` used to fall through to the concat path and the mask came
                    // out 2N long (`vec_not` saw n=1784 for 892 rows).
                    let vecish = |t: Option<Type>| {
                        matches!(t, Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)))
                    };
                    let boolish = |t: Option<Type>| matches!(t, Some(Type::Bool));
                    // Batch 398: a mask keeps its Bool element through `|`. The
                    // `I64` element is this compiler's "unknown element" marker
                    // (see the `vec_push` refinement below `lower_call`), so an
                    // `I64` result here was indistinguishable from an index list
                    // at `df[...]`.
                    let maskish = |t: Option<Type>| match t {
                        Some(Type::Bool) => true,
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            matches!(*e, Type::Bool)
                        }
                        _ => false,
                    };
                    if vecish(self.type_map.get(&left_id).cloned())
                        || vecish(self.type_map.get(&right_id).cloned())
                        || boolish(self.type_map.get(&left_id).cloned())
                        || boolish(self.type_map.get(&right_id).cloned())
                    {
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_or".to_string(),
                            args: vec![left_id, right_id],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        let elem = if maskish(self.type_map.get(&left_id).cloned())
                            || maskish(self.type_map.get(&right_id).cloned())
                        {
                            Type::Bool
                        } else {
                            Type::I64
                        };
                        self.type_map
                            .insert(dest, Type::DynamicArray(Box::new(elem)));
                        return dest;
                    }
                    let elem = match (
                        elem_of(self.type_map.get(&left_id).cloned()),
                        elem_of(self.type_map.get(&right_id).cloned()),
                    ) {
                        (Some(l), Some(r)) => {
                            if matches!(l, Type::I64) {
                                r
                            } else {
                                l
                            }
                        }
                        (Some(l), None) => l,
                        (None, Some(r)) => r,
                        (None, None) => Type::I64,
                    };
                    self.stmts.push(MirStmt::Call {
                        func: "py_array_concat".to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(elem)));
                } else if op == "|"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::Named(n, _)) if n == "map" || n == "dict" || n == "set"
                    )
                    && matches!(
                        self.type_map.get(&right_id),
                        Some(Type::Named(n, _)) if n == "map" || n == "dict" || n == "set"
                    )
                {
                    // `s |= {...}` / `d |= {...}` desugar to `x = x | y`. Without
                    // this the bitwise-or ran on the two HANDLES, so the set stayed
                    // EMPTY: `c not in fetched_codes` was always true (measured in
                    // `fetch_stocks`, which then carried every code forward and
                    // finally crashed inside `py_list_contains` on the set handle).
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_map_update".to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    let lt = self.type_map.get(&left_id).cloned().unwrap_or_else(Type::slot_fallback);
                    self.type_map.insert(dest, lt);
                } else if op == "+" {
                    self.stmts.push(MirStmt::SemiringFold {
                        op: SemiringOp::Add,
                        values: vec![left_id, right_id],
                        result: dest,
                    });
                    self.exprs.insert(
                        dest,
                        MirExpr::SemiringFold {
                            op: SemiringOp::Add,
                            values: vec![left_id, right_id],
                        },
                    );
                    let op_type = match (
                        self.type_map.get(&left_id),
                        self.type_map.get(&right_id),
                    ) {
                        (Some(Type::F32) | Some(Type::F64), _)
                        | (_, Some(Type::F32) | Some(Type::F64)) => Type::F64,
                        _ => Type::I64,
                    };
                    self.type_map.insert(dest, op_type);
                } else if op == "**" {
                    // 批次 805（#表示上限格）：字面量**字面量编译期折算——
                    // 结果入 i64 ⇒ IntLit；溢出但在 128 位大数模型内 ⇒ BigInt
                    // 折算（zeta_big 家族，647 的 BigIntLit 同款出码）。此前
                    // 2**127 及以上落 zeta_pow_i64 静默溢出打 0（探针实拍；
                    // 差分过滤器按 2^126 上限设计，suite 抓不到）。变底数形
                    // （6**40）的运行期升级是另一格；>128 位超出模型上限。
                    if let (Some(a), Some(b)) =
                        (Self::ctfe_int_of(&self.exprs, &self.ctfe_consts, left_id),
                         Self::ctfe_int_of(&self.exprs, &self.ctfe_consts, right_id))
                    {
                        if b >= 0 && b <= 127 {
                            let mut r: i128 = 1;
                            let mut over = false;
                            let base = a as i128;
                            for _ in 0..b {
                                match r.checked_mul(base) {
                                    Some(x) => r = x,
                                    None => {
                                        over = true;
                                        break;
                                    }
                                }
                            }
                            if !over {
                                if r >= i64::MIN as i128 && r <= i64::MAX as i128 {
                                    self.exprs.insert(dest, MirExpr::IntLit(r as i64));
                                    self.type_map.insert(dest, Type::I64);
                                } else {
                                    let lo = self.int_slot(r as i64);
                                    let hi = self.int_slot((r >> 64) as i64);
                                    self.stmts.push(MirStmt::Call {
                                        func: "zeta_big_new".to_string(),
                                        args: vec![lo, hi],
                                        dest,
                                        type_args: vec![],
                                    });
                                    self.exprs.insert(dest, MirExpr::Var(dest));
                                    self.type_map
                                        .insert(dest, Type::Named("BigInt".to_string(), vec![]));
                                }
                                return dest;
                            }
                        }
                    }
                    // PY-A: Python's power operator. Previously `2 ** 10` was
                    // parsed as `2 * (*10)` and dereferenced the literal as a
                    // pointer (crash). Integer bases use an exponentiation
                    // loop; a float operand uses libm pow.
                    let is_float = matches!(
                        self.type_map.get(&left_id),
                        Some(Type::F64) | Some(Type::F32)
                    ) || matches!(
                        self.type_map.get(&right_id),
                        Some(Type::F64) | Some(Type::F32)
                    );
                    let func = if is_float {
                        "py_math_pow"
                    } else {
                        "zeta_pow_i64"
                    };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, if is_float { Type::F64 } else { Type::I64 });
                } else if op == "*"
                    && matches!(self.type_map.get(&left_id), Some(Type::Str))
                    && !matches!(self.type_map.get(&right_id), Some(Type::Str))
                {
                    // PY-A: `"-" * 40` — string repeat. Previously the numeric
                    // multiply ran on the pointer and produced garbage.
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_repeat".to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map.insert(dest, Type::Str);
                } else if op == "*"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::I64)
                            | Some(Type::I32)
                            | Some(Type::U32)
                            | Some(Type::U64)
                            | Some(Type::Usize)
                    )
                    && matches!(self.type_map.get(&right_id), Some(Type::Str))
                {
                    // The mirror of the arm above: `40 * "-"`. Python's `*` is
                    // commutative on `str`, and without this side the numeric
                    // multiply ran on the pointer and printed an address.
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_repeat".to_string(),
                        args: vec![right_id, left_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map.insert(dest, Type::Str);
                } else if op == "%"
                    && matches!(self.type_map.get(&left_id), Some(Type::Str))
                {
                    // PY-A: `"f=%s" % value` — Python's printf-style string
                    // formatting.  Previously the `%` fell through to integer
                    // modulo on the string pointer and the value handle, so
                    // `"f=%s" % d["name"]` printed 13 (garbage) instead of
                    // f=abc (measured in t401).  Route through a dedicated
                    // runtime function that parses the format string and
                    // converts each value — PyDynamic goes through
                    // zeta_dyn_to_string, integers get snprintf'd, strings
                    // pass through as char*.
                    let left_id = self.materialize_for_call(left_id);
                    let right_id = self.materialize_for_call(right_id);
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_str_percent_fmt".to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map.insert(dest, Type::Str);
                } else if op == "*"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    )
                    && matches!(
                        self.type_map.get(&right_id),
                        Some(Type::I64)
                            | Some(Type::I32)
                            | Some(Type::U32)
                            | Some(Type::U64)
                            | Some(Type::Usize)
                    )
                {
                    // PY-A: `[0] * 3` — list repeat (also previously garbage).
                    let elem = match self.type_map.get(&left_id).cloned() {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => *e,
                        _ => Type::I64,
                    };
                    self.stmts.push(MirStmt::Call {
                        func: "py_array_repeat".to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(elem)));
                } else if op == "*"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    )
                    && matches!(
                        self.type_map.get(&right_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    )
                {
                    // Two VECTORS: pandas' `df["close"] * df["volume"]` is an
                    // ELEMENT-WISE product. The SemiringFold (matmul) path below
                    // walked the STRING elements as arrays and crashed in
                    // `array_len` (measured: `validate_and_repair_stock_ohlcv`'s
                    // `amount` recompute, `probe + 3316`).
                    self.stmts.push(MirStmt::Call {
                        func: "py_vec_mul".to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(Type::Str)));
                } else if op == "*" || op == "@" {
                    // 批次145 重放: `@`（Python matmul）与 `*` 同走 SemiringFold
                    // Mul——标量 matmul 即乘法（t214: 3@4=12）；数组 matmul 超出
                    // V1 范围。此前 `@` 落入未知 op 分支 → Call{func:"@"} → 链接失败。
                    self.stmts.push(MirStmt::SemiringFold {
                        op: SemiringOp::Mul,
                        values: vec![left_id, right_id],
                        result: dest,
                    });
                    self.exprs.insert(
                        dest,
                        MirExpr::SemiringFold {
                            op: SemiringOp::Mul,
                            values: vec![left_id, right_id],
                        },
                    );
                    let op_type = match (
                        self.type_map.get(&left_id),
                        self.type_map.get(&right_id),
                    ) {
                        (Some(Type::F32) | Some(Type::F64), _)
                        | (_, Some(Type::F32) | Some(Type::F64)) => Type::F64,
                        _ => Type::I64,
                    };
                    self.type_map.insert(dest, op_type);
                } else if matches!(op.as_str(), ">" | "<" | ">=" | "<=" | "==" | "!=") && {
                    let is_arr = |t: Option<Type>| {
                        matches!(t, Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)))
                    };
                    // Batch 398: `x is None` arrives here as `x == 0` — the parser
                    // erases `is`/`is not` into `==`/`!=` (`parser/expr.rs:2491-2495`)
                    // and `None` into `Lit(0)` (`:1467`), so an IDENTITY test on a
                    // list-typed value is textually indistinguishable from
                    // `column == 0`. Measured: t150's `if types is None` took the
                    // mask path (a non-empty handle is truthy) and printed `2`.
                    // Only `==`/`!=` against a zero literal is excluded — `>`/`<`
                    // can never be an identity test. The excluded shape falls to the
                    // generic `==` below, which compares the two HANDLES, i.e. the
                    // identity answer. Root fix (keep `is` as its own op) registered.
                    let identity_sentinel = matches!(op.as_str(), "==" | "!=")
                        && [left_id, right_id]
                            .iter()
                            .any(|id| matches!(self.exprs.get(id), Some(MirExpr::IntLit(0))));
                    !identity_sentinel
                        && (is_arr(self.type_map.get(&left_id).cloned())
                            ^ is_arr(self.type_map.get(&right_id).cloned()))
                } {
                    // `column > scalar` — ELEMENT-WISE comparison producing a 0/1
                    // mask. Without this the result was typed Bool, so the mask
                    // was used as a vector: `len(mask)` read `mask-16` on a small
                    // integer and `DataFrame.loc` aborted ("mask is missing").
                    // Measured in `remove_extreme_return_bars`
                    // (`mask = ret > max_abs_daily_return`).
                    let arr_side = |t: Option<Type>| {
                        matches!(t, Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)))
                    };
                    let left_is_arr = arr_side(self.type_map.get(&left_id).cloned());
                    let (vec_id, num_id) = if left_is_arr {
                        (left_id, right_id)
                    } else {
                        (right_id, left_id)
                    };
                    let base = match op.as_str() {
                        ">" => "py_vec_gt",
                        "<" => "py_vec_lt",
                        ">=" => "py_vec_ge",
                        "<=" => "py_vec_le",
                        "==" => "py_vec_eq",
                        _ => "py_vec_ne",
                    };
                    let num_is_float = matches!(
                        self.type_map.get(&num_id),
                        Some(Type::F32) | Some(Type::F64)
                    ) || matches!(self.exprs.get(&num_id), Some(MirExpr::FloatLit(_)));
                    // A float literal's type_map entry is not always F64, so
                    // `ret > 0.2` used to pick the `_i` variant and pass a
                    // truncated 0 (`sum` was 891/892 instead of a small count,
                    // and an all-true mask then aborted in `DataFrame.loc`).
                    // A float LITERAL must travel as its bit pattern: the codegen
                    // puts literals in integer registers, so a `double` parameter
                    // arrived as 0 (`ret > 0.2` behaved like `ret > 0`; measured
                    // `sum == 891/892` and an all-true mask).
                    let lit_bits = match self.exprs.get(&num_id) {
                        Some(MirExpr::FloatLit(v)) if num_is_float => {
                            Some(v.to_bits() as i64)
                        }
                        _ => None,
                    };
                    let num_arg = match lit_bits {
                        Some(bits) => self.next_id_with_lit(bits),
                        None => num_id,
                    };
                    // A STRING vector (our "YYYY-MM-DD" dates) must compare with
                    // strcmp: the numeric variants parsed the dates with strtod
                    // and `col >= "2023-08-05"` answered 0 for every row, making
                    // the cache-coverage check see an empty slice.
                    let elem_is_str = matches!(
                        self.type_map.get(&vec_id),
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) if matches!(**e, Type::Str)
                    ) && matches!(self.type_map.get(&num_id), Some(Type::Str));
                    let kind = match op.as_str() {
                        ">" => 0i64,
                        "<" => 1,
                        ">=" => 2,
                        "<=" => 3,
                        "==" => 4,
                        _ => 5,
                    };
                    // `trade_date >= pd.Timestamp("2023-08-05")` — the column is
                    // a vector of "YYYY-MM-DD" STRINGS (the parquet reader
                    // formats INT64 dates that way) while the scalar is a PyDate
                    // HANDLE. The numeric variants passed the raw pointer as
                    // rhs (`strtod("2022-05-05")=2022 >= 4.3e9` → all-false
                    // mask), so `partial` in `fetch_stocks` was always EMPTY,
                    // nothing was ever appended to `all_data`, and the empty
                    // list's pointer-truthiness then fed `concat([])` a NULL
                    // first frame (SIGSEGV in `column_names`). Render the
                    // handle as its date string and strcmp instead —
                    // "YYYY-MM-DD" sorts chronologically.
                    let num_is_dt = matches!(
                        self.type_map.get(&num_id),
                        Some(Type::Named(n, _)) if n == "PyDate"
                    );
                    if num_is_dt {
                        let fmt_id = self.next_id();
                        self.exprs
                            .insert(fmt_id, MirExpr::StringLit("%Y-%m-%d".to_string()));
                        self.type_map.insert(fmt_id, Type::Str);
                        let ts_id = self.emit_call("py_dt_strftime", vec![num_id, fmt_id], Type::Str);
                        let kid = self.next_id_with_lit(kind);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_cmp_str".to_string(),
                            args: vec![vec_id, ts_id, kid],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map
                            .insert(dest, Type::DynamicArray(Box::new(Type::Bool)));
                        return dest;
                    }
                    let func = if elem_is_str {
                        let kid = self.next_id_with_lit(kind);
                        let kid2 = self.next_id_with_lit(kind);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_cmp_str".to_string(),
                            args: vec![vec_id, num_arg, kid],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map
                            .insert(dest, Type::DynamicArray(Box::new(Type::Bool)));
                        let _ = kid2;
                        return dest;
                    } else if lit_bits.is_some() {
                        format!("{}_bits", base)
                    } else if num_is_float {
                        base.to_string()
                    } else {
                        // Batch 779 (#203⑥)：变体跟**向量元素型**走——F64 元素
                        // 的列表配浮点变体（元素是 f64 位），整型标量在 C 侧
                        // sitofp；落 `_i` 会把 f64 位当整数除（[10.0,20.0]/2
                        // 元素读回 2.3e18 实拍）。
                        let vec_elem_f64 = matches!(
                            self.type_map.get(&vec_id),
                            Some(Type::DynamicArray(e)) | Some(Type::Array(e, _))
                                if matches!(**e, Type::F64)
                        );
                        if vec_elem_f64 {
                            base.to_string()
                        } else {
                            format!("{}_i", base)
                        }
                    };
                    self.stmts.push(MirStmt::Call {
                        func,
                        args: vec![vec_id, num_arg],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(Type::Bool)));
                } else if matches!(op.as_str(), "&" | "|") && {
                    let is_arr_mask = |t: Option<Type>| {
                        matches!(t, Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)))
                    };
                    is_arr_mask(self.type_map.get(&left_id).cloned())
                        || is_arr_mask(self.type_map.get(&right_id).cloned())
                } {
                    // `(col >= x) & (col <= y)` — element-wise mask AND/OR.
                    // Falling through to the generic path emitted a scalar
                    // LLVM `and` of the two vector HANDLES (garbage pointer
                    // that passed `py_is_vec`, then SIGSEGV in `sum`/`loc`;
                    // measured in `fetch_stocks`' cache-window filter).
                    let func = if op == "&" { "py_vec_and" } else { "py_vec_or" };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    // Batch 398: the mask element survives the AND/OR — a
                    // `DynamicArray(Bool)` is what `df[...]` reads to tell a row
                    // filter from an index list. An operand that only arrived as
                    // `lt(vec, i64)` (the shim's `loc`/`iloc` annotation) keeps the
                    // legacy element.
                    let maskish = |t: Option<Type>| match t {
                        Some(Type::Bool) => true,
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            matches!(*e, Type::Bool)
                        }
                        _ => false,
                    };
                    let elem = if maskish(self.type_map.get(&left_id).cloned())
                        || maskish(self.type_map.get(&right_id).cloned())
                    {
                        Type::Bool
                    } else {
                        Type::I64
                    };
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(elem)));
                } else {
                    // For comparison operators used in loop conditions, create BinaryOp expression
                    // instead of caching the result in a variable
                    if op == "<"
                        || op == ">"
                        || op == "<="
                        || op == ">="
                        || op == "=="
                        || op == "!="
                        || op == "&&"
                        || op == "||"
                    {
                        // 容器比较族（container_cmp_order，批次 553）：list/tuple
                        // 的 `< <= > >= == !=` 都是先按词典序（或逐元素相等）
                        // 得到 -1/0/1，再与 0 比较——掉到这里的整型回退比的是
                        // 句柄，`[1, 2] < [1, 3]` 恒 False。== / != 也从这里走：
                        // 旧表路径不认元组变量（t1 == (1, 2) 打 False 实拍）。
                        let vec_shaped = |t: Option<&Type>| match t {
                            Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                            | Some(Type::Tuple(_)) => true,
                            Some(Type::Named(n, _)) => n == "tuple",
                            _ => false,
                        };
                        let both_vec = matches!(
                            op.as_str(),
                            "<" | "<=" | ">" | ">=" | "==" | "!="
                        ) && vec_shaped(self.type_map.get(&left_id))
                            && vec_shaped(self.type_map.get(&right_id));
                        if both_vec {
                            let elem_is_str = match (
                                self.type_map.get(&left_id),
                                self.type_map.get(&right_id),
                            ) {
                                (Some(Type::DynamicArray(e)), _)
                                | (_, Some(Type::DynamicArray(e)))
                                    if matches!(**e, Type::Str) =>
                                {
                                    1i64
                                }
                                (Some(Type::Tuple(ts)), _)
                                | (_, Some(Type::Tuple(ts)))
                                    if ts.first().map_or(false, |t| matches!(t, Type::Str)) =>
                                {
                                    1
                                }
                                _ => 0,
                            };
                            let flag = self.next_id_with_lit(elem_is_str);
                            let cmp_id = self.emit_call("py_list_cmp", vec![left_id, right_id, flag], Type::I64);
                            let zero = self.next_id_with_lit(0);
                            self.exprs.insert(
                                dest,
                                MirExpr::BinaryOp {
                                    op: op.clone(),
                                    left: cmp_id,
                                    right: zero,
                                },
                            );
                            self.type_map.insert(dest, Type::Bool);
                            return dest;
                        }
                        // dict 的 == / !=：键值全等（map__eq）。此前两个句柄
                        // 按整数比较，内容相等的两个 dict 恒 False。
                        let map_shaped = |t: Option<&Type>| {
                            matches!(t, Some(t) if t.is_map())
                        };
                        if matches!(op.as_str(), "==" | "!=")
                            && map_shaped(self.type_map.get(&left_id))
                            && map_shaped(self.type_map.get(&right_id))
                        {
                            let eq_id = self.emit_call("map__eq", vec![left_id, right_id], Type::I64);
                            if op == "==" {
                                // map__eq IS the answer — comparing it to 0
                                // here would invert it (measured: d1 == d1
                                // printed False).
                                self.exprs.insert(dest, MirExpr::Var(eq_id));
                                self.type_map.insert(dest, Type::Bool);
                                return dest;
                            }
                            let zero = self.next_id_with_lit(0);
                            self.exprs.insert(
                                dest,
                                MirExpr::BinaryOp {
                                    op: "==".to_string(),
                                    left: eq_id,
                                    right: zero,
                                },
                            );
                            self.type_map.insert(dest, Type::Bool);
                            return dest;
                        }
                        self.exprs.insert(
                            dest,
                            MirExpr::BinaryOp {
                                op: op.clone(),
                                left: left_id,
                                right: right_id,
                            },
                        );
                        // PY-A: string operands — result of `+` is a string
                        // (concat), comparisons yield Bool. print/println
                        // dispatch relies on this type.
                        if matches!(op.as_str(), "+" | "==" | "!=") {
                            let l_str = matches!(self.type_map.get(&left_id), Some(Type::Str));
                            let r_str = matches!(self.type_map.get(&right_id), Some(Type::Str));
                            if l_str || r_str {
                                let ty = if op == "+" { Type::Str } else { Type::Bool };
                                self.type_map.insert(dest, ty);
                            }
                        }
                    } else {
                        self.stmts.push(MirStmt::Call {
                            func: op.to_string(),
                            args: vec![left_id, right_id],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                    }
                    // Preserve float type for arithmetic ops — `c / d` on f64
                    // operands must stay f64, otherwise later casts misbehave.
                    //
                    // But a COMPARISON of two floats is still a bool: `w = 5.0 >
                    // 1.0` was typed f64, so `w` held the raw integer 1 in a
                    // double slot and `print(w)` printed 0.000000 — a silent
                    // wrong value in every float guard.
                    let is_cmp = matches!(
                        op.as_str(),
                        "==" | "!=" | "<" | ">" | "<=" | ">=" | "&&" | "||" | "in" | "not in"
                    );
                    let op_type = match (
                        self.type_map.get(&left_id),
                        self.type_map.get(&right_id),
                    ) {
                        // BATCH-455 (#203②): a COLUMN operand makes the answer
                        // another column, never a double. The float arm below typed
                        // `100.0 / df["open"]` F64, so the runtime handle was stored
                        // in a double slot and `DataFrame::__setitem__` was called
                        // with `fptosi(handle)` — the new column read back `<null>`.
                        // I64 is the handle representation codegen already uses for
                        // the column-first spelling (`df["close"] / 2`).
                        // The op list must stay in sync with
                        // `Codegen::column_arith_dispatch` (codegen.rs): a route that
                        // fires without this arm re-creates exactly this bug.
                        // Batch 458: the answer's element representation is text, not
                        // a bare handle word. `zt_col_arith` (py_additions.c:3678)
                        // unconditionally `snprintf("%.10g")`s each element and pushes
                        // the `GC_strdup`ed `char*`, so typing this I64 made every
                        // consumer print the pointer: `h = df["amount"] / 2` then
                        // `print(h[0])` or `for x in h` answered
                        // `4347658192／4347658176` (rc=0, silent wrong value). Only the
                        // `df["r"] = ...` spelling was right, because there the map tag
                        // side table (456) drives the read boundary instead of this
                        // type. `DynamicArray(Str)` is the same i64-word representation
                        // as I64 — the handle still crosses as one word — so 455's
                        // fptosi problem stays solved: this arm is still ahead of the
                        // F64 arm.
                        _ if !is_cmp
                            && matches!(
                                op.as_str(),
                                "/" | "div" | "floordiv" | "%" | "mod" | "-" | "sub"
                            )
                            && (matches!(
                                self.type_map.get(&left_id),
                                Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                            ) || matches!(
                                self.type_map.get(&right_id),
                                Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                            )) =>
                        {
                            Type::DynamicArray(Box::new(Type::Str))
                        }
                        (Some(Type::F32) | Some(Type::F64), _)
                        | (_, Some(Type::F32) | Some(Type::F64)) => {
                            if is_cmp {
                                Type::Bool
                            } else {
                                Type::F64
                            }
                        }
                        // Batch 291: an INTEGER comparison is still a Bool.
                        // `1 == 1` / `i < n` fell to the I64 fallback, so
                        // `print(1 == 1)` printed `1` and any `x = (a == b)`
                        // slot was an i64 — print/println dispatch never
                        // reached print_bool. Python: every comparison yields
                        // a bool; the runtime rep is i64 either way, only the
                        // inferred TYPE drives printing and later `type()`.
                        // NB: `&&`/`||` are NOT refinement targets here — they are
                        // value-selecting in Python and get their own op_type pass
                        // right below; typing them Bool here broke 38 tests.
                        _ if is_cmp && !matches!(op.as_str(), "||" | "&&") => Type::Bool,
                        // PY-A: `/` on two integers is TRUE division. The I64
                        // fallback below typed the quotient as an integer and the
                        // codegen ran `sdiv`, so `round(66 / 10)` answered 6
                        // instead of 7. Only both-integral operands are promoted:
                        // a Str/Named/Dynamic side keeps its previous answer.
                        // `//` never reaches here (the parser emits "floordiv").
                        (Some(Type::I64) | Some(Type::Bool), Some(Type::I64) | Some(Type::Bool))
                            if op == "/" && !dead_div =>
                        {
                            Type::F64
                        }
                        _ => Type::I64,
                    };
                    // Python `and`/`or` are VALUE-selecting, not boolean:
                    // `cfg = m or {}` evaluates to `m` (a map) or `{}` (a map), and
                    // `t = s or "x"` to a string. Typing the result I64 turned every
                    // downstream method call (`cfg.get(k, d)`, `t.upper()`) into an
                    // opaque bare symbol — `_get` had 7 reference sites in the
                    // REasyQuant local backtest, 3 of them from
                    // `CostModel::from_jq`'s `cfg = config or {}` + `cfg.get(...)`.
                    // Only refine when the operands agree on a concrete type (or one
                    // is concrete and the other is the untyped default), so
                    // `if a or b:` keeps Bool.
                    let op_type = if matches!(op.as_str(), "||" | "&&") {
                        let lt = self.type_map.get(&left_id).cloned();
                        let rt = self.type_map.get(&right_id).cloned();
                        let concrete = |t: &Option<Type>| match t {
                            Some(Type::I64)
                            | Some(Type::PyDynamic)
                            | Some(Type::Bool)
                            | None => None,
                            other => other.clone(),
                        };
                        match (concrete(&lt), concrete(&rt)) {
                            (Some(a), Some(b)) if a == b => a,
                            (Some(a), _) => a,
                            (_, Some(b)) => b,
                            _ => match (lt, rt) {
                                (Some(a), Some(b)) if a == b => a,
                                _ => Type::I64,
                            },
                        }
                    } else {
                        op_type
                    };
                    self.type_map.insert(dest, op_type);
                }
                return dest;
            
    }
}
