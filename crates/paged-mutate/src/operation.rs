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
            | NodeId::Layer(s) => s,
            NodeId::StoryRange { story_id, .. } => story_id,
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
            NodeId::Layer(_) => "Layer",
            NodeId::StoryRange { .. } => "StoryRange",
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
            PropertyPath::FrameStrokeEndCap => "frame.strokeEndCap",
            PropertyPath::FrameTextWrapMode => "frame.textWrapMode",
            PropertyPath::FrameTextWrapOffsets => "frame.textWrapOffsets",
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
        }
    }
}

/// Track J — wire-shape mirror of `paged_parse::PathAnchor`. The
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
    pub fn from_parse(a: &paged_parse::PathAnchor) -> Self {
        Self {
            anchor: [a.anchor.0, a.anchor.1],
            left: [a.left.0, a.left.1],
            right: [a.right.0, a.right.1],
        }
    }
    pub fn to_parse(&self) -> paged_parse::PathAnchor {
        paged_parse::PathAnchor {
            anchor: (self.anchor[0], self.anchor[1]),
            left: (self.left[0], self.left[1]),
            right: (self.right[0], self.right[1]),
        }
    }
}

/// Editor-ops — wire mirror of `paged_parse::GradientFeatherStop`
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

/// Editor-ops — wire mirror of `paged_parse::GradientFeatherParams`.
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
    pub fn from_parse(p: &paged_parse::GradientFeatherParams) -> Self {
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
    pub fn to_parse(&self) -> paged_parse::GradientFeatherParams {
        paged_parse::GradientFeatherParams {
            gradient_type: self.gradient_type.clone(),
            start_point: self.start_point.map(|[x, y]| (x, y)),
            end_point: self.end_point.map(|[x, y]| (x, y)),
            angle_deg: self.angle_deg,
            stops: self
                .stops
                .iter()
                .map(|s| paged_parse::GradientFeatherStop {
                    stop_color: s.stop_color.clone(),
                    location_pct: s.location_pct,
                    alpha_pct: s.alpha_pct,
                    midpoint_pct: s.midpoint_pct,
                })
                .collect(),
        }
    }
}

/// W0.2 — wire mirror of `paged_parse::styles::ParagraphRule` (the
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
    pub fn from_parse(p: &paged_parse::styles::ParagraphRule) -> Self {
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
    pub fn to_parse(&self) -> paged_parse::styles::ParagraphRule {
        paged_parse::styles::ParagraphRule {
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

/// W0.2 — wire mirror of `paged_parse::TabStop`. The `ParagraphTabStops`
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
    pub fn from_parse(t: &paged_parse::TabStop) -> Self {
        Self {
            position: t.position,
            alignment: t.alignment.clone(),
            alignment_character: t.alignment_character.clone(),
            leader: t.leader.clone(),
        }
    }
    pub fn to_parse(&self) -> paged_parse::TabStop {
        paged_parse::TabStop {
            position: self.position,
            alignment: self.alignment.clone(),
            alignment_character: self.alignment_character.clone(),
            leader: self.leader.clone(),
        }
    }
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
}

impl NodeSpec {
    pub fn node_id(&self) -> NodeId {
        match self {
            NodeSpec::TextFrame { self_id, .. } => NodeId::TextFrame(self_id.clone()),
            NodeSpec::Rectangle { self_id, .. } => NodeId::Rectangle(self_id.clone()),
            NodeSpec::GraphicLine { self_id, .. } => NodeId::GraphicLine(self_id.clone()),
            NodeSpec::Polygon { self_id, .. } => NodeId::Polygon(self_id.clone()),
            NodeSpec::CloneTranslate { self_id, source, .. } => match source {
                NodeId::TextFrame(_) => NodeId::TextFrame(self_id.clone()),
                NodeId::Rectangle(_) => NodeId::Rectangle(self_id.clone()),
                // Other shape kinds aren't supported yet — apply.rs
                // raises UnsupportedProperty on them.
                _ => source.clone(),
            },
        }
    }
}

/// Wire-format description of a colour swatch (`<Color>`), mirroring
/// the editable fields of `paged_parse::ColorEntry` with primitive,
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
