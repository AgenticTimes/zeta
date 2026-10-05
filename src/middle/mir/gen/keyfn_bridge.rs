// src/middle/mir/gen/keyfn_bridge.rs
// 批 992（提案③核心）：key= 臂的声明式分派。
//
// 背景：批 963–991 的链上事故里，"这次 min/max(key=…) 该走哪条桥"
// 的判定曾以 if 链形式内联在 call_dispatch 的 key= 臂——判定与发射
// 混在一起，任何改动都要整段重读（批 978–988 的叠块残骸即产物）。
// 本模块把判定收进两个纯函数：`spec_worthy`（要不要铸造 f64 通道
// 特化副本）与 `choose_bridge`（发射走哪条桥、结果槽是什么类别），
// 全矩阵单测锁定；臂里只剩"取证据 → 查裁决 → 按裁决发射"。
// C 侧桥的 ABI 事实来源见 backend/codegen/signature_table.rs（批 990）。

use crate::middle::types::Type;

/// keyfn 特化副本的名字前缀。本模块是唯一铸造点（`spec_name`）；
/// C 桥合同侧（backend/codegen/signature_table.rs）经 re-export 引用
/// 同一常量，漂移在编译期不可能。
pub const SPEC_PREFIX: &str = "__ZKEYF64_";

/// 铸造特化副本名：`kf` ⇒ `__ZKEYF64_kf`。原名入 mir_map 会覆盖原
/// 函数的 MIR（批 965 实证），登记前必须改名。
pub fn spec_name(orig: &str) -> String {
    format!("{}{}", SPEC_PREFIX, orig)
}

/// 分桥裁决。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum KeyBridge {
    /// key=abs 特化：内建 abs 无一等函数值形式（llvm.fabs 内在经
    /// zeta_call1 ABI 不匹配，实拍 exit 138），C 侧 fabs 比较循环直接
    /// 分派，绕开函数值 ABI（批 956）。
    AbsBuiltin { func: &'static str, ret_f64: bool },
    /// 特化副本桥：FuncAddr(__ZKEYF64_*) ＋ C 桥。f64 元素走位桥
    /// （元素位模式 bitcast 成 double）；i64 元素走 sitofp 桥（值转换，
    /// Python 语义 kf(3)=1.5 要求整→浮转换，位重解整数 3 得非规格数，
    /// 比较全错）。返回原元素。
    Specialized { func: &'static str, ret_f64: bool },
    /// 旧路：py_min_key/py_max_key ＋ key_is_f64 旗标（批 962）。
    Legacy,
}

/// 特化登记门槛：哪些（元素型 × keyfn 返回域）值得铸造 f64 通道副本。
/// - f64/f32 元素 ⇒ 总是（元素本就是 f64 位模式，位桥语义，批 967）；
/// - i64 元素 ⇒ 仅 float 返回 keyfn——旧路 double(*)(int64_t) 调用签名
///   与 float 返回 kf 体 i64(i64) 签名寄存器类错配（体写 x0、C 读 v0，
///   批 983 lldb 定位）；int 返回 keyfn 旧路 i64 比较本来精确
///   （>2^53 才失真），不劫持（批 989 裁定）；
/// - 元素型未知/其他（str 句柄、嵌套容器…）⇒ 否，落旧路。
pub(crate) fn spec_worthy(elem: Option<&Type>, keyfn_ret_f64: bool) -> bool {
    match elem {
        Some(t) if matches!(t, Type::F64 | Type::F32) => true,
        Some(Type::I64) => keyfn_ret_f64,
        _ => false,
    }
}

/// 分桥裁决。`registered` 指特化副本已在（或刚被本次调用登记进）共享
/// 存储；特化路要求 `registered && spec_worthy(elem, keyfn_ret_f64)`——
/// 单 registered 不够：同一 keyfn 在 i64 数组点登记后，另一个元素型
/// 未知/非数值的调用点必须继续走旧路（sitofp 桥对字符串句柄是垃圾）。
pub(crate) fn choose_bridge(
    method: &str,
    keyfn_is_abs: bool,
    elem: Option<&Type>,
    keyfn_ret_f64: bool,
    registered: bool,
) -> KeyBridge {
    let elem_f64 = matches!(elem, Some(t) if matches!(t, Type::F64 | Type::F32));
    if keyfn_is_abs {
        let func = match (method, elem_f64) {
            ("min", true) => "py_builtin_min_abs_f64",
            ("max", true) => "py_builtin_max_abs_f64",
            ("max", false) => "py_builtin_max_abs_i64",
            _ => "py_builtin_min_abs_i64",
        };
        return KeyBridge::AbsBuiltin { func, ret_f64: elem_f64 };
    }
    if registered && spec_worthy(elem, keyfn_ret_f64) {
        let func = match (method, elem_f64) {
            ("min", true) => "py_min_key_f64",
            ("max", true) => "py_max_key_f64",
            ("min", false) => "py_min_key_i64_f64",
            _ => "py_max_key_i64_f64",
        };
        return KeyBridge::Specialized { func, ret_f64: elem_f64 };
    }
    KeyBridge::Legacy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(ty: Type) -> Option<Type> {
        Some(ty)
    }

    #[test]
    fn spec_name_mints_canonical_form() {
        assert_eq!(spec_name("kf"), "__ZKEYF64_kf");
        assert_eq!(spec_name("abs"), "__ZKEYF64_abs");
        // 前缀常量与铸造函数同源（消费方 codegen 按 contains 匹配）
        assert!(spec_name("kf").starts_with(SPEC_PREFIX));
    }

    #[test]
    fn spec_worthy_matrix() {
        // f64/f32 元素：总是
        assert!(spec_worthy(t(Type::F64).as_ref(), false));
        assert!(spec_worthy(t(Type::F64).as_ref(), true));
        assert!(spec_worthy(t(Type::F32).as_ref(), false));
        // i64 元素：仅 float 返回 keyfn（批 989 裁定）
        assert!(spec_worthy(t(Type::I64).as_ref(), true));
        assert!(!spec_worthy(t(Type::I64).as_ref(), false));
        // 其他/未知：否
        assert!(!spec_worthy(t(Type::Str).as_ref(), true));
        assert!(!spec_worthy(t(Type::Bool).as_ref(), true));
        assert!(!spec_worthy(
            t(Type::DynamicArray(Box::new(Type::I64))).as_ref(),
            true
        ));
        assert!(!spec_worthy(None, true));
    }

    #[test]
    fn abs_arm_dispatch() {
        let cases = [
            ("min", true, "py_builtin_min_abs_f64"),
            ("max", true, "py_builtin_max_abs_f64"),
            ("max", false, "py_builtin_max_abs_i64"),
            ("min", false, "py_builtin_min_abs_i64"),
        ];
        for (method, float, want) in cases {
            let elem = if float { t(Type::F64) } else { t(Type::I64) };
            match choose_bridge(method, true, elem.as_ref(), false, false) {
                KeyBridge::AbsBuiltin { func, ret_f64 } => {
                    assert_eq!(func, want);
                    assert_eq!(ret_f64, float);
                }
                other => panic!("abs 臂应得 AbsBuiltin，得 {:?}", other),
            }
        }
    }

    #[test]
    fn specialized_dispatch_by_element() {
        // f64 元素 ⇒ 位桥
        match choose_bridge("min", false, t(Type::F64).as_ref(), true, true) {
            KeyBridge::Specialized { func, ret_f64 } => {
                assert_eq!(func, "py_min_key_f64");
                assert!(ret_f64);
            }
            other => panic!("得 {:?}", other),
        }
        match choose_bridge("max", false, t(Type::F32).as_ref(), false, true) {
            KeyBridge::Specialized { func, .. } => assert_eq!(func, "py_max_key_f64"),
            other => panic!("得 {:?}", other),
        }
        // i64 元素 ⇒ sitofp 桥（仅 float 返回 keyfn 会登记到这）
        match choose_bridge("min", false, t(Type::I64).as_ref(), true, true) {
            KeyBridge::Specialized { func, ret_f64 } => {
                assert_eq!(func, "py_min_key_i64_f64");
                assert!(!ret_f64, "i64 元素返回原元素，结果槽 I64");
            }
            other => panic!("得 {:?}", other),
        }
        match choose_bridge("max", false, t(Type::I64).as_ref(), true, true) {
            KeyBridge::Specialized { func, .. } => {
                assert_eq!(func, "py_max_key_i64_f64")
            }
            other => panic!("得 {:?}", other),
        }
    }

    #[test]
    fn specialized_requires_worthy_element_not_just_registration() {
        // 同一 keyfn 在 i64 点登记后，元素型未知/非数值的点必须继续旧路
        assert_eq!(
            choose_bridge("max", false, None, true, true),
            KeyBridge::Legacy
        );
        assert_eq!(
            choose_bridge("max", false, t(Type::Str).as_ref(), true, true),
            KeyBridge::Legacy
        );
        // int 返回 keyfn：即便已登记（不可能路径的防御）也不走特化
        assert_eq!(
            choose_bridge("max", false, t(Type::I64).as_ref(), false, true),
            KeyBridge::Legacy
        );
        // 常规未登记
        assert_eq!(
            choose_bridge("max", false, t(Type::F64).as_ref(), true, false),
            KeyBridge::Legacy
        );
    }
}
