//! A TypeScript 7 content mapper for Vue single-file components: a Rust port of the virtual-code
//! generation of `@vue/language-core` (vuejs/language-tools PR #6170, see tools/reference).

pub mod code;
pub mod codegen;
pub mod mappings;
pub mod names;
pub mod options;
pub mod paths;
pub mod sfc;
pub mod shared;
pub mod template;
pub mod text;
pub mod parsers;
pub mod ts_ast;
pub mod style;
pub mod vue_tsx;

pub use vue_tsx::{TransformResult, transform};
