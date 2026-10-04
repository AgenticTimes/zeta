//! 批次 876：Call 臂整体迁入（5840 行——方法分派主干）。
//!
//! 869 零适配法：签名取臂的原样解构类型（Call 的 Option/Vec 字段），
//! 臂体逐字。臂内 15 处 `return self.lower_expr(...)` 早退产出非 id 槽，
//! 调用点必须转发返回值（866 教训）。子族发射体仍住在 call_print 等
//! 家族文件，本函数只承载留在 gen.rs 里的分派主干。

use super::call_class::{classify_call, type_name_of, CallClass};
use super::call_json::json_route;
use super::call_str::{path_ends_with_mem, str_method_symbol, str_method_symbol3, to_string_channel};
use super::MirGen;
use crate::frontend::ast::AstNode;
use super::format_template_parts;
use super::list_elem_suffix;
use super::registry_ret_type;
use super::repr_routable;
use crate::middle::mir::mir::{MirExpr, MirStmt, SemiringOp};
use crate::middle::mir::r#gen::TypeDecl;
use crate::middle::types::{ArraySize, Type};
use std::collections::HashMap;


impl MirGen {
    pub(super) fn lower_call_arm(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &String,
        args: &Vec<AstNode>,
        type_args: &Vec<String>,
        id: u32,
    ) -> u32 {
            // PY-A batch 291: `cls(...)` inside a @classmethod body. The
            // class desugar keeps classmethods as plain functions named
            // `Class::method` with `cls` as a first parameter that no
            // call site binds — the free call slipped past every function
            // lookup and was emitted as the bare symbol `cls` (died in the
            // loud stub). The constructor is desugared under the CLASS
            // name, so rewrite the callee to the enclosing class.
            let cls_ctor: Option<String> = if receiver.is_none()
                && method.as_str() == "cls"
                && !self.func_ret_types.contains_key("cls")
            {
                self.current_class.clone()
            } else {
                None
            };
            let method: &String = cls_ctor.as_ref().unwrap_or(method);
            // PY-A: `from <user module> import member` — the module's file
            // is loaded and lowered into THIS binary under
            // `<module>__<name>` symbols, so a free call to an imported
            // member must be routed there. Without this the call went out as
            // an unresolved external, which is why the in-repo LOCAL shim
            // (strategies/code/jq_shim.py) could never link.
            if receiver.is_none() {
                if let Some((module, member)) = self.py_member_aliases.get(method).cloned() {
                    // Batch 754 (#265): the caller's OWN module defining the
                    // same bare name wins over a transitive import alias —
                    // CPython scope semantics: another module's
                    // `from .x import f` must not leak into this file's
                    // namespace. jq_wufu.py defines its own tuple-returning
                    // `get_premium_rate`; wufu_core's import alias of the
                    // backend's same-name f64 function hijacked every call
                    // in the file, typed the tuple result F64 and crashed
                    // codegen on the tuple unpack (stack_array_get on a
                    // float-typed slot, corpus 38/40 → this fix).
                    // (cleanup lane batch 10006 ported the same guard.)
                    let (module, member) = {
                        let own =
                            format!("{}__{}", self.current_module.replace('.', "_"), method);
                        let alias_q = format!("{}__{}", module.replace('.', "_"), member);
                        if own != alias_q && self.func_param_names.contains_key(&own) {
                            (self.current_module.clone(), method.clone())
                        } else {
                            (module, member)
                        }
                    };
                    // A member the REGISTRY already declares keeps its C
                    // shim: a library that SUPPLEMENTS a registered module
                    // would otherwise steal `pd.Timestamp` from the
                    // handle-typed shim (regressed t179).
                    let registered =
                        crate::middle::pylib::find_member(&module, &member).is_some();
                    if self.py_user_modules.contains(&module) && !registered {
                        // Python default arguments: the generic call path
                        // fills them, and skipping that here silently read 0
                        // (`add3(1, 2)` gave 3, not 13).
                        let mut call_args = args.clone();
                        // The resolver keys a LOADED MODULE's defaults by
                        // the module-qualified name (`pyfixturearity__add3`);
                        // the bare name only exists for functions defined in
                        // the file being compiled.
                        // Canonicalize: the SAME file is reachable as
                        // `jq_shim` and as `strategies.code.jq_shim`, and the
                        // definitions live under whichever spelling loaded
                        // FIRST. Without this the call site emitted a second,
                        // undefined prefix (`U _strategies_code_jq_shim__…`
                        // against `T _jq_shim__…`, 22 reference sites).
                        let module = self
                            .py_module_aliases
                            .get(&module)
                            .cloned()
                            .unwrap_or(module);
                        let qualified = format!("{}__{}", module.replace('.', "_"), member);
                        let defaults = self
                            .param_defaults
                            .get(&qualified)
                            .or_else(|| self.param_defaults.get(member.as_str()))
                            .cloned();
                        // Batch 409: `name=value` arrives as `__kwarg__` markers
                        // and nothing here read the names — the values were
                        // appended in WRITTEN order, so skipping a defaulted
                        // middle parameter bound every later argument one slot
                        // early. This path, not the generic one that does bind
                        // by name, lowers every callee imported from another
                        // module (`from callee import inject`).
                        let names = self
                            .func_param_names
                            .get(&qualified)
                            .or_else(|| self.func_param_names.get(member.as_str()))
                            .cloned();
                        if let Some(ordered) = names.as_deref().and_then(|n| {
                            Self::bind_kwarg_markers(&call_args, n, defaults.as_deref())
                        }) {
                            call_args = ordered;
                        }
                        if let Some(defaults) = defaults {
                            for (i, d) in defaults.iter().enumerate() {
                                if i >= call_args.len() {
                                    match d {
                                        Some(dv) => call_args.push(dv.clone()),
                                        None => break,
                                    }
                                }
                            }
                        }
                        let arg_ids: Vec<u32> =
                            call_args.iter().map(|a| self.lower_expr(a)).collect();
                        let func = format!("{}__{}", module.replace('.', "_"), member);
                        self.stmts.push(MirStmt::Call {
                            func: func.clone(),
                            args: arg_ids,
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        // Keep the inferred result type. The resolver
                        // registers a loaded module's function return types,
                        // so `first("hello")` stays a str — guessing i64 here
                        // printed the pointer (t53_param_inference).
                        if !self.type_map.contains_key(&id) {
                            let ty = self
                                .func_ret_types
                                .get(&func)
                                .or_else(|| self.func_ret_types.get(&member))
                                .cloned()
                                .unwrap_or(Type::slot_fallback());
                            self.type_map.insert(id, ty);
                        }
                        return id;
                    }
                }
            }
            // 批次148 重放(批次122): np.where — 1-arg = nonzero indices
            // (Vec), 3-arg = elementwise/scalar select. The C runtime
            // dispatches on zt_looks_like_vec; the registry's ret=i64 loses
            // the Vec-ness, so the following [0] subscript guessed a map
            // and SEGV'd. Intercept BEFORE the registry dispatch and type
            // the result by the cond's static shape.
            // `np.where(...)`：receiver 是模块别名（np），参数才是 mask/三元。
            // 仅自由调用（np.where）——DataFrame 的方法 `.where(a)` 仍走
            // 幽灵路径（t213 期望编译报错）。
            let where_is_free = matches!(
                receiver.as_ref().map(|r| &**r),
                Some(AstNode::Var(v)) if self.py_module_aliases.contains_key(v.as_str())
            ) || (receiver.is_none()
                && self
                    .py_member_target(&None, "where")
                    .map_or(false, |(m, mem)| m == "numpy" && mem == "where"));
            if where_is_free && method == "where" && (args.len() == 1 || args.len() == 3) {
                let ids: Vec<u32> = args.iter().map(|a| self.lower_expr(a)).collect();
                let (func, ret) = if args.len() == 1 {
                    ("zeta_np_where1", Type::DynamicArray(Box::new(Type::I64)))
                } else {
                    let cond_vec = matches!(
                        self.type_map.get(&ids[0]),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    );
                    (
                        "zeta_np_where3",
                        if cond_vec {
                            Type::DynamicArray(Box::new(Type::I64))
                        } else {
                            Type::I64
                        },
                    )
                };
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: ids,
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, ret);
                return id;
            }

            // np.zeros((r, c)) — the TUPLE form must unpack into the 2-arg
            // primitive. `pylib/numpy.z`'s `zeros(n)` wrapper took the tuple
            // handle as the length, so the runtime allocated a nonsense flat
            // array and the program HUNG (t221). The MIR interception the
            // library comment referred to did not exist.
            let zeros_is_free = matches!(
                receiver.as_ref().map(|r| &**r),
                Some(AstNode::Var(v)) if self.py_module_aliases.contains_key(v.as_str())
            ) || (receiver.is_none()
                && self
                    .py_member_target(&None, "zeros")
                    .map_or(false, |(m, mem)| m == "numpy" && mem == "zeros"));
            if zeros_is_free && method == "zeros" && args.len() == 1 {
                if let AstNode::Tuple(items) = &args[0] {
                    if items.len() == 2 {
                        let r = self.lower_expr(&items[0]);
                        let c = self.lower_expr(&items[1]);
                        self.stmts.push(MirStmt::Call {
                            func: "zeta_np_zeros2".to_string(),
                            args: vec![r, c],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map
                            .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                        return id;
                    }
                }
            }

            // 批次 835：re.sub 臂迁入 gen/call_re.rs；入口判定读分类器
            // （RegularSub）。
            if classify_call(method) == CallClass::RegularSub {
                if let Some(nid) = self.lower_re_sub(receiver, method, &args, id) {
                    return nid;
                }
            }
            // PY-A: a keyword-argument marker that reached expression
            // position (unknown callee) is just its value.
            if method == "__kwarg__" && receiver.is_none() && args.len() == 2 {
                return self.lower_expr(&args[1]);
            }
            // PY-A: `Counter(<list of str>)` must content-hash its keys,
            // exactly like a dict literal — the plain shim keys by pointer
            // and would count each literal site separately.
            // PY-A: `dataclasses.asdict(x)` — the receiver's struct type is
            // known statically, so expand to a dict literal of its fields.
            // There is no runtime reflection to build such a dict, and a
            // link-time `asdict` extern would be a worse outcome than the
            // compile-time expansion. Restricted to a plain variable
            // receiver so the expression is not evaluated twice.
            if method == "asdict" && args.len() == 1 {
                let is_asdict = match receiver {
                    None => self
                        .py_member_aliases
                        .get("asdict")
                        .map(|(m, mem)| m == "dataclasses" && mem == "asdict")
                        .unwrap_or(false),
                    Some(_) => self
                        .py_member_target(receiver, method)
                        .map(|(m, mem)| m == "dataclasses" && mem == "asdict")
                        .unwrap_or(false),
                };
                if is_asdict {
                    if let AstNode::Var(_) = &args[0] {
                        let base_id = self.lower_expr(&args[0]);
                        let tyname = match self.type_map.get(&base_id) {
                            Some(Type::Named(n, _)) => Some(n.clone()),
                            _ => None,
                        };
                        let fields = tyname.and_then(|tn| {
                            match self.shared_type_decls.get(&tn) {
                                Some(TypeDecl::Struct { fields, .. }) => {
                                    Some(fields.clone())
                                }
                                _ => None,
                            }
                        });
                        if let Some(fields) = fields {
                            let entries: Vec<(AstNode, AstNode)> = fields
                                .iter()
                                .map(|(f, _)| {
                                    (
                                        AstNode::StringLit(f.clone()),
                                        AstNode::FieldAccess {
                                            base: Box::new(args[0].clone()),
                                            field: f.clone(),
                                        },
                                    )
                                })
                                .collect();
                            return self.lower_expr(&AstNode::DictLit { entries });
                        }
                    }
                }
            }
            // 批次 833 重做：json.dumps/dump 执行者调用（原臂逐字迁入
            // call_json.rs::lower_json）。
            if classify_call(method) == CallClass::JsonDump {
                if let Some(nid) = self.lower_json(receiver, method, &args, id) {
                    return nid;
                }
            }

            // PY-A: `import X` for a user module also runs its module body
            // once (Python executes a module on import). The init function
            // guards itself, so repeated imports are harmless. `from X
            // import y` must run it too — otherwise bound members that read
            // module-level state see uninitialized globals (silent wrong
            // values, e.g. `LIMIT` read as 0).
            if (method == "zeta_py_import"
                || method == "zeta_py_from"
                || method == "zeta_py_star")
                && receiver.is_none()
            {
                if let Some(AstNode::StringLit(module)) = args.first() {
                    // `zeta_py_import` args = (module, alias);
                    // `zeta_py_from` args = (module, member, alias) — the
                    // module is first in both. `zeta_py_star` = (module,).
                    // A function-local RELATIVE import carries the raw spec
                    // (`..datasrc.market_data_universe`); map it to the
                    // resolved module first, or the containment test fails and
                    // the module init is never emitted.
                    let module = self
                        .py_module_aliases
                        .get(module)
                        .cloned()
                        .unwrap_or_else(|| module.clone());
                    let is_user = self.py_user_modules.contains(&module);
                    let has_init = self
                        .func_ret_types
                        .contains_key(&format!("{}__init", module.replace('.', "_")));
                    if crate::diagnostics::env_flag("ZETA_PROBE_GLOBALS") {
                        eprintln!(
                            "IMPORT lowering `{}`: user={} has_init={} user_mods={} cur={:?}",
                            module,
                            is_user,
                            has_init,
                            self.py_user_modules.len(),
                            self.current_module
                        );
                    }
                    if is_user || has_init {
                        let init_sym = format!("{}__init", module.replace('.', "_"));
                        let init_dest = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: init_sym,
                            args: vec![],
                            dest: init_dest,
                            type_args: vec![],
                        });
                    }
                }
            }
            // PY-A: `from X import *` is a compile-time-only marker — the
            // resolver has already bound the public names. Emit no runtime
            // call (there is no `zeta_py_star` shim); the value is unused.
            if method == "zeta_py_star" && receiver.is_none() {
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
                return id;
            }
            // PY-A: module-internal bare call → mangled symbol (imported
            // modules are registered under `mod__name`).
            if receiver.is_none() {
                if let Some(mangled) = self.symbol_renames.get(method).cloned() {
                    if mangled != *method {
                        let rewritten = AstNode::Call {
                            receiver: None,
                            method: mangled,
                            args: args.clone(),
                            type_args: type_args.clone(),
                            structural: false,
                        };
                        return self.lower_expr(&rewritten);
                    }
                }
                // A module-PRIVATE call (`_source_score`) from inside a
                // CLOSURE: the child MirGen inherits `symbol_renames`, but the
                // enclosing function never called that name itself, so the
                // table can be empty and the closure emitted a bare
                // `_source_score` — the linker then resolved it to nothing
                // (measured: `__source_score` undefined, from
                // `_ranked_fetch_sources`'s `key=lambda s: … _source_score(s, bs_code)`).
                // Rebuild the mangled name from the current module.
                // Batch 288 guard: an `_`-leading bare call that is an
                // IMPORT ALIAS (`from dotenv import load_dotenv as
                // _load_dotenv`, market_data_sources.py:17-19) must not be
                // mangled to `module___load_dotenv` — no such definition
                // exists and LINKING fails for the whole program.
                // Batch 288 guard 2: NESTED defs are hoisted to synthetic
                // `__closure_N` functions and bound by user name in
                // closure_vars/hoisted_names (gen.rs:1688); the call path
                // at :9442 dispatches them. Mangling `_src`/`_ensure`
                // (data_ops_log.py:132,136) to `module___src` preempted
                // that lookup → undefined symbols.
                if method.starts_with('_')
                    && !method.starts_with("__")
                    && !self.py_member_aliases.contains_key(method.as_str())
                    && !self.module_globals.contains(method.as_str())
                    && !self.closure_vars.contains_key(method.as_str())
                    && !self.hoisted_names.contains_key(method.as_str())
                    // Batch 288 guard 4: a leading-underscore CLASS name
                    // (t137 `_Frame(v=7)`) — the constructor is emitted
                    // under its bare name; mangling breaks the link.
                    && !self.type_decls.contains_key(method.as_str())
                    // Batch 406 guard 5: a ROOT-file (`__main__`) `_x` def is keyed and
                    // emitted BARE (`_fmt` → C `__fmt`), so mangling its call site to
                    // `__main____fmt` aims at a ghost no definition satisfies — and guard 3
                    // cannot rescue it, since the `__<method>` suffix it matches never appears
                    // on a bare key. Measured pre-fix: `nm` `U ___main_____fmt` vs `T __fmt`
                    // ⇒ whole-program link failure (rc=1), not a wrong value.
                    && !self.func_ret_types.contains_key(method.as_str())
                {
                    if !self.current_module.is_empty() {
                        let m = self.current_module.clone();
                        let mut mangled =
                            format!("{}__{}", m.replace('.', "_"), method);
                        // Batch 288 guard 3: the caller's module context
                        // can be wrong — `MarketDataFetcher.
                        // is_cache_fully_covered` (market_data_fetcher.py:
                        // 151-158) lowered under `__main__` mangled `_to_ts`
                        // to `__main______to_ts` while the definition is
                        // `backend_datasrc_market_data_fetcher___to_ts`.
                        // Only trust the current-module name when such a
                        // definition actually exists; otherwise adopt the
                        // unique defined `module__<method>` (ambiguity keeps
                        // the old behavior so real misses still surface).
                        if !self.func_ret_types.contains_key(&mangled) {
                            let suffix = format!("__{}", method);
                            let hits: Vec<&String> = self
                                .func_ret_types
                                .keys()
                                .filter(|k| k.ends_with(&suffix.as_str()))
                                .collect();
                            if hits.len() == 1 {
                                mangled = hits[0].clone();
                            }
                        }
                        let rewritten = AstNode::Call {
                            receiver: None,
                            method: mangled,
                            args: args.clone(),
                            type_args: type_args.clone(),
                            structural: false,
                        };
                        return self.lower_expr(&rewritten);
                    }
                }
            }
            // PY-A: `with X [as n]:` desugar — route the context protocol.
            // Library handles use their tagged method (PyLock → mutex
            // acquire/release); anything else falls back to identity AND
            // warns, so a `with` that does not actually enter is recorded
            // rather than silently pretending (the old desugar ignored the
            // protocol entirely — `with lock:` never locked).
            if (method == "zeta_with_enter" || method == "zeta_with_exit")
                && receiver.is_none()
                && args.len() == 1
            {
                let proto = if method == "zeta_with_exit" {
                    "__exit__"
                } else {
                    "__enter__"
                };
                let recv_tag = self.py_handle_of(&args[0]);
                let arg_id = self.lower_expr(&args[0]);
                // Known Py* handle → registry shim (e.g. PyLock → acquire/
                // release). Otherwise dispatch to a USER-defined
                // `__enter__`/`__exit__` on the receiver's type (Python
                // context protocol) — `class C: def __enter__(self): ...`.
                // Only a type with neither falls back to identity + warning.
                let routed: Option<String> = match &recv_tag {
                    Some(tag) => crate::middle::pylib::method_symbol(tag, proto)
                        .map(|(sym, _)| sym.to_string()),
                    None => match self.type_map.get(&arg_id) {
                        Some(Type::Named(tn, _)) => {
                            let qual = format!("{}::{}", tn, proto);
                            if self.func_ret_types.contains_key(&qual) {
                                Some(qual)
                            } else if self.func_ret_types.contains_key(proto) {
                                Some(proto.to_string())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    },
                };
                let symbol = match routed {
                    Some(sym) => sym,
                    None => {
                        eprintln!(
                            "warning: PY-A: `with` on a value with no known context \
                             protocol — enter/exit are no-ops"
                        );
                        "zeta_identity1".to_string()
                    }
                };
                self.stmts.push(MirStmt::Call {
                    func: symbol,
                    args: vec![arg_id],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                // `with X as n:` binds the ENTER result, which is X itself
                // for every context manager we shim (file handles, locks).
                // Typing it i64 lost the handle tag, so `f.write(...)`
                // inside the block silently did nothing.
                self.type_map.insert(
                    id,
                    match (method.as_str(), recv_tag) {
                        ("zeta_with_enter", Some(tag)) => {
                            Type::Named(tag.to_string(), vec![])
                        }
                        _ => Type::I64,
                    },
                );
                return id;
            }
            // PY-A: `Thread(target, args=(a, b, ...))` with TWO OR MORE
            // positional args. The spawned entry point receives exactly one
            // i64 (the packed tuple handle), so calling the target directly
            // reads the handle as its first argument (silent garbage). We
            // synthesize an adapter that unpacks the tuple and calls the
            // real target: `fn __tp: target(__tp[0], __tp[1], ...)`.
            // 0/1-arg threads keep the direct path below (a 1-tuple
            // collapses to its element, so the value is already right).
            if method == "Thread"
                && (receiver.is_none()
                    || matches!(receiver.as_deref(), Some(AstNode::Var(v)) if v == "threading"))
            {
                if let Some((target_name, elems)) = Self::thread_args_tuple(args) {
                    if elems.len() >= 2 {
                        let mut call_args: Vec<AstNode> = Vec::with_capacity(elems.len());
                        for i in 0..elems.len() {
                            call_args.push(AstNode::Call {
                                receiver: None,
                                method: "array_get".to_string(),
                                args: vec![
                                    AstNode::Var("__tp".to_string()),
                                    AstNode::Lit(i as i64),
                                ],
                                type_args: vec![],
                                structural: false,
                            });
                        }
                        let body = AstNode::Call {
                            receiver: None,
                            method: target_name,
                            args: call_args,
                            type_args: vec![],
                            structural: false,
                        };
                        let adapter = self.lower_closure(&["__tp".to_string()], &body);
                        let adapter_addr = self.next_id();
                        self.exprs.insert(adapter_addr, MirExpr::FuncAddr(adapter));
                        self.type_map.insert(adapter_addr, Type::I64);
                        // Pack the tuple ourselves (not via lower_expr(Tuple))
                        // so we can see each element's static type: the
                        // adapter reads i64 slots, so an f64 element would
                        // lose its float encoding unless bit-cast — which
                        // the MIR has no primitive for. Record it loudly
                        // rather than launching a thread that reads garbage.
                        let mut elem_ids = Vec::with_capacity(elems.len());
                        let mut has_float = false;
                        for e in &elems {
                            let eid = self.lower_expr(e);
                            if matches!(
                                self.type_map.get(&eid),
                                Some(Type::F64) | Some(Type::F32)
                            ) {
                                has_float = true;
                            }
                            elem_ids.push(eid);
                        }
                        if has_float {
                            eprintln!(
                                "warning: PY-A: Thread args contain an f64 value; the \
                                 thread entry point receives i64 slots, so a float argument \
                                 is not bit-exact (use an int argument, or cast inside the \
                                 target)"
                            );
                        }
                        // `py_threading_thread_new_2(fn, arg)` takes ONE
                        // argument: a single-element `Thread(f, args=(41,))`
                        // must pass the ELEMENT. (Before one-element tuples
                        // existed the parser collapsed `(41,)` to `41`, so this
                        // worked by accident.)
                        let n = elem_ids.len();
                        let packed = if n == 1 {
                            elem_ids.remove(0)
                        } else {
                            let packed = self.next_id();
                            self.exprs.insert(
                                packed,
                                MirExpr::StackArray {
                                    elements: elem_ids,
                                    size: n,
                                },
                            );
                            self.type_map.insert(packed, Type::Tuple(vec![Type::I64; n]));
                            packed
                        };
                        self.stmts.push(MirStmt::Call {
                            func: "py_threading_thread_new".to_string(),
                            args: vec![adapter_addr, packed],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map
                            .insert(id, Type::Named("PyThread".to_string(), vec![]));
                        return id;
                    }
                }
            }
            // PY-A: Python stdlib shims — `threading.Thread(f)` /
            // `from threading import Thread; Thread(f)` dispatch to runtime
            // shims, and calls on a returned handle (`t.start()`,
            // `lock.acquire()`) dispatch by the handle's type tag.
            // PY-A: argparse — ArgumentParser(...) / add_argument(...) /
            // parse_args() and `args.<flag>` are compiler-side rewrites: the
            // flag kinds only exist at the call site, so a static registry
            // method table cannot type `args.<field>`.
            if method == "ArgumentParser"
                && (receiver.is_none()
                    || matches!(receiver.as_deref(), Some(AstNode::Var(v)) if v == "argparse"))
            {
                self.stmts.push(MirStmt::Call {
                    func: "py_argparse_new".to_string(),
                    args: vec![],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::Named("PyArgParser".to_string(), vec![]));
                return id;
            }
            if method == "add_argument" && receiver.is_some() {
                let mut flag: Option<String> = None;
                let mut default_expr: Option<AstNode> = None;
                for a in args {
                    if let AstNode::Call {
                        receiver: None,
                        method: m,
                        args: ka,
                        ..
                    } = a
                    {
                        if m == "__kwarg__" && ka.len() == 2 {
                            if let AstNode::StringLit(nm) = &ka[0] {
                                match nm.as_str() {
                                    "default" => default_expr = Some(ka[1].clone()),
                                    "required" => eprintln!(
                                        "note: PY-A: argparse `required=True` is accepted but \
                                         not enforced (V1)"
                                    ),
                                    _ => {}
                                }
                                continue;
                            }
                        }
                    }
                    if let AstNode::StringLit(s) = a {
                        if flag.is_none() {
                            flag = Some(s.clone());
                        }
                    }
                }
                if let Some(fname) = flag {
                    let recv = self.lower_expr(receiver.as_ref().unwrap());
                    let dstr = match &default_expr {
                        Some(AstNode::StringLit(s)) => {
                            let sid = self.next_id();
                            self.exprs.insert(sid, MirExpr::StringLit(s.clone()));
                            self.type_map.insert(sid, Type::Str);
                            sid
                        }
                        Some(e) => {
                            let v = self.lower_expr(e);
                            self.lower_to_string(v)
                        }
                        None => {
                            let sid = self.next_id();
                            self.exprs.insert(sid, MirExpr::StringLit(String::new()));
                            self.type_map.insert(sid, Type::Str);
                            sid
                        }
                    };
                    let name_id = self.next_id();
                    // Keyed by Python's `dest` (dashes stripped, `-`→`_`)
                    // so it matches what `args.<field>` looks up; the raw
                    // dashed flag is what argv is scanned for.
                    let norm = fname.trim_start_matches('-').replace('-', "_");
                    self.exprs.insert(name_id, MirExpr::StringLit(norm));
                    self.type_map.insert(name_id, Type::Str);
                    let flag_id = self.next_id();
                    self.exprs.insert(flag_id, MirExpr::StringLit(fname.clone()));
                    self.type_map.insert(flag_id, Type::Str);
                    self.emit_call_into(id, "py_argparse_add", vec![recv, name_id, flag_id, dstr], Type::I64);
                    return id;
                }
            }
            if method == "parse_args" && receiver.is_some() {
                let recv = self.lower_expr(receiver.as_ref().unwrap());
                self.stmts.push(MirStmt::Call {
                    func: "py_argparse_parse".to_string(),
                    args: vec![recv],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::Named("PyArgNS".to_string(), vec![]));
                return id;
            }
            // PY-A: `from datetime import datetime, timedelta` shadows the
            // MODULE name with the imported name, so `import`-style aliases
            // are absent and `py_member_call` bails out — `datetime.timedelta(
            // days=375)` then degraded to a free call to the bare member name
            // (undefined `timedelta`, 16 call sites). Resolve by the ROOT TEXT
            // when it names a registered module.
            // PY-A: `x.replace(year=…, month=…, day=…)` — the kwargs SET
            // identifies `date.replace` uniquely (str.replace takes
            // positional args only). Without this the call fell through to
            // the generic name table (`replace` → host_str_replace, arity 3),
            // the arity mismatch mangled the symbol to
            // `host_str_replace_4`, and the link failed.
            if method == "replace" && !args.is_empty() {
                let kw = |a: &AstNode| -> Option<String> {
                    if let AstNode::Call { receiver: None, method, args, .. } = a {
                        if method == "__kwarg__" && args.len() == 2 {
                            if let AstNode::StringLit(n) = &args[0] {
                                return Some(n.clone());
                            }
                        }
                    }
                    None
                };
                let mut slots: Vec<(String, u32)> = Vec::new();
                let mut kw_count = 0usize;
                let mut all_kwargs = true;
                let mut missing_name: Option<String> = None;
                for a in args {
                    match kw(a).as_deref() {
                        Some(n @ ("year" | "month" | "day")) => {
                            kw_count += 1;
                            let v = if let AstNode::Call { args, .. } = a {
                                self.lower_expr(&args[1])
                            } else {
                                self.next_id_with_lit(0)
                            };
                            slots.push((n.to_string(), v));
                        }
                        _ => {
                            all_kwargs = false;
                            missing_name = Some(String::new());
                        }
                    }
                }
                let _ = missing_name;
                if all_kwargs && kw_count > 0 {
                    if let Some(recv) = receiver.as_ref() {
                        let recv_id = self.lower_expr(recv);
                        let mut ids = [0u32; 3];
                        for (n, v) in slots {
                            match n.as_str() {
                                "year" => ids[0] = v,
                                "month" => ids[1] = v,
                                _ => ids[2] = v,
                            }
                        }
                        for slot in ids.iter_mut() {
                            if *slot == 0 {
                                *slot = self.next_id_with_lit(0);
                            }
                        }
                        self.stmts.push(MirStmt::Call {
                            func: "py_dt_replace".to_string(),
                            args: vec![recv_id, ids[0], ids[1], ids[2]],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map
                            .insert(id, Type::Named("PyDate".to_string(), vec![]));
                        return id;
                    }
                }
            }
            // PY-A: `logging.FileHandler(path, mode="w")` — `mode` is a
            // hint the local no-op shim has no use for, but it must not turn
            // the call into a phantom arity-suffixed symbol
            // (`py_logging_FileHandler_2`). Keep the first positional arg.
            // 批次 836：logging.FileHandler 迁入 gen/call_logging.rs。
            if classify_call(method) == CallClass::Logging {
                if let Some(nid) = self.lower_logging(receiver, method, &args, id) {
                    return nid;
                }
            }
            // 批次 836：logging.getLogger 迁入 gen/call_logging.rs
            //（0 参补空名防幽灵符号）。
            if classify_call(method) == CallClass::Logging {
                if let Some(nid) = self.lower_logging(receiver, method, &args, id) {
                    return nid;
                }
            }
            // `pd.DataFrame(columns=[...])` / `pd.DataFrame(index=…, dtype=…)`
            // — the KWARG-ONLY schema constructors. The kwarg NAME is dropped
            // by the generic path, so the VALUE list was passed as `data`: the
            // shim's ctor stored a Vec where a column map belongs and
            // `len(df)` SEGV'd (measured). Build the map the ctor wants:
            // each named column becomes an EMPTY column; a schema-less frame
            // becomes an empty map. 10 `columns=` + 4 `index=/dtype=` sites in
            // REasyQuant's data/ML layer hit this.
            // 批次 837：构造器族（DataFrame kwarg ctor／Counter）迁入
            // gen/call_ctor.rs；入口判定读分类器（Special + 方法名）。
            if classify_call(method) == CallClass::Special
                && (method == "DataFrame" || method == "Counter")
            {
                if let Some(nid) = self.lower_ctor(receiver, method, type_args, args, id) {
                    return nid;
                }
            }
            // `typing.cast(T, v)` is a NO-OP that only narrows the STATIC
            // type: lower `v` and keep ITS type. Going through the registry
            // (`F typing cast py_typing_cast args=i64,i64 ret=i64`) retyped the
            // value I64, so `cast(pd.Timestamp, ts).strftime(...)` dispatched on
            // an I64 receiver — the PyDate handle was read as an integer and
            // `strftime` crashed (measured: `warmup_start_of` + strftime_l).
            // Verified MIR: `py_typing_cast(…) -> 9: I64` feeding
            // `strftime_2(9, …)`.
            if method == "cast"
                && args.len() == 2
                && self
                    .py_member_target(receiver, "cast")
                    .map_or(false, |(m, mem)| m == "typing" && mem == "cast")
            {
                return self.lower_expr(&args[1]);
            }
            // 批次 843（#272）：float(字符串) 静默错值根治——注册表
            // float 项符号是 zeta_float_i64（整数语义），Str 实参会把
            // 句柄当 i64 转 double（实测 float("2.5") 打 4.3e9）。在
            // 注册表路由**之前**按实参静态类型改道：Str ⇒ zeta_float_str
            //（运行期 strtod，py_additions.c:3810 在库）。
            if method == "float"
                && receiver.is_none()
                && args.len() == 1
            {
                eprintln!("[DBG-B] float 改道臂命中");
                let a = self.lower_expr(&args[0]);
                let route = match self.type_map.get(&a).cloned() {
                    Some(Type::Str) => "zeta_float_str",
                    _ => "zeta_float_i64",
                };
                let nid = self.emit_call(route, vec![a], Type::F64);
                return nid;
            }
            let member_call = self.py_member_call(receiver, method).or_else(|| {
                let recv = receiver.as_ref()?;
                let (root, parts) = Self::flatten_module_receiver(recv)?;
                crate::middle::pylib::find_module(&root)?;
                let member = if parts.is_empty() {
                    method.to_string()
                } else {
                    format!("{}.{}", parts.join("."), method)
                };
                // The registry keys class-static members by their dotted
                // name (`datetime.now`, `date.today`), so try that too.
                let dotted = format!("{}.{}", root, member);
                crate::middle::pylib::find_member(&root, &member)
                    .or_else(|| crate::middle::pylib::find_member(&root, &dotted))
                    .map(|e| (e.symbol.as_str(), e.handle.as_deref(), e.ret.as_str()))
            });
            if let Some((symbol, handle, ret)) = member_call {
                if crate::diagnostics::env_flag("ZETA_PROBE_CALL") {
                    eprintln!("PROBE hit symbol={} handle={:?} ret={}", symbol, handle, ret);
                }
                let mut lowered = Vec::with_capacity(args.len());
                for a in args {
                    lowered.push(self.lower_expr(a));
                }
                // `pd.Timestamp(x)` where x is ALREADY a PyDate: the symbol
                // parses a STRING pointer; handed a PyDate cell ([days][secs])
                // it produced garbage dates (the 1969-… warmup, `_to_ts` →
                // covers_range). Type it as identity instead.
                if symbol == "py_dt_from_str"
                    && lowered.len() == 1
                    && matches!(
                        self.type_map.get(&lowered[0]),
                        Some(Type::Named(n, _)) if n == "PyDate"
                    )
                {
                    let src = lowered[0];
                    self.exprs.insert(id, MirExpr::Var(src));
                    self.type_map
                        .insert(id, Type::Named("PyDate".to_string(), vec![]));
                    return id;
                }
                self.stmts.push(MirStmt::Call {
                    func: symbol.to_string(),
                    args: lowered,
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                // A module loaded from disk (`numpy__arange`) carries its
                // declared return type in `func_ret_types`; `py_member_call`
                // can only report the coarse "str"/"f64"/"i64" kind, so a
                // `-> lt(vec, i64)` library function came back typed I64 and
                // `np.arange(4)[2]` did a MAP subscript on a Vec (SEGV,
                // t212/t227). Prefer the declared type when we have one.
                let declared_ret = self
                    .func_ret_types
                    .get(symbol)
                    .cloned()
                    .or_else(|| {
                        // `pd.DataFrame({...})` POSITIONAL inside a FUNCTION
                        // body resolves to the shim class's arity-mangled
                        // ctor (`DataFrame_2`), which carries no declared
                        // return type — the result was typed I64, so every
                        // later `df.columns` / `df.data` became a MAP lookup
                        // (measured: `ParquetCache.load` returned 892 rows /
                        // **0 columns**, and `df.itertuples` crashed). At
                        // module level the same call goes through the typed
                        // path, which is why only function bodies broke.
                        if method == "DataFrame" && symbol.contains("DataFrame") {
                            Some(Type::Named("DataFrame".to_string(), vec![]))
                        } else {
                            None
                        }
                    });
                // FORCE the shim struct for the pandas ctor: the arity-mangled
                // entry DOES declare a ret (i64), so `or_else` above never
                // fired and the result stayed I64.
                let declared_ret = if method == "DataFrame" && symbol.starts_with("DataFrame") {
                    Some(Type::Named("DataFrame".to_string(), vec![]))
                } else {
                    declared_ret
                };
                self.type_map.insert(
                    id,
                    match (handle, ret) {
                        (Some(h), _) => Type::Named(h.to_string(), vec![]),
                        (None, r) => declared_ret.unwrap_or_else(|| match r {
                            "f64" => Type::F64,
                            "str" => Type::Str,
                            "vec" => Type::DynamicArray(Box::new(Type::I64)),
                            "vecstr" => Type::DynamicArray(Box::new(Type::Str)),
                            // `pd.read_parquet(...)`: the runtime reader
                            // returns the column map (name -> vector of value
                            // strings). Without this the result was I64, so
                            // `df["trade_date"]` was a MAP subscript on an
                            // integer and `"col" in df` reported the
                            // unsupported-container warning.
                            "map" => Type::Named(
                                "map".to_string(),
                                vec![
                                    Type::Str,
                                    Type::DynamicArray(Box::new(Type::Str)),
                                ],
                            ),
                            "vecmatch" => Type::DynamicArray(Box::new(Type::Named(
                                "PyMatch".to_string(),
                                vec![],
                            ))),
                            _ => Type::I64,
                        }),
                    },
                );
                return id;
            }
            // PY-A: variadic `log.info(fmt, *args)` — the registry declares a
            // fixed arity, so extra args were arity-mangled into phantom
            // symbols (`py_logger_info_4/_5`). Route to the variadic helper
            // (V1: no %-substitution, but the values are PRINTED, never
            // dropped). Only when the receiver is a known PyLogger and no
            // arg is a kwarg wrapper.
            if matches!(method.as_str(), "info" | "warning" | "error")
                && args.len() >= 2
                && args.len() <= 5
            {
                let no_kwargs = !args.iter().any(|a| {
                    matches!(a, AstNode::Call { method: km, .. } if km == "__kwarg__")
                });
                if no_kwargs {
                    if let Some(recv) = receiver.as_ref() {
                        if self.py_handle_of(recv).as_deref() == Some("PyLogger") {
                            let lg = self.lower_expr(recv);
                            let fmt = self.lower_expr(&args[0]);
                            let mut vals: Vec<u32> = Vec::new();
                            let mut n_extra = 0i64;
                            for a in &args[1..] {
                                // `log.info(fmt, *args)` — a varargs HANDLE has no
                                // static length, and lowering the starred operand
                                // emitted a raw Deref that codegen compiled into
                                // `ldr [x8]` with x8 == the (possibly EMPTY, i.e. 0)
                                // handle → SEGV in `_LogAdapter::info` (measured:
                                // args param == 0 at the fault). Skip such an
                                // operand, but say so — never silently and never a
                                // bogus dereference.
                                if let AstNode::UnaryOp { op, expr } = a {
                                    if op == "*" {
                                        eprintln!(
                                            "PY-A: `log.{}(..., *{})` — a starred operand \
has no static length here; it is NOT expanded (no values dropped from a fixed-arity log \
call, no NULL-handle dereference).",
                                            method,
                                            match &**expr {
                                                AstNode::Var(v) => v.as_str(),
                                                _ => "expr",
                                            }
                                        );
                                        continue;
                                    }
                                }
                                n_extra += 1;
                                vals.push(self.lower_expr(a));
                            }
                            let n_lit = self.next_id_with_lit(n_extra);
                            while vals.len() < 4 {
                                vals.push(self.next_id_with_lit(0));
                            }
                            self.emit_call_into(id, &format!("py_logger_{}_n", method), vec![lg, fmt, n_lit, vals[0], vals[1], vals[2], vals[3]], Type::I64);
                            return id;
                        }
                    }
                }
            }
            if let Some(recv) = receiver {
                if let Some(tag) = self.py_handle_of(recv) {
                    // `.items()` / `.values()` on a parsed JSON OBJECT: the
                    // object's payload IS a dict, but the registry has no
                    // entry for those on PyJson, so the call arity-mangled
                    // into a stub returning 0 — `{str(k): str(v) for k, v in
                    // data.items()}` silently produced {} (measured: the ETF
                    // listing cache parsed to 1724 keys and the comprehension
                    // to 0). Route to the map helpers on the object payload.
                    if tag == "PyJson"
                        && args.is_empty()
                        && (method == "items" || method == "values")
                    {
                        let recv_id = self.lower_expr(recv);
                        let dest = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: format!("py_json_{}", method),
                            args: vec![recv_id],
                            dest,
                            type_args: vec![],
                        });
                        // Elements keep their JSON identity: `for v in
                        // obj.values(): print(v)` must print the VALUE, not
                        // the raw 64-bit handle (t57 regression). pairs from
                        // `.items()` stay untyped slots.
                        let elem = if method == "values" {
                            Type::Named("PyJson".to_string(), vec![])
                        } else {
                            Type::I64
                        };
                        let ty = Type::DynamicArray(Box::new(elem));
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map.insert(dest, ty.clone());
                        self.exprs.insert(id, MirExpr::Var(dest));
                        self.type_map.insert(id, ty);
                        if method == "items" {
                            self.py_json_items_ids.insert(id);
                            self.py_json_items_ids.insert(dest);
                        }
                        return id;
                    }
                    if let Some((symbol, ret_handle)) =
                        crate::middle::pylib::method_symbol(&tag, method)
                    {
                        let mut lowered = vec![self.lower_expr(recv)];
                        for a in args {
                            lowered.push(self.lower_expr(a));
                        }
                        // `p.open()` — Python omits the mode (which defaults
                        // to "r"); the registry declares `args=2`, so the
                        // 1-arg call arity-mangled the symbol to an
                        // undefined `py_file_open_1` (t232). `py_file_open`
                        // already treats a null mode as "r", so 0 is the
                        // right filler. Deliberately NOT generic: a missing
                        // POSITIONAL argument must stay loud (there ARE
                        // `_N` variants in the runtime, e.g.
                        // `py_threading_thread_new_2`).
                        if tag == "PyPath" && method == "open" {
                            if lowered.len() == 1 || (lowered.len() == 2
                                && matches!(args.first(), Some(AstNode::Call { method: km, .. }) if km == "__kwarg__"))
                            {
                                lowered.truncate(1);
                                let z = self.next_id();
                                self.exprs.insert(z, MirExpr::IntLit(0));
                                self.type_map.insert(z, Type::I64);
                                lowered.push(z);
                            }
                        }
                        // `d.get(k)` — Python's optional default; the
                        // registry declares the 3-argument form.
                        if tag == "PyJson" && method == "get" {
                            if lowered.len() == 2 {
                                let z = self.next_id();
                                self.exprs.insert(z, MirExpr::IntLit(0));
                                self.type_map.insert(z, Type::I64);
                                lowered.push(z);
                            }
                            // Pass the default's static type so a non-Json
                            // default can be wrapped into a Json value.
                            let dtag: i64 = if lowered.len() >= 3 {
                                match self.type_map.get(&lowered[2]) {
                                    Some(Type::Str) => 2,
                                    Some(Type::F64) | Some(Type::F32) => 1,
                                    _ => 0,
                                }
                            } else {
                                0
                            };
                            let t = self.next_id();
                            self.exprs.insert(t, MirExpr::IntLit(dtag));
                            self.type_map.insert(t, Type::I64);
                            lowered.push(t);
                        }
                        if crate::diagnostics::env_flag("ZETA_PROBE_W") {
                            eprintln!(
                                "PROBE siteA tag={} method={} ret_handle={:?} ret={:?}",
                                tag, method, ret_handle,
                                crate::middle::pylib::method_ret(&tag, method)
                            );
                        }
                        self.stmts.push(MirStmt::Call {
                            func: symbol.to_string(),
                            args: lowered,
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(
                            id,
                            match ret_handle {
                                Some(h) => Type::Named(h.to_string(), vec![]),
                                None => match crate::middle::pylib::method_ret(&tag, method) {
                                    Some("str") => Type::Str,
                                    Some("f64") => Type::F64,
                                    Some("vecstr") => {
                                        Type::DynamicArray(Box::new(Type::Str))
                                    }
                                    Some("vecpath") => Type::DynamicArray(Box::new(
                                        Type::Named("PyPath".to_string(), vec![]),
                                    )),
                                    Some("vecjson") => Type::DynamicArray(Box::new(
                                        Type::Named("PyJson".to_string(), vec![]),
                                    )),
                                    Some("vecmatch") => Type::DynamicArray(Box::new(
                                        Type::Named("PyMatch".to_string(), vec![]),
                                    )),
                                    _ => Type::I64,
                                },
                            },
                        );
                        return id;
                    }
                }
            }
            // SPECIAL HANDLING: __builtin_swap generates Swap MIR statement
            if method == "__builtin_swap" && receiver.is_none() && args.len() >= 3 {
                // __builtin_swap(a_ptr, b_ptr, size) -> swap *a_ptr with *b_ptr (size bytes)
                let a_ptr_id = self.lower_expr(&args[0]);
                let b_ptr_id = self.lower_expr(&args[1]);
                let size_id = self.lower_expr(&args[2]);

                self.stmts.push(MirStmt::Swap {
                    a_ptr: a_ptr_id,
                    b_ptr: b_ptr_id,
                    size: size_id,
                });

                // Unit return
                let unit_id = self.next_id();
                self.exprs.insert(unit_id, MirExpr::IntLit(0));
                self.type_map.insert(unit_id, Type::Tuple(vec![]));
                return unit_id;
            }

            // SPECIAL HANDLING: copy() builtin — creates a copy of a value
            if method == "copy" && receiver.is_none() && args.len() == 1 {
                // For now, copy is just identity (values are copy-by-default in Zeta)
                let val_id = self.lower_expr(&args[0]);
                self.exprs.insert(id, MirExpr::Var(val_id));
                if let Some(ty) = self.type_map.get(&val_id) {
                    self.type_map.insert(id, ty.clone());
                } else {
                    self.type_map.insert(id, Type::I64);
                }
                return id;
            }

            // SPECIAL HANDLING: move() builtin — transfers ownership (no-op in current arch)
            if method == "move" && receiver.is_none() && args.len() == 1 {
                let val_id = self.lower_expr(&args[0]);
                self.exprs.insert(id, MirExpr::Var(val_id));
                if let Some(ty) = self.type_map.get(&val_id) {
                    self.type_map.insert(id, ty.clone());
                } else {
                    self.type_map.insert(id, Type::I64);
                }
                return id;
            }

            // SPECIAL HANDLING: syscall() — emits raw Linux syscall via inline asm
            if method == "syscall" && receiver.is_none() && args.len() >= 1 {
                let num_id = self.lower_expr(&args[0]);
                let mut arg_ids = Vec::new();
                for i in 1..args.len() {
                    arg_ids.push(self.lower_expr(&args[i]));
                }
                self.exprs.insert(id, MirExpr::Syscall(num_id, arg_ids));
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // SPECIAL HANDLING: trait queries — trait::value_type<T>, trait::is_same<T, U>, etc.
            if method.starts_with("trait::") && receiver.is_none() {
                let query = &method[7..]; // Strip "trait::"
                match query {
                    "value_type" if args.len() == 1 => {
                        // trait::value_type<Container> — for now returns i64
                        self.exprs.insert(id, MirExpr::IntLit(0));
                        self.type_map
                            .insert(id, Type::TraitResult("i64".to_string()));
                    }
                    "difference_type" if args.len() == 1 => {
                        self.exprs.insert(id, MirExpr::IntLit(0));
                        self.type_map
                            .insert(id, Type::TraitResult("i64".to_string()));
                    }
                    "iterator_category" if args.len() == 1 => {
                        self.exprs.insert(id, MirExpr::IntLit(0));
                        self.type_map
                            .insert(id, Type::TraitResult("RandomAccessIterator".to_string()));
                    }
                    "is_regular" if args.len() == 1 => {
                        self.exprs.insert(id, MirExpr::IntLit(1));
                        self.type_map.insert(id, Type::Bool);
                    }
                    "is_integer" if args.len() == 1 => {
                        self.exprs.insert(id, MirExpr::IntLit(1));
                        self.type_map.insert(id, Type::Bool);
                    }
                    "is_floating_point" if args.len() == 1 => {
                        self.exprs.insert(id, MirExpr::IntLit(0));
                        self.type_map.insert(id, Type::Bool);
                    }
                    "is_same" if args.len() == 2 => {
                        // trait::is_same<T, U> — for now returns 1 (true)
                        self.exprs.insert(id, MirExpr::IntLit(1));
                        self.type_map.insert(id, Type::Bool);
                    }
                    s if s.starts_with("enable_if<") => {
                        // trait::enable_if<condition, T> — for now returns 0
                        self.exprs.insert(id, MirExpr::IntLit(0));
                        self.type_map.insert(id, Type::I64);
                    }
                    _ => {
                        self.exprs.insert(id, MirExpr::IntLit(0));
                        self.type_map.insert(id, Type::I64);
                    }
                }
                return id;
            }

            // SPECIAL HANDLING: pre()/post()/invariant() assertions
            if (method == "pre" || method == "post" || method == "invariant")
                && receiver.is_none()
                && !args.is_empty()
            {
                let cond_id = self.lower_expr(&args[0]);
                let message = if args.len() >= 2 {
                    if let AstNode::StringLit(msg) = &args[1] {
                        msg.clone()
                    } else {
                        format!("{} assertion", method)
                    }
                } else {
                    format!("{} assertion", method)
                };

                let stmt = match method.as_str() {
                    "pre" => MirStmt::Pre {
                        cond: cond_id,
                        message,
                    },
                    "post" => MirStmt::Post {
                        cond: cond_id,
                        message,
                    },
                    "invariant" => MirStmt::Invariant {
                        cond: cond_id,
                        message,
                    },
                    _ => unreachable!(),
                };
                self.stmts.push(stmt);

                let unit_id = self.next_id();
                self.exprs.insert(unit_id, MirExpr::IntLit(0));
                self.type_map.insert(unit_id, Type::Tuple(vec![]));
                return unit_id;
            }

            // SPECIAL HANDLING: source(it) — dereference an iterator (read value)
            if method == "source" && receiver.is_none() && args.len() == 1 {
                let it_id = self.lower_expr(&args[0]);
                // Dereference: read value through the iterator
                self.exprs.insert(
                    id,
                    MirExpr::Deref {
                        addr_id: it_id,
                        pointee_width: 8,
                    },
                );
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // SPECIAL HANDLING: spawn(fn(arg,...)) — async task spawn.
            // Parses as Call{method:"spawn", args:[Call{method:"worker",...}]}.
            // Must NOT evaluate the inner call synchronously; instead emit a
            // dedicated SpawnStmt that codegen lowers into a thunk + pthread.
            if method == "spawn" && receiver.is_none() && args.len() == 1 {
                if let AstNode::Call {
                    method: inner_fn,
                    args: inner_args,
                    ..
                } = &args[0]
                {
                    let mut inner_arg_ids = vec![];
                    for a in inner_args {
                        inner_arg_ids.push(self.lower_expr(a));
                    }
                    let spawn_dest = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: format!("__spawn_thunk_{}", inner_fn),
                        args: inner_arg_ids,
                        dest: spawn_dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(spawn_dest, MirExpr::Var(spawn_dest));
                    self.type_map.insert(spawn_dest, Type::I64);
                    return spawn_dest;
                }
            }

            // SPECIAL HANDLING: join(handle) — wait for spawn'd task.
            if method == "join" && receiver.is_none() && args.len() == 1 {
                let handle_id = self.lower_expr(&args[0]);
                let join_dest = self.emit_call("join", vec![handle_id], Type::I64);
                return join_dest;
            }

            // SPECIAL HANDLING: sink(it, val) — write to an iterator position
            if method == "sink" && receiver.is_none() && args.len() == 2 {
                let it_id = self.lower_expr(&args[0]);
                let val_id = self.lower_expr(&args[1]);
                self.stmts.push(MirStmt::Store {
                    addr_id: it_id,
                    val_id,
                    pointee_width: 8,
                });
                let unit_id = self.next_id();
                self.exprs.insert(unit_id, MirExpr::IntLit(0));
                self.type_map.insert(unit_id, Type::Tuple(vec![]));
                return unit_id;
            }

            // SPECIAL HANDLING: begin(r) / end(r) — get iterators from a range/container
            if (method == "begin" || method == "end") && receiver.is_none() && args.len() == 1 {
                // For now, begin returns the pointer to start, end returns pointer past end
                // Simplified: just pass through the container pointer
                let val_id = self.lower_expr(&args[0]);
                let zero_id = self.next_id();
                self.exprs.insert(zero_id, MirExpr::IntLit(0));
                self.type_map.insert(zero_id, Type::I64);
                self.exprs.insert(
                    id,
                    MirExpr::BinaryOp {
                        op: "+".to_string(),
                        left: val_id,
                        right: zero_id,
                    },
                );
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // SPECIAL HANDLING: advance(it, n) — advance iterator by n (with concept dispatch)
            if method == "advance" && receiver.is_none() && args.len() == 2 {
                let it_id = self.lower_expr(&args[0]);
                let n_id = self.lower_expr(&args[1]);
                self.exprs.insert(
                    id,
                    MirExpr::BinaryOp {
                        op: "+".to_string(),
                        left: it_id,
                        right: n_id,
                    },
                );
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // PY-4: Python-style free-function `len(x)` — dispatch by
            // argument type: literal-size arrays resolve at compile time,
            // strings → str_len, others → array_len runtime stub.
            // PY-A: the builtin `format(value, spec)` — `format(cost, '.2f')`.
            // It emitted a FREE CALL named `format` (undefined symbol, 7
            // corpus sites). Lower it as `str(value)`: the value is kept and
            // only the presentation is lost (V1 has no spec engine), and say
            // so once — never silent.
            if method == "format" && receiver.is_none() && args.len() == 2 {
                static WARNED_FMTB: std::sync::OnceLock<()> = std::sync::OnceLock::new();
                WARNED_FMTB.get_or_init(|| {
                    eprintln!(
                        "warning: PY-A: builtin format(value, spec) ignores the spec \
                         (the value is kept)"
                    );
                });
                return self.lower_expr(&AstNode::Call {
                    receiver: None,
                    method: "str".to_string(),
                    args: vec![args[0].clone()],
                    type_args: vec![],
                    structural: false,
                });
            }
            // PY-A: `object()` — Python's bare object is exactly the opaque
            // platform handle the runtime already provides. Without this the
            // call degraded to a free call named `object` and failed at LINK
            // time (3 corpus call sites).
            if method == "object" && receiver.is_none() && args.is_empty() {
                let z = self.next_id_with_lit(0);
                self.emit_call_into(id, "zeta_platform_obj", vec![z, z, z, z], Type::I64);
                return id;
            }
            // PY-A: `getattr(obj, "field")` with a LITERAL name is Python's
            // static attribute access — safe to rewrite to a field access
            // ONLY when the receiver's handle tag is statically known and the
            // member is registered (that check is what makes it safe: an
            // unknown receiver would silently read garbage, see batch 26).
            // Otherwise fall through to the compile-time diagnostic below.
            if method == "getattr" && receiver.is_none() && (2..=3).contains(&args.len()) {
                // 批次 900：getattr 字面量 face 迁入 gen/call_getattr.rs（顺序保持）。
                if let Some(r) = self.lower_getattr_literal(receiver, method, args, id) {
                    return r;
                }
            }
            // PY-A: the builtin `slice(a, b)` / `slice(a, b, step)` —
            // `slice_obj = slice(start_idx, last_idx + 1)` in the corpus
            // emitted a free call `slice` (undefined symbol, 3 sites). Route
            // to the SAME opaque slice handle the comma-subscript path uses
            // (`py_slice_new`, batch 5), so both spellings agree.
            if method == "slice" && receiver.is_none() && (1..=3).contains(&args.len()) {
                let mut ids: Vec<u32> = args.iter().map(|a| self.lower_expr(a)).collect();
                while ids.len() < 3 {
                    let fill = self.next_id_with_lit(if ids.len() == 2 { 1 } else { 0 });
                    ids.push(fill);
                }
                self.emit_call_into(id, "py_slice_new", vec![ids[0], ids[1], ids[2]], Type::I64);
                return id;
            }
            // PY-A: `range(n)` as a VALUE (`xs = range(5)`) — materialize it
            // with the existing `zeta_arange` (a Vec of [0..n)), instead of
            // emitting a free call named `range`. 2/3-arg forms need
            // offset/step and stay fail-loud below.
            // `range(a, b)` as a value: a Vec of [a, b) — `zeta_arange` only
            // covers [0, n), hence the `_from` variant. 3-arg (step) forms
            // stay fail-loud below.
            // `range(a, b, step)` as a value. Positive steps only: a literal
            // step <= 0 is reported here (Python allows negative steps but
            // they need a descending Vec) instead of silently producing an
            // ascending one.
            if method == "range" && receiver.is_none() && args.len() == 3 {
                if let AstNode::Lit(n) = &args[2] {
                    if *n <= 0 {
                        eprintln!(
                            "error: range(a, b, step) with a non-positive literal step \
                             is not implemented (negative steps need a descending Vec)"
                        );
                    }
                }
                let a0 = self.lower_expr(&args[0]);
                let a1 = self.lower_expr(&args[1]);
                let a2 = self.lower_expr(&args[2]);
                self.stmts.push(MirStmt::Call {
                    func: "zeta_arange_step".to_string(),
                    args: vec![a0, a1, a2],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                return id;
            }
            if method == "range" && receiver.is_none() && args.len() == 2 {
                let a0 = self.lower_expr(&args[0]);
                let a1 = self.lower_expr(&args[1]);
                self.stmts.push(MirStmt::Call {
                    func: "zeta_arange_from".to_string(),
                    args: vec![a0, a1],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                return id;
            }
            if method == "range" && receiver.is_none() && args.len() == 1 {
                let n = self.lower_expr(&args[0]);
                self.stmts.push(MirStmt::Call {
                    func: "zeta_arange".to_string(),
                    args: vec![n],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                return id;
            }
            // 批次 824：入口判定改读分类器（classify_call）——路由决策
            // 收敛到唯一决策点；本臂只负责 Len 路由的发射。
            // 批次 840：len 臂整体迁入 gen/call_len.rs（分类函数与发射
            // 同文件）；入口判定读分类器。
            if classify_call(method) == CallClass::Len
                && receiver.is_none()
                && args.len() == 1
            {
                if let Some(nid) = self.lower_len(args, id) {
                    return nid;
                }
            }

            // PY-A: `Some(v)` / `Ok(v)` / `Err(e)` free-call form —
            // enum variant constructors without a path.
            //
            // The block written here MUST be the block the runtime readers
            // decode: `option_is_some` tests slot 0 and `option_get_data`
            // reads slot 1 (tokio_runtime_stub.c), i.e. `[tag | data]`.
            // Emitting only the payload used to make every `Some(7)` read
            // back as "not some" (`7 == 1` is false) and, worse, as
            // "some" whenever the payload happened to be 1 —
            // `match Some(7) { Some(n) => n + 100, … }` took the wildcard.
            if receiver.is_none()
                && matches!(method.as_str(), "Some" | "Ok" | "Err")
                && args.len() == 1
            {
                let val_id = self.lower_expr(&args[0]);
                let (variant, tag_val) = match method.as_str() {
                    "Some" => ("Option::Some", 1),
                    "Ok" => ("Result::Ok", 1),
                    _ => ("Result::Err", 0),
                };
                // `Option` is `[tag | data]`, `Result` is `[tag | ok | err]`
                // (the layouts `option_get_data` / `host_result_get_data`
                // decode), so an unused payload slot is written as 0.
                let tag_id = self.int_slot(tag_val);
                let z = self.int_slot(0);
                let fields = match variant {
                    "Option::Some" => vec![
                        ("__tag".to_string(), tag_id),
                        ("f0".to_string(), val_id),
                    ],
                    "Result::Ok" => vec![
                        ("__tag".to_string(), tag_id),
                        ("f0".to_string(), val_id),
                        ("f1".to_string(), z),
                    ],
                    _ => vec![
                        ("__tag".to_string(), tag_id),
                        ("f0".to_string(), z),
                        ("f1".to_string(), val_id),
                    ],
                };
                self.exprs.insert(
                    id,
                    MirExpr::Struct {
                        variant: variant.to_string(),
                        fields,
                    },
                );
                self.type_map.insert(id, Type::I64);
                return id;
            }
            // `None()` free-call
            if receiver.is_none() && method == "None" && args.is_empty() {
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // 批次 834：assert 臂迁入 gen/call_assert.rs。
            if classify_call(method) == CallClass::Assert
                && method == "assert"
                && receiver.is_none()
            {
                if let Some(unit_id) = self.lower_assert(args) {
                    return unit_id;
                }
            }

            // 批次 830：abs/sum 臂体迁入 gen/call_num.rs（家族执行文件）；
            // 入口判定（分类器）已在此前接好，这里只调用执行者。
            if classify_call(method) == CallClass::NumericBuiltin
                && receiver.is_none()
                && (method == "abs" || method == "sum")
                && args.len() == 1
            {
                if let Some(nid) = self.lower_numeric_builtin(method, &args, id) {
                    return nid;
                }
            }

            if receiver.is_none()
                && (method == "min" || method == "max")
                && args.len() >= 2
            {
                // 批次 826：入口判定改读分类器（NumericBuiltin）。
                // A `key=` keyword selects the ITERABLE form; handle it
                // here, before the min-of-two path treats the callable as a
                // value (which returned the function pointer as the result).
                if let AstNode::Call {
                    receiver: None,
                    method: m,
                    args: ka,
                    ..
                } = &args[1]
                {
                    if m == "__kwarg__"
                        && ka.len() == 2
                        && matches!(&ka[0], AstNode::StringLit(n) if n == "key")
                    {
                        let xs = self.lower_expr(&args[0]);
                        let f = self.lower_expr(&ka[1]);
                        let func = if method == "min" {
                            "py_min_key"
                        } else {
                            "py_max_key"
                        };
                        self.stmts.push(MirStmt::Call {
                            func: func.to_string(),
                            args: vec![xs, f],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
                        return id;
                    }
                }
                // 批次146: N 参 min/max（max(1, 5, 3)）—— 两两折叠；此前
                // 仅 2 参，3 参落裸名 `_min`/`_max` 链接失败（t230）。
                let ids: Vec<u32> = args.iter().map(|a| self.lower_expr(a)).collect();
                let any_f = ids.iter().any(|i| {
                    matches!(self.type_map.get(i), Some(Type::F64) | Some(Type::F32))
                });
                let stem = if method == "min" { "zeta_min" } else { "zeta_max" };
                let mut acc = ids[0];
                for (k, &arg) in ids.iter().enumerate().skip(1) {
                    let last = k + 1 == ids.len();
                    let dest = if last { id } else { self.next_id() };
                    if any_f {
                        // f64 via llvm.minnum/maxnum intrinsics (double args)
                        let intr = format!(
                            "llvm.{}.f64",
                            if method == "min" { "minnum" } else { "maxnum" }
                        );
                        self.stmts.push(MirStmt::Call {
                            func: intr,
                            args: vec![acc, arg],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map.insert(dest, Type::F64);
                    } else {
                        self.stmts.push(MirStmt::Call {
                            func: format!("{}_{}", stem, "i64"),
                            args: vec![acc, arg],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        // Batch 564: min/max over ALL-Bool operands is a
                        // Bool (`min(1 == 2, 3 == 4)` is False) — typing
                        // it I64 made print render `0`. A Bool mixed with
                        // an int keeps I64 (the int result would misrender
                        // as True/False).
                        let both_bool = matches!(
                            self.type_map.get(&acc),
                            Some(Type::Bool)
                        ) && matches!(self.type_map.get(&arg), Some(Type::Bool));
                        self.type_map
                            .insert(dest, if both_bool { Type::Bool } else { Type::I64 });
                    }
                    acc = dest;
                }
                return id;
            }
            if method == "sum" && crate::diagnostics::env_flag("ZETA_PROBE") {
                eprintln!("PROBE sum seen, receiver_none={} args={}", receiver.is_none(), args.len());
            }
            // 已知 struct（或可从构造调用恢复类名）→ 改写为 FieldAccess，
            // 复用字段/零参方法分派与类型；字段不存在 → default；
            // 未跟踪接收者 → 有 default 用 default（一次告警）；
            // 无 default 或动态名 → py_getattr_dynamic 运行期响亮 abort。
            if receiver.is_none() && method == "getattr" && (args.len() == 2 || args.len() == 3)
            {
                // 批次 900：getattr struct face 迁入 gen/call_getattr.rs（顺序保持）。
                if let Some(r) = self.lower_getattr_struct(receiver, method, args, id) {
                    return r;
                }
            }
            // Batch 405: this guard used to sit ABOVE the `getattr` arms, so
            // it rang for forms those arms then implemented — 5 of the 6
            // corpus lines were false alarms (compile rc=0, no undefined
            // `getattr` in the binary). It now runs only on the ghost path.
            // Unimplemented builtins that would otherwise emit a FREE CALL
            // named after themselves (an undefined symbol at link time, with
            // zero information about the cause). Ring the bell at COMPILE
            // time instead; the symbol is still emitted so behaviour is
            // unchanged, but the message names the culprit.
            if receiver.is_none()
                && ((method == "getattr" && !args.is_empty())
                    || (method == "range" && !args.is_empty()))
            {
                // PY-A (batch 288): the statement path reaches this guard
                // for dynamic-name getattr (`getattr(import_module(src),
                // name)` in market_data's PEP 562 facade, and
                // `getattr(bs, MAP[api])` in fundamental_data). The lower_expr
                // route already maps those to py_getattr_dynamic; here the
                // bare `getattr` free call leaked an undefined symbol and
                // killed LINKING for the whole program. Route it the same
                // way. Literal names keep the ghost path (t225 expects the
                // link failure).
                if method == "getattr"
                    && args.len() >= 2
                    && !matches!(args[1], AstNode::StringLit(_))
                {
                    let a0 = self.lower_expr(&args[0]);
                    let a1 = self.lower_expr(&args[1]);
                    self.emit_call_into(id, "py_getattr_dynamic", vec![a0, a1], Type::I64);
                    return id;
                }
                eprintln!(
                    "error: builtin `{}` is not implemented in this form (it would link \
                     against an undefined symbol named `{}`)",
                    method, method
                );
            }
            // 批次145 重放(批次123): builtin next(it[, default]) + opaque .next().
            if receiver.is_none() && method == "next" && !args.is_empty() {
                if args.len() >= 2 {
                    eprintln!("warning: PY-A: next(it, default) returns the default (V1)");
                    return self.lower_expr(&args[1]);
                }
                let aid = self.lower_expr(&args[0]);
                self.emit_call_into(id, "py_builtin_next", vec![aid], Type::I64);
                return id;
            }
            if method == "next" && receiver.is_some() {
                // 已知 struct 的 next 方法 → 限定调用；否则 opaque → 0 (exhausted)。
                let sid = self.lower_expr(receiver.as_ref().unwrap());
                let known_next = match self.type_map.get(&sid).cloned() {
                    Some(Type::Named(tn, _)) => {
                        self.func_ret_types.contains_key(&format!("{}::next", tn))
                    }
                    _ => false,
                };
                if !known_next {
                    self.emit_call_into(id, "py_method_next", vec![sid], Type::I64);
                    return id;
                }
            }
            // PY-A: min(xs, key=f) / max(xs, key=f) — linear scan calling
            // the key once per element (ties keep the first, like Python).
            if receiver.is_none()
                && (method == "min" || method == "max")
                && args.len() >= 2
            {
                let keyf = match &args[1] {
                    AstNode::Call {
                        receiver: None,
                        method: m,
                        args: ka,
                        ..
                    } if m == "__kwarg__"
                        && ka.len() == 2
                        && matches!(&ka[0], AstNode::StringLit(n) if n == "key") =>
                    {
                        Some(ka[1].clone())
                    }
                    _ => None,
                };
                if let Some(k) = keyf {
                    let xs = self.lower_expr(&args[0]);
                    let f = self.lower_expr(&k);
                    let func = if method == "min" {
                        "py_min_key"
                    } else {
                        "py_max_key"
                    };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: vec![xs, f],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
            }
            // PY-A: max(xs) / min(xs) — the 1-argument form (previously a
            // bare `max`/`min` extern → link failure).
            // 批次 925：f64 元素走 f64 比较版（元素按 f64 位模式存取，此前
            // 只有 i64 版＋warning 提示绕行，min([1.5,2.5]) 打 1.5 的位模式
            // 4609434218613702656）；元素型 type_map 优先、checker_env 兜底。
            if receiver.is_none()
                && (method == "max" || method == "min")
                && args.len() == 1
            {
                let a = self.lower_expr(&args[0]);
                let elem_is_float = match self.type_map.get(&a) {
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                        matches!(**e, Type::F64 | Type::F32)
                    }
                    _ => match &args[0] {
                        AstNode::Var(nm) => matches!(
                            self.checker_type_of(nm),
                            Some(Type::DynamicArray(e))
                                if matches!(*e, Type::F64 | Type::F32)
                        ),
                        _ => false,
                    },
                };
                let (func, dest_f64) =
                    minmax_builtin_target(&method, elem_is_float);
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: vec![a],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(
                    id,
                    if dest_f64 { Type::F64 } else { Type::I64 },
                );
                return id;
            }
            // PY-A: isinstance(x, T) — the value's STATIC type decides.
            // Only the builtin names and the exact class name are claimed;
            // anything else falls through (loud) rather than guessing.
            if receiver.is_none() && method == "isinstance" && args.len() == 2 {
                if let AstNode::Var(tn) = &args[1] {
                    let val_id = self.lower_expr(&args[0]);
                    let mut vt = self.type_map.get(&val_id).cloned();
                    // 批 943：checker 兜底——槽型是 ABI 缺省（I64/PyDynamic）
                    // 或未知时不可信（数组也按 i64 指针传参），checker_env 的
                    // 具名型（用户类/容器，来自调用点证据链）更可信
                    if let AstNode::Var(vn) = &args[0] {
                        let abi_default = matches!(
                            &vt,
                            None | Some(Type::I64) | Some(Type::PyDynamic)
                        );
                        if abi_default {
                            if let Some(ct) = self.checker_type_of(vn) {
                                if matches!(ct, Type::Named(_, _)) {
                                    vt = Some(ct);
                                }
                            }
                        }
                    }
                    // A parsed JSON value's Python type is only known at
                    // RUNTIME — one static PyJson tag covers objects, arrays,
                    // strings and numbers. Answering statically returned 0, so
                    // `if isinstance(data, dict) and len(data) > 50:` fell
                    // through to the platform path: the ETF listing cache was
                    // parsed CORRECTLY (1724 keys) and then ignored, and the
                    // fallback crashed. Dispatch on the runtime tag instead
                    // (ZJ_INT 1 / ZJ_F64 2 / ZJ_STR 3 / ZJ_ARR 4 / ZJ_OBJ 5).
                    if matches!(&vt, Some(Type::Named(n, _)) if n == "PyJson") {
                        let tag = match tn.as_str() {
                            "int" => Some(1i64),
                            "float" => Some(2),
                            "str" | "str_holder" => Some(3),
                            "list" => Some(4),
                            "dict" => Some(5),
                            _ => None,
                        };
                        if let Some(t) = tag {
                            let vid = self.next_id();
                            self.exprs.insert(vid, MirExpr::Var(val_id));
                            self.type_map.insert(vid, vt.clone().unwrap_or(Type::slot_fallback()));
                            let tid = self.next_id();
                            self.exprs.insert(tid, MirExpr::IntLit(t));
                            self.type_map.insert(tid, Type::I64);
                            self.emit_call_into(id, "py_json_is_kind", vec![vid, tid], Type::Bool);
                            return id;
                        }
                    }
                    let hit = match tn.as_str() {
                        "int" => matches!(
                            vt,
                            Some(Type::I64)
                                | Some(Type::I32)
                                | Some(Type::I8)
                                | Some(Type::I16)
                                | Some(Type::U8)
                                | Some(Type::U16)
                                | Some(Type::U32)
                                | Some(Type::U64)
                                | Some(Type::Usize)
                                // 批次147: PyDynamic 值按 i64 ABI 传递——
                                // B3 引入后未标注形参的 isinstance(x, int)
                                // 落 0（t87 f(5)）；按 ABI 现实判 int。
                                | Some(Type::PyDynamic)
                        ),
                        "float" => matches!(vt, Some(Type::F64) | Some(Type::F32)),
                        "str" => matches!(vt, Some(Type::Str)),
                        "bool" => matches!(vt, Some(Type::Bool)),
                        "list" => {
                            matches!(vt, Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)))
                        }
                        "dict" => matches!(&vt, Some(Type::Named(n, _)) if n == "map"),
                        other => matches!(&vt, Some(Type::Named(n, _)) if n == other),
                    };
                    self.exprs.insert(id, MirExpr::IntLit(hit as i64));
                    self.type_map.insert(id, Type::Bool);
                    return id;
                }
            }
            // PY-A: hex/oct/bin(n) and reversed(xs) — all four were bare
            // externs (link failure; `bin`/`hex`/`oct` have no libc symbol).
            if receiver.is_none()
                && matches!(method.as_str(), "hex" | "oct" | "bin")
                && args.len() == 1
            {
                let a = self.lower_expr(&args[0]);
                let func = match method.as_str() {
                    "hex" => "py_builtin_hex",
                    "oct" => "py_builtin_oct",
                    _ => "py_builtin_bin",
                };
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: vec![a],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::Str);
                return id;
            }
            // Batch 754 (#265 后续): tuple(xs) — a bare `tuple` extern
            // (link failure, `_tuple` undefined in wufu_strategy
            // `_log_rebalance`). A tuple IS a dynamic array in the value
            // model, so the constructor is the identity on its argument;
            // keep the element type.
            if receiver.is_none() && method == "tuple" && args.len() == 1 {
                let a = self.lower_expr(&args[0]);
                let ty = self.type_map.get(&a).cloned().unwrap_or_else(Type::slot_fallback);
                self.exprs.insert(id, MirExpr::Var(a));
                self.type_map.insert(id, ty);
                return id;
            }
            if receiver.is_none() && method == "reversed" && args.len() == 1 {
                let a = self.lower_expr(&args[0]);
                let elem = match self.type_map.get(&a).cloned() {
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => *e,
                    _ => Type::I64,
                };
                self.stmts.push(MirStmt::Call {
                    func: "py_builtin_reversed".to_string(),
                    args: vec![a],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(elem)));
                return id;
            }
            // PY-A: bool(x) — truthiness for containers (length) and
            // numbers (non-zero). Previously a bare `bool` extern.
            if receiver.is_none() && method == "bool" && args.len() == 1 {
                let a = self.lower_expr(&args[0]);
                let len_func = match self.type_map.get(&a).cloned() {
                    Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)) => Some("array_len"),
                    Some(Type::Str) => Some("str_len"),
                    Some(ty) if ty.is_map() => Some("zeta_map_len"),
                    _ => None,
                };
                let lhs = if let Some(f) = len_func {
                    let lid = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: f.to_string(),
                        args: vec![a],
                        dest: lid,
                        type_args: vec![],
                    });
                    self.exprs.insert(lid, MirExpr::Var(lid));
                    self.type_map.insert(lid, Type::I64);
                    lid
                } else {
                    a
                };
                let zero = self.next_id();
                self.exprs.insert(zero, MirExpr::IntLit(0));
                self.type_map.insert(zero, Type::I64);
                self.exprs.insert(
                    id,
                    MirExpr::BinaryOp {
                        op: "!=".to_string(),
                        left: lhs,
                        right: zero,
                    },
                );
                self.type_map.insert(id, Type::Bool);
                return id;
            }
            // PY-A: int(text, base) / dict.fromkeys(keys, val) — both
            // previously fell to bare externs (link failure).
            if receiver.is_none() && method == "int" && args.len() == 2 {
                let a = self.lower_expr(&args[0]);
                let b = self.lower_expr(&args[1]);
                self.emit_call_into(id, "py_int_base", vec![a, b], Type::I64);
                return id;
            }
            if method == "fromkeys" && (args.len() == 1 || args.len() == 2) {
                let is_dict = receiver.is_none()
                    || matches!(receiver.as_deref(), Some(AstNode::Var(v)) if v == "dict");
                if is_dict {
                    // 批次 804（#113 fromkeys 格）：显式 None / 1 参缺省 /
                    // NoneVar 名 ⇒ 值全 None——结果型 map[K, NoneValue]，
                    // 读边界元素型块（本文件 Named("map",targs).get(1) 臂）
                    // 让 `d["k"]` 带上 NoneValue，print 按型渲染 "None"
                    //（654 NoneVar 按名渲染的按型同款）。值仍是 i64 0，
                    // 算术/比较面不改（CPython 会 TypeError 的形不在格内）。
                    let none_val = match args.get(1) {
                        None => true,
                        Some(AstNode::NoneLit) => true,
                        Some(AstNode::Var(v)) if self.none_vars_gen.contains(v) => true,
                        _ => false,
                    };
                    // A LITERAL key list is built inline as a dict literal:
                    // a literal list lowers to a StackArray (no `[cap|len]`
                    // header), so the runtime helper read a garbage length and
                    // walked off the buffer — measured as a SEGV inside
                    // `py_map_fromkeys` during `wufu_constants' module init`.
                    if let AstNode::ArrayLit(items) = &args[0] {
                        let val_expr = match args.get(1) {
                            Some(v) => v.clone(),
                            None => AstNode::Lit(0),
                        };
                        let did = self.lower_expr(&AstNode::DictLit {
                            entries: items
                                .iter()
                                .map(|k| (k.clone(), val_expr.clone()))
                                .collect(),
                        });
                        if none_val {
                            self.type_map.insert(
                                did,
                                Type::Named(
                                    "map".to_string(),
                                    vec![Type::Str, Type::Named("NoneValue".to_string(), vec![])],
                                ),
                            );
                        }
                        return did;
                    }
                    let keys = self.lower_expr(&args[0]);
                    let keys_are_str = matches!(
                        self.type_map.get(&keys),
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _))
                            if matches!(**e, Type::Str)
                    );
                    // `dict.fromkeys(keys)` — Python defaults the value to
                    // None; the i64 model uses 0. Without this the 1-arg
                    // form fell to a bare `_fromkeys` (link failure, t231).
                    let val = match args.get(1) {
                        Some(v) => self.lower_expr(v),
                        None => {
                            let z = self.next_id();
                            self.exprs.insert(z, MirExpr::IntLit(0));
                            self.type_map.insert(z, Type::I64);
                            z
                        }
                    };
                    let flag = self.next_id();
                    self.exprs.insert(flag, MirExpr::IntLit(keys_are_str as i64));
                    self.type_map.insert(flag, Type::I64);
                    self.stmts.push(MirStmt::Call {
                        func: "py_map_fromkeys".to_string(),
                        args: vec![keys, val, flag],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    let key_ty = if keys_are_str { Type::Str } else { Type::I64 };
                    self.type_map.insert(
                        id,
                        if none_val {
                            Type::Named(
                                "map".to_string(),
                                vec![key_ty, Type::Named("NoneValue".to_string(), vec![])],
                            )
                        } else {
                            Type::Named("map".to_string(), vec![key_ty])
                        },
                    );
                    return id;
                }
            }
            // PY-A: round(x[, n]). Without this the 2-argument form fell to
            // a bare libm `round` extern whose first argument had been
            // coerced to i64 (2.345 became 2), and round(2.5) gave 3.
            if receiver.is_none()
                && method == "round"
                && (args.len() == 1 || args.len() == 2)
            {
                let x = self.lower_expr(&args[0]);
                let is_float = matches!(
                    self.type_map.get(&x),
                    Some(Type::F64) | Some(Type::F32)
                );
                if !is_float {
                    // round(int) is the integer itself.
                    return x;
                }
                if args.len() == 1 {
                    self.emit_call_into(id, "py_round_i64", vec![x], Type::I64);
                } else {
                    let n = self.lower_expr(&args[1]);
                    self.emit_call_into(id, "py_round_n", vec![x, n], Type::F64);
                }
                return id;
            }
            // PY-A: repr(x) / set(xs) — repr quotes strings; set returns a
            // deduplicated Vec (V1: no add/remove).
            if receiver.is_none() && method == "repr" && args.len() == 1 {
                let a = self.lower_expr(&args[0]);
                let (func, ty) = match self.type_map.get(&a).cloned() {
                    Some(Type::Str) => ("py_repr_str", Type::Str),
                    Some(Type::F64) | Some(Type::F32) => ("to_string_f64", Type::Str),
                    Some(Type::Bool) => ("to_string_bool", Type::Str),
                    // Batch 653 (#117): dynamic values need GC-geometry conversion.
                    Some(Type::PyDynamic) => ("zeta_dyn_to_string", Type::Str),
                    _ => ("to_string_i64", Type::Str),
                };
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: vec![a],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, ty);
                return id;
            }
            if receiver.is_none() && method == "set" && args.len() <= 1 {
                // `set()` with no argument — empty set. Zeta has no set
                // type (set literals already degrade to lists, batch 11),
                // so this is an empty dynamic array. Without the 0-arg case
                // the call emitted a bare `_set` (link failure, t246/t231).
                // `py_builtin_set(0)` is null-safe (`zt_vec_len(0) == 0`).
                let (a, elem) = match args.first() {
                    Some(arg) => {
                        let a = self.lower_expr(arg);
                        let elem = match self.type_map.get(&a).cloned() {
                            Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => *e,
                            _ => Type::I64,
                        };
                        (a, elem)
                    }
                    None => {
                        let z = self.next_id();
                        self.exprs.insert(z, MirExpr::IntLit(0));
                        self.type_map.insert(z, Type::I64);
                        (z, Type::I64)
                    }
                };
                // Batch 809 (#267①): the constructor deduped by slot WORD,
                // but a `vec<str>` slot holds a pointer, so `set(["a","a","b"])`
                // kept three elements — every `list(set(pool))` over ticker
                // strings in the real corpus silently kept its duplicates.
                // The flag tells the runtime to compare text by CONTENT, the
                // same rule `py_list_contains` uses for membership.
                let elem_is_str = matches!(elem, Type::Str);
                let flag = self.next_id();
                self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                self.type_map.insert(flag, Type::I64);
                self.stmts.push(MirStmt::Call {
                    func: "py_builtin_set".to_string(),
                    args: vec![a, flag],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(elem)));
                return id;
            }
            // 批次 841：内建函数族第一片迁入 gen/call_builtin.rs
            //（map/filter/chr/ord/divmod/dict；zip 起余段批 842 续迁）。
            if receiver.is_none() {
                if let Some(nid) = self.lower_builtin_1(receiver, method, args, id) {
                    return nid;
                }
            }
            // 批次 842：内建族第二片（zip/any/all/enumerate/list/
            // float/sorted 表段）迁入 gen/call_builtin.rs。
            if receiver.is_none() {
                if let Some(nid) = self.lower_builtin_2(receiver, method, args, id) {
                    return nid;
                }
            }
            if method == "str" && receiver.is_none() && args.len() == 1 {

                        // Batch 659: a map Subscript behind a Str-refined
                        // value type renders through the per-key tag side
                        // table — the raw word otherwise reaches string
                        // consumption as a bogus pointer.
                        let mapsub_render = |g: &mut Self, b: &AstNode, k: &AstNode| -> Option<u32> {
                            let is_map_str = if let AstNode::Var(vname) = b {
                                g.name_to_id.get(vname.as_str()).map_or(false, |&sid| {
                                    g.type_map.get(&sid).map_or(false, |ty| {
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
                            if !is_map_str {
                                return None;
                            }
                            let recv_id = g.lower_expr(b);
                            let k_id = g.lower_expr(k);
                            let r_id = g.next_id();
                            g.stmts.push(MirStmt::Call {
                                func: "zeta_map_get_render".to_string(),
                                args: vec![recv_id, k_id],
                                dest: r_id,
                                type_args: vec![],
                            });
                            g.exprs.insert(r_id, MirExpr::Var(r_id));
                            g.type_map.insert(r_id, Type::Str);
                            Some(r_id)
                        };
                // Batch 659: `str(d[k])` on a Str-refined map — per-key
                // tag render (the raw word would strlen as a pointer).
                if let AstNode::Subscript { base, index } = &args[0] {
                    if let Some(r_id) = mapsub_render(self, base, index) {
                        self.exprs.insert(id, MirExpr::Var(r_id));
                        self.type_map.insert(id, Type::Str);
                        return id;
                    }
                }
                // Batch 624: `str(None)` literal face renders "None"
                // (value representation stays 0, #113/#189 deep water).
                // The constant must reach the result through a DEFINING
                // stmt — aliasing a bare constant id reads an
                // uninitialized alloca (the BATCH-296 trap below).
                if matches!(args[0], AstNode::NoneLit) {
                    let cid = self.next_id();
                    self.exprs
                        .insert(cid, MirExpr::StringLit("None".to_string()));
                    self.type_map.insert(cid, Type::Str);
                    let nid = self.emit_call("zeta_identity", vec![cid], Type::Str);
                    self.exprs.insert(id, MirExpr::Var(nid));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                let arg_id = self.lower_expr(&args[0]);
                // Batch 647: `str(<BigInt>)` renders the decimal value.
                if matches!(
                    self.type_map.get(&arg_id),
                    Some(Type::Named(n, _)) if n == "BigInt"
                ) {
                    let nid = self.emit_call("zeta_big_to_string", vec![arg_id], Type::Str);
                    self.exprs.insert(id, MirExpr::Var(nid));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                // Batch 626: `str(<list>)` — the same CPython repr the
                // print face renders (py_json_dumps_vec_typed, batches
                // 557/565). lower_to_string on a vec handle printed the
                // raw address (type_conversion_gaps diff line #3).
                if let Some(tags) = match self.type_map.get(&arg_id) {
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                        Some(Self::elem_tag_string(e))
                    }
                    _ => None,
                } {
                    // Batch 633: nested lists (`[[1, 2], [3]]`) carry a
                    // RECURSIVE tag string ("4,0" = list of int-lists);
                    // the flat forms keep the typed dumps call.
                    let (fname, tags_arg): (&str, MirExpr) = if tags.len() == 1 {
                        ("py_json_dumps_vec_typed", MirExpr::IntLit(
                            tags.as_bytes()[0].wrapping_sub(b'0') as i64,
                        ))
                    } else {
                        ("py_json_dumps_vec_nested", MirExpr::StringLit(tags.clone()))
                    };
                    let tid = self.next_id();
                    self.exprs.insert(tid, tags_arg);
                    if fname.ends_with("_typed") {
                        self.type_map.insert(tid, Type::I64);
                    } else {
                        self.type_map.insert(tid, Type::Str);
                    }
                    let nid = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: fname.to_string(),
                        args: vec![arg_id, tid],
                        dest: nid,
                        type_args: vec![],
                    });
                    self.exprs.insert(nid, MirExpr::Var(nid));
                    self.type_map.insert(nid, Type::Str);
                    self.exprs.insert(id, MirExpr::Var(nid));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                // A Json value knows its own type: stringify by tag.
                if matches!(
                    self.type_map.get(&arg_id),
                    Some(Type::Named(n, _)) if n == "PyJson"
                ) {
                    let nid = self.emit_call("py_json_as_str", vec![arg_id], Type::Str);
                    self.exprs.insert(id, MirExpr::Var(nid));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                // BATCH-296: `str(<already a str>)` is the identity, but the
                // identity was lowered as a bare ALIAS (`Var(arg)`) with no
                // defining stmt — at module level nothing ever stored the
                // dest slot, so `x = str("ab")` printed from an uninitialized
                // alloca and `puts(0)` SEGFAULTed (measured: IR had
                // `%13 = load i64, ptr %0` with no preceding store). Route
                // through the identity helper so the slot is really written.
                let nid = if matches!(self.type_map.get(&arg_id), Some(Type::Str)) {
                    let sid = self.emit_call("zeta_identity", vec![arg_id], Type::Str);
                    sid
                } else {
                    self.lower_to_string(arg_id)
                };
                // `str(<PyPath>)` yields a STRING: the handle IS the path, so
                // `lower_to_string` passes it through unchanged. Only the
                // RESULT id is retyped — retyping the source id would make the
                // original PyPath slot dispatch as Str later, and routing
                // through a fresh id has no alloca (SEGV; measured as t73).
                // Without this, `self.cache_dir = cache_dir or str(_PROJECT_ROOT
                // / "data")` produced an integer-typed value and
                // `os.path.join(self.cache_dir, …)` dereferenced the number.
                let src_is_path =
                    matches!(self.type_map.get(&nid), Some(Type::Named(n, _)) if n == "PyPath");
                self.exprs.insert(id, MirExpr::Var(nid));
                let ty = if src_is_path {
                    Type::Str
                } else {
                    self.type_map.get(&nid).cloned().unwrap_or(Type::Str)
                };
                self.type_map.insert(id, ty);
                return id;
            }

            // PY-4/PY-A: Python-style `print(x...)` — the runtime's
            // legacy `print` symbol is string-only (fputs) and crashes on
            // ints. Python semantics: dispatch each arg by type, separate
            // args with a space, and end with a newline (the last arg goes
            // through the println_* family which bakes in the newline).
            // Multi-arg no longer routes through the fragile print.N C
            // alias table (which only emitted the first arg).
            // 批次 832：print 家族整体迁入 gen/call_print.rs（sep/end/
            // 多参/按格渲染）。入口判定读分类器。
            if classify_call(method) == CallClass::Print
                && receiver.is_none()
                && !args.is_empty()
            {
                self.lower_print(args, id);
                // 批 945：print 返回 None——lower_print 只发 VoidCall 不写
                // dest 槽，lower_expr 尾检见空就喊 W1010（每次 print 白喊
                // 声）。补与 fallback 同值的占位，语义不变、警告消失。
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // SPECIAL HANDLING: println generates VoidCall not Call.
            // PY-4: dispatch a single argument by type — strings go to
            // println_str, floats to println_f64.
            if method.as_str() == "println" && receiver.is_none() {
                let mut arg_ids = vec![];
                for a in args {
                    arg_ids.push(self.lower_expr(a));
                }

                // Batch 653 (#117): PyDynamic needs conversion to string first.
                if arg_ids.len() == 1 && matches!(self.type_map.get(&arg_ids[0]), Some(Type::PyDynamic)) {
                    let sid = self.emit_call("zeta_dyn_to_string", vec![arg_ids[0]], Type::Str);
                    arg_ids[0] = sid;
                }

                let func = if arg_ids.len() == 1 {
                    match self.type_map.get(&arg_ids[0]) {
                        Some(Type::Str) => "println_str",
                        Some(Type::Named(n, _)) if n == "PyPath" => "println_str",
                        Some(Type::F64) | Some(Type::F32) => "println_f64",
                        _ => "println",
                    }
                } else {
                    "println"
                };

                self.stmts.push(MirStmt::VoidCall {
                    func: func.to_string(),
                    args: arg_ids,
                });

                // Unit return
                let unit_id = self.next_id();
                self.exprs.insert(unit_id, MirExpr::IntLit(0));
                self.type_map.insert(unit_id, Type::Tuple(vec![]));
                return unit_id;
            }

            // SPECIAL HANDLING: If method is "call" and receiver is a function name,
            // generate direct function call instead of call_i64
            if method == "call" {
                // receiver is &Option<Box<AstNode>>
                if let Some(receiver_ast) = receiver {
                    // receiver_ast is &Box<AstNode>, dereference to &AstNode
                    let ast_ref: &AstNode = receiver_ast;
                    if let AstNode::Var(func_name) = ast_ref {
                        // Generate direct call to function
                        let mut arg_ids = vec![];
                        for a in args {
                            arg_ids.push(self.lower_expr(a));
                        }

                        // Convert type arguments from strings to Type objects
                        let mir_type_args: Vec<Type> =
                            type_args.iter().map(|t| Type::from_string(t)).collect();

                        self.stmts.push(MirStmt::Call {
                            func: func_name.clone(),
                            args: arg_ids,
                            dest: id,
                            type_args: mir_type_args,
                        });

                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
                        return id;
                    }
                    // Not a variable, fall through
                }
            }

            let mut arg_ids = vec![];
            let receiver_ty = if let Some(r) = receiver {
                let rid = self.lower_expr(r);
                arg_ids.push(rid);
                Some(self.type_map.get(&rid).cloned().unwrap_or_else(Type::slot_fallback))
            } else {
                None
            };
            // PY-A: keyword arguments — bind by parameter name when the                // PY-A: keyword arguments — bind by parameter name when the
            // callee's signature is known (Python semantics). The parser
            // wraps `name=value` as a __kwarg__ marker.
            let ordered_args: Vec<AstNode> = {
                let mut pos: Vec<AstNode> = Vec::new();
                let mut kw: Vec<(String, AstNode)> = Vec::new();
                // PY-A: `f(**mapping)`. The mapping's keys only exist at
                // runtime, but the callee's parameter NAMES are static, so
                // the call expands to `f(m["a"], m["b"])`.
                let mut spread: Vec<AstNode> = Vec::new();
                for a in args {
                    match a {
                        AstNode::Call {
                            receiver: None,
                            method,
                            args: ka,
                            ..
                        } if method == "zeta_kwargs_unpack" && ka.len() == 1 => {
                            spread.push(ka[0].clone());
                        }
                        AstNode::Call {
                            receiver: None,
                            method,
                            args: ka,
                            ..
                        } if method == "__kwarg__" && ka.len() == 2 => {
                            if let AstNode::StringLit(n) = &ka[0] {
                                kw.push((n.clone(), ka[1].clone()));
                                continue;
                            }
                            pos.push(a.clone());
                        }
                        _ => pos.push(a.clone()),
                    }
                }
                // PY-A: Python default argument values for the callee, so
                // `def add(a, b = 10)` + `add(5)` binds b = 10 instead of
                // silently reading 0 (a wrong value with no diagnostic).
                let callee_defaults: Option<Vec<Option<AstNode>>> = if receiver.is_none() {
                    self.param_defaults.get(method.as_str()).cloned()
                } else {
                    None
                };
                let callee_name = method.clone();
                // Batch 747 (#264): the callee's `*args` / `**kwargs`
                // star-params, if declared. Positional overflow collects
                // into a list bound to the former; keyword arguments
                // matching no declared parameter collect into a dict
                // bound to the latter.
                let star_param: Option<(Option<String>, Option<String>)> =
                    self.func_star_params.get(method.as_str()).cloned();
                let (star_args, star_kwargs) = star_param
                    .as_ref()
                    .map_or((None, None), |(a, k)| (a.as_deref(), k.as_deref()));
                let fill = |slots: &mut Vec<Option<AstNode>>,
                            params: &[String],
                            pos: Vec<AstNode>,
                            kw: Vec<(String, AstNode)>,
                            spread: &[AstNode],
                            star_args: Option<&str>,
                            star_kwargs: Option<&str>| {
                    // Star-param slot indices: neither kind of star slot
                    // ever takes a positional binding (Python semantics).
                    let kw_slot =
                        star_kwargs.and_then(|sp| params.iter().position(|p| p == sp));
                    let args_slot =
                        star_args.and_then(|sp| params.iter().position(|p| p == sp));
                    let pos_targets: Vec<usize> = (0..params.len())
                        .filter(|&i| Some(i) != kw_slot && Some(i) != args_slot)
                        .collect();
                    // Batch 752: positional overflow collects into the
                    // `*args` list; without one, keep the old append
                    // (callee ignores extras).
                    let mut pos_extra: Vec<AstNode> = Vec::new();
                    let mut cursor = 0usize;
                    for a in pos {
                        if cursor < pos_targets.len() {
                            slots[pos_targets[cursor]] = Some(a);
                            cursor += 1;
                        } else if args_slot.is_some() {
                            pos_extra.push(a);
                        } else {
                            slots.push(Some(a));
                        }
                    }
                    // #264: unmatched keyword arguments go to the star
                    // param when the callee declares one; without one,
                    // keep the old append (callee ignores extras).
                    let mut kw_acc: Vec<(AstNode, AstNode)> = Vec::new();
                    for (n, v) in kw {
                        match params.iter().position(|p| *p == n) {
                            Some(i) => slots[i] = Some(v),
                            None => match kw_slot {
                                Some(_) => kw_acc.push((AstNode::StringLit(n), v)),
                                None => slots.push(Some(v)),
                            },
                        }
                    }
                    // A `**` mapping fills whatever is still unbound.
                    for m in spread {
                        for (i, slot) in slots.iter_mut().enumerate() {
                            if slot.is_none() {
                                *slot = Some(AstNode::Subscript {
                                    base: Box::new(m.clone()),
                                    index: Box::new(AstNode::StringLit(
                                        params[i].clone(),
                                    )),
                                });
                            }
                        }
                    }
                    // Finally, declared defaults.
                    if let Some(defaults) = &callee_defaults {
                        for (i, slot) in slots.iter_mut().enumerate() {
                            if slot.is_none() {
                                if let Some(Some(d)) = defaults.get(i) {
                                    *slot = Some(d.clone());
                                }
                            }
                        }
                    }
                    // #264/752: star-param slots bind fresh containers —
                    // the collected overflow, or empty `[]` / `{}` when
                    // nothing was passed (Python semantics). A slot
                    // already bound (explicit positional / `**` mapping)
                    // stays.
                    if let Some(si) = args_slot {
                        if si < slots.len() && slots[si].is_none() {
                            slots[si] = Some(AstNode::ArrayLit(std::mem::take(
                                &mut pos_extra,
                            )));
                        }
                    }
                    if let Some(si) = kw_slot {
                        if si < slots.len() && slots[si].is_none() {
                            slots[si] = Some(AstNode::DictLit {
                                entries: kw_acc,
                            });
                        }
                    }
                };
                if !spread.is_empty() {
                    // Unknown signature (external shim / method): there are no
                    // parameter names to bind against, so say so instead of
                    // silently calling with unbound arguments.
                    let params = if receiver.is_none() {
                        self.func_param_names.get(method.as_str()).cloned()
                    } else {
                        None
                    };
                    match params {
                        Some(params) => {
                            let mut slots: Vec<Option<AstNode>> =
                                params.iter().map(|_| None).collect();
                            fill(&mut slots, &params, pos, kw, &spread, star_args, star_kwargs);
                            Self::warn_unbound(&callee_name, &params, &mut slots);
                            slots.into_iter().flatten().collect()
                        }
                        None => {
                            eprintln!(
                                "warning: PY-A: `{}` is called with `**` unpacking but its \
                                 signature is unknown — the mapping is DROPPED (nothing is \
                                 bound for it)",
                                method
                            );
                            kw.into_iter().map(|(_, v)| v).chain(pos).collect()
                        }
                    }
                } else if kw.is_empty() {
                    // No keyword arguments, but omitted ones may still need
                    // their DEFAULTS filled: `add(5)` for `def add(a, b = 10)`
                    // must bind b = 10 rather than silently reading 0.
                    let names = self.func_param_names.get(method.as_str()).cloned();
                    let method_defaults = if receiver.is_some() {
                        self.param_defaults.get(method.as_str()).cloned()
                    } else {
                        None
                    };
                    match names {
                        Some(params) if receiver.is_none() => {
                            let mut slots: Vec<Option<AstNode>> =
                                params.iter().map(|_| None).collect();
                            fill(&mut slots, &params, pos, kw, &[], star_args, star_kwargs);
                            Self::warn_unbound(&callee_name, &params, &mut slots);
                            slots.into_iter().flatten().collect()
                        }
                        // Method call whose callee DECLARES defaults —
                        // or star-params (batch 752: the `*args`/`**kwargs`
                        // slots need their fresh containers even when the
                        // call passes neither extra positionals nor
                        // keywords). `func_param_names` / `param_defaults`
                        // are keyed by the bare method name and index
                        // `self` at 0, which the dispatch site prepends —
                        // so drop it here. Without this `a.reset_index()`
                        // left the argument unbound and codegen padded 0,
                        // i.e. drop=0 (=False) instead of the declared
                        // True (t229, wrong value with no diagnostic).
                        Some(params)
                            if method_defaults
                                .as_ref()
                                .map_or(false, |d| d.iter().any(|x| x.is_some()))
                                || star_args.is_some()
                                || star_kwargs.is_some() =>
                        {
                            let params: Vec<String> =
                                params.into_iter().skip(1).collect();
                            let mut slots: Vec<Option<AstNode>> =
                                params.iter().map(|_| None).collect();
                            fill(&mut slots, &params, pos, kw, &[], star_args, star_kwargs);
                            if let Some(d) = &method_defaults {
                                for (i, slot) in slots.iter_mut().enumerate() {
                                    if slot.is_none() {
                                        if let Some(Some(v)) = d.get(i + 1) {
                                            *slot = Some(v.clone());
                                        }
                                    }
                                }
                            }
                            Self::warn_unbound(&callee_name, &params, &mut slots);
                            slots.into_iter().flatten().collect()
                        }
                        _ => args.clone(),
                    }
                } else if receiver.is_none() {
                    match self.func_param_names.get(method.as_str()).cloned() {
                        Some(params) => {
                            let mut slots: Vec<Option<AstNode>> =
                                params.iter().map(|_| None).collect();
                            fill(&mut slots, &params, pos, kw, &[], star_args, star_kwargs);
                            Self::warn_unbound(&callee_name, &params, &mut slots);
                            slots.into_iter().flatten().collect()
                        }
                        None => kw.into_iter().map(|(_, v)| v).chain(pos).collect(),
                    }
                } else {
                    // Method call WITH kwargs — bind by NAME like the
                    // free-function path, dropping the leading `self`
                    // (`func_param_names`/`param_defaults` index self at 0,
                    // and the dispatch site prepends the receiver).
                    // Appending the kwarg values instead shifted every later
                    // argument: `cache.covers_range(df, a, b, buffer_days=5)`
                    // put `5` into the slot of the first defaulted parameter
                    // and `covers_range` then read garbage as `cache_df`
                    // (measured: `DataFrame::n_rows` SEGV from
                    // `ParquetCache::covers_range + 160`).
                    let names = self.func_param_names.get(method.as_str()).cloned();
                    let mdef = self.param_defaults.get(method.as_str()).cloned();
                    match names {
                        Some(params) if params.len() > 1 => {
                            let params: Vec<String> =
                                params.into_iter().skip(1).collect();
                            let mut slots: Vec<Option<AstNode>> =
                                params.iter().map(|_| None).collect();
                            fill(&mut slots, &params, pos, kw, &[], star_args, star_kwargs);
                            if let Some(d) = &mdef {
                                for (i, slot) in slots.iter_mut().enumerate() {
                                    if slot.is_none() {
                                        if let Some(Some(v)) = d.get(i + 1) {
                                            *slot = Some(v.clone());
                                        }
                                    }
                                }
                            }
                            Self::warn_unbound(&callee_name, &params, &mut slots);
                            slots.into_iter().flatten().collect()
                        }
                        _ => {
                            // Unknown signature: keep the old behaviour
                            // (value order preserved, names dropped).
                            kw.into_iter().map(|(_, v)| v).chain(pos).collect()
                        }
                    }
                }
            };
            // PY-A: starred args `f(*arr)` expand to per-element args
            // (V1: static-size arrays compile-time unrolled).
            for a in &ordered_args {
                if let AstNode::UnaryOp { op, expr } = a {
                    if op == "*" {
                        let arr_id = self.lower_expr(expr);
                        let n = match self.type_map.get(&arr_id).cloned() {
                            Some(Type::Array(_, ArraySize::Literal(n))) => n,
                            // Non-float list literals are DynamicArrays now,
                            // so fall back to the remembered literal length
                            // (0 would silently drop every argument).
                            _ => match &**expr {
                                AstNode::Var(v) => {
                                    self.array_lit_lens.get(v).copied().unwrap_or(0)
                                }
                                _ => 0,
                            },
                        };
                        for i in 0..n {
                            let idx_id = self.next_id();
                            self.exprs.insert(idx_id, MirExpr::IntLit(i as i64));
                            self.type_map.insert(idx_id, Type::I64);
                            let elem = self.emit_call("array_get", vec![arr_id, idx_id], Type::I64);
                            arg_ids.push(elem);
                        }
                        continue;
                    }
                }
                // PY-A: a comprehension's loop variable must take the
                // ITERABLE's element type. The iterable (receiver) has
                // already been lowered into arg_ids[0], so hand the lambda
                // the element type just before lowering it — otherwise a
                // string element is an i64 and every string operation inside
                // the comprehension (`s.upper()`, a dict key `f`) silently
                // works on a pointer.
                // BATCH-296: `.map(lambda …)` on a column vector is the same
                // situation — the lambda's parameter is the vec's element.
                if matches!(method.as_str(), "__collect__" | "__collect_dict__" | "map") {
                    if let AstNode::Closure { .. } = a {
                        if let Some(recv_id) = arg_ids.first() {
                            if method == "__collect_dict__"
                                && self.py_json_items_ids.contains(recv_id)
                            {
                                // Batch 288: `for k, v in json_obj.items()`
                                // — the parser binds the tuple target to
                                // ONE lambda param holding the packed pair
                                // (expr.rs comp_target_binding), so the
                                // hint types the PAIR; the slot reads
                                // (`stack_array_get(e, i)`) inherit the
                                // element type below. Without this
                                // `v.get("ok", 0)` (sources_selector.py:125)
                                // loses the PyJson tag and degrades to the
                                // bare `get` ghost (runtime abort).
                                self.pending_closure_param_types = Some(vec![
                                    Type::DynamicArray(Box::new(Type::Tuple(vec![
                                        Type::Str,
                                        Type::Named("PyJson".to_string(), vec![]),
                                    ]))),
                                ]);
                            } else {
                                let elem = match self.type_map.get(recv_id).cloned() {
                                    Some(Type::DynamicArray(e))
                                    | Some(Type::Array(e, _)) => (*e).clone(),
                                    _ => Type::I64,
                                };
                                // Batch 794 (#214)：闭包形参按**用法**推断——
                                // 数值证据（形参 ×/＋ 数字字面量）⇒ F64（文本列
                                // 的数值 map 由 zeta_series_map_f64 逐元素
                                // strtod 喂闭包）；否则接收者元素（旧行为）。
                                // 批 795 两点收紧：取**首个形参**（任何名字，
                                // 原先只认字面 "x"，lambda y 直接漏判）；数值
                                // 升级只限 method=="map"（comprehension 走
                                // zeta_collect_vec_n 的整数调用规约，绝不能
                                // 给成 double 签名——t34 实拍 x*2 恒等化和）。
                                let mut final_hint = elem;
                                if method == "map" {
                                    if let AstNode::Closure { params, body, .. } = a {
                                        if let Some(p0) = params.first().cloned() {
                                            if Self::closure_param_usage(body, &p0, 0) == 1 {
                                                final_hint = Type::F64;
                                            }
                                        }
                                    }
                                }
                                self.pending_closure_param_types =
                                    Some(vec![final_hint]);
                            }
                        }
                    }
                }
                let aid = self.lower_expr(a);
                // An INLINE expression (tuple / array / struct literal) has no
                // alloca: passing it straight to a call let the callee read an
                // unset slot. Measured: `ParquetCache.load(path)` received a
                // dangling `date_cols` default and
                // `MarketDataFetcher._load_cache` then crashed in `map_insert`.
                let aid = if matches!(self.exprs.get(&aid), Some(MirExpr::Var(_))) {
                    aid
                } else {
                    self.materialize_for_call(aid)
                };
                arg_ids.push(aid);
            }
            // Never let a stale hint leak into an unrelated closure.
            self.pending_closure_param_types = None;

            // Batch 111: `df.drop("col")` — library expects lt(vec,str).
            // Wrap a lone Str arg into a 1-element StackArray.
            // Batch 412: this used to take the TAIL of the argument vector, which
            // stopped being `labels` the moment the callee declared another
            // parameter — `drop(self, labels=[], columns=[])` puts the bound
            // default after it, so the wrap missed and `labels` arrived as a raw
            // Str (t206: `for k in labels` walked the characters and aborted).
            // Bind to the DECLARED slot instead; `func_param_names` indexes `self`
            // at 0 and the dispatch site prepends the receiver, so the two
            // indices line up. Unknown signature keeps the old tail behaviour.
            if method == "drop" && arg_ids.len() >= 2 {
                let last = arg_ids.len() - 1;
                let idx = self
                    .func_param_names
                    .get(method.as_str())
                    .and_then(|p| {
                        p.iter()
                            .position(|n| n == "labels")
                            .filter(|i| *i < arg_ids.len())
                    })
                    .unwrap_or(last);
                let lab = arg_ids[idx];
                if matches!(self.type_map.get(&lab), Some(Type::Str)) {
                    let arr = self.next_id();
                    self.exprs.insert(
                        arr,
                        MirExpr::StackArray {
                            elements: vec![lab],
                            size: 1,
                        },
                    );
                    self.type_map
                        .insert(arr, Type::DynamicArray(Box::new(Type::Str)));
                    arg_ids[idx] = arr;
                }
            }

            // PY-A: `type(x)` — the static type is already known, so fold it
            // to the Python type NAME as a string (no runtime reflection, no
            // `_type` extern). `print(type(x))` / `type(x) is int`-style code
            // in the wild then works instead of failing to link.
            if method == "type" && receiver.is_none() && arg_ids.len() == 1 {
                // 批次 843：判定面收敛到 call_class::type_name_of（单测
                // 钉住每条映射；未知落 object 保守正确）。
                let tn = match self.type_map.get(&arg_ids[0]) {
                    Some(t) => type_name_of(t),
                    None => "object",
                };
                let sid = self.next_id();
                self.exprs.insert(sid, MirExpr::StringLit(tn.to_string()));
                self.type_map.insert(sid, Type::Str);
                self.exprs.insert(id, MirExpr::Var(sid));
                return sid;
            }
            // PY-A: platform class constructor calls (FixedSlippage(0.001),
            // OrderCost(...), MACD(...)) — capitalized free calls with no
            // local definition route to the platform-object runtime.
            // If a user-defined function with this name exists (class
            // desugar emits `Accumulator(...)` constructors), it wins.
            // A class defined inside a function is hoisted like a nested
            // def, so its constructor lives in `closure_vars` under the
            // class name — NOT in `func_ret_types`. Missing that check sent
            // `P(n)` to the opaque platform-object runtime.
            let user_fn_defined = self.func_ret_types.contains_key(&method.clone())
                || self.closure_vars.contains_key(&method.clone())
                || self.hoisted_names.contains_key(&method.clone());
            if method.chars().next().map_or(false, |c| c.is_uppercase())
                && receiver.is_none()
                && !user_fn_defined
            {
                let name_id = self.next_id();
                self.exprs
                    .insert(name_id, MirExpr::StringLit(method.clone()));
                self.type_map.insert(name_id, Type::Str);
                let mut call_args = vec![name_id];
                call_args.extend(arg_ids.iter().copied());
                while call_args.len() < 4 {
                    let z = self.next_id();
                    self.exprs.insert(z, MirExpr::IntLit(0));
                    self.type_map.insert(z, Type::I64);
                    call_args.push(z);
                }
                self.stmts.push(MirStmt::Call {
                    func: "zeta_platform_obj".to_string(),
                    args: call_args,
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // `pkg.user.show()` — the receiver is a DOTTED MODULE PATH, not a
            // value. `import pkg.user` registers only the ROOT alias, so the
            // receiver lowered to an env read of the non-existent global
            // `pkg__user` (0) and the call became a bare `show_1` ghost. When
            // alias + parts names a real user module, call its function.
            if let Some(recv) = receiver {
                if let Some((root, parts)) = Self::flatten_module_receiver(recv) {
                    if crate::diagnostics::env_flag("ZETA_PROBE_CALL") {
                        eprintln!(
                            "MODCALL probe: root={:?} parts={:?} method={} aliases_has_root={} user_mods={}",
                            root,
                            parts,
                            method,
                            self.py_module_aliases.contains_key(&root),
                            self.py_user_modules.len()
                        );
                    }
                    // `import pkg.user` may register the alias under the FULL
                    // dotted name (key `pkg.user`) rather than the root, so try
                    // both spellings before giving up.
                    let dotted_text = if parts.is_empty() {
                        root.clone()
                    } else {
                        format!("{}.{}", root, parts.join("."))
                    };
                    let alias_mod = self
                        .py_module_aliases
                        .get(&root)
                        .cloned()
                        .or_else(|| self.py_module_aliases.get(&dotted_text).cloned())
                        .or_else(|| {
                            self.py_user_modules
                                .contains(&dotted_text)
                                .then(|| dotted_text.clone())
                        });
                    if let Some(alias_mod) = alias_mod {
                        let dotted = if parts.is_empty() {
                            alias_mod.clone()
                        } else {
                            format!("{}.{}", alias_mod, parts.join("."))
                        };
                        if self.py_user_modules.contains(&dotted) {
                            let sym = format!("{}__{}", dotted.replace('.', "_"), method);
                            let arg_ids: Vec<u32> =
                                args.iter().map(|a| self.lower_expr(a)).collect();
                            self.stmts.push(MirStmt::Call {
                                func: sym,
                                args: arg_ids,
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map.insert(id, Type::I64);
                            return id;
                        }
                    }
                }
            }
            // Python `set` methods on our list-backed sets: `s.add(x)` is a
            // push; `s.discard(x)` / `s.remove(x)` rebuild without the slot.
            // They compiled to ghosts (`_set__add`) and failed the link.
            // 批次 831：数值内建族（successor/predecessor/abs/sum/min/max）
            // 全部迁入 gen/call_num.rs；入口判定读分类器，执行者只管发射。
            if classify_call(method) == CallClass::NumericBuiltin
                && receiver.is_none()
            {
                if let Some(nid) = self.lower_numeric_builtin(method, args, id) {
                    return nid;
                }
            }

            // 批次 816/824：集合族（add/discard/remove/intersection）搬
            // 子模块 gen/call_set.rs；入口判定改读分类器（SetMutation/
            // SetIntersection 互斥在分类层保证），执行文件只管发射。
            if matches!(
                classify_call(method),
                CallClass::SetMutation | CallClass::SetIntersection
            ) && receiver_ty.is_some()
            {
                if let Some(sid) = self.lower_set_family(
                    receiver.as_deref(),
                    receiver_ty.as_ref().unwrap_or(&Type::I64),
                    method.as_str(),
                    &arg_ids,
                    id,
                ) {
                    return sid;
                }
            }
            // `s.clear()` on a list-backed set (batch 294:
            // `PositionLedger._today_buys.clear()` hit the weak `_clear`
            // abort stub). In-place len=0 — the handle stays valid, so no
            // write-back is needed at the call site. The map receiver is
            // NOT matched here (zeta_map_clear owns it below); a `str`
            // receiver has no clear.
            if method == "clear"
                && arg_ids.len() == 1
                && receiver.is_some()
                && receiver_ty.as_ref().map_or(false, |t| {
                    matches!(t, Type::DynamicArray(_) | Type::Array(_, _))
                        || matches!(t, Type::Named(n, _) if n == "set" || n == "frozenset")
                        || t.is_untyped()
                })
            {
                self.stmts.push(MirStmt::Call {
                    func: "zeta_vec_clear".to_string(),
                    args: vec![arg_ids[0]],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, receiver_ty.unwrap_or(Type::slot_fallback()));
                return id;
            }
            // 批次 806（t572 known-fail 收口）：vec 形接收者上的 `xs.into_iter()`
            // 就是那个 vec。for 降型本就把结果交给 array_len/array_get 去读
            // （MIR 实拍：ghost 调用的 dest 之后紧跟 `array_len(21)`／
            // `array_get(21, 24)`），缺的只有那一次调用本身——接收者的元素型
            // 进了幽灵名 ⇒ `[dynamic]i64::into_iter` 一族，而批次 428 的判据是
            // "不在 DYN_RUNTIME_BINDINGS 白名单里的 ghost ⇒ 抛"，于是
            // `xs=[1,2]; for v in xs.into_iter()` 运行期 code=1（i64/f64/str 三形
            // 实拍全抛）。折成本地 Assign 把句柄原样交给循环，元素型随接收者走：
            // 若在这里硬写 I64，f64 列的位整会被当整数读——那是把"抛异常"换成
            // "静默错值"，本仓定为最恶劣的一类，不做。
            if matches!(method.as_str(), "into_iter" | "iter")
                && arg_ids.len() == 1
                && receiver.is_some()
                && receiver_ty.as_ref().map_or(false, |t| {
                    matches!(t, Type::DynamicArray(_) | Type::Array(_, _))
                })
            {
                self.stmts.push(MirStmt::Assign {
                    lhs: id,
                    rhs: arg_ids[0],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                let ty = receiver_ty.unwrap();
                self.type_map.insert(id, ty);
                return id;
            }
            // BATCH-294: `<opaque>.date()` (e.g. `context.current_dt.date()`
            // where the context factory erased the field type) reached the
            // weak `_date` abort stub. Zeta's datetime handle IS a PyDate
            // (day-count first field — same shape `.strftime`/comparisons
            // read), so the projection is the handle itself.
            if method == "date"
                && arg_ids.len() == 1
                && receiver.is_some()
                && matches!(
                    receiver_ty.as_ref(),
                    None | Some(Type::I64) | Some(Type::PyDynamic)
                )
            {
                self.stmts.push(MirStmt::Call {
                    func: "zeta_dt_date".to_string(),
                    args: vec![arg_ids[0]],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::Named("PyDate".to_string(), vec![]));
                return id;
            }
            // `df[<boolean mask>]` — the shim's `__getitem__` assumes a
            // COLUMN NAME (`self.data[map_str_key(key)]`), so a row mask went
            // into the map lookup and crashed inside `map_str_key`
            // (measured: `fetch_stocks`'s
            // `cached[(cached["trade_date"] >= eff_start) & (… <= req_end)]`).
            // Batch 398 reads the ELEMENT type: `DynamicArray(Bool)` is a mask,
            // `DynamicArray(I64)` is either a positional index list or a mask
            // that entered through the shim's `lt(vec, i64)` annotation — the
            // two are still indistinguishable for those, so the I64 arm keeps
            // routing to `loc` (see the roadmap residual).
            if (method == "__getitem__" || method == "column")
                && receiver_ty.as_ref().map_or(false, |t| match t {
                    Type::Named(n, _) => {
                        n == "DataFrame" || n.ends_with(".DataFrame")
                    }
                    _ => false,
                })
                && matches!(
                    arg_ids.get(1).and_then(|a| self.type_map.get(a)),
                    Some(Type::DynamicArray(_))
                )
            {
                let elem = match arg_ids.get(1).and_then(|a| self.type_map.get(a)) {
                    Some(Type::DynamicArray(e)) => (**e).clone(),
                    _ => Type::PyDynamic,
                };
                if matches!(elem, Type::Bool | Type::I64) {
                    self.stmts.push(MirStmt::Call {
                        func: "DataFrame::loc".to_string(),
                        args: vec![arg_ids[0], arg_ids[1]],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::Named("DataFrame".to_string(), vec![]));
                    return id;
                }
                if matches!(elem, Type::Str) {
                    // `df[["a", "b"]]` — pandas reads a LIST-LIKE key as a
                    // COLUMN SUBSET (verified against pandas 3.0.5:
                    // `df[["code"]]` keeps every row of that column). The
                    // vector handle used to go into
                    // `self.data[map_str_key(key)]` as if it were a column
                    // NAME (measured: `df[["code"]]` SIGSEGV, rc=139).
                    let tn = match receiver_ty.as_ref() {
                        Some(Type::Named(n, _)) => n.clone(),
                        _ => String::new(),
                    };
                    let target = self
                        .qualified_method_candidate(&tn, "select_columns")
                        .unwrap_or_else(|| "DataFrame::select_columns".to_string());
                    let ret_ty = self.func_ret_types.get(&target).cloned();
                    self.stmts.push(MirStmt::Call {
                        func: target,
                        args: vec![arg_ids[0], arg_ids[1]],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(
                        id,
                        ret_ty.unwrap_or_else(|| {
                            Type::Named("DataFrame".to_string(), vec![])
                        }),
                    );
                    return id;
                }
            }
            // `col.clip(lower=0, upper=...)` on a numeric COLUMN — element-wise
            // clamp. Without this the call hit the zero-arity `clip` stub and
            // aborted (measured in `validate_and_repair_stock_ohlcv`).
            if method == "clip"
                && receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::DynamicArray(_) | Type::Array(_, _)))
            {
                let lo = arg_ids.get(1).copied().unwrap_or(0);
                let hi = arg_ids.get(2).copied().unwrap_or(0);
                let has_lo = if arg_ids.len() > 1 { 1i64 } else { 0 };
                let has_hi = if arg_ids.len() > 2 { 1i64 } else { 0 };
                let f1 = self.next_id();
                self.exprs.insert(f1, MirExpr::IntLit(has_lo));
                self.type_map.insert(f1, Type::I64);
                let f2 = self.next_id();
                self.exprs.insert(f2, MirExpr::IntLit(has_hi));
                self.type_map.insert(f2, Type::I64);
                // Batch 784 (#213① 下游)：元素型分派——f64 位元素列表走
                // py_vec_clip_f64（元素按 f64 位 clamp）；py_vec_clip 是
                // vec<str> 列的数值裁剪（元素按 char* → strtod），f64 位
                // 元素传入 = 按位当指针 ⇒ strtod(SEGV，clip→len ASLR 闪崩
                // 的真身，783 实拍六跑全 139)。
                let elem_f64 = matches!(
                    self.type_map.get(&arg_ids[0]),
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _))
                        if matches!(**e, Type::F64)
                );
                let clip_fn = if elem_f64 { "py_vec_clip_f64" } else { "py_vec_clip" };
                self.stmts.push(MirStmt::Call {
                    func: clip_fn.to_string(),
                    args: vec![arg_ids[0], lo, hi, f1, f2],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, receiver_ty.clone().unwrap());
                return id;
            }
            // Batch 785 (#213① 下游·裸形)：裸 `clip(v[, lo[, hi]])` 调用的
            // 元素型分派——f64 位元素列表走 py_vec_clip_f64（C 侧按位
            // clamp）；裸名原路（C clip 族按 char* → strtod）对 f64 位
            // 元素 = 按位当指针 ⇒ strtod(SEGV，clip→len 闪崩真身)。
            // 非 F64 元素（str/int 列）原路不动。bounds 整型先 sitofp。
            if receiver.is_none()
                && matches!(method.as_str(), "clip" | "clip_2" | "clip_3" | "clip_4")
                && !arg_ids.is_empty()
                && matches!(
                    self.type_map.get(&arg_ids[0]),
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _))
                        if matches!(**e, Type::F64)
                )
            {
                fn to_f64(s: &mut MirGen, id: u32) -> u32 {
                    match s.type_map.get(&id).cloned() {
                        Some(Type::F64) => id,
                        _ => {
                            let f = s.next_id();
                            s.stmts.push(MirStmt::Call {
                                func: "zeta_float_i64".to_string(),
                                args: vec![id],
                                dest: f,
                                type_args: vec![],
                            });
                            s.exprs.insert(f, MirExpr::Var(f));
                            s.type_map.insert(f, Type::F64);
                            f
                        }
                    }
                }
                let lo_f = arg_ids.get(1).map(|&id| to_f64(self, id));
                let hi_f = arg_ids.get(2).map(|&id| to_f64(self, id));
                let lo = lo_f.unwrap_or_else(|| self.next_id_with_lit(0));
                let hi = hi_f.unwrap_or_else(|| self.next_id_with_lit(0));
                let has_lo = self.next_id_with_lit(if lo_f.is_some() { 1 } else { 0 });
                let has_hi = self.next_id_with_lit(if hi_f.is_some() { 1 } else { 0 });
                self.type_map.insert(has_lo, Type::I64);
                self.type_map.insert(has_hi, Type::I64);
                self.stmts.push(MirStmt::Call {
                    func: "py_vec_clip_f64".to_string(),
                    args: vec![arg_ids[0], lo, hi, has_lo, has_hi],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(
                    id,
                    Type::DynamicArray(Box::new(Type::F64)),
                );
                return id;
            }
            // `series.max()` / `.min()` on a COLUMN — same ghost family
            // (`[dynamic]str__max`), reached once annotated params keep their
            // DataFrame type (`covers_range` does `cache_df[date_col].max()`).
            if matches!(method.as_str(), "max" | "min")
                && receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::DynamicArray(_) | Type::Array(_, _)))
                && arg_ids.len() == 1
            {
                self.emit_call_into(id, &format!("py_vec_{}", method), vec![arg_ids[0]], Type::Str);
                return id;
            }
            // `mask.any()` / `mask.all()` on a boolean or label vector —
            // same ghost family as `.isna()` (`[dynamic]i64__all`).
            if matches!(method.as_str(), "any" | "all")
                && receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::DynamicArray(_) | Type::Array(_, _)))
                && arg_ids.len() == 1
            {
                self.emit_call_into(id, &format!("py_vec_{}", method), vec![arg_ids[0]], Type::Bool);
                return id;
            }
            // `series.isna()` on a COLUMN (a string vector): the shim's module
            // function cannot be a vec METHOD, so it compiled to the ghost
            // `[dynamic]str__isna` and aborted. Route to the runtime helper.
            if matches!(method.as_str(), "isna" | "notna")
                && receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::DynamicArray(_) | Type::Array(_, _)))
                && arg_ids.len() == 1
            {
                self.stmts.push(MirStmt::Call {
                    func: format!("py_vec_{}", method),
                    args: vec![arg_ids[0]],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(Type::Bool)));
                return id;
            }
            // `series.pct_change()` / `series.abs()` on a COLUMN (a string
            // vector): compiled to the ghost `[dynamic]str__pct_change`, and
            // the runtime helper was typed I64 — so `c[2]` was an integer
            // subscript of a vector and `print_str(c[2])` crashed (measured:
            // `remove_extreme_return_bars` in the local backtest).
            if matches!(method.as_str(), "pct_change" | "abs")
                && receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::DynamicArray(_) | Type::Array(_, _)))
                && arg_ids.len() == 1
            {
                self.stmts.push(MirStmt::Call {
                    func: format!("py_vec_{}", method),
                    args: vec![arg_ids[0]],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(Type::Str)));
                return id;
            }
            // BATCH-296: `series.map(fn)` on a COLUMN vector. The runtime
            // helper (`_[dynamic]str__map`) already applies `fn` elementwise,
            // but the call was typed I64, so `s2[0]` printed a heap pointer
            // and `s2.nunique()` skipped the vec table and landed on the
            // shim's `Series.nunique` with a raw vec handle — it read the
            // header as `self.data` and reported 0 unique
            // (measured: `result["stock_code"].map(...)` → `0 只标的`).
            // Element type: the closure's recorded return type when known,
            // else the receiver's own element type.
            if method == "map"
                && arg_ids.len() == 2
                && matches!(receiver_ty.as_ref(), Some(Type::DynamicArray(_)))
            {
                let elem = match receiver_ty.as_ref().unwrap() {
                    Type::DynamicArray(e) => (**e).clone(),
                    _ => Type::I64,
                };
                let res_elem = match self.exprs.get(&arg_ids[1]) {
                    Some(MirExpr::FuncAddr(n)) => self
                        .closure_ret_tys
                        .get(n)
                        .cloned()
                        .unwrap_or(elem.clone()),
                    _ => elem.clone(),
                };
                // 批 795：数值判定与 hint 侧同判据——**首个形参**按用法
                // （任何名字，原先只认字面 "x"，lambda y 漏判掉进 str__map
                // 再撞 int/double ABI 错配）；接收者元素已是 F64 时即使
                // 用法判不出也走 f64 变体（恒等/透传闭包仍是 double 签名）。
                // 元素是文本 → strtod 变体；已是 f64 位 → 位型变体
                // （map_f64 的输出与 zeta_vec_push_f64 的列都是位型，
                // strtod 会把位整当指针解引用）。
                let numeric_map = match args.first() {
                    Some(AstNode::Closure { params, body, .. }) => params
                        .first()
                        .map_or(false, |p0| Self::closure_param_usage(body, p0, 0) == 1),
                    _ => false,
                };
                let f64_map = numeric_map || matches!(elem, Type::F64);
                let res_elem = if f64_map { Type::F64 } else { res_elem };
                self.stmts.push(MirStmt::Call {
                    func: (if !f64_map {
                        "[dynamic]str__map"
                    } else if matches!(elem, Type::Str) {
                        "zeta_series_map_f64"
                    } else {
                        "zeta_series_map_f64_bits"
                    })
                    .to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(res_elem)));
                return id;
            }
            // 批次 797（selfhost RUN 挡格）：`.map(closure)` 于**无型条目面**的
            // 接收者——枚举变体载荷绑定槽与通用调用 dest（如 `into_iter_1`）
            // 默认定型 I64，selfhost:141 的 `asts.into_iter().map(|a|
            // SimpleEval::eval(a)).sum()` 正是该形：上面 BATCH-296 列臂只认
            // Some(DynamicArray(_)) ⇒ 漏臂落幽灵名 `map_2` ⇒ codegen 剥 arity
            // 后缀链到 `_map` weak 桩 ⇒ 调用即 abort rc=134（改前 probe_map.z
            // 实拍：compile/link rc=0、run 134、PY-A `_map`）。运行期 helper
            // `_[dynamic]str__map` 逐元素按位传闭包、按位收结果
            // （py_additions.c:1469，元素型无关），vec 句柄运行时正落在形参位。
            if method == "map"
                && arg_ids.len() == 2
                && matches!(args.first(), Some(AstNode::Closure { .. }))
                && receiver_ty
                    .as_ref()
                    .map_or(true, |t| matches!(t, Type::I64))
            {
                let res_elem = match self.exprs.get(&arg_ids[1]) {
                    Some(MirExpr::FuncAddr(n)) => self
                        .closure_ret_tys
                        .get(n)
                        .cloned()
                        .unwrap_or(Type::slot_fallback()),
                    _ => Type::I64,
                };
                self.stmts.push(MirStmt::Call {
                    func: "[dynamic]str__map".to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(res_elem)));
                return id;
            }
            // `pd.to_datetime(vecstr).normalize()` — our shim represents dates
            // as "YYYY-MM-DD" strings, so `.normalize()` (drop the time part)
            // is the identity. Without this the call resolved to the ghost
            // `[dynamic]str__normalize` and the link failed.
            if method == "normalize"
                && receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::DynamicArray(_)))
                && arg_ids.len() == 1
            {
                self.exprs.insert(id, MirExpr::Var(arg_ids[0]));
                self.type_map.insert(id, receiver_ty.clone().unwrap());
                return id;
            }
            // `s.startswith(("000", "399"))` — Python accepts a TUPLE of
            // prefixes. The tuple handle was passed straight to
            // host_str_starts_with, which returned 0 for everything
            // (measured: `_is_likely_index("000300.XSHG")` was False, so A-share
            // index codes were scored as ordinary ETFs in the source selector).
            // Expand the literal into OR-ed single-prefix tests.
            if matches!(method.as_str(), "startswith" | "endswith")
                && args.len() == 1
                && arg_ids.len() == 2
            {
                if let AstNode::Tuple(items) | AstNode::ArrayLit(items) = &args[0] {
                    if !items.is_empty() {
                        // Build the prefix VEC via the existing literal
                        // lowering, then test them in the runtime. The
                        // OR-chain version compiled to two calls plus an
                        // `||` expression and returned 0 for a matching
                        // prefix (measured: `_is_likely_index("000300.XSHG")`
                        // was False), so keep the combination inside one
                        // helper.
                        let lit = AstNode::ArrayLit(items.clone());
                        let vec_id = self.lower_expr(&lit);
                        let flag_id = self.next_id();
                        self.exprs.insert(
                            flag_id,
                            MirExpr::IntLit(if method == "startswith" { 0 } else { 1 }),
                        );
                        self.type_map.insert(flag_id, Type::I64);
                        self.emit_call_into(id, "py_str_prefix_any", vec![arg_ids[0], vec_id, flag_id], Type::Bool);
                        return id;
                    }
                }
            }
            // PY-A: `x in container` membership — strings via
            // host_str_contains; other container kinds are a V1 limit
            // (emit 0 with a compile-time note).
            if method == "__contains__" {
                // A module-level LIST/CONSTANT read across a module boundary has
                // no `type_map` entry (the value arrives through an env read), so
                // `x in mod.LIST` typed the container I64 — every branch below
                // missed and the expression compiled to a bare
                // `<mod>__LIST.__contains__` ghost symbol (measured:
                // `_backend_strategy_wufu_constants__WUFU_INDEX_BS_CODES
                // .__contains__`, which broke the strategy's index filter).
                // Scoped to THIS branch on purpose: widening the general dispatch
                // turned `df.sort_values(...)` into a `map__sort_values` ghost.
                let receiver_ty = match receiver_ty {
                    Some(Type::I64) | Some(Type::PyDynamic) => receiver
                        .as_ref()
                        .and_then(|r| Self::receiver_global_key(&self.py_module_aliases, r))
                        .and_then(|k| self.global_ty_of(&k))
                        .or(receiver_ty),
                    other => other,
                };
                let is_str = receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::Str));
                // A `lt(map, K, V)` PARAMETER is typed via `source_types`,
                // not type_map (params default to I64), so `k in m` inside a
                // method missed the map branch and fell through to the
                // unique-`__contains__` heuristic below — which dispatched
                // `DataFrame::__contains__(m, k)` with the MAP as `self`
                // (segfault in pylib/pandas.z `rename`, t204).
                let src_ty = self.source_types.get(&arg_ids[0]).cloned().unwrap_or_default();
                let is_map = receiver_ty
                    .as_ref()
                    .map_or(false, Type::is_map)
                    || src_ty.starts_with("map<");
                if is_str && arg_ids.len() == 2 {
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_contains".to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Bool);
                    return id;
                }
                // PY-A: `x in list` — linear scan. Previously an array
                // receiver fell through and the expression silently
                // produced 0 (false) whatever the elements were.
                // An ARRAY PARAMETER is typed via `source_types`, not
                // `type_map` (unannotated params default to i64 there), so
                // without this fallback `v in xs` inside a function returned
                // 0 for every element — silently wrong for the filtering
                // code that strategies are full of.
                let is_array_param =
                    src_ty.starts_with('[') || src_ty.starts_with("*mut [");
                if (matches!(
                    receiver_ty.as_ref(),
                    Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                ) || is_array_param) && arg_ids.len() == 2
                {
                    let elem_is_str = match receiver_ty.as_ref() {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            matches!(**e, Type::Str)
                        }
                        // e.g. `[str]` / `*mut [str]`
                        _ => {
                            let inner = src_ty
                                .trim_start_matches("*mut ")
                                .trim_start_matches('[');
                            inner.starts_with("str") || inner.starts_with("String")
                        }
                    };
                    // 批 926：f64 元素走 double 域比较版（位模式按位比较
                    // 让 `2.5 in [1.5, 2.5]` 为 False）
                    let elem_is_f64 = match receiver_ty.as_ref() {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            matches!(**e, Type::F64 | Type::F32)
                        }
                        _ => false,
                    };
                    if elem_is_f64 {
                        self.emit_call_into(
                            id,
                            "py_list_contains_f64",
                            vec![arg_ids[0], arg_ids[1]],
                            Type::Bool,
                        );
                        return id;
                    }
                    let flag = self.next_id();
                    self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                    self.type_map.insert(flag, Type::I64);
                    self.emit_call_into(id, "py_list_contains", vec![arg_ids[0], arg_ids[1], flag], Type::Bool);
                    return id;
                }
                if is_map && arg_ids.len() == 2 {
                    /* Batch 682b: `key in dict` probes the ENTRY TABLE
                       via py_map_contains — the old DictGet(key) != 0
                       answered False for a key whose VALUE is 0
                       (`d["b"] = 0` stored 0, `0 != 0` false; batch 598
                       fixed .get's existence check the same way, this is
                       the `in` sibling). The key goes through the same
                       map_str_key normalization the write side uses. */
                    let key_id = self.lower_map_key(arg_ids[1]);
                    self.emit_call_into(id, "py_map_contains", vec![arg_ids[0], key_id], Type::Bool);
                    return id;
                }
                // PY-A: `x in obj` — dispatch to a `__contains__` method
                // when the container is a KNOWN struct, or when exactly
                // one `X::__contains__` definition exists (the receiver's
                // Named type is pass-order dependent: the constructor call
                // does not always record it, leaving I64 here — mirrors
                // the Call path's unique-qualified fallback).
                if arg_ids.len() == 2 {
                    let named_hit = match receiver_ty.as_ref() {
                        Some(Type::Named(tn, _)) => {
                            self.qualified_method_candidate(tn, "__contains__")
                        }
                        _ => None,
                    };
                    let qualified = match named_hit {
                        Some(q) => Some(q),
                        None => {
                            // The unique-definition fallback may only fire for
                            // the method's own `self`: it passes the receiver
                            // as `self`, so firing it on any untyped receiver
                            // calls `Class::__contains__(<non-self>, k)` — a
                            // segfault, not a diagnostic.
                            let self_slot = self.name_to_id.get("self").copied();
                            if !matches!(receiver_ty.as_ref(), Some(Type::Named(_, _)))
                                && self_slot == Some(arg_ids[0])
                            {
                                let hits: Vec<String> = self
                                    .func_ret_types
                                    .keys()
                                    .filter(|k| k.ends_with("::__contains__"))
                                    .cloned()
                                    .collect();
                                if hits.len() == 1 {
                                    hits.into_iter().next()
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                    };
                    if let Some(q) = qualified {
                        self.stmts.push(MirStmt::Call {
                            func: q,
                            args: arg_ids.clone(),
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::Bool);
                        return id;
                    }
                }
                // BATCH-296: `k in c` where `c` is a value the compiler
                // could not type (element of a `dict[str, Any]`, dyn param).
                // Everything above missed, and the old answer was a silent
                // constant 0 plus a warning — i.e. membership against a
                // real dict or list reported False. `zeta_dyn_contains`
                // tells map / vec / text apart at runtime by GC geometry;
                // the two key forms are what the typed branches pass
                // (content hash for the dict, raw handle for the list).
                // t402: tuple literals lower to StackArray, whose block
                // carries the same [cap, len] header the vec probe reads —
                // routing Tuple receivers here scans the elements instead
                // of answering a constant False.
                if arg_ids.len() == 2
                    && matches!(
                        receiver_ty.as_ref(),
                        None
                            | Some(Type::I64)
                            | Some(Type::PyDynamic)
                            | Some(Type::Tuple(_))
                    )
                {
                    let key_id = arg_ids[1];
                    let key_is_str =
                        matches!(self.type_map.get(&key_id), Some(Type::Str)) as i64;
                    let flag = self.next_id();
                    self.exprs.insert(flag, MirExpr::IntLit(key_is_str));
                    self.type_map.insert(flag, Type::I64);
                    let hkey = self.lower_map_key(key_id);
                    self.emit_call_into(id, "zeta_dyn_contains", vec![arg_ids[0], key_id, hkey, flag], Type::Bool);
                    return id;
                }
                eprintln!(
                    "warning: `in` membership is only supported for strings/dicts in V1 (container type: {:?})",
                    receiver_ty
                );
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::Bool);
                return id;
            }

            // PY-A: Vec push — vec_push may reallocate and RETURNS the
            // (new) data handle; the caller must rebind (`v = v.push(x)`).
            if method == "push"
                && receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::DynamicArray(_)))
                && arg_ids.len() == 2
            {
                self.stmts.push(MirStmt::Call {
                    func: "vec_push".to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, receiver_ty.clone().unwrap());
                // `arr.push(x)` as a STATEMENT discards the returned
                // handle. vec_push returns a NEW handle when it reallocates,
                // so without rebinding the variable the array stays empty
                // (its growth was silently lost). Python semantics: append
                // mutates in place — so write the result back.
                if method == "push"
                    && let Some(recv_ast) = receiver
                    && let AstNode::Var(name) = &**recv_ast
                    && let Some(&slot) = self.name_to_id.get(name)
                {
                    self.stmts.push(MirStmt::Assign { lhs: slot, rhs: id });
                }
                return id;
            }

            // PY-A: `s.add(v)` on a set (which is a Vec here) — dedup push.
            // `py_set_add` was declared + registered since the set work but
            // nothing ever emitted it, so `set(); s.add(1)` linked against a
            // bare `_add` (t246/t231). Same rebind rule as `push`: the runtime
            // returns the possibly-grown handle and Python mutates in place.
            if receiver.is_some()
                && method == "add"
                && receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::DynamicArray(_)))
                && arg_ids.len() == 2
            {
                self.stmts.push(MirStmt::Call {
                    func: "py_set_add".to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, receiver_ty.clone().unwrap());
                if let Some(recv_ast) = receiver
                    && let AstNode::Var(name) = &**recv_ast
                    && let Some(&slot) = self.name_to_id.get(name)
                {
                    self.stmts.push(MirStmt::Assign { lhs: slot, rhs: id });
                }
                return id;
            }

            // PY-A: d.keys() / d.values() / Counter.most_common([n]) —
            // `map` is not a Py* tag, so these go through this dedicated
            // branch rather than the handle-method dispatch.
            if receiver_ty
                .as_ref()
                .map_or(false, Type::is_map)
            {
                let func = match (method.as_str(), args.len()) {
                    ("keys", 0) => Some("map_keys"),
                    ("values", 0) => Some("map_values"),
                    ("items", 0) => Some("py_map_items"),
                    ("most_common", 0) => Some("py_map_most_common"),
                    ("most_common", 1) => Some("py_map_most_common_2"),
                    _ => None,
                };
                if let Some(fname) = func {
                    let mut call_args = vec![arg_ids[0]];
                    call_args.extend(arg_ids.iter().skip(1).copied());
                    self.stmts.push(MirStmt::Call {
                        func: fname.to_string(),
                        args: call_args,
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    // A map's VALUE type must reach the elements: the local
                    // backtest reads `for pos in portfolio.positions.values()`
                    // and then `pos.total_amount`. Typed I64 (the old default,
                    // borrowed from the KEY kind), the field read fell through
                    // to the raw-slot path and printed the handle — every
                    // holding valued at ~0, so `final_value` stayed at the
                    // cash balance.
                    // Batch 299: an UNTYPED map keys by `str`. The key type
                    // is what decides content-hashing, and a static I64 left
                    // a comprehension key as the string's ADDRESS: `d["AAA"]`
                    // missed, returned 0, and the field read on the null
                    // Position SEGFAULTED (measured on the positions map).
                    // `map_str_key` is identity for a non-pointer word, so a
                    // genuinely integer key still round-trips unchanged.
                    // A `PyDynamic` key carries no information either — same
                    // treatment (measured: a declared `dict[str, float]`
                    // field reaches MIR as `map[PyDynamic, I64]`).
                    let key_ty = match receiver_ty.as_ref() {
                        Some(Type::Named(_, args)) => match args.first().cloned() {
                            None | Some(Type::PyDynamic) => Type::Str,
                            Some(t) => t,
                        },
                        _ => Type::Str,
                    };
                    let val_ty = match receiver_ty.as_ref() {
                        Some(Type::Named(_, args)) => {
                            args.get(1).cloned().unwrap_or_else(Type::slot_fallback)
                        }
                        _ => Type::I64,
                    };
                    let elem_ty = if method == "values" {
                        val_ty.clone()
                    } else {
                        key_ty.clone()
                    };
                    let out_ty = if method == "most_common" || method == "items" {
                        Type::DynamicArray(Box::new(Type::Tuple(vec![
                            key_ty,
                            val_ty,
                        ])))
                    } else {
                        Type::DynamicArray(Box::new(elem_ty))
                    };
                    self.type_map.insert(id, out_ty);
                    return id;
                }
            }

            // PY-A: `base[start:end]` slicing → runtime zeta_slice_vec,
            // returning a Vec-layout handle (len()/indexing work on it).
            // PY-A: `s[start:end:step]` strided slices — `s[::-1]`,
            // `a[::2]`. A 3-part slice previously failed to parse, which
            // silently dropped the rest of the statement stream.
            if method == "__slice_step__" && arg_ids.len() == 4 {
                let (func, ty) = match receiver_ty.as_ref() {
                    Some(Type::Str) => ("str_slice_step", Type::Str),
                    other => {
                        let elem = match other {
                            Some(Type::Array(e, _)) | Some(Type::DynamicArray(e)) => {
                                (**e).clone()
                            }
                            _ => Type::I64,
                        };
                        (
                            "zeta_slice_vec_step",
                            Type::DynamicArray(Box::new(elem)),
                        )
                    }
                };
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, ty);
                return id;
            }
            if method == "__slice__" && arg_ids.len() == 3 {
                // Python string slicing: `s[1:]` / `s[:3]` / `s[:-1]`.
                // The omitted-end sentinel is `Lit(-1)`; an explicit
                // negative end parses as a unary minus, so the two are
                // told apart here and passed as an explicit flag.
                if matches!(receiver_ty.as_ref(), Some(Type::Str)) {
                    // `args` are the slice bounds only (the receiver is
                    // not part of them): args[0] = start, args[1] = end.
                    let to_end = matches!(
                        args.get(1),
                        Some(AstNode::Lit(v)) if *v == i64::MIN
                    );
                    let flag = self.next_id();
                    self.exprs.insert(flag, MirExpr::IntLit(to_end as i64));
                    self.type_map.insert(flag, Type::I64);
                    self.emit_call_into(id, "str_slice", vec![arg_ids[0], arg_ids[1], arg_ids[2], flag], Type::Str);
                    return id;
                }
                let elem = match receiver_ty.as_ref() {
                    Some(Type::Array(e, _)) => (**e).clone(),
                    Some(Type::DynamicArray(e)) => (**e).clone(),
                    _ => Type::I64,
                };
                let mut args2 = arg_ids.clone();
                // 静态数组不能读 Vec 头：把"省略终点"哨兵（i64::MIN）与字面
                // 负界在编译期按已知长度折成具体界。动态句柄保留原始界，由
                // zeta_slice_vec 按真实长度归一化（旧写法把哨兵改写成 -1，
                // 与显式 `xs[:-1]` 撞成同一个值）。
                if let Some(Type::Array(_, ArraySize::Literal(n))) =
                    receiver_ty.as_ref()
                {
                    let len = *n as i64;
                    let fixed_start = match args.get(0) {
                        Some(AstNode::Lit(v)) if *v < 0 => Some(len + *v),
                        _ => None,
                    };
                    if let Some(v) = fixed_start {
                        let s = self.next_id();
                        self.exprs.insert(s, MirExpr::IntLit(v));
                        self.type_map.insert(s, Type::I64);
                        args2[1] = s;
                    }
                    let fixed_end = match args.get(1) {
                        Some(AstNode::Lit(v)) if *v == i64::MIN => Some(len),
                        Some(AstNode::Lit(v)) if *v < 0 => Some(len + *v),
                        _ => None,
                    };
                    if let Some(v) = fixed_end {
                        let s = self.next_id();
                        self.exprs.insert(s, MirExpr::IntLit(v));
                        self.type_map.insert(s, Type::I64);
                        args2[2] = s;
                    }
                }
                self.stmts.push(MirStmt::Call {
                    func: "zeta_slice_vec".to_string(),
                    args: args2,
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(elem)));
                return id;
            }

            // PY-A: Python list methods. The opaque-receiver fallback
            // just below routes `index`/`count` to the *string* runtime
            // (host_str_find/count) and leaves insert/remove/pop/sort/
            // reverse as bare externs (link failure). For a receiver the
            // compiler KNOWS is an array, dispatch to the list runtime,
            // choosing the element-type-aware variant for value compare.
            if let Some(rt) = receiver_ty.as_ref() {
                let elem = match rt {
                    Type::Array(e, _) | Type::DynamicArray(e) => Some((**e).clone()),
                    _ => None,
                };
                let list_func: Option<String> =
                    match (elem.as_ref(), method.as_str(), arg_ids.len()) {
                        (Some(e), "index", 2) => {
                            Some(format!("zeta_list_index{}", list_elem_suffix(e)))
                        }
                        (Some(e), "count", 2) => {
                            Some(format!("zeta_list_count{}", list_elem_suffix(e)))
                        }
                        (Some(e), "remove", 2) => {
                            Some(format!("zeta_list_remove{}", list_elem_suffix(e)))
                        }
                        // `xs.sort(key=f[, reverse=B])` — key callable
                        // (decorate-sort-undecorate in the runtime).
                        (Some(_), "sort", 2) => Some("py_list_sort_key".to_string()),
                        (Some(_), "sort", 3) => Some("py_list_sort_key".to_string()),
                        (Some(e), "sort", 1) => {
                            Some(format!("zeta_list_sort{}", list_elem_suffix(e)))
                        }
                        (Some(_), "insert", 3) => Some("zeta_list_insert".to_string()),
                        (Some(_), "extend", 2) => Some("py_list_extend".to_string()),
                        (Some(_), "pop", 1) => Some("zeta_list_pop".to_string()),
                        (Some(_), "pop", 2) => Some("zeta_list_pop_at".to_string()),
                        (Some(_), "reverse", 1) => Some("zeta_list_reverse".to_string()),
                        _ => None,
                    };
                if let Some(func) = list_func {
                    let mut call_args = arg_ids.clone();
                    // `xs.sort(key=f)` has no reverse argument; the runtime
                    // signature always takes one, and a missing trailing
                    // argument is garbage (it made sort(key=) behave as if
                    // reversed).
                    if func == "py_list_sort_key" && call_args.len() == 2 {
                        let z = self.next_id();
                        self.exprs.insert(z, MirExpr::IntLit(0));
                        self.type_map.insert(z, Type::I64);
                        call_args.push(z);
                    }
                    self.stmts.push(MirStmt::Call {
                        func,
                        args: call_args,
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    let ret_ty = match method.as_str() {
                        "index" | "count" => Type::I64,
                        "pop" => elem.clone().unwrap_or(Type::slot_fallback()),
                        "sort" | "reverse" => rt.clone(),
                        _ => Type::I64,
                    };
                    self.type_map.insert(id, ret_ty);
                    // insert/remove/sort/reverse are called for their side
                    // effect; insert may move the handle (growth), so rebind
                    // the receiver variable — same reason `push` rebinds.
                    if matches!(
                        method.as_str(),
                        "insert" | "remove" | "sort" | "reverse" | "extend"
                    )
                        && let Some(recv_ast) = receiver
                        && let AstNode::Var(name) = &**recv_ast
                        && let Some(&slot) = self.name_to_id.get(name)
                    {
                        self.stmts.push(MirStmt::Assign { lhs: slot, rhs: id });
                    }
                    return id;
                }
            }

            // B4: dyn receiver → unique W-table method by name only.
            // Also allow I64 leftovers (pre-B3 default) with the same SKIP
            // denylist — unique names like `get`/`keys` must not steal.
            if receiver_ty.as_ref().map_or(false, Type::is_dynamic) {
                if !crate::middle::pylib::NAME_ROUTE_DENYLIST.contains(&method.as_str())
                    && let Some((_handle, symbol, ret_handle, ret)) =
                        crate::middle::pylib::method_by_unique_name(method)
                {
                    self.stmts.push(MirStmt::Call {
                        func: symbol.to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, registry_ret_type(ret_handle, ret));
                    return id;
                }
            }

            // 批次145 重放: list methods on dyn/untyped receivers —
            // `xs.sum()` / `xs.unique()` previously fell to bare externs
            // (`_sum`/`_unique`) and failed to link. The C runtime provides
            // zeta_sum_vec / zeta_vec_unique (order-preserving dedup).
            // Some(I64) 也纳入：字面量列表的元素类型在部分 lowering 轮次
            // 未跟踪（与 Call 路径的 I64 遗留一致）。DynamicArray 与定长
            // Array 也在此拦截（保持元素类型），否则会掉进 opaque 回退的
            // str_fallback 发射裸名 `_unique`/`_sum`（链接失败）。
            let vec_elem = match receiver_ty.as_ref() {
                Some(Type::DynamicArray(e)) => Some((**e).clone()),
                Some(Type::Array(e, _)) => Some((**e).clone()),
                _ => None,
            };
            if matches!(
                receiver_ty.as_ref(),
                Some(Type::PyDynamic)
                    | Some(Type::I64)
                    | Some(Type::DynamicArray(_))
                    | Some(Type::Array(_, _))
                    | None
            ) {
                let elem = vec_elem.unwrap_or(Type::Str);
                let vfunc = match (method.as_str(), arg_ids.len()) {
                    ("sum", 1) => Some(("zeta_sum_vec", 1)),
                    ("unique", 1) => Some(("zeta_vec_unique", 1)),
                    // BATCH-291: `series.dt.strftime(fmt)` — `.dt` passed
                    // the array through; elementwise date format yields a
                    // Str vector (which `unique`/`sorted` then keep as str).
                    ("strftime", 2) => Some(("zeta_vec_strftime", 2)),
                    _ => None,
                };
                if let Some((fname, nargs)) = vfunc {
                    self.stmts.push(MirStmt::Call {
                        func: fname.to_string(),
                        args: arg_ids[..nargs].to_vec(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    // Batch 569: `m.get(k, default)` — a concrete
                    // non-I64 default refines an under-typed map's value
                    // type (the same widening the setdefault arm does),
                    // so both this get's result and later reads render
                    // correctly.
                    if method == "get"
                        && arg_ids.len() == 3
                        && self
                            .type_map
                            .get(&arg_ids[0])
                            .map_or(false, |ty| ty.is_map())
                        && let Some(Type::Named(n, mut targs)) =
                            self.type_map.get(&arg_ids[0]).cloned()
                    {
                        let vt = self
                            .type_map
                            .get(&arg_ids[2])
                            .cloned()
                            .unwrap_or(Type::slot_fallback());
                        let concrete_other = !matches!(vt, Type::I64);
                        while targs.len() < 2 {
                            targs.push(Type::I64);
                        }
                        if concrete_other && targs[1] == Type::I64 {
                            targs[1] = vt;
                            self.type_map.insert(arg_ids[0], Type::Named(n, targs));
                        }
                    }
                    // Batch 568/569: get/setdefault/pop RETURN the map's
                    // value type — read LIVE from type_map (after the
                    // refinement above), not the pre-refinement snapshot.
                    let val_ty = match method.as_str() {
                        "get" | "setdefault" | "pop" => {
                            match self.type_map.get(&arg_ids[0]).cloned() {
                                Some(Type::Named(_, targs)) if targs.len() == 2 => {
                                    targs[1].clone()
                                }
                                _ => Type::I64,
                            }
                        }
                        _ => Type::I64,
                    };
                    self.type_map.insert(
                        id,
                        match method.as_str() {
                            "unique" => Type::DynamicArray(Box::new(elem)),
                            "strftime" => Type::DynamicArray(Box::new(Type::Str)),
                            _ => val_ty,
                        },
                    );
                    return id;
                }
            }

            // 批次 810：`mean` 此前只被下面 opaque 兜底的 pandas 链式臂接走
            // （`("mean", _) => zeta_identity`），而它是那条列表里唯一返回**标量**的
            // 成员 ⇒ 向量句柄当数用（夹具 t810_mean_fold 改前实拍：期望 2.0 打出堆
            // 地址，末行乘 2 也"看着对"）。这里只接静态类型是向量／PyDynamic／未知
            // 标量的接收者：`Type::Named` 一律放行——pylib/pandas.z:339 的
            // GroupBy.mean 库方法表就是靠兜底臂继续生效的，把它折成数值是新造的
            // 静默错值。
            // 批次 10004（#267，cleanup 车道修正随合并入库）：接收者收窄到静态
            // 向量。原先还接 PyDynamic／I64／None 这些"类型未知"的接收者，而折叠
            // 结果一律标 Type::F64——pandas 对象句柄被当浮点存槽后，后续字典下标
            // 打到 codegen 的硬转（静默错值在先、响亮失败在后）。类型未知时不折
            // 叠、交回兜底臂当对象看待。
            // 批次 893（#279 方案②）：接收者分两路——静态向量照旧折叠
            // （F64，元素型已知）；未知型接收者（PyDynamic／I64——运行期可能
            // 是列表也可能是 dict／pandas 对象）走 zeta_mean_to_string：vec
            // 返回均值文本（"20.0"）、非 vec 原样返回句柄（t10004 字典面）。
            // 结果型诚实标 PyDynamic——下游按形状分派（print 的 dyn 渲染／
            // dict 下标各自正确）；静态单型（F64 或 Str）都必毒化一面
            // （890 实测），PyDynamic＋形状分派是方案②的落地形。
            if method == "mean"
                && receiver.is_some()
                && arg_ids.len() == 1
                && matches!(
                    receiver_ty.as_ref(),
                    Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                )
            {
                let elem_is_i64 = match receiver_ty.as_ref() {
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                        matches!(**e, Type::I64)
                    }
                    _ => false,
                };
                let flag = self.next_id();
                self.exprs.insert(flag, MirExpr::IntLit(elem_is_i64 as i64));
                self.type_map.insert(flag, Type::I64);
                self.emit_call_into(id, "zeta_mean_vec", vec![arg_ids[0], flag], Type::F64);
                return id;
            }
            if method == "mean"
                && receiver.is_some()
                && arg_ids.len() == 1
                && matches!(
                    receiver_ty.as_ref(),
                    Some(Type::PyDynamic) | Some(Type::I64)
                )
            {
                // 批次 913（P3）：接收者是 Var 且 checker_env 有已知型时升级
                // ——DynamicArray(F64) ⇒ 折叠 zeta_mean_vec（F64 正确值而非文
                // 本）；Named(map) ⇒ identity（字典面）。查表 miss（参数型
                // Unknown）仍走 mean_to_string（893 语义保留）。
                if let AstNode::Var(recv_name) = &**receiver.as_ref().unwrap() {
                    if let Some(ty) = self.checker_type_of(recv_name) {
                        match &ty {
                            Type::DynamicArray(e) if matches!(**e, Type::F64) => {
                                // checker 已知是 f64 向量 ⇒ 直接折叠（F64 正确值）
                                let flag = self.next_id();
                                self.exprs.insert(flag, MirExpr::IntLit(0));
                                self.type_map.insert(flag, Type::I64);
                                self.emit_call_into(id, "zeta_mean_vec", vec![arg_ids[0], flag], Type::F64);
                                return id;
                            }
                            Type::Named(n, _) if n == "map" => {
                                // checker 已知是 map ⇒ identity（字典面）
                                self.exprs.insert(id, MirExpr::Var(arg_ids[0]));
                                self.type_map.insert(id, ty.clone());
                                return id;
                            }
                            _ => {}
                        }
                    }
                }
                self.emit_call_into(
                    id,
                    "zeta_mean_to_string",
                    vec![arg_ids[0]],
                    Type::PyDynamic,
                );
                return id;
            }

            // Batch 288: slot read of a TYPED element. The comprehension
            // tuple target desugars to `stack_array_get(ELEMENT, i)`; with
            // the element's DynamicArray hint in play, the slot carries the
            // element type instead of a bare i64 — that is how the value
            // half of a json.items() pair stays a PyJson handle.
            if method == "stack_array_get" && receiver.is_none() && arg_ids.len() == 2 {
                let elem_ty = match self.type_map.get(&arg_ids[0]).cloned() {
                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => Some(*e),
                    // Batch 299: the generic comprehension hint (gen.rs
                    // `__collect_dict__`) stores the ELEMENT type on the
                    // lambda param, not `DynamicArray(element)` like the
                    // json.items() branch — so a tuple target's param is
                    // already `Tuple[k, v]`. Without this the per-slot
                    // lookup below never ran and a str key stayed I64.
                    Some(t @ Type::Tuple(_)) => Some(t),
                    _ => None,
                };
                if let Some(elem_ty) = elem_ty {
                    // A PAIR element (`Type::Tuple`) carries per-slot types:
                    // taking the whole tuple for slot 0 left the key typed
                    // as a handle, so `__pack_pair__` never content-hashed
                    // it and `{k: v for k, v in d.items()}` was keyed by
                    // pointer (`"512050.XSHG" in m` → False). The index
                    // literal must come from the AST — the lowered slot id
                    // is a Var, not an IntLit.
                    let slot_ty = match &elem_ty {
                        Type::Tuple(ts) => {
                            let idx = args.get(1).and_then(|a| match a {
                                AstNode::Lit(i) => Some(*i),
                                _ => None,
                            });
                            match idx {
                                Some(i) if i >= 0 => {
                                    ts.get(i as usize).cloned().unwrap_or_else(Type::slot_fallback)
                                }
                                _ => Type::I64,
                            }
                        }
                        other => other.clone(),
                    };
                    self.stmts.push(MirStmt::Call {
                        func: "stack_array_get".to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, slot_ty);
                    return id;
                }
            }

            // PY-A fallback: method calls on unknown/opaque receivers
            // (user structs from undefined modules, BitArray, Sieve,
            // QuantumCircuit) map to runtime equivalents by NAME so
            // object-style tests link and run. V1 heuristic.
            // A method defined on a KNOWN struct must win over the opaque
            // fallback: `c.get("a")` matched `("get", 2) => array_get` and
            // read the struct as an ARRAY (the key became an element offset
            // → SEGFAULT) instead of calling `C::get`.
            let struct_has_method = match receiver_ty.as_ref() {
                Some(Type::Named(tn, _)) => {
                    let qualified = format!("{}::{}", tn, method);
                    self.func_ret_types.contains_key(&qualified)
                        || self.func_ret_types.contains_key(method.as_str())
                }
                _ => false,
            };
            // A receiver whose static TYPE already names a callable route
            // must not be captured by the two arms below: `struct_has_method`
            // only asks for the literal `T::m` / bare `m` keys, while the
            // generic dispatch further down normalizes mangled and dotted
            // spellings (`pandas__DataFrame`, `pd.DataFrame` → `DataFrame::get`,
            // batch 147/291) and siteB dispatches pylib handle tags
            // (`PyJson` → `py_json_get_default`, batch 288). Measured with
            // only `struct_has_method` as a guard: 2 live
            // `py_json_get_default` sites became `map_get_default` on a JSON
            // arena index, plus 2 dead `DataFrame::get` fallback sites.
            let receiver_has_typed_route = match receiver_ty.as_ref() {
                Some(Type::Named(tn, _)) => {
                    self.qualified_method_candidate(tn, method).is_some()
                        || {
                            let tag = crate::middle::pylib::handle_tag(tn)
                                .map(|h| h.to_string())
                                .unwrap_or_else(|| tn.clone());
                            crate::middle::pylib::method_symbol(&tag, method).is_some()
                        }
                }
                _ => false,
            };
            // 批次 420: `.get(<str key>)` on a receiver we cannot statically
            // type is a DICT lookup, but the name table below maps
            // `("get", 2)` to `array_get`, whose load is `base + key*8` —
            // with a string key that uses the key's ADDRESS as an element
            // index (measured pre-fix: `def g(dd): return dd.get("x")` plus
            // one call → rc=139, `ldr x19, [x19, x8, lsl #3]`). Emit the same
            // `MirStmt::DictGet` the typed path uses, so one representation
            // and one ABI survive (`map_get` takes `ptr`, not `i64`).
            if !struct_has_method
                && !receiver_has_typed_route
                && receiver.is_some()
                && method == "get"
                && arg_ids.len() == 2
                && matches!(self.type_map.get(&arg_ids[1]), Some(Type::Str))
                && !receiver_ty.as_ref().map_or(false, |ty| ty.is_map())
            {
                let key_id = self.lower_map_key(arg_ids[1]);
                self.stmts.push(MirStmt::DictGet {
                    map_id: arg_ids[0],
                    key_id,
                    dest: id,
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
                return id;
            }
            // 批次 421: the 3-arg spelling `dd.get(k, default)`. The name
            // table has no `("get", 3)` arm, so this fell all the way
            // through to the bare extern `get` — a weak stub that aborts
            // with "PY-A: `_get` is NOT implemented" (measured pre-fix:
            // rc=134, empty stdout). Emit the same `map_get_default` call
            // the statically-typed map path uses further below.
            //
            // The key must be statically known to be a string: `lower_map_key`
            // only folds it through `map_str_key` (the content hash the map was
            // built with) in that case, so an untyped key would be hashed as a
            // raw handle — a guaranteed miss that returns the default. Measured:
            // `def pick(dd, k): return dd.get(k, "fb")` with k="x" present
            // printed `fb` (rc=0). A silent wrong value is worse than the abort,
            // so untyped keys keep the previous behaviour.
            if !struct_has_method
                && !receiver_has_typed_route
                && method == "get"
                && arg_ids.len() == 3
                && matches!(self.type_map.get(&arg_ids[1]), Some(Type::Str))
                && !receiver_ty.as_ref().map_or(false, |ty| ty.is_map())
            {
                let key_id = self.lower_map_key(arg_ids[1]);
                /* Batch 646: when the default is a STRING (the union
                   shape — int values + a str default), the single dest
                   cannot type both. Return the TAGGED CELL instead
                   (zeta_map_get_default_cell) typed PyJson: the print
                   face's existing PyJson handling renders scalars bare,
                   so both the hit (int) and the miss (str) render as
                   CPython does. */
                let dflt_is_str = matches!(
                    self.type_map.get(&arg_ids[2]),
                    Some(Type::Str)
                );
                if dflt_is_str {
                    let dis = self.next_id_with_lit(1);
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_map_get_default_cell".to_string(),
                        args: vec![arg_ids[0], key_id, arg_ids[2], dis],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::Named("PyJson".to_string(), vec![]));
                    return id;
                }
                self.stmts.push(MirStmt::Call {
                    func: "map_get_default".to_string(),
                    args: vec![arg_ids[0], key_id, arg_ids[2]],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                // Same re-tagging rule as the typed path (batch 291): the
                // default argument is the caller's evidence of the value
                // type; a dynamic receiver carries none of its own.
                let vty = match self.type_map.get(&arg_ids[2]).cloned() {
                    Some(Type::F64) => Type::F64,
                    Some(Type::Str) => Type::Str,
                    _ => Type::I64,
                };
                self.type_map.insert(id, vty);
                return id;
            }
            // Batch 781 (#203⑥)：`v.append(<f64>)` on a DynamicArray
            // receiver — the f64 value must travel as BITS. vec_push is an
            // i64 channel and the codegen coerce fptosi's computed floats
            // (measured: append(10.0) stored 0). zeta_vec_push_f64 takes
            // the double directly; the list's element type widens I64 → F64
            // (the `= []` degenerate guess, 738 principle).
            if method == "append"
                && receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::DynamicArray(_)))
                && arg_ids.len() == 2
                && matches!(
                    self.type_map.get(&arg_ids[1]).cloned(),
                    Some(Type::F64)
                )
            {
                self.stmts.push(MirStmt::Call {
                    func: "zeta_vec_push_f64".to_string(),
                    args: vec![arg_ids[0], arg_ids[1]],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                if let Some(Type::DynamicArray(e)) = receiver_ty.clone() {
                    if matches!(*e, Type::I64) {
                        if let AstNode::Var(vname) = &**receiver.as_ref().unwrap() {
                            if let Some(&slot) = self.name_to_id.get(vname.as_str()) {
                                self.type_map.insert(
                                    slot,
                                    Type::DynamicArray(Box::new(Type::F64)),
                                );
                            }
                        }
                    }
                }
                self.type_map.insert(id, receiver_ty.clone().unwrap());
                return id;
            }

            let opaque_fallback: Option<(&str, &str)> = if receiver.is_some()
                && !struct_has_method
                // 批次 902（轴 F）：is_map 收敛到唯一判定（t 为 &Type，借用
                // 调用无移动）；Named("dict") 幻影型已证（898），is_map 即
                // map/dict 全集。
                && receiver_ty.as_ref().map_or(true, |t| {
                    !(matches!(t, Type::Str) || t.is_map())
                })
            {
                // Untyped receiver (a Python function parameter almost
                // always is): the default arm below falls back to the
                // string runtime so `def f(s): return s.capitalize()` works.
                let str_fallback =
                    str_method_symbol(method.as_str()).map(|(f, _, r)| (f, r));
                match (method.as_str(), arg_ids.len()) {
                    ("get", 2) => Some(("array_get", "i64")),
                    ("set", 3) => Some(("array_set", "i64")),
                    ("push", 2) | ("append", 2) => Some(("vec_push", "i64")),
                    ("len", 1) | ("size", 1) => Some(("vec_len", "i64")),
                    ("get_bit", 2) => Some(("zeta_bit_get", "i64")),
                    ("set_bit", 3) => Some(("zeta_bit_set", "i64")),
                    ("run", 1) => Some(("zeta_sieve_run", "i64")),
                    ("count_primes", 1) => Some(("zeta_sieve_count", "i64")),
                    ("h", 2) | ("x", 2) | ("z", 2) | ("measure", 2) => {
                        Some(("zeta_qc_measure", "i64"))
                    }
                    ("cnot", 3) | ("cz", 3) | ("swap", 3) => {
                        Some(("zeta_qc_noop3", "i64"))
                    }
                    ("execute", 1) => Some(("zeta_qc_execute", "i64")),
                    ("is_normalized", 1) => Some(("zeta_qc_is_normalized", "i64")),
                    ("cx", 2) | ("cx", 3) | ("cz", 2) | ("cz", 3) => {
                        Some(("zeta_qc_measure", "i64"))
                    }
                    ("measure_all", _) => Some(("zeta_qc_measure", "i64")),
                    ("allocate", _) => Some(("zeta_dynarray_new", "i64")),
                    // `.tolist()` on an array we do not have a static type
                    // for: our arrays ARE Vecs, so the list is the same
                    // handle. Identity never dereferences, so an unknown
                    // receiver cannot corrupt data.
                    ("tolist", 1) => Some(("zeta_identity", "vec")),
                    // PY-A: pandas-style chainables route through identity;
                    // ALL other unknown methods also chain by identity so
                    // real-world sources link. (Earlier strict `_ => None`
                    // made every chain a link error.)
                    // 批次 810：单参的 `x.mean()` 在上面向量折叠表后就返回了，
                    // 留在这条的只有多参（`mean(axis=…)`）与 Named 接收者。
                    ("fillna", _) | ("astype", _) | ("shift", _) | ("groupby", _)
                    | ("transform", _) | ("rank", _) | ("sort_values", _)
                    | ("rolling", _) | ("mean", _) | ("to_period", _)
                    | ("set_index", _) => {
                        Some(("zeta_identity", "i64"))
                    }
                    // BATCH-295: `.items()` on an unknown receiver must NOT
                    // chain by identity — the comprehension collector then
                    // reads the MAP handle as a Vec header and SEGFAULTS
                    // (measured: `[s for s, p in
                    // context.portfolio.positions.items()]` in
                    // `_parity_snapshot`, crash at `zeta_collect_vec_n+40`).
                    // In this codebase a `.items()` receiver is a dict;
                    // py_map_items yields the pair-vec the loops expect.
                    ("items", 1) => Some(("py_map_items", "vec")),
                    // Unknown method on an untyped receiver: string runtime
                    // (carrying the method's result kind, so `.capitalize()`
                    // stays a string and `.split()` stays a list).
                    _ => str_fallback,
                }
            } else {
                None
            };
            // 批次 753（#45 第十一成员落点三）：`x.to_string()` 在非 str
            // 接收者上过去落到下面 opaque_fallback 的 `_ => str_fallback`
            // 臂——host_str_to_string 把 i64/f64/bool 的**值**当 char* 解
            // 引用，运行期 rc=139（改前实拍：六形状里除 `a: str` 外全崩，
            // 编译 rc=0 且带一条 fptosi ABI 警告）。接收者静态类型已知且
            // 属 repr 通道可渲染家族时，改发 str() 家族同款 `lower_to_string`
            // （f-string 部件、批次 742 str(d) 的那条）。PyDynamic／未知
            // 类型接收者不动＝363 在册的未定型 str 兜底语义保留，未定型
            // struct 接收者亦别把崩溃换成静默句柄值。
            if method == "to_string"
                && arg_ids.len() == 1
                && receiver_ty.as_ref().map_or(false, repr_routable)
            {
                return self.lower_to_string(arg_ids[0]);
            }
            if let Some((func, ret)) = opaque_fallback {
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(
                    id,
                    match ret {
                        "str" => Type::Str,
                        "bool" => Type::Bool,
                        "split" => Type::DynamicArray(Box::new(Type::Str)),
                        "vec" => Type::DynamicArray(Box::new(Type::I64)),
                        _ => Type::I64,
                    },
                );
                // vec_push may reallocate and returns the (possibly new)
                // handle. Python's `lst.append(x)` discards the return, so
                // rebind the receiver variable — otherwise growth is lost
                // once the array's capacity is exceeded.
                if func == "vec_push"
                    && let Some(recv_ast) = receiver
                    && let AstNode::Var(name) = &**recv_ast
                    && let Some(&slot) = self.name_to_id.get(name)
                {
                    self.stmts.push(MirStmt::Assign { lhs: slot, rhs: id });
                }
                // `lst = []` followed by `lst.append(x)`: an EMPTY literal
                // typed the element I64, which is "unknown" — not "numeric".
                // `x in lst` then passed elem_is_str=0 and compared HANDLES, so
                // every string counted as absent (measured:
                // `[c for c in items if c not in fetched]` kept everything, and
                // `fetch_stocks`'s source loop carried on with the wrong set).
                // Refine I64 -> the appended value's type.
                if func == "vec_push" && arg_ids.len() == 2 {
                    if let Some(AstNode::Var(name)) = receiver.as_ref().map(|r| &**r) {
                        if let Some(&slot) = self.name_to_id.get(name) {
                            let elem_is_i64 = matches!(
                                self.type_map.get(&slot),
                                Some(Type::DynamicArray(e)) if matches!(**e, Type::I64)
                            );
                            if elem_is_i64 {
                                let vty = self
                                    .type_map
                                    .get(&arg_ids[1])
                                    .cloned()
                                    .unwrap_or(Type::slot_fallback());
                                if !matches!(vty, Type::I64) {
                                    self.type_map
                                        .insert(slot, Type::DynamicArray(Box::new(vty)));
                                }
                            }
                        }
                    }
                }
                return id;
            }

            // PY-A: comprehension over literal elements — variadic map.
            // args = [e1..en, lam]; emit zeta_collect_literals(count, lam, e1..en).
            if method == "__collect_literals__" && args.len() >= 1 {
                let lam = args[args.len() - 1].clone();
                let elems = &args[..args.len() - 1];
                let mut lowered_elems = Vec::new();
                for a in elems {
                    lowered_elems.push(self.lower_expr(a));
                }
                let lam_id = self.lower_expr(&lam);
                let count_id = self.next_id();
                self.exprs
                    .insert(count_id, MirExpr::IntLit(lowered_elems.len() as i64));
                self.type_map.insert(count_id, Type::I64);
                let mut call_args = vec![count_id, lam_id];
                call_args.extend(lowered_elems);
                self.stmts.push(MirStmt::Call {
                    func: "zeta_collect_literals".to_string(),
                    args: call_args,
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                return id;
            }

            // PY-A: dict comprehension collect — lambda returns packed
            // (k<<32)|v pairs via __pack_pair__; runtime fills a map.
            if method == "__collect_dict__" && arg_ids.len() == 2 {
                self.stmts.push(MirStmt::Call {
                    func: "zeta_collect_dict".to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                let pair_ty = self.last_dict_pair_ty.take();
                let map_ty = match pair_ty {
                    Some((k, v)) => Type::Named(
                        "map".to_string(),
                        vec![
                            if matches!(k, Type::Str) {
                                Type::Str
                            } else {
                                Type::I64
                            },
                            v,
                        ],
                    ),
                    None => Type::Named("map".to_string(), vec![]),
                };
                self.type_map.insert(id, map_ty);
                return id;
            }
            // __pack_pair__(k, v) — the runtime keeps the pair as a 2-slot
            // heap pair, so string keys must be content-hashed first (the
            // same `lower_map_key` rule a dict literal uses); otherwise the
            // map would key by pointer and `d[k]` lookups would miss.
            // String VALUES are refused loudly further down instead of being
            // silently stored as a pointer-to-i64.
            if method == "__pack_pair__" && arg_ids.len() == 2 {
                let k_ty = self.type_map.get(&arg_ids[0]).cloned().unwrap_or_else(Type::slot_fallback);
                let v_ty = self.type_map.get(&arg_ids[1]).cloned().unwrap_or_else(Type::slot_fallback);
                self.last_dict_pair_ty = Some((k_ty.clone(), v_ty.clone()));
                if matches!(k_ty, Type::Str) {
                    let hashed = self.emit_call("map_str_key", vec![arg_ids[0]], Type::I64);
                    arg_ids[0] = hashed;
                }
            }
            if method == "__pack_pair__" && arg_ids.len() == 2 {
                self.stmts.push(MirStmt::Call {
                    func: "zeta_pack_pair".to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // PY-A: list comprehension collect — receiver is the iterable,
            // arg is the lambda FuncAddr. zeta_collect_vec returns a new
            // Vec handle skipping -1 (filtered-out) results.
            if method == "__collect__" && arg_ids.len() == 2 {
                // Element type from the lambda's body (a list of strings
                // must be Vec<str>, not Vec<i64>) — `last_closure_ret_ty`
                // was recorded while the lambda above was lowered.
                let collected_elem = self.last_closure_ret_ty.clone();
                let len_id = match self.type_map.get(&arg_ids[0]).cloned() {
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
                self.stmts.push(MirStmt::Call {
                    func: "zeta_collect_vec_n".to_string(),
                    args: vec![arg_ids[0], arg_ids[1], len_id],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                // Carry the lambda's body type, so a list of strings is
                // Vec<str> and `x[0]` prints text instead of a pointer.
                self.type_map.insert(
                    id,
                    Type::DynamicArray(Box::new(
                        collected_elem.unwrap_or(Type::slot_fallback()),
                    )),
                );
                return id;
            }

            // `map.get(k[, d])` returns the map's VALUE type. Hardcoding
            // I64 made the caller compare a str handle against a string
            // (`SOURCE_STRATEGIES.get(source, "full") == "skip"` in
            // calibration.py) — the comparison still worked, but `print`
            // showed the raw handle, i.e. one silent wrong VALUE per use.
            let map_value_ty = match receiver_ty.as_ref() {
                Some(Type::Named(_, targs)) => targs.get(1).cloned().unwrap_or_else(Type::slot_fallback),
                _ => Type::I64,
            };
            // PY-A: dict methods — Python d.get(k) (missing key → 0)
            if method == "get"
                && receiver_ty
                    .as_ref()
                    .map_or(false, Type::is_map)
                && arg_ids.len() == 2
            {
                // Same codegen path as d[k] subscripting (DictGet)
                let key_id = self.lower_map_key(arg_ids[1]);
                self.stmts.push(MirStmt::DictGet {
                    map_id: arg_ids[0],
                    key_id,
                    dest: id,
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, map_value_ty);
                return id;
            }
            // PY-A: d.get(k, default) — 3-arg form via runtime
            if method == "get"
                && receiver_ty
                    .as_ref()
                    .map_or(false, Type::is_map)
                && arg_ids.len() == 3
            {
                /* Batch 646: an ERASED map value type (I64/PyDynamic)
                   with a STRING default is the genuine union — the hit
                   (int) and the miss (str) cannot share one dest type.
                   Return the TAGGED CELL (the runtime's per-key value
                   tag types the hit; the default's kind types the miss)
                   typed PyJson, rendered bare by the print face. */
                let erased_value_ty = matches!(
                    &map_value_ty,
                    Type::I64 | Type::PyDynamic
                );
                let str_default = matches!(
                    self.type_map.get(&arg_ids[2]),
                    Some(Type::Str)
                );
                if erased_value_ty && str_default {
                    let key_id = self.lower_map_key(arg_ids[1]);
                    let dis = self.next_id_with_lit(1);
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_map_get_default_cell".to_string(),
                        args: vec![arg_ids[0], key_id, arg_ids[2], dis],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::Named("PyJson".to_string(), vec![]));
                    return id;
                }
                let key_id = self.lower_map_key(arg_ids[1]);
                self.stmts.push(MirStmt::Call {
                    func: "map_get_default".to_string(),
                    args: vec![arg_ids[0], key_id, arg_ids[2]],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                // Batch 291: a bare `dict` annotation erases the value type
                // to I64 (lt_annotation_type), so `config.get("slippage",
                // 0.0)` on a dict PARAMETER read back the stored double
                // BITS as an integer — `4562254508917369340` instead of
                // `0.001` (measured). The default argument carries the
                // expected value type; use it to re-tag when the map's own
                // value type is the erased I64 default.
                let vty = match (
                    &map_value_ty,
                    self.type_map.get(&arg_ids[2]).cloned(),
                ) {
                    (Type::I64, Some(Type::F64)) => Type::F64,
                    (Type::I64, Some(Type::Str)) => Type::Str,
                    // Batch 576: under-typed map fields (`self.data = {}`
                    // → PyDynamic placeholders) with a concrete default.
                    (Type::PyDynamic, Some(Type::Str)) => Type::Str,
                    (Type::PyDynamic, Some(Type::F64)) => Type::F64,
                    _ => map_value_ty.clone(),
                };
                self.type_map.insert(id, vty);
                return id;
            }

            // PY-A: dict update / pop / clear. `map` receivers are excluded
            // from the opaque fallback, so these previously fell through to
            // bare externs (`_update`/`_pop`/`_clear`) and failed to link.
            if receiver_ty
                .as_ref()
                .map_or(false, Type::is_map)
            {
                // Batch 568b: `d = {}` then `d.setdefault("k", "v")` — the
                // empty literal carries NO value type, so d["k"] rendered
                // the raw pointer. Refine d's value type from the
                // default's static type at this call.
                if method == "setdefault"
                    && arg_ids.len() == 3
                    && let Some(Type::Named(n, mut targs)) =
                        self.type_map.get(&arg_ids[0]).cloned()
                {
                    // Empty literals spell their placeholders as
                    // [I64, I64]; a CONCRETE non-I64 default widens the
                    // value type (Str default -> values read as Str).
                    let vt = self
                        .type_map
                        .get(&arg_ids[2])
                        .cloned()
                        .unwrap_or(Type::slot_fallback());
                    let concrete_other = !matches!(vt, Type::I64);
                    while targs.len() < 2 {
                        targs.push(Type::I64);
                    }
                    if concrete_other && targs[1] == Type::I64 {
                        targs[1] = vt;
                        self.type_map.insert(arg_ids[0], Type::Named(n, targs));
                    }
                }
                let dfunc = match (method.as_str(), arg_ids.len()) {
                    ("update", 2) => Some("zeta_map_update"),
                    ("pop", 2) => Some("zeta_map_pop"),
                    ("pop", 3) => Some("zeta_map_pop_default"),
                    ("clear", 1) => Some("zeta_map_clear"),
                    ("setdefault", 3) => Some("py_map_setdefault"),
                    // 批次145 重放: `del d[k]` desugars to
                    // `d.__delitem__(k)`; batch 127 mapped it to
                    // zeta_map_pop — lost in the 143 restore (t239).
                    ("__delitem__", 2) => Some("zeta_map_pop"),
                    _ => None,
                };
                if let Some(fname) = dfunc {
                    // pop keys are content-hashed exactly like subscripting
                    // (update's second operand is another map, not a key).
                    let call_args = match (method.as_str(), arg_ids.len()) {
                        ("update", 2) => vec![arg_ids[0], arg_ids[1]],
                        ("pop", 2) => vec![arg_ids[0], self.lower_map_key(arg_ids[1])],
                        ("__delitem__", 2) => {
                            vec![arg_ids[0], self.lower_map_key(arg_ids[1])]
                        }
                        ("pop", 3) => vec![
                            arg_ids[0],
                            self.lower_map_key(arg_ids[1]),
                            arg_ids[2],
                        ],
                        ("setdefault", 3) => vec![
                            arg_ids[0],
                            self.lower_map_key(arg_ids[1]),
                            arg_ids[2],
                        ],
                        _ => vec![arg_ids[0]],
                    };
                    self.stmts.push(MirStmt::Call {
                        func: fname.to_string(),
                        args: call_args,
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    // Batch 568: setdefault returns the inserted-or-existing
                    // VALUE — type it from the map's declared value type so
                    // `x = d.setdefault("k", "v")` prints `v`, not the
                    // pointer.
                    let ret_ty = match (method.as_str(), receiver_ty.as_ref()) {
                        ("setdefault", Some(Type::Named(_, targs)))
                            if targs.len() == 2 =>
                        {
                            targs[1].clone()
                        }
                        _ => Type::I64,
                    };
                    self.type_map.insert(id, ret_ty);
                    return id;
                }
            }

            // PY-A: Vec::new() → runtime vec_new with initial capacity
            // (vec_push growth doubles from cap, so cap must be > 0)
            if (method == "Vec::new" || (method == "new" && matches!(
                receiver.as_deref(),
                Some(AstNode::Var(v)) if v == "Vec"
            ))) && args.is_empty() {
                let cap_id = self.next_id();
                self.exprs.insert(cap_id, MirExpr::IntLit(8));
                self.type_map.insert(cap_id, Type::I64);
                self.emit_call_into(id, "vec_new", vec![cap_id], Type::DynamicArray(Box::new(Type::I64)));
                return id;
            }

            // 批次 550: `d.copy()` / `d.popitem()` on a dict, `xs.copy()`
            // on a list. The generic member path emitted `map::copy` →
            // undefined `map__copy` (LINK_FAIL), and a list `.copy()`
            // fell into the identity builtin and returned the SAME
            // handle — copies that alias their source.
            if method == "copy"
                && arg_ids.len() == 1
                && matches!(
                    receiver_ty.as_ref(),
                    Some(t) if t.is_map()
                )
            {
                let ret = receiver_ty.as_ref().cloned().unwrap_or_else(Type::slot_fallback);
                self.stmts.push(MirStmt::Call {
                    func: "map__copy".to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, ret);
                return id;
            }
            if method == "popitem"
                && arg_ids.len() == 1
                && matches!(
                    receiver_ty.as_ref(),
                    Some(ty @ Type::Named(_, ts)) if ty.is_map() && ts.len() == 2
                )
            {
                let (kt, vt) = match receiver_ty.as_ref() {
                    Some(Type::Named(_, ts)) => (ts[0].clone(), ts[1].clone()),
                    _ => (Type::Str, Type::I64),
                };
                self.stmts.push(MirStmt::Call {
                    func: "map__popitem".to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::Named("tuple".to_string(), vec![kt, vt]));
                return id;
            }
            if method == "copy"
                && arg_ids.len() == 1
                && receiver_ty.as_ref().map_or(false, |t| matches!(t, Type::DynamicArray(_)))
            {
                let ret = receiver_ty.as_ref().cloned().unwrap_or_else(Type::slot_fallback);
                self.stmts.push(MirStmt::Call {
                    func: "zeta_vec_copy".to_string(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, ret);
                return id;
            }
            // PY-A: string methods — dispatch to host_str_* runtime by
            // receiver type (Python s.upper()/s.contains(x)/... )
            if receiver_ty.as_ref().map_or(false, |t| matches!(t, Type::Str)) {
                // `s.replace(old, new, count)` — bounded replacement.
                if method == "replace" && arg_ids.len() == 4 {
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_replace_n".to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                // `s.strip(chars)` / lstrip / rstrip — a CHARACTER SET, not
                // a substring (the 2-argument form had no arity match and
                // fell to a bare extern).
                if matches!(method.as_str(), "strip" | "lstrip" | "rstrip")
                    && arg_ids.len() == 2
                {
                    let func = match method.as_str() {
                        "lstrip" => "host_str_lstrip_chars",
                        "rstrip" => "host_str_rstrip_chars",
                        _ => "host_str_strip_chars",
                    };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                // `s.split(sep, maxsplit)` — bounded split.
                if method == "split" && arg_ids.len() == 3 {
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_split_max".to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::DynamicArray(Box::new(Type::Str)));
                    return id;
                }
                // `s.split()` (no separator) splits on whitespace runs.
                if method == "split" && arg_ids.len() == 1 {
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_split_ws".to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::DynamicArray(Box::new(Type::Str)));
                    return id;
                }
                // 批次 549: `s.rsplit(sep[, maxsplit])` — split from the
                // RIGHT, at most maxsplit pieces (negative = unlimited).
                // The bare member path emitted an untyped extern and every
                // element read rendered a raw pointer; these arms pin the
                // symbol and the result type.
                if method == "rsplit" && (arg_ids.len() == 2 || arg_ids.len() == 3) {
                    let mut args = arg_ids.clone();
                    if args.len() == 2 {
                        args.push(self.next_id_with_lit(-1));
                    }
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_rsplit".to_string(),
                        args,
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::DynamicArray(Box::new(Type::Str)));
                    return id;
                }
                // `s.partition(sep)` / `s.rpartition(sep)` — the
                // (head, sep, tail) triple; the per-position types drive
                // the destructured names to Str (same source-typing rule
                // as the tuple destructure above).
                if matches!(method.as_str(), "partition" | "rpartition")
                    && arg_ids.len() == 2
                {
                    let func = if method == "partition" {
                        "host_str_partition"
                    } else {
                        "host_str_rpartition"
                    };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    // Named("tuple", …) is the codebase convention for a
                    // tuple that came from a CALL (the subscript and
                    // destructure paths both recognize it; a bare
                    // Type::Tuple from a call is treated as dict-typed and
                    // `p[0]` compiled into a map_get).
                    self.type_map.insert(
                        id,
                        Type::Named(
                            "tuple".to_string(),
                            vec![Type::Str, Type::Str, Type::Str],
                        ),
                    );
                    return id;
                }
                // `s.expandtabs()` — Python's default tabsize 8 (V1: fixed).
                if method == "expandtabs" && arg_ids.len() == 1 {
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_expandtabs".to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                // ljust/rjust/center take width[, fillchar] — Python's
                // common form has ONE argument after the receiver, so the
                // 3-arity table entry alone never matched.
                if let Some(fname) = match (method.as_str(), arg_ids.len()) {
                    ("ljust", 2) => Some("host_str_ljust2"),
                    ("ljust", 3) => Some("host_str_ljust"),
                    ("rjust", 2) => Some("host_str_rjust2"),
                    ("rjust", 3) => Some("host_str_rjust"),
                    ("center", 2) => Some("host_str_center2"),
                    ("center", 3) => Some("host_str_center"),
                    _ => None,
                } {
                    self.stmts.push(MirStmt::Call {
                        func: fname.to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                // `"{} and {}".format(a, b)` on a LITERAL template: rewrite
                // to an f-string so the existing to_string_* dispatch
                // formats each argument by its own type.
                if method == "format" {
                    if let Some(recv) = receiver {
                        if let AstNode::StringLit(tmpl) = &**recv {
                            // Split keyword (`__kwarg__`) args from positional
                            // ones: `"{a}".format(a=1)` is resolved by NAME,
                            // and auto-indexing must only count positional.
                            let mut named: Vec<(String, AstNode)> = Vec::new();
                            let mut positional: Vec<AstNode> = Vec::new();
                            for a in args {
                                match a {
                                    AstNode::Call { method: km, args: ka, .. }
                                        if km == "__kwarg__" && ka.len() == 2 =>
                                    {
                                        if let AstNode::StringLit(n) = &ka[0] {
                                            named.push((n.clone(), ka[1].clone()));
                                        }
                                    }
                                    _ => positional.push(a.clone()),
                                }
                            }
                            if let Some(parts) =
                                format_template_parts(tmpl, &positional, &named)
                            {
                                return self.lower_expr(&AstNode::FString(parts));
                            }
                            // Named fields (`"{a}".format(a=…)`) with no
                            // positional fallback still cannot be rewritten —
                            // say so here instead of leaving a bare `format`
                            // symbol for the linker to report.
                            if !named.is_empty() {
                                static WARNED_FMT: std::sync::OnceLock<()> =
                                    std::sync::OnceLock::new();
                                WARNED_FMT.get_or_init(|| {
                                    eprintln!(
                                        "error: str.format with named fields could not be \
                                         rewritten — the call will link against an \
                                         undefined `format` symbol"
                                    );
                                });
                            }
                        }
                        // Batch 588: a receiver that is NOT a literal (or a
                        // literal whose rewrite failed with only positional
                        // args left) — the template lives at runtime, so
                        // render every argument by its own type (the same
                        // `lower_to_string` an f-string part uses), pass a
                        // Vec<str> plus the template to the runtime shim.
                        // `{name}` without named args renders literally
                        // (CPython raises; fail-soft corner, batch 588).
                        let mut named: Vec<(String, AstNode)> = Vec::new();
                        let mut positional: Vec<AstNode> = Vec::new();
                        for a in args {
                            match a {
                                AstNode::Call { method: km, args: ka, .. }
                                    if km == "__kwarg__" && ka.len() == 2 =>
                                {
                                    if let AstNode::StringLit(n) = &ka[0] {
                                        named.push((n.clone(), ka[1].clone()));
                                    }
                                }
                                _ => positional.push(a.clone()),
                            }
                        }
                        if named.is_empty() {
                            if positional.is_empty() {
                                // `t.format()` — a template is returned
                                // unchanged when no argument is consumed;
                                // CPython only raises when the template
                                // still holds a replacement field (corner).
                                return self.lower_expr(recv);
                            }
                            let rid = self.lower_expr(recv);
                            let mut elem_ids: Vec<u32> = Vec::new();
                            for a in &positional {
                                let aid = self.lower_expr(a);
                                let sid = self.lower_to_string(aid);
                                elem_ids.push(sid);
                            }
                            let cap_id = self.next_id_with_lit(positional.len() as i64);
                            let mut vec_id = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "vec_new".to_string(),
                                args: vec![cap_id],
                                dest: vec_id,
                                type_args: vec![],
                            });
                            self.exprs.insert(vec_id, MirExpr::Var(vec_id));
                            self.type_map
                                .insert(vec_id, Type::DynamicArray(Box::new(Type::Str)));
                            for e in &elem_ids {
                                let nid = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: "vec_push".to_string(),
                                    args: vec![vec_id, *e],
                                    dest: nid,
                                    type_args: vec![],
                                });
                                self.exprs.insert(nid, MirExpr::Var(nid));
                                self.type_map
                                    .insert(nid, Type::DynamicArray(Box::new(Type::Str)));
                                vec_id = nid;
                            }
                            self.emit_call_into(id, "host_str_format", vec![rid, vec_id], Type::Str);
                            return id;
                        }
                    }
                }
                // 批次 753（#45 第十一成员落点三）：`x.to_string()` 在非 str
                // 接收者上过去被本表恒发 host_str_to_string——i64/f64/bool 的
                // 值被当 char* 解引用，运行期 rc=139（改前实拍五形状仅 str
                // 接收者一形通过）。已知非 str 接收者走 str() 家族同款 repr
                // 通道（`lower_to_string`＝f-string 部件、批次 742 str(d) 的
                // 那条）。未知形状（PyDynamic）与其他 Named 不动：363 在册的
                // 未定类型接收者保留 str 兜底语义，别把崩溃换成静默句柄值。
                if method == "to_string" && arg_ids.len() == 1 {
                    let routable = match self.type_map.get(&arg_ids[0]).cloned() {
                        Some(Type::I8) | Some(Type::I16) | Some(Type::I32)
                        | Some(Type::I64) | Some(Type::U8) | Some(Type::U16)
                        | Some(Type::U32) | Some(Type::U64) | Some(Type::Usize)
                        | Some(Type::F32) | Some(Type::F64) | Some(Type::Bool)
                        | Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                        | Some(Type::Tuple(_)) => true,
                        Some(ty @ Type::Named(..)) => ty.is_map(),
                        _ => false,
                    };
                    if routable {
                        return self.lower_to_string(arg_ids[0]);
                    }
                }
                // 批次 794（#266）：Rust 风格 `word.push(ch)` 在 Str 接收者上。
                // 重绑定约定抄 vec push（上方 DynamicArray 臂）：
                // host_str_push_str 是纯函数（返回新句柄），而语句形
                // `word.push(c)` 会丢弃返回值——不写回槽位 word 永远为空
                // （selfhost.z 分词器的 word 累积即此形，改前链接失败无在跑依赖）。
                if method == "push" && arg_ids.len() == 2 {
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_push_str".to_string(),
                        args: arg_ids.clone(),
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Str);
                    if let Some(recv_ast) = receiver
                        && let AstNode::Var(name) = &**recv_ast
                        && let Some(&slot) = self.name_to_id.get(name)
                    {
                        self.stmts.push(MirStmt::Assign { lhs: slot, rhs: id });
                    }
                    return id;
                }
                let mut m = str_method_symbol(method.as_str());
                // Batch 588: the start-offset forms — `s.find(sub, start)`
                // maps to the 3-argument shim when a third argument is
                // present (table rows are single-arity; without this the
                // 3-arg form fell through and linked a bare `find`).
                if arg_ids.len() == 3 {
                    if let Some(f3) = str_method_symbol3(method.as_str()) {
                        m = Some(f3);
                    }
                }
                // 批次 794（#266）：Rust `ch.is_digit(10)` 的 radix 形——改前落
                // 裸别名 is_digit（一参，radix 被忽略）且返回值定型 I64，
                // 打印 1 而非 True（/tmp/b782/push4.z 实拍）。
                if method == "is_digit" && arg_ids.len() == 2 {
                    m = Some(("host_str_is_digit", 2, "bool"));
                }
                // Batch 588: `rsplit(sep)` without maxsplit equals
                // `split(sep)` in CPython.
                if method == "rsplit" && arg_ids.len() == 2 {
                    m = Some(("host_str_split", 2, "split"));
                }
                if let Some((func, argc, ret)) = m {
                    if ret == "split" {
                        // returns a Vec handle of string elements
                        self.stmts.push(MirStmt::Call {
                            func: func.to_string(),
                            args: arg_ids.clone(),
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map
                            .insert(id, Type::DynamicArray(Box::new(Type::Str)));
                        return id;
                    }
                    if arg_ids.len() == argc {
                        self.stmts.push(MirStmt::Call {
                            func: func.to_string(),
                            args: arg_ids.clone(),
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(
                            id,
                            match ret {
                                "str" => Type::Str,
                                "bool" => Type::Bool,
                                _ => Type::I64,
                            },
                        );
                        return id;
                    }
                }
            }

            // A handle-tag receiver (`PyDate`, `PyPath`, …) whose handle
            // could not be derived from the AST — `datetime.datetime(…)
            // .strftime(…)` with no `import datetime` resolves the
            // constructor late, so `py_handle_of` saw nothing — still has a
            // statically known tag. Dispatch the W-table shim; without this
            // the Named branch below emitted `PyDate::strftime`, i.e. an
            // undefined `_PyDate__strftime` (t220).
            if let Some(Type::Named(tn, _)) = receiver_ty.as_ref() {
                // Batch 573: a USER-CLASS method shadows the W-table
                // builtin of the same name (`Counter.get()` must call the
                // class's own `get`, not the dict shim this arm would
                // emit for the registry's `map` tag).
                if self
                    .qualified_method_candidate(tn, method)
                    .is_none()
                {
                // The receiver's type may carry the PYTHON class spelling
                // (`Path`, `Timestamp`) rather than the registry handle tag
                // (`PyPath`). Resolve it through the same table the parameter
                // path uses, otherwise `cache_path.write_text(...)` emitted
                // `Path::write_text` (undefined) even though `W PyPath
                // write_text` exists.
                let tag = crate::middle::pylib::handle_tag(tn)
                    .map(|h| h.to_string())
                    .unwrap_or_else(|| tn.clone());
                if let Some((sym, ret_handle)) =
                    crate::middle::pylib::method_symbol(&tag, method)
                {
                    // Batch 288: siteB mirror of siteA's PyJson `get`
                    // padding (see the tag==PyJson block upstream) — the
                    // registry declares the 4-argument form
                    // (recv, key, default, default-tag). Reached when the
                    // receiver's tag comes from its TYPE (a comprehension
                    // pair slot) rather than the AST.
                    let mut lowered = arg_ids.clone();
                    if tag == "PyJson" && method == "get" {
                        if lowered.len() == 2 {
                            let z = self.next_id();
                            self.exprs.insert(z, MirExpr::IntLit(0));
                            self.type_map.insert(z, Type::I64);
                            lowered.push(z);
                        }
                        if lowered.len() == 3 {
                            let dtag: i64 = match self.type_map.get(&lowered[2]) {
                                Some(Type::Str) => 2,
                                Some(Type::F64) | Some(Type::F32) => 1,
                                _ => 0,
                            };
                            let t = self.next_id();
                            self.exprs.insert(t, MirExpr::IntLit(dtag));
                            self.type_map.insert(t, Type::I64);
                            lowered.push(t);
                        }
                    }
                    self.stmts.push(MirStmt::Call {
                        func: sym.to_string(),
                        args: lowered,
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    // `map.get(k, d)` returns the map's VALUE type — the W
                    // table can only declare a coarse `ret=i64`, and with
                    // that the caller compared a str handle against a string
                    // (`SOURCE_STRATEGIES.get(source, "full") == "skip"` in
                    // calibration.py) — a SILENT wrong answer. `-> dict`
                    // annotations now reach here (batch 155), so the V type
                    // must come from the receiver.
                    let map_value_ty = if tag == "map" && method == "get" {
                        match receiver_ty.as_ref() {
                            Some(Type::Named(_, targs)) => {
                                targs.get(1).cloned().unwrap_or_else(Type::slot_fallback)
                            }
                            _ => Type::I64,
                        }
                    } else {
                        Type::I64
                    };
                    // Batch 576: the DEFAULT argument is the caller's own
                    // evidence of the value type (batch 291 re-tagging
                    // rule). An under-typed map field (`self.data = {}` —
                    // placeholders [I64, I64]) with a Str default returned
                    // the default's POINTER typed I64.
                    let dty = arg_ids
                        .get(2)
                        .and_then(|i| self.type_map.get(i).cloned());
                    let map_value_ty = match (&map_value_ty, dty.as_ref()) {
                        (Type::I64, Some(Type::Str)) => Type::Str,
                        (Type::I64, Some(Type::F64)) => Type::F64,
                        (Type::PyDynamic, Some(Type::Str)) => Type::Str,
                        (Type::PyDynamic, Some(Type::F64)) => Type::F64,
                        _ => map_value_ty,
                    };
                    let ty = match ret_handle {
                        Some(h) => Type::Named(h.to_string(), vec![]),
                        None if tag == "map" && method == "get" => map_value_ty,
                        None => match crate::middle::pylib::method_ret(&tag, method) {
                            Some("str") => Type::Str,
                            Some("f64") => Type::F64,
                            Some("vecstr") => Type::DynamicArray(Box::new(Type::Str)),
                            Some("vecpath") => Type::DynamicArray(Box::new(
                                Type::Named("PyPath".to_string(), vec![]),
                            )),
                            Some("vecjson") => Type::DynamicArray(Box::new(
                                Type::Named("PyJson".to_string(), vec![]),
                            )),
                            Some("vecmatch") => Type::DynamicArray(Box::new(
                                Type::Named("PyMatch".to_string(), vec![]),
                            )),
                            _ => Type::I64,
                        },
                    };
                    self.type_map.insert(id, ty);
                    return id;
                }
                }
            }

            // Check if this is a method call on a dynamic array
            // BATCH-294: call-through-a-value. `for routine in [morning_
            // routine, …]: routine(context)` — the callee NAME is bound to
            // a local variable holding a function address (FuncAddr lowers
            // to its i64 pointer), so a static symbol would link against
            // the weak `_routine` abort stub. Dispatch via the C
            // trampoline instead. Only when no global function of that
            // name exists (those keep priority). A `lambda` bound to a name
            // lives in `closure_vars` and keeps its recorded return type
            // through the closure path below — the trampoline would erase it
            // (t129: `f = lambda s: s.upper()` printed a heap pointer).
            //
            // 批次 395: BATCH-294 shipped exactly one trampoline, so this
            // arm only had one arity. `routine()` and `routine(a, b)` fell
            // straight through to the symbol path and died at link time
            // with the LOCAL variable's name (`let f = add2; f(3, 4)` ->
            // Undefined symbols "_f"; tests/unit-tests/quantum_basic.z:136
            // `test_fn()` -> "_test_fn"). Arity 0..4 now dispatch through
            // `zeta_call<argc>`; 5 and up still take the symbol path.
            if receiver.is_none()
                && arg_ids.len() <= 4
                && !self.func_ret_types.contains_key(method.as_str())
                && !self.closure_vars.contains_key(method.as_str())
            {
                if let Some(&vid) = self.name_to_id.get(method.as_str()) {
                    let is_value_slot = matches!(
                        self.exprs.get(&vid),
                        Some(MirExpr::Var(_)) | Some(MirExpr::FuncAddr(_))
                    );
                    if is_value_slot {
                        let mut call_args = vec![vid];
                        call_args.extend_from_slice(&arg_ids);
                        self.stmts.push(MirStmt::Call {
                            func: format!("zeta_call{}", arg_ids.len()),
                            args: call_args,
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
                        return id;
                    }
                }
            }

            // Batch 397: set when a float-receiver method is bound to a
            // `py_math_*` registry symbol; carries that symbol's declared
            // return so the dest slot is not the i64 default.
            let mut float_math_ret: Option<&'static str> = None;
            // Batch 432: set when a call that would degrade to a BARE symbol
            // resolves to exactly one implemented `W` entry of matching
            // arity. Carries the entry's slot type. Also keeps the call out of
            // the `_<argc>` disambiguation below: the registry symbol is the
            // runtime C name, which carries no suffix.
            let mut bare_registry: Option<Type> = None;
            let (func, is_array_len, is_array_push) = if let Some(ref rty) = receiver_ty {
                // Check if receiver is a dynamic array type
                if let Type::DynamicArray(_) = rty {
                    // Map array methods to runtime functions
                    match method.as_str() {
                        "push" => ("array_push".to_string(), false, true),
                        "len" => ("array_len".to_string(), true, false),
                        // 批次145 重放: order-preserving dedup / sum on vecs
                        "unique" => ("zeta_vec_unique".to_string(), false, false),
                        // BATCH-296: `df["col"].nunique()` — the column IS a
                        // vec, so the qualified-name fallback reached
                        // `Series::nunique`, which read the vec header as
                        // `self.data` and ran the loop off the end
                        // (measured: SIGBUS at `nunique+72` from a 2-element
                        // str column).
                        "nunique" => ("zeta_vec_nunique".to_string(), false, false),
                        "sum" => ("zeta_sum_vec".to_string(), false, false),
                        // BATCH-291: elementwise date format (see fallback table).
                        "strftime" => ("zeta_vec_strftime".to_string(), false, false),
                        _ => {
                            // For other methods, use qualified name: Type::method
                            let rty_name = rty.display_name();
                            let qualified = format!("{}::{}", rty_name, method);
                            (qualified, false, false)
                        }
                    }
                } else if let Type::Named(tn, _) = rty {
                    // A method on a KNOWN struct must be called by its
                    // QUALIFIED name: the definitions are emitted as
                    // `DataFrame::column`, while the plain name resolved to
                    // an unrelated stub (`@column`) whose result type is i64
                    // — so `a.column("code")[1]` then did a MAP subscript on
                    // a Vec and SEGFAULTED.
                    (
                        self.qualified_method_candidate(tn, method)
                            .unwrap_or_else(|| format!("{}::{}", tn, method)),
                        false,
                        false,
                    )
                } else {
                    // The receiver's type may be UNKNOWN (e.g. the result of
                    // `pd.DataFrame(...)` whose type is not tracked), in which
                    // case the plain name resolved to an unrelated stub whose
                    // result type is i64. Fall back to the UNIQUE qualified
                    // definition `X::method` when there is exactly one — with
                    // more than one candidate we keep the plain name rather
                    // than guessing.
                    let suffix = format!("::{}", method);
                    let mut cands = self
                        .func_ret_types
                        .keys()
                        .filter(|k| k.ends_with(&suffix))
                        .cloned()
                        .collect::<Vec<_>>();
                    cands.sort();
                    cands.dedup();
                    if cands.len() == 1 {
                        (cands.remove(0), false, false)
                    } else if matches!(*rty, Type::F64 | Type::F32)
                        && let Some(entry) =
                            crate::middle::pylib::find_member("math", method)
                        && entry.args.len() == arg_ids.len()
                        && entry.args.iter().all(|t| t == "f64")
                        && !entry.stub
                        && (entry.ret == "f64" || entry.ret == "i64")
                    {
                        // Batch 397: `x.sqrt()` on a float receiver used to
                        // lower to the bare method name, which codegen then
                        // declared as `i64(i64, …)` — the double travelled
                        // in an integer register after an `fptosi`, so the
                        // result was garbage (`9.0.sqrt()` → 9). The
                        // `math` table already holds the right symbol,
                        // argument types and return; route onto it, exactly
                        // as `a ** b` routes to `py_math_pow`.
                        float_math_ret = Some(entry.ret.as_str());
                        (entry.symbol.clone(), false, false)
                    } else if let Some((_h, symbol, ret_handle, ret)) =
                        crate::middle::pylib::unique_method_for_bare_call(
                            method,
                            arg_ids.len(),
                        )
                    {
                        bare_registry = Some(registry_ret_type(ret_handle, ret));
                        (symbol.to_string(), false, false)
                    } else {
                        // For inherent methods, use plain method name.
                        self.note_member_bare(method, receiver.is_some());
                        (method.clone(), false, false)
                    }
                }
            } else {
                self.note_member_bare(method, receiver.is_some());
                (method.clone(), false, false)
            };

            // Special case: ptr.add(offset) / ptr.offset(offset) for raw pointer types
            // Detect by checking if the mangled function name starts with "add_*"
            // OR if the receiver's source type contains "*mut" or "*const"
            // offset is ONLY defined on raw pointers in Rust, so method="offset" is always ptr arith
            let is_ptr_add = func.starts_with("add_*")
                || method == "offset"
                || (method == "add"
                    && receiver.is_some()
                    && arg_ids
                        .first()
                        .map(|&id| {
                            self.source_types
                                .get(&id)
                                .is_some_and(|st| st.contains("*mut") || st.contains("*const"))
                        })
                        .unwrap_or(false));
            if is_ptr_add {
                let ptr_id = arg_ids[0];
                let offset_id = arg_ids[1];

                // Determine element size from the function name (e.g., "add_*mut u64" -> 8)
                // Note: for offset(), the mangled name is usually "offset_i64" (MIR loses pointer type),
                // so we hard-code elem_size=1 for offset (it's always byte-addressable in practice)
                let elem_size = if method == "offset" {
                    1 // offset is always byte-level on u8 pointers
                } else if func.contains("u64")
                    || func.contains("i64")
                    || func.contains("f64")
                    || func.contains("usize")
                {
                    8
                } else if func.contains("u32") || func.contains("i32") || func.contains("f32") {
                    4
                } else if func.contains("u16") || func.contains("i16") {
                    2
                } else if func.contains("u8") || func.contains("i8") || func.contains("bool") {
                    1
                } else {
                    8
                };

                let size_id = self.next_id();
                self.exprs.insert(size_id, MirExpr::IntLit(elem_size));

                let mul_id = self.next_id();
                self.exprs.insert(
                    mul_id,
                    MirExpr::BinaryOp {
                        op: "*".to_string(),
                        left: offset_id,
                        right: size_id,
                    },
                );

                // Store the pointer arithmetic directly as an inline expression.
                // Must NOT use a Var indirection — Var(X) calls load_local(X) in codegen,
                // but intermediate IDs' allocas are never written to, loading garbage.
                self.exprs.insert(
                    id,
                    MirExpr::BinaryOp {
                        op: "+".to_string(),
                        left: ptr_id,
                        right: mul_id,
                    },
                );
                self.type_map.insert(id, Type::I64);
                self.pointee_widths.insert(id, elem_size as u8);
                return id;
            }

            // Convert type arguments from strings to Type objects
            let mut mir_type_args: Vec<Type> =
                type_args.iter().map(|t| Type::from_string(t)).collect();

            // PY: call to a declared generic function with no explicit
            // type args — infer them from the lowered argument types so
            // codegen monomorphizes per concrete instance (id(3.5) → F64
            // instance, not the eager i64 default). The callee's declared
            // return type (Type::Variable) is substituted below so the
            // caller's dest slot matches the instance's concrete return.
            let base_callee = func.as_str();
            let generic_ret_has_var = self
                .func_ret_types
                .get(base_callee)
                .map(|r| matches!(r, Type::Variable(_)))
                .unwrap_or(false);
            if mir_type_args.is_empty() && generic_ret_has_var {
                mir_type_args = arg_ids
                    .iter()
                    .map(|&aid| {
                        self.type_map
                            .get(&aid)
                            .cloned()
                            .unwrap_or(Type::slot_fallback())
                    })
                    .collect();
            }

            // PY-A: closure call — if the callee is a var bound to a
            // lambda/closure value, lower to a direct named call to the
            // synthetic closure function (f(41) → __closure_0(41)).
            let base_func = func.as_str();
            if let Some(closure_fn) = self.closure_vars.get(base_func) {
                let closure_fn = closure_fn.clone();
                self.stmts.push(MirStmt::Call {
                    func: closure_fn.clone(),
                    args: arg_ids.clone(),
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                // A class defined INSIDE a function is hoisted as a closure,
                // so its constructor arrives here — apply the same struct
                // treatment as the ordinary call path: return the struct and
                // refine its field types from the arguments (otherwise a
                // `str` field of such a class prints as a pointer).
                if let Some(TypeDecl::Struct {
                    fields: decl_fields,
                    ..
                }) = self.type_decls.get_mut(base_func)
                {
                    let pnames = self
                        .func_param_names
                        .get(base_func)
                        .cloned()
                        .unwrap_or_default();
                    for (i, aid) in arg_ids.iter().enumerate() {
                        let concrete = match self.type_map.get(aid) {
                            Some(Type::Str) => "str",
                            Some(Type::F64) => "f64",
                            Some(Type::F32) => "f32",
                            Some(Type::Bool) => "bool",
                            _ => continue,
                        };
                        // Batch 608: refine by PARAM NAME (positional
                        // fallback) — see the generic-call site.
                        let target = pnames.get(i).and_then(|pn| {
                            decl_fields.iter().position(|(f, _)| f == pn)
                        }).unwrap_or(i);
                        if let Some((_, dt)) = decl_fields.get_mut(target) {
                            if dt.as_str() == "i64" {
                                *dt = concrete.to_string();
                            }
                        }
                    }
                    self.type_map
                        .insert(id, Type::Named(base_func.to_string(), vec![]));
                    return id;
                }
                let ret_ty = self
                    .closure_ret_tys
                    .get(&closure_fn)
                    .cloned()
                    .unwrap_or(Type::slot_fallback());
                self.type_map.insert(id, ret_ty);
                return id;
            }

            // Append arg count to disambiguate overloaded functions.
            // gen_mirs creates name_N for overloaded declarations;
            // this ensures call sites match the right declaration.
            // PY-A: zeta_* runtime dispatch names stay bare — the codegen
            // method-dispatch (opaque fallback) matches them exactly.
            // Module-qualified names (`<module>__<name>`) are NOT
            // arity-suffixed: the suffix exists to disambiguate overloads,
            // and a module's function is unique — its definition carries no
            // suffix, so a suffixed CALL referenced a symbol that never
            // exists (4 undefined symbols: __get_price_3 / __get_price_7 /
            // __get_trade_days_2 / __OrderCost_6).
            // `::`-qualified names are like module-qualified ones: the
            // definition carries no arity suffix, so a suffixed CALL would
            // reference a symbol that never exists.
            // Whether the `_<argc>` disambiguation suffix was actually
            // appended — the return-type lookup below must NOT strip an
            // underscore that was part of the NAME. `DataFrame::reset_index`
            // was read as base `DataFrame::reset`, missed `func_ret_types`,
            // and typed the result I64 ⇒ `b.columns` became a bogus struct
            // field read (`array_len` of stack garbage = 0) instead of
            // `DataFrame::columns` (batch 99 fixed this in codegen only).
            let suffixed = !(func.starts_with("zeta_")
                || func.contains("__")
                || func.contains("::"))
                && float_math_ret.is_none()
                && bare_registry.is_none();
            let func_name = if suffixed {
                format!("{}_{}", func, arg_ids.len())
            } else {
                func.clone()
            };
            // Pre-compute base name for return-type lookup before moving func_name.
            let base_name = if suffixed { Some(func.clone()) } else { None };
            self.stmts.push(MirStmt::Call {
                func: func_name,
                args: arg_ids.clone(),
                dest: id,
                type_args: mir_type_args.clone(),
            });
            self.exprs.insert(id, MirExpr::Var(id));
            // For array methods, set appropriate return type
            if is_array_len {
                self.type_map.insert(id, Type::I64);
            } else if is_array_push {
                // push returns void
                self.type_map.insert(id, Type::Tuple(vec![]));
            } else {
                // Look up the callee's known return type (name may carry an
                // "_argc" disambiguation suffix added above).
                let base = base_name.as_deref().unwrap_or(func.as_str());
                let ret_ty = match bare_registry {
                    Some(t) => t,
                    None => match float_math_ret {
                        Some("f64") => Type::F64,
                        Some(_) => Type::I64,
                        None => self
                            .func_ret_types
                            .get(base)
                            .cloned()
                            .unwrap_or(Type::slot_fallback()),
                    },
                };
                // PY: generic callee — substitute concrete type args into
                // the declared return type so the dest slot matches the
                // monomorphized instance (fn f[T](..) -> T with T=f64 must
                // produce an f64-typed dest, not the i64 default).
                let ret_ty = if mir_type_args.is_empty() {
                    ret_ty
                } else {
                    let mut sub =
                        crate::middle::types::Substitution::new();
                    for (i, ta) in mir_type_args.iter().enumerate() {
                        sub.mapping.insert(
                            crate::middle::types::TypeVar(i as u32),
                            ta.clone(),
                        );
                    }
                    sub.apply(&ret_ty)
                };
                // A constructor of a struct declared in this program returns
                // that struct, and its ARGUMENTS tell us the field types.
                // `def __init__(self, s)` is unannotated, so the class
                // desugaring recorded every field as i64: the runtime value
                // was a correct string handle (`A("hi").name == "hi"` was
                // True) but the field READ was typed i64 and `print` showed
                // the pointer. Refine here, in the PARENT context — a child
                // MirGen gets a clone of `type_decls`, so a refinement made
                // while lowering the constructor body never comes back.
                let ret_ty = if let Some(TypeDecl::Struct {
                    fields: decl_fields,
                    ..
                }) = self.type_decls.get_mut(base)
                {
                    let pnames = self
                        .func_param_names
                        .get(base)
                        .cloned()
                        .unwrap_or_default();
                    for (i, aid) in arg_ids.iter().enumerate() {
                        let concrete: Option<String> = match self.type_map.get(aid) {
                            Some(Type::Str) => Some("str".to_string()),
                            Some(Type::F64) => Some("f64".to_string()),
                            Some(Type::F32) => Some("f32".to_string()),
                            Some(Type::Bool) => Some("bool".to_string()),
                            // A map value CARRIES its key type and the field must keep it:
                            // without it a content-hashing map probed with an unannotated
                            // parameter key misses (value 0, no diagnostic). Note
                            // `from_string` parses generics as `lt(Name, Args)`, not `<...>`.
                            Some(Type::Named(m, targs)) if m == "map" => {
                                let k = match targs.first() {
                                    Some(Type::Str) => "str",
                                    _ => "i64",
                                };
                                Some(format!("lt(map, {})", k))
                            }
                            _ => None,
                        };
                        if let Some(concrete) = concrete {
                            // Batch 608: refine by PARAM NAME (field ==
                            // param by the `self.x = x` convention) with a
                            // positional FALLBACK — the positional form
                            // refined `legs` for `Dog("Rex")` (args map to
                            // PARAMS, not field order).
                            let target = pnames.get(i).and_then(|pn| {
                                decl_fields.iter().position(|(f, _)| f == pn)
                            }).unwrap_or(i);
                            if let Some((_, dt)) = decl_fields.get_mut(target) {
                                if dt.as_str() == "i64" || dt.as_str() == "map" {
                                    *dt = concrete;
                                }
                            }
                        }
                    }
                    Type::Named(base.to_string(), vec![])
                } else {
                    ret_ty
                };
                self.type_map.insert(id, ret_ty);
            }
        id
    }
}

/// min()/max() 单参数数组形式的降级方案纯面（批 925）：元素是否浮点 ⇒
///（函数名, 结果槽是否 f64）。f64 版按位模式读、double 域比较、位模式返回；
/// 结果槽标 F64 让下游按浮点消费。此前只有 i64 版，浮点数组按位模式比大小，
/// gen 侧仅 warning 提示绕行。
fn minmax_builtin_target(method: &str, elem_is_float: bool) -> (&'static str, bool) {
    match (method, elem_is_float) {
        ("max", true) => ("py_builtin_max_f64", true),
        ("min", true) => ("py_builtin_min_f64", true),
        ("max", false) => ("py_builtin_max", false),
        ("min", false) => ("py_builtin_min", false),
        _ => ("py_builtin_min", false),
    }
}

#[cfg(test)]
mod tests {
    use super::minmax_builtin_target;

    #[test]
    fn minmax_float_arrays_target_f64_variants() {
        assert_eq!(minmax_builtin_target("max", true), ("py_builtin_max_f64", true));
        assert_eq!(minmax_builtin_target("min", true), ("py_builtin_min_f64", true));
    }

    #[test]
    fn minmax_int_arrays_keep_legacy() {
        assert_eq!(minmax_builtin_target("max", false), ("py_builtin_max", false));
        assert_eq!(minmax_builtin_target("min", false), ("py_builtin_min", false));
    }
}
