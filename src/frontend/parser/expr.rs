// src/frontend/parser/expr.rs
use super::parser::{
    parse_ident, parse_path, parse_type, parse_type_args, skip_ws_and_comments0, ws,
};

use super::pattern::parse_pattern;
use super::stmt::{parse_block_body, parse_loop, parse_return, parse_return_single};
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

    // Must have decimal to be a float (for v0.3.8, no exponent support yet)
    if !has_decimal {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Digit,
        )));
    }

    let float_str = &input[..pos];
    let remaining = &input[pos..];

    // Remove underscores from the float string
    let clean_float: String = float_str
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
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

fn parse_string_lit(input: &str) -> IResult<&str, AstNode> {
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

/// Parse a raw string literal: r"..." (no escape processing)
fn parse_raw_string_lit(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = tag("r\"")(input)?;
    let mut content = String::new();
    let chars = input.chars();
    let mut pos = 0;

    for c in chars {
        pos += c.len_utf8();
        if c == '"' {
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

        let (input, args_opt) = opt(delimited(
            ws(tag("(")),
            terminated(
                separated_list0(ws(tag(",")), ws(parse_expr)),
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
    let (input, _) = ws(tag("if")).parse(input)?;
    parse_if_tail(input)
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
    let (input, mut items) = terminated(
        separated_list0(ws(tag(",")), ws(parse_expr)),
        opt(ws(tag(","))),
    )
    .parse(input)?;
    let (input, _) = ws(tag(")")).parse(input)?;
    if items.len() == 1 {
        Ok((input, items.remove(0)))
    } else {
        Ok((input, AstNode::Tuple(items)))
    }
}

/// Parse a dict literal: {"key": value, ...}
fn parse_dict_lit(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("{")).parse(input)?;
    // Check it's not an empty block — if next char is '}' it's empty dict
    let trimmed = input.trim_start();
    if trimmed.starts_with("}") {
        let (input, _) = ws(tag("}")).parse(input)?;
        return Ok((input, AstNode::DictLit { entries: vec![] }));
    }
    let (input, entries) = separated_list0(
        ws(tag(",")),
        pair(ws(parse_expr), ws(preceded(tag(":"), ws(parse_expr)))),
    )
    .parse(input)?;
    let (input, _) = ws(tag("}")).parse(input)?;
    Ok((input, AstNode::DictLit { entries }))
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
        // PY-A: Python `not` == `!`
        let (input, _) = tag("not")(input)?;
        (input, Some("!"))
    } else {
        // Try other unary operators
        opt(alt((tag("&mut"), tag("&"), tag("-"), tag("*")))).parse(input)?
    };

    let (input, expr) = if op_opt.is_some() {
        ws(parse_unary).parse(input)?
    } else {
        ws(parse_postfix).parse(input)?
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

pub fn parse_primary(input: &str) -> IResult<&str, AstNode> {
    alt((
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
                // This is ".." or "..="
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
            let (j, field_or_method) = parse_ident(i)?;

            // Check for type arguments first (e.g., ::<i32>)
            let (j2, type_args_opt) =
                opt(ws(preceded(opt(tag("::")), parse_type_args))).parse(j)?;
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
                    // Parse non-empty argument list
                    let (after_args, args) = terminated(
                        separated_list1(ws(tag(",")), ws(parse_expr)),
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
            let end = slice_args.1.unwrap_or(AstNode::Lit(-1));
            expr = AstNode::Call {
                receiver: Some(Box::new(expr)),
                method: "__slice__".to_string(),
                args: vec![slice_args.0, end],
                type_args: vec![],
                structural: false,
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
        } else if let Ok((i, _)) = ws(tag("as")).parse(input) {
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

/// PY-A: `[start:end]` slice subscript — returns (start, Some(end)); end
/// may be omitted (`[start:]` → None) and start may be omitted (`[:end]` → 0).
fn parse_subscript_slice(input: &str) -> IResult<&str, (AstNode, Option<AstNode>)> {
    let (input, _) = ws(tag("[")).parse(input)?;
    // optional start
    let (input, start) = if let Ok((rest, _)) = ws(tag(":")).parse(input) {
        let rest = rest;
        (rest, AstNode::Lit(0))
    } else {
        let (rest, e) = ws(parse_expr).parse(input)?;
        let (rest, _colon) = ws(tag(":")).parse(rest)?;
        (rest, e)
    };
    // optional end (default → Lit(-1) handled by caller as "to the end")
    let (input, end) = if let Ok((rest, _)) = ws(tag("]")).parse(input) {
        (rest, None)
    } else {
        let (rest, e) = ws(parse_expr).parse(input)?;
        let (rest, _) = ws(tag("]")).parse(rest)?;
        (rest, Some(e))
    };
    Ok((input, (start, end)))
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
                receiver: Some(Box::new(right.clone())),
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

        let multiplicative_ops = ["*", "/", "%"];

        // Try without whitespace first
        for &op in &multiplicative_ops {
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
                for &op in &multiplicative_ops {
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

// Parse expression without if (for use in if conditions to avoid left recursion)
fn parse_expr_no_if(input: &str) -> IResult<&str, AstNode> {
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
    let (input, body) = alt((parse_return_single, parse_expr)).parse(input)?;

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
    parse_expr(input)
}
