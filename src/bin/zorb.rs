//! zorb — minimal package installer (V1).
//!
//! The laziest thing that makes third-party code usable by the compiler:
//! put the source where the import loader looks for it. `zorb install` copies
//! (or fetches) a package into the packages directory, and the compiler's
//! module search finds it — no index, no dependency resolution, no lock file.
//!
//! Usage:
//!   zorb install <path | http(s)-url | git-url>   install a package/module
//!   zorb list                                     show installed packages
//!   zorb remove <name>                            uninstall
//!   zorb path                                     print the packages directory
//!
//! Accepted inputs:
//!   - a directory containing `__init__.py`/`__init__.z` (a package)
//!   - a single `X.py` / `X.z` file (a plain module)
//!   - an http(s) URL to such a file (fetched with the system `curl`)
//!   - a git URL (cloned with the system `git`, installed from its root)
//!
//! ponytail: no versions, no dependency resolution, no checksums. Add them
//! when a real consumer needs them; the compiler only needs the files to be
//! on the search path.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Same location the compiler searches (`ZETA_PACKAGES_DIR` or `~/.zeta/packages`).
fn packages_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ZETA_PACKAGES_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".zeta/packages")
}

fn die(msg: String) -> ! {
    eprintln!("zorb: error: {}", msg);
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    match cmd {
        "install" | "add" | "i" => {
            let Some(src) = args.get(1) else {
                die("usage: zorb install <path|url|git-url>".into());
            };
            install(src);
        }
        "list" | "ls" => list(),
        "remove" | "rm" | "uninstall" => {
            let Some(name) = args.get(1) else {
                die("usage: zorb remove <name>".into());
            };
            remove(name);
        }
        "path" => println!("{}", packages_dir().display()),
        "" | "help" | "--help" | "-h" => usage(),
        other => die(format!(
            "unknown command `{}` (try: install / list / remove / path)",
            other
        )),
    }
}

fn usage() {
    println!(
        "zorb — minimal package installer for Zeta\n\
         \n\
         usage:\n\
         \x20 zorb install <path|url|git-url>   install a package or module\n\
         \x20 zorb list                         show installed packages\n\
         \x20 zorb remove <name>                uninstall\n\
         \x20 zorb path                         print the packages directory\n\
         \n\
         accepted inputs: a directory with __init__.py/.z, a single X.py/.z,\n\
         an http(s) URL (uses curl), or a git URL (uses git clone).\n\
         installs into $ZETA_PACKAGES_DIR or ~/.zeta/packages — the same\n\
         directory the compiler searches when it sees `import X`."
    );
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn is_git_url(s: &str) -> bool {
    is_url(s) && (s.ends_with(".git") || s.starts_with("git@")) || s.starts_with("git@")
}

fn install(src: &str) {
    let staging = std::env::temp_dir().join(format!("zorb-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);

    if is_git_url(src) {
        if let Err(e) = fs::create_dir_all(&staging) {
            die(format!("cannot create staging dir: {}", e));
        }
        let ok = Command::new("git")
            .args(["clone", "--depth", "1", src])
            .arg(&staging)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            die(format!("git clone failed for {}", src));
        }
        install_from(&staging, src);
        return;
    }

    if is_url(src) {
        if let Err(e) = fs::create_dir_all(&staging) {
            die(format!("cannot create staging dir: {}", e));
        }
        let name = src.rsplit('/').next().unwrap_or("download");
        let dest = staging.join(name);
        let ok = Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&dest)
            .arg(src)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            die(format!("curl failed for {} (is curl installed?)", src));
        }
        install_from(&dest, src);
        return;
    }

    let path = PathBuf::from(src);
    if !path.exists() {
        die(format!("no such file or directory: {}", src));
    }
    install_from(&path, src);
}

fn install_from(path: &Path, origin: &str) {
    let root = packages_dir();
    if let Err(e) = fs::create_dir_all(&root) {
        die(format!("cannot create {}: {}", root.display(), e));
    }

    if path.is_dir() {
        // A directory package needs an entry module, otherwise `import` would
        // find the directory and fail confusingly later.
        let entry_py = path.join("__init__.py");
        let entry_z = path.join("__init__.z");
        if !entry_py.is_file() && !entry_z.is_file() {
            die(format!(
                "{} is not a package: expected __init__.py or __init__.z",
                path.display()
            ));
        }
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "package".to_string());
        let dest = root.join(&name);
        if dest.exists() {
            let _ = fs::remove_dir_all(&dest);
        }
        if let Err(e) = copy_dir(path, &dest) {
            die(format!("copy failed: {}", e));
        }
        println!("installed {} -> {}  (from {})", name, dest.display(), origin);
        println!("import it with:  import {}", name);
        return;
    }

    // Plain module file: X.py / X.z
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let base = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if base.is_empty() {
        die(format!("cannot derive a module name from {}", path.display()));
    }
    let dest = root.join(&name);
    if let Err(e) = fs::copy(path, &dest) {
        die(format!("copy failed: {}", e));
    }
    println!("installed {} -> {}  (from {})", name, dest.display(), origin);
    println!("import it with:  import {}", base);
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let src = e.path();
        let dst = to.join(e.file_name());
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

fn list() {
    let root = packages_dir();
    if !root.is_dir() {
        println!("(no packages installed; {})", root.display());
        return;
    }
    let mut names: Vec<String> = fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    if names.is_empty() {
        println!("(no packages installed; {})", root.display());
        return;
    }
    println!("{} package(s) in {}:", names.len(), root.display());
    for n in names {
        let p = root.join(&n);
        let kind = if p.is_dir() { "package" } else { "module" };
        println!("  {} ({})", n, kind);
    }
}

fn remove(name: &str) {
    let root = packages_dir();
    let dir = root.join(name);
    let file_py = root.join(format!("{}.py", name));
    let file_z = root.join(format!("{}.z", name));
    let mut removed = false;
    if dir.is_dir() {
        if let Err(e) = fs::remove_dir_all(&dir) {
            die(format!("cannot remove {}: {}", dir.display(), e));
        }
        removed = true;
    }
    for f in [&file_py, &file_z] {
        if f.is_file() {
            if let Err(e) = fs::remove_file(f) {
                die(format!("cannot remove {}: {}", f.display(), e));
            }
            removed = true;
        }
    }
    if removed {
        println!("removed {}", name);
    } else {
        die(format!("{} is not installed", name));
    }
}
