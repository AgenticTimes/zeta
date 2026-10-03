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
        None
    }
}
