//! 批次 900：getattr face 家族（自 call_dispatch 顺序保持抽取——
//! 每 face 一个 Option<u32> 函数：Some＝接住、None＝贯穿下一臂；
//! 委托在原位转发，派发顺序严格不变。Batch 405 ghost 守卫与 range 共享，
//! 留在 call_dispatch 原位不属本族。）

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::r#gen::TypeDecl;
use crate::middle::mir::mir::MirStmt;
use crate::middle::types::Type;

impl MirGen {
    /// 字面量名 getattr：registry handle／struct 字段改写；未命中贯穿。
    pub(super) fn lower_getattr_literal(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &String,
        args: &Vec<AstNode>,
        id: u32,
    ) -> Option<u32> {
            if method == "getattr" && receiver.is_none() && (2..=3).contains(&args.len()) {
                if let AstNode::StringLit(name) = &args[1] {
                    // (a) a registry handle with a registered member.
                    if let Some(tag) = self.py_handle_of(&args[0]) {
                        if crate::middle::pylib::method_symbol(&tag, name).is_some() {
                            let rewritten = AstNode::FieldAccess {
                                base: Box::new(args[0].clone()),
                                field: name.clone(),
                            };
                            return Some(self.lower_expr(&rewritten));
                        }
                    }
                    // (b) a struct-typed receiver (JoinQuant's `g`, a
                    // module global, a config object): the field exists ->
                    // plain field access; the field is absent but a default
                    // was given -> the default, which IS Python's semantics
                    // for a missing attribute. Absent with no default keeps
                    // the loud diagnostic below (Python would raise
                    // AttributeError, we must not silently read 0).
                    if let Some(tyname) = self.py_struct_type_of(&args[0]) {
                        if self.py_struct_has_field(&tyname, name) {
                            let rewritten = AstNode::FieldAccess {
                                base: Box::new(args[0].clone()),
                                field: name.clone(),
                            };
                            return Some(self.lower_expr(&rewritten));
                        }
                        if args.len() == 3 {
                            return Some(self.lower_expr(&args[2]));
                        }
                    }
                }
            }
        None
    }
    /// struct/动态名/default getattr：改写字段或 default，动态名响亮失败；
    /// 字面量未命中贯穿（批次 123 红线，守 t225）。
    pub(super) fn lower_getattr_struct(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &String,
        args: &Vec<AstNode>,
        id: u32,
    ) -> Option<u32> {
            if receiver.is_none() && method == "getattr" && (args.len() == 2 || args.len() == 3)
            {
                if let AstNode::StringLit(lit) = &args[1] {
                    let obj_id = self.lower_expr(&args[0]);
                    // 类名：接收者的 Named 类型，或构造调用的方法名
                    let class_of = |ty: Option<&Type>, node: &AstNode| -> Option<String> {
                        if let Some(Type::Named(n, _)) = ty {
                            if n != "map" && n != "dict" {
                                return Some(n.clone());
                            }
                        }
                        if let AstNode::Call {
                            receiver: None,
                            method: m,
                            ..
                        } = node
                        {
                            return Some(m.clone());
                        }
                        None
                    };
                    let obj_ty = self.type_map.get(&obj_id).cloned();
                    let cls = class_of(obj_ty.as_ref(), &args[0]);
                    match cls {
                        Some(tn) => {
                            let field_exists = self.type_decls.get(&tn).and_then(|d| match d {
                                TypeDecl::Struct { fields, .. } => fields
                                    .iter()
                                    .find(|(fname, _)| fname.as_str() == lit.as_str())
                                    .map(|(_, ft)| Type::from_string(ft)),
                                _ => None,
                            });
                            if field_exists.is_some() || args.len() == 2 {
                                let fa = AstNode::FieldAccess {
                                    base: Box::new(args[0].clone()),
                                    field: lit.clone(),
                                };
                                return Some(self.lower_expr(&fa));
                            }
                            // 已知 struct 但字段不存在 → default
                            if args.len() == 3 {
                                return Some(self.lower_expr(&args[2]));
                            }
                        }
                        None => {
                            if args.len() == 3 {
                                eprintln!(
                                    "warning: PY-A: getattr on untyped receiver uses the default for '{}'",
                                    lit
                                );
                                return Some(self.lower_expr(&args[2]));
                            }
                        }
                    }
                    // 字面量名未命中（已知 struct 缺字段 / 未跟踪接收者）
                    // 且无 default → 故意保持幽灵路径（批次 123 红线，守 t225：
                    // 链接期未定义符号即编译失败，不静默读 0）。
                    // 不 return，落出本块即可。
                } else {
                    // 动态名（非字面量）→ 响亮失败
                    let obj_id = self.lower_expr(&args[0]);
                    let name_id = self.lower_expr(&args[1]);
                    self.emit_call_into(id, "py_getattr_dynamic", vec![obj_id, name_id], Type::I64);
                    return Some(id);
                }
            }
        None
    }
}
