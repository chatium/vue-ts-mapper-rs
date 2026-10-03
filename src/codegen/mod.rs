//! Port of `codegen/utils/*`, `codegen/localTypes.ts` and the output plumbing.

pub mod script;
pub mod style;
pub mod template;

use std::cell::RefCell;

use indexmap::IndexSet;

use crate::code::{Code, DirectiveMarker, Feat, Phase, Policy, Src, Verification, features, next_id};
use crate::sfc::Block;
use crate::shared::{camelize, capitalize, is_identifier};
use crate::text::{Text, len16};
use crate::ts_ast::Parsed;

pub const NL: &str = "\n";
pub const EOL: &str = ";\n";

/// How codes pushed now get resolved (`resolveCodeFeatures` with the innermost comment-directive
/// scope of the template codegen).
#[derive(Clone, Copy, Default)]
pub enum Resolve {
    #[default]
    None,
    Ignore(Option<DirectiveMarker>),
    Expect(Option<DirectiveMarker>),
}

/// The codegen output. Mapped codes get their features resolved when pushed, which is when the
/// reference yields them.
#[derive(Default)]
pub struct Out {
    pub codes: Vec<Code>,
    pub resolve: Resolve,
}

impl Out {
    pub fn new() -> Self {
        Out::default()
    }

    /// A scratch buffer under the same resolution.
    pub fn scratch(&self) -> Out {
        Out { codes: Vec::new(), resolve: self.resolve }
    }

    #[inline]
    pub fn t(&mut self, s: impl Into<String>) {
        self.codes.push(Code::Text(s.into()));
    }

    pub fn seg(&mut self, text: impl Into<String>, src: Src, offset: usize, feat: Feat) {
        let feat = self.resolve_feat(feat);
        self.codes.push(Code::Seg { text: text.into(), src, offset: offset as u32, feat });
    }

    fn resolve_feat(&self, feat: Feat) -> Feat {
        let verifies = matches!(feat.verification, Verification::True | Verification::Expect);
        if !verifies {
            return feat;
        }
        match self.resolve {
            Resolve::None => feat,
            Resolve::Ignore(d) => {
                let mut f = feat;
                f.verification = Verification::False;
                f.directive = d.map(|d| DirectiveMarker { phase: Phase::Content, ..d });
                f
            }
            Resolve::Expect(d) => {
                let mut f = feat;
                f.verification = Verification::Expect;
                f.directive = d.map(|d| DirectiveMarker { phase: Phase::Content, ..d });
                f
            }
        }
    }

    /// Appends codes generated earlier (already resolved).
    pub fn extend(&mut self, codes: Vec<Code>) {
        self.codes.extend(codes);
    }

    /// Appends a code as is (diagnostic directive markers).
    pub fn raw(&mut self, code: Code) {
        self.codes.push(code);
    }

    pub fn to_string(&self) -> String {
        crate::code::codes_to_string(&self.codes)
    }
}

pub fn is_ts_lang(lang: &str) -> bool {
    lang == "ts" || lang == "tsx"
}

/// `asType(type, lang)`
pub fn as_type(ty: &str, lang: &str) -> String {
    if is_ts_lang(lang) { format!("{{}} as {ty}") } else { format!("/** @type {{{ty}}} */ ({{}})") }
}

/// `generateTypedVar(kind, name, lang, type)`
pub fn typed_var(out: &mut Out, kind: &str, name: &str, lang: &str, ty: impl FnOnce(&mut Out)) {
    if is_ts_lang(lang) {
        out.t(format!("{kind} {name}!: "));
        ty(out);
        out.t(EOL);
    } else {
        out.t(format!("var {name} = /** @type {{"));
        ty(out);
        out.t(format!("}} */ ({{}}){EOL}"));
    }
}

/// `generateTypeAlias(name, lang, type)`
pub fn type_alias(out: &mut Out, name: &str, lang: &str, ty: impl FnOnce(&mut Out)) {
    if is_ts_lang(lang) {
        out.t(format!("type {name} = "));
        ty(out);
        out.t(EOL);
    } else {
        out.t("/** @typedef {");
        ty(out);
        out.t(format!("}} {name} */{NL}"));
    }
}

/// `getRefBrandArgument(vueCompilerOptions, lang)`
pub fn ref_brand_argument(lib: &str, lang: &str) -> String {
    as_type(&format!("import('{lib}').Ref<unknown>"), lang)
}

/// A script / script setup block with its parsed AST.
pub struct ScriptBlock<'a> {
    pub block: &'a Block,
    pub text: Text<'a>,
    pub parsed: Parsed,
}

/// `generateSfcBlockSection(block, start, end, features)`
pub fn sfc_block_section(out: &mut Out, sb: &ScriptBlock, start: usize, end: usize, feat: Feat) {
    let text = sb.text.slice(start, end);
    out.seg(text, sb.block.name, start, feat);
    // #3632: a truncated block gets a terminator mapped to the end of the section
    // (the reference compares absolute diagnostic offsets with the section-relative `textEnd`)
    let text_end = len16(crate::text::js_trim_end(text));
    if sb.parsed.errors.iter().any(|&(s, e)| s >= text_end && e <= end) {
        out.seg(";", sb.block.name, end, features::VERIFICATION);
        out.t(NL);
    }
}

/// The block content is followed by generated statements; without a trailing newline its last
/// statement would run into them (`foo()const __VLS_ctx = ...`).
pub fn block_terminator(out: &mut Out, block: &Block) {
    if !block.content.ends_with('\n') {
        out.t(format!("{NL};{NL}"));
    }
}

/// `Boundary.start` / `boundary.end()`: zero-length markers sharing a fresh combine token.
pub struct Boundary {
    src: Src,
    end: usize,
    pub feat: Feat,
}

impl Boundary {
    pub fn start(out: &mut Out, src: Src, start: usize, end: usize, feat: Feat) -> Boundary {
        let feat = feat.with_combine(next_id());
        out.seg("", src, start, feat);
        Boundary { src, end, feat }
    }

    pub fn end(self, out: &mut Out) {
        out.seg("", self.src, self.end, self.feat);
    }
}

/// `generateCamelized(code, source, offset, features)`
pub fn camelized(out: &mut Out, code: &str, src: Src, mut offset: usize, feat: Feat) {
    let feat = if feat.combine == 0 { feat.with_combine(next_id()) } else { feat };
    for (i, part) in code.split('-').enumerate() {
        if !part.is_empty() {
            if i == 0 {
                out.seg(part, src, offset, feat);
            } else {
                out.seg(capitalize(part), src, offset, feat);
            }
        }
        offset += len16(part) + 1;
    }
}

/// `generateEscaped(text, source, offset, features, /([\\'])/)`: text split around backslashes and
/// single quotes, each of which gets escaped.
pub fn escaped(out: &mut Out, text: &str, src: Src, mut offset: usize, feat: Feat) {
    let feat = if feat.combine == 0 { feat.with_combine(next_id()) } else { feat };
    // `text.split(/([\\'])/)`: parts alternate between plain text and a captured character
    let mut parts: Vec<&str> = Vec::new();
    let mut last = 0;
    for (i, c) in text.char_indices() {
        if c == '\\' || c == '\'' {
            parts.push(&text[last..i]);
            parts.push(&text[i..i + 1]);
            last = i + 1;
        }
    }
    parts.push(&text[last..]);
    let mut is_escape_target = false;
    for part in parts {
        if is_escape_target {
            out.t("\\");
        }
        out.seg(part, src, offset, feat);
        offset += len16(part);
        is_escape_target = !is_escape_target;
    }
}

/// `generateStringLiteralKey(code, offset, features)`
pub fn string_literal_key(out: &mut Out, code: &str, offset: usize, feat: Feat) {
    let b = Boundary::start(out, Src::Template, offset, offset + len16(code), feat);
    out.t("'");
    out.seg(code, Src::Template, offset, b.feat);
    out.t("'");
    b.end(out);
}

/// `generateUnicode(code, offset, features)`
pub fn unicode(out: &mut Out, code: &str, offset: usize, feat: Feat) {
    if code.contains('\\') || code.contains('\n') {
        let b = Boundary::start(out, Src::Template, offset, offset + len16(code), feat);
        let mut s = String::with_capacity(code.len() * 6);
        for unit in code.encode_utf16() {
            s.push_str(&format!("\\u{unit:04x}"));
        }
        out.t(s);
        b.end(out);
    } else {
        out.seg(code, Src::Template, offset, feat);
    }
}

/// `generateSpreadMerge(...codes)`
pub fn spread_merge(out: &mut Out, codes: Vec<Code>) {
    if codes.len() <= 1 {
        out.extend(codes);
    } else {
        out.t(format!("{{{NL}"));
        for c in codes {
            out.t("...");
            out.raw(c);
            out.t(format!(",{NL}"));
        }
        out.t("}");
    }
}

/// `utils/transform.ts`: a replaced range or an insertion point, with its generator.
pub struct CodeTransform<'f> {
    pub start: usize,
    pub end: usize,
    pub generate: Box<dyn FnOnce(&mut Out) + 'f>,
}

pub fn replace<'f>(start: usize, end: usize, f: impl FnOnce(&mut Out) + 'f) -> CodeTransform<'f> {
    CodeTransform { start, end, generate: Box::new(f) }
}

pub fn insert<'f>(pos: usize, f: impl FnOnce(&mut Out) + 'f) -> CodeTransform<'f> {
    CodeTransform { start: pos, end: pos, generate: Box::new(f) }
}

/// `generateCodeWithTransforms(start, end, transforms, section)`
pub fn code_with_transforms(
    out: &mut Out,
    mut start: usize,
    end: usize,
    mut transforms: Vec<CodeTransform>,
    section: &dyn Fn(&mut Out, usize, usize),
) {
    // stable sort, like `Array.prototype.sort`
    transforms.sort_by_key(|t| t.start);
    for t in transforms {
        section(out, start, t.start);
        (t.generate)(out);
        start = t.end;
    }
    section(out, start, end);
}

/// `getLocalTypesGenerator`: helper types emitted on first use, in order of first use.
pub struct LocalTypes {
    is_ts: bool,
    lib: String,
    used: RefCell<IndexSet<&'static str>>,
}

pub const PRETTIFY_LOCAL: &str = "__VLS_PrettifyLocal";
pub const WITH_DEFAULTS: &str = "__VLS_WithDefaults";
pub const WITH_SLOTS: &str = "__VLS_WithSlots";
pub const PROPS_CHILDREN: &str = "__VLS_PropsChildren";
pub const TYPE_PROPS_TO_OPTION: &str = "__VLS_TypePropsToOption";
pub const OMIT_INDEX_SIGNATURE: &str = "__VLS_OmitIndexSignature";

impl LocalTypes {
    pub fn new(lib: &str, lang: &str) -> Self {
        LocalTypes { is_ts: is_ts_lang(lang), lib: lib.to_string(), used: RefCell::new(IndexSet::new()) }
    }

    /// Reading a helper's name marks it used.
    pub fn name(&self, name: &'static str) -> &'static str {
        self.used.borrow_mut().insert(name);
        name
    }

    fn js_typedef(params: &str, name: &str, body: &str) -> String {
        format!("/**{NL} * @template {params}{NL} * @typedef {{{body}}} {name}{NL} */{NL}")
    }

    fn generate_one(&self, name: &str) -> String {
        let lib = &self.lib;
        match name {
            WITH_DEFAULTS => {
                let pl = self.name(PRETTIFY_LOCAL);
                if self.is_ts {
                    format!(
                        "type __VLS_WithDefaults<P, D> = {{\n\t[K in keyof Pick<P, keyof P>]: K extends keyof D\n\t\t? {pl}<P[K] & {{ default: D[K] }}>\n\t\t: P[K]\n}};\n"
                    )
                } else {
                    Self::js_typedef(
                        "P, D",
                        "__VLS_WithDefaults",
                        &format!("{{ [K in keyof Pick<P, keyof P>]: K extends keyof D ? {pl}<P[K] & {{ default: D[K] }}> : P[K] }}"),
                    )
                }
            }
            PRETTIFY_LOCAL => {
                if self.is_ts {
                    format!("type __VLS_PrettifyLocal<T> = (T extends any ? {{ [K in keyof T]: T[K]; }} : {{ [K in keyof T as K]: T[K]; }}) & {{}}{EOL}")
                } else {
                    Self::js_typedef(
                        "T",
                        "__VLS_PrettifyLocal",
                        "(T extends any ? { [K in keyof T]: T[K]; } : { [K in keyof T as K]: T[K]; }) & {}",
                    )
                }
            }
            WITH_SLOTS => {
                if self.is_ts {
                    "type __VLS_WithSlots<T, S> = T & {\n\tnew(): {\n\t\t$slots: S;\n\t}\n};\n".to_string()
                } else {
                    Self::js_typedef("T, S", "__VLS_WithSlots", "T & { new(): { $slots: S; } }")
                }
            }
            PROPS_CHILDREN => {
                if self.is_ts {
                    "type __VLS_PropsChildren<S> = {\n\t[K in keyof (\n\t\tboolean extends (\n\t\t\t// @ts-ignore\n\t\t\tJSX.ElementChildrenAttribute extends never\n\t\t\t\t? true\n\t\t\t\t: false\n\t\t)\n\t\t\t? never\n\t\t\t// @ts-ignore\n\t\t\t: JSX.ElementChildrenAttribute\n\t)]?: S;\n};\n".to_string()
                } else {
                    format!("// @ts-ignore{NL}/** @template S @typedef {{{{ [K in keyof (boolean extends (JSX.ElementChildrenAttribute extends never ? true : false) ? never : JSX.ElementChildrenAttribute)]?: S; }}}} __VLS_PropsChildren */{NL}")
                }
            }
            TYPE_PROPS_TO_OPTION => {
                if self.is_ts {
                    format!(
                        "type __VLS_TypePropsToOption<T> = {{\n\t[K in keyof T]-?: {{}} extends Pick<T, K>\n\t\t? {{ type: import('{lib}').PropType<Required<T>[K]> }}\n\t\t: {{ type: import('{lib}').PropType<T[K]>, required: true }}\n}};\n"
                    )
                } else {
                    Self::js_typedef(
                        "T",
                        "__VLS_TypePropsToOption",
                        &format!("{{ [K in keyof T]-?: {{}} extends Pick<T, K> ? {{ type: import('{lib}').PropType<Required<T>[K]> }} : {{ type: import('{lib}').PropType<T[K]>, required: true }} }}"),
                    )
                }
            }
            OMIT_INDEX_SIGNATURE => {
                if self.is_ts {
                    format!("type __VLS_OmitIndexSignature<T> = {{ [K in keyof T as {{}} extends Record<K, unknown> ? never : K]: T[K]; }}{EOL}")
                } else {
                    Self::js_typedef(
                        "T",
                        "__VLS_OmitIndexSignature",
                        "{ [K in keyof T as {} extends Record<K, unknown> ? never : K]: T[K] }",
                    )
                }
            }
            _ => unreachable!(),
        }
    }

    /// `generate()`: every used helper, including ones marked while generating others.
    pub fn generate(&self, out: &mut Out) {
        let mut i = 0;
        loop {
            let name = {
                let used = self.used.borrow();
                match used.get_index(i) {
                    Some(n) => *n,
                    None => break,
                }
            };
            out.t(self.generate_one(name));
            i += 1;
        }
        self.used.borrow_mut().clear();
    }
}

/// `capitalize(camelize(name))`
pub fn pascal(name: &str) -> String {
    capitalize(&camelize(name))
}

pub fn identifier(s: &str) -> bool {
    is_identifier(s)
}

pub fn new_directive(policy: Policy, original_length: usize) -> DirectiveMarker {
    DirectiveMarker { id: next_id(), policy, original_length: original_length as u32, phase: Phase::Anchor }
}
