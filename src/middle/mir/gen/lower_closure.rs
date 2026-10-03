//! 批次 877：闭包降型发射体（lower_closure 自 gen.rs 原样迁入；
//! 命名空间生成、自由变量收集与捕获、env 绑定）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_closure(&mut self, params: &[String], body: &AstNode) -> String {
        // T0 (refactor.md B.5): the symbol is derived from the ENCLOSING
        // scope's declared name plus this closure's ordinal in source order.
        // It used to come from a process-global `AtomicU32`, so a closure's name
        // depended on the order the compiler happened to visit functions — and
        // that order is HashMap-random (measured on jq_wufu.py: `__closure_0`
        // was the `code` lambda in one compile and the `codes` lambda in the
        // next). Cross-scope uniqueness now rests on function-name uniqueness,
        // the same assumption `final_mirs` (keyed by name) already makes; the
        // guard below makes a collision loud instead of silent.
        let n = self.closure_seq;
        self.closure_seq += 1;
        // The readable part is the parent's BARE name; the hash keeps parents
        // that share a bare name (same class/method in two modules) apart. Two
        // shape constraints come from codegen's symbol waterfall:
        //  - no interior `__` and no leading-`_` ambiguity: a name that looks
        //    module-mangled is resolved as `module__fn` and its definition was
        //    dropped (measured: `___closure__backend_datasrc_split_factors__
        //    load_split_factors__0` undefined at link on the local wufu driver);
        //  - the LAST segment must not be all digits, or the waterfall's arity
        //    stripping (codegen.rs:2675/2689/2708/2731) renames the reference
        //    but not the definition. Hence ordinal first, `_c<hash>` last.
        let bare0 = self.closure_ns.rsplit("::").next().unwrap_or("");
        let bare0 = bare0.rsplit_once("__").map(|(_, t)| t).unwrap_or(bare0);
        let mut bare = String::new();
        for c in bare0.chars() {
            if c == '_' {
                if !bare.ends_with('_') {
                    bare.push('_');
                }
            } else if c.is_ascii_alphanumeric() {
                bare.push(c);
            } else {
                bare.push('_');
            }
        }
        let bare = if bare.is_empty() { "anon".to_string() } else { bare };
        let mut ns_hash: u64 = 0xcbf29ce484222325;
        for b in self.closure_ns.as_bytes() {
            ns_hash ^= *b as u64;
            ns_hash = ns_hash.wrapping_mul(0x100000001b3);
        }
        let closure_name = format!(
            "__closure_{}_{}_c{:08x}",
            n,
            bare,
            (ns_hash & 0xffff_ffff) as u32
        );
        {
            thread_local! {
                static MINTED: std::cell::RefCell<std::collections::HashMap<String, String>> =
                    std::cell::RefCell::new(std::collections::HashMap::new());
            }
            MINTED.with(|m| {
                let mut m = m.borrow_mut();
                match m.get(&closure_name) {
                    Some(prev) if *prev != self.closure_ns => eprintln!(
                        "warning: [W2001] closure symbol {} minted by two scopes ({} and {})",
                        closure_name, prev, self.closure_ns
                    ),
                    Some(_) => {}
                    None => {
                        m.insert(closure_name.clone(), self.closure_ns.clone());
                    }
                }
            });
        }

        // PY-A V2: free variables of the body (against params + currently
        // bound names) are captured THROUGH the env runtime — each read is
        // zeta_env_get("name"), each assignment zeta_env_set("name", v).
        // NOTE: only params count as bound — enclosing locals are NOT visible
        // inside the synthetic function, so any other referenced name is a
        // free variable that must go through the env runtime.
        let mut bound: std::collections::HashSet<String> = params.iter().cloned().collect();
        // BATCH-438: a hoisted nested `class` method spells its receiver
        // `&mut self`, while the body writes `self`. Without the alias the name
        // was collected as a FREE variable and read through
        // `zeta_env_get("self")` — the enclosing object, not the method's own.
        if params.iter().any(|p| Self::is_receiver_param(p)) {
            bound.insert("self".to_string());
        }
        let mut free: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        Self::collect_free_vars(body, &mut bound, &mut free);
        if crate::diagnostics::env_flag("ZETA_PROBE") {
            eprintln!("PROBE child nonlocal={:?} free={:?}", self.nonlocal_names, free);
        }

        // Fresh sub-MIR with its own id space (params start at id 1).
        // Inherits nonlocal_names so inner assignments route through env.
        let mut child = MirGen::new()
            .with_nonlocal_names(self.nonlocal_names.clone())
            .with_module_globals(self.module_globals.clone())
            // The child context needs the struct/enum declarations the parent
            // already registered. A class defined inside a function is lowered
            // through this path (hoisted ctor), and without the declaration its
            // `StructLit` body lowered to NOTHING — the constructor returned 0
            // and every field read was garbage.
            .with_type_decls(self.type_decls.clone())
            .with_func_ret_types(self.func_ret_types.clone())
            // A closure inside an imported module must resolve that module's
            // own functions too (`lambda m: lowercase(m.group(0))`).
            .with_symbol_renames(self.symbol_renames.clone())
            // The import tables, module-global types, defaults and CLI kinds
            // are PROGRAM-wide context: without them a closure body could not
            // see them at all — `[pd.DataFrame(r) for r in rs]` emitted a bare
            // `_DataFrame` (link failure, t216) because `pd` only existed in
            // the parent's alias table.
            .with_py_imports(
                self.py_module_aliases.clone(),
                self.py_member_aliases.clone(),
            )
            .with_py_user_modules(self.py_user_modules.clone())
            .with_py_module_paths(self.py_module_paths.clone())
            .with_module_global_types(self.module_global_types.clone())
            .with_func_param_names(self.func_param_names.clone())
            .with_func_star_params(self.func_star_params.clone())
            .with_argparse_kinds(self.argparse_kinds.clone())
            .with_param_defaults(self.param_defaults.clone())
            .with_source_file(self.source_file.clone())
            .with_global_consts(self.global_consts.clone());
        // Lambda values bound in the ENCLOSING scope (`f = lambda x: …;` then a
        // comprehension calling `f(x)`) plus the class of an enclosing method
        // are lowering context the child cannot rebuild — without them the call
        // site emitted a ghost `_f` symbol.
        child.shared_type_decls = self.shared_type_decls.clone();
        child.closure_vars = self.closure_vars.clone();
        child.closure_ret_tys = self.closure_ret_tys.clone();
        child.hoisted_names = self.hoisted_names.clone();
        child.current_class = self.current_class.clone();
        child.current_module = self.current_module.clone();
        child.closure_ns = closure_name.clone();
        child.closure_seq = 0;
        child.re_repl_param = self.re_repl_param;
        child.fn_depth = self.fn_depth + 1;
        let param_hint = self.pending_closure_param_types.take();
        for (pi, p) in params.iter().enumerate() {
            let id = child.next_id();
            child.name_to_id.insert(p.clone(), id);
            let is_receiver = Self::is_receiver_param(p);
            if is_receiver {
                child.name_to_id.insert("self".to_string(), id);
            }
            child.exprs.insert(id, MirExpr::Var(id));
            // A `re.sub` replacement closure receives a Match handle.
            let hinted = param_hint.as_ref().and_then(|h| h.get(pi)).cloned();
            let mut ty = match hinted {
                Some(t) => t,
                None => {
                    if self.re_repl_param {
                        Type::Named("PyMatch".to_string(), vec![])
                    } else {
                        Type::I64
                    }
                }
            };
            // BATCH-438: same rule the ordinary item path applies at :1393 — the
            // receiver of a (nested) method is the class its impl block named.
            if is_receiver
                && matches!(ty, Type::I64 | Type::PyDynamic)
                && let Some(cls) = self.current_class.clone()
            {
                ty = Type::Named(cls, vec![]);
            }
            child.type_map.insert(id, ty);
        }

        child.stmts = params
            .iter()
            .enumerate()
            .map(|(i, _)| MirStmt::ParamInit {
                param_id: i as u32 + 1,
                arg_index: i as u32,
            })
            .collect();
        // Free vars: pre-bind each name to an env-load id (must come AFTER
        // the ParamInit seed above — that assignment replaces child.stmts).
        for name in &free {
            let key = self.capture_env_key(name);
            let name_id = child.next_id();
            child
                .exprs
                .insert(name_id, MirExpr::StringLit(key.clone()));
            child.type_map.insert(name_id, Type::Str);
            let slot_id = child.next_id();
            child.stmts.push(MirStmt::Call {
                func: "zeta_env_get".to_string(),
                args: vec![name_id],
                dest: slot_id,
                type_args: vec![],
            });
            child.exprs.insert(slot_id, MirExpr::Var(slot_id));
            // Give the captured name its REAL type: typing every capture as i64
            // made `if f in d` inside a comprehension test a string key against
            // a dict whose handle was treated as an integer — every item was
            // filtered out and the result silently came back empty.
            let cap_ty = self
                .name_to_id
                .get(name)
                .and_then(|pid| self.type_map.get(pid))
                .cloned()
                .or_else(|| self.global_ty_of(&key))
                .unwrap_or(Type::I64);
            child.type_map.insert(slot_id, cap_ty);
            child.name_to_id.insert(name.clone(), slot_id);
            child.captured_vars.insert(name.clone(), name_id);
            // Batch 662: a captured name bound to a LAMBDA keeps its
            // callability inside the closure — the comprehension filter
            // `if condition(m)` (condition = a loop-unpacked lambda from
            // the enclosing scope) otherwise falls to the `_condition`
            // ghost stub at run time (t233's registered red). The
            // ret-type entry rides along so the direct named call types
            // its result.
            if let Some(cfn) = self.closure_vars.get(name) {
                child.closure_vars.insert(name.clone(), cfn.clone());
                if let Some(rt) = self.closure_ret_tys.get(cfn) {
                    child.closure_ret_tys.insert(cfn.clone(), rt.clone());
                }
            }
        }
        if crate::diagnostics::env_flag("ZETA_PROBE") {
            eprintln!("PROBE closure {} body stmts={}", closure_name,
                match body { AstNode::Block { body } => body.len(), _ => 1 });
        }
        // A hoisted FUNCTION body (the constructor synthesized for a class
        // defined inside a function) is a STATEMENT LIST, not an expression.
        // `lower_expr` on a Block dropped every statement, so the constructor
        // came out empty (`ret 0`) and every field read was garbage — while a
        // lambda/comprehension body really is an expression and keeps the old
        // path.
        let body_val = match body {
            AstNode::Block { body: stmts } => {
                for st in stmts {
                    child.lower_ast(st);
                }
                let z = child.next_id();
                child.exprs.insert(z, MirExpr::IntLit(0));
                child.type_map.insert(z, Type::I64);
                z
            }
            // Batch 800: a lambda/comprehension body whose single expression
            // is statement-wrapped arrives as `ExprStmt{expr}`. `lower_expr`
            // has no ExprStmt arm and fell to the `IntLit(0)` fallback with
            // zero emitted statements — the body silently vanished
            // (`|a| double_it(a)` returned 0). Unwrap first, same as the
            // Block-last-expression arm at :4794.
            AstNode::ExprStmt { expr } => child.lower_expr(expr),
            _ => child.lower_expr(body),
        };
        // Remember the body's value type for the enclosing comprehension (the
        // child MirGen owns that type map, so read it here).
        // A STATEMENT-LIST body has no body value at all — `body_val` there is
        // the dummy 0 literal above, so reading its type recorded I64 for a
        // hoisted nested `def` that returns a string. Take the type off the
        // body's own `Return` instead; no top-level `Return` keeps the old
        // (none) behaviour rather than inventing one.
        self.last_closure_ret_ty = match body {
            AstNode::Block { .. } => child
                .stmts
                .iter()
                .find_map(|s| match s {
                    MirStmt::Return { val } => child.type_map.get(val).cloned(),
                    _ => None,
                }),
            _ => child.type_map.get(&body_val).cloned(),
        };
        // Batch 299: a DICT comprehension's lambda records its key/value types
        // on the child (`__pack_pair__`), so the collected map can be typed
        // `map[k, v]` instead of the untyped `map` that made `.values()`
        // elements i64.
        if child.last_dict_pair_ty.is_some() {
            self.last_dict_pair_ty = child.last_dict_pair_ty.clone();
        }
        // Ensure the closure returns its body value.
        if crate::diagnostics::env_flag("ZETA_PROBE") {
            eprintln!("PROBE closure {} final stmts={}", closure_name, child.stmts.len());
        }
        if !child
            .stmts
            .iter()
            .any(|s| matches!(s, MirStmt::Return { .. }))
        {
            child.stmts.push(MirStmt::Return { val: body_val });
        }

        let mut mir = child.build_mir(params);
        mir.name = Some(closure_name.clone());
        mir.is_extern = false;
        mir.param_indices = params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i as u32 + 1))
            .collect();
        self.generated_mirs.push(mir);
        // Closures nested INSIDE this closure were lowered by the child and
        // published into the child's own list; `build_mir` does not move it, so
        // drain it here. Without this the nested `FuncAddr` reference survives
        // while its definition is dropped (undefined symbol at link).
        for g in std::mem::take(&mut child.generated_mirs) {
            self.generated_mirs.push(g);
        }
        closure_name
    }
}
