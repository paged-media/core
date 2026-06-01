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
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(23);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
pub struct ProtocolVersion(pub u32);

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
    RequestPage {
        page_id: PageId,
        lod: LodTier,
    },
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
    RequestMarqueeHits {
        page_id: PageId,
        rect: [f32; 4],
    },
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
    RequestGroupLeaves {
        group_id: String,
    },
    /// Step 5 — request the path-anchor table for a single Polygon /
    /// Rectangle / Oval / TextFrame so the path-edit overlay can draw
    /// one dot per anchor + Bezier-handle pair. Reply: `PathAnchors`.
    /// Elements that don't carry an `anchors` array (rectangles
    /// declared via `GeometricBounds` only) come back with `anchors`
    /// empty.
    RequestPathAnchors {
        id: crate::element_selection::ElementId,
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
    RequestCollection {
        name: CollectionName,
    },
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
    RequestColorPreview {
        swatch_id: String,
    },
    /// Scripting Stage 2 — execute a JS source string against the
    /// loaded document. The script's mutations route through
    /// `Operation::SetProperty` (same channel as gestures + REPL)
    /// so undo/redo work identically. Reply: `ScriptResult`.
    ExecuteScript { source: String },
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
    SnapshotFailed { error: crate::snapshot::SnapshotError },
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
    },
    /// Phase 3 Item 4 — rect-per-line geometry for a selection range.
    SelectionGeometry {
        rects: Vec<crate::SelectionRect>,
    },
    /// Phase 3 Item 3 — caret position for a selection.
    CaretGeometry {
        caret: Option<crate::geometry::CaretGeometry>,
    },
    /// Phase 3 Item 7 — undo applied. `undone_seq` is the
    /// `applied_seq` of the mutation that was reversed.
    UndoApplied {
        undone_seq: u64,
        applied_seq: u64,
        page_ids: Vec<PageId>,
        cache_stats: LayoutCacheStats,
    },
    /// Phase 3 Item 7 — redo applied.
    RedoApplied {
        redone_seq: u64,
        applied_seq: u64,
        page_ids: Vec<PageId>,
        cache_stats: LayoutCacheStats,
    },
    /// `RegisterFont` reply: the font is now part of the worker's
    /// asset resolver.
    FontRegistered { family: String },
    /// `ClearFontRegistry` reply.
    FontRegistryCleared,
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
    ElementGeometry {
        items: Vec<ElementGeometryItem>,
    },
    /// Phase H — `RequestGroupLeaves` reply. Empty when the group id
    /// doesn't resolve.
    GroupLeaves {
        ids: Vec<crate::element_selection::ElementId>,
    },
    /// Step 5 — `RequestPathAnchors` reply. `None` when the id doesn't
    /// resolve or sits on a master spread; `Some` even when the
    /// element's anchor list is empty (lets the caller distinguish
    /// "no path data" from "didn't resolve").
    PathAnchors {
        result: Option<PathAnchorsResult>,
    },
    /// Track M — `RequestLayers` reply. Documents without `<Layer>`
    /// elements (rare; the IDML container always emits at least a
    /// default layer) come back with an empty Vec.
    Layers {
        items: Vec<LayerSummary>,
    },
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
    DocumentMetaReply {
        meta: DocumentMeta,
    },
    /// SDK Phase 5 (v1 sweep) — `RequestColorPreview` reply.
    /// `result` is `None` when the swatch id doesn't resolve.
    ColorPreviewReply {
        result: Option<ColorPreview>,
    },
    /// Inspector P1 — `RequestElementProperties` reply. `None` when
    /// the id doesn't resolve.
    ElementProperties {
        result: Option<ElementProperties>,
    },
    /// Inspector P1 — `RequestSceneTree` reply.
    SceneTree {
        roots: Vec<SceneTreeNode>,
    },
    /// Scripting Stage 2 — `ExecuteScript` reply. `output` is the
    /// concatenated console.* lines; `error` is non-null when the
    /// script threw an unhandled exception.
    ScriptResult {
        output: Vec<String>,
        error: Option<String>,
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
    UnsupportedGesture { reason: String },
    AlreadyActive { handle: crate::gesture::GestureHandle },
    HandleMismatch,
    ElementNotFound { id: crate::element_selection::ElementId },
    RotatedFrameUnsupported,
    EmptySelection,
    MissingAnchor,
    UnknownAnchorPage { page_id: PageId },
    Other { message: String },
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
        }
    }

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
}

/// SDK Phase 5 (v1 sweep) — one page summary. Backs
/// `documentCollection:pages`. Mirrors `DocumentHandle.page_ids`
/// + `page_sizes_pt` so a Pages-as-collection panel can render a
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

/// Phase 4 Step 2 — per-rebuild layout cache statistics.
///
/// Sent piggyback on `MutationApplied` / `UndoApplied` / `RedoApplied`
/// so the main thread's HUD can show the incremental-layout win.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Tsify)]
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
}

impl From<paged_text::CacheStats> for LayoutCacheStats {
    fn from(s: paged_text::CacheStats) -> Self {
        Self {
            hits: s.hits,
            misses: s.misses,
            len: s.len,
            capacity: s.capacity,
            rebuild_ms: 0.0,
        }
    }
}

/// A content-space mutation. Phase 1 carries the *envelope* only —
/// the worker rejects each variant with `WorkerError::NotImplemented`.
/// Phase 3 lights these up incrementally.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "op", content = "args")]
pub enum Mutation {
    InsertText {
        story_id: String,
        offset: u32,
        text: String,
    },
    DeleteRange {
        story_id: String,
        start: u32,
        end: u32,
    },
    ApplyStyle {
        story_id: String,
        start: u32,
        end: u32,
        attributes: serde_json::Value,
    },
    InsertField {
        story_id: String,
        offset: u32,
        field_kind: String,
    },
    MoveFrame {
        frame_id: String,
        transform: [f32; 6],
    },
    ResizeFrame {
        frame_id: String,
        bounds: (f32, f32, f32, f32),
    },
    LinkFrames {
        frame_a: String,
        frame_b: String,
    },
    UnlinkFrames {
        chain_id: String,
        after_frame: String,
    },
    InsertPage {
        after_page_id: Option<PageId>,
        master_id: Option<String>,
    },
    DeletePage {
        page_id: PageId,
    },
    InsertFrame {
        page_id: PageId,
        bounds: (f32, f32, f32, f32),
    },
    DeleteFrame {
        frame_id: String,
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
    Batch { ops: Vec<Mutation> },
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
            Self::InsertFrame { .. } => "InsertFrame",
            Self::DeleteFrame { .. } => "DeleteFrame",
            Self::PathPointInsert { .. } => "PathPointInsert",
            Self::PathPointRemove { .. } => "PathPointRemove",
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
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind", content = "details")]
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
            json.contains("\"cmykIccProfile\":") || json.contains("\"cmyk_icc_profile\":") == false,
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
            },
            ruler_guides: Vec::new(),
        };
        let json = serde_json::to_string(&h).unwrap();
        for needle in ["\"docId\":", "\"pageCount\":", "\"pageIds\":", "\"pageSizesPt\":"] {
            assert!(json.contains(needle), "{needle} missing in {json}");
        }
        for snake in ["\"doc_id\":", "\"page_count\":", "\"page_ids\":", "\"page_sizes_pt\":"] {
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
        };
        assert_eq!(m.discriminant(), "InsertText");
        let json = serde_json::to_string(&m).unwrap();
        // Wire tag is camelCase but `discriminant()` is PascalCase
        // for human-readable error messages. Both contracts.
        assert!(json.contains("\"op\":\"insertText\""), "tag drift: {json}");
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
}
