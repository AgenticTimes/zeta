// src/backend/codegen/codegen_stmt_flow.rs
// 批 1002（轴 D 第二刀续）：gen_stmt 的控制流族自 codegen.rs 迁出——
// If / While（含 Python while-else 与 break 语义）／For（range 版，
// for-else 与 continue→for.inc）。语义零变，纯位置迁移（迁前/迁后
// LLVM IR 基线逐字节 diff 为空，纪律同批 1000）。

use super::codegen::LLVMCodegen;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use inkwell::IntPredicate;
use std::collections::HashMap;

impl<'ctx> LLVMCodegen<'ctx> {
    /// `if cond { then } else { else_ }`——then/else 尾态三分（break/continue/
    /// 普通落空）＋按块状态补 merge 分支（嵌套 if 双臂 return 的精确终止判定）。
    pub(super) fn gen_stmt_if(
        &mut self,
        cond: &u32,
        then: &[MirStmt],
        else_: &[MirStmt],
        exprs: &HashMap<u32, MirExpr>,
    ) {
        let cond_i1 = {
            let cv = self.gen_expr_safe(cond, exprs);
            self.cond_i1_from(*cond, cv, "cond_i1")
        };
        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let then_bb = self.context.append_basic_block(parent_fn, "then");
        let else_bb = self.context.append_basic_block(parent_fn, "else");
        let merge_bb = self.context.append_basic_block(parent_fn, "merge");
        self.builder
            .build_conditional_branch(cond_i1, then_bb, else_bb)
            .unwrap();

        // Generate then block
        self.builder.position_at_end(then_bb);
        let then_ends_with_break =
            then.last().is_some_and(|s| matches!(s, MirStmt::Break));
        let then_ends_with_continue =
            then.last().is_some_and(|s| matches!(s, MirStmt::Continue));
        if then_ends_with_break {
            for s in &then[..then.len() - 1] {
                self.gen_stmt(s, exprs);
            }
            if let Some((_, exit_bb)) = self.loop_stack.last() {
                self.builder.build_unconditional_branch(*exit_bb).unwrap();
            }
        } else if then_ends_with_continue {
            for s in &then[..then.len() - 1] {
                self.gen_stmt(s, exprs);
            }
            if let Some((cond_bb, _)) = self.loop_stack.last() {
                self.builder.build_unconditional_branch(*cond_bb).unwrap();
            }
        } else {
            for s in then {
                self.gen_stmt(s, exprs);
            }
        }
        // Ask the BLOCK, not the statement list: a branch whose last
        // statement is an inner `if` with both arms returning IS
        // terminated, yet contains no top-level `Return` — the old
        // statement scan missed that and appended a branch after the
        // `ret` ("Terminator found in the middle of a basic block").
        // Conversely a `Return` nested mid-branch left the block open
        // ("does not have terminator"). The block's own state is exact.
        let then_needs_branch = self
            .builder
            .get_insert_block()
            .map_or(false, |b| b.get_terminator().is_none());
        if then_needs_branch {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }

        // Generate else block
        self.builder.position_at_end(else_bb);
        let else_ends_with_break =
            else_.last().is_some_and(|s| matches!(s, MirStmt::Break));
        let else_ends_with_continue =
            else_.last().is_some_and(|s| matches!(s, MirStmt::Continue));
        if else_ends_with_break {
            for s in &else_[..else_.len() - 1] {
                self.gen_stmt(s, exprs);
            }
            if let Some((_, exit_bb)) = self.loop_stack.last() {
                self.builder.build_unconditional_branch(*exit_bb).unwrap();
            }
        } else if else_ends_with_continue {
            for s in &else_[..else_.len() - 1] {
                self.gen_stmt(s, exprs);
            }
            if let Some((cond_bb, _)) = self.loop_stack.last() {
                self.builder.build_unconditional_branch(*cond_bb).unwrap();
            }
        } else {
            for s in else_ {
                self.gen_stmt(s, exprs);
            }
        }
        // Same block-state check for the else arm.
        let else_needs_branch = self
            .builder
            .get_insert_block()
            .map_or(false, |b| b.get_terminator().is_none());
        if else_needs_branch {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }

        // Continue at merge block
        self.builder.position_at_end(merge_bb);
        // dest is handled by assignments in the branches
    }

    /// `while cond { body } else { else_body }`——Python while-else：正常
    /// 退出走 else 块，break 越过 else（break 目标＝loop_exit）。
    pub(super) fn gen_stmt_while(
        &mut self,
        cond: &u32,
        pre_cond: &[MirStmt],
        body: &[MirStmt],
        else_body: &[MirStmt],
        exprs: &HashMap<u32, MirExpr>,
    ) {
        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        // PY-A: Python `while … else`. Two exit targets:
        //   - normal exit (condition false) → the else block
        //   - `break` → past the else block (break target = loop_exit_bb)
        // This is exactly Python's semantics, and it needs no `break`
        // rewriting and no "did we break" flag.
        let has_else = !else_body.is_empty();
        let loop_cond_bb = self.context.append_basic_block(parent_fn, "while.cond");
        let loop_body_bb = self.context.append_basic_block(parent_fn, "while.body");
        let loop_exit_bb = self.context.append_basic_block(parent_fn, "while.exit");
        let loop_else_bb = if has_else {
            self.context.append_basic_block(parent_fn, "while.else")
        } else {
            loop_exit_bb
        };
        let normal_exit_bb = loop_else_bb;

        // Branch to condition block
        self.builder
            .build_unconditional_branch(loop_cond_bb)
            .unwrap();

        // Generate condition block — run pre_cond first so side
        // effects of the condition (len/calls) refresh the slot.
        self.builder.position_at_end(loop_cond_bb);
        for s in pre_cond {
            self.gen_stmt(s, exprs);
        }
        let cond_i1 = {
            let cv = self.gen_expr_safe(cond, exprs);
            self.cond_i1_from(*cond, cv, "while.cond")
        };
        self.builder
            .build_conditional_branch(cond_i1, loop_body_bb, normal_exit_bb)
            .unwrap();

        // Generate loop body. `break` must skip the else block, so the
        // break target is the block AFTER it.
        self.loop_stack.push((loop_cond_bb, loop_exit_bb));
        self.builder.position_at_end(loop_body_bb);
        for s in body {
            self.gen_stmt(s, exprs);
        }
        self.loop_stack.pop();
        // Branch back to condition (unless body ends with return/break/continue,
        // which already emitted a terminator that unwinds the loop)
        let body_ends_terminated = body.iter().any(|s| {
            matches!(s, MirStmt::Return { .. } | MirStmt::Break | MirStmt::Continue)
        });
        if !body_ends_terminated {
            self.builder
                .build_unconditional_branch(loop_cond_bb)
                .unwrap();
        }

        // Else block: reached only on normal completion.
        if has_else {
            self.builder.position_at_end(loop_else_bb);
            for s in else_body {
                self.gen_stmt(s, exprs);
            }
            let else_terminated = else_body
                .iter()
                .any(|s| matches!(s, MirStmt::Return { .. }));
            if !else_terminated {
                self.builder
                    .build_unconditional_branch(loop_exit_bb)
                    .unwrap();
            }
        }

        // Continue at exit block
        self.builder.position_at_end(loop_exit_bb);
    }

    /// `for i in start..end { body } else { else_body }`——range 版：
    /// counter 槽驱动条件与自增（`for.inc` 承接 continue），for-else 与
    /// break 越过 else 同 While。
    pub(super) fn gen_stmt_for(
        &mut self,
        iterator: &u32,
        counter_id: &u32,
        body: &[MirStmt],
        else_body: &[MirStmt],
        exprs: &HashMap<u32, MirExpr>,
    ) {
        // For now, implement simple range-based for loop: for i in start..end
        // We need to get the range expression
        if let Some(MirExpr::Range { start, end }) = exprs.get(iterator) {
            let parent_fn = self
                .builder
                .get_insert_block()
                .unwrap()
                .get_parent()
                .unwrap();

            // Create basic blocks for loop. `for.inc` exists so
            // `continue` lands on the increment — jumping straight to
            // the condition would skip `i = i + 1` and spin forever.
            // PY-A: for `for … else`, the normal exit goes to `for.else`
            // and `break` targets the block AFTER it (see `loop_stack`
            // push below) — that is Python's semantics with no flag.
            let has_else = !else_body.is_empty();
            let loop_cond_bb = self.context.append_basic_block(parent_fn, "for.cond");
            let loop_body_bb = self.context.append_basic_block(parent_fn, "for.body");
            let loop_inc_bb = self.context.append_basic_block(parent_fn, "for.inc");
            let loop_exit_bb = self.context.append_basic_block(parent_fn, "for.exit");
            let loop_else_bb = if has_else {
                self.context.append_basic_block(parent_fn, "for.else")
            } else {
                loop_exit_bb
            };
            let normal_exit_bb = loop_else_bb;

            // Get start and end values
            let start_val = self.gen_expr_safe(start, exprs).into_int_value();
            let end_val = self.gen_expr_safe(end, exprs).into_int_value();

            // Get loop variable pointer from locals map
            // PY-A (任务 #54): the counter slot drives the condition and
            // the increment; `var_id` is written only by the bind `Assign`
            // that gen.rs prepends to `body`. Keeping the two apart is what
            // makes Python's post-loop value (`range(3)` ⇒ last bound 2,
            // not 3) fall out of the CFG instead of needing a fix-up store.
            let counter_ptr = *self.locals.get(counter_id).unwrap();

            // Initialize the counter to start
            self.builder.build_store(counter_ptr, start_val).unwrap();

            // Branch to condition block
            self.builder
                .build_unconditional_branch(loop_cond_bb)
                .unwrap();

            // Generate condition block
            self.builder.position_at_end(loop_cond_bb);

            // Load current counter value
            let current_val = self
                .builder
                .build_load(self.i64_type, counter_ptr, "")
                .unwrap()
                .into_int_value();

            // Check if current_val < end_val
            let cond = self
                .builder
                .build_int_compare(
                    IntPredicate::SLT, // Signed less than
                    current_val,
                    end_val,
                    "for.cond",
                )
                .unwrap();

            self.builder
                .build_conditional_branch(cond, loop_body_bb, normal_exit_bb)
                .unwrap();

            // Generate loop body
            self.builder.position_at_end(loop_body_bb);

            // Store loop variable in local variables map for use in body
            // We need to find the variable ID for this pattern
            // For now, we'll just use the pointer directly

            // `break`/`continue` inside a for body need a target too —
            // without pushing here, `if cond: continue` produced a
            // basic block with no terminator (LLVM: "Basic Block does
            // not have terminator") and `break` fell through.
            self.loop_stack.push((loop_inc_bb, loop_exit_bb));
            for s in body {
                self.gen_stmt(s, exprs);
            }
            self.loop_stack.pop();

            // The body may already have terminated (continue/break/
            // return); only fall through to the increment otherwise.
            let needs_fallthrough = self
                .builder
                .get_insert_block()
                .map(|b| b.get_terminator().is_none())
                .unwrap_or(false);
            if needs_fallthrough {
                self.builder
                    .build_unconditional_branch(loop_inc_bb)
                    .unwrap();
            }

            // Increment block: counter += 1, then back to the condition.
            self.builder.position_at_end(loop_inc_bb);
            let current_val_after = self
                .builder
                .build_load(self.i64_type, counter_ptr, "")
                .unwrap()
                .into_int_value();

            let next_val = self
                .builder
                .build_int_add(current_val_after, self.i64_type.const_int(1, false), "")
                .unwrap();

            self.builder.build_store(counter_ptr, next_val).unwrap();

            self.builder
                .build_unconditional_branch(loop_cond_bb)
                .unwrap();

            // Continue at exit block
            if has_else {
                self.builder.position_at_end(loop_else_bb);
                for s in else_body {
                    self.gen_stmt(s, exprs);
                }
                let else_terminated = else_body
                    .iter()
                    .any(|s| matches!(s, MirStmt::Return { .. }));
                if !else_terminated {
                    self.builder
                        .build_unconditional_branch(loop_exit_bb)
                        .unwrap();
                }
            }
            self.builder.position_at_end(loop_exit_bb);
        } else {
            // Not a range iterator - for now, just skip
        }
    }
}
