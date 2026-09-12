// src/frontend/parser/top_level.rs
//! Top-level parser for Zeta language constructs (functions, concepts, impls, etc.).
//! Updated to correctly handle explicit `return` statements in function bodies
//! without accidentally discarding them during implicit-return extraction.
#![allow(unused_variables)]
use super::expr::parse_full_expr;
use super::parser::{
    cfg_should_skip, parse_attributes, parse_generic_params_as_enum, parse_ident, parse_path,
    parse_trait_bounds, parse_type, parse_where_clause, skip_ws_and_comments, ws,
};
use super::stmt::parse_block_body;
use crate::frontend::ast::AstNode;
use nom::IResult;
use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::character::complete::none_of;
use nom::combinator::{map, not, opt, peek, value};

use nom::multi::{many0, separated_list0};
use nom::sequence::{delimited, preceded, terminated};

/// PY-A: default value in `def f(x=None, y=3)` — parsed and discarded (V1:
/// params are typed i64 regardless; the default expr is not evaluated at def
/// time). Bounded by ',' ')' and newline to avoid swallowing following params.
fn parse_default_value(input: &str) -> IResult<&str, AstNode> {
    parse_full_expr(input)
}

fn parse_param(input: &str) -> IResult<&str, (String, String)> {
    // Parse a function parameter.
    // Two forms are supported:
    // 1. Self parameters: `self`, `&self`, `&mut self` (without explicit type)
    //    These must NOT be followed by `:` (checked via peek).
    //    Returns ("self"|"&self"|"&mut self", "Self").
    // 2. Regular parameters: `ident: type`
    //    Returns (ident, type_string).
    //
    // Note: Patterns (e.g., `(x, y): (i64, i64)`) are not supported in function
    // parameters by this parser, matching the AST representation.
    let parse_self = alt((
        // &mut self (must not be followed by :)
        map(
            (ws(tag("&mut")), ws(tag("self")), peek(not(ws(tag(":"))))),
            |_| ("&mut self".to_string(), "Self".to_string()),
        ),
        // &self (must not be followed by :)
        map(
            (ws(tag("&")), ws(tag("self")), peek(not(ws(tag(":"))))),
            |_| ("&self".to_string(), "Self".to_string()),
        ),
        // mut self (owned, mutable — must not be followed by :)
        map(
            (ws(tag("mut")), ws(tag("self")), peek(not(ws(tag(":"))))),
            |_| ("mut self".to_string(), "Self".to_string()),
        ),
        // self (without &, must not be followed by :)
        map((ws(tag("self")), peek(not(ws(tag(":"))))), |_| {
            ("self".to_string(), "Self".to_string())
        }),
    ));

    // PY-A: `*args` / `**kwargs` star-params — consumed as a single opaque
    // i64 param (V1: call sites with extra args coerce; real variadics need
    // arg-tuple support). The FIRST star is mandatory so this branch can
    // never shadow regular params.
    let parse_star = map(
        (
            ws(tag("*")),
            opt(ws(tag("*"))),
            ws(parse_ident),
        ),
        |(_, _, name)| (name, "i64".to_string()),
    );

    // Try regular parameter: `name: type` — PY-A: the type annotation is
    // optional (Python style `def f(x):`), defaulting to i64. Call-site
    // coercion (coerce_call_args) adapts f64 args at monomorphic call sites.
    let parse_regular = map(
        (
            ws(parse_ident),
            opt(preceded(ws(tag(":")), ws(parse_type))),
            opt(ws(preceded(tag("="), ws(parse_default_value)))),
        ),
        |(name, ty, _default)| (name, ty.unwrap_or_else(|| "i64".to_string())),
    );

    // PY-A: allow Python-common names that collide with Zeta keywords in
    // PARAMETER POSITION only (e.g. JoinQuant strategies use `fn` as a param
    // name: `def run_daily(fn, time)`). A dedicated relaxed-ident parser —
    // general statement positions keep the keyword rules.
    let parse_kw_param = map(
        (
            ws(alt((
                tag("fn"),
                tag("match"),
                tag("type"),
                tag("impl"),
                tag("open"),
                tag("high"),
                tag("low"),
                tag("set"),
            ))),
            opt(preceded(ws(tag(":")), ws(parse_type))),
            opt(ws(preceded(tag("="), ws(parse_default_value)))),
        ),
        |(name, ty, _default)| (name.to_string(), ty.unwrap_or_else(|| "i64".to_string())),
    );

    alt((parse_self, parse_star, parse_kw_param, parse_regular)).parse(input)
}

fn parse_use_statement(input: &str) -> IResult<&str, Vec<AstNode>> {
    let (input, _) = ws(tag("use")).parse(input)?;
    let (input, path) = ws(parse_path).parse(input)?;
    let (input, group_opt) = opt(preceded(
        ws(tag("::")),
        delimited(
            ws(tag("{")),
            terminated(
                separated_list0(
                    ws(tag(",")),
                    ws(alt((value("self".to_string(), tag("self")), parse_ident))),
                ),
                opt(ws(tag(","))),
            ),
            ws(tag("}")),
        ),
    ))
    .parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    let nodes = if let Some(items) = group_opt {
        let mut ns = vec![];
        for item in items {
            if item == "self" {
                ns.push(AstNode::Use { path: path.clone() });
            } else {
                let mut p = path.clone();
                p.push(item);
                ns.push(AstNode::Use { path: p });
            }
        }
        ns
    } else {
        vec![AstNode::Use { path }]
    };
    Ok((input, nodes))
}

/// Parse visibility modifier (pub keyword)
fn parse_visibility(input: &str) -> IResult<&str, bool> {
    let (input, pub_opt) = opt(ws(tag("pub"))).parse(input)?;
    Ok((input, pub_opt.is_some()))
}

pub(crate) fn parse_func(input: &str) -> IResult<&str, AstNode> {
    let input = skip_decorator_lines(input);
    let (input, attrs) = match parse_attributes(input) {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };

    let (input, pub_) = match parse_visibility(input) {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };

    let (input, comptime_opt) = opt(ws(tag("comptime"))).parse(input)?;
    let (input, const_opt) = opt(ws(tag("const"))).parse(input)?;
    let (input, async_opt) = opt(ws(tag("async"))).parse(input)?;
    let (input, extern_opt) = opt(ws(tag("extern"))).parse(input)?;
    let (input, _) = match ws(alt((
        tag("fn"),
        // PY-2: `def` alias for `fn`, word-boundary guarded (`default` stays an ident)
        terminated(
            tag("def"),
            peek(none_of("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_")),
        ),
    )))
    .parse(input)
    {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };
    let (input, path) = match ws(parse_path).parse(input) {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };
    let name = path.join("::");

    let (input, generics_opt) = opt(ws(parse_generic_params_as_enum)).parse(input)?;
    let mut lifetimes = Vec::new();
    let mut generics = Vec::new();

    if let Some(params) = generics_opt {
        for param in params {
            match param {
                crate::frontend::ast::GenericParam::Lifetime { name } => {
                    lifetimes.push(name);
                }
                _ => {
                    generics.push(param);
                }
            }
        }
    }

    let (input, params) = match delimited(
        ws(tag("(")),
        terminated(
            separated_list0(ws(tag(",")), ws(parse_param)),
            opt(ws(tag(","))),
        ),
        ws(tag(")")),
    )
    .parse(input)
    {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };

    let (input, ret_opt) = match opt(preceded(ws(tag("->")), ws(parse_type))).parse(input) {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };

    // Parse where clause if present
    let (input, where_clauses_opt) = opt(ws(parse_where_clause)).parse(input)?;
    let where_clauses = where_clauses_opt.unwrap_or_default();
    let (input, (body, ret_expr, single_line)) = if extern_opt.is_some() {
        let (input, _) = match ws(tag(";")).parse(input) {
            Ok(r) => r,
            Err(e) => {
                return Err(e);
            }
        };
        (input, (vec![], None, false))
    } else {
        let body_result = alt((
            map(
                delimited(ws(tag("{")), parse_block_body, ws(tag("}"))),
                |mut b| {
                    let re = if let Some(AstNode::ExprStmt { .. }) = b.last() {
                        if let Some(AstNode::ExprStmt { expr }) = b.pop() {
                            Some(expr)
                        } else {
                            None
                        }
                    } else if let Some(last) = b.last() {
                        // Bare expressions (If, Call, Match, Block) parsed via parse_stmt
                        // can end up directly in body without ExprStmt wrapping.
                        // Promote them to ret_expr for proper return value handling.
                        match last {
                            AstNode::If { .. }
                            | AstNode::Call { .. }
                            | AstNode::PathCall { .. }
                            | AstNode::Match { .. }
                            | AstNode::Block { .. }
                            | AstNode::Loop { .. } => b.pop().map(Box::new),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    (b, re, false)
                },
            ),
            map(preceded(ws(tag("=")), ws(parse_full_expr)), |e| {
                (vec![], Some(Box::new(e)), true)
            }),
        ))
        .parse(input);
        match body_result {
            Ok(r) => r,
            Err(e) => {
                return Err(e);
            }
        }
    };
    let input = if single_line {
        let (i, _) = ws(tag(";")).parse(input)?;
        i
    } else {
        input
    };
    let ret = ret_opt.unwrap_or_else(|| "()".to_string());
    let ast = if extern_opt.is_some() {
        AstNode::ExternFunc {
            name,
            generics,
            lifetimes,
            params,
            ret,
            where_clauses,
        }
    } else {
        AstNode::FuncDef {
            name,
            generics,
            lifetimes,
            params,
            ret,
            body,
            attrs,
            ret_expr,
            single_line,
            doc: "".to_string(),
            pub_,
            async_: async_opt.is_some(),
            const_: const_opt.is_some(),
            comptime_: comptime_opt.is_some(),
            where_clauses,
        }
    };
    Ok((input, ast))
}

pub fn parse_type_alias(input: &str) -> IResult<&str, AstNode> {
    // Parse attributes
    let (input, _attrs) = parse_attributes(input)?;

    // Parse visibility
    let (input, pub_) = parse_visibility(input)?;

    let (input, _) = ws(tag("type")).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;
    let (input, _) = ws(tag("=")).parse(input)?;
    let (input, ty) = ws(parse_type).parse(input)?;
    // PY-A: trailing semicolon optional (Python style has none)
    let (input, _) = opt(ws(tag(";"))).parse(input)?;
    Ok((input, AstNode::TypeAlias { name, ty, pub_ }))
}

fn parse_concept(input: &str) -> IResult<&str, AstNode> {
    // Parse attributes
    let (input, attrs) = parse_attributes(input)?;

    // Parse visibility
    let (input, pub_) = parse_visibility(input)?;

    let (input, _) = ws(tag("concept")).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;
    let (input, generics_opt) = opt(ws(parse_generic_params_as_enum)).parse(input)?;
    let mut lifetimes = Vec::new();
    let mut generics = Vec::new();

    if let Some(params) = generics_opt {
        for param in params {
            match param {
                crate::frontend::ast::GenericParam::Lifetime { name } => {
                    lifetimes.push(name);
                }
                _ => {
                    generics.push(param);
                }
            }
        }
    }

    // Parse supertraits (concept inheritance) if present
    let (input, supertraits) = if let Ok((input, _)) = ws(tag(":")).parse(input) {
        // Parse trait bounds without the leading ':'
        // Clone the parse_trait_bounds logic but skip the initial ':'
        let (input, first_bound_path) = ws(parse_path).parse(input)?;
        let first_bound = first_bound_path.join("::");
        let mut bounds = vec![first_bound];

        // Parse additional bounds with +
        let mut input = input;
        while let Ok((new_input, _)) = ws(tag("+")).parse(input) {
            let (new_input, bound_path) = ws(parse_path).parse(new_input)?;
            let bound = bound_path.join("::");
            bounds.push(bound);
            input = new_input;
        }

        (input, bounds)
    } else {
        (input, Vec::new())
    };

    // Parse where clause if present
    let (input, where_clauses_opt) = opt(ws(parse_where_clause)).parse(input)?;
    let where_clauses = where_clauses_opt.unwrap_or_default();
    // Parse concept body - can contain methods
    let (input, methods) =
        delimited(ws(tag("{")), many0(ws(parse_method_sig)), ws(tag("}"))).parse(input)?;

    // For now, we don't parse associated types
    let associated_types = Vec::new();
    Ok((
        input,
        AstNode::ConceptDef {
            name,
            generics,
            lifetimes,
            methods,
            associated_types,
            attrs,
            doc: "".to_string(),
            pub_,
            where_clauses,
            supertraits,
        },
    ))
}

fn parse_associated_type(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("type")).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;

    // Parse optional trait bounds: type Name: Bound1 + Bound2
    let (input, bounds) = if let Ok((input, _)) = ws(tag(":")).parse(input) {
        parse_trait_bounds(input)?
    } else {
        (input, Vec::new())
    };

    // Parse optional default type: type Name = DefaultType
    let (input, default) = if let Ok((input, _)) = ws(tag("=")).parse(input) {
        let (input, default_ty) = ws(parse_type).parse(input)?;
        (input, Some(default_ty))
    } else {
        (input, None)
    };

    let (input, _) = ws(tag(";")).parse(input)?;

    Ok((
        input,
        AstNode::AssociatedType {
            name,
            default,
            bounds,
        },
    ))
}

fn parse_method_sig(input: &str) -> IResult<&str, AstNode> {
    // Parse attributes
    let (input, attrs) = parse_attributes(input)?;

    let (input, _) = ws(tag("fn")).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;
    let (input, generics_opt) = opt(ws(parse_generic_params_as_enum)).parse(input)?;
    let mut lifetimes = Vec::new();
    let mut generics = Vec::new();

    if let Some(params) = generics_opt {
        for param in params {
            match param {
                crate::frontend::ast::GenericParam::Lifetime { name } => {
                    lifetimes.push(name);
                }
                _ => {
                    generics.push(param);
                }
            }
        }
    }
    let (input, params) = delimited(
        ws(tag("(")),
        terminated(
            separated_list0(ws(tag(",")), ws(parse_param)),
            opt(ws(tag(","))),
        ),
        ws(tag(")")),
    )
    .parse(input)?;
    let (input, ret_opt) = opt(preceded(ws(tag("->")), ws(parse_type))).parse(input)?;
    // Parse where clause if present
    let (input, where_clauses_opt) = opt(ws(parse_where_clause)).parse(input)?;
    let where_clauses = where_clauses_opt.unwrap_or_default();

    // Check if there's a body (default implementation) or just a signature
    let (input, body) = if let Ok((input, body)) = parse_block_body(input) {
        (input, Some(body))
    } else {
        // Just a signature, ends with ;
        let (input, _) = ws(tag(";")).parse(input)?;
        (input, None)
    };

    let ret = ret_opt.unwrap_or_else(|| "()".to_string());
    Ok((
        input,
        AstNode::Method {
            name,
            params,
            ret,
            generics,
            lifetimes,
            attrs,
            doc: "".to_string(),
            where_clauses,
            body,
        },
    ))
}

fn parse_impl(input: &str) -> IResult<&str, AstNode> {
    // Parse attributes
    let (input, attrs) = parse_attributes(input)?;

    let (input, _) = ws(tag("impl")).parse(input)?;
    let (input, generics_opt) = opt(ws(parse_generic_params_as_enum)).parse(input)?;
    let mut lifetimes = Vec::new();
    let mut generics = Vec::new();

    if let Some(params) = generics_opt {
        for param in params {
            match param {
                crate::frontend::ast::GenericParam::Lifetime { name } => {
                    lifetimes.push(name);
                }
                _ => {
                    generics.push(param);
                }
            }
        }
    }

    // Try to parse as inherent impl: impl<Generics> Type { ... }
    let parse_result = alt((
        // Trait impl: impl<Generics> Concept for Type
        map(
            (ws(parse_ident), ws(tag("for")), ws(parse_type)),
            |(concept, _, ty)| (concept, ty),
        ),
        // Inherent impl: impl<Generics> Type
        // FIXED: Use parse_type instead of parse_ident to handle complex types like Option<i64>
        map(ws(parse_type), |ty| ("".to_string(), ty)),
    ))
    .parse(input);

    let (input, (concept, ty)) = match parse_result {
        Ok((input, (concept, ty))) => (input, (concept, ty)),
        Err(e) => {
            return Err(e);
        }
    };
    // Parse where clause if present
    let (input, where_clauses_opt) = opt(ws(parse_where_clause)).parse(input)?;
    let where_clauses = where_clauses_opt.unwrap_or_default();
    let (input, body) =
        delimited(ws(tag("{")), many0(ws(parse_func)), ws(tag("}"))).parse(input)?;
    Ok((
        input,
        AstNode::ImplBlock {
            concept,
            generics,
            lifetimes,
            ty,
            body,
            attrs,
            doc: "".to_string(),
            where_clauses,
        },
    ))
}

fn parse_variant(input: &str) -> IResult<&str, (String, Vec<String>)> {
    let (input, name) = ws(parse_ident).parse(input)?;
    let (input, params_opt) = opt(alt((
        delimited(
            ws(tag("(")),
            terminated(
                separated_list0(ws(tag(",")), ws(parse_type)),
                opt(ws(tag(","))),
            ),
            ws(tag(")")),
        ),
        delimited(
            ws(tag("{")),
            terminated(
                separated_list0(ws(tag(",")), parse_struct_field),
                opt(ws(tag(","))),
            ),
            ws(tag("}")),
        )
        .map(|fields: Vec<(String, String)>| fields.into_iter().map(|(_, ty)| ty).collect()),
    )))
    .parse(input)?;
    let params = params_opt.unwrap_or_default();
    Ok((input, (name, params)))
}

fn parse_enum(input: &str) -> IResult<&str, AstNode> {
    // Parse attributes
    let (input, attrs) = parse_attributes(input)?;

    // Parse visibility
    let (input, pub_) = parse_visibility(input)?;

    let (input, _) = ws(tag("enum")).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;
    // Parse generic parameters if present (e.g., <T> or <T, E>)
    let (input, generics_opt) = opt(ws(parse_generic_params_as_enum)).parse(input)?;
    let mut lifetimes = Vec::new();
    let mut generics = Vec::new();

    if let Some(params) = generics_opt {
        for param in params {
            match param {
                crate::frontend::ast::GenericParam::Lifetime { name } => {
                    lifetimes.push(name);
                }
                _ => {
                    generics.push(param);
                }
            }
        }
    }
    // Parse where clause if present
    let (input, where_clauses_opt) = opt(ws(parse_where_clause)).parse(input)?;
    let where_clauses = where_clauses_opt.unwrap_or_default();
    // PY-2: variant separator (`,`/`;`) is optional — newline-separated
    // variants work, which is what indented enum bodies normalize to.
    let (input, variants) = delimited(
        ws(tag("{")),
        terminated(
            many0(preceded(
                opt(alt((ws(tag(",")), ws(tag(";"))))),
                ws(parse_variant),
            )),
            opt(alt((ws(tag(",")), ws(tag(";"))))),
        ),
        ws(tag("}")),
    )
    .parse(input)?;
    Ok((
        input,
        AstNode::EnumDef {
            name,
            generics,
            lifetimes,
            variants,
            attrs,
            doc: "".to_string(),
            pub_,
            where_clauses,
        },
    ))
}

fn parse_struct_field(input: &str) -> IResult<&str, (String, String)> {
    // Parse optional visibility modifier (pub) but don't store it
    let (input, _) = opt(ws(tag("pub"))).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;
    let (input, _) = ws(tag(":")).parse(input)?;
    let (input, ty) = ws(parse_type).parse(input)?;
    Ok((input, (name, ty)))
}

/// PY-A: does this method body return a string value? (f-string or string
/// literal as the tail expression or inside a return statement)
fn body_is_string_return(body: &[AstNode]) -> bool {
    fn is_str_expr(e: &AstNode) -> bool {
        matches!(e, AstNode::FString(_) | AstNode::StringLit(_))
    }
    if let Some(last) = body.last() {
        if let AstNode::ExprStmt { expr } = last {
            if is_str_expr(expr) {
                return true;
            }
        }
    }
    body.iter().any(|st| {
        matches!(st, AstNode::Return(inner) if is_str_expr(inner.as_ref()))
    })
}

/// PY-A: Python `class` → `struct` + `impl` + constructor desugar.
///
/// ```python
/// class Counter:
///     def __init__(self):
///         self.count = 0
///     def inc(self):
///         self.count = self.count + 1
///         return self.count
/// ```
/// becomes
/// ```zeta
/// struct Counter { count: i64 }
/// impl Counter { fn inc(&mut self) -> i64 { ... } }
/// fn Counter() -> Counter { return Counter { count: 0 } }
/// ```
/// Field types are inferred from `__init__` literal defaults (i64/f64/str/
/// bool/lists); fields assigned an `__init__` parameter default to i64.
/// Inheritance (`class A(B):`) is not supported in V1 and errors.
/// PY-A: decorator lines `@name` / `@name(args)` — consumed and ignored in
/// V1 (decorator semantics need call-rewriting; parse-ignore keeps real
/// Python files parseable). Only valid at item start.
fn skip_decorator_lines(input: &str) -> &str {
    let mut cur = input;
    loop {
        let t = cur.trim_start();
        if t.starts_with('@') && !t.starts_with("#[") {
            match t.find('\n') {
                Some(pos) => cur = &t[pos..],
                None => return "",
            }
        } else {
            return cur;
        }
    }
}

fn parse_class(input: &str) -> IResult<&str, AstNode> {
    let input = skip_decorator_lines(input);
    let (input, _) = ws(terminated(tag("class"), peek(none_of("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_")))).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;
    // Inheritance bases are not supported — reject explicitly (never fail-open)
    if let Ok((after, _)) = ws(tag("(")).parse(input) {
        let _ = after;
        return Err(nom::Err::Failure(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    let (input, _) = ws(tag("{")).parse(input)?;

    // Collect methods and __init__
    let mut methods: Vec<AstNode> = Vec::new();
    let mut init_params: Vec<(String, String)> = Vec::new();
    let mut init_stmts: Vec<AstNode> = Vec::new();
    let mut has_init = false;
    let mut cur = input;
    loop {
        let (next, _) = skip_ws_and_comments(cur)?;
        if next.starts_with('}') {
            cur = next;
            break;
        }
        // def method(...) { ... } — reuse parse_func (def alias supported)
        match parse_func(next) {
            Ok((rest, AstNode::FuncDef { name: mname, params, body, .. })) => {
                if mname == "__init__" {
                    has_init = true;
                    init_params = params
                        .iter()
                        .filter(|(n, _)| n != "self" && n != "&self" && n != "&mut self")
                        .cloned()
                        .collect();
                    init_stmts = body;
                } else {
                    // Python `def m(self, a, b)` → `fn m(&mut self, a, b)`
                    let mut new_params: Vec<(String, String)> =
                        vec![("&mut self".to_string(), "Self".to_string())];
                    for (pn, pt) in &params {
                        if pn != "self" && pn != "&self" && pn != "&mut self" {
                            new_params.push((pn.clone(), pt.clone()));
                        }
                    }
                    // Return type inference: string-returning bodies (f-string
                    // or string literal results) get "str", else i64.
                    let ret = if body_is_string_return(&body) {
                        "str".to_string()
                    } else {
                        "i64".to_string()
                    };
                    methods.push(AstNode::FuncDef {
                        name: mname,
                        generics: Vec::new(),
                        lifetimes: Vec::new(),
                        params: new_params,
                        ret,
                        body,
                        attrs: Vec::new(),
                        ret_expr: None,
                        single_line: false,
                        doc: String::new(),
                        pub_: false,
                        async_: false,
                        const_: false,
                        comptime_: false,
                        where_clauses: Vec::new(),
                    });
                }
                cur = rest;
            }
            Ok((rest, _other)) => {
                cur = rest;
            }
            Err(e) => return Err(e),
        }
    }
    let (input, _) = ws(tag("}")).parse(cur)?;

    // Field extraction from `__init__` `self.<field> = <rhs>`
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut field_inits: Vec<(String, AstNode)> = Vec::new();
    let param_names: Vec<&str> = init_params.iter().map(|(n, _)| n.as_str()).collect();
    for st in &init_stmts {
        if let AstNode::Assign(lhs, rhs) = st {
            if let AstNode::FieldAccess { base, field } = &**lhs {
                if let AstNode::Var(v) = &**base {
                    if v == "self" {
                        let ty = match &**rhs {
                            AstNode::Lit(_) => "i64".to_string(),
                            AstNode::Bool(_) => "bool".to_string(),
                            AstNode::FloatLit(_) => "f64".to_string(),
                            AstNode::StringLit(_) => "str".to_string(),
                            AstNode::ArrayLit(_) | AstNode::DynamicArrayLit { .. } => {
                                "DynamicArray".to_string()
                            }
                            AstNode::Var(name) if param_names.contains(&name.as_str()) => {
                                // `self.x = x` — type unknown, call-site coercion adapts
                                "i64".to_string()
                            }
                            _ => "i64".to_string(),
                        };
                        if !fields.iter().any(|(f, _)| f == field) {
                            fields.push((field.clone(), ty));
                            field_inits.push((field.clone(), (**rhs).clone()));
                        }
                    }
                }
            }
        }
    }

    // Constructor fn `Name(params) -> Name { return Name { field: init, ... } }`
    let ctor_body: Vec<AstNode> = vec![AstNode::Return(Box::new(AstNode::StructLit {
        variant: name.clone(),
        fields: field_inits
            .iter()
            .map(|(f, expr)| (f.clone(), expr.clone()))
            .collect(),
    }))];
    let ctor = AstNode::FuncDef {
        name: name.clone(),
        generics: Vec::new(),
        lifetimes: Vec::new(),
        params: init_params.clone(),
        ret: name.clone(),
        body: ctor_body,
        attrs: Vec::new(),
        ret_expr: None,
        single_line: false,
        doc: String::new(),
        pub_: false,
        async_: false,
        const_: false,
        comptime_: false,
        where_clauses: Vec::new(),
    };

    // Wrap struct + impl + ctor into a Block; parse_zeta flattens
    // top-level Blocks into separate items.
    let struct_node = AstNode::StructDef {
        name: name.clone(),
        generics: Vec::new(),
        lifetimes: Vec::new(),
        fields,
        attrs: Vec::new(),
        doc: String::new(),
        pub_: false,
        where_clauses: Vec::new(),
    };
    let impl_node = AstNode::ImplBlock {
        concept: String::new(),
        generics: Vec::new(),
        lifetimes: Vec::new(),
        ty: name.clone(),
        body: methods,
        attrs: Vec::new(),
        doc: String::new(),
        where_clauses: Vec::new(),
    };
    let _ = has_init;
    Ok((
        input,
        AstNode::Block {
            body: vec![struct_node, impl_node, ctor],
        },
    ))
}

fn parse_struct(input: &str) -> IResult<&str, AstNode> {
    // Parse attributes
    let (input, attrs) = parse_attributes(input)?;

    // Parse visibility
    let (input, pub_) = parse_visibility(input)?;

    let (input, _) = ws(tag("struct")).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;
    // Parse generic parameters if present
    let (input, generics_opt) = opt(ws(parse_generic_params_as_enum)).parse(input)?;
    let mut lifetimes = Vec::new();
    let mut generics = Vec::new();

    if let Some(params) = generics_opt {
        for param in params {
            match param {
                crate::frontend::ast::GenericParam::Lifetime { name } => {
                    lifetimes.push(name);
                }
                _ => {
                    generics.push(param);
                }
            }
        }
    }

    // Parse where clause if present
    let (input, where_clauses_opt) = opt(ws(parse_where_clause)).parse(input)?;
    let where_clauses = where_clauses_opt.unwrap_or_default();

    // PY-2: field separator (`,`/`;`) is optional — newline-separated fields
    // work, which is what indented struct bodies normalize to.
    let (input, fields) = delimited(
        ws(tag("{")),
        terminated(
            many0(preceded(
                opt(alt((ws(tag(",")), ws(tag(";"))))),
                ws(parse_struct_field),
            )),
            opt(alt((ws(tag(",")), ws(tag(";"))))),
        ),
        ws(tag("}")),
    )
    .parse(input)?;

    Ok((
        input,
        AstNode::StructDef {
            name,
            generics,
            lifetimes,
            fields,
            attrs,
            doc: "".to_string(),
            pub_,
            where_clauses,
        },
    ))
}

pub(crate) fn parse_const(input: &str) -> IResult<&str, AstNode> {
    // Parse attributes
    let (input, attrs) = parse_attributes(input)?;

    // Parse visibility
    let (input, pub_) = parse_visibility(input)?;

    // Parse const or comptime keyword
    let (input, comptime_) = alt((
        ws(tag("comptime")).map(|_| true),
        ws(tag("const")).map(|_| false),
    ))
    .parse(input)?;

    let (input, name) = ws(parse_ident).parse(input)?;
    // Type annotation is optional: `const F = 55;` is valid
    let (input, ty) = opt(preceded(ws(tag(":")), ws(parse_type)))
        .parse(input)
        .map(|(i, t)| (i, t.unwrap_or_else(|| "i64".to_string())))?;
    let (input, _) = ws(tag("=")).parse(input)?;
    let (input, value) = ws(parse_full_expr).parse(input)?;
    let (input, _) = opt(ws(tag(";"))).parse(input)?;

    Ok((
        input,
        AstNode::ConstDef {
            name,
            ty,
            value: Box::new(value),
            attrs,
            pub_,
            comptime_,
        },
    ))
}

fn parse_mod(input: &str) -> IResult<&str, AstNode> {
    // Parse attributes first
    let (input, attrs) = parse_attributes(input)?;

    // Parse visibility before checking cfg — must consume tokens in order
    let (input, pub_) = parse_visibility(input)?;

    let (input, _) = ws(tag("mod")).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;

    // Check #[cfg(feature = "...")] — if false, skip this module but still consume body
    if cfg_should_skip(&attrs) {
        // Consume body without building AST
        let (input, _) = alt((
            delimited(
                ws(tag("{")),
                many0(ws(alt((
                    parse_use_statement,
                    map(parse_top_level_item, |node| vec![node]),
                )))),
                ws(tag("}")),
            ),
            map(ws(tag(";")), |_| vec![]),
        ))
        .parse(input)?;
        return Ok((input, AstNode::Skip));
    }

    // Parse module body — either { ... } or ; (forward declaration)
    let (input, items) = alt((
        // Inline module: mod Name { ... }
        delimited(
            ws(tag("{")),
            many0(ws(alt((
                parse_use_statement,
                map(parse_top_level_item, |node| vec![node]),
            )))),
            ws(tag("}")),
        ),
        // Forward declaration: mod Name;
        map(ws(tag(";")), |_| vec![]),
    ))
    .parse(input)?;

    // Flatten the items and filter out skipped cfg-gated items
    let items: Vec<AstNode> = items
        .into_iter()
        .flatten()
        .filter(|n| !matches!(n, AstNode::Skip))
        .collect();

    Ok((
        input,
        AstNode::ModDef {
            name,
            items,
            pub_,
            attrs,
        },
    ))
}

fn parse_macro_def(input: &str) -> IResult<&str, AstNode> {
    let (input, _) = ws(tag("macro_rules!")).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;

    // Parse the macro body (simplified - just capture everything between braces)
    let (input, _) = ws(tag("{")).parse(input)?;

    // Find matching closing brace
    let mut depth = 1;
    let mut pos = 0;
    let chars: Vec<char> = input.chars().collect();

    while depth > 0 && pos < chars.len() {
        match chars[pos] {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        pos += 1;
    }

    if depth > 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Eof,
        )));
    }

    let body: String = chars[..pos - 1].iter().collect(); // pos-1 to exclude the closing '}'
    let remaining = &input[pos..];

    Ok((
        remaining,
        AstNode::MacroDef {
            name,
            patterns: body,
        },
    ))
}

fn parse_top_level_item(input: &str) -> IResult<&str, AstNode> {
    // Order matters: type/struct/enum/concept checked before func because
    // they share attribute syntax (#[derive], etc.) and alt cannot backtrack
    // once an alternative consumes input on failure.
    alt((
        parse_type_alias,
        parse_concept,
        parse_impl,
        parse_enum,
        parse_class,
        parse_struct,
        parse_const,
        parse_macro_def,
        parse_mod,
        parse_func,
        // Also allow statements at top level
        crate::frontend::parser::stmt::parse_stmt,
    ))
    .parse(input)
}

pub fn parse_zeta(input: &str) -> IResult<&str, Vec<AstNode>> {
    // PY-1: normalize indentation blocks to braces before parsing (design of
    // record: docs/python-syntax.md R1). Brace-style sources pass through
    // unchanged (Ok(None) keeps the original &str so `remaining` slices stay
    // valid).
    match crate::frontend::indent::indent_preprocess(input) {
        Ok(Some(processed)) => {
            let processed: &'static str =
                Box::leak(processed.into_boxed_str());
            // Parse the preprocessed text; `remaining` refers to the leaked
            // string. Callers only check remaining.is_empty(); a leaked
            // process-lifetime copy is fine for a single compile (matching the
            // existing leak-heavy interpreter design).
            parse_zeta_impl(processed)
                .map(|(rem, asts)| (rem, synthesize_implicit_main(asts)))
        }
        Ok(None) => parse_zeta_impl(input)
            .map(|(rem, asts)| (rem, synthesize_implicit_main(asts))),
        Err(tab_err) => {
            // Hard indentation error (tab indent). nom errors carry no
            // message payload, so callers see a parse failure at the file
            // start; the detailed reason is available via the error's Display.
            let _ = &tab_err;
            Err(nom::Err::Failure(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            )))
        }
    }
}

/// PY-A: Python modules run their top-level statements. Statement-level items
/// (an `if __name__ == "__main__":` guard body unwraps to a Block, bare calls,
/// import no-ops) are collected into a synthesized `fn main` when the module
/// has none — otherwise the compiled binary has no entry point.
fn synthesize_implicit_main(asts: Vec<AstNode>) -> Vec<AstNode> {
    let has_main = asts
        .iter()
        .any(|a| matches!(a, AstNode::FuncDef { name, .. } if name == "main"));
    if has_main {
        return asts;
    }
    let mut out = Vec::with_capacity(asts.len() + 1);
    let mut main_body: Vec<AstNode> = Vec::new();
    // Definition allowlist: these stay at top level; EVERYTHING else is a
    // module-level statement and becomes the implicit main's body (Python
    // runs top-level statements at import — if/while/for/calls/assignments).
    fn is_definition(a: &AstNode) -> bool {
        matches!(
            a,
            AstNode::FuncDef { .. }
                | AstNode::StructDef { .. }
                | AstNode::EnumDef { .. }
                | AstNode::ImplBlock { .. }
                | AstNode::TypeAlias { .. }
                | AstNode::ConstDef { .. }
                | AstNode::Use { .. }
                | AstNode::ConceptDef { .. }
                | AstNode::ExternFunc { .. }
                | AstNode::ModDef { .. }
        )
    }
    for a in asts {
        match a {
            // PY-A: a class desugars to [struct, impl, ctor] wrapped in a
            // Block — lift definitions back out to top level.
            AstNode::Block { body } => {
                for node in body {
                    if is_definition(&node) {
                        out.push(node);
                    } else {
                        main_body.push(node);
                    }
                }
            }
            other if is_definition(&other) => out.push(other),
            stmt => main_body.push(stmt),
        }
    }
    // PY-A: a module whose only content is definitions (a "library" module
    // run as a script) still needs an entry point to link as an executable —
    // emit an empty main when nothing else exists.
    out.push(AstNode::FuncDef {
        name: "main".to_string(),
        generics: Vec::new(),
        lifetimes: Vec::new(),
        params: Vec::new(),
        ret: "i64".to_string(),
        body: main_body,
        attrs: Vec::new(),
        ret_expr: None,
        single_line: false,
        doc: String::new(),
        pub_: false,
        async_: false,
        const_: false,
        comptime_: false,
        where_clauses: Vec::new(),
    });
    out
}

fn parse_zeta_impl(input: &str) -> IResult<&str, Vec<AstNode>> {
    let (input, _) = skip_ws_and_comments(input)?;

    let parse_result = many0(ws(alt((
        parse_use_statement,
        map(parse_top_level_item, |node| vec![node]),
    ))))
    .parse(input);

    let (input, vec_vec) = match parse_result {
        Ok((i, v)) => (i, v),
        Err(e) => {
            return Err(e);
        }
    };

    let asts: Vec<AstNode> = vec_vec
        .into_iter()
        .flatten()
        .filter(|n| !matches!(n, AstNode::Skip))
        .collect::<Vec<AstNode>>();

    Ok((input, asts))
}
