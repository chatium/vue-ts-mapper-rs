//! `codegen/template/interpolation.ts`

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
    pub destructured_props: &'a IndexSet<String>,
    pub imported_components: &'a IndexSet<String>,
    pub setup_refs: &'a IndexSet<String>,
    pub setup_bindings: &'a IndexSet<String>,
    pub dot_value_bindings: &'a IndexSet<String>,
    pub lib: &'a str,
    pub script_lang: &'a str,
}

struct Access {
    name: String,
    offset: usize,
    is_shorthand: bool,
    is_narrowing: bool,
    in_type_query: bool,
    is_new_operand: bool,
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
        out.t(prefix);
    }
    let text = Text::new(code);
    let mut prev_end = 0usize;
    for a in for_each_identifiers(ctx, code, prefix, suffix, in_narrowing) {
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
        if o.destructured_props.contains(name) || o.imported_components.contains(name) {
            out.seg(name.clone(), src, at, identifier_data);
        } else if o.setup_refs.contains(name) {
            out.seg(name.clone(), src, at, data);
            out.seg(".value", src, at, features::VERIFICATION);
        } else if o.setup_bindings.contains(name) {
            ctx.access_variable(src, name, Some(at), a.in_type_query || a.is_narrowing);
            if a.in_type_query || o.dot_value_bindings.contains(name) {
                out.seg(name.clone(), src, at, identifier_data);
                out.seg(".value", src, at, features::VERIFICATION);
            } else {
                if a.is_new_operand {
                    out.t("(");
                }
                out.t(format!("{}(", names::UNWRAP));
                out.seg(name.clone(), src, at, identifier_data);
                out.t(format!(", {})", ref_brand_argument(o.lib, o.script_lang)));
                if a.is_new_operand {
                    out.t(")");
                }
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
        out.t(suffix);
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

fn for_each_identifiers(ctx: &mut Ctx, code: &str, prefix: &str, suffix: &str, in_narrowing: bool) -> Vec<Access> {
    if is_identifier(code) && !should_identifier_skipped(&ctx.scopes, code) {
        return vec![Access {
            name: code.to_string(),
            offset: 0,
            is_shorthand: false,
            is_narrowing: in_narrowing,
            in_type_query: false,
            is_new_operand: false,
        }];
    }
    let scope = ctx.scope();
    let full = format!("{prefix}{code}{suffix}");
    let parsed = ts_ast::parse_as(&full, false, false);
    let units: Vec<u16> = full.encode_utf16().collect();
    let items = {
        let mut w = Walker { p: &parsed, units: &units, scopes: &mut ctx.scopes, items: Vec::new() };
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
        })
        .collect()
}
