// src/frontend/parser/stmt.rs
//! Module for parsing statements in the Zeta language.

use super::expr::{parse_condition, parse_full_expr, parse_match_expr};
use super::parser::{
    kw_boundary, parse_ident, parse_path, parse_type, skip_ws_and_comments, skip_ws_and_comments0,
    ws,
};
use super::pattern::parse_pattern;
use super::top_level::{parse_class, parse_const, parse_func, parse_type_alias, parse_use_targets};
use crate::frontend::ast::AstNode;
use nom::IResult;
use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::{tag, take_while};
use nom::character::complete::none_of;
use nom::combinator::{map, opt, peek};
use nom::error::Error as NomError;
use nom::sequence::{delimited, preceded, terminated};

pub fn parse_block_body(input: &str) -> IResult<&str, Vec<AstNode>> {
    let mut body = vec![];
    let mut current = input;
    loop {
        let (next, _) = skip_ws_and_comments(current)?;
        if next.is_empty()
            || peek(tag::<&str, &str, NomError<&str>>("}"))
                .parse(next)
                .is_ok()
            || peek(tag::<&str, &str, NomError<&str>>(")"))
                .parse(next)
                .is_ok()
            || peek(tag::<&str, &str, NomError<&str>>(","))
                .parse(next)
                .is_ok()
        {
            current = next;
            break;
        }

        // Skip extra semicolons (treat as no-op)
        if let Ok((next_skip, _)) = opt(ws(tag::<_, _, NomError<&str>>(";"))).parse(next) {
            if next_skip.len() < next.len() {
                current = next_skip;
                continue;
            }
        }
        if let Ok((next_stmt, stmt)) = parse_stmt(next) {
            let name = match &stmt {
                AstNode::FuncDef { name, .. } => Some(name.as_str()),
                AstNode::ConstDef { name, .. } => Some(name.as_str()),
                _ => None,
            };
            body.push(stmt);
            current = next_stmt;
            continue;
        }
        if let Ok((next_expr, expr)) = parse_full_expr(next) {
            warn_if_swallowed_prefix(&expr, next, next_expr);
            body.push(AstNode::ExprStmt {
                expr: Box::new(expr),
            });
            current = next_expr;
            continue;
        }

        // Env-gated trace (`ZETA_PARSE_TRACE=1`): report exactly where the
        // block loop choked. This is how multi-line truncation causes get
        // found — by instrument, not by guessing constructs one at a time.
        if crate::diagnostics::env_flag("ZETA_PARSE_TRACE") {
            let consumed = input.len() - next.len();
            let preview: String = next.chars().take(80).collect();
            eprintln!("[parse-trace] 语句解析失败：块内偏移 {consumed}，剩余文本开头：{preview:?}");
        }
        return Err(nom::Err::Error(NomError::new(
            next,
            nom::error::ErrorKind::Many0,
        )));
    }
    Ok((current, body))
}

fn parse_let(input: &str) -> IResult<&str, AstNode> {
    // `var` is the Zeta spelling of a mutable local declaration. It was never
    // recognised here, so `var arr: [N]u64 = ...` parsed as a stray `var`
    // expression followed by an unparseable `arr: … = …` — which aborted the
    // enclosing block and silently dropped the remainder of the body.
    let (input, _) = alt((ws(tag("let")), ws(tag("var")))).parse(input)?;
    let (input, mut_) = opt(ws(tag("mut"))).parse(input)?;
    let (input, pattern) = ws(parse_pattern).parse(input)?;
    let (input, ty) = opt(preceded(ws(tag(":")), ws(parse_type))).parse(input)?;
    // Allow `let x: Type;` (no initializer) as well as `let x = expr;` and `let x: Type = expr;`
    let (input, init_expr) = opt(preceded(ws(tag("=")), ws(parse_full_expr))).parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    Ok((
        input,
        AstNode::Let {
            mut_: mut_.is_some(),
            pattern: Box::new(pattern),
            ty,
            expr: match init_expr {
                Some(e) => Box::new(e),
                None => Box::new(AstNode::Lit(0)), // Placeholder for uninitialized
            },
        },
    ))
}

fn parse_for(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("for")).parse(input)?;
    let (rest, first) = ws(parse_pattern).parse(input)?;
    // PY-A: Python tuple target without parentheses — `for i, v in xs:`.
    // parse_pattern only accepts a parenthesised tuple, so the unparenthesised
    // form failed to parse and the whole statement was dropped.
    let (rest, pattern) = {
        let mut cur = rest.trim_start();
        let mut items = vec![first];
        while let Some(after_comma) = cur.strip_prefix(',') {
            let (next, p) = ws(parse_pattern).parse(after_comma)?;
            items.push(p);
            cur = next.trim_start();
        }
        if items.len() > 1 {
            (cur, AstNode::Tuple(items))
        } else {
            (rest, items.remove(0))
        }
    };
    let (input, _) = ws(tag("in")).parse(rest)?;
    let (input, expr) = ws(parse_full_expr).parse(input)?;
    // PY-4: Python `range(n)` / `range(a, b)` → Range node (end-exclusive,
    // matching both Python semantics and Zeta's `..` operator). A step
    // argument is not supported in V1 and stays an unknown-function error.
    let expr = match expr {
        AstNode::Call {
            receiver: None,
            ref method,
            ref args,
            ..
        } if method == "range" && (args.len() == 1 || args.len() == 2) => {
            let start = if args.len() == 2 {
                args[0].clone()
            } else {
                AstNode::Lit(0)
            };
            let end = args[args.len() - 1].clone();
            AstNode::Range {
                start: Box::new(start),
                end: Box::new(end),
                inclusive: false,
            }
        }
        other => other,
    };
    let (input, body) = delimited(ws(tag("{")), parse_block_body, ws(tag("}"))).parse(input)?;
    let (input, else_body) = parse_loop_else(input)?;
    // PY-A: `for i, v in enumerate(X):` — desugar to an index loop:
    //   for i in range(len(X)): v = X[i]; <body>
    // enumerate() has no runtime representation and previously the whole loop
    // body was silently dropped (the loop vanished from the MIR entirely).
    if let (
        AstNode::Tuple(names),
        AstNode::Call {
            receiver: None,
            method,
            args: call_args,
            ..
        },
    ) = (&pattern, &expr)
    {
        if method == "enumerate" && names.len() == 2 && (1..=2).contains(&call_args.len()) {
            if let (AstNode::Var(idx), AstNode::Var(val)) = (&names[0], &names[1]) {
                let coll = call_args[0].clone();
                // `enumerate(xs, start)`: the ARRAY index always runs
                // 0..len(xs); only the yielded counter is offset by `start`.
                // Using `start` as the array index made every element read out
                // of bounds (the loop body never ran).
                let (loop_var, idx_expr, subscript_index) = if call_args.len() == 2 {
                    static ENUM_SEQ: std::sync::atomic::AtomicUsize =
                        std::sync::atomic::AtomicUsize::new(0);
                    let k = format!(
                        "__enum_k_{}",
                        ENUM_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    );
                    let kvar = AstNode::Var(k.clone());
                    (
                        kvar.clone(),
                        AstNode::BinaryOp {
                            op: "+".to_string(),
                            left: Box::new(call_args[1].clone()),
                            right: Box::new(kvar.clone()),
                        },
                        kvar,
                    )
                } else {
                    let iv = AstNode::Var(idx.clone());
                    (iv.clone(), iv.clone(), iv)
                };
                let len_call = AstNode::Call {
                    receiver: None,
                    method: "len".to_string(),
                    args: vec![coll.clone()],
                    type_args: vec![],
                    structural: false,
                };
                let range_expr = AstNode::Range {
                    start: Box::new(AstNode::Lit(0)),
                    end: Box::new(len_call),
                    inclusive: false,
                };
                let elem = AstNode::Subscript {
                    base: Box::new(coll),
                    index: Box::new(subscript_index),
                };
                let mut new_body = Vec::with_capacity(body.len() + 2);
                if call_args.len() == 2 {
                    new_body.push(AstNode::Assign(
                        Box::new(AstNode::Var(idx.clone())),
                        Box::new(idx_expr),
                    ));
                }
                new_body.push(AstNode::Assign(
                    Box::new(AstNode::Var(val.clone())),
                    Box::new(elem),
                ));
                new_body.extend(body);
                return Ok((
                    input,
                    AstNode::For {
                        pattern: Box::new(loop_var),
                        expr: Box::new(range_expr),
                        body: new_body,
                        else_body,
                    },
                ));
            }
        }
    }
    Ok((
        input,
        AstNode::For {
            pattern: Box::new(pattern),
            expr: Box::new(expr),
            body,
            else_body,
        },
    ))
}

pub(crate) fn parse_loop(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("loop")).parse(input)?;
    let (input, body) = delimited(ws(tag("{")), parse_block_body, ws(tag("}"))).parse(input)?;
    Ok((input, AstNode::Loop { body }))
}

fn parse_while(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("while")).parse(input)?;
    let (input, cond) = ws(parse_condition).parse(input)?;
    let (input, body) = delimited(ws(tag("{")), parse_block_body, ws(tag("}"))).parse(input)?;
    let (input, else_body) = parse_loop_else(input)?;
    Ok((
        input,
        AstNode::While {
            cond: Box::new(cond),
            body,
            else_body,
        },
    ))
}

/// PY-A: optional trailing `else` block on a loop — Python's `for … else` /
/// `while … else`, which runs only when the loop finished WITHOUT `break`.
/// `while … else` with no `else` parses as an empty vec.
fn parse_loop_else(input: &str) -> IResult<&str, Vec<AstNode>> {
    let (input, body) = opt(preceded(
        ws(tag("else")),
        delimited(ws(tag("{")), parse_block_body, ws(tag("}"))),
    ))
    .parse(input)?;
    Ok((input, body.unwrap_or_default()))
}

fn parse_unsafe(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("unsafe")).parse(input)?;
    let (input, body) = delimited(ws(tag("{")), parse_block_body, ws(tag("}"))).parse(input)?;
    Ok((input, AstNode::Unsafe { body }))
}

fn parse_if_let(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("if")).parse(input)?;
    let (input, _) = ws(tag("let")).parse(input)?;
    let (input, pattern) = ws(parse_pattern).parse(input)?;
    let (input, _) = ws(tag("=")).parse(input)?;
    let (input, expr) = ws(parse_full_expr).parse(input)?;
    let (input, then) = delimited(ws(tag("{")), parse_block_body, ws(tag("}"))).parse(input)?;
    let (input, else_opt) = opt(preceded(
        ws(tag("else")),
        delimited(ws(tag("{")), parse_block_body, ws(tag("}"))),
    ))
    .parse(input)?;
    let else_: Vec<AstNode> = else_opt.unwrap_or(vec![]);
    Ok((
        input,
        AstNode::IfLet {
            pattern: Box::new(pattern),
            expr: Box::new(expr),
            then,
            else_,
        },
    ))
}

fn parse_if(input: &str) -> IResult<&str, AstNode> {
    // Peek: if the next non-whitespace token after 'if' is 'let', this is
    // if-let, not if. Fail fast without consuming 'if'.
    let saved = input;
    let (input, _) = ws(tag("if")).parse(input)?;
    if input.trim_start().starts_with("let") {
        return Err(nom::Err::Error(NomError::new(
            saved,
            nom::error::ErrorKind::Tag,
        )));
    }
    parse_if_tail(input)
}

/// Condition + then-block + else chain; the leading `if` keyword must already
/// be consumed. Also the entry point for Python-style `elif` chains.
fn parse_if_tail(input: &str) -> IResult<&str, AstNode> {
    let (input, cond) = ws(parse_full_expr).parse(input)?;
    let (input, then) = delimited(ws(tag("{")), parse_block_body, ws(tag("}"))).parse(input)?;

    // Parse else clause: `else { ... }`, `else if ...`, or `elif ...`
    let (input, else_opt) = opt(alt((
        preceded(
            ws(tag("else")),
            alt((
                // else { ... } block
                map(
                    delimited(ws(tag("{")), parse_block_body, ws(tag("}"))),
                    |body| body,
                ),
                // else if ... (parse_if consumes its own `if` keyword)
                map(parse_if, |if_node| vec![if_node]),
            )),
        ),
        // elif ... (PY-2 alias for else-if)
        preceded(ws(tag("elif")), map(parse_if_tail, |if_node| vec![if_node])),
    )))
    .parse(input)?;

    let else_: Vec<AstNode> = else_opt.unwrap_or(vec![]);

    // BATCH-290: the `if __name__ == "__main__":` guard is NO LONGER unwrapped
    // here. Unwrapping it for EVERY module made `import pkg.mod` execute that
    // module's entry point (with garbage argparse args — measured: importing
    // `jq_wufu_local` ran a whole bogus backtest before the driver's own code).
    // The If survives; MIR lowers `__name__` to the compiling module's name
    // (gen.rs), so the guard runs only in the root (`__main__`) module — CPython
    // semantics. The recursion this unwrap originally dodged is handled in
    // `synthesize_implicit_main` (top_level.rs) instead.

    Ok((
        input,
        AstNode::If {
            cond: Box::new(cond),
            then,
            else_,
        },
    ))
}

fn parse_assign(input: &str) -> IResult<&str, AstNode> {
    use super::expr::parse_unary;

    // PY-A: annotated assignment / annotation-only statement —
    // `x: int = 5`, `x: int`, and attribute form `self._x: T = v`.
    // Bare-ident-only left `self._today_buys: set[str] = set()` as a leftover
    // `: …`, which aborted the enclosing class — PositionLedger / LocalBackend
    // vanished from the module graph (fail-open).
    {
        if let Ok((after_lhs, lhs)) = ws(parse_unary).parse(input) {
            let is_ann_target = matches!(
                &lhs,
                AstNode::Var(_) | AstNode::FieldAccess { .. } | AstNode::Subscript { .. }
            );
            if is_ann_target {
                let after_lhs = after_lhs.trim_start();
                if after_lhs.starts_with(':') && !after_lhs.starts_with("::") {
                    if let Ok((after_ty, ty)) = ws(parse_type).parse(&after_lhs[1..]) {
                        let after_ty_ws = after_ty.trim_start();
                        if let Some(rhs) = after_ty_ws.strip_prefix('=') {
                            if !rhs.starts_with('=') {
                                let (after_val, val) = parse_full_expr(rhs)?;
                                // A class-shaped annotation (`pd.DataFrame | None`)
                                // is the ONLY record of the slot's real type when
                                // the initializer is `None` — keep it attached so
                                // MIR lowering can refresh the slot type.
                                let class_like = ty
                                    .split('|')
                                    .map(|p| p.trim())
                                    .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case("none"))
                                    .any(|p| {
                                        p.rsplit('.')
                                            .next()
                                            .map(|t| t.starts_with(char::is_uppercase))
                                            .unwrap_or(false)
                                    });
                                let lhs = if class_like && matches!(&lhs, AstNode::Var(_)) {
                                    Box::new(AstNode::TypeAnnotatedPattern {
                                        pattern: Box::new(lhs),
                                        ty,
                                    })
                                } else {
                                    Box::new(lhs)
                                };
                                return Ok((after_val, AstNode::Assign(lhs, Box::new(val))));
                            }
                        }
                        // Annotation with no value: a declared-but-unbound name.
                        // Consume it as a no-op rather than aborting the parse.
                        return Ok((
                            after_ty,
                            AstNode::ExprStmt {
                                expr: Box::new(AstNode::Lit(0)),
                            },
                        ));
                    }
                }
            }
        }
    }

    // PY-A: parallel assignment / tuple unpacking `a, b = x, y` — all LHS
    // elements must be simple targets; falls through to the single-target
    // path when no comma follows the first element.
    {
        let saved = input;
        if let Ok((after_first, first)) = ws(parse_unary).parse(input) {
            let after_first = skip_ws_and_comments(after_first)
                .map(|(i, _)| i)
                .unwrap_or(after_first);
            if after_first.starts_with(',') {
                let mut items = vec![first];
                let mut cur = after_first;
                let mut ok = true;
                loop {
                    // after each element: either `=` ends the LHS, or `,` +
                    // another element continues it
                    let cur_ws = skip_ws_and_comments(cur)
                        .map(|(i, _)| i)
                        .unwrap_or(cur);
                    if cur_ws.starts_with('=') {
                        cur = cur_ws;
                        break;
                    }
                    match ws(tag(",")).parse(cur_ws)
                        .ok()
                        .map(|(rest, _)| rest)
                    {
                        Some(rest) => {
                            let rest = skip_ws_and_comments(rest)
                                .map(|(i, _)| i)
                                .unwrap_or(rest);
                            match ws(parse_unary).parse(rest) {
                                Ok((rest2, item)) => {
                                    items.push(item);
                                    cur = rest2;
                                }
                                Err(_) => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && items.len() > 1 {
                    let (after_eq, _) = ws(tag("=")).parse(cur)?;
                    let mut rhs_items = Vec::new();
                    let mut rcur = after_eq;
                    loop {
                        let (rest, item) = ws(parse_full_expr).parse(rcur)?;
                        rhs_items.push(item);
                        let rest = skip_ws_and_comments(rest)
                            .map(|(i, _)| i)
                            .unwrap_or(rest);
                        if rest.starts_with(',') {
                            rcur = skip_ws_and_comments(&rest[1..])
                                .map(|(i, _)| i)
                                .unwrap_or(&rest[1..]);
                            if rcur.starts_with('\n') || rcur.is_empty() {
                                break;
                            }
                        } else {
                            rcur = rest;
                            break;
                        }
                    }
                    if rhs_items.len() == items.len() {
                        let (after_stmt, _) = opt(ws(tag(";"))).parse(rcur)?;
                        return Ok((
                            after_stmt,
                            AstNode::Assign(
                                Box::new(AstNode::Tuple(items)),
                                Box::new(AstNode::Tuple(rhs_items)),
                            ),
                        ));
                    }
                    if rhs_items.len() == 1 {
                        // `a, b = f()` — call-return tuple unpacking
                        let (after_stmt, _) = opt(ws(tag(";"))).parse(rcur)?;
                        return Ok((
                            after_stmt,
                            AstNode::Assign(
                                Box::new(AstNode::Tuple(items)),
                                Box::new(rhs_items.pop().unwrap()),
                            ),
                        ));
                    }
                }
            }
            let _ = saved;
        }
    }

    // First try to parse the left-hand side (which includes unary prefix and postfix)
    let (input, lhs) = ws(parse_unary).parse(input)?;

    // Try to parse compound assignment operators first, then simple assignment
    let (input, op) = alt((
        ws(tag("+=")),
        ws(tag("-=")),
        ws(tag("*=")),
        ws(tag("/=")),
        ws(tag("%=")),
        ws(tag("&=")),
        ws(tag("|=")),
        ws(tag("^=")),
        ws(tag("<<=")),
        ws(tag(">>=")),
        ws(tag("=")),
    ))
    .parse(input)?;

    let (input, rhs) = ws(parse_full_expr).parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;

    if op == "=" {
        // PY-A: chained assignment `a = b = expr` (very common Python). The
        // parser used to consume `a = b` and leave `= expr`, so the statement
        // failed and the whole definition — plus the rest of the file — was
        // silently dropped. The right-hand side is evaluated ONCE into the
        // first target, then copied into the rest (`a = b = 0` ≡ `a = 0; b = a`).
        // PY-A: chained assignment `a = b = expr` (very common Python). The
        // rhs parse stops at `b` and leaves `= expr` behind, so that leftover
        // `expr` is the VALUE OF THE WHOLE CHAIN, while each thing parsed so far
        // becomes a target. Emit `a = expr; b = a; …` (the rhs is evaluated
        // once into the first target, then copied left-to-right) — before this
        // the statement failed to parse and the whole definition, plus the rest
        // of the file, was silently dropped.
        let mut targets: Vec<AstNode> = Vec::new();
        let mut value = rhs;
        let mut rest = input;
        loop {
            let t = rest.trim_start();
            if !(t.starts_with('=') && !t[1..].starts_with('=')) {
                break;
            }
            // Whatever we parsed just before the `=` is a TARGET, not the value.
            targets.push(value);
            let (after_eq, next_rhs) = ws(parse_full_expr).parse(&t[1..])?;
            value = next_rhs;
            rest = after_eq;
        }
        let (rest, _) = opt(ws(tag(";"))).parse(rest)?;
        if targets.is_empty() {
            return Ok((rest, AstNode::Assign(Box::new(lhs), Box::new(value))));
        }
        let lhs_for_copy = lhs.clone();
        let mut body = vec![AstNode::Assign(Box::new(lhs), Box::new(value.clone()))];
        for t in targets {
            let val = match &lhs_for_copy {
                AstNode::Var(_) => lhs_for_copy.clone(),
                _ => value.clone(),
            };
            body.push(AstNode::Assign(Box::new(t), Box::new(val)));
        }
        return Ok((rest, AstNode::Block { body }));
    } else {
        // Compound assignment: produce AssignOp with the base operator (without =)
        let bin_op = op.trim_end_matches('=').to_string();
        Ok((
            input,
            AstNode::AssignOp {
                op: bin_op,
                target: Box::new(lhs),
                value: Box::new(rhs),
            },
        ))
    }
}

/// PY-A: match-arm form — single value only. Inside a match arm the comma
/// separates arms, so a bare `return 0,` must NOT consume the next arm.
pub fn parse_return_single(input: &str) -> IResult<&str, AstNode> {
    let input = kw_boundary(input, "return")
        .ok_or_else(|| nom::Err::Error(NomError::new(input, nom::error::ErrorKind::Tag)))?;
    let (input, inner) = opt(ws(parse_full_expr)).parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    Ok((
        input,
        AstNode::Return(Box::new(inner.unwrap_or(AstNode::Lit(0)))),
    ))
}

pub fn parse_return(input: &str) -> IResult<&str, AstNode> {
    let input = kw_boundary(input, "return")
        .ok_or_else(|| nom::Err::Error(NomError::new(input, nom::error::ErrorKind::Tag)))?;
    let (mut cur, first) = match opt(ws(parse_full_expr)).parse(input)? {
        (rest, Some(e)) => (rest, Some(e)),
        (rest, None) => (rest, None),
    };
    // PY-A: `return a, b` — comma-separated values return a tuple
    let mut items: Vec<AstNode> = match first {
        Some(e) => vec![e],
        None => vec![],
    };
    loop {
        let cur_ws = skip_ws_and_comments(cur)
            .map(|(i, _)| i)
            .unwrap_or(cur);
        match ws(tag(",")).parse(cur_ws)
            .ok()
            .map(|(rest, _)| rest)
        {
            Some(rest) => {
                let rest = skip_ws_and_comments(rest)
                    .map(|(i, _)| i)
                    .unwrap_or(rest);
                if rest.is_empty() || rest.starts_with('\n') || rest.starts_with(';') {
                    cur = rest;
                    break;
                }
                match ws(parse_full_expr).parse(rest) {
                    Ok((rest2, item)) => {
                        items.push(item);
                        cur = rest2;
                    }
                    Err(_) => {
                        cur = rest;
                        break;
                    }
                }
            }
            None => break,
        }
    }
    let value = if items.len() > 1 {
        AstNode::Tuple(items)
    } else {
        items.pop().unwrap_or(AstNode::Lit(0))
    };
    let (input, _) = opt(ws(tag(";"))).parse(cur)?;
    Ok((input, AstNode::Return(Box::new(value))))
}

fn parse_break(input: &str) -> IResult<&str, AstNode> {
    let input = kw_boundary(input, "break")
        .ok_or_else(|| nom::Err::Error(NomError::new(input, nom::error::ErrorKind::Tag)))?;
    let (input, expr_opt) = opt(ws(parse_full_expr)).parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    Ok((input, AstNode::Break(expr_opt.map(Box::new))))
}

fn parse_continue(input: &str) -> IResult<&str, AstNode> {
    let input = kw_boundary(input, "continue")
        .ok_or_else(|| nom::Err::Error(NomError::new(input, nom::error::ErrorKind::Tag)))?;
    let (input, expr_opt) = opt(ws(parse_full_expr)).parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    Ok((input, AstNode::Continue(expr_opt.map(Box::new))))
}

fn parse_expr_stmt(input: &str) -> IResult<&str, AstNode> {
    let start = input;
    let (input, expr) = parse_full_expr(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    warn_if_swallowed_prefix(&expr, start, input);
    Ok((
        input,
        AstNode::ExprStmt {
            expr: Box::new(expr),
        },
    ))
}

/// A statement that is one bare name or literal does nothing, and the parser
/// only gets there by *dropping* a word it had no rule for (`static mut c = 0`
/// loses `static`, `import a::b;` binds `a` and discards `b`). Measured over the
/// 533-file suite the shape is otherwise rare: a function's implicit return is
/// the same node, but nothing parseable follows it except the `}`/`} else {` the
/// indent preprocessor appends — hence the delimiter strip. The text here is
/// preprocessed, so one of its "lines" can span several source lines.
/// 4 hits in 533 files, all real mis-parses; see roadmap batch 337.
fn warn_if_swallowed_prefix(expr: &AstNode, stmt_start: &str, rest: &str) {
    let word = match expr {
        AstNode::Var(n) => n.clone(),
        AstNode::Lit(_) => "literal".to_string(),
        _ => return,
    };
    let head = rest.split('\n').next().unwrap_or("").trim();
    if head.is_empty() {
        return;
    }
    let stripped: String = head
        .chars()
        .filter(|c| !matches!(c, '{' | '}' | '(' | ')' | ',' | ' ' | '\t'))
        .collect();
    if stripped.is_empty() || stripped == "else" {
        return;
    }
    let off = crate::frontend::indent::remaining_byte_offset(stmt_start, "");
    let line = crate::frontend::indent::original_line_at(off, "");
    eprintln!(
        "warning: [W1004] :{line}: `{word}` became a stand-alone statement while \
         '{head}' was parsed as the next one — a word on this line is being ignored"
    );
}

/// PY-A: `pass` — no-op statement.
fn parse_pass(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("pass")).parse(input)?;
    // Word-boundary: `passed` stays an identifier
    if input
        .chars()
        .next()
        .map_or(false, |c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    Ok((
        input,
        AstNode::ExprStmt {
            expr: Box::new(AstNode::Lit(0)),
        },
    ))
}

/// PY-A: `...` (Ellipsis) as a statement — abc abstractmethod stubs.
/// Without this, a method body of only `...` aborted the enclosing class
/// (`MarketDataSource`) and dropped the rest of `market_data_sources.py`.
fn parse_ellipsis_stmt(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("...")).parse(input)?;
    Ok((
        input,
        AstNode::ExprStmt {
            expr: Box::new(AstNode::Lit(0)),
        },
    ))
}

/// PY-A: `assert cond` / `assert cond, msg` — statement form.
/// Without this, `assert False` split into ExprStmt(Var("assert")) +
/// ExprStmt(Bool(false)), both no-ops — failures were silent. Call form
/// `assert(cond[, msg])` already lowers via MIR (`zeta_assert_fail`); leave
/// that path alone by declining when `(` follows the keyword.
fn parse_assert(input: &str) -> IResult<&str, AstNode> {
    let Some(rest) = super::parser::kw_boundary(input, "assert") else {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    };
    // `assert(...)` is a normal Call — do not steal it here.
    let t = rest.trim_start();
    if t.starts_with('(') {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    // Condition must be on the same line (Python); bare `assert` alone is a
    // syntax error there — here decline so a lone Ident still parses as before.
    let same_line = {
        let t = rest.trim_start_matches(|c| c == ' ' || c == '\t');
        !t.is_empty() && !t.starts_with('\n') && !t.starts_with('\r')
    };
    if !same_line {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    let (input, cond) = ws(parse_full_expr).parse(rest)?;
    let (input, msg) = match ws(tag::<_, _, NomError<&str>>(",")).parse(input) {
        Ok((after_comma, _)) => {
            let (after, m) = ws(parse_full_expr).parse(after_comma)?;
            (after, Some(m))
        }
        Err(_) => (input, None),
    };
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    let mut args = vec![cond];
    if let Some(m) = msg {
        args.push(m);
    }
    Ok((
        input,
        AstNode::ExprStmt {
            expr: Box::new(AstNode::Call {
                receiver: None,
                method: "assert".to_string(),
                args,
                type_args: vec![],
                structural: false,
            }),
        },
    ))
}

/// PY-A: `del target` — V1 only `del obj[key]` → `obj.__delitem__(key)`.
    /// Without this, `del` was a bare Var expr-stmt and the subscript was a GET
    /// (SEGFAULT / silent no-op for DataFrame columns).
    fn parse_del(input: &str) -> IResult<&str, AstNode> {
        let Some(rest) = super::parser::kw_boundary(input, "del") else {
            return Err(nom::Err::Error(NomError::new(
                input,
                nom::error::ErrorKind::Tag,
            )));
        };
        // PY-A: `del a, b` / `del x[k]` — must consume EVERY comma-separated
        // target. Stopping after the first left `, b` in the try body; the block
        // parser then failed to find `}`, the whole `try` aborted, and
        // `market_data_sources.py` dropped every def after the dotenv try
        // (only `sources__init` remained).
        //
        // Targets via parse_unary (postfix): `self.x` / `self.m[k]` — parse_primary
        // only ate `self`, left `._positions[security]` unparsed, aborted `sell`,
        // and dropped PositionLedger + LocalBackend from wufu_backend.
        let mut cur = rest;
        let mut stmts: Vec<AstNode> = Vec::new();
        loop {
            let (after, target) = ws(crate::frontend::parser::expr::parse_unary).parse(cur)?;
            let is_del_target = matches!(
                &target,
                AstNode::Var(_) | AstNode::FieldAccess { .. } | AstNode::Subscript { .. }
            );
            if !is_del_target {
                return Err(nom::Err::Error(NomError::new(
                    cur,
                    nom::error::ErrorKind::Tag,
                )));
            }
            let stmt = match target {
                AstNode::Subscript { base, index } => AstNode::ExprStmt {
                    expr: Box::new(AstNode::Call {
                        receiver: Some(base),
                        method: "__delitem__".to_string(),
                        args: vec![*index],
                        type_args: vec![],
                        structural: false,
                    }),
                },
                _ => {
                    // Name / attribute delete — V1 no-op (bindings stay); still
                    // consume so the enclosing block can continue.
                    AstNode::ExprStmt {
                        expr: Box::new(AstNode::Lit(0)),
                    }
                }
            };
            stmts.push(stmt);
            let trimmed = after.trim_start();
            if let Some(r) = trimmed.strip_prefix(',') {
                cur = r;
                continue;
            }
            if stmts.len() == 1 {
                return Ok((after, stmts.pop().unwrap()));
            }
            return Ok((after, AstNode::Block { body: stmts }));
        }
    }

/// PY-A: Python `import x[.y][ as z]` — no longer swallowed: emits a
/// `zeta_py_import("module", "alias")` marker that the Resolver collects into
/// a module-alias table. Unknown modules are diagnosed there (fail-loud),
/// so an unsupported import can never silently produce a wrong program.
fn parse_python_import(input: &str) -> IResult<&str, AstNode> {
    let start = input;
    // The word boundary must be checked BEFORE whitespace is consumed —
    // `ws(tag("import"))` swallows the trailing space, so the alphanumeric
    // test would always see the module name and reject every real import.
    let (input, _) = tag("import").parse(input)?;
    if input
        .chars()
        .next()
        .map_or(false, |c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(nom::Err::Error(NomError::new(
            start,
            nom::error::ErrorKind::Tag,
        )));
    }
    let (input, _) = take_while(|c: char| c == ' ' || c == '\t' || c == '\r')(input)?;
    let mut out: Vec<AstNode> = Vec::new();
    // `a.b.c [as d] [, e.f ...]`
    let mut cur = input;
    loop {
        // A `::` target is the Rust spelling and means what `use` means: the
        // LAST segment is what gets bound (`import std::memory;` gives
        // `memory`). Before this branch existed the parser stopped after `std`,
        // so the head was imported and the tail became a no-effect statement.
        if let Ok((after_path, path)) = ws(parse_path).parse(cur) {
            if path.len() > 1 {
                // Commit only when `use`'s tail grammar can't be the wrong read of
                // this text. The reject list is measured shape by shape (see the
                // tail-family probe in roadmap 批次 338): `as` is the one shape the
                // shared grammar has no rule for (an alias needs a field
                // `AstNode::Use` does not have) and `=`/`;` are continuations that
                // the top-level loop cannot start an item with. Committing any of
                // them strands the tail, and a top-level item no rule accepts stops
                // the caller's `many0` — so the REST OF THE FILE is dropped.
                // Measured on `import std::memory as m;`: 0 diagnostics and 171
                // lines of MIR before this branch (mis-bound to `std`, but the file
                // survived) vs W1002 and 39 lines when the `as` was committed.
                // Falling back keeps those shapes exactly where batch 337 left them.
                if let Ok((rest, nodes)) = parse_use_targets(path, after_path) {
                    let tail = rest.trim_start_matches([' ', '\t', '\r']);
                    let rejected = kw_boundary(tail, "as").is_some()
                        || tail.starts_with('=')
                        || tail.starts_with(';');
                    if !rejected {
                        out.extend(nodes);
                        match tail.strip_prefix(',') {
                            Some(t) => cur = t,
                            None => {
                                cur = tail;
                                break;
                            }
                        }
                        continue;
                    }
                }
            }
        }
        let (after_name, module) = match ws(parse_dotted_name).parse(cur) {
            Ok(v) => v,
            Err(_) => break,
        };
        if module.is_empty() {
            break;
        }
        // optional `as alias`
        let trimmed = after_name.trim_start();
        let (after_alias, alias) = if let Some(t) = trimmed.strip_prefix("as") {
            // `ws(parse_dotted_name)` above already consumed the whitespace
            // BEFORE `as`, so `t` still carries the space that must separate
            // `as` from the alias (`import pandas as pd` ⇒ t == " pd").
            // Testing `t` itself for an alphanumeric start therefore always
            // failed, the alias was skipped, and the parser went on to read
            // `as` as a second module name — leaving "as pd" unconsumed, which
            // truncated the file at its first line. Trim first, and require the
            // separating whitespace so `import a asb` is not misread as an alias.
            let t2 = t.trim_start();
            if t.len() > t2.len()
                && t2.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            {
                let (r, a) = ws(parse_ident).parse(t2).map_err(|_| {
                    nom::Err::Error(NomError::new(start, nom::error::ErrorKind::Tag))
                })?;
                (r, a)
            } else {
                (trimmed, default_module_alias(&module))
            }
        } else {
            (trimmed, default_module_alias(&module))
        };
        out.push(py_import_marker("zeta_py_import", vec![&module, &alias]));
        let r = after_alias.trim_start();
        if let Some(t) = r.strip_prefix(',') {
            cur = t;
        } else {
            cur = r;
            break;
        }
    }
    // No trailing-`;` consumption here on purpose: a bare `;` is a top-level
    // EMPTY statement (top_level::parse_top_level_entry, batch 339), so this
    // rule does not have to eat its own terminator to keep the caller's `many0`
    // from stopping. Batch 338 added that eaten `;` as the only defence against
    // dropping the rest of the file; keeping it would put the same rule in two
    // places.
    Ok((cur, AstNode::Block { body: out }))
}

/// Last dotted component: `import concurrent.futures` binds `futures`.
fn default_module_alias(module: &str) -> String {
    module.rsplit('.').next().unwrap_or(module).to_string()
}

fn py_import_marker(method: &str, args: Vec<&str>) -> AstNode {
    AstNode::ExprStmt {
        expr: Box::new(AstNode::Call {
            receiver: None,
            method: method.to_string(),
            args: args.into_iter().map(|a| AstNode::StringLit(a.to_string())).collect(),
            type_args: vec![],
            structural: false,
        }),
    }
}

/// PY-A: `from .mod import y` — a relative module specifier. One or more
/// leading dots give the package level; an optional dotted name follows.
/// The dots are kept in the returned string (`.mod`, `..pkg.mod`, `.`) so the
/// resolver can resolve the name against the importing module's package.
fn parse_relative_module(input: &str) -> IResult<&str, String> {
    let mut cur = input;
    let mut dots = 0usize;
    while let Some(t) = cur.strip_prefix('.') {
        dots += 1;
        cur = t;
    }
    if dots == 0 {
        return parse_dotted_name(input);
    }
    let mut name = ".".repeat(dots);
    if cur.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
        let (r, first) = parse_ident(cur)?;
        name.push_str(&first);
        cur = r;
        while let Some(t) = cur.strip_prefix('.') {
            if t.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                let (r2, seg) = parse_ident(t)?;
                name.push('.');
                name.push_str(&seg);
                cur = r2;
            } else {
                break;
            }
        }
    }
    Ok((cur, name))
}

/// `a.b.c` — dotted module path.
fn parse_dotted_name(input: &str) -> IResult<&str, String> {    let (input, first) = parse_ident(input)?;
    let mut name = first;
    let mut cur = input;
    while let Some(t) = cur.strip_prefix('.') {
        if t.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
            let (r, seg) = parse_ident(t)?;
            name.push('.');
            name.push_str(&seg);
            cur = r;
        } else {
            break;
        }
    }
    Ok((cur, name))
}


/// PY-A: drop `#` comments from an import member list (`from x import (a,  # c
/// b)`). Import lists contain no string literals, so a plain cut at `#` is safe.
fn strip_py_line_comments(s: &str) -> String {
    s.lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// PY-A: `from x import y[, z [as w]]` — emits `zeta_py_from("module",
/// "member", "alias")` markers binding the member into scope. Modules and
/// members are validated against the Python-library registry.
fn parse_python_from_import(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = tag("from").parse(input)?;
    if input
        .chars()
        .next()
        .map_or(false, |c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    let (input, _) = take_while(|c: char| c == ' ' || c == '\t' || c == '\r')(input)?;
    // `from` must be followed by a (possibly relative) dotted module name
    // then `import`
    let (input, module) = ws(parse_relative_module).parse(input)?;
    let (input, _) = ws(tag("import")).parse(input)?;
    // The member list may be PARENTHESIZED and span multiple lines — real
    // Python does this constantly:
    //     from strategies.code.jq_shim import (
    //         OrderCost,
    //         g,
    //     )
    // Taking only the rest of the line (the old behaviour) left the members
    // unparsed, which failed the enclosing block and (via `many0`) silently
    // dropped the rest of the file.
    let (input, _) = take_while(|c: char| c == ' ' || c == '\t' || c == '\r')(input)?;
    let (tail, input) = if let Some(inner) = input.strip_prefix('(') {
        // Imports have no nested parens, so the first `)` closes the list.
        match inner.find(')') {
            Some(idx) => (
                strip_py_line_comments(&inner[..idx]),
                &inner[idx + 1..],
            ),
            None => (strip_py_line_comments(inner), ""),
        }
    } else {
        let (rest, line) = take_while(|c: char| c != '\n' && c != '\r')(input)?;
        (strip_py_line_comments(line), rest)
    };
    let tail: &str = &tail;
    let mut out: Vec<AstNode> = Vec::new();
    // Star-import: `from X import *` binds the module's public top-level
    // names into scope. A distinct marker tells the resolver to enumerate
    // them (the plain import marker binds nothing).
    if tail.trim_start().starts_with('*') {
        out.push(py_import_marker("zeta_py_star", vec![&module]));
        return Ok((input, AstNode::Block { body: out }));
    }
    let mut cur = tail;
    loop {
        let (after_member, member) = match ws(parse_ident).parse(cur) {
            Ok(v) => v,
            Err(_) => break,
        };
        let trimmed = after_member.trim_start();
        // `ws(parse_ident)` above ate the whitespace before `as`, so `t` still
        // carries the separator space (`t == " alias"`) and testing `t` itself
        // for an alphanumeric start always failed — `from x import y as z`
        // silently dropped the alias. Trim first (same bug as in
        // `parse_python_import`), and require the separating whitespace.
        let (after_alias, alias) = if let Some(t) = trimmed.strip_prefix("as") {
            let t2 = t.trim_start();
            if t.len() > t2.len()
                && t2.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            {
                let (r, a) = ws(parse_ident).parse(t2).map_err(|_| {
                    nom::Err::Error(NomError::new(input, nom::error::ErrorKind::Tag))
                })?;
                (r, a)
            } else {
                (trimmed, member.clone())
            }
        } else {
            (trimmed, member.clone())
        };
        out.push(py_import_marker(
            "zeta_py_from",
            vec![&module, &member, &alias],
        ));
        let r = after_alias.trim_start();
        if let Some(t) = r.strip_prefix(',') {
            cur = t;
        } else {
            break;
        }
    }
    Ok((input, AstNode::Block { body: out }))
}

/// PY-A: `try/except/finally` — desugars (parser-only, no AST change) into:
///   zeta_try_enter()
/// Does this statement list fall through to the next statement, or does it
/// definitely transfer control away (return/break/continue)?
///
/// The try/except desugaring appends `zeta_try_end()` to a branch, and a call
/// is not a terminator — appending it after a `return`/`break`/`continue`
/// produced a basic block with instructions after its terminator:
/// "Terminator found in the middle of a basic block". This kept 6 corpus files
/// from compiling at all. Skipping the call on a branch that cannot fall
/// through is correct: control left the try region anyway.
fn branch_falls_through(body: &[AstNode]) -> bool {
    match body.last() {
        None => true,
        Some(AstNode::Return(_) | AstNode::Break(_) | AstNode::Continue(_)) => false,
        // Nested block: fall through iff its body falls through.
        Some(AstNode::Block { body: inner }) => branch_falls_through(inner),
        // `if c: return … else: return …` — both arms leave, so does the if.
        // Empty else: the false path falls through to the next stmt.
        Some(AstNode::If { then, else_, .. }) => {
            if else_.is_empty() {
                true
            } else {
                branch_falls_through(then) || branch_falls_through(else_)
            }
        }
        _ => true,
    }
}

/// PY-A: exit call for the with-desugar (see `parse_with`).
fn zeta_with_exit_stmt(ctx: &str) -> AstNode {
    AstNode::ExprStmt {
        expr: Box::new(AstNode::Call {
            receiver: None,
            method: "zeta_with_exit".to_string(),
            args: vec![AstNode::Var(ctx.to_string())],
            type_args: vec![],
            structural: false,
        }),
    }
}

/// PY-A: rewrite terminators inside a `with` body so __exit__ runs before
/// the block is left. `return v` (always leaves the with) becomes
/// `{ __with_ret_N = v; exit(); return __with_ret_N }` — in-place insertion
/// keeps if/loop arm control flow intact. `break`/`continue` only leave the
/// with when they bind to a loop OUTSIDE it, so `at_loop_depth` (true while
/// the nearest enclosing loop is still the with's own block) gates whether
/// an exit is prepended; descending into a loop body clears the flag.
fn rewrite_with_exits(body: Vec<AstNode>, ctx: &str, seq: usize, at_loop_depth: bool) -> Vec<AstNode> {
    let mut out: Vec<AstNode> = Vec::with_capacity(body.len());
    for stmt in body {
        match stmt {
            AstNode::Return(v) => {
                let ret_var = format!("__with_ret_{}", seq);
                out.push(AstNode::Assign(
                    Box::new(AstNode::Var(ret_var.clone())),
                    v,
                ));
                out.push(zeta_with_exit_stmt(ctx));
                out.push(AstNode::Return(Box::new(AstNode::Var(ret_var))));
            }
            AstNode::Break(opt) if at_loop_depth => {
                out.push(zeta_with_exit_stmt(ctx));
                out.push(AstNode::Break(opt));
            }
            AstNode::Continue(opt) if at_loop_depth => {
                out.push(zeta_with_exit_stmt(ctx));
                out.push(AstNode::Continue(opt));
            }
            AstNode::If { cond, then, else_ } => out.push(AstNode::If {
                cond,
                then: rewrite_with_exits(then, ctx, seq, at_loop_depth),
                else_: rewrite_with_exits(else_, ctx, seq, at_loop_depth),
            }),
            AstNode::IfLet { pattern, expr, then, else_ } => out.push(AstNode::IfLet {
                pattern,
                expr,
                then: rewrite_with_exits(then, ctx, seq, at_loop_depth),
                else_: rewrite_with_exits(else_, ctx, seq, at_loop_depth),
            }),
            AstNode::Block { body: inner } => {
                out.push(AstNode::Block { body: rewrite_with_exits(inner, ctx, seq, at_loop_depth) })
            }
            AstNode::Loop { body } => out.push(AstNode::Loop {
                body: rewrite_with_exits(body, ctx, seq, false),
            }),
            AstNode::For { pattern, expr, body, else_body } => out.push(AstNode::For {
                pattern,
                expr,
                body: rewrite_with_exits(body, ctx, seq, false),
                else_body: rewrite_with_exits(else_body, ctx, seq, at_loop_depth),
            }),
            AstNode::While { cond, body, else_body } => out.push(AstNode::While {
                cond,
                body: rewrite_with_exits(body, ctx, seq, false),
                else_body: rewrite_with_exits(else_body, ctx, seq, at_loop_depth),
            }),
            other => out.push(other),
        }
    }
    out
}

///   if zeta_try_setjmp() == 0 { body; zeta_try_end() }
///   else { [e = zeta_last_error();] handler; zeta_try_end() }
///   [finally body]
/// The runtime longjmps from `raise` back into zeta_try_setjmp's frame.
/// V1: the first except handler catches everything (error type ignored);
/// multiple except clauses beyond the first are consumed and ignored.
fn parse_try_stmt(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("try")).parse(input)?;
    let (input, _) = ws(tag("{")).parse(input)?;
    let (input, body) = parse_block_body(input)?;
    let (input, _) = ws(tag("}")).parse(input)?;

    let mut handler: Vec<AstNode> = Vec::new();
    let mut as_var: Option<String> = None;
    let mut saw_except = false;
    let mut cur = input;
    loop {
        // After preprocessing, each except header reads `except ... {`.
        let t = cur.trim_start();
        if !t.starts_with("except") {
            break;
        }
        let after_kw = &t[6..];
        if after_kw
            .chars()
            .next()
            .map_or(false, |c| c.is_ascii_alphanumeric() || c == '_')
        {
            break;
        }
        saw_except = true;
        // Everything between `except` and the body `{` is the header
        // (optional error type + optional `as var`)
        let brace_rel = after_kw.find('{').unwrap_or(after_kw.len());
        let header = after_kw[..brace_rel].trim();
        if as_var.is_none() {
            // `except as e` / `except ValueError as e` / bare `except`
            if let Some((_, name)) = header.rsplit_once(" as ") {
                as_var = Some(name.trim().to_string());
            } else if let Some(rest) = header.strip_prefix("as ") {
                as_var = Some(rest.trim().to_string());
            } else if let Some((_, name)) = header.rsplit_once(" as") {
                as_var = Some(name.trim().to_string());
            }
        }
        let off_in_cur = cur.len() - t.len() + 6 + brace_rel;
        cur = &cur[off_in_cur..];
        let (next, _) = ws(tag("{")).parse(cur)?;
        let (next, hbody) = parse_block_body(next)?;
        let (next, _) = ws(tag("}")).parse(next)?;
        if handler.is_empty() {
            handler = hbody;
        }
        cur = next;
    }
    // optional finally
    //
    // NOTE the ordering: a bare `try/finally` (NO `except`) must be accepted
    // too. Rejecting it here used to `Err` the whole `try` statement, and the
    // caller's fallback then parsed `finally { … }` as a PLAIN BLOCK — its
    // statements leaked into the enclosing body *after* the try body. With
    // `try: … return X` + `finally: cleanup()` that put instructions after a
    // `ret` in the same basic block: "Terminator found in the middle of a basic
    // block", which aborted the WHOLE compile
    // (`backend/datasrc/market_data_universe.py`). The `saw_except` check moved
    // below the finally parse so `try/finally` is a first-class form.
    let mut finally_body: Vec<AstNode> = Vec::new();
    let t = cur.trim_start();
    if t.starts_with("finally") {
        let after = &t[7..];
        if !after
            .chars()
            .next()
            .map_or(true, |c| c.is_ascii_alphanumeric() || c == '_')
        {
            let brace_off = after.find('{').unwrap_or(0);
            let off_in_cur = cur.len() - cur.trim_start().len() + 7 + brace_off;
            cur = &cur[off_in_cur..];
            let (next, _) = ws(tag("{")).parse(cur)?;
            let (next, fbody) = parse_block_body(next)?;
            let (next, _) = ws(tag("}")).parse(next)?;
            finally_body = fbody;
            cur = next;
        }
    }
    if !saw_except && finally_body.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }

    // ── desugar (error-state polling — no setjmp) ──
    // raise sets a global error; every body statement is guarded by
    // `if zeta_last_error() == 0` so the rest of the body is skipped;
    // the trailing `if err != 0` runs the handler (first statement clears
    // the error so outer scopes continue normally). finally runs after.
    let mk_call = |name: &str| AstNode::ExprStmt {
        expr: Box::new(AstNode::Call {
            receiver: None,
            method: name.to_string(),
            args: vec![],
            type_args: vec![],
            structural: false,
        }),
    };
    let err_eq_zero = AstNode::BinaryOp {
        op: "==".to_string(),
        left: Box::new(AstNode::Call {
            receiver: None,
            method: "zeta_last_error".to_string(),
            args: vec![],
            type_args: vec![],
            structural: false,
        }),
        right: Box::new(AstNode::Lit(0)),
    };
    let err_ne_zero = AstNode::BinaryOp {
        op: "!=".to_string(),
        left: Box::new(AstNode::Call {
            receiver: None,
            method: "zeta_last_error".to_string(),
            args: vec![],
            type_args: vec![],
            structural: false,
        }),
        right: Box::new(AstNode::Lit(0)),
    };

    // setjmp form: `raise` longjmps IMMEDIATELY (real exception semantics)
    let mut out = vec![mk_call("zeta_try_enter")];
    let setjmp_call = AstNode::Call {
        receiver: None,
        method: "zeta_try_setjmp".to_string(),
        args: vec![],
        type_args: vec![],
        structural: false,
    };
    // ALWAYS pop the try frame:
    //  - in the HANDLER branch it must happen FIRST (the longjmp already restored
    //    the stack; the frame's job is done), otherwise `except ...: return {}`
    //    leaked the frame — `ParquetCache.load_metadata` did exactly that and the
    //    NEXT `raise` longjmped into a stale frame, so the caller continued into
    //    `t.schema.metadata` with t == 0 (SEGV at load_metadata + 184).
    //  - at the end of the body branch (fall-through).
    let mut then_branch = body;
    // Only when the body FALLS THROUGH: appending a call after a `raise`/`return`
    // puts a terminator in the middle of a basic block (the backend rejects it).
    if branch_falls_through(&then_branch) {
        then_branch.push(mk_call("zeta_try_end"));
    }
    let mut else_branch: Vec<AstNode> = Vec::new();
    if let Some(v) = as_var {
        else_branch.push(AstNode::Assign(
            Box::new(AstNode::Var(v)),
            Box::new(AstNode::Call {
                receiver: None,
                method: "zeta_last_error".to_string(),
                args: vec![],
                type_args: vec![],
                structural: false,
            }),
        ));
    }
    // Pop the frame AFTER capturing the error value (`zeta_last_error()` reads the
    // top frame) and BEFORE the handler body — otherwise `except ... as e` saw the
    // OUTER frame's code. Keeping it unpopped leaked the frame, so a later `raise`
    // longjmped into a dead frame (SEGV in load_metadata).
    else_branch.push(mk_call("zeta_try_end"));
    else_branch.extend(handler);
    out.push(AstNode::If {
        cond: Box::new(AstNode::BinaryOp {
            op: "==".to_string(),
            left: Box::new(setjmp_call),
            right: Box::new(AstNode::Lit(0)),
        }),
        then: then_branch,
        else_: else_branch,
    });
    out.extend(finally_body);
    Ok((cur, AstNode::Block { body: out }))
}


/// PY-A: `global a, b` — same env routing as nonlocal (the name's storage
/// lives in the shared env, visible across functions). Shares the
/// zeta_nonlocal_decl marker so Resolver/gen routes reuse the V3 path.
fn parse_global(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("global")).parse(input)?;
    let mut out: Vec<AstNode> = Vec::new();
    let mut cur = input;
    loop {
        let (rest, name) = ws(parse_ident).parse(cur)?;
        out.push(AstNode::ExprStmt {
            expr: Box::new(AstNode::Call {
                receiver: None,
                method: "zeta_nonlocal_decl".to_string(),
                args: vec![AstNode::StringLit(name)],
                type_args: vec![],
                structural: false,
            }),
        });
        let rest2 = skip_ws_and_comments(rest).map(|(i, _)| i).unwrap_or(rest);
        if rest2.starts_with(',') {
            cur = &rest2[1..];
        } else {
            cur = rest2;
            break;
        }
    }
    let (input, _) = opt(ws(tag(";"))).parse(cur)?;
    Ok((
        input,
        AstNode::Block { body: out },
    ))
}

/// PY-A: `nonlocal a, b` — declares capture-by-reference of outer-scope
/// names. Lowered to a `zeta_nonlocal_decl("a")` call per name; MIR lowering
/// uses these to route the name's reads/writes through the closure env.
fn parse_nonlocal(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("nonlocal")).parse(input)?;
    let mut out: Vec<AstNode> = Vec::new();
    let mut cur = input;
    loop {
        let (rest, name) = ws(parse_ident).parse(cur)?;
        out.push(AstNode::ExprStmt {
            expr: Box::new(AstNode::Call {
                receiver: None,
                method: "zeta_nonlocal_decl".to_string(),
                args: vec![AstNode::StringLit(name)],
                type_args: vec![],
                structural: false,
            }),
        });
        let rest2 = skip_ws_and_comments(rest).map(|(i, _)| i).unwrap_or(rest);
        if rest2.starts_with(',') {
            cur = &rest2[1..];
        } else {
            cur = rest2;
            break;
        }
    }
    let (input, _) = opt(ws(tag(";"))).parse(cur)?;
    Ok((
        input,
        AstNode::Block { body: out },
    ))
}

/// PY-A: `with EXPR as NAME:` — context-manager sugar. V1 desugar: evaluate
/// EXPR, bind NAME (optional), run body. No __enter__/__exit__ protocol yet
/// (documented limit; real resource management needs the runtime protocol).
fn parse_with(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("with")).parse(input)?;
    // Header text runs to the body `{` (preprocessed). Split `as NAME` off
    // manually — parse_full_expr would consume `as a` as a Cast.
    let brace = input.find('{').ok_or_else(|| {
        nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag))
    })?;
    let header = input[..brace].trim().to_string();
    let (mgr_text, as_name) = match header.rsplit_once(" as ") {
        Some((m, n)) => (m.trim().to_string(), Some(n.trim().to_string())),
        None => (header.clone(), None),
    };
    // Parse the manager expression properly from mgr_text.
    // parse_full_expr's rest borrows mgr_text, so we only keep the node.
    let parsed = parse_full_expr(mgr_text.as_str());
    match parsed {
        Ok((mgr_rest, mgr)) if mgr_rest.trim().is_empty() => {
            let input = &input[brace..];
            let (input, _) = ws(tag("{")).parse(input)?;
            let (input, body) = parse_block_body(input)?;
            let (input, _) = ws(tag("}")).parse(input)?;

            // PY-A: `with X [as n]:` runs the context protocol. Desugars to:
            //   let __with_ctx<N> = X
            //   [n =] zeta_with_enter(__with_ctx<N>)
            //   body  (each `return` rewritten to `__exit__; return`)
            //   zeta_with_exit(...)   // only if body can fall through
            // Appending exit after a `return` produced "Terminator found in
            // the middle of a basic block" and aborted modules that use
            // `with lock: return …` (sources_selector / market_data_sources).
            static WITH_SEQ: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let seq = WITH_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let ctx = format!("__with_ctx_{}", seq);
            let mut stmts: Vec<AstNode> = Vec::new();
            stmts.push(AstNode::Assign(
                Box::new(AstNode::Var(ctx.clone())),
                Box::new(mgr),
            ));
            let enter = AstNode::Call {
                receiver: None,
                method: "zeta_with_enter".to_string(),
                args: vec![AstNode::Var(ctx.clone())],
                type_args: vec![],
                structural: false,
            };
            match as_name {
                Some(name) => stmts.push(AstNode::Assign(
                    Box::new(AstNode::Var(name)),
                    Box::new(enter),
                )),
                None => stmts.push(AstNode::ExprStmt {
                    expr: Box::new(enter),
                }),
            }
            let exit = AstNode::ExprStmt {
                expr: Box::new(AstNode::Call {
                    receiver: None,
                    method: "zeta_with_exit".to_string(),
                    args: vec![AstNode::Var(ctx.clone())],
                    type_args: vec![],
                    structural: false,
                }),
            };
            // PY-A: early terminators inside the body must still run __exit__.
            // `with lock: return …` (sources_selector._load_source_stats) kept
            // the mutex forever and deadlocked the next acquirer (measured:
            // hang in py_threading_lock_acquire). Rewrite each `return` in
            // place to `__with_ret_N = v; exit(); return __with_ret_N`, and
            // prepend a bare `exit()` to break/continue that bind to an
            // enclosing loop of the WITH (not to a loop inside the body —
            // that flag resets once we descend into a loop body).
            let falls = branch_falls_through(&body);
            let body = rewrite_with_exits(body, &ctx, seq, true);
            stmts.extend(body);
            if falls {
                stmts.push(exit);
            }
            Ok((input, AstNode::Block { body: stmts }))
        }
        _ => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        ))),
    }
}

fn parse_full_expr_as_target(input: &str) -> IResult<&str, AstNode> {
    parse_full_expr(input)
}
/// PY-A: `raise(expr)` → zeta_raise(expr) — longjmps to the innermost try
fn parse_raise(input: &str) -> IResult<&str, AstNode> {
    // PY-A: nonlocal/global/with dispatch here to stay under nom's alt tuple
    // arity cap
    if let Ok(r) = parse_global(input) {
        return Ok(r);
    }
    if let Ok(r) = parse_nonlocal(input) {
        return Ok(r);
    }
    if let Ok(r) = parse_with(input) {
        return Ok(r);
    }
    if let Ok(r) = parse_with(input) {
        return Ok(r);
    }
    let (input, _) = ws(terminated(
        tag("raise"),
        peek(none_of("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_")),
    ))
    .parse(input)?;
    // Two forms: Zeta's `raise(expr)` and Python's bare `raise Expr`
    // (e.g. `raise ValueError("boom")`). The bare form previously fell through
    // to a stray `raise` variable, so the exception was never raised and the
    // statements after it were dropped.
    let same_line = {
        let t = input.trim_start_matches(|c| c == ' ' || c == '\t');
        !t.is_empty() && !t.starts_with('\n') && !t.starts_with('\r')
    };
    let (input, arg) = match ws(tag::<_, _, NomError<&str>>("(")).parse(input) {
        Ok((rest, _)) => {
            let (rest, a) = opt(ws(parse_full_expr)).parse(rest)?;
            let (rest, _) = ws(tag(")")).parse(rest)?;
            (rest, a.unwrap_or(AstNode::Lit(0)))
        }
        Err(_) if same_line => match ws(parse_full_expr).parse(input) {
            Ok((rest, e)) => (rest, e),
            Err(_) => (input, AstNode::Lit(0)),
        },
        Err(_) => (input, AstNode::Lit(0)),
    };
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    Ok((
        input,
        AstNode::ExprStmt {
            expr: Box::new(AstNode::Call {
                receiver: None,
                method: "zeta_raise".to_string(),
                args: vec![arg],
                type_args: vec![],
                structural: false,
            }),
        },
    ))
}

pub fn parse_stmt(input: &str) -> IResult<&str, AstNode> {
    alt((
        parse_return,
        parse_break,
        parse_continue,
        parse_if_let,
        parse_if,
        parse_for,
        parse_loop,
        parse_while,
        parse_unsafe,
        parse_match_expr,
        parse_let,
        parse_assign,
        parse_type_alias,
        parse_const,
        // PY-A: a `class` inside a function body. `parse_class` was only wired
        // into the top-level definitions list, so a nested class was SWALLOWED
        // by the statement path: no node, no parse error — the synthesized
        // constructor never existed and `P(n)` produced garbage.
        // (nom's `alt` takes at most 21 alternatives in one tuple, hence the
        // nested alt instead of a 22nd entry.)
        alt((parse_func, parse_class)),
        parse_python_from_import,
        parse_python_import,
        parse_try_stmt,
        parse_raise,
        // PY-A: pass/del/assert/ellipsis nest under one alt arm (nom 21-arm limit).
        alt((parse_pass, parse_ellipsis_stmt, parse_del, parse_assert)),
        parse_expr_stmt,
    ))
    .parse(input)
}
