//! `element.ts`, `elementProps.ts`, `elementEvents.ts`, `elementDirectives.ts`, `slotOutlet.ts`

use indexmap::{IndexMap, IndexSet};
use vue_sfc::core::ast::{ConstantType, DirectiveNode, ElementType, Node, NodeId, SourceLocation, TextNode};

use super::context::{ComponentRef, Ctx, SlotInfo};
use super::{
    TemplateOptions, element_tag_offsets, exp_of, object_property, property_access, style_scoped_class_references,
    template_child, v_slot,
};
use crate::code::{Code, Feat, Shorthand, Src, Verification, codes_to_string, features};
use crate::codegen::script::export_declare_equal;
use crate::codegen::template::binding_references::should_identifier_skipped;
use crate::codegen::{
    Boundary, EOL, NL, Out, as_type, camelized, is_ts_lang, ref_brand_argument, string_literal_key, typed_var, unicode,
};
use crate::names;
use crate::options::ModelPropTag;
use crate::sfc::normalize_attribute_value;
use crate::shared::{camelize, capitalize, glob_match, hyphenate, hyphenate_attr, is_built_in_directive, is_identifier};
use crate::text::{Text, len16};
use crate::ts_ast;

fn off(p: &vue_sfc::core::ast::Position) -> usize {
    p.offset as usize
}

/// `start + delta` for JS offsets that may come from an `indexOf` of `-1`.
fn add(start: usize, delta: Option<usize>) -> usize {
    match delta {
        Some(d) => start + d,
        None => start.wrapping_sub(1),
    }
}

fn is_v_slot(a: &vue_sfc::core::ast::Arena, p: NodeId) -> bool {
    matches!(a.node(p), Node::Directive(d) if d.name == "slot")
}

// ---------------------------------------------------------------------------------------------
// element.ts

pub fn component(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId) {
    let a = o.arena();
    let el = a.el(node);
    let mut tag = el.tag.clone();
    let mut props = el.props.clone();
    let (mut start_tag_offset, mut end_tag_offset) = element_tag_offsets(o, node);
    let mut is_expression = false;
    let mut is_is_shorthand = false;

    if tag.contains('.') {
        is_expression = true;
    } else if tag == "component" {
        for &p in &el.props {
            let Node::Directive(d) = a.node(p) else { continue };
            if d.name != "bind" || d.arg.is_none_or(|x| &*a.node(x).loc().source != "is") {
                continue;
            }
            let Some(e) = exp_of(a, d.exp) else { continue };
            is_is_shorthand = a.node(d.arg.unwrap()).loc().end.offset == e.loc.end.offset;
            is_expression = true;
            tag = e.content.clone();
            start_tag_offset = off(&e.loc.start);
            end_tag_offset = None;
            props.retain(|&x| x != p);
            break;
        }
    }

    let component_var = ctx.internal_variable();

    if is_expression {
        out.t(format!("const {component_var} = "));
        let feat = if is_is_shorthand { features::WITHOUT_HIGHLIGHT_AND_COMPLETION } else { features::ALL };
        o.interpolate(ctx, out, feat, &tag, start_tag_offset, "(", ")", false);
        if let Some(end) = end_tag_offset {
            out.t(" || ");
            o.interpolate(ctx, out, features::WITHOUT_COMPLETION, &tag, end, "(", ")", false);
        }
        out.t(EOL);
    } else {
        let original_names: IndexSet<String> = [capitalize(&camelize(&tag)), camelize(&tag), tag.clone()].into_iter().collect();
        let matched = original_names.iter().find(|n| o.imported_components.contains(*n)).cloned();
        if let Some(m) = matched {
            // navigation & auto import support
            out.t(format!("const {component_var} = "));
            let mut first = m.chars();
            let name = format!("{}{}", first.next().map(String::from).unwrap_or_default(), skip_first(&tag));
            camelized(
                out,
                &name,
                Src::Template,
                start_tag_offset,
                features::WITHOUT_HIGHLIGHT_AND_COMPLETION.merge(features::IMPORT_COMPLETION_ONLY),
            );
            if let Some(end) = end_tag_offset {
                out.t(" || ");
                camelized(out, &name, Src::Template, end, features::WITHOUT_HIGHLIGHT_AND_COMPLETION);
            }
            out.t(EOL);
        } else {
            typed_var(out, "let", &component_var, o.script_lang, |out| {
                out.t(format!(
                    "{}<'{tag}', {}, {}",
                    names::T_WITH_COMPONENT,
                    names::T_LOCAL_COMPONENTS,
                    names::T_GLOBAL_COMPONENTS
                ));
                if original_names.contains(o.component_name) {
                    out.t(format!(", typeof {}", names::EXPORT));
                } else {
                    out.t(", void");
                }
                for name in &original_names {
                    out.t(format!(", '{name}'"));
                }
                out.t(">[");
                string_literal_key(
                    out,
                    &tag,
                    start_tag_offset,
                    features::SEMANTIC_WITHOUT_HIGHLIGHT.merge(features::VERIFICATION),
                );
                out.t("]");
            });

            if is_identifier(&camelize(&tag)) {
                // navigation support
                out.t("/** @ts-ignore @type {");
                for offset in [Some(start_tag_offset), end_tag_offset].into_iter().flatten() {
                    out.t(format!(" | typeof {}.", names::COMPONENTS));
                    camelized(out, &tag, Src::Template, offset, features::NAVIGATION);
                    let first = tag.chars().next().map(String::from).unwrap_or_default();
                    if first != first.to_uppercase() {
                        out.t(format!(" | typeof {}.", names::COMPONENTS));
                        camelized(out, &capitalize(&tag), Src::Template, offset, features::NAVIGATION);
                    }
                    if tag.contains('-') {
                        out.t(format!(" | typeof {}[", names::COMPONENTS));
                        string_literal_key(out, &tag, offset, features::NAVIGATION);
                        out.t("]");
                    }
                }
                out.t(format!("}} */{NL}"));
                // auto import support
                camelized(out, &tag, Src::Template, start_tag_offset, features::IMPORT_COMPLETION_ONLY);
                out.t(EOL);
            }
        }
    }

    let functional_var = ctx.internal_variable();
    let vnode_var = ctx.internal_variable();
    let ctx_var = ctx.internal_variable();
    let props_var = ctx.internal_variable();
    ctx.components.push(ComponentRef {
        ctx_var: ctx_var.clone(),
        props_var: props_var.clone(),
        ctx_used: false,
        props_used: false,
    });
    let comp = ctx.components.len() - 1;

    let mut failed = Vec::new();
    let mut prop_out = out.scratch();
    element_props(o, ctx, &mut prop_out, node, &props, Some(&mut failed));
    let prop_codes = prop_out.codes;
    let props_str = codes_to_string(&prop_codes);

    out.t(format!("// @ts-ignore{NL}"));
    out.t(format!(
        "const {functional_var} = {}({component_var}, new {component_var}({{{NL}",
        names::AS_FUNCTIONAL_COMPONENT0
    ));
    out.t(format!("// @ts-ignore{NL}"));
    out.t(props_str.replace('\n', "\n// @ts-ignore\n"));
    out.t(format!("}})){EOL}"));

    export_declare_equal(out, Src::Template, &vnode_var, off(&el.loc.start), off(&el.loc.end));
    out.t(functional_var.clone());

    if let Some((content, offset)) = ctx.generic() {
        let b = Boundary::start(out, Src::Template, offset, offset + len16(&content), features::VERIFICATION);
        out.t("<");
        out.seg(content, Src::Template, offset, features::ALL);
        out.t(">");
        b.end(out);
    }

    let should_inherit_attrs = has_v_bind_attrs(o, ctx, node);

    out.t("(");
    let b2 = Boundary::start(
        out,
        Src::Template,
        start_tag_offset,
        start_tag_offset + len16(&tag),
        if should_inherit_attrs && o.vue.check_required_fallthrough_attributes {
            Feat::EMPTY
        } else {
            features::VERIFICATION
        },
    );
    out.t("{");
    out.seg("", Src::Template, off(&el.loc.start), features::PROPS_COMPLETION);
    out.t(NL);
    out.extend(prop_codes);
    out.t("}");
    b2.end(out);
    out.t(format!(", ...{}({functional_var})){EOL}", names::FUNCTIONAL_COMPONENT_ARGS_REST));
    out.t(format!("void {vnode_var}{EOL}"));

    failed_expressions(o, ctx, out, &failed);
    element_events(o, ctx, out, node, &component_var, comp);
    element_directives(o, ctx, out, node);

    let template_ref = template_ref(o, node);
    let is_single_root =
        ctx.single_root_nodes.contains(&Some(node)) && !o.vue.fallthrough_component_names.contains(&hyphenate(&tag));
    let infer_root_el = ctx.dollar_vars.contains("$el") || o.vue.infer_component_dollar_el;
    let infer_template_refs =
        o.vue.infer_template_dollar_refs || o.vue.infer_component_dollar_refs || !o.setup_refs.is_empty();

    if (template_ref.is_some() && infer_template_refs) || (is_single_root && infer_root_el) {
        let instance_var = ctx.internal_variable();
        let cv = ctx.component_ctx_var(comp);
        typed_var(out, "var", &instance_var, o.script_lang, |out| {
            out.t(format!("Parameters<NonNullable<typeof {cv}['expose']>>[0]"));
        });
        if let (Some((name, offset)), true) = (&template_ref, infer_template_refs) {
            let mut type_exp = format!("typeof {} | null", ctx.hoist_variable(&instance_var));
            if ctx.in_v_for {
                type_exp = format!("({type_exp})[]");
            }
            ctx.add_template_ref(name, type_exp, *offset);
        }
        if is_single_root && infer_root_el {
            ctx.single_root_el_types.insert(format!("NonNullable<typeof {instance_var}>['$el']"));
        }
    }

    if should_inherit_attrs {
        if o.vue.check_required_fallthrough_attributes {
            let rests_var = ctx.internal_variable();
            let pv = ctx.component_props_var(comp);
            out.t(format!("var {rests_var} = {}({pv}, {{\n{props_str}}}){EOL}", names::OMIT));
            ctx.inherited_attr_vars.insert(rests_var);
        } else {
            let pv = ctx.component_props_var(comp);
            ctx.inherited_attr_vars.insert(pv);
        }
    }

    style_scoped_class_references(o, out, node);

    let slot_dir = el.props.iter().copied().find(|&p| is_v_slot(a, p));
    if slot_dir.is_some() || !el.children.is_empty() {
        let cv = ctx.component_ctx_var(comp);
        v_slot(o, ctx, out, node, slot_dir, &cv);
    }

    let c = ctx.components.pop().unwrap();
    if c.ctx_used {
        typed_var(out, "var", &ctx_var, o.script_lang, |out| {
            out.t(format!("{}<typeof {component_var}, typeof {vnode_var}>", names::T_EXTRACT_COMPONENT_CONTEXT));
        });
    }
    if c.props_used {
        typed_var(out, "var", &props_var, o.script_lang, |out| {
            out.t(format!("{}<typeof {component_var}, typeof {vnode_var}>", names::T_EXTRACT_COMPONENT_PROPS));
        });
    }
}

fn skip_first(s: &str) -> &str {
    let mut c = s.chars();
    c.next();
    c.as_str()
}

pub fn element(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId) {
    let a = o.arena();
    let el = a.el(node);
    let (start_tag_offset, end_tag_offset) = element_tag_offsets(o, node);
    let mut failed = Vec::new();

    out.t(format!("{}({}", names::AS_FUNCTIONAL_ELEMENT0, names::INTRINSICS));
    property_access(o, ctx, out, &el.tag, start_tag_offset, features::WITHOUT_HIGHLIGHT_AND_COMPLETION);
    if let Some(end) = end_tag_offset {
        out.t(", ");
        out.t(names::INTRINSICS);
        property_access(o, ctx, out, &el.tag, end, features::WITHOUT_HIGHLIGHT_AND_COMPLETION);
    }
    out.t(")(");
    let b = Boundary::start(
        out,
        Src::Template,
        start_tag_offset,
        start_tag_offset + len16(&el.tag),
        features::VERIFICATION,
    );
    out.t(format!("{{{NL}"));
    element_props(o, ctx, out, node, &el.props, Some(&mut failed));
    out.t("}");
    b.end(out);
    out.t(format!("){EOL}"));

    failed_expressions(o, ctx, out, &failed);
    element_directives(o, ctx, out, node);

    if let Some((name, offset)) = template_ref(o, node) {
        let mut type_exp = format!("{}['{}']", names::T_ELEMENTS, el.tag);
        if ctx.in_v_for {
            type_exp.push_str("[]");
        }
        ctx.add_template_ref(&name, type_exp, offset);
    }
    if ctx.single_root_nodes.contains(&Some(node)) && (ctx.dollar_vars.contains("$el") || o.vue.infer_component_dollar_el)
    {
        ctx.single_root_el_types.insert(format!("{}['{}']", names::T_ELEMENTS, el.tag));
    }

    if has_v_bind_attrs(o, ctx, node) {
        ctx.inherited_attr_vars.insert(format!("{}.{}", names::INTRINSICS, el.tag));
    }

    style_scoped_class_references(o, out, node);

    ctx.diagnostic_directive_end(out);
    for &child in &el.children {
        template_child(o, ctx, out, child, true, false);
    }
}

pub fn fragment(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId) {
    let a = o.arena();
    let el = a.el(node);
    let (start_tag_offset, _) = element_tag_offsets(o, node);
    let mut failed = Vec::new();

    // special case for <template v-for="..." :key="..." />
    if !el.props.is_empty() {
        out.t(format!("{}({}.template)(", names::AS_FUNCTIONAL_ELEMENT0, names::INTRINSICS));
        let b = Boundary::start(
            out,
            Src::Template,
            start_tag_offset,
            start_tag_offset + len16(&el.tag),
            features::VERIFICATION,
        );
        out.t(format!("{{{NL}"));
        element_props(o, ctx, out, node, &el.props, Some(&mut failed));
        out.t("}");
        b.end(out);
        out.t(format!("){EOL}"));
        failed_expressions(o, ctx, out, &failed);
    }

    ctx.diagnostic_directive_end(out);
    for &child in &el.children {
        template_child(o, ctx, out, child, true, false);
    }
}

pub struct Failed {
    node: NodeId,
    prefix: &'static str,
    suffix: &'static str,
}

fn failed_expressions(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, failed: &[Failed]) {
    let a = o.arena();
    for f in failed {
        let loc = a.node(f.node).loc();
        o.interpolate(ctx, out, features::ALL, &loc.source, off(&loc.start), f.prefix, f.suffix, false);
        out.t(EOL);
    }
}

fn template_ref(o: &TemplateOptions, node: NodeId) -> Option<(String, usize)> {
    let a = o.arena();
    for &p in &a.el(node).props {
        if let Node::Attribute(attr) = a.node(p) {
            if attr.name == "ref" {
                if let Some(v) = &attr.value {
                    return Some(normalize_attribute_value(&v.loc.source, off(&v.loc.start)));
                }
            }
        }
    }
    None
}

fn has_v_bind_attrs(o: &TemplateOptions, ctx: &Ctx, node: NodeId) -> bool {
    let a = o.arena();
    o.vue.fallthrough_attributes
        && ((o.inherit_attrs && ctx.single_root_nodes.contains(&Some(node)))
            || a.el(node).props.iter().any(|&p| {
                matches!(a.node(p), Node::Directive(d)
                    if d.name == "bind" && d.exp.is_some_and(|e| &*a.node(e).loc().source == "$attrs"))
            }))
}

// ---------------------------------------------------------------------------------------------
// elementProps.ts

pub fn element_props(
    o: &TemplateOptions,
    ctx: &mut Ctx,
    out: &mut Out,
    node: NodeId,
    props: &[NodeId],
    mut failed: Option<&mut Vec<Failed>>,
) {
    let a = o.arena();
    let el = a.el(node);
    let is_component = el.tag_type == ElementType::Component;
    let mut fail = |f: Failed| {
        if let Some(v) = failed.as_deref_mut() {
            v.push(f);
        }
    };

    for &p in props {
        let Node::Directive(d) = a.node(p) else { continue };
        if d.name != "on" {
            continue;
        }
        let arg = exp_of(a, d.arg);
        let exp = exp_of(a, d.exp);
        if let Some(arg) = arg.filter(|x| !x.loc.source.starts_with('[') && !x.loc.source.ends_with(']')) {
            if !is_component {
                out.t("...{ ");
                event_arg(out, &arg.loc.source, off(&arg.loc.start), "on", None);
                out.t(": ");
                event_expression(o, ctx, out, d);
                out.t("},");
            } else {
                out.t(format!(
                    "...{{ '{}': {} }},",
                    camelize(&format!("on-{}", &*arg.loc.source)),
                    as_type("any", o.script_lang)
                ));
            }
            out.t(NL);
        } else if arg.is_some_and(|x| x.loc.source.starts_with('[') && x.loc.source.ends_with(']')) && exp.is_some() {
            fail(Failed { node: d.arg.unwrap(), prefix: "(", suffix: ")" });
            fail(Failed { node: d.exp.unwrap(), prefix: "() => {", suffix: "}" });
        } else if d.arg.is_none() && exp.is_some() {
            fail(Failed { node: d.exp.unwrap(), prefix: "(", suffix: ")" });
        }
    }

    for &p in props {
        match a.node(p) {
            Node::Directive(d)
                if ((d.name == "bind" && exp_of(a, d.arg).is_some()) || d.name == "model")
                    && (d.exp.is_none() || exp_of(a, d.exp).is_some()) =>
            {
                let prop_name = match exp_of(a, d.arg) {
                    Some(arg) => Some(if arg.const_type == ConstantType::CanStringify {
                        arg.content.clone()
                    } else {
                        arg.loc.source.to_string()
                    }),
                    None => model_prop_name(o, node),
                };
                let Some(mut prop_name) =
                    prop_name.filter(|n| !o.vue.data_attributes.iter().any(|pat| glob_match(n, pat)))
                else {
                    if let Some(e) = exp_of(a, d.exp) {
                        if e.const_type != ConstantType::CanStringify {
                            fail(Failed { node: d.exp.unwrap(), prefix: "(", suffix: ")" });
                        }
                    }
                    continue;
                };

                if d.name == "bind"
                    && d.modifiers.iter().any(|&m| matches!(a.exp(m).content.as_str(), "prop" | "attr"))
                {
                    prop_name = skip_first(&prop_name).to_string();
                }

                let should_spread = prop_name == "style" || prop_name == "class";
                let should_camelize = get_should_camelize(o, node, p, &prop_name);
                let feat = props_feat();

                if should_spread {
                    out.t("...{ ");
                }
                let b = Boundary::start(out, Src::Template, off(&d.loc.start), off(&d.loc.end), features::VERIFICATION);
                if let Some(arg) = d.arg {
                    let arg_start = off(&a.node(arg).loc().start);
                    object_property(o, ctx, out, &prop_name, arg_start, feat, should_camelize, false);
                } else {
                    let start = off(&d.loc.start);
                    let b2 = Boundary::start(
                        out,
                        Src::Template,
                        start,
                        start + "v-model".len(),
                        features::WITHOUT_HIGHLIGHT_AND_COMPLETION,
                    );
                    out.t(prop_name.clone());
                    b2.end(out);
                }
                out.t(": ");
                let arg_loc: &SourceLocation = d.arg.map(|x| a.node(x).loc()).unwrap_or(&d.loc);
                let b3 = Boundary::start(out, Src::Template, off(&arg_loc.start), off(&arg_loc.end), features::VERIFICATION);
                prop_exp(o, ctx, out, d, d.exp);
                b3.end(out);
                b.end(out);
                if should_spread {
                    out.t(" }");
                }
                out.t(format!(",{NL}"));

                if is_component && d.name == "model" && !d.modifiers.is_empty() {
                    let property_name = match exp_of(a, d.arg) {
                        Some(arg) if !arg.is_static => {
                            format!("[{}(`${{{}}}Modifiers`)]", names::TRY_AS_CONSTANT, arg.content)
                        }
                        Some(_) => format!("{}Modifiers", camelize(&prop_name)),
                        None => "modelModifiers".to_string(),
                    };
                    modifiers(o, ctx, out, d, &property_name, true);
                    out.t(NL);
                }
            }
            Node::Attribute(attr) => {
                if o.vue.data_attributes.iter().any(|pat| glob_match(&attr.name, pat)) {
                    continue;
                }
                let should_spread = attr.name == "style" || attr.name == "class";
                let should_camelize = get_should_camelize(o, node, p, &attr.name);
                let feat = props_feat();
                let start = off(&attr.loc.start);

                if should_spread {
                    out.t("...{ ");
                }
                let b = Boundary::start(out, Src::Template, start, off(&attr.loc.end), features::VERIFICATION);
                let prefix = o.text.slice(start, start + 1);
                if prefix == "." || prefix == "#" {
                    // Pug shorthand syntax
                    for ch in attr.name.chars() {
                        out.seg(ch.to_string(), Src::Template, start, feat);
                    }
                } else {
                    object_property(o, ctx, out, &attr.name, start, feat, should_camelize, false);
                }
                out.t(": ");
                if attr.name == "style" {
                    out.t("{}");
                } else if let Some(v) = &attr.value {
                    attr_value(out, v, features::WITHOUT_NAVIGATION);
                } else {
                    out.t("true");
                }
                b.end(out);
                if should_spread {
                    out.t(" }");
                }
                out.t(format!(",{NL}"));
            }
            Node::Directive(d) if d.name == "bind" && d.arg.is_none() && exp_of(a, d.exp).is_some() => {
                let e = exp_of(a, d.exp).unwrap();
                if &*e.loc.source == "$attrs" {
                    fail(Failed { node: d.exp.unwrap(), prefix: "(", suffix: ")" });
                } else {
                    let b = Boundary::start(out, Src::Template, off(&e.loc.start), off(&e.loc.end), features::VERIFICATION);
                    out.t("...");
                    prop_exp(o, ctx, out, d, d.exp);
                    b.end(out);
                    out.t(format!(",{NL}"));
                }
            }
            _ => {}
        }
    }
}

pub fn prop_exp(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, prop: &DirectiveNode, exp: Option<NodeId>) {
    let a = o.arena();
    let Some(exp) = exp else {
        out.t("{}");
        return;
    };
    let e = a.node(exp);
    let loc = e.loc();
    let start = off(&loc.start);
    let arg_start = prop.arg.map(|x| a.node(x).loc().start.offset);
    let exp_start = prop.exp.map(|x| a.node(x).loc().start.offset);
    if arg_start != exp_start {
        o.interpolate(ctx, out, features::ALL, &loc.source, start, "(", ")", false);
        return;
    }
    let var_name = camelize(&loc.source);
    if !is_identifier(&var_name) {
        return;
    }
    let feat = Feat { shorthand: Shorthand::Html, ..features::WITHOUT_HIGHLIGHT_AND_COMPLETION };
    let codes = |out: &mut Out| camelized(out, &loc.source, Src::Template, start, feat);

    // Keep in sync with the access strategy in interpolation.rs.
    if o.destructured_props.contains(&var_name) || o.imported_components.contains(&var_name) {
        codes(out);
    } else if should_identifier_skipped(&ctx.scopes, &var_name) {
        codes(out);
    } else if o.setup_refs.contains(&var_name) {
        codes(out);
        out.seg(".value", Src::Template, start, features::VERIFICATION);
    } else if o.setup_bindings.contains(&var_name) {
        ctx.access_variable(Src::Template, &var_name, Some(start), false);
        if o.dot_value_bindings.contains(&var_name) {
            codes(out);
            out.seg(".value", Src::Template, start, features::VERIFICATION);
        } else {
            out.t(format!("{}(", names::UNWRAP));
            codes(out);
            out.t(format!(", {})", ref_brand_argument(&o.vue.lib, o.script_lang)));
        }
    } else {
        ctx.access_variable(Src::Template, &var_name, Some(start), false);
        out.t(names::CTX);
        out.t(".");
        codes(out);
    }
}

fn attr_value(out: &mut Out, v: &TextNode, feat: Feat) {
    let quote = if v.loc.source.starts_with('\'') { "'" } else { "\"" };
    let (content, offset) = normalize_attribute_value(&v.loc.source, off(&v.loc.start));
    out.t(quote);
    unicode(out, &content, offset, feat);
    out.t(quote);
}

fn get_should_camelize(o: &TemplateOptions, node: NodeId, prop: NodeId, prop_name: &str) -> bool {
    let a = o.arena();
    let tag_type = a.el(node).tag_type;
    let static_arg = match a.node(prop) {
        Node::Directive(d) => exp_of(a, d.arg).is_some_and(|e| e.is_static),
        _ => true,
    };
    matches!(tag_type, ElementType::Component | ElementType::Slot)
        && static_arg
        && hyphenate_attr(prop_name) == prop_name
        && (tag_type == ElementType::Slot || !o.vue.html_attributes.iter().any(|p| glob_match(prop_name, p)))
}

fn props_feat() -> Feat {
    Feat { props_completion: true, ..features::WITHOUT_HIGHLIGHT_AND_COMPLETION.merge(features::VERIFICATION) }
}

fn model_prop_name(o: &TemplateOptions, node: NodeId) -> Option<String> {
    let a = o.arena();
    let el = a.el(node);
    let name_of = |n: &String| if n.is_empty() { None } else { Some(n.clone()) };
    for (model_name, tags) in &o.vue.experimental_model_prop_name {
        for (tag, val) in tags {
            if el.tag != *tag && el.tag != hyphenate(tag) {
                continue;
            }
            let ModelPropTag::Attrs(arr) = val else { continue };
            for attrs in arr {
                let all_match = attrs.iter().all(|(attr, expected)| {
                    el.props.iter().find_map(|&p| match a.node(p) {
                        Node::Attribute(at) if at.name == *attr => Some(at),
                        _ => None,
                    })
                    .is_some_and(|at| at.value.as_ref().is_some_and(|v| v.content == *expected))
                });
                if all_match {
                    return name_of(model_name);
                }
            }
        }
    }
    for (model_name, tags) in &o.vue.experimental_model_prop_name {
        for (tag, val) in tags {
            if (el.tag == *tag || el.tag == hyphenate(tag)) && *val == ModelPropTag::Bool(true) {
                return name_of(model_name);
            }
        }
    }
    Some("modelValue".to_string())
}

// ---------------------------------------------------------------------------------------------
// elementEvents.ts

struct EventDef {
    prop_prefix: &'static str,
    emit_prefix: &'static str,
    prop_name: String,
    emit_name: String,
    items: Vec<(NodeId, String, Option<usize>)>,
}

fn drop_last(s: &str) -> &str {
    &s[..s.len().saturating_sub(1)]
}

fn element_events(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId, component_var: &str, comp: usize) {
    let a = o.arena();
    let mut defs: IndexMap<String, EventDef> = IndexMap::new();
    for &p in &a.el(node).props {
        let Node::Directive(d) = a.node(p) else { continue };
        let static_arg = exp_of(a, d.arg).is_some_and(|e| e.is_static);
        if !((d.name == "on" && static_arg) || (d.name == "model" && (d.arg.is_none() || static_arg))) {
            continue;
        }
        let arg = d.arg.map(|x| a.node(x).loc());
        let mut source = arg.map(|l| l.source.to_string()).unwrap_or_else(|| "model-value".to_string());
        let mut offset = arg.map(|l| off(&l.start));
        let (mut prop_prefix, mut emit_prefix) = ("on-", "");
        if d.name == "model" {
            prop_prefix = "onUpdate:";
            emit_prefix = "update:";
        } else if let Some(rest) = source.strip_prefix("vue:") {
            source = rest.to_string();
            offset = offset.map(|o| o + "vue:".len());
            prop_prefix = "onVnode-";
            emit_prefix = "vnode-";
        }
        let prop_name = camelize(&format!("{prop_prefix}{source}"));
        let emit_name = format!("{emit_prefix}{source}");
        let mut key = format!("{}+{prop_name}", d.name);
        for &m in &d.modifiers {
            key.push('+');
            key.push_str(&a.exp(m).content);
        }
        defs.entry(key)
            .or_insert_with(|| EventDef { prop_prefix, emit_prefix, prop_name, emit_name, items: Vec::new() })
            .items
            .push((p, source, offset));
    }
    if defs.is_empty() {
        return;
    }

    let emits_var = ctx.internal_variable();
    let cv = ctx.component_ctx_var(comp);
    typed_var(out, "let", &emits_var, o.script_lang, |out| {
        out.t(format!("{}<typeof {component_var}, typeof {cv}.emit>", names::T_RESOLVE_EMITS));
    });

    for def in defs.values() {
        let event_var = ctx.internal_variable();
        let pv = ctx.component_props_var(comp);
        let event_type = format!(
            "Partial<{}<typeof {pv}, typeof {emits_var}, '{}', '{}', '{}'>>",
            names::T_RESOLVE_EVENT,
            def.prop_name,
            def.emit_name,
            camelize(&def.emit_name)
        );
        if is_ts_lang(o.script_lang) {
            out.t(format!("const {event_var}: {event_type} = {{{NL}"));
        } else {
            out.t(format!("/** @type {{{event_type}}} */{NL}const {event_var} = {{{NL}"));
        }
        for (p, source, offset) in &def.items {
            let d = a.dir(*p);
            if d.name == "on" {
                let offset = offset.unwrap_or(0);
                out.t(format!("/** @type {{typeof {emits_var}."));
                event_arg(out, source, offset, drop_last(def.emit_prefix), Some(features::NAVIGATION));
                out.t(format!("}} */{NL}"));
                event_arg(out, source, offset, drop_last(def.prop_prefix), None);
                out.t(": ");
                event_expression(o, ctx, out, d);
            } else {
                out.t(format!("'{}': ", def.prop_name));
                model_event_expression(o, ctx, out, d);
            }
            out.t(format!(",{NL}"));
        }
        out.t(format!("}}{EOL}"));
        out.t(format!("void {event_var}{EOL}"));
    }
}

pub fn event_arg(out: &mut Out, name: &str, start: usize, directive: &str, feat: Option<Feat>) {
    let feat = feat.unwrap_or(
        features::SEMANTIC_WITHOUT_HIGHLIGHT.merge(features::NAVIGATION_WITHOUT_RENAME).merge(features::VERIFICATION),
    );
    let name = if directive.is_empty() { name.to_string() } else { capitalize(name) };
    let b = Boundary::start(out, Src::Template, start, start + len16(&name), feat);
    if is_identifier(&camelize(&name)) {
        out.t(directive);
        camelized(out, &name, Src::Template, start, b.feat);
        // the reference leaves this boundary open
    } else {
        out.t("'");
        out.t(directive);
        camelized(out, &name, Src::Template, start, b.feat);
        out.t("'");
        b.end(out);
    }
}

pub fn event_expression(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, prop: &DirectiveNode) {
    let a = o.arena();
    let Some(e) = exp_of(a, prop.exp) else {
        out.t("() => {}");
        return;
    };
    let parsed = ts_ast::parse_as(&e.content, false, false);
    let shape = statement_shape(&parsed, &e.content);
    let start = off(&e.loc.start);
    if !shape.is_compound {
        o.interpolate(ctx, out, features::ALL, &e.content, start, "(", ")", false);
        return;
    }
    let scope = ctx.scope();
    ctx.declare(scope, ["$event".to_string()]);
    let mark = ctx.access_log.len();
    let mut codes = out.scratch();
    o.interpolate(ctx, &mut codes, features::ALL, &e.content, start, "", "", false);

    out.t(format!("// @ts-ignore{NL}"));
    out.t(format!("(...[$event]) => {{{NL}"));
    // consume $event to avoid TS6133 when the expression doesn't use it
    out.t(format!("void $event{EOL}"));
    reasserts(o, ctx, out, mark);
    ctx.generate_condition_guards(out);
    if shape.is_single_expression {
        out.t("return (");
        out.extend(codes.codes);
        out.t(")");
    } else {
        out.extend(codes.codes);
    }
    out.t(EOL);
    ctx.end_scope(out);
    out.t("}");
}

fn model_event_expression(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, prop: &DirectiveNode) {
    let a = o.arena();
    let Some(e) = exp_of(a, prop.exp) else {
        out.t("() => {}");
        return;
    };
    let mark = ctx.access_log.len();
    let mut codes = out.scratch();
    o.interpolate(ctx, &mut codes, features::VERIFICATION, &e.content, off(&e.loc.start), "", "", true);
    out.t(format!("// @ts-ignore{NL}"));
    out.t(format!("(...[$event]) => {{{NL}"));
    reasserts(o, ctx, out, mark);
    ctx.generate_condition_guards(out);
    out.extend(codes.codes);
    out.t(format!(" = $event{EOL}"));
    out.t("}");
}

/// Assertion narrowing does not flow into nested closures for imports and `let`/`var` bindings
/// (TS limitation), so re-assert those bindings at closure tops.
fn reasserts(o: &TemplateOptions, ctx: &Ctx, out: &mut Out, mark: usize) {
    let mut names_: IndexSet<&str> = IndexSet::new();
    let condition_names = ctx.conditions.iter().flat_map(|c| c.accesses.iter());
    for name in ctx.access_log[mark..].iter().chain(condition_names) {
        if o.reassert_bindings.contains(name) && !ctx.scopes.iter().any(|s| s.contains(name)) {
            names_.insert(name);
        }
    }
    for name in names_ {
        out.t(format!(
            "{}({name}, {}){EOL}",
            names::WITH_DOT_VALUE,
            ref_brand_argument(&o.vue.lib, o.script_lang)
        ));
    }
}

struct StatementShape {
    is_compound: bool,
    is_single_expression: bool,
}

/// `isCompoundExpression` / `isSingleExpression`
fn statement_shape(parsed: &ts_ast::Parsed, text: &str) -> StatementShape {
    use swc_core::common::Spanned;
    use swc_core::ecma::ast::{Decl, Expr, OptChainBase, Program, Stmt};
    let stmts: &[Stmt] = match &parsed.program {
        Some(Program::Script(s)) => &s.body,
        _ => {
            // `function (e) { … }`: TypeScript recovers a nameless function declaration, which
            // is not compound; swc gives up on it
            let wrapped = format!("({text})");
            let p = ts_ast::parse_as(&wrapped, false, false);
            let is_fn = matches!(&p.program, Some(Program::Script(s)) if s.body.len() == 1
                && matches!(&s.body[0], Stmt::Expr(es) if matches!(&*es.expr, Expr::Paren(p) if matches!(&*p.expr, Expr::Fn(_)))));
            return StatementShape { is_compound: !is_fn, is_single_expression: false };
        }
    };
    if stmts.is_empty() {
        return StatementShape { is_compound: false, is_single_expression: false };
    }
    let mut shape = StatementShape { is_compound: true, is_single_expression: false };
    // `ast.text[ast.endOfFileToken.pos - 1] !== ';'`: the last token is not a semicolon
    let last_end = parsed.end(&stmts[stmts.len() - 1].span());
    let ends_with_semi = last_end > 0 && Text::new(text).code_at(last_end - 1) == Some(b';' as u16);
    if stmts.len() == 1 && !ends_with_semi {
        match &stmts[0] {
            Stmt::Expr(es) => {
                shape.is_single_expression = true;
                let node = crate::parsers::unwrap_expression(&es.expr);
                let is_access = match node {
                    Expr::Arrow(_) | Expr::Ident(_) | Expr::Member(_) | Expr::SuperProp(_) => true,
                    Expr::OptChain(c) => matches!(&*c.base, OptChainBase::Member(_)),
                    _ => false,
                };
                if is_access {
                    shape.is_compound = false;
                }
            }
            Stmt::Decl(Decl::Fn(_)) => shape.is_compound = false,
            _ => {}
        }
    }
    shape
}

// ---------------------------------------------------------------------------------------------
// elementDirectives.ts

fn element_directives(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId) {
    let a = o.arena();
    for &p in &a.el(node).props {
        let Node::Directive(d) = a.node(p) else { continue };
        if matches!(d.name.as_str(), "slot" | "on" | "model" | "bind") {
            continue;
        }
        let b = Boundary::start(out, Src::Template, off(&d.loc.start), off(&d.loc.end), features::VERIFICATION);
        if o.is_vapor && !is_built_in_directive(&d.name) {
            // vapor custom directives receive a value getter instead of a vdom binding object
            directive_identifier(ctx, out, d);
            out.t(format!("({}(null), ", names::NON_NULL));
            directive_value(o, ctx, out, d, false);
            directive_arg(o, ctx, out, d, false);
            modifiers(o, ctx, out, d, "modifiers", false);
            out.t(")");
        } else {
            out.t(format!("{}(", names::AS_FUNCTIONAL_DIRECTIVE));
            directive_identifier(ctx, out, d);
            out.t(format!(
                ", {})({}(null), {{ ...{}, ",
                as_type(&format!("import('{}').ObjectDirective", o.vue.lib), o.script_lang),
                names::NON_NULL,
                names::DIRECTIVE_BINDING_REST_FIELDS
            ));
            directive_arg(o, ctx, out, d, true);
            modifiers(o, ctx, out, d, "modifiers", true);
            directive_value(o, ctx, out, d, true);
            out.t(format!("}},{NL}// @ts-ignore{NL}null, null)"));
        }
        b.end(out);
        out.t(EOL);
    }
}

fn directive_identifier(ctx: &mut Ctx, out: &mut Out, d: &DirectiveNode) {
    let raw_name = format!("v-{}", d.name);
    let start = off(&d.loc.start);
    let b = Boundary::start(out, Src::Template, start, start + len16(&raw_name), features::VERIFICATION);
    out.t(names::DIRECTIVES);
    out.t(".");
    let builtin = is_built_in_directive(&d.name);
    let feat = Feat {
        verification: if builtin { Verification::False } else { Verification::True },
        ..features::WITHOUT_HIGHLIGHT_AND_COMPLETION
    };
    camelized(out, &raw_name, Src::Template, start, feat);
    if !builtin {
        ctx.access_variable(Src::Template, &camelize(&raw_name), Some(start), false);
    }
    b.end(out);
}

fn directive_arg(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, d: &DirectiveNode, as_binding_property: bool) {
    let a = o.arena();
    if let Some(arg) = exp_of(a, d.arg) {
        let start = add(off(&arg.loc.start), Text::new(&arg.loc.source).index_of(&arg.content, 0));
        if as_binding_property {
            let b = Boundary::start(out, Src::Template, start, start + len16(&arg.content), features::VERIFICATION);
            out.t("arg");
            b.end(out);
            out.t(": ");
        }
        if arg.is_static {
            string_literal_key(out, &arg.content, start, features::ALL);
        } else {
            o.interpolate(ctx, out, features::ALL, &arg.content, start, "(", ")", false);
        }
        out.t(", ");
    } else if !as_binding_property {
        out.t("undefined, ");
    }
}

pub fn modifiers(
    o: &TemplateOptions,
    ctx: &mut Ctx,
    out: &mut Out,
    d: &DirectiveNode,
    property_name: &str,
    as_binding_property: bool,
) {
    let a = o.arena();
    if d.modifiers.is_empty() {
        if !as_binding_property {
            out.t("undefined, ");
        }
        return;
    }
    if as_binding_property {
        let first = off(&a.exp(d.modifiers[0]).loc.start).wrapping_sub(1);
        let last = off(&a.exp(*d.modifiers.last().unwrap()).loc.end);
        let b = Boundary::start(out, Src::Template, first, last, features::VERIFICATION);
        out.t(property_name);
        b.end(out);
        out.t(": ");
    }
    out.t("{ ");
    for &m in &d.modifiers {
        let m = a.exp(m);
        object_property(o, ctx, out, &m.content, off(&m.loc.start), features::WITHOUT_HIGHLIGHT, false, false);
        out.t(": true, ");
    }
    out.t("}, ");
}

fn directive_value(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, d: &DirectiveNode, as_binding_property: bool) {
    let a = o.arena();
    if let Some(e) = exp_of(a, d.exp) {
        let b = Boundary::start(out, Src::Template, off(&e.loc.start), off(&e.loc.end), features::VERIFICATION);
        if as_binding_property {
            out.t("value");
            b.end(out);
            out.t(": ");
            prop_exp(o, ctx, out, d, d.exp);
        } else {
            out.t("() => ");
            prop_exp(o, ctx, out, d, d.exp);
            b.end(out);
        }
        out.t(", ");
    } else if !as_binding_property {
        out.t("undefined, ");
    }
}

// ---------------------------------------------------------------------------------------------
// slotOutlet.ts

pub fn slot_outlet(o: &TemplateOptions, ctx: &mut Ctx, out: &mut Out, node: NodeId) {
    let a = o.arena();
    let el = a.el(node);
    let (start_tag_offset, _) = element_tag_offsets(o, node);
    let start_tag_end_offset = start_tag_offset + len16(&el.tag);
    let props_var = ctx.internal_variable();
    let name_prop = el.props.iter().copied().find(|&p| match a.node(p) {
        Node::Attribute(at) => at.name == "name",
        Node::Directive(d) => d.name == "bind" && exp_of(a, d.arg).is_some_and(|e| e.content == "name"),
        _ => false,
    });
    let other_props: Vec<NodeId> = el.props.iter().copied().filter(|&p| Some(p) != name_prop).collect();

    if o.has_define_slots {
        out.t(format!("{}(", names::AS_FUNCTIONAL_SLOT));
        let slots_name = o.slots_assign_name.unwrap_or(names::SLOTS);
        if let Some(np) = name_prop {
            enum Codes {
                // generated lazily, after the boundary
                Access(String, usize),
                Ready(Vec<Code>),
                Default,
            }
            let codes = match a.node(np) {
                Node::Attribute(at) if at.value.is_some() => {
                    let v = at.value.as_ref().unwrap();
                    let (content, offset) = normalize_attribute_value(&v.loc.source, off(&v.loc.start));
                    Codes::Access(content, offset)
                }
                Node::Directive(d) if exp_of(a, d.exp).is_some() => {
                    let mut s = out.scratch();
                    s.t("[");
                    prop_exp(o, ctx, &mut s, d, d.exp);
                    s.t("]");
                    Codes::Ready(s.codes)
                }
                _ => Codes::Default,
            };
            let loc = a.node(np).loc();
            let b = Boundary::start(out, Src::Template, off(&loc.start), off(&loc.end), features::VERIFICATION);
            out.t(slots_name);
            match codes {
                Codes::Access(content, offset) => {
                    property_access(o, ctx, out, &content, offset, features::NAVIGATION_AND_VERIFICATION)
                }
                Codes::Ready(codes) => out.extend(codes),
                Codes::Default => out.t("['default']"),
            }
            b.end(out);
        } else {
            let b = Boundary::start(out, Src::Template, start_tag_offset, start_tag_end_offset, features::VERIFICATION);
            out.t(format!("{slots_name}["));
            let b2 = Boundary::start(out, Src::Template, start_tag_offset, start_tag_end_offset, features::VERIFICATION);
            out.t("'default'");
            b2.end(out);
            out.t("]");
            b.end(out);
        }
        out.t(")(");
        let b = Boundary::start(out, Src::Template, start_tag_offset, start_tag_end_offset, features::VERIFICATION);
        out.t(format!("{{{NL}"));
        element_props(o, ctx, out, node, &other_props, None);
        out.t("}");
        b.end(out);
        out.t(format!("){EOL}"));
    } else {
        out.t(format!("var {props_var} = {{{NL}"));
        element_props(o, ctx, out, node, &other_props, None);
        out.t(format!("}}{EOL}"));

        match name_prop.map(|np| a.node(np)) {
            Some(Node::Attribute(at)) if at.value.is_some() => {
                let v = at.value.as_ref().unwrap();
                let idx = Text::new(&at.loc.source).index_of(&v.content, len16(&at.name));
                let offset = add(off(&at.loc.start), idx);
                let props_var = ctx.hoist_variable(&props_var);
                ctx.slots.push(SlotInfo {
                    name: v.content.clone(),
                    offset: Some(offset),
                    tag_range: (start_tag_offset, start_tag_offset + len16(&el.tag)),
                    props_var,
                });
            }
            Some(Node::Directive(d)) if exp_of(a, d.exp).is_some() => {
                let e = exp_of(a, d.exp).unwrap();
                let is_short_hand = d.arg.map(|x| a.node(x).loc().start.offset) == Some(e.loc.start.offset);
                let exp_var = ctx.internal_variable();
                out.t(format!("var {exp_var} = {}(", names::TRY_AS_CONSTANT));
                let feat = if is_short_hand { features::WITHOUT_HIGHLIGHT_AND_COMPLETION } else { features::ALL };
                o.interpolate(ctx, out, feat, &e.content, off(&e.loc.start), "", "", false);
                out.t(format!("){EOL}"));
                let exp_var = ctx.hoist_variable(&exp_var);
                let props_var = ctx.hoist_variable(&props_var);
                ctx.dynamic_slots.push((exp_var, props_var));
            }
            _ => {
                let props_var = ctx.hoist_variable(&props_var);
                ctx.slots.push(SlotInfo {
                    name: "default".to_string(),
                    offset: None,
                    tag_range: (start_tag_offset, start_tag_end_offset),
                    props_var,
                });
            }
        }
    }
    ctx.diagnostic_directive_end(out);
    for &child in &el.children {
        template_child(o, ctx, out, child, true, false);
    }
}
