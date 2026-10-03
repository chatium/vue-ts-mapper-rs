//! `codegen/template/*`: the template's type-checking code.

pub mod binding_references;
pub mod context;
pub mod element;
pub mod interpolation;

use indexmap::IndexSet;
use vue_sfc::core::ast::{Arena, ElementType, Node, NodeId};

use crate::code::{Feat, Src, features};
use crate::codegen::template::context::Ctx;
use crate::codegen::template::interpolation::{InterpOpts, interpolation};
use crate::codegen::{Boundary, EOL, NL, Out, as_type, camelized, escaped, is_ts_lang, string_literal_key, type_alias, typed_var};
use crate::names;
use crate::options::VueOptions;
use crate::parsers::collect_binding_names;
use crate::sfc::{Block, normalize_attribute_value};
use crate::shared::{camelize, hyphenate, is_identifier};
use crate::template::TemplateAst;
use crate::text::{Text, len16};
use crate::ts_ast;

pub struct TemplateOptions<'a> {
    pub vue: &'a VueOptions,
    pub template: &'a Block,
    pub text: Text<'a>,
    pub ast: Option<&'a TemplateAst>,
    pub is_vapor: bool,
    pub script_lang: &'a str,
    pub component_name: &'a str,
    pub setup_consts: &'a IndexSet<String>,
    pub imported_components: &'a IndexSet<String>,
    pub setup_refs: &'a IndexSet<String>,
    pub setup_bindings: &'a IndexSet<String>,
    pub dot_value_bindings: &'a IndexSet<String>,
    pub reassert_bindings: &'a IndexSet<String>,
    pub has_define_slots: bool,
    pub props_assign_name: Option<&'a str>,
    pub slots_assign_name: Option<&'a str>,
    pub inherit_attrs: bool,
    pub expr_cache: &'a interpolation::ExprCache,
}

impl<'a> TemplateOptions<'a> {
    pub fn interp(&self) -> InterpOpts<'_> {
        InterpOpts {
            setup_consts: self.setup_consts,
            imported_components: self.imported_components,
            setup_refs: self.setup_refs,
            setup_bindings: self.setup_bindings,
            dot_value_bindings: self.dot_value_bindings,
            reassert_bindings: self.reassert_bindings,
            lib: &self.vue.lib,
            script_lang: self.script_lang,
            cache: self.expr_cache,
        }
    }

    pub fn arena(&self) -> &'a Arena {
        &self.ast.expect("template ast").arena
    }

    /// `generateInterpolation(options, ctx, options.template, ...)`
    #[allow(clippy::too_many_arguments)]
    pub fn interpolate(&self, ctx: &mut Ctx, out: &mut Out, data: Feat, code: &str, start: usize, prefix: &str, suffix: &str, in_narrowing: bool) {
        interpolation(&self.interp(), ctx, out, Src::Template, data, code, start, prefix, suffix, in_narrowing);
    }
}

pub struct TemplateResult {
    pub codes: Vec<crate::code::Code>,
    pub generated_types: IndexSet<String>,
    pub dot_value_accesses: IndexSet<String>,
}

/// `generateTemplate(options)`
pub fn generate(o: &TemplateOptions) -> TemplateResult {
    let mut ctx = Ctx::default();
    let mut out = Out::new();
    let scope = ctx.scope();
    if let Some(n) = o.slots_assign_name {
        ctx.declare(scope, [n.to_string()]);
    }
    if let Some(n) = o.props_assign_name {
        ctx.declare(scope, [n.to_string()]);
    }
    if o.vue.infer_template_dollar_slots {
        ctx.dollar_vars.insert("$slots".into());
    }
    if o.vue.infer_template_dollar_attrs {
        ctx.dollar_vars.insert("$attrs".into());
    }
    if o.vue.infer_template_dollar_refs {
        ctx.dollar_vars.insert("$refs".into());
    }
    if o.vue.infer_template_dollar_el {
        ctx.dollar_vars.insert("$el".into());
    }
    if let Some(ast) = o.ast {
        template_child(o, &mut ctx, &mut out, ast.root, true, false);
    }
    ctx.generate_hoist_variables(&mut out, o.script_lang);
    slots_type(o, &mut ctx, &mut out);
    inherited_attrs_type(o, &mut ctx, &mut out);
    template_refs_type(o, &mut ctx, &mut out);
    root_el_type(o, &mut ctx, &mut out);

    if !ctx.dollar_vars.is_empty() {
        let lib = &o.vue.lib;
        let dv = ctx.dollar_vars.clone();
        let gt = ctx.generated_types.clone();
        typed_var(&mut out, "var", names::DOLLARS, o.script_lang, |out| {
            out.t(format!("{{{NL}"));
            if dv.contains("$slots") {
                let ty = if gt.contains(names::T_SLOTS) { names::T_SLOTS } else { "{}" };
                out.t(format!("$slots: {ty}{EOL}"));
            }
            if dv.contains("$attrs") {
                out.t(format!("$attrs: import('{lib}').ComponentPublicInstance['$attrs']"));
                if gt.contains(names::T_INHERITED_ATTRS) {
                    out.t(format!(" & {}", names::T_INHERITED_ATTRS));
                }
                out.t(EOL);
            }
            if dv.contains("$refs") {
                let ty = if gt.contains(names::T_TEMPLATE_REFS) { names::T_TEMPLATE_REFS } else { "{}" };
                out.t(format!("$refs: {ty}{EOL}"));
            }
            if dv.contains("$el") {
                let ty = if gt.contains(names::T_ROOT_EL) { names::T_ROOT_EL } else { "any" };
                out.t(format!("$el: {ty}{EOL}"));
            }
            out.t(format!("}} & {{ [K in keyof import('{lib}').ComponentPublicInstance]: unknown }}"));
        });
    }
    ctx.end_scope(&mut out);
    TemplateResult { codes: out.codes, generated_types: ctx.generated_types, dot_value_accesses: ctx.dot_value_accesses }
}

fn slots_type(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out) {
    if o.has_define_slots {
        ctx.generated_types.insert(names::T_SLOTS.into());
        return;
    }
    if ctx.slots.is_empty() && ctx.dynamic_slots.is_empty() {
        return;
    }
    ctx.generated_types.insert(names::T_SLOTS.into());
    let dynamic = std::mem::take(&mut ctx.dynamic_slots);
    let slots = std::mem::take(&mut ctx.slots);
    type_alias(out, names::T_SLOTS, o.script_lang, |out| {
        out.t("{}");
        for (exp_var, props_var) in &dynamic {
            out.t(format!("{NL}& {{ [K in NonNullable<typeof {exp_var}>]?: (props: typeof {props_var}) => any }}"));
        }
        for slot in &slots {
            out.t(format!("{NL}& {{ "));
            if let (false, Some(offset)) = (slot.name.is_empty(), slot.offset) {
                object_property(o, ctx, out, &slot.name, offset, features::NAVIGATION, false, false);
            } else {
                let b = Boundary::start(out, Src::Template, slot.tag_range.0, slot.tag_range.1, features::NAVIGATION);
                out.t("default");
                b.end(out);
            }
            out.t(format!("?: (props: typeof {}) => any }}", slot.props_var));
        }
    });
    ctx.dynamic_slots = dynamic;
    ctx.slots = slots;
}

fn inherited_attrs_type(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out) {
    if ctx.inherited_attr_vars.is_empty() {
        return;
    }
    ctx.generated_types.insert(names::T_INHERITED_ATTRS.into());
    let ty = ctx.inherited_attr_vars.iter().map(|n| format!("typeof {n}")).collect::<Vec<_>>().join(" & ");
    type_alias(out, names::T_INHERITED_ATTRS, o.script_lang, |out| {
        out.t(if o.vue.check_required_fallthrough_attributes { ty.clone() } else { format!("Partial<{ty}>") });
    });
}

fn template_refs_type(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out) {
    if ctx.template_refs.is_empty()
        || !(o.vue.infer_template_dollar_refs || o.vue.infer_component_dollar_refs || !o.setup_refs.is_empty())
    {
        return;
    }
    ctx.generated_types.insert(names::T_TEMPLATE_REFS.into());
    let refs = std::mem::take(&mut ctx.template_refs);
    type_alias(out, names::T_TEMPLATE_REFS, o.script_lang, |out| {
        out.t("{}");
        for (name, list) in &refs {
            out.t(format!("{NL}& "));
            if list.len() >= 2 {
                out.t("(");
            }
            for (i, (type_exp, offset)) in list.iter().enumerate() {
                if i > 0 {
                    out.t(" | ");
                }
                out.t("{ ");
                object_property(o, ctx, out, name, *offset, features::NAVIGATION, false, false);
                out.t(format!(": {type_exp} }}"));
            }
            if list.len() >= 2 {
                out.t(")");
            }
        }
    });
    ctx.template_refs = refs;
}

fn root_el_type(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out) {
    if ctx.single_root_el_types.is_empty() || ctx.single_root_nodes.contains(&None) {
        return;
    }
    ctx.generated_types.insert(names::T_ROOT_EL.into());
    let types = ctx.single_root_el_types.clone();
    type_alias(out, names::T_ROOT_EL, o.script_lang, |out| {
        for t in &types {
            out.t(format!("{NL}| {t}"));
        }
    });
}

// ---------------------------------------------------------------------------------------------
// templateChild.ts

pub fn template_child(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId, enter_node: bool, treat_template_as_fragment: bool) {
    let a = o.arena();
    if enter_node {
        if let Node::Comment(c) = a.node(node) {
            ctx.enter_comment(c.loc.source.to_string(), c.loc.start.offset as usize, c.loc.end.offset as usize);
            return;
        }
        if !ctx.enter(out) {
            return;
        }
        ctx.diagnostic_directive_start(out);
    }

    match a.node(node) {
        Node::Root(r) => {
            for item in collect_single_root_nodes(o, &r.children, false) {
                ctx.single_root_nodes.insert(item);
            }
            for &child in &r.children {
                template_child(o, ctx, out, child, true, false);
            }
        }
        Node::Element(el) => {
            if el.tag_type == ElementType::Slot {
                element::slot_outlet(o, ctx, out, node);
            } else {
                let slot_dir = el.props.iter().copied().find(|&p| matches!(a.node(p), Node::Directive(d) if d.name == "slot"));
                if el.tag_type == ElementType::Template && !ctx.components.is_empty() && slot_dir.is_some() {
                    let idx = ctx.components.len() - 1;
                    let ctx_var = ctx.component_ctx_var(idx);
                    v_slot(o, ctx, out, node, slot_dir, &ctx_var);
                } else if el.tag_type == ElementType::Template && treat_template_as_fragment {
                    element::fragment(o, ctx, out, node);
                } else if el.tag_type == ElementType::Component {
                    element::component(o, ctx, out, node);
                } else {
                    element::element(o, ctx, out, node);
                }
            }
        }
        Node::CompoundExpression(c) => {
            // {{ ... }} {{ ... }}
            for &child in &c.children {
                if matches!(a.node(child), Node::Str(_)) {
                    continue;
                }
                template_child(o, ctx, out, child, false, false);
            }
        }
        Node::Interpolation(i) => {
            let (content, start) = parse_interpolation_node(o, a.node(i.content).loc());
            o.interpolate(ctx, out, features::ALL, &content, start, "(", &format!("){EOL}"), false);
        }
        Node::If(_) => v_if(o, ctx, out, node),
        Node::For(_) => v_for(o, ctx, out, node),
        _ => {}
    }

    if enter_node {
        ctx.diagnostic_directive_end(out);
        ctx.exit(out);
    }
}

/// `collectSingleRootNodes`
fn collect_single_root_nodes(o: &TemplateOptions, children: &[NodeId], treat_template_as_fragment: bool) -> Vec<Option<NodeId>> {
    let a = o.arena();
    let children: Vec<NodeId> = children.iter().copied().filter(|&c| !matches!(a.node(c), Node::Comment(_))).collect();
    if children.len() != 1 {
        return if children.len() > 1 { vec![None] } else { vec![] };
    }
    let child = children[0];
    match a.node(child) {
        Node::If(n) => {
            let mut out = Vec::new();
            for &b in &n.branches {
                out.extend(collect_single_root_nodes(o, &a.branch(b).children, true));
            }
            out
        }
        Node::Element(el) => {
            if el.tag_type == ElementType::Template && treat_template_as_fragment {
                return collect_single_root_nodes(o, &el.children, false);
            }
            let mut out = vec![Some(child)];
            let tag = hyphenate(&el.tag);
            if o.vue.fallthrough_component_names.contains(&tag) {
                out.extend(collect_single_root_nodes(o, &el.children, false));
            }
            out
        }
        _ => vec![],
    }
}

/// `parseInterpolationNode(node, template)`: the expression widened over surrounding whitespace.
pub fn parse_interpolation_node(o: &TemplateOptions, loc: &vue_sfc::core::ast::SourceLocation) -> (String, usize) {
    let t = &o.text;
    let mut start = loc.start.offset as usize;
    let mut end = loc.end.offset as usize;
    let is_blank = |o: Option<u16>| o.is_some_and(|c| char::from_u32(c as u32).is_some_and(crate::text::is_js_space));
    while start > 0 && is_blank(t.code_at(start - 1)) {
        start -= 1;
    }
    while is_blank(t.code_at(end)) {
        end += 1;
    }
    (t.slice(start, end).to_string(), start)
}

// ---------------------------------------------------------------------------------------------
// objectProperty.ts / propertyAccess.ts

#[allow(clippy::too_many_arguments)]
pub fn object_property(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, code: &str, offset: usize, feat: Feat, should_camelize: bool, should_be_constant: bool) {
    if code.starts_with('[') && code.ends_with(']') && code.len() >= 2 {
        if should_be_constant {
            o.interpolate(ctx, out, feat, &code[1..code.len() - 1], offset + 1, &format!("[{}(", names::TRY_AS_CONSTANT), ")]", false);
        } else {
            o.interpolate(ctx, out, feat, code, offset, "", "", false);
        }
    } else if should_camelize {
        if is_identifier(&camelize(code)) {
            camelized(out, code, Src::Template, offset, feat);
        } else {
            let b = Boundary::start(out, Src::Template, offset, offset + len16(code), feat);
            out.t("'");
            camelized(out, code, Src::Template, offset, b.feat);
            out.t("'");
            b.end(out);
        }
    } else if is_identifier(code) {
        out.seg(code, Src::Template, offset, feat);
    } else {
        string_literal_key(out, code, offset, feat);
    }
}

pub fn property_access(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, code: &str, offset: usize, feat: Feat) {
    if code.starts_with('[') && code.ends_with(']') && code.len() >= 2 {
        o.interpolate(ctx, out, feat, code, offset, "", "", false);
    } else if is_identifier(code) {
        out.t(".");
        out.seg(code, Src::Template, offset, feat);
    } else {
        out.t("[");
        string_literal_key(out, code, offset, feat);
        out.t("]");
    }
}

// ---------------------------------------------------------------------------------------------
// vIf.ts

fn v_if(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId) {
    let a = o.arena();
    let original_len = ctx.conditions.len();
    let branches = a.if_node(node).branches.clone();
    for (i, &b) in branches.iter().enumerate() {
        let branch = a.branch(b);
        if i == 0 {
            out.t("if ");
        } else if branch.condition.is_some() {
            out.t("else if ");
        } else {
            out.t("else ");
        }
        let mut added = false;
        if let Some(cond) = branch.condition {
            if let Node::SimpleExpression(e) = a.node(cond) {
                let mark = ctx.access_log.len();
                let mut scratch = out.scratch();
                // a bare `true` / `false` makes TypeScript's binder mark the other branch unreachable,
                // where references lose their narrowing (and the `.value` assertions); parenthesized,
                // it is an ordinary condition
                let (prefix, suffix) = if matches!(e.content.trim(), "true" | "false") { ("((", "))") } else { ("(", ")") };
                o.interpolate(ctx, &mut scratch, features::ALL, &e.content, e.loc.start.offset as usize, prefix, suffix, true);
                let text = scratch.to_string();
                out.extend(scratch.codes);
                ctx.conditions.push(context::Condition { text, accesses: ctx.access_log[mark..].to_vec() });
                added = true;
                out.t(" ");
            }
        }
        out.t(format!("{{{NL}"));
        for &child in &branch.children {
            template_child(o, ctx, out, child, i != 0, true);
        }
        out.t(format!("}}{NL}"));
        if added {
            let c = ctx.conditions.last_mut().unwrap();
            c.text = format!("!{}", c.text);
        }
    }
    ctx.conditions.truncate(original_len);
}

// ---------------------------------------------------------------------------------------------
// vFor.ts

fn v_for(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId) {
    let a = o.arena();
    let f = a.for_node(node);
    let pr = &f.parse_result;
    let source = pr.source;
    // parseVForNode
    let first = pr.value.or(pr.key).or(pr.index);
    let last = pr.index.or(pr.key).or(pr.value);
    let left = match (first, last) {
        (Some(fst), Some(lst)) => {
            let start = a.node(fst).loc().start.offset as usize;
            let end = a.node(lst).loc().end.offset as usize;
            let base = f.loc.start.offset as usize;
            let src = Text::new(&f.loc.source);
            Some((start, src.slice(start - base, end - base).to_string()))
        }
        _ => None,
    };
    let scope = ctx.scope();
    let mut binding_names: Vec<String> = Vec::new();
    let mut default_ranges: Vec<(usize, usize)> = Vec::new();
    if let Some((_, left_text)) = left.as_ref().filter(|(_, t)| !t.is_empty()) {
        let wrap = "const [";
        let full = format!("{wrap}{left_text}]");
        let parsed = ts_ast::parse_as(&full, false, false);
        let units: Vec<u16> = full.encode_utf16().collect();
        let src = crate::parsers::Src { text: &full, units: units.clone(), p: &parsed };
        if let Some(swc_core::ecma::ast::Program::Script(s)) = &parsed.program {
            if let Some(swc_core::ecma::ast::Stmt::Decl(swc_core::ecma::ast::Decl::Var(v))) = s.body.first() {
                if let Some(d) = v.decls.first() {
                    binding_names = collect_binding_names(&src, &d.name);
                    let mut inits = Vec::new();
                    collect_default_initializers(&d.name, &mut inits);
                    for init in inits {
                        let (s0, e0) = parsed.range(swc_core::common::Spanned::span(init));
                        default_ranges.push((s0 - len16(wrap), e0 - len16(wrap)));
                    }
                    default_ranges.sort_by_key(|r| r.0);
                }
            }
        }
    }

    let mut source_alias = None;
    if let Node::SimpleExpression(e) = a.node(source) {
        let alias = ctx.internal_variable();
        out.t(format!("const {alias} = {}(", names::TRY_AS_CONSTANT));
        o.interpolate(ctx, out, features::ALL, &e.content, e.loc.start.offset as usize, "(", ")", false);
        out.t(")");
        out.t(EOL);
        source_alias = Some(alias);
    }
    ctx.declare(scope, binding_names);

    out.t("for (const [");
    if let Some((left_start, left_text)) = left.as_ref().filter(|(_, t)| !t.is_empty()) {
        let lt = Text::new(left_text);
        let mut last_offset = 0;
        for &(s0, e0) in &default_ranges {
            if s0 > last_offset {
                out.seg(lt.slice(last_offset, s0), Src::Template, left_start + last_offset, features::ALL);
            }
            o.interpolate(ctx, out, features::ALL, lt.slice(s0, e0), left_start + s0, "", "", false);
            last_offset = e0;
        }
        if last_offset < lt.len() {
            out.seg(lt.slice_from(last_offset), Src::Template, left_start + last_offset, features::ALL);
        }
    }
    out.t("] of ");
    if let Some(alias) = source_alias {
        out.t(format!("{}({}(", names::V_FOR, names::NON_NULL));
        out.t(alias);
        out.t("))");
    } else {
        out.t(as_type("any", o.script_lang));
    }
    out.t(format!(") {{{NL}"));
    let in_v_for = ctx.in_v_for;
    ctx.in_v_for = true;
    for &child in &a.for_node(node).children {
        template_child(o, ctx, out, child, false, true);
    }
    ctx.in_v_for = in_v_for;
    ctx.end_scope(out);
    out.t(format!("}}{NL}"));
}

fn collect_default_initializers<'p>(pat: &'p swc_core::ecma::ast::Pat, out: &mut Vec<&'p swc_core::ecma::ast::Expr>) {
    use swc_core::ecma::ast::{ObjectPatProp, Pat};
    let element = |el: &'p Pat, out: &mut Vec<&'p swc_core::ecma::ast::Expr>| {
        let (name, init) = match el {
            Pat::Assign(a) => (a.left.as_ref(), Some(a.right.as_ref())),
            Pat::Rest(r) => (r.arg.as_ref(), None),
            other => (other, None),
        };
        if !matches!(name, Pat::Ident(_)) {
            collect_default_initializers(name, out);
        }
        if let Some(i) = init {
            out.push(i);
        }
    };
    match pat {
        Pat::Array(a) => {
            for el in a.elems.iter().flatten() {
                element(el, out);
            }
        }
        Pat::Object(o) => {
            for p in &o.props {
                match p {
                    ObjectPatProp::KeyValue(kv) => element(&kv.value, out),
                    ObjectPatProp::Assign(a) => {
                        if let Some(v) = &a.value {
                            out.push(v);
                        }
                    }
                    ObjectPatProp::Rest(r) => element(&r.arg, out),
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// vSlot.ts

pub fn v_slot(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId, slot_dir: Option<NodeId>, ctx_var: &str) {
    let a = o.arena();
    let slot_var = ctx.internal_variable();
    if let Some(sd) = slot_dir {
        let d = a.dir(sd);
        out.t(format!("{{{NL}"));
        out.t("const { ");
        match d.arg.map(|arg| a.node(arg)) {
            Some(Node::SimpleExpression(arg)) if !arg.content.is_empty() => {
                let feat = if arg.is_static { features::WITHOUT_HIGHLIGHT } else { features::ALL };
                object_property(o, ctx, out, &arg.loc.source, arg.loc.start.offset as usize, feat, false, true);
            }
            _ => {
                let start = d.loc.start.offset as usize;
                let raw = d.raw_name.as_deref().map(len16).unwrap_or(0);
                let b = Boundary::start(out, Src::Template, start, start + raw, features::WITHOUT_HIGHLIGHT_AND_COMPLETION);
                out.t("default");
                b.end(out);
            }
        }
    } else {
        out.t("const { ");
        // #932: reference for implicit default slot
        let loc = &a.node(node).loc();
        let b = Boundary::start(out, Src::Template, loc.start.offset as usize, loc.end.offset as usize, features::NAVIGATION);
        out.t("default");
        b.end(out);
    }
    out.t(format!(": {slot_var} }} = {}({ctx_var}.slots){EOL}", names::NON_NULL));

    let scope = ctx.scope();
    if let Some(sd) = slot_dir {
        if let Some(exp) = a.dir(sd).exp {
            if let Node::SimpleExpression(e) = a.node(exp) {
                let full = format!("({}) => {{}}", e.content);
                let parsed = ts_ast::parse_as(&full, false, false);
                slot_parameters(o, ctx, out, &full, &parsed, e, &slot_var);
                let names = vslot_binding_names(&full, &parsed);
                ctx.declare(scope, names);
            }
        }
    }
    ctx.diagnostic_directive_end(out);
    for &child in &a.el(node).children {
        template_child(o, ctx, out, child, true, false);
    }
    ctx.end_scope(out);

    if let Some(sd) = slot_dir {
        let d = a.dir(sd);
        let mut is_static = true;
        if let Some(Node::SimpleExpression(arg)) = d.arg.map(|x| a.node(x)) {
            is_static = arg.is_static;
        }
        out.t(format!("}}{NL}"));
        if is_static && d.arg.is_none() {
            let src: &str = &d.loc.source;
            if src.starts_with('#') || src.starts_with("v-slot:") {
                out.t(format!("/** @type {{NonNullable<typeof {ctx_var}['slots']>['"));
                let skip = if src.starts_with('#') { 1 } else { "v-slot:".len() };
                out.seg("", Src::Template, d.loc.start.offset as usize + skip, features::COMPLETION);
                out.t(format!("']}} */{EOL}"));
            }
        }
    }
}

/// `collectBindingNames(ts, slotAst, slotAst)`: every identifier under the arrow function, except
/// the default values of binding pattern elements.
fn vslot_binding_names(full: &str, parsed: &ts_ast::Parsed) -> Vec<String> {
    use swc_core::ecma::ast::*;
    use swc_core::ecma::visit::{Visit, VisitWith};
    struct V<'a> {
        units: Vec<u16>,
        p: &'a ts_ast::Parsed,
        out: Vec<String>,
    }
    impl V<'_> {
        fn name(&mut self, id: &Ident) {
            let (s, e) = self.p.range(id.span);
            self.out.push(String::from_utf16_lossy(&self.units[s..e]));
        }
    }
    impl Visit for V<'_> {
        fn visit_ident(&mut self, id: &Ident) {
            self.name(id);
        }
        fn visit_ident_name(&mut self, id: &IdentName) {
            let (s, e) = self.p.range(id.span);
            self.out.push(String::from_utf16_lossy(&self.units[s..e]));
        }
        fn visit_binding_ident(&mut self, b: &BindingIdent) {
            let (s, e) = ts_ast::binding_ident_range(self.p, &self.units, b);
            self.out.push(String::from_utf16_lossy(&self.units[s..e]));
            b.type_ann.visit_with(self);
        }
        // a pattern element contributes its name only
        fn visit_object_pat_prop(&mut self, p: &ObjectPatProp) {
            match p {
                ObjectPatProp::KeyValue(kv) => pattern_element_names(self, &kv.value),
                ObjectPatProp::Assign(a) => self.name(&a.key.id),
                ObjectPatProp::Rest(r) => pattern_element_names(self, &r.arg),
            }
        }
        fn visit_array_pat(&mut self, a: &ArrayPat) {
            for el in a.elems.iter().flatten() {
                pattern_element_names(self, el);
            }
            a.type_ann.visit_with(self);
        }
    }
    fn pattern_element_names(v: &mut V, el: &Pat) {
        match el {
            Pat::Assign(a) => pattern_element_names(v, &a.left),
            Pat::Rest(r) => pattern_element_names(v, &r.arg),
            Pat::Ident(b) => {
                let (s, e) = ts_ast::binding_ident_range(v.p, &v.units, b);
                v.out.push(String::from_utf16_lossy(&v.units[s..e]));
            }
            Pat::Array(a) => {
                for e in a.elems.iter().flatten() {
                    pattern_element_names(v, e);
                }
            }
            Pat::Object(o) => {
                for p in &o.props {
                    v.visit_object_pat_prop(p);
                }
            }
            _ => {}
        }
    }
    let mut v = V { units: full.encode_utf16().collect(), p: parsed, out: Vec::new() };
    if let Some(program) = &parsed.program {
        program.visit_with(&mut v);
    }
    v.out
}

fn slot_parameters(
    o: &TemplateOptions,
    ctx: &mut Ctx,
    out: &mut Out,
    full: &str,
    parsed: &ts_ast::Parsed,
    exp: &vue_sfc::core::ast::SimpleExpressionNode,
    slot_var: &str,
) {
    use swc_core::ecma::ast::*;
    let Some(Program::Script(s)) = &parsed.program else { return };
    let Some(Stmt::Expr(es)) = s.body.first() else { return };
    let Expr::Arrow(arrow) = es.expr.as_ref() else { return };
    let start_offset = exp.loc.start.offset as usize - 1;
    let full_len = len16(full);
    let mut scratch = out.scratch();
    o.interpolate(ctx, &mut scratch, features::ALL, full, start_offset, "", "", false);
    let mut interp = scratch.codes;
    replace_source_range(&mut interp, start_offset, start_offset + 1);
    replace_source_range(&mut interp, start_offset + full_len - len16(") => {}"), start_offset + full_len);
    let units: Vec<u16> = full.encode_utf16().collect();
    let mut types: Vec<Option<(String, usize)>> = Vec::new();
    for p in &arrow.params {
        let Some(t) = param_type_ann(p) else {
            types.push(None);
            continue;
        };
        // TypeScript's `name.end`: swc widens typed patterns over their annotation, so walk back
        // from the annotation's colon over `?` and whitespace
        let mut ne = parsed.pos(t.span.lo);
        while ne > 0
            && (units[ne - 1] == b'?' as u16
                || char::from_u32(units[ne - 1] as u32).is_some_and(crate::text::is_js_space))
        {
            ne -= 1;
        }
        let te = parsed.pos(t.span.hi);
        types.push(Some((String::from_utf16_lossy(&units[ne..te]), start_offset + ne)));
        replace_source_range(&mut interp, start_offset + ne, start_offset + te);
    }
    out.t("const [");
    out.extend(interp);
    out.t(format!("] = {}({}({slot_var})", names::V_SLOT, names::NON_NULL));
    if types.iter().any(|t| t.is_some()) {
        out.t(", ");
        let loc = &exp.loc;
        let b = Boundary::start(out, Src::Template, loc.start.offset as usize, loc.end.offset as usize, features::VERIFICATION);
        out.t("(");
        for t in &types {
            match t {
                Some((text, offset)) => {
                    out.t("_");
                    out.seg(text.clone(), Src::Template, *offset, features::ALL);
                    out.t(", ");
                }
                None => out.t("_, "),
            }
        }
        out.t(if is_ts_lang(o.script_lang) { ") => [] as any" } else { ") => /** @type {any} */ ([])" });
        b.end(out);
    }
    out.t(format!("){EOL}"));
}

fn param_type_ann(p: &swc_core::ecma::ast::Pat) -> Option<&swc_core::ecma::ast::TsTypeAnn> {
    use swc_core::ecma::ast::Pat;
    match p {
        Pat::Ident(b) => b.type_ann.as_deref(),
        Pat::Object(o) => o.type_ann.as_deref(),
        Pat::Array(a) => a.type_ann.as_deref(),
        Pat::Rest(r) => r.type_ann.as_deref(),
        Pat::Assign(a) => param_type_ann(&a.left),
        _ => None,
    }
}

/// muggle-string's `replaceSourceRange(segments, 'template', start, end)` with no replacement: the
/// first mapped segment containing the range loses that part of its text.
pub fn replace_source_range(codes: &mut Vec<crate::code::Code>, start: usize, end: usize) {
    use crate::code::Code;
    for i in 0..codes.len() {
        let Code::Seg { text, src, offset, feat } = &codes[i] else { continue };
        if *src != Src::Template {
            continue;
        }
        let seg_start = *offset as usize;
        let t = Text::new(text);
        let seg_end = seg_start + t.len();
        if seg_start <= start && seg_end >= end {
            let (src, feat) = (*src, *feat);
            let mut inserts = Vec::new();
            if start > seg_start {
                inserts.push(Code::Seg { text: t.slice(0, start - seg_start).to_string(), src, offset: seg_start as u32, feat });
            }
            if end < seg_end {
                let trim = end - seg_start;
                inserts.push(Code::Seg { text: t.slice_from(trim).to_string(), src, offset: (seg_start + trim) as u32, feat });
            }
            codes.splice(i..=i, inserts);
            return;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// styleScopedClasses.ts

pub fn style_scoped_class_references(o: &TemplateOptions, out: &mut Out, node: NodeId) {
    let a = o.arena();
    for &p in &a.el(node).props {
        match a.node(p) {
            Node::Attribute(attr) if attr.name == "class" => {
                if let Some(v) = &attr.value {
                    let (text, start) = normalize_attribute_value(&v.loc.source, v.loc.start.offset as usize);
                    for (class_name, offset) in for_each_class_name(&text) {
                        style_scoped_class_reference(out, Src::Template, &class_name, start + offset, None);
                    }
                }
            }
            Node::Directive(d) => {
                let (Some(arg), Some(exp)) = (d.arg, d.exp) else { continue };
                let (Node::SimpleExpression(arg), Node::SimpleExpression(exp)) = (a.node(arg), a.node(exp)) else { continue };
                if arg.content != "class" {
                    continue;
                }
                let content = format!("({})", exp.content);
                let start_offset = exp.loc.start.offset as usize - 1;
                class_binding_references(out, &content, start_offset);
            }
            _ => {}
        }
    }
}

fn class_binding_references(out: &mut Out, content: &str, start_offset: usize) {
    use swc_core::ecma::ast::*;
    let parsed = ts_ast::parse_as(content, false, false);
    let units: Vec<u16> = content.encode_utf16().collect();
    let text = |s: &dyn swc_core::common::Spanned| {
        let (a, b) = parsed.range(s.span());
        (String::from_utf16_lossy(&units[a..b]), b)
    };
    // (cooked value, end offset) of each string literal, collected for after the walk
    let mut literals: Vec<(String, usize)> = Vec::new();
    let Some(Program::Script(s)) = &parsed.program else { return };

    fn string_like(e: &Expr) -> Option<(String, swc_core::common::Span)> {
        match e {
            Expr::Lit(Lit::Str(s)) => Some((s.value.to_string_lossy().into_owned(), s.span)),
            Expr::Tpl(t) if t.exprs.is_empty() => {
                let cooked = t.quasis.first().and_then(|q| q.cooked.as_ref()).map(|c| c.to_string_lossy().into_owned()).unwrap_or_default();
                Some((cooked, t.span))
            }
            _ => None,
        }
    }

    let walk_object = |obj: &ObjectLit, out: &mut Out, literals: &mut Vec<(String, usize)>| {
        for p in &obj.props {
            let PropOrSpread::Prop(p) = p else { continue };
            match p.as_ref() {
                Prop::KeyValue(kv) => match &kv.key {
                    PropName::Ident(id) => {
                        let (t, end) = text(id);
                        let start = end - len16(&t) + start_offset;
                        style_scoped_class_reference(out, Src::Template, &t, start, None);
                    }
                    PropName::Str(s) => {
                        let (_, end) = text(s);
                        literals.push((s.value.to_string_lossy().into_owned(), end));
                    }
                    PropName::Computed(c) => {
                        if let Some((v, span)) = string_like(&c.expr) {
                            literals.push((v, parsed.pos(span.hi)));
                        }
                    }
                    _ => {}
                },
                Prop::Assign(AssignProp { key: id, .. }) | Prop::Shorthand(id) => {
                    let (t, end) = text(id);
                    let start = end - len16(&t) + start_offset;
                    style_scoped_class_reference(out, Src::Template, &t, start, None);
                }
                _ => {}
            }
        }
    };

    for stmt in &s.body {
        let Stmt::Expr(es) = stmt else { continue };
        let Expr::Paren(paren) = es.expr.as_ref() else { continue };
        let e = paren.expr.as_ref();
        if let Some((v, span)) = string_like(e) {
            literals.push((v, parsed.pos(span.hi)));
        } else if let Expr::Array(arr) = e {
            for el in arr.elems.iter().flatten() {
                if el.spread.is_some() {
                    continue;
                }
                if let Some((v, span)) = string_like(&el.expr) {
                    literals.push((v, parsed.pos(span.hi)));
                } else if let Expr::Object(obj) = el.expr.as_ref() {
                    walk_object(obj, out, &mut literals);
                }
            }
        } else if let Expr::Object(obj) = e {
            walk_object(obj, out, &mut literals);
        }
    }
    for (value, end) in literals {
        // `literal.end - literal.text.length - 1`
        let start = (end + start_offset).wrapping_sub(len16(&value) + 1);
        for (class_name, offset) in for_each_class_name(&value) {
            style_scoped_class_reference(out, Src::Template, &class_name, start + offset, None);
        }
    }
}

fn for_each_class_name(content: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for name in content.split(' ') {
        out.push((name.to_string(), offset));
        offset += len16(name) + 1;
    }
    out
}

/// `generateStyleScopedClassReference(block, className, offset, fullStart)`
pub fn style_scoped_class_reference(out: &mut Out, src: Src, class_name: &str, offset: usize, full_start: Option<usize>) {
    if class_name.is_empty() {
        out.t(format!("/** @type {{{}['", names::T_STYLE_SCOPED_CLASSES));
        out.seg("", Src::Template, offset, features::COMPLETION);
        out.t(format!("']}} */{EOL}"));
        return;
    }
    out.t(format!("/** @type {{{}[", names::T_STYLE_SCOPED_CLASSES));
    let b = Boundary::start(out, src, full_start.unwrap_or(offset), offset + len16(class_name), features::NAVIGATION_AND_COMPLETION);
    out.t("'");
    escaped(out, class_name, src, offset, b.feat);
    out.t("'");
    b.end(out);
    out.t(format!("]}} */{EOL}"));
}

/// `getElementTagOffsets(node, template)`
pub fn element_tag_offsets(o: &TemplateOptions, node: NodeId) -> (usize, Option<usize>) {
    let el = o.arena().el(node);
    let start = el.loc.start.offset as usize;
    let first = o.text.index_of(&el.tag, start).unwrap_or(usize::MAX);
    if !el.is_self_closing && o.template.lang == "html" {
        // `node.loc.source.lastIndexOf(node.tag)`; the source is the template's text from `start`
        if let Some(last) = el.loc.source.rfind(el.tag.as_str()) {
            let abs = o.text.byte(start) + last;
            let end_tag = if o.text.s.is_char_boundary(abs) && abs <= o.text.s.len() {
                o.text.utf16(abs)
            } else {
                start + crate::text::len16(&el.loc.source[..last])
            };
            if first == usize::MAX || end_tag > first {
                return (first, Some(end_tag));
            }
        }
    }
    (first, None)
}

pub fn exp_of(a: &Arena, id: Option<NodeId>) -> Option<&vue_sfc::core::ast::SimpleExpressionNode> {
    match id.map(|i| a.node(i)) {
        Some(Node::SimpleExpression(e)) => Some(e),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vslot_names() {
        let full = "(data: { foo: string }) => {}";
        let parsed = ts_ast::parse_as(full, false, false);
        assert!(parsed.program.is_some());
        assert_eq!(vslot_binding_names(full, &parsed), vec!["data", "foo"]);
    }
}
