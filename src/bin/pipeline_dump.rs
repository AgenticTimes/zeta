//! Debug tool: dump pipeline stages for a .z file (parse → register → lower).
//! Usage: cargo run --release --bin pipeline_dump -- <file.z>
use zetac::frontend::ast::AstNode;
use zetac::frontend::parser::top_level::parse_zeta;
use zetac::middle::resolver::resolver::Resolver;

fn main() {
    let path = std::env::args().nth(1).expect("usage: pipeline_dump <file.z>");
    let code = std::fs::read_to_string(&path).expect("read failed");
    let (remaining, asts) = parse_zeta(&code).expect("parse failed");
    println!("parsed {} items; remaining = {:?} (len {})", asts.len(), remaining, remaining.len());
    for a in &asts {
        if let AstNode::FuncDef { name, generics, params, body, .. } = a {
            println!("  fn {} generics={} params={}", name, generics.len(), params.len());
            if std::env::args().any(|a| a == "-ast") {
                for st in body {
                    println!("    body: {:#?}", st);
                }
            }
        } else {
            println!("  {:?}", std::mem::discriminant(a));
        }
    }

    let mut resolver = Resolver::new();
    for ast in &asts {
        resolver.register(ast.clone());
    }
    let _ = resolver.typecheck(&asts);
    let func_asts = resolver.get_registered_funcs();
    println!("registered funcs: {}", func_asts.len());
    for f in &func_asts {
        if let AstNode::FuncDef { name, .. } = f {
            println!("  registered: {}", name);
        }
    }
    for f in &func_asts {
        if let AstNode::FuncDef { name, .. } = f {
            let mir = resolver.lower_to_mir(f);
            println!("lowered {} -> stmts {}", name, mir.stmts.len());
            if std::env::args().any(|a| a == "-v") {
                for st in &mir.stmts {
                    println!("    {:?}", st);
                }
                for (id, e) in &mir.exprs {
                    println!("    e{} = {:?}", id, e);
                }
                for (id, t) in &mir.type_map {
                    println!("    t{} = {:?}", id, t);
                }
            }
        }
    }
    let used = resolver.collect_used_specializations(&asts);
    println!("used_specs: {:?}", used.keys().collect::<Vec<_>>());
    for (fn_name, specs) in &used {
        println!("  {} -> {:?}", fn_name, specs);
    }
}
