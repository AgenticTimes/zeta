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
    /// NAME → element count for a `x = [ … ]` literal binding. `f(*x)` used to
    /// read the count from `Type::Array(_, Literal(n))`; now that non-float list
    /// literals are DynamicArrays that type is gone, so the count is remembered
    /// here (a runtime length cannot drive a compile-time unroll).
    array_lit_lens: HashMap<String, usize>,
    /// Tracks pointee element width (in bytes) for pointer-typed expression IDs.
    /// Populated by the offset/add handler, used when generating Store/Deref.
    pointee_widths: HashMap<u32, u8>,
    /// Batch 431: bare call targets this body produced by degrading a
    /// `receiver.member(...)` (see `note_member_bare`). Moved into
    /// `Mir::member_bare_calls` so a later layer can tell a degraded member from
    /// an ordinary call to a same-spelled function.
    member_bare_calls: std::collections::BTreeSet<String>,
    /// Batch 431: bare call targets this body reached as an ordinary call
    /// (`foo(...)` with no receiver). Moved into `Mir::plain_call_names`.
    plain_call_names: std::collections::BTreeSet<String>,
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
    /// Batch 288: slots holding the result of `PyJson.items()` — the pair's
    /// value half is a PyJson handle but the array element type is i64
    /// (untyped pair). The dict-comprehension hint consults this to type the
    /// lambda's value param, so `{k: int(v.get("ok", 0)) … for k, v in
    /// raw.items()}` (sources_selector.py:125) reaches `py_json_get_default`
    /// instead of the bare `get` ghost symbol (runtime abort).
    py_json_items_ids: std::collections::HashSet<u32>,
    /// PY-A: value type of the most recently lowered closure body, so a
    /// comprehension's result can carry a real element type instead of i64.
    last_closure_ret_ty: Option<Type>,
    /// Batch 299: key/value types of the most recently lowered
    /// `__pack_pair__` (a dict comprehension's lambda). `zeta_collect_dict`
    /// filled a map with NO type arguments, so `d.values()` fell back to i64
    /// elements and `portfolio.positions.values()` read struct fields off a
    /// pointer (`PositionLedger.positions` — the whole end-of-period
    /// valuation came out as garbage integers).
    last_dict_pair_ty: Option<(Type, Type)>,
    /// Stack of loop result slots for `loop { break EXPR; }` value semantics.
    loop_value_stack: Vec<u32>,
    /// Result slot of the most recently lowered loop (for implicit ret_val).
    last_loop_result: Option<u32>,
    /// PY-A (任务 #55): this is the entry `main` the parser synthesized from a
    /// module body — every exit path hands back 0, see `PY_ENTRY_ATTR`.
    py_entry: bool,
    /// Additional MIRs generated during lowering (e.g., async poll functions).
    generated_mirs: Vec<Mir>,
    /// Lowering depth: 0 at top level, >0 inside a function body — used to
    /// skip nested defs (their inline Return would corrupt the enclosing stream).
    fn_depth: u32,
    /// T0 (refactor.md B.5): stable namespace for synthetic closure symbols —
    /// the enclosing function's DECLARED name, and for a closure body its own
    /// generated symbol. Replaces the process-global `AtomicU32` counter, which
    /// made a closure's name depend on the ORDER the compiler happened to lower
    /// functions (HashMap iteration), so two compiles of one input disagreed on
    /// which body `__closure_0` was.
    closure_ns: String,
    /// Per-scope ordinal of the closures lowered so far, in source order.
    closure_seq: usize,
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
    /// PY-A: module name → the file it was loaded from. `__file__` is per
    /// module, so a module's own path has to be reachable from its functions.
    py_module_paths: std::collections::HashMap<String, String>,
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
    /// BATCH-438: `Class::method` → synthetic closure symbol, for the methods of
    /// a `class` written inside a function body. Collected per impl-block window
    /// and consumed by `rewrite_nested_class_calls` — see there for why the
    /// parent's `closure_vars` cannot carry this on its own.
    nested_class_aliases: Vec<(String, String)>,
    /// The module this function belongs to — the value of `__name__`
    /// (`logging.getLogger(__name__)` produced a NULL-ish name and `fprintf`
    /// crashed in `strlen`; a bare `print(__name__)` printed 1).
    current_module: String,
    /// Fields already computed inside a synthesized constructor. `__init__`'s
    /// `self` is dropped from the ctor signature, so a LATER field whose
    /// initializer reads `self.<earlier field>` had no `self` slot at all.
    self_field_aliases: Vec<(String, u32)>,
    /// Ids that hold a TUPLE value (a StackArray literal, possibly copied into a
    /// slot by an assignment). The static type is not enough: a dict
    /// comprehension's result can leak a `Tuple` annotation while the value is a
    /// map, and indexing that with `stack_array_get` read the map header.
    tuple_slots: std::collections::HashSet<u32>,
    /// Names captured from enclosing scopes in the closure currently being
    /// lowered (name → env key id) — used to route assignments to env stores.
    captured_vars: std::collections::HashMap<String, u32>,
    /// Known function return types (base name -> Type), injected by Resolver.
    func_ret_types: HashMap<String, Type>,
    /// Parameter names per function, for keyword-argument binding.
    func_param_names: HashMap<String, Vec<String>>,
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
            array_lit_lens: HashMap::new(),
            pointee_widths: HashMap::new(),
            type_decls: HashMap::new(),
            shared_type_decls: HashMap::new(),
            source_file: None,
            argparse_kinds: HashMap::new(),
            param_defaults: HashMap::new(),
            pending_closure_param_types: None,
            py_json_items_ids: std::collections::HashSet::new(),
            last_closure_ret_ty: None,
            last_dict_pair_ty: None,
            loop_value_stack: Vec::new(),
            last_loop_result: None,
            py_entry: false,
            generated_mirs: vec![],
            member_bare_calls: std::collections::BTreeSet::new(),
            plain_call_names: std::collections::BTreeSet::new(),
            fn_depth: 0,
            closure_ns: String::new(),
            closure_seq: 0,
            hoisted_names: std::collections::HashMap::new(),
            nonlocal_names: std::collections::HashSet::new(),
            module_globals: std::collections::HashSet::new(),
            py_module_aliases: HashMap::new(),
            py_member_aliases: HashMap::new(),
            py_user_modules: std::collections::HashSet::new(),
            py_module_paths: std::collections::HashMap::new(),
            symbol_renames: HashMap::new(),
            module_global_types: HashMap::new(),
            re_repl_param: false,
            current_class: None,
            nested_class_aliases: Vec::new(),
            current_module: "__main__".to_string(),
            self_field_aliases: Vec::new(),
            tuple_slots: std::collections::HashSet::new(),
            captured_vars: std::collections::HashMap::new(),
            func_ret_types: HashMap::new(),
            func_param_names: HashMap::new(),
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
        if crate::diagnostics::env_flag("ZETA_PROBE") {
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

    /// PY-A: module name → source file, the per-module value of `__file__`.
    pub fn with_py_module_paths(mut self, paths: HashMap<String, String>) -> Self {
        self.py_module_paths = paths;
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
    /// Static type of a module-level global. The resolver's table carries both
    /// the BARE name and the `<module>__<name>` mangled form, but the mangled
    /// form is only added for modules already registered when the table was
    /// built — in a multi-module compile that prefix list can be empty, so
    /// `import a; a.C.exists()` looked up `a__C`, missed, typed the value I64 and
    /// emitted a bare `_a__C.exists` (link error). `from a import C` worked
    /// because it looks up the bare name. Fall back to stripping each KNOWN
    /// module prefix (`_PROJECT_ROOT` keeps its leading underscore — splitting on
    /// `__` would yield `PROJECT_ROOT` and still miss).
    fn global_ty_of(&self, name: &str) -> Option<Type> {
        if let Some(t) = self.module_global_types.get(name) {
            return Some(t.clone());
        }
        let mut prefixes: Vec<String> = self
            .py_module_aliases
            .values()
            .map(|m| format!("{}__", m.replace('.', "_")))
            .collect();
        prefixes.sort();
        prefixes.dedup();
        for pfx in prefixes {
            if let Some(rest) = name.strip_prefix(pfx.as_str()) {
                if !rest.is_empty() {
                    if let Some(t) = self.module_global_types.get(rest) {
                        return Some(t.clone());
                    }
                }
            }
        }
        if let Some(idx) = name.rfind("__") {
            let rest = &name[idx + 2..];
            if !rest.is_empty() {
                if let Some(t) = self.module_global_types.get(rest) {
                    return Some(t.clone());
                }
            }
        }
        None
    }

    /// Store `value` into the env cell called `name` and return the key's id,
    /// which callers reuse for the matching `zeta_env_get`.
    ///
    /// The cell is one raw 64-bit word (`zeta_env_set(i64, i64)`), and a reader
    /// hands it back typed by the name's declared type (`global_ty_of`) — so a
    /// float cell is REINTERPRETED, not converted. A float going into such a
    /// cell therefore has to go in as its BIT PATTERN: the call-argument
    /// coercion used to `fptosi` it, which dropped the fraction (measured:
    /// module global `ratio = 2.5`, read in another function as `0.000000`).
    /// Names with no declared type keep the truncating store on purpose — their
    /// readers type the cell `I64`, and bits would come out as a huge integer
    /// instead of today's truncated one (closure `nonlocal` floats).
    fn env_store(&mut self, name: &str, value: u32) -> u32 {
        let (stmts, key_id) = self.env_mirror(name, value);
        self.stmts.extend(stmts);
        key_id
    }

    /// BUILD (do not emit) the statements of `env_store`, plus the key id they
    /// use. Split out so `splice_env_mirrors` can insert a mirror into the
    /// middle of an already-lowered statement list.
    fn env_mirror(&mut self, name: &str, value: u32) -> (Vec<MirStmt>, u32) {
        let mut out: Vec<MirStmt> = Vec::new();
        let key_id = self.next_id();
        self.exprs
            .insert(key_id, MirExpr::StringLit(name.to_string()));
        self.type_map.insert(key_id, Type::Str);
        let declared_float = matches!(self.global_ty_of(name), Some(Type::F32) | Some(Type::F64));
        let value_float = matches!(
            self.type_map.get(&value),
            Some(Type::F32) | Some(Type::F64)
        );
        let stored = if declared_float && value_float {
            // Read the word back out of the slot's own address: `load i64` from
            // the `double` alloca is the same reinterpretation the read side
            // does. A bare expression is put through a slot first, both to have
            // an address to read and because codegen re-evaluates expressions at
            // each use (see the `=`/`+=` mirror sites).
            let slot = match self.exprs.get(&value) {
                Some(MirExpr::Var(_)) => value,
                _ => {
                    let fresh = self.next_id();
                    self.exprs.insert(fresh, MirExpr::Var(fresh));
                    self.type_map
                        .insert(fresh, self.type_map.get(&value).cloned().unwrap_or(Type::F64));
                    out.push(MirStmt::Assign {
                        lhs: fresh,
                        rhs: value,
                    });
                    fresh
                }
            };
            let addr_id = self.next_id();
            self.exprs
                .insert(addr_id, MirExpr::AddrOf { alloca_id: slot });
            self.type_map.insert(addr_id, Type::I64);
            let bits_id = self.next_id();
            self.exprs.insert(
                bits_id,
                MirExpr::Deref {
                    addr_id,
                    pointee_width: 8,
                },
            );
            self.type_map.insert(bits_id, Type::I64);
            bits_id
        } else {
            value
        };
        out.push(MirStmt::VoidCall {
            func: "zeta_env_set".to_string(),
            args: vec![key_id, stored],
        });
        (out, key_id)
    }

    /// The type to give a slot that just read `name` out of the env cell: the
    /// name's declared type when it has one (the cell holds that value's word),
    /// `I64` otherwise. See `env_store` — write and read have to agree.
    fn env_slot_ty(&self, name: &str) -> Type {
        self.global_ty_of(name).unwrap_or(Type::I64)
    }

    /// PY-A: THE module-global write rule, in one place: every write to a
    /// module global's own slot refreshes the env cell that the other top-level
    /// items read. Runs after a body is lowered, over the whole statement list
    /// including nested blocks.
    ///
    /// This replaces the mirrors that used to sit beside individual STATEMENT
    /// kinds (`=` at the bind, `=` at the rebind, `+=`). Those could never cover
    /// the writes made from inside `lower_expr`: a container method rebinds its
    /// receiver slot at 6 separate sites (`push`, `append`, `add`,
    /// `insert`/`remove`/`sort`/`reverse`/`extend`, `discard`, `set.add`) —
    /// measured before this pass (`/tmp/b390/m1b.z`):
    /// `xs = [3,1,2]; xs.remove(3)` then `def peek() -> i64 { return len(xs) }`
    /// printed `3` while the module body's own `print(xs)` printed `[1, 2]`.
    /// One name, two answers, no diagnostic.
    ///
    /// The slot set is derived, not registered: within one item's lowering, a
    /// module-global name's slot IS `name_to_id[name]` — every writer takes its
    /// lhs from that same lookup, so no bind site has to remember to declare it.
    ///
    /// The mirror reads the SLOT it follows, never the `rhs` id: codegen
    /// re-evaluates an expression at each use, so handing over the right-hand
    /// side a second time counted it twice (`total = total + 3` in one function
    /// returned 7 while the cell held 11; `total += 1; total += 2` returned 20
    /// while the cell held 22 — batches 385/386).
    fn mirror_module_global_writes(&mut self) {
        let slots: Vec<(u32, String)> = self
            .name_to_id
            .iter()
            .filter(|(name, _)| self.module_globals.contains(*name))
            .map(|(name, &slot)| (slot, name.clone()))
            .collect();
        if slots.is_empty() {
            return;
        }
        let src = std::mem::take(&mut self.stmts);
        let mut out: Vec<MirStmt> = Vec::with_capacity(src.len());
        self.splice_env_mirrors(src, &slots, &mut out);
        self.stmts = out;
    }

    /// Copy `src` into `out`, inserting an env mirror after every `Assign` whose
    /// lhs is one of `slots`. `env_mirror` only allocates fresh ids, so splicing
    /// mid-list cannot alias a slot already lowered.
    fn splice_env_mirrors(
        &mut self,
        src: Vec<MirStmt>,
        slots: &[(u32, String)],
        out: &mut Vec<MirStmt>,
    ) {
        for st in src {
            match st {
                MirStmt::Assign { lhs, rhs } => {
                    out.push(MirStmt::Assign { lhs, rhs });
                    for &(slot, ref name) in slots {
                        if slot == lhs {
                            let (mirror, _key) = self.env_mirror(name, lhs);
                            out.extend(mirror);
                        }
                    }
                }
                MirStmt::If {
                    cond,
                    then,
                    else_,
                    dest,
                } => {
                    let (t, e) = (self.sub_splice(then, slots), self.sub_splice(else_, slots));
                    out.push(MirStmt::If {
                        cond,
                        then: t,
                        else_: e,
                        dest,
                    });
                }
                MirStmt::For {
                    iterator,
                    pattern,
                    var_id,
                    counter_id,
                    body,
                    else_body,
                } => {
                    let (b, e) = (self.sub_splice(body, slots), self.sub_splice(else_body, slots));
                    out.push(MirStmt::For {
                        iterator,
                        pattern,
                        var_id,
                        counter_id,
                        body: b,
                        else_body: e,
                    });
                }
                MirStmt::While {
                    cond,
                    pre_cond,
                    body,
                    else_body,
                } => {
                    let (p, b, e) = (
                        self.sub_splice(pre_cond, slots),
                        self.sub_splice(body, slots),
                        self.sub_splice(else_body, slots),
                    );
                    out.push(MirStmt::While {
                        cond,
                        pre_cond: p,
                        body: b,
                        else_body: e,
                    });
                }
                other => out.push(other),
            }
        }
    }

    fn sub_splice(&mut self, src: Vec<MirStmt>, slots: &[(u32, String)]) -> Vec<MirStmt> {
        let mut out = Vec::with_capacity(src.len());
        self.splice_env_mirrors(src, slots, &mut out);
        out
    }

    /// Key for a module-level global read: `Var(n)` -> n, and a module member
    /// (`mod.NAME`) -> `<module with . as _>__NAME` (plus the bare spelling,
    /// which `global_ty_of` also accepts).
    fn receiver_global_key(
        aliases: &HashMap<String, String>,
        r: &AstNode,
    ) -> Option<String> {
        match r {
            AstNode::Var(n) => Some(n.clone()),
            _ => {
                let (root, parts) = Self::flatten_module_receiver(r)?;
                if parts.len() != 1 {
                    return None;
                }
                let module = aliases.get(&root)?.clone();
                Some(format!("{}__{}", module.replace('.', "_"), parts[0]))
            }
        }
    }

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
        if let Some(Type::Named(n, _)) = self.global_ty_of(name) {
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

    /// BATCH-438: the three spellings a method's receiver parameter arrives in.
    /// `trim_start_matches('&')` is NOT enough — `&mut self` becomes `mut self`.
    fn is_receiver_param(name: &str) -> bool {
        matches!(name, "self" | "&self" | "&mut self")
    }

    /// BATCH-438: bind the call sites written inside a `class` in a function body
    /// to that class's own hoisted methods.
    ///
    /// Why a pass AFTER the block instead of a table entry during it: every
    /// method of such a class goes through `lower_closure`, which clones the
    /// parent's `closure_vars` (see there) before this method's sibling has
    /// published anything. So `self._jq_bar_types()` inside `on_start`
    /// (`backend/strategy/nautilus_backend.py:54`) missed the closure dispatch,
    /// kept its qualified spelling, and codegen turned that into an external
    /// symbol nothing defines (`ld: Undefined symbols: __Impl___jq_bar_types` ⇒
    /// `Error: "Linking failed"`). The alias table is complete only once the loop
    /// is done, so the re-binding has to run over the Mir items the loop emitted.
    ///
    /// `{ty}::{member}` with no alias in THIS window is a member the class does
    /// not define at all (an inherited one: `self.subscribe_bars()` from
    /// `class _Impl(Strategy)`). Those get the `[dynamic]` spelling so batch 428's
    /// rule takes them — raise at the call site by name, instead of a declare the
    /// linker can only miss. Before this batch both spellings resolved against the
    /// ENCLOSING class and were papered over by two weak stubs in
    /// `runtime/unavailable_stubs.c:148-153`, which is the silent-wrong-value
    /// shape batch 438 exists to remove.
    ///
    /// Scope: only windows that published aliases are touched, i.e. only a `class`
    /// written inside a function body. A top-level `impl` keeps its pre-438
    /// spellings (its methods are ordinary items; its inherited members are still
    /// resolved downstream), so this pass cannot re-decide those call sites.
    fn rewrite_nested_class_calls(
        &mut self,
        ty: &str,
        aliases: &[(String, String)],
        mir_start: usize,
    ) {
        if ty.is_empty() || aliases.is_empty() {
            return;
        }
        let prefix = format!("{ty}::");
        let rets: HashMap<String, Type> = aliases
            .iter()
            .filter_map(|(key, sym)| {
                self.closure_ret_tys
                    .get(sym)
                    .map(|t| (key.clone(), t.clone()))
            })
            .collect();
        // "Something already defines it" has to be asked of the DEFINITIONS, not
        // of the call-site evidence table: `func_ret_types` gets a key for every
        // `recv.member` the scanner sees, so an undefined member was in it too and
        // the ghost arm never fired (measured: a nested `class Inner(Thing)` whose
        // `on_start` calls `self.notsdefined(x)` kept `Inner::notsdefined`, and
        // codegen emitted `ld: Undefined symbols: _Inner__notsdefined`).
        let defined: std::collections::HashSet<String> = self
            .generated_mirs
            .iter()
            .filter_map(|m| m.name.clone())
            .filter(|n| n.starts_with(&prefix))
            .collect();
        for mir in &mut self.generated_mirs[mir_start..] {
            let Mir { stmts, type_map, .. } = mir;
            Self::rebind_nested_calls(stmts, &prefix, aliases, &defined, &rets, type_map);
        }
    }

    /// BATCH-438: sweep a statement list, including the ones nested inside it.
    /// `mir.rs` nests statements in exactly three variants (`If`, `For`, `While`),
    /// so this covers the whole set; a top-level-only sweep misses every call
    /// written in a loop or branch body (measured: `for x in self.bar_types():
    /// self.notsdefined(x)` kept `Inner::notsdefined` ⇒ `ld: Undefined symbols:
    /// _Inner__notsdefined`, and in the corpus the same shape left
    /// `_Impl__subscribe_bars` undefined).
    fn rebind_nested_calls(
        stmts: &mut [MirStmt],
        prefix: &str,
        aliases: &[(String, String)],
        defined: &std::collections::HashSet<String>,
        rets: &HashMap<String, Type>,
        type_map: &mut HashMap<u32, Type>,
    ) {
        for stmt in stmts.iter_mut() {
            match stmt {
                MirStmt::Call { func, dest, .. } => {
                    if let Some(next) =
                        Self::rebound_nested_target(func, prefix, aliases, defined, rets, Some(*dest), type_map)
                    {
                        *func = next;
                    }
                }
                MirStmt::VoidCall { func, .. } => {
                    if let Some(next) =
                        Self::rebound_nested_target(func, prefix, aliases, defined, rets, None, type_map)
                    {
                        *func = next;
                    }
                }
                MirStmt::If { then, else_, .. } => {
                    Self::rebind_nested_calls(then, prefix, aliases, defined, rets, type_map);
                    Self::rebind_nested_calls(else_, prefix, aliases, defined, rets, type_map);
                }
                MirStmt::For {
                    body, else_body, ..
                } => {
                    Self::rebind_nested_calls(body, prefix, aliases, defined, rets, type_map);
                    Self::rebind_nested_calls(else_body, prefix, aliases, defined, rets, type_map);
                }
                MirStmt::While {
                    pre_cond,
                    body,
                    else_body,
                    ..
                } => {
                    Self::rebind_nested_calls(pre_cond, prefix, aliases, defined, rets, type_map);
                    Self::rebind_nested_calls(body, prefix, aliases, defined, rets, type_map);
                    Self::rebind_nested_calls(else_body, prefix, aliases, defined, rets, type_map);
                }
                _ => {}
            }
        }
    }

    /// The decision for one call target: bind to the class's own hoisted method,
    /// ghost an undefined member so batch 428 raises it, or leave the spelling
    /// alone (not this class's member, or a name an item really defines).
    fn rebound_nested_target(
        func: &str,
        prefix: &str,
        aliases: &[(String, String)],
        defined: &std::collections::HashSet<String>,
        rets: &HashMap<String, Type>,
        dest: Option<u32>,
        type_map: &mut HashMap<u32, Type>,
    ) -> Option<String> {
        if !func.starts_with(prefix) {
            return None;
        }
        if let Some((_, sym)) = aliases.iter().find(|(key, _)| key == func) {
            if let (Some(d), Some(t)) = (dest, rets.get(sym)) {
                type_map.insert(d, t.clone());
            }
            return Some(sym.clone());
        }
        if defined.contains(func) {
            return None;
        }
        Some(format!("[dynamic]{func}"))
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
            // Batch 291: the class name ITSELF starts with an underscore
            // (`log = _LogAdapter()` in jq_shim, used from jq_wufu). The
            // mangled receiver is `jq_shim___LogAdapter`, and `rsplit_once("__")`
            // swallows the class's own leading underscore — the definitions are
            // keyed bare (`_LogAdapter::info`), so every `log.info(...)` fell
            // through to a generic `jq_shim___LogAdapter__info` reference with
            // NO definition (link error). Re-prepend 1-2 underscores.
            for k in 1..=2 {
                let cand = format!("{}{}::{}", "_".repeat(k), tail, method);
                if self.func_ret_types.contains_key(&cand) {
                    return Some(cand);
                }
            }
        }
        // A DOTTED type name (`pd.DataFrame`, from a `-> pd.DataFrame | None`
        // annotation) must still find the class's methods: without this
        // `df["col"] = v` missed `DataFrame::__setitem__` entirely and fell
        // through to `DictInsert` on the DataFrame STRUCT pointer — a garbage
        // write that crashed inside `map_insert`.
        if let Some(last) = tn.rsplit('.').next() {
            let cand = format!("{}::{}", last, method);
            if self.func_ret_types.contains_key(&cand) {
                return Some(cand);
            }
        }
        None
    }

    /// BATCH-429: a property read (`df.columns`, no parens) whose receiver has
    /// NO static class tag. The arm above needs `Type::Named`, so without a tag
    /// the read degraded to a raw field load and returned the handle's first
    /// word (corpus, `ZETA_DBG_FA` on the pre-429 binary: 33 of the 152 reads
    /// that fell through both look-ups are `index` 16 / `columns` 12 /
    /// `empty` 5; the library itself records the symptom at
    /// `pylib/pandas.z:270`). Dispatch on the NAME alone only when it identifies
    /// exactly ONE declared method and that method takes nothing but `self` —
    /// the same uniqueness rule the dynamic-receiver CALL path already uses
    /// (`pylib::method_by_unique_name`, reached at :10299). Ambiguity, a missing
    /// parameter list or a non-zero arity returns `None` and the caller keeps
    /// the old behaviour, which BATCH-427's `FA decls` probe makes audible.
    fn unique_zero_arg_property(&self, name: &str) -> Option<(String, Type)> {
        let suffix = format!("::{}", name);
        let mut hits = self
            .func_ret_types
            .keys()
            .filter(|k| k.ends_with(suffix.as_str()));
        let key = hits.next()?.to_string();
        if hits.next().is_some() {
            return None;
        }
        let params = self.func_param_names.get(&key)?;
        let no_args = params
            .iter()
            .all(|p| matches!(p.as_str(), "self" | "&self" | "&mut self"));
        if !no_args {
            return None;
        }
        Some((key.clone(), self.func_ret_types.get(&key)?.clone()))
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

    /// PY-A (batch 409): lay a call's arguments out in the callee's declared
    /// parameter order. The parser wraps `name=value` as `__kwarg__(name, value)`.
    /// `None` means "not statically bindable" — no markers at all, a `**`
    /// unpacking, a name the callee does not declare, a parameter given twice, or
    /// a parameter that receives neither an argument nor a default — and the
    /// caller then keeps its existing positional list rather than a guess.
    fn bind_kwarg_markers(
        args: &[AstNode],
        params: &[String],
        defaults: Option<&[Option<AstNode>]>,
    ) -> Option<Vec<AstNode>> {
        let mut pos: Vec<&AstNode> = Vec::new();
        let mut kw: Vec<(&str, &AstNode)> = Vec::new();
        for a in args {
            match a {
                AstNode::Call {
                    receiver: None,
                    method,
                    args: ka,
                    ..
                } if method == "__kwarg__" && ka.len() == 2 => match &ka[0] {
                    AstNode::StringLit(n) => kw.push((n.as_str(), &ka[1])),
                    _ => return None,
                },
                AstNode::Call {
                    receiver: None,
                    method,
                    ..
                } if method == "zeta_kwargs_unpack" => return None,
                other => pos.push(other),
            }
        }
        if kw.is_empty() {
            return None;
        }
        let mut slots: Vec<Option<AstNode>> = params.iter().map(|_| None).collect();
        for (i, a) in pos.into_iter().enumerate() {
            let slot = slots.get_mut(i)?;
            *slot = Some(a.clone());
        }
        for (name, value) in kw {
            let i = params.iter().position(|p| p == name)?;
            let slot = &mut slots[i];
            if slot.is_some() {
                return None;
            }
            *slot = Some(value.clone());
        }
        for (i, slot) in slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = defaults?.get(i)?.clone();
            }
        }
        Some(slots.into_iter().flatten().collect())
    }

/// PY-A: a call that binds fewer arguments than the callee declares and has no
/// default for the rest is a Python `TypeError`. We cannot fail the build here
/// (platform shims legitimately differ), but staying silent would repeat the
/// exact failure mode this work removes — a wrong value with no diagnostic —
/// so name the unbound parameters, then give each of them an explicit 0 in
/// ITS OWN slot. The dispatch sites collect with `flatten()`, which used to
/// delete the hole and shift every later argument one parameter to the left, so
/// `f(b = 3)` handed b's value to `a` (batch 412: measured `F 3 0` where Python
/// raises `TypeError: f() missing 1 required positional argument: 'a'`).
fn warn_unbound(callee: &str, params: &[String], slots: &mut Vec<Option<AstNode>>) {
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
    for slot in slots.iter_mut() {
        if slot.is_none() {
            *slot = Some(AstNode::Lit(0));
        }
    }
}

    /// Pre-seed type declarations collected program-wide by the Resolver.
    pub fn with_type_decls(mut self, decls: HashMap<String, TypeDecl>) -> Self {
        self.shared_type_decls = decls;
        self
    }

    /// PY-A: the file being compiled, so `__file__` can resolve to it.
    pub fn with_current_module(mut self, module: String) -> Self {
        self.current_module = module;
        self
    }

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

    /// Point every `Return` in an entry-function body (nested `if`/`for`/`while`
    /// branches included) at `zero`. See the `py_entry` call site in
    /// `lower_to_mir` and `PY_ENTRY_ATTR` in the parser.
    fn force_entry_returns(stmts: &mut [MirStmt], zero: u32) {
        for stmt in stmts {
            match stmt {
                MirStmt::Return { val } => *val = zero,
                MirStmt::If { then, else_, .. } => {
                    Self::force_entry_returns(then, zero);
                    Self::force_entry_returns(else_, zero);
                }
                MirStmt::For {
                    body, else_body, ..
                }
                | MirStmt::While {
                    body, else_body, ..
                } => {
                    Self::force_entry_returns(body, zero);
                    Self::force_entry_returns(else_body, zero);
                }
                _ => {}
            }
        }
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
        // T0 (refactor.md B.5): this scope's closure-symbol namespace, consumed
        // by `lower_closure`. Declared name for a function, module name for a
        // non-FuncDef item (module bodies, hoisted items).
        self.closure_ns = match ast {
            AstNode::FuncDef { name, .. } | AstNode::ExternFunc { name, .. } => name.clone(),
            _ => self.current_module.clone(),
        };
        self.closure_seq = 0;
        // PY-A (任务 #55): see `py_entry` — reset per item, this generator is reused.
        self.py_entry = matches!(ast, AstNode::FuncDef { attrs, .. }
            if attrs.iter().any(|a| a.as_str() == crate::frontend::parser::top_level::PY_ENTRY_ATTR));

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
                    } else if pt_str == "str" || pt_str == "Str" {
                        // Inferred string parameter (Python functions carry no
                        // annotations): string ops on it must dispatch as str.
                        // `Str` is the same type under its Zeta spelling — the
                        // self-host corpus declares `fn tokenize(input: Str)`,
                        // which used to reach the class arm below as
                        // `Named("Str")` (a fake class) and so lost every str
                        // dispatch: `input[i]` became a DictGet over a `char*`.
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
                        // Batch 291: store the CANONICAL TAG, not the spelling.
                        // `d: datetime` used to stay `Named("datetime")`, which
                        // blocked the struct-annotation mapper below (it only
                        // fires on I64/PyDynamic) — so `handle_op` never saw
                        // PyDate, `d - pd.Timedelta(days=1)` degraded to a
                        // DynamicArray, and `.strftime` dispatched to
                        // `zeta_vec_strftime` on a scalar handle (SIGSEGV in
                        // `new_context`, jq_shim.py:644).
                        let tag = crate::middle::pylib::handle_tag(param_type.trim())
                            .unwrap_or(param_type.trim());
                        self.type_map.insert(id, Type::Named(tag.to_string(), vec![]));
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
                    // A CLASS annotation (`df: pd.DataFrame`, `cfg: MarketCleanConfig`)
                    // is the only place a parameter's struct/handle identity exists.
                    // Leaving the param I64 made `len(df)` dispatch to `array_len`
                    // (0) and `df.empty` a bare `empty` call — measured:
                    // `validate_and_repair_stock_ohlcv` took its `if df.empty:
                    // return pd.DataFrame()` early exit and the cache load returned
                    // an EMPTY frame.
                    if matches!(self.type_map.get(&id), Some(Type::I64) | Some(Type::PyDynamic)) {
                        let ann = param_type.trim();
                        if !ann.is_empty() && ann != "dyn" && ann != "()" {
                            let ty = Type::from_string(ann);
                            if matches!(ty, Type::Named(_, _)) {
                                let mapped = match &ty {
                                    Type::Named(n, args) => match crate::middle::pylib::handle_tag(n)
                                    {
                                        Some(tag) => Type::Named(tag.to_string(), args.clone()),
                                        None => {
                                            // `pandas.DataFrame` -> `DataFrame`
                                            let last = n
                                                .rsplit(['.', ':'])
                                                .find(|s| !s.is_empty())
                                                .unwrap_or(n);
                                            Type::Named(last.to_string(), args.clone())
                                        }
                                    },
                                    _ => ty.clone(),
                                };
                                self.type_map.insert(id, mapped);
                            }
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
            // PY-A (任务 #55): the entry function's tail value is NOT handed back —
            // see `force_entry_returns`, the single place that rewrites returns.
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

        // PY-A (任务 #55): the process entry handed its tail value back to clang's
        // crt, which turns it into the EXIT CODE — so `l = [3,5,7]; sum(l)` exited 15
        // while CPython exits 0 (Python only echoes a REPL result). A return can
        // reach `main` by four routes (the gate above, `lower_expr`'s Return arm,
        // the parser's tail-expression promotion into `FuncDef::ret_expr`, and an
        // `ExprStmt`-wrapped return), so instead of gating each one this rewrites
        // every `Return` of the entry body — including inside if/for/while branches
        // — to a single zero slot. Side effects stay: only the handed-back slot
        // changes. A brace-style `fn main() -> i64 { 42 }` is untouched because the
        // parser never puts `py_entry` on it (see `PY_ENTRY_ATTR`).
        if self.py_entry {
            let zero = self.next_id_with_lit(0);
            Self::force_entry_returns(&mut self.stmts, zero);
        }

        // PY-A (任务 #102 / 批次 391): the module-global env mirror, once, over
        // the finished body — see `mirror_module_global_writes`.
        self.mirror_module_global_writes();

        // Batch 299: a bare `-> dict` / `-> list` annotation carries NO element
        // type (`map` with zero type arguments), so every consumer of the call
        // result degraded to i64. The RESOLVER refines such an annotation with
        // the container type the body actually returns (`MirGen` is rebuilt per
        // top-level item, so a refinement made here never reaches a call site).
        let mut mir = Mir {
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
            member_bare_calls: std::mem::take(&mut self.member_bare_calls),
            plain_call_names: std::mem::take(&mut self.plain_call_names),
        };
        Self::pin_declared_int_return(&mut mir, ast);
        mir
    }

    /// PY-A (批次 454 第五格): `Mir::signature_ret_ty` answers from the BODY (the
    /// first top-level `return` whose value carries a type), while a call site is
    /// typed from the DECLARED `-> T` (the resolver's `func_ret_types`). The
    /// integer-`/` promotion lets those two sources disagree for a function that
    /// declares an int and does `return a / b`: the callee gets a `double` LLVM
    /// signature, the caller reads the word as an integer, and R7 reinterprets
    /// the bits instead of converting (measured on the official corpus:
    /// `divide(100, 4)` in `test_arithmetic` printed 4627730092099895296 — the
    /// bits of 25.0 — and `murphy_sieve`'s `return limit / 10` did the same at 5
    /// call sites). The pin: when the declaration pins an int word AND the slot
    /// that decides the signature is exactly a promoted integer quotient, that
    /// quotient keeps `sdiv`. A float that came from anywhere else (`return 2.5`
    /// in a `-> i64` function) is left alone — that disagreement predates this
    /// batch and belongs to batch 399 / task #33.
    fn pin_declared_int_return(mir: &mut Mir, ast: &AstNode) {
        const INT_SPELLINGS: [&str; 8] =
            ["i64", "int", "i32", "u64", "u32", "isize", "usize", "bool"];
        let AstNode::FuncDef { ret, .. } = ast else {
            return;
        };
        if !INT_SPELLINGS.contains(&ret.as_str()) {
            return;
        }
        // Same scan as `signature_ret_ty`: the FIRST top-level return whose value
        // has a type is what decides the callee's LLVM signature.
        let Some(val) = mir.stmts.iter().find_map(|s| match s {
            MirStmt::Return { val } => mir.type_map.get(val).map(|_| *val),
            _ => None,
        }) else {
            return;
        };
        if !matches!(mir.type_map.get(&val), Some(Type::F32) | Some(Type::F64)) {
            return;
        }
        let is_promoted_quotient =
            mir.stmts
                .iter()
                .any(|s| matches!(s, MirStmt::Call { func, args, dest, .. }
                    if func == "/"
                        && dest == &val
                        && args.len() == 2
                        && args.iter().all(|a| matches!(
                            mir.type_map.get(a),
                            Some(Type::I64) | Some(Type::Bool)
                        ))));
        if is_promoted_quotient {
            mir.type_map.insert(val, Type::I64);
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
            // The declaration itself was lifted to a module-level assignment by
            // `hoist_statics`, which runs once at program start, so this node is
            // only the marker saying "in this function, the name is that cell".
            // `nonlocal_names` is exactly that routing (reads and writes go to the
            // env global, no local slot), and it is per-function here because a
            // MirGen is built per function.
            AstNode::Static {
                name,
                hoisted: true,
                ..
            } => {
                self.nonlocal_names.insert(name.clone());
            }
            // Not lifted — it sits where the pass cannot reach (a `macro_rules!`
            // body is expanded afterwards) or its name was already taken. Lowering
            // it as a plain local resets the value on every call and prints a
            // plausible wrong number in silence, so it says which one it did.
            AstNode::Static { name, expr, .. } => {
                eprintln!(
                    "warning: [W1008] `{name}` is still inside a function body, so it is a \
                     local that resets on every call rather than one persistent cell"
                );
                self.lower_ast_inner(&AstNode::Let {
                    mut_: true,
                    pattern: Box::new(AstNode::Var(name.clone())),
                    ty: None,
                    expr: expr.clone(),
                });
            }
            AstNode::Let { pattern, expr, .. } => {
                // Handle different pattern types
                match &**pattern {
                    AstNode::Var(name) if self.nonlocal_names.contains(name) => {
                        // PY-A V3: nonlocal name — defining assignment stores
                        // through env; bind local slot to an env load.
                        let rhs_id = self.lower_expr(expr);
                        let key_id = self.env_store(name, rhs_id);
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
                        ty,
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
                            let rhs_ty = self.type_map.get(&rhs_id).cloned();
                            // `x: set[str] = set()` — the ANNOTATION is the only
                            // place the element type exists: `set()` lowers to an
                            // empty DynamicArray, so dropping the annotation left
                            // the element I64 ("unknown") and `"q" in c` compared
                            // HANDLES — every string counted as absent (measured:
                            // `fetched_codes: set[str] = set()` in fetch_stocks).
                            let refined = rhs_ty.clone().map(|t| {
                                match (&t, Self::annotation_elem_ty(ty)) {
                                    (Type::DynamicArray(e), Some(el))
                                        if matches!(**e, Type::I64) =>
                                    {
                                        Type::DynamicArray(Box::new(el))
                                    }
                                    // `set()` may come back wholly untyped
                                    // (I64 / PyDynamic) — the annotation still
                                    // says what the container holds.
                                    (Type::I64, Some(el)) | (Type::PyDynamic, Some(el)) => {
                                        Type::DynamicArray(Box::new(el))
                                    }
                                    _ => t,
                                }
                            });
                            // `partial: pd.DataFrame | None = None` — the None
                            // initializer types the slot I64, and later rebinds
                            // do not refresh types, so `len(partial)` kept
                            // dispatching `array_len` on the DataFrame object
                            // (header read = 0 rows) and every fetch_stocks
                            // cache gate failed. The annotation names the real
                            // class; adopt it when the initializer is untyped.
                            let untyped_init = matches!(
                                refined,
                                None | Some(Type::I64) | Some(Type::PyDynamic)
                            );
                            let refined = if untyped_init {
                                self.annotation_named_ty(ty).or(refined)
                            } else {
                                refined
                            };
                            let refined = refined.unwrap_or(Type::I64);
                            self.type_map.insert(lhs_id, refined.clone());
                            if matches!(refined, Type::Named(_, _)) {
                                self.apply_dict_annotation(lhs_id, ty);
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
                // PY-A: annotated assignment `partial: pd.DataFrame | None = None`
                // — the parser keeps the class-shaped annotation attached. Lower
                // the plain assignment first, then (when the slot is still
                // untyped) adopt the annotated class: `= None` types the slot
                // I64 and rebinds never refresh it, so `len(partial)` used to
                // dispatch `array_len` on a DataFrame handle (header read = 0
                // rows) and every fetch_stocks cache gate failed.
                if let AstNode::TypeAnnotatedPattern { pattern: inner, ty } = &**lhs {
                    if let AstNode::Var(name) = &**inner {
                        let bare = AstNode::Assign(
                            Box::new((**inner).clone()),
                            Box::new((**rhs).clone()),
                        );
                        self.lower_ast(&bare);
                        if let Some(&slot) = self.name_to_id.get(name.as_str()) {
                            let cur = self.type_map.get(&slot).cloned();
                            if matches!(cur, None | Some(Type::I64) | Some(Type::PyDynamic)) {
                                if let Some(nt) = self.annotation_named_ty(ty) {
                                    self.type_map.insert(slot, nt);
                                }
                            }
                            self.apply_dict_annotation(slot, ty);
                        }
                        return;
                    }
                }
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
                                        ts.get(i).cloned().unwrap_or(Type::I64)
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
                                        ts.get(i).cloned().unwrap_or(Type::I64)
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
                        let val_ty = self.type_map.get(&rhs_id).cloned().unwrap_or(Type::I64);
                        let key_ty = self
                            .type_map
                            .get(&index_id)
                            .cloned()
                            .unwrap_or(Type::I64);
                        if let Type::Named(_, targs) = &base_ty {
                            let old_key = targs.first().cloned().unwrap_or(Type::I64);
                            let old_val = targs.get(1).cloned().unwrap_or(Type::I64);
                            let new_key = if matches!(old_key, Type::I64)
                                && matches!(key_ty, Type::Str)
                            {
                                Type::Str
                            } else {
                                old_key.clone()
                            };
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
                    // Remember the literal's element count for `f(*x)` below.
                    if let AstNode::ArrayLit(items) = &**rhs {
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
            }
            AstNode::AssignOp { op, target, value } => {
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
                    let ty = self.type_map.get(&rhs_id).cloned().unwrap_or(Type::I64);
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
            AstNode::Return(inner) => {
                // `return (a, b)` must hand back a HEAP array. A StackArray is an
                // alloca: the pointer dies with the frame, so the caller's
                // `stack_array_get` destructuring read dead stack (measured:
                // `return d.iloc[0:0], 7` → `len(o)` SEGV). Build a real
                // `[cap|len]` array — `stack_array_get(arr, i)` reads
                // `((i64*)arr)[i]`, i.e. exactly the dynarray DATA pointer that
                // `zeta_dynarray_new`/`vec_push` hand out.
                let val = if let AstNode::Tuple(items) = &**inner {
                    let mut vals = Vec::with_capacity(items.len());
                    let mut tys = Vec::with_capacity(items.len());
                    for it in items {
                        let vid = self.lower_expr(it);
                        tys.push(self.type_map.get(&vid).cloned().unwrap_or(Type::I64));
                        vals.push(vid);
                    }
                    let cap = self.next_id_with_lit(items.len() as i64);
                    let h = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_dynarray_new".to_string(),
                        args: vec![cap],
                        dest: h,
                        type_args: vec![],
                    });
                    self.exprs.insert(h, MirExpr::Var(h));
                    self.type_map.insert(
                        h,
                        Type::DynamicArray(Box::new(tys.first().cloned().unwrap_or(Type::I64))),
                    );
                    // Mirror the ArrayLit lowering EXACTLY: every push targets the
                    // ORIGINAL handle `h` (the runtime grows it and returns a new
                    // data pointer, which the codegen tracks via the call's dest),
                    // and each dest is registered as its own Var. Chaining the
                    // handles (`cur = pushed`) made `vec_push` receive an
                    // unregistered slot and SEGV inside `vec_push + 24`.
                    for v in vals {
                        let sink = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "vec_push".to_string(),
                            args: vec![h, v],
                            dest: sink,
                            type_args: vec![],
                        });
                        self.exprs.insert(sink, MirExpr::Var(sink));
                        self.type_map.insert(
                            sink,
                            Type::DynamicArray(Box::new(tys.first().cloned().unwrap_or(Type::I64))),
                        );
                    }
                    self.type_map.insert(h, Type::Tuple(tys));
                    self.tuple_slots.insert(h);
                    h
                } else {
                    self.lower_expr(inner)
                };
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
                    // BATCH-441: the parser promotes a block body's TAIL element out of
                    // `body` into `ret_expr` (`top_level.rs:297-331`); the non-nested arm
                    // below consumes it, this hoisted path cloned only `body` — so every
                    // nested `def`, and every method of a `class` written inside a
                    // function body, silently lost its LAST statement (t472; a trailing
                    // `try:` is a `Block` whose tail is an `If`, which is why batch 438
                    // registered ② saw a whole method body vanish). Put it back as a
                    // STATEMENT: Python discards a trailing expression's value, and the
                    // hoisted copy's return stays whatever its own `return` says.
                    let mut hoisted_body = body.clone();
                    if let Some(tail) = ret_expr {
                        hoisted_body.push(tail.as_ref().clone());
                    }
                    let body_node = AstNode::Block { body: hoisted_body };
                    let hoisted = self.lower_closure(&param_names, &body_node);
                    // BATCH-438: a method of a `class` written inside a function
                    // body is called through its QUALIFIED name — the receiver is
                    // typed `Named(Inner)`, so the call route asks for
                    // `Inner::bump`, which no table carried (measured on the pre
                    // binary: `Undefined symbols for architecture arm64:
                    // "_Inner__bump"`). Publish that spelling alongside the bare
                    // one; the definition keeps its unique `__closure_*` symbol,
                    // so two modules may each own a nested `_Impl::on_start`
                    // without colliding.
                    if let Some(cls) = self.current_class.clone()
                        && param_names.iter().any(|p| Self::is_receiver_param(p))
                    {
                        let qualified = format!("{cls}::{fn_name}");
                        self.closure_vars.insert(qualified.clone(), hoisted.clone());
                        self.hoisted_names.insert(qualified.clone(), hoisted.clone());
                        self.nested_class_aliases
                            .push((qualified, hoisted.clone()));
                    }
                    // bind user name → synthetic fn so `inc()` calls dispatch
                    self.closure_vars.insert(fn_name.clone(), hoisted.clone());
                    // The call site reads the return type off `closure_ret_tys`
                    // (and falls back to I64, which for a string means "print the
                    // heap address"). The Closure-expression arm records it; the
                    // hoisted-def arm had no equivalent line, so every nested
                    // `def` was called through an I64-typed destination.
                    if let Some(t) = self.last_closure_ret_ty.clone() {
                        self.closure_ret_tys.insert(hoisted.clone(), t);
                    }
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

                // An expression-if's dest starts I64; a str-valued branch then
                // leaks a pointer-typed slot (batch 293: `[d for d in days if
                // cond]` desugars to `if cond { d } else { -1 }`, the dest
                // stayed I64, so `trading_days_filtered` was DynamicArray(I64)
                // and the driver's date filter pointer-compared to 0 days).
                // Trust the branch that carries the comprehension element; the
                // -1 skip sentinel needs no type.
                if let Some(dest) = dest_id {
                    let branch_val = |stmts: &Vec<MirStmt>| -> Option<u32> {
                        stmts.last().and_then(|s| match s {
                            MirStmt::Assign { lhs, rhs } if *lhs == dest => Some(*rhs),
                            _ => None,
                        })
                    };
                    let refined = branch_val(&then_stmts)
                        .or_else(|| branch_val(&else_stmts))
                        .and_then(|v| self.type_map.get(&v).cloned());
                    if let Some(t) = refined {
                        self.type_map.insert(dest, t);
                    }
                }

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
                    // The temp must CARRY the expression's type. Typing it I64
                    // unconditionally erased F64 from every expression-statement
                    // arm of a ternary: `return p*(1+self.s) if c else ...`
                    // (`CostModel.fill_price`) then inferred an i64 signature, so
                    // the early `return price` compiled to `fptosi double->i64`
                    // and the caller's bitcast produced 2.5e-323 — i.e. the whole
                    // local backtest had `total = 0` and `final_value -> 0`.
                    let temp_ty = self
                        .type_map
                        .get(&expr_id)
                        .cloned()
                        .unwrap_or(Type::I64);
                    self.type_map.insert(temp_id, temp_ty);
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
                            // `for k, grp in df.groupby(col)` — call the runtime
                            // grouping DIRECTLY with the call's own receiver/key
                            // (probed: going through the shim's `GroupBy` struct
                            // delivered key == 0, so every group collapsed).
                            let grp_call = match &*expr_clone {
                                AstNode::Call {
                                    receiver: Some(recv),
                                    method,
                                    args,
                                    ..
                                } if method == "groupby" && !args.is_empty() => {
                                    Some((recv.clone(), args[0].clone()))
                                }
                                _ => None,
                            };
                            let raw_id = if let Some((recv, key)) = grp_call {
                                let recv_id = self.lower_expr(&recv);
                                let key_id = self.lower_expr(&key);
                                let pid = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: "py_df_groupby".to_string(),
                                    args: vec![recv_id, key_id],
                                    dest: pid,
                                    type_args: vec![],
                                });
                                self.exprs.insert(pid, MirExpr::Var(pid));
                                self.type_map.insert(
                                    pid,
                                    Type::DynamicArray(Box::new(Type::Tuple(vec![
                                        Type::Str,
                                        Type::Named("DataFrame".to_string(), vec![]),
                                    ]))),
                                );
                                pid
                            } else if matches!(
                                self.type_map.get(&raw_id),
                                Some(Type::Named(n, _)) if n == "GroupBy"
                            ) {
                                let pid = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: "py_groupby_pairs".to_string(),
                                    args: vec![raw_id],
                                    dest: pid,
                                    type_args: vec![],
                                });
                                self.exprs.insert(pid, MirExpr::Var(pid));
                                self.type_map.insert(
                                    pid,
                                    Type::DynamicArray(Box::new(Type::Tuple(vec![
                                        Type::Str,
                                        Type::Named("DataFrame".to_string(), vec![]),
                                    ]))),
                                );
                                pid
                            } else if matches!(
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
                                    // Batch 288: a json.items() pair — the value
                                    // half is a PyJson handle (the array element
                                    // type is plain i64, see py_json_items_ids at
                                    // the `.items()` lowering). BATCH-296: the
                                    // KEY half is text — `py_map_items` hands
                                    // back `zt_key_display(key)`, and a JSON
                                    // object's keys are always strings. Typed as
                                    // i64 it defeated content hashing on insert
                                    // (`{k: v for k, v in d.items()}` stored raw
                                    // pointers → every later `"512050.XSHG" in m`
                                    // missed) and `str(k)` stringified the
                                    // POINTER: the ETF listing cache came back as
                                    // 1 entry, the 上市日 filter dropped nothing
                                    // and 28 codes fell to the network.
                                    let pair_tys = pair_tys.or_else(|| {
                                        if self.py_json_items_ids.contains(&raw_id) {
                                            Some(vec![
                                                Type::Str,
                                                Type::Named("PyJson".to_string(), vec![]),
                                            ])
                                        } else {
                                            None
                                        }
                                    });
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
                    // `for i: usize in …` carries the annotation as a wrapper
                    // around the name. Unwrapping it here keeps the range path;
                    // previously the wrapper missed all three arms, the loop
                    // fell through to the COLLECTION path, and a Range iterated
                    // as an empty collection — body ran zero times, silently.
                    AstNode::TypeAnnotatedPattern { pattern: inner, .. } => match &**inner {
                        AstNode::Var(n) => Some(n.clone()),
                        AstNode::Ignore => Some("__wildcard".to_string()),
                        _ => None,
                    },
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

                    // PY-A (任务 #54): the induction counter is a SECOND slot.
                    // Sharing `var_id` made the loop's bookkeeping observable:
                    // `for k in range(3)` left k=3 (Python: 2, the last bound
                    // value), and a body that wrote k advanced from *that* value
                    // (`for k in range(3): k = 9` ran once and left k=10).
                    let counter_id = self.next_id();
                    self.exprs.insert(counter_id, MirExpr::Var(counter_id));
                    self.type_map.insert(counter_id, Type::I64);

                    // Initialize loop variable: let mut i = start
                    self.stmts.push(MirStmt::Assign {
                        lhs: var_id,
                        rhs: start_id,
                    });
                    self.stmts.push(MirStmt::Assign {
                        lhs: counter_id,
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
                    let mut body_stmts = self.stmts.split_off(stmts_before_body);

                    // PY-A (任务 #54): bind the user-visible name to the counter
                    // at the TOP of every taken iteration — that is what Python
                    // does (`for k in range(3)` binds 0, 1, 2 and the iterator
                    // then stops without binding 3). Prepending after the split
                    // keeps it invisible to the body's own statement list.
                    body_stmts.insert(
                        0,
                        MirStmt::Assign {
                            lhs: var_id,
                            rhs: counter_id,
                        },
                    );

                    // PY-A: `for … else` — lowered after the split.
                    let else_stmts = self.lower_loop_else(else_body);

                    // Create For statement in MIR
                    self.stmts.push(MirStmt::For {
                        iterator: range_id,
                        pattern: var_name.clone(),
                        var_id,
                        counter_id,
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
            AstNode::ImplBlock { ty, body, .. } => {
                // Lower any items inside the impl block (functions, etc.).
                // BATCH-438: a `class` written inside a function body desugars
                // to [StructDef, ImplBlock, ctor] and this arm is the only
                // place that still holds the class NAME — the methods below are
                // lowered as plain nested `def`s, so without publishing it here
                // `Inner::bump`'s `self` was captured from the env and typed as
                // the ENCLOSING class (its field reads resolved against that
                // layout, declined, and fell through to the `("", 2)` stand-in).
                let outer_class = std::mem::replace(
                    &mut self.current_class,
                    if ty.is_empty() { None } else { Some(ty.clone()) },
                );
                // BATCH-438: everything this block publishes belongs to THIS
                // window: a second `class _Impl` elsewhere in the program owns a
                // second window with the same key spelling and different symbols.
                let mir_start = self.generated_mirs.len();
                let alias_start = self.nested_class_aliases.len();
                for item in body {
                    self.lower_ast(item);
                }
                let aliases = self.nested_class_aliases.split_off(alias_start);
                self.rewrite_nested_class_calls(ty, &aliases, mir_start);
                self.current_class = outer_class;
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
            AstNode::Match { .. } => {
                // Statement-position `match`: the arms are lowered by the
                // expression path (its branch statements are already hoisted
                // into the arm branches), and the match's value slot is unused.
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
    /// Copy a value into a FRESH slot so it can be passed to a runtime call:
    /// codegen's call-arg path reads operands from local slots, and an
    /// expression result (FieldAccess, call, subscript) has no alloca.
    fn materialize_for_call(&mut self, id: u32) -> u32 {
        let ty = self.type_map.get(&id).cloned().unwrap_or(Type::I64);
        let slot = self.next_id();
        self.stmts.push(MirStmt::Assign {
            lhs: slot,
            rhs: id,
        });
        self.exprs.insert(slot, MirExpr::Var(slot));
        self.type_map.insert(slot, ty);
        slot
    }

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
    /// Element type declared by a container ANNOTATION (`list[str]`,
    /// `set[str]`, `frozenset[int]`). Zeta has no set type — sets degrade to
    /// DynamicArray — so the annotation is the only source of the element type.
    fn annotation_elem_ty(ty: &str) -> Option<Type> {
        let t = ty.trim();
        for kw in ["list", "set", "frozenset", "List", "Set", "FrozenSet"] {
            if let Some(rest) = t.strip_prefix(kw) {
                if let Some(inner) = rest
                    .trim()
                    .strip_prefix('[')
                    .and_then(|r| r.trim_end().strip_suffix(']'))
                {
                    return match inner.trim() {
                        "str" | "String" => Some(Type::Str),
                        "int" | "i64" => Some(Type::I64),
                        "float" | "f64" => Some(Type::F64),
                        "bool" => Some(Type::Bool),
                        _ => None,
                    };
                }
            }
        }
        None
    }

    /// `map<K, V>` / `dict[K, V]` → the declared key and value types, but ONLY
    /// when this table knows both element names. `Any` and `object` say "no
    /// single type" (`Type::PyDynamic`). A name it does not know (a class, a
    /// nested container) yields `None` for the whole annotation: half-applying
    /// `dict[str, pd.DataFrame]` asserts `map<str, i64>` over a map of object
    /// handles, which is a worse claim than the `i64` placeholder it replaces.
    /// The parser normalizes the python spelling to angle brackets (parse_type).
    fn annotation_dict_kv(ty: &str) -> Option<(Type, Type)> {
        let t = ty.trim().trim_start_matches("typing.").to_string();
        let (open, close) = if t.contains('<') {
            ('<', '>')
        } else {
            ('[', ']')
        };
        let (head, rest) = t.split_once(open)?;
        if !matches!(head.trim(), "map" | "dict" | "Dict") {
            return None;
        }
        let inner = rest.trim().trim_end_matches(close).trim();
        let one = |s: &str| -> Option<Type> {
            match s.trim() {
                "Any" | "object" => Some(Type::PyDynamic),
                "str" | "String" => Some(Type::Str),
                "int" | "i64" => Some(Type::I64),
                "float" | "f64" => Some(Type::F64),
                "bool" => Some(Type::Bool),
                _ => None,
            }
        };
        let mut it = inner.split(',');
        Some((it.next().and_then(one)?, it.next().and_then(one)?))
    }

    /// Batch 413: apply a `dict[K, V]` annotation to a slot that already holds a
    /// map. `c: dict[str, Any] = {}` lowers the initializer to `map<i64, i64>`,
    /// and the batch-299 refinement then pinned the value type to whatever the
    /// FIRST write carried — so after `c["df"] = df` every later read of the map
    /// dispatched statically to `DataFrame::__len__`, including `len(c["lst"])`,
    /// which re-interpreted a list header as a map and segfaulted in
    /// `map_resolve(0x1)`. `Any` is the only place "many types" is written down.
    fn apply_dict_annotation(&mut self, slot: u32, ty: &str) {
        let Some((nk, nv)) = Self::annotation_dict_kv(ty) else {
            return;
        };
        let (cur_key, cur_val) = match self.type_map.get(&slot) {
            Some(Type::Named(n, targs)) if n == "map" || n == "dict" => (
                targs.first().cloned().unwrap_or(Type::I64),
                targs.get(1).cloned().unwrap_or(Type::I64),
            ),
            _ => return,
        };
        // Only the untouched `i64` placeholder gives way to the annotation.
        let key = if matches!(cur_key, Type::I64) { nk } else { cur_key.clone() };
        let val = if matches!(cur_val, Type::I64) { nv } else { cur_val.clone() };
        if key == cur_key && val == cur_val {
            return;
        }
        self.type_map
            .insert(slot, Type::Named("map".to_string(), vec![key, val]));
    }

    /// `pd.DataFrame | None` → `Type::Named("DataFrame")` when the tail class
    /// actually has methods registered (a `Class::method` key exists). Only
    /// uppercase class tokens qualify, so `str`/`int` annotations are ignored.
    fn annotation_named_ty(&self, ty: &str) -> Option<Type> {
        for part in ty.split('|') {
            let p = part.trim();
            if p.is_empty() || p.eq_ignore_ascii_case("none") {
                continue;
            }
            let last = p.rsplit('.').next().unwrap_or(p);
            if !last.starts_with(char::is_uppercase) {
                continue;
            }
            let prefix = format!("{}::", last);
            if self.func_ret_types.keys().any(|k| k.starts_with(&prefix)) {
                return Some(Type::Named(last.to_string(), vec![]));
            }
        }
        None
    }

    /// Both arms of an inline conditional are string literals/f-strings.
    fn both_branches_are_strings(n: &AstNode) -> bool {
        let strish = |x: &AstNode| {
            matches!(
                x,
                AstNode::StringLit(_) | AstNode::FString { .. } | AstNode::FString(..)
            )
        };
        match n {
            AstNode::If { then, else_, .. } => {
                let t = then.iter().rev().find_map(|s| match s {
                    AstNode::ExprStmt { expr } => Some(&**expr),
                    AstNode::Return(e) => Some(&**e),
                    _ => None,
                });
                let e = else_.iter().rev().find_map(|s| match s {
                    AstNode::ExprStmt { expr } => Some(&**expr),
                    AstNode::Return(e) => Some(&**e),
                    _ => None,
                });
                matches!((t, e), (Some(a), Some(b)) if strish(a) && strish(b))
            }
            _ => false,
        }
    }

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

    /// `(enum_name, tag, payload_count)` of a registered user-enum variant path
    /// (`Token::Ident` → `("Token", 0, 1)`).
    fn enum_variant_of(&self, name: &str) -> Option<(String, i64, usize)> {
        let (enum_name, variant) = name.rsplit_once("::")?;
        match self.type_decls.get(enum_name)? {
            TypeDecl::Enum { variants, .. } => variants
                .iter()
                .position(|(v, _)| v == variant)
                .map(|i| (enum_name.to_string(), i as i64, variants[i].1.len())),
            _ => None,
        }
    }

    /// Prepend the `__tag` slot to a boxed enum variant's field list.
    ///
    /// A value of a payload-carrying variant is the heap block `[tag, p0, …]`
    /// that the arm test reads (see `enum_is_boxed`), and codegen stores the
    /// vector in order, so the tag only has to be *first* in the list. A zero
    /// payload count, a length that disagrees with the declaration, and a bare
    /// class name (no `::`) all leave the list untouched — a class literal has
    /// no tag and an all-unit enum is a bare discriminant.
    ///
    /// This is the same rule the tuple-ctor route applies at its own site.
    fn with_enum_tag(
        &mut self,
        variant: &str,
        mut fields: Vec<(String, u32)>,
    ) -> Vec<(String, u32)> {
        let tag = match self.enum_variant_of(variant) {
            Some((_, tag, payload_n)) if payload_n > 0 && payload_n == fields.len() => Some(tag),
            _ => None,
        };
        if let Some(tag) = tag {
            let tag_id = self.next_id();
            self.exprs.insert(tag_id, MirExpr::IntLit(tag));
            self.type_map.insert(tag_id, Type::I64);
            fields.insert(0, ("__tag".to_string(), tag_id));
        }
        fields
    }

    /// Does an enum's value representation have to be a heap block?
    ///
    /// A variant with payload needs `[tag, p0, …]`, so *every* variant of that
    /// enum is boxed — a mixed representation would make the arm test have to
    /// tell a discriminant integer from a heap address at run time, which is
    /// exactly the read-the-wrong-thing crash this family keeps producing.
    /// All-unit enums keep the bare integer discriminant, so `== Color::Red`
    /// and every existing match on such an enum is untouched.
    fn enum_is_boxed(&self, enum_name: &str) -> bool {
        match self.type_decls.get(enum_name) {
            Some(TypeDecl::Enum { variants, .. }) => {
                variants.iter().any(|(_, params)| !params.is_empty())
            }
            _ => false,
        }
    }

    /// A standalone integer literal, already registered in `exprs`/`type_map`.
    fn int_slot(&mut self, value: i64) -> u32 {
        let id = self.next_id();
        self.exprs.insert(id, MirExpr::IntLit(value));
        self.type_map.insert(id, Type::I64);
        id
    }

    /// `dest = deref(addr_id)` — one i64 loaded through a heap block pointer.
    /// Materialized into a slot (not left as a bare expr) because arm bindings
    /// and `==` operands are read back as variables.
    fn deref_slot(&mut self, addr_id: u32, byte_offset: i64) -> u32 {
        let addr_expr_id = self.next_id();
        if byte_offset == 0 {
            self.exprs.insert(addr_expr_id, MirExpr::Var(addr_id));
        } else {
            let off_id = self.next_id();
            self.exprs.insert(off_id, MirExpr::IntLit(byte_offset));
            self.type_map.insert(off_id, Type::I64);
            self.exprs.insert(
                addr_expr_id,
                MirExpr::BinaryOp {
                    op: "+".to_string(),
                    left: addr_id,
                    right: off_id,
                },
            );
        }
        self.type_map.insert(addr_expr_id, Type::I64);
        let deref_id = self.next_id();
        self.exprs.insert(
            deref_id,
            MirExpr::Deref {
                addr_id: addr_expr_id,
                pointee_width: 8,
            },
        );
        self.type_map.insert(deref_id, Type::I64);
        let slot = self.next_id();
        self.stmts.push(MirStmt::Assign {
            lhs: slot,
            rhs: deref_id,
        });
        self.exprs.insert(slot, MirExpr::Var(slot));
        self.type_map.insert(slot, Type::I64);
        slot
    }

    /// `cond = (tag of a boxed enum value) == expected_tag`.
    fn boxed_tag_guard(&mut self, scrutinee_id: u32, tag: i64) -> u32 {
        let tag_slot = self.deref_slot(scrutinee_id, 0);
        let expect_id = self.next_id();
        self.exprs.insert(expect_id, MirExpr::IntLit(tag));
        self.type_map.insert(expect_id, Type::I64);
        let cond_id = self.next_id();
        self.stmts.push(MirStmt::Call {
            func: "==".to_string(),
            args: vec![tag_slot, expect_id],
            dest: cond_id,
            type_args: vec![],
        });
        self.exprs.insert(cond_id, MirExpr::Var(cond_id));
        self.type_map.insert(cond_id, Type::Bool);
        cond_id
    }

    /// Lower a range pattern as a match guard on `scrutinee_id`, returning a Bool
    /// condition id: `scrutinee >= start && scrutinee (<= | <) end`.
    ///
    /// Every intermediate id must be registered in `exprs`, not merely used as a
    /// `Call` dest — `gen_expr_safe` answers an unregistered id with `i64 0`
    /// (`codegen.rs:5606`), so the conjunction read `0 && 0` and no range arm
    /// could ever match: `match 5 { 1..=10 => 111, _ => 222 }` yielded 222 with
    /// zero diagnostics.
    fn lower_range_guard(
        &mut self,
        scrutinee_id: u32,
        start: &AstNode,
        end: &AstNode,
        inclusive: bool,
    ) -> u32 {
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
        self.exprs.insert(ge_id, MirExpr::Var(ge_id));
        self.type_map.insert(ge_id, Type::Bool);

        self.stmts.push(MirStmt::Call {
            func: if inclusive { "<=" } else { "<" }.to_string(),
            args: vec![scrutinee_id, end_id],
            dest: le_id,
            type_args: vec![],
        });
        self.exprs.insert(le_id, MirExpr::Var(le_id));
        self.type_map.insert(le_id, Type::Bool);

        let and_id = self.next_id();
        self.stmts.push(MirStmt::Call {
            func: "&&".to_string(),
            args: vec![ge_id, le_id],
            dest: and_id,
            type_args: vec![],
        });
        self.exprs.insert(and_id, MirExpr::Var(and_id));
        self.type_map.insert(and_id, Type::Bool);
        and_id
    }

    /// PY-A: `dest = norm_index(len, idx)` — Python subscript normalization for
    /// an index whose sign is only known at runtime (`idx < 0 ? len + idx : idx`).
    /// Codegen inlines it as a select, so there is no runtime symbol to bind and
    /// the AOT and in-process JIT paths share one implementation.
    fn emit_norm_index(&mut self, len_id: u32, idx_id: u32) -> u32 {
        let dest = self.next_id();
        self.stmts.push(MirStmt::Call {
            func: "norm_index".to_string(),
            args: vec![len_id, idx_id],
            dest,
            type_args: vec![],
        });
        self.exprs.insert(dest, MirExpr::Var(dest));
        self.type_map.insert(dest, Type::I64);
        dest
    }

    /// PY-A (任务 #53): the read and write lowering of a subscript share this, so
    /// one fix closes both sides. Returns the index id to use — unchanged when the
    /// index is a statically non-negative literal or the base is not an array,
    /// otherwise `norm_index(len, idx)` with `len` from `vec_len` (dynamic array)
    /// or the literal size (fixed array).
    fn normalize_subscript_index(
        &mut self,
        base_id: u32,
        base_ty: &Type,
        index: &AstNode,
        idx_id: u32,
    ) -> u32 {
        if matches!(index, AstNode::Lit(k) if *k >= 0) {
            return idx_id;
        }
        let len_id = match base_ty {
            Type::DynamicArray(_) => {
                let len_id = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: "vec_len".to_string(),
                    args: vec![base_id],
                    dest: len_id,
                    type_args: vec![],
                });
                self.exprs.insert(len_id, MirExpr::Var(len_id));
                self.type_map.insert(len_id, Type::I64);
                len_id
            }
            Type::Array(_, ArraySize::Literal(n)) => {
                let len_id = self.next_id();
                self.exprs.insert(len_id, MirExpr::IntLit(*n as i64));
                self.type_map.insert(len_id, Type::I64);
                len_id
            }
            _ => return idx_id,
        };
        self.emit_norm_index(len_id, idx_id)
    }

    /// Batch 431: remember that a `receiver.member(...)` call degraded to the
    /// bare symbol `member`. Lowering is the only layer that knows the call was
    /// written as a member access — by codegen the target string is identical to
    /// what an ordinary `member(...)` call emits, so the fact must be recorded
    /// here or it is gone. A call with no receiver is the ordinary spelling of
    /// the same string, recorded in `plain_call_names` so a per-name judgement
    /// can say which names have both.
    fn note_member_bare(&mut self, method: &str, has_receiver: bool) {
        if has_receiver {
            self.member_bare_calls.insert(method.to_string());
        } else {
            self.plain_call_names.insert(method.to_string());
        }
    }

    /// One closed exit for expression lowering. A route that returns `id`
    /// without registering it in `self.exprs` leaves a phantom slot, and
    /// codegen indexes `exprs` raw for `*`/`+` operands
    /// (`MirExpr::SemiringFold` — `codegen.rs:6715`, `:6735`, `:6765`) and for
    /// struct-literal fields: a phantom is a `no entry found for key` panic
    /// there, or a silent 0 anywhere else. `lower_expr_node` has dozens of
    /// exits and the ones that miss are all in the `PathCall` arm, whose routes
    /// are gated on heuristics about the *method*'s spelling (`is_upper`) and on
    /// hardcoded paths — so `HashMap::new()` and `std::env::var(..)` reached the
    /// end of the arm with neither. The invariant is therefore enforced at this
    /// single point rather than one route at a time.
    fn lower_expr(&mut self, expr: &AstNode) -> u32 {
        let id = self.lower_expr_node(expr);
        if !self.exprs.contains_key(&id) {
            let label = match expr {
                AstNode::PathCall { path, method, .. } if !path.is_empty() => {
                    format!("{}::{}()", path.join("::"), method)
                }
                AstNode::PathCall { method, .. } => format!("{}()", method),
                other => {
                    let d = format!("{:?}", other);
                    match d.split('(').next().and_then(|h| h.split(' ').next()) {
                        Some(kind) => format!("{} expression", kind),
                        None => "expression".to_string(),
                    }
                }
            };
            eprintln!(
                "warning: [W1010] `{}` has no lowering route — its slot reads 0, \
                 and a `*`/`+` operand used to abort codegen here instead.",
                label
            );
            self.exprs.insert(id, MirExpr::IntLit(0));
            self.type_map.insert(id, Type::I64);
        }
        id
    }

    fn lower_expr_node(&mut self, expr: &AstNode) -> u32 {
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
                    let dest = match self.name_to_id.get(name).copied() {
                        None => {
                            let new_id = self.next_id();
                            self.exprs.insert(new_id, MirExpr::Var(new_id));
                            let ty = self.type_map.get(&rhs_id).cloned().unwrap_or(Type::I64);
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
                self.lower_ast(&AstNode::Assign(lhs.clone(), rhs.clone()));
                let z = self.next_id();
                self.exprs.insert(z, MirExpr::IntLit(0));
                self.type_map.insert(z, Type::I64);
                return z;
            }
            AstNode::AssignOp { .. } => {
                // PY-A: `i += 1` in expression position (an assignment match
                // arm). Run it through the statement side, which owns the full
                // semantics (slot type refresh, field/subscript targets); the
                // expression itself has no value, so it reads back 0 like a
                // block that ends in an assignment.
                self.lower_ast(expr);
                let z = self.next_id();
                self.exprs.insert(z, MirExpr::IntLit(0));
                self.type_map.insert(z, Type::I64);
                return z;
            }
            AstNode::Var(name) => {
                // PY-A: `__file__` — the source path, known at compile time.
                // It is PER MODULE: reading the program-wide entry path made
                // `Path(__file__).parent…` arithmetic in an imported module
                // point at the ENTRY's ancestors (REasyQuant
                // `market_data_sources._PROJECT_ROOT` landed on
                // `/Users/meetai/source`, so every parquet cache miss).
                if name == "__file__" && !self.name_to_id.contains_key(name.as_str()) {
                    let own = self
                        .py_module_paths
                        .get(&self.current_module)
                        .cloned()
                        .or_else(|| self.source_file.clone());
                    if let Some(f) = own {
                        self.exprs.insert(id, MirExpr::StringLit(f));
                        self.type_map.insert(id, Type::Str);
                        return id;
                    }
                }
                // PY-A: a module's own top-level name reads its module-global
                // slot (`mod__NAME` in the env). Locals win, so only rewrite
                // when nothing local shadows it.
                // `__name__` — Python's module-name string. It had NO handling at
                // all (a bare `print(__name__)` printed 1), and
                // `logging.getLogger(__name__)` produced a bad pointer that crashed
                // `fprintf`/`strlen` inside the first log line of `run_backtest`.
                if name == "__name__" && !self.name_to_id.contains_key("__name__") {
                    let v = self.current_module.clone();
                    self.exprs.insert(id, MirExpr::StringLit(v));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
                if !self.name_to_id.contains_key(name.as_str()) {
                    if let Some(mangled) = self.symbol_renames.get(name.as_str()).cloned() {
                        if mangled != *name && self.module_globals.contains(&mangled) {
                            return self.lower_expr(&AstNode::Var(mangled));
                        }
                    }
                    // `from <module> import <VARIABLE>` — a member import read as
                    // a bare NAME (not a call). Function imports work because
                    // calls go through `py_member_call`; a variable had no path at
                    // all, so `D` became an uninitialized slot and read 0:
                    //   from regmod import D ; len(D)  →  0   (should be the dict)
                    //   from regmod import D, reg      →  SEGV (bad handle)
                    // The value lives in the module's env global `<mod>__<member>`
                    // (the loader stores every module-level binding there).
                    if let Some((module, member)) =
                        self.py_member_aliases.get(name.as_str()).cloned()
                    {
                        // Canonicalize (same file under two module names — see
                        // `py_member_call`), or the env key carries the wrong
                        // prefix and the read misses.
                        let module = self
                            .py_module_aliases
                            .get(&module)
                            .cloned()
                            .unwrap_or(module);
                        if self.py_user_modules.contains(&module) {
                            let mangled =
                                format!("{}__{}", module.replace('.', "_"), member);
                            if self.module_globals.contains(&mangled) {
                                return self.lower_expr(&AstNode::Var(mangled));
                            }
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
                    let ty = self.env_slot_ty(name);
                    self.type_map.insert(slot_id, ty);
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
                    let ty = self.global_ty_of(name).unwrap_or(Type::I64);
                    self.type_map.insert(slot_id, ty);
                    // A module's plain `def` never reaches the env, so a bare read
                    // of one holds 0 and `zeta_call1(0, x)` no-ops by contract. Take
                    // the symbol address instead, and leave the env read above alone:
                    // that keeps statement and slot allocation identical to before, so
                    // the only observable delta here is what the slot ends up holding.
                    if self.func_ret_types.contains_key(name)
                        && !self.global_consts.contains_key(name)
                        && !self.type_decls.contains_key(name)
                    {
                        self.exprs.insert(slot_id, MirExpr::FuncAddr(name.clone()));
                        self.type_map.insert(slot_id, Type::I64);
                    }
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
                    let ty = self.env_slot_ty(name);
                    self.type_map.insert(slot_id, ty);
                    return slot_id;
                }

                // Unit-variant path of a registered enum (e.g. `Color::Green`)
                // lowers to its variant tag (integer discriminant).
                //
                // This must be checked BEFORE the "bare function name as value"
                // path below: the resolver registers every enum variant as a
                // constructor signature, so `func_ret_types` contains
                // `Color::Green` and the name was lowered to
                // `FuncAddr("Color::Green")` — the address of a function that is
                // declared but never defined. Measured on `let c = Color::Green;`
                // used as a value: link fails with `Undefined symbols
                // "_Color__Green"`; when the slot is dead-code-eliminated the
                // scrutinee holds garbage and every `match` arm falls through to
                // `_` (`match c { Color::Red => 100, Color::Green => 200, _ => 300 }`
                // printed 300).
                if name.contains("::") {
                    if let Some(tag) = self.enum_unit_variant_index(name) {
                        // An enum that has ANY payload-carrying variant is boxed
                        // as a whole, so this unit variant's value has to be a
                        // `[tag]` block too — a bare integer discriminant could
                        // not be told apart from a block pointer by the arm test.
                        let boxed = self
                            .enum_variant_of(name)
                            .map(|(enum_name, _, _)| self.enum_is_boxed(&enum_name))
                            .unwrap_or(false);
                        if boxed {
                            let tag_id = self.next_id();
                            self.exprs.insert(tag_id, MirExpr::IntLit(tag));
                            self.type_map.insert(tag_id, Type::I64);
                            self.exprs.insert(
                                id,
                                MirExpr::Struct {
                                    variant: name.clone(),
                                    fields: vec![("__tag".to_string(), tag_id)],
                                },
                            );
                        } else {
                            self.exprs.insert(id, MirExpr::IntLit(tag));
                        }
                        self.type_map.insert(id, Type::I64);
                        return id;
                    }
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
                    // An INLINE CONDITIONAL whose branches are both strings
                    // (`f"{'sh' if exch == 'XSHG' else 'sz'}.{num}"`) left the
                    // part typed i64, so `lower_to_string` stringified the raw
                    // handle: `jq_to_bs("510300.XSHG")` returned
                    // `4339988730.510300` instead of `sh.510300` — i.e. EVERY
                    // code in the wufu universe came out numerically garbage.
                    if matches!(self.type_map.get(&pid), Some(Type::I64) | Some(Type::PyDynamic)) {
                        if Self::both_branches_are_strings(p) {
                            self.type_map.insert(pid, Type::Str);
                        }
                    }
                    let pid = self.lower_to_string(pid);
                    part_ids.push(pid);
                }
                self.exprs.insert(id, MirExpr::FString(part_ids));
                self.type_map.insert(id, Type::Str);
            }
            AstNode::BinaryOp { op, left, right } => {
                // `x in ("sh", "sz")` — a TUPLE literal on the right. Tuples are
                // `StackArray`s with a `[len|elems]` layout, which the membership
                // path reads as a dynamic array ⇒ every membership test against a
                // tuple answered 0 (`"sz" in ("sh","sz")` → 0, while the same value
                // in a LIST worked). `code_conv.normalize_to_jq` is built on such
                // tuples, so `sh.513120` never normalized and every wufu code
                // failed to resolve to a cache file.
                if matches!(op.as_str(), "in" | "not in") {
                    if let AstNode::Tuple(items) = &**right {
                        let rewritten = AstNode::BinaryOp {
                            op: op.clone(),
                            left: left.clone(),
                            right: Box::new(AstNode::ArrayLit(items.clone())),
                        };
                        return self.lower_expr(&rewritten);
                    }
                }
                // PY-A: `and`/`or` must SHORT-CIRCUIT. The old path lowered
                // BOTH sides eagerly and let codegen `select` between the two
                // values — so `df is not None and len(df) > 0` ran `len(df)`
                // even when df was the None handle (crash measured in
                // `MarketDataFetcher::fetch_stocks`, drvE/drvF SIGBUS). The
                // value-select typing below is preserved: dest takes the whole
                // result via a nested If whose branches splice the deferred
                // lowering (same stmt-buffer swap the if-expression uses).
                if matches!(op.as_str(), "&&" | "||") {
                    let left_id = self.lower_expr(left);
                    let dest = self.next_id();
                    let zero_id = self.next_id_with_lit(0);
                    let cond_id = self.next_id();
                    // BATCH-297: WHICH side to take is a truthiness question,
                    // and `!= 0` is not the same answer for a container: an
                    // empty list/dict/string handle is non-zero, so
                    // `ranked = getattr(g, "ranked_etfs_result", []) or []`
                    // kept the empty side and `pool or fixed_pool` never fell
                    // through. `zeta_dyn_truth` asks the value (GC geometry →
                    // length, anything else → `!= 0`), so an int answers the
                    // same as before. Floats keep their own rule (bits != 0).
                    let left_ty = self.type_map.get(&left_id).cloned();
                    // A USER-CLASS instance is always truthy in Python (our
                    // dataclasses define no `__bool__`/`__len__`) and it lives in
                    // an i64 slot as a heap pointer, so `!= 0` is the whole
                    // question. `zeta_dyn_truth` instead probes the object's first
                    // word as a length: `self._cost = cost or CostModel()` read
                    // `slippage` (0.0 bits) as "empty", silently swapped in a
                    // default model, and every `fee()` became 0 — the ledger's
                    // cash barely moved.
                    let is_object = matches!(
                        &left_ty,
                        Some(Type::Named(n, _))
                            if !matches!(
                                n.as_str(),
                                "map" | "dict" | "set" | "frozenset" | "PyJson" | "str"
                            )
                    );
                    let truth_id =
                        if matches!(left_ty, Some(Type::F64) | Some(Type::Bool)) || is_object {
                            left_id
                        } else {
                        // A JSON cell needs its own reader (tag + payload), the
                        // geometry probes cannot see through the tag word.
                        let func = if matches!(&left_ty, Some(Type::Named(n, _)) if n == "PyJson") {
                            "py_json_truth"
                        } else {
                            "zeta_dyn_truth"
                        };
                        let tid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: func.to_string(),
                            args: vec![left_id],
                            dest: tid,
                            type_args: vec![],
                        });
                        self.exprs.insert(tid, MirExpr::Var(tid));
                        self.type_map.insert(tid, Type::I64);
                        tid
                    };
                    self.exprs.insert(
                        cond_id,
                        MirExpr::BinaryOp {
                            op: "!=".to_string(),
                            left: truth_id,
                            right: zero_id,
                        },
                    );
                    self.type_map.insert(cond_id, Type::Bool);

                    let saved_stmts = std::mem::take(&mut self.stmts);
                    let right_id = self.lower_expr(right);
                    let mut right_stmts = std::mem::take(&mut self.stmts);
                    self.stmts = saved_stmts;

                    let left_then = vec![MirStmt::Assign { lhs: dest, rhs: left_id }];
                    let right_then_assign = {
                        right_stmts.push(MirStmt::Assign { lhs: dest, rhs: right_id });
                        right_stmts
                    };
                    let (then_b, else_b) = if op == "&&" {
                        // a and b: falsy a selects a; truthy a evaluates b.
                        (right_then_assign, left_then)
                    } else {
                        (left_then, right_then_assign)
                    };
                    // Value semantics typing (batch 152, kept through the
                    // short-circuit rewrite): only adopt a CONCRETE operand
                    // type — an untyped param (PyDynamic/None/I64/Bool) must
                    // not poison the dest, or `cfg = m or {}` would type cfg
                    // PyDynamic and `cfg.get` would dispatch to `_get`.
                    let lt = self.type_map.get(&left_id).cloned();
                    let rt = self.type_map.get(&right_id).cloned();
                    let concrete = |t: &Option<Type>| match t {
                        Some(Type::I64) | Some(Type::PyDynamic) | Some(Type::Bool) | None => None,
                        other => other.clone(),
                    };
                    let dest_ty = match (concrete(&lt), concrete(&rt)) {
                        (Some(a), Some(b)) if a == b => a,
                        (Some(a), _) => a,
                        (_, Some(b)) => b,
                        _ => match (lt, rt) {
                            (Some(a), Some(b)) if a == b => a,
                            _ => Type::I64,
                        },
                    };
                    self.type_map.insert(dest, dest_ty);
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.stmts.push(MirStmt::If {
                        cond: cond_id,
                        then: then_b,
                        else_: else_b,
                        dest: Some(dest),
                    });
                    return dest;
                }
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
                        // The codegen's call-arg path loads an operand from its
                        // LOCAL slot; a FieldAccess / call result has no alloca, so
                        // `self.cache_dir / "x"` (or `_PROJECT_ROOT / "data"` where
                        // the global came through a field) loaded NULL and
                        // `py_os_path_join` dereferenced it — measured SEGV at
                        // `MarketDataFetcher.__init__+68`. Materialize both
                        // operands into fresh slots first (same rule the map
                        // subscript path already follows).
                        let left_id = self.materialize_for_call(left_id);
                        let right_id = self.materialize_for_call(right_id);
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
                    && self.is_array_like(&left_id)
                    && self.is_array_like(&right_id)
                {
                    // PY-A: list equality. `a == b` on two lists used to compile
                    // to a plain integer compare of the two HANDLES, so two
                    // equal-content lists always compared 0 — a silent wrong
                    // answer in every `if a == b` guard.
                    // Batch 398: the guard used to be `||`, which also swallowed
                    // `column == scalar` — exactly the shape the element-wise
                    // mask path below exists for. Measured: `e = df["code"] ==
                    // "d"; len(df[e])` printed 0 where pandas gives 1, because
                    // `py_list_eq` compared a vector against a bare string
                    // pointer (`zt_vec_len` on the scalar) and handed back a
                    // Bool, not a mask. List-vs-list stays here; vector-vs-
                    // scalar falls through to `py_vec_cmp_str`/`py_vec_*`.
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
                } else if op == "|"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::Named(n, _)) if n == "set"
                    )
                    || (op == "|"
                        && (matches!(
                            self.type_map.get(&left_id),
                            Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                        ) || matches!(
                            self.type_map.get(&right_id),
                            Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                        )))
                {
                    // Python sets DEGRADE TO LISTS here, so `s |= {...}` is a
                    // UNION. Without this the bitwise-or ran on two HANDLES and
                    // produced garbage: the set stayed empty, `c not in
                    // fetched_codes` was always true, and the garbage handle
                    // afterwards crashed `py_list_contains` (measured in
                    // `fetch_stocks`). Approximated by concatenation — membership
                    // stays correct, duplicates survive (dedup needs runtime
                    // content comparison; tracked in the roadmap).
                    // Prefer whichever side carries a REAL element type: `set()`
                    // reports I64 = "unknown", and taking that side made
                    // `c |= {"q"}` a DynamicArray(I64) — so `"q" in c` compared
                    // HANDLES afterwards and every string counted as absent.
                    let elem_of = |t: Option<Type>| match t {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => Some(*e),
                        _ => None,
                    };
                    // A boolean/label mask PAIR must be OR-ed element-wise, not
                    // concatenated: `isna(col) | (col < x)` produced 2N elements and
                    // `df.loc[...]` kept 2N rows.
                    // Either side being a VECTOR is enough: Python's `|` on two
                    // Series is element-wise OR (only `+` concatenates), and
                    // `isna(col)` is typed `DynamicArray(I64)`, not Bool — so the
                    // `|` used to fall through to the concat path and the mask came
                    // out 2N long (`vec_not` saw n=1784 for 892 rows).
                    let vecish = |t: Option<Type>| {
                        matches!(t, Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)))
                    };
                    let boolish = |t: Option<Type>| matches!(t, Some(Type::Bool));
                    // Batch 398: a mask keeps its Bool element through `|`. The
                    // `I64` element is this compiler's "unknown element" marker
                    // (see the `vec_push` refinement below `lower_call`), so an
                    // `I64` result here was indistinguishable from an index list
                    // at `df[...]`.
                    let maskish = |t: Option<Type>| match t {
                        Some(Type::Bool) => true,
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            matches!(*e, Type::Bool)
                        }
                        _ => false,
                    };
                    if vecish(self.type_map.get(&left_id).cloned())
                        || vecish(self.type_map.get(&right_id).cloned())
                        || boolish(self.type_map.get(&left_id).cloned())
                        || boolish(self.type_map.get(&right_id).cloned())
                    {
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_or".to_string(),
                            args: vec![left_id, right_id],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        let elem = if maskish(self.type_map.get(&left_id).cloned())
                            || maskish(self.type_map.get(&right_id).cloned())
                        {
                            Type::Bool
                        } else {
                            Type::I64
                        };
                        self.type_map
                            .insert(dest, Type::DynamicArray(Box::new(elem)));
                        return dest;
                    }
                    let elem = match (
                        elem_of(self.type_map.get(&left_id).cloned()),
                        elem_of(self.type_map.get(&right_id).cloned()),
                    ) {
                        (Some(l), Some(r)) => {
                            if matches!(l, Type::I64) {
                                r
                            } else {
                                l
                            }
                        }
                        (Some(l), None) => l,
                        (None, Some(r)) => r,
                        (None, None) => Type::I64,
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
                } else if op == "|"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::Named(n, _)) if n == "map" || n == "dict" || n == "set"
                    )
                    && matches!(
                        self.type_map.get(&right_id),
                        Some(Type::Named(n, _)) if n == "map" || n == "dict" || n == "set"
                    )
                {
                    // `s |= {...}` / `d |= {...}` desugar to `x = x | y`. Without
                    // this the bitwise-or ran on the two HANDLES, so the set stayed
                    // EMPTY: `c not in fetched_codes` was always true (measured in
                    // `fetch_stocks`, which then carried every code forward and
                    // finally crashed inside `py_list_contains` on the set handle).
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_map_update".to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    let lt = self.type_map.get(&left_id).cloned().unwrap_or(Type::I64);
                    self.type_map.insert(dest, lt);
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
                        Some(Type::I64)
                            | Some(Type::I32)
                            | Some(Type::U32)
                            | Some(Type::U64)
                            | Some(Type::Usize)
                    )
                    && matches!(self.type_map.get(&right_id), Some(Type::Str))
                {
                    // The mirror of the arm above: `40 * "-"`. Python's `*` is
                    // commutative on `str`, and without this side the numeric
                    // multiply ran on the pointer and printed an address.
                    self.stmts.push(MirStmt::Call {
                        func: "host_str_repeat".to_string(),
                        args: vec![right_id, left_id],
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
                } else if op == "*"
                    && matches!(
                        self.type_map.get(&left_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    )
                    && matches!(
                        self.type_map.get(&right_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    )
                {
                    // Two VECTORS: pandas' `df["close"] * df["volume"]` is an
                    // ELEMENT-WISE product. The SemiringFold (matmul) path below
                    // walked the STRING elements as arrays and crashed in
                    // `array_len` (measured: `validate_and_repair_stock_ohlcv`'s
                    // `amount` recompute, `probe + 3316`).
                    self.stmts.push(MirStmt::Call {
                        func: "py_vec_mul".to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(Type::Str)));
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
                } else if matches!(op.as_str(), ">" | "<" | ">=" | "<=" | "==" | "!=") && {
                    let is_arr = |t: Option<Type>| {
                        matches!(t, Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)))
                    };
                    // Batch 398: `x is None` arrives here as `x == 0` — the parser
                    // erases `is`/`is not` into `==`/`!=` (`parser/expr.rs:2491-2495`)
                    // and `None` into `Lit(0)` (`:1467`), so an IDENTITY test on a
                    // list-typed value is textually indistinguishable from
                    // `column == 0`. Measured: t150's `if types is None` took the
                    // mask path (a non-empty handle is truthy) and printed `2`.
                    // Only `==`/`!=` against a zero literal is excluded — `>`/`<`
                    // can never be an identity test. The excluded shape falls to the
                    // generic `==` below, which compares the two HANDLES, i.e. the
                    // identity answer. Root fix (keep `is` as its own op) registered.
                    let identity_sentinel = matches!(op.as_str(), "==" | "!=")
                        && [left_id, right_id]
                            .iter()
                            .any(|id| matches!(self.exprs.get(id), Some(MirExpr::IntLit(0))));
                    !identity_sentinel
                        && (is_arr(self.type_map.get(&left_id).cloned())
                            ^ is_arr(self.type_map.get(&right_id).cloned()))
                } {
                    // `column > scalar` — ELEMENT-WISE comparison producing a 0/1
                    // mask. Without this the result was typed Bool, so the mask
                    // was used as a vector: `len(mask)` read `mask-16` on a small
                    // integer and `DataFrame.loc` aborted ("mask is missing").
                    // Measured in `remove_extreme_return_bars`
                    // (`mask = ret > max_abs_daily_return`).
                    let arr_side = |t: Option<Type>| {
                        matches!(t, Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)))
                    };
                    let left_is_arr = arr_side(self.type_map.get(&left_id).cloned());
                    let (vec_id, num_id) = if left_is_arr {
                        (left_id, right_id)
                    } else {
                        (right_id, left_id)
                    };
                    let base = match op.as_str() {
                        ">" => "py_vec_gt",
                        "<" => "py_vec_lt",
                        ">=" => "py_vec_ge",
                        "<=" => "py_vec_le",
                        "==" => "py_vec_eq",
                        _ => "py_vec_ne",
                    };
                    let num_is_float = matches!(
                        self.type_map.get(&num_id),
                        Some(Type::F32) | Some(Type::F64)
                    ) || matches!(self.exprs.get(&num_id), Some(MirExpr::FloatLit(_)));
                    // A float literal's type_map entry is not always F64, so
                    // `ret > 0.2` used to pick the `_i` variant and pass a
                    // truncated 0 (`sum` was 891/892 instead of a small count,
                    // and an all-true mask then aborted in `DataFrame.loc`).
                    // A float LITERAL must travel as its bit pattern: the codegen
                    // puts literals in integer registers, so a `double` parameter
                    // arrived as 0 (`ret > 0.2` behaved like `ret > 0`; measured
                    // `sum == 891/892` and an all-true mask).
                    let lit_bits = match self.exprs.get(&num_id) {
                        Some(MirExpr::FloatLit(v)) if num_is_float => {
                            Some(v.to_bits() as i64)
                        }
                        _ => None,
                    };
                    let num_arg = match lit_bits {
                        Some(bits) => self.next_id_with_lit(bits),
                        None => num_id,
                    };
                    // A STRING vector (our "YYYY-MM-DD" dates) must compare with
                    // strcmp: the numeric variants parsed the dates with strtod
                    // and `col >= "2023-08-05"` answered 0 for every row, making
                    // the cache-coverage check see an empty slice.
                    let elem_is_str = matches!(
                        self.type_map.get(&vec_id),
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) if matches!(**e, Type::Str)
                    ) && matches!(self.type_map.get(&num_id), Some(Type::Str));
                    let kind = match op.as_str() {
                        ">" => 0i64,
                        "<" => 1,
                        ">=" => 2,
                        "<=" => 3,
                        "==" => 4,
                        _ => 5,
                    };
                    // `trade_date >= pd.Timestamp("2023-08-05")` — the column is
                    // a vector of "YYYY-MM-DD" STRINGS (the parquet reader
                    // formats INT64 dates that way) while the scalar is a PyDate
                    // HANDLE. The numeric variants passed the raw pointer as
                    // rhs (`strtod("2022-05-05")=2022 >= 4.3e9` → all-false
                    // mask), so `partial` in `fetch_stocks` was always EMPTY,
                    // nothing was ever appended to `all_data`, and the empty
                    // list's pointer-truthiness then fed `concat([])` a NULL
                    // first frame (SIGSEGV in `column_names`). Render the
                    // handle as its date string and strcmp instead —
                    // "YYYY-MM-DD" sorts chronologically.
                    let num_is_dt = matches!(
                        self.type_map.get(&num_id),
                        Some(Type::Named(n, _)) if n == "PyDate"
                    );
                    if num_is_dt {
                        let fmt_id = self.next_id();
                        self.exprs
                            .insert(fmt_id, MirExpr::StringLit("%Y-%m-%d".to_string()));
                        self.type_map.insert(fmt_id, Type::Str);
                        let ts_id = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "py_dt_strftime".to_string(),
                            args: vec![num_id, fmt_id],
                            dest: ts_id,
                            type_args: vec![],
                        });
                        self.exprs.insert(ts_id, MirExpr::Var(ts_id));
                        self.type_map.insert(ts_id, Type::Str);
                        let kid = self.next_id_with_lit(kind);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_cmp_str".to_string(),
                            args: vec![vec_id, ts_id, kid],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map
                            .insert(dest, Type::DynamicArray(Box::new(Type::Bool)));
                        return dest;
                    }
                    let func = if elem_is_str {
                        let kid = self.next_id_with_lit(kind);
                        let kid2 = self.next_id_with_lit(kind);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_cmp_str".to_string(),
                            args: vec![vec_id, num_arg, kid],
                            dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        self.type_map
                            .insert(dest, Type::DynamicArray(Box::new(Type::Bool)));
                        let _ = kid2;
                        return dest;
                    } else if lit_bits.is_some() {
                        format!("{}_bits", base)
                    } else if num_is_float {
                        base.to_string()
                    } else {
                        format!("{}_i", base)
                    };
                    self.stmts.push(MirStmt::Call {
                        func,
                        args: vec![vec_id, num_arg],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(Type::Bool)));
                } else if matches!(op.as_str(), "&" | "|") && {
                    let is_arr_mask = |t: Option<Type>| {
                        matches!(t, Some(Type::DynamicArray(_)) | Some(Type::Array(_, _)))
                    };
                    is_arr_mask(self.type_map.get(&left_id).cloned())
                        || is_arr_mask(self.type_map.get(&right_id).cloned())
                } {
                    // `(col >= x) & (col <= y)` — element-wise mask AND/OR.
                    // Falling through to the generic path emitted a scalar
                    // LLVM `and` of the two vector HANDLES (garbage pointer
                    // that passed `py_is_vec`, then SIGSEGV in `sum`/`loc`;
                    // measured in `fetch_stocks`' cache-window filter).
                    let func = if op == "&" { "py_vec_and" } else { "py_vec_or" };
                    self.stmts.push(MirStmt::Call {
                        func: func.to_string(),
                        args: vec![left_id, right_id],
                        dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    // Batch 398: the mask element survives the AND/OR — a
                    // `DynamicArray(Bool)` is what `df[...]` reads to tell a row
                    // filter from an index list. An operand that only arrived as
                    // `lt(vec, i64)` (the shim's `loc`/`iloc` annotation) keeps the
                    // legacy element.
                    let maskish = |t: Option<Type>| match t {
                        Some(Type::Bool) => true,
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            matches!(*e, Type::Bool)
                        }
                        _ => false,
                    };
                    let elem = if maskish(self.type_map.get(&left_id).cloned())
                        || maskish(self.type_map.get(&right_id).cloned())
                    {
                        Type::Bool
                    } else {
                        Type::I64
                    };
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(elem)));
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
                        // BATCH-455 (#203②): a COLUMN operand makes the answer
                        // another column, never a double. The float arm below typed
                        // `100.0 / df["open"]` F64, so the runtime handle was stored
                        // in a double slot and `DataFrame::__setitem__` was called
                        // with `fptosi(handle)` — the new column read back `<null>`.
                        // I64 is the handle representation codegen already uses for
                        // the column-first spelling (`df["close"] / 2`).
                        // The op list must stay in sync with
                        // `Codegen::column_arith_dispatch` (codegen.rs): a route that
                        // fires without this arm re-creates exactly this bug.
                        _ if !is_cmp
                            && matches!(
                                op.as_str(),
                                "/" | "div" | "floordiv" | "%" | "mod" | "-" | "sub"
                            )
                            && (matches!(
                                self.type_map.get(&left_id),
                                Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                            ) || matches!(
                                self.type_map.get(&right_id),
                                Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                            )) =>
                        {
                            Type::I64
                        }
                        (Some(Type::F32) | Some(Type::F64), _)
                        | (_, Some(Type::F32) | Some(Type::F64)) => {
                            if is_cmp {
                                Type::Bool
                            } else {
                                Type::F64
                            }
                        }
                        // Batch 291: an INTEGER comparison is still a Bool.
                        // `1 == 1` / `i < n` fell to the I64 fallback, so
                        // `print(1 == 1)` printed `1` and any `x = (a == b)`
                        // slot was an i64 — print/println dispatch never
                        // reached print_bool. Python: every comparison yields
                        // a bool; the runtime rep is i64 either way, only the
                        // inferred TYPE drives printing and later `type()`.
                        // NB: `&&`/`||` are NOT refinement targets here — they are
                        // value-selecting in Python and get their own op_type pass
                        // right below; typing them Bool here broke 38 tests.
                        _ if is_cmp && !matches!(op.as_str(), "||" | "&&") => Type::Bool,
                        // PY-A: `/` on two integers is TRUE division. The I64
                        // fallback below typed the quotient as an integer and the
                        // codegen ran `sdiv`, so `round(66 / 10)` answered 6
                        // instead of 7. Only both-integral operands are promoted:
                        // a Str/Named/Dynamic side keeps its previous answer.
                        // `//` never reaches here (the parser emits "floordiv").
                        (Some(Type::I64) | Some(Type::Bool), Some(Type::I64) | Some(Type::Bool))
                            if op == "/" =>
                        {
                            Type::F64
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

                // The destination's type must come from the BRANCHES: typing it
                // I64 unconditionally made `prefix = "sh" if c else "sz"` hold a
                // string handle in an I64 slot — `len(prefix)` then did
                // `array_len(handle)` and produced garbage, `f"{prefix}.{left}"`
                // printed `<address>.<code>`, and the whole wufu universe came out
                // as `4296191491.513180`. That is why `get_universe("wufu")`
                // returned junk codes and `_load_cache` failed for all of them.
                let ast_branch_ty: Option<Type> = {
                    let ast_ty = |n: &AstNode| -> Option<Type> {
                        match n {
                            AstNode::StringLit(_) | AstNode::FString { .. } => Some(Type::Str),
                            AstNode::FloatLit(_) => Some(Type::F64),
                            AstNode::Bool(_) => Some(Type::Bool),
                            AstNode::Lit(_) => Some(Type::I64),
                            // A comprehension that keeps its element as-is
                            // (`[d for d in days if cond]`) has a VAR tail —
                            // resolve it from the enclosing type map (batch
                            // 293: the trading-day filter typed I64, so the
                            // driver's date window matched 0 days).
                            AstNode::Var(name) => self
                                .name_to_id
                                .get(name.as_str())
                                .and_then(|i| self.type_map.get(i))
                                .cloned(),
                            _ => None,
                        }
                    };
                    let tail_of = |blk: &[AstNode]| -> Option<Type> {
                        // The ternary's arms arrive as BLOCKS (`Block { body }`),
                        // so unwrap down to the last real statement.
                        let mut tail = blk.last();
                        while let Some(AstNode::Block { body }) = tail {
                            tail = body.last();
                        }
                        match tail {
                            Some(AstNode::ExprStmt { expr }) | Some(AstNode::Return(expr)) => {
                                ast_ty(expr)
                            }
                            Some(AstNode::Assign(_, rhs)) => ast_ty(rhs),
                            _ => None,
                        }
                    };
                    let t = tail_of(then);
                    let e = tail_of(else_);
                    match (t, e) {
                        (Some(a), Some(b)) if a == b => Some(a),
                        // Mixed arms: a filtered comprehension is
                        // `if cond { ELEMENT } else { -1 }` — the i64 sentinel
                        // must not drag the result type to I64 (batch 293:
                        // this made every str comprehension a DynamicArray
                        // (I64), and the driver's trading-day filter then
                        // pointer-compared to 0 days).
                        (Some(Type::I64), Some(b)) => Some(b),
                        (Some(a), Some(Type::I64)) => Some(a),
                        (Some(a), None) => Some(a),
                        (None, Some(b)) => Some(b),
                        _ => None,
                    }
                };
                // Create destination for expression result
                self.exprs.insert(dest_id, MirExpr::Var(dest_id));
                self.type_map
                    .insert(dest_id, ast_branch_ty.clone().unwrap_or(Type::I64));

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
                // The AST-level inference is AUTHORITATIVE and must win: the
                // statement-level pass above derives I64 for
                // `s = "sh" if c else "sz"` (both arms are plain Assigns), which
                // made `len(s)` emit `array_len` on a string handle and
                // `f"{prefix}.{code}"` print `<address>.<code>` for every code in
                // the wufu universe (measured: `4296191491.513180`).
                if let Some(ast_ty) = ast_branch_ty {
                    self.type_map.insert(dest_id, ast_ty);
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
                // `pd.DataFrame(columns=[...])` / `pd.DataFrame(index=…, dtype=…)`
                // — the KWARG-ONLY schema constructors. The kwarg NAME is dropped
                // by the generic path, so the VALUE list was passed as `data`: the
                // shim's ctor stored a Vec where a column map belongs and
                // `len(df)` SEGV'd (measured). Build the map the ctor wants:
                // each named column becomes an EMPTY column; a schema-less frame
                // becomes an empty map. 10 `columns=` + 4 `index=/dtype=` sites in
                // REasyQuant's data/ML layer hit this.
                if method == "DataFrame"
                    && !args.is_empty()
                    && args.iter().all(|a| {
                        matches!(a, AstNode::Call { method: km, args: ka, .. }
                            if km == "__kwarg__" && ka.len() == 2)
                    })
                {
                    let is_pd = match receiver.as_deref() {
                        Some(AstNode::Var(v)) => {
                            self.py_module_aliases.get(v).map_or(false, |m| m == "pandas")
                        }
                        None => self
                            .py_member_target(&None, "DataFrame")
                            .map_or(false, |(m, mem)| m == "pandas" && mem == "DataFrame"),
                        _ => false,
                    };
                    if is_pd {
                        let mut columns: Option<AstNode> = None;
                        for a in args {
                            if let AstNode::Call { method: km, args: ka, .. } = a {
                                if km == "__kwarg__" && ka.len() == 2 {
                                    if let AstNode::StringLit(n) = &ka[0] {
                                        if n == "columns" {
                                            columns = Some(ka[1].clone());
                                        }
                                    }
                                }
                            }
                        }
                        // A LITERAL column list is built as a dict literal of
                        // `name: []` in MIR: a literal list lowers to a
                        // StackArray (no `[cap|len]` header), so the runtime
                        // helper below would read its length as 0 and produce an
                        // EMPTY frame (measured: `len(df.columns)` was 0, not 2).
                        // Non-literal lists (`list(fields)`, `df.columns`) are real
                        // DynamicArrays and go through the helper.
                        let data = match columns {
                            Some(AstNode::ArrayLit(items)) => AstNode::DictLit {
                                entries: items
                                    .iter()
                                    .map(|it| {
                                        (
                                            it.clone(),
                                            AstNode::DynamicArrayLit {
                                                elem_type: "str".to_string(),
                                                elements: vec![],
                                            },
                                        )
                                    })
                                    .collect(),
                            },
                            Some(expr) => AstNode::Call {
                                receiver: None,
                                method: "zeta_df_with_columns".to_string(),
                                args: vec![expr],
                                type_args: vec![],
                                structural: false,
                            },
                            None => AstNode::DictLit { entries: vec![] },
                        };
                        // Re-enter the ORDINARY path with a positional data
                        // argument: it is the one that resolves the ctor symbol
                        // (`pandas__DataFrame`) AND types the result
                        // (`Named("DataFrame")`, which the later `.columns` /
                        // `len()` dispatch needs). Emitting a bare free call here
                        // left the result I64, so `e.columns` became a struct
                        // field read + `array_len` → 0 (measured).
                        return self.lower_expr(&AstNode::Call {
                            receiver: receiver.clone(),
                            method: "DataFrame".to_string(),
                            args: vec![data],
                            type_args: type_args.clone(),
                            structural: false,
                        });
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
                        // BATCH-296: a value the compiler could not type (the
                        // element of a `dict[str, Any]`, a dyn parameter). The
                        // old `array_len` fallback read the PRECEDING GC block
                        // and answered 0 — `codes=0` in the local backtest while
                        // `_build_stock_arrays` really returned 91 entries. Let
                        // the runtime tell map / vec / text apart by geometry.
                        None | Some(Type::I64) | Some(Type::PyDynamic) => {
                            self.stmts.push(MirStmt::Call {
                                func: "zeta_dyn_len".to_string(),
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
                if method == "sum" && crate::diagnostics::env_flag("ZETA_PROBE") {
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
                        self.stmts.push(MirStmt::Call {
                            func: "py_getattr_dynamic".to_string(),
                            args: vec![a0, a1],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
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
                                self.type_map.insert(vid, vt.clone().unwrap_or(Type::I64));
                                let tid = self.next_id();
                                self.exprs.insert(tid, MirExpr::IntLit(t));
                                self.type_map.insert(tid, Type::I64);
                                self.stmts.push(MirStmt::Call {
                                    func: "py_json_is_kind".to_string(),
                                    args: vec![vid, tid],
                                    dest: id,
                                    type_args: vec![],
                                });
                                self.exprs.insert(id, MirExpr::Var(id));
                                self.type_map.insert(id, Type::Bool);
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
                        // A LITERAL key list is built inline as a dict literal:
                        // a literal list lowers to a StackArray (no `[cap|len]`
                        // header), so the runtime helper read a garbage length and
                        // walked off the buffer — measured as a SEGV inside
                        // `py_map_fromkeys` during `wufu_constants`' module init.
                        if let AstNode::ArrayLit(items) = &args[0] {
                            let val_expr = match args.get(1) {
                                Some(v) => v.clone(),
                                None => AstNode::Lit(0),
                            };
                            return self.lower_expr(&AstNode::DictLit {
                                entries: items
                                    .iter()
                                    .map(|k| (k.clone(), val_expr.clone()))
                                    .collect(),
                            });
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
                                let sorted_ty = match self.type_map.get(&xs).cloned() {
                                    Some(Type::DynamicArray(e)) => Type::DynamicArray(e),
                                    Some(Type::Array(e, _)) => Type::DynamicArray(e),
                                    _ => Type::DynamicArray(Box::new(Type::I64)),
                                };
                                self.type_map.insert(nid, sorted_ty.clone());
                                self.exprs.insert(id, MirExpr::Var(nid));
                                self.type_map.insert(id, sorted_ty);
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
                                let sorted_ty = match self.type_map.get(&a).cloned() {
                                    Some(Type::DynamicArray(e)) => Type::DynamicArray(e),
                                    Some(Type::Array(e, _)) => Type::DynamicArray(e),
                                    _ => Type::DynamicArray(Box::new(Type::I64)),
                                };
                                self.type_map.insert(nid, sorted_ty.clone());
                                self.exprs.insert(id, MirExpr::Var(nid));
                                self.type_map.insert(id, sorted_ty);
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
                                    let kty = targs.first().cloned().unwrap_or(Type::I64);
                                    self.type_map
                                        .insert(nid, Type::DynamicArray(Box::new(kty.clone())));
                                    self.exprs.insert(id, MirExpr::Var(nid));
                                    self.type_map
                                        .insert(id, Type::DynamicArray(Box::new(kty)));
                                    return nid;
                                }
                            }
                            self.exprs.insert(id, MirExpr::Var(src));
                            if let Some(t) = self.type_map.get(&src).cloned() {
                                self.type_map.insert(id, t);
                            } else {
                                self.type_map.insert(id, Type::I64);
                            }
                            return id;
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
                        let func = match method.as_str() {
                            "sorted" if src_elem_str => "zeta_sorted_vec_len_str",
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
                            id,
                            match method.as_str() {
                                "sorted" => sorted_ty,
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
                    // BATCH-296: `str(<already a str>)` is the identity, but the
                    // identity was lowered as a bare ALIAS (`Var(arg)`) with no
                    // defining stmt — at module level nothing ever stored the
                    // dest slot, so `x = str("ab")` printed from an uninitialized
                    // alloca and `puts(0)` SEGFAULTed (measured: IR had
                    // `%13 = load i64, ptr %0` with no preceding store). Route
                    // through the identity helper so the slot is really written.
                    let nid = if matches!(self.type_map.get(&arg_id), Some(Type::Str)) {
                        let sid = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "zeta_identity".to_string(),
                            args: vec![arg_id],
                            dest: sid,
                            type_args: vec![],
                        });
                        self.exprs.insert(sid, MirExpr::Var(sid));
                        self.type_map.insert(sid, Type::Str);
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
                            // Batch 291: bool must print Python-style (True/False),
                            // not as the i64 fallback (1/0). print_bool exists in the
                            // runtime since the beginning but was never selected —
                            // `print(1 == 1)` printed `1`. No println_bool: the newline
                            // comes from the `print_str("\n")` emitted below for is_last.
                            Some(Type::Bool) => "print_bool",
                            _ => {
                                if is_last { "println_i64" } else { "print_i64" }
                            }
                        };
                        self.stmts.push(MirStmt::VoidCall {
                            func: func.to_string(),
                            args: vec![printed_id],
                        });
                        if matches!(self.type_map.get(&printed_id), Some(Type::Bool)) && is_last {
                            let nl = self.next_id();
                            self.exprs.insert(nl, MirExpr::StringLit("\n".to_string()));
                            self.type_map.insert(nl, Type::Str);
                            self.stmts.push(MirStmt::VoidCall {
                                func: "print_str".to_string(),
                                args: vec![nl],
                            });
                        }
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
                                fill(&mut slots, &params, pos, kw, &[]);
                                Self::warn_unbound(&callee_name, &params, &mut slots);
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
                                fill(&mut slots, &params, pos, kw, &[]);
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
                                fill(&mut slots, &params, pos, kw, &[]);
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
                                    self.pending_closure_param_types = Some(vec![elem]);
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
                if matches!(method.as_str(), "add" | "discard" | "remove")
                    && receiver_ty.as_ref().map_or(false, |t| {
                        matches!(t, Type::DynamicArray(_) | Type::Array(_, _))
                            || matches!(t, Type::Named(n, _) if n == "set" || n == "frozenset")
                            || matches!(t, Type::Str) == false && matches!(t, Type::I64 | Type::PyDynamic)
                    })
                {
                    if method == "add" && arg_ids.len() == 2 {
                        let elem_is_str = matches!(self.type_map.get(&arg_ids[1]), Some(Type::Str));
                        let flag = self.next_id();
                        self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                        self.type_map.insert(flag, Type::I64);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_add_unique".to_string(),
                            args: vec![arg_ids[0], arg_ids[1], flag],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        // vec_push may reallocate and returns the new handle — write
                        // it back so a growing set is not silently lost.
                        if let Some(AstNode::Var(name)) = receiver.as_ref().map(|r| &**r) {
                            if let Some(&slot) = self.name_to_id.get(name) {
                                self.stmts.push(MirStmt::Assign { lhs: slot, rhs: id });
                            }
                        }
                        self.type_map.insert(id, receiver_ty.clone().unwrap());
                        return id;
                    }
                    if arg_ids.len() == 2 {
                        let elem_is_str = matches!(
                            self.type_map.get(&arg_ids[1]),
                            Some(Type::Str)
                        );
                        let flag = self.next_id();
                        self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                        self.type_map.insert(flag, Type::I64);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_discard".to_string(),
                            args: vec![arg_ids[0], arg_ids[1], flag],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        if let Some(AstNode::Var(name)) = receiver.as_ref().map(|r| &**r) {
                            if let Some(&slot) = self.name_to_id.get(name) {
                                self.stmts.push(MirStmt::Assign { lhs: slot, rhs: id });
                            }
                        }
                        self.type_map.insert(id, receiver_ty.clone().unwrap());
                        return id;
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
                            || matches!(t, Type::I64 | Type::PyDynamic)
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
                        .insert(id, receiver_ty.unwrap_or(Type::I64));
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
                    self.stmts.push(MirStmt::Call {
                        func: "py_vec_clip".to_string(),
                        args: vec![arg_ids[0], lo, hi, f1, f2],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, receiver_ty.clone().unwrap());
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
                    self.stmts.push(MirStmt::Call {
                        func: format!("py_vec_{}", method),
                        args: vec![arg_ids[0]],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Str);
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
                    self.stmts.push(MirStmt::Call {
                        func: format!("py_vec_{}", method),
                        args: vec![arg_ids[0]],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Bool);
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
                            self.stmts.push(MirStmt::Call {
                                func: "py_str_prefix_any".to_string(),
                                args: vec![arg_ids[0], vec_id, flag_id],
                                dest: id,
                                type_args: vec![],
                            });
                            self.exprs.insert(id, MirExpr::Var(id));
                            self.type_map.insert(id, Type::Bool);
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
                    // BATCH-296: `k in c` where `c` is a value the compiler
                    // could not type (element of a `dict[str, Any]`, dyn param).
                    // Everything above missed, and the old answer was a silent
                    // constant 0 plus a warning — i.e. membership against a
                    // real dict or list reported False. `zeta_dyn_contains`
                    // tells map / vec / text apart at runtime by GC geometry;
                    // the two key forms are what the typed branches pass
                    // (content hash for the dict, raw handle for the list).
                    if arg_ids.len() == 2
                        && matches!(
                            receiver_ty.as_ref(),
                            None | Some(Type::I64) | Some(Type::PyDynamic)
                        )
                    {
                        let key_id = arg_ids[1];
                        let key_is_str =
                            matches!(self.type_map.get(&key_id), Some(Type::Str)) as i64;
                        let flag = self.next_id();
                        self.exprs.insert(flag, MirExpr::IntLit(key_is_str));
                        self.type_map.insert(flag, Type::I64);
                        let hkey = self.lower_map_key(key_id);
                        self.stmts.push(MirStmt::Call {
                            func: "zeta_dyn_contains".to_string(),
                            args: vec![arg_ids[0], key_id, hkey, flag],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::Bool);
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
                                args.get(1).cloned().unwrap_or(Type::I64)
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
                        self.type_map.insert(
                            id,
                            match method.as_str() {
                                "unique" => Type::DynamicArray(Box::new(elem)),
                                "strftime" => Type::DynamicArray(Box::new(Type::Str)),
                                _ => Type::I64,
                            },
                        );
                        return id;
                    }
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
                                        ts.get(i as usize).cloned().unwrap_or(Type::I64)
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
                    && method == "get"
                    && arg_ids.len() == 2
                    && matches!(self.type_map.get(&arg_ids[1]), Some(Type::Str))
                    && !matches!(receiver_ty.as_ref(), Some(Type::Named(n, _)) if n == "map")
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
                    && !matches!(receiver_ty.as_ref(), Some(Type::Named(n, _)) if n == "map")
                {
                    let key_id = self.lower_map_key(arg_ids[1]);
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
                                        .unwrap_or(Type::I64);
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
                    let k_ty = self.type_map.get(&arg_ids[0]).cloned().unwrap_or(Type::I64);
                    let v_ty = self.type_map.get(&arg_ids[1]).cloned().unwrap_or(Type::I64);
                    self.last_dict_pair_ty = Some((k_ty.clone(), v_ty.clone()));
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

                // `map.get(k[, d])` returns the map's VALUE type. Hardcoding
                // I64 made the caller compare a str handle against a string
                // (`SOURCE_STRATEGIES.get(source, "full") == "skip"` in
                // calibration.py) — the comparison still worked, but `print`
                // showed the raw handle, i.e. one silent wrong VALUE per use.
                let map_value_ty = match receiver_ty.as_ref() {
                    Some(Type::Named(_, targs)) => targs.get(1).cloned().unwrap_or(Type::I64),
                    _ => Type::I64,
                };
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
                    self.type_map.insert(id, map_value_ty);
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
                                    targs.get(1).cloned().unwrap_or(Type::I64)
                                }
                                _ => Type::I64,
                            }
                        } else {
                            Type::I64
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
                                .unwrap_or(Type::I64),
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
                // PY-A (#38 ①): the result slot used to be typed `I64` no matter
                // what the arms returned, so `let s = match 1 { 1 => "hello", _ =>
                // "world" }; print(s)` printed the string handle as a number
                // (measured: `4370672064`). Collect each arm's type instead; see
                // the unification below the chain.
                let mut arm_value_tys: Vec<Type> = Vec::new();

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
                        AstNode::StringLit(pattern_value) => {
                            // String-literal pattern. Deliberately **not** the
                            // `MirStmt::Call{func:"=="}` shape the integer arm
                            // above uses: the `==` operator is declared
                            // External as `i64(i64,i64)`, so calling it on two
                            // `Str` operands compares their *addresses* — always
                            // false, and every arm silently fell through to `_`.
                            // `BinaryOp` is what `op == "+"` lowers to, and the
                            // backend routes it to the str-compare branch.
                            let pattern_id = self.next_id();
                            self.exprs
                                .insert(pattern_id, MirExpr::StringLit(pattern_value.clone()));
                            self.type_map.insert(pattern_id, Type::Str);

                            self.exprs.insert(
                                cond_id,
                                MirExpr::BinaryOp {
                                    op: "==".to_string(),
                                    left: scrutinee_id,
                                    right: pattern_id,
                                },
                            );
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
                            // `None` / `Err` written bare are the same arm as
                            // `Option::None` / `Result::Err`; the checks below
                            // key on the qualified spelling. Left bare, a `None`
                            // arm fell through to "capture variable, always
                            // matches" and swallowed every value.
                            let var_name: &str = match var_name.as_str() {
                                "None" => "Option::None",
                                "Err" => "Result::Err",
                                other => other,
                            };
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
                                // Unit-variant pattern of a registered enum
                                // (e.g. `Color::Red`).
                                let boxed = self
                                    .enum_variant_of(var_name)
                                    .map(|(enum_name, _, _)| self.enum_is_boxed(&enum_name))
                                    .unwrap_or(false);
                                let cond = if boxed {
                                    // The value is a `[tag, …]` block: read the tag.
                                    self.boxed_tag_guard(scrutinee_id, tag)
                                } else {
                                    // All-unit enum: the value IS the discriminant,
                                    // so a bare `scrutinee == tag` stays correct.
                                    let cond = self.next_id();
                                    let pattern_id = self.next_id();
                                    self.exprs.insert(pattern_id, MirExpr::IntLit(tag));
                                    self.type_map.insert(pattern_id, Type::I64);
                                    self.stmts.push(MirStmt::Call {
                                        func: "==".to_string(),
                                        args: vec![scrutinee_id, pattern_id],
                                        dest: cond,
                                        type_args: vec![],
                                    });
                                    self.exprs.insert(cond, MirExpr::Var(cond));
                                    self.type_map.insert(cond, Type::Bool);
                                    cond
                                };
                                self.exprs.insert(cond_id, MirExpr::Var(cond));
                                self.type_map.insert(cond_id, Type::Bool);
                            } else if let Some((_, tag, _)) = self.enum_variant_of(var_name) {
                                // Same spelling without a payload list
                                // (`Token::Eof | Token::BraceClose` as a pattern,
                                // or a unit arm of an enum the parser wrote as a
                                // bare path) for a *boxed* enum whose value block
                                // carries the tag in slot 0.
                                let cond = self.boxed_tag_guard(scrutinee_id, tag);
                                self.exprs.insert(cond_id, MirExpr::Var(cond));
                                self.type_map.insert(cond_id, Type::Bool);
                            } else if var_name == "_" {
                                // Wildcard pattern - always true
                                self.exprs.insert(cond_id, MirExpr::IntLit(1));
                                self.type_map.insert(cond_id, Type::Bool);
                            } else {
                                // Regular variable binding pattern - always matches
                                // Add binding to name_to_id so the arm body can reference it
                                self.name_to_id.insert(var_name.to_string(), scrutinee_id);
                                self.exprs.insert(cond_id, MirExpr::IntLit(1));
                                self.type_map.insert(cond_id, Type::Bool);
                            }
                        }
                        AstNode::StructPattern {
                            variant,
                            fields,
                            rest: _,
                        } => {
                            // The bare Python-style spelling of a builtin
                            // constructor means the same arm (`Some(n)` ==
                            // `Option::Some(n)`), and the tests below key on the
                            // qualified name.
                            let variant: &str = match variant.as_str() {
                                "Some" => "Option::Some",
                                "None" => "Option::None",
                                "Ok" => "Result::Ok",
                                "Err" => "Result::Err",
                                other => other,
                            };
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
                            } else if let Some((_, tag, _)) = self.enum_variant_of(variant) {
                                // A payload-carrying variant of a registered
                                // user enum (`Token::Ident(n)`, `Ast::Lit(n)`).
                                //
                                // This used to be the `else` branch below, which
                                // bound every field name to a literal `0` and
                                // set the condition to "always true" — so
                                // `match t { Token::Ident(n) => n + 1, _ => 900 }`
                                // took the first arm for ANY value and printed 1
                                // (measured). The block that such a variant now
                                // builds is `[tag, p0, …]` (see `enum_is_boxed`),
                                // so the test reads slot 0 and each binding reads
                                // slot 1+k.
                                let cond = self.boxed_tag_guard(scrutinee_id, tag);
                                for (k, (_field_name, field_pattern)) in
                                    fields.iter().enumerate()
                                {
                                    if let AstNode::Var(var_name) = field_pattern {
                                        let slot =
                                            self.deref_slot(scrutinee_id, 8 * (k as i64 + 1));
                                        self.name_to_id.insert(var_name.clone(), slot);
                                    }
                                }
                                self.exprs.insert(cond_id, MirExpr::Var(cond));
                                self.type_map.insert(cond_id, Type::Bool);
                            } else {
                                // Nothing registered this variant name, so there is
                                // no tag to compare against and no slot to read the
                                // payload from: the arm CANNOT be tested.
                                //
                                // This used to set the condition to "always true"
                                // and bind every field to `0`, which made an
                                // untestable arm claim a match on any value —
                                // `match 5 { Foo(n) => n, _ => 7 }` printed 0
                                // (measured) instead of falling through. Fail
                                // closed instead: the arm never matches, so the
                                // value goes to the arms that CAN answer, and the
                                // wildcard or a later arm decides.
                                for (_field_name, field_pattern) in fields {
                                    if let AstNode::Var(var_name) = field_pattern {
                                        // Register the names so the arm body still
                                        // resolves; it is unreachable either way.
                                        let field_id = self.next_id();
                                        self.name_to_id.insert(var_name.clone(), field_id);
                                        self.exprs.insert(field_id, MirExpr::IntLit(0));
                                        self.type_map.insert(field_id, Type::I64);
                                    }
                                }
                                self.exprs.insert(cond_id, MirExpr::IntLit(0));
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
                                    AstNode::StringLit(val) => {
                                        // Same reason as the top-level string
                                        // arm: `Call "=="` has an i64 signature
                                        // and would compare pointers.
                                        self.exprs
                                            .insert(sub_lit_id, MirExpr::StringLit(val.clone()));
                                        self.type_map.insert(sub_lit_id, Type::Str);
                                        self.exprs.insert(
                                            sub_pat_id,
                                            MirExpr::BinaryOp {
                                                op: "==".to_string(),
                                                left: scrutinee_id,
                                                right: sub_lit_id,
                                            },
                                        );
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
                                // The arms above that lower through a
                                // `MirStmt::Call` leave `sub_pat_id` as a
                                // register to load; the string arm already wrote
                                // an inline condition expr there, which this
                                // must not clobber.
                                if !matches!(self.exprs.get(&sub_pat_id), Some(MirExpr::BinaryOp { .. }))
                                {
                                    self.exprs.insert(sub_pat_id, MirExpr::Var(sub_pat_id));
                                }
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
                        } if matches!(&**inner, AstNode::Ignore) => {
                            // PY-A: `q @ _` — the inner pattern is the wildcard, so
                            // the arm matches everything and the binding is the
                            // whole point. Without this case the fallback below
                            // lowered `_` as a *value*, and `lower_expr` gives a
                            // wildcard `IntLit(0)`; the arm became
                            // `scrutinee == 0`, so `match 4 { q @ _ => q + 1 }`
                            // caught nothing and returned the match result slot
                            // nothing ever wrote. The `=> true` reading is the one
                            // the bare-`_` arm above already uses (#38 ⑤).
                            self.name_to_id.insert(name.clone(), scrutinee_id);
                            self.exprs.insert(cond_id, MirExpr::IntLit(1));
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
                            let cond_val = match &**inner {
                                AstNode::RangePattern {
                                    start,
                                    end,
                                    inclusive,
                                } => {
                                    // x @ start..end: bind, then guard the range. The
                                    // guard ends in a stored `Call` dest and has to be
                                    // read through THAT id — an extra `Var(inner_cond_id)`
                                    // hop read an alloca nothing ever wrote, and `-O`
                                    // lowers that load to `brk #0x1` (SIGTRAP).
                                    self.lower_range_guard(scrutinee_id, start, end, *inclusive)
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
                                    inner_cond_id
                                }
                            };
                            self.exprs.insert(cond_val, MirExpr::Var(cond_val));
                            self.type_map.insert(cond_val, Type::Bool);
                            self.exprs.insert(cond_id, MirExpr::Var(cond_val));
                            self.type_map.insert(cond_id, Type::Bool);
                        }
                        AstNode::RangePattern {
                            start,
                            end,
                            inclusive,
                        } => {
                            let and_id =
                                self.lower_range_guard(scrutinee_id, start, end, *inclusive);
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
                    // PY-A: anything this lowering pushes into `self.stmts` belongs
                    // to THIS arm — before the drain below it was left in the
                    // enclosing function, so every arm's side effects ran instead of
                    // only the matched one. Measured on `match 2 { 1 => n+=1,
                    // 2 => n+=10, _ => n+=100 }`:改前退出码 111（三条都跑），改后 10。
                    let arm_body_start = self.stmts.len();
                    let mut then_branch = if let AstNode::Return(inner) = &*arm.body {
                        let ret_val = self.lower_expr(inner);
                        if let Some(ty) = self.type_map.get(&ret_val).cloned() {
                            arm_value_tys.push(ty);
                        }
                        vec![MirStmt::Return { val: ret_val }]
                    } else {
                        let arm_body_id = self.lower_expr(&arm.body);
                        if let Some(ty) = self.type_map.get(&arm_body_id).cloned() {
                            arm_value_tys.push(ty);
                        }
                        vec![MirStmt::Assign {
                            lhs: result_id,
                            rhs: arm_body_id,
                        }]
                    };
                    let arm_body_end = self.stmts.len();
                    if arm_body_end > arm_body_start {
                        let mut arm_stmts = self.stmts.split_off(arm_body_start);
                        arm_stmts.append(&mut then_branch);
                        then_branch = arm_stmts;
                    }

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
                // Unify: every arm has to store a value of the same type. A
                // `return` arm leaves the slot unwritten, and mixed types have no
                // single answer, so both cases keep the old `I64`.
                let all_same = arm_value_tys.len() == arms.len()
                    && arm_value_tys.first().map_or(false, |first| {
                        arm_value_tys.iter().all(|t| t == first)
                    });
                let result_ty = if all_same {
                    arm_value_tys[0].clone()
                } else {
                    Type::I64
                };
                self.type_map.insert(id, result_ty);
            }
            AstNode::FieldAccess { base, field } => {
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
                                let ty = self.type_map.get(&aid).cloned().unwrap_or(Type::I64);
                                self.exprs.insert(id, MirExpr::Var(aid));
                                self.type_map.insert(id, ty);
                                return id;
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
                                    dest: id,
                                    type_args: vec![],
                                });
                                self.exprs.insert(id, MirExpr::Var(id));
                                let ty = match entry.handle.as_deref() {
                                    Some(h) => Type::Named(h.to_string(), vec![]),
                                    None => match entry.ret.as_str() {
                                        "f64" => Type::F64,
                                        "str" => Type::Str,
                                        _ => Type::I64,
                                    },
                                };
                                self.type_map.insert(id, ty);
                                return id;
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
                                self.exprs.insert(id, MirExpr::Var(probe_id));
                                self.type_map.insert(id, t);
                                return id;
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
                                        if let Some(tag) = crate::middle::pylib::handle_tag(other) {
                                            Some(Type::Named(tag.to_string(), vec![]))
                                        } else if other == "PyPath" {
                                            Some(Type::Named("PyPath".to_string(), vec![]))
                                        } else {
                                            None
                                        }
                                    }
                                };
                                if let Some(m) = mapped {
                                    self.type_map.insert(id, m);
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
                                .insert(key_id, MirExpr::StringLit(key.clone()));
                            self.type_map.insert(key_id, Type::Str);
                            self.stmts.push(MirStmt::Call {
                                func: "zeta_env_get".to_string(),
                                args: vec![key_id],
                                dest: id,
                                type_args: vec![],
                            });
                            if self.func_ret_types.contains_key(&key)
                                && !self.global_consts.contains_key(&key)
                                && !self.type_decls.contains_key(&key)
                            {
                                // `mod.fn` as a value: same reason as the `Var` arm
                                // above — the def is not in the env, so read the
                                // symbol's address, and leave the env read in place.
                                self.exprs.insert(id, MirExpr::FuncAddr(key));
                                self.type_map.insert(id, Type::I64);
                                return id;
                            }
                            self.exprs.insert(id, MirExpr::Var(id));
                            // Keep the global's static type. Hardcoding I64 made
                            // `import a; a.C.exists()` a call on an untyped value:
                            // the handle tag was lost and the link failed with a
                            // bare `_a__C.exists`. `from a import C` worked (its
                            // binding path already carried the type).
                            let ty = self.global_ty_of(&key).unwrap_or(Type::I64);
                            self.type_map.insert(id, ty);
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
                // BATCH-429: the arms above took Named / vec-shaped receivers, so
                // what is left has no class tag at all (i64 / dynamic slot) —
                // a unique zero-arg member of that name is a property, so call it
                // instead of loading a field off an untagged handle.
                if !matches!(self.type_map.get(&base_id), Some(Type::Named(_, _)))
                    && let Some((symbol, ret_ty)) = self.unique_zero_arg_property(field)
                {
                    self.stmts.push(MirStmt::Call {
                        func: symbol,
                        args: vec![base_id],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, ret_ty);
                    return id;
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
                        _ => None,
                    },
                };
                self.type_map.insert(id, field_ty.unwrap_or(Type::I64));
            }
            AstNode::StructLit { variant, fields } => {
                // Implement proper struct literal creation
                let mut field_ids = Vec::new();
                // Fields are lowered in source order and a Python `__init__` may
                // read a field it assigned a line earlier (`self.a = x` then
                // `self.b = join(self.a, ...)`). The synthesized ctor has NO
                // `self`, so record each computed value under its field name for
                // the later initializers of this same literal. Nesting is safe:
                // the list is truncated back on the way out.
                let alias_mark = self.self_field_aliases.len();
                for (field_name, field_expr) in fields {
                    let field_id = self.lower_expr(field_expr);
                    // Register the alias as a REAL SLOT, not as the expression id.
                    // A literal/expression id has no alloca, so a later
                    // `self.x` read passed it to a runtime call and codegen loaded
                    // the (missing) slot: `self.cd = "/tmp/root/data";
                    // self.scd = os.path.join(self.cd, "stocks")` produced just
                    // "stocks" — and `MarketDataFetcher.cache_dir` came out as
                    // 2 bytes of garbage, which made every cache path miss.
                    let slot = self.next_id();
                    self.stmts.push(MirStmt::Assign { lhs: slot, rhs: field_id });
                    let ty = self.type_map.get(&field_id).cloned().unwrap_or(Type::I64);
                    self.exprs.insert(slot, MirExpr::Var(slot));
                    self.type_map.insert(slot, ty);
                    self.self_field_aliases.push((field_name.clone(), slot));
                    field_ids.push((field_name.clone(), field_id));
                }
                self.self_field_aliases.truncate(alias_mark);
                // Create Struct expression.
                //
                // `with_enum_tag` is load-bearing for the named-field form of a
                // variant ctor (`Shape::Rect { w: 3, h: 4 }`): this used to copy
                // the literal's own field list straight through, so the block
                // had no slot 0 tag and the arm test — which reads slot 0 —
                // never matched one. `match v { Shape::Rect { w, h } => 1, … }`
                // fell to the wildcard and printed 99 (measured).
                let struct_fields = self.with_enum_tag(variant, field_ids);
                self.exprs.insert(
                    id,
                    MirExpr::Struct {
                        variant: variant.clone(),
                        fields: struct_fields,
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
                // `std::mem::size_of::<T>()` / `align_of::<T>()` → the bare
                // intrinsic name the codegen intercept answers
                // (`codegen.rs:4323`, width from `type_args`). That intercept has
                // been dead since it was written: every route in this arm is gated
                // either on `is_upper` — the *method's* first letter, so `size_of`
                // is lowercase — or on a hardcoded path, and none covered
                // `std::mem`. The call then fell out of the arm with no statement
                // and no `exprs` entry: `let n = size_of::<i64>()` printed 0, and
                // the same phantom id as a `*` operand panicked codegen at
                // `exprs[&values[1]]`. `zeta_src/runtime/array.z:50` is that second
                // shape, so every grow was `malloc(0 * count)`.
                if path_ends_with_mem(path)
                    && (method == "size_of" || method == "align_of")
                    && args.is_empty()
                {
                    let mut mir_type_args = Vec::new();
                    // Only the sub-word widths need naming: the intercept's
                    // fallback arm answers 8, which is also what an unmapped
                    // (generic `T`) type arg must get.
                    if let Some(t) = type_args.first().map(|s| s.trim()) {
                        let w = match t {
                            "i8" | "int8" => Some(Type::I8),
                            "u8" | "uint8" | "byte" => Some(Type::U8),
                            "i16" => Some(Type::I16),
                            "u16" => Some(Type::U16),
                            "i32" | "int32" => Some(Type::I32),
                            "u32" | "uint32" => Some(Type::U32),
                            "f32" | "float32" => Some(Type::F32),
                            "char" => Some(Type::Char),
                            _ => None,
                        };
                        if let Some(w) = w {
                            mir_type_args.push(w);
                        }
                    }
                    self.stmts.push(MirStmt::Call {
                        func: method.clone(),
                        args: vec![],
                        dest: id,
                        type_args: mir_type_args,
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
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
                // (batch 446: the gate asks whether a `quantum` MODULE segment is
                // in the path, not who spelled its prefix. `path[0] == "std"`
                // accepted only `std::quantum::Circuit::new(2)` and left the
                // `import std::quantum;` + `quantum::Circuit::new(2)` spelling
                // (1 census line) reading the ghost 0. The last segment is the
                // class, so `quantum` must sit before it — `quantum::new()` keeps
                // its old answer.)
                if path.len() >= 2 && method == "new" && args.len() <= 2
                    && path[..path.len() - 1].iter().any(|s| s == "quantum")
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
                // `std::quantum::QubitState::one/zero` etc. (batch 446: same
                // module-segment gate as the route above, so the `quantum::`
                // spelling after `import std::quantum;` binds too.)
                if path.len() >= 2
                    && matches!(
                        method.as_str(),
                        "one" | "zero" | "plus" | "minus" | "conj" | "norm" | "abs"
                    )
                    && args.len() <= 1
                    && path[..path.len() - 1].iter().any(|s| s == "quantum")
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

                // A payload-carrying variant of a registered user enum
                // (`Token::Ident(5)`, `Ast::Lit(0)`) constructs the value block
                // `[tag, p0, …]` that the arm test reads (see `enum_is_boxed`).
                //
                // This must come before every capitalized-name route below: the
                // platform-class catch-all swallowed `Enum::Variant(args)` first
                // and emitted `zeta_platform_obj("Ident", 5)`, so the tag was
                // never written and no arm could tell the value from any other.
                if path.len() == 1 && type_args.is_empty() {
                    let qualified = format!("{}::{}", path[0], method);
                    if let Some((_, tag, payload_n)) = self.enum_variant_of(&qualified) {
                        if payload_n == args.len() {
                            let tag_id = self.next_id();
                            self.exprs.insert(tag_id, MirExpr::IntLit(tag));
                            self.type_map.insert(tag_id, Type::I64);
                            let mut fields: Vec<(String, u32)> =
                                vec![("__tag".to_string(), tag_id)];
                            for a in args {
                                let arg_id = self.lower_expr(a);
                                fields.push((format!("f{}", fields.len() - 1), arg_id));
                            }
                            self.exprs.insert(
                                id,
                                MirExpr::Struct {
                                    variant: qualified,
                                    fields,
                                },
                            );
                            self.type_map.insert(id, Type::I64);
                            return id;
                        }
                    }
                }

                // A qualified name the Resolver has a signature for IS a call —
                // regardless of the case of its last segment. Everything below
                // this point is a PY-A dialect heuristic ("a capitalized bare
                // name is a handle constructor"), and those heuristics used to
                // run first: `Parser::new(src)` never reached a call site at all
                // (the `new`-excluded catch-all fell through the capitalised-only
                // general route, leaving a dangling expr id whose slot stayed 0 ⇒
                // null `self` ⇒ SIGSEGV), while `Parser::tokenize(src)` was
                // rewritten into `zeta_platform_obj("tokenize", …)` and printed a
                // heap address. Ask the table before the guess.
                if !path.is_empty() && type_args.is_empty()
                    && self.func_ret_types.contains_key(&func_name)
                {
                    let mut arg_ids = Vec::with_capacity(args.len());
                    for a in args {
                        arg_ids.push(self.lower_expr(a));
                    }
                    let ret_ty = self
                        .func_ret_types
                        .get(&func_name)
                        .cloned()
                        .unwrap_or(Type::I64);
                    self.stmts.push(MirStmt::Call {
                        func: func_name.clone(),
                        args: arg_ids,
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, ret_ty);
                    return id;
                }

                // BATCH-446: `HashMap::new()` is an EMPTY MAP, not a ghost. The
                // table route above still wins when the project really defines a
                // `HashMap`; when nothing does, every consumer read slot 0 —
                // `zeta_src/runtime/actor/map.z:17` (`inner: HashMap::new()`) and
                // `tests/stdlib-foundation/collections_test.z:40` among the 5 census
                // lines in 443's §五 list. `map_new` is a DEFINED C host
                // (`runtime/tokio_runtime_stub.c:213`, unlike `fs_read_to_string`
                // which only exists Rust-side and fails to link — measured this
                // batch), and `MirStmt::MapNew` is the statement the dict literal at
                // :5540 and `dict(m)` at :8456 already emit, so the handle and the
                // `map` tag match the shape `map_insert` / `len` / `[]` expect.
                if args.is_empty() && path.last().map(|s| s.as_str()) == Some("HashMap")
                {
                    self.stmts.push(MirStmt::MapNew { dest: id });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map
                        .insert(id, Type::Named("map".to_string(), vec![]));
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

                // `Box::new(v)` is an IDENTITY: a `Box<T>` slot is the same
                // 64-bit handle as the value inside it. Before this arm the call
                // lowered to NOTHING (no statement, no `exprs` entry), so the
                // phantom id read back as garbage through a `let` and crashed the
                // backend where struct literals index `exprs[field_id]`
                // (codegen.rs:6176, hit via minimal_compiler.z:129).
                if path.len() == 1 && path[0] == "Box" && method == "new" && args.len() == 1 {
                    return self.lower_expr(&args[0]);
                }
                // `String::new()` — same transparency, same phantom-id hole.
                if path.len() == 1 && path[0] == "String" && method == "new" && args.is_empty() {
                    self.exprs.insert(id, MirExpr::StringLit(String::new()));
                    self.type_map.insert(id, Type::Str);
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
                // Lower the elements ONCE — the branch below decides the
                // representation, and lowering twice would duplicate side effects.
                let mut lowered_elems = Vec::new();
                for element in elements {
                    lowered_elems.push(self.lower_expr(element));
                }
                let elem_ty_pre = self.get_common_element_type(&lowered_elems);
                // A Python list literal must be a GROWABLE list with the
                // `[cap|len]` header: every vec_* runtime call (concat /
                // fromkeys / len / index / push) reads that header. As a
                // StackArray those calls read 16 bytes BEFORE the buffer and
                // walked off it — measured as a SEGV in `py_map_fromkeys`, from
                // `wufu_constants`' `dict.fromkeys(GLOBAL_ETF_POOL + …)`.
                // FLOAT lists keep the StackArray form: `vec_push` is an i64
                // channel, so an f64 element would be reinterpreted as its bit
                // pattern (t56/t139 went red when EVERY literal became dynamic).
                if size == 0 || !matches!(elem_ty_pre, Type::F32 | Type::F64) {
                    let start = if size == 0 { Vec::new() } else { lowered_elems };
                    let elem_ty = if size == 0 { Type::I64 } else { elem_ty_pre };
                    let capacity_id = self.next_id();
                    self.exprs.insert(capacity_id, MirExpr::IntLit(size as i64));
                    self.type_map.insert(capacity_id, Type::I64);
                    let h = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_dynarray_new".to_string(),
                        args: vec![capacity_id],
                        dest: h,
                        type_args: vec![],
                    });
                    self.exprs.insert(h, MirExpr::Var(h));
                    self.type_map
                        .insert(h, Type::DynamicArray(Box::new(elem_ty.clone())));
                    for e in start {
                        let sink = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "vec_push".to_string(),
                            args: vec![h, e],
                            dest: sink,
                            type_args: vec![],
                        });
                        self.exprs.insert(sink, MirExpr::Var(sink));
                        self.type_map
                            .insert(sink, Type::DynamicArray(Box::new(elem_ty.clone())));
                        self.stmts.push(MirStmt::Assign { lhs: h, rhs: sink });
                    }
                    self.exprs.insert(id, MirExpr::Var(h));
                    self.type_map
                        .insert(id, Type::DynamicArray(Box::new(elem_ty)));
                    return id;
                }

                // HYBRID MEMORY SYSTEM: Check if this should be a stack array
                // For small, fixed-size arrays, use stack allocation
                if size <= 20000 {
                    // Reasonable stack size limit

                    let element_ids = lowered_elems;
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
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        let ty = recv_ty.unwrap_or(Type::Named("DataFrame".to_string(), vec![]));
                        self.type_map.insert(id, ty);
                        return id;
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
                        return self.lower_expr(base);
                    }
                }
                let bid = self.lower_expr(base);
                let base_ty_pre = self.type_map.get(&bid).cloned().unwrap_or(Type::I64);
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
                                let len_id = self.next_id();
                                self.stmts.push(MirStmt::Call {
                                    func: "vec_len".to_string(),
                                    args: vec![bid],
                                    dest: len_id,
                                    type_args: vec![],
                                });
                                self.exprs.insert(len_id, MirExpr::Var(len_id));
                                self.type_map.insert(len_id, Type::I64);
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
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::I64);
                    return id;
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
                        || matches!(self.type_map.get(&bid), Some(Type::Named(n, _)) if n == "tuple");
                    let tuple_base = self.type_map.get(&bid).cloned();
                    if is_tuple {
                    if let Some(Type::Tuple(ts)) = tuple_base {
                        self.stmts.push(MirStmt::Call {
                            func: "stack_array_get".to_string(),
                            args: vec![bid, iid],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        let elem = match &*index {
                            AstNode::Lit(k) => ts.get(*k as usize).cloned().unwrap_or(Type::I64),
                            _ => Type::I64,
                        };
                        self.type_map.insert(id, elem);
                        return id;
                    }
                    if matches!(tuple_base, Some(Type::Named(ref n, _)) if n == "tuple") {
                        self.stmts.push(MirStmt::Call {
                            func: "stack_array_get".to_string(),
                            args: vec![bid, iid],
                            dest: id,
                            type_args: vec![],
                        });
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
                        return id;
                    }
                    }
                }
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
                // Python `not` is LOGICAL negation — it must NOT become `!`,
                // because the `!` branch treats an ARRAY operand as `~mask`
                // (element-wise NOT). `if not parts:` on a 1-element list became
                // `py_vec_not(parts)` → a list, which is truthy, so the code took
                // the wrong branch and the whole cleaning result came out empty.
                // Lower it to the runtime helper instead (falsy = 0 / empty array).
                if op == "not" {
                    let expr_id = self.lower_expr(expr);
                    // BATCH-297: a parsed-JSON value is a TAGGED cell, so the
                    // geometry reader in `py_not` cannot tell `[]` from `[1]` —
                    // ask the JSON accessor first (it answers 0/1, which the
                    // regular `py_not` negates unchanged).
                    let subject =
                        if matches!(self.type_map.get(&expr_id), Some(Type::Named(n, _)) if n == "PyJson")
                        {
                            let tj = self.next_id();
                            self.stmts.push(MirStmt::Call {
                                func: "py_json_truth".to_string(),
                                args: vec![expr_id],
                                dest: tj,
                                type_args: vec![],
                            });
                            self.exprs.insert(tj, MirExpr::Var(tj));
                            self.type_map.insert(tj, Type::I64);
                            tj
                        } else {
                            expr_id
                        };
                    self.stmts.push(MirStmt::Call {
                        func: "py_not".to_string(),
                        args: vec![subject],
                        dest: id,
                        type_args: vec![],
                    });
                    self.exprs.insert(id, MirExpr::Var(id));
                    self.type_map.insert(id, Type::Bool);
                    return id;
                }
                let op: &str = op;
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
                    // A VECTOR operand is Python's `~mask` (element-wise NOT): the
                    // parser/lowering maps `~` onto `!`, so `df.loc[~invalid]` used
                    // to call the INTEGER `!` on the vector handle — `loc` then got
                    // mask == 0 (measured: "DataFrame.loc: mask is missing").
                    if matches!(
                        self.type_map.get(&expr_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    ) {
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_not".to_string(),
                            args: vec![expr_id],
                            dest,
                            type_args: vec![],
                        });
                        // Batch 398: `~` keeps the operand's element — a Bool mask
                        // stays a Bool mask, an index list stays an index list.
                        let elem = match self.type_map.get(&expr_id) {
                            Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => (**e).clone(),
                            _ => Type::I64,
                        };
                        self.type_map
                            .insert(dest, Type::DynamicArray(Box::new(elem)));
                    } else {
                        // Logical NOT operator
                        let stmt = MirStmt::Call {
                            func: "!".to_string(),
                            args: vec![expr_id],
                            dest,
                            type_args: vec![],
                        };
                        self.stmts.push(stmt);
                    }
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
                } else if op == "~"
                    && matches!(
                        self.type_map.get(&expr_id),
                        Some(Type::DynamicArray(_)) | Some(Type::Array(_, _))
                    )
                {
                    // `~mask` on a mask VECTOR (Python's `df.loc[~invalid]`):
                    // element-wise NOT. The integer bitwise-NOT of the handle made
                    // the mask garbage (measured: `loc` received mask == 0).
                    self.stmts.push(MirStmt::Call {
                        func: "py_vec_not".to_string(),
                        args: vec![expr_id],
                        dest,
                        type_args: vec![],
                    });
                    let elem = match self.type_map.get(&expr_id) {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => (**e).clone(),
                        _ => Type::I64,
                    };
                    self.type_map
                        .insert(dest, Type::DynamicArray(Box::new(elem)));
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
                    // A COMPUTED element (call / subscript / field / binary op)
                    // must be materialized BEFORE the tuple handle is built:
                    // `return d.iloc[0:0], 7` put an uninitialized slot into the
                    // pair, so the caller's frame was garbage and `len(o)` SEGV'd
                    // (measured: `remove_extreme_return_bars`'s empty-parts path).
                    let elem_id = if matches!(
                        elem,
                        AstNode::Call { .. }
                            | AstNode::Subscript { .. }
                            | AstNode::FieldAccess { .. }
                            | AstNode::BinaryOp { .. }
                    ) {
                        self.materialize_for_call(elem_id)
                    } else {
                        elem_id
                    };
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
                // The element types are markers only — every slot is a raw 64-bit
                // word — so hard-wiring them to I64 was free to do but not free to
                // keep: it erased a `Str` member, so unpacking `[(1, "a")]` bound the
                // loop var with no marker and `print(v)` printed the heap address.
                // Take each type from the lowered element instead.
                let tys = element_ids
                    .iter()
                    .map(|&eid| self.type_map.get(&eid).cloned().unwrap_or(Type::I64))
                    .collect();
                self.type_map.insert(id, Type::Tuple(tys));
                self.tuple_slots.insert(id);
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
                            self.env_store(name, cur_id);
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
                // Let-bound names are local from here on; the old code left a
                // duplicate unreachable arm and never inserted them, so a body
                // that `let`s a name shadowing an outer binding captured the
                // OUTER one through the env.
                Self::collect_bound_names(pattern, bound);
            }
            AstNode::Return(e) => Self::collect_free_vars(e, bound, free),
            AstNode::Block { body } => {
                for st in body {
                    Self::collect_free_vars(st, bound, free);
                }
            }
            AstNode::ExprStmt { expr } => Self::collect_free_vars(expr, bound, free),
            // `key=lambda s: (s != "tushare", -score(bs))` — `bs` sits inside a
            // Tuple, which the old matcher never descended into: the capture
            // list came out EMPTY, no env_set/env_get was emitted, and the
            // closure read an uninitialized stack slot (str_trim SIGSEGV on a
            // dynamic string; a stale literal pointer "worked" by luck).
            AstNode::Tuple(items) | AstNode::ArrayLit(items) => {
                for it in items {
                    Self::collect_free_vars(it, bound, free);
                }
            }
            AstNode::FString(parts) => {
                for p in parts {
                    Self::collect_free_vars(p, bound, free);
                }
            }
            AstNode::DictLit { entries } => {
                for (k, v) in entries {
                    Self::collect_free_vars(k, bound, free);
                    Self::collect_free_vars(v, bound, free);
                }
            }
            AstNode::DynamicArrayLit { elements, .. } => {
                for e in elements {
                    Self::collect_free_vars(e, bound, free);
                }
            }
            AstNode::ArrayRepeat { value, size } => {
                Self::collect_free_vars(value, bound, free);
                Self::collect_free_vars(size, bound, free);
            }
            AstNode::PathCall { args, .. } | AstNode::Spawn { args, .. } => {
                for a in args {
                    Self::collect_free_vars(a, bound, free);
                }
            }
            AstNode::AssignOp { target, value, .. } => {
                Self::collect_free_vars(target, bound, free);
                Self::collect_free_vars(value, bound, free);
            }
            AstNode::Cast { expr, .. }
            | AstNode::Await(expr)
            | AstNode::TimingOwned { inner: expr, .. } => {
                Self::collect_free_vars(expr, bound, free);
            }
            AstNode::TryProp { expr } => Self::collect_free_vars(expr, bound, free),
            AstNode::StructLit { fields, .. } => {
                for (_, v) in fields {
                    Self::collect_free_vars(v, bound, free);
                }
            }
            AstNode::Break(v) | AstNode::Continue(v) => {
                if let Some(v) = v {
                    Self::collect_free_vars(v, bound, free);
                }
            }
            AstNode::While { cond, body, else_body } => {
                Self::collect_free_vars(cond, bound, free);
                for st in body {
                    Self::collect_free_vars(st, bound, free);
                }
                for st in else_body {
                    Self::collect_free_vars(st, bound, free);
                }
            }
            AstNode::For { pattern, expr, body, else_body } => {
                Self::collect_free_vars(expr, bound, free);
                for st in body {
                    Self::collect_free_vars(st, bound, free);
                }
                for st in else_body {
                    Self::collect_free_vars(st, bound, free);
                }
                Self::collect_bound_names(pattern, bound);
            }
            AstNode::Loop { body } | AstNode::Unsafe { body } => {
                for st in body {
                    Self::collect_free_vars(st, bound, free);
                }
            }
            AstNode::Match { scrutinee, arms } => {
                Self::collect_free_vars(scrutinee, bound, free);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        Self::collect_free_vars(g, bound, free);
                    }
                    Self::collect_free_vars(&arm.body, bound, free);
                }
            }
            AstNode::Closure { params, body, .. } => {
                // A nested lambda's params are bound for its body; the rest of
                // its free names belong to THIS closure's env.
                let saved: Vec<String> = params.clone();
                for p in &saved {
                    bound.insert(p.clone());
                }
                Self::collect_free_vars(body, bound, free);
                for p in &saved {
                    bound.remove(p);
                }
            }
            _ => {}
        }
    }

    /// Insert every `Var` name reachable in a pattern (Var / Tuple /
    /// BindPattern / TypeAnnotatedPattern) into `bound`.
    fn collect_bound_names(pat: &AstNode, bound: &mut std::collections::HashSet<String>) {
        match pat {
            AstNode::Var(n) => {
                bound.insert(n.clone());
            }
            AstNode::Tuple(items) => {
                for it in items {
                    Self::collect_bound_names(it, bound);
                }
            }
            AstNode::BindPattern { pattern, .. } => Self::collect_bound_names(pattern, bound),
            AstNode::TypeAnnotatedPattern { pattern, .. } => {
                Self::collect_bound_names(pattern, bound)
            }
            _ => {}
        }
    }

    /// Env key a closure must use to read `name`. A `from <mod> import <VAR>`
    /// name is bound in the DEFINING module under `<mod>__<name>`, so capturing
    /// it by its bare alias reads a slot nobody ever wrote (batch 411: the
    /// capture returned 0 and `[c for c in codes if c not in BS]` filtered
    /// nothing). Mirrors the rewrite the ordinary Var read applies.
    fn capture_env_key(&self, name: &str) -> String {
        if self.module_globals.contains(name) {
            return name.to_string();
        }
        if let Some(mangled) = self.symbol_renames.get(name) {
            if mangled != name && self.module_globals.contains(mangled) {
                return mangled.clone();
            }
        }
        if let Some((module, member)) = self.py_member_aliases.get(name) {
            let module = self
                .py_module_aliases
                .get(module)
                .cloned()
                .unwrap_or_else(|| module.clone());
            if self.py_user_modules.contains(&module) {
                let mangled = format!("{}__{}", module.replace('.', "_"), member);
                if self.module_globals.contains(&mangled) {
                    return mangled;
                }
            }
        }
        name.to_string()
    }

    fn lower_closure(&mut self, params: &[String], body: &AstNode) -> String {
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

    /// Assemble a Mir from the current lowering state. Shared by
    /// `lower_to_mir` (top-level items) and `lower_closure` (synthetic
    /// closure functions).
    fn build_mir(&mut self, params: &[String]) -> Mir {
        self.mirror_module_global_writes();
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
            member_bare_calls: std::mem::take(&mut self.member_bare_calls),
            plain_call_names: std::mem::take(&mut self.plain_call_names),
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

/// MIR slot type for a `W` registry return, shared by the B4 dyn route and the
/// batch-432 bare-member route: the handle tag wins (dispatch on it is exact),
/// otherwise the declared scalar / vector kind decides.
fn registry_ret_type(ret_handle: Option<&str>, ret: &str) -> Type {
    match ret_handle {
        Some(h) => Type::Named(h.to_string(), vec![]),
        None => match ret {
            "str" => Type::Str,
            "f64" => Type::F64,
            "vecstr" => Type::DynamicArray(Box::new(Type::Str)),
            "vecpath" => Type::DynamicArray(Box::new(Type::Named("PyPath".to_string(), vec![]))),
            "vecjson" => Type::DynamicArray(Box::new(Type::Named(
                "PyJson".to_string(),
                vec![],
            ))),
            "vecmatch" => Type::DynamicArray(Box::new(Type::Named(
                "PyMatch".to_string(),
                vec![],
            ))),
            _ => Type::I64,
        },
    }
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

/// `std::mem::size_of` is written both fully qualified and after a `use`
/// (`mem::size_of::<f32>()` in `zeta_src/runtime/tensor.z:37`).
fn path_ends_with_mem(path: &[String]) -> bool {
    path.last().map(String::as_str) == Some("mem")
        && (path.len() == 1 || (path.len() == 2 && path[0].as_str() == "std"))
}

fn str_method_symbol(method: &str) -> Option<(&'static str, usize, &'static str)> {    match method {
        "upper" => Some(("host_str_to_uppercase", 1, "str")),
        // `.to_string()` on a string is identity (strings are immutable here);
        // the same-named C alias `to_string` covers call sites whose receiver
        // type is unknown and bypasses this table (batch 363, task #42).
        "to_string" => Some(("host_str_to_string", 1, "str")),
        // Batch 364 (task #42): the string predicate family. `is_whitespace`
        // follows Rust's all-chars rule; `clone` is identity for the same
        // immutability reason as `to_string`. Bare C aliases for the untyped
        // fallback path are in tokio_runtime_stub.c.
        "is_empty" => Some(("host_str_is_empty", 1, "bool")),
        "is_whitespace" => Some(("host_str_is_whitespace", 1, "bool")),
        "clone" => Some(("host_str_clone", 1, "str")),
        // push_str 返回新串（纯函数）；不进表的话结果被定型 I64，
        // 后续 .len() 派发就错了（批次 365 实测）。
        "push_str" => Some(("host_str_push_str", 2, "str")),
        // chars 返回单字符字符串的 vec（"split" = vec<str> 返回种类）。
        "chars" => Some(("host_str_chars", 1, "split")),
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
        // `s.repeat(n)` is the Rust spelling of `"ab" * 3`: same symbol, and
        // without this row the fall-through emitted a bare `_repeat` extern.
        "repeat" => Some(("host_str_repeat", 2, "str")),
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

