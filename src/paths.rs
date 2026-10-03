//! The `path-browserify` (posix) functions the codegen uses.

/// `path.normalize` for posix paths.
pub fn normalize(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let absolute = p.starts_with('/');
    let trailing = p.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let mut out = parts.join("/");
    if out.is_empty() && !absolute {
        out = ".".into();
    }
    if trailing && !out.is_empty() && out != "." {
        out.push('/');
    }
    if absolute { format!("/{out}") } else { out }
}

pub fn join(a: &str, b: &str) -> String {
    if a.is_empty() {
        return normalize(b);
    }
    if b.is_empty() {
        return normalize(a);
    }
    normalize(&format!("{a}/{b}"))
}

pub fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
}

/// `path.dirname`
pub fn dirname(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".into();
    }
    match trimmed.rfind('/') {
        None => ".".into(),
        Some(0) => "/".into(),
        Some(i) => trimmed[..i].trim_end_matches('/').to_string().max_or_root(),
    }
}

trait RootIfEmpty {
    fn max_or_root(self) -> String;
}

impl RootIfEmpty for String {
    fn max_or_root(self) -> String {
        if self.is_empty() { "/".into() } else { self }
    }
}

/// `path.basename`
pub fn basename(p: &str) -> &str {
    let trimmed = p.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

/// `path.relative(from, to)` for absolute paths.
pub fn relative(from: &str, to: &str) -> String {
    let from = normalize(from);
    let to = normalize(to);
    if from == to {
        return String::new();
    }
    let f: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let t: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = Vec::new();
    for _ in common..f.len() {
        parts.push("..");
    }
    parts.extend(&t[common..]);
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix() {
        assert_eq!(relative("/a/b/c", "/a/x/types"), "../../x/types");
        assert_eq!(relative("/a/b", "/a/b/types"), "types");
        assert_eq!(dirname("/a/b/c.vue"), "/a/b");
        assert_eq!(dirname("/c.vue"), "/");
        assert_eq!(basename("/a/b/c.vue"), "c.vue");
        assert_eq!(join("/a/b", "../c"), "/a/c");
    }
}
