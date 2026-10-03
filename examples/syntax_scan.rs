//! Lists SFCs whose virtual code does not parse: `cargo run --release --example syntax_scan -- <file.vue>...`
fn main() {
    let options = vue_ts_mapper::options::default_options(99.0, "vue", "/types");
    for path in std::env::args().skip(1) {
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let Ok(r) = vue_ts_mapper::transform(&path, &content, &options, false) else {
            println!("{path}: transform failed");
            continue;
        };
        let lang = r.extension.trim_start_matches('.');
        let parsed = vue_ts_mapper::ts_ast::parse_block(&r.text, lang);
        if let Some(&(start, _)) = parsed.errors.first() {
            let units: Vec<u16> = r.text.encode_utf16().collect();
            let line_start = units[..start.min(units.len())].iter().rposition(|&u| u == b'\n' as u16).map_or(0, |p| p + 1);
            let line_end = units[start.min(units.len())..].iter().position(|&u| u == b'\n' as u16).map_or(units.len(), |p| start + p);
            println!("{path}: {} | {}", parsed.errors.len(), String::from_utf16_lossy(&units[line_start..line_end]).chars().take(160).collect::<String>());
        }
    }
}
