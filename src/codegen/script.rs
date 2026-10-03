//! `codegen/script/*`: the virtual script around the user's `<script>` / `<script setup>`.

use std::cell::RefCell;

use indexmap::IndexSet;

use crate::code::{Code, Src, features};
use crate::codegen::{
    Boundary, CodeTransform, EOL, LocalTypes, NL, OMIT_INDEX_SIGNATURE, Out, PRETTIFY_LOCAL, PROPS_CHILDREN,
    ScriptBlock, TYPE_PROPS_TO_OPTION, WITH_DEFAULTS, WITH_SLOTS, as_type, block_terminator, camelized,
    code_with_transforms, insert, is_ts_lang, ref_brand_argument, replace, sfc_block_section, spread_merge, type_alias,
    typed_var,
};
use crate::names;
use crate::options::VueOptions;
use crate::parsers::{DefineModel, ExportDefault, Range, ScriptRanges, ScriptSetupRanges};
use crate::paths;
use crate::sfc::IrAttr;
use crate::shared::{camelize, is_identifier};
use crate::text::{Text, js_trim_end, len16};

pub struct ScriptOptions<'a> {
    pub vue: &'a VueOptions,
    pub file_name: &'a str,
    pub script: Option<&'a ScriptBlock<'a>>,
    pub script_setup: Option<&'a ScriptBlock<'a>>,
    pub script_lang: &'a str,
    pub script_ranges: Option<&'a ScriptRanges>,
    pub script_setup_ranges: Option<&'a ScriptSetupRanges>,
    pub template_and_style_types: &'a IndexSet<String>,
    /// consumed by the (single) template section
    pub template_and_style_codes: RefCell<Vec<Code>>,
    pub setup_bindings: &'a IndexSet<String>,
    pub local_components: &'a IndexSet<String>,
    pub local_directives: &'a IndexSet<String>,
    pub dot_value_bindings: &'a IndexSet<String>,
}

struct Ctx {
    generated_types: IndexSet<String>,
    local_types: LocalTypes,
}

type Body<'b> = &'b dyn Fn(&mut Ctx, &mut Out);

pub fn generate(o: &ScriptOptions) -> Vec<Code> {
    let mut ctx = Ctx { generated_types: IndexSet::new(), local_types: LocalTypes::new(&o.vue.lib, o.script_lang) };
    let mut out = Out::new();
    worker(o, &mut ctx, &mut out);
    out.codes
}

fn worker(o: &ScriptOptions, c: &mut Ctx, out: &mut Out) {
    let lang = o.script_lang;
    let export_expression = as_type(&format!("typeof {}", names::EXPORT), lang);
    global_types_reference(out, o.vue, o.file_name);

    let script_src = o.script.and_then(|s| match &s.block.src {
        Some(IrAttr::Text { text, offset }) => Some((text.as_str(), *offset)),
        _ => None,
    });

    if let Some((text, offset)) = script_src {
        // <script src="">
        let mut src = text.to_string();
        if src.ends_with(".ts") && !src.ends_with(".d.ts") {
            src = format!("{}.js", &src[..src.len() - 3]);
        } else if src.ends_with(".tsx") {
            src = format!("{}.jsx", &src[..src.len() - 4]);
        }
        out.t(format!("import {} from ", names::SRC));
        let feat = if src != text { features::ALL.merge(features::NAVIGATION_WITHOUT_RENAME) } else { features::ALL };
        let b = Boundary::start(out, Src::Main, offset, offset + len16(text), feat);
        out.t("'");
        let t = Text::new(&src);
        out.seg(t.slice(0, len16(text)), Src::Main, offset, b.feat);
        out.t(t.slice_from(len16(text)));
        out.t("'");
        b.end(out);
        out.t(EOL);
        synthetic_default_export(out, names::SRC);
        template(o, c, out, Some(names::SRC));
    } else if let (Some(script), Some(sr), Some(setup), Some(ssr)) =
        (o.script, o.script_ranges, o.script_setup, o.script_setup_ranges)
    {
        // <script> + <script setup>
        script_setup_imports(out, setup, ssr);
        let mut self_type = None;
        if let Some(ed) = &sr.export_default {
            self_type = Some(names::SELF);
            script_with_export_default(o, c, out, script, ed, names::SELF, &export_expression, None);
        } else {
            sfc_block_section(out, script, 0, script.text.len(), features::ALL);
            block_terminator(out, script.block);
            synthetic_default_export(out, &export_expression);
        }

        export_declare_equal(out, setup.block.name, names::EXPORT, 0, setup.text.len());
        let body = |c: &mut Ctx, out: &mut Out| template(o, c, out, self_type);
        if let Some(generic) = &setup.block.generic {
            let setup_fn = |c: &mut Ctx, out: &mut Out| setup_function(o, c, out, setup, ssr, &body, None);
            generic_fn(o, c, out, setup, ssr, generic, &setup_fn);
        } else {
            out.t(format!("await (async () => {{{NL}"));
            setup_function(o, c, out, setup, ssr, &body, Some(&|out: &mut Out| out.t("return ")));
            out.t(format!("}})(){EOL}"));
        }
    } else if let (Some(setup), Some(ssr)) = (o.script_setup, o.script_setup_ranges) {
        // only <script setup>
        script_setup_imports(out, setup, ssr);
        let body = |c: &mut Ctx, out: &mut Out| template(o, c, out, None);
        if let Some(generic) = &setup.block.generic {
            export_declare_equal(out, setup.block.name, names::EXPORT, 0, setup.text.len());
            let setup_fn = |c: &mut Ctx, out: &mut Out| setup_function(o, c, out, setup, ssr, &body, None);
            generic_fn(o, c, out, setup, ssr, generic, &setup_fn);
        } else {
            // no script block, generate script setup code at root
            let output = |out: &mut Out| export_declare_equal(out, setup.block.name, names::EXPORT, 0, setup.text.len());
            setup_function(o, c, out, setup, ssr, &body, Some(&output));
        }
        synthetic_default_export(out, &export_expression);
    } else if let (Some(script), Some(sr)) = (o.script, o.script_ranges) {
        // only <script>
        if let Some(ed) = &sr.export_default {
            let body = |c: &mut Ctx, out: &mut Out| template(o, c, out, Some(names::EXPORT));
            script_with_export_default(o, c, out, script, ed, names::EXPORT, &export_expression, Some(&body));
        } else {
            sfc_block_section(out, script, 0, script.text.len(), features::ALL);
            block_terminator(out, script.block);
            export_declare_equal(out, script.block.name, names::EXPORT, 0, script.text.len());
            out.t(format!("(await import('{}')).defineComponent({{}}){EOL}", o.vue.lib));
            template(o, c, out, Some(names::EXPORT));
            synthetic_default_export(out, &export_expression);
        }
    }

    c.local_types.generate(out);

    // The <script src> branch never embeds the script setup content, so no macro references can
    // appear and the import would be unused.
    if o.script_setup.is_some() && o.script_setup_ranges.is_some() && script_src.is_none() {
        macros(o, out);
    }
}

fn synthetic_default_export(out: &mut Out, exported: &str) {
    // keep the synthetic default export navigable: go-to-definition on
    // `import Foo from './Foo.vue'` lands on this zero-length anchor at the SFC top
    out.seg("", Src::Main, 0, features::NAVIGATION_WITHOUT_RENAME);
    out.t(format!("export default {exported}{EOL}"));
}

#[allow(clippy::too_many_arguments)]
fn script_with_export_default(
    o: &ScriptOptions,
    c: &mut Ctx,
    out: &mut Out,
    script: &ScriptBlock,
    ed: &ExportDefault,
    var_name: &str,
    export_expression: &str,
    template_gen: Option<Body>,
) {
    let (expression, is_object_literal) = match &ed.options {
        Some(op) => (op.expression, op.is_object_literal),
        None => (ed.expression, ed.is_object_literal),
    };
    let (mut wrap_left, mut wrap_right) = ("", "");
    if is_object_literal && !o.vue.options_wrapper.is_empty() {
        wrap_left = &o.vue.options_wrapper[0];
        wrap_right = o.vue.options_wrapper.get(1).map(String::as_str).unwrap_or("");
    }

    sfc_block_section(out, script, 0, expression.start, features::ALL);
    out.t(export_expression);
    sfc_block_section(out, script, expression.end, ed.end, features::ALL);
    out.t(EOL);
    if let Some(t) = template_gen {
        t(c, out);
    }
    export_declare_equal(out, script.block.name, var_name, 0, script.text.len());
    if !wrap_left.is_empty() && !wrap_right.is_empty() {
        out.t(wrap_left);
        sfc_block_section(out, script, expression.start, expression.end, features::ALL);
        out.t(wrap_right);
    } else {
        sfc_block_section(out, script, expression.start, expression.end, features::ALL);
    }
    out.t(EOL);
    sfc_block_section(out, script, ed.end, script.text.len(), features::ALL);
    block_terminator(out, script.block);
}

fn global_types_reference(out: &mut Out, vue: &VueOptions, file_name: &str) {
    let types_root = &vue.types_root;
    let types_path = if paths::is_absolute(types_root) {
        let mut relative = paths::relative(&paths::dirname(file_name), types_root);
        if relative != *types_root && !relative.starts_with("./") && !relative.starts_with("../") {
            relative = format!("./{relative}");
        }
        relative
    } else {
        types_root.clone()
    };
    let json = |s: String| serde_json::to_string(&s).unwrap();
    out.t(format!("/// <reference types={} />{NL}", json(format!("{types_path}/template-helpers.d.ts"))));
    if vue.lib == "vue" && vue.target < 3.5 {
        out.t(format!("/// <reference types={} />{NL}", json(format!("{types_path}/vue-3.4-shims.d.ts"))));
    }
}

/// `generateExportDeclareEqual(block, name, start, end)`
pub fn export_declare_equal(out: &mut Out, src: Src, name: &str, start: usize, end: usize) {
    out.t("const ");
    let b = Boundary::start(out, src, start, end, features::VERIFICATION);
    out.t(name);
    b.end(out);
    out.t(" = ");
}

// ---------------------------------------------------------------------------------------------
// scriptSetup.ts

fn script_setup_imports(out: &mut Out, setup: &ScriptBlock, ssr: &ScriptSetupRanges) {
    let end = ssr.import_section_end_offset.max(ssr.leading_comment_end_offset);
    out.seg(setup.text.slice(0, end), setup.block.name, 0, features::ALL);
}

#[allow(clippy::too_many_arguments)]
fn generic_fn(
    o: &ScriptOptions,
    c: &mut Ctx,
    out: &mut Out,
    setup: &ScriptBlock,
    ssr: &ScriptSetupRanges,
    generic: &IrAttr,
    body: Body,
) {
    let lib = &o.vue.lib;
    out.t("(");
    if let IrAttr::Text { text, offset } = generic {
        let b = Boundary::start(out, Src::Main, *offset, offset + len16(text), features::VERIFICATION);
        out.t("<");
        out.seg(text.clone(), Src::Main, *offset, features::ALL);
        if !js_trim_end(text).ends_with(',') {
            out.t(",");
        }
        out.t(">");
        b.end(out);
    }
    let pl = c.local_types.name(PRETTIFY_LOCAL);
    let (props, setup_, ctx_, exposed) = (names::PROPS, names::SETUP, names::CTX, names::EXPOSED);
    out.t(format!(
        "({NL}\t{props}: NonNullable<Awaited<typeof {setup_}>>['props'],{NL}\t{ctx_}?: {pl}<Pick<NonNullable<Awaited<typeof {setup_}>>, 'attrs' | 'emit' | 'slots'>>,{NL}\t{exposed}?: NonNullable<Awaited<typeof {setup_}>>['expose'],{NL}\t{setup_} = (async () => {{{NL}"
    ));

    body(c, out);

    let mut prop_types: Vec<String> = Vec::new();
    let mut emit_types: Vec<String> = Vec::new();
    if c.generated_types.contains(names::T_PUBLIC_PROPS) {
        prop_types.push(names::T_PUBLIC_PROPS.into());
    }
    if let Some(arg) = ssr.define_props.as_ref().and_then(|d| d.call.arg) {
        out.t(format!("const {} = ", names::PROPS_OPTION));
        sfc_block_section(out, setup, arg.start, arg.end, features::NAVIGATION);
        out.t(EOL);
        prop_types.push(format!("import('{lib}').ExtractPublicPropTypes<typeof {}>", names::PROPS_OPTION));
    }
    if ssr.define_emits.is_some() || !ssr.define_model.is_empty() {
        prop_types.push(names::T_EMIT_PROPS.into());
    }
    if o.template_and_style_types.contains(names::T_INHERITED_ATTRS) {
        prop_types.push(names::T_INHERITED_ATTRS.into());
    }
    if let Some(de) = &ssr.define_emits {
        emit_types.push(format!("typeof {}", de.name.as_deref().unwrap_or(names::EMIT)));
    }
    if !ssr.define_model.is_empty() {
        emit_types.push(format!("typeof {}", names::MODEL_EMIT));
    }

    out.t(format!("return {{}} as {{{NL}"));
    out.t("\tprops: ");
    if o.vue.target >= 3.4 {
        out.t(format!("import('{lib}').PublicProps"));
    } else {
        out.t(format!(
            "import('{lib}').VNodeProps & import('{lib}').AllowedComponentProps & import('{lib}').ComponentCustomProps"
        ));
    }
    if !prop_types.is_empty() {
        let pl = c.local_types.name(PRETTIFY_LOCAL);
        out.t(format!(" & {pl}<{}>", prop_types.join(" & ")));
    }
    out.t(EOL);
    out.t("\texpose: (exposed: ");
    if ssr.define_expose.is_some() {
        out.t(format!("import('{lib}').ShallowUnwrapRef<typeof {}>", names::EXPOSED));
    } else {
        out.t("{}");
    }
    if o.vue.infer_component_dollar_refs && o.template_and_style_types.contains(names::T_TEMPLATE_REFS) {
        out.t(format!(" & {{ $refs: {}; }}", names::T_TEMPLATE_REFS));
    }
    if o.vue.infer_component_dollar_el && o.template_and_style_types.contains(names::T_ROOT_EL) {
        out.t(format!(" & {{ $el: {}; }}", names::T_ROOT_EL));
    }
    out.t(format!(") => void{EOL}"));
    out.t(format!("\tattrs: any{EOL}"));
    out.t(format!("\tslots: {}{EOL}", if has_slots_type(o) { names::T_SLOTS } else { "{}" }));
    let emit = if emit_types.is_empty() { "{}".to_string() } else { emit_types.join(" & ") };
    out.t(format!("\temit: {emit}{EOL}"));
    out.t(format!("}}{EOL}"));
    out.t(format!("}})(),{NL}"));
    out.t(format!(
        ") => ({{}} as import('{lib}').VNode & {{ __ctx?: NonNullable<Awaited<typeof {}>> }})){EOL}",
        names::SETUP
    ));
}

#[allow(clippy::too_many_arguments)]
fn setup_function(
    o: &ScriptOptions,
    c: &mut Ctx,
    out: &mut Out,
    setup: &ScriptBlock,
    ssr: &ScriptSetupRanges,
    body: Body,
    output: Option<&dyn Fn(&mut Out)>,
) {
    let lang = o.script_lang;
    let ts = is_ts_lang(lang);
    let lib = o.vue.lib.as_str();
    let mut transforms: Vec<CodeTransform> = Vec::new();

    if let Some(dp) = &ssr.define_props {
        let call_exp = ssr.with_defaults.as_ref().map(|w| w.call_exp).unwrap_or(dp.call.call_exp);
        let name = dp.name.as_deref();
        transforms.extend(define_with_type(setup, dp.statement, call_exp, dp.call.type_arg, name, names::PROPS, names::T_PROPS));
    }
    if let Some(de) = &ssr.define_emits {
        let name = de.name.as_deref();
        transforms.extend(define_with_type(setup, de.statement, de.call.call_exp, de.call.type_arg, name, names::EMIT, names::T_EMIT));
    }
    if let Some(ds) = &ssr.define_slots {
        let name = ds.name.as_deref();
        transforms.extend(define_with_type(setup, ds.statement, ds.call.call_exp, ds.call.type_arg, name, names::SLOTS, names::T_SLOTS));
        if ds.name.is_none() {
            transforms.push(insert(ds.statement.end, |out| out.t(format!("{NL}void {}{EOL}", names::SLOTS))));
        }
    }
    if let Some(dx) = &ssr.define_expose {
        if let Some(ta) = dx.type_arg {
            transforms.push(insert(dx.call_exp.start, move |out| {
                out.t(format!("let {}!: ", names::EXPOSED));
                sfc_block_section(out, setup, ta.start, ta.end, features::ALL);
                out.t(EOL);
            }));
            transforms.push(replace(ta.start, ta.end, |out| out.t(format!("typeof {}", names::EXPOSED))));
        } else if let Some(arg) = dx.arg {
            transforms.push(insert(dx.call_exp.start, move |out| {
                out.t(format!("const {} = ", names::EXPOSED));
                sfc_block_section(out, setup, arg.start, arg.end, features::ALL);
                out.t(EOL);
            }));
            transforms.push(replace(arg.start, arg.end, |out| out.t(names::EXPOSED)));
        } else {
            transforms.push(insert(dx.call_exp.start, |out| out.t(format!("const {} = {{}}{EOL}", names::EXPOSED))));
        }
    }
    if o.vue.infer_template_dollar_attrs {
        for ua in &ssr.use_attrs {
            cast_call(&mut transforms, ua.call_exp, format!("typeof {}.$attrs", names::DOLLARS), ts);
        }
    }
    for cm in &ssr.use_css_module {
        let ty = if o.template_and_style_types.contains(names::T_STYLE_MODULES) { names::T_STYLE_MODULES } else { "{}" };
        if let Some(arg) = cm.arg {
            transforms.push(insert(cm.call_exp.start, move |out| {
                if ts {
                    out.t("(");
                } else {
                    out.t(format!("/** @type {{Omit<{ty}, '$style'>["));
                    sfc_block_section(out, setup, arg.start, arg.end, features::WITHOUT_SEMANTIC);
                    out.t("]} */ (");
                }
            }));
            transforms.push(insert(cm.call_exp.end, move |out| {
                if ts {
                    out.t(format!(" as Omit<{ty}, '$style'>["));
                    sfc_block_section(out, setup, arg.start, arg.end, features::WITHOUT_SEMANTIC);
                    out.t("])");
                } else {
                    out.t(")");
                }
            }));
            transforms.push(replace(arg.start, arg.end, move |out| out.t(as_type("any", lang))));
        } else {
            let exp = cm.exp;
            transforms.push(insert(cm.call_exp.start, move |out| {
                out.t(if ts { "(".to_string() } else { format!("/** @type {{{ty}['$style']}} */ (") });
            }));
            transforms.push(insert(cm.call_exp.end, move |out| {
                if ts {
                    out.t(format!(" as {ty}["));
                    let b = Boundary::start(out, setup.block.name, exp.start, exp.end, features::VERIFICATION);
                    out.t("'$style'");
                    b.end(out);
                    out.t("])");
                } else {
                    out.t(")");
                }
            }));
        }
    }
    if o.vue.infer_template_dollar_slots {
        for us in &ssr.use_slots {
            cast_call(&mut transforms, us.call_exp, format!("typeof {}.$slots", names::DOLLARS), ts);
        }
    }
    for tr in &ssr.use_template_ref {
        let arg = tr.call.arg;
        let ref_type = move |out: &mut Out| {
            if let Some(arg) = arg {
                out.t(names::T_TEMPLATE_REFS);
                out.t("[");
                sfc_block_section(out, setup, arg.start, arg.end, features::WITHOUT_SEMANTIC);
                out.t("]");
            } else {
                out.t("unknown");
            }
        };
        transforms.push(insert(tr.call.call_exp.start, move |out| {
            if ts {
                out.t("(");
                return;
            }
            out.t(format!("/** @type {{Readonly<import('{lib}').ShallowRef<"));
            ref_type(out);
            out.t(" | null>>} */ (");
        }));
        transforms.push(insert(tr.call.call_exp.end, move |out| {
            if ts {
                out.t(format!(" as Readonly<import('{lib}').ShallowRef<"));
                ref_type(out);
                out.t(" | null>>)");
            } else {
                out.t(")");
            }
        }));
        if let Some(arg) = arg {
            transforms.push(replace(arg.start, arg.end, move |out| out.t(as_type("any", lang))));
        }
    }

    let start = ssr.import_section_end_offset.max(ssr.leading_comment_end_offset);
    code_with_transforms(out, start, setup.text.len(), transforms, &|out, s, e| {
        sfc_block_section(out, setup, s, e, features::ALL)
    });
    block_terminator(out, setup.block);
    models(o, out, setup, ssr);
    public_props(o, c, out, setup, ssr);
    body(c, out);

    if let Some(output) = output {
        if has_slots_type(o) {
            out.t(format!("const {} = ", names::BASE));
            component(o, c, out, setup, ssr);
            out.t(EOL);
            output(out);
            let ws = c.local_types.name(WITH_SLOTS);
            out.t(as_type(&format!("{ws}<typeof {}, {}>", names::BASE, names::T_SLOTS), lang));
            out.t(EOL);
        } else {
            output(out);
            component(o, c, out, setup, ssr);
            out.t(EOL);
        }
    }
}

/// `(call as T)` in TS, `/** @type {T} */ (call)` in JS.
fn cast_call(transforms: &mut Vec<CodeTransform>, call: Range, ty: String, ts: bool) {
    let ty2 = ty.clone();
    transforms.push(insert(call.start, move |out| {
        out.t(if ts { "(".to_string() } else { format!("/** @type {{{ty}}} */ (") });
    }));
    transforms.push(insert(call.end, move |out| {
        out.t(if ts { format!(" as {ty2})") } else { ")".to_string() });
    }));
}

/// `generateMacros`: an import keeps the macros resolvable in JS virtual files.
fn macros(o: &ScriptOptions, out: &mut Out) {
    if o.vue.target < 3.3 {
        return;
    }
    let (Some(setup), Some(ssr)) = (o.script_setup, o.script_setup_ranges) else { return };
    let mut used: Vec<(&str, String)> = Vec::new();
    let mut add = |canonical: &'static str, exp: Option<Range>| {
        if let Some(e) = exp {
            used.push((canonical, setup.text.slice(e.start, e.end).to_string()));
        }
    };
    add("defineProps", ssr.define_props.as_ref().map(|d| d.call.exp));
    add("defineEmits", ssr.define_emits.as_ref().map(|d| d.call.exp));
    add("defineSlots", ssr.define_slots.as_ref().map(|d| d.call.exp));
    add("defineExpose", ssr.define_expose.as_ref().map(|d| d.exp));
    add("withDefaults", ssr.with_defaults.as_ref().map(|d| d.exp));
    if !ssr.define_model.is_empty() {
        used.push(("defineModel", "defineModel".into()));
    }
    if ssr.define_options.is_some() {
        used.push(("defineOptions", "defineOptions".into()));
    }
    let to_import: Vec<_> = used.into_iter().filter(|(_, alias)| !o.setup_bindings.contains(alias)).collect();
    if to_import.is_empty() {
        return;
    }
    out.t("import { ");
    for (canonical, alias) in to_import {
        if alias == canonical {
            out.t(format!("{canonical}, "));
        } else {
            out.t(format!("{canonical} as {alias}, "));
        }
    }
    out.t(format!("}} from '{}'{EOL}", o.vue.lib));
}

fn define_with_type<'f>(
    setup: &'f ScriptBlock<'f>,
    statement: Range,
    call_exp: Range,
    type_arg: Option<Range>,
    name: Option<&str>,
    default_name: &'static str,
    type_name: &'static str,
) -> Vec<CodeTransform<'f>> {
    let mut v = Vec::new();
    let section = move |out: &mut Out, s: usize, e: usize| sfc_block_section(out, setup, s, e, features::ALL);
    if let Some(ta) = type_arg {
        v.push(insert(statement.start, move |out| {
            out.t(format!("type {type_name} = "));
            section(out, ta.start, ta.end);
            out.t(EOL);
        }));
        v.push(replace(ta.start, ta.end, move |out| out.t(type_name)));
    }
    match name {
        None => {
            if statement.start == call_exp.start && statement.end == call_exp.end {
                v.push(insert(call_exp.start, move |out| out.t(format!("const {default_name} = "))));
            } else if let Some(ta) = type_arg {
                v.push(replace(statement.start, ta.start, move |out| {
                    out.t(format!("const {default_name} = "));
                    section(out, call_exp.start, ta.start);
                }));
                v.push(replace(ta.end, call_exp.end, move |out| {
                    section(out, ta.end, call_exp.end);
                    out.t(EOL);
                    section(out, statement.start, call_exp.start);
                    out.t(default_name);
                }));
            } else {
                v.push(replace(statement.start, call_exp.end, move |out| {
                    out.t(format!("const {default_name} = "));
                    section(out, call_exp.start, call_exp.end);
                    out.t(EOL);
                    section(out, statement.start, call_exp.start);
                    out.t(default_name);
                }));
            }
        }
        Some(n) if !is_identifier(n) => {
            v.push(replace(statement.start, call_exp.start, move |out| out.t(format!("const {default_name} = "))));
            v.push(insert(statement.end, move |out| {
                out.t(EOL);
                section(out, statement.start, call_exp.start);
                out.t(default_name);
            }));
        }
        _ => {}
    }
    v
}

fn public_props(o: &ScriptOptions, c: &mut Ctx, out: &mut Out, setup: &ScriptBlock, ssr: &ScriptSetupRanges) {
    let type_arg = ssr.define_props.as_ref().and_then(|d| d.call.type_arg);
    if let (Some(_), Some(arg)) = (type_arg, ssr.with_defaults.as_ref().and_then(|w| w.arg)) {
        out.t(format!("const {} = ", names::DEFAULTS));
        sfc_block_section(out, setup, arg.start, arg.end, features::NAVIGATION);
        out.t(EOL);
        out.t(format!("void {}{EOL}", names::DEFAULTS));
    }

    let mut prop_types: Vec<String> = Vec::new();
    if o.vue.jsx_slots && has_slots_type(o) {
        let pc = c.local_types.name(PROPS_CHILDREN);
        prop_types.push(format!("{pc}<{}>", names::T_SLOTS));
    }
    if type_arg.is_some() {
        prop_types.push(names::T_PROPS.into());
    }
    if !ssr.define_model.is_empty() {
        prop_types.push(names::T_MODEL_PROPS.into());
    }
    let target = o.vue.target;
    let used = setup.block.generic.is_some()
        || target < 3.6
        || (target >= 3.5 && ssr.define_props.as_ref().is_none_or(|d| d.call.arg.is_none()));
    if !prop_types.is_empty() && used {
        type_alias(out, names::T_PUBLIC_PROPS, o.script_lang, |out| out.t(prop_types.join(" & ")));
        c.generated_types.insert(names::T_PUBLIC_PROPS.into());
    }
}

fn has_slots_type(o: &ScriptOptions) -> bool {
    o.script_setup_ranges.is_some_and(|r| r.define_slots.is_some())
        || o.template_and_style_types.contains(names::T_SLOTS)
}

fn models(o: &ScriptOptions, out: &mut Out, setup: &ScriptBlock, ssr: &ScriptSetupRanges) {
    if ssr.define_model.is_empty() {
        return;
    }
    let text = |r: Range| setup.text.slice(r.start, r.end);
    let mut default_codes: Vec<String> = Vec::new();
    let mut prop_codes: Vec<Code> = Vec::new();
    let mut emit_codes: Vec<String> = Vec::new();

    for dm in &ssr.define_model {
        let prop_name = match dm.name {
            Some(n) => {
                let quoted = Text::new(text(n));
                camelize(quoted.slice(1, quoted.len().saturating_sub(1)))
            }
            None => "modelValue".to_string(),
        };
        let model_type = if let Some(t) = dm.type_ {
            // Infer from defineModel<T>
            text(t).to_string()
        } else if let (Some(_), Some(local)) = (dm.runtime_type, dm.local_name) {
            // Infer from actual prop declaration code
            format!("typeof {}['value']", text(local))
        } else if dm.default_value.is_some() && !prop_name.is_empty() {
            // Infer from defineModel({ default: T })
            format!("typeof {}['{prop_name}']", names::DEFAULT_MODELS)
        } else {
            "any".to_string()
        };
        if let Some(dv) = dm.default_value {
            default_codes.push(format!("'{prop_name}': {},{NL}", text(dv)));
        }
        prop_codes.extend(model_prop(o, setup, dm, &prop_name, &model_type));
        emit_codes.push(model_emit(dm, &prop_name, &model_type));
    }

    if !default_codes.is_empty() {
        out.t(format!("const {} = {{{NL}", names::DEFAULT_MODELS));
        for c in default_codes {
            out.t(c);
        }
        out.t(format!("}}{EOL}"));
        out.t(format!("void {}{EOL}", names::DEFAULT_MODELS));
    }
    type_alias(out, names::T_MODEL_PROPS, o.script_lang, |out| {
        out.t(format!("{{{NL}"));
        out.extend(prop_codes);
        out.t("}");
    });
    type_alias(out, names::T_MODEL_EMIT, o.script_lang, |out| {
        out.t(format!("{{{NL}"));
        for c in emit_codes {
            out.t(c);
        }
        out.t("}");
    });
    // avoid `defineModel<...>()` to prevent JS AST issues
    typed_var(out, "let", names::MODEL_EMIT, o.script_lang, |out| {
        out.t(format!("{}<{}>", names::T_SHORT_EMITS, names::T_MODEL_EMIT));
    });
}

fn model_prop(o: &ScriptOptions, setup: &ScriptBlock, dm: &DefineModel, prop_name: &str, model_type: &str) -> Vec<Code> {
    let mut out = Out::new();
    // In JS the alias body lives inside a JSDoc comment; user comments could contain `*/` and
    // terminate it early, so they are dropped.
    if let (Some(c), true) = (dm.comments, is_ts_lang(o.script_lang)) {
        out.t(setup.text.slice(c.start, c.end));
        out.t(NL);
    }
    if let Some(n) = dm.name {
        camelized(&mut out, setup.text.slice(n.start, n.end), setup.block.name, n.start, features::NAVIGATION);
    } else {
        out.t(prop_name);
    }
    out.t(if dm.required { ": " } else { "?: " });
    out.t(model_type);
    out.t(EOL);
    if let Some(mt) = dm.modifier_type {
        let modifier_name = format!("{}Modifiers", if prop_name == "modelValue" { "model" } else { prop_name });
        out.t(format!("'{modifier_name}'?: Partial<Record<{}, true>>{EOL}", setup.text.slice(mt.start, mt.end)));
    }
    out.codes
}

fn model_emit(dm: &DefineModel, prop_name: &str, model_type: &str) -> String {
    let undefined = if !dm.required && dm.default_value.is_none() { " | undefined" } else { "" };
    format!("'update:{prop_name}': [value: {model_type}{undefined}]{EOL}")
}

// ---------------------------------------------------------------------------------------------
// component.ts

fn component(o: &ScriptOptions, c: &mut Ctx, out: &mut Out, setup: &ScriptBlock, ssr: &ScriptSetupRanges) {
    let lang = o.script_lang;
    out.t(format!("(await import('{}')).defineComponent({{{NL}", o.vue.lib));
    if ssr.define_expose.is_some() {
        out.t(format!("setup: () => {},{NL}", names::EXPOSED));
    }
    let emit_option = emits_option(o, ssr);
    let has_emits_option = !emit_option.is_empty();
    out.extend(emit_option);
    props_option(o, c, out, setup, ssr, has_emits_option);
    let target = o.vue.target;
    if target >= 3.5 && o.vue.infer_component_dollar_refs && o.template_and_style_types.contains(names::T_TEMPLATE_REFS)
    {
        out.t(format!("__typeRefs: {},{NL}", as_type(names::T_TEMPLATE_REFS, lang)));
    }
    if target >= 3.5 && o.vue.infer_component_dollar_el && o.template_and_style_types.contains(names::T_ROOT_EL) {
        out.t(format!("__typeEl: {},{NL}", as_type(names::T_ROOT_EL, lang)));
    }
    out.t("})");
}

fn text_codes(v: Vec<String>) -> Vec<Code> {
    v.into_iter().map(Code::Text).collect()
}

fn emits_option(o: &ScriptOptions, ssr: &ScriptSetupRanges) -> Vec<Code> {
    let lang = o.script_lang;
    let mut out = Out::new();
    let mut type_codes: Vec<String> = Vec::new();
    if o.vue.target >= 3.5 && !ssr.define_emits.as_ref().is_some_and(|d| d.has_union_type_arg) {
        if !ssr.define_model.is_empty() {
            type_codes.push(names::T_MODEL_EMIT.into());
        }
        if ssr.define_emits.as_ref().is_some_and(|d| d.call.type_arg.is_some()) {
            type_codes.push(names::T_EMIT.into());
        }
    }
    let mut runtime_codes: Vec<String> = Vec::new();
    if type_codes.is_empty() {
        if !ssr.define_model.is_empty() {
            runtime_codes.push(as_type(&format!("{}<typeof {}>", names::T_NORMALIZE_EMITS, names::MODEL_EMIT), lang));
        }
        if let Some(de) = &ssr.define_emits {
            let name = de.name.as_deref().unwrap_or(names::EMIT);
            runtime_codes.push(as_type(&format!("{}<typeof {name}>", names::T_NORMALIZE_EMITS), lang));
        }
    }
    if !type_codes.is_empty() {
        out.t(format!("__typeEmits: {},{NL}", as_type(&type_codes.join(" & "), lang)));
    } else if !runtime_codes.is_empty() {
        out.t("emits: ");
        spread_merge(&mut out, text_codes(runtime_codes));
        out.t(format!(",{NL}"));
    }
    out.codes
}

fn props_option(
    o: &ScriptOptions,
    c: &mut Ctx,
    out: &mut Out,
    setup: &ScriptBlock,
    ssr: &ScriptSetupRanges,
    has_emits_option: bool,
) {
    let lang = o.script_lang;
    let target = o.vue.target;
    let inherited_attrs = || {
        if has_emits_option {
            format!("Omit<{}, keyof {}>", names::T_INHERITED_ATTRS, names::T_EMIT_PROPS)
        } else {
            names::T_INHERITED_ATTRS.to_string()
        }
    };

    let mut type_codes: Vec<String> = Vec::new();
    if target >= 3.5 && ssr.define_props.as_ref().is_none_or(|d| d.call.arg.is_none()) {
        if o.template_and_style_types.contains(names::T_INHERITED_ATTRS) {
            type_codes.push(as_type(&inherited_attrs(), lang));
        }
        if c.generated_types.contains(names::T_PUBLIC_PROPS) {
            type_codes.push(as_type(names::T_PUBLIC_PROPS, lang));
        }
    }

    let mut runtime = Out::new();
    if ssr.with_defaults.is_some() || type_codes.is_empty() {
        if o.template_and_style_types.contains(names::T_INHERITED_ATTRS) {
            let to_option = c.local_types.name(TYPE_PROPS_TO_OPTION);
            let omit_index = c.local_types.name(OMIT_INDEX_SIGNATURE);
            let props_type =
                format!("{to_option}<{}<{omit_index}<{}>, {{}}>>", names::T_PICK_NOT_ANY, inherited_attrs());
            runtime.t(as_type(&props_type, lang));
        }
        if c.generated_types.contains(names::T_PUBLIC_PROPS) && target < 3.6 {
            let to_option = c.local_types.name(TYPE_PROPS_TO_OPTION);
            let mut props_type = format!("{to_option}<{}>", names::T_PUBLIC_PROPS);
            if ssr.with_defaults.as_ref().is_some_and(|w| w.arg.is_some()) {
                let wd = c.local_types.name(WITH_DEFAULTS);
                props_type = format!("{wd}<{props_type}, typeof {}>", names::DEFAULTS);
            }
            runtime.t(as_type(&props_type, lang));
        }
        if let Some(arg) = ssr.define_props.as_ref().and_then(|d| d.call.arg) {
            sfc_block_section(&mut runtime, setup, arg.start, arg.end, features::NAVIGATION);
        }
    }

    if !type_codes.is_empty() {
        if target >= 3.6 && ssr.with_defaults.as_ref().is_some_and(|w| w.arg.is_some()) {
            out.t(format!("__defaults: {},{NL}", names::DEFAULTS));
        }
        out.t("__typeProps: ");
        spread_merge(out, text_codes(type_codes));
        out.t(format!(",{NL}"));
    }
    if !runtime.codes.is_empty() {
        out.t("props: ");
        spread_merge(out, runtime.codes);
        out.t(format!(",{NL}"));
    }
}

// ---------------------------------------------------------------------------------------------
// template.ts

fn template(o: &ScriptOptions, _c: &mut Ctx, out: &mut Out, self_type: Option<&str>) {
    template_ctx(o, out, self_type);
    template_components(o, out);
    template_directives(o, out);
    out.t(format!("void {}, {}, {}, {}{EOL}", names::CTX, names::COMPONENTS, names::INTRINSICS, names::DIRECTIVES));
    for name in o.dot_value_bindings {
        out.t(format!("// @ts-ignore{NL}"));
        out.t(format!(
            "{}({name}, {}){EOL}",
            names::WITH_DOT_VALUE,
            ref_brand_argument(&o.vue.lib, o.script_lang)
        ));
    }
    let codes = std::mem::take(&mut *o.template_and_style_codes.borrow_mut());
    out.extend(codes);
}

fn template_ctx(o: &ScriptOptions, out: &mut Out, self_type: Option<&str>) {
    let lang = o.script_lang;
    let ssr = o.script_setup_ranges;
    let mut exps: Vec<String> = Vec::new();
    let mut emit_types: Vec<String> = Vec::new();
    let mut prop_types: Vec<String> = Vec::new();

    match self_type {
        Some(st) => exps.push(as_type(&format!("InstanceType<{}<typeof {st}, new () => {{}}>>", names::T_PICK_NOT_ANY), lang)),
        None => exps.push(as_type(&format!("import('{}').ComponentPublicInstance", o.vue.lib), lang)),
    }
    if o.template_and_style_types.contains(names::T_STYLE_MODULES) {
        exps.push(as_type(names::T_STYLE_MODULES, lang));
    }
    if let Some(de) = ssr.and_then(|r| r.define_emits.as_ref()) {
        emit_types.push(format!("typeof {}", de.name.as_deref().unwrap_or(names::EMIT)));
    }
    if ssr.is_some_and(|r| !r.define_model.is_empty()) {
        emit_types.push(format!("typeof {}", names::MODEL_EMIT));
    }
    if !emit_types.is_empty() {
        let joined = emit_types.join(" & ");
        type_alias(out, names::T_EMIT_PROPS, lang, |out| {
            out.t(format!("{}<{}<{joined}>>", names::T_EMITS_TO_PROPS, names::T_NORMALIZE_EMITS));
        });
        exps.push(as_type(&format!("{{ $emit: {joined} }}"), lang));
    }
    if let Some(dp) = ssr.and_then(|r| r.define_props.as_ref()) {
        prop_types.push(format!("typeof {}", dp.name.as_deref().unwrap_or(names::PROPS)));
    }
    if ssr.is_some_and(|r| !r.define_model.is_empty()) {
        prop_types.push(names::T_MODEL_PROPS.into());
    }
    if !emit_types.is_empty() {
        prop_types.push(names::T_EMIT_PROPS.into());
    }
    if !prop_types.is_empty() {
        let joined = prop_types.join(" & ");
        exps.push(as_type(&format!("{{ $props: {joined} }}"), lang));
        exps.push(as_type(&joined, lang));
    }
    out.t(format!("const {} = ", names::CTX));
    spread_merge(out, text_codes(exps));
    out.t(EOL);
}

fn exposed_type(lib: &str, bindings: &IndexSet<String>) -> String {
    let members: Vec<String> = bindings.iter().map(|n| format!("{n}: typeof {n};")).collect();
    format!("import('{lib}').ShallowUnwrapRef<{{\n{}\n}}>", members.join("\n"))
}

fn component_options<'a>(o: &'a ScriptOptions<'a>) -> Option<&'a crate::parsers::ComponentOptions> {
    o.script_ranges?.export_default.as_ref()?.options.as_ref()
}

fn template_components(o: &ScriptOptions, out: &mut Out) {
    let lang = o.script_lang;
    let lib = &o.vue.lib;
    let mut types: Vec<String> = Vec::new();
    if !o.local_components.is_empty() {
        types.push(exposed_type(lib, o.local_components));
    }
    if let (Some(script), Some(comps)) = (o.script, component_options(o).and_then(|op| op.components)) {
        out.t(format!("const {} = ", names::COMPONENTS_OPTION));
        sfc_block_section(out, script, comps.start, comps.end, features::NAVIGATION);
        out.t(EOL);
        types.push(format!("typeof {}", names::COMPONENTS_OPTION));
    }
    type_alias(out, names::T_LOCAL_COMPONENTS, lang, |out| {
        out.t(if types.is_empty() { "{}".to_string() } else { types.join(" & ") });
    });
    type_alias(out, names::T_GLOBAL_COMPONENTS, lang, |out| {
        if o.vue.target >= 3.5 {
            out.t(format!("import('{lib}').GlobalComponents"));
        } else {
            out.t(format!("import('{lib}').GlobalComponents & Pick<typeof import('{lib}'), 'Transition' | 'TransitionGroup' | 'KeepAlive' | 'Suspense' | 'Teleport'>"));
        }
    });
    typed_var(out, "let", names::COMPONENTS, lang, |out| {
        out.t(format!("{} & {}", names::T_LOCAL_COMPONENTS, names::T_GLOBAL_COMPONENTS));
    });
    typed_var(out, "let", names::INTRINSICS, lang, |out| {
        out.t(format!("import('{lib}/jsx-runtime').JSX.IntrinsicElements"));
    });
}

fn template_directives(o: &ScriptOptions, out: &mut Out) {
    let lang = o.script_lang;
    let lib = &o.vue.lib;
    let mut types: Vec<String> = Vec::new();
    if !o.local_directives.is_empty() {
        types.push(exposed_type(lib, o.local_directives));
    }
    if let (Some(script), Some(dirs)) = (o.script, component_options(o).and_then(|op| op.directives)) {
        out.t(format!("const {} = ", names::DIRECTIVES_OPTION));
        sfc_block_section(out, script, dirs.start, dirs.end, features::NAVIGATION);
        out.t(EOL);
        types.push(format!("{}<typeof {}>", names::T_RESOLVE_DIRECTIVES, names::DIRECTIVES_OPTION));
    }
    type_alias(out, names::T_LOCAL_DIRECTIVES, lang, |out| {
        out.t(if types.is_empty() { "{}".to_string() } else { types.join(" & ") });
    });
    typed_var(out, "let", names::DIRECTIVES, lang, |out| {
        out.t(format!(
            "{} & {} & import('{lib}').GlobalDirectives",
            names::T_LOCAL_DIRECTIVES,
            names::T_BUILT_IN_DIRECTIVES
        ));
    });
}

