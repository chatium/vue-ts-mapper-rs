//! UTF-16 offsets over UTF-8 text.
//!
//! The reference implementation is JavaScript, so every offset it produces (and every offset the
//! content-mapper protocol speaks) counts UTF-16 code units. This port keeps the same unit
//! everywhere and converts only when it slices a Rust string.

/// A string together with a UTF-16 → byte offset table (absent for ASCII text, where both agree).
pub struct Text<'a> {
    pub s: &'a str,
    /// `byte_of[i]` is the byte offset of UTF-16 unit `i`; the low half of a surrogate pair maps to
    /// the start of its character. Has `len16 + 1` entries.
    byte_of: Option<Vec<u32>>,
}

impl<'a> Text<'a> {
    pub fn new(s: &'a str) -> Self {
        if s.is_ascii() {
            return Text { s, byte_of: None };
        }
        let mut byte_of = Vec::with_capacity(s.len() + 1);
        for (b, c) in s.char_indices() {
            byte_of.push(b as u32);
            if c.len_utf16() == 2 {
                byte_of.push(b as u32);
            }
        }
        byte_of.push(s.len() as u32);
        Text { s, byte_of: Some(byte_of) }
    }

    pub fn len(&self) -> usize {
        match &self.byte_of {
            None => self.s.len(),
            Some(t) => t.len() - 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.s.is_empty()
    }

    /// Byte offset of UTF-16 offset `o` (clamped to the end).
    pub fn byte(&self, o: usize) -> usize {
        match &self.byte_of {
            None => o.min(self.s.len()),
            Some(t) => t[o.min(t.len() - 1)] as usize,
        }
    }

    /// UTF-16 offset of byte offset `b` (which must sit on a char boundary).
    pub fn utf16(&self, b: usize) -> usize {
        match &self.byte_of {
            None => b,
            // the first unit at or past `b`; the table is sorted
            Some(t) => t.partition_point(|&x| (x as usize) < b),
        }
    }

    /// `str.slice(start, end)` with JS semantics for in-range offsets.
    pub fn slice(&self, start: usize, end: usize) -> &'a str {
        let (a, b) = (self.byte(start), self.byte(end));
        if a >= b { "" } else { &self.s[a..b] }
    }

    pub fn slice_from(&self, start: usize) -> &'a str {
        &self.s[self.byte(start)..]
    }

    /// `str.charCodeAt(o)`; `None` past the end.
    pub fn code_at(&self, o: usize) -> Option<u16> {
        if o >= self.len() {
            return None;
        }
        match &self.byte_of {
            None => Some(self.s.as_bytes()[o] as u16),
            Some(t) => {
                let b = t[o] as usize;
                let c = self.s[b..].chars().next()?;
                let mut buf = [0u16; 2];
                let units = c.encode_utf16(&mut buf);
                // the low half of a surrogate pair shares its char's byte offset
                let first = o == 0 || t[o - 1] as usize != b;
                Some(if first { units[0] } else { units[units.len() - 1] })
            }
        }
    }

    /// `str.indexOf(needle, from)` in UTF-16 units.
    pub fn index_of(&self, needle: &str, from: usize) -> Option<usize> {
        let from_b = self.byte(from);
        self.s[from_b..].find(needle).map(|i| self.utf16(from_b + i))
    }

    /// `str.lastIndexOf(needle)` in UTF-16 units.
    pub fn last_index_of(&self, needle: &str) -> Option<usize> {
        self.s.rfind(needle).map(|i| self.utf16(i))
    }
}

/// `str.length` of a Rust string.
pub fn len16(s: &str) -> usize {
    // every char is one unit except the 4-byte ones, which are two: count the bytes that start a
    // char, plus the bytes that start a 4-byte char
    s.as_bytes().iter().map(|&b| ((b & 0xc0) != 0x80) as usize + (b >= 0xf0) as usize).sum()
}

/// JS `String.prototype.trim()` whitespace (`\s` plus the line terminators).
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

pub fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_space)
}

pub fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets() {
        let t = Text::new("aй😀b");
        assert_eq!(t.len(), 5);
        assert_eq!(t.slice(1, 2), "й");
        assert_eq!(t.slice(2, 4), "😀");
        assert_eq!(t.slice(4, 5), "b");
        assert_eq!(t.code_at(2), Some(0xd83d));
        assert_eq!(t.code_at(3), Some(0xde00));
        assert_eq!(t.utf16(t.s.len()), 5);
        assert_eq!(t.index_of("b", 0), Some(4));
        assert_eq!(len16("😀"), 2);
    }

    #[test]
    fn offsets() {
        let s = "aй😀b";
        assert_eq!(len16(s), 5);
        let t = Text::new(s);
        assert_eq!(t.len(), 5);
        assert_eq!(t.utf16(0), 0);
        assert_eq!(t.utf16(1), 1);
        assert_eq!(t.utf16(3), 2);
        assert_eq!(t.utf16(7), 4);
        assert_eq!(t.index_of("b", 0), Some(4));
        assert_eq!(t.slice(1, 2), "й");
    }
}
