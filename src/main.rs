//! The TypeScript 7 content-mapper protocol over stdio: `Content-Length`-framed JSON-RPC with
//! `initialize`, `openProject`, `closeProject` and `transform`. Transforms run on a thread pool
//! and answer out of order.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex, RwLock, mpsc};

use serde_json::{Value, json};
use vue_ts_mapper::project::{self, Project};

struct State {
    projects: RwLock<HashMap<String, Arc<Project>>>,
    types_root: String,
    cwd: String,
    out: Mutex<std::io::Stdout>,
}

fn main() {
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let arg = |name: &str| {
        let prefix = format!("--{name}=");
        std::env::args().find_map(|a| a.strip_prefix(&prefix).map(String::from))
    };
    // tsgo starts mappers in their package directory, which ships the helper typings
    let types_root = arg("types-root").unwrap_or_else(|| format!("{cwd}/types"));
    let state = Arc::new(State {
        projects: RwLock::new(HashMap::new()),
        types_root,
        cwd,
        out: Mutex::new(std::io::stdout()),
    });

    let threads = std::env::var("VUE_TS_MAPPER_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    let (tx, rx) = mpsc::channel::<Value>();
    let rx = Arc::new(Mutex::new(rx));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let rx = rx.clone();
            let state = state.clone();
            std::thread::spawn(move || {
                loop {
                    let message = match rx.lock().unwrap().recv() {
                        Ok(m) => m,
                        Err(_) => break,
                    };
                    let response = respond(&message, transform(&state, &message["params"]));
                    write(&state, &response);
                }
            })
        })
        .collect();

    let mut input = BufReader::new(std::io::stdin().lock());
    while let Some(message) = read_message(&mut input) {
        match message["method"].as_str() {
            // order matters for the project lifecycle: handled in sequence
            Some("initialize") => write(&state, &respond(&message, initialize(&message["params"]))),
            Some("openProject") => {
                let result = project::open(&message["params"], &state.types_root, &state.cwd).map(|opened| {
                    let handle = message["params"]["projectHandle"].as_str().unwrap_or_default().to_string();
                    state.projects.write().unwrap().insert(handle, Arc::new(opened.project));
                    opened.result
                });
                write(&state, &respond(&message, result));
            }
            Some("closeProject") => {
                if let Some(h) = message["params"]["projectHandle"].as_str() {
                    state.projects.write().unwrap().remove(h);
                }
                write(&state, &respond(&message, Ok(Value::Null)));
            }
            Some("transform") => tx.send(message).unwrap(),
            other => {
                let error = json!({ "code": -32601, "message": format!("Unknown method {}", other.unwrap_or("")) });
                write(&state, &json!({ "jsonrpc": "2.0", "id": message["id"], "error": error }));
            }
        }
    }
    drop(tx);
    for w in workers {
        let _ = w.join();
    }
}

fn read_message(input: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = vec![0; length?];
    input.read_exact(&mut body).ok()?;
    match serde_json::from_slice(&body) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("vue-ts-mapper-rs: invalid message: {e}");
            Some(Value::Null)
        }
    }
}

fn write(state: &State, message: &Value) {
    let body = serde_json::to_vec(message).unwrap();
    let mut out = state.out.lock().unwrap();
    let _ = write!(out, "Content-Length: {}\r\n\r\n", body.len());
    let _ = out.write_all(&body);
    let _ = out.flush();
}

fn respond(message: &Value, result: Result<Value, String>) -> Value {
    match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": message["id"], "error": { "code": -32603, "message": e } }),
    }
}

fn initialize(params: &Value) -> Result<Value, String> {
    let utf16 = params["positionEncodings"].as_array().is_some_and(|a| a.iter().any(|e| e == "utf-16"));
    if !utf16 {
        return Err("vue-ts-mapper-rs requires UTF-16 position encoding".into());
    }
    Ok(json!({ "positionEncoding": "utf-16", "diagnosticSource": "vue" }))
}

fn transform(state: &State, params: &Value) -> Result<Value, String> {
    let file_name = params["fileName"].as_str().ok_or("missing fileName")?;
    let content = params["content"].as_str().ok_or("missing content")?;
    let project = match params["projectHandle"].as_str() {
        Some(h) => state
            .projects
            .read()
            .unwrap()
            .get(h)
            .cloned()
            .ok_or_else(|| format!("Unknown vue-ts-mapper-rs project handle: {h}"))?,
        None => {
            // a file outside any project: the nearest tsconfig.json, like the reference
            let config = find_config(file_name).unwrap_or_default();
            let opened = project::open(&json!({ "configFileName": config }), &state.types_root, &state.cwd)?;
            Arc::new(opened.project)
        }
    };
    let r = std::panic::catch_unwind(|| vue_ts_mapper::transform(file_name, content, &project.vue, project.language_features))
        .map_err(|e| {
            let message = e.downcast_ref::<String>().cloned().or(e.downcast_ref::<&str>().map(|s| s.to_string()));
            format!("vue-ts-mapper-rs panicked on {file_name}: {}", message.unwrap_or_default())
        })??;
    Ok(json!({
        "text": r.text,
        "extension": r.extension,
        "mappings": r.mappings,
        "diagnosticDirectives": {
            "unusedExpectDirectiveDiagnostics": [{ "code": 2578, "messageText": "Unused '@ts-expect-error' directive." }],
            "directives": r.directives,
        },
    }))
}

fn find_config(file_name: &str) -> Option<String> {
    let mut dir = std::path::Path::new(file_name).parent()?;
    loop {
        let candidate = dir.join("tsconfig.json");
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
        dir = dir.parent()?;
    }
}
