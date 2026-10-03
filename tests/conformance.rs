//! Snapshots of the language-tools test-workspace SFCs: generated text, span mappings and
//! diagnostic directives. They started as the reference mapper's output (tools/reference/dump.cjs)
//! and now follow vue-tsc 3.3's checking semantics where the two differ; after an intended change,
//! `UPDATE_FIXTURES=1 cargo test --test conformance` rewrites them.

use std::path::Path;

use serde_json::{Value, json};
use vue_ts_mapper::options::default_options;

/// The `/// <reference types>` lines hold a path relative to the file's directory; replace them
/// with a fixed one and shift the generated offsets behind them.
fn canonical(text: &str, mappings: &Value, directives: &Value) -> (String, Value, Value) {
    let mut out = String::new();
    let mut rest = text;
    let mut shifts: Vec<(i64, i64)> = Vec::new(); // (old end of line, delta)
    let mut old_pos = 0i64;
    let mut delta = 0i64;
    while let Some(line) = rest.split_inclusive('\n').next().filter(|l| l.starts_with("/// <reference types=")) {
        let name = line.trim_end().trim_end_matches("\" />").rsplit('/').next().unwrap_or_default();
        let fixed = format!("/// <reference types=\"<types>/{name}\" />\n");
        old_pos += line.encode_utf16().count() as i64;
        delta += fixed.len() as i64 - line.encode_utf16().count() as i64;
        shifts.push((old_pos, delta));
        out.push_str(&fixed);
        rest = &rest[line.len()..];
    }
    out.push_str(rest);
    let shift = |v: i64| -> i64 {
        let d = shifts.iter().rev().find(|(end, _)| v >= *end).map_or(0, |s| s.1);
        v + d
    };
    let mappings = Value::Array(
        mappings
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                let mut m = m.as_array().unwrap().clone();
                m[0] = json!(shift(m[0].as_i64().unwrap()));
                Value::Array(m)
            })
            .collect(),
    );
    let directives = Value::Array(
        directives
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                let mut d = d.as_array().unwrap().clone();
                d[2] = json!(shift(d[2].as_i64().unwrap()));
                d[3] = json!(shift(d[3].as_i64().unwrap()));
                Value::Array(d)
            })
            .collect(),
    );
    (out, mappings, directives)
}

#[test]
fn language_tools_workspace() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let options = default_options(99.0, "vue", "/vue-ts-mapper-rs/types");
    let update = std::env::var_os("UPDATE_FIXTURES").is_some();
    let mut updated = Vec::new();
    let mut failures = Vec::new();
    let mut total = 0;
    for line in std::fs::read_to_string(dir.join("language-tools.jsonl")).unwrap().lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let file = dir.join("language-tools").join(r["file"].as_str().unwrap());
        let file = file.to_string_lossy();
        let content = std::fs::read_to_string(&*file).unwrap();
        total += 1;
        let res = match vue_ts_mapper::transform(&file, &content, &options, true) {
            Ok(res) => res,
            Err(e) => {
                failures.push(format!("{file}: {e}"));
                continue;
            }
        };
        let mappings = json!(res.mappings.iter().map(|m| json!([m.0, m.1, m.2, m.3, m.4, m.5])).collect::<Vec<_>>());
        let directives = json!(res.directives.iter().map(|d| json!([d.0, d.1, d.2, d.3, d.4])).collect::<Vec<_>>());
        if update {
            let mut r = r.clone();
            r["text"] = json!(res.text);
            r["extension"] = json!(res.extension);
            r["mappings"] = mappings;
            r["directives"] = directives;
            updated.push(serde_json::to_string(&r).unwrap());
            continue;
        }
        let ours = canonical(&res.text, &mappings, &directives);
        let reference = canonical(r["text"].as_str().unwrap(), &r["mappings"], &r["directives"]);
        if ours.0 != reference.0 || res.extension != r["extension"] {
            failures.push(format!("{file}: text"));
        } else if ours.1 != reference.1 {
            failures.push(format!("{file}: mappings"));
        } else if ours.2 != reference.2 {
            failures.push(format!("{file}: directives"));
        }
    }
    if update {
        std::fs::write(dir.join("language-tools.jsonl"), updated.join("\n") + "\n").unwrap();
    }
    assert!(failures.is_empty(), "{} of {total} differ:\n{}", failures.len(), failures.join("\n"));
}

/// The virtual code of a valid SFC must parse: TypeScript reports syntax errors in it regardless of
/// directives, and any syntax error stops the semantic check of the whole program.
#[test]
fn virtual_code_parses() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut options = default_options(99.0, "vue", "/vue-ts-mapper-rs/types");
    options.strict_v_model = true;
    let mut files = vec![dir.join("syntax/edge-cases.vue").to_string_lossy().into_owned()];
    for line in std::fs::read_to_string(dir.join("language-tools.jsonl")).unwrap().lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let file = r["file"].as_str().unwrap();
        // `_failed_` fixtures have syntax errors of their own
        if !file.contains("_failed_") {
            files.push(dir.join("language-tools").join(file).to_string_lossy().into_owned());
        }
    }
    let mut failures = Vec::new();
    for file in &files {
        let content = std::fs::read_to_string(file).unwrap();
        let res = vue_ts_mapper::transform(file, &content, &options, true).unwrap();
        let lang = res.extension.trim_start_matches('.');
        let parsed = vue_ts_mapper::ts_ast::parse_block(&res.text, lang);
        if let Some(e) = parsed.errors.first() {
            failures.push(format!("{file}: syntax error at {}", e.0));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
