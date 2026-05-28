//! Worker-side canvas data model.
//!
//! `CanvasModel` is the single owner of parsed document state, the
//! built display lists, and (in later phases) the salsa database
//! that memoises Tiers 2–4. Phase-1 implementation is intentionally
//! synchronous and non-incremental: `load()` parses + builds the
//! whole document, and every mutation triggers a fresh rebuild.
//! The point of this phase is to nail down the **API surface** the
//! main thread depends on; incrementality is Phase 3's problem.

use std::collections::HashMap;

use idml_renderer::{
    pipeline, BuiltDocument, BuiltPage, BytesResolver, DisplayList, Document, PageId,
    PipelineOptions,
};
use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

use crate::channel::{LoadError, Mutation};

/// Options that flow through to `idml-renderer::PipelineOptions`.
/// Mirrors the subset of the renderer's options the worker needs
/// to surface to the main thread on `LoadDocument`.
#[derive(Debug, Clone, Default)]
pub struct CanvasOptions {
    /// Default-font fallback. First-entry-still-wins semantics: the
    /// renderer's `PipelineOptions::font` receives this byte slice and
    /// uses it for any `AppliedFont` that doesn't resolve via the
    /// family registry. Kept as a `Vec<Vec<u8>>` so callers that don't
    /// know about the registry (e.g. the dev shell auto-loading
    /// Inter.ttf) can still drop bytes in here without naming them.
    pub fonts: Vec<Vec<u8>>,
    /// Named font registry. Each entry binds an `AppliedFont` family
    /// (and optionally a style like "Bold") to a font payload, mirroring
    /// `idml-inspect --font-family "Family=path"`. Translates 1:1 to
    /// `BytesResolver::add_font` entries on every build/rebuild.
    pub font_registry: Vec<FontEntry>,
    /// CMYK ICC profile bytes for accurate colour. Optional; the
    /// renderer falls back to naive conversion when absent.
    pub cmyk_icc_profile: Option<Vec<u8>>,
}

/// Named font payload used to populate the renderer's per-family
/// asset resolver.
#[derive(Debug, Clone)]
pub struct FontEntry {
    /// IDML family name as it appears in `AppliedFont` (e.g.
    /// `"Poppins"`).
    pub family: String,
    /// IDML style string when known (`"Regular"`, `"Bold Italic"`).
    /// `None` registers the family bare and matches every style via
    /// the bare-family fall-through in `BytesResolver::resolve_font`.
    pub style: Option<String>,
    pub bytes: Vec<u8>,
}

/// One-time facts about a loaded document. Sent to the main thread
/// on a successful `LoadDocument` so the navigator + page count UI
/// can render before the first page is rasterised.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct DocumentHandle {
    /// Stable id assigned by the worker; used by the main thread when
    /// addressing operations to a specific document (the worker may
    /// hold more than one document open in the future).
    pub doc_id: String,
    /// Total page count. Stable for the life of the document unless
    /// a mutation explicitly inserts / deletes pages.
    pub page_count: usize,
    /// Page ids in document order. The navigator displays them as
    /// "page N" with `N = 1 + index`; the canvas uses the ids
    /// directly for cache keys.
    pub page_ids: Vec<PageId>,
    /// Per-page dimensions in points. Same length as `page_ids`.
    /// The navigator needs these to size thumbnails before any
    /// rasterisation has happened.
    pub page_sizes_pt: Vec<(f32, f32)>,
    /// Aggregate counts for debugging / UI badges.
    pub stats: DocumentStats,
}

/// Structural counts. The main thread surfaces these in the debug
/// HUD. Mirrors `idml-renderer::PipelineStats` but lives in serde-
/// friendly form so it can cross the message channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
pub struct DocumentStats {
    pub spreads: usize,
    pub pages: usize,
    pub frames: usize,
    pub stories: usize,
    pub paragraphs: usize,
    pub runs: usize,
    pub glyphs: usize,
    pub lines: usize,
}

/// What `CanvasModel::apply_mutation` returns on success. The
/// `applied_seq` is the monotone id the worker assigns; `page_ids`
/// lists pages the caller must invalidate in its LOD cache; the
/// `inverse` is the op to push onto the undo log.
#[derive(Debug, Clone)]
pub struct MutationOutcome {
    pub applied_seq: u64,
    pub page_ids: Vec<PageId>,
    pub inverse: crate::mutate::TextOp,
}

/// Phase B — frame-mutation analogue of [`MutationOutcome`]. Carries
/// the full `AppliedOperation` (op + inverse + invalidation hint)
/// rather than just the inverse text op, since frame mutations come
/// from the canonical `idml_mutate` log.
#[derive(Debug, Clone)]
pub struct FrameMutationOutcome {
    pub applied_seq: u64,
    pub page_ids: Vec<PageId>,
    pub applied: idml_mutate::AppliedOperation,
}

/// What `CanvasModel::undo` / `redo` return.
#[derive(Debug, Clone)]
pub struct UndoOutcome {
    /// `applied_seq` of the mutation being reversed (undo) or
    /// re-applied (redo).
    pub undone_seq: u64,
    /// Newly assigned `applied_seq` for the undo/redo operation
    /// itself.
    pub applied_seq: u64,
    pub page_ids: Vec<PageId>,
    /// Phase 4 Step 3 — story id touched by the undo/redo, used by
    /// the wasm dispatch to scope GPU cache invalidation. None when
    /// the op carries no story id (frame moves, page inserts) —
    /// reserved for later mutation types.
    pub affected_story_id: Option<String>,
}

fn story_id_of_text_op(op: &crate::mutate::TextOp) -> &str {
    match op {
        crate::mutate::TextOp::InsertText { story_id, .. } => story_id,
        crate::mutate::TextOp::DeleteRange { story_id, .. } => story_id,
    }
}

impl From<&pipeline::PipelineStats> for DocumentStats {
    fn from(s: &pipeline::PipelineStats) -> Self {
        Self {
            spreads: s.spreads,
            pages: s.pages,
            frames: s.frames,
            stories: s.stories,
            paragraphs: s.paragraphs,
            runs: s.runs,
            glyphs: s.glyphs,
            lines: s.lines,
        }
    }
}

/// The worker's view of a single loaded document plus all derived
/// canvas state.
///
/// Today this is a thin wrapper: store the parsed scene + the most
/// recent `BuiltDocument`. Tomorrow (Phase 3) the `BuiltDocument`
/// becomes a salsa-tracked derived value and incremental Tier 2
/// runs against checkpoints stored alongside `scene`.
pub struct CanvasModel {
    doc_id: String,
    pub(crate) scene: Document,
    pub(crate) built: BuiltDocument,
    /// Index from `PageId` to `BuiltDocument::pages` position. Built
    /// once at load and refreshed after every rebuild. Worker callers
    /// (display-list-for-page, snapshot rendering, hit-test) all key
    /// by id; the linear-scan fallback on `BuiltDocument::page` is
    /// fine in absolute terms but salsa-shaped lookups should be O(1).
    page_index: HashMap<PageId, usize>,
    /// Owned option inputs. `PipelineOptions` borrows from these on
    /// every rebuild; storing them owned keeps the worker self-contained.
    font_bytes: Option<Vec<u8>>,
    /// Named per-family payloads consulted via `BytesResolver` during
    /// every (re)build. Owned by the model so the assets resolver
    /// borrowed in `PipelineOptions` doesn't need lifetimes leaking out.
    font_registry: Vec<FontEntry>,
    icc_bytes: Option<Vec<u8>>,
    /// Phase 3 Item 6 — content hash of the scene at load time.
    /// Drives determinism tests: replaying the recorded mutation log
    /// against the same `initial_state_hash` must produce a matching
    /// post-state hash.
    initial_state_hash: [u8; 32],
    /// Monotone counter assigned by the worker for each successfully
    /// applied mutation. The main thread matches against its own
    /// `client_seq` via the `MutationApplied` reply.
    last_applied_seq: u64,
    /// Active text selection mirrored from the main thread.
    pub current_selection: Option<crate::selection::ContentSelection>,
    /// Phase A — active element selection (frames, images, vectors).
    /// Mirrored from the main thread, never enters the Operation log.
    /// Survives mutations and re-layout; the main thread is responsible
    /// for clearing entries whose target was removed.
    pub element_selection: crate::element_selection::ElementSelection,
    /// Phase B — in-flight gesture (translate / future resize / rotate).
    /// Only one gesture is active at a time; `begin_gesture` errors on
    /// re-entry. None when no drag is happening.
    pub(crate) active_gesture: Option<crate::gesture::GestureSession>,
    /// Monotone counter for `GestureHandle` allocation. Bumped on
    /// every successful `begin_gesture`. Persists across cancels so a
    /// stale handle from a previous gesture can never collide with a
    /// fresh one.
    pub(crate) next_gesture_handle: u64,
    /// Phase 3 Item 7 — undo log. Each entry holds the op + inverse
    /// + the applied_seq that was assigned at apply time.
    applied_log: Vec<AppliedRecord>,
    /// Phase 3 Item 7 — redo stack. Populated by `undo()`; consumed
    /// by `redo()`. Cleared when a new mutation lands (standard
    /// editor convention).
    redo_log: Vec<AppliedRecord>,
    /// Phase 4 Step 1 — persistent per-paragraph layout cache.
    /// Installed on every rebuild via `idml_text::cache::with_layout_cache`
    /// so unchanged paragraphs short-circuit Knuth-Plass on
    /// mutation-driven rebuilds. Survives across mutations.
    layout_cache: idml_text::LayoutCache,
    /// Phase 4 Step 3 — map of story id → page ids the story's
    /// frame chain touches. Built after every rebuild. Used by
    /// `apply_mutation` to compute the dirty page set for the GPU
    /// scene cache invalidation hint.
    story_pages: HashMap<String, Vec<PageId>>,
    /// Perf-S — persistent `URI → DecodedImage` cache shared across
    /// rebuilds. Threaded into `PipelineOptions::image_decode_cache`
    /// on every `rebuild_after_mutation` so gesture-driven rebuilds
    /// don't re-decode placed images. Per the corrected perf
    /// investigation, image decoding isn't actually the bottleneck
    /// in the current test harness; this stays as forward-looking
    /// infra for when asset resolvers wire up.
    image_decode_cache:
        std::cell::RefCell<HashMap<String, idml_compose::DecodedImage>>,
    /// Perf-FontTable — pre-built shaping table reused across every
    /// `rebuild_after_mutation`. The `FontTable::build` walk costs
    /// ~225ms on a multi-spread fixture (harvests every paragraph's
    /// cascade-resolved font key, then resolver-fetches bytes per
    /// key). The document's font registry only changes at
    /// loadDocument boundaries — fresh CanvasModel ⇒ fresh table —
    /// so we never need to invalidate mid-lifetime.
    font_table: idml_renderer::FontTable,
    /// Perf-MasterText — per-(master_frame_self_id, page_idx) cache
    /// of the DisplayList delta the master-text pass appends to a
    /// page. The COLD build populates this; every gesture-driven
    /// rebuild hits and skips the emit. Structural mutations clear
    /// it (handled in `apply_operation`) because the master+frame
    /// pass's path-buffer state changes when frames are added/
    /// removed and the cached relative-path-id rebase would
    /// produce visually-correct but order-divergent output. ~161ms
    /// savings per rebuild on a multi-spread fixture.
    master_text_emit_cache: std::cell::RefCell<
        HashMap<(String, usize), idml_renderer::MasterTextEmitDelta>,
    >,
    /// Perf-BodyStory — per-(story_self_id, signature) cache of
    /// the multi-page body-story emission delta. Signature
    /// hashes the chain's frames + wrap_rects on chain pages, so
    /// a story whose chain doesn't see a change keeps hitting
    /// through a drag. Body-story emission is the largest single
    /// cost in `build_document` on a multi-spread fixture
    /// (~613ms); most stories are unaffected by any given gesture
    /// so the hit ratio is high. Cleared by `apply_operation` on
    /// structural commits.
    body_story_emit_cache: std::cell::RefCell<
        HashMap<(String, u64), idml_renderer::BodyStoryEmissionDelta>,
    >,
}

/// One entry in the applied / redo logs.
///
/// Phase B — generalized to hold both text edits (legacy `TextOp`
/// path) and frame mutations (canonical `idml_mutate::AppliedOperation`)
/// so a single Cmd-Z timeline covers both. The full convergence
/// (folding `TextOp` into `idml_mutate::Operation`) is tracked
/// separately and is **out of scope** for Phase B per the plan §3.5.
#[derive(Debug, Clone)]
pub struct AppliedRecord {
    pub applied_seq: u64,
    pub kind: LoggedMutation,
}

#[derive(Debug, Clone)]
pub enum LoggedMutation {
    /// Legacy text edit. `op` is the forward action; `inverse` is the
    /// pre-captured `TextOp` that reverses it. Both live in
    /// `crate::mutate`.
    Text {
        op: crate::mutate::TextOp,
        inverse: crate::mutate::TextOp,
    },
    /// Frame / structural mutation routed through `idml_mutate::apply`.
    /// The `AppliedOperation` already pairs op + inverse + invalidation.
    Frame(idml_mutate::AppliedOperation),
}

impl CanvasModel {
    /// Parse `bytes` and run the renderer pipeline to produce the
    /// initial `BuiltDocument`. Returns a `CanvasModel` the worker
    /// uses to serve all subsequent queries.
    pub fn load(
        doc_id: impl Into<String>,
        bytes: &[u8],
        opts: CanvasOptions,
    ) -> Result<Self, LoadError> {
        let doc_id = doc_id.into();
        let t_parse = phase_now();
        let scene = Document::open(bytes).map_err(|e| LoadError::Parse(e.to_string()))?;
        phase_log("CanvasModel::load parse", t_parse);

        // Honour the first font and the ICC profile. Take ownership
        // up-front so the model is self-contained — no caller-managed
        // lifetimes leaking through.
        let font_bytes = opts.fonts.into_iter().next();
        let icc_bytes = opts.cmyk_icc_profile;
        let font_registry = opts.font_registry;
        let resolver = build_font_resolver(&font_registry, font_bytes.as_deref());

        let t_build = phase_now();
        // Perf-S — image-decode cache populated by the initial
        // build_document then stored on Self below for subsequent
        // rebuilds. The cold load pays the full decode cost; every
        // mutation-driven rebuild after that reuses.
        let image_decode_cache: std::cell::RefCell<
            HashMap<String, idml_compose::DecodedImage>,
        > = std::cell::RefCell::new(HashMap::new());
        // Perf-FontTable — pre-build the shaping table once so the
        // initial build_document + every subsequent
        // rebuild_after_mutation skips the harvest walk
        // (~225ms/call on a multi-spread fixture).
        let font_table_options = PipelineOptions {
            font: font_bytes.as_deref(),
            assets: resolver.as_ref().map(|r| r as &dyn idml_renderer::AssetResolver),
            cmyk_icc_profile: icc_bytes.as_deref(),
            ..PipelineOptions::default()
        };
        let font_table = idml_renderer::FontTable::build(&scene, &font_table_options);
        // Perf-MasterText — empty cache; the initial build_document
        // below populates it as each master-text emit runs.
        let master_text_emit_cache: std::cell::RefCell<
            HashMap<(String, usize), idml_renderer::MasterTextEmitDelta>,
        > = std::cell::RefCell::new(HashMap::new());
        // Perf-BodyStory — same pattern; populated by the initial
        // build, reused by every subsequent rebuild.
        let body_story_emit_cache: std::cell::RefCell<
            HashMap<(String, u64), idml_renderer::BodyStoryEmissionDelta>,
        > = std::cell::RefCell::new(HashMap::new());
        let (built_result, layout_cache) = {
            let options = PipelineOptions {
                font: font_bytes.as_deref(),
                assets: resolver.as_ref().map(|r| r as &dyn idml_renderer::AssetResolver),
                cmyk_icc_profile: icc_bytes.as_deref(),
                image_decode_cache: Some(&image_decode_cache),
                pre_built_font_table: Some(&font_table),
                master_text_emit_cache: Some(&master_text_emit_cache),
                body_story_emit_cache: Some(&body_story_emit_cache),
                ..PipelineOptions::default()
            };
            // Phase 4 Step 1 — install an empty cache for the initial
            // build. Every paragraph misses; the cache fills up so
            // subsequent mutation-driven rebuilds can hit.
            idml_text::cache::with_layout_cache(idml_text::LayoutCache::default(), || {
                pipeline::build_document(&scene, &options)
            })
        };
        let built = built_result.map_err(|e| LoadError::Build(e.to_string()))?;
        phase_log("CanvasModel::load build", t_build);

        let t_post = phase_now();
        let page_index = built
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();

        let initial_state_hash = scene.canonical_hash();
        let story_pages = compute_story_pages(&built);
        phase_log("CanvasModel::load post (index+hash+story_pages)", t_post);
        Ok(Self {
            doc_id,
            scene,
            built,
            page_index,
            font_bytes,
            font_registry,
            icc_bytes,
            initial_state_hash,
            last_applied_seq: 0,
            current_selection: None,
            element_selection: crate::element_selection::ElementSelection::new(),
            active_gesture: None,
            next_gesture_handle: 0,
            applied_log: Vec::new(),
            redo_log: Vec::new(),
            layout_cache,
            story_pages,
            // Perf-S — cache populated by the initial build_document
            // above; subsequent rebuild_after_mutation calls share
            // this RefCell so decode cost amortises.
            image_decode_cache,
            // Perf-FontTable — built once above and reused by every
            // rebuild_after_mutation.
            font_table,
            // Perf-MasterText — cache populated by the initial
            // build above; subsequent rebuilds reuse + structural
            // mutations clear it.
            master_text_emit_cache,
            // Perf-BodyStory — same lifecycle as master_text.
            body_story_emit_cache,
        })
    }

    /// Initial canonical hash captured at load. Phase 3 Item 6 —
    /// determinism tests assert that replaying the mutation log
    /// against the same `initial_state_hash` reproduces a known
    /// post-state hash.
    pub fn initial_state_hash(&self) -> [u8; 32] {
        self.initial_state_hash
    }

    /// Current canonical hash of the (possibly-mutated) scene.
    pub fn current_state_hash(&self) -> [u8; 32] {
        self.scene.canonical_hash()
    }

    /// Most-recently-assigned `applied_seq`. 0 if no mutations have
    /// landed.
    pub fn last_applied_seq(&self) -> u64 {
        self.last_applied_seq
    }

    /// Increment + return the next applied_seq. Worker calls this
    /// when assigning ordering to a mutation that successfully
    /// applied.
    pub fn bump_applied_seq(&mut self) -> u64 {
        self.last_applied_seq += 1;
        self.last_applied_seq
    }

    pub fn doc_id(&self) -> &str {
        &self.doc_id
    }

    pub fn handle(&self) -> DocumentHandle {
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        let page_sizes_pt: Vec<(f32, f32)> = self
            .built
            .pages
            .iter()
            .map(|p| (p.width_pt, p.height_pt))
            .collect();
        DocumentHandle {
            doc_id: self.doc_id.clone(),
            page_count: self.built.pages.len(),
            page_ids,
            page_sizes_pt,
            stats: DocumentStats::from(&self.built.stats),
        }
    }

    pub fn page_count(&self) -> usize {
        self.built.pages.len()
    }

    pub fn page_ids(&self) -> impl Iterator<Item = &PageId> {
        self.built.page_ids()
    }

    pub fn page(&self, id: &PageId) -> Option<&BuiltPage> {
        self.page_index.get(id).map(|&i| &self.built.pages[i])
    }

    /// Tier 4 seam: the per-page display list the worker hands to
    /// the GPU rasterizer (Vello in `apps/canvas/`, tiny-skia in
    /// headless tests).
    pub fn display_list_for_page(&self, id: &PageId) -> Option<&DisplayList> {
        self.page(id).map(|p| &p.list)
    }

    /// Apply a `Mutation` from the main thread. Phase 3 — InsertText
    /// and DeleteRange route through `crate::mutate`; other variants
    /// (style, frame, page, structural) still return `NotImplemented`
    /// until Items 5b/c + 8 land.
    ///
    /// On success: bumps `last_applied_seq`, returns the
    /// `applied_seq` + the inverse op (for the caller's undo log) +
    /// the list of affected page ids (caller invalidates the LOD
    /// cache for them). Rebuild is full + synchronous —
    /// "correctness, not pessimisation" (see plan §Item 5).
    pub fn apply_mutation(
        &mut self,
        mutation: &Mutation,
    ) -> Result<MutationOutcome, crate::channel::WorkerError> {
        // Phase B — route frame-shape mutations through the canonical
        // `idml_mutate::apply` path; only the text mutations stay on
        // the legacy `TextOp` log. The `MutationOutcome` is text-only
        // (carries an inverse `TextOp`), so frame-shape mutations
        // synthesise an empty text op into the response. Future
        // convergence folds both into one shape.
        if let Some(op) = self.try_translate_frame_mutation_to_operation(mutation) {
            let outcome = self.apply_operation(op)?;
            return Ok(MutationOutcome {
                applied_seq: outcome.applied_seq,
                page_ids: outcome.page_ids,
                // No text op to send back — frame mutations carry their
                // inverse in the AppliedOperation stored on the log.
                inverse: crate::mutate::TextOp::InsertText {
                    story_id: String::new(),
                    offset: 0,
                    text: String::new(),
                },
            });
        }
        let text_op: crate::mutate::TextOp = match mutation {
            Mutation::InsertText {
                story_id,
                offset,
                text,
            } => crate::mutate::TextOp::InsertText {
                story_id: story_id.clone(),
                offset: *offset,
                text: text.clone(),
            },
            Mutation::DeleteRange {
                story_id,
                start,
                end,
            } => crate::mutate::TextOp::DeleteRange {
                story_id: story_id.clone(),
                start: *start,
                end: *end,
                recovered: String::new(),
            },
            other => {
                return Err(crate::channel::WorkerError::NotImplemented {
                    what: format!("Mutation::{}", other.discriminant()),
                })
            }
        };
        let applied = crate::mutate::apply(&mut self.scene, &text_op).map_err(|e| {
            crate::channel::WorkerError::NotImplemented {
                what: format!("text mutation failed: {e}"),
            }
        })?;
        // Perf-BodyStory — text edits change the *content* of a story
        // but not its frame chain, so the body-story signature would
        // wrongly match and the edit would never display. Blow the
        // cache; the rebuild repopulates from the new content. We
        // also clear master_text for symmetry with apply_operation —
        // text in a master is rare but if it happens we want the
        // same invariant.
        self.master_text_emit_cache.borrow_mut().clear();
        self.body_story_emit_cache.borrow_mut().clear();
        self.rebuild_after_mutation().map_err(|e| {
            crate::channel::WorkerError::NotImplemented {
                what: format!("rebuild after mutation: {e}"),
            }
        })?;
        let applied_seq = self.bump_applied_seq();
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        // Shift the active selection through the mutation so caret
        // tracking survives the edit (AC-E-9).
        if let Some(sel) = self.current_selection.take() {
            let shifted = match &text_op {
                crate::mutate::TextOp::InsertText { story_id, offset, text } => sel
                    .shift_for_insert(story_id, *offset, text.chars().count() as u32),
                crate::mutate::TextOp::DeleteRange {
                    story_id,
                    start,
                    end,
                    ..
                } => sel.shift_for_delete(story_id, *start, *end),
            };
            self.current_selection = Some(shifted);
        }
        // Phase 3 Item 7 — push to undo log; clear redo log (any
        // pending redo is invalidated by a fresh mutation).
        self.applied_log.push(AppliedRecord {
            applied_seq,
            kind: LoggedMutation::Text {
                op: text_op,
                inverse: applied.inverse.clone(),
            },
        });
        self.redo_log.clear();
        Ok(MutationOutcome {
            applied_seq,
            page_ids,
            inverse: applied.inverse,
        })
    }

    /// Phase B — convert a channel `Mutation` into an
    /// `idml_mutate::Operation` when the mutation is a frame-shape
    /// edit. Returns `None` for text edits + any mutation kind not yet
    /// bridged (MoveFrame, InsertFrame, etc.).
    fn try_translate_frame_mutation_to_operation(
        &self,
        mutation: &Mutation,
    ) -> Option<idml_mutate::Operation> {
        use idml_mutate::{NodeId, Operation, PropertyPath, Value};
        match mutation {
            Mutation::ResizeFrame { frame_id, bounds } => {
                let node = self.resolve_frame_node_id(frame_id)?;
                Some(Operation::SetProperty {
                    node,
                    path: PropertyPath::FrameBounds,
                    // Channel carries `(top, left, bottom, right)`;
                    // `Value::Bounds` is `[top, left, bottom, right]`.
                    value: Value::Bounds([bounds.0, bounds.1, bounds.2, bounds.3]),
                })
            }
            Mutation::PathPointInsert {
                polygon_id,
                index,
                anchor,
            } => Some(Operation::SetProperty {
                node: NodeId::Polygon(polygon_id.clone()),
                path: PropertyPath::PathPointInsert,
                value: Value::PathPointInsert {
                    index: *index as usize,
                    anchor: *anchor,
                    prev_subpath_starts: None,
                },
            }),
            Mutation::PathPointRemove { polygon_id, index } => Some(Operation::SetProperty {
                node: NodeId::Polygon(polygon_id.clone()),
                path: PropertyPath::PathPointRemove,
                value: Value::PathPointRemove {
                    index: *index as usize,
                    prev_subpath_starts: None,
                },
            }),
            Mutation::PathPointCurveType {
                polygon_id,
                index,
                smooth,
            } => Some(Operation::SetProperty {
                node: NodeId::Polygon(polygon_id.clone()),
                path: PropertyPath::PathPointCurveType,
                value: Value::PathPointCurveType {
                    index: *index as usize,
                    smooth: *smooth,
                    prev: None,
                },
            }),
            Mutation::PathPointSet {
                polygon_id,
                index,
                role,
                position,
            } => Some(Operation::SetProperty {
                node: NodeId::Polygon(polygon_id.clone()),
                path: PropertyPath::FramePathPoint,
                value: Value::PathPoint {
                    address: idml_mutate::PathPointAddress {
                        index: *index as usize,
                        role: *role,
                    },
                    position: *position,
                },
            }),
            Mutation::Batch { ops } => {
                // Recursive: every child mutation must translate
                // through this same dispatch. If any child fails
                // (text mutation, unimplemented variant) the whole
                // batch falls through to the non-frame handler so
                // the worker can return a coherent error.
                let mut translated = Vec::with_capacity(ops.len());
                for child in ops {
                    let op = self.try_translate_frame_mutation_to_operation(child)?;
                    translated.push(op);
                }
                Some(Operation::Batch { ops: translated })
            }
            _ => None,
        }
    }

    /// Phase B — look up a frame's `NodeId` by its raw `Self` id.
    /// Searches text frames first, then rectangles. Phase D extends
    /// to ovals / polygons / graphic lines as `apply.rs` graduates.
    pub(crate) fn resolve_frame_node_id(
        &self,
        frame_id: &str,
    ) -> Option<idml_mutate::NodeId> {
        for parsed in &self.scene.spreads {
            let s = &parsed.spread;
            if s.text_frames.iter().any(|f| f.self_id.as_deref() == Some(frame_id)) {
                return Some(idml_mutate::NodeId::TextFrame(frame_id.to_string()));
            }
            if s.rectangles.iter().any(|f| f.self_id.as_deref() == Some(frame_id)) {
                return Some(idml_mutate::NodeId::Rectangle(frame_id.to_string()));
            }
        }
        None
    }

    /// Phase B — apply a canonical `idml_mutate::Operation` (frame
    /// mutation, fill, etc.), rebuild, push to the unified undo log.
    /// The bridge from `Mutation::MoveFrame` / `ResizeFrame` (channel
    /// envelope) lands here.
    ///
    /// Returns the dirty page set + the underlying `AppliedOperation`
    /// so the caller can also feed the LOD-cache invalidation hint.
    pub fn apply_operation(
        &mut self,
        op: idml_mutate::Operation,
    ) -> Result<FrameMutationOutcome, crate::channel::WorkerError> {
        let applied = idml_mutate::apply(&mut self.scene, &op).map_err(|e| {
            crate::channel::WorkerError::NotImplemented {
                what: format!("frame mutation failed: {e}"),
            }
        })?;
        // Perf-MasterText + Perf-BodyStory — committed mutations
        // can shift the per-page pool state (Alt-duplicate inserts
        // a new frame whose path the frame pass emits earlier,
        // path-topology mutations change anchor counts, etc.).
        // The cached relative-path-id rebase only stays valid
        // when those earlier passes produce the same pool state.
        // Clear both caches; the post-rebuild fresh capture
        // re-pins. Gesture-driven update_gesture mutates the
        // scene directly without going through apply_operation,
        // so the cache survives the whole drag.
        self.master_text_emit_cache.borrow_mut().clear();
        self.body_story_emit_cache.borrow_mut().clear();
        self.rebuild_after_mutation().map_err(|e| {
            crate::channel::WorkerError::NotImplemented {
                what: format!("rebuild after frame mutation: {e}"),
            }
        })?;
        let applied_seq = self.bump_applied_seq();
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        self.applied_log.push(AppliedRecord {
            applied_seq,
            kind: LoggedMutation::Frame(applied.clone()),
        });
        self.redo_log.clear();
        Ok(FrameMutationOutcome {
            applied_seq,
            page_ids,
            applied,
        })
    }

    /// Undo the most recent applied mutation. Phase 3 Item 7 —
    /// applies the cached inverse + rebuilds + pushes onto the redo
    /// stack. Phase B — handles both text and frame variants of the
    /// unified log.
    pub fn undo(&mut self) -> Option<UndoOutcome> {
        let rec = self.applied_log.pop()?;
        let affected_story_id = match &rec.kind {
            LoggedMutation::Text { op: _, inverse } => {
                let _ = crate::mutate::apply(&mut self.scene, inverse).ok()?;
                Some(story_id_of_text_op(inverse).to_string())
            }
            LoggedMutation::Frame(applied) => {
                let _ = idml_mutate::apply(&mut self.scene, &applied.inverse).ok()?;
                None
            }
        };
        self.rebuild_after_mutation().ok()?;
        let undone_seq = rec.applied_seq;
        let applied_seq = self.bump_applied_seq();
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        self.redo_log.push(rec);
        Some(UndoOutcome {
            undone_seq,
            applied_seq,
            page_ids,
            affected_story_id,
        })
    }

    /// Redo the most-recently-undone mutation. Phase 3 Item 7. Phase B
    /// — handles both text and frame variants.
    pub fn redo(&mut self) -> Option<UndoOutcome> {
        let rec = self.redo_log.pop()?;
        let (new_kind, affected_story_id) = match &rec.kind {
            LoggedMutation::Text { op, inverse: _ } => {
                let applied = crate::mutate::apply(&mut self.scene, op).ok()?;
                let sid = Some(story_id_of_text_op(op).to_string());
                (
                    LoggedMutation::Text {
                        op: op.clone(),
                        inverse: applied.inverse,
                    },
                    sid,
                )
            }
            LoggedMutation::Frame(prev_applied) => {
                let applied = idml_mutate::apply(&mut self.scene, &prev_applied.op).ok()?;
                (LoggedMutation::Frame(applied), None)
            }
        };
        self.rebuild_after_mutation().ok()?;
        let redone_seq = rec.applied_seq;
        let applied_seq = self.bump_applied_seq();
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        self.applied_log.push(AppliedRecord {
            applied_seq: redone_seq,
            kind: new_kind,
        });
        Some(UndoOutcome {
            undone_seq: redone_seq,
            applied_seq,
            page_ids,
            affected_story_id,
        })
    }

    /// Number of mutations in the undo log (read-only inspection
    /// for debug UI + tests).
    pub fn applied_log_len(&self) -> usize {
        self.applied_log.len()
    }
    pub fn redo_log_len(&self) -> usize {
        self.redo_log.len()
    }

    /// Read-only inspection of the most recently logged mutation.
    /// Used by Phase B integration tests to confirm the bridge routed
    /// a frame mutation onto the canonical `LoggedMutation::Frame`
    /// variant rather than the legacy text path.
    pub fn applied_log_back(&self) -> Option<&AppliedRecord> {
        self.applied_log.last()
    }

    /// Expose the inner scene for read-only inspection. Used by the
    /// inspector devtools wasm + tests. Mutating consumers should
    /// route through `apply_mutation`.
    pub fn scene(&self) -> &Document {
        &self.scene
    }

    /// Mutable accessor for the parsed scene. Phase 3 — used by the
    /// `mutate` module to apply text edits in place. Callers must
    /// follow up with `rebuild_after_mutation` so the `BuiltDocument`
    /// and page index stay in sync; bypassing the rebuild leaves the
    /// canvas painting stale pixels.
    pub fn scene_mut(&mut self) -> &mut Document {
        &mut self.scene
    }

    /// Phase 4 Step 3 — return the pages whose frame chains touch
    /// `story_id`. Used by the wasm dispatch to scope GPU scene-cache
    /// invalidation after a mutation: instead of clearing every page's
    /// cached Vello scene, only invalidate the affected ones. Returns
    /// an empty slice when the story id is unknown (e.g. the mutation
    /// failed validation, or the story has no on-page frames).
    pub fn pages_for_story(&self, story_id: &str) -> &[PageId] {
        self.story_pages
            .get(story_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Phase A — oriented geometry for the requested element ids.
    /// Skips ids that don't resolve (unknown / removed / not on a body
    /// page). Used by the overlay layer to draw element-selection
    /// chrome without re-deriving the affine math in TS.
    /// Phase H — return every leaf `ElementId` (no groups) reachable
    /// from the named group, descending through nested groups. Used
    /// by the canvas's double-click-to-enter-group gesture so the
    /// user can select a whole group as a unit and translate / scale
    /// it via the existing union handles.
    pub fn group_leaves(
        &self,
        group_self_id: &str,
    ) -> Vec<crate::element_selection::ElementId> {
        use crate::element_selection::ElementId;
        let mut out = Vec::new();
        for parsed in &self.scene.spreads {
            let spread = &parsed.spread;
            let Some(group) = spread.groups.iter().find(|g| {
                g.self_id.as_deref() == Some(group_self_id)
            }) else {
                continue;
            };
            // Recurse via a worklist instead of true recursion so we
            // don't blow the stack on pathological group nesting.
            let mut stack: Vec<&idml_parse::Group> = vec![group];
            while let Some(g) = stack.pop() {
                for member in &g.members {
                    match *member {
                        idml_parse::FrameRef::TextFrame(i) => {
                            if let Some(f) = spread.text_frames.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::TextFrame(id.to_string()));
                                }
                            }
                        }
                        idml_parse::FrameRef::Rectangle(i) => {
                            if let Some(f) = spread.rectangles.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::Rectangle(id.to_string()));
                                }
                            }
                        }
                        idml_parse::FrameRef::Oval(i) => {
                            if let Some(f) = spread.ovals.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::Oval(id.to_string()));
                                }
                            }
                        }
                        idml_parse::FrameRef::Polygon(i) => {
                            if let Some(f) = spread.polygons.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::Polygon(id.to_string()));
                                }
                            }
                        }
                        idml_parse::FrameRef::GraphicLine(i) => {
                            if let Some(f) = spread.graphic_lines.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::GraphicLine(id.to_string()));
                                }
                            }
                        }
                        idml_parse::FrameRef::Group(i) => {
                            if let Some(nested) = spread.groups.get(i) {
                                stack.push(nested);
                            }
                        }
                    }
                }
            }
            break;
        }
        out
    }

    pub fn element_geometry(
        &self,
        ids: &[crate::element_selection::ElementId],
    ) -> Vec<crate::channel::ElementGeometryItem> {
        use crate::element_selection::ElementId;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let raw = id.raw_id();
            for parsed in &self.scene().spreads {
                let spread = &parsed.spread;
                let resolved: Option<(
                    idml_parse::Bounds,
                    Option<[f32; 6]>,
                    bool,
                )> = match id {
                    ElementId::TextFrame(_) => spread
                        .text_frames
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, false)),
                    ElementId::Rectangle(_) => spread
                        .rectangles
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, f.has_image_element)),
                    ElementId::Oval(_) => spread
                        .ovals
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, false)),
                    ElementId::Polygon(_) => spread
                        .polygons
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, false)),
                    ElementId::GraphicLine(_) => spread
                        .graphic_lines
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, false)),
                    // Groups themselves are not directly addressable
                    // through geometry today — the overlay draws the
                    // leaves. A future revision can compute the group's
                    // union bbox once Phase B introduces "enter group".
                    ElementId::Group(_) => None,
                };
                let Some((bounds, item_transform, has_image)) = resolved else {
                    continue;
                };
                // Locate the page the element sits on by checking
                // which built page's spread-coord rect contains the
                // transformed centroid. Walks self.built().pages so
                // off-page items (master-spread leftovers) get dropped.
                let aabb = crate::hit::transform_bbox(bounds, item_transform);
                let cx = (aabb.left + aabb.right) * 0.5;
                let cy = (aabb.top + aabb.bottom) * 0.5;
                let page = self.built().pages.iter().find(|bp| {
                    let (ox, oy) = bp.spread_origin;
                    cx >= ox
                        && cx <= ox + bp.width_pt
                        && cy >= oy
                        && cy <= oy + bp.height_pt
                });
                if let Some(bp) = page {
                    out.push(crate::channel::ElementGeometryItem {
                        id: id.clone(),
                        page_id: bp.id.clone(),
                        bounds: [bounds.top, bounds.left, bounds.bottom, bounds.right],
                        item_transform,
                        has_image,
                    });
                }
                break;
            }
        }
        out
    }

    /// Step 5 — `RequestPathAnchors` accessor. Returns the anchor
    /// list + subpath markers for a single Polygon / Rectangle / Oval
    /// / TextFrame, alongside its page and composed transform.
    /// Rectangles / Ovals declared via `GeometricBounds` only (no
    /// `<PathGeometry>`) come back with an empty `anchors` vector —
    /// callers treat that as "nothing to draw".
    pub fn path_anchors(
        &self,
        id: &crate::element_selection::ElementId,
    ) -> Option<crate::channel::PathAnchorsResult> {
        use crate::channel::{PathAnchorTriple, PathAnchorsResult};
        use crate::element_selection::ElementId;
        use idml_parse::PathAnchor;

        let raw = id.raw_id();
        for parsed in &self.scene().spreads {
            let spread = &parsed.spread;
            let resolved: Option<(
                idml_parse::Bounds,
                Option<[f32; 6]>,
                &[PathAnchor],
                &[usize],
            )> = match id {
                ElementId::TextFrame(_) => spread
                    .text_frames
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        (
                            f.bounds,
                            f.item_transform,
                            f.anchors.as_slice(),
                            f.subpath_starts.as_slice(),
                        )
                    }),
                ElementId::Rectangle(_) => spread
                    .rectangles
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        (
                            f.bounds,
                            f.item_transform,
                            f.anchors.as_slice(),
                            f.subpath_starts.as_slice(),
                        )
                    }),
                // Ovals don't carry a parsed PathGeometry — they're
                // declared by GeometricBounds; render shows them as
                // ellipses. Drop them silently rather than synthesise
                // 4 fake anchors.
                ElementId::Oval(_) => None,
                ElementId::Polygon(_) => spread
                    .polygons
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        (
                            f.bounds,
                            f.item_transform,
                            f.anchors.as_slice(),
                            f.subpath_starts.as_slice(),
                        )
                    }),
                ElementId::GraphicLine(_) => spread
                    .graphic_lines
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        (
                            f.bounds,
                            f.item_transform,
                            f.anchors.as_slice(),
                            f.subpath_starts.as_slice(),
                        )
                    }),
                ElementId::Group(_) => None,
            };
            let Some((bounds, item_transform, anchors, subpath_starts)) = resolved else {
                continue;
            };
            // Same page-resolution as element_geometry: transform the
            // bounds centroid and look up the containing built page.
            let aabb = crate::hit::transform_bbox(bounds, item_transform);
            let cx = (aabb.left + aabb.right) * 0.5;
            let cy = (aabb.top + aabb.bottom) * 0.5;
            let page = self.built().pages.iter().find(|bp| {
                let (ox, oy) = bp.spread_origin;
                cx >= ox && cx <= ox + bp.width_pt && cy >= oy && cy <= oy + bp.height_pt
            })?;
            let anchors_out: Vec<PathAnchorTriple> = anchors
                .iter()
                .map(|a| PathAnchorTriple {
                    anchor: [a.anchor.0, a.anchor.1],
                    left: [a.left.0, a.left.1],
                    right: [a.right.0, a.right.1],
                })
                .collect();
            return Some(PathAnchorsResult {
                id: id.clone(),
                page_id: page.id.clone(),
                anchors: anchors_out,
                subpath_starts: subpath_starts.iter().map(|&n| n as u32).collect(),
                item_transform,
            });
        }
        None
    }

    /// Same as `pages_for_story` but returns page *indices* into
    /// `built().pages`. Convenient for the GPU scene cache which
    /// keys by index. Indices not currently in `page_index` (stale
    /// after a rebuild that removed pages) are skipped.
    pub fn page_indices_for_story(&self, story_id: &str) -> Vec<usize> {
        self.pages_for_story(story_id)
            .iter()
            .filter_map(|id| self.page_index.get(id).copied())
            .collect()
    }

    /// Rebuild the `BuiltDocument` from the (possibly-mutated) scene.
    /// Phase 4 Step 1 — installs the persistent `layout_cache` so
    /// paragraphs whose `(text, style, width, font)` signature didn't
    /// change short-circuit Knuth-Plass. The first build (cold cache)
    /// pays the full layout cost; subsequent mutation rebuilds only
    /// recompose the touched paragraph(s).
    pub fn rebuild_after_mutation(&mut self) -> Result<(), crate::channel::LoadError> {
        let resolver = build_font_resolver(&self.font_registry, self.font_bytes.as_deref());
        let options = PipelineOptions {
            font: self.font_bytes.as_deref(),
            assets: resolver.as_ref().map(|r| r as &dyn idml_renderer::AssetResolver),
            cmyk_icc_profile: self.icc_bytes.as_deref(),
            // Perf-S — reuse the persistent image-decode cache so
            // placed images don't re-decode on every gesture rebuild.
            image_decode_cache: Some(&self.image_decode_cache),
            // Perf-FontTable — reuse the shaping table built at
            // load. Saves the ~225ms harvest+resolve walk that
            // FontTable::build does internally.
            pre_built_font_table: Some(&self.font_table),
            // Perf-MasterText — reuse the per-page master-text
            // emit deltas captured at load. Saves the ~161ms
            // emission walk for footers/headers across body pages.
            // `apply_operation` clears this when a structural
            // mutation lands so the next rebuild repopulates.
            master_text_emit_cache: Some(&self.master_text_emit_cache),
            // Perf-BodyStory — reuse the per-story body emit
            // deltas. Signature-keyed so stories whose frame
            // chain isn't affected by the active gesture keep
            // hitting; the dragged frame's story misses and
            // re-emits. Largest single perf opportunity in
            // build_document; ~613ms ceiling on a multi-spread
            // fixture.
            body_story_emit_cache: Some(&self.body_story_emit_cache),
            ..PipelineOptions::default()
        };
        let mut cache = std::mem::take(&mut self.layout_cache);
        cache.reset_stats();
        let (build_result, cache) =
            idml_text::cache::with_layout_cache(cache, || {
                pipeline::build_document(&self.scene, &options)
            });
        self.layout_cache = cache;
        let built = build_result
            .map_err(|e| crate::channel::LoadError::Build(e.to_string()))?;
        self.page_index = built
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();
        self.story_pages = compute_story_pages(&built);
        self.built = built;
        Ok(())
    }

    /// Phase 4 instrumentation — last rebuild's layout cache stats.
    /// Hits / misses reflect the most recent `rebuild_after_mutation`
    /// (or initial `load`) so callers can verify incremental wins on
    /// a typing test.
    pub fn layout_cache_stats(&self) -> idml_text::CacheStats {
        self.layout_cache.stats()
    }

    /// Expose the inner built document for tests and the wasm
    /// renderer-on-demand path that needs to read display lists.
    pub fn built(&self) -> &BuiltDocument {
        &self.built
    }

    pub fn font_bytes(&self) -> Option<&[u8]> {
        self.font_bytes.as_deref()
    }

    pub fn icc_bytes(&self) -> Option<&[u8]> {
        self.icc_bytes.as_deref()
    }
}

/// Phase 4 Step 3 — build `story_id → Vec<PageId>` from the freshly
/// built document by walking every page's `story_layout` and grouping
/// LineLayout entries by story. Pages preserve their order; each
/// story's `Vec<PageId>` is in first-appearance order without
/// duplicates.
/// Cheap-but-coarse perf instrumentation. On wasm32 we use
/// `js_sys::Date::now()` because std::time::Instant panics. On native
/// we use Instant. Output goes to `tracing::info!` (web console on
/// wasm via `tracing-subscriber`'s wasm hook) and to a `console.log`
/// fallback so the line is also visible in DevTools.
#[cfg(target_arch = "wasm32")]
fn phase_now() -> f64 {
    js_sys::Date::now()
}
#[cfg(not(target_arch = "wasm32"))]
fn phase_now() -> std::time::Instant {
    std::time::Instant::now()
}

#[cfg(target_arch = "wasm32")]
fn phase_log(label: &str, start: f64) {
    let ms = js_sys::Date::now() - start;
    web_sys::console::log_1(&format!("[idml-canvas perf] {label}: {ms:.0} ms").into());
}
#[cfg(not(target_arch = "wasm32"))]
fn phase_log(label: &str, start: std::time::Instant) {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    tracing::info!("[idml-canvas perf] {label}: {ms:.0} ms");
}

/// Build a `BytesResolver` from a font registry. Returns `None` when
/// the registry is empty AND no default font is provided — the
/// pipeline already handles `assets: None` cleanly, so we save the
/// allocation in the common single-font dev path.
fn build_font_resolver(
    registry: &[FontEntry],
    default_font: Option<&[u8]>,
) -> Option<BytesResolver> {
    if registry.is_empty() && default_font.is_none() {
        return None;
    }
    let mut r = BytesResolver::new();
    for entry in registry {
        r.add_font(&entry.family, entry.style.as_deref(), entry.bytes.clone());
    }
    if let Some(bytes) = default_font {
        r.default_font = Some(bytes.to_vec().into());
    }
    Some(r)
}

fn compute_story_pages(built: &BuiltDocument) -> HashMap<String, Vec<PageId>> {
    let mut out: HashMap<String, Vec<PageId>> = HashMap::new();
    for page in &built.pages {
        for line in &page.story_layout {
            let entry = out.entry(line.story_id.clone()).or_default();
            if entry.last().map(|p| p != &page.id).unwrap_or(true) {
                entry.push(page.id.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimum-viable IDML the canvas can load. Hand-rolled so the
    // model test stays independent of the heavier `idml-gen` fixture
    // generator. Single Letter-sized page, no stories, no styles —
    // just the package files `Document::open` needs to parse.
    fn minimal_idml_bytes() -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions =
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);

            // mimetype must be the first file, stored uncompressed.
            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();

            // META-INF/container.xml
            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
            )
            .unwrap();

            // designmap.xml — references one spread.
            zip.start_file("designmap.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<?aid style="50" type="document" readerVersion="13.0" featureSet="513" product="13.1(255)"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
            )
            .unwrap();

            // Spreads/Spread_s1.xml — one Letter-sized page.
            zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
            )
            .unwrap();

            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn load_minimal_document_produces_one_page_with_stable_id() {
        let bytes = minimal_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default())
            .expect("minimal IDML parses + builds");
        assert_eq!(model.page_count(), 1);
        let ids: Vec<PageId> = model.page_ids().cloned().collect();
        assert_eq!(ids.len(), 1);
        // The page carried Self="p1" in the IDML — the renderer
        // surfaces that directly as PageId. If parsing falls back to
        // a synthetic id, the spec contract is broken.
        assert_eq!(ids[0].as_str(), "p1");
        // Display list seam is reachable.
        let list = model
            .display_list_for_page(&ids[0])
            .expect("page exists, display list returns Some");
        // No stories or frames yet => no commands. Just confirm we
        // returned a borrow on the in-place list, not a clone.
        assert!(list.commands.is_empty());
    }

    #[test]
    fn handle_exposes_page_dimensions() {
        let bytes = minimal_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        let handle = model.handle();
        assert_eq!(handle.page_count, 1);
        assert_eq!(handle.page_sizes_pt.len(), 1);
        let (w, h) = handle.page_sizes_pt[0];
        assert!((w - 612.0).abs() < 0.01, "expected Letter width, got {w}");
        assert!((h - 792.0).abs() < 0.01, "expected Letter height, got {h}");
    }

    #[test]
    fn unknown_page_id_returns_none() {
        let bytes = minimal_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        assert!(model.page(&PageId("does-not-exist".into())).is_none());
        assert!(model
            .display_list_for_page(&PageId("nope".into()))
            .is_none());
    }

    #[test]
    fn canonical_hash_is_stable_across_loads() {
        let bytes = minimal_idml_bytes();
        let a = CanvasModel::load("a", &bytes, CanvasOptions::default()).unwrap();
        let b = CanvasModel::load("b", &bytes, CanvasOptions::default()).unwrap();
        assert_eq!(
            a.initial_state_hash(),
            b.initial_state_hash(),
            "same bytes → same canonical hash (doc_id is not part of content)"
        );
        assert_eq!(a.initial_state_hash(), a.current_state_hash());
    }

    #[test]
    fn applied_seq_starts_at_zero_and_bumps() {
        let bytes = minimal_idml_bytes();
        let mut m = CanvasModel::load("a", &bytes, CanvasOptions::default()).unwrap();
        assert_eq!(m.last_applied_seq(), 0);
        assert_eq!(m.bump_applied_seq(), 1);
        assert_eq!(m.bump_applied_seq(), 2);
        assert_eq!(m.last_applied_seq(), 2);
    }
}
