//! `parsers/scriptRanges.ts`, `parsers/scriptSetupRanges.ts`, `parsers/utils.ts` and
//! `utils/collectBindings.ts`, over the swc AST.

use indexmap::IndexMap;
use swc_core::common::Spanned;
use swc_core::ecma::ast::*;

use crate::options::VueOptions;
use crate::ts_ast::{CommentKind, Parsed, leading_comment_ranges, skip_trivia};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

impl Range {
    pub fn new(start: usize, end: usize) -> Self {
        Range { start, end }
    }
}

/// The text of a parsed block, with the UTF-16 view the comment scanner needs.
pub struct Src<'a> {
    pub text: &'a str,
    pub units: Vec<u16>,
    pub p: &'a Parsed,
}

impl<'a> Src<'a> {
    pub fn new(text: &'a str, p: &'a Parsed) -> Self {
        Src { text, units: text.encode_utf16().collect(), p }
    }

    pub fn range(&self, s: &impl Spanned) -> Range {
        let (start, end) = self.p.range(s.span());
        Range { start, end }
    }

    pub fn slice(&self, r: Range) -> String {
        String::from_utf16_lossy(&self.units[r.start.min(self.units.len())..r.end.min(self.units.len())])
    }

    pub fn node_text(&self, s: &impl Spanned) -> String {
        self.slice(self.range(s))
    }

    /// The last multi-line comment among the leading comments at `pos`.
    pub fn closest_multi_line_comment(&self, pos: usize) -> Option<Range> {
        leading_comment_ranges(&self.units, pos)
            .into_iter()
            .rev()
            .find(|c| c.kind == CommentKind::MultiLine)
            .map(|c| Range::new(c.pos, c.end))
    }
}

// ---------------------------------------------------------------------------------------------
// collectBindings

pub struct BindingId {
    pub name: String,
    pub range: Range,
    pub is_rest: bool,
    pub initializer: Option<Range>,
}

/// `collectBindingIdentifiers(ts, name)` for a binding name (identifier or pattern).
pub fn collect_binding_identifiers(src: &Src, pat: &Pat, out: &mut Vec<BindingId>, is_rest: bool, initializer: Option<Range>) {
    match pat {
        Pat::Ident(b) => out.push(BindingId {
            name: b.id.sym.to_string(),
            range: {
                let (s, e) = crate::ts_ast::binding_ident_range(src.p, &src.units, b);
                Range::new(s, e)
            },
            is_rest,
            initializer,
        }),
        Pat::Array(a) => {
            for el in a.elems.iter().flatten() {
                binding_element(src, el, out);
            }
        }
        Pat::Object(o) => {
            for prop in &o.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => binding_element(src, &kv.value, out),
                    ObjectPatProp::Assign(a) => out.push(BindingId {
                        name: a.key.id.sym.to_string(),
                        range: src.range(&a.key.id),
                        is_rest: false,
                        initializer: a.value.as_ref().map(|v| src.range(v.as_ref())),
                    }),
                    ObjectPatProp::Rest(r) => collect_binding_identifiers(src, &r.arg, out, true, None),
                }
            }
        }
        // only reachable as a parameter / declaration name with a default or rest
        Pat::Assign(a) => collect_binding_identifiers(src, &a.left, out, is_rest, Some(src.range(a.right.as_ref()))),
        Pat::Rest(r) => collect_binding_identifiers(src, &r.arg, out, true, None),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

/// A `BindingElement` of a pattern: its name, with `...` and the default value.
fn binding_element(src: &Src, el: &Pat, out: &mut Vec<BindingId>) {
    match el {
        Pat::Rest(r) => collect_binding_identifiers(src, &r.arg, out, true, None),
        Pat::Assign(a) => collect_binding_identifiers(src, &a.left, out, false, Some(src.range(a.right.as_ref()))),
        other => collect_binding_identifiers(src, other, out, false, None),
    }
}

pub fn collect_binding_names(src: &Src, pat: &Pat) -> Vec<String> {
    let mut out = Vec::new();
    collect_binding_identifiers(src, pat, &mut out, false, None);
    out.into_iter().map(|b| b.name).collect()
}

// ---------------------------------------------------------------------------------------------
// parseBindingRanges

#[derive(Default)]
pub struct BindingRanges {
    pub bindings: Vec<Range>,
    pub components: Vec<Range>,
    /// imports and `let` / `var` bindings: the template codegen re-asserts them per closure
    pub non_flowing_bindings: Vec<Range>,
}

fn module_items(p: &Parsed) -> &[ModuleItem] {
    match &p.program {
        Some(Program::Module(m)) => &m.body,
        _ => &[],
    }
}

/// The declaration a top-level item carries, looking through `export` / `export default`.
enum TopDecl<'a> {
    Var { decls: &'a [VarDeclarator], is_const: bool },
    Fn(Option<&'a Ident>),
    Class(Option<&'a Ident>),
    Enum(&'a Ident),
    Import(&'a ImportDecl),
    None,
}

fn top_decl(item: &ModuleItem) -> TopDecl<'_> {
    let decl = match item {
        ModuleItem::Stmt(Stmt::Decl(d)) => d,
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => &e.decl,
        ModuleItem::ModuleDecl(ModuleDecl::Import(i)) => return TopDecl::Import(i),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(d)) => {
            return match &d.decl {
                DefaultDecl::Fn(f) => TopDecl::Fn(f.ident.as_ref()),
                DefaultDecl::Class(c) => TopDecl::Class(c.ident.as_ref()),
                DefaultDecl::TsInterfaceDecl(_) => TopDecl::None,
            };
        }
        _ => return TopDecl::None,
    };
    match decl {
        Decl::Var(v) => TopDecl::Var { decls: &v.decls, is_const: v.kind == VarDeclKind::Const },
        // TypeScript's `NodeFlags.AwaitUsing` includes the `Const` bit
        Decl::Using(u) => TopDecl::Var { decls: &u.decls, is_const: u.is_await },
        Decl::Fn(f) => TopDecl::Fn(Some(&f.ident)),
        Decl::Class(c) => TopDecl::Class(Some(&c.ident)),
        Decl::TsEnum(e) => TopDecl::Enum(&e.id),
        _ => TopDecl::None,
    }
}

pub fn parse_binding_ranges(src: &Src, extensions: &[String]) -> BindingRanges {
    let mut r = BindingRanges::default();
    for item in module_items(src.p) {
        match top_decl(item) {
            TopDecl::Var { decls, is_const } => {
                for d in decls {
                    let mut ids = Vec::new();
                    collect_binding_identifiers(src, &d.name, &mut ids, false, None);
                    for id in ids {
                        r.bindings.push(id.range);
                        if !is_const {
                            r.non_flowing_bindings.push(id.range);
                        }
                    }
                }
            }
            TopDecl::Fn(Some(id)) | TopDecl::Class(Some(id)) | TopDecl::Enum(id) => r.bindings.push(src.range(id)),
            TopDecl::Import(i) => {
                if i.type_only {
                    continue;
                }
                // `getNodeText(node.moduleSpecifier).slice(1, -1)`
                let quoted = src.node_text(i.src.as_ref());
                let module_name = if quoted.len() >= 2 { &quoted[1..quoted.len() - 1] } else { "" };
                let is_component_module = extensions.iter().any(|ext| module_name.ends_with(ext.as_str()));
                for spec in &i.specifiers {
                    match spec {
                        ImportSpecifier::Default(d) => {
                            if is_component_module {
                                r.components.push(src.range(&d.local));
                            } else {
                                r.bindings.push(src.range(&d.local));
                                r.non_flowing_bindings.push(src.range(&d.local));
                            }
                        }
                        ImportSpecifier::Named(n) => {
                            if n.is_type_only {
                                continue;
                            }
                            let is_default = match &n.imported {
                                Some(ModuleExportName::Ident(id)) => &*id.sym == "default",
                                Some(ModuleExportName::Str(s)) => src.node_text(s) == "default",
                                None => false,
                            };
                            if is_default && is_component_module {
                                r.components.push(src.range(&n.local));
                            } else {
                                r.bindings.push(src.range(&n.local));
                                r.non_flowing_bindings.push(src.range(&n.local));
                            }
                        }
                        ImportSpecifier::Namespace(ns) => {
                            r.bindings.push(src.range(&ns.local));
                            r.non_flowing_bindings.push(src.range(&ns.local));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    r
}

/// `node.pos` of each top-level item: where the previous item ended.
fn item_full_starts(src: &Src) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev = 0;
    for item in module_items(src.p) {
        out.push(prev);
        prev = src.p.end(item);
    }
    out
}

/// `getUnwrappedExpression`: through parentheses, `as` / `<T>` assertions, `!` and `satisfies`.
pub fn unwrap_expression(mut e: &Expr) -> &Expr {
    loop {
        e = match e {
            Expr::Paren(p) => &p.expr,
            Expr::TsAs(a) => &a.expr,
            Expr::TsTypeAssertion(a) => &a.expr,
            Expr::TsConstAssertion(a) => &a.expr,
            Expr::TsNonNull(n) => &n.expr,
            Expr::TsSatisfies(s) => &s.expr,
            _ => return e,
        };
    }
}

// ---------------------------------------------------------------------------------------------
// parseScriptRanges

#[derive(Debug, Clone)]
pub struct ComponentOptions {
    pub is_object_literal: bool,
    pub expression: Range,
    pub args: Range,
    pub components: Option<Range>,
    pub directives: Option<Range>,
    pub name: Option<Range>,
    pub inherit_attrs: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExportDefault {
    pub start: usize,
    pub end: usize,
    pub expression: Range,
    pub is_object_literal: bool,
    pub options: Option<ComponentOptions>,
}

pub struct ScriptRanges {
    pub bindings: BindingRanges,
    pub export_default: Option<ExportDefault>,
}

pub fn parse_script_ranges(src: &Src, options: &VueOptions) -> ScriptRanges {
    let mut export_default = None;
    let starts = item_full_starts(src);
    for (i, item) in module_items(src.p).iter().enumerate() {
        let (span, expr) = match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(e)) => (e.span, e.expr.as_ref()),
            ModuleItem::ModuleDecl(ModuleDecl::TsExportAssignment(e)) => (e.span, e.expr.as_ref()),
            _ => continue,
        };
        let r = src.range(&span);
        let mut ed = ExportDefault {
            start: r.start,
            end: r.end,
            expression: src.range(expr),
            is_object_literal: matches!(expr, Expr::Object(_)),
            options: parse_options_from_expression(src, expr),
        };
        if let Some(c) = src.closest_multi_line_comment(starts[i]) {
            ed.start = c.start;
        }
        export_default = Some(ed);
    }
    ScriptRanges { bindings: parse_binding_ranges(src, &options.extensions), export_default }
}

fn prop_name_ident(key: &PropName) -> Option<&IdentName> {
    match key {
        PropName::Ident(i) => Some(i),
        _ => None,
    }
}

pub fn parse_options_from_expression(src: &Src, exp: &Expr) -> Option<ComponentOptions> {
    let exp = unwrap_expression(exp);
    let obj = match exp {
        Expr::Object(o) => Some(o),
        Expr::Call(c) if !c.args.is_empty() => match (c.args[0].spread.is_none(), c.args[0].expr.as_ref()) {
            (true, Expr::Object(o)) => Some(o),
            _ => None,
        },
        _ => None,
    }?;
    let mut components = None;
    let mut directives = None;
    let mut name = None;
    let mut inherit_attrs = None;
    for prop in &obj.props {
        let PropOrSpread::Prop(p) = prop else { continue };
        let Prop::KeyValue(kv) = p.as_ref() else { continue };
        let Some(key) = prop_name_ident(&kv.key) else { continue };
        match &*key.sym {
            "components" => {
                if let Expr::Object(o) = kv.value.as_ref() {
                    components = Some(src.range(o));
                }
            }
            "directives" => {
                if let Expr::Object(o) = kv.value.as_ref() {
                    directives = Some(src.range(o));
                }
            }
            "name" => {
                if let Expr::Lit(Lit::Str(s)) = kv.value.as_ref() {
                    name = Some(src.range(s));
                }
            }
            "inheritAttrs" => inherit_attrs = Some(src.node_text(kv.value.as_ref())),
            _ => {}
        }
    }
    Some(ComponentOptions {
        is_object_literal: matches!(exp, Expr::Object(_)),
        expression: src.range(exp),
        args: src.range(obj),
        components,
        directives,
        name,
        inherit_attrs,
    })
}

// ---------------------------------------------------------------------------------------------
// parseScriptSetupRanges

#[derive(Debug, Clone, Default)]
pub struct CallRange {
    pub call_exp: Range,
    pub exp: Range,
    pub arg: Option<Range>,
    pub type_arg: Option<Range>,
}

#[derive(Debug, Clone, Default)]
pub struct DefineModel {
    pub arg: Option<Range>,
    pub local_name: Option<Range>,
    pub name: Option<Range>,
    pub type_: Option<Range>,
    pub modifier_type: Option<Range>,
    pub runtime_type: Option<Range>,
    pub default_value: Option<Range>,
    pub required: bool,
    pub comments: Option<Range>,
}

#[derive(Debug, Clone, Default)]
pub struct DefineProps {
    pub call: CallRange,
    pub name: Option<String>,
    pub destructured: Option<IndexMap<String, Option<Range>>>,
    pub destructured_rest: Option<String>,
    pub statement: Range,
}

#[derive(Debug, Clone, Default)]
pub struct DefineEmits {
    pub call: CallRange,
    pub name: Option<String>,
    pub has_union_type_arg: bool,
    pub statement: Range,
}

#[derive(Debug, Clone, Default)]
pub struct DefineSlots {
    pub call: CallRange,
    pub name: Option<String>,
    pub statement: Range,
}

#[derive(Debug, Clone, Default)]
pub struct DefineOptions {
    pub name: Option<String>,
    pub inherit_attrs: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UseTemplateRef {
    pub call: CallRange,
    pub name: Option<String>,
}

#[derive(Default)]
pub struct ScriptSetupRanges {
    pub bindings: BindingRanges,
    pub leading_comment_end_offset: usize,
    pub import_section_end_offset: usize,
    pub define_model: Vec<DefineModel>,
    pub define_props: Option<DefineProps>,
    pub with_defaults: Option<CallRange>,
    pub define_emits: Option<DefineEmits>,
    pub define_slots: Option<DefineSlots>,
    pub define_expose: Option<CallRange>,
    pub define_options: Option<DefineOptions>,
    pub use_attrs: Vec<CallRange>,
    pub use_css_module: Vec<CallRange>,
    pub use_slots: Vec<CallRange>,
    pub use_template_ref: Vec<UseTemplateRef>,
}

/// `/^\/\/\s*@ts-(?:no)?check(?:$|\s)/`
fn is_ts_check_comment(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("//") else { return false };
    let rest = rest.trim_start_matches(|c: char| c.is_whitespace());
    let rest = match rest.strip_prefix("@ts-") {
        Some(r) => r,
        None => return false,
    };
    let rest = rest.strip_prefix("no").unwrap_or(rest);
    match rest.strip_prefix("check") {
        Some(r) => r.is_empty() || r.starts_with(|c: char| c.is_whitespace()),
        None => false,
    }
}

/// What the macro visitor needs to know about an ancestor.
#[derive(Clone, Copy)]
enum Parent<'a> {
    VarDecl(&'a VarDeclarator),
    Call(&'a CallExpr),
    /// a statement: its `getStatementRange` children range and its full start
    Stmt { range: Range, pos: usize },
    Other,
}

struct SetupVisitor<'s, 'a> {
    src: &'s Src<'a>,
    options: &'s VueOptions,
    r: ScriptSetupRanges,
}

pub fn parse_script_setup_ranges(src: &Src, options: &VueOptions) -> ScriptSetupRanges {
    let units = &src.units;
    let text_of = |c: &crate::ts_ast::CommentRange| String::from_utf16_lossy(&units[c.pos..c.end]);
    let leading = leading_comment_ranges(units, 0);
    let leading_comment_end_offset =
        leading.iter().rev().find(|c| is_ts_check_comment(&text_of(c))).map(|c| c.end).unwrap_or(0);

    // the import section ends at the first item that is not an import / re-export (or at the
    // end-of-file token, which `forEachChild` also visits)
    let items = module_items(src.p);
    let starts = item_full_starts(src);
    let mut import_section_end_offset = None;
    for (i, item) in items.iter().enumerate() {
        let skip = match item {
            ModuleItem::ModuleDecl(
                ModuleDecl::Import(_) | ModuleDecl::ExportNamed(_) | ModuleDecl::ExportAll(_) | ModuleDecl::TsImportEquals(_),
            ) => true,
            ModuleItem::Stmt(Stmt::Empty(_)) => true,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => {
                matches!(e.decl, Decl::TsTypeAlias(_) | Decl::TsInterface(_))
            }
            _ => false,
        };
        if skip {
            continue;
        }
        import_section_end_offset = Some(first_comment_or_start(src, starts[i], src.p.start(item)));
        break;
    }
    let import_section_end_offset = import_section_end_offset.unwrap_or_else(|| {
        let pos = items.last().map(|i| src.p.end(i)).unwrap_or(0);
        first_comment_or_start(src, pos, skip_trivia(units, pos))
    });

    let mut v = SetupVisitor {
        src,
        options,
        r: ScriptSetupRanges {
            bindings: parse_binding_ranges(src, &options.extensions),
            leading_comment_end_offset,
            import_section_end_offset,
            ..Default::default()
        },
    };
    let mut parents: Vec<Parent> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        v.visit_item(item, starts[i], &mut parents);
    }
    v.r
}

fn first_comment_or_start(src: &Src, pos: usize, start: usize) -> usize {
    let comments = leading_comment_ranges(&src.units, pos);
    match comments.iter().map(|c| c.pos).min() {
        Some(p) => p,
        None => start,
    }
}

impl<'s, 'a> SetupVisitor<'s, 'a> {
    fn range(&self, s: &impl Spanned) -> Range {
        self.src.range(s)
    }

    fn text(&self, s: &impl Spanned) -> String {
        self.src.node_text(s)
    }

    fn var_stmt_range(&self, outer_start: usize, v: &VarDecl) -> Range {
        let end = v.decls.last().map(|d| self.src.p.end(d)).unwrap_or(self.src.p.end(v));
        Range::new(outer_start, end)
    }

    fn visit_item(&mut self, item: &'a ModuleItem, pos: usize, parents: &mut Vec<Parent<'a>>) {
        match item {
            ModuleItem::Stmt(s) => self.visit_stmt(s, pos, parents),
            ModuleItem::ModuleDecl(d) => match d {
                ModuleDecl::ExportDecl(e) => {
                    if let Decl::Var(v) = &e.decl {
                        let range = self.var_stmt_range(self.src.p.start(e), v);
                        parents.push(Parent::Stmt { range, pos });
                        self.visit_var_decls(&v.decls, parents);
                        parents.pop();
                    } else {
                        let range = self.range(e);
                        parents.push(Parent::Stmt { range, pos });
                        self.visit_decl(&e.decl, pos, parents);
                        parents.pop();
                    }
                }
                ModuleDecl::ExportDefaultExpr(e) => {
                    let range = self.range(e.expr.as_ref());
                    parents.push(Parent::Stmt { range, pos });
                    self.visit_expr(&e.expr, parents);
                    parents.pop();
                }
                ModuleDecl::TsExportAssignment(e) => {
                    let range = self.range(e.expr.as_ref());
                    parents.push(Parent::Stmt { range, pos });
                    self.visit_expr(&e.expr, parents);
                    parents.pop();
                }
                ModuleDecl::ExportDefaultDecl(e) => {
                    if let DefaultDecl::Class(c) = &e.decl {
                        let range = self.range(e);
                        parents.push(Parent::Stmt { range, pos });
                        self.visit_class(&c.class, parents);
                        parents.pop();
                    }
                }
                _ => {}
            },
        }
    }

    fn visit_var_decls(&mut self, decls: &'a [VarDeclarator], parents: &mut Vec<Parent<'a>>) {
        parents.push(Parent::Other); // VariableDeclarationList
        for d in decls {
            parents.push(Parent::VarDecl(d));
            if let Some(init) = &d.init {
                self.visit_expr(init, parents);
            }
            parents.pop();
        }
        parents.pop();
    }

    fn visit_decl(&mut self, d: &'a Decl, pos: usize, parents: &mut Vec<Parent<'a>>) {
        match d {
            Decl::Var(v) => self.visit_var_decls(&v.decls, parents),
            Decl::Using(u) => self.visit_var_decls(&u.decls, parents),
            Decl::Class(c) => self.visit_class(&c.class, parents),
            Decl::TsEnum(e) => {
                for m in &e.members {
                    if let Some(init) = &m.init {
                        self.visit_expr(init, parents);
                    }
                }
            }
            Decl::TsModule(m) => {
                if let Some(TsNamespaceBody::TsModuleBlock(b)) = &m.body {
                    let mut prev = pos;
                    for item in &b.body {
                        self.visit_item(item, prev, parents);
                        prev = self.src.p.end(item);
                    }
                }
            }
            // functions are not entered; types contain no calls
            _ => {}
        }
    }

    fn visit_class(&mut self, c: &'a Class, parents: &mut Vec<Parent<'a>>) {
        parents.push(Parent::Other);
        for dec in &c.decorators {
            self.visit_expr(&dec.expr, parents);
        }
        if let Some(s) = &c.super_class {
            self.visit_expr(s, parents);
        }
        for m in &c.body {
            match m {
                ClassMember::ClassProp(p) => {
                    if let PropName::Computed(k) = &p.key {
                        self.visit_expr(&k.expr, parents);
                    }
                    if let Some(v) = &p.value {
                        self.visit_expr(v, parents);
                    }
                }
                ClassMember::PrivateProp(p) => {
                    if let Some(v) = &p.value {
                        self.visit_expr(v, parents);
                    }
                }
                ClassMember::StaticBlock(b) => {
                    let mut prev = self.src.p.start(&b.body) + 1;
                    for s in &b.body.stmts {
                        self.visit_stmt(s, prev, parents);
                        prev = self.src.p.end(s);
                    }
                }
                // methods, accessors and constructors are function-like: not entered at all
                _ => {}
            }
        }
        parents.pop();
    }

    fn visit_block(&mut self, b: &'a BlockStmt, parents: &mut Vec<Parent<'a>>) {
        let range = self.range(b);
        let mut prev = range.start + 1;
        parents.push(Parent::Stmt { range, pos: prev });
        for s in &b.stmts {
            self.visit_stmt(s, prev, parents);
            prev = self.src.p.end(s);
        }
        parents.pop();
    }

    fn visit_stmt(&mut self, s: &'a Stmt, pos: usize, parents: &mut Vec<Parent<'a>>) {
        let full = self.range(s);
        // `getStatementRange`: from the first child to the last one, which drops the keyword of
        // most statements and their `;`
        let trimmed = |end: usize| -> Range {
            let mut e = end;
            if e > full.start && self.src.units.get(e - 1) == Some(&(b';' as u16)) {
                e -= 1;
            }
            Range::new(full.start, e)
        };
        match s {
            Stmt::Decl(Decl::Var(v)) => {
                let range = self.var_stmt_range(full.start, v);
                parents.push(Parent::Stmt { range, pos });
                self.visit_var_decls(&v.decls, parents);
                parents.pop();
            }
            Stmt::Decl(d) => {
                parents.push(Parent::Stmt { range: trimmed(full.end), pos });
                self.visit_decl(d, pos, parents);
                parents.pop();
            }
            Stmt::Expr(e) => {
                parents.push(Parent::Stmt { range: self.range(e.expr.as_ref()), pos });
                self.visit_expr(&e.expr, parents);
                parents.pop();
            }
            Stmt::Block(b) => self.visit_block(b, parents),
            _ => {
                parents.push(Parent::Stmt { range: trimmed(full.end), pos });
                match s {
                    Stmt::If(i) => {
                        self.visit_expr(&i.test, parents);
                        self.visit_stmt(&i.cons, self.src.p.end(i.test.as_ref()), parents);
                        if let Some(a) = &i.alt {
                            self.visit_stmt(a, self.src.p.end(i.cons.as_ref()), parents);
                        }
                    }
                    Stmt::Return(r) => {
                        if let Some(a) = &r.arg {
                            self.visit_expr(a, parents);
                        }
                    }
                    Stmt::Throw(t) => self.visit_expr(&t.arg, parents),
                    Stmt::While(w) => {
                        self.visit_expr(&w.test, parents);
                        self.visit_stmt(&w.body, self.src.p.end(w.test.as_ref()), parents);
                    }
                    Stmt::DoWhile(w) => {
                        self.visit_stmt(&w.body, full.start + 2, parents);
                        self.visit_expr(&w.test, parents);
                    }
                    Stmt::For(f) => {
                        match &f.init {
                            Some(VarDeclOrExpr::VarDecl(v)) => self.visit_var_decls(&v.decls, parents),
                            Some(VarDeclOrExpr::Expr(e)) => self.visit_expr(e, parents),
                            None => {}
                        }
                        if let Some(t) = &f.test {
                            self.visit_expr(t, parents);
                        }
                        if let Some(u) = &f.update {
                            self.visit_expr(u, parents);
                        }
                        self.visit_stmt(&f.body, full.start, parents);
                    }
                    Stmt::ForIn(f) => {
                        self.visit_for_head(&f.left, parents);
                        self.visit_expr(&f.right, parents);
                        self.visit_stmt(&f.body, self.src.p.end(f.right.as_ref()), parents);
                    }
                    Stmt::ForOf(f) => {
                        self.visit_for_head(&f.left, parents);
                        self.visit_expr(&f.right, parents);
                        self.visit_stmt(&f.body, self.src.p.end(f.right.as_ref()), parents);
                    }
                    Stmt::Labeled(l) => self.visit_stmt(&l.body, self.src.p.end(&l.label), parents),
                    Stmt::Switch(sw) => {
                        self.visit_expr(&sw.discriminant, parents);
                        for case in &sw.cases {
                            if let Some(t) = &case.test {
                                self.visit_expr(t, parents);
                            }
                            let mut prev = self.src.p.start(case);
                            for c in &case.cons {
                                self.visit_stmt(c, prev, parents);
                                prev = self.src.p.end(c);
                            }
                        }
                    }
                    Stmt::Try(t) => {
                        self.visit_block(&t.block, parents);
                        if let Some(h) = &t.handler {
                            self.visit_block(&h.body, parents);
                        }
                        if let Some(f) = &t.finalizer {
                            self.visit_block(f, parents);
                        }
                    }
                    Stmt::With(w) => {
                        self.visit_expr(&w.obj, parents);
                        self.visit_stmt(&w.body, self.src.p.end(w.obj.as_ref()), parents);
                    }
                    _ => {}
                }
                parents.pop();
            }
        }
    }

    fn visit_for_head(&mut self, head: &'a ForHead, parents: &mut Vec<Parent<'a>>) {
        match head {
            ForHead::VarDecl(v) => self.visit_var_decls(&v.decls, parents),
            ForHead::UsingDecl(u) => self.visit_var_decls(&u.decls, parents),
            ForHead::Pat(_) => {}
        }
    }

    fn visit_exprs(&mut self, exprs: impl IntoIterator<Item = &'a Expr>, parents: &mut Vec<Parent<'a>>) {
        for e in exprs {
            self.visit_expr(e, parents);
        }
    }

    fn visit_expr(&mut self, e: &'a Expr, parents: &mut Vec<Parent<'a>>) {
        if let Expr::Call(call) = e {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Ident(id) = callee.as_ref() {
                    self.on_macro_call(call, &id.sym, parents);
                }
            }
        }
        // children (function-like nodes are not entered)
        match e {
            Expr::Fn(_) | Expr::Arrow(_) => {}
            Expr::Array(a) => {
                parents.push(Parent::Other);
                self.visit_exprs(a.elems.iter().flatten().map(|x| x.expr.as_ref()), parents);
                parents.pop();
            }
            Expr::Object(o) => {
                parents.push(Parent::Other);
                for p in &o.props {
                    match p {
                        PropOrSpread::Spread(s) => self.visit_expr(&s.expr, parents),
                        PropOrSpread::Prop(p) => match p.as_ref() {
                            Prop::KeyValue(kv) => {
                                parents.push(Parent::Other);
                                if let PropName::Computed(k) = &kv.key {
                                    self.visit_expr(&k.expr, parents);
                                }
                                self.visit_expr(&kv.value, parents);
                                parents.pop();
                            }
                            Prop::Assign(a) => self.visit_expr(&a.value, parents),
                            // methods and accessors are function-like: not entered at all
                            Prop::Getter(_) | Prop::Setter(_) | Prop::Method(_) | Prop::Shorthand(_) => {}
                        },
                    }
                }
                parents.pop();
            }
            Expr::Unary(u) => self.visit_child(&u.arg, parents),
            Expr::Update(u) => self.visit_child(&u.arg, parents),
            Expr::Bin(b) => {
                parents.push(Parent::Other);
                self.visit_expr(&b.left, parents);
                self.visit_expr(&b.right, parents);
                parents.pop();
            }
            Expr::Assign(a) => {
                parents.push(Parent::Other);
                if let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = &a.left {
                    self.visit_member(m, parents);
                }
                self.visit_expr(&a.right, parents);
                parents.pop();
            }
            Expr::Member(m) => {
                parents.push(Parent::Other);
                self.visit_member(m, parents);
                parents.pop();
            }
            Expr::Cond(c) => {
                parents.push(Parent::Other);
                self.visit_expr(&c.test, parents);
                self.visit_expr(&c.cons, parents);
                self.visit_expr(&c.alt, parents);
                parents.pop();
            }
            Expr::Call(c) => {
                parents.push(Parent::Call(c));
                if let Callee::Expr(callee) = &c.callee {
                    self.visit_expr(callee, parents);
                }
                self.visit_exprs(c.args.iter().map(|a| a.expr.as_ref()), parents);
                parents.pop();
            }
            Expr::New(n) => {
                parents.push(Parent::Other);
                self.visit_expr(&n.callee, parents);
                if let Some(args) = &n.args {
                    self.visit_exprs(args.iter().map(|a| a.expr.as_ref()), parents);
                }
                parents.pop();
            }
            Expr::Seq(s) => {
                // `a, b, c` is a left-nested chain of binary expressions in TypeScript
                for _ in 1..s.exprs.len() {
                    parents.push(Parent::Other);
                }
                for (i, x) in s.exprs.iter().enumerate() {
                    self.visit_expr(x, parents);
                    if i > 0 {
                        parents.pop();
                    }
                }
            }
            Expr::Tpl(t) => {
                parents.push(Parent::Other);
                self.visit_exprs(t.exprs.iter().map(|x| x.as_ref()), parents);
                parents.pop();
            }
            Expr::TaggedTpl(t) => {
                parents.push(Parent::Other);
                self.visit_expr(&t.tag, parents);
                self.visit_exprs(t.tpl.exprs.iter().map(|x| x.as_ref()), parents);
                parents.pop();
            }
            Expr::Class(c) => self.visit_class(&c.class, parents),
            Expr::Yield(y) => {
                if let Some(a) = &y.arg {
                    self.visit_child(a, parents);
                }
            }
            Expr::Await(a) => self.visit_child(&a.arg, parents),
            Expr::Paren(p) => self.visit_child(&p.expr, parents),
            Expr::TsTypeAssertion(a) => self.visit_child(&a.expr, parents),
            Expr::TsConstAssertion(a) => self.visit_child(&a.expr, parents),
            Expr::TsNonNull(a) => self.visit_child(&a.expr, parents),
            Expr::TsAs(a) => self.visit_child(&a.expr, parents),
            Expr::TsInstantiation(a) => self.visit_child(&a.expr, parents),
            Expr::TsSatisfies(a) => self.visit_child(&a.expr, parents),
            Expr::OptChain(o) => {
                parents.push(Parent::Other);
                match o.base.as_ref() {
                    OptChainBase::Member(m) => self.visit_member(m, parents),
                    OptChainBase::Call(c) => {
                        self.visit_expr(&c.callee, parents);
                        self.visit_exprs(c.args.iter().map(|a| a.expr.as_ref()), parents);
                    }
                }
                parents.pop();
            }
            _ => {}
        }
    }

    fn visit_child(&mut self, e: &'a Expr, parents: &mut Vec<Parent<'a>>) {
        parents.push(Parent::Other);
        self.visit_expr(e, parents);
        parents.pop();
    }

    fn visit_member(&mut self, m: &'a MemberExpr, parents: &mut Vec<Parent<'a>>) {
        self.visit_expr(&m.obj, parents);
        if let MemberProp::Computed(c) = &m.prop {
            self.visit_expr(&c.expr, parents);
        }
    }

    fn call_range(&self, call: &CallExpr) -> CallRange {
        let Callee::Expr(callee) = &call.callee else { unreachable!() };
        CallRange {
            call_exp: self.range(call),
            exp: self.range(callee.as_ref()),
            arg: call.args.first().map(|a| self.range(a.expr.as_ref())),
            type_arg: call.type_args.as_ref().and_then(|t| t.params.first()).map(|t| self.range(t.as_ref())),
        }
    }

    fn assignment_name(&self, parent: Option<&Parent>) -> Option<String> {
        match parent {
            Some(Parent::VarDecl(d)) => match &d.name {
                Pat::Ident(b) => Some(b.id.sym.to_string()),
                _ => None,
            },
            _ => None,
        }
    }

    fn statement_range(&self, call: &CallExpr, parents: &[Parent]) -> Range {
        for p in parents.iter().rev() {
            if let Parent::Stmt { range, .. } = p {
                return *range;
            }
        }
        self.range(call)
    }

    fn on_macro_call(&mut self, call: &'a CallExpr, name: &str, parents: &[Parent<'a>]) {
        let parent = parents.last();
        let macros = &self.options.macros;
        let composables = &self.options.composables;
        let has = |list: &[String]| list.iter().any(|m| m == name);
        if has(&macros.define_model) {
            let mut dm = DefineModel::default();
            if let Some(Parent::VarDecl(d)) = parent {
                if let Pat::Ident(b) = &d.name {
                    dm.local_name = Some(self.range(&b.id));
                }
            }
            if let Some(t) = &call.type_args {
                if let Some(first) = t.params.first() {
                    dm.type_ = Some(self.range(first.as_ref()));
                }
                if let Some(second) = t.params.get(1) {
                    dm.modifier_type = Some(self.range(second.as_ref()));
                }
            }
            let is_string_like = |e: &Expr| matches!(e, Expr::Lit(Lit::Str(_))) || matches!(e, Expr::Tpl(t) if t.exprs.is_empty());
            let (mut prop_name, mut opts): (Option<&Expr>, Option<&Expr>) = (None, None);
            if call.args.len() >= 2 {
                prop_name = Some(&call.args[0].expr);
                opts = Some(&call.args[1].expr);
            } else if call.args.len() == 1 {
                if is_string_like(&call.args[0].expr) {
                    prop_name = Some(&call.args[0].expr);
                } else {
                    opts = Some(&call.args[0].expr);
                }
            }
            if let Some(Expr::Object(o)) = opts {
                for p in &o.props {
                    let PropOrSpread::Prop(p) = p else { continue };
                    let Prop::KeyValue(kv) = p.as_ref() else { continue };
                    let Some(key) = prop_name_ident(&kv.key) else { continue };
                    match &*key.sym {
                        "type" => dm.runtime_type = Some(self.range(kv.value.as_ref())),
                        "default" => dm.default_value = Some(self.range(kv.value.as_ref())),
                        "required" => {
                            if matches!(kv.value.as_ref(), Expr::Lit(Lit::Bool(Bool { value: true, .. }))) {
                                dm.required = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            if let Some(pn) = prop_name {
                if is_string_like(pn) {
                    dm.name = Some(self.range(pn));
                }
            }
            dm.comments = self.closest_comment(parents);
            dm.arg = Some(self.range(call));
            self.r.define_model.push(dm);
        } else if has(&macros.define_props) {
            let mut dp = DefineProps {
                call: self.call_range(call),
                name: self.assignment_name(parent),
                statement: self.statement_range(call, parents),
                ..Default::default()
            };
            if let Some(Parent::VarDecl(d)) = parent {
                if let Pat::Object(_) = &d.name {
                    let mut ids = Vec::new();
                    collect_binding_identifiers(self.src, &d.name, &mut ids, false, None);
                    let mut map = IndexMap::new();
                    for id in ids {
                        if id.is_rest {
                            dp.destructured_rest = Some(id.name);
                        } else {
                            map.insert(id.name, id.initializer);
                        }
                    }
                    dp.destructured = Some(map);
                }
            } else if let Some(Parent::Call(outer)) = parent {
                if let Callee::Expr(callee) = &outer.callee {
                    if has_name(&macros.with_defaults, &self.text(callee.as_ref())) {
                        if let Some(Parent::VarDecl(grand)) = parents.len().checked_sub(2).map(|i| &parents[i]) {
                            dp.name = Some(self.pat_text(&grand.name));
                        }
                    }
                }
            }
            self.r.define_props = Some(dp);
        } else if has(&macros.with_defaults) {
            let Callee::Expr(callee) = &call.callee else { return };
            self.r.with_defaults = Some(CallRange {
                call_exp: self.range(call),
                exp: self.range(callee.as_ref()),
                arg: call.args.get(1).map(|a| self.range(a.expr.as_ref())),
                type_arg: None,
            });
        } else if has(&macros.define_emits) {
            let mut de = DefineEmits {
                call: self.call_range(call),
                name: self.assignment_name(parent),
                statement: self.statement_range(call, parents),
                ..Default::default()
            };
            if let Some(t) = call.type_args.as_ref().and_then(|t| t.params.first()) {
                if let TsType::TsTypeLit(lit) = t.as_ref() {
                    for m in &lit.members {
                        if let TsTypeElement::TsCallSignatureDecl(sig) = m {
                            let ty = sig.params.first().and_then(|p| match p {
                                TsFnParam::Ident(b) => b.type_ann.as_ref(),
                                TsFnParam::Array(a) => a.type_ann.as_ref(),
                                TsFnParam::Rest(r) => r.type_ann.as_ref(),
                                TsFnParam::Object(o) => o.type_ann.as_ref(),
                            });
                            if let Some(ty) = ty {
                                if matches!(
                                    ty.type_ann.as_ref(),
                                    TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(_))
                                ) {
                                    de.has_union_type_arg = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            self.r.define_emits = Some(de);
        } else if has(&macros.define_slots) {
            self.r.define_slots = Some(DefineSlots {
                call: self.call_range(call),
                name: self.assignment_name(parent),
                statement: self.statement_range(call, parents),
            });
        } else if has(&macros.define_expose) {
            self.r.define_expose = Some(self.call_range(call));
        } else if has(&macros.define_options)
            && call.args.first().is_some_and(|a| a.spread.is_none() && matches!(a.expr.as_ref(), Expr::Object(_)))
        {
            let Expr::Object(obj) = call.args[0].expr.as_ref() else { unreachable!() };
            let mut d = DefineOptions::default();
            for p in &obj.props {
                let PropOrSpread::Prop(p) = p else { continue };
                let Prop::KeyValue(kv) = p.as_ref() else { continue };
                let Some(key) = prop_name_ident(&kv.key) else { continue };
                match &*key.sym {
                    "inheritAttrs" => d.inherit_attrs = Some(self.text(kv.value.as_ref())),
                    "name" => {
                        if let Expr::Lit(Lit::Str(s)) = kv.value.as_ref() {
                            d.name = Some(s.value.to_string_lossy().into_owned());
                        }
                    }
                    _ => {}
                }
            }
            self.r.define_options = Some(d);
        } else if has(&composables.use_attrs) {
            self.r.use_attrs.push(self.call_range(call));
        } else if has(&composables.use_css_module) {
            self.r.use_css_module.push(self.call_range(call));
        } else if has(&composables.use_slots) {
            self.r.use_slots.push(self.call_range(call));
        } else if has(&composables.use_template_ref) && call.type_args.as_ref().is_none_or(|t| t.params.is_empty()) {
            self.r.use_template_ref.push(UseTemplateRef { call: self.call_range(call), name: self.assignment_name(parent) });
        }
    }

    /// `getNodeText(declaration.name)`: identifiers without their type annotation.
    fn pat_text(&self, pat: &Pat) -> String {
        match pat {
            Pat::Ident(b) => b.id.sym.to_string(),
            other => {
                let r = self.range(other);
                let text = self.src.slice(r);
                // swc's pattern span includes a type annotation, TypeScript's name does not
                match other {
                    Pat::Object(o) if o.type_ann.is_some() => cut_type_annotation(&text, '}'),
                    Pat::Array(a) if a.type_ann.is_some() => cut_type_annotation(&text, ']'),
                    _ => text,
                }
            }
        }
    }

    /// `getClosestMultiLineCommentRange(ts, node, parents)` for a call expression.
    fn closest_comment(&self, parents: &[Parent]) -> Option<Range> {
        let pos = parents
            .iter()
            .rev()
            .find_map(|p| match p {
                Parent::Stmt { pos, .. } => Some(*pos),
                _ => None,
            })
            .unwrap_or(0);
        self.src.closest_multi_line_comment(pos)
    }
}

fn has_name(list: &[String], name: &str) -> bool {
    list.iter().any(|m| m == name)
}

fn cut_type_annotation(text: &str, close: char) -> String {
    match text.rfind(close) {
        Some(i) => text[..=i].to_string(),
        None => text.to_string(),
    }
}
