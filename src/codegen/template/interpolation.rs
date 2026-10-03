//! `codegen/template/interpolation.ts`

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexSet;

use crate::code::{Feat, Shorthand, Src, features};
use crate::codegen::template::binding_references::{Walker, should_identifier_skipped};
use crate::codegen::template::context::Ctx;
use crate::codegen::{Boundary, Out, ref_brand_argument};
use crate::names;
use crate::shared::is_identifier;
use crate::text::{Text, len16};
use crate::ts_ast;

/// The options `generateInterpolation` reads, shared by the template and style codegen.
pub struct InterpOpts<'a> {
    pub setup_consts: &'a IndexSet<String>,
    pub imported_components: &'a IndexSet<String>,
    pub setup_refs: &'a IndexSet<String>,
    pub setup_bindings: &'a IndexSet<String>,
    pub dot_value_bindings: &'a IndexSet<String>,
    /// imports and `let` / `var` bindings among them (`BindingFlag.Variable`)
    pub reassert_bindings: &'a IndexSet<String>,
    pub lib: &'a str,
    pub script_lang: &'a str,
    pub cache: &'a ExprCache,
}

/// Parsed interpolation texts of one SFC, shared by its codegen passes (the reference caches
/// them per block too).
pub type ExprCache = RefCell<HashMap<String, Rc<(ts_ast::Parsed, Vec<u16>)>>>;

struct Access {
    name: String,
    offset: usize,
    is_shorthand: bool,
    is_narrowing: bool,
    in_type_query: bool,
    is_new_operand: bool,
    in_function: bool,
    is_write: bool,
}

/// `generateInterpolation(options, ctx, block, data, code, start, prefix, suffix, inNarrowing)`
#[allow(clippy::too_many_arguments)]
pub fn interpolation(
    o: &InterpOpts,
    ctx: &mut Ctx,
    out: &mut Out,
    src: Src,
    data: Feat,
    code: &str,
    start: usize,
    prefix: &str,
    suffix: &str,
    in_narrowing: bool,
) {
    if !prefix.is_empty() {
        out.t(prefix.to_string());
    }
    let text = Text::new(code);
    let mut prev_end = 0usize;
    for a in for_each_identifiers(o.cache, ctx, code, prefix, suffix, in_narrowing) {
        let identifier_data = if a.is_shorthand { Feat { shorthand: Shorthand::Js, ..data } } else { data };
        let name_len = len16(&a.name);
        if a.is_shorthand {
            non_identifier_code(out, text.slice(prev_end, a.offset + name_len), src, start + prev_end, data, prev_end > 0);
            out.t(": ");
        } else if prev_end < a.offset {
            non_identifier_code(out, text.slice(prev_end, a.offset), src, start + prev_end, data, prev_end > 0);
        }

        let name = &a.name;
        let at = start + a.offset;
        if o.setup_consts.contains(name) || o.imported_components.contains(name) {
            out.seg(name.clone(), src, at, identifier_data);
        } else if o.setup_refs.contains(name) {
            out.seg(name.clone(), src, at, identifier_data);
            dot_value(out, src, at, at + name_len);
        } else if o.setup_bindings.contains(name) {
            ctx.access_variable(src, name, Some(at), a.in_type_query || a.is_narrowing);
            // inside a function written in the expression, the `.value` assertion does not reach
            // imports and `let` / `var` bindings: read those like any other binding (a write needs
            // an assignable reference, so it keeps `.value`)
            let with_dot_value = o.dot_value_bindings.contains(name)
                && !(a.in_function && !a.is_write && o.reassert_bindings.contains(name));
            if a.in_type_query || with_dot_value {
                out.seg(name.clone(), src, at, identifier_data);
                dot_value(out, src, at, at + name_len);
            } else {
                // the wrapper maps to the identifier, so that diagnostics on the whole access
                // (`… is possibly 'null'`) are reported, as vue-tsc reports them on `__VLS_ctx.name`
                let b = Boundary::start(out, src, at, at + name_len, features::VERIFICATION);
                if a.is_new_operand {
                    out.t("(");
                }
                out.t(format!("{}(", names::UNWRAP));
                out.seg(name.clone(), src, at, identifier_data);
                out.t(format!(", {})", ref_brand_argument(o.lib, o.script_lang)));
                if a.is_new_operand {
                    out.t(")");
                }
                b.end(out);
            }
        } else {
            // #1205, #1264
            let b = Boundary::start(out, src, at, at + name_len, features::VERIFICATION);
            if ctx.dollar_vars.contains(name) {
                out.t(names::DOLLARS);
            } else {
                ctx.access_variable(src, name, Some(at), false);
                out.t(names::CTX);
            }
            out.t(".");
            out.seg(name.clone(), src, at, identifier_data);
            b.end(out);
        }
        prev_end = a.offset + name_len;
    }
    if prev_end < text.len() {
        non_identifier_code(out, text.slice_from(prev_end), src, start + prev_end, data, prev_end > 0);
    }
    if !suffix.is_empty() {
        out.t(suffix.to_string());
    }
}

/// `generateNonIdentifierCode`: the boundary character is cut off as verification-only.
fn non_identifier_code(out: &mut Out, code: &str, src: Src, offset: usize, data: Feat, should_cut: bool) {
    if code.is_empty() {
        return;
    }
    if !should_cut {
        out.seg(code, src, offset, data);
        return;
    }
    let first = code.chars().next().unwrap();
    let (head, tail) = code.split_at(first.len_utf8());
    out.seg(head, src, offset, Feat { verification: data.verification, ..Feat::EMPTY });
    if !tail.is_empty() {
        out.seg(tail, src, offset + first.len_utf16(), data);
    }
}

fn for_each_identifiers(cache: &ExprCache, ctx: &mut Ctx, code: &str, prefix: &str, suffix: &str, in_narrowing: bool) -> Vec<Access> {
    if is_identifier(code) && !should_identifier_skipped(&ctx.scopes, code) {
        return vec![Access {
            name: code.to_string(),
            offset: 0,
            is_shorthand: false,
            is_narrowing: in_narrowing,
            in_type_query: false,
            is_new_operand: false,
            in_function: false,
            is_write: false,
        }];
    }
    let scope = ctx.scope();
    let full = format!("{prefix}{code}{suffix}");
    let cached = cache.borrow().get(&full).cloned();
    let parsed = cached.unwrap_or_else(|| {
        let parsed = Rc::new((ts_ast::parse_as(&full, false, false), full.encode_utf16().collect()));
        cache.borrow_mut().insert(full, parsed.clone());
        parsed
    });
    let items = {
        let mut w = Walker { p: &parsed.0, units: &parsed.1, scopes: &mut ctx.scopes, items: Vec::new(), function_depth: 0 };
        w.program(scope, in_narrowing);
        w.items
    };
    ctx.end_scope_silent();
    let prefix_len = len16(prefix);
    items
        .into_iter()
        .filter(|i| !i.skipped)
        .map(|i| Access {
            offset: i.start.wrapping_sub(prefix_len),
            name: i.name,
            is_shorthand: i.is_shorthand,
            is_narrowing: i.is_narrowing,
            in_type_query: i.in_type_query,
            is_new_operand: i.is_new_operand,
            in_function: i.in_function,
            is_write: i.is_write,
        })
        .collect()
}

/// `.value` after a binding, mapped back to the binding (vue-tsc 3.3.12).
pub fn dot_value(out: &mut Out, src: Src, start: usize, end: usize) {
    out.t(".");
    let b = Boundary::start(out, src, start, end, features::VERIFICATION);
    out.t("value");
    b.end(out);
}
