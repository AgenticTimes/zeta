//! 批次 881：Var 读臂发射体（nonlocal/env 优先读、模块全局、函数地址、
//! 捕获变量、未声明名告警——869 零适配法迁出）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_var_read(&mut self, name: &String, id: u32) -> u32 {
            // PY-A: `__file__` — the source path, known at compile time.
            // It is PER MODULE: reading the program-wide entry path made
            // `Path(__file__).parent…` arithmetic in an imported module
            // point at the ENTRY's ancestors (REasyQuant
            // `market_data_sources._PROJECT_ROOT` landed on
            // `/Users/meetai/source`, so every parquet cache miss).
            if name == "__file__" && !self.name_to_id.contains_key(name.as_str()) {
                let own = self
                    .py_module_paths
                    .get(&self.current_module)
                    .cloned()
                    .or_else(|| self.source_file.clone());
                if let Some(f) = own {
                    self.exprs.insert(id, MirExpr::StringLit(f));
                    self.type_map.insert(id, Type::Str);
                    return id;
                }
            }
            // PY-A: a module's own top-level name reads its module-global
            // slot (`mod__NAME` in the env). Locals win, so only rewrite
            // when nothing local shadows it.
            // `__name__` — Python's module-name string. It had NO handling at
            // all (a bare `print(__name__)` printed 1), and
            // `logging.getLogger(__name__)` produced a bad pointer that crashed
            // `fprintf`/`strlen` inside the first log line of `run_backtest`.
            if name == "__name__" && !self.name_to_id.contains_key("__name__") {
                let v = self.current_module.clone();
                self.exprs.insert(id, MirExpr::StringLit(v));
                self.type_map.insert(id, Type::Str);
                return id;
            }
            if !self.name_to_id.contains_key(name.as_str()) {
                if let Some(mangled) = self.symbol_renames.get(name.as_str()).cloned() {
                    if mangled != *name && self.module_globals.contains(&mangled) {
                        return self.lower_expr(&AstNode::Var(mangled));
                    }
                }
                // `from <module> import <VARIABLE>` — a member import read as
                // a bare NAME (not a call). Function imports work because
                // calls go through `py_member_call`; a variable had no path at
                // all, so `D` became an uninitialized slot and read 0:
                //   from regmod import D ; len(D)  →  0   (should be the dict)
                //   from regmod import D, reg      →  SEGV (bad handle)
                // The value lives in the module's env global `<mod>__<member>`
                // (the loader stores every module-level binding there).
                if let Some((module, member)) =
                    self.py_member_aliases.get(name.as_str()).cloned()
                {
                    // Canonicalize (same file under two module names — see
                    // `py_member_call`), or the env key carries the wrong
                    // prefix and the read misses.
                    let module = self
                        .py_module_aliases
                        .get(&module)
                        .cloned()
                        .unwrap_or(module);
                    if self.py_user_modules.contains(&module) {
                        let mangled =
                            format!("{}__{}", module.replace('.', "_"), member);
                        if self.module_globals.contains(&mangled) {
                            return self.lower_expr(&AstNode::Var(mangled));
                        }
                    }
                }
            }
            // PY-A V3: nonlocal names ALWAYS read through env (fresh
            // value), even when a local alias exists.
            if self.nonlocal_names.contains(name) {
                let key_id = self.next_id();
                self.exprs
                    .insert(key_id, MirExpr::StringLit(name.clone()));
                self.type_map.insert(key_id, Type::Str);
                let slot_id = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: "zeta_env_get".to_string(),
                    args: vec![key_id],
                    dest: slot_id,
                    type_args: vec![],
                });
                self.exprs.insert(slot_id, MirExpr::Var(slot_id));
                let ty = self.env_slot_ty(name);
                self.type_map.insert(slot_id, ty);
                return slot_id;
            }
            // 批 967：keyfn 单态化特化副本的裸读 ⇒ FuncAddr。独立早
            // 分支——副本不在 module_globals 名单，env-read 分支内的
            // FuncAddr 块永远走不到（实拍 keyfn 参数收 0x103）
            if name.contains(super::keyfn_bridge::SPEC_PREFIX)
                && !self.global_consts.contains_key(name)
                && !self.type_decls.contains_key(name)
            {
                let slot_id = self.next_id();
                self.exprs
                    .insert(slot_id, MirExpr::FuncAddr(name.clone()));
                self.type_map.insert(slot_id, Type::I64);
                return slot_id;
            }
            if let Some(&existing) = self.name_to_id.get(name) {
                // t425: in a module body (synthesized main or user main
                // with module statements merged in — both carry
                // PY_ENTRY_ATTR) a module global's own slot holds what
                // THIS body last wrote; a function called since may have
                // refreshed the env cell behind its back (`add()` mutates
                // `total`, batch 385 pinned the two-answers shape), so
                // those names read env-first here. Three gates keep the
                // slot when the cell cannot faithfully represent the
                // value: loop variables (their per-iteration value lives
                // only in the slot); class instances (their read side
                // needs "which ctor call made me" provenance, which an
                // env round-trip loses — measured: t464's two same-named
                // classes with swapped field orders both fell back to
                // the first-writer layout and the fields transposed); and
                // type mismatch — the cell reads back typed from the
                // name's declared/inferred type, and where that is
                // coarser than the module body's own binding the env
                // read would DOWNGRADE it (measured: `for k, v in pairs`
                // with pairs a module global — its slot carries
                // Tuple element types that the cell's `DynamicArray(I64)`
                // inference loses, and the destructure read garbage).
                let cell_ty = self.global_ty_of(name).unwrap_or(Type::slot_fallback());
                let env_first = self.py_entry
                    && self.module_globals.contains(name)
                    && !self.loop_var_active.contains(name)
                    && !matches!(cell_ty, Type::Named(..))
                    && self
                        .type_map
                        .get(&existing)
                        .map_or(false, |t| *t == cell_ty);
                if !env_first {
                    return existing;
                }
            }
            // PY-A: module-global name not bound locally — env read
            // (implicit module global: no global declaration needed).
            if self.module_globals.contains(name) {
                let key_id = self.next_id();
                self.exprs
                    .insert(key_id, MirExpr::StringLit(name.clone()));
                self.type_map.insert(key_id, Type::Str);
                let slot_id = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: "zeta_env_get".to_string(),
                    args: vec![key_id],
                    dest: slot_id,
                    type_args: vec![],
                });
                self.exprs.insert(slot_id, MirExpr::Var(slot_id));
                // Keep the global's static type: an untyped env read made
                // `q.put(x)` a bare call, because the handle tag was lost.
                let ty = self.global_ty_of(name).unwrap_or(Type::slot_fallback());
                self.type_map.insert(slot_id, ty);
                // A module's plain `def` never reaches the env, so a bare read
                // of one holds 0 and `zeta_call1(0, x)` no-ops by contract. Take
                // the symbol address instead, and leave the env read above alone:
                // that keeps statement and slot allocation identical to before, so
                // the only observable delta here is what the slot ends up holding.
                // 批 965：keyfn 单态化特化副本（__keyf64 后缀）也发
                // FuncAddr——副本 Mir 由 main 的补 lower 循环注入 mir_map，
                // codegen 据此生成其 LLVM 函数；条件不含 func_ret_types
                //（副本不在注解表）。
                let is_keyfn_spec = name.contains(super::keyfn_bridge::SPEC_PREFIX)
                    && !self.global_consts.contains_key(name)
                    && !self.type_decls.contains_key(name);
                if (self.func_ret_types.contains_key(name) || is_keyfn_spec)
                    && !self.global_consts.contains_key(name)
                    && !self.type_decls.contains_key(name)
                {
                    self.exprs.insert(slot_id, MirExpr::FuncAddr(name.clone()));
                    self.type_map.insert(slot_id, Type::I64);
                }
                return slot_id;
            }
            // PY-A V3: nonlocal name not bound locally — env read.
            if self.nonlocal_names.contains(name) {
                let key_id = self.next_id();
                self.exprs
                    .insert(key_id, MirExpr::StringLit(name.clone()));
                self.type_map.insert(key_id, Type::Str);
                let slot_id = self.next_id();
                self.stmts.push(MirStmt::Call {
                    func: "zeta_env_get".to_string(),
                    args: vec![key_id],
                    dest: slot_id,
                    type_args: vec![],
                });
                self.exprs.insert(slot_id, MirExpr::Var(slot_id));
                let ty = self.env_slot_ty(name);
                self.type_map.insert(slot_id, ty);
                return slot_id;
            }
            // Batch 593: a nested `def` hoisted as a closure — the name
            // used as a VALUE (`return inner`; `f = inner`) must produce
            // the synthetic function's address, exactly like a lambda
            // value. Without this the read fell to the implicit-declare
            // slot and `return inner` handed back an uninitialized word
            // (indirect call → SIGBUS; closure_nonlocal).
            if let Some(hoisted) = self
                .closure_vars
                .get(name.as_str())
                .cloned()
                .or_else(|| self.hoisted_names.get(name.as_str()).cloned())
            {
                let id = self.next_id();
                self.exprs.insert(id, MirExpr::FuncAddr(hoisted));
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // Unit-variant path of a registered enum (e.g. `Color::Green`)
            // lowers to its variant tag (integer discriminant).
            //
            // This must be checked BEFORE the "bare function name as value"
            // path below: the resolver registers every enum variant as a
            // constructor signature, so `func_ret_types` contains
            // `Color::Green` and the name was lowered to
            // `FuncAddr("Color::Green")` — the address of a function that is
            // declared but never defined. Measured on `let c = Color::Green;`
            // used as a value: link fails with `Undefined symbols
            // "_Color__Green"`; when the slot is dead-code-eliminated the
            // scrutinee holds garbage and every `match` arm falls through to
            // `_` (`match c { Color::Red => 100, Color::Green => 200, _ => 300 }`
            // printed 300).
            if name.contains("::") {
                if let Some(tag) = self.enum_unit_variant_index(name) {
                    // An enum that has ANY payload-carrying variant is boxed
                    // as a whole, so this unit variant's value has to be a
                    // `[tag]` block too — a bare integer discriminant could
                    // not be told apart from a block pointer by the arm test.
                    let boxed = self
                        .enum_variant_of(name)
                        .map(|(enum_name, _, _)| self.enum_is_boxed(&enum_name))
                        .unwrap_or(false);
                    if boxed {
                        let tag_id = self.next_id();
                        self.exprs.insert(tag_id, MirExpr::IntLit(tag));
                        self.type_map.insert(tag_id, Type::I64);
                        self.exprs.insert(
                            id,
                            MirExpr::Struct {
                                variant: name.clone(),
                                fields: vec![("__tag".to_string(), tag_id)],
                            },
                        );
                    } else {
                        self.exprs.insert(id, MirExpr::IntLit(tag));
                    }
                    self.type_map.insert(id, Type::I64);
                    return id;
                }
            }

            // PY-A: a bare known function name used as a value (e.g.
            // `threading.Thread(work)`) becomes its address. Constants
            // share the signature table, so exclude them — they lower to
            // their value below, not to a symbol.
            if self.func_ret_types.contains_key(name)
                && !self.global_consts.contains_key(name)
            {
                self.exprs.insert(id, MirExpr::FuncAddr(name.clone()));
                self.type_map.insert(id, Type::I64);
                return id;
            }

            // Check if this is a global constant
            if let Some(const_val) = self.global_consts.get(name) {
                match const_val {
                    crate::middle::ctfe::value::ConstValue::Int(n) => {
                        self.exprs.insert(id, MirExpr::IntLit(*n));
                        self.type_map.insert(id, Type::I64);
                        return id;
                    }
                    crate::middle::ctfe::value::ConstValue::String(s) => {
                        self.exprs
                            .insert(id, MirExpr::StringLit(s.clone()));
                        self.type_map.insert(id, Type::Str);
                        return id;
                    }
                    crate::middle::ctfe::value::ConstValue::Array(elements) => {
                        // For array constants, create a StackArray expression
                        let array_size = elements.len();
                        let mut element_ids = Vec::new();

                        // Extract integer values first to avoid borrow issues
                        let mut int_values = Vec::new();
                        for elem in elements {
                            match elem {
                                crate::middle::ctfe::value::ConstValue::Int(n) => {
                                    int_values.push(*n);
                                }
                                _ => {
                                    // Fallback to regular variable if not simple int
                                    self.exprs.insert(id, MirExpr::Var(id));
                                    self.type_map.insert(id, Type::I64);
                                    return id;
                                }
                            }
                        }

                        // Now create MIR expressions (can mutate self)
                        for n in int_values {
                            let elem_id = self.next_id();
                            self.exprs.insert(elem_id, MirExpr::IntLit(n));
                            self.type_map.insert(elem_id, Type::I64);
                            element_ids.push(elem_id);
                        }

                        self.exprs.insert(
                            id,
                            MirExpr::StackArray {
                                elements: element_ids,
                                size: array_size,
                            },
                        );
                        self.type_map.insert(
                            id,
                            Type::Array(
                                Box::new(Type::I64),
                                crate::middle::types::ArraySize::Literal(array_size),
                            ),
                        );
                        return id;
                    }
                    _ => {
                        // Fallback to regular variable
                        self.warn_undeclared(name);
                        self.exprs.insert(id, MirExpr::Var(id));
                        self.type_map.insert(id, Type::I64);
                    }
                }
            } else if name.ends_with("::MAX") || name == "true" || name == "false" {
                // Built-in constants (u64::MAX, usize::MAX, etc. in expression context)
                let val = if name.ends_with("::MAX") {
                    -1
                } else if name == "true" {
                    1
                } else {
                    0
                };
                self.exprs.insert(id, MirExpr::IntLit(val));
                self.type_map.insert(id, Type::I64);
            } else {
                // Regular variable
                self.warn_undeclared(name);
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::I64);
            }
    id
    }
}
