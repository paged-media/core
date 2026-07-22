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

//! `Operation` — the single typed primitive every committed mutation
//! flows through. The five variants match the scripting-layer briefing
//! (`docs/paged/scripting-layer.md`): `SetProperty`, `InsertNode`,
//! `RemoveNode`, `MoveNode`, `Batch`. Extensions require deliberation.
//!
//! Every Operation is `Serialize`/`Deserialize` so the same value can
//! cross the WASM/JS boundary, persist into an operation log, or
//! travel over a wire for future collaboration without changing shape.
//!
//! Note on `Value`: this is the *wire-format payload of a `SetProperty`
//! Op*, not the scene-graph `Value<T>` literal-or-binding scaffold in
//! [`paged_scene::Value`]. The two compose — a SetProperty whose value
//! is a `Computed { ... }` binding will encode that intent here and
//! the scene-graph property cell will lift it into its `Value<T>`
//! variant at apply time. For Stage 1 only literal values exist.

use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

/// Serde helper for `Option<Option<T>>` "tri-state" fields: a present
/// field (including `null`) deserialises to `Some(inner)`; an absent
/// field deserialises to `None`. Plain `#[serde(default)]` collapses a
/// `null` into the outer `None`, which would lose the "set to None"
/// signal `EditSection` relies on. Used with `default` so an omitted
/// key still yields the outer `None`.
mod double_option {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        // The field is present (serde only calls this when the key
        // exists), so wrap the inner `Option<T>` in `Some`.
        Deserialize::deserialize(deserializer).map(Some)
    }
}

/// Stable identifier for a scene-graph node. The string payload is the
/// IDML `Self` attribute (e.g. `"TextFrame/u14"`) — stable for the
/// lifetime of the document. Operations reference nodes by ID, never
/// by path or index, so an Op generated on one client applies
/// meaningfully on another even after the tree has shuffled.
///
/// Variants today cover the page-item kinds the inspector mutates plus
/// the structural containers an `InsertNode`/`MoveNode` Op can target
/// as a parent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(tag = "kind", content = "id")]
pub enum NodeId {
    // Page items.
    TextFrame(String),
    Rectangle(String),
    Oval(String),
    Polygon(String),
    GraphicLine(String),
    Group(String),
    // Structural parents — addressable so InsertNode / MoveNode can
    // name where a node lands.
    Spread(String),
    Page(String),
    /// S-03 — a `<Story>` addressed by its IDML `Self`. The only
    /// structural parent an `InsertNode { node: NodeSpec::Table }`
    /// targets: a table lives INSIDE a story (it hangs off
    /// `Paragraph::table`), not on a spread. Unlike the page-item
    /// kinds, a story is never itself inserted/removed by these ops —
    /// it is purely the create-target container for `NodeSpec::Table`.
    Story(String),
    /// Track M — `<Layer>` defined in the `designmap.xml`. The
    /// associated `String` is the layer's IDML `Self` id.
    Layer(String),
    /// SDK Phase 3 — a half-open `[start, end)` character range inside
    /// a Story. The address Character / Paragraph `PropertyPath`s
    /// operate against: a `SetProperty { node: StoryRange, path:
    /// CharacterFontSize, value: Length(Some(12.0)) }` writes 12pt
    /// to every `CharacterRun` covered by the range, splitting runs
    /// at the boundaries when needed. Offsets are character indices
    /// in the story (IDML's native convention — matches the
    /// `<CharacterStyleRange>` / `<ParagraphStyleRange>` serialization).
    /// Paragraph paths round the addressed range to paragraph
    /// boundaries (paragraphs are atomic in IDML) before applying.
    StoryRange {
        story_id: String,
        start: u32,
        end: u32,
    },
    /// W3.A1 — a `<Table>` nested inside a story paragraph. Tables are
    /// NOT in the story's character/run space (they hang off
    /// `Paragraph::table`), so they're addressed by `(story_id,
    /// table_id)` rather than a story offset. `table_id` is the table's
    /// IDML `Self`. The `Table*` `PropertyPath`s (e.g.
    /// `AppliedTableStyle`) and the table-structure Operations
    /// (`SetRowHeight`, `InsertTableRow`, …) target this variant.
    Table {
        story_id: String,
        table_id: String,
    },
    /// W3.A1 — one cell of a table, addressed by its zero-indexed
    /// `(row, col)` (IDML serialises cell `Name="col:row"`; we expose
    /// the row-major `(row, col)` order designers think in). Cell-scoped
    /// scalar `PropertyPath`s (`CellFillColor`, `CellInsets*`,
    /// `CellVerticalJustification`, `AppliedCellStyle`) write here — the
    /// index rides the NodeId so the fieldless `PropertyPath` enum stays
    /// payload-free.
    TableCell {
        story_id: String,
        table_id: String,
        row: u32,
        col: u32,
    },
}

impl NodeId {
    /// Returns the IDML `Self` string identifying the **container**
    /// of this node — the story id for `StoryRange`, the page-item
    /// or layer self_id otherwise. Range bounds are carried as
    /// metadata on the variant itself; callers needing them should
    /// match on the variant.
    pub fn self_id(&self) -> &str {
        match self {
            NodeId::TextFrame(s)
            | NodeId::Rectangle(s)
            | NodeId::Oval(s)
            | NodeId::Polygon(s)
            | NodeId::GraphicLine(s)
            | NodeId::Group(s)
            | NodeId::Spread(s)
            | NodeId::Page(s)
            | NodeId::Story(s)
            | NodeId::Layer(s) => s,
            NodeId::StoryRange { story_id, .. } => story_id,
            // The story is the container that owns the table; table /
            // cell ids are carried as metadata on the variant.
            NodeId::Table { story_id, .. } | NodeId::TableCell { story_id, .. } => story_id,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            NodeId::TextFrame(_) => "TextFrame",
            NodeId::Rectangle(_) => "Rectangle",
            NodeId::Oval(_) => "Oval",
            NodeId::Polygon(_) => "Polygon",
            NodeId::GraphicLine(_) => "GraphicLine",
            NodeId::Group(_) => "Group",
            NodeId::Spread(_) => "Spread",
            NodeId::Page(_) => "Page",
            NodeId::Story(_) => "Story",
            NodeId::Layer(_) => "Layer",
            NodeId::StoryRange { .. } => "StoryRange",
            NodeId::Table { .. } => "Table",
            NodeId::TableCell { .. } => "TableCell",
        }
    }
}

/// Typed property path for `SetProperty` Ops. A closed enum (rather
/// than free-form `Vec<String>`) preserves Rust's exhaustiveness
/// guarantee inside `apply`/`invert`, and the `serde` rename lets the
/// wire format read like the dotted path the briefing illustrates
/// (`"fill.color"`) — so JS callers don't need to learn the Rust
/// enum shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum PropertyPath {
    /// Frame geometric bounds: `[top, left, bottom, right]`.
    FrameBounds,
    /// Frame fill-colour reference (a swatch self_id, e.g.
    /// `"Color/Red"`). `None` ⇒ no fill.
    FrameFillColor,
    /// Frame stroke-colour reference (analogous to fill).
    FrameStrokeColor,
    /// Frame stroke weight in points. `None` ⇒ inherit document default
    /// (typically 1pt). Setting to a non-None value pins the per-frame
    /// override.
    FrameStrokeWeight,
    /// Frame opacity percent (0..=100). `None` ⇒ inherit document
    /// default (100% fully opaque). Stored as a plain `f32` in
    /// `Length`-tagged `Value` since IDML carries the value in `%`
    /// units already.
    FrameOpacity,
    /// Phase D — frame `ItemTransform` (2D affine `[a, b, c, d, tx, ty]`).
    /// The IDML wire shape is the same matrix; the renderer applies it
    /// to the frame's content-box coordinates. Phase D's rotate, scale,
    /// and rotated-frame translate gestures all commit through this
    /// path.
    FrameTransform,
    /// Phase F — Rectangle's inner image transform (the `ItemTransform`
    /// on the nested `<Image>` element). Maps the image's pixel-grid
    /// origin into the frame's inner coordinate system. The
    /// content-grabber gesture edits this matrix to translate / scale
    /// the placed image inside an unchanged frame.
    ImageContentTransform,
    /// Phase H — one Bezier control point on a `Polygon`'s
    /// `PathPointArray`. Addressed via `PathPointAddress { index,
    /// role }` carried in the `Value::PathPoint` payload. The role
    /// picks between the anchor and its two direction handles.
    FramePathPoint,
    /// Track J — insert a new `PathAnchor` into a `Polygon`'s
    /// `PathPointArray` at the given flat index. Value carries the
    /// anchor to insert; apply also updates `subpath_starts` so
    /// any entry at or past the insert index shifts +1. Inverse is
    /// `PathPointRemove` at the same index.
    PathPointInsert,
    /// Track J — remove the `PathAnchor` at the given flat index
    /// from a `Polygon`'s `PathPointArray`. Apply captures the
    /// removed anchor into the returned `PathPointInsert` inverse
    /// and updates `subpath_starts` so any entry past the remove
    /// index shifts -1.
    PathPointRemove,
    /// Track J — toggle a `PathAnchor` between corner (handles
    /// equal to anchor) and smooth (handles derived from the
    /// neighbouring segments' tangents, 1/3-distance heuristic).
    /// Inverse restores the previous `left` + `right` exactly so
    /// repeated toggles round-trip bytewise.
    PathPointCurveType,
    /// Track M — `<Layer Visible="true|false">` toggle. Applies to
    /// `NodeId::Layer(self_id)`; value is `Value::Bool`. The
    /// renderer's layer-visibility helper already honours
    /// `DesignMap.layers[i].visible` so the next rebuild paints
    /// items on a now-hidden layer through.
    LayerVisible,
    /// Track M — `<Layer Locked="...">` toggle. The renderer
    /// ignores this but the canvas's hit-tester gates selection
    /// on it (a locked layer's items become un-clickable).
    LayerLocked,
    /// Track M — `<Layer Printable="...">` toggle. Non-printable
    /// layers are skipped during rendering.
    LayerPrintable,
    /// Track M — `<Layer Name="...">` rename. Value is `Value::Text`.
    LayerName,
    /// SDK Phase 3 — character font size, in points, addressed against
    /// a `NodeId::StoryRange`. Value is `Value::Length(Some(_))`. The
    /// apply layer walks every `CharacterRun` covered by the range,
    /// splits runs at the boundaries where needed, and writes the
    /// new `point_size` per run. Inverse is a `Batch` of per-run
    /// restorations.
    CharacterFontSize,
    /// SDK Phase 3 — character leading (line-spacing) in points.
    /// `Value::Length(Some(_))` carries a positive number;
    /// `Value::Length(None)` represents "Auto" (IDML's leading-from-
    /// applied-style fallback). Addressed against `NodeId::StoryRange`.
    CharacterLeading,
    /// SDK Phase 3 — character tracking (letter-spacing) in 1/1000 em.
    /// Value is `Value::Length`. Addressed against `NodeId::StoryRange`.
    CharacterTracking,
    /// SDK Phase 3 — character fill colour. Value is
    /// `Value::ColorRef(Some(swatch_id))` or `Value::ColorRef(None)`
    /// for "no fill". Addressed against `NodeId::StoryRange`.
    CharacterFillColor,
    /// SDK Phase 3 — paragraph space-before in points. Value is
    /// `Value::Length`. Addressed against `NodeId::StoryRange`;
    /// the apply layer rounds the range to paragraph boundaries
    /// (paragraphs are atomic — you can't half-apply space-before).
    ParagraphSpaceBefore,
    /// SDK Phase 3 — paragraph space-after in points. Same shape
    /// as SpaceBefore.
    ParagraphSpaceAfter,
    /// SDK Phase 3 — first-line indent in points. Same shape.
    ParagraphFirstLineIndent,
    /// SDK Phase 3 — applied paragraph style ref. Value is
    /// `Value::Text(String)` carrying the style's `self_id`
    /// (e.g. `"ParagraphStyle/$ID/Heading 1"`). Addressed against
    /// `NodeId::StoryRange`; the apply layer rounds the range to
    /// whole paragraphs (paragraphs are atomic) and sets each
    /// paragraph's `paragraph_style` reference. This is the
    /// `apply-an-entity` write per D3 of
    /// `docs/paged/panel-catalog-and-sdk-extension.md` — same
    /// binding kind as a scalar SetProperty, just a string-ref
    /// value.
    AppliedParagraphStyle,
    /// SDK Phase 3 — applied character style ref. Same shape as
    /// `AppliedParagraphStyle` but per-`CharacterRun` (with
    /// run-splitting for partial ranges).
    AppliedCharacterStyle,
    /// SDK Phase 5 (D3 completion) — applied object style ref. Value
    /// is `Value::Text(String)` carrying the style's `self_id`
    /// (e.g. `"ObjectStyle/$ID/Logo"`). Addressed against a page-item
    /// `NodeId` (TextFrame / Rectangle / Oval / Polygon / GraphicLine
    /// / Group). The page item's `applied_object_style` reference is
    /// rewritten; the renderer's style-cascade re-resolves on next
    /// rebuild. Inverse restores the previous reference.
    AppliedObjectStyle,
    /// SDK Phase 5 (D3 completion) — applied cell style ref. Wire-
    /// shape only for v1: the apply layer errors with
    /// `UnsupportedProperty` until the Table NodeId surface
    /// (Tier 2d) lands. Reserved so Cell Style panels can declare
    /// their write surface today and the audit pipeline picks them up.
    AppliedCellStyle,
    /// SDK Phase 5 (D3 completion) — applied table style ref. Same
    /// placeholder treatment as `AppliedCellStyle`: wire-shape only,
    /// apply layer errors until Tier 2d.
    AppliedTableStyle,
    /// SDK Phase 5 (v1 sweep) — whole-path replacement on any path-
    /// bearing page item. Value is `Value::FramePath { anchors,
    /// subpath_starts }`. The apply layer swaps the frame's anchor
    /// list wholesale; the inverse captures the prior anchors +
    /// subpath_starts so undo round-trips bytewise. Used by
    /// Pathfinder's Subtract / Exclude where the result is a fresh
    /// polygon set rather than a partial edit.
    FramePath,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting="true|false"` toggle on
    /// any page-item kind. `Value::Bool`. The renderer keeps the
    /// item visible on canvas but excludes it from print/export.
    FrameNonprinting,
    /// SDK Phase 5 (v1 sweep) — frame `FillTint` percent (0..=100).
    /// `Value::Length(Some(_))` carries the tint percentage;
    /// `Value::Length(None)` represents "no tint override" — the
    /// renderer uses the swatch at full strength. Tints scale the
    /// resolved colour toward paper white at composition time.
    FrameFillTint,
    /// Editor-ops (Gradient Swatch tool) — the gradient axis on a
    /// frame whose fill references a `Gradient/<id>` swatch. Angle in
    /// degrees (renderer convention: 0° = left→right, 90° =
    /// top→bottom); length in pt (`None` = renderer default — the
    /// bbox-derived axis). `Value::Length`. Carried on every
    /// path-bearing page-item kind; no-ops visually while the fill is
    /// a solid swatch.
    FrameGradientFillAngle,
    FrameGradientFillLength,
    /// Editor-ops — the stroke-gradient analogues.
    FrameGradientStrokeAngle,
    FrameGradientStrokeLength,
    /// Editor-ops (Scissors) — open/split the path at an anchor.
    /// `Value::PathOpenAt`; any path-bearing kind. See the Value
    /// variant for the cut semantics + the snapshot inverse.
    PathOpenAt,
    /// B-05 kernel op — replace the path with its stroke-expansion
    /// outline (`Value::OutlineStroke`). Geometry-only: paint
    /// transfer (fill := old stroke, stroke := none) is the
    /// CALLER's batch, keeping the op kind-generic.
    OutlineStroke,
    /// B-08 kernel op — replace an open path with a VARIABLE-width
    /// stroke outline (`Value::OutlineStrokeVariable`), tapering across
    /// per-anchor width stops (the pressure profile the editor's Pen
    /// captures). Same geometry-only / caller-batch-paint convention as
    /// `OutlineStroke`.
    OutlineStrokeVariable,
    /// B-05 kernel op — replace a single closed contour with its
    /// inset/outset (`Value::OffsetPath`).
    OffsetPath,
    /// B-05 kernel op — re-express the path within a max-deviation
    /// tolerance with fewer anchors (`Value::SimplifyPath`).
    SimplifyPath,
    /// Editor-ops — whole gradient-feather replacement on an
    /// effect-bearing page item (`Value::GradientFeather`). One path
    /// for the whole struct — kind + axis + the stop LIST edit
    /// together, and per-field shapes can't carry a list.
    FrameGradientFeather,
    /// Editor-ops (Page tool) — a page's `GeometricBounds`
    /// `[top, left, bottom, right]` in the page's INNER coordinate
    /// system (`Value::Bounds`). Only `NodeId::Page` carries it.
    /// Items keep their coordinates (InDesign's layout-adjust off);
    /// `spread_origin` re-derives on rebuild.
    PageBounds,
    /// SDK Phase 5 (v1 sweep) — drop-shadow per-field editors.
    /// All five operate on the frame's `drop_shadow:
    /// Option<DropShadowSetting>`. Writing to any of them
    /// materialises a default DropShadowSetting if the prior
    /// was `None`, then sets the named field. Use
    /// `FrameDropShadow` (the boolean toggle, defined below) to
    /// fully clear the shadow.
    ///
    /// `FrameDropShadowMode` carries the IDML mode string
    /// ("Drop" / "Inner" / etc); the renderer only branches on
    /// "Drop" today, others fall back to it.
    FrameDropShadowMode,
    /// X offset in pt. Positive = right.
    FrameDropShadowXOffset,
    /// Y offset in pt. Positive = down.
    FrameDropShadowYOffset,
    /// Blur radius in pt.
    FrameDropShadowSize,
    /// Opacity percent (0..=100).
    FrameDropShadowOpacity,
    /// Shadow tint colour ref. `Value::ColorRef`.
    FrameDropShadowColor,
    /// SDK Phase 5 (v1 sweep) — drop-shadow enabled toggle. Wire
    /// value is `Value::Bool`. Setting `true` materialises a
    /// default `DropShadowSetting` (mode="Drop", small offset, low
    /// opacity) on the frame; setting `false` clears it. The
    /// renderer's transparency pipeline reads `drop_shadow` on the
    /// next rebuild.
    ///
    /// v1 collapses: the toggle is one bit, but
    /// `DropShadowSetting` carries six fields. Round-trip of an
    /// existing customised shadow through false→true loses the
    /// original mode/offsets/etc. — undo restores defaults rather
    /// than the prior state. A typed wire shape per-field
    /// (DropShadowOffset / DropShadowColor / DropShadowOpacity)
    /// lands when the Effects panel grows to expose them.
    FrameDropShadow,
    /// SDK Phase 5 (v1 sweep) — `<FrameFittingOption>` crops on a
    /// Rectangle hosting a placed image. Wire value is
    /// `Value::Bounds([top, left, bottom, right])` in pt — IDML's
    /// signed-from-frame-edge convention; negative numbers grow the
    /// image outside the frame (typical of FillProportionally fits).
    /// Only `NodeId::Rectangle` carries the field; other kinds
    /// raise `UnsupportedProperty`. The apply layer treats the
    /// Bounds as four separate crops, materialising a FrameFitting
    /// when the prior was `None`.
    FrameFittingCrops,
    /// SDK Phase 5 (v1 sweep) — `<FrameFittingOption
    /// FittingOnEmptyFrame="…">` enum. Wire value is `Value::Text`
    /// carrying the IDML attribute string (`"None"`,
    /// `"Proportionally"`, `"FillProportionally"`, `"FitContent"`,
    /// `"FitContentToFrame"`, `"ContentAwareFit"`). The renderer
    /// currently doesn't branch on this attribute (the crops alone
    /// drive placement); keeping the wire shape so the Frame
    /// Fitting panel can declare it today. Empty string clears
    /// the override.
    FrameFittingType,
    /// SDK Phase 5 (v1 sweep) — `<TextWrapPreference Mode="…">` enum.
    /// Wire value is `Value::Text` carrying the IDML attribute string
    /// (`"None"`, `"BoundingBoxTextWrap"`, `"ContourTextWrap"`,
    /// `"JumpObjectTextWrap"`, `"NextColumnTextWrap"`). The apply arm
    /// reads the current `Option<TextWrap>`, replaces the `mode`
    /// (preserving `offsets`), and writes back; if the prior was
    /// `None` it creates a TextWrap with default `[0,0,0,0]` offsets.
    /// Empty string clears the override (`text_wrap = None`).
    FrameTextWrapMode,
    /// SDK Phase 5 (v1 sweep) — `<TextWrapPreference TextWrapOffset="…">`.
    /// Wire value is `Value::Bounds([top, left, bottom, right])` in
    /// pt. Same Option<TextWrap> handling as `FrameTextWrapMode` —
    /// preserves `mode` when set on a prior-None state by defaulting
    /// to `TextWrapMode::None`.
    FrameTextWrapOffsets,
    /// W2.5 — `<ContourOption ContourType="…">` for `ContourTextWrap`.
    /// Wire value is `Value::Text` carrying the IDML attribute string
    /// (`"SameAsClipping"`, `"GraphicFrame"`, `"DetectEdges"`,
    /// `"AlphaChannel"`, `"PhotoshopPath"`); empty string clears the
    /// contour source (`contour_type = None`). Same `Option<TextWrap>`
    /// materialise-on-None handling as `FrameTextWrapMode` (preserves
    /// `mode`/`offsets`/`invert`). Carried on every page-item kind with
    /// a `text_wrap` field. Other-frame reflow → structural rebuild.
    FrameTextWrapContourType,
    /// W2.5 — `<ContourOption IncludeInsideEdges="…">`. `Value::Bool`.
    /// `true` lets text flow into a contour's interior holes. Same
    /// `Option<TextWrap>` handling as `FrameTextWrapContourType`.
    /// `None`→default undo note like `CharacterUnderline` (the field is
    /// `Option<bool>`; undo of a write whose prior was `None` restores
    /// `Some(false)`).
    FrameTextWrapContourIncludeInside,
    /// SDK Phase 5 (v1 sweep) — paragraph alignment / justification.
    /// Wire value is `Value::Text` carrying the IDML attribute string
    /// (`"LeftAlign"`, `"CenterAlign"`, `"RightAlign"`,
    /// `"LeftJustified"`, `"CenterJustified"`, `"RightJustified"`,
    /// `"FullyJustified"`, `"ToBindingSide"`, `"AwayFromBindingSide"`)
    /// — the same shape `Justification::as_idml()` round-trips
    /// through. Addressed against a `NodeId::StoryRange`; the apply
    /// arm rounds the range to whole paragraphs (paragraphs are
    /// atomic in IDML). Unknown strings raise `InvalidValue`.
    ParagraphJustification,
    /// styles.next-style (W1.22) — a paragraph style's `NextStyle`
    /// reference. Wire value is `Value::Text(String)` carrying the
    /// next style's `self_id` (empty string clears the chain).
    /// STYLE-DEFINITION path only: addressed via `SetStyleProperty`
    /// against `StyleCollection::Paragraph`, NOT against a story-range
    /// node (NextStyle lives on the `<ParagraphStyle>` definition, not
    /// on a paragraph instance). The editor reads this off
    /// `ParagraphStyleSummary.next_style` and applies the chain at
    /// typing time (Enter at paragraph end). The renderer never acts
    /// on it. Additive — no consumer breakage, no bump on its own.
    ParagraphStyleNextStyle,
    /// W1.22 (engine gap 22) — a paragraph's `AppliedNumberingList`
    /// reference. Wire value is `Value::Text(String)` carrying the
    /// `NumberingList/<id>` self id (empty string clears it).
    /// Addressed against a `NodeId::StoryRange`; the apply arm rounds
    /// the range to whole paragraphs (atomic) and rewrites each
    /// paragraph's `applied_numbering_list`. The renderer resolves it
    /// to find the list's cross-story continuity flag. Additive.
    ParagraphAppliedNumberingList,
    /// SDK Phase 5 (v1 sweep) — frame stroke end-cap. Wire value is
    /// `Value::Text` carrying the IDML enum string
    /// (`"ButtEndCap"`, `"RoundEndCap"`, `"ProjectingEndCap"`).
    /// Addressed against any page-item kind that carries stroke
    /// state; the renderer uses the field on next paint. Empty
    /// string clears the override.
    FrameStrokeEndCap,
    /// SDK Phase 5 (v1 sweep) — `<TextFramePreference InsetSpacing="…">`
    /// in pt as a `Value::Bounds([top, left, bottom, right])`. Only
    /// `NodeId::TextFrame` carries inset spacing (the field doesn't
    /// exist on other page-item kinds — IDML's text-frame options are
    /// genuinely text-frame-specific). `None` on the parse side means
    /// "inherit from the document default"; the apply arm always
    /// records the inverse with the prior `Option<[f32; 4]>` so undo
    /// round-trips bytewise. The renderer's text composer already
    /// honours `inset_spacing` on the next rebuild.
    FrameInsetSpacing,
    /// SDK Phase 5 (D3 completion) — applied conditions on a
    /// `NodeId::StoryRange`. Value is `Value::Text(String)` carrying
    /// a space-separated list of `<Condition>` `self_id`s — IDML's
    /// native `AppliedConditions` serialisation. The apply layer
    /// walks every `CharacterRun` covered by the range (splitting
    /// at boundaries like `AppliedCharacterStyle` does), sets each
    /// run's `applied_conditions`, and emits a per-run Batch inverse.
    /// Set semantics (de-duplication, add/remove of an individual
    /// id) are the caller's responsibility for v1; the value is
    /// written verbatim.
    AppliedConditions,
    /// W0.1 — character font family (`AppliedFont`). Value is
    /// `Value::Text`; the empty string clears the per-run override
    /// (`None` ⇒ inherit from the applied character / paragraph
    /// style cascade). Addressed against a `NodeId::StoryRange`;
    /// runs split at the range boundaries. Reflow-affecting (a new
    /// font remeasures every glyph), so the InvalidationHint targets
    /// the host text frame's reflow.
    CharacterFontFamily,
    /// W0.1 — character font style (`FontStyle`, e.g. `"Bold"`,
    /// `"Italic"`). `Value::Text`; empty clears the override.
    /// Reflow-affecting. Addressed against a `NodeId::StoryRange`.
    CharacterFontStyle,
    /// W0.1 — kerning method (`KerningMethod`). `Value::Text`
    /// carrying the IDML enum string (`"Metrics"`, `"Optical"`,
    /// `"None"`); empty clears the override. Reflow-affecting
    /// (kerning changes advances). Addressed against a
    /// `NodeId::StoryRange`. The value is stored verbatim — the
    /// toggle-group primitive ensures the UI never emits an
    /// unknown string.
    CharacterKerningMethod,
    /// W0.1 — capitalization (`Capitalization`). `Value::Text`
    /// carrying the IDML enum string (`"Normal"`, `"SmallCaps"`,
    /// `"AllCaps"`, `"CapToSmallCap"`); empty clears the override.
    /// Reflow-affecting (`AllCaps` shapes uppercased glyphs).
    /// Addressed against a `NodeId::StoryRange`.
    CharacterCase,
    /// W0.1 — position (`Position`). `Value::Text` carrying the
    /// IDML enum string (`"Normal"`, `"Superscript"`,
    /// `"Subscript"`, …); empty clears the override.
    /// Reflow-affecting (super/subscript scale glyphs and shift the
    /// baseline). Addressed against a `NodeId::StoryRange`.
    CharacterPosition,
    /// W0.1 — applied language (`AppliedLanguage`). `Value::Text`
    /// carrying the IDML language reference; empty clears the
    /// override. Paint/reflow-neutral today (no renderer behaviour
    /// is keyed off it yet) — the InvalidationHint targets reflow so
    /// the host frame rebuilds when hyphenation eventually honours
    /// it. Addressed against a `NodeId::StoryRange`.
    CharacterLanguage,
    /// W0.1 — baseline shift (`BaselineShift`) in points.
    /// `Value::Length(Some(_))` lifts (positive) / drops (negative)
    /// the glyphs; `Value::Length(None)` clears the override.
    /// Reflow-affecting (shifted glyphs change the line's ink
    /// bounds). Addressed against a `NodeId::StoryRange`.
    CharacterBaselineShift,
    /// W0.1 — horizontal glyph scale (`HorizontalScale`) as a
    /// percentage (100 = identity). `Value::Length`; `None` clears
    /// the override. Reflow-affecting (the x-scale changes
    /// advances). Addressed against a `NodeId::StoryRange`.
    CharacterHorizontalScale,
    /// W0.1 — vertical glyph scale (`VerticalScale`) as a
    /// percentage (100 = identity). `Value::Length`; `None` clears
    /// the override. Reflow-affecting (the y-scale changes the
    /// line's ink bounds). Addressed against a `NodeId::StoryRange`.
    CharacterVerticalScale,
    /// W0.1 — glyph skew (`Skew`) in degrees (positive =
    /// right-leaning). `Value::Length`; `None` clears the override.
    /// Reflow-affecting (the shear changes glyph extents).
    /// Addressed against a `NodeId::StoryRange`.
    CharacterSkew,
    /// W0.1 — underline toggle (`Underline`). `Value::Bool`.
    /// Paint-only (an underline decoration doesn't reflow text), so
    /// the InvalidationHint targets the host frame's style/paint.
    /// Addressed against a `NodeId::StoryRange`.
    ///
    /// Round-trip note: the run field is `Option<bool>` (`None` ⇒
    /// inherit). `Value::Bool` carries no `None`, so undo of a write
    /// whose prior was `None` restores `Some(false)` (the underline
    /// default) rather than `None`. Writes over an explicit prior
    /// round-trip bytewise. Same lossy-default precedent as
    /// `FrameDropShadow`.
    CharacterUnderline,
    /// W0.1 — strikethrough toggle (`StrikeThru`). `Value::Bool`.
    /// Paint-only, like `CharacterUnderline`. Addressed against a
    /// `NodeId::StoryRange`. Same `None`→default undo note as
    /// `CharacterUnderline`.
    CharacterStrikethru,
    /// W0.1 — ligatures toggle (`Ligatures`, the `ligatures_on`
    /// field). `Value::Bool`. Reflow-affecting (toggling ligature
    /// substitution changes glyph runs and advances). Addressed
    /// against a `NodeId::StoryRange`. Same `None`→default undo note
    /// as `CharacterUnderline` (the ligatures default is `true`).
    CharacterLigatures,
    /// W0.1 — OpenType feature tags as an opaque, space-separated
    /// list (e.g. `"frac ordn ss01"`). `Value::Text`; empty clears
    /// the override. IDML has no single tag-list attribute, so the
    /// value is owned by the mutate API as a free-form authoring
    /// string and written verbatim onto the run's `otf_features`.
    /// Reflow-affecting (feature substitution changes glyph runs).
    /// Addressed against a `NodeId::StoryRange`.
    CharacterOtfFeatures,
    /// W0.2 — paragraph left indent (`LeftIndent`) in points.
    /// `Value::Length`; `None` clears the per-paragraph override
    /// (inherit from the style cascade). Addressed against a
    /// `NodeId::StoryRange`, rounded to whole paragraphs.
    /// Reflow-affecting (the indent reshapes every line).
    ParagraphLeftIndent,
    /// W0.2 — paragraph right indent (`RightIndent`) in points.
    /// `Value::Length`; `None` clears the override. Reflow-affecting.
    ParagraphRightIndent,
    /// W0.2 — drop-cap character count (`DropCapCharacters`). The
    /// run field is a `u32`; the wire carries it as
    /// `Value::Length(Some(count))` (the integer-as-Length convention
    /// the inspector already uses for counts). `Length(None)` ⇒ 0
    /// (no drop cap). Reflow-affecting (the drop cap reflows the
    /// first lines). Addressed against a `NodeId::StoryRange`.
    ParagraphDropCapCharacters,
    /// W0.2 — drop-cap line span (`DropCapLines`). `Value::Length`
    /// carrying the integer line count; `None` ⇒ 0. Reflow-affecting.
    ParagraphDropCapLines,
    /// W0.2 — hyphenation toggle (`Hyphenation`). `Value::Bool`.
    /// Reflow-affecting (toggling hyphenation re-breaks lines).
    /// Addressed against a `NodeId::StoryRange`.
    ///
    /// Round-trip note: the field is `Option<bool>` (`None` ⇒
    /// inherit). `Value::Bool` carries no `None`, so undo of a write
    /// whose prior was `None` restores `Some(true)` (the IDML
    /// hyphenation default) rather than `None`. Writes over an
    /// explicit prior round-trip bytewise.
    ParagraphHyphenation,
    /// W0.2 — keep-lines-together toggle (`KeepLinesTogether`).
    /// `Value::Bool`. Reflow-affecting (changes column / frame
    /// breaking). Same `None`→default undo note as
    /// `ParagraphHyphenation`, but the keep-lines default is `false`.
    ParagraphKeepLinesTogether,
    /// W0.2 — keep-with-next line count (`KeepWithNext`). IDML
    /// serialises a line count, not a boolean, so the wire carries
    /// `Value::Length(Some(count))`; `Length(None)` clears the
    /// override. Reflow-affecting. Addressed against a
    /// `NodeId::StoryRange`.
    ParagraphKeepWithNext,
    /// W0.2 — whole `RuleAbove*` rule struct, mirroring the
    /// `FrameGradientFeather` whole-struct pattern. Value is
    /// `Value::ParagraphRule(Some(spec))` to set, or
    /// `Value::ParagraphRule(None)` to clear the rule back to the
    /// all-`None` default. Reflow-neutral but repaints the frame —
    /// the InvalidationHint targets the host frame's reflow (the rule
    /// can change line geometry via its offset). Addressed against a
    /// `NodeId::StoryRange`.
    ParagraphRuleAbove,
    /// W0.2 — whole `RuleBelow*` rule struct. See
    /// `ParagraphRuleAbove`.
    ParagraphRuleBelow,
    /// W0.2 — whole `<TabList>` replacement. Value is
    /// `Value::TabStops(Vec<TabStopSpec>)` (the empty vec clears all
    /// stops). Whole-list replacement, like the gradient-feather stop
    /// list — `Value` has no per-element list-edit form, so the UI
    /// sends the full new stop list. Reflow-affecting (tab stops
    /// reposition tabbed content). Addressed against a
    /// `NodeId::StoryRange`.
    ParagraphTabStops,
    /// W0.2 — bullets / numbering list type
    /// (`BulletsAndNumberingListType`). `Value::Text` carrying the
    /// IDML enum string (`"NoList"`, `"BulletList"`,
    /// `"NumberedList"`); empty clears the override. Reflow-affecting
    /// (a marker inserts / removes leading content). Addressed
    /// against a `NodeId::StoryRange`.
    ParagraphListType,
    /// W0.2 — bullet glyph character. `Value::Text` carrying the
    /// glyph itself (the run field is a `u32` codepoint; the wire
    /// carries the single character). Empty clears the override.
    /// Reflow-affecting. Addressed against a `NodeId::StoryRange`.
    ParagraphBulletCharacter,
    /// W0.2 — numbering-format expression (`NumberingFormat`, e.g.
    /// `"^#.^t"`). `Value::Text`; empty clears the override.
    /// Reflow-affecting (the marker text changes). Addressed against
    /// a `NodeId::StoryRange`.
    ParagraphNumberingFormat,

    // ---- W0.3 — text-frame prefs --------------------------------
    /// W0.3 — `<TextFramePreference TextColumnCount="...">`. The run
    /// field is a `u32`; the wire carries it as
    /// `Value::Length(Some(count))` (integer-as-Length, like the
    /// drop-cap counts). `Length(None)` clears the per-frame override.
    /// Only `NodeId::TextFrame` carries it. Reflow-affecting (column
    /// split reshapes the text). The composer's per-column layout is a
    /// later wave; the field is wired for authoring + round-trip.
    TextFrameColumnCount,
    /// W0.3 — `<TextFramePreference TextColumnGutter="...">` in pt.
    /// `Value::Length`; `None` clears the override. TextFrame-only.
    /// Reflow-affecting.
    TextFrameColumnGutter,
    /// W0.3 — `<TextFramePreference VerticalBalanceColumns="...">`.
    /// `Value::Bool`. TextFrame-only. Reflow-affecting (balancing
    /// redistributes the last lines). `None`→default undo note like
    /// `CharacterUnderline` (the balance default is `false`).
    TextFrameColumnBalance,
    /// W0.3 — `<TextFramePreference VerticalJustification="...">` enum.
    /// `Value::Text` carrying the IDML attribute string (`"TopAlign"`,
    /// `"CenterAlign"`, `"BottomAlign"`, `"JustifyAlign"`); empty
    /// clears the override. TextFrame-only. Reflow-affecting (vertical
    /// distribution shifts every line). Unknown strings clear (parse
    /// `from_idml` returns `None`).
    TextFrameVerticalJustification,
    /// W0.3 — `<TextFramePreference AutoSizingType="...">` enum.
    /// `Value::Text` carrying the IDML attribute string (`"Off"`,
    /// `"HeightOnly"`, `"WidthOnly"`, `"HeightAndWidth"`,
    /// `"HeightAndWidthProportionally"`); empty clears the override.
    /// TextFrame-only. Reflow-affecting (auto-grow changes bounds).
    TextFrameAutoSizing,
    /// W0.3 — `<TextFramePreference FirstBaselineOffset="...">` enum.
    /// `Value::Text` carrying the IDML attribute string (`"AscentOffset"`,
    /// `"CapHeight"`, `"XHeight"`, `"EmBoxHeight"`, `"LeadingOffset"`,
    /// `"FixedHeight"`); empty clears the override. TextFrame-only.
    /// Reflow-affecting (the first line's baseline moves).
    TextFrameFirstBaseline,

    // ---- W0.3 — text wrap ---------------------------------------
    /// W0.3 — `<TextWrapPreference Inverse="...">`. `Value::Bool`.
    /// Carried on every page-item kind with a `text_wrap` field
    /// (TextFrame / Rectangle / Oval / Polygon / GraphicLine). Writing
    /// materialises a default `TextWrap` (mode=None, offsets=[0;4]) if
    /// the prior was `None`. Text-reflow-affecting on *other* frames
    /// (the wrap exclusion changes), so the InvalidationHint is a
    /// structural rebuild rather than a single-frame repaint.
    /// `None`→default undo note like `CharacterUnderline`.
    TextWrapInvert,

    // ---- W0.3 — frame fitting -----------------------------------
    /// W0.3 — `<FrameFittingOption FittingAlignment="...">` enum.
    /// `Value::Text` carrying the IDML reference-point string
    /// (`"TopLeftPoint"`, `"CenterPoint"`, …); empty clears the
    /// override. `NodeId::Rectangle` only (the kind that hosts placed
    /// images). Materialises a `FrameFittingOption` when the prior was
    /// `None`. Paint-only re-fit on the next rebuild → `frame_style`.
    FrameFittingReferencePoint,
    /// W0.3 — `<FrameFittingOption AutoFit="...">`. `Value::Bool`.
    /// Rectangle-only. Same materialise-on-None handling as
    /// `FrameFittingReferencePoint`. Informational until the live-fit
    /// pass lands; `frame_style` invalidation. `None`→default undo.
    FrameAutoFit,

    // ---- W0.3 — stroke ------------------------------------------
    /// W0.3 — `StrokeType` reference (`"StrokeStyle/$ID/Solid"`,
    /// `"…/Dashed"`, `"…/Dotted"`, `"…/Canned Dotted"`, custom names).
    /// `Value::Text`; empty clears the override. Carried on every
    /// stroked page-item kind. Paint-only (`frame_style`).
    FrameStrokeType,
    /// W0.3 — `EndJoin` (`"MiterEndJoin"`, `"RoundEndJoin"`,
    /// `"BevelEndJoin"`). `Value::Text`; empty clears. Rectangle-only
    /// (the kind that parses `end_join`). Paint-only.
    FrameStrokeJoin,
    /// W0.3 — `MiterLimit` (multiple of stroke width, default 4.0).
    /// `Value::Length`; `None` clears. Rectangle-only. Paint-only.
    FrameStrokeMiterLimit,
    /// W0.3 — `StrokeAlignment` (`"CenterAlignment"`,
    /// `"InsideAlignment"`, `"OutsideAlignment"`). `Value::Text`;
    /// empty clears. Rectangle-only. Paint-only (the renderer
    /// inset/outsets by half the weight on rebuild).
    FrameStrokeAlignment,
    /// W0.3 — `GapColor` reference for dashed-stroke gaps.
    /// `Value::ColorRef`. Carried on every stroked page-item kind.
    /// Paint-only.
    FrameStrokeGapColor,
    /// W0.3 — `GapTint` percent (0..=100) for the gap colour.
    /// `Value::Length`; `None` clears. Stroked kinds. Paint-only.
    FrameStrokeGapTint,
    /// W1.1 — per-frame `StrokeDashAndGap` override: the alternating
    /// on/off dash lengths in pt. `Value::Lengths(vec)`; the empty vec
    /// CLEARS the per-frame override so the stroke falls back to its
    /// `StrokeType` (`StrokeStyleDef` pattern or built-in name).
    /// Carried on every stroked page-item kind (parsed onto
    /// `CommonAttrs::stroke_dash`). Takes PRECEDENCE over the
    /// `StrokeStyleDef` pattern at paint time — the same instance-wins
    /// precedent as `FrameStrokeGapColor` (FINDING #7.5). Paint-only
    /// (`frame_style`); a dash change repaints but does not reflow.
    FrameStrokeDashArray,

    // ---- W0.3 — corners (Rectangle) -----------------------------
    /// W0.3 — per-corner `CornerOption` enum (`"None"`,
    /// `"RoundedCorner"`, `"InverseRoundedCorner"`, `"InsetCorner"`,
    /// `"BeveledCorner"`, `"FancyCorner"`). `Value::Text`; empty
    /// clears that corner's override. Rectangle-only; addresses one of
    /// the four entries in `corners[4]` (IDML order
    /// `[top_left, top_right, bottom_right, bottom_left]`). Paint-only
    /// (the renderer re-derives the rounded-rect path on rebuild).
    FrameCornerOptionTopLeft,
    FrameCornerOptionTopRight,
    FrameCornerOptionBottomLeft,
    FrameCornerOptionBottomRight,
    /// W0.3 — per-corner `CornerRadius` in pt. `Value::Length`;
    /// `None` clears that corner's radius. Rectangle-only; pairs with
    /// the matching `FrameCornerOption*`. Paint-only.
    FrameCornerRadiusTopLeft,
    FrameCornerRadiusTopRight,
    FrameCornerRadiusBottomLeft,
    FrameCornerRadiusBottomRight,

    // ---- W0.3 — transform decompose (gap 6/16) ------------------
    /// W0.3 — frame rotation angle in degrees, decomposed from the
    /// frame's `ItemTransform`. `Value::Length(Some(deg))`; `None`
    /// resets rotation to 0 while preserving scale + translation.
    /// Read decomposes the matrix; write recomposes
    /// `T · R(angle) · scale · flip` preserving the existing
    /// translation, scale, and flip. Carried on every page-item kind
    /// with an `item_transform`. Reflow-affecting (rotating a text
    /// frame re-lays its content) → `frame_geometry`. Shear is NOT
    /// represented — a sheared matrix decomposes lossily (see
    /// `decompose_transform`).
    FrameRotationAngle,
    /// W0.3 — horizontal scale factor (1.0 = identity), decomposed
    /// from `ItemTransform`. `Value::Length`; `None` resets to 1.0.
    /// Sign is carried by the flip paths, so the magnitude here is
    /// always non-negative. `frame_geometry`.
    FrameScaleX,
    /// W0.3 — vertical scale factor. See `FrameScaleX`.
    FrameScaleY,
    /// W0.3 — horizontal flip (mirror across the vertical axis).
    /// `Value::Bool`. Detected from the sign of the decomposed
    /// X-scale (equivalently the matrix determinant). Recompose
    /// negates the X-scale when set. `frame_geometry`.
    FrameFlipH,
    /// W0.3 — vertical flip (mirror across the horizontal axis).
    /// `Value::Bool`. See `FrameFlipH`.
    FrameFlipV,

    // ---- W0.3 — overprint ---------------------------------------
    /// W0.3 — `OverprintFill="true"`. `Value::Bool`. Carried on every
    /// page-item kind with a fill (`overprint_fill` field). Paint-only
    /// (`frame_style`).
    FrameOverprintFill,
    /// W0.3 — `OverprintStroke="true"`. `Value::Bool`. Every stroked
    /// page-item kind. Paint-only.
    FrameOverprintStroke,

    // ---- W0.4 — transparency effects (gap 18) -------------------
    // Per-field editors for the non-DropShadow effect blocks that
    // already parse onto `effects: Option<FrameEffects>` (each effect
    // is itself an `Option<…Params>` inside that bag). The recipe
    // mirrors the DropShadow per-field set: writing any field
    // materialises the effect block (and the parent `FrameEffects`)
    // with InDesign-preset defaults if the prior was `None`, then sets
    // the named field. Each effect also carries an `*Enabled` boolean
    // toggle whose semantics match `FrameDropShadow`: the *presence* of
    // the `Option<…Params>` is the enabled bit (the parser drops the
    // whole block when `Applied="false"`), so `true` materialises a
    // default block and `false` clears it. Wired on the effect-bearing
    // kinds (`TextFrame` / `Rectangle` / `Oval`); other kinds raise
    // `UnsupportedProperty`. All paint-only → `frame_style` (the
    // rasterizer's effect compositor reads them on the next rebuild;
    // none reflow). The `*Enabled` toggle is lossy on a customised
    // block round-tripped through false→true, same caveat as
    // `FrameDropShadow`.
    /// W0.4 — inner-shadow enabled toggle. `Value::Bool`. Materialises
    /// a default `InnerShadowParams` on `true`, clears on `false`.
    FrameInnerShadowEnabled,
    /// W0.4 — `<InnerShadowSetting BlendMode="…">`. `Value::Text`
    /// (IDML enum string, e.g. `"Multiply"`); empty clears.
    FrameInnerShadowBlendMode,
    /// W0.4 — `EffectColor` ref. `Value::ColorRef`.
    FrameInnerShadowColor,
    /// W0.4 — `Opacity` percent (0..=100). `Value::Length`.
    FrameInnerShadowOpacity,
    /// W0.4 — `Angle` in degrees. `Value::Length`.
    FrameInnerShadowAngle,
    /// W0.4 — `Distance` in pt. `Value::Length`.
    FrameInnerShadowDistance,
    /// W0.4 — `Size` (blur radius) in pt. `Value::Length`.
    FrameInnerShadowSize,
    /// W0.4 — `ChokeAmount` percent (the inner-shadow "spread"/choke).
    /// `Value::Length`.
    FrameInnerShadowChoke,
    /// W0.4 — `Noise` percent. `Value::Length`.
    FrameInnerShadowNoise,

    /// W0.4 — outer-glow enabled toggle. `Value::Bool`.
    FrameOuterGlowEnabled,
    /// W0.4 — `<OuterGlowSetting BlendMode="…">`. `Value::Text`.
    FrameOuterGlowBlendMode,
    /// W0.4 — `EffectColor` ref. `Value::ColorRef`.
    FrameOuterGlowColor,
    /// W0.4 — `Opacity` percent. `Value::Length`.
    FrameOuterGlowOpacity,
    /// W0.4 — `Spread` percent. `Value::Length`.
    FrameOuterGlowSpread,
    /// W0.4 — `Size` in pt. `Value::Length`.
    FrameOuterGlowSize,
    /// W0.4 — `Noise` percent. `Value::Length`.
    FrameOuterGlowNoise,

    /// W0.4 — inner-glow enabled toggle. `Value::Bool`.
    FrameInnerGlowEnabled,
    /// W0.4 — `<InnerGlowSetting BlendMode="…">`. `Value::Text`.
    FrameInnerGlowBlendMode,
    /// W0.4 — `EffectColor` ref. `Value::ColorRef`.
    FrameInnerGlowColor,
    /// W0.4 — `Opacity` percent. `Value::Length`.
    FrameInnerGlowOpacity,
    /// W0.4 — `ChokeAmount` percent. `Value::Length`.
    FrameInnerGlowChoke,
    /// W0.4 — `Size` in pt. `Value::Length`.
    FrameInnerGlowSize,
    /// W0.4 — `Source` (`"EdgeGlow"` / `"CenterGlow"`). `Value::Text`;
    /// empty clears.
    FrameInnerGlowSource,
    /// W0.4 — `Noise` percent. `Value::Length`.
    FrameInnerGlowNoise,

    /// W0.4 — bevel/emboss enabled toggle. `Value::Bool`.
    FrameBevelEnabled,
    /// W0.4 — `<BevelAndEmbossSetting Style="…">` (`"InnerBevel"`,
    /// `"OuterBevel"`, `"Emboss"`, `"PillowEmboss"`,
    /// `"StrokeEmboss"`). `Value::Text`; empty clears.
    FrameBevelStyle,
    /// W0.4 — `Technique` (`"Smooth"`, `"ChiselHard"`,
    /// `"ChiselSoft"`). `Value::Text`; empty clears.
    FrameBevelTechnique,
    /// W0.4 — `Depth` percent. `Value::Length`.
    FrameBevelDepth,
    /// W0.4 — `Direction` (`"Up"` / `"Down"`). `Value::Text`; empty
    /// clears.
    FrameBevelDirection,
    /// W0.4 — `Size` in pt. `Value::Length`.
    FrameBevelSize,
    /// W0.4 — `Soften` in pt. `Value::Length`.
    FrameBevelSoften,
    /// W0.4 — `Angle` in degrees. `Value::Length`.
    FrameBevelAngle,
    /// W0.4 — `Altitude` in degrees. `Value::Length`.
    FrameBevelAltitude,
    /// W0.4 — `HighlightColor` ref. `Value::ColorRef`.
    FrameBevelHighlightColor,
    /// W0.4 — `ShadowColor` ref. `Value::ColorRef`.
    FrameBevelShadowColor,
    /// W0.4 — `HighlightOpacity` percent. `Value::Length`.
    FrameBevelHighlightOpacity,
    /// W0.4 — `ShadowOpacity` percent. `Value::Length`.
    FrameBevelShadowOpacity,

    /// W0.4 — satin enabled toggle. `Value::Bool`.
    FrameSatinEnabled,
    /// W0.4 — `<SatinSetting BlendMode="…">`. `Value::Text`.
    FrameSatinBlendMode,
    /// W0.4 — `EffectColor` ref. `Value::ColorRef`.
    FrameSatinColor,
    /// W0.4 — `Opacity` percent. `Value::Length`.
    FrameSatinOpacity,
    /// W0.4 — `Angle` in degrees. `Value::Length`.
    FrameSatinAngle,
    /// W0.4 — `Distance` in pt. `Value::Length`.
    FrameSatinDistance,
    /// W0.4 — `Size` in pt. `Value::Length`.
    FrameSatinSize,
    /// W0.4 — `Invert` flag. `Value::Bool`.
    FrameSatinInvert,

    /// W0.4 — (basic) feather enabled toggle. `Value::Bool`.
    FrameFeatherEnabled,
    /// W0.4 — `<FeatherSetting Width="…">` in pt. `Value::Length`.
    FrameFeatherWidth,
    /// W0.4 — `CornerType` (`"Sharp"`, `"Rounded"`, `"Diffusion"`).
    /// `Value::Text`; empty clears.
    FrameFeatherCornerType,
    /// W0.4 — `Noise` percent. `Value::Length`.
    FrameFeatherNoise,
    /// W0.4 — `ChokeAmount` percent. `Value::Length`.
    FrameFeatherChoke,

    /// W0.4 — directional-feather enabled toggle. `Value::Bool`.
    FrameDirectionalFeatherEnabled,
    /// W0.4 — `LeftWidth` in pt. `Value::Length`.
    FrameDirectionalFeatherLeftWidth,
    /// W0.4 — `RightWidth` in pt. `Value::Length`.
    FrameDirectionalFeatherRightWidth,
    /// W0.4 — `TopWidth` in pt. `Value::Length`.
    FrameDirectionalFeatherTopWidth,
    /// W0.4 — `BottomWidth` in pt. `Value::Length`.
    FrameDirectionalFeatherBottomWidth,
    /// W0.4 — `Angle` in degrees. `Value::Length`.
    FrameDirectionalFeatherAngle,
    /// W0.4 — `NoiseAmount` percent. `Value::Length`.
    FrameDirectionalFeatherNoise,
    /// W0.4 — `ChokeAmount` percent. `Value::Length`.
    FrameDirectionalFeatherChoke,

    /// W0.4 — object-level transparency blend mode
    /// (`<BlendingSetting BlendMode="…">`). `Value::Text` carrying the
    /// IDML enum string (`"Normal"`, `"Multiply"`, `"Screen"`,
    /// `"Overlay"`, …); empty clears the override (`blend_mode = None`).
    /// Carried on every page-item kind with a `blend_mode` field
    /// (TextFrame / Rectangle). The rasterizer doesn't yet honour
    /// non-Normal modes; the field is wired for authoring + round-trip.
    /// Paint-only (`frame_style`). The companion `FrameOpacity` path
    /// (the `<BlendingSetting Opacity="…">` half) already exists.
    FrameBlendMode,

    // ---- W3.A0 — text-frame threading (READ-ONLY) ---------------
    // The thread chain is *authored* via the `LinkFrames` /
    // `UnlinkFrames` operations (which capture the prior link for
    // undo), NOT via `SetProperty`. These two paths exist only so the
    // inspector can *read* the chain as `PropertyEntry`s: they have no
    // arm in `apply_set_property`, so a `SetProperty` carrying either
    // falls through to the catch-all and is rejected with
    // `UnsupportedProperty` — the standard read-only contract.
    /// W3.A0 (read-only) — the `NextTextFrame` link target: the
    /// self id of the frame this one's overflow threads into, or an
    /// empty string when the frame ends its chain. `Value::Text`.
    /// Write via `Operation::LinkFrames` / `UnlinkFrames`, not
    /// `SetProperty` (which rejects this path).
    NextTextFrame,
    /// W3.A0 (read-only) — the previous frame in the thread: the
    /// self id of the frame whose `NextTextFrame` points at this one,
    /// or empty when this frame starts its chain. Derived by scanning
    /// the spread's frames. `Value::Text`. Read-only (see
    /// `NextTextFrame`).
    PreviousTextFrame,

    // ---- W3.A1 — table cell properties --------------------------
    // Cell-scoped scalar writes. Addressed against a
    // `NodeId::TableCell { story_id, table_id, row, col }` — the
    // (row, col) index rides the NodeId so these paths stay
    // payload-free like every other `PropertyPath`. The host story
    // reflows on any of these (cell geometry / fill can shift the
    // table), so the InvalidationHint targets the host frame's
    // text_reflow. `AppliedCellStyle` (defined above, the Tier-2d
    // placeholder) is the fifth cell-scoped path and now has a live
    // apply arm.
    /// W3.A1 — inline `<Cell FillColor="Color/…">`. `Value::ColorRef`;
    /// `None` clears the inline override (the cell-style cascade then
    /// supplies the fill).
    CellFillColor,
    /// W3.A1 — `<Cell FillTint="…">` percent (0..=100). IDML stores the
    /// tint inline only as part of the fill; we model it as the cell's
    /// fill tint. The parse side has no dedicated cell fill-tint field
    /// today, so v1 routes this through the same precedence as
    /// `CellFillColor` (see the apply arm). `Value::Length`; `None`
    /// clears.
    CellFillTint,
    /// W3.A1 — `<Cell TextTopInset="…">` in pt. `Value::Length(Some(_))`;
    /// `None` resets to 0 (IDML's default cell inset). Four separate
    /// paths because the parse models four independent inset fields
    /// (`text_{top,left,bottom,right}_inset`) — matching the
    /// per-corner `FrameCornerRadius*` precedent of one path per side.
    CellInsetTop,
    CellInsetLeft,
    CellInsetBottom,
    CellInsetRight,
    /// W3.A1 — `<Cell VerticalJustification="…">` enum. `Value::Text`
    /// carrying the IDML attribute string (`"TopAlign"`,
    /// `"CenterAlign"`, `"BottomAlign"`, `"JustifyAlign"`); empty
    /// clears the override. The parse side has no dedicated field yet
    /// (cell content uses Ascent semantics) — v1 stores the value on a
    /// new optional cell field for round-trip; the renderer honours it
    /// when the cell-vertical-justify pass lands. Reflow-affecting.
    CellVerticalJustification,

    // ---- W1.11b — per-cell edge strokes -------------------------
    // Per-cell boundary stroke overrides, addressed against a
    // `NodeId::TableCell`. IDML serialises each cell edge explicitly
    // (`Top/Bottom/Left/RightEdgeStroke{Color,Weight,Tint}`) even when
    // the AppliedCellStyle is `[None]`; without honouring these the
    // row/column dividers vanish. Four edges × three facets =
    // twelve paths, following the one-path-per-side `CellInset*`
    // precedent. Colour paths carry `Value::ColorRef` (`None` clears
    // the inline override → cascade), weight/tint carry `Value::Length`
    // (`None` clears). All ride v35 (additive PropertyPath variants on
    // the unpublished protocol — the `FrameStrokeDashArray` precedent).
    CellTopEdgeStrokeColor,
    CellTopEdgeStrokeWeight,
    CellTopEdgeStrokeTint,
    CellBottomEdgeStrokeColor,
    CellBottomEdgeStrokeWeight,
    CellBottomEdgeStrokeTint,
    CellLeftEdgeStrokeColor,
    CellLeftEdgeStrokeWeight,
    CellLeftEdgeStrokeTint,
    CellRightEdgeStrokeColor,
    CellRightEdgeStrokeWeight,
    CellRightEdgeStrokeTint,

    // ---- Aftercare-A — table dimensions (READ-ONLY) -------------
    // Like `NextTextFrame` / `PreviousTextFrame`, these exist only so
    // the inspector can *read* a table's shape as `PropertyEntry`s:
    // they have no arm in `apply_table_property` (which rejects every
    // path but `AppliedTableStyle` with `UnsupportedProperty`), so a
    // `SetProperty` carrying either is rejected — the standard
    // read-only contract. Structure edits go through the dedicated
    // `Insert/DeleteTableRow` / `Insert/DeleteTableColumn` Operations.
    /// Aftercare-A (read-only) — the table's total row count
    /// (header + body + footer rows). The wire carries it as
    /// `Value::Length(Some(count))` (the integer-as-`Length`
    /// convention the inspector uses for drop-cap counts). Addressed
    /// against a `NodeId::Table`. Read-only (see the section comment).
    TableRowCount,
    /// Aftercare-A (read-only) — the table's column count.
    /// `Value::Length(Some(count))`. Addressed against a
    /// `NodeId::Table`. Read-only (see the section comment).
    TableColumnCount,

    /// Plugin-metadata carrier (decision 9 facility) — one
    /// `Properties/Label` `KeyValuePair` on a leaf page item, in the
    /// reserved `x-paged:` key namespace. The payload (key + new
    /// value + prev snapshot) rides in `Value::PluginMetadata`;
    /// `value: None` deletes the entry. Write-gated at apply time:
    /// key prefix, 64 KiB cap, JSON envelope `{v, data, …}`.
    PluginMetadata,

    // ---- W1.16 — anchored-object settings -----------------------
    // The `<AnchoredObjectSetting>` block that lives on an inline /
    // anchored frame nested inside a story's `<CharacterStyleRange>`.
    // The frame is addressed by its OWN page-item `NodeId` (the
    // anchored TextFrame / Rectangle / Group's `Self` id) — the apply
    // arm locates its `AnchoredObjectSetting` by scanning the stories'
    // runs (and nested group children) rather than the spread page-item
    // vecs. All ten are kind-agnostic over the page-item NodeId
    // variants (like the Track J path ops). The setting changes the
    // anchored frame's placement, so the InvalidationHint targets
    // text_reflow (anchored placement reflows the host line). Writing
    // any of them materialises a default `AnchoredObjectSetting` when
    // the frame carried none.
    /// W1.16 — `AnchoredPosition` (`"InlinePosition"`, `"AbovePosition"`,
    /// `"Custom"`). `Value::Text`; empty clears the override (`None` ⇒
    /// the cascaded `InlinePosition` default).
    AnchoredPosition,
    /// W1.16 — `AnchorPoint` (`"TopLeftAnchor"`, `"CenterAnchor"`, …).
    /// `Value::Text`; empty clears the override.
    AnchorPoint,
    /// W1.16 — `AnchorXoffset` in pt (horizontal nudge from the anchor
    /// point). `Value::Length(Some(_))`; `Length(None)` resets to 0
    /// (the IDML default offset).
    AnchoredXOffset,
    /// W1.16 — `AnchorYoffset` in pt. Same shape as `AnchoredXOffset`.
    AnchoredYOffset,
    /// W1.16 — `HorizontalReferencePoint` for Custom positioning
    /// (`"AnchorLocation"`, `"ColumnEdge"`, `"TextFrame"`,
    /// `"PageMargins"`, `"PageEdge"`). `Value::Text`; empty clears.
    AnchoredHorizontalReference,
    /// W1.16 — `VerticalReferencePoint` (`"LineBaseline"`,
    /// `"LineXHeight"`, `"Column"`, `"TextFrame"`, `"PageMargins"`,
    /// `"PageEdge"`, …). `Value::Text`; empty clears.
    AnchoredVerticalReference,
    /// W1.16 — `HorizontalAlignment` (`"LeftAlign"`, `"CenterAlign"`,
    /// `"RightAlign"`). `Value::Text`; empty clears.
    AnchoredHorizontalAlignment,
    /// W1.16 — `VerticalAlignment` (`"TopAlign"`, `"CenterAlign"`,
    /// `"BottomAlign"`). `Value::Text`; empty clears.
    AnchoredVerticalAlignment,
    /// W1.16 — `SpineRelative` flag (flips the offset direction on
    /// facing pages). `Value::Bool`. The parse field is a plain
    /// `bool` (default `false`), so this round-trips bytewise.
    AnchoredSpineRelative,
    /// W1.16 — `LockPosition` flag (pins the anchored frame to its
    /// current page position). `Value::Bool`. Plain `bool` parse
    /// field (default `false`); round-trips bytewise.
    AnchoredLockPosition,

    // ---- W2.5 — element-level visibility / lock -----------------
    /// W2.5 — element-level `Visible="true|false"` on any page item.
    /// `Value::Bool`. Distinct from `LayerVisible` (which gates a whole
    /// layer): this hides one item. Carried on every page-item kind
    /// (`CommonAttrs::visible`, default `true`). The renderer skips
    /// emitting items whose `Visible="false"` on the next rebuild, so
    /// the InvalidationHint is a structural rebuild. The parse field is
    /// a plain `bool`, so it round-trips bytewise.
    ElementVisible,
    /// W2.5 — element-level `Locked="true|false"` on any page item.
    /// `Value::Bool`. The renderer ignores it (locked items still
    /// paint); the canvas hit-tester blocks selection of a locked item
    /// — the `LayerLocked` precedent. Paint-neutral, so the
    /// InvalidationHint is empty (no scene change). Plain `bool` parse
    /// field (default `false`); round-trips bytewise.
    ElementLocked,

    // ---- v43 batch — stroke line ends (arrowheads) ---------------
    /// v43 — `LeftLineEnd`: the arrowhead at the line's START anchor.
    /// `Value::Text` carrying the IDML `ArrowHead` enumeration token
    /// (`"SimpleArrowHead"`, `"TriangleArrowHead"`,
    /// `"CircleSolidArrowHead"`, ... — `ArrowheadType::as_idml`'s
    /// vocabulary); empty string clears (= `"None"`). GraphicLine-only
    /// (the kind that parses the attribute; InDesign draws line ends
    /// on open paths, which IDML serialises as `<GraphicLine>`).
    /// Unknown tokens raise `InvalidValue`. Paint-only
    /// (`frame_style`). Undo note: a prior out-of-vocabulary token
    /// (`ArrowheadType::Other`, unreachable from real InDesign
    /// exports) inverts to clear — the parse layer discarded the raw
    /// spelling.
    FrameStrokeStartArrowhead,
    /// v43 — `RightLineEnd`: the arrowhead at the line's END anchor.
    /// Same contract as `FrameStrokeStartArrowhead`.
    FrameStrokeEndArrowhead,
}

/// Phase H — which corner of a `PathAnchor` the path-point edit
/// targets: the anchor itself or one of its two Bezier handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum PathPointRole {
    Anchor,
    Left,
    Right,
}

/// Phase H — address of one Bezier handle inside a `Polygon`'s
/// `PathPointArray`. `index` is the flat anchor index across all
/// subpaths (compound polygons concatenate subpaths into one
/// `anchors` Vec; `subpath_starts` marks each contour's first
/// index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct PathPointAddress {
    pub index: usize,
    pub role: PathPointRole,
}

impl PropertyPath {
    /// Human-friendly label for diagnostics + descriptor surfaces.
    pub fn label(&self) -> &'static str {
        match self {
            PropertyPath::FrameBounds => "frame.bounds",
            PropertyPath::FrameFillColor => "frame.fillColor",
            PropertyPath::FrameStrokeColor => "frame.strokeColor",
            PropertyPath::FrameStrokeWeight => "frame.strokeWeight",
            PropertyPath::FrameOpacity => "frame.opacity",
            PropertyPath::FrameTransform => "frame.transform",
            PropertyPath::ImageContentTransform => "frame.imageContentTransform",
            PropertyPath::FramePathPoint => "frame.pathPoint",
            PropertyPath::PathPointInsert => "frame.pathPointInsert",
            PropertyPath::PathPointRemove => "frame.pathPointRemove",
            PropertyPath::PathPointCurveType => "frame.pathPointCurveType",
            PropertyPath::LayerVisible => "layer.visible",
            PropertyPath::LayerLocked => "layer.locked",
            PropertyPath::LayerPrintable => "layer.printable",
            PropertyPath::LayerName => "layer.name",
            PropertyPath::CharacterFontSize => "character.fontSize",
            PropertyPath::CharacterLeading => "character.leading",
            PropertyPath::CharacterTracking => "character.tracking",
            PropertyPath::CharacterFillColor => "character.fillColor",
            PropertyPath::ParagraphSpaceBefore => "paragraph.spaceBefore",
            PropertyPath::ParagraphSpaceAfter => "paragraph.spaceAfter",
            PropertyPath::ParagraphFirstLineIndent => "paragraph.firstLineIndent",
            PropertyPath::AppliedParagraphStyle => "paragraph.appliedStyle",
            PropertyPath::AppliedCharacterStyle => "character.appliedStyle",
            PropertyPath::AppliedObjectStyle => "object.appliedStyle",
            PropertyPath::AppliedCellStyle => "cell.appliedStyle",
            PropertyPath::AppliedTableStyle => "table.appliedStyle",
            PropertyPath::AppliedConditions => "story.appliedConditions",
            PropertyPath::FrameInsetSpacing => "textFrame.insetSpacing",
            PropertyPath::ParagraphJustification => "paragraph.justification",
            PropertyPath::ParagraphStyleNextStyle => "paragraphStyle.nextStyle",
            PropertyPath::ParagraphAppliedNumberingList => "paragraph.appliedNumberingList",
            PropertyPath::FrameStrokeEndCap => "frame.strokeEndCap",
            PropertyPath::FrameTextWrapMode => "frame.textWrapMode",
            PropertyPath::FrameTextWrapOffsets => "frame.textWrapOffsets",
            PropertyPath::FrameTextWrapContourType => "frame.textWrapContourType",
            PropertyPath::FrameTextWrapContourIncludeInside => "frame.textWrapContourIncludeInside",
            PropertyPath::FrameFittingCrops => "frame.fittingCrops",
            PropertyPath::FrameFittingType => "frame.fittingType",
            PropertyPath::FrameDropShadow => "frame.dropShadow",
            PropertyPath::FramePath => "frame.path",
            PropertyPath::FrameFillTint => "frame.fillTint",
            PropertyPath::FrameGradientFillAngle => "frame.gradientFillAngle",
            PropertyPath::FrameGradientFillLength => "frame.gradientFillLength",
            PropertyPath::FrameGradientStrokeAngle => "frame.gradientStrokeAngle",
            PropertyPath::FrameGradientStrokeLength => "frame.gradientStrokeLength",
            PropertyPath::PathOpenAt => "path.openAt",
            PropertyPath::OutlineStroke => "path.outlineStroke",
            PropertyPath::OutlineStrokeVariable => "path.outlineStrokeVariable",
            PropertyPath::OffsetPath => "path.offset",
            PropertyPath::SimplifyPath => "path.simplify",
            PropertyPath::FrameGradientFeather => "frame.gradientFeather",
            PropertyPath::PageBounds => "page.bounds",
            PropertyPath::FrameNonprinting => "frame.nonprinting",
            PropertyPath::FrameDropShadowMode => "frame.dropShadowMode",
            PropertyPath::FrameDropShadowXOffset => "frame.dropShadowXOffset",
            PropertyPath::FrameDropShadowYOffset => "frame.dropShadowYOffset",
            PropertyPath::FrameDropShadowSize => "frame.dropShadowSize",
            PropertyPath::FrameDropShadowOpacity => "frame.dropShadowOpacity",
            PropertyPath::FrameDropShadowColor => "frame.dropShadowColor",
            PropertyPath::CharacterFontFamily => "character.fontFamily",
            PropertyPath::CharacterFontStyle => "character.fontStyle",
            PropertyPath::CharacterKerningMethod => "character.kerningMethod",
            PropertyPath::CharacterCase => "character.case",
            PropertyPath::CharacterPosition => "character.position",
            PropertyPath::CharacterLanguage => "character.language",
            PropertyPath::CharacterBaselineShift => "character.baselineShift",
            PropertyPath::CharacterHorizontalScale => "character.horizontalScale",
            PropertyPath::CharacterVerticalScale => "character.verticalScale",
            PropertyPath::CharacterSkew => "character.skew",
            PropertyPath::CharacterUnderline => "character.underline",
            PropertyPath::CharacterStrikethru => "character.strikethru",
            PropertyPath::CharacterLigatures => "character.ligatures",
            PropertyPath::CharacterOtfFeatures => "character.otfFeatures",
            PropertyPath::ParagraphLeftIndent => "paragraph.leftIndent",
            PropertyPath::ParagraphRightIndent => "paragraph.rightIndent",
            PropertyPath::ParagraphDropCapCharacters => "paragraph.dropCapCharacters",
            PropertyPath::ParagraphDropCapLines => "paragraph.dropCapLines",
            PropertyPath::ParagraphHyphenation => "paragraph.hyphenation",
            PropertyPath::ParagraphKeepLinesTogether => "paragraph.keepLinesTogether",
            PropertyPath::ParagraphKeepWithNext => "paragraph.keepWithNext",
            PropertyPath::ParagraphRuleAbove => "paragraph.ruleAbove",
            PropertyPath::ParagraphRuleBelow => "paragraph.ruleBelow",
            PropertyPath::ParagraphTabStops => "paragraph.tabStops",
            PropertyPath::ParagraphListType => "paragraph.listType",
            PropertyPath::ParagraphBulletCharacter => "paragraph.bulletCharacter",
            PropertyPath::ParagraphNumberingFormat => "paragraph.numberingFormat",
            // W0.3 — text-frame prefs.
            PropertyPath::TextFrameColumnCount => "textFrame.columnCount",
            PropertyPath::TextFrameColumnGutter => "textFrame.columnGutter",
            PropertyPath::TextFrameColumnBalance => "textFrame.columnBalance",
            PropertyPath::TextFrameVerticalJustification => "textFrame.verticalJustification",
            PropertyPath::TextFrameAutoSizing => "textFrame.autoSizing",
            PropertyPath::TextFrameFirstBaseline => "textFrame.firstBaseline",
            // W0.3 — text wrap.
            PropertyPath::TextWrapInvert => "frame.textWrapInvert",
            // W0.3 — frame fitting.
            PropertyPath::FrameFittingReferencePoint => "frame.fittingReferencePoint",
            PropertyPath::FrameAutoFit => "frame.autoFit",
            // W0.3 — stroke.
            PropertyPath::FrameStrokeType => "frame.strokeType",
            PropertyPath::FrameStrokeJoin => "frame.strokeJoin",
            PropertyPath::FrameStrokeMiterLimit => "frame.strokeMiterLimit",
            PropertyPath::FrameStrokeAlignment => "frame.strokeAlignment",
            PropertyPath::FrameStrokeGapColor => "frame.strokeGapColor",
            PropertyPath::FrameStrokeGapTint => "frame.strokeGapTint",
            PropertyPath::FrameStrokeDashArray => "frame.strokeDashArray",
            // W0.3 — corners.
            PropertyPath::FrameCornerOptionTopLeft => "frame.cornerOptionTopLeft",
            PropertyPath::FrameCornerOptionTopRight => "frame.cornerOptionTopRight",
            PropertyPath::FrameCornerOptionBottomLeft => "frame.cornerOptionBottomLeft",
            PropertyPath::FrameCornerOptionBottomRight => "frame.cornerOptionBottomRight",
            PropertyPath::FrameCornerRadiusTopLeft => "frame.cornerRadiusTopLeft",
            PropertyPath::FrameCornerRadiusTopRight => "frame.cornerRadiusTopRight",
            PropertyPath::FrameCornerRadiusBottomLeft => "frame.cornerRadiusBottomLeft",
            PropertyPath::FrameCornerRadiusBottomRight => "frame.cornerRadiusBottomRight",
            // W0.3 — transform decompose.
            PropertyPath::FrameRotationAngle => "frame.rotationAngle",
            PropertyPath::FrameScaleX => "frame.scaleX",
            PropertyPath::FrameScaleY => "frame.scaleY",
            PropertyPath::FrameFlipH => "frame.flipH",
            PropertyPath::FrameFlipV => "frame.flipV",
            // W0.3 — overprint.
            PropertyPath::FrameOverprintFill => "frame.overprintFill",
            PropertyPath::FrameOverprintStroke => "frame.overprintStroke",
            // W0.4 — transparency effects.
            PropertyPath::FrameInnerShadowEnabled => "frame.innerShadow",
            PropertyPath::FrameInnerShadowBlendMode => "frame.innerShadow.blendMode",
            PropertyPath::FrameInnerShadowColor => "frame.innerShadow.color",
            PropertyPath::FrameInnerShadowOpacity => "frame.innerShadow.opacity",
            PropertyPath::FrameInnerShadowAngle => "frame.innerShadow.angle",
            PropertyPath::FrameInnerShadowDistance => "frame.innerShadow.distance",
            PropertyPath::FrameInnerShadowSize => "frame.innerShadow.size",
            PropertyPath::FrameInnerShadowChoke => "frame.innerShadow.choke",
            PropertyPath::FrameInnerShadowNoise => "frame.innerShadow.noise",
            PropertyPath::FrameOuterGlowEnabled => "frame.outerGlow",
            PropertyPath::FrameOuterGlowBlendMode => "frame.outerGlow.blendMode",
            PropertyPath::FrameOuterGlowColor => "frame.outerGlow.color",
            PropertyPath::FrameOuterGlowOpacity => "frame.outerGlow.opacity",
            PropertyPath::FrameOuterGlowSpread => "frame.outerGlow.spread",
            PropertyPath::FrameOuterGlowSize => "frame.outerGlow.size",
            PropertyPath::FrameOuterGlowNoise => "frame.outerGlow.noise",
            PropertyPath::FrameInnerGlowEnabled => "frame.innerGlow",
            PropertyPath::FrameInnerGlowBlendMode => "frame.innerGlow.blendMode",
            PropertyPath::FrameInnerGlowColor => "frame.innerGlow.color",
            PropertyPath::FrameInnerGlowOpacity => "frame.innerGlow.opacity",
            PropertyPath::FrameInnerGlowChoke => "frame.innerGlow.choke",
            PropertyPath::FrameInnerGlowSize => "frame.innerGlow.size",
            PropertyPath::FrameInnerGlowSource => "frame.innerGlow.source",
            PropertyPath::FrameInnerGlowNoise => "frame.innerGlow.noise",
            PropertyPath::FrameBevelEnabled => "frame.bevel",
            PropertyPath::FrameBevelStyle => "frame.bevel.style",
            PropertyPath::FrameBevelTechnique => "frame.bevel.technique",
            PropertyPath::FrameBevelDepth => "frame.bevel.depth",
            PropertyPath::FrameBevelDirection => "frame.bevel.direction",
            PropertyPath::FrameBevelSize => "frame.bevel.size",
            PropertyPath::FrameBevelSoften => "frame.bevel.soften",
            PropertyPath::FrameBevelAngle => "frame.bevel.angle",
            PropertyPath::FrameBevelAltitude => "frame.bevel.altitude",
            PropertyPath::FrameBevelHighlightColor => "frame.bevel.highlightColor",
            PropertyPath::FrameBevelShadowColor => "frame.bevel.shadowColor",
            PropertyPath::FrameBevelHighlightOpacity => "frame.bevel.highlightOpacity",
            PropertyPath::FrameBevelShadowOpacity => "frame.bevel.shadowOpacity",
            PropertyPath::FrameSatinEnabled => "frame.satin",
            PropertyPath::FrameSatinBlendMode => "frame.satin.blendMode",
            PropertyPath::FrameSatinColor => "frame.satin.color",
            PropertyPath::FrameSatinOpacity => "frame.satin.opacity",
            PropertyPath::FrameSatinAngle => "frame.satin.angle",
            PropertyPath::FrameSatinDistance => "frame.satin.distance",
            PropertyPath::FrameSatinSize => "frame.satin.size",
            PropertyPath::FrameSatinInvert => "frame.satin.invert",
            PropertyPath::FrameFeatherEnabled => "frame.feather",
            PropertyPath::FrameFeatherWidth => "frame.feather.width",
            PropertyPath::FrameFeatherCornerType => "frame.feather.cornerType",
            PropertyPath::FrameFeatherNoise => "frame.feather.noise",
            PropertyPath::FrameFeatherChoke => "frame.feather.choke",
            PropertyPath::FrameDirectionalFeatherEnabled => "frame.directionalFeather",
            PropertyPath::FrameDirectionalFeatherLeftWidth => "frame.directionalFeather.leftWidth",
            PropertyPath::FrameDirectionalFeatherRightWidth => {
                "frame.directionalFeather.rightWidth"
            }
            PropertyPath::FrameDirectionalFeatherTopWidth => "frame.directionalFeather.topWidth",
            PropertyPath::FrameDirectionalFeatherBottomWidth => {
                "frame.directionalFeather.bottomWidth"
            }
            PropertyPath::FrameDirectionalFeatherAngle => "frame.directionalFeather.angle",
            PropertyPath::FrameDirectionalFeatherNoise => "frame.directionalFeather.noise",
            PropertyPath::FrameDirectionalFeatherChoke => "frame.directionalFeather.choke",
            PropertyPath::FrameBlendMode => "frame.blendMode",
            PropertyPath::NextTextFrame => "textFrame.nextTextFrame",
            PropertyPath::PreviousTextFrame => "textFrame.previousTextFrame",
            // W3.A1 — table cell properties.
            PropertyPath::CellFillColor => "cell.fillColor",
            PropertyPath::CellFillTint => "cell.fillTint",
            PropertyPath::CellInsetTop => "cell.insetTop",
            PropertyPath::CellInsetLeft => "cell.insetLeft",
            PropertyPath::CellInsetBottom => "cell.insetBottom",
            PropertyPath::CellInsetRight => "cell.insetRight",
            PropertyPath::CellVerticalJustification => "cell.verticalJustification",
            // W1.11b — per-cell edge strokes.
            PropertyPath::CellTopEdgeStrokeColor => "cell.topEdgeStrokeColor",
            PropertyPath::CellTopEdgeStrokeWeight => "cell.topEdgeStrokeWeight",
            PropertyPath::CellTopEdgeStrokeTint => "cell.topEdgeStrokeTint",
            PropertyPath::CellBottomEdgeStrokeColor => "cell.bottomEdgeStrokeColor",
            PropertyPath::CellBottomEdgeStrokeWeight => "cell.bottomEdgeStrokeWeight",
            PropertyPath::CellBottomEdgeStrokeTint => "cell.bottomEdgeStrokeTint",
            PropertyPath::CellLeftEdgeStrokeColor => "cell.leftEdgeStrokeColor",
            PropertyPath::CellLeftEdgeStrokeWeight => "cell.leftEdgeStrokeWeight",
            PropertyPath::CellLeftEdgeStrokeTint => "cell.leftEdgeStrokeTint",
            PropertyPath::CellRightEdgeStrokeColor => "cell.rightEdgeStrokeColor",
            PropertyPath::CellRightEdgeStrokeWeight => "cell.rightEdgeStrokeWeight",
            PropertyPath::CellRightEdgeStrokeTint => "cell.rightEdgeStrokeTint",
            // Aftercare-A — table dimensions (read-only).
            PropertyPath::TableRowCount => "table.rowCount",
            PropertyPath::TableColumnCount => "table.columnCount",
            PropertyPath::PluginMetadata => "plugin.metadata",
            PropertyPath::AnchoredPosition => "anchored.position",
            PropertyPath::AnchorPoint => "anchored.anchorPoint",
            PropertyPath::AnchoredXOffset => "anchored.xOffset",
            PropertyPath::AnchoredYOffset => "anchored.yOffset",
            PropertyPath::AnchoredHorizontalReference => "anchored.horizontalReference",
            PropertyPath::AnchoredVerticalReference => "anchored.verticalReference",
            PropertyPath::AnchoredHorizontalAlignment => "anchored.horizontalAlignment",
            PropertyPath::AnchoredVerticalAlignment => "anchored.verticalAlignment",
            PropertyPath::AnchoredSpineRelative => "anchored.spineRelative",
            PropertyPath::AnchoredLockPosition => "anchored.lockPosition",
            // W2.5 — element-level visibility / lock.
            PropertyPath::ElementVisible => "element.visible",
            PropertyPath::ElementLocked => "element.locked",
            // v43 batch — stroke line ends (arrowheads).
            PropertyPath::FrameStrokeStartArrowhead => "frame.strokeStartArrowhead",
            PropertyPath::FrameStrokeEndArrowhead => "frame.strokeEndArrowhead",
        }
    }
}

/// Track J — wire-shape mirror of `paged_model::PathAnchor`. The
/// parse-side type doesn't carry `Deserialize`/`PartialEq`/`Tsify`,
/// and the mutate API needs all three so this Op crosses the wasm
/// boundary. The field shapes match exactly: `anchor` is the
/// on-curve point, `left` / `right` are the incoming / outgoing
/// Bezier handles, all in the page item's inner coordinate system.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct PathAnchorSpec {
    pub anchor: [f32; 2],
    pub left: [f32; 2],
    pub right: [f32; 2],
}

impl PathAnchorSpec {
    pub fn from_parse(a: &paged_model::PathAnchor) -> Self {
        Self {
            anchor: [a.anchor.0, a.anchor.1],
            left: [a.left.0, a.left.1],
            right: [a.right.0, a.right.1],
        }
    }
    pub fn to_parse(&self) -> paged_model::PathAnchor {
        paged_model::PathAnchor {
            anchor: (self.anchor[0], self.anchor[1]),
            left: (self.left[0], self.left[1]),
            right: (self.right[0], self.right[1]),
        }
    }
}

/// Editor-ops — wire mirror of `paged_model::GradientFeatherStop`
/// (the AST type predates `PartialEq`/`Tsify`; the mirror keeps the
/// op wire-shaped, the `PathAnchorSpec` precedent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GradientFeatherStopSpec {
    #[serde(default)]
    pub stop_color: Option<String>,
    pub location_pct: f32,
    pub alpha_pct: f32,
    #[serde(default)]
    pub midpoint_pct: f32,
}

/// Editor-ops — wire mirror of `paged_model::GradientFeatherParams`.
/// Whole-struct authoring (kind + axis + stop LIST change together;
/// `Value` has no generic list form, so the drop-shadow per-field
/// shape doesn't fit). The renderer already draws this effect; only
/// authoring was missing. `stop_color` round-trips faithfully but the
/// rasterizer currently consumes `alpha_pct` only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GradientFeatherSpec {
    /// `"Linear"` or `"Radial"`.
    #[serde(default)]
    pub gradient_type: Option<String>,
    #[serde(default)]
    pub start_point: Option<[f32; 2]>,
    #[serde(default)]
    pub end_point: Option<[f32; 2]>,
    #[serde(default)]
    pub angle_deg: Option<f32>,
    #[serde(default)]
    pub stops: Vec<GradientFeatherStopSpec>,
}

impl GradientFeatherSpec {
    pub fn from_parse(p: &paged_model::GradientFeatherParams) -> Self {
        Self {
            gradient_type: p.gradient_type.clone(),
            start_point: p.start_point.map(|(x, y)| [x, y]),
            end_point: p.end_point.map(|(x, y)| [x, y]),
            angle_deg: p.angle_deg,
            stops: p
                .stops
                .iter()
                .map(|s| GradientFeatherStopSpec {
                    stop_color: s.stop_color.clone(),
                    location_pct: s.location_pct,
                    alpha_pct: s.alpha_pct,
                    midpoint_pct: s.midpoint_pct,
                })
                .collect(),
        }
    }
    pub fn to_parse(&self) -> paged_model::GradientFeatherParams {
        paged_model::GradientFeatherParams {
            gradient_type: self.gradient_type.clone(),
            start_point: self.start_point.map(|[x, y]| (x, y)),
            end_point: self.end_point.map(|[x, y]| (x, y)),
            angle_deg: self.angle_deg,
            stops: self
                .stops
                .iter()
                .map(|s| paged_model::GradientFeatherStop {
                    stop_color: s.stop_color.clone(),
                    location_pct: s.location_pct,
                    alpha_pct: s.alpha_pct,
                    midpoint_pct: s.midpoint_pct,
                })
                .collect(),
        }
    }
}

/// W0.2 — wire mirror of `paged_model::ParagraphRule` (the
/// AST type predates `Tsify`; the mirror keeps the op wire-shaped,
/// the `GradientFeatherSpec` precedent). Carries every field the
/// parser models so the whole-struct `ParagraphRuleAbove` /
/// `ParagraphRuleBelow` paths round-trip a paragraph's rule verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphRuleSpec {
    #[serde(default)]
    pub on: Option<bool>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub tint: Option<f32>,
    #[serde(default)]
    pub weight: Option<f32>,
    #[serde(default)]
    pub offset: Option<f32>,
    #[serde(default)]
    pub left_indent: Option<f32>,
    #[serde(default)]
    pub right_indent: Option<f32>,
    #[serde(default)]
    pub width: Option<String>,
}

impl ParagraphRuleSpec {
    pub fn from_parse(p: &paged_model::ParagraphRule) -> Self {
        Self {
            on: p.on,
            color: p.color.clone(),
            tint: p.tint,
            weight: p.weight,
            offset: p.offset,
            left_indent: p.left_indent,
            right_indent: p.right_indent,
            width: p.width.clone(),
        }
    }
    pub fn to_parse(&self) -> paged_model::ParagraphRule {
        paged_model::ParagraphRule {
            on: self.on,
            color: self.color.clone(),
            tint: self.tint,
            weight: self.weight,
            offset: self.offset,
            left_indent: self.left_indent,
            right_indent: self.right_indent,
            width: self.width.clone(),
        }
    }
}

/// W0.2 — wire mirror of `paged_model::TabStop`. The `ParagraphTabStops`
/// path replaces the paragraph's whole `<TabList>` in one op; `Value`
/// has no per-element list-edit form, so the UI sends the full new
/// stop list (the gradient-feather stop-list precedent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct TabStopSpec {
    pub position: f32,
    #[serde(default)]
    pub alignment: Option<String>,
    #[serde(default)]
    pub alignment_character: Option<String>,
    #[serde(default)]
    pub leader: Option<String>,
}

impl TabStopSpec {
    pub fn from_parse(t: &paged_model::TabStop) -> Self {
        Self {
            position: t.position,
            alignment: t.alignment.clone(),
            alignment_character: t.alignment_character.clone(),
            leader: t.leader.clone(),
        }
    }
    pub fn to_parse(&self) -> paged_model::TabStop {
        paged_model::TabStop {
            position: self.position,
            alignment: self.alignment.clone(),
            alignment_character: self.alignment_character.clone(),
            leader: self.leader.clone(),
        }
    }
}

/// W3.A1 — opaque JSON blob capturing a removed table row/column's
/// content for the `DeleteTable{Row,Column}` inverse. Carried on
/// `InsertTable{Row,Column} { restore }` so undo re-inserts the line
/// losslessly (within the v1 field set — see `TableCellSpec`).
///
/// A `String` rather than a typed wire struct because the parse-side
/// `TableRow` / `TableColumn` / `TableCell` are `Serialize`-only (no
/// `Deserialize` / `Tsify`); serialising the apply layer's own
/// `Deserialize`-able mirror to a string keeps the Op wire-shaped — the
/// `restore_spread_json` precedent on `InsertPage`. `None` on a forward
/// delete; the apply layer fills it on the inverse.
pub type TableLineRestoreJson = String;

/// W3.A1 — `Deserialize`-able mirror of the round-trippable fields of a
/// `paged_model::TableCell`, used inside the `DeleteTable{Row,Column}`
/// restore blob. Captures the cell's structure + style (spans, insets,
/// fill, applied style, vertical justification). **Does NOT carry the
/// cell's `paragraphs`** — cell text content is out-of-band of the
/// story-offset space and not addressable in W3.A1 (see the task's
/// cell-text finding); restoring cell *text* on a delete-undo is a v2
/// item. The renderer re-emits an empty cell from the restored
/// structure, matching `NodeSpec`'s "minimal supported field set"
/// precedent (drop_shadow / effects residue on re-insert).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableCellSpec {
    pub name: Option<String>,
    pub row_span: u32,
    pub column_span: u32,
    pub text_top_inset: f32,
    pub text_left_inset: f32,
    pub text_bottom_inset: f32,
    pub text_right_inset: f32,
    pub applied_cell_style: Option<String>,
    pub fill_color: Option<String>,
    pub vertical_justification: Option<String>,
}

impl TableCellSpec {
    pub fn from_parse(c: &paged_model::TableCell) -> Self {
        Self {
            name: c.name.clone(),
            row_span: c.row_span,
            column_span: c.column_span,
            text_top_inset: c.text_top_inset,
            text_left_inset: c.text_left_inset,
            text_bottom_inset: c.text_bottom_inset,
            text_right_inset: c.text_right_inset,
            applied_cell_style: c.applied_cell_style.clone(),
            fill_color: c.fill_color.clone(),
            vertical_justification: c.vertical_justification.clone(),
        }
    }
    pub fn to_parse(&self) -> paged_model::TableCell {
        paged_model::TableCell {
            name: self.name.clone(),
            row_span: self.row_span.max(1),
            column_span: self.column_span.max(1),
            text_top_inset: self.text_top_inset,
            text_left_inset: self.text_left_inset,
            text_bottom_inset: self.text_bottom_inset,
            text_right_inset: self.text_right_inset,
            applied_cell_style: self.applied_cell_style.clone(),
            fill_color: self.fill_color.clone(),
            vertical_justification: self.vertical_justification.clone(),
            ..Default::default()
        }
    }
}

/// W3.A1 — `Deserialize`-able mirror of a `paged_model::TableRow`'s
/// round-trippable fields, for the `DeleteTableRow` restore blob.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableRowSpec {
    pub name: Option<String>,
    pub single_row_height: Option<f32>,
    pub minimum_height: Option<f32>,
    pub maximum_height: Option<f32>,
}

impl TableRowSpec {
    pub fn from_parse(r: &paged_model::TableRow) -> Self {
        Self {
            name: r.name.clone(),
            single_row_height: r.single_row_height,
            minimum_height: r.minimum_height,
            maximum_height: r.maximum_height,
        }
    }
    pub fn to_parse(&self) -> paged_model::TableRow {
        paged_model::TableRow {
            name: self.name.clone(),
            single_row_height: self.single_row_height,
            minimum_height: self.minimum_height,
            maximum_height: self.maximum_height,
            ..Default::default()
        }
    }
}

/// W3.A1 — `Deserialize`-able mirror of a `paged_model::TableColumn`,
/// for the `DeleteTableColumn` restore blob.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableColumnSpec {
    pub name: Option<String>,
    pub single_column_width: Option<f32>,
}

impl TableColumnSpec {
    pub fn from_parse(c: &paged_model::TableColumn) -> Self {
        Self {
            name: c.name.clone(),
            single_column_width: c.single_column_width,
        }
    }
    pub fn to_parse(&self) -> paged_model::TableColumn {
        paged_model::TableColumn {
            name: self.name.clone(),
            single_column_width: self.single_column_width,
            ..Default::default()
        }
    }
}

/// W3.A1 — the captured content of a removed table row or column. JSON-
/// encoded into the `restore` blob on `DeleteTable{Row,Column}`'s
/// inverse.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemovedTableLine {
    pub row: Option<TableRowSpec>,
    pub column: Option<TableColumnSpec>,
    pub cells: Vec<TableCellSpec>,
}

/// Typed payload for a `SetProperty` Op. Each variant carries a value
/// of a specific kind; the apply layer's `TypeMismatch` error fires if
/// the variant doesn't match what the path expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Value {
    Bounds([f32; 4]),
    ColorRef(Option<String>),
    /// Inspector M1 Phase A: a single floating-point number with an
    /// implicit unit (the property's documentation says which — pt
    /// for stroke weight, % for opacity, etc.). `None` represents
    /// "unset / inherit document default" on properties that allow
    /// the absence; a present `Some(_)` is a per-frame override.
    Length(Option<f32>),
    /// Phase D — 2D affine matrix `[a, b, c, d, tx, ty]` (IDML
    /// `ItemTransform` packing: a point `(x, y)` maps to
    /// `(a*x + c*y + tx, b*x + d*y + ty)`). `None` represents
    /// "no `ItemTransform`" — the renderer falls back to identity.
    Transform(Option<[f32; 6]>),
    /// Phase H — addressed 2D point on a `Polygon`'s `PathPointArray`.
    /// `position` is the new (x, y) in the frame's inner coordinate
    /// system; `address` picks which handle of which anchor.
    PathPoint {
        address: PathPointAddress,
        position: [f32; 2],
    },
    /// Track J — insert a new anchor into the path at `index`. Used
    /// both as the forward value of a `PathPointInsert` op (UI
    /// dispatches it from a segment click; the anchor is the
    /// de-Casteljau split result) and as the inverse value of a
    /// `PathPointRemove` op. `prev_subpath_starts` is populated by
    /// the apply layer when this Value is the inverse of a Remove
    /// — restoring the full pre-Remove subpath-boundary table
    /// guarantees bytewise round-trip even when the Remove
    /// collapsed a degenerate single-anchor subpath. UI senders
    /// leave it `None` and the apply layer derives the new
    /// `subpath_starts` from the increment rule.
    PathPointInsert {
        index: usize,
        anchor: PathAnchorSpec,
        #[serde(default)]
        prev_subpath_starts: Option<Vec<usize>>,
    },
    /// Track J — remove the anchor at `index`. Forward value of a
    /// `PathPointRemove` op (UI dispatches it from Backspace on a
    /// selected anchor); also the inverse value of `PathPointInsert`.
    /// `prev_subpath_starts` mirrors the `PathPointInsert` field
    /// and serves the same round-trip role.
    PathPointRemove {
        index: usize,
        #[serde(default)]
        prev_subpath_starts: Option<Vec<usize>>,
    },
    /// Track J — set the curve type of the anchor at `index`.
    /// `smooth: true` derives handles from neighbour tangents
    /// (1/3-distance heuristic); `smooth: false` collapses handles
    /// to the anchor (corner). When `prev` is `Some`, apply restores
    /// the carried anchor verbatim and ignores `smooth` — used by
    /// the inverse so undo round-trips bytewise even when the
    /// "smooth" derivation would lose the prior handle positions.
    PathPointCurveType {
        index: usize,
        smooth: bool,
        #[serde(default)]
        prev: Option<PathAnchorSpec>,
    },
    /// Plugin-metadata carrier — one Label `KeyValuePair`. `value:
    /// None` deletes the entry; `prev` is the apply-layer snapshot
    /// (`Some(None)` = "was absent") so the inverse restores exactly.
    /// Wire callers leave `prev` at `None`.
    PluginMetadata {
        key: String,
        value: Option<String>,
        /// B-16 — the calling plugin's manifest id. When `Some(id)`, the
        /// engine enforces that `key == "x-paged:<id>"` (mirrors the SDK
        /// door `foreignMetadataKey`), closing the bypass where a bundle
        /// holding the raw handle writes another plugin's namespace.
        /// Additive: `None` (the editor / pre-B-16 callers) keeps the
        /// prefix+cap+envelope-only behaviour. Full teeth arrive with the
        /// isolate; this is the server-side defence-in-depth half.
        #[serde(default)]
        caller: Option<String>,
        #[serde(default)]
        prev: Option<Option<String>>,
    },
    /// Track M — boolean toggle (e.g. layer visibility / lock /
    /// printable). The inverse is just the same Value with the
    /// flag negated.
    Bool(bool),
    /// Track M — plain text value (layer name, future story
    /// titles, etc.). Inverse via the previous text.
    Text(String),
    /// SDK Phase 5 (v1 sweep) — full path replacement on any
    /// path-bearing page item. Carries the new anchor list +
    /// `subpath_starts` for compound paths. Used by Pathfinder
    /// (Subtract / Exclude) — the result of a boolean op is a
    /// fresh polygon set that we drop in via one SetProperty,
    /// rather than churning through N PathPointInsert/Remove ops.
    ///
    /// The inverse `Value::FramePath` carries the prior anchors +
    /// starts so undo round-trips bytewise.
    FramePath {
        anchors: Vec<PathAnchorSpec>,
        subpath_starts: Vec<usize>,
    },
    /// Editor-ops (Scissors) — cut the path at the anchor at flat
    /// `index`. On a CLOSED subpath the contour opens there: the cut
    /// anchor splits into two coincident endpoints (every original
    /// edge survives; the contour just no longer closes). On an OPEN
    /// subpath an interior cut splits it into two open subpaths
    /// sharing duplicated endpoints. Mid-segment cuts are expressed
    /// editor-side as a Batch of `PathPointInsert` (the de Casteljau
    /// split) followed by `PathOpenAt` at the new anchor.
    ///
    /// The `prev_*` triple is inverse-only: the apply layer snapshots
    /// `(anchors, subpath_starts, subpath_open)` before cutting and
    /// the inverse restores all three verbatim — `FramePath` cannot
    /// serve as the inverse because it does not carry `subpath_open`.
    PathOpenAt {
        index: usize,
        #[serde(default)]
        prev_anchors: Option<Vec<PathAnchorSpec>>,
        #[serde(default)]
        prev_subpath_starts: Option<Vec<usize>>,
        #[serde(default)]
        prev_subpath_open: Option<Vec<bool>>,
    },
    /// B-05 — stroke-expansion outline. Same `prev_*` snapshot-
    /// inverse convention as `PathOpenAt` (FramePath lacks
    /// `subpath_open`). `cap`: `"butt" | "round" | "square"`;
    /// `join`: `"miter" | "round" | "bevel"`.
    OutlineStroke {
        width: f32,
        cap: String,
        join: String,
        miter_limit: f32,
        #[serde(default)]
        prev_anchors: Option<Vec<PathAnchorSpec>>,
        #[serde(default)]
        prev_subpath_starts: Option<Vec<usize>>,
        #[serde(default)]
        prev_subpath_open: Option<Vec<bool>>,
    },
    /// B-08 — VARIABLE-width stroke outline. `widths` are the full
    /// stroke widths at evenly-spaced stops along the centreline
    /// (arc-length-normalised), tapering the brush stroke; otherwise the
    /// same snapshot-inverse convention as `OutlineStroke`. `cap`/`join`
    /// reserved for v2 end/join styling.
    OutlineStrokeVariable {
        widths: Vec<f32>,
        cap: String,
        join: String,
        miter_limit: f32,
        #[serde(default)]
        prev_anchors: Option<Vec<PathAnchorSpec>>,
        #[serde(default)]
        prev_subpath_starts: Option<Vec<usize>>,
        #[serde(default)]
        prev_subpath_open: Option<Vec<bool>>,
    },
    /// B-05 — inset (`delta < 0`) / outset (`delta > 0`) of a single
    /// closed contour. Snapshot-inverse like `PathOpenAt`.
    OffsetPath {
        delta: f32,
        join: String,
        miter_limit: f32,
        #[serde(default)]
        prev_anchors: Option<Vec<PathAnchorSpec>>,
        #[serde(default)]
        prev_subpath_starts: Option<Vec<usize>>,
        #[serde(default)]
        prev_subpath_open: Option<Vec<bool>>,
    },
    /// B-05 — anchor-reduction within `tolerance` pt max deviation.
    /// Snapshot-inverse like `PathOpenAt`.
    SimplifyPath {
        tolerance: f32,
        #[serde(default)]
        prev_anchors: Option<Vec<PathAnchorSpec>>,
        #[serde(default)]
        prev_subpath_starts: Option<Vec<usize>>,
        #[serde(default)]
        prev_subpath_open: Option<Vec<bool>>,
    },
    /// Editor-ops — whole gradient-feather struct (`None` clears the
    /// effect). The inverse carries the prior `Option<spec>` so undo
    /// round-trips bytewise.
    GradientFeather(Option<GradientFeatherSpec>),
    /// W0.2 — whole paragraph rule struct (`RuleAbove` / `RuleBelow`).
    /// `None` clears the rule back to the all-`None` default. The
    /// inverse carries the prior `Option<spec>` so undo round-trips
    /// bytewise. Same whole-struct precedent as `GradientFeather`.
    ParagraphRule(Option<ParagraphRuleSpec>),
    /// W0.2 — whole `<TabList>` replacement. The empty vec clears all
    /// stops. The inverse carries the prior stop list so undo
    /// round-trips bytewise.
    TabStops(Vec<TabStopSpec>),
    /// W1.1 — a list of pt lengths. Serialises as
    /// `{ type: "lengths", value: [...] }`. The `FrameStrokeDashArray`
    /// path carries the per-frame `StrokeDashAndGap` override here: the
    /// alternating on/off dash lengths in pt. The empty vec clears the
    /// per-frame override (the stroke falls back to its
    /// `StrokeStyleDef` pattern / built-in name). The whole-list,
    /// `Value`-has-no-per-element-edit-form precedent of `TabStops`;
    /// additive new variant (rides the current protocol, no bump — the
    /// W0.2 `TabStops` / `ParagraphRule` precedent).
    Lengths(Vec<f32>),
}

/// Description of a node about to be inserted. Carries the minimal
/// Stage-1 supported field set plus `item_transform` — `RemoveNode` →
/// undo → re-insertion round-trips these reliably. (Without the
/// transform, undoing a deleteFrame snapped the frame back to the page
/// origin — the editor-suite AC-E2E-PROVE-3 finding.) Remaining
/// non-essential fields (drop_shadow, opacity, effects, …) still
/// default on re-insertion; that residue of the Stage 1 limitation
/// tightens in later stages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NodeSpec {
    TextFrame {
        self_id: String,
        bounds: [f32; 4],
        #[serde(default)]
        fill_color: Option<String>,
        #[serde(default)]
        stroke_color: Option<String>,
        #[serde(default)]
        stroke_weight: Option<f32>,
        /// 6-element affine `[a b c d tx ty]` — preserved across
        /// RemoveNode → undo so the frame re-inserts in place.
        #[serde(default)]
        item_transform: Option<[f32; 6]>,
        /// `ParentStory` id. `None` on a FRESH insert ⇒ the apply layer
        /// MINTS an empty story (`Story/u<n>`) and attaches it — every
        /// text frame carries a story, InDesign's model (a fresh frame
        /// used to carry none, so `hitTest` answered `storyId: null`
        /// and no caller could ever pour text into it — found live by
        /// the sheets K-1 e2e). `Some` on the RemoveNode-built spec ⇒
        /// undo of a delete REATTACHES the original story.
        #[serde(default)]
        parent_story: Option<String>,
    },
    Rectangle {
        self_id: String,
        bounds: [f32; 4],
        #[serde(default)]
        fill_color: Option<String>,
        #[serde(default)]
        stroke_color: Option<String>,
        #[serde(default)]
        stroke_weight: Option<f32>,
        /// 6-element affine `[a b c d tx ty]` — preserved across
        /// RemoveNode → undo so the frame re-inserts in place.
        #[serde(default)]
        item_transform: Option<[f32; 6]>,
    },
    /// W0.5 — an ellipse (`<Oval>`). Mirrors `Rectangle`'s spec arm:
    /// bounds + the same fill/stroke triple + an optional
    /// `item_transform` so RemoveNode → undo re-inserts byte-identically.
    /// The Ellipse tool's only structural difference from Rectangle is
    /// the kind vec it lands in (`Spread::ovals`) and how the renderer
    /// fills the bounds (an ellipse rather than a rect).
    Oval {
        self_id: String,
        bounds: [f32; 4],
        #[serde(default)]
        fill_color: Option<String>,
        #[serde(default)]
        stroke_color: Option<String>,
        #[serde(default)]
        stroke_weight: Option<f32>,
        #[serde(default)]
        item_transform: Option<[f32; 6]>,
    },
    /// Editor-ops — a graphic line. `anchors` carries the explicit
    /// path (two corner anchors for the Line tool; possibly more for
    /// captured lines) in spread coordinates with an identity
    /// `item_transform`; empty anchors fall back to the renderer's
    /// bounds-diagonal. `bounds` is the anchors' bounding box (used
    /// for hit-testing / selection chrome).
    GraphicLine {
        self_id: String,
        bounds: [f32; 4],
        #[serde(default)]
        anchors: Vec<PathAnchorSpec>,
        #[serde(default)]
        subpath_starts: Vec<usize>,
        #[serde(default)]
        subpath_open: Vec<bool>,
        #[serde(default)]
        stroke_color: Option<String>,
        #[serde(default)]
        stroke_weight: Option<f32>,
        /// Captured-node transform (RemoveNode → undo). New Line-tool
        /// creations pass `None` (anchors are already spread-space).
        #[serde(default)]
        item_transform: Option<[f32; 6]>,
    },
    /// Editor-ops — a polygon (the Pencil/freehand and captured-path
    /// kind). Carries the full path tables so `RemoveNode` → undo
    /// round-trips compound/open paths byte-identically.
    Polygon {
        self_id: String,
        bounds: [f32; 4],
        #[serde(default)]
        anchors: Vec<PathAnchorSpec>,
        #[serde(default)]
        subpath_starts: Vec<usize>,
        #[serde(default)]
        subpath_open: Vec<bool>,
        #[serde(default)]
        fill_color: Option<String>,
        #[serde(default)]
        stroke_color: Option<String>,
        #[serde(default)]
        stroke_weight: Option<f32>,
        /// Captured-node transform (RemoveNode → undo). Freehand
        /// creations pass `None`.
        #[serde(default)]
        item_transform: Option<[f32; 6]>,
    },
    /// Phase H — deep-clone the `source` node into a new node with
    /// `self_id`, shifting its bounds (or its item_transform's tx/ty
    /// for rotated frames) by `(dx, dy)`. The clone preserves every
    /// other field — fill, stroke, image link/bytes, item transform,
    /// the inner `image_item_transform`, etc. — so the duplicate
    /// looks identical to the original at the new position. Used by
    /// the canvas's Alt-drag-to-duplicate gesture; never serialised
    /// from a script.
    ///
    /// Track K — `destination_spread_id` lets the apply layer route
    /// the clone to a different spread than the source's. When
    /// `Some`, `dx`/`dy` are still world-space pointer deltas; the
    /// apply path additionally corrects for the source-vs-destination
    /// spread-origin offset so the inserted clone lands at the right
    /// page-local position on the destination. `None` preserves the
    /// Phase H.4 behaviour (clone into source's spread).
    CloneTranslate {
        self_id: String,
        source: NodeId,
        dx: f32,
        dy: f32,
        #[serde(default)]
        destination_spread_id: Option<String>,
    },
    /// S-03 — a `<Table>` created inside a story (parent
    /// `NodeId::Story`). Unlike the frame arms there is NO
    /// `item_transform`: a table is in-story content (it hangs off
    /// `Paragraph::table`), not a page item with its own affine. The
    /// apply layer builds a `paged_model::Table` of `rows × cols` empty
    /// `TableCell`s (coordinate `Name="col:row"`), sets the header /
    /// footer band counts, and applies the per-column / per-row sizing.
    /// `column_widths` / `row_heights` are pt; a short / empty vec leaves
    /// the trailing lines unsized (`SingleColumnWidth` / `SingleRowHeight`
    /// `None`). `self_id` becomes the table's IDML `Self` (the table_id).
    Table {
        self_id: String,
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
}

impl NodeSpec {
    /// The `NodeId` this spec will own once inserted.
    ///
    /// For the page-item kinds (frames, shapes, clone) the id is fully
    /// determined by the spec. For `NodeSpec::Table` the id is NOT — a
    /// table NodeId also needs the parent story id, which the spec
    /// alone doesn't carry. Callers that need a table's full
    /// `NodeId::Table` use [`NodeSpec::node_id_in_story`] with the
    /// parent story; this bare accessor returns a placeholder
    /// `NodeId::Table` whose `story_id` is empty (the apply layer never
    /// routes a Table insert through the generic `invert_insert_node`
    /// path — it builds the inverse from the parent story directly).
    pub fn node_id(&self) -> NodeId {
        match self {
            NodeSpec::TextFrame { self_id, .. } => NodeId::TextFrame(self_id.clone()),
            NodeSpec::Rectangle { self_id, .. } => NodeId::Rectangle(self_id.clone()),
            NodeSpec::Oval { self_id, .. } => NodeId::Oval(self_id.clone()),
            NodeSpec::GraphicLine { self_id, .. } => NodeId::GraphicLine(self_id.clone()),
            NodeSpec::Polygon { self_id, .. } => NodeId::Polygon(self_id.clone()),
            NodeSpec::Table { self_id, .. } => NodeId::Table {
                story_id: String::new(),
                table_id: self_id.clone(),
            },
            NodeSpec::CloneTranslate {
                self_id, source, ..
            } => match source {
                NodeId::TextFrame(_) => NodeId::TextFrame(self_id.clone()),
                NodeId::Rectangle(_) => NodeId::Rectangle(self_id.clone()),
                // Other shape kinds aren't supported yet — apply.rs
                // raises UnsupportedProperty on them.
                _ => source.clone(),
            },
        }
    }

    /// S-03 — the full `NodeId::Table { story_id, table_id }` a
    /// `NodeSpec::Table` owns once inserted into `story_id`. Returns the
    /// bare [`NodeSpec::node_id`] for every non-table spec (they don't
    /// nest in a story).
    pub fn node_id_in_story(&self, story_id: &str) -> NodeId {
        match self {
            NodeSpec::Table { self_id, .. } => NodeId::Table {
                story_id: story_id.to_string(),
                table_id: self_id.clone(),
            },
            other => other.node_id(),
        }
    }

    /// S-03 — build a `paged_model::Table` of `rows × cols` empty cells
    /// from a `NodeSpec::Table`. Cells are keyed `Name="col:row"` (the
    /// IDML convention, column-major document order). Header / footer
    /// band counts and per-line sizing are honoured; the body row count
    /// is the rows not covered by a band. Panics if called on a
    /// non-table spec (apply only calls it inside the Table arm).
    pub fn to_parse_table(&self) -> paged_model::Table {
        let NodeSpec::Table {
            self_id,
            rows,
            cols,
            header_rows,
            footer_rows,
            column_widths,
            row_heights,
        } = self
        else {
            unreachable!("to_parse_table called on a non-Table NodeSpec");
        };
        let rows = *rows;
        let cols = *cols;
        // Bands can't exceed the row count; the body absorbs the rest.
        let header = (*header_rows).min(rows);
        let footer = (*footer_rows).min(rows.saturating_sub(header));
        let body = rows.saturating_sub(header).saturating_sub(footer);

        let parse_rows: Vec<paged_model::TableRow> = (0..rows)
            .map(|r| paged_model::TableRow {
                name: Some(r.to_string()),
                single_row_height: row_heights.get(r as usize).copied(),
                ..Default::default()
            })
            .collect();
        let parse_columns: Vec<paged_model::TableColumn> = (0..cols)
            .map(|c| paged_model::TableColumn {
                name: Some(c.to_string()),
                single_column_width: column_widths.get(c as usize).copied(),
                ..Default::default()
            })
            .collect();
        // Column-major document order: all cells in column 0, then 1, ….
        let mut cells: Vec<paged_model::TableCell> = Vec::with_capacity((rows * cols) as usize);
        for c in 0..cols {
            for r in 0..rows {
                cells.push(paged_model::TableCell {
                    name: Some(format!("{c}:{r}")),
                    row_span: 1,
                    column_span: 1,
                    ..Default::default()
                });
            }
        }
        paged_model::Table {
            self_id: Some(self_id.clone()),
            header_row_count: header,
            footer_row_count: footer,
            body_row_count: body,
            column_count: cols,
            rows: parse_rows,
            columns: parse_columns,
            cells,
            ..Default::default()
        }
    }
}

/// Wire-format description of a colour swatch (`<Color>`), mirroring
/// the editable fields of `paged_model::ColorEntry` with primitive,
/// `Deserialize`-able types (the AST `ColorEntry` is `Serialize`-only).
/// Carried by the swatch-collection mutations so create / edit /
/// delete-undo are lossless. `space` / `model` / `alternate_space` are
/// the IDML attribute strings (`ColorSpace::as_attr` etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SwatchSpec {
    /// IDML `Self` id. `None` on create ⇒ the apply layer assigns a
    /// deterministic non-colliding `Color/u<n>`.
    #[serde(default)]
    pub self_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// `Space` attribute: `"CMYK"` | `"RGB"` | `"LAB"` | `"Gray"`.
    pub space: String,
    /// Channel values in `space` (4 for CMYK, 3 for RGB/Lab, 1 for Gray).
    pub value: Vec<f32>,
    /// `Model`: `"Process"` (default) | `"Spot"`.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub alternate_space: Option<String>,
    #[serde(default)]
    pub alternate_value: Vec<f32>,
    #[serde(default)]
    pub tint: Option<f32>,
    #[serde(default)]
    pub alpha: Option<f32>,
}

/// Which style collection a `SetStyleProperty` targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum StyleCollection {
    Paragraph,
    Character,
    Object,
    Cell,
    Table,
}

/// One stop of a gradient on the wire. Mirrors `GradientStopRef`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GradientStopSpec {
    /// `Color/<id>` reference for this stop.
    pub stop_color: String,
    /// 0..=100 position along the ramp.
    pub location_pct: f32,
    /// 0..=100 midpoint to the next stop; `None` ⇒ linear (50).
    #[serde(default)]
    pub midpoint_pct: Option<f32>,
}

/// B-04 — creation spec for a page-item group. Members are NodeIds
/// of page items: leaf shapes OR (v2 / W1.20) existing `Group`s, so
/// `createGroup` can nest a group-of-groups. The apply layer resolves
/// them to `FrameRef`s, orders them by current document order, and
/// performs the `frames_in_order` surgery so z-order is provably
/// unchanged (the new group takes the slot of its topmost member —
/// the InDesign semantic, identical to the flat v1 rule). `self_id`
/// follows the page-item `u<hex>` convention (minted when absent;
/// echoed resolved in the applied op so the wire reports `createdId`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GroupSpec {
    #[serde(default)]
    pub self_id: Option<String>,
    pub members: Vec<NodeId>,
    /// W1.20 inverse-only — when the group being (re)created is NESTED
    /// inside a parent group, this carries `(parent_group_id,
    /// index_in_parent_members)` so `apply_create_group` re-nests it
    /// into the parent's `members` at the exact slot (rather than the
    /// default top-level `frames_in_order` placement). Wire callers
    /// creating a fresh top-level group omit it; it is filled by the
    /// `DissolveGroup` inverse so undo of a nested ungroup restores the
    /// parent→child link bytewise. `members` is likewise the captured
    /// `Group`'s own member NodeIds, so the group's transform + member
    /// order survive the round-trip.
    #[serde(default)]
    pub parent: Option<NestedParent>,
    /// W1.20 inverse-only — the group's own `ItemTransform` to restore
    /// on re-creation (a nested group carries its own transform, which
    /// a fresh top-level create never has). `None` ⇒ identity.
    #[serde(default)]
    pub item_transform: Option<[f32; 6]>,
}

/// W1.20 — `(parent_group_id, index_within_parent_members)` carried by
/// a `GroupSpec` when a group must be (re)created nested inside another
/// group rather than at the spread's top level. Inverse-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct NestedParent {
    pub group_id: String,
    pub index: u32,
}

/// Wire description of a gradient swatch, mirroring `GradientEntry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GradientSpec {
    #[serde(default)]
    pub self_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// `Type`: `"Linear"` | `"Radial"`.
    pub kind: String,
    pub stops: Vec<GradientStopSpec>,
}

/// W1.22 (engine gap 22) — wire description of a `<NumberingList>`
/// resource, mirroring `paged_model::NumberingListDef`. The
/// CRUD ops (`CreateNumberingList` / `EditNumberingList` /
/// `DeleteNumberingList`) carry this. `self_id` is minted
/// (`NumberingList/u<n>`) when absent on create; echoed resolved in
/// the applied op. `continue_across_stories` is the field the renderer
/// reads for cross-story numbering continuity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct NumberingListSpec {
    #[serde(default)]
    pub self_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// `ContinueNumbersAcrossStories`. `None` ⇒ false (default).
    #[serde(default)]
    pub continue_across_stories: Option<bool>,
    /// `ContinueNumbersAcrossDocuments` (round-trip only). `None` ⇒ false.
    #[serde(default)]
    pub continue_across_documents: Option<bool>,
}

/// Wire description of a colour group, mirroring `ColorGroupEntry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ColorGroupSpec {
    #[serde(default)]
    pub self_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// `Color/<id>` (or `Swatch/<id>`) member refs, in order.
    #[serde(default)]
    pub members: Vec<String>,
}

/// The canonical mutation primitive. A closed set, extended only with
/// deliberation. Collection mutations (swatches, styles) operate on the
/// document's `BTreeMap` palettes/stylesheets rather than the scene
/// tree, so they're top-level variants rather than `InsertNode`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(tag = "kind")]
pub enum Operation {
    SetProperty {
        node: NodeId,
        path: PropertyPath,
        value: Value,
    },
    InsertNode {
        parent: NodeId,
        position: usize,
        node: NodeSpec,
        /// Editor-ops — slot in the spread's `frames_in_order` z-order
        /// table. `None` ⇒ on top (new creations). `Some(slot)` is set
        /// by the `RemoveNode` inverse so undo-of-delete restores the
        /// exact stacking position, not just the kind-vec position.
        /// Ignored on spreads whose `frames_in_order` is empty (the
        /// renderer's legacy vec-walk fallback covers those).
        #[serde(default)]
        z_slot: Option<usize>,
    },
    RemoveNode {
        node: NodeId,
    },
    MoveNode {
        node: NodeId,
        new_parent: NodeId,
        position: usize,
    },
    Batch {
        ops: Vec<Operation>,
    },
    /// Editor-ops (Page tool) — insert a new SINGLE-PAGE SPREAD
    /// immediately after the spread hosting `after_page_id` (or at
    /// the end when `None`). Page size clones the reference page
    /// (Letter 612×792 fallback); `master_id` is applied when given.
    /// `spread_self_id` / `page_self_id` are normally `None` (the
    /// apply layer mints fresh ids) — they are filled on the op echo
    /// so redo re-creates the exact ids. `restore_spread_json` is
    /// inverse-only: the `RemovePage` undo carries the full captured
    /// spread (lossless, including every page item) and the apply
    /// layer reinserts it verbatim at its original index.
    ///
    /// Kept top-level (like the layer ops) rather than `InsertNode`:
    /// a new spread has no pre-existing parent `NodeId` to address.
    InsertPage {
        #[serde(default)]
        after_page_id: Option<String>,
        #[serde(default)]
        master_id: Option<String>,
        #[serde(default)]
        spread_self_id: Option<String>,
        #[serde(default)]
        page_self_id: Option<String>,
        #[serde(default)]
        restore_spread_json: Option<String>,
    },
    /// Editor-ops (Page tool) — remove the page `page_id`. v1
    /// supports single-page spreads only (the hosting spread is
    /// removed wholesale and captured for undo); deleting a page out
    /// of a multi-page spread, or the document's only page, is
    /// rejected with `InvalidValue`.
    RemovePage {
        page_id: String,
    },
    /// Track M — reorder a layer to a new zero-based index in
    /// `designmap.layers`. Inverse moves it back. Layer-affecting
    /// op kept top-level (rather than `MoveNode { node: Layer }`)
    /// because layers don't sit under a NodeId parent — they live
    /// in the DesignMap vec.
    MoveLayer {
        layer_id: String,
        new_index: usize,
    },
    /// Track M — insert a new layer at `position` with `name`. When
    /// `self_id` is `None` the apply layer assigns one
    /// deterministically (`Layer/u<seq>`); when `Some` it's used
    /// verbatim so the RemoveLayer inverse can restore an exact id
    /// (including the layer's original `visible/locked/printable`
    /// flags via a Batch).
    InsertLayer {
        position: usize,
        name: String,
        #[serde(default)]
        self_id: Option<String>,
    },
    /// Track M — remove a layer. The apply layer captures the
    /// removed layer's full state for the inverse so undo restores
    /// name + flags + position bytewise.
    RemoveLayer {
        layer_id: String,
    },
    /// Collection mutation — create a `<Color>` swatch in the document
    /// palette. When `spec.self_id` is `None` the apply layer assigns a
    /// deterministic `Color/u<n>`. Inverse: `DeleteSwatch`.
    CreateSwatch {
        spec: SwatchSpec,
    },
    /// Collection mutation — replace a swatch's editable fields
    /// (colour, name, model, …) in place. `swatch_id` is the target's
    /// `Self`; `spec.self_id` is ignored. Covers rename (edit with a
    /// new name). Inverse: `EditSwatch` carrying the prior spec.
    EditSwatch {
        swatch_id: String,
        spec: SwatchSpec,
    },
    /// Collection mutation — delete a swatch. The apply layer captures
    /// the full entry so the inverse (`CreateSwatch`) restores it
    /// losslessly at its original id.
    DeleteSwatch {
        swatch_id: String,
    },
    /// Collection mutation — create a paragraph style. The editor sends
    /// `name` / `based_on` (the apply layer builds a default def, the
    /// rest inheriting via the cascade). `restore_json` is **inverse-
    /// only**: the `DeleteParagraphStyle` inverse fills it with the
    /// serialized captured def so undo is lossless; when present, the
    /// other fields are ignored. Inverse: `DeleteParagraphStyle`.
    CreateParagraphStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
        #[serde(default)]
        restore_json: Option<String>,
    },
    /// Collection mutation — rename a paragraph style. Inverse restores
    /// the prior name.
    RenameParagraphStyle {
        style_id: String,
        name: String,
    },
    /// Collection mutation — delete a paragraph style. Inverse:
    /// `CreateParagraphStyle` carrying the captured def (`restore_json`).
    DeleteParagraphStyle {
        style_id: String,
    },
    /// Character-style analogue of `CreateParagraphStyle`.
    CreateCharacterStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
        #[serde(default)]
        restore_json: Option<String>,
    },
    /// Character-style analogue of `RenameParagraphStyle`.
    RenameCharacterStyle {
        style_id: String,
        name: String,
    },
    /// Character-style analogue of `DeleteParagraphStyle`.
    DeleteCharacterStyle {
        style_id: String,
    },
    /// Object-style analogue of `CreateParagraphStyle`.
    CreateObjectStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
        #[serde(default)]
        restore_json: Option<String>,
    },
    RenameObjectStyle {
        style_id: String,
        name: String,
    },
    DeleteObjectStyle {
        style_id: String,
    },
    /// Cell-style analogue of `CreateParagraphStyle`.
    CreateCellStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
        #[serde(default)]
        restore_json: Option<String>,
    },
    RenameCellStyle {
        style_id: String,
        name: String,
    },
    DeleteCellStyle {
        style_id: String,
    },
    /// Table-style analogue of `CreateParagraphStyle`.
    CreateTableStyle {
        #[serde(default)]
        self_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        based_on: Option<String>,
        #[serde(default)]
        restore_json: Option<String>,
    },
    RenameTableStyle {
        style_id: String,
        name: String,
    },
    DeleteTableStyle {
        style_id: String,
    },
    /// Collection mutation — create a gradient swatch. `spec.self_id`
    /// `None` ⇒ assigned `Gradient/u<n>`. Inverse: `DeleteGradient`.
    /// B-04 — group LEAF page items on one spread. Reference-based:
    /// members stay in their per-kind vecs; a `Group` entry joins
    /// `spread.groups` and `frames_in_order` swaps the member entries
    /// for one `FrameRef::Group` at the earliest member's slot.
    /// Members CONTIGUOUS in z-order group paint-neutrally; scattered
    /// members deterministically collect at the earliest slot (the
    /// InDesign semantic). Inverse: `DissolveGroup` carrying the
    /// members' original slots so undo restores z-order exactly.
    CreateGroup {
        spec: GroupSpec,
    },
    /// B-04 — dissolve a group: members return to `frames_in_order`
    /// at the group's slot in stored order (or, when `restore_slots`
    /// is carried by an undo inverse, at their exact pre-group
    /// indices); the `Group` entry is removed (with a
    /// `FrameRef::Group` index fix-up across the spread). Inverse:
    /// `CreateGroup` with the captured spec.
    DissolveGroup {
        group_id: String,
        /// Snapshot-inverse data (cf. `prev_anchors`): the members'
        /// `frames_in_order` indices before grouping. Wire callers
        /// omit it.
        #[serde(default)]
        restore_slots: Option<Vec<u32>>,
    },
    /// W1.20 (groups v2) — move/scale/rotate a group AS A UNIT. Unlike
    /// the v1 `SetProperty(Group, FrameTransform)` arm (which stores
    /// only the group's own `ItemTransform` and relies on the editor to
    /// pair it with per-leaf rebase ops in a Batch), this op does the
    /// whole composition atomically: it sets the group's own transform
    /// to `transform` AND rebases every descendant's EFFECTIVE
    /// `item_transform` by the delta `transform * inv(prev)` so the
    /// members follow the group rigidly (renderer + hit-test agree —
    /// both read each leaf's pre-baked effective transform). Nested
    /// child groups' own transforms ride the delta too. Inverse: the
    /// same op carrying the captured `prev` as the new transform.
    SetGroupTransform {
        group: String,
        /// New group-local `ItemTransform` `[a, b, c, d, tx, ty]`;
        /// `None` ⇒ identity.
        #[serde(default)]
        transform: Option<[f32; 6]>,
        /// Inverse-only: the group's transform before this op. Wire
        /// callers omit it; the apply layer captures it for the
        /// inverse so undo restores the prior geometry exactly.
        #[serde(default)]
        prev: Option<[f32; 6]>,
    },
    CreateGradient {
        spec: GradientSpec,
    },
    /// Replace a gradient's stops / kind / name in place. Inverse:
    /// `EditGradient` carrying the prior spec.
    EditGradient {
        gradient_id: String,
        spec: GradientSpec,
    },
    /// Delete a gradient; inverse `CreateGradient` restores it.
    DeleteGradient {
        gradient_id: String,
    },
    /// Collection mutation — create a colour group. Inverse:
    /// `DeleteColorGroup`.
    CreateColorGroup {
        spec: ColorGroupSpec,
    },
    /// Replace a colour group's name / members in place. Inverse:
    /// `EditColorGroup` carrying the prior spec.
    EditColorGroup {
        group_id: String,
        spec: ColorGroupSpec,
    },
    /// Delete a colour group; inverse `CreateColorGroup` restores it.
    DeleteColorGroup {
        group_id: String,
    },
    /// W1.22 (engine gap 22) — create a `<NumberingList>` resource.
    /// Inverse: `DeleteNumberingList`. // rides v35 (added before first
    /// consumer sync — v35 bumped in W1.23 but is not yet tagged /
    /// published; highest tag is v0.34.0).
    CreateNumberingList {
        spec: NumberingListSpec,
    },
    /// W1.22 — replace a numbering list's name / continuity flags in
    /// place. Inverse: `EditNumberingList` carrying the prior spec.
    /// // rides v35.
    EditNumberingList {
        list_id: String,
        spec: NumberingListSpec,
    },
    /// W1.22 — delete a numbering list; inverse `CreateNumberingList`
    /// restores it. // rides v35.
    DeleteNumberingList {
        list_id: String,
    },
    /// Style-options editing — set one property on a *style definition*
    /// (not the selection). Reuses the `PropertyPath` + `Value`
    /// vocabulary of `SetProperty`, so the style-editor panel renders
    /// with the same primitive leaves as the Character / Paragraph
    /// panels (per the panel-catalog plan §5.3). `collection` picks the
    /// target stylesheet; `style_id` the entry. Inverse carries the
    /// prior value. Paragraph + character defs are covered; object /
    /// cell / table style property editing is a follow-up.
    SetStyleProperty {
        collection: StyleCollection,
        style_id: String,
        path: PropertyPath,
        value: Value,
    },
    /// SDK Phase 5 (v1 sweep) — multi-target Bezier boolean op.
    /// `kept` is the survivor (its path is replaced with the
    /// result); `others` are the inputs that disappear. For
    /// Subtract, `kept` is the "top" path being subtracted from;
    /// `others` are subtracted. The apply layer:
    ///   1. Reads each input's path (anchors + subpath_starts).
    ///   2. Runs `pathfinder::pathfinder_boolean` (flo_curves
    ///      curve-preserving CSG; output is real Bezier curves).
    ///   3. Builds an internal Batch:
    ///      `SetProperty(kept, FramePath, result)` +
    ///      `RemoveNode(other)` per other.
    ///   4. Applies the Batch; returns it as the AppliedOperation
    ///      so undo reverses the whole pathfinder in one Cmd-Z.
    PathfinderBoolean {
        kept: NodeId,
        others: Vec<NodeId>,
        // `kind` is reserved by serde for the enum discriminator
        // tag (`#[serde(tag = "kind")]` above) — use `opKind` on
        // the wire to disambiguate.
        #[serde(rename = "opKind")]
        op_kind: PathfinderKind,
    },
    /// W0.5 — thread two text frames: rewrite `from`'s
    /// `NextTextFrame` to point at `to` so the story reflows into
    /// `to` on overflow. Validation (apply): both frames exist; `to`
    /// owns no story content of its own (InDesign only threads into
    /// empty frames); the link must not create a cycle (walk the
    /// existing chain). Invalidation: the story reflows. Inverse:
    /// `UnlinkFrames { frame: from }` (with the prior `next_text_frame`
    /// captured so undo restores any pre-existing link target).
    LinkFrames {
        from: String,
        to: String,
    },
    /// W0.5 — break the thread leaving `frame`: clear its
    /// `NextTextFrame`. Inverse re-links to the captured prior target
    /// via `LinkFrames`. The `prev_next` field is **inverse-only** —
    /// when set, `apply` restores `frame.next_text_frame` to it
    /// instead of clearing (so `UnlinkFrames` can serve as
    /// `LinkFrames`'s undo without a separate variant).
    UnlinkFrames {
        frame: String,
        #[serde(default)]
        prev_next: Option<String>,
    },
    /// W0.5 — apply a named paragraph or character style to a story
    /// range. Delegates to the same run/paragraph splitter as
    /// `SetProperty(AppliedCharacterStyle / AppliedParagraphStyle)`;
    /// the inverse is a `Batch` of per-segment style restorations the
    /// splitter captures. `scope` picks character- vs paragraph-level.
    ApplyStyle {
        story_id: String,
        start: u32,
        end: u32,
        /// `ParagraphStyle/<id>` or `CharacterStyle/<id>` ref.
        style: String,
        scope: StyleScope,
    },
    /// W0.5 — insert a field marker (e.g. the auto current-page-number
    /// marker, U+E018) into a story at a character offset. v1 supports
    /// `PageNumber` only; `field` is extensible. Implemented as a
    /// single-character text insertion, so the inverse is a
    /// `DeleteRange` of that one character.
    InsertField {
        story_id: String,
        offset: u32,
        field: FieldKind,
    },
    /// W0.5 — inverse-only companion to `InsertField`: remove the
    /// single field-marker character at `offset` (for a
    /// `FieldKind::Placeholder`, remove the whole tagged run starting
    /// at `offset`). Inverse re-inserts it via `InsertField`; for a
    /// placeholder the inverse captures the run's CURRENT value so a
    /// delete after re-resolution undoes faithfully.
    DeleteField {
        story_id: String,
        offset: u32,
        field: FieldKind,
    },
    /// v43 (D-01) — update the cached display value of the
    /// `FieldKind::Placeholder` run containing the story char
    /// `offset` (offsets come fresh from the
    /// `RequestDocumentPlaceholders` read door; the echoed op and its
    /// inverse are normalised to the run's START offset). `value:
    /// None` returns the field to its unresolved `<key>` display.
    /// Re-resolving is ONE undoable step: the inverse carries the
    /// prior value. The hosting story reflows.
    SetFieldValue {
        story_id: String,
        offset: u32,
        #[serde(default)]
        value: Option<String>,
    },
    /// v43 (D-14) — place (or clear) an image asset on a graphic
    /// frame. `frame` must be a Rectangle / Oval / Polygon. Sets the
    /// frame's `image_link` (the same `LinkResourceURI` lane parsed
    /// placed images use — the renderer resolves it through
    /// `AssetResolver::resolve_image` and renders the image iff the
    /// resolver serves the uri; an unreachable uri leaves the frame
    /// rendering exactly as before). `image_uri: None` clears the
    /// link (the inverse lane for place-onto-empty). `fit` writes the
    /// IDML `FittingOnEmptyFrame` vocabulary (`Proportionally` |
    /// `FillProportionally` | `FitContentToFrame` | `ContentAwareFit`;
    /// the same strings `PropertyPath::FrameFittingType` reads/writes,
    /// Rectangle-only — IDML nests `<FrameFittingOption>` only there):
    /// `Some("")` clears, `None` leaves the fitting untouched. Inverse
    /// restores the prior link/fit.
    PlaceImage {
        frame: NodeId,
        #[serde(default)]
        image_uri: Option<String>,
        #[serde(default)]
        fit: Option<String>,
    },
    /// C-1 Stage B (pixel save-back) — replace a placed graphic frame's
    /// INLINE image bytes (the decoded `image_bytes` lane the renderer
    /// prefers over a `<Link>` uri). The companion to the per-drag
    /// `SubmitPixelLayer` preview: where that ephemerally composites tiles
    /// over the frame during a gesture, this COMMITS the processed result
    /// as a single undoable document mutation. `frame` must be a Rectangle
    /// / Oval / Polygon. `bytes: Some(_)` installs the new inline payload
    /// (typically a freshly-encoded PNG/JPEG from the plugin pipeline) and
    /// marks the frame an image element (`has_image_element = true`) so it
    /// renders even when no `<Image>` was parsed; `bytes: None` clears the
    /// inline payload (the save-back-of-a-delete lane). The apply layer
    /// captures the prior bytes + prior `has_image_element` so the inverse
    /// (another `ReplaceImageBytes`, carrying `prior_has_image_element`)
    /// restores them losslessly — was-absent restores to absent. Does NOT
    /// touch `image_link` / `image_item_transform`: bytes outrank the link
    /// in the renderer (the same precedence parsed inline-CDATA images
    /// get), and the transform stays so the bytes land in the same place.
    ReplaceImageBytes {
        frame: NodeId,
        #[serde(default)]
        #[tsify(type = "number[] | null")]
        bytes: Option<Vec<u8>>,
        /// Inverse-only: the frame's `has_image_element` flag BEFORE the
        /// op, restored by the inverse. `None` on a forward op (the apply
        /// layer always sets it true; the inverse carries the captured
        /// prior). A forward op MAY also set it explicitly — the apply
        /// layer treats `None` as "true" for the forward direction.
        #[serde(default)]
        prior_has_image_element: Option<bool>,
    },
    /// W0.5 — insert a ruler guide on the spread `spread_id`.
    /// `position` is the page-local coordinate on the perpendicular
    /// axis (x for Vertical, y for Horizontal); `page_index` is the
    /// zero-based page within the spread. `guide_id` is normally
    /// `None` (apply mints a deterministic `Guide/u<n>` recorded on
    /// the op echo); set verbatim by the inverse so redo re-creates
    /// the same id. Inverse: `DeleteGuide`.
    InsertGuide {
        spread_id: String,
        orientation: GuideOrientationSpec,
        position: f32,
        #[serde(default)]
        page_index: u32,
        #[serde(default)]
        guide_id: Option<String>,
    },
    /// W0.5 — move an existing guide to a new perpendicular-axis
    /// position. Inverse carries the prior position.
    MoveGuide {
        guide_id: String,
        position: f32,
    },
    /// W0.5 — delete a guide. Apply captures the full guide for the
    /// inverse (`InsertGuide` restores it at its original id/spread).
    DeleteGuide {
        guide_id: String,
    },
    /// W0.5 — flip a `<Condition>`'s `Visible` flag in the document
    /// condition table. Conditional text changes layout, so the whole
    /// document reflows. Inverse carries the prior visibility.
    SetConditionVisible {
        condition: String,
        visible: bool,
    },
    /// W0.5 — make every condition referenced by the named
    /// `<ConditionSet>` visible and every other condition hidden (the
    /// "show only this set" affordance). Apply captures the full prior
    /// visibility map so the inverse (`RestoreConditionVisibility`)
    /// can undo it in one step.
    ActivateConditionSet {
        set: String,
    },
    /// W0.5 — inverse-only companion to `ActivateConditionSet`:
    /// restore each listed condition's prior `Visible` flag.
    RestoreConditionVisibility {
        /// `(condition_id, prior_visible)` pairs.
        states: Vec<(String, bool)>,
    },
    /// W0.5 — set a page's `AppliedMaster` ref. `None` detaches the
    /// master ([None]). Inverse carries the prior master ref.
    ApplyMasterToPage {
        page: String,
        #[serde(default)]
        master: Option<String>,
    },
    /// W0.5 — duplicate a single-page spread (the page plus every page
    /// item) immediately after the source, minting fresh self ids for
    /// the clone. Inverse: `RemovePage` of the cloned page.
    /// `clone_spread_json` is **echo/redo-only** — the apply layer
    /// fills it with the materialised clone so redo re-creates the
    /// exact ids and geometry.
    DuplicatePage {
        page: String,
        #[serde(default)]
        clone_spread_json: Option<String>,
    },
    /// W0.5 — insert a `<Section>` anchored at `at_page`. Inverse:
    /// `DeleteSection`. `self_id` is minted when `None` and echoed.
    InsertSection {
        at_page: String,
        #[serde(default)]
        prefix: Option<String>,
        #[serde(default)]
        numbering_style: Option<String>,
        #[serde(default)]
        start_at: Option<u32>,
        #[serde(default)]
        self_id: Option<String>,
    },
    /// W0.5 — edit fields of an existing `<Section>`. Each `Some`
    /// field overwrites; `None` leaves the field unchanged. Inverse
    /// carries the prior values as a full `EditSection`.
    EditSection {
        section_id: String,
        /// `Some(_)` overwrites `section_prefix` (inner `None` clears
        /// it); outer `None` leaves the prefix unchanged. The
        /// `double_option` deserialiser preserves the `Some(None)` vs
        /// `None` distinction a plain `#[serde(default)]` would lose.
        #[serde(default, deserialize_with = "double_option::deserialize")]
        prefix: Option<Option<String>>,
        #[serde(default)]
        numbering_style: Option<String>,
        /// Same double-option semantics as `prefix`.
        #[serde(default, deserialize_with = "double_option::deserialize")]
        start_at: Option<Option<u32>>,
    },
    /// W0.5 — inverse-only companion to `InsertSection`: remove the
    /// section by id. Inverse re-inserts it via `InsertSection` with
    /// the captured fields.
    DeleteSection {
        section_id: String,
    },

    // ---- W3.A1 — table structure --------------------------------
    // Indexed, structural table edits. Kept as top-level Operations
    // (not `SetProperty`) because `PropertyPath` is a fieldless enum
    // that can't carry a row/col index, and because row/column
    // insert/delete reshapes the cell grid (every higher row index
    // shifts) — the LinkFrames / InsertGuide precedent for
    // index-carrying structural ops. All address the table by
    // `(story_id, table_id)`; all reflow the host story.
    /// W3.A1 — set row `row`'s `SingleRowHeight` to `height` pt.
    /// `None` clears the per-row override (the row grows to fit
    /// content). Inverse carries the prior height.
    SetRowHeight {
        story_id: String,
        table_id: String,
        row: u32,
        #[serde(default)]
        height: Option<f32>,
    },
    /// W3.A1 — set column `col`'s `SingleColumnWidth` to `width` pt.
    /// `None` clears the override. Inverse carries the prior width.
    SetColumnWidth {
        story_id: String,
        table_id: String,
        col: u32,
        #[serde(default)]
        width: Option<f32>,
    },
    /// W3.A1 — insert an empty body row at index `at` (0-based,
    /// clamped to `[0, body_row_count]`). Cells in rows ≥ `at` shift
    /// down by one; `body_row_count` increments; a fresh empty cell
    /// is minted per column. `restore` is **inverse-only**: the
    /// `DeleteTableRow` undo fills it with the captured removed row so
    /// re-insertion is byte-identical. Inverse: `DeleteTableRow` at
    /// the same index.
    InsertTableRow {
        story_id: String,
        table_id: String,
        at: u32,
        #[serde(default)]
        restore: Option<TableLineRestoreJson>,
    },
    /// W3.A1 — delete the row at index `at`. Apply captures the row's
    /// declaration + every cell originating in it into the inverse's
    /// `restore` blob so undo (`InsertTableRow { restore }`) is
    /// lossless. Cells in rows > `at` shift up by one. Rejected when
    /// `at` is out of range or the table has only one row.
    DeleteTableRow {
        story_id: String,
        table_id: String,
        at: u32,
    },
    /// W3.A1 — insert an empty column at index `at`. Cells in columns
    /// ≥ `at` shift right; `column_count` increments. `restore` is
    /// inverse-only (see `InsertTableRow`).
    InsertTableColumn {
        story_id: String,
        table_id: String,
        at: u32,
        #[serde(default)]
        restore: Option<TableLineRestoreJson>,
    },
    /// W3.A1 — delete the column at index `at`. Captures the column +
    /// its cells for the inverse, like `DeleteTableRow`.
    DeleteTableColumn {
        story_id: String,
        table_id: String,
        at: u32,
    },

    // ── W1.12a — header / footer row inserts (ride v35) ─────────────
    // IDML's `<Table HeaderRowCount="…" FooterRowCount="…">` carves the
    // row sequence into a header band (the first N rows), the body, and
    // a footer band (the last M rows). Header / footer rows replay
    // across NextTextFrame breaks (see `pipeline/tables.rs`). These ops
    // grow / shrink those bands. Insert mints an empty row at the band
    // boundary and bumps the band count; the inverse removes it. All
    // ride v35 — additive `Operation` variants on the unpublished
    // protocol (the W1.22 list-definition precedent).
    /// W1.12a — insert an empty row at the TOP of the header band
    /// (header index 0; existing header rows + the whole body shift
    /// down by one). `header_row_count` increments. Inverse:
    /// `RemoveHeaderRow`. `restore` is inverse-only (the `RemoveHeaderRow`
    /// undo re-inserts the captured row, like `InsertTableRow`).
    InsertHeaderRow {
        story_id: String,
        table_id: String,
        #[serde(default)]
        restore: Option<TableLineRestoreJson>,
    },
    /// W1.12a — remove the FIRST header row (the top of the table).
    /// Captures it for the inverse (`InsertHeaderRow { restore }`).
    /// `header_row_count` decrements. Rejected when the table has no
    /// header rows.
    RemoveHeaderRow {
        story_id: String,
        table_id: String,
    },
    /// W1.12a — insert an empty row at the BOTTOM of the footer band
    /// (after the last existing row). `footer_row_count` increments.
    /// Inverse: `RemoveFooterRow`.
    InsertFooterRow {
        story_id: String,
        table_id: String,
        #[serde(default)]
        restore: Option<TableLineRestoreJson>,
    },
    /// W1.12a — remove the LAST footer row (the bottom of the table).
    /// Captures it for the inverse. `footer_row_count` decrements.
    /// Rejected when the table has no footer rows.
    RemoveFooterRow {
        story_id: String,
        table_id: String,
    },

    // ── W1.12b — merge / split spans (ride v35) ─────────────────────
    /// W1.12b — set the `RowSpan` / `ColumnSpan` of the cell originating
    /// at `(row, col)`. Merging (`row_span` / `column_span` > 1) makes
    /// the cell cover the slots below / to the right of it — InDesign's
    /// "Merge Cells". Splitting back to `(1, 1)` is "Unmerge". The
    /// inverse carries the prior `(row_span, column_span)` so undo
    /// restores the exact prior spans. The renderer already widens /
    /// lengthens a cell's rect by its spans (`pipeline/tables.rs`), so a
    /// span change is immediately visible once the host story reflows.
    /// Cells the new span newly covers are NOT removed from the cell
    /// list (the renderer skips slots a span already painted); pruning
    /// covered cells is a future-fidelity follow-up.
    SetCellSpan {
        story_id: String,
        table_id: String,
        row: u32,
        col: u32,
        row_span: u32,
        column_span: u32,
    },
}

/// W0.5 — character- vs paragraph-level style application for
/// [`Operation::ApplyStyle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum StyleScope {
    Paragraph,
    Character,
}

/// W0.5 — the kind of field marker inserted by
/// [`Operation::InsertField`]. v1 implemented the two page-number
/// built-ins (single private-use marker chars the renderer
/// substitutes); v43 (D-01) adds the plugin `Placeholder` — a tagged,
/// edit-surviving anchor run whose text is the field's cached display
/// value (see `paged_model::PlaceholderField`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    PageNumber,
    NextPageNumber,
    /// v43 (D-01) — a plugin-owned tagged placeholder. Inserts a
    /// dedicated run tagged `(plugin, key)` whose text displays
    /// `value` (or the `<key>` token while `value` is `None`); the
    /// engine never re-resolves it — `Operation::SetFieldValue`
    /// updates the cached display.
    Placeholder {
        plugin: String,
        key: String,
        #[serde(default)]
        value: Option<String>,
    },
}

impl FieldKind {
    /// The Unicode marker char the parser uses to represent this field
    /// in a story's flattened text (mirrors
    /// `paged_model::AUTO_PAGE_NUMBER_MARKER` etc.). `None` for
    /// `Placeholder` — that field is a tagged run carrying its display
    /// text, not a single substituted marker char.
    pub fn marker_char(&self) -> Option<char> {
        match self {
            // U+E018 — IDML `<?ACE 18?>` auto current-page-number.
            FieldKind::PageNumber => Some('\u{E018}'),
            // U+E019 — IDML `<?ACE 19?>` next-page-number marker.
            FieldKind::NextPageNumber => Some('\u{E019}'),
            FieldKind::Placeholder { .. } => None,
        }
    }
}

/// W0.5 — wire mirror of `paged_model::GuideOrientation`
/// (which is `Deserialize` but lives in the parse crate; kept here so
/// the operation wire type doesn't depend on the parser's
/// serialization shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum GuideOrientationSpec {
    Vertical,
    Horizontal,
}

impl GuideOrientationSpec {
    pub fn to_parse(self) -> paged_model::GuideOrientation {
        match self {
            GuideOrientationSpec::Vertical => paged_model::GuideOrientation::Vertical,
            GuideOrientationSpec::Horizontal => paged_model::GuideOrientation::Horizontal,
        }
    }
    pub fn from_parse(o: paged_model::GuideOrientation) -> Self {
        match o {
            paged_model::GuideOrientation::Vertical => GuideOrientationSpec::Vertical,
            paged_model::GuideOrientation::Horizontal => GuideOrientationSpec::Horizontal,
        }
    }
}

/// SDK Phase 5 (v1 sweep) — wire enum for Pathfinder ops. Mirrors
/// `pathfinder::PathfinderKind` (the internal enum used by the
/// flo_curves layer) — kept separate so the apply layer doesn't
/// leak `flo_curves` types onto the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum PathfinderKind {
    Union,
    Intersect,
    Subtract,
    Exclude,
}

/// Hint to downstream caches about what the apply touched. Lists
/// instead of a single enum so a Batch aggregates by union without
/// losing per-node detail. Consumers (renderer, glyph cache, layout
/// cache) decide which lists to honour. Stays advisory — nothing in
/// `paged-mutate` invalidates anything itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct InvalidationHint {
    pub frame_geometry: Vec<NodeId>,
    pub frame_style: Vec<NodeId>,
    pub text_reflow: Vec<NodeId>,
    /// Set when the tree shape changed (any Insert/Remove/Move).
    pub structural: bool,
}

impl InvalidationHint {
    pub fn merge(&mut self, other: InvalidationHint) {
        self.frame_geometry.extend(other.frame_geometry);
        self.frame_style.extend(other.frame_style);
        self.text_reflow.extend(other.text_reflow);
        self.structural |= other.structural;
    }
}

/// Result of a successful `apply`. Holds the original op, the
/// pre-computed inverse op (ready to push onto an undo stack), and
/// the invalidation hint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
pub struct AppliedOperation {
    pub op: Operation,
    pub inverse: Operation,
    pub invalidation: InvalidationHint,
}

// ------------------------------------------------------------------
// W0.3 — ItemTransform decomposition (gap 6/16)
// ------------------------------------------------------------------

/// W0.3 — the rotation / scale / flip / translation that compose an
/// IDML `ItemTransform` `[a, b, c, d, tx, ty]`. The matrix maps a
/// point `(x, y)` to `(a·x + c·y + tx, b·x + d·y + ty)`.
///
/// The decomposition is the standard QR-style polar form for the
/// linear 2×2 block `[[a, c], [b, d]]`:
///
/// 1. `flip_h` is read from the sign of the determinant — a negative
///    determinant means the matrix includes a reflection. We fold the
///    whole reflection into the X axis (`flip_h`) and keep `flip_v`
///    addressable independently so the two editor toggles round-trip.
/// 2. `angle_deg` is `atan2(b, a)` of the first basis vector.
/// 3. `scale_x` is `‖(a, b)‖` (always ≥ 0; the sign lives in the
///    flip flags); `scale_y` is the height of the parallelogram
///    (`det / scale_x`), also taken as a magnitude.
/// 4. `shear` is the off-axis skew (`(a·c + b·d) / scale_x²`),
///    captured for round-trip fidelity but NOT exposed as a wire
///    path — a sheared frame's `scale_y`/`angle` are only meaningful
///    once the shear is re-applied on recompose.
///
/// `recompose` is the exact left-inverse for the shear-free, single-
/// flip case (`recompose(decompose(m)) == m`); when both flips are
/// set it normalises to the equivalent 180°-rotation form, which is
/// the same matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformDecomp {
    pub translate: [f32; 2],
    pub angle_deg: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub shear: f32,
    pub flip_h: bool,
    pub flip_v: bool,
}

/// Decompose an IDML `ItemTransform` into rotation / scale / flip /
/// translation. `None` (no transform) decomposes to the identity.
pub fn decompose_transform(m: Option<[f32; 6]>) -> TransformDecomp {
    let [a, b, c, d, tx, ty] = m.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let det = a * d - b * c;
    // A negative determinant = the matrix mirrors. Fold the whole
    // reflection into the X axis so the polar decomposition below
    // operates on a proper (det ≥ 0) rotation·scale.
    let flip_h = det < 0.0;
    let sign = if flip_h { -1.0 } else { 1.0 };
    // Apply the X reflection up front: (a, b) → (sign·a, sign·b).
    let (a2, b2) = (sign * a, sign * b);
    let scale_x = (a2 * a2 + b2 * b2).sqrt();
    let angle_deg = if scale_x == 0.0 {
        0.0
    } else {
        b2.atan2(a2).to_degrees()
    };
    // The de-reflected determinant is non-negative; scale_y is the
    // parallelogram height. scale_x == 0 is a degenerate matrix; guard
    // the division.
    let det2 = sign * det; // == a2·d - b2·c, always ≥ 0
    let scale_y = if scale_x == 0.0 { 0.0 } else { det2 / scale_x };
    let shear = if scale_x == 0.0 {
        0.0
    } else {
        (a2 * c + b2 * d) / (scale_x * scale_x)
    };
    TransformDecomp {
        translate: [tx, ty],
        angle_deg,
        scale_x,
        scale_y,
        shear,
        flip_h,
        // `flip_v` is not derivable from a proper decomposition (a
        // single reflection is fully captured by `flip_h`); it starts
        // `false` and is toggled by the `FrameFlipV` path, which
        // recompose honours by negating the Y scale.
        flip_v: false,
    }
}

/// Recompose an `ItemTransform` from a [`TransformDecomp`], preserving
/// the translation. Inverse of [`decompose_transform`] for the
/// shear-free single-flip case. Order:
/// `T · R(angle) · shear · diag(±scale_x, ±scale_y)`.
pub fn recompose_transform(t: &TransformDecomp) -> [f32; 6] {
    let rad = t.angle_deg.to_radians();
    let (sin, cos) = rad.sin_cos();
    let sx = if t.flip_h { -t.scale_x } else { t.scale_x };
    let sy = if t.flip_v { -t.scale_y } else { t.scale_y };
    // First column = R · (sx, 0): the scaled+rotated X basis.
    let a = cos * sx;
    let b = sin * sx;
    // Second column = R · (shear·sx? , sy): fold shear into the Y
    // basis. shear is expressed in pre-rotation X units, so the
    // pre-rotation second column is (shear·? , sy) → we reconstruct
    // it as the rotated (shear-along-X, sy) vector.
    let pre_cx = t.shear * sy;
    let c = cos * pre_cx - sin * sy;
    let d = sin * pre_cx + cos * sy;
    [a, b, c, d, t.translate[0], t.translate[1]]
}

#[cfg(test)]
mod transform_decompose_tests {
    use super::*;

    fn approx(m1: [f32; 6], m2: [f32; 6]) {
        for i in 0..6 {
            assert!(
                (m1[i] - m2[i]).abs() < 1e-3,
                "component {i}: {} vs {}\n{m1:?}\n{m2:?}",
                m1[i],
                m2[i]
            );
        }
    }

    #[test]
    fn identity_round_trips() {
        let d = decompose_transform(None);
        assert!((d.angle_deg).abs() < 1e-4);
        assert!((d.scale_x - 1.0).abs() < 1e-4);
        assert!((d.scale_y - 1.0).abs() < 1e-4);
        assert!(!d.flip_h && !d.flip_v);
        approx(recompose_transform(&d), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn pure_rotation_round_trips() {
        // 30° rotation, translation (10, 20).
        let rad = 30f32.to_radians();
        let (s, c) = rad.sin_cos();
        let m = [c, s, -s, c, 10.0, 20.0];
        let d = decompose_transform(Some(m));
        assert!((d.angle_deg - 30.0).abs() < 1e-2, "angle {}", d.angle_deg);
        assert!((d.scale_x - 1.0).abs() < 1e-3);
        assert!((d.scale_y - 1.0).abs() < 1e-3);
        assert!(!d.flip_h && !d.flip_v);
        assert_eq!(d.translate, [10.0, 20.0]);
        approx(recompose_transform(&d), m);
    }

    #[test]
    fn scale_and_rotation_round_trip() {
        // scale (2, 3) then rotate 45°, translate (5, -7).
        let rad = 45f32.to_radians();
        let (s, c) = rad.sin_cos();
        let (sx, sy) = (2.0f32, 3.0f32);
        let m = [c * sx, s * sx, -s * sy, c * sy, 5.0, -7.0];
        let d = decompose_transform(Some(m));
        assert!((d.angle_deg - 45.0).abs() < 1e-2);
        assert!((d.scale_x - 2.0).abs() < 1e-3, "sx {}", d.scale_x);
        assert!((d.scale_y - 3.0).abs() < 1e-3, "sy {}", d.scale_y);
        assert!(d.shear.abs() < 1e-3, "shear {}", d.shear);
        approx(recompose_transform(&d), m);
    }

    #[test]
    fn horizontal_flip_detected_via_negative_determinant() {
        // Mirror across the vertical axis: x → -x. det = -1.
        let m = [-1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let d = decompose_transform(Some(m));
        assert!(d.flip_h, "negative-det matrix must flag flip_h");
        assert!((d.scale_x - 1.0).abs() < 1e-4);
        assert!((d.scale_y - 1.0).abs() < 1e-4);
        // Round-trip recomposes the same reflection.
        approx(recompose_transform(&d), m);
    }

    #[test]
    fn vertical_flip_recompose() {
        // FrameFlipV toggles flip_v; recompose negates scale_y. Start
        // from identity, set flip_v, recompose → y-mirror matrix.
        let mut d = decompose_transform(None);
        d.flip_v = true;
        approx(recompose_transform(&d), [1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
        // Decomposing that y-mirror reads as a 180° rotation + flip_h
        // (a single reflection is folded into X) — the matrix is the
        // same either way, which is what round-trip fidelity needs.
        let re = decompose_transform(Some([1.0, 0.0, 0.0, -1.0, 0.0, 0.0]));
        approx(recompose_transform(&re), [1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
    }

    #[test]
    fn translation_preserved_on_recompose() {
        let m = [2.0, 0.0, 0.0, 2.0, 33.0, 44.0];
        let d = decompose_transform(Some(m));
        assert_eq!(d.translate, [33.0, 44.0]);
        let re = recompose_transform(&d);
        assert!((re[4] - 33.0).abs() < 1e-4 && (re[5] - 44.0).abs() < 1e-4);
    }
}
