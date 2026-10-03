//! 批次 830：数值内建族执行文件（abs/sum 先行；min/max/successor 下批）。
//! 入口判定在 gen.rs（classify_call → NumericBuiltin），本文件只管发射。

use super::MirGen;
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
    fn lower_sum(&mut self, arg0_node: &AstNode, dest: u32) -> Option<u32> {
        let arg_id = self.lower_expr(arg0_node);
        let (func, extra) = match self.type_map.get(&arg_id).cloned() {
            Some(Type::DynamicArray(_)) => ("zeta_sum_vec".to_string(), Vec::new()),
            Some(Type::PyDynamic) | None => ("zeta_sum_vec".to_string(), Vec::new()),
            Some(Type::Array(_, ArraySize::Literal(n))) => (
                "zeta_sum_n".to_string(),
                vec![self.int_slot(n as i64)],
            ),
            _ => ("zeta_sum_n".to_string(), Vec::new()),
        };
        let mut call_args = vec![arg_id];
        call_args.extend(extra);
        self.stmts.push(MirStmt::Call {
            func,
            args: call_args,
            dest,
            type_args: vec![],
        });
        self.exprs.insert(dest, MirExpr::Var(dest));
        self.type_map.insert(dest, Type::I64);
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
                let xs = self.lower_expr(&args[0]);
                let f = self.lower_expr(&ka[1]);
                let func = if method == "min" { "py_min_key" } else { "py_max_key" };
                self.stmts.push(MirStmt::Call {
                    func: func.to_string(),
                    args: vec![xs, f],
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
