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
    pub cached_mirs: HashMap<String, Mir>,
    pub mono_mirs: HashMap<MonoKey, Mir>,
    pub borrow_checker: RefCell<BorrowChecker>,
    pub associated_types: HashMap<(String, String), String>,
    pub ctfe_consts: HashMap<String, crate::middle::ctfe::value::ConstValue>,
    funcs: HashMap<String, FuncSignature>,
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
    /// PY-A: per-module top-level definition names (rename map source).
    py_module_own_names:
        RefCell<std::collections::HashMap<String, std::collections::HashSet<String>>>,
    /// PY-A: mangled module definition name → owning module.
    py_mangled_to_module: RefCell<std::collections::HashMap<String, String>>,
    /// PY-A: modules already loaded from disk (recursion / duplicate guard).
    py_loaded_modules: RefCell<std::collections::HashSet<String>>,
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
    registered_func_defs: RefCell<Vec<AstNode>>,
    /// Identity inference context for capability-based type inference
    identity_inference: crate::middle::types::identity::inference::IdentityInferenceContext,
    /// Capability inferencer for identity-aware type inference
    capability_inferencer: crate::middle::types::identity::inference::CapabilityInferencer,
}

// Learning: Complex type factored into type definition per clippy suggestion
type FuncSignature = (Vec<(String, Type)>, Type, bool);

impl Resolver {
    pub fn new() -> Self {
        let mut r = Self {
            impls: HashMap::new(),
            cached_mirs: HashMap::new(),
            mono_mirs: HashMap::new(),
            borrow_checker: RefCell::new(BorrowChecker::new()),
            associated_types: HashMap::new(),
            ctfe_consts: HashMap::new(),
            funcs: HashMap::new(),
            registered_funcs: HashMap::new(),
            module_resolver: ModuleResolver::new("."),
            macro_expander: MacroExpander::new(),
            identity_inference:
                crate::middle::types::identity::inference::IdentityInferenceContext::new(),
            type_decls: HashMap::new(),
            generated_closures: RefCell::new(HashMap::new()),
            nonlocal_names: RefCell::new(std::collections::HashSet::new()),
            module_globals: RefCell::new(std::collections::HashSet::new()),
            py_module_aliases: RefCell::new(std::collections::HashMap::new()),
            py_member_aliases: RefCell::new(std::collections::HashMap::new()),
            py_user_modules: RefCell::new(std::collections::HashSet::new()),
            py_module_own_names: RefCell::new(std::collections::HashMap::new()),
            py_mangled_to_module: RefCell::new(std::collections::HashMap::new()),
            py_loaded_modules: RefCell::new(std::collections::HashSet::new()),
            py_source_dir: RefCell::new(None),
            source_file: RefCell::new(None),
            argparse_kinds: RefCell::new(std::collections::HashMap::new()),
            param_defaults: RefCell::new(std::collections::HashMap::new()),
            py_module_pkg: RefCell::new(std::collections::HashMap::new()),
            py_current_module: RefCell::new(None),
            registered_func_defs: RefCell::new(Vec::new()),
            capability_inferencer:
                crate::middle::types::identity::inference::CapabilityInferencer::new(),
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
        // PY-A: keep the definition for return-type inference (imported
        // modules register through this same path).
        if matches!(ast, AstNode::FuncDef { .. }) {
            self.registered_func_defs.borrow_mut().push(ast.clone());
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
                    AstNode::FuncDef { body, .. } => {
                        for s in body {
                            walk_nonlocal(s, set);
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
            if std::env::var("ZETA_PROBE").is_ok() {
                eprintln!("PROBE nonlocal set: {:?}", self.nonlocal_names.borrow());
            }
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
                    }
                    AstNode::ExprStmt { expr } => walk_py_import(expr, out),
                    AstNode::Block { body } => {
                        for s in body {
                            walk_py_import(s, out);
                        }
                    }
                    AstNode::FuncDef { body, .. } => {
                        for s in body {
                            walk_py_import(s, out);
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
                        if names.is_empty() {
                            eprintln!(
                                "warning: PY-A: `from {} import *` bound no public names",
                                module
                            );
                        }
                        for n in names {
                            self.py_member_aliases
                                .borrow_mut()
                                .insert(n.clone(), (module.clone(), n));
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
                    // The built-in shim wins over a same-named file. Say so —
                    // silently ignoring the user's file is exactly the kind of
                    // surprise this compiler must not spring.
                    eprintln!(
                        "warning: PY-A: `{}` resolves to the built-in shim; the local file {} \
                         is ignored (rename it, or import it under a different name)",
                        module,
                        path.display()
                    );
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
                        self.py_member_aliases
                            .borrow_mut()
                            .insert(alias, (module.clone(), m.clone()));
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
                                    for b in body.clone() {
                                        if let AstNode::FuncDef {
                                            name,
                                            params,
                                            ret,
                                            async_,
                                            ..
                                        } = &b
                                        {
                                            let qualified_name = format!("{}::{}", ty, name);
                                            let typed_params: Vec<_> = params
                                                .iter()
                                                .map(|(n, t)| (n.clone(), self.string_to_type(t)))
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
                                            let qualified = format!("{}::{}", ty, fn_name);
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
                            "W2002",
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
                    .map(|(name, ty_str)| {
                        (name.clone(), self.string_to_generic_type(ty_str, &generic_names))
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
                    .map(|(name, ty_str)| (name.clone(), self.string_to_type(ty_str)))
                    .collect();
                let typed_ret = self.string_to_type(&ret);
                self.funcs.insert(name, (typed_params, typed_ret, true));
            }
            AstNode::ImplBlock {
                concept, ty, body, ..
            } => {
                self.impls.insert((concept, ty.clone()), body.clone());
                // Register functions with qualified names
                for b in body.clone() {
                    if let AstNode::FuncDef {
                        name, params, ret, ..
                    } = &b
                    {
                        // Create qualified name: Type::method
                        let qualified_name = format!("{}::{}", ty, name);
                        // Convert string types to Type enum
                        let typed_params: Vec<(String, Type)> = params
                            .iter()
                            .map(|(name, ty_str)| (name.clone(), self.string_to_type(ty_str)))
                            .collect();
                        let typed_ret = self.string_to_type(ret);
                        self.funcs
                            .insert(qualified_name, (typed_params, typed_ret, false));
                    }
                    // Register with qualified name (for MIR resolution)
                    // Clone the func and override its name so MIR matches the call site
                    if let AstNode::FuncDef { name: fn_name, .. } = &b {
                        let qualified = format!("{}::{}", ty, fn_name);
                        let mut qualified_ast = b.clone();
                        if let AstNode::FuncDef { ref mut name, .. } = qualified_ast {
                            *name = qualified.clone();
                        }
                        self.registered_funcs.insert(qualified, qualified_ast);
                    }
                    // Also register with simple name for backwards compat
                    self.register(b);
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
                    .map(|(name, ty_str)| (name.clone(), self.string_to_type(ty_str)))
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
                        crate::diag_warning!("W2003", "Failed to parse macro {}: {}", name, e);
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
        let asts: Vec<AstNode> = self.registered_func_defs.borrow().clone();
        if std::env::var("ZETA_PROBE").is_ok() {

        }
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
                _ => 0,
            }
        }
        // Each pass propagates evidence one call level deeper (main ->
        // snakecase -> lowercase -> ...), so give the chain room.
        for _ in 0..6 {
            for ast in &asts {
                let AstNode::FuncDef { name, body, ret, .. } = ast else {
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
                if infer_return {
                    collect_returns(body, &mut rets);
                }
                let mut saw_str = false;
                let mut saw_f64 = false;
                let mut saw_i64 = false;
                let aliases = self.py_module_aliases.borrow().clone();
                for r in &rets {
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
                let new_ret = if saw_str && !saw_i64 && !saw_f64 {
                    Some(Type::Str)
                } else if saw_f64 && !saw_i64 && !saw_str {
                    Some(Type::F64)
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
                    for (i, a) in pargs.iter().enumerate() {
                        if i >= param_types.len() || param_types[i] != Type::I64 {
                            continue;
                        }
                        let cur_params2: Vec<(String, Type)> = self
                            .funcs
                            .get(name)
                            .map(|(p, _, _)| p.clone())
                            .unwrap_or_default();
                        match classify(a, &self.funcs, prefix.as_deref(), &aliases2, &cur_params2) {
                            1 => changed.push((i, Type::Str)),
                            2 => changed.push((i, Type::F64)),
                            _ => {}
                        }
                    }
                    for (i, t) in &changed {
                        param_types[*i] = t.clone();
                    }
                    let changed_pairs = changed.clone();
                    let changed = !changed.is_empty();
                    if changed {
                        if std::env::var("ZETA_PROBE").is_ok() {
                            eprintln!("PROBE param infer: {} -> {:?}", callee, param_types);
                        }
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
        let globals = self.module_globals.borrow().clone();
        let aliases = self.py_module_aliases.borrow().clone();
        let mut out: HashMap<String, Type> = HashMap::new();
        fn walk(stmts: &[AstNode], globals: &std::collections::HashSet<String>,
                aliases: &HashMap<String, String>, out: &mut HashMap<String, Type>) {
            for s in stmts {
                let (name, rhs) = match s {
                    AstNode::Assign(lhs, rhs) => match &**lhs {
                        AstNode::Var(n) => (n.clone(), Some(&**rhs)),
                        _ => continue,
                    },
                    AstNode::Let { pattern, expr, .. } => match &**pattern {
                        AstNode::Var(n) => (n.clone(), Some(&**expr)),
                        _ => continue,
                    },
                    AstNode::Block { body } => {
                        walk(body, globals, aliases, out);
                        continue;
                    }
                    _ => continue,
                };
                if !globals.contains(&name) {
                    continue;
                }
                let ty = match rhs {
                    Some(AstNode::StringLit(_)) | Some(AstNode::FString { .. }) => Some(Type::Str),
                    Some(AstNode::FloatLit(_)) => Some(Type::F64),
                    Some(AstNode::Call { receiver, method, .. }) => {
                        // Resolve `X.Y(...)` / `Y(...)` through the registry to
                        // its declared result (handle tag or str).
                        let member = match receiver {
                            Some(recv) => {
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
                                        _ => break,
                                    }
                                }
                                parts.reverse();
                                // A receiver that is not a `name(.name)*` chain
                                // (e.g. a call result) yields nothing — never
                                // index an empty vector.
                                if parts.is_empty() {
                                    None
                                } else {
                                    let root = parts.remove(0);
                                    let rest = if parts.is_empty() {
                                        method.to_string()
                                    } else {
                                        format!("{}.{}", parts.join("."), method)
                                    };
                                    aliases.get(&root).map(|module| (module.clone(), rest))
                                }
                            }
                            None => None,
                        };
                        member.and_then(|(module, mem)| {
                            crate::middle::pylib::find_member(&module, &mem).map(|e| {
                                match (e.handle.as_deref(), e.ret.as_str()) {
                                    (Some(h), _) => Type::Named(h.to_string(), vec![]),
                                    (None, "str") => Type::Str,
                                    (None, "f64") => Type::F64,
                                    _ => Type::I64,
                                }
                            })
                        })
                    }
                    _ => None,
                };
                if let Some(t) = ty {
                    out.insert(name, t);
                }
            }
        }
        let defs = self.registered_func_defs.borrow().clone();
        for d in &defs {
            if let AstNode::FuncDef { body, .. } = d {
                walk(body, &globals, &aliases, &mut out);
            }
        }
        out
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
    ///   <dir of the file being compiled>/X.{py,z}, pylib/X.{py,z},
    ///   $ZETA_PYLIB/X.{py,z}, build/stubs/X.{py,z}
    /// Every top-level definition is prefixed `X__` so two modules (or a module
    /// and the main program) may both define `helper`. Returns false when no
    /// file is found.
    /// PY-A: locate a Python module file on disk: (path, is_python_source).
    /// Order: the directory of the file being compiled, `pylib`, `$ZETA_PYLIB`,
    /// then `build/stubs`.
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
        let rel: std::path::PathBuf = module.split('.').collect();
        let mut bases: Vec<std::path::PathBuf> = Vec::new();
        // The file being compiled wins, then explicitly configured paths, then
        // installed packages, then the bundled shim sources.
        if let Some(d) = self.py_source_dir.borrow().clone() {
            bases.push(d);
        }
        if let Ok(p) = std::env::var("ZETA_PYLIB") {
            bases.push(std::path::PathBuf::from(p));
        }
        bases.push(crate::middle::pylib::packages_dir());
        bases.push(std::path::PathBuf::from("pylib"));
        bases.push(std::path::PathBuf::from("build/stubs"));
        for base in &bases {
            // Single-file module: X.py / X.z
            for (ext, is_py) in [("py", true), ("z", false)] {
                let mut p = base.join(&rel);
                p.set_extension(ext);
                if p.is_file() {
                    return Some((p, is_py));
                }
            }
            // Directory package: X/__init__.py / X/__init__.z
            for (name, is_py) in [("__init__.py", true), ("__init__.z", false)] {
                let p = base.join(&rel).join(name);
                if p.is_file() {
                    return Some((p, is_py));
                }
            }
        }
        None
    }

    fn load_user_python_module(&mut self, module: &str) -> bool {
        if !self.py_loaded_modules.borrow_mut().insert(module.to_string()) {
            return true; // already loaded (or currently loading)
        }
        let (path, is_py) = match self.find_py_module_file(module) {
            Some(v) => v,
            None => {
                self.py_loaded_modules.borrow_mut().remove(module);
                return false;
            }
        };
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
        let asts = match crate::frontend::parser::top_level::parse_zeta(&pre) {
            Ok((_rem, a)) => a,
            Err(_) => {
                eprintln!("warning: PY-A: cannot parse module file {}", path.display());
                return false;
            }
        };
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
        for a in asts {
            match a {
                AstNode::FuncDef { ref name, ref body, .. } if name == "main" => {
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
        self.py_current_module.replace(saved_ctx);
        eprintln!("PY-A: imported module `{}` from {}", module, path.display());
        true
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
    fn module_renames_for(&self, func_name: &str) -> std::collections::HashMap<String, String> {
        let mut out = std::collections::HashMap::new();
        let module = match self.py_mangled_to_module.borrow().get(func_name) {
            Some(m) => m.clone(),
            None => return out,
        };
        if let Some(own) = self.py_module_own_names.borrow().get(&module) {
            let prefix = format!("{}__", module.replace('.', "_"));
            for n in own {
                out.insert(n.clone(), format!("{}{}", prefix, n));
            }
        }
        out
    }

    pub fn lower_to_mir(&self, ast: &AstNode) -> Mir {
        let ret_types: HashMap<String, Type> = self
            .get_all_func_signatures()
            .iter()
            .map(|(name, (_, ret, _))| (name.clone(), ret.clone()))
            .collect();
        let mut mir_gen = crate::middle::mir::r#gen::MirGen::new()
            .with_global_consts(self.ctfe_consts.clone())
            .with_func_ret_types(ret_types)
            .with_func_param_names(self.func_param_names())
            .with_type_decls(self.type_decls.clone())
            .with_nonlocal_names(self.nonlocal_names.borrow().clone())
            .with_module_globals(self.module_globals.borrow().clone())
            .with_py_imports(
                self.py_module_aliases.borrow().clone(),
                self.py_member_aliases.borrow().clone(),
            )
            .with_py_user_modules(self.py_user_modules.borrow().clone())
            .with_module_global_types(self.module_global_types())
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
        mir
    }

    /// PY-A V3: is this name declared `nonlocal` anywhere?
    pub fn is_nonlocal_name(&self, name: &str) -> bool {
        if std::env::var("ZETA_PROBE").is_ok() {
            eprintln!("PROBE is_nonlocal({}) = {}", name, self.nonlocal_names.borrow().contains(name));
        }
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
            AstNode::MacroCall { name, args } => {
                // Expand macro call
                self.macro_expander.expand_macro_call(name, args)
            }
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
            | AstNode::ConceptDef { attrs, .. }
            | AstNode::ImplBlock { attrs, .. } => {
                // Process attributes
                let mut nodes = vec![node.clone()];
                let attr_expansions =
                    crate::frontend::macro_expand::process_attributes(attrs, node)?;
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
            _ => {
                // For other nodes, just return them as-is
                Ok(vec![node.clone()])
            }
        }
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
        } => AstNode::FuncDef {
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

/// PY-A: bare names a module-level statement binds (`LIMIT = 5`, `let x = …`).
fn module_level_bindings(stmt: &AstNode) -> Vec<String> {
    let mut out = Vec::new();
    match stmt {
        AstNode::Assign(lhs, _) => {
            if let AstNode::Var(n) = &**lhs {
                out.push(n.clone());
            }
        }
        AstNode::Let { pattern, .. } => {
            if let AstNode::Var(n) = &**pattern {
                out.push(n.clone());
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
