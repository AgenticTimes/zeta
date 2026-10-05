//! 批次 959（轴 D）：pylib 平台成员分派族（869 零适配法自 gen.rs 迁出）。
//! py_member_target/py_member_call/py_struct_type_of/py_struct_has_field/
//! py_handle_of——`os.path.join(...)` 等 dotted 平台调用的成员解析、
//! 调用发射、struct 型推断与句柄 tag 识别，主题聚合。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::r#gen::TypeDecl;
use crate::middle::types::Type;

impl MirGen {
    /// hands it back typed by the name's declared type (`global_ty_of`) — so a
    /// float cell is REINTERPRETED, not converted. A float going into such a
    /// cell therefore has to go in as its BIT PATTERN: the call-argument
    /// coercion used to `fptosi` it, which dropped the fraction (measured:
    /// module global `ratio = 2.5`, read in another function as `0.000000`).
    pub(crate) fn py_member_target(
        &self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
    ) -> Option<(String, String)> {
        let (module, member) = match receiver {
            None => self.py_member_aliases.get(method)?.clone(),
            Some(recv) => {
                let (root, parts) = Self::flatten_module_receiver(recv)?;
                let module = self.py_module_aliases.get(&root)?.clone();
                let member = if parts.is_empty() {
                    method.to_string()
                } else {
                    format!("{}.{}", parts.join("."), method)
                };
                (module, member)
            }
        };
        // `a.C.exists()` where `a.C` is a module-level VALUE, not a registry
        // member: the generic member path appended the method to the global's
        // name and emitted a bare `_a__C.exists` (link error). If the registry has
        // no such member but the dotted prefix names a typed global, this is a
        // handle method call on that value — return None so the caller lowers
        // `a.C` as a value and dispatches on its handle tag.
        if crate::middle::pylib::find_member(&module, &member).is_none() {
            if let Some((prefix, _last)) = member.rsplit_once('.') {
                let mangled = format!("{}_{}", module.replace('.', "_"), prefix);
                if self.global_ty_of(&mangled).is_some() || self.global_ty_of(prefix).is_some() {
                    return None;
                }
            }
        }
        Some((module, member))
    }

    pub(crate) fn py_member_call(
        &self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
    ) -> Option<(&'static str, Option<&'static str>, &'static str)> {
        let (module, member) = match receiver {
            None => {
                let (m, mem) = self.py_member_aliases.get(method)?;
                (m.clone(), mem.clone())
            }
            Some(recv) => {
                // `os.path.join(...)` parses as Call{receiver: FieldAccess{os,path}}
                // — flatten the chain into a dotted member path so submodule
                // members (os.path.*, os.environ.*) can be registered.
                let (root, parts) = Self::flatten_module_receiver(recv)?;
                let module = self.py_module_aliases.get(&root)?.clone();
                let member = if parts.is_empty() {
                    method.to_string()
                } else {
                    format!("{}.{}", parts.join("."), method)
                };
                (module, member)
            }
        };
        if let Some(entry) = crate::middle::pylib::find_member(&module, &member) {
            return Some((entry.symbol.as_str(), entry.handle.as_deref(), entry.ret.as_str()));
        }
        // `a.C.exists()` where `a.C` is a module-level VALUE: the member chain is
        // not a registry shim, so the disk-module fallback below would emit
        // `a__C.exists` — a symbol that does not exist (link error). If the dotted
        // prefix names a typed global, this is a handle method call on that value;
        // return None so the caller lowers `a.C` as a value and dispatches on its
        // handle tag (same fix as in py_member_target).
        if let Some((prefix, _last)) = member.rsplit_once('.') {
            let mangled = format!("{}_{}", module.replace('.', "_"), prefix);
            if self.global_ty_of(&mangled).is_some() || self.global_ty_of(prefix).is_some() {
                return None;
            }
        }
        // Not a registry shim: a module loaded from disk resolves to its
        // `mod__name` mangled symbol (no handle tag, i64 result).
        //
        // Canonicalize the module name first: the SAME file is reachable as
        // `jq_shim` (bare, via the strategy dir on sys.path) and as
        // `strategies.code.jq_shim` (dotted). The definitions land under whichever
        // spelling loaded first (`jq_shim__get_cost_config`), so a literal
        // spelling here emitted a second, undefined prefix — measured as
        // `U _strategies_code_jq_shim__get_cost_config` against
        // `T _jq_shim__get_cost_config`.
        let module = self
            .py_module_aliases
            .get(&module)
            .cloned()
            .unwrap_or(module);
        // `import pkg.user` (a DOTTED import without an alias) registers the
        // alias for the ROOT only, so `pkg.user.show()` came out as a bare
        // `show_1` ghost call. When an alias plus the split member path names a
        // real user module, treat that as the module: `pkg` + `.` + `user` ->
        // `pkg.user`, leaving `show` as the member.
        if let Some((first, rest_parts)) = member.split_once('.') {
            let dotted = format!("{}.{}", module, first);
            if self.py_user_modules.contains(&dotted) {
                let new_member = rest_parts.to_string();
                if let Some(entry) = crate::middle::pylib::find_member(&dotted, &new_member) {
                    return Some((
                        entry.symbol.as_str(),
                        entry.handle.as_deref(),
                        entry.ret.as_str(),
                    ));
                }
                // A user module's function: `<module with _>__<member>`.
                return Some((
                    Box::leak(format!("{}__{}", dotted.replace('.', "_"), new_member).into_boxed_str()),
                    None,
                    "i64",
                ));
            }
        }
        if self.py_user_modules.contains(&module) {
            let prefix = format!("{}__", module.replace('.', "_"));
            let sym = format!("{}{}", prefix, member);
            // Carry the inferred return type: a library function returning a
            // string must be typed str at the call site, or its result gets
            // printed/compared as an integer.
            let ret_kind: &'static str = match self.func_ret_types.get(&sym) {
                Some(Type::Str) => "str",
                Some(Type::F64) => "f64",
                _ => "i64",
            };
            // Leaked so the &'static str signature holds; one small alloc per
            // distinct module member per compile.
            let leaked: &'static str = Box::leak(sym.into_boxed_str());
            return Some((leaked, None, ret_kind));
        }
        // Registry module with an unknown member reached through attribute
        // access (`threading.nope()`): from-imports already warn, so warn here
        // too instead of leaving a bare linker error as the only feedback.
        {
            use std::collections::HashSet;
            use std::sync::{Mutex, OnceLock};
            static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
            let w = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
            let key = format!("{}.{}", module, member);
            if let Ok(mut set) = w.lock() {
                if set.insert(key) {
                    eprintln!(
                        "warning: PY-A: unknown member `{}` in Python module `{}` — the \
                         symbol will be resolved by name at link time",
                        member, module
                    );
                }
            }
        }
        None
    }

    /// PY-A: the library handle tag of a receiver expression, if any
    /// (lets `t.start()` / `lock.acquire()` dispatch exactly instead of by
    /// name-guessing).
    /// PY-A: the declared type name of a simple variable receiver, for the
    /// `getattr(obj, "literal")` static rewrite. Unlike `py_handle_of` this is
    /// not restricted to handle tags: JoinQuant's `g` is a plain struct, and
    /// `getattr(g, "ranked_etfs_result", [])` must resolve to its field (or to
    /// the given default) instead of a bogus undefined symbol.
    pub(crate) fn py_struct_type_of(&self, recv: &AstNode) -> Option<String> {
        let name = match recv {
            AstNode::Var(n) => n,
            _ => return None,
        };
        if let Some(id) = self.name_to_id.get(name) {
            if let Some(Type::Named(n, _)) = self.type_map.get(id) {
                return Some(n.clone());
            }
        }
        if let Some(Type::Named(n, _)) = self.global_ty_of(name) {
            return Some(n.clone());
        }
        None
    }

    /// Field lookup that also accepts a module-mangled type name
    /// (`jq_shim__G` -> `G`), since `type_decls` keys come from the plain
    /// class/struct declaration.
    pub(crate) fn py_struct_has_field(&self, tyname: &str, field: &str) -> bool {
        let mut candidates = vec![tyname.to_string()];
        if let Some((_, tail)) = tyname.rsplit_once("__") {
            candidates.push(tail.to_string());
        }
        if let Some((_, tail)) = tyname.rsplit_once('_') {
            candidates.push(tail.to_string());
        }
        candidates.iter().any(|t| {
            matches!(
                self.type_decls.get(t),
                Some(TypeDecl::Struct { fields, .. }) if fields.iter().any(|(f, _)| f == field)
            )
        })
    }

    /// BATCH-438: the three spellings a method's receiver parameter arrives in.
    pub(crate) fn py_handle_of(&self, recv: &AstNode) -> Option<String> {
        // A chained call whose callee is a registry member that declares a
        // handle (e.g. `hashlib.md5("x").hexdigest()`): the result's tag is
        // known statically from the registry, so no lowering is needed here.
        if let AstNode::Call {
            receiver: inner,
            method,
            ..
        } = recv
        {
            if let Some((module, member)) = self.py_member_target(inner, method) {
                if let Some(h) = crate::middle::pylib::find_member(&module, &member)
                    .and_then(|m| m.handle.clone())
                {
                    return Some(h);
                }
            }
            // Chained method on a handle: `pat.search(s).group(2)` — the inner
            // call's own result tag comes from the registry ret_handle. Without
            // this the outer method fell through to a bare `group` extern.
            if let Some(inner_ast) = inner {
                if let Some(tag) = self.py_handle_of(inner_ast) {
                    if let Some((_, Some(ret))) =
                        crate::middle::pylib::method_symbol(&tag, method)
                    {
                        return Some(ret.to_string());
                    }
                }
            }
        }
        // A handle-returning attribute (`Path(...).resolve().parent`) — the
        // tag comes from the registry's ret_handle for that method.
        if let AstNode::FieldAccess { base, field } = recv {
            if let Some(tag) = self.py_handle_of(base) {
                if let Some((_, Some(ret))) =
                    crate::middle::pylib::method_symbol(&tag, field)
                {
                    return Some(ret.to_string());
                }
            }
        }
        let AstNode::Var(name) = recv else {
            return None;
        };
        if let Some(id) = self.name_to_id.get(name) {
            if let Some(Type::Named(n, _)) = self.type_map.get(id) {
                if n.starts_with("Py") {
                    return Some(n.clone());
                }
            }
            return None;
        }
        // A module-level global is read through the env, so it has no local
        // slot — its type comes from the resolver's module-global table.
        // Without this, `q = queue.Queue()` then `q.put(x)` in another function
        // emitted a bare `put` call.
        if let Some(Type::Named(n, _)) = self.module_global_types.get(name) {
            if n.starts_with("Py") {
                return Some(n.clone());
            }
        }
        None
    }
}
