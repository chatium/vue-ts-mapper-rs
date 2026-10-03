//! Prints how `ts_ast::parse` sees a script: `cargo run --example parse_probe -- <file> [jsx]`
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let jsx = std::env::args().nth(2).is_some();
    let text = std::fs::read_to_string(&path).unwrap();
    let p = vue_ts_mapper::ts_ast::parse(&text, jsx);
    println!("len16 {} program {} error_at {:?}", p.len16(), p.program.is_some(), p.error_at);
}
