// src/backend/codegen/signature_table.rs
// 批 990（提案①第一段）：C 运行时签名表＋keyfn 指针合同。
//
// 背景：批 963–989 的四类事故——kf 体 LLVM 签名 i64(i64) 与 C 桥
// double(*)(double) 的寄存器类错配（x0/v0 读残留，批 983 lldb 定位）、
// mangled 名漂移（`{nm}__keyf64` vs `__ZKEYF64_{nm}`）、登记不改名覆盖
// 原函数 MIR（批 965）——共同根因是签名合同分散在 AST 注解字符串 /
// MIR 槽型 / LLVM 签名 / C 函数指针四层，没有任何一环核对两端一致。
// 本模块给核对一个单一事实来源：
//   * `lookup`：C 运行时函数的 ABI 签名（表项逐条对勘 runtime/*.c，
//     来源行号写在分支注释；扩表随批，无来源不入表）；
//   * `KEYFN_PTR_CONTRACT`：C 桥对 keyfn 函数指针的硬编码预期
//     double(*)(double)——codegen 在特化副本实体化时按它核对，不符即
//     W0911 响亮失败（宁可响亮失败原则：错签名产出静默垃圾更恶劣）。
// 指针与整数同槽（i64 位宽），故表内不设指针类别。

use crate::middle::types::Type;

/// ABI 层值类别。f32 与 f64 分列——宽度不同就是不同的寄存器类，
/// 合同核对时 f32 不得冒充 f64。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValTy {
    I64,
    F64,
    F32,
}

/// 一个 C 符号的 ABI 签名：返回类别＋按位的参数类别。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sig {
    pub ret: ValTy,
    pub params: &'static [ValTy],
}

/// C 运行时函数签名表（首批＝keyfn 族）。表项与 runtime/py_additions.c
/// 的函数定义逐条对勘；表外名字返回 None，调用方维持全 i64 兜底。
pub fn lookup(name: &str) -> Option<Sig> {
    match name {
        // runtime/py_additions.c:1111 — int64_t py_min_key(vec, keyfn, key_is_f64)
        "py_min_key" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64, ValTy::I64, ValTy::I64] }),
        // runtime/py_additions.c:1217 — int64_t py_max_key(vec, keyfn, key_is_f64)
        "py_max_key" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64, ValTy::I64, ValTy::I64] }),
        // runtime/py_additions.c:1203 — int64_t py_max_key_f64(vec, keyfn_addr)
        "py_max_key_f64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64, ValTy::I64] }),
        // runtime/py_additions.c:1207 — int64_t py_min_key_f64(vec, keyfn_addr)
        "py_min_key_f64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64, ValTy::I64] }),
        // runtime/py_additions.c:1218 — int64_t py_max_key_i64_f64(vec, keyfn_addr)
        "py_max_key_i64_f64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64, ValTy::I64] }),
        // runtime/py_additions.c:1233 — int64_t py_min_key_i64_f64(vec, keyfn_addr)
        "py_min_key_i64_f64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64, ValTy::I64] }),
        // 批 991 扩表（对勘 runtime/py_additions.c 与 runtime/tokio_runtime_stub.c）：
        // runtime/py_additions.c:3324 / :3334 — int64_t py_builtin_max/min(vec)
        "py_builtin_max" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64] }),
        "py_builtin_min" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64] }),
        // runtime/py_additions.c:3350 / :3441 — f64 结果按位模式落 i64 槽
        "py_builtin_max_f64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64] }),
        "py_builtin_min_f64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64] }),
        // runtime/py_additions.c:3372 / :3391 / :3410 / :3425 — key=abs 特化族
        "py_builtin_max_abs_f64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64] }),
        "py_builtin_min_abs_f64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64] }),
        "py_builtin_max_abs_i64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64] }),
        "py_builtin_min_abs_i64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64] }),
        // runtime/py_additions.c:3004 — int64_t zeta_pow_i64(base, exp)
        "zeta_pow_i64" => Some(Sig { ret: ValTy::I64, params: &[ValTy::I64, ValTy::I64] }),
        // runtime/tokio_runtime_stub.c:1025 — double py_math_pow(a, b)
        //（真正的 F64 ABI：实参错送整数寄存器即读残留——表驱动的价值样本）
        "py_math_pow" => Some(Sig { ret: ValTy::F64, params: &[ValTy::F64, ValTy::F64] }),
        // 批 997：curated 未命中落注册表生成段（--emit-sigtable，299 项）
        _ => super::signature_table_gen::lookup_gen(name),
    }
}

/// C 桥对 keyfn 函数指针的合同：double(*)(double)。特化副本（参数注解
/// f64）的 LLVM 签名必须逐位对上——批 967/974/975/989 实证 int64 形参
/// 直传读 x0 残留、f32 宽度不符同错。
pub const KEYFN_PTR_CONTRACT: Sig = Sig {
    ret: ValTy::F64,
    params: &[ValTy::F64],
};

/// keyfn 特化副本的名字前缀。铸造点唯一：call_dispatch key= 臂
/// （src/middle/mir/gen/call_dispatch.rs 登记块，批 989 后为唯一铸造处）。
pub use crate::middle::mir::r#gen::keyfn_bridge::SPEC_PREFIX as KEYFN_SPEC_PREFIX;
// （字面量唯一铸造点在 middle 层 keyfn_bridge.rs；此处 re-export，
//  漂移即编译错误。）

/// 实际签名是否符合预期（类别逐一相等，参数个数含在切片比较内）。
pub fn sig_matches(ret: ValTy, params: &[ValTy], expected: Sig) -> bool {
    ret == expected.ret && params == expected.params
}

/// MIR 槽型 → ABI 值类别。None（无槽型）与一切非浮点槽都归 I64——
/// 指针、句柄、i64 槽上的 f64 位模式同槽；f32/f64 按宽度分列。
pub fn val_ty_of(ty: Option<&Type>) -> ValTy {
    match ty {
        Some(Type::F64) => ValTy::F64,
        Some(Type::F32) => ValTy::F32,
        _ => ValTy::I64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyfn_bridges_registered_with_i64_abi() {
        for n in [
            "py_max_key_f64",
            "py_min_key_f64",
            "py_max_key_i64_f64",
            "py_min_key_i64_f64",
        ] {
            let s = lookup(n).unwrap_or_else(|| panic!("{} 应在签名表", n));
            assert_eq!(s.ret, ValTy::I64, "{} 返回类别", n);
            assert_eq!(s.params, &[ValTy::I64, ValTy::I64], "{} 参数", n);
        }
    }

    #[test]
    fn old_path_key_variants_take_flag() {
        for n in ["py_max_key", "py_min_key"] {
            let s = lookup(n).unwrap_or_else(|| panic!("{} 应在签名表", n));
            assert_eq!(s.params.len(), 3, "{} 带 key_is_f64 旗标位", n);
        }
    }

    #[test]
    fn batch991_expansion_entries() {
        // 单参 min/max 内建族（f64 结果按位模式落 i64 槽）
        for n in [
            "py_builtin_max",
            "py_builtin_min",
            "py_builtin_max_f64",
            "py_builtin_min_f64",
            "py_builtin_max_abs_f64",
            "py_builtin_min_abs_f64",
            "py_builtin_max_abs_i64",
            "py_builtin_min_abs_i64",
        ] {
            let s = lookup(n).unwrap_or_else(|| panic!("{} 应在签名表", n));
            assert_eq!(
                s,
                Sig { ret: ValTy::I64, params: &[ValTy::I64] },
                "{} 单参 i64 ABI",
                n
            );
        }
        assert_eq!(
            lookup("zeta_pow_i64"),
            Some(Sig { ret: ValTy::I64, params: &[ValTy::I64, ValTy::I64] })
        );
        // py_math_pow：全表首个真 F64 ABI 条目——实参/返回都是 double
        assert_eq!(
            lookup("py_math_pow"),
            Some(Sig { ret: ValTy::F64, params: &[ValTy::F64, ValTy::F64] })
        );
    }

    #[test]
    fn contract_accepts_only_double_double() {
        assert!(sig_matches(ValTy::F64, &[ValTy::F64], KEYFN_PTR_CONTRACT));
        // 批 983 错配形态：体 i64(i64) 对 C 桥 double(*)(double)
        assert!(!sig_matches(ValTy::I64, &[ValTy::I64], KEYFN_PTR_CONTRACT));
        // 返回域错：i64 形参＋double 返回（混合签名同样不可过桥）
        assert!(!sig_matches(ValTy::F64, &[ValTy::I64], KEYFN_PTR_CONTRACT));
        // 宽度错：f32 冒充 f64
        assert!(!sig_matches(ValTy::F32, &[ValTy::F64], KEYFN_PTR_CONTRACT));
        // 形参个数错
        assert!(!sig_matches(ValTy::F64, &[], KEYFN_PTR_CONTRACT));
    }

    #[test]
    fn unknown_names_absent() {
        assert!(lookup("py_print").is_none());
        assert!(lookup("").is_none());
        assert!(lookup("__ZKEYF64_kf").is_none());
    }

    #[test]
    fn mir_slot_ty_maps_to_val_ty() {
        assert_eq!(val_ty_of(Some(&Type::F64)), ValTy::F64);
        assert_eq!(val_ty_of(Some(&Type::F32)), ValTy::F32);
        assert_eq!(val_ty_of(Some(&Type::I64)), ValTy::I64);
        // 指针/句柄/字符串同槽
        assert_eq!(val_ty_of(Some(&Type::Str)), ValTy::I64);
        assert_eq!(val_ty_of(None), ValTy::I64);
    }
}
