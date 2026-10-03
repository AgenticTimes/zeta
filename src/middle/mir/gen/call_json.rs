//! 批次 825：json 序列化族的**纯分类面**（dumps/dump 两处共用同一套
//! 判定——此前是两份手写 match，改一处漏一处的温床）。发射留在 gen.rs。

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
