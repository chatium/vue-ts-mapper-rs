//! Generated codes → Volar mappings (`buildMappings` and the `__combineToken` merge of
//! `getMappingsForCode`) → the content-mapper protocol's span mappings (`toSpanMappings`) and
//! diagnostic directives (`toDiagnosticDirectives`, `withSynthesizedDiagnosticIgnores`).

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::code::{Code, Feat, Navigation, Phase, Policy, Semantic, Src};
use crate::text::{Text, len16};

/// A Volar mapping: one or more ranges sharing their data.
pub struct VolarMapping {
    pub source: Vec<i64>,
    pub generated: Vec<i64>,
    pub lengths: Vec<i64>,
    pub feat: Feat,
}

/// `getMappingsForCode`: block-relative offsets become SFC offsets (`block_start` gives a block's
/// `startTagEnd`), and the ranges of a combine token join the mapping that first used it.
pub fn build(codes: &[Code], block_start: impl Fn(Src) -> Option<usize>) -> Vec<VolarMapping> {
    let mut length = 0i64;
    let mut mappings: Vec<VolarMapping> = Vec::new();
    let mut tokens: HashMap<u32, usize> = HashMap::new();
    for code in codes {
        match code {
            Code::Text(t) => length += len16(t) as i64,
            Code::Seg { text, src, offset, feat } => {
                let l = len16(text) as i64;
                let source = *offset as i64 + block_start(*src).unwrap_or(0) as i64;
                if feat.combine != 0 {
                    if let Some(&i) = tokens.get(&feat.combine) {
                        let m = &mut mappings[i];
                        m.source.push(source);
                        m.generated.push(length);
                        m.lengths.push(l);
                        length += l;
                        continue;
                    }
                    tokens.insert(feat.combine, mappings.len());
                }
                mappings.push(VolarMapping { source: vec![source], generated: vec![length], lengths: vec![l], feat: *feat });
                length += l;
            }
        }
    }
    mappings
}

// SpanMapFeature
const HOVER: u32 = 1 << 0;
const SIGNATURE_HELP: u32 = 1 << 1;
const COMPLETION: u32 = 1 << 2;
const DEFINITION: u32 = 1 << 3;
const TYPE_DEFINITION: u32 = 1 << 4;
const IMPLEMENTATION: u32 = 1 << 5;
const REFERENCES: u32 = 1 << 6;
const DOCUMENT_HIGHLIGHTS: u32 = 1 << 7;
const RENAME: u32 = 1 << 8;
const CALL_HIERARCHY: u32 = 1 << 9;
const CODE_ACTIONS: u32 = 1 << 10;
const FORMATTING: u32 = 1 << 11;
const INLAY_HINTS: u32 = 1 << 12;
const SEMANTIC_TOKENS: u32 = 1 << 13;
const FOLDING_RANGES: u32 = 1 << 14;
const SELECTION_RANGES: u32 = 1 << 15;
const LINKED_EDITING: u32 = 1 << 16;
const AUTO_INSERT: u32 = 1 << 17;
const DOCUMENT_SYMBOLS: u32 = 1 << 18;
const CODE_LENS: u32 = 1 << 19;

const SEMANTIC_FEATURES: u32 = HOVER | SIGNATURE_HELP | INLAY_HINTS | SEMANTIC_TOKENS;
const COMPLETION_FEATURES: u32 = COMPLETION | AUTO_INSERT;
const NAVIGATION_FEATURES: u32 = DEFINITION
    | TYPE_DEFINITION
    | IMPLEMENTATION
    | REFERENCES
    | DOCUMENT_HIGHLIGHTS
    | RENAME
    | CALL_HIERARCHY
    | CODE_ACTIONS
    | LINKED_EDITING;
const STRUCTURE_FEATURES: u32 = FOLDING_RANGES | SELECTION_RANGES | DOCUMENT_SYMBOLS | CODE_LENS;

pub const VERBATIM: u8 = 0;
pub const ATOM: u8 = 1;

fn features_of(f: &Feat) -> u32 {
    let mut features = 0;
    if f.semantic != Semantic::Absent {
        features |= SEMANTIC_FEATURES;
        if f.semantic == Semantic::NoHighlight {
            features &= !SEMANTIC_TOKENS;
        }
    }
    if f.completion {
        features |= COMPLETION_FEATURES;
    }
    if f.navigation != Navigation::Absent {
        features |= NAVIGATION_FEATURES;
        if f.navigation == Navigation::NoRename {
            features &= !RENAME;
        }
    }
    if f.structure {
        features |= STRUCTURE_FEATURES;
    }
    if f.format {
        features |= FORMATTING;
    }
    features
}

/// `[generatedStart, generatedLength, originalStart, originalLength, kind, features]`
pub type SpanMapping = (i64, i64, i64, i64, u8, u32);

struct Entry<'m> {
    generated_start: i64,
    generated_length: i64,
    original_start: i64,
    original_length: i64,
    feat: &'m Feat,
    token: u32,
}

struct Candidate {
    generated_start: i64,
    generated_end: i64,
    original_start: i64,
    original_end: i64,
    kind: u8,
    features: u32,
}

/// `toSpanMappings(mappings, generatedText, originalText, languageFeatures)`
pub fn to_span_mappings(mappings: &[VolarMapping], generated: &Text, original: &Text, language_features: bool) -> Vec<SpanMapping> {
    let mut entries: Vec<Entry> = Vec::new();
    for m in mappings {
        for i in 0..m.lengths.len() {
            let (g, o, l) = (m.generated[i], m.source[i], m.lengths[i]);
            if l < 0 || g < 0 || o < 0 {
                continue;
            }
            entries.push(Entry {
                generated_start: g,
                generated_length: l,
                original_start: o,
                original_length: l,
                feat: &m.feat,
                token: m.feat.combine,
            });
        }
    }

    // Mappings sharing a multi-use combine token are chunks of one logical token; each group
    // becomes a single span. A token used once stays individual.
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for e in &entries {
        if e.token != 0 {
            *counts.entry(e.token).or_default() += 1;
        }
    }
    let mut groups: IndexMap<u32, Vec<usize>> = IndexMap::new();
    for (i, e) in entries.iter().enumerate() {
        if e.token != 0 && counts[&e.token] > 1 {
            groups.entry(e.token).or_default().push(i);
        }
    }
    let grouped: HashSet<usize> = groups.values().flatten().copied().collect();
    let individual = entries.len();
    for group in groups.values_mut() {
        group.sort_by_key(|&i| entries[i].generated_start);
        let first = &entries[group[0]];
        let last = &entries[*group.last().unwrap()];
        let combined = Entry {
            generated_start: first.generated_start,
            generated_length: last.generated_start + last.generated_length - first.generated_start,
            original_start: first.original_start,
            original_length: last.original_start + last.original_length - first.original_start,
            feat: first.feat,
            token: first.token,
        };
        entries.push(combined);
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if i < individual && grouped.contains(&i) {
            continue;
        }
        if e.generated_start + e.generated_length > generated.len() as i64
            || e.original_start + e.original_length > original.len() as i64
        {
            continue;
        }
        let generated_end = e.generated_start + e.generated_length;
        let original_end = e.original_start + e.original_length;
        let verbatim = e.generated_length == e.original_length
            && slice(generated, e.generated_start, generated_end) == slice(original, e.original_start, original_end);
        let mut features = if language_features { features_of(e.feat) } else { 0 };
        if e.generated_length == 0 {
            // zero-length spans only serve as navigation anchors; drop boundary scaffolding
            if e.feat.combine != 0 {
                continue;
            }
            features &= NAVIGATION_FEATURES;
            if features == 0 {
                continue;
            }
        }
        candidates.push(Candidate {
            generated_start: e.generated_start,
            generated_end,
            original_start: e.original_start,
            original_end,
            kind: if verbatim { VERBATIM } else { ATOM },
            features,
        });
    }

    candidates.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.generated_start.cmp(&b.generated_start))
            .then((a.generated_end - a.generated_start).cmp(&(b.generated_end - b.generated_start)))
    });

    let mut selected: Vec<&Candidate> = Vec::new();
    let mut generated_intervals = IntervalSet::new(false);
    let mut original_intervals = IntervalSet::new(true);
    for c in &candidates {
        if !generated_intervals.can_add(c.generated_start, c.generated_end) {
            continue;
        }
        if !original_intervals.can_add(c.original_start, c.original_end) {
            continue;
        }
        generated_intervals.add(c.generated_start, c.generated_end);
        original_intervals.add(c.original_start, c.original_end);
        selected.push(c);
    }
    selected.sort_by_key(|c| c.generated_start);
    selected
        .into_iter()
        .map(|c| {
            (
                c.generated_start,
                c.generated_end - c.generated_start,
                c.original_start,
                c.original_end - c.original_start,
                c.kind,
                c.features,
            )
        })
        .collect()
}

/// `text.slice(start, end)` (UTF-16 offsets).
fn slice<'t>(text: &Text<'t>, start: i64, end: i64) -> &'t str {
    let len = text.len() as i64;
    text.slice(start.clamp(0, len) as usize, end.clamp(0, len) as usize)
}

/// The reference's treap answers "does any stored interval overlap `[start, end)`"; the stored
/// intervals never overlap each other, so ordered by `(start, end)` their ends never decrease
/// and the last one starting before `end` decides.
struct IntervalSet {
    allow_exact_duplicates: bool,
    /// sorted by `(start, end)`
    set: Vec<(i64, i64)>,
    /// an inverted interval (a combined span whose original range runs backwards) voids the
    /// ordering argument; queries then scan
    inverted: bool,
}

impl IntervalSet {
    fn new(allow_exact_duplicates: bool) -> Self {
        IntervalSet { allow_exact_duplicates, set: Vec::new(), inverted: false }
    }

    fn can_add(&self, start: i64, end: i64) -> bool {
        if self.allow_exact_duplicates && self.set.binary_search(&(start, end)).is_ok() {
            return true;
        }
        // `rangesOverlap(start, end, a, b)`: `start < b && a < end`
        let before = &self.set[..self.set.partition_point(|&(a, _)| a < end)];
        if self.inverted {
            return !before.iter().any(|&(a, b)| start < b && a < end);
        }
        !before.last().is_some_and(|&(_, b)| start < b)
    }

    fn add(&mut self, start: i64, end: i64) {
        self.inverted |= end < start;
        let i = self.set.partition_point(|&x| x < (start, end));
        if self.allow_exact_duplicates && self.set.get(i) == Some(&(start, end)) {
            return;
        }
        self.set.insert(i, (start, end));
    }
}

/// `[originalStart, originalLength, virtualStart, virtualEnd, policy]`
pub type DirectiveMapping = (i64, i64, i64, i64, u8);

pub const IGNORE: u8 = 0;
pub const EXPECT: u8 = 1;

struct DirectiveState {
    policy: Policy,
    original_length: i64,
    original_start: i64,
    content: Vec<(i64, i64)>,
    anchor: Option<i64>,
    end: Option<i64>,
}

/// `toDiagnosticDirectives(mappings)`
pub fn to_diagnostic_directives(mappings: &[VolarMapping]) -> Result<Vec<DirectiveMapping>, String> {
    let mut states: IndexMap<u32, DirectiveState> = IndexMap::new();
    for m in mappings {
        let Some(marker) = m.feat.directive else { continue };
        for i in 0..m.generated.len() {
            let (virtual_offset, original_start) = (m.generated[i], m.source[i]);
            let state = states.entry(marker.id).or_insert_with(|| DirectiveState {
                policy: marker.policy,
                original_length: marker.original_length as i64,
                original_start,
                content: Vec::new(),
                anchor: None,
                end: None,
            });
            match marker.phase {
                Phase::Anchor => {
                    state.anchor = Some(virtual_offset);
                    state.original_start = original_start;
                }
                Phase::Content => {
                    let length = m.lengths[i];
                    if length > 0 {
                        state.content.push((virtual_offset, virtual_offset + length));
                    }
                }
                Phase::End => state.end = Some(virtual_offset),
            }
        }
    }

    let mut result: Vec<DirectiveMapping> = Vec::new();
    for state in states.values() {
        let Some(anchor) = state.anchor else {
            return Err("Vue diagnostic directive is missing an original anchor".into());
        };
        let content = merge_ranges(&state.content);
        if state.policy == Policy::Expect {
            let virtual_end = state.end.or(content.last().map(|r| r.1)).unwrap_or(anchor);
            result.push((state.original_start, state.original_length, anchor, virtual_end, EXPECT));
        } else {
            for &(s, e) in &content {
                result.push((state.original_start, state.original_length, s, e, IGNORE));
            }
        }
    }
    result.sort_by_key(|r| r.2);
    for w in result.windows(2) {
        if w[1].2 < w[0].3 {
            return Err(format!(
                "Vue diagnostic directive virtual ranges overlap: {}:{} and {}:{}",
                w[0].2, w[0].3, w[1].2, w[1].3
            ));
        }
    }
    Ok(result)
}

fn merge_ranges(ranges: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|r| r.0);
    let mut out: Vec<(i64, i64)> = Vec::new();
    for r in sorted {
        match out.last_mut() {
            Some(prev) if r.0 <= prev.1 => prev.1 = prev.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

/// `withSynthesizedDiagnosticIgnores(virtualLength, mappings, directives)`: generated code that no
/// span maps back is covered by ignore directives, except where a real directive already is.
pub fn with_synthesized_ignores(virtual_length: i64, mappings: &[SpanMapping], directives: &[DirectiveMapping]) -> Vec<DirectiveMapping> {
    let mut result = directives.to_vec();
    let mut blocked = directives.to_vec();
    blocked.sort_by_key(|d| d.2);
    let mut blocked_index = 0;
    let mut add_ignore_ranges = |result: &mut Vec<DirectiveMapping>, start: i64, end: i64| {
        if start >= end {
            return;
        }
        while blocked_index < blocked.len() && blocked[blocked_index].3 <= start {
            blocked_index += 1;
        }
        let mut cursor = start;
        for d in &blocked[blocked_index..] {
            if d.2 >= end {
                break;
            }
            add_ignore(result, cursor, d.2.min(end));
            cursor = cursor.max(d.3);
            if cursor >= end {
                return;
            }
        }
        add_ignore(result, cursor, end);
    };
    let mut virtual_start = 0;
    for m in mappings {
        add_ignore_ranges(&mut result, virtual_start, m.0);
        virtual_start = m.0 + m.1;
    }
    add_ignore_ranges(&mut result, virtual_start, virtual_length);
    result.sort_by_key(|d| d.2);
    result
}

fn add_ignore(result: &mut Vec<DirectiveMapping>, start: i64, end: i64) {
    if start < end {
        result.push((0, 0, start, end, IGNORE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_set() {
        let mut s = IntervalSet::new(false);
        s.add(0, 10);
        s.add(10, 10);
        assert!(!s.can_add(9, 11));
        assert!(s.can_add(10, 12));
        assert!(s.can_add(10, 10));
        assert!(!s.can_add(5, 5));
        let mut o = IntervalSet::new(true);
        o.add(3, 8);
        assert!(o.can_add(3, 8));
        assert!(!o.can_add(4, 9));
    }

    #[test]
    fn synthesized_ignores() {
        let r = with_synthesized_ignores(20, &[(5, 5, 0, 5, 0, 0)], &[(1, 1, 12, 15, IGNORE)]);
        assert_eq!(r, vec![(0, 0, 0, 5, 0), (0, 0, 10, 12, 0), (1, 1, 12, 15, 0), (0, 0, 15, 20, 0)]);
    }
}
