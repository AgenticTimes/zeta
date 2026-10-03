//! 批次 823：Call 臂的**路由分类函数**（rustc 模式骨架——判定与发射分离）。
//!
//! gen.rs 的 Call 臂有 112 个方法名分支，"哪个方法归哪个家族"的规则散在
//! 八千行的先后次序里。本文件把它收成一个纯函数：给定方法名（＋将来可扩
//! 接收者类型/参数个数），返回家族分类。后续家族拆分 = 给枚举加变体＋
//! 在对应执行文件实现；**分类函数是唯一路由决策点**。

use crate::middle::types::Type;

/// Call 臂的家族分类。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CallClass {
    /// 集合变异族（add/discard/remove——写回接收者）。
    SetMutation,
    /// 集合查询族（intersection——返回新句柄）。
    SetIntersection,
    /// 内建 len()。
    Len,
    /// 断言族（assert(cond, msg)——失败即 zeta_assert_fail）。
    Assert,
    /// 内建 min/max/sum/abs/successor/predecessor 数值族。
    NumericBuiltin,
    /// json.dumps/dump 序列化族。
    JsonDump,
    /// re.sub 正则替换族。
    RegularSub,
    /// logging 家族（FileHandler/getLogger）。
    Logging,
    /// print 家族。
    Print,
    /// long tail：逐条特判的方法（getattr/groupby/spawn/…，各有独立语义）。
    Special,
    /// 不认识的成员——走既有兜底（注册表/响亮失败），分类器不越权。
    Unknown,
}

/// 纯函数：方法名 → 家族分类。无副作用。
pub fn classify_call(method: &str) -> CallClass {
    match method {
        "add" | "discard" | "remove" => CallClass::SetMutation,
        "intersection" => CallClass::SetIntersection,
        "len" => CallClass::Len,
        "assert" => CallClass::Assert,
        "min" | "max" | "sum" | "abs" | "successor" | "predecessor" => {
            CallClass::NumericBuiltin
        }
        "dumps" | "dump" => CallClass::JsonDump,
        "sub" => CallClass::RegularSub,
        "FileHandler" | "getLogger" => CallClass::Logging,
        "print" => CallClass::Print,
        _ => CallClass::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_family_split_is_exact() {
        // 821 回归钉（升级到分类器层）：查询/变异两族互斥，绝不互相吸收
        assert_eq!(classify_call("add"), CallClass::SetMutation);
        assert_eq!(classify_call("discard"), CallClass::SetMutation);
        assert_eq!(classify_call("remove"), CallClass::SetMutation);
        assert_eq!(classify_call("intersection"), CallClass::SetIntersection);
    }

    #[test]
    fn builtin_families_route() {
        assert_eq!(classify_call("len"), CallClass::Len);
        for m in ["min", "max", "sum", "abs", "successor", "predecessor"] {
            assert_eq!(classify_call(m), CallClass::NumericBuiltin);
        }
        assert_eq!(classify_call("dumps"), CallClass::JsonDump);
        assert_eq!(classify_call("dump"), CallClass::JsonDump);
        assert_eq!(classify_call("print"), CallClass::Print);
    }

    #[test]
    fn assert_routes_to_assert() {
        assert_eq!(classify_call("assert"), CallClass::Assert);
    }

    #[test]
    fn unknown_stays_unknown() {
        // 分类器不越权：不认识的名字必须 Unknown（兜底与响亮失败归调用方），
        // 猜一个家族＝批次 816 的 intersection 误入变异族同形。
        for m in ["no_such", "upper", "union", "clear", "push", "corr"] {
            assert_eq!(classify_call(m), CallClass::Unknown, "{m} 应 Unknown");
        }
    }
}

/// 批次 843：`type(x)` 的纯判定面——静态类型 → Python 类型名。
/// 无运行期反射（_type extern 不再链接）；未知落 "object"。
pub fn type_name_of(ty: &Type) -> &'static str {
    match ty {
        Type::Str => "str",
        Type::F64 => "float",
        Type::Bool => "bool",
        Type::I64 => "int",
        Type::DynamicArray(_) | Type::Array(_, _) => "list",
        t if t.is_map() => "dict",
        Type::Named(n, _) if n == "PySlice" => "slice",
        _ => "object",
    }
}

#[cfg(test)]
mod type_name_tests {
    use super::*;

    #[test]
    fn python_type_names() {
        assert_eq!(type_name_of(&Type::Str), "str");
        assert_eq!(type_name_of(&Type::F64), "float");
        assert_eq!(type_name_of(&Type::Bool), "bool");
        assert_eq!(type_name_of(&Type::I64), "int");
        assert_eq!(
            type_name_of(&Type::DynamicArray(Box::new(Type::Str))),
            "list"
        );
        assert_eq!(type_name_of(&Type::Named("map".into(), vec![])), "dict");
        assert_eq!(type_name_of(&Type::Named("dict".into(), vec![])), "dict");
        assert_eq!(
            type_name_of(&Type::Named("PySlice".into(), vec![])),
            "slice"
        );
    }

    #[test]
    fn unknown_falls_to_object() {
        // PyDynamic/自定义 Named/PyJson 都落 "object"（CPython 语义：
        // type(未知) 只有 Python 层才知道，静态面给 object 是保守正确）。
        assert_eq!(type_name_of(&Type::PyDynamic), "object");
        assert_eq!(
            type_name_of(&Type::Named("MyStruct".into(), vec![])),
            "object"
        );
        assert_eq!(
            type_name_of(&Type::Named("PyJson".into(), vec![])),
            "object"
        );
    }
}
