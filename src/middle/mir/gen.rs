// src/middle/mir/gen.rs
//! # MIR Generation from AST
//!
//! Lowers Zeta AST to our clean Minimal Intermediate Representation (MIR).
//! All Zeta features (methods, generics, control flow, dicts, etc.) are lowered here.
//! Clean, fast, and fully documented.

mod call_set;
mod call_assert;
mod call_builtin;
mod call_logging;
mod call_re;
mod call_class;
mod call_ctor;
mod call_field;
mod call_json;
mod call_num;
mod call_print;
mod call_subscript;
mod call_if;
mod call_len;
mod call_str;
mod call_binary;
mod call_match;
mod call_unary;
mod call_dict;
mod call_dispatch;
mod call_expr_lit;
mod call_flow;
mod stmt_assign;
mod stmt_funcdef;
mod stmt_misc;
mod stmt_let;
mod call_fstring;
use self::call_class::{classify_call, type_name_of, CallClass};
use self::call_json::json_route;
use self::call_str::{path_ends_with_mem, str_method_symbol, str_method_symbol3, to_string_channel};

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
    /// Batch 761 (#80③): 未声明名告警去重——同一名字一次编译只喊一声。
    undeclared_warned: std::collections::HashSet<String>,
    /// Batch 767 (值标签大弧·读侧)：下标读结果槽 → 格标签槽（zeta_map_value_tag
    /// 读回的 int）。len() 消费：PyDynamic 实参带格标签 ⇒ 运行期按 tag 分派
    /// 各类的 __len__，全不中落 zeta_dyn_len 几何兜底。
    slot_tags: HashMap<u32, u32>,
    /// Batch 763 (#33 M5): 当前函数的声明返回型——Return 处把 I64 值收口成
    /// F64（sitofp 语义），替代按位重读（`-> f64 { return 1 }` 曾打 5e-324）。
    current_fn_ret: Option<Type>,
    /// Batch 761 (#80③): REPL 降值模式——每行独立 resolver、无 import/模块面，
    /// 裸未知名必然真未声明 ⇒ 告警只在 repl_mode 出声（文件路动态名合法链路多，
    /// 全开会淹语料：39/39 文件 1222 行实测）。
    repl_mode: bool,
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
    /// 批次 557: slots known to hold PAIR handles from a list-of-pairs
    /// subscript — their chained subscripts route to stack_array_get.
    pair_slots: std::collections::HashSet<u32>,
    /// t425: for-loop variables whose body is currently being lowered. Their
    /// per-iteration value lives only in the slot, so the py_entry env-first
    /// module-global read carves them out until their loop is done.
    loop_var_active: std::collections::HashSet<String>,
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
    /// Batch 603: class -> first base (resolver `__bases__` markers) — the
    /// base-chain walk for inherited-method dispatch.
    class_bases: HashMap<String, String>,
    /// Batch 654: names whose last top-level assignment was `None` —
    /// threaded from the ctfe pass; print/str render these as "None".
    none_vars_gen: std::collections::HashSet<String>,
    /// Batch 627: `Class::method` -> [(FuncDef param position, Type)] —
    /// call-site refinements for unannotated method params (see the
    /// builder).
    method_param_refinements: HashMap<String, Vec<(usize, Type)>>,
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
    /// Batch 747 (#264): callee -> (its `*args` param, its `**kwargs`
    /// param). Call sites collect positional overflow (list) and unmatched
    /// keyword arguments (dict) bound to these parameters.
    func_star_params: HashMap<String, (Option<String>, Option<String>)>,
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

/// 批次 753（#45 第十一成员落点三，跨道补全）：接收者类型是否属 repr 通道
/// 可渲染家族——`x.to_string()` 据此改发 `lower_to_string`。白名单＝数值/
/// bool/str/char、地图、数组；PyDynamic／未知 Named（未定型 struct 句柄）/
/// TypeVariable 不入——363 在册的未定型 str 兜底语义保留，句柄值不被静默
/// 打印（宁崩不假值）。本辅助在制品缺定义致全树不编译，按单车道约定补全。
fn repr_routable(t: &Type) -> bool {
    match t {
        Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::F32
        | Type::F64
        | Type::Bool
        | Type::Char
        | Type::Str => true,
        Type::Named(n, _) => n == "map" || n == "String",
        Type::DynamicArray(_) | Type::Array(..) => true,
        _ => false,
    }
}

impl MirGen {
    pub fn new() -> Self {
        Self {
            undeclared_warned: std::collections::HashSet::new(),
            slot_tags: HashMap::new(),
            current_fn_ret: None,
            repl_mode: false,
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
            loop_var_active: std::collections::HashSet::new(),
            pair_slots: std::collections::HashSet::new(),
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
            class_bases: HashMap::new(),
    none_vars_gen: std::collections::HashSet::new(),
            method_param_refinements: HashMap::new(),
            re_repl_param: false,
            current_class: None,
            nested_class_aliases: Vec::new(),
            current_module: "__main__".to_string(),
            self_field_aliases: Vec::new(),
            tuple_slots: std::collections::HashSet::new(),
            captured_vars: std::collections::HashMap::new(),
            func_ret_types: HashMap::new(),
            func_param_names: HashMap::new(),
            func_star_params: HashMap::new(),
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

    /// Batch 603: resolver's class->base map (see `class_bases`).
    pub fn with_class_bases(mut self, bases: HashMap<String, String>) -> Self {
        self.class_bases = bases;
        self
    }

    /// Batch 654: the NoneVar set (see the ctfe pass).
    pub fn with_none_vars(mut self, set: std::collections::HashSet<String>) -> Self {
        self.none_vars_gen = set;
        self
    }

    /// Batch 627: `Class::method` -> [(FuncDef param position, Type)] from
    /// the resolver's call-site scan — refines unannotated method params so
    /// f-string parts type from the argument, not the i64 default.
    pub fn with_method_param_refinements(
        mut self,
        map: HashMap<String, Vec<(usize, Type)>>,
    ) -> Self {
        self.method_param_refinements = map;
        self
    }

    /// PY-A: parameter names, so `f(a=1)` binds by name.
    pub fn with_func_param_names(mut self, names: HashMap<String, Vec<String>>) -> Self {
        self.func_param_names = names;
        self
    }

    /// Batch 747 (#264): `*name`/`**name` star-params, so positional
    /// overflow and unmatched keyword arguments collect at the call site.
    pub fn with_func_star_params(
        mut self,
        stars: HashMap<String, (Option<String>, Option<String>)>,
    ) -> Self {
        self.func_star_params = stars;
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
        // Batch 603: INHERITED methods — walk the base chain (`Dog` ->
        // `Animal`) while the direct class misses. The W-table/registry
        // paths below cannot see user classes, so an inherited call
        // (`d.greet()` with greet on Animal) otherwise degraded to an
        // I64-typed generic call and printed the handle.
        let mut cur = tn.rsplit("__").next().unwrap_or(tn).to_string();
        if crate::diagnostics::env_flag("ZETA_PROBE_GLOBALS") {
            eprintln!(
                "[P603] candidate walk tn={} method={} bases={:?} fret={}",
                tn,
                method,
                self.class_bases.get(&cur),
                self.func_ret_types.contains_key(&format!("Animal::{}", method))
            );
        }
        for _ in 0..8 {
            let Some(base) = self.class_bases.get(&cur) else {
                break;
            };
            let cand = format!("{}::{}", base, method);
            if self.func_ret_types.contains_key(&cand) {
                return Some(cand);
            }
            cur = base.clone();
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
                let mpr_key = if fname.contains("::") {
                    fname.clone()
                } else {
                    self.current_class
                        .as_ref()
                        .map(|c| format!("{}::{}", c, fname))
                        .unwrap_or_else(|| fname.clone())
                };
                for (i, (name, param_type)) in params.iter().enumerate() {
                    let id = self.next_id();
                    self.name_to_id.insert(name.clone(), id);
                    self.exprs.insert(id, MirExpr::Var(id));
                    // Batch 627: call-site refinement for unannotated method
                    // params (`g.greet("World")` ⇒ name: Str) — see the
                    // resolver's refine_method_param_types.
                    let mpr = self
                        .method_param_refinements
                        .get(&mpr_key)
                        .and_then(|v| v.iter().find(|(pos, _)| *pos == i))
                        .map(|(_, t)| t.clone());
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
                    } else if pt_str == "**" {
                        // Batch 747 (#264): `**kwargs` star-param — the slot
                        // holds an ordinary map handle, so print/len/subscript
                        // inside the body dispatch as map.
                        self.type_map
                            .insert(id, Type::Named("map".to_string(), Vec::new()));
                    } else if pt_str == "*" {
                        // Batch 752: `*args` star-param — the slot holds an
                        // ordinary list handle, so len/subscript inside the
                        // body dispatch as array.
                        self.type_map
                            .insert(id, Type::DynamicArray(Box::new(Type::PyDynamic)));
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
                    if let Some(t) = mpr {
                        self.type_map.insert(id, t);
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
                // 批次 867：Let 臂迁入 gen/stmt_let.rs（原臂逐字）。
                self.lower_let_stmt(pattern, expr);
            }
            AstNode::Assign(lhs, rhs) => {
                // 批次 865：Assign 臂迁入 gen/stmt_assign.rs（原臂逐字）。
                self.lower_assign_stmt(lhs, rhs);
            }
            AstNode::AssignOp { op, target, value } => {
                // 批次 868：AssignOp 臂迁入 gen/stmt_assign.rs（原臂逐字，
                // 签名取原样解构类型故零适配）。
                self.lower_assign_op(op, target, value);
            }
            AstNode::Return(inner) => {
                // `return (a, b)` must hand back a HEAP array. A StackArray is an
                // alloca: the pointer dies with the frame, so the caller's
                // `stack_array_get` destructuring read dead stack (measured:
                // `return d.iloc[0:0], 7` → `len(o)` SEGV). Build a real
                // `[cap|len]` array — `stack_array_get(arr, i)` reads
                // `((i64*)arr)[i]`, i.e. exactly the dynarray DATA pointer that
                // `zeta_dynarray_new`/`vec_push` hand out.
                let mut val = if let AstNode::Tuple(items) = &**inner {
                    let mut vals = Vec::with_capacity(items.len());
                    let mut tys = Vec::with_capacity(items.len());
                    for it in items {
                        let vid = self.lower_expr(it);
                        tys.push(self.type_map.get(&vid).cloned().unwrap_or_else(Type::slot_fallback));
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
                        Type::DynamicArray(Box::new(tys.first().cloned().unwrap_or_else(Type::slot_fallback))),
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
                            Type::DynamicArray(Box::new(tys.first().cloned().unwrap_or_else(Type::slot_fallback))),
                        );
                    }
                    self.type_map.insert(h, Type::Tuple(tys));
                    self.tuple_slots.insert(h);
                    h
                } else {
                    self.lower_expr(inner)
                };
                let val = self.coerce_return_val(val);
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
                // 批次 874：FuncDef 语句臂（107 行）迁入 gen/stmt_funcdef.rs（869 法）。
                self.lower_funcdef_stmt(fn_name, params, body, ret_expr);
            }
            AstNode::If { cond, then, else_ } => {
                // 批次 864：If 语句臂迁入 gen/call_if.rs（原臂逐字）。
                self.lower_if_stmt(cond, then, else_);
            }
            AstNode::ExprStmt { expr } => {
                // 批次 875：ExprStmt 臂迁入 gen/stmt_misc.rs（869 法）。
                self.lower_exprstmt(expr);
            }
            AstNode::For {
                pattern,
                expr,
                body,
                else_body,
            } => {
                // 批次 863：For 语句臂迁入 gen/call_flow.rs（原臂逐字）。
                self.lower_for_stmt(pattern, expr, body, else_body);
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
                // 批次 862：While 语句臂迁入 gen/call_flow.rs（原臂逐字）。
                self.lower_while_stmt(cond, body, else_body);
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
                // 批次 875：ConstDef 臂迁入 gen/stmt_misc.rs（869 法）。
                self.lower_constdef_stmt(name, value);
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
                // 批次 875：IfLet 臂迁入 gen/stmt_misc.rs（869 法）。
                self.lower_iflet_stmt(pattern, expr, then, else_);
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
                // 批次 875：Await 臂迁入 gen/stmt_misc.rs（869 法）。
                self.lower_await_stmt(body);
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
        let ty = self.type_map.get(&id).cloned().unwrap_or_else(Type::slot_fallback);
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
            let nid = self.emit_call("map_str_key", vec![id], Type::I64);
            return nid;
        }
        self.lower_map_key(id)
    }

    fn lower_map_key(&mut self, id: u32) -> u32 {
        // Batch 574: ALWAYS route keys through the runtime normalizer —
        // map_str_key content-hashes readable text handles and passes
        // everything else (raw ints, already-hashed keys) through identity.
        // Previously only Str-TYPED keys were normalized; an under-typed
        // key holding a text pointer stored the POINTER as the key, so the
        // same lookup through a different literal missed (`class_inventory_dict`:
        // add_item(name, qty) wrote pointer-hashed keys, report() read
        // content-hashed keys → all values 0).
        let nid = self.emit_call("map_str_key", vec![id], Type::I64);
        nid
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
                targs.first().cloned().unwrap_or_else(Type::slot_fallback),
                targs.get(1).cloned().unwrap_or_else(Type::slot_fallback),
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

    /// Batch 659: a map Subscript whose receiver is a plain variable with a
    /// Str-refined value type renders through the 535 per-key tag side table
    /// (`zeta_map_get_render`) — the raw word otherwise reaches string
    /// consumption as a bogus pointer (strlen/segv or truncated concat).
    /// Returns None for anything else so callers fall through untouched.
    fn mapsub_render_str(&mut self, b: &AstNode, k: &AstNode) -> Option<u32> {
        let is_map_str = if let AstNode::Var(vname) = b {
            self.name_to_id.get(vname.as_str()).map_or(false, |&sid| {
                matches!(
                    self.type_map.get(&sid),
                    Some(Type::Named(n, params))
                        if n == "map" && matches!(params.last(), Some(Type::Str))
                )
            })
        } else {
            false
        };
        if !is_map_str {
            return None;
        }
        let recv_id = self.lower_expr(b);
        let k_id = self.lower_expr(k);
        let r_id = self.emit_call("zeta_map_get_render", vec![recv_id, k_id], Type::Str);
        Some(r_id)
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
        // 批次 822：通道判定收敛到 call_str::to_string_channel（单测钉住
        // 每条通道）；元组特算留在本处（需要 arity+逐位标签）。
        if let Some(func) = self
            .type_map
            .get(&id)
            .cloned()
            .as_ref()
            .and_then(to_string_channel)
        {
            let nid = self.emit_call(func, vec![id], Type::Str);
            return nid;
        }
        let func = match self.type_map.get(&id).cloned() {
            Some(Type::Tuple(ts)) => {
                // General tuple repr: arity + per-element 2-bit kind tags are
                // known statically from Type::Tuple; the runtime renders the
                // contiguous i64-slot run (stack_array_get layout).
                let mut tags: i64 = 0;
                for (i, t) in ts.iter().enumerate() {
                    if i >= 31 {
                        break;
                    }
                    let code: i64 = match t {
                        Type::Str => 1,
                        Type::F64 | Type::F32 => 2,
                        Type::Bool => 3,
                        _ => 0,
                    };
                    tags |= code << (2 * i);
                }
                let len_id = self.next_id();
                self.exprs
                    .insert(len_id, MirExpr::IntLit(ts.len() as i64));
                self.type_map.insert(len_id, Type::I64);
                let tags_id = self.next_id();
                self.exprs.insert(tags_id, MirExpr::IntLit(tags));
                self.type_map.insert(tags_id, Type::I64);
                let nid = self.emit_call("zeta_tuple_repr", vec![id, len_id, tags_id], Type::Str);
                return nid;
            }
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

    /// 批次 535：容器实参的元素表示编号（与 `codegen.rs` 的 `DictInsert` 登记臂同一张
    /// 表：4=vec<str>、5=vec<i64>、6=vec<f64>、7=vec<bool>）。**只登记容器**：标量走
    /// `py_df_setitem` 的广播分支，列里每个格都是同一个字，登记与否都不改变读边界。
    fn zt_container_value_tag(ty: Option<&Type>) -> Option<i64> {
        let el: &Type = match ty {
            Some(Type::DynamicArray(el)) => &**el,
            Some(Type::Array(el, _)) if matches!(**el, Type::F64) => &**el,
            _ => return None,
        };
        Some(match el {
            Type::Str => 4,
            Type::F64 | Type::F32 => 6,
            Type::Bool => 7,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::U8 | Type::U16 | Type::U32
            | Type::U64 | Type::Usize => 5,
            _ => return None,
        })
    }

    /// A standalone integer literal, already registered in `exprs`/`type_map`.
    fn int_slot(&mut self, value: i64) -> u32 {
        let id = self.next_id();
        self.exprs.insert(id, MirExpr::IntLit(value));
        self.type_map.insert(id, Type::I64);
        id
    }


    /// 批次 814（重构第一步）：发射一条运行期调用并完成三行记账
    /// （登记指令、登记表达式、登记类型）。收拢前这三行在全文手写
    /// 381 处，漏写 type_map 一行就是"静默错值"事故。
    /// 【勘案补记】本助手曾因笔误自调自身（应为调 emit_call_into），
    /// 尾调用优化成无限循环＝814 验证期所有"编译转圈"的真凶；
    /// 车道并发修改纯属巧合。勘误见 worktree.md 815 行。
    fn emit_call(&mut self, func: &str, args: Vec<u32>, ty: Type) -> u32 {
        let id = self.next_id();
        self.emit_call_into(id, func, args, ty);
        id
    }

    /// 同上，但槽号由调用方持有（`let x = self.next_id();` 在前的现场）。
    fn emit_call_into(&mut self, dest: u32, func: &str, args: Vec<u32>, ty: Type) {
        self.stmts.push(MirStmt::Call {
            func: func.to_string(),
            args,
            dest,
            type_args: vec![],
        });
        self.exprs.insert(dest, MirExpr::Var(dest));
        self.type_map.insert(dest, ty);
    }

    /// 批次 805（#表示上限格）：expr id 的编译期整值（字面量或 CTFE 常量），
    /// 供 `**` 折算臂判定溢出面。
    fn ctfe_int_of(
        exprs: &HashMap<u32, MirExpr>,
        consts: &HashMap<u32, i64>,
        id: u32,
    ) -> Option<i64> {
        if let Some(MirExpr::IntLit(v)) = exprs.get(&id) {
            return Some(*v);
        }
        consts.get(&id).copied()
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
        let cond_id = self.emit_call("==", vec![tag_slot, expect_id], Type::Bool);
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

        self.emit_call_into(ge_id, ">=", vec![scrutinee_id, start_id], Type::Bool);

        self.stmts.push(MirStmt::Call {
            func: if inclusive { "<=" } else { "<" }.to_string(),
            args: vec![scrutinee_id, end_id],
            dest: le_id,
            type_args: vec![],
        });
        self.exprs.insert(le_id, MirExpr::Var(le_id));
        self.type_map.insert(le_id, Type::Bool);

        let and_id = self.emit_call("&&", vec![ge_id, le_id], Type::Bool);
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
                let len_id = self.emit_call("vec_len", vec![base_id], Type::I64);
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
    /// Batch 761 (#80③): REPL 降值模式开关（见 repl_mode 字段）。
    pub fn with_repl_mode(mut self, on: bool) -> Self {
        self.repl_mode = on;
        self
    }

    /// Batch 766 (值标签大弧·写侧第一步)：类名字母表——type_decls 的 py 模式类
    /// 按名字排序给稳定 id，tag 值＝CLASS_TAG_BASE + id。运行期只存不解释这个
    /// int；读侧（下一格）拿 tag 反查类名做 __len__/方法分派。
    const CLASS_TAG_BASE: i64 = 100;

    fn class_tag_id(&self, name: &str) -> Option<i64> {
        if !self.type_decls.contains_key(name) {
            return None;
        }
        let mut names: Vec<&String> = self.type_decls.keys().collect();
        names.sort();
        names
            .iter()
            .position(|n| n.as_str() == name)
            .map(|i| Self::CLASS_TAG_BASE + i as i64)
    }

    /// Batch 763 (#33 M5): 声明返回 F64 的函数里 return 的 I64 值收口成 F64。
    /// 按位重读曾把整数位当浮点尾数打 5e-324（t 座探针 m5.z）。反向
    /// （声明 I64 返回 F64）不动——现状偶然与 CPython 一致，收口反成回归。
    fn coerce_return_val(&mut self, val: u32) -> u32 {
        if matches!(self.current_fn_ret, Some(Type::F64))
            && matches!(self.type_map.get(&val), Some(Type::I64))
        {
            let nid = self.emit_call("zeta_float_i64", vec![val], Type::F64);
            return nid;
        }
        val
    }

    /// Batch 794 (#214)：map 闭包形参用法推断。1＝数值证据（形参直参与
    /// 数字字面量做 `*`/`+`/`-`）、0＝未知/str。深度限 12。
    fn closure_param_usage(node: &AstNode, param: &str, depth: i32) -> i8 {
        if depth > 12 {
            return 0;
        }
        let is_param = |n: &AstNode| matches!(n, AstNode::Var(v) if v == param);
        match node {
            AstNode::BinaryOp { op, left, right } => {
                if matches!(op.as_str(), "*" | "+" | "-" | "/")
                    && ((is_param(left)
                        && matches!(
                            **right,
                            AstNode::Lit(_) | AstNode::FloatLit(_)
                        ))
                        || (is_param(right)
                            && matches!(
                                **left,
                                AstNode::Lit(_) | AstNode::FloatLit(_)
                            )))
                {
                    return 1;
                }
                let l = Self::closure_param_usage(left, param, depth + 1);
                if l != 0 {
                    return l;
                }
                Self::closure_param_usage(right, param, depth + 1)
            }
            AstNode::ExprStmt { expr } => Self::closure_param_usage(expr, param, depth + 1),
            AstNode::Return(val) => match &**val {
                AstNode::Tuple(items) => {
                    let mut any = 0;
                    for it in items {
                        any = Self::closure_param_usage(it, param, depth + 1);
                    }
                    any
                }
                _ => Self::closure_param_usage(val, param, depth + 1),
            },
            _ => 0,
        }
    }

    /// Batch 761 (#80③): 未声明名读——此前静默 fabricate（REPL 里 `exit`
    /// 打 1、`fn main() -> i64 { exit }` 整个文件无声编译）。行为保持（槽值
    /// 照旧），只把沉默变成点名告警；每名每编译一次。
    fn warn_undeclared(&mut self, name: &str) {
        if self.undeclared_warned.contains(name) {
            return;
        }
        if !self.repl_mode {
            return;
        }
        self.undeclared_warned.insert(name.to_string());
        // 直写 stderr（`warning: [Wxxx] ` 前缀＝run_all 诊断聚合的 grep 口径）。
        // 不走 diagnostics::emit——TL_REPORTER 缓冲全仓无排水方（take_diagnostics
        // 零调用），走它就是黑洞（本批实测：warn 被调用、W0106 不落 stderr）。
        eprintln!(
            "warning: [W0106] PY-A: undeclared name `{}` — reading it fabricated a slot value; check spelling or declare it",
            name
        );
    }

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
                // Batch 572: `Counter.count = x` — class VARIABLE write
                // rewrites to the mangled module global.
                if let AstNode::FieldAccess { base, field } = &**lhs {
                    if let AstNode::Var(vname) = &**base {
                        let gname = format!("{}__{}", vname, field);
                        if self.type_decls.contains_key(vname.as_str())
                            && self.module_globals.contains(&gname)
                        {
                            let rewritten = AstNode::Assign(
                                Box::new(AstNode::Var(gname)),
                                rhs.clone(),
                            );
                            self.lower_ast(&rewritten);
                            return self.i64_zero_id();
                        }
                    }
                }
                // PY-A: walrus `name := expr` in expression position — lower
                // rhs, bind the name (implicit decl or rebinding), and the
                // expression value is the assigned value.
                if let AstNode::Var(name) = &**lhs {
                    let rhs_id = self.lower_expr(rhs);
                    let dest = match self.name_to_id.get(name).copied() {
                        None => {
                            let new_id = self.next_id();
                            self.exprs.insert(new_id, MirExpr::Var(new_id));
                            let ty = self.type_map.get(&rhs_id).cloned().unwrap_or_else(Type::slot_fallback);
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
                    // t425: in a module body (synthesized main or user main
                    // with module statements merged in — both carry
                    // PY_ENTRY_ATTR) a module global's own slot holds what
                    // THIS body last wrote; a function called since may have
                    // refreshed the env cell behind its back (`add()` mutates
                    // `total`, batch 385 pinned the two-answers shape), so
                    // those names read env-first here. Three gates keep the
                    // slot when the cell cannot faithfully represent the
                    // value: loop variables (their per-iteration value lives
                    // only in the slot); class instances (their read side
                    // needs "which ctor call made me" provenance, which an
                    // env round-trip loses — measured: t464's two same-named
                    // classes with swapped field orders both fell back to
                    // the first-writer layout and the fields transposed); and
                    // type mismatch — the cell reads back typed from the
                    // name's declared/inferred type, and where that is
                    // coarser than the module body's own binding the env
                    // read would DOWNGRADE it (measured: `for k, v in pairs`
                    // with pairs a module global — its slot carries
                    // Tuple element types that the cell's `DynamicArray(I64)`
                    // inference loses, and the destructure read garbage).
                    let cell_ty = self.global_ty_of(name).unwrap_or(Type::I64);
                    let env_first = self.py_entry
                        && self.module_globals.contains(name)
                        && !self.loop_var_active.contains(name)
                        && !matches!(cell_ty, Type::Named(..))
                        && self
                            .type_map
                            .get(&existing)
                            .map_or(false, |t| *t == cell_ty);
                    if !env_first {
                        return existing;
                    }
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
                // Batch 593: a nested `def` hoisted as a closure — the name
                // used as a VALUE (`return inner`; `f = inner`) must produce
                // the synthetic function's address, exactly like a lambda
                // value. Without this the read fell to the implicit-declare
                // slot and `return inner` handed back an uninitialized word
                // (indirect call → SIGBUS; closure_nonlocal).
                if let Some(hoisted) = self
                    .closure_vars
                    .get(name.as_str())
                    .cloned()
                    .or_else(|| self.hoisted_names.get(name.as_str()).cloned())
                {
                    let id = self.next_id();
                    self.exprs.insert(id, MirExpr::FuncAddr(hoisted));
                    self.type_map.insert(id, Type::I64);
                    return id;
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
                            self.warn_undeclared(name);
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
                    self.warn_undeclared(name);
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
            AstNode::NoneLit => {
                // Batch 624: `None` as a VALUE stays 0 (i64) — only the
                // print/str literal faces render "None" (see those arms).
                self.exprs.insert(id, MirExpr::IntLit(0));
                self.type_map.insert(id, Type::I64);
            }
            AstNode::BigIntLit(text) => {
                // Batch 647: a beyond-i64 literal lowers to the zeta_big
                // runtime handle ([lo|hi] 16-byte GC block, batch 641),
                // statically typed Named("BigInt") — arithmetic on it
                // routes through the big family, print/str render via
                // zeta_big_to_string.
                let v: i128 = text.parse().unwrap_or(0);
                let lo = self.int_slot(v as i64);
                let hi = self.int_slot((v >> 64) as i64);
                self.stmts.push(MirStmt::Call {
                    func: "zeta_big_new".to_string(),
                    args: vec![lo, hi],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::Named("BigInt".to_string(), vec![]));
            }
            AstNode::StringLit(s) => {
                self.exprs.insert(id, MirExpr::StringLit(s.clone()));
                self.type_map.insert(id, Type::Str);
            }
            AstNode::FString(parts) => {
                // 批次 860：FString 臂迁入 gen/call_fstring.rs。
                return self.lower_fstring(parts, id);
            }
            AstNode::BinaryOp { op, left, right } => {
                // 批次 869：BinaryOp 臂（1485 行）迁入 gen/call_binary.rs——
                // 签名取原样解构类型故臂体零适配；返回值必须转发
                //（臂内 in (tuple) 改写等产出非 id 槽，866 教训）。
                return self.lower_binary_op(op, left, right, id);
            }

            AstNode::Loop { body } => {
                // 批次 861：Loop 臂迁入 gen/call_flow.rs（批 860 的迁移从未
                // 编译通过——替换文本被脚本拼进 dict_spread 块，本批重做）。
                return self.lower_loop(body, id);
            }
            AstNode::If { cond, then, else_ } => {
                // 批次 859：If 表达式臂迁入 gen/call_if.rs（原臂逐字）。
                return self.lower_if_expr(cond, then, else_, id);
            }

            AstNode::DictLit { entries } => {
                // 批次 873：DictLit 臂（120 行）迁入 gen/call_dict.rs（869 法）。
                return self.lower_dict_lit(entries, id);
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
                // 批次 876：Call 臂（5840 行）整体迁入 gen/call_dispatch.rs
                // ——869 零适配法；返回值必须转发（臂内 15 处产出非 id 槽）。
                return self.lower_call_arm(receiver, method, args, type_args, id);
            }
            AstNode::Match { scrutinee, arms } => {
                // 批次 870：Match 臂（599 行）迁入 gen/call_match.rs——
                // 签名取原样解构类型，臂体零适配（869 法）。
                return self.lower_match_expr(scrutinee, arms, id);
            }
            AstNode::FieldAccess { base, field } => {
                // 批次 846：FieldAccess 臂整体迁入 gen/call_field.rs。
                // 批次 866：返回值必须转发——函数体内多处早退
                // `return self.lower_expr(...)`（类变量路由、实例类变量路由、
                // self_field_aliases 等）产出的是新槽而不是 dest；批 846 的
                // 迁移调用点丢弃了返回值，这些路径全被预填的替身槽
                // （IntLit(0)）顶掉＝静默错值（Counter::get 打 0，
                // class_variable_crash／class_inheritance_field 两例回归）。
                return self.lower_field_access(base, field, id);
            }
            AstNode::StructLit { variant, fields } => {
                // 批次 872：StructLit 臂迁入 gen/call_expr_lit.rs（869 法）。
                return self.lower_struct_lit(variant, fields, id);
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
                    self.emit_call_into(id, "monotonic_ns", vec![], Type::I64);
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
                    self.emit_call_into(id, "zeta_qc_new", vec![n_id], Type::I64);
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
                    self.emit_call_into(id, "zeta_qc_is_normalized", vec![z], Type::I64);
                    return id;
                }

                // PY-A: `std::memory::capability::new(n)` — capability handle
                if path.len() == 3 && path[0] == "std" && path[1] == "memory"
                    && path[2] == "capability" && method == "new" && args.len() == 1
                {
                    let n_id = self.lower_expr(&args[0]);
                    self.emit_call_into(id, "zeta_dynarray_new", vec![n_id], Type::I64);
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
                                    let elem = self.emit_call("array_get", vec![arr_id, idx_id], Type::I64);
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
                // 批次 872：ArrayLit 臂迁入 gen/call_expr_lit.rs（869 法）。
                return self.lower_array_lit(elements, id);
            }
            AstNode::ArrayRepeat { value, size } => {
                let value_id = self.lower_expr(value);

                // Get the type of the value expression
                let elem_type = self.type_map.get(&value_id).cloned().unwrap_or_else(Type::slot_fallback);

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
                // 批次 845：Subscript 臂整体迁入 gen/call_subscript.rs
                //（loc/iloc 重写、PyJson 路由、dict 兜底、zeta_dyn_getitem
                // 几何判形——原臂逐字）。
                self.lower_subscript(base, index, id);
            }
            AstNode::DynamicArrayLit {
                elem_type,
                elements,
            } => {
                // 批次 872：DynamicArrayLit 臂迁入 gen/call_expr_lit.rs（869 法）。
                return self.lower_dynamic_array_lit(elem_type, elements, id);
            }
            AstNode::UnaryOp { op, expr } => {
                // 批次 871：UnaryOp 臂（184 行）迁入 gen/call_unary.rs（869 法）。
                return self.lower_unary_op(op, expr, id);
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
                // 批次 872：Tuple 表达式臂迁入 gen/call_expr_lit.rs（869 法）。
                return self.lower_tuple_expr(elements, id);
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

    /// Batch 655: statically determine whether an AND/OR expression's result
    /// is a None literal.  `3 and None` → true (truthy left ⇒ selects right =
    /// None).  `None or 5` → true (falsy left ⇒ selects left = None).  Used
    /// by the print lowering to render "None" instead of "0" (t507).
    fn and_or_select_is_none(&self, expr: &AstNode) -> bool {
        if let AstNode::BinaryOp { op, left, right } = expr {
            if op != "&&" && op != "||" {
                return false;
            }
            let lhs_truth = match left.as_ref() {
                AstNode::Lit(0) => Some(false),
                AstNode::NoneLit => Some(false),
                AstNode::Lit(n) if *n != 0 => Some(true),
                AstNode::StringLit(s) => Some(!s.is_empty()),
                _ => None,
            };
            match (op.as_str(), lhs_truth) {
                ("&&", Some(true)) => matches!(right.as_ref(), AstNode::NoneLit),
                ("&&", Some(false)) => matches!(left.as_ref(), AstNode::NoneLit),
                ("||", Some(false)) => matches!(right.as_ref(), AstNode::NoneLit),
                ("||", Some(true)) => matches!(left.as_ref(), AstNode::NoneLit),
                _ => false,
            }
        } else {
            false
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
    /// Batch 633: recursive element-type tag string for nested-list repr —
    /// "0" int, "1" f64, "2" str, "3" bool, "4,<inner>" list. The runtime
    /// `py_json_dumps_vec_nested` walks the tokens level by level.
    fn elem_tag_string(t: &Type) -> String {
        match t {
            Type::DynamicArray(e) | Type::Array(e, _) => {
                format!("4,{}", Self::elem_tag_string(e))
            }
            Type::F64 | Type::F32 => "1".to_string(),
            Type::Str => "2".to_string(),
            Type::Bool => "3".to_string(),
            _ => "0".to_string(),
        }
    }

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
                // Batch 659 (mainline) / 662 (cleanup): a no-receiver CALLEE
                // name is a free variable when it names a captured local
                // lambda — `condition(m)` inside a comprehension filter
                // captures `condition` by CALL, not by a Var read (t233:
                // without this the closure body fell to the `_condition`
                // ghost stub). Both lanes converged on the same fix. builtin/
                // module callee names over-collect harmlessly: every capture
                // consumer filters through name_to_id.
                if receiver.is_none() && !bound.contains(method) {
                    free.insert(method.clone());
                }
                // args 递归
                for a in args {
                    Self::collect_free_vars(a, bound, free);
                }
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

