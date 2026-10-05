//! 批次 885：PathCall 臂发射体（`module.func(...)`／`Type::func(...)` 的
//! 静态路径调用——869 零适配法迁出）。

use super::MirGen;
use super::call_str::path_ends_with_mem;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::{ArraySize, Type};

impl MirGen {
    pub(super) fn lower_path_call(
        &mut self,
        path: &Vec<String>,
        method: &String,
        args: &Vec<AstNode>,
        type_args: &Vec<String>,
        id: u32,
    ) -> u32 {
            // Construct qualified name: path::method
            let func_name = if path.is_empty() {
                method.clone()
            } else {
                format!("{}::{}", path.join("::"), method)
            };

            // If this is a path-qualified call with an uppercase method name
            // (e.g., AstNode::Lit(42)), it's an enum variant constructor —
            // emit Struct instead of Call. Lowercase method names like
            // `LLVMCodegen::new("bench")` are regular (static) function calls.
            // Type_args also indicate a generic function call.
            let is_upper = !method.is_empty()
                && method
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false);
            // `std::mem::size_of::<T>()` / `align_of::<T>()` → the bare
            // intrinsic name the codegen intercept answers
            // (`codegen.rs:4323`, width from `type_args`). That intercept has
            // been dead since it was written: every route in this arm is gated
            // either on `is_upper` — the *method's* first letter, so `size_of`
            // is lowercase — or on a hardcoded path, and none covered
            // `std::mem`. The call then fell out of the arm with no statement
            // and no `exprs` entry: `let n = size_of::<i64>()` printed 0, and
            // the same phantom id as a `*` operand panicked codegen at
            // `exprs[&values[1]]`. `zeta_src/runtime/array.z:50` is that second
            // shape, so every grow was `malloc(0 * count)`.
            if path_ends_with_mem(path)
                && (method == "size_of" || method == "align_of")
                && args.is_empty()
            {
                let mut mir_type_args = Vec::new();
                // Only the sub-word widths need naming: the intercept's
                // fallback arm answers 8, which is also what an unmapped
                // (generic `T`) type arg must get.
                if let Some(t) = type_args.first().map(|s| s.trim()) {
                    let w = match t {
                        "i8" | "int8" => Some(Type::I8),
                        "u8" | "uint8" | "byte" => Some(Type::U8),
                        "i16" => Some(Type::I16),
                        "u16" => Some(Type::U16),
                        "i32" | "int32" => Some(Type::I32),
                        "u32" | "uint32" => Some(Type::U32),
                        "f32" | "float32" => Some(Type::F32),
                        "char" => Some(Type::Char),
                        _ => None,
                    };
                    if let Some(w) = w {
                        mir_type_args.push(w);
                    }
                }
                self.stmts.push(MirStmt::Call {
                    func: method.clone(),
                    args: vec![],
                    dest: id,
                    type_args: mir_type_args,
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
                return id;
            }
            // PY-A: `std::time::now()` → monotonic_ns runtime
            if path.len() == 2 && path[0] == "std" && path[1] == "time"
                && method == "now" && args.is_empty()
            {
                self.emit_call_into(id, "monotonic_ns", vec![], Type::I64);
                return id;
            }

            // PY-A: `std::quantum::*::new` — V1 placeholder platform objects
            // (batch 446: the gate asks whether a `quantum` MODULE segment is
            // in the path, not who spelled its prefix. `path[0] == "std"`
            // accepted only `std::quantum::Circuit::new(2)` and left the
            // `import std::quantum;` + `quantum::Circuit::new(2)` spelling
            // (1 census line) reading the ghost 0. The last segment is the
            // class, so `quantum` must sit before it — `quantum::new()` keeps
            // its old answer.)
            if path.len() >= 2 && method == "new" && args.len() <= 2
                && path[..path.len() - 1].iter().any(|s| s == "quantum")
            {
                let n_id = if args.is_empty() {
                    let z = self.next_id();
                    self.exprs.insert(z, MirExpr::IntLit(1));
                    self.type_map.insert(z, Type::I64);
                    z
                } else {
                    self.lower_expr(&args[0])
                };
                self.emit_call_into(id, "zeta_qc_new", vec![n_id], Type::I64);
                return id;
            }
            // `std::quantum::QubitState::one/zero` etc. (batch 446: same
            // module-segment gate as the route above, so the `quantum::`
            // spelling after `import std::quantum;` binds too.)
            if path.len() >= 2
                && matches!(
                    method.as_str(),
                    "one" | "zero" | "plus" | "minus" | "conj" | "norm" | "abs"
                )
                && args.len() <= 1
                && path[..path.len() - 1].iter().any(|s| s == "quantum")
            {
                let z = self.next_id();
                self.exprs.insert(z, MirExpr::IntLit(1));
                self.type_map.insert(z, Type::I64);
                self.emit_call_into(id, "zeta_qc_is_normalized", vec![z], Type::I64);
                return id;
            }

            // PY-A: `std::memory::capability::new(n)` — capability handle
            if path.len() == 3 && path[0] == "std" && path[1] == "memory"
                && path[2] == "capability" && method == "new" && args.len() == 1
            {
                let n_id = self.lower_expr(&args[0]);
                self.emit_call_into(id, "zeta_dynarray_new", vec![n_id], Type::I64);
                return id;
            }

            // A payload-carrying variant of a registered user enum
            // (`Token::Ident(5)`, `Ast::Lit(0)`) constructs the value block
            // `[tag, p0, …]` that the arm test reads (see `enum_is_boxed`).
            //
            // This must come before every capitalized-name route below: the
            // platform-class catch-all swallowed `Enum::Variant(args)` first
            // and emitted `zeta_platform_obj("Ident", 5)`, so the tag was
            // never written and no arm could tell the value from any other.
            if path.len() == 1 && type_args.is_empty() {
                let qualified = format!("{}::{}", path[0], method);
                if let Some((_, tag, payload_n)) = self.enum_variant_of(&qualified) {
                    if payload_n == args.len() {
                        let tag_id = self.next_id();
                        self.exprs.insert(tag_id, MirExpr::IntLit(tag));
                        self.type_map.insert(tag_id, Type::I64);
                        let mut fields: Vec<(String, u32)> =
                            vec![("__tag".to_string(), tag_id)];
                        for a in args {
                            let arg_id = self.lower_expr(a);
                            fields.push((format!("f{}", fields.len() - 1), arg_id));
                        }
                        self.exprs.insert(
                            id,
                            MirExpr::Struct {
                                variant: qualified,
                                fields,
                            },
                        );
                        self.type_map.insert(id, Type::I64);
                        return id;
                    }
                }
            }

            // A qualified name the Resolver has a signature for IS a call —
            // regardless of the case of its last segment. Everything below
            // this point is a PY-A dialect heuristic ("a capitalized bare
            // name is a handle constructor"), and those heuristics used to
            // run first: `Parser::new(src)` never reached a call site at all
            // (the `new`-excluded catch-all fell through the capitalised-only
            // general route, leaving a dangling expr id whose slot stayed 0 ⇒
            // null `self` ⇒ SIGSEGV), while `Parser::tokenize(src)` was
            // rewritten into `zeta_platform_obj("tokenize", …)` and printed a
            // heap address. Ask the table before the guess.
            if !path.is_empty() && type_args.is_empty()
                && self.func_ret_types.contains_key(&func_name)
            {
                let mut arg_ids = Vec::with_capacity(args.len());
                for a in args {
                    arg_ids.push(self.lower_expr(a));
                }
                let ret_ty = self
                    .func_ret_types
                    .get(&func_name)
                    .cloned()
                    .unwrap_or(Type::slot_fallback());
                self.stmts.push(MirStmt::Call {
                    func: func_name.clone(),
                    args: arg_ids,
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, ret_ty);
                return id;
            }

            // BATCH-446: `HashMap::new()` is an EMPTY MAP, not a ghost. The
            // table route above still wins when the project really defines a
            // `HashMap`; when nothing does, every consumer read slot 0 —
            // `zeta_src/runtime/actor/map.z:17` (`inner: HashMap::new()`) and
            // `tests/stdlib-foundation/collections_test.z:40` among the 5 census
            // lines in 443's §五 list. `map_new` is a DEFINED C host
            // (`runtime/tokio_runtime_stub.c:213`, unlike `fs_read_to_string`
            // which only exists Rust-side and fails to link — measured this
            // batch), and `MirStmt::MapNew` is the statement the dict literal at
            // :5540 and `dict(m)` at :8456 already emit, so the handle and the
            // `map` tag match the shape `map_insert` / `len` / `[]` expect.
            if args.is_empty() && path.last().map(|s| s.as_str()) == Some("HashMap")
            {
                self.stmts.push(MirStmt::MapNew { dest: id });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::Named("map".to_string(), vec![]));
                return id;
            }

            // PY-A: platform class constructors (FixedSlippage(0.001),
            // OrderCost(...), MarketOrderStyle(...), etc.) → opaque handle.
            // method=="new" excluded — Vec::new()/DynArray::new() have
            // dedicated intercepts below.
            if path.len() == 1 && args.len() <= 8 && method != "new" {
                let class_id = self.next_id();
                self.exprs
                    .insert(class_id, MirExpr::StringLit(method.clone()));
                self.type_map.insert(class_id, Type::Str);
                let mut call_args = vec![class_id];
                let mut arg_ids2 = vec![];
                for a in args {
                    arg_ids2.push(self.lower_expr(a));
                }
                call_args.extend(arg_ids2);
                self.stmts.push(MirStmt::Call {
                    func: "zeta_platform_obj".to_string(),
                    args: call_args,
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // PY-A: `memory::BitArray::new(n)` / `memory::Sieve::new(n)` /
            // `memory::DynamicArray::new(n)` — opaque handle allocation.
            if path.len() == 2 && path[0] == "memory" && method == "new"
                && args.len() == 1
            {
                let n_id = self.lower_expr(&args[0]);
                let alloc = match path[1].as_str() {
                    "BitArray" => "zeta_bitarray_new",
                    "Sieve" => "zeta_sieve_new",
                    _ => "zeta_dynarray_new",
                };
                self.stmts.push(MirStmt::Call {
                    func: alloc.to_string(),
                    args: vec![n_id],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // `Box::new(v)` is an IDENTITY: a `Box<T>` slot is the same
            // 64-bit handle as the value inside it. Before this arm the call
            // lowered to NOTHING (no statement, no `exprs` entry), so the
            // phantom id read back as garbage through a `let` and crashed the
            // backend where struct literals index `exprs[field_id]`
            // (codegen.rs:6176, hit via minimal_compiler.z:129).
            if path.len() == 1 && path[0] == "Box" && method == "new" && args.len() == 1 {
                return self.lower_expr(&args[0]);
            }
            // `String::new()` — same transparency, same phantom-id hole.
            if path.len() == 1 && path[0] == "String" && method == "new" && args.is_empty() {
                self.exprs.insert(id, MirExpr::StringLit(String::new()));
                self.type_map.insert(id, Type::Str);
                return id;
            }

            // PY-A: `Vec::new()` → runtime vec_new with initial capacity
            // (vec_push growth doubles from cap, so cap must be > 0).
            if path.len() == 1 && path[0] == "Vec" && method == "new" && args.is_empty() {
                let cap_id = self.next_id();
                self.exprs.insert(cap_id, MirExpr::IntLit(8));
                self.type_map.insert(cap_id, Type::I64);
                self.stmts.push(MirStmt::Call {
                    func: "vec_new".to_string(),
                    args: vec![cap_id],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map
                    .insert(id, Type::DynamicArray(Box::new(Type::I64)));
                return id;
            }
            // PY-A: platform class constructors (FixedSlippage(0.001),
            // OrderCost(...), MarketOrderStyle(...), MACD(...)) — these
            // are single-segment capitalized calls from Python sources.
            // Route them to the opaque platform-object runtime rather
            // than an enum Struct (which has no runtime symbol).
            if path.len() == 1 && type_args.is_empty() && is_upper {
                let class_id = self.next_id();
                self.exprs
                    .insert(class_id, MirExpr::StringLit(method.clone()));
                self.type_map.insert(class_id, Type::Str);
                let mut call_args = vec![class_id];
                let mut arg_ids2 = vec![];
                for a in args {
                    arg_ids2.push(self.lower_expr(a));
                }
                call_args.extend(arg_ids2);
                self.stmts.push(MirStmt::Call {
                    func: "zeta_platform_obj".to_string(),
                    args: call_args,
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
                return id;
            }
            if !path.is_empty() && type_args.is_empty() && is_upper {
                // Regular function call or unqualified call
                // Generate argument IDs
                // PY-A: starred args `f(*arr)` expand to per-element args
                // (V1: static-size arrays only — compile-time unrolling).
                let mut arg_ids: Vec<u32> = Vec::new();
                for a in args {
                    if let AstNode::UnaryOp { op, expr } = a {
                        if op == "*" {
                            let arr_id = self.lower_expr(expr);
                            let n = match self.type_map.get(&arr_id).cloned() {
                                Some(Type::Array(_, ArraySize::Literal(n))) => n,
                                _ => 0,
                            };
                            for i in 0..n {
                                let idx_id = self.next_id();
                                self.exprs.insert(idx_id, MirExpr::IntLit(i as i64));
                                self.type_map.insert(idx_id, Type::I64);
                                let elem = self.emit_call("array_get", vec![arr_id, idx_id], Type::I64);
                                arg_ids.push(elem);
                            }
                            continue;
                        }
                    }
                    arg_ids.push(self.lower_expr(a));
                }

                // Convert type arguments from strings to Type objects
                let mir_type_args: Vec<Type> =
                    type_args.iter().map(|t| Type::from_string(t)).collect();

                // PY-A: zeta_* runtime-dispatched names must stay bare —
                // the arity suffix would make the call miss the runtime
                // symbol (get_or_declare strips it, but the emitted call
                // still references the suffixed name directly).
                let call_name = if func_name.starts_with("zeta_")
                    || func_name.contains("__")
                {
                    func_name.clone()
                } else {
                    format!("{}_{}", func_name, arg_ids.len())
                };
                self.stmts.push(MirStmt::Call {
                    func: call_name,
                    args: arg_ids,
                    dest: id,
                    type_args: mir_type_args,
                });

                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
            }
    id
    }
}
