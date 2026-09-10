// src/frontend/indent.rs
//! Indentation-based block preprocessing (PY-1 / PY-5).
//!
//! Converts Python-style indented blocks into brace blocks before the nom
//! parser runs. Design of record: `docs/python-syntax.md` (R1/R7).
//!
//! Rules:
//!   - Only spaces define indentation; a tab in a code line's leading
//!     whitespace is a hard error (`IndentError::TabIndent`).
//!   - A line is a block header iff its code part ends with `:`, does not
//!     contain `{`, starts with a known block keyword (see `HEADER_KEYWORDS`,
//!     optionally preceded by `pub`/`const`/...), and the next code line is
//!     indented deeper. The header's trailing colon is stripped and ` {` is
//!     appended.
//!   - Dedent (current indent < top of stack) emits `}` at the start of the
//!     next code line, once per level.
//!   - Blank lines, comment-only lines and lines inside triple-quoted strings
//!     carry no bookkeeping: they never open or close blocks.
//!   - String literals are blanked before comment/colon/brace detection, so
//!     `print("http://x")` and `let s = "a:"` cannot misfire.
//!   - At EOF any still-open blocks are closed with `}` on a fresh line.
//!   - Lines that already use braces are untouched — mixed styles work.
//!
//! V1 limitations: single-line blocks (`if x: stmt`) are not supported;
//! a header colon must be followed by a newline + deeper indent.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentError {
    /// 1-based line number of the offending code line.
    TabIndent { line: usize },
}

impl fmt::Display for IndentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndentError::TabIndent { line } => write!(
                f,
                "TAB_INDENT_ERROR at line {line}: tabs are not allowed for indentation (spaces only)"
            ),
        }
    }
}

/// Keywords whose colon form opens an indented block. Gating on a known
/// keyword guarantees brace-style sources that merely end a line with `:`
/// (e.g. a wrapped `let x:\n    i64 = 5`) pass through untouched.
const HEADER_KEYWORDS: &[&str] = &[
    "fn", "def", "if", "elif", "else", "for", "while", "loop", "match",
    "struct", "enum", "impl", "trait", "concept", "unsafe", "comptime", "mod",
];

/// Modifiers that may precede a header keyword (`pub const fn f():` ...).
const HEADER_PREFIXES: &[&str] = &["pub", "const", "comptime", "async", "extern"];

/// Per-line scan result.
struct LineInfo {
    /// Code content: string literals blanked to bare quote delimiters and
    /// `//` comments stripped. Whitespace-only ⇒ no bookkeeping for the line.
    code: String,
    /// Byte offset in the raw line where the code part ends (comment start
    /// or end of line).
    code_end: usize,
    /// Leading-space width of the line (only meaningful when code is
    /// non-empty).
    indent: usize,
}

/// Preprocess `input`. Returns `Ok(None)` when nothing looks python-style
/// (cheap passthrough for brace-style sources — official test-suite
/// guarantee), `Ok(Some(transformed))` when block normalization happened, or
/// `Err` on a hard indentation error.
pub fn indent_preprocess(input: &str) -> Result<Option<String>, IndentError> {
    let lines: Vec<&str> = input.split('\n').collect();

    let mut infos: Vec<LineInfo> = Vec::with_capacity(lines.len());
    let mut in_triple: Option<char> = None;
    for (idx, line) in lines.iter().enumerate() {
        infos.push(scan_line(line, &mut in_triple).map_err(|_| IndentError::TabIndent {
            line: idx + 1,
        })?);
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 1);
    let mut stack: Vec<usize> = Vec::new();
    let mut changed = false;

    for i in 0..lines.len() {
        let info = &infos[i];
        if info.code.trim().is_empty() {
            // Blank / comment-only / inside-triple line: verbatim, no
            // bookkeeping (never opens or closes a block).
            out.push(lines[i].to_string());
            continue;
        }

        // Dedent: emit one `}` per popped level, on its own line before the
        // dedented line (comments/blank lines between are skipped by the
        // parser, so order relative to them is irrelevant).
        while stack.last().map_or(false, |&top| top > info.indent) {
            stack.pop();
            out.push("}".to_string());
            changed = true;
        }

        let next_indent = next_code_indent(&infos, i + 1);
        let opens = info.code.trim_end().ends_with(':')
            && !info.code.contains('{')
            && is_header(&info.code)
            && next_indent.map_or(false, |ni| ni > info.indent);

        let line_out = if opens {
            let mut l = String::new();
            l.push_str(strip_trailing_colon(lines[i], info.code_end).trim_end());
            l.push_str(" {");
            // Preserve any trailing comment after the header colon.
            l.push_str(&lines[i][info.code_end..]);
            // Push the BODY indent (the next code line's), so the block
            // closes as soon as a line dedents below the body.
            stack.push(next_indent.unwrap());
            changed = true;
            l
        } else {
            lines[i].to_string()
        };
        out.push(line_out);
    }

    // Close any still-open blocks on a fresh line (never appended to the
    // last line — it may be a comment, which would swallow the braces).
    if !stack.is_empty() {
        out.push("}".repeat(stack.len()));
        changed = true;
    }

    if changed {
        Ok(Some(out.join("\n")))
    } else {
        Ok(None)
    }
}

/// Indent of the first code line at or after `from`, skipping blank,
/// comment-only and string-continuation lines.
fn next_code_indent(infos: &[LineInfo], from: usize) -> Option<usize> {
    infos[from..]
        .iter()
        .find(|info| !info.code.trim().is_empty())
        .map(|info| info.indent)
}

/// Raw line with its header colon (and trailing spaces) removed.
fn strip_trailing_colon(raw: &str, code_end: usize) -> &str {
    let head = raw[..code_end].trim_end();
    match head.ends_with(':') {
        true => head[..head.len() - 1].trim_end(),
        false => head,
    }
}

/// Is this code line a block header? Either it starts with a block keyword
/// (after modifiers) or it is a `let` whose initializer opens a block
/// expression (`let r = loop:`, `let m = match c:`, `let x = if c:`, ...).
fn is_header(code: &str) -> bool {
    if is_header_start(code) {
        return true;
    }
    let t = code.trim_start();
    if first_word(t) != Some("let") {
        return false;
    }
    match find_top_level_eq(t) {
        Some(pos) => matches!(
            first_word(t[pos + 1..].trim_start()),
            Some("if") | Some("match") | Some("loop") | Some("unsafe") | Some("comptime")
        ),
        None => false,
    }
}

/// Byte offset of the first top-level (bracket-depth-0) assignment `=`,
/// skipping `==`, `!=`, `<=`, `>=` and compound assignments.
fn find_top_level_eq(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'=' if depth == 0 => {
                if b.get(i + 1) == Some(&b'=') {
                    i += 2;
                    continue;
                }
                if i > 0
                    && matches!(
                        b[i - 1],
                        b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^'
                    )
                {
                    i += 1;
                    continue;
                }
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Does the code part start with a block-header keyword (after modifiers)?
fn is_header_start(code: &str) -> bool {
    let mut rest = code.trim_start();
    loop {
        match first_word(rest) {
            Some(w) if HEADER_PREFIXES.contains(&w) => rest = rest[w.len()..].trim_start(),
            _ => break,
        }
    }
    match first_word(rest) {
        Some(w) => HEADER_KEYWORDS.contains(&w),
        None => false,
    }
}

fn first_word(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    if end == 0 {
        None
    } else {
        Some(&s[..end])
    }
}

/// Scan one raw line into its `LineInfo`, updating the cross-line triple-quote
/// state. `Err(())` = tab in leading whitespace of a code line.
fn scan_line(line: &str, in_triple: &mut Option<char>) -> Result<LineInfo, ()> {
    let b = line.as_bytes();
    let mut i = 0usize;
    let mut code = String::new();
    let start_in_triple = in_triple.is_some();
    let mut indent = 0usize;
    let mut saw_tab = false;

    if !start_in_triple {
        // Leading whitespace: spaces only; a tab here is flagged (error only
        // if the line turns out to have code — tabs before a comment are
        // fine, as in Python).
        while i < b.len() {
            match b[i] {
                b' ' => {
                    indent += 1;
                    i += 1;
                }
                b'\t' => {
                    saw_tab = true;
                    indent += 1;
                    i += 1;
                }
                b'\r' => i += 1, // tolerate CRLF
                _ => break,
            }
        }
    }

    // Scan the code part: blank string literals, cut `//` comments.
    let mut code_end = line.len();
    while i < b.len() {
        if let Some(q) = *in_triple {
            match find_triple_close(line, i, q) {
                Some(p) => {
                    code.push_str("\"\"\"");
                    i = p;
                    *in_triple = None;
                }
                None => break, // rest of the line is string content
            }
            continue;
        }
        let c = b[i];
        match c {
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                code_end = i;
                break;
            }
            b'\'' | b'"' => {
                let q = c as char;
                if line[i..].starts_with(&q.to_string().repeat(3)) {
                    code.push_str("\"\"\"");
                    i += 3;
                    *in_triple = Some(q); // the loop top hunts for the close
                } else {
                    code.push(q);
                    i += 1;
                    let mut closed = false;
                    while i < b.len() {
                        if b[i] == b'\\' {
                            i += 2;
                            continue;
                        }
                        if b[i] as char == q {
                            i += 1;
                            closed = true;
                            break;
                        }
                        i += 1;
                    }
                    code.push(q);
                    if !closed {
                        // Unterminated single-line string: leave the rest to
                        // the parser's own error; not a block header.
                        code_end = i;
                        break;
                    }
                }
            }
            _ => {
                let len = utf8_len(c);
                code.push_str(&line[i..i + len]);
                i += len;
            }
        }
    }

    // Lines that START inside a triple string are physical continuations of
    // an earlier logical line: no bookkeeping, matching Python (their
    // indentation carries no meaning).
    if start_in_triple {
        return Ok(LineInfo {
            code: String::new(),
            code_end: line.len(),
            indent: 0,
        });
    }
    if saw_tab && !code.trim().is_empty() {
        return Err(());
    }
    Ok(LineInfo {
        code,
        code_end,
        indent,
    })
}

/// Offset of the first closing triple-quote run for `q` at or after `from`,
/// honoring backslash escapes. `None` = not closed on this line.
fn find_triple_close(line: &str, from: usize, q: char) -> Option<usize> {
    let b = line.as_bytes();
    let (q0, q1, q2) = (q as u8, q as u8, q as u8);
    let mut i = from;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q0 && i + 2 < b.len() && b[i + 1] == q1 && b[i + 2] == q2 {
            // Return the offset just past the closing run.
            return Some(i + 3);
        }
        i += 1;
    }
    None
}

fn utf8_len(first_byte: u8) -> usize {
    match first_byte {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(s: &str) -> String {
        indent_preprocess(s)
            .expect("no indent error")
            .expect("changed")
    }

    #[test]
    fn passthrough_brace_style() {
        let src = "fn main() -> i64 {\n    return 0\n}\n";
        assert_eq!(indent_preprocess(src), Ok(None));
    }

    #[test]
    fn colon_without_header_keyword_is_untouched() {
        // Wrapped type ascription in a brace-style file must not transform.
        let src = "fn main() {\n    let x:\n        i64 = 5\n}";
        assert_eq!(indent_preprocess(src), Ok(None));
    }

    #[test]
    fn basic_fn_if() {
        let out = tx("fn main() -> i64:\n    let x = 5\n    if x > 3:\n        println_i64(x)\n    return x\n");
        assert_eq!(
            out,
            "fn main() -> i64 {\n    let x = 5\n    if x > 3 {\n        println_i64(x)\n}\n    return x\n\n}"
        );
    }

    #[test]
    fn header_colon_stripped_with_trailing_comment() {
        let out = tx("fn f():  // comment: with colon\n    return 1\n");
        assert!(out.starts_with("fn f() {// comment: with colon\n"));
    }

    #[test]
    fn column0_comment_does_not_close_block() {
        let out = tx("fn f():\n    let x = 1\n// col0 comment\n    return x\n");
        assert!(!out.contains("}\n// col0"));
        assert!(out.ends_with("    return x\n\n}"));
    }

    #[test]
    fn string_with_slashes_and_colons() {
        let out = tx("fn f():\n    let u = \"http://x:1\"\n    return 0\n");
        assert!(!out.contains("\"http://x:1\" {")); // string never opens a block
        assert_eq!(out.matches('{').count(), out.matches('}').count());
    }

    #[test]
    fn triple_quoted_string_is_skipped() {
        let src = "fn f():\n    let s = \"\"\"\nWHERE: x\n\"\"\"\n    return 0\n";
        let out = tx(src);
        assert_eq!(out.matches('{').count(), out.matches('}').count());
        assert!(out.contains("WHERE: x\n\"\"\""));
    }

    #[test]
    fn tab_indent_is_error() {
        assert_eq!(
            indent_preprocess("fn f():\n\tlet x = 1\n"),
            Err(IndentError::TabIndent { line: 2 })
        );
    }

    #[test]
    fn tab_before_comment_only_is_ok() {
        assert!(indent_preprocess("fn f():\n    return 1\n\t// note\n").is_ok());
    }

    #[test]
    fn mixed_styles() {
        let out = tx("fn a() -> i64 {\n    if true {\n        return 1\n    }\n    return 0\n}\n\nfn b() -> i64:\n    if true {\n        return 2\n    }\n    return 0\n");
        assert_eq!(out.matches('{').count(), out.matches('}').count());
        assert!(out.contains("fn b() -> i64 {"));
    }

    #[test]
    fn eof_tail_on_fresh_line() {
        let out = tx("fn f():\n    return 1\n// trailing comment");
        assert!(out.ends_with("\n}"));
    }

    #[test]
    fn let_initializer_block_expr() {
        let out = tx("fn f(c: bool) -> i64:\n    let r = loop:\n        if c:\n            break 7\n        break 8\n    return r\n");
        assert!(out.contains("let r = loop {"));
        assert_eq!(out.matches('{').count(), out.matches('}').count());
    }

    #[test]
    fn wrapped_type_ascription_still_protected() {
        // `let x:` has no top-level `=` before the colon -> not a header.
        let src = "fn f() {\n    let x:\n        i64 = 5\n}";
        assert_eq!(indent_preprocess(src), Ok(None));
    }

    #[test]
    fn elif_else_chain() {
        let out = tx("fn g(x: i64) -> i64:\n    if x > 0:\n        return 1\n    elif x < 0:\n        return 2\n    else:\n        return 3\n");
        assert!(out.contains("\n    elif x < 0 {"));
        assert!(out.contains("\n    else {"));
    }
}
