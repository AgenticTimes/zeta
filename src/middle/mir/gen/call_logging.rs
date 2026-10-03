//! 批次 836：logging 家族执行文件——FileHandler（mode/… 忽略并警告一次）
//! 与 getLogger（0 参补空名——运行期 logger 身份即名字，本地全 no-op shim）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;
use std::sync::OnceLock;

impl MirGen {
    /// 统一入口：FileHandler / getLogger 分派。
    pub(super) fn lower_logging(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
        self.lower_logging_file_handler(receiver, method, args, dest)
            .or_else(|| self.lower_logging_get_logger(receiver, method, args, dest))
    }

    /// `logging.FileHandler(path)` — mode/… kwargs ignored (warned once).
    pub(super) fn lower_logging_file_handler(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
        if args.len() <= 1 {
            return None;
        }
        let (m, mem) = self.py_member_target(receiver, method)?;
        if m != "logging" || mem != "FileHandler" {
            return None;
        }
        static WARNED_FH: OnceLock<()> = OnceLock::new();
        WARNED_FH.get_or_init(|| {
            eprintln!(
                "warning: PY-A: logging.FileHandler mode/… is ignored \
                 (local no-op shim)"
            );
        });
        let path = self.lower_expr(&args[0]);
        self.stmts.push(MirStmt::Call {
            func: "py_logging_FileHandler".to_string(),
            args: vec![path],
            dest,
            type_args: vec![],
        });
        self.exprs.insert(dest, MirExpr::Var(dest));
        self.type_map
            .insert(dest, Type::Named("PyFileHandler".to_string(), vec![]));
        Some(dest)
    }

    /// `logging.getLogger()` / `logging.getLogger(name)` — 0 参补空名
    ///（防 arity-mangled 幽灵符号 py_logging_getLogger_0）。
    pub(super) fn lower_logging_get_logger(
        &mut self,
        receiver: &Option<Box<AstNode>>,
        method: &str,
        args: &[AstNode],
        dest: u32,
    ) -> Option<u32> {
        if !args.is_empty() {
            return None;
        }
        let (m, mem) = self.py_member_target(receiver, method)?;
        if m != "logging" || mem != "getLogger" {
            return None;
        }
        let name_id = self.next_id();
        self.exprs
            .insert(name_id, MirExpr::StringLit(String::new()));
        self.type_map.insert(name_id, Type::Str);
        self.stmts.push(MirStmt::Call {
            func: "py_logging_getLogger".to_string(),
            args: vec![name_id],
            dest,
            type_args: vec![],
        });
        self.exprs.insert(dest, MirExpr::Var(dest));
        self.type_map
            .insert(dest, Type::Named("PyLogger".to_string(), vec![]));
        Some(dest)
    }
}
