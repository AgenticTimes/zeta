//! 批次 819/840：`len()` 内建——路由分类（classify_len）与发射体
//! （lower_len）同文件；判定与发射分离但归族一处。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::{ArraySize, Type};

/// `len(x)` 的发射路由。
#[derive(Debug, PartialEq, Eq)]
pub enum LenRoute {
    KnownLength(usize),
    Str,
    Map,
    PyJson,
    Vec,
    DynLen,
}

/// 纯函数：静态类型 → 发射路由。
pub fn classify_len(arg_ty: Option<&Type>) -> LenRoute {
    match arg_ty {
        Some(Type::Array(_, ArraySize::Literal(n))) => LenRoute::KnownLength(*n),
        Some(Type::Str) => LenRoute::Str,
        Some(t) if t.is_map() => LenRoute::Map,
        Some(Type::Named(n, _)) if n == "PyJson" => LenRoute::PyJson,
        Some(Type::DynamicArray(_)) => LenRoute::Vec,
        None | Some(Type::I64) | Some(Type::PyDynamic) => LenRoute::DynLen,
        _ => LenRoute::DynLen,
    }
}

impl MirGen {
    /// 批次 840：`len(x)` 发射体整体迁入（classify_len 同文件）。
    /// 返回 Some(dest)＝已发射。
    pub(super) fn lower_len(
        &mut self,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
                    let arg_id = self.lower_expr(&args[0]);
                    let arg_ty = self.type_map.get(&arg_id).cloned();
                    // 批次146 重放: `len(obj)` dispatches to the object's
                    // `__len__` method (t196: `len(F())`, `len(df)`).
                    if let Some(Type::Named(n, _)) = &arg_ty {
                        if let Some(qlen) = self.qualified_method_candidate(n, "__len__") {
                            self.emit_call_into(dest, &qlen, vec![arg_id], Type::I64);
                            return Some(dest);
                        }
                    }
                    // Batch 767 (值标签大弧·读侧按格分派)：PyDynamic 实参带格
                    // 标签 ⇒ 运行期 tag == 类 id 的按格分派（有 __len__ 的类，
                    // 名字序定链序），全不中落 zeta_dyn_len 几何兜底。t450 的
                    // len(c["df"]) 由这里兑现 2（DataFrame.__len__）。
                    if arg_ty.as_ref().map_or(true, |t| t.is_dynamic()) {
                        if let Some(tag_slot) = self.slot_tags.get(&arg_id).cloned() {
                            let mut candidates: Vec<(i64, String)> = Vec::new();
                            let mut cls_names: Vec<&String> =
                                self.type_decls.keys().collect();
                            cls_names.sort();
                            for cn in cls_names {
                                if let (Some(t), Some(q)) = (
                                    self.class_tag_id(cn),
                                    self.qualified_method_candidate(cn, "__len__"),
                                ) {
                                    candidates.push((t, q));
                                }
                            }
                            if !candidates.is_empty() {
                                let mut chain_else: Vec<MirStmt> =
                                    vec![MirStmt::Call {
                                        func: "zeta_dyn_len".to_string(),
                                        args: vec![arg_id],
                                        dest: dest,
                                        type_args: vec![],
                                    }];
                                for (tag_val, qlen) in candidates.iter().rev() {
                                    let lit = self.next_id();
                                    self.exprs.insert(lit, MirExpr::IntLit(*tag_val));
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
                                    self.type_map.insert(cond, Type::Bool);
                                    let then_stmts = vec![MirStmt::Call {
                                        func: qlen.clone(),
                                        args: vec![arg_id],
                                        dest: dest,
                                        type_args: vec![],
                                    }];
                                    let if_stmt = MirStmt::If {
                                        cond,
                                        then: then_stmts,
                                        else_: chain_else,
                                        dest: None,
                                    };
                                    chain_else = vec![if_stmt];
                                }
                                self.stmts.extend(chain_else);
                                self.exprs.insert(dest, MirExpr::Var(dest));
                                self.type_map.insert(dest, Type::I64);
                                return Some(dest);
                            }
                        }
                    }
                    match arg_ty {
                        Some(Type::Array(_, ArraySize::Literal(n))) => {
                            self.exprs.insert(dest, MirExpr::IntLit(n as i64));
                            self.type_map.insert(dest, Type::I64);
                            return Some(dest);
                        }
                        Some(Type::Str) => {
                            self.stmts.push(MirStmt::Call {
                                func: "str_len".to_string(),
                                args: vec![arg_id],
                                dest: dest,
                                type_args: vec![],
                            });
                        }
                        Some(t) if t.is_map() => {
                            // len(dict/Counter): count the used slots.
                            // 批次 819：路由判定收敛到 classify_len（含 dict
                            // 拼写等价——815 规则，TDD 曾抓到本臂漏 dict）。
                            self.stmts.push(MirStmt::Call {
                                func: "zeta_map_len".to_string(),
                                args: vec![arg_id],
                                dest: dest,
                                type_args: vec![],
                            });
                        }
                        Some(Type::Named(n, _)) if n == "PyJson" => {
                            // Json length by tag: array/object/string.
                            self.stmts.push(MirStmt::Call {
                                func: "py_json_len".to_string(),
                                args: vec![arg_id],
                                dest: dest,
                                type_args: vec![],
                            });
                        }
                        Some(Type::DynamicArray(_)) => {
                            self.stmts.push(MirStmt::Call {
                                func: "vec_len".to_string(),
                                args: vec![arg_id],
                                dest: dest,
                                type_args: vec![],
                            });
                        }
                        // BATCH-296: a value the compiler could not type (the
                        // element of a `dict[str, Any]`, a dyn parameter). The
                        // old `array_len` fallback read the PRECEDING GC block
                        // and answered 0 — `codes=0` in the local backtest while
                        // `_build_stock_arrays` really returned 91 entries. Let
                        // the runtime tell map / vec / text apart by geometry.
                        None | Some(Type::I64) | Some(Type::PyDynamic) => {
                            self.stmts.push(MirStmt::Call {
                                func: "zeta_dyn_len".to_string(),
                                args: vec![arg_id],
                                dest: dest,
                                type_args: vec![],
                            });
                        }
                        _ => {
                            self.stmts.push(MirStmt::Call {
                                func: "array_len".to_string(),
                                args: vec![arg_id],
                                dest: dest,
                                type_args: vec![],
                            });
                        }
                    }
                    self.exprs.insert(dest, MirExpr::Var(dest));
                    self.type_map.insert(dest, Type::I64);
                    return Some(dest);
                
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_by_static_type() {
        assert_eq!(
            classify_len(Some(&Type::Array(Box::new(Type::I64), ArraySize::Literal(4)))),
            LenRoute::KnownLength(4)
        );
        assert_eq!(classify_len(Some(&Type::Str)), LenRoute::Str);
        assert_eq!(
            classify_len(Some(&Type::Named("map".into(), vec![]))),
            LenRoute::Map
        );
        assert_eq!(
            classify_len(Some(&Type::Named("dict".into(), vec![]))),
            LenRoute::Map
        );
        assert_eq!(
            classify_len(Some(&Type::Named("PyJson".into(), vec![]))),
            LenRoute::PyJson
        );
        assert_eq!(
            classify_len(Some(&Type::DynamicArray(Box::new(Type::Str)))),
            LenRoute::Vec
        );
    }

    #[test]
    fn unknown_types_take_geometric_fallback() {
        assert_eq!(classify_len(Some(&Type::I64)), LenRoute::DynLen);
        assert_eq!(classify_len(Some(&Type::PyDynamic)), LenRoute::DynLen);
        assert_eq!(classify_len(None), LenRoute::DynLen);
    }
}
