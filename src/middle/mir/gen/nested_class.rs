//! 批次 960（轴 D）：BATCH-438 嵌套类调用重绑族（869 零适配法自 gen.rs
//! 迁出）。函数体内 `class` 的方法在 lower_closure 里以裸名发射，
//! 这三方法在类窗口闭合后把调用点重绑到 hoisted 方法／按 batch 428
//! 幽灵化。主题聚合。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{Mir, MirStmt};
use crate::middle::types::Type;
use std::collections::HashMap;

impl MirGen {
    /// BATCH-438: bind the call sites written inside a `class` in a function body
    /// to that class's own hoisted methods.
    ///
    /// Why a pass AFTER the block instead of a table entry during it: every
    /// method of such a class goes through `lower_closure`, which clones the
    /// parent's `closure_vars` (see there) before this method's sibling has
    /// published anything. So `self._jq_bar_types()` inside `on_start`
    /// (`backend/strategy/nautilus_backend.py:54`) missed the closure dispatch,
    /// kept its qualified spelling, and codegen turned that into an external
    /// symbol nothing defines (`ld: Undefined symbols: __Impl___jq_bar_types` ⇒
    /// `Error: "Linking failed"`). The alias table is complete only once the loop
    /// is done, so the re-binding has to run over the Mir items the loop emitted.
    ///
    /// `{ty}::{member}` with no alias in THIS window is a member the class does
    /// not define at all (an inherited one: `self.subscribe_bars()` from
    /// `class _Impl(Strategy)`). Those get the `[dynamic]` spelling so batch 428's
    /// rule takes them — raise at the call site by name, instead of a declare the
    /// linker can only miss. Before this batch both spellings resolved against the
    /// ENCLOSING class and were papered over by two weak stubs in
    /// `runtime/unavailable_stubs.c:148-153`, which is the silent-wrong-value
    /// shape batch 438 exists to remove.
    ///
    /// Scope: only windows that published aliases are touched, i.e. only a `class`
    /// written inside a function body. A top-level `impl` keeps its pre-438
    /// spellings (its methods are ordinary items; its inherited members are still
    /// resolved downstream), so this pass cannot re-decide those call sites.
    pub(crate) fn rewrite_nested_class_calls(
        &mut self,
        ty: &str,
        aliases: &[(String, String)],
        mir_start: usize,
    ) {
        if ty.is_empty() || aliases.is_empty() {
            return;
        }
        let prefix = format!("{ty}::");
        let rets: HashMap<String, Type> = aliases
            .iter()
            .filter_map(|(key, sym)| {
                self.closure_ret_tys
                    .get(sym)
                    .map(|t| (key.clone(), t.clone()))
            })
            .collect();
        // "Something already defines it" has to be asked of the DEFINITIONS, not
        // of the call-site evidence table: `func_ret_types` gets a key for every
        // `recv.member` the scanner sees, so an undefined member was in it too and
        // the ghost arm never fired (measured: a nested `class Inner(Thing)` whose
        // `on_start` calls `self.notsdefined(x)` kept `Inner::notsdefined`, and
        // codegen emitted `ld: Undefined symbols: _Inner__notsdefined`).
        let defined: std::collections::HashSet<String> = self
            .generated_mirs
            .iter()
            .filter_map(|m| m.name.clone())
            .filter(|n| n.starts_with(&prefix))
            .collect();
        for mir in &mut self.generated_mirs[mir_start..] {
            let Mir { stmts, type_map, .. } = mir;
            Self::rebind_nested_calls(stmts, &prefix, aliases, &defined, &rets, type_map);
        }
    }

    /// BATCH-438: sweep a statement list, including the ones nested inside it.
    /// `mir.rs` nests statements in exactly three variants (`If`, `For`, `While`),
    /// so this covers the whole set; a top-level-only sweep misses every call
    /// written in a loop or branch body (measured: `for x in self.bar_types():
    /// self.notsdefined(x)` kept `Inner::notsdefined` ⇒ `ld: Undefined symbols:
    /// _Inner__notsdefined`, and in the corpus the same shape left
    /// `_Impl__subscribe_bars` undefined).
    pub(crate) fn rebind_nested_calls(
        stmts: &mut [MirStmt],
        prefix: &str,
        aliases: &[(String, String)],
        defined: &std::collections::HashSet<String>,
        rets: &HashMap<String, Type>,
        type_map: &mut HashMap<u32, Type>,
    ) {
        for stmt in stmts.iter_mut() {
            match stmt {
                MirStmt::Call { func, dest, .. } => {
                    if let Some(next) =
                        Self::rebound_nested_target(func, prefix, aliases, defined, rets, Some(*dest), type_map)
                    {
                        *func = next;
                    }
                }
                MirStmt::VoidCall { func, .. } => {
                    if let Some(next) =
                        Self::rebound_nested_target(func, prefix, aliases, defined, rets, None, type_map)
                    {
                        *func = next;
                    }
                }
                MirStmt::If { then, else_, .. } => {
                    Self::rebind_nested_calls(then, prefix, aliases, defined, rets, type_map);
                    Self::rebind_nested_calls(else_, prefix, aliases, defined, rets, type_map);
                }
                MirStmt::For {
                    body, else_body, ..
                } => {
                    Self::rebind_nested_calls(body, prefix, aliases, defined, rets, type_map);
                    Self::rebind_nested_calls(else_body, prefix, aliases, defined, rets, type_map);
                }
                MirStmt::While {
                    pre_cond,
                    body,
                    else_body,
                    ..
                } => {
                    Self::rebind_nested_calls(pre_cond, prefix, aliases, defined, rets, type_map);
                    Self::rebind_nested_calls(body, prefix, aliases, defined, rets, type_map);
                    Self::rebind_nested_calls(else_body, prefix, aliases, defined, rets, type_map);
                }
                _ => {}
            }
        }
    }

    /// The decision for one call target: bind to the class's own hoisted method,
    /// ghost an undefined member so batch 428 raises it, or leave the spelling
    /// alone (not this class's member, or a name an item really defines).
    pub(crate) fn rebound_nested_target(
        func: &str,
        prefix: &str,
        aliases: &[(String, String)],
        defined: &std::collections::HashSet<String>,
        rets: &HashMap<String, Type>,
        dest: Option<u32>,
        type_map: &mut HashMap<u32, Type>,
    ) -> Option<String> {
        if !func.starts_with(prefix) {
            return None;
        }
        if let Some((_, sym)) = aliases.iter().find(|(key, _)| key == func) {
            if let (Some(d), Some(t)) = (dest, rets.get(sym)) {
                type_map.insert(d, t.clone());
            }
            return Some(sym.clone());
        }
        if defined.contains(func) {
            return None;
        }
        Some(format!("[dynamic]{func}"))
    }
}
