//! Small text helpers whose exact behaviour the app depends on: Python's line splitting and `\w`, case
//! folding, and ways to name values in error messages.

pub fn is_word_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

pub fn has_word_char(text: &str) -> bool {
    text.chars().any(is_word_char)
}

pub fn casefold(text: &str) -> String {
    caseless::default_case_fold_str(text)
}

/// Lines split at every line boundary a user can paste, not only `\n`.
pub fn split_lines(text: &str) -> Vec<&str> {
    split_lines_keepends(text)
        .into_iter()
        .map(|line| line.trim_end_matches(is_line_break))
        .collect()
}

/// Each line with its terminator. `\r\n` is one terminator.
pub fn split_lines_keepends(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if is_line_break(c) {
            let mut end = index + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
                chars.next();
                end += 1;
            }
            lines.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// A float the way Python's `%g` prints a timeout: 8 for 8.0, 2.5 for 2.5.
pub fn short_float(value: f64) -> String {
    let text = format!("{value:.6}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}
