/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! The wire vocabulary — the identities and the operations every
//! surface addresses the engine by.
//!
//! These types were spread across the two heaviest crates: `PageId` in
//! `paged-renderer`, `ElementId` and the 117 `Mutation` ops in
//! `paged-canvas`. Nothing about them needs either. What they ARE is
//! the contract the CLI, the wasm wire, the Boa bridge, the plugin host
//! and the editor all speak, and the capability catalog could not carry
//! the op vocabulary at all because `paged-introspect` — light and
//! published — must not drag in the canvas model to read a list of
//! names.
//!
//! So they live here, in a leaf crate that depends on the model and the
//! mutation vocabulary and nothing else. `paged-renderer` and
//! `paged-canvas` re-export what they always exported, so no call site
//! moves.
//!
//! ## Why `mutations` is a feature
//!
//! The crate has two halves. The IDENTITIES — `PageId`, `ElementId`,
//! `SelectionMode`, `TextCellAddr`, `ByteBuf` — are plain newtypes over
//! strings and bytes, and every surface names things with them. The
//! OPERATIONS — `Mutation` and its roster — carry payload types from
//! `paged-mutate`, so anything that wants them links the mutation
//! engine.
//!
//! `paged-renderer` wants exactly one thing here: `PageId`. It is also
//! the crate the READ-ONLY viewer SDK is built on, and `paged-sdk` is
//! "sibling, not a shrunk app" by ratified design — no `paged-mutate`,
//! no `paged-canvas`, no `paged-script` in its tree, enforced by a
//! subset audit in the publish workflow. Merging `PageId` into this
//! crate without a feature therefore put the whole mutation engine into
//! the viewer's wasm, and the audit caught it while publishing v0.63.0.
//!
//! With `mutations` (default) the crate is what it was. Without it —
//! `default-features = false`, which is how `paged-renderer` takes it —
//! `paged-mutate` is not a dependency at all, and the viewer gets the
//! identities and nothing else.

use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

mod address;
pub use address::{address_forms, Address, AddressForm, AddressKind, GrammarError, StyleFamily};
pub mod selector;
pub use selector::Selector;
/// Selector evaluation against a document — the `query` feature, so the
/// read-only viewer (which takes this crate with no default features)
/// does not pick up a scene dependency it never asked for.
#[cfg(feature = "query")]
pub mod query;

#[derive(
    Debug,
    Default,
    Clone,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    tsify_next::Tsify,
)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
pub struct PageId(pub String);

impl PageId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn synthetic(spread_idx: usize, local_idx: usize) -> Self {
        Self(format!("page-{spread_idx}-{local_idx}"))
    }
}

impl std::fmt::Display for PageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Element address the user can select OR a `SetElementProperty`
/// mutation can target. The first six variants are page items
/// (selection state holds these); `StoryRange` is the half-open
/// character range that character / paragraph property writes
/// address. Selection state today never holds `StoryRange` (the
/// text-side caret + range live in `ContentSelection`); the
/// variant exists so the apply layer can be reached via the
/// existing `Mutation::SetElementProperty` wire shape — see
/// the editor's client-SDK implementation plan §3c.1 (not published).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum ElementId {
    TextFrame(String),
    Rectangle(String),
    Oval(String),
    Polygon(String),
    GraphicLine(String),
    Group(String),
    /// SDK Phase 3 — addresses a half-open `[start, end)` character
    /// range inside a story for character / paragraph property
    /// writes. Maps to `paged_mutate::NodeId::StoryRange { ... }`.
    StoryRange {
        story_id: String,
        start: u32,
        end: u32,
    },
    /// W3.A1 — a `<Table>` addressed by `(story_id, table_id)`. Maps to
    /// `paged_mutate::NodeId::Table`. Backs `AppliedTableStyle` writes
    /// and the table-structure Mutations.
    Table {
        story_id: String,
        table_id: String,
    },
    /// W3.A1 — a table cell addressed by `(story_id, table_id, row,
    /// col)`. Maps to `paged_mutate::NodeId::TableCell`. Backs the
    /// cell-scoped `SetElementProperty` writes (fill / insets / vertical
    /// justify / applied cell style).
    TableCell {
        story_id: String,
        table_id: String,
        row: u32,
        col: u32,
    },
}

impl ElementId {
    /// The bare `Self` id, regardless of kind. Useful when matching
    /// against parser/scene structures that only expose the string.
    /// For `StoryRange` this returns the story id; callers needing
    /// the range bounds should match on the variant.
    pub fn raw_id(&self) -> &str {
        match self {
            ElementId::TextFrame(id)
            | ElementId::Rectangle(id)
            | ElementId::Oval(id)
            | ElementId::Polygon(id)
            | ElementId::GraphicLine(id)
            | ElementId::Group(id) => id,
            ElementId::StoryRange { story_id, .. } => story_id,
            // The story is the container; table / cell ids ride the
            // variant. `raw_id` returns the story for table addresses
            // (callers needing the table / cell coords match on the
            // variant) — consistent with `StoryRange`.
            ElementId::Table { story_id, .. } | ElementId::TableCell { story_id, .. } => story_id,
        }
    }

    /// Parse an address (ADR 131, see [`Address`]) and keep it when it
    /// names an `ElementId` without needing a document: the six page-item
    /// forms (`textFrame:<id>`, `rectangle:<id>` — alias `rect:`, `oval:`,
    /// `polygon:`, `graphicLine:`, `group:`; also master-scoped) and
    /// `storyRange:<storyId>@<start>..<end>` (alias `story:…@…`).
    ///
    /// The grammar itself lives in [`Address::parse`], so this, the Boa
    /// bridge, the CLI and the catalog cannot disagree about it. A table
    /// or cell address (`table:<id>`, `cell:<id>/<r>,<c>`) needs the
    /// document to find its story and is resolved by the query module,
    /// not here; `frame:<id>` likewise needs the document for its kind.
    ///
    /// `None` on anything else — this is user input, so a bad address
    /// is an answer, never a panic.
    pub fn parse(s: &str) -> Option<Self> {
        Address::parse(s).ok()?.to_element_id()
    }

    /// The canonical address string ([`Address`]'s `Display`). Always
    /// `Some` since ADR 131 gave table and cell a form; the `Option` is
    /// kept so no call site moves. A table or cell address drops the
    /// story (the table `Self` is unique per document), so it parses back
    /// through a document's resolver, not through [`ElementId::parse`].
    pub fn to_address(&self) -> Option<String> {
        Some(Address::from(self.clone()).to_string())
    }

    /// Short human-readable kind label used by the Inspector panel
    /// and scene tree. Matches the IDML element name conventionally
    /// shown to designers.
    pub fn kind_label(&self) -> &'static str {
        match self {
            ElementId::TextFrame(_) => "TextFrame",
            ElementId::Rectangle(_) => "Rectangle",
            ElementId::Oval(_) => "Oval",
            ElementId::Polygon(_) => "Polygon",
            ElementId::GraphicLine(_) => "GraphicLine",
            ElementId::Group(_) => "Group",
            ElementId::StoryRange { .. } => "StoryRange",
            ElementId::Table { .. } => "Table",
            ElementId::TableCell { .. } => "TableCell",
        }
    }
}

/// How a `SetElementSelection` request combines with the current set.
/// Mirrors the canonical macOS / industry convention:
/// - `Replace` — plain click; selection becomes the request.
/// - `Add` — Shift-click; union (already-selected ids stay).
/// - `Toggle` — Cmd/Ctrl-click; ids already in the set are removed,
///   ids not in the set are added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum SelectionMode {
    Replace,
    Add,
    Toggle,
}

/// The application-state set of selected elements. Order is preserved
/// in selection order so the UI can render "primary" selection chrome
/// (e.g., the last-selected item) differently if it wants.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementSelection {
    pub ids: Vec<ElementId>,
}

impl ElementSelection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn contains(&self, id: &ElementId) -> bool {
        self.ids.iter().any(|i| i == id)
    }

    pub fn clear(&mut self) {
        self.ids.clear();
    }

    /// Replace the entire selection with `ids`. Duplicates in the
    /// input are kept in first-occurrence order.
    pub fn set(&mut self, ids: Vec<ElementId>) {
        let mut out: Vec<ElementId> = Vec::with_capacity(ids.len());
        for id in ids {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        self.ids = out;
    }

    /// Add `id` if not already present.
    pub fn add(&mut self, id: ElementId) {
        if !self.contains(&id) {
            self.ids.push(id);
        }
    }

    /// Add `id` if absent, remove it if present.
    pub fn toggle(&mut self, id: ElementId) {
        if let Some(pos) = self.ids.iter().position(|i| i == &id) {
            self.ids.remove(pos);
        } else {
            self.ids.push(id);
        }
    }

    /// Remove `id` if present; no-op otherwise.
    pub fn remove(&mut self, id: &ElementId) {
        if let Some(pos) = self.ids.iter().position(|i| i == id) {
            self.ids.remove(pos);
        }
    }

    /// Apply a `SelectionMode` against the current set with the given
    /// request ids.
    pub fn apply_mode(&mut self, ids: &[ElementId], mode: SelectionMode) {
        match mode {
            SelectionMode::Replace => self.set(ids.to_vec()),
            SelectionMode::Add => {
                for id in ids {
                    self.add(id.clone());
                }
            }
            SelectionMode::Toggle => {
                for id in ids {
                    self.toggle(id.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "mutations")]
    #[test]
    fn a_set_target_travels_as_an_address_string_and_the_v70_object_still_parses() {
        let m = Mutation::Set {
            address: Address::Swatch {
                id: "Color/Red".into(),
            },
            path: "tint".into(),
            value: paged_mutate::Value::Number(50.0),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""address":"swatch:Color/Red""#), "{json}");
        let back: Mutation = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            Mutation::Set {
                address: Address::Swatch { .. },
                ..
            }
        ));
        // deprecated: the protocol-70 target object
        let legacy: Mutation = serde_json::from_str(
            r#"{"op":"set","args":{"address":{"kind":"cell","storyId":"Story/s","tableId":"ut","row":1,"col":2},"path":"rowSpan","value":{"type":"number","value":2}}}"#,
        )
        .unwrap();
        match legacy {
            Mutation::Set { address, .. } => assert_eq!(address.to_string(), "cell:ut/1,2"),
            other => panic!("{other:?}"),
        }
        // a selector is not a target
        assert!(serde_json::from_str::<Mutation>(
            r#"{"op":"set","args":{"address":"frame[name=x]","path":"name","value":{"type":"text","value":"y"}}}"#
        )
        .is_err());
    }

    #[test]
    fn every_virtual_target_maps_to_an_address_and_back() {
        let targets = vec![
            VirtualTarget::Doc,
            VirtualTarget::Page {
                page_id: "p".into(),
            },
            VirtualTarget::Layer {
                layer_id: "l".into(),
            },
            VirtualTarget::Master {
                master_id: "m".into(),
            },
            VirtualTarget::Story {
                story_id: "Story/s".into(),
            },
            VirtualTarget::Field {
                story_id: "Story/s".into(),
                offset: 3,
            },
            VirtualTarget::Swatch {
                swatch_id: "Color/Red".into(),
            },
            VirtualTarget::Gradient {
                gradient_id: "Gradient/g".into(),
            },
            VirtualTarget::ColorGroup {
                group_id: "ColorGroup/c".into(),
            },
            VirtualTarget::NumberingList {
                list_id: "NumberingList/n".into(),
            },
            VirtualTarget::Style {
                family: StyleFamily::Object,
                style_id: "ObjectStyle/o".into(),
            },
            VirtualTarget::Section {
                section_id: "s".into(),
            },
            VirtualTarget::Guide {
                guide_id: "Guide/Spread/u1/0".into(),
            },
            VirtualTarget::Condition {
                condition_id: "Condition/c".into(),
            },
            VirtualTarget::TextVariable {
                variable_id: "dTextVariablenX".into(),
            },
            VirtualTarget::Spread {
                spread_id: "sp".into(),
            },
            VirtualTarget::ConditionSet {
                set_id: "cs".into(),
            },
            VirtualTarget::Hyperlink {
                hyperlink_id: "h".into(),
            },
            VirtualTarget::Element {
                element_id: ElementId::Oval("o".into()),
            },
        ];
        for t in targets {
            let a = Address::from(t.clone());
            let back = VirtualTarget::from_address_direct(&a).unwrap();
            if matches!(t, VirtualTarget::Style { .. }) {
                // a display name may stand for the Self: the document decides
                assert_eq!(back, None, "{a}");
            } else {
                assert_eq!(back.as_ref(), Some(&t), "{a}");
            }
        }
    }

    use super::*;

    fn tf(id: &str) -> ElementId {
        ElementId::TextFrame(id.to_string())
    }
    fn rect(id: &str) -> ElementId {
        ElementId::Rectangle(id.to_string())
    }

    /// Every canonical address round-trips, so a surface can hand an id
    /// out and take it back.
    #[test]
    fn every_addressable_variant_round_trips() {
        let cases = [
            ElementId::TextFrame("u1".into()),
            ElementId::Rectangle("u2".into()),
            ElementId::Oval("u3".into()),
            ElementId::Polygon("u4".into()),
            ElementId::GraphicLine("u5".into()),
            ElementId::Group("u6".into()),
            ElementId::StoryRange {
                story_id: "Story/u1".into(),
                start: 0,
                end: 6,
            },
        ];
        for id in cases {
            let address = id.to_address().expect("addressable");
            assert_eq!(
                ElementId::parse(&address),
                Some(id.clone()),
                "{address} did not parse back"
            );
        }
    }

    /// Table and cell have a textual form now (ADR 131): `table:<id>`
    /// and `cell:<tableId>/<row>,<col>`. Their story is the document's to
    /// find, so `ElementId::parse` (which has no document) does not
    /// produce them.
    #[test]
    fn table_addresses_have_a_textual_form() {
        let table = ElementId::Table {
            story_id: "Story/u1".into(),
            table_id: "t1".into(),
        };
        assert_eq!(table.to_address().as_deref(), Some("table:t1"));
        let cell = ElementId::TableCell {
            story_id: "Story/u1".into(),
            table_id: "t1".into(),
            row: 0,
            col: 3,
        };
        assert_eq!(cell.to_address().as_deref(), Some("cell:t1/0,3"));
        assert_eq!(ElementId::parse("cell:t1/0,3"), None);
    }

    /// `story:<id>@a..b` (the plan's notation) is an alias of the
    /// canonical `storyRange:` form.
    #[test]
    fn story_alias_parses_to_the_story_range() {
        assert_eq!(
            ElementId::parse("story:Story/u1@0..6"),
            ElementId::parse("storyRange:Story/u1@0..6")
        );
    }

    /// Addresses that name something `ElementId` has no variant for are
    /// not element ids.
    #[test]
    fn non_element_addresses_are_not_element_ids() {
        for s in [
            "doc",
            "page:u1",
            "layer:ub3",
            "style:paragraph/Body",
            "frame:u1",
        ] {
            assert_eq!(ElementId::parse(s), None, "{s}");
        }
    }

    /// The aliases parse and are never emitted.
    #[test]
    fn aliases_parse_to_the_canonical_variant() {
        assert_eq!(
            ElementId::parse("textframe:u1"),
            Some(ElementId::TextFrame("u1".into()))
        );
        assert_eq!(
            ElementId::parse("rect:u1"),
            Some(ElementId::Rectangle("u1".into()))
        );
        assert_eq!(
            ElementId::parse("graphicline:u1"),
            Some(ElementId::GraphicLine("u1".into()))
        );
        assert_eq!(
            ElementId::parse("storyrange:Story/u1@0..6"),
            ElementId::parse("storyRange:Story/u1@0..6")
        );
    }

    /// A zero-length range is a caret — the paragraph it stands in.
    #[test]
    fn a_caret_parses() {
        assert!(ElementId::parse("storyRange:Story/u1@0..0").is_some());
        assert!(ElementId::parse("storyRange:Story/u1@4..4").is_some());
    }

    /// Bad input is an answer, not a panic — this grammar reads command
    /// lines and script arguments.
    #[test]
    fn malformed_addresses_are_none_not_panics() {
        for bad in [
            "",
            "textFrame",
            "textFrame:",
            ":u1",
            "sprocket:u1",
            "storyRange:@0..6",
            "storyRange:Story/u1@6..0",
            "storyRange:Story/u1@x..6",
            "storyRange:Story/u1",
        ] {
            assert_eq!(ElementId::parse(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn empty_by_default() {
        let s = ElementSelection::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn add_dedupes() {
        let mut s = ElementSelection::new();
        s.add(tf("a"));
        s.add(tf("a"));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn toggle_round_trips() {
        let mut s = ElementSelection::new();
        s.toggle(tf("a"));
        assert!(s.contains(&tf("a")));
        s.toggle(tf("a"));
        assert!(!s.contains(&tf("a")));
    }

    #[test]
    fn replace_resets() {
        let mut s = ElementSelection::new();
        s.add(tf("a"));
        s.add(tf("b"));
        s.apply_mode(&[rect("c")], SelectionMode::Replace);
        assert_eq!(s.ids, vec![rect("c")]);
    }

    #[test]
    fn add_mode_unions() {
        let mut s = ElementSelection::new();
        s.add(tf("a"));
        s.apply_mode(&[tf("b"), tf("a")], SelectionMode::Add);
        assert_eq!(s.ids, vec![tf("a"), tf("b")]);
    }

    #[test]
    fn toggle_mode_xor() {
        let mut s = ElementSelection::new();
        s.add(tf("a"));
        s.apply_mode(&[tf("a"), tf("b")], SelectionMode::Toggle);
        // a was present → removed; b was absent → added
        assert_eq!(s.ids, vec![tf("b")]);
    }

    #[test]
    fn set_dedupes_input() {
        let mut s = ElementSelection::new();
        s.set(vec![tf("a"), tf("a"), tf("b")]);
        assert_eq!(s.ids, vec![tf("a"), tf("b")]);
    }

    #[test]
    fn raw_id_unwraps_variant() {
        assert_eq!(tf("uXYZ").raw_id(), "uXYZ");
        assert_eq!(rect("uABC").raw_id(), "uABC");
        assert_eq!(ElementId::Group("g1".to_string()).raw_id(), "g1");
    }

    #[test]
    fn variants_with_same_id_are_distinct() {
        let mut s = ElementSelection::new();
        s.add(tf("u1"));
        s.add(rect("u1"));
        assert_eq!(s.len(), 2);
        assert!(s.contains(&tf("u1")));
        assert!(s.contains(&rect("u1")));
    }
}

/// W1.13 — cell qualifier for a content address that points INTO a
/// table cell rather than the story's main paragraph flow.
///
/// ## The two-stream addressing model
///
/// Table-cell text is stored out of band on `Table.cells[].paragraphs`
/// (see `idml_import`), disjoint from `Story.paragraphs`. So a content
/// address needs to say *which* paragraph stream its byte offsets index:
///
/// - `ContentSelection.cell == None` — offsets are story-local bytes
///   over `story.paragraphs` (the body flow). Unchanged from before.
/// - `ContentSelection.cell == Some(addr)` — offsets are CELL-LOCAL
///   bytes over `cell.paragraphs`, under the same story-offset contract
///   (run bytes + one synthetic `\n` per inter-paragraph boundary,
///   counted within the cell). The owning story is still `story_id`;
///   `addr` picks the cell within that story's table.
///
/// `table_id` / `row` / `col` are the SAME identifiers the hit-test
/// surface emits (`HitResult.table_context` / `TableHitContext`) and
/// that the renderer stamps onto cell `LineLayout`s
/// (`paged_renderer::CellAddr`), so a hit that lands in a cell hands
/// back exactly the qualifier the caret/edit address needs — no second
/// query.
///
/// ## Why a qualifier and not a re-numbered flat offset
///
/// The alternative — fold cells into one flat story-offset space via a
/// reserved high-bit/region scheme — was rejected: it makes
/// `shift_for_insert`/`shift_for_delete`, undo inverse offsets, and the
/// existing body-only consumers (BreakRecord, the A/B harness, every
/// `RequestWordBounds`/`RequestLineBounds` caller) all have to learn the
/// encoding, and a single arithmetic slip silently routes an edit into
/// the wrong cell. The qualifier keeps body addressing byte-identical
/// (the field defaults to `None` and is `#[serde(default)]`, so it
/// rides v35 additively — old senders omit it) and makes "which stream"
/// an explicit, type-checked decision. Undo is trivially correct
/// because the inverse op carries the same `cell` qualifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct TextCellAddr {
    /// `<Table Self="...">` id within `story_id`.
    pub table_id: String,
    /// Template row (0-based); span-origin row for spanned cells.
    pub row: u32,
    /// Column (0-based); span-origin column for spanned cells.
    pub col: u32,
}

/// ADR 132 — the RESOLVED target of a `Mutation::Set` / a property
/// read: every object kind a property can live on, named by the ids the
/// dedicated operations take.
///
/// The wire names a target by [`Address`] (ADR 131); the engine resolves
/// that against the document into this enum ([`query::Index::to_virtual_target`],
/// total over every core address kind) — `frame:` finds its kind,
/// `page:#n` its page id, a style's display name its `Self`, and a
/// `table:`/`cell:` the story that owns the table. `Element` carries the
/// six page-item kinds plus `storyRange:` through the existing `ElementId`.
///
/// This enum's JSON (`{ "kind": "layer", "layerId": "ua" }`) was the
/// `Set` target at protocol 70 before addresses landed; it is still
/// accepted in that position as a DEPRECATED alias (see
/// [`From<VirtualTarget> for Address`](Address)). The id fields are spelled
/// `…Id` so batch handle substitution (`$h:`) reaches them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum VirtualTarget {
    /// The document: defaults, colour management, setup, labels.
    Doc,
    Page {
        page_id: String,
    },
    Layer {
        layer_id: String,
    },
    Master {
        master_id: String,
    },
    Story {
        story_id: String,
    },
    /// A text field anchored at a story offset.
    Field {
        story_id: String,
        offset: u32,
    },
    Swatch {
        swatch_id: String,
    },
    Gradient {
        gradient_id: String,
    },
    ColorGroup {
        group_id: String,
    },
    NumberingList {
        list_id: String,
    },
    Style {
        family: StyleFamily,
        style_id: String,
    },
    Section {
        section_id: String,
    },
    Guide {
        guide_id: String,
    },
    Condition {
        condition_id: String,
    },
    TextVariable {
        variable_id: String,
    },
    Spread {
        spread_id: String,
    },
    ConditionSet {
        set_id: String,
    },
    Hyperlink {
        hyperlink_id: String,
    },
    Table {
        story_id: String,
        table_id: String,
    },
    Cell {
        story_id: String,
        table_id: String,
        row: u32,
        col: u32,
    },
    /// A page item or a story range — every `SetElementProperty` path,
    /// plus the frame-level virtual paths (`imageUri`, `labels[key]`,
    /// `groupTransform`).
    Element {
        element_id: ElementId,
    },
}

impl VirtualTarget {
    /// The ADR 131 address kind this target is (`"doc"`, `"page"`, …).
    /// Element targets answer `"frame"`, `"group"` or `"storyRange"`.
    pub fn address_kind(&self) -> &'static str {
        match self {
            VirtualTarget::Doc => "doc",
            VirtualTarget::Page { .. } => "page",
            VirtualTarget::Layer { .. } => "layer",
            VirtualTarget::Master { .. } => "master",
            VirtualTarget::Story { .. } => "story",
            VirtualTarget::Field { .. } => "field",
            VirtualTarget::Swatch { .. } => "swatch",
            VirtualTarget::Gradient { .. } => "gradient",
            VirtualTarget::ColorGroup { .. } => "colorGroup",
            VirtualTarget::NumberingList { .. } => "numberingList",
            VirtualTarget::Style { .. } => "style",
            VirtualTarget::Section { .. } => "section",
            VirtualTarget::Guide { .. } => "guide",
            VirtualTarget::Condition { .. } => "condition",
            VirtualTarget::TextVariable { .. } => "textVariable",
            VirtualTarget::Spread { .. } => "spread",
            VirtualTarget::ConditionSet { .. } => "conditionSet",
            VirtualTarget::Hyperlink { .. } => "hyperlink",
            VirtualTarget::Table { .. } => "table",
            VirtualTarget::Cell { .. } => "cell",
            VirtualTarget::Element { element_id } => match element_id {
                ElementId::Group(_) => "group",
                ElementId::StoryRange { .. } => "storyRange",
                ElementId::Table { .. } => "table",
                ElementId::TableCell { .. } => "cell",
                _ => "frame",
            },
        }
    }
}

/// Total: every resolved target has exactly one canonical address. This
/// is what keeps the protocol-70 JSON target form working as an alias of
/// the address string (and it is the inverse of the document resolver
/// for every target that resolver produces).
impl From<VirtualTarget> for Address {
    fn from(t: VirtualTarget) -> Self {
        match t {
            VirtualTarget::Doc => Address::Doc,
            VirtualTarget::Page { page_id } => Address::Page { id: page_id },
            VirtualTarget::Layer { layer_id } => Address::Layer { id: layer_id },
            VirtualTarget::Master { master_id } => Address::Master { id: master_id },
            VirtualTarget::Story { story_id } => Address::Story { id: story_id },
            VirtualTarget::Field { story_id, offset } => Address::Field { story_id, offset },
            VirtualTarget::Swatch { swatch_id } => Address::Swatch { id: swatch_id },
            VirtualTarget::Gradient { gradient_id } => Address::Gradient { id: gradient_id },
            VirtualTarget::ColorGroup { group_id } => Address::ColorGroup { id: group_id },
            VirtualTarget::NumberingList { list_id } => Address::NumberingList { id: list_id },
            VirtualTarget::Style { family, style_id } => Address::Style {
                family,
                id: style_id,
            },
            VirtualTarget::Section { section_id } => Address::Section { id: section_id },
            VirtualTarget::Guide { guide_id } => Address::Guide { id: guide_id },
            VirtualTarget::Condition { condition_id } => Address::Condition { id: condition_id },
            VirtualTarget::TextVariable { variable_id } => {
                Address::TextVariable { id: variable_id }
            }
            VirtualTarget::Spread { spread_id } => Address::Spread { id: spread_id },
            VirtualTarget::ConditionSet { set_id } => Address::ConditionSet { id: set_id },
            VirtualTarget::Hyperlink { hyperlink_id } => Address::Hyperlink { id: hyperlink_id },
            VirtualTarget::Table { table_id, .. } => Address::Table { id: table_id },
            VirtualTarget::Cell {
                table_id, row, col, ..
            } => Address::Cell { table_id, row, col },
            VirtualTarget::Element { element_id } => Address::from(element_id),
        }
    }
}

impl VirtualTarget {
    /// The target an address names WITHOUT consulting a document, for
    /// every kind whose address already carries the ids the operations
    /// take. `Ok(None)` for the kinds that need the document to resolve —
    /// `frame:` (its kind), `page:#n` (its id), `style:` (a display name
    /// may stand for the `Self`), `table:` / `cell:` (the owning story);
    /// [`query::Index::to_virtual_target`] handles those. `Err` for the
    /// two kinds that are never a core target: a plugin object (routed by
    /// the host registry) and an unresolved batch handle.
    pub fn from_address_direct(a: &Address) -> Result<Option<VirtualTarget>, String> {
        Ok(Some(match a {
            Address::Doc => VirtualTarget::Doc,
            Address::TextFrame { .. }
            | Address::Rectangle { .. }
            | Address::Oval { .. }
            | Address::Polygon { .. }
            | Address::GraphicLine { .. }
            | Address::Group { .. }
            | Address::StoryRange { .. } => VirtualTarget::Element {
                element_id: a.to_element_id().expect("a page item or a story range"),
            },
            Address::OnMaster { item, .. } => match item.to_element_id() {
                Some(element_id) => VirtualTarget::Element { element_id },
                None => return Ok(None),
            },
            Address::Page { id } => VirtualTarget::Page {
                page_id: id.clone(),
            },
            Address::Spread { id } => VirtualTarget::Spread {
                spread_id: id.clone(),
            },
            Address::Layer { id } => VirtualTarget::Layer {
                layer_id: id.clone(),
            },
            Address::Master { id } => VirtualTarget::Master {
                master_id: id.clone(),
            },
            Address::Story { id } => VirtualTarget::Story {
                story_id: id.clone(),
            },
            Address::Field { story_id, offset } => VirtualTarget::Field {
                story_id: story_id.clone(),
                offset: *offset,
            },
            Address::Swatch { id } => VirtualTarget::Swatch {
                swatch_id: id.clone(),
            },
            Address::Gradient { id } => VirtualTarget::Gradient {
                gradient_id: id.clone(),
            },
            Address::ColorGroup { id } => VirtualTarget::ColorGroup {
                group_id: id.clone(),
            },
            Address::NumberingList { id } => VirtualTarget::NumberingList {
                list_id: id.clone(),
            },
            Address::Guide { id } => VirtualTarget::Guide {
                guide_id: id.clone(),
            },
            Address::Condition { id } => VirtualTarget::Condition {
                condition_id: id.clone(),
            },
            Address::ConditionSet { id } => VirtualTarget::ConditionSet { set_id: id.clone() },
            Address::Section { id } => VirtualTarget::Section {
                section_id: id.clone(),
            },
            Address::Hyperlink { id } => VirtualTarget::Hyperlink {
                hyperlink_id: id.clone(),
            },
            Address::TextVariable { id } => VirtualTarget::TextVariable {
                variable_id: id.clone(),
            },
            Address::Frame { .. }
            | Address::PageNumber { .. }
            | Address::Style { .. }
            | Address::Table { .. }
            | Address::Cell { .. } => return Ok(None),
            Address::Plugin { plugin_id, .. } => {
                return Err(format!(
                    "{a} is a {plugin_id} object, not a core one — plugin objects are set \
                     through the host's object registry, not the engine"
                ))
            }
            Address::Handle { name } => {
                return Err(format!(
                    "$h:{name} is a batch handle with nothing bound — a handle resolves only \
                     inside the batch that bound it"
                ))
            }
        }))
    }
}

/// The `Set` target on the wire (ADR 131/132): the canonical address
/// STRING (`"swatch:Color/Red"`, `"style:object/ObjectStyle/x"`, `"doc"`).
/// Deserialisation also takes the protocol-70 `VirtualTarget` object
/// (`{ "kind": "layer", "layerId": "ua" }`) as a deprecated alias.
pub mod address_wire {
    use super::{Address, VirtualTarget};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(a: &Address, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(a)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Address, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            Text(String),
            Legacy(VirtualTarget),
        }
        match Form::deserialize(d)? {
            Form::Text(s) => Address::parse(&s).map_err(serde::de::Error::custom),
            Form::Legacy(t) => Ok(t.into()),
        }
    }
}

/// A byte buffer that crosses the message channel. Wraps `Vec<u8>`
/// so transferable-via-`postMessage` semantics are explicit at call
/// sites; the wasm crate decides whether to clone or transfer based
/// on whether the value is the JS-side `Uint8Array` or a Rust-side
/// `Vec`. The wire form is whatever serde produces for `Vec<u8>` —
/// JSON renders an array of numbers; future binary protocols (CBOR
/// / messagepack) render a real bytes blob without code change.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(transparent)]
pub struct ByteBuf(pub Vec<u8>);

impl ByteBuf {
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for ByteBuf {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

/// A content-space mutation. Phase 1 carries the *envelope* only —
/// the worker rejects each variant with `WorkerError::NotImplemented`.
/// Phase 3 lights these up incrementally.
///
/// Behind the default `mutations` feature: its payload types come from
/// `paged-mutate`, and the read-only viewer SDK must not link that.
/// See the crate docs.
#[cfg(feature = "mutations")]
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "op",
    content = "args"
)]
pub enum Mutation {
    InsertText {
        story_id: String,
        offset: u32,
        text: String,
        /// W1.13 — cell qualifier (rides v35, additive). `None` /
        /// absent ⇒ `offset` is a story-local body offset; `Some` ⇒
        /// cell-local offset into the named table cell. Mirrors
        /// `ContentSelection.cell` / `TextOp::cell`.
        #[serde(default)]
        cell: Option<TextCellAddr>,
    },
    DeleteRange {
        story_id: String,
        start: u32,
        end: u32,
        /// W1.13 — cell qualifier (see `InsertText::cell`).
        #[serde(default)]
        cell: Option<TextCellAddr>,
    },
    /// W0.5 — apply a named paragraph/character style to a story
    /// range. `scope` picks the level; `style` is the style ref
    /// (`ParagraphStyle/<id>` or `CharacterStyle/<id>`). Routes to
    /// `Operation::ApplyStyle`.
    ApplyStyle {
        story_id: String,
        start: u32,
        end: u32,
        style: String,
        scope: paged_mutate::operation::StyleScope,
        /// v55 — cell qualifier (additive). `None` / absent ⇒ `[start, end)` is a
        /// story-local BODY range; `Some` ⇒ a cell-local range into the named
        /// table cell's own paragraphs. Mirrors `InsertText.cell`, and closes the
        /// gap where cell text could only carry default formatting.
        #[serde(default)]
        cell: Option<TextCellAddr>,
        /// v65 — paragraph address (additive). `Some(i)` with paragraph scope
        /// styles exactly paragraph `i` (0-based, in the body or in `cell`)
        /// and ignores `start`/`end`: the only way to name one EMPTY
        /// paragraph among several in a row.
        #[serde(default)]
        paragraph: Option<u32>,
    },
    /// W0.5 — insert a field marker (page-number etc.) at a story
    /// offset. Routes to `Operation::InsertField`. v43 (D-01): `field`
    /// additionally accepts the plugin `placeholder` kind
    /// (`{ placeholder: { plugin, key, value? } }`) — a tagged,
    /// edit-surviving anchor run displaying its cached value (or the
    /// `<key>` token while unresolved).
    InsertField {
        story_id: String,
        offset: u32,
        field: paged_mutate::operation::FieldKind,
        /// v69 — the insertion point in the CARET unit instead: UTF-8 bytes
        /// of the runs plus one synthetic `\n` per paragraph boundary (the
        /// `ContentSelection` / `insertText` / `host.text.caret()` unit).
        /// `offset` and every other field operation count characters with no
        /// paragraph separator; the two agree only inside the first
        /// paragraph of ASCII text. When present the engine converts this
        /// against the story at apply time and ignores `offset` (send 0).
        /// An older engine ignores the field, so gate it on `protocol >= 68`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_offset: Option<u32>,
    },
    /// v52 — insert an image-bearing anchored Rectangle into a story, anchored
    /// at the paragraph containing character `offset`, sized `width`×`height`
    /// pt, with `image_uri` as its image link. The model mints the frame's
    /// self id and surfaces it as the outcome's `createdId`. Backs an inline
    /// image placed in the text flow (paged.doc); the existing anchored-frame
    /// render path (`paged-renderer` `anchored.rs`) draws it inline.
    InsertAnchoredFrame {
        story_id: String,
        offset: u32,
        width: f32,
        height: f32,
        #[serde(default)]
        image_uri: Option<String>,
        /// v65 — paragraph address (additive). `Some(i)` anchors the frame
        /// in paragraph `i` (0-based) of the story; `offset` stays the story
        /// offset (so an engine without this field reads the same message)
        /// and is clamped into that paragraph. Names an empty paragraph.
        #[serde(default)]
        paragraph: Option<u32>,
    },
    /// v53 — the hyperlink CREATE door: make the story range `[start, end)`
    /// (contiguous char offsets, the `ApplyStyle` address space) a native
    /// clickable link to `url`. The model mints the three cross-referencing
    /// ids a link needs (a `HyperlinkTextSource` tag, a `Hyperlink`, and a
    /// `HyperlinkURLDestination`) and registers them, so the renderer's
    /// existing link resolution makes the span clickable. Undoable in one
    /// step (inverse drops the tag + the two designmap resources). Backs
    /// paged.doc's `w:hyperlink` runs. v70: with `page`, the link goes to
    /// that page instead (`url` is then ignored and may be empty): a
    /// slide's jump to another slide.
    InsertHyperlink {
        story_id: String,
        start: u32,
        end: u32,
        url: String,
        #[serde(default)]
        page: Option<PageId>,
    },
    /// v0.71.13 — insert a footnote (ADR 163): its reference at story char
    /// `offset` (contiguous char offsets, the `InsertHyperlink` address
    /// space) and the note, whose text is its number, the document's
    /// separator, then `text`. The model mints the note's id and reports
    /// it in the reply. Undoable in one step.
    InsertFootnote {
        story_id: String,
        offset: u32,
        /// The paragraph `offset` stands in (the end of a paragraph and the
        /// start of the next share an offset); `offset` is clamped into it.
        #[serde(default)]
        paragraph: Option<u32>,
        #[serde(default)]
        text: String,
    },
    /// v0.71.13 — delete a footnote: its reference and the note, by the note's
    /// id (`Footnote.self_id`). Undoable in one step: undo puts the note
    /// back as it was, its text and formatting included.
    DeleteFootnote {
        story_id: String,
        footnote_id: String,
    },
    /// v0.71.13 — delete a hyperlink by its `Hyperlink` `Self` id (ADR 164),
    /// from any origin: its text stops linking, the link goes, and so does
    /// its URL or page destination when nothing else points at it. A text
    /// destination stays (it is an anchor in its story). Undoable in one
    /// step.
    DeleteHyperlink {
        hyperlink_id: String,
    },
    /// v0.71.13 — edit a hyperlink (ADR 164): `name` renames it (empty clears
    /// it); `page` (wins) or `url` sends it somewhere new. Its own
    /// destination of that kind changes in place; a shared one, a text
    /// anchor or one of the other kind stays, and the link moves to a new
    /// destination the model mints. Fields left out are unchanged.
    /// Undoable in one step.
    EditHyperlink {
        hyperlink_id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        page: Option<PageId>,
    },
    /// v0.71.13 — Update Table of Contents (ADR 165): rewrite TOC story
    /// `story_id` (one whose story or frame names a TOC style) from its
    /// style over the current layout, as InDesign writes it: the title,
    /// then one paragraph per heading with its page number. Until then the
    /// story prints the text it holds. Undoable in one step.
    UpdateTableOfContents {
        story_id: String,
    },
    /// v72 — Generate Index (ADR 168): write the document's index into
    /// story `story_id` (its text replaced, as InDesign's Replace Existing
    /// Index does) from its topics and page references over the current
    /// layout, as InDesign writes one: the title, a heading per letter,
    /// each topic with its pages and cross-references. Index styles the
    /// document lacks are created, as InDesign creates them. Undoable in
    /// one step.
    GenerateIndex {
        story_id: String,
    },
    /// Update Cross-References (ADR 169): rewrite cross-reference source
    /// `source_id`'s text (every source's when `None`, as Update All
    /// Cross-References does) from its format and its destination
    /// paragraph, as InDesign writes it. The page number stays the live
    /// `XRefPageNumber` it always prints. Undoable in one step.
    UpdateCrossReferences {
        #[serde(default)]
        source_id: Option<String>,
    },
    /// Insert an editorial note (ADR 172): its place at story char
    /// `offset` (the `InsertFootnote` address space) — one character of the
    /// story, as InDesign keeps one — and its `text`, paragraphs separated
    /// by U+000D. `user` and `date` are what InDesign writes as `UserName`
    /// and `CreationDate` (`2026-10-16T09:59:49`); the engine reads no
    /// clock, so the host passes them. The model mints the note's id and
    /// reports it in the reply. Undoable in one step.
    InsertNote {
        story_id: String,
        offset: u32,
        /// The paragraph `offset` stands in; `offset` is clamped into it.
        #[serde(default)]
        paragraph: Option<u32>,
        #[serde(default)]
        text: String,
        #[serde(default)]
        user: Option<String>,
        #[serde(default)]
        date: Option<String>,
    },
    /// Rewrite note `note_id`'s text (ADR 172); `date` becomes its
    /// `ModificationDate`. Undoable in one step.
    EditNote {
        note_id: String,
        text: String,
        #[serde(default)]
        date: Option<String>,
    },
    /// Delete note `note_id` and its place in the text (ADR 172).
    /// Undoable in one step: undo puts it back as it was.
    DeleteNote {
        note_id: String,
    },
    /// Convert to Text (ADR 172): note `note_id`'s place becomes a space
    /// and the note's text, its paragraphs the story's, as InDesign 20.0.1
    /// converts one. Undoable in one step.
    ConvertNoteToText {
        note_id: String,
    },
    /// New Bookmark (ADR 173): bookmark `name` to page `page`, through a
    /// page destination of its own as InDesign makes one, or else to the
    /// existing destination `destination` (a text anchor, a page
    /// destination); under bookmark `parent` (the top level when absent) at
    /// `index` among its children (the end when absent). The model mints
    /// the bookmark's id and reports it in the reply. Undoable in one step.
    CreateBookmark {
        name: String,
        #[serde(default)]
        page: Option<PageId>,
        #[serde(default)]
        destination: Option<String>,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        index: Option<u32>,
    },
    /// Delete bookmark `bookmark_id` (ADR 173) and the bookmarks under it,
    /// as InDesign does, and the page destinations nothing else goes to; a
    /// text destination stays. Undoable in one step.
    DeleteBookmark {
        bookmark_id: String,
    },
    /// Rename bookmark `bookmark_id` (ADR 173). Undoable in one step.
    RenameBookmark {
        bookmark_id: String,
        name: String,
    },
    /// Insert an endnote (ADR 174): its reference at story char `offset`
    /// (the `InsertFootnote` address space) and its range in the endnote
    /// story, after the range of the reference before it, `text` after
    /// its number and separator, as InDesign 20.0.1 writes one. The first
    /// endnote of a document makes the endnote story as InDesign does: a
    /// page at the end with an `<EndnoteTextFrame>` over its margin box,
    /// the story holding the title. The model mints the ids and reports the
    /// endnote's in the reply. Undoable in one step.
    InsertEndnote {
        story_id: String,
        offset: u32,
        /// The paragraph `offset` stands in; `offset` is clamped into it.
        #[serde(default)]
        paragraph: Option<u32>,
        #[serde(default)]
        text: String,
    },
    /// Delete an endnote by its `<Endnote>` `Self` id (ADR 174): its
    /// reference and its range. Undoable in one step.
    DeleteEndnote {
        endnote_id: String,
    },
    /// Convert a footnote to an endnote (ADR 174): the footnote goes and an
    /// endnote of its text takes its reference's place. InDesign's DOM has
    /// no such call (its Convert Footnote and Endnote is a dialog), so this
    /// is the two ops InDesign has, in one undo step.
    ConvertFootnoteToEndnote {
        story_id: String,
        footnote_id: String,
    },
    /// Move bookmark `bookmark_id` with the bookmarks under it (ADR 173):
    /// under `parent` (the top level when absent) at `index` among its
    /// children (the end when absent). Undoable in one step.
    MoveBookmark {
        bookmark_id: String,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        index: Option<u32>,
    },
    /// v43 (D-01) — update the cached display value of the placeholder
    /// field containing the story char `offset` (offsets come fresh
    /// from `RequestDocumentPlaceholders`). `value: null` returns the
    /// field to its unresolved `<key>` display. ONE undoable step;
    /// the hosting story reflows. Routes to
    /// `Operation::SetFieldValue`.
    SetFieldValue {
        story_id: String,
        offset: u32,
        #[serde(default)]
        value: Option<String>,
    },
    /// v43 (D-14) — place an image asset on a graphic frame
    /// (Rectangle / Oval / Polygon): sets the frame's image link (the
    /// parsed `LinkResourceURI` lane). The renderer shows the image
    /// iff the asset resolver serves `uri`; an unreachable uri leaves
    /// the frame rendering as before (honest miss, no badge). `fit`
    /// takes the IDML `FittingOnEmptyFrame` vocabulary (the same
    /// strings as the `frame.fittingType` property; Rectangle-only).
    /// Routes to `Operation::PlaceImage`.
    PlaceImage {
        element_id: String,
        uri: String,
        #[serde(default)]
        fit: Option<String>,
    },
    /// v50 (C-1 Stage B — pixel save-back) — commit processed pixels as the
    /// frame's INLINE image bytes (the decoded `image_bytes` lane the
    /// renderer prefers over a `<Link>` uri). The undoable companion to the
    /// ephemeral per-drag `SubmitPixelLayer` preview: that composites tiles
    /// over the frame DURING a gesture; this writes the result into the
    /// document as ONE undoable mutation. `element_id` resolves to a
    /// Rectangle / Oval / Polygon. `bytes: None` clears the inline payload.
    /// Routes to `Operation::ReplaceImageBytes`.
    ReplaceImageBytes {
        element_id: String,
        #[serde(default)]
        #[tsify(type = "number[] | null")]
        bytes: Option<ByteBuf>,
    },
    /// Set a page item's whole placement transform, in the space
    /// `elementGeometry` reads it: the page the item sits on, its origin
    /// at `(0, 0)` (spread space for an item on the pasteboard). The
    /// engine adds the page's origin inside its spread back before
    /// storing the `ItemTransform`, so read, translate, write moves the
    /// item by exactly the step on any page. The `frameTransform`
    /// property is the stored, spread-space matrix.
    MoveFrame {
        frame_id: String,
        transform: [f32; 6],
    },
    ResizeFrame {
        frame_id: String,
        bounds: (f32, f32, f32, f32),
    },
    /// W0.5 — thread `from`'s overflow into the empty frame `to`.
    /// Routes to `Operation::LinkFrames`.
    LinkFrames {
        from: String,
        to: String,
    },
    /// W0.5 — break the thread leaving `frame`. Routes to
    /// `Operation::UnlinkFrames`.
    UnlinkFrames {
        frame: String,
    },
    /// A new single-page spread after `after_page_id` (or at the end).
    /// v69 (D-23) — the page takes its master's margins, else those of
    /// the page it follows.
    InsertPage {
        after_page_id: Option<PageId>,
        master_id: Option<String>,
    },
    DeletePage {
        page_id: PageId,
    },
    /// Editor-ops (Page tool) — resize the page's GeometricBounds
    /// (page-inner coords, `(top, left, bottom, right)`). Items keep
    /// their coordinates; spread origins re-derive on rebuild.
    ResizePage {
        page_id: PageId,
        bounds: (f32, f32, f32, f32),
    },
    InsertFrame {
        page_id: PageId,
        bounds: (f32, f32, f32, f32),
    },
    /// W2.0 (rides v28) — insert an EMPTY text frame (no story). The
    /// threading target `LinkFrames` requires, and the Type tool's
    /// frame-draw gesture. Same page-local bounds as `InsertFrame`.
    InsertTextFrame {
        page_id: PageId,
        bounds: (f32, f32, f32, f32),
    },
    DeleteFrame {
        frame_id: String,
    },
    /// Editor-ops — the Line tool. `start`/`end` are page-local pt;
    /// the model converts to spread coordinates, mints a self id, and
    /// inserts a two-anchor open `GraphicLine` (document-default
    /// stroke applied).
    InsertLine {
        page_id: PageId,
        start: (f32, f32),
        end: (f32, f32),
    },
    /// Editor-ops — the Pencil tool (and any caller with explicit
    /// path geometry). `anchors` are page-local; `open` marks an open
    /// contour. `smooth: true` runs the engine's Bezier fitter over
    /// the (typically RDP-simplified) polyline so freehand strokes
    /// land as curves rather than corner chains.
    InsertPath {
        page_id: PageId,
        anchors: Vec<paged_mutate::operation::PathAnchorSpec>,
        open: bool,
        #[serde(default)]
        smooth: bool,
    },
    /// Editor-ops — document defaults for NEWLY-CREATED objects (the
    /// fill/stroke wells with nothing selected). Whole-triple
    /// semantics: every field IS the new default (`None` = no fill /
    /// no stroke / engine-default weight) — the editor reads the
    /// current triple from `DocumentMeta` and writes it back
    /// modified. App-level state: not undoable, no scene rebuild.
    SetDocumentDefaults {
        fill_color: Option<String>,
        stroke_color: Option<String>,
        stroke_weight: Option<f32>,
    },
    /// Concept 2 — replace the document's colour-management
    /// settings. WHOLE-STATE semantics like `SetDocumentDefaults`
    /// (the editor reads `DocumentMeta`, modifies, writes back the
    /// full set). Not undoable (output/app configuration, not
    /// content), but unlike the defaults it FORCES a full rebuild —
    /// switching the CMYK working space must visibly change the
    /// canvas (AC-3).
    ///
    /// `cmyk_profile_name` resolves against the
    /// `RegisterColorProfile` registry; `None` restores the
    /// load-time profile (the `LoadDocument` `cmykIccProfile` bytes
    /// or a registry hit on the designmap's profile name). An
    /// unknown name fails the mutation. `intent` is one of the four
    /// ICC rendering-intent names; `None` ⇒ Relative Colorimetric.
    /// `rgb_policy` is carried for Concept 3 ("preserve" |
    /// "convertToWorkingSpace" | "off"); display ignores it today.
    SetColorSettings {
        cmyk_profile_name: Option<String>,
        rgb_policy: Option<String>,
        intent: Option<String>,
        bpc: Option<bool>,
    },
    /// Concept 2 — soft-proofing (InDesign "Proof Colors" / "Proof
    /// Setup"). `profile_name: Some` simulates the named output
    /// condition on the canvas: CMYK content renders through the
    /// PROOF profile instead of the working space (the numbers go
    /// to the device unconverted — printing's native semantics);
    /// `simulate_paper_white` switches the proof transform to
    /// absolute-colorimetric so CMYK 0/0/0/0 lands on the
    /// condition's media white instead of display white.
    /// `profile_name: None` turns proofing off. Not undoable;
    /// forces a full rebuild. v1 scope: CMYK content proofs on both
    /// targets; RGB/Lab content stays display-resolved (the full
    /// cross-space proofing transform is native-lcms2 territory and
    /// lands with Concept 3's export work).
    SetProofSetup {
        profile_name: Option<String>,
        #[serde(default)]
        simulate_paper_white: bool,
        intent: Option<String>,
    },
    /// Concept 2 — import an Adobe Swatch Exchange (`.ase`) library
    /// (the freieFarbe HLC atlas, arbitrary user libraries). The
    /// worker parses the raw bytes; every colour lands as a swatch
    /// and every `.ase` group becomes a ColorGroup, all inside ONE
    /// undoable operation (a single Cmd-Z removes the whole
    /// import). `group_name` overrides the group for entries the
    /// file leaves ungrouped. Names are preserved verbatim (for HLC
    /// the name IS the colour identity / provenance).
    ImportSwatchLibrary {
        #[tsify(type = "number[]")]
        bytes: ByteBuf,
        #[serde(default)]
        group_name: Option<String>,
    },
    /// Concept 2 (Ink Manager) — replace one ink's output-time
    /// settings (whole-row semantics). Not undoable; never touches
    /// the swatch. Settings surface through the `inks` collection;
    /// separations consume them at export (Concept 3).
    SetInkSetting {
        spot_id: String,
        #[serde(default)]
        convert_to_process: bool,
        #[serde(default)]
        alias_to: Option<String>,
    },
    /// Concept 2 (Ink Manager) — prefer a spot's device-independent
    /// Lab PRIMARY over its CMYK alternate when resolving previews
    /// (InDesign's "Use Standard Lab Values for Spots"). Repaints
    /// previews; not undoable.
    SetUseStandardLabForSpots {
        enabled: bool,
    },
    /// Track J — insert a new anchor into a path-bearing element's
    /// PathPointArray at flat `index`. UI dispatches from a segment
    /// click in path-edit mode; `anchor` is the de Casteljau split
    /// result so the curve's visible shape is preserved.
    ///
    /// `element_id` accepts any of the four path-bearing kinds —
    /// Polygon (the original v1 target), TextFrame, Rectangle, and
    /// GraphicLine. The apply layer routes via the kind discriminant.
    ///
    /// `prev_subpath_starts` is the closing-edge override path: when
    /// inserting at a subpath boundary (the wraparound segment from
    /// the last anchor of a closed subpath back to its first), the
    /// apply layer's default "strictly-greater" increment rule would
    /// make the new anchor join the NEXT subpath. Passing the
    /// desired post-Insert starts here overrides that rule. Omit
    /// (`None`) for the common internal-segment insert.
    PathPointInsert {
        element_id: ElementId,
        index: u32,
        anchor: paged_mutate::operation::PathAnchorSpec,
        #[serde(default)]
        prev_subpath_starts: Option<Vec<u32>>,
    },
    /// Track J — remove the anchor at flat `index` from any path-
    /// bearing element. UI dispatches from Backspace/Delete on the
    /// selected anchor.
    PathPointRemove {
        element_id: ElementId,
        index: u32,
    },
    /// Editor-ops (Scissors) — cut the path at the anchor at flat
    /// `index`: a closed contour opens there (the anchor splits into
    /// two coincident endpoints); an open contour splits into two.
    /// For a mid-segment cut the editor sends
    /// `Batch [PathPointInsert (the de Casteljau split), PathOpenAt]`
    /// so the whole cut is one undo step.
    PathOpenAt {
        element_id: ElementId,
        index: u32,
    },
    /// B-05 (protocol v30) — replace the element's path with its
    /// stroke-expansion outline. Geometry-only: the editor composes
    /// paint transfer (fill := old stroke, stroke := none) as a
    /// Batch alongside this op. `cap`: `"butt"|"round"|"square"`;
    /// `join`: `"miter"|"round"|"bevel"`.
    OutlineStroke {
        element_id: ElementId,
        width: f32,
        cap: String,
        join: String,
        miter_limit: f32,
    },
    /// B-05 (protocol v30) — inset (`delta < 0`) / outset
    /// (`delta > 0`) of a single closed contour.
    OffsetPath {
        element_id: ElementId,
        delta: f32,
        join: String,
        miter_limit: f32,
    },
    /// B-05 (protocol v30) — re-express the path within `tolerance`
    /// pt max deviation with fewer anchors.
    SimplifyPath {
        element_id: ElementId,
        tolerance: f32,
    },
    /// v56 (Wave B) — close an OPEN subpath of a path element: the
    /// inverse gesture of `PathOpenAt`'s scissors cut. `subpath`
    /// picks the contour by index; omit for the default — the
    /// single/last open subpath. Coincident endpoints (the
    /// duplicated pair a cut leaves) merge back into one anchor;
    /// endpoints apart gain the implicit straight closing edge.
    ClosePath {
        element_id: ElementId,
        #[serde(default)]
        subpath: Option<u32>,
    },
    /// v56 (Wave B) — weld two OPEN single-contour path elements
    /// into one (InDesign's Join): connect the NEAREST endpoints of
    /// `element_id` and `other_id` (appending the other's anchors in
    /// the matching orientation), then delete `other_id`. If both
    /// endpoint pairs are coincident the result closes into a ring.
    /// Non-path / closed / multi-contour inputs are rejected with an
    /// honest error (no-op). One undo step restores both elements
    /// (the same Batch machinery `PathfinderBoolean` rides).
    JoinPaths {
        element_id: ElementId,
        other_id: ElementId,
    },
    /// v56 (B-18) — InDesign paste-into: nest an existing TOP-LEVEL
    /// page item inside a container Rectangle / Oval / Polygon. The
    /// child keeps its document-space geometry (nothing moves on
    /// canvas) and renders clipped by the container's outline. Rides
    /// `Operation::PasteInto`; one undo pops the child back to its
    /// exact stacking slot. Grouped / already-nested children and
    /// non-container hosts are rejected with an honest error.
    PasteInto {
        container_id: ElementId,
        child_id: ElementId,
    },
    /// v56 (B-18) — the inverse gesture: pop a pasted-in child back
    /// to top level (stacks on top), world transform preserved.
    /// Rides `Operation::ReleaseFrom`; one undo re-nests at the same
    /// child index.
    ReleaseFrom {
        child_id: ElementId,
    },
    /// v59 — **Arrange**: restack an element within the sibling list
    /// it already belongs to. The z-order door; nothing on the wire
    /// could change stacking before this.
    ///
    /// `to` is `"front" | "back" | "forward" | "backward" |
    /// { index: n }`. Prefer the relative verbs: they are evaluated
    /// against the order the engine holds at apply time, so a
    /// concurrent insert/delete cannot make them restack the wrong
    /// slot. `{ index }` is absolute (0 = backmost, painted first) —
    /// exactly what the inverse carries, so one undo restores the
    /// previous order bytewise; an out-of-range index is rejected with
    /// an honest error rather than clamped.
    ///
    /// The sibling list is DERIVED from where the element is: a
    /// top-level item reorders in the spread's z table, a group member
    /// inside its group, a B-18 pasted-in child inside its container.
    /// There is no parent argument, so a reorder cannot move anything
    /// between those scopes — use `pasteInto` / `releaseFrom` /
    /// `createGroup` / `dissolveGroup` for that. A C-28 opacity-mask
    /// artwork is painted from no list and is rejected.
    ///
    /// **Create-then-arrange in ONE undo step** (the paged.draw Live
    /// Paint shape — fill a face, then put the fill UNDER the strokes
    /// bounding it): batch `[insert…, bindCreated { handle },
    /// reorderElement { elementId: "$h:<handle>", to: "back" }]`. Note
    /// the `bindCreated`: the bare v34 `$created` sentinel is understood
    /// by `setPluginMetadata` / `setElementProperty` only, while the
    /// C-15 handle resolver rewrites any id position of any mutation
    /// kind — and it is armed by a `bindCreated` child being present.
    ///
    /// **Honest limit 1 — Arrange is WITHIN a layer.** The renderer
    /// sorts the z table by `ItemLayer` first (Q-10), so bring-to-front
    /// cannot lift an item above one on a higher layer — same as
    /// InDesign, where crossing layers is a different gesture, not an
    /// Arrange.
    ///
    /// **THAT GESTURE IS NOT IMPLEMENTED (C-35).** This used to say
    /// `setElementProperty` on the layer, which does not work: there is
    /// no layer `PropertyPath` and no `ElementId` that addresses a
    /// layer. Moving an item across layers is inexpressible today.
    ///
    /// **Both save paths carry it.** `.paged` rides the native model
    /// part; `.idml` gained a z-reorder save-back lane in the writer
    /// (`idml-export`'s `reorder.rs`), which SPLICES each page item's
    /// serialised bytes into its new slot rather than re-minting the
    /// element — a re-mint would rebuild only what the model tracks and
    /// would silently drop `<Image>`, `<TextWrapPreference>` and every
    /// unparsed attribute. It moves only ids the model's sibling list
    /// names, and returns the input buffer untouched when nothing
    /// moved, so unmutated documents keep their verbatim ZIP copy.
    /// Pinned by `a_reorder_survives_both_paged_and_idml_export`.
    ///
    /// Note this covers ALL THREE sibling lists a node can live in —
    /// top level, `Group::members`, and a container's nested children —
    /// not just the spread's z table.
    ///
    /// Rides `Operation::ReorderNode`.
    ReorderElement {
        element_id: ElementId,
        to: paged_mutate::ZOrderTarget,
    },
    /// v58 (C-28) — Illustrator's **Make Opacity Mask**: the item at
    /// `mask_id` stops painting on its own and becomes the alpha mask
    /// of the item at `target_id`. Unlike `PasteInto`'s hard clip the
    /// coverage is CONTINUOUS, so a black→white gradient fades the
    /// target out. Both must be Rectangle / Oval / GraphicLine /
    /// Polygon on the SAME spread; a TextFrame on either side is
    /// rejected with an honest error (its glyphs are emitted outside
    /// the mask bracket). `mask_type` defaults to `"luminosity"`
    /// (Illustrator's default — the artwork's colour drives coverage);
    /// `"alpha"` reads its opacity instead. `invert` is Illustrator's
    /// Invert Mask. Rides `Operation::ApplyOpacityMask`; one undo pops
    /// the artwork back to its exact stacking slot.
    ApplyOpacityMask {
        target_id: ElementId,
        mask_id: ElementId,
        #[serde(default)]
        mask_type: Option<String>,
        #[serde(default)]
        invert: Option<bool>,
    },
    /// v58 (C-28) — **Release Opacity Mask**: drop the relation; the
    /// artwork returns to top level, geometry untouched. Rides
    /// `Operation::ReleaseOpacityMask`; one undo re-applies the mask
    /// with its original mode/invert at the same z slot.
    ReleaseOpacityMask {
        target_id: ElementId,
    },
    /// v58 (C-29) — InDesign's **Type on a Path**: flow an existing
    /// story along an existing path element. `element_id` must be a
    /// Rectangle / GraphicLine / Polygon (the kinds the renderer's
    /// text-path pass walks); the story must exist and must not
    /// already flow into a text frame or onto another path. The
    /// optional knobs are the ones the renderer HONOURS:
    /// `path_type_alignment` (`BaselinePathType` default /
    /// `CenterPathType` / `AscenderPathType` / `DescenderPathType`),
    /// `flip_path_effect` (`Flipped` / `NotFlipped`), and the
    /// `start_bracket` / `end_bracket` arc-length window (text is
    /// centred in the window when it fits; glyphs past the end are
    /// dropped and reported as overset). `PathEffect` is deliberately
    /// absent — only `RainbowPathEffect` renders, so there is no knob
    /// to set. Rides `Operation::AttachTextToPath`.
    AttachTextToPath {
        element_id: ElementId,
        story_id: String,
        #[serde(default)]
        path_type_alignment: Option<String>,
        #[serde(default)]
        flip_path_effect: Option<String>,
        #[serde(default)]
        start_bracket: Option<f32>,
        #[serde(default)]
        end_bracket: Option<f32>,
    },
    /// v58 (C-29) — the inverse gesture: unlink the text from the
    /// path. **The story survives** (unlike InDesign's "Delete Type
    /// from Path", which also deletes the text) — attach only ever
    /// linked an existing story, so unlinking is its exact inverse and
    /// one undo re-attaches with the same knobs. Takes the host's
    /// first `<TextPath>`. Rides `Operation::DetachTextFromPath`.
    DetachTextFromPath {
        element_id: ElementId,
    },
    /// B-04 (protocol v32) — group page items on one spread.
    /// Reference-based and z-order-neutral (the group takes the
    /// earliest member's paint slot). Reply carries the minted group
    /// id as `createdId` so the editor can select it. W1.20 (groups
    /// v2): `member_ids` may include existing `Group`s, producing a
    /// nested group-of-groups.
    CreateGroup {
        member_ids: Vec<ElementId>,
    },
    /// B-04 (protocol v32) — dissolve a group; members return to the
    /// group's paint slot in stored order. W1.20 (groups v2):
    /// dissolving a NESTED group splices its members back into the
    /// parent group at the right slot (geometry unchanged), rather
    /// than rejecting.
    DissolveGroup {
        group_id: String,
    },
    /// C-64 (rides v65) — duplicate page items. Each of `elementIds`
    /// gets a clone translated by `offset` (`[dx, dy]`, points, spread
    /// space), inserted DIRECTLY ABOVE its source — in the spread's
    /// z-order, or inside the group / container that holds the source —
    /// all in one undo step. A clone is the whole item: every property,
    /// its plugin metadata, a container's pasted-in children, a group's
    /// members (nested groups too), and a text frame's story, copied
    /// under a fresh id.
    ///
    /// The reply's `minted` lists the clones in `elementIds` order (one
    /// per source — a group's clone is its group id; its members are
    /// reached through it), and `createdId` is the last of them.
    ///
    /// Refused as a whole, by name, when any source cannot be cloned
    /// completely: not a page item on a body spread; anchored in a
    /// story; serving as or carrying an opacity mask; a threaded text
    /// frame; a text frame whose story holds a table, an anchored
    /// object or a footnote; an element named together with a group or
    /// container that holds it. Rides `Operation::DuplicateNodes`.
    ///
    /// v69 (D-24) — a story holding hyperlink SOURCES (every Data Merge
    /// placeholder is one) is copied: the copy's sources get fresh ids
    /// and each owning hyperlink is copied onto them, same destination.
    DuplicateElements {
        element_ids: Vec<ElementId>,
        offset: (f32, f32),
    },
    /// W1.20 (groups v2, rides v35) — move/scale/rotate a group as a
    /// unit. The engine sets the group's own `ItemTransform` and
    /// rebases every descendant member's effective transform by the
    /// delta so the members follow rigidly; the hit-tester sees them at
    /// their transformed positions automatically. `transform: None` ⇒
    /// identity.
    ///
    /// `transform` is the group's STORED transform, the one the group's
    /// `frameTransform` / `groupTransform` reads answer: relative to the
    /// group that holds it, as IDML stores it (spread space for a
    /// top-level group). Nested groups keep theirs and move with it. To
    /// move a group by `(dx, dy)` on the spread, add `(dx, dy)` seen from
    /// its parent's frame: for a top-level group, or one whose ancestors
    /// only translate, that is `(dx, dy)` itself.
    SetGroupTransform {
        group_id: String,
        #[serde(default)]
        transform: Option<[f32; 6]>,
    },
    /// Plugin-metadata carrier (protocol v33) — one Label
    /// `KeyValuePair` on a leaf page item. `value: None` deletes the
    /// entry. The engine gates the write (reserved `x-paged:` key
    /// namespace, 64 KiB cap, JSON envelope). B-16: an optional
    /// `caller` (the calling plugin's manifest id) makes the engine ALSO
    /// enforce that the key is in that plugin's own namespace — the
    /// server-side half of the identity gate the SDK door enforces.
    /// `None` keeps the prior behaviour (the editor / pre-B-16 callers).
    SetPluginMetadata {
        element_id: ElementId,
        key: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        caller: Option<String>,
    },
    /// v70 — `SetPluginMetadata` for a page: one Label `KeyValuePair`
    /// on the page itself (a slide's notes, transition, hidden flag), with
    /// the same gates (reserved `x-paged:` namespace, 64 KiB cap, JSON
    /// envelope, optional `caller`). It travels with the page through
    /// move, duplicate, delete and undo. `value: None` deletes the entry.
    SetPageMetadata {
        page: PageId,
        key: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        caller: Option<String>,
    },
    /// v69 — document-scoped plugin metadata: one Label `KeyValuePair` on
    /// the DOCUMENT rather than a page item, for state that belongs to no
    /// frame (a data session, the live version of a plugin's container
    /// parts). `value: None` deletes. Same gates as `SetPluginMetadata`
    /// (`x-paged:` namespace, optional `caller` gate, 64 KiB, JSON
    /// envelope); ONE undoable step that composes in a `batch`. Read back
    /// through `RequestDocumentMeta` → `DocumentMeta.pluginMetadata`.
    SetDocumentMetadata {
        key: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        caller: Option<String>,
    },
    /// Track J — toggle the curve type of an anchor between corner
    /// (handles equal to anchor) and smooth (handles derived from
    /// neighbour tangents). UI dispatches from a double-click on
    /// the anchor.
    PathPointCurveType {
        element_id: ElementId,
        index: u32,
        smooth: bool,
    },
    /// Track J — direct write of one Bezier handle (anchor / left /
    /// right) on an element's PathPointArray. Phase H's drag-anchor
    /// gesture already does this through `Operation::SetProperty`
    /// at the apply layer, but the channel exposed it only through
    /// the gesture path; the segment-click insert (J.5b) needs
    /// it as a direct mutation so a curve-preserving Batch can
    /// adjust the two segment-endpoint handles alongside the new
    /// anchor's insertion.
    PathPointSet {
        element_id: ElementId,
        index: u32,
        role: paged_mutate::PathPointRole,
        position: [f32; 2],
    },
    /// Track J — atomic group of mutations recorded as one undo
    /// entry. The segment-click insert uses this to update the
    /// neighbouring anchors' Bezier handles AND insert the new
    /// mid-anchor in one Cmd-Z step. Children translate
    /// recursively; an empty ops vec is a valid no-op (mirrors
    /// `Operation::Batch` semantics in paged-mutate).
    Batch {
        ops: Vec<Mutation>,
    },
    /// C-15 (v57) — name the element the most recent CREATING child of
    /// this batch minted, so LATER children can address it as
    /// `$h:<handle>` (anywhere an id is expected: an `elementId` /
    /// `frameId` / `memberIds` entry, or a `storyId` when the handle
    /// names a text frame — the frame's minted `ParentStory`). Any
    /// number of handles may be live at once, which is what the v34
    /// `$created` sentinel could not do: it named only the LAST
    /// creation, and only two mutation kinds understood it.
    ///
    /// Legal ONLY as a direct child of a `Batch` — standalone it
    /// addresses nothing and is rejected. It contributes no operation
    /// and no undo entry of its own; a batch that uses handles is still
    /// exactly ONE undo step. Binding before any creating child fails
    /// the batch (nothing to name). Scope is the batch that declares it:
    /// visible to its own later children INCLUDING nested batches,
    /// never outward.
    ///
    /// See `batch_handles` for the resolution rules — notably that a
    /// `$h:` in a text payload is content and is never rewritten.
    ///
    /// v69 (paged.data D-22) — a PAGE is named too: after an
    /// `insertPage` / `duplicatePage` child, `bindCreated` binds the page
    /// it minted, and `$h:<handle>` resolves in a page position
    /// (`pageId`, `afterPageId`, `page`, `atPage`) — and only there; an
    /// element handle in a page position, or a page handle anywhere
    /// else, fails the batch. Pages and their content are then ONE undo
    /// step. A page is not an element, so it is not listed in `minted`.
    BindCreated {
        handle: String,
    },
    /// Track M — `<Layer>` visibility toggle. The Layers panel
    /// dispatches this when the user clicks the eye icon.
    LayerSetVisible {
        layer_id: String,
        visible: bool,
    },
    /// Track M — `<Layer>` lock toggle.
    LayerSetLocked {
        layer_id: String,
        locked: bool,
    },
    /// Track M — `<Layer>` printable toggle.
    LayerSetPrintable {
        layer_id: String,
        printable: bool,
    },
    /// Track M — `<Layer>` rename.
    LayerSetName {
        layer_id: String,
        name: String,
    },
    /// Track M — reorder a layer to a new zero-based index.
    LayerMove {
        layer_id: String,
        new_index: u32,
    },
    /// Track M — append a new layer. Apply layer assigns the
    /// self-id deterministically; the panel can ignore the
    /// returned id and re-fetch via `RequestLayers`.
    LayerInsert {
        position: u32,
        name: String,
    },
    /// Track M — remove a layer. Inverse restores the layer's
    /// previous flags and name in a single Cmd-Z step.
    LayerRemove {
        layer_id: String,
    },
    /// Inspector P1 — generic property write. Routes the named
    /// element's property edit through `Operation::SetProperty`,
    /// covering whatever path/value variants the apply layer
    /// already understands. The Inspector + REPL both ride this
    /// shape; the gesture spine's typed ops (`MoveFrame`,
    /// `ResizeFrame`, `LayerSet*`) stay as ergonomic shortcuts.
    SetElementProperty {
        element_id: ElementId,
        path: paged_mutate::PropertyPath,
        value: paged_mutate::Value,
    },
    /// ADR 132 — set one property on any object, named by `(address,
    /// path)`. `path` is a `PropertyPath` name or a virtual path from the
    /// catalog's `propertySchemas` (`name`, `tint`, `labels[x-paged:k]`,
    /// `rows[2].height`, …). The engine resolves it to the ONE existing
    /// operation that already writes that property (`SetProperty`,
    /// `EditSwatch` with the spec read and merged, `LayerSetName`, …), so
    /// there is no second apply path: one undo step, legal in a batch,
    /// `$h:` handles resolve inside `address`. The value is checked
    /// against the path's schema first (enum members, ranges; `Number`,
    /// `Enum`, `Ref` normalise).
    ///
    /// `address` is ONE address string (ADR 131) — `"doc"`,
    /// `"swatch:Color/Red"`, `"cell:u2f/0,1"`, `"frame:u12"`,
    /// `"page:#3"`. A SELECTOR is not a target: resolve it with
    /// `RequestQuery` first and send one `Set` per address (in one
    /// `Batch` for one undo step). The protocol-70 target object
    /// (`{ "kind": "layer", "layerId": "ua" }`) still parses here as a
    /// deprecated alias; replies and re-encodings use the string.
    Set {
        #[serde(with = "address_wire")]
        #[tsify(type = "string")]
        address: Address,
        path: String,
        value: paged_mutate::Value,
    },
    /// SDK Phase 5 (v1 sweep) — Pathfinder boolean op routed
    /// through `Operation::PathfinderBoolean`. Same wire shape
    /// the Pathfinder panel emits on a button click.
    PathfinderBoolean {
        kept: ElementId,
        others: Vec<ElementId>,
        kind: paged_mutate::PathfinderKind,
    },

    // ── B-22 (protocol v57) — the REGION Pathfinder row. Where the
    //    Shape Modes above combine paths into one, these six resolve the
    //    planar ARRANGEMENT of the inputs (the faces overlapping paths
    //    divide the plane into) and operate per face. `element_ids` is
    //    TOP-TO-BOTTOM stacking order — index 0 is the frontmost object,
    //    the convention `PathfinderBoolean`'s `kept`-is-top already
    //    sets. Each rides `Operation::PathfinderRegion`; one undo
    //    restores every input.
    /// Every face of the arrangement becomes its own object, keeping
    /// the attributes of the topmost input covering it.
    PathfinderDivide {
        element_ids: Vec<ElementId>,
    },
    /// Each input is clipped to the part nothing above it covers, and
    /// loses its stroke. Inputs entirely hidden are deleted.
    PathfinderTrim {
        element_ids: Vec<ElementId>,
    },
    /// Trim, then coalesce: inputs that share a fill colour merge into
    /// one object.
    PathfinderMerge {
        element_ids: Vec<ElementId>,
    },
    /// Keep only what falls inside the TOPMOST input, coloured by the
    /// objects beneath it; the topmost input is consumed as the cookie
    /// cutter and disappears.
    PathfinderCrop {
        element_ids: Vec<ElementId>,
    },
    /// Fills become strokes: every arrangement edge (split at each
    /// crossing) becomes an open `GraphicLine` stroked with the fill of
    /// the input that contributed it. All inputs are consumed.
    PathfinderOutline {
        element_ids: Vec<ElementId>,
    },
    /// Illustrator's Minus Back: the FRONTMOST element (the first of
    /// `elementIds`, which is top-to-bottom) minus every element behind
    /// it, in the front element's paint; the others are consumed.
    PathfinderMinusBack {
        element_ids: Vec<ElementId>,
    },
    /// B-22 (protocol v57) — Shape Builder's click/drag output: ONE
    /// element built from the selected faces of the arrangement, united.
    /// `faces` carries the stable ids `RequestPlanarRegions` reported;
    /// `mode` selects them (`keep`) or every other face (`remove`; with
    /// no id named, every face — the op then unites its inputs). The
    /// result is written onto the topmost input covering a selected face
    /// and every other input is removed: a face outside the selection is
    /// NOT kept. A Shape Builder that leaves the faces a gesture did not
    /// cross builds those pieces itself (paged.draw does) and unites only
    /// the crossed ones here. An unknown id is refused, not ignored.
    PathfinderFaces {
        element_ids: Vec<ElementId>,
        faces: Vec<String>,
        mode: paged_mutate::FaceSelectMode,
    },

    // ── Collection mutations (swatches / gradients / colour groups /
    //    styles) — route 1:1 to the matching `paged_mutate::Operation`.
    //    The Swatches / Styles / Gradients panels emit these on their
    //    new / edit / delete affordances. `restore_json` (style delete
    //    undo) is engine-internal and never travels from the editor.
    CreateSwatch {
        spec: paged_mutate::SwatchSpec,
    },
    EditSwatch {
        swatch_id: String,
        spec: paged_mutate::SwatchSpec,
    },
    DeleteSwatch {
        swatch_id: String,
    },
    CreateGradient {
        spec: paged_mutate::GradientSpec,
    },
    EditGradient {
        gradient_id: String,
        spec: paged_mutate::GradientSpec,
    },
    DeleteGradient {
        gradient_id: String,
    },
    CreateColorGroup {
        spec: paged_mutate::ColorGroupSpec,
    },
    EditColorGroup {
        group_id: String,
        spec: paged_mutate::ColorGroupSpec,
    },
    DeleteColorGroup {
        group_id: String,
    },
    // W1.22 (engine gap 22) — numbering-list CRUD. New Mutation
    // variants → new Operation variants. // rides v35 (added before
    // first consumer sync; v35 bumped in W1.23 is not yet tagged /
    // published — highest tag is v0.34.0).
    CreateNumberingList {
        spec: paged_mutate::NumberingListSpec,
    },
    EditNumberingList {
        list_id: String,
        spec: paged_mutate::NumberingListSpec,
    },
    DeleteNumberingList {
        list_id: String,
    },
    CreateParagraphStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
    },
    RenameParagraphStyle {
        style_id: String,
        name: String,
    },
    DeleteParagraphStyle {
        style_id: String,
    },
    CreateCharacterStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
    },
    RenameCharacterStyle {
        style_id: String,
        name: String,
    },
    DeleteCharacterStyle {
        style_id: String,
    },
    CreateObjectStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
    },
    RenameObjectStyle {
        style_id: String,
        name: String,
    },
    DeleteObjectStyle {
        style_id: String,
    },
    CreateCellStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
    },
    RenameCellStyle {
        style_id: String,
        name: String,
    },
    DeleteCellStyle {
        style_id: String,
    },
    CreateTableStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
    },
    RenameTableStyle {
        style_id: String,
        name: String,
    },
    DeleteTableStyle {
        style_id: String,
    },
    /// Style-options editing — set one property on a style definition.
    SetStyleProperty {
        collection: paged_mutate::StyleCollection,
        style_id: String,
        path: paged_mutate::PropertyPath,
        value: paged_mutate::Value,
    },
    // ── W0.5 wire-expansion ─────────────────────────────────────────
    /// W0.5 — insert an Oval (Ellipse tool). `page_id` + page-local
    /// `bounds` `(top, left, bottom, right)`; the model resolves the
    /// host spread and mints the self id (mirrors `InsertFrame`).
    InsertOval {
        page_id: PageId,
        bounds: (f32, f32, f32, f32),
    },
    /// W0.5 — insert a ruler guide. `position` is page-local on the
    /// perpendicular axis. Routes to `Operation::InsertGuide`.
    InsertGuide {
        spread_id: String,
        orientation: paged_mutate::operation::GuideOrientationSpec,
        position: f32,
        #[serde(default)]
        page_index: u32,
    },
    /// W0.5 — move a guide by its `Operation::InsertGuide`-minted id.
    MoveGuide {
        guide_id: String,
        position: f32,
    },
    /// W0.5 — delete a guide.
    DeleteGuide {
        guide_id: String,
    },
    /// W0.5 — flip a condition's visibility.
    SetConditionVisible {
        condition: String,
        visible: bool,
    },
    /// ADR 026 — let a story's frame chain GROW (`grow: true`):
    /// generated pages are added after its last frame's page while it
    /// oversets, the way InDesign's Smart Text Reflow adds them, and dropped
    /// when they end up empty. `grow: false` clears the rule. `maxPages`
    /// caps the generated pages (renderer default when absent).
    /// `copyFrameOptions` gives generated frames the chain's frame options
    /// instead of InDesign's defaults (a Word section's mode, ADR 029).
    SetFlowGrowRule {
        story_id: String,
        grow: bool,
        #[serde(default)]
        max_pages: Option<u32>,
        #[serde(default)]
        copy_frame_options: Option<bool>,
    },
    /// v71 — define a CUSTOM text variable (`<TextVariable
    /// VariableType="CustomTextType">`). Its `Self` is
    /// `dTextVariablen<name>` (InDesign's spelling), so the address is
    /// known up front: `textVariable:dTextVariablen<name>`. The name must
    /// be unique. Instances go into text with `InsertField { field: {
    /// textVariable: { variableId } } }`; the value is switched with one
    /// `Set` on `textVariableContents`, which re-bakes every instance.
    /// InDesign keeps a text variable on save (a placeholder it drops).
    CreateTextVariable {
        name: String,
        #[serde(default)]
        contents: Option<String>,
        #[serde(default)]
        text_before: Option<String>,
        #[serde(default)]
        text_after: Option<String>,
    },
    /// v71 — remove a text variable definition. Refused while any
    /// instance of it is still in the text.
    DeleteTextVariable {
        variable_id: String,
    },
    /// W0.5 — "show only this set": activate one `<ConditionSet>`.
    ActivateConditionSet {
        set: String,
    },
    /// W0.5 — set a page's applied master (`None` detaches).
    ApplyMasterToPage {
        page: PageId,
        #[serde(default)]
        master: Option<String>,
    },
    /// W0.5 — duplicate a single-page spread after the source.
    /// v69 (D-23) — the copy owns COPIES of its frames' stories (hyperlink
    /// sources re-minted with their hyperlinks), threads inside the page
    /// kept, and keeps the page's margins; a story holding a table, an
    /// anchored object or a footnote is refused, as `duplicateElements`
    /// refuses it.
    DuplicatePage {
        page: PageId,
    },
    /// v70 — apply `mutation` to the items of master spread `master` (its
    /// `Self` id): edit a layout's logo, footer or background as a page's.
    /// Any page-item, style or property mutation may be wrapped (a `batch`
    /// too); page and spread mutations are refused. Text edits need no
    /// wrapper: they address stories, and a master frame's story is one.
    /// One undo step; every page using the master repaints.
    OnMaster {
        master: String,
        mutation: Box<Mutation>,
    },
    /// v70 — create master spread `master` (its `Self` id, chosen by the
    /// caller): one page of `width_pt` × `height_pt`, or a copy of
    /// `duplicate_of` with fresh ids (the size is then ignored). `name`
    /// defaults to the copied master's. One undo step.
    CreateMaster {
        master: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        width_pt: f32,
        #[serde(default)]
        height_pt: f32,
        #[serde(default)]
        duplicate_of: Option<String>,
    },
    /// v70 — delete a master no page applies; refused while one does.
    /// Undo restores it exactly.
    DeleteMaster {
        master: String,
    },
    /// v70 — set a master's name (absent: clear it).
    RenameMaster {
        master: String,
        #[serde(default)]
        name: Option<String>,
    },
    /// v70 — move a page (its single-page spread) to follow `after`, or to
    /// the front when `after` is absent: a slide sorter's reorder. Spreads
    /// stacked down the pasteboard restack in the new order. One undo step.
    MovePage {
        page: PageId,
        #[serde(default)]
        after: Option<PageId>,
    },
    /// W0.5 — insert a `<Section>` anchored at `at_page`.
    InsertSection {
        at_page: PageId,
        #[serde(default)]
        prefix: Option<String>,
        #[serde(default)]
        numbering_style: Option<String>,
        #[serde(default)]
        start_at: Option<u32>,
    },
    /// W0.5 — edit a `<Section>`. `prefix`/`start_at` are tri-state
    /// (`Some(None)` clears; outer `None` leaves unchanged).
    EditSection {
        section_id: String,
        #[serde(default)]
        prefix: Option<Option<String>>,
        #[serde(default)]
        numbering_style: Option<String>,
        #[serde(default)]
        start_at: Option<Option<u32>>,
    },
    /// W0.5 — delete a `<Section>`.
    DeleteSection {
        section_id: String,
    },
    // ── W3.A1 table structure ───────────────────────────────────────
    /// W3.A1 — set a table row's height in pt (`None` clears the
    /// per-row override). Routes to `Operation::SetRowHeight`.
    SetRowHeight {
        story_id: String,
        table_id: String,
        row: u32,
        #[serde(default)]
        height: Option<f32>,
    },
    /// W3.A1 — set a table column's width in pt. Routes to
    /// `Operation::SetColumnWidth`.
    SetColumnWidth {
        story_id: String,
        table_id: String,
        col: u32,
        #[serde(default)]
        width: Option<f32>,
    },
    /// W3.A1 — insert an empty body row at `at`. Routes to
    /// `Operation::InsertTableRow`.
    InsertTableRow {
        story_id: String,
        table_id: String,
        at: u32,
    },
    /// W3.A1 — delete the row at `at`. Routes to
    /// `Operation::DeleteTableRow` (captures content for undo).
    DeleteTableRow {
        story_id: String,
        table_id: String,
        at: u32,
    },
    /// W3.A1 — insert an empty column at `at`.
    InsertTableColumn {
        story_id: String,
        table_id: String,
        at: u32,
    },
    /// W3.A1 — delete the column at `at`.
    DeleteTableColumn {
        story_id: String,
        table_id: String,
        at: u32,
    },
    // ── W1.12a — header / footer row inserts ────────────────────────
    // New Mutation variants → matching `paged_mutate::Operation`
    // variants. // rides v35 (additive; v35 is unpublished — highest
    // tag is v0.34.0, same posture as the W1.22 list-definition CRUD).
    /// W1.12a — insert an empty row at the top of the header band.
    /// Routes to `Operation::InsertHeaderRow`.
    InsertHeaderRow {
        story_id: String,
        table_id: String,
    },
    /// W1.12a — remove the first header row. Routes to
    /// `Operation::RemoveHeaderRow` (captures content for undo).
    RemoveHeaderRow {
        story_id: String,
        table_id: String,
    },
    /// W1.12a — insert an empty row at the bottom of the footer band.
    /// Routes to `Operation::InsertFooterRow`.
    InsertFooterRow {
        story_id: String,
        table_id: String,
    },
    /// W1.12a — remove the last footer row. Routes to
    /// `Operation::RemoveFooterRow`.
    RemoveFooterRow {
        story_id: String,
        table_id: String,
    },
    // ── W1.12b — merge / split spans ────────────────────────────────
    /// W1.12b — set a cell's `RowSpan` / `ColumnSpan`. Routes to
    /// `Operation::SetCellSpan`. // rides v35.
    SetCellSpan {
        story_id: String,
        table_id: String,
        row: u32,
        col: u32,
        row_span: u32,
        column_span: u32,
    },
    // ── S-03 — table CREATE (v37) ───────────────────────────────────
    /// S-03 — create a `<Table>` inside a story (the missing table-CREATE
    /// op; the row/column/band/span ops above all edit an EXISTING
    /// table). Routes to `Operation::InsertNode { parent:
    /// NodeId::Story(story_id), node: NodeSpec::Table { … } }`. The model
    /// mints the table's `Self` id and returns it as the
    /// `MutationOutcome::created_id` so the plugin can address cells next.
    /// `column_widths` / `row_heights` are pt; a short / empty vec leaves
    /// the trailing lines unsized. The table attaches on a fresh
    /// paragraph at the END of the story (see `apply_insert_table`).
    InsertTable {
        story_id: String,
        rows: u32,
        cols: u32,
        #[serde(default)]
        header_rows: u32,
        #[serde(default)]
        footer_rows: u32,
        #[serde(default)]
        column_widths: Vec<f32>,
        #[serde(default)]
        row_heights: Vec<f32>,
    },
    /// Delete a whole `<Table>` — the inverse of `InsertTable`, which
    /// the wire could create but not take away (a plugin replacing its
    /// placed table had to leave the old one behind). Routes to
    /// `Operation::RemoveNode { NodeId::Table }`, which removes the
    /// table's host paragraph and captures it whole, so undo puts back
    /// every cell where it was.
    DeleteTable {
        story_id: String,
        table_id: String,
    },
}

/// The wire op vocabulary, written once.
///
/// `discriminant` and [`MUTATION_NAMES`] come from this one list, so the
/// enum's names and the enumerable roster of them cannot drift. Before
/// it, `discriminant` was the only place the 117 ops were written down
/// and it was not enumerable — anything that needed the POPULATION (the
/// render sweep, the capability catalog, `state`'s op map) had to
/// re-derive it, by scraping this function's match arms out of the
/// source text or by hand-copying them into another repo. Both happened.
#[cfg(feature = "mutations")]
macro_rules! mutation_vocabulary {
    ($($variant:ident,)*) => {
        impl Mutation {
            /// Short string discriminant for logging + `NotImplemented`
            /// errors. PascalCase, matching the variant; the WIRE spells
            /// it camelCase — see [`Mutation::wire_tag`].
            pub fn discriminant(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => stringify!($variant),)*
                }
            }
        }

        /// Every wire op, PascalCase. The population `discriminant`
        /// names one at a time.
        pub const MUTATION_NAMES: &[&str] = &[$(stringify!($variant),)*];
    };
}

#[cfg(feature = "mutations")]
mutation_vocabulary! {
    InsertText,
    DeleteRange,
    ApplyStyle,
    InsertField,
    InsertAnchoredFrame,
    InsertHyperlink,
    InsertFootnote,
    DeleteFootnote,
    DeleteHyperlink,
    EditHyperlink,
    UpdateTableOfContents,
    GenerateIndex,
    UpdateCrossReferences,
    InsertNote,
    EditNote,
    DeleteNote,
    ConvertNoteToText,
    CreateBookmark,
    DeleteBookmark,
    RenameBookmark,
    MoveBookmark,
    InsertEndnote,
    DeleteEndnote,
    ConvertFootnoteToEndnote,
    SetFieldValue,
    PlaceImage,
    ReplaceImageBytes,
    MoveFrame,
    ResizeFrame,
    LinkFrames,
    UnlinkFrames,
    InsertPage,
    DeletePage,
    ResizePage,
    InsertFrame,
    InsertTextFrame,
    DeleteFrame,
    InsertLine,
    InsertPath,
    SetDocumentDefaults,
    SetColorSettings,
    SetProofSetup,
    ImportSwatchLibrary,
    SetInkSetting,
    SetUseStandardLabForSpots,
    PathPointInsert,
    PathPointRemove,
    PathOpenAt,
    OutlineStroke,
    OffsetPath,
    SimplifyPath,
    ClosePath,
    JoinPaths,
    PasteInto,
    ReleaseFrom,
    ReorderElement,
    ApplyOpacityMask,
    ReleaseOpacityMask,
    AttachTextToPath,
    DetachTextFromPath,
    CreateGroup,
    DissolveGroup,
    DuplicateElements,
    SetGroupTransform,
    SetPluginMetadata,
    SetPageMetadata,
    SetDocumentMetadata,
    CreateTextVariable,
    DeleteTextVariable,
    PathPointCurveType,
    PathPointSet,
    Batch,
    BindCreated,
    LayerSetVisible,
    LayerSetLocked,
    LayerSetPrintable,
    LayerSetName,
    LayerMove,
    LayerInsert,
    LayerRemove,
    SetElementProperty,
    Set,
    PathfinderBoolean,
    PathfinderDivide,
    PathfinderTrim,
    PathfinderMerge,
    PathfinderCrop,
    PathfinderOutline,
    PathfinderMinusBack,
    PathfinderFaces,
    CreateSwatch,
    EditSwatch,
    DeleteSwatch,
    CreateGradient,
    EditGradient,
    DeleteGradient,
    CreateColorGroup,
    EditColorGroup,
    DeleteColorGroup,
    CreateNumberingList,
    EditNumberingList,
    DeleteNumberingList,
    CreateParagraphStyle,
    RenameParagraphStyle,
    DeleteParagraphStyle,
    CreateCharacterStyle,
    RenameCharacterStyle,
    DeleteCharacterStyle,
    CreateObjectStyle,
    RenameObjectStyle,
    DeleteObjectStyle,
    CreateCellStyle,
    RenameCellStyle,
    DeleteCellStyle,
    CreateTableStyle,
    RenameTableStyle,
    DeleteTableStyle,
    SetStyleProperty,
    InsertOval,
    InsertGuide,
    MoveGuide,
    DeleteGuide,
    SetConditionVisible,
    SetFlowGrowRule,
    ActivateConditionSet,
    ApplyMasterToPage,
    DuplicatePage,
    MovePage,
    OnMaster,
    CreateMaster,
    DeleteMaster,
    RenameMaster,
    InsertSection,
    EditSection,
    DeleteSection,
    SetRowHeight,
    SetColumnWidth,
    InsertTableRow,
    DeleteTableRow,
    InsertTableColumn,
    DeleteTableColumn,
    InsertHeaderRow,
    RemoveHeaderRow,
    InsertFooterRow,
    RemoveFooterRow,
    SetCellSpan,
    InsertTable,
    DeleteTable,
}

#[cfg(feature = "mutations")]
impl Mutation {
    /// The op's tag as it travels on the wire — what `serde` writes for
    /// `#[serde(tag = "op", rename_all = "camelCase")]`, and therefore
    /// the spelling every surface addresses it by. `wire_tag_of` is the
    /// same rule applied to a name from [`MUTATION_NAMES`].
    pub fn wire_tag(&self) -> String {
        wire_tag_of(self.discriminant())
    }
}

/// PascalCase → camelCase, serde's rule for a variant name: lowercase
/// the first character and leave the rest alone. Pinned against real
/// serde output by `wire_tags_are_what_serde_writes`.
pub fn wire_tag_of(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
