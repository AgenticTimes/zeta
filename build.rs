// build.rs — 批次 759（#26 根）：把 runtime/*.c 那组 C 运行期按 tools/build_runtime.sh
// 的配方新鲜编进 OUT_DIR，并把合并后的两颗 .o 直接链进 zetac 可执行文件。
// jit.rs 的 `defined_in_process_image` 走 dlopen(NULL)+dlsym，符号进了进程镜像即可命中，
// JIT 模式（无 -o）从此不再对整族 C 运行期符号打 E4016 陷阱；jit.rs 本体零改动。
// 失败时只放弃嵌入（维持改前"无绑定"行为），不把整棵 crate 的构建打死。

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let sources = [
        "runtime/py_additions.c",
        "runtime/parquet_min.c",
        "runtime/tokio_runtime_stub.c",
        "runtime/unavailable_stubs.c",
        "tokio_runtime.c",
        "tools/build_runtime.sh",
    ];
    for s in sources {
        println!("cargo:rerun-if-changed={s}");
    }

    if env::var("CARGO_CFG_TARGET_OS").map(|os| os != "macos").unwrap_or(true) {
        println!("cargo:warning=非 mac 目标：跳过 C 运行期嵌入（配方按 /opt/homebrew 写死）");
        return;
    }
    if Command::new("clang").arg("--version").output().is_err() {
        println!("cargo:warning=PATH 上没有 clang：跳过 C 运行期嵌入");
        return;
    }

    let o_str = |name: &str| out.join(name).to_str().unwrap().to_string();
    let inc = ["-I/opt/homebrew/include", "-Iruntime"];

    let cc = |args: &[&str]| -> bool {
        let mut full: Vec<&str> = vec!["-c"];
        full.extend(args);
        Command::new("clang")
            .args(&full)
            .current_dir(&manifest)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    // 与 tools/build_runtime.sh 同一步骤序；任一必需步失败即放弃（改前行为）。
    if !cc(&["-O2", inc[0], inc[1], "-DZT_REAL_ASYNC",
             "runtime/tokio_runtime_stub.c", "-o", &o_str("zt_stub.o")]) {
        println!("cargo:warning=runtime/tokio_runtime_stub.c 编译失败：跳过嵌入");
        return;
    }
    let mut tokio_parts = vec![o_str("zt_stub.o")];
    if manifest.join("runtime/unavailable_stubs.c").exists() {
        let p = o_str("zt_unavail.o");
        if cc(&["-O1", inc[0], inc[1], "runtime/unavailable_stubs.c", "-o", &p]) {
            tokio_parts.push(p);
        }
    }
    if manifest.join("tokio_runtime.c").exists() {
        let p = o_str("zt_async.o");
        if cc(&["-O2", inc[0], inc[1], "-DZT_REAL_ASYNC", "tokio_runtime.c", "-o", &p]) {
            tokio_parts.insert(0, p);
        } else {
            eprintln!("note: tokio_runtime.c 编译不过，按 build_runtime.sh 走 stub-only");
        }
    }
    if !ld_r(&tokio_parts, &o_str("zt_tokio_runtime.o")) {
        println!("cargo:warning=tokio 侧 ld -r 合并失败：跳过嵌入");
        return;
    }

    if !cc(&["-O2", inc[0], inc[1], "runtime/py_additions.c", "-o", &o_str("zt_c_runtime.o")]) {
        println!("cargo:warning=runtime/py_additions.c 编译失败：跳过嵌入");
        return;
    }
    let mut c_parts = vec![o_str("zt_c_runtime.o")];
    let pq = o_str("zt_pq.o");
    if cc(&["-O2", inc[0], inc[1], "runtime/parquet_min.c", "-o", &pq]) {
        c_parts.push(pq);
    } else {
        eprintln!("note: runtime/parquet_min.c 编译不过，只链 py_additions");
    }
    if !ld_r(&c_parts, &o_str("zt_zeta_runtime_c.o")) {
        println!("cargo:warning=C 侧 ld -r 合并失败：跳过嵌入");
        return;
    }

    // 直接给 .o 而非 .a：静态库里未被引用的成员会被链接器整个丢掉。
    // rustc 在 mac 上无条件传 -Wl,-dead_strip；C 符号与 Rust 代码无静态引用链会被剥掉
    // （实验 /tmp/b759/exp：plain 与 alias 档 dlsym 命中，裸 dead_strip 档三枚全 0x0）。
    // -alias 让目标成为 dead-strip 根：_keep_* 别名把原符号整体钉进镜像。
    let mut keep: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for obj in ["zt_zeta_runtime_c.o", "zt_tokio_runtime.o"] {
        let nm = Command::new("nm")
            .args(["-g", "-U", "-j"])
            .arg(out.join(obj))
            .output()
            .expect("nm 不可用");
        if !nm.status.success() {
            println!("cargo:warning=nm 读取 {obj} 失败：跳过嵌入");
            return;
        }
        for line in String::from_utf8_lossy(&nm.stdout).lines() {
            let line = line.trim();
            if !line.is_empty() {
                keep.insert(line.to_string());
            }
        }
    }
    let c_objs = [
        format!("cargo:rustc-link-arg-bin=zetac={}", o_str("zt_zeta_runtime_c.o")),
        format!("cargo:rustc-link-arg-bin=zetac={}", o_str("zt_tokio_runtime.o")),
    ];
    let aliases: Vec<String> = keep
        .iter()
        .map(|sym| format!("cargo:rustc-link-arg-bin=zetac=-Wl,-alias,{sym},_ztk_keep{sym}"))
        .collect();
    let tail = [
        "cargo:rustc-link-arg-bin=zetac=-L/opt/homebrew/opt/bdw-gc/lib".to_string(),
        "cargo:rustc-link-arg-bin=zetac=-lgc".to_string(),
    ];
    for arg in c_objs.iter().chain(aliases.iter()).chain(tail.iter()) {
        println!("{arg}");
    }
}

fn ld_r(parts: &[String], out_path: &str) -> bool {
    let mut cmd = Command::new("ld");
    cmd.arg("-r").args(parts).args(["-o", out_path]);
    cmd.status().map(|s| s.success()).unwrap_or(false)
}
