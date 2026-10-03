//! 批次 861：f-string 发射体（原臂逐字迁入；批 860 写坏的本体本批重修）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_fstring(&mut self, parts: &[AstNode], id: u32) -> u32 {
            // PY-A: every part must be a string handle — non-string
            // expressions go through a to_string_* dispatch.
            let mut part_ids: Vec<u32> = Vec::new();
            for p in parts {
                // Batch 625: `f"{None}"` literal face renders "None" —
                // lowered as the ordinary StringLit part it is (value
                // representation stays 0, #113/#189 deep water).
                if matches!(p, AstNode::NoneLit) {
                    let pid = self.lower_expr(&AstNode::StringLit("None".to_string()));
                    part_ids.push(pid);
                    continue;
                }
                // Batch 654: a NoneVar part (`x = None; f"{x}"`) — same
                // render-at-consumption face as the print arm; the slot
                // stays I64 0.
                if let AstNode::Var(x) = p {
                    if self.none_vars_gen.contains(x) {
                        let pid = self.lower_expr(&AstNode::StringLit("None".to_string()));
                        part_ids.push(pid);
                        continue;
                    }
                }
                // Batch 659: a map Subscript part behind a Str-refined
                // value type (`f"n={d["k"]}"`) — same per-key-tag render
                // as the print arm; the raw word otherwise flowed into
                // str_concat as a "string" pointer (measured ZT-WARN +
                // truncated output).
                if let AstNode::Subscript { base, index } = p {
                    let base_map_str = if let AstNode::Var(vname) = &**base {
                        self.name_to_id
                            .get(vname.as_str())
                            .map_or(false, |&sid| {
                                matches!(
                                    self.type_map.get(&sid),
                                    Some(Type::Named(n, params))
                                        if n == "map"
                                            && matches!(params.last(), Some(Type::Str))
                                )
                            })
                    } else {
                        false
                    };
                    if base_map_str {
                        let recv_id = self.lower_expr(base);
                        let k_id = self.lower_expr(index);
                        let r_id = self.emit_call("zeta_map_get_render", vec![recv_id, k_id], Type::Str);
                        part_ids.push(r_id);
                        continue;
                    }
                }
                let pid = self.lower_expr(p);
                // An INLINE CONDITIONAL whose branches are both strings
                // (`f"{'sh' if exch == 'XSHG' else 'sz'}.{num}"`) left the
                // part typed i64, so `lower_to_string` stringified the raw
                // handle: `jq_to_bs("510300.XSHG")` returned
                // `4339988730.510300` instead of `sh.510300` — i.e. EVERY
                // code in the wufu universe came out numerically garbage.
                if matches!(self.type_map.get(&pid), Some(Type::I64) | Some(Type::PyDynamic)) {
                    if Self::both_branches_are_strings(p) {
                        self.type_map.insert(pid, Type::Str);
                    }
                }
                let pid = self.lower_to_string(pid);
                part_ids.push(pid);
            }
            self.exprs.insert(id, MirExpr::FString(part_ids));
            self.type_map.insert(id, Type::Str);
            id
    }
}
