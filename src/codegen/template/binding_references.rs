//! `codegen/template/bindingReferences.ts`: which identifiers of a template expression read the
//! component context, walking the swc AST the way the reference walks TypeScript's.

use indexmap::IndexSet;
use swc_core::common::Spanned;
use swc_core::ecma::ast::*;
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::shared::is_globally_allowed;
use crate::ts_ast::Parsed;

#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    /// UTF-16 offset in the parsed text
    pub start: usize,
    pub is_shorthand: bool,
    pub is_narrowing: bool,
    pub skipped: bool,
    pub in_type_query: bool,
    pub is_new_operand: bool,
    /// inside a function written in the expression, where assertion narrowing of imports and
    /// `let` / `var` bindings does not reach
    pub in_function: bool,
    /// the target of an assignment or `++` / `--`
    pub is_write: bool,
}

/// `shouldIdentifierSkipped(ctx, text)`
pub fn should_identifier_skipped(scopes: &[IndexSet<String>], text: &str) -> bool {
    scopes.iter().any(|s| s.contains(text))
        || is_globally_allowed(text)
        || matches!(text, "true" | "false" | "null" | "this")
        || text == "require"
        || text.starts_with("__VLS_")
}

pub struct Walker<'a> {
    pub p: &'a Parsed,
    pub units: &'a [u16],
    pub scopes: &'a mut Vec<IndexSet<String>>,
    pub items: Vec<Item>,
    pub function_depth: usize,
}

impl<'a> Walker<'a> {
    fn text(&self, s: &impl Spanned) -> String {
        let (a, b) = self.p.range(s.span());
        String::from_utf16_lossy(&self.units[a.min(self.units.len())..b.min(self.units.len())])
    }

    fn push_scope(&mut self) -> usize {
        self.scopes.push(IndexSet::new());
        self.scopes.len() - 1
    }

    fn end_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, scope: usize, names: impl IntoIterator<Item = String>) {
        for n in names {
            self.scopes[scope].insert(n);
        }
    }

    fn yield_id(&mut self, id: &Ident, is_shorthand: bool, is_narrowing: bool, is_new_operand: bool) {
        let name = self.text(id);
        let skipped = should_identifier_skipped(self.scopes, &name);
        self.items.push(Item {
            start: self.p.start(id),
            name,
            is_shorthand,
            is_narrowing,
            skipped,
            in_type_query: false,
            is_new_operand,
            in_function: self.function_depth > 0,
            is_write: false,
        });
    }

    fn yield_write(&mut self, id: &Ident) {
        self.yield_id(id, false, true, false);
        self.items.last_mut().unwrap().is_write = true;
    }

    /// `collectBindingNames(ts, name)` for a declaration / parameter name.
    fn binding_names(&self, pat: &Pat) -> Vec<String> {
        let mut out = Vec::new();
        collect_names(self, pat, &mut out);
        out
    }

    // ------------------------------------------------------------------------- program

    /// `forEachDeclarations(sourceFile)`
    pub fn program(&mut self, scope: usize, in_narrowing: bool) {
        let p: &'a Parsed = self.p;
        let Some(program) = p.program.as_ref() else { return };
        match program {
            Program::Script(s) => {
                self.predeclare(scope, s.body.iter());
                for st in &s.body {
                    self.stmt(st, scope, in_narrowing);
                }
            }
            Program::Module(m) => {
                let stmts: Vec<&Stmt> = m.body.iter().filter_map(|i| i.as_stmt()).collect();
                self.predeclare(scope, stmts.iter().copied());
                for item in &m.body {
                    if let ModuleItem::Stmt(st) = item {
                        self.stmt(st, scope, in_narrowing);
                    }
                }
            }
        }
    }

    fn predeclare<'s>(&mut self, scope: usize, stmts: impl Iterator<Item = &'s Stmt>) {
        for s in stmts {
            if let Stmt::Decl(Decl::Fn(f)) = s {
                let name = self.text(&f.ident);
                self.declare(scope, [name]);
            }
        }
    }

    // ------------------------------------------------------------------------- statements

    fn stmt(&mut self, s: &Stmt, scope: usize, in_narrowing: bool) {
        match s {
            Stmt::Expr(e) => self.expr(&e.expr, scope, in_narrowing),
            Stmt::Block(b) => self.block(&b.stmts),
            Stmt::Decl(d) => self.decl(d, scope),
            Stmt::If(i) => {
                self.expr(&i.test, scope, true);
                self.stmt(&i.cons, scope, false);
                if let Some(a) = &i.alt {
                    self.stmt(a, scope, false);
                }
            }
            Stmt::While(w) => {
                self.expr(&w.test, scope, true);
                self.stmt(&w.body, scope, false);
            }
            Stmt::DoWhile(w) => {
                self.stmt(&w.body, scope, false);
                self.expr(&w.test, scope, true);
            }
            Stmt::For(f) => {
                let s2 = self.push_scope();
                match &f.init {
                    Some(VarDeclOrExpr::VarDecl(v)) => {
                        for d in &v.decls {
                            let names = self.binding_names(&d.name);
                            self.declare(s2, names);
                            if let Some(init) = &d.init {
                                self.expr(init, s2, false);
                            }
                        }
                    }
                    Some(VarDeclOrExpr::Expr(e)) => self.expr(e, s2, false),
                    None => {}
                }
                if let Some(t) = &f.test {
                    self.expr(t, s2, true);
                }
                if let Some(u) = &f.update {
                    self.expr(u, s2, false);
                }
                self.stmt(&f.body, s2, false);
                self.end_scope();
            }
            Stmt::ForIn(f) => self.for_in_of(&f.left, &f.right, &f.body),
            Stmt::ForOf(f) => self.for_in_of(&f.left, &f.right, &f.body),
            Stmt::Switch(sw) => {
                let all: Vec<&Stmt> = sw.cases.iter().flat_map(|c| c.cons.iter()).collect();
                self.predeclare(scope, all.into_iter());
                self.expr(&sw.discriminant, scope, true);
                for c in &sw.cases {
                    if let Some(t) = &c.test {
                        self.expr(t, scope, false);
                    }
                    for st in &c.cons {
                        self.stmt(st, scope, false);
                    }
                }
            }
            Stmt::Labeled(l) => self.stmt(&l.body, scope, false),
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Empty(_) | Stmt::Debugger(_) => {}
            Stmt::Return(r) => {
                if let Some(a) = &r.arg {
                    self.expr(a, scope, false);
                }
            }
            Stmt::Throw(t) => self.expr(&t.arg, scope, false),
            Stmt::Try(t) => {
                self.block(&t.block.stmts);
                if let Some(h) = &t.handler {
                    let s2 = self.push_scope();
                    if let Some(param) = &h.param {
                        let names = self.binding_names(param);
                        self.declare(s2, names);
                        self.binding(param, s2);
                    }
                    self.block(&h.body.stmts);
                    self.end_scope();
                }
                if let Some(f) = &t.finalizer {
                    self.block(&f.stmts);
                }
            }
            Stmt::With(w) => {
                self.expr(&w.obj, scope, false);
                self.stmt(&w.body, scope, false);
            }
        }
    }

    fn block(&mut self, stmts: &[Stmt]) {
        let scope = self.push_scope();
        self.predeclare(scope, stmts.iter());
        for s in stmts {
            self.stmt(s, scope, false);
        }
        self.end_scope();
    }

    fn for_in_of(&mut self, left: &ForHead, right: &Expr, body: &Stmt) {
        let scope = self.push_scope();
        let decls: Option<&[VarDeclarator]> = match left {
            ForHead::VarDecl(v) => Some(&v.decls),
            ForHead::UsingDecl(u) => Some(&u.decls),
            ForHead::Pat(_) => None,
        };
        match (decls, left) {
            (Some(decls), _) => {
                for d in decls {
                    if let Some(init) = &d.init {
                        self.expr(init, scope, false);
                    }
                }
            }
            (None, ForHead::Pat(p)) => self.target_pat(p, scope),
            _ => {}
        }
        // the source resolves before the loop bindings are in scope
        self.expr(right, scope, false);
        if let Some(decls) = decls {
            for d in decls {
                let names = self.binding_names(&d.name);
                self.declare(scope, names);
            }
        }
        self.stmt(body, scope, false);
        self.end_scope();
    }

    fn decl(&mut self, d: &Decl, scope: usize) {
        match d {
            Decl::Var(v) => {
                for d in &v.decls {
                    self.var_declarator(d, scope);
                }
            }
            Decl::Using(u) => {
                for d in &u.decls {
                    self.var_declarator(d, scope);
                }
            }
            Decl::Fn(f) => {
                let name = self.text(&f.ident);
                self.declare(scope, [name]);
                self.function(&f.function, None);
            }
            Decl::Class(c) => {
                let name = self.text(&c.ident);
                self.declare(scope, [name]);
                self.class(&c.class, scope);
            }
            Decl::TsInterface(i) => self.type_visit(i.as_ref()),
            Decl::TsTypeAlias(t) => self.type_visit(t.as_ref()),
            Decl::TsEnum(e) => {
                let name = self.text(&e.id);
                self.declare(scope, [name]);
                for m in &e.members {
                    if let Some(init) = &m.init {
                        self.expr(init, scope, false);
                    }
                }
            }
            Decl::TsModule(m) => match &m.body {
                Some(TsNamespaceBody::TsModuleBlock(b)) => {
                    let s2 = self.push_scope();
                    let stmts: Vec<&Stmt> = b.body.iter().filter_map(|i| i.as_stmt()).collect();
                    self.predeclare(s2, stmts.iter().copied());
                    for st in stmts {
                        self.stmt(st, s2, false);
                    }
                    self.end_scope();
                }
                Some(TsNamespaceBody::TsNamespaceDecl(_)) | None => {}
            },
        }
    }

    /// `ts.isVariableDeclaration(node)`
    fn var_declarator(&mut self, d: &VarDeclarator, scope: usize) {
        let names = self.binding_names(&d.name);
        self.declare(scope, names);
        // forEachDeclarationsInBinding(decl)
        let (name, ty) = split_type(&d.name);
        if !matches!(name, Pat::Ident(_)) {
            self.pattern(name, scope);
        }
        if let Some(ty) = ty {
            self.type_visit(ty);
        }
        if let Some(init) = &d.init {
            self.expr(init, scope, false);
        }
    }

    // ------------------------------------------------------------------------- bindings

    /// `ts.isArrayBindingPattern(node) || ts.isObjectBindingPattern(node)`
    fn pattern(&mut self, p: &Pat, scope: usize) {
        match p {
            Pat::Array(a) => {
                for el in a.elems.iter().flatten() {
                    self.binding_element(el, None, scope);
                }
            }
            Pat::Object(o) => {
                for prop in &o.props {
                    match prop {
                        ObjectPatProp::KeyValue(kv) => self.binding_element(&kv.value, Some(&kv.key), scope),
                        ObjectPatProp::Assign(a) => {
                            if let Some(v) = &a.value {
                                self.expr(v, scope, false);
                            }
                        }
                        ObjectPatProp::Rest(r) => self.binding_element(&r.arg, None, scope),
                    }
                }
            }
            _ => {}
        }
    }

    /// `forEachDeclarationsInBinding(element)` for a `BindingElement`.
    fn binding_element(&mut self, el: &Pat, key: Option<&PropName>, scope: usize) {
        if let Some(PropName::Computed(c)) = key {
            self.expr(&c.expr, scope, true);
        }
        let (name, init) = match el {
            Pat::Assign(a) => (a.left.as_ref(), Some(a.right.as_ref())),
            Pat::Rest(r) => (r.arg.as_ref(), None),
            other => (other, None),
        };
        if !matches!(name, Pat::Ident(_)) {
            self.pattern(name, scope);
        }
        if let Some(init) = init {
            self.expr(init, scope, false);
        }
    }

    /// `forEachDeclarationsInBinding(param)` for a parameter / catch binding.
    fn binding(&mut self, param: &Pat, scope: usize) {
        let (inner, init) = match param {
            Pat::Assign(a) => (a.left.as_ref(), Some(a.right.as_ref())),
            Pat::Rest(r) => (r.arg.as_ref(), None),
            other => (other, None),
        };
        let (name, ty) = split_type(inner);
        let rest_ty = match param {
            Pat::Rest(r) => r.type_ann.as_deref(),
            _ => None,
        };
        if !matches!(name, Pat::Ident(_)) {
            self.pattern(name, scope);
        }
        if let Some(ty) = ty.or(rest_ty) {
            self.type_visit(ty);
        }
        if let Some(init) = init {
            self.expr(init, scope, false);
        }
    }

    // ------------------------------------------------------------------------- functions

    /// `forEachDeclarationsInFunction(node)`
    fn function(&mut self, f: &Function, name: Option<&Ident>) {
        let scope = self.push_scope();
        self.function_depth += 1;
        if let Some(n) = name {
            let t = self.text(n);
            self.declare(scope, [t]);
        }
        // swc keeps TypeScript's leading `this` parameter apart from the others
        if f.this_param.is_some() {
            self.declare(scope, ["this".to_string()]);
        }
        for p in &f.params {
            let names = self.binding_names(&p.pat);
            self.declare(scope, names);
        }
        if let Some(tp) = &f.type_params {
            self.type_visit(tp.as_ref());
        }
        if let Some(ta) = f.this_param.as_ref().and_then(|t| t.type_ann.as_ref()) {
            self.type_visit(ta.as_ref());
        }
        for p in &f.params {
            self.binding(&p.pat, scope);
        }
        if let Some(rt) = &f.return_type {
            self.type_visit(rt.as_ref());
        }
        if let Some(body) = &f.body {
            self.block(&body.stmts);
        }
        self.function_depth -= 1;
        self.end_scope();
    }

    fn arrow(&mut self, a: &ArrowExpr) {
        let scope = self.push_scope();
        self.function_depth += 1;
        for p in &a.params {
            let names = self.binding_names(p);
            self.declare(scope, names);
        }
        if let Some(tp) = &a.type_params {
            self.type_visit(tp.as_ref());
        }
        for p in &a.params {
            self.binding(p, scope);
        }
        if let Some(rt) = &a.return_type {
            self.type_visit(rt.as_ref());
        }
        match a.body.as_ref() {
            ArrowFunctionBody::FunctionBody(b) => self.block(&b.stmts),
            ArrowFunctionBody::Expr(e) => self.expr(e, scope, false),
        }
        self.function_depth -= 1;
        self.end_scope();
    }

    /// A getter / setter / method of an object literal or class.
    fn accessor(&mut self, params: &[&Pat], type_params: Option<&TsTypeParamDecl>, ret: Option<&TsTypeAnn>, body: Option<&FunctionBody>) {
        let scope = self.push_scope();
        self.function_depth += 1;
        for p in params {
            let names = self.binding_names(p);
            self.declare(scope, names);
        }
        if let Some(tp) = type_params {
            self.type_visit(tp);
        }
        for p in params {
            self.binding(p, scope);
        }
        if let Some(rt) = ret {
            self.type_visit(rt);
        }
        if let Some(b) = body {
            self.block(&b.stmts);
        }
        self.function_depth -= 1;
        self.end_scope();
    }

    fn class(&mut self, c: &Class, scope: usize) {
        if let Some(tp) = &c.type_params {
            self.type_visit(tp.as_ref());
        }
        if let Some(sc) = &c.super_class {
            self.expr(sc, scope, false);
            if let Some(ta) = &c.super_type_params {
                self.type_visit(ta.as_ref());
            }
        }
        for imp in &c.implements {
            self.type_visit(imp);
        }
        for m in &c.body {
            match m {
                ClassMember::ClassProp(p) => {
                    if let PropName::Computed(k) = &p.key {
                        self.expr(&k.expr, scope, true);
                    }
                    if let Some(t) = &p.type_ann {
                        self.type_visit(t.as_ref());
                    }
                    if let Some(v) = &p.value {
                        self.expr(v, scope, false);
                    }
                }
                ClassMember::PrivateProp(p) => {
                    if let Some(t) = &p.type_ann {
                        self.type_visit(t.as_ref());
                    }
                    if let Some(v) = &p.value {
                        self.expr(v, scope, false);
                    }
                }
                ClassMember::Method(m) => {
                    if let PropName::Computed(k) = &m.key {
                        self.expr(&k.expr, scope, true);
                    }
                    self.function(&m.function, None);
                }
                ClassMember::PrivateMethod(m) => self.function(&m.function, None),
                ClassMember::Constructor(ctor) => {
                    let pats: Vec<Pat> = ctor
                        .params
                        .iter()
                        .map(|p| match p {
                            ParamOrTsParamProp::Param(p) => p.pat.clone(),
                            ParamOrTsParamProp::TsParamProp(tp) => match &tp.param {
                                TsParamPropParam::Ident(i) => Pat::Ident(i.clone()),
                                TsParamPropParam::Assign(a) => Pat::Assign(a.clone()),
                            },
                        })
                        .collect();
                    let refs: Vec<&Pat> = pats.iter().collect();
                    self.accessor(&refs, None, None, ctor.body.as_ref());
                }
                ClassMember::StaticBlock(b) => {
                    let s2 = self.push_scope();
                    self.predeclare(s2, b.body.stmts.iter());
                    for st in &b.body.stmts {
                        self.stmt(st, s2, false);
                    }
                    self.end_scope();
                }
                ClassMember::TsIndexSignature(_) | ClassMember::Empty(_) | ClassMember::AutoAccessor(_) => {}
            }
        }
    }

    // ------------------------------------------------------------------------- expressions

    pub fn expr(&mut self, e: &Expr, scope: usize, in_narrowing: bool) {
        match e {
            Expr::Ident(id) => self.yield_id(id, false, in_narrowing, false),
            Expr::Member(m) => self.member(m, scope),
            Expr::SuperProp(sp) => {
                if let SuperProp::Computed(c) = &sp.prop {
                    self.expr(&c.expr, scope, false);
                }
            }
            Expr::Call(c) => {
                if let Callee::Expr(callee) = &c.callee {
                    self.expr(callee, scope, false);
                }
                if let Some(ta) = &c.type_args {
                    self.type_visit(ta.as_ref());
                }
                for a in &c.args {
                    self.arg(a, scope, in_narrowing);
                }
            }
            Expr::New(n) => {
                if let Expr::Ident(id) = n.callee.as_ref() {
                    self.yield_id(id, false, false, true);
                } else {
                    self.expr(&n.callee, scope, false);
                }
                if let Some(ta) = &n.type_args {
                    self.type_visit(ta.as_ref());
                }
                for a in n.args.iter().flatten() {
                    self.arg(a, scope, in_narrowing);
                }
            }
            Expr::TaggedTpl(t) => {
                self.expr(&t.tag, scope, false);
                if let Some(ta) = &t.type_params {
                    self.type_visit(ta.as_ref());
                }
                for x in &t.tpl.exprs {
                    self.expr(x, scope, false);
                }
            }
            Expr::Tpl(t) => {
                for x in &t.exprs {
                    self.expr(x, scope, false);
                }
            }
            Expr::Paren(p) => self.expr(&p.expr, scope, in_narrowing),
            Expr::TsNonNull(n) => self.expr(&n.expr, scope, in_narrowing),
            Expr::TsTypeAssertion(a) => {
                self.type_visit(a.type_ann.as_ref());
                self.expr(&a.expr, scope, in_narrowing);
            }
            Expr::TsAs(a) => {
                self.expr(&a.expr, scope, in_narrowing);
                self.type_visit(a.type_ann.as_ref());
            }
            Expr::TsConstAssertion(a) => self.expr(&a.expr, scope, in_narrowing),
            Expr::TsSatisfies(a) => {
                self.expr(&a.expr, scope, in_narrowing);
                self.type_visit(a.type_ann.as_ref());
            }
            Expr::TsInstantiation(i) => {
                self.expr(&i.expr, scope, false);
                self.type_visit(i.type_args.as_ref());
            }
            Expr::Seq(s) => {
                let last = s.exprs.len().saturating_sub(1);
                for (i, x) in s.exprs.iter().enumerate() {
                    self.expr(x, scope, if i == last { in_narrowing } else { false });
                }
            }
            Expr::Bin(b) => match b.op {
                BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing => {
                    self.expr(&b.left, scope, true);
                    self.expr(&b.right, scope, in_narrowing);
                }
                op => {
                    let eq = matches!(op, BinaryOp::EqEqEq | BinaryOp::NotEqEq | BinaryOp::EqEq | BinaryOp::NotEq);
                    self.expr(&b.left, scope, eq || op == BinaryOp::InstanceOf);
                    self.expr(&b.right, scope, eq || op == BinaryOp::In);
                }
            },
            Expr::Assign(a) => {
                self.target(&a.left, scope);
                self.expr(&a.right, scope, false);
            }
            Expr::Cond(c) => {
                self.expr(&c.test, scope, true);
                self.expr(&c.cons, scope, false);
                self.expr(&c.alt, scope, false);
            }
            Expr::Unary(u) => {
                let narrowing = matches!(u.op, UnaryOp::Bang | UnaryOp::TypeOf | UnaryOp::Delete);
                self.expr(&u.arg, scope, narrowing);
            }
            Expr::Update(u) => match u.arg.as_ref() {
                Expr::Ident(id) => self.yield_write(id),
                arg => self.expr(arg, scope, true),
            },
            Expr::Arrow(a) => self.arrow(a),
            Expr::Fn(f) => self.function(&f.function, f.ident.as_ref()),
            Expr::Class(c) => {
                let s2 = self.push_scope();
                if let Some(id) = &c.ident {
                    let t = self.text(id);
                    self.declare(s2, [t]);
                }
                self.class(&c.class, s2);
                self.end_scope();
            }
            Expr::Object(o) => self.object(o, scope),
            Expr::Array(a) => {
                for el in a.elems.iter().flatten() {
                    self.expr(&el.expr, scope, false);
                }
            }
            Expr::Await(a) => self.expr(&a.arg, scope, false),
            Expr::Yield(y) => {
                if let Some(a) = &y.arg {
                    self.expr(a, scope, false);
                }
            }
            Expr::OptChain(o) => match o.base.as_ref() {
                OptChainBase::Member(m) => self.member(m, scope),
                OptChainBase::Call(c) => {
                    self.expr(&c.callee, scope, false);
                    if let Some(ta) = &c.type_args {
                        self.type_visit(ta.as_ref());
                    }
                    for a in &c.args {
                        self.arg(a, scope, in_narrowing);
                    }
                }
            },
            Expr::MetaProp(_) | Expr::This(_) | Expr::Lit(_) | Expr::PrivateName(_) | Expr::Invalid(_) => {}
            Expr::JSXMember(_) | Expr::JSXNamespacedName(_) | Expr::JSXEmpty(_) | Expr::JSXElement(_) | Expr::JSXFragment(_) => {}
        }
    }

    /// A call argument: a spread is walked by the generic branch (no narrowing).
    fn arg(&mut self, a: &ExprOrSpread, scope: usize, in_narrowing: bool) {
        if a.spread.is_some() {
            self.expr(&a.expr, scope, false);
        } else {
            self.expr(&a.expr, scope, in_narrowing);
        }
    }

    fn member(&mut self, m: &MemberExpr, scope: usize) {
        self.expr(&m.obj, scope, true);
        if let MemberProp::Computed(c) = &m.prop {
            self.expr(&c.expr, scope, false);
        }
    }

    fn object(&mut self, o: &ObjectLit, scope: usize) {
        for p in &o.props {
            match p {
                PropOrSpread::Spread(s) => self.expr(&s.expr, scope, false),
                PropOrSpread::Prop(p) => match p.as_ref() {
                    Prop::KeyValue(kv) => {
                        if let PropName::Computed(c) = &kv.key {
                            self.expr(&c.expr, scope, false);
                        }
                        self.expr(&kv.value, scope, false);
                    }
                    Prop::Shorthand(id) => self.yield_id(id, true, false, false),
                    // `{ a = 1 }` only parses as a destructuring target; TypeScript keeps it a shorthand
                    Prop::Assign(a) => self.yield_id(&a.key, true, false, false),
                    Prop::Method(m) => {
                        if let PropName::Computed(c) = &m.key {
                            self.expr(&c.expr, scope, false);
                        }
                        self.function(&m.function, None);
                    }
                    Prop::Getter(g) => {
                        if let PropName::Computed(c) = &g.key {
                            self.expr(&c.expr, scope, false);
                        }
                        self.function(&g.function, None);
                    }
                    Prop::Setter(s) => {
                        if let PropName::Computed(c) = &s.key {
                            self.expr(&c.expr, scope, false);
                        }
                        self.function(&s.function, None);
                    }
                },
            }
        }
    }

    // ------------------------------------------------------------------------- assignment targets

    /// `forEachDeclarationsInAssignmentTarget`
    fn target(&mut self, t: &AssignTarget, scope: usize) {
        match t {
            AssignTarget::Simple(s) => match s {
                SimpleAssignTarget::Ident(b) => self.yield_write(&b.id),
                SimpleAssignTarget::Paren(p) => self.target_expr(&p.expr, scope),
                SimpleAssignTarget::Member(m) => self.member(m, scope),
                SimpleAssignTarget::SuperProp(sp) => {
                    if let SuperProp::Computed(c) = &sp.prop {
                        self.expr(&c.expr, scope, false);
                    }
                }
                SimpleAssignTarget::OptChain(o) => self.expr(&Expr::OptChain(o.clone()), scope, true),
                SimpleAssignTarget::TsAs(a) => {
                    self.expr(&a.expr, scope, true);
                    self.type_visit(a.type_ann.as_ref());
                }
                SimpleAssignTarget::TsSatisfies(a) => {
                    self.expr(&a.expr, scope, true);
                    self.type_visit(a.type_ann.as_ref());
                }
                SimpleAssignTarget::TsNonNull(n) => self.expr(&n.expr, scope, true),
                SimpleAssignTarget::TsTypeAssertion(a) => {
                    self.type_visit(a.type_ann.as_ref());
                    self.expr(&a.expr, scope, true);
                }
                SimpleAssignTarget::TsInstantiation(i) => {
                    self.expr(&i.expr, scope, false);
                    self.type_visit(i.type_args.as_ref());
                }
                SimpleAssignTarget::Invalid(_) => {}
            },
            AssignTarget::Pat(p) => match p {
                AssignTargetPat::Array(a) => self.target_array(a, scope),
                AssignTargetPat::Object(o) => self.target_object(o, scope),
                AssignTargetPat::Invalid(_) => {}
            },
        }
    }

    fn target_expr(&mut self, e: &Expr, scope: usize) {
        match e {
            Expr::Ident(id) => self.yield_id(id, false, true, false),
            Expr::Paren(p) => self.target_expr(&p.expr, scope),
            other => self.expr(other, scope, true),
        }
    }

    fn target_pat(&mut self, p: &Pat, scope: usize) {
        match p {
            Pat::Ident(b) => self.yield_id(&b.id, false, true, false),
            Pat::Array(a) => self.target_array(a, scope),
            Pat::Object(o) => self.target_object(o, scope),
            Pat::Expr(e) => self.target_expr(e, scope),
            Pat::Assign(a) => {
                self.target_pat(&a.left, scope);
                self.expr(&a.right, scope, false);
            }
            Pat::Rest(r) => self.target_pat(&r.arg, scope),
            Pat::Invalid(_) => {}
        }
    }

    fn target_array(&mut self, a: &ArrayPat, scope: usize) {
        for el in a.elems.iter().flatten() {
            match el {
                Pat::Rest(r) => self.target_pat(&r.arg, scope),
                Pat::Assign(asg) => {
                    self.target_pat(&asg.left, scope);
                    self.expr(&asg.right, scope, false);
                }
                other => self.target_pat(other, scope),
            }
        }
    }

    fn target_object(&mut self, o: &ObjectPat, scope: usize) {
        for prop in &o.props {
            match prop {
                ObjectPatProp::KeyValue(kv) => {
                    if let PropName::Computed(c) = &kv.key {
                        self.expr(&c.expr, scope, false);
                    }
                    self.target_pat(&kv.value, scope);
                }
                ObjectPatProp::Assign(a) => {
                    self.yield_id(&a.key.id, true, true, false);
                    if let Some(v) = &a.value {
                        self.expr(v, scope, false);
                    }
                }
                ObjectPatProp::Rest(r) => self.target_pat(&r.arg, scope),
            }
        }
    }

    // ------------------------------------------------------------------------- types

    /// `forEachDeclarationsInTypeNode`: `typeof x` queries and computed member names.
    fn type_visit<N: ?Sized>(&mut self, n: &N)
    where
        for<'w> N: VisitWith<TypeVisitor<'w, 'a>>,
    {
        let mut v = TypeVisitor { w: self };
        n.visit_with(&mut v);
    }
}

pub struct TypeVisitor<'w, 'a> {
    w: &'w mut Walker<'a>,
}

impl<'w, 'a> TypeVisitor<'w, 'a> {
    fn computed(&mut self, key: &Expr) {
        let before = self.w.items.len();
        let scope = self.w.scopes.len().saturating_sub(1);
        self.w.expr(key, scope, false);
        for item in &mut self.w.items[before..] {
            item.in_type_query = true;
        }
    }
}

impl<'w, 'a> Visit for TypeVisitor<'w, 'a> {
    fn visit_ts_type_query(&mut self, q: &TsTypeQuery) {
        if let TsTypeQueryExpr::TsEntityName(name) = &q.expr_name {
            let mut n = name;
            let id = loop {
                match n {
                    TsEntityName::Ident(i) => break i,
                    TsEntityName::TsQualifiedName(q) => n = &q.left,
                }
            };
            let text = self.w.text(id);
            let skipped = should_identifier_skipped(self.w.scopes, &text);
            self.w.items.push(Item {
                start: self.w.p.start(id),
                name: text,
                is_shorthand: false,
                is_narrowing: false,
                skipped,
                in_type_query: true,
                is_new_operand: false,
                in_function: self.w.function_depth > 0,
                is_write: false,
            });
        }
    }

    fn visit_ts_property_signature(&mut self, s: &TsPropertySignature) {
        if s.computed {
            self.computed(&s.key);
        }
        if let Some(t) = &s.type_ann {
            t.visit_with(self);
        }
    }

    fn visit_ts_method_signature(&mut self, s: &TsMethodSignature) {
        if s.computed {
            self.computed(&s.key);
        }
        s.type_params.visit_with(self);
        s.params.visit_with(self);
        s.type_ann.visit_with(self);
    }

    fn visit_ts_getter_signature(&mut self, s: &TsGetterSignature) {
        if s.computed {
            self.computed(&s.key);
        }
        s.type_ann.visit_with(self);
    }

    fn visit_ts_setter_signature(&mut self, s: &TsSetterSignature) {
        if s.computed {
            self.computed(&s.key);
        }
        s.param.visit_with(self);
    }

    // expressions inside types (enum member initializers, decorators, ...) are not walked
    fn visit_expr(&mut self, _: &Expr) {}
}

/// Splits a declaration / parameter name from its type annotation (swc keeps it on the pattern).
fn split_type(p: &Pat) -> (&Pat, Option<&TsTypeAnn>) {
    let ty = match p {
        Pat::Ident(b) => b.type_ann.as_deref(),
        Pat::Array(a) => a.type_ann.as_deref(),
        Pat::Object(o) => o.type_ann.as_deref(),
        Pat::Rest(r) => r.type_ann.as_deref(),
        Pat::Assign(_) | Pat::Expr(_) | Pat::Invalid(_) => None,
    };
    (p, ty)
}

/// `collectBindingNames(ts, name)`
fn collect_names(w: &Walker, pat: &Pat, out: &mut Vec<String>) {
    match pat {
        Pat::Ident(b) => {
            let (s, e) = crate::ts_ast::binding_ident_range(w.p, w.units, b);
            out.push(String::from_utf16_lossy(&w.units[s..e]));
        }
        Pat::Array(a) => {
            for el in a.elems.iter().flatten() {
                collect_element(w, el, out);
            }
        }
        Pat::Object(o) => {
            for prop in &o.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_element(w, &kv.value, out),
                    ObjectPatProp::Assign(a) => out.push(w.text(&a.key.id)),
                    ObjectPatProp::Rest(r) => collect_names(w, &r.arg, out),
                }
            }
        }
        Pat::Assign(a) => collect_names(w, &a.left, out),
        Pat::Rest(r) => collect_names(w, &r.arg, out),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

fn collect_element(w: &Walker, el: &Pat, out: &mut Vec<String>) {
    match el {
        Pat::Assign(a) => collect_names(w, &a.left, out),
        Pat::Rest(r) => collect_names(w, &r.arg, out),
        other => collect_names(w, other, out),
    }
}
