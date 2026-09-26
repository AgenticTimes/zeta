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
    let (rest, (n, t, _default)) = parse_param_full(input)?;
    Ok((rest, (n, t)))
}

/// PY-A: `parse_param` plus the optional Python default value. Kept as a
/// separate function so the Zeta-style call sites (impl methods, extern decls)
/// keep their two-tuple signature.
fn parse_param_full(input: &str) -> IResult<&str, (String, String, Option<AstNode>)> {
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
            |_| ("&mut self".to_string(), "Self".to_string(), None),
        ),
        // &self (must not be followed by :)
        map(
            (ws(tag("&")), ws(tag("self")), peek(not(ws(tag(":"))))),
            |_| ("&self".to_string(), "Self".to_string(), None),
        ),
        // mut self (owned, mutable — must not be followed by :)
        map(
            (ws(tag("mut")), ws(tag("self")), peek(not(ws(tag(":"))))),
            |_| ("mut self".to_string(), "Self".to_string(), None),
        ),
        // self (without &, must not be followed by :)
        map((ws(tag("self")), peek(not(ws(tag(":"))))), |_| {
            ("self".to_string(), "Self".to_string(), None)
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
        |(_, _, name)| (name, "i64".to_string(), None),
    );

    // Try regular parameter: `name: type` — PY-A / B3: type annotation optional
    // (Python style `def f(x):`); default is `dyn` (Type::PyDynamic), ABI still i64.
    let parse_regular = map(
        (
            ws(parse_ident),
            opt(preceded(ws(tag(":")), ws(parse_type))),
            opt(ws(preceded(tag("="), ws(parse_default_value)))),
        ),
        |(name, ty, default)| (name, ty.unwrap_or_else(|| "dyn".to_string()), default),
    );

    // PY-A: allow Python-common names that collide with Zeta keywords in
    // PARAMETER POSITION only (e.g. JoinQuant strategies use `fn` as a param
    // name: `def run_daily(fn, time)`). A dedicated relaxed-ident parser —
    // general statement positions keep the keyword rules.
    //
    // The keywords need WORD BOUNDARIES: `tag("type")` also matched the `type`
    // inside a normal parameter name, so `def f(types)` parsed as a param named
    // `type` followed by leftover `s` — the parameter list (and with it the
    // whole definition) failed. `type_s` / `type_name` / `typeOf` had the same
    // fate (`jq_shim.get_all_securities(types: …)`).
    let parse_kw_param = map(
        (
            parse_kw_param_name,
            opt(preceded(ws(tag(":")), ws(parse_type))),
            opt(ws(preceded(tag("="), ws(parse_default_value)))),
        ),
        |(name, ty, default)| (name, ty.unwrap_or_else(|| "dyn".to_string()), default),
    );

    alt((parse_self, parse_star, parse_kw_param, parse_regular)).parse(input)
}

/// Keyword-named parameter (`fn`, `type`, …) — see `parse_kw_param`. Uses a
/// word boundary so that ordinary parameter names starting with a keyword
/// (`types`, `type_s`, `opener`) are not stolen by the relaxed parser.
fn parse_kw_param_name(input: &str) -> IResult<&str, String> {
    for kw in ["fn", "match", "type", "impl", "open", "high", "low", "set"] {
        if let Some(rest) = super::parser::kw_boundary(input, kw) {
            return Ok((rest, kw.to_string()));
        }
    }
    Err(nom::Err::Error(nom::error::Error::new(
        input,
        nom::error::ErrorKind::Tag,
    )))
}

fn parse_use_statement(input: &str) -> IResult<&str, Vec<AstNode>> {
    let (input, _) = ws(tag("use")).parse(input)?;
    let (input, path) = ws(parse_path).parse(input)?;
    parse_use_targets(path, input)
}

/// The tail of a `::` import path: an optional `::{a, b}` group and an optional
/// `;`. `import` shares this (see `stmt::parse_python_import`) so the two
/// spellings of a `::` path cannot drift apart.
pub(crate) fn parse_use_targets(path: Vec<String>, input: &str) -> IResult<&str, Vec<AstNode>> {
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

    let (input, params_full) = match delimited(
        ws(tag("(")),
        terminated(
            separated_list0(ws(tag(",")), ws(parse_param_full)),
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

    let params: Vec<(String, String)> = params_full
        .iter()
        .map(|(n, t, _)| (n.clone(), t.clone()))
        .collect();

    let (input, ret_opt) = match opt(preceded(
        ws(tag("->")),
        ws(alt((parse_py_quoted_type, parse_type))),
    ))
    .parse(input)
    {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };

    // Parse where clause if present
    let (input, where_clauses_opt) = opt(ws(parse_where_clause)).parse(input)?;
    let where_clauses = where_clauses_opt.unwrap_or_default();
    // 批次 465（backlog #61/#67 的 `fn…;` 成员）：`fn f(...);`——不带 `extern`
    // 前缀、以 `;` 收尾的**原型声明**——与 `extern fn f(...);` 走同一条路：
    // 签名承诺、external linkage、缺实现 = 链接期响亮失败（"宁可报错"红线），
    // 而不是解析失败把整个文件截断。`extern_opt.is_some()` 直接进；无前缀时
    // peek 一眼 `;`，是则按原型处理。
    let is_decl_only = extern_opt.is_some() || peek(ws(tag(";"))).parse(input).is_ok();
    let (input, (body, ret_expr, single_line)) = if is_decl_only {
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
                            // PY-A (batch 287): a Block whose last stmt is a
                            // `return` is a statement block, not a value —
                            // the `with lock: return …` desugar ends with the
                            // rewritten `return __with_ret_N`; promoting the
                            // block to ret_expr routed it through the expr
                            // path, which dropped that return and emitted
                            // `return 0` instead (silent misvalue).
                            AstNode::Block { body: inner } => {
                                if matches!(inner.last(), Some(AstNode::Return(_))) {
                                    None
                                } else {
                                    b.pop().map(Box::new)
                                }
                            }
                            AstNode::If { .. }
                            | AstNode::Call { .. }
                            | AstNode::PathCall { .. }
                            | AstNode::Match { .. }
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
    // PY-A: Python default argument values. The AST's `params` only carries
    // (name, type), so defaults travel as a body-prologue marker
    // `zeta_param_default(index, value)`: the Resolver collects them per
    // function and the MIR lowering fills omitted call arguments from that
    // table. Before this, `def add(a, b = 10)` + `add(5)` silently used 0 for
    // `b` — a wrong value with no diagnostic.
    let mut body = body;
    if !is_decl_only {
        let mut prologue: Vec<AstNode> = Vec::new();
        for (i, (_n, _t, default)) in params_full.iter().enumerate() {
            if let Some(d) = default {
                prologue.push(AstNode::ExprStmt {
                    expr: Box::new(AstNode::Call {
                        receiver: None,
                        method: "zeta_param_default".to_string(),
                        args: vec![AstNode::Lit(i as i64), d.clone()],
                        type_args: vec![],
                        structural: false,
                    }),
                });
            }
        }
        prologue.append(&mut body);
        body = prologue;
    }

    let input = if single_line {
        let (i, _) = ws(tag(";")).parse(input)?;
        i
    } else {
        input
    };
    let ret = ret_opt.unwrap_or_else(|| "()".to_string());
    let ast = if is_decl_only {
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

/// PY-A: quoted forward reference in a return annotation —
/// `-> "OhlcvRepairReport | None"`. Real Python uses strings for forward
/// refs; rejecting them aborted `parse_func` and, inside a class body, the
/// **entire class** (MarketDataFetcher lost every method after the first
/// quoted `->`). Accept the string and parse its first union arm as the
/// type; on failure keep the raw name (opaque Named).
fn parse_py_quoted_type(input: &str) -> IResult<&str, String> {
    let (input, node) = crate::frontend::parser::expr::parse_primary(input)?;
    let s = match node {
        AstNode::StringLit(s) => s,
        _ => {
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            )));
        }
    };
    let first = s.split('|').next().unwrap_or(&s).trim();
    if first.is_empty() {
        return Ok((input, "i64".to_string()));
    }
    if let Ok((_, ty)) = parse_type(first) {
        Ok((input, ty))
    } else {
        // Keep a usable type name (`OhlcvRepairReport`) rather than i64 so
        // later member lookups have something to hang onto.
        let name: String = first
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        Ok((input, if name.is_empty() { "i64".into() } else { name }))
    }
}

pub(crate) fn parse_class(input: &str) -> IResult<&str, AstNode> {
    let input = skip_decorator_lines(input);
    let (input, _) = ws(terminated(tag("class"), peek(none_of("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_")))).parse(input)?;
    let (input, name) = ws(parse_ident).parse(input)?;
    // PY-A: inheritance bases (`class A(B):`, `class S(abc.ABC):`) — V1 does
    // not model MRO, but *rejecting* the `(` used to Failure the whole class
    // and drop every following top-level def (`market_data_sources.py` lost
    // `_baostock_login` / `_from_rq_code` / …). Consume the base list and
    // continue; bases are ignored.
    let input = if let Ok((after_paren, _)) = ws(tag("(")).parse(input) {
        match after_paren.find(')') {
            Some(idx) => &after_paren[idx + 1..],
            None => {
                return Err(nom::Err::Failure(nom::error::Error::new(
                    input,
                    nom::error::ErrorKind::Tag,
                )));
            }
        }
    } else {
        input
    };
    let (input, _) = ws(tag("{")).parse(input)?;

    // Collect methods and __init__
    let mut methods: Vec<AstNode> = Vec::new();
    let mut init_params: Vec<(String, String)> = Vec::new();
    let mut init_stmts: Vec<AstNode> = Vec::new();
    let mut has_init = false;
    // PY-A: bare annotated fields (`@dataclass class P: x: int`) — no
    // `__init__`; the constructor is synthesized from these declarations.
    let mut annotated_fields: Vec<(String, String, Option<AstNode>)> = Vec::new();
    let mut cur = input;
    loop {
        let (next, _) = skip_ws_and_comments(cur)?;
        if next.starts_with('}') {
            cur = next;
            break;
        }
        // PY-A: skip a CLASS DOCSTRING — a bare string literal as the first
        // statement (`class C:\n    """doc."""`). It is documentation, not a
        // member; without this the class body failed to parse and the class —
        // plus everything after it — was silently dropped. Functions already
        // tolerated a docstring; classes did not.
        if let Ok((after, lit)) = crate::frontend::parser::expr::parse_primary(next) {
            if matches!(lit, AstNode::StringLit(_)) {
                cur = after;
                continue;
            }
        }
        // PY-A: method decorators (`@staticmethod`, `@property`, …) sit
        // inside the class body. Skipping them only *before* `class` left
        // `@staticmethod\n    def f` as the next item — `parse_func` failed,
        // the whole class aborted, and every method after `__init__` (often
        // the entire class in real code) was dropped. Consume decorator lines
        // here; V1 still ignores decorator *semantics* (no call rewriting).
        let next = skip_decorator_lines(next);
        let (next, _) = skip_ws_and_comments(next)?;
        if next.starts_with('}') {
            cur = next;
            break;
        }
        // def method(...) { ... } — reuse parse_func (def alias supported)
        match parse_func(next) {
            Ok((
                rest,
                AstNode::FuncDef {
                    name: mname,
                    params,
                    body,
                    ret,
                    ret_expr,
                    ..
                },
            )) => {
                if mname == "__init__" {
                    has_init = true;
                    init_params = params
                        .iter()
                        .filter(|(n, _)| n != "self" && n != "&self" && n != "&mut self")
                        .cloned()
                        .collect();
                    // `__init__` body may have promoted a trailing ExprStmt into
                    // ret_expr (parse_func); fold it back so field extraction sees it.
                    let mut init_body = body;
                    if let Some(re) = ret_expr {
                        init_body.push(AstNode::ExprStmt { expr: re });
                    }
                    init_stmts = init_body;
                } else {
                    // Python `def m(self, a, b)` → `fn m(&mut self, a, b)`
                    // `self` must be typed as the CLASS, not the literal
                    // "Self": with "Self" the field read inside a method could
                    // not find the struct at all, so every `self.<field>` was
                    // typed i64 (a `map` field's `.keys()` -> undefined `_keys`).
                    // Batch 291: a @classmethod's first parameter is `cls`,
                    // not `self`. Forcing `&mut self` prepended a receiver slot
                    // no call site fills (`CM.from_jq(cfg)` bound cfg into the
                    // cls slot and left `config` 0 — every `cfg.get(...)` then
                    // returned the default, silently). Keep `cls` as a plain
                    // parameter: the dotted call passes its real args first,
                    // and MIR rewrites `cls(...)` to the class constructor.
                    let is_classmethod =
                        params.first().map(|(n, _)| n == "cls").unwrap_or(false);
                    let mut new_params: Vec<(String, String)> = if is_classmethod {
                        Vec::new()
                    } else {
                        vec![("&mut self".to_string(), name.clone())]
                    };
                    for (pn, pt) in &params {
                        if pn != "self" && pn != "&self" && pn != "&mut self" {
                            new_params.push((pn.clone(), pt.clone()));
                        }
                    }
                    // An EXPLICIT return annotation wins; the body heuristic only
                    // applies to unannotated methods (the parser marks those "()").
                    // Include ret_expr so a sole `return "x"` / expression body
                    // still counts as a string return after parse_func promotion.
                    let ret = if !ret.is_empty() && ret != "()" && ret != "i64" {
                        ret.clone()
                    } else if body_is_string_return(&body)
                        || ret_expr
                            .as_ref()
                            .map_or(false, |e| matches!(e.as_ref(), AstNode::StringLit(_)))
                    {
                        "str".to_string()
                    } else {
                        "i64".to_string()
                    };
                    // CRITICAL: keep parse_func's ret_expr. A method whose only
                    // statement is an ExprStmt (`self.d.pop(key)`, `self.x`) has
                    // that stmt promoted out of `body` into `ret_expr`. Dropping
                    // it here left `__delitem__` / one-liner methods as empty
                    // stubs (ret 0) with no MIR for the call.
                    methods.push(AstNode::FuncDef {
                        name: mname,
                        generics: Vec::new(),
                        lifetimes: Vec::new(),
                        params: new_params,
                        ret,
                        body,
                        attrs: Vec::new(),
                        ret_expr,
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
            Err(e) => {
                // PY-A: a bare annotated field `x: int` (dataclass body).
                // Previously this aborted the whole class parse — and since
                // the class was the current top-level item, every statement
                // after it was silently dropped too (fail-open).
                match parse_param_full(next) {
                    Ok((rest, (fname, fty, def)))
                        if !fname.starts_with('*')
                            && fname != "self"
                            && fname != "&self"
                            && fname != "&mut self" =>
                    {
                        // `x: int = 40` — a dataclass field DEFAULT (parse_param_full
                        // already carries it). Without it the synthesized
                        // constructor had no value for the field and every read
                        // returned 0: `MarketCleanConfig()` gave `min_price 0 /
                        // drop_extreme 0 / max_abs 0` — silent wrong values that
                        // then drove the whole cleaning path.
                        annotated_fields.push((fname, fty, def));
                        cur = rest;
                    }
                    _ => return Err(e),
                }
            }
        }
    }
    let (input, _) = ws(tag("}")).parse(cur)?;

    // Field extraction from `__init__` `self.<field> = <rhs>`
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut field_inits: Vec<(String, AstNode)> = Vec::new();
    // PY-A: dataclass-style body — no `__init__`, fields declared as bare
    // annotations. Synthesize the constructor and field table from them, in
    // declaration order (`P(1, 2)` → `P { x: 1, y: 2 }`).
    // Dataclass field defaults become constructor-parameter defaults (the same
    // `zeta_param_default` prologue the parser emits for `def f(a, b=1)`), so
    // `MarketCleanConfig()` gets `min_price = 0.01` instead of 0. The marker
    // index counts `self`, which the synthesized constructor lacks — hence i+1.
    let mut dataclass_defaults: Vec<AstNode> = Vec::new();
    if !has_init && !annotated_fields.is_empty() {
        for (i, (n, t, def)) in annotated_fields.iter().enumerate() {
            fields.push((n.clone(), t.clone()));
            // Parameters keep DECLARATION order (the marker index must match).
            init_params.push((n.clone(), t.clone()));
            // `x: list[str] = list()` / `= dict()` — a CALL default. Lowering it
            // verbatim emitted a call to the builtin `list`, which has no runtime
            // symbol (`Undefined symbols: _list` in `jq_shim._G`). `list()` IS an
            // empty growable list, so map it to the literal form the ArrayLit
            // lowering already handles; same for `dict()`.
            let norm_default = |d: &AstNode| -> AstNode {
                if let AstNode::Call {
                    receiver: None,
                    method,
                    args,
                    ..
                } = d
                {
                    if args.is_empty() {
                        match method.as_str() {
                            "list" => return AstNode::ArrayLit(vec![]),
                            "dict" => return AstNode::DictLit { entries: vec![] },
                            _ => {}
                        }
                    }
                }
                d.clone()
            };
            match def {
                Some(d) => {
                    let d = &norm_default(d);
                    // Batch 291: the struct literal must read the PARAMETER,
                    // not the default expression. `CM(2.5)` with
                    // `slippage: float = 1.5` returned 1.5 — every explicit
                    // constructor argument was silently dropped (measured).
                    // Omitted arguments are filled from the marker below by
                    // the Resolver at the CALL site, exactly like
                    // `def f(a, b=1)`.
                    field_inits.push((n.clone(), AstNode::Var(n.clone())));
                    dataclass_defaults.push(AstNode::ExprStmt {
                        expr: Box::new(AstNode::Call {
                            receiver: None,
                            method: "zeta_param_default".to_string(),
                            args: vec![AstNode::Lit(i as i64), d.clone()],
                            type_args: vec![],
                            structural: false,
                        }),
                    });
                }
                None => {
                    field_inits.push((n.clone(), AstNode::Var(n.clone())));
                }
            }
        }
    }
    let param_names: Vec<&str> = init_params.iter().map(|(n, _)| n.as_str()).collect();
    for st in &init_stmts {
        if let AstNode::Assign(lhs, rhs) = st {
            if let AstNode::FieldAccess { base, field } = &**lhs {
                if let AstNode::Var(v) = &**base {
                    if v == "self" {
                        // `self._cost = cost or CostModel()` — a defaulting `or`
                        // evaluates to its LEFT operand whenever that is present,
                        // so the field's type is the left operand's. Typing it
                        // from the whole expression fell to i64, and every read
                        // through the field lost the class: `l._cost.commission`
                        // printed the raw 8-byte slot (4557750909289998844 for
                        // 0.0005) and `l._cost.fee(...)` mis-dispatched.
                        let mut rhs_eff: &AstNode = rhs;
                        while let AstNode::BinaryOp { op, left, .. } = rhs_eff {
                            if matches!(op.as_str(), "or" | "||" | "and" | "&&") {
                                rhs_eff = left;
                            } else {
                                break;
                            }
                        }
                        let ty = match rhs_eff {
                            AstNode::Lit(_) => "i64".to_string(),
                            AstNode::Bool(_) => "bool".to_string(),
                            // `self.x = {}` — a dict literal field. Without this arm
                            // the field typed i64, so EVERY map operation on it
                            // (`self.m.get(k, d)`, `.values()`, `.keys()`, `k in
                            // self.m`) fell through to an opaque bare symbol
                            // (`_get` / `_values` / `_exists` — 7 reference sites
                            // each in the REasyQuant local backtest, e.g.
                            // `PositionLedger._positions` / `._today_buys`).
                            AstNode::DictLit { .. } => "map".to_string(),
                            AstNode::FloatLit(_) => "f64".to_string(),
                            AstNode::StringLit(_) => "str".to_string(),
                            AstNode::ArrayLit(_) | AstNode::DynamicArrayLit { .. } => {
                                "DynamicArray".to_string()
                            }
                            AstNode::Var(name) if param_names.contains(&name.as_str()) => {
                                // `self.x = x` — take the PARAMETER's declared
                                // type when it has one. Hardcoding i64 ignored
                                // `def __init__(self, d: map)`: every library
                                // field became i64 and its own
                                // `self.data.keys()` turned into an undefined
                                // `_keys`.
                                // B3: unannotated params are `"dyn"`; treat that
                                // like the old empty/i64 default so constructor
                                // call-site field refinement (`dt == "i64"`) still
                                // upgrades `self.name = s` when `s` is a string.
                                init_params
                                    .iter()
                                    .find(|(n, _)| n == name)
                                    .map(|(_, ty)| ty.clone())
                                    .filter(|ty| !ty.is_empty() && ty != "dyn")
                                    .unwrap_or_else(|| "i64".to_string())
                            }
                            // `self._ledger = PositionLedger(...)` — a field
                            // holding an instance of a user class. With the old
                            // i64 default the inner calls (`self._ledger
                            // .clear_today_buys()`) fell to the bare-name
                            // dispatch, and two classes sharing the method name
                            // made codegen emit a self-recursive duplicate —
                            // infinite recursion (batch 294). Capitalized
                            // callee ⇒ remember the class name; MIR gen types
                            // the field `Named(cls)` and dispatches qualified.
                            AstNode::Call {
                                receiver: None,
                                method,
                                ..
                            } if method.chars().next().map_or(false, |c| c.is_uppercase()) => {
                                method.clone()
                            }
                            // A field whose initializer is a CALL keeps the
                            // callee's declared result type when the registry
                            // knows it (`self.cache_dir = os.path.join(...)` is a
                            // str). Falling back to i64 typed every such field as
                            // an integer: `len(f.cache_dir)` was 0 and
                            // `str(f.cache_dir)` printed the handle as digits.
                            AstNode::Call {
                                receiver, method, ..
                            } if receiver.is_some() => {
                                let mut ty = "i64".to_string();
                                if let Some(recv) = receiver {
                                    let mut parts: Vec<String> = Vec::new();
                                    let mut cur: &AstNode = recv;
                                    loop {
                                        match cur {
                                            AstNode::FieldAccess { base, field } => {
                                                parts.push(field.clone());
                                                cur = base;
                                            }
                                            AstNode::Var(root) => {
                                                parts.push(root.clone());
                                                break;
                                            }
                                            _ => break,
                                        }
                                    }
                                    parts.reverse();
                                    // The receiver's dotted path IS the module
                                    // (`os.path`), the method is the member.
                                    let module = parts.join(".");
                                    let fallback = module
                                        .strip_prefix(parts[0].as_str())
                                        .and_then(|rest| rest.strip_prefix('.'))
                                        .map(|rest| rest.to_string());
                                    if let Some(e) =
                                        crate::middle::pylib::find_member(&module, method)
                                            .or_else(|| {
                                                fallback.as_deref().and_then(|rest| {
                                                    crate::middle::pylib::find_member(
                                                        parts[0].as_str(),
                                                        rest,
                                                    )
                                                })
                                            })
                                    {
                                        if e.ret == "str" {
                                            ty = "str".to_string();
                                        }
                                    }
                                }
                                ty
                            }
                            // BATCH-298: `self._cash = float(initial_cash)` — the
                            // builtin conversions have a known result type. Left at
                            // i64 the field is arithmetic-typed as an integer, but
                            // an 8-byte slot holds a double's BIT PATTERN (see
                            // `StructFieldStore`), so `self._cash -= total` became
                            // `sub i64` on those bits: the ledger's cash never
                            // moved and the local backtest valued the portfolio at
                            // 0 (`final_value = portfolio.available_cash`).
                            AstNode::Call {
                                receiver: None,
                                method,
                                ..
                            } if matches!(
                                method.as_str(),
                                "float" | "int" | "str" | "bool"
                            ) =>
                            {
                                match method.as_str() {
                                    "float" => "f64",
                                    "str" => "str",
                                    "bool" => "bool",
                                    _ => "i64",
                                }
                                .to_string()
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
    //
    // PY-A: carry `__init__`'s default-argument markers onto the synthesized
    // constructor. The Resolver keys defaults by FUNCTION name, and `Pair(5)`
    // resolves to the constructor `Pair` — not to `__init__`. Without this,
    // `def __init__(self, x, y=2)` + `Pair(5)` read 0 for `y`: a wrong value
    // with no diagnostic. The marker's index counts `self`, which the
    // constructor's parameter list does not, so it shifts down by one.
    let mut ctor_body: Vec<AstNode> = dataclass_defaults.clone();
    for st in &init_stmts {
        if let AstNode::ExprStmt { expr } = st {
            if let AstNode::Call {
                receiver: None,
                method,
                args,
                ..
            } = &**expr
            {
                if method == "zeta_param_default" && args.len() == 2 {
                    if let AstNode::Lit(i) = &args[0] {
                        if *i >= 1 {
                            ctor_body.push(AstNode::ExprStmt {
                                expr: Box::new(AstNode::Call {
                                    receiver: None,
                                    method: method.clone(),
                                    args: vec![AstNode::Lit(*i - 1), args[1].clone()],
                                    type_args: vec![],
                                    structural: false,
                                }),
                            });
                        }
                    }
                }
            }
        }
    }
    ctor_body.push(AstNode::Return(Box::new(AstNode::StructLit {
        variant: name.clone(),
        fields: field_inits
            .iter()
            .map(|(f, expr)| (f.clone(), expr.clone()))
            .collect(),
    })));
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
                many0(ws(parse_top_level_entry)),
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
            many0(ws(parse_top_level_entry)),
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
    let mut definitions = alt((
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
    ));
    match definitions.parse(input) {
        Ok(r) => Ok(r),
        Err(def_err) => {
            // PY-A fail-loud: keywords that can only begin a DEFINITION
            // (`def`/`class`/`fn`/…) must never fall through to the statement
            // parser. Otherwise a definition whose body has one unsupported
            // construct degrades silently: `parse_stmt` accepts `def ` as a
            // bare identifier (4 bytes!) and then `name(args) ` as another
            // statement, SPLITTING the definition into stray fragments and
            // letting `many0` chew through the rest of the file. Failing here
            // instead stops the caller's `many0`, so the CLI reports the
            // dropped tail at the real starting point (`def …`).
            //
            // `if`/`for`/`while`/`match`/`try`/`with` are NOT in this list:
            // module-level statements are legitimate and must still reach
            // `parse_stmt`.
            const DEFINITION_KEYWORDS: &[&str] = &[
                "def", "class", "fn", "struct", "enum", "impl", "trait", "concept", "macro",
                "mod", "const", "pub",
            ];
            // `impl` is a definition keyword only when it begins an impl BLOCK
            // (`impl Type { … }`). Now that `impl` may be an ordinary variable
            // (`impl = strategy._make()`), `impl = …` / `impl.foo()` must still
            // reach the statement parser — otherwise top-level uses fail while
            // the same use inside a function body works.
            let impl_is_block = input.starts_with("impl")
                && !input[4..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                && matches!(
                    input[4..].trim_start().chars().next(),
                    Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '<'
                );
            let is_def_kw = DEFINITION_KEYWORDS
                .iter()
                .filter(|kw| **kw != "impl" || impl_is_block)
                .any(|kw| {
                    input.starts_with(kw)
                        && !input[kw.len()..]
                            .starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                });
            if is_def_kw {
                return Err(def_err);
            }
            // Also allow statements at top level
            crate::frontend::parser::stmt::parse_stmt(input)
        }
    }
}

/// One top-level entry: a `use`, an item, or an EMPTY statement (`;`).
///
/// The `;` arm exists because a bare semicolon is otherwise unparseable, and at
/// this level "unparseable" is fatal rather than cosmetic: `many0` stops at the
/// first failing entry, so the whole rest of the file is dropped (W1002).
/// Measured before this arm existed (against the pre-fix binary, `tools/
/// empty_stmt_inventory.sh` A wing): 12 of 13 top-level positions truncate — as
/// the first entry (one, two and three `;`), indented, and after a comment, an
/// expression statement, a `fn`/`def` definition, `use`, `import`, inside a
/// `mod { … }` body, at end of file. The two that did NOT are the ones that
/// prove the inconsistency rather than the fix: right after an assignment the
/// same `;` is invisible (the assignment eats it), and inside a block it was
/// already an empty statement. Whether one stray character deletes the program
/// must not depend on which rule happened to run before it.
///
/// Shared by the file loop and both `mod { … }` bodies (and the opt-in recovery
/// loop) so those paths cannot disagree about the same input.
fn parse_top_level_entry(input: &str) -> IResult<&str, Vec<AstNode>> {
    alt((
        parse_use_statement,
        map(parse_top_level_item, |node| vec![node]),
        value(vec![], tag(";")),
    ))
    .parse(input)
}

pub fn parse_zeta(input: &str) -> IResult<&str, Vec<AstNode>> {
    // PY-1: normalize indentation blocks to braces before parsing (design of
    // record: docs/python-syntax.md R1). Brace-style sources pass through
    // unchanged (Ok(None) keeps the original &str so `remaining` slices stay
    // valid).
    crate::frontend::indent::clear_last_preprocess();
    match crate::frontend::indent::indent_preprocess(input) {
        Ok(Some(processed)) => {
            if let Ok(path) = std::env::var("ZETA_DUMP_PP") {
                let _ = std::fs::write(&path, &processed);
            }
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
/// PY-A: a module top-level bare assignment `X = e` (rhs is not a lambda/
/// closure) marks `X` as a module global. Reads from other functions fall
/// back to the shared env without an explicit `global` declaration.
fn collect_module_global(stmt: &AstNode, out: &mut Vec<String>) {
    // Assign, AssignOp, and Let all bind a bare Var name at module level.
    let bind_name = match stmt {
        AstNode::Assign(lhs, rhs) => {
            if matches!(&**rhs, AstNode::Closure { .. }) {
                return;
            }
            match bare_bound_name(lhs) {
                Some(n) => Some(n),
                None => return,
            }
        }
        AstNode::AssignOp { target, .. } => match bare_bound_name(target) {
            Some(n) => Some(n),
            None => return,
        },
        AstNode::Let { pattern, expr, .. } => {
            if matches!(&**expr, AstNode::Closure { .. }) {
                return;
            }
            match bare_bound_name(pattern) {
                Some(n) => Some(n),
                None => return,
            }
        }
        _ => return,
    };
    if let Some(n) = bind_name {
        if !out.contains(&n) {
            out.push(n);
        }
    }
}

/// A binding target that names exactly one slot: `x`, or an annotated form of
/// it (`x: dict[str, Any] = {}` parses to `TypeAnnotatedPattern { Var("x") }`).
/// Batch 413: skipping the annotated wrapper left the name out of
/// `module_globals`, so no `zeta_module_decl` was emitted and every other
/// function's read of that module global fell back to an unregistered env slot
/// (measured: a module-level `dict[str, str]` read back as empty, then a
/// SIGSEGV in `map_insert` on the real corpus).
fn bare_bound_name(target: &AstNode) -> Option<String> {
    match target {
        AstNode::Var(n) => Some(n.clone()),
        AstNode::TypeAnnotatedPattern { pattern, .. } => match &**pattern {
            AstNode::Var(n) => Some(n.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// (an `if __name__ == "__main__":` guard body unwraps to a Block, bare calls,
/// import no-ops) are collected into a synthesized `fn main` when the module
/// has none — otherwise the compiled binary has no entry point.
/// `main()` as a bare statement (the unwrapped `__main__` guard).
fn is_bare_main_call(a: &AstNode) -> bool {
    let call = match a {
        AstNode::ExprStmt { expr } => &**expr,
        other => other,
    };
    matches!(
        call,
        AstNode::Call { receiver: None, method, args, .. } if method == "main" && args.is_empty()
    )
}

/// BATCH-290: set by the resolver's IMPORT path around `parse_zeta`. Tells
/// `synthesize_implicit_main` this file is never the executable entry, so the
/// `__main__` guard must survive as a runtime check and `main` must stay a
/// function. Compilation is single-threaded; a plain atomic suffices.
static PARSING_IMPORTED_MODULE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_parsing_imported_module(on: bool) {
    PARSING_IMPORTED_MODULE.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn parsing_imported_module() -> bool {
    PARSING_IMPORTED_MODULE.load(std::sync::atomic::Ordering::Relaxed)
}

/// BATCH-290: `if __name__ == "__main__":` (either operand order, `==` or `is`).
/// The parser no longer unwraps it, so it reaches here as an `If` node.
fn is_main_guard(a: &AstNode) -> bool {
    let cond = match a {
        AstNode::If { cond, else_, .. } if else_.is_empty() => &**cond,
        _ => return false,
    };
    let op_ok = matches!(cond, AstNode::BinaryOp { op, .. } if op == "==" || op == "is");
    if !op_ok {
        return false;
    }
    let (left, right) = match cond {
        AstNode::BinaryOp { left, right, .. } => (left, right),
        _ => return false,
    };
    let is_name = |n: &AstNode| matches!(n, AstNode::Var(v) if v == "__name__");
    let is_main = |n: &AstNode| matches!(n, AstNode::StringLit(s) if s == "__main__");
    (is_name(left) && is_main(right)) || (is_main(left) && is_name(right))
}

/// BATCH-290: in a file that defines `main`, a `__main__` guard's body is
/// spliced into the module statements (which the merge prepends to `main`),
/// minus any bare `main()` self-call — the old parse-time unwrap, moved to
/// the one place that knows the recursion is unsafe.
fn splice_main_guard_body(
    guard: AstNode,
    main_body: &mut Vec<AstNode>,
    module_globals: &mut Vec<String>,
) {
    let AstNode::If { then, .. } = guard else { return };
    for node in then {
        if is_bare_main_call(&node) {
            continue;
        }
        collect_module_global(&node, module_globals);
        main_body.push(node);
    }
}

/// Marks a `main` that carries a **module body**: the one this function
/// synthesizes when the source declared no entry function, and the user's
/// `main` when module statements got prepended into it. `MirGen` otherwise ends
/// every body with its tail value (`gen.rs:1062`), and for the entry function
/// that value is the process exit code through clang's crt — so a script ending
/// in the bare expression `sum(l)` exited 15 (task #55; CPython discards it).
/// Deliberately NOT derived from "did the indent preprocessor fire": that
/// predicate is `!changed` (indent.rs:237), so a python file with no indented
/// block counts as brace-style — the same flaw task #51 records for `//`, and
/// it would have missed every flat reproducer in this family.
pub const PY_ENTRY_ATTR: &str = "py_entry";

/// Lift every `static` declaration out of the statement lists it can reach and
/// put it at module level, immediately before the item that contained it, as an
/// ordinary module assignment — the form the module-global machinery already
/// runs once at program start (`collect_module_global` + the `zeta_module_decl`
/// markers below, then the env read/write paths in `gen.rs`). Zeta has no
/// per-function static storage, and what Rust guarantees for
/// `static mut counter: u64 = 0` is exactly "one cell, initialized before anyone
/// can read it, alive for the whole run". The declaration stays in the body as a
/// marker so the function that wrote it binds its name to that cell.
///
/// Keyed on the statement-list shapes only, so a `Static` inside a
/// `macro_rules!` body (expanded after this pass) keeps `hoisted: false` and
/// `MirGen` reports W1008 rather than resetting the cell on every call.
fn hoist_statics(items: Vec<AstNode>) -> Vec<AstNode> {
    // Names the module binds at top level: lifting a `static` under one of those
    // would overwrite the module's own cell before `main` runs.
    let mut claimed: std::collections::HashSet<String> =
        items.iter().filter_map(module_bound_name).collect();
    let mut out = Vec::with_capacity(items.len());
    for mut item in items {
        if let AstNode::Static { name, expr, .. } = item {
            claimed.insert(name.clone());
            out.push(AstNode::Assign(Box::new(AstNode::Var(name)), expr));
            continue;
        }
        hoist_statics_from(&mut item, &mut out, &mut claimed, false);
        out.push(item);
    }
    out
}

/// The name one module-level item binds, if it binds a bare one.
fn module_bound_name(item: &AstNode) -> Option<String> {
    match item {
        AstNode::Assign(lhs, _) | AstNode::AssignOp { target: lhs, .. } => match &**lhs {
            AstNode::Var(n) => Some(n.clone()),
            _ => None,
        },
        AstNode::Let { pattern, .. } => match &**pattern {
            AstNode::Var(n) => Some(n.clone()),
            _ => None,
        },
        AstNode::ConstDef { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// Lift one body-local `static`, and report whether it happened. Two
/// declarations of one name need two cells and this rewrite can only spell one,
/// so the second is left unlifted (`MirGen` calls it a local, out loud) rather
/// than sharing a cell in silence.
fn lift_one(
    name: &str,
    expr: &AstNode,
    claimed: &mut std::collections::HashSet<String>,
    pulled: &mut Vec<AstNode>,
) -> bool {
    if !claimed.insert(name.to_string()) {
        eprintln!(
            "warning: [W1009] `{name}` is declared `static` in more than one place (or under a \
             name the module already binds) — each would need its own cell and this rewrite can \
             only lift one, so this declaration was left alone"
        );
        return false;
    }
    pulled.push(AstNode::Assign(
        Box::new(AstNode::Var(name.to_string())),
        Box::new(expr.clone()),
    ));
    true
}

/// Recurse through the statement lists of one node, lifting the `static`s found
/// in them up to module level.
///
/// `in_body` says whether the lists below were reached through a body — i.e.
/// whether `Resolver::register` would miss a `use` sitting in them (see
/// `from_block`).
fn hoist_statics_from(
    node: &mut AstNode,
    pulled: &mut Vec<AstNode>,
    claimed: &mut std::collections::HashSet<String>,
    in_body: bool,
) {
    fn from_block(
        stmts: &mut Vec<AstNode>,
        pulled: &mut Vec<AstNode>,
        claimed: &mut std::collections::HashSet<String>,
        in_body: bool,
    ) {
        for stmt in stmts.iter_mut() {
            if let AstNode::Static {
                name, expr, hoisted, ..
            } = stmt
            {
                *hoisted = lift_one(name, expr, claimed, pulled);
                continue;
            }
            // 批次 393: a `use` inside a function body is a no-op where it stands —
            // the module load happens in `Resolver::register`, which walks item
            // lists but never a function body. So there the import moves up and the
            // statement keeps a placeholder. One copy per path: the same module
            // loaded twice would just re-register the same declarations.
            //
            // `in_body` is what keeps this off text that is NOT a body: a top-level
            // `import a::b;` is lowered by `parse_python_import` into a
            // `Block` item that already holds an `AstNode::Use`, and lifting that
            // would rewrite a working in-place import into a module-level one plus
            // an `Ignore` — measured to break batch 337's `import` ≡ `use` MIR
            // equality (`Return val` 4 → 5). Only lists reached through a body are
            // lifted, so the flag propagates instead of being re-decided per node.
            if in_body {
                if let AstNode::Use { path } = stmt {
                    let lifted = AstNode::Use { path: path.clone() };
                    if !pulled.contains(&lifted) {
                        pulled.push(lifted);
                    }
                    *stmt = AstNode::Ignore;
                    continue;
                }
            }
            hoist_statics_from(stmt, pulled, claimed, in_body);
        }
    }
    match node {
        // A `Block` carries no verdict of its own: the same node is the shape a
        // top-level `import` lowers to AND the shape of any nested body, so it
        // inherits the answer instead of deciding.
        AstNode::Block { body } => from_block(body, pulled, claimed, in_body),
        AstNode::Program(items) | AstNode::ModDef { items, .. } => {
            from_block(items, pulled, claimed, in_body)
        }
        AstNode::FuncDef { body, .. }
        | AstNode::ImplBlock { body, .. }
        | AstNode::Loop { body }
        | AstNode::Unsafe { body }
        | AstNode::ComptimeBlock { body } => from_block(body, pulled, claimed, true),
        AstNode::Method { body, .. } => {
            if let Some(body) = body {
                from_block(body, pulled, claimed, true);
            }
        }
        AstNode::If { then, else_, .. } | AstNode::IfLet { then, else_, .. } => {
            from_block(then, pulled, claimed, true);
            from_block(else_, pulled, claimed, true);
        }
        AstNode::For {
            body, else_body, ..
        }
        | AstNode::While {
            body, else_body, ..
        } => {
            from_block(body, pulled, claimed, true);
            from_block(else_body, pulled, claimed, true);
        }
        AstNode::ConceptDef { methods, .. } => from_block(methods, pulled, claimed, true),
        AstNode::Match { arms, .. } => {
            for arm in arms.iter_mut() {
                hoist_statics_from(&mut arm.body, pulled, claimed, true);
            }
        }
        AstNode::Closure { body, .. } | AstNode::ExprStmt { expr: body } => {
            hoist_statics_from(body, pulled, claimed, in_body)
        }
        _ => {}
    }
}

fn synthesize_implicit_main(asts: Vec<AstNode>) -> Vec<AstNode> {
    let asts = hoist_statics(asts);
    let has_main = asts
        .iter()
        .any(|a| matches!(a, AstNode::FuncDef { name, .. } if name == "main"));
    let mut out = Vec::with_capacity(asts.len() + 1);
    let mut main_body: Vec<AstNode> = Vec::new();
    // PY-A (任务 #55): a `__main__` guard is itself module-body evidence even when
    // its body is only the bare `main()` self-call that gets dropped (q12:
    // `def main(): …` + `if __name__ == "__main__": main()` leaves `main_body`
    // empty, so the length test alone can't see it). Guard syntax does not occur
    // in a brace-style source, so this cannot mark `fn main() -> i64 { 42 }`.
    let mut saw_main_guard = false;
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
    // PY-A: names assigned directly at module top level (bare `X = e`)
    // become module globals — reads from other functions fall back to the
    // shared env without an explicit `global` declaration (Python semantics).
    // Only collected here (the implicit-main path): explicit `fn main`
    // bodies never touch this, so lambda/closure bindings are unaffected.
    let mut module_globals: Vec<String> = Vec::new();
    // BATCH-290: when parsing an IMPORTED module, the `__main__` guard must
    // survive as a runtime check (its `__name__` lowers to the module name →
    // false) and the user's `main` must stay a FUNCTION — the resolver runs the
    // module statements as the import-time initializer, so merging them into
    // `main` would execute the whole entry point on every import.
    let module_ctx = parsing_imported_module();
    for a in asts {
        match a {
            // PY-A: a class desugars to [struct, impl, ctor] wrapped in a
            // Block — lift definitions back out to top level.
            AstNode::Block { body } => {
                for node in body {
                    if is_definition(&node) {
                        out.push(node);
                    } else {
                        if has_main && is_bare_main_call(&node) {
                            continue;
                        }
                        if has_main && !module_ctx && is_main_guard(&node) {
                            saw_main_guard = true;
                            splice_main_guard_body(node, &mut main_body, &mut module_globals);
                            continue;
                        }
                        collect_module_global(&node, &mut module_globals);
                        main_body.push(node);
                    }
                }
            }
            other if is_definition(&other) => out.push(other),
            stmt => {
                // BATCH-290: the `if __name__ == "__main__":` guard now SURVIVES
                // parsing as an If (stmt.rs no longer unwraps it). When this file
                // defines `main`, its module statements get PREPENDED into that
                // main — a surviving guard whose body calls `main()` would then
                // recurse forever (measured: an infinite "A" stream, then SEGV),
                // and the program entry already calls `main`. Splice the guard's
                // non-`main()` work in and drop the self-call.
                if has_main && is_bare_main_call(&stmt) {
                    continue;
                }
                if has_main && !module_ctx && is_main_guard(&stmt) {
                    saw_main_guard = true;
                    splice_main_guard_body(stmt, &mut main_body, &mut module_globals);
                    continue;
                }
                collect_module_global(&stmt, &mut module_globals);
                main_body.push(stmt);
            }
        }
    }
    // Emit zeta_module_decl markers (mirroring parse_global's
    // zeta_nonlocal_decl) so the resolver registers these names as module
    // globals and gen.rs routes reads/writes through the env.
    for name in &module_globals {
        main_body.insert(0, AstNode::ExprStmt {
            expr: Box::new(AstNode::Call {
                receiver: None,
                method: "zeta_module_decl".to_string(),
                args: vec![AstNode::StringLit(name.clone())],
                type_args: vec![],
                structural: false,
            }),
        });
    }
    // A file that ALREADY defines `main` (the `if __name__ == "__main__": main()`
    // convention — every real Python entry point) used to take an early return
    // here, so its module-level statements were NEVER COLLECTED AND NEVER RAN:
    // `jq_wufu_local.py`'s `if os.environ.get('REPLAYQUANT_LOCAL') == '1': import
    // jq_wufu as _strategy; from strategies.code.jq_shim import (...)` silently
    // did nothing, leaving `_strategy` unbound and the whole local backtest
    // crashing before its first output. Prepend the module statements to the
    // user's `main` instead (Python runs them at import, i.e. before main).
    if has_main {
        if module_ctx {
            // Imported module: `main` stays a function (registered mangled by
            // the resolver); the module statements go into a dedicated carrier
            // the resolver extracts as the import-time initializer.
            out.push(AstNode::FuncDef {
                name: "__zeta_module_body__".to_string(),
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
            return out;
        }
        for node in &mut out {
            if let AstNode::FuncDef {
                name,
                body,
                attrs,
                ..
            } = node
            {
                if name == "main" {
                    let mut merged = std::mem::take(&mut main_body);
                    // PY-A (任务 #55): module statements really were merged in —
                    // this `main` is now entry + module body at once, so it is a
                    // module body as much as the synthesized one below. A
                    // brace-style source gets here with an empty `merged`, which
                    // is what keeps `fn main() -> i64 { 42 }` exiting 42.
                    let module_body = !merged.is_empty() || saw_main_guard;
                    merged.extend(std::mem::take(body));
                    if module_body {
                        attrs.push(PY_ENTRY_ATTR.to_string());
                    }
                    *body = merged;
                    break;
                }
            }
        }
        return out;
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
        attrs: vec![PY_ENTRY_ATTR.to_string()],
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
    // C2 recovery is opt-in (`ZETA_PARSE_RECOVER=1`). Default keeps historical
    // `many0` stop-at-first-error behaviour so official/corpus floors stay green
    // while C1 line maps still apply to the leftover tail (W1002).
    if crate::diagnostics::env_flag("ZETA_PARSE_RECOVER") {
        return parse_zeta_impl_recover(input);
    }

    let (input, _) = skip_ws_and_comments(input)?;

    let parse_result = many0(ws(parse_top_level_entry)).parse(input);

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

fn parse_zeta_impl_recover(input: &str) -> IResult<&str, Vec<AstNode>> {
    let (mut input, _) = skip_ws_and_comments(input)?;
    let mut asts: Vec<AstNode> = Vec::new();

    loop {
        let (next, _) = match skip_ws_and_comments(input) {
            Ok(v) => v,
            Err(_) => break,
        };
        input = next;
        if input.is_empty() {
            break;
        }

        let before_len = input.len();

        // Same entry grammar as the default path (including the empty `;`), so a
        // shape cannot be fatal under `many0` and merely skipped under recovery.
        match parse_top_level_entry(input) {
            Ok((rest, nodes)) => {
                if rest.len() == before_len {
                    break;
                }
                for n in nodes {
                    if !matches!(n, AstNode::Skip) {
                        asts.push(n);
                    }
                }
                input = rest;
                continue;
            }
            Err(_) => {
                let snippet: String = input.chars().take(48).collect();
                let base_off =
                    crate::frontend::indent::remaining_byte_offset(input, "");
                let line = crate::frontend::indent::original_line_at(base_off, "");
                eprintln!(
                    "warning: [W1003] :{line}: skipped unparseable top-level item; \
                     syncing to next def/class/import/…. Near: '{}'",
                    snippet.replace('\n', "\\n")
                );
                if crate::diagnostics::env_flag("ZETA_STRICT_PARSE") {
                    return Err(nom::Err::Failure(nom::error::Error::new(
                        input,
                        nom::error::ErrorKind::Tag,
                    )));
                }
                let advanced = skip_to_top_level_sync(input);
                if advanced.len() >= before_len {
                    break;
                }
                input = advanced;
            }
        }
    }

    Ok((input, asts))
}

/// C2: advance to the next line that looks like a top-level sync point.
fn skip_to_top_level_sync(input: &str) -> &str {
    let mut rest = match input.find('\n') {
        Some(p) => &input[p + 1..],
        None => return "",
    };
    while !rest.is_empty() {
        let line_end = rest.find('\n').unwrap_or(rest.len());
        let line = &rest[..line_end];
        let trimmed = line.trim_start();
        if is_top_level_sync_line(trimmed) {
            return rest;
        }
        rest = if line_end < rest.len() {
            &rest[line_end + 1..]
        } else {
            ""
        };
    }
    ""
}

fn is_top_level_sync_line(trimmed: &str) -> bool {
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
        return false;
    }
    if trimmed.starts_with('@') {
        return true;
    }
    const PREFIXES: &[&str] = &[
        "def ", "def\t", "class ", "class\t", "fn ", "fn\t", "import ", "from ",
        "struct ", "enum ", "impl ", "concept ", "trait ", "type ", "const ",
        "async def", "async fn", "pub ", "try:", "try ", "with ",
    ];
    for p in PREFIXES {
        if trimmed.starts_with(p) {
            return true;
        }
    }
    for kw in ["def", "class", "fn", "import", "from", "struct", "enum", "impl"] {
        if trimmed == kw {
            return true;
        }
        if let Some(rest) = trimmed.strip_prefix(kw) {
            let c = rest.chars().next().unwrap_or('\0');
            if c == '(' || c == ':' || c.is_whitespace() {
                return true;
            }
        }
    }
    false
}
