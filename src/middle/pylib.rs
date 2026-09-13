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
}

#[derive(Debug, Clone)]
pub struct PyModule {
    pub name: String,
    pub aliases: Vec<String>,
    pub members: Vec<PyMember>,
}

#[derive(Debug, Clone)]
pub struct PyMethod {
    pub handle: String,
    pub method: String,
    pub symbol: String,
    /// Argument count including the receiver handle.
    pub arity: usize,
    pub ret_handle: Option<String>,
}

struct Registry {
    modules: Vec<PyModule>,
    methods: Vec<PyMethod>,
}

fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(parse_registry)
}

fn parse_registry() -> Registry {
    let mut modules: Vec<PyModule> = Vec::new();
    let mut methods: Vec<PyMethod> = Vec::new();
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
                    });
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
                    });
                } else {
                    eprintln!(
                        "warning: pylib/registry.txt:{}: member for undeclared module `{}`",
                        lineno + 1,
                        module
                    );
                }
            }
            Some("W") => {
                let (Some(handle), Some(method), Some(symbol)) = (it.next(), it.next(), it.next())
                else {
                    continue;
                };
                let mut ret_handle = None;
                let mut arity = 1usize;
                for kv in it {
                    if let Some(v) = kv.strip_prefix("ret_handle=") {
                        ret_handle = Some(v.to_string());
                    } else if let Some(v) = kv.strip_prefix("args=") {
                        arity = v.parse().unwrap_or(1);
                    }
                }
                methods.push(PyMethod {
                    handle: handle.to_string(),
                    method: method.to_string(),
                    symbol: symbol.to_string(),
                    arity,
                    ret_handle,
                });
            }
            _ => {}
        }
    }
    Registry { modules, methods }
}

/// Resolve a module name (or accepted alias) to the canonical entry.
pub fn find_module(name: &str) -> Option<&'static PyModule> {
    registry()
        .modules
        .iter()
        .find(|m| m.name == name || m.aliases.iter().any(|a| a == name))
}

/// Resolve a module-level member.
pub fn find_member(module: &str, member: &str) -> Option<&'static PyMember> {
    find_module(module).and_then(|m| m.members.iter().find(|x| x.name == member))
}

/// Method dispatch for library handles: (runtime symbol, result handle tag).
pub fn method_symbol(handle: &str, method: &str) -> Option<(&'static str, Option<&'static str>)> {
    registry()
        .methods
        .iter()
        .find(|m| m.handle == handle && m.method == method)
        .map(|m| (m.symbol.as_str(), m.ret_handle.as_deref()))
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
            let params: Vec<&'static str> = f.args.iter().map(|s| s.as_str()).collect();
            out.insert(f.symbol.as_str(), (params, f.ret.as_str()));
        }
    }
    // Methods take the receiver as their first argument (i64 handle).
    for m in &registry().methods {
        let params: Vec<&'static str> = std::iter::repeat_n("i64", m.arity).collect();
        let ret = if m.symbol.starts_with("py_time_") { "f64" } else { "i64" };
        out.entry(m.symbol.as_str()).or_insert((params, ret));
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
