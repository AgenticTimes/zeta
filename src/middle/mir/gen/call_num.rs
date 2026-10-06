//! 批次 830：数值内建族执行文件（abs/sum 先行；min/max/successor 下批）。
//! 入口判定在 gen.rs（classify_call → NumericBuiltin），本文件只管发射。

use super::MirGen;
use super::call_dispatch::keyfn_returns_f64;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;
use crate::middle::types::ArraySize;

impl MirGen {
    /// PY-A abs：f64 走 llvm.fabs.f64 内在函数（i64 签名会强转浮点位模式
    /// 出垃圾，实测批注在案）；i64 走 zeta_abs_i64。
    fn lower_abs(&mut self, arg0_node: &AstNode, dest: u32) -> Option<u32> {
        let arg_id = self.lower_expr(arg0_node);
        let f64_arg = matches!(
            self.type_map.get(&arg_id),
            Some(Type::F64) | Some(Type::F32)
        );
        let func = if f64_arg { "llvm.fabs.f64" } else { "zeta_abs_i64" };
        self.stmts.push(MirStmt::Call {
            func: func.to_string(),
            args: vec![arg_id],
            dest,
            type_args: vec![],
        });
        self.exprs.insert(dest, MirExpr::Var(dest));
        self.type_map.insert(
            dest,
            if f64_arg { Type::F64 } else { Type::I64 },
        );
        Some(dest)
    }

    /// PY-A sum：DynamicArray 走 zeta_sum_vec；PyDynamic/未知实参在运行期
    /// 是动态 vec（批次 145：此前落 zeta_sum_n 静态路径打印 0，t224f）；
    /// 定长数组折叠 zeta_sum_n（长度编译期已知）。
    /// 批次 924：f64 元素走 f64 累加版（此前 i64 位模式累加产出垃圾和，
    /// sum([1.5,2.5]) 实拍 9222246136947933184）；元素型 type_map 优先、
    /// checker_env 兜底（与 mean 臂同模式的 P3 消费点）。
    /// 批次 10063：标量槽（I64/Str/…）改走动态 vec 路——见 sum_seq_of 的注释。
    fn lower_sum(&mut self, arg0_node: &AstNode, dest: u32) -> Option<u32> {
        let arg_id = self.lower_expr(arg0_node);
        let (seq, elem_f64) = match sum_seq_of(self.type_map.get(&arg_id)) {
            Some(pair) => pair,
            None => (
                SumSeq::Dynamic,
                matches!(
                    arg0_node,
                    AstNode::Var(nm) if matches!(
                        self.checker_type_of(nm),
                        Some(Type::DynamicArray(e)) if matches!(*e, Type::F64)
                    )
                ),
            ),
        };
        let (func, dest_f64, n_arg) = sum_target(elem_f64, seq);
        let mut call_args = vec![arg_id];
        if let Some(n) = n_arg {
            call_args.push(self.int_slot(n));
        }
        self.stmts.push(MirStmt::Call {
            func: func.to_string(),
            args: call_args,
            dest,
            type_args: vec![],
        });
        self.exprs.insert(dest, MirExpr::Var(dest));
        self.type_map.insert(
            dest,
            if dest_f64 { Type::F64 } else { Type::I64 },
        );
        Some(dest)
    }

    /// 执行者入口（gen.rs 的 NumericBuiltin 臂消费）：abs/sum 命中返回
    /// Some(dest)；其余 None 落链（min/max/successor 下批迁入）。
    pub(super) fn lower_numeric_builtin(
        &mut self,
        method: &str,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
        if method == "abs" && args.len() == 1 {
            return self.lower_abs(&args[0], dest);
        }
        if method == "sum" && args.len() == 1 {
            return self.lower_sum(&args[0], dest);
        }
        if method == "successor" {
            return Some(self.lower_successor(args, dest));
        }
        if method == "predecessor" {
            return Some(self.lower_predecessor(args, dest));
        }
        if (method == "min" || method == "max") && !args.is_empty() {
            return Some(self.lower_minmax(method, args, dest));
        }
        None
    }

    /// SPECIAL: successor(it)——迭代器推进 1（it+1）。
    fn lower_successor(&mut self, args: &[AstNode], dest: u32) -> u32 {
        let it_id = self.lower_expr(&args[0]);
        let one_id = self.next_id();
        self.exprs.insert(one_id, MirExpr::IntLit(1));
        self.type_map.insert(one_id, Type::I64);
        self.exprs.insert(
            dest,
            MirExpr::BinaryOp {
                op: "+".to_string(),
                left: it_id,
                right: one_id,
            },
        );
        self.type_map.insert(dest, Type::I64);
        dest
    }

    /// SPECIAL: predecessor(it)——迭代器回退 1（it-1）。
    fn lower_predecessor(&mut self, args: &[AstNode], dest: u32) -> u32 {
        let it_id = self.lower_expr(&args[0]);
        let one_id = self.next_id();
        self.exprs.insert(one_id, MirExpr::IntLit(1));
        self.type_map.insert(one_id, Type::I64);
        self.exprs.insert(
            dest,
            MirExpr::BinaryOp {
                op: "-".to_string(),
                left: it_id,
                right: one_id,
            },
        );
        self.type_map.insert(dest, Type::I64);
        dest
    }

    /// PY-A min/max：key= 选可迭代形式（py_min_key/py_max_key）；N 参两两折叠
    /// （批次 146：此前仅 2 参，3 参落裸名链接失败 t230）；f64 走
    /// llvm.minnum/maxnum 内在；全 Bool 结果 Bool（批次 564）。
    fn lower_minmax(&mut self, method: &str, args: &[AstNode], dest: u32) -> u32 {
        // key= keyword selects the ITERABLE form; handle it before the
        // min-of-two path treats the callable as a value (which returned
        // the function pointer as the result).
        if let AstNode::Call {
            receiver: None,
            method: m,
            args: ka,
            ..
        } = &args[1]
        {
            if m == "__kwarg__"
                && ka.len() == 2
                && matches!(&ka[0], AstNode::StringLit(n) if n == "key")
            {
                // 批 982 补：登记块——NumericBuiltin 入口先于 call_dispatch
                // 的 key= 臂执行，store 恒空导致特化发射永不触发（min/max
                // 落旧路 flag=0）
                // 批 984 补：float 注解 keyfn＋非 f64 元素 ⇒ 响亮告警
                //（keyfn 返回域 float 而元素通道 i64 ⇒ 比较可能错序——
                // 宁可响亮失败原则，静默错值更恶劣）
                if let AstNode::Var(kn) = &ka[1] {
                    let kf_ret_f64 = matches!(
                        self.func_ret_types.get(kn.as_str()),
                        Some(Type::F64) | Some(Type::F32)
                    );
                    let xs_lower = self.lower_expr(&args[0]);
                    let elem_f64 = matches!(
                        self.type_map.get(&xs_lower),
                        Some(Type::DynamicArray(e))
                            if matches!(**e, Type::F64 | Type::F32)
                    );
                    if kf_ret_f64 && !elem_f64 {
                        crate::diag_warning!(
                            "W0901",
                            "min/max `key=` function `{}` returns float but the iterable elements are not floats — ordering may be wrong; annotate the iterable as a float list",
                            kn
                        );
                    }
                }
                // 批 989：本臂旧的登记/发射块整段拆除（978–988 迭代残骸）。
                // 旧登记块克隆后**不改名**（push 原名 FuncDef——批 965 的老
                // 缺陷形态），mangled 永不命中 ⇒ 发射块死路；发射块又不分
                // 元素型（i64 元素会误走位桥）。探针实证 max(xs,key=kf) 的
                // 路由在 call_dispatch 的 key= 臂（登记/发射已全量接通，
                // 见 call_dispatch.rs 批 989 段），本臂落旧路 py_min/
                // py_max_key 即基线行为。W0901 告警保留。
                if std::env::var("ZETA_PROBE_CHECKER").is_ok() {
                    eprintln!(
                        "REG989: arm-entered ka1={:?} args_len={}",
                        ka[1],
                        args.len()
                    );
                }
                let xs = self.lower_expr(&args[0]);
                let f = self.lower_expr(&ka[1]);
                let func = if method == "min" { "py_min_key" } else { "py_max_key" };
                // 批 962：keyfn 返回域分派
                let flag = self.int_slot(
                    keyfn_returns_f64(&self.func_ret_types, &ka[1]) as i64,
                );
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: vec![xs, f, flag],
                    dest,
                    type_args: vec![],
                });
                self.exprs.insert(dest, MirExpr::Var(dest));
                self.type_map.insert(dest, Type::I64);
                return dest;
            }
        }
        // 批次 146: N 参 min/max 两两折叠
        let ids: Vec<u32> = args.iter().map(|a| self.lower_expr(a)).collect();
        let any_f = ids.iter().any(|i| {
            matches!(self.type_map.get(i), Some(Type::F64) | Some(Type::F32))
        });
        let stem = if method == "min" { "zeta_min" } else { "zeta_max" };
        let mut acc = ids[0];
        for (k, &arg) in ids.iter().enumerate().skip(1) {
            let last = k + 1 == ids.len();
            let d = if last { dest } else { self.next_id() };
            if any_f {
                let intr = format!(
                    "llvm.{}.f64",
                    if method == "min" { "minnum" } else { "maxnum" }
                );
                self.stmts.push(MirStmt::Call {
                    func: intr,
                    args: vec![acc, arg],
                    dest: d,
                    type_args: vec![],
                });
                self.exprs.insert(d, MirExpr::Var(d));
                self.type_map.insert(d, Type::F64);
            } else {
                self.stmts.push(MirStmt::Call {
                    func: format!("{}_{}", stem, "i64"),
                    args: vec![acc, arg],
                    dest: d,
                    type_args: vec![],
                });
                self.exprs.insert(d, MirExpr::Var(d));
                // Batch 564: all-Bool operands yield Bool; a Bool mixed with
                // an int keeps I64 (the int result would misrender).
                let both_bool = matches!(self.type_map.get(&acc), Some(Type::Bool))
                    && matches!(self.type_map.get(&arg), Some(Type::Bool));
                self.type_map.insert(d, if both_bool { Type::Bool } else { Type::I64 });
            }
            acc = d;
        }
        dest
    }
}

/// sum 实参序列形状（批 924 抽纯面）：Dynamic=动态 vec（读 header）；
/// Static(n)=定长栈数组（长度编译期已知，传 n）；LegacyBare=旧路裸调
/// zeta_sum_n 不带长度参数（Slice 等，历史上如此，保持原样不动）。
#[derive(Debug, Clone, Copy, PartialEq)]
enum SumSeq {
    Dynamic,
    Static(i64),
    LegacyBare,
}

/// sum 降级方案纯面（批 924）：元素是否 f64＋序列形状 ⇒（函数名, 结果槽
/// 是否 f64, 长度参数）。语义合同：f64 元素按位模式存取（zeta_vec_push_f64
/// 同一约定），必须走 f64 累加版——此前 i64 位模式累加产出垃圾和
/// （sum([1.5,2.5]) 实拍 9222246136947933184，CPython 4.0）。
fn sum_target(elem_f64: bool, seq: SumSeq) -> (&'static str, bool, Option<i64>) {
    match (elem_f64, seq) {
        (true, SumSeq::Static(n)) => ("zeta_sum_n_f64", true, Some(n)),
        (false, SumSeq::Static(n)) => ("zeta_sum_n", false, Some(n)),
        (true, SumSeq::Dynamic) => ("zeta_sum_vec_f64", true, None),
        (false, SumSeq::Dynamic) => ("zeta_sum_vec", false, None),
        (_, SumSeq::LegacyBare) => ("zeta_sum_n", false, None),
    }
}

/// sum 实参形状判定（批次 10063 从 lower_sum 抽出，同批 924 抽纯面的做法）：
/// 类型槽 ⇒（序列形状, 元素是否 f64）。返回 `None` = 本表判不了，交调用点走
/// checker 兜底（PyDynamic 与"槽里没有型"两种）。
/// 批次 10063 改的是最后的兜底支：此前落 LegacyBare ⇒ 发射点只发 1 个实参，
/// 而在册签名 `zeta_sum_n args=i64,i64`（pylib/registry.txt:513）要 2 个，
/// 长度参数取第二个实参寄存器的残值、从数据地址按残值长度逐格读
/// （D2 夹具 `sum(生成器)` 实拍挂死）。兜底支改指 1 参版 `zeta_sum_vec`
/// （registry.txt:488；运行期 `if (!data) return 0;` 自带空值保护）后，
/// 发出的参数个数与在册签名对齐。
/// Type::Array 的非字面量长度仍走 LegacyBare：那是栈上数组、没有动态 vec 的
/// header，按 header 读长度比按残值读更错，本批未动该形状。
fn sum_seq_of(t: Option<&Type>) -> Option<(SumSeq, bool)> {
    match t {
        Some(Type::DynamicArray(e)) => Some((SumSeq::Dynamic, matches!(**e, Type::F64))),
        Some(Type::Array(e, ArraySize::Literal(n))) => {
            Some((SumSeq::Static(*n as i64), matches!(**e, Type::F64)))
        }
        Some(Type::Array(e, _)) => Some((SumSeq::LegacyBare, matches!(**e, Type::F64))),
        Some(Type::PyDynamic) | None => None,
        _ => Some((SumSeq::Dynamic, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 浮点动态数组 ⇒ f64 累加版、结果槽 F64（批 924 缺陷主面）。
    #[test]
    fn sum_f64_dynamic_targets_f64_accumulator() {
        assert_eq!(
            sum_target(true, SumSeq::Dynamic),
            ("zeta_sum_vec_f64", true, None)
        );
    }

    /// 整数动态数组保持原路（zeta_sum_vec、I64）。
    #[test]
    fn sum_i64_dynamic_keeps_legacy_path() {
        assert_eq!(
            sum_target(false, SumSeq::Dynamic),
            ("zeta_sum_vec", false, None)
        );
    }

    /// 定长栈数组：f64 版带长度参数；整数版同样带。
    #[test]
    fn sum_static_passes_length() {
        assert_eq!(
            sum_target(true, SumSeq::Static(3)),
            ("zeta_sum_n_f64", true, Some(3))
        );
        assert_eq!(
            sum_target(false, SumSeq::Static(3)),
            ("zeta_sum_n", false, Some(3))
        );
    }

    /// 旧路裸调（Slice 等）不带长度参数，f64 与否都落 zeta_sum_n（保持原样）。
    #[test]
    fn sum_legacy_bare_unchanged() {
        assert_eq!(
            sum_target(true, SumSeq::LegacyBare),
            ("zeta_sum_n", false, None)
        );
        assert_eq!(
            sum_target(false, SumSeq::LegacyBare),
            ("zeta_sum_n", false, None)
        );
    }

    /// 批次 10063 回归钉：标量槽（I64/Str/Bool/F64）兜底走 1 参版
    /// `zeta_sum_vec`。改前实拍＝落 LegacyBare ⇒ 对 2 参签名 `zeta_sum_n
    /// args=i64,i64` 少发一个长度参数，D2 夹具 `sum(生成器)` 挂死。
    #[test]
    fn sum_scalar_slot_targets_one_arg_vector_reader() {
        for t in [&Type::I64, &Type::Str, &Type::Bool, &Type::F64] {
            assert_eq!(
                sum_seq_of(Some(t)),
                Some((SumSeq::Dynamic, false)),
                "标量槽 {t:?} 应走动态 vec 路"
            );
        }
        assert_eq!(
            sum_target(false, SumSeq::Dynamic),
            ("zeta_sum_vec", false, None)
        );
    }

    /// 其余形状逐条不变（动态 vec、定长栈数组、无字面量长度的栈数组、
    /// PyDynamic 与"槽里没型"两种兜底前）。
    #[test]
    fn sum_seq_of_other_shapes_unchanged() {
        assert_eq!(
            sum_seq_of(Some(&Type::DynamicArray(Box::new(Type::I64)))),
            Some((SumSeq::Dynamic, false))
        );
        assert_eq!(
            sum_seq_of(Some(&Type::DynamicArray(Box::new(Type::F64)))),
            Some((SumSeq::Dynamic, true))
        );
        assert_eq!(
            sum_seq_of(Some(&Type::Array(
                Box::new(Type::I64),
                ArraySize::Literal(3)
            ))),
            Some((SumSeq::Static(3), false))
        );
        assert_eq!(
            sum_seq_of(Some(&Type::Array(
                Box::new(Type::F64),
                ArraySize::ConstParam("N".to_string())
            ))),
            Some((SumSeq::LegacyBare, true))
        );
        assert_eq!(sum_seq_of(Some(&Type::PyDynamic)), None);
        assert_eq!(sum_seq_of(None), None);
    }
}
