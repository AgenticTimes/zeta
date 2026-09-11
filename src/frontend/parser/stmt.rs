// src/frontend/parser/stmt.rs
//! Module for parsing statements in the Zeta language.

use super::expr::{parse_condition, parse_full_expr, parse_match_expr};
use super::parser::{parse_type, skip_ws_and_comments, ws};
use super::pattern::parse_pattern;
use super::top_level::{parse_const, parse_func, parse_type_alias};
use crate::frontend::ast::AstNode;
use nom::IResult;
use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::{tag, take_while};
use nom::combinator::{map, opt, peek};
use nom::error::Error as NomError;
use nom::sequence::{delimited, preceded};

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
            body.push(AstNode::ExprStmt {
                expr: Box::new(expr),
            });
            current = next_expr;
            continue;
        }

        return Err(nom::Err::Error(NomError::new(
            next,
            nom::error::ErrorKind::Many0,
        )));
    }
    Ok((current, body))
}

fn parse_let(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("let")).parse(input)?;
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
    let (input, pattern) = ws(parse_pattern).parse(input)?;
    let (input, _) = ws(tag("in")).parse(input)?;
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
    Ok((
        input,
        AstNode::For {
            pattern: Box::new(pattern),
            expr: Box::new(expr),
            body,
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
    Ok((
        input,
        AstNode::While {
            cond: Box::new(cond),
            body,
        },
    ))
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

    // PY-A: `if __name__ == "__main__":` — unwrap the guard so the body
    // always runs (module main detection has no runtime meaning here).
    {
        let is_name = |n: &AstNode| matches!(n, AstNode::Var(v) if v == "__name__");
        let is_main = |n: &AstNode| matches!(n, AstNode::StringLit(s) if s == "__main__");
        if let AstNode::BinaryOp { op, left, right } = &cond {
            if (op == "==" || op == "is")
                && ((is_name(left) && is_main(right)) || (is_main(left) && is_name(right)))
            {
                return Ok((input, AstNode::Block { body: then }));
            }
        }
    }

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
        Ok((input, AstNode::Assign(Box::new(lhs), Box::new(rhs))))
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

pub fn parse_return(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("return")).parse(input)?;
    let (input, inner) = opt(ws(parse_full_expr)).parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    if let Some(expr) = inner {
        Ok((input, AstNode::Return(Box::new(expr))))
    } else {
        // return; without expression - use Lit(0) as default
        Ok((input, AstNode::Return(Box::new(AstNode::Lit(0)))))
    }
}

fn parse_break(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("break")).parse(input)?;
    let (input, expr_opt) = opt(ws(parse_full_expr)).parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    Ok((input, AstNode::Break(expr_opt.map(Box::new))))
}

fn parse_continue(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("continue")).parse(input)?;
    let (input, expr_opt) = opt(ws(parse_full_expr)).parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    Ok((input, AstNode::Continue(expr_opt.map(Box::new))))
}

fn parse_expr_stmt(input: &str) -> IResult<&str, AstNode> {
    let (input, expr) = parse_full_expr(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    Ok((
        input,
        AstNode::ExprStmt {
            expr: Box::new(expr),
        },
    ))
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

/// PY-A: Python `import x[.y as z]` / `from x import y` — consumed and
/// ignored in V1 so real Python files parse; module mapping is a later item.
fn parse_python_import(input: &str) -> IResult<&str, AstNode> {
    let start = input;
    let (input, _) = ws(tag("import")).parse(input)?;
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
    let (input, _) = take_while(|c| c != '\n' && c != '\r')(input)?;
    Ok((
        input,
        AstNode::ExprStmt {
            expr: Box::new(AstNode::Lit(0)),
        },
    ))
}

/// PY-A: `from x import y[, z]` — consumed and ignored (V1).
fn parse_python_from_import(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("from")).parse(input)?;
    // `from` must be followed by a dotted module name then `import`
    let (input, _) = ws(take_while(|c: char| {
        c.is_ascii_alphanumeric() || c == '_' || c == '.'
    }))
    .parse(input)?;
    let (input, _) = ws(tag("import")).parse(input)?;
    let (input, _) = take_while(|c| c != '\n' && c != '\r')(input)?;
    Ok((
        input,
        AstNode::ExprStmt {
            expr: Box::new(AstNode::Lit(0)),
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
        parse_func,
        parse_python_from_import,
        parse_python_import,
        parse_pass,
        parse_expr_stmt,
    ))
    .parse(input)
}
