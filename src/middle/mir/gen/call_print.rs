//! 批次 828：print 家族的按格渲染（767/808 值标签大弧 print 位）。
//! 路由 8=None 词/4=文本/5=i64/6=f64 位/7=bool 词，全不中落 i64；
//! 自底向上 else-if（len 链同构）。

use super::MirGen;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
/// 批次 827：print 位按格渲染链抽为独立方法（808 混型 dict None 值格）。
/// 路由 8=None 词/4=文本/5=i64/6=f64 位/7=bool 词，全不中落 i64；
/// 自底向上 else-if（767 len 链同构）。返回 true＝已发射（调用方 continue）。
pub(super) fn emit_tagged_print(&mut self, arg_id: u32, tag_slot: u32, is_last: bool) -> bool {
                        let nl_id = self.int_slot(is_last as i64);
                        let (s_pr, s_prn) = ("print_str", "println_str");
                        let (i_pr, i_prn) = ("print_i64", "println_i64");
                        let none_str = self.next_id();
                        self.exprs
                            .insert(none_str, MirExpr::StringLit("None".to_string()));
                        self.type_map.insert(none_str, Type::Str);
                        let layers: Vec<(i64, MirStmt)> = vec![
                            (
                                8,
                                MirStmt::VoidCall {
                                    func: (if is_last { s_prn } else { s_pr }).to_string(),
                                    args: vec![none_str],
                                },
                            ),
                            (
                                4,
                                MirStmt::VoidCall {
                                    func: (if is_last { s_prn } else { s_pr }).to_string(),
                                    args: vec![arg_id],
                                },
                            ),
                            (
                                5,
                                MirStmt::VoidCall {
                                    func: (if is_last { i_prn } else { i_pr }).to_string(),
                                    args: vec![arg_id],
                                },
                            ),
                            (
                                6,
                                MirStmt::VoidCall {
                                    func: "zeta_print_f64_word".to_string(),
                                    args: vec![arg_id, nl_id],
                                },
                            ),
                            (
                                7,
                                MirStmt::VoidCall {
                                    func: "zeta_print_bool_word".to_string(),
                                    args: vec![arg_id, nl_id],
                                },
                            ),
                        ];
                        let mut acc: Vec<MirStmt> = vec![MirStmt::VoidCall {
                            func: (if is_last { i_prn } else { i_pr }).to_string(),
                            args: vec![arg_id],
                        }];
                        for (tag, then_stmt) in layers.iter().rev() {
                            let lit = self.next_id();
                            self.exprs.insert(lit, MirExpr::IntLit(*tag));
                            self.type_map.insert(lit, Type::I64);
                            let cond = self.next_id();
                            self.exprs.insert(
                                cond,
                                MirExpr::BinaryOp {
                                    op: "==".to_string(),
                                    left: tag_slot,
                                    right: lit,
                                },
                            );
                            self.type_map.insert(cond, Type::I64);
                            acc = vec![MirStmt::If {
                                cond,
                                then: vec![then_stmt.clone()],
                                else_: std::mem::take(&mut acc),
                                dest: None,
                            }];
                        }
                        self.stmts.extend(acc);
                        return true;
    true
}
}
