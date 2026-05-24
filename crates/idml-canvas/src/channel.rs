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

use idml_renderer::PageId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{DocumentHandle, DocumentStats};

/// Bumped on any incompatible change to the channel envelopes.
/// Main thread compares this against its bundled value at worker
/// handshake and refuses to proceed on mismatch — better to fail
/// loud than to silently desync.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtocolVersion(pub u32);

/// One message from main → worker.
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        bytes: ByteBuf,
        #[serde(default)]
        font: Option<ByteBuf>,
        #[serde(default)]
        cmyk_icc_profile: Option<ByteBuf>,
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
    /// and the canvas at overview zoom.
    RequestSnapshot {
        page_id: PageId,
        target_width_px: u32,
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
    /// Undo the most recent applied mutation. Reply: `UndoApplied`
    /// or `MutationFailed` (when the log is empty).
    Undo,
    /// Re-apply the most recently undone mutation. Reply:
    /// `RedoApplied` or `MutationFailed`.
    Redo,
}

/// Coarse LOD tiers requested by the navigator + canvas (per spec §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HitFilter {
    Frame,
    Text,
    Any,
}

/// Hit-test result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HitResult {
    pub frame_id: Option<String>,
    pub story_id: Option<String>,
    pub offset_within_story: Option<u32>,
    /// Selected frame's bounding box in page-local coordinates.
    /// Returned alongside `frame_id` so the main thread can draw a
    /// selection outline without a second round-trip.
    pub frame_bounds: Option<FrameBounds>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    },
    /// Phase 3 Item 4 — rect-per-line geometry for a selection range.
    SelectionGeometry {
        rects: Vec<crate::SelectionRect>,
    },
    /// Phase 3 Item 7 — undo applied. `undone_seq` is the
    /// `applied_seq` of the mutation that was reversed.
    UndoApplied {
        undone_seq: u64,
        applied_seq: u64,
        page_ids: Vec<PageId>,
    },
    /// Phase 3 Item 7 — redo applied.
    RedoApplied {
        redone_seq: u64,
        applied_seq: u64,
        page_ids: Vec<PageId>,
    },
}

/// A content-space mutation. Phase 1 carries the *envelope* only —
/// the worker rejects each variant with `WorkerError::NotImplemented`.
/// Phase 3 lights these up incrementally.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "op", content = "args")]
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
        }
    }
}

/// Typed `LoadDocument` failure. Each variant maps to a specific UI
/// recovery in the main thread (corrupted file → "try another file";
/// missing font → "install or substitute"; etc.).
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "message")]
pub enum LoadError {
    /// idml-parse failed (zip / xml structural problem).
    #[error("idml parse error: {0}")]
    Parse(String),
    /// idml-scene resolution failed (missing master, broken
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
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "details")]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
