//! 批次 846：FieldAccess 臂原样迁入（struct 字段读／枚举载荷／
//! 平台对象字段等判定链）。

use super::MirGen;
use super::lt_annotation_type;
use std::collections::HashMap;
use crate::middle::mir::r#gen::TypeDecl;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// 字段读发射体。`dest`＝结果槽。
    pub(super) fn lower_field_access(
        &mut self,
        base: &Box<AstNode>,
        field: &String,
        dest: u32,
    ) -> u32 {
            // Batch 572: `Counter.count` — a CLASS VARIABLE read. Class
            // variables live as module globals `{Class}__{name}` (the
            // parser desugars class-body bare assignments there); route
            // when the base names a known class and the mangled global
            // exists. Without this the read lowered to a FieldAccess on
            // the class's FuncAddr — garbage slot → crash.
            if let AstNode::Var(vname) = &**base {
                let gname = format!("{}__{}", vname, field);
                if self.type_decls.contains_key(vname.as_str())
                    && self.module_globals.contains(&gname)
                {
                    return self.lower_expr(&AstNode::Var(gname));
                }
            }
            // Batch 610: `c.kind` — a CLASS VARIABLE read through an
            // INSTANCE base (the route above needs the base to BE the
            // class name). When the base's declared type is a known class
            // whose mangled class-variable global exists — and the field
            // is NOT a struct field of it — read the global (CPython:
            // instance lookup falls back to the class). Without this the
            // read hit the instance layout's stand-in slot and returned
            // another field's value (cif: `c.kind` printed 7 = c.v).
            if let AstNode::Var(vname) = &**base {
                let tn = self.module_global_types.get(vname.as_str()).and_then(
                    |t| match t {
                        Type::Named(n, _)
                            if !self.type_decls.contains_key(vname.as_str()) =>
                        {
                            Some(n.clone())
                        }
                        _ => None,
                    },
                );
                if let Some(tn) = tn {
                    let gname = format!("{}__{}", tn, field);
                    let is_struct_field = matches!(
                        self.type_decls.get(tn.as_str()),
                        Some(crate::middle::mir::r#gen::TypeDecl::Struct {
                            fields,
                            ..
                        }) if fields.iter().any(|(f, _)| f == field)
                    );
                    if !is_struct_field && self.module_globals.contains(&gname) {
                        return self.lower_expr(&AstNode::Var(gname));
                    }
                }
            }
            // `self.<field>` where `self` is NOT bound (a synthesized
            // constructor): the value is the one already computed for that
            // field — see `self_field_aliases`. Guarded on `self` being
            // absent, so a real `self` (methods, Rust-style literals) keeps
            // reading the actual field. Measured: `self.cache_dir` read via
            // an unassigned slot → `py_os_path_join` dereferenced NULL
            // (MarketDataFetcher.__init__+92).
            if !self.name_to_id.contains_key("self") {
                if let AstNode::Var(v) = &**base {
                    if v == "self" {
                        if let Some((_, aid)) = self
                            .self_field_aliases
                            .iter()
                            .rev()
                            .find(|(f, _)| f == field)
                            .cloned()
                        {
                            let ty = self.type_map.get(&aid).cloned().unwrap_or_else(Type::slot_fallback);
                            self.exprs.insert(dest, MirExpr::Var(aid));
                            self.type_map.insert(dest, ty);
                            return dest;
                        }
                    }
                }
            }
            // PY-A: argparse results namespace — `args.<flag>` is typed from
            // the kind recorded at `add_argument` (a static method table
            // cannot enumerate dynamic field names). An undeclared flag is
            // reported and lowered to 0 rather than silently defaulting.
            // Batch 291: `timezone.utc` — an attribute on an IMPORTED
            // stdlib member. Stdlib members have no runtime value (the
            // generic path read an uninitialized local slot and
            // dereferenced it: varying SIGSEGV faults inside
            // `_now_iso`). If the registry carries the dotted member
            // (`datetime` → `timezone.utc`) as a zero-arg shim, call it.
            if let AstNode::Var(vname) = &**base {
                if let Some((module, member)) =
                    self.py_member_aliases.get(vname.as_str()).cloned()
                {
                    let dotted = format!("{}.{}", member, field);
                    if let Some(entry) =
                        crate::middle::pylib::find_member(&module, &dotted)
                    {
                        if entry.args.is_empty() {
                            self.stmts.push(MirStmt::Call {
                                func: entry.symbol.clone(),
                                args: vec![],
                                dest: dest,
                                type_args: vec![],
                            });
                            self.exprs.insert(dest, MirExpr::Var(dest));
                            let ty = match entry.handle.as_deref() {
                                Some(h) => Type::Named(h.to_string(), vec![]),
                                None => match entry.ret.as_str() {
                                    "f64" => Type::F64,
                                    "str" => Type::Str,
                                    _ => Type::I64,
                                },
                            };
                            self.type_map.insert(dest, ty);
                            return dest;
                        }
                    }
                }
            }
            {
                let probe_id = self.lower_expr(base);
                // BATCH-291: `series.dt` (pandas datetime accessor) has no
                // runtime property object — pass the array handle through so
                // the accessor METHODS (`strftime` etc.) dispatch on the
                // vector itself (`zeta_vec_strftime`). Measured: `df[col]
                // .dt.strftime("%Y-%m-%d")` reached LIBC `strftime` with a
                // garbage fmt pointer (jq_wufu_local.py:81, SIGSEGV).
                if field == "dt" {
                    if let Some(t) = self.type_map.get(&probe_id).cloned() {
                        if matches!(t, Type::DynamicArray(_) | Type::Array(_, _)) {
                            self.exprs.insert(dest, MirExpr::Var(probe_id));
                            self.type_map.insert(dest, t);
                            return dest;
                        }
                    }
                }
                // A struct FIELD read keeps its DECLARED type. Without this every
                // `self.cache_dir` / `f.cache_dir` came back I64: `len(field)` was
                // 0 and `str(field)` printed the handle as digits (measured on
                // `MarketDataFetcher.cache_dir`). The declared field type string
                // lives in `type_decls`.
                if let Some(Type::Named(cls, _)) = self.type_map.get(&probe_id).cloned() {
                    let decl = self
                        .type_decls
                        .get(&cls)
                        .or_else(|| self.shared_type_decls.get(&cls))
                        .or_else(|| {
                            // module-qualified keys (`mod__Cls`)
                            let suffix = format!("__{}", cls);
                            self.shared_type_decls
                                .iter()
                                .find(|(k, _)| k.ends_with(suffix.as_str()))
                                .map(|(_, v)| v)
                        });
                    if let Some(TypeDecl::Struct { fields, .. }) = decl {
                        if let Some((_, ty)) = fields.iter().find(|(f, _)| f == field) {
                            let mapped = match ty.as_str() {
                                "str" | "String" => Some(Type::Str),
                                "f64" | "float" => Some(Type::F64),
                                "bool" => Some(Type::Bool),
                                "i64" | "int" | "dyn" => None,
                                other => {
                                    // Batch 594: `list[T]` field spellings
                                    // (element-aware init-expr inference)
                                    // must reach the vec type here too —
                                    // `self.names`-style reads inside
                                    // methods otherwise stayed I64.
                                    if let Some(t) = lt_annotation_type(other) {
                                        Some(t)
                                    } else if let Some(tag) =
                                        crate::middle::pylib::handle_tag(other)
                                    {
                                        Some(Type::Named(tag.to_string(), vec![]))
                                    } else if other == "PyPath" {
                                        Some(Type::Named("PyPath".to_string(), vec![]))
                                    } else {
                                        None
                                    }
                                }
                            };
                            if let Some(m) = mapped {
                                self.type_map.insert(dest, m);
                            }
                        }
                    }
                }
                if matches!(
                    self.type_map.get(&probe_id),
                    Some(Type::Named(n, _)) if n == "PyArgNS"
                ) {
                    let kind = self.argparse_kinds.get(field).cloned();
                    let (func, ty) = match kind.as_deref() {
                        Some("i64") => ("py_argparse_get_i64", Type::I64),
                        Some("f64") => ("py_argparse_get_f64", Type::F64),
                        Some("bool") => ("py_argparse_get_bool", Type::Bool),
                        Some("str") => ("py_argparse_get_str", Type::Str),
                        _ => {
                            eprintln!(
                                "warning: PY-A: `args.{}` is not a declared argument \
                                 (no matching add_argument) — lowering as 0",
                                field
                            );
                            self.exprs.insert(dest, MirExpr::IntLit(0));
                            self.type_map.insert(dest, Type::I64);
                            return dest;
                        }
                    };
                    let name_id = self.next_id();
                    self.exprs
                        .insert(name_id, MirExpr::StringLit(field.clone()));
                    self.type_map.insert(name_id, Type::Str);
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: vec![probe_id, name_id],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map.insert(dest, ty);
                    return dest;
                }
            }
            // PY-A: library-handle attribute read (`d.year`, `delta.days`).
            // Field names are declared as one-argument "methods" in the
            // registry, so reads and calls share one dispatch table.
            if let AstNode::Var(vname) = &**base {
                if let Some(&slot) = self.name_to_id.get(vname.as_str()) {
                    if let Some(Type::Named(tag, _)) = self.type_map.get(&slot).cloned() {
                        if let Some((sym, ret_handle)) =
                            crate::middle::pylib::method_symbol(&tag, field)
                        {
                            let base_id = self.lower_expr(base);
                            self.stmts.push(MirStmt::Call {
                                func: sym.to_string(),
                                args: vec![base_id],
                                dest: dest,
                                type_args: vec![],
                            });
                            self.exprs.insert(dest, MirExpr::Var(dest));
                            let ty = match ret_handle {
                                Some(h) => Type::Named(h.to_string(), vec![]),
                                None => match crate::middle::pylib::method_ret(&tag, field)
                                    .unwrap_or("i64")
                                {
                                    "str" => Type::Str,
                                    "f64" => Type::F64,
                                    "vecstr" => Type::DynamicArray(Box::new(Type::Str)),
                                    "vecpath" => Type::DynamicArray(Box::new(
                                        Type::Named("PyPath".to_string(), vec![]),
                                    )),
                                    "vecjson" => Type::DynamicArray(Box::new(
                                        Type::Named("PyJson".to_string(), vec![]),
                                    )),
                                    "vecmatch" => Type::DynamicArray(Box::new(
                                        Type::Named("PyMatch".to_string(), vec![]),
                                    )),
                                    _ => Type::I64,
                                },
                            };
                            self.type_map.insert(dest, ty);
                            return dest;
                        }
                    }
                }
            }
            // PY-A: a module attribute used as a *value*
            // (`level=logging.INFO`) resolves through the registry to its
            // zero-argument shim.
            // `flatten_module_receiver` already includes `field` as the
            // last part — do not append it twice (a doubled path misses
            // the registry and falls through to a real field access on a
            // module handle, which dereferences garbage).
            // 批次 880（#278）：这里必须把当前 `field` 拼进路径——本臂的
            // flatten 只收到 `base`（`math.pi` 的 base＝Var("math") ⇒ parts=[]），
            // 不补 field 则 member 恒为空串，find_member 必然 None ⇒ 模块常量
            // 全部静默降 0（math.pi／math.e 一族，python_style 14 例）。
            // "flatten 已含 field"是 Call 臂的口径（receiver 不含方法名），
            // 抄到这里时没跟着改。
            if let Some((root, mut parts)) = Self::flatten_module_receiver(base) {
                parts.push(field.clone());
                if let Some(module) = self.py_module_aliases.get(&root).cloned() {
                    // User module namespace read: `mod.CONST` is the module
                    // global `mod__CONST` in the env (written by the
                    // module's init), so read it back through the env.
                    if self.py_user_modules.contains(&module) && parts.len() == 1 {
                        let key = format!("{}{}", module.replace('.', "_") + "__", parts[0]);
                        let key_id = self.next_id();
                        self.exprs
                            .insert(key_id, MirExpr::StringLit(key.clone()));
                        self.type_map.insert(key_id, Type::Str);
                        self.stmts.push(MirStmt::Call {
                            func: "zeta_env_get".to_string(),
                            args: vec![key_id],
                            dest: dest,
                            type_args: vec![],
                        });
                        if self.func_ret_types.contains_key(&key)
                            && !self.global_consts.contains_key(&key)
                            && !self.type_decls.contains_key(&key)
                        {
                            // `mod.fn` as a value: same reason as the `Var` arm
                            // above — the def is not in the env, so read the
                            // symbol's address, and leave the env read in place.
                            self.exprs.insert(dest, MirExpr::FuncAddr(key));
                            self.type_map.insert(dest, Type::I64);
                            return dest;
                        }
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        // Keep the global's static type. Hardcoding I64 made
                        // `import a; a.C.exists()` a call on an untyped value:
                        // the handle tag was lost and the link failed with a
                        // bare `_a__C.exists`. `from a import C` worked (its
                        // binding path already carried the type).
                        let ty = self.global_ty_of(&key).unwrap_or(Type::slot_fallback());
                        self.type_map.insert(dest, ty);
                        return dest;
                    }
                    let member = parts.join(".");
                    match crate::middle::pylib::find_member(&module, &member) {
                        Some(entry) if entry.args.is_empty() => {
                            let sym = entry.symbol.as_str();
                            self.stmts.push(MirStmt::Call {
                                func: sym.to_string(),
                                args: vec![],
                                dest: dest,
                                type_args: vec![],
                            });
                            self.exprs.insert(dest, MirExpr::Var(dest));
                            self.type_map.insert(
                                dest,
                                match entry.ret.as_str() {
                                    "f64" => Type::F64,
                                    "str" => Type::Str,
                                    "vec" => Type::DynamicArray(Box::new(Type::I64)),
                                    // Without these a module attribute read
                                    // was typed i64, so a later subscript or
                                    // for-in lost the element type and handed
                                    // back handles/0 instead of strings.
                                    "vecstr" => {
                                        Type::DynamicArray(Box::new(Type::Str))
                                    }
                                    "vecpath" => Type::DynamicArray(Box::new(
                                        Type::Named("PyPath".to_string(), vec![]),
                                    )),
                                    "vecjson" => Type::DynamicArray(Box::new(
                                        Type::Named("PyJson".to_string(), vec![]),
                                    )),
                                    "vecmatch" => Type::DynamicArray(Box::new(
                                        Type::Named("PyMatch".to_string(), vec![]),
                                    )),
                                    _ => Type::I64,
                                },
                            );
                            return dest;
                        }
                        Some(_) => {
                            // Needs arguments; a bare attribute read can
                            // only be the attribute itself.
                        }
                        None => {
                            eprintln!(
                                "warning: PY-A: `{}.{}` has no registry entry and is not \
                                 a value — lowering it as 0",
                                root, member
                            );
                            self.exprs.insert(dest, MirExpr::IntLit(0));
                            self.type_map.insert(dest, Type::I64);
                            return dest;
                        }
                    }
                }
            }
            // Implement proper field access
            // 1. Evaluate the base expression
            let base_id = self.lower_expr(base);
            // PY-A: library-handle attribute read (`d.year`,
            // `p.date().year`, `delta.days`). Field names are declared as
            // one-argument "methods" in the registry, so reads and calls
            // share one dispatch table. Checked on the *lowered* base type
            // so chained calls work, not only plain variables.
            if let Some(Type::Named(tag, _)) = self.type_map.get(&base_id).cloned() {
                if let Some((sym, ret_handle)) =
                    crate::middle::pylib::method_symbol(&tag, field)
                {
                    self.stmts.push(MirStmt::Call {
                        func: sym.to_string(),
                        args: vec![base_id],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    // A handle-returning attribute (Path.parent) keeps its
                    // tag so the next `.name`/`.parent` still dispatches.
                    // The vec kinds matter too: `Path(...).parents[2]` must be
                    // DynamicArray(PyPath) so `[2]` yields a PyPath handle,
                    // otherwise `.exists()` / `.read_text()` on it emitted
                    // bare symbols (`_exists` 7 / `_read_text` 6 reference
                    // sites in the REasyQuant local backtest).
                    let ty = match ret_handle {
                        Some(h) => Type::Named(h.to_string(), vec![]),
                        None => match crate::middle::pylib::method_ret(&tag, field)
                            .unwrap_or("i64")
                        {
                            "str" => Type::Str,
                            "f64" => Type::F64,
                            "vecstr" => Type::DynamicArray(Box::new(Type::Str)),
                            "vecpath" => Type::DynamicArray(Box::new(
                                Type::Named("PyPath".to_string(), vec![]),
                            )),
                            "vecjson" => Type::DynamicArray(Box::new(
                                Type::Named("PyJson".to_string(), vec![]),
                            )),
                            "vecmatch" => Type::DynamicArray(Box::new(
                                Type::Named("PyMatch".to_string(), vec![]),
                            )),
                            _ => Type::I64,
                        },
                    };
                    self.type_map.insert(dest, ty);
                    return dest;
                }
            }
            // PY-A: zero-argument method read via FieldAccess on a KNOWN
            // struct — `df.columns` (no parens) must dispatch to
            // `DataFrame::columns`, not read a raw slot. Without this the
            // field read returned the `data` map handle and
            // `len(df.columns)` did array_len on a map → printed 0.
            if let Some(Type::Named(tn, _)) = self.type_map.get(&base_id).cloned() {
                let qualified = format!("{}::{}", tn, field);
                // A MODULE-MANGLED receiver name
                // (`backend_strategy_wufu_backend__LocalBackend`) missed the
                // plain-keyed `func_ret_types` table, so its @property read
                // degraded to a RAW FIELD LOAD and the next map walk SEGFAULTED
                // (measured in `_parity_snapshot` after context.portfolio got
                // refined to Named). Retry with the trailing class name.
                let qualified = if self.func_ret_types.contains_key(&qualified)
                    || !tn.contains("__")
                {
                    qualified
                } else {
                    let plain = tn.rsplit("__").next().unwrap_or(&tn);
                    let q2 = format!("{}::{}", plain, field);
                    if self.func_ret_types.contains_key(&q2) {
                        q2
                    } else {
                        qualified
                    }
                };
                if let Some(ret_ty) = self.func_ret_types.get(&qualified).cloned() {
                    self.emit_call_into(dest, &qualified, vec![base_id], ret_ty);
                    return dest;
                }
            }
            // 批次147 重放: FieldAccess on a VEC/LIST handle (`xs.values`,
            // `.tolist`) — vecs have no fields, the handle IS the value
            // (identity). A raw struct field read on a vec handle
            // returned garbage and crashed the subscript (t202).
            // 定长数组字面量与动态 vec 共享堆布局（[cap|len|elems]），
            // 两者皆 identity。
            match self.type_map.get(&base_id).cloned() {
                Some(Type::DynamicArray(elem)) => {
                    self.exprs.insert(dest, MirExpr::Var(base_id));
                    self.type_map.insert(dest, Type::DynamicArray(elem));
                    return dest;
                }
                Some(Type::Array(elem, _)) => {
                    self.exprs.insert(dest, MirExpr::Var(base_id));
                    self.type_map
                        .insert(dest, Type::DynamicArray(elem));
                    return dest;
                }
                _ => {}
            }
            // BATCH-429: the arms above took Named / vec-shaped receivers, so
            // what is left has no class tag at all (i64 / dynamic slot) —
            // a unique zero-arg member of that name is a property, so call it
            // instead of loading a field off an untagged handle.
            if !matches!(self.type_map.get(&base_id), Some(Type::Named(_, _)))
                && let Some((symbol, ret_ty)) = self.unique_zero_arg_property(field)
            {
                self.emit_call_into(dest, &symbol, vec![base_id], ret_ty);
                return dest;
            }
            // 2. Create FieldAccess expression
            self.exprs.insert(
                dest,
                MirExpr::FieldAccess {
                    base: base_id,
                    field: field.clone(),
                },
            );
            // 3. Take the field's DECLARED type from the struct. Typing
            //    every field as i64 meant a `str` field was used as an
            //    integer afterwards: `print(A("hi").name)` printed the
            //    pointer, while `A("hi").name == "hi"` was still True —
            //    the value was right, only its TYPE was lost.
            let struct_field_ty = |decls: &HashMap<String, TypeDecl>, tn: &str, f: &str| {
                let mut cands = vec![tn.to_string()];
                if let Some((_, tail)) = tn.rsplit_once("__") {
                    cands.push(tail.to_string());
                }
                // Batch 299: the OTHER mangling direction. A cross-module
                // read sees the BARE class name (`p.code` in the driver,
                // `Position` from `wufu_backend`) while `type_decls` keys
                // it as `backend_strategy_wufu_backend__Position`, so every
                // field fell back to I64: `total_amount` printed the raw
                // double bits (4652007308841189376) and `code` printed the
                // string handle — the closing valuation came out at 0.
                let suffix = format!("__{}", tn);
                let mut module_qualified: Vec<String> = decls
                    .iter()
                    .filter(|(k, _)| k.ends_with(&suffix))
                    .map(|(k, _)| k.clone())
                    .collect();
                // Deterministic order: two modules may each declare a class
                // of this name, and `HashMap` iteration would otherwise pick
                // a different field type per compile.
                module_qualified.sort();
                cands.extend(module_qualified);
                cands.iter().find_map(|t| match decls.get(t) {
                    Some(TypeDecl::Struct { fields, .. }) => fields
                        .iter()
                        .find(|(x, _)| x == f)
                        // Batch 291: annotate-path first — `list[str]`
                        // must reach DynamicArray(Str); `from_string`
                        // alone left the field I64-typed, so
                        // `g.pool + [x]` added two HANDLES (len 14)
                        // instead of concatenating.
                        .map(|(_, ft)| {
                            lt_annotation_type(ft)
                                .unwrap_or_else(|| Type::from_string(ft))
                        }),
                    _ => None,
                })
            };
            let field_ty = match self.type_map.get(&base_id) {
                Some(Type::Named(tn, _)) => struct_field_ty(&self.type_decls, tn, field),
                // `A("hi").name` — recover the struct from the call itself
                // when the base's type was lost upstream.
                _ => match &**base {
                    AstNode::Call {
                        receiver: None,
                        method,
                        ..
                    } => struct_field_ty(&self.type_decls, method, field),
                    // 批 944：基槽是 ABI 缺省（I64/PyDynamic/未知）时查
                    // checker_env 的具名型——参数 v 的槽标 I64（数组按
                    // 指针传参），但调用点证据链可能已把 v 定成 Named
                    // (Point)；字段型丢在函数边界会让返回槽落 I64，
                    // f64 位模式被整数算术读成垃圾（x*2 实拍
                    // 9218868437227405312，CPython 3.0）
                    _ => match &**base {
                        AstNode::Var(vn) => {
                            match self.checker_type_of(vn) {
                                Some(Type::Named(tn, _)) => {
                                    struct_field_ty(&self.type_decls, &tn, field)
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    },
                },
            };
            self.type_map.insert(dest, field_ty.unwrap_or(Type::slot_fallback()));
            dest
    }
}

#[cfg(test)]
mod tests_906 {
    use super::*;
    use crate::middle::mir::r#gen::TypeDecl;
    use std::collections::HashMap;

    /// 批次 906（866 回归钉）：类变量读（FieldAccess）必须返回 env_get 的
    /// 结果槽——866 前的委托丢弃 lower_field_access 的返回值，lower_expr
    /// 退回派发器缺省 IntLit(0) 槽（t105/t813 族的静默错值面）。
    #[test]
    fn class_var_read_returns_env_slot() {
        let mut decls = HashMap::new();
        decls.insert(
            "Counter".to_string(),
            TypeDecl::Struct {
                fields: vec![("count".to_string(), "i64".to_string())],
                generics: vec![],
            },
        );
        let mut globals = std::collections::HashSet::new();
        globals.insert("Counter__count".to_string());
        let mut g = MirGen::new()
            .with_type_decls(decls)
            .with_module_globals(globals);
        // 台架须模拟 lower_to_mir 的前置合并（shared_type_decls → type_decls）
        // ——直接调 lower_expr 不经过 lower_to_mir，type_decls 停留在空表。
        g.type_decls.extend(
            g.shared_type_decls
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        let r = g.lower_expr(&AstNode::FieldAccess {
            base: Box::new(AstNode::Var("Counter".to_string())),
            field: "count".to_string(),
        });
        // 866 前的委托丢返回值 ⇒ r 落派发器缺省 IntLit(0) 槽
        assert!(
            matches!(g.exprs.get(&r), Some(MirExpr::Var(_))),
            "866: 字段读必须返回 env_get 结果槽（缺省 IntLit(0)＝回归）"
        );
    }
}
