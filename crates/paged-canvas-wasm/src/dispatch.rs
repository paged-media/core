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

//! Target-agnostic worker dispatch core.
//!
//! The wasm-bindgen surface in `lib.rs` is a thin shell over this
//! module: it owns the `js_sys::Date` clock, console logging, and the
//! GPU presenter / scene cache, and forwards every parsed message here.
//! Everything that is *pure over `paged-canvas` types* — the
//! parse → dispatch → serialise envelope handling, the per-kind arms,
//! `story_id_for_mutation`, the page-table diff, and the export-session
//! bookkeeping — lives here so it compiles on every target and is
//! exercised by `cargo test` (see `tests/dispatch.rs`).
//!
//! Behaviour is byte-identical to the pre-extraction wasm shell: the
//! wire replies these functions produce are the exact `WorkerToMain`
//! envelopes the old inline `dispatch` produced. The two seams the
//! shell still owns — timing and the GPU scene cache — are abstracted
//! out as a [`Clock`] closure and a returned [`CacheEffect`], neither
//! of which changes a single serialised field.

use paged_canvas::{
    channel::LayoutCacheStats, CanvasModel, CanvasOptions, ColorProfileEntry, FontEntry,
    MainToWorker, MainToWorkerKind, PageId, WorkerError, WorkerToMain, WorkerToMainKind,
    PROTOCOL_VERSION,
};

/// Wall-clock source for the `rebuild_ms` instrumentation. On wasm this
/// is `js_sys::Date::now`; native tests pass a deterministic stub. The
/// unit is milliseconds — same as `Date.now()` — so the arithmetic in
/// the dispatch arms is unchanged.
pub type Clock<'a> = dyn Fn() -> f64 + 'a;

/// What the GPU scene cache must do as a result of a dispatch. The
/// scene cache is `#[cfg(feature = "gpu")]`-gated and lives in the wasm
/// shell, so the cfg-agnostic dispatch can't touch it directly. Instead
/// it computes the *intent* here (using the model it already owns) and
/// returns it; the shell applies it. On a non-gpu build the shell
/// simply ignores the effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheEffect {
    /// Nothing to invalidate (read-only query, handshake, failed op).
    None,
    /// Drop every cached page scene (load, gesture, or a story with no
    /// on-page frames where per-page invalidation can't be scoped).
    ClearAll,
    /// Drop only the named page indices' cached scenes.
    InvalidatePages(Vec<usize>),
}

/// Target-agnostic worker state holder. Mirrors the non-GPU fields of
/// the wasm `CanvasWorker`; the shell composes one of these alongside
/// its presenter + scene cache.
pub struct WorkerCore {
    pub model: Option<CanvasModel>,
    /// Per-family font payloads accumulated via `RegisterFont`.
    /// Survives across `LoadDocument` calls so a Playwright suite can
    /// preload Inter / Poppins / Roboto once per worker.
    pub font_registry: Vec<FontEntry>,
    /// Named ICC profiles registered via `RegisterColorProfile`. Same
    /// lifecycle as the font registry: survives across loads.
    pub color_profiles: Vec<ColorProfileEntry>,
    /// In-flight PDF export sessions, keyed by the id handed out in
    /// `ExportPdfBegun`. Cleared on `LoadDocument` (they own a build of
    /// the PREVIOUS scene).
    pub export_sessions: std::collections::HashMap<u32, paged_canvas::export::CanvasExportSession>,
    /// Monotone id source for export sessions.
    pub next_export_session: u32,
}

impl Default for WorkerCore {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of the built page table (id + size), diffed across
/// undo/redo so page-mutation reversals carry the same page-grid
/// refresh fields as `MutationApplied`.
pub fn page_table(model: &CanvasModel) -> Vec<(PageId, (f32, f32))> {
    model
        .built()
        .pages
        .iter()
        .map(|p| (p.id.clone(), (p.width_pt, p.height_pt)))
        .collect()
}

/// Pluck the story id out of a `Mutation` so the caller can scope GPU
/// cache invalidation. Variants without a story id (frame moves, page
/// inserts) return `None`; the caller falls back to a full cache clear.
pub fn story_id_for_mutation(m: &paged_canvas::channel::Mutation) -> Option<String> {
    use paged_canvas::channel::Mutation as M;
    match m {
        M::InsertText { story_id, .. } => Some(story_id.clone()),
        M::DeleteRange { story_id, .. } => Some(story_id.clone()),
        M::ApplyStyle { story_id, .. } => Some(story_id.clone()),
        M::InsertField { story_id, .. } => Some(story_id.clone()),
        _ => None,
    }
}

/// Map an affected-story id to a `CacheEffect`. `None` story id (frame
/// move, page insert) or a story with no on-page frames (overflowed
/// chain) clears the whole cache; otherwise we invalidate just the
/// pages the story touches. Matches the gpu arms in the old shell.
fn cache_effect_for_story(model: &CanvasModel, story_id: Option<&str>) -> CacheEffect {
    match story_id {
        Some(sid) => {
            let dirty = model.page_indices_for_story(sid);
            if dirty.is_empty() {
                CacheEffect::ClearAll
            } else {
                CacheEffect::InvalidatePages(dirty)
            }
        }
        None => CacheEffect::ClearAll,
    }
}

impl WorkerCore {
    pub fn new() -> Self {
        Self {
            model: None,
            font_registry: Vec::new(),
            color_profiles: Vec::new(),
            export_sessions: std::collections::HashMap::new(),
            next_export_session: 1,
        }
    }

    /// Parse one main-thread message and dispatch it. Returns the JSON
    /// string the JS side posts back, plus the GPU cache effect the
    /// shell applies. The malformed-message seq-salvage path lives here
    /// so the wire-robustness behaviour is testable natively.
    pub fn handle_message(&mut self, input: &str, clock: &Clock<'_>) -> (String, CacheEffect) {
        let msg: MainToWorker = match serde_json::from_str(input) {
            Ok(m) => m,
            Err(e) => {
                // Salvage the seq so the caller's pending promise
                // RESOLVES (as a failure) instead of hanging — the
                // client correlates replies by seq, and a seq-less
                // warning leaves `mutate()` waiting forever.
                let seq = serde_json::from_str::<serde_json::Value>(input)
                    .ok()
                    .and_then(|v| v.get("seq").and_then(|s| s.as_u64()));
                let err = WorkerToMain {
                    seq,
                    protocol: PROTOCOL_VERSION,
                    kind: match seq {
                        Some(_) => WorkerToMainKind::MutationFailed {
                            error: WorkerError::NotImplemented {
                                what: format!("malformed message: {e}"),
                            },
                        },
                        None => WorkerToMainKind::Warning {
                            kind: "protocol".into(),
                            details: format!("malformed message: {e}"),
                        },
                    },
                };
                return (
                    serde_json::to_string(&err).unwrap_or_default(),
                    CacheEffect::None,
                );
            }
        };
        let (reply, effect) = self.dispatch(msg, clock);
        (serde_json::to_string(&reply).unwrap_or_default(), effect)
    }

    /// Dispatch a parsed message to the right model call. Returns the
    /// reply envelope and the GPU cache effect.
    pub fn dispatch(
        &mut self,
        msg: MainToWorker,
        clock: &Clock<'_>,
    ) -> (WorkerToMain, CacheEffect) {
        let seq = Some(msg.seq);
        let mut effect = CacheEffect::None;
        // Helper to wrap an early-return reply with no cache effect.
        macro_rules! reply {
            ($kind:expr) => {
                return (
                    WorkerToMain {
                        seq,
                        protocol: PROTOCOL_VERSION,
                        kind: $kind,
                    },
                    CacheEffect::None,
                )
            };
        }

        let kind = match msg.kind {
            MainToWorkerKind::Hello => WorkerToMainKind::Ready {
                protocol: PROTOCOL_VERSION,
            },
            MainToWorkerKind::LoadDocument {
                bytes,
                font,
                cmyk_icc_profile,
            } => {
                let opts = CanvasOptions {
                    fonts: font.map(|b| vec![b.into_vec()]).unwrap_or_default(),
                    font_registry: self.font_registry.clone(),
                    cmyk_icc_profile: cmyk_icc_profile.map(|b| b.into_vec()),
                    color_profiles: self.color_profiles.clone(),
                };
                let doc_id = format!("doc-{}", msg.seq);
                match CanvasModel::load(doc_id, bytes.as_slice(), opts) {
                    Ok(model) => {
                        let handle = model.handle();
                        self.model = Some(model);
                        // Export sessions hold a build of the PREVIOUS
                        // document — drop them.
                        self.export_sessions.clear();
                        // The per-page Vello scene cache was keyed to
                        // the previous model's BuiltPages.
                        effect = CacheEffect::ClearAll;
                        WorkerToMainKind::DocumentLoaded(handle)
                    }
                    Err(e) => WorkerToMainKind::LoadFailed { error: e },
                }
            }
            MainToWorkerKind::Mutate(m) => {
                if self.model.is_none() {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                }
                // Capture the affected story id BEFORE applying the
                // mutation; the post-rebuild story_pages map is the
                // right authority for which pages the story touches, so
                // we read it after.
                let affected_story = story_id_for_mutation(&m);
                let t0 = clock();
                let model = self.model.as_mut().expect("checked above");
                match model.apply_mutation(&m) {
                    Ok(outcome) => {
                        // Invalidate only the pages that contain the
                        // affected story. Other pages keep their cached
                        // Vello scenes so `presentFrame` after this
                        // mutation skips a per-page scene rebuild.
                        effect = cache_effect_for_story(model, affected_story.as_deref());
                        let mut stats: LayoutCacheStats = model.layout_cache_stats().into();
                        stats.rebuild_ms = (clock() - t0) as f32;
                        // W1.24 (audit B18) — fold the model's internal
                        // op-apply / pages / paragraphs / rebuild-count
                        // breakdown onto the wire stats (additive, rides
                        // v35). `rebuild_ms` above stays the end-to-end
                        // measure; these add the finer split.
                        stats = stats.with_rebuild_stats(&model.last_rebuild_stats());
                        // page-list mutations carry the refreshed sizes
                        // so the editor can rebuild its page grid
                        // without a document reload.
                        let page_sizes_pt = outcome.page_structure_changed.then(|| {
                            model
                                .built()
                                .pages
                                .iter()
                                .map(|p| (p.width_pt, p.height_pt))
                                .collect()
                        });
                        WorkerToMainKind::MutationApplied {
                            client_seq: msg.seq,
                            applied_seq: outcome.applied_seq,
                            page_ids: outcome.page_ids,
                            cache_stats: stats,
                            created_id: outcome.created_id,
                            page_structure_changed: outcome.page_structure_changed,
                            page_sizes_pt,
                        }
                    }
                    Err(error) => WorkerToMainKind::MutationFailed { error },
                }
            }
            MainToWorkerKind::RequestPage { page_id, lod } => {
                let Some(model) = self.model.as_ref() else {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                };
                let Some(page) = model.page(&page_id) else {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::UnknownPage { page_id },
                    });
                };
                WorkerToMainKind::DisplayListReady {
                    page_id: page.id.clone(),
                    lod,
                    commands: page.list.commands.len(),
                    layout_generation: page.layout_generation,
                    numbering_generation: page.numbering_generation,
                }
            }
            MainToWorkerKind::HitTest {
                page_id,
                doc_point,
                filter,
            } => {
                let result = self
                    .model
                    .as_ref()
                    .map(|m| m.hit_test_filtered(&page_id, doc_point, filter))
                    .unwrap_or_default();
                WorkerToMainKind::HitResult(paged_canvas::HitResult {
                    frame_id: result.frame_id,
                    story_id: result.story_id,
                    offset_within_story: result.offset_within_story,
                    frame_bounds: result
                        .frame_bounds
                        .map(|b| paged_canvas::channel::FrameBounds {
                            left: b[0],
                            top: b[1],
                            right: b[2],
                            bottom: b[3],
                        }),
                    element: result.element,
                    bounds: result.bounds,
                    item_transform: result.item_transform,
                    group_chain: result.group_chain,
                    table_context: result.table_context.map(|t| {
                        paged_canvas::channel::TableHitContext {
                            table_id: t.table_id,
                            row: t.row,
                            col: t.col,
                        }
                    }),
                })
            }
            MainToWorkerKind::RequestSnapshot {
                page_id,
                target_width_px,
                dpi,
            } => {
                let Some(model) = self.model.as_ref() else {
                    reply!(WorkerToMainKind::SnapshotFailed {
                        error: paged_canvas::SnapshotError::UnknownPage { page_id },
                    });
                };
                // An explicit `dpi` (> 0) wins over `target_width_px`:
                // the fidelity suite drives DPI directly so the PNG
                // matches `pdftoppm -r <dpi>` at the dimension boundary.
                let res = match dpi {
                    Some(d) if d > 0.0 => {
                        paged_canvas::render_snapshot_png_at_dpi(model, &page_id, d)
                    }
                    _ => paged_canvas::render_snapshot_png(model, &page_id, target_width_px),
                };
                match res {
                    Ok(snap) => WorkerToMainKind::SnapshotReady(snap),
                    Err(error) => WorkerToMainKind::SnapshotFailed { error },
                }
            }
            MainToWorkerKind::SetSelection { selection } => {
                if let Some(model) = self.model.as_mut() {
                    model.current_selection = selection;
                    WorkerToMainKind::Stats(model.handle().stats)
                } else {
                    WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    }
                }
            }
            MainToWorkerKind::RequestSelectionGeometry { selection } => {
                let Some(model) = self.model.as_ref() else {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                };
                let rects = paged_canvas::selection_geometry(model.built(), &selection);
                WorkerToMainKind::SelectionGeometry { rects }
            }
            MainToWorkerKind::RequestCaretGeometry { selection } => {
                let Some(model) = self.model.as_ref() else {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                };
                let caret = paged_canvas::caret_geometry(model.built(), &selection);
                WorkerToMainKind::CaretGeometry { caret }
            }
            MainToWorkerKind::RequestCaretNav {
                story_id,
                offset,
                direction,
                cell,
            } => {
                let Some(model) = self.model.as_ref() else {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                };
                let offset =
                    paged_canvas::caret_nav(model.built(), &story_id, &cell, offset, direction);
                WorkerToMainKind::CaretNavResult { offset }
            }
            MainToWorkerKind::RequestLineBounds {
                story_id,
                offset,
                cell,
            } => {
                let Some(model) = self.model.as_ref() else {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                };
                let bounds = paged_canvas::line_bounds(model.built(), &story_id, &cell, offset);
                WorkerToMainKind::LineBoundsResult { bounds }
            }
            MainToWorkerKind::RequestWordBounds {
                story_id,
                offset,
                cell,
            } => {
                let Some(model) = self.model.as_ref() else {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                };
                let bounds = model.word_bounds(&story_id, cell.as_ref(), offset);
                WorkerToMainKind::WordBoundsResult { bounds }
            }
            MainToWorkerKind::RequestParagraphBounds {
                story_id,
                offset,
                cell,
            } => {
                let Some(model) = self.model.as_ref() else {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                };
                let bounds = model.paragraph_bounds(&story_id, cell.as_ref(), offset);
                WorkerToMainKind::ParagraphBoundsResult { bounds }
            }
            MainToWorkerKind::Undo => {
                if self.model.is_none() {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                }
                let t0 = clock();
                let model = self.model.as_mut().expect("checked above");
                // Diff the built page table across the undo so
                // page-mutation undos refresh the editor's page grid
                // (same contract as MutationApplied).
                let pages_before = page_table(model);
                match model.undo() {
                    Some(outcome) => {
                        effect =
                            cache_effect_for_story(model, outcome.affected_story_id.as_deref());
                        let mut stats: LayoutCacheStats = model.layout_cache_stats().into();
                        stats.rebuild_ms = (clock() - t0) as f32;
                        // W1.24 (audit B18) — fold the model's internal
                        // op-apply / pages / paragraphs / rebuild-count
                        // breakdown onto the wire stats (additive, rides
                        // v35). `rebuild_ms` above stays the end-to-end
                        // measure; these add the finer split.
                        stats = stats.with_rebuild_stats(&model.last_rebuild_stats());
                        let pages_after = page_table(model);
                        let page_structure_changed = pages_before != pages_after;
                        WorkerToMainKind::UndoApplied {
                            undone_seq: outcome.undone_seq,
                            applied_seq: outcome.applied_seq,
                            page_ids: outcome.page_ids,
                            cache_stats: stats,
                            page_structure_changed,
                            page_sizes_pt: page_structure_changed
                                .then(|| pages_after.into_iter().map(|p| p.1).collect()),
                        }
                    }
                    None => WorkerToMainKind::MutationFailed {
                        error: WorkerError::NotImplemented {
                            what: "undo log empty".into(),
                        },
                    },
                }
            }
            MainToWorkerKind::RegisterFont {
                family,
                style,
                bytes,
            } => {
                self.font_registry.push(FontEntry {
                    family: family.clone(),
                    style,
                    bytes: bytes.into_vec(),
                });
                WorkerToMainKind::FontRegistered { family }
            }
            MainToWorkerKind::ClearFontRegistry => {
                self.font_registry.clear();
                WorkerToMainKind::FontRegistryCleared
            }
            MainToWorkerKind::RegisterColorProfile { name, bytes } => {
                let bytes = bytes.into_vec();
                self.color_profiles.push(ColorProfileEntry {
                    name: name.clone(),
                    bytes: bytes.clone(),
                });
                // Keep the LIVE model's registry in sync so a profile
                // registered after load is immediately resolvable by
                // SetColorSettings (the worker copy seeds future loads).
                if let Some(model) = self.model.as_mut() {
                    model.register_color_profile(name.clone(), bytes);
                }
                WorkerToMainKind::ColorProfileRegistered { name }
            }
            MainToWorkerKind::SetElementSelection { ids, mode } => {
                if let Some(model) = self.model.as_mut() {
                    model.element_selection.apply_mode(&ids, mode);
                    WorkerToMainKind::ElementSelectionApplied {
                        ids: model.element_selection.ids.clone(),
                    }
                } else {
                    WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    }
                }
            }
            MainToWorkerKind::RequestMarqueeHits { page_id, rect } => {
                let ids = self
                    .model
                    .as_ref()
                    .map(|m| m.marquee_hits(&page_id, rect))
                    .unwrap_or_default();
                WorkerToMainKind::MarqueeHits { ids }
            }
            MainToWorkerKind::RequestElementGeometry { ids } => {
                let items = self
                    .model
                    .as_ref()
                    .map(|m| m.element_geometry(&ids))
                    .unwrap_or_default();
                WorkerToMainKind::ElementGeometry { items }
            }
            MainToWorkerKind::RequestGroupLeaves { group_id } => {
                let ids = self
                    .model
                    .as_ref()
                    .map(|m| m.group_leaves(&group_id))
                    .unwrap_or_default();
                WorkerToMainKind::GroupLeaves { ids }
            }
            MainToWorkerKind::RequestPathAnchors { id } => {
                let result = self.model.as_ref().and_then(|m| m.path_anchors(&id));
                WorkerToMainKind::PathAnchors { result }
            }
            MainToWorkerKind::RequestNearestPathPoint { id, point } => {
                let result = self
                    .model
                    .as_ref()
                    .and_then(|m| m.nearest_path_point(&id, point));
                WorkerToMainKind::NearestPathPoint { result }
            }
            MainToWorkerKind::RequestLayers => {
                let items = self.model.as_ref().map(|m| m.layers()).unwrap_or_default();
                WorkerToMainKind::Layers { items }
            }
            MainToWorkerKind::RequestCollection { name } => {
                let items = self
                    .model
                    .as_ref()
                    .map(|m| m.collection(name))
                    .unwrap_or(serde_json::Value::Array(Vec::new()));
                WorkerToMainKind::CollectionReply { name, items }
            }
            MainToWorkerKind::RequestDocumentMeta => {
                let meta = self.model.as_ref().map(|m| m.document_meta()).unwrap_or(
                    paged_canvas::channel::DocumentMeta {
                        page_count: 0,
                        active_page: None,
                        units: String::new(),
                        color_mode: String::new(),
                        document_name: String::new(),
                        dirty: false,
                        default_fill_color: None,
                        default_stroke_color: None,
                        default_stroke_weight: None,
                        cmyk_profile_name: None,
                        cmyk_profile_active: false,
                        rgb_policy: None,
                        rendering_intent: None,
                        black_point_compensation: None,
                        proof_profile_name: None,
                        proof_simulate_paper_white: None,
                        use_standard_lab_for_spots: None,
                    },
                );
                WorkerToMainKind::DocumentMetaReply { meta }
            }
            MainToWorkerKind::RequestColorPreview { swatch_id } => {
                let result = self
                    .model
                    .as_ref()
                    .and_then(|m| m.color_preview(&swatch_id));
                WorkerToMainKind::ColorPreviewReply { result }
            }
            MainToWorkerKind::ExportSwatchLibrary { group_id } => match self.model.as_ref() {
                Some(m) => WorkerToMainKind::SwatchLibraryExported {
                    ase_bytes: m.export_ase(group_id.as_deref()).into(),
                },
                None => WorkerToMainKind::MutationFailed {
                    error: WorkerError::NoDocument,
                },
            },
            MainToWorkerKind::ExportPdfBegin { options } => match self.model.as_ref() {
                Some(m) => match paged_canvas::export::CanvasExportSession::begin(m, &options) {
                    Ok((session, page_count)) => {
                        let id = self.next_export_session;
                        self.next_export_session += 1;
                        self.export_sessions.insert(id, session);
                        WorkerToMainKind::ExportPdfBegun {
                            session: id,
                            page_count: page_count as u32,
                        }
                    }
                    Err(error) => WorkerToMainKind::ExportPdfFailed { error },
                },
                None => WorkerToMainKind::ExportPdfFailed {
                    error: "no document loaded".into(),
                },
            },
            MainToWorkerKind::ExportPdfPage { session } => {
                match self.export_sessions.get_mut(&session) {
                    Some(s) => match s.export_next_page() {
                        Ok((done, total)) => WorkerToMainKind::ExportPdfProgress {
                            session,
                            done: done as u32,
                            total: total as u32,
                        },
                        Err(error) => {
                            // A failed page poisons the writer state —
                            // drop the session.
                            self.export_sessions.remove(&session);
                            WorkerToMainKind::ExportPdfFailed { error }
                        }
                    },
                    None => WorkerToMainKind::ExportPdfFailed {
                        error: format!("unknown export session: {session}"),
                    },
                }
            }
            MainToWorkerKind::ExportPdfFinish { session } => {
                match self.export_sessions.remove(&session) {
                    Some(s) => match s.finish() {
                        Ok(done) => WorkerToMainKind::PdfExported {
                            pdf_bytes: done.pdf_bytes.into(),
                            diagnostics: done.diagnostics,
                            findings: done.findings,
                        },
                        Err(error) => WorkerToMainKind::ExportPdfFailed { error },
                    },
                    None => WorkerToMainKind::ExportPdfFailed {
                        error: format!("unknown export session: {session}"),
                    },
                }
            }
            MainToWorkerKind::ExportPdfCancel { session } => {
                // Removal IS the cancellation — the writer state and the
                // one-shot build drop here. Unknown ids reply success
                // (cancel must be idempotent).
                self.export_sessions.remove(&session);
                WorkerToMainKind::ExportPdfCancelled { session }
            }
            MainToWorkerKind::ExportIdml {} => match self.model.as_ref() {
                // W3.B2 — one-shot IDML save-back. The carry-through
                // writer is cheap (patch the model-owned entries, copy
                // the rest verbatim) so there's no session loop like the
                // PDF export.
                Some(m) => match m.export_idml() {
                    Ok(bytes) => WorkerToMainKind::IdmlExported {
                        idml_bytes: bytes.into(),
                    },
                    Err(e) => WorkerToMainKind::ExportIdmlFailed {
                        error: e.to_string(),
                    },
                },
                None => WorkerToMainKind::ExportIdmlFailed {
                    error: "no document loaded".into(),
                },
            },
            MainToWorkerKind::RequestGradientDetail { gradient_id } => {
                let result = self
                    .model
                    .as_ref()
                    .and_then(|m| m.gradient_detail(&gradient_id));
                WorkerToMainKind::GradientDetailReply { result }
            }
            MainToWorkerKind::RequestColorCompute {
                space,
                value,
                tint,
                model,
                alternate_space,
                alternate_value,
            } => match self.model.as_ref() {
                Some(m) => {
                    let (rgb_hex, cmyk, out_of_gamut) = m.color_compute(
                        &space,
                        &value,
                        tint,
                        model.as_deref(),
                        alternate_space.as_deref(),
                        alternate_value.as_deref(),
                    );
                    WorkerToMainKind::ColorComputeReply {
                        rgb_hex,
                        cmyk,
                        out_of_gamut,
                    }
                }
                None => WorkerToMainKind::MutationFailed {
                    error: WorkerError::NoDocument,
                },
            },
            MainToWorkerKind::RequestElementProperties { id } => {
                let result = self.model.as_ref().and_then(|m| m.element_properties(&id));
                WorkerToMainKind::ElementProperties { result }
            }
            MainToWorkerKind::RequestSceneTree => {
                let roots = self
                    .model
                    .as_ref()
                    .map(|m| m.scene_tree())
                    .unwrap_or_default();
                WorkerToMainKind::SceneTree { roots }
            }
            MainToWorkerKind::ExecuteScript { source } => {
                let Some(model) = self.model.as_mut() else {
                    reply!(WorkerToMainKind::ScriptResult {
                        output: Vec::new(),
                        error: Some("no document loaded".to_string()),
                    });
                };
                let result = paged_script::execute_script(model, &source);
                WorkerToMainKind::ScriptResult {
                    output: result.output,
                    error: result.error,
                }
            }
            MainToWorkerKind::BeginGesture {
                nodes,
                gesture,
                anchor,
                camera_scale,
            } => {
                let Some(model) = self.model.as_mut() else {
                    reply!(WorkerToMainKind::GestureFailed {
                        error: paged_canvas::channel::GestureFailure::NoDocument,
                    });
                };
                match model.begin_gesture_with_scale(nodes, gesture, anchor, camera_scale) {
                    Ok(handle) => WorkerToMainKind::GestureBegun { handle },
                    Err(e) => WorkerToMainKind::GestureFailed { error: e.into() },
                }
            }
            MainToWorkerKind::UpdateGesture {
                handle,
                delta,
                modifiers,
            } => {
                let Some(model) = self.model.as_mut() else {
                    reply!(WorkerToMainKind::GestureFailed {
                        error: paged_canvas::channel::GestureFailure::NoDocument,
                    });
                };
                match model.update_gesture(handle, delta, modifiers) {
                    Ok(result) => {
                        // Phase B v1 — clear the GPU scene cache
                        // wholesale on every update. Per-page
                        // invalidation is a Phase B v2 perf knob once the
                        // rebuild path stops dominating.
                        effect = CacheEffect::ClearAll;
                        WorkerToMainKind::GestureUpdated {
                            handle,
                            page_ids: result.page_ids,
                            snap_lines: result.snap_lines,
                        }
                    }
                    Err(e) => WorkerToMainKind::GestureFailed { error: e.into() },
                }
            }
            MainToWorkerKind::CommitGesture { handle } => {
                if self.model.is_none() {
                    reply!(WorkerToMainKind::GestureFailed {
                        error: paged_canvas::channel::GestureFailure::NoDocument,
                    });
                }
                let t0 = clock();
                let model = self.model.as_mut().expect("checked above");
                match model.commit_gesture(handle) {
                    Ok(outcome) => {
                        effect = CacheEffect::ClearAll;
                        let mut stats: LayoutCacheStats = model.layout_cache_stats().into();
                        stats.rebuild_ms = (clock() - t0) as f32;
                        // W1.24 (audit B18) — fold the model's internal
                        // op-apply / pages / paragraphs / rebuild-count
                        // breakdown onto the wire stats (additive, rides
                        // v35). `rebuild_ms` above stays the end-to-end
                        // measure; these add the finer split.
                        stats = stats.with_rebuild_stats(&model.last_rebuild_stats());
                        WorkerToMainKind::GestureCommitted {
                            handle,
                            applied_seq: outcome.applied_seq,
                            page_ids: outcome.page_ids,
                            cache_stats: stats,
                        }
                    }
                    Err(e) => WorkerToMainKind::GestureFailed { error: e.into() },
                }
            }
            MainToWorkerKind::CancelGesture { handle } => {
                let Some(model) = self.model.as_mut() else {
                    reply!(WorkerToMainKind::GestureFailed {
                        error: paged_canvas::channel::GestureFailure::NoDocument,
                    });
                };
                match model.cancel_gesture(handle) {
                    Ok(page_ids) => {
                        effect = CacheEffect::ClearAll;
                        WorkerToMainKind::GestureCancelled { handle, page_ids }
                    }
                    Err(e) => WorkerToMainKind::GestureFailed { error: e.into() },
                }
            }
            MainToWorkerKind::Redo => {
                if self.model.is_none() {
                    reply!(WorkerToMainKind::MutationFailed {
                        error: WorkerError::NoDocument,
                    });
                }
                let t0 = clock();
                let model = self.model.as_mut().expect("checked above");
                let pages_before = page_table(model);
                match model.redo() {
                    Some(outcome) => {
                        effect =
                            cache_effect_for_story(model, outcome.affected_story_id.as_deref());
                        let mut stats: LayoutCacheStats = model.layout_cache_stats().into();
                        stats.rebuild_ms = (clock() - t0) as f32;
                        // W1.24 (audit B18) — fold the model's internal
                        // op-apply / pages / paragraphs / rebuild-count
                        // breakdown onto the wire stats (additive, rides
                        // v35). `rebuild_ms` above stays the end-to-end
                        // measure; these add the finer split.
                        stats = stats.with_rebuild_stats(&model.last_rebuild_stats());
                        let pages_after = page_table(model);
                        let page_structure_changed = pages_before != pages_after;
                        WorkerToMainKind::RedoApplied {
                            redone_seq: outcome.undone_seq,
                            applied_seq: outcome.applied_seq,
                            page_ids: outcome.page_ids,
                            cache_stats: stats,
                            page_structure_changed,
                            page_sizes_pt: page_structure_changed
                                .then(|| pages_after.into_iter().map(|p| p.1).collect()),
                        }
                    }
                    None => WorkerToMainKind::MutationFailed {
                        error: WorkerError::NotImplemented {
                            what: "redo log empty".into(),
                        },
                    },
                }
            }
        };
        (
            WorkerToMain {
                seq,
                protocol: PROTOCOL_VERSION,
                kind,
            },
            effect,
        )
    }
}
