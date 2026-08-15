//! Best-effort beautification of generated RTICX expansions.
//!
//! The raw expansion written by `rticx-core` is a single (mostly single-line)
//! token rendering. This module applies two text-level transformations before
//! `rustfmt` turns the result into readable code:
//!
//! 1. **Identifier shortening** — generated identifiers are prefixed with
//!    `__rticx...` and internal statics use long `__rticx_internal__` names.
//!    They are shortened to `_...` while preserving uniqueness:
//!    - `__rticx_internal__Worker__INPUTS` → `_Worker__INPUTS`
//!    - `__rticx_internal_MASKS_core0` → `_MASKS_core0`
//!    - `__rticx__internal__Core0` → `_Core0`
//!    - `__rticx_interrupt_free` → `_interrupt_free`
//!    - `__shared_resources` → `_shared_resources`
//! 2. **Doc-attribute conversion** — `#[doc = "..."]` attributes generated as
//!    pseudo-comments become real `//` comments; `#[doc(hidden)]` markers are
//!    dropped.
//!
//! The transformation is deliberately token-aware (strings, chars, lifetimes,
//! raw strings and comments are skipped) but best-effort: it never fails, and
//! the output remains valid Rust even when a construct is not recognized.

/// Applies identifier shortening and doc-attribute conversion to `input`.
pub fn beautify(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'_' || b.is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            out.push_str(&rename_ident(&input[start..i]));
            continue;
        }
        if b == b'#'
            && bytes.get(i + 1) == Some(&b'[')
            && let Some(end) = find_matching_bracket(bytes, i + 1)
        {
            let content = &input[i + 2..end];
            match doc_attr_to_comment(content) {
                Some(comment) => {
                    // The expansion is (mostly) single-line, so comments must
                    // be surrounded by newlines or they would swallow the code
                    // around the original attribute.
                    if !comment.is_empty() {
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push('\n');
                        }
                        out.push_str(&comment);
                        out.push('\n');
                    }
                }
                None => out.push_str(&input[i..=end]),
            }
            i = end + 1;
            continue;
        }
        let len = literal_len(bytes, i);
        out.push_str(&input[i..i + len]);
        i += len;
    }
    out
}

/// Shortens a generated identifier. Returns the original when no rule applies.
fn rename_ident(ident: &str) -> String {
    if let Some(rest) = ident
        .strip_prefix("__rticx_internal__")
        .or_else(|| ident.strip_prefix("__rticx_internal_"))
        .or_else(|| ident.strip_prefix("__rticx__internal__"))
    {
        return short_rest(rest);
    }
    if let Some(rest) = ident.strip_prefix("__rticx") {
        // `rest` starts with `_` (e.g. `_interrupt_free`); use it directly.
        return if rest.is_empty() {
            ident.to_string()
        } else {
            rest.to_string()
        };
    }
    if let Some(rest) = ident.strip_prefix("__") {
        return short_rest(rest);
    }
    ident.to_string()
}

/// Prepends `_` to the remainder unless it already carries a leading `_`.
fn short_rest(rest: &str) -> String {
    if rest.is_empty() || rest.starts_with('_') {
        rest.to_string()
    } else {
        format!("_{rest}")
    }
}

/// Converts a `doc` attribute body to a comment. Returns `None` when the
/// attribute is not a `doc` attribute (the caller should keep it verbatim).
/// Returns `Some(String::new())` for `doc(hidden)` (dropped entirely).
fn doc_attr_to_comment(content: &str) -> Option<String> {
    let content = content.trim();
    let doc = content.strip_prefix("doc")?;
    if let Some(lit) = doc.trim().strip_prefix('=') {
        let (text, is_raw) = string_literal_content(lit.trim())?;
        let text = if is_raw { text } else { unescape(&text) };
        let mut comment = String::new();
        for (idx, line) in text.lines().enumerate() {
            if idx == 0 {
                comment.push_str("// ");
            } else {
                comment.push_str("\n// ");
            }
            comment.push_str(line);
        }
        return Some(comment);
    }
    if doc.trim() == "(hidden)" {
        return Some(String::new());
    }
    None
}

/// Extracts the content of a string literal (normal or raw, `r"..."`,
/// `r#"..."#`), or `None` if `lit` is not a string literal. The returned flag
/// is true for raw strings (whose content must not be unescaped).
fn string_literal_content(lit: &str) -> Option<(String, bool)> {
    let bytes = lit.as_bytes();
    let (content_start, content_end, is_raw) = if bytes.first() == Some(&b'"') {
        let end = quoted_end(bytes, 0)?;
        (1, end, false)
    } else if bytes.first() == Some(&b'r') {
        let mut j = 1;
        let mut hashes = 0;
        while bytes.get(j) == Some(&b'#') {
            hashes += 1;
            j += 1;
        }
        if bytes.get(j) != Some(&b'"') {
            return None;
        }
        let end = raw_end(bytes, j + 1, hashes)?;
        (j + 1, end, true)
    } else {
        return None;
    };
    Some((lit[content_start..content_end].to_string(), is_raw))
}

/// Reverses the escapes `proc_macro2` uses when rendering string literals.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => {}
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('{') => out.push('{'),
            Some('}') => out.push('}'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Index of the `]` matching the `[` at `open`, or `None`. Skips strings,
/// chars, lifetimes, and comments.
fn find_matching_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    debug_assert_eq!(bytes[open], b'[');
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' | b'\'' => i += literal_len(bytes, i) - 1,
            b'r' | b'b' | b'c' if is_literal_start(bytes, i) => i += literal_len(bytes, i) - 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') || bytes.get(i + 1) == Some(&b'*') => {
                i += literal_len(bytes, i) - 1
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// True when a string/char/comment literal starts at `i`.
fn is_literal_start(bytes: &[u8], i: usize) -> bool {
    let b = bytes[i];
    match b {
        b'"' | b'\'' => true,
        b'r' => {
            let mut j = i + 1;
            while bytes.get(j) == Some(&b'#') {
                j += 1;
            }
            bytes.get(j) == Some(&b'"')
        }
        b'b' => {
            bytes.get(i + 1) == Some(&b'"')
                || (bytes.get(i + 1) == Some(&b'r')
                    && bytes.get(i + 2).is_some_and(|c| *c == b'#' || *c == b'"'))
        }
        b'c' => {
            bytes.get(i + 1) == Some(&b'"')
                || (bytes.get(i + 1) == Some(&b'r')
                    && bytes.get(i + 2).is_some_and(|c| *c == b'#' || *c == b'"'))
        }
        b'/' => bytes.get(i + 1) == Some(&b'/') || bytes.get(i + 1) == Some(&b'*'),
        _ => false,
    }
}

/// Length of the literal (string/char/comment) starting at `i`, or 1 when no
/// literal starts at `i`.
fn literal_len(bytes: &[u8], i: usize) -> usize {
    let b = bytes[i];
    if b == b'"' {
        return quoted_end(bytes, i).map_or(1, |end| end - i + 1);
    }
    if b == b'\'' {
        // Char literal with escape: '\n' ; plain char: 'a' ; lifetime: 'static
        if bytes.get(i + 1) == Some(&b'\\') {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j] != b'\'' {
                j += 1;
            }
            return if j < bytes.len() { j - i + 1 } else { 1 };
        }
        if bytes.get(i + 1).is_some_and(|c| *c != b'\'') && bytes.get(i + 2) == Some(&b'\'') {
            return 3;
        }
        return 1;
    }
    if b == b'r' {
        let mut j = i + 1;
        let mut hashes = 0;
        while bytes.get(j) == Some(&b'#') {
            hashes += 1;
            j += 1;
        }
        if bytes.get(j) == Some(&b'"') {
            return raw_end(bytes, j + 1, hashes).map_or(1, |end| end - i + 1);
        }
        return 1;
    }
    if (b == b'b' || b == b'c') && bytes.get(i + 1) == Some(&b'"') {
        return quoted_end(bytes, i + 1).map_or(1, |end| end - i + 1);
    }
    if (b == b'b' || b == b'c') && bytes.get(i + 1) == Some(&b'r') {
        let mut j = i + 2;
        let mut hashes = 0;
        while bytes.get(j) == Some(&b'#') {
            hashes += 1;
            j += 1;
        }
        if bytes.get(j) == Some(&b'"') {
            return raw_end(bytes, j + 1, hashes).map_or(1, |end| end - i + 1);
        }
        return 1;
    }
    if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
        let mut j = i;
        while j < bytes.len() && bytes[j] != b'\n' {
            j += 1;
        }
        return j - i;
    }
    if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
        let mut j = i + 2;
        while j < bytes.len() && !(bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/')) {
            j += 1;
        }
        return (j + 2).min(bytes.len()) - i;
    }
    1
}

/// Index of the closing `"` of the string opened at `open` (escapes respected).
fn quoted_end(bytes: &[u8], open: usize) -> Option<usize> {
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Index of the closing `"` of the raw string whose content starts at `open`,
/// closed by `hashes` number of `#`s after the quote.
fn raw_end(bytes: &[u8], open: usize, hashes: usize) -> Option<usize> {
    let mut i = open;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let mut ok = true;
            for k in 0..hashes {
                if bytes.get(i + 1 + k) != Some(&b'#') {
                    ok = false;
                    break;
                }
            }
            if ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_generated_identifiers() {
        assert_eq!(
            rename_ident("__rticx_internal__Worker__INPUTS"),
            "_Worker__INPUTS"
        );
        assert_eq!(rename_ident("__rticx_internal_MASKS_core0"), "_MASKS_core0");
        assert_eq!(rename_ident("__rticx__internal__Core0"), "_Core0");
        assert_eq!(rename_ident("__rticx__internal__Core0Inner"), "_Core0Inner");
        assert_eq!(rename_ident("__rticx_interrupt_free"), "_interrupt_free");
        assert_eq!(rename_ident("__rticx_trait_checks"), "_trait_checks");
        assert_eq!(
            rename_ident("__rticx_sw_system_initialized"),
            "_sw_system_initialized"
        );
        assert_eq!(rename_ident("__rticx_async_Pong"), "_async_Pong");
        assert_eq!(rename_ident("__rticx_local_irq_pend"), "_local_irq_pend");
        assert_eq!(rename_ident("__shared_resources"), "_shared_resources");
        assert_eq!(rename_ident("__task_inits"), "_task_inits");
        assert_eq!(rename_ident("__counter_mutex"), "_counter_mutex");
        // user code untouched
        assert_eq!(rename_ident("Worker"), "Worker");
        assert_eq!(rename_ident("_task_inits"), "_task_inits");
    }

    #[test]
    fn converts_doc_attributes_to_comments() {
        assert_eq!(beautify(r#"#[doc = " # CORE 0"]"#), "//  # CORE 0\n");
        assert_eq!(
            beautify(r#"#[doc = r" Include peripheral crate(s)"]"#),
            "//  Include peripheral crate(s)\n"
        );
        // escaped quotes render as readable text
        assert_eq!(
            beautify(r#"#[doc = "RTIC\'s SRP locking"]"#),
            "// RTIC's SRP locking\n"
        );
        // doc(hidden) is dropped
        assert_eq!(beautify("#[doc(hidden)]"), "");
        // non-doc attributes are preserved
        assert_eq!(
            beautify("#[allow(non_upper_case_globals)]"),
            "#[allow(non_upper_case_globals)]"
        );
    }

    #[test]
    fn does_not_touch_string_contents() {
        let input =
            r#"let s = "__rticx_internal__Worker__INPUTS"; __rticx_internal__Worker__INPUTS;"#;
        assert_eq!(
            beautify(input),
            r#"let s = "__rticx_internal__Worker__INPUTS"; _Worker__INPUTS;"#
        );
    }

    #[test]
    fn handles_raw_strings_chars_and_lifetimes() {
        let input = r##"let r = r#"__rticx_raw {brace}"#; let c = '#'; let l = 'static; __x;"##;
        assert_eq!(
            beautify(input),
            r##"let r = r#"__rticx_raw {brace}"#; let c = '#'; let l = 'static; _x;"##
        );
    }

    #[test]
    fn handles_multiline_doc_after_rustfmt() {
        // rustfmt may split long doc attributes across lines; the scanner is
        // whitespace-insensitive.
        assert_eq!(beautify("#[doc =\n\"hello world\"]"), "// hello world\n");
    }
}
