//! 批次 835：re.sub 族执行文件——闭包替换走 py_re_sub_call（闭包形参
//! typed as Match），字符串替换走 py_re_sub。分类器归类 RegularSub。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    /// PY-A: `re.sub(pat, repl, s)` — repl may be a STRING or a callable
    /// (`lambda m: ...`). The closure's parameter must be typed as a Match
    /// so `m.group(0)` inside it dispatches. 命中返回 Some(id)。
    pub(super) fn lower_re_sub(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
        args: &[AstNode],
        id: u32,
    ) -> Option<u32> {
        let (m, mem) = self.py_member_target(receiver, method)?;
        if m != "re" || mem != "sub" || args.len() != 3 {
            return None;
        }
        let pat_id = self.lower_expr(&args[0]);
        let callable_repl = matches!(&args[1], AstNode::Closure { .. })
            || matches!(
                &args[1],
                AstNode::Var(n) if self.func_ret_types.contains_key(n.as_str())
            );
        let repl_id = {
            if callable_repl {
                self.re_repl_param = true;
            }
            let id_ = self.lower_expr(&args[1]);
            self.re_repl_param = false;
            id_
        };
        let s_id = self.lower_expr(&args[2]);
        self.stmts.push(MirStmt::Call {
            func: if callable_repl {
                "py_re_sub_call".to_string()
            } else {
                "py_re_sub".to_string()
            },
            args: vec![pat_id, repl_id, s_id],
            dest: id,
            type_args: vec![],
        });
        self.exprs.insert(id, MirExpr::Var(id));
        self.type_map.insert(id, Type::Str);
        Some(id)
    }
}
