//! SWC parsing with the conventions of the TypeScript AST the reference walks: UTF-16 offsets
//! relative to the parsed text, `getTokenPosOfNode`-style starts, `node.pos` full starts and
//! `getLeadingCommentRanges`.

use swc_core::common::{BytePos, Span, Spanned};
use swc_core::ecma::ast::{EsVersion, Program};
use swc_core::ecma::parser::error::SyntaxError;
use swc_core::ecma::parser::{EsSyntax, Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

/// Byte position of offset 0 (swc reserves `BytePos(0)` for dummy spans).
const BASE: u32 = 1;

pub struct Parsed {
    pub program: Option<Program>,
    /// UTF-16 offset of the fatal syntax error, when parsing failed
    pub error_at: Option<usize>,
    /// UTF-16 ranges of the errors TypeScript would also report while parsing: the fatal one and
    /// recovered missing tokens (`'}' expected`)
    pub errors: Vec<(usize, usize)>,
    utf16_of_byte: Option<Vec<u32>>,
    len16: usize,
}

impl Parsed {
    /// UTF-16 offset of a span boundary.
    pub fn pos(&self, b: BytePos) -> usize {
        let byte = b.0.saturating_sub(BASE) as usize;
        match &self.utf16_of_byte {
            None => byte,
            Some(t) => t[byte.min(t.len() - 1)] as usize,
        }
    }

    pub fn start(&self, s: &impl Spanned) -> usize {
        self.pos(s.span().lo)
    }

    pub fn end(&self, s: &impl Spanned) -> usize {
        self.pos(s.span().hi)
    }

    pub fn range(&self, span: Span) -> (usize, usize) {
        (self.pos(span.lo), self.pos(span.hi))
    }

    pub fn len16(&self) -> usize {
        self.len16
    }
}

/// Parses `text` with TypeScript syntax (JSX when `jsx`), as a module or as a (sloppy-mode)
/// script. The TypeScript parser the reference uses accepts any of JS/TS syntax in any file and
/// always produces a tree; on a fatal swc error there is no tree at all.
pub fn parse(text: &str, jsx: bool) -> Parsed {
    parse_as(text, jsx, true)
}

pub fn parse_as(text: &str, jsx: bool, module: bool) -> Parsed {
    parse_with(text, Syntax::Typescript(TsSyntax { tsx: jsx, decorators: true, ..Default::default() }), module)
}

/// A `<script>` block in `lang`. TypeScript parses `.js` / `.jsx` files without type arguments in
/// expressions (`f<T>(x)` is two comparisons there), which plain JS syntax reproduces; TS-only
/// syntax in such a file still parses, through the TypeScript fallback.
pub fn parse_block(text: &str, lang: &str) -> Parsed {
    match lang {
        "ts" => parse(text, false),
        "tsx" => parse(text, true),
        _ => {
            let es = parse_with(text, Syntax::Es(EsSyntax { jsx: true, decorators: true, ..Default::default() }), true);
            if es.program.is_some() { es } else { parse(text, true) }
        }
    }
}

fn parse_with(text: &str, syntax: Syntax, module: bool) -> Parsed {
    let lexer = Lexer::new(syntax, EsVersion::latest(), StringInput::new(text, BytePos(BASE), BytePos(BASE + text.len() as u32)), None);
    let mut parser = Parser::new_from(lexer);
    let result = if module {
        parser.parse_module().map(Program::Module)
    } else {
        parser.parse_script().map(Program::Script)
    };
    let recovered = parser.take_errors();
    let utf16_of_byte = if text.is_ascii() {
        None
    } else {
        let mut t = vec![0u32; text.len() + 1];
        let mut u = 0u32;
        for (b, c) in text.char_indices() {
            for k in 0..c.len_utf8() {
                t[b + k] = u;
            }
            u += c.len_utf16() as u32;
        }
        t[text.len()] = u;
        Some(t)
    };
    let len16 = crate::text::len16(text);
    let mut parsed = Parsed { program: None, error_at: None, errors: Vec::new(), utf16_of_byte, len16 };
    for e in recovered {
        if matches!(e.kind(), SyntaxError::Expected(..) | SyntaxError::Eof) {
            let range = parsed.range(e.span());
            parsed.errors.push(range);
        }
    }
    match result {
        // recoverable errors still leave a usable tree, like TypeScript's
        Ok(program) => parsed.program = Some(program),
        Err(e) => {
            let range = parsed.range(e.span());
            parsed.error_at = Some(range.0);
            parsed.errors.push(range);
        }
    }
    parsed
}

/// The UTF-16 range of a binding identifier's name. swc widens the identifier of some parameters
/// (`(a?: T) => …`) over the `?` and type annotation; TypeScript's identifier node ends at the name.
pub fn binding_ident_range(p: &Parsed, units: &[u16], b: &swc_core::ecma::ast::BindingIdent) -> (usize, usize) {
    let (start, mut end) = p.range(b.id.span);
    if let Some(t) = &b.type_ann {
        end = end.min(p.pos(t.span.lo));
    }
    if b.type_ann.is_some() || b.id.optional {
        while end > start
            && (units[end - 1] == b'?' as u16 || char::from_u32(units[end - 1] as u32).is_some_and(crate::text::is_js_space))
        {
            end -= 1;
        }
    }
    (start, end)
}

/// TypeScript's `isWhiteSpaceSingleLine`.
fn is_white_space_single_line(c: u16) -> bool {
    matches!(
        c,
        0x20 | 0x09 | 0x0b | 0x0c | 0xa0 | 0x85 | 0x1680 | 0x2000..=0x200b | 0x202f | 0x205f | 0x3000 | 0xfeff
    )
}

fn is_line_break(c: u16) -> bool {
    matches!(c, 0x0a | 0x0d | 0x2028 | 0x2029)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentKind {
    SingleLine,
    MultiLine,
}

#[derive(Clone, Copy, Debug)]
pub struct CommentRange {
    pub pos: usize,
    pub end: usize,
    pub kind: CommentKind,
}

/// `ts.getLeadingCommentRanges(text, pos)` over UTF-16 code units.
pub fn leading_comment_ranges(units: &[u16], mut pos: usize) -> Vec<CommentRange> {
    let mut out = Vec::new();
    let mut collecting = false;
    let mut pending: Option<CommentRange> = None;
    if pos == 0 {
        collecting = true;
        // shebang
        if units.len() >= 2 && units[0] == b'#' as u16 && units[1] == b'!' as u16 {
            while pos < units.len() && !is_line_break(units[pos]) {
                pos += 1;
            }
        }
    }
    while pos < units.len() {
        let ch = units[pos];
        match ch {
            0x0d | 0x0a => {
                if ch == 0x0d && units.get(pos + 1) == Some(&0x0a) {
                    pos += 1;
                }
                pos += 1;
                collecting = true;
                continue;
            }
            0x09 | 0x0b | 0x0c | 0x20 => {
                pos += 1;
                continue;
            }
            0x2f => {
                let next = units.get(pos + 1).copied();
                if next == Some(0x2f) || next == Some(0x2a) {
                    let kind = if next == Some(0x2f) { CommentKind::SingleLine } else { CommentKind::MultiLine };
                    let start = pos;
                    pos += 2;
                    if kind == CommentKind::SingleLine {
                        while pos < units.len() && !is_line_break(units[pos]) {
                            pos += 1;
                        }
                    } else {
                        while pos < units.len() {
                            if units[pos] == 0x2a && units.get(pos + 1) == Some(&0x2f) {
                                pos += 2;
                                break;
                            }
                            pos += 1;
                        }
                    }
                    if collecting {
                        if let Some(p) = pending.take() {
                            out.push(p);
                        }
                        pending = Some(CommentRange { pos: start, end: pos, kind });
                    }
                    continue;
                }
                break;
            }
            _ => {
                if ch > 0x7f && (is_white_space_single_line(ch) || is_line_break(ch)) {
                    pos += 1;
                    continue;
                }
                break;
            }
        }
    }
    if let Some(p) = pending {
        out.push(p);
    }
    out
}

/// `skipTrivia(text, pos)`: the first offset at or after `pos` that is not whitespace or a comment.
pub fn skip_trivia(units: &[u16], mut pos: usize) -> usize {
    while pos < units.len() {
        let ch = units[pos];
        match ch {
            0x0a | 0x0d | 0x09 | 0x0b | 0x0c | 0x20 => pos += 1,
            0x2f => match units.get(pos + 1) {
                Some(0x2f) => {
                    pos += 2;
                    while pos < units.len() && !is_line_break(units[pos]) {
                        pos += 1;
                    }
                }
                Some(0x2a) => {
                    pos += 2;
                    while pos < units.len() {
                        if units[pos] == 0x2a && units.get(pos + 1) == Some(&0x2f) {
                            pos += 2;
                            break;
                        }
                        pos += 1;
                    }
                }
                _ => break,
            },
            c if c > 0x7f && (is_white_space_single_line(c) || is_line_break(c)) => pos += 1,
            _ => break,
        }
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn comments() {
        let t = u("a; // trailing\n/* one */ // two\nb");
        let r = leading_comment_ranges(&t, 2);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].kind, CommentKind::MultiLine);
        let r0 = leading_comment_ranges(&u("// a\nx"), 0);
        assert_eq!(r0.len(), 1);
        assert_eq!(skip_trivia(&u("  /* x */ y"), 0), 10);
    }

    #[test]
    fn parse_positions() {
        let p = parse("const й = 1\nfoo()", false);
        assert!(p.program.is_some());
        let p = parse("const = ", false);
        assert!(p.error_at.is_some());
    }
}
