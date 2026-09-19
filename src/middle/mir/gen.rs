// src/middle/mir/gen.rs
//! # MIR Generation from AST
//!
//! Lowers Zeta AST to our clean Minimal Intermediate Representation (MIR).
//! All Zeta features (methods, generics, control flow, dicts, etc.) are lowered here.
//! Clean, fast, and fully documented.

use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{Mir, MirExpr, MirStmt, SemiringOp};
use crate::middle::specialization::MonoKey;
use crate::middle::types::{ArraySize, Type};
use std::collections::HashMap;
/// PY-A: `os.environ.get('NAME'[, default])` / `os.getenv('NAME')` with a
/// LITERAL name — readable at compile time. Python returns None for a
/// missing key, so a missing variable is a *known* answer, not an unknown
/// one: `os.environ.get('X') == '1'` is False, `!= '1'` is True.
pub(crate) fn eval_env_read(call: &AstNode) -> Option<String> {
    let AstNode::Call {
        receiver,
        method,
        args,
        ..
    } = call
    else {
        return None;
    };
    let is_environ_get = method == "get"
        && matches!(
            receiver.as_deref(),
            Some(AstNode::FieldAccess { base, field })
                if field == "environ"
                    && matches!(&**base, AstNode::Var(v) if v == "os")
        );
    let is_getenv = method == "getenv"
        && matches!(receiver.as_deref(), Some(AstNode::Var(v)) if v == "os");
    if !(is_environ_get || is_getenv) {
        return None;
    }
    let name = match args.first() {
        Some(AstNode::StringLit(s)) => s,
        _ => return None,
    };
    if let Ok(v) = std::env::var(name) {
        return Some(v);
    }
    match args.get(1) {
        Some(AstNode::StringLit(d)) => Some(d.clone()),
        Some(AstNode::Lit(n)) => Some(n.to_string()),
        // Python: None — never equal to any literal we compare against.
        _ => Some("\u{0}<unset>".to_string()),
    }
}

/// PY-A: fold `if <env read> == 'lit':` / `!=` into a compile-time decision
/// so only the taken branch is lowered. This is what lets the LOCAL
/// implementation build without the platform: the strategy's
/// `if os.environ.get('REPLAYQUANT_LOCAL') == '1': from <shim> import …`
/// / `else: from jqdata import *` otherwise lowers BOTH branches, dragging
/// in the JoinQuant host symbols the local run never uses.
pub(crate) fn fold_env_condition(cond: &AstNode) -> Option<bool> {
    let (op, left, right) = match cond {
        AstNode::BinaryOp { op, left, right } if op == "==" || op == "!=" => (op, left, right),
        _ => return None,
    };
    let (call, lit) = match (&**left, &**right) {
        (AstNode::Call { .. }, other) => (&**left, other),
        (other, AstNode::Call { .. }) => (&**right, other),
        _ => return None,
    };
    let value = eval_env_read(call)?;
    let lit_str = match lit {
        AstNode::StringLit(s) => s.clone(),
        AstNode::Lit(n) => n.to_string(),
        _ => return None,
    };
    let eq = value == lit_str;
    Some(if op == "==" { eq } else { !eq })
}



/// Type declaration metadata registered during MIR lowering.
#[derive(Debug, Clone)]
pub enum TypeDecl {
    /// A struct type with named fields: (name, type_string).
    Struct {
        fields: Vec<(String, String)>,
        generics: Vec<crate::frontend::ast::GenericParam>,
    },
    /// An enum type with variants: (variant_name, field_types).
    Enum {
        variants: Vec<(String, Vec<String>)>,
        generics: Vec<crate::frontend::ast::GenericParam>,
    },
    /// A type alias.
    Alias { target: String },
}

pub struct MirGen {
    next_id: u32,
    stmts: Vec<MirStmt>,
    exprs: HashMap<u32, MirExpr>,
    ctfe_consts: HashMap<u32, i64>, // TODO: Change to ConstValue
    type_map: HashMap<u32, Type>,
    name_to_id: HashMap<String, u32>,
    global_consts: HashMap<String, crate::middle::ctfe::value::ConstValue>,
    // Preserve original source-level type strings for parameters (e.g., "*mut u64")
    source_types: HashMap<u32, String>,
    /// Tracks pointee element width (in bytes) for pointer-typed expression IDs.
    /// Populated by the offset/add handler, used when generating Store/Deref.
    pointee_widths: HashMap<u32, u8>,
    /// Type declarations seen during lowering (structs, enums, aliases).
    type_decls: HashMap<String, TypeDecl>,
    /// Type declarations collected by the Resolver across the whole program
    /// (enums/aliases live in their own AST items, but each function gets a
    /// fresh MirGen — these are re-seeded into `type_decls` per lowering).
    shared_type_decls: HashMap<String, TypeDecl>,
    /// PY-A: the source file being compiled — the value of `__file__`.
    source_file: Option<String>,
    /// PY-A: argparse flag → value kind (program-wide, from the Resolver).
    argparse_kinds: HashMap<String, String>,
    /// PY-A: Python default argument values per function (see the Resolver).
    param_defaults: HashMap<String, Vec<Option<AstNode>>>,
    /// PY-A: element type to give a comprehension lambda's parameter. Set just
    /// before the lambda is lowered (the iterable's element type is known by
    /// then) and consumed by `lower_closure`; without it the loop variable was
    /// i64 even for a list of strings, so `[s.upper() for s in strs]` and
    /// `{f: g[f] for f in fs}` silently operated on pointers.
    pending_closure_param_types: Option<Vec<Type>>,
    /// PY-A: value type of the most recently lowered closure body, so a
    /// comprehension's result can carry a real element type instead of i64.
    last_closure_ret_ty: Option<Type>,
    /// Stack of loop result slots for `loop { break EXPR; }` value semantics.
    loop_value_stack: Vec<u32>,
    /// Result slot of the most recently lowered loop (for implicit ret_val).
    last_loop_result: Option<u32>,
    /// Additional MIRs generated during lowering (e.g., async poll functions).
    generated_mirs: Vec<Mir>,
    /// Lowering depth: 0 at top level, >0 inside a function body — used to
    /// skip nested defs (their inline Return would corrupt the enclosing stream).
    fn_depth: u32,
    /// Nested defs hoisted to standalone functions (user name → closure fn).
    hoisted_names: std::collections::HashMap<String, String>,
    /// PY-A V3: program-wide `nonlocal` names — reads/writes route through
    /// the closure env in any scope (defining and inner).
    nonlocal_names: std::collections::HashSet<String>,
    /// PY-A: module-top-level bare-assigned names — reads fall back to env
    /// when not bound locally; defining assignments store through env.
    module_globals: std::collections::HashSet<String>,
    /// PY-A: Python-library imports — alias → canonical module name.
    py_module_aliases: HashMap<String, String>,
    /// PY-A: `from X import y as b` — b → (module, member).
    py_member_aliases: HashMap<String, (String, String)>,
    /// PY-A: Python modules loaded from disk (mangled `mod__name` symbols).
    py_user_modules: std::collections::HashSet<String>,
    /// PY-A: bare module-internal name → mangled symbol, for the function
    /// currently being lowered.
    symbol_renames: HashMap<String, String>,
    /// PY-A: static type of each module-level global, so env reads keep the
    /// handle tag (`q = queue.Queue()` then `q.put(x)` inside a function).
    module_global_types: HashMap<String, Type>,
    /// PY-A: set while lowering the replacement closure of `re.sub`, so its
    /// parameter is typed as a Match (`m.group(0)` must dispatch).
    re_repl_param: bool,
    /// PY-A: class name when lowering a class method (`DataFrame::columns`),
    /// so `self` binds as Named(class) and `self.<field>` keeps the field's
    /// declared type (map membership in `__contains__` dispatches on it).
    current_class: Option<String>,
    /// Names captured from enclosing scopes in the closure currently being
    /// lowered (name → env key id) — used to route assignments to env stores.
    captured_vars: std::collections::HashMap<String, u32>,
    /// Async state machine: state pointer expression ID.
    async_state_ptr: Option<u32>,
    /// Async state machine: current segment index for dispatch.
    async_segment_count: u32,
    /// Whether we are lowering an async function body.
    is_async_fn: bool,
    /// Snapshot of name_to_id at current await point for variable save/restore.
    async_saved_vars: Vec<(String, u32)>,
    /// Known function return types (base name -> Type), injected by Resolver.
    func_ret_types: HashMap<String, Type>,
    /// Parameter names per function, for keyword-argument binding.
    func_param_names: HashMap<String, Vec<String>>,
    /// PY-A: monotonic counter for synthetic closure function names.
    closure_counter: u32,
    /// PY-A: variables bound to a lambda/closure value, mapped to the
    /// synthetic closure function name. Lets call sites (`f(41)` where `f =
    /// lambda x: x+1`) lower to a direct named call to the closure function.
    closure_vars: HashMap<String, String>,
    /// PY-A: value type of each synthetic closure's body, so a call through a
    /// closure variable (`f = lambda s: s.upper(); f(x)`) yields a string
    /// rather than an i64-boxed pointer.
    closure_ret_tys: HashMap<String, Type>,
    /// The var name being bound when a `let f = lambda...` RHS is lowered;
    /// the Closure lowering reads it to record the closure_vars entry.
    pending_closure_binding: Option<String>,
}

impl MirGen {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            stmts: vec![],
            exprs: HashMap::new(),
            ctfe_consts: HashMap::new(),
            type_map: HashMap::new(),
            name_to_id: HashMap::new(),
            global_consts: HashMap::new(),
            source_types: HashMap::new(),
            pointee_widths: HashMap::new(),
            type_decls: HashMap::new(),
            shared_type_decls: HashMap::new(),
            source_file: None,
            argparse_kinds: HashMap::new(),
            param_defaults: HashMap::new(),
            pending_closure_param_types: None,
            last_closure_ret_ty: None,
            loop_value_stack: Vec::new(),
            last_loop_result: None,
            generated_mirs: vec![],
            fn_depth: 0,
            hoisted_names: std::collections::HashMap::new(),
            nonlocal_names: std::collections::HashSet::new(),
            module_globals: std::collections::HashSet::new(),
            py_module_aliases: HashMap::new(),
            py_member_aliases: HashMap::new(),
            py_user_modules: std::collections::HashSet::new(),
            symbol_renames: HashMap::new(),
            module_global_types: HashMap::new(),
            re_repl_param: false,
            current_class: None,
            captured_vars: std::collections::HashMap::new(),
            async_state_ptr: None,
            async_segment_count: 0,
            is_async_fn: false,
            async_saved_vars: vec![],
            func_ret_types: HashMap::new(),
            func_param_names: HashMap::new(),
            closure_counter: 0,
            closure_vars: HashMap::new(),
            closure_ret_tys: HashMap::new(),
            pending_closure_binding: None,
        }
    }

    pub fn with_global_consts(
        mut self,
        consts: HashMap<String, crate::middle::ctfe::value::ConstValue>,
    ) -> Self {
        self.global_consts = consts;
        self
    }

    /// PY-A: module-global name → static type (see the field docs).
    pub fn with_module_global_types(mut self, types: HashMap<String, Type>) -> Self {
        self.module_global_types = types;
        self
    }

    /// PY-A: parameter names, so `f(a=1)` binds by name.
    pub fn with_func_param_names(mut self, names: HashMap<String, Vec<String>>) -> Self {
        self.func_param_names = names;
        self
    }

    pub fn with_func_ret_types(
        mut self,
        ret_types: HashMap<String, Type>,
    ) -> Self {
        self.func_ret_types = ret_types;
        self
    }

    fn i64_zero_id(&mut self) -> u32 {
        let z = self.next_id();
        self.exprs.insert(z, MirExpr::IntLit(0));
        self.type_map.insert(z, Type::I64);
        z
    }

    /// PY-A V3: names declared `nonlocal` (reads/writes route through env).
    pub fn with_nonlocal_names(mut self, names: std::collections::HashSet<String>) -> Self {
        if std::env::var("ZETA_PROBE").is_ok() {
            eprintln!("PROBE with_nonlocal_names: {:?}", names);
        }
        self.nonlocal_names = names;
        self
    }

    /// PY-A: module-top-level bare-assigned names (implicit module globals).
    pub fn with_module_globals(mut self, names: std::collections::HashSet<String>) -> Self {
        self.module_globals = names;
        self
    }

    /// PY-A: Python-library import tables collected by the Resolver.
    pub fn with_py_imports(
        mut self,
        modules: HashMap<String, String>,
        members: HashMap<String, (String, String)>,
    ) -> Self {
        self.py_module_aliases = modules;
        self.py_member_aliases = members;
        self
    }

    /// PY-A: modules loaded from disk register under a `mod__name` prefix.
    pub fn with_py_user_modules(mut self, mods: std::collections::HashSet<String>) -> Self {
        self.py_user_modules = mods;
        self
    }

    /// PY-A: bare module-internal name → mangled symbol for this function.
    pub fn with_symbol_renames(mut self, renames: HashMap<String, String>) -> Self {
        self.symbol_renames = renames;
        self
    }

    /// PY-A: resolve a Python-library call to (runtime symbol, handle tag).
    /// Handles both `Thread(...)` (bound member) and `threading.Thread(...)`
    /// (module-qualified).
    /// Flatten `os` / `os.path` / `a.b.c` into (root, [parts…]).
    fn flatten_module_receiver(n: &AstNode) -> Option<(String, Vec<String>)> {
        match n {
            AstNode::Var(v) => Some((v.clone(), Vec::new())),
            AstNode::FieldAccess { base, field } => {
                let (root, mut parts) = Self::flatten_module_receiver(base)?;
                parts.push(field.clone());
                Some((root, parts))
            }
            _ => None,
        }
    }

    /// PY-A: canonical (module, member) for a library call, before symbol
    /// mapping — lets special cases (json.dumps' typed dispatch) recognise
    /// the call site.
    fn py_member_target(
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
        Some((module, member))
    }

    fn py_member_call(
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
        // Not a registry shim: a module loaded from disk resolves to its
        // `mod__name` mangled symbol (no handle tag, i64 result).
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
    fn py_struct_type_of(&self, recv: &AstNode) -> Option<String> {
        let name = match recv {
            AstNode::Var(n) => n,
            _ => return None,
        };
        if let Some(id) = self.name_to_id.get(name) {
            if let Some(Type::Named(n, _)) = self.type_map.get(id) {
                return Some(n.clone());
            }
        }
        if let Some(Type::Named(n, _)) = self.module_global_types.get(name) {
            return Some(n.clone());
        }
        None
    }

    /// Field lookup that also accepts a module-mangled type name
    /// (`jq_shim__G` -> `G`), since `type_decls` keys come from the plain
    /// class/struct declaration.
    fn py_struct_has_field(&self, tyname: &str, field: &str) -> bool {
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

    /// 批次147: qualified-method name with module-mangle candidates.
    /// `self` in a library's own method is typed Named("pandas__DataFrame")
    /// (the mangled struct), while func_ret_types keys the method as
    /// "DataFrame::column_names" (unmangled) — try both spellings.
    fn qualified_method_candidate(&self, tn: &str, method: &str) -> Option<String> {
        let direct = format!("{}::{}", tn, method);
        if self.func_ret_types.contains_key(&direct) {
            return Some(direct);
        }
        if let Some((_, tail)) = tn.rsplit_once("__") {
            let cand = format!("{}::{}", tail, method);
            if self.func_ret_types.contains_key(&cand) {
                return Some(cand);
            }
        }
        None
    }

    fn py_handle_of(&self, recv: &AstNode) -> Option<String> {
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

    /// PY-A: for `Thread(target, args=(a, b, ...))`, return the target name
    /// and the literal args-tuple elements, so a multi-argument thread can be
    /// adapted. Only a literal tuple target/args is recognized; anything else
    /// returns None and falls through to the direct shim call.
    fn thread_args_tuple(args: &[AstNode]) -> Option<(String, Vec<AstNode>)> {
        let mut target: Option<String> = None;
        let mut tuple: Option<Vec<AstNode>> = None;
        let mut positional: Vec<AstNode> = Vec::new();
        for a in args {
            if let AstNode::Call {
                receiver: None,
                method,
                args: ka,
                ..
            } = a
            {
                if method == "__kwarg__" && ka.len() == 2 {
                    if let AstNode::StringLit(n) = &ka[0] {
                        match n.as_str() {
                            "target" => {
                                if let AstNode::Var(v) = &ka[1] {
                                    target = Some(v.clone());
                                }
                            }
                            "args" => {
                                if let AstNode::Tuple(els) = &ka[1] {
                                    tuple = Some(els.clone());
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
                }
            }
            positional.push(a.clone());
        }
        if target.is_none() {
            if let Some(AstNode::Var(v)) = positional.first() {
                target = Some(v.clone());
            }
        }
        match (target, tuple) {
            (Some(t), Some(els)) => Some((t, els)),
            _ => None,
        }
    }

/// PY-A: a call that binds fewer arguments than the callee declares and has no
/// default for the rest is a Python `TypeError`. We cannot fail the build here
/// (platform shims legitimately differ), but staying silent would repeat the
/// exact failure mode this work removes — a wrong value with no diagnostic —
/// so name the unbound parameters.
fn warn_unbound(callee: &str, params: &[String], slots: &[Option<AstNode>]) {
    let missing: Vec<&str> = params
        .iter()
        .enumerate()
        .filter(|(i, _)| slots.get(*i).map_or(true, |s| s.is_none()))
        .map(|(_, n)| n.as_str())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "warning: PY-A: `{}` is called without argument(s) for [{}] and it declares no \
             default — those parameters read 0 (Python would raise TypeError)",
            callee,
            missing.join(", ")
        );
    }
}

    /// Pre-seed type declarations collected program-wide by the Resolver.
    pub fn with_type_decls(mut self, decls: HashMap<String, TypeDecl>) -> Self {
        self.shared_type_decls = decls;
        self
    }

    /// PY-A: the file being compiled, so `__file__` can resolve to it.
    pub fn with_source_file(mut self, path: Option<String>) -> Self {
        self.source_file = path;
        self
    }

    /// PY-A: argparse flag kinds collected program-wide by the Resolver.
    pub fn with_argparse_kinds(mut self, kinds: HashMap<String, String>) -> Self {
        self.argparse_kinds = kinds;
        self
    }

    /// PY-A: default argument values, so omitted call arguments can be filled.
    pub fn with_param_defaults(
        mut self,
        defaults: HashMap<String, Vec<Option<AstNode>>>,
    ) -> Self {
        self.param_defaults = defaults;
        self
    }

    pub fn lower_to_mir(&mut self, ast: &AstNode) -> Mir {
        self.name_to_id.clear();
        self.stmts.clear();
        self.exprs.clear();
        self.source_types.clear();
        self.pointee_widths.clear();
        self.type_decls.clear();
        self.type_decls
            .extend(self.shared_type_decls.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.next_id = 1;

        // Check if this is an extern/FFI function declaration.
        // Only AstNode::ExternFunc is truly extern. FuncDef with empty
        // bodies are user-defined stub functions, not extern — they get
        // stub body emission in codegen.
        // Builtins like malloc/free are handled via a name lookup in codegen.
        let is_extern = matches!(ast, AstNode::ExternFunc { .. });

        if !is_extern {
            if let AstNode::FuncDef { name: fname, params, generics, .. } = ast {
                // PY-A: pylib class methods are registered under their
                // qualified name (`DataFrame::columns`). Record the class so
                // `self` binds as Named(class) — without it `self.data` loses
                // the field's declared map type and `key in self.data` inside
                // `__contains__` cannot dispatch.
                self.current_class = if fname.contains("::") {
                    // 剥模块 mangle 前缀（pandas__DataFrame → DataFrame），
                    // type_decls/func_ret_types 以裸类名为键
                    let cc = fname.split("::").next().unwrap_or("");
                    let cc = cc.rsplit_once("__").map(|(_, t)| t).unwrap_or(cc);
                    Some(cc.to_string())
                } else {
                    None
                };
                // Declared type-parameter names in order (PY: fn f[T](x: T) is
                // monomorphized with TypeVar(i) → type_args[i]; the param's
                // type_map entry must be that Variable for substitution to
                // produce concrete param types instead of the I64 default).
                let generic_names: Vec<&String> = generics
                    .iter()
                    .filter_map(|g| match g {
                        crate::frontend::ast::GenericParam::Type { name, .. } => Some(name),
                        _ => None,
                    })
                    .collect();
                for (i, (name, param_type)) in params.iter().enumerate() {
                    let id = self.next_id();
                    self.name_to_id.insert(name.clone(), id);
                    self.exprs.insert(id, MirExpr::Var(id));
                    // Keep params as I64 — arrays pass as pointers (i64).
                    // Only true f64/i64 params should be non-I64, and those
                    // are handled by the codegen's param_types inference below.
                    self.type_map.insert(id, Type::I64);
                    // Also set the "declared" type for codegen param inference.
                    // Arrays pass as i64 pointers; true f64/i32 params get their
                    // natural type so codegen can emit the correct LLVM signature.
                    let pt_str = param_type.trim();
                    // B3: unannotated / dyn params are PyDynamic (ABI still i64).
                    if pt_str.is_empty() || pt_str == "dyn" || pt_str == "PyDynamic" {
                        self.type_map.insert(id, Type::PyDynamic);
                    } else if pt_str == "f64" || pt_str == "f32" || pt_str == "float" {
                        self.type_map.insert(id, Type::F64);
                    } else if pt_str == "bool" {
                        self.type_map.insert(id, Type::Bool);
                    } else if pt_str == "str" {
                        // Inferred string parameter (Python functions carry no
                        // annotations): string ops on it must dispatch as str.
                        self.type_map.insert(id, Type::Str);
                    } else if let Some(gidx) =
                        generic_names.iter().position(|g| g.as_str() == pt_str)
                    {
                        // Generic param `x: T` carries the type variable so
                        // monomorphization can substitute the concrete call-site
                        // type into both the signature and the body.
                        self.type_map.insert(
                            id,
                            Type::Variable(crate::middle::types::TypeVar(gidx as u32)),
                        );
                    } else if pt_str.starts_with('[') {
                        // Array param stays I64 — pointer semantics.
                        // Element type is inferred from source_types in Subscript.
                    }
                    // PY-A: a param annotated with a LIBRARY HANDLE tag
                    // (`def f(d: PyDate)`) must keep that tag. Without this it
                    // stayed I64, so `py_handle_of` saw no handle and every
                    // attribute/method on it degraded: `d.year` read garbage
                    // (18388 instead of 2020) and `d.strftime(...)` emitted a
                    // bare symbol. Only overrides the I64 default.
                    // A param whose declared type names a STRUCT we know
                    // (`&mut self: C`, or `def f(p: C)`) must keep that type:
                    // staying I64 meant the field read inside the method could
                    // not find the struct, so `self.<map field>.keys()` was an
                    // undefined `_keys`.
                    // A struct defined in an IMPORTED module is registered under
                    // its mangled name (`pandas__DataFrame`), so accept that
                    // spelling too — otherwise `self` in the library's own
                    // methods stayed I64 and every `self.<map field>.keys()` was
                    // an undefined `_keys`.
                    if matches!(self.type_map.get(&id), Some(Type::I64) | Some(Type::PyDynamic)) {
                        let mut struct_key: Option<String> = None;
                        if matches!(self.type_decls.get(pt_str), Some(TypeDecl::Struct { .. })) {
                            struct_key = Some(pt_str.to_string());
                        } else if let Some(k) = self
                            .type_decls
                            .keys()
                            .find(|k| {
                                k.ends_with(&format!("__{}", pt_str))
                                    && matches!(
                                        self.type_decls.get(*k),
                                        Some(TypeDecl::Struct { .. })
                                    )
                            })
                            .cloned()
                        {
                            struct_key = Some(k);
                        }
                        if let Some(k) = struct_key {
                            self.type_map.insert(id, Type::Named(k, vec![]));
                        }
                    }
                    if matches!(self.type_map.get(&id), Some(Type::I64) | Some(Type::PyDynamic))
                        && crate::middle::pylib::handle_tag(param_type.trim()).is_some()
                    {
                        self.type_map.insert(
                            id,
                            Type::Named(param_type.trim().to_string(), vec![]),
                        );
                    }
                    // `lt(map, K, V)` / `lt(vec, T)`: the ANNOTATION is the only
                    // place a parameter's element/value type exists. Leaving the
                    // param I64 degraded every consumer at once — `m[k]` lost its
                    // value type, so `out[m[k]] = v` inserted a RAW string handle
                    // as a key while lookups hashed it (`"y" in df` = 0, then a
                    // miss → 0 → SEGV, t204), and `k in m` never took the map
                    // branch. Canonical forms only: `vec`→DynamicArray,
                    // `map`→Named("map",[K,V]) (V added by this batch).
                    if matches!(self.type_map.get(&id), Some(Type::I64) | Some(Type::PyDynamic)) {
                        if let Some(ty) = lt_annotation_type(pt_str) {
                            self.type_map.insert(id, ty);
                        }
                    }
                    self.source_types.insert(id, param_type.clone());
                    self.stmts.push(MirStmt::ParamInit {
                        param_id: id,
                        arg_index: i as u32,
                    });
                    // &self and &mut self are parsed with "&" prefix in the name
                    // Register "self" as an alias so the body can reference it.
                    if name == "&self" || name == "&mut self" {
                        self.name_to_id.insert("self".to_string(), id);
                    }
                    // PY-A: type `self` as its class (methods lowered from
                    // Python `def m(self)` carry no annotation, so the slot
                    // stays PyDynamic and `self.<field>` loses the declared
                    // type — `key in self.data` then cannot see the map).
                    if name.trim_start_matches('&') == "self" {
                        if let Some(cls) = self.current_class.clone() {
                            if matches!(
                                self.type_map.get(&id),
                                Some(Type::I64) | Some(Type::PyDynamic)
                            ) {
                                self.type_map.insert(id, Type::Named(cls, vec![]));
                            }
                        }
                    }
                }
            }

            self.lower_ast(ast);
        }

        // Extern functions get no body stmts at all (not even a default Return)
        if is_extern {
            // Ensure clean state
            self.stmts.clear();
        } else if self.stmts.is_empty()
            || !matches!(self.stmts.last(), Some(MirStmt::Return { .. }))
        {
            let ret_val = if let Some(last) = self.stmts.last() {
                match last {
                    MirStmt::Call { dest, .. } => *dest,
                    MirStmt::SemiringFold { result, .. } => *result,
                    MirStmt::Assign { lhs, .. } => *lhs,
                    MirStmt::DictGet { dest, .. } => *dest,
                    MirStmt::If {
                        dest: Some(dest_id),
                        ..
                    } => {
                        // If expression produces a value
                        *dest_id
                    }
                    MirStmt::While { .. } if self.last_loop_result.is_some() => {
                        // loop { break EXPR; } — value lives in the result slot
                        self.last_loop_result.take().unwrap()
                    }
                    MirStmt::Return { val } => *val,
                    _ => self.next_id_with_lit(0),
                }
            } else {
                self.next_id_with_lit(0)
            };
            self.stmts.push(MirStmt::Return { val: ret_val });
        }

        Mir {
            name: match ast {
                AstNode::FuncDef { name, .. } | AstNode::ExternFunc { name, .. } => {
                    Some(name.clone())
                }
                _ => None,
            },
            generic_params: match ast {
                AstNode::FuncDef { generics, .. } => generics
                    .iter()
                    .filter_map(|g| match g {
                        crate::frontend::ast::GenericParam::Type { name, .. } => {
                            Some(name.clone())
                        }
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            },
            // Count params for potential name mangling (used by get_or_declare_function)
            param_indices: match ast {
                AstNode::FuncDef { params, .. } | AstNode::ExternFunc { params, .. } => {
                    params
                        .iter()
                        .map(|(n, _)| (n.clone(), self.name_to_id[n.as_str()].clone()))
                        .collect()
                }
                _ => vec![],
            },
            properties: if let AstNode::FuncDef { attrs, .. } = ast {
                attrs
                    .iter()
                    .filter_map(|a| {
                        let a = a.trim();
                        if a == "commutative" || a == "#[commutative]" {
                            Some("commutative".to_string())
                        } else if a == "associative" || a == "#[associative]" {
                            Some("associative".to_string())
                        } else if a.starts_with("identity") || a.starts_with("#[identity") {
                            Some(a.trim_start_matches('#').to_string())
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                vec![]
            },
            stmts: std::mem::take(&mut self.stmts),
            exprs: std::mem::take(&mut self.exprs),
            is_extern, // store whether this is an extern/FFI declaration
            ctfe_consts: std::mem::take(&mut self.ctfe_consts),
            type_map: std::mem::take(&mut self.type_map),
            global_consts: std::mem::take(&mut self.global_consts),
        }
    }

    fn lower_ast(&mut self, ast: &AstNode) {
        let is_def = matches!(ast, AstNode::FuncDef { .. });
        if is_def {
            self.fn_depth += 1;
        }
        self.lower_ast_inner(ast);
        if is_def {
            self.fn_depth -= 1;
        }
    }

    fn lower_ast_inner(&mut self, ast: &AstNode) {
        match ast {
            AstNode::Let { pattern, expr, .. } => {
                // Handle different pattern types
                match &**pattern {
                    AstNode::Var(name) if self.nonlocal_names.contains(name) => {
                        // PY-A V3: nonlocal name — defining assignment stores
                        // through env; bind local slot to an env load.
                        let rhs_id = self.lower_expr(expr);
                        let key_id = self.next_id();
                        self.exprs
                            .insert(key_id, MirExpr::StringLit(name.clone()));
                        self.type_map.insert(key_id, Type::Str);
                        self.stmts.push(MirStmt::VoidCall {
                            func: "zeta_env_set".to_string(),
                            args: vec![key_id, rhs_id],
                        });
                        let slot_id = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "zeta_env_get".to_string(),
                            args: vec![key_id],
                            dest: slot_id,
                            type_args: vec![],
                        });
                        self.exprs.insert(slot_id, MirExpr::Var(slot_id));
                        self.type_map.insert(slot_id, Type::I64);
                        self.name_to_id.insert(name.clone(), slot_id);
                    }
                    AstNode::Var(name) => {
                        self.pending_closure_binding = Some(name.clone());
                        let rhs_id = self.lower_expr(expr);
                        self.pending_closure_binding = None;
                        let lhs_id = self.next_id();
                        self.stmts.push(MirStmt::Assign {
                            lhs: lhs_id,
                            rhs: rhs_id,
                        });
                        self.name_to_id.insert(name.clone(), lhs_id);
                        self.exprs.insert(lhs_id, MirExpr::Var(lhs_id));
                        // Copy type from RHS to LHS
                        if let Some(rhs_type) = self.type_map.get(&rhs_id) {
                            self.type_map.insert(lhs_id, rhs_type.clone());
                        } else {
                            self.type_map.insert(lhs_id, Type::I64);
                        }
                    }
                    AstNode::TypeAnnotatedPattern {
                        pattern: inner_pattern,
                        ty: _,
                    } => {
                        // For type-annotated patterns, extract the inner pattern
                        // The type checking should have been done by the type checker
                        if let AstNode::Var(name) = &**inner_pattern {
                            let rhs_id = self.lower_expr(expr);
                            let lhs_id = self.next_id();
                            self.stmts.push(MirStmt::Assign {
                                lhs: lhs_id,
                                rhs: rhs_id,
                            });
                            self.name_to_id.insert(name.clone(), lhs_id);
                            self.exprs.insert(lhs_id, MirExpr::Var(lhs_id));
                            // Copy type from RHS to LHS
                            if let Some(rhs_type) = self.type_map.get(&rhs_id) {
                                self.type_map.insert(lhs_id, rhs_type.clone());
                            } else {
                                self.type_map.insert(lhs_id, Type::I64);
                            }
                        }
                        // Note: We could add runtime identity checking here if needed,
                        // but the type checker should have already validated the type.
                    }
                    _ => {
                        // For other pattern types, generate a simple assignment
                        // This is a simplification - in a full implementation,
                        // we would need to handle destructuring patterns
                        if let AstNode::Tuple(elements) = &**pattern {
                            // Tuple destructuring: let (a, b) = expr;
                            // Lower as multiple field accesses.
                            let rhs_id = self.lower_expr(expr);
                            for (i, elem) in elements.iter().enumerate() {
                                if let AstNode::Var(name) = elem {
                                    let elem_id = self.next_id();
                                    // Access field i of the tuple
                                    let field_id = self.next_id();
                                    self.exprs.insert(field_id, MirExpr::IntLit(i as i64));
                                    self.type_map.insert(field_id, Type::I64);
                                    self.stmts.push(MirStmt::Call {
                                        func: "stack_array_get".to_string(),
                                        args: vec![rhs_id, field_id],
                                        dest: elem_id,
                                        type_args: vec![],
                                    });
                                    self.name_to_id.insert(name.clone(), elem_id);
                                    self.exprs.insert(elem_id, MirExpr::Var(elem_id));
                                    self.type_map.insert(elem_id, Type::I64);
                                }
                            }
                        } else {
                            let rhs_id = self.lower_expr(expr);
                            let lhs_id = self.next_id();
                            self.stmts.push(MirStmt::Assign {
                                lhs: lhs_id,
                                rhs: rhs_id,
                            });
                            self.exprs.insert(lhs_id, MirExpr::Var(lhs_id));
                            if let Some(rhs_type) = self.type_map.get(&rhs_id) {
                                self.type_map.insert(lhs_id, rhs_type.clone());
                            } else {
                                self.type_map.insert(lhs_id, Type::I64);
                            }
                        }
                    }
                }
            }
            AstNode::Assign(lhs, rhs) => {
                // PY-A: parallel assignment `a, b = x, y` (tuple unpacking with
                // tuple rhs; call-return unpacking needs temps — later item)
                if let (AstNode::Tuple(litems), AstNode::Tuple(ritems)) = (&**lhs, &**rhs) {
                    if litems.len() == ritems.len() && !litems.is_empty() {
                        for (l, r) in litems.iter().zip(ritems.iter()) {
                            let pair = AstNode::Assign(Box::new(l.clone()), Box::new(r.clone()));
                            self.lower_ast(&pair);
                        }
                        return;
                    }
                }
                // PY-A: call-return tuple unpacking `a, b = f()` — the rhs is
                // lowered ONCE into a temp, elements read via stack_array_get
                // (tuples materialize as fixed-size arrays).
                if let AstNode::Tuple(litems) = &**lhs {
                    if !litems.is_empty()
                        && !matches!(&**rhs, AstNode::Tuple(_))
                    {
                        let rhs_id = self.lower_expr(rhs);
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
                                self.type_map.insert(elem_id, Type::I64);
                            }
                        }
                        return;
                    }
                }
                // PY-A: Python-style `f = lambda …` must register the closure
                // binding, exactly like the Zeta `let f = lambda …` path does.
                // Without it a later `f(x)` emitted a CALL to a symbol named `f`,
                // which does not exist — the link failed with `_f` undefined.
                let binding_name = match &**lhs {
                    AstNode::Var(n) if matches!(&**rhs, AstNode::Closure { .. }) => Some(n.clone()),
                    _ => None,
                };
                if let Some(n) = &binding_name {
                    self.pending_closure_binding = Some(n.clone());
                }
                let rhs_id = self.lower_expr(rhs);
                self.pending_closure_binding = None;
                if let AstNode::Subscript { base, index } = &**lhs {
                    let base_id = self.lower_expr(base);
                    let index_id = self.lower_expr(index);

                    // Check if base is an array type
                    let base_ty = self.type_map.get(&base_id).cloned().unwrap_or(Type::I64);
                    let source_ty = self.source_types.get(&base_id).cloned().unwrap_or_default();
                    let is_array_param =
                        source_ty.starts_with("[") || source_ty.starts_with("*mut [");
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
                        self.stmts.push(MirStmt::DictInsert {
                            map_id: base_id,
                            key_id,
                            val_id: rhs_id,
                        });
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
                        self.stmts.push(MirStmt::DictInsert {
                            map_id: base_id,
                            key_id: index_id,
                            val_id: rhs_id,
                        });
                    }
                } else if let AstNode::FieldAccess { base, field } = &**lhs {
                    // self.field = val → store through the heap struct pointer
                    let base_id = self.lower_expr(base);
                    self.stmts.push(MirStmt::StructFieldStore {
                        base_id,
                        field: field.clone(),
                        val_id: rhs_id,
                    });
                } else if let AstNode::UnaryOp { op, expr } = &**lhs {
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
                } else if let AstNode::Var(name) = &**lhs {
                    // PY-A: Python-style bare assignment — implicitly declare
                    // the variable when it is not already bound (function
                    // locals; module-level variables are a later item).
                    if let Some(&existing) = self.name_to_id.get(name) {
                        if self.nonlocal_names.contains(name) {
                            // PY-A V3: nonlocal write → env store
                            let key_id = self.next_id();
                            self.exprs
                                .insert(key_id, MirExpr::StringLit(name.clone()));
                            self.type_map.insert(key_id, Type::Str);
                            self.stmts.push(MirStmt::VoidCall {
                                func: "zeta_env_set".to_string(),
                                args: vec![key_id, rhs_id],
                            });
                            let unit_id = self.next_id();
                            self.exprs.insert(unit_id, MirExpr::IntLit(0));
                            self.type_map.insert(unit_id, Type::Tuple(vec![]));
                            return;
                        }
                        self.stmts.push(MirStmt::Assign {
                            lhs: existing,
                            rhs: rhs_id,
                        });
                        if self.module_globals.contains(name) {
                            // module-global update: keep the local slot (with
                            // its real type) AND mirror the value into the env
                            // so other functions read the fresh value.
                            let key_id = self.next_id();
                            self.exprs
                                .insert(key_id, MirExpr::StringLit(name.clone()));
                            self.type_map.insert(key_id, Type::Str);
                            self.stmts.push(MirStmt::VoidCall {
                                func: "zeta_env_set".to_string(),
                                args: vec![key_id, rhs_id],
                            });
                        }
                    } else if self.nonlocal_names.contains(name) {
                        // PY-A V3: inner-scope write before any local bind
                        let key_id = self.next_id();
                        self.exprs
                            .insert(key_id, MirExpr::StringLit(name.clone()));
                        self.type_map.insert(key_id, Type::Str);
                        self.stmts.push(MirStmt::VoidCall {
                            func: "zeta_env_set".to_string(),
                            args: vec![key_id, rhs_id],
                        });
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
                        self.type_map.insert(slot_id, Type::I64);
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
                            rhs.clone(),
                        ));
                    } else {
                        // module-global / plain local: bind normally so the
                        // local slot keeps the rhs's real type (dict/Vec/etc
                        // would be mangled by an env-load I64 alias).
                        let new_id = self.next_id();
                        self.name_to_id.insert(name.clone(), new_id);
                        self.exprs.insert(new_id, MirExpr::Var(new_id));
                        let ty = self.type_map.get(&rhs_id).cloned().unwrap_or(Type::I64);
                        self.type_map.insert(new_id, ty);
                        self.stmts.push(MirStmt::Assign {
                            lhs: new_id,
                            rhs: rhs_id,
                        });
                        if self.module_globals.contains(name) {
                            // mirror into env so cross-function reads work
                            let key_id = self.next_id();
                            self.exprs
                                .insert(key_id, MirExpr::StringLit(name.clone()));
                            self.type_map.insert(key_id, Type::Str);
                            self.stmts.push(MirStmt::VoidCall {
                                func: "zeta_env_set".to_string(),
                                args: vec![key_id, rhs_id],
                            });
                        }
                    }
                } else {
                    let lhs_id = self.lower_expr(lhs);
                    self.stmts.push(MirStmt::Assign {
                        lhs: lhs_id,
                        rhs: rhs_id,
                    });
                }
            }
            AstNode::AssignOp { op, target, value } => {
                // Desugar: target op= value → target = target op value
                let new_rhs = Box::new(AstNode::BinaryOp {
                    op: op.clone(),
                    left: target.clone(),
                    right: value.clone(),
                });
                let assign = AstNode::Assign(target.clone(), new_rhs);
                self.lower_ast(&assign);
            }
            AstNode::Return(inner) => {
                let val = self.lower_expr(inner);
                self.stmts.push(MirStmt::Return { val });
            }
            AstNode::BinaryOp { op, left, right } => {
                let _ = self.lower_expr(&AstNode::BinaryOp {
                    op: op.clone(),
                    left: left.clone(),
                    right: right.clone(),
                });
            }
            AstNode::TryProp { expr } => {
                let expr_id = self.lower_expr(expr);
                let ok = self.next_id();
                let err = self.next_id();
                self.stmts.push(MirStmt::TryProp {
                    expr_id,
                    ok_dest: ok,
                    err_dest: err,
                });
            }
            AstNode::DictLit { .. } => {
                // PY-A fix: dict literals are expressions (assignment rhs,
                // call args). Delegated to lower_expr, which owns the real
                // lowering — lower_expr had NO DictLit branch, so a dict rhs
                // silently became IntLit(0).
                self.lower_expr(ast);
            }
            AstNode::Subscript { base, index } => {
                // This is handled in lower_expr
                let _ = self.lower_expr(&AstNode::Subscript {
                    base: base.clone(),
                    index: index.clone(),
                });
            }
            AstNode::FuncDef {
                name: fn_name,
                params,
                body,
                ret_expr,
                ..
            } => {
                // PY-A: NESTED def inside a function body — lowering inline
                // mixes its Returns into the enclosing stream (double
                // terminator). Instead, lower it as a STANDALONE synthetic
                // function via the same child-MirGen path as closures, and
                // publish it through generated_mirs (merged into codegen by
                // the Resolver pipeline). Free variables resolve through the
                // env runtime; `outer.inner(...)` and bare `inner(...)`
                // call sites both bind by name via nonlocal/fallback.
                if self.fn_depth > 1 {
                    let param_names: Vec<String> =
                        params.iter().map(|(n, _)| n.clone()).collect();
                    let body_node = AstNode::Block { body: body.clone() };
                    let hoisted = self.lower_closure(&param_names, &body_node);
                    // bind user name → synthetic fn so `inc()` calls dispatch
                    self.closure_vars.insert(fn_name.clone(), hoisted.clone());
                    // Publish under the user-visible name too (alias map)
                    self.hoisted_names.insert(fn_name.clone(), hoisted);
                    return;
                }
                for stmt in body {
                    self.lower_ast(stmt);
                }
                if let Some(ret_expr) = ret_expr {
                    let val = self.lower_expr(ret_expr);
                    self.stmts.push(MirStmt::Return { val });
                }
            }
            AstNode::If { cond, then, else_ } => {
                // PY-A: compile-time env switch (see fold_env_condition).
                if let Some(taken) = fold_env_condition(cond) {
                    let branch: &[AstNode] = if taken { then } else { else_ };
                    for s in branch {
                        self.lower_expr(s);
                    }
                    return;
                }
                let cond_id = self.lower_expr(cond);

                // Check if this is expression if (branches produce values) or statement if
                // Simple heuristic: if any branch contains return, treat as statement
                let mut is_statement_if = false;
                let mut then_has_return = false;
                let mut else_has_return = false;

                // Scan branches for returns/breaks/continues (statement-only)
                for s in then.iter() {
                    if let AstNode::Return(_) | AstNode::Break(_) | AstNode::Continue(_) = s {
                        then_has_return = true;
                        is_statement_if = true;
                        break;
                    }
                }
                for s in else_.iter() {
                    if let AstNode::Return(_) | AstNode::Break(_) | AstNode::Continue(_) = s {
                        else_has_return = true;
                        is_statement_if = true;
                        break;
                    }
                }

                let dest_id = if is_statement_if {
                    // Statement if: no destination needed
                    None
                } else {
                    // Expression if: create destination
                    let id = self.next_id();

                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    Some(id)
                };

                // Generate then block in isolated context
                // Generate then block inline (no isolated context)
                let mut then_stmts = vec![];
                if !then.is_empty() {
                    // Save current statements
                    let saved_stmts = std::mem::take(&mut self.stmts);

                    // Generate block statements directly in current context
                    for s in then {
                        self.lower_ast(s);
                    }

                    // Take the generated statements
                    then_stmts = std::mem::take(&mut self.stmts);

                    // Restore main statements
                    self.stmts = saved_stmts;

                    // For expression if, capture the last value
                    if let Some(dest) = dest_id
                        && !then_has_return
                    {
                        if let Some(last_stmt) = then_stmts.last() {
                            match last_stmt {
                                MirStmt::Assign { lhs, .. } => {
                                    // Add assignment to dest
                                    then_stmts.push(MirStmt::Assign {
                                        lhs: dest,
                                        rhs: *lhs,
                                    });
                                }
                                MirStmt::If {
                                    dest: Some(if_dest),
                                    ..
                                } => {
                                    // Block ends with if expression - use its destination
                                    then_stmts.push(MirStmt::Assign {
                                        lhs: dest,
                                        rhs: *if_dest,
                                    });
                                }
                                MirStmt::Call {
                                    dest: call_dest, ..
                                } => {
                                    // Block ends with function call - use its result
                                    then_stmts.push(MirStmt::Assign {
                                        lhs: dest,
                                        rhs: *call_dest,
                                    });
                                }
                                _ => {
                                    // No value-producing statement found
                                    let zero_id = self.next_id_with_lit(0);
                                    then_stmts.push(MirStmt::Assign {
                                        lhs: dest,
                                        rhs: zero_id,
                                    });
                                }
                            }
                        } else if let Some(last_ast) = then.last() {
                            // then_stmts is empty but then block has AST nodes
                            // Lower the last AST as an expression for its value
                            let val_id = self.lower_expr(last_ast);
                            then_stmts.push(MirStmt::Assign {
                                lhs: dest,
                                rhs: val_id,
                            });
                        }
                    }
                }

                // Generate else block inline
                let mut else_stmts = vec![];
                if !else_.is_empty() {
                    // Save current statements
                    let saved_stmts = std::mem::take(&mut self.stmts);

                    // Generate block statements directly in current context
                    for s in else_ {
                        self.lower_ast(s);
                    }

                    // Take the generated statements
                    else_stmts = std::mem::take(&mut self.stmts);

                    // Restore main statements
                    self.stmts = saved_stmts;

                    // For expression if, capture the last value
                    if let Some(dest) = dest_id
                        && !else_has_return
                    {
                        if let Some(last_stmt) = else_stmts.last() {
                            match last_stmt {
                                MirStmt::Assign { lhs, .. } => {
                                    // Add assignment to dest
                                    else_stmts.push(MirStmt::Assign {
                                        lhs: dest,
                                        rhs: *lhs,
                                    });
                                }
                                MirStmt::If {
                                    dest: Some(if_dest),
                                    ..
                                } => {
                                    // Block ends with if expression - use its destination
                                    else_stmts.push(MirStmt::Assign {
                                        lhs: dest,
                                        rhs: *if_dest,
                                    });
                                }
                                MirStmt::Call {
                                    dest: call_dest, ..
                                } => {
                                    // Block ends with function call - use its result
                                    else_stmts.push(MirStmt::Assign {
                                        lhs: dest,
                                        rhs: *call_dest,
                                    });
                                }
                                _ => {
                                    // No value-producing statement found
                                    let zero_id = self.next_id_with_lit(0);
                                    else_stmts.push(MirStmt::Assign {
                                        lhs: dest,
                                        rhs: zero_id,
                                    });
                                }
                            }
                        } else if let Some(last_ast) = else_.last() {
                            // else_stmts is empty but else block has AST nodes
                            // Lower the last AST as an expression for its value
                            let val_id = self.lower_expr(last_ast);
                            else_stmts.push(MirStmt::Assign {
                                lhs: dest,
                                rhs: val_id,
                            });
                        }
                    }
                }

                // Expressions are already in self.exprs (generated inline)
                // No need to merge or update next_id

                self.stmts.push(MirStmt::If {
                    cond: cond_id,
                    then: then_stmts,
                    else_: else_stmts,
                    dest: dest_id,
                });
            }
            AstNode::ExprStmt { expr } => {
                if let AstNode::Return(inner) = &**expr {
                    let val = self.lower_expr(inner);
                    self.stmts.push(MirStmt::Return { val });
                } else {
                    let expr_id = self.lower_expr(expr);
                    // For expression statements that are values (not just side effects),
                    // we need to capture the value. Create a temporary assignment.
                    // This will be optimized away if not needed.
                    let temp_id = self.next_id();
                    self.stmts.push(MirStmt::Assign {
                        lhs: temp_id,
                        rhs: expr_id,
                    });
                    // Store the temp ID for implicit return to find
                    self.exprs.insert(temp_id, MirExpr::Var(temp_id));
                    self.type_map.insert(temp_id, Type::I64);
                }
            }
            AstNode::For {
                pattern,
                expr,
                body,
                else_body,
            } => {
                // For now, implement simple desugaring for range-based for loops
                // for i in start..end { body } desugars to:
                // let mut i = start;
                // while i < end {
                //   body;
                //   i = i + 1;
                // }

                // Check if expr is a range expression (BinaryOp with ".." or AstNode::Range)
                let (start_expr, end_expr): (Box<AstNode>, Box<AstNode>) = match &**expr {
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
                            let raw_id = if matches!(
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
                                self.type_map.get(&raw_id).cloned().unwrap_or(Type::I64),
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
                            let (var_name, is_simple_var) = if let AstNode::Var(n) = &*pattern_clone
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
                            match &*pattern_clone {
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

                            for stmt in &body_clone {
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
                let range_var: Option<String> = match &**pattern {
                    AstNode::Var(n) => Some(n.clone()),
                    AstNode::Ignore => Some("__wildcard".to_string()),
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

                    // Initialize loop variable: let mut i = start
                    self.stmts.push(MirStmt::Assign {
                        lhs: var_id,
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

                    // Generate loop body
                    for stmt in body {
                        self.lower_ast(stmt);
                    }

                    // Get body statements
                    let body_stmts = self.stmts.split_off(stmts_before_body);

                    // PY-A: `for … else` — lowered after the split.
                    let else_stmts = self.lower_loop_else(else_body);

                    // Create For statement in MIR
                    self.stmts.push(MirStmt::For {
                        iterator: range_id,
                        pattern: var_name.clone(),
                        var_id,
                        body: body_stmts,
                        else_body: else_stmts,
                    });
                }
            }
            AstNode::Loop { body } => {
                // Loop as statement: value is captured via last_loop_result.
                self.last_loop_result = self.loop_value_stack.last().cloned();
                let _ = self.lower_expr(&AstNode::Loop { body: body.clone() });
                // After the call, loop_value_stack is empty; last_loop_result
                // holds the slot that the while-exit will fall through to.
                // We clear it so it only applies to the immediately preceding loop.
                // The gen_fn ret_val computation reads it below.
                // (last_loop_result is intentionally NOT cleared here —
                // gen_fn checks it after the match so the last-stmt logic works.)
            }
            AstNode::While {
                cond,
                body,
                else_body,
            } => {
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
            AstNode::Unsafe { body } => {
                for stmt in body {
                    self.lower_ast(stmt);
                }
            }
            AstNode::ComptimeBlock { body } => {
                for stmt in body {
                    self.lower_ast(stmt);
                }
            }
            AstNode::Break(val) => {
                // break EXPR — write the value to the enclosing loop's result slot
                if let Some(v) = val {
                    if let Some(&result_id) = self.loop_value_stack.last() {
                        let val_id = self.lower_expr(v);
                        self.stmts.push(MirStmt::Assign {
                            lhs: result_id,
                            rhs: val_id,
                        });
                    }
                }
                self.stmts.push(MirStmt::Break);
            }
            AstNode::Continue(_) => {
                self.stmts.push(MirStmt::Continue);
            }
            // Expression-as-statement nodes: lower the expression, discard the result value
            // (side effects through self.stmts are what matter)
            AstNode::ConstDef { name, value, .. } => {
                // Register local constant values so Var(name) references can resolve them
                // CTFE should have already evaluated `value` to a Lit/Bool by this point
                match &**value {
                    AstNode::Lit(n) => {
                        self.global_consts.insert(
                            name.clone(),
                            crate::middle::ctfe::value::ConstValue::Int(*n),
                        );
                    }
                    AstNode::Bool(b) => {
                        self.global_consts.insert(
                            name.clone(),
                            crate::middle::ctfe::value::ConstValue::Bool(*b),
                        );
                    }
                    AstNode::StringLit(s) => {
                        self.global_consts.insert(
                            name.clone(),
                            crate::middle::ctfe::value::ConstValue::String(s.clone()),
                        );
                    }
                    _ => {
                        // Try CTFE evaluation at MIR gen time
                        if let Ok(val) = crate::middle::ctfe::eval_const_expr(value) {
                            self.global_consts.insert(name.clone(), val);
                        }
                    }
                }
            }
            // ── Priority A: Type Definition Nodes ──
            AstNode::StructDef {
                name,
                fields,
                generics,
                ..
            } => {
                // Register struct type definition for later reference by StructLit / FieldAccess.
                self.type_decls.insert(
                    name.clone(),
                    TypeDecl::Struct {
                        fields: fields.clone(),
                        generics: generics.clone(),
                    },
                );
            }
            AstNode::EnumDef {
                name,
                variants,
                generics,
                ..
            } => {
                // Register enum type definition for pattern-match lowering.
                self.type_decls.insert(
                    name.clone(),
                    TypeDecl::Enum {
                        variants: variants.clone(),
                        generics: generics.clone(),
                    },
                );
            }
            AstNode::ImplBlock { body, .. } => {
                // Lower any items inside the impl block (functions, etc.).
                for item in body {
                    self.lower_ast(item);
                }
            }
            AstNode::ConceptDef { methods, .. } => {
                // Lower any default-method bodies inside the concept.
                for method in methods {
                    self.lower_ast(method);
                }
            }
            AstNode::TypeAlias { name, ty, .. } => {
                // Register the type alias so type resolution works at MIR level.
                self.type_decls
                    .insert(name.clone(), TypeDecl::Alias { target: ty.clone() });
            }
            AstNode::Method {
                body: Some(method_body),
                ..
            } => {
                // Lower default method bodies (inside concepts/traits).
                for stmt in method_body {
                    self.lower_ast(stmt);
                }
            }
            AstNode::Method { body: None, .. } => {
                // Method signature without body — no code to generate.
            }
            AstNode::AssociatedType { .. } => {
                // Associated type declaration — no runtime code.
            }
            AstNode::ExternFunc { .. } => {
                // Extern/FFI function declaration — no body to lower.
            }
            // ── Priority B: Pattern Matching Nodes ──
            AstNode::IfLet {
                pattern,
                expr,
                then,
                else_,
            } => {
                // Desugar: if let <pat> = <expr> { then } [else { else_ }]
                match &**pattern {
                    AstNode::Var(name) => {
                        // Simple binding — always matches.
                        let expr_id = self.lower_expr(expr);
                        let lhs_id = self.next_id();
                        self.stmts.push(MirStmt::Assign {
                            lhs: lhs_id,
                            rhs: expr_id,
                        });
                        self.name_to_id.insert(name.clone(), lhs_id);
                        self.exprs.insert(lhs_id, MirExpr::Var(lhs_id));
                        if let Some(ty) = self.type_map.get(&expr_id) {
                            self.type_map.insert(lhs_id, ty.clone());
                        } else {
                            self.type_map.insert(lhs_id, Type::I64);
                        }
                        for stmt in then {
                            self.lower_ast(stmt);
                        }
                    }
                    AstNode::Ignore => {
                        // Wildcard — always matches, discard value.
                        self.lower_expr(expr);
                        for stmt in then {
                            self.lower_ast(stmt);
                        }
                    }
                    _ => {
                        // Complex pattern: lower expr (side effects), always run then.
                        self.lower_expr(expr);
                        for stmt in then {
                            self.lower_ast(stmt);
                        }
                    }
                }
            }
            AstNode::Tuple(elements) => {
                // Tuple in statement position — evaluate all elements.
                for elem in elements {
                    self.lower_ast(elem);
                }
            }
            AstNode::Ignore => {
                // Wildcard / ignore — no-op in statement position.
            }
            AstNode::StructPattern { .. } => {
                // Struct destructuring pattern in statement position — no-op for now.
            }
            AstNode::OrPattern(_) | AstNode::BindPattern { .. } | AstNode::RangePattern { .. } => {
                // These are primarily used inside Match / IfLet arms.
                // As standalone stmts, evaluate them as expressions.
                self.lower_expr(ast);
            }
            // ── Priority C: Module System ──
            AstNode::Use { .. } => {
                // Use/import declaration — all semantic processing handled by resolver.
            }
            AstNode::ModDef { items, .. } => {
                // Module definition — lower all items.
                for item in items {
                    self.lower_ast(item);
                }
            }
            // ── Priority D & E: Remaining Nodes ──
            AstNode::Defer(body) => {
                // Defer: execute the body immediately (simplified lowering).
                self.lower_ast(body);
            }
            AstNode::Await(body) => {
                // Await statement: evaluate the inner expression and poll.
                let fut_id = self.lower_expr(body);
                let pr_id = self.next_id();
                let zero_id = self.next_id_with_lit(0);
                let stored_fut = self.next_id();
                self.exprs.insert(stored_fut, MirExpr::Var(stored_fut));
                self.type_map.insert(stored_fut, Type::I64);
                self.stmts.push(MirStmt::Assign {
                    lhs: stored_fut,
                    rhs: fut_id,
                });
                self.exprs.insert(pr_id, MirExpr::Var(pr_id));
                self.type_map.insert(pr_id, Type::I64);

                let mut body_stmts = vec![];
                body_stmts.push(MirStmt::Call {
                    func: "future_poll".to_string(),
                    args: vec![stored_fut],
                    dest: pr_id,
                    type_args: vec![],
                });
                let mut then_stmts = vec![];
                then_stmts.push(MirStmt::Break);
                let cond_id = self.next_id();
                self.exprs.insert(
                    cond_id,
                    MirExpr::BinaryOp {
                        op: "!=".to_string(),
                        left: pr_id,
                        right: zero_id,
                    },
                );
                self.type_map.insert(cond_id, Type::Bool);
                body_stmts.push(MirStmt::If {
                    cond: cond_id,
                    then: then_stmts,
                    else_: vec![],
                    dest: None,
                });
                let true_id = self.next_id_with_lit(1);
                self.stmts.push(MirStmt::While {
                    cond: true_id,
                    pre_cond: vec![],
                    body: body_stmts,
                    else_body: vec![],
                });
            }
            AstNode::Closure { body, .. } => {
                // Closure in statement position: evaluate body as expression.
                self.lower_expr(body);
            }
            AstNode::Spawn { func, args } => {
                // Actor spawn: treat as a function call for now.
                let mut arg_ids = vec![];
                for a in args {
                    arg_ids.push(self.lower_expr(a));
                }
                let spawn_dest = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: format!("spawn_{}", func),
                    args: arg_ids,
                    dest: spawn_dest,
                    type_args: vec![],
                });
                self.exprs.insert(spawn_dest, MirExpr::Var(spawn_dest));
                self.type_map.insert(spawn_dest, Type::I64);
            }
            AstNode::TimingOwned { inner, .. } => {
                // Constant-time wrapper: evaluate inner expression with TimingOwned node.
                self.lower_expr(inner);
            }
            AstNode::Block { body } => {
                // Block as statement: lower all body statements.
                for stmt in body {
                    self.lower_ast(stmt);
                }
            }
            AstNode::MacroCall { .. } | AstNode::MacroDef { .. } => {
                // Macros should have been expanded before MIR lowering.
                // Silently skip if they reach here.
            }
            AstNode::If { .. } | AstNode::Call { .. } | AstNode::PathCall { .. } => {
                self.lower_expr(ast);
            }
            _ => {}
        }
    }

    /// PY-A: normalize a dict key — string keys hash by CONTENT (map_str_key,
    /// FNV-1a) because identical literals allocate distinct handles and the
    /// runtime map compares keys numerically. Non-string keys pass through.
    /// Content-hash a key for a map whose declared KEY TYPE is `str`.
    /// `lower_map_key` only hashes when the KEY's own type is `Str`; with an
    /// unannotated parameter the key is I64, so a content-hashing map was
    /// probed by POINTER — a silent miss (value 0, no diagnostic).
    fn lower_map_key_typed(&mut self, id: u32, map_ty: Option<&Type>) -> u32 {
        let key_is_str = matches!(
            map_ty,
            Some(Type::Named(_, targs)) if matches!(targs.first(), Some(Type::Str))
        );
        if key_is_str {
            let nid = self.next_id();
            self.stmts.push(MirStmt::Call {
                func: "map_str_key".to_string(),
                args: vec![id],
                dest: nid,
                type_args: vec![],
            });
            self.exprs.insert(nid, MirExpr::Var(nid));
            self.type_map.insert(nid, Type::I64);
            return nid;
        }
        self.lower_map_key(id)
    }

    fn lower_map_key(&mut self, id: u32) -> u32 {
        if matches!(self.type_map.get(&id), Some(Type::Str)) {
            let nid = self.next_id();
            self.stmts.push(MirStmt::Call {
                func: "map_str_key".to_string(),
                args: vec![id],
                dest: nid,
                type_args: vec![],
            });
            self.exprs.insert(nid, MirExpr::Var(nid));
            self.type_map.insert(nid, Type::I64);
            return nid;
        }
        id
    }

    /// PY-A: ensure an expression id is a string handle — non-string values
    /// go through the to_string_* runtime dispatch (Python `str()`).
    fn lower_to_string(&mut self, id: u32) -> u32 {
        if matches!(self.type_map.get(&id), Some(Type::Str)) {
            return id;
        }
        // A handle whose value already IS a string (pathlib.Path) needs no
        // conversion — otherwise it went through to_string_i64 and the
        // pointer was printed as a number.
        if matches!(self.type_map.get(&id), Some(Type::Named(n, _)) if n == "PyPath") {
            return id;
        }
        let func = match self.type_map.get(&id).cloned() {
            Some(Type::F64) | Some(Type::F32) => "to_string_f64",
            Some(Type::Bool) => "to_string_bool",
            _ => "to_string_i64",
        };
        let nid = self.next_id();
        self.stmts.push(MirStmt::Call {
            func: func.to_string(),
            args: vec![id],
            dest: nid,
            type_args: vec![],
        });
        self.exprs.insert(nid, MirExpr::Var(nid));
        self.type_map.insert(nid, Type::Str);
        nid
    }

    /// If `name` is a unit-variant path of a registered enum (e.g.
    /// `Color::Green`), return its variant index. Data-carrying variants are
    /// not covered (they need tagged allocation; runtime-backed enums like
    /// Option/Result are handled separately).
    fn enum_unit_variant_index(&self, name: &str) -> Option<i64> {
        let (enum_name, variant) = name.rsplit_once("::")?;
        match self.type_decls.get(enum_name)? {
            TypeDecl::Enum { variants, .. } => variants
                .iter()
                .position(|(v, params)| v == variant && params.is_empty())
                .map(|i| i as i64),
            _ => None,
        }
    }

    fn lower_expr(&mut self, expr: &AstNode) -> u32 {
        let id = self.next_id();
        match expr {
            AstNode::Block { body } => {
                // Block expression: lower body statements, capture last value.
                if body.is_empty() {
                    self.exprs.insert(id, MirExpr::IntLit(0));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
                // Save and isolate block stmts
                let saved_stmts = std::mem::take(&mut self.stmts);
                // Lower all but the last as statements
                for stmt in &body[..body.len() - 1] {
                    self.lower_ast(stmt);
                }
                // Lower the last as an expression (block value)
                let last = &body[body.len() - 1];
                // If last is an ExprStmt, unwrap it
                let val_id = match last {
                    AstNode::ExprStmt { expr } => self.lower_expr(expr),
                    // PY-A: statement-form last (Assign/Let/Return…) — lower
                    // as a statement; the block value falls back to the last
                    // produced value or 0.
                    AstNode::Return(_) => self.i64_zero_id(), // Return handled by closure fn tail
                    AstNode::Assign(_, _) | AstNode::Let { .. } => {
                        self.lower_ast(last);
                        self.i64_zero_id()
                    }
                    other => self.lower_expr(other),
                };
                let block_stmts = std::mem::take(&mut self.stmts);
                self.stmts = saved_stmts;
                // Forward all block stmts
                for s in block_stmts {
                    self.stmts.push(s);
                }
                // Assign the block result to the block's local slot
                self.stmts.push(MirStmt::Assign {
                    lhs: id,
                    rhs: val_id,
                });
                // Store the block result (reference own alloca so gen_expr_safe loads from it)
                self.exprs.insert(id, MirExpr::Var(id));
                if let Some(ty) = self.type_map.get(&val_id) {
                    self.type_map.insert(id, ty.clone());
                } else {
                    self.type_map.insert(id, Type::I64);
                }
                return id;
            }
            AstNode::FloatLit(s) => {
                // Float literal: parse directly to f64, store as FloatLit.
                let val: f64 = s.parse::<f64>().ok().unwrap_or(0.0);
                self.exprs.insert(id, MirExpr::FloatLit(val));
                self.type_map.insert(id, Type::F64);
            }
            AstNode::MacroCall { .. } | AstNode::MacroDef { .. } => {
                // Macros should be expanded before MIR lowering.
                // If they reach here, silently return 0.
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
            }
            // Match is handled below with full if-else chain lowering.
            AstNode::Assign(lhs, rhs) => {
                // PY-A: walrus `name := expr` in expression position — lower
                // rhs, bind the name (implicit decl or rebinding), and the
                // expression value is the assigned value.
                if let AstNode::Var(name) = &**lhs {
                    let rhs_id = self.lower_expr(rhs);
                    if !self.name_to_id.contains_key(name) {
                        let new_id = self.next_id();
                        self.exprs.insert(new_id, MirExpr::Var(new_id));
                        let ty = self.type_map.get(&rhs_id).cloned().unwrap_or(Type::I64);
                        self.type_map.insert(new_id, ty);
                        self.name_to_id.insert(name.clone(), new_id);
                        self.stmts.push(MirStmt::Assign {
                            lhs: new_id,
                            rhs: rhs_id,
                        });
                    }
                    return rhs_id;
                }
                // Non-var lhs: statement assign with a 0-value expression
                self.lower_ast(&AstNode::Assign(lhs.clone(), rhs.clone()));
                let z = self.next_id();
                self.exprs.insert(z, MirExpr::IntLit(0));
                self.type_map.insert(z, Type::I64);
                return z;
            }
            AstNode::Var(name) => {
                // PY-A: `__file__` — the source path, known at compile time.
                if name == "__file__" && !self.name_to_id.contains_key(name.as_str()) {
                    if let Some(f) = self.source_file.clone() {
                        self.exprs.insert(id, MirExpr::StringLit(f));
                        self.type_map.insert(id, Type::Str);
                        return id;
                    }
                }
                // PY-A: a module's own top-level name reads its module-global
                // slot (`mod__NAME` in the env). Locals win, so only rewrite
                // when nothing local shadows it.
                if !self.name_to_id.contains_key(name.as_str()) {
                    if let Some(mangled) = self.symbol_renames.get(name.as_str()).cloned() {
                        if mangled != *name && self.module_globals.contains(&mangled) {
                            return self.lower_expr(&AstNode::Var(mangled));
                        }
                    }
                }
                // PY-A V3: nonlocal names ALWAYS read through env (fresh
                // value), even when a local alias exists.
                if self.nonlocal_names.contains(name) {
                    let key_id = self.next_id();
                    self.exprs
                        .insert(key_id, MirExpr::StringLit(name.clone()));
                    self.type_map.insert(key_id, Type::Str);
                    let slot_id = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_env_get".to_string(),
                        args: vec![key_id],
                        dest: slot_id,
                        type_args: vec![],
                    });
                    self.exprs.insert(slot_id, MirExpr::Var(slot_id));
                    self.type_map.insert(slot_id, Type::I64);
                    return slot_id;
                }
                if let Some(&existing) = self.name_to_id.get(name) {
                    return existing;
                }
                // PY-A: module-global name not bound locally — env read
                // (implicit module global: no global declaration needed).
                if self.module_globals.contains(name) {
                    let key_id = self.next_id();
                    self.exprs
                        .insert(key_id, MirExpr::StringLit(name.clone()));
                    self.type_map.insert(key_id, Type::Str);
                    let slot_id = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_env_get".to_string(),
                        args: vec![key_id],
                        dest: slot_id,
                        type_args: vec![],
                    });
                    self.exprs.insert(slot_id, MirExpr::Var(slot_id));
                    // Keep the global's static type: an untyped env read made
                    // `q.put(x)` a bare call, because the handle tag was lost.
                    let ty = self
                        .module_global_types
                        .get(name)
                        .cloned()
                        .unwrap_or(Type::I64);
                    self.type_map.insert(slot_id, ty);
                    return slot_id;
                }
                // PY-A V3: nonlocal name not bound locally — env read.
                if self.nonlocal_names.contains(name) {
                    let key_id = self.next_id();
                    self.exprs
                        .insert(key_id, MirExpr::StringLit(name.clone()));
                    self.type_map.insert(key_id, Type::Str);
                    let slot_id = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_env_get".to_string(),
                        args: vec![key_id],
                        dest: slot_id,
                        type_args: vec![],
                    });
                    self.exprs.insert(slot_id, MirExpr::Var(slot_id));
                    self.type_map.insert(slot_id, Type::I64);
                    return slot_id;
                }

                // PY-A: a bare known function name used as a value (e.g.
                // `threading.Thread(work)`) becomes its address. Constants
                // share the signature table, so exclude them — they lower to
                // their value below, not to a symbol.
                if self.func_ret_types.contains_key(name)
                    && !self.global_consts.contains_key(name)
                {
                    self.exprs.insert(id, MirExpr::FuncAddr(name.clone()));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }

                // Unit-variant path of a registered enum (e.g. `Color::Green`)
                // lowers to its variant tag (integer discriminant).
                if name.contains("::") {
                    if let Some(tag) = self.enum_unit_variant_index(name) {
                        self.exprs.insert(id, MirExpr::IntLit(tag));
                        self.type_map.insert(id, Type::I64);
                        return id;
                    }
                }

                // Check if this is a global constant
                if let Some(const_val) = self.global_consts.get(name) {
                    match const_val {
                        crate::middle::ctfe::value::ConstValue::Int(n) => {
                            self.exprs.insert(id, MirExpr::IntLit(*n));
                            self.type_map.insert(id, Type::I64);
                            return id;
                        }
                        crate::middle::ctfe::value::ConstValue::String(s) => {
                            self.exprs
                                .insert(id, MirExpr::StringLit(s.clone()));
                            self.type_map.insert(id, Type::Str);
                            return id;
                        }
                        crate::middle::ctfe::value::ConstValue::Array(elements) => {
                            // For array constants, create a StackArray expression
                            let array_size = elements.len();
                            let mut element_ids = Vec::new();

                            // Extract integer values first to avoid borrow issues
                            let mut int_values = Vec::new();
                            for elem in elements {
                                match elem {
                                    crate::middle::ctfe::value::ConstValue::Int(n) => {
                                        int_values.push(*n);
                                    }
                                    _ => {
                                        // Fallback to regular variable if not simple int
                                        self.exprs.insert(id, MirExpr::Var(id));
                                        self.type_map.insert(id, Type::I64);
                                        return id;
                                    }
                                }
                            }

                            // Now create MIR expressions (can mutate self)
                            for n in int_values {
                                let elem_id = self.next_id();
                                self.exprs.insert(elem_id, MirExpr::IntLit(n));
                                self.type_map.insert(elem_id, Type::I64);
                                element_ids.push(elem_id);
                            }

                            self.exprs.insert(
                                id,
                                MirExpr::StackArray {
                                    elements: element_ids,
                                    size: array_size,
                                },
                            );
                            self.type_map.insert(
                                id,
                                Type::Array(
                                    Box::new(Type::I64),
                                    crate::middle::types::ArraySize::Literal(array_size),
                                ),
                            );
                            return id;
                        }
                        _ => {
                            // Fallback to regular variable
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map.insert(id, Type::I64);
                        }
                    }
                } else if name.ends_with("::MAX") || name == "true" || name == "false" {
                    // Built-in constants (u64::MAX, usize::MAX, etc. in expression context)
                    let val = if name.ends_with("::MAX") {
                        -1
                    } else if name == "true" {
                        1
                    } else {
                        0
                    };
                    self.exprs.insert(id, MirExpr::IntLit(val));
                    self.type_map.insert(id, Type::I64);
                } else {
                    // Regular variable
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                }
            }
            AstNode::Lit(n) => {
                self.exprs.insert(id, MirExpr::IntLit(*n));
                // Use I64 for integer literals to match codegen (everything is i64)
                // and resolver inference. I32 caused monomorphized function name
                // mismatches (typecheck_i32 vs actual i64 arguments).
                self.type_map.insert(id, Type::I64);
            }
            AstNode::Bool(b) => {
                // Convert bool to i64: true = 1, false = 0
                let value = if *b { 1 } else { 0 };
                self.exprs.insert(id, MirExpr::IntLit(value));
                self.type_map.insert(id, Type::Bool);
            }
            AstNode::StringLit(s) => {
                self.exprs.insert(id, MirExpr::StringLit(s.clone()));
                self.type_map.insert(id, Type::Str);
            }
            AstNode::FString(parts) => {
                // PY-A: every part must be a string handle — non-string
                // expressions go through a to_string_* dispatch.
                let mut part_ids: Vec<u32> = Vec::new();
                for p in parts {
                    let pid = self.lower_expr(p);
                    let pid = self.lower_to_string(pid);
                    part_ids.push(pid);
                }
                self.exprs.insert(id, MirExpr::FString(part_ids));
                self.type_map.insert(id, Type::Str);
            }
            AstNode::BinaryOp { op, left, right } => {
                let left_id = self.lower_expr(left);
                let right_id = self.lower_expr(right);
                let dest = self.next_id();

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
                    && (self.is_array_like(&left_id) || self.is_array_like(&right_id))
                {
                    // PY-A: list equality. `a == b` on two lists used to compile
                    // to a plain integer compare of the two HANDLES, so two
                    // equal-content lists always compared 0 — a silent wrong
                    // answer in every `if a == b` guard.
                    let elem_is_str = self
                        .array_elem_is_str(&left_id)
                        || self.array_elem_is_str(&right_id);
                    let flag = self.next_id();
                    self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                    self.type_map.insert(flag, Type::I64);
                    let eq_id = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "py_list_eq".to_string(),
                        args: vec![left_id, right_id, flag],
                        dest: eq_id,
                        type_args: vec![],
                    });
                    self.exprs.insert(eq_id, MirExpr::Var(eq_id));
                    self.type_map.insert(eq_id, Type::Bool);
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
                    self.exprs.insert(
                        dest,
                        MirExpr::BinaryOp {
                            op: op.clone(),
                            left: left_id,
                            right: right_id,
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
                    let elem = match self.type_map.get(&left_id).cloned() {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => *e,
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
                        (Some(Type::F32) | Some(Type::F64), _)
                        | (_, Some(Type::F32) | Some(Type::F64)) => {
                            if is_cmp {
                                Type::Bool
                            } else {
                                Type::F64
                            }
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

            AstNode::Loop { body } => {
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
                return result_id;
            }
            AstNode::If { cond, then, else_ } => {
                // If expression - generate control flow with destination
                let cond_id = self.lower_expr(cond);
                let dest_id = self.next_id();

                // Create destination for expression result
                self.exprs.insert(dest_id, MirExpr::Var(dest_id));
                self.type_map.insert(dest_id, Type::I64);

                // Helper function to process block
                fn process_block(
                    mir_gen: &mut MirGen,
                    block: &[AstNode],
                    dest: u32,
                ) -> Vec<MirStmt> {
                    if block.is_empty() {
                        // Empty block - assign 0
                        let zero_id = mir_gen.next_id_with_lit(0);
                        return vec![MirStmt::Assign {
                            lhs: dest,
                            rhs: zero_id,
                        }];
                    }

                    // Save current statements
                    let saved_stmts = std::mem::take(&mut mir_gen.stmts);

                    // Generate block statements
                    for s in block {
                        mir_gen.lower_ast(s);
                    }

                    // Take generated statements
                    let mut block_stmts = std::mem::take(&mut mir_gen.stmts);

                    // Restore original statements
                    mir_gen.stmts = saved_stmts;

                    // Capture last expression value if block produces value
                    if let Some(last_stmt) = block_stmts.last() {
                        match last_stmt {
                            MirStmt::Assign { lhs, .. } => {
                                // Block ends with assignment - use that value
                                block_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *lhs,
                                });
                            }
                            MirStmt::If {
                                dest: Some(if_dest),
                                ..
                            } => {
                                // Block ends with if expression - use its destination
                                block_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *if_dest,
                                });
                            }
                            MirStmt::Return { val } => {
                                // Block ends with return - can't assign to dest
                                // (function returns, dest unused)
                            }
                            MirStmt::Call {
                                dest: call_dest, ..
                            } => {
                                // Block ends with function call - use its result
                                block_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: *call_dest,
                                });
                            }
                            _ => {
                                // No value-producing statement - assign 0
                                let zero_id = mir_gen.next_id_with_lit(0);
                                block_stmts.push(MirStmt::Assign {
                                    lhs: dest,
                                    rhs: zero_id,
                                });
                            }
                        }
                    } else {
                        // Block has AST nodes but no MIR statements were generated
                        // (e.g. bare Var/Lit/BinaryOp expressions in statement position)
                        // Lower the last AST node as an expression to capture its value
                        if let Some(last_ast) = block.last() {
                            // PY-A: unwrap `ExprStmt` — lowering the WRAPPER node
                            // is not an expression lowering at all, so the value
                            // came back untyped (i64) and a string branch of a
                            // ternary was printed as a pointer.
                            let inner = match last_ast {
                                AstNode::ExprStmt { expr } => expr.as_ref(),
                                other => other,
                            };
                            let val_id = mir_gen.lower_expr(inner);
                            block_stmts.push(MirStmt::Assign {
                                lhs: dest,
                                rhs: val_id,
                            });
                        } else {
                            // Shouldn't reach here (empty block handled above), but fallback
                            let zero_id = mir_gen.next_id_with_lit(0);
                            block_stmts.push(MirStmt::Assign {
                                lhs: dest,
                                rhs: zero_id,
                            });
                        }
                    }

                    block_stmts
                }

                // Process then and else blocks
                let then_stmts = process_block(self, then, dest_id);
                let else_stmts = process_block(self, else_, dest_id);

                // PY-A: `dest_id` was typed i64 up front, so a conditional
                // expression whose branches yield STRINGS
                // (`"big" if a > 3 else "small"`) stored a string pointer in an
                // i64 slot — `print` then showed a number instead of the text.
                // Take the type from the branch values instead (they agree in
                // every real case; disagreement keeps the i64 fallback).
                let branch_ty = |stmts: &[MirStmt]| -> Option<Type> {
                    stmts.iter().rev().find_map(|s| match s {
                        MirStmt::Assign { rhs, .. } => self.type_map.get(rhs).cloned(),
                        _ => None,
                    })
                };
                if let Some(ty) = branch_ty(&then_stmts).or_else(|| branch_ty(&else_stmts)) {
                    if !matches!(ty, Type::I64)
                        || branch_ty(&else_stmts).map_or(true, |t| matches!(t, Type::I64))
                    {
                        self.type_map.insert(dest_id, ty);
                    }
                }

                // Create If statement with destination
                self.stmts.push(MirStmt::If {
                    cond: cond_id,
                    then: then_stmts,
                    else_: else_stmts,
                    dest: Some(dest_id),
                });

                return dest_id;
            }

            AstNode::DictLit { entries } => {
                let map_id = id;
                self.stmts.push(MirStmt::MapNew { dest: map_id });
                let mut key_ty = Type::I64;
                // The VALUE type is tracked as the map's second type argument, so
                // `a = m["code"]` keeps DynamicArray(Str) instead of degrading to
                // I64 — `a[0]` then did a map_get on a Vec handle (SEGV, t193).
                let mut val_ty = Type::I64;
                let mut first_key = true;
                let mut first_val = true;
                for (k, v) in entries {
                    // PY-A: `{**m, ...}` — merge m's entries into the literal.
                    if let AstNode::Call {
                        receiver: None,
                        method,
                        args: ka,
                        ..
                    } = k
                    {
                        if method == "zeta_dict_spread" && ka.len() == 1 {
                            let src_id = self.lower_expr(&ka[0]);
                            // Take the key kind from the source map so a
                            // spread-only literal (`{**a}`) is still
                            // string-keyed and `d.keys()` stays Vec<str>.
                            if first_key {
                                if let Some(Type::Named(n, params)) =
                                    self.type_map.get(&src_id).cloned()
                                {
                                    if n == "map" {
                                        if let Some(kt) = params.first() {
                                            key_ty = kt.clone();
                                            first_key = false;
                                        }
                                    }
                                }
                            }
                            let scratch = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_map_update".to_string(),
                                args: vec![map_id, src_id],
                                dest: scratch,
                                type_args: vec![],
                            });
                            continue;
                        }
                    }
                    let kid0 = self.lower_expr(k);
                    if first_key {
                        // Remember whether keys are strings: the map type carries
                        // the key kind so `d.keys()` is typed Vec<str>.
                        key_ty = self.type_map.get(&kid0).cloned().unwrap_or(Type::I64);
                        first_key = false;
                    }
                    let kid = self.lower_map_key(kid0);
                    let vid = self.lower_expr(v);
                    if first_val {
                        val_ty = self.type_map.get(&vid).cloned().unwrap_or(Type::I64);
                        first_val = false;
                    }
                    self.stmts.push(MirStmt::DictInsert {
                        map_id,
                        key_id: kid,
                        val_id: vid,
                    });
                }
                let key_ty = if matches!(key_ty, Type::Str) {
                    Type::Str
                } else {
                    Type::I64
                };
                self.exprs.insert(map_id, MirExpr::Var(map_id));
                self.type_map.insert(
                    map_id,
                    Type::Named("map".to_string(), vec![key_ty, val_ty]),
                );
                return map_id;
            }
            AstNode::Range {
                start,
                end,
                inclusive: _,
            } => {
                let start_id = self.lower_expr(start);
                let end_id = self.lower_expr(end);
                let dest = self.next_id();

                // Range expression for for loops
                self.exprs.insert(
                    dest,
                    MirExpr::Range {
                        start: start_id,
                        end: end_id,
                    },
                );
                self.type_map.insert(dest, Type::Range);
                return dest;
            }
            AstNode::Call {
                receiver,
                method,
                args,
                type_args,
                ..
            } => {
                // PY-A: `from <user module> import member` — the module's file
                // is loaded and lowered into THIS binary under
                // `<module>__<name>` symbols, so a free call to an imported
                // member must be routed there. Without this the call went out as
                // an unresolved external, which is why the in-repo LOCAL shim
                // (strategies/code/jq_shim.py) could never link.
                if receiver.is_none() {
                    if let Some((module, member)) = self.py_member_aliases.get(method).cloned() {
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
                            let qualified = format!("{}__{}", module.replace('.', "_"), member);
                            let defaults = self
                                .param_defaults
                                .get(&qualified)
                                .or_else(|| self.param_defaults.get(member.as_str()))
                                .cloned();
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
                                    .unwrap_or(Type::I64);
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

                // PY-A: `re.sub(pat, repl, s)` — repl may be a STRING or a
                // callable (`lambda m: ...`). The closure's parameter must be
                // typed as a Match so `m.group(0)` inside it dispatches.
                if let Some((m, mem)) = self.py_member_target(receiver, method) {
                    if m == "re" && mem == "sub" && args.len() == 3 {
                        let pat_id = self.lower_expr(&args[0]);
                        let callable_repl = matches!(
                            &args[1],
                            AstNode::Closure { .. }
                        ) || matches!(
                            &args[1],
                            AstNode::Var(n) if self.func_ret_types.contains_key(n.as_str())
                        );
                        let repl_id = {
                            if callable_repl {
                                self.re_repl_param = true;
                            }
                            let id_ = self.lower_expr(&args[1]);
                            self.re_repl_param = false;
                            id_
                        };
                        let s_id = self.lower_expr(&args[2]);
                        self.stmts.push(MirStmt::Call {
                            func: if callable_repl {
                                "py_re_sub_call".to_string()
                            } else {
                                "py_re_sub".to_string()
                            },
                            args: vec![pat_id, repl_id, s_id],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::Str);
                        return id;
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
                if method == "Counter" && args.len() == 1 {
                    let is_counter = match receiver {
                        None => self
                            .py_member_aliases
                            .get("Counter")
                            .map(|(m, mem)| m == "collections" && mem == "Counter")
                            .unwrap_or(false),
                        Some(_) => self
                            .py_member_target(receiver, method)
                            .map(|(m, mem)| m == "collections" && mem == "Counter")
                            .unwrap_or(false),
                    };
                    if is_counter {
                        let arg_id = self.lower_expr(&args[0]);
                        let elem_is_str = match self.type_map.get(&arg_id) {
                            Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                                matches!(**e, Type::Str)
                            }
                            _ => false,
                        };
                        if elem_is_str {
                            self.stmts.push(MirStmt::Call {
                                func: "py_collections_counter_new_str".to_string(),
                                args: vec![arg_id],
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            // String-keyed Counter → keys() is Vec<str>.
                            self.type_map.insert(
                                id,
                                Type::Named("map".to_string(), vec![Type::Str]),
                            );
                            return id;
                        }
                    }
                }
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
                // PY-A: `json.dumps(x)` needs the COMPILER's type — an i64
                // handle carries no runtime tag, so dispatch to the typed
                // entry point here instead of guessing in C.
                if let Some((m, mem)) = self.py_member_target(receiver, method) {
                    // `json.dumps(x, ensure_ascii=False, indent=2)` — the extra
                    // args are `__kwarg__` formatting hints. Before this they made
                    // `args.len() == 1` fail, the call fell through to the generic
                    // path and emitted a phantom arity-suffixed symbol
                    // (`py_json_dumps_i64_3`, 4 corpus call sites). Keep the first
                    // POSITIONAL value; say once that formatting kwargs are ignored
                    // (cosmetic only, and never silent).
                    let first_is_positional = !matches!(
                        args.first(),
                        Some(AstNode::Call { method: km, .. }) if km == "__kwarg__"
                    );
                    if m == "json" && mem == "dumps" && !args.is_empty() && first_is_positional {
                        if args.len() > 1 {
                            static WARNED_DUMPS: std::sync::OnceLock<()> = std::sync::OnceLock::new();
                            WARNED_DUMPS.get_or_init(|| {
                                eprintln!(
                                    "warning: PY-A: json.dumps formatting kwargs \
                                     (ensure_ascii/indent/…) are ignored"
                                );
                            });
                        }
                        let arg_id = self.lower_expr(&args[0]);
                        let ty = self.type_map.get(&arg_id).cloned().unwrap_or(Type::I64);
                        let sym = match &ty {
                            Type::Str => "py_json_dumps_str",
                            Type::F64 => "py_json_dumps_f64",
                            Type::Bool => "py_json_dumps_bool",
                            Type::Named(n, _) if n == "map" => "py_json_dumps_map",
                            // A Json value carries its own types: dump it
                            // recursively, no guessing and no warning.
                            Type::Named(n, _) if n == "PyJson" => "py_json_dump",
                            Type::DynamicArray(_) | Type::Array(_, _) => "py_json_dumps_vec",
                            _ => "py_json_dumps_i64",
                        };
                        // A list is homogeneous, so its element type decides
                        // the serializer — no side table needed.
                        let vec_elem_tag: Option<i64> = match &ty {
                            Type::DynamicArray(e) | Type::Array(e, _) => Some(match **e {
                                Type::F64 | Type::F32 => 1,
                                Type::Str => 2,
                                Type::Bool => 3,
                                _ => 0,
                            }),
                            _ => None,
                        };
                        if let Some(tag) = vec_elem_tag {
                            let tag_id = self.next_id();
                            self.exprs.insert(tag_id, MirExpr::IntLit(tag));
                            self.type_map.insert(tag_id, Type::I64);
                            self.stmts.push(MirStmt::Call {
                                func: "py_json_dumps_vec_typed".to_string(),
                                args: vec![arg_id, tag_id],
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map.insert(id, Type::Str);
                            return id;
                        }
                        // dict values now carry a type tag recorded at insert
                        // time, so no warning is needed for maps.

                        self.stmts.push(MirStmt::Call {
                            func: sym.to_string(),
                            args: vec![arg_id],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::Str);
                        return id;
                    }
                }
                // PY-A: `json.dump(obj, f)` — serialize the object with the
                // same type-driven serializer as json.dumps, then write it.
                if let Some((m, mem)) = self.py_member_target(receiver, method) {
                    if m == "json" && mem == "dump" && args.len() == 2 {
                        let obj_id = self.lower_expr(&args[0]);
                        let oty = self.type_map.get(&obj_id).cloned().unwrap_or(Type::I64);
                        let (sym, vec_tag): (&str, Option<i64>) = match &oty {
                            Type::Str => ("py_json_dumps_str", None),
                            Type::F64 => ("py_json_dumps_f64", None),
                            Type::Bool => ("py_json_dumps_bool", None),
                            Type::Named(n, _) if n == "map" => ("py_json_dumps_map", None),
                            Type::Named(n, _) if n == "PyJson" => ("py_json_dump", None),
                            Type::DynamicArray(e) | Type::Array(e, _) => (
                                "py_json_dumps_vec_typed",
                                Some(match **e {
                                    Type::F64 | Type::F32 => 1,
                                    Type::Str => 2,
                                    Type::Bool => 3,
                                    _ => 0,
                                }),
                            ),
                            _ => ("py_json_dumps_i64", None),
                        };
                        let text_id = self.next_id();
                        let mut cargs = vec![obj_id];
                        if let Some(tag) = vec_tag {
                            let tid = self.next_id();
                            self.exprs.insert(tid, MirExpr::IntLit(tag));
                            self.type_map.insert(tid, Type::I64);
                            cargs.push(tid);
                        }
                        self.stmts.push(MirStmt::Call {
                            func: sym.to_string(),
                            args: cargs,
                            dest: text_id,
                            type_args: vec![],
                        });
                        self.exprs.insert(text_id, MirExpr::Var(text_id));
                        self.type_map.insert(text_id, Type::Str);
                        let file_id = self.lower_expr(&args[1]);
                        self.stmts.push(MirStmt::Call {
                            func: "py_file_write".to_string(),
                            args: vec![file_id, text_id],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
                        return id;
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
                        if self.py_user_modules.contains(module) {
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
                            let packed = self.next_id();
                            let n = elem_ids.len();
                            self.exprs.insert(
                                packed,
                                MirExpr::StackArray {
                                    elements: elem_ids,
                                    size: n,
                                },
                            );
                            self.type_map.insert(packed, Type::Tuple(vec![Type::I64; n]));
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
                        self.stmts.push(MirStmt::Call {
                            func: "py_argparse_add".to_string(),
                            args: vec![recv, name_id, flag_id, dstr],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
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
                if method == "FileHandler" && args.len() > 1 {
                    if let Some((m, mem)) = self.py_member_target(receiver, method) {
                        if m == "logging" && mem == "FileHandler" {
                            static WARNED_FH: std::sync::OnceLock<()> = std::sync::OnceLock::new();
                            WARNED_FH.get_or_init(|| {
                                eprintln!(
                                    "warning: PY-A: logging.FileHandler mode/… is ignored \
                                     (local no-op shim)"
                                );
                            });
                            let path = self.lower_expr(&args[0]);
                            self.stmts.push(MirStmt::Call {
                                func: "py_logging_FileHandler".to_string(),
                                args: vec![path],
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map.insert(
                                id,
                                Type::Named("PyFileHandler".to_string(), vec![]),
                            );
                            return id;
                        }
                    }
                }
                // PY-A: `logging.getLogger()` — Python's name argument is
                // OPTIONAL, but the registry declares one required arg, so a
                // 0-arg call was arity-mangled into the phantom symbol
                // `py_logging_getLogger_0` (corpus: `logging.getLogger()`).
                // Pass an empty name: the runtime stub's logger identity IS its
                // name, and every method on it is a no-op shim locally.
                if args.is_empty() {
                    if let Some((m, mem)) = self.py_member_target(receiver, method) {
                        if m == "logging" && mem == "getLogger" {
                            let name_id = self.next_id();
                            self.exprs.insert(name_id, MirExpr::StringLit(String::new()));
                            self.type_map.insert(name_id, Type::Str);
                            self.stmts.push(MirStmt::Call {
                                func: "py_logging_getLogger".to_string(),
                                args: vec![name_id],
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map
                                .insert(id, Type::Named("PyLogger".to_string(), vec![]));
                            return id;
                        }
                    }
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
                    if std::env::var("ZETA_PROBE_CALL").is_ok() {
                        eprintln!("PROBE hit symbol={} handle={:?} ret={}", symbol, handle, ret);
                    }
                    let mut lowered = Vec::with_capacity(args.len());
                    for a in args {
                        lowered.push(self.lower_expr(a));
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
                    let declared_ret = self.func_ret_types.get(symbol).cloned();
                    self.type_map.insert(
                        id,
                        match (handle, ret) {
                            (Some(h), _) => Type::Named(h.to_string(), vec![]),
                            (None, r) => declared_ret.unwrap_or_else(|| match r {
                                "f64" => Type::F64,
                                "str" => Type::Str,
                                "vec" => Type::DynamicArray(Box::new(Type::I64)),
                                "vecstr" => Type::DynamicArray(Box::new(Type::Str)),
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
                                let n_lit = self.next_id_with_lit((args.len() - 1) as i64);
                                let mut vals: Vec<u32> = Vec::new();
                                for a in &args[1..] {
                                    vals.push(self.lower_expr(a));
                                }
                                while vals.len() < 4 {
                                    vals.push(self.next_id_with_lit(0));
                                }
                                self.stmts.push(MirStmt::Call {
                                    func: format!("py_logger_{}_n", method),
                                    args: vec![lg, fmt, n_lit, vals[0], vals[1], vals[2], vals[3]],
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
                if let Some(recv) = receiver {
                    if let Some(tag) = self.py_handle_of(recv) {
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
                    let join_dest = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "join".to_string(),
                        args: vec![handle_id],
                        dest: join_dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(join_dest, MirExpr::Var(join_dest));
                    self.type_map.insert(join_dest, Type::I64);
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

                // SPECIAL HANDLING: successor(it) — advance iterator by 1
                if method == "successor" && receiver.is_none() && args.len() == 1 {
                    let it_id = self.lower_expr(&args[0]);
                    let one_id = self.next_id();
                    self.exprs.insert(one_id, MirExpr::IntLit(1));
                    self.type_map.insert(one_id, Type::I64);
                    self.exprs.insert(
                        id,
                        MirExpr::BinaryOp {
                            op: "+".to_string(),
                            left: it_id,
                            right: one_id,
                        },
                    );
                    self.type_map.insert(id, Type::I64);
                    return id;
                }

                // SPECIAL HANDLING: predecessor(it) — advance iterator by -1
                if method == "predecessor" && receiver.is_none() && args.len() == 1 {
                    let it_id = self.lower_expr(&args[0]);
                    let one_id = self.next_id();
                    self.exprs.insert(one_id, MirExpr::IntLit(1));
                    self.type_map.insert(one_id, Type::I64);
                    self.exprs.insert(
                        id,
                        MirExpr::BinaryOp {
                            op: "-".to_string(),
                            left: it_id,
                            right: one_id,
                        },
                    );
                    self.type_map.insert(id, Type::I64);
                    return id;
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
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_platform_obj".to_string(),
                        args: vec![z, z, z, z],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
                // PY-A: `getattr(obj, "field")` with a LITERAL name is Python's
                // static attribute access — safe to rewrite to a field access
                // ONLY when the receiver's handle tag is statically known and the
                // member is registered (that check is what makes it safe: an
                // unknown receiver would silently read garbage, see batch 26).
                // Otherwise fall through to the compile-time diagnostic below.
                if method == "getattr" && receiver.is_none() && (2..=3).contains(&args.len()) {
                    if let AstNode::StringLit(name) = &args[1] {
                        // (a) a registry handle with a registered member.
                        if let Some(tag) = self.py_handle_of(&args[0]) {
                            if crate::middle::pylib::method_symbol(&tag, name).is_some() {
                                let rewritten = AstNode::FieldAccess {
                                    base: Box::new(args[0].clone()),
                                    field: name.clone(),
                                };
                                return self.lower_expr(&rewritten);
                            }
                        }
                        // (b) a struct-typed receiver (JoinQuant's `g`, a
                        // module global, a config object): the field exists ->
                        // plain field access; the field is absent but a default
                        // was given -> the default, which IS Python's semantics
                        // for a missing attribute. Absent with no default keeps
                        // the loud diagnostic below (Python would raise
                        // AttributeError, we must not silently read 0).
                        if let Some(tyname) = self.py_struct_type_of(&args[0]) {
                            if self.py_struct_has_field(&tyname, name) {
                                let rewritten = AstNode::FieldAccess {
                                    base: Box::new(args[0].clone()),
                                    field: name.clone(),
                                };
                                return self.lower_expr(&rewritten);
                            }
                            if args.len() == 3 {
                                return self.lower_expr(&args[2]);
                            }
                        }
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
                    self.stmts.push(MirStmt::Call {
                        func: "py_slice_new".to_string(),
                        args: vec![ids[0], ids[1], ids[2]],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
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
                // Unimplemented builtins that would otherwise emit a FREE CALL
                // named after themselves (an undefined symbol at link time, with
                // zero information about the cause). Ring the bell at COMPILE
                // time instead; the symbol is still emitted so behaviour is
                // unchanged, but the message names the culprit.
                if receiver.is_none()
                    && ((method == "getattr" && !args.is_empty())
                        || (method == "range" && !args.is_empty()))
                {
                    eprintln!(
                        "error: builtin `{}` is not implemented in this form (it would link \
                         against an undefined symbol named `{}`)",
                        method, method
                    );
                }
                if method == "len" && receiver.is_none() && args.len() == 1 {
                    let arg_id = self.lower_expr(&args[0]);
                    let arg_ty = self.type_map.get(&arg_id).cloned();
                    // 批次146 重放: `len(obj)` dispatches to the object's
                    // `__len__` method (t196: `len(F())`, `len(df)`).
                    if let Some(Type::Named(n, _)) = &arg_ty {
                        if let Some(qlen) = self.qualified_method_candidate(n, "__len__") {
                            self.stmts.push(MirStmt::Call {
                                func: qlen,
                                args: vec![arg_id],
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map.insert(id, Type::I64);
                            return id;
                        }
                    }
                    match arg_ty {
                        Some(Type::Array(_, ArraySize::Literal(n))) => {
                            self.exprs.insert(id, MirExpr::IntLit(n as i64));
                            self.type_map.insert(id, Type::I64);
                            return id;
                        }
                        Some(Type::Str) => {
                            self.stmts.push(MirStmt::Call {
                                func: "str_len".to_string(),
                                args: vec![arg_id],
                                dest: id,
                                type_args: vec![],
                            });
                        }
                        Some(Type::Named(n, _)) if n == "map" => {
                            // len(dict/Counter): count the used slots.
                            self.stmts.push(MirStmt::Call {
                                func: "zeta_map_len".to_string(),
                                args: vec![arg_id],
                                dest: id,
                                type_args: vec![],
                            });
                        }
                        Some(Type::Named(n, _)) if n == "PyJson" => {
                            // Json length by tag: array/object/string.
                            self.stmts.push(MirStmt::Call {
                                func: "py_json_len".to_string(),
                                args: vec![arg_id],
                                dest: id,
                                type_args: vec![],
                            });
                        }
                        Some(Type::DynamicArray(_)) => {
                            self.stmts.push(MirStmt::Call {
                                func: "vec_len".to_string(),
                                args: vec![arg_id],
                                dest: id,
                                type_args: vec![],
                            });
                        }
                        _ => {
                            self.stmts.push(MirStmt::Call {
                                func: "array_len".to_string(),
                                args: vec![arg_id],
                                dest: id,
                                type_args: vec![],
                            });
                        }
                    }
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }

                // PY-A: `Some(v)` / `Ok(v)` / `Err(e)` free-call form —
                // enum variant constructors without a path. Lower as Struct
                // with the value in field f0 (Option/Result runtime shape).
                if receiver.is_none()
                    && matches!(method.as_str(), "Some" | "Ok" | "Err")
                    && args.len() == 1
                {
                    let val_id = self.lower_expr(&args[0]);
                    self.exprs.insert(
                        id,
                        MirExpr::Struct {
                            variant: method.clone(),
                            fields: vec![("f0".to_string(), val_id)],
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

                // PY-A: `assert(cond, msg)` — on failure print msg and abort.
                // if cond == 0 { zeta_assert_fail(msg) }
                if method == "assert" && receiver.is_none() && args.len() >= 1 {
                    let cond_id = self.lower_expr(&args[0]);
                    let msg_id = if args.len() > 1 {
                        self.lower_expr(&args[1])
                    } else {
                        let m = self.next_id();
                        self.exprs
                            .insert(m, MirExpr::StringLit("assertion failed".to_string()));
                        self.type_map.insert(m, Type::Str);
                        m
                    };
                    let zero_id = self.next_id();
                    self.exprs.insert(zero_id, MirExpr::IntLit(0));
                    self.type_map.insert(zero_id, Type::I64);
                    let eq_id = self.next_id();
                    self.exprs.insert(
                        eq_id,
                        MirExpr::BinaryOp {
                            op: "==".to_string(),
                            left: cond_id,
                            right: zero_id,
                        },
                    );
                    self.type_map.insert(eq_id, Type::Bool);
                    self.stmts.push(MirStmt::If {
                        cond: eq_id,
                        then: vec![MirStmt::VoidCall {
                            func: "zeta_assert_fail".to_string(),
                            args: vec![msg_id],
                        }],
                        else_: vec![],
                        dest: None,
                    });
                    let unit_id = self.next_id();
                    self.exprs.insert(unit_id, MirExpr::IntLit(0));
                    self.type_map.insert(unit_id, Type::I64);
                    return unit_id;
                }

                // PY-A: Python builtins abs/min/max/sum — dispatch by type
                if receiver.is_none() && method == "abs" && args.len() == 1 {
                    let arg_id = self.lower_expr(&args[0]);
                    let f64_arg = matches!(
                        self.type_map.get(&arg_id),
                        Some(Type::F64) | Some(Type::F32)
                    );
                    // PY-A fix: f64 abs via the llvm.fabs.f64 intrinsic — a
                    // runtime extern with an i64 signature coerced the float
                    // bit pattern and returned garbage.
                    let func = if f64_arg { "llvm.fabs.f64" } else { "zeta_abs_i64" };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: vec![arg_id],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(
                        id,
                        if f64_arg { Type::F64 } else { Type::I64 },
                    );
                    return id;
                }
                if receiver.is_none()
                    && (method == "min" || method == "max")
                    && args.len() >= 2
                {
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
                            self.type_map.insert(dest, Type::I64);
                        }
                        acc = dest;
                    }
                    return id;
                }
                if method == "sum" && std::env::var("ZETA_PROBE").is_ok() {
                    eprintln!("PROBE sum seen, receiver_none={} args={}", receiver.is_none(), args.len());
                }
                if receiver.is_none() && method == "sum" && args.len() == 1 {
                    let arg_id = self.lower_expr(&args[0]);
                    let (func, extra) = match self.type_map.get(&arg_id).cloned() {
                        Some(Type::DynamicArray(_)) => ("zeta_sum_vec".to_string(), Vec::new()),
                        // 批次145: PyDynamic/未跟踪的实参（如未注解形参）在运行期
                        // 是动态 vec —— 此前落到 zeta_sum_n 静态路径打印 0（t224 f）。
                        Some(Type::PyDynamic) | None => {
                            ("zeta_sum_vec".to_string(), Vec::new())
                        }
                        Some(Type::Array(_, ArraySize::Literal(n))) => (
                            "zeta_sum_n".to_string(),
                            vec![{
                                let nid = self.next_id();
                                self.exprs.insert(nid, MirExpr::IntLit(n as i64));
                                self.type_map.insert(nid, Type::I64);
                                nid
                            }],
                        ),
                        _ => ("zeta_sum_n".to_string(), Vec::new()),
                    };
                    let mut call_args = vec![arg_id];
                    call_args.extend(extra);
                    self.stmts.push(MirStmt::Call {
                        func,
                        args: call_args,
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
                // 批次145 重放(批次123): builtin `getattr(obj, "name" [, default])`.
                // 已知 struct（或可从构造调用恢复类名）→ 改写为 FieldAccess，
                // 复用字段/零参方法分派与类型；字段不存在 → default；
                // 未跟踪接收者 → 有 default 用 default（一次告警）；
                // 无 default 或动态名 → py_getattr_dynamic 运行期响亮 abort。
                if receiver.is_none() && method == "getattr" && (args.len() == 2 || args.len() == 3)
                {
                    if let AstNode::StringLit(lit) = &args[1] {
                        let obj_id = self.lower_expr(&args[0]);
                        // 类名：接收者的 Named 类型，或构造调用的方法名
                        let class_of = |ty: Option<&Type>, node: &AstNode| -> Option<String> {
                            if let Some(Type::Named(n, _)) = ty {
                                if n != "map" && n != "dict" {
                                    return Some(n.clone());
                                }
                            }
                            if let AstNode::Call {
                                receiver: None,
                                method: m,
                                ..
                            } = node
                            {
                                return Some(m.clone());
                            }
                            None
                        };
                        let obj_ty = self.type_map.get(&obj_id).cloned();
                        let cls = class_of(obj_ty.as_ref(), &args[0]);
                        match cls {
                            Some(tn) => {
                                let field_exists = self.type_decls.get(&tn).and_then(|d| match d {
                                    TypeDecl::Struct { fields, .. } => fields
                                        .iter()
                                        .find(|(fname, _)| fname.as_str() == lit.as_str())
                                        .map(|(_, ft)| Type::from_string(ft)),
                                    _ => None,
                                });
                                if field_exists.is_some() || args.len() == 2 {
                                    let fa = AstNode::FieldAccess {
                                        base: Box::new(args[0].clone()),
                                        field: lit.clone(),
                                    };
                                    return self.lower_expr(&fa);
                                }
                                // 已知 struct 但字段不存在 → default
                                if args.len() == 3 {
                                    return self.lower_expr(&args[2]);
                                }
                            }
                            None => {
                                if args.len() == 3 {
                                    eprintln!(
                                        "warning: PY-A: getattr on untyped receiver uses the default for '{}'",
                                        lit
                                    );
                                    return self.lower_expr(&args[2]);
                                }
                            }
                        }
                        // 字面量名未命中（已知 struct 缺字段 / 未跟踪接收者）
                        // 且无 default → 故意保持幽灵路径（批次 123 红线，守 t225：
                        // 链接期未定义符号即编译失败，不静默读 0）。
                        // 不 return，落出本块即可。
                    } else {
                        // 动态名（非字面量）→ 响亮失败
                        let obj_id = self.lower_expr(&args[0]);
                        let name_id = self.lower_expr(&args[1]);
                        self.stmts.push(MirStmt::Call {
                            func: "py_getattr_dynamic".to_string(),
                            args: vec![obj_id, name_id],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
                        return id;
                    }
                }
                // 批次145 重放(批次123): builtin next(it[, default]) + opaque .next().
                if receiver.is_none() && method == "next" && !args.is_empty() {
                    if args.len() >= 2 {
                        eprintln!("warning: PY-A: next(it, default) returns the default (V1)");
                        return self.lower_expr(&args[1]);
                    }
                    let aid = self.lower_expr(&args[0]);
                    self.stmts.push(MirStmt::Call {
                        func: "py_builtin_next".to_string(),
                        args: vec![aid],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
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
                        self.stmts.push(MirStmt::Call {
                            func: "py_method_next".to_string(),
                            args: vec![sid],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
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
                // bare `max`/`min` extern → link failure). f64 elements live as
                // raw bit patterns, so those are warned about instead of
                // silently compared as integers.
                if receiver.is_none()
                    && (method == "max" || method == "min")
                    && args.len() == 1
                {
                    let a = self.lower_expr(&args[0]);
                    let elem_is_float = match self.type_map.get(&a) {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            matches!(**e, Type::F64 | Type::F32)
                        }
                        _ => false,
                    };
                    if elem_is_float {
                        eprintln!(
                            "warning: PY-A: `{}` over a float array compares raw bit \
                             patterns — pass integers, or compare explicitly",
                            method
                        );
                    }
                    let func = if method == "max" {
                        "py_builtin_max"
                    } else {
                        "py_builtin_min"
                    };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: vec![a],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
                // PY-A: isinstance(x, T) — the value's STATIC type decides.
                // Only the builtin names and the exact class name are claimed;
                // anything else falls through (loud) rather than guessing.
                if receiver.is_none() && method == "isinstance" && args.len() == 2 {
                    if let AstNode::Var(tn) = &args[1] {
                        let val_id = self.lower_expr(&args[0]);
                        let vt = self.type_map.get(&val_id).cloned();
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
                        Some(Type::Named(n, _)) if n == "map" => Some("zeta_map_len"),
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
                    self.stmts.push(MirStmt::Call {
                        func: "py_int_base".to_string(),
                        args: vec![a, b],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
                if method == "fromkeys" && (args.len() == 1 || args.len() == 2) {
                    let is_dict = receiver.is_none()
                        || matches!(receiver.as_deref(), Some(AstNode::Var(v)) if v == "dict");
                    if is_dict {
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
                        self.type_map.insert(
                            id,
                            Type::Named(
                                "map".to_string(),
                                vec![if keys_are_str { Type::Str } else { Type::I64 }],
                            ),
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
                        self.stmts.push(MirStmt::Call {
                            func: "py_round_i64".to_string(),
                            args: vec![x],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
                    } else {
                        let n = self.lower_expr(&args[1]);
                        self.stmts.push(MirStmt::Call {
                            func: "py_round_n".to_string(),
                            args: vec![x, n],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::F64);
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
                    self.stmts.push(MirStmt::Call {
                        func: "py_builtin_set".to_string(),
                        args: vec![a],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::DynamicArray(Box::new(elem)));
                    return id;
                }
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
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                    return id;
                }
                // PY-A: chr(n) / ord(s) / divmod(a, b) / dict() — previously
                // bare externs (link failure) or missing entirely.
                if receiver.is_none() && method == "chr" && args.len() == 1 {
                    let a = self.lower_expr(&args[0]);
                    self.stmts.push(MirStmt::Call {
                        func: "py_builtin_chr".to_string(),
                        args: vec![a],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                if receiver.is_none() && method == "ord" && args.len() == 1 {
                    let a = self.lower_expr(&args[0]);
                    self.stmts.push(MirStmt::Call {
                        func: "py_builtin_ord".to_string(),
                        args: vec![a],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
                // divmod(a, b) → (a / b, a % b) as a tuple, so
                // `q, r = divmod(a, b)` unpacks via the call-return path.
                if receiver.is_none() && method == "divmod" && args.len() == 2 {
                    let q = AstNode::BinaryOp {
                        op: "/".to_string(),
                        left: Box::new(args[0].clone()),
                        right: Box::new(args[1].clone()),
                    };
                    let r = AstNode::BinaryOp {
                        op: "%".to_string(),
                        left: Box::new(args[0].clone()),
                        right: Box::new(args[1].clone()),
                    };
                    return self.lower_expr(&AstNode::Tuple(vec![q, r]));
                }
                // dict() with no arguments is an empty map.
                if receiver.is_none() && method == "dict" && args.is_empty() {
                    return self.lower_expr(&AstNode::DictLit { entries: vec![] });
                }
                // `dict(m)` is a SHALLOW COPY in Python, not an alias: mutating
                // the result must not touch the source. Only done when the
                // argument is statically a map — an unknown argument keeps the
                // loud diagnostic (copying an unknown handle would corrupt data).
                if receiver.is_none() && method == "dict" && args.len() == 1 {
                    let src_id = self.lower_expr(&args[0]);
                    let is_map = matches!(
                        self.type_map.get(&src_id),
                        Some(Type::Named(n, _)) if n == "map" || n == "dict"
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
                        let sink = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "py_map_update".to_string(),
                            args: vec![fresh, src_id],
                            dest: sink,
                            type_args: vec![],
                        });
                        self.exprs.insert(sink, MirExpr::Var(sink));
                        self.type_map.insert(sink, Type::I64);
                        self.exprs.insert(id, MirExpr::Var(fresh));
                        self.type_map.insert(id, Type::Named("map".to_string(), vec![]));
                        return id;
                    }
                }
                // PY-A: `zip(a, b)` — a Vec of (a[i], b[i]) pairs, so
                // `for x, y in zip(a, b):` destructures. (Previously a bare
                // `zip` extern → link failure.)
                if receiver.is_none() && method == "zip" && args.len() == 2 {
                    let a = self.lower_expr(&args[0]);
                    let b = self.lower_expr(&args[1]);
                    self.stmts.push(MirStmt::Call {
                        func: "py_zip".to_string(),
                        args: vec![a, b],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(
                        id,
                        Type::DynamicArray(Box::new(Type::Tuple(vec![
                            Type::I64,
                            Type::I64,
                        ]))),
                    );
                    return id;
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
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Bool);
                    return id;
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
                        _ => "py_fmt_i64",
                    };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: vec![val_id, spec_id],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }

                // PY-A: Python builtins list/int/float/sorted + numpy subset
                // (arange/linspace/diff as free calls) — dispatch by type.
                if receiver.is_none() && args.len() <= 3 {
                    let argc = args.len();
                    let lowered_args: Option<Vec<u32>> = match method.as_str() {
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
                            self.exprs.insert(id, MirExpr::Var(nid));
                            self.type_map.insert(id, Type::F64);
                            return id;
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
                            self.exprs.insert(id, MirExpr::Var(nid));
                            self.type_map
                                .insert(id, Type::Named("PyFile".to_string(), vec![]));
                            return id;
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
                            if !understood {
                                None
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
                                self.type_map
                                    .insert(nid, Type::DynamicArray(Box::new(Type::I64)));
                                self.exprs.insert(id, MirExpr::Var(nid));
                                self.type_map
                                    .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                                return id;
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
                                    _ => {
                                        let nid = self.next_id();
                                        self.exprs.insert(nid, MirExpr::IntLit(-1));
                                        self.type_map.insert(nid, Type::I64);
                                        nid
                                    }
                                };
                                let nid = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: "py_sorted_vec_rev".to_string(),
                                    args: vec![a, len_id, rev_id],
                                    dest: nid,
                                    type_args: vec![],
                                });
                                self.exprs.insert(nid, MirExpr::Var(nid));
                                self.type_map
                                    .insert(nid, Type::DynamicArray(Box::new(Type::I64)));
                                self.exprs.insert(id, MirExpr::Var(nid));
                                self.type_map
                                    .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                                return id;
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
                            self.exprs.insert(id, MirExpr::Var(nid));
                            self.type_map.insert(id, Type::I64);
                            return nid;
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
                            self.exprs.insert(id, MirExpr::Var(nid));
                            self.type_map.insert(id, Type::I64);
                            return nid;
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
                            self.exprs.insert(id, MirExpr::Var(nid));
                            self.type_map.insert(id, Type::I64);
                            return nid;
                        }
                        _ => None,
                    };
                    if let Some(call_args) = lowered_args {
                        // PY-A: list(x) is a passthrough — handles are already
                        // array-like; forcing zeta_list breaks StackArray
                        // handles (no Vec header).
                        if method == "list" {
                            let src = call_args[0];
                            self.exprs.insert(id, MirExpr::Var(src));
                            if let Some(t) = self.type_map.get(&src).cloned() {
                                self.type_map.insert(id, t);
                            } else {
                                self.type_map.insert(id, Type::I64);
                            }
                            return id;
                        }
                        let func = match method.as_str() {
                            "sorted" => "zeta_sorted_vec_len",
                            _ => "zeta_int_i64",
                        };
                        self.stmts.push(MirStmt::Call {
                            func: func.to_string(),
                            args: call_args,
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(
                            id,
                            match method.as_str() {
                                "sorted" => Type::DynamicArray(Box::new(Type::I64)),
                                _ => Type::I64,
                            },
                        );
                        return id;
                    }
                }

                // PY-A: Python `str(x)` — convert any value to its string form
                if method == "str" && receiver.is_none() && args.len() == 1 {
                    let arg_id = self.lower_expr(&args[0]);
                    // A Json value knows its own type: stringify by tag.
                    if matches!(
                        self.type_map.get(&arg_id),
                        Some(Type::Named(n, _)) if n == "PyJson"
                    ) {
                        let nid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "py_json_as_str".to_string(),
                            args: vec![arg_id],
                            dest: nid,
                            type_args: vec![],
                        });
                        self.exprs.insert(nid, MirExpr::Var(nid));
                        self.type_map.insert(nid, Type::Str);
                        self.exprs.insert(id, MirExpr::Var(nid));
                        self.type_map.insert(id, Type::Str);
                        return id;
                    }
                    let nid = self.lower_to_string(arg_id);
                    self.exprs.insert(id, MirExpr::Var(nid));
                    let ty = self.type_map.get(&nid).cloned().unwrap_or(Type::Str);
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
                if method == "print" && receiver.is_none() && !args.is_empty() {
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
                    let mut arg_ids = vec![];
                    for a in &positional {
                        arg_ids.push(self.lower_expr(a));
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
                        // A Json value prints by its tag (scalars bare,
                        // containers as JSON text) — converting to a string
                        // first keeps it on the existing print path.
                        // A list prints by its (static) element type, Python
                        // repr differs only in spacing/quoting.
                        if let Some(tag) = match self.type_map.get(arg_id) {
                            Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                                Some(match **e {
                                    Type::F64 | Type::F32 => 1,
                                    Type::Str => 2,
                                    Type::Bool => 3,
                                    _ => 0,
                                })
                            }
                            _ => None,
                        } {
                            let sid = self.next_id();
                            let tid = self.next_id();
                            self.exprs.insert(tid, MirExpr::IntLit(tag));
                            self.type_map.insert(tid, Type::I64);
                            self.stmts.push(MirStmt::Call {
                                func: "py_json_dumps_vec_typed".to_string(),
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
                            let sid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_json_dumps_map".to_string(),
                                args: vec![*arg_id],
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
                        let printed_id = if matches!(
                            self.type_map.get(arg_id),
                            Some(Type::Named(name, _)) if name == "PyJson"
                        ) {
                            let sid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_json_repr".to_string(),
                                args: vec![*arg_id],
                                dest: sid,
                                type_args: vec![],
                            });
                            self.exprs.insert(sid, MirExpr::Var(sid));
                            self.type_map.insert(sid, Type::Str);
                            sid
                        } else if matches!(
                            self.type_map.get(arg_id),
                            Some(Type::Named(name, _)) if name == "PyMatch"
                        ) {
                            // Print the matched text rather than the raw handle.
                            let gid = self.next_id();
                            self.exprs.insert(gid, MirExpr::IntLit(0));
                            self.type_map.insert(gid, Type::I64);
                            let sid = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_re_group".to_string(),
                                args: vec![*arg_id, gid],
                                dest: sid,
                                type_args: vec![],
                            });
                            self.exprs.insert(sid, MirExpr::Var(sid));
                            self.type_map.insert(sid, Type::Str);
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
                            _ => {
                                if is_last { "println_i64" } else { "print_i64" }
                            }
                        };
                        self.stmts.push(MirStmt::VoidCall {
                            func: func.to_string(),
                            args: vec![printed_id],
                        });
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
                    return unit_id;
                }

                // SPECIAL HANDLING: println generates VoidCall not Call.
                // PY-4: dispatch a single argument by type — strings go to
                // println_str, floats to println_f64.
                if method.as_str() == "println" && receiver.is_none() {
                    let mut arg_ids = vec![];
                    for a in args {
                        arg_ids.push(self.lower_expr(a));
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
                    Some(self.type_map.get(&rid).cloned().unwrap_or(Type::I64))
                } else {
                    None
                };
                // PY-A: keyword arguments — bind by parameter name when the
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
                    let fill = |slots: &mut Vec<Option<AstNode>>,
                                params: &[String],
                                pos: Vec<AstNode>,
                                kw: Vec<(String, AstNode)>,
                                spread: &[AstNode]| {
                        for (i, a) in pos.into_iter().enumerate() {
                            if i < slots.len() {
                                slots[i] = Some(a);
                            } else {
                                slots.push(Some(a));
                            }
                        }
                        for (n, v) in kw {
                            match params.iter().position(|p| *p == n) {
                                Some(i) => slots[i] = Some(v),
                                None => slots.push(Some(v)),
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
                                fill(&mut slots, &params, pos, kw, &spread);
                                Self::warn_unbound(&callee_name, &params, &slots);
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
                                fill(&mut slots, &params, pos, kw, &[]);
                                Self::warn_unbound(&callee_name, &params, &slots);
                                slots.into_iter().flatten().collect()
                            }
                            // Method call whose callee DECLARES defaults.
                            // `func_param_names` / `param_defaults` are keyed
                            // by the bare method name and index `self` at 0,
                            // which the dispatch site prepends — so drop it
                            // here. Without this `a.reset_index()` left the
                            // argument unbound and codegen padded 0, i.e.
                            // drop=0 (=False) instead of the declared True
                            // (t229, wrong value with no diagnostic).
                            Some(params)
                                if method_defaults
                                    .as_ref()
                                    .map_or(false, |d| d.iter().any(|x| x.is_some())) =>
                            {
                                let params: Vec<String> =
                                    params.into_iter().skip(1).collect();
                                let mut slots: Vec<Option<AstNode>> =
                                    params.iter().map(|_| None).collect();
                                fill(&mut slots, &params, pos, kw, &[]);
                                if let Some(d) = &method_defaults {
                                    for (i, slot) in slots.iter_mut().enumerate() {
                                        if slot.is_none() {
                                            if let Some(Some(v)) = d.get(i + 1) {
                                                *slot = Some(v.clone());
                                            }
                                        }
                                    }
                                }
                                slots.into_iter().flatten().collect()
                            }
                            _ => args.clone(),
                        }
                    } else if receiver.is_none() {
                        match self.func_param_names.get(method.as_str()).cloned() {
                            Some(params) => {
                                let mut slots: Vec<Option<AstNode>> =
                                    params.iter().map(|_| None).collect();
                                fill(&mut slots, &params, pos, kw, &[]);
                                Self::warn_unbound(&callee_name, &params, &slots);
                                slots.into_iter().flatten().collect()
                            }
                            None => kw.into_iter().map(|(_, v)| v).chain(pos).collect(),
                        }
                    } else {
                        // Method/library call: names are the registry's business.
                        kw.into_iter().map(|(_, v)| v).chain(pos).collect()
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
                                _ => 0,
                            };
                            for i in 0..n {
                                let idx_id = self.next_id();
                                self.exprs.insert(idx_id, MirExpr::IntLit(i as i64));
                                self.type_map.insert(idx_id, Type::I64);
                                let elem = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: "array_get".to_string(),
                                    args: vec![arr_id, idx_id],
                                    dest: elem,
                                    type_args: vec![],
                                });
                                self.exprs.insert(elem, MirExpr::Var(elem));
                                self.type_map.insert(elem, Type::I64);
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
                    if matches!(method.as_str(), "__collect__" | "__collect_dict__") {
                        if let AstNode::Closure { .. } = a {
                            if let Some(recv_id) = arg_ids.first() {
                                let elem = match self.type_map.get(recv_id).cloned() {
                                    Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                                        (*e).clone()
                                    }
                                    _ => Type::I64,
                                };
                                self.pending_closure_param_types = Some(vec![elem]);
                            }
                        }
                    }
                    arg_ids.push(self.lower_expr(a));
                }
                // Never let a stale hint leak into an unrelated closure.
                self.pending_closure_param_types = None;

                // Batch 111: `df.drop("col")` — library expects lt(vec,str).
                // Wrap a lone Str arg into a 1-element StackArray.
                if method == "drop" && arg_ids.len() >= 2 {
                    let lab = arg_ids[arg_ids.len() - 1];
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
                        let last = arg_ids.len() - 1;
                        arg_ids[last] = arr;
                    }
                }

                // PY-A: `type(x)` — the static type is already known, so fold it
                // to the Python type NAME as a string (no runtime reflection, no
                // `_type` extern). `print(type(x))` / `type(x) is int`-style code
                // in the wild then works instead of failing to link.
                if method == "type" && receiver.is_none() && arg_ids.len() == 1 {
                    let tn = match self.type_map.get(&arg_ids[0]) {
                        Some(Type::Str) => "str",
                        Some(Type::F64) => "float",
                        Some(Type::Bool) => "bool",
                        Some(Type::I64) => "int",
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)) => "list",
                        Some(Type::Named(n, _)) if n == "map" => "dict",
                        Some(Type::Named(n, _)) if n == "PySlice" => "slice",
                        _ => "object",
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

                // PY-A: `x in container` membership — strings via
                // host_str_contains; other container kinds are a V1 limit
                // (emit 0 with a compile-time note).
                if method == "__contains__" {
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
                        .map_or(false, |t| matches!(t, Type::Named(n, _) if n == "map"))
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
                        let flag = self.next_id();
                        self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                        self.type_map.insert(flag, Type::I64);
                        self.stmts.push(MirStmt::Call {
                            func: "py_list_contains".to_string(),
                            args: vec![arg_ids[0], arg_ids[1], flag],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::Bool);
                        return id;
                    }
                    if is_map && arg_ids.len() == 2 {
                        // `key in dict` — DictGet(key) != 0 (V1: value 0 is
                        // indistinguishable from a missing key); DictGet keeps
                        // the same codegen path as d[key] subscripting.
                        let get_id = self.next_id();
                        let key_id = self.lower_map_key(arg_ids[1]);
                        self.stmts.push(MirStmt::DictGet {
                            map_id: arg_ids[0],
                            key_id,
                            dest: get_id,
                        });
                        self.exprs.insert(get_id, MirExpr::Var(get_id));
                        self.type_map.insert(get_id, Type::I64);
                        let zero_id = self.next_id();
                        self.exprs.insert(zero_id, MirExpr::IntLit(0));
                        self.type_map.insert(zero_id, Type::I64);
                        // i64 != via BinaryOp (the verified comparison path)
                        self.exprs.insert(
                            id,
                            MirExpr::BinaryOp {
                                op: "!=".to_string(),
                                left: get_id,
                                right: zero_id,
                            },
                        );
                        self.type_map.insert(id, Type::Bool);
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
                    .map_or(false, |t| matches!(t, Type::Named(n, _) if n == "map"))
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
                        // Key kind comes from the map type's args: a string-keyed
                        // map yields Vec<str> for keys() and Vec<(str, i64)> for
                        // most_common.
                        let key_ty = match receiver_ty.as_ref() {
                            Some(Type::Named(_, args))
                                if matches!(args.first(), Some(Type::Str)) =>
                            {
                                Type::Str
                            }
                            _ => Type::I64,
                        };
                        let out_ty = if method == "most_common" || method == "items" {
                            Type::DynamicArray(Box::new(Type::Tuple(vec![
                                key_ty,
                                Type::I64,
                            ])))
                        } else {
                            Type::DynamicArray(Box::new(key_ty))
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
                        self.stmts.push(MirStmt::Call {
                            func: "str_slice".to_string(),
                            args: vec![arg_ids[0], arg_ids[1], arg_ids[2], flag],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::Str);
                        return id;
                    }
                    let elem = match receiver_ty.as_ref() {
                        Some(Type::Array(e, _)) => (**e).clone(),
                        Some(Type::DynamicArray(e)) => (**e).clone(),
                        _ => Type::I64,
                    };
                    // Static-size arrays: replace the "to the end" sentinel
                    // (-1) with the known length (zeta_slice_vec reads the
                    // Vec header only for dynamic handles).
                    let mut args2 = arg_ids.clone();
                    // Normalize the omitted-end sentinel to -1, which is what
                    // zeta_slice_vec understands as "to the end".
                    if matches!(args.get(1), Some(AstNode::Lit(v)) if *v == i64::MIN) {
                        let neg = self.next_id();
                        self.exprs.insert(neg, MirExpr::IntLit(-1));
                        self.type_map.insert(neg, Type::I64);
                        args2[2] = neg;
                    }
                    if let Some(Type::Array(_, ArraySize::Literal(n))) =
                        receiver_ty.as_ref()
                    {
                        if matches!(
                            self.exprs.get(&args2[2]),
                            Some(MirExpr::IntLit(-1))
                        ) {
                            let n_id = self.next_id();
                            self.exprs
                                .insert(n_id, MirExpr::IntLit(*n as i64));
                            self.type_map.insert(n_id, Type::I64);
                            args2[2] = n_id;
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
                            "pop" => elem.clone().unwrap_or(Type::I64),
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
                if matches!(receiver_ty.as_ref(), Some(Type::PyDynamic)) {
                    const SKIP: &[&str] = &[
                        "get", "set", "keys", "values", "items", "clear", "pop",
                        "update", "append", "push", "len", "tolist",
                        "__contains__", "__getitem__", "__setitem__", "__delitem__",
                        "__len__", "__iter__", "__enter__", "__exit__",
                    ];
                    if !SKIP.contains(&method.as_str()) {
                        if let Some((_handle, symbol, ret_handle, ret)) =
                            crate::middle::pylib::method_by_unique_name(method)
                        {
                            self.stmts.push(MirStmt::Call {
                                func: symbol.to_string(),
                                args: arg_ids.clone(),
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map.insert(
                                id,
                                match ret_handle {
                                    Some(h) => Type::Named(h.to_string(), vec![]),
                                    None => match ret {
                                        "str" => Type::Str,
                                        "f64" => Type::F64,
                                        "vecstr" => {
                                            Type::DynamicArray(Box::new(Type::Str))
                                        }
                                        _ => Type::I64,
                                    },
                                },
                            );
                            return id;
                        }
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
                        ("sum", 1) => Some("zeta_sum_vec"),
                        ("unique", 1) => Some("zeta_vec_unique"),
                        _ => None,
                    };
                    if let Some(fname) = vfunc {
                        self.stmts.push(MirStmt::Call {
                            func: fname.to_string(),
                            args: vec![arg_ids[0]],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(
                            id,
                            match method.as_str() {
                                "unique" => Type::DynamicArray(Box::new(elem)),
                                _ => Type::I64,
                            },
                        );
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
                let opaque_fallback: Option<(&str, &str)> = if receiver.is_some()
                    && !struct_has_method
                    && receiver_ty.as_ref().map_or(true, |t| {
                        let is_str = matches!(t, Type::Str);
                        let is_map = matches!(t, Type::Named(n, _) if n == "map");
                        !(is_str || is_map)
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
                        ("fillna", _) | ("astype", _) | ("shift", _) | ("groupby", _)
                        | ("transform", _) | ("rank", _) | ("sort_values", _)
                        | ("rolling", _) | ("mean", _) | ("to_period", _)
                        | ("set_index", _) | ("items", _) => {
                            Some(("zeta_identity", "i64"))
                        }
                        // Unknown method on an untyped receiver: string runtime
                        // (carrying the method's result kind, so `.capitalize()`
                        // stays a string and `.split()` stays a list).
                        _ => str_fallback,
                    }
                } else {
                    None
                };
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
                    self.type_map
                        .insert(id, Type::Named("map".to_string(), vec![]));
                    return id;
                }
                // __pack_pair__(k, v) — the runtime keeps the pair as a 2-slot
                // heap pair, so string keys must be content-hashed first (the
                // same `lower_map_key` rule a dict literal uses); otherwise the
                // map would key by pointer and `d[k]` lookups would miss.
                // String VALUES are refused loudly further down instead of being
                // silently stored as a pointer-to-i64.
                if method == "__pack_pair__" && arg_ids.len() == 2 {
                    let k_ty = self.type_map.get(&arg_ids[0]).cloned().unwrap_or(Type::I64);
                    if matches!(k_ty, Type::Str) {
                        let hashed = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "map_str_key".to_string(),
                            args: vec![arg_ids[0]],
                            dest: hashed,
                            type_args: vec![],
                        });
                        self.exprs.insert(hashed, MirExpr::Var(hashed));
                        self.type_map.insert(hashed, Type::I64);
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
                            collected_elem.unwrap_or(Type::I64),
                        )),
                    );
                    return id;
                }

                // PY-A: dict methods — Python d.get(k) (missing key → 0)
                if method == "get"
                    && receiver_ty
                        .as_ref()
                        .map_or(false, |t| matches!(t, Type::Named(n, _) if n == "map"))
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
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
                // PY-A: d.get(k, default) — 3-arg form via runtime
                if method == "get"
                    && receiver_ty
                        .as_ref()
                        .map_or(false, |t| matches!(t, Type::Named(n, _) if n == "map"))
                    && arg_ids.len() == 3
                {
                    let key_id = self.lower_map_key(arg_ids[1]);
                    self.stmts.push(MirStmt::Call {
                        func: "map_get_default".to_string(),
                        args: vec![arg_ids[0], key_id, arg_ids[2]],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }

                // PY-A: dict update / pop / clear. `map` receivers are excluded
                // from the opaque fallback, so these previously fell through to
                // bare externs (`_update`/`_pop`/`_clear`) and failed to link.
                if receiver_ty
                    .as_ref()
                    .map_or(false, |t| matches!(t, Type::Named(n, _) if n == "map"))
                {
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
                        self.type_map.insert(id, Type::I64);
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
                    self.stmts.push(MirStmt::Call {
                        func: "vec_new".to_string(),
                        args: vec![cap_id],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::DynamicArray(Box::new(Type::I64)));
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
                                // Named fields (`"{a}".format(a=…)`) and index
                                // mismatches cannot be rewritten — say so here
                                // instead of leaving a bare `format` symbol for
                                // the linker to report.
                                static WARNED_FMT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
                                WARNED_FMT.get_or_init(|| {
                                    eprintln!(
                                        "error: str.format on a literal template could not be \
                                         rewritten (named fields / index out of range) — \
                                         the call will link against an undefined `format` symbol"
                                    );
                                });
                            }
                        }
                    }
                    let m = str_method_symbol(method.as_str());
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
                    if let Some((sym, ret_handle)) =
                        crate::middle::pylib::method_symbol(tn, method)
                    {
                        self.stmts.push(MirStmt::Call {
                            func: sym.to_string(),
                            args: arg_ids.clone(),
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        let ty = match ret_handle {
                            Some(h) => Type::Named(h.to_string(), vec![]),
                            None => match crate::middle::pylib::method_ret(tn, method) {
                                Some("str") => Type::Str,
                                Some("f64") => Type::F64,
                                Some("vecstr") => Type::DynamicArray(Box::new(Type::Str)),
                                _ => Type::I64,
                            },
                        };
                        self.type_map.insert(id, ty);
                        return id;
                    }
                }

                // Check if this is a method call on a dynamic array
                let (func, is_array_len, is_array_push) = if let Some(ref rty) = receiver_ty {
                    // Check if receiver is a dynamic array type
                    if let Type::DynamicArray(_) = rty {
                        // Map array methods to runtime functions
                        match method.as_str() {
                            "push" => ("array_push".to_string(), false, true),
                            "len" => ("array_len".to_string(), true, false),
                            // 批次145 重放: order-preserving dedup / sum on vecs
                            "unique" => ("zeta_vec_unique".to_string(), false, false),
                            "sum" => ("zeta_sum_vec".to_string(), false, false),
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
                        } else {
                            // For inherent methods, use plain method name.
                            (method.clone(), false, false)
                        }
                    }
                } else {
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
                                .unwrap_or(Type::I64)
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
                        for (i, aid) in arg_ids.iter().enumerate() {
                            let concrete = match self.type_map.get(aid) {
                                Some(Type::Str) => "str",
                                Some(Type::F64) => "f64",
                                Some(Type::F32) => "f32",
                                Some(Type::Bool) => "bool",
                                _ => continue,
                            };
                            if let Some((_, dt)) = decl_fields.get_mut(i) {
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
                        .unwrap_or(Type::I64);
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
                    || func.contains("::"));
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
                    let ret_ty = self
                        .func_ret_types
                        .get(base)
                        .cloned()
                        .unwrap_or(Type::I64);
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
                                if let Some((_, dt)) = decl_fields.get_mut(i) {
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
            }
            AstNode::Match { scrutinee, arms } => {
                // Lower the scrutinee expression
                let scrutinee_id = self.lower_expr(scrutinee);

                // Generate if-else chain for match arms
                let result_id = id;

                // We'll build the match as a series of if-else statements
                // Start from the last arm and work backwards
                let mut else_branch = Vec::new();

                for arm in arms.iter().rev() {
                    // Generate condition based on pattern
                    let cond_id = self.next_id();

                    match &*arm.pattern {
                        AstNode::Lit(pattern_value) => {
                            // For literal patterns, generate equality check
                            let pattern_id = self.next_id();
                            self.exprs.insert(pattern_id, MirExpr::IntLit(*pattern_value));
                            self.type_map.insert(pattern_id, Type::I64);

                            // Create equality comparison: scrutinee == pattern
                            // This creates a call to the "==" operator
                            self.stmts.push(MirStmt::Call {
                                func: "==".to_string(),
                                args: vec![scrutinee_id, pattern_id],
                                dest: cond_id,
                                type_args: vec![],
                            });
                            self.exprs.insert(cond_id, MirExpr::Var(cond_id));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                        AstNode::Var(var_name) if var_name == "_" => {
                            // Wildcard pattern - always true
                            self.exprs.insert(cond_id, MirExpr::IntLit(1));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                        AstNode::Ignore => {
                            // `_` wildcard pattern — always true
                            self.exprs.insert(cond_id, MirExpr::IntLit(1));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                        AstNode::Var(var_name) => {
                            // Check if this is an enum variant name like Option::None
                            if var_name == "Option::None" || var_name == "Result::Err" {
                                // For enum variant without data, check if it matches
                                let check_func = if var_name == "Option::None" {
                                    "option_is_some" // We'll invert this
                                } else if var_name == "Result::Err" {
                                    "host_result_is_ok" // We'll invert this
                                } else {
                                    // Should not happen
                                    self.exprs.insert(cond_id, MirExpr::IntLit(0));
                                    self.type_map.insert(cond_id, Type::Bool);
                                    continue;
                                };

                                // Call the runtime function to check the variant
                                let check_result_id = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: check_func.to_string(),
                                    args: vec![scrutinee_id],
                                    dest: check_result_id,
                                    type_args: vec![],
                                });
                                self.exprs
                                    .insert(check_result_id, MirExpr::Var(check_result_id));
                                self.type_map.insert(check_result_id, Type::Bool);

                                // Invert the check (None is not Some, Err is not Ok)
                                let inverted_id = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: "!".to_string(),
                                    args: vec![check_result_id],
                                    dest: inverted_id,
                                    type_args: vec![],
                                });
                                self.exprs.insert(inverted_id, MirExpr::Var(inverted_id));
                                self.type_map.insert(inverted_id, Type::Bool);

                                self.exprs.insert(cond_id, MirExpr::Var(inverted_id));
                                self.type_map.insert(cond_id, Type::Bool);
                            } else if let Some(tag) = self.enum_unit_variant_index(var_name) {
                                // User-defined enum unit-variant path pattern
                                // (e.g. `Color::Red`): equality against the tag.
                                let pattern_id = self.next_id();
                                self.exprs.insert(pattern_id, MirExpr::IntLit(tag));
                                self.type_map.insert(pattern_id, Type::I64);
                                self.stmts.push(MirStmt::Call {
                                    func: "==".to_string(),
                                    args: vec![scrutinee_id, pattern_id],
                                    dest: cond_id,
                                    type_args: vec![],
                                });
                                self.exprs.insert(cond_id, MirExpr::Var(cond_id));
                                self.type_map.insert(cond_id, Type::Bool);
                            } else if var_name == "_" {
                                // Wildcard pattern - always true
                                self.exprs.insert(cond_id, MirExpr::IntLit(1));
                                self.type_map.insert(cond_id, Type::Bool);
                            } else {
                                // Regular variable binding pattern - always matches
                                // Add binding to name_to_id so the arm body can reference it
                                self.name_to_id.insert(var_name.clone(), scrutinee_id);
                                self.exprs.insert(cond_id, MirExpr::IntLit(1));
                                self.type_map.insert(cond_id, Type::Bool);
                            }
                        }
                        AstNode::StructPattern {
                            variant,
                            fields,
                            rest: _,
                        } => {
                            // Handle enum variant patterns like Option::Some(x) or Result::Ok(val)
                            // Check if this is an enum variant pattern
                            if variant.starts_with("Option::") || variant.starts_with("Result::") {
                                // Generate condition to check the variant
                                let check_func = if variant == "Option::Some" {
                                    "option_is_some"
                                } else if variant == "Option::None" {
                                    // For None, we check if it's not Some
                                    "option_is_some" // We'll invert this below
                                } else if variant == "Result::Ok" {
                                    "host_result_is_ok"
                                } else if variant == "Result::Err" {
                                    // For Err, we check if it's not Ok
                                    "host_result_is_ok" // We'll invert this below
                                } else {
                                    // Unknown variant, treat as false
                                    self.exprs.insert(cond_id, MirExpr::IntLit(0));
                                    self.type_map.insert(cond_id, Type::Bool);
                                    continue;
                                };

                                // Call the runtime function to check the variant
                                let check_result_id = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: check_func.to_string(),
                                    args: vec![scrutinee_id],
                                    dest: check_result_id,
                                    type_args: vec![],
                                });
                                self.exprs
                                    .insert(check_result_id, MirExpr::Var(check_result_id));
                                self.type_map.insert(check_result_id, Type::Bool);

                                // For None and Err, we need to invert the check
                                let final_check_id =
                                    if variant == "Option::None" || variant == "Result::Err" {
                                        let inverted_id = self.next_id();
                                        self.stmts.push(MirStmt::Call {
                                            func: "!".to_string(),
                                            args: vec![check_result_id],
                                            dest: inverted_id,
                                            type_args: vec![],
                                        });
                                        self.exprs.insert(inverted_id, MirExpr::Var(inverted_id));
                                        self.type_map.insert(inverted_id, Type::Bool);
                                        inverted_id
                                    } else {
                                        check_result_id
                                    };

                                // Set up bindings for field patterns
                                for (_field_name, field_pattern) in fields {
                                    if let AstNode::Var(var_name) = field_pattern {
                                        // Extract the field value from the enum
                                        let field_id = self.next_id();
                                        let extract_func = if variant == "Option::Some" {
                                            "option_get_data"
                                        } else if variant == "Result::Ok"
                                            || variant == "Result::Err"
                                        {
                                            "host_result_get_data"
                                        } else {
                                            // No data to extract
                                            continue;
                                        };

                                        // Call runtime function to extract the data
                                        self.stmts.push(MirStmt::Call {
                                            func: extract_func.to_string(),
                                            args: vec![scrutinee_id],
                                            dest: field_id,
                                            type_args: vec![],
                                        });
                                        self.exprs.insert(field_id, MirExpr::Var(field_id));
                                        self.type_map.insert(field_id, Type::I64);
                                        self.name_to_id.insert(var_name.clone(), field_id);
                                    }
                                }

                                self.exprs.insert(cond_id, MirExpr::Var(final_check_id));
                                self.type_map.insert(cond_id, Type::Bool);
                            } else {
                                // For regular struct patterns, treat as always matching for now
                                // Set up bindings for the field patterns
                                for (_field_name, field_pattern) in fields {
                                    if let AstNode::Var(var_name) = field_pattern {
                                        // Create a placeholder ID for the field value
                                        let field_id = self.next_id();
                                        self.name_to_id.insert(var_name.clone(), field_id);
                                        self.exprs.insert(field_id, MirExpr::IntLit(0)); // Placeholder
                                        self.type_map.insert(field_id, Type::I64);
                                    }
                                }
                                self.exprs.insert(cond_id, MirExpr::IntLit(1));
                                self.type_map.insert(cond_id, Type::Bool);
                            }
                        }
                        AstNode::TypeAnnotatedPattern {
                            pattern: inner_pattern,
                            ty: _,
                        } => {
                            // For type-annotated patterns, extract the inner pattern
                            // The type checking should have been done by the type checker
                            match &**inner_pattern {
                                AstNode::Var(var_name) => {
                                    // Regular variable binding pattern - always matches
                                    // Add binding to name_to_id so the arm body can reference it
                                    self.name_to_id.insert(var_name.clone(), scrutinee_id);
                                    self.exprs.insert(cond_id, MirExpr::IntLit(1));
                                    self.type_map.insert(cond_id, Type::Bool);
                                }
                                _ => {
                                    // For other inner patterns, treat as always false for now
                                    self.exprs.insert(cond_id, MirExpr::IntLit(0));
                                    self.type_map.insert(cond_id, Type::Bool);
                                }
                            }
                        }
                        AstNode::OrPattern(patterns) => {
                            // Or pattern: any sub-pattern matches.
                            // Lower each sub-pattern and OR their conditions.
                            let mut or_cond = None;
                            for sub_pat in patterns {
                                let sub_pat_id = self.next_id();
                                // Lower the sub-pattern as a match condition on scrutinee.
                                // Re-use the scrutinee_id — sub-pattern checks reference it.
                                let sub_lit_id = self.next_id();
                                match sub_pat {
                                    AstNode::Lit(val) => {
                                        self.exprs.insert(sub_lit_id, MirExpr::IntLit(*val));
                                        self.type_map.insert(sub_lit_id, Type::I64);
                                        self.stmts.push(MirStmt::Call {
                                            func: "==".to_string(),
                                            args: vec![scrutinee_id, sub_lit_id],
                                            dest: sub_pat_id,
                                            type_args: vec![],
                                        });
                                    }
                                    AstNode::Var(name) if name == "_" => {
                                        // Wildcard always matches
                                        self.exprs.insert(sub_pat_id, MirExpr::IntLit(1));
                                        self.type_map.insert(sub_pat_id, Type::Bool);
                                    }
                                    _ => {
                                        // Fallback for other sub-patterns
                                        self.exprs.insert(sub_pat_id, MirExpr::IntLit(0));
                                        self.type_map.insert(sub_pat_id, Type::Bool);
                                    }
                                }
                                self.exprs.insert(sub_pat_id, MirExpr::Var(sub_pat_id));
                                self.type_map.insert(sub_pat_id, Type::Bool);

                                if let Some(prev) = or_cond {
                                    // OR the conditions: prev || sub_pat
                                    let or_result_id = self.next_id();
                                    self.stmts.push(MirStmt::Call {
                                        func: "||".to_string(),
                                        args: vec![prev, sub_pat_id],
                                        dest: or_result_id,
                                        type_args: vec![],
                                    });
                                    self.exprs.insert(or_result_id, MirExpr::Var(or_result_id));
                                    self.type_map.insert(or_result_id, Type::Bool);
                                    or_cond = Some(or_result_id);
                                } else {
                                    or_cond = Some(sub_pat_id);
                                }
                            }
                            let cond_val = or_cond.unwrap_or_else(|| {
                                let default = self.next_id();
                                self.exprs.insert(default, MirExpr::IntLit(1));
                                self.type_map.insert(default, Type::Bool);
                                default
                            });
                            self.exprs.insert(cond_id, MirExpr::Var(cond_val));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                        AstNode::BindPattern {
                            name,
                            pattern: inner,
                        } => {
                            // x @ pattern: bind name to scrutinee, then match inner pattern.
                            self.name_to_id.insert(name.clone(), scrutinee_id);
                            // Check the inner pattern
                            let inner_cond_id = self.next_id();
                            match &**inner {
                                AstNode::RangePattern {
                                    start,
                                    end,
                                    inclusive: _,
                                } => {
                                    // x @ start..=end: check x >= start && x <= end
                                    let ge_id = self.next_id();
                                    let le_id = self.next_id();
                                    let start_id = self.lower_expr(start);
                                    let end_id = self.lower_expr(end);
                                    self.stmts.push(MirStmt::Call {
                                        func: ">=".to_string(),
                                        args: vec![scrutinee_id, start_id],
                                        dest: ge_id,
                                        type_args: vec![],
                                    });
                                    self.stmts.push(MirStmt::Call {
                                        func: "<=".to_string(),
                                        args: vec![scrutinee_id, end_id],
                                        dest: le_id,
                                        type_args: vec![],
                                    });
                                    // AND them
                                    let and_id = self.next_id();
                                    self.stmts.push(MirStmt::Call {
                                        func: "&&".to_string(),
                                        args: vec![ge_id, le_id],
                                        dest: and_id,
                                        type_args: vec![],
                                    });
                                    self.exprs.insert(inner_cond_id, MirExpr::Var(and_id));
                                    self.type_map.insert(inner_cond_id, Type::Bool);
                                }
                                _ => {
                                    // Other inner patterns: match by lowering.
                                    let inner_id = self.lower_expr(inner);
                                    self.stmts.push(MirStmt::Call {
                                        func: "==".to_string(),
                                        args: vec![scrutinee_id, inner_id],
                                        dest: inner_cond_id,
                                        type_args: vec![],
                                    });
                                    self.exprs
                                        .insert(inner_cond_id, MirExpr::Var(inner_cond_id));
                                    self.type_map.insert(inner_cond_id, Type::Bool);
                                }
                            }
                            self.exprs.insert(cond_id, MirExpr::Var(inner_cond_id));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                        AstNode::RangePattern {
                            start,
                            end,
                            inclusive: _,
                        } => {
                            // Range pattern: scrutinee >= start && scrutinee <= end
                            let ge_id = self.next_id();
                            let le_id = self.next_id();
                            let start_id = self.lower_expr(start);
                            let end_id = self.lower_expr(end);
                            self.stmts.push(MirStmt::Call {
                                func: ">=".to_string(),
                                args: vec![scrutinee_id, start_id],
                                dest: ge_id,
                                type_args: vec![],
                            });
                            self.stmts.push(MirStmt::Call {
                                func: "<=".to_string(),
                                args: vec![scrutinee_id, end_id],
                                dest: le_id,
                                type_args: vec![],
                            });
                            let and_id = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "&&".to_string(),
                                args: vec![ge_id, le_id],
                                dest: and_id,
                                type_args: vec![],
                            });
                            self.exprs.insert(cond_id, MirExpr::Var(and_id));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                        AstNode::Tuple(elements) => {
                            // Tuple pattern: match each element (simplified: always true for now)
                            // In a full implementation, we'd destructure and match each element.
                            // For now, bind elements by index position.
                            for (i, elem) in elements.iter().enumerate() {
                                if let AstNode::Var(name) = elem {
                                    let field_id = self.next_id();
                                    self.name_to_id.insert(name.clone(), field_id);
                                    self.exprs.insert(field_id, MirExpr::IntLit(0));
                                    self.type_map.insert(field_id, Type::I64);
                                }
                            }
                            self.exprs.insert(cond_id, MirExpr::IntLit(1));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                        _ => {
                            // For now, treat other patterns as always false
                            self.exprs.insert(cond_id, MirExpr::IntLit(0));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                    }

                    // Handle guard clause if present
                    let final_cond_id = if let Some(ref guard) = arm.guard {
                        // Lower the guard expression
                        let guard_id = self.lower_expr(guard);

                        // Create AND condition: pattern_matches && guard_condition
                        let and_cond_id = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "&&".to_string(),
                            args: vec![cond_id, guard_id],
                            dest: and_cond_id,
                            type_args: vec![],
                        });
                        self.exprs.insert(and_cond_id, MirExpr::Var(and_cond_id));
                        self.type_map.insert(and_cond_id, Type::Bool);

                        and_cond_id
                    } else {
                        cond_id
                    };

                    // Now lower the arm body (after establishing pattern bindings)
                    // If the arm body is a `return` statement, emit a Return
                    // (lower_expr has no Return arm and would fabricate 0).
                    let then_branch = if let AstNode::Return(inner) = &*arm.body {
                        let ret_val = self.lower_expr(inner);
                        vec![MirStmt::Return { val: ret_val }]
                    } else {
                        let arm_body_id = self.lower_expr(&arm.body);
                        vec![MirStmt::Assign {
                            lhs: result_id,
                            rhs: arm_body_id,
                        }]
                    };

                    let if_stmt = MirStmt::If {
                        cond: final_cond_id,
                        then: then_branch,
                        else_: else_branch,
                        dest: None,
                    };

                    // For the next iteration, the current if becomes the else branch
                    else_branch = vec![if_stmt];
                }

                // The final else_branch contains the complete if-else chain
                // Add it to statements
                if !else_branch.is_empty() {
                    // The chain starts with the first arm's if statement
                    self.stmts.extend(else_branch);
                }

                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
            }
            AstNode::FieldAccess { base, field } => {
                // PY-A: argparse results namespace — `args.<flag>` is typed from
                // the kind recorded at `add_argument` (a static method table
                // cannot enumerate dynamic field names). An undeclared flag is
                // reported and lowered to 0 rather than silently defaulting.
                {
                    let probe_id = self.lower_expr(base);
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
                                self.exprs.insert(id, MirExpr::IntLit(0));
                                self.type_map.insert(id, Type::I64);
                                return id;
                            }
                        };
                        let name_id = self.next_id();
                        self.exprs
                            .insert(name_id, MirExpr::StringLit(field.clone()));
                        self.type_map.insert(name_id, Type::Str);
                        self.stmts.push(MirStmt::Call {
                            func: func.to_string(),
                            args: vec![probe_id, name_id],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, ty);
                        return id;
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
                                    dest: id,
                                    type_args: vec![],
                                });
                                self.exprs.insert(id, MirExpr::Var(id));
                                let ty = match ret_handle {
                                    Some(h) => Type::Named(h.to_string(), vec![]),
                                    None => match crate::middle::pylib::method_ret(&tag, field)
                                        .unwrap_or("i64")
                                    {
                                        "str" => Type::Str,
                                        "f64" => Type::F64,
                                        _ => Type::I64,
                                    },
                                };
                                self.type_map.insert(id, ty);
                                return id;
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
                if let Some((root, parts)) = Self::flatten_module_receiver(expr) {
                    if let Some(module) = self.py_module_aliases.get(&root).cloned() {
                        // User module namespace read: `mod.CONST` is the module
                        // global `mod__CONST` in the env (written by the
                        // module's init), so read it back through the env.
                        if self.py_user_modules.contains(&module) && parts.len() == 1 {
                            let key = format!("{}{}", module.replace('.', "_") + "__", parts[0]);
                            let key_id = self.next_id();
                            self.exprs
                                .insert(key_id, MirExpr::StringLit(key));
                            self.type_map.insert(key_id, Type::Str);
                            self.stmts.push(MirStmt::Call {
                                func: "zeta_env_get".to_string(),
                                args: vec![key_id],
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map.insert(id, Type::I64);
                            return id;
                        }
                        let member = parts.join(".");
                        match crate::middle::pylib::find_member(&module, &member) {
                            Some(entry) if entry.args.is_empty() => {
                                let sym = entry.symbol.as_str();
                                self.stmts.push(MirStmt::Call {
                                    func: sym.to_string(),
                                    args: vec![],
                                    dest: id,
                                    type_args: vec![],
                                });
                                self.exprs.insert(id, MirExpr::Var(id));
                                self.type_map.insert(
                                    id,
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
                                        "vecjson" => Type::DynamicArray(Box::new(
                                            Type::Named("PyJson".to_string(), vec![]),
                                        )),
                                        "vecmatch" => Type::DynamicArray(Box::new(
                                            Type::Named("PyMatch".to_string(), vec![]),
                                        )),
                                        _ => Type::I64,
                                    },
                                );
                                return id;
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
                                self.exprs.insert(id, MirExpr::IntLit(0));
                                self.type_map.insert(id, Type::I64);
                                return id;
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
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        // A handle-returning attribute (Path.parent) keeps its
                        // tag so the next `.name`/`.parent` still dispatches.
                        let ty = match ret_handle {
                            Some(h) => Type::Named(h.to_string(), vec![]),
                            None => match crate::middle::pylib::method_ret(&tag, field)
                                .unwrap_or("i64")
                            {
                                "str" => Type::Str,
                                "f64" => Type::F64,
                                _ => Type::I64,
                            },
                        };
                        self.type_map.insert(id, ty);
                        return id;
                    }
                }
                // PY-A: zero-argument method read via FieldAccess on a KNOWN
                // struct — `df.columns` (no parens) must dispatch to
                // `DataFrame::columns`, not read a raw slot. Without this the
                // field read returned the `data` map handle and
                // `len(df.columns)` did array_len on a map → printed 0.
                if let Some(Type::Named(tn, _)) = self.type_map.get(&base_id).cloned() {
                    let qualified = format!("{}::{}", tn, field);
                    if let Some(ret_ty) = self.func_ret_types.get(&qualified).cloned() {
                        self.stmts.push(MirStmt::Call {
                            func: qualified,
                            args: vec![base_id],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, ret_ty);
                        return id;
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
                        self.exprs.insert(id, MirExpr::Var(base_id));
                        self.type_map.insert(id, Type::DynamicArray(elem));
                        return id;
                    }
                    Some(Type::Array(elem, _)) => {
                        self.exprs.insert(id, MirExpr::Var(base_id));
                        self.type_map
                            .insert(id, Type::DynamicArray(elem));
                        return id;
                    }
                    _ => {}
                }
                // 2. Create FieldAccess expression
                self.exprs.insert(
                    id,
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
                    cands.iter().find_map(|t| match decls.get(t) {
                        Some(TypeDecl::Struct { fields, .. }) => fields
                            .iter()
                            .find(|(x, _)| x == f)
                            .map(|(_, ft)| Type::from_string(ft)),
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
                        _ => None,
                    },
                };
                self.type_map.insert(id, field_ty.unwrap_or(Type::I64));
            }
            AstNode::StructLit { variant, fields } => {
                // Implement proper struct literal creation
                let mut field_ids = Vec::new();
                for (field_name, field_expr) in fields {
                    // Evaluate each field expression
                    let field_id = self.lower_expr(field_expr);
                    field_ids.push((field_name.clone(), field_id));
                }
                // Create Struct expression
                self.exprs.insert(
                    id,
                    MirExpr::Struct {
                        variant: variant.clone(),
                        fields: field_ids,
                    },
                );
                // For now, assume struct type is a generic type
                // TODO: Need proper type inference for struct literals
                self.type_map
                    .insert(id, Type::Named("Struct".to_string(), vec![]));
            }
            AstNode::PathCall {
                path,
                method,
                args,
                type_args,
            } => {
                // Construct qualified name: path::method
                let func_name = if path.is_empty() {
                    method.clone()
                } else {
                    format!("{}::{}", path.join("::"), method)
                };

                // If this is a path-qualified call with an uppercase method name
                // (e.g., AstNode::Lit(42)), it's an enum variant constructor —
                // emit Struct instead of Call. Lowercase method names like
                // `LLVMCodegen::new("bench")` are regular (static) function calls.
                // Type_args also indicate a generic function call.
                let is_upper = !method.is_empty()
                    && method
                        .chars()
                        .next()
                        .map(|c| c.is_uppercase())
                        .unwrap_or(false);
                // PY-A: `std::time::now()` → monotonic_ns runtime
                if path.len() == 2 && path[0] == "std" && path[1] == "time"
                    && method == "now" && args.is_empty()
                {
                    self.stmts.push(MirStmt::Call {
                        func: "monotonic_ns".to_string(),
                        args: vec![],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }

                // PY-A: `std::quantum::*::new` — V1 placeholder platform objects
                if path.len() >= 2 && path[0] == "std" && path[1] == "quantum"
                    && method == "new" && args.len() <= 2
                {
                    let n_id = if args.is_empty() {
                        let z = self.next_id();
                        self.exprs.insert(z, MirExpr::IntLit(1));
                        self.type_map.insert(z, Type::I64);
                        z
                    } else {
                        self.lower_expr(&args[0])
                    };
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_qc_new".to_string(),
                        args: vec![n_id],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
                // `std::quantum::QubitState::one/zero` etc.
                if path.len() >= 2 && path[0] == "std" && path[1] == "quantum"
                    && matches!(
                        method.as_str(),
                        "one" | "zero" | "plus" | "minus" | "conj" | "norm" | "abs"
                    )
                    && args.len() <= 1
                {
                    let z = self.next_id();
                    self.exprs.insert(z, MirExpr::IntLit(1));
                    self.type_map.insert(z, Type::I64);
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_qc_is_normalized".to_string(),
                        args: vec![z],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }

                // PY-A: `std::memory::capability::new(n)` — capability handle
                if path.len() == 3 && path[0] == "std" && path[1] == "memory"
                    && path[2] == "capability" && method == "new" && args.len() == 1
                {
                    let n_id = self.lower_expr(&args[0]);
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_dynarray_new".to_string(),
                        args: vec![n_id],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }

                // PY-A: platform class constructors (FixedSlippage(0.001),
                // OrderCost(...), MarketOrderStyle(...), etc.) → opaque handle.
                // method=="new" excluded — Vec::new()/DynArray::new() have
                // dedicated intercepts below.
                if path.len() == 1 && args.len() <= 8 && method != "new" {
                    let class_id = self.next_id();
                    self.exprs
                        .insert(class_id, MirExpr::StringLit(method.clone()));
                    self.type_map.insert(class_id, Type::Str);
                    let mut call_args = vec![class_id];
                    let mut arg_ids2 = vec![];
                    for a in args {
                        arg_ids2.push(self.lower_expr(a));
                    }
                    call_args.extend(arg_ids2);
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

                // PY-A: `memory::BitArray::new(n)` / `memory::Sieve::new(n)` /
                // `memory::DynamicArray::new(n)` — opaque handle allocation.
                if path.len() == 2 && path[0] == "memory" && method == "new"
                    && args.len() == 1
                {
                    let n_id = self.lower_expr(&args[0]);
                    let alloc = match path[1].as_str() {
                        "BitArray" => "zeta_bitarray_new",
                        "Sieve" => "zeta_sieve_new",
                        _ => "zeta_dynarray_new",
                    };
                    self.stmts.push(MirStmt::Call {
                        func: alloc.to_string(),
                        args: vec![n_id],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }

                // PY-A: `Vec::new()` → runtime vec_new with initial capacity
                // (vec_push growth doubles from cap, so cap must be > 0).
                if path.len() == 1 && path[0] == "Vec" && method == "new" && args.is_empty() {
                    let cap_id = self.next_id();
                    self.exprs.insert(cap_id, MirExpr::IntLit(8));
                    self.type_map.insert(cap_id, Type::I64);
                    self.stmts.push(MirStmt::Call {
                        func: "vec_new".to_string(),
                        args: vec![cap_id],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                    return id;
                }
                // PY-A: platform class constructors (FixedSlippage(0.001),
                // OrderCost(...), MarketOrderStyle(...), MACD(...)) — these
                // are single-segment capitalized calls from Python sources.
                // Route them to the opaque platform-object runtime rather
                // than an enum Struct (which has no runtime symbol).
                if path.len() == 1 && type_args.is_empty() && is_upper {
                    let class_id = self.next_id();
                    self.exprs
                        .insert(class_id, MirExpr::StringLit(method.clone()));
                    self.type_map.insert(class_id, Type::Str);
                    let mut call_args = vec![class_id];
                    let mut arg_ids2 = vec![];
                    for a in args {
                        arg_ids2.push(self.lower_expr(a));
                    }
                    call_args.extend(arg_ids2);
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
                if !path.is_empty() && type_args.is_empty() && is_upper {
                    // Regular function call or unqualified call
                    // Generate argument IDs
                    // PY-A: starred args `f(*arr)` expand to per-element args
                    // (V1: static-size arrays only — compile-time unrolling).
                    let mut arg_ids: Vec<u32> = Vec::new();
                    for a in args {
                        if let AstNode::UnaryOp { op, expr } = a {
                            if op == "*" {
                                let arr_id = self.lower_expr(expr);
                                let n = match self.type_map.get(&arr_id).cloned() {
                                    Some(Type::Array(_, ArraySize::Literal(n))) => n,
                                    _ => 0,
                                };
                                for i in 0..n {
                                    let idx_id = self.next_id();
                                    self.exprs.insert(idx_id, MirExpr::IntLit(i as i64));
                                    self.type_map.insert(idx_id, Type::I64);
                                    let elem = self.next_id();
                                    self.stmts.push(MirStmt::Call {
                                        func: "array_get".to_string(),
                                        args: vec![arr_id, idx_id],
                                        dest: elem,
                                        type_args: vec![],
                                    });
                                    self.exprs.insert(elem, MirExpr::Var(elem));
                                    self.type_map.insert(elem, Type::I64);
                                    arg_ids.push(elem);
                                }
                                continue;
                            }
                        }
                        arg_ids.push(self.lower_expr(a));
                    }

                    // Convert type arguments from strings to Type objects
                    let mir_type_args: Vec<Type> =
                        type_args.iter().map(|t| Type::from_string(t)).collect();

                    // PY-A: zeta_* runtime-dispatched names must stay bare —
                    // the arity suffix would make the call miss the runtime
                    // symbol (get_or_declare strips it, but the emitted call
                    // still references the suffixed name directly).
                    let call_name = if func_name.starts_with("zeta_")
                        || func_name.contains("__")
                    {
                        func_name.clone()
                    } else {
                        format!("{}_{}", func_name, arg_ids.len())
                    };
                    self.stmts.push(MirStmt::Call {
                        func: call_name,
                        args: arg_ids,
                        dest: id,
                        type_args: mir_type_args,
                    });

                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                }
            }
            AstNode::Cast { expr, ty } => {
                let expr_id = self.lower_expr(expr);
                let target_type = Type::from_string(ty);
                self.type_map.insert(id, target_type.clone());
                // Create As expression node
                self.exprs.insert(
                    id,
                    MirExpr::As {
                        expr: expr_id,
                        target_type,
                    },
                );
            }
            AstNode::ArrayLit(elements) => {
                // Create an array using ArrayHeader API

                let size = elements.len();

                // Python's `[]` is a GROWABLE list, never a 0-length fixed array.
                // As a `StackArray` of size 0 it had no `[cap|len]` header, so
                // `xs.append(v)`'s `vec_push` read the header 16 bytes BEFORE the
                // alloca (garbage) and wrote past the buffer — stack corruption.
                // That is the UB behind the pandas cluster's Bus errors / SEGVs
                // (`idx: lt(vec, str) = []` + `idx.append(str(i))` in
                // pylib/pandas.z) and behind t207's `len(xs)` staying 0.
                if size == 0 {
                    let capacity_id = self.next_id();
                    self.exprs.insert(capacity_id, MirExpr::IntLit(0));
                    self.type_map.insert(capacity_id, Type::I64);
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_dynarray_new".to_string(),
                        args: vec![capacity_id],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                    return id;
                }

                // HYBRID MEMORY SYSTEM: Check if this should be a stack array
                // For small, fixed-size arrays, use stack allocation
                if size <= 20000 {
                    // Reasonable stack size limit

                    // Lower each element expression
                    let mut element_ids = Vec::new();
                    for element in elements {
                        let elem_id = self.lower_expr(element);
                        element_ids.push(elem_id);
                    }

                    // Clone element_ids before moving it
                    let element_ids_clone = element_ids.clone();

                    // Create StackArray expression
                    self.exprs.insert(
                        id,
                        MirExpr::StackArray {
                            elements: element_ids,
                            size,
                        },
                    );

                    // Determine element type from elements
                    let elem_type = self.get_common_element_type(&element_ids_clone);

                    // Set the type to Array(elem_type, size) for subscript access
                    self.type_map.insert(
                        id,
                        Type::Array(Box::new(elem_type), ArraySize::Literal(size)),
                    );
                } else {
                    // Large array, use heap allocation

                    // Call array_new with capacity = size
                    let array_data_ptr = self.next_id();
                    let capacity_id = self.next_id();
                    self.exprs.insert(capacity_id, MirExpr::IntLit(size as i64));
                    self.stmts.push(MirStmt::Call {
                        func: "array_new".to_string(),
                        args: vec![capacity_id],
                        dest: array_data_ptr,
                        type_args: vec![],
                    });

                    // For heap arrays, we need to set the length
                    let len_id = self.next_id();
                    self.exprs.insert(len_id, MirExpr::IntLit(size as i64));
                    self.stmts.push(MirStmt::VoidCall {
                        func: "array_set_len".to_string(),
                        args: vec![array_data_ptr, len_id],
                    });

                    // Set each element at its index and collect element IDs
                    let mut heap_element_ids = Vec::new();
                    for (i, element) in elements.iter().enumerate() {
                        let elem_id = self.lower_expr(element);
                        heap_element_ids.push(elem_id);
                        let index_id = self.next_id();
                        self.exprs.insert(index_id, MirExpr::IntLit(i as i64));
                        self.stmts.push(MirStmt::VoidCall {
                            func: "array_set".to_string(),
                            args: vec![array_data_ptr, index_id, elem_id],
                        });
                    }

                    // Clone heap_element_ids before using it
                    let heap_element_ids_clone = heap_element_ids.clone();

                    // Return the data pointer (after header)
                    self.exprs.insert(id, MirExpr::Var(array_data_ptr));
                    // Determine element type from elements
                    let elem_type = self.get_common_element_type(&heap_element_ids_clone);
                    // Set the type to Array(elem_type, size) for subscript access
                    self.type_map.insert(
                        id,
                        Type::Array(Box::new(elem_type), ArraySize::Literal(size)),
                    );
                }
            }
            AstNode::ArrayRepeat { value, size } => {
                let value_id = self.lower_expr(value);

                // Get the type of the value expression
                let elem_type = self.type_map.get(&value_id).cloned().unwrap_or(Type::I64);

                // Check if size is a literal by examining the AST node directly
                // We need to pattern match on the boxed value
                match size.as_ref() {
                    AstNode::Lit(size_lit) => {
                        let size_val = *size_lit as usize;

                        // HYBRID MEMORY SYSTEM: Use StackArray for small fixed-size arrays
                        if size_val <= 20000 {
                            // Reasonable stack size limit

                            // Create StackArray expression with repeated value
                            self.exprs.insert(
                                id,
                                MirExpr::StackArray {
                                    elements: vec![value_id; size_val],
                                    size: size_val,
                                },
                            );

                            // Set the type to Array(elem_type, size) for subscript access
                            self.type_map.insert(
                                id,
                                Type::Array(Box::new(elem_type), ArraySize::Literal(size_val)),
                            );
                        } else {
                            // Large array, use heap allocation

                            // Allocate array using array_new with capacity = size
                            let array_ptr = self.next_id();
                            let capacity_id = self.next_id();
                            self.exprs
                                .insert(capacity_id, MirExpr::IntLit(size_val as i64));
                            self.stmts.push(MirStmt::Call {
                                func: "array_new".to_string(),
                                args: vec![capacity_id],
                                dest: array_ptr,
                                type_args: vec![],
                            });

                            // Set array length first
                            let len_id = self.next_id();
                            self.exprs.insert(len_id, MirExpr::IntLit(size_val as i64));
                            self.stmts.push(MirStmt::VoidCall {
                                func: "array_set_len".to_string(),
                                args: vec![array_ptr, len_id],
                            });

                            // Fill array with value
                            // Use memset intrinsic for zero initialization (performance optimization)
                            let val_expr = value_id;
                            let is_lit_zero = match self.exprs.get(&val_expr) {
                                Some(MirExpr::IntLit(0)) => true,
                                _ => false,
                            };
                            if is_lit_zero && size_val > 4 {
                                // Zero initialization: use memset for efficiency
                                let byte_size_id = self.next_id();
                                let elem_byte_size = match &elem_type {
                                    Type::I8 | Type::U8 | Type::Bool => 1,
                                    Type::I16 | Type::U16 => 2,
                                    Type::I32 | Type::U32 | Type::F32 => 4,
                                    Type::I64 | Type::U64 | Type::F64 | Type::Usize => 8,
                                    _ => 8, // Default to 8 bytes for complex types
                                };
                                self.exprs.insert(
                                    byte_size_id,
                                    MirExpr::IntLit((size_val * elem_byte_size) as i64),
                                );
                                self.stmts.push(MirStmt::VoidCall {
                                    func: "__builtin_memset".to_string(),
                                    args: vec![array_ptr, value_id, byte_size_id],
                                });
                            } else {
                                // Non-zero or small array: use per-element assignment
                                for idx in 0..size_val {
                                    let idx_id = self.next_id();
                                    self.exprs.insert(idx_id, MirExpr::IntLit(idx as i64));
                                    self.stmts.push(MirStmt::VoidCall {
                                        func: "array_set".to_string(),
                                        args: vec![array_ptr, idx_id, value_id],
                                    });
                                }
                            }

                            // Return the array pointer
                            self.exprs.insert(id, MirExpr::Var(array_ptr));
                            // Set the type to Array(elem_type, size) for subscript access
                            self.type_map.insert(
                                id,
                                Type::Array(Box::new(elem_type), ArraySize::Literal(size_val)),
                            );
                        }
                    }
                    _ => {
                        // Size is not a literal constant
                        // For now, create a placeholder
                        self.exprs.insert(id, MirExpr::IntLit(0));
                        self.type_map.insert(id, Type::I64);
                    }
                }
            }
            AstNode::Subscript { base, index } => {
                // 批次148 重放: `np.where(mask)[0]` — numpy 的 where 返回元组，
                // [0] 取第一个数组。V1：where 调用已返回索引 Vec，[0] 即其本身
                // （否则 array_get 取出首索引，随后的 len(idx) 对标量求长度 SEGV）。
                if let AstNode::Call { receiver, method, .. } = &**base {
                    if method == "where"
                        && matches!(**index, AstNode::Lit(0))
                        && receiver.as_ref().map_or(false, |r| matches!(**r, AstNode::Var(_)))
                    {
                        return self.lower_expr(base);
                    }
                }
                let bid = self.lower_expr(base);
                let base_ty_pre = self.type_map.get(&bid).cloned().unwrap_or(Type::I64);
                // PY-A: negative index `arr[-k]` → `arr[n-k]` for arrays with a
                // compile-time-known size (Python semantics).
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
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
                let iid = self.lower_expr(&index);

                // Check if base is an array type (dynamic or static)
                let base_ty = self.type_map.get(&bid).cloned().unwrap_or(Type::I64);
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
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map
                            .insert(id, Type::Named("PyJson".to_string(), vec![]));
                        return id;
                    }
                }
                if let Type::Str = base_ty {
                    // Python `s[i]` on a string yields a 1-character string.
                    self.stmts.push(MirStmt::Call {
                        func: "str_get".to_string(),
                        args: vec![bid, iid],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                // 批次145 重放: subscript on a KNOWN struct with `__getitem__`
                // (`df["c"]`) must dispatch the qualified method — previously it
                // fell through to DictGet on the struct pointer (map_get on a
                // non-map → garbage/SEGV). Batch 99's fix, lost in the 143
                // restore.
                if let Type::Named(tn, _) = &base_ty {
                    if tn != "map" && tn != "dict" {
                        if let Some(qualified) =
                            self.qualified_method_candidate(tn, "__getitem__")
                        {
                            let ret_ty = self.func_ret_types.get(&qualified).cloned();
                            self.stmts.push(MirStmt::Call {
                                func: qualified,
                                args: vec![bid, iid],
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map.insert(
                                id,
                                ret_ty.unwrap_or(Type::I64),
                            );
                            return id;
                        }
                    }
                }
                // A MAP subscript as an EXPRESSION. The assignment path
                // (`d[k] = v`) already handled maps, but the expression path did
                // not: `self.d["a"]` fell through to the array branches and read
                // the map handle as an array — a SEGFAULT, not a wrong value.
                if matches!(&base_ty, Type::Named(n, _) if n == "map" || n == "dict") {
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
                        dest: id,
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    // `m[k]` keeps the map's VALUE type (second type argument of
                    // `map<K, V>`), so a Vec-valued map stays indexable.
                    let val_ty = match &base_ty {
                        Type::Named(_, targs) => {
                            targs.get(1).cloned().unwrap_or(Type::I64)
                        }
                        _ => Type::I64,
                    };
                    self.type_map.insert(id, val_ty);
                    return id;
                }
                if let Type::DynamicArray(_) = base_ty {
                    // Generate array_get call for dynamic arrays
                    self.stmts.push(MirStmt::Call {
                        func: "array_get".to_string(),
                        args: vec![bid, iid],
                        dest: id,
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
                                dest: id,
                                type_args: vec![],
                            });
                        }
                        _ => {
                            // Dynamic or large array - use heap array access
                            self.stmts.push(MirStmt::Call {
                                func: "array_get".to_string(),
                                args: vec![bid, iid],
                                dest: id,
                                type_args: vec![],
                            });
                        }
                    }
                } else if is_array_param {
                    // Param is an array type even if type_map doesn't know it yet
                    self.stmts.push(MirStmt::Call {
                        func: "array_get".to_string(),
                        args: vec![bid, iid],
                        dest: id,
                        type_args: vec![],
                    });
                } else {
                    // Use DictGet for other types (maps/dicts) — string keys
                    // are content-hashed (see lower_map_key)
                    let key_id = self.lower_map_key(iid);
                    self.stmts.push(MirStmt::DictGet {
                        map_id: bid,
                        key_id,
                        dest: id,
                    });
                }
                self.exprs.insert(id, MirExpr::Var(id));
                // Element type: from the base array's element type if known,
                // otherwise default to i64. Without this, `f64arr[i]` is typed
                // i64 and later casts read raw double bits.
                // Check type_map first (covers annotated locals like `let x: [f64; 4]`),
                // fall back to source_types (function params).
                let elem_type = {
                    let from_ty = match &base_ty_clone {
                        Type::DynamicArray(elem) => Some((**elem).clone()),
                        Type::Array(elem, _) => Some((**elem).clone()),
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
                self.type_map.insert(id, elem_type);
            }
            AstNode::DynamicArrayLit {
                elem_type,
                elements,
            } => {
                // `[dynamic]T{}` must use the [cap|len|data] layout, because
                // every vec_* runtime call (push/len/get) reads that header.
                // `array_new` allocates a HEADERLESS buffer (its 1-arg form
                // takes a byte count), so pushes used to write beside it and
                // len() read 0 — `arr.push(x)` in a loop left the array empty.
                let array_ptr = self.next_id();
                let capacity_id = self.next_id();
                self.exprs
                    .insert(capacity_id, MirExpr::IntLit(elements.len() as i64));
                self.stmts.push(MirStmt::Call {
                    func: "zeta_dynarray_new".to_string(),
                    args: vec![capacity_id],
                    dest: array_ptr,
                    type_args: vec![],
                });
                self.exprs.insert(array_ptr, MirExpr::Var(array_ptr));
                let array_type = Type::DynamicArray(Box::new(Type::from_string(elem_type)));
                self.type_map.insert(array_ptr, array_type.clone());

                // Push each element to the array
                for element in elements {
                    let elem_id = self.lower_expr(element);
                    let void_dest = self.next_id(); // push returns void
                    self.stmts.push(MirStmt::Call {
                        func: "array_push".to_string(),
                        args: vec![array_ptr, elem_id],
                        dest: void_dest,
                        type_args: vec![],
                    });
                }

                // Return the array pointer (use array_ptr as the result)
                self.exprs.insert(id, MirExpr::Var(array_ptr));
                self.type_map.insert(id, array_type);
                return array_ptr; // Return the array pointer ID, not a new ID
            }
            AstNode::UnaryOp { op, expr } => {
                // Handle unary operators like ! (not)
                // PY-A: Python `not` lowers identically to `!`
                let op: &str = if op == "not" { "!" } else { op };
                let expr_id = self.lower_expr(expr);
                let dest = self.next_id();

                // PY-A fix: negative float literals fold at MIR — the
                // i64 unary_minus path corrupts their bit pattern (→ NaN).
                if op == "-" {
                    if let AstNode::FloatLit(v) = &**expr {
                        self.exprs.insert(dest, MirExpr::FloatLit(-v.parse::<f64>().unwrap_or(0.0)));
                        self.type_map.insert(dest, Type::F64);
                        return dest;
                    }
                }
                if op == "&mut" || op == "&" {
                    // Address-of: pass the variable's location, not its value.
                    // For a simple variable, use its alloca address (ptrtoint in codegen).
                    if let AstNode::Var(name) = &**expr {
                        if let Some(&addr_alloca) = self.name_to_id.get(name.as_str()) {
                            self.exprs.insert(dest, MirExpr::AddrOf { alloca_id: addr_alloca });
                            self.type_map.insert(dest, Type::I64);
                            return dest;
                        }
                    }
                    // Non-variable operand: fall through to passing its value
                    // (rvalues have no address; the callee mutation is discarded).
                    self.exprs.insert(dest, MirExpr::Var(expr_id));
                    self.type_map.insert(dest, Type::I64);
                    return dest;
                }
                if op == "!" {
                    // Logical NOT operator
                    let stmt = MirStmt::Call {
                        func: "!".to_string(),
                        args: vec![expr_id],
                        dest,
                        type_args: vec![],
                    };
                    self.stmts.push(stmt);
                } else if op == "*" {
                    // Pointer dereference - use Deref MirExpr with pointee width
                    let pointee_width = self.pointee_widths.get(&expr_id).copied().unwrap_or(8);
                    self.exprs.insert(
                        dest,
                        MirExpr::Deref {
                            addr_id: expr_id,
                            pointee_width,
                        },
                    );
                    self.type_map.insert(dest, Type::I64);
                    // SKIP the common Var(dest) insert below — the Deref expr is used inline
                    // by gen_expr_safe, so we don't need an alloca to load from.
                    return dest;
                } else if op == "-" {
                    // PY-A fix: floating-point operands must NOT go through
                    // the i64 unary_minus runtime (bit-pattern negation →
                    // NaN for negatives like -2.5). 0 - x via BinaryOp keeps
                    // the float type; ints keep the dedicated symbol.
                    let is_float = matches!(
                        self.type_map.get(&expr_id),
                        Some(Type::F64) | Some(Type::F32)
                    );
                    if is_float {
                        let zero_id = self.next_id();
                        self.exprs.insert(zero_id, MirExpr::FloatLit(0.0));
                        self.type_map.insert(zero_id, Type::F64);
                        self.exprs.insert(
                            dest,
                            MirExpr::BinaryOp {
                                op: "-".to_string(),
                                left: zero_id,
                                right: expr_id,
                            },
                        );
                    } else {
                        // Unary minus - use special function name to avoid conflict with binary minus
                        self.stmts.push(MirStmt::Call {
                            func: "unary_minus".to_string(),
                            args: vec![expr_id],
                            dest,
                            type_args: vec![],
                        });
                    }
                } else {
                    // Other unary operators (unary plus?, etc.)
                    let stmt = MirStmt::Call {
                        func: op.to_string(),
                        args: vec![expr_id],
                        dest,
                        type_args: vec![],
                    };
                    self.stmts.push(stmt);
                }

                self.exprs.insert(dest, MirExpr::Var(dest));
                // Preserve the operand's type — unary_minus on f64 should still be f64.
                // The old code hardcoded Type::I64, which broke float casts like `b as i64`.
                let op_ty = self.type_map.get(&expr_id).cloned().unwrap_or(Type::I64);
                self.type_map.insert(dest, op_ty);
                return dest;
            }
            AstNode::Unsafe { body } => {
                // Evaluate the last expression in an unsafe block as the result
                if let Some(last) = body.last()
                    && let AstNode::ExprStmt { expr } = last
                {
                    return self.lower_expr(expr);
                }
                // Fallback: evaluate the whole body as statements
                for stmt in body {
                    self.lower_ast(stmt);
                }
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
            }
            // ── Priority B: Pattern Expression Nodes ──
            AstNode::Tuple(elements) => {
                // Tuple expression: lower each element, create stack array.
                let mut element_ids = Vec::new();
                for elem in elements {
                    let elem_id = self.lower_expr(elem);
                    element_ids.push(elem_id);
                }
                let size = element_ids.len();
                self.exprs.insert(
                    id,
                    MirExpr::StackArray {
                        elements: element_ids.clone(),
                        size,
                    },
                );
                self.type_map.insert(id, Type::Tuple(vec![Type::I64; size]));
            }
            AstNode::Ignore => {
                // Wildcard / ignore expression.
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
            }
            AstNode::BindPattern { name, pattern } => {
                // Binding pattern: x @ pattern — bind name to inner value.
                let inner_id = self.lower_expr(pattern);
                self.name_to_id.insert(name.clone(), inner_id);
                self.exprs.insert(id, MirExpr::Var(inner_id));
                if let Some(ty) = self.type_map.get(&inner_id) {
                    self.type_map.insert(id, ty.clone());
                } else {
                    self.type_map.insert(id, Type::I64);
                }
            }
            AstNode::RangePattern {
                start,
                end,
                inclusive: _,
            } => {
                // Range pattern: lower start/end for comparison.
                let start_id = self.lower_expr(start);
                let end_id = self.lower_expr(end);
                self.exprs.insert(
                    id,
                    MirExpr::Range {
                        start: start_id,
                        end: end_id,
                    },
                );
                self.type_map.insert(id, Type::Range);
            }
            AstNode::OrPattern(patterns) => {
                // Or pattern: evaluate the first alternative.
                if let Some(first) = patterns.first() {
                    return self.lower_expr(first);
                }
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
            }
            AstNode::StructPattern {
                variant, fields, ..
            } => {
                // Struct pattern in expression position — create struct value.
                let mut field_ids = Vec::new();
                for (field_name, field_expr) in fields {
                    let field_id = self.lower_expr(field_expr);
                    field_ids.push((field_name.clone(), field_id));
                }
                self.exprs.insert(
                    id,
                    MirExpr::Struct {
                        variant: variant.clone(),
                        fields: field_ids,
                    },
                );
                self.type_map
                    .insert(id, Type::Named(variant.clone(), vec![]));
            }
            // ── Priority D & E: Remaining Expression Nodes ──
            AstNode::Closure { params, body, .. } => {
                // PY-A: lambda/closure → emitted as a standalone synthetic
                // function `__closure_<N>`; the expression value is the
                // function address (V1: non-capturing only — the body may
                // reference its own params; free-variable captures fall back
                // to the existing no-op stub behaviour, noted in the
                // lower_closure docs).
                let closure_name = self.lower_closure(params, body);
                // PY-A V2a: value-capture — snapshot each free variable into
                // the closure env at creation time (reads see the snapshot).
                {
                    let mut bound: std::collections::HashSet<String> =
                        params.iter().cloned().collect();
                    let mut free: std::collections::BTreeSet<String> =
                        std::collections::BTreeSet::new();
                    Self::collect_free_vars(body, &mut bound, &mut free);
                    for name in free.iter() {
                        if let Some(&cur_id) = self.name_to_id.get(name) {
                            let key_id = self.next_id();
                            self.exprs
                                .insert(key_id, MirExpr::StringLit(name.clone()));
                            self.type_map.insert(key_id, Type::Str);
                            self.stmts.push(MirStmt::VoidCall {
                                func: "zeta_env_set".to_string(),
                                args: vec![key_id, cur_id],
                            });
                        }
                    }
                }
                if let Some(v) = self.pending_closure_binding.take() {
                    self.closure_vars.insert(v.clone(), closure_name.clone());
                }
                if let Some(t) = self.last_closure_ret_ty.clone() {
                    self.closure_ret_tys.insert(closure_name.clone(), t);
                }
                let addr_id = self.next_id();
                self.exprs.insert(addr_id, MirExpr::FuncAddr(closure_name));
                self.type_map.insert(addr_id, Type::I64);
                return addr_id;
            }
            AstNode::Defer(body) => {
                // Defer expression: evaluate and return the inner expression.
                return self.lower_expr(body);
            }
            AstNode::Await(body) => {
                // Await expression: poll the sub-future until ready, then extract value.
                // Generates:
                //   let __fut = <body>;           // create sub-future
                //   while true {
                //       let __pr = future_poll(__fut);
                //       if __pr != 0 {               // Ready
                //           result = future_result(__fut);
                //           break;
                //       }
                //   }
                //   return result;
                let fut_id = self.lower_expr(body);
                // Create IDs
                let pr_id = self.next_id();
                let result_id = self.next_id();
                let zero_id = self.next_id_with_lit(0);

                // Store the sub-future pointer
                let stored_fut = self.next_id();
                self.exprs.insert(stored_fut, MirExpr::Var(stored_fut));
                self.type_map.insert(stored_fut, Type::I64);
                self.stmts.push(MirStmt::Assign {
                    lhs: stored_fut,
                    rhs: fut_id,
                });

                // poll result
                self.exprs.insert(pr_id, MirExpr::Var(pr_id));
                self.type_map.insert(pr_id, Type::I64);

                // result
                self.exprs.insert(result_id, MirExpr::Var(result_id));
                self.type_map.insert(result_id, Type::I64);

                // While loop body
                let mut body_stmts = vec![];

                // pr = future_poll(stored_fut)
                body_stmts.push(MirStmt::Call {
                    func: "future_poll".to_string(),
                    args: vec![stored_fut],
                    dest: pr_id,
                    type_args: vec![],
                });

                // if pr != 0 { result = future_result(stored_fut); break; }
                let mut then_stmts = vec![];
                then_stmts.push(MirStmt::Call {
                    func: "future_result".to_string(),
                    args: vec![stored_fut],
                    dest: result_id,
                    type_args: vec![],
                });
                then_stmts.push(MirStmt::Break);

                let cond_id = self.next_id();
                self.exprs.insert(
                    cond_id,
                    MirExpr::BinaryOp {
                        op: "!=".to_string(),
                        left: pr_id,
                        right: zero_id,
                    },
                );
                self.type_map.insert(cond_id, Type::Bool);

                body_stmts.push(MirStmt::If {
                    cond: cond_id,
                    then: then_stmts,
                    else_: vec![],
                    dest: None,
                });

                // While(true) loop
                let true_id = self.next_id_with_lit(1);
                self.stmts.push(MirStmt::While {
                    cond: true_id,
                    pre_cond: vec![],
                    body: body_stmts,
                    else_body: vec![],
                });

                // Store result back to the expression ID so it's accessible
                // via load_local(id) later (e.g., in a return statement).
                self.stmts.push(MirStmt::Assign {
                    lhs: id,
                    rhs: result_id,
                });

                // Register result as the expression value
                self.exprs.insert(id, MirExpr::Var(result_id));
                if let Some(ty) = self.type_map.get(&fut_id) {
                    self.type_map.insert(id, ty.clone());
                } else {
                    self.type_map.insert(id, Type::I64);
                }
                return id;
            }
            AstNode::TimingOwned { inner, .. } => {
                // Timing-owned: wrap the inner expression.
                let inner_id = self.lower_expr(inner);
                self.exprs.insert(id, MirExpr::TimingOwned(inner_id));
                if let Some(ty) = self.type_map.get(&inner_id) {
                    self.type_map.insert(id, ty.clone());
                } else {
                    self.type_map.insert(id, Type::I64);
                }
            }
            _ => {
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
            }
        }
        id
    }

    /// PY-A: lower one element of a comma subscript. A slice element
    /// (`df.iloc[:, 0]`) must NOT go through the real `zeta_slice_vec` path:
    /// that interprets the base as a Vec handle, and the base here is an
    /// opaque platform object (numpy/pandas), so it read a garbage header off
    /// the handle — `d[:, 0]` on a plain integer base segfaulted. The slice is
    /// an opaque placeholder, which is all `py_getitem2` needs.
    fn lower_multi_index_element(&mut self, item: &AstNode) -> u32 {
        if let AstNode::Call {
            method, args, ..
        } = item
        {
            if method == "__slice__" || method == "__slice_step__" {
                let mut ids: Vec<u32> = args.iter().map(|a| self.lower_expr(a)).collect();
                while ids.len() < 3 {
                    let filler = self.next_id();
                    self.exprs.insert(filler, MirExpr::IntLit(0));
                    self.type_map.insert(filler, Type::I64);
                    ids.push(filler);
                }
                let dest = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: "py_slice_new".to_string(),
                    args: vec![ids[0], ids[1], ids[2]],
                    dest,
                    type_args: vec![],
                });
                self.exprs.insert(dest, MirExpr::Var(dest));
                self.type_map.insert(dest, Type::I64);
                return dest;
            }
        }
        self.lower_expr(item)
    }

    fn next_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Is this value an array (or an array-typed parameter)? Parameters are
    /// typed through `source_types` because unannotated ones default to i64.
    fn is_array_like(&self, id: &u32) -> bool {
        matches!(
            self.type_map.get(id),
            Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
        ) || self
            .source_types
            .get(id)
            .map_or(false, |s| s.starts_with('[') || s.starts_with("*mut ["))
    }

    /// Element-is-string flag for the list helpers (`py_list_contains` /
    /// `py_list_eq`), taken from either `type_map` or the source-type string.
    fn array_elem_is_str(&self, id: &u32) -> bool {
        match self.type_map.get(id) {
            Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => matches!(**e, Type::Str),
            _ => {
                let src = self.source_types.get(id).cloned().unwrap_or_default();
                let inner = src.trim_start_matches("*mut ").trim_start_matches('[');
                inner.starts_with("str") || inner.starts_with("String")
            }
        }
    }

    /// PY-A: lower a `for … else` / `while … else` body into its own statement
    /// list, kept OUT of the loop body (codegen runs it only when the loop
    /// finished without `break`). Empty when there is no `else` clause.
    fn lower_loop_else(&mut self, else_body: &[AstNode]) -> Vec<MirStmt> {
        if else_body.is_empty() {
            return Vec::new();
        }
        let before = self.stmts.len();
        for stmt in else_body {
            self.lower_ast(stmt);
        }
        self.stmts.split_off(before)
    }

    /// Get the common type of element expressions.
    /// If elements is empty, returns Type::I64 as default.
    fn get_common_element_type(&self, element_ids: &[u32]) -> Type {
        if let Some(first_elem_id) = element_ids.first() {
            self.type_map
                .get(first_elem_id)
                .cloned()
                .unwrap_or(Type::I64)
        } else {
            Type::I64
        }
    }

    fn next_id_with_lit(&mut self, n: i64) -> u32 {
        let id = self.next_id();
        self.exprs.insert(id, MirExpr::IntLit(n));
        self.type_map.insert(id, Type::I64);
        id
    }

    /// PY-A: lower a closure body into a standalone synthetic function and
    /// return its name. The closure value is then the function address
    /// (i64), so `let f = lambda x: x + 1; f(41)` lowers to a direct call
    /// to the named closure — no environment struct / capture machinery
    /// (V1: non-capturing lambdas only; capturing bodies keep the prior
    /// no-op-stub behaviour and stay known-fail).
    /// Collect free variables of an expression against a set of bound names
    /// (params + locals already known when the closure is created).
    fn collect_free_vars(expr: &AstNode, bound: &mut std::collections::HashSet<String>, free: &mut std::collections::BTreeSet<String>) {
        match expr {
            AstNode::Var(name) => {
                if !bound.contains(name) {
                    free.insert(name.clone());
                }
            }
            AstNode::BinaryOp { left, right, .. } => {
                Self::collect_free_vars(left, bound, free);
                Self::collect_free_vars(right, bound, free);
            }
            AstNode::UnaryOp { expr, .. } => Self::collect_free_vars(expr, bound, free),
            AstNode::Call { receiver, method, args, .. } => {
                if let Some(r) = receiver {
                    Self::collect_free_vars(r, bound, free);
                }
                // method 名不是自由变量；args 递归
                for a in args {
                    Self::collect_free_vars(a, bound, free);
                }
                let _ = method;
            }
            AstNode::FieldAccess { base, .. } => Self::collect_free_vars(base, bound, free),
            AstNode::Subscript { base, index } => {
                Self::collect_free_vars(base, bound, free);
                Self::collect_free_vars(index, bound, free);
            }
            AstNode::Assign(lhs, rhs) => {
                // 赋值目标也是自由变量（写捕获）
                Self::collect_free_vars(lhs, bound, free);
                Self::collect_free_vars(rhs, bound, free);
            }
            AstNode::If { cond, then, else_ } => {
                Self::collect_free_vars(cond, bound, free);
                for s in then { Self::collect_free_vars(s, bound, free); }
                for s in else_ { Self::collect_free_vars(s, bound, free); }
            }
            AstNode::Let { pattern, expr, .. } => {
                Self::collect_free_vars(expr, bound, free);
                // let 绑定的名字成为局部，不改变外层 free 集（近似）
                let _ = pattern;
            }
            AstNode::Return(e) => Self::collect_free_vars(e, bound, free),
            AstNode::Block { body } => {
                for st in body {
                    Self::collect_free_vars(st, bound, free);
                }
            }
            AstNode::ExprStmt { expr } => Self::collect_free_vars(expr, bound, free),
            AstNode::Let { pattern, expr, .. } => {
                Self::collect_free_vars(expr, bound, free);
                if let AstNode::Var(n) = &**pattern {
                    // bound inside this scope after the let
                    bound.insert(n.clone());
                }
            }
            _ => {}
        }
    }

    fn lower_closure(&mut self, params: &[String], body: &AstNode) -> String {
        // Globally unique across functions — per-MirGen counters made two
        // different closures share "__closure_0" (first definition won, other
        // call sites silently called the wrong body).
        static CLOSURE_SEQ: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(0);
        let n = CLOSURE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as usize;
        let closure_name = format!("__closure_{}", n);

        // PY-A V2: free variables of the body (against params + currently
        // bound names) are captured THROUGH the env runtime — each read is
        // zeta_env_get("name"), each assignment zeta_env_set("name", v).
        // NOTE: only params count as bound — enclosing locals are NOT visible
        // inside the synthetic function, so any other referenced name is a
        // free variable that must go through the env runtime.
        let mut bound: std::collections::HashSet<String> = params.iter().cloned().collect();
        let mut free: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        Self::collect_free_vars(body, &mut bound, &mut free);
        if std::env::var("ZETA_PROBE").is_ok() {
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
            .with_module_global_types(self.module_global_types.clone())
            .with_func_param_names(self.func_param_names.clone())
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
        child.re_repl_param = self.re_repl_param;
        child.fn_depth = self.fn_depth + 1;
        let param_hint = self.pending_closure_param_types.take();
        for (pi, p) in params.iter().enumerate() {
            let id = child.next_id();
            child.name_to_id.insert(p.clone(), id);
            child.exprs.insert(id, MirExpr::Var(id));
            // A `re.sub` replacement closure receives a Match handle.
            let hinted = param_hint.as_ref().and_then(|h| h.get(pi)).cloned();
            child.type_map.insert(
                id,
                match hinted {
                    Some(t) => t,
                    None => {
                        if self.re_repl_param {
                            Type::Named("PyMatch".to_string(), vec![])
                        } else {
                            Type::I64
                        }
                    }
                },
            );
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
            let name_id = child.next_id();
            child
                .exprs
                .insert(name_id, MirExpr::StringLit(name.clone()));
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
                .unwrap_or(Type::I64);
            child.type_map.insert(slot_id, cap_ty);
            child.name_to_id.insert(name.clone(), slot_id);
            child.captured_vars.insert(name.clone(), name_id);
        }
        if std::env::var("ZETA_PROBE").is_ok() {
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
            _ => child.lower_expr(body),
        };
        // Remember the body's value type for the enclosing comprehension (the
        // child MirGen owns that type map, so read it here).
        self.last_closure_ret_ty = child.type_map.get(&body_val).cloned();
        // Ensure the closure returns its body value.
        if std::env::var("ZETA_PROBE").is_ok() {
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
        closure_name
    }

    /// Assemble a Mir from the current lowering state. Shared by
    /// `lower_to_mir` (top-level items) and `lower_closure` (synthetic
    /// closure functions).
    fn build_mir(&mut self, params: &[String]) -> Mir {
        Mir {
            name: None,
            generic_params: vec![],
            param_indices: params
                .iter()
                .enumerate()
                .map(|(i, p)| (p.clone(), i as u32 + 1))
                .collect(),
            properties: vec![],
            stmts: std::mem::take(&mut self.stmts),
            exprs: std::mem::take(&mut self.exprs),
            is_extern: false,
            ctfe_consts: std::mem::take(&mut self.ctfe_consts),
            type_map: std::mem::take(&mut self.type_map),
            global_consts: std::mem::take(&mut self.global_consts),
        }
    }

    pub fn take_generated_mirs(&mut self) -> Vec<Mir> {
        std::mem::take(&mut self.generated_mirs)
    }
}

impl Default for MirGen {
    fn default() -> Self {
        Self::new()
    }
}

/// PY-A: Python string method -> (runtime symbol, arity, result kind).
/// Shared by the typed (Str receiver) path and the untyped-receiver fallback,
/// so the two cannot disagree.
/// PY-A: split a `"{} and {}"` format template into FString parts. Returns
/// None for templates this V1 cannot rewrite (format specs like `{0:>5}`,
/// conversion flags, named fields, or a missing argument), so the caller falls
/// through and the use fails loudly at link time rather than mis-formatting.
fn format_template_parts(
    tmpl: &str,
    args: &[AstNode],
    named: &[(String, AstNode)],
) -> Option<Vec<AstNode>> {
    let mut parts: Vec<AstNode> = Vec::new();
    let mut lit = String::new();
    let mut auto = 0usize;
    let mut i = 0usize;
    while i < tmpl.len() {
        let c = tmpl[i..].chars().next()?;
        if c == '{' {
            if tmpl[i + 1..].starts_with('{') {
                lit.push('{');
                i += 2;
                continue;
            }
            let rest = &tmpl[i + 1..];
            let close = rest.find('}')?;
            let inner = &rest[..close];
            // FORMAT SPECS (`{:.2f}`, `{:<0}`) and conversions (`{!r}`) cannot be
            // expressed through the f-string path. Dropping the spec keeps the
            // VALUE (only the presentation differs) instead of bailing out of the
            // rewrite entirely — bailing made the whole call a free `format` call
            // and the program failed to LINK (9 corpus call sites). Announced once
            // so it is never silent.
            let (field, _had_spec) = match inner.find([':', '!']) {
                Some(pos) => (&inner[..pos], true),
                None => (inner, false),
            };
            if _had_spec {
                static WARNED_SPEC: std::sync::OnceLock<()> = std::sync::OnceLock::new();
                WARNED_SPEC.get_or_init(|| {
                    eprintln!(
                        "warning: PY-A: str.format format-specs ({{:.2f}} / {{:<0}} / {{!r}}) \
                         are ignored — the value is kept, the presentation is not"
                    );
                });
            }
            // `{a}` — a NAMED field: `"{a}".format(a=1)`. Resolve it from the
            // keyword arguments instead of bailing out (bailing made the call a
            // free `format` call and the program failed to LINK; 7 corpus sites).
            let arg: &AstNode = if field.is_empty() {
                let k = auto;
                auto += 1;
                args.get(k)?
            } else if let Ok(idx) = field.parse::<usize>() {
                args.get(idx)?
            } else {
                match named.iter().find(|(n, _)| n == field) {
                    Some((_, v)) => v,
                    None => return None,
                }
            };
            if !lit.is_empty() {
                parts.push(AstNode::StringLit(std::mem::take(&mut lit)));
            }
            parts.push(arg.clone());
            i = i + 1 + close + 1;
        } else if c == '}' {
            if tmpl[i + 1..].starts_with('}') {
                lit.push('}');
                i += 2;
                continue;
            }
            return None;
        } else {
            lit.push(c);
            i += c.len_utf8();
        }
    }
    if !lit.is_empty() {
        parts.push(AstNode::StringLit(lit));
    }
    Some(parts)
}

/// Runtime suffix selecting the element-type-aware list-method variant:
/// string elements compare by content, f64 elements by value (their slots
/// hold the IEEE bit pattern); i64 is the default.
fn list_elem_suffix(elem: &Type) -> &'static str {
    match elem {
        Type::Str => "_str",
        Type::F64 => "_f64",
        _ => "",
    }
}

/// `lt(map, K, V)` / `lt(vec, T)` annotation → the canonical MIR type, or
/// `None` for anything else. `parse_lt_type` renders the sugar as `map<K, V>`
/// / `vec<T>`, and `vecstr` is the library shorthand for `vec<str>`.
fn lt_annotation_type(s: &str) -> Option<Type> {
    let s = s.trim();
    let (head, inner) = match s.split_once('<') {
        Some((h, rest)) => (h.trim(), Some(rest.trim_end_matches('>'))),
        None => (s, None),
    };
    let one = |t: &str| -> Type {
        let t = t.trim();
        match t {
            "vecstr" => Type::DynamicArray(Box::new(Type::Str)),
            _ => Type::from_string(t),
        }
    };
    match head {
        "vec" | "list" => Some(Type::DynamicArray(Box::new(
            inner
                .and_then(|i| i.split(',').next())
                .map(one)
                .unwrap_or(Type::I64),
        ))),
        "vecstr" => Some(Type::DynamicArray(Box::new(Type::Str))),
        "map" | "dict" => {
            let mut it = inner.unwrap_or("").split(',');
            let k = it.next().map(one).unwrap_or(Type::I64);
            let v = it.next().map(one).unwrap_or(Type::I64);
            Some(Type::Named("map".to_string(), vec![k, v]))
        }
        _ => None,
    }
}

fn str_method_symbol(method: &str) -> Option<(&'static str, usize, &'static str)> {    match method {
        "upper" => Some(("host_str_to_uppercase", 1, "str")),
        "lower" => Some(("host_str_to_lowercase", 1, "str")),
        "capitalize" => Some(("host_str_capitalize", 1, "str")),
        "title" => Some(("host_str_title", 1, "str")),
        "swapcase" => Some(("host_str_swapcase", 1, "str")),
        "trim" | "strip" => Some(("host_str_trim", 1, "str")),
        "lstrip" => Some(("host_str_lstrip", 1, "str")),
        "rstrip" => Some(("host_str_rstrip", 1, "str")),
        "contains" => Some(("host_str_contains", 2, "bool")),
        "startswith" | "starts_with" => Some(("host_str_starts_with", 2, "bool")),
        "endswith" | "ends_with" => Some(("host_str_ends_with", 2, "bool")),
        "replace" => Some(("host_str_replace", 3, "str")),
        "find" | "index" => Some(("host_str_find", 2, "i64")),
        "rfind" => Some(("host_str_rfind", 2, "i64")),
        "count" => Some(("host_str_count", 2, "i64")),
        "len" => Some(("host_str_len", 1, "i64")),
        "split" => Some(("host_str_split", 2, "split")),
        "join" => Some(("host_str_join", 2, "str")),
        "zfill" => Some(("host_str_zfill", 2, "str")),
        "ljust" => Some(("host_str_ljust", 3, "str")),
        "rjust" => Some(("host_str_rjust", 3, "str")),
        "isalpha" => Some(("host_str_isalpha", 1, "bool")),
        "isdigit" => Some(("host_str_isdigit", 1, "bool")),
        "isupper" => Some(("host_str_isupper", 1, "bool")),
        "islower" => Some(("host_str_islower", 1, "bool")),
        // The rest of the str.is* family. Without a table entry each name
        // linked against the same-named libc ctype function (a different
        // signature entirely) and silently returned 0.
        "isalnum" => Some(("host_str_isalnum", 1, "bool")),
        "isspace" => Some(("host_str_isspace", 1, "bool")),
        "isnumeric" => Some(("host_str_isnumeric", 1, "bool")),
        "isdecimal" => Some(("host_str_isdecimal", 1, "bool")),
        "isascii" => Some(("host_str_isascii", 1, "bool")),
        "isprintable" => Some(("host_str_isprintable", 1, "bool")),
        "istitle" => Some(("host_str_istitle", 1, "bool")),
        "removeprefix" => Some(("host_str_removeprefix", 2, "str")),
        "removesuffix" => Some(("host_str_removesuffix", 2, "str")),
        _ => None,
    }
}
