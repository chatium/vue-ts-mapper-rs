//! Project configuration for the content-mapper protocol: the Vue options of a tsconfig's
//! `extends` chain (`createParsedCommandLine`), the mapper entry's `options` on top, the files that
//! went into them, and the option diagnostics (`content-mapper/mapper.ts`, `optionDiagnostics.ts`).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::options::{Resolver, VueOptions, default_options};
use crate::paths;

pub struct Project {
    pub vue: VueOptions,
    pub language_features: bool,
}

pub struct Opened {
    pub project: Project,
    pub result: Value,
}

/// Reads files the way the reference's watching host does: every absolute file that exists is a
/// dependency of the configuration.
struct Host {
    watched: RefCell<BTreeSet<String>>,
}

impl Host {
    fn read(&self, file: &str) -> Option<String> {
        let content = std::fs::read_to_string(file).ok()?;
        if file.starts_with('/') {
            self.watched.borrow_mut().insert(paths::normalize(file));
        }
        Some(content)
    }

    /// `resolveVueVersion(folder)`: `node_modules/vue/package.json` upwards from `folder`.
    fn vue_version(&self, folder: &str) -> Option<f64> {
        let mut dir = folder.to_string();
        loop {
            let candidate = paths::join(&dir, "node_modules/vue/package.json");
            if let Some(content) = self.read(&candidate) {
                let version = serde_json::from_str::<Value>(&content).ok()?["version"].as_str()?.to_string();
                let mut parts = version.split('.');
                let (major, minor) = (parts.next()?, parts.next().unwrap_or(""));
                // `Number(major + '.' + minor)`
                return format!("{major}.{minor}").parse().ok();
            }
            let parent = paths::dirname(&dir);
            if parent == dir {
                return None;
            }
            dir = parent;
        }
    }
}

/// JSON with comments and trailing commas, as tsconfig files are.
pub fn parse_jsonc(text: &str) -> Option<Value> {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut last_significant = 0usize;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i = (i + 1).min(b.len());
                out.push_str(&text[start..i]);
                last_significant = out.len();
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            c @ (b'}' | b']') => {
                // drop a trailing comma before a closing bracket
                if out.as_bytes().get(last_significant.wrapping_sub(1)) == Some(&b',') {
                    out.truncate(last_significant - 1);
                }
                out.push(c as char);
                last_significant = out.len();
                i += 1;
            }
            c => {
                let ch_len = text[i..].chars().next().map_or(1, char::len_utf8);
                out.push_str(&text[i..i + ch_len]);
                if !c.is_ascii_whitespace() {
                    last_significant = out.len();
                }
                i += ch_len;
            }
        }
    }
    let out = out.trim_start_matches('\u{feff}');
    serde_json::from_str(out).ok()
}

/// TypeScript's `extends` resolution: a path, or a package's `tsconfig.json` / subpath.
fn resolve_extends(host: &Host, spec: &str, dir: &str) -> Option<String> {
    let exists = |p: &str| Path::new(p).is_file();
    if spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') {
        let p = if spec.starts_with('/') { spec.to_string() } else { paths::join(dir, spec) };
        if exists(&p) {
            return Some(p);
        }
        if !p.ends_with(".json") && exists(&format!("{p}.json")) {
            return Some(format!("{p}.json"));
        }
        return None;
    }
    let mut d = dir.to_string();
    loop {
        let base = paths::join(&d, &format!("node_modules/{spec}"));
        for candidate in [base.clone(), format!("{base}.json"), paths::join(&base, "tsconfig.json")] {
            if exists(&candidate) {
                return Some(candidate);
            }
        }
        // a package's `tsconfig` field
        if let Some(pkg) = host.read(&paths::join(&base, "package.json")) {
            if let Some(t) = serde_json::from_str::<Value>(&pkg).ok().and_then(|v| v["tsconfig"].as_str().map(String::from)) {
                let p = paths::join(&base, &t);
                if exists(&p) {
                    return Some(p);
                }
            }
        }
        let parent = paths::dirname(&d);
        if parent == d {
            return None;
        }
        d = parent;
    }
}

/// The configs in TypeScript's read order: the config, then each `extends` depth-first.
fn read_chain(host: &Host, file: &str, seen: &mut Vec<String>, configs: &mut Vec<(String, Value)>) {
    if seen.iter().any(|s| s == file) {
        return;
    }
    seen.push(file.to_string());
    let Some(json) = host.read(file).and_then(|t| parse_jsonc(&t)) else { return };
    let dir = paths::dirname(file);
    let extends: Vec<String> = match &json["extends"] {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        _ => vec![],
    };
    configs.push((file.to_string(), json));
    for e in extends {
        if let Some(p) = resolve_extends(host, &e, &dir) {
            read_chain(host, &p, seen, configs);
        }
    }
}

/// `createParsedCommandLine(ts, host, configFileName).vueOptions`
fn tsconfig_vue_options(host: &Host, config_file: &str, types_root: &str) -> VueOptions {
    let mut configs = Vec::new();
    read_chain(host, config_file, &mut Vec::new(), &mut configs);
    let mut resolver = Resolver::default();
    let version = |dir: &str| host.vue_version(dir);
    for (file, json) in configs.iter().rev() {
        let raw = json["vueCompilerOptions"].as_object().cloned().unwrap_or_default();
        resolver.add_config(&raw, &paths::dirname(file), &version);
    }
    let lib = resolver.options.get("lib").and_then(Value::as_str).unwrap_or("vue").to_string();
    let defaults = default_options(
        resolver.target.unwrap_or(99.0),
        &lib,
        resolver.types_root.as_deref().unwrap_or(types_root),
    );
    resolver.build(&defaults)
}

/// `openProject(params)`
pub fn open(params: &Value, types_root: &str, cwd: &str) -> Result<Opened, String> {
    let config_file = params["configFileName"].as_str().unwrap_or("");
    let options = params["options"].as_object().cloned();
    let host = Host { watched: RefCell::new(BTreeSet::new()) };
    let root_dir = if config_file.is_empty() { cwd.to_string() } else { paths::dirname(config_file) };
    if !config_file.is_empty() {
        host.watched.borrow_mut().insert(paths::normalize(config_file));
    }

    let base = if config_file.is_empty() {
        default_options(99.0, "vue", types_root)
    } else {
        tsconfig_vue_options(&host, config_file, types_root)
    };
    let mut mapper_options: Map<String, Value> = options.clone().unwrap_or_default();
    let language_features = mapper_options.remove("languageFeatures") != Some(Value::Bool(false));
    let mut resolver = Resolver::default();
    resolver.add_config(&mapper_options, &root_dir, &|dir| host.vue_version(dir));
    let mut defaults = base.clone();
    defaults.target = resolver.target.unwrap_or(base.target);
    defaults.types_root = resolver.types_root.clone().unwrap_or(base.types_root.clone());
    let vue = resolver.build(&defaults);

    let watched: Vec<String> = host.watched.borrow().iter().cloned().collect();
    let mut identity = Fnv128::default();
    identity.write(serde_json::to_string(&params["options"]).unwrap_or_default().as_bytes());
    identity.write(serde_json::to_string(&params["compilerOptions"]).unwrap_or_default().as_bytes());
    identity.write(env!("CARGO_PKG_VERSION").as_bytes());
    for f in &watched {
        identity.write(f.as_bytes());
        identity.write(&std::fs::read(f).unwrap_or_default());
    }
    Ok(Opened {
        project: Project { vue, language_features },
        result: json!({
            "configIdentity": identity.hex(),
            "watchedFiles": watched,
            "optionDiagnostics": option_diagnostics(options.as_ref()),
        }),
    })
}

/// 128-bit FNV-1a; the identity only has to change when its inputs do.
struct Fnv128(u128);

impl Default for Fnv128 {
    fn default() -> Self {
        Fnv128(0x6c62272e07bb014262b821756295c58d)
    }
}

impl Fnv128 {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u128;
            self.0 = self.0.wrapping_mul(0x0000000001000000000000000000013B);
        }
        // a separator, so that concatenations differ
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x0000000001000000000000000000013B);
    }

    fn hex(&self) -> String {
        format!("{:032x}", self.0)
    }
}

const UNKNOWN_OPTION: u32 = 1;
const REMOVED_OPTION: u32 = 2;
const INVALID_OPTION_TYPE: u32 = 3;

/// `toOptionDiagnostics(options)`
fn option_diagnostics(options: Option<&Map<String, Value>>) -> Vec<Value> {
    let Some(options) = options else { return vec![] };
    let removed = |k: &str| -> Option<&'static str> {
        Some(match k {
            "strictTemplates" => "Template checking now follows the TypeScript settings for the script's language, so this option has no equivalent.",
            "strictVModel" => "`v-model` is always checked strictly, so this option has no equivalent.",
            "checkUnknownComponents" => "Unknown components are always reported; declare them in the `GlobalComponents` interface instead.",
            "checkUnknownDirectives" => "Unknown directives are always reported; declare them in the `GlobalDirectives` interface instead.",
            "checkUnknownEvents" => "Unknown events are always reported, so this option has no equivalent.",
            "checkUnknownProps" => "Unknown props are always reported, so this option has no equivalent.",
            _ => return None,
        })
    };
    let d = |path: Value, message: String, code: u32| json!({ "path": path, "messageText": message, "code": code });
    let mut out = Vec::new();
    for (key, value) in options {
        if key == "languageFeatures" {
            continue;
        }
        let path = json!([key]);
        if key == "vueCompilerOptions" {
            out.push(d(path, "Options are flattened in v4: pass them directly under the mapper's `options` instead of nesting them in `vueCompilerOptions`.".into(), UNKNOWN_OPTION));
            continue;
        }
        if let Some(hint) = removed(key) {
            out.push(d(path, format!("Option '{key}' was removed in v4. {hint}"), REMOVED_OPTION));
            continue;
        }
        let invalid = |msg: String| d(json!([key]), msg, INVALID_OPTION_TYPE);
        match key.as_str() {
            "target" => {
                if value.as_str() != Some("auto") && !value.is_number() {
                    out.push(invalid("Option 'target' requires a number or 'auto'.".into()));
                }
            }
            "resolveStyleClassNames" => {
                if !value.is_boolean() && value.as_str() != Some("scoped") {
                    out.push(invalid("Option 'resolveStyleClassNames' requires a boolean or 'scoped'.".into()));
                }
            }
            "plugins" => match value.as_array() {
                None => out.push(invalid("Option 'plugins' requires an array.".into())),
                Some(a) if !a.is_empty() => out.push(d(
                    path,
                    "Vue language plugins are JavaScript and are not loaded by vue-ts-mapper-rs.".into(),
                    UNKNOWN_OPTION,
                )),
                _ => {}
            },
            "extensions" | "fallthroughComponentNames" | "dataAttributes" | "htmlAttributes" => {
                if !value.is_array() {
                    out.push(invalid(format!("Option '{key}' requires an array.")));
                }
            }
            "optionsWrapper" => match value.as_array() {
                None => out.push(invalid("Option 'optionsWrapper' requires an array.".into())),
                Some(a) if a.iter().any(|v| !v.is_string()) => {
                    out.push(invalid("Option 'optionsWrapper' requires an array of strings.".into()))
                }
                _ => {}
            },
            "lib" | "typesRoot" => {
                if !value.is_string() {
                    out.push(invalid(format!("Option '{key}' requires a string.")));
                }
            }
            "macros" | "composables" | "experimentalModelPropName" => {
                if !value.is_object() {
                    out.push(invalid(format!("Option '{key}' requires an object.")));
                }
            }
            "jsxSlots" | "strictCssModules" | "inferComponentDollarEl" | "inferComponentDollarRefs"
            | "inferTemplateDollarAttrs" | "inferTemplateDollarEl" | "inferTemplateDollarRefs"
            | "inferTemplateDollarSlots" | "skipTemplateCodegen" | "vapor" | "fallthroughAttributes"
            | "checkRequiredFallthroughAttributes" | "resolveStyleImports" => {
                if !value.is_boolean() {
                    out.push(invalid(format!("Option '{key}' requires a boolean.")));
                }
            }
            _ => out.push(d(path, format!("Unknown option '{key}'."), UNKNOWN_OPTION)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc() {
        let v = parse_jsonc("{\n // c\n \"a\": [1, 2,], /* x */ \"b\": \"//not\",\n}").unwrap();
        assert_eq!(v, json!({ "a": [1, 2], "b": "//not" }));
    }
}
