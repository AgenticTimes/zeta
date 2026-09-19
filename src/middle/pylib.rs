//! Python standard-library shim registry (data-driven, V1).
//!
//! `import X` / `from X import y` no longer get swallowed: the parser emits
//! `zeta_py_import` / `zeta_py_from` markers, the Resolver validates them
//! against this table (fail-loud on unknown members — a silent no-op import
//! could otherwise compile into a program that reads garbage), and MirGen
//! maps module member calls onto runtime shims.
//!
//! The table itself lives in `pylib/registry.txt` and is embedded with
//! `include_str!`, so **adding a library is a data edit, not a Rust edit**:
//! declare the module/member lines there and (if it needs new primitives)
//! implement the C shim; the extern declarations codegen emits are derived
//! from the same file.
//!
//! The concurrency modules are thin shims over the existing native
//! primitives (pthread spawn/join, mutexes, fork, clocks) — see
//! runtime/tokio_runtime_stub.c.

use std::collections::HashMap;
use std::sync::OnceLock;

const REGISTRY_SRC: &str = include_str!("../../pylib/registry.txt");

#[derive(Debug, Clone)]
pub struct PyMember {
    pub name: String,
    pub symbol: String,
    /// Argument types in order (so codegen can declare the extern exactly —
    /// an f64 parameter declared i64 truncates through fptosi).
    pub args: Vec<String>,
    /// "i64" | "f64" | "void"
    pub ret: String,
    /// Handle type tag stored in the MIR type map, so method dispatch on the
    /// returned handle is exact instead of name-guessing.
    pub handle: Option<String>,
    /// A1: `decl=0` → codegen must not auto-declare (internal / rewrite-only).
    /// Default true (declare).
    pub decl: bool,
    /// A1: `stub=1` → not implemented; loud-fail path (task D). nm check skips.
    pub stub: bool,
    /// A1: `alias-of=<sym>` → LLVM `.N` rename alias data (task A3).
    pub alias_of: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PyModule {
    pub name: String,
    pub aliases: Vec<String>,
    pub members: Vec<PyMember>,
    /// `N <module>`: a module whose imports are accepted and bind nothing
    /// (`__future__`) — no member validation, no warning.
    pub noop: bool,
}

#[derive(Debug, Clone)]
pub struct PyMethod {
    pub handle: String,
    pub method: String,
    pub symbol: String,
    /// Argument count including the receiver handle.
    pub arity: usize,
    pub ret_handle: Option<String>,
    /// "i64" (default), "f64" or "str".
    pub ret: String,
    pub decl: bool,
    pub stub: bool,
    pub alias_of: Option<String>,
}

/// Helper shim (`X` line): may be compiler-emitted but not importable.
#[derive(Debug, Clone)]
pub struct PyHelper {
    pub symbol: String,
    pub args: Vec<String>,
    pub ret: String,
    pub decl: bool,
    pub stub: bool,
    pub alias_of: Option<String>,
}

struct Registry {
    modules: Vec<PyModule>,
    methods: Vec<PyMethod>,
    /// `X <symbol> args=… ret=…`: shims the compiler may emit but that are
    /// not importable members (e.g. the typed json.dumps variants). Declared
    /// so codegen emits their externs with the right ABI.
    helpers: Vec<PyHelper>,
}

fn parse_bool_flag(v: &str) -> bool {
    matches!(v, "1" | "true" | "yes")
}

fn parse_registry() -> Registry {
    let mut modules: Vec<PyModule> = Vec::new();
    let mut methods: Vec<PyMethod> = Vec::new();
    let mut helpers: Vec<PyHelper> = Vec::new();
    for (lineno, raw) in REGISTRY_SRC.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        match it.next() {
            Some("M") => {
                if let Some(name) = it.next() {
                    modules.push(PyModule {
                        name: name.to_string(),
                        aliases: Vec::new(),
                        members: Vec::new(),
                        noop: false,
                    });
                }
            }
            Some("N") => {
                if let Some(name) = it.next() {
                    if let Some(m) = modules.iter_mut().find(|m| m.name == name) {
                        m.noop = true;
                    } else {
                        modules.push(PyModule {
                            name: name.to_string(),
                            aliases: Vec::new(),
                            members: Vec::new(),
                            noop: true,
                        });
                    }
                }
            }
            Some("A") => {
                let (Some(module), Some(alias)) = (it.next(), it.next()) else {
                    continue;
                };
                if let Some(m) = modules.iter_mut().find(|m| m.name == module) {
                    m.aliases.push(alias.to_string());
                }
            }
            Some("F") => {
                let (Some(module), Some(member), Some(symbol)) = (it.next(), it.next(), it.next())
                else {
                    continue;
                };
                let mut args: Vec<String> = Vec::new();
                let mut ret = "i64".to_string();
                let mut handle: Option<String> = None;
                let mut decl = true;
                let mut stub = false;
                let mut alias_of: Option<String> = None;
                for kv in it {
                    let Some((k, v)) = kv.split_once('=') else {
                        continue;
                    };
                    match k {
                        "args" => {
                            if !v.is_empty() {
                                args = v.split(',').map(|s| s.to_string()).collect();
                            }
                        }
                        "ret" => ret = v.to_string(),
                        "handle" => handle = Some(v.to_string()),
                        "decl" => decl = parse_bool_flag(v),
                        "stub" => stub = parse_bool_flag(v),
                        "alias-of" => alias_of = Some(v.to_string()),
                        _ => {}
                    }
                }
                if let Some(m) = modules.iter_mut().find(|m| m.name == module) {
                    m.members.push(PyMember {
                        name: member.to_string(),
                        symbol: symbol.to_string(),
                        args,
                        ret,
                        handle,
                        decl,
                        stub,
                        alias_of,
                    });
                } else {
                    eprintln!(
                        "warning: pylib/registry.txt:{}: member for undeclared module `{}`",
                        lineno + 1,
                        module
                    );
                }
            }
            Some("X") => {
                let Some(symbol) = it.next() else { continue };
                let mut args: Vec<String> = Vec::new();
                let mut ret = "i64".to_string();
                let mut decl = true;
                let mut stub = false;
                let mut alias_of: Option<String> = None;
                for kv in it {
                    let Some((k, v)) = kv.split_once('=') else { continue };
                    match k {
                        "args" => {
                            if !v.is_empty() {
                                args = v.split(',').map(|s| s.to_string()).collect();
                            }
                        }
                        "ret" => ret = v.to_string(),
                        "decl" => decl = parse_bool_flag(v),
                        "stub" => stub = parse_bool_flag(v),
                        "alias-of" => alias_of = Some(v.to_string()),
                        _ => {}
                    }
                }
                helpers.push(PyHelper {
                    symbol: symbol.to_string(),
                    args,
                    ret,
                    decl,
                    stub,
                    alias_of,
                });
            }
            Some("W") => {
                let (Some(handle), Some(method), Some(symbol)) = (it.next(), it.next(), it.next())
                else {
                    continue;
                };
                let mut ret_handle = None;
                let mut arity = 1usize;
                let mut ret = "i64".to_string();
                let mut decl = true;
                let mut stub = false;
                let mut alias_of: Option<String> = None;
                for kv in it {
                    let Some((k, v)) = kv.split_once('=') else {
                        // also accept ret_handle= via strip for back-compat
                        if let Some(vh) = kv.strip_prefix("ret_handle=") {
                            ret_handle = Some(vh.to_string());
                        } else if let Some(va) = kv.strip_prefix("args=") {
                            arity = va.parse().unwrap_or(1);
                        } else if let Some(vr) = kv.strip_prefix("ret=") {
                            ret = vr.to_string();
                        }
                        continue;
                    };
                    match k {
                        "ret_handle" => ret_handle = Some(v.to_string()),
                        "args" => arity = v.parse().unwrap_or(1),
                        "ret" => ret = v.to_string(),
                        "decl" => decl = parse_bool_flag(v),
                        "stub" => stub = parse_bool_flag(v),
                        "alias-of" => alias_of = Some(v.to_string()),
                        _ => {}
                    }
                }
                methods.push(PyMethod {
                    handle: handle.to_string(),
                    method: method.to_string(),
                    symbol: symbol.to_string(),
                    arity,
                    ret_handle,
                    ret,
                    decl,
                    stub,
                    alias_of,
                });
            }
            _ => {}
        }
    }
    Registry { modules, methods, helpers }
}

fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(parse_registry)
}

/// Resolve a module name (or accepted alias) to the canonical entry.
pub fn find_module(name: &str) -> Option<&'static PyModule> {
    registry()
        .modules
        .iter()
        .find(|m| m.name == name || m.aliases.iter().any(|a| a == name))
}

/// Resolve a module-level member.
///
/// Also handles submodule members: `from os.path import join` asks for
/// member `join` of module `os.path`, while the registry declares `os` with
/// member `path.join` — so `a.b` + `c` retries as `a` + `b.c` (recursively).
pub fn find_member(module: &str, member: &str) -> Option<&'static PyMember> {
    if let Some(m) = find_module(module).and_then(|m| m.members.iter().find(|x| x.name == member)) {
        return Some(m);
    }
    if let Some((prefix, rest)) = module.rsplit_once('.') {
        let combined = format!("{}.{}", rest, member);
        return find_member(prefix, &combined);
    }
    None
}

/// Method dispatch for library handles: (runtime symbol, result handle tag).
pub fn method_symbol(handle: &str, method: &str) -> Option<(&'static str, Option<&'static str>)> {
    registry()
        .methods
        .iter()
        .find(|m| m.handle == handle && m.method == method)
        .map(|m| (m.symbol.as_str(), m.ret_handle.as_deref()))
}

/// B4: when the receiver is `dyn` / untyped, look up a W-table method by name
/// alone — only if the name is unique across handles (otherwise keep guessing).
pub fn method_by_unique_name(
    method: &str,
) -> Option<(&'static str, &'static str, Option<&'static str>, &'static str)> {
    let hits: Vec<&PyMethod> = registry()
        .methods
        .iter()
        .filter(|m| m.method == method)
        .collect();
    if hits.len() != 1 {
        return None;
    }
    let m = hits[0];
    Some((
        m.handle.as_str(),
        m.symbol.as_str(),
        m.ret_handle.as_deref(),
        m.ret.as_str(),
    ))
}

/// Human-readable list of the known modules, for diagnostics.
pub fn known_module_names() -> Vec<&'static str> {
    registry().modules.iter().map(|m| m.name.as_str()).collect()
}

/// Every extern the shims need, with its exact signature — codegen declares
/// these instead of keeping a second hand-written list in sync.
pub fn all_externs() -> Vec<(&'static str, Vec<&'static str>, &'static str)> {
    let mut out: HashMap<&'static str, (Vec<&'static str>, &'static str)> = HashMap::new();
    for m in &registry().modules {
        for f in &m.members {
            if !f.decl || f.stub {
                continue;
            }
            let params: Vec<&'static str> = f.args.iter().map(|s| s.as_str()).collect();
            out.insert(f.symbol.as_str(), (params, f.ret.as_str()));
        }
    }
    // Methods take the receiver as their first argument (i64 handle).
    for m in &registry().methods {
        if !m.decl || m.stub {
            continue;
        }
        let params: Vec<&'static str> = std::iter::repeat_n("i64", m.arity).collect();
        out.entry(m.symbol.as_str())
            .or_insert((params, m.ret.as_str()));
    }
    for h in &registry().helpers {
        if !h.decl || h.stub {
            continue;
        }
        let p: Vec<&'static str> = h.args.iter().map(|s| s.as_str()).collect();
        out.insert(h.symbol.as_str(), (p, h.ret.as_str()));
    }
    let mut v: Vec<_> = out
        .into_iter()
        .filter(|(sym, _)| sym.starts_with("py_"))
        .map(|(sym, (params, ret))| (sym, params, ret))
        .collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    v
}

/// Where installed third-party packages live: `$ZETA_PACKAGES_DIR`, else
/// `~/.zeta/packages`. `zorb install` writes here and the import loader
/// searches here, so the two cannot drift.
pub fn packages_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("ZETA_PACKAGES_DIR") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".zeta/packages")
}

/// Is this a no-op module (`N` directive)? Its members are never validated
/// and never bound; the import is simply accepted.
pub fn is_noop_module(module: &str) -> bool {
    find_module(module).map(|m| m.noop).unwrap_or(false)
}

/// Result type of a handle method ("i64" | "f64" | "str"), for MIR typing.
pub fn method_ret(handle: &str, method: &str) -> Option<&'static str> {
    registry()
        .methods
        .iter()
        .find(|m| m.handle == handle && m.method == method)
        .map(|m| m.ret.as_str())
}

/// Operator dispatch for library handles (datetime date/timedelta arithmetic
/// and comparisons). Returns (symbol, result kind) where kind is
/// "date" | "delta" | "bool" | "i64".
pub fn handle_op(op: &str, left: &str, right: &str) -> Option<(&'static str, &'static str)> {
    // pathlib: `Path / "sub"` and `Path / Path` both join.
    if op == "/" && left == "PyPath" && (right == "PyPath" || right == "str") {
        return Some(("py_os_path_join", "path"));
    }
    let is_dt = |t: &str| t == "PyDate" || t == "PyDelta";
    if !is_dt(left) || !is_dt(right) {
        return None;
    }
    match op {
        "-" if left == "PyDate" && right == "PyDate" => Some(("py_dt_sub_dates", "delta")),
        "-" if left == "PyDate" && right == "PyDelta" => Some(("py_dt_sub_delta", "date")),
        "+" if left == "PyDate" && right == "PyDelta" => Some(("py_dt_add_delta", "date")),
        "+" if left == "PyDelta" && right == "PyDate" => Some(("py_dt_add_delta", "date")),
        "-" if left == "PyDelta" && right == "PyDelta" => Some(("py_dt_sub_dates", "delta")),
        "<" => Some(("py_dt_lt", "bool")),
        "<=" => Some(("py_dt_le", "bool")),
        ">" => Some(("py_dt_gt", "bool")),
        ">=" => Some(("py_dt_ge", "bool")),
        "==" => Some(("py_dt_eq", "bool")),
        "!=" => Some(("py_dt_ne", "bool")),
        _ => None,
    }
}

/// The handle tag of a MIR type, when it is a library handle.
pub fn handle_tag(t: &str) -> Option<&'static str> {
    // Any W-table receiver tag (PyPath, PyJson, DataFrame, …) — previously
    // only PyDate/PyDelta were listed, so call-site upgrades to PyPath left
    // the param as I64 and `p.exists()` emitted a bare `_exists`.
    if registry().methods.iter().any(|m| m.handle == t) {
        // Leak-free: return a static if we know it, else intern via the
        // first matching method's handle String (already 'static via registry).
        let h = registry()
            .methods
            .iter()
            .find(|m| m.handle == t)
            .map(|m| m.handle.as_str())
            .unwrap();
        return Some(h);
    }
    // The PYTHON spelling of the class the library constructs — `-> Path` /
    // `def f(p: Path)` rather than the tag `PyPath`. `F <module> <Class> …
    // handle=<Tag>` is exactly that mapping, so teach this lookup about it:
    // without it a user annotation stayed `Named("Path")`, method dispatch
    // found no user struct and no handle tag, and emitted `Path::open` →
    // undefined `_Path__open` (t232).
    for m in &registry().modules {
        for f in &m.members {
            if f.name == t {
                if let Some(h) = &f.handle {
                    return Some(h.as_str());
                }
            }
        }
    }
    None
}

/// A1: symbols marked `stub=1` (intentionally unimplemented / rewrite-only).
pub fn stub_symbols() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for m in &registry().modules {
        for f in &m.members {
            if f.stub {
                v.push(f.symbol.as_str());
            }
        }
    }
    for m in &registry().methods {
        if m.stub {
            v.push(m.symbol.as_str());
        }
    }
    for h in &registry().helpers {
        if h.stub {
            v.push(h.symbol.as_str());
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// D4: `# stub: <name>` markers in `pylib/*.z` (soft: prefix kept in the name).
pub fn pylib_file_stubs() -> Vec<&'static str> {
    static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut v = Vec::new();
        for src in [
            include_str!("../../pylib/numpy.z"),
            include_str!("../../pylib/pandas.z"),
        ] {
            for line in src.lines() {
                let t = line.trim();
                let rest = if let Some(r) = t.strip_prefix("# stub:") {
                    r
                } else if let Some(r) = t.strip_prefix("// stub:") {
                    r
                } else {
                    continue;
                };
                let name = rest.trim();
                if !name.is_empty() {
                    // Leak so we can return &'static str without holding the file.
                    v.push(Box::leak(name.to_string().into_boxed_str()) as &'static str);
                }
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    })
    .clone()
}

/// D4: registry stub=1 ∪ pylib `# stub:` markers (for `--list-stubs`).
pub fn all_stub_symbols() -> Vec<&'static str> {
    let mut v = stub_symbols();
    v.extend(pylib_file_stubs());
    v.sort_unstable();
    v.dedup();
    v
}

/// A4: map a registry symbol (or its `alias-of`) to the canonical declared name.
/// `decl=1` py_* entries (including stub=1 abort wrappers) — same set
/// `declare_registry_runtime_fns` emits after task D.
pub fn lookup_declared_symbol(name: &str) -> Option<&'static str> {
    static INDEX: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let idx = INDEX.get_or_init(|| {
        let mut m = HashMap::new();
        let reg = registry();
        let insert = |m: &mut HashMap<&'static str, &'static str>,
                      symbol: &'static str,
                      alias_of: Option<&'static str>| {
            if !symbol.starts_with("py_") {
                return;
            }
            let canon = alias_of.unwrap_or(symbol);
            m.insert(symbol, canon);
        };
        for mod_ in &reg.modules {
            for f in &mod_.members {
                if !f.decl {
                    continue;
                }
                insert(
                    &mut m,
                    f.symbol.as_str(),
                    f.alias_of.as_deref(),
                );
            }
        }
        for meth in &reg.methods {
            if !meth.decl {
                continue;
            }
            insert(
                &mut m,
                meth.symbol.as_str(),
                meth.alias_of.as_deref(),
            );
        }
        for h in &reg.helpers {
            if !h.decl {
                continue;
            }
            insert(
                &mut m,
                h.symbol.as_str(),
                h.alias_of.as_deref(),
            );
        }
        m
    });
    idx.get(name).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asdict_is_stub_declared() {
        let m = find_member("dataclasses", "asdict").expect("asdict registered");
        assert!(m.stub, "asdict must be stub=1");
        assert!(m.decl, "asdict abort wrapper is declared (task D)");
        assert_eq!(m.symbol, "py_asdict_unexpanded");
        assert!(stub_symbols().contains(&"py_asdict_unexpanded"));
    }

    #[test]
    fn pathlib_path_still_declared() {
        let m = find_member("pathlib", "Path").expect("Path");
        assert!(m.decl);
        assert!(!m.stub);
    }

    #[test]
    fn lookup_declared_symbol_hits_path_new() {
        assert_eq!(lookup_declared_symbol("py_path_new"), Some("py_path_new"));
        assert_eq!(
            lookup_declared_symbol("py_asdict_unexpanded"),
            Some("py_asdict_unexpanded")
        ); // stub=1 but decl=1
        assert!(lookup_declared_symbol("not_a_runtime_fn").is_none());
    }

    #[test]
    fn pylib_file_stubs_include_date_range() {
        let files = pylib_file_stubs();
        assert!(
            files.iter().any(|s| *s == "pandas.date_range"),
            "expected pandas.date_range in {:?}",
            files
        );
        let all = all_stub_symbols();
        assert!(all.contains(&"py_asdict_unexpanded"));
        assert!(all.iter().any(|s| s.contains("pandas.date_range")));
    }
}
