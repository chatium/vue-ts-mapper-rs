//! `VueCompilerOptions`: defaults, the raw (JSON) option layers and their resolution
//! (`CompilerOptionsResolver` / `getDefaultCompilerOptions` / `parseVueCompilerOptions`).

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::shared::{camelize, hyphenate};

#[derive(Clone, Debug, PartialEq)]
pub enum StyleClassNames {
    Bool(bool),
    Scoped,
}

/// A value of `experimentalModelPropName[name][tag]`.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelPropTag {
    Bool(bool),
    Attrs(Vec<IndexMap<String, String>>),
    /// anything else (JS treats it as neither `true` nor an object)
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Macros {
    pub define_props: Vec<String>,
    pub define_slots: Vec<String>,
    pub define_emits: Vec<String>,
    pub define_expose: Vec<String>,
    pub define_model: Vec<String>,
    pub define_options: Vec<String>,
    pub with_defaults: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Composables {
    pub use_attrs: Vec<String>,
    pub use_css_module: Vec<String>,
    pub use_slots: Vec<String>,
    pub use_template_ref: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VueOptions {
    pub target: f64,
    pub lib: String,
    pub types_root: String,
    pub extensions: Vec<String>,
    pub jsx_slots: bool,
    pub strict_css_modules: bool,
    pub infer_component_dollar_el: bool,
    pub infer_component_dollar_refs: bool,
    pub infer_template_dollar_attrs: bool,
    pub infer_template_dollar_el: bool,
    pub infer_template_dollar_refs: bool,
    pub infer_template_dollar_slots: bool,
    pub skip_template_codegen: bool,
    pub vapor: bool,
    pub fallthrough_attributes: bool,
    pub check_required_fallthrough_attributes: bool,
    pub resolve_style_imports: bool,
    pub resolve_style_class_names: StyleClassNames,
    pub fallthrough_component_names: Vec<String>,
    pub data_attributes: Vec<String>,
    pub html_attributes: Vec<String>,
    pub options_wrapper: Vec<String>,
    pub macros: Macros,
    pub composables: Composables,
    pub experimental_model_prop_name: IndexMap<String, IndexMap<String, ModelPropTag>>,
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `getDefaultCompilerOptions(target, lib, typesRoot)`
pub fn default_options(target: f64, lib: &str, types_root: &str) -> VueOptions {
    let mut model = IndexMap::new();
    model.insert("".to_string(), IndexMap::from([("input".to_string(), ModelPropTag::Bool(true))]));
    model.insert(
        "value".to_string(),
        IndexMap::from([
            (
                "input".to_string(),
                ModelPropTag::Attrs(vec![IndexMap::from([("type".to_string(), "text".to_string())])]),
            ),
            ("textarea".to_string(), ModelPropTag::Bool(true)),
            ("select".to_string(), ModelPropTag::Bool(true)),
        ]),
    );
    VueOptions {
        target,
        lib: lib.to_string(),
        types_root: types_root.to_string(),
        extensions: strings(&[".vue"]),
        jsx_slots: false,
        strict_css_modules: false,
        infer_component_dollar_el: false,
        infer_component_dollar_refs: false,
        infer_template_dollar_attrs: false,
        infer_template_dollar_el: false,
        infer_template_dollar_refs: false,
        infer_template_dollar_slots: false,
        skip_template_codegen: false,
        vapor: false,
        fallthrough_attributes: false,
        check_required_fallthrough_attributes: false,
        resolve_style_imports: false,
        resolve_style_class_names: StyleClassNames::Scoped,
        fallthrough_component_names: strings(&["Transition", "KeepAlive", "Teleport", "Suspense"]),
        data_attributes: vec![],
        html_attributes: strings(&["aria-*"]),
        options_wrapper: vec![format!("(await import('{lib}')).defineComponent("), ")".to_string()],
        macros: Macros {
            define_props: strings(&["defineProps"]),
            define_slots: strings(&["defineSlots"]),
            define_emits: strings(&["defineEmits"]),
            define_expose: strings(&["defineExpose"]),
            define_model: strings(&["defineModel"]),
            define_options: strings(&["defineOptions"]),
            with_defaults: strings(&["withDefaults"]),
        },
        composables: Composables {
            use_attrs: strings(&["useAttrs"]),
            use_css_module: strings(&["useCssModule"]),
            use_slots: strings(&["useSlots"]),
            use_template_ref: strings(&["useTemplateRef", "templateRef"]),
        },
        experimental_model_prop_name: model,
    }
}

/// JS truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn string_list(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().map(js_string).collect())
}

/// `String(value)` for the JSON values options carry.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

fn parse_model_prop_name(v: &Value) -> IndexMap<String, IndexMap<String, ModelPropTag>> {
    let mut out = IndexMap::new();
    let Some(obj) = v.as_object() else { return out };
    for (name, tags) in obj {
        let mut t = IndexMap::new();
        if let Some(tags) = tags.as_object() {
            for (tag, val) in tags {
                let parsed = match val {
                    Value::Bool(b) => ModelPropTag::Bool(*b),
                    Value::Object(o) => ModelPropTag::Attrs(vec![attrs_of(o)]),
                    Value::Array(a) => {
                        ModelPropTag::Attrs(a.iter().map(|x| x.as_object().map(attrs_of).unwrap_or_default()).collect())
                    }
                    _ => ModelPropTag::Other,
                };
                t.insert(tag.clone(), parsed);
            }
        }
        out.insert(name.clone(), t);
    }
    out
}

fn attrs_of(o: &Map<String, Value>) -> IndexMap<String, String> {
    o.iter().map(|(k, v)| (k.clone(), js_string(v))).collect()
}

/// `CompilerOptionsResolver`: accumulates raw option layers, later layers win.
#[derive(Default)]
pub struct Resolver {
    /// raw options other than target / typesRoot / plugins, in insertion order
    pub options: Map<String, Value>,
    pub target: Option<f64>,
    pub types_root: Option<String>,
}

impl Resolver {
    /// `addConfig(options, rootDir)`. `resolve_vue_version` stands in for reading
    /// `node_modules/vue/package.json` upwards from `root_dir`.
    pub fn add_config(&mut self, raw: &Map<String, Value>, root_dir: &str, resolve_vue_version: &dyn Fn(&str) -> Option<f64>) {
        for (key, value) in raw {
            match key.as_str() {
                "target" => {
                    if value.as_str() == Some("auto") {
                        self.target = resolve_vue_version(root_dir);
                    } else {
                        self.target = value.as_f64();
                    }
                }
                "typesRoot" => {
                    if let Some(s) = value.as_str() {
                        self.types_root = Some(if s.starts_with('/') {
                            s.to_string()
                        } else {
                            crate::paths::join(root_dir, s)
                        });
                    }
                }
                // Vue language plugins are JavaScript; the Rust mapper cannot load them
                "plugins" => {}
                _ => {
                    self.options.insert(key.clone(), value.clone());
                }
            }
        }
        if !raw.contains_key("target") && self.target.is_none() {
            self.target = resolve_vue_version(root_dir);
        }
    }

    /// `build(defaults)`
    pub fn build(&self, defaults: &VueOptions) -> VueOptions {
        let mut o = defaults.clone();
        let opts = &self.options;
        let flag = |key: &str, field: &mut bool| {
            if let Some(v) = opts.get(key) {
                *field = truthy(v);
            }
        };
        flag("jsxSlots", &mut o.jsx_slots);
        flag("strictCssModules", &mut o.strict_css_modules);
        flag("inferComponentDollarEl", &mut o.infer_component_dollar_el);
        flag("inferComponentDollarRefs", &mut o.infer_component_dollar_refs);
        flag("inferTemplateDollarAttrs", &mut o.infer_template_dollar_attrs);
        flag("inferTemplateDollarEl", &mut o.infer_template_dollar_el);
        flag("inferTemplateDollarRefs", &mut o.infer_template_dollar_refs);
        flag("inferTemplateDollarSlots", &mut o.infer_template_dollar_slots);
        flag("skipTemplateCodegen", &mut o.skip_template_codegen);
        flag("vapor", &mut o.vapor);
        flag("fallthroughAttributes", &mut o.fallthrough_attributes);
        flag("checkRequiredFallthroughAttributes", &mut o.check_required_fallthrough_attributes);
        flag("resolveStyleImports", &mut o.resolve_style_imports);
        if let Some(v) = opts.get("lib") {
            o.lib = js_string(v);
        }
        if let Some(v) = opts.get("resolveStyleClassNames") {
            o.resolve_style_class_names = match v {
                Value::String(s) if s == "scoped" => StyleClassNames::Scoped,
                other => StyleClassNames::Bool(truthy(other)),
            };
        }
        if let Some(l) = opts.get("extensions").and_then(string_list) {
            o.extensions = l;
        }
        if let Some(l) = opts.get("dataAttributes").and_then(string_list) {
            o.data_attributes = l;
        }
        if let Some(l) = opts.get("htmlAttributes").and_then(string_list) {
            o.html_attributes = l;
        }
        if let Some(l) = opts.get("optionsWrapper").and_then(string_list) {
            o.options_wrapper = l;
        }
        if let Some(m) = opts.get("macros").and_then(Value::as_object) {
            let set = |k: &str, f: &mut Vec<String>| {
                if let Some(l) = m.get(k).and_then(string_list) {
                    *f = l;
                }
            };
            set("defineProps", &mut o.macros.define_props);
            set("defineSlots", &mut o.macros.define_slots);
            set("defineEmits", &mut o.macros.define_emits);
            set("defineExpose", &mut o.macros.define_expose);
            set("defineModel", &mut o.macros.define_model);
            set("defineOptions", &mut o.macros.define_options);
            set("withDefaults", &mut o.macros.with_defaults);
        }
        if let Some(m) = opts.get("composables").and_then(Value::as_object) {
            let set = |k: &str, f: &mut Vec<String>| {
                if let Some(l) = m.get(k).and_then(string_list) {
                    *f = l;
                }
            };
            set("useAttrs", &mut o.composables.use_attrs);
            set("useCssModule", &mut o.composables.use_css_module);
            set("useSlots", &mut o.composables.use_slots);
            set("useTemplateRef", &mut o.composables.use_template_ref);
        }
        let mut names = defaults.fallthrough_component_names.clone();
        if let Some(l) = opts.get("fallthroughComponentNames").and_then(string_list) {
            names.extend(l);
        }
        o.fallthrough_component_names = names.iter().map(|n| hyphenate(n)).collect();
        let model = match opts.get("experimentalModelPropName") {
            Some(v) => parse_model_prop_name(v),
            None => defaults.experimental_model_prop_name.clone(),
        };
        o.experimental_model_prop_name = model.into_iter().map(|(k, v)| (camelize(&k), v)).collect();
        o
    }
}

/// `parseVueCompilerOptions(comments)`: `<!-- @key value -->` root comments of the SFC.
pub fn parse_comment_options(comments: &[String]) -> Option<Map<String, Value>> {
    let mut out = Map::new();
    for text in comments {
        // `/^\s*@(?<key>.+?)\s+(?<value>.+?)\s*$/m`: the first line that matches
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let l = line.trim_start();
            let Some(rest) = l.strip_prefix('@') else { continue };
            let Some(sp) = rest.find(|c: char| c.is_whitespace()) else { continue };
            let key = &rest[..sp];
            let value = rest[sp..].trim();
            if key.is_empty() || value.is_empty() {
                continue;
            }
            // only the first match is tried; invalid JSON drops the comment
            if let Ok(v) = serde_json::from_str::<Value>(value) {
                out.insert(key.to_string(), v);
            }
            break;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}
