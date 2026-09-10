//! Debug tool: dump the indent-preprocessed source (PY-1).
//! Usage: cargo run --release --bin indent_dump -- <file.z>
fn main() {
    let path = std::env::args().nth(1).expect("usage: indent_dump <file.z>");
    let src = std::fs::read_to_string(&path).expect("read failed");
    match zetac::frontend::indent::indent_preprocess(&src) {
        Ok(Some(out)) => print!("{out}"),
        Ok(None) => print!("{src}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
