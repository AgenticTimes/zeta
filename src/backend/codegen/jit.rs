// src/backend/codegen/jit.rs
//! # JIT & AOT Finalization
//!
//! Final stage of compilation: optimization, execution engine creation, and runtime mapping.
//! Clean, minimal, and production-ready.

use inkwell::OptimizationLevel;
use inkwell::execution_engine::ExecutionEngine;
use inkwell::targets::{FileType, InitializationConfig, Target, TargetMachine, TargetTriple};
use std::error::Error;
use std::ffi::CString;
use std::fs;
use std::path::Path;

/// The `vec_*` families JIT mode binds to the Rust runtime, as (prefix, host fn).
/// `finalize_and_jit` registers from this table and `jit_symbol_resolvable`
/// predicts from it, so the two cannot drift apart.
const JIT_VEC_BINDINGS: &[(&str, *const ())] = &[
    ("vec_push_", crate::runtime::vec::zeta_vec_push as *const ()),
    ("vec_get_", crate::runtime::vec::zeta_vec_get as *const ()),
    ("vec_len_", crate::runtime::vec::zeta_vec_len as *const ()),
];

/// Can the execution engine from `finalize_and_jit` give `name` an address?
///
/// MCJIT has exactly two ways to settle an external here: the bindings made by
/// `finalize_and_jit` (the generated table + `JIT_VEC_BINDINGS`), and a plain
/// `dlsym` over the images already loaded in this process — which is the
/// compiler's own image, since nothing links `runtime/*.c` into `zetac` (there
/// is no build script). A Python-style module binding therefore has no address:
/// `zeta_env_set`/`zeta_env_get`/`zeta_module_decl`/`zeta_nonlocal_decl`/
/// `zeta_param_default` are defined only in `runtime/py_additions.c`.
///
/// And asking the engine is not a safe way to find that out: lookup lazily runs
/// `MCJIT::finalizeLoadedModules -> RuntimeDyldImpl::resolveRelocations`, which
/// faults instead of reporting (`EXC_BAD_ACCESS at 0xb0` inside
/// `pthread_mutex_lock`, measured under lldb). Predict, never probe.
fn jit_symbol_resolvable(name: &str) -> bool {
    JIT_VEC_BINDINGS.iter().any(|(p, _)| name.starts_with(p))
        || super::jit_mappings_gen::JIT_MAPPINGS
            .iter()
            .any(|(n, _)| *n == name)
        || match std::ffi::CString::new(name) {
            Ok(c) => defined_in_process_image(c.as_ptr()),
            Err(_) => false,
        }
}

/// Is `sym` defined by an image this process already loaded? That is the scope
/// MCJIT's own host lookup searches, so "yes" means the engine will settle it.
///
/// `dlopen(NULL)` hands back a handle for exactly that scope. `RTLD_DEFAULT`
/// names the same thing but is a *macro* with no portable Rust spelling: Apple
/// needs `-2`, which `libc` does not export there, and `libc` spells it `NULL` on
/// Linux — and a bad handle does not report an error, it just finds nothing,
/// which would read as "no external binds" and trap programs that run fine.
fn defined_in_process_image(sym: *const libc::c_char) -> bool {
    unsafe {
        let image = libc::dlopen(std::ptr::null(), libc::RTLD_LAZY);
        if image.is_null() {
            return false;
        }
        let found = !libc::dlsym(image, sym).is_null();
        libc::dlclose(image);
        found
    }
}

/// Run LLVM's -O3 IR optimization pipeline using the new pass manager.
/// This promotes allocas to SSA (mem2reg), runs instcombine, GVN,
/// loop optimizations, and the full -O3 pipeline.
/// Uses LLVMRunPasses C API directly (LLVM 17+ new PM).
fn optimize_module<'ctx>(module: &inkwell::module::Module<'ctx>, target_machine: &TargetMachine) {
    // `ZETA_NO_OPT=1` skips the pipeline. Diagnostic only: it tells a
    // MISCOMPILE (the -O3 pipeline turns valid IR into something that traps)
    // apart from bad generated IR. Never use it for releases — the object is
    // ~5x bigger and slower.
    if std::env::var("ZETA_NO_OPT").is_ok() {
        return;
    }
    // Run the full -O3 pipeline on the module via LLVM's new PM pass builder
    unsafe {
        let pipeline = CString::new("default<O3>").unwrap();
        let options = llvm_sys::transforms::pass_builder::LLVMCreatePassBuilderOptions();

        let err = llvm_sys::transforms::pass_builder::LLVMRunPasses(
            module.as_mut_ptr(),
            pipeline.as_ptr(),
            target_machine.as_mut_ptr(),
            options,
        );

        if !err.is_null() {
            // Get error message from LLVMErrorRef
            let msg_ptr = llvm_sys::error::LLVMGetErrorMessage(err);
            let msg = std::ffi::CStr::from_ptr(msg_ptr)
                .to_string_lossy()
                .into_owned();
            llvm_sys::error::LLVMConsumeError(err);
            #[cfg(debug_assertions)]
            eprintln!("[LLVM opt warning: {}]", msg);
        }
    }
}

impl<'ctx> crate::backend::codegen::LLVMCodegen<'ctx> {
    pub fn finalize_and_jit(
        &mut self,
        target_str: &str,
    ) -> Result<ExecutionEngine<'ctx>, Box<dyn Error>> {
        if target_str == "wasm32" {
            return Err("JIT not supported for WASM target. Use AOT compilation instead.".into());
        }
        self.module.verify()?;

        Target::initialize_native(&InitializationConfig::default())?;
        let target_triple = TargetMachine::get_default_triple();
        let target = Target::from_triple(&target_triple)?;
        let target_machine = target
            .create_target_machine(
                &target_triple,
                &TargetMachine::get_host_cpu_name().to_string(),
                &TargetMachine::get_host_cpu_features().to_string(),
                OptimizationLevel::Aggressive,
                inkwell::targets::RelocMode::Default,
                inkwell::targets::CodeModel::Default,
            )
            .ok_or("Failed to create target machine")?;

        self.module.set_triple(&target_triple);
        self.module
            .set_data_layout(&target_machine.get_target_data().get_data_layout());

        // Run the full LLVM optimization pipeline before JIT
        optimize_module(&self.module, &target_machine);

        // Turn "call a runtime symbol JIT cannot bind" into a report that names
        // the symbol, instead of a jump to address 0. Must happen before
        // `create_jit_execution_engine`, which is where the module is copied.
        let trapped = self.trap_unresolved_symbols();

        // Debug: print module IR
        // self.module.print_to_stderr();

        let ee = self
            .module
            .create_jit_execution_engine(OptimizationLevel::Aggressive)?;

        // Static LLVM→Rust host mappings (pylib/jit_mappings.txt).
        super::jit_mappings_gen::register_jit_mappings(&self.module, &ee);

        if !trapped.is_empty() {
            let reporter = self
                .module
                .get_function("zeta_jit_missing_symbol")
                .expect("installed by trap_unresolved_symbols");
            ee.add_global_mapping(&reporter, zeta_jit_missing_symbol as *const () as usize);
        }

        // Map monomorphized vec_* functions
        for func_name in self.module.get_functions() {
            let name = func_name.get_name().to_str().unwrap().to_string();
            for (prefix, ptr) in JIT_VEC_BINDINGS {
                if name.starts_with(prefix) {
                    ee.add_global_mapping(&func_name, *ptr as usize);
                }
            }
        }

        Ok(ee)
    }

    /// Give every external this program references but the engine cannot bind a
    /// body that reports itself.
    ///
    /// Why not simply refuse to compile instead: an unresolved call only traps
    /// when it is *executed*, and a call -O3 leaves in the IR may be dynamically
    /// dead — a static verdict wrongly blocked `test_advanced_patterns.z`, which
    /// does run under `--jit` today. Trapping keeps every program that works
    /// working, and turns the ones that died with SIGSEGV into a message naming
    /// the symbol. Returns the trapped names, which are also warned about, since
    /// a program that never reaches one still runs with a hole in it.
    fn trap_unresolved_symbols(&self) -> Vec<String> {
        use inkwell::module::Linkage;
        use inkwell::values::AsValueRef;

        let candidates: Vec<(String, inkwell::values::FunctionValue<'ctx>)> = self
            .module
            .get_functions()
            .filter(|f| {
                f.get_first_basic_block().is_none()
                    && !unsafe { llvm_sys::core::LLVMGetFirstUse(f.as_value_ref()) }.is_null()
            })
            .filter_map(|f| {
                let n = f.get_name().to_str().ok()?;
                (!n.starts_with("llvm.") && !jit_symbol_resolvable(n)).then(|| (n.to_string(), f))
            })
            .collect();
        if candidates.is_empty() {
            return Vec::new();
        }

        let reporter_type = self.i64_type.fn_type(&[self.ptr_type.into()], false);
        let reporter = self
            .module
            .add_function("zeta_jit_missing_symbol", reporter_type, None);

        for (idx, (name, f)) in candidates.iter().enumerate() {
            let text = format!("{name}\0");
            let name_global = self.module.add_global(
                self.context.i8_type().array_type(text.len() as u32),
                None,
                &format!("jit_missing_name_{idx}"),
            );
            name_global.set_initializer(&self.context.const_string(text.as_bytes(), false));
            name_global.set_linkage(Linkage::Private);
            name_global.set_constant(true);

            let block = self.context.append_basic_block(*f, "jit_missing");
            self.builder.position_at_end(block);
            let _ = self
                .builder
                .build_call(reporter, &[name_global.as_pointer_value().into()], "");
            self.builder.build_unreachable();
        }

        let names: Vec<String> = candidates.into_iter().map(|(n, _)| n).collect();
        eprintln!(
            "warning[E4016]: JIT mode (no -o) has no binding for {} runtime symbol(s) this \
             program references: [{}]. Each is replaced by a trap that names itself and exits 1 \
             if it is reached; compile with -o to get the real runtime.",
            names.len(),
            names.join(", ")
        );
        names
    }
}

/// What a trapped call runs: name the symbol, then leave with a non-zero status.
///
/// Only reachable through a body `trap_unresolved_symbols` installs over a
/// declaration, so `name` is that declaration's own name as a NUL-terminated C
/// string. `process::exit` diverges, which is why the declared `i64` result
/// needs no value.
unsafe extern "C" fn zeta_jit_missing_symbol(name: *const libc::c_char) -> i64 {
    let name = std::ffi::CStr::from_ptr(name).to_string_lossy();
    let diag = crate::error_codes::diagnostic_from_code(
        "E4016",
        format!("`{name}` has no binding in JIT mode, so calling it would jump to address 0"),
        None,
    );
    eprintln!("{}", diag.format(None));
    std::process::exit(1)
}

pub fn finalize_and_aot<'ctx>(
    codegen: &crate::backend::codegen::LLVMCodegen<'ctx>,
    path: &Path,
    target_str: &str,
) -> Result<(), Box<dyn Error>> {
    codegen.module.verify()?;

    let (triple, cpu, features) = if target_str == "wasm32" || target_str == "wasm32-wasi" {
        // WASM targets — initialize all targets to register WebAssembly
        Target::initialize_all(&InitializationConfig::default());
        let triple = if target_str == "wasm32-wasi" {
            "wasm32-wasi"
        } else {
            "wasm32-unknown-unknown"
        };
        (
            triple.to_string(),
            "generic".to_string(),
            "+bulk-memory,+simd128".to_string(),
        )
    } else if target_str == "x86-64" || target_str == "x86-64-v2" || target_str == "x86-64-v3" {
        // Generic x86-64 target — compatible with any x86-64 CPU
        Target::initialize_native(&InitializationConfig::default())?;
        let triple_str = TargetMachine::get_default_triple();
        (
            triple_str.as_str().to_str().unwrap_or("x86_64").to_string(),
            target_str.to_string(),
            String::new(),
        )
    } else {
        // Native target — optimized for host CPU
        Target::initialize_native(&InitializationConfig::default())?;
        // Use default triple directly (as TargetTriple, not String)
        let triple_str = TargetMachine::get_default_triple();
        (
            triple_str.as_str().to_str().unwrap_or("x86_64").to_string(),
            TargetMachine::get_host_cpu_name().to_string(),
            TargetMachine::get_host_cpu_features().to_string(),
        )
    };

    let target_triple = TargetTriple::create(&triple);
    let target = Target::from_triple(&target_triple)?;
    // `ZETA_NO_OPT=1` also disables the LLVM -O3 pipeline here. The IR printed by
    // `--emit-llvm` is the UNOPTIMIZED module, and a miscompile at -O3 can make the
    // assembly disagree with it (measured: `call i64 @"DataFrame::copy"(i64 %250)`
    // in the IR vs. a `bl DataFrame::copy` with NO argument load in the object).
    // Keep the debug switch honest end-to-end.
    let opt_level = if std::env::var("ZETA_NO_OPT").is_ok() {
        OptimizationLevel::None
    } else {
        OptimizationLevel::Aggressive
    };
    let target_machine = target
        .create_target_machine(
            &target_triple,
            &cpu,
            &features,
            opt_level,
            inkwell::targets::RelocMode::Default,
            inkwell::targets::CodeModel::Default,
        )
        .ok_or("Failed to create target machine")?;

    codegen.module.set_triple(&target_triple);
    codegen
        .module
        .set_data_layout(&target_machine.get_target_data().get_data_layout());

    // Run the full LLVM optimization pipeline before codegen
    optimize_module(&codegen.module, &target_machine);

    let buffer = target_machine.write_to_memory_buffer(&codegen.module, FileType::Object)?;
    fs::write(path, buffer.as_slice())?;
    Ok(())
}
