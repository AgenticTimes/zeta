//! 批次 837：构造器族执行文件——DataFrame kwarg ctor 与 Counter。
//! 原臂逐字迁入（833 教训：带副作用的臂迁移必须逐字＋探针即测）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// PY-A: `pd.DataFrame(columns=[...])` 等 kwarg-only schema 构造——
    /// generic 路径把值列表当 data，len(df) SEGV（实测）。
    pub(super) fn lower_dataframe_kwarg(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
                if method == "DataFrame"
                    && !args.is_empty()
                    && args.iter().all(|a| {
                        matches!(a, AstNode::Call { method: km, args: ka, .. }
                            if km == "__kwarg__" && ka.len() == 2)
                    })
                {
                    let is_pd = match receiver.as_deref() {
                        Some(AstNode::Var(v)) => {
                            self.py_module_aliases.get(v).map_or(false, |m| m == "pandas")
                        }
                        None => self
                            .py_member_target(&None, "DataFrame")
                            .map_or(false, |(m, mem)| m == "pandas" && mem == "DataFrame"),
                        _ => false,
                    };
                    if is_pd {
                        let mut columns: Option<AstNode> = None;
                        for a in args {
                            if let AstNode::Call { method: km, args: ka, .. } = a {
                                if km == "__kwarg__" && ka.len() == 2 {
                                    if let AstNode::StringLit(n) = &ka[0] {
                                        if n == "columns" {
                                            columns = Some(ka[1].clone());
                                        }
                                    }
                                }
                            }
                        }
                        // A LITERAL column list is built as a dict literal of
                        // `name: []` in MIR: a literal list lowers to a
                        // StackArray (no `[cap|len]` header), so the runtime
                        // helper below would read its length as 0 and produce an
                        // EMPTY frame (measured: `len(df.columns)` was 0, not 2).
                        // Non-literal lists (`list(fields)`, `df.columns`) are real
                        // DynamicArrays and go through the helper.
                        let data = match columns {
                            Some(AstNode::ArrayLit(items)) => AstNode::DictLit {
                                entries: items
                                    .iter()
                                    .map(|it| {
                                        (
                                            it.clone(),
                                            AstNode::DynamicArrayLit {
                                                elem_type: "str".to_string(),
                                                elements: vec![],
                                            },
                                        )
                                    })
                                    .collect(),
                            },
                            Some(expr) => AstNode::Call {
                                receiver: None,
                                method: "zeta_df_with_columns".to_string(),
                                args: vec![expr],
                                type_args: vec![],
                                structural: false,
                            },
                            None => AstNode::DictLit { entries: vec![] },
                        };
                        // Re-enter the ORDINARY path with a positional data
                        // argument: it is the one that resolves the ctor symbol
                        // (`pandas__DataFrame`) AND types the result
                        // (`Named("DataFrame")`, which the later `.columns` /
                        // `len()` dispatch needs). Emitting a bare free call here
                        // left the result I64, so `e.columns` became a struct
                        // field read + `array_len` → 0 (measured).
                        return Some(self.lower_expr(&AstNode::Call {
                            receiver: receiver.clone(),
                            method: "DataFrame".to_string(),
                            args: vec![data],
                            type_args: type_args.clone(),
                            structural: false,
                        });
                    }
                }
        None
    }

    /// PY-A: `Counter(<str list>)` 必须按内容哈希键（同 dict 字面量）——
    /// 指针键 shim 会把每个字面量点位分开计数。
    pub(super) fn lower_counter(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
                if method == "Counter" && args.len() == 1 {
                    let is_counter = match receiver {
                        None => self
                            .py_member_aliases
                            .get("Counter")
                            .map(|(m, mem)| m == "collections" && mem == "Counter")
                            .unwrap_or(false),
                        Some(_) => self
                            .py_member_target(receiver, method)
                            .map(|(m, mem)| m == "collections" && mem == "Counter")
                            .unwrap_or(false),
                    };
                    if is_counter {
                        let arg_id = self.lower_expr(&args[0]);
                        let elem_is_str = match self.type_map.get(&arg_id) {
                            Some(Type::DynamicArray(e)) | Some(Type::Array(e, _)) => {
                                matches!(**e, Type::Str)
                            }
                            _ => false,
                        };
                        if elem_is_str {
                            self.stmts.push(MirStmt::Call {
                                func: "py_collections_counter_new_str".to_string(),
                                args: vec![arg_id],
                                dest: dest,
                                type_args: vec![],
                            });
                            self.exprs.insert(dest, MirExpr::Var(id));
                            // String-keyed Counter → keys() is Vec<str>.
                            self.type_map.insert(
                                dest,
                                Type::Named("map".to_string(), vec![Type::Str]),
                            );
                            return Some(dest);
                        }
                    }
                }
        None
    }

    /// 执行者入口：命中返回 Some(dest)；否则 None 落链。
    pub(super) fn lower_ctor(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
        self.lower_dataframe_kwarg(receiver, args, dest)
            .or_else(|| self.lower_counter(receiver, args, dest))
    }
}
