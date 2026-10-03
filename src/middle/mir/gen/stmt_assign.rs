//! 批次 865：Assign 语句臂发射体（自 gen.rs 原臂逐字迁入）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::{ArraySize, Type};

impl MirGen {
    /// 赋值语句：类变量写改写、带注解赋值、并行赋值（RHS 整体求值）、
    /// 调用返回元组解包、普通单赋值——形状分派链（原臂逐字）。
    pub(super) fn lower_assign_stmt(&mut self, lhs: &AstNode, rhs: &AstNode) {
            // Batch 572: `Counter.count = x` — class VARIABLE write rewrites
            // to the mangled module global (statement position).
            if let AstNode::FieldAccess { base, field } = lhs {
                if let AstNode::Var(vname) = &**base {
                    let gname = format!("{}__{}", vname, field);
                    if self.type_decls.contains_key(vname.as_str())
                        && self.module_globals.contains(&gname)
                    {
                        let rewritten = AstNode::Assign(
                            Box::new(AstNode::Var(gname)),
                            Box::new(rhs.clone()),
                        );
                        self.lower_ast(&rewritten);
                        return;
                    }
                }
            }
            // PY-A: annotated assignment `partial: pd.DataFrame | None = None`
            // — the parser keeps the class-shaped annotation attached. Lower
            // the plain assignment first, then (when the slot is still
            // untyped) adopt the annotated class: `= None` types the slot
            // I64 and rebinds never refresh it, so `len(partial)` used to
            // dispatch `array_len` on a DataFrame handle (header read = 0
            // rows) and every fetch_stocks cache gate failed.
            if let AstNode::TypeAnnotatedPattern { pattern: inner, ty } = lhs {
                if let AstNode::Var(name) = &**inner {
                    let bare = AstNode::Assign(
                        Box::new((**inner).clone()),
                        Box::new(rhs.clone()),
                    );
                    self.lower_ast(&bare);
                    if let Some(&slot) = self.name_to_id.get(name.as_str()) {
                        let cur = self.type_map.get(&slot).cloned();
                        if matches!(cur, None | Some(Type::I64) | Some(Type::PyDynamic)) {
                            if let Some(nt) = self.annotation_named_ty(ty) {
                                self.type_map.insert(slot, nt);
                            }
                        }
                        // 批次 879（#276）：容器注解的元素型细化——与 Let 臂的
                        // TypeAnnotatedPattern 分支同款（annotation_elem_ty）。
                        // `codes: set[str] = set()` 的 RHS 只能降成元素未知的空
                        // 容器（DynamicArray(I64)），注解是元素型的唯一记录；
                        // 这里不细化，后续 `|=` 集合路由就认不出 Str 元（成员判
                        // 定按句柄判等）。注意守卫不能挡 DynamicArray——那正是
                        // 待细化的形状。
                        let refined = cur.clone().map(|t| {
                            match (&t, MirGen::annotation_elem_ty(ty)) {
                                (Type::DynamicArray(e), Some(el))
                                    if matches!(**e, Type::I64) =>
                                {
                                    Type::DynamicArray(Box::new(el))
                                }
                                (Type::I64, Some(el)) | (Type::PyDynamic, Some(el)) => {
                                    Type::DynamicArray(Box::new(el))
                                }
                                _ => t,
                            }
                        });
                        if let Some(rt) = refined {
                            self.type_map.insert(slot, rt);
                        }
                        self.apply_dict_annotation(slot, ty);
                    }
                    return;
                }
            }
            // PY-A: parallel assignment `a, b = x, y` (tuple unpacking with
            // tuple rhs; call-return unpacking needs temps — later item)
            // PY-A (批次 531/#190): Python 语义 = RHS 先**整体求值**再逐个绑定。
            // 旧的逐对 Assign 顺序执行 ⇒ `a, b = b, a` 腐蚀为 (2,2)。
            // 修法：先把每个 RHS 元素赋给临时变量（__swap_tmp_N），
            // 再从临时变量赋给 LHS 目标——打断顺序依赖。
            if let (AstNode::Tuple(litems), AstNode::Tuple(ritems)) = (lhs, rhs) {
                if litems.len() == ritems.len() && !litems.is_empty() {
                    let temp_names: Vec<String> = (0..litems.len())
                        .map(|i| format!("__swap_tmp_{}", i))
                        .collect();
                    // Step 1: RHS → temps
                    for (i, r) in ritems.iter().enumerate() {
                        let pair = AstNode::Assign(
                            Box::new(AstNode::Var(temp_names[i].clone())),
                            Box::new(r.clone()),
                        );
                        self.lower_ast(&pair);
                    }
                    // Step 2: temps → LHS targets
                    for (i, l) in litems.iter().enumerate() {
                        let pair = AstNode::Assign(
                            Box::new(l.clone()),
                            Box::new(AstNode::Var(temp_names[i].clone())),
                        );
                        self.lower_ast(&pair);
                    }
                    return;
                }
            }
            // PY-A: call-return tuple unpacking `a, b = f()` — the rhs is
            // lowered ONCE into a temp, elements read via stack_array_get
            // (tuples materialize as fixed-size arrays).
            if let AstNode::Tuple(litems) = lhs {
                if !litems.is_empty()
                    && !matches!(rhs, AstNode::Tuple(_))
                {
                    let rhs_id = self.lower_expr(rhs);
                    // Element type from the SOURCE, not hardcoded I64:
                    // `num, exch = jq.split(".")` gave correctly-valued but
                    // I64-typed names, so `num.isdigit()` /
                    // `num.startswith(("000","399"))` dispatched as handle
                    // methods on an integer — a garbage pointer and a SEGV in
                    // `_is_likely_index` (measured in the local backtest).
                    for (i, l) in litems.iter().enumerate() {
                        if let AstNode::Var(name) = l {
                            let elem_id = self.next_id();
                            let idx_id = self.next_id();
                            self.exprs
                                .insert(idx_id, MirExpr::IntLit(i as i64));
                            self.type_map.insert(idx_id, Type::I64);
                            self.stmts.push(MirStmt::Call {
                                func: "stack_array_get".to_string(),
                                args: vec![rhs_id, idx_id],
                                dest: elem_id,
                                type_args: vec![],
                            });
                            self.name_to_id.insert(name.clone(), elem_id);
                            self.exprs.insert(elem_id, MirExpr::Var(elem_id));
                            // Element type from the SOURCE, not hardcoded I64:
                            // `num, exch = jq.split(".")` produced correctly
                            // valued but I64-typed names, so `num.isdigit()` and
                            // `num.startswith(("000","399"))` dispatched as
                            // handle methods on an integer — garbage pointer,
                            // SEGV in `_is_likely_index` (local backtest).
                            let ty = match self.type_map.get(&rhs_id).cloned() {
                                Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => *e,
                                Some(Type::Tuple(ts)) => {
                                    ts.get(i).cloned().unwrap_or_else(Type::slot_fallback)
                                }
                                // A function returning a tuple ANNOTATED in
                                // Python spelling (`-> tuple[pd.DataFrame, int]`,
                                // `remove_extreme_return_bars`) is typed
                                // `Named("tuple", [..])`, not `Type::Tuple` —
                                // the element fell to the I64 default, so the
                                // destructured frame had a garbage type
                                // (`len(out.columns)` → MAP lookup = 0;
                                // `out["a"]` → SEGV).
                                Some(Type::Named(n, ts)) if n == "tuple" => {
                                    ts.get(i).cloned().unwrap_or_else(Type::slot_fallback)
                                }
                                Some(Type::Str) => Type::Str,
                                _ => Type::I64,
                            };
                            self.type_map.insert(elem_id, ty);
                        }
                    }
                    return;
                }
            }
            // PY-A: Python-style `f = lambda …` must register the closure
            // binding, exactly like the Zeta `let f = lambda …` path does.
            // Without it a later `f(x)` emitted a CALL to a symbol named `f`,
            // which does not exist — the link failed with `_f` undefined.
            let binding_name = match lhs {
                AstNode::Var(n) if matches!(rhs, AstNode::Closure { .. }) => Some(n.clone()),
                _ => None,
            };
            if let Some(n) = &binding_name {
                self.pending_closure_binding = Some(n.clone());
            }
            let rhs_id = self.lower_expr(rhs);
            self.pending_closure_binding = None;
            // 批次 564: the parser desugars a slice LHS `xs[a:b] = repl`
            // into `__slice__(xs, a, b)` CALL — not a Subscript — so the
            // assignment was dropped silently. Splice via the runtime and
            // rebind the base slot (the vec_push rebind convention).
            // The slice LHS parses as Call{receiver: base, method:
            // "__slice__", args: [start, end]} (2-arg form; the parser
            // folds omitted bounds into Lit(0) / Lit(i64::MIN) sentinels).
            if let AstNode::Call {
                method,
                args: slice_args,
                receiver: Some(recv),
                ..
            } = lhs
            {
                if method == "__slice__" && slice_args.len() == 2 {
                    if let AstNode::Var(bname) = &**recv {
                        if let Some(base_slot) = self.name_to_id.get(bname).copied() {
                            let xs = self.lower_expr(recv);
                            let s_id = self.lower_expr(&slice_args[0]);
                            let e_id = self.lower_expr(&slice_args[1]);
                            let repl_id = self.lower_expr(rhs);
                            let nid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_list_splice".to_string(),
                                args: vec![xs, s_id, e_id, repl_id],
                                dest: nid,
                                type_args: vec![],
                            });
                            self.exprs.insert(nid, MirExpr::Var(nid));
                            self.type_map
                                .insert(nid, Type::DynamicArray(Box::new(Type::I64)));
                            self.stmts.push(MirStmt::Assign {
                                lhs: base_slot,
                                rhs: nid,
                            });
                            return;
                        }
                    }
                }
            }
            if let AstNode::Subscript { base, index } = lhs {
                // 批次 564: slice assignment `xs[a:b] = repl` — previously
                // dropped silently (xs unchanged). Splice via the runtime
                // and rebind the base slot to the new handle (the vec_push
                // rebind convention — other aliases see stale data, same
                // limitation the codebase already documents).
                // `1:3` and `1..3` are both valid slice spellings here.
                let slice_bounds: Option<(&AstNode, &AstNode)> =
                    match &**index {
                        AstNode::Range { start, end, .. } => Some((start, end)),
                        AstNode::BinaryOp { op, left, right }
                            if op == ".." =>
                        {
                            Some((left, right))
                        }
                        _ => None,
                    };
                if let (AstNode::Var(bname), Some((start, end))) =
                    (&**base, slice_bounds)
                {
                    let base_slot = self.name_to_id.get(bname).copied();
                    if let Some(base_slot) = base_slot {
                        let xs = self.lower_expr(base);
                        let s_id = self.lower_expr(start);
                        let e_id = self.lower_expr(end);
                        let repl_id = self.lower_expr(rhs);
                        let nid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "py_list_splice".to_string(),
                            args: vec![xs, s_id, e_id, repl_id],
                            dest: nid,
                            type_args: vec![],
                        });
                        self.exprs.insert(nid, MirExpr::Var(nid));
                        self.type_map
                            .insert(nid, Type::DynamicArray(Box::new(Type::I64)));
                        // rebind the base variable to the new handle
                        self.stmts.push(MirStmt::Assign {
                            lhs: base_slot,
                            rhs: nid,
                        });
                        return;
                    }
                }
                let base_id = self.lower_expr(base);
                let index_id = self.lower_expr(index);

                // Check if base is an array type
                let base_ty = self.type_map.get(&base_id).cloned().unwrap_or_else(Type::slot_fallback);
                let source_ty = self.source_types.get(&base_id).cloned().unwrap_or_default();
                let is_array_param =
                    source_ty.starts_with("[") || source_ty.starts_with("*mut [");
                // PY-A (任务 #53, 写侧): an unnormalized negative index reached
                // `array_set` too — measured `l = [3,5,7]; l[0-1] = 9` wrote a
                // slot past the end and grew the list to
                // `[3, 5, 7, 0, 0, 0, 0, 0, 0]` instead of `[3, 5, 9]`.
                let index_id =
                    self.normalize_subscript_index(base_id, &base_ty, index, index_id);
                // 批次146 重放: subscript ASSIGN on a KNOWN struct with
                // `__setitem__` (`f["a"] = 1`, `df["col"] = [...]`) must
                // dispatch the qualified method — previously it fell to
                // DictInsert on the struct pointer (garbage write).
                if let Type::Named(n, _) = &base_ty {
                    if n != "map" && n != "dict" {
                        if let Some(qualified) =
                            self.qualified_method_candidate(n, "__setitem__")
                        {
                            self.stmts.push(MirStmt::VoidCall {
                                func: qualified,
                                args: vec![base_id, index_id, rhs_id],
                            });
                            // 批次 535：列写侧登记元素表示。`df["col"] = <容器>` 走
                            // shim 的 `__setitem__` → `py_df_setitem`，那条路把实参
                            // 句柄**原样**转进列映射，而元素表示只在出码层的
                            // `DictInsert` 臂登记过 —— 下标赋值这条路于是留 tag 0，
                            // 读边界 `zt_col_as_text` 按批次 456 的约定"不复制、不猜"
                            // 原样返回句柄，非文本格被当 `char*` 交给 `str_trim`。
                            // 语料实拍（lldb 零重编）：`map_insert <- py_df_setitem`
                            // 发布的列 cap=2048 len=1615 且 1615 格全为 0，同一句柄
                            // 被 `zt_vec_textify(tag=0)` 原样返回 ⇒ 崩 str_trim+24。
                            if let Some(tag) =
                                Self::zt_container_value_tag(self.type_map.get(&rhs_id))
                            {
                                let tag_id = self.int_slot(tag);
                                self.stmts.push(MirStmt::VoidCall {
                                    func: "py_df_set_value_tag".to_string(),
                                    args: vec![base_id, index_id, tag_id],
                                });
                            }
                            return;
                        }
                    }
                }
                // `d[k] = v` on a dict/Counter: a map insert, keyed by the
                // content hash (the index was already lowered through
                // lower_map_key for map literals; do the same here).
                if matches!(&base_ty, Type::Named(n, _) if n == "map") {
                    // 批次146: hash by the MAP's declared key type (same as
                    // the expression path) — a param-typed key used to be
                    // pointer-hashed and never matched.
                    let key_id = self.lower_map_key_typed(index_id, Some(&base_ty));
                    // Batch 299: `m = {}` binds map[i64, i64], so every later
                    // `m[k] = obj` left the map's VALUE type i64 — `m.values()`
                    // then handed out i64 elements and `p.code` read struct
                    // fields off the handle (`PositionLedger.positions`).
                    // Inserting a value refines the declared type, exactly like
                    // the first entry of a dict literal.
                    let val_ty = self.type_map.get(&rhs_id).cloned().unwrap_or_else(Type::slot_fallback);
                    let key_ty = self
                        .type_map
                        .get(&index_id)
                        .cloned()
                        .unwrap_or(Type::I64);
                    if let Type::Named(_, targs) = &base_ty {
                        let old_key = targs.first().cloned().unwrap_or_else(Type::slot_fallback);
                        let old_val = targs.get(1).cloned().unwrap_or_else(Type::slot_fallback);
                        let new_key = if matches!(old_key, Type::I64)
                            && matches!(key_ty, Type::Str)
                        {
                            Type::Str
                        } else {
                            old_key.clone()
                        };
                        // Batch 765 (值标签大弧): first-insert refinement
                        // for PyDynamic placeholders was TRIED and
                        // REVERTED — it fixes homogeneous `dict[str, Any]`
                        // (t450 len dispatches __len__ = 2, 5-run stable)
                        // but re-arms batch 413's heterogeneous SEGV:
                        // t449 writes DataFrame first, then a list — the
                        // pinned V dispatches DataFrame.__len__ on a vec
                        // handle (413-era rc=139). The correct grid is
                        // PER-CELL tags (zeta_map_set_tag side table with
                        // a class-id alphabet), not dict-level pinning.
                        let new_val = if matches!(old_val, Type::I64)
                            && !matches!(val_ty, Type::I64)
                        {
                            val_ty.clone()
                        } else {
                            old_val.clone()
                        };
                        if new_key != old_key || new_val != old_val {
                            self.type_map.insert(
                                base_id,
                                Type::Named("map".to_string(), vec![new_key, new_val]),
                            );
                        }
                    }
                    self.stmts.push(MirStmt::DictInsert {
                        map_id: base_id,
                        key_id,
                        val_id: rhs_id,
                    });
                    // Batch 766 (值标签大弧·写侧第一步)：值是 py 模式类实例
                    // ⇒ 侧表登记类 id（CLASS_TAG_BASE+id）。运行期只存不解释；
                    // 读侧按格分派是下一格。现有字母表 0..7（标量＋vec 族）
                    // 不覆盖类实例 ⇒ 类实例在 dict[str, Any] 槽读回即失型
                    // （t450 几何判形闪败的写侧半）。
                    if let Type::Named(cn, _) = &val_ty {
                        if let Some(tag) = self.class_tag_id(cn) {
                            let tag_id = self.int_slot(tag);
                            self.stmts.push(MirStmt::VoidCall {
                                func: "zeta_map_set_tag".to_string(),
                                args: vec![base_id, key_id, tag_id],
                            });
                        }
                    }
                } else if let Type::DynamicArray(_) = base_ty {
                    // Generate array_set call for dynamic arrays
                    self.stmts.push(MirStmt::VoidCall {
                        func: "array_set".to_string(),
                        args: vec![base_id, index_id, rhs_id],
                    });
                } else if let Type::Array(_, size) = base_ty {
                    // Check if this is a stack array (fixed size) or heap array
                    match size {
                        ArraySize::Literal(n) if n <= 20000 => {
                            // Small fixed-size array - treat as stack array
                            // Use array_set for direct memory access (stack arrays handled in runtime)
                            self.stmts.push(MirStmt::VoidCall {
                                func: "array_set".to_string(),
                                args: vec![base_id, index_id, rhs_id],
                            });
                        }
                        _ => {
                            // Dynamic or large array - use heap array access
                            self.stmts.push(MirStmt::VoidCall {
                                func: "array_set".to_string(),
                                args: vec![base_id, index_id, rhs_id],
                            });
                        }
                    }
                } else if is_array_param {
                    self.stmts.push(MirStmt::VoidCall {
                        func: "array_set".to_string(),
                        args: vec![base_id, index_id, rhs_id],
                    });
                } else {
                    // Use DictInsert for other types (maps/dicts)
                    // 批次410: the dynamic-receiver write never hashed its key
                    // while the read (`lower_map_key` in the matching DictGet
                    // fall-through below) always did, so `d[k]=v` through an
                    // unannotated parameter inserted under the string HANDLE and
                    // every lookup missed — `len(d)` grew (the key landed), the
                    // value read back 0 silently.
                    let key_id = self.lower_map_key(index_id);
                    self.stmts.push(MirStmt::DictInsert {
                        map_id: base_id,
                        key_id,
                        val_id: rhs_id,
                    });
                }
            } else if let AstNode::FieldAccess { base, field } = lhs {
                // self.field = val → store through the heap struct pointer
                let base_id = self.lower_expr(base);
                self.stmts.push(MirStmt::StructFieldStore {
                    base_id,
                    field: field.clone(),
                    val_id: rhs_id,
                });
            } else if let AstNode::UnaryOp { op, expr } = lhs {
                if op == "*" {
                    // Store through pointer: *ptr = val
                    let addr_id = self.lower_expr(expr);
                    let pointee_width = self.pointee_widths.get(&addr_id).copied().unwrap_or(8);
                    self.stmts.push(MirStmt::Store {
                        addr_id,
                        val_id: rhs_id,
                        pointee_width,
                    });
                } else {
                    // Other unary assignment (unlikely)
                    let lhs_id = self.lower_expr(lhs);
                    self.stmts.push(MirStmt::Assign {
                        lhs: lhs_id,
                        rhs: rhs_id,
                    });
                }
            } else if let AstNode::Var(name) = lhs {
                // Remember the literal's element count for `f(*x)` below.
                if let AstNode::ArrayLit(items) = rhs {
                    self.array_lit_lens.insert(name.clone(), items.len());
                }
                // PY-A: Python-style bare assignment — implicitly declare
                // the variable when it is not already bound (function
                // locals; module-level variables are a later item).
                if let Some(&existing) = self.name_to_id.get(name) {
                    if self.nonlocal_names.contains(name) {
                        // PY-A V3: nonlocal write → env store
                        self.env_store(name, rhs_id);
                        let unit_id = self.next_id();
                        self.exprs.insert(unit_id, MirExpr::IntLit(0));
                        self.type_map.insert(unit_id, Type::Tuple(vec![]));
                        return;
                    }
                    self.stmts.push(MirStmt::Assign {
                        lhs: existing,
                        rhs: rhs_id,
                    });
                    /* Batch 657: the slot's static type must follow a
                       re-assignment on the BigInt edge — `big = 0`
                       pins I64, a later in-loop `big = 1 << 100`
                       stores an i64-width handle that print/arm
                       dispatch then read as a plain int (measured:
                       `print(big)` → 4301332416). Handles need no
                       widening; only the type moves. Retype ONLY on
                       the BigInt↔int edge: widening is always safe,
                       narrowing on a plain-int rhs keeps flip-flop
                       loops coherent — every other kind change keeps
                       its existing dyn-coercion behavior (the 637
                       lesson: propagation faces stay off the table). */
                    let rhs_big = matches!(
                        self.type_map.get(&rhs_id),
                        Some(Type::Named(n, _)) if n == "BigInt"
                    );
                    let cur_big = matches!(
                        self.type_map.get(&existing),
                        Some(Type::Named(n, _)) if n == "BigInt"
                    );
                    if rhs_big != cur_big {
                        let new_ty = if rhs_big {
                            Type::Named("BigInt".to_string(), vec![])
                        } else {
                            Type::I64
                        };
                        self.type_map.insert(existing, new_ty);
                    }
                    /* Batch 738: nested-list accumulator edge — `out = []`
                       pins the element I64; a later `out = out + [row]`
                       (row a vec handle) stores handles that read back as
                       pointers unless the slot's ELEMENT widens to the rhs
                       element. Same "slot follows re-assignment" principle
                       as the BigInt edge above: widening an I64 element to
                       a real container type is always safe (the narrow
                       I64 was the degenerate empty-literal guess), and
                       narrowing back keeps flip-flop loops coherent. */
                    if let (
                        Some(Type::DynamicArray(le)),
                        Some(Type::DynamicArray(re)),
                    ) = (
                        self.type_map.get(&existing).cloned(),
                        self.type_map.get(&rhs_id).cloned(),
                    ) {
                        if *le == Type::I64 && *re != Type::I64 {
                            self.type_map.insert(
                                existing,
                                Type::DynamicArray(re.clone()),
                            );
                        }
                    }
                    // Batch 779 (#203③): same principle, scalar face —
                    // `s = 0` pins the slot I64; a later `s = s + 1 / 2`
                    // (true-div → F64) stored f64 BITS into the I64 slot
                    // and `print(s)` showed the bit pattern as a huge
                    // int (measured: 4602678819172646912 = 0.5). Widening
                    // I64 → F64 on a float rhs is Python-correct and
                    // always safe (the I64 was the `= 0` degenerate guess).
                    if matches!(
                        self.type_map.get(&existing).cloned(),
                        Some(Type::I64)
                    )
                        && matches!(
                            self.type_map.get(&rhs_id).cloned(),
                            Some(Type::F64)
                        ) {
                        self.type_map.insert(existing, Type::F64);
                    }
                    // The env mirror for a module-global write is emitted by
                    // `mirror_module_global_writes`, once, over the finished
                    // body (batch 391).
                } else if self.nonlocal_names.contains(name) {
                    // PY-A V3: inner-scope write before any local bind
                    let key_id = self.env_store(name, rhs_id);
                    // rebind the local alias to an env load so later reads
                    // see the fresh value
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
                    let unit_id = self.next_id();
                    self.exprs.insert(unit_id, MirExpr::IntLit(0));
                    self.type_map.insert(unit_id, Type::Tuple(vec![]));
                    return;
                } else if let Some(mangled) = self
                    .symbol_renames
                    .get(name.as_str())
                    .cloned()
                    .filter(|m| *m != **name && self.module_globals.contains(m))
                {
                    // Module-level binding of an imported module: store
                    // through its own global slot.
                    return self.lower_ast(&AstNode::Assign(
                        Box::new(AstNode::Var(mangled)),
                        Box::new(rhs.clone()),
                    ));
                } else {
                    // module-global / plain local: bind normally so the
                    // local slot keeps the rhs's real type (dict/Vec/etc
                    // would be mangled by an env-load I64 alias).
                    let new_id = self.next_id();
                    self.name_to_id.insert(name.clone(), new_id);
                    self.exprs.insert(new_id, MirExpr::Var(new_id));
                    let ty = self.type_map.get(&rhs_id).cloned().unwrap_or_else(Type::slot_fallback);
                    self.type_map.insert(new_id, ty);
                    if self.tuple_slots.contains(&rhs_id) {
                        self.tuple_slots.insert(new_id);
                    }
                    self.stmts.push(MirStmt::Assign {
                        lhs: new_id,
                        rhs: rhs_id,
                    });
                }
            } else {
                let lhs_id = self.lower_expr(lhs);
                self.stmts.push(MirStmt::Assign {
                    lhs: lhs_id,
                    rhs: rhs_id,
                });
            }
            // Batch 768 (值标签延伸格)：格标签随 Assign 跟随——`df = c["df"]`
            // 之后 len(df) 与 len(c["df"]) 同样按格分派（767 只覆盖内联直读形）。
            if let AstNode::Var(name) = lhs {
                if let Some(&sid) = self.name_to_id.get(name.as_str()) {
                    if let Some(t) = self.slot_tags.get(&rhs_id).cloned() {
                        self.slot_tags.insert(sid, t);
                    }
                }
            }
    }

    /// `target op= value`：类变量复合赋值改写、env 名走 env_store、
    /// 普通 Var 名刷新槽型，其余脱糖为 `target = target op value`
    /// （批 868 自 gen.rs 原臂逐字迁入；签名取臂的原样解构类型，
    /// 臂体零适配）。
    pub(super) fn lower_assign_op(
        &mut self,
        op: &String,
        target: &Box<AstNode>,
        value: &Box<AstNode>,
    ) {
            // Batch 572: `Counter.count += 1` — class VARIABLE compound
            // assignment rewrites to the mangled module global, whose
            // Var target below routes reads and writes through the env.
            if let AstNode::FieldAccess { base, field } = &**target {
                if let AstNode::Var(vname) = &**base {
                    let gname = format!("{}__{}", vname, field);
                    if self.type_decls.contains_key(vname.as_str())
                        && self.module_globals.contains(&gname)
                    {
                        let rewritten = AstNode::AssignOp {
                            op: op.clone(),
                            target: Box::new(AstNode::Var(gname)),
                            value: value.clone(),
                        };
                        self.lower_ast(&rewritten);
                        return;
                    }
                }
            }
            // Desugar: target op= value → target = target op value
            let new_rhs = Box::new(AstNode::BinaryOp {
                op: op.clone(),
                left: target.clone(),
                right: value.clone(),
            });
            // A plain `Var` target is handled HERE so the slot's static type
            // can be refreshed from the combined value. `c: set[str] =
            // set(); c |= {"q"}` kept the slot's OLD type (`DynamicArray(I64)`
            // = "unknown"), so `"q" in c` later passed elem_is_str=0 and
            // compared HANDLES — every string counted as absent (measured in
            // `fetch_stocks`'s `fetched_codes`).
            if let AstNode::Var(name) = &**target {
                // A name routed through the env global (module global, `global`,
                // or a lifted `static`) has no local slot to refresh — writing
                // one here left the cell at its initial value while the read
                // path, which always goes to the env, printed that value back.
                if self.nonlocal_names.contains(name) {
                    let rhs_id = self.lower_expr(&new_rhs);
                    self.env_store(name, rhs_id);
                    return;
                }
                let rhs_id = self.lower_expr(&new_rhs);
                let ty = self.type_map.get(&rhs_id).cloned().unwrap_or_else(Type::slot_fallback);
                match self.name_to_id.get(name).copied() {
                    Some(slot) => {
                        self.stmts.push(MirStmt::Assign { lhs: slot, rhs: rhs_id });
                        if !matches!(ty, Type::I64 | Type::PyDynamic) {
                            self.type_map.insert(slot, ty);
                        }
                    }
                    None => {
                        let slot = self.next_id();
                        self.exprs.insert(slot, MirExpr::Var(slot));
                        self.type_map.insert(slot, ty);
                        self.name_to_id.insert(name.clone(), slot);
                        self.stmts.push(MirStmt::Assign { lhs: slot, rhs: rhs_id });
                    }
                }
                // The env cell is the one other functions read (they have no
                // slot for this name), so a `+=` that stops at the slot is
                // discarded on the next call — `total += 5` twice printed `5`
                // both times (batch 385). `mirror_module_global_writes` now
                // emits that mirror for every write to the slot, `=` and `+=`
                // alike, so the rule has one home instead of one per
                // statement kind.
                return;
            }
            let assign = AstNode::Assign(target.clone(), new_rhs);
            self.lower_ast(&assign);
    }


    /// 赋值表达式位（`a = b` 出现在表达式上下文——批 886 原臂逐字迁入）。
    pub(super) fn lower_assign_expr(&mut self, lhs: &AstNode, rhs: &AstNode) -> u32 {
            // Batch 572: `Counter.count = x` — class VARIABLE write
            // rewrites to the mangled module global.
            if let AstNode::FieldAccess { base, field } = lhs {
                if let AstNode::Var(vname) = &**base {
                    let gname = format!("{}__{}", vname, field);
                    if self.type_decls.contains_key(vname.as_str())
                        && self.module_globals.contains(&gname)
                    {
                        let rewritten = AstNode::Assign(
                            Box::new(AstNode::Var(gname)),
                            Box::new(rhs.clone()),
                        );
                        self.lower_ast(&rewritten);
                        return self.i64_zero_id();
                    }
                }
            }
            // PY-A: walrus `name := expr` in expression position — lower
            // rhs, bind the name (implicit decl or rebinding), and the
            // expression value is the assigned value.
            if let AstNode::Var(name) = lhs {
                let rhs_id = self.lower_expr(rhs);
                let dest = match self.name_to_id.get(name).copied() {
                    None => {
                        let new_id = self.next_id();
                        self.exprs.insert(new_id, MirExpr::Var(new_id));
                        let ty = self.type_map.get(&rhs_id).cloned().unwrap_or_else(Type::slot_fallback);
                        self.type_map.insert(new_id, ty);
                        self.name_to_id.insert(name.clone(), new_id);
                        self.stmts.push(MirStmt::Assign {
                            lhs: new_id,
                            rhs: rhs_id,
                        });
                        new_id
                    }
                    // PY-A: the name is already bound — the store was simply
                    // missing, so rebinding (`i := i + 1`, and now an
                    // assignment arm `_ => i = 5`) left the slot untouched and
                    // read back its old value. Measured: `match 1 { _ =>
                    // (i := i + 1) }` exited 0 with `i` still 0.
                    Some(slot) => {
                        self.stmts.push(MirStmt::Assign { lhs: slot, rhs: rhs_id });
                        slot
                    }
                };
                // The value is the SLOT, not `rhs_id`: an expression id can be
                // consumed more than once downstream, and re-emitting it re-runs
                // its computation — `d = (m := m + 3)` with `m = 4` stored 7 into
                // `m` and then read 10 into `d`.
                return dest;
            }
            // Non-var lhs: statement assign with a 0-value expression
            self.lower_ast(&AstNode::Assign(Box::new(lhs.clone()), Box::new(rhs.clone())));
            let z = self.next_id();
            self.exprs.insert(z, MirExpr::IntLit(0));
            self.type_map.insert(z, Type::I64);
            return z;
    }

}