//! Prints the virtual code, span mappings and directives of one SFC:
//! `cargo run --release --example dump_one -- <file.vue> [needle]`
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let needle = std::env::args().nth(2);
    let content = std::fs::read_to_string(&path).unwrap();
    let options = vue_ts_mapper::options::default_options(99.0, "vue", "/types");
    let r = vue_ts_mapper::transform(&path, &content, &options, true).unwrap();
    match needle {
        None => println!("{}", r.text),
        Some(n) => {
            let units: Vec<u16> = r.text.encode_utf16().collect();
            let text16 = |a: i64, b: i64| String::from_utf16_lossy(&units[a as usize..b as usize]);
            for (i, _) in r.text.match_indices(&n) {
                let at = r.text[..i].encode_utf16().count() as i64;
                let line_start = r.text[..i].rfind('\n').map_or(0, |p| p + 1);
                let line_end = r.text[i..].find('\n').map_or(r.text.len(), |p| i + p);
                println!("@{at}: {}", &r.text[line_start..line_end]);
                for m in r.mappings.iter().filter(|m| m.0 <= at + 200 && m.0 + m.1 >= at - 50) {
                    println!("  map gen[{}..{}] orig[{}..{}] kind {} {:?}", m.0, m.0 + m.1, m.2, m.2 + m.3, m.4, text16(m.0, m.0 + m.1));
                }
                for d in r.directives.iter().filter(|d| d.3 >= at - 50 && d.2 <= at + 200) {
                    println!("  directive virt[{}..{}] policy {} orig {}+{}", d.2, d.3, d.4, d.0, d.1);
                }
            }
        }
    }
}
