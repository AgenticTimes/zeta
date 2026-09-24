// src/frontend/parser/expr.rs
use super::parser::{
    parse_ident, parse_member_ident, parse_path, parse_type, parse_type_args, skip_ws_and_comments0, ws,
};

use super::pattern::parse_pattern;
use super::stmt::{parse_assign, parse_block_body, parse_loop, parse_return, parse_return_single};
use crate::frontend::ast::{AstNode, MatchArm};
use nom::IResult;
use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::character::complete::none_of;
use nom::combinator::{map, opt, peek};
use nom::error::Error as NomError;
use nom::multi::{separated_list0, separated_list1};
use nom::sequence::{delimited, pair, preceded, terminated};

/// `as` cast keyword with a word-boundary guard. Without it, the `as` inside
/// `async def ...` matches and the cast's type name swallows `ync`, silently
/// corrupting the preceding statement (and breaking every `async def`).
fn cast_as_keyword(input: &str) -> IResult<&str, &str> {
    // Only leading whitespace may be skipped here: `ws` also eats trailing
    // whitespace, which would make the boundary test below see the type name
    // and reject every real `x as i64`.
    let (i, _) = skip_ws_and_comments0(input)?;
    let (i, kw) = tag("as").parse(i)?;
    if i.chars()
        .next()
        .map_or(false, |c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(nom::Err::Error(NomError::new(
            i,
            nom::error::ErrorKind::Tag,
        )));
    }
    Ok((i, kw))
}


fn parse_float_lit(input: &str) -> IResult<&str, AstNode> {
    // OPTIMIZED: Use byte slices instead of character iteration
    // Simple float parser: digits.digits
    // For v0.3.8, support basic floats without exponent

    let bytes = input.as_bytes();
    let mut pos = 0;
    let mut has_digit = false;

    // Parse integer part (digits and underscores)
    let start_pos = pos;
    while pos < bytes.len() && (bytes[pos].is_ascii_digit() || bytes[pos] == b'_') {
        if bytes[pos].is_ascii_digit() {
            has_digit = true;
        }
        pos += 1;
    }

    if !has_digit {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Digit,
        )));
    }

    // Check for decimal point
    let mut has_decimal = false;
    if pos < bytes.len() && bytes[pos] == b'.' {
        // Parse fractional part (at least one digit)
        has_decimal = true;
        pos += 1; // Skip the decimal point

        // Parse fraction part (digits after decimal, with underscores)
        let fraction_start = pos;
        let mut fraction_has_digit = false;
        while pos < bytes.len() && (bytes[pos].is_ascii_digit() || bytes[pos] == b'_') {
            if bytes[pos].is_ascii_digit() {
                fraction_has_digit = true;
            }
            pos += 1;
        }

        if !fraction_has_digit {
            // Invalid: decimal point without digits
            return Err(nom::Err::Error(NomError::new(
                input,
                nom::error::ErrorKind::Digit,
            )));
        }
    }

    // Exponent: `1e8`, `1.5e-3`, `2E+10`. Consumed only when digits really
    // follow the marker, so `1e` + identifier stays `1` + `e`. Without this the
    // literal was silently truncated: `v = 1e8` compiled to `v = 1` (the `e8`
    // became a stray identifier statement) and `f(1e8)` broke the enclosing
    // call outright.
    let mut has_exponent = false;
    if pos < bytes.len() && (bytes[pos] == b'e' || bytes[pos] == b'E') {
        let mut p = pos + 1;
        if p < bytes.len() && (bytes[p] == b'+' || bytes[p] == b'-') {
            p += 1;
        }
        let digits_start = p;
        while p < bytes.len() && (bytes[p].is_ascii_digit() || bytes[p] == b'_') {
            p += 1;
        }
        if input[digits_start..p].bytes().any(|b| b.is_ascii_digit()) {
            has_exponent = true;
            pos = p;
        }
    }

    // Must have a decimal point or an exponent to be a float.
    if !has_decimal && !has_exponent {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Digit,
        )));
    }

    let float_str = &input[..pos];
    let remaining = &input[pos..];

    // Remove underscores from the float string, keeping the exponent marker and
    // its sign (`f64::from_str` understands `1e-8`).
    let clean_float: String = float_str
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
        .collect();

    // Consume optional type suffix (e.g. `3.14f64`) so `f(3.14f64)` parses
    // like `f(3.14)`. Mirrors the integer-suffix handling in parse_lit.
    let after_lit = remaining;
    let suffix = ["f32", "f64"]
        .iter()
        .find(|s| after_lit.starts_with(**s));
    let remaining = match suffix {
        Some(s) => &after_lit[s.len()..],
        None => after_lit,
    };

    Ok((remaining, AstNode::FloatLit(clean_float)))
}

pub fn parse_lit(input: &str) -> IResult<&str, AstNode> {
    // Try float first, then integer
    match parse_float_lit(input) {
        Ok(result) => {
            return Ok(result);
        }
        Err(_) => {
            // Not a float, try integer
        }
    }

    // Parse integer with optional underscores
    let bytes = input.as_bytes();

    // Check for hex literal (0x/0X), octal (0o/0O), or binary (0b/0B)
    if bytes.len() >= 2 && bytes[0] == b'0' {
        let prefix = bytes[1];
        if prefix == b'x'
            || prefix == b'X'
            || prefix == b'o'
            || prefix == b'O'
            || prefix == b'b'
            || prefix == b'B'
        {
            let (radix, valid_chars): (u32, fn(u8) -> bool) = match prefix {
                b'x' | b'X' => (16, |c: u8| c.is_ascii_hexdigit() || c == b'_'),
                b'o' | b'O' => (8, |c: u8| (b'0'..=b'7').contains(&c) || c == b'_'),
                b'b' | b'B' => (2, |c: u8| c == b'0' || c == b'1' || c == b'_'),
                _ => unreachable!(),
            };
            let mut pos = 2;
            let mut has_digit = false;
            while pos < bytes.len() && valid_chars(bytes[pos]) {
                if bytes[pos] != b'_' {
                    has_digit = true;
                }
                pos += 1;
            }
            if !has_digit {
                return Err(nom::Err::Error(NomError::new(
                    input,
                    nom::error::ErrorKind::Digit,
                )));
            }
            let num_str = &input[..pos];
            let remaining = &input[pos..];
            // Remove underscores and parse with appropriate radix
            let clean: String = num_str[2..].chars().filter(|c| *c != '_').collect();
            let value = u64::from_str_radix(&clean, radix).unwrap_or(0) as i64;
            return Ok((remaining, AstNode::Lit(value)));
        }
    }

    // Regular decimal integer
    let mut pos = 0;
    let mut has_digit = false;

    // Parse digits and underscores
    while pos < bytes.len() && (bytes[pos].is_ascii_digit() || bytes[pos] == b'_') {
        if bytes[pos].is_ascii_digit() {
            has_digit = true;
        }
        pos += 1;
    }

    if !has_digit {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Digit,
        )));
    }

    let num_str = &input[..pos];
    let remaining = &input[pos..];

    // Remove underscores and parse as i64
    let clean_num: String = num_str.chars().filter(|c| c.is_ascii_digit()).collect();
    let value = clean_num.parse::<i64>().unwrap_or(0);

    // Consume optional type suffix (e.g. `42i64`, `42u32`, `42f64`). The
    // suffix is type annotation only — value stays the same. Without this,
    // `f(42i64)` would leave `i64` unconsumed and the call argument parse
    // would fail, silently dropping `main` from the MIR.
    let after_lit = remaining;
    let suffix = [
        "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64",
        "f32", "f64", "usize", "isize",
    ]
    .iter()
    .find(|s| after_lit.starts_with(**s));
    let remaining = match suffix {
        Some(s) => &after_lit[s.len()..],
        None => after_lit,
    };

    Ok((remaining, AstNode::Lit(value)))
}

fn parse_triple_quoted_string(input: &str) -> IResult<&str, AstNode> {
    // Check for triple quotes: """ or '''
    let (input, quote_char) =
        if let Ok((i, _)) = tag::<_, _, nom::error::Error<_>>("\"\"\"").parse(input) {
            (i, '"')
        } else if let Ok((i, _)) = tag::<_, _, nom::error::Error<_>>("'''").parse(input) {
            (i, '\'')
        } else {
            return Err(nom::Err::Error(NomError::new(
                input,
                nom::error::ErrorKind::Tag,
            )));
        };

    let mut content = String::new();
    let chars = input.chars();
    let mut pos = 0;
    let mut quote_run = 0;

    for c in chars {
        pos += c.len_utf8();
        if c == quote_char {
            quote_run += 1;
            if quote_run >= 3 {
                // Found closing triple quote — trim trailing quote_chars
                let remaining = &input[pos..];
                return Ok((remaining, AstNode::StringLit(content)));
            }
        } else {
            // Flush any accumulated quote chars
            for _ in 0..quote_run {
                content.push(quote_char);
            }
            quote_run = 0;
            content.push(c);
        }
    }
    // Handle trailing quotes (e.g., end-of-file with partial triple quote)
    if quote_run == 3 {
        // Exact match at EOF
        let remaining = &input[pos..];
        return Ok((remaining, AstNode::StringLit(content)));
    }

    Err(nom::Err::Error(NomError::new(
        input,
        nom::error::ErrorKind::Tag,
    )))
}

pub fn parse_string_lit(input: &str) -> IResult<&str, AstNode> {
    // Support both single-quoted and double-quoted strings
    let quote = if let Ok((i, _)) = tag::<_, _, nom::error::Error<_>>("\"").parse(input) {
        (i, '"')
    } else if let Ok((i, _)) = tag::<_, _, nom::error::Error<_>>("'").parse(input) {
        (i, '\'')
    } else {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    };
    let (input, quote_char) = (quote.0, quote.1);

    // Simple parser that handles escaped quotes
    let mut content = String::new();
    let mut chars = input.chars();
    let mut pos = 0;

    while let Some(c) = chars.next() {
        pos += c.len_utf8();

        if c == '\\' {
            // Handle escape
            if let Some(next_c) = chars.next() {
                pos += next_c.len_utf8();
                if next_c == '"' || next_c == '\'' {
                    content.push(next_c);
                } else if next_c == '\\' {
                    content.push('\\');
                } else if next_c == 'n' {
                    content.push('\n');
                } else if next_c == 't' {
                    content.push('\t');
                } else if next_c == 'r' {
                    content.push('\r');
                } else {
                    // Keep both chars for unknown escape
                    content.push('\\');
                    content.push(next_c);
                }
            } else {
                content.push('\\');
            }
        } else if c == quote_char {
            // End of string
            let remaining = &input[pos..];
            return Ok((remaining, AstNode::StringLit(content)));
        } else {
            content.push(c);
        }
    }

    // No closing quote found
    Err(nom::Err::Error(NomError::new(
        input,
        nom::error::ErrorKind::Tag,
    )))
}

/// Parse a raw string literal: `r"..."` / `r'...'` / `r#"..."#` (no escape processing).
///
/// The single-quoted form was missing, and the failure was *silent* in
/// assignment position: `x = r'2|3|4|5'` parsed as `x = r` plus a stray string
/// statement, while the same literal as a call argument
/// (`df['t'].str.contains(r'2|3|4|5')`) broke the call outright — five corpus
/// strategies' `filter_audit`.
///
/// The hash form exists for multi-line source snippets (test_suite.z embeds a
/// whole program in one); it ends at the first `"` followed by exactly as many
/// `#` as opened it.
fn parse_raw_string_lit(input: &str) -> IResult<&str, AstNode> {
    let (after_r, _) = tag::<_, _, nom::error::Error<_>>("r").parse(input)?;
    let hashes = &after_r[..after_r.bytes().take_while(|b| *b == b'#').count()];
    let after_hashes = &after_r[hashes.len()..];

    if !hashes.is_empty() {
        let (body, _) = tag::<_, _, nom::error::Error<_>>("\"").parse(after_hashes)?;
        let close = format!("\"{}", hashes);
        let end = match body.find(&close) {
            Some(i) => i,
            None => {
                return Err(nom::Err::Error(NomError::new(
                    input,
                    nom::error::ErrorKind::Tag,
                )))
            }
        };
        return Ok((
            &body[end + close.len()..],
            AstNode::StringLit(body[..end].to_string()),
        ));
    }

    let (input, quote) = alt((tag("\""), tag("'"))).parse(after_hashes)?;
    // In a raw string a backslash does NOT escape the quote, so the literal
    // ends at the first occurrence of the opening quote character.
    let q = quote.as_bytes()[0] as char;
    let mut content = String::new();
    let mut pos = 0;

    for c in input.chars() {
        pos += c.len_utf8();
        if c == q {
            let remaining = &input[pos..];
            return Ok((remaining, AstNode::StringLit(content)));
        } else {
            content.push(c);
        }
    }
    Err(nom::Err::Error(NomError::new(
        input,
        nom::error::ErrorKind::Tag,
    )))
}

fn parse_path_expr(input: &str) -> IResult<&str, AstNode> {
    // Check for :: at the beginning (special case for error handling)
    let original_input = input;
    let (input, path) = parse_path(input)?;
    if path.is_empty() {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Many0,
        )));
    }

    // Special case: if the original input started with :: and we have a single segment path,
    // and we're about to parse a function call, this is likely ::ident() which is invalid.
    // Note: This is a heuristic and might not catch all cases, but it helps with tests.
    if path.len() == 1 && original_input.starts_with("::") {
        // Check if we're about to parse a function call
        let temp_input = input;
        let (_, type_args_opt) = opt(ws(preceded(opt(tag("::")), parse_type_args)))
            .parse(temp_input)
            .unwrap_or((temp_input, None));
        let type_args: Vec<String> = type_args_opt.unwrap_or_default();

        // Check for :: separator (for static methods)
        let (temp_input_after_type_args, has_coloncolon) =
            match opt(ws(tag("::"))).parse(temp_input) {
                Ok((i, sep)) => (i, sep.is_some()),
                Err(_) => (temp_input, false),
            };

        // Check for parentheses (function call)
        let has_parens = temp_input_after_type_args.trim_start().starts_with("(");

        // If we have :: at the beginning, single segment, and parentheses, reject it
        // This catches cases like ::new() but allows ::std::new()
        if !has_coloncolon && has_parens {
            return Err(nom::Err::Error(NomError::new(
                original_input,
                nom::error::ErrorKind::Tag,
            )));
        }
    }

    let method = path.join("::");

    // Check for macro call: ident! but NOT ident!=
    let (input, is_macro) = if input.starts_with("!") && !input.starts_with("!=") {
        // It's ! for macro, not != operator
        let (input, _) = ws(tag("!")).parse(input)?;
        (input, Some(()))
    } else {
        (input, None)
    };

    if is_macro.is_some() {
        let (input, delim_start) = ws(alt((tag("("), tag("["), tag("{")))).parse(input)?;
        let close = match delim_start {
            "(" => ")",
            "[" => "]",
            "{" => "}",
            _ => {
                return Err(nom::Err::Error(NomError::new(
                    delim_start,
                    nom::error::ErrorKind::Alt,
                )));
            }
        };
        let (input, args) = terminated(
            separated_list0(ws(tag(",")), ws(parse_expr)),
            opt(ws(tag(","))),
        )
        .parse(input)?;
        let (input, _) = ws(tag(close)).parse(input)?;
        Ok((input, AstNode::MacroCall { name: method, args }))
    } else {
        let (input, type_args_opt) = opt(ws(preceded(tag("::"), parse_type_args))).parse(input)?;
        let type_args: Vec<String> = type_args_opt.unwrap_or_default();

        // Check if there's another :: for a method call
        // This can happen in three cases:
        // 1. After type arguments: Vec::<i32>::new()  (type args, then ::method)
        // 2. Direct generic function call: vec_new::<i32>()  (type args, no ::method)
        // 3. In a path without type arguments: Point::new()  (no type args, has ::method)
        let (input, method_name) = if !type_args.is_empty() {
            // Case 1 or 2: Check if there's :: after type arguments
            match opt(ws(tag("::"))).parse(input) {
                Ok((i, Some(_))) => {
                    // Case 1: :: after type arguments, parse method name
                    let (i, name) = parse_ident(i)?;
                    (i, Some(name))
                }
                Ok((i, None)) => {
                    // Case 2: No :: after type arguments, this is a direct generic function call
                    // like vec_new::<i32>()
                    (i, None)
                }
                Err(e) => return Err(e),
            }
        } else {
            // Case 3: Check if this is a path call like Point::new()
            // If path has multiple segments (e.g., ["Point", "new"]), it's already a path call
            // If path has single segment but we have :: followed by identifier, parse it
            let (i, has_sep) = match opt(ws(tag("::"))).parse(input) {
                Ok((i, sep)) => (i, sep.is_some()),
                Err(_) => (input, false),
            };

            if has_sep {
                // Parse method name after ::
                let (i, name) = parse_ident(i)?;
                (i, Some(name))
            } else {
                (input, None)
            }
        };

        // PY-A: kwarg-aware arg list (`OrderCost(a=0)`, `f(x, w=1)`)
        let (input, args_opt) = opt(delimited(
            ws(tag("(")),
            terminated(
                separated_list0(ws(tag(",")), ws(parse_call_arg)),
                opt(ws(tag(","))),
            ),
            ws(tag(")")),
        ))
        .parse(input)?;

        if let Some(args) = args_opt {
            if let Some(method_name) = method_name {
                // This is a call with a method name after ::
                // Could be with or without type arguments
                Ok((
                    input,
                    AstNode::Call {
                        receiver: None,
                        method: method_name,
                        args,
                        type_args,
                        structural: false,
                    },
                ))
            } else if path.len() >= 2 {
                // Split into path and method (no type arguments case, path already has method)
                let (method_path, method_name) = path.split_at(path.len() - 1);
                Ok((
                    input,
                    AstNode::PathCall {
                        path: method_path.to_vec(),
                        method: method_name[0].clone(),
                        args,
                        type_args,
                    },
                ))
            } else {
                // Single segment path, create regular Call
                Ok((
                    input,
                    AstNode::Call {
                        receiver: None,
                        method,
                        args,
                        type_args,
                        structural: false,
                    },
                ))
            }
        } else {
            // No arguments, check for struct literal
            // IMPORTANT: if { ... } is followed by "else", it's an if-then block
            // (not a struct literal), so we must NOT consume the braces.
            // Pre-check: if { is followed by } else, this is definitely a block, not struct
            let is_else_after_brace = {
                let check = input;
                // Quick scan for matching } then check for "else"
                if let Ok((after_open, _)) = ws(tag("{")).parse(check) {
                    let mut depth = 1;
                    let mut pos = 0;
                    let bytes = after_open.as_bytes();
                    while pos < bytes.len() && depth > 0 {
                        if bytes[pos] == b'{' {
                            depth += 1;
                        } else if bytes[pos] == b'}' {
                            depth -= 1;
                        }
                        pos += 1;
                    }
                    if depth == 0 {
                        let after_close = &after_open[pos..];
                        let trimmed = skip_ws_and_comments0(after_close)
                            .unwrap_or((after_close, ()))
                            .0;
                        trimmed.starts_with("else")
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            let (struct_remaining, fields_opt) = if is_else_after_brace {
                (input, None)
            } else {
                let (remaining, parsed) = opt(delimited(
                    ws(tag("{")),
                    terminated(
                        separated_list1(ws(tag(",")), ws(parse_field_expr)),
                        opt(ws(tag(","))),
                    ),
                    ws(tag("}")),
                ))
                .parse(input)?;
                (remaining, parsed)
            };

            if let Some(fields) = fields_opt {
                Ok((
                    struct_remaining,
                    AstNode::StructLit {
                        variant: method,
                        fields,
                    },
                ))
            } else if let Some(mname) = method_name {
                // We parsed ::ident after type arguments or path separator
                // This could be:
                // 1. A variant without arguments (e.g., Option::None)
                // 2. A method call without parentheses (error)
                // For now, treat it as a Var with the full path
                let full_path = if !type_args.is_empty() {
                    // With type arguments: Option::<bool>::None
                    format!("{}::<{}>::{}", method, type_args.join(", "), mname)
                } else {
                    // Without type arguments: Option::None
                    format!("{}::{}", method, mname)
                };
                Ok((input, AstNode::Var(full_path)))
            } else {
                // Just a variable reference
                Ok((input, AstNode::Var(method)))
            }
        }
    }
}

fn parse_field_expr(input: &str) -> IResult<&str, (String, AstNode)> {
    let (input, name) = ws(parse_ident).parse(input)?;
    let (input, colon) = opt(ws(tag(":"))).parse(input)?;
    let (input, expr) = if colon.is_some() {
        ws(parse_expr).parse(input)?
    } else {
        (input, AstNode::Var(name.clone()))
    };
    Ok((input, (name, expr)))
}

/// PY-A/4: `lambda x, y: expr` — single-expression closure, identical to
/// `|x, y| expr`. The colon body is comma-free (tuple return needs parens).
fn parse_lambda(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("lambda")).parse(input)?;
    // params: comma-separated idents up to ':'
    let mut params: Vec<String> = Vec::new();
    let mut cur = input;
    loop {
        let rest = skip_ws_and_comments0(cur).map(|(i, _)| i).unwrap_or(cur);
        if rest.starts_with(':') {
            cur = &rest[1..];
            break;
        }
        let (rest, name) = ws(parse_ident).parse(cur)?;
        params.push(name);
        let rest2 = skip_ws_and_comments0(rest).map(|(i, _)| i).unwrap_or(rest);
        if rest2.starts_with(',') {
            cur = &rest2[1..];
        } else {
            let rest3 = skip_ws_and_comments0(rest2).map(|(i, _)| i).unwrap_or(rest2);
            if rest3.starts_with(':') {
                cur = &rest3[1..];
                break;
            }
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            )));
        }
    }
    let (input, body) = ws(parse_expr).parse(cur)?;
    Ok((
        input,
        AstNode::Closure {
            params,
            body: Box::new(body),
        },
    ))
}

fn parse_closure(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("|")).parse(input)?;
    let (input, params) = separated_list0(ws(tag(",")), ws(parse_ident)).parse(input)?;
    let (input, _) = ws(tag("|")).parse(input)?;
    // Try statement first (handles if, if-let, while, etc.), then expression
    let (input, body) =
        alt((crate::frontend::parser::stmt::parse_stmt, parse_expr)).parse(input)?;
    Ok((
        input,
        AstNode::Closure {
            params,
            body: Box::new(body),
        },
    ))
}

fn parse_block(input: &str) -> IResult<&str, AstNode> {
    let (input, body) = delimited(ws(tag("{")), parse_block_body, ws(tag("}"))).parse(input)?;
    Ok((input, AstNode::Block { body }))
}

fn parse_unsafe_expr(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("unsafe")).parse(input)?;
    let (input, block) = parse_block(input)?;
    if let AstNode::Block { body } = block {
        Ok((input, AstNode::Unsafe { body }))
    } else {
        Ok((input, block))
    }
}

fn parse_comptime_block(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("comptime")).parse(input)?;
    let (input, block) = parse_block(input)?;
    if let AstNode::Block { body } = block {
        Ok((input, AstNode::ComptimeBlock { body }))
    } else {
        Ok((input, block))
    }
}

pub fn parse_condition(input: &str) -> IResult<&str, AstNode> {
    // Parse condition for if/while - stops at '{' or other block delimiters
    parse_expr_no_if(input)
}

fn parse_if(input: &str) -> IResult<&str, AstNode> {
    let saved = input;
    let (input, _) = ws(tag("if")).parse(input)?;
    if let_keyword(input).is_ok() {
        return parse_if_let_expr(input).map_err(|_| {
            nom::Err::Error(NomError::new(saved, nom::error::ErrorKind::Tag))
        });
    }
    parse_if_tail(input)
}

/// The keyword `let`, with a word-boundary guard so an identifier that merely
/// starts with it (`letter`, `let_x`) is not read as an if-let.
fn let_keyword(input: &str) -> IResult<&str, &str> {
    let (i, _) = skip_ws_and_comments0(input)?;
    let (i, kw) = tag("let").parse(i)?;
    if i.chars()
        .next()
        .map_or(false, |c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    Ok((i, kw))
}

/// Can the match machinery actually TEST this pattern, or would it take the arm
/// unconditionally and hand the bindings a placeholder? The second kind would
/// make an arm that LOOKS conditional silently return the wrong branch's value.
///
/// Constructor patterns (`Token::Ident(n)`, `Option::Some(n)`) are accepted
/// since batch 396: a variant of a registered enum now carries a `[tag, p0, …]`
/// block, and both the tag test and the payload bindings read that block
/// (measured — `match t { Token::Ident(n) => n + 1, _ => 900 }` printed 1 for
/// every value before, and 6 for `Token::Ident(5)` / 900 for `Token::Eof` now).
///
/// Tuples are still refused: no representation stores a tuple's arity or
/// elements as a testable tag, so such an arm would still be the
/// always-matches kind this guard exists to keep out.
fn pattern_has_matcher(pattern: &AstNode) -> bool {
    match pattern {
        AstNode::Tuple(_) => false,
        AstNode::BindPattern { pattern, .. } => pattern_has_matcher(pattern),
        AstNode::TypeAnnotatedPattern { pattern, .. } => pattern_has_matcher(pattern),
        AstNode::OrPattern(ps) => ps.iter().all(pattern_has_matcher),
        _ => true,
    }
}

/// PY-A: `if let PAT = EXPR { … } else { … }` in VALUE position (a `let`
/// right-hand side, an argument, a return). Statement position has always had
/// its own parser; here the same spelling was unparsable, and one failed
/// top-level item costs the rest of the file (W1002).
///
/// Desugared to a `match` because the match result slot is the only existing
/// machinery that carries a value out of an arm body — the statement-position
/// `IfLet` node has no result id, and its `else_` is never lowered.
fn parse_if_let_expr(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = let_keyword(input)?;
    let (input, pattern) = ws(parse_pattern).parse(input)?;
    if !pattern_has_matcher(&pattern) {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    let (input, _) = ws(tag("=")).parse(input)?;
    let (input, scrutinee) = ws(parse_full_expr).parse(input)?;
    let (input, then) = parse_block(input)?;
    let (input, else_body) = opt(preceded(ws(tag("else")), parse_block)).parse(input)?;
    // A value-producing if-let without `else` has no defined value for the
    // non-matching case, so it is not desugared: the file keeps failing here.
    let else_body = match else_body {
        Some(b) => b,
        None => {
            return Err(nom::Err::Error(NomError::new(
                input,
                nom::error::ErrorKind::Tag,
            )))
        }
    };
    Ok((
        input,
        AstNode::Match {
            scrutinee: Box::new(scrutinee),
            arms: vec![
                MatchArm {
                    pattern: Box::new(pattern),
                    guard: None,
                    body: Box::new(then),
                },
                MatchArm {
                    pattern: Box::new(AstNode::Ignore),
                    guard: None,
                    body: Box::new(else_body),
                },
            ],
        },
    ))
}

/// Condition + then-block + else chain; the leading `if` keyword must already
/// be consumed. Also the entry point for Python-style `elif` chains.
fn parse_if_tail(input: &str) -> IResult<&str, AstNode> {
    // Parse condition with special handling
    let (input, cond) = if let Ok((i, expr)) = parse_condition(input) {
        (i, expr)
    } else {
        // Fallback: parse any expression
        ws(parse_expr_no_if).parse(input)?
    };
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
                // else if ... (parse as another if statement)
                preceded(ws(tag("if")), map(parse_if_tail, |if_node| vec![if_node])),
            )),
        ),
        // elif ... (PY-2 alias for else-if)
        preceded(ws(tag("elif")), map(parse_if_tail, |if_node| vec![if_node])),
    )))
    .parse(input)?;

    let else_: Vec<AstNode> = else_opt.unwrap_or(vec![]);
    Ok((
        input,
        AstNode::If {
            cond: Box::new(cond),
            then,
            else_,
        },
    ))
}

fn parse_tuple_or_paren(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("(")).parse(input)?;
    // PY-A: genexp `(expr for name in iter (if cond)?)` — same desugar as
    // listcomp (V1: eager collection, no laziness).
    if comp_probe(input, b'(', b')').is_some() {
        let (rest, call) = parse_listcomp_full(input)?;
        return Ok((rest, call));
    }
    // PY-A: walrus `(name := expr)` — single parenthesized assignment.
    // Detected before the general expr list (parse_expr cannot parse ':=').
    {
        let t = input.trim_start();
        if let Some((ident, rest)) = take_ident(t) {
            let rest_trim = rest.trim_start();
            if rest_trim.starts_with(":=") {
                let after = &rest_trim[2..];
                let (after, expr) = parse_full_expr(after)?;
                let (after, _) = ws(tag(")")).parse(after)?;
                return Ok((
                    after,
                    AstNode::Assign(
                        Box::new(AstNode::Var(ident.to_string())),
                        Box::new(expr),
                    ),
                ));
            }
        }
    }
    // 无参闭包 `(|| expr)` / `(|| { … })` —— 括号内没有左操作数，`||` 在这里
    // 只能是无参闭包的参数栏，不可能是逻辑或。此前这种写法整条语句被静默
    // 丢弃（integration_all_features / integration_test_program 两个官方文件）。
    {
        let t = input.trim_start();
        if t.starts_with("||") && !t[2..].starts_with('|') {
            let after_bars = &t[2..];
            let (after, body) =
                alt((crate::frontend::parser::stmt::parse_stmt, parse_expr)).parse(after_bars)?;
            let (after, _) = ws(tag(")")).parse(after)?;
            return Ok((
                after,
                AstNode::Closure {
                    params: vec![],
                    body: Box::new(body),
                },
            ));
        }
    }
    let (input, mut items) = separated_list0(ws(tag(",")), ws(parse_expr)).parse(input)?;
    let (input, trailing_comma) = opt(ws(tag(","))).parse(input)?;
    let (input, _) = ws(tag(")")).parse(input)?;
    // `()` is the empty tuple; `(x,)` is a one-element tuple; only `(x)` — a
    // parenthesized expression — unwraps. (Taking the remove path for an EMPTY
    // list panicked: `removal index (is 0) should be < len (is 0)`, measured on
    // the strategies corpus.)
    if items.len() == 1 && trailing_comma.is_none() {
        Ok((input, items.remove(0)))
    } else {
        Ok((input, AstNode::Tuple(items)))
    }
}

/// Parse a dict literal: {"key": value, ...}
/// PY-A: dict comprehension body `{k: v for NAME in ITER (if COND)?}` —
/// caller has consumed `{` and proven ` for ` presence. Desugars to
/// Call{receiver: ITER, method: "__collect_dict__", args: [lambda]}.
/// The lambda returns a packed (k<<32)|v pair (V1: both i64).
fn parse_dictcomp_full(input: &str) -> IResult<&str, AstNode> {
    let (input, key_expr) = ws(parse_full_expr).parse(input)?;
    let (input, _) = ws(tag(":")).parse(input)?;
    let (input, val_expr) = ws(parse_full_expr).parse(input)?;
    let (input, _) = ws(tag("for")).parse(input)?;
    let (input, names) = parse_comp_target(input)?;
    let (input, _) = ws(tag("in")).parse(input)?;
    let (input, iter) = ws(parse_full_expr).parse(input)?;
    let (dc_name, dc_smap) = comp_target_binding(&names);
    let key_expr = apply_subst(key_expr, &dc_smap);
    let val_expr = apply_subst(val_expr, &dc_smap);
    // PY-A: optional `if <cond>` filter — `{k: v for k in it if pred}`. Without
    // this the trailing `if …` was left unconsumed, the `}` check failed, and
    // the ENTIRE definition (plus everything after it) was silently dropped.
    // Mirrors the list comp: the lambda returns the -1 sentinel for filtered-out
    // items and the runtime collect skips it.
    let (input, cond) = if let Ok((rest, _)) = ws(tag("if")).parse(input) {
        let (rest, c) = ws(parse_full_expr).parse(rest)?;
        (rest, Some(c))
    } else {
        (input, None)
    };
    let cond_body = |pair: AstNode, cond: Option<AstNode>| -> AstNode {
        match cond {
            None => pair,
            Some(c) => AstNode::If {
                cond: Box::new(c),
                then: vec![AstNode::ExprStmt { expr: Box::new(pair) }],
                else_: vec![AstNode::ExprStmt {
                    expr: Box::new(AstNode::Lit(-1)),
                }],
            },
        }
    };
    // pair lambda: |NAME| pack(KEY, VAL) — use __pack_pair__ runtime
    let pair_expr = AstNode::Call {
        receiver: None,
        method: "__pack_pair__".to_string(),
        args: vec![key_expr, val_expr],
        type_args: vec![],
        structural: false,
    };
    let lam = AstNode::Closure {
        params: vec![dc_name],
        body: Box::new(cond_body(pair_expr, cond.map(|c| apply_subst(c, &dc_smap)))),
    };
    let call = AstNode::Call {
        receiver: Some(Box::new(iter)),
        method: "__collect_dict__".to_string(),
        args: vec![lam],
        type_args: vec![],
        structural: false,
    };
    let (input, _) = ws(tag("}")).parse(input)?;
    Ok((input, call))
}

/// PY-A: one dict-literal entry — either `key: value` or a `**mapping` spread
/// (represented as a special KEY marker with a dummy value; the MIR lowering
/// merges it into the map).
fn parse_dict_entry(input: &str) -> IResult<&str, (AstNode, AstNode)> {
    if let Some(after) = input.trim_start().strip_prefix("**") {
        let (rest, value) = parse_expr(after.trim_start())?;
        return Ok((
            rest,
            (
                AstNode::Call {
                    receiver: None,
                    method: "zeta_dict_spread".to_string(),
                    args: vec![value],
                    type_args: vec![],
                    structural: false,
                },
                AstNode::Lit(0),
            ),
        ));
    }
    pair(ws(parse_expr), ws(preceded(tag(":"), ws(parse_expr)))).parse(input)
}

fn parse_dict_lit(input: &str) -> IResult<&str, AstNode> {
    // PY-A: dict/set comprehension probe BEFORE consuming `{` — comp_probe
    // needs the opening brace to do its depth scan.
    if comp_probe(input, b'{', b'}').is_some() {
        let (input, _) = ws(tag("{")).parse(input)?;
        // Try dictcomp: EXPR: EXPR for NAME in ITER
        if let Ok((rest, node)) = parse_dictcomp_full(input) {
            return Ok((rest, node));
        }
        // Try setcomp: EXPR for NAME in ITER (parsed as listcomp body)
        if let Ok((rest, node)) = parse_listcomp_full(input) {
            return Ok((rest, node));
        }
    }
    let (input, _) = ws(tag("{")).parse(input)?;
    // Check it's not an empty block — if next char is '}' it's empty dict
    let trimmed = input.trim_start();
    if trimmed.starts_with("}") {
        let (input, _) = ws(tag("}")).parse(input)?;
        return Ok((input, AstNode::DictLit { entries: vec![] }));
    }
    // PY-A: `{a, b, c}` is a SET literal, not a dict. Zeta has no set type, and
    // the existing set comprehension already lowers to a list, so a set literal
    // does the same (duplicates are NOT collapsed — same accepted limitation as
    // setcomp). Without this branch the whole enclosing definition was dropped:
    // `g.stocks={'510500.XSHG', …}` in `initialize`.
    //
    // Dispatch by looking for a top-level `:` after the first element — a dict
    // entry is `KEY: VALUE`, a set element is a bare expression. `{**a}` fails
    // the probe and falls through to the dict path, which handles it.
    // `{**a, **b}` must NOT be mistaken for a set — a spread element starts
    // with `*`, and `parse_expr` happily parses `**a` as a double deref, so the
    // missing `:` would route it into the set branch (this broke t121).
    if !input.trim_start().starts_with('*') {
    if let Ok((after_first, first)) = ws(parse_expr).parse(input) {
        if ws(tag(":")).parse(after_first).is_err() {
            let mut rest = after_first;
            let mut items = vec![first];
            while let Ok((after_comma, _)) = ws(tag(",")).parse(rest) {
                match ws(parse_expr).parse(after_comma) {
                    Ok((r, e)) => {
                        items.push(e);
                        rest = r;
                    }
                    // Trailing comma — `{1, 2,}`.
                    Err(_) => {
                        rest = after_comma;
                        break;
                    }
                }
            }
            let (rest, _) = ws(tag("}")).parse(rest)?;
            // PY-A: a macro invocation is not a collection element. `parse_expr`
            // accepts `println!(…)` as an atom, so `{ println!("x") }` looked like
            // a one-element set here and the macro never reached the expander —
            // MIR then skipped it silently and `2 => { println!("arm-two") }`
            // printed nothing. Failing the whole alternative lets the next one in
            // `parse_primary_atom` (right below: `parse_block`) read the braces as
            // a block, which is what `{ …; … }` already means. (batch 379, #38 ④)
            if items.iter().any(|i| matches!(i, AstNode::MacroCall { .. })) {
                return Err(nom::Err::Error(NomError::new(
                    input,
                    nom::error::ErrorKind::Verify,
                )));
            }
            return Ok((rest, AstNode::ArrayLit(items)));
        }
    }
    }
    let (input, entries) =
        separated_list0(ws(tag(",")), parse_dict_entry).parse(input)?;
    // PEP 8 / black style: a trailing comma before the closing brace is normal
    // Python — `{"a": 1,}` and, more importantly, every multi-line dict. nom's
    // `separated_list0` rewinds past a final separator that is not followed by
    // another element, so it must be consumed explicitly. Without this the
    // `,}` pair failed, the whole statement failed to parse, and the enclosing
    // block (and everything after it) was silently dropped.
    let (input, _) = if entries.is_empty() {
        (input, None)
    } else {
        opt(ws(tag(","))).parse(input)?
    };
    let (input, _) = ws(tag("}")).parse(input)?;
    Ok((input, AstNode::DictLit { entries }))
}

/// PY-A: shared comprehension scanner — probes `for ` at depth 1 inside the
/// opening bracket. Returns Some(()) if this looks like a comprehension.
fn comp_probe(input: &str, open: u8, _close: u8) -> Option<()> {
    let b = input.as_bytes();
    let mut depth = 1i32;
    let mut quote: Option<u8> = None;
    let mut i = if b.first() == Some(&open) { 1 } else { return None };
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' { i += 2; continue; }
            if c == q { quote = None; }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' => quote = Some(c),
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return None;
                }
            }
            b' ' if depth == 1
                && i + 5 <= b.len()
                && input.is_char_boundary(i)
                && input.is_char_boundary(i + 5)
                && &input[i..i + 5] == " for " =>
            {
                return Some(());
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// PY-A: generic comprehension parse for `(`/`[`/`{` forms.
/// kind: "list" | "genexp" | "set" | "dict"
/// PY-A: Python list comprehension `[expr for name in iterable (if cond)?]`
/// desugars into `__collect__(iterable, lambda(name) expr_or_filter)` — a
/// runtime helper that builds a Vec of results. The lambda uses the Closure
/// node (env capture works for the enclosing scope's variables).
fn parse_listcomp_full(input: &str) -> IResult<&str, AstNode> {
    let first = input.trim_start().as_bytes().first().copied();
    if first != Some(b'[') && first != Some(b'(') && first != Some(b'{') {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    // Heuristic pre-scan: contains " for " before the matching close?
    let mut depth = 1i32;
    let mut quote: Option<u8> = None;
    let b = input.as_bytes();
    let mut has_for = false;
    let mut i = 1;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' { i += 2; continue; }
            if c == q { quote = None; }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' => quote = Some(c),
            b'[' | b'(' | b'{' => depth += 1,
            b']' => { depth -= 1; if depth == 0 { break; } }
            b')' | b'}' => depth -= 1,
            b' ' if depth == 1
                && i + 5 <= b.len()
                && input.is_char_boundary(i)
                && input.is_char_boundary(i + 5) =>
            {
                if &input[i..i + 5] == " for " {
                    has_for = true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if !has_for {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    // Parse as comprehension: OPENER ELEM for NAME in ITER (if COND)? CLOSE
    let (open_str, close_str) = match first {
        Some(b'(') => ("(", ")"),
        Some(b'{') => ("{", "}"),
        _ => ("[", "]"),
    };
    let (input, _) = ws(tag(open_str)).parse(input)?;
    let (input, elem) = ws(parse_full_expr).parse(input)?;
    let (input, _) = ws(tag("for")).parse(input)?;
    let (input, names) = parse_comp_target(input)?;
    let (input, _) = ws(tag("in")).parse(input)?;
    let (input, iter) = ws(parse_full_expr).parse(input)?;
    // optional filter — everything up to CLOSE is the condition
    let (input, cond) = if let Ok((rest, _)) = ws(tag("if")).parse(input) {
        let (rest, c) = ws(parse_full_expr).parse(rest)?;
        (rest, Some(c))
    } else {
        (input, None)
    };
    let (input, _) = ws(tag(close_str)).parse(input)?;
    // PY-A: `for k, v in pairs` — bind through the element's slots.
    let (name, smap) = comp_target_binding(&names);
    let elem = apply_subst(elem, &smap);
    let cond = cond.map(|c| apply_subst(c, &smap));

    // Build lambda: |NAME| if COND { collect(EXPR) } — via a single expression:
    // __comp_item__(EXPR, COND) semantics handled in runtime collect.
    // Simplest desugar: __collect__(ITER, COND_HANDLE?, lambda)
    // We construct: __collect__(ITER, |NAME| EXPR, |NAME| COND?) — two lambdas
    // is awkward; instead the runtime receives ITER and one lambda returning
    // Option-like: -1 sentinel means skip. Use lambda body:
    //   if COND_missing { EXPR } else { if COND { EXPR } else { -1 } }
    let body = match cond {
        None => elem,
        Some(c) => AstNode::If {
            cond: Box::new(c),
            then: vec![AstNode::ExprStmt { expr: Box::new(elem) }],
            else_: vec![AstNode::ExprStmt {
                expr: Box::new(AstNode::Lit(-1)),
            }],
        },
    };
    let lam = AstNode::Closure {
        params: vec![name],
        body: Box::new(body),
    };

    // Build the call: __collect__(ITER, LAMBDA) — receiver-style so parse
    // produces Call{receiver: Some(iter), method: "__collect__", args:[lam]}
    let call = AstNode::Call {
        receiver: Some(Box::new(iter)),
        method: "__collect__".to_string(),
        args: vec![lam],
        type_args: vec![],
        structural: false,
    };
    Ok((input, call))
}

fn parse_array_lit(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("[")).parse(input)?;

    // First, check if this is a dynamic array literal: [dynamic]T{}
    let dynamic_array_parser = |input| {
        // Check for "dynamic]"
        let (input, _) = ws(tag("dynamic")).parse(input)?;
        let (input, _) = ws(tag("]")).parse(input)?;

        // Parse the element type
        let (input, elem_type) = ws(parse_type).parse(input)?;

        // Parse the opening brace
        let (input, _) = ws(tag("{")).parse(input)?;

        // Parse optional initializer values
        let (input, items) = terminated(
            separated_list0(ws(tag(",")), ws(parse_expr)),
            opt(ws(tag(","))),
        )
        .parse(input)?;

        // Parse the closing brace
        let (input, _) = ws(tag("}")).parse(input)?;

        // Create a dynamic array literal node
        Ok((
            input,
            AstNode::DynamicArrayLit {
                elem_type,
                elements: items,
            },
        ))
    };

    // Try to parse as repeat syntax: [value; size]
    let repeat_parser = |input| {
        let (input, value) = ws(parse_expr).parse(input)?;
        let (input, _) = ws(tag(";")).parse(input)?;
        let (input, size) = ws(parse_expr).parse(input)?;
        let (input, _) = ws(tag("]")).parse(input)?;
        Ok((
            input,
            AstNode::ArrayRepeat {
                value: Box::new(value),
                size: Box::new(size),
            },
        ))
    };

    // Try to parse as regular array literal: [item1, item2, ...]
    let regular_parser = |input| {
        let (input, items) = terminated(
            separated_list0(ws(tag(",")), ws(parse_expr)),
            opt(ws(tag(","))),
        )
        .parse(input)?;
        let (input, _) = ws(tag("]")).parse(input)?;
        Ok((input, AstNode::ArrayLit(items)))
    };

    // Try dynamic array first, then repeat syntax, then regular syntax
    alt((dynamic_array_parser, repeat_parser, regular_parser)).parse(input)
}

fn parse_bool(input: &str) -> IResult<&str, AstNode> {
    alt((
        tag("true").map(|_| AstNode::Bool(true)),
        tag("false").map(|_| AstNode::Bool(false)),
    ))
    .parse(input)
}

pub(crate) fn parse_unary(input: &str) -> IResult<&str, AstNode> {
    // Check for "!" but NOT followed by "=" (which would be != operator)
    let (input, op_opt) = if input.starts_with("!") && !input.starts_with("!=") {
        // It's a unary !, not !=
        let (input, _) = tag("!")(input)?;
        (input, Some("!"))
    } else if starts_with_kw(input, "not") {
        // PY-A: `not` keeps its OWN operator. Mapping it to `!` made
        // `if not parts:` lower to `py_vec_not(parts)` (the `!` branch treats an
        // ARRAY as `~mask`), so a 1-element list became a list — truthy — and the
        // wrong branch was taken (measured: `remove_extreme_return_bars` returned
        // an empty frame). `~`/`!` stay element-wise for arrays.
        let (input, _) = tag("not")(input)?;
        (input, Some("not"))
    } else if input.starts_with("~") {
        // PY-A: Python `~` == bitwise not — same as `!` for i64 masks
        let (input, _) = tag("~")(input)?;
        (input, Some("!"))
    } else {
        // Try other unary operators
        opt(alt((tag("&mut"), tag("&"), tag("-"), tag("*")))).parse(input)?
    };

    let (input, expr) = if op_opt.is_some() {
        ws(parse_unary).parse(input)?
    } else {
        ws(parse_power).parse(input)?
    };
    if let Some(op) = op_opt {
        Ok((
            input,
            AstNode::UnaryOp {
                op: op.to_string(),
                expr: Box::new(expr),
            },
        ))
    } else {
        Ok((input, expr))
    }
}

/// PY-A: `**` gets its own precedence tier, TIGHTER than `* / % @` and LOOSER
/// than a leading unary minus (so `parse_unary` reaches it as its operand and
/// `-2 ** 2` is `-(2 ** 2)`). `parse_multiplicative` used to carry `**` in its
/// own operator list, which made it left-associative with the rest of the
/// tier: `2 * 3 ** 2` folded as `(2*3)**2` = 36 and `2 ** 3 % 5` swallowed the
/// `%` into the exponent = 8. The exponent re-enters `parse_unary`, so
/// `2 ** -3` and the right-associative `2 ** 3 ** 2` fall out for free.
fn parse_power(input: &str) -> IResult<&str, AstNode> {
    let (input, base) = parse_postfix(input)?;
    let after = match skip_ws_and_comments0(input) {
        Ok((i, _)) => i,
        Err(_) => input,
    };
    let Some(rhs) = after.strip_prefix("**") else {
        return Ok((input, base));
    };
    let (rest, exp) = ws(parse_unary).parse(rhs)?;
    Ok((
        rest,
        AstNode::BinaryOp {
            op: "**".to_string(),
            left: Box::new(base),
            right: Box::new(exp),
        },
    ))
}

fn parse_simple_ident(input: &str) -> IResult<&str, AstNode> {
    // Simple identifier parser, doesn't try to parse paths or method calls
    let (input, ident) = parse_ident(input)?;
    Ok((input, AstNode::Var(ident)))
}

/// Parse `trait::` queries: trait::value_type<T>, trait::is_same<T, U>, etc.
fn parse_trait_query(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("trait::")).parse(input)?;
    let (input, query_name) = parse_ident(input)?;

    // Parse type arguments: <T> or <T, U>
    // Parse type arguments (required for trait queries)
    let (input, type_args) = ws(delimited(
        tag("<"),
        separated_list1(ws(tag(",")), ws(parse_type)),
        tag(">"),
    ))
    .parse(input)?;

    // Build the method name as "trait::query_name"
    let method = format!("trait::{}", query_name);

    // Create a Call expression with the type args as arguments
    // We pass the type arguments as AstNode::StringLit nodes that represent type names
    let args: Vec<AstNode> = type_args.into_iter().map(AstNode::StringLit).collect();

    Ok((
        input,
        AstNode::Call {
            receiver: None,
            method,
            args,
            type_args: vec![], // Type args go as string args
            structural: false,
        },
    ))
}

/// Python-style literals: `True` / `False` / `None` (word-boundary guarded).
/// `None` lowers to 0 — pragmatic V1 (`is None` / `== None` both work).
fn parse_python_bool(input: &str) -> IResult<&str, AstNode> {
    fn boundary(input: &str) -> IResult<&str, char> {
        peek(alt((
            none_of("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_"),
            map(nom::combinator::eof, |_| ' '),
        )))
        .parse(input)
    }
    alt((
        map(terminated(tag("True"), boundary), |_| AstNode::Bool(true)),
        map(terminated(tag("False"), boundary), |_| AstNode::Bool(false)),
        map(terminated(tag("None"), boundary), |_| AstNode::Lit(0)),
    ))
    .parse(input)
}

/// PY-A: does `s` start with keyword `kw` followed by a non-identifier char?
pub(crate) fn starts_with_kw(s: &str, kw: &str) -> bool {
    s.starts_with(kw)
        && s[kw.len()..]
            .chars()
            .next()
            .map_or(true, |c| !(c.is_ascii_alphanumeric() || c == '_'))
}

/// PY-A: f-string `f"hello {name}!"` → FString(parts). Literal text stays
/// StringLit; `{expr}` parts parse as full expressions (converted to strings
/// at MIR lowering). `{{`/`}}` escape braces. V1: no format specs (`{x:.2f}`),
/// no multiline f-strings.
fn parse_fstring(input: &str) -> IResult<&str, AstNode> {
    fn err<T>(input: &str) -> IResult<&str, T> {
        Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
    }
    if !(input.starts_with("f\"") || input.starts_with("f'")) {
        return err(input);
    }
    let quote = input.as_bytes()[1];
    let b = input.as_bytes();
    let mut parts: Vec<AstNode> = Vec::new();
    let mut lit = String::new();
    let mut i = 2usize;
    while i < b.len() {
        let c = b[i];
        if c == b'\\' && i + 1 < b.len() {
            let e = b[i + 1];
            match e {
                b'n' => lit.push('\n'),
                b't' => lit.push('\t'),
                b'r' => lit.push('\r'),
                other => {
                    lit.push('\\');
                    lit.push(other as char);
                }
            }
            i += 2;
            continue;
        }
        if c == quote {
            let rest = &input[i + 1..];
            if !lit.is_empty() {
                parts.push(AstNode::StringLit(std::mem::take(&mut lit)));
            }
            if parts.is_empty() {
                parts.push(AstNode::StringLit(String::new()));
            }
            return Ok((rest, AstNode::FString(parts)));
        }
        if c == b'{' {
            if i + 1 < b.len() && b[i + 1] == b'{' {
                lit.push('{');
                i += 2;
                continue;
            }
            let mut j = i + 1;
            let mut depth = 1usize;
            while j < b.len() {
                match b[j] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            if j >= b.len() {
                return err(input); // unterminated brace
            }
            let inner = &input[i + 1..j];
            if !lit.is_empty() {
                parts.push(AstNode::StringLit(std::mem::take(&mut lit)));
            }
            // PY-A: format spec `{expr:spec}` — split on the first top-level
            // colon (outside quotes/brackets). V1: f64 uses the spec via
            // zeta_fmt_f64_spec; other types ignore it.
            let (expr_text, spec) = match split_format_spec(inner) {
                Some((e, sp)) if !sp.is_empty() => (e, Some(sp)),
                _ => (inner, None),
            };
            let base_expr = match parse_full_expr(expr_text) {
                Ok((rem, e)) if rem.trim().is_empty() => e,
                _ => AstNode::StringLit(expr_text.to_string()), // V1: unparsable → literal
            };
            let expr = match spec {
                Some(sp) => AstNode::Call {
                    receiver: None,
                    method: "__fmtspec__".to_string(),
                    args: vec![base_expr, AstNode::StringLit(sp.to_string())],
                    type_args: vec![],
                    structural: false,
                },
                None => base_expr,
            };
            parts.push(expr);
            i = j + 1;
            continue;
        }
        if c == b'}' {
            if i + 1 < b.len() && b[i + 1] == b'}' {
                lit.push('}');
                i += 2;
                continue;
            }
            lit.push('}');
            i += 1;
            continue;
        }
        // Copy the full UTF-8 char
        let ch_len = utf8_seq_len(c);
        lit.push_str(&input[i..i + ch_len]);
        i += ch_len;
    }
    err(input) // unterminated f-string
}

/// PY-A: split `{expr:spec}` at the first top-level colon (outside quotes
/// and brackets). Returns (expr_text, spec) or None when there is no spec.
fn split_format_spec(inner: &str) -> Option<(&str, &str)> {
    let b = inner.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    for (i, &c) in b.iter().enumerate() {
        if let Some(q) = quote {
            if c == b'\\' {
                continue; // skip escaped char (index i+1 skipped by loop too)
            }
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            b'\'' | b'"' => quote = Some(c),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b':' if depth == 0 => {
                return Some((&inner[..i], inner[i + 1..].trim()));
            }
            _ => {}
        }
    }
    None
}

fn utf8_seq_len(first_byte: u8) -> usize {
    match first_byte {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// PY-A: implicit concatenation of adjacent string literals — `"a" "b"`,
/// `f"x={x}" f"y={y}"`. Basic Python (used constantly for long messages, and
/// wrapped across lines inside calls). Without this the SECOND literal stayed
/// unconsumed, which failed the enclosing call and silently truncated the file.
fn concat_string_nodes(a: AstNode, b: AstNode) -> AstNode {
    fn into_parts(n: AstNode) -> Vec<AstNode> {
        match n {
            AstNode::FString(p) => p,
            AstNode::StringLit(s) => vec![AstNode::StringLit(s)],
            other => vec![other],
        }
    }
    let mut parts = into_parts(a);
    parts.extend(into_parts(b));
    if parts.iter().all(|p| matches!(p, AstNode::StringLit(_))) {
        let joined: String = parts
            .into_iter()
            .map(|p| match p {
                AstNode::StringLit(s) => s,
                _ => String::new(),
            })
            .collect();
        return AstNode::StringLit(joined);
    }
    AstNode::FString(parts)
}

pub fn parse_primary(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut node) = parse_primary_atom(input)?;
    if matches!(node, AstNode::StringLit(_) | AstNode::FString(_)) {
        loop {
            // `trim_start` (not just spaces) so the two halves may sit on
            // different lines, as they do inside a wrapped call.
            let probe = input.trim_start();
            let Ok((after, next)) = parse_primary_atom(probe) else {
                break;
            };
            if !matches!(next, AstNode::StringLit(_) | AstNode::FString(_)) {
                break;
            }
            if after.len() == probe.len() {
                break; // zero-width match guard
            }
            node = concat_string_nodes(node, next);
            input = after;
        }
    }
    Ok((input, node))
}

fn parse_primary_atom(input: &str) -> IResult<&str, AstNode> {
    alt((
        parse_lambda,
        parse_fstring,
        parse_python_bool,
        parse_trait_query,
        parse_tuple_or_paren,
        parse_lit,
        parse_raw_string_lit,
        parse_triple_quoted_string,
        parse_string_lit,
        parse_match_expr,
        parse_loop,
        parse_path_expr,
        parse_simple_ident,
        parse_listcomp_full,
        parse_array_lit,
        parse_bool,
        parse_closure,
        parse_dict_lit,
        parse_block,
        parse_unsafe_expr,
        parse_comptime_block,
    ))
    .parse(input)
}

/// PY-A: comma subscript `a[i, j]` — pandas `.iloc[r, c]` / `.loc[r, c]`.
/// Without this branch `if x[0,0]:` lost the whole top-level item outright, and
/// `a = x[0,0]` was *silently* wrong: the subscript failed, `x` became the
/// whole right-hand side and `[0,0]` was swallowed as a stray array-literal
/// statement. Only 2+ indices take this path — a single index must keep using
/// the plain subscript branch.
fn parse_multi_index(input: &str) -> IResult<&str, Vec<AstNode>> {
    let (rest, items) = delimited(
        ws(tag("[")),
        terminated(
            separated_list1(ws(tag(",")), ws(parse_multi_index_element)),
            // Trailing comma: `a[:,]` (all rows, all columns).
            opt(ws(tag(","))),
        ),
        ws(tag("]")),
    )
    .parse(input)?;
    if items.len() < 2 {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Verify,
        )));
    }
    Ok((rest, items))
}

/// PY-A: one element of a comma subscript. A plain expression, or a slice —
/// pandas spells “all rows, column 0” as `.iloc[:, 0]` / `.loc[i, :]`, and a
/// bare `:` is not an expression, so those files failed to parse outright.
///
/// A slice element becomes the same shape the single-index slice path builds
/// (`Call { method: "__slice__" }`) but with a **placeholder receiver**; the
/// caller binds the real base (see `bind_slice_receiver`).
fn parse_multi_index_element(input: &str) -> IResult<&str, AstNode> {
    let slice_call = |start: Option<AstNode>, end: Option<AstNode>, step: Option<AstNode>| {
        let end = end.unwrap_or(AstNode::Lit(i64::MIN));
        match step {
            Some(step) => AstNode::Call {
                receiver: None,
                method: "__slice_step__".to_string(),
                args: vec![start.unwrap_or(AstNode::Lit(i64::MIN)), end, step],
                type_args: vec![],
                structural: false,
            },
            None => AstNode::Call {
                receiver: None,
                method: "__slice__".to_string(),
                args: vec![start.unwrap_or(AstNode::Lit(0)), end],
                type_args: vec![],
                structural: false,
            },
        }
    };

    // Start: `:…` (omitted) or `expr :` (slice) or `expr` (plain element).
    let (input, start) = if let Ok((rest, _)) = ws(tag(":")).parse(input) {
        (rest, None)
    } else {
        let (rest, e) = ws(parse_expr).parse(input)?;
        match ws(tag(":")).parse(rest) {
            Ok((rest2, _)) => (rest2, Some(e)),
            Err(_) => return Ok((rest, e)),
        }
    };
    // End (optional).
    let (input, end) = match ws(parse_expr).parse(input) {
        Ok((rest, e)) => (rest, Some(e)),
        Err(_) => (input, None),
    };
    // Step (optional): `:` or `:expr`.
    let (input, step) = if let Ok((rest, _)) = ws(tag(":")).parse(input) {
        match ws(parse_expr).parse(rest) {
            Ok((r, e)) => (r, Some(e)),
            Err(_) => (rest, Some(AstNode::Lit(1))),
        }
    } else {
        (input, None)
    };
    Ok((input, slice_call(start, end, step)))
}

/// A slice element of a comma subscript is a view of the subscript's base, so
/// bind the base that `parse_multi_index_element` left as `receiver: None`.
/// Without this the slice lowered as a free `__slice__` call (an undefined
/// symbol) instead of the base's slice.
fn bind_slice_receiver(elem: AstNode, base: &AstNode) -> AstNode {
    match elem {
        AstNode::Call {
            receiver: None,
            method,
            args,
            type_args,
            structural,
        } if method == "__slice__" || method == "__slice_step__" => AstNode::Call {
            receiver: Some(Box::new(base.clone())),
            method,
            args,
            type_args,
            structural,
        },
        other => other,
    }
}

pub(crate) fn parse_postfix(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut expr) = parse_primary(input)?;
    loop {
        // Check if this is a range operator ".." or "..=" before parsing as field access
        // We need to look ahead to see if the dot is followed by another dot or equals
        let bytes = input.as_bytes();
        let mut is_range_operator = false;

        // Skip whitespace
        let mut pos = 0;
        while pos < bytes.len() && (bytes[pos] as char).is_whitespace() {
            pos += 1;
        }

        // Check if we have a dot
        if pos < bytes.len() && bytes[pos] == b'.' {
            // Check if next character is also dot or equals
            if pos + 1 < bytes.len() && bytes[pos + 1] == b'.' {
                // `...` is Ellipsis (stmt/type arg), not field access and not
                // a `..` range — stop postfix so the statement parser sees it.
                // (`..` / `..=` are also stopped here for the range parser.)
                is_range_operator = true;
            }
        }

        // Check if this is a range operator
        if is_range_operator {
            // It's a range operator, break and let binary operator parsing handle it
            break;
        }

        // Check for .await BEFORE general .ident parsing — .await must not be
        // consumed as a field access to a method named "await".
        if let Ok((i, _)) = ws(tag(".await")).parse(input) {
            expr = AstNode::Await(Box::new(expr));
            input = i;
            continue; // allow more postfix operators after .await
        }

        let dot_result = ws(tag(".")).parse(input);
        if let Ok((i, _)) = dot_result {
            let (j, field_or_method) = parse_member_ident(i)?;

            // Type arguments on a member must use the explicit turbofish
            // (`x.foo::<i64>()`). Accepting a bare `<` here ate comparisons:
            // `f(a.b<10, c.d>20)` had its `<` read as `<...>` and the inner
            // slice scanner grabbed everything up to the `>` in `c.d>20`, so
            // the call broke. No source in the repo uses the bare form
            // (grep: none), and Rust has the same rule.
            let (j2, type_args_opt) =
                opt(ws(preceded(tag("::"), parse_type_args))).parse(j)?;
            let type_args: Vec<String> = type_args_opt.unwrap_or_default();

            // Now check if this is a method call (has parentheses) or field access
            // Check if there's a '('
            let args_opt = if let Ok((after_paren, _)) = ws(tag("(")).parse(j2) {
                // Parse argument list
                // Check if we have ')' immediately (empty argument list)
                let close_paren_result = ws(tag(")")).parse(after_paren);
                if let Ok((after_paren2, _)) = close_paren_result {
                    (Some(vec![]), after_paren2)
                } else {
                    // Parse non-empty argument list (PY-A: kwargs accepted,
                    // names dropped — positional binding)
                    let (after_args, args) = terminated(
                        separated_list1(ws(tag(",")), ws(parse_call_arg)),
                        opt(ws(tag(","))),
                    )
                    .parse(after_paren)?;

                    // Parse closing ')'
                    let (k, _) = ws(tag(")")).parse(after_args)?;
                    (Some(args), k)
                }
            } else {
                (None, j2)
            };

            let (args_opt_result, k) = args_opt;

            if let Some(args) = args_opt_result {
                // It's a method call (with or without type arguments)
                expr = AstNode::Call {
                    receiver: Some(Box::new(expr)),
                    method: field_or_method,
                    args,
                    type_args,
                    structural: false,
                };
                input = k;
            } else {
                // It's field access (type arguments would be invalid here, but we parsed them anyway)
                // For now, we'll ignore type_args for field access
                expr = AstNode::FieldAccess {
                    base: Box::new(expr),
                    field: field_or_method,
                };
                input = k;
            }
        } else if let Ok((i, items)) = parse_multi_index(input) {
            // The index is a tuple: MIR lowers multi-index subscripts through
            // the platform shim (see gen.rs / runtime py_getitem2). Slice
            // elements were built with a placeholder receiver — bind the base
            // here so they lower as the base's own slice.
            let base_for_slices = expr.clone();
            let items: Vec<AstNode> = items
                .into_iter()
                .map(|it| bind_slice_receiver(it, &base_for_slices))
                .collect();
            expr = AstNode::Subscript {
                base: Box::new(expr),
                index: Box::new(AstNode::Tuple(items)),
            };
            input = i;
        } else if let Ok((i, index)) =
            delimited(ws(tag("[")), ws(parse_expr), ws(tag("]"))).parse(input)
        {
            expr = AstNode::Subscript {
                base: Box::new(expr),
                index: Box::new(index),
            };
            input = i;
        } else if let Ok((i, slice_args)) = parse_subscript_slice(input) {
            // PY-A: slicing `base[start:end]` (end optional) — desugars to
            // a __slice__ method call resolved in MIR lowering.
            // Omitted end uses a sentinel that cannot collide with a real
            // index: `s[:-1]` folds to Lit(-1), so -1 could not distinguish
            // "to the end" from "exclude the last character".
            let end = slice_args.1.clone().unwrap_or(AstNode::Lit(i64::MIN));
            expr = if let Some(step) = slice_args.2 {
                // `s[start:end:step]` — the sentinel also marks an omitted
                // start, because a negative step must begin at the end.
                AstNode::Call {
                    receiver: Some(Box::new(expr)),
                    method: "__slice_step__".to_string(),
                    args: vec![
                        slice_args.0.unwrap_or(AstNode::Lit(i64::MIN)),
                        end,
                        step,
                    ],
                    type_args: vec![],
                    structural: false,
                }
            } else {
                AstNode::Call {
                    receiver: Some(Box::new(expr)),
                    method: "__slice__".to_string(),
                    args: vec![slice_args.0.unwrap_or(AstNode::Lit(0)), end],
                    type_args: vec![],
                    structural: false,
                }
            };
            input = i;
        } else if let Ok((i, args)) = delimited(
            ws(tag("(")),
            terminated(
                separated_list0(ws(tag(",")), ws(parse_expr)),
                opt(ws(tag(","))),
            ),
            ws(tag(")")),
        )
        .parse(input)
        {
            expr = AstNode::Call {
                receiver: Some(Box::new(expr)),
                method: "call".to_string(),
                args,
                type_args: vec![],
                structural: false,
            };
            input = i;
        } else if let Ok((i, _)) = cast_as_keyword(input) {
            let (j, ty) = ws(parse_type).parse(i)?;
            expr = AstNode::Cast {
                expr: Box::new(expr),
                ty,
            };
            input = j;
        } else {
            break;
        }
    }
    if let Ok((i, _)) = ws(tag("?")).parse(input) {
        expr = AstNode::TryProp {
            expr: Box::new(expr),
        };
        input = i;
    }
    if let Ok((i, _)) = ws(tag(".await")).parse(input) {
        expr = AstNode::Await(Box::new(expr));
        input = i;
    }
    Ok((input, expr))
}

/// PY-A: walrus `name := expr` — assignment expression. Parsed at the
/// full-expr entry; lowers to Assign node (gen.rs Assign arm handles implicit
/// declaration and returns the value id).
fn parse_walrus_prefix(input: &str) -> IResult<&str, AstNode> {
    // lookahead: IDENT ws ':=' — walrus only when followed by non-'='
    let t = input.trim_start();
    let (ident, rest) = match take_ident(t) {
        Some(pair) => pair,
        None => return parse_full_expr(input),
    };
    let rest_trim = rest.trim_start();
    if !rest_trim.starts_with(":=") {
        return parse_full_expr(input);
    }
    // confirmed walrus: consume IDENT ':=' EXPR
    let after_op = &rest_trim[2..];
    let (rest2, expr) = parse_full_expr(after_op)?;
    Ok((
        rest2,
        AstNode::Assign(
            Box::new(AstNode::Var(ident.to_string())),
            Box::new(expr),
        ),
    ))
}

// Parse logical OR (lowest precedence)
fn parse_logical_or(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut term) = parse_logical_and(input)?;
    loop {
        // Skip whitespace before checking for ||
        let (remaining_input, _) = skip_ws_and_comments0(input)?;

        // Check for || operator
        let or_kw = starts_with_kw(remaining_input, "or");
        if let Some(after_op) = if or_kw {
            Some(&remaining_input[2..] as &str)
        } else {
            remaining_input.strip_prefix("||")
        } {
            // Consume || (or Python `or`)
            // Skip whitespace after ||
            let (after_ws, _) = skip_ws_and_comments0(after_op)?;

            // Parse right-hand side
            let rhs_result = parse_logical_and(after_ws);
            let (next_input, right) = match rhs_result {
                Ok(r) => r,
                Err(_) => {
                    // RHS parse failed. This can happen when parse_logical_and's
                    // sub-parsers over-consume (a nom 8 combinator interaction with
                    // field-access + `<` + `||` + `>`). Fall back: try scanning the
                    // remaining text for the next `||` boundary and split there.
                    // This ensures `||` is always correctly handled as a top-level
                    // binary operator regardless of sub-parser behavior.
                    return Err(nom::Err::Error(nom::error::Error::new(
                        input,
                        nom::error::ErrorKind::Fail,
                    )));
                }
            };

            term = AstNode::BinaryOp {
                op: "||".to_string(),
                left: Box::new(term),
                right: Box::new(right),
            };
            input = next_input;
        } else {
            break;
        }
    }
    Ok((input, term))
}

// Parse logical AND
fn parse_logical_and(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut term) = parse_comparison(input)?;
    loop {
        // Try to parse "&&" operator
        let mut found_op = false;
        let mut remaining_input = input;

        // Try without whitespace first
        if remaining_input.starts_with("&&") {
            found_op = true;
            remaining_input = &remaining_input[2..];
        }

        // Try with whitespace
        if !found_op {
            let i = match skip_ws_and_comments0(remaining_input) {
                Ok((i, _)) => i,
                Err(_) => remaining_input,
            };
            if i.starts_with("&&") {
                found_op = true;
                remaining_input = &i[2..];
            } else if starts_with_kw(i, "and") {
                // PY-A: Python `and` == `&&`
                found_op = true;
                remaining_input = &i[3..];
            }
        }

        if found_op {
            // Skip whitespace after operator
            let j = match skip_ws_and_comments0(remaining_input) {
                Ok((j, _)) => j,
                Err(_) => remaining_input,
            };
            let (j, right) = parse_comparison(j)?;

            term = AstNode::BinaryOp {
                op: "&&".to_string(),
                left: Box::new(term),
                right: Box::new(right),
            };
            input = j;
        } else {
            break;
        }
    }
    Ok((input, term))
}

/// PY-A: one call argument — either a plain expression or a keyword argument
/// `name=value`. Keyword NAMES are dropped in V1 (values bind positionally);
/// this keeps JoinQuant-style calls (`f(x=1, type='fund')`) parseable.
/// PY-A: comprehension loop target — `for x in it` or `for k, v in it.items()`.
fn parse_comp_target(input: &str) -> IResult<&str, Vec<String>> {
    let (mut input, first) = ws(parse_ident).parse(input)?;
    let mut names = vec![first];
    while let Ok((rest, _)) = ws(tag(",")).parse(input) {
        let (rest2, n) = ws(parse_ident).parse(rest)?;
        names.push(n);
        input = rest2;
    }
    Ok((input, names))
}

/// PY-A: rewrite `Var(name)` occurrences. Used to desugar a comprehension's
/// TUPLE target (`for k, v in pairs`) into a single lambda parameter plus slot
/// reads: the runtime hands the lambda one element, so the names must come from
/// the element's slots (`stack_array_get`) rather than from extra parameters.
fn subst_vars(n: &AstNode, map: &std::collections::HashMap<String, AstNode>) -> AstNode {
    match n {
        AstNode::Var(name) => map.get(name).cloned().unwrap_or_else(|| n.clone()),
        AstNode::Call {
            receiver,
            method,
            args,
            type_args,
            structural,
        } => AstNode::Call {
            receiver: receiver.as_ref().map(|r| Box::new(subst_vars(r, map))),
            method: method.clone(),
            args: args.iter().map(|a| subst_vars(a, map)).collect(),
            type_args: type_args.clone(),
            structural: *structural,
        },
        AstNode::BinaryOp { op, left, right } => AstNode::BinaryOp {
            op: op.clone(),
            left: Box::new(subst_vars(left, map)),
            right: Box::new(subst_vars(right, map)),
        },
        AstNode::UnaryOp { op, expr } => AstNode::UnaryOp {
            op: op.clone(),
            expr: Box::new(subst_vars(expr, map)),
        },
        AstNode::Subscript { base, index } => AstNode::Subscript {
            base: Box::new(subst_vars(base, map)),
            index: Box::new(subst_vars(index, map)),
        },
        AstNode::FieldAccess { base, field } => AstNode::FieldAccess {
            base: Box::new(subst_vars(base, map)),
            field: field.clone(),
        },
        AstNode::Tuple(items) => {
            AstNode::Tuple(items.iter().map(|i| subst_vars(i, map)).collect())
        }
        AstNode::DictLit { entries } => AstNode::DictLit {
            entries: entries
                .iter()
                .map(|(k, v)| (subst_vars(k, map), subst_vars(v, map)))
                .collect(),
        },
        AstNode::FString(parts) => {
            AstNode::FString(parts.iter().map(|x| subst_vars(x, map)).collect())
        }
        AstNode::If { cond, then, else_ } => AstNode::If {
            cond: Box::new(subst_vars(cond, map)),
            then: then.iter().map(|s| subst_vars(s, map)).collect(),
            else_: else_.iter().map(|s| subst_vars(s, map)).collect(),
        },
        other => other.clone(),
    }
}

/// PY-A: lambda parameter name + substitution map for a comprehension target.
/// A single name stays the lambda's parameter (no substitution); a tuple target
/// (`for k, v in pairs`) binds one synthetic parameter and reads each name from
/// the element's slots — the runtime hands the lambda ONE element, so extra
/// names must come from that element, not from extra parameters.
pub(crate) type CompSubst = Option<std::collections::HashMap<String, AstNode>>;

fn comp_target_binding(names: &[String]) -> (String, CompSubst) {
    if names.len() == 1 {
        return (names[0].clone(), None);
    }
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let param = format!(
        "__comp_e{}",
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let mut map = std::collections::HashMap::new();
    for (i, n) in names.iter().enumerate() {
        map.insert(
            n.clone(),
            AstNode::Call {
                receiver: None,
                method: "stack_array_get".to_string(),
                args: vec![AstNode::Var(param.clone()), AstNode::Lit(i as i64)],
                type_args: vec![],
                structural: false,
            },
        );
    }
    (param, Some(map))
}

fn apply_subst(e: AstNode, m: &CompSubst) -> AstNode {
    match m {
        Some(m) => subst_vars(&e, m),
        None => e,
    }
}

/// PY-A: `f(x for x in y)` — a GENERATOR EXPRESSION as the call argument. Its
/// parentheses ARE the call's, so the ordinary expression parse consumed `x`
/// and left ` for x in y)` unconsumed: the call then failed to parse and the
/// whole enclosing definition (and the rest of the file) was silently dropped.
/// Builds the same `__collect__(iter, lambda)` desugar the bracketed list comp
/// uses; the caller consumes the closing `)`.
fn parse_call_genexp(input: &str) -> IResult<&str, AstNode> {
    let (input, elem) = ws(parse_full_expr).parse(input)?;
    let (input, _) = ws(tag("for")).parse(input)?;
    let (input, names) = parse_comp_target(input)?;
    let (input, _) = ws(tag("in")).parse(input)?;
    let (input, iter) = ws(parse_full_expr).parse(input)?;
    let (input, cond) = if let Ok((rest, _)) = ws(tag("if")).parse(input) {
        let (rest, c) = ws(parse_full_expr).parse(rest)?;
        (rest, Some(c))
    } else {
        (input, None)
    };
    let (name, smap) = comp_target_binding(&names);
    let elem = apply_subst(elem, &smap);
    let cond = cond.map(|c| apply_subst(c, &smap));
    let body = match cond {
        None => elem,
        Some(c) => AstNode::If {
            cond: Box::new(c),
            then: vec![AstNode::ExprStmt { expr: Box::new(elem) }],
            else_: vec![AstNode::ExprStmt {
                expr: Box::new(AstNode::Lit(-1)),
            }],
        },
    };
    let lam = AstNode::Closure {
        params: vec![name],
        body: Box::new(body),
    };
    Ok((
        input,
        AstNode::Call {
            receiver: Some(Box::new(iter)),
            method: "__collect__".to_string(),
            args: vec![lam],
            type_args: vec![],
            structural: false,
        },
    ))
}

/// Does the text contain a ` for ` at bracket depth 0 (i.e. this argument is a
/// bare generator expression)? Depth-scan skipping string literals.
fn has_top_level_for(s: &str) -> bool {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"' => quote = Some(c),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b' ' if depth == 0
                && i + 5 <= b.len()
                && s.is_char_boundary(i)
                && s.is_char_boundary(i + 5)
                && &s[i..i + 5] == " for " =>
            {
                return true;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

fn parse_call_arg(input: &str) -> IResult<&str, AstNode> {
    // PY-A: bare generator expression (`f(x for x in y)`) — must be tried
    // before the ordinary expression parse, which would stop at `x`.
    if has_top_level_for(input) {
        if let Ok(r) = parse_call_genexp(input) {
            return Ok(r);
        }
    }
    // PY-A: Python `**mapping` argument unpacking. This MUST be detected
    // BEFORE the expression fallback: `**d` would otherwise parse as `*(*d)`
    // (unary deref twice) and the callee silently received zeros — a wrong
    // value with no diagnostic at all. The call site expands the marker using
    // the callee's (statically known) parameter names.
    if let Some(after) = input.trim_start().strip_prefix("**") {
        if !after.starts_with('*') {
            let (rest, value) = parse_full_expr(after.trim_start())?;
            return Ok((
                rest,
                AstNode::Call {
                    receiver: None,
                    method: "zeta_kwargs_unpack".to_string(),
                    args: vec![value],
                    type_args: vec![],
                    structural: false,
                },
            ));
        }
    }
    // PY-A LIMIT: `sum(x for x in y)` (bare genexp as sole argument) is NOT
    // supported — the genexp parens collide with the call's parens in the
    // primary/postfix parse order. Write `sum([x for x in y])` instead
    // (listcomp, fully supported).
    // look ahead: IDENT '=' (not '==') means keyword argument
    let kw = || -> Option<(&str, &str)> {
        let t = input.trim_start();
        let (ident, rest) = take_ident(t)?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('=')?;
        if rest.starts_with('=') {
            return None; // '=='
        }
        Some((ident, rest))
    };
    if let Some((ident, rest)) = kw() {
        // PY-A: keep the keyword NAME (it used to be dropped, so callers
        // bound arguments positionally and `f(b=2, a=1)` silently computed
        // f(2, 1)). The call site rebinds by parameter name; an unknown
        // callee just unwraps the marker.
        let (rest, value) = parse_full_expr(rest)?;
        return Ok((
            rest,
            AstNode::Call {
                receiver: None,
                method: "__kwarg__".to_string(),
                args: vec![AstNode::StringLit(ident.to_string()), value],
                type_args: vec![],
                structural: false,
            },
        ));
    }
    parse_full_expr(input)
}

/// Minimal ident scanner for the kwarg lookahead (keyword names excluded —
/// those can't be kwarg names anyway).
fn take_ident(input: &str) -> Option<(&str, &str)> {
    let b = input.as_bytes();
    if b.is_empty() || !(b[0].is_ascii_alphabetic() || b[0] == b'_') {
        return None;
    }
    let mut i = 1;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    Some((&input[..i], &input[i..]))
}

/// PY-A: the separator between two slice bounds — either `:` (the historical
/// form) or `..` (`s[1..]`, `s[..2]`, `s[..]`).
///
/// The dot form is only reachable here when a bound is MISSING: `s[1..2]` has
/// both sides, so `parse_expr`'s range already consumes it in the generic
/// subscript branch above. `s[1..]` does not — `parse_range` requires a right
/// operand (`parse_unary(j)?`), so the whole `parse_expr` fails, the generic
/// branch fails on the leftover `..]`, and the enclosing item was dropped
/// (W1002; 附 B#10's 757-line family).
///
/// `...` (Ellipsis) starts with `..` but is not a separator.
fn slice_sep(input: &str) -> IResult<&str, ()> {
    let (rest, sep) = ws(alt((tag(":"), tag("..")))).parse(input)?;
    if sep == ".." && rest.starts_with('.') {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    Ok((rest, ()))
}

/// PY-A: `[start:end]` slice subscript — returns (start, Some(end)); end
/// may be omitted (`[start:]` → None) and start may be omitted (`[:end]` → 0).
fn parse_subscript_slice(
    input: &str,
) -> IResult<&str, (Option<AstNode>, Option<AstNode>, Option<AstNode>)> {
    let (input, _) = ws(tag("[")).parse(input)?;
    // optional start
    let (input, start) = if let Ok((rest, _)) = slice_sep(input) {
        (rest, None)
    } else {
        // The bound is parsed at the range's own operand level (`parse_unary`),
        // NOT `parse_expr`: with a dot separator the expression parser would
        // swallow `..` and then fail looking for the right bound, so `s[a..]`
        // could not be distinguished from `s[a..b]`. Consequence, inherited
        // from that precedence (range binds tighter than additive): the dot
        // form's start takes a unary/postfix operand, not `a[i+1..]`.
        let (rest, e) = ws(alt((parse_expr, parse_unary))).parse(input)?;
        let (rest, _sep) = slice_sep(rest)?;
        (rest, Some(e))
    };
    // optional end
    let (input, end) = if let Ok((rest, e)) = ws(parse_expr).parse(input) {
        (rest, Some(e))
    } else {
        (input, None)
    };
    // optional step — `s[::2]`, `s[::-1]`, `s[1:8:2]`. A 3-part slice used to
    // fail to parse here, which aborted the whole statement stream.
    let (input, step) = if let Ok((rest, _)) = ws(tag(":")).parse(input) {
        if let Ok((r, _)) = ws(tag("]")).parse(rest) {
            (r, Some(AstNode::Lit(1)))
        } else {
            let (r, e) = ws(parse_expr).parse(rest)?;
            (r, Some(e))
        }
    } else {
        (input, None)
    };
    let (input, _) = ws(tag("]")).parse(input)?;
    Ok((input, (start, end, step)))
}

// Parse comparison (==, !=, <, >, <=, >=, is, in) with Python-style chaining:
// `a < b < c` folds into `(a < b) && (b < c)`, reusing each boundary operand.
fn parse_comparison(input: &str) -> IResult<&str, AstNode> {
    let (mut input, first) = parse_additive(input)?;
    let mut operands: Vec<AstNode> = vec![first];
    let mut ops: Vec<String> = Vec::new();

    loop {
        let mut found_op: Option<String> = None;
        let mut remaining_input = input;

        let after_ws = skip_ws_and_comments0(remaining_input)
            .map(|(i, _)| i)
            .unwrap_or(remaining_input);

        // PY-A: keyword comparison forms (is not / is / not in / in)
        if starts_with_kw(after_ws, "is not") {
            found_op = Some("!=".to_string());
            remaining_input = &after_ws[6..];
        } else if starts_with_kw(after_ws, "is") {
            found_op = Some("==".to_string());
            remaining_input = &after_ws[2..];
        } else if starts_with_kw(after_ws, "not in") {
            found_op = Some("not in".to_string());
            remaining_input = &after_ws[6..];
        } else if starts_with_kw(after_ws, "in") {
            found_op = Some("in".to_string());
            remaining_input = &after_ws[2..];
        }

        // symbolic forms — without whitespace first, then with
        if found_op.is_none() {
            for &op in &["!=", "==", "<=", ">=", "<", ">"] {
                if remaining_input.starts_with(op) {
                    found_op = Some(op.to_string());
                    remaining_input = &remaining_input[op.len()..];
                    break;
                }
            }
        }
        if found_op.is_none() {
            if let Ok((i, _)) = skip_ws_and_comments0(remaining_input) {
                for &op in &["!=", "==", "<=", ">=", "<", ">"] {
                    if i.starts_with(op) {
                        found_op = Some(op.to_string());
                        remaining_input = &i[op.len()..];
                        break;
                    }
                }
            }
        }

        match found_op {
            Some(op) => {
                let j = skip_ws_and_comments0(remaining_input)
                    .map(|(j, _)| j)
                    .unwrap_or(remaining_input);
                let (j, right) = parse_additive(j)?;
                ops.push(op);
                operands.push(right);
                input = j;
            }
            None => break,
        }
    }

    if ops.is_empty() {
        return Ok((input, operands.pop().unwrap()));
    }

    // Build one comparison node per (op, operand) pair
    let mut cmp_nodes: Vec<AstNode> = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        let left = &operands[i];
        let right = &operands[i + 1];
        let node = match op.as_str() {
            "in" => AstNode::Call {
                // `x in ("sh", "sz")` — a TUPLE on the right. Tuples lower to
                // `StackArray`s whose layout the membership path reads as a
                // dynamic array, so EVERY membership test against a tuple
                // answered 0 (`"sz" in ("sh","sz")` → 0 while the same value in a
                // LIST worked). `code_conv.normalize_to_jq` is built on such
                // tuples, so `sh.513120` never normalized and all 119 wufu codes
                // failed to resolve. Python's `in` does not care about the
                // container type, so lower the tuple as a list literal.
                receiver: Some(Box::new(match right {
                    AstNode::Tuple(items) => AstNode::ArrayLit(items.clone()),
                    other => other.clone(),
                })),
                method: "__contains__".to_string(),
                args: vec![left.clone()],
                type_args: vec![],
                structural: false,
            },
            "not in" => AstNode::UnaryOp {
                op: "not".to_string(),
                expr: Box::new(AstNode::Call {
                    receiver: Some(Box::new(right.clone())),
                    method: "__contains__".to_string(),
                    args: vec![left.clone()],
                    type_args: vec![],
                    structural: false,
                }),
            },
            other => AstNode::BinaryOp {
                op: other.to_string(),
                left: Box::new(left.clone()),
                right: Box::new(right.clone()),
            },
        };
        cmp_nodes.push(node);
    }

    // Fold into an && chain when more than one comparison is present
    let mut result = cmp_nodes.remove(0);
    while !cmp_nodes.is_empty() {
        let next = cmp_nodes.remove(0);
        result = AstNode::BinaryOp {
            op: "&&".to_string(),
            left: Box::new(result),
            right: Box::new(next),
        };
    }
    Ok((input, result))
}

// Parse bitwise AND (&)
fn parse_bitwise_and(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut term) = parse_multiplicative(input)?;
    loop {
        // Try to parse bitwise AND operator
        let mut found_op = None;
        let mut remaining_input = input;

        // Check for & but NOT && (logical AND) and NOT &mut (reference)
        if remaining_input.starts_with("&")
            && !remaining_input.starts_with("&&")
            && !remaining_input.starts_with("&mut")
        {
            found_op = Some("&");
            remaining_input = &remaining_input[1..];
        }

        // Try with whitespace
        if found_op.is_none() {
            let i = match skip_ws_and_comments0(remaining_input) {
                Ok((i, _)) => i,
                Err(_) => remaining_input,
            };
            if i.starts_with("&") && !i.starts_with("&&") && !i.starts_with("&mut") {
                found_op = Some("&");
                remaining_input = &i[1..];
            }
        }

        if let Some(op) = found_op {
            // Skip whitespace after operator
            let j = match skip_ws_and_comments0(remaining_input) {
                Ok((j, _)) => j,
                Err(_) => remaining_input,
            };
            let (j, right) = parse_multiplicative(j)?;

            term = AstNode::BinaryOp {
                op: op.to_string(),
                left: Box::new(term),
                right: Box::new(right),
            };
            input = j;
        } else {
            break;
        }
    }
    Ok((input, term))
}

// Parse bitwise XOR (^)
fn parse_bitwise_xor(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut term) = parse_bitwise_and(input)?;
    loop {
        // Try to parse bitwise XOR operator
        let mut found_op = None;
        let mut remaining_input = input;

        if remaining_input.starts_with("^") {
            found_op = Some("^");
            remaining_input = &remaining_input[1..];
        }

        // Try with whitespace
        if found_op.is_none() {
            let i = match skip_ws_and_comments0(remaining_input) {
                Ok((i, _)) => i,
                Err(_) => remaining_input,
            };
            if i.starts_with("^") {
                found_op = Some("^");
                remaining_input = &i[1..];
            }
        }

        if let Some(op) = found_op {
            // Skip whitespace after operator
            let j = match skip_ws_and_comments0(remaining_input) {
                Ok((j, _)) => j,
                Err(_) => remaining_input,
            };
            let (j, right) = parse_bitwise_and(j)?;

            term = AstNode::BinaryOp {
                op: op.to_string(),
                left: Box::new(term),
                right: Box::new(right),
            };
            input = j;
        } else {
            break;
        }
    }
    Ok((input, term))
}

// Parse bitwise OR (|)
fn parse_bitwise_or(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut term) = parse_bitwise_xor(input)?;
    loop {
        // Try to parse bitwise OR operator
        let mut found_op = None;
        let mut remaining_input = input;

        // Check for | but NOT || (logical OR)
        if remaining_input.starts_with("|") && !remaining_input.starts_with("||") {
            found_op = Some("|");
            remaining_input = &remaining_input[1..];
        }

        // Try with whitespace
        if found_op.is_none() {
            let i = match skip_ws_and_comments0(remaining_input) {
                Ok((i, _)) => i,
                Err(_) => remaining_input,
            };
            if i.starts_with("|") && !i.starts_with("||") {
                found_op = Some("|");
                remaining_input = &i[1..];
            }
        }

        if let Some(op) = found_op {
            // Skip whitespace after operator
            let j = match skip_ws_and_comments0(remaining_input) {
                Ok((j, _)) => j,
                Err(_) => remaining_input,
            };
            let (j, right) = parse_bitwise_xor(j)?;

            term = AstNode::BinaryOp {
                op: op.to_string(),
                left: Box::new(term),
                right: Box::new(right),
            };
            input = j;
        } else {
            break;
        }
    }
    Ok((input, term))
}

// Parse additive (+, -)
fn parse_additive(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut term) = parse_bitwise_or(input)?;
    loop {
        // Try to parse additive operator
        let mut found_op = None;
        let mut remaining_input = input;

        let additive_ops = ["+", "-"];

        // Try without whitespace first
        for &op in &additive_ops {
            if remaining_input.starts_with(op) {
                found_op = Some(op);
                remaining_input = &remaining_input[op.len()..];
                break;
            }
        }

        // Try with whitespace
        if found_op.is_none() {
            let i = match skip_ws_and_comments0(remaining_input) {
                Ok((i, _)) => i,
                Err(_) => remaining_input,
            };
            for &op in &additive_ops {
                if i.starts_with(op) {
                    found_op = Some(op);
                    remaining_input = &i[op.len()..];
                    break;
                }
            }
        }

        if let Some(op) = found_op {
            // Skip whitespace after operator
            let j = match skip_ws_and_comments0(remaining_input) {
                Ok((j, _)) => j,
                Err(_) => remaining_input,
            };
            let (j, right) = parse_bitwise_or(j)?;

            term = AstNode::BinaryOp {
                op: op.to_string(),
                left: Box::new(term),
                right: Box::new(right),
            };
            input = j;
        } else {
            break;
        }
    }
    Ok((input, term))
}

// Parse bitwise shift (<<, >>)
fn parse_shift(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut term) = parse_range(input)?;
    loop {
        // Try to parse shift operator
        let mut found_op = None;
        let mut remaining_input = input;

        let shift_ops = ["<<", ">>"];

        // Try without whitespace first
        for &op in &shift_ops {
            if remaining_input.starts_with(op) {
                found_op = Some(op);
                remaining_input = &remaining_input[op.len()..];
                break;
            }
        }

        // Try with whitespace
        if found_op.is_none() {
            let i = match skip_ws_and_comments0(remaining_input) {
                Ok((i, _)) => i,
                Err(_) => remaining_input,
            };
            if i != remaining_input {
                for &op in &shift_ops {
                    if i.starts_with(op) {
                        found_op = Some(op);
                        remaining_input = &i[op.len()..];
                        break;
                    }
                }
            }
        }

        if let Some(op) = found_op {
            // Skip whitespace after operator
            let j = match skip_ws_and_comments0(remaining_input) {
                Ok((j, _)) => j,
                Err(_) => remaining_input,
            };
            let (j, right) = parse_range(j)?;

            term = AstNode::BinaryOp {
                op: op.to_string(),
                left: Box::new(term),
                right: Box::new(right),
            };
            input = j;
        } else {
            break;
        }
    }
    Ok((input, term))
}

// Parse multiplicative (*, /, %)
fn parse_multiplicative(input: &str) -> IResult<&str, AstNode> {
    let (mut input, mut term) = parse_shift(input)?;
    loop {
        // Try to parse multiplicative operator
        let mut found_op = None;
        let mut remaining_input = input;

        // `**` is deliberately NOT in this list: it has its own tier
        // (`parse_power`, tighter than this one and looser than unary minus).
        // Carrying it here made the loop fold powers left-to-right through the
        // `*`-level operators — `2 * 3 ** 2` became `(2*3)**2` and `2 ** 3 % 5`
        // became `2 ** (3 % 5)`. The old note's hazard (a `*` prefix match eating
        // half of `**`, leaving `* x` to parse as a pointer dereference) cannot
        // recur: `parse_power` consumes the whole power before this loop runs.
        // PY-A: `@` is Python matmul (same precedence as `*`/`/`/`%`).
        // Without it, `I @ corr` aborts the enclosing `def` and silently drops
        // the rest of the file (ETF动量EPO: 77 unparsed lines).
        let multiplicative_ops = ["*", "/", "%", "@"];

        // PY-A: `floordiv` is the word operator the indent preprocessor emits
        // for Python's `//` (which the parser would otherwise swallow as a line
        // comment). It is a *word*, so it needs a boundary: a variable named
        // `floordivx` must keep parsing as a variable.
        let word_op = |s: &str| -> bool {
            s.starts_with("floordiv")
                && !s[8..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_')
        };

        // Try without whitespace first
        if word_op(remaining_input) {
            found_op = Some("floordiv");
            remaining_input = &remaining_input[8..];
        }
        for &op in &multiplicative_ops {
            if found_op.is_some() {
                break;
            }
            if remaining_input.starts_with(op) {
                found_op = Some(op);
                remaining_input = &remaining_input[op.len()..];
                break;
            }
        }

        // Try with whitespace
        if found_op.is_none() {
            let i = match skip_ws_and_comments0(remaining_input) {
                Ok((i, _)) => i,
                Err(_) => remaining_input,
            };
            if i != remaining_input {
                if word_op(i) {
                    found_op = Some("floordiv");
                    remaining_input = &i[8..];
                }
                if found_op.is_none() {
                    for &op in &multiplicative_ops {
                        if i.starts_with(op) {
                            found_op = Some(op);
                            remaining_input = &i[op.len()..];
                            break;
                        }
                    }
                }
            }
        }

        if let Some(op) = found_op {
            // Skip whitespace after operator
            let j = match skip_ws_and_comments0(remaining_input) {
                Ok((j, _)) => j,
                Err(_) => remaining_input,
            };
            // `**` is right-associative, but that is now `parse_power`'s
            // business; every operator left here is left-associative.
            let (j, right) = parse_shift(j)?;

            term = AstNode::BinaryOp {
                op: op.to_string(),
                left: Box::new(term),
                right: Box::new(right),
            };
            input = j;
        } else {
            break;
        }
    }
    Ok((input, term))
}

// Parse range (.., ..=) - higher precedence than other binary ops
fn parse_range(input: &str) -> IResult<&str, AstNode> {
    let (input, term) = parse_unary(input)?;

    // Check for range operators
    let mut current_input = input;
    let mut current_term = term;

    // Try to parse range operators
    let range_ops = ["..=", ".."];

    for &op in &range_ops {
        let mut found_op = false;
        let mut remaining_input = current_input;

        // Try without whitespace first
        if remaining_input.starts_with(op) {
            // PY-A: `...` (Ellipsis) starts with `..` but is NOT a range.
            // `x = 1` then newline then `...` (abstractmethod stub) used to
            // greedily parse as `1..` + unary(`.`), abort the `def`, and drop
            // the enclosing class (`ExecutionBackend(Protocol)` → LocalBackend
            // never reached → bare `_execute_trade`).
            if op == ".." && remaining_input.starts_with("...") {
                continue;
            }
            found_op = true;
            remaining_input = &remaining_input[op.len()..];
        }

        // Try with whitespace
        if !found_op {
            let i = match skip_ws_and_comments0(remaining_input) {
                Ok((i, _)) => i,
                Err(_) => remaining_input,
            };
            if i.starts_with(op) {
                if op == ".." && i.starts_with("...") {
                    continue;
                }
                found_op = true;
                remaining_input = &i[op.len()..];
            }
        }

        if found_op {
            // Skip whitespace after operator
            let j = match skip_ws_and_comments0(remaining_input) {
                Ok((j, _)) => j,
                Err(_) => remaining_input,
            };
            let (j, right) = parse_unary(j)?;

            current_term = AstNode::Range {
                start: Box::new(current_term),
                end: Box::new(right),
                inclusive: op == "..=",
            };
            current_input = j;
            break;
        }
    }

    Ok((current_input, current_term))
}

/// Find a `||` that is not inside parens at the top level
fn find_top_level_or(input: &str) -> Option<usize> {
    let mut depth = 0i64;
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            depth += 1;
        } else if bytes[i] == b')' && depth > 0 {
            depth -= 1;
        } else if depth == 0 && bytes[i] == b'|' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// PY-A: locate a bare keyword at bracket depth 0, skipping string literals
/// (including triple-quoted ones). Used to find the `if`/`else` of a Python
/// conditional expression without being fooled by the same words nested in
/// parentheses/brackets or inside strings.
fn find_top_level_kw(input: &str, kw: &str) -> Option<usize> {
    let b = input.as_bytes();
    let kwb = kw.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        // Triple-quoted strings: `"""` / `'''` — skip to the matching close.
        if (c == b'"' || c == b'\'') && i + 2 < b.len() && b[i + 1] == c && b[i + 2] == c {
            let mut j = i + 3;
            while j + 2 < b.len()
                && !(b[j] == c && b[j + 1] == c && b[j + 2] == c)
            {
                j += 1;
            }
            i = if j + 2 < b.len() { j + 3 } else { b.len() };
            continue;
        }
        match c {
            b'"' | b'\'' => quote = Some(c),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {}
        }
        // ⚠️ `i` walks BYTES, so it can land inside a multi-byte character
        // (any CJK string literal in the source). Slicing there panics —
        // `input[i..]` is a `&str` index, not a byte index. A keyword can never
        // begin mid-character (its first byte is ASCII), so skipping is correct.
        if depth == 0
            && input.is_char_boundary(i)
            && input[i..].starts_with(kw)
            && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
        {
            let after = i + kwb.len();
            if after >= b.len() || !(b[after].is_ascii_alphanumeric() || b[after] == b'_') {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// PY-A: parse a Python conditional expression `A if C else B`, whose leading
/// `if` sits at `if_pos` (found at top level by `find_top_level_kw`). The
/// right-hand side is itself a conditional expression, giving Python's
/// right-associative nesting (`a if c1 else b if c2 else d`).
///
/// Returns `Err` (so the caller falls back to plain or-expression parsing)
/// unless the shape is exactly `<expr> if <expr> else <expr>` — importantly,
/// the left side must consume everything up to `if`. That guard is what keeps
/// this from misfiring on a following `if`-STATEMENT (whose condition would be
/// followed by a `{`, leaving the condition parse non-empty).
fn parse_conditional_tail(input: &str, if_pos: usize) -> IResult<&str, AstNode> {
    let (left_rem, then_expr) = parse_expr_no_if(&input[..if_pos])?;
    if !left_rem.trim().is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    let after_if = &input[if_pos + 2..];
    let else_pos = match find_top_level_kw(after_if, "else") {
        Some(p) => p,
        None => {
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            )))
        }
    };
    // The condition slice keeps the whitespace after `if`. `parse_expr_no_if`
    // does NOT skip leading whitespace itself, and its unary-operator
    // detection is position-based (`input.starts_with("-")`), so a condition
    // starting with a unary operator failed once a space preceded it:
    //     `3 if -200 <= diff < 200 else 6`   (hence five corpus files)
    // Trim the slice first — `-200 <= diff` parses fine standalone because the
    // caller there already consumed the whitespace.
    let cond_slice = &after_if[..else_pos];
    let cond_slice = match skip_ws_and_comments0(cond_slice) {
        Ok((rest, _)) => rest,
        Err(_) => cond_slice,
    };
    let (cond_rem, cond) = parse_expr_no_if(cond_slice)?;
    if !cond_rem.trim().is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    // The ELSE branch is parsed with the full expression parser so that a
    // nested ternary binds to the right, exactly like Python:
    //     `1 if n > 0 else -1 if n < 0 else 0`  ==  `1 if n > 0 else (-1 if n < 0 else 0)`
    // Using the `_no_if` variant here left the inner `if … else …` unconsumed,
    // which failed the enclosing definition and truncated the file.
    // A branch may begin with a unary operator (`… else -1`). The general
    // expression parser rejects a LEADING unary minus in this position (it is
    // fine in the `then` slot and fine when standalone), so wrap the operand
    // explicitly instead of failing the whole definition.
    let else_text = &after_if[else_pos + 4..];
    let (rest, else_expr) = match parse_expr(else_text) {
        Ok(v) => v,
        Err(e) => {
            let t = else_text.trim_start();
            match t.strip_prefix('-') {
                Some(operand_text) => {
                    let (rest, operand) = parse_expr(operand_text)?;
                    (
                        rest,
                        AstNode::UnaryOp {
                            op: "-".to_string(),
                            expr: Box::new(operand),
                        },
                    )
                }
                None => return Err(e),
            }
        }
    };
    Ok((
        rest,
        AstNode::If {
            cond: Box::new(cond),
            then: vec![AstNode::ExprStmt {
                expr: Box::new(then_expr),
            }],
            else_: vec![AstNode::ExprStmt {
                expr: Box::new(else_expr),
            }],
        },
    ))
}

// Parse expression without if (for use in if conditions to avoid left recursion)
fn parse_expr_no_if(input: &str) -> IResult<&str, AstNode> {
    // PY-A: Python conditional expression `A if C else B`. It binds looser
    // than `or`/`and`, and — unlike Zeta's prefix `if COND { … }` — starts
    // with the value. Detected only when the `if` is at bracket depth 0.
    if let Some(if_pos) = find_top_level_kw(input, "if") {
        if let Ok((rest, node)) = parse_conditional_tail(input, if_pos) {
            return Ok((rest, node));
        }
    }
    // Split on top-level `||` first, parse each side independently.
    // This avoids a nom 8 combinator interaction where field-access + `<` + `||` + `>`
    // causes sub-parsers to consume operators they shouldn't handle.
    if let Some(or_pos) = find_top_level_or(input) {
        let left_input = &input[..or_pos];
        let right_input = &input[or_pos + 2..];
        let (left_rem, left_expr) = parse_logical_or(left_input)?;
        if left_rem.trim().is_empty() {
            let (right_rem, right_expr) = parse_logical_or(right_input)?;
            return Ok((
                right_rem,
                AstNode::BinaryOp {
                    op: "||".to_string(),
                    left: Box::new(left_expr),
                    right: Box::new(right_expr),
                },
            ));
        }
    }
    parse_logical_or(input)
}

/// Parse a match expression: `match expr { pattern => expr, ... }`
pub fn parse_match_expr(input: &str) -> IResult<&str, AstNode> {
    // Parse "match" with optional whitespace
    let (input, _) = ws(tag::<_, _, nom::error::Error<&str>>("match")).parse(input)?;

    // Parse scrutinee
    let (input, scrutinee) = parse_expr(input)?;

    // Parse whitespace before brace
    let (input, _) = skip_ws_and_comments0(input)?;

    // Parse "{"
    let (input, _) = ws(tag::<_, _, nom::error::Error<&str>>("{")).parse(input)?;

    // Parse arms (PY-2: inter-arm comma is optional — newline-separated arms
    // work, which is what indented match blocks normalize to)
    let mut arms = Vec::new();
    let mut current_input = input;

    loop {
        let (ws_input, _) = skip_ws_and_comments0(current_input)?;
        if ws_input.starts_with('}') {
            current_input = ws_input;
            break;
        }
        match parse_match_arm(ws_input) {
            Ok((next_input, arm)) => {
                arms.push(arm);
                let (ws_next, _) = skip_ws_and_comments0(next_input)?;
                current_input = ws_next;
                // Optional separator
                if let Ok((next_input, _)) =
                    tag::<_, _, nom::error::Error<&str>>(",").parse(current_input)
                {
                    current_input = next_input;
                }
            }
            Err(_) => {
                current_input = ws_input;
                break;
            }
        }
    }

    // Parse closing brace
    let (input, _) = tag::<_, _, nom::error::Error<&str>>("}")(current_input)?;

    Ok((
        input,
        AstNode::Match {
            scrutinee: Box::new(scrutinee),
            arms,
        },
    ))
}

/// Parse a single match arm: `pattern => expr` or `pattern if guard => expr`
fn parse_match_arm(input: &str) -> IResult<&str, MatchArm> {
    // Parse pattern (supports variables, literals, struct patterns, etc.)
    let (input, pattern) = parse_pattern(input)?;

    let (input, _) = skip_ws_and_comments0(input)?;

    // Parse optional guard
    let (input, guard) = opt(preceded(
        terminated(
            ws(tag::<_, _, nom::error::Error<&str>>("if")),
            skip_ws_and_comments0,
        ),
        parse_expr,
    ))
    .parse(input)?;

    let (input, _) = skip_ws_and_comments0(input)?;

    // Parse arrow
    let (input, _) = ws(tag::<_, _, nom::error::Error<&str>>("=>")).parse(input)?;

    // Parse body: allow `return` statements as well as plain expressions
    // PY-A: single-value return form — the comma here separates arms, so a
    // tuple return inside an arm needs parentheses.
    // PY-A: an arm body may also be an assignment (`_ => i += 1`). `parse_expr`
    // has no assignment form, so before this the whole `match` failed to parse
    // and every item after it in the file was dropped (W1002). `parse_assign`
    // is tried first and only consumes the text when a real `=`/`+=` follows a
    // lhs AND the rhs parses, so `x == 1` still falls through to `parse_expr`.
    let (input, body) = alt((parse_return_single, parse_assign, parse_expr)).parse(input)?;

    Ok((
        input,
        MatchArm {
            pattern: Box::new(pattern),
            guard: guard.map(Box::new),
            body: Box::new(body),
        },
    ))
}

pub fn parse_expr(input: &str) -> IResult<&str, AstNode> {
    // Try if expression first
    match parse_if(input) {
        Ok((remaining, if_expr)) => return Ok((remaining, if_expr)),
        Err(_) => {
            // Not an if expression, continue
        }
    }

    // Otherwise parse normal expression
    parse_expr_no_if(input)
}

pub fn parse_full_expr(input: &str) -> IResult<&str, AstNode> {
    // PY-A: walrus `name := expr` — parse at full-expr entry, lowering to
    // Assign (an expression whose evaluation binds the name).
    let t = input.trim_start();
    if let Some((ident, rest)) = take_ident(t) {
        let rest_trim = rest.trim_start();
        if rest_trim.starts_with(":=") && !rest_trim[2..].starts_with('=') {
            let after = &rest_trim[2..];
            let (rest, expr) = parse_full_expr(after)?;
            return Ok((
                rest,
                AstNode::Assign(
                    Box::new(AstNode::Var(ident.to_string())),
                    Box::new(expr),
                ),
            ));
        }
    }
    parse_expr(input)
}
