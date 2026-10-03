//! The codegen's output unit, a port of Volar's `Segment<VueCodeInformation>`: either generated text,
//! or text copied from (and mapped to) an offset in one of the SFC blocks.

use std::borrow::Cow;
use std::cell::Cell;

/// The block a mapped segment points into (`IRBlock.name` in language-core). Offsets are relative to
/// the block content, except for `Main`, which is the whole SFC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Src {
    Main,
    Template,
    Script,
    ScriptSetup,
    Style(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Verification {
    #[default]
    Absent,
    True,
    False,
    /// `{ shouldReport: () => false }` (inside a `@vue-expect-error` region)
    Expect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Semantic {
    #[default]
    Absent,
    True,
    /// `{ shouldHighlight: () => false }`
    NoHighlight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Navigation {
    #[default]
    Absent,
    True,
    /// `{ shouldRename: () => false }`
    NoRename,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Shorthand {
    #[default]
    Absent,
    Html,
    Js,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Ignore,
    Expect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Anchor,
    Content,
    End,
}

/// `__diagnosticDirective`; `id` identifies the directive object (JS compares by reference).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectiveMarker {
    pub id: u32,
    pub policy: Policy,
    pub original_length: u32,
    pub phase: Phase,
}

/// `VueCodeInformation`. Every field can be absent, because the reference merges these objects with
/// spread syntax and the right-hand object only overrides the keys it actually has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Feat {
    pub verification: Verification,
    pub completion: bool,
    pub semantic: Semantic,
    pub navigation: Navigation,
    pub structure: bool,
    pub format: bool,
    pub import_completion: bool,
    pub props_completion: bool,
    pub shorthand: Shorthand,
    /// `__combineToken`, 0 when absent
    pub combine: u32,
    pub directive: Option<DirectiveMarker>,
}

impl Feat {
    pub const EMPTY: Feat = Feat {
        verification: Verification::Absent,
        completion: false,
        semantic: Semantic::Absent,
        navigation: Navigation::Absent,
        structure: false,
        format: false,
        import_completion: false,
        props_completion: false,
        shorthand: Shorthand::Absent,
        combine: 0,
        directive: None,
    };

    /// `{ ...self, ...other }`
    pub const fn merge(self, other: Feat) -> Feat {
        Feat {
            verification: match other.verification {
                Verification::Absent => self.verification,
                v => v,
            },
            completion: self.completion || other.completion,
            semantic: match other.semantic {
                Semantic::Absent => self.semantic,
                v => v,
            },
            navigation: match other.navigation {
                Navigation::Absent => self.navigation,
                v => v,
            },
            structure: self.structure || other.structure,
            format: self.format || other.format,
            import_completion: self.import_completion || other.import_completion,
            props_completion: self.props_completion || other.props_completion,
            shorthand: match other.shorthand {
                Shorthand::Absent => self.shorthand,
                v => v,
            },
            combine: if other.combine != 0 { other.combine } else { self.combine },
            directive: match other.directive {
                None => self.directive,
                v => v,
            },
        }
    }

    pub const fn with_combine(self, token: u32) -> Feat {
        let mut f = self;
        f.combine = token;
        f
    }
}

/// `codeFeatures` presets.
pub mod features {
    use super::*;

    const fn f() -> Feat {
        Feat::EMPTY
    }

    pub const FULL: Feat = Feat {
        verification: Verification::True,
        completion: true,
        semantic: Semantic::True,
        navigation: Navigation::True,
        structure: true,
        format: true,
        ..f()
    };
    pub const ALL: Feat = Feat {
        verification: Verification::True,
        completion: true,
        semantic: Semantic::True,
        navigation: Navigation::True,
        ..f()
    };
    pub const IMPORT_COMPLETION_ONLY: Feat = Feat { import_completion: true, ..f() };
    pub const VERIFICATION: Feat = Feat { verification: Verification::True, ..f() };
    pub const COMPLETION: Feat = Feat { completion: true, ..f() };
    pub const WITHOUT_COMPLETION: Feat = Feat {
        verification: Verification::True,
        semantic: Semantic::True,
        navigation: Navigation::True,
        ..f()
    };
    pub const NAVIGATION: Feat = Feat { navigation: Navigation::True, ..f() };
    pub const NAVIGATION_WITHOUT_RENAME: Feat = Feat { navigation: Navigation::NoRename, ..f() };
    pub const NAVIGATION_AND_COMPLETION: Feat = Feat {
        navigation: Navigation::True,
        completion: true,
        ..f()
    };
    pub const NAVIGATION_AND_VERIFICATION: Feat = Feat {
        navigation: Navigation::True,
        verification: Verification::True,
        ..f()
    };
    pub const WITHOUT_NAVIGATION: Feat = Feat {
        verification: Verification::True,
        completion: true,
        semantic: Semantic::True,
        ..f()
    };
    pub const SEMANTIC_WITHOUT_HIGHLIGHT: Feat = Feat { semantic: Semantic::NoHighlight, ..f() };
    pub const WITHOUT_HIGHLIGHT: Feat = Feat {
        semantic: Semantic::NoHighlight,
        verification: Verification::True,
        navigation: Navigation::True,
        completion: true,
        ..f()
    };
    pub const WITHOUT_HIGHLIGHT_AND_COMPLETION: Feat = Feat {
        semantic: Semantic::NoHighlight,
        verification: Verification::True,
        navigation: Navigation::True,
        ..f()
    };
    pub const WITHOUT_SEMANTIC: Feat = Feat {
        verification: Verification::True,
        navigation: Navigation::True,
        completion: true,
        ..f()
    };
    pub const PROPS_COMPLETION: Feat = Feat { props_completion: true, ..f() };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Code {
    /// generated text; mostly static pieces, which need no allocation
    Text(Cow<'static, str>),
    Seg { text: String, src: Src, offset: u32, feat: Feat },
}

impl Code {
    pub fn text(&self) -> &str {
        match self {
            Code::Text(t) => t,
            Code::Seg { text, .. } => text,
        }
    }
}

pub type Codes = Vec<Code>;

/// `toString(codes)`
pub fn codes_to_string(codes: &[Code]) -> String {
    let mut s = String::new();
    for c in codes {
        s.push_str(c.text());
    }
    s
}

thread_local! {
    static NEXT_ID: Cell<u32> = const { Cell::new(1) };
}

/// A fresh `Symbol()` (combine token / directive identity), unique within the thread.
pub fn next_id() -> u32 {
    NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id.wrapping_add(1).max(1));
        id
    })
}

