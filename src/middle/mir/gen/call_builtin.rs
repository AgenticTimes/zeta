//! 批次 841：无接收者内建函数族第一片（map/filter/chr/ord/divmod/dict）。
//! if 串联臂逐字迁入；zip/any/all/enumerate/list/float/sorted 表段在
//! gen.rs 原位（批 842 续迁）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::{ArraySize, Type};

impl MirGen {
    /// 无接收者内建族第一片执行者。命中返回 Some(dest)；None 落链。
    pub(super) fn lower_builtin_1(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
                    // PY-A: map(f, xs) / filter(f, xs) — now that a bare function
                    // name is a FuncAddr this can call back into it. Eager (V1):
                    // the result is a Vec, not an iterator.
                    if receiver.is_none()
                        && (method == "map" || method == "filter")
                        && args.len() == 2
                    {
                        let f = self.lower_expr(&args[0]);
                        let xs = self.lower_expr(&args[1]);
                        let func = if method == "map" {
                            "py_builtin_map"
                        } else {
                            "py_builtin_filter"
                        };
                        self.stmts.push(MirStmt::Call {
                            func: func.to_string(),
                            args: vec![f, xs],
                            dest: dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map
                            .insert(dest, Type::DynamicArray(Box::new(Type::I64)));
                        return Some(dest);
                    }
                    // PY-A: chr(n) / ord(s) / divmod(a, b) / dict() — previously
                    // bare externs (link failure) or missing entirely.
                    if receiver.is_none() && method == "chr" && args.len() == 1 {
                        let a = self.lower_expr(&args[0]);
                        self.emit_call_into(dest, "py_builtin_chr", vec![a], Type::Str);
                        return Some(dest);
                    }
                    if receiver.is_none() && method == "ord" && args.len() == 1 {
                        let a = self.lower_expr(&args[0]);
                        self.emit_call_into(dest, "py_builtin_ord", vec![a], Type::I64);
                        return Some(dest);
                    }
                    // divmod(a, b) → (a // b, a % b) as a tuple, so
                    // `q, r = divmod(a, b)` unpacks via the call-return path.
                    // PY-A: the quotient must be FLOOR division — with `/` here the
                    // true-division fix would have made `divmod(7, 2)` answer 3.5.
                    if receiver.is_none() && method == "divmod" && args.len() == 2 {
                        let q = AstNode::BinaryOp {
                            op: "floordiv".to_string(),
                            left: Box::new(args[0].clone()),
                            right: Box::new(args[1].clone()),
                        };
                        let r = AstNode::BinaryOp {
                            op: "%".to_string(),
                            left: Box::new(args[0].clone()),
                            right: Box::new(args[1].clone()),
                        };
                        return Some(self.lower_expr(&AstNode::Tuple(vec![q, r])));
                    }
                    // dict() with no arguments is an empty map.
                    if receiver.is_none() && method == "dict" && args.is_empty() {
                        return Some(self.lower_expr(&AstNode::DictLit { entries: vec![] }));
                    }
                    // `dict(m)` is a SHALLOW COPY in Python, not an alias: mutating
                    // the result must not touch the source. Only done when the
                    // argument is statically a map — an unknown argument keeps the
                    // loud diagnostic (copying an unknown handle would corrupt data).
                    if receiver.is_none() && method == "dict" && args.len() == 1 {
                        let src_id = self.lower_expr(&args[0]);
                        let is_map = matches!(
                            self.type_map.get(&src_id),
                            Some(t) if t.is_map()
                        );
                        if is_map {
                            let fresh = self.next_id();
                            self.stmts.push(MirStmt::MapNew { dest: fresh });
                            self.exprs.insert(fresh, MirExpr::Var(fresh));
                            self.type_map
                                .insert(fresh, Type::Named("map".to_string(), vec![]));
                            // The VALUE of `dict(m)` is the new map, not
                            // py_map_update's return (which is 0 — using it as the
                            // dest made `dict(d)` an empty map).
                            let sink = self.emit_call("py_map_update", vec![fresh, src_id], Type::I64);
                            self.exprs.insert(dest, MirExpr::Var(fresh));
                            self.type_map.insert(dest, Type::Named("map".to_string(), vec![]));
                            return Some(dest);
                        } else {
                            // Dynamic/unknown argument: emit `map__copy(src)` which
                            // works on any handle (creates new map + copies entries).
                            // This fixes t231 where `dict(x)` with `x: PyDynamic`
                            // fell through to ghost `dict_1`.
                            self.emit_call_into(dest, "map__copy", vec![src_id], Type::Named("map".to_string(), vec![]));
                            return Some(dest);
                        }
                        /* Batch 661 (cleanup) reconciliation: the dynamic route
                           above is mainline 658's `map__copy` — functionally
                           equivalent to this lane's `py_dict_ctor` (both funnel
                           non-dict handles through map_resolve's loud guard =
                           TypeError parity), so ONE route is kept. The runtime
                           `py_dict_ctor` stays as unwired infrastructure. */
                    }
        None
    }

    /// 批次 842：内建族第二片执行者（zip/any/all/enumerate/list/int/
    /// float/sorted 表段）。命中返回 Some(dest)；None 落链。
    pub(super) fn lower_builtin_2(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
        let _ = dest;
            // `for x, y in zip(a, b):` destructures. (Previously a bare
            // `zip` extern → link failure.)
            if receiver.is_none() && method == "zip" && args.len() == 2 {
                let a = self.lower_expr(&args[0]);
                let b = self.lower_expr(&args[1]);
                // 批次 557: per-position element types from the operands
                // (zip(["x","y"], [10,20]) pairs are (Str, I64) — the old
                // hardcoded (I64, I64) made every destructured name print
                // its pointer).
                let ta = match self.type_map.get(&a).cloned() {
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => *e,
                    Some(Type::Str) => Type::Str,
                    _ => Type::I64,
                };
                let tb = match self.type_map.get(&b).cloned() {
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => *e,
                    Some(Type::Str) => Type::Str,
                    _ => Type::I64,
                };
                self.stmts.push(MirStmt::Call {
                    func: "py_zip".to_string(),
                    args: vec![a, b],
                    dest: dest,
                    type_args: vec![],
                });
                self.exprs.insert(dest, MirExpr::Var(dest));
                self.type_map.insert(
                    dest,
                    Type::DynamicArray(Box::new(Type::Tuple(vec![ta, tb]))),
                );
                return Some(dest);
            }
            // PY-A: `any(xs)` / `all(xs)` over an array — Python truthiness
            // is non-zero. Previously these emitted bare `any`/`all`
            // externs and failed to link.
            if receiver.is_none()
                && (method == "any" || method == "all")
                && args.len() == 1
            {
                let arg_id = self.lower_expr(&args[0]);
                let func = if method == "any" {
                    "py_builtin_any"
                } else {
                    "py_builtin_all"
                };
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: vec![arg_id],
                    dest: dest,
                    type_args: vec![],
                });
                self.exprs.insert(dest, MirExpr::Var(dest));
                self.type_map.insert(dest, Type::Bool);
                return Some(dest);
            }
            // PY-A: f-string format spec — `__fmtspec__(value, spec)` →
            // runtime snprintf with the user spec (V1: f64 uses it, i64/str
            // fall back to plain conversion)
            if method == "__fmtspec__" && receiver.is_none() && args.len() == 2 {
                let val_id = self.lower_expr(&args[0]);
                let spec_id = self.lower_expr(&args[1]);
                // One formatter per value kind, so the value keeps its ABI
                // (a shared i64 entry point would fptosi the f64).
                let func = match self.type_map.get(&val_id).cloned() {
                    Some(Type::F64) | Some(Type::F32) => "py_fmt_f64",
                    Some(Type::Str) => "py_fmt_str",
                    // 批次 748（#45 第十成员）：bool 档缺失时落 i64 打印器，
                    // `format!("{}", true)` 打 1 而不是 True（291 契约格）。
                    Some(Type::Bool) => "py_fmt_bool",
                    _ => "py_fmt_i64",
                };
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: vec![val_id, spec_id],
                    dest: dest,
                    type_args: vec![],
                });
                self.exprs.insert(dest, MirExpr::Var(dest));
                self.type_map.insert(dest, Type::Str);
                return Some(dest);
            }

            // PY-A: Python builtins list/int/float/sorted + numpy subset
            // (arange/linspace/diff as free calls) — dispatch by type.
            if receiver.is_none() && args.len() <= 3 {
                let argc = args.len();
                // 批次 557: `enumerate(xs[, start])` as a VALUE — the for
                // desugar covered the loop spelling only, so
                // `list(enumerate(xs, start=1))` built an EMPTY list (the
                // `start=1` kwarg dropped to a positional start, and no
                // runtime function existed). Pairs carry [2,2] headers so
                // len()/subscript/destructure all see the same shape.
                if method == "enumerate" && (argc == 1 || argc == 2) {
                    let coll_id = self.lower_expr(&args[0]);
                    let start_id = if argc == 2 {
                        self.lower_expr(&args[1])
                    } else {
                        self.next_id_with_lit(0)
                    };
                    let elem_ty = match self.type_map.get(&coll_id).cloned() {
                        Some(Type::DynamicArray(e)) => *e,
                        Some(Type::Str) => Type::Str,
                        _ => Type::I64,
                    };
                    let pid = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "py_enumerate".to_string(),
                        args: vec![coll_id, start_id],
                        dest: pid,
                        type_args: vec![],
                    });
                    self.exprs.insert(pid, MirExpr::Var(pid));
                    self.type_map.insert(
                        pid,
                        Type::DynamicArray(Box::new(Type::Tuple(vec![
                            Type::I64,
                            elem_ty,
                        ]))),
                    );
                    return Some(pid);
                }
                let lowered_args: Option<Vec<u32>> = match method {
                    // 批次 559: pow 按操作数类型分派——整型幂走
                    // zeta_pow_i64（否则裸外名 pow 链到 libc 的 double
                    // 签名、i64 读回垃圾）；含 F64 操作数走 libm
                    // py_math_pow；dyn 参数保留原路（已知角）。
                    "pow" if argc == 2 => {
                        let a = self.lower_expr(&args[0]);
                        let b = self.lower_expr(&args[1]);
                        let float_args = matches!(
                            self.type_map.get(&a).cloned(),
                            Some(Type::F64) | Some(Type::F32)
                        ) || matches!(
                            self.type_map.get(&b).cloned(),
                            Some(Type::F64) | Some(Type::F32)
                        ) || matches!(self.exprs.get(&b), Some(MirExpr::FloatLit(_)))
                            || matches!(self.exprs.get(&a), Some(MirExpr::FloatLit(_)));
                        // dyn operands also take the int pow — the
                        // previous fallback (libc pow with i64 args read
                        // as doubles) was garbage in every case.
                        let (f, ty) = if float_args {
                            ("py_math_pow", Type::F64)
                        } else {
                            ("zeta_pow_i64", Type::I64)
                        };
                        let nid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: f.to_string(),
                            args: vec![a, b],
                            dest: nid,
                            type_args: vec![],
                        });
                        self.exprs.insert(nid, MirExpr::Var(nid));
                        self.type_map.insert(nid, ty.clone());
                        self.exprs.insert(dest, MirExpr::Var(nid));
                        self.type_map.insert(dest, ty);
                        return Some(dest);
                    }
                    "list" if argc == 1 => Some(vec![self.lower_expr(&args[0])]),
                    "int" if argc == 1 => {
                        // This used to compute the right conversion
                        // function and then return the ARGUMENT unchanged
                        // (f was never emitted), so int(x) only worked
                        // where the value already was an i64.
                        let a = self.lower_expr(&args[0]);
                        let f = match self.type_map.get(&a).cloned() {
                            Some(Type::Str) => "zeta_int_str",
                            Some(Type::F64) | Some(Type::F32) => "zeta_int_f64",
                            Some(Type::Named(n, _)) if n == "PyJson" => "py_json_as_i64",
                            _ => "zeta_int_i64",
                        };
                        let nid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: f.to_string(),
                            args: vec![a],
                            dest: nid,
                            type_args: vec![],
                        });
                        self.exprs.insert(nid, MirExpr::Var(nid));
                        self.type_map.insert(nid, Type::I64);
                        Some(vec![nid])
                    }
                    "float" if argc == 1 => {
                        let a = self.lower_expr(&args[0]);
                        let f = match self.type_map.get(&a).cloned() {
                            Some(Type::F64) | Some(Type::F32) => "zeta_float_f64",
                            Some(Type::Named(n, _)) if n == "PyJson" => "py_json_as_f64",
                            _ => "zeta_float_i64",
                        };
                        let nid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: f.to_string(),
                            args: vec![a],
                            dest: nid,
                            type_args: vec![],
                        });
                        self.exprs.insert(nid, MirExpr::Var(nid));
                        self.type_map.insert(nid, Type::F64);
                        self.exprs.insert(dest, MirExpr::Var(nid));
                        self.type_map.insert(dest, Type::F64);
                        return Some(dest);
                    }
                    // open(path[, mode]) -> file handle. Python's mode
                    // defaults to "r"; the two-argument form passes the
                    // mode string through.
                    "open" if argc == 1 || argc == 2 => {
                        let p = self.lower_expr(&args[0]);
                        let m = if argc == 2 {
                            self.lower_expr(&args[1])
                        } else {
                            let mid = self.next_id();
                            self.exprs
                                .insert(mid, MirExpr::StringLit("r".to_string()));
                            self.type_map.insert(mid, Type::Str);
                            mid
                        };
                        let nid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "py_file_open".to_string(),
                            args: vec![p, m],
                            dest: nid,
                            type_args: vec![],
                        });
                        self.exprs.insert(nid, MirExpr::Var(nid));
                        self.type_map
                            .insert(nid, Type::Named("PyFile".to_string(), vec![]));
                        self.exprs.insert(dest, MirExpr::Var(nid));
                        self.type_map
                            .insert(dest, Type::Named("PyFile".to_string(), vec![]));
                        return Some(dest);
                    }
                    "sorted" if argc == 1 => {
                        let a = self.lower_expr(&args[0]);
                        // len: from type if known, else pass -1 (Vec header)
                        let len_id = match self.type_map.get(&a).cloned() {
                            Some(Type::Array(_, ArraySize::Literal(n))) => {
                                let nid = self.next_id();
                                self.exprs.insert(nid, MirExpr::IntLit(n as i64));
                                self.type_map.insert(nid, Type::I64);
                                nid
                            }
                            _ => {
                                let nid = self.next_id();
                                self.exprs.insert(nid, MirExpr::IntLit(-1));
                                self.type_map.insert(nid, Type::I64);
                                nid
                            }
                        };
                        Some(vec![a, len_id])
                    }
                    // PY-A: sorted(xs[, key=f][, reverse=B]) — keyword
                    // arguments are matched BY NAME (their source order is
                    // not guaranteed). Without a key this is the plain
                    // sort-then-reverse path; with one it goes through the
                    // decorate-sort-undecorate runtime.
                    "sorted" if argc == 2 || argc == 3 => {
                        let mut keyf: Option<AstNode> = None;
                        let mut rev: Option<AstNode> = None;
                        let mut understood = true;
                        for a in args.iter().skip(1) {
                            match a {
                                AstNode::Call {
                                    receiver: None,
                                    method,
                                    args: ka,
                                    ..
                                } if method == "__kwarg__" && ka.len() == 2 => {
                                    if let AstNode::StringLit(n) = &ka[0] {
                                        match n.as_str() {
                                            "key" => keyf = Some(ka[1].clone()),
                                            "reverse" => rev = Some(ka[1].clone()),
                                            _ => understood = false,
                                        }
                                    }
                                }
                                AstNode::Bool(_) => rev = Some(a.clone()),
                                _ => understood = false,
                            }
                        }
                        // `key=None` — None lexes to Lit(0); the raw 0
                        // reached py_sorted_key as a function pointer and
                        // SEGV'd (batch 560). Treat it as no key.
                        // Batch 624: None now lexes as AstNode::NoneLit
                        // (the literal-face batch) — keep both shapes.
                        // `key=abs` — builtins have no function value to
                        // pass through the fn-pointer sort (raw abs
                        // lowered to a dead slot -> SEGV); route to the
                        // abs-key sort runtime.
                        let none_key =
                            matches!(&keyf, Some(AstNode::Lit(0)) | Some(AstNode::NoneLit));
                        let abs_key = matches!(&keyf, Some(AstNode::Var(n)) if n == "abs");
                        if none_key || abs_key {
                            keyf = None;
                        }
                        if !understood {
                            None
                        } else if abs_key {
                            let xs = self.lower_expr(&args[0]);
                            let rev_id = match &rev {
                                Some(e) => self.lower_expr(e),
                                None => self.next_id_with_lit(0),
                            };
                            let nid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_sorted_key_abs".to_string(),
                                args: vec![xs, rev_id],
                                dest: nid,
                                type_args: vec![],
                            });
                            self.exprs.insert(nid, MirExpr::Var(nid));
                            let sorted_ty = match self.type_map.get(&xs).cloned() {
                                Some(Type::DynamicArray(e)) => Type::DynamicArray(e),
                                Some(Type::Array(e, _)) => Type::DynamicArray(e),
                                _ => Type::DynamicArray(Box::new(Type::I64)),
                            };
                            self.type_map.insert(nid, sorted_ty.clone());
                            self.exprs.insert(dest, MirExpr::Var(nid));
                            self.type_map.insert(dest, sorted_ty);
                            return Some(dest);
                        } else if none_key {
                            let xs = self.lower_expr(&args[0]);
                            let rev_id = match &rev {
                                Some(e) => self.lower_expr(e),
                                None => self.next_id_with_lit(0),
                            };
                            let len_id = match self.type_map.get(&xs).cloned() {
                                Some(Type::Array(_, ArraySize::Literal(n))) => {
                                    let nid = self.next_id();
                                    self.exprs.insert(nid, MirExpr::IntLit(n as i64));
                                    self.type_map.insert(nid, Type::I64);
                                    nid
                                }
                                _ => {
                                    let nid = self.emit_call("vec_len", vec![xs], Type::I64);
                                    nid
                                }
                            };
                            let elem_is_str = matches!(
                                self.type_map.get(&xs).cloned(),
                                Some(Type::DynamicArray(e)) | Some(Type::Array(e, _))
                                    if matches!(*e, Type::Str)
                            ) as i64;
                            let flag_id = self.next_id_with_lit(elem_is_str);
                            let nid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_sorted_vec_rev".to_string(),
                                args: vec![xs, len_id, rev_id, flag_id],
                                dest: nid,
                                type_args: vec![],
                            });
                            self.exprs.insert(nid, MirExpr::Var(nid));
                            let sorted_ty = match self.type_map.get(&xs).cloned() {
                                Some(Type::DynamicArray(e)) => Type::DynamicArray(e),
                                Some(Type::Array(e, _)) => Type::DynamicArray(e),
                                _ => Type::DynamicArray(Box::new(Type::I64)),
                            };
                            self.type_map.insert(nid, sorted_ty.clone());
                            self.exprs.insert(dest, MirExpr::Var(nid));
                            self.type_map.insert(dest, sorted_ty);
                            return Some(dest);
                        } else if let Some(k) = keyf {
                            let xs = self.lower_expr(&args[0]);
                            let f = self.lower_expr(&k);
                            let r = match &rev {
                                Some(e) => self.lower_expr(e),
                                None => {
                                    let z = self.next_id();
                                    self.exprs.insert(z, MirExpr::IntLit(0));
                                    self.type_map.insert(z, Type::I64);
                                    z
                                }
                            };
                            let nid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_sorted_key".to_string(),
                                args: vec![xs, f, r],
                                dest: nid,
                                type_args: vec![],
                            });
                            self.exprs.insert(nid, MirExpr::Var(nid));
                            let sorted_ty = match self.type_map.get(&xs).cloned() {
                                Some(Type::DynamicArray(e)) => Type::DynamicArray(e),
                                Some(Type::Array(e, _)) => Type::DynamicArray(e),
                                _ => Type::DynamicArray(Box::new(Type::I64)),
                            };
                            self.type_map.insert(nid, sorted_ty.clone());
                            self.exprs.insert(dest, MirExpr::Var(nid));
                            self.type_map.insert(dest, sorted_ty);
                            return Some(dest);
                        } else if rev.is_some() {
                            let a = self.lower_expr(&args[0]);
                            let rev_id = self.lower_expr(rev.as_ref().unwrap());
                            let len_id = match self.type_map.get(&a).cloned() {
                                Some(Type::Array(_, ArraySize::Literal(n))) => {
                                    let nid = self.next_id();
                                    self.exprs.insert(nid, MirExpr::IntLit(n as i64));
                                    self.type_map.insert(nid, Type::I64);
                                    nid
                                }
                                // Batch 559: a DynamicArray has no
                                // literal size — len=-1 made
                                // zeta_sorted_vec_len sort NOTHING, so
                                // reverse=True over `["c","b","a"]` just
                                // reversed the input order. Ask the vec.
                                _ => {
                                    let nid = self.next_id();
                                    self.stmts.push(MirStmt::Call {
                                        func: "vec_len".to_string(),
                                        args: vec![a],
                                        dest: nid,
                                        type_args: vec![],
                                    });
                                    self.exprs
                                        .insert(nid, MirExpr::Var(nid));
                                    self.type_map.insert(nid, Type::I64);
                                    nid
                                }
                            };
                            let elem_is_str = matches!(
                                self.type_map.get(&a).cloned(),
                                Some(Type::DynamicArray(e)) | Some(Type::Array(e, _))
                                    if matches!(*e, Type::Str)
                            ) as i64;
                            let flag_id = self.next_id_with_lit(elem_is_str);
                            let nid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_sorted_vec_rev".to_string(),
                                args: vec![a, len_id, rev_id, flag_id],
                                dest: nid,
                                type_args: vec![],
                            });
                            self.exprs.insert(nid, MirExpr::Var(nid));
                            let sorted_ty = match self.type_map.get(&a).cloned() {
                                Some(Type::DynamicArray(e)) => Type::DynamicArray(e),
                                Some(Type::Array(e, _)) => Type::DynamicArray(e),
                                _ => Type::DynamicArray(Box::new(Type::I64)),
                            };
                            self.type_map.insert(nid, sorted_ty.clone());
                            self.exprs.insert(dest, MirExpr::Var(nid));
                            self.type_map.insert(dest, sorted_ty);
                            return Some(dest);
                        } else {
                            None
                        }
                    }
                    "arange" if argc == 1 => {
                        let a = self.lower_expr(&args[0]);
                        let nid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "zeta_arange".to_string(),
                            args: vec![a],
                            dest: nid,
                            type_args: vec![],
                        });
                        self.exprs.insert(nid, MirExpr::Var(nid));
                        self.type_map
                            .insert(nid, Type::DynamicArray(Box::new(Type::I64)));
                        self.exprs.insert(dest, MirExpr::Var(nid));
                        self.type_map.insert(dest, Type::I64);
                        return Some(nid);
                    }
                    "linspace" if argc == 3 => {
                        let a = self.lower_expr(&args[0]);
                        let b = self.lower_expr(&args[1]);
                        let c = self.lower_expr(&args[2]);
                        let nid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "zeta_linspace_i64".to_string(),
                            args: vec![a, b, c],
                            dest: nid,
                            type_args: vec![],
                        });
                        self.exprs.insert(nid, MirExpr::Var(nid));
                        self.type_map
                            .insert(nid, Type::DynamicArray(Box::new(Type::I64)));
                        self.exprs.insert(dest, MirExpr::Var(nid));
                        self.type_map.insert(dest, Type::I64);
                        return Some(nid);
                    }
                    "diff" if argc == 1 => {
                        let a = self.lower_expr(&args[0]);
                        let len_id = match self.type_map.get(&a).cloned() {
                            Some(Type::Array(_, ArraySize::Literal(n))) => {
                                let nid = self.next_id();
                                self.exprs.insert(nid, MirExpr::IntLit(n as i64));
                                self.type_map.insert(nid, Type::I64);
                                nid
                            }
                            _ => {
                                let nid = self.next_id();
                                self.exprs.insert(nid, MirExpr::IntLit(-1));
                                self.type_map.insert(nid, Type::I64);
                                nid
                            }
                        };
                        let nid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "zeta_diff_n".to_string(),
                            args: vec![a, len_id],
                            dest: nid,
                            type_args: vec![],
                        });
                        self.exprs.insert(nid, MirExpr::Var(nid));
                        self.type_map
                            .insert(nid, Type::DynamicArray(Box::new(Type::I64)));
                        self.exprs.insert(dest, MirExpr::Var(nid));
                        self.type_map.insert(dest, Type::I64);
                        return Some(nid);
                    }
                    _ => None,
                };
                if let Some(call_args) = lowered_args {
                    // `list(<map>)` is Python's KEYS list — a map handle is
                    // NOT array-like, so the passthrough below returned the
                    // map itself (type and value both wrong): `WUFU_JQ_CODES =
                    // list(dict.fromkeys(POOL + POOL2))` then held a map, so
                    // `wufu_constants._register_into_datasrc()` registered an
                    // empty universe and the local entry raised
                    // "Unknown universe: wufu".
                    if method == "list" {
                        let src = call_args[0];
                        if let Some(Type::Named(n, targs)) =
                            self.type_map.get(&src).cloned()
                        {
                            if n == "map" || n == "dict" {
                                let nid = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: "map_keys".to_string(),
                                    args: vec![src],
                                    dest: nid,
                                    type_args: vec![],
                                });
                                self.exprs.insert(nid, MirExpr::Var(nid));
                                let kty = targs.first().cloned().unwrap_or_else(Type::slot_fallback);
                                self.type_map
                                    .insert(nid, Type::DynamicArray(Box::new(kty.clone())));
                                self.exprs.insert(dest, MirExpr::Var(nid));
                                self.type_map
                                    .insert(dest, Type::DynamicArray(Box::new(kty)));
                                return Some(nid);
                            }
                        }
                        // 批次 562: a literal argument ("abc") never gets
                        // stored into a slot — forcing Var(src) made
                        // codegen load an UNINITIALIZED alloca and
                        // println_str strlen(NULL)'d. Mirror pure-value
                        // expressions instead.
                        match self.exprs.get(&src).cloned() {
                            Some(e @ MirExpr::StringLit(_))
                            | Some(e @ MirExpr::IntLit(_))
                            | Some(e @ MirExpr::FloatLit(_)) => {
                                self.exprs.insert(dest, e);
                            }
                            _ => {
                                // 批 1007：`len(list([8, 9]))` 内联形状——
                                // dest 是外层 len 的结果槽，而 list 字面量
                                // 的构造语句写在 src 槽（dynarray）。仅
                                // exprs 别名 dest→src 会让 dest 槽自身零
                                // store（codegen alloca 后无人写，读栈垃
                                // 圾——OPT 矩阵 NO_OPT 档实拍 0，O3 碰巧
                                // 寄存器分配掩盖）。物化一条 Assign 把
                                // src 搬进 dest，两槽都有定义。
                                self.stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: src,
                                });
                                self.exprs.insert(dest, MirExpr::Var(dest));
                            }
                        }
                        // list(str) — CPython splits into 1-char
                        // strings (a bare passthrough handed the string
                        // itself back — and before 562 a literal arg
                        // Var-passthrough read an uninit slot).
                        if matches!(self.type_map.get(&src).cloned(), Some(Type::Str)) {
                            let nid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_list_str".to_string(),
                                args: vec![src],
                                dest: nid,
                                type_args: vec![],
                            });
                            self.exprs.insert(nid, MirExpr::Var(nid));
                            self.type_map
                                .insert(nid, Type::DynamicArray(Box::new(Type::Str)));
                            self.exprs.insert(dest, MirExpr::Var(nid));
                            self.type_map
                                .insert(dest, Type::DynamicArray(Box::new(Type::Str)));
                            return Some(nid);
                        }
                        if let Some(t) = self.type_map.get(&src).cloned() {
                            self.type_map.insert(dest, t);
                        } else {
                            self.type_map.insert(dest, Type::I64);
                        }
                        return Some(dest);
                    }
                    let src_id = call_args[0];
                    // Str elements need CONTENT order: the generic helper
                    // compares i64 slots numerically, which for str arrays
                    // sorted heap pointers (batch 293 — `sorted(dates)`
                    // came back in insertion order).
                    let src_elem_str = matches!(
                        self.type_map.get(&src_id),
                        Some(Type::DynamicArray(e)) if **e == Type::Str
                    );
                    let func = match method {
                        "sorted" if src_elem_str => "zeta_sorted_vec_len_str",
                        "sorted" => "zeta_sorted_vec_len",
                        _ => "zeta_int_i64",
                    };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: call_args,
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    // `sorted(x)` keeps x's element type. Hardcoding I64
                    // made `sorted(str_list)` an array of bare ints, so
                    // downstream `d >= start_date` skipped the string
                    // compare and pointer-compared instead → the trading-
                    // day filter in jq_wufu_local kept 0 days.
                    let sorted_ty = match self.type_map.get(&src_id).cloned() {
                        Some(Type::DynamicArray(e)) => Type::DynamicArray(e),
                        Some(Type::Array(e, _)) => Type::DynamicArray(e),
                        _ => Type::DynamicArray(Box::new(Type::I64)),
                    };
                    self.type_map.insert(
                        dest,
                        match method {
                            "sorted" => sorted_ty,
                            _ => Type::I64,
                        },
                    );
                    return Some(dest);
                }
            }

            // PY-A: Python `str(x)` — convert any value to its string form
        None
    }
}
