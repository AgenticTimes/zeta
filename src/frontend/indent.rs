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

use std::cell::RefCell;
use std::fmt;

// C1: indent-preprocess line maps (preprocessed line → original 1-based line),
// kept per source file so diagnostics can turn a byte offset / remaining suffix
// back into an original line. Also keeps the preprocessed text so
// `ensure_fully_parsed` can locate a remaining suffix.
//
// 批次 466（#65 LAST_PP 串号）：行表按**栈**保存——同一进程里先编主文件再编
// 被导入模块时，旧实现只剩最后一份表，主文件的诊断查到模块的行表 ⇒ 行号串号
// （语料实测：128 行的文件报 ：729）。查询按"remaining 是哪份 pp 的后缀"匹配，
// 与解析顺序无关；上限 64 份（REPL 每行一压也不无界）。
thread_local! {
    static LAST_PP: RefCell<Vec<(String, Vec<usize>)>> = const { RefCell::new(Vec::new()) };
}

const LAST_PP_CAP: usize = 64;

/// Record preprocess output for C1 line lookup (called from `parse_zeta`).
pub fn set_last_preprocess(text: String, origins: Vec<usize>) {
    LAST_PP.with(|c| {
        let mut stack = c.borrow_mut();
        // 同一份文本重复解析（REPL 逐行重编）只保留一份
        if stack.last().is_some_and(|(t, _)| *t == text) {
            return;
        }
        if stack.len() >= LAST_PP_CAP {
            stack.remove(0);
        }
        stack.push((text, origins));
    });
}

pub fn clear_last_preprocess() {
    LAST_PP.with(|c| c.borrow_mut().clear());
}

/// 在栈里找"remaining 是其 pp 文本后缀"的那份行表，返回该后缀起点对应的原始行。
/// 找不到（表全部不匹配）时退回 `fallback_source` 的后缀；再不行返回 None——
/// **宁可没有行号，不报别的文件的行号**（#65 的教训）。
fn line_for_suffix_impl(
    remaining: &str,
    fallback_source: &str,
    trim_leading: bool,
) -> Option<usize> {
    LAST_PP.with(|c| {
        for (pp, origins) in c.borrow().iter().rev() {
            if pp.ends_with(remaining) {
                let off = pp.len() - remaining.len();
                let trim = if trim_leading {
                    remaining.len() - remaining.trim_start().len()
                } else {
                    0
                };
                let byte_off = off + trim;
                let pp_line = pp[..byte_off.min(pp.len())].bytes().filter(|&b| b == b'\n').count();
                return Some(origins.get(pp_line).copied().unwrap_or(pp_line + 1));
            }
        }
        if fallback_source.ends_with(remaining) {
            let off = fallback_source.len() - remaining.len();
            let trim = if trim_leading {
                remaining.len() - remaining.trim_start().len()
            } else {
                0
            };
            let byte_off = off + trim;
            return Some(
                fallback_source[..byte_off.min(fallback_source.len())]
                    .bytes()
                    .filter(|&b| b == b'\n')
                    .count()
                    + 1,
            );
        }
        None
    })
}

/// `ensure_fully_parsed` 用：跳过 remaining 的前导空白后取行（C1 原两步查询的合并）。
pub fn line_for_remaining(remaining: &str, fallback_source: &str) -> Option<usize> {
    line_for_suffix_impl(remaining, fallback_source, true)
}

/// 吞词/同步点诊断用：remaining 起点即语句位置，不跳空白。
pub fn line_for_stmt_start(remaining: &str, fallback_source: &str) -> Option<usize> {
    line_for_suffix_impl(remaining, fallback_source, false)
}

/// Estimate per-preprocessed-line origins by walking the original source.
fn estimate_line_origins(original: &str, processed: &str) -> Vec<usize> {
    let orig_lines: Vec<&str> = original.lines().collect();
    let mut origins: Vec<usize> = Vec::new();
    let mut search_from = 0usize;
    for pline in processed.lines() {
        let key: String = pline
            .trim()
            .trim_end_matches('{')
            .trim()
            .trim_end_matches(':')
            .trim()
            .to_string();
        if key.is_empty() || key.chars().all(|c| c == '}') {
            origins.push(origins.last().copied().unwrap_or(1));
            continue;
        }
        let mut found = None;
        for (j, ol) in orig_lines.iter().enumerate().skip(search_from) {
            let ot = ol.trim();
            if ot.is_empty() {
                continue;
            }
            if ot.contains(&key) || key.contains(ot.trim_end_matches(':').trim()) {
                found = Some(j + 1);
                search_from = j;
                break;
            }
        }
        origins.push(found.unwrap_or_else(|| origins.last().copied().unwrap_or(1)));
    }
    if origins.is_empty() {
        origins.push(1);
    }
    origins
}

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
    "class", "try", "except", "finally", "with",
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
    // PY-A: normalize leading tabs to 4 spaces per line first — real Python
    // codebases mix tab/space indentation (JoinQuant strategies are tab-
    // indented). After this pass, all indentation is pure spaces.
    let normalized: String = {
        let mut out = String::with_capacity(input.len());
        for line in input.split('\n') {
            let mut expanded = String::with_capacity(line.len() + 16);
            let mut chars = line.chars().peekable();
            let mut col = 0usize;
            let mut in_indent = true;
            while let Some(c) = chars.peek().copied() {
                if in_indent && c == '\t' {
                    let next_stop = (col / 4 + 1) * 4;
                    for _ in col..next_stop {
                        expanded.push(' ');
                    }
                    col = next_stop;
                    chars.next();
                } else {
                    if !c.is_whitespace() {
                        in_indent = false;
                    }
                    expanded.push(c);
                    col += 1;
                    chars.next();
                }
            }
            out.push_str(&expanded);
            out.push('\n');
        }
        // trailing newline added above — strip the final one to match input
        out.pop();
        out
    };
    let input: &str = if normalized == input {
        input
    } else {
        // leak is fine: parse_zeta already leaks preprocessed output
        &Box::leak(normalized.into_boxed_str())
    };
    let lines_raw: Vec<&str> = input.split('\n').collect();
    // PY-A: fold Python's backslash line continuations BEFORE anything else
    // looks at the text. `x = 1 + \` + newline + `2` is ONE logical line, and
    // the continuation's indentation carries no meaning. Leaving the `\` in
    // place made every such statement — and therefore the whole enclosing
    // definition — fail to parse.
    let folded = fold_backslash_continuations(&lines_raw);
    let after_bs: Vec<String> = match &folded {
        Some(joined) => joined.to_vec(),
        None => lines_raw.iter().map(|s| s.to_string()).collect(),
    };
    // PY-A: one-line compound bodies (`if not xs: return []`) — same rationale
    // as above, and it must also happen before any indentation bookkeeping.
    let after_bs_refs: Vec<&str> = after_bs.iter().map(|s| s.as_str()).collect();
    let inline = fold_inline_bodies(&after_bs_refs);
    let folded_lines: Vec<String> = match &inline {
        Some(v) => v.clone(),
        None => after_bs.clone(),
    };
    let joined_text = folded_lines.join("\n");
    let lines: Vec<&str> = folded_lines.iter().map(|s| s.as_str()).collect();
    let (out, changed) = normalize_blocks(&lines)?;
    if !changed {
        // PY-A (任务 #55/#51): `changed` means "this file needed an
        // indentation-to-brace rewrite", NOT "this file is python" — a flat
        // script needs no rewrite, and so does a brace-style source. Deriving
        // the dialect from it therefore turns floor division into a comment for
        // every flat python file (`print(7 // 2)` lost its argument).
        // Without indentation evidence, decide per occurrence: `//` is the
        // operator only while a `(`/`[` is still open at that column. Measured
        // over the official corpus: 209 `//`-after-code occurrences, none of
        // them inside an open bracket — `return 1  // Success` and friends all
        // sit at depth 0, so they keep their comment.
        let rewritten = rewrite_floordiv_lines(&lines, true);
        if rewritten != lines {
            let text = rewritten.join("\n");
            let origins = estimate_line_origins(input, &text);
            set_last_preprocess(text.clone(), origins);
            return Ok(Some(text));
        }
        // Still hand back the folded text when we folded something — that is
        // the only edit we made.
        return if folded.is_some() || inline.is_some() {
            let origins = estimate_line_origins(input, &joined_text);
            set_last_preprocess(joined_text.clone(), origins);
            Ok(Some(joined_text))
        } else {
            // Identity: map each source line to itself for C1.
            let origins: Vec<usize> = (1..=input.lines().count().max(1)).collect();
            set_last_preprocess(input.to_string(), origins);
            Ok(None)
        };
    }
    // PY-A: in a python-style file `//` is floor division, not a comment
    // (Python's comment marker is `#`). The parser's comment skipper cannot
    // tell the dialects apart, so rewrite the operator into `floordiv`, a word
    // operator the parser does know — and then normalize AGAIN: cutting a line
    // at `//` leaves its brackets unbalanced (`int(diff_value` never closes),
    // which poisons the bracket-depth bookkeeping for the rest of the block, so
    // later headers never got their `{` at all and the definition failed with
    // or without the operator.
    let rewritten = rewrite_floordiv_lines(&lines, false);
    let lines_rw: Vec<&str> = rewritten.iter().map(|s| s.as_str()).collect();
    let (out, _) = normalize_blocks(&lines_rw)?;
    let text = out.join("\n");
    let origins = estimate_line_origins(input, &text);
    set_last_preprocess(text.clone(), origins);
    Ok(Some(text))
}

/// PY-A: fold Python backslash continuations (`x = 1 + \` NEWLINE `2`) into one
/// logical line, dropping the continuation lines' leading whitespace. Returns
/// `None` when there was nothing to fold, so callers can keep the identity
/// passthrough for brace-style sources.
fn fold_backslash_continuations(lines: &[&str]) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut in_triple: Option<char> = None;
    let mut folded_any = false;
    let mut i = 0usize;
    while i < lines.len() {
        let mut code = scan_line(lines[i], &mut in_triple)
            .map(|info| info.code)
            .unwrap_or_default();
        let mut joined = lines[i].to_string();
        // A trailing backslash in CODE position (not inside a string, not a
        // comment) continues the logical line. Keep the state machine in step
        // by scanning each physical line exactly once, in order.
        while code.trim_end().ends_with('\\') && i + 1 < lines.len() {
            if let Some(cut) = joined.rfind('\\') {
                joined.truncate(cut);
            }
            i += 1;
            let next = lines[i];
            joined.push_str(next.trim_start());
            code = scan_line(next, &mut in_triple)
                .map(|info| info.code)
                .unwrap_or_default();
            folded_any = true;
        }
        out.push(joined);
        i += 1;
    }
    if folded_any {
        Some(out)
    } else {
        None
    }
}

/// PY-A: Python allows a compound statement's body on the SAME line —
/// `if not xs: return []`, `def f(x): return x`. The indent pass only rewrote
/// headers whose body sat indented on following lines (`opens` requires that),
/// so these fell through with the colon still in place and the whole enclosing
/// definition was dropped (84 such lines across 16 corpus files). Rewrite them
/// to the brace form the parser expects: `head { body }`.
///
/// Detection is deliberately narrow: the line must **start** with a block
/// keyword (so dict literals / lambdas / slices are never candidates) and the
/// colon must sit at bracket depth 0 in CODE position (not in a string, not a
/// `::` or `:=`). One level of nesting is handled by recursing on the body; a
/// doubly-nested one-liner still fails loudly.
fn fold_inline_bodies(lines: &[&str]) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut in_triple: Option<char> = None;
    let mut folded_any = false;
    for line in lines {
        // Keep the string state in step: lines inside a triple-quoted string
        // are string content, never code.
        let inside_string = {
            let info = scan_line(line, &mut in_triple).ok()?;
            info.code.trim().is_empty()
        };
        if inside_string {
            out.push(line.to_string());
            continue;
        }
        match rewrite_inline_body(line) {
            Some(new_line) => {
                folded_any = true;
                out.push(new_line);
            }
            None => out.push(line.to_string()),
        }
    }
    if folded_any {
        Some(out)
    } else {
        None
    }
}

/// One-line body for a single header line, or `None` when the line is not that
/// shape. Everything is located on the RAW line, so the body keeps its original
/// bytes (strings and f-strings included).
fn rewrite_inline_body(line: &str) -> Option<String> {
    if !is_header_start(line) {
        return None;
    }
    let (colon, code_end) = find_inline_colon(line)?;
    // Defensive: never slice backwards, whatever the scanner found.
    if colon >= code_end {
        return None;
    }
    let raw_body = &line[colon + 1..code_end];
    if raw_body.trim().is_empty() {
        return None; // ordinary header — the normal path handles it
    }
    let body = raw_body.trim();
    let inner = rewrite_inline_body(body).unwrap_or_else(|| body.to_string());
    // Keep the whitespace that followed the body so a trailing comment does not
    // get glued to the closing brace (`}# note` reads badly in ZETA_DUMP_PP).
    let gap = &raw_body[raw_body.trim_end().len()..];
    let mut out = String::with_capacity(line.len() + 6);
    out.push_str(line[..colon].trim_end());
    out.push_str(" { ");
    out.push_str(&inner);
    out.push_str(" }");
    out.push_str(gap);
    out.push_str(&line[code_end..]);
    Some(out)
}

/// Byte offsets of the first top-level code `:` (not `::`, not `:=`) and of the
/// end of the code part (comment start or end of line) on a RAW line.
fn find_inline_colon(line: &str) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    let mut i = 0usize;
    let mut depth = 0i32;
    while i < b.len() {
        let c = b[i];
        match c {
            b'#' if !line[i..].starts_with("#[") => return find_colon_before(line, i),
            // `//` is a comment too — without this a `// primes: 2,3,5,7`
            // trailing comment donated its colon and the slice below went
            // backwards (a compiler panic on valid input).
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => return find_colon_before(line, i),
            b'\'' | b'"' => {
                let q = c;
                if line[i..].starts_with(&(q as char).to_string().repeat(3)) {
                    // triple-quoted: treat the rest of the line as string
                    return find_colon_before(line, i);
                }
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                i += 1;
            }
            b':' if depth == 0 => {
                let next = b.get(i + 1).copied();
                let prev = i.checked_sub(1).and_then(|p| b.get(p)).copied();
                if next != Some(b':') && next != Some(b'=') && prev != Some(b':') {
                    // 冒号之后的代码里如果已经有本语句自己的 `{`（Rust 风格行尾
                    // 开块），这个冒号就是类型注解而不是 Python 单行块的起始：
                    // `for i: usize in 0..10 {` 不能被改写成
                    // `for i { usize in 0..10 { }`。实测（primezeta_usize_test）
                    // 丢 36 行的根因。
                    let code = &line[i + 1..find_code_end(line)];
                    if code.contains('{') {
                        return None;
                    }
                    return Some((i, find_code_end(line)));
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// With a `:` found before `stop`, the code part ends at the comment start (or
/// EOL). Comment markers inside strings must not count.
fn find_colon_before(line: &str, stop: usize) -> Option<(usize, usize)> {
    find_inline_colon(&line[..stop]).map(|(c, _)| (c, stop))
}

/// Index where the code part ends: the first `#`/`//` outside a string, else EOL.
fn find_code_end(line: &str) -> usize {
    let b = line.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'#' if !line[i..].starts_with("#[") => return i,
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => return i,
            b'\'' | b'"' => {
                let q = b[i];
                if line[i..].starts_with(&(q as char).to_string().repeat(3)) {
                    return i;
                }
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    line.len()
}

/// Indentation → braces for one dialect-agnostic pass. Returns the rewritten
/// lines and whether anything was actually recognized as python-style.
fn normalize_blocks(lines: &[&str]) -> Result<(Vec<String>, bool), IndentError> {
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
    // Bracket depth + the FIRST line of the current logical (possibly
    // multi-line) statement. A signature split over several lines —
    //     def inject_local_data(
    //         market_df: pd.DataFrame,
    //     ) -> None:
    // — only ends with `:` on its LAST physical line, where the block keyword
    // is long gone. Testing that line alone never recognized the header, the
    // colon was left in place, and the definition failed to parse (which then
    // truncated the rest of the file). Track the logical statement's opening
    // line so the same detection works regardless of how it is wrapped.
    let mut depth: i32 = 0;
    let mut logical_head: Option<(String, usize)> = None;

    for i in 0..lines.len() {
        let info = &infos[i];
        if info.code.trim().is_empty() {
            // Blank / comment-only / inside-triple line: verbatim, no
            // bookkeeping (never opens or closes a block).
            out.push(lines[i].to_string());
            continue;
        }

        // A line inside an open bracket is a *continuation*: Python gives its
        // indentation no meaning, so it must not close blocks. An unindented
        // continuation line used to trick the dedent pass into emitting `}` in
        // the middle of the expression — `x = [\n1, 2]` closed the enclosing
        // `def` right after the `[` line, and everything below it was dropped.
        let continuation = depth > 0;

        // Dedent: emit one `}` per popped level, on its own line before the
        // dedented line (comments/blank lines between are skipped by the
        // parser, so order relative to them is irrelevant).
        while !continuation && stack.last().map_or(false, |&top| top > info.indent) {
            stack.pop();
            out.push("}".to_string());
            changed = true;
        }

        let depth_after = depth + bracket_delta(&info.code);
        let (head_code, head_indent) = match &logical_head {
            Some((h, hi)) if depth > 0 => (h.as_str(), *hi),
            _ => (info.code.as_str(), info.indent),
        };
        let next_indent = next_code_indent(&infos, i + 1);
        let opens = depth_after == 0
            && info.code.trim_end().ends_with(':')
            && !info.code.contains('{')
            && is_header(head_code)
            && next_indent.map_or(false, |ni| ni > head_indent);

        if depth_after > 0 && depth == 0 {
            logical_head = Some((info.code.clone(), info.indent));
        }
        depth = if depth_after < 0 { 0 } else { depth_after };
        if depth == 0 {
            logical_head = None;
        }

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

    Ok((out, changed))
}

/// Run `rewrite_floordiv_line` over a whole file with the string and bracket
/// state carried from line to line.
fn rewrite_floordiv_lines(lines: &[&str], only_in_brackets: bool) -> Vec<String> {
    let mut in_triple: Option<char> = None;
    let mut depth: i32 = 0;
    lines
        .iter()
        .map(|line| {
            let out = rewrite_floordiv_line(line, &mut in_triple, &mut depth, only_in_brackets);
            if depth < 0 {
                depth = 0;
            }
            out
        })
        .collect()
}

/// Rewrite the floor-division operator on one line, leaving strings, `#`
/// comments and comment-only `//` lines untouched. A `//` preceded by code on
/// the line is the operator; a `//` that *starts* the line's code stays a
/// comment (that is how this repo's own python_style `// expect:` headers are
/// written).
///
/// `only_in_brackets` is the fallback for files whose dialect is NOT proven by
/// indentation: there `//` after code is the operator only while `(`/`[` are
/// unclosed at that column, i.e. the parser is mid-expression. `{` is
/// deliberately not part of the depth — a brace-style `if x {  // note` must
/// keep its comment. `depth` is carried across lines (a continuation line of a
/// multi-line call is still inside it) and clamped at 0, so a stray `)` cannot
/// make later lines look "closed".
fn rewrite_floordiv_line(
    line: &str,
    in_triple: &mut Option<char>,
    depth: &mut i32,
    only_in_brackets: bool,
) -> String {
    let b = line.as_bytes();
    let mut out = String::with_capacity(line.len() + 16);
    let mut i = 0usize;
    // Has any code token been seen before the `//`? Strings count: a literal
    // followed by `/` is still an operand.
    let mut seen_code = false;
    while i < b.len() {
        if let Some(q) = *in_triple {
            match find_triple_close(line, i, q) {
                Some(p) => {
                    out.push_str(&line[i..p]);
                    i = p;
                    *in_triple = None;
                }
                None => {
                    out.push_str(&line[i..]);
                    return out;
                }
            }
            continue;
        }
        let c = b[i];
        match c {
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                if !seen_code {
                    // Comment-only line: keep the comment verbatim.
                    out.push_str(&line[i..]);
                    return out;
                }
                if only_in_brackets && *depth <= 0 {
                    // Dialect unproven and the line is not inside an open
                    // bracket: keep the comment, which also stops us from
                    // scanning the comment's own text for brackets and quotes.
                    out.push_str(&line[i..]);
                    return out;
                }
                // Spaces on both sides: `a//b` must not glue into
                // `afloordivb`.
                out.push_str(" floordiv ");
                i += 2;
            }
            b'#' if !line[i..].starts_with("#[") => {
                out.push_str(&line[i..]);
                return out;
            }
            b'\'' | b'"' => {
                let q = c as char;
                seen_code = true;
                let start = i;
                if line[i..].starts_with(&q.to_string().repeat(3)) {
                    // Keep the real quote characters: unlike `scan_line` this
                    // output IS the program text, and `'''` must not become
                    // `"""` (the closing run would no longer match).
                    out.push_str(&line[i..i + 3]);
                    i += 3;
                    *in_triple = Some(q);
                } else {
                    i += 1;
                    while i < b.len() {
                        if b[i] == b'\\' {
                            i += 2;
                            continue;
                        }
                        if b[i] as char == q {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    out.push_str(&line[start..i.min(b.len())]);
                }
            }
            _ => {
                if !c.is_ascii_whitespace() {
                    seen_code = true;
                }
                // Only `(` and `[`: `{` would make a brace-style
                // `if x {  // note` look like the inside of an expression.
                if c == b'(' || c == b'[' {
                    *depth += 1;
                } else if c == b')' || c == b']' {
                    *depth -= 1;
                }
                let len = utf8_len(c);
                out.push_str(&line[i..(i + len).min(b.len())]);
                i += len;
            }
        }
    }
    out
}

/// Indent of the first code line at or after `from`, skipping blank,
/// comment-only and string-continuation lines.
fn next_code_indent(infos: &[LineInfo], from: usize) -> Option<usize> {
    infos[from..]
        .iter()
        .find(|info| !info.code.trim().is_empty())
        .map(|info| info.indent)
}


/// Net bracket depth of an already-string-blanked code line, used to fold a
/// multi-line statement into one "logical line" for header detection.
fn bracket_delta(code: &str) -> i32 {
    let mut d = 0i32;
    for c in code.chars() {
        match c {
            '(' | '[' | '{' => d += 1,
            ')' | ']' | '}' => d -= 1,
            _ => {}
        }
    }
    d
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
            // PY-A: Python-style `#` comment (but `#[` is attribute syntax)
            b'#' if !line[i..].starts_with("#[") => {
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
    fn flat_floordiv_inside_a_call_is_the_operator() {
        // Task #51: a flat python file needs no indentation rewrite, so the
        // dialect cannot be read off `changed` — the open bracket has to be
        // enough evidence on its own.
        let out = indent_preprocess("print(7 // 2)\n")
            .expect("no indent error")
            .expect("rewritten");
        assert!(out.contains("floordiv"), "{out}");
        assert!(!out.contains("//"), "{out}");
    }

    #[test]
    fn flat_floordiv_depth_carries_across_a_multiline_call() {
        let out = indent_preprocess("print(\n    x // 10)\n")
            .expect("no indent error")
            .expect("rewritten");
        assert!(out.contains("x  floordiv  10"), "{out}");
    }

    #[test]
    fn flat_brace_style_trailing_comment_is_never_the_operator() {
        // The shapes the official corpus actually contains (209 of them, all at
        // bracket depth 0): none may be rewritten.
        for src in [
            "fn main() -> i64 {\n    return 1;  // Success\n}\n",
            "if x {  // note\n    y = 1\n}\n",
            "let v = f(a)  // SIMD\n",
        ] {
            assert_eq!(indent_preprocess(src), Ok(None), "{src}");
        }
    }

    #[test]
    fn flat_url_in_string_or_hash_comment_survives() {
        for src in [
            "print(len(\"https://x//y\"))\n",
            "# see https://x//y\nprint(1)\n",
        ] {
            assert_eq!(indent_preprocess(src), Ok(None), "{src}");
        }
    }

    #[test]
    fn flat_floordiv_at_depth_zero_is_still_a_comment() {
        // OPEN (batch 334): with no open bracket a flat file has no evidence
        // left to decide on, and python's `t = a // b` is shaped exactly like
        // brace-style `return 1  // Success`. Pinned so the hole stays visible.
        assert_eq!(indent_preprocess("t = 7 // 2\nprint(t)\n"), Ok(None));
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
        // python 风格文件的行内注释标记是 `#`（PY-A 语义）：`//` 在代码之后
        // 一律是地板除（t138 三处真用法），这是批次 330 前后就定下的方言规则，
        // 本测试曾用 `//` 写尾注释、与该规则相抵而长期红着（refactor.md 批次
        // 311 记的 cargo test 存量 1 failed 就是它）。
        let out = tx("fn f():  # comment: with colon\n    return 1\n");
        assert!(out.starts_with("fn f() {# comment: with colon\n"));
    }

    #[test]
    fn inline_double_slash_after_code_is_floordiv_in_python_style() {
        // 方言钉子：python 风格（有缩进改写证据）的文件里，代码后的 `//` 是
        // 运算符不是注释——改写成 ` floordiv `（与 tests/python_style/t138 的
        // `return a // b` 同一规则）。注意平铺文件走的是另一条规则（批次 334：
        // 深度 0 保留注释，t406 known-fail 记录的那个缺口），别用平铺输入测这条。
        let out = tx("def f(a, b):\n    return a // b\n");
        assert!(out.contains(" floordiv "));
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
    fn tab_indent_is_normalized() {
        // PY-A 归一化（本文件头部）：行首 tab 按 4 列 stop 展开为空格，不再报错。
        // 旧名 `tab_indent_is_error` 与行为相反——归一化先于一切处理，
        // `saw_tab` 错误分支在主管线上不可达（pipeline-rules BR-F02）。
        let result = indent_preprocess("fn f():\n\tlet x = 1\n");
        assert!(result.is_ok());
        let out = result.unwrap();
        assert!(out.is_some());
        assert!(out.unwrap().contains("let x = 1"));
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

#[cfg(test)]
mod last_pp_stack_tests {
    use super::*;

    fn pp(lines: usize, tag: &str) -> (String, Vec<usize>) {
        let text: String = (1..=lines).map(|i| format!("{tag} line {i}\n")).collect();
        let origins: Vec<usize> = (1..=lines).collect();
        (text, origins)
    }

    #[test]
    fn suffix_picks_the_matching_map_even_when_older() {
        // 主文件先入栈，模块后入栈——主文件的后缀仍应查到主文件的行表（#65 串号回归钉）。
        let (a_text, a_orig) = pp(10, "AAA");
        set_last_preprocess(a_text.clone(), a_orig);
        let (b_text, b_orig) = pp(3, "BBB");
        set_last_preprocess(b_text.clone(), b_orig);
        // A 的后缀（从第 8 行起）
        let a_suffix = a_text.split_inclusive('\n').skip(7).collect::<String>();
        let line = line_for_stmt_start(&a_suffix, "").unwrap_or(0);
        assert_eq!(line, 8, "A 的后缀必须查到 A 的行表，而不是栈顶的 B");
        // B 的后缀照常
        let b_suffix = b_text.split_inclusive('\n').skip(1).collect::<String>();
        assert_eq!(line_for_stmt_start(&b_suffix, "").unwrap_or(0), 2);
    }

    #[test]
    fn no_matching_map_reports_none_not_other_file() {
        let (a_text, a_orig) = pp(5, "AAA");
        set_last_preprocess(a_text, a_orig);
        let (b_text, b_orig) = pp(5, "CCC");
        set_last_preprocess(b_text, b_orig);
        // 谁也不是的后缀 → None（诚实缺省），而不是从别的表里夹出一个假行号
        let stray = "ZZZ line 1\nZZZ line 2\n";
        assert_eq!(line_for_stmt_start(stray, ""), None);
        assert_eq!(line_for_remaining(stray, ""), None);
        // fallback_source 匹配时照常给出
        assert_eq!(line_for_stmt_start(stray, stray), Some(1));
        clear_last_preprocess();
    }

    #[test]
    fn trim_leading_variant_skips_blank_prefix() {
        let (a_text, a_orig) = pp(6, "AAA");
        set_last_preprocess(a_text.clone(), a_orig);
        // remaining 带前导空白（必须取到文件尾，suffix 关系才成立）：
        // line_for_remaining 跳过空白落到首个非空白字符所在行；stmt_start 不跳。
        let tail: String = a_text.split_inclusive('\n').skip(4).collect();
        let suffix = "\n".to_string() + &tail;
        assert_eq!(line_for_remaining(&suffix, "").unwrap_or(0), 5);
        assert_eq!(line_for_stmt_start(&suffix, "").unwrap_or(0), 4);
        clear_last_preprocess();
    }
}
