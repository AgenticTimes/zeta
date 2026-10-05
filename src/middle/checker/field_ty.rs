//! 批次 952（轴 F.4.1 第一刀）：`self.<field> = <rhs>` 字段定型纯函数。
//! 逻辑自 parser（top_level.rs 的 RHS 形状猜测块）逐字迁入 checker 归属；
//! parser 调用点已替换。行为零变——各臂语义见函数内原注释。

use crate::frontend::ast::AstNode;

/// 由 RHS 形状推断字段类型串。`init_params` 为 `__init__` 参数表
/// （名, 注解）——`self.x = x` 时取参数注解（非空非 dyn）。
pub fn guess_field_type_from_rhs(
    rhs_eff: &AstNode,
    init_params: &[(String, String)],
) -> String {
    match rhs_eff {
        AstNode::Lit(_) => "i64".to_string(),
        AstNode::Bool(_) => "bool".to_string(),
        // `self.x = {}` — a dict literal field. Without this arm
        // the field typed i64, so EVERY map operation on it
        // (`self.m.get(k, d)`, `.values()`, `.keys()`, `k in
        // self.m`) fell through to an opaque bare symbol
        // (`_get` / `_values` / `_exists` — 7 reference sites
        // each in the REasyQuant local backtest, e.g.
        // `PositionLedger._positions` / `._today_buys`).
        AstNode::DictLit { .. } => "map".to_string(),
        AstNode::FloatLit(_) => "f64".to_string(),
        AstNode::StringLit(_) => "str".to_string(),
        AstNode::ArrayLit(items) | AstNode::DynamicArrayLit { elements: items, .. } => {
            // Batch 594: an ELEMENT-AWARE spelling. The bare
            // "DynamicArray" left every list field i64-typed
            // at the read sites (`print(p.ages)` rendered the
            // handle; `q = p.ages; q[0]` dispatched map_get
            // and crashed). `list[T]` is the spelling the
            // read side already parses (lt_annotation_type,
            // batch 291). Element type from the first item's
            // literal shape; anything else conservatively i64.
            let elem = items.first().map(|e| match e {
                AstNode::StringLit(_) => "str",
                AstNode::FloatLit(_) => "f64",
                AstNode::Bool(_) => "bool",
                _ => "i64",
            }).unwrap_or("i64");
            // `list<…>` (angle form) is the dialect
            // `lt_annotation_type` — the read side —
            // parses; the `[…]` subscript form belongs to
            // the annotation parser and is NOT read here.
            format!("list<{}>", elem)
        }
        AstNode::Var(name)
            if init_params.iter().any(|(n, _)| n == name) =>
        {
            // `self.x = x` — take the PARAMETER's declared
            // type when it has one. Hardcoding i64 ignored
            // `def __init__(self, d: map)`: every library
            // field became i64 and its own
            // `self.data.keys()` turned into an undefined
            // `_keys`.
            // B3: unannotated params are `"dyn"`; treat that
            // like the old empty/i64 default so constructor
            // call-site field refinement (`dt == "i64"`) still
            // upgrades `self.name = s` when `s` is a string.
            init_params
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, ty)| ty.clone())
                .filter(|ty| !ty.is_empty() && ty != "dyn")
                .unwrap_or_else(|| "i64".to_string())
        }
        // `self._ledger = PositionLedger(...)` — a field
        // holding an instance of a user class. With the old
        // i64 default the inner calls (`self._ledger
        // .clear_today_buys()`) fell to the bare-name
        // dispatch, and two classes sharing the method name
        // made codegen emit a self-recursive duplicate —
        // infinite recursion (batch 294). Capitalized
        // callee ⇒ remember the class name; MIR gen types
        // the field `Named(cls)` and dispatches qualified.
        AstNode::Call {
            receiver: None,
            method,
            ..
        } if method.chars().next().map_or(false, |c| c.is_uppercase()) => {
            method.clone()
        }
        // A field whose initializer is a CALL keeps the
        // callee's declared result type when the registry
        // knows it (`self.cache_dir = os.path.join(...)` is a
        // str). Falling back to i64 typed every such field as
        // an integer: `len(f.cache_dir)` was 0 and
        // `str(f.cache_dir)` printed the handle as digits.
        AstNode::Call {
            receiver, method, ..
        } if receiver.is_some() => {
            let mut ty = "i64".to_string();
            if let Some(recv) = receiver {
                let mut parts: Vec<String> = Vec::new();
                let mut cur: &AstNode = recv;
                loop {
                    match cur {
                        AstNode::FieldAccess { base, field } => {
                            parts.push(field.clone());
                            cur = base;
                        }
                        AstNode::Var(root) => {
                            parts.push(root.clone());
                            break;
                        }
                        _ => break,
                    }
                }
                parts.reverse();
                // The receiver's dotted path IS the module
                // (`os.path`), the method is the member.
                let module = parts.join(".");
                let fallback = module
                    .strip_prefix(parts[0].as_str())
                    .and_then(|rest| rest.strip_prefix('.'))
                    .map(|rest| rest.to_string());
                if let Some(e) =
                    crate::middle::pylib::find_member(&module, method)
                        .or_else(|| {
                            fallback.as_deref().and_then(|rest| {
                                crate::middle::pylib::find_member(
                                    parts[0].as_str(),
                                    rest,
                                )
                            })
                        })
                {
                    if e.ret == "str" {
                        ty = "str".to_string();
                    }
                }
            }
            ty
        }
        // BATCH-298: `self._cash = float(initial_cash)` — the
        // builtin conversions have a known result type. Left at
        // i64 the field is arithmetic-typed as an integer, but
        // an 8-byte slot holds a double's BIT PATTERN (see
        // `StructFieldStore`), so `self._cash -= total` became
        // `sub i64` on those bits: the ledger's cash never
        // moved and the local backtest valued the portfolio at
        // 0 (`final_value = portfolio.available_cash`).
        AstNode::Call {
            receiver: None,
            method,
            ..
        } if matches!(
            method.as_str(),
            "float" | "int" | "str" | "bool"
        ) =>
        {
            match method.as_str() {
                "float" => "f64",
                "str" => "str",
                "bool" => "bool",
                _ => "i64",
            }
            .to_string()
        }
        _ => "i64".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, ty: &str) -> (String, String) {
        (name.to_string(), ty.to_string())
    }

    /// 字面量形状 ⇒ 基础型（批 952 迁出回归锁）。
    #[test]
    fn literal_shapes() {
        assert_eq!(
            guess_field_type_from_rhs(&AstNode::Lit(7), &[]),
            "i64"
        );
        assert_eq!(
            guess_field_type_from_rhs(&AstNode::FloatLit("1.5".into()), &[]),
            "f64"
        );
        assert_eq!(
            guess_field_type_from_rhs(&AstNode::StringLit("s".into()), &[]),
            "str"
        );
        assert_eq!(
            guess_field_type_from_rhs(&AstNode::Bool(true), &[]),
            "bool"
        );
        assert_eq!(
            guess_field_type_from_rhs(&AstNode::DictLit { entries: vec![] }, &[]),
            "map"
        );
    }

    /// 列表字面量 ⇒ 元素感知 list<elem>（batch 594 语义）。
    #[test]
    fn array_lit_element_aware() {
        let strs = AstNode::ArrayLit(vec![AstNode::StringLit("x".into())]);
        assert_eq!(
            guess_field_type_from_rhs(&strs, &[]),
            "list<str>"
        );
        let mixed = AstNode::ArrayLit(vec![AstNode::Lit(1), AstNode::Lit(2)]);
        assert_eq!(
            guess_field_type_from_rhs(&mixed, &[]),
            "list<i64>"
        );
    }

    /// self.x = x ⇒ 取参数注解（非空非 dyn）；无注解 ⇒ i64 缺省（B3 语义）。
    #[test]
    fn var_takes_param_annotation() {
        let params = vec![p("d", "map"), p("s", "dyn")];
        assert_eq!(
            guess_field_type_from_rhs(&AstNode::Var("d".into()), &params),
            "map"
        );
        assert_eq!(
            guess_field_type_from_rhs(&AstNode::Var("s".into()), &params),
            "i64",
            "dyn 注解视同缺省"
        );
        assert_eq!(
            guess_field_type_from_rhs(&AstNode::Var("q".into()), &params),
            "i64"
        );
    }

    /// 大写无接收者调用 ⇒ 类名（batch 294：字段持用户类实例）。
    #[test]
    fn capitalized_ctor_yields_class_name() {
        let ctor = AstNode::Call {
            receiver: None,
            method: "PositionLedger".to_string(),
            args: vec![],
            type_args: vec![],
            structural: false,
        };
        assert_eq!(
            guess_field_type_from_rhs(&ctor, &[]),
            "PositionLedger"
        );
    }

    /// 内建转换 ⇒ 已知结果型（BATCH-298：位模式算术）。
    #[test]
    fn builtin_conversions() {
        let conv = |m: &str| AstNode::Call {
            receiver: None,
            method: m.to_string(),
            args: vec![AstNode::Var("x".into())],
            type_args: vec![],
            structural: false,
        };
        assert_eq!(guess_field_type_from_rhs(&conv("float"), &[]), "f64");
        assert_eq!(guess_field_type_from_rhs(&conv("str"), &[]), "str");
        assert_eq!(guess_field_type_from_rhs(&conv("int"), &[]), "i64");
    }

    /// 未知形态 ⇒ i64 兜底。
    #[test]
    fn unknown_falls_back_to_i64() {
        assert_eq!(
            guess_field_type_from_rhs(&AstNode::Ignore, &[]),
            "i64"
        );
    }
}
