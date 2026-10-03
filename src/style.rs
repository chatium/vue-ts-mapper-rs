//! `plugins/vue-style-css.ts`: `@import`s, `v-bind()` bindings and class names of a style block,
//! with the exact matching of its regular expressions. Works on UTF-16 units, so offsets match.

#[derive(Clone, Debug, PartialEq)]
pub struct StyleItem {
    pub text: String,
    pub offset: usize,
}

pub struct StyleInfo {
    pub imports: Vec<StyleItem>,
    pub bindings: Vec<StyleItem>,
    pub class_names: Vec<StyleItem>,
}

pub fn parse(css: &str) -> StyleInfo {
    let units: Vec<u16> = css.encode_utf16().collect();
    let imports = parse_imports(&units);
    let no_comments = blank_comments(&units);
    let bindings = parse_bindings(&no_comments);
    let no_fragments = blank_fragments(&no_comments);
    let class_names = parse_class_names(&no_fragments);
    StyleInfo { imports, bindings, class_names }
}

const fn u(c: char) -> u16 {
    c as u16
}

fn is_line_break(c: u16) -> bool {
    matches!(c, 0x0a | 0x0d | 0x2028 | 0x2029)
}

/// JS regex `\s`
fn is_js_space(c: u16) -> bool {
    matches!(
        c,
        0x09..=0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff
    )
}

fn is_word(c: u16) -> bool {
    c < 0x80 && ((c as u8).is_ascii_alphanumeric() || c == u('_'))
}

fn ends_with_at(units: &[u16], pos: usize, s: &str) -> bool {
    let s: Vec<u16> = s.encode_utf16().collect();
    pos >= s.len() && units[pos - s.len()..pos] == s[..]
}

fn find_from(units: &[u16], from: usize, needle: &[u16]) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    (from..units.len().saturating_sub(needle.len() - 1)).find(|&i| units[i..i + needle.len()] == *needle)
}

fn text_of(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// `trimQuotes(text, offset, trim)`
fn trim_quotes(text: &[u16], offset: usize, trim: bool) -> StyleItem {
    let mut start = 0usize;
    let mut end = text.len();
    let has_quote = text.iter().any(|&c| c == u('"') || c == u('\''));
    if trim || has_quote {
        // `!text[i]?.trim()`: whitespace (or, past the end, `undefined`)
        while start < text.len() && is_trim_space(text[start]) {
            start += 1;
        }
        while end > 0 && is_trim_space(text[end - 1]) {
            end -= 1;
        }
    }
    if end >= 1
        && start < text.len()
        && ((text[start] == u('"') && text[end - 1] == u('"')) || (text[start] == u('\'') && text[end - 1] == u('\'')))
    {
        start += 1;
        end -= 1;
    }
    let slice = if start < end { &text[start..end] } else { &[][..] };
    StyleItem { text: text_of(slice), offset: offset + start }
}

/// `String.prototype.trim` whitespace: `\s` plus the line terminators (same set for one unit).
fn is_trim_space(c: u16) -> bool {
    is_js_space(c)
}

/// `/(?<=@import\s+url\()(["']?).*?\1(?=\))|(?<=@import\b\s*)(["']).*?\2/g`
fn parse_imports(units: &[u16]) -> Vec<StyleItem> {
    let mut out = Vec::new();
    let mut p = 0;
    while p <= units.len() {
        if let Some(end) = import_match_at(units, p) {
            out.push(trim_quotes(&units[p..end], p, true));
            p = if end > p { end } else { p + 1 };
        } else {
            p += 1;
        }
    }
    out
}

fn import_match_at(units: &[u16], p: usize) -> Option<usize> {
    // alternative 1: after `@import\s+url(`
    if p >= 1 && units[p - 1] == u('(') && ends_with_at(units, p, "url(") {
        let mut q = p - 4;
        let mut spaces = 0;
        while q > 0 && is_js_space(units[q - 1]) {
            q -= 1;
            spaces += 1;
        }
        if spaces > 0 && ends_with_at(units, q, "@import") {
            let quote = units.get(p).copied().filter(|&c| c == u('"') || c == u('\''));
            // `(["']?)` tries the quote first and backtracks to an empty group
            for quote in [quote, None] {
                let body_start = p + quote.map_or(0, |_| 1);
                // `.*?\1(?=\))`: the shortest run (no line breaks) followed by the quote and `)`
                let mut k = body_start;
                loop {
                    match quote {
                        Some(q) => {
                            if units.get(k) == Some(&q) && units.get(k + 1) == Some(&u(')')) {
                                return Some(k + 1);
                            }
                        }
                        None => {
                            if units.get(k) == Some(&u(')')) {
                                return Some(k);
                            }
                        }
                    }
                    if k >= units.len() || is_line_break(units[k]) {
                        break;
                    }
                    k += 1;
                }
            }
        }
    }
    // alternative 2: after `@import\b\s*`
    if let Some(&q) = units.get(p) {
        if q == u('"') || q == u('\'') {
            let mut s = p;
            while s > 0 && is_js_space(units[s - 1]) {
                s -= 1;
            }
            if ends_with_at(units, s, "@import") {
                // `\b` right after `@import`
                let boundary = units.get(s).is_none_or(|&c| !is_word(c));
                if boundary {
                    let mut k = p + 1;
                    while k < units.len() && !is_line_break(units[k]) {
                        if units[k] == q {
                            return Some(k + 1);
                        }
                        k += 1;
                    }
                }
            }
        }
    }
    None
}

/// `fillBlank(css, commentRE)` with `/(?<=\/\*)[\s\S]*?(?=\*\/)|(?<=\/\/)[\s\S]*?(?=\n)/g`
fn blank_comments(units: &[u16]) -> Vec<u16> {
    let mut out = units.to_vec();
    let star_slash: Vec<u16> = "*/".encode_utf16().collect();
    let mut p = 0;
    while p <= units.len() {
        let mut m = None;
        if ends_with_at(units, p, "/*") {
            m = find_from(units, p, &star_slash);
        }
        if m.is_none() && ends_with_at(units, p, "//") {
            m = (p..units.len()).find(|&i| units[i] == u('\n'));
        }
        match m {
            Some(end) => {
                for x in &mut out[p..end] {
                    *x = u(' ');
                }
                p = if end > p { end } else { p + 1 };
            }
            None => p += 1,
        }
    }
    out
}

/// `fillBlank(css, fragmentRE)` with `/(?<={)[^{]*(?=(?<!\\);)/g`
fn blank_fragments(units: &[u16]) -> Vec<u16> {
    let mut out = units.to_vec();
    let mut p = 0;
    while p <= units.len() {
        let mut m = None;
        if p >= 1 && units[p - 1] == u('{') {
            let run_end = (p..units.len()).find(|&i| units[i] == u('{')).unwrap_or(units.len());
            // greedy `[^{]*` backtracks to the last `;` not preceded by a backslash
            m = (p..run_end).rev().find(|&k| units[k] == u(';') && !(k >= 1 && units[k - 1] == u('\\')));
            // the run itself ends at `run_end`; a `;` exactly there is not part of `[^{]*`
        }
        match m {
            Some(end) => {
                for x in &mut out[p..end] {
                    *x = u(' ');
                }
                p = if end > p { end } else { p + 1 };
            }
            None => p += 1,
        }
    }
    out
}

/// `/\bv-bind\s*\(/g` followed by `lexBinding`
fn parse_bindings(units: &[u16]) -> Vec<StyleItem> {
    let mut out = Vec::new();
    let vbind: Vec<u16> = "v-bind".encode_utf16().collect();
    let mut p = 0;
    while let Some(i) = find_from(units, p, &vbind) {
        let boundary = i == 0 || !is_word(units[i - 1]);
        let mut k = i + vbind.len();
        while k < units.len() && is_js_space(units[k]) {
            k += 1;
        }
        if boundary && units.get(k) == Some(&u('(')) {
            let start = k + 1;
            if let Some(end) = lex_binding(units, start) {
                out.push(trim_quotes(&units[start..end], start, false));
            }
            p = start;
        } else {
            p = i + 1;
        }
    }
    out
}

fn lex_binding(units: &[u16], start: usize) -> Option<usize> {
    #[derive(PartialEq)]
    enum S {
        Parens,
        Single,
        Double,
    }
    let mut state = S::Parens;
    let mut depth = 0;
    for (i, &c) in units.iter().enumerate().skip(start) {
        match state {
            S::Parens => {
                if c == u('\'') {
                    state = S::Single;
                } else if c == u('"') {
                    state = S::Double;
                } else if c == u('(') {
                    depth += 1;
                } else if c == u(')') {
                    if depth > 0 {
                        depth -= 1;
                    } else {
                        return Some(i);
                    }
                }
            }
            S::Single => {
                if c == u('\'') {
                    state = S::Parens;
                }
            }
            S::Double => {
                if c == u('"') {
                    state = S::Parens;
                }
            }
        }
    }
    None
}

/// `/\.[a-z_][-\w]*(?=[\s.,+~>:#)[{])/gi`
fn parse_class_names(units: &[u16]) -> Vec<StyleItem> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < units.len() {
        if units[p] == u('.') {
            if let Some(&first) = units.get(p + 1) {
                if first < 0x80 && ((first as u8).is_ascii_alphabetic() || first == u('_')) {
                    let mut k = p + 2;
                    while k < units.len() && (is_word(units[k]) || units[k] == u('-')) {
                        k += 1;
                    }
                    if let Some(&next) = units.get(k) {
                        let ok = is_js_space(next) || ".,+~>:#)[{".encode_utf16().any(|c| c == next);
                        if ok {
                            out.push(StyleItem { text: text_of(&units[p..k]), offset: p });
                            p = k;
                            continue;
                        }
                    }
                }
            }
        }
        p += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style() {
        let css = "@import 'a.css';\n@import url(\"b.css\");\n.foo-bar, .baz:hover { color: v-bind(color); /* .x */ }\n.q{a:b;c:.5em;}";
        let s = parse(css);
        assert_eq!(s.imports.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["a.css", "b.css"]);
        assert_eq!(s.bindings[0].text, "color");
        let names: Vec<_> = s.class_names.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(names, [".foo-bar", ".baz", ".q"]);
    }
}
