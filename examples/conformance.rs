//! Differential test against the reference mapper's output (tools/reference/dump.cjs).
//!
//! usage: cargo run --release --example conformance -- <ref.jsonl> [--limit N] [--filter SUBSTR] [--out DIR]

use std::io::{BufRead, BufReader, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value, json};
use vue_ts_mapper::options::{Resolver, default_options};

#[derive(Default)]
struct Stats {
    total: AtomicUsize,
    text: AtomicUsize,
    mappings: AtomicUsize,
    directives: AtomicUsize,
    errors: AtomicUsize,
    panics: AtomicUsize,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("reference jsonl");
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let limit: usize = flag("--limit").map(|s| s.parse().unwrap()).unwrap_or(usize::MAX);
    let filter = flag("--filter");
    let out_dir = flag("--out");
    if let Some(d) = &out_dir {
        std::fs::create_dir_all(d).unwrap();
    }

    // the dump's mapper entry options: `{ typesRoot: '/vue-ts-mapper-rs/types', target: 99 }`
    let defaults = default_options(99.0, "vue", "");
    let mut resolver = Resolver::default();
    let raw: Map<String, Value> =
        serde_json::from_value(json!({ "typesRoot": "/vue-ts-mapper-rs/types", "target": 99 })).unwrap();
    let cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
    resolver.add_config(&raw, &cwd, &|_| None);
    let mut base = defaults.clone();
    base.target = resolver.target.unwrap_or(defaults.target);
    base.types_root = resolver.types_root.clone().unwrap_or(defaults.types_root.clone());
    let options = resolver.build(&base);

    let reader = BufReader::new(std::fs::File::open(path).unwrap());
    let lines: Vec<String> = reader
        .lines()
        .map(Result::unwrap)
        .filter(|l| filter.as_ref().is_none_or(|f| l.contains(f.as_str())))
        .take(limit)
        .collect();

    let stats = Stats::default();
    let failures: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(line) = lines.get(i) else { break };
                    let r: Value = serde_json::from_str(line).unwrap();
                    let file = r["file"].as_str().unwrap().to_string();
                    if r.get("error").is_some() {
                        continue;
                    }
                    stats.total.fetch_add(1, Ordering::Relaxed);
                    let content = std::fs::read_to_string(&file).unwrap();
                    let result = std::panic::catch_unwind(|| vue_ts_mapper::transform(&file, &content, &options, true));
                    let res = match result {
                        Ok(Ok(res)) => res,
                        Ok(Err(e)) => {
                            stats.errors.fetch_add(1, Ordering::Relaxed);
                            failures.lock().unwrap().push((file, format!("error: {e}")));
                            continue;
                        }
                        Err(_) => {
                            stats.panics.fetch_add(1, Ordering::Relaxed);
                            failures.lock().unwrap().push((file, "panic".into()));
                            continue;
                        }
                    };
                    let ref_text = r["text"].as_str().unwrap();
                    if res.text != ref_text || res.extension != r["extension"].as_str().unwrap() {
                        stats.text.fetch_add(1, Ordering::Relaxed);
                        failures.lock().unwrap().push((file.clone(), text_diff(ref_text, &res.text)));
                        if let Some(d) = &out_dir {
                            let base = file.replace('/', "_");
                            std::fs::write(format!("{d}/{base}.ref.ts"), ref_text).unwrap();
                            std::fs::write(format!("{d}/{base}.rs.ts"), &res.text).unwrap();
                        }
                        continue;
                    }
                    let mappings: Vec<Value> = res
                        .mappings
                        .iter()
                        .map(|m| json!([m.0, m.1, m.2, m.3, m.4, m.5]))
                        .collect();
                    if Value::Array(mappings.clone()) != r["mappings"] {
                        stats.mappings.fetch_add(1, Ordering::Relaxed);
                        let empty = vec![];
                        let reference = r["mappings"].as_array().unwrap_or(&empty);
                        failures.lock().unwrap().push((file.clone(), list_diff("mapping", reference, &mappings)));
                        continue;
                    }
                    let directives: Vec<Value> =
                        res.directives.iter().map(|d| json!([d.0, d.1, d.2, d.3, d.4])).collect();
                    if Value::Array(directives.clone()) != r["directives"] {
                        stats.directives.fetch_add(1, Ordering::Relaxed);
                        let empty = vec![];
                        let reference = r["directives"].as_array().unwrap_or(&empty);
                        failures.lock().unwrap().push((file.clone(), list_diff("directive", reference, &directives)));
                    }
                }
            });
        }
    });

    let mut failures = failures.into_inner().unwrap();
    failures.sort();
    let mut stdout = std::io::stdout().lock();
    for (file, why) in failures.iter().take(40) {
        writeln!(stdout, "--- {file}\n{why}").unwrap();
    }
    let total = stats.total.load(Ordering::Relaxed);
    let bad = failures.len();
    writeln!(
        stdout,
        "\n{total} files: {} identical, text {}, mappings {}, directives {}, errors {}, panics {}",
        total - bad,
        stats.text.load(Ordering::Relaxed),
        stats.mappings.load(Ordering::Relaxed),
        stats.directives.load(Ordering::Relaxed),
        stats.errors.load(Ordering::Relaxed),
        stats.panics.load(Ordering::Relaxed),
    )
    .unwrap();
}

/// The first differing line with a little context.
fn text_diff(reference: &str, ours: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (reference.lines().collect(), ours.lines().collect());
    let i = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let from = i.saturating_sub(2);
    let mut s = format!("text differs at line {} (ref {} lines, ours {})\n", i + 1, a.len(), b.len());
    for (tag, lines) in [("ref", &a), ("rs ", &b)] {
        for (k, l) in lines.iter().enumerate().skip(from).take(5) {
            let l: String = l.chars().take(220).collect();
            s.push_str(&format!("  {tag} {:>5}: {l}\n", k + 1));
        }
    }
    s
}

fn list_diff(what: &str, reference: &[Value], ours: &[Value]) -> String {
    let i = reference.iter().zip(ours).position(|(x, y)| x != y).unwrap_or(reference.len().min(ours.len()));
    let show = |v: &[Value]| v.iter().skip(i.saturating_sub(1)).take(4).map(|x| x.to_string()).collect::<Vec<_>>().join(" ");
    format!(
        "{what}s differ at #{i} (ref {}, ours {})\n  ref: {}\n  rs : {}",
        reference.len(),
        ours.len(),
        show(reference),
        show(ours)
    )
}
