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
use inkwell::FloatPredicate;
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


    pub(super) fn emit_join(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "join" && args.len() == 1 {
                    let handle = self.gen_expr_safe(&args[0], exprs).into_int_value();
                    let join_fn = self
                        .module
                        .get_function("join")
                        .unwrap_or_else(|| {
                            let ft = self
                                .i64_type
                                .fn_type(&[self.i64_type.into()], false);
                            self.module
                                .add_function("join", ft, Some(Linkage::External))
                        });
                    let ret = self
                        .builder
                        .build_call(join_fn, &[handle.into()], "join_call")
                        .unwrap();
                    let val = Self::call_site_to_basic_value(ret)
                        .unwrap_or(self.i64_type.const_zero().into());
                    let alloca = *self.locals.get(&dest).unwrap();
                    self.builder.build_store(alloca, val).unwrap();
                    return true;
        }
        false
    }

    pub(super) fn emit_call_i64(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "call_i64" && args.len() >= 2 {
                    // call_i64(func_ptr: i64, arg: i64) -> i64
                    // For now, use identity workaround
                    let arg_val = self.gen_expr_safe(&args[1], exprs);
                    let dest_alloca = *self.locals.get(&dest).unwrap();
                    self.builder.build_store(dest_alloca, arg_val).unwrap();
                    return true;
        }
        false
    }

    pub(super) fn emit_norm_index(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "norm_index" && args.len() == 2 {
                    let len_v = self.gen_expr_safe(&args[0], exprs).into_int_value();
                    let idx_v = self.gen_expr_safe(&args[1], exprs).into_int_value();
                    let zero = self.i64_type.const_zero();
                    let is_neg = self
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::SLT,
                            idx_v,
                            zero,
                            "normidx_lt",
                        )
                        .unwrap();
                    let fixed = self.builder.build_int_add(len_v, idx_v, "normidx_add").unwrap();
                    let result: inkwell::values::BasicValueEnum<'ctx> = self
                        .builder
                        .build_select(is_neg, fixed, idx_v, "normidx")
                        .unwrap()
                        .into();
                    let dest_alloca = *self.locals.get(&dest).unwrap();
                    self.builder.build_store(dest_alloca, result).unwrap();
                    return true;
        }
        false
    }

    pub(super) fn emit_ptr_read(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "read" && args.len() == 1 {
                        let ptr = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let pt = self.context.ptr_type(inkwell::AddressSpace::default());
                        let elem_ptr = self.builder.build_int_to_ptr(ptr, pt, "rd_ptr").unwrap();
                        let elem_i64 = self
                            .builder
                            .build_pointer_cast(elem_ptr, pt, "rd_i64")
                            .unwrap();
                        let val = self
                            .builder
                            .build_load(self.i64_type, elem_i64, "rd_val")
                            .unwrap();
                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(alloca, val).unwrap();
                        return true;
        }
        false
    }

    pub(super) fn emit_ptr_write(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "write" && args.len() == 2 {
                        let ptr = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let val = self.gen_expr_safe(&args[1], exprs).into_int_value();
                        let pt = self.context.ptr_type(inkwell::AddressSpace::default());
                        let elem_ptr = self.builder.build_int_to_ptr(ptr, pt, "wr_ptr").unwrap();
                        let elem_i64 = self
                            .builder
                            .build_pointer_cast(elem_ptr, pt, "wr_i64")
                            .unwrap();
                        self.builder.build_store(elem_i64, val).unwrap();
                        if let Some(&alloca) = self.locals.get(&dest) {
                            self.builder
                                .build_store(alloca, self.i64_type.const_int(0, false))
                                .unwrap();
                        }
                        return true;
        }
        false
    }

    pub(super) fn emit_is_null(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "is_null" && args.len() == 1 {
                        let ptr = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let is_null = self
                            .builder
                            .build_int_compare(
                                inkwell::IntPredicate::EQ,
                                ptr,
                                self.i64_type.const_int(0, false),
                                "is_null",
                            )
                            .unwrap();
                        let result = self
                            .builder
                            .build_int_z_extend(is_null, self.i64_type, "is_null_ext")
                            .unwrap();
                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(alloca, result).unwrap();
                        return true;
        }
        false
    }

    pub(super) fn emit_ptr_offset(&mut self, func: &str, args: &[u32], dest: u32, type_args: &[crate::middle::types::Type], exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, type_args, exprs);
        if func == "offset" && args.len() == 2 {
                        let ptr = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let count = self.gen_expr_safe(&args[1], exprs).into_int_value();
                        let elem_size: u64 = if let Some(ty) = type_args.first() {
                            match ty {
                                crate::middle::types::Type::I8 | crate::middle::types::Type::U8 => {
                                    1
                                }
                                crate::middle::types::Type::I16
                                | crate::middle::types::Type::U16 => 2,
                                crate::middle::types::Type::I32
                                | crate::middle::types::Type::U32
                                | crate::middle::types::Type::F32 => 4,
                                _ => 8,
                            }
                        } else {
                            8
                        };
                        let byte_offset = self
                            .builder
                            .build_int_mul(
                                count,
                                self.i64_type.const_int(elem_size, false),
                                "byte_off",
                            )
                            .unwrap();
                        let ptr = self
                            .builder
                            .build_int_add(ptr, byte_offset, "off_ptr")
                            .unwrap();
                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(alloca, ptr).unwrap();
                        return true;
        }
        false
    }

    pub(super) fn emit_replace(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "replace" && args.len() == 2 {
                        let ptr = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let new_val = self.gen_expr_safe(&args[1], exprs);
                        let ptr_type = self.context.ptr_type(inkwell::AddressSpace::default());
                        let elem_ptr = self
                            .builder
                            .build_int_to_ptr(ptr, ptr_type, "rpl_ptr")
                            .unwrap();
                        let elem_i64 = self
                            .builder
                            .build_pointer_cast(elem_ptr, ptr_type, "rpl_i64")
                            .unwrap();
                        let old_val = self
                            .builder
                            .build_load(self.i64_type, elem_i64, "old")
                            .unwrap();
                        self.builder
                            .build_store(elem_i64, new_val.into_int_value())
                            .unwrap();
                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(alloca, old_val).unwrap();
                        return true;
        }
        false
    }

    pub(super) fn emit_syscall(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "syscall" || func.starts_with("syscall_") {
                        let num_val = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let mut all_args: Vec<BasicMetadataValueEnum> = vec![num_val.into()];
                        for i in 1..args.len() {
                            let val = self.gen_expr_safe(&args[i], exprs).into_int_value();
                            all_args.push(val.into());
                        }
                        while all_args.len() < 7 {
                            all_args.push(self.i64_type.const_zero().into());
                        }
                        let fn_type = self.i64_type.fn_type(
                            &[self.i64_type.into(); 7],
                            false,
                        );
                        let callee = self.module.add_function(
                            "zenith_syscall", fn_type, None,
                        );
                        let call = self.builder
                            .build_call(callee, &all_args, "syscall")
                            .unwrap();
                        let basic_val = Self::call_site_to_basic_value(call)
                            .unwrap_or(self.i64_type.const_zero().into());
                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(alloca, basic_val).unwrap();
                        return true;
        }
        false
    }

    pub(super) fn emit_capy_store(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "capy_store_i64" || func == "capy_store_i64_2" {
                        if args.len() >= 2 {
                            let addr = self.gen_expr_safe(&args[0], exprs).into_int_value();
                            let val = self.gen_expr_safe(&args[1], exprs).into_int_value();
                            let ptr = self
                                .builder
                                .build_int_to_ptr(addr, self.ptr_type, "store_ptr")
                                .unwrap();
                            self.builder.build_store(ptr, val).unwrap();
                        }
                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder
                            .build_store(alloca, self.i64_type.const_zero())
                            .unwrap();
                        return true;
        }
        false
    }

    pub(super) fn emit_capy_load(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        let _ = (func, args, dest, exprs);
        if func == "capy_load_i64" || func == "capy_load_i64_1" {
                        let addr = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let ptr = self
                            .builder
                            .build_int_to_ptr(addr, self.ptr_type, "load_ptr")
                            .unwrap();
                        let val = self.builder.build_load(self.i64_type, ptr, "loaded").unwrap();
                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(alloca, val).unwrap();
                        return true;
        }
        false
    }

    pub(super) fn emit_unary_minus_pre(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        if !(args.len() == 1 && (func == "-" || func == "unary_minus")) {
            return false;
        }
                    let operand = self.gen_expr_safe(&args[0], exprs);
                    let zero = self.i64_type.const_zero();
                    let result: inkwell::values::BasicValueEnum<'ctx> = if operand.is_float_value() {
                        let f = operand.into_float_value();
                        self.builder.build_float_neg(f, "neg").unwrap().into()
                    } else {
                        self.builder
                            .build_int_sub(zero, operand.into_int_value(), "neg")
                            .unwrap()
                            .into()
                    };
                    let alloca = *self.locals.get(&dest).unwrap();
                    self.builder.build_store(alloca, result).unwrap();
                    return true;
        true
    }

    pub(super) fn emit_operator_family(&mut self, func: &str, args: &[u32], dest: u32, type_args: &[crate::middle::types::Type], exprs: &HashMap<u32, MirExpr>) -> bool {
        if !(self.is_operator(func)) {
            return false;
        }
                    // Handle unary operators
                    if args.len() == 1 && func == "!" {
                        // Logical NOT — must yield 0/1, not a bitwise complement.
                        // (`x ^ -1` gave -1 for `not 0`, so `(not x) == 1` was
                        // false and `print(not x)` printed -1.)
                        let operand = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let is_zero = self
                            .builder
                            .build_int_compare(
                                inkwell::IntPredicate::EQ,
                                operand,
                                self.i64_type.const_zero(),
                                "lognot",
                            )
                            .unwrap();
                        let result = self
                            .builder
                            .build_int_z_extend(is_zero, self.i64_type, "lognot_ext")
                            .unwrap();

                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(alloca, result).unwrap();
                        return true;
                    }

                    // Handle unary minus
                    if args.len() == 1 && func == "-" {
                        let operand = self.gen_expr_safe(&args[0], exprs);
                        // Unary minus: 0 - operand
                        let zero = self.i64_type.const_zero();
                        let result = self
                            .builder
                            .build_int_sub(zero, operand.into_int_value(), "neg")
                            .unwrap();

                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(alloca, result).unwrap();
                        return true;
                    }

                    // Handle binary operators
                    if args.len() == 2 {
                        let left = self.gen_expr_safe(&args[0], exprs);
                        let right = self.gen_expr_safe(&args[1], exprs);
                        // BATCH-455 (#203): a column operand takes the
                        // element-wise route instead of scalar pointer math.
                        if let Some(kind) = self.column_arith_dispatch(func, args[0], args[1]) {
                            if let Some(v) = self.gen_column_arith(
                                kind,
                                left,
                                right,
                                kind & 4 != 0,
                                kind & 8 != 0,
                            ) {
                                // A dest with no slot has nowhere to land — fall
                                // through to the scalar route, don't panic (#176 同族).
                                if let Some(alloca) = self.locals.get(&dest).copied() {
                                    self.builder.build_store(alloca, v).unwrap();
                                    return true;
                                }
                            }
                        }
                        let is_float = matches!(left.get_type(), inkwell::types::BasicTypeEnum::FloatType(_))
                            || matches!(right.get_type(), inkwell::types::BasicTypeEnum::FloatType(_));

                        let result = if is_float {
                            let l = if matches!(left.get_type(), inkwell::types::BasicTypeEnum::FloatType(_)) {
                                left.into_float_value()
                            } else {
                                self.builder.build_signed_int_to_float(left.into_int_value(), self.f64_type, "op_l_sitofp").unwrap()
                            };
                            let r = if matches!(right.get_type(), inkwell::types::BasicTypeEnum::FloatType(_)) {
                                right.into_float_value()
                            } else {
                                self.builder.build_signed_int_to_float(right.into_int_value(), self.f64_type, "op_r_sitofp").unwrap()
                            };
                            match func {
                                "+" | "add" => self.builder.build_float_add(l, r, "add").unwrap().into(),
                                "-" | "sub" => self.builder.build_float_sub(l, r, "sub").unwrap().into(),
                                "*" | "mul" => self.builder.build_float_mul(l, r, "mul").unwrap().into(),
                                "/" | "div" => self.builder.build_float_div(l, r, "div").unwrap().into(),
                                "floordiv" => self.build_floordiv_float(l, r),
                                "%" | "mod" => self.builder.build_float_rem(l, r, "mod").unwrap().into(),
                                "==" | "eq" => {
                                    let cmp = self
                                        .builder
                                        .build_float_compare(FloatPredicate::OEQ, l, r, "cmp_eq")
                                        .unwrap();
                                    self.builder
                                        .build_int_z_extend(cmp, self.i64_type, "cmp_eq_ext")
                                        .unwrap()
                                        .into()
                                }
                                "!=" | "ne" => {
                                    let cmp = self
                                        .builder
                                        .build_float_compare(FloatPredicate::ONE, l, r, "cmp_ne")
                                        .unwrap();
                                    self.builder
                                        .build_int_z_extend(cmp, self.i64_type, "cmp_ne_ext")
                                        .unwrap()
                                        .into()
                                }
                                "<" | "lt" => {
                                    let cmp = self
                                        .builder
                                        .build_float_compare(FloatPredicate::OLT, l, r, "cmp_lt")
                                        .unwrap();
                                    self.builder
                                        .build_int_z_extend(cmp, self.i64_type, "cmp_lt_ext")
                                        .unwrap()
                                        .into()
                                }
                                ">" | "gt" => {
                                    let cmp = self
                                        .builder
                                        .build_float_compare(FloatPredicate::OGT, l, r, "cmp_gt")
                                        .unwrap();
                                    self.builder
                                        .build_int_z_extend(cmp, self.i64_type, "cmp_gt_ext")
                                        .unwrap()
                                        .into()
                                }
                                "<=" | "le" => {
                                    let cmp = self
                                        .builder
                                        .build_float_compare(FloatPredicate::OLE, l, r, "cmp_le")
                                        .unwrap();
                                    self.builder
                                        .build_int_z_extend(cmp, self.i64_type, "cmp_le_ext")
                                        .unwrap()
                                        .into()
                                }
                                ">=" | "ge" => {
                                    let cmp = self
                                        .builder
                                        .build_float_compare(FloatPredicate::OGE, l, r, "cmp_ge")
                                        .unwrap();
                                    self.builder
                                        .build_int_z_extend(cmp, self.i64_type, "cmp_ge_ext")
                                        .unwrap()
                                        .into()
                                }
                                _ => self.i64_type.const_zero().into(),
                            }
                        } else {
                            let l = left.into_int_value();
                            let r = right.into_int_value();
                            match func {
                                "+" | "add" | "add_i64" => self.builder.build_int_add(l, r, "add").unwrap().into(),
                                "-" | "sub" | "sub_i64" => self.builder.build_int_sub(l, r, "sub").unwrap().into(),
                                "*" | "mul" | "mul_i64" => self.builder.build_int_mul(l, r, "mul").unwrap().into(),
                                // PY-A: `/` on two integers is TRUE division — promote
                                // both sides and divide as f64. `div_i64` stays an
                                // integer division (it is the explicit int form).
                                // The guard is the slot check (see `slot_is_float`):
                                // an int-shaped destination means the quotient must
                                // stay an integer, or the double bits are re-read as
                                // a handle.
                                "/" | "div" if self.slot_is_float(dest) => {
                                    let lf = self.builder.build_signed_int_to_float(l, self.f64_type, "td_l_sitofp").unwrap();
                                    let rf = self.builder.build_signed_int_to_float(r, self.f64_type, "td_r_sitofp").unwrap();
                                    self.builder.build_float_div(lf, rf, "div").unwrap().into()
                                }
                                "/" | "div" | "div_i64" => self.builder.build_int_signed_div(l, r, "div").unwrap().into(),
                                "floordiv" => self.build_floordiv_int(l, r),
                                "%" | "mod" | "mod_i64" => self.build_floormod_int(l, r),
                                "<<" | "shl" | "shl_i64" => self.builder.build_left_shift(l, r, "shl").unwrap().into(),
                                // Batch 595: is_signed=true (arith shift) — the
                                // scalar interceptor only ever sees i64 (there is
                                // no unsigned scalar in the python surface), and
                                // the logical variant shredded the sign extension
                                // of negative operands (`-149… >> 2` returned a
                                // ~4.6e18 positive; gen_stmts_s585001_* chains).
                                ">>" | "shr" | "shr_i64" => self.builder.build_right_shift(l, r, true, "shr").unwrap().into(),
                                "&" | "bitand" | "and_i64" => self.builder.build_and(l, r, "bitand").unwrap().into(),
                                "|" | "bitor" | "or_i64" => self.builder.build_or(l, r, "bitor").unwrap().into(),
                                "^" | "bitxor" | "xor_i64" => self.builder.build_xor(l, r, "bitxor").unwrap().into(),
                                "==" | "eq" | "eq_i64" => {
                                    let cmp = self.builder.build_int_compare(inkwell::IntPredicate::EQ, l, r, "eq").unwrap();
                                    self.builder.build_int_z_extend(cmp, self.i64_type, "eq_ext").unwrap().into()
                                }
                                "!=" | "ne" | "ne_i64" => {
                                    let cmp = self.builder.build_int_compare(inkwell::IntPredicate::NE, l, r, "ne").unwrap();
                                    self.builder.build_int_z_extend(cmp, self.i64_type, "ne_ext").unwrap().into()
                                }
                                "<" | "lt" | "lt_i64" => {
                                    let cmp = self.builder.build_int_compare(inkwell::IntPredicate::SLT, l, r, "lt").unwrap();
                                    self.builder.build_int_z_extend(cmp, self.i64_type, "lt_ext").unwrap().into()
                                }
                                ">" | "gt" | "gt_i64" => {
                                    let cmp = self.builder.build_int_compare(inkwell::IntPredicate::SGT, l, r, "gt").unwrap();
                                    self.builder.build_int_z_extend(cmp, self.i64_type, "gt_ext").unwrap().into()
                                }
                                "<=" | "le" | "le_i64" => {
                                    let cmp = self.builder.build_int_compare(inkwell::IntPredicate::SLE, l, r, "le").unwrap();
                                    self.builder.build_int_z_extend(cmp, self.i64_type, "le_ext").unwrap().into()
                                }
                                ">=" | "ge" | "ge_i64" => {
                                    let cmp = self.builder.build_int_compare(inkwell::IntPredicate::SGE, l, r, "ge").unwrap();
                                    self.builder.build_int_z_extend(cmp, self.i64_type, "ge_ext").unwrap().into()
                                }
                                "&&" | "and" => {
                                    let left_bool = self.builder.build_int_compare(inkwell::IntPredicate::NE, l, self.i64_type.const_int(0, false), "left_bool").unwrap();
                                    let right_bool = self.builder.build_int_compare(inkwell::IntPredicate::NE, r, self.i64_type.const_int(0, false), "right_bool").unwrap();
                                    let bool_and = self.builder.build_and(left_bool, right_bool, "and").unwrap();
                                    self.builder.build_int_z_extend(bool_and, self.i64_type, "and_ext").unwrap().into()
                                }
                                "||" | "or" => {
                                    let left_bool = self.builder.build_int_compare(inkwell::IntPredicate::NE, l, self.i64_type.const_int(0, false), "left_bool").unwrap();
                                    let right_bool = self.builder.build_int_compare(inkwell::IntPredicate::NE, r, self.i64_type.const_int(0, false), "right_bool").unwrap();
                                    let bool_or = self.builder.build_or(left_bool, right_bool, "or").unwrap();
                                    self.builder.build_int_z_extend(bool_or, self.i64_type, "or_ext").unwrap().into()
                                }
                                _ => {
                                    let callee = self.get_or_declare_function(func, type_args, args.len());
                                    let arg_vals: Vec<BasicMetadataValueEnum> = args.iter().map(|&id| self.gen_expr_safe(&id, exprs).into()).collect();
                                    let coerced = self.coerce_call_args(callee, arg_vals, args);
                                    let call = self.builder.build_call(callee, &coerced, "").unwrap();
                                    Self::call_site_to_basic_value(call).unwrap_or(self.i64_type.const_zero().into())
                                }
                            }
                        };

                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder.build_store(alloca, result).unwrap();
                    } else {
                        // Operator with wrong number of arguments, fall through to regular function call
                        let callee = self.get_or_declare_function(func, type_args, args.len());
                        let arg_vals: Vec<BasicMetadataValueEnum> = args
                            .iter()
                            .map(|&id| self.gen_expr_safe(&id, exprs).into())
                            .collect();
                        let coerced = self.coerce_call_args(callee, arg_vals, args);
                        let call = self.builder.build_call(callee, &coerced, "").unwrap();
                        if let Some(val) = Self::call_site_to_basic_value(call) {
                            let alloca = *self.locals.get(&dest).unwrap();
                            self.builder.build_store(alloca, val).unwrap();
                        }
                    }
        true
    }

    pub(super) fn emit_v4i64_andnot(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        if !(func == "__builtin_v4i64_andnot" && args.len() == 6) {
            return false;
        }
                        let ptr_i64 = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let word_idx = self.gen_expr_safe(&args[1], exprs).into_int_value();
                        let m0 = self.gen_expr_safe(&args[2], exprs).into_int_value();
                        let m1 = self.gen_expr_safe(&args[3], exprs).into_int_value();
                        let m2 = self.gen_expr_safe(&args[4], exprs).into_int_value();
                        let m3 = self.gen_expr_safe(&args[5], exprs).into_int_value();

                        let thirty_two = self.i64_type.const_int(32, false);
                        let byte_offset = self
                            .builder
                            .build_int_mul(word_idx, thirty_two, "off")
                            .unwrap();
                        let base_ptr = self
                            .builder
                            .build_int_to_ptr(ptr_i64, self.ptr_type, "base")
                            .unwrap();
                        let vec_ptr = unsafe {
                            self.builder
                                .build_gep(self.context.i8_type(), base_ptr, &[byte_offset], "vptr")
                                .unwrap()
                        };
                        let loaded = self
                            .builder
                            .build_load(self.vec4_i64_type, vec_ptr, "load")
                            .unwrap()
                            .into_vector_value();

                        let poison = self.vec4_i64_type.get_undef();
                        let z = self.i64_type.const_int(0, false);
                        let o = self.i64_type.const_int(1, false);
                        let t = self.i64_type.const_int(2, false);
                        let h = self.i64_type.const_int(3, false);
                        let mut mask = self
                            .builder
                            .build_insert_element(poison, m0, z, "m0")
                            .unwrap();
                        mask = self
                            .builder
                            .build_insert_element(mask, m1, o, "m1")
                            .unwrap();
                        mask = self
                            .builder
                            .build_insert_element(mask, m2, t, "m2")
                            .unwrap();
                        mask = self
                            .builder
                            .build_insert_element(mask, m3, h, "m3")
                            .unwrap();

                        let one_val = self.i64_type.const_int(u64::MAX, false);
                        let mut all_ones = self
                            .builder
                            .build_insert_element(poison, one_val, z, "o0")
                            .unwrap();
                        all_ones = self
                            .builder
                            .build_insert_element(all_ones, one_val, o, "o1")
                            .unwrap();
                        all_ones = self
                            .builder
                            .build_insert_element(all_ones, one_val, t, "o2")
                            .unwrap();
                        all_ones = self
                            .builder
                            .build_insert_element(all_ones, one_val, h, "o3")
                            .unwrap();

                        let not_mask = self.builder.build_xor(mask, all_ones, "not").unwrap();
                        let result = self.builder.build_and(loaded, not_mask, "res").unwrap();
                        self.builder.build_store(vec_ptr, result).unwrap();

                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder
                            .build_store(alloca, self.i64_type.const_zero())
                            .unwrap();
                        return true;
        true
    }

    pub(super) fn emit_v4i64_store(&mut self, func: &str, args: &[u32], dest: u32, exprs: &HashMap<u32, MirExpr>) -> bool {
        if !(func == "__builtin_v4i64_store" && args.len() == 6) {
            return false;
        }
                        let ptr_i64 = self.gen_expr_safe(&args[0], exprs).into_int_value();
                        let word_idx = self.gen_expr_safe(&args[1], exprs).into_int_value();
                        let v0 = self.gen_expr_safe(&args[2], exprs).into_int_value();
                        let v1 = self.gen_expr_safe(&args[3], exprs).into_int_value();
                        let v2 = self.gen_expr_safe(&args[4], exprs).into_int_value();
                        let v3 = self.gen_expr_safe(&args[5], exprs).into_int_value();

                        let thirty_two = self.i64_type.const_int(32, false);
                        let byte_offset = self
                            .builder
                            .build_int_mul(word_idx, thirty_two, "off")
                            .unwrap();
                        let base_ptr = self
                            .builder
                            .build_int_to_ptr(ptr_i64, self.ptr_type, "base")
                            .unwrap();
                        let vec_ptr = unsafe {
                            self.builder
                                .build_gep(self.context.i8_type(), base_ptr, &[byte_offset], "vptr")
                                .unwrap()
                        };

                        let poison = self.vec4_i64_type.get_undef();
                        let z = self.i64_type.const_int(0, false);
                        let o = self.i64_type.const_int(1, false);
                        let t = self.i64_type.const_int(2, false);
                        let h = self.i64_type.const_int(3, false);
                        let mut vec = self
                            .builder
                            .build_insert_element(poison, v0, z, "v0")
                            .unwrap();
                        vec = self.builder.build_insert_element(vec, v1, o, "v1").unwrap();
                        vec = self.builder.build_insert_element(vec, v2, t, "v2").unwrap();
                        vec = self.builder.build_insert_element(vec, v3, h, "v3").unwrap();

                        self.builder.build_store(vec_ptr, vec).unwrap();

                        let alloca = *self.locals.get(&dest).unwrap();
                        self.builder
                            .build_store(alloca, self.i64_type.const_zero())
                            .unwrap();
                        return true;
        true
    }
}
