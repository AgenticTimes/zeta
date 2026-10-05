// src/backend/codegen/codegen_call_arm.rs
// 批 1003（轴 D 第二刀续）：gen_stmt Call 臂的三个终态子族自
// codegen.rs 迁出——try/except 的 _setjmp 面、__spawn_thunk_ 线程
// 面、array_get/stack_array_get 内联面。逐字搬迁（内层 return; 改
// return true; 表达"已处理"），语义零变（迁前/迁后 LLVM IR 基线
// 逐字节 diff 为空，纪律同批 1000/1002）。

use super::codegen::LLVMCodegen;
use crate::middle::mir::mir::MirExpr;
use crate::middle::types::Type;
use inkwell::AddressSpace;
use inkwell::module::Linkage;
use inkwell::values::BasicMetadataValueEnum;
use std::collections::HashMap;

impl<'ctx> LLVMCodegen<'ctx> {

    pub(super) fn emit_try_setjmp(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        if func.starts_with("zeta_try_setjmp") {
                    let slot_fn = self
                        .module
                        .get_function("zeta_try_slot")
                        .unwrap_or_else(|| {
                            self.module.add_function(
                                "zeta_try_slot",
                                self.ptr_type.fn_type(&[], false),
                                Some(Linkage::External),
                            )
                        });
                    let sj_fn = self
                        .module
                        .get_function("_setjmp")
                        .unwrap_or_else(|| {
                            let f = self.module.add_function(
                                "_setjmp",
                                self.context
                                    .i32_type()
                                    .fn_type(&[self.ptr_type.into()], false),
                                Some(Linkage::External),
                            );
                            let kind = inkwell::attributes::Attribute::get_named_enum_kind_id(
                                "returns_twice",
                            );
                            eprintln!("PROBE returns_twice kind_id={}", kind);
                            let attr = self.context.create_enum_attribute(kind, 0);
                            f.add_attribute(
                                inkwell::attributes::AttributeLoc::Function,
                                attr,
                            );
                            eprintln!("PROBE attr added kind={}", kind);
                            f
                        });
                    let slot = Self::call_site_to_basic_value(
                        self.builder.build_call(slot_fn, &[], "try_slot").unwrap(),
                    )
                    .unwrap();
                    let r32 = Self::call_site_to_basic_value(
                        self.builder.build_call(sj_fn, &[slot.into()], "try_setjmp").unwrap(),
                    )
                    .unwrap();
                    let r = self
                        .builder
                        .build_int_z_extend(
                            r32.into_int_value(),
                            self.i64_type,
                            "try_setjmp_ext",
                        )
                        .unwrap();
                    let alloca = *self.locals.get(&dest).unwrap();
                    self.builder.build_store(alloca, r).unwrap();
                    return true;
        }
        false
    }

    pub(super) fn emit_spawn_thunk(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        if func.starts_with("__spawn_thunk_") {
                    let real_fn_name = &func["__spawn_thunk_".len()..];
                    // Get (or declare) the thunk wrapper: signature (args...) -> i64
                    // plus a zero-arg entry the runtime can call on the thread.
                    // We use a two-step approach:
                    //   1. __spawn_thunk_<fn>(args) — real work, called by thread
                    //   2. __spawn_entry_<fn>()     — no-arg, captured args via
                    //      global buffer; the runtime calls this on the thread
                    //
                    // Simplification: pass the real function's address directly.
                    // For zero-arg or all-constant args, we generate a wrapper.
                    let real_callee = self.get_or_declare_function(real_fn_name, &[], args.len());
                    let arg_vals: Vec<BasicMetadataValueEnum> = args
                        .iter()
                        .map(|&id| self.gen_expr_safe(&id, exprs).into())
                        .collect();

                    // Generate a no-arg wrapper that captures args via global storage.
                    // For the MVP: if all args are constants, bake them in;
                    // otherwise fall back to synchronous call (ponytail: closure capture).
                    let all_const = args.iter().all(|&id| {
                        matches!(exprs.get(&id), Some(MirExpr::IntLit(_)) | Some(MirExpr::FloatLit(_)))
                            || matches!(
                                exprs.get(&id).and_then(|e| match e {
                                    MirExpr::Var(v) => exprs.get(v),
                                    other => Some(other),
                                }),
                                Some(MirExpr::IntLit(_)) | Some(MirExpr::FloatLit(_))
                            )
                    });

                    if all_const {
                        // Create wrapper fn: i64 -> i64, ignores its arg, calls real fn
                        // Unique name per call site (multiple spawns of same fn must not collide)
                        let wrapper_name = format!("{}__entry{}", func, {
                            // Use a monotonically increasing id stored on self
                            self.spawn_counter += 1;
                            self.spawn_counter
                        });
                        let wrapper_type = self.i64_type.fn_type(&[self.i64_type.into()], false);
                        let wrapper = match self.module.get_function(&wrapper_name) {
                            Some(f) => f,
                            None => self.module.add_function(&wrapper_name, wrapper_type, None),
                        };

                        // Save current insert point
                        let saved_block = self.builder.get_insert_block().unwrap();
                        let saved_fn = self
                            .builder
                            .get_insert_block()
                            .and_then(|b| b.get_parent());

                        // Emit wrapper body
                        let entry = self
                            .context
                            .append_basic_block(wrapper, "entry");
                        self.builder.position_at_end(entry);
                        let call_args: Vec<BasicMetadataValueEnum> = arg_vals
                            .iter()
                            .map(|v| v.clone())
                            .collect();
                        let ret = self
                            .builder
                            .build_call(real_callee, &call_args, "thunk_call")
                            .unwrap();
                        let ret_val = Self::call_site_to_basic_value(ret)
                            .unwrap_or(self.i64_type.const_zero().into());
                        self.builder.build_return(Some(&ret_val)).unwrap();

                        // Restore insert point
                        self.builder.position_at_end(saved_block);
                        let _ = saved_fn;

                        // Call runtime: spawn(wrapper_fn_ptr) -> handle
                        let spawn_fn = self
                            .module
                            .get_function("spawn")
                            .unwrap_or_else(|| {
                                let ft = self
                                    .i64_type
                                    .fn_type(&[self.i64_type.into()], false);
                                self.module
                                    .add_function("spawn", ft, Some(Linkage::External))
                            });
                        let fn_ptr = wrapper
                            .as_global_value()
                            .as_pointer_value();
                        let fn_addr = self
                            .builder
                            .build_ptr_to_int(fn_ptr, self.i64_type, "thunk_addr")
                            .unwrap();
                        let spawn_ret = self
                            .builder
                            .build_call(spawn_fn, &[fn_addr.into()], "spawn_call")
                            .unwrap();
                        let handle = Self::call_site_to_basic_value(spawn_ret)
                            .unwrap_or(self.i64_type.const_zero().into());
                        let dest_alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(dest_alloca, handle).unwrap();
                        return true;
                    } else {
                        // Non-constant args: synchronous fallback
                        // ponytail: closure capture needed for real async args
                        let callee = self.get_or_declare_function(func, &[], args.len());
                        let call_vals: Vec<BasicMetadataValueEnum> = args
                            .iter()
                            .map(|&id| self.gen_expr_safe(&id, exprs).into())
                            .collect();
                        let call = self.builder.build_call(callee, &call_vals, "").unwrap();
                        if let Some(val) = Self::call_site_to_basic_value(call) {
                            let alloca = *self.locals.get(&dest).unwrap();
                            self.builder.build_store(alloca, val).unwrap();
                        }
                        return true;
                    }
        }
        false
    }

    pub(super) fn emit_array_get_inline(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        if (func == "array_get" || func == "stack_array_get") && args.len() == 2 {
                    let p0 = self.gen_expr_safe(&args[0], exprs);
                    let p1 = self.gen_expr_safe(&args[1], exprs);
                    let array_ptr_val = p0.into_int_value();
                    // Batch 769 (#79)：`/` 真除法（Python 语义，#188 家族）让整型
                    // 索引算术产出 F64（`mid = lo + (hi - lo) / 2`，correctness.z
                    // 的泛型 binary_search）——into_int_value 直接 panic（rc=101，
                    // selfhost 全量判据的新失败）。索引 F64 ⇒ fptosi 收口（与调用
                    // 实参 ABI coerce 同款），不再假设索引恒整。
                    let index_val: inkwell::values::IntValue<'ctx> = if p1.is_float_value() {
                        self.builder
                            .build_float_to_signed_int(
                                p1.into_float_value(),
                                self.context.i64_type(),
                                "idx_fptosi",
                            )
                            .unwrap()
                    } else {
                        p1.into_int_value()
                    };

                    let array_ptr = self
                        .builder
                        .build_int_to_ptr(
                            array_ptr_val,
                            self.context.ptr_type(AddressSpace::default()),
                            "array_ptr",
                        )
                        .unwrap();

                    // Determine element type from dest variable's type_map entry
                    let elem_llvm_type: inkwell::types::BasicTypeEnum<'ctx> = match
                        self.current_type_map.as_ref().and_then(|tm| tm.get(&dest))
                    {
                        Some(Type::F32) => self.context.f32_type().into(),
                        Some(Type::F64) => self.f64_type.into(),
                        _ => self.i64_type.into(),
                    };

                    let elem_ptr = unsafe {
                        self.builder
                            .build_gep(elem_llvm_type, array_ptr, &[index_val], "elem_ptr")
                            .unwrap()
                    };

                    let value: inkwell::values::BasicValueEnum<'ctx> = match elem_llvm_type {
                        inkwell::types::BasicTypeEnum::IntType(it) => {
                            self.builder.build_load(it, elem_ptr, "array_elem").unwrap().into()
                        }
                        inkwell::types::BasicTypeEnum::FloatType(ft) => {
                            self.builder.build_load(ft, elem_ptr, "array_elem").unwrap().into()
                        }
                        _ => self.i64_type.const_zero().into(),
                    };

                    let dest_alloca = *self.locals.get(&dest).unwrap();
                    self.builder.build_store(dest_alloca, value).unwrap();
                    return true;
        }
        false
    }

}