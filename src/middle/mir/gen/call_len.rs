//! 批次 819：`len()` 内建的**路由分类**（rustc 模式：判定与发射分离）。
//! `classify_len` 是纯函数——给定编译期实参类型，判定该走哪条发射路径；
//! 单元测试钉住每条路由的判定合同（gen.rs 的 len 臂消费本分类结果）。

use crate::middle::types::{ArraySize, Type};


/// `len(x)` 的发射路由。
#[derive(Debug, PartialEq, Eq)]
pub enum LenRoute {
    /// 编译期已知长度的定长数组：直接折叠成字面量（携带 n）。
    KnownLength(usize),
    /// 字符串：str_len（字节长）。
    Str,
    /// dict/Counter：zeta_map_len（数已用槽）。
    Map,
    /// PyJson：按 JSON tag 分派（py_json_len）。
    PyJson,
    /// 动态数组：vec_len。
    Vec,
    /// 其余（未知型/I64/PyDynamic）：运行期按几何形判（zeta_dyn_len，
    /// map/vec/文本三态）——767 值标签大弧的几何兜底。
    DynLen,
}

/// 纯函数：实参静态类型 → 发射路由。无副作用，可毫秒级单测。
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
        // I64 槽（字典值兜底）、PyDynamic（异构安全）、完全未知——三条都落
        // 运行期几何形判；这是"值对类型丢"防线上的兜底路由，不许误入
        // Vec/Str 专属路（历史实测：array_len 读前一块头答 0，map 句柄读表头）。
        assert_eq!(classify_len(Some(&Type::I64)), LenRoute::DynLen);
        assert_eq!(classify_len(Some(&Type::PyDynamic)), LenRoute::DynLen);
        assert_eq!(classify_len(None), LenRoute::DynLen);
    }

    #[test]
    fn map_and_dict_spellings_both_route_to_map() {
        // 815 的等价规则：dict/map 两种拼写同一语义，路由不得分叉。
        assert_eq!(
            classify_len(Some(&Type::Named("dict".into(), vec![]))),
            classify_len(Some(&Type::Named("map".into(), vec![])))
        );
    }
}
