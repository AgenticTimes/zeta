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

/// Run LLVM's -O3 IR optimization pipeline using the new pass manager.
/// This promotes allocas to SSA (mem2reg), runs instcombine, GVN,
/// loop optimizations, and the full -O3 pipeline.
/// Uses LLVMRunPasses C API directly (LLVM 17+ new PM).
fn optimize_module<'ctx>(module: &inkwell::module::Module<'ctx>, target_machine: &TargetMachine) {
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

        // Debug: print module IR
        // self.module.print_to_stderr();

        let ee = self
            .module
            .create_jit_execution_engine(OptimizationLevel::Aggressive)?;

        // Static LLVM→Rust host mappings (pylib/jit_mappings.txt).
        super::jit_mappings_gen::register_jit_mappings(&self.module, &ee);

        // Map monomorphized vec_* functions
        for func_name in self.module.get_functions() {
            let name = func_name.get_name().to_str().unwrap().to_string();
            if name.starts_with("vec_push_") {
                ee.add_global_mapping(
                    &func_name,
                    crate::runtime::vec::zeta_vec_push as *const () as usize,
                );
            }
            if name.starts_with("vec_get_") {
                ee.add_global_mapping(
                    &func_name,
                    crate::runtime::vec::zeta_vec_get as *const () as usize,
                );
            }
            if name.starts_with("vec_len_") {
                ee.add_global_mapping(
                    &func_name,
                    crate::runtime::vec::zeta_vec_len as *const () as usize,
                );
            }
        }

        Ok(ee)
    }
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
    let target_machine = target
        .create_target_machine(
            &target_triple,
            &cpu,
            &features,
            OptimizationLevel::Aggressive,
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
