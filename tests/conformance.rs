//! The language-tools test-workspace SFCs against the reference mapper's output
//! (tools/reference/dump.cjs): generated text, span mappings and diagnostic directives must be
//! identical.

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
        let name = if line.contains("/vue-3.4-shims.d.ts") { "vue-3.4-shims" } else { "template-helpers" };
        let fixed = format!("/// <reference types=\"<types>/{name}.d.ts\" />\n");
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
        let ours = canonical(
            &res.text,
            &json!(res.mappings.iter().map(|m| json!([m.0, m.1, m.2, m.3, m.4, m.5])).collect::<Vec<_>>()),
            &json!(res.directives.iter().map(|d| json!([d.0, d.1, d.2, d.3, d.4])).collect::<Vec<_>>()),
        );
        let reference = canonical(r["text"].as_str().unwrap(), &r["mappings"], &r["directives"]);
        if ours.0 != reference.0 || res.extension != r["extension"] {
            failures.push(format!("{file}: text"));
        } else if ours.1 != reference.1 {
            failures.push(format!("{file}: mappings"));
        } else if ours.2 != reference.2 {
            failures.push(format!("{file}: directives"));
        }
    }
    assert!(failures.is_empty(), "{} of {total} differ:\n{}", failures.len(), failures.join("\n"));
}
