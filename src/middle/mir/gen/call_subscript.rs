//! 批次 845：下标读发射体（Subscript 臂原臂逐字迁入）。
//! 路由：loc/iloc 重写、PyJson tag、Vec/Array array_get、zeta_dyn_getitem
//! 几何判形、DictGet 兜底——合同见各段批注（295/452/767/808/815）。

use super::MirGen;
use crate::frontend::ast::{AstNode};
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::{ArraySize, Type};

impl MirGen {
    /// 下标读发射体。`dest`＝结果槽。
    pub(super) fn lower_subscript(
        &mut self,
        base: &Box<AstNode>,
        index: &Box<AstNode>,
        dest: u32,
    ) {
            // `df.loc[<掩码>]` / `df.iloc[<掩码>]`: `.loc` is a PROPERTY that
            // then gets subscripted — the runtime has no property objects, so
            // rewrite the pair into a plain METHOD call `df.loc(<掩码>)`. Without
            // this the subscript became a map_get with a BOOLEAN key, returned
            // 0 and `…copy()` dereferenced it.
            if let AstNode::FieldAccess { base: recv, field } = &**base {
                if field == "loc" || field == "iloc" {
                    let recv_id = self.lower_expr(recv);
                    let arg_id = self.lower_expr(index);
                    let recv_ty = self.type_map.get(&recv_id).cloned();
                    let func = match recv_ty.as_ref() {
                        Some(Type::Named(n, _)) => format!("{}::{}", n, field),
                        _ => format!("DataFrame::{}", field),
                    };
                    self.stmts.push(MirStmt::Call {
                        func,
                        args: vec![recv_id, arg_id],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    let ty = recv_ty.unwrap_or(Type::Named("DataFrame".to_string(), vec![]));
                    self.type_map.insert(dest, ty);
                    return;
                }
            }
            // 批次148 重放: `np.where(mask)[0]` — numpy 的 where 返回元组，
            // [0] 取第一个数组。V1：where 调用已返回索引 Vec，[0] 即其本身
            // （否则 array_get 取出首索引，随后的 len(idx) 对标量求长度 SEGV）。
            if let AstNode::Call { receiver, method, .. } = &**base {
                if method == "where"
                    && matches!(**index, AstNode::Lit(0))
                    && receiver.as_ref().map_or(false, |r| matches!(**r, AstNode::Var(_)))
                {
                    self.lower_expr(base);
                    return;
                }
            }
            let bid = self.lower_expr(base);
            let base_ty_pre = self.type_map.get(&bid).cloned().unwrap_or_else(Type::slot_fallback);
            // PY-A: negative index `arr[-k]` → `arr[n-k]` for arrays with a
            // compile-time-known size (Python semantics).
            // A DynamicArray base needs the count at RUNTIME: `arr[-k]` →
            // `arr[vec_len(arr) - k]`. Previously only compile-time-sized
            // arrays were folded, so `data[-1]` on a list ran `array_get(-1)`
            // and read out of bounds (t24 printed 4 instead of 40).
            let mut negative_dyn_index: Option<u32> = None;
            let index: Box<AstNode> = match (&**index, &base_ty_pre) {
                (
                    AstNode::UnaryOp { op, expr },
                    Type::Array(_, ArraySize::Literal(n)),
                ) if op == "-" => match &**expr {
                    AstNode::Lit(k) if (*k as i64) <= *n as i64 && *k > 0 => {
                        Box::new(AstNode::Lit((*n as i64 - k) as i64))
                    }
                    _ => index.clone(),
                },
                (AstNode::UnaryOp { op, expr }, Type::DynamicArray(_)) if op == "-" => {
                    match &**expr {
                        AstNode::Lit(k) if *k > 0 => {
                            let len_id = self.emit_call("vec_len", vec![bid], Type::I64);
                            let k_id = self.next_id();
                            self.exprs.insert(k_id, MirExpr::IntLit(*k));
                            self.type_map.insert(k_id, Type::I64);
                            let idx = self.next_id();
                            self.exprs.insert(
                                idx,
                                MirExpr::BinaryOp {
                                    op: "-".to_string(),
                                    left: len_id,
                                    right: k_id,
                                },
                            );
                            self.type_map.insert(idx, Type::I64);
                            negative_dyn_index = Some(idx);
                            index.clone()
                        }
                        _ => index.clone(),
                    }
                }
                _ => index.clone(),
            };
            // PY-A: comma subscript `a[i, j]` (pandas `.iloc[r, c]` /
            // `.loc[r, c]`). There is no DataFrame in this compiler, so the
            // receiver is an opaque platform handle. Lower it through the
            // platform shim instead of pretending a 2-D lookup happened —
            // without this the parser produced `a = x` for `a = x[0,0]`
            // (the trailing `[0,0]` parsed as a stray array literal).
            if let AstNode::Tuple(items) = &*index {
                let mut call_args = vec![bid];
                for item in items {
                    call_args.push(self.lower_multi_index_element(item));
                }
                self.stmts.push(MirStmt::Call {
                    func: "py_getitem2".to_string(),
                    args: call_args,
                    dest: dest,
                    type_args: vec![],
                });
                self.exprs.insert(dest, MirExpr::Var(dest));
                self.type_map.insert(dest, Type::I64);
                return;
            }
            let iid = match negative_dyn_index {
                Some(pre) => pre,
                None => self.lower_expr(&index),
            };
            // PY-A (任务 #53): the arms above match the index by **AST shape**
            // (`UnaryOp{-, Lit}`), so they only catch a literal minus. `l[0 - 1]`
            // is a runtime `-` in MIR (CTFE does not fold it) and `l[k]` is a
            // Var, so both reached `array_get` with a signed index and read
            // before the array header — wrong value, or SIGSEGV when the
            // element is itself a container handle. Close the rest by **value**.
            let iid = if negative_dyn_index.is_some() {
                iid
            } else {
                self.normalize_subscript_index(bid, &base_ty_pre, &index, iid)
            };

            // `t[i]` on a TUPLE (a StackArray literal): index the stack array.
            // Without this the subscript fell into the DICT branch (DictGet on a
            // stack-array pointer) and `("a",)[0]` read 0.
            {
                // Only INLINE tuple literals (`(a, b)[0]`): a variable that is
                // merely TYPED Tuple may actually hold a map/dict (a
                // dict-comprehension's result leaks a Tuple annotation), and
                // indexing that with stack_array_get read the map header.
                // `is_tuple` covers tuple LITERALS; a function that RETURNS a
                // tuple comes back as `Named("tuple", …)`, so accept that too
                // (the value is a stack array at runtime either way).
                let is_tuple = self.tuple_slots.contains(&bid)
                    || self.pair_slots.contains(&bid)
                    || matches!(self.type_map.get(&bid), Some(Type::Named(n, _)) if n == "tuple")
                    || matches!(
                        self.type_map.get(&bid),
                        Some(Type::DynamicArray(inner)) | Some(Type::Array(inner, _))
                            if matches!(**inner, Type::Tuple(_))
                    );
                let tuple_base = self.type_map.get(&bid).cloned();
                if is_tuple {
                // list-of-pairs base (`list(enumerate(xs))[0]`): the vec
                // holds pair HANDLES, stack_array_get returns one; the
                // element carries the whole pair type so `e[0][1]` chains.
                if let Some(Type::DynamicArray(inner))
                | Some(Type::Array(inner, _)) = &tuple_base {
                    if let Type::Tuple(ts) = &**inner {
                        self.stmts.push(MirStmt::Call {
                            func: "stack_array_get".to_string(),
                            args: vec![bid, iid],
                            dest: dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map
                            .insert(dest, Type::Tuple(ts.clone()));
                        self.pair_slots.insert(dest);
                        return;
                    }
                }
                if let Some(Type::Tuple(ts)) = tuple_base.clone() {
                    self.stmts.push(MirStmt::Call {
                        func: "stack_array_get".to_string(),
                        args: vec![bid, iid],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    let elem = match &*index {
                        AstNode::Lit(k) => ts.get(*k as usize).cloned().unwrap_or_else(Type::slot_fallback),
                        _ => Type::I64,
                    };
                    self.type_map.insert(dest, elem);
                    return;
                }
                if matches!(tuple_base, Some(Type::Named(ref n, _)) if n == "tuple") {
                    self.stmts.push(MirStmt::Call {
                        func: "stack_array_get".to_string(),
                        args: vec![bid, iid],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    // Element type from the annotation (`partition(...)`
                    // hands back Named("tuple", [Str, Str, Str]) — typing
                    // the element I64 made `p[0].startswith(...)`
                    // dispatch on an integer and print a raw pointer).
                    let elem = match (&tuple_base, &*index) {
                        (Some(Type::Named(_, ts)), AstNode::Lit(k)) => ts
                            .get(*k as usize)
                            .cloned()
                            .unwrap_or(Type::I64),
                        _ => Type::I64,
                    };
                    self.type_map.insert(dest, elem);
                    return;
                }
                }
            }
            // Check if base is an array type (dynamic or static)
            let base_ty = self.type_map.get(&bid).cloned().unwrap_or_else(Type::slot_fallback);
            // 批次 796（#266 RUN 面）：Rust 方言形参标注 `xs: Vec<T>` 定型为
            // Named("Vec", [T])——下面整条下标链（DynamicArray/Array/
            // array_param/I64|PyDynamic 判别）没有这个形的臂，落进 dict
            // 兜底发 map_str_key+DictGet：selfhost build_ast 的 `tokens[i]`
            // 实拍运行时 map_get 见非 dict 形抛 code=1、零输出（stub:422 守卫；
            // --jit/AOT 同形）。归一化成 DynamicArray 走既有 array_get 臂；
            // 元素型解析不出时按 I64（与该链尾部约定一致）。
            let base_ty = match &base_ty {
                Type::Named(n, targs) if n == "Vec" => {
                    Type::DynamicArray(Box::new(
                        targs.first().cloned().unwrap_or_else(Type::slot_fallback),
                    ))
                }
                _ => base_ty.clone(),
            };
            let base_ty_clone = base_ty.clone(); // clone for later elem-type lookup
            // Also check source_types for function params with array types
            let source_ty = self.source_types.get(&bid).cloned().unwrap_or_default();
            let is_array_param = source_ty.starts_with("[") || source_ty.starts_with("*mut [");
            if let Type::Named(n, _) = &base_ty {
                if n == "PyJson" {
                    // `cfg["k"]` / `arr[0]`: the runtime dispatches on the
                    // value's tag (object vs array).
                    self.stmts.push(MirStmt::Call {
                        func: "py_json_get".to_string(),
                        args: vec![bid, iid],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::Named("PyJson".to_string(), vec![]));
                    return;
                }
            }
            if let Type::Str = base_ty {
                // Python `s[i]` on a string yields a 1-character string.
                self.emit_call_into(dest, "str_get", vec![bid, iid], Type::Str);
                return;
            }
            // 批次145 重放: subscript on a KNOWN struct with `__getitem__`
            // (`df["c"]`) must dispatch the qualified method — previously it
            // fell through to DictGet on the struct pointer (map_get on a
            // non-map → garbage/SEGV). Batch 99's fix, lost in the 143
            // restore.
            if let Type::Named(tn, _) = &base_ty {
                // `df[<boolean mask>]` — pandas ROW FILTERING, not a column
                // lookup. `__getitem__` assumes a column name
                // (`self.data[map_str_key(key)]`), so a mask went into the map
                // lookup: garbage key / empty frame (measured: `a[m]` gave
                // `0 0`, and `fetch_stocks`'s
                // `cached[(cached["trade_date"] >= eff_start) & (…)]` crashed
                // inside `map_str_key`).
                // Batch 398: `DynamicArray(Bool)` IS the mask. The I64 arm is
                // the legacy fallback for masks that crossed a
                // `lt(vec, i64)`-annotated boundary.
                let mask_like = matches!(
                    self.type_map.get(&iid),
                    Some(Type::DynamicArray(e))
                        if matches!(**e, Type::Bool | Type::I64)
                );
                if mask_like && tn.contains("DataFrame") {
                    let target = self
                        .qualified_method_candidate(tn, "loc")
                        .unwrap_or_else(|| "DataFrame::loc".to_string());
                    let ret_ty = self.func_ret_types.get(&target).cloned();
                    self.stmts.push(MirStmt::Call {
                        func: target,
                        args: vec![bid, iid],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map.insert(
                        dest,
                        ret_ty.unwrap_or_else(|| {
                            Type::Named("DataFrame".to_string(), vec![])
                        }),
                    );
                    return;
                }
                // `df[["a", "b"]]` — LIST-LIKE key = COLUMN SUBSET (same
                // element-type reading as the `__getitem__` dispatch site;
                // batch 398).
                if tn.contains("DataFrame")
                    && matches!(
                        self.type_map.get(&iid),
                        Some(Type::DynamicArray(e)) if matches!(**e, Type::Str)
                    )
                {
                    let target = self
                        .qualified_method_candidate(tn, "select_columns")
                        .unwrap_or_else(|| "DataFrame::select_columns".to_string());
                    let ret_ty = self.func_ret_types.get(&target).cloned();
                    self.stmts.push(MirStmt::Call {
                        func: target,
                        args: vec![bid, iid],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map.insert(
                        dest,
                        ret_ty.unwrap_or_else(|| {
                            Type::Named("DataFrame".to_string(), vec![])
                        }),
                    );
                    return;
                }
                if tn != "map" && tn != "dict" {
                    if let Some(qualified) =
                        self.qualified_method_candidate(tn, "__getitem__")
                    {
                        let ret_ty = self.func_ret_types.get(&qualified).cloned();
                        self.stmts.push(MirStmt::Call {
                            func: qualified,
                            args: vec![bid, iid],
                            dest: dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map.insert(
                            dest,
                            ret_ty.unwrap_or(Type::I64),
                        );
                        return;
                    }
                }
            }
            // A MAP subscript as an EXPRESSION. The assignment path
            // (`d[k] = v`) already handled maps, but the expression path did
            // not: `self.d["a"]` fell through to the array branches and read
            // the map handle as an array — a SEGFAULT, not a wrong value.
            if base_ty.is_map() {
                let key_id = self.lower_map_key_typed(iid, Some(&base_ty));
                // The base must live in a LOCAL slot: codegen's DictGet does
                // `load_local(map_id)`, and a field read (or any non-local
                // expression) has no alloca — `self.d["a"]` SEGFAULTED.
                // Materialize it first.
                let map_slot = self.next_id();
                self.stmts.push(MirStmt::Assign {
                    lhs: map_slot,
                    rhs: bid,
                });
                self.exprs.insert(map_slot, MirExpr::Var(map_slot));
                self.type_map.insert(map_slot, base_ty.clone());
                self.stmts.push(MirStmt::DictGet {
                    map_id: map_slot,
                    key_id,
                    dest: dest,
                });
                self.exprs.insert(dest, MirExpr::Var(dest));
                // `m[k]` keeps the map's VALUE type (second type argument of
                // `map<K, V>`), so a Vec-valued map stays indexable.
                let val_ty = match &base_ty {
                    Type::Named(_, targs) => {
                        targs.get(1).cloned().unwrap_or_else(Type::slot_fallback)
                    }
                    _ => Type::I64,
                };
                self.type_map.insert(dest, val_ty.clone());
                // Batch 767 (值标签大弧·读侧)：Any 值槽 ⇒ 读格标签备用。
                // 766 写侧登记的类实例 tag（≥CLASS_TAG_BASE）由 len() 等消费，
                // 运行期按格分派，静态槽型保持 PyDynamic（异构字典安全）。
                if matches!(val_ty, Type::PyDynamic) {
                    let tag_slot = self.emit_call("zeta_map_value_tag", vec![map_slot, key_id], Type::I64);
                    self.slot_tags.insert(dest, tag_slot);
                }
                return;
            }
            if let Type::DynamicArray(_) = base_ty {
                // Generate array_get call for dynamic arrays
                self.stmts.push(MirStmt::Call {
                    func: "array_get".to_string(),
                    args: vec![bid, iid],
                    dest: dest,
                    type_args: vec![],
                });
            } else if let Type::Array(_, size) = base_ty {
                // Check if this is a stack array (fixed size) or heap array
                match size {
                    ArraySize::Literal(n) if n <= 1024 => {
                        // Small fixed-size array - treat as stack array
                        // Use array_get for direct memory access (stack arrays handled in runtime)
                        self.stmts.push(MirStmt::Call {
                            func: "array_get".to_string(),
                            args: vec![bid, iid],
                            dest: dest,
                            type_args: vec![],
                        });
                    }
                    _ => {
                        // Dynamic or large array - use heap array access
                        self.stmts.push(MirStmt::Call {
                            func: "array_get".to_string(),
                            args: vec![bid, iid],
                            dest: dest,
                            type_args: vec![],
                        });
                    }
                }
            } else if is_array_param {
                // Param is an array type even if type_map doesn't know it yet
                self.stmts.push(MirStmt::Call {
                    func: "array_get".to_string(),
                    args: vec![bid, iid],
                    dest: dest,
                    type_args: vec![],
                });
            } else if matches!(base_ty, Type::I64 | Type::PyDynamic)
                && source_ty != "map"
                && !matches!(self.type_map.get(&iid), Some(Type::Str))
            {
                // BATCH-295: an UNKNOWN receiver with a non-string key. The
                // old fall-through always emitted `map_get`, so `t[i]` on a
                // dyn-typed LIST handle walked a fake bucket chain and
                // SEGFAULTED (measured in `_run_local`'s enumerate loop). Let
                // the runtime discriminate Vec-vs-map by the GC header.
                self.stmts.push(MirStmt::Call {
                    func: "zeta_dyn_getitem".to_string(),
                    args: vec![bid, iid],
                    dest: dest,
                    type_args: vec![],
                });
            } else {
                // Use DictGet for other types (maps/dicts) — string keys
                // are content-hashed (see lower_map_key)
                let key_id = self.lower_map_key(iid);
                self.stmts.push(MirStmt::DictGet {
                    map_id: bid,
                    key_id,
                    dest: dest,
                });
            }
            self.exprs.insert(dest, MirExpr::Var(dest));
            // Element type: from the base array's element type if known,
            // otherwise default to i64. Without this, `f64arr[i]` is typed
            // i64 and later casts read raw double bits.
            // Check type_map first (covers annotated locals like `let x: [f64; 4]`),
            // fall back to source_types (function params).
            let elem_type = {
                let from_ty = match &base_ty_clone {
                    Type::DynamicArray(elem) => Some((**elem).clone()),
                    Type::Array(elem, _) => Some((**elem).clone()),
                    // `df["close"]` on a `map<Str, vecstr>`-typed DataFrame:
                    // the dict path (DictGet) must keep the VALUE type, else
                    // the result is I64 and every column method call fell to a
                    // bare ghost (`notna_1`, `[dynamic]str__isna`, …).
                    Type::Named(n, targs) if n == "map" || n == "dict" || n == "dict_like" => {
                        targs.get(1).cloned()
                    }
                    _ => None,
                };
                if let Some(et) = from_ty {
                    et
                } else {
                    let src = self.source_types.get(&bid).cloned().unwrap_or_default();
                    if src.starts_with('[') {
                        let inner = src
                            .trim_start_matches('[')
                            .split(']')
                            .next()
                            .unwrap_or("");
                        let elem_str = inner
                            .split(';')
                            .next()
                            .unwrap_or("")
                            .trim();
                        Type::from_string(elem_str)
                    } else {
                        Type::I64
                    }
                }
            };
            self.type_map.insert(dest, elem_type);
        
    }
}
