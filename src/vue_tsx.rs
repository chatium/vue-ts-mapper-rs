//! `plugins/vue-tsx.ts`: one SFC to its virtual script, plus the content-mapper `transform`.

use std::cell::RefCell;

use indexmap::IndexSet;

use crate::code::{Code, Src, codes_to_string};
use crate::codegen::script::{ScriptOptions, generate as generate_script};
use crate::codegen::style::{StyleBlock, StyleOptions, StyleResult, generate as generate_style};
use crate::codegen::template::interpolation::InterpOpts;
use crate::codegen::template::{TemplateOptions, TemplateResult, generate as generate_template};
use crate::codegen::ScriptBlock;
use crate::mappings::{self, DirectiveMapping, SpanMapping};
use crate::options::{Resolver, VueOptions, parse_comment_options};
use crate::parsers::{self, ScriptRanges, ScriptSetupRanges};
use crate::sfc::{self, Block};
use crate::shared::{camelize, capitalize};
use crate::style::{self, StyleInfo};
use crate::template::{self, TemplateAst};
use crate::text::Text;
use crate::{paths, ts_ast};

pub struct TransformResult {
    pub text: String,
    pub extension: &'static str,
    pub mappings: Vec<SpanMapping>,
    pub directives: Vec<DirectiveMapping>,
}

fn is_valid_lang(lang: &str) -> bool {
    matches!(lang, "js" | "jsx" | "ts" | "tsx")
}

/// `computeLang(ir)`
fn compute_lang(script: Option<&Block>, script_setup: Option<&Block>) -> &'static str {
    let mut lang = script_setup.or(script).map(|b| b.lang.as_str());
    if let (Some(s), Some(ss)) = (script, script_setup) {
        lang = Some(if ss.lang != "js" { &ss.lang } else { &s.lang });
    }
    match lang {
        Some("js") => "js",
        Some("jsx") => "jsx",
        Some("tsx") => "tsx",
        _ => "ts",
    }
}

/// The script setup stand-in of an SFC without any script (vuejs/language-tools#2113).
fn empty_script_setup() -> Block {
    Block {
        name: Src::ScriptSetup,
        tag: "script".into(),
        lang: "ts".into(),
        attrs: Default::default(),
        content: String::new(),
        start: 0,
        end: 0,
        start_tag_end: 0,
        end_tag_start: 0,
        src: None,
        generic: None,
        module: None,
        scoped: false,
        setup: None,
    }
}

fn script_block(block: &Block) -> ScriptBlock<'_> {
    let parsed = if is_valid_lang(&block.lang) {
        ts_ast::parse_block(&block.content, &block.lang)
    } else {
        ts_ast::parse("", false)
    };
    ScriptBlock { block, text: Text::new(&block.content), parsed }
}

/// Generates the virtual script of a `.vue` file and maps it back.
pub fn transform(file_name: &str, content: &str, base: &VueOptions, language_features: bool) -> Result<TransformResult, String> {
    let sfc = sfc::parse(content);

    let resolved;
    let vue: &VueOptions = match parse_comment_options(&sfc.comments) {
        Some(raw) => {
            // comment options cannot resolve `target: "auto"`
            let mut r = Resolver::default();
            r.add_config(&raw, &paths::dirname(file_name), &|_| None);
            resolved = r.build(base);
            &resolved
        }
        None => base,
    };

    let template_block = sfc.template.as_ref();
    let script_setup_block = match (&sfc.script, &sfc.script_setup) {
        (None, None) => Some(empty_script_setup()),
        (_, ss) => ss.clone(),
    };
    let script = sfc.script.as_ref().map(script_block);
    let script_setup = script_setup_block.as_ref().map(script_block);
    let lang = compute_lang(sfc.script.as_ref(), script_setup_block.as_ref());
    let is_vapor = vue.vapor
        || script_setup_block.as_ref().is_some_and(|b| b.has_truthy_attr("vapor"))
        || template_block.is_some_and(|b| b.has_truthy_attr("vapor"));

    let script_ranges: Option<ScriptRanges> = script
        .as_ref()
        .filter(|s| is_valid_lang(&s.block.lang))
        .map(|s| parsers::parse_script_ranges(&parsers::Src::new(&s.block.content, &s.parsed), vue));
    let script_setup_ranges: Option<ScriptSetupRanges> = script_setup
        .as_ref()
        .filter(|s| is_valid_lang(&s.block.lang))
        .map(|s| parsers::parse_script_setup_ranges(&parsers::Src::new(&s.block.content, &s.parsed), vue));

    // binding sets, in the reference's insertion order
    let mut imported_components = IndexSet::new();
    let mut setup_bindings = IndexSet::new();
    let mut non_flowing_bindings = IndexSet::new();
    let mut script_setup_bindings = IndexSet::new();
    if let (Some(ss), Some(ssr)) = (&script_setup, &script_setup_ranges) {
        let text = |sb: &ScriptBlock, r: &parsers::Range| sb.text.slice(r.start, r.end).to_string();
        let sr = script.as_ref().zip(script_ranges.as_ref());
        for r in &ssr.bindings.components {
            imported_components.insert(text(ss, r));
        }
        for r in &ssr.bindings.bindings {
            setup_bindings.insert(text(ss, r));
            script_setup_bindings.insert(text(ss, r));
        }
        for r in &ssr.bindings.non_flowing_bindings {
            non_flowing_bindings.insert(text(ss, r));
        }
        if let Some((s, sr)) = sr {
            for r in &sr.bindings.components {
                imported_components.insert(text(s, r));
            }
            for r in &sr.bindings.bindings {
                setup_bindings.insert(text(s, r));
            }
            for r in &sr.bindings.non_flowing_bindings {
                non_flowing_bindings.insert(text(s, r));
            }
        }
    }
    let define_props = script_setup_ranges.as_ref().and_then(|r| r.define_props.as_ref());
    let mut destructured_props: IndexSet<String> =
        define_props.and_then(|d| d.destructured.as_ref()).map(|d| d.keys().cloned().collect()).unwrap_or_default();
    if let Some(rest) = define_props.and_then(|d| d.destructured_rest.clone()) {
        destructured_props.insert(rest);
    }
    let setup_refs: IndexSet<String> = script_setup_ranges
        .as_ref()
        .map(|r| r.use_template_ref.iter().filter_map(|u| u.name.clone()).collect())
        .unwrap_or_default();
    let has_define_slots = script_setup_ranges.as_ref().is_some_and(|r| r.define_slots.is_some());
    let props_assign_name = define_props.and_then(|d| d.name.as_deref());
    let slots_assign_name =
        script_setup_ranges.as_ref().and_then(|r| r.define_slots.as_ref()).and_then(|d| d.name.as_deref());
    let component_options = script_ranges.as_ref().and_then(|r| r.export_default.as_ref()).and_then(|e| e.options.as_ref());
    let inherit_attrs = script_setup_ranges
        .as_ref()
        .and_then(|r| r.define_options.as_ref())
        .and_then(|d| d.inherit_attrs.as_deref())
        .or(component_options.and_then(|o| o.inherit_attrs.as_deref()))
        != Some("false");
    let component_name = {
        let define_options_name =
            script_setup_ranges.as_ref().and_then(|r| r.define_options.as_ref()).and_then(|d| d.name.as_deref());
        let name = match (&script, component_options.and_then(|o| o.name)) {
            (Some(s), Some(n)) => s.text.slice(n.start + 1, n.end.saturating_sub(1)).to_string(),
            _ => match define_options_name.filter(|n| !n.is_empty() && script_setup_block.is_some()) {
                Some(n) => n.to_string(),
                None => {
                    let base_name = paths::basename(file_name);
                    let t = Text::new(base_name);
                    // `baseName.slice(0, baseName.lastIndexOf('.'))`
                    match base_name.rfind('.') {
                        Some(i) => base_name[..i].to_string(),
                        None => t.slice(0, t.len().saturating_sub(1)).to_string(),
                    }
                }
            },
        };
        capitalize(&camelize(&name))
    };

    let template_ast: Option<TemplateAst> =
        template_block.filter(|t| t.lang == "html" || t.lang == "md").map(|t| template::compile(&t.content));
    let style_infos: Vec<StyleInfo> = sfc.styles.iter().map(|s| style::parse(&s.content)).collect();
    let style_blocks: Vec<StyleBlock> =
        sfc.styles.iter().zip(&style_infos).map(|(block, info)| StyleBlock { block, info }).collect();

    let expr_cache = Default::default();
    let template_pass = |dot_value_bindings: &IndexSet<String>| -> Option<TemplateResult> {
        let tb = template_block.filter(|_| !vue.skip_template_codegen)?;
        let reassert_bindings: IndexSet<String> =
            dot_value_bindings.iter().filter(|n| non_flowing_bindings.contains(*n)).cloned().collect();
        Some(generate_template(&TemplateOptions {
            vue,
            template: tb,
            text: Text::new(&tb.content),
            ast: template_ast.as_ref(),
            is_vapor,
            script_lang: lang,
            component_name: &component_name,
            destructured_props: &destructured_props,
            imported_components: &imported_components,
            setup_refs: &setup_refs,
            setup_bindings: &setup_bindings,
            dot_value_bindings,
            reassert_bindings: &reassert_bindings,
            has_define_slots,
            props_assign_name,
            slots_assign_name,
            inherit_attrs,
            expr_cache: &expr_cache,
        }))
    };
    let style_pass = |dot_value_bindings: &IndexSet<String>| -> Option<StyleResult> {
        if style_blocks.is_empty() {
            return None;
        }
        Some(generate_style(&StyleOptions {
            vue,
            styles: &style_blocks,
            interp: InterpOpts {
                destructured_props: &destructured_props,
                imported_components: &imported_components,
                setup_refs: &setup_refs,
                setup_bindings: &setup_bindings,
                dot_value_bindings,
                lib: &vue.lib,
                script_lang: lang,
                cache: &expr_cache,
            },
        }))
    };

    // First pass: collect the bindings used in narrowing positions (output discarded); the second
    // pass emits every access of these with `.value`.
    let dot_value_bindings: IndexSet<String> = if setup_bindings.is_empty() {
        IndexSet::new()
    } else {
        let empty = IndexSet::new();
        let mut names = IndexSet::new();
        if let Some(t) = template_pass(&empty) {
            names.extend(t.dot_value_accesses);
        }
        if let Some(s) = style_pass(&empty) {
            names.extend(s.dot_value_accesses);
        }
        names
    };
    let generated_template = template_pass(&dot_value_bindings);
    let generated_style = style_pass(&dot_value_bindings);

    let local_components: IndexSet<String> = if setup_bindings.is_empty() {
        IndexSet::new()
    } else {
        template_ast
            .as_ref()
            .map(|ast| ast.components.as_slice())
            .unwrap_or_default()
            .iter()
            .flat_map(|name| {
                let c = camelize(name);
                let p = capitalize(&c);
                [c, p]
            })
            .filter(|n| setup_bindings.contains(n))
            .collect()
    };
    let local_directives: IndexSet<String> = setup_bindings
        .iter()
        .filter(|n| {
            let b = n.as_bytes();
            b.len() >= 2 && b[0] == b'v' && b[1].is_ascii_uppercase()
        })
        .cloned()
        .collect();

    let mut types: IndexSet<String> = IndexSet::new();
    let mut template_and_style_codes: Vec<Code> = Vec::new();
    if let Some(t) = &generated_template {
        types.extend(t.generated_types.iter().cloned());
    }
    if let Some(s) = generated_style {
        types.extend(s.generated_types);
        template_and_style_codes.extend(s.codes);
    }
    if let Some(t) = generated_template {
        template_and_style_codes.extend(t.codes);
    }

    let codes = generate_script(&ScriptOptions {
        vue,
        file_name,
        script: script.as_ref(),
        script_setup: script_setup.as_ref(),
        script_lang: lang,
        script_ranges: script_ranges.as_ref(),
        script_setup_ranges: script_setup_ranges.as_ref(),
        template_and_style_types: &types,
        template_and_style_codes: RefCell::new(template_and_style_codes),
        setup_bindings: &script_setup_bindings,
        local_components: &local_components,
        local_directives: &local_directives,
        dot_value_bindings: &dot_value_bindings,
    });

    let text = codes_to_string(&codes);
    let block_start = |src: Src| -> Option<usize> {
        match src {
            Src::Main => None,
            Src::Template => template_block.map(|b| b.start_tag_end),
            Src::Script => sfc.script.as_ref().map(|b| b.start_tag_end),
            Src::ScriptSetup => script_setup_block.as_ref().map(|b| b.start_tag_end),
            Src::Style(i) => sfc.styles.get(i as usize).map(|b| b.start_tag_end),
        }
    };
    let volar = mappings::build(&codes, block_start);
    let generated = Text::new(&text);
    let original = Text::new(content);
    let spans = mappings::to_span_mappings(&volar, &generated, &original, language_features);
    let directives = mappings::to_diagnostic_directives(&volar)?;
    let directives = mappings::with_synthesized_ignores(generated.len() as i64, &spans, &directives);

    Ok(TransformResult {
        text,
        extension: match lang {
            "js" => ".js",
            "jsx" => ".jsx",
            "tsx" => ".tsx",
            _ => ".ts",
        },
        mappings: spans,
        directives,
    })
}
