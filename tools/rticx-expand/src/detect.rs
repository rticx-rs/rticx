//! Locates the `#[<distro>::app(...)] pub mod name { ... }` item inside a
//! user source file so the tool can splice the expansion in its place.

/// Start (byte index of `#[`) and exclusive end (byte index after the
/// closing `}`) of the `#[...::app(...)] mod ... { ... }` item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppItem {
    pub attr_start: usize,
    pub end: usize,
}

/// Finds every `#[<path>::app(...)]` attribute in `src` together with the end
/// of the module it decorates.
pub fn find_app_items(src: &str) -> Vec<AppItem> {
    let bytes = src.as_bytes();
    let mut items = Vec::new();
    let mut search = 0;
    while let Some(rel) = find_from(bytes, search) {
        let attr_start = rel;
        let Some(item) = locate_app_item(bytes, attr_start) else {
            search = attr_start + 1;
            continue;
        };
        items.push(item);
        search = item.end;
    }
    items
}

/// Finds the next `#[<crate>::app(` attribute start (index of `#`), scanning
/// from `search` (byte index).
fn find_from(bytes: &[u8], search: usize) -> Option<usize> {
    let mut i = search;
    while i + 5 <= bytes.len() {
        // Fast path: look for `::app(`.
        if bytes[i] == b':' && &bytes[i..i + 5] == b"::app" {
            // Check the following char is `(` (modulo whitespace).
            let mut j = i + 5;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if bytes.get(j) == Some(&b'(') {
                // Walk backwards over the attribute path to its `#[`.
                if let Some(hash) = attr_start_of(bytes, i) {
                    return Some(hash);
                }
            }
        }
        i += 1;
    }
    None
}

/// Walks backwards from the `::app` position over `[A-Za-z0-9_:]` to find the
/// `#[` opening the attribute.
fn attr_start_of(bytes: &[u8], app_pos: usize) -> Option<usize> {
    let mut j = app_pos - 1;
    while j > 0 && (bytes[j] == b'_' || bytes[j] == b':' || bytes[j].is_ascii_alphanumeric()) {
        j -= 1;
    }
    // Skip whitespace between the `#[` and the path (rustfmt may insert some).
    while j > 0 && bytes[j].is_ascii_whitespace() {
        j -= 1;
    }
    if j > 0 && bytes[j] == b'[' && bytes[j - 1] == b'#' {
        Some(j - 1)
    } else {
        None
    }
}

/// Given the start of `#[`, finds the end of the decorated module (exclusive
/// byte index after its closing brace).
fn locate_app_item(bytes: &[u8], attr_start: usize) -> Option<AppItem> {
    let attr_end = find_matching_bracket(bytes, attr_start + 1)?;
    let mut i = attr_end + 1;

    // Skip whitespace/comments and any further attributes (`#[cfg(...)]`),
    // then an optional visibility, then `mod name {`.
    loop {
        skip_trivia(bytes, &mut i);
        if bytes.get(i) == Some(&b'#') && bytes.get(i + 1) == Some(&b'[') {
            i = find_matching_bracket(bytes, i + 1)? + 1;
            continue;
        }
        break;
    }
    // Optional visibility keywords: `pub`, `pub(crate)`, ...
    skip_visibility(bytes, &mut i);
    skip_trivia(bytes, &mut i);
    if !consume_word(bytes, &mut i, b"mod") {
        return None;
    }
    skip_trivia(bytes, &mut i);
    // module name
    while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
        i += 1;
    }
    skip_trivia(bytes, &mut i);
    if bytes.get(i) != Some(&b'{') {
        return None;
    }
    let open = i;
    let close = find_matching_brace(bytes, open)?;
    Some(AppItem {
        attr_start,
        end: close + 1,
    })
}

/// Skips whitespace and comments.
fn skip_trivia(bytes: &[u8], i: &mut usize) {
    loop {
        while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
            *i += 1;
        }
        if bytes.get(*i) == Some(&b'/') && bytes.get(*i + 1) == Some(&b'/') {
            while *i < bytes.len() && bytes[*i] != b'\n' {
                *i += 1;
            }
            continue;
        }
        if bytes.get(*i) == Some(&b'/') && bytes.get(*i + 1) == Some(&b'*') {
            *i += 2;
            while *i < bytes.len() && !(bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/')) {
                *i += 1;
            }
            *i = (*i + 2).min(bytes.len());
            continue;
        }
        break;
    }
}

/// Skips an optional visibility specifier before `mod`.
fn skip_visibility(bytes: &[u8], i: &mut usize) {
    if consume_word(bytes, i, b"pub") {
        skip_trivia(bytes, i);
        if bytes.get(*i) == Some(&b'(')
            && let Some(end) = find_matching_delim(bytes, *i, b'(', b')')
        {
            *i = end + 1;
        }
    }
}

/// Consumes the ASCII word `word` at `i` (followed by a non-identifier byte).
fn consume_word(bytes: &[u8], i: &mut usize, word: &[u8]) -> bool {
    if bytes.get(*i..*i + word.len()) != Some(word) {
        return false;
    }
    let after = bytes.get(*i + word.len());
    if after.is_some_and(|b| *b == b'_' || b.is_ascii_alphanumeric()) {
        return false;
    }
    *i += word.len();
    true
}

/// Index of the `}` matching the `{` at `open`. Skips strings, chars,
/// lifetimes, raw strings, and comments.
fn find_matching_brace(bytes: &[u8], open: usize) -> Option<usize> {
    debug_assert_eq!(bytes[open], b'{');
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' | b'\'' | b'r' | b'b' | b'c' if is_literal_start(bytes, i) => {
                i += literal_len(bytes, i) - 1
            }
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

/// Length of the literal starting at `i`, or 1 when no literal starts there.
fn literal_len(bytes: &[u8], i: usize) -> usize {
    let b = bytes[i];
    if b == b'"' {
        return quoted_end(bytes, i).map_or(1, |end| end - i + 1);
    }
    if b == b'\'' {
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

/// Index of the `]` matching the `[` at `open`.
fn find_matching_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    find_matching_delim(bytes, open, b'[', b']')
}

/// Index of the delimiter closing the one at `open`, or `None`. Skips
/// strings, chars, lifetimes, raw strings, and comments.
fn find_matching_delim(bytes: &[u8], open: usize, open_ch: u8, close_ch: u8) -> Option<usize> {
    debug_assert_eq!(bytes[open], open_ch);
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b if b == open_ch => depth += 1,
            b if b == close_ch => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' | b'\'' | b'r' | b'b' | b'c' if is_literal_start(bytes, i) => {
                i += literal_len(bytes, i) - 1
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') || bytes.get(i + 1) == Some(&b'*') => {
                i += literal_len(bytes, i) - 1
            }
            _ => {}
        }
        i += 1;
    }
    None
}

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
    fn finds_simple_app_item() {
        let src = "#![no_std]\n\nuse panic_halt as _;\n\n#[rticx_cortex_m::app(device = mypac, dispatchers = [TIM6])]\npub mod my_app {\n    #[init]\n    fn init() -> (Shared, TaskInits) {\n        let s = \"} brace in string {\";\n        (Shared {}, TaskInits {})\n    }\n}\n";
        let items = find_app_items(src);
        assert_eq!(items.len(), 1);
        assert!(src[..items[0].attr_start].ends_with("use panic_halt as _;\n\n"));
        assert!(src[items[0].end..].is_empty() || src[items[0].end..].ends_with("\n"));
    }

    #[test]
    fn skips_braces_in_strings_comments_and_raw_strings() {
        let src = "#[rticx::app()]\nmod app {\n    // } comment brace\n    /* } block { comment */\n    let raw = r#\"}\"#;\n    let ch = '}';\n    let s = \"}\n{\";\n    let _ = [1, 2, 3];\n}\n// trailing comment\n";
        let items = find_app_items(src);
        assert_eq!(items.len(), 1);
        assert!(src[items[0].end..].starts_with("\n// trailing comment"));
    }

    #[test]
    fn handles_cfg_attribute_before_mod() {
        let src = "#[rticx::app()]\n#[cfg(feature = \"x\")]\npub(crate) mod app { struct S; }\n";
        let items = find_app_items(src);
        assert_eq!(items.len(), 1);
        assert_eq!(&src[items[0].end..], "\n");
    }

    #[test]
    fn rejects_non_app_attributes() {
        let src = "#[derive(Debug)]\nstruct S;\n";
        assert!(find_app_items(src).is_empty());
    }

    #[test]
    fn finds_multiple_app_items() {
        let src = "#[rticx::app()] mod a {}\n#[rticx::app()] mod b {}\n";
        assert_eq!(find_app_items(src).len(), 2);
    }
}
