//! 批次 873：DictLit 表达式臂发射体（键型传播／`{**m}` 展开／全 None 值
//! 判定／混型值格标签——869 零适配法迁出）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    pub(super) fn lower_dict_lit(&mut self, entries: &Vec<(AstNode, AstNode)>, id: u32) -> u32 {
            let map_id = id;
            self.stmts.push(MirStmt::MapNew { dest: map_id });
            let mut key_ty = Type::I64;
            // The VALUE type is tracked as the map's second type argument, so
            // `a = m["code"]` keeps DynamicArray(Str) instead of degrading to
            // I64 — `a[0]` then did a map_get on a Vec handle (SEGV, t193).
            let mut val_ty = Type::I64;
            let mut first_key = true;
            let mut first_val = true;
            // 批次 806：全 None 值判定（混型不动）
            let mut all_none_val = true;
            // 批次 808（混型 dict None 值格）：含 None 且含非 None ⇒
            // 混型——值型 PyDynamic（读侧 17638 格标签读随之点亮），
            // 字面量值写侧打格标签（8=None、4=文本、5=i64、6=f64 位、
            // 7=bool），print 位运行期按格渲染。非字面量值 tag 0 落
            // i64 缺省（残界）。纯字典零新增面。
            let has_none = entries.iter().any(|(_, v)| matches!(v, AstNode::NoneLit));
            let mixed = has_none && entries.iter().any(|(_, v)| !matches!(v, AstNode::NoneLit));
            for (k, v) in entries {
                // PY-A: `{**m, ...}` — merge m's entries into the literal.
                if let AstNode::Call {
                    receiver: None,
                    method,
                    args: ka,
                    ..
                } = k
                {
                    if method == "zeta_dict_spread" && ka.len() == 1 {
                        let src_id = self.lower_expr(&ka[0]);
                        // Take the key kind from the source map so a
                        // spread-only literal (`{**a}`) is still
                        // string-keyed and `d.keys()` stays Vec<str>.
                        if first_key {
                            if let Some(Type::Named(n, params)) =
                                self.type_map.get(&src_id).cloned()
                            {
                                if n == "map" {
                                    if let Some(kt) = params.first() {
                                        key_ty = kt.clone();
                                        first_key = false;
                                    }
                                }
                            }
                        }
                        let scratch = self.next_id();
                        self.stmts.push(MirStmt::Call {
                            func: "py_map_update".to_string(),
                            args: vec![map_id, src_id],
                            dest: scratch,
                            type_args: vec![],
                        });
                        continue;
                    }
                }
                let kid0 = self.lower_expr(k);
                if first_key {
                    // Remember whether keys are strings: the map type carries
                    // the key kind so `d.keys()` is typed Vec<str>.
                    key_ty = self.type_map.get(&kid0).cloned().unwrap_or_else(Type::slot_fallback);
                    first_key = false;
                }
                let kid = self.lower_map_key(kid0);
                let vid = self.lower_expr(v);
                if first_val {
                    val_ty = self.type_map.get(&vid).cloned().unwrap_or_else(Type::slot_fallback);
                    first_val = false;
                }
                // 批次 806（#113 携带读面）：全 None 值字典 ⇒ 值型
                // NoneValue（print 按型渲染 "None"，804 fromkeys 同款）。
                // 混型字典不动（首个值型 Wins 的旧约定，per-key 渲染另格）。
                if !matches!(v, AstNode::NoneLit) {
                    all_none_val = false;
                }
                self.stmts.push(MirStmt::DictInsert {
                    map_id,
                    key_id: kid,
                    val_id: vid,
                });
                // 批次 808：混型字典的字面量值打格标签（767 值标签大弧
                // 写侧；读侧格标签读由值型 PyDynamic 点亮，print 位按格
                // 渲染）。非字面量值不打（tag 0 落 i64 缺省＝残界）。
                if mixed {
                    let tag: Option<i64> = match v {
                        AstNode::NoneLit => Some(8),
                        AstNode::StringLit(_) => Some(4),
                        AstNode::Lit(_) => Some(5),
                        AstNode::Bool(_) => Some(7),
                        AstNode::FloatLit(_) => Some(6),
                        _ => None,
                    };
                    if let Some(t) = tag {
                        let tid = self.int_slot(t);
                        self.stmts.push(MirStmt::VoidCall {
                            func: "zeta_map_set_tag".to_string(),
                            args: vec![map_id, kid, tid],
                        });
                    }
                }
            }
            let key_ty = if matches!(key_ty, Type::Str) {
                Type::Str
            } else {
                Type::I64
            };
            if all_none_val && !entries.is_empty() {
                val_ty = Type::Named("NoneValue".to_string(), vec![]);
            }
            // 批次 808：混型（含 None 又含非 None）⇒ 值型 PyDynamic——
            // 读侧格标签读点亮（17638 门），print 位按格渲染；异构字典
            // 安全语义与 apply_dict_annotation 的 Any 同源。
            if mixed {
                val_ty = Type::PyDynamic;
            }
            self.exprs.insert(map_id, MirExpr::Var(map_id));
            self.type_map.insert(
                map_id,
                Type::Named("map".to_string(), vec![key_ty, val_ty]),
            );
            return map_id;
    id
    }
}
