//! 批次 872：表达式字面量族发射体（Tuple／ArrayLit／StructLit／
//! DynamicArrayLit——857 稳定化时内联回 gen.rs，本批按 869 零适配法迁出；
//! 四臂尾部均写 id 槽，函数尾统一补 `id` 对齐派发约定）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::{ArraySize, Type};

impl MirGen {
    pub(super) fn lower_struct_lit(
        &mut self,
        variant: &String,
        fields: &Vec<(String, AstNode)>,
        id: u32,
    ) -> u32 {
            // Implement proper struct literal creation
            let mut field_ids = Vec::new();
            // Fields are lowered in source order and a Python `__init__` may
            // read a field it assigned a line earlier (`self.a = x` then
            // `self.b = join(self.a, ...)`). The synthesized ctor has NO
            // `self`, so record each computed value under its field name for
            // the later initializers of this same literal. Nesting is safe:
            // the list is truncated back on the way out.
            let alias_mark = self.self_field_aliases.len();
            for (field_name, field_expr) in fields {
                let field_id = self.lower_expr(field_expr);
                // Register the alias as a REAL SLOT, not as the expression id.
                // A literal/expression id has no alloca, so a later
                // `self.x` read passed it to a runtime call and codegen loaded
                // the (missing) slot: `self.cd = "/tmp/root/data";
                // self.scd = os.path.join(self.cd, "stocks")` produced just
                // "stocks" — and `MarketDataFetcher.cache_dir` came out as
                // 2 bytes of garbage, which made every cache path miss.
                let slot = self.next_id();
                self.stmts.push(MirStmt::Assign { lhs: slot, rhs: field_id });
                let ty = self.type_map.get(&field_id).cloned().unwrap_or_else(Type::slot_fallback);
                self.exprs.insert(slot, MirExpr::Var(slot));
                self.type_map.insert(slot, ty);
                self.self_field_aliases.push((field_name.clone(), slot));
                field_ids.push((field_name.clone(), field_id));
            }
            self.self_field_aliases.truncate(alias_mark);
            // Create Struct expression.
            //
            // `with_enum_tag` is load-bearing for the named-field form of a
            // variant ctor (`Shape::Rect { w: 3, h: 4 }`): this used to copy
            // the literal's own field list straight through, so the block
            // had no slot 0 tag and the arm test — which reads slot 0 —
            // never matched one. `match v { Shape::Rect { w, h } => 1, … }`
            // fell to the wildcard and printed 99 (measured).
            let struct_fields = self.with_enum_tag(variant, field_ids);
            self.exprs.insert(
                id,
                MirExpr::Struct {
                    variant: variant.clone(),
                    fields: struct_fields,
                },
            );
            // Batch 805: the literal's type is its VARIANT name (`S {..}` →
            // Named("S")), not the placeholder spelling "Struct" — with the
            // placeholder every method call on a Rust-shape literal missed
            // func_ret_types ("S::greet") and degraded to I64, printing the
            // heap handle for a Str-returning method (measured twice: 404
            // `[impl=4374191776]`, and `-> str` methods too).
            self.type_map
                .insert(id, Type::Named(variant.clone(), vec![]));
    id
    }

    pub(super) fn lower_array_lit(&mut self, elements: &Vec<AstNode>, id: u32) -> u32 {
            // Create an array using ArrayHeader API

            let size = elements.len();

            // Python's `[]` is a GROWABLE list, never a 0-length fixed array.
            // As a `StackArray` of size 0 it had no `[cap|len]` header, so
            // `xs.append(v)`'s `vec_push` read the header 16 bytes BEFORE the
            // alloca (garbage) and wrote past the buffer — stack corruption.
            // That is the UB behind the pandas cluster's Bus errors / SEGVs
            // (`idx: lt(vec, str) = []` + `idx.append(str(i))` in
            // pylib/pandas.z) and behind t207's `len(xs)` staying 0.
            // Lower the elements ONCE — the branch below decides the
            // representation, and lowering twice would duplicate side effects.
            let mut lowered_elems = Vec::new();
            for element in elements {
                lowered_elems.push(self.lower_expr(element));
            }
            let elem_ty_pre = self.get_common_element_type(&lowered_elems);
            // A Python list literal must be a GROWABLE list with the
            // `[cap|len]` header: every vec_* runtime call (concat /
            // fromkeys / len / index / push) reads that header. As a
            // StackArray those calls read 16 bytes BEFORE the buffer and
            // walked off it — measured as a SEGV in `py_map_fromkeys`, from
            // `wufu_constants`' `dict.fromkeys(GLOBAL_ETF_POOL + …)`.
            // FLOAT lists keep the StackArray form: `vec_push` is an i64
            // channel, so an f64 element would be reinterpreted as its bit
            // pattern (t56/t139 went red when EVERY literal became dynamic).
            // Batch 782 (#203⑥ 尾巴)：float 字面量列表改 DynamicArray——
            // 元素经 zeta_vec_push_f64（f64 直进 xmm，C 侧按位 push），列表
            // 型 DynamicArray(F64) ⇒ len/下标/vec_* 全走 dyn 面。t56/t139
            // 重锚验证见本批读数。
            if size == 0 {
                let capacity_id = self.next_id();
                self.exprs.insert(capacity_id, MirExpr::IntLit(0));
                self.type_map.insert(capacity_id, Type::I64);
                let h = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: "zeta_dynarray_new".to_string(),
                    args: vec![capacity_id],
                    dest: h,
                    type_args: vec![],
                });
                self.exprs.insert(h, MirExpr::Var(h));
                self.type_map
                    .insert(h, Type::DynamicArray(Box::new(Type::I64)));
                self.exprs.insert(id, MirExpr::Var(h));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                return id;
            }
            if !matches!(elem_ty_pre, Type::F32 | Type::F64) {
                let start = if size == 0 { Vec::new() } else { lowered_elems };
                let elem_ty = if size == 0 { Type::I64 } else { elem_ty_pre };
                let capacity_id = self.next_id();
                self.exprs.insert(capacity_id, MirExpr::IntLit(size as i64));
                self.type_map.insert(capacity_id, Type::I64);
                let h = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: "zeta_dynarray_new".to_string(),
                    args: vec![capacity_id],
                    dest: h,
                    type_args: vec![],
                });
                self.exprs.insert(h, MirExpr::Var(h));
                self.type_map
                    .insert(h, Type::DynamicArray(Box::new(elem_ty.clone())));
                for e in start {
                    let sink = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "vec_push".to_string(),
                        args: vec![h, e],
                        dest: sink,
                        type_args: vec![],
                    });
                    self.exprs.insert(sink, MirExpr::Var(sink));
                    self.type_map
                        .insert(sink, Type::DynamicArray(Box::new(elem_ty.clone())));
                    self.stmts.push(MirStmt::Assign { lhs: h, rhs: sink });
                }
                self.exprs.insert(id, MirExpr::Var(h));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(elem_ty)));
                return id;
            }

            // Batch 782 (#203⑥ 尾巴)：float 字面量列表 → DynamicArray(F64)
            // 元素经 zeta_vec_push_f64（f64 直进 xmm，C 侧按位 push）；列表
            // 型 DynamicArray(F64) ⇒ 下标读 F64 渲染、len/vec_* 可用。
            if matches!(elem_ty_pre, Type::F32 | Type::F64) {
                let capacity_id = self.next_id();
                self.exprs.insert(capacity_id, MirExpr::IntLit(size as i64));
                self.type_map.insert(capacity_id, Type::I64);
                let h = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: "zeta_dynarray_new".to_string(),
                    args: vec![capacity_id],
                    dest: h,
                    type_args: vec![],
                });
                self.exprs.insert(h, MirExpr::Var(h));
                self.type_map.insert(
                    h,
                    Type::DynamicArray(Box::new(elem_ty_pre.clone())),
                );
                for e in &lowered_elems {
                    let e_ty = self.type_map.get(e).cloned().unwrap_or_else(Type::slot_fallback);
                    let e_f = if matches!(e_ty, Type::F64) {
                        *e
                    } else {
                        let f = self.emit_call("zeta_float_i64", vec![*e], Type::F64);
                        f
                    };
                    let sink = self.next_id();
                    self.stmts.push(MirStmt::Call {
                        func: "zeta_vec_push_f64".to_string(),
                        args: vec![h, e_f],
                        dest: sink,
                        type_args: vec![],
                    });
                    self.exprs.insert(sink, MirExpr::Var(sink));
                    self.type_map.insert(
                        sink,
                        Type::DynamicArray(Box::new(Type::F64)),
                    );
                    self.stmts.push(MirStmt::Assign { lhs: h, rhs: sink });
                }
                self.exprs.insert(id, MirExpr::Var(h));
                self.type_map.insert(
                    id,
                    Type::DynamicArray(Box::new(elem_ty_pre.clone())),
                );
                return id;
            }

            // HYBRID MEMORY SYSTEM: Check if this should be a stack array
            // For small, fixed-size arrays, use stack allocation
            if size <= 20000 {
                // Reasonable stack size limit

                let element_ids = lowered_elems;
                let element_ids_clone = element_ids.clone();

                // Create StackArray expression
                self.exprs.insert(
                    id,
                    MirExpr::StackArray {
                        elements: element_ids,
                        size,
                    },
                );

                // Determine element type from elements
                let elem_type = self.get_common_element_type(&element_ids_clone);

                // Set the type to Array(elem_type, size) for subscript access
                self.type_map.insert(
                    id,
                    Type::Array(Box::new(elem_type), ArraySize::Literal(size)),
                );
            } else {
                // Large array, use heap allocation

                // Call array_new with capacity = size
                let array_data_ptr = self.next_id();
                let capacity_id = self.next_id();
                self.exprs.insert(capacity_id, MirExpr::IntLit(size as i64));
                self.stmts.push(MirStmt::Call {
                    func: "array_new".to_string(),
                    args: vec![capacity_id],
                    dest: array_data_ptr,
                    type_args: vec![],
                });

                // For heap arrays, we need to set the length
                let len_id = self.next_id();
                self.exprs.insert(len_id, MirExpr::IntLit(size as i64));
                self.stmts.push(MirStmt::VoidCall {
                    func: "array_set_len".to_string(),
                    args: vec![array_data_ptr, len_id],
                });

                // Set each element at its index and collect element IDs
                let mut heap_element_ids = Vec::new();
                for (i, element) in elements.iter().enumerate() {
                    let elem_id = self.lower_expr(element);
                    heap_element_ids.push(elem_id);
                    let index_id = self.next_id();
                    self.exprs.insert(index_id, MirExpr::IntLit(i as i64));
                    self.stmts.push(MirStmt::VoidCall {
                        func: "array_set".to_string(),
                        args: vec![array_data_ptr, index_id, elem_id],
                    });
                }

                // Clone heap_element_ids before using it
                let heap_element_ids_clone = heap_element_ids.clone();

                // Return the data pointer (after header)
                self.exprs.insert(id, MirExpr::Var(array_data_ptr));
                // Determine element type from elements
                let elem_type = self.get_common_element_type(&heap_element_ids_clone);
                // Set the type to Array(elem_type, size) for subscript access
                self.type_map.insert(
                    id,
                    Type::Array(Box::new(elem_type), ArraySize::Literal(size)),
                );
            }
    id
    }

    pub(super) fn lower_dynamic_array_lit(
        &mut self,
        elem_type: &String,
        elements: &Vec<AstNode>,
        id: u32,
    ) -> u32 {
            // `[dynamic]T{}` must use the [cap|len|data] layout, because
            // every vec_* runtime call (push/len/get) reads that header.
            // `array_new` allocates a HEADERLESS buffer (its 1-arg form
            // takes a byte count), so pushes used to write beside it and
            // len() read 0 — `arr.push(x)` in a loop left the array empty.
            let array_ptr = self.next_id();
            let capacity_id = self.next_id();
            self.exprs
                .insert(capacity_id, MirExpr::IntLit(elements.len() as i64));
            self.stmts.push(MirStmt::Call {
                func: "zeta_dynarray_new".to_string(),
                args: vec![capacity_id],
                dest: array_ptr,
                type_args: vec![],
            });
            self.exprs.insert(array_ptr, MirExpr::Var(array_ptr));
            let array_type = Type::DynamicArray(Box::new(Type::from_string(elem_type)));
            self.type_map.insert(array_ptr, array_type.clone());

            // Push each element to the array
            for element in elements {
                let elem_id = self.lower_expr(element);
                let void_dest = self.next_id(); // push returns void
                self.stmts.push(MirStmt::Call {
                    func: "array_push".to_string(),
                    args: vec![array_ptr, elem_id],
                    dest: void_dest,
                    type_args: vec![],
                });
            }

            // Return the array pointer (use array_ptr as the result)
            self.exprs.insert(id, MirExpr::Var(array_ptr));
            self.type_map.insert(id, array_type);
            return array_ptr; // Return the array pointer ID, not a new ID
    id
    }

    pub(super) fn lower_tuple_expr(&mut self, elements: &Vec<AstNode>, id: u32) -> u32 {
            // Tuple expression: lower each element, create stack array.
            let mut element_ids = Vec::new();
            for elem in elements {
                let elem_id = self.lower_expr(elem);
                // A COMPUTED element (call / subscript / field / binary op)
                // must be materialized BEFORE the tuple handle is built:
                // `return d.iloc[0:0], 7` put an uninitialized slot into the
                // pair, so the caller's frame was garbage and `len(o)` SEGV'd
                // (measured: `remove_extreme_return_bars`'s empty-parts path).
                let elem_id = if matches!(
                    elem,
                    AstNode::Call { .. }
                        | AstNode::Subscript { .. }
                        | AstNode::FieldAccess { .. }
                        | AstNode::BinaryOp { .. }
                ) {
                    self.materialize_for_call(elem_id)
                } else {
                    elem_id
                };
                element_ids.push(elem_id);
            }
            let size = element_ids.len();
            self.exprs.insert(
                id,
                MirExpr::StackArray {
                    elements: element_ids.clone(),
                    size,
                },
            );
            // The element types are markers only — every slot is a raw 64-bit
            // word — so hard-wiring them to I64 was free to do but not free to
            // keep: it erased a `Str` member, so unpacking `[(1, "a")]` bound the
            // loop var with no marker and `print(v)` printed the heap address.
            // Take each type from the lowered element instead.
            let tys = element_ids
                .iter()
                .map(|&eid| self.type_map.get(&eid).cloned().unwrap_or_else(Type::slot_fallback))
                .collect();
            self.type_map.insert(id, Type::Tuple(tys));
            self.tuple_slots.insert(id);
    id
    }

    /// `[v; n]` 数组重复字面量（批 886 自 gen.rs 原臂逐字迁入）。
    pub(super) fn lower_array_repeat(
        &mut self,
        value: &Box<AstNode>,
        size: &Box<AstNode>,
        id: u32,
    ) -> u32 {
            let value_id = self.lower_expr(value);

            // Get the type of the value expression
            let elem_type = self.type_map.get(&value_id).cloned().unwrap_or_else(Type::slot_fallback);

            // Check if size is a literal by examining the AST node directly
            // We need to pattern match on the boxed value
            match size.as_ref() {
                AstNode::Lit(size_lit) => {
                    let size_val = *size_lit as usize;

                    // HYBRID MEMORY SYSTEM: Use StackArray for small fixed-size arrays
                    if size_val <= 20000 {
                        // Reasonable stack size limit

                        // Create StackArray expression with repeated value
                        self.exprs.insert(
                            id,
                            MirExpr::StackArray {
                                elements: vec![value_id; size_val],
                                size: size_val,
                            },
                        );

                        // Set the type to Array(elem_type, size) for subscript access
                        self.type_map.insert(
                            id,
                            Type::Array(Box::new(elem_type), ArraySize::Literal(size_val)),
                        );
                    } else {
                        // Large array, use heap allocation

                        // Allocate array using array_new with capacity = size
                        let array_ptr = self.next_id();
                        let capacity_id = self.next_id();
                        self.exprs
                            .insert(capacity_id, MirExpr::IntLit(size_val as i64));
                        self.stmts.push(MirStmt::Call {
                            func: "array_new".to_string(),
                            args: vec![capacity_id],
                            dest: array_ptr,
                            type_args: vec![],
                        });

                        // Set array length first
                        let len_id = self.next_id();
                        self.exprs.insert(len_id, MirExpr::IntLit(size_val as i64));
                        self.stmts.push(MirStmt::VoidCall {
                            func: "array_set_len".to_string(),
                            args: vec![array_ptr, len_id],
                        });

                        // Fill array with value
                        // Use memset intrinsic for zero initialization (performance optimization)
                        let val_expr = value_id;
                        let is_lit_zero = match self.exprs.get(&val_expr) {
                            Some(MirExpr::IntLit(0)) => true,
                            _ => false,
                        };
                        if is_lit_zero && size_val > 4 {
                            // Zero initialization: use memset for efficiency
                            let byte_size_id = self.next_id();
                            let elem_byte_size = match &elem_type {
                                Type::I8 | Type::U8 | Type::Bool => 1,
                                Type::I16 | Type::U16 => 2,
                                Type::I32 | Type::U32 | Type::F32 => 4,
                                Type::I64 | Type::U64 | Type::F64 | Type::Usize => 8,
                                _ => 8, // Default to 8 bytes for complex types
                            };
                            self.exprs.insert(
                                byte_size_id,
                                MirExpr::IntLit((size_val * elem_byte_size) as i64),
                            );
                            self.stmts.push(MirStmt::VoidCall {
                                func: "__builtin_memset".to_string(),
                                args: vec![array_ptr, value_id, byte_size_id],
                            });
                        } else {
                            // Non-zero or small array: use per-element assignment
                            for idx in 0..size_val {
                                let idx_id = self.next_id();
                                self.exprs.insert(idx_id, MirExpr::IntLit(idx as i64));
                                self.stmts.push(MirStmt::VoidCall {
                                    func: "array_set".to_string(),
                                    args: vec![array_ptr, idx_id, value_id],
                                });
                            }
                        }

                        // Return the array pointer
                        self.exprs.insert(id, MirExpr::Var(array_ptr));
                        // Set the type to Array(elem_type, size) for subscript access
                        self.type_map.insert(
                            id,
                            Type::Array(Box::new(elem_type), ArraySize::Literal(size_val)),
                        );
                    }
                }
                _ => {
                    // Size is not a literal constant
                    // For now, create a placeholder
                    self.exprs.insert(id, MirExpr::IntLit(0));
                    self.type_map.insert(id, Type::I64);
                }
            }
    id
    }

}