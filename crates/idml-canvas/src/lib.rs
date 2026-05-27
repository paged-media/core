//! IDML Web Canvas — worker-side data model and facade.
//!
//! Pure Rust. No wasm-bindgen — that lives in `idml-canvas-wasm`,
//! a thin binding layer on top of this crate. Unit-testable via
//! `cargo test`.
//!
//! What this crate owns (per `docs/verso/canvas.md`):
//!
//! - The worker-side `CanvasModel` that wraps a parsed IDML document
//!   and the four-tier pipeline state (content, layout, resolution,
//!   output).
//! - Stable identifiers (`PageId`, `StoryId`, `FrameId`) re-exported
//!   from upstream crates so consumers depend on one surface.
//! - The typed message channel envelopes (`MainToWorker`,
//!   `WorkerToMain`) — versioned serde structs that the wasm crate
//!   wires up to `postMessage`.
//! - The `SharedArrayBuffer` camera contract (`camera::Camera`,
//!   `camera::CameraLayout`) shared between main and worker.
//!
//! Phase 1 (this crate at first landing) provides:
//!
//! - `CanvasModel::load(bytes)` — parses + builds a `BuiltDocument`
//!   in one shot. Replays of `mutate(...)` rebuild from scratch (no
//!   incremental Tier 2 yet — that's Phase 3).
//! - `CanvasModel::display_list_for_page(page_id)` — Tier 4 seam.
//! - `CanvasModel::page_ids()` / `page_count()` — used by the page
//!   navigator + snapshot atlas.
//!
//! Later phases extend this with: anchor + field model (Phase 2),
//! incremental Tier 2 with checkpoints (Phase 3), salsa retrofit
//! (Phase 3).

pub mod camera;
pub mod channel;
pub mod element_selection;
pub mod gesture;
pub mod geometry;
pub mod hit;
pub mod model;
pub mod mutate;
pub mod resolve;
pub mod selection;
pub mod snap;
pub mod snapshot;

pub use camera::{Camera, CameraLayout, CAMERA_SAB_BYTES};
pub use channel::{
    ElementGeometryItem, HitFilter, HitResult, LoadError, MainToWorker, MainToWorkerKind, Mutation,
    ProtocolVersion, WorkerError, WorkerToMain, WorkerToMainKind, PROTOCOL_VERSION,
};
pub use element_selection::{ElementId, ElementSelection, SelectionMode};
pub use gesture::{
    GestureAnchor, GestureError, GestureHandle, GestureModifiers, GestureType, ResizeHandle,
};
pub use snap::{SnapAxis, SnapLine};
pub use hit::HitTestResult;
pub use geometry::{caret_geometry, selection_geometry, CaretGeometry};
pub use mutate::{AppliedText, TextOp, TextOpError};
pub use selection::{ContentSelection, Side};

/// Phase 3 Item 4 — one rect-per-line in page-local coords for a
/// content selection range. Defined in the root so the channel
/// (Item 6) can reference it without depending on a yet-to-land
/// `geometry` module.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, tsify_next::Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRect {
    pub page_id: PageId,
    pub frame_id: Option<String>,
    pub left_pt: f32,
    pub top_pt: f32,
    pub width_pt: f32,
    pub height_pt: f32,
}
pub use model::{
    AppliedRecord, CanvasModel, CanvasOptions, DocumentHandle, DocumentStats, FontEntry,
    FrameMutationOutcome, LoggedMutation,
};
pub use resolve::{
    resolve, AnchorPosition, FieldChange, NumberingMap, ResolutionResult, ResolveOptions,
};
pub use snapshot::{SnapshotError, SnapshotPng};
#[cfg(feature = "cpu")]
pub use snapshot::{
    render_snapshot, render_snapshot_at_dpi, render_snapshot_png, render_snapshot_png_at_dpi,
    Snapshot,
};

// Re-export upstream identifiers + the display-list IR so consumers
// depend on a single root crate.
pub use idml_renderer::{BuiltDocument, BuiltPage, DisplayCommand, DisplayList, PageId};
