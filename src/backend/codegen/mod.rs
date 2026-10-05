// src/backend/codegen/mod.rs
#![allow(clippy::module_inception)] // Learning: Module named same as parent is common pattern in Rust
mod codegen;
mod jit;
mod monomorphize;
// 批 990（提案①第一段）：C 运行时签名表＋keyfn 指针合同（手写，非生成）。
mod signature_table;
// 批 997（提案①全表覆盖）：注册表生成的签名表段（--emit-sigtable）。
mod signature_table_gen;
// 批 1000（轴 D 第二刀）：keyfn 家族（W0911 核对＋FuncAddr 兜底臂）
// 自 codegen.rs 迁出。
mod codegen_keyfn;
// 批 1002（轴 D 第二刀续）：控制流族（If/While/For）迁出。
mod codegen_stmt_flow;
// 批 1003/1004（轴 D 第二刀续）：Call 臂三终态子族＋11 个自含子族迁出。
mod codegen_call_arm;
// @generated — regenerate via `python3 tools/gen_from_registry.py --emit`
mod runtime_decls_registry;
// @generated — regenerate via `python3 tools/gen_from_registry.py --emit-core`
mod runtime_decls_core;
// @generated — regenerate via `python3 tools/gen_from_registry.py --emit-jit`
mod jit_mappings_gen;
// SIMD operations are now handled inline in codegen.rs via LLVM vector IR.
// No separate module needed — vectors are stack-allocated and operated on
// as LLVM native vector types.

pub use codegen::LLVMCodegen;
pub use jit::*;
pub use monomorphize::*;
