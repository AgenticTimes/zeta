//! 批次 825：json 序列化族的**纯分类面**（dumps/dump 两处共用同一套
//! 判定——此前是两份手写 match，改一处漏一处的温床）。发射留在 gen.rs。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

/// 序列化路由：符号 ＋（vec 族才有的）元素型标签。
#[derive(Debug, PartialEq, Eq)]
pub struct JsonRoute {
    pub sym: &'static str,
    pub vec_elem_tag: Option<i64>,
}

/// 纯函数：静态类型 → 序列化符号与元素标签。
/// 合同（单测钉住）：标量各归其道；容器走 typed vec（元素型决定标签，
/// 非 f64/str/bool 元素落 0＝运行期按值判）；PyJson 递归自 dump；
/// 未知落 i64（值对类型丢风险位，钉住）。
pub fn json_route(ty: &Type) -> JsonRoute {
    let sym = match ty {
        Type::Str => "py_json_dumps_str",
        Type::F64 => "py_json_dumps_f64",
        Type::Bool => "py_json_dumps_bool",
        t if t.is_map() => "py_json_dumps_map",
        Type::Named(n, _) if n == "PyJson" => "py_json_dump",
        Type::DynamicArray(_) | Type::Array(_, _) => "py_json_dumps_vec_typed",
        _ => "py_json_dumps_i64",
    };
    let vec_elem_tag = match ty {
        Type::DynamicArray(e) | Type::Array(e, _) => Some(match **e {
            Type::F64 | Type::F32 => 1,
            Type::Str => 2,
            Type::Bool => 3,
            _ => 0,
        }),
        _ => None,
    };
    JsonRoute { sym, vec_elem_tag }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_routes() {
        assert_eq!(json_route(&Type::Str).sym, "py_json_dumps_str");
        assert_eq!(json_route(&Type::F64).sym, "py_json_dumps_f64");
        assert_eq!(json_route(&Type::Bool).sym, "py_json_dumps_bool");
        assert_eq!(json_route(&Type::I64).sym, "py_json_dumps_i64");
        // 兜底钉：PyDynamic（未知）也落 i64——与 len 分类同款风险位合同
        assert_eq!(json_route(&Type::PyDynamic).sym, "py_json_dumps_i64");
    }

    #[test]
    fn containers_route_with_elem_tag() {
        let r = json_route(&Type::DynamicArray(Box::new(Type::Str)));
        assert_eq!(r.sym, "py_json_dumps_vec_typed");
        assert_eq!(r.vec_elem_tag, Some(2));
        let r = json_route(&Type::Array(
            Box::new(Type::F64),
            crate::middle::types::ArraySize::Literal(2),
        ));
        assert_eq!(r.vec_elem_tag, Some(1));
        // 混型元素落 0（运行期按值判）
        let r = json_route(&Type::DynamicArray(Box::new(Type::Bool)));
        assert_eq!(r.vec_elem_tag, Some(3));
    }

    #[test]
    fn map_and_json_special() {
        // 815 等价规则：dict 拼写同路
        assert_eq!(json_route(&Type::Named("map".into(), vec![])).sym, "py_json_dumps_map");
        assert_eq!(json_route(&Type::Named("dict".into(), vec![])).sym, "py_json_dumps_map");
        assert_eq!(json_route(&Type::Named("PyJson".into(), vec![])).sym, "py_json_dump");
        // map/json 无元素标签（tag 机制只服务 vec）
        assert_eq!(json_route(&Type::Named("map".into(), vec![])).vec_elem_tag, None);
    }
}

impl MirGen {
    /// 批次 833 重做：json.dumps/dump 执行者——原臂体**逐字**搬入
    /// （821/833 教训：带副作用的臂迁移必须逐字＋探针即测＋先算后写）。
    /// 返回 Some(id)＝命中并发射；None＝不归本族。
    pub(super) fn lower_json(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
        args: &[AstNode],
        id: u32,
    ) -> Option<u32> {
        if let Some((m, mem)) = self.py_member_target(receiver, method) {
            let first_is_positional = !matches!(
                args.first(),
                Some(AstNode::Call { method: km, .. }) if km == "__kwarg__"
            );
            if m == "json" && mem == "dumps" && !args.is_empty() && first_is_positional {
                if args.len() > 1 {
                    static WARNED_DUMPS: std::sync::OnceLock<()> = std::sync::OnceLock::new();
                    WARNED_DUMPS.get_or_init(|| {
                        eprintln!(
                            "warning: PY-A: json.dumps formatting kwargs \
                             (ensure_ascii/indent/…) are ignored"
                        );
                    });
                }
                let arg_id = self.lower_expr(&args[0]);
                let ty = self.type_map.get(&arg_id).cloned().unwrap_or(Type::slot_fallback());
                let route = json_route(&ty);
                if let Some(tag) = route.vec_elem_tag {
                    let tag_id = self.next_id();
                    self.exprs.insert(tag_id, MirExpr::IntLit(tag));
                    self.type_map.insert(tag_id, Type::I64);
                    self.emit_call_into(id, "py_json_dumps_vec_typed", vec![arg_id, tag_id], Type::Str);
                    return Some(id);
                }
                self.stmts.push(MirStmt::Call {
                    func: route.sym.to_string(),
                    args: vec![arg_id],
                    dest: id,
                    type_args: vec![],
                });
                self.exprs.insert(id, MirExpr::Var(id));
                self.type_map.insert(id, Type::Str);
                return Some(id);
            }
            if m == "json" && mem == "dump" && args.len() == 2 {
                let obj_id = self.lower_expr(&args[0]);
                let oty = self.type_map.get(&obj_id).cloned().unwrap_or(Type::slot_fallback());
                let route = json_route(&oty);
                let text_id = self.next_id();
                let mut cargs = vec![obj_id];
                if let Some(tag) = route.vec_elem_tag {
                    let tid = self.next_id();
                    self.exprs.insert(tid, MirExpr::IntLit(tag));
                    self.type_map.insert(tid, Type::I64);
                    cargs.push(tid);
                }
                self.stmts.push(MirStmt::Call {
                    func: route.sym.to_string(),
                    args: cargs,
                    dest: text_id,
                    type_args: vec![],
                });
                self.exprs.insert(text_id, MirExpr::Var(text_id));
                self.type_map.insert(text_id, Type::Str);
                let file_id = self.lower_expr(&args[1]);
                self.emit_call_into(id, "py_file_write", vec![file_id, text_id], Type::I64);
                return Some(id);
            }
        }
        None
    }
}
