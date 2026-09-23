// src/frontend/parser/pattern.rs
//! Module for parsing patterns in the Zeta language.

use super::expr::{parse_lit, parse_string_lit};
use super::parser::{parse_ident, parse_path, skip_ws_and_comments, ws};
use crate::frontend::ast::AstNode;
use nom::IResult;
use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::combinator::map;
use nom::combinator::not;
use nom::combinator::opt;
use nom::combinator::peek;
use nom::sequence::pair;
use nom::multi::separated_list0;
use nom::sequence::{delimited, preceded, terminated};

/// Parse a pattern: `_`, identifier, literal, tuple pattern, struct pattern, range pattern, bind pattern, or or-pattern.
pub fn parse_pattern(input: &str) -> IResult<&str, AstNode> {
    // First parse a basic pattern
    let (input, pattern) = alt((
        // Wildcard pattern — a BARE `_`. Without the boundary check `_code`
        // matched the wildcard `_` and left `code` behind, so every
        // underscore-prefixed loop variable (`for _code, data in …`, the usual
        // Python spelling for an unused name) failed to parse and the enclosing
        // function — plus the rest of the file — was silently dropped.
        map(
            pair(
                tag("_"),
                peek(not(nom::character::complete::satisfy(|c: char| {
                    c.is_ascii_alphanumeric() || c == '_'
                }))),
            ),
            |_| AstNode::Ignore,
        ),
        // Tuple pattern: `(pattern, pattern, ...)`
        parse_tuple_pattern,
        // Bind pattern: `ident @ pattern` — **must** precede the struct/variable
        // pattern below. `parse_struct_pattern` deliberately succeeds on a bare
        // path (its `Ok((input, AstNode::Var(variant)))` fallback at the end of
        // that fn), so for `x @ 1..=10` it consumed `x`, returned `Var("x")` and
        // left `@ …` behind; the arm then had no `=>` and the whole enclosing
        // `fn` — plus the rest of the file — was dropped (W1002, 附 B#10).
        // 故意不写行号：本批插了 7 行注释，注释里的行号当场就漂了。
        parse_bind_pattern,
        // Struct pattern: `Path { field: pattern, ... }` or `Path(pattern, ...)`
        parse_struct_pattern,
        // Range pattern: `start..end` or `start..=end`
        parse_range_pattern,
        // Or pattern: `pattern | pattern | ...`
        parse_or_pattern,
        // Literal pattern — numeric only (`parse_lit` has no string branch).
        parse_lit,
        // String-literal pattern: `"+" => …`, `"a" | "b" => …`. Before this arm
        // existed, a string in any match arm left `"+" => …` unconsumed,
        // `parse_match_arm` never reached its `=>`, and the whole enclosing
        // `fn` — plus the rest of the file — was dropped (W1002, 附 B#10:
        // 757 of the 1,749 lines).
        parse_string_lit,
        // Boolean pattern (for match arms like `true => ...`)
        tag("true").map(|_| AstNode::Bool(true)),
        tag("false").map(|_| AstNode::Bool(false)),
        // Variable pattern
        parse_ident.map(AstNode::Var),
    ))
    .parse(input)?;

    // Collect an or-pattern chain here rather than relying on the `parse_or_pattern`
    // arm above: `parse_struct_pattern` succeeds on a bare path (`A`, `Some(1)`), so
    // for anything but a literal-led arm head it returns after the first
    // alternative and the `| …` text never reaches that arm.
    let (input, tail) =
        nom::multi::many0(preceded(ws(tag("|")), ws(parse_pattern))).parse(input)?;
    let pattern = if tail.is_empty() {
        pattern
    } else {
        let mut all = vec![pattern];
        all.extend(tail);
        AstNode::OrPattern(all)
    };

    // Then check for type annotation
    let (input, ty_opt) = opt(preceded(
        ws(tag(":")),
        ws(crate::frontend::parser::parser::parse_type),
    ))
    .parse(input)?;

    if let Some(ty) = ty_opt {
        Ok((
            input,
            AstNode::TypeAnnotatedPattern {
                pattern: Box::new(pattern),
                ty,
            },
        ))
    } else {
        Ok((input, pattern))
    }
}

/// Parse a tuple pattern: `(pattern, pattern, ...)`
fn parse_tuple_pattern(input: &str) -> IResult<&str, AstNode> {
    delimited(
        ws(tag("(")),
        terminated(
            separated_list0(ws(tag(",")), ws(parse_pattern)),
            opt(ws(tag(","))),
        ),
        ws(tag(")")),
    )
    .map(AstNode::Tuple)
    .parse(input)
}

/// Parse a struct pattern: either tuple struct `Path(pattern, ...)` or named struct `Path { field: pattern, ... }`
fn parse_struct_pattern(input: &str) -> IResult<&str, AstNode> {
    let (input, path) = parse_path(input)?;
    let variant = path.join("::");

    let (input, _) = skip_ws_and_comments(input)?;

    // Try tuple struct pattern first: `Path(pattern, ...)`
    if let Ok((_i, _)) = ws(tag("(")).parse(input) {
        return parse_tuple_struct_pattern(input, variant);
    }

    // Try named struct pattern: `Path { field: pattern, ... }`
    if let Ok((_i, _)) = ws(tag("{")).parse(input) {
        return parse_named_struct_pattern(input, variant);
    }

    // If neither, it's just a variable pattern with a path
    Ok((input, AstNode::Var(variant)))
}

/// Parse a tuple struct pattern: `Variant(pattern, pattern, ...)`
fn parse_tuple_struct_pattern(input: &str, variant: String) -> IResult<&str, AstNode> {
    let (input, pats) = delimited(
        ws(tag("(")),
        terminated(
            separated_list0(ws(tag(",")), ws(parse_pattern)),
            opt(ws(tag(","))),
        ),
        ws(tag(")")),
    )
    .parse(input)?;

    Ok((
        input,
        AstNode::StructPattern {
            variant,
            fields: pats
                .into_iter()
                .enumerate()
                .map(|(i, p)| (i.to_string(), p))
                .collect(),
            rest: false,
        },
    ))
}

/// Parse a named struct pattern: `Struct { field: pattern, ... }`
fn parse_named_struct_pattern(input: &str, variant: String) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("{")).parse(input)?;

    // Parse fields: separated by commas, with optional trailing comma
    let (input, fields) = terminated(
        separated_list0(ws(tag(",")), ws(parse_field_pattern)),
        opt(ws(tag(","))),
    )
    .parse(input)?;

    // Parse optional rest pattern: `..` (with or without trailing comma before `}`)
    let (input, has_rest) =
        opt(alt((preceded(ws(tag(",")), ws(tag(".."))), ws(tag(".."))))).parse(input)?;

    // Parse closing brace
    let (input, _) = ws(tag("}")).parse(input)?;

    Ok((
        input,
        AstNode::StructPattern {
            variant,
            fields,
            rest: has_rest.is_some(),
        },
    ))
}

/// Parse a field in a struct pattern: `field: pattern` or `field` (shorthand)
fn parse_field_pattern(input: &str) -> IResult<&str, (String, AstNode)> {
    let (input, name) = ws(parse_ident).parse(input)?;
    let (input, colon) = opt(ws(tag(":"))).parse(input)?;

    let (input, pat) = if colon.is_some() {
        // Field with explicit pattern: `field: pattern`
        ws(parse_pattern).parse(input)?
    } else {
        // Field shorthand: `field` is equivalent to `field: field`
        (input, AstNode::Var(name.clone()))
    };

    Ok((input, (name, pat)))
}

/// Char literal in pattern position: `'a'` / `'\n'` → its codepoint.
///
/// Single quotes are also ordinary string delimiters in this language
/// (`s.split(',')`), so outside a pattern `'x'` stays a 1-char string. In a
/// pattern it has to be an integer, because that is what a char is in a slot —
/// and `parse_range_pattern`'s endpoints used to be `parse_lit` only, which is
/// what dropped `advanced_patterns_test.z`'s `'a'..='z'` arms whole (W1002).
fn parse_char_lit(input: &str) -> IResult<&str, AstNode> {
    fn fail<'i>(input: &'i str) -> nom::Err<nom::error::Error<&'i str>> {
        nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Char))
    }

    let rest = match input.strip_prefix('\'') {
        Some(r) => r,
        None => return Err(fail(input)),
    };
    let (ch, rest) = match rest.chars().next() {
        Some(c) => (c, &rest[c.len_utf8()..]),
        None => return Err(fail(input)),
    };
    let (code, rest) = if ch != '\\' {
        (ch as i64, rest)
    } else {
        let e = match rest.chars().next() {
            Some(c) => c,
            None => return Err(fail(input)),
        };
        let code = match e {
            'n' => 10,
            'r' => 13,
            't' => 9,
            '0' => 0,
            c @ ('\\' | '\'' | '"') => c as i64,
            _ => return Err(fail(input)),
        };
        (code, &rest[e.len_utf8()..])
    };
    match rest.strip_prefix('\'') {
        Some(r) => Ok((r, AstNode::Lit(code))),
        None => Err(fail(input)),
    }
}

/// Parse a range pattern: `start..end` or `start..=end`
fn parse_range_pattern(input: &str) -> IResult<&str, AstNode> {
    let (input, start) = alt((parse_char_lit, parse_lit)).parse(input)?;
    let (input, _) = ws(tag("..")).parse(input)?;
    let (input, inclusive): (_, Option<&str>) = opt(ws(tag("="))).parse(input)?;
    let (input, end) = alt((parse_char_lit, parse_lit)).parse(input)?;

    Ok((
        input,
        AstNode::RangePattern {
            start: Box::new(start),
            end: Box::new(end),
            inclusive: inclusive.is_some(),
        },
    ))
}

/// Parse a bind pattern: `ident @ pattern`
fn parse_bind_pattern(input: &str) -> IResult<&str, AstNode> {
    let (input, name) = parse_ident(input)?;
    let (input, _) = ws(tag("@")).parse(input)?;
    let (input, pattern) = parse_pattern(input)?;

    Ok((
        input,
        AstNode::BindPattern {
            name,
            pattern: Box::new(pattern),
        },
    ))
}

/// Parse an or-pattern: `pattern | pattern | ...`
fn parse_or_pattern(input: &str) -> IResult<&str, AstNode> {
    let (input, first) = parse_simple_pattern(input)?;
    let (input, patterns) =
        nom::multi::many0(preceded(ws(tag("|")), ws(parse_simple_pattern))).parse(input)?;

    // If there are no additional patterns, this isn't really an or-pattern
    if patterns.is_empty() {
        Ok((input, first))
    } else {
        let mut all_patterns = vec![first];
        all_patterns.extend(patterns);

        Ok((input, AstNode::OrPattern(all_patterns)))
    }
}

/// Parse a simple pattern (without | operator) for use in or-patterns
fn parse_simple_pattern(input: &str) -> IResult<&str, AstNode> {
    alt((
        tag("_").map(|_| AstNode::Ignore),
        parse_tuple_pattern,
        parse_bind_pattern,
        parse_struct_pattern,
        parse_range_pattern,
        parse_lit,
        parse_char_lit,
        parse_string_lit,
        parse_ident.map(AstNode::Var),
    ))
    .parse(input)
}
