//! 批次 816：集合家族的调用降级（gen.rs Call 巨臂拆家族第一刀）。
//! 按业界模板（rustc/Go/Swift 均按构造种类分文件）：一个家族一个文件、
//! 一张显式路由表。子模块可访问父模块 MirGen 私有字段。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// 集合族：add/discard/remove（写回接收者）与 intersection（返回新句柄）。
    /// 命中返回 Some(dest)；None = 不归本族，链上后续继续。
    pub(super) fn lower_set_family(
        &mut self,
        receiver: Option<&AstNode>,
        receiver_ty: &Type,
        method: &str,
        arg_ids: &[u32],
        dest: u32,
    ) -> Option<u32> {
                if set_like_receiver(receiver_ty)
                    && set_mutation_ok(method, arg_ids.len())
                {
                    if method == "add" && arg_ids.len() == 2 {
                        let elem_is_str = matches!(self.type_map.get(&arg_ids[1]), Some(Type::Str));
                        let flag = self.next_id();
                        self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                        self.type_map.insert(flag, Type::I64);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_add_unique".to_string(),
                            args: vec![arg_ids[0], arg_ids[1], flag],
                            dest: dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        // vec_push may reallocate and returns the new handle — write
                        // it back so a growing set is not silently lost.
                        if let Some(AstNode::Var(name)) = receiver.as_ref().map(|r| &**r) {
                            if let Some(&slot) = self.name_to_id.get(name) {
                                self.stmts.push(MirStmt::Assign { lhs: slot, rhs: dest });
                            }
                        }
                        self.type_map.insert(dest, receiver_ty.clone());
                        return Some(dest);
                    }
                    if arg_ids.len() == 2 {
                        // 批 928：f64 元素走 double 域剔除版（按位整数比较
                        // 让 xs.remove(1.5) 失配、元素原样留着）
                        let recv_elem_f64 = matches!(
                            self.type_map.get(&arg_ids[0]),
                            Some(Type::DynamicArray(e)) | Some(Type::Array(e, _))
                                if matches!(**e, Type::F64 | Type::F32)
                        );
                        if recv_elem_f64 {
                            self.stmts.push(MirStmt::Call {
                                func: "py_vec_discard_f64".to_string(),
                                args: vec![arg_ids[0], arg_ids[1]],
                                dest,
                                type_args: vec![],
                            });
                            self.exprs.insert(dest, MirExpr::Var(dest));
                            if let Some(AstNode::Var(name)) =
                                receiver.as_ref().map(|r| &**r)
                            {
                                if let Some(&slot) = self.name_to_id.get(name) {
                                    self.stmts.push(MirStmt::Assign {
                                        lhs: slot,
                                        rhs: dest,
                                    });
                                }
                            }
                            self.type_map.insert(dest, receiver_ty.clone());
                            return Some(dest);
                        }
                        let elem_is_str = matches!(
                            self.type_map.get(&arg_ids[1]),
                            Some(Type::Str)
                        );
                        let flag = self.next_id();
                        self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                        self.type_map.insert(flag, Type::I64);
                        self.stmts.push(MirStmt::Call {
                            func: "py_vec_discard".to_string(),
                            args: vec![arg_ids[0], arg_ids[1], flag],
                            dest: dest,
                            type_args: vec![],
                        });
                        self.exprs.insert(dest, MirExpr::Var(dest));
                        if let Some(AstNode::Var(name)) = receiver.as_ref().map(|r| &**r) {
                            if let Some(&slot) = self.name_to_id.get(name) {
                                self.stmts.push(MirStmt::Assign { lhs: slot, rhs: dest });
                            }
                        }
                        self.type_map.insert(dest, receiver_ty.clone());
                        return Some(dest);
                    }
                }
                // Batch 807: `sa.intersection(sb)` on a list-backed set. Batch
                // 10063: the same arm also serves `sa.union(sb)` — both take
                // (a, b, elem_is_str) and return a FRESH vector. Measured before
                // the arm: `sa.union(sb)` compiled with
                // "`[dynamic]i64::union` 无定义" and raised at run time
                // (`Unhandled exception: code=1`) while CPython prints the union.
                // The receiver's element type is what reached the ghost name
                // (`[dynamic]i64::intersection` / `[dynamic]str::…`), so batch
                // 428's rule turned the call site into a raise — measured 9 of
                // the 40 real strategy files writing
                // `list(set(temp).intersection(set(stockList)))`.
                // The result type INHERITS the receiver: a hard-coded I64 would
                // read a str column's handles as integers, swapping a loud raise
                // for a silent wrong value (this repo's worst class).
                if (method == "intersection" || method == "union")
                    && receiver.is_some()
                    && set_like_receiver(receiver_ty)
                    && set_new_set_ok(method, arg_ids.len())
                {
                    let elem_of_str = |t: Option<&Type>| match t {
                        Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                            matches!(**e, Type::Str)
                        }
                        Some(Type::Str) => true,
                        _ => false,
                    };
                    let elem_is_str =
                        elem_of_str(Some(receiver_ty)) || elem_of_str(self.type_map.get(&arg_ids[1]));
                    let flag = self.next_id();
                    self.exprs.insert(flag, MirExpr::IntLit(elem_is_str as i64));
                    self.type_map.insert(flag, Type::I64);
                    self.stmts.push(MirStmt::Call {
                        func: if method == "union" {
                            "py_vec_union".to_string()
                        } else {
                            "py_vec_intersect".to_string()
                        },
                        args: vec![arg_ids[0], arg_ids[1], flag],
                        dest: dest,
                        type_args: vec![],
                    });
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    // The helper returns a FRESH vector and never mutates the
                    // receiver, so no write-back here (unlike add/discard above).
                    self.type_map.insert(dest, receiver_ty.clone());
                    return Some(dest);
                }
        None
    }
}

/// 批次 816（TDD 内化）：集合族守卫的纯函数面——从 if 链抽出，可毫秒级单测。
/// 语义合同（CPython set 语义＋本仓约定）：
/// - 接收者像集合：DynamicArray/Array（list-backed set）、Named set/frozenset、
///   或非 Str 的 I64/PyDynamic（动态槽，运行期再判）；Str 明确不是集合。
/// - add/discard/remove 与 intersection/union 都要求恰好 2 参（含接收者），
///   否则不归本族（落链上后续，最终由响亮失败兜底）。
fn set_like_receiver(t: &Type) -> bool {
    matches!(t, Type::DynamicArray(_) | Type::Array(_, _))
        || matches!(t, Type::Named(n, _) if n == "set" || n == "frozenset")
        || (!matches!(t, Type::Str) && t.is_untyped())
}

fn set_mutation_ok(method: &str, arg_len: usize) -> bool {
    // 变异族（写回接收者）：add/discard/remove。**不含 intersection**——
    // 批次 816 抽取时曾把 intersection 误入本清单，`a.intersection(b)`
    // 走进 discard 臂＝交集结果变差集（t807 十行实拍 3/x/y），特此拆分。
    arg_len == 2 && matches!(method, "add" | "discard" | "remove")
}

/// 返回新集合、不写回接收者的一族：intersection 与 union（批次 10063 并入）。
/// 两者 C 侧同形（`int64_t f(int64_t a, int64_t b, int64_t elem_is_str)`），
/// 差别只在合并策略，故共用一条发射臂、按方法名选符号。
fn set_new_set_ok(method: &str, arg_len: usize) -> bool {
    arg_len == 2 && (method == "intersection" || method == "union")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dyn_str() -> Type { Type::DynamicArray(Box::new(Type::Str)) }
    fn dyn_i64() -> Type { Type::DynamicArray(Box::new(Type::I64)) }

    #[test]
    fn set_like_receiver_truth_table() {
        assert!(set_like_receiver(&dyn_str()));
        assert!(set_like_receiver(&dyn_i64()));
        assert!(set_like_receiver(&Type::Named("set".into(), vec![])));
        assert!(set_like_receiver(&Type::Named("frozenset".into(), vec![])));
        assert!(set_like_receiver(&Type::I64));       // 动态槽运行期再判
        assert!(set_like_receiver(&Type::PyDynamic)); // 同上
        assert!(!set_like_receiver(&Type::Str));      // 字符串不是集合
        assert!(!set_like_receiver(&Type::F64));      // 浮点不是集合
        assert!(!set_like_receiver(&Type::Bool));
    }

    #[test]
    fn set_mutation_truth_table() {
        for m in ["add", "discard", "remove"] {
            assert!(set_mutation_ok(m, 2), "{m} 两参应命中");
            assert!(!set_mutation_ok(m, 1), "{m} 单参不归本族");
            assert!(!set_mutation_ok(m, 3), "{m} 三参不归本族");
        }
        // 816 回归钉：intersection 绝不许进变异族（曾致交集变差集）
        assert!(!set_mutation_ok("intersection", 2));
        // 10063 同族钉：union 返回新集合，写回接收者＝并集变污染原集合
        assert!(!set_mutation_ok("union", 2));
        assert!(!set_mutation_ok("clear", 2));  // clear 归 map 族
        assert!(!set_mutation_ok("push", 2));   // vec 语义归他族
    }

    #[test]
    fn set_intersection_truth_table() {
        assert!(set_new_set_ok("intersection", 2));
        assert!(!set_new_set_ok("intersection", 1));
        assert!(!set_new_set_ok("intersection", 3));
        // 816 回归钉：intersection 不落 str__map/text 通用路
        assert!(!set_new_set_ok("add", 2));
        assert!(!set_new_set_ok("upper", 2));
    }

    /// 批次 10063：`sa.union(sb)` 与 intersection 同族（两参、返回新集合）。
    /// 改前实拍＝union 不走本族 ⇒ `[dynamic]i64::union` 无定义、运行期抛
    /// `Unhandled exception: code=1`（CPython 同程打印并集）。
    #[test]
    fn set_union_shares_the_new_set_arm() {
        assert!(set_new_set_ok("union", 2));
        assert!(!set_new_set_ok("union", 1));
        assert!(!set_new_set_ok("union", 3));
        // 邻近拼写不许被顺带接管（difference/symmetric_difference 未实现）
        assert!(!set_new_set_ok("difference", 2));
        assert!(!set_new_set_ok("symmetric_difference", 2));
        assert!(!set_new_set_ok("update", 2));
    }
}
