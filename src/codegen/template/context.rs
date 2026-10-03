//! `codegen/template/context.ts`

use indexmap::{IndexMap, IndexSet};
use vue_sfc::core::ast::NodeId;

use crate::code::{Code, DirectiveMarker, Feat, Phase, Policy, Src, features};
use crate::codegen::{EOL, NL, Out, Resolve, new_directive};

#[derive(Default)]
struct Info {
    ignore_error: bool,
    expect_error: bool,
    /// (directive, original start)
    directives: Vec<(DirectiveMarker, usize)>,
    directive_ended: bool,
    generic: Option<(String, usize)>,
}

pub struct ComponentRef {
    pub ctx_var: String,
    pub props_var: String,
    pub ctx_used: bool,
    pub props_used: bool,
}

pub struct SlotInfo {
    pub name: String,
    pub offset: Option<usize>,
    pub tag_range: (usize, usize),
    pub props_var: String,
}

pub struct Condition {
    pub text: String,
    pub accesses: Vec<String>,
}

#[derive(Default)]
pub struct Ctx {
    stack: Vec<Info>,
    comment_buffer: Vec<(String, usize, usize)>,
    variable_id: u32,
    pub scopes: Vec<IndexSet<String>>,
    pub context_accesses: IndexMap<String, IndexMap<Src, IndexSet<usize>>>,
    pub dot_value_accesses: IndexSet<String>,
    pub access_log: Vec<String>,
    pub conditions: Vec<Condition>,
    pub hoist_vars: IndexMap<String, String>,
    pub template_refs: IndexMap<String, Vec<(String, usize)>>,
    pub components: Vec<ComponentRef>,
    pub dollar_vars: IndexSet<String>,
    pub generated_types: IndexSet<String>,
    pub inherited_attr_vars: IndexSet<String>,
    pub single_root_el_types: IndexSet<String>,
    pub single_root_nodes: IndexSet<Option<NodeId>>,
    pub slots: Vec<SlotInfo>,
    pub dynamic_slots: Vec<(String, String)>,
    pub in_v_for: bool,
}

/// `/^<!--\s*@vue-(?<name>[-\w]+)\b(?<content>[\s\S]*)-->$/`
fn match_directive_comment(source: &str) -> Option<(String, String)> {
    let rest = source.strip_prefix("<!--")?;
    let rest = rest.trim_start_matches(crate::text::is_js_space);
    let rest = rest.strip_prefix("@vue-")?;
    if !source.ends_with("-->") || source.len() < 7 {
        return None;
    }
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let run = rest.chars().take_while(|&c| is_word(c) || c == '-').map(char::len_utf8).sum::<usize>();
    let mut end = run;
    // `\b` after the name; backtrack the greedy run until it holds
    while end > 0 {
        let last = rest[..end].chars().next_back().unwrap();
        let next = rest[end..].chars().next();
        if is_word(last) != next.is_some_and(is_word) {
            break;
        }
        end -= last.len_utf8();
    }
    if end == 0 {
        return None;
    }
    let after = &rest[end..];
    // the content runs up to the final `-->`
    let content_len = after.len().checked_sub(3)?;
    if !after.ends_with("-->") {
        return None;
    }
    Some((rest[..end].to_string(), after[..content_len].to_string()))
}

impl Ctx {
    /// `enter(node)`; comments are buffered and decide the directives of the next node.
    pub fn enter_comment(&mut self, source: String, start: usize, end: usize) {
        self.comment_buffer.push((source, start, end));
    }

    pub fn enter(&mut self, out: &mut Out) -> bool {
        let mut info = Info::default();
        let comments = std::mem::take(&mut self.comment_buffer);
        let mut ignore_node: Option<(usize, usize)> = None;
        let mut expect_node: Option<(usize, usize)> = None;
        for (source, start, end) in &comments {
            if let Some((name, content)) = match_directive_comment(source) {
                match name.as_str() {
                    "skip" => return false,
                    "ignore" => {
                        info.ignore_error = true;
                        ignore_node = Some((*start, *end));
                    }
                    "expect-error" => {
                        info.expect_error = true;
                        expect_node = Some((*start, *end));
                    }
                    "generic" => {
                        let text = crate::text::js_trim(&content);
                        if text.starts_with('{') && text.ends_with('}') && text.len() >= 2 {
                            let brace = crate::text::Text::new(source).index_of("{", 0).unwrap_or(0);
                            info.generic = Some((text[1..text.len() - 1].to_string(), start + brace + 1));
                        }
                    }
                    _ => {}
                }
            }
        }
        for (policy, node) in [(Policy::Ignore, ignore_node), (Policy::Expect, expect_node)] {
            if let Some((start, end)) = node {
                info.directives.push((new_directive(policy, end - start), start));
            }
        }
        self.stack.push(info);
        self.update_resolve(out);
        true
    }

    pub fn exit(&mut self, out: &mut Out) {
        self.stack.pop();
        self.comment_buffer.clear();
        self.update_resolve(out);
    }

    fn update_resolve(&self, out: &mut Out) {
        out.resolve = match self.stack.last() {
            None => Resolve::None,
            Some(info) if info.ignore_error => {
                Resolve::Ignore(info.directives.iter().find(|(d, _)| d.policy == Policy::Ignore).map(|(d, _)| *d))
            }
            Some(info) if info.expect_error => {
                Resolve::Expect(info.directives.iter().find(|(d, _)| d.policy == Policy::Expect).map(|(d, _)| *d))
            }
            Some(_) => Resolve::None,
        };
    }

    pub fn generic(&self) -> Option<(String, usize)> {
        self.stack.last().and_then(|i| i.generic.clone())
    }

    fn marker(d: DirectiveMarker, start: usize, phase: Phase) -> Code {
        Code::Seg {
            text: String::new(),
            src: Src::Template,
            offset: start as u32,
            feat: Feat { directive: Some(DirectiveMarker { phase, ..d }), ..Feat::EMPTY },
        }
    }

    pub fn diagnostic_directive_start(&self, out: &mut Out) {
        if let Some(info) = self.stack.last() {
            for (d, start) in &info.directives {
                out.raw(Self::marker(*d, *start, Phase::Anchor));
            }
        }
    }

    pub fn diagnostic_directive_end(&mut self, out: &mut Out) {
        let Some(info) = self.stack.last_mut() else { return };
        if info.directive_ended {
            return;
        }
        info.directive_ended = true;
        let has_ignore = info.directives.iter().any(|(d, _)| d.policy == Policy::Ignore);
        for (d, start) in &info.directives {
            if d.policy == Policy::Expect && !has_ignore {
                out.raw(Self::marker(*d, *start, Phase::End));
            }
        }
    }

    pub fn internal_variable(&mut self) -> String {
        let v = format!("__VLS_{}", self.variable_id);
        self.variable_id += 1;
        v
    }

    pub fn scope(&mut self) -> usize {
        self.scopes.push(IndexSet::new());
        self.scopes.len() - 1
    }

    pub fn declare(&mut self, scope: usize, names: impl IntoIterator<Item = String>) {
        for n in names {
            self.scopes[scope].insert(n);
        }
    }

    /// `scope.end()` with its auto-import array consumed.
    pub fn end_scope(&mut self, out: &mut Out) {
        self.scopes.pop();
        self.generate_auto_import(out);
    }

    /// `scope.end()` whose returned generator is dropped.
    pub fn end_scope_silent(&mut self) {
        self.scopes.pop();
    }

    pub fn access_variable(&mut self, src: Src, name: &str, offset: Option<usize>, dot_value: bool) {
        self.access_log.push(name.to_string());
        let map = self.context_accesses.entry(name.to_string()).or_default();
        let set = map.entry(src).or_default();
        if let Some(o) = offset {
            set.insert(o);
        }
        if dot_value {
            self.dot_value_accesses.insert(name.to_string());
        }
    }

    pub fn generate_auto_import(&mut self, out: &mut Out) {
        // `all.some(([, offsets]) => offsets.size)` tests the per-source maps, which are never
        // emptied: once anything was accessed, every later scope end emits an array
        if !self.context_accesses.values().any(|m| !m.is_empty()) {
            return;
        }
        out.t(format!("// @ts-ignore{NL}"));
        out.t("[");
        for (name, map) in self.context_accesses.iter_mut() {
            for (src, offsets) in map.iter_mut() {
                if !RESERVED_WORDS.contains(&name.as_str()) {
                    for &o in offsets.iter() {
                        out.seg(name.clone(), *src, o, features::IMPORT_COMPLETION_ONLY);
                        out.t(",");
                    }
                }
                offsets.clear();
            }
        }
        out.t(format!("]{EOL}"));
    }

    pub fn generate_condition_guards(&self, out: &mut Out) {
        for c in &self.conditions {
            if !c.accesses.is_empty() && c.accesses.iter().all(|n| self.scopes.iter().any(|s| s.contains(n))) {
                continue;
            }
            out.t(format!("if (!{}) throw 0{EOL}", c.text));
        }
    }

    pub fn hoist_variable(&mut self, original: &str) -> String {
        if let Some(n) = self.hoist_vars.get(original) {
            return n.clone();
        }
        let name = format!("__VLS_{}", self.variable_id);
        self.variable_id += 1;
        self.hoist_vars.insert(original.to_string(), name.clone());
        name
    }

    pub fn generate_hoist_variables(&self, out: &mut Out, script_lang: &str) {
        if self.hoist_vars.is_empty() {
            return;
        }
        out.t("var ");
        let bang = if script_lang == "ts" || script_lang == "tsx" { "!" } else { "" };
        for (i, (original, hoist)) in self.hoist_vars.iter().enumerate() {
            if i > 0 {
                out.t(", ");
            }
            out.t(format!("{hoist} = {original}{bang}"));
        }
        out.t(EOL);
    }

    pub fn add_template_ref(&mut self, name: &str, type_exp: String, offset: usize) {
        self.template_refs.entry(name.to_string()).or_default().push((type_exp, offset));
    }

    /// `getCtxVar()` of the innermost component
    pub fn component_ctx_var(&mut self, index: usize) -> String {
        let c = &mut self.components[index];
        c.ctx_used = true;
        c.ctx_var.clone()
    }

    pub fn component_props_var(&mut self, index: usize) -> String {
        let c = &mut self.components[index];
        c.props_used = true;
        c.props_var.clone()
    }
}

// Reserved words can never be auto-imported bindings, and emitting them as bare array elements is
// a syntax error (#5267: `[class,]`).
const RESERVED_WORDS: &[&str] = &[
    "arguments", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete",
    "do", "else", "enum", "eval", "export", "extends", "false", "finally", "for", "function", "if", "implements",
    "import", "in", "instanceof", "interface", "let", "new", "null", "package", "private", "protected", "public",
    "return", "static", "super", "switch", "this", "throw", "true", "try", "typeof", "var", "void", "while", "with",
    "yield",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_comments() {
        assert_eq!(match_directive_comment("<!-- @vue-ignore -->"), Some(("ignore".into(), " ".into())));
        assert_eq!(
            match_directive_comment("<!-- @vue-generic {T} -->"),
            Some(("generic".into(), " {T} ".into()))
        );
        assert_eq!(match_directive_comment("<!-- @vue-ignore- x -->"), Some(("ignore".into(), "- x ".into())));
        assert_eq!(match_directive_comment("<!-- vue-ignore -->"), None);
    }
}
