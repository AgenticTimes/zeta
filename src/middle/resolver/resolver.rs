// src/middle/resolver/resolver.rs
//! # Resolver - Semantic Analysis & Monomorphization Engine
//!
//! Single source of truth for type resolution, specialization caching,
//! borrow checking coordination, and MIR lowering.

use crate::frontend::ast::AstNode;
use crate::frontend::ast::GenericParam;
use crate::frontend::borrow::BorrowChecker;
use crate::frontend::macro_expand::MacroExpander;
use crate::middle::mir::mir::Mir;
use crate::middle::resolver::module_resolver::ModuleResolver;
use crate::middle::resolver::typecheck_new::NewTypeCheck;
use crate::middle::specialization::{
    CACHE, MonoKey, MonoValue, is_cache_safe, record_specialization,
};
use crate::middle::types::ArraySize;
use crate::middle::types::TypeVar;
use crate::middle::types::identity::{CapabilityLevel, IdentityType};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub use crate::middle::types::Type;

const SPECIALIZATION_CACHE_FILE: &str = ".zeta_specialization_cache.json";

#[derive(Serialize, Deserialize)]
struct CacheFile {
    entries: HashMap<MonoKey, MonoValue>,
}

pub struct Resolver {
    pub impls: HashMap<(String, String), Vec<AstNode>>,
    /// Batch 602: (class, base) pairs marked by parse_class (`__bases__:B`
    /// on the ImplBlock attrs, no own `__init__`) — consumed by
    /// `inherit_class_members` after registration completes.
    pending_inherits: std::cell::RefCell<Vec<(String, String)>>,
    /// Batch 654: module-level names whose last top-level assignment was the
    /// literal `None` — sequential; feeds the gen print/str render faces.
    none_vars: std::cell::RefCell<std::collections::HashSet<String>>,
    /// Batch 738: memoized `module_global_types` for the lower_to_mir builder
    /// (site s5195) — lower_to_mir runs once per function/closure (651 calls
    /// on jq_wufu_local), and each call re-walked the WHOLE module AST to
    /// rebuild the same map ⇒ O(functions × program). The module-level
    /// assignment set is fixed once lowering begins, so the map is computed
    /// once and cloned per request. Other call sites (refine passes) keep
    /// the uncached path: their inputs may still be growing.
    mgt_cache: std::cell::RefCell<Option<HashMap<String, Type>>>,
    /// Batch 628: `Class::method` -> [(FuncDef param position, Type)] — the
    /// 627 call-site refinements, computed once at typecheck so the return
    /// inference below can read them and the generator builder can reuse
    /// the same map.
    pub(crate) method_param_refinements: HashMap<String, Vec<(usize, Type)>>,
    /// Batch 602/603: class -> first base (from `__bases__:` ImplBlock attrs),
    /// recorded for EVERY class with a base; consumed by the gen-side
    /// base-chain method lookup via `class_bases()`.
    class_bases: std::cell::RefCell<HashMap<String, String>>,
    /// Batch 620: (class, base, args) from `__baseargs__:<B>:` ImplBlock
    /// attrs — the explicit `B.__init__(self, ·)` call args (self stripped,
    /// non-Var kinds encoded by parse). Consumed by `inherit_class_members`
    /// to bind the base ctor's params positionally before adopting its
    /// field initializers.
    pending_baseargs: std::cell::RefCell<Vec<(String, String, Vec<AstNode>)>>,
    pub cached_mirs: HashMap<String, Mir>,
    pub mono_mirs: HashMap<MonoKey, Mir>,
    pub borrow_checker: RefCell<BorrowChecker>,
    pub associated_types: HashMap<(String, String), String>,
    pub ctfe_consts: HashMap<String, crate::middle::ctfe::value::ConstValue>,
    funcs: HashMap<String, FuncSignature>,
    /// Batch 761 (#80③): REPL 每行降值模式旗标（穿线给 MirGen::with_repl_mode）。
    repl_lowering: bool,
    /// Batch 747 (#264): callee -> (its `*args` param, its `**kwargs`
    /// param), for call-site collection of positional overflow / unmatched
    /// keyword arguments.
    star_params: RefCell<HashMap<String, (Option<String>, Option<String>)>>,
    /// Registered function ASTs (including module functions)
    registered_funcs: HashMap<String, AstNode>,
    /// Module resolver for Zorb imports
    module_resolver: ModuleResolver,
    /// Macro expander for macro processing
    macro_expander: MacroExpander,
    /// Program-wide type declarations (enums/type aliases), seeded into every
    /// per-function MirGen so pattern matching and enum paths can resolve them.
    type_decls: HashMap<String, crate::middle::mir::r#gen::TypeDecl>,
    /// Synthetic lambda/closure MIRs accumulated during per-function lowering
    /// (they would otherwise be dropped — generated_mirs lives on the
    /// per-call MirGen).
    generated_closures: RefCell<HashMap<String, Mir>>,
    /// PY-A V3: names declared `nonlocal` anywhere in the program. The
    /// DEFINING scope must also store these through the closure env so the
    /// inner function sees the initialized slot (capture-by-reference).
    nonlocal_names: RefCell<std::collections::HashSet<String>>,
    /// PY-A: module-top-level bare-assigned names (implicit-module global
    /// reads fall back to env without explicit `global` declaration).
    module_globals: RefCell<std::collections::HashSet<String>>,
    /// PY-A: `import X as a` → a → canonical module name.
    py_module_aliases: RefCell<std::collections::HashMap<String, String>>,
    /// PY-A: `from X import y as b` → b → (module, member).
    /// PY-A: `from X import y as b` — b → (module, member).
    py_member_aliases: RefCell<std::collections::HashMap<String, (String, String)>>,
    /// PY-A: Python modules loaded from disk (not registry shims).
    py_user_modules: RefCell<std::collections::HashSet<String>>,
    /// PY-A: module name → the file it was loaded from (per-module `__file__`).
    py_module_paths: RefCell<std::collections::HashMap<String, String>>,
    /// PY-A: per-module top-level definition names (rename map source).
    py_module_own_names:
        RefCell<std::collections::HashMap<String, std::collections::HashSet<String>>>,
    /// PY-A: facade re-exports — `from .sub import f` inside `pkg` records
    /// `pkg[f] → (pkg.sub, f)`. Consumers doing `from pkg import f` must bind
    /// the *defining* module, or calls emit `pkg__f` while only `pkg_sub__f`
    /// exists (REasyQuant `market_data` facade pattern).
    py_module_reexports: RefCell<
        std::collections::HashMap<String, std::collections::HashMap<String, (String, String)>>,
    >,
    /// PY-A: mangled module definition name → owning module.
    py_mangled_to_module: RefCell<std::collections::HashMap<String, String>>,
    /// PY-A: modules already loaded from disk (recursion / duplicate guard).
    py_loaded_modules: RefCell<std::collections::HashSet<String>>,
    /// resolved file path → the module name it was loaded under. The SAME file is
    /// reachable under two names (`strategies.code.jq_shim` vs the bare
    /// `jq_shim`, both on sys.path), and each name got its own prefix — so a
    /// consumer importing the bare form emitted `jq_shim__new_context` while the
    /// definition was emitted as `strategies_code_jq_shim__new_context`
    /// (undefined). Canonicalize onto the first name loaded.
    py_loaded_paths: RefCell<std::collections::HashMap<std::path::PathBuf, String>>,
    /// PY-A: directory of the file being compiled (module search root).
    py_source_dir: RefCell<Option<std::path::PathBuf>>,
    /// PY-A: the file being compiled — the value of `__file__`.
    source_file: RefCell<Option<String>>,
    /// PY-A: argparse flag → value kind, collected program-wide (the
    /// `add_argument` call and the `args.<flag>` read usually live in
    /// different functions, so per-MirGen state would not see it).
    argparse_kinds: RefCell<std::collections::HashMap<String, String>>,
    /// PY-A: Python default argument values per function name. `params` in the
    /// AST only holds (name, type); the parser records defaults as a
    /// body-prologue `zeta_param_default(index, value)` marker, collected here
    /// so the MIR lowering can fill omitted call arguments.
    param_defaults: RefCell<std::collections::HashMap<String, Vec<Option<AstNode>>>>,
    /// PY-A: module name → its `__package__` (relative-import anchor). A
    /// package's `__init__` anchors to itself; a submodule anchors to its
    /// parent package.
    py_module_pkg: RefCell<std::collections::HashMap<String, String>>,
    /// PY-A: the module currently being registered/loaded, so relative
    /// imports inside it resolve against the right package.
    py_current_module: RefCell<Option<String>>,
    /// PY-A: every registered function definition (including ones loaded from
    /// imported modules) — return-type inference must cover all of them.
    registered_func_defs: RefCell<Vec<std::rc::Rc<AstNode>>>,
    /// 批次 813（#268②）：函数名 → 它自己降完 MIR 后的 `signature_ret_ty()`，
    /// 只登记 F32/F64。供后面的调用点填空调用槽（声明表里的空白位），与
    /// codegen 给被调方定的 LLVM 签名同源＝批次 399 定的那一条规则。
    body_ret_tys: RefCell<HashMap<String, Type>>,
    /// 批次 813：`prime_body_ret` 已就地降过名的集合（非浮点的 def 也进，避免反复重降）。
    primed_names: RefCell<std::collections::HashSet<String>>,
    /// 批次 813：正在就地预热的名字，挡住 `a` 调 `b`、`b` 又调 `a` 的环。
    priming: RefCell<std::collections::HashSet<String>>,
    /// 批次 400（类型基础③）：`"函数.参数"` → 该参数在各调用点看到的实参类形
    /// （如 `str≠i64`）。只有**证据不一致**的参数才会进这张表，它们是"因为冲突
    /// 而保持动态"的位置，`--report-untyped` 把它们和"从来没有任何证据"的动态
    /// 参数分开出声。
    ambiguous_dyn_params: RefCell<std::collections::HashMap<String, String>>,
}

// Learning: Complex type factored into type definition per clippy suggestion
type FuncSignature = (Vec<(String, Type)>, Type, bool);

impl Resolver {
    pub fn new() -> Self {
        let mut r = Self {
            impls: HashMap::new(),
            pending_inherits: std::cell::RefCell::new(Vec::new()),
            none_vars: std::cell::RefCell::new(std::collections::HashSet::new()),
            mgt_cache: std::cell::RefCell::new(None),
            method_param_refinements: HashMap::new(),
            class_bases: std::cell::RefCell::new(HashMap::new()),
            pending_baseargs: std::cell::RefCell::new(Vec::new()),
            cached_mirs: HashMap::new(),
            mono_mirs: HashMap::new(),
            borrow_checker: RefCell::new(BorrowChecker::new()),
            associated_types: HashMap::new(),
            ctfe_consts: HashMap::new(),
            funcs: HashMap::new(),
            repl_lowering: false,
            star_params: RefCell::new(HashMap::new()),
            registered_funcs: HashMap::new(),
            module_resolver: ModuleResolver::new("."),
            macro_expander: MacroExpander::new(),
            type_decls: HashMap::new(),
            generated_closures: RefCell::new(HashMap::new()),
            nonlocal_names: RefCell::new(std::collections::HashSet::new()),
            module_globals: RefCell::new(std::collections::HashSet::new()),
            py_module_aliases: RefCell::new(std::collections::HashMap::new()),
            py_member_aliases: RefCell::new(std::collections::HashMap::new()),
            py_user_modules: RefCell::new(std::collections::HashSet::new()),
            py_module_paths: RefCell::new(std::collections::HashMap::new()),
            py_module_own_names: RefCell::new(std::collections::HashMap::new()),
            py_module_reexports: RefCell::new(std::collections::HashMap::new()),
            py_mangled_to_module: RefCell::new(std::collections::HashMap::new()),
            py_loaded_modules: RefCell::new(std::collections::HashSet::new()),
            py_loaded_paths: RefCell::new(std::collections::HashMap::new()),
            py_source_dir: RefCell::new(None),
            source_file: RefCell::new(None),
            argparse_kinds: RefCell::new(std::collections::HashMap::new()),
            param_defaults: RefCell::new(std::collections::HashMap::new()),
            py_module_pkg: RefCell::new(std::collections::HashMap::new()),
            py_current_module: RefCell::new(None),
            registered_func_defs: RefCell::new(Vec::new()),
            body_ret_tys: RefCell::new(HashMap::new()),
            primed_names: RefCell::new(std::collections::HashSet::new()),
            priming: RefCell::new(std::collections::HashSet::new()),
            ambiguous_dyn_params: RefCell::new(std::collections::HashMap::new()),
        };

        // Register built-in runtime functions
        r.register_builtin_functions();

        r.load_specialization_cache();
        r
    }

    fn load_specialization_cache(&mut self) {
        let path = PathBuf::from(SPECIALIZATION_CACHE_FILE);
        if let Ok(data) = fs::read_to_string(&path)
            && let Ok(cache) = serde_json::from_str::<CacheFile>(&data)
        {
            // OPTIMIZATION: Iterate without cloning keys
            for (key, value) in cache.entries {
                // Use key directly without clone when possible
                self.mono_mirs.insert(key.clone(), Mir::default());
                record_specialization(key, value);
            }
        }
    }

    pub fn persist_specialization_cache(&self) {
        let cache_guard = CACHE.read().unwrap();
        let entries = cache_guard.clone();
        let cache_file = CacheFile { entries };
        if let Ok(json) = serde_json::to_string_pretty(&cache_file) {
            let _ = fs::write(SPECIALIZATION_CACHE_FILE, json);
        }
    }

    pub fn register(&mut self, ast: AstNode) {
        if crate::diagnostics::env_flag("ZETA_PROBE_GLOBALS") {
            eprintln!(
                "REGISTER: defs={} globals={} loaded_mods={}",
                self.registered_func_defs.borrow().len(),
                self.module_globals.borrow().len(),
                self.py_loaded_modules.borrow().len()
            );
        }
        // PY-A: keep the definition for return-type inference (imported
        // modules register through this same path).
        if matches!(ast, AstNode::FuncDef { .. }) {
            self.registered_func_defs
                .borrow_mut()
                .push(std::rc::Rc::new(ast.clone()));
        }
        // PY-A V3: pre-collect all `nonlocal` names (program-wide set) so the
        // defining scope's assignments route through the closure env.
        {
            fn walk_nonlocal(n: &AstNode, set: &mut std::collections::HashSet<String>) {
                match n {
                    AstNode::Call { receiver: None, method, args, .. }
                        if method == "zeta_nonlocal_decl" =>
                    {
                        for a in args {
                            if let AstNode::StringLit(name) = a {
                                set.insert(name.clone());
                            }
                        }
                    }
                    AstNode::FuncDef { body, ret_expr, .. } => {
                        for s in body {
                            walk_nonlocal(s, set);
                        }
                        if let Some(e) = ret_expr {
                            walk_nonlocal(e, set);
                        }
                    }
                    AstNode::Block { body } => {
                        for s in body {
                            walk_nonlocal(s, set);
                        }
                    }
                    AstNode::ExprStmt { expr } => walk_nonlocal(expr, set),
                    AstNode::Assign(lhs, rhs) => {
                        walk_nonlocal(lhs, set);
                        walk_nonlocal(rhs, set);
                    }
                    AstNode::Let { expr, .. } => walk_nonlocal(expr, set),
                    AstNode::Return(e) => walk_nonlocal(e, set),
                    AstNode::BinaryOp { left, right, .. } => {
                        walk_nonlocal(left, set);
                        walk_nonlocal(right, set);
                    }
                    AstNode::If { cond, then, else_ } => {
                        walk_nonlocal(cond, set);
                        for s in then { walk_nonlocal(s, set); }
                        for s in else_ { walk_nonlocal(s, set); }
                    }
                    AstNode::While {
                        cond,
                        body,
                        else_body,
                    } => {
                        walk_nonlocal(cond, set);
                        for s in body { walk_nonlocal(s, set); }
                        for s in else_body { walk_nonlocal(s, set); }
                    }
                    AstNode::For { body, else_body, .. } => {
                        for s in body { walk_nonlocal(s, set); }
                        for s in else_body { walk_nonlocal(s, set); }
                    }
                    _ => {}
                }
            }
            walk_nonlocal(&ast, &mut self.nonlocal_names.borrow_mut());
        }
        // PY-A: collect module-top-level bare assignment names (implicit
        // module globals) so reads from other functions fall back to env.
        {
            fn walk_module(n: &AstNode, set: &mut std::collections::HashSet<String>) {
                match n {
                    AstNode::Call { receiver: None, method, args, .. }
                        if method == "zeta_module_decl" =>
                    {
                        for a in args {
                            if let AstNode::StringLit(name) = a {
                                set.insert(name.clone());
                            }
                        }
                    }
                    AstNode::ExprStmt { expr } => walk_module(expr, set),
                    AstNode::Block { body } => {
                        for s in body { walk_module(s, set); }
                    }
                    AstNode::FuncDef { body, .. } => {
                        for s in body { walk_module(s, set); }
                    }
                    _ => {}
                }
            }
            walk_module(&ast, &mut self.module_globals.borrow_mut());
        }
        // PY-A: collect Python-library imports. Known modules bind for real;
        // unknown ones are reported (never silently swallowed, which could
        // compile into a program that reads garbage) and left as external
        // shims whose members resolve by name.
        {
            fn walk_py_import(n: &AstNode, out: &mut Vec<(String, String, String)>) {
                match n {
                    AstNode::Call { receiver: None, method, args, .. }
                        if method == "zeta_py_import"
                            || method == "zeta_py_from"
                            || method == "zeta_py_star" =>
                    {
                        let mut strs: Vec<String> = Vec::new();
                        for a in args {
                            if let AstNode::StringLit(s) = a {
                                strs.push(s.clone());
                            }
                        }
                        if method == "zeta_py_import" && strs.len() >= 2 {
                            out.push(("import".to_string(), strs[0].clone(), strs[1].clone()));
                        } else if method == "zeta_py_star" && !strs.is_empty() {
                            out.push(("star".to_string(), strs[0].clone(), String::new()));
                        } else if method == "zeta_py_from" && strs.len() >= 3 {
                            out.push((strs[0].clone(), strs[1].clone(), strs[2].clone()));
                        }
                        // Still walk args (no nested imports expected).
                    }
                    AstNode::Call { receiver, args, .. } => {
                        // PY-A: try/except desugars to Calls (`zeta_try_enter`,
                        // setjmp, …) wrapping the real body. Previously only
                        // import-marker Calls were matched and other Calls
                        // were NOT recursed into — imports sitting beside
                        // them in a Block were fine, but any import nested
                        // under a non-marker Call was skipped. Recurse.
                        if let Some(r) = receiver {
                            walk_py_import(r, out);
                        }
                        for a in args {
                            walk_py_import(a, out);
                        }
                    }
                    AstNode::ExprStmt { expr } => walk_py_import(expr, out),
                    AstNode::Return(e) => walk_py_import(e, out),
                    AstNode::Assign(lhs, rhs) => {
                        walk_py_import(lhs, out);
                        walk_py_import(rhs, out);
                    }
                    AstNode::Let { expr, .. } => walk_py_import(expr, out),
                    AstNode::BinaryOp { left, right, .. } => {
                        walk_py_import(left, out);
                        walk_py_import(right, out);
                    }
                    AstNode::Block { body } => {
                        for s in body {
                            walk_py_import(s, out);
                        }
                    }
                    // Imports nested in an `if` are REAL imports — the
                    // `if os.environ.get('…') == '1': from <local shim> import …
                    // else: from jqdata import *` switch at the top of a
                    // strategy is the whole reason the local implementation can
                    // be built without the platform. Not walking the branches
                    // left the shim unloaded and every imported name an
                    // unresolved external.
                    AstNode::If { cond, then, else_, .. } => {
                        // Also walk `cond`: try/except desugars to
                        // `if zeta_try_setjmp() == 0` and the import lives in
                        // `then` — cond walk is cheap; then/else are required.
                        // Skipping then/else left `try: from … import X` unbound
                        // (bare `_X` at link) while the same import outside try
                        // worked.
                        walk_py_import(cond, out);
                        for s in then {
                            walk_py_import(s, out);
                        }
                        for s in else_ {
                            walk_py_import(s, out);
                        }
                    }
                    AstNode::For { body, else_body, .. } => {
                        for s in body {
                            walk_py_import(s, out);
                        }
                        for s in else_body {
                            walk_py_import(s, out);
                        }
                    }
                    AstNode::While { cond, body, else_body, .. } => {
                        walk_py_import(cond, out);
                        for s in body {
                            walk_py_import(s, out);
                        }
                        for s in else_body {
                            walk_py_import(s, out);
                        }
                    }
                    AstNode::FuncDef { body, ret_expr, .. } => {
                        for s in body {
                            walk_py_import(s, out);
                        }
                        // parse_func promotes a trailing Block/If/… into
                        // `ret_expr`, leaving `body` empty. `try/except`
                        // desugars to exactly such a Block — so a
                        // `try: from … import X` lived only in ret_expr and
                        // was never collected (bare `_X` at link) while the
                        // same import as a non-trailing stmt worked.
                        if let Some(e) = ret_expr {
                            walk_py_import(e, out);
                        }
                    }
                    _ => {}
                }
            }
            let mut found: Vec<(String, String, String)> = Vec::new();
            walk_py_import(&ast, &mut found);
            // The module currently being registered (if any) anchors relative
            // imports: `register` recurses through loaded modules, so a
            // `from . import x` inside `pkg/sub.py` must resolve against `pkg`.
            let ctx = self.py_current_module.borrow().clone();
            for (kind, a, b) in found {
                // `from X import *` — bind the module's public top-level names.
                if kind == "star" {
                    if let Some(module) = self.resolve_py_module_spec(&a, ctx.as_deref()) {
                        if crate::middle::pylib::find_module(&module).is_none() {
                            let _ = self.load_user_python_module(&module);
                        }
                        let names: Vec<String> = self
                            .py_module_own_names
                            .borrow()
                            .get(&module)
                            .map(|s| {
                                s.iter()
                                    .filter(|n| !n.starts_with('_'))
                                    .cloned()
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mut bound = 0usize;
                        for n in names {
                            let (sm, sn) = self.resolve_reexport_target(&module, &n);
                            self.py_member_aliases
                                .borrow_mut()
                                .insert(n, (sm, sn));
                            bound += 1;
                        }
                        // Facade packages often only re-export; those names are
                        // not in own_names (no local def) but must still bind.
                        if let Some(rex) = self.py_module_reexports.borrow().get(&module).cloned() {
                            for (alias, (sm, sn)) in rex {
                                if alias.starts_with('_') {
                                    continue;
                                }
                                self.py_member_aliases
                                    .borrow_mut()
                                    .insert(alias, (sm, sn));
                                bound += 1;
                            }
                        }
                        if bound == 0 {
                            eprintln!(
                                "warning: PY-A: `from {} import *` bound no public names",
                                module
                            );
                        }
                    }
                    continue;
                }
                let (spec, member, alias) = if kind == "import" {
                    // ("import", module, alias)
                    (a, None, b)
                } else {
                    // (module, member, alias)
                    (kind, Some(a), b)
                };
                // A relative specifier (`.`, `..pkg.mod`) resolves against the
                // importing module's package; absolute names pass through.
                let module = match self.resolve_py_module_spec(&spec, ctx.as_deref()) {
                    Some(m) => m,
                    None => continue, // already reported (fail-loud)
                };
                // Record the RAW specifier → resolved module so the MIR can map a
                // RELATIVE import (`from ..datasrc.x import f`) back to its module
                // when it decides whether to run `<module>__init`. Without this a
                // function-local relative import never triggered the target's
                // module init, so `register_universe` wrote into an uninitialized
                // `UNIVERSES` and the registration was lost (silent wrong state).
                if !spec.is_empty() && spec != module {
                    self.py_module_aliases
                        .borrow_mut()
                        .insert(spec.clone(), module.clone());
                }
                // `from . import submod` — a bare-dots spec whose member is a
                // SUBMODULE imports that module (binds `submod` to `pkg.submod`),
                // not a member of `pkg`.
                if let Some(m) = member.clone() {
                    if !spec.is_empty() && spec.chars().all(|c| c == '.') {
                        let sub = format!("{}.{}", module, m);
                        if self.find_py_module_file(&sub).is_some()
                            || crate::middle::pylib::find_module(&sub).is_some()
                        {
                            let _ = self.load_user_python_module(&sub);
                            self.py_module_aliases.borrow_mut().insert(alias, sub);
                            continue;
                        }
                    }
                }
                // Built-in registry first; otherwise try to load a user file
                // from disk (that is what makes `import` generic); only when
                // neither exists do we fall back to an external shim.
                let in_registry = crate::middle::pylib::find_module(&module).is_some();
                if !in_registry {
                    let _ = self.load_user_python_module(&module);
                } else if let Some((path, _)) = self.find_py_module_file(&module) {
                    // A local file for a module that ALSO has a built-in shim is
                    // a SUPPLEMENT: the registry keeps the members only it can
                    // express (handle-typed `pd.Timestamp` -> PyDate) and the
                    // file adds the rest as LIBRARY code.
                    eprintln!(
                        "warning: PY-A: `{}` has both a built-in shim and the local library \
                         {} — the library supplements it (registered members win)",
                        module,
                        path.display()
                    );
                    let _ = self.load_user_python_module(&module);
                }
                let is_user = self.py_user_modules.borrow().contains(&module);
                match &member {
                    None => {
                        if !in_registry && !is_user {
                            eprintln!(
                                "warning: PY-A: unknown Python module `{}` — treated as an \
                                 external shim (members resolve by name, no type checking)",
                                module
                            );
                        }
                        self.py_module_aliases.borrow_mut().insert(alias, module);
                    }
                    Some(m) => {
                        let known = crate::middle::pylib::find_member(&module, m).is_some()
                            || is_user
                            || crate::middle::pylib::is_noop_module(&module);
                        if !known {
                            eprintln!(
                                "warning: PY-A: unknown member `{}` in Python module \
                                 `{}` — treated as an external shim",
                                m, module
                            );
                        }
                        // Follow facade re-exports to the defining module so
                        // `from pkg import f` links against `pkg_sub__f`, not a
                        // missing `pkg__f`. Record the edge when this import
                        // itself lives inside a user module being loaded.
                        let (src_mod, src_mem) = self.resolve_reexport_target(&module, m);
                        if let Some(current) = self.py_current_module.borrow().clone() {
                            if current != src_mod || alias != src_mem {
                                self.py_module_reexports
                                    .borrow_mut()
                                    .entry(current)
                                    .or_default()
                                    .insert(alias.clone(), (src_mod.clone(), src_mem.clone()));
                            }
                        }
                        self.py_member_aliases
                            .borrow_mut()
                            .insert(alias, (src_mod, src_mem));
                    }
                }
            }
            // PY-A: `initialize = _strategy.initialize` — bind LHS to the
            // defining module member so bare `initialize(...)` links to
            // `jq_wufu__initialize`, not ghost `_initialize` (jq_wufu_local).
            // Also `_LocalPortfolio = LocalBackend` when LocalBackend was
            // `from … import`ed (not a local class) — same-module own_names
            // chase misses imports (jq_shim).
            {
                let mut field_binds: Vec<(String, String, String)> = Vec::new();
                walk_module_member_assigns(&ast, &mut field_binds);
                for (local, mod_alias, member) in field_binds {
                    let Some(module) = self.py_module_aliases.borrow().get(&mod_alias).cloned()
                    else {
                        continue;
                    };
                    let (src_mod, src_mem) = self.resolve_reexport_target(&module, &member);
                    self.py_member_aliases
                        .borrow_mut()
                        .insert(local, (src_mod, src_mem));
                }
                let mut name_binds: Vec<(String, String)> = Vec::new();
                walk_name_aliases(&ast, &mut name_binds);
                for (alias, target) in name_binds {
                    let Some((sm, sn)) = self.py_member_aliases.borrow().get(&target).cloned()
                    else {
                        continue;
                    };
                    let (src_mod, src_mem) = self.resolve_reexport_target(&sm, &sn);
                    self.py_member_aliases
                        .borrow_mut()
                        .insert(alias.clone(), (src_mod.clone(), src_mem.clone()));
                    if let Some(current) = self.py_current_module.borrow().clone() {
                        self.py_module_reexports
                            .borrow_mut()
                            .entry(current)
                            .or_default()
                            .insert(alias, (src_mod, src_mem));
                    }
                }
            }
        }
        // PY-A: collect Python default argument values from the body-prologue
        // markers the parser emits for `def f(a, b = 10)`.
        if let AstNode::FuncDef { name, params, body, .. } = &ast {
            let mut defaults: Vec<Option<AstNode>> = vec![None; params.len()];
            let mut any = false;
            for st in body {
                // A body statement is normally `ExprStmt`, but bare expression
                // nodes end up directly in `body` too (documented in
                // parse_func's promotion logic) — accept both shapes.
                let expr = match st {
                    AstNode::ExprStmt { expr } => expr.as_ref(),
                    c @ AstNode::Call { .. } => c,
                    _ => continue,
                };
                if let AstNode::Call {
                    receiver: None,
                    method,
                    args,
                    ..
                } = expr
                {
                    if method == "zeta_param_default" && args.len() == 2 {
                        if let AstNode::Lit(i) = &args[0] {
                            if (*i as usize) < defaults.len() {
                                // Fail-loud when the default's literal kind cannot
                                // live in the parameter's declared type: an
                                // unannotated param is i64, so `def f(x = "s")`
                                // would pass a pointer as an i64 (garbage output)
                                // and `def f(x = 1.5)` would silently truncate.
                                if let Some((pname, pty)) = params.get(*i as usize) {
                                    let mismatch = match &args[1] {
                                        AstNode::StringLit(_) => pty != "Str" && pty != "str",
                                        AstNode::FloatLit(_) => pty == "i64",
                                        _ => false,
                                    };
                                    if mismatch {
                                        eprintln!(
                                            "warning: PY-A: the default for `{}` has a kind that \
                                             parameter type `{}` cannot hold — it is coerced \
                                             (annotate the parameter, or change the default)",
                                            pname, pty
                                        );
                                    }
                                }
                                defaults[*i as usize] = Some(args[1].clone());
                                any = true;
                            }
                        }
                    }
                }
            }
            if any {
                self.param_defaults
                    .borrow_mut()
                    .insert(name.clone(), defaults);
            }
        }
        // PY-A: collect argparse flag kinds program-wide. `add_argument` is
        // usually at module level while `args.<flag>` is read inside a
        // function, so the kind table cannot live in a single MirGen.
        {
            fn walk_argparse(n: &AstNode, out: &mut std::collections::HashMap<String, String>) {
                match n {
                    AstNode::Call { method, args, .. } if method == "add_argument" => {
                        let mut flag: Option<String> = None;
                        let mut kind: Option<String> = None;
                        let mut dflt: Option<AstNode> = None;
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
                                            "type" => {
                                                if let AstNode::Var(t) = &ka[1] {
                                                    kind = Some(
                                                        match t.as_str() {
                                                            "float" => "f64",
                                                            "int" => "i64",
                                                            _ => "str",
                                                        }
                                                        .to_string(),
                                                    );
                                                }
                                            }
                                            "action" => {
                                                if let AstNode::StringLit(s) = &ka[1] {
                                                    if s == "store_true" {
                                                        kind = Some("bool".to_string());
                                                    }
                                                }
                                            }
                                            "default" => dflt = Some(ka[1].clone()),
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
                        if let Some(f) = flag {
                            let k = kind.unwrap_or_else(|| {
                                match dflt {
                                    Some(AstNode::StringLit(_)) => "str".to_string(),
                                    Some(AstNode::FloatLit(_)) => "f64".to_string(),
                                    _ => "str".to_string(),
                                }
                            });
                            // Python's `dest`: strip leading dashes and turn
                            // `-` into `_` (args.fusion_weight for --fusion-weight).
                            out.insert(
                                f.trim_start_matches('-').replace('-', "_"),
                                k,
                            );
                        }
                    }
                    AstNode::ExprStmt { expr } => walk_argparse(expr, out),
                    AstNode::Block { body } => {
                        for s in body {
                            walk_argparse(s, out);
                        }
                    }
                    AstNode::FuncDef { body, .. } => {
                        for s in body {
                            walk_argparse(s, out);
                        }
                    }
                    _ => {}
                }
            }
            let mut found: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            walk_argparse(&ast, &mut found);
            let mut table = self.argparse_kinds.borrow_mut();
            for (k, v) in found {
                table.insert(k, v);
            }
        }
        // Collect program-wide type declarations for MIR lowering.
        match &ast {
            AstNode::StructDef {
                name,
                fields,
                generics,
                ..
            } => {
                // Struct fields must be visible to MIR lowering: dataclasses
                // `asdict(x)` expands to a dict of the receiver's fields, which
                // needs the field list at compile time.
                self.type_decls.insert(
                    name.clone(),
                    crate::middle::mir::r#gen::TypeDecl::Struct {
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
                self.type_decls.insert(
                    name.clone(),
                    crate::middle::mir::r#gen::TypeDecl::Enum {
                        variants: variants.clone(),
                        generics: generics.clone(),
                    },
                );
            }
            AstNode::TypeAlias { name, ty, .. } => {
                self.type_decls.insert(
                    name.clone(),
                    crate::middle::mir::r#gen::TypeDecl::Alias { target: ty.clone() },
                );
            }
            _ => {}
        }
        match ast {
            AstNode::Use { path } => {
                // Process use statement to load module
                match self.module_resolver.process_use_statement(&path) {
                    Ok(module_asts) => {
                        // Register only enum/struct definitions, not impl blocks
                        for module_ast in module_asts {
                            match &module_ast {
                                AstNode::EnumDef {
                                    name,
                                    variants,
                                    generics,
                                    lifetimes,
                                    ..
                                } => {
                                    self.register(module_ast.clone());

                                    // Also register enum variant constructors as functions
                                    for (variant_name, variant_params) in variants {
                                        // Create a function name like "Option::Some"
                                        let func_name = format!("{}::{}", name, variant_name);

                                        // For generic enums, include type parameters in return type
                                        let generic_vars: Vec<String> = generics
                                            .iter()
                                            .filter_map(|g| match g {
                                                GenericParam::Type { name: n, .. } => {
                                                    Some(n.clone())
                                                }
                                                _ => None,
                                            })
                                            .collect();
                                        let ret_type = if generic_vars.is_empty() {
                                            name.clone()
                                        } else {
                                            format!("{}<{}>", name, generic_vars.join(", "))
                                        };

                                        // Create parameter types from variant params
                                        let params: Vec<(String, String)> = variant_params
                                            .iter()
                                            .enumerate()
                                            .map(|(i, param_type)| {
                                                (i.to_string(), param_type.clone())
                                            })
                                            .collect();

                                        // Create a fake function definition for the variant constructor
                                        let variant_func = AstNode::FuncDef {
                                            name: func_name.clone(),
                                            generics: generics.clone(),
                                            lifetimes: lifetimes.clone(),
                                            params,
                                            ret: ret_type,
                                            body: vec![],
                                            attrs: vec![],
                                            ret_expr: None,
                                            single_line: false,
                                            doc: "".to_string(),
                                            pub_: true, // Variant constructors are always public
                                            async_: false, // Variant constructors are not async
                                            const_: false, // Variant constructors are not const
                                            comptime_: false, // Variant constructors are not comptime
                                            where_clauses: vec![],
                                        };

                                        // Register the variant constructor
                                        self.register(variant_func);
                                    }
                                }
                                AstNode::StructDef { name, .. } => {
                                    self.register(module_ast);
                                }
                                AstNode::TypeAlias { name, .. } => {
                                    self.register(module_ast);
                                }
                                AstNode::ConstDef {
                                    name, comptime_, ..
                                } => {
                                    self.register(module_ast);
                                }
                                AstNode::FuncDef { name, .. } => {
                                    // When importing via `use std::malloc`, register with simple name
                                    // The function will be available as `malloc` in current scope
                                    self.register(module_ast);
                                }
                                AstNode::ImplBlock {
                                    concept, ty, body, ..
                                } => {
                                    // Register impl block functions with qualified names
                                    // (Type::method) so method calls can be resolved.
                                    self.impls
                                        .insert((concept.clone(), ty.clone()), body.clone());
                                    let base_ty = impl_key_base(ty.as_str());
                                    for b in body.clone() {
                                        if let AstNode::FuncDef {
                                            name,
                                            params,
                                            ret,
                                            async_,
                                            ..
                                        } = &b
                                        {
                                            let qualified_name =
                                                format!("{}::{}", base_ty, name);
                                            let typed_params: Vec<_> = params
                                                .iter()
                                                .map(|(n, t)| {
                                                    (
                                                        n.clone(),
                                                        self.typed_param_type(
                                                            &qualified_name,
                                                            n,
                                                            t,
                                                            &[],
                                                        ),
                                                    )
                                                })
                                                .collect();
                                            let typed_ret = self.string_to_type(ret);
                                            eprintln!(
                                                "REG_IB: {} -> {} params={}",
                                                name,
                                                qualified_name,
                                                params.len()
                                            );
                                            self.funcs.insert(
                                                qualified_name,
                                                (typed_params, typed_ret, *async_),
                                            );
                                        }
                                        if let AstNode::FuncDef { name: fn_name, .. } = &b {
                                            let qualified =
                                                format!("{}::{}", base_ty, fn_name);
                                            let mut qualified_ast = b.clone();
                                            if let AstNode::FuncDef {
                                                name: ref mut q_name,
                                                ..
                                            } = qualified_ast
                                            {
                                                *q_name = qualified.clone();
                                            }
                                            // Don't overwrite existing entries — modules loaded first (oneshot)
                                            // should keep their function signatures over later modules (broadcast).
                                            if !self.registered_funcs.contains_key(&qualified) {
                                                self.registered_funcs
                                                    .insert(qualified.clone(), qualified_ast);
                                            }
                                        }
                                        self.register(b);
                                    }
                                }
                                // Skip other items (impl blocks with issues, etc.)
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        crate::diag_warning!(
                            "W1007",
                            "Failed to process use statement {}: {}",
                            path.join("::"),
                            e
                        );
                    }
                }
            }
            AstNode::FuncDef {
                ref name,
                ref params,
                ref ret,
                ref async_,
                ref generics,
                ..
            } => {
                // Declared generic type-parameter names (fn f[T](...)).
                let generic_names: Vec<String> = generics
                    .iter()
                    .filter_map(|g| match g {
                        crate::frontend::ast::GenericParam::Type { name, .. } => {
                            Some(name.clone())
                        }
                        _ => None,
                    })
                    .collect();
                // Convert string types to Type enum
                let typed_params: Vec<(String, Type)> = params
                    .iter()
                    .map(|(pname, ty_str)| {
                        (
                            pname.clone(),
                            self.typed_param_type(name, pname, ty_str, &generic_names),
                        )
                    })
                    .collect();
                let typed_ret = self.string_to_generic_type(ret, &generic_names);
                // ;
                let name_clone = name.clone();
                self.funcs
                    .insert(name_clone.clone(), (typed_params, typed_ret, *async_));
                self.registered_funcs.insert(name_clone, ast.clone());
            }
            AstNode::ExternFunc {
                name,
                generics: _,
                lifetimes: _,
                params,
                ret,
                where_clauses: _,
            } => {
                // Convert string types to Type enum
                let typed_params: Vec<(String, Type)> = params
                    .iter()
                    .map(|(pname, ty_str)| {
                        (
                            pname.clone(),
                            self.typed_param_type(&name, pname, ty_str, &[]),
                        )
                    })
                    .collect();
                let typed_ret = self.string_to_type(&ret);
                self.funcs.insert(name, (typed_params, typed_ret, true));
            }
            AstNode::ImplBlock {
                concept, ty, body, attrs, ..
            } => {
                self.impls.insert((concept, ty.clone()), body.clone());
                // Batch 602: single-inheritance adoption marker (no own
                // `__init__` — parse_class only marks that shape).
                for a in attrs {
                    if let Some(b) = a.strip_prefix("__bases__:") {
                        self.pending_inherits
                            .borrow_mut()
                            .push((ty.clone(), b.to_string()));
                        self.class_bases
                            .borrow_mut()
                            .insert(ty.clone(), b.to_string());
                    }
                    // Batch 620: `__baseargs__:<B>:<enc>` — the explicit
                    // `B.__init__(self, ·)` args, self stripped, kinds encoded
                    // by parse (`v`ar/i`nt`/`f`loat/`s`tring/`b`ool`,`
                    // `x` unsupported), joined by \u{1f}. Decoded back to
                    // AstNodes here; consumed by `inherit_class_members`.
                    if let Some(rest) = a.strip_prefix("__baseargs__:") {
                        if let Some((bname, enc)) = rest.split_once(':') {
                            let args = enc
                                .split('\u{1f}')
                                .filter(|s| !s.is_empty())
                                .map(|s| match s.split_at(1) {
                                    ("v", n) => AstNode::Var(n.to_string()),
                                    ("i", x) => AstNode::Lit(x.parse().unwrap_or(0)),
                                    ("f", x) => AstNode::FloatLit(x.to_string()),
                                    ("s", x) => AstNode::StringLit(x.to_string()),
                                    ("b", "1") => AstNode::Bool(true),
                                    ("b", _) => AstNode::Bool(false),
                                    _ => AstNode::Lit(0),
                                })
                                .collect();
                            self.pending_baseargs
                                .borrow_mut()
                                .push((ty.clone(), bname.to_string(), args));
                        }
                    }
                }
                // Register functions with qualified names
                let base_ty = impl_key_base(ty.as_str());
                // Batch 651: cross-module same-named classes share the bare
                // qualified key (`Cfg::show` from both `m647a` and `m647b`).
                // `qualified_method_candidate` (gen.rs:1050) tries the mangled
                // direct key first (`m647a__Cfg::show`) — add it as an alias
                // so the direct arm hits and the tail fallback (which collides
                // across modules) is not needed.
                let mangled_cls = self
                    .py_current_module
                    .borrow()
                    .as_ref()
                    .map(|m| format!("{}__{}", m.replace('.', "_"), base_ty))
                    .filter(|mc| self.type_decls.contains_key(mc));
                for b in body.clone() {
                    if let AstNode::FuncDef {
                        name, params, ret, ..
                    } = &b
                    {
                        // Create qualified name: Type::method
                        let qualified_name = format!("{}::{}", base_ty, name);
                        // Convert string types to Type enum
                        let typed_params: Vec<(String, Type)> = params
                            .iter()
                            .map(|(pname, ty_str)| {
                                (
                                    pname.clone(),
                                    self.typed_param_type(&qualified_name, pname, ty_str, &[]),
                                )
                            })
                            .collect();
                        let typed_ret = self.string_to_type(ret);
                        self.funcs
                            .insert(qualified_name, (typed_params.clone(), typed_ret.clone(), false));
                    }
                    // Register with qualified name (for MIR resolution)
                    // Clone the func and override its name so MIR matches the call site
                    if let AstNode::FuncDef { name: fn_name, .. } = &b {
                        let qualified = format!("{}::{}", base_ty, fn_name);
                        let mut qualified_ast = b.clone();
                        if let AstNode::FuncDef { ref mut name, .. } = qualified_ast {
                            *name = qualified.clone();
                        }
                        self.registered_funcs.insert(qualified, qualified_ast);
                    }
                    // Also register with simple name for backwards compat.
                    // Defaults stay keyed by the bare method name (`reset_index`);
                    // MirGen::callee_sig_for_call also looks up `Type::method`.
                    // Batch 651: clone `b` so the mangled-alias block below
                    // can still borrow/clone it after register consumes the original.
                    self.register(b.clone());
                    // Batch 651: RE-INSERT mangled-class aliases AFTER
                    // self.register(b) — the bare-name registration overwrites
                    // both self.funcs and registered_funcs entries, so the
                    // mangled aliases must be set AFTER it, not before.
                    if let Some(ref mc) = mangled_cls {
                        if let AstNode::FuncDef {
                            name: mname,
                            params: mparams,
                            ret: mret,
                            ..
                        } = &b
                        {
                            let mq = format!("{}::{}", mc, mname);
                            let mp: Vec<(String, Type)> = mparams
                                .iter()
                                .map(|(n, t)| {
                                    (
                                        n.clone(),
                                        self.typed_param_type(&mq, n, t, &[]),
                                    )
                                })
                                .collect();
                            let mr = self.string_to_type(mret);
                            self.funcs.insert(mq.clone(), (mp, mr, false));
                            let mut mangled_ast = b.clone();
                            if let AstNode::FuncDef {
                                ref mut name,
                                ref mut params,
                                ..
                            } = mangled_ast
                            {
                                *name = mq.clone();
                                // Batch 651: also rewrite the `self` parameter
                                // type to use the mangled class name, so the
                                // receiver type in the method body matches the
                                // mangled struct (not the bare name that
                                // resolves to the wrong module's class).
                                if let Some((_, self_ty)) = params.first_mut() {
                                    if self_ty == &base_ty {
                                        *self_ty = mc.clone();
                                    }
                                }
                            }
                            self.registered_funcs.insert(mq, mangled_ast);
                        }
                    }
                }
            }
            AstNode::ConceptDef { methods, .. } => {
                for m in methods {
                    self.register(m);
                }
            }
            AstNode::Method {
                name, params, ret, ..
            } => {
                // Convert string types to Type enum
                let typed_params: Vec<(String, Type)> = params
                    .iter()
                    .map(|(pname, ty_str)| {
                        (
                            pname.clone(),
                            self.typed_param_type(&name, pname, ty_str, &[]),
                        )
                    })
                    .collect();
                let typed_ret = self.string_to_type(&ret);
                self.funcs.insert(name, (typed_params, typed_ret, false));
            }
            AstNode::ConstDef {
                ref name,
                ref ty,
                ref value,
                attrs: _,
                pub_: _,
                comptime_: _,
            } => {
                // Register constant for compile-time evaluation
                // Try to convert AST value to ConstValue
                if let Some(const_val) = self.ast_to_const_value(value) {
                    self.ctfe_consts.insert(name.clone(), const_val);
                }
                // Also register as a function-like entity for name resolution
                let typed_ret = self.string_to_type(ty);
                self.funcs.insert(name.clone(), (vec![], typed_ret, false));
            }
            AstNode::EnumDef {
                name,
                variants,
                ref generics,
                ..
            } => {
                // Register enum and its variants
                // For now, we'll register each variant as a function-like entity
                for (variant_name, _) in variants {
                    let full_name = name.clone() + "::" + &variant_name;
                    // Type::Named with generic type variables so lookup can match
                    let generic_vars: Vec<Type> = generics
                        .iter()
                        .enumerate()
                        .filter_map(|(i, g)| match g {
                            GenericParam::Type { .. } => Some(Type::Variable(TypeVar(i as u32))),
                            _ => None,
                        })
                        .collect();
                    let typed_ret = Type::Named(name.clone(), generic_vars);
                    self.funcs.insert(full_name, (vec![], typed_ret, false));
                }
            }
            AstNode::ModDef {
                name: module_name,
                items,
                pub_,
                ..
            } => {
                // Register module and its items
                // Register all items in the module with module-qualified names
                for item in items {
                    // Create a copy with module-qualified name if needed
                    let qualified_item = match &item {
                        AstNode::FuncDef {
                            name: func_name, ..
                        } => {
                            let mut new_item = item.clone();
                            if let AstNode::FuncDef { ref mut name, .. } = new_item {
                                *name = format!("{}::{}", module_name, func_name);
                            }
                            new_item
                        }
                        AstNode::EnumDef {
                            name: enum_name, ..
                        } => {
                            let mut new_item = item.clone();
                            if let AstNode::EnumDef { ref mut name, .. } = new_item {
                                // For enums, we need to register the enum itself and its variants
                                *name = format!("{}::{}", module_name, enum_name);
                            }
                            new_item
                        }
                        AstNode::StructDef {
                            name: struct_name, ..
                        } => {
                            let mut new_item = item.clone();
                            if let AstNode::StructDef { ref mut name, .. } = new_item {
                                *name = format!("{}::{}", module_name, struct_name);
                            }
                            new_item
                        }
                        AstNode::TypeAlias {
                            name: alias_name, ..
                        } => {
                            let mut new_item = item.clone();
                            if let AstNode::TypeAlias { ref mut name, .. } = new_item {
                                *name = format!("{}::{}", module_name, alias_name);
                            }
                            new_item
                        }
                        AstNode::ConstDef {
                            name: const_name,
                            comptime_,
                            ..
                        } => {
                            let mut new_item = item.clone();
                            if let AstNode::ConstDef { ref mut name, .. } = new_item {
                                *name = format!("{}::{}", module_name, const_name);
                            }
                            new_item
                        }
                        _ => item.clone(),
                    };
                    self.register(qualified_item);
                }
            }
            AstNode::MacroDef { name, patterns } => {
                // Parse and register the macro
                match crate::frontend::macro_expand::parse_macro_rules(&patterns) {
                    Ok(macro_def) => {
                        self.macro_expander
                            .register_declarative_macro(name.clone(), macro_def);
                    }
                    Err(e) => {
                        crate::diag_warning!("W0004", "Failed to parse macro {}: {}", name, e);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn resolve_impl(&self, concept: &str, ty: &str) -> Option<Vec<AstNode>> {
        self.impls
            .get(&(concept.to_string(), ty.to_string()))
            .cloned()
    }

    /// Get function signature for type checking
    #[allow(clippy::type_complexity)]
    pub fn get_func_signature(&self, name: &str) -> Option<&(Vec<(String, Type)>, Type, bool)> {
        let result = self.funcs.get(name);
        result.is_none();
        result
    }

    /// Get all function signatures (for type inference)
    pub fn get_all_func_signatures(&self) -> &HashMap<String, (Vec<(String, Type)>, Type, bool)> {
        &self.funcs
    }

    /// B3: list `(func, param)` where the registered type is `PyDynamic`.
    pub fn report_untyped_params(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (fname, (params, _ret, _)) in &self.funcs {
            for (pname, ty) in params {
                if matches!(ty, Type::PyDynamic) {
                    out.push((fname.clone(), pname.clone()));
                }
            }
        }
        out.sort();
        out
    }

    /// 批次 400（类型基础③）：`"函数.参数"` → 该参数在各调用点看到的**互相矛盾**的
    /// 实参类形。这些位置现在（正确地）保持动态，报告要把它们和"从来没遇到过任何
    /// 证据"的动态位置区分开 —— 前者才是"最小类型检查"该出声的地方。
    pub fn ambiguous_dyn_params(&self) -> std::collections::HashMap<String, String> {
        self.ambiguous_dyn_params.borrow().clone()
    }

    /// PY-A: infer the return type of untyped Python-style functions.
    ///
    /// `def f(s): return s.capitalize()` is declared i64 by the parser, so a
    /// library function returning a string was mis-typed at EVERY call site:
    /// the value was right but printing/comparison treated it as an integer.
    /// Conservative evidence-only inference:
    ///   - a definite string source (string literal, f-string, a call to a
    ///     function already known to return str, or a str method) => str;
    ///   - a definite float source => f64;
    ///   - any int/bool evidence, or no evidence at all, keeps the i64 default.
    /// Iterated a few times so callees propagate into their callers.
    pub fn infer_untyped_returns(&mut self, _asts: &[AstNode]) {
        // Includes functions from imported modules: they were registered
        // through the same path, so their bodies are here too.
        let asts: Vec<std::rc::Rc<AstNode>> = self.registered_func_defs.borrow().clone();
        fn collect_returns(body: &[AstNode], out: &mut Vec<AstNode>) {
            for s in body {
                match s {
                    AstNode::Return(e) => out.push((**e).clone()),
                    AstNode::If { then, else_, .. } => {
                        collect_returns(then, out);
                        collect_returns(else_, out);
                    }
                    AstNode::While { body, .. } | AstNode::For { body, .. } => {
                        collect_returns(body, out);
                    }
                    AstNode::Block { body } => collect_returns(body, out),
                    // Batch 758 (#38①): match arms carry returns too — a
                    // `case pat:` block recurses (its Returns are the
                    // evidence); an `=> expr` body IS the returned value.
                    AstNode::Match { arms, .. } => {
                        for arm in arms {
                            match &*arm.body {
                                AstNode::Block { body } => {
                                    collect_returns(body, out);
                                    // Batch 758 (#38①): a trailing bare
                                    // expression IS the arm's value (the
                                    // promoted ret_expr is the fn tail) —
                                    // `case 5: "five"` returns "five".
                                    if let Some(last) = body.last() {
                                        let is_stmt = matches!(
                                            last,
                                            AstNode::Return(_)
                                                | AstNode::Assign(..)
                                                | AstNode::Let { .. }
                                                | AstNode::While { .. }
                                                | AstNode::For { .. }
                                                | AstNode::If { .. }
                                                | AstNode::Match { .. }
                                        );
                                        if !is_stmt {
                                            match last {
                                                AstNode::ExprStmt { expr } => {
                                                    out.push((**expr).clone())
                                                }
                                                _ => out.push((*last).clone()),
                                            }
                                        }
                                    }
                                }
                                AstNode::Return(_) => {
                                    collect_returns(
                                        std::slice::from_ref(&*arm.body),
                                        out,
                                    );
                                }
                                other => out.push((*other).clone()),
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Batch 291: names that receive a DICT literal anywhere in the body
        // (`names = {} ; ... ; return names`). Used to infer a `-> dict`
        // return type for unannotated helpers like `_load_etf_names` — without
        // it the caller typed the result I64 and `len(d)` became `array_len`
        // on a map handle (dict1.z printed 0 for a 3-entry dict).
        fn collect_map_locals(body: &[AstNode], out: &mut std::collections::HashSet<String>) {
            for s in body {
                match s {
                    AstNode::Assign(lhs, rhs) => {
                        if let (AstNode::Var(n), AstNode::DictLit { .. }) = (&**lhs, &**rhs) {
                            out.insert(n.clone());
                        }
                    }
                    AstNode::Let { pattern, expr, .. } => {
                        if let (AstNode::Var(n), AstNode::DictLit { .. }) =
                            (&**pattern, &**expr)
                        {
                            out.insert(n.clone());
                        }
                    }
                    AstNode::If { then, else_, .. } => {
                        collect_map_locals(then, out);
                        collect_map_locals(else_, out);
                    }
                    AstNode::While { body, .. } | AstNode::For { body, .. } => {
                        collect_map_locals(body, out);
                    }
                    AstNode::Block { body } => collect_map_locals(body, out),
                    _ => {}
                }
            }
        }
        // Evidence enum: 1 = str, 2 = f64, 3 = i64, 0 = unknown.
        fn classify(
            e: &AstNode,
            funcs: &HashMap<String, (Vec<(String, Type)>, Type, bool)>,
            prefix: Option<&str>,
            module_aliases: &HashMap<String, String>,
            current_params: &[(String, Type)],
        ) -> u8 {
            match e {
                AstNode::StringLit(_) | AstNode::FString { .. } => 1,
                // PY-A: the ternary `A if C else B` desugars to an `If` node of
                // two one-expression blocks, so classification has to look at
                // the branch VALUES. Otherwise `return "big" if a > 3 else
                // "small"` was classified i64 and callers printed a pointer.
                AstNode::If { then, else_, .. } => {
                    let probe = |b: &Vec<AstNode>| -> u8 {
                        for s in b.iter().rev() {
                            match s {
                                AstNode::ExprStmt { expr } => {
                                    return classify(
                                        expr,
                                        funcs,
                                        prefix,
                                        module_aliases,
                                        current_params,
                                    )
                                }
                                AstNode::Return(e) => {
                                    return classify(
                                        e,
                                        funcs,
                                        prefix,
                                        module_aliases,
                                        current_params,
                                    )
                                }
                                // A body statement is often a BARE expression
                                // (the `ExprStmt` wrapper is normalized away),
                                // so classify it directly — otherwise a ternary
                                // `"a" if c else "b"` classified as unknown (0)
                                // and the caller printed a pointer as i64.
                                other => {
                                    return classify(
                                        other,
                                        funcs,
                                        prefix,
                                        module_aliases,
                                        current_params,
                                    )
                                }
                            }
                        }
                        0
                    };
                    let t = probe(then);
                    if t != 0 {
                        t
                    } else {
                        probe(else_)
                    }
                }
                AstNode::FloatLit(_) => 2,
                AstNode::Lit(_) | AstNode::Bool(_) => 3,
                AstNode::Call {
                    receiver: None,
                    method,
                    ..
                } => {
                    // Inside an imported module the callee is registered as
                    // `mod__name`, but the source says `name`.
                    let direct = funcs.get(method).map(|(_, ret, _)| ret);
                    let prefixed = prefix
                        .and_then(|p| funcs.get(&format!("{}{}", p, method)))
                        .map(|(_, ret, _)| ret);
                    match direct.or(prefixed) {
                        Some(Type::Str) => 1,
                        Some(Type::F64) => 2,
                        Some(Type::I64) => 3,
                        _ => 0,
                    }
                }
                AstNode::Call {
                    receiver: Some(recv),
                    method,
                    ..
                } => {
                    if method == "__slice__" {
                        if let AstNode::Var(v) = &**recv {
                            if current_params
                                .iter()
                                .any(|(n, t)| n == v && *t == Type::Str)
                            {
                                return 1;
                            }
                        }
                    }
                    // `re.sub(...)` / `os.path.join(...)`: a registry member
                    // that is declared to return a string.
                    if let AstNode::Var(alias) = &**recv {
                        if let Some(module) = module_aliases.get(alias) {
                            if let Some(entry) =
                                crate::middle::pylib::find_member(module, method)
                            {
                                return match entry.ret.as_str() {
                                    "str" => 1,
                                    "f64" => 2,
                                    _ => 0,
                                };
                            }
                        }
                    }
                    match str_method_symbol_kind(method) {
                        Some("str") => 1,
                        Some("bool") => 3,
                        _ => 0,
                    }
                }
                // `s[0]` / `s[1:]` where `s` is a parameter already inferred to
                // be a string yields a string.
                AstNode::Subscript { base, .. } => {
                    if let AstNode::Var(v) = &**base {
                        if current_params
                            .iter()
                            .any(|(n, t)| n == v && *t == Type::Str)
                        {
                            return 1;
                        }
                    }
                    0
                }
                AstNode::BinaryOp { op, left, right } if op == "+" => {
                    let l = classify(left, funcs, prefix, module_aliases, current_params);
                    let r = classify(right, funcs, prefix, module_aliases, current_params);
                    if l == 3 || r == 3 {
                        3
                    } else if l == 1 || r == 1 {
                        1
                    } else if l == 2 || r == 2 {
                        2
                    } else {
                        0
                    }
                }
                // 批次 652：数组字面量（`[1,2,3]` / `DynamicArrayLit`）返回 kind=4，
                // 让参数证据链里数组可见。旧代码 `_ => 0` 让数组永远不触发冲突检测
                // ⇒ `len(cache["k"])` 与 `len([1,2,3])` 共享参数时，数组那侧的证据
                // 被吞，参数被单方面钉成 Str ⇒ len() 走 str_len 路径 ⇒ 错值。
                AstNode::ArrayLit(_) | AstNode::DynamicArrayLit { .. } => 4,
                _ => 0,
            }
        }
        // Each pass propagates evidence one call level deeper (main ->
        // snakecase -> lowercase -> ...), so give the chain room.
        // 批次 400（类型基础③）：一条参数只有在**所有**调用点类形一致时才钉。
        // `pinned` = 本函数自己钉过的下标（冲突时只回退这些，用户手写的注解不动）；
        // `conflicts` 一旦记入就永久拒绝再钉，否则下一个 pass 会把它钉回去。
        let mut pinned: std::collections::HashSet<(String, usize)> =
            std::collections::HashSet::new();
        let mut conflicts: std::collections::HashSet<(String, usize)> =
            std::collections::HashSet::new();
        for _ in 0..6 {
            for ast in &asts {
                let ast: &AstNode = ast;
                let AstNode::FuncDef {
                    name,
                    body,
                    ret,
                    ret_expr,
                    ..
                } = ast
                else {
                    continue;
                };
                let prefix = self
                    .py_mangled_to_module
                    .borrow()
                    .get(name)
                    .map(|m| format!("{}__", m.replace('.', "_")));
                // `def f():` has no annotation, so the parser defaults the
                // return type to `()` (unit) — that marks a function as
                // inferable for its RETURN. Call-site evidence for PARAMETERS
                // must be collected from every function, including ones with
                // an explicit return type (e.g. the synthesized `main`, which
                // is where top-level call sites live).
                let infer_return = (ret.is_empty() || ret == "()")
                    && self.funcs.contains_key(name);
                let mut rets: Vec<AstNode> = Vec::new();
                let mut map_locals: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                if infer_return {
                    collect_returns(body, &mut rets);
                    // Batch 758 (#38①): parse_func promotes the body's
                    // trailing match into `ret_expr` and leaves `body` empty
                    // — the arms carry the returns (measured: a match-bodied
                    // unannotated fn registered Tuple([]) and callers of its
                    // string returns printed addresses).
                    if let Some(rx) = ret_expr {
                        collect_returns(std::slice::from_ref(rx), &mut rets);
                    }
                    collect_map_locals(body, &mut map_locals);
                }
                let mut saw_str = false;
                let mut saw_f64 = false;
                let mut saw_i64 = false;
                let mut saw_map = false;
                // 批次 806（#113 携带读面）：`return None` 的 None 性在 AST
                // 可见（NoneLit）——纯 None 返回 ⇒ NoneValue（print 按型渲染，
                // 804 fromkeys 同款）。混型（None∨其它）不动，落弃权。
                let saw_none = rets.iter().any(|r| matches!(r, AstNode::NoneLit));
                let aliases = self.py_module_aliases.borrow().clone();
                for r in &rets {
                    if matches!(r, AstNode::DictLit { .. })
                        || matches!(r, AstNode::Var(v) if map_locals.contains(v.as_str()))
                        // 批次 801（#409(a)①）：`return dict(x)` 收进来是构造
                        // 调用而非 DictLit——classify 落 0、saw_map 不亮，返回型
                        // 停默认，调用点 `len()` 对 map 句柄派了 array_len（读
                        // vec 表头答 0，实拍 len(d1)=0 而 d1["a"]=1）。dict 构造
                        // 调用恒产 map，无 union 投毒面。
                        || matches!(r, AstNode::Call { receiver: None, method, .. } if method == "dict")
                        || matches!(r, AstNode::ExprStmt { expr }
                            if matches!(**expr, AstNode::Call { receiver: None, method: ref m2, .. } if m2 == "dict"))
                    {
                        saw_map = true;
                    }
                    let cur_params: Vec<(String, Type)> = self
                        .funcs
                        .get(name)
                        .map(|(p, _, _)| p.clone())
                        .unwrap_or_default();
                    match classify(r, &self.funcs, prefix.as_deref(), &aliases, &cur_params) {
                        1 => saw_str = true,
                        2 => saw_f64 = true,
                        3 => saw_i64 = true,
                        _ => {}
                    }
                }
                let new_ret = if saw_map && !saw_str && !saw_i64 && !saw_f64 {
                    Some(Type::Named("map".to_string(), vec![]))
                } else if saw_str && !saw_i64 && !saw_f64 && !saw_map {
                    // 批次 802（#117）：补 !saw_map——map ∨ str 真 union 原先
                    // 落进这条 Str 臂把 dict 侧投毒（m["a"] 派 str_get 把 map
                    // 句柄当 char* 读，实拍空行、len 打 1）。
                    Some(Type::Str)
                } else if saw_f64 && !saw_i64 && !saw_str {
                    Some(Type::F64)
                } else if saw_map {
                    // 批次 802（#117）：map ∨ 其它 = 真 union——弃权不等于停
                    // 默认。定型 PyDynamic：读边界按运行期句柄形分派（字符串
                    // 键下标走 DictGet、len 走 zeta_dyn_len 几何形判），map/str
                    // 两面都活。
                    Some(Type::PyDynamic)
                } else if saw_none && !saw_str && !saw_f64 && !saw_i64 && !saw_map {
                    // 批次 806（#113 携带读面）：纯 None 返回 ⇒ NoneValue——
                    // 调用点槽带型，print 按型渲染 "None"（值仍 i64 0）。
                    Some(Type::Named("NoneValue".to_string(), vec![]))
                } else {
                    None
                };
                if infer_return {
                    if let Some(t) = new_ret {
                        if let Some(entry) = self.funcs.get_mut(name) {
                            entry.1 = t;
                        }
                    }
                }

                // Pass B: parameter types from call-site evidence. Python
                // parameters are unannotated, so the parser defaults them to
                // i64 — which made `s[0]` / `s[1:]` / string methods on a
                // parameter fall into the ARRAY path (garbage, and an absurd
                // allocation). Strong evidence only: a parameter that is
                // passed a string literal (or a value known to be str/f64) at
                // some call site is that type.
                let aliases2 = self.py_module_aliases.borrow().clone();
                let mut calls: Vec<(String, Vec<AstNode>)> = Vec::new();
                fn collect_calls(body: &[AstNode], out: &mut Vec<(String, Vec<AstNode>)>) {
                    for s in body {
                        match s {
                            AstNode::Call {
                                receiver: None,
                                method,
                                args,
                                ..
                            } => {
                                out.push((method.clone(), args.clone()));
                                // Recurse into the arguments: the call-site
                                // evidence for a parameter usually sits inside
                                // another call, e.g. print(f("literal")).
                                collect_calls(args, out);
                            }
                            AstNode::If { cond, then, else_ } => {
                                collect_calls(std::slice::from_ref(cond.as_ref()), out);
                                collect_calls(then, out);
                                collect_calls(else_, out);
                            }
                            AstNode::While {
                                cond,
                                body,
                                else_body,
                            } => {
                                collect_calls(std::slice::from_ref(cond.as_ref()), out);
                                collect_calls(body, out);
                                collect_calls(else_body, out);
                            }
                            AstNode::For { body, else_body, .. } => {
                                collect_calls(body, out);
                                collect_calls(else_body, out);
                            }
                            AstNode::Block { body } => collect_calls(body, out),
                            AstNode::ExprStmt { expr } => {
                                collect_calls(std::slice::from_ref(expr.as_ref()), out)
                            }
                            // The call site is very often inside a `return`
                            // (`return lowercase(s[0]) + ...`) — without these
                            // arms the evidence chain never left `main`.
                            AstNode::Return(e) => {
                                collect_calls(std::slice::from_ref(e.as_ref()), out)
                            }
                            AstNode::Let { expr, .. } => {
                                collect_calls(std::slice::from_ref(expr.as_ref()), out)
                            }
                            AstNode::Assign(lhs, rhs) => {
                                collect_calls(std::slice::from_ref(lhs.as_ref()), out);
                                collect_calls(std::slice::from_ref(rhs.as_ref()), out);
                            }
                            AstNode::BinaryOp { left, right, .. } => {
                                collect_calls(std::slice::from_ref(left.as_ref()), out);
                                collect_calls(std::slice::from_ref(right.as_ref()), out);
                            }
                            // Batch 407: an f-string is a LEAF everywhere else in
                            // this file (`:1794`, `infer` at `:2937` and the AST-text
                            // reader all answer `Type::Str` without descending), so a
                            // call site that only ever appears inside an
                            // interpolation contributed NO parameter evidence.
                            AstNode::FString(parts) => collect_calls(parts, out),
                            _ => {}
                        }
                    }
                }
                collect_calls(body, &mut calls);
                for (callee, pargs) in calls {
                    // Resolve the callee name the same three ways the MIR
                    // does: a plain registered function, a `from X import f`
                    // alias (X__f), or a module-local bare call inside an
                    // imported module (prefix + name).
                    let callee = {
                        if self.funcs.contains_key(&callee) {
                            callee
                        } else if let Some((module, member)) =
                            self.py_member_aliases.borrow().get(&callee)
                        {
                            format!("{}__{}", module.replace('.', "_"), member)
                        } else if let Some(p) = prefix.as_deref() {
                            let cand = format!("{}{}", p, callee);
                            if self.funcs.contains_key(&cand) {
                                cand
                            } else {
                                callee
                            }
                        } else {
                            callee
                        }
                    };
                    let mut param_types: Vec<Type> = match self.funcs.get(&callee) {
                        Some((params, _, _)) => params
                            .iter()
                            .map(|(_, t)| t.clone())
                            .collect(),
                        None => continue,
                    };
                    let mut changed: Vec<(usize, Type)> = Vec::new();
                    let member_aliases = self.py_member_aliases.borrow().clone();
                    for (i, a) in pargs.iter().enumerate() {
                        if i >= param_types.len() {
                            continue;
                        }
                        let cur_params2: Vec<(String, Type)> = self
                            .funcs
                            .get(name)
                            .map(|(p, _, _)| p.clone())
                            .unwrap_or_default();
                        // 批次 400（类型基础③）：实参类形要在"这条参数还钉不钉得动"
                        // 那道闸门**之前**取。上一个 pass 已把它钉成 str 时旧代码直接
                        // 跳过整条实参，另一个调用点的 i64 证据就永远看不见 —— 于是
                        // 一处调用点单方面决定了全体调用点的接收槽（c3.z：`show(3)` 的
                        // dest 槽 type_map 判为 Str、走 `println_str` ⇒ SIGSEGV）。
                        let mut kind = match classify(
                            a,
                            &self.funcs,
                            prefix.as_deref(),
                            &aliases2,
                            &cur_params2,
                        ) {
                            1 => "str",
                            2 => "f64",
                            3 => "i64",
                            // 批次 652：数组字面量参与冲突检测。"array" 与 "str"/"handle"
                            // 算跨族（clash 触发 ⇒ 参数退 PyDynamic），与 i64/f64 也算跨族。
                            4 => "array",
                            _ => "",
                        };
                        // PY-A: a param that receives a library HANDLE value
                        // (`f(datetime.date(2020,1,1))`, or `f(date(…))` after
                        // `from datetime import date`) must be typed as that
                        // handle, or every attribute/method inside the callee
                        // degrades. Both call shapes count: dotted
                        // `Root.member(…)` and a bare from-imported `member(…)`.
                        let mut handle: Option<String> = None;
                        if let AstNode::Call { receiver, method: m, .. } = a {
                            let target = match receiver {
                                None => member_aliases.get(m).cloned(),
                                Some(r) => match &**r {
                                    AstNode::Var(root) => Some((
                                        aliases2
                                            .get(root)
                                            .cloned()
                                            .unwrap_or_else(|| root.clone()),
                                        m.clone(),
                                    )),
                                    _ => None,
                                },
                            };
                            if let Some((module, member)) = target {
                                if let Some(tag) =
                                    crate::middle::pylib::find_member(&module, &member)
                                        .and_then(|e| e.handle.clone())
                                {
                                    handle = Some(tag);
                                }
                            }
                        }
                        // 句柄之间不算冲突（同一条参数可以收 `date(...)` 也可以收
                        // `timedelta(...)`，钉住任一都比退化成动态强）；句柄与非句柄
                        // 实参并存才算。
                        if handle.is_some() {
                            kind = "handle";
                        }
                        // B3: unannotated params are PyDynamic (was I64). Both
                        // remain upgradeable from call-site evidence.
                        if !matches!(param_types[i], Type::I64 | Type::PyDynamic) {
                            let pinned_kind = match param_types[i] {
                                Type::Str => "str",
                                Type::F64 => "f64",
                                Type::Named(..) => "handle",
                                _ => "",
                            };
                            // i64 与 f64 算**同族**：钉 f64 后整数实参在调用点做
                            // sitofp，两个调用点的值都正确（实测：`twice(3)`/`twice(1.5)`
                            // 走 f64 钉时输出 6.000000/3.000000）。跨族（str×数值、
                            // 句柄×其余）才是真冲突 —— 值身上没有类型标记，一种签名
                            // 不可能同时正确服务两类实参。
                            let numeric = |k: &str| k == "i64" || k == "f64";
                            let clash = !pinned_kind.is_empty()
                                && !kind.is_empty()
                                && !(numeric(pinned_kind) && numeric(kind));
                            if clash
                                && pinned_kind != kind
                                && pinned.contains(&(callee.clone(), i))
                            {
                                let pname = self
                                    .funcs
                                    .get(&callee)
                                    .and_then(|(p, _, _)| p.get(i))
                                    .map(|(n, _)| n.clone())
                                    .unwrap_or_default();
                                let first = self
                                    .ambiguous_dyn_params
                                    .borrow_mut()
                                    .insert(
                                        format!("{}.{}", callee, pname),
                                        format!("{}≠{}", pinned_kind, kind),
                                    )
                                    .is_none();
                                if first {
                                    // 6 个 pass、多个调用点都会重复撞上同一处，只在
                                    // 第一次出声。
                                    eprintln!(
                                        "warning: PY-A: parameter `{}.{}` receives \
                                         incompatible argument kinds at different call \
                                         sites ({} vs {}) — kept dynamic: one LLVM \
                                         signature cannot serve both, so the value of \
                                         the other kind travels through the integer slot. \
                                         Annotate the parameter to settle it.",
                                        callee, pname, pinned_kind, kind
                                    );
                                }
                                conflicts.insert((callee.clone(), i));
                                pinned.remove(&(callee.clone(), i));
                                if let Some(entry) = self.funcs.get_mut(&callee) {
                                    if i < entry.0.len() {
                                        entry.0[i].1 = Type::PyDynamic;
                                    }
                                }
                                // MIR 从 AST 文本读参数类型，所以两边都要回退。
                                if let Some(AstNode::FuncDef { params, .. }) =
                                    self.registered_funcs.get_mut(&callee)
                                {
                                    if i < params.len() {
                                        params[i].1 = "dyn".to_string();
                                    }
                                }
                            }
                            continue;
                        }
                        if !kind.is_empty() && conflicts.contains(&(callee.clone(), i)) {
                            continue;
                        }
                        match kind {
                            "str" => changed.push((i, Type::Str)),
                            "f64" => changed.push((i, Type::F64)),
                            // 批次 652：数组字面量钉成 DynamicArray(I64)。与 Str 跨族 ⇒
                            // 下一调用点若传 str 会触发 clash ⇒ 退 PyDynamic ⇒ len()
                            // 走 zeta_dyn_len（本批注册）而不是 str_len。
                            "array" => changed.push((i, Type::DynamicArray(Box::new(Type::I64)))),
                            _ => {}
                        }
                        if let Some(tag) = handle {
                            changed.push((i, Type::Named(tag, vec![])));
                        }
                    }
                    for (i, t) in &changed {
                        param_types[*i] = t.clone();
                        // 本函数钉的下标 —— 回退只针对这些，用户注解过的不动。
                        pinned.insert((callee.clone(), *i));
                    }
                    let changed_pairs = changed.clone();
                    let changed = !changed.is_empty();
                    if changed {
                        let types_snapshot = param_types.clone();
                        let changed_types: Vec<(usize, Type)> = changed_pairs.clone();
                        if let Some(entry) = self.funcs.get_mut(&callee) {
                            for (i, t) in param_types.into_iter().enumerate() {
                                if i < entry.0.len() {
                                    entry.0[i].1 = t;
                                }
                            }
                        }
                        // MIR lowering reads the parameter type from the AST
                        // string, so rewrite it there too — updating only the
                        // signature table had no effect on the generated code.
                        // ONLY the upgraded indices: rewriting untouched ones
                        // turned e.g. an array parameter into "array(...)".
                        if let Some(AstNode::FuncDef { params, .. }) =
                            self.registered_funcs.get_mut(&callee)
                        {
                            for (i, t) in changed_types.iter() {
                                if *i < params.len() {
                                    params[*i].1 = match t {
                                        Type::Str => "str".to_string(),
                                        Type::F64 => "f64".to_string(),
                                        // library handle tags are valid type texts;
                                        // MIR now maps them to Type::Named(handle).
                                        Type::Named(n, _) => n.clone(),
                                        _ => continue,
                                    };
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// PY-A: static type of each module-level global. The module's statements
    /// live in the synthesized `main`, so this scans every registered function
    /// body for assignments to names that became module globals.

    pub fn module_global_types(&self) -> HashMap<String, Type> {
        self.module_global_types_at("direct")
    }

    fn module_global_types_at(&self, site: &str) -> HashMap<String, Type> {
        // Batch 738: the s5195 site (lower_to_mir's builder) runs once per
        // function/closure — 651 identical full-AST walks on jq_wufu_local
        // (= 42s, profiled: 72% of compile time in AstNode clone/drop). The
        // module-level assignment set is FIXED once lowering begins, so the
        // map is memoized for this site; the other two call sites (refine
        // passes) may see a still-growing table and keep recomputing.
        if site == "s5195" {
            if let Some(hit) = self.mgt_cache.borrow().as_ref() {
                return hit.clone();
            }
            let computed = self.module_global_types_uncached();
            *self.mgt_cache.borrow_mut() = Some(computed.clone());
            return computed;
        }
        self.module_global_types_uncached()
    }

    fn module_global_types_uncached(&self) -> HashMap<String, Type> {
    /// Infer the MIR type of a module-level expression. `seen` holds the types of
    /// globals defined EARLIER in the same module (so aliases and constant chains
    /// resolve in declaration order); `aliases` maps import aliases to modules.
    fn infer_global_ty(
        n: &AstNode,
        seen: &HashMap<String, Type>,
        aliases: &HashMap<String, String>,
        member_aliases: &HashMap<String, (String, String)>,
        fn_rets: &HashMap<String, Type>,
        classes: &[String],
    ) -> Option<Type> {
        use crate::middle::pylib;
        match n {
            AstNode::StringLit(_) | AstNode::FString { .. } => Some(Type::Str),
            AstNode::FloatLit(_) => Some(Type::F64),
            AstNode::Bool(_) => Some(Type::Bool),
            AstNode::Lit(_) => Some(Type::I64),
            // Batch 647: a beyond-i64 literal types the global BigInt —
            // reads route through the zeta_big runtime family.
            AstNode::BigIntLit(_) => Some(Type::Named("BigInt".to_string(), vec![])),
            AstNode::DictLit { entries } => Some(Type::Named(
                "map".to_string(),
                match entries.first() {
                    Some((k, v)) => vec![
                        infer_global_ty(k, seen, aliases, member_aliases, fn_rets, classes)
                            .unwrap_or(Type::Str),
                        infer_global_ty(v, seen, aliases, member_aliases, fn_rets, classes)
                            .unwrap_or(Type::I64),
                    ],
                    None => vec![],
                },
            )),
            AstNode::ArrayLit(items) => {
                // Element type from the FIRST element: a str list typed
                // `DynamicArray(I64)` made every later consumer treat the elements
                // as integers — `dict.fromkeys(CODES + ["y"])` then passed
                // `keys_are_str = false`, inserted the keys UNHASHED, and
                // `list(...)` returned duplicates (dedup lost) — which broke
                // `wufu_constants`' universe registration.
                let elem = items
                    .first()
                    .and_then(|e| infer_global_ty(e, seen, aliases, member_aliases, fn_rets, classes))
                    .unwrap_or(Type::I64);
                Some(Type::DynamicArray(Box::new(elem)))
            }
            AstNode::DynamicArrayLit { elem_type, .. } => {
                Some(Type::DynamicArray(Box::new(Type::from_string(elem_type))))
            }
            // An alias to an earlier global keeps that global's type
            // (`_CACHE = _LISTING_CACHE`).
            AstNode::Var(v) => seen.get(v).cloned(),
            // `A / B` — pathlib join (and, later, other handle operators).
            AstNode::BinaryOp { op, left, right } => {
                let lt = infer_global_ty(left, seen, aliases, member_aliases, fn_rets, classes)?;
                let rt = match &**right {
                    AstNode::StringLit(_) => Type::Str,
                    other => infer_global_ty(other, seen, aliases, member_aliases, fn_rets, classes)?,
                };
                let (lname, rname) = match (&lt, &rt) {
                    (Type::Named(l, _), Type::Named(r, _)) => (l.clone(), r.clone()),
                    (Type::Named(l, _), Type::Str) => (l.clone(), "str".to_string()),
                    _ => return None,
                };
                pylib::handle_op(op, &lname, &rname).map(|(_sym, kind)| match kind {
                    "path" => Type::Named("PyPath".to_string(), vec![]),
                    "date" => Type::Named("PyDate".to_string(), vec![]),
                    "delta" => Type::Named("PyDelta".to_string(), vec![]),
                    "bool" => Type::Bool,
                    _ => Type::I64,
                })
            }
            // `p.parents[2]` — index into a Vec-returning attribute.
            AstNode::Subscript { base, .. } => match infer_global_ty(base, seen, aliases, member_aliases, fn_rets, classes)? {
                Type::DynamicArray(e) => Some(*e),
                Type::Tuple(ts) => ts.first().cloned(),
                _ => None,
            },
            // `handle.attr` — a W-table method used as a property (Path.parents).
            AstNode::FieldAccess { base, field } => {
                let b = infer_global_ty(base, seen, aliases, member_aliases, fn_rets, classes)?;
                match b {
                    Type::Named(tag, _) => method_result_ty(&tag, field),
                    _ => None,
                }
            }
            // `<vec>.__collect__(<closure>)` — a LIST COMPREHENSION's desugaring
            // (`WUFU_BS_CODES = [jq_to_bs(c) for c in WUFU_JQ_CODES]`). Without
            // this the constant had NO type, stayed I64, and `LIST + LIST`
            // compiled as a NUMERIC add — the garbage handle went to
            // `dict.fromkeys` and `py_map_fromkeys` dereferenced it (50% SIGSEGV
            // per run, 100% with MallocScribble=1). A comprehension is ALWAYS a
            // list, so an unknown element type still yields DynamicArray.
            AstNode::Call {
                receiver: Some(recv),
                method,
                args,
                ..
            } if method == "__collect__" => {
                let elem = args
                    .first()
                    .and_then(|cl| match cl {
                        AstNode::Closure { body, .. } => infer_global_ty(
                            body,
                            seen,
                            aliases,
                            member_aliases,
                            fn_rets,
                            classes,
                        ),
                        other => infer_global_ty(
                            other,
                            seen,
                            aliases,
                            member_aliases,
                            fn_rets,
                            classes,
                        ),
                    })
                    .or_else(|| infer_global_ty(recv, seen, aliases, member_aliases, fn_rets, classes))
                    .unwrap_or(Type::I64);
                Some(Type::DynamicArray(Box::new(elem)))
            }
            // `X.Y(...)` / `Y(...)` through the registry, or `handle.method()`.
            AstNode::Call {
                receiver,
                method,
                args,
                ..
            } => {
                // Batch 291: `g = _G()` — a global built from a USER CLASS ctor
                // must keep the class type. With no entry here, every
                // cross-module `g.<field>` read fell back to I64, so
                // `g.fixed_etf_pool + [x]` compiled to a pointer ADD and the
                // garbage handle crashed `array_len` in `jq_wufu___load_etf_names`.
                // The program-wide decl table is keyed by the MANGLED name
                // (`jq_shim___G`), so also accept a unique `<mangling>__<Class>`
                // suffix match. Checked BEFORE the fn_rets lookup: the synthesized
                // ctor may be registered as a function returning I64, and a
                // `__<Class>` suffix hit would shadow the real struct type.
                if receiver.is_none() {
                    let hit = classes.iter().find(|c| *c == method).or_else(|| {
                        let suffix = format!("__{}", method);
                        let mut hits =
                            classes.iter().filter(|c| c.ends_with(suffix.as_str()));
                        let first = hits.next()?;
                        if hits.next().is_some() {
                            None
                        } else {
                            Some(first)
                        }
                    });
                    if let Some(c) = hit {
                        return Some(Type::Named(c.clone(), vec![]));
                    }
                }
                // A user function called by its BARE name: its declared return
                // type. `from ..datasrc.code_conv import jq_to_bs` may carry a
                // RELATIVE spec in `member_aliases`, so the `<module>__<member>`
                // key cannot be built here — accept a UNIQUE `__<name>` suffix
                // match. This is what types `[jq_to_bs(c) for c in WUFU_JQ_CODES]`
                // (the universe list is a list of STRINGS, not raw handles).
                if receiver.is_none() {
                    if let Some(t) = fn_rets.get(method) {
                        return Some(t.clone());
                    }
                    let suffix = format!("__{}", method);
                    let hits: Vec<Type> = fn_rets
                        .iter()
                        .filter(|(k, _)| k.ends_with(suffix.as_str()))
                        .map(|(_, v)| v.clone())
                        .collect();
                    // Two same-named helpers can exist (REasyQuant has BOTH
                    // `backend.datasrc.code_conv.jq_to_bs` and
                    // `backend.strategy.code_conv.jq_to_bs`); when every candidate
                    // agrees on the return type the answer is unambiguous.
                    if !hits.is_empty() && hits.iter().all(|t| *t == hits[0]) {
                        return Some(hits[0].clone());
                    }
                }
                if let Some(recv) = receiver {
                    // Batch 580: `sep.join(iterable)` on a Str receiver yields
                    // Str (the W table has no str/join row, so the infer fell
                    // through and tag_str()-style methods stayed I64-typed,
                    // printing the joined string's POINTER).
                    if method == "join"
                        && args.len() == 1
                        && matches!(
                            infer_global_ty(recv, seen, aliases, member_aliases, fn_rets, classes),
                            Some(Type::Str)
                        )
                    {
                        return Some(Type::Str);
                    }
                    if let Some(Type::Named(tag, _)) = infer_global_ty(recv, seen, aliases, member_aliases, fn_rets, classes) {
                        if let Some(t) = method_result_ty(&tag, method) {
                            return Some(t);
                        }
                        // Batch 601: a USER-CLASS method — fn_rets carries the
                        // recovered `Class::method` return (qualified-defs
                        // recovery, batch 600). The registry paths below cannot
                        // see user classes, so a module-level `d.speak()` fell
                        // through to None and printed the handle.
                        let q = format!("{}::{}", tag, method);
                        if let Some(t) = fn_rets.get(&q) {
                            return Some(t.clone());
                        }
                        let suffix = format!("::{}", method);
                        let hits: Vec<Type> = fn_rets
                            .iter()
                            .filter(|(k, _)| k.ends_with(suffix.as_str()))
                            .map(|(_, v)| v.clone())
                            .collect();
                        if !hits.is_empty() && hits.iter().all(|t| *t == hits[0]) {
                            return Some(hits[0].clone());
                        }
                    }
                    // `mod.member(...)` — a registry module member.
                    let mut parts: Vec<String> = Vec::new();
                    let mut cur: &AstNode = recv;
                    loop {
                        match cur {
                            AstNode::FieldAccess { base, field } => {
                                parts.push(field.clone());
                                cur = base;
                            }
                            AstNode::Var(root) => {
                                parts.push(root.clone());
                                break;
                            }
                            _ => return None,
                        }
                    }
                    parts.reverse();
                    let root = parts.remove(0);
                    let rest = if parts.is_empty() {
                        method.to_string()
                    } else {
                        format!("{}.{}", parts.join("."), method)
                    };
                    let module = aliases.get(&root)?;
                    let e = pylib::find_member(module, &rest)?;
                    return match (e.handle.as_deref(), e.ret.as_str()) {
                        (Some(h), _) => Some(Type::Named(h.to_string(), vec![])),
                        (None, "str") => Some(Type::Str),
                        (None, "f64") => Some(Type::F64),
                        _ => Some(Type::I64),
                    };
                }
                // A same-file function: its declared return type is the element
                // type of a comprehension built from it.
                if let Some(t) = fn_rets.get(method) {
                    return Some(t.clone());
                }
                // `Path(__file__)` — a member imported BY NAME
                // (`from pathlib import Path`). This is where a module-level
                // path constant starts, so it must be resolved before the
                // receiver-less case gives up.
                let (module, member) = member_aliases.get(method)?;
                // A USER function imported by name (`from ..datasrc.code_conv
                // import jq_to_bs`): its declared return type is what a
                // comprehension or call over it produces. Module-level constants
                // are built exactly that way (`WUFU_BS_CODES = [jq_to_bs(c) for c
                // in WUFU_JQ_CODES]`), and without this the constant had no type —
                // every later `LIST + LIST` became a NUMERIC add.
                let mangled = format!("{}__{}", module.replace('.', "_"), member);
                if let Some(t) = fn_rets.get(&mangled).or_else(|| fn_rets.get(member)) {
                    return Some(t.clone());
                }
                let e = pylib::find_member(module, member)?;
                match (e.handle.as_deref(), e.ret.as_str()) {
                    (Some(h), _) => Some(Type::Named(h.to_string(), vec![])),
                    (None, "str") => Some(Type::Str),
                    (None, "f64") => Some(Type::F64),
                    _ => Some(Type::I64),
                }
            }
            _ => None,
        }
    }

    /// The MIR type of a `W`-table method's declared result. Kept in ONE place:
    /// the same mapping had to be patched at four separate call sites in the MIR
    /// layer for `vecpath` (batch 153) — forgetting one silently typed the result
    /// I64 and every later method call on it became a bare symbol.
    fn method_result_ty(tag: &str, method: &str) -> Option<Type> {
        use crate::middle::pylib;
        if let Some((_sym, Some(h))) = pylib::method_symbol(tag, method) {
            return Some(Type::Named(h.to_string(), vec![]));
        }
        match pylib::method_ret(tag, method)? {
            "str" => Some(Type::Str),
            "f64" => Some(Type::F64),
            "vecstr" => Some(Type::DynamicArray(Box::new(Type::Str))),
            "vecpath" => Some(Type::DynamicArray(Box::new(Type::Named(
                "PyPath".to_string(),
                vec![],
            )))),
            "vecjson" => Some(Type::DynamicArray(Box::new(Type::Named(
                "PyJson".to_string(),
                vec![],
            )))),
            "vecmatch" => Some(Type::DynamicArray(Box::new(Type::Named(
                "PyMatch".to_string(),
                vec![],
            )))),
            "vec" => Some(Type::DynamicArray(Box::new(Type::I64))),
            // `pd.read_parquet(...)` — the reader returns the runtime's column
            // map (name -> vector of value strings), which is what the pandas
            // shim's `DataFrame(data: map<str, vecstr>)` wraps.
            "map" => Some(Type::Named(
                "map".to_string(),
                vec![
                    Type::Str,
                    Type::DynamicArray(Box::new(Type::Str)),
                ],
            )),
            _ => Some(Type::I64),
        }
    }
        let globals = self.module_globals.borrow().clone();
        let aliases = self.py_module_aliases.borrow().clone();
        let member_aliases = self.py_member_aliases.borrow().clone();
        // In a MULTI-module compile `module_globals` holds the MANGLED form for
        // imported modules (`backend_datasrc_etf_listing___LISTING_CACHE`) while
        // the walk below sees the BARE source name (`_LISTING_CACHE`) — so the
        // containment test never matched and the whole table came out EMPTY
        // (probe: defs=448 globals=372 typed=0), leaving every module-level path
        // constant untyped in the linked program (etf_listing's `_LISTING_CACHE`
        // → bare `_exists`). Accept both spellings.
        // Strip the KNOWN module prefix (`<module with _ for .>__`) rather than
        // splitting on "__": a global like `_PROJECT_ROOT` is stored as
        // `backend_datasrc_etf_listing___PROJECT_ROOT` (THREE underscores — the
        // separator's two plus the name's leading one), and `rsplit_once("__")`
        // would yield `PROJECT_ROOT` (leading underscore lost) so the lookup for
        // `_PROJECT_ROOT` still missed. That bug is why the table stayed empty
        // (typed=0) even after the first attempt.
        let prefixes: Vec<String> = self
            .py_loaded_modules
            .borrow()
            .iter()
            .map(|m| format!("{}__", m.replace('.', "_")))
            .collect();
        let bare_globals: std::collections::HashSet<String> = globals
            .iter()
            .map(|g| {
                for pfx in &prefixes {
                    if let Some(rest) = g.strip_prefix(pfx.as_str()) {
                        if !rest.is_empty() {
                            return rest.to_string();
                        }
                    }
                }
                g.clone()
            })
            .collect();
        let mut out: HashMap<String, Type> = HashMap::new();
        if crate::diagnostics::env_flag("ZETA_PROBE_GLOBALS") {
            eprintln!("GLOBALS probe: globals={} prefixes={:?}", globals.len(), prefixes);
        }
        fn walk(stmts: &[AstNode], globals: &std::collections::HashSet<String>,
                bare_globals: &std::collections::HashSet<String>,
                aliases: &HashMap<String, String>,
                member_aliases: &HashMap<String, (String, String)>,
                fn_rets: &HashMap<String, Type>,
                classes: &[String],
                module_prefix: Option<&str>,
                out: &mut HashMap<String, Type>) {
            for s in stmts {
                let (name, rhs) = match s {
                    AstNode::Assign(lhs, rhs) => match bound_var(lhs) {
                        Some(n) => (n, Some(&**rhs)),
                        None => continue,
                    },
                    AstNode::Let { pattern, expr, .. } => match bound_var(pattern) {
                        Some(n) => (n, Some(&**expr)),
                        None => continue,
                    },
                    AstNode::Block { body } => {
                        walk(body, globals, bare_globals, aliases, member_aliases, fn_rets, classes, module_prefix, out);
                        continue;
                    }
                    _ => continue,
                };
                if name.contains("WUFU") && crate::diagnostics::env_flag("ZETA_PROBE_GLOBALS") {
                    if let Some(r) = rhs {
                        let d = format!("{:?}", r);
                        let head: String = d.chars().take(320).collect();
                        eprintln!(
                            "WALK {}: in_bare={} rhs={}",
                            name,
                            bare_globals.contains(&name),
                            head
                        );
                    }
                }
                if !globals.contains(&name) && !bare_globals.contains(&name) {
                    continue;
                }
                // Resolve a module-level global's type — including CONSTANT
                // CHAINS (`_CACHE = _PROJECT_ROOT / "data" / "x.json"`,
                // `_ROOT = Path(__file__).resolve().parents[2]`). Real projects
                // build paths that way; before this the global stayed I64 and
                // every `.exists()` / `.read_text()` on it emitted a bare symbol
                // (`_exists` 7 / `_read_text` 6 reference sites in the REasyQuant
                // local backtest).
                let ty = rhs.and_then(|r| {
                    infer_global_ty(r, &out, &aliases, &member_aliases, fn_rets, classes)
                });
                if name.contains("WUFU") && crate::diagnostics::env_flag("ZETA_PROBE_GLOBALS") {
                    eprintln!("INFER {}: {:?}", name, ty);
                }
                if let Some(t) = ty {
                    // Key it under the BARE source name (what a function body in
                    // the same module reads) AND under the module-mangled name
                    // (`<prefix><name>`) — a name re-exported from another module
                    // (`_PROJECT_ROOT` imported into market_data_universe) is read
                    // through the mangled global, so one key alone misses.
                    out.insert(name.clone(), t.clone());
                    if let Some(pfx) = module_prefix {
                        out.insert(format!("{}{}", pfx, name), t);
                    }
                }
            }
        }
        let defs = self.registered_func_defs.borrow().clone();
        // Return types of every registered function. A module-level constant built
        // by a comprehension over an annotated helper
        // (`WUFU_BS_CODES = [jq_to_bs(c) for c in WUFU_JQ_CODES]`, `-> str`) had no
        // inferable type, so the global stayed I64 and
        // `WUFU_BS_CODES + WUFU_INDEX_BS_CODES` compiled as a NUMERIC add — the
        // garbage handle then went to `dict.fromkeys` and `py_map_fromkeys`
        // dereferenced it (50% SIGSEGV per run, 100% with MallocScribble=1).
        let mut fn_rets: HashMap<String, Type> = self
            .funcs
            .iter()
            .map(|(n, (_, r, _))| (n.clone(), r.clone()))
            .collect();
        // Batch 291: user-class names (see the ctor case in `infer_global_ty`).
        // Sorted for deterministic iteration; the suffix match requires a UNIQUE
        // hit, ambiguous names simply stay untyped.
        let mut classes: Vec<String> = self.type_decls.keys().cloned().collect();
        classes.sort();
        let sig_params = self.sig_params_snapshot();
        // Batch 300: same recovery as `lower_to_mir` does for MIR's signature
        // table — a module-level global assigned from an UNANNOTATED function
        // (`pf = make(…)`) otherwise carries UNIT, and every `pf.<attr>` read then
        // indexes an empty struct variant (field 0) instead of the class.
        {
            let declared = fn_rets.clone();
            // Batch 600: the recovery must see the QUALIFIED method defs —
            // registered_func_defs carries the BARE originals, and two classes
            // defining the same method name (`speak` on Animal and Dog) made
            // the suffix match hit twice → the uniqueness gate vetoed BOTH
            // recoveries → i64. registered_funcs carries the qualified clones
            // with real bodies.
            let mut rec_defs = defs.clone();
            for (k, v) in self.registered_funcs.iter() {
                if k.contains("::") {
                    if let AstNode::FuncDef { .. } = v {
                        rec_defs.push(std::rc::Rc::new(v.clone()));
                    }
                }
            }
            for (fname, fty) in fn_rets.iter_mut() {
                // Batch 600: a py CLASS method arrives with the desugaring's
                // `i64` default (parse_class body_is_string_return only
                // claims bare StringLit returns), so `::`-qualified entries
                // join the recovery exactly like batch 451's per-lowering
                // pass. Without it, `print(d.speak())` typed the call I64 and
                // printed the handle (class_inheritance).
                let is_class_method =
                    matches!(fty, Type::I64) && fname.contains("::");
                if !matches!(fty, Type::Tuple(inner) if inner.is_empty()) && !is_class_method {
                    continue;
                }
                if let Some(t) =
                    Self::unannotated_return_ty(&rec_defs, fname, &classes, &declared, &sig_params)
                {
                    *fty = t;
                }
            }
        }
        for d in &defs {
            let d: &AstNode = d;
            if let AstNode::FuncDef { name, body, .. } = d {
                // A module body is registered as `<module with _ for .>__init`;
                // the prefix tells us the mangled spelling of its globals.
                let prefix = name
                    .strip_suffix("init")
                    .filter(|p| p.ends_with("__"))
                    .map(|p| p.to_string());
                walk(
                    body,
                    &globals,
                    &bare_globals,
                    &aliases,
                    &member_aliases,
                    &fn_rets,
                    &classes,
                    prefix.as_deref(),
                    &mut out,
                );
            }
        }
        if crate::diagnostics::env_flag("ZETA_PROBE_GLOBALS") {
            let mut keys: Vec<String> = out.keys().cloned().collect();
            keys.sort();
            eprintln!("GLOBALS out: n={} keys={:?}", out.len(), keys);
            for (k, v) in &out {
                eprintln!("   {} -> {:?}", k, v);
            }
        }
        out
    }

    /// Batch 602: single-inheritance adoption — for each `class C(B)` with NO
    /// own `__init__` (parse_class marker on the ImplBlock attrs), adopt B's
    /// ctor parameters and merge B's fields into C's layout, so `Dog("Rex")`
    /// builds a Dog carrying `name` (class_inheritance). Method inheritance
    /// rides the existing bare-name fallback dispatch. Runs after ALL
    /// registration (the synthesized empty ctor is overwritten here) and
    /// before `refine_ctor_field_types`.
    /// Batch 613: a synthesized ctor carries its StructLit in the BODY's
    /// `Return` (ret_expr is None for parse-synthesized ctors).
    fn ctor_structlit_pairs(v: &AstNode) -> Vec<(String, AstNode)> {
        match v {
            AstNode::FuncDef { body, ret_expr, .. } => {
                let lit = ret_expr
                    .as_ref()
                    .and_then(|re| match &**re {
                        AstNode::StructLit { .. } => Some(re.as_ref()),
                        _ => None,
                    })
                    .or_else(|| {
                        body.iter().find_map(|st| match st {
                            AstNode::Return(val) => match &**val {
                                AstNode::StructLit { .. } => Some(val.as_ref()),
                                _ => None,
                            },
                            _ => None,
                        })
                    });
                match lit {
                    Some(AstNode::StructLit { fields, .. }) => fields.clone(),
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn expr_refs_var(e: &AstNode, name: &str) -> bool {
        match e {
            AstNode::Var(v) => v == name,
            AstNode::BinaryOp { left, right, .. } => {
                Self::expr_refs_var(left, name) || Self::expr_refs_var(right, name)
            }
            AstNode::Call { args, .. } => args.iter().any(|a| Self::expr_refs_var(a, name)),
            _ => false,
        }
    }

    /// Batch 620: deep-rewrite `Var(p)` occurrences through `bind`
    /// (base ctor param -> explicit call arg). Unbound names survive.
    fn subst_vars(e: &AstNode, bind: &[(String, AstNode)]) -> AstNode {
        match e {
            AstNode::Var(v) => match bind.iter().find(|(p, _)| p == v) {
                Some((_, a)) => a.clone(),
                None => e.clone(),
            },
            AstNode::BinaryOp {
                op,
                left,
                right,
            } => AstNode::BinaryOp {
                op: op.clone(),
                left: Box::new(Self::subst_vars(left, bind)),
                right: Box::new(Self::subst_vars(right, bind)),
            },
            AstNode::Call {
                receiver,
                method,
                args,
                type_args,
                structural,
            } => AstNode::Call {
                receiver: receiver.as_ref().map(|r| Box::new(Self::subst_vars(r, bind))),
                method: method.clone(),
                args: args.iter().map(|a| Self::subst_vars(a, bind)).collect(),
                type_args: type_args.clone(),
                structural: *structural,
            },
            other => other.clone(),
        }
    }

    pub fn inherit_class_members(&mut self) {
        let pairs = self.pending_inherits.borrow().clone();
        let mut order: Vec<String> = Vec::new();
        let mut bases_of: HashMap<String, Vec<String>> = HashMap::new();
        for (ty, base) in pairs {
            if !bases_of.contains_key(&ty) {
                order.push(ty.clone());
            }
            bases_of.entry(ty.clone()).or_default().push(base);
        }
        for ty in order {
            let bases = &bases_of[&ty];
            // Merge fields from ALL bases (first occurrence wins = MRO-lite).
            let mut merged: Vec<(String, String)> = Vec::new();
            let mut base_inits: Vec<(String, AstNode)> = Vec::new();
            for b in bases {
                if let Some(crate::middle::mir::r#gen::TypeDecl::Struct {
                    fields: bf,
                    ..
                }) = self.type_decls.get(b)
                {
                    for (f, s) in bf {
                        if !merged.iter().any(|(x, _)| x == f) {
                            merged.push((f.clone(), s.clone()));
                        }
                    }
                }
                if let Some(v) = self.registered_funcs.get(b) {
                    // Batch 620: bind this base's ctor params to the explicit
                    // call args (`A.__init__(self, 7)` ⇒ n ← 7) positionally,
                    // then rewrite its field initializers through that
                    // binding — the adopted ctor inherits the call's VALUES,
                    // not dangling base-param names.
                    let bind: Vec<(String, AstNode)> = {
                        let mut out: Vec<(String, AstNode)> = Vec::new();
                        if let Some((p, _, _)) = self.funcs.get(b) {
                            let call_args = self
                                .pending_baseargs
                                .borrow()
                                .iter()
                                .find(|(c, bb, _)| c == &ty && bb == b)
                                .map(|(_, _, a)| a.clone())
                                .unwrap_or_default();
                            for (k, (pn, _)) in p.iter().enumerate() {
                                if let Some(a) = call_args.get(k) {
                                    out.push((pn.clone(), a.clone()));
                                }
                            }
                        }
                        out
                    };
                    for (f, e) in Self::ctor_structlit_pairs(v) {
                        let e = if bind.is_empty() {
                            e
                        } else {
                            Self::subst_vars(&e, &bind)
                        };
                        if !base_inits.iter().any(|(x, _)| x.as_str() == f.as_str()) {
                            base_inits.push((f, e));
                        }
                    }
                }
            }
            let own = match self.type_decls.get(&ty) {
                Some(crate::middle::mir::r#gen::TypeDecl::Struct { fields, .. }) => {
                    fields.clone()
                }
                _ => Vec::new(),
            };
            for (f, s) in &own {
                if !merged.iter().any(|(x, _)| x == f) {
                    merged.push((f.clone(), s.clone()));
                }
            }
            if let Some(crate::middle::mir::r#gen::TypeDecl::Struct {
                fields: cf,
                ..
            }) = self.type_decls.get_mut(&ty)
            {
                *cf = merged.clone();
            }
            // The parse-synthesized ctor's own pairs/params, needed by BOTH
            // paths below (batch 621 hoisted the extraction here).
            let (synth_pairs, synth_params) = match self.registered_funcs.get(&ty) {
                Some(v @ AstNode::FuncDef { params, .. }) => {
                    (Self::ctor_structlit_pairs(v), params.clone())
                }
                _ => (Vec::new(), Vec::new()),
            };
            // Own-`__init__` subclasses are merge-only — the synthesized ctor
            // carries every OWN field (the :1146 collection pairs fields with
            // inits). Batch 621: EXCEPT when an explicit base call passed
            // non-Var args — the 608 collection names slots after the arg's
            // Var, so it cannot create those, and the ctor's StructLit misses
            // base-layout fields while positional writes follow the StructLit
            // order (s16: `Animal.__init__(self, "Rex")` + own `legs` made
            // d.name read legs' slot and print 4). When a gap actually exists
            // AND base call args were recorded, rebuild the ctor in merged
            // layout order — own pairs preserved verbatim (params untouched:
            // the own `__init__` signature is the user-facing one), missing
            // fields from the substituted base initializers.
            if !own.is_empty() {
                let has_baseargs = self
                    .pending_baseargs
                    .borrow()
                    .iter()
                    .any(|(c, _, _)| c == &ty);
                let has_gap = merged
                    .iter()
                    .any(|(f, _)| !synth_pairs.iter().any(|(n, _)| n == f));
                if !has_gap || !has_baseargs {
                    continue;
                }
                let mut ctor_fields: Vec<(String, AstNode)> = Vec::new();
                for (f, _s) in merged.iter() {
                    let value = if let Some((_, e)) =
                        synth_pairs.iter().find(|(n, _)| n == f)
                    {
                        e.clone()
                    } else if let Some((_, e)) =
                        base_inits.iter().find(|(n, _)| n == f)
                    {
                        e.clone()
                    } else if synth_params.iter().any(|(pn, _)| pn == f) {
                        AstNode::Var(f.clone())
                    } else {
                        AstNode::Lit(0)
                    };
                    ctor_fields.push((f.clone(), value));
                }
                // A base initializer naming a base param that is not among the
                // own signature's params would dangle — degrade to Lit 0.
                for (_, v) in ctor_fields.iter_mut() {
                    if let AstNode::Var(name) = v {
                        if !synth_params.iter().any(|(pn, _)| pn == name) {
                            *v = AstNode::Lit(0);
                        }
                    }
                }
                // Same kind refinement as the adoption path below: the base's
                // inference saw only its unannotated params (i64), so a Str
                // initializer would read back as a raw pointer.
                if let Some(crate::middle::mir::r#gen::TypeDecl::Struct {
                    fields: cf, ..
                }) = self.type_decls.get_mut(&ty)
                {
                    for (f, v) in cf.iter_mut() {
                        if let Some((_, e)) = ctor_fields.iter().find(|(n, _)| n == f) {
                            match e {
                                AstNode::StringLit(_) => *v = "str".to_string(),
                                AstNode::FloatLit(_) => *v = "f64".to_string(),
                                AstNode::Bool(_) => *v = "bool".to_string(),
                                _ => {}
                            }
                        }
                    }
                }
                let ctor = AstNode::FuncDef {
                    name: ty.clone(),
                    generics: Vec::new(),
                    lifetimes: Vec::new(),
                    params: synth_params,
                    ret: ty.clone(),
                    body: vec![],
                    attrs: vec![],
                    // Batch 651: the variant name must match the mangled class
                    // name (e.g., `m647a__Cfg`), not the bare name (`Cfg`).
                    // Without this, cross-module same-named classes both return
                    // `Struct { variant: "Cfg" }`, and the receiver type is bare
                    // `Cfg`, causing method dispatch to collide on `Cfg::show`.
                    // Search `type_decls` for the mangled name: it ends with
                    // `__<bare_name>` and belongs to the same module context.
                    ret_expr: Some(Box::new(AstNode::StructLit {
                        variant: {
                            let suffix = format!("__{}", ty);
                            self.type_decls
                                .keys()
                                .find(|k| k.ends_with(&suffix))
                                .cloned()
                                .unwrap_or_else(|| ty.clone())
                        },
                        fields: ctor_fields,
                    })),
                    single_line: false,
                    doc: String::new(),
                    pub_: false,
                    async_: false,
                    const_: false,
                    comptime_: false,
                    where_clauses: Vec::new(),
                };
                self.register(ctor);
                continue;
            }
            // No-own-init adoption. Batch 620: the parse-synthesized ctor for
            // `ty` — which this adoption REPLACES — carries the 605-collected
            // explicit-base-call args (`A.__init__(self, 7)` ⇒ x ← 7). Those
            // call-site values outrank the param-name and base-initializer
            // fallbacks (previously they were discarded, so the arg read 0).
            // Params: the synthesized ctor's own signature first (it is the
            // user-facing one), then base-ctor params; any param no ctor
            // field references is dropped, and a base initializer naming a
            // dropped param degrades to Lit 0 (a dangling Var reads garbage).
            let mut ctor_params: Vec<(String, String)> = synth_params;
            for b in bases {
                if let Some((p, _, _)) = self.funcs.get(b) {
                    if !p.is_empty() {
                        for (pn, pt) in p {
                            if !ctor_params.iter().any(|(n, _)| n == pn) {
                                let s = match pt {
                                    Type::Str => "str".to_string(),
                                    Type::F64 => "f64".to_string(),
                                    Type::Bool => "bool".to_string(),
                                    Type::Named(n, _) => n.clone(),
                                    Type::PyDynamic => "dyn".to_string(),
                                    _ => "i64".to_string(),
                                };
                                ctor_params.push((pn.clone(), s));
                            }
                        }
                        break;
                    }
                }
            }
            let mut ctor_fields: Vec<(String, AstNode)> = Vec::new();
            for (f, _s) in merged.iter() {
                // Order matters: base_inits carries the 620 call-site
                // substitution (`A.__init__(self, "Rex", 4)` ⇒ name ← "Rex")
                // and must outrank the param-name branch — when a base param
                // shares the field's name, Var(f) would silently discard the
                // explicit arg (measured: s15 printed 0/0).
                let value = if let Some((_, e)) = synth_pairs.iter().find(|(n, _)| n == f) {
                    e.clone()
                } else if let Some((_, e)) =
                    base_inits.iter().find(|(n, _)| n == f)
                {
                    e.clone()
                } else if ctor_params.iter().any(|(pn, _)| pn == f) {
                    AstNode::Var(f.clone())
                } else {
                    AstNode::Lit(0)
                };
                ctor_fields.push((f.clone(), value));
            }
            ctor_params.retain(|(pn, _)| {
                ctor_fields
                    .iter()
                    .any(|(_, e)| Self::expr_refs_var(e, pn))
            });
            // Batch 620: the folded initializer kinds refine the field types —
            // `A.__init__(self, "Rex", 4)` makes C.name a Str slot. The base's
            // own inference saw only the unannotated param and typed it i64,
            // so the read printed the raw pointer (measured).
            if let Some(crate::middle::mir::r#gen::TypeDecl::Struct {
                fields: cf, ..
            }) = self.type_decls.get_mut(&ty)
            {
                for (f, v) in cf.iter_mut() {
                    if let Some((_, e)) = ctor_fields.iter().find(|(n, _)| n == f) {
                        match e {
                            AstNode::StringLit(_) => *v = "str".to_string(),
                            AstNode::FloatLit(_) => *v = "f64".to_string(),
                            AstNode::Bool(_) => *v = "bool".to_string(),
                            _ => {}
                        }
                    }
                }
            }
            for (_, v) in ctor_fields.iter_mut() {
                if let AstNode::Var(name) = v {
                    if !ctor_params.iter().any(|(pn, _)| pn == name) {
                        *v = AstNode::Lit(0);
                    }
                }
            }
            let ctor = AstNode::FuncDef {
                name: ty.clone(),
                generics: Vec::new(),
                lifetimes: Vec::new(),
                params: ctor_params,
                ret: ty.clone(),
                body: vec![],
                attrs: vec![],
                // Batch 651: same fix as the other constructor path — use the
                // mangled class name for the variant so the receiver type is
                // mangled and method dispatch doesn't collide.
                ret_expr: Some(Box::new(AstNode::StructLit {
                    variant: {
                        let suffix = format!("__{}", ty);
                        self.type_decls
                            .keys()
                            .find(|k| k.ends_with(&suffix))
                            .cloned()
                            .unwrap_or_else(|| ty.clone())
                    },
                    fields: ctor_fields,
                })),
                single_line: false,
                doc: String::new(),
                pub_: false,
                async_: false,
                const_: false,
                comptime_: false,
                where_clauses: Vec::new(),
            };
            self.register(ctor);
        }
    }



    /// Batch 600: ctor call-site field refinement — a module-level
    /// `a = Animal("Generic")` PROVES field `name` is Str; upgrade the
    /// class's i64-default field spellings from the call's positional args
    /// (param→field by shared name, `self.name = name`). Must run BEFORE
    /// any per-function lowering snapshots `type_decls`, so `self.name`
    /// reads inside methods (and the concatenation arm) see the concrete
    /// type. Literal/Var args only (out is the module-global type table);
    /// anything else stays conservatively i64.
    /// Batch 627: method-parameter refinement from module-level call sites.
    /// `g.greet("World")` — an unannotated method parameter stayed i64, so an
    /// f-string part stringified the string handle with `to_string_i64`
    /// (`Hello, <addr>!`, closure_nonlocal diff line #4). Record refinements
    /// for the generator: `Class::method` -> [(FuncDef param position, Type)].
    /// Only i64-typed (unannotated) params are refined — explicit annotations
    /// are never overridden. Literal-kind mapping mirrors
    /// `refine_ctor_field_types`.
    pub fn refine_method_param_types(&self) -> HashMap<String, Vec<(usize, Type)>> {
        let out = self.module_global_types_at("s2842");
        let defs = self.registered_func_defs.borrow().clone();
        let mut map: HashMap<String, Vec<(usize, Type)>> = HashMap::new();
        for d in &defs {
            let d: &AstNode = d;
            if let AstNode::FuncDef { body, .. } = d {
                // Local class env: `g = Greeter(...)` at module level. The
                // resolver-wide table (out) does not carry plain module vars.
                let mut env: HashMap<String, String> = HashMap::new();
                for st in body {
                    // Track `x = <Ctor>(...)` before collecting this
                    // statement's sites (rhs uses the previous env).
                    if let AstNode::Assign(lhs, rhs) = st {
                        if let (
                            AstNode::Var(x),
                            AstNode::Call {
                                receiver: None,
                                method,
                                ..
                            },
                        ) = (&**lhs, &**rhs)
                        {
                            if self.funcs.contains_key(method.as_str()) {
                                env.insert(x.clone(), method.clone());
                            }
                        }
                        Self::collect_p627_in_expr(
                            rhs,
                            &out,
                            &env,
                            self,
                            &mut map,
                        );
                        continue;
                    }
                    match st {
                        AstNode::ExprStmt { expr } => {
                            Self::collect_p627_in_expr(expr, &out, &env, self, &mut map);
                        }
                        AstNode::Call { .. } => {
                            // The statement's own Call is itself a potential
                            // site (`bg.add("a")` as a bare statement — the
                            // site is the statement, not an argument).
                            Self::collect_p627_in_expr(st, &out, &env, self, &mut map);
                        }
                        _ => {}
                    }
                }
            }
        }
        map
    }

    fn p627_record(
        cls: &str,
        method: &str,
        args: &[AstNode],
        resolver: &Self,
        map: &mut HashMap<String, Vec<(usize, Type)>>,
    ) {
        let qname = format!("{}::{}", cls, method);
        let Some((params, _, _)) = resolver.funcs.get(&qname) else {
            return;
        };
        for (i, arg) in args.iter().enumerate() {
            // Method FuncDefs carry self at param position 0.
            let Some((_pn, pt)) = params.get(i + 1) else {
                continue;
            };
            // Unannotated params sit as I64 in `funcs` (or PyDynamic in the
            // gen-side spelling) — both mean "no annotation"; an explicit
            // annotation is never overridden.
            if !matches!(pt, Type::I64 | Type::PyDynamic) {
                continue;
            }
            let spell = match arg {
                AstNode::StringLit(_) => Some(Type::Str),
                AstNode::FloatLit(_) => Some(Type::F64),
                AstNode::Bool(_) => Some(Type::Bool),
                _ => None,
            };
            if let Some(t) = spell {
                map.entry(qname.clone()).or_default().push((i + 1, t));
            }
        }
    }

    fn collect_p627_in_expr(
        e: &AstNode,
        out: &HashMap<String, Type>,
        env: &HashMap<String, String>,
        resolver: &Self,
        map: &mut HashMap<String, Vec<(usize, Type)>>,
    ) {
        match e {
            AstNode::Call { receiver, method, args, .. } => {
                if let Some(rc) = receiver {
                    if let AstNode::Var(v) = &**rc {
                        let cls = out
                            .get(v)
                            .and_then(|t| match t {
                                Type::Named(n, _) => Some(n.clone()),
                                _ => None,
                            })
                            .or_else(|| env.get(v).cloned());
                        if let Some(cls) = cls {
                            Self::p627_record(&cls, method, args, resolver, map);
                        }
                    }
                }
                for a in args {
                    Self::collect_p627_in_expr(a, out, env, resolver, map);
                }
            }
            AstNode::BinaryOp { left, right, .. } => {
                Self::collect_p627_in_expr(left, out, env, resolver, map);
                Self::collect_p627_in_expr(right, out, env, resolver, map);
            }
            AstNode::UnaryOp { expr: inner, .. } => {
                Self::collect_p627_in_expr(inner, out, env, resolver, map);
            }
            AstNode::FieldAccess { base, .. } => {
                Self::collect_p627_in_expr(base, out, env, resolver, map);
            }
            AstNode::FString(parts) | AstNode::ArrayLit(parts) | AstNode::Tuple(parts) => {
                for p in parts {
                    Self::collect_p627_in_expr(p, out, env, resolver, map);
                }
            }
            AstNode::DictLit { entries } => {
                for (k, v) in entries {
                    Self::collect_p627_in_expr(k, out, env, resolver, map);
                    Self::collect_p627_in_expr(v, out, env, resolver, map);
                }
            }
            AstNode::Assign(lhs, rhs) => {
                Self::collect_p627_in_expr(lhs, out, env, resolver, map);
                Self::collect_p627_in_expr(rhs, out, env, resolver, map);
            }
            AstNode::Return(v) => {
                Self::collect_p627_in_expr(v, out, env, resolver, map);
            }
            _ => {}
        }
    }

    /// Batch 628: return-type inference for unannotated methods — the
    /// #195 remainder. `def ident(self, w): return w` refines `w` to Str in
    /// the body (627), yet the caller printed the raw handle: the call site
    /// types the result from the REGISTERED return type (I64) and picks
    /// println_i64. Infer the return type from every `Return` expression in
    /// the body — a StringLit, or a parameter the 627 map already refined —
    /// and write it into `funcs` when all returns agree and the registered
    /// type is still I64 (unannotated). The generator's ret_types snapshot
    /// is built from `funcs` at gen time, so one write reaches every call
    /// site. Methods come from `impls` (class-attributed keys); plain
    /// module functions ride their bare-name signatures.
    pub fn refine_method_return_types(
        &mut self,
        param_map: &HashMap<String, Vec<(usize, Type)>>,
    ) -> usize {
        let mut refined = 0;
        // Batch 631: return CHAINS (`return self.inner()`) need the callee's
        // ret refined first — iterate to a fixpoint (bounded) rebuilding the
        // ret snapshot each round.
        for _round in 0..4 {
        let mut refined_round = 0;
        // (qualified name, method FuncDef) pairs: methods from impls,
        // plain functions from registered_func_defs.
        let mut candidates: Vec<(String, AstNode)> = Vec::new();
        for ((_concept, ty), body) in self.impls.iter() {
            for item in body {
                if let AstNode::FuncDef { name, .. } = item {
                    candidates.push((format!("{}::{}", ty, name), item.clone()));
                }
            }
        }
        for d in self.registered_func_defs.borrow().iter() {
            let d: &AstNode = d;
            if let AstNode::FuncDef { name, .. } = d {
                // Plain module functions only: the funcs key is the bare
                // name, and a `::`-qualified entry belongs to the impls loop.
                if !name.contains("::") && self.funcs.contains_key(name.as_str()) {
                    candidates.push((name.clone(), d.clone()));
                }
            }
        }
        // Batch 630: map-field value types, for the `.get(k, <lit>)`
        // return face — the default literal is only evidence when it agrees
        // with the map's voted value type (an int-valued map with a string
        // default is a genuine union — inferring Str would corrupt the hit
        // path).
        let mut map_vals: HashMap<(String, String), Type> = HashMap::new();
        for (ty, td) in self.type_decls.iter() {
            if let crate::middle::mir::r#gen::TypeDecl::Struct { fields, .. } = td {
                for (f, spelling) in fields {
                    if let Some(rest) = spelling.strip_prefix("map<") {
                        let inner = rest.trim_end_matches('>');
                        let mut it = inner.split(',');
                        let one = |x: &str| match x.trim() {
                            "str" => Some(Type::Str),
                            "f64" => Some(Type::F64),
                            "bool" => Some(Type::Bool),
                            "i64" => Some(Type::I64),
                            _ => None,
                        };
                        if let (Some(_k), Some(v)) = (
                            it.next().and_then(|x| one(x)),
                            it.next().and_then(|x| one(x)),
                        ) {
                            map_vals.insert((ty.clone(), f.clone()), v);
                        }
                    } else if spelling == "map" {
                        /* Batch 646: the BARE spelling — values erased
                           (PyDynamic). A `.get(k, <str>)` on it is the
                           genuine union; the ret becomes PyJson so the
                           caller renders the gen-side tagged cell by tag. */
                        map_vals.insert(
                            (ty.clone(), f.clone()),
                            Type::PyDynamic,
                        );
                    }
                }
            }
        }
        let fn_rets: HashMap<String, Type> = self
            .funcs
            .iter()
            .map(|(n, (_, r, _))| (n.clone(), r.clone()))
            .collect();
        /* Batch 646: (class, field) whose map spelling is the BARE "map"
           (values erased) — a `return self.<f>.get(k, <str>)` on such a
           field is the genuine union; the gen-side cell route returns a
           tagged cell, so the method's registered ret becomes PyJson and
           the caller renders through py_json_as_str. */
        let mut erased_map_fields: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for (ty, td) in self.type_decls.iter() {
            if let crate::middle::mir::r#gen::TypeDecl::Struct { fields, .. } = td {
                for (f, spelling) in fields {
                    if spelling == "map" {
                        erased_map_fields.insert((ty.clone(), f.clone()));
                    }
                }
            }
        }
        for (qname, fd) in candidates {
            // Batch 758 (#38①c): the placeholder face for "no declared
            // return" is not only I64 — body-derived inference answers
            // Tuple([]) (unit) for match-bodied functions (its Match arm
            // abstains), and those were exactly the fns this fixpoint
            // exists for. A unit placeholder with unanimous literal-return
            // evidence is safe to refine: a body that returns literals is
            // not a procedure.
            let unannotated = match self.funcs.get(&qname) {
                Some((_, Type::I64, _)) => true,
                Some((_, Type::Tuple(ts), _)) if ts.is_empty() => true,
                _ => false,
            };
            if !unannotated {
                continue;
            }
            let cls = qname.split("::").next().unwrap_or("").to_string();
            let AstNode::FuncDef { params, .. } = &fd else {
                continue;
            };
            let mut rets: Vec<Type> = Vec::new();
            let mut dyn_faces = 0usize;
            Self::collect_return_kinds(
                &fd,
                params,
                param_map,
                &qname,
                &map_vals,
                &erased_map_fields,
                &cls,
                &fn_rets,
                &mut rets,
                &mut dyn_faces,
            );
            if rets.is_empty() {
                continue;
            }
            let first = rets[0].clone();
            // Only literal-kind types are writable; a unanimous PyDynamic
            // vote means every return was un-inferable — abstain.
            // Batch 660 (mainline): an all-dyn union face (every return is
            // evidenced PyDynamic) writes PyDynamic. Batch 646 (cleanup):
            // Named("PyJson") is also writable — the tagged-cell return face
            // (the caller renders by tag).
            let writable = ((matches!(first, Type::Str | Type::F64 | Type::Bool)
                || matches!(&first, Type::Tuple(ts) if !ts.is_empty())
                || first == Type::Named("PyJson".to_string(), vec![]))
                && rets.iter().all(|t| *t == first))
                || (dyn_faces == rets.len()
                    && rets.iter().all(|t| matches!(t, Type::PyDynamic)));
            if writable {
                if let Some((_, ret, _)) = self.funcs.get_mut(&qname) {
                    // Batch 758: the unit placeholder writes too (see the
                    // unannotated gate above) — it is the Match-bodied
                    // spelling of "no declared return".
                    if matches!(ret, Type::I64)
                        || matches!(ret, Type::Tuple(ts) if ts.is_empty())
                    {
                        *ret = first.clone();
                        refined += 1;
                        refined_round += 1;
                    }
                }
            }
        }
        if refined_round == 0 {
            break;
        }
        }
        refined
    }

    /// Collect the inferable type of every `Return` expression in a method
    /// body: a string/float/bool literal, or a parameter the 627 map
    /// refined. Anything else yields no evidence (abstain).
    #[allow(clippy::too_many_arguments)]
    fn collect_return_kinds(
        fd: &AstNode,
        params: &[(String, String)],
        param_map: &HashMap<String, Vec<(usize, Type)>>,
        qname: &str,
        map_vals: &HashMap<(String, String), Type>,
        erased_map_fields: &std::collections::HashSet<(String, String)>,
        cls: &str,
        fn_rets: &HashMap<String, Type>,
        out: &mut Vec<Type>,
        // 批次 660：`out` 里的 PyDynamic 有两种成因 ——「证据表明这是真并集」（已知动态，
        // 计入本计数）与「这一条返回我推不出来」（毒票，不计）。写回时只有**全部**是
        // 已知动态才允许把返回钉成 PyDynamic，毒票维持批次 628 的弃权。
        dyn_faces: &mut usize,
    ) {
        // `+` with any inferable-Str operand is Str concat (CPython);
        // F64+F64 stays F64. Recursion is bounded by expression depth.
        fn refinable(
            e: &AstNode,
            params: &[(String, String)],
            param_map: &HashMap<String, Vec<(usize, Type)>>,
            qname: &str,
        ) -> Option<Type> {
            match e {
                AstNode::StringLit(_) => Some(Type::Str),
                AstNode::FloatLit(_) => Some(Type::F64),
                AstNode::Bool(_) => Some(Type::Bool),
                // 批次 883（#274）：元组字面量也是证据——每个元素各自可推断
                // 即得 per-位元组型（`return ("abc", "def")` ⇒ Tuple([Str, Str])）。
                // 此前元组无证据 ⇒ 函数停在单元占位 Tuple([])，调用点解包的
                // 每位元素全落 I64，字符串按指针字渲染（静默错值）。
                AstNode::Tuple(items) => {
                    let mut ts = Vec::with_capacity(items.len());
                    for it in items {
                        ts.push(refinable(it, params, param_map, qname)?);
                    }
                    Some(Type::Tuple(ts))
                }
                AstNode::Var(v) => {
                    let pos = params.iter().position(|(pn, _)| pn == v)?;
                    param_map
                        .get(qname)?
                        .iter()
                        .find(|(p, _)| *p == pos)
                        .map(|(_, t)| t.clone())
                }
                AstNode::BinaryOp { op, left, right } if op == "+" => {
                    let l = refinable(left, params, param_map, qname);
                    let r = refinable(right, params, param_map, qname);
                    if matches!(l, Some(Type::Str)) || matches!(r, Some(Type::Str)) {
                        Some(Type::Str)
                    } else if matches!(l, Some(Type::F64)) && matches!(r, Some(Type::F64)) {
                        Some(Type::F64)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        let refinable = |e: &AstNode| -> Option<Type> {
            refinable(e, params, param_map, qname)
        };
        let mut walk = |stmts: &Vec<AstNode>, out: &mut Vec<Type>| {
            for st in stmts {
                match st {
                    AstNode::Return(val) => {
                        // A bare `return` ( Lit(0) placeholder) carries no
                        // evidence; Lit(0) is also the historical None
                        // spelling, so it never infers.
                        if !matches!(**val, AstNode::Lit(0)) {
                            if let Some(t) = refinable(val) {
                                out.push(t);
                            } else if let AstNode::Call {
                                receiver: Some(rc),
                                method,
                                args,
                                ..
                            } = &**val
                            {
                                // Batch 630/631 call faces, in order:
                                // ① `return self.<f>.get(k, <lit>)` — the
                                //    default literal is value-type evidence
                                //    ONLY when it agrees with the map's voted
                                //    value type; disagreement = genuine union
                                //    (poison — the hit path must not corrupt).
                                // ② `return self.<m>(·)` / `return <plain>(·)`
                                //    — the callee's ALREADY-REFINED ret is
                                //    evidence (the 631 fixpoint re-runs the
                                //    pass so chains converge); an
                                //    I64/unknown callee poisons.
                                let mut face: Option<Type> = None;
                                if method == "get" && args.len() == 2 {
                                    if let AstNode::FieldAccess { base: fb, field } = &**rc {
                                        if let AstNode::Var(b) = &**fb {
                                            if b == "self" {
                                                let vt = map_vals
                                                    .get(&(cls.to_string(), field.clone()));
                                                let dt = match &args[1] {
                                                    AstNode::StringLit(_) => Some(Type::Str),
                                                    AstNode::FloatLit(_) => Some(Type::F64),
                                                    AstNode::Bool(_) => Some(Type::Bool),
                                                    _ => None,
                                                };
                                                face = match (vt, dt) {
                                                    (Some(v), Some(d)) if *v == d => Some(d.clone()),
                                                    // Batch 660 (mainline): the field has no
                                                    // map<k,v> annotation ⇒ the stored value
                                                    // type is statically unknown ⇒ the return
                                                    // is a genuine union (hit ∪ default) = a
                                                    // known-dynamic face.
                                                    (None, Some(_)) => Some(Type::PyDynamic),
                                                    /* Batch 646 (cleanup): the genuine union
                                                       (erased map values + a str default) —
                                                       the gen cell route returns the tagged
                                                       cell; type the ret PyJson so the caller
                                                       renders by tag. */
                                                    (Some(Type::PyDynamic), Some(d)) => {
                                                        Some(Type::Named(
                                                            "PyJson".to_string(),
                                                            vec![],
                                                        ))
                                                    }
                                                    (Some(_), Some(_)) => Some(Type::PyDynamic),
                                                    _ => None,
                                                };
                                            }
                                        }
                                    }
                                }
                                if face.is_none() {
                                    let callee = match &**rc {
                                        AstNode::Var(b) if b == "self" => {
                                            Some(format!("{}::{}", cls, method))
                                        }
                                        AstNode::Var(b) if !b.is_empty() => Some(b.clone()),
                                        _ => None,
                                    };
                                    face = callee
                                        .and_then(|c| fn_rets.get(&c))
                                        .filter(|t| {
                                            matches!(**t, Type::Str | Type::F64 | Type::Bool)
                                        })
                                        .cloned();
                                }
                                // 已知动态面（只有 `.get(k, default)` 面会产生 PyDynamic 的
                                // face —— callee 回退那条 filter 只放行 Str/F64/Bool）。
                                if matches!(face, Some(Type::PyDynamic)) {
                                    *dyn_faces += 1;
                                }
                                match face {
                                    Some(t) if t != Type::PyDynamic => out.push(t),
                                    _ => out.push(Type::PyDynamic),
                                }
                            } else {
                                // Un-inferable return: poison the vote so a
                                // mixed body never refines.
                                out.push(Type::PyDynamic);
                            }
                        }
                    }
                    AstNode::If { then, else_, .. } => {
                        for s in then.iter().chain(else_.iter()) {
                            if let AstNode::Return(val) = s {
                                if let Some(t) = refinable(val) {
                                    out.push(t);
                                } else if !matches!(**val, AstNode::Lit(0)) {
                                    out.push(Type::PyDynamic);
                                }
                            }
                        }
                    }
                    // Batch 745: match arms carry returns too — the Python
                    // `case pat:` form wraps them in a Block (parse_match_arm's
                    // block-body form), and the Rust `=> expr` form IS an
                    // implicit return of the arm expression. Without this
                    // descent a match-based String return left the function
                    // pinned I64 (callers printed 0 for `f(1)`).
                    AstNode::Match { arms, .. } => {
                        for arm in arms {
                            // case form: body is a Block of stmts (collect the
                            // Returns); => form: body IS the implicit return
                            // expr. Nested match-in-match inside an arm body is
                            // a registered boundary (no recursion in this walk).
                            match &*arm.body {
                                AstNode::Block { body } => {
                                    for s in body {
                                        if let AstNode::Return(val) = s {
                                            if let Some(t) = refinable(val) {
                                                out.push(t);
                                            } else if !matches!(**val, AstNode::Lit(0)) {
                                                out.push(Type::PyDynamic);
                                            }
                                        }
                                    }
                                    // Batch 758 (#38①): the trailing bare
                                    // expression is the arm's value — vote it
                                    // when refinable (no PyDynamic poison:
                                    // statement-only arms abstain).
                                    if let Some(last) = body.last() {
                                        let v = match last {
                                            AstNode::ExprStmt { expr } => Some(&**expr),
                                            other => Some(other),
                                        };
                                        if let Some(v) = v {
                                            if !matches!(v, AstNode::Return(_)) {
                                                if let Some(t) = refinable(v) {
                                                    out.push(t);
                                                }
                                            }
                                        }
                                    }
                                }
                                other => {
                                    // Batch 758 (#38①b): an arrow arm body of
                                    // `return <expr>` IS a return — unwrap it
                                    // (refinable() has no Return arm, so the
                                    // evidence was dropped and the fn stayed
                                    // I64; callers printed the string handle
                                    // as an address). Non-inferable values
                                    // mirror the Block branch's PyDynamic.
                                    if let AstNode::Return(val) = other {
                                        if let Some(t) = refinable(val) {
                                            out.push(t);
                                        } else if !matches!(**val, AstNode::Lit(0)) {
                                            out.push(Type::PyDynamic);
                                        }
                                    } else if let Some(t) = refinable(other) {
                                        out.push(t);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        };
        if let AstNode::FuncDef { body, .. } = fd {
            walk(body, out);
        }
        // Batch 758 (#38①): the promoted ret_expr (see infer_untyped_returns)
        // carries the same returns for match-bodied functions.
        if let AstNode::FuncDef {
            ret_expr: Some(rx), ..
        } = fd
        {
            let one = vec![(**rx).clone()];
            walk(&one, out);
        }
    }

    /// Batch 629: field ELEMENT-type refinement from method-body append
    /// calls (class_list_field_method). `self.items = []` carries no element
    /// evidence — the 594 spelling defaults to `list<i64>` — so
    /// `bg.items[0]` read the string handle as i64 and printed the address.
    /// When a method body calls `self.<f>.append(<e>)` and e's type is
    /// inferable (a literal, or a parameter the 627 map refined), vote for
    /// that element type; when all votes for a field agree, rewrite the
    /// spelling `list<i64>` -> `list<T>` — never overriding an explicit or
    /// already-refined element type.
    pub fn refine_field_element_types(
        &mut self,
        param_map: &HashMap<String, Vec<(usize, Type)>>,
    ) -> usize {
        let mut votes: HashMap<(String, String), Vec<Type>> = HashMap::new();
        for ((_concept, ty), body) in self.impls.iter() {
            for item in body {
                if let AstNode::FuncDef {
                    name: mname,
                    params,
                    body: mbody,
                    ret_expr,
                    ..
                } = item
                {
                    let qname = format!("{}::{}", ty, mname);
                    Self::p629_walk_stmts(
                        mbody, ty, params, param_map, &qname, &mut votes,
                    );
                    // Batch-592/:992 behaviour: a method whose only statement
                    // is an ExprStmt has it promoted into ret_expr —
                    // `self.items.append(x)` lives THERE, not in body.
                    if let Some(re) = ret_expr {
                        Self::p629_walk_expr(
                            re, ty, params, param_map, &qname, &mut votes,
                        );
                    }
                }
            }
        }
        let spell = |t: &Type| match t {
            Type::Str => "str",
            Type::F64 => "f64",
            Type::Bool => "bool",
            _ => "i64",
        };
        let mut refined = 0;
        for ((ty, field), vs) in votes {
            if vs.is_empty() {
                continue;
            }
            let first = vs[0].clone();
            if !vs.iter().all(|t| *t == first) {
                continue;
            }
            if let Some(crate::middle::mir::r#gen::TypeDecl::Struct {
                fields, ..
            }) = self.type_decls.get_mut(&ty)
            {
                for (f, spelling) in fields.iter_mut() {
                    if f == &field && spelling == "list<i64>" {
                        *spelling = format!("list<{}>", spell(&first));
                        refined += 1;
                    }
                }
            }
        }
        refined
    }

    /// Recursive walker for append-call sites (top-level statements plus
    /// If/While/For bodies and Assign/Return expressions).
    fn p629_walk_stmts(
        stmts: &Vec<AstNode>,
        ty: &str,
        params: &[(String, String)],
        param_map: &HashMap<String, Vec<(usize, Type)>>,
        qname: &str,
        votes: &mut HashMap<(String, String), Vec<Type>>,
    ) {
        for st in stmts {
            match st {
                AstNode::ExprStmt { expr } => {
                    Self::p629_walk_expr(expr, ty, params, param_map, qname, votes);
                }
                AstNode::Call { args, .. } => {
                    for a in args {
                        Self::p629_walk_expr(a, ty, params, param_map, qname, votes);
                    }
                }
                AstNode::Assign(_, rhs) | AstNode::Return(rhs) => {
                    Self::p629_walk_expr(rhs, ty, params, param_map, qname, votes);
                }
                AstNode::AssignOp { value, .. } => {
                    Self::p629_walk_expr(value, ty, params, param_map, qname, votes);
                }
                AstNode::If { then, else_, .. } => {
                    Self::p629_walk_stmts(then, ty, params, param_map, qname, votes);
                    Self::p629_walk_stmts(else_, ty, params, param_map, qname, votes);
                }
                AstNode::While { body, else_body, .. } => {
                    Self::p629_walk_stmts(body, ty, params, param_map, qname, votes);
                    Self::p629_walk_stmts(else_body, ty, params, param_map, qname, votes);
                }
                AstNode::For {
                    body,
                    else_body,
                    ..
                } => {
                    Self::p629_walk_stmts(body, ty, params, param_map, qname, votes);
                    Self::p629_walk_stmts(else_body, ty, params, param_map, qname, votes);
                }
                _ => {}
            }
        }
    }

    fn p629_infer_elem(
        e: &AstNode,
        params: &[(String, String)],
        param_map: &HashMap<String, Vec<(usize, Type)>>,
        qname: &str,
    ) -> Option<Type> {
        match e {
            AstNode::StringLit(_) => Some(Type::Str),
            AstNode::FloatLit(_) => Some(Type::F64),
            AstNode::Bool(_) => Some(Type::Bool),
            AstNode::Var(v) => {
                let pos = params.iter().position(|(pn, _)| pn == v);
                let pos = pos?;
                param_map
                    .get(qname)?
                    .iter()
                    .find(|(p, _)| *p == pos)
                    .map(|(_, t)| t.clone())
                    .filter(|t| matches!(t, Type::Str | Type::F64 | Type::Bool))
            }
            _ => None,
        }
    }

    fn p629_walk_expr(
        e: &AstNode,
        ty: &str,
        params: &[(String, String)],
        param_map: &HashMap<String, Vec<(usize, Type)>>,
        qname: &str,
        votes: &mut HashMap<(String, String), Vec<Type>>,
    ) {
        match e {
            AstNode::Call {
                receiver: Some(rc),
                method,
                args,
                ..
            } => {
                if method == "append" && args.len() == 1 {
                    if let AstNode::FieldAccess { base, field } = &**rc {
                        if let AstNode::Var(b) = &**base {
                            if b == "self" {
                                if let Some(t) = Self::p629_infer_elem(
                                    &args[0], params, param_map, qname,
                                ) {
                                    votes
                                        .entry((ty.to_string(), field.clone()))
                                        .or_default()
                                        .push(t);
                                }
                                return;
                            }
                        }
                    }
                }
                Self::p629_walk_expr(rc, ty, params, param_map, qname, votes);
                for a in args {
                    Self::p629_walk_expr(a, ty, params, param_map, qname, votes);
                }
            }
            AstNode::BinaryOp { left, right, .. } => {
                Self::p629_walk_expr(left, ty, params, param_map, qname, votes);
                Self::p629_walk_expr(right, ty, params, param_map, qname, votes);
            }
            AstNode::ExprStmt { expr } => {
                Self::p629_walk_expr(expr, ty, params, param_map, qname, votes);
            }
            AstNode::Assign(lhs, rhs) => {
                Self::p629_walk_expr(lhs, ty, params, param_map, qname, votes);
                Self::p629_walk_expr(rhs, ty, params, param_map, qname, votes);
            }
            AstNode::Return(v) => {
                Self::p629_walk_expr(v, ty, params, param_map, qname, votes);
            }
            _ => {}
        }
    }

    /// Batch 630: map VALUE-type voting from method-body subscript writes
    /// (629's recipe for dicts). `self.d = {}` carries no value evidence
    /// (the 594 spelling is the bare "map"), so a `self.d.get(k, <str>)`
    /// chain had no static value type and the method's return stayed I64 —
    /// the caller printed the string handle as an integer (s38). Vote the
    /// (key, value) kinds from `self.<f>[<k>] = <v>` sites (literals or
    /// 627-refined params); when a field's votes are unanimous and the
    /// value kind is Str/F64/Bool, rewrite the spelling `map` ->
    /// `map<K, V>` (the lt_annotation_type form the read side parses).
    /// Annotated `map<K, V>` spellings are never touched.
    pub fn refine_map_value_types(
        &mut self,
        param_map: &HashMap<String, Vec<(usize, Type)>>,
    ) -> usize {
        let mut votes: HashMap<(String, String), Vec<(Type, Type)>> = HashMap::new();
        for ((_concept, ty), body) in self.impls.iter() {
            for item in body {
                if let AstNode::FuncDef {
                    name: mname,
                    params,
                    body: mbody,
                    ret_expr,
                    ..
                } = item
                {
                    let qname = format!("{}::{}", ty, mname);
                    Self::p630_walk_stmts(mbody, ty, params, param_map, &qname, &mut votes);
                    // The :992 promotion — a sole-statement method body lives
                    // in ret_expr (same trap batch 629 hit for append).
                    if let Some(re) = ret_expr {
                        Self::p630_walk_expr(re, ty, params, param_map, &qname, &mut votes);
                    }
                }
            }
        }
        let spell = |t: &Type| match t {
            Type::Str => "str",
            Type::F64 => "f64",
            Type::Bool => "bool",
            _ => "i64",
        };
        let mut refined = 0;
        for ((ty, field), vs) in votes {
            if vs.is_empty() {
                continue;
            }
            let (k0, v0) = (&vs[0].0, &vs[0].1);
            if !vs.iter().all(|(k, v)| k == k0 && v == v0) {
                continue;
            }
            if !matches!(v0, Type::Str | Type::F64 | Type::Bool) {
                continue;
            }
            if let Some(crate::middle::mir::r#gen::TypeDecl::Struct {
                fields, ..
            }) = self.type_decls.get_mut(&ty)
            {
                for (f, spelling) in fields.iter_mut() {
                    if f == &field && spelling == "map" {
                        *spelling = format!("map<{}, {}>", spell(k0), spell(v0));
                        refined += 1;
                    }
                }
            }
        }
        refined
    }

    fn p630_walk_stmts(
        stmts: &Vec<AstNode>,
        ty: &str,
        params: &[(String, String)],
        param_map: &HashMap<String, Vec<(usize, Type)>>,
        qname: &str,
        votes: &mut HashMap<(String, String), Vec<(Type, Type)>>,
    ) {
        for st in stmts {
            match st {
                AstNode::ExprStmt { expr } => {
                    Self::p630_walk_expr(expr, ty, params, param_map, qname, votes);
                }
                AstNode::Call { args, .. } => {
                    for a in args {
                        Self::p630_walk_expr(a, ty, params, param_map, qname, votes);
                    }
                }
                AstNode::Assign(lhs, rhs) => {
                    // The voting site: `self.<f>[<k>] = <v>`.
                    if let (
                        AstNode::Subscript {
                            base,
                            index: kexpr,
                        },
                        vexpr,
                    ) = (&**lhs, &**rhs)
                    {
                        if let AstNode::FieldAccess { base: fb, field } = &**base {
                            if let AstNode::Var(b) = &**fb {
                                if b == "self" {
                                    let kt = Self::p629_infer_elem(
                                        kexpr, params, param_map, qname,
                                    );
                                    let vt = Self::p629_infer_elem(
                                        vexpr, params, param_map, qname,
                                    );
                                    if let (Some(kt), Some(vt)) = (kt, vt) {
                                        votes
                                            .entry((ty.to_string(), field.clone()))
                                            .or_default()
                                            .push((kt, vt));
                                    }
                                    continue;
                                }
                            }
                        }
                    }
                    Self::p630_walk_expr(rhs, ty, params, param_map, qname, votes);
                }
                AstNode::AssignOp { value, .. } => {
                    Self::p630_walk_expr(value, ty, params, param_map, qname, votes);
                }
                AstNode::Return(v) => {
                    Self::p630_walk_expr(v, ty, params, param_map, qname, votes);
                }
                AstNode::If { then, else_, .. } => {
                    Self::p630_walk_stmts(then, ty, params, param_map, qname, votes);
                    Self::p630_walk_stmts(else_, ty, params, param_map, qname, votes);
                }
                AstNode::While { body, else_body, .. } => {
                    Self::p630_walk_stmts(body, ty, params, param_map, qname, votes);
                    Self::p630_walk_stmts(else_body, ty, params, param_map, qname, votes);
                }
                AstNode::For {
                    body,
                    else_body,
                    ..
                } => {
                    Self::p630_walk_stmts(body, ty, params, param_map, qname, votes);
                    Self::p630_walk_stmts(else_body, ty, params, param_map, qname, votes);
                }
                _ => {}
            }
        }
    }

    fn p630_walk_expr(
        e: &AstNode,
        ty: &str,
        params: &[(String, String)],
        param_map: &HashMap<String, Vec<(usize, Type)>>,
        qname: &str,
        votes: &mut HashMap<(String, String), Vec<(Type, Type)>>,
    ) {
        match e {
            AstNode::Assign(lhs, rhs) => {
                if let (
                    AstNode::Subscript {
                        base,
                        index: kexpr,
                    },
                    vexpr,
                ) = (&**lhs, &**rhs)
                {
                    if let AstNode::FieldAccess { base: fb, field } = &**base {
                        if let AstNode::Var(b) = &**fb {
                            if b == "self" {
                                let kt =
                                    Self::p629_infer_elem(kexpr, params, param_map, qname);
                                let vt =
                                    Self::p629_infer_elem(vexpr, params, param_map, qname);
                                if let (Some(kt), Some(vt)) = (kt, vt) {
                                    votes
                                        .entry((ty.to_string(), field.clone()))
                                        .or_default()
                                        .push((kt, vt));
                                }
                                return;
                            }
                        }
                    }
                }
                Self::p630_walk_expr(lhs, ty, params, param_map, qname, votes);
                Self::p630_walk_expr(rhs, ty, params, param_map, qname, votes);
            }
            AstNode::BinaryOp { left, right, .. } => {
                Self::p630_walk_expr(left, ty, params, param_map, qname, votes);
                Self::p630_walk_expr(right, ty, params, param_map, qname, votes);
            }
            AstNode::Call { args, .. } => {
                for a in args {
                    Self::p630_walk_expr(a, ty, params, param_map, qname, votes);
                }
            }
            _ => {}
        }
    }

    /// Batch 654: record `x = None` module-level assigns into the NoneVar
    /// set (sequential; any later plain assignment to x kills the entry).
    /// The set threads to the gen print/str faces via with_none_vars.
    pub fn note_none_vars(&mut self) {
        let defs = self.registered_func_defs.borrow().clone();
        for d in &defs {
            let d: &AstNode = d;
            if let AstNode::FuncDef { body, .. } = d {
                for st in body {
                    match st {
                        AstNode::Assign(lhs, rhs) => {
                            if let (AstNode::Var(x), rk) = (&**lhs, &**rhs) {
                                match rk {
                                    AstNode::NoneLit => {
                                        self.none_vars
                                            .borrow_mut()
                                            .insert(x.clone());
                                    }
                                    AstNode::Lit(_) | AstNode::FloatLit(_)
                                    | AstNode::StringLit(_) | AstNode::Bool(_)
                                    | AstNode::BigIntLit(_) => {
                                        self.none_vars.borrow_mut().remove(x);
                                    }
                                    // Batch 654: `z = x` copies the
                                    // None-ness when the source is
                                    // registered; unknown sources stay
                                    // untouched (conservative).
                                    AstNode::Var(src) => {
                                        if self.none_vars.borrow().contains(src) {
                                            self.none_vars
                                                .borrow_mut()
                                                .insert(x.clone());
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        AstNode::AssignOp { target, .. } => {
                            if let AstNode::Var(x) = &**target {
                                self.none_vars.borrow_mut().remove(x);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    pub fn refine_ctor_field_types(&mut self) {
        let out = self.module_global_types_at("s3782");
        let defs = self.registered_func_defs.borrow().clone();
        for d in &defs {
            let d: &AstNode = d;
            if let AstNode::FuncDef { body, .. } = d {
                for st in body {
                    if let AstNode::Assign(lhs, rhs) = st {
                        if let (
                            AstNode::Var(_),
                            AstNode::Call {
                                receiver: None,
                                method,
                                args,
                                ..
                            },
                        ) = (&**lhs, &**rhs)
                        {
                            let pnames = self
                                .func_param_names()
                                .get(method.as_str())
                                .cloned()
                                .unwrap_or_default();
                            if pnames.is_empty() {
                                continue;
                            }
                            let mut decl_fields = match self
                                .type_decls
                                .get_mut(method.as_str())
                            {
                                Some(crate::middle::mir::r#gen::TypeDecl::Struct {
                                    fields,
                                    ..
                                }) => fields,
                                _ => continue,
                            };
                            for (i, arg) in args.iter().enumerate() {
                                let Some(pn) = pnames.get(i).cloned() else {
                                    continue;
                                };
                                let spell = match arg {
                                    AstNode::StringLit(_) => Some("str".to_string()),
                                    AstNode::FloatLit(_) => Some("f64".to_string()),
                                    AstNode::Bool(_) => Some("bool".to_string()),
                                    AstNode::Var(v) => match out.get(v) {
                                        Some(Type::Str) => Some("str".to_string()),
                                        Some(Type::F64) => Some("f64".to_string()),
                                        Some(Type::Bool) => Some("bool".to_string()),
                                        Some(Type::DynamicArray(e)) => Some(format!(
                                            "list<{}>",
                                            match **e {
                                                Type::Str => "str",
                                                Type::F64 => "f64",
                                                _ => "i64",
                                            }
                                        )),
                                        _ => None,
                                    },
                                    _ => None,
                                };
                                if let Some(spell) = spell {
                                    if let Some((_, dt)) = decl_fields
                                        .iter_mut()
                                        .find(|(f, _)| *f == pn)
                                    {
                                        if dt.as_str() == "i64" {
                                            *dt = spell;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Batch 603: class -> first base, for the gen-side base-chain method
    /// lookup (inherited methods: `d.greet()` on a Dog with greet on Animal).
    pub fn class_bases(&self) -> HashMap<String, String> {
        let m = self.class_bases.borrow().clone();
        if crate::diagnostics::env_flag("ZETA_PROBE_GLOBALS") {
            eprintln!("[P603] class_bases() -> {:?} ({} entries)", m, m.len());
        }
        m
    }

    /// PY-A: parameter names per function, for keyword-argument binding.
    pub fn func_param_names(&self) -> HashMap<String, Vec<String>> {
        self.funcs
            .iter()
            .map(|(name, (params, _, _))| {
                (
                    name.clone(),
                    params.iter().map(|(n, _)| n.clone()).collect(),
                )
            })
            .collect()
    }

    /// Batch 747 (#264): one parameter's type at registration time. The
    /// parser marks `**name` star-params with the reserved type string "**"
    /// (and `*name` with "*", batch 752); the slot is an ordinary map /
    /// list handle (which is what `**name` / `*name` IS), and the name is
    /// recorded — first of each kind wins, Python allows one of each — so
    /// call sites can collect positional overflow (list) and unmatched
    /// keyword arguments (dict) bound to these parameters.
    fn typed_param_type(
        &self,
        fname: &str,
        pname: &str,
        ty_str: &str,
        generic_names: &[String],
    ) -> Type {
        match ty_str.trim() {
            "**" | "*" => {
                let mut sp = self.star_params.borrow_mut();
                let e = sp
                    .entry(fname.to_string())
                    .or_insert_with(|| (None, None));
                if ty_str.trim() == "**" {
                    if e.1.is_none() {
                        e.1 = Some(pname.to_string());
                    }
                } else if e.0.is_none() {
                    e.0 = Some(pname.to_string());
                }
                if ty_str.trim() == "**" {
                    Type::Named("map".to_string(), Vec::new())
                } else {
                    Type::DynamicArray(Box::new(Type::PyDynamic))
                }
            }
            _ => self.string_to_generic_type(ty_str, generic_names),
        }
    }

    /// Batch 747 (#264): callee -> (`*name`, `**name`) star-params, keyed
    /// like `func_param_names`.
    pub fn func_star_params(&self) -> HashMap<String, (Option<String>, Option<String>)> {
        self.star_params.borrow().clone()
    }

    pub fn is_abi_stable(&self, key: &MonoKey) -> bool {
        key.type_args.iter().all(|t| is_cache_safe(t))
    }

    pub fn record_mono(&mut self, key: MonoKey, mut mir: Mir) {
        let mangled = key.mangle();
        mir.name = Some(mangled.clone());
        self.mono_mirs.insert(key.clone(), mir);
        record_specialization(
            key.clone(),
            MonoValue {
                llvm_func_name: mangled,
                cache_safe: self.is_abi_stable(&key),
            },
        );
    }

    /// Batch 761 (#80③): REPL 每行降值模式——MirGen 的未声明名告警只在
    /// repl_mode 出声（见 gen.rs repl_mode 字段）。
    pub fn set_repl_lowering(&mut self, on: bool) {
        self.repl_lowering = on;
    }

    pub fn set_source_dir(&mut self, path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            self.module_resolver.set_root_dir(parent);
            *self.py_source_dir.borrow_mut() = Some(parent.to_path_buf());
        }
        // `__file__` is the path as given on the command line, matching what a
        // Python script would see for its own source.
        *self.source_file.borrow_mut() = Some(path.to_string_lossy().to_string());
    }

    /// PY-A: load a Python module from disk so `import X` works for the user's
    /// own files, not just the built-in registry. Search order:
    ///   <dir of the file being compiled> and its ancestors, $ZETA_PYLIB,
    ///   ~/.zeta/packages, the bundled `pylib`, build/stubs — each for
    ///   X.{py,z} or X/__init__.{py,z}
    /// Every top-level definition is prefixed `X__` so two modules (or a module
    /// and the main program) may both define `helper`. Returns false when no
    /// file is found.
    /// PY-A: locate a Python module file on disk: (path, is_python_source).
    /// Order as above; the bundled `pylib` comes from
    /// [`crate::middle::pylib::bundled_pylib_dirs`], not from the working
    /// directory, so the library surface is the same wherever zetac is run.
    /// PY-A: resolve a possibly-relative module specifier against the module
    /// currently being loaded. `.` is the current package, `..` its parent,
    /// etc. Absolute names pass through unchanged. A relative import with no
    /// package context (a top-level script) or one that escapes the top-level
    /// package is reported and dropped rather than silently mis-resolved.
    fn resolve_py_module_spec(&self, spec: &str, ctx: Option<&str>) -> Option<String> {
        let dots = spec.chars().take_while(|c| *c == '.').count();
        if dots == 0 {
            return Some(spec.to_string());
        }
        let rest = &spec[dots..];
        let ctx = match ctx {
            Some(c) => c,
            None => {
                eprintln!(
                    "warning: PY-A: relative import `{}` has no package context \
                     (top-level script) — ignored",
                    spec
                );
                return None;
            }
        };
        let pkg = self
            .py_module_pkg
            .borrow()
            .get(ctx)
            .cloned()
            .unwrap_or_default();
        let parts: Vec<&str> = if pkg.is_empty() {
            Vec::new()
        } else {
            pkg.split('.').collect()
        };
        // Level 1 (`.`) anchors at `__package__`; each extra dot drops one
        // trailing component.
        let drop = dots - 1;
        if drop > parts.len() {
            eprintln!(
                "warning: PY-A: relative import `{}` goes beyond the top-level \
                 package — ignored",
                spec
            );
            return None;
        }
        let base = parts[..parts.len() - drop].join(".");
        let full = if rest.is_empty() {
            base
        } else if base.is_empty() {
            rest.to_string()
        } else {
            format!("{}.{}", base, rest)
        };
        if full.is_empty() {
            eprintln!(
                "warning: PY-A: relative import `{}` resolved to an empty module — ignored",
                spec
            );
            None
        } else {
            Some(full)
        }
    }

    fn find_py_module_file(&self, module: &str) -> Option<(std::path::PathBuf, bool)> {
        self.find_py_module_file_ranked(module)
            .map(|(p, is_py, _)| (p, is_py))
    }

    /// Same search, plus the ancestor rank it matched on: 0 is the directory of
    /// the file being compiled, 1..=5 its ancestors, `None` an explicitly
    /// configured base. Silent by design — the probe sites that only ask
    /// "does this resolve" share this function, so warning here would turn one
    /// import into two identical lines (measured). Only
    /// [`Self::load_user_python_module`] speaks.
    fn find_py_module_file_ranked(
        &self,
        module: &str,
    ) -> Option<(std::path::PathBuf, bool, Option<usize>)> {
        let rel: std::path::PathBuf = module.split('.').collect();
        // (base, ancestor rank). Rank 0 is the directory of the file being
        // compiled; 1..=5 are its ancestors. `None` marks an explicitly
        // configured base.
        let mut bases: Vec<(std::path::PathBuf, Option<usize>)> = Vec::new();
        // The file being compiled wins, then explicitly configured paths, then
        // installed packages, then the bundled shim sources.
        if let Some(d) = self.py_source_dir.borrow().clone() {
            // The file being compiled wins — and so do its ANCESTORS: a
            // project-root-relative dotted import (`from strategies.code.jq_shim
            // import …` compiled from strategies/code/jq_wufu.py) resolves
            // against the repo root, not against the file's own directory.
            // Depth is capped so a plain name can never match `/a.py`.
            //
            // The same cap that makes that import work also lets a file in
            // `$HOME` outrank `pylib`, and it makes the reading depend on how
            // deep the checkout sits — same content, one directory deeper, and a
            // module silently stops resolving. Ancestors >0 therefore speak
            // (W1005): the resolution itself stays unchanged.
            let mut cur = Some(d.as_path());
            for rank in 0..6 {
                match cur {
                    Some(p) => {
                        bases.push((p.to_path_buf(), Some(rank)));
                        cur = p.parent();
                    }
                    None => break,
                }
            }
        }
        if let Ok(p) = std::env::var("ZETA_PYLIB") {
            bases.push((std::path::PathBuf::from(p), None));
        }
        bases.push((crate::middle::pylib::packages_dir(), None));
        for d in crate::middle::pylib::bundled_pylib_dirs() {
            bases.push((d, None));
        }
        bases.push((std::path::PathBuf::from("build/stubs"), None));
        for (base, rank) in &bases {
            let pkg = base.join(&rel);
            let mut single_py = pkg.clone();
            single_py.set_extension("py");
            let mut single_z = pkg.clone();
            single_z.set_extension("z");
            let init_py = pkg.join("__init__.py");
            let init_z = pkg.join("__init__.z");
            // Single-file module X.{py,z}, then directory package X/__init__.{py,z}.
            for (p, is_py) in [
                (single_py.as_path(), true),
                (single_z.as_path(), false),
                (init_py.as_path(), true),
                (init_z.as_path(), false),
            ] {
                if !p.is_file() {
                    continue;
                }
                return Some((p.to_path_buf(), is_py, *rank));
            }
        }
        None
    }

    fn load_user_python_module(&mut self, module: &str) -> bool {
        if !self.py_loaded_modules.borrow_mut().insert(module.to_string()) {
            return true; // already loaded (or currently loading)
        }
        let (path, is_py) = match self.find_py_module_file_ranked(module) {
            Some((p, is_py, Some(rank))) if rank > 0 => {
                let origin = self
                    .py_source_dir
                    .borrow()
                    .as_ref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                eprintln!(
                    "warning: [W1005] PY-A: module `{module}` resolved by walking {rank} \
                     level(s) up from {origin} to {} — an ancestor directory outranks \
                     `pylib`, so this import is location-sensitive",
                    p.display()
                );
                (p, is_py)
            }
            Some((p, is_py, _)) => (p, is_py),
            None => {
                // The bare spelling of an ALREADY-LOADED module (`from jq_shim
                // import X` while the file was loaded as
                // `strategies.code.jq_shim`): the project puts the strategy
                // directory on sys.path at runtime, so both spellings are the
                // same file. Without this the consumer emitted
                // `jq_shim__new_context` while the definition lived under
                // `strategies_code_jq_shim__new_context` (undefined). Alias onto
                // the loaded module when exactly one matches by basename.
                let hits: Vec<String> = self
                    .py_loaded_modules
                    .borrow()
                    .iter()
                    .filter(|m| {
                        m.rsplit('.').next() == Some(module)
                            || m.split('.').last() == Some(module)
                    })
                    .cloned()
                    .collect();
                if hits.len() == 1 {
                    self.py_module_aliases
                        .borrow_mut()
                        .insert(module.to_string(), hits[0].clone());
                    return true;
                }
                self.py_loaded_modules.borrow_mut().remove(module);
                return false;
            }
        };
        // Mark this module as a USER MODULE **before** parsing/registering it:
        // the registration below recurses into `register`, whose import handling
        // resolves members of imported modules. During a CIRCULAR import the
        // target is still mid-load, so `py_user_modules` was not populated yet,
        // `is_user` came out false and every member reference was reported as
        // "unknown member — external shim" ⇒ bare symbols (261 such warnings in
        // the REasyQuant local-backtest compile, e.g. `get_cost_config`).
        self.py_user_modules.borrow_mut().insert(module.to_string());
        // `__file__` is per module: record which file this module came from so
        // its own defs read their own path (see `MirGen::with_py_module_paths`).
        self.py_module_paths
            .borrow_mut()
            .insert(module.to_string(), path.to_string_lossy().to_string());
        // Same FILE under another name? Alias onto the already-loaded module so
        // both spellings share one prefix (`<canonical>__member`). Without this
        // the second spelling defined/emit lookups under a DIFFERENT prefix and
        // every member reference became an undefined symbol (measured:
        // `_jq_shim__new_context` / `_jq_shim___LocalPortfolio`).
        {
            let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            let existing = self.py_loaded_paths.borrow().get(&key).cloned();
            match existing {
                Some(canon) if canon != module => {
                    self.py_module_aliases
                        .borrow_mut()
                        .insert(module.to_string(), canon);
                    return true;
                }
                Some(_) => {}
                None => {
                    self.py_loaded_paths
                        .borrow_mut()
                        .insert(key, module.to_string());
                }
            }
        }
        // `__package__` anchor for relative imports: a package's `__init__`
        // anchors to itself; a submodule anchors to its parent package.
        {
            let is_pkg = path
                .file_stem()
                .map(|s| s == "__init__")
                .unwrap_or(false);
            let pkg = if is_pkg {
                module.to_string()
            } else {
                module
                    .rsplit_once('.')
                    .map(|(p, _)| p.to_string())
                    .unwrap_or_default()
            };
            self.py_module_pkg.borrow_mut().insert(module.to_string(), pkg);
        }
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "warning: PY-A: cannot read module `{}` ({}): {}",
                    module,
                    path.display(),
                    e
                );
                return false;
            }
        };
        let src = src.trim_start_matches('\u{FEFF}').to_string();
        let pre = if is_py {
            match crate::frontend::indent::indent_preprocess(&src) {
                Ok(Some(t)) => t,
                Ok(None) => src.clone(),
                Err(_) => src.clone(),
            }
        } else {
            src.clone()
        };
        crate::frontend::parser::top_level::set_parsing_imported_module(true);
        let asts = match crate::frontend::indent::parse_zeta_tagged(&path.to_string_lossy(), &pre) {
            Ok((_rem, a)) => a,
            Err(_) => {
                crate::frontend::parser::top_level::set_parsing_imported_module(false);
                eprintln!("warning: PY-A: cannot parse module file {}", path.display());
                return false;
            }
        };
        crate::frontend::parser::top_level::set_parsing_imported_module(false);
        let prefix = format!("{}__", module.replace('.', "_"));
        // Registrations below recurse into `register`, whose import handling
        // resolves relative specifiers — it must see THIS module as context.
        let saved_ctx = self.py_current_module.replace(Some(module.to_string()));
        // `parse_zeta` folds a module's top-level statements into a synthesized
        // `fn main`. Split it back out: definitions are registered (mangled),
        // the statements become `<prefix>init()` which runs once at import time
        // — Python executes a module body on import, and skipping it left
        // module-level constants silently unbound (`cfg.get()` returned 0).
        let mut defs: Vec<AstNode> = Vec::new();
        let mut body_stmts: Vec<AstNode> = Vec::new();
        // BATCH-290: an imported module that DEFINES `main` gets its module
        // statements in the `__zeta_module_body__` carrier instead of merged
        // into `main` — then `main` is a regular (mangled) definition and only
        // the statements run at import. Without a carrier, the synthesized
        // `main` IS the statement list (library modules, historical path).
        let has_carrier = asts.iter().any(|a| {
            matches!(a, AstNode::FuncDef { name, .. } if name == "__zeta_module_body__")
        });
        for a in asts {
            match a {
                AstNode::FuncDef { ref name, ref body, .. }
                    if name == "__zeta_module_body__" || (!has_carrier && name == "main") =>
                {
                    body_stmts.extend(body.iter().cloned());
                }
                other => defs.push(other),
            }
        }
        let mut own: std::collections::HashSet<String> = std::collections::HashSet::new();
        for a in &defs {
            if let Some(n) = definition_name(a) {
                own.insert(n.to_string());
            }
        }
        // Module-level bindings are part of the module namespace too, so the
        // module's own functions can read them (and they must be prefixed, or
        // two modules would share one slot).
        for stmt in &body_stmts {
            for n in module_level_bindings(stmt) {
                own.insert(n);
            }
        }
        // PY-A: `_LocalPortfolio = LocalBackend` — same-module class/func alias.
        // own_names alone mangled the alias to `mod___LocalPortfolio` (ghost
        // extern); the real ctor is `mod__LocalBackend`. Record as a re-export
        // so `from mod import _LocalPortfolio` / renames chase the target
        // (jq_shim / jq_wufu_local).
        {
            let mut rex = self.py_module_reexports.borrow_mut();
            for stmt in &body_stmts {
                collect_same_module_aliases(stmt, module, &own, &mut rex);
            }
        }
        // `synthesize_implicit_main` injected `zeta_module_decl("NAME")` markers
        // for the module's bare assignments. They must carry the module prefix
        // too, or every module's `LIMIT` would share one global slot.
        let body_stmts: Vec<AstNode> = body_stmts
            .into_iter()
            .map(|s| prefix_module_decl_markers(s, &prefix))
            .collect();
        for a in defs {
            let mangled = match a {
                AstNode::FuncDef { .. }
                | AstNode::ConstDef { .. }
                | AstNode::StructDef { .. }
                | AstNode::EnumDef { .. } => rename_definition(a, &prefix),
                other => other,
            };
            if let Some(n) = definition_name(&mangled) {
                self.py_mangled_to_module
                    .borrow_mut()
                    .insert(n.to_string(), module.to_string());
            }
            self.register(mangled);
        }
        // Module-level names become env-routed globals under their prefixed
        // name, so reads from the module's functions see what `init` wrote.
        {
            let mut globals = self.module_globals.borrow_mut();
            for n in &own {
                globals.insert(format!("{}{}", prefix, n));
            }
        }
        // `<prefix>init()` — idempotent module body.
        let init_name = format!("{}init", prefix);
        let inited_key = format!("__inited__{}", module);
        let mut init_body: Vec<AstNode> = vec![
            // if zeta_env_get("<key>") != 0 { return 0 }
            AstNode::If {
                cond: Box::new(AstNode::BinaryOp {
                    op: "!=".to_string(),
                    left: Box::new(AstNode::Call {
                        receiver: None,
                        method: "zeta_env_get".to_string(),
                        args: vec![AstNode::StringLit(inited_key.clone())],
                        type_args: vec![],
                        structural: false,
                    }),
                    right: Box::new(AstNode::Lit(0)),
                }),
                then: vec![AstNode::Return(Box::new(AstNode::Lit(0)))],
                else_: vec![],
            },
            AstNode::ExprStmt {
                expr: Box::new(AstNode::Call {
                    receiver: None,
                    method: "zeta_env_set".to_string(),
                    args: vec![AstNode::StringLit(inited_key), AstNode::Lit(1)],
                    type_args: vec![],
                    structural: false,
                }),
            },
        ];
        init_body.extend(body_stmts);
        self.py_mangled_to_module
            .borrow_mut()
            .insert(init_name.clone(), module.to_string());
        self.register(AstNode::FuncDef {
            name: init_name,
            generics: Vec::new(),
            lifetimes: Vec::new(),
            params: Vec::new(),
            ret: "i64".to_string(),
            body: init_body,
            attrs: Vec::new(),
            ret_expr: None,
            single_line: false,
            doc: String::new(),
            pub_: false,
            async_: false,
            const_: false,
            comptime_: false,
            where_clauses: Vec::new(),
        });
        self.py_module_own_names
            .borrow_mut()
            .insert(module.to_string(), own);
        self.py_user_modules.borrow_mut().insert(module.to_string());
        // `__file__` is per module: record which file this module came from so
        // its own defs read their own path (see `MirGen::with_py_module_paths`).
        self.py_module_paths
            .borrow_mut()
            .insert(module.to_string(), path.to_string_lossy().to_string());
        self.py_current_module.replace(saved_ctx);
        eprintln!("PY-A: imported module `{}` from {}", module, path.display());
        true
    }

    /// PY-A: chase facade re-exports (`pkg.f` → `pkg.sub.f` → …) to the
    /// defining module. Caps the chain so a cycle cannot hang the compiler.
    fn resolve_reexport_target(&self, module: &str, member: &str) -> (String, String) {
        let mut m = module.to_string();
        let mut n = member.to_string();
        for _ in 0..32 {
            let next = self
                .py_module_reexports
                .borrow()
                .get(&m)
                .and_then(|map| map.get(&n).cloned());
            match next {
                Some((nm, nn)) if nm != m || nn != n => {
                    m = nm;
                    n = nn;
                }
                _ => break,
            }
        }
        (m, n)
    }

    /// PY-A: does this user module have an import-time initializer?
    pub fn py_module_init_symbol(&self, module: &str) -> Option<String> {
        if !self.py_user_modules.borrow().contains(module) {
            return None;
        }
        Some(format!("{}__init", module.replace('.', "_")))
    }

    /// PY-A: rename map for the function being lowered — bare references to a
    /// module's own top-level names are redirected to their `mod__` mangled
    /// form, so a module's internals resolve without rewriting its whole AST.
    /// Re-exported names map to the *defining* module's mangled symbol so a
    /// later `from pkg import f` cannot overwrite the global alias table and
    /// break `pkg.compute()` (which still needs `pkg_sub__f`).
    fn module_renames_for(&self, func_name: &str) -> std::collections::HashMap<String, String> {
        let mut out = std::collections::HashMap::new();
        let module = match self.py_mangled_to_module.borrow().get(func_name) {
            Some(m) => m.clone(),
            None => {
                // A METHOD's MIR name is `Class::method` and is NOT a key in
                // `py_mangled_to_module` (which holds definitions), so its rename
                // table was empty and module-private helpers referenced from a
                // method body stayed bare (`_to_ts`, 8 reference sites). Match the
                // class against the module that OWNS it, and build the table from
                // OWN NAMES ONLY: the re-exports half produced fresh ghosts
                // (`_pd.Timestamp__date`, `_filter`) when applied inside methods.
                let head = func_name.split("::").next().unwrap_or("").to_string();
                // `head` is the MANGLED class name (e.g. "pandas__DataFrame").
                // `py_module_own_names` stores BARE names (e.g. "DataFrame").
                // Strip the module prefix from `head` to recover the bare class
                // name before searching — otherwise methods never get a rename
                // table and `DataFrame(...)` inside a method body falls through
                // to `zeta_platform_obj` (wrong memory layout ⇒ crash at
                // `map_keys`; measured: t404 with `import pandas as pd`).
                let bare_head = {
                    let own = self.py_module_own_names.borrow();
                    let mut found = head.clone();
                    for (m, _) in own.iter() {
                        let prefix = format!("{}__", m.replace('.', "_"));
                        if let Some(stripped) = found.strip_prefix(&prefix) {
                            found = stripped.to_string();
                            break;
                        }
                    }
                    found
                };
                let hits: Vec<String> = if bare_head.is_empty() {
                    Vec::new()
                } else {
                    self.py_module_own_names
                        .borrow()
                        .iter()
                        .filter(|(_, names)| names.contains(&bare_head))
                        .map(|(m, _)| m.clone())
                        .collect()
                };
                if hits.len() == 1 {
                    let module = hits.into_iter().next().unwrap();
                    let prefix = format!("{}__", module.replace('.', "_"));
                    if let Some(own) = self.py_module_own_names.borrow().get(&module) {
                        for n in own {
                            out.insert(n.clone(), format!("{}{}", prefix, n));
                        }
                    }
                }
                return out;
            }
        };
        if let Some(own) = self.py_module_own_names.borrow().get(&module) {
            let prefix = format!("{}__", module.replace('.', "_"));
            for n in own {
                out.insert(n.clone(), format!("{}{}", prefix, n));
            }
        }
        if let Some(reexports) = self.py_module_reexports.borrow().get(&module) {
            for (alias, (src_mod, src_mem)) in reexports {
                // Registry shims (`pathlib.Path` → `py_path_new`,
                // `datetime.timedelta` → `py_dt_timedelta`) must NOT be
                // rewritten to `mod__name`. That rename ran before
                // `py_member_call` and turned `_Path(...)` /
                // `timedelta(...)` into unresolved `pathlib__Path` /
                // `datetime__timedelta` externs (jq_wufu_local).
                // Leave them to `py_member_aliases` → registry symbol.
                if crate::middle::pylib::find_member(src_mod, src_mem).is_some() {
                    continue;
                }
                // Only rename when `src_mod` really IS a module (loaded from disk
                // or known to the registry). A dotted ATTRIBUTE path
                // (`pd.Timestamp.date`) is not, and renaming `date` to
                // `pd.Timestamp__date` produced a ghost symbol.
                if !self.py_user_modules.borrow().contains(src_mod)
                    && crate::middle::pylib::find_module(src_mod).is_none()
                {
                    continue;
                }
                // Same canonicalization as `py_member_call`: the definition may
                // live under the OTHER spelling of the same file
                // (`jq_shim__OrderCost` vs `strategies_code_jq_shim__OrderCost`).
                let canon = self
                    .py_module_aliases
                    .borrow()
                    .get(src_mod)
                    .cloned()
                    .unwrap_or_else(|| src_mod.clone());
                let mangled = format!("{}__{}", canon.replace('.', "_"), src_mem);
                out.insert(alias.clone(), mangled);
            }
        }
        out
    }

    /// Does this function's body `return json.loads(...)`?
    fn returns_json_loads(body: &[AstNode]) -> bool {
        fn is_json_loads(n: &AstNode) -> bool {
            match n {
                AstNode::Call {
                    receiver: Some(recv),
                    method,
                    ..
                } => {
                    if let AstNode::Var(v) = &**recv {
                        if v == "json" && method == "loads" {
                            return true;
                        }
                    }
                    false
                }
                _ => false,
            }
        }
        body.iter().any(|st| match st {
            AstNode::Return(e) => is_json_loads(e),
            AstNode::Block { body } => Self::returns_json_loads(body),
            AstNode::If { then, else_, .. } => {
                Self::returns_json_loads(then) || Self::returns_json_loads(else_)
            }
            _ => false,
        })
    }

    /// Batch 399: parameter types as the SIGNATURE TABLE holds them, which is what
    /// the call-site refinement (see the `registered_funcs` rewrite) updates. The
    /// AST copies in `registered_func_defs` keep their original `()` annotations,
    /// so anything that infers from a body must read the table to see the same
    /// parameters the callee is compiled with.
    fn sig_params_snapshot(&self) -> HashMap<String, Vec<(String, Type)>> {
        self.funcs
            .iter()
            .map(|(n, (params, _, _))| (n.clone(), params.clone()))
            .collect()
    }

    /// Batch 300: the return type of an UNANNOTATED `def`, recovered from its own
    /// `return` statements. With no `-> …` the signature registers as UNIT, and
    /// that unit flows into the caller: `pf = make(1000.0)` leaves `pf` with no
    /// struct to index, so every later `pf.<attr>` silently reads field 0 of an
    /// empty variant and `pf.positions.values()` links to the `_values` stub
    /// (measured: /tmp/wl/z300/b4.z printed `cash 4652007308841189376` — the bit
    /// pattern of 1000.0 — then aborted).
    /// Only syntactic shapes are recovered, and only when EVERY `return` in the
    /// body agrees; anything uncertain keeps the previous answer.
    /// Batch 399 (类型基础②): `sig_params` is the resolver's OWN signature table
    /// (`funcs`), whose parameter types include the call-site refinement that
    /// rewrites the AST string in `registered_funcs` — but NOT the parallel
    /// `registered_func_defs` copy this recovery reads. Without the overlay, a
    /// function that merely forwards a float it received
    /// (`def forward(v): return v`, called as `forward(1.25)`) recovered no type
    /// here, so the caller's slot stayed i64 while the callee's signature was
    /// already `double` (measured: `h=4608308318706860032`, 1.25's bit pattern).
    fn unannotated_return_ty(
        defs: &[std::rc::Rc<AstNode>],
        name: &str,
        classes: &[String],
        fn_rets: &HashMap<String, Type>,
        sig_params: &HashMap<String, Vec<(String, Type)>>,
    ) -> Option<Type> {
        fn is_unit(t: &Type) -> bool {
            matches!(t, Type::Tuple(inner) if inner.is_empty())
        }

        /// A class named as written in source: bare, or module-mangled
        /// (`LocalBackend` → `backend_strategy_wufu_backend__LocalBackend`).
        /// An ambiguous suffix is NO type rather than a guess.
        fn class_ty(named: &str, classes: &[String]) -> Option<Type> {
            if classes.iter().any(|c| c == named) {
                return Some(Type::Named(named.to_string(), vec![]));
            }
            let suffix = format!("__{}", named);
            let mut hits = classes.iter().filter(|c| c.ends_with(&suffix));
            let first = hits.next()?.clone();
            if hits.next().is_some() {
                return None;
            }
            Some(Type::Named(first, vec![]))
        }

        /// `from <module> import <member> as <alias>` desugars to
        /// `zeta_py_from(module, member, alias)` string literals.
        fn import_alias(n: &AstNode) -> Option<(String, String)> {
            match n {
                AstNode::Call {
                    receiver: None,
                    method,
                    args,
                    ..
                } if method == "zeta_py_from" => {
                    let s: Vec<String> = args
                        .iter()
                        .filter_map(|a| match a {
                            AstNode::StringLit(x) => Some(x.clone()),
                            _ => None,
                        })
                        .collect();
                    if s.len() >= 3 && !s[2].is_empty() {
                        Some((s[2].clone(), s[1].clone()))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }

        fn infer(
            n: &AstNode,
            seen: &HashMap<String, Type>,
            aliases: &HashMap<String, String>,
            classes: &[String],
            fn_rets: &HashMap<String, Type>,
        ) -> Option<Type> {
            match n {
                AstNode::Var(v) => seen.get(v).cloned(),
                AstNode::StringLit(_) | AstNode::FString(_) => Some(Type::Str),
                AstNode::FloatLit(_) => Some(Type::F64),
                AstNode::Lit(_) => Some(Type::I64),
                AstNode::Bool(_) => Some(Type::Bool),
                // Batch 399: arithmetic that the lowering will type FLOAT. The rule is
                // `gen.rs`'s own (its numeric arms, e.g. gen.rs:4323-4330): any float
                // operand on `+ - * / // % **` makes the result F64. `//` is spelled
                // `floordiv` by the time it reaches the AST (indent.rs rewrites it into
                // a WORD so the two dialects stay distinguishable), so both spellings
                // count. NOTHING is claimed for int-only operands — measured on the PRE
                // compiler `tdiv(3, 2)` → `1`, `fdiv(7, 2)` → `3`, `mdiv(7, 2)` → `7`,
                // i.e. `/` and `%` disagree with CPython on their own; that is the
                // numeric-dialect family (#46/#48), not this batch's business.
                // Comparison and `and`/`or` are deliberately NOT inferred either: this
                // batch only ever fills a blank slot, and a Bool/PyDynamic guess has no
                // measured member today.
                AstNode::BinaryOp { op, left, right } => {
                    let l = infer(left, seen, aliases, classes, fn_rets);
                    let r = infer(right, seen, aliases, classes, fn_rets);
                    // Batch 575: comparisons and membership ALWAYS yield Bool
                    // in Python — inferable, unlike and/or (value-selecting).
                    // class_tag_probe: `return t in self.tags` recovered Bool,
                    // so has_tag("web") prints True instead of the raw 1.
                    if matches!(
                        op.as_str(),
                        "==" | "!=" | "<" | ">" | "<=" | ">=" | "in" | "not in"
                    ) {
                        return Some(Type::Bool);
                    }
                    let is_f = |t: Option<&Type>| matches!(t, Some(Type::F32) | Some(Type::F64));
                    if matches!(
                        op.as_str(),
                        "+" | "-" | "*" | "/" | "//" | "floordiv" | "%" | "**"
                    ) && (is_f(l.as_ref()) || is_f(r.as_ref()))
                    {
                        Some(Type::F64)
                    } else if op == "+"
                        && (matches!(l.as_ref(), Some(Type::Str))
                            && r.as_ref().map_or(true, |t| matches!(t, Type::Str))
                            || matches!(r.as_ref(), Some(Type::Str))
                                && l.as_ref().map_or(true, |t| matches!(t, Type::Str)))
                    {
                        // Batch 587: Python has no implicit str↔num `+`, so a
                        // `+` with one Str side and the other Str-or-unknown is
                        // concatenation — infer Str instead of vetoing the
                        // whole function's recovered return (`"hello " +
                        // self.name()` printed the pointer before this).
                        Some(Type::Str)
                    } else {
                        None
                    }
                }
                AstNode::StructLit { variant, .. } => class_ty(variant, classes),
                AstNode::DictLit { entries } => Some(Type::Named(
                    "map".to_string(),
                    match entries.first() {
                        Some((k, v)) => vec![
                            infer(k, seen, aliases, classes, fn_rets).unwrap_or(Type::Str),
                            infer(v, seen, aliases, classes, fn_rets).unwrap_or(Type::I64),
                        ],
                        None => vec![],
                    },
                )),
                AstNode::ArrayLit(items) => Some(Type::DynamicArray(Box::new(
                    items
                        .first()
                        .and_then(|e| infer(e, seen, aliases, classes, fn_rets))
                        .unwrap_or(Type::I64),
                ))),
                // Batch 579: `x in y` desugars to `y.__contains__(x)` —
                // membership is ALWAYS Bool in Python (class_tag_probe:
                // has_tag returned the un-inferable call and stayed I64).
                AstNode::Call { method, .. } if method == "__contains__" => {
                    Some(Type::Bool)
                }
                // Batch 592: a list comprehension desugars to
                // `__collect__(iter, λ)` — the recovered return type must see
                // it as DynamicArray(element). Without this arm a method
                // returning a comprehension was vetoed to I64 and `print`
                // rendered the raw handle (class_str_comprehension: `summary`
                // printed the pointer while `longest` was already right).
                // The element type comes from the lambda body's expression —
                // for the filtered form that is the `if cond { EXPR } else {
                // -1 }` then-branch.
                AstNode::Call { method, args, .. } if method == "__collect__" => {
                    let elem: Option<&AstNode> = args.get(1).and_then(|lam| match lam {
                        AstNode::Closure { body, .. } => match &**body {
                            AstNode::If { then, .. } => then.first().and_then(|s| match s {
                                AstNode::ExprStmt { expr } => Some(expr.as_ref()),
                                _ => None,
                            }),
                            other => Some(other),
                        },
                        _ => None,
                    });
                    let et = elem.and_then(|e| infer(e, seen, aliases, classes, fn_rets));
                    Some(Type::DynamicArray(Box::new(et.unwrap_or(Type::I64))))
                }
                // Batch 580: `sep.join(iterable)` on a Str receiver yields
                // Str (tag_str()-style methods stayed I64-typed and printed
                // the joined string's POINTER).
                AstNode::Call {
                    receiver: Some(recv),
                    method: m2,
                    ..
                } if m2 == "join"
                    || matches!(
                        m2.as_str(),
                        "upper"
                            | "lower"
                            | "capitalize"
                            | "title"
                            | "strip"
                            | "lstrip"
                            | "rstrip"
                            | "trim"
                            | "trim_start"
                            | "trim_end"
                            | "to_string"
                    ) =>
                {
                    // Batch 587: the str-method family on an unknown-typed
                    // receiver yields Str (the same fallback the codegen
                    // uses); `join` additionally requires a Str receiver.
                    match (
                        m2 == "join",
                        infer(recv, seen, aliases, classes, fn_rets),
                    ) {
                        (_, Some(Type::Str)) => Some(Type::Str),
                        (_, None) => Some(Type::Str),
                        _ => None,
                    }
                }
                AstNode::Call {
                    receiver: None,
                    method,
                    ..
                } => {
                    let named = aliases
                        .get(method)
                        .cloned()
                        .unwrap_or_else(|| method.clone());
                    // The ctor wins over the function table: a synthesized ctor is
                    // registered as a function returning I64, which would shadow
                    // the real struct type (same ordering reason as batch 291).
                    if let Some(t) = class_ty(&named, classes) {
                        return Some(t);
                    }
                    match fn_rets.get(&named) {
                        Some(t) if !is_unit(t) => Some(t.clone()),
                        _ => None,
                    }
                }
                _ => None,
            }
        }

        /// Sequential scan: assignments build the local scope, `return`s
        /// contribute candidates. `None` in `cands` records an un-inferable
        /// `return`, which vetoes the whole function.
        fn walk(
            body: &[AstNode],
            seen: &mut HashMap<String, Type>,
            aliases: &mut HashMap<String, String>,
            cands: &mut Vec<Option<Type>>,
            classes: &[String],
            fn_rets: &HashMap<String, Type>,
        ) {
            for st in body {
                match st {
                    AstNode::Assign(lhs, rhs) => {
                        // `m: dict[str, str] = {}` parses with the name wrapped in
                        // a `TypeAnnotatedPattern`, so unwrapping it here is what
                        // keeps `m` in scope for the `return m` below. Missing it
                        // vetoed the whole function back to UNIT (batch 413:
                        // `p = probe()` then typed the call dest `Tuple([])`, and
                        // `len(p)` dispatched `array_len` — read 0, and on the real
                        // corpus a SIGSEGV in `map_insert`).
                        let var = match &**lhs {
                            AstNode::Var(v) => Some(v.clone()),
                            AstNode::TypeAnnotatedPattern { pattern, .. } => {
                                match &**pattern {
                                    AstNode::Var(v) => Some(v.clone()),
                                    _ => None,
                                }
                            }
                            _ => None,
                        };
                        if let Some(v) = var {
                            if let Some(t) = infer(rhs, seen, aliases, classes, fn_rets) {
                                seen.insert(v, t);
                            }
                        }
                    }
                    AstNode::Return(e) => {
                        cands.push(infer(e, seen, aliases, classes, fn_rets));
                    }
                    AstNode::Block { body } => {
                        walk(body, seen, aliases, cands, classes, fn_rets)
                    }
                    AstNode::If { then, else_, .. } => {
                        walk(then, seen, aliases, cands, classes, fn_rets);
                        walk(else_, seen, aliases, cands, classes, fn_rets);
                    }
                    AstNode::For { body, .. } | AstNode::While { body, .. } => {
                        walk(body, seen, aliases, cands, classes, fn_rets)
                    }
                    other => {
                        if let Some((alias, member)) = import_alias(other) {
                            aliases.insert(alias, member);
                        }
                        // Batch 587: a statement-position call reaches this walk in
                        // TWO shapes — `ExprStmt`-wrapped (block parser) and a BARE
                        // `Call` (loop bodies; measured on `result.append(t.upper())`
                        // inside `for`, which arrived unwrapped and made every
                        // ExprStmt-keyed refinement dead code). Unwrap both.
                        let stmt_call: Option<&AstNode> = match other {
                            AstNode::ExprStmt { expr } => {
                                if let Some((alias, member)) = import_alias(expr) {
                                    aliases.insert(alias, member);
                                }
                                Some(expr.as_ref())
                            }
                            other @ AstNode::Call { .. } => Some(other),
                            _ => None,
                        };
                        if let Some(AstNode::Call {
                            receiver: Some(recv),
                            method,
                            args,
                            ..
                        }) = stmt_call
                        {
                            // `result.append(v)` — a concrete non-I64 appended value
                            // widens an I64-placeholder vec's element type in `seen`,
                            // so the fn's recovered return carries it (`up_all`
                            // returned vec<i64> and `r[0]` rendered the raw pointer
                            // before this).
                            if method == "append"
                                && args.len() == 1
                                && let AstNode::Var(v) = &**recv
                                && let Some(Type::DynamicArray(e)) =
                                    seen.get(v.as_str()).cloned()
                                && matches!(*e, Type::I64)
                                && let Some(vt) =
                                    infer(&args[0], seen, aliases, classes, fn_rets)
                                && !matches!(
                                    vt,
                                    Type::I64 | Type::PyDynamic | Type::Variable(_)
                                )
                                && !is_unit(&vt)
                            {
                                seen.insert(v.clone(), Type::DynamicArray(Box::new(vt)));
                            }
                        }
                    }
                }
            }
        }

        let tail = name.rsplit("::").next().unwrap_or(name);
        let exact = defs.iter().find(|d| {
            matches!(d.as_ref(), AstNode::FuncDef { name: n, .. } if n == name)
        });
        let def: &AstNode = match exact {
            Some(d) => Some(d.as_ref()),
            // A method is keyed `Type::method` while the definition carries the
            // bare name — accept it only when exactly one definition matches, so
            // two classes sharing a method name cannot cross-contaminate.
            None => {
                let hits: Vec<&AstNode> = defs
                    .iter()
                    .filter(|d| {
                        matches!(d.as_ref(), AstNode::FuncDef { name: n, .. }
                            if n.rsplit("::").next().unwrap_or("") == tail)
                    })
                    .map(|d| d.as_ref())
                    .collect();
                if hits.len() == 1 {
                    Some(hits[0])
                } else {
                    None
                }
            }
        }?;
        let (params, body, ret_expr) = match def {
            AstNode::FuncDef {
                params,
                ret,
                body,
                ret_expr,
                ..
            } => {
                // The Python parser spells "no annotation" `()` (and the Zeta one
                // leaves it empty); anything else is a declared type to respect.
                let unannotated = matches!(ret.trim(), "" | "()");
                // Batch 451: a `class` method has NO annotation either, yet the
                // desugaring hard-writes `i64` (`top_level.rs:955-965` only writes
                // `str` for a bare StringLit `return`), so the `()` test above never
                // fires for methods and every caller typed the result as an integer
                // (measured: `class B: def r(self): out = "a"; return out` printed
                // `4333154800` — the str pointer — while `len(B().r())` was 1).
                // The receiver's TYPE separates the parser default from a written
                // annotation: the desugaring types `self` with the CLASS name
                // (`top_level.rs:944`) while a hand-written `impl` types it `Self`
                // (`top_level.rs:53`), so zeta-mode `-> i64` methods stay untouched.
                let py_method_default_i64 = ret.trim() == "i64"
                    && matches!(params.first(), Some((pn, pt))
                        if ((pn == "self" || pn == "&self" || pn == "&mut self")
                            && pt.trim() != "Self"
                            && class_ty(pt.trim(), classes).is_some())
                            /* @classmethod (backlog #195): the desugar keeps
                               `cls` as a plain parameter — no receiver for the
                               self-arm to see — so the class-table tie is
                               replaced by "first param is an UNANNOTATED cls",
                               which only the classmethod path produces (a
                               zeta-side `fn f(cls: i64)` carries a real type
                               and stays untouched). */
                            || (pn == "cls" && pt.trim() == "dyn"));
                if !unannotated && !py_method_default_i64 {
                    return None;
                }
                (params, body, ret_expr)
            }
            _ => return None,
        };
        let mut seen: HashMap<String, Type> = HashMap::new();
        for (pn, pt) in params {
            if pn == "self" {
                continue;
            }
            let t = Type::from_string(pt);
            if !is_unit(&t) {
                seen.insert(pn.clone(), t);
            }
        }
        // Batch 399: the signature table is where a parameter's type actually lives.
        // An un-annotated Python parameter is spelled `"dyn"` in the AST, which seeds
        // as PyDynamic — "nothing known yet" — while the SAME parameter carries a
        // refined type in `funcs` as soon as a call site proves one (the call-site
        // rewrite at the `registered_funcs` update). Measured on the PRE compiler,
        // `def forward(v): return v` reached this point with `seen={"v": PyDynamic}`
        // and `sig=[("v", F64)]`, so the caller's slot stayed int while the callee
        // was already compiled `double` → `h=4608308318706860032`, 1.25's bit pattern.
        // A real declaration always wins; PyDynamic/Variable is the table's own
        // spelling of "still unknown" and never becomes a claimed type.
        if let Some(sig) = sig_params.get(name) {
            for (pn, pt) in sig {
                if pn == "self" || matches!(pt, Type::PyDynamic | Type::Variable(_)) {
                    continue;
                }
                let blank = matches!(
                    seen.get(pn),
                    None | Some(Type::PyDynamic) | Some(Type::Variable(_))
                );
                if blank {
                    seen.insert(pn.clone(), pt.clone());
                }
            }
        }
        let mut aliases: HashMap<String, String> = HashMap::new();
        let mut cands: Vec<Option<Type>> = Vec::new();
        walk(body, &mut seen, &mut aliases, &mut cands, classes, fn_rets);
        if let Some(e) = ret_expr {
            cands.push(infer(e, &seen, &aliases, classes, fn_rets));
        }
        let mut it = cands.iter();
        let first = it.next()?.as_ref()?;
        if is_unit(first) || !it.all(|c| c.as_ref() == Some(first)) {
            return None;
        }
        Some(first.clone())
    }

    /// Batch 299: a `-> dict` annotation carries NO key/value type (`map` with
    /// zero type arguments), so EVERY consumer of the result lost it:

    /// `for pos in portfolio.positions.values()` read struct fields off a raw
    /// handle (the end-of-period valuation printed integers) and `d[k]`
    /// SEGVFAULTED on the miss (`PositionLedger.positions`,
    /// `jq_wufu_local.py:97-147`). When the body returns a dict comprehension,
    /// recover the pair types from the desugared AST
    /// (`{k: V for ...}` → `__collect_dict__(λ. __pack_pair__(k, V))`).
    fn dict_comprehension_ret(body: &[AstNode]) -> Option<Type> {
        fn pair_from(node: &AstNode) -> Option<Type> {
            let args = match node {
                AstNode::Call {
                    receiver: Some(_),
                    method,
                    args,
                    ..
                } if method == "__collect_dict__" && args.len() == 1 => args,
                _ => return None,
            };
            let inner = match &args[0] {
                AstNode::Closure { body, .. } => body,
                _ => return None,
            };
            // The filtered form is `if cond { pack(k, v) } else { -1 }`.
            let pair = match &**inner {
                AstNode::Call { .. } => &**inner,
                AstNode::If { then, .. } => match then.first() {
                    Some(AstNode::ExprStmt { expr }) => &**expr,
                    _ => return None,
                },
                _ => return None,
            };
            let args = match pair {
                AstNode::Call {
                    receiver: None,
                    method,
                    args,
                    ..
                } if method == "__pack_pair__" && args.len() == 2 => args,
                _ => return None,
            };
            let key = match &args[0] {
                AstNode::StringLit(_) | AstNode::FString { .. } => Type::Str,
                _ => Type::I64,
            };
            // Only a type the AST states outright — anything looser would
            // mis-type every caller, so those keep the old (untyped) behavior.
            let val = match &args[1] {
                AstNode::Call {
                    receiver: None,
                    method,
                    ..
                } if method.chars().next().map_or(false, |c| c.is_uppercase()) => {
                    Type::Named(method.clone(), vec![])
                }
                AstNode::StringLit(_) => Type::Str,
                AstNode::FloatLit(_) => Type::F64,
                _ => return None,
            };
            Some(Type::Named("map".to_string(), vec![key, val]))
        }
        body.iter().find_map(|st| match st {
            AstNode::Return(e) => pair_from(e),
            AstNode::Block { body } => Self::dict_comprehension_ret(body),
            AstNode::If { then, else_, .. } => Self::dict_comprehension_ret(then)
                .or_else(|| Self::dict_comprehension_ret(else_)),
            _ => None,
        })
    }

/// `pd.DataFrame` → `DataFrame` (shim struct) anywhere inside a type,
/// including nested type arguments (`tuple[pd.DataFrame, int]`, `[pd.DataFrame]`).
fn shim_class_normalize(t: &Type) -> Type {
    match t {
        Type::Named(n, args) => {
            if let Some(tag) = crate::middle::pylib::handle_tag(n) {
                return Type::Named(tag.to_string(), args.iter().map(Self::shim_class_normalize).collect());
            }
            let last = n.rsplit('.').next().unwrap_or(n.as_str());
            let name = if n.contains('.') && matches!(last, "DataFrame" | "Series" | "GroupBy") {
                last.to_string()
            } else {
                n.clone()
            };
            Type::Named(name, args.iter().map(Self::shim_class_normalize).collect())
        }
        Type::DynamicArray(inner) => Type::DynamicArray(Box::new(Self::shim_class_normalize(inner))),
        // `-> tuple[pd.DataFrame, int]` (`remove_extreme_return_bars`): the frame
        // element kept the qualified name, so the DESTRUCTURED name was typed
        // `pd.DataFrame` and `len(out.columns)` was a MAP lookup (0) while
        // `out["a"]` was a map subscript on a struct handle (SEGV, measured).
        Type::Tuple(items) => Type::Tuple(items.iter().map(Self::shim_class_normalize).collect()),
        other => other.clone(),
    }
}

    pub fn lower_to_mir(&self, ast: &AstNode) -> Mir {
        // Batch 601: the unannotated-return recovery must see the QUALIFIED
        // method defs — the bare originals collide across classes (two
        // `speak` defs → the uniqueness gate vetoed BOTH recoveries → i64).
        // registered_funcs carries the qualified clones with real bodies.
        let mut defs_snapshot = self.registered_func_defs.borrow().clone();
        for (k, v) in self.registered_funcs.iter() {
            if k.contains("::") {
                if let AstNode::FuncDef { .. } = v {
                    defs_snapshot.push(std::rc::Rc::new(v.clone()));
                }
            }
        }
        // Batch 300 inputs for `unannotated_return_ty`. The decl table is keyed by
        // the MANGLED class name, and iteration order must not matter.
        let mut class_names: Vec<String> = self.type_decls.keys().cloned().collect();
        class_names.sort();
        let decl_rets: HashMap<String, Type> = self
            .get_all_func_signatures()
            .iter()
            .map(|(n, (_, r, _))| (n.clone(), r.clone()))
            .collect();
        let sig_params = self.sig_params_snapshot();
        let mut ret_types: HashMap<String, Type> = self
            .get_all_func_signatures()
            .iter()
            .map(|(name, (_, ret, _))| {
                // Normalize `vec`/`list` to the ARRAY type at the single place
                // where signatures are handed to MIR: several annotation parsers
                // turn a library's `-> lt(vec, str)` into `Named("vec", [T])`,
                // which a caller cannot index as a list.
                let ret = match ret {
                    Type::Named(n, args) if n == "vec" || n == "list" => match args.first() {
                        Some(t) => Type::DynamicArray(Box::new(t.clone())),
                        None => Type::DynamicArray(Box::new(Type::I64)),
                    },
                    // `-> Path` / `-> Timestamp`: the library's PYTHON class
                    // spelling must become its registry handle tag, otherwise
                    // method dispatch on the result looks for `Path::open`
                    // (undefined) instead of the `W PyPath open` entry (t232).
                    Type::Named(n, args) => match crate::middle::pylib::handle_tag(n) {
                        Some(tag) => Type::Named(tag.to_string(), args.clone()),
                        // `-> pd.DataFrame` (`ParquetCache.load`) / `-> pd.Series`:
                        // the MODULE-QUALIFIED annotation name matched no shim
                        // struct, so the caller's `df.columns` / `df.data` became
                        // MAP lookups (measured: `load()` returned 892 rows /
                        // **0 columns**, and `df.itertuples` crashed). Strip the
                        // qualifier when the last segment names a shim class —
                        // RECURSIVELY, because it also appears NESTED
                        // (`-> tuple[pd.DataFrame, int]`, which
                        // `remove_extreme_return_bars` returns: the destructured
                        // frame was empty and `len(out)` crashed in `array_len`).
                        None => Self::shim_class_normalize(&Type::Named(n.clone(), args.clone())),
                    },
                    other => other.clone(),
                };
                // Batch 300: an UNANNOTATED `def` registers as UNIT, and that unit
                // propagated into the caller (`pf = make(…)` → `pf.<attr>` read
                // field 0 of an empty struct variant, `pf.positions.values()`
                // linked to the `_values` stub). Recover the type from the body's
                // own `return` statements when they agree.
                // Batch 451: `i64` joins the entry test because a py `class` method
                // arrives with the desugaring's `i64` default rather than a blank —
                // `unannotated_return_ty` still vets it (receiver typed with the
                // class name, and every `return` in the body must agree).
                let ret = if matches!(ret, Type::Tuple(ref inner) if inner.is_empty())
                    || matches!(ret, Type::I64)
                {
                    let recovered =
                        Self::unannotated_return_ty(&defs_snapshot, name, &class_names, &decl_rets, &sig_params);
                    recovered.unwrap_or(ret)
                } else {
                    ret
                };
                // `-> dict[...]` on a function whose body returns `json.loads(...)`:
                // the declared type says map while the VALUE is a PyJson cell, so
                // `stats.get(k, d)` compiled to the map primitive and HUNG —
                // `map_get_default` read the JSON tag as a capacity and spun
                // (measured with lldb in the local backtest). The runtime identity
                // wins: type it PyJson, whose `.get(k, default)` / `len()` /
                // `.items()` paths already exist.
                let is_dict_ret = ret.is_map();
                if is_dict_ret {
                    let hit = defs_snapshot
                        .iter()
                        .find(|d| {
                            matches!(d.as_ref(), AstNode::FuncDef { name: n, .. } if n == name)
                        })
                        .map(|d| d.as_ref());
                    if let Some(AstNode::FuncDef { body, .. }) = hit {
                        if Self::returns_json_loads(body) {
                            return (name.clone(), Type::Named("PyJson".to_string(), vec![]));
                        }
                        // Batch 299: recover the key/value types of a returned
                        // dict comprehension (see `dict_comprehension_ret`).
                        if let Some(t) = Self::dict_comprehension_ret(body) {
                            return (name.clone(), t);
                        }
                    }
                    // A METHOD is registered under its qualified key
                    // (`Ledg::build`) while the definitions carry the bare name
                    // (`build`), so the lookup above misses. Retry by tail —
                    // but only when exactly one definition has that method name,
                    // so two classes sharing `positions` cannot cross-contaminate
                    // each other's value type.
                    let tail = name.rsplit("::").next().unwrap_or(name.as_str());
                    let hits: Vec<&AstNode> = defs_snapshot
                        .iter()
                        .filter(|d| {
                            matches!(d.as_ref(), AstNode::FuncDef { name: n, .. }
                                if n.rsplit("::").next().unwrap_or("") == tail)
                        })
                        .map(|d| d.as_ref())
                        .collect();
                    if hits.len() == 1 {
                        if let Some(AstNode::FuncDef { body, .. }) = hits.first() {
                            if let Some(t) = Self::dict_comprehension_ret(body) {
                                return (name.clone(), t);
                            }
                        }
                    }
                }
                (name.clone(), ret)
            })
            .collect();
        // 批次 813（#268②）：调用槽的型与 LLVM 签名此前由两张表各自决定——被调
        // 方读自己 MIR 的 `signature_ret_ty()`（批次 399 定的唯一规则），调用方读
        // 下面这张声明表。未标注的 `def f(v): return v.mean()` 在声明表里是空格
        // （普通 def＝单元、class 方法＝i64 脱糖默认），而 `v.mean()` 的浮点型是
        // gen.rs 降 MIR 时才折出来的，AST 级回收（`unannotated_return_ty`）看不见
        // ⇒ 槽留 i64、被调方发 `double`，R7 把位模式当整数打（实拍
        // `print(f(xs))` 打 4624633867356078080，真值 15.0）。修法＝把已经降完的
        // 函数的 body 型回灌进这张表，且只灌声明表里本来就空着的槽：
        //   - 声明了 `-> i64` 却返回浮点的分歧不动（那是 docs/ABI.md §2 R7 自己的
        //     问题，批次 399 / 任务 #33 在册）；
        //   - 表里没有的名字不新增——gen.rs 有 20 多处 `func_ret_types.contains_key`
        //     把"在册"当"是用户函数"用，凭空加键会拨动那些臂。
        // 读数必须与降型顺序无关：lib 的降型循环按 `registered_funcs`（HashMap）取项，
        // 顺序逐次变——实拍同一份 min1.z（一个 `def m` ＋ `print(m(xs))`）连编 6 次，
        // `main` 有 3 次排在 `m` 之前，那次表还是空的，同一个源文件的读数就在 20.0 与
        // 4626322717216342016 之间随机翻（本批第一版按"降到的顺序"填表，就是这样翻的）。
        // 所以填表前先就地预热：把本张表里每个空槽对应的 def 立刻降一遍（MIR 丢弃，只留
        // 登记的 body 型），这样任何调用点看到的表都是齐的。递归降下去时同一规则继续适用
        // ⇒ def 调 def 的链也齐（环由 `priming` 挡住）。
        {
            let blanks: Vec<String> = ret_types
                .iter()
                .filter(|(_, ty)| {
                    matches!(ty, Type::I64)
                        || matches!(ty, Type::Tuple(inner) if inner.is_empty())
                })
                .map(|(name, _)| name.clone())
                .collect();
            let missing: Vec<String> = {
                let seen = self.body_ret_tys.borrow();
                let done = self.primed_names.borrow();
                blanks
                    .iter()
                    .filter(|n| {
                        !seen.contains_key(n.as_str()) && !done.contains(n.as_str())
                    })
                    .cloned()
                    .collect()
            };
            for name in &missing {
                self.prime_body_ret(name);
            }
            let body = self.body_ret_tys.borrow();
            for name in &blanks {
                if let Some(float @ (Type::F32 | Type::F64)) = body.get(name.as_str()) {
                    if let Some(ty) = ret_types.get_mut(name.as_str()) {
                        *ty = float.clone();
                    }
                }
            }
        }
        // `__name__` = the module this function belongs to (the root file is
        // `__main__`). `py_mangled_to_module` maps a definition's mangled name to
        // its module, so a function that is not there belongs to the root file.
        let fn_name_for_module = match ast {
            AstNode::FuncDef { name, .. } => name.as_str(),
            _ => "",
        };
        let current_module = self
            .py_mangled_to_module
            .borrow()
            .get(fn_name_for_module)
            .cloned()
            .unwrap_or_else(|| "__main__".to_string());
        let mut mir_gen = crate::middle::mir::r#gen::MirGen::new()
            .with_current_module(current_module)
            .with_global_consts(self.ctfe_consts.clone())
            .with_func_ret_types(ret_types)
            .with_func_param_names(self.func_param_names())
            .with_func_star_params(self.func_star_params())
            .with_repl_mode(self.repl_lowering)
            .with_type_decls(self.type_decls.clone())
            .with_nonlocal_names(self.nonlocal_names.borrow().clone())
            .with_module_globals(self.module_globals.borrow().clone())
            .with_py_imports(
                self.py_module_aliases.borrow().clone(),
                self.py_member_aliases.borrow().clone(),
            )
            .with_py_user_modules(self.py_user_modules.borrow().clone())
            .with_py_module_paths(self.py_module_paths.borrow().clone())
            .with_module_global_types(self.module_global_types_at("s5195"))
            .with_class_bases(self.class_bases())
            .with_none_vars(self.none_vars.borrow().clone())
            .with_method_param_refinements(self.method_param_refinements.clone())
            .with_source_file(self.source_file.borrow().clone())
            .with_argparse_kinds(self.argparse_kinds.borrow().clone())
            .with_param_defaults(
                self.param_defaults
                    .borrow()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
            .with_symbol_renames(self.module_renames_for(
                match ast {
                    AstNode::FuncDef { name, .. } => name.as_str(),
                    _ => "",
                },
            ));
        let mir = mir_gen.lower_to_mir(ast);
        // PY-A: synthetic lambda/closure functions synthesized while lowering
        // are parked on the resolver so they reach codegen exactly once.
        let generated = mir_gen.take_generated_mirs();
        if !generated.is_empty() {
            self.generated_closures
                .borrow_mut()
                .extend(generated.into_iter().filter_map(|m| {
                    m.name.clone().map(|n| (n, m))
                }));
        }
        // 批次 813（#268②）：登记本函数的 body 型，供后面的调用点填空调用槽。
        // 只登记 F32/F64——i64 就是空槽原本的读法，把它也写进来会让这张表变成
        // 第二个真相源。嵌套 def 走 `lower_closure`、不经过这里（调用点已按
        // `closure_ret_tys` 取 body 型）。
        if let AstNode::FuncDef { name, .. } = ast {
            if let Some(float @ (Type::F32 | Type::F64)) = mir.signature_ret_ty() {
                self.body_ret_tys.borrow_mut().insert(name.clone(), float);
            }
        }
        mir
    }

    /// 批次 813（#268②）：把 `name` 这个 def 就地降一遍，只为让 `lower_to_mir` 末尾
    /// 把它的 body 型登记进 `body_ret_tys`；产出的 MIR 丢弃——真正的定义仍由 lib 的
    /// 降型循环各发一遍。`primed_names` 记住"这个名看过了"（非浮点的 def 也记，否则每
    /// 个调用点都要重降一次），`priming` 挡住 `a` 调 `b`、`b` 又调 `a` 的环。
    fn prime_body_ret(&self, name: &str) {
        if self.primed_names.borrow().contains(name) || self.priming.borrow().contains(name) {
            return;
        }
        let mut target: Option<AstNode> = None;
        // lib 的降型循环取的是 `registered_funcs`，预热必须同源（cleanup 车道批次
        // 随合并入库）；`registered_func_defs` 留作回退源。
        if let Some(v) = self.registered_funcs.get(name) {
            if let AstNode::FuncDef { .. } = v {
                target = Some(v.clone());
            }
        }
        if target.is_none() {
            for d in self.registered_func_defs.borrow().iter() {
                if let AstNode::FuncDef { name: n, .. } = d.as_ref() {
                    if n == name {
                        target = Some(d.as_ref().clone());
                        break;
                    }
                }
            }
        }
        let Some(node) = target else {
            self.primed_names.borrow_mut().insert(name.to_string());
            return;
        };
        self.priming.borrow_mut().insert(name.to_string());
        let mir = self.lower_to_mir(&node);
        self.priming.borrow_mut().remove(name);
        drop(mir);
        self.primed_names.borrow_mut().insert(name.to_string());
    }

    /// PY-A V3: is this name declared `nonlocal` anywhere?
    pub fn is_nonlocal_name(&self, name: &str) -> bool {
        self.nonlocal_names.borrow().contains(name)
    }

    /// Take all synthetic closures accumulated by lower_to_mir so far.
    pub fn take_generated_closures(&self) -> Vec<Mir> {
        let mut v: Vec<Mir> = self.generated_closures.borrow().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        self.generated_closures.borrow_mut().clear();
        v
    }

    /// Convert AST node to ConstValue if it's a simple constant expression
    fn ast_to_const_value(&self, ast: &AstNode) -> Option<crate::middle::ctfe::value::ConstValue> {
        match ast {
            AstNode::Lit(n) => Some(crate::middle::ctfe::value::ConstValue::Int(*n)),
            AstNode::Bool(b) => Some(crate::middle::ctfe::value::ConstValue::Bool(*b)),
            AstNode::ArrayLit(elements) => {
                let mut const_elements = Vec::new();
                for elem in elements {
                    if let Some(const_elem) = self.ast_to_const_value(elem) {
                        const_elements.push(const_elem);
                    } else {
                        return None;
                    }
                }
                Some(crate::middle::ctfe::value::ConstValue::Array(
                    const_elements,
                ))
            }
            AstNode::ArrayRepeat { value, size } => {
                // Handle [value; size]
                if let AstNode::Lit(size_lit) = &**size {
                    let size_val = *size_lit as usize;
                    if let Some(const_val) = self.ast_to_const_value(value) {
                        // Repeat the value size_val times
                        let mut const_elements = Vec::new();
                        for _ in 0..size_val {
                            const_elements.push(const_val.clone());
                        }
                        return Some(crate::middle::ctfe::value::ConstValue::Array(
                            const_elements,
                        ));
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn collect_used_specializations(
        &mut self,
        asts: &[AstNode],
    ) -> HashMap<String, Vec<Vec<String>>> {
        let mut used = HashMap::new();
        fn walk(
            node: &AstNode,
            used: &mut HashMap<String, Vec<Vec<String>>>,
            resolver: &mut Resolver,
        ) {
            if let AstNode::Call {
                receiver,
                method,
                type_args,
                ..
            } = node
            {
                let mut args = type_args.clone();
                if let Some(r) = receiver {
                    let ty = resolver.infer_type(r);
                    if args.is_empty() {
                        // Convert Type to string for compatibility
                        args = vec![ty.display_name()];
                    }
                }
                if !args.is_empty() {
                    used.entry(method.clone()).or_default().push(args);
                }
            }
            if let AstNode::PathCall {
                path,
                method,
                type_args,
                ..
            } = node
            {
                if !type_args.is_empty() {
                    // Use fully-qualified name to disambiguate functions with the same
                    // method name but different modules (e.g., oneshot::channel vs mpsc::channel)
                    let qualified = if path.is_empty() {
                        method.clone()
                    } else {
                        format!("{}::{}", path.join("::"), method)
                    };
                    used.entry(qualified).or_default().push(type_args.clone());
                }
            }
            match node {
                AstNode::Call { method, receiver, .. } if method.starts_with("zeta_") || method == "__contains__" || method == "__fmtspec__" || method == "__slice__" => {
                    // PY-A: runtime-dispatched calls (zeta_* helpers and
                    // __dunder__ builtins) are handled by the codegen method
                    // dispatch — never monomorphize them as user functions.
                }
                AstNode::FuncDef { body, .. } => body.iter().for_each(|s| walk(s, used, resolver)),
                AstNode::Return(inner) => walk(inner, used, resolver),
                AstNode::ExprStmt { expr } => walk(expr, used, resolver),
                AstNode::BinaryOp { left, right, .. } => {
                    walk(left, used, resolver);
                    walk(right, used, resolver);
                }
                AstNode::If {
                    cond, then, else_, ..
                } => {
                    walk(cond, used, resolver);
                    then.iter().for_each(|s| walk(s, used, resolver));
                    else_.iter().for_each(|s| walk(s, used, resolver));
                }
                AstNode::Let { expr, .. } => walk(expr, used, resolver),
                AstNode::Assign(lhs, rhs) => {
                    walk(lhs, used, resolver);
                    walk(rhs, used, resolver);
                }
                AstNode::Call { receiver, args, .. } => {
                    if let Some(r) = receiver {
                        walk(r, used, resolver);
                    }
                    args.iter().for_each(|a| walk(a, used, resolver));
                }
                _ => {}
            }
        }
        for ast in asts {
            walk(ast, &mut used, self);
        }
        used
    }

    pub fn monomorphize(&self, key: MonoKey, ast: &AstNode) -> AstNode {
        let mut mono = ast.clone();
        let mut subst: HashMap<String, String> = key
            .type_args
            .iter()
            .cloned()
            .zip(key.type_args.iter().cloned())
            .collect();
        if !key.type_args.is_empty() {
            subst.insert("Self".to_string(), key.type_args[0].clone());
        }
        fn substitute(node: &mut AstNode, subst: &HashMap<String, String>) {
            match node {
                AstNode::FuncDef {
                    generics,
                    params,
                    ret,
                    body,
                    ..
                } => {
                    generics.clear();
                    for (_, ty) in params.iter_mut() {
                        if let Some(r) = subst.get(ty) {
                            *ty = r.clone();
                        }
                    }
                    if let Some(r) = subst.get(ret) {
                        *ret = r.clone();
                    }
                    for s in body {
                        substitute(s, subst);
                    }
                }
                AstNode::Call { type_args, .. } => type_args.clear(),
                AstNode::TimingOwned { ty, inner } => {
                    if let Some(r) = subst.get(ty) {
                        *ty = r.clone();
                    }
                    substitute(inner, subst);
                }
                AstNode::BinaryOp { left, right, .. } => {
                    substitute(left, subst);
                    substitute(right, subst);
                }
                AstNode::If {
                    cond, then, else_, ..
                } => {
                    substitute(cond, subst);
                    for s in then {
                        substitute(s, subst);
                    }
                    for s in else_ {
                        substitute(s, subst);
                    }
                }
                _ => {}
            }
        }
        substitute(&mut mono, &subst);
        mono
    }

    /// Expand macros in AST nodes
    pub fn expand_macros(&mut self, asts: &[AstNode]) -> Result<Vec<AstNode>, String> {
        let mut expanded = Vec::new();

        for ast in asts {
            let result = self.expand_macros_in_node(ast)?;
            expanded.extend(result);
        }

        Ok(expanded)
    }

    /// Expand macros in a single AST node
    fn expand_macros_in_node(&mut self, node: &AstNode) -> Result<Vec<AstNode>, String> {
        match node {
            AstNode::MacroCall { name, args } => self.expand_macro_site(name, args),
            AstNode::FuncDef {
                attrs,
                name,
                generics,
                lifetimes,
                params,
                ret,
                body,
                ret_expr,
                single_line,
                doc,
                pub_,
                async_,
                const_,
                comptime_,
                where_clauses,
            } => {
                // Recursively expand macros inside the function body
                let mut expanded_body = Vec::new();
                for stmt in body {
                    let expanded = self.expand_macros_in_node(stmt)?;
                    expanded_body.extend(expanded);
                }
                // Expand macros in ret_expr; statements from expansion go into body
                let mut final_ret_expr = None;
                if let Some(re) = ret_expr {
                    let mut expanded = self.expand_macros_in_node(re)?;
                    let mut had_expr_stmt = false;
                    for node in expanded.drain(..) {
                        match node {
                            AstNode::ExprStmt { .. } | AstNode::Let { .. } => {
                                // Statement goes into the body
                                expanded_body.push(node);
                                had_expr_stmt = true;
                            }
                            node => {
                                // Expression goes into ret_expr
                                final_ret_expr = Some(Box::new(node));
                            }
                        }
                    }
                    if !had_expr_stmt && final_ret_expr.is_none() {
                        final_ret_expr = None;
                    }
                }
                let expanded_func = AstNode::FuncDef {
                    attrs: attrs.clone(),
                    name: name.clone(),
                    generics: generics.clone(),
                    lifetimes: lifetimes.clone(),
                    params: params.clone(),
                    ret: ret.clone(),
                    body: expanded_body,
                    ret_expr: final_ret_expr,
                    single_line: *single_line,
                    doc: doc.clone(),
                    pub_: *pub_,
                    async_: *async_,
                    const_: *const_,
                    comptime_: *comptime_,
                    where_clauses: where_clauses.clone(),
                };
                let mut nodes = vec![expanded_func];
                let attr_expansions =
                    crate::frontend::macro_expand::process_attributes(attrs, node)?;
                nodes.extend(attr_expansions);
                Ok(nodes)
            }
            AstNode::StructDef { attrs, .. }
            | AstNode::EnumDef { attrs, .. }
            | AstNode::ConceptDef { attrs, .. } => {
                // Process attributes
                let mut nodes = vec![node.clone()];
                let attr_expansions =
                    crate::frontend::macro_expand::process_attributes(attrs, node)?;
                nodes.extend(attr_expansions);
                Ok(nodes)
            }
            // PY-A: an `impl` method body is a statement list like a function
            // body, but it was grouped with the declaration-only arms below and
            // only cloned. The body still runs (the methods are registered), so
            // every macro call inside it reached MIR lowering unexpanded and
            // produced nothing — no output, no diagnostic.
            AstNode::ImplBlock {
                concept,
                generics,
                lifetimes,
                ty,
                body,
                attrs,
                doc,
                where_clauses,
            } => {
                let expanded = AstNode::ImplBlock {
                    concept: concept.clone(),
                    generics: generics.clone(),
                    lifetimes: lifetimes.clone(),
                    ty: ty.clone(),
                    body: self.expand_stmts(body)?,
                    attrs: attrs.clone(),
                    doc: doc.clone(),
                    where_clauses: where_clauses.clone(),
                };
                let attr_expansions =
                    crate::frontend::macro_expand::process_attributes(attrs, &expanded)?;
                let mut nodes = vec![expanded];
                nodes.extend(attr_expansions);
                Ok(nodes)
            }
            AstNode::Program(nodes) => {
                // Recursively expand macros in program
                let mut expanded_nodes = Vec::new();
                for node in nodes {
                    let result = self.expand_macros_in_node(node)?;
                    expanded_nodes.extend(result);
                }
                Ok(vec![AstNode::Program(expanded_nodes)])
            }
            // PY-A: macro calls nested in a statement list were never visited.
            // The catch-all below returned them untouched, and MIR lowering
            // silently skips a MacroCall statement (`gen.rs`), so a
            // `for … { println!(…) }` / `if … { println!(…) }` body produced no
            // code at all — the loop itself ran, only its output vanished.
            AstNode::For { pattern, expr, body, else_body } => {
                Ok(vec![AstNode::For {
                    pattern: pattern.clone(),
                    expr: expr.clone(),
                    body: self.expand_stmts(body)?,
                    else_body: self.expand_stmts(else_body)?,
                }])
            }
            AstNode::While { cond, body, else_body } => {
                Ok(vec![AstNode::While {
                    cond: self.expand_expr_node(cond)?,
                    body: self.expand_stmts(body)?,
                    else_body: self.expand_stmts(else_body)?,
                }])
            }
            AstNode::If { cond, then, else_ } => Ok(vec![AstNode::If {
                cond: self.expand_expr_node(cond)?,
                then: self.expand_stmts(then)?,
                else_: self.expand_stmts(else_)?,
            }]),
            AstNode::Loop { body } => {
                Ok(vec![AstNode::Loop { body: self.expand_stmts(body)? }])
            }
            AstNode::Block { body } => {
                Ok(vec![AstNode::Block { body: self.expand_stmts(body)? }])
            }
            // PY-A: a macro call that arrives in statement position wrapped in
            // `ExprStmt` (that is how a block-shaped statement list stores it)
            // used to fall through to the catch-all below and be cloned
            // untouched, so the unexpanded MacroCall reached MIR lowering —
            // which skips it silently — and `2 => { println!("in-block") }`
            // printed nothing (batch 378, backlog #38 ④).
            AstNode::ExprStmt { expr } => {
                if let AstNode::MacroCall { name, args } = &**expr {
                    self.expand_macro_site(name, args)
                } else {
                    Ok(vec![node.clone()])
                }
            }
            // PY-A: `match` was missing from this recursion altogether, so every
            // arm body reached MIR lowering unexpanded — and MIR skips a
            // MacroCall silently, so `2 => println!("two")` printed nothing
            // (batch 378, backlog #38 ④).
            AstNode::Match { scrutinee, arms } => {
                let mut expanded_arms = Vec::with_capacity(arms.len());
                for arm in arms {
                    let body = self.expand_expr_node(&arm.body)?;
                    expanded_arms.push(crate::frontend::ast::MatchArm {
                        pattern: arm.pattern.clone(),
                        guard: arm.guard.clone(),
                        body,
                    });
                }
                Ok(vec![AstNode::Match {
                    scrutinee: scrutinee.clone(),
                    arms: expanded_arms,
                }])
            }
            // PY-A: a `match` sitting in expression position was never reached
            // either, because `let` and `=` are not in this recursion: the value
            // arrived but the arm's side effects did not —
            // `let x = match n { 2 => { println!("side"); 7 } _ => 0 }` printed
            // only `x=7` (batch 378, backlog #38 ④).
            AstNode::Let { mut_, pattern, ty, expr } => Ok(vec![AstNode::Let {
                mut_: *mut_,
                pattern: pattern.clone(),
                ty: ty.clone(),
                expr: self.expand_expr_node(expr)?,
            }]),
            AstNode::Assign(lhs, rhs) => {
                Ok(vec![AstNode::Assign(lhs.clone(), self.expand_expr_node(rhs)?)])
            }
            // PY-A: `return` and the operand shapes a formatted string is built
            // out of (`&format!(…)` inside a call, `format!(…) + format!(…)`) were
            // not in this recursion, so their MacroCall reached MIR lowering
            // unexpanded — the value came out 0/null with no diagnostic.
            AstNode::Return(expr) => Ok(vec![AstNode::Return(self.expand_expr_node(expr)?)]),
            AstNode::UnaryOp { op, expr } => Ok(vec![AstNode::UnaryOp {
                op: op.clone(),
                expr: self.expand_expr_node(expr)?,
            }]),
            AstNode::BinaryOp { op, left, right } => Ok(vec![AstNode::BinaryOp {
                op: op.clone(),
                left: self.expand_expr_node(left)?,
                right: self.expand_expr_node(right)?,
            }]),
            AstNode::Call { receiver, method, args, type_args, structural } => {
                Ok(vec![AstNode::Call {
                    receiver: match receiver {
                        Some(r) => Some(self.expand_expr_node(r)?),
                        None => None,
                    },
                    method: method.clone(),
                    args: self.expand_expr_list(args)?,
                    type_args: type_args.clone(),
                    structural: *structural,
                }])
            }
            _ => {
                // For other nodes, just return them as-is
                Ok(vec![node.clone()])
            }
        }
    }

    /// Expand a macro call site — arguments first, then the callee.
    ///
    /// The arguments used to reach `expand_macro_call` unexpanded, so a macro
    /// nested in an argument (`println!("{}", format!(…))`) was handed to the
    /// expander as a MacroCall and came out as a call to a function that does
    /// not exist — the value printed 0 while the outer template survived.
    fn expand_macro_site(&mut self, name: &str, args: &[AstNode]) -> Result<Vec<AstNode>, String> {
        let expanded = self.expand_expr_list(args)?;
        self.macro_expander.expand_macro_call(name, &expanded)
    }

    /// Expand each node of an expression list, keeping the list shape.
    fn expand_expr_list(&mut self, nodes: &[AstNode]) -> Result<Vec<AstNode>, String> {
        let mut expanded = Vec::with_capacity(nodes.len());
        for node in nodes {
            expanded.push(*self.expand_expr_node(node)?);
        }
        Ok(expanded)
    }

    /// Expand macros across a nested statement list (see the container arms above).
    fn expand_stmts(&mut self, stmts: &[AstNode]) -> Result<Vec<AstNode>, String> {
        let mut expanded = Vec::new();
        for stmt in stmts {
            expanded.extend(self.expand_macros_in_node(stmt)?);
        }
        Ok(expanded)
    }

    /// Expand a node that has to stay a single node (a `match` arm body, a `let`
    /// or `=` value). When the macro turns it into several statements they are
    /// wrapped in a block, whose value is its last statement.
    fn expand_expr_node(&mut self, node: &AstNode) -> Result<Box<AstNode>, String> {
        let nodes = self.expand_macros_in_node(node)?;
        Ok(match nodes.len() {
            0 => Box::new(node.clone()),
            1 => match &nodes[0] {
                AstNode::ExprStmt { expr } => expr.clone(),
                other => Box::new(other.clone()),
            },
            _ => Box::new(AstNode::Block { body: nodes }),
        })
    }

    /// Get all registered function ASTs
    pub fn get_registered_funcs(&self) -> Vec<AstNode> {
        for name in self.registered_funcs.keys() {}
        self.registered_funcs.values().cloned().collect()
    }

    /// Register built-in runtime functions that are required for compilation
    fn register_builtin_functions(&mut self) {
        // malloc(size: i64) -> i64 (allocate memory)
        self.register(AstNode::ExternFunc {
            name: "malloc".to_string(),
            generics: vec![],
            lifetimes: vec![],
            params: vec![("size".to_string(), "i64".to_string())],
            ret: "i64".to_string(),
            where_clauses: vec![],
        });

        // free(ptr: i64) -> () (free memory)
        self.register(AstNode::ExternFunc {
            name: "free".to_string(),
            generics: vec![],
            lifetimes: vec![],
            params: vec![("ptr".to_string(), "i64".to_string())],
            ret: "()".to_string(),
            where_clauses: vec![],
        });

        // clone_i64(value: i64) -> i64
        self.funcs.insert(
            "clone_i64".to_string(),
            (
                vec![("value".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );

        // is_null_i64(value: i64) -> bool
        self.funcs.insert(
            "is_null_i64".to_string(),
            (
                vec![("value".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );

        // to_string_str(value: str) -> str
        self.funcs.insert(
            "to_string_str".to_string(),
            (
                vec![("value".to_string(), Type::Str)],
                Type::Str,
                false, // not async
            ),
        );

        // to_string_i64(value: i64) -> str
        self.funcs.insert(
            "to_string_i64".to_string(),
            (
                vec![("value".to_string(), Type::I64)],
                Type::Str,
                false, // not async
            ),
        );

        // to_string_bool(value: bool) -> str
        self.funcs.insert(
            "to_string_bool".to_string(),
            (
                vec![("value".to_string(), Type::Bool)],
                Type::Str,
                false, // not async
            ),
        );

        // String runtime functions
        // str_concat(a: str, b: str) -> str
        self.funcs.insert(
            "str_concat".to_string(),
            (
                vec![("a".to_string(), Type::Str), ("b".to_string(), Type::Str)],
                Type::Str,
                false, // not async
            ),
        );

        // str_len(s: str) -> i64
        self.funcs.insert(
            "str_len".to_string(),
            (
                vec![("s".to_string(), Type::Str)],
                Type::I64,
                false, // not async
            ),
        );

        // Identity-aware string functions
        // read_only_string(value: str) -> identity(value)[read]
        self.funcs.insert(
            "read_only_string".to_string(),
            (
                vec![("value".to_string(), Type::Str)],
                Type::Identity(Box::new(IdentityType {
                    value: None,
                    capabilities: vec![CapabilityLevel::Read],
                    delegatable: false,
                    constraints: vec![],
                    type_params: vec![],
                })),
                false, // not async
            ),
        );

        // Basic I/O functions
        // print(value: i64) -> ()
        self.funcs.insert(
            "print".to_string(),
            (
                vec![("value".to_string(), Type::I64)],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );

        // println(value: i64) -> ()
        self.funcs.insert(
            "println".to_string(),
            (
                vec![("value".to_string(), Type::I64)],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );

        // read_write_string(value: str) -> identity(value)[read, write]
        self.funcs.insert(
            "read_write_string".to_string(),
            (
                vec![("value".to_string(), Type::Str)],
                Type::Identity(Box::new(IdentityType {
                    value: None,
                    capabilities: vec![CapabilityLevel::Read, CapabilityLevel::Write],
                    delegatable: false,
                    constraints: vec![],
                    type_params: vec![],
                })),
                false, // not async
            ),
        );

        // owned_string(value: str) -> identity(value)[read, write, owned]
        self.funcs.insert(
            "owned_string".to_string(),
            (
                vec![("value".to_string(), Type::Str)],
                Type::Identity(Box::new(IdentityType {
                    value: None,
                    capabilities: vec![
                        CapabilityLevel::Read,
                        CapabilityLevel::Write,
                        CapabilityLevel::Owned,
                    ],
                    delegatable: false,
                    constraints: vec![],
                    type_params: vec![],
                })),
                false, // not async
            ),
        );

        // str_to_lowercase(s: str) -> str
        self.funcs.insert(
            "str_to_lowercase".to_string(),
            (
                vec![("s".to_string(), Type::Str)],
                Type::Str,
                false, // not async
            ),
        );

        // str_to_uppercase(s: str) -> str
        self.funcs.insert(
            "str_to_uppercase".to_string(),
            (
                vec![("s".to_string(), Type::Str)],
                Type::Str,
                false, // not async
            ),
        );

        // str_trim(s: str) -> str
        self.funcs.insert(
            "str_trim".to_string(),
            (
                vec![("s".to_string(), Type::Str)],
                Type::Str,
                false, // not async
            ),
        );

        // str_starts_with(haystack: str, needle: str) -> bool
        self.funcs.insert(
            "str_starts_with".to_string(),
            (
                vec![
                    ("haystack".to_string(), Type::Str),
                    ("needle".to_string(), Type::Str),
                ],
                Type::Bool,
                false, // not async
            ),
        );

        // str_ends_with(haystack: str, needle: str) -> bool
        self.funcs.insert(
            "str_ends_with".to_string(),
            (
                vec![
                    ("haystack".to_string(), Type::Str),
                    ("needle".to_string(), Type::Str),
                ],
                Type::Bool,
                false, // not async
            ),
        );

        // str_contains(haystack: str, needle: str) -> bool
        self.funcs.insert(
            "str_contains".to_string(),
            (
                vec![
                    ("haystack".to_string(), Type::Str),
                    ("needle".to_string(), Type::Str),
                ],
                Type::Bool,
                false, // not async
            ),
        );

        // str_replace(s: str, old: str, new: str) -> str
        self.funcs.insert(
            "str_replace".to_string(),
            (
                vec![
                    ("s".to_string(), Type::Str),
                    ("old".to_string(), Type::Str),
                    ("new".to_string(), Type::Str),
                ],
                Type::Str,
                false, // not async
            ),
        );

        // str_split(s: str, delim: str) -> str
        self.funcs.insert(
            "str_split".to_string(),
            (
                vec![
                    ("s".to_string(), Type::Str),
                    ("delim".to_string(), Type::Str),
                ],
                Type::Str,
                false,
            ),
        );
        // str_join(parts: str, sep: str) -> str
        self.funcs.insert(
            "str_join".to_string(),
            (
                vec![
                    ("parts".to_string(), Type::Str),
                    ("sep".to_string(), Type::Str),
                ],
                Type::Str,
                false,
            ),
        );
        // str_find(haystack: str, needle: str) -> i64
        self.funcs.insert(
            "str_find".to_string(),
            (
                vec![
                    ("haystack".to_string(), Type::Str),
                    ("needle".to_string(), Type::Str),
                ],
                Type::I64,
                false,
            ),
        );
        // str_count(haystack: str, needle: str) -> i64
        self.funcs.insert(
            "str_count".to_string(),
            (
                vec![
                    ("haystack".to_string(), Type::Str),
                    ("needle".to_string(), Type::Str),
                ],
                Type::I64,
                false,
            ),
        );
        // str_strip(s: str) -> str
        self.funcs.insert(
            "str_strip".to_string(),
            (vec![("s".to_string(), Type::Str)], Type::Str, false),
        );
        // str_lstrip(s: str) -> str
        self.funcs.insert(
            "str_lstrip".to_string(),
            (vec![("s".to_string(), Type::Str)], Type::Str, false),
        );
        // str_rstrip(s: str) -> str
        self.funcs.insert(
            "str_rstrip".to_string(),
            (vec![("s".to_string(), Type::Str)], Type::Str, false),
        );
        // str_isalpha(s: str) -> i64
        self.funcs.insert(
            "str_isalpha".to_string(),
            (vec![("s".to_string(), Type::Str)], Type::I64, false),
        );
        // str_isnumeric(s: str) -> i64
        self.funcs.insert(
            "str_isnumeric".to_string(),
            (vec![("s".to_string(), Type::Str)], Type::I64, false),
        );

        // Comparison operators for i64
        // eq_i64(a: i64, b: i64) -> bool
        self.funcs.insert(
            "eq_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // ne_i64(a: i64, b: i64) -> bool
        self.funcs.insert(
            "ne_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // lt_i64(a: i64, b: i64) -> bool
        self.funcs.insert(
            "lt_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // gt_i64(a: i64, b: i64) -> bool
        self.funcs.insert(
            "gt_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // le_i64(a: i64, b: i64) -> bool
        self.funcs.insert(
            "le_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // ge_i64(a: i64, b: i64) -> bool
        self.funcs.insert(
            "ge_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // Also register operator symbols for completeness
        // ==(a: i64, b: i64) -> bool
        self.funcs.insert(
            "==".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // !=(a: i64, b: i64) -> bool
        self.funcs.insert(
            "!=".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // <(a: i64, b: i64) -> bool
        self.funcs.insert(
            "<".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // >(a: i64, b: i64) -> bool
        self.funcs.insert(
            ">".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // <=(a: i64, b: i64) -> bool
        self.funcs.insert(
            "<=".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );
        // >=(a: i64, b: i64) -> bool
        self.funcs.insert(
            ">=".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::Bool,
                false, // not async
            ),
        );

        // Arithmetic operators for i64
        // +(a: i64, b: i64) -> i64
        self.funcs.insert(
            "+".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // -(a: i64, b: i64) -> i64
        self.funcs.insert(
            "-".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // *(a: i64, b: i64) -> i64
        self.funcs.insert(
            "*".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // /(a: i64, b: i64) -> i64
        self.funcs.insert(
            "/".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // %(a: i64, b: i64) -> i64
        self.funcs.insert(
            "%".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // <<(a: i64, b: i64) -> i64
        self.funcs.insert(
            "<<".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // >>(a: i64, b: i64) -> i64
        self.funcs.insert(
            ">>".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // &(a: i64, b: i64) -> i64
        self.funcs.insert(
            "&".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // |(a: i64, b: i64) -> i64
        self.funcs.insert(
            "|".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // ^(a: i64, b: i64) -> i64
        self.funcs.insert(
            "^".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );

        // Arithmetic operator functions for i64
        // add_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "add_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // sub_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "sub_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // mul_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "mul_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // div_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "div_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // mod_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "mod_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // shl_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "shl_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // shr_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "shr_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // and_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "and_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // or_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "or_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );
        // xor_i64(a: i64, b: i64) -> i64
        self.funcs.insert(
            "xor_i64".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );

        // Array runtime functions
        // array_new(capacity: usize) -> i64 (pointer to array)
        self.funcs.insert(
            "array_new".to_string(),
            (
                vec![("capacity".to_string(), Type::Usize)],
                Type::I64,
                false, // not async
            ),
        );

        // array_push(arr: i64, value: i64) -> void
        self.funcs.insert(
            "array_push".to_string(),
            (
                vec![
                    ("arr".to_string(), Type::I64),
                    ("value".to_string(), Type::I64),
                ],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );

        // array_len(arr: i64) -> i64
        self.funcs.insert(
            "array_len".to_string(),
            (
                vec![("arr".to_string(), Type::I64)],
                Type::I64,
                false, // not async
            ),
        );

        // array_get(arr: i64, index: i64) -> i64
        self.funcs.insert(
            "array_get".to_string(),
            (
                vec![
                    ("arr".to_string(), Type::I64),
                    ("index".to_string(), Type::I64),
                ],
                Type::I64,
                false, // not async
            ),
        );

        // array_set(arr: i64, index: i64, value: i64) -> void
        self.funcs.insert(
            "array_set".to_string(),
            (
                vec![
                    ("arr".to_string(), Type::I64),
                    ("index".to_string(), Type::I64),
                    ("value".to_string(), Type::I64),
                ],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );

        // array_free(arr: i64) -> void
        self.funcs.insert(
            "array_free".to_string(),
            (
                vec![("arr".to_string(), Type::I64)],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );

        // Memory allocation functions
        // runtime_malloc(size: usize) -> i64 (pointer to allocated memory)
        self.funcs.insert(
            "runtime_malloc".to_string(),
            (
                vec![("size".to_string(), Type::Usize)],
                Type::I64,
                false, // not async
            ),
        );

        // map_get(map: i64, key: i64) -> i64
        self.funcs.insert(
            "map_get".to_string(),
            (
                vec![
                    ("map".to_string(), Type::I64),
                    ("key".to_string(), Type::I64),
                ],
                Type::I64,
                false, // not async
            ),
        );

        // print_i64(value: i64) -> void
        self.funcs.insert(
            "print_i64".to_string(),
            (
                vec![("value".to_string(), Type::I64)],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );

        // println() -> void
        self.funcs.insert(
            "println".to_string(),
            (
                vec![],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );

        // Vector constructor for Vector<u64, 8> (most common for Murphy's Sieve)
        // Note: This is a hack - we should handle generic vector types properly
        self.funcs.insert(
            "Vector::new".to_string(),
            (
                vec![
                    ("a0".to_string(), Type::U64),
                    ("a1".to_string(), Type::U64),
                    ("a2".to_string(), Type::U64),
                    ("a3".to_string(), Type::U64),
                    ("a4".to_string(), Type::U64),
                    ("a5".to_string(), Type::U64),
                    ("a6".to_string(), Type::U64),
                    ("a7".to_string(), Type::U64),
                ],
                Type::Vector(Box::new(Type::U64), ArraySize::Literal(8)),
                false, // not async
            ),
        );
        // Vector splat for Vector<u64, 8>
        self.funcs.insert(
            "Vector::splat".to_string(),
            (
                vec![("value".to_string(), Type::U64)],
                Type::Vector(Box::new(Type::U64), ArraySize::Literal(8)),
                false, // not async
            ),
        );

        // SIMD runtime functions for Vector<u64, 8>
        self.funcs.insert(
            "vector_make_u64x8".to_string(),
            (
                vec![
                    ("a0".to_string(), Type::I64),
                    ("a1".to_string(), Type::I64),
                    ("a2".to_string(), Type::I64),
                    ("a3".to_string(), Type::I64),
                    ("a4".to_string(), Type::I64),
                    ("a5".to_string(), Type::I64),
                    ("a6".to_string(), Type::I64),
                    ("a7".to_string(), Type::I64),
                ],
                Type::I64, // Returns pointer to vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_splat_u64x8".to_string(),
            (
                vec![("value".to_string(), Type::I64)],
                Type::I64, // Returns pointer to vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_add_u64x8".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64, // Returns pointer to new vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_sub_u64x8".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64, // Returns pointer to new vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_mul_u64x8".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64, // Returns pointer to new vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_get_u64x8".to_string(),
            (
                vec![
                    ("ptr".to_string(), Type::I64),
                    ("index".to_string(), Type::I64),
                ],
                Type::I64, // Returns element value
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_set_u64x8".to_string(),
            (
                vec![
                    ("ptr".to_string(), Type::I64),
                    ("index".to_string(), Type::I64),
                    ("value".to_string(), Type::I64),
                ],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );
        self.funcs.insert(
            "vector_free_u64x8".to_string(),
            (
                vec![("ptr".to_string(), Type::I64)],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );

        // SIMD runtime functions for Vector<i32, 4>
        self.funcs.insert(
            "vector_make_i32x4".to_string(),
            (
                vec![
                    ("a0".to_string(), Type::I64),
                    ("a1".to_string(), Type::I64),
                    ("a2".to_string(), Type::I64),
                    ("a3".to_string(), Type::I64),
                ],
                Type::I64, // Returns pointer to vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_splat_i32x4".to_string(),
            (
                vec![("value".to_string(), Type::I64)],
                Type::I64, // Returns pointer to vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_add_i32x4".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64, // Returns pointer to new vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_sub_i32x4".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64, // Returns pointer to new vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_mul_i32x4".to_string(),
            (
                vec![("a".to_string(), Type::I64), ("b".to_string(), Type::I64)],
                Type::I64, // Returns pointer to new vector
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_get_i32x4".to_string(),
            (
                vec![
                    ("ptr".to_string(), Type::I64),
                    ("index".to_string(), Type::I64),
                ],
                Type::I64, // Returns element value
                false,     // not async
            ),
        );
        self.funcs.insert(
            "vector_set_i32x4".to_string(),
            (
                vec![
                    ("ptr".to_string(), Type::I64),
                    ("index".to_string(), Type::I64),
                    ("value".to_string(), Type::I64),
                ],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );
        self.funcs.insert(
            "vector_free_i32x4".to_string(),
            (
                vec![("ptr".to_string(), Type::I64)],
                Type::Tuple(vec![]), // void
                false,               // not async
            ),
        );

        // Disabled for performance
        // ;
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Resolver {
    fn drop(&mut self) {
        self.persist_specialization_cache();
    }
}

/// `Map<K, V>` → `Map`: the class name an `impl` header declares keeps its
/// generic spelling, while every call site writes the bare name — so a
/// `Type::method` key built from the header never matches. `Map::new()` read
/// slot 0, and a non-`new` method on a generic class fell to the
/// capitalized-name catch-all and became a `zeta_platform_obj` handle.
fn impl_key_base(ty: &str) -> String {
    match ty.find('<') {
        Some(i) => ty[..i].trim_end().to_string(),
        None => ty.to_string(),
    }
}

/// PY-A: top-level definition name of an AST node, if it defines one.
fn definition_name(a: &AstNode) -> Option<&str> {
    match a {
        AstNode::FuncDef { name, .. }
        | AstNode::StructDef { name, .. }
        | AstNode::EnumDef { name, .. }
        | AstNode::ConstDef { name, .. }
        | AstNode::TypeAlias { name, .. } => Some(name),
        _ => None,
    }
}

/// PY-A: shallow-rename a definition so imported modules cannot collide with
/// the importing program (or with each other).
fn rename_definition(a: AstNode, prefix: &str) -> AstNode {
    match a {
        AstNode::FuncDef {
            name,
            generics,
            lifetimes,
            params,
            ret,
            body,
            attrs,
            ret_expr,
            single_line,
            doc,
            pub_,
            async_,
            const_,
            comptime_,
            where_clauses,
        } => {
            // Batch 651: when a constructor is mangled (e.g., `Cfg` →
            // `m647a__Cfg`), its return expression's StructLit variant must
            // also be mangled so the returned value carries the mangled type
            // name. Without this, cross-module same-named classes both return
            // `Struct { variant: "Cfg" }`, and method dispatch on the receiver
            // type resolves to the bare `Cfg::show` (which collides) instead
            // of the mangled `m647a__Cfg::show`.
            let ret_expr = ret_expr.map(|re| {
                if let AstNode::StructLit { variant, fields } = *re {
                    Box::new(AstNode::StructLit {
                        variant: format!("{}{}", prefix, variant),
                        fields,
                    })
                } else {
                    re
                }
            });
            // Batch 651: the parser-synthesized constructor puts the
            // StructLit in the body as a Return statement (ret_expr is
            // None), so the ret_expr mangling above does not catch it.
            // Walk the body and mangle any StructLit variants inside
            // Return nodes so the constructor returns the mangled type.
            let body = body
                .into_iter()
                .map(|stmt| match stmt {
                    AstNode::Return(inner) => {
                        if let AstNode::StructLit { variant, fields } =
                            *inner
                        {
                            AstNode::Return(Box::new(AstNode::StructLit {
                                variant: format!("{}{}", prefix, variant),
                                fields,
                            }))
                        } else {
                            AstNode::Return(inner)
                        }
                    },
                    other => other,
                })
                .collect();
            // Batch 651: for constructors, the `ret` field is the class
            // name (same as the function name). Mangle it so the return
            // type carries the mangled class name, which propagates to
            // the receiver type at method call sites.
            let ret = if ret == name {
                format!("{}{}", prefix, ret)
            } else {
                ret
            };
            AstNode::FuncDef {
                name: format!("{}{}", prefix, name),
                generics,
                lifetimes,
                params,
                ret,
                body,
                attrs,
                ret_expr,
                single_line,
                doc,
                pub_,
                async_,
                const_,
                comptime_,
                where_clauses,
            }
        },
        AstNode::StructDef {
            name,
            fields,
            generics,
            lifetimes,
            attrs,
            doc,
            pub_,
            where_clauses,
            ..
        } => AstNode::StructDef {
            name: format!("{}{}", prefix, name),
            fields,
            generics,
            lifetimes,
            attrs,
            doc,
            pub_,
            where_clauses,
        },
        AstNode::EnumDef {
            name,
            variants,
            generics,
            lifetimes,
            attrs,
            doc,
            pub_,
            where_clauses,
        } => AstNode::EnumDef {
            name: format!("{}{}", prefix, name),
            variants,
            generics,
            lifetimes,
            attrs,
            doc,
            pub_,
            where_clauses,
        },
        AstNode::ConstDef {
            name,
            ty,
            value,
            attrs,
            pub_,
            comptime_,
        } => AstNode::ConstDef {
            name: format!("{}{}", prefix, name),
            ty,
            value,
            attrs,
            pub_,
            comptime_,
        },
        other => other,
    }
}

/// A binding target that names exactly one slot: `x`, or `x: T` — which parses
/// to `TypeAnnotatedPattern { Var("x") }` and is otherwise invisible to every
/// walker that matches a bare `Var` on an assignment target. Two module-level
/// walks need it (batch 413, both measured):
/// - `module_level_bindings`: a module-level `UA: dict[str, str] = {…}` dropped
///   out of `own`, its mangled name never reached `module_globals`, and the
///   module's own functions lowered `UA` as an uninitialized slot — `DictInsert`
///   with a fresh empty `map_id` → SIGSEGV in `map_insert`.
/// - `module_global_types`: the same name missing there leaves the global
///   untyped for readers in other functions — `U.keys()` emitted the bare symbol
///   `_keys` instead of `map_keys` (link failure).
fn bound_var(target: &AstNode) -> Option<String> {
    match target {
        AstNode::Var(n) => Some(n.clone()),
        AstNode::TypeAnnotatedPattern { pattern, .. } => match &**pattern {
            AstNode::Var(n) => Some(n.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// PY-A: bare names a module-level statement binds (`LIMIT = 5`, `let x = …`).
fn module_level_bindings(stmt: &AstNode) -> Vec<String> {
    let mut out = Vec::new();
    match stmt {
        AstNode::Assign(lhs, _) => {
            if let Some(n) = bound_var(lhs) {
                out.push(n);
            }
        }
        AstNode::Let { pattern, .. } => {
            if let Some(n) = bound_var(pattern) {
                out.push(n);
            }
        }
        AstNode::Block { body } => {
            for s in body {
                out.extend(module_level_bindings(s));
            }
        }
        _ => {}
    }
    out
}

/// PY-A: `_Alias = RealName` where RealName is a def/class in the same module.
fn collect_same_module_aliases(
    stmt: &AstNode,
    module: &str,
    own: &std::collections::HashSet<String>,
    reexports: &mut std::collections::HashMap<String, std::collections::HashMap<String, (String, String)>>,
) {
    match stmt {
        AstNode::Assign(lhs, rhs) => {
            if let (AstNode::Var(alias), AstNode::Var(target)) = (&**lhs, &**rhs) {
                if own.contains(target) && alias != target {
                    reexports
                        .entry(module.to_string())
                        .or_default()
                        .insert(alias.clone(), (module.to_string(), target.clone()));
                }
            }
        }
        AstNode::Block { body } => {
            for s in body {
                collect_same_module_aliases(s, module, own, reexports);
            }
        }
        AstNode::If { then, else_, .. } => {
            for s in then {
                collect_same_module_aliases(s, module, own, reexports);
            }
            for s in else_ {
                collect_same_module_aliases(s, module, own, reexports);
            }
        }
        _ => {}
    }
}

/// PY-A: `local = mod_alias.member` — collect (local, mod_alias, member).
fn walk_module_member_assigns(n: &AstNode, out: &mut Vec<(String, String, String)>) {
    match n {
        AstNode::Assign(lhs, rhs) => {
            if let AstNode::Var(local) = &**lhs {
                if let AstNode::FieldAccess { base, field } = &**rhs {
                    if let AstNode::Var(mod_alias) = &**base {
                        out.push((local.clone(), mod_alias.clone(), field.clone()));
                    }
                }
            }
        }
        AstNode::FuncDef { body, ret_expr, .. } => {
            for s in body {
                walk_module_member_assigns(s, out);
            }
            if let Some(e) = ret_expr {
                walk_module_member_assigns(e, out);
            }
        }
        AstNode::Block { body } => {
            for s in body {
                walk_module_member_assigns(s, out);
            }
        }
        AstNode::If { then, else_, .. } => {
            for s in then {
                walk_module_member_assigns(s, out);
            }
            for s in else_ {
                walk_module_member_assigns(s, out);
            }
        }
        AstNode::ExprStmt { expr } => walk_module_member_assigns(expr, out),
        _ => {}
    }
}

/// PY-A: `Alias = ImportedName` — collect (alias, target).
fn walk_name_aliases(n: &AstNode, out: &mut Vec<(String, String)>) {
    match n {
        AstNode::Assign(lhs, rhs) => {
            if let (AstNode::Var(alias), AstNode::Var(target)) = (&**lhs, &**rhs) {
                if alias != target {
                    out.push((alias.clone(), target.clone()));
                }
            }
        }
        AstNode::FuncDef { body, ret_expr, .. } => {
            for s in body {
                walk_name_aliases(s, out);
            }
            if let Some(e) = ret_expr {
                walk_name_aliases(e, out);
            }
        }
        AstNode::Block { body } => {
            for s in body {
                walk_name_aliases(s, out);
            }
        }
        AstNode::If { then, else_, .. } => {
            for s in then {
                walk_name_aliases(s, out);
            }
            for s in else_ {
                walk_name_aliases(s, out);
            }
        }
        AstNode::ExprStmt { expr } => walk_name_aliases(expr, out),
        _ => {}
    }
}

/// PY-A: rewrite `zeta_module_decl("X")` → `zeta_module_decl("<prefix>X")`
/// throughout a module body so its globals land in the module's own slots.
fn prefix_module_decl_markers(n: AstNode, prefix: &str) -> AstNode {
    match n {
        AstNode::ExprStmt { expr } => AstNode::ExprStmt {
            expr: Box::new(prefix_module_decl_markers(*expr, prefix)),
        },
        AstNode::Block { body } => AstNode::Block {
            body: body
                .into_iter()
                .map(|s| prefix_module_decl_markers(s, prefix))
                .collect(),
        },
        AstNode::Call {
            receiver: None,
            method,
            mut args,
            type_args,
            structural,
        } if method == "zeta_module_decl" => {
            if let Some(AstNode::StringLit(name)) = args.first_mut() {
                *name = format!("{}{}", prefix, name);
            }
            AstNode::Call {
                receiver: None,
                method,
                args,
                type_args,
                structural,
            }
        }
        other => other,
    }
}

/// PY-A: result kind of a string method, for return-type inference. Mirrors
/// the compiler's own str-method table.
fn str_method_symbol_kind(method: &str) -> Option<&'static str> {
    match method {
        "upper" | "lower" | "capitalize" | "title" | "strip" | "trim" | "lstrip" | "rstrip"
        | "replace" | "join" | "zfill" | "ljust" | "rjust" => Some("str"),
        "isdigit" | "isalpha" | "isupper" | "islower" | "startswith" | "endswith"
        | "contains" | "starts_with" | "ends_with" => Some("bool"),
        _ => None,
    }
}
