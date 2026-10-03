//! 批次 874：FuncDef 语句臂发射体（嵌套函数定义的就地降型——869 零适配法）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_funcdef_stmt(
        &mut self,
        fn_name: &String,
        params: &Vec<(String, String)>,
        body: &Vec<AstNode>,
        ret_expr: &Option<Box<AstNode>>,
    ) {
            // Batch 763 (#33 M5): 本函数的声明返回型供 Return 收口；嵌套
            // def 走提升臂、子 MirGen 各自为政，这里保存/恢复外层值。
            let saved_fn_ret = self.current_fn_ret.take();
            self.current_fn_ret = self.func_ret_types.get(fn_name).cloned();
            // PY-A: NESTED def inside a function body — lowering inline
            // mixes its Returns into the enclosing stream (double
            // terminator). Instead, lower it as a STANDALONE synthetic
            // function via the same child-MirGen path as closures, and
            // publish it through generated_mirs (merged into codegen by
            // the Resolver pipeline). Free variables resolve through the
            // env runtime; `outer.inner(...)` and bare `inner(...)`
            // call sites both bind by name via nonlocal/fallback.
            if self.fn_depth > 1 {
                let param_names: Vec<String> =
                    params.iter().map(|(n, _)| n.clone()).collect();
                // BATCH-441: the parser promotes a block body's TAIL element out of
                // `body` into `ret_expr` (`top_level.rs:297-331`); the non-nested arm
                // below consumes it, this hoisted path cloned only `body` — so every
                // nested `def`, and every method of a `class` written inside a
                // function body, silently lost its LAST statement (t472; a trailing
                // `try:` is a `Block` whose tail is an `If`, which is why batch 438
                // registered ② saw a whole method body vanish). Put it back as a
                // STATEMENT: Python discards a trailing expression's value, and the
                // hoisted copy's return stays whatever its own `return` says.
                let mut hoisted_body = body.clone();
                if let Some(tail) = ret_expr {
                    hoisted_body.push(tail.as_ref().clone());
                }
                let body_node = AstNode::Block { body: hoisted_body };
                let hoisted = self.lower_closure(&param_names, &body_node);
                // BATCH-438: a method of a `class` written inside a function
                // body is called through its QUALIFIED name — the receiver is
                // typed `Named(Inner)`, so the call route asks for
                // `Inner::bump`, which no table carried (measured on the pre
                // binary: `Undefined symbols for architecture arm64:
                // "_Inner__bump"`). Publish that spelling alongside the bare
                // one; the definition keeps its unique `__closure_*` symbol,
                // so two modules may each own a nested `_Impl::on_start`
                // without colliding.
                if let Some(cls) = self.current_class.clone()
                    && param_names.iter().any(|p| Self::is_receiver_param(p))
                {
                    let qualified = format!("{cls}::{fn_name}");
                    self.closure_vars.insert(qualified.clone(), hoisted.clone());
                    self.hoisted_names.insert(qualified.clone(), hoisted.clone());
                    self.nested_class_aliases
                        .push((qualified, hoisted.clone()));
                }
                // bind user name → synthetic fn so `inc()` calls dispatch.
                // BATCH-642: the BARE key must not silently shadow a
                // different definition. `def helper()` in the enclosing
                // function and method `helper` of a nested class are two
                // definitions, and last-write-wins bound BOTH call sites to
                // one `__closure_*` symbol (measured pre-fix: the fixture
                // printed `9 9` where CPython prints `7 9`; MIR showed the
                // two calls carrying the identical `func:` field). Keep the
                // first owner of the bare name and say out loud what was
                // not bound — the method stays reachable through the
                // receiver-typed route (`Box::helper` qualified key).
                let shadowed = self
                    .closure_vars
                    .get(fn_name.as_str())
                    .filter(|prev| *prev != &hoisted)
                    .cloned();
                if let Some(prev) = &shadowed {
                    eprintln!(
                        "warning: PY-A: bare name `{}` already refers to `{}` — the later \
                         definition `{}` is not bound under that bare name (calls through a \
                         receiver still resolve it)",
                        fn_name, prev, hoisted
                    );
                } else {
                    self.closure_vars.insert(fn_name.clone(), hoisted.clone());
                }
                // The call site reads the return type off `closure_ret_tys`
                // (and falls back to I64, which for a string means "print the
                // heap address"). The Closure-expression arm records it; the
                // hoisted-def arm had no equivalent line, so every nested
                // `def` was called through an I64-typed destination.
                if let Some(t) = self.last_closure_ret_ty.clone() {
                    self.closure_ret_tys.insert(hoisted.clone(), t);
                }
                // Publish under the user-visible name too (alias map) — same
                // BATCH-642 rule: a second definition never silently retargets
                // a bare name that already belongs to another one.
                if shadowed.is_none() {
                    self.hoisted_names.insert(fn_name.clone(), hoisted);
                }
                self.current_fn_ret = saved_fn_ret;
                return;
            }
            for stmt in body {
                self.lower_ast(stmt);
            }
            if let Some(ret_expr) = ret_expr {
                let val = self.lower_expr(ret_expr);
                let val = self.coerce_return_val(val);
                self.stmts.push(MirStmt::Return { val });
            }
            self.current_fn_ret = saved_fn_ret;
    }
}
