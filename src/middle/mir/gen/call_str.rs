//! 批次 818：字符串方法族的符号表（gen.rs 拆家族第二刀）。
//! 表驱动先例（str_method_symbol）从 19k 行主文件迁出，随附表驱动
//! 真值表单测——gen.rs 里第一批可毫秒级单测的纯函数。

use crate::middle::types::Type;

pub(super) fn path_ends_with_mem(path: &[String]) -> bool {
    path.last().map(String::as_str) == Some("mem")
        && (path.len() == 1 || (path.len() == 2 && path[0].as_str() == "std"))
}

/// `std::mem::size_of` is written both fully qualified and after a `use`
/// (`mem::size_of::<f32>()` in `zeta_src/runtime/tensor.z:37`).
pub(super) fn str_method_symbol(method: &str) -> Option<(&'static str, usize, &'static str)> {    match method {
        "upper" => Some(("host_str_to_uppercase", 1, "str")),
        // `.to_string()` on a string is identity (strings are immutable here);
        // the same-named C alias `to_string` covers call sites whose receiver
        // type is unknown and bypasses this table (batch 363, task #42).
        "to_string" => Some(("host_str_to_string", 1, "str")),
        // Batch 364 (task #42): the string predicate family. `is_whitespace`
        // follows Rust's all-chars rule; `clone` is identity for the same
        // immutability reason as `to_string`. Bare C aliases for the untyped
        // fallback path are in tokio_runtime_stub.c.
        "is_empty" => Some(("host_str_is_empty", 1, "bool")),
        "is_whitespace" => Some(("host_str_is_whitespace", 1, "bool")),
        "clone" => Some(("host_str_clone", 1, "str")),
        // push_str 返回新串（纯函数）；不进表的话结果被定型 I64，
        // 后续 .len() 派发就错了（批次 365 实测）。
        "push_str" => Some(("host_str_push_str", 2, "str")),
        // chars 返回单字符字符串的 vec（"split" = vec<str> 返回种类）。
        "chars" => Some(("host_str_chars", 1, "split")),
        "lower" => Some(("host_str_to_lowercase", 1, "str")),
        "capitalize" => Some(("host_str_capitalize", 1, "str")),
        "title" => Some(("host_str_title", 1, "str")),
        "swapcase" => Some(("host_str_swapcase", 1, "str")),
        "trim" | "strip" => Some(("host_str_trim", 1, "str")),
        "lstrip" => Some(("host_str_lstrip", 1, "str")),
        "rstrip" => Some(("host_str_rstrip", 1, "str")),
        "contains" => Some(("host_str_contains", 2, "bool")),
        "startswith" | "starts_with" => Some(("host_str_starts_with", 2, "bool")),
        "endswith" | "ends_with" => Some(("host_str_ends_with", 2, "bool")),
        "replace" => Some(("host_str_replace", 3, "str")),
        // `s.repeat(n)` is the Rust spelling of `"ab" * 3`: same symbol, and
        // without this row the fall-through emitted a bare `_repeat` extern.
        "repeat" => Some(("host_str_repeat", 2, "str")),
        "find" | "index" => Some(("host_str_find", 2, "i64")),
        "rfind" => Some(("host_str_rfind", 2, "i64")),
        "count" => Some(("host_str_count", 2, "i64")),
        "len" => Some(("host_str_len", 1, "i64")),
        "split" => Some(("host_str_split", 2, "split")),
        // Batch 588: newline-only sibling of split (the C side walks \n,
        // \r\n, \r itself — CPython's exotic Unicode separators are a
        // registered corner).
        "splitlines" => Some(("host_str_splitlines", 1, "split")),
        // Batch 588: the 2-argument user form `s.rsplit(sep, maxsplit)`;
        // `rsplit(sep)` is routed to host_str_split at the call site
        // (identical semantics when maxsplit is absent).
        "rsplit" => Some(("host_str_rsplit", 3, "split")),
        "join" => Some(("host_str_join", 2, "str")),
        "zfill" => Some(("host_str_zfill", 2, "str")),
        "ljust" => Some(("host_str_ljust", 3, "str")),
        "rjust" => Some(("host_str_rjust", 3, "str")),
        "isalpha" => Some(("host_str_isalpha", 1, "bool")),
        // 批次 794（#266）：Rust 拼写的判定族成员。selfhost.z 在 `input[i]`
        // （str_get 结果，静态类型 Str）上调 `.is_alphabetic()/.is_alphanumeric()`，
        // 表里没有这两行时走通用派发发裸 `is_alphabetic_1`，-o 链接失败；
        // is_alphanumeric 虽有 C 裸别名兜链接，返回值定型 I64、打印不出 True。
        "is_alphabetic" => Some(("host_str_isalpha", 1, "bool")),
        "is_alphanumeric" => Some(("host_str_isalnum", 1, "bool")),
        // 批次 794（#266）：`word.as_str()` 恒等（串即句柄，与 clone 同理）——
        // 没有这行时定型 I64，print 打句柄数字（改前是链接失败）。
        "as_str" => Some(("host_str_clone", 1, "str")),
        "isdigit" => Some(("host_str_isdigit", 1, "bool")),
        "isupper" => Some(("host_str_isupper", 1, "bool")),
        "islower" => Some(("host_str_islower", 1, "bool")),
        // The rest of the str.is* family. Without a table entry each name
        // linked against the same-named libc ctype function (a different
        // signature entirely) and silently returned 0.
        "isalnum" => Some(("host_str_isalnum", 1, "bool")),
        "isspace" => Some(("host_str_isspace", 1, "bool")),
        "isnumeric" => Some(("host_str_isnumeric", 1, "bool")),
        "isdecimal" => Some(("host_str_isdecimal", 1, "bool")),
        "isascii" => Some(("host_str_isascii", 1, "bool")),
        "isprintable" => Some(("host_str_isprintable", 1, "bool")),
        "istitle" => Some(("host_str_istitle", 1, "bool")),
        "removeprefix" => Some(("host_str_removeprefix", 2, "str")),
        "removesuffix" => Some(("host_str_removesuffix", 2, "str")),
        _ => None,
    }
}

/// Batch 588: the start-offset overloads — consulted only when a call site
/// carries a third argument (`s.find(sub, start)`). `index` keeps find's
/// -1-on-miss semantics instead of raising (registered corner).
pub(super) fn str_method_symbol3(method: &str) -> Option<(&'static str, usize, &'static str)> {
    match method {
        "find" | "index" => Some(("host_str_find3", 3, "i64")),
        _ => None,
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_methods_resolve_to_symbols() {
        let (f, arity, ret) = str_method_symbol("upper").expect("upper 应在表");
        assert_eq!((f, arity, ret), ("host_str_to_uppercase", 1, "str"));
        assert_eq!(str_method_symbol("to_string").unwrap().0, "host_str_to_string");
        let (f, _, r) = str_method_symbol("is_empty").expect("is_empty 应在表");
        assert_eq!((f, r), ("host_str_is_empty", "bool"));
    }

    #[test]
    fn unknown_methods_abstain() {
        // 表不认识的成员返回 None（调用方落兜底/响亮失败）——
        // 保证"注册表赢"的路由顺序不被本表越权。
        assert!(str_method_symbol("no_such_method").is_none());
        assert!(str_method_symbol3("no_such_method").is_none());
    }

    #[test]
    fn three_arg_variant_only_for_find_family() {
        assert_eq!(str_method_symbol3("find").unwrap().0, "host_str_find3");
        assert_eq!(str_method_symbol3("index").unwrap().0, "host_str_find3");
        assert!(str_method_symbol3("upper").is_none()); // 单参表不越界
    }

    #[test]
    fn all_symbols_are_str_prefixed_or_known_alias() {
        // 合同：表内符号必须是宿主字符串族（host_str_*）——防止拼错方法名
        // 静默落到别的运行期家族（批次 784 的 clip 选错同形）。
        for m in ["upper", "to_string", "is_empty", "find", "lower", "trim",
                  "split", "contains", "replace", "startswith", "ends_with"] {
            if let Some((f, _, _)) = str_method_symbol(m) {
                assert!(f.starts_with("host_str_"), "{m} → {f} 不是字符串族符号");
            }
        }
    }
}

/// 批次 822：to_string 通道的**纯判定面**——给定静态类型，返回该走的
/// 转换函数名；None = 无需转换（值已经是字符串形态：Str/PyPath）。
/// 元组特例（需要 arity+逐位标签）不在本表，由调用方单走。
/// 单测钉住每条通道（str(d)/str(t) 曾把句柄当整数打印指针，批次 742/743）。
pub(super) fn to_string_channel(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::Str => None,
        Type::Named(n, _) if n == "PyPath" => None,
        Type::F64 | Type::F32 => Some("to_string_f64"),
        Type::Bool => Some("to_string_bool"),
        t if t.is_map() => Some("py_json_dumps_map"),
        Type::DynamicArray(_) | Type::Array(_, _) => Some("py_json_dumps_vec"),
        Type::Tuple(_) => None, // 元组需 arity+标签特算，调用方单走
        _ => Some("to_string_i64"),
    }
}

#[cfg(test)]
mod to_string_tests {
    use super::*;
    use crate::middle::types::ArraySize;

    #[test]
    fn identity_faces_return_none() {
        assert_eq!(to_string_channel(&Type::Str), None);
        assert_eq!(
            to_string_channel(&Type::Named("PyPath".into(), vec![])),
            None
        );
        // 元组特例由调用方单走
        assert_eq!(
            to_string_channel(&Type::Tuple(vec![Type::Str, Type::I64])),
            None
        );
    }

    #[test]
    fn scalar_and_container_channels() {
        assert_eq!(
            to_string_channel(&Type::F64),
            Some("to_string_f64")
        );
        assert_eq!(to_string_channel(&Type::Bool), Some("to_string_bool"));
        assert_eq!(
            to_string_channel(&Type::Named("map".into(), vec![])),
            Some("py_json_dumps_map")
        );
        // 815 等价规则：dict 拼写同路
        assert_eq!(
            to_string_channel(&Type::Named("dict".into(), vec![])),
            Some("py_json_dumps_map")
        );
        assert_eq!(
            to_string_channel(&Type::DynamicArray(Box::new(Type::I64))),
            Some("py_json_dumps_vec")
        );
        assert_eq!(
            to_string_channel(&Type::Array(
                Box::new(Type::Str),
                ArraySize::Literal(3)
            )),
            Some("py_json_dumps_vec")
        );
    }

    #[test]
    fn unknown_falls_to_i64_channel() {
        // I64 槽兜底：编译器不知道类型时按整数转——这是"值对类型丢"
        // 风险位，本测试钉住该兜底存在且不被误改。
        assert_eq!(to_string_channel(&Type::I64), Some("to_string_i64"));
        assert_eq!(
            to_string_channel(&Type::PyDynamic),
            Some("to_string_i64")
        );
    }
}
