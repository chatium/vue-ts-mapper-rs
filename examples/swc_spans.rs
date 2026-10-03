use swc_core::common::{BytePos, Spanned, comments::SingleThreadedComments};
use swc_core::ecma::ast::*;
use swc_core::ecma::parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

fn main() {
    let src = std::env::args().nth(1).unwrap();
    let comments = SingleThreadedComments::default();
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax { tsx: false, decorators: true, ..Default::default() }),
        EsVersion::latest(),
        StringInput::new(&src, BytePos(1), BytePos(1 + src.len() as u32)),
        Some(&comments),
    );
    let mut p = Parser::new_from(lexer);
    let r = p.parse_program();
    for e in p.take_errors() {
        println!("recoverable error: {:?} at {:?}", e.kind().msg(), e.span());
    }
    match r {
        Ok(Program::Module(m)) => {
            for item in &m.body {
                let sp = item.span();
                println!("item {:?} [{}..{}] {:?}", std::mem::discriminant(item), sp.lo.0 - 1, sp.hi.0 - 1, &src[(sp.lo.0 - 1) as usize..(sp.hi.0 - 1) as usize]);
                if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(v))) = item {
                    for d in &v.decls {
                        println!("  decl [{}..{}]", d.span.lo.0 - 1, d.span.hi.0 - 1);
                    }
                }
                if let ModuleItem::Stmt(Stmt::Expr(e)) = item {
                    println!("  expr [{}..{}]", e.expr.span().lo.0 - 1, e.expr.span().hi.0 - 1);
                }
            }
        }
        Ok(Program::Script(s)) => println!("script {}", s.body.len()),
        Err(e) => println!("fatal: {:?} at {:?}", e.kind().msg(), e.span()),
    }
}
