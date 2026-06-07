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

//! Typed message channel between the React main thread and the
//! canvas Web Worker.
//!
//! Envelopes are versioned serde structs with externally-tagged
//! enums (`kind`/`payload`) so the TS side can switch on `kind`
//! without knowing the Rust enum representation. Every message
//! carries a `seq` for ordering / acknowledgement bookkeeping;
//! the main thread is the source of `seq` for its outgoing
//! messages, the worker echoes `seq` back in the corresponding
//! `WorkerToMain`.
//!
//! Camera updates are NOT in this channel — they go through the
//! `SharedArrayBuffer` defined in `crate::camera` for sub-frame
//! latency.

use paged_renderer::PageId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tsify_next::Tsify;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::model::{DocumentHandle, DocumentStats};

// MainToWorker / WorkerToMain are `#[serde(flatten)]`-style structs
// over a discriminated union (`MainToWorkerKind` / `WorkerToMainKind`).
// Tsify-next emits these as `interface MainToWorker extends
// MainToWorkerKind` which is invalid TypeScript — TS interfaces
// can't extend a type-alias union. Manual TS type aliases via
// intersection give consumers the discriminated-union view they need.
//
// Tsify derives stay off both outer envelope structs; they're
// JSON-serialized through `handleMessage(string) -> string`, so the
// only consumer of their TS shape is the worker-message marshalling
// on the main thread.
#[wasm_bindgen(typescript_custom_section)]
const TS_ENVELOPES: &'static str = r#"
export type MainToWorker = MainToWorkerKind & {
  seq: number;
  protocol: ProtocolVersion;
};

export type WorkerToMain = WorkerToMainKind & {
  seq: number | null;
  protocol: ProtocolVersion;
};
"#;

/// Bumped on any incompatible change to the channel envelopes.
/// Main thread compares this against its bundled value at worker
/// handshake and refuses to proceed on mismatch — better to fail
/// loud than to silently desync.
///
/// - v27: NodeSpec carries item_transform (RemoveNode → undo keeps the
///   frame in place instead of snapping to the page origin).
/// - v28 (W0.5/W0.6): the final wire-expansion bump. Covers
///   - W0.6 caret queries: `RequestCaretNav` / `RequestLineBounds`
///     requests + `CaretNavResult` / `LineBoundsResult` replies.
///   - W0.5 operations reachable via `Mutation` translation:
///     `LinkFrames` / `UnlinkFrames` (frame threading), `ApplyStyle`
///     (named para/char style over a story range), `InsertField`
///     (page-number marker), guide CRUD (`InsertGuide` / `MoveGuide` /
///     `DeleteGuide`), `SetConditionVisible` / `ActivateConditionSet`,
///     `ApplyMasterToPage`, `DuplicatePage`, section ops
///     (`InsertSection` / `EditSection` / `DeleteSection`), and the
///     `NodeSpec::Oval` insert kind. (The new `paged_mutate::Operation`
///     variants ride the existing `Mutate(Mutation)` envelope, so the
///     Mutation enum gains the variants below.)
/// - v29 (W3.A1 — table NodeId surface): tables become addressable +
///   mutable through the wire. Covers:
///   - `HitResult.table_context` — `HitTest` into a table cell now
///     returns the `(tableId, row, col)` it landed in (new
///     `TableHitContext` payload).
///   - cell-scoped `PropertyPath`s on a `NodeId::TableCell`
///     (`cellFillColor` / `cellFillTint` / `cellInset{Top,Left,Bottom,
///     Right}` / `cellVerticalJustification`; plus the now-live
///     `appliedCellStyle`) and `appliedTableStyle` on a `NodeId::Table`
///     — reachable via `SetElementProperty` against the new
///     `ElementId::Table` / `ElementId::TableCell` addresses.
///   - table-structure `Mutation`s: `SetRowHeight` / `SetColumnWidth` /
///     `InsertTableRow` / `DeleteTableRow` / `InsertTableColumn` /
///     `DeleteTableColumn` (translate to the matching new
///     `paged_mutate::Operation` variants, with delete capturing the
///     removed line for undo).
/// - v31 (Aftercare-A — editor read-surface fill-in): three read-only
///   additions found during the gap campaign.
///   - `RequestWordBounds` / `WordBoundsResult`: the word containing a
///     story byte offset, per Unicode word segmentation (UAX-29). The
///     editor flips double-click word-selection on this. Mirrors the
///     `RequestLineBounds` / `LineBoundsResult` wiring exactly.
///   - table dimension reads: `element_properties` on a
///     `NodeId::Table` now also returns `tableRowCount` /
///     `tableColumnCount` (the integer-as-`Length` convention used for
///     drop-cap counts). Read-only like `NextTextFrame` — no apply arm
///     (writes reject via `apply_table_property`'s non-`AppliedTableStyle`
///     guard).
///   - per-cell geometry: `RequestElementGeometry` on a
///     `ElementId::TableCell` now resolves against the BuiltPage's
///     `cell_rects` (page-local pt, `item_transform: None`) instead of
///     returning nothing.
/// - v32 (B-04 — groups): `CreateGroup { member_ids }` /
///   `DissolveGroup { group_id }` mutations. Create is z-order-neutral
///   for members contiguous in paint order (scattered members collect
///   at the earliest member's slot, InDesign-style); the reply carries
///   the minted group id as `createdId`. Undo restores the exact
///   pre-group z-order via inverse-side `restore_slots` (internal —
///   not a wire field).
/// - v34 (batch-created sentinel): inside a `Batch`, a child
///   `SetPluginMetadata` / `SetElementProperty` whose element id is
///   the literal `$created` addresses the element minted by the most
///   recent CREATING child of the same (flat) batch — create + attach
///   metadata/properties as ONE atomic, single-undo-step mutation.
///   Nested batches don't propagate the sentinel outward.
/// - v33 (plugin-metadata carrier, decision 9 facility):
///   `SetPluginMetadata { element_id, key, value }` — set / replace /
///   delete (value: null) one `Properties/Label` `KeyValuePair` in the
///   reserved `x-paged:` namespace (engine-gated: key prefix, 64 KiB
///   cap, JSON envelope `{v, data, engine?}`). `element_properties` on
///   a leaf page item now also returns its `x-paged:*` entries as
///   `PropertyPath::PluginMetadata` / `Value::PluginMetadata` pairs.
/// - v35 (W1.23 — paragraph-bounds read surface): new message kinds
///   `RequestParagraphBounds { story_id, offset }` →
///   `ParagraphBoundsResult { bounds }` — the `[start, end)` byte span
///   of the paragraph containing the offset (the caret's triple-click
///   wire). Mirrors `RequestWordBounds` / `WordBoundsResult` exactly.
///   New message kinds, so this bumps. Rides v35 (added before first
///   consumer sync): the additive `FontSummary.styles` field (W1.23 —
///   styles-per-family for the glyphs panel) — a `#[serde(default)]`
///   field that wouldn't bump on its own, but it ships in the same
///   mergeable unit as the new kinds.
///   - W1.22 (engine gap 22 — list definitions) also rides v35: the
///     `Mutation::{Create,Edit,Delete}NumberingList` variants (→ the
///     matching `paged_mutate::Operation` variants). New Mutation /
///     Operation variants would normally bump per rule 2, but v35 is
///     still unpublished (highest tag v0.34.0, no v0.35 tag), so per
///     governance rule 4 they ride the open number with this comment.
///     The additive read surface that ships with them does NOT bump on
///     its own (rule 1): the `NumberingLists` CollectionName +
///     `NumberingListSummary`, `ParagraphAppliedNumberingList` +
///     `ParagraphStyleNextStyle` PropertyPaths, and the
///     `ParagraphStyleSummary.next_style` `#[serde(default)]` field.
///   - W1.11b / W1.12a / W1.12b (tables v2) also ride v35 — same
///     unpublished-protocol posture (rule 4). New, additive:
///     - W1.11b: twelve per-cell edge-stroke `PropertyPath`s
///       (`cell{Top,Bottom,Left,Right}EdgeStroke{Color,Weight,Tint}`)
///       on a `NodeId::TableCell`, reachable via `SetElementProperty`;
///       `element_properties` reports them read-side.
///     - W1.12a: `Mutation::{Insert,Remove}{Header,Footer}Row` (→ the
///       matching `Operation` variants) growing / shrinking the
///       `HeaderRowCount` / `FooterRowCount` bands.
///     - W1.12b: `Mutation::SetCellSpan` (→ `Operation::SetCellSpan`)
///       setting a cell's `RowSpan` / `ColumnSpan` (merge / split),
///       inverse restoring the prior spans.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(35);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
pub struct ProtocolVersion(pub u32);

/// Concept 3 — PDF export options as the dialog sends them. Every
/// field is optional/defaulted so the wire stays forward-compatible;
/// the worker maps it onto `paged_export_pdf::ExportOptions`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ExportPdfWireOptions {
    /// "pdf17" (default) | "pdfx4".
    #[serde(default)]
    pub standard: Option<String>,
    /// Output-intent profile NAME, resolved against the worker's
    /// registered profile registry. `None` ⇒ the active working
    /// space profile.
    #[serde(default)]
    pub output_intent_profile: Option<String>,
    /// Human-readable output condition for the OutputIntent dict.
    #[serde(default)]
    pub output_condition: Option<String>,
    /// "preserveNumbers" (default) | "convertToDestination".
    #[serde(default)]
    pub color_policy: Option<String>,
    /// 0-based inclusive page range; both `None` = all pages.
    #[serde(default)]
    pub page_from: Option<u32>,
    #[serde(default)]
    pub page_to: Option<u32>,
    #[serde(default)]
    pub crop_marks: bool,
    #[serde(default)]
    pub registration_marks: bool,
    #[serde(default)]
    pub color_bars: bool,
    #[serde(default)]
    pub page_info: bool,
    #[serde(default)]
    pub marks_offset_pt: Option<f32>,
    /// Bleed override in pt (top, inside/left, bottom,
    /// outside/right); `None` = the document's declared bleed.
    #[serde(default)]
    pub bleed_override_pt: Option<[f32; 4]>,
    /// Resample images above this effective ppi; `None` = never.
    #[serde(default)]
    pub downsample_ppi: Option<f32>,
    /// Raster resolution for effect soft-mask stamps (default 150).
    #[serde(default)]
    pub effect_dpi: Option<f32>,
    /// "outline" (default) | "fail".
    #[serde(default)]
    pub restricted_font_policy: Option<String>,
    /// Document title for Info/XMP.
    #[serde(default)]
    pub title: Option<String>,
}

/// panels.md gap 20 — one structured PDF-export preflight finding for
/// the export dialog's findings list. The wire mirror of
/// `paged_export_pdf::PreflightFinding`. `severity` is `"warning"` /
/// `"error"`; `code` is a stable machine tag (`"font_not_embeddable"`
/// / `"image_missing_bytes"`); `page_index` is the 0-based body-page
/// the finding was raised on (`None` for document-level findings).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct PreflightFinding {
    pub code: String,
    pub severity: String,
    pub message: String,
    #[serde(default)]
    pub page_index: Option<u32>,
}

/// One message from main → worker. (Tsify derive intentionally
/// omitted; see `TS_ENVELOPES` above for the TS-side declaration.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainToWorker {
    pub seq: u64,
    pub protocol: ProtocolVersion,
    #[serde(flatten)]
    pub kind: MainToWorkerKind,
}

/// The discriminated payload of a `MainToWorker` message. Tagged so
/// TS can do `switch (msg.kind) { case "loadDocument": ... }` against
/// camelCase field names. `rename_all_fields` cascades to struct
/// variants so e.g. `cmyk_icc_profile` becomes `cmykIccProfile` on
/// the wire — the TS protocol mirror locks the camelCase contract.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    content = "payload"
)]
pub enum MainToWorkerKind {
    /// Initial handshake. Worker replies with `WorkerToMainKind::Ready`
    /// once it has loaded its WASM and is ready for the first
    /// `LoadDocument`. Sent once per worker lifetime.
    Hello,
    /// Replace the active document with `bytes`. Returns
    /// `DocumentLoaded` (success) or `LoadFailed`.
    LoadDocument {
        #[tsify(type = "number[]")]
        bytes: ByteBuf,
        #[serde(default)]
        #[tsify(type = "number[] | null")]
        font: Option<ByteBuf>,
        #[serde(default)]
        #[tsify(type = "number[] | null")]
        cmyk_icc_profile: Option<ByteBuf>,
    },
    /// Register a named font with the worker's family resolver. Sent
    /// any time before `LoadDocument` (and persists across loads so a
    /// fidelity test can preload Poppins/Roboto/etc once per worker).
    /// Reply: `FontRegistered`.
    RegisterFont {
        family: String,
        #[serde(default)]
        style: Option<String>,
        #[tsify(type = "number[]")]
        bytes: ByteBuf,
    },
    /// Drop every font previously registered via `RegisterFont`. Reply:
    /// `FontRegistryCleared`. Useful between two consecutive packs in a
    /// long-running worker.
    ClearFontRegistry,
    /// Concept 2 — register a named ICC profile with the worker's
    /// colour-profile registry (the `RegisterFont` pattern: sent any
    /// time, persists across loads). Profiles are assets shipped by
    /// the editor and loaded over the wire — never baked into the
    /// wasm binary. `Mutation::SetColorSettings` resolves working-
    /// space names against this registry; a document whose designmap
    /// names a registered profile picks it up at load automatically.
    /// Reply: `ColorProfileRegistered`.
    RegisterColorProfile {
        name: String,
        #[tsify(type = "number[]")]
        bytes: ByteBuf,
    },
    /// Apply a content mutation. Phase 1 returns `MutationFailed`
    /// (NotImplemented). The message exists so the JS side can plumb
    /// it end-to-end now.
    Mutate(Mutation),
    /// Request the per-page display list. Worker replies with
    /// `DisplayListReady` carrying a small descriptor (page id +
    /// command count + generation counters). Phase 1 does not stream
    /// the actual command buffer over `postMessage` — it stays in
    /// shared worker memory; the JS side calls into wasm directly
    /// for the bytes when it needs them.
    RequestPage { page_id: PageId, lod: LodTier },
    /// Hit-test a document-space point. Reply: `HitResult`.
    HitTest {
        page_id: PageId,
        doc_point: (f32, f32),
        filter: HitFilter,
    },
    /// Render a snapshot (low-resolution thumbnail) of a page.
    /// Reply: `SnapshotReady` with PNG bytes. Used by the navigator
    /// and the canvas at overview zoom. `dpi` is optional and wins
    /// over `target_width_px` when both are provided (the fidelity
    /// suite uses DPI directly so the resulting PNG matches
    /// `pdftoppm -r <dpi>` byte-for-byte at the dimension boundary).
    RequestSnapshot {
        page_id: PageId,
        target_width_px: u32,
        #[serde(default)]
        dpi: Option<f32>,
    },
    /// Replace the worker's current selection. Phase 3 Item 1 — the
    /// worker mirrors the main thread's `ContentSelection` so the
    /// caret / selection geometry queries have a stable state to
    /// read.
    SetSelection {
        selection: Option<crate::selection::ContentSelection>,
    },
    /// Compute selection geometry (rect-per-line). Reply:
    /// `SelectionGeometry`.
    RequestSelectionGeometry {
        selection: crate::selection::ContentSelection,
    },
    /// Compute caret geometry for a selection. Reply:
    /// `CaretGeometry`.
    RequestCaretGeometry {
        selection: crate::selection::ContentSelection,
    },
    /// panels.md (W0.6 caret queries) — vertical caret navigation:
    /// move the caret one visible line up/down from `offset`,
    /// targeting the column nearest the source caret's x. Reply:
    /// `CaretNavResult` whose `offset` is `None` when there's no line
    /// in that direction (caret already on the first/last line).
    RequestCaretNav {
        story_id: String,
        offset: u32,
        direction: crate::geometry::CaretDirection,
        /// W1.13 — cell qualifier (rides v35, additive). `None` ⇒
        /// `offset` is a story-local body offset; `Some` ⇒ cell-local.
        /// Navigation stays WITHIN the addressed stream (a cell's
        /// caret-up/down does not escape the cell). See `TextCellAddr`.
        #[serde(default)]
        cell: Option<crate::selection::TextCellAddr>,
    },
    /// panels.md (W0.6 caret queries) — the `[line_start, line_end]`
    /// story offsets of the visible line containing `offset` (Home /
    /// End targets). Reply: `LineBoundsResult`.
    RequestLineBounds {
        story_id: String,
        offset: u32,
        /// W1.13 — cell qualifier (rides v35, additive). See
        /// `RequestCaretNav::cell`.
        #[serde(default)]
        cell: Option<crate::selection::TextCellAddr>,
    },
    /// Aftercare-A (protocol v31) — the `[start, end]` story byte
    /// offsets of the word containing the character at `offset`, per
    /// Unicode word segmentation (UAX-29). The editor flips
    /// double-click word-selection on this. Reply: `WordBoundsResult`.
    /// Offsets are story-local *bytes* — the same address space
    /// `HitResult.offset_within_story` / `RequestLineBounds` /
    /// `RequestCaretNav` use. An offset that lands on a run of
    /// whitespace selects that whitespace span (documented in
    /// `word_bounds`); an offset past the story end clamps to the
    /// final word.
    RequestWordBounds {
        story_id: String,
        offset: u32,
        /// W1.13 — cell qualifier (rides v35, additive). When `Some`,
        /// offsets are cell-local and segmentation runs over the cell's
        /// text. See `RequestCaretNav::cell`.
        #[serde(default)]
        cell: Option<crate::selection::TextCellAddr>,
    },
    /// W1.23 (protocol v35) — the `[start, end)` story byte offsets of
    /// the paragraph containing the character at `offset`. The editor
    /// flips triple-click paragraph-selection on this. Reply:
    /// `ParagraphBoundsResult`. Offsets are story-local *bytes* — the
    /// same address space `HitResult.offset_within_story` /
    /// `RequestWordBounds` / `RequestLineBounds` use; a paragraph is a
    /// maximal run between the synthetic inter-paragraph `\n`
    /// separators (the boundary `\n` is excluded from the span). An
    /// offset past the story end clamps to the final paragraph. Mirrors
    /// `RequestWordBounds` exactly.
    RequestParagraphBounds {
        story_id: String,
        offset: u32,
        /// W1.13 — cell qualifier (rides v35, additive). See
        /// `RequestCaretNav::cell`.
        #[serde(default)]
        cell: Option<crate::selection::TextCellAddr>,
    },
    /// Undo the most recent applied mutation. Reply: `UndoApplied`
    /// or `MutationFailed` (when the log is empty).
    Undo,
    /// Re-apply the most recently undone mutation. Reply:
    /// `RedoApplied` or `MutationFailed`.
    Redo,
    /// Phase A — replace the worker's element selection. Selection is
    /// application state (not in the Operation log); the worker
    /// mirrors it so geometry queries have a stable read.
    /// Reply: `ElementSelectionApplied`.
    SetElementSelection {
        ids: Vec<crate::element_selection::ElementId>,
        mode: crate::element_selection::SelectionMode,
    },
    /// Phase A — return every selectable element whose oriented bounds
    /// intersect `rect` (page-local `[top, left, bottom, right]`).
    /// Reply: `MarqueeHits`.
    RequestMarqueeHits { page_id: PageId, rect: [f32; 4] },
    /// Phase A — fetch oriented geometry (raw bounds + composed
    /// transform) for one or more elements so the overlay can draw
    /// selection chrome without re-deriving the math in TS.
    /// Reply: `ElementGeometry`.
    RequestElementGeometry {
        ids: Vec<crate::element_selection::ElementId>,
    },
    /// Phase H — return every leaf descendant of the named group.
    /// Used by the canvas's double-click-to-enter-group gesture.
    /// Reply: `GroupLeaves`.
    RequestGroupLeaves { group_id: String },
    /// Step 5 — request the path-anchor table for a single Polygon /
    /// Rectangle / Oval / TextFrame so the path-edit overlay can draw
    /// one dot per anchor + Bezier-handle pair. Reply: `PathAnchors`.
    /// Elements that don't carry an `anchors` array (rectangles
    /// declared via `GeometricBounds` only) come back with `anchors`
    /// empty.
    RequestPathAnchors {
        id: crate::element_selection::ElementId,
    },
    /// B-06 (protocol v30) — closest on-curve point on the element's
    /// path. `point` is in the element's LOCAL coordinate space (the
    /// same space `PathAnchors` reports — callers inverse-apply
    /// `item_transform` first, exactly like the anchor tools).
    /// Reply: `NearestPathPoint`.
    RequestNearestPathPoint {
        id: crate::element_selection::ElementId,
        point: [f32; 2],
    },
    /// Track M — list every `<Layer>` from the loaded document's
    /// designmap. Reply: `Layers`. The Layers panel polls this on
    /// mount and on every `MutationApplied` / `UndoApplied` /
    /// `RedoApplied` push (same pattern as the Inspector) — a
    /// dedicated `LayersChanged` notification is overkill given the
    /// small payload size and existing subscription wiring.
    RequestLayers,
    /// SDK Phase 5 (D1) — typed read of any document collection per
    /// `panel-catalog-and-sdk-extension.md` §5.1. Single envelope
    /// handles all 21 named collections; the dispatcher in
    /// `CanvasModel::collection(name)` routes to the underlying
    /// per-collection accessor. Reply: `CollectionReply` whose
    /// `items` is a `serde_json::Value` — the consumer deserializes
    /// to the typed `*Summary[]` it expects (matching the
    /// `documentCollection:<name>` ReadSpec it declared). Unknown /
    /// unimplemented collections come back with an empty array.
    RequestCollection { name: CollectionName },
    /// SDK Phase 5 (D1) — singleton document meta read per
    /// `panel-catalog-and-sdk-extension.md` §5.6. Backs the
    /// `documentMeta:<key>` ReadSpec form. The reply carries every
    /// field at once; the consumer picks the one it bound against.
    /// Volume is trivial so paging per-key isn't worth the round-
    /// trip cost. Reply: `DocumentMetaReply`.
    RequestDocumentMeta,
    /// SDK Phase 5 (v1 sweep) — resolved CMYK + RGB readout for a
    /// named swatch. Powers the Color panel's CMYK/RGB display.
    /// Editor sliders (which would mutate the swatch's channel
    /// values) are a v2 follow-up needing
    /// `Operation::SetSwatchValue` + a Color NodeId variant.
    RequestColorPreview { swatch_id: String },
    /// Concept 2 — resolve an ARBITRARY colour value (not a swatch
    /// ref) through the document's active colour management:
    /// display RGB + out-of-gamut verdict. Powers the mixer's live
    /// preview + warning triangle while the user drags sliders,
    /// BEFORE any swatch exists. `space` is the SwatchSpec
    /// vocabulary ("CMYK" | "RGB" | "LAB" | "Gray"); `value` its
    /// channels; `tint` 0..=100; spot alternates resolve like a
    /// swatch would. Reply: `ColorComputeReply`.
    RequestColorCompute {
        space: String,
        value: Vec<f32>,
        #[serde(default)]
        tint: Option<f32>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        alternate_space: Option<String>,
        #[serde(default)]
        alternate_value: Option<Vec<f32>>,
    },
    /// Concept 2 — full stop detail for ONE gradient (the ramp
    /// editor + faithful gradient chips). The lightweight
    /// `GradientSummary` collection stays stop-free; detail is
    /// fetched per selected gradient. Reply: `GradientDetailReply`.
    RequestGradientDetail { gradient_id: String },
    /// Concept 2 — serialise swatches back to `.ase` (the Swatches
    /// panel's "Save .ase…"; lossless raw channel values, core owns
    /// the format both ways). `group_id: Some` exports one
    /// ColorGroup; `None` exports the whole palette grouped by the
    /// document's ColorGroups. Reply: `SwatchLibraryExported`.
    ExportSwatchLibrary {
        #[serde(default)]
        group_id: Option<String>,
    },
    /// Scripting Stage 2 — execute a JS source string against the
    /// loaded document. The script's mutations route through
    /// `Operation::SetProperty` (same channel as gestures + REPL)
    /// so undo/redo work identically. Reply: `ScriptResult`.
    ExecuteScript { source: String },
    /// Concept 3 — open a PDF export session. The worker re-runs the
    /// scene build one-shot (glyph side-channel on, splice caches
    /// off) and parks the writer state under a session id. Reply:
    /// `ExportPdfBegun` (or `ExportPdfFailed`).
    ExportPdfBegin { options: ExportPdfWireOptions },
    /// Concept 3 — export ONE page of the session. The main thread
    /// drives this loop, which is what makes progress + cancellation
    /// real on a synchronous worker. Reply: `ExportPdfProgress`.
    ExportPdfPage { session: u32 },
    /// Concept 3 — serialise the finished document and drop the
    /// session. Reply: `PdfExported`.
    ExportPdfFinish { session: u32 },
    /// Concept 3 — abandon an in-flight session (dialog Cancel /
    /// AbortSignal). Reply: `ExportPdfCancelled`.
    ExportPdfCancel { session: u32 },
    /// W3.B2 (rides v29 — added before first editor sync) — serialise
    /// the loaded (possibly-mutated) document back to an IDML package
    /// for save-back. Unlike the PDF export this is a single one-shot
    /// (the carry-through writer is cheap: it patches only the
    /// model-owned Spreads/Stories and copies every other entry
    /// verbatim), so there's no session/progress loop. Reply:
    /// `IdmlExported` (or `ExportIdmlFailed`).
    ExportIdml {},
    /// Inspector P1 — return a property snapshot for one element so
    /// the Inspector panel can render typed editors. Reply:
    /// `ElementProperties`. Each entry carries the property path +
    /// its authored value tagged by kind so the UI picks the right
    /// editor without re-deriving the schema. `None` when the id
    /// doesn't resolve.
    RequestElementProperties {
        id: crate::element_selection::ElementId,
    },
    /// Inspector P1 — return the scene hierarchy
    /// (spread → page → group? → frame) so the Tree panel can render
    /// a navigable outline. The reply carries name + id + kind per
    /// node + nested children. Lightweight enough to send eagerly.
    RequestSceneTree,
    /// Phase B — start a gesture against the listed elements. Reply
    /// `GestureBegun { handle }` carrying the opaque handle the main
    /// thread sends back on every subsequent update / commit / cancel.
    /// Errors with `MutationFailed` when a gesture is already active.
    ///
    /// Phase D — `anchor` is required for Rotate / Scale (the pointer
    /// position at gesture start, in page-local coords + the page id).
    /// Optional for Translate / Resize. Phase G — `camera_scale`
    /// (px/pt) lets the snap pass keep its tolerance constant in
    /// screen px. Omitting it falls back to a 4 doc-space-pt
    /// tolerance.
    BeginGesture {
        nodes: Vec<crate::element_selection::ElementId>,
        gesture: crate::gesture::GestureType,
        #[serde(default)]
        anchor: Option<crate::gesture::GestureAnchor>,
        #[serde(default)]
        camera_scale: Option<f32>,
    },
    /// Phase B — push a pointer-delta + modifier state into the
    /// active gesture. Worker rewrites the preview and replies
    /// `GestureUpdated { handle, page_ids }`.
    UpdateGesture {
        handle: crate::gesture::GestureHandle,
        /// Cumulative pointer delta since `BeginGesture`, in doc pt.
        delta: (f32, f32),
        modifiers: crate::gesture::GestureModifiers,
    },
    /// Phase B — commit the active gesture. Reply
    /// `GestureCommitted { handle, applied_seq, page_ids }`. The
    /// committed mutation lands on the unified undo log.
    CommitGesture {
        handle: crate::gesture::GestureHandle,
    },
    /// Phase B — discard the active gesture. Reply
    /// `GestureCancelled { handle, page_ids }`; scene reverts to the
    /// pre-`BeginGesture` snapshot.
    CancelGesture {
        handle: crate::gesture::GestureHandle,
    },
}

/// Which runtime budget a script exhausted (B-09 / W-08). The typed
/// half of a `ScriptResult`: lets the host distinguish a budget abort
/// from an ordinary script exception (e.g. show a "script hit its
/// time/iteration limit" banner). Mirrors `paged_script::
/// ScriptBudgetKind` — kept in this crate so the wire types carry no
/// dependency on `paged-script` (which depends on us). Additive on the
/// wire: rides protocol v35 as an optional field on `ScriptResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum ScriptBudgetKind {
    /// Loop-iteration limit tripped (runaway / pathological pure-JS
    /// loop). Enforced natively by Boa's bytecode loop opcode.
    Iterations,
    /// Recursion-depth limit tripped (unbounded / too-deep recursion).
    Recursion,
    /// VM value-stack overflow guard tripped.
    StackSize,
    /// Wall-clock deadline elapsed during a host call. The single-
    /// threaded wasm worker cannot preempt a host-call-free pure-JS
    /// loop, so this fires at the next `paged.*`/`console.*` boundary.
    WallClock,
}

/// Coarse LOD tiers requested by the navigator + canvas (per spec §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum LodTier {
    /// Atlas thumbnail. Used by the navigator and overview zoom.
    Snapshot,
    /// Per-page bitmap. Used at page-fit-ish zoom.
    MidRes,
    /// Live Vello tiles at the current zoom.
    Live,
}

/// What to consider when hit-testing. The inspector + editor route
/// pointer events through this. Phase 1 only implements `Frame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum HitFilter {
    Frame,
    Text,
    Any,
}

/// Hit-test result.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct HitResult {
    pub frame_id: Option<String>,
    pub story_id: Option<String>,
    pub offset_within_story: Option<u32>,
    /// Selected frame's bounding box in page-local coordinates.
    /// AABB of the transformed corners. Returned for back-compat with
    /// callers that only want a quick rectangle.
    pub frame_bounds: Option<FrameBounds>,
    /// Phase A — typed element identifier, the new canonical handle.
    /// `frame_id` is kept as the raw-id alias for back-compat with
    /// callers that haven't migrated.
    #[serde(default)]
    pub element: Option<crate::element_selection::ElementId>,
    /// Phase A — the element's raw `GeometricBounds` (content-box
    /// space). Combine with `item_transform` to draw an oriented
    /// selection chrome on the main thread without re-deriving the
    /// math. `[top, left, bottom, right]`.
    #[serde(default)]
    pub bounds: Option<[f32; 4]>,
    /// Phase A — composed affine `[a, b, c, d, tx, ty]` on the hit
    /// element. `None` for items with no `ItemTransform`.
    #[serde(default)]
    pub item_transform: Option<[f32; 6]>,
    /// Phase A — containing group ancestry, outer-most first. Empty
    /// when the hit element is not nested in any group.
    #[serde(default)]
    pub group_chain: Vec<String>,
    /// W3.A1 — table cell context when the point landed inside a cell
    /// of the hit frame's story. `None` for non-table hits. Carries
    /// `(tableId, row, col)` so the canvas can select / mutate the cell
    /// without a second query.
    #[serde(default)]
    pub table_context: Option<TableHitContext>,
}

/// W3.A1 — wire shape of [`crate::hit::TableHitContext`]: the table
/// cell a `HitTest` landed in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct TableHitContext {
    pub table_id: String,
    pub row: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct FrameBounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// One message from worker → main.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerToMain {
    /// Echo of the corresponding `MainToWorker.seq` when this message
    /// is a reply; `None` for unsolicited messages (e.g. `PagesDirty`).
    #[serde(default)]
    pub seq: Option<u64>,
    pub protocol: ProtocolVersion,
    #[serde(flatten)]
    pub kind: WorkerToMainKind,
}

/// Discriminated payload of a `WorkerToMain` message.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    content = "payload"
)]
pub enum WorkerToMainKind {
    /// Worker bootstrap complete; ready to accept `LoadDocument`.
    Ready { protocol: ProtocolVersion },
    /// `LoadDocument` succeeded. Carries the handle the main thread
    /// shows in the page navigator + structural debug HUD.
    DocumentLoaded(DocumentHandle),
    /// `LoadDocument` failed.
    LoadFailed { error: LoadError },
    /// `Mutate` failed (Phase 1: always).
    MutationFailed { error: WorkerError },
    /// `RequestPage` reply: page is renderable at the requested LOD.
    /// Carries the latest generation counters so the main thread can
    /// detect staleness.
    DisplayListReady {
        page_id: PageId,
        lod: LodTier,
        commands: usize,
        layout_generation: u64,
        numbering_generation: u64,
    },
    /// `HitTest` reply.
    HitResult(HitResult),
    /// Unsolicited: the listed pages have new display lists. If the
    /// main thread has any of them in its viewport, redraw.
    PagesDirty { page_ids: Vec<PageId> },
    /// Unsolicited: a story's content changed (used by inspector to
    /// update content views).
    StoryDirty { story_id: String },
    /// Unsolicited: convergence cap, missing font, overset text, etc.
    Warning { kind: String, details: String },
    /// Unsolicited: the worker's most recent operation produced
    /// metrics worth surfacing (initial document stats, post-build
    /// counts, etc.).
    Stats(DocumentStats),
    /// `RequestSnapshot` reply: PNG-encoded snapshot ready for the
    /// main thread to stash in an `<img>` / `ImageBitmap` / texture
    /// atlas. Carries the source page's generation counters so the
    /// main thread can detect staleness before drawing.
    SnapshotReady(crate::snapshot::SnapshotPng),
    /// `RequestSnapshot` failed (e.g. unknown page id).
    SnapshotFailed {
        error: crate::snapshot::SnapshotError,
    },
    /// Phase 3 Item 6 — a mutation was successfully applied. The
    /// worker assigns `applied_seq` (monotone); the main thread
    /// matches against its own `client_seq` to ack pending sends.
    MutationApplied {
        client_seq: u64,
        applied_seq: u64,
        /// Pages whose display lists need re-rendering. The canvas
        /// invalidates their entries in the LOD cache.
        page_ids: Vec<PageId>,
        /// Phase 4 Step 2 instrumentation — layout-cache stats for
        /// the rebuild that just finished. `hits + misses` equals the
        /// number of paragraphs that ran through the layout pass.
        cache_stats: LayoutCacheStats,
        /// Editor-ops — the element created by a structural insert
        /// (`InsertFrame` / `InsertLine` / `InsertPath`). The editor
        /// uses it to select the fresh element. `None` for every
        /// other mutation kind — including `InsertPage`: pages are
        /// not elements (`ElementId` has no Page variant); the new
        /// page is discoverable from `page_ids` + `page_sizes_pt`.
        #[serde(default)]
        created_id: Option<crate::element_selection::ElementId>,
        /// Editor-ops — `true` when the mutation changed the page
        /// LIST itself (insert/delete/resize page); the editor must
        /// refresh its page grid from `page_sizes_pt` + `page_ids`
        /// instead of only repainting.
        #[serde(default)]
        page_structure_changed: bool,
        /// Editor-ops — the post-mutation per-page sizes, populated
        /// only when `page_structure_changed` (ordered like
        /// `page_ids`).
        #[serde(default)]
        page_sizes_pt: Option<Vec<(f32, f32)>>,
    },
    /// Phase 3 Item 4 — rect-per-line geometry for a selection range.
    SelectionGeometry { rects: Vec<crate::SelectionRect> },
    /// Phase 3 Item 3 — caret position for a selection.
    CaretGeometry {
        caret: Option<crate::geometry::CaretGeometry>,
    },
    /// panels.md (W0.6 caret queries) — `RequestCaretNav` reply.
    /// `offset` is the destination story offset, or `None` when there
    /// was no line in the requested direction.
    CaretNavResult {
        #[serde(default)]
        offset: Option<u32>,
    },
    /// panels.md (W0.6 caret queries) — `RequestLineBounds` reply.
    /// `None` when the offset doesn't fall on a visible line (story
    /// has no captured layout).
    LineBoundsResult {
        #[serde(default)]
        bounds: Option<crate::geometry::LineBounds>,
    },
    /// Aftercare-A (protocol v31) — `RequestWordBounds` reply. `None`
    /// when the story doesn't resolve or carries no text; otherwise
    /// the `[start, end)` byte span of the word (or whitespace run)
    /// containing the requested offset.
    WordBoundsResult {
        #[serde(default)]
        bounds: Option<crate::geometry::WordBounds>,
    },
    /// W1.23 (protocol v35) — `RequestParagraphBounds` reply. `None`
    /// when the story doesn't resolve or carries no text; otherwise the
    /// `[start, end)` byte span of the paragraph containing the
    /// requested offset.
    ParagraphBoundsResult {
        #[serde(default)]
        bounds: Option<crate::geometry::ParagraphBounds>,
    },
    /// Phase 3 Item 7 — undo applied. `undone_seq` is the
    /// `applied_seq` of the mutation that was reversed.
    UndoApplied {
        undone_seq: u64,
        applied_seq: u64,
        page_ids: Vec<PageId>,
        cache_stats: LayoutCacheStats,
        /// Editor-ops — same page-grid refresh contract as
        /// `MutationApplied`: undoing a page mutation changes the
        /// page list and the editor must not need a reload to see
        /// it. The worker diffs the built page table across the
        /// undo to populate these.
        #[serde(default)]
        page_structure_changed: bool,
        #[serde(default)]
        page_sizes_pt: Option<Vec<(f32, f32)>>,
    },
    /// Phase 3 Item 7 — redo applied.
    RedoApplied {
        redone_seq: u64,
        applied_seq: u64,
        page_ids: Vec<PageId>,
        cache_stats: LayoutCacheStats,
        #[serde(default)]
        page_structure_changed: bool,
        #[serde(default)]
        page_sizes_pt: Option<Vec<(f32, f32)>>,
    },
    /// `RegisterFont` reply: the font is now part of the worker's
    /// asset resolver.
    FontRegistered { family: String },
    /// `ClearFontRegistry` reply.
    FontRegistryCleared,
    /// Concept 2 — `RegisterColorProfile` reply.
    ColorProfileRegistered { name: String },
    /// Phase A — `SetElementSelection` reply. Echoes the post-update
    /// selection so the main thread can reconcile if its optimistic
    /// update drifted.
    ElementSelectionApplied {
        ids: Vec<crate::element_selection::ElementId>,
    },
    /// Phase A — `RequestMarqueeHits` reply. Element ids in paint
    /// order, top-first.
    MarqueeHits {
        ids: Vec<crate::element_selection::ElementId>,
    },
    /// Phase A — `RequestElementGeometry` reply. One entry per id;
    /// elements that don't resolve (id missing or not on a body page)
    /// are dropped silently.
    ElementGeometry { items: Vec<ElementGeometryItem> },
    /// Phase H — `RequestGroupLeaves` reply. Empty when the group id
    /// doesn't resolve.
    GroupLeaves {
        ids: Vec<crate::element_selection::ElementId>,
    },
    /// Step 5 — `RequestPathAnchors` reply. `None` when the id doesn't
    /// resolve or sits on a master spread; `Some` even when the
    /// element's anchor list is empty (lets the caller distinguish
    /// "no path data" from "didn't resolve").
    PathAnchors { result: Option<PathAnchorsResult> },
    /// B-06 — `RequestNearestPathPoint` reply. `None` when the id
    /// doesn't resolve or carries no path data.
    NearestPathPoint {
        result: Option<NearestPathPointResult>,
    },
    /// Track M — `RequestLayers` reply. Documents without `<Layer>`
    /// elements (rare; the IDML container always emits at least a
    /// default layer) come back with an empty Vec.
    Layers { items: Vec<LayerSummary> },
    /// SDK Phase 5 (D1) — `RequestCollection` reply. `items` is a
    /// `serde_json::Value` (always an array on the wire) so a single
    /// envelope handles every collection's typed shape. The consumer
    /// deserializes against the typed `*Summary` it expects —
    /// `SwatchSummary[]` for `name: "swatches"`,
    /// `ParagraphStyleSummary[]` for `name: "paragraphStyles"`,
    /// etc. Per `panel-catalog-and-sdk-extension.md` §5.1. Unknown
    /// or not-yet-implemented collections come back with an empty
    /// array.
    CollectionReply {
        name: CollectionName,
        #[tsify(type = "any")]
        items: serde_json::Value,
    },
    /// SDK Phase 5 (D1) — `RequestDocumentMeta` reply. Per
    /// `panel-catalog-and-sdk-extension.md` §5.6.
    DocumentMetaReply { meta: DocumentMeta },
    /// SDK Phase 5 (v1 sweep) — `RequestColorPreview` reply.
    /// `result` is `None` when the swatch id doesn't resolve.
    ColorPreviewReply { result: Option<ColorPreview> },
    /// Concept 2 — `RequestColorCompute` reply.
    ColorComputeReply {
        rgb_hex: String,
        cmyk: Option<[f32; 4]>,
        out_of_gamut: bool,
    },
    /// Concept 2 — `RequestGradientDetail` reply. `None` when the
    /// id doesn't resolve to a gradient.
    GradientDetailReply { result: Option<GradientDetail> },
    /// Concept 2 — `ExportSwatchLibrary` reply.
    SwatchLibraryExported {
        #[tsify(type = "number[]")]
        ase_bytes: ByteBuf,
    },
    /// Concept 3 — `ExportPdfBegin` reply.
    ExportPdfBegun { session: u32, page_count: u32 },
    /// Concept 3 — `ExportPdfPage` reply (one page exported).
    ExportPdfProgress { session: u32, done: u32, total: u32 },
    /// Concept 3 — `ExportPdfFinish` reply. `diagnostics` carries the
    /// human-readable summary lines; panels.md gap 20 — `findings`
    /// carries the SAME findings structured (code + severity + page)
    /// so the dialog can render a grouped, severity-coloured list and
    /// deep-link to the offending page.
    PdfExported {
        #[tsify(type = "number[]")]
        pdf_bytes: ByteBuf,
        diagnostics: Vec<String>,
        #[serde(default)]
        findings: Vec<PreflightFinding>,
    },
    /// Concept 3 — `ExportPdfCancel` reply.
    ExportPdfCancelled { session: u32 },
    /// Concept 3 — any export request that could not be honoured
    /// (unknown session, bad options, build/write failure).
    ExportPdfFailed { error: String },
    /// W3.B2 (rides v29 — added before first editor sync) — `ExportIdml`
    /// reply. `idml_bytes` is the re-serialised package (mirrors how
    /// `PdfExported` carries `pdf_bytes` as a `ByteBuf` rendered as a
    /// `number[]` on the wire). The main thread offers it to the user
    /// as a `.idml` download / save target.
    IdmlExported {
        #[tsify(type = "number[]")]
        idml_bytes: ByteBuf,
    },
    /// W3.B2 (rides v29 — added before first editor sync) — `ExportIdml`
    /// failed (no document loaded, or the carry-through writer errored).
    /// Mirrors `ExportPdfFailed`'s flat-string error shape.
    ExportIdmlFailed { error: String },
    /// Inspector P1 — `RequestElementProperties` reply. `None` when
    /// the id doesn't resolve.
    ElementProperties { result: Option<ElementProperties> },
    /// Inspector P1 — `RequestSceneTree` reply.
    SceneTree { roots: Vec<SceneTreeNode> },
    /// Scripting Stage 2 — `ExecuteScript` reply. `output` is the
    /// concatenated console.* lines; `error` is non-null when the
    /// script threw an unhandled exception. `budget_kind` is set (with
    /// `error` also set) when the abort was a runtime-budget exhaustion
    /// (B-09 / W-08 typed-exhaustion contract). Additive on the wire —
    /// rides protocol v35; omitted from the JSON for ordinary results,
    /// so pre-existing consumers are unaffected.
    ScriptResult {
        output: Vec<String>,
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        budget_kind: Option<ScriptBudgetKind>,
    },
    /// Phase B — `BeginGesture` succeeded.
    GestureBegun {
        handle: crate::gesture::GestureHandle,
    },
    /// Phase B — `UpdateGesture` applied. `page_ids` is the dirty set
    /// so the canvas can scope its LOD-cache invalidation. Phase E —
    /// `snap_lines` is the active set of snap guides the overlay
    /// should render (one entry per axis that snapped this update).
    GestureUpdated {
        handle: crate::gesture::GestureHandle,
        page_ids: Vec<PageId>,
        #[serde(default)]
        snap_lines: Vec<crate::snap::SnapLine>,
    },
    /// Phase B — `CommitGesture` succeeded. Mirrors
    /// `MutationApplied`'s payload: the new applied_seq + dirty pages
    /// + layout-cache stats so the main thread can update its HUD.
    GestureCommitted {
        handle: crate::gesture::GestureHandle,
        applied_seq: u64,
        page_ids: Vec<PageId>,
        cache_stats: LayoutCacheStats,
    },
    /// Phase B — `CancelGesture` complete; scene was restored from the
    /// snapshot. `page_ids` covers the restored pages.
    GestureCancelled {
        handle: crate::gesture::GestureHandle,
        page_ids: Vec<PageId>,
    },
    /// Phase B — gesture-lifecycle error. Sent for any of
    /// `BeginGesture` / `UpdateGesture` / `CommitGesture` /
    /// `CancelGesture` that the worker can't fulfil (stale handle,
    /// rotated frame, already-active gesture).
    GestureFailed { error: GestureFailure },
    /// Sent by the JS-side worker glue (not by Rust) after the
    /// renderer attaches to the host canvas. Carries the GPU
    /// readiness flag + the configured scene-cache budget. The Rust
    /// variant exists so the TS contract is unified — emitting code
    /// lives in `apps/canvas/src/worker/worker.ts`.
    AttachReady {
        gpu_active: bool,
        scene_cache_budget: u32,
    },
    /// Step 5e — fired by the JS-side worker glue after a SAB-drain
    /// tick that ran `update_gesture_raw`. The SAB hot path bypasses
    /// the `GestureUpdated` JSON envelope, so this unsolicited notify
    /// is how the overlay still learns about the active snap guides.
    /// Always carries the latest snap-line set, including the empty
    /// vec when the gesture left a previously-snapped axis (so the
    /// overlay can clear stale guides). Emitting code lives in
    /// `apps/canvas/src/worker/worker.ts`.
    GestureSnapLines {
        snap_lines: Vec<crate::snap::SnapLine>,
    },
    /// Sent by the JS-side worker glue (not by Rust) after `LoadDocument`
    /// succeeds, carrying the Tier 3 resolution result. The Rust variant
    /// exists so the TS contract is unified — emitting code lives in
    /// `apps/canvas/src/worker/worker.ts`.
    ResolutionDone(crate::resolve::ResolutionResult),
}

/// Wire-format errors for the gesture envelope. Mirrors the variants
/// of `crate::gesture::GestureError` so the channel doesn't expose the
/// internal `thiserror` representation.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase", tag = "kind", content = "details")]
pub enum GestureFailure {
    NoDocument,
    UnsupportedGesture {
        reason: String,
    },
    AlreadyActive {
        handle: crate::gesture::GestureHandle,
    },
    HandleMismatch,
    ElementNotFound {
        id: crate::element_selection::ElementId,
    },
    RotatedFrameUnsupported,
    EmptySelection,
    MissingAnchor,
    UnknownAnchorPage {
        page_id: PageId,
    },
    Other {
        message: String,
    },
}

impl From<crate::gesture::GestureError> for GestureFailure {
    fn from(e: crate::gesture::GestureError) -> Self {
        use crate::gesture::GestureError::*;
        match e {
            NoDocument => GestureFailure::NoDocument,
            UnsupportedGesture(g) => GestureFailure::UnsupportedGesture {
                reason: format!("{g:?}"),
            },
            AlreadyActive(h) => GestureFailure::AlreadyActive { handle: h },
            HandleMismatch => GestureFailure::HandleMismatch,
            ElementNotFound(id) => GestureFailure::ElementNotFound { id },
            RotatedFrameUnsupported => GestureFailure::RotatedFrameUnsupported,
            EmptySelection => GestureFailure::EmptySelection,
            Mutate(msg) => GestureFailure::Other { message: msg },
            MissingAnchor => GestureFailure::MissingAnchor,
            UnknownAnchorPage(page_id) => GestureFailure::UnknownAnchorPage { page_id },
        }
    }
}

/// Oriented geometry for one selected element. `bounds` is the raw
/// `GeometricBounds` (content-box space); `item_transform` is the
/// composed affine. The overlay layer multiplies bounds corners by
/// the transform to draw the oriented selection chrome.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ElementGeometryItem {
    pub id: crate::element_selection::ElementId,
    pub page_id: PageId,
    /// `[top, left, bottom, right]`.
    pub bounds: [f32; 4],
    /// `[a, b, c, d, tx, ty]`.
    #[serde(default)]
    pub item_transform: Option<[f32; 6]>,
    /// Phase F — `true` when this element hosts a placed image
    /// (`Rectangle` with `<Image>` / `<EPSImage>` / `<PDF>` /
    /// `<ImportedPage>` nested). The TS overlay uses this to decide
    /// whether a Cmd-drag should kick off `TranslateContent` instead
    /// of `Translate`.
    #[serde(default)]
    pub has_image: bool,
}

/// Step 5 — one anchor's three control points, in the polygon's
/// inner coords (before `item_transform` + page-origin shift). The
/// overlay applies the same affine chain it already uses for selection
/// chrome.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct PathAnchorTriple {
    pub anchor: [f32; 2],
    pub left: [f32; 2],
    pub right: [f32; 2],
}

/// Track M — wire-shape mirror of `paged_parse::Layer`. Surfaces
/// everything the Layers panel needs without leaking parse-side
/// fields the wasm boundary doesn't understand. `z` is the layer's
/// zero-based index in `designmap.layers` (top-first, matching the
/// renderer's paint order via `layer_z_index`).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct LayerSummary {
    pub self_id: String,
    pub name: Option<String>,
    pub visible: bool,
    pub locked: bool,
    pub printable: bool,
    pub z: u32,
}

/// Inspector P1 — typed property snapshot for one element. The
/// Inspector panel maps each entry to the right typed editor:
/// bounds → `BoundsInput`, transform → 6-cell matrix, colour ref →
/// `ColorPicker`, length → `LengthInput`, etc.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ElementProperties {
    pub id: crate::element_selection::ElementId,
    pub kind: String,
    /// Optional human-readable name (frame label, layer name, …) when
    /// the underlying type carries one.
    #[serde(default)]
    pub name: Option<String>,
    pub entries: Vec<PropertyEntry>,
}

/// Inspector P1 — one row of the inspector. `path` is the
/// `PropertyPath` discriminant (camelCase). `value` mirrors the
/// `Value` wire shape so the panel can pass it through to
/// `Mutation::SetElementProperty` without re-encoding.
///
/// SDK Phase 3 — `value` is `Option<Value>` (was `Value`). `None`
/// signals "mixed / indeterminate" — a `NodeId::StoryRange` whose
/// `CharacterRun`s carry conflicting values for this path returns
/// `None` so the binding renderer can show a placeholder (em-dash)
/// rather than picking an arbitrary winner. For frame-level reads
/// the value is always `Some(_)`.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct PropertyEntry {
    pub path: paged_mutate::PropertyPath,
    #[serde(default)]
    pub value: Option<paged_mutate::Value>,
}

/// SDK Phase 5 (D1) — closed enumeration of every document
/// collection a panel may bind against. Per
/// `panel-catalog-and-sdk-extension.md` §5.1. The Rust enum and the
/// TS `CollectionName` union (in `packages/catalog/src/types.ts`)
/// stay in lockstep; tsify emits a string-tag enum at the boundary
/// so consumers can pass names verbatim.
///
/// Not every variant has a backing model accessor yet — the wire
/// surface lands here as the §5 binding ceiling, and the per-
/// collection accessors fill in as panels need them. The
/// `CanvasModel::collection(name)` dispatcher returns an empty
/// `serde_json::Value::Array` for unimplemented entries, surfacing
/// a runtime warning rather than a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum CollectionName {
    Swatches,
    Gradients,
    ColorGroups,
    ParagraphStyles,
    CharacterStyles,
    ObjectStyles,
    CellStyles,
    TableStyles,
    Layers,
    Spreads,
    Pages,
    MasterPages,
    Links,
    Articles,
    Hyperlinks,
    Bookmarks,
    CrossReferences,
    Conditions,
    ConditionSets,
    Fonts,
    IndexTopics,
    /// Concept 2 — the Ink Manager's ink list (one row per spot
    /// swatch, carrying its output-time settings).
    Inks,
    /// panels.md gaps 9/10/19 — `<Section>` numbering definitions
    /// (one row per section). Backs the Pages panel's section bands.
    Sections,
    /// W3.A0 — the document's stories (one row per `<Story>`, carrying
    /// character/paragraph counts + the overset flag). The same
    /// `StorySummary` list `paged.stories()` already builds. Backs
    /// link-panel / preflight surfaces that bind against the collection
    /// rather than the bespoke `stories()` accessor.
    Stories,
    /// W1.22 (engine gap 22) — the document's `<NumberingList>`
    /// resources (one row per list, carrying its continuity flags).
    /// Backs `documentCollection:numberingLists` — the editor's
    /// list-definitions surface. Additive read-only collection.
    NumberingLists,
}

impl CollectionName {
    /// String form matching the TS `CollectionName` union — used by
    /// the script-host generic `paged.collection(name)` to round-trip
    /// from a JS string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Swatches => "swatches",
            Self::Gradients => "gradients",
            Self::ColorGroups => "colorGroups",
            Self::ParagraphStyles => "paragraphStyles",
            Self::CharacterStyles => "characterStyles",
            Self::ObjectStyles => "objectStyles",
            Self::CellStyles => "cellStyles",
            Self::TableStyles => "tableStyles",
            Self::Layers => "layers",
            Self::Spreads => "spreads",
            Self::Pages => "pages",
            Self::MasterPages => "masterPages",
            Self::Links => "links",
            Self::Articles => "articles",
            Self::Hyperlinks => "hyperlinks",
            Self::Bookmarks => "bookmarks",
            Self::CrossReferences => "crossReferences",
            Self::Conditions => "conditions",
            Self::ConditionSets => "conditionSets",
            Self::Fonts => "fonts",
            Self::IndexTopics => "indexTopics",
            Self::Inks => "inks",
            Self::Sections => "sections",
            Self::Stories => "stories",
            Self::NumberingLists => "numberingLists",
        }
    }

    // Inherent `from_str` returns `Option` (unknown name → `None`); the
    // `FromStr` trait would force a `Result`/`Err` type and change callers.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "swatches" => Self::Swatches,
            "gradients" => Self::Gradients,
            "colorGroups" => Self::ColorGroups,
            "paragraphStyles" => Self::ParagraphStyles,
            "characterStyles" => Self::CharacterStyles,
            "objectStyles" => Self::ObjectStyles,
            "cellStyles" => Self::CellStyles,
            "tableStyles" => Self::TableStyles,
            "layers" => Self::Layers,
            "spreads" => Self::Spreads,
            "pages" => Self::Pages,
            "masterPages" => Self::MasterPages,
            "links" => Self::Links,
            "articles" => Self::Articles,
            "hyperlinks" => Self::Hyperlinks,
            "bookmarks" => Self::Bookmarks,
            "crossReferences" => Self::CrossReferences,
            "conditions" => Self::Conditions,
            "conditionSets" => Self::ConditionSets,
            "fonts" => Self::Fonts,
            "indexTopics" => Self::IndexTopics,
            "inks" => Self::Inks,
            "sections" => Self::Sections,
            "stories" => Self::Stories,
            "numberingLists" => Self::NumberingLists,
            _ => return None,
        })
    }
}

/// SDK Phase 5 (D1) — singleton document-level state. Per
/// `panel-catalog-and-sdk-extension.md` §5.6. Powers the Info panel,
/// status bar, and any chrome that reflects whole-document state
/// (vs. selection state). Scalar reads of singleton properties; the
/// six fields cover the v1 panel needs.
///
/// `dirty` mirrors the Project's "has uncommitted edits since the
/// last save" flag (always `false` at v1 since there's no
/// save/export path through the worker yet — the flag exists so
/// the Info panel and tab title can react when one lands).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct DocumentMeta {
    pub page_count: u32,
    pub active_page: Option<PageId>,
    /// User-facing measurement unit — `"pt"` / `"px"` / `"in"` /
    /// `"mm"` / `"cm"` / `"pica"` etc. Empty when the IDML doesn't
    /// declare a default and the renderer hasn't established one.
    pub units: String,
    /// IDML's document colour mode — `"cmyk"` / `"rgb"`. Empty when
    /// the source doesn't declare it.
    pub color_mode: String,
    /// Human-readable document name. Often the source `.idml`
    /// filename minus extension; empty for synthetic / in-memory
    /// documents.
    pub document_name: String,
    /// `true` when the worker has applied a mutation since
    /// `LoadDocument`. Reset on save/export when that path lands.
    pub dirty: bool,
    /// Editor-ops — document defaults for newly-created objects (the
    /// triple `SetDocumentDefaults` writes). `None` = no fill / no
    /// stroke / engine-default weight.
    #[serde(default)]
    pub default_fill_color: Option<String>,
    #[serde(default)]
    pub default_stroke_color: Option<String>,
    #[serde(default)]
    pub default_stroke_weight: Option<f32>,
    /// Concept 2 — active colour-management settings (the state
    /// `SetColorSettings` writes; seeded from the IDML designmap's
    /// `CMYKProfile`/`SolidColorIntent` at load). `cmyk_profile_name`
    /// is `None` until a registered profile is active by name.
    #[serde(default)]
    pub cmyk_profile_name: Option<String>,
    /// Concept 3 — true when ACTUAL profile bytes back the working
    /// space (explicit load bytes or a registry hit). The NAME above
    /// can be a designmap declaration with no bytes behind it — the
    /// export dialog's X-4 gate needs this, not the name.
    #[serde(default)]
    pub cmyk_profile_active: bool,
    #[serde(default)]
    pub rgb_policy: Option<String>,
    #[serde(default)]
    pub rendering_intent: Option<String>,
    #[serde(default)]
    pub black_point_compensation: Option<bool>,
    /// Concept 2 — active soft-proof condition (`None` = proofing
    /// off) + its paper-white flag.
    #[serde(default)]
    pub proof_profile_name: Option<String>,
    #[serde(default)]
    pub proof_simulate_paper_white: Option<bool>,
    /// Concept 2 (Ink Manager) — global "Use Standard Lab Values
    /// for Spots" toggle.
    #[serde(default)]
    pub use_standard_lab_for_spots: Option<bool>,
}

/// SDK Phase 3 — one swatch's identity + display name + kind.
/// Surfaced by `CanvasModel::swatches()` and the `paged.swatches()`
/// host fn so collection-backed panels (Swatches, the color picker
/// dropdown, the Character/Stroke fill-color enum-select) can
/// enumerate the document's colour palette without re-parsing the
/// graphic resource.
///
/// `kind` is the IDML colour-model discriminant — `"process"` for
/// CMYK/RGB/Lab process colours, `"spot"` for named-ink swatches
/// (PANTONE etc.), `"mixedInk"` / `"mixedInkGroup"` for those
/// composites, and the literal labels `"none"` / `"paper"` /
/// `"black"` / `"registration"` for the four special swatches
/// IDML treats as built-ins. Renderers use this to badge the
/// swatch grid.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SwatchSummary {
    pub self_id: String,
    pub name: String,
    pub kind: String,
}

/// SDK Phase 3 — one paragraph style's identity + display name +
/// based-on link. Surfaced by `CanvasModel::paragraph_styles()`
/// (and `paged.paragraphStyles()`) so collection-backed Style
/// panels can render the hierarchy without re-parsing styles.xml.
/// The `based_on` field is the parent style's `selfId` (the cascade
/// root); `None` means this is a top-level style.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphStyleSummary {
    pub self_id: String,
    pub name: String,
    pub based_on: Option<String>,
    /// styles.next-style (W1.22) — the style's `NextStyle` reference
    /// (the style applied to the following paragraph when the user
    /// presses Enter at this paragraph's end). `None` ⇒ no chain
    /// declared. Additive `#[serde(default)]` field — the editor
    /// reads it to implement the typing-time next-style flow; the
    /// renderer never acts on it. No protocol bump on its own.
    #[serde(default)]
    pub next_style: Option<String>,
}

/// SDK Phase 3 — one character style's summary. Same shape as
/// `ParagraphStyleSummary`; separate type so a future SwatchPicker
/// composition can disambiguate styles in its options source.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct CharacterStyleSummary {
    pub self_id: String,
    pub name: String,
    pub based_on: Option<String>,
}

/// SDK Phase 5 (v1 sweep) — one spread summary. Backs
/// `documentCollection:spreads`. `pageCount` is the number of
/// `<Page>` children in the spread; `label` is the spread's
/// `Self` id (or filename when missing).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SpreadSummary {
    pub self_id: String,
    pub label: String,
    pub page_count: u32,
    /// W3.A0 — the spread's live `<Guide>` set, refreshed on every
    /// `collection("spreads")` request. `DocumentHandle.ruler_guides`
    /// is load-time-only (it doesn't pick up `InsertGuide` /
    /// `MoveGuide` / `DeleteGuide` mutations), so the editor re-queries
    /// this collection after an undo/redo to re-sync its overlay
    /// mirror. Empty for spreads with no guides.
    #[serde(default)]
    pub guides: Vec<GuideSummary>,
}

/// W3.A0 — one live ruler guide on a spread, carried inline on
/// [`SpreadSummary`]. `id` is the positional id the guide-CRUD
/// mutations mint (`"Guide/<spreadSelf>/<index>"`), so the editor can
/// address a `MoveGuide` / `DeleteGuide` at it without a second
/// round-trip. `position` is the page-local coordinate on the
/// perpendicular axis (x for `Vertical`, y for `Horizontal`).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GuideSummary {
    /// Positional id — `"Guide/<spreadSelf>/<index>"`. Matches the id
    /// `Operation::InsertGuide` mints (see `apply::guide_id_for`).
    pub id: String,
    /// `"vertical"` (snaps on x) or `"horizontal"` (snaps on y).
    pub orientation: crate::model::GuideOrientationWire,
    /// Page-local coordinate on the perpendicular axis (pt).
    pub position: f32,
    /// Zero-based index into the spread's pages (IDML's `PageIndex`).
    pub page_index: u32,
}

/// SDK Phase 5 (v1 sweep) — one page summary. Backs
/// `documentCollection:pages`. Mirrors `DocumentHandle.page_ids` plus
/// `page_sizes_pt` so a Pages-as-collection panel can render a
/// thumbnail/label list. The Navigator (existing legacy panel)
/// uses the same data through a different surface.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct PageSummary {
    /// Stable id (matches `PageId` everywhere else).
    pub self_id: String,
    /// 1-based index — what the user types in "Go to page #".
    pub index: u32,
    /// `[width, height]` in points.
    pub size_pt: [f32; 2],
    /// panels.md gap 10 — page margins in pt (from the page's
    /// `<MarginPreference>`). All four default to 0 when the page
    /// declared no margins. The editor's margin-box overlay insets
    /// the page rect by these.
    #[serde(default)]
    pub margin_top_pt: f32,
    #[serde(default)]
    pub margin_left_pt: f32,
    #[serde(default)]
    pub margin_bottom_pt: f32,
    #[serde(default)]
    pub margin_right_pt: f32,
    /// panels.md gap 10 — column grid inside the margin box.
    /// `column_count` defaults to 1, `column_gutter_pt` to 0.
    #[serde(default)]
    pub column_count: u32,
    #[serde(default)]
    pub column_gutter_pt: f32,
    /// panels.md gap 10 — document bleed in pt (top, left, bottom,
    /// right), from `<DocumentPreference>`. Document-level (the same
    /// values on every page); carried per-page so the overlay can
    /// draw the bleed box without a second round-trip. All 0 when the
    /// document declares no bleed.
    #[serde(default)]
    pub bleed_top_pt: f32,
    #[serde(default)]
    pub bleed_left_pt: f32,
    #[serde(default)]
    pub bleed_bottom_pt: f32,
    #[serde(default)]
    pub bleed_right_pt: f32,
}

/// panels.md gaps 9/10/19 — one `<Section>` definition. Backs
/// `documentCollection:sections`. The Pages panel groups page
/// thumbnails by section and labels each group with its prefix +
/// numbering style; `start_page_index` + `page_count` let it draw
/// the section bands. `page_count` is computed by walking the body
/// pages between this section's start and the next section's start
/// (or the document end).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SectionSummary {
    /// IDML `Self` id of the `<Section>`.
    pub self_id: String,
    /// `SectionPrefix` (e.g. `"A-"`); empty when the section has no
    /// prefix or doesn't include it in labels.
    pub prefix: String,
    /// Page-number style — `"arabic"` / `"upperRoman"` /
    /// `"lowerRoman"` / `"upperAlpha"` / `"lowerAlpha"`. The label a
    /// panel renders next to the section band.
    pub label_style: String,
    /// 0-based flat body-page index where this section begins (the
    /// page whose `Self` matches `PageStart`). `None` when the
    /// section's start page can't be located in the built document.
    #[serde(default)]
    pub start_page_index: Option<u32>,
    /// Number of body pages this section spans (up to the next
    /// section's start, or the document end).
    pub page_count: u32,
}

/// SDK Phase 5 (v1 sweep) — one master-spread summary. Backs
/// `documentCollection:masterPages`. Documents typically ship 1–3
/// master spreads (A-Master, B-Master, …) that pages reference
/// via `AppliedMaster`.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct MasterPageSummary {
    pub self_id: String,
    pub label: String,
    pub page_count: u32,
}

/// SDK Phase 5 (v1 sweep) — one cell-style summary. Backs
/// `documentCollection:cellStyles`. Apply-an-entity via
/// `AppliedCellStyle` is wire-shape-only (UnsupportedProperty
/// until the Table NodeId surface lands); the panel can still
/// list defined styles today.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct CellStyleSummary {
    pub self_id: String,
    pub name: String,
    pub based_on: Option<String>,
}

/// SDK Phase 5 (v1 sweep) — one table-style summary. Backs
/// `documentCollection:tableStyles`. Same shape + apply-an-entity
/// pattern as `CellStyleSummary`.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct TableStyleSummary {
    pub self_id: String,
    pub name: String,
    pub based_on: Option<String>,
}

/// SDK Phase 5 (v1 sweep) — one font family/style entry derived
/// from the document's content. The parse layer doesn't carry a
/// font registry — fonts are referenced from runs + paragraph
/// styles. The accessor walks them and dedups; the result is the
/// set of typefaces *used* by the document.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct FontSummary {
    /// Family name (`"Open Sans"`, `"Helvetica Neue"`, …). Used as
    /// the row react-key.
    pub family: String,
    /// Number of runs/styles that reference this family. Surfaces
    /// "this font is used N times" without a full audit pass.
    pub reference_count: u32,
    /// panels.md gap 4 — `true` when the family can't be resolved to
    /// face bytes by the worker's font registry (`BytesResolver`),
    /// so the renderer substituted a fallback. The Fonts/Preflight
    /// panel flags these in red. `false` means at least one style of
    /// the family resolved.
    ///
    /// `embedded` is intentionally omitted: IDML packages reference
    /// fonts by name (the `Fonts/Font_*.xml` resource carries no face
    /// bytes), so the engine can't honestly say whether a font is
    /// "embedded" — only whether it's installed/registered. Surfacing
    /// a fabricated `embedded` flag would mislead the panel.
    #[serde(default)]
    pub is_missing: bool,
    /// W1.23 — the distinct style strings observed for this family,
    /// sorted. Populated from the document's own `FontStyle` strings
    /// (character runs + paragraph/character style defaults) unioned
    /// with the styles registered for the family via `RegisterFont`.
    /// The glyphs / fonts panel renders these as the per-family style
    /// list. Additive field (rides v35) — `#[serde(default)]` keeps the
    /// wire backward-compatible, so an older consumer that doesn't know
    /// the field reads an empty list.
    #[serde(default)]
    pub styles: Vec<String>,
}

/// SDK Phase 5 (v1 sweep) — resolved colour readout for a single
/// swatch. The Color panel uses this to surface "what does this
/// swatch actually look like" — CMYK percentages for spot / CMYK
/// process inks, and an RGB hex string for the display fallback
/// the renderer paints with. Editor sliders are v2.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ColorPreview {
    pub self_id: String,
    pub name: String,
    /// IDML colour model — `"process"` / `"spot"` / `"mixedInk"`
    /// / `"none"` / `"paper"` / `"black"` / `"registration"`.
    pub model: String,
    /// CMYK percent values (0..=100). `None` for non-CMYK swatches
    /// (e.g. RGB / Lab process colours; spots whose alternate
    /// isn't CMYK).
    pub cmyk: Option<[f32; 4]>,
    /// Display RGB as `#rrggbb`. Always present (the renderer
    /// computes a fallback RGB for every swatch).
    pub rgb_hex: String,
    /// Concept 2 — out-of-gamut against the document's active CMYK
    /// working space (false when no working profile is configured).
    #[serde(default)]
    pub out_of_gamut: bool,
    /// Concept 2 — the RAW authored space + channels (IDML units),
    /// so the swatch editor seeds losslessly (a Lab swatch edits in
    /// Lab, not via its display RGB).
    #[serde(default)]
    pub space: Option<String>,
    #[serde(default)]
    pub value: Option<Vec<f32>>,
}

/// Concept 2 — full gradient detail: the stop table the ramp
/// editor mutates and the chips render. Stops carry the swatch REF
/// (gradients reference swatches, never inline colours — edits to a
/// component swatch propagate, spot stops survive to Separation at
/// export) plus a display-resolved hex for painting the ramp.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GradientDetail {
    pub self_id: String,
    pub name: String,
    /// "linear" | "radial" | "unknown".
    pub kind: String,
    pub stops: Vec<GradientStopWire>,
}

/// Concept 2 — one resolved gradient stop.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GradientStopWire {
    /// `Color/<id>` reference — the model identity.
    pub stop_color_ref: String,
    /// Display-resolved `#rrggbb` via the active CMM (ramp render).
    pub resolved_rgb_hex: String,
    /// 0..=100 position along the ramp.
    pub location_pct: f32,
    /// 0..=100 blend midpoint toward the NEXT stop; `None` = 50.
    pub midpoint_pct: Option<f32>,
}

/// Concept 2 — one ink row for the Ink Manager: a spot swatch's
/// identity + its OUTPUT-TIME settings. Converting to process or
/// aliasing never edits the swatch itself (AC-8) — these are
/// separations decisions consumed by Concept 3's export encoding
/// (and, for `useStandardLabForSpots`, by the preview resolver).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct InkSummary {
    /// The spot swatch's `Color/<id>`.
    pub spot_id: String,
    /// The ink/colourant name (the swatch name — for spots this IS
    /// the colourant identity).
    pub name: String,
    pub convert_to_process: bool,
    /// Output as another ink's plate (`Color/<id>` of the alias
    /// target). `None` = own plate.
    pub alias_to: Option<String>,
}

/// SDK Phase 5 (v1 sweep) — one `<Article>` summary. Backs
/// `documentCollection:articles`.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ArticleSummary {
    pub self_id: String,
    pub name: String,
    pub members: Vec<String>,
}

/// SDK Phase 5 (v1 sweep) — one `<Hyperlink>` summary. Backs
/// `documentCollection:hyperlinks`.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct HyperlinkSummary {
    pub self_id: String,
    pub name: String,
    pub source: String,
    pub destination: String,
}

/// SDK Phase 5 (v1 sweep) — one `<Bookmark>` summary. Backs
/// `documentCollection:bookmarks`.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkSummary {
    pub self_id: String,
    pub name: String,
    pub destination: String,
}

/// SDK Phase 5 (v1 sweep) — one `<CrossReferenceSource>` summary.
/// Backs `documentCollection:crossReferences`.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct CrossReferenceSummary {
    pub self_id: String,
    pub name: String,
    pub format: String,
    pub destination: String,
}

/// SDK Phase 5 (v1 sweep) — one `<Topic>` summary. Backs
/// `documentCollection:indexTopics`.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct IndexTopicSummary {
    pub self_id: String,
    pub name: String,
    pub sort_order: String,
}

/// SDK Phase 5 (v1 sweep) — one `<ConditionSet>` definition. Backs
/// `documentCollection:conditionSets` per §5.1. Each entry is a
/// named grouping of Condition self_ids; the editor's Conditions
/// panel can use this to toggle a set as a unit (v2 affordance).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ConditionSetSummary {
    pub self_id: String,
    pub name: String,
    /// Member Condition self_ids the set wraps.
    pub conditions: Vec<String>,
}

/// SDK Phase 5 (v1 sweep) — one `<ColorGroup>` definition. Backs
/// `documentCollection:colorGroups` per §5.1. A user-defined
/// grouping of `Color` self_ids the document organises its
/// palette into.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ColorGroupSummary {
    pub self_id: String,
    pub name: String,
    /// Member color/swatch self_ids the group wraps.
    pub members: Vec<String>,
}

/// SDK Phase 5 (v1 sweep) — one `<Condition>` definition. Backs
/// `documentCollection:conditions` per `panel-catalog-and-sdk-
/// extension.md` §5.1. The Conditions panel renders this for
/// inspection; per-condition visibility toggling requires a new
/// `Operation::SetConditionVisible` that v1 doesn't ship yet.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ConditionSummary {
    pub self_id: String,
    pub name: String,
    /// Default `true` when the IDML doesn't specify (`Visible`
    /// attribute is optional).
    pub visible: bool,
    /// `"Underline"` / `"Highlight"` / `"None"` (or empty).
    pub indicator_method: String,
}

/// W1.22 (engine gap 22) — one `<NumberingList>` resource. Backs
/// `documentCollection:numberingLists`. The editor's list-definitions
/// surface renders this; `continue_across_stories` is the flag that
/// drives cross-story numbering continuity in the renderer.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct NumberingListSummary {
    pub self_id: String,
    pub name: String,
    /// `ContinueNumbersAcrossStories`. Default `false` when the IDML
    /// doesn't specify.
    pub continue_across_stories: bool,
    /// `ContinueNumbersAcrossDocuments` (round-trip only). Default
    /// `false`.
    pub continue_across_documents: bool,
}

/// SDK Phase 5 (v1 sweep) — one placed-image link summary. Backs
/// `documentCollection:links` per `panel-catalog-and-sdk-extension.md`
/// §5.1. Each entry is a `(frame, image_link)` pair derived from
/// the parse layer's `Rectangle::image_link` / `Oval::image_link` /
/// `Polygon::image_link` fields. The Links panel renders this list
/// for inspection; the per-link "relocate" / "update" actions land
/// when those Operations ship.
///
/// `host_kind` lets a future panel disambiguate "this link sits on
/// a Rectangle vs. an Oval". `host_self_id` is the host frame's
/// IDML `Self` id; the panel uses it as the row react-key.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct LinkSummary {
    pub host_self_id: String,
    pub host_kind: String,
    pub uri: String,
    /// panels.md gap 2 — `"ok"` when the build resolved + decoded the
    /// link, `"missing"` when the renderer fell back to the grey
    /// missing-image placeholder (`ImageLinkMissing` /
    /// `ImageDecodeFailed` diagnostic for this frame). Derived from
    /// the build's render diagnostics, so it reflects the SAME
    /// resolution outcome the rendered page shows.
    #[serde(default)]
    pub status: String,
    /// panels.md gap 3 — placed-image colour space (`"CMYK"` /
    /// `"RGB"` / `"Gray"` / `"LAB"`), from the `<Image Space>`
    /// attribute InDesign baked at export. `None` when the IDML
    /// omits it (synthetic fixtures, vector placements).
    #[serde(default)]
    pub colorspace: Option<String>,
    /// panels.md gap 3 — effective ppi at print size (native ppi ÷
    /// placement scale), from the `<Image EffectivePpi>` attribute.
    /// The number a preflight resolution check compares against a
    /// 300-ppi floor. `None` when the IDML omits it.
    #[serde(default)]
    pub effective_ppi: Option<f32>,
}

/// SDK Phase 5 (v1 sweep) — one object style's summary. Backs
/// `documentCollection:objectStyles` per `panel-catalog-and-sdk-
/// extension.md` §5.1; consumed by the Object Styles panel via
/// the `collection-select` primitive to drive an
/// `appliedObjectStyle` write on the selected frame.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ObjectStyleSummary {
    pub self_id: String,
    pub name: String,
    pub based_on: Option<String>,
}

/// SDK Phase 3 — one gradient swatch's summary. `kind` is the
/// IDML `Type` attribute — `"linear"` / `"radial"` — so a picker
/// composition can icon-badge linear vs radial.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GradientSummary {
    pub self_id: String,
    pub name: String,
    pub kind: String,
}

/// SDK Phase 3 — one story's identity + total character length.
/// Surfaced by `CanvasModel::stories()` and the `paged.stories()`
/// script host function so consumers can pick valid character
/// ranges (e.g. `[0, length)` is always a well-formed StoryRange).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct StorySummary {
    /// IDML `Self` id (`Story/u123`).
    pub self_id: String,
    /// Total character count across every `CharacterRun.text` in
    /// every paragraph. The largest valid `StoryRange.end`.
    pub character_count: u32,
    /// Number of paragraphs. Useful for binding-renderer fallbacks
    /// that want to address "the whole story" without computing
    /// the character count.
    pub paragraph_count: u32,
    /// panels.md gap 1 — `true` when this story's text overflowed the
    /// last frame in its chain at build time (overset). Derived from
    /// the build's `OversetTextDropped` diagnostics; drives the
    /// Preflight panel + the red "+" overset badge on the frame.
    #[serde(default)]
    pub overset: bool,
}

/// Inspector P1 — one node in the scene tree. Children are nested
/// (Spread → Page → Group? → frame leaf). `kind` is a short label
/// the panel renders ("Spread", "Page", "TextFrame", "Group", …).
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SceneTreeNode {
    /// Element id when the node is selectable (frames, groups). For
    /// Spread / Page rows that don't address into the gesture spine,
    /// `None`.
    #[serde(default)]
    pub id: Option<crate::element_selection::ElementId>,
    pub kind: String,
    /// Human-readable label. For frames falls back to the kind + raw
    /// id; for pages uses the parsed `<Page Name>`.
    pub label: String,
    #[serde(default)]
    pub children: Vec<SceneTreeNode>,
}

/// Step 5 — `RequestPathAnchors` reply payload. `anchors.len()` may
/// be zero (e.g. a Rectangle with no `<PathGeometry>`); the overlay
/// treats that as "nothing to draw" without surfacing an error.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct PathAnchorsResult {
    pub id: crate::element_selection::ElementId,
    pub page_id: PageId,
    pub anchors: Vec<PathAnchorTriple>,
    /// Per-contour boundaries. Empty for the common single-contour
    /// case so callers can iterate a single subpath without special-
    /// casing the empty `subpath_starts` vector.
    pub subpath_starts: Vec<u32>,
    /// Parallel to `subpath_starts` (or, when `subpath_starts` is
    /// empty, a single entry for the single contour). `true` ⇒ the
    /// contour is open. Lets the overlay emit closing-edge insert
    /// hit-zones for closed subpaths only.
    #[serde(default)]
    pub subpath_open: Vec<bool>,
    /// `[a, b, c, d, tx, ty]`. None ⇒ identity.
    #[serde(default)]
    pub item_transform: Option<[f32; 6]>,
}

/// B-06 — `RequestNearestPathPoint` reply payload. Coordinates are
/// in the element's local space (the `PathAnchors` space).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct NearestPathPointResult {
    /// Flat index of the segment's START anchor.
    pub seg_start: u32,
    /// Flat index of the segment's END anchor (wraps to the subpath
    /// start on a closing segment).
    pub seg_end: u32,
    /// Curve parameter on that segment, 0..=1.
    pub t: f32,
    pub point: [f32; 2],
    pub distance: f32,
}

/// Phase 4 Step 2 — per-rebuild layout cache statistics.
///
/// Sent piggyback on `MutationApplied` / `UndoApplied` / `RedoApplied`
/// so the main thread's HUD can show the incremental-layout win.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct LayoutCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub len: usize,
    pub capacity: usize,
    /// Phase 4 instrumentation — wall-clock duration of the rebuild
    /// that produced these stats, in milliseconds. Lets the HUD
    /// compare cache wins against the underlying budget (AC-E-1
    /// requires < 32 ms).
    pub rebuild_ms: f32,
    // ---- W1.24 (audit B18) — additive RebuildStats breakdown. -------
    // These ride v35 (additive, `#[serde(default)]`): the existing
    // `rebuild_ms` already shipped on this struct, so a richer
    // breakdown is a back-compatible field add, not a new message kind
    // — no PROTOCOL_VERSION bump (governance rule 1). A pre-W1.24 main
    // thread that omits them deserialises to 0; the editor HUD reads
    // them to show "build X ms / op Y ms over N pages" instead of one
    // opaque number.
    /// Wall-clock of the scene edit that preceded the rebuild, ms.
    #[serde(default)]
    pub op_apply_ms: f32,
    /// Pages in the freshly built document.
    #[serde(default)]
    pub pages: u32,
    /// Paragraphs laid out (relayout cost scales with this).
    #[serde(default)]
    pub paragraphs: u32,
    /// Monotone rebuild counter (initial load = 1).
    #[serde(default)]
    pub rebuilds: u64,
    /// Undo-log depth after this rebuild (B19 cap visible here — never
    /// exceeds `paged_canvas::MAX_APPLIED_LOG`).
    #[serde(default)]
    pub applied_log_len: u32,
}

impl From<paged_text::CacheStats> for LayoutCacheStats {
    fn from(s: paged_text::CacheStats) -> Self {
        Self {
            hits: s.hits,
            misses: s.misses,
            len: s.len,
            capacity: s.capacity,
            rebuild_ms: 0.0,
            op_apply_ms: 0.0,
            pages: 0,
            paragraphs: 0,
            rebuilds: 0,
            applied_log_len: 0,
        }
    }
}

impl LayoutCacheStats {
    /// W1.24 (audit B18) — fold a model's `RebuildStats` breakdown onto
    /// the wire stats. The dispatch layer calls this after a mutation /
    /// undo / redo so the main thread gets the op-apply / pages /
    /// paragraphs / rebuild-count detail alongside the cache hit ratio.
    /// `rebuild_ms` stays whatever the caller measured end-to-end (it
    /// already includes op-apply + build); the added fields are the
    /// finer split the model captured internally.
    pub fn with_rebuild_stats(mut self, s: &crate::RebuildStats) -> Self {
        self.op_apply_ms = s.op_apply_ms as f32;
        self.pages = s.pages as u32;
        self.paragraphs = s.paragraphs as u32;
        self.rebuilds = s.rebuilds;
        self.applied_log_len = s.applied_log_len as u32;
        self
    }
}

/// A content-space mutation. Phase 1 carries the *envelope* only —
/// the worker rejects each variant with `WorkerError::NotImplemented`.
/// Phase 3 lights these up incrementally.
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
        cell: Option<crate::selection::TextCellAddr>,
    },
    DeleteRange {
        story_id: String,
        start: u32,
        end: u32,
        /// W1.13 — cell qualifier (see `InsertText::cell`).
        #[serde(default)]
        cell: Option<crate::selection::TextCellAddr>,
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
    },
    /// W0.5 — insert a field marker (page-number etc.) at a story
    /// offset. Routes to `Operation::InsertField`.
    InsertField {
        story_id: String,
        offset: u32,
        field: paged_mutate::operation::FieldKind,
    },
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
        element_id: crate::element_selection::ElementId,
        index: u32,
        anchor: paged_mutate::operation::PathAnchorSpec,
        #[serde(default)]
        prev_subpath_starts: Option<Vec<u32>>,
    },
    /// Track J — remove the anchor at flat `index` from any path-
    /// bearing element. UI dispatches from Backspace/Delete on the
    /// selected anchor.
    PathPointRemove {
        element_id: crate::element_selection::ElementId,
        index: u32,
    },
    /// Editor-ops (Scissors) — cut the path at the anchor at flat
    /// `index`: a closed contour opens there (the anchor splits into
    /// two coincident endpoints); an open contour splits into two.
    /// For a mid-segment cut the editor sends
    /// `Batch [PathPointInsert (the de Casteljau split), PathOpenAt]`
    /// so the whole cut is one undo step.
    PathOpenAt {
        element_id: crate::element_selection::ElementId,
        index: u32,
    },
    /// B-05 (protocol v30) — replace the element's path with its
    /// stroke-expansion outline. Geometry-only: the editor composes
    /// paint transfer (fill := old stroke, stroke := none) as a
    /// Batch alongside this op. `cap`: `"butt"|"round"|"square"`;
    /// `join`: `"miter"|"round"|"bevel"`.
    OutlineStroke {
        element_id: crate::element_selection::ElementId,
        width: f32,
        cap: String,
        join: String,
        miter_limit: f32,
    },
    /// B-05 (protocol v30) — inset (`delta < 0`) / outset
    /// (`delta > 0`) of a single closed contour.
    OffsetPath {
        element_id: crate::element_selection::ElementId,
        delta: f32,
        join: String,
        miter_limit: f32,
    },
    /// B-05 (protocol v30) — re-express the path within `tolerance`
    /// pt max deviation with fewer anchors.
    SimplifyPath {
        element_id: crate::element_selection::ElementId,
        tolerance: f32,
    },
    /// B-04 (protocol v32) — group leaf page items on one spread.
    /// Reference-based and z-order-neutral (the group takes the
    /// earliest member's paint slot). Reply carries the minted group
    /// id as `createdId` so the editor can select it.
    CreateGroup {
        member_ids: Vec<crate::element_selection::ElementId>,
    },
    /// B-04 (protocol v32) — dissolve a group; members return to the
    /// group's paint slot in stored order.
    DissolveGroup {
        group_id: String,
    },
    /// Plugin-metadata carrier (protocol v33) — one Label
    /// `KeyValuePair` on a leaf page item. `value: None` deletes the
    /// entry. The engine gates the write (reserved `x-paged:` key
    /// namespace, 64 KiB cap, JSON envelope); plugin identity is
    /// enforced one layer up in the SDK's document door.
    SetPluginMetadata {
        element_id: crate::element_selection::ElementId,
        key: String,
        #[serde(default)]
        value: Option<String>,
    },
    /// Track J — toggle the curve type of an anchor between corner
    /// (handles equal to anchor) and smooth (handles derived from
    /// neighbour tangents). UI dispatches from a double-click on
    /// the anchor.
    PathPointCurveType {
        element_id: crate::element_selection::ElementId,
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
        element_id: crate::element_selection::ElementId,
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
        element_id: crate::element_selection::ElementId,
        path: paged_mutate::PropertyPath,
        value: paged_mutate::Value,
    },
    /// SDK Phase 5 (v1 sweep) — Pathfinder boolean op routed
    /// through `Operation::PathfinderBoolean`. Same wire shape
    /// the Pathfinder panel emits on a button click.
    PathfinderBoolean {
        kept: crate::element_selection::ElementId,
        others: Vec<crate::element_selection::ElementId>,
        kind: paged_mutate::PathfinderKind,
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
    DuplicatePage {
        page: PageId,
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
}

impl Mutation {
    /// Short string discriminant for logging + `NotImplemented` errors.
    pub fn discriminant(&self) -> &'static str {
        match self {
            Self::InsertText { .. } => "InsertText",
            Self::DeleteRange { .. } => "DeleteRange",
            Self::ApplyStyle { .. } => "ApplyStyle",
            Self::InsertField { .. } => "InsertField",
            Self::MoveFrame { .. } => "MoveFrame",
            Self::ResizeFrame { .. } => "ResizeFrame",
            Self::LinkFrames { .. } => "LinkFrames",
            Self::UnlinkFrames { .. } => "UnlinkFrames",
            Self::InsertPage { .. } => "InsertPage",
            Self::DeletePage { .. } => "DeletePage",
            Self::ResizePage { .. } => "ResizePage",
            Self::InsertFrame { .. } => "InsertFrame",
            Self::InsertTextFrame { .. } => "InsertTextFrame",
            Self::DeleteFrame { .. } => "DeleteFrame",
            Self::InsertLine { .. } => "InsertLine",
            Self::InsertPath { .. } => "InsertPath",
            Self::SetDocumentDefaults { .. } => "SetDocumentDefaults",
            Self::SetColorSettings { .. } => "SetColorSettings",
            Self::SetProofSetup { .. } => "SetProofSetup",
            Self::ImportSwatchLibrary { .. } => "ImportSwatchLibrary",
            Self::SetInkSetting { .. } => "SetInkSetting",
            Self::SetUseStandardLabForSpots { .. } => "SetUseStandardLabForSpots",
            Self::PathPointInsert { .. } => "PathPointInsert",
            Self::PathPointRemove { .. } => "PathPointRemove",
            Self::PathOpenAt { .. } => "PathOpenAt",
            Self::OutlineStroke { .. } => "OutlineStroke",
            Self::OffsetPath { .. } => "OffsetPath",
            Self::SimplifyPath { .. } => "SimplifyPath",
            Self::CreateGroup { .. } => "CreateGroup",
            Self::DissolveGroup { .. } => "DissolveGroup",
            Self::SetPluginMetadata { .. } => "SetPluginMetadata",
            Self::PathPointCurveType { .. } => "PathPointCurveType",
            Self::PathPointSet { .. } => "PathPointSet",
            Self::Batch { .. } => "Batch",
            Self::LayerSetVisible { .. } => "LayerSetVisible",
            Self::LayerSetLocked { .. } => "LayerSetLocked",
            Self::LayerSetPrintable { .. } => "LayerSetPrintable",
            Self::LayerSetName { .. } => "LayerSetName",
            Self::LayerMove { .. } => "LayerMove",
            Self::LayerInsert { .. } => "LayerInsert",
            Self::LayerRemove { .. } => "LayerRemove",
            Self::SetElementProperty { .. } => "SetElementProperty",
            Self::PathfinderBoolean { .. } => "PathfinderBoolean",
            Self::CreateSwatch { .. } => "CreateSwatch",
            Self::EditSwatch { .. } => "EditSwatch",
            Self::DeleteSwatch { .. } => "DeleteSwatch",
            Self::CreateGradient { .. } => "CreateGradient",
            Self::EditGradient { .. } => "EditGradient",
            Self::DeleteGradient { .. } => "DeleteGradient",
            Self::CreateColorGroup { .. } => "CreateColorGroup",
            Self::EditColorGroup { .. } => "EditColorGroup",
            Self::DeleteColorGroup { .. } => "DeleteColorGroup",
            Self::CreateNumberingList { .. } => "CreateNumberingList",
            Self::EditNumberingList { .. } => "EditNumberingList",
            Self::DeleteNumberingList { .. } => "DeleteNumberingList",
            Self::CreateParagraphStyle { .. } => "CreateParagraphStyle",
            Self::RenameParagraphStyle { .. } => "RenameParagraphStyle",
            Self::DeleteParagraphStyle { .. } => "DeleteParagraphStyle",
            Self::CreateCharacterStyle { .. } => "CreateCharacterStyle",
            Self::RenameCharacterStyle { .. } => "RenameCharacterStyle",
            Self::DeleteCharacterStyle { .. } => "DeleteCharacterStyle",
            Self::CreateObjectStyle { .. } => "CreateObjectStyle",
            Self::RenameObjectStyle { .. } => "RenameObjectStyle",
            Self::DeleteObjectStyle { .. } => "DeleteObjectStyle",
            Self::CreateCellStyle { .. } => "CreateCellStyle",
            Self::RenameCellStyle { .. } => "RenameCellStyle",
            Self::DeleteCellStyle { .. } => "DeleteCellStyle",
            Self::CreateTableStyle { .. } => "CreateTableStyle",
            Self::RenameTableStyle { .. } => "RenameTableStyle",
            Self::DeleteTableStyle { .. } => "DeleteTableStyle",
            Self::SetStyleProperty { .. } => "SetStyleProperty",
            Self::InsertOval { .. } => "InsertOval",
            Self::InsertGuide { .. } => "InsertGuide",
            Self::MoveGuide { .. } => "MoveGuide",
            Self::DeleteGuide { .. } => "DeleteGuide",
            Self::SetConditionVisible { .. } => "SetConditionVisible",
            Self::ActivateConditionSet { .. } => "ActivateConditionSet",
            Self::ApplyMasterToPage { .. } => "ApplyMasterToPage",
            Self::DuplicatePage { .. } => "DuplicatePage",
            Self::InsertSection { .. } => "InsertSection",
            Self::EditSection { .. } => "EditSection",
            Self::DeleteSection { .. } => "DeleteSection",
            Self::SetRowHeight { .. } => "SetRowHeight",
            Self::SetColumnWidth { .. } => "SetColumnWidth",
            Self::InsertTableRow { .. } => "InsertTableRow",
            Self::DeleteTableRow { .. } => "DeleteTableRow",
            Self::InsertTableColumn { .. } => "InsertTableColumn",
            Self::DeleteTableColumn { .. } => "DeleteTableColumn",
            Self::InsertHeaderRow { .. } => "InsertHeaderRow",
            Self::RemoveHeaderRow { .. } => "RemoveHeaderRow",
            Self::InsertFooterRow { .. } => "InsertFooterRow",
            Self::RemoveFooterRow { .. } => "RemoveFooterRow",
            Self::SetCellSpan { .. } => "SetCellSpan",
        }
    }
}

/// Typed `LoadDocument` failure. Each variant maps to a specific UI
/// recovery in the main thread (corrupted file → "try another file";
/// missing font → "install or substitute"; etc.).
#[derive(Debug, Clone, Error, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase", tag = "kind", content = "message")]
pub enum LoadError {
    /// paged-parse failed (zip / xml structural problem).
    #[error("idml parse error: {0}")]
    Parse(String),
    /// paged-scene resolution failed (missing master, broken
    /// cross-reference, etc.).
    #[error("idml scene error: {0}")]
    Scene(String),
    /// pipeline::build_document failed.
    #[error("idml build error: {0}")]
    Build(String),
}

/// Typed worker-side error for non-load operations. Mutations,
/// hit-tests, page requests all report through this. Variants are
/// kept stable across protocol versions.
#[derive(Debug, Clone, Error, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    content = "details"
)]
pub enum WorkerError {
    /// Feature not yet implemented in this phase. `what` carries a
    /// short identifier (e.g. `"Mutation::InsertText"`).
    #[error("not implemented: {what}")]
    NotImplemented { what: String },
    /// Requested page id is unknown — caller is using a stale id
    /// from before a page was deleted, or a typo.
    #[error("unknown page id: {page_id}")]
    UnknownPage { page_id: PageId },
    /// Worker has no document loaded — `LoadDocument` must come first.
    #[error("no document loaded")]
    NoDocument,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_to_worker_envelope_roundtrips_through_json() {
        let msg = MainToWorker {
            seq: 7,
            protocol: PROTOCOL_VERSION,
            kind: MainToWorkerKind::LoadDocument {
                bytes: ByteBuf::from(vec![1, 2, 3]),
                font: None,
                cmyk_icc_profile: None,
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        // External tag lives at top level — `kind` field decides the
        // payload shape. Check by string match so we lock down the
        // wire format the TS side consumes.
        assert!(
            json.contains("\"kind\":\"loadDocument\""),
            "tag missing: {json}"
        );
        // Inner field rename: cmyk_icc_profile must emit as
        // cmykIccProfile. If this assertion fires, the TS protocol
        // mirror needs updating in lockstep — the browser will see
        // `undefined` for the field and React renders will crash.
        assert!(
            json.contains("\"cmykIccProfile\":") || !json.contains("\"cmyk_icc_profile\":"),
            "camelCase field rename broken: {json}"
        );
        let back: MainToWorker = serde_json::from_str(&json).unwrap();
        match back.kind {
            MainToWorkerKind::LoadDocument { bytes, .. } => {
                assert_eq!(bytes.as_slice(), &[1, 2, 3]);
            }
            other => panic!("expected LoadDocument, got {other:?}"),
        }
    }

    #[test]
    fn document_handle_serialises_with_camel_case_fields() {
        // Frozen wire format the TS DocumentHandle interface
        // consumes. Regression test for the snake-case leak that
        // showed up as "Cannot read properties of undefined" in
        // React when the rename_all dropped through.
        let h = crate::model::DocumentHandle {
            doc_id: "doc-1".into(),
            page_count: 2,
            page_ids: vec![PageId("p1".into()), PageId("p2".into())],
            page_sizes_pt: vec![(612.0, 792.0), (612.0, 792.0)],
            stats: crate::model::DocumentStats {
                spreads: 1,
                pages: 2,
                frames: 4,
                stories: 1,
                paragraphs: 2,
                runs: 3,
                glyphs: 50,
                lines: 5,
                overset_stories: 0,
            },
            ruler_guides: Vec::new(),
        };
        let json = serde_json::to_string(&h).unwrap();
        for needle in [
            "\"docId\":",
            "\"pageCount\":",
            "\"pageIds\":",
            "\"pageSizesPt\":",
        ] {
            assert!(json.contains(needle), "{needle} missing in {json}");
        }
        for snake in [
            "\"doc_id\":",
            "\"page_count\":",
            "\"page_ids\":",
            "\"page_sizes_pt\":",
        ] {
            assert!(!json.contains(snake), "{snake} leaked snake_case: {json}");
        }
    }

    #[test]
    fn request_snapshot_payload_uses_camel_case() {
        let msg = MainToWorker {
            seq: 1,
            protocol: PROTOCOL_VERSION,
            kind: MainToWorkerKind::RequestSnapshot {
                page_id: PageId("p1".into()),
                target_width_px: 256,
                dpi: None,
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"requestSnapshot\""), "{json}");
        assert!(json.contains("\"pageId\":"), "{json}");
        assert!(json.contains("\"targetWidthPx\":"), "{json}");
        assert!(!json.contains("target_width_px"), "snake leaked: {json}");
    }

    #[test]
    fn mutation_discriminant_is_stable() {
        let m = Mutation::InsertText {
            story_id: "s1".into(),
            offset: 0,
            text: "x".into(),
            cell: None,
        };
        assert_eq!(m.discriminant(), "InsertText");
        let json = serde_json::to_string(&m).unwrap();
        // Wire tag is camelCase but `discriminant()` is PascalCase
        // for human-readable error messages. Both contracts.
        assert!(json.contains("\"op\":\"insertText\""), "tag drift: {json}");
    }

    #[test]
    fn w05_mutations_round_trip_through_the_mutate_envelope() {
        // Every W0.5 Mutation variant survives the JSON envelope the
        // worker decodes `Mutate(Mutation)` from.
        let muts = vec![
            Mutation::LinkFrames {
                from: "TextFrame/a".into(),
                to: "TextFrame/b".into(),
            },
            Mutation::UnlinkFrames {
                frame: "TextFrame/a".into(),
            },
            Mutation::ApplyStyle {
                story_id: "Story/u1".into(),
                start: 0,
                end: 5,
                style: "ParagraphStyle/Body".into(),
                scope: paged_mutate::operation::StyleScope::Paragraph,
            },
            Mutation::InsertField {
                story_id: "Story/u1".into(),
                offset: 2,
                field: paged_mutate::operation::FieldKind::PageNumber,
            },
            Mutation::InsertOval {
                page_id: PageId("Page/u1".into()),
                bounds: (1.0, 2.0, 3.0, 4.0),
            },
            Mutation::InsertTextFrame {
                page_id: PageId("Page/u1".into()),
                bounds: (1.0, 2.0, 3.0, 4.0),
            },
            Mutation::InsertGuide {
                spread_id: "Spread/u1".into(),
                orientation: paged_mutate::operation::GuideOrientationSpec::Horizontal,
                position: 50.0,
                page_index: 0,
            },
            Mutation::MoveGuide {
                guide_id: "Guide/Spread/u1/0".into(),
                position: 75.0,
            },
            Mutation::DeleteGuide {
                guide_id: "Guide/Spread/u1/0".into(),
            },
            Mutation::SetConditionVisible {
                condition: "Condition/A".into(),
                visible: false,
            },
            Mutation::ActivateConditionSet {
                set: "ConditionSet/Print".into(),
            },
            Mutation::ApplyMasterToPage {
                page: PageId("Page/u1".into()),
                master: Some("MasterSpread/uA".into()),
            },
            Mutation::DuplicatePage {
                page: PageId("Page/u1".into()),
            },
            Mutation::InsertSection {
                at_page: PageId("Page/u1".into()),
                prefix: Some("A-".into()),
                numbering_style: Some("UpperRoman".into()),
                start_at: Some(1),
            },
            Mutation::EditSection {
                section_id: "Section/u0".into(),
                prefix: Some(None),
                numbering_style: None,
                start_at: Some(Some(3)),
            },
            Mutation::DeleteSection {
                section_id: "Section/u0".into(),
            },
        ];
        for m in muts {
            let disc = m.discriminant();
            let env = MainToWorker {
                seq: 1,
                protocol: PROTOCOL_VERSION,
                kind: MainToWorkerKind::Mutate(m),
            };
            let json = serde_json::to_string(&env).unwrap();
            let back: MainToWorker = serde_json::from_str(&json).unwrap();
            match back.kind {
                MainToWorkerKind::Mutate(m2) => assert_eq!(m2.discriminant(), disc, "{json}"),
                other => panic!("unexpected: {other:?}"),
            }
        }
    }

    /// W1.2 — `MoveFrame` survives the mutate envelope with its fields
    /// intact (frame id + the 6-element `ItemTransform`). The variant
    /// already shipped at v34; W1.2 only un-stubs its semantics, so no
    /// bump — this test pins the wire shape.
    #[test]
    fn w12_move_frame_round_trips_through_the_mutate_envelope() {
        let m = Mutation::MoveFrame {
            frame_id: "Rectangle/r1".into(),
            transform: [1.0, 0.0, 0.0, 1.0, 12.5, -7.0],
        };
        let env = MainToWorker {
            seq: 1,
            protocol: PROTOCOL_VERSION,
            kind: MainToWorkerKind::Mutate(m),
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: MainToWorker = serde_json::from_str(&json).unwrap();
        match back.kind {
            MainToWorkerKind::Mutate(Mutation::MoveFrame {
                frame_id,
                transform,
            }) => {
                assert_eq!(frame_id, "Rectangle/r1");
                assert_eq!(transform, [1.0, 0.0, 0.0, 1.0, 12.5, -7.0]);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// W1.1 — `Value::Lengths` serialises as the additive
    /// `{ type: "lengths", value: [...] }` wire shape and round-trips
    /// (incl. the empty-clear list). Rides the current protocol — an
    /// additive `Value` variant does not bump (the `TabStops` /
    /// `ParagraphRule` precedent).
    #[test]
    fn w11_value_lengths_wire_shape_round_trips() {
        let v = paged_mutate::Value::Lengths(vec![6.0, 3.0]);
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#"{"type":"lengths","value":[6.0,3.0]}"#);
        let back: paged_mutate::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back, v);
        // Empty-clear list also round-trips.
        let empty = paged_mutate::Value::Lengths(Vec::new());
        let json = serde_json::to_string(&empty).unwrap();
        assert_eq!(json, r#"{"type":"lengths","value":[]}"#);
        assert_eq!(
            serde_json::from_str::<paged_mutate::Value>(&json).unwrap(),
            empty
        );
    }

    #[test]
    fn protocol_version_is_v35() {
        assert_eq!(PROTOCOL_VERSION.0, 35);
    }

    /// W1.23 — the new `RequestParagraphBounds` request kind serialises
    /// with the camelCase tag the TS side switches on and round-trips
    /// its `storyId` / `offset` payload.
    #[test]
    fn w123_request_paragraph_bounds_round_trips() {
        let env = MainToWorker {
            seq: 9,
            protocol: PROTOCOL_VERSION,
            kind: MainToWorkerKind::RequestParagraphBounds {
                story_id: "story1".into(),
                offset: 7,
                cell: None,
            },
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(
            json.contains("\"kind\":\"requestParagraphBounds\""),
            "tag missing: {json}"
        );
        assert!(json.contains("\"storyId\":\"story1\""), "{json}");
        let back: MainToWorker = serde_json::from_str(&json).unwrap();
        match back.kind {
            MainToWorkerKind::RequestParagraphBounds {
                story_id,
                offset,
                cell: _,
            } => {
                assert_eq!(story_id, "story1");
                assert_eq!(offset, 7);
            }
            other => panic!("expected RequestParagraphBounds, got {other:?}"),
        }
    }

    /// W1.23 — the `ParagraphBoundsResult` reply kind serialises with
    /// its camelCase tag and round-trips the `bounds` payload (both the
    /// `Some` span and the `None` "no resolution" case).
    #[test]
    fn w123_paragraph_bounds_result_round_trips() {
        let env = WorkerToMain {
            seq: Some(9),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::ParagraphBoundsResult {
                bounds: Some(crate::geometry::ParagraphBounds { start: 6, end: 10 }),
            },
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(
            json.contains("\"kind\":\"paragraphBoundsResult\""),
            "tag missing: {json}"
        );
        let back: WorkerToMain = serde_json::from_str(&json).unwrap();
        match back.kind {
            WorkerToMainKind::ParagraphBoundsResult { bounds } => {
                let b = bounds.expect("Some span");
                assert_eq!((b.start, b.end), (6, 10));
            }
            other => panic!("expected ParagraphBoundsResult, got {other:?}"),
        }
        // The `None` case also round-trips.
        let none = WorkerToMain {
            seq: Some(9),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::ParagraphBoundsResult { bounds: None },
        };
        let json = serde_json::to_string(&none).unwrap();
        let back: WorkerToMain = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back.kind,
            WorkerToMainKind::ParagraphBoundsResult { bounds: None }
        ));
    }

    /// W1.23 — the additive `FontSummary.styles` field serialises as a
    /// camelCase array and survives a round-trip, AND an older payload
    /// that omits the field deserialises to an empty list (the
    /// `#[serde(default)]` back-compat that lets it ride v35 without an
    /// extra bump).
    #[test]
    fn w123_font_summary_styles_field_round_trips_and_defaults() {
        let fs = FontSummary {
            family: "Open Sans".into(),
            reference_count: 3,
            is_missing: false,
            styles: vec!["Bold".into(), "Regular".into()],
        };
        let json = serde_json::to_string(&fs).unwrap();
        assert!(json.contains("\"styles\":[\"Bold\",\"Regular\"]"), "{json}");
        let back: FontSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.styles, vec!["Bold".to_string(), "Regular".to_string()]);
        // A legacy payload with no `styles` key defaults to empty.
        let legacy = r#"{"family":"Inter","referenceCount":1,"isMissing":false}"#;
        let back: FontSummary = serde_json::from_str(legacy).unwrap();
        assert!(back.styles.is_empty(), "missing styles must default empty");
    }

    /// W1.24 (audit B18) — the additive `RebuildStats` breakdown on
    /// `LayoutCacheStats` serialises as camelCase fields, round-trips,
    /// AND an older payload that omits them deserialises to 0 (the
    /// `#[serde(default)]` back-compat that lets them ride v35 without a
    /// PROTOCOL_VERSION bump — `rebuild_ms` already shipped on this
    /// struct, so the breakdown is a back-compatible field add).
    #[test]
    fn w124_layout_cache_stats_rebuild_breakdown_round_trips_and_defaults() {
        let stats = LayoutCacheStats {
            hits: 9,
            misses: 1,
            len: 10,
            capacity: 16,
            rebuild_ms: 12.5,
            op_apply_ms: 0.3,
            pages: 4,
            paragraphs: 42,
            rebuilds: 7,
            applied_log_len: 3,
        };
        let json = serde_json::to_string(&stats).unwrap();
        // Additive fields present in camelCase.
        assert!(json.contains("\"opApplyMs\":0.3"), "{json}");
        assert!(json.contains("\"pages\":4"), "{json}");
        assert!(json.contains("\"paragraphs\":42"), "{json}");
        assert!(json.contains("\"rebuilds\":7"), "{json}");
        assert!(json.contains("\"appliedLogLen\":3"), "{json}");
        let back: LayoutCacheStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stats);

        // A legacy payload carrying only the pre-W1.24 fields
        // deserialises with the new fields defaulted to 0 — no bump.
        let legacy = r#"{"hits":1,"misses":0,"len":1,"capacity":4,"rebuildMs":5.0}"#;
        let back: LayoutCacheStats = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.op_apply_ms, 0.0, "missing opApplyMs defaults to 0");
        assert_eq!(back.pages, 0, "missing pages defaults to 0");
        assert_eq!(back.paragraphs, 0);
        assert_eq!(back.rebuilds, 0);
        assert_eq!(back.applied_log_len, 0);
        assert_eq!(back.rebuild_ms, 5.0, "pre-existing rebuildMs still parses");
    }

    #[test]
    fn w3a1_table_mutations_round_trip_through_the_mutate_envelope() {
        let muts = vec![
            Mutation::SetRowHeight {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                row: 1,
                height: Some(42.0),
            },
            Mutation::SetColumnWidth {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                col: 0,
                width: None,
            },
            Mutation::InsertTableRow {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                at: 1,
            },
            Mutation::DeleteTableRow {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                at: 0,
            },
            Mutation::InsertTableColumn {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                at: 2,
            },
            Mutation::DeleteTableColumn {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                at: 1,
            },
        ];
        for m in muts {
            let disc = m.discriminant();
            let env = MainToWorker {
                seq: 1,
                protocol: PROTOCOL_VERSION,
                kind: MainToWorkerKind::Mutate(m),
            };
            let json = serde_json::to_string(&env).unwrap();
            let back: MainToWorker = serde_json::from_str(&json).unwrap();
            match back.kind {
                MainToWorkerKind::Mutate(m2) => assert_eq!(m2.discriminant(), disc, "{json}"),
                other => panic!("unexpected: {other:?}"),
            }
        }
    }

    #[test]
    fn w3a1_hit_result_table_context_round_trips() {
        let hr = HitResult {
            frame_id: Some("frameA".into()),
            story_id: Some("u10".into()),
            table_context: Some(TableHitContext {
                table_id: "t1".into(),
                row: 1,
                col: 0,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&hr).unwrap();
        // camelCase wire keys.
        assert!(json.contains("\"tableContext\":"), "{json}");
        assert!(json.contains("\"tableId\":\"t1\""), "{json}");
        let back: HitResult = serde_json::from_str(&json).unwrap();
        let tc = back.table_context.expect("table_context round-trips");
        assert_eq!((tc.row, tc.col), (1, 0));
        assert_eq!(tc.table_id, "t1");
        // A non-table hit serialises `table_context: null`.
        let plain = HitResult {
            frame_id: Some("rectA".into()),
            ..Default::default()
        };
        let back2: HitResult =
            serde_json::from_str(&serde_json::to_string(&plain).unwrap()).unwrap();
        assert!(back2.table_context.is_none());
    }

    #[test]
    fn worker_to_main_unsolicited_pages_dirty_roundtrips() {
        let msg = WorkerToMain {
            seq: None,
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::PagesDirty {
                page_ids: vec![PageId("p1".into()), PageId("p2".into())],
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: WorkerToMain = serde_json::from_str(&json).unwrap();
        assert!(back.seq.is_none());
        match back.kind {
            WorkerToMainKind::PagesDirty { page_ids } => {
                assert_eq!(page_ids.len(), 2);
                assert_eq!(page_ids[0].as_str(), "p1");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn load_error_serialises_with_kind_message() {
        let e = LoadError::Parse("malformed zip".into());
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"kind\":\"parse\""), "{json}");
        assert!(json.contains("malformed zip"), "{json}");
    }

    #[test]
    fn sections_collection_name_round_trips() {
        // panels.md gaps 9/10/19 — the new collection name maps both
        // ways and serialises as the camelCase tag the TS union uses.
        assert_eq!(CollectionName::Sections.as_str(), "sections");
        assert_eq!(
            CollectionName::from_str("sections"),
            Some(CollectionName::Sections)
        );
        let json = serde_json::to_string(&CollectionName::Sections).unwrap();
        assert_eq!(json, "\"sections\"");
    }

    #[test]
    fn caret_nav_request_round_trips_through_json() {
        // panels.md (W0.6 caret queries) — the new request variant
        // survives the JSON envelope round-trip the worker uses.
        let msg = MainToWorker {
            seq: 9,
            protocol: PROTOCOL_VERSION,
            kind: MainToWorkerKind::RequestCaretNav {
                story_id: "u10".into(),
                offset: 5,
                direction: crate::geometry::CaretDirection::Down,
                cell: None,
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"requestCaretNav\""), "{json}");
        assert!(json.contains("\"direction\":\"down\""), "{json}");
        let back: MainToWorker = serde_json::from_str(&json).unwrap();
        match back.kind {
            MainToWorkerKind::RequestCaretNav {
                story_id,
                offset,
                direction,
                cell: _,
            } => {
                assert_eq!(story_id, "u10");
                assert_eq!(offset, 5);
                assert_eq!(direction, crate::geometry::CaretDirection::Down);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn caret_nav_and_line_bounds_replies_round_trip() {
        let r = WorkerToMain {
            seq: Some(1),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::CaretNavResult { offset: Some(12) },
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: WorkerToMain = serde_json::from_str(&json).unwrap();
        match back.kind {
            WorkerToMainKind::CaretNavResult { offset } => assert_eq!(offset, Some(12)),
            other => panic!("wrong variant: {other:?}"),
        }

        let r = WorkerToMain {
            seq: Some(1),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::LineBoundsResult {
                bounds: Some(crate::geometry::LineBounds {
                    line_start: 3,
                    line_end: 9,
                }),
            },
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: WorkerToMain = serde_json::from_str(&json).unwrap();
        match back.kind {
            WorkerToMainKind::LineBoundsResult { bounds } => {
                let b = bounds.expect("bounds present");
                assert_eq!((b.line_start, b.line_end), (3, 9));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn word_bounds_request_round_trips_through_json() {
        // Aftercare-A — the new request variant survives the JSON
        // envelope with the camelCase tag the worker switches on.
        let msg = MainToWorker {
            seq: 13,
            protocol: PROTOCOL_VERSION,
            kind: MainToWorkerKind::RequestWordBounds {
                story_id: "u10".into(),
                offset: 7,
                cell: None,
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"requestWordBounds\""), "{json}");
        let back: MainToWorker = serde_json::from_str(&json).unwrap();
        match back.kind {
            MainToWorkerKind::RequestWordBounds {
                story_id,
                offset,
                cell: _,
            } => {
                assert_eq!(story_id, "u10");
                assert_eq!(offset, 7);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn word_bounds_reply_round_trips_through_json() {
        // Aftercare-A — `WordBoundsResult` carries a camelCase
        // `[start, end)` span; `None` is the unresolved-story case.
        let r = WorkerToMain {
            seq: Some(13),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::WordBoundsResult {
                bounds: Some(crate::geometry::WordBounds { start: 4, end: 9 }),
            },
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"kind\":\"wordBoundsResult\""), "{json}");
        assert!(json.contains("\"start\":4"), "{json}");
        let back: WorkerToMain = serde_json::from_str(&json).unwrap();
        match back.kind {
            WorkerToMainKind::WordBoundsResult { bounds } => {
                let b = bounds.expect("bounds present");
                assert_eq!((b.start, b.end), (4, 9));
            }
            other => panic!("wrong variant: {other:?}"),
        }

        let none = WorkerToMain {
            seq: Some(13),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::WordBoundsResult { bounds: None },
        };
        let back: WorkerToMain =
            serde_json::from_str(&serde_json::to_string(&none).unwrap()).unwrap();
        match back.kind {
            WorkerToMainKind::WordBoundsResult { bounds } => assert!(bounds.is_none()),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// W3.B2 — the IDML save-back message kinds (ride v29) survive the
    /// JSON envelope with the camelCase tag + field contract the editor
    /// mirrors. `ExportIdml` (main→worker) carries no payload; the
    /// replies `IdmlExported` / `ExportIdmlFailed` carry the bytes /
    /// error.
    #[test]
    fn idml_export_message_kinds_round_trip_through_json() {
        // Request: empty-payload struct variant. The external tag must
        // be `exportIdml`.
        let req = MainToWorker {
            seq: 11,
            protocol: PROTOCOL_VERSION,
            kind: MainToWorkerKind::ExportIdml {},
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("\"kind\":\"exportIdml\""),
            "tag drift: {json}"
        );
        let back: MainToWorker = serde_json::from_str(&json).unwrap();
        assert!(matches!(back.kind, MainToWorkerKind::ExportIdml {}));

        // Success reply: idml_bytes renders as a number[] (ByteBuf) and
        // the field is camelCase on the wire.
        let ok = WorkerToMain {
            seq: Some(11),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::IdmlExported {
                idml_bytes: ByteBuf::from(vec![80, 75, 3, 4]), // "PK\x03\x04"
            },
        };
        let json = serde_json::to_string(&ok).unwrap();
        assert!(
            json.contains("\"kind\":\"idmlExported\""),
            "tag drift: {json}"
        );
        assert!(
            json.contains("\"idmlBytes\":"),
            "field rename broken: {json}"
        );
        assert!(!json.contains("idml_bytes"), "snake leaked: {json}");
        let back: WorkerToMain = serde_json::from_str(&json).unwrap();
        match back.kind {
            WorkerToMainKind::IdmlExported { idml_bytes } => {
                assert_eq!(idml_bytes.as_slice(), &[80, 75, 3, 4]);
            }
            other => panic!("wrong variant: {other:?}"),
        }

        // Failure reply: flat-string error, mirroring ExportPdfFailed.
        let err = WorkerToMain {
            seq: Some(11),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::ExportIdmlFailed {
                error: "no document loaded".into(),
            },
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(
            json.contains("\"kind\":\"exportIdmlFailed\""),
            "tag drift: {json}"
        );
        let back: WorkerToMain = serde_json::from_str(&json).unwrap();
        match back.kind {
            WorkerToMainKind::ExportIdmlFailed { error } => {
                assert_eq!(error, "no document loaded");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// B-09 / W-08 — the typed budget-exhaustion field rides the
    /// `ScriptResult` reply over the wire. A wall-clock abort serialises
    /// its `budgetKind` as the camelCase tag the host matches on, and
    /// round-trips back to the typed enum. Additive on protocol v35.
    #[test]
    fn script_result_budget_kind_round_trips() {
        let env = WorkerToMain {
            seq: Some(7),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::ScriptResult {
                output: vec!["[log] hi".into()],
                error: Some("runtime budget exceeded: …".into()),
                budget_kind: Some(ScriptBudgetKind::WallClock),
            },
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(
            json.contains("\"kind\":\"scriptResult\""),
            "tag drift: {json}"
        );
        assert!(
            json.contains("\"budgetKind\":\"wallClock\""),
            "typed budget kind missing / mis-tagged: {json}"
        );
        let back: WorkerToMain = serde_json::from_str(&json).unwrap();
        match back.kind {
            WorkerToMainKind::ScriptResult {
                error, budget_kind, ..
            } => {
                assert!(error.is_some());
                assert_eq!(budget_kind, Some(ScriptBudgetKind::WallClock));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// Additive-on-the-wire proof: a `ScriptResult` for an ordinary
    /// (non-budget) outcome omits `budgetKind` from the JSON, and a
    /// PRE-EXISTING reply that never had the field still decodes — so
    /// older producers/consumers ride v35 unchanged.
    #[test]
    fn script_result_omits_budget_kind_and_decodes_legacy() {
        // Producing: ordinary result → no budgetKind key.
        let env = WorkerToMain {
            seq: Some(7),
            protocol: PROTOCOL_VERSION,
            kind: WorkerToMainKind::ScriptResult {
                output: vec![],
                error: None,
                budget_kind: None,
            },
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(
            !json.contains("budgetKind"),
            "ordinary result must omit budgetKind: {json}"
        );

        // Consuming: a legacy reply with no budgetKind field decodes
        // with budget_kind defaulting to None. The envelope is
        // adjacently tagged (`tag = "kind", content = "payload"`), so
        // the variant fields live under `payload`.
        let legacy =
            r#"{"seq":7,"protocol":35,"kind":"scriptResult","payload":{"output":[],"error":null}}"#;
        let back: WorkerToMain = serde_json::from_str(legacy).unwrap();
        match back.kind {
            WorkerToMainKind::ScriptResult { budget_kind, .. } => {
                assert_eq!(budget_kind, None);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
