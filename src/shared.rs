//! The `@vue/shared` helpers the codegen uses, with exact JS semantics (regex `\w`, `\B`, UTF-16).

/// `str.replace(/-(\w)/g, (_, c) => c.toUpperCase())`
pub fn camelize(s: &str) -> String {
    if !s.contains('-') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '-' {
            if let Some(&n) = chars.peek() {
                if n.is_ascii_alphanumeric() || n == '_' {
                    chars.next();
                    out.push(n.to_ascii_uppercase());
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// `str.charAt(0).toUpperCase() + str.slice(1)`
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => {
            let mut out: String = c.to_uppercase().collect();
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `str.replace(/\B([A-Z])/g, '-$1').toLowerCase()`
pub fn hyphenate(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev: Option<char> = None;
    for c in s.chars() {
        if c.is_ascii_uppercase() && prev.is_some_and(is_word) {
            out.push('-');
        }
        out.push(c);
        prev = Some(c);
    }
    out.to_lowercase()
}

/// language-core's `hyphenateAttr`
pub fn hyphenate_attr(s: &str) -> String {
    let mut h = hyphenate(s);
    if let Some(c) = s.chars().next() {
        if c.to_lowercase().next() != Some(c) || c.to_lowercase().count() != 1 {
            h.insert(0, '-');
        }
    }
    h
}

const GLOBALS_ALLOWED: &[&str] = &[
    "Infinity", "undefined", "NaN", "isFinite", "isNaN", "parseFloat", "parseInt", "decodeURI",
    "decodeURIComponent", "encodeURI", "encodeURIComponent", "Math", "Number", "Date", "Array",
    "Object", "Boolean", "String", "RegExp", "Map", "Set", "JSON", "Intl", "BigInt", "console",
    "Error", "Symbol",
];

/// `isGloballyAllowed`
pub fn is_globally_allowed(name: &str) -> bool {
    GLOBALS_ALLOWED.contains(&name)
}

/// `isBuiltInDirective`
pub fn is_built_in_directive(name: &str) -> bool {
    matches!(
        name,
        "bind" | "cloak" | "else-if" | "else" | "for" | "html" | "if" | "model" | "on" | "once"
            | "pre" | "show" | "slot" | "text" | "memo"
    )
}

/// `/^[a-zA-Z_$][0-9a-zA-Z_$]*$/`
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// picomatch's `isMatch` for the attribute-name patterns the options take (`aria-*`, `data-*`):
/// `*` (no `/`), `?`, `**`, and literal characters.
pub fn glob_match(name: &str, pattern: &str) -> bool {
    fn go(n: &[char], p: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => {
                let globstar = p.get(1) == Some(&'*');
                let rest = if globstar { &p[2..] } else { &p[1..] };
                for i in 0..=n.len() {
                    if go(&n[i..], rest) {
                        return true;
                    }
                    if i < n.len() && n[i] == '/' && !globstar {
                        return false;
                    }
                }
                false
            }
            Some('?') => !n.is_empty() && n[0] != '/' && go(&n[1..], &p[1..]),
            Some(c) => !n.is_empty() && n[0] == *c && go(&n[1..], &p[1..]),
        }
    }
    let n: Vec<char> = name.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    // picomatch never matches dotfiles with a leading wildcard
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    go(&n, &p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_semantics() {
        assert_eq!(camelize("foo-bar"), "fooBar");
        assert_eq!(camelize("a--b"), "a-B");
        assert_eq!(camelize("a-"), "a-");
        assert_eq!(camelize("on-update:model-value"), "onUpdate:modelValue");
        assert_eq!(camelize("a-й"), "a-й");
        assert_eq!(hyphenate("FooBar"), "foo-bar");
        assert_eq!(hyphenate("fooBAR"), "foo-b-a-r");
        assert_eq!(hyphenate("-Bar"), "-bar");
        assert_eq!(hyphenate_attr("FooBar"), "-foo-bar");
        assert_eq!(capitalize("foo"), "Foo");
        assert!(is_identifier("$foo_1"));
        assert!(!is_identifier("1foo"));
        assert!(glob_match("aria-label", "aria-*"));
        assert!(!glob_match("data-x", "aria-*"));
    }
}
