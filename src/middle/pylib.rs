//! Python standard-library shim registry (V1).
//!
//! `import X` / `from X import y` no longer get swallowed: the parser emits
//! `zeta_py_import` / `zeta_py_from` markers, the Resolver validates them
//! against this table (fail-loud on unknown modules — a silent no-op import
//! could otherwise compile into a program that reads garbage), and MirGen
//! maps module member calls (`threading.Thread(...)`,
//! `from threading import Thread; Thread(...)`) onto runtime shims.
//!
//! The concurrency modules are thin shims over the existing native
//! primitives (pthread spawn/join, mutexes, channels, atomics) — see
//! runtime/tokio_runtime_stub.c.

/// A module we know how to lower. `members` are module-level names.
pub struct PyModule {
    pub name: &'static str,
    /// Extra aliases accepted in `import` (e.g. `futures` for
    /// `concurrent.futures`).
    pub aliases: &'static [&'static str],
    pub members: &'static [PyMember],
}

pub struct PyMember {
    pub name: &'static str,
    pub symbol: &'static str,
    /// Handle type tag stored in the MIR type map, so method dispatch on the
    /// returned handle is exact instead of name-guessing.
    pub handle: Option<&'static str>,
    /// MIR result type: "i64" (default) or "f64".
    pub ret: &'static str,
}

const THREADING: PyModule = PyModule {
    name: "threading",
    aliases: &[],
    members: &[
        PyMember { name: "Thread", symbol: "py_threading_thread_new", handle: Some("PyThread"), ret: "i64" },
        PyMember { name: "Lock", symbol: "py_threading_lock_new", handle: Some("PyLock"), ret: "i64" },
        PyMember { name: "RLock", symbol: "py_threading_lock_new", handle: Some("PyLock"), ret: "i64" },
        PyMember { name: "current_thread", symbol: "py_threading_current_thread", handle: None, ret: "i64" },
        PyMember { name: "get_ident", symbol: "py_threading_get_ident", handle: None, ret: "i64" },
        PyMember { name: "active_count", symbol: "py_threading_active_count", handle: None, ret: "i64" },
    ],
};

const FUTURES: PyModule = PyModule {
    name: "concurrent.futures",
    aliases: &["futures"],
    members: &[
        PyMember { name: "ThreadPoolExecutor", symbol: "py_futures_executor_new", handle: Some("PyExecutor"), ret: "i64" },
        PyMember { name: "ProcessPoolExecutor", symbol: "py_futures_executor_new", handle: Some("PyExecutor"), ret: "i64" },
    ],
};

const MULTIPROCESSING: PyModule = PyModule {
    name: "multiprocessing",
    aliases: &[],
    members: &[
        PyMember { name: "Process", symbol: "py_mp_process_new", handle: Some("PyProcess"), ret: "i64" },
        PyMember { name: "Pool", symbol: "py_mp_pool_new", handle: Some("PyPool"), ret: "i64" },
        PyMember { name: "current_process", symbol: "py_mp_current_process", handle: None, ret: "i64" },
        PyMember { name: "cpu_count", symbol: "py_mp_cpu_count", handle: None, ret: "i64" },
    ],
};

const ASYNCIO: PyModule = PyModule {
    name: "asyncio",
    aliases: &[],
    members: &[
        PyMember { name: "run", symbol: "py_asyncio_run", handle: None, ret: "i64" },
        PyMember { name: "sleep", symbol: "py_asyncio_sleep", handle: None, ret: "i64" },
        PyMember { name: "create_task", symbol: "py_asyncio_run", handle: None, ret: "i64" },
        PyMember { name: "ensure_future", symbol: "py_asyncio_run", handle: None, ret: "i64" },
    ],
};

/// `time` is not a concurrency library, but every threading example uses
/// `time.sleep()`; the shim is three lines over the existing clock.
const TIME: PyModule = PyModule {
    name: "time",
    aliases: &[],
    members: &[
        PyMember { name: "sleep", symbol: "py_time_sleep", handle: None, ret: "i64" },
        PyMember { name: "time", symbol: "py_time_time", handle: None, ret: "f64" },
        PyMember { name: "monotonic", symbol: "py_time_monotonic", handle: None, ret: "f64" },
        PyMember { name: "perf_counter", symbol: "py_time_monotonic", handle: None, ret: "f64" },
    ],
};

pub const MODULES: &[PyModule] = &[THREADING, FUTURES, MULTIPROCESSING, ASYNCIO, TIME];

/// Method dispatch for library handles, keyed by handle tag + method name.
/// Returns (runtime symbol, handle tag of the result) — e.g. `submit`
/// returns a Future handle, so `.result()` dispatches exactly.
pub fn method_symbol(handle: &str, method: &str) -> Option<(&'static str, Option<&'static str>)> {
    let table: &[(&str, &str, &str, Option<&'static str>)] = &[
        // threading.Thread
        ("PyThread", "start", "py_threading_thread_start", None),
        ("PyThread", "join", "py_threading_thread_join", None),
        ("PyThread", "is_alive", "py_threading_thread_is_alive", None),
        // threading.Lock
        ("PyLock", "acquire", "py_threading_lock_acquire", None),
        ("PyLock", "release", "py_threading_lock_release", None),
        ("PyLock", "locked", "py_threading_lock_locked", None),
        // concurrent.futures.Executor / Future
        ("PyExecutor", "submit", "py_futures_submit", Some("PyFuture")),
        ("PyExecutor", "map", "py_futures_map", None),
        ("PyExecutor", "shutdown", "py_futures_shutdown", None),
        ("PyFuture", "result", "py_futures_result", None),
        ("PyFuture", "done", "py_futures_done", None),
        // multiprocessing.Process / Pool
        ("PyProcess", "start", "py_mp_process_start", None),
        ("PyProcess", "join", "py_mp_process_join", None),
        ("PyProcess", "is_alive", "py_mp_process_is_alive", None),
        ("PyProcess", "exitcode", "py_mp_process_exitcode", None),
        ("PyPool", "map", "py_mp_pool_map", None),
        ("PyPool", "apply", "py_mp_pool_apply", None),
        ("PyPool", "close", "py_mp_pool_close", None),
        ("PyPool", "join", "py_mp_pool_join", None),
    ];
    table
        .iter()
        .find(|(t, m, _, _)| *t == handle && *m == method)
        .map(|(_, _, s, h)| (*s, *h))
}

/// Resolve a module name (or accepted alias) to the canonical entry.
pub fn find_module(name: &str) -> Option<&'static PyModule> {
    MODULES
        .iter()
        .find(|m| m.name == name || m.aliases.contains(&name))
}

/// Resolve a module-level member to its runtime symbol.
pub fn find_member(module: &str, member: &str) -> Option<&'static PyMember> {
    find_module(module)
        .and_then(|m| m.members.iter().find(|x| x.name == member))
}

/// Human-readable list of the known modules, for diagnostics.
pub fn known_module_names() -> Vec<&'static str> {
    MODULES.iter().map(|m| m.name).collect()
}
