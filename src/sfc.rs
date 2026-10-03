//! language-core's SFC parse (`utils/parseSfc.ts`, `plugins/file-vue.ts`) and the block IR
//! (`virtualCode/ir.ts`).

use indexmap::IndexMap;
use vue_sfc::core::ast::{Node, SourceLocation};
use vue_sfc::core::parser::{ParseMode, base_parse};
use vue_sfc::dom::parser_options::dom_parser_options;

use crate::code::Src;
use crate::text::Text;

/// `IRAttr`: a bare attribute, or its value with an offset into the SFC.
#[derive(Clone, Debug, PartialEq)]
pub enum IrAttr {
    True,
    Text { text: String, offset: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    True,
    Str(String),
}

#[derive(Clone, Debug)]
pub struct Block {
    pub name: Src,
    /// the tag name (`template`, `script`, `style`, or a custom block's)
    pub tag: String,
    pub lang: String,
    pub attrs: IndexMap<String, AttrValue>,
    pub content: String,
    /// offset of `<tag`
    pub start: usize,
    /// offset just past `</tag>`
    pub end: usize,
    pub start_tag_end: usize,
    pub end_tag_start: usize,
    pub src: Option<IrAttr>,
    pub generic: Option<IrAttr>,
    pub module: Option<IrAttr>,
    pub scoped: bool,
    pub setup: Option<AttrValue>,
}

impl Block {
    pub fn attr(&self, name: &str) -> Option<&AttrValue> {
        self.attrs.get(name)
    }

    pub fn has_truthy_attr(&self, name: &str) -> bool {
        match self.attrs.get(name) {
            Some(AttrValue::True) => true,
            Some(AttrValue::Str(s)) => !s.is_empty(),
            None => false,
        }
    }
}

pub struct Sfc {
    pub comments: Vec<String>,
    pub template: Option<Block>,
    pub script: Option<Block>,
    pub script_setup: Option<Block>,
    pub styles: Vec<Block>,
    pub custom_blocks: Vec<Block>,
}

/// Block as produced by `createBlock`: offsets are still the raw `loc` ones, attribute offsets are
/// relative to the element start.
struct RawBlock {
    tag: String,
    content: String,
    loc_start: usize,
    loc_end: usize,
    attrs: IndexMap<String, AttrValue>,
    lang: Option<String>,
    src: Option<IrAttr>,
    generic: Option<IrAttr>,
    module: Option<IrAttr>,
    scoped: bool,
    setup: Option<AttrValue>,
}

fn always(_: &str) -> bool {
    true
}

/// `normalizeAttributeValue(node)`: the value without its quotes, and the offset of its content.
pub fn normalize_attribute_value(source: &str, start: usize) -> (String, usize) {
    if source.len() >= 2
        && ((source.starts_with('"') && source.ends_with('"')) || (source.starts_with('\'') && source.ends_with('\'')))
    {
        return (source[1..source.len() - 1].to_string(), start + 1);
    }
    (source.to_string(), start)
}

pub fn parse(source: &str) -> Sfc {
    let text = Text::new(source);
    let mut options = dom_parser_options();
    options.is_native_tag = Some(always);
    options.is_pre_tag = always;
    options.parse_mode = ParseMode::Sfc;
    options.comments = true;
    let parsed = base_parse(source, options);
    let a = &parsed.arena;

    let mut comments = Vec::new();
    let mut template: Option<RawBlock> = None;
    let mut script: Option<RawBlock> = None;
    let mut script_setup: Option<RawBlock> = None;
    let mut styles = Vec::new();
    let mut custom = Vec::new();

    for &child in &a.root(parsed.root).children {
        match a.node(child) {
            Node::Comment(c) => comments.push(c.content.clone()),
            Node::Element(el) => {
                let block = create_block(a, el, &text);
                match el.tag.as_str() {
                    "template" => template = Some(block),
                    "script" => {
                        let is_setup = block.setup.as_ref().is_some_and(|s| match s {
                            AttrValue::True => true,
                            AttrValue::Str(s) => !s.is_empty(),
                        });
                        if is_setup && script_setup.is_none() {
                            script_setup = Some(block);
                        } else if !is_setup && script.is_none() {
                            script = Some(block);
                        }
                    }
                    "style" => styles.push(block),
                    _ => custom.push(block),
                }
            }
            _ => {}
        }
    }

    // `Element is missing end tag.` on the template's own start line: the template ends at the
    // last `<` when that is a prefix of `</template>` (#4893)
    if let Some(t) = &mut template {
        let start_line = text_line_of(source, &text, t.loc_start);
        for err in &parsed.errors {
            if err.code == 24 {
                if let Some(loc) = &err.loc {
                    if loc.start.line as usize == start_line {
                        let content = Text::new(&t.content);
                        if let Some(end_tag) = content.last_index_of("<") {
                            let end_text = crate::text::js_trim_end(content.slice_from(end_tag));
                            if "</template>".starts_with(end_text) {
                                t.loc_end = t.loc_start + end_tag;
                                t.content = content.slice(0, end_tag).to_string();
                            }
                        }
                    }
                }
            }
        }
    }

    let to_block = |raw: RawBlock, name: Src, default_lang: &str| -> Block {
        // `source.slice(0, startTagEnd).lastIndexOf('<' + type)` (a parsed block always has one)
        let start = text.slice(0, raw.loc_start).rfind(&format!("<{}", raw.tag)).map(|b| text.utf16(b)).unwrap_or(0);
        // `endTagStart + source.slice(endTagStart).indexOf('>') + 1`
        let rest = text.slice_from(raw.loc_end);
        let end = match rest.find('>') {
            Some(b) => raw.loc_end + crate::text::len16(&rest[..b]) + 1,
            None => raw.loc_end,
        };
        let rebase = |attr: Option<IrAttr>| match attr {
            Some(IrAttr::Text { text, offset }) => Some(IrAttr::Text { text, offset: start + offset }),
            other => other,
        };
        Block {
            name,
            tag: raw.tag,
            lang: raw.lang.unwrap_or_else(|| default_lang.to_string()),
            attrs: raw.attrs,
            content: raw.content,
            start,
            end,
            start_tag_end: raw.loc_start,
            end_tag_start: raw.loc_end,
            src: rebase(raw.src),
            generic: rebase(raw.generic),
            module: rebase(raw.module),
            scoped: raw.scoped,
            setup: raw.setup,
        }
    };

    Sfc {
        comments,
        template: template.map(|b| to_block(b, Src::Template, "html")),
        script: script.map(|b| to_block(b, Src::Script, "js")),
        script_setup: script_setup.map(|b| to_block(b, Src::ScriptSetup, "js")),
        styles: styles.into_iter().enumerate().map(|(i, b)| to_block(b, Src::Style(i as u32), "css")).collect(),
        custom_blocks: custom.into_iter().map(|b| to_block(b, Src::Main, "txt")).collect(),
    }
}

fn text_line_of(source: &str, text: &Text, offset: usize) -> usize {
    source[..text.byte(offset)].matches('\n').count() + 1
}

fn create_block(a: &vue_sfc::core::ast::Arena, el: &vue_sfc::core::ast::ElementNode, text: &Text) -> RawBlock {
    let mut start = el.loc.start.offset as usize;
    let end;
    let mut content = String::new();
    if !el.children.is_empty() {
        start = a.node(el.children[0]).loc().start.offset as usize;
        end = a.node(*el.children.last().unwrap()).loc().end.offset as usize;
        content = text.slice(start, end).to_string();
    } else {
        let src = Text::new(&el.loc.source);
        if let Some(offset) = src.index_of("</", 0) {
            start += offset;
        }
        end = start;
    }
    let mut attrs = IndexMap::new();
    let mut lang = None;
    let mut src_attr = None;
    let mut generic = None;
    let mut module = None;
    let mut scoped = false;
    let mut setup: Option<AttrValue> = None;
    let is_script = el.tag == "script";
    let is_style = el.tag == "style";
    for &p in &el.props {
        let Node::Attribute(attr) = a.node(p) else { continue };
        let value = match &attr.value {
            Some(v) if !v.content.is_empty() => AttrValue::Str(v.content.clone()),
            _ => AttrValue::True,
        };
        attrs.insert(attr.name.clone(), value.clone());
        match attr.name.as_str() {
            "lang" => lang = attr.value.as_ref().map(|v| v.content.clone()),
            "src" => src_attr = Some(parse_attr(attr, el)),
            name if is_script => match name {
                "vapor" => {
                    if setup.is_none() {
                        setup = Some(value);
                    }
                    if generic.is_none() {
                        generic = Some(IrAttr::True);
                    }
                }
                "setup" => setup = Some(value),
                "generic" => generic = Some(parse_attr(attr, el)),
                _ => {}
            },
            name if is_style => match name {
                "scoped" => scoped = true,
                "module" => {
                    let parsed = parse_attr(attr, el);
                    module = Some(match parsed {
                        IrAttr::Text { ref text, .. } if !text.is_empty() => parsed,
                        _ => IrAttr::Text {
                            text: String::new(),
                            offset: (attr.loc.start.offset - el.loc.start.offset) as usize,
                        },
                    });
                }
                _ => {}
            },
            _ => {}
        }
    }
    RawBlock { tag: el.tag.clone(), content, loc_start: start, loc_end: end, attrs, lang, src: src_attr, generic, module, scoped, setup }
}

fn parse_attr(p: &vue_sfc::core::ast::AttributeNode, node: &vue_sfc::core::ast::ElementNode) -> IrAttr {
    let Some(value) = &p.value else { return IrAttr::True };
    let (content, offset) = normalize_attribute_value(&value.loc.source, value.loc.start.offset as usize);
    IrAttr::Text { text: content, offset: offset - node.loc.start.offset as usize }
}

pub fn loc_range(loc: &SourceLocation) -> (usize, usize) {
    (loc.start.offset as usize, loc.end.offset as usize)
}
