//! `codegen/style/*`: CSS modules, scoped class names and `v-bind()` in CSS.

use indexmap::IndexSet;

use crate::code::{Code, Src, features};
use crate::codegen::template::context::Ctx;
use crate::codegen::template::interpolation::{InterpOpts, interpolation};
use crate::codegen::template::style_scoped_class_reference;
use crate::codegen::{Boundary, EOL, NL, Out, is_ts_lang, type_alias};
use crate::names;
use crate::options::{StyleClassNames, VueOptions};
use crate::sfc::{Block, IrAttr};
use crate::shared::is_identifier;
use crate::style::StyleInfo;
use crate::text::len16;

pub struct StyleBlock<'a> {
    pub block: &'a Block,
    pub info: &'a StyleInfo,
}

pub struct StyleOptions<'a> {
    pub vue: &'a VueOptions,
    pub styles: &'a [StyleBlock<'a>],
    pub interp: InterpOpts<'a>,
}

pub struct StyleResult {
    pub codes: Vec<Code>,
    pub generated_types: IndexSet<String>,
    pub dot_value_accesses: IndexSet<String>,
}

pub fn generate(o: &StyleOptions) -> StyleResult {
    let mut ctx = Ctx::default();
    let mut out = Out::new();
    ctx.scope();
    scoped_classes(o, &mut ctx, &mut out);
    modules(o, &mut ctx, &mut out);
    css_vars(o, &mut ctx, &mut out);
    ctx.end_scope(&mut out);
    StyleResult { codes: out.codes, generated_types: ctx.generated_types, dot_value_accesses: ctx.dot_value_accesses }
}

fn skip_first(s: &str) -> &str {
    let mut c = s.chars();
    c.next();
    c.as_str()
}

fn scoped_classes(o: &StyleOptions, ctx: &mut Ctx, out: &mut Out) {
    let all = match o.vue.resolve_style_class_names {
        StyleClassNames::Bool(false) => return,
        StyleClassNames::Bool(true) => true,
        StyleClassNames::Scoped => false,
    };
    let scoped: Vec<&StyleBlock> = o.styles.iter().filter(|s| all || s.block.scoped).collect();
    if scoped.is_empty() {
        return;
    }
    ctx.generated_types.insert(names::T_STYLE_SCOPED_CLASSES.into());

    let mut visited: IndexSet<&str> = IndexSet::new();
    let mut deferred: Vec<(Src, &str, usize)> = Vec::new();
    type_alias(out, names::T_STYLE_SCOPED_CLASSES, o.interp.script_lang, |out| {
        out.t("{}");
        for s in &scoped {
            if o.vue.resolve_style_imports {
                style_imports(out, s);
            }
            for cn in &s.info.class_names {
                if visited.insert(&cn.text) {
                    class_property(out, s.block.name, &cn.text, cn.offset, "boolean");
                } else {
                    deferred.push((s.block.name, skip_first(&cn.text), cn.offset + 1));
                }
            }
        }
    });
    if is_ts_lang(o.interp.script_lang) {
        // avoid TS6196: JSDoc refs don't count as usage; JS @typedefs skip the check
        out.t(format!("void ({{}} as {}){EOL}", names::T_STYLE_SCOPED_CLASSES));
    }
    for (src, name, offset) in deferred {
        style_scoped_class_reference(out, src, name, offset, None);
    }
}

fn modules(o: &StyleOptions, ctx: &mut Ctx, out: &mut Out) {
    let mods: Vec<&StyleBlock> = o.styles.iter().filter(|s| s.block.module.is_some()).collect();
    if mods.is_empty() {
        return;
    }
    ctx.generated_types.insert(names::T_STYLE_MODULES.into());

    type_alias(out, names::T_STYLE_MODULES, o.interp.script_lang, |out| {
        out.t(format!("{{{NL}"));
        for s in &mods {
            match s.block.module.as_ref().unwrap() {
                IrAttr::True => out.t("$style"),
                IrAttr::Text { text, offset } => {
                    if text.is_empty() {
                        out.seg("$style", Src::Main, *offset, features::VERIFICATION);
                    } else if is_identifier(text) {
                        out.seg(text.clone(), Src::Main, *offset, features::NAVIGATION_AND_VERIFICATION);
                    } else {
                        let b = Boundary::start(out, Src::Main, *offset, offset + len16(text), features::NAVIGATION_AND_VERIFICATION);
                        out.t("'");
                        out.seg(text.clone(), Src::Main, *offset, b.feat);
                        out.t("'");
                        b.end(out);
                    }
                }
            }
            out.t(": ");
            if !o.vue.strict_css_modules {
                out.t("Record<string, string> & ");
            }
            out.t(format!("{}<{{}}", names::T_PRETTIFY_GLOBAL));
            if o.vue.resolve_style_imports {
                style_imports(out, s);
            }
            for cn in &s.info.class_names {
                class_property(out, s.block.name, &cn.text, cn.offset, "string");
            }
            out.t(format!(">{EOL}"));
        }
        out.t("}");
    });
}

fn css_vars(o: &StyleOptions, ctx: &mut Ctx, out: &mut Out) {
    for s in o.styles {
        for b in &s.info.bindings {
            interpolation(&o.interp, ctx, out, s.block.name, features::ALL, &b.text, b.offset, "(", ")", false);
            out.t(EOL);
        }
    }
}

/// `generateClassProperty(source, classNameWithDot, offset, propertyType)`
fn class_property(out: &mut Out, src: Src, class_name_with_dot: &str, offset: usize, property_type: &str) {
    out.t(format!("{NL} & {{ "));
    let b = Boundary::start(out, src, offset, offset + len16(class_name_with_dot), features::NAVIGATION);
    out.t("'");
    out.seg(skip_first(class_name_with_dot), src, offset + 1, b.feat);
    out.t("'");
    b.end(out);
    out.t(format!(": {property_type}"));
    out.t(" }");
}

/// `generateStyleImports(style)`
fn style_imports(out: &mut Out, s: &StyleBlock) {
    if let Some(IrAttr::Text { text, offset }) = &s.block.src {
        out.t(format!("{NL} & typeof import("));
        let b = Boundary::start(out, Src::Main, *offset, offset + len16(text), features::NAVIGATION_AND_VERIFICATION);
        out.t("'");
        out.seg(text.clone(), Src::Main, *offset, b.feat);
        out.t("'");
        b.end(out);
        out.t(").default");
    }
    for imp in &s.info.imports {
        out.t(format!("{NL} & typeof import('"));
        out.seg(imp.text.clone(), s.block.name, imp.offset, features::NAVIGATION_AND_VERIFICATION);
        out.t("').default");
    }
}
