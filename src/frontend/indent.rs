// src/frontend/indent.rs
//! Indentation-based block preprocessing (PY-1).
//!
//! Converts Python-style indented blocks into brace blocks before the nom
//! parser runs. Rules (V1):
//!   - A line whose trailing (non-comment) token is `:` starts a block if the
//!     next non-blank line is indented deeper; we emit `{` at end of line.
//!   - Dedent closes blocks: we emit `}` before the first line at shallower
//!     indentation.
//!   - Lines that already contain braces (e.g. `fn f() { ... }`, `else {`)
//!     are left untouched — mixed styles work.
//!   - Blank lines and comment-only lines are preserved but ignored for
//!     indentation bookkeeping.
//!   - Tabs at line start are rejected (only spaces define indentation).
//!
//! V1 limitation: single-line blocks (`if x: stmt`) are not supported — a
//! block header must be followed by a newline + deeper indent. Multiline
//! string literals are not yet protected (PY-5).

/// Preprocess `input`, returning a brace-normalized string ready for the
/// existing parser. If no indentation block is detected, returns the input
/// unchanged (cheap fast path for brace-style sources).
pub fn indent_preprocess(input: &str) -> String {
    // Fast path: if no line ends with ':' (outside obvious comments) there is
    // nothing to do. We still scan to be safe.
    if !looks_like_python_style(input) {
        return input.to_string();
    }

    let lines: Vec<&str> = input.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut indent_stack: Vec<usize> = Vec::new();

    // Pre-scan: indent level of each line's first non-blank content, or None.
    let mut levels: Vec<Option<(usize, bool)>> = Vec::with_capacity(lines.len());
    for line in &lines {
        let trimmed_start = line.trim_start();
        if trimmed_start.is_empty() {
            levels.push(None);
            continue;
        }
        // Tabs at line start are rejected.
        if let Some(c) = trimmed_start.chars().next() {
            let leading = &line[..line.len() - trimmed_start.len()];
            if leading.contains('\t') {
                levels.push(Some((leading.len(), true))); // tab error marker
                continue;
            }
        }
        let indent = line.len() - trimmed_start.len();
        levels.push(Some((indent, false)));
    }

    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];

        // Preserve blank lines exactly.
        if line.trim().is_empty() {
            out.push(line.to_string());
            i += 1;
            continue;
        }

        let (indent, is_tab) = match levels[i] {
            Some((n, t)) => (n, t),
            None => {
                out.push(line.to_string());
                i += 1;
                continue;
            }
        };
        if is_tab {
            // Attach a parse-error marker the parser will reject.
            // Simplest: leave line but emit a clear comment is not enough —
            // we let the parser fail naturally and surface the message below.
            out.push(format!("TAB_INDENT_ERROR {line}"));
            i += 1;
            continue;
        }

        // Dedent: pop stack while current indent < top.
        let mut prefix = String::new();
        while let Some(&top) = indent_stack.last() {
            if indent < top {
                indent_stack.pop();
                prefix.push('}');
            } else {
                break;
            }
        }
        // Same indent as stack top: no change (closes nothing, opens nothing).

        // Comment-only lines: never open/close blocks.
        let content = strip_line_comment(line.trim_start());
        if content.trim().is_empty() {
            out.push(format!("{prefix}{}", line));
            i += 1;
            continue;
        }

        // Does this line end with ':' (block header)?
        let ends_with_colon = content.trim_end().ends_with(':');

        // Peek next non-blank line indent.
        let mut j = i + 1;
        let mut next_indent = None;
        while j < lines.len() {
            if let Some((ni, tab)) = levels[j] {
                if !tab {
                    next_indent = Some(ni);
                    break;
                }
            }
            j += 1;
        }

        let opens_block = ends_with_colon
            && next_indent.map(|ni| ni > indent).unwrap_or(false)
            // Skip lines that already use brace syntax on the same line.
            && !line.contains('{');

        out.push(format!("{prefix}{line}{}", if opens_block { " {" } else { "" }));
        if opens_block {
            // Push the BODY indent (next line's indent), not the header line's.
            // A block closes when a line's indent < body indent.
            indent_stack.push(next_indent.unwrap());
        }
        i += 1;
    }

    // Close any remaining open blocks at EOF.
    let mut tail = String::new();
    while indent_stack.pop().is_some() {
        tail.push('}');
    }
    if !tail.is_empty() {
        if let Some(last) = out.last_mut() {
            last.push_str(&tail);
        }
    }

    out.join("\n")
}

/// Quick heuristic: does any non-comment line end with ':'?
pub fn looks_like_python_style(input: &str) -> bool {
    for line in input.split('\n') {
        let content = strip_line_comment(line.trim_start());
        let t = content.trim_end();
        if t.ends_with(':') {
            return true;
        }
    }
    false
}

/// Strip a trailing `// ...` comment, ignoring colon detection inside it.
/// (V1: does not protect `//` inside string literals — PY-5 will address.)
fn strip_line_comment(line: &str) -> String {
    match line.find("//") {
        Some(pos) => line[..pos].to_string(),
        None => line.to_string(),
    }
}
