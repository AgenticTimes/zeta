//! 批次 958（轴 D）：env-first 全局存取镜像族（869 法自 gen.rs 迁出）。
//! env_store/env_mirror/env_slot_ty/mirror_module_global_writes/
//! splice_env_mirrors/sub_splice/receiver_global_key——模块全局经
//! zeta_env_set/get 存取的镜像与裁剪机制，主题聚合。

use super::MirGen;
use crate::frontend::ast::AstNode;
use std::collections::HashMap;

use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// Names with no declared type keep the truncating store on purpose — their
    /// readers type the cell `I64`, and bits would come out as a huge integer
    /// instead of today's truncated one (closure `nonlocal` floats).
    pub(crate) fn env_store(&mut self, name: &str, value: u32) -> u32 {
        let (stmts, key_id) = self.env_mirror(name, value);
        self.stmts.extend(stmts);
        key_id
    }

    /// BUILD (do not emit) the statements of `env_store`, plus the key id they
    /// use. Split out so `splice_env_mirrors` can insert a mirror into the
    /// middle of an already-lowered statement list.
    pub(crate) fn env_mirror(&mut self, name: &str, value: u32) -> (Vec<MirStmt>, u32) {
        let mut out: Vec<MirStmt> = Vec::new();
        let key_id = self.next_id();
        self.exprs
            .insert(key_id, MirExpr::StringLit(name.to_string()));
        self.type_map.insert(key_id, Type::Str);
        let declared_float = matches!(self.global_ty_of(name), Some(Type::F32) | Some(Type::F64));
        let value_float = matches!(
            self.type_map.get(&value),
            Some(Type::F32) | Some(Type::F64)
        );
        let stored = if declared_float && value_float {
            // Read the word back out of the slot's own address: `load i64` from
            // the `double` alloca is the same reinterpretation the read side
            // does. A bare expression is put through a slot first, both to have
            // an address to read and because codegen re-evaluates expressions at
            // each use (see the `=`/`+=` mirror sites).
            let slot = match self.exprs.get(&value) {
                Some(MirExpr::Var(_)) => value,
                _ => {
                    let fresh = self.next_id();
                    self.exprs.insert(fresh, MirExpr::Var(fresh));
                    self.type_map
                        .insert(fresh, self.type_map.get(&value).cloned().unwrap_or(Type::F64));
                    out.push(MirStmt::Assign {
                        lhs: fresh,
                        rhs: value,
                    });
                    fresh
                }
            };
            let addr_id = self.next_id();
            self.exprs
                .insert(addr_id, MirExpr::AddrOf { alloca_id: slot });
            self.type_map.insert(addr_id, Type::I64);
            let bits_id = self.next_id();
            self.exprs.insert(
                bits_id,
                MirExpr::Deref {
                    addr_id,
                    pointee_width: 8,
                },
            );
            self.type_map.insert(bits_id, Type::I64);
            bits_id
        } else {
            value
        };
        out.push(MirStmt::VoidCall {
            func: "zeta_env_set".to_string(),
            args: vec![key_id, stored],
        });
        (out, key_id)
    }

    /// The type to give a slot that just read `name` out of the env cell: the
    /// name's declared type when it has one (the cell holds that value's word),
    /// `I64` otherwise. See `env_store` — write and read have to agree.
    pub(crate) fn env_slot_ty(&self, name: &str) -> Type {
        self.global_ty_of(name).unwrap_or(Type::slot_fallback())
    }

    /// PY-A: THE module-global write rule, in one place: every write to a
    /// module global's own slot refreshes the env cell that the other top-level
    /// items read. Runs after a body is lowered, over the whole statement list
    /// including nested blocks.
    ///
    /// This replaces the mirrors that used to sit beside individual STATEMENT
    /// kinds (`=` at the bind, `=` at the rebind, `+=`). Those could never cover
    /// the writes made from inside `lower_expr`: a container method rebinds its
    /// receiver slot at 6 separate sites (`push`, `append`, `add`,
    /// `insert`/`remove`/`sort`/`reverse`/`extend`, `discard`, `set.add`) —
    /// measured before this pass (`/tmp/b390/m1b.z`):
    /// `xs = [3,1,2]; xs.remove(3)` then `def peek() -> i64 { return len(xs) }`
    /// printed `3` while the module body's own `print(xs)` printed `[1, 2]`.
    /// One name, two answers, no diagnostic.
    ///
    /// The slot set is derived, not registered: within one item's lowering, a
    /// module-global name's slot IS `name_to_id[name]` — every writer takes its
    /// lhs from that same lookup, so no bind site has to remember to declare it.
    ///
    /// The mirror reads the SLOT it follows, never the `rhs` id: codegen
    /// re-evaluates an expression at each use, so handing over the right-hand
    /// side a second time counted it twice (`total = total + 3` in one function
    /// returned 7 while the cell held 11; `total += 1; total += 2` returned 20
    /// while the cell held 22 — batches 385/386).
    pub(crate) fn mirror_module_global_writes(&mut self) {
        let slots: Vec<(u32, String)> = self
            .name_to_id
            .iter()
            .filter(|(name, _)| self.module_globals.contains(*name))
            .map(|(name, &slot)| (slot, name.clone()))
            .collect();
        if slots.is_empty() {
            return;
        }
        let src = std::mem::take(&mut self.stmts);
        let mut out: Vec<MirStmt> = Vec::with_capacity(src.len());
        self.splice_env_mirrors(src, &slots, &mut out);
        self.stmts = out;
    }

    /// Copy `src` into `out`, inserting an env mirror after every `Assign` whose
    /// lhs is one of `slots`. `env_mirror` only allocates fresh ids, so splicing
    /// mid-list cannot alias a slot already lowered.
    pub(crate) fn splice_env_mirrors(
        &mut self,
        src: Vec<MirStmt>,
        slots: &[(u32, String)],
        out: &mut Vec<MirStmt>,
    ) {
        for st in src {
            match st {
                MirStmt::Assign { lhs, rhs } => {
                    out.push(MirStmt::Assign { lhs, rhs });
                    for &(slot, ref name) in slots {
                        if slot == lhs {
                            let (mirror, _key) = self.env_mirror(name, lhs);
                            out.extend(mirror);
                        }
                    }
                }
                MirStmt::If {
                    cond,
                    then,
                    else_,
                    dest,
                } => {
                    let (t, e) = (self.sub_splice(then, slots), self.sub_splice(else_, slots));
                    out.push(MirStmt::If {
                        cond,
                        then: t,
                        else_: e,
                        dest,
                    });
                }
                MirStmt::For {
                    iterator,
                    pattern,
                    var_id,
                    counter_id,
                    body,
                    else_body,
                } => {
                    let (b, e) = (self.sub_splice(body, slots), self.sub_splice(else_body, slots));
                    out.push(MirStmt::For {
                        iterator,
                        pattern,
                        var_id,
                        counter_id,
                        body: b,
                        else_body: e,
                    });
                }
                MirStmt::While {
                    cond,
                    pre_cond,
                    body,
                    else_body,
                } => {
                    let (p, b, e) = (
                        self.sub_splice(pre_cond, slots),
                        self.sub_splice(body, slots),
                        self.sub_splice(else_body, slots),
                    );
                    out.push(MirStmt::While {
                        cond,
                        pre_cond: p,
                        body: b,
                        else_body: e,
                    });
                }
                other => out.push(other),
            }
        }
    }

    pub(crate) fn sub_splice(&mut self, src: Vec<MirStmt>, slots: &[(u32, String)]) -> Vec<MirStmt> {
        let mut out = Vec::with_capacity(src.len());
        self.splice_env_mirrors(src, slots, &mut out);
        out
    }

    /// Key for a module-level global read: `Var(n)` -> n, and a module member
    /// (`mod.NAME`) -> `<module with . as _>__NAME` (plus the bare spelling,
    /// which `global_ty_of` also accepts).
    pub(crate) fn receiver_global_key(
        aliases: &HashMap<String, String>,
        r: &AstNode,
    ) -> Option<String> {
        match r {
            AstNode::Var(n) => Some(n.clone()),
            _ => {
                let (root, parts) = Self::flatten_module_receiver(r)?;
                if parts.len() != 1 {
                    return None;
                }
                let module = aliases.get(&root)?.clone();
                Some(format!("{}__{}", module.replace('.', "_"), parts[0]))
            }
        }
    }


}
