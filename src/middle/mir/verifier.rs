//! 批次 979（轴 G.6 首批）：MIR 结构不变量 verifier。
//!
//! 与 mir_diff.sh 分工：diff 管"变没变"（重构安全网），verifier 管
//! "合法不合法"（正确性闸门）——互补不互替。
//! 首批不变量：**悬空槽引用**——stmts（含嵌套块）中所有被引用的槽 id
//! 必须有定义（ParamInit / exprs 条目 / 循环变量 / 嵌套块 dest）。
//! 批 971 审计的 FuncAddr 覆盖丢失类 bug（槽被 env read 覆盖后引用
//! 悬空）由此一秒可抓。

use super::mir::{Mir, MirExpr, MirStmt};
use std::collections::HashSet;

/// 校验单个 Mir，返回违规清单（空 = 通过）。
pub fn verify(mir: &Mir) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    let mut defined: HashSet<u32> = HashSet::new();
    for k in mir.exprs.keys() {
        defined.insert(*k);
    }
    let mut uses: Vec<u32> = Vec::new();
    collect_stmt_uses_and_defs(&mir.stmts, &mut defined, &mut uses, &mut errors);
    for u in uses {
        if !defined.contains(&u) {
            errors.push(format!("悬空槽引用：ExprId {} 无定义", u));
        }
    }
    // type_map 引用存在性：type_map 记录的槽必须有定义
    for k in mir.type_map.keys() {
        if !defined.contains(k) {
            errors.push(format!("type_map 槽 {} 无定义", k));
        }
    }
    errors
}

/// 递归收集语句的 uses（引用）并就地登记 defs（定义）。
fn collect_stmt_uses_and_defs(
    stmts: &[MirStmt],
    defined: &mut HashSet<u32>,
    uses: &mut Vec<u32>,
    errors: &mut Vec<String>,
) {
    for stmt in stmts {
        match stmt {
            MirStmt::Assign { lhs, rhs } => {
                uses.push(*rhs);
                mark_def(*lhs, defined);
            }
            MirStmt::Call { args, dest, .. } => {
                uses.extend(args.iter().copied());
                mark_def(*dest, defined);
            }
            MirStmt::VoidCall { args, .. } => uses.extend(args.iter().copied()),
            MirStmt::Return { val } => uses.push(*val),
            MirStmt::SemiringFold { values, result, .. } => {
                uses.extend(values.iter().copied());
                mark_def(*result, defined);
            }
            MirStmt::ParamInit { param_id, .. } => mark_def(*param_id, defined),
            MirStmt::Consume { id } => uses.push(*id),
            MirStmt::If {
                cond,
                then,
                else_,
                dest,
            } => {
                uses.push(*cond);
                collect_stmt_uses_and_defs(then, defined, uses, errors);
                collect_stmt_uses_and_defs(else_, defined, uses, errors);
                if let Some(d) = dest {
                    mark_def(*d, defined);
                }
            }
            MirStmt::TryProp {
                expr_id,
                ok_dest,
                err_dest,
            } => {
                uses.push(*expr_id);
                mark_def(*ok_dest, defined);
                mark_def(*err_dest, defined);
            }
            MirStmt::DictInsert {
                map_id,
                key_id,
                val_id,
            } => {
                uses.push(*map_id);
                uses.push(*key_id);
                uses.push(*val_id);
            }
            MirStmt::DictGet {
                map_id,
                key_id,
                dest,
            } => {
                uses.push(*map_id);
                uses.push(*key_id);
                mark_def(*dest, defined);
            }
            MirStmt::MapNew { dest } => mark_def(*dest, defined),
            MirStmt::StructNew { fields, dest, .. } => {
                for (_, id) in fields {
                    uses.push(*id);
                }
                mark_def(*dest, defined);
            }
            MirStmt::For {
                iterator,
                var_id,
                counter_id,
                body,
                else_body,
                ..
            } => {
                uses.push(*iterator);
                mark_def(*var_id, defined);
                mark_def(*counter_id, defined);
                collect_stmt_uses_and_defs(body, defined, uses, errors);
                collect_stmt_uses_and_defs(else_body, defined, uses, errors);
            }
            MirStmt::While {
                pre_cond,
                cond,
                body,
                else_body,
                ..
            } => {
                // pre_cond 是语句列表（while 首迭条件在块内）
                collect_stmt_uses_and_defs(pre_cond, defined, uses, errors);
                uses.push(*cond);
                collect_stmt_uses_and_defs(body, defined, uses, errors);
                collect_stmt_uses_and_defs(else_body, defined, uses, errors);
            }
            MirStmt::Break | MirStmt::Continue => {}
            MirStmt::Swap { a_ptr, b_ptr, size } => {
                uses.push(*a_ptr);
                uses.push(*b_ptr);
                uses.push(*size);
            }
            MirStmt::Pre { cond, .. }
            | MirStmt::Post { cond, .. }
            | MirStmt::Invariant { cond, .. } => uses.push(*cond),
            MirStmt::Store { addr_id, val_id, .. } => {
                uses.push(*addr_id);
                uses.push(*val_id);
            }
            MirStmt::StructFieldStore {
                base_id, val_id, ..
            } => {
                uses.push(*base_id);
                uses.push(*val_id);
            }
        }
    }
}

fn mark_def(id: u32, defined: &mut HashSet<u32>) {
    defined.insert(id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middle::mir::mir::{Mir, MirExpr, MirStmt};
    use crate::middle::types::Type;
    use std::collections::HashMap;

    fn good_mir() -> Mir {
        let mut mir = Mir::default();
        mir.exprs.insert(1, MirExpr::IntLit(42));
        mir.exprs.insert(2, MirExpr::Var(1));
        mir.type_map.insert(1, Type::I64);
        mir.stmts.push(MirStmt::Assign { lhs: 2, rhs: 1 });
        mir
    }

    /// 好 MIR ⇒ 零违规（语料基线）。
    #[test]
    fn good_mir_passes() {
        assert!(verify(&good_mir()).is_empty());
    }

    /// 坏 MIR：Call 引用未定义槽 ⇒ 红（批 971 类 FuncAddr 覆盖丢失
    /// 的抓取器）。
    #[test]
    fn dangling_call_arg_is_caught() {
        let mut mir = good_mir();
        mir.stmts.push(MirStmt::Call {
            func: "foo".to_string(),
            args: vec![99],
            dest: 3,
            type_args: vec![],
        });
        mir.exprs.insert(3, MirExpr::Var(3));
        let errs = verify(&mir);
        assert!(
            errs.iter().any(|e| e.contains("99")),
            "悬空槽 99 应被抓：{:?}",
            errs
        );
    }

    /// type_map 记录的槽必须有定义。
    #[test]
    fn type_map_dangling_is_caught() {
        let mut mir = good_mir();
        mir.type_map.insert(777, Type::F64);
        let errs = verify(&mir);
        assert!(
            errs.iter().any(|e| e.contains("777")),
            "type_map 悬空槽 777 应被抓：{:?}",
            errs
        );
    }

    /// 嵌套块（If.then）内的引用同样检查。
    #[test]
    fn nested_block_refs_checked() {
        let mut mir = good_mir();
        mir.stmts.push(MirStmt::If {
            cond: 1,
            then: vec![MirStmt::VoidCall {
                func: "p".to_string(),
                args: vec![555],
            }],
            else_: vec![],
            dest: None,
        });
        let errs = verify(&mir);
        assert!(
            errs.iter().any(|e| e.contains("555")),
            "嵌套块悬空槽 555 应被抓：{:?}",
            errs
        );
    }

    /// ParamInit 定义参数槽（无 exprs 条目也合法）。
    #[test]
    fn param_init_counts_as_def() {
        let mut mir = Mir::default();
        mir.stmts.push(MirStmt::ParamInit {
            param_id: 1,
            arg_index: 0,
        });
        mir.stmts.push(MirStmt::VoidCall {
            func: "p".to_string(),
            args: vec![1],
        });
        mir.type_map.insert(1, Type::I64);
        assert!(verify(&mir).is_empty());
    }
}
