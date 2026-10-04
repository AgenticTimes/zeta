//! 批次 911（轴 F 方案② P1 骨架）：类型格 LatticeTy ＋ meet。
//!
//! 三值格（设计稿 docs/f2-checker-design.md §2）：
//! ```text
//!     ⊥ Conflict            ← 同槽两种已知型（不静默选边，890 变体 A 教训）
//!           │ meet
//!     Known(Type)           ← 具体型（I64/F64/Str/…）
//!           │ meet
//!     ⊤ Unknown             ← 未知（运行期 PyDynamic＋形状分派）
//! ```

use crate::middle::types::Type;

/// 格值：Unknown＝未知（⊥上方），Known＝具体型，Conflict＝两种已知型相撞。
#[derive(Debug, Clone, PartialEq)]
pub enum LatticeTy {
    Unknown,
    Known(Type),
    Conflict,
}

impl LatticeTy {
    pub fn unknown() -> Self {
        LatticeTy::Unknown
    }

    pub fn known(ty: Type) -> Self {
        LatticeTy::Known(ty)
    }

    /// meet：单调合并（格的二元最小上界）。
    /// Unknown⊕x＝x；Known(a)⊕Known(a)＝a；Known(a)⊕Known(b≠a)＝Conflict。
    pub fn meet(&self, other: &LatticeTy) -> LatticeTy {
        use LatticeTy::*;
        match (self, other) {
            (Unknown, x) => x.clone(),
            (x, Unknown) => x.clone(),
            (Known(a), Known(b)) => {
                if a == b {
                    Known(a.clone())
                } else {
                    Conflict
                }
            }
            (Conflict, _) | (_, Conflict) => Conflict,
        }
    }

    /// 是否为已知型（供查表方区分"未知"与"冲突"）。
    pub fn is_known(&self) -> bool {
        matches!(self, LatticeTy::Known(_))
    }

    /// 已知型提取（Unknown/Conflict 返回 None）。
    pub fn known_ty(&self) -> Option<Type> {
        match self {
            LatticeTy::Known(t) => Some(t.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(ty: Type) -> LatticeTy {
        LatticeTy::Known(ty)
    }

    /// 格六律：单位元／交换律／结合律的核心三式＋冲突。
    #[test]
    fn meet_unknown_is_identity() {
        let t = k(Type::I64);
        assert_eq!(LatticeTy::Unknown.meet(&t), t);
        assert_eq!(t.meet(&LatticeTy::Unknown), t);
    }

    #[test]
    fn meet_same_known_is_stable() {
        let t = k(Type::I64);
        assert_eq!(t.meet(&t), t);
    }

    #[test]
    fn meet_conflicting_knowns_is_conflict() {
        let a = k(Type::I64);
        let b = k(Type::Str);
        assert_eq!(a.meet(&b), LatticeTy::Conflict);
        assert_eq!(b.meet(&a), LatticeTy::Conflict);
    }

    #[test]
    fn meet_conflict_is_absorbing() {
        let c = LatticeTy::Conflict;
        assert_eq!(c.meet(&k(Type::I64)), LatticeTy::Conflict);
        assert_eq!(c.meet(&LatticeTy::Unknown), LatticeTy::Conflict);
    }

    #[test]
    fn known_ty_extracts_known_only() {
        assert_eq!(k(Type::F64).known_ty(), Some(Type::F64));
        assert_eq!(LatticeTy::Unknown.known_ty(), None);
        assert_eq!(LatticeTy::Conflict.known_ty(), None);
    }

    #[test]
    fn meet_associativity_core_case() {
        // (g ⊔ bump) ⊔ h ＝ g ⊔ (b ⊔ h) 的核心三槽案例（#275 回归面）
        let g = k(Type::I64);
        let call = LatticeTy::Unknown;
        let h = k(Type::I64);
        let left = call.meet(&h).meet(&g);
        let right = call.meet(&g.meet(&h));
        assert_eq!(left, right);
    }
}
