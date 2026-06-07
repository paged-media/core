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

//! End-to-end pipeline: `Document` → `DisplayList` → `RgbaImage`.
//!
//! Everything the inspect binary does minus the pretty-printing. This
//! is the thin, reusable top-level Rust API that hosts (WASM binding,
//! native tools, the fidelity harness) call into.
//!
//! The pipeline consumes `&paged_scene::Document` — parsing and resource
//! walking live in that crate so we stay focused on layout + emission.

use std::collections::HashMap;

use bytes::Bytes;
use paged_compose::{
    emit_ellipse, emit_glyph_slice, emit_glyph_slice_stroke, emit_line, emit_paragraph, emit_rect,
    emit_stroke_rect, emit_stroke_rect_transformed, Color, DisplayList, DropShadow, GlyphCacheKey,
    GlyphOutliner, Paint, PathData, PathSegment, Rect, Stroke, Transform, TtfOutliner,
};
use paged_parse::{
    graphic, Graphic, GraphicLine, Oval, PathAnchor, Polygon, Rectangle, TextFrame, TextPath,
};
use paged_scene::Document;

use crate::diagnostics::{Diagnostic, DiagnosticCode, RenderDiagnostics};
use crate::module::geometry::rewrite_tail_for_overprint;
use crate::module::{Geometry, ResolvedFrame};
use crate::AssetResolver;

mod anchored;
mod color_paint;
mod image_convert;
mod image_decode;
mod images;
mod links;
mod numbering;
mod shapes;
mod stroke_geom;
mod tables;
mod text_path;

pub use anchored::AnchoredImageEmit;
use anchored::{emit_anchored_frames_for_paragraph, emit_anchored_rect_image};
// Doc-link-only imports: referenced from `///` comments in this file.
#[cfg(doc)]
use anchored::{emit_anchored_textframe_story, MAX_ANCHORED_STORY_RECURSION};
#[cfg(doc)]
use tables::emit_cell_paragraph;

use images::{emit_oval_image, emit_polygon_image, emit_rectangle_image};
use tables::emit_table_into_chain;

pub(crate) use shapes::{
    blend_mode_from_idml, corner_rect_path, inset_rect, per_corner_kinds, per_corner_radii,
    stroke_alignment_offset,
};
use shapes::{
    emit_line_into, emit_oval_into, emit_oval_missing_image_placeholder,
    emit_polygon_missing_image_placeholder, emit_rectangle_into,
    emit_rectangle_missing_image_placeholder, unit_ellipse_path,
};
#[cfg(test)]
use shapes::{PLACEHOLDER_FILL_RGB, PLACEHOLDER_X_RGB, PLACEHOLDER_X_STROKE_PT};

pub(crate) use color_paint::{apply_fill_tint, stroke_for};
use color_paint::{
    build_footnote_paint_picker, build_run_paint_picker_resolved, build_run_stroke_picker,
};
pub use color_paint::{
    build_run_paint_picker, build_run_paint_picker_with_cmyk, color_id_to_paint,
    color_id_to_paint_with_list, color_id_to_paint_with_list_dir, gradient_midpoint_paint,
    resolve_fill, resolve_rect_fill, resolve_rect_stroke, resolve_stroke, RunPaintPicker,
    RunStrokePicker,
};
#[cfg(test)]
use color_paint::{color_lerp, linear_gradient_endpoints, midpoint_blend};
use image_convert::{cmyk32_to_rgba, l16_to_rgba, l8_to_rgba, rgb24_to_rgba};
use image_decode::decode_image_bytes;
#[cfg(test)]
use image_decode::decode_image_bytes_with_target_max;
use numbering::{bullet_marker_character_style, list_prefix};
#[cfg(test)]
use numbering::{format_number, substitute_numbering_expression};
#[cfg(test)]
use text_path::polygon_path_from_anchors;
pub(crate) use text_path::polygon_path_from_anchors_with_open;
use text_path::{emit_polygon_into, emit_text_path_into};

/// Per-family override of the metrics the renderer uses for
/// baseline-placement math. Glyph outlines still come from whichever
/// font the asset resolver returned for that family; only the values
/// `first_baseline_for_frame` reads (ascender, optional cap-height /
/// x-height) are sourced here.
///
/// Use case: an IDML names "Arial" but you've substituted Roboto via
/// `--font-family Arial=Roboto-Regular.ttf`. Roboto's ascender (~0.928)
/// differs from Arial's (~0.905) and the per-frame baseline drift
/// dominates the per-pixel ΔE against an Arial-rendered reference PDF.
/// Registering Arial's metrics here pins the baseline math without
/// touching glyph rendering.
///
/// Values are em-fractions (parsed-from-font fields are scaled by
/// `units_per_em`).
#[derive(Clone, Copy, Debug, Default)]
pub struct FontMetricsOverride {
    pub ascender: f32,
    pub cap_height: Option<f32>,
    pub x_height: Option<f32>,
}

/// Knobs the caller tunes when driving the full pipeline.
#[derive(Clone)]
pub struct PipelineOptions<'a> {
    /// Default font bytes. Used as a fallback for any paragraph
    /// whose `AppliedFont` doesn't resolve via `assets`. `None` plus
    /// no resolver hit → text is skipped.
    pub font: Option<&'a [u8]>,
    /// Asset resolver consulted per (family, style). When set, the
    /// pipeline pre-resolves every distinct font referenced in the
    /// document; runs without a hit fall back to `font`.
    pub assets: Option<&'a dyn AssetResolver>,
    /// Fallback point size for runs with no `PointSize` attribute.
    pub default_point_size: f32,
    /// Fallback column width in pt when a paragraph has no frame
    /// (extremely rare).
    pub fallback_column_width_pt: Option<f32>,
    /// Fill paint for frames that have no resolvable FillColor.
    pub fallback_frame_fill: Paint,
    /// Fill paint for runs that have no resolvable FillColor.
    pub fallback_text_paint: Paint,
    /// CMYK ICC profile bytes. When present (and on a target with
    /// lcms2 available — i.e. not wasm32), CMYK swatches are routed
    /// through ICC instead of the naive math in `paged-parse::graphic`.
    /// None → naive conversion (existing behaviour).
    pub cmyk_icc_profile: Option<&'a [u8]>,
    /// Concept 2 — rendering intent for the CMYK display transform.
    /// The default reproduces the previously hardcoded behaviour
    /// (Relative Colorimetric on native; qcms maps it per target).
    pub cmyk_intent: paged_color::Intent,
    /// Concept 2 — black-point compensation for the CMYK display
    /// transform. Default `true` (the previously hardcoded value).
    pub cmyk_bpc: bool,
    /// Concept 3 (PDF export) — record the glyph-run side-channel
    /// on every page's display list so the exporter can emit real
    /// text. Default `false`: the live canvas build never pays for
    /// it and the command stream stays byte-identical.
    pub collect_glyph_runs: bool,
    /// W1.4 (PDF export) — record the link-region side-channel on
    /// every page's display list so the exporter can emit `/Annots`
    /// Link annotations for hyperlinks / cross-references. Default
    /// `false`: the live canvas build never pays for it.
    pub collect_link_regions: bool,
    /// Synthetic drop shadow applied to every TextFrame and
    /// Rectangle. Useful for tooling demos and as a stopgap until
    /// `<TransparencySetting>` parsing lands and per-frame effects
    /// flow from the IDML itself.
    pub frame_drop_shadow: Option<DropShadow>,
    /// Per-family metric overrides keyed by IDML `AppliedFont` name.
    /// When `first_baseline_for_frame` resolves a run's family that
    /// matches an entry here, the override wins over the metrics
    /// parsed from the substitute font's bytes. Empty by default.
    pub font_metrics_overrides: &'a [(String, FontMetricsOverride)],
    /// When `true` (default), frames that nest an `<Image>` (or
    /// `<EPSImage>` / `<PDF>` / `<ImportedPage>`) whose link cannot be
    /// resolved are stamped with InDesign's missing-image placeholder
    /// — a 50% grey fill clipped to the host path plus two 1.5pt black
    /// diagonal stroke segments. Templates routinely ship with broken
    /// links so every "Your Image Here" slot ends up looking like the
    /// IDML's reference PDF instead of falling back to the frame's raw
    /// fill.
    pub missing_image_placeholder: bool,
    /// Track 2: when true, the renderer records one [`BreakRecord`] per
    /// laid-out line into [`BuiltDocument::breaks`]. Cheap (Vec push
    /// per line) and gated so production renders pay zero cost.
    pub collect_breaks: bool,
    /// Cycle 6 Track 1: when `collect_breaks` is on, restrict
    /// collection to lines whose `story_id` matches. `None` ⇒ collect
    /// from every story (cycle-5 behaviour). Used by the A/B harness
    /// to isolate a single body story from pack-wide structural
    /// divergence noise.
    pub break_story_filter: Option<String>,
    /// Cycle 6 Track 1: half-open `[start, end)` page-index filter
    /// for break collection. `None` ⇒ no filter. ANDs with
    /// `break_story_filter` when both are set.
    pub break_page_range: Option<std::ops::Range<u32>>,
    /// Perf-S — persistent `URI → DecodedImage` cache. When `Some`,
    /// `build_document` reuses entries instead of re-decoding every
    /// image on every call. The same `RefCell` shared across multiple
    /// `build_document` calls amortises decode cost — critical for
    /// gesture rebuilds on image-heavy fixtures, where the per-call
    /// scratch cache otherwise dominates the rebuild time (5e perf
    /// finding: ~1s per `update_gesture` on `brand-guidelines`, 99%
    /// of which is image decoding). `None` falls back to the v0
    /// behaviour: a fresh local cache per call.
    pub image_decode_cache:
        Option<&'a std::cell::RefCell<HashMap<String, paged_compose::DecodedImage>>>,
    /// Perf-FontTable — pre-built shaping table. When `Some`,
    /// `build_document` skips its internal `FontTable::build` call
    /// (which walks every paragraph + resolver-fetches every font
    /// and costs ~225ms on a multi-spread fixture). Callers that
    /// own the document for multiple builds (e.g. `CanvasModel`)
    /// should pre-build once and pass `&self.font_table` here on
    /// every subsequent rebuild. `None` ⇒ build fresh per call.
    pub pre_built_font_table: Option<&'a FontTable>,
    /// Perf-MasterText — per-(master_frame_self_id, page_idx) cache
    /// of the DisplayList delta produced by the master-text pass.
    /// Master stories (page-number footers, running headers) are
    /// stable across gesture-driven rebuilds — they depend only on
    /// the master frame's content + page index + page label.
    /// The cache stores path entries + commands with path_ids
    /// RELATIVE to the path-buffer state at emit-start, so on hit
    /// we can splice the delta into a different per-build state
    /// without renumbering pre-existing references. Entries that
    /// touched the gradient or image pools during emission are
    /// marked uncacheable and skip the cache — covers rare master
    /// frames with gradient fills or embedded images. ~161ms
    /// savings per rebuild on a multi-spread fixture.
    pub master_text_emit_cache:
        Option<&'a std::cell::RefCell<HashMap<(String, usize), MasterTextEmitDelta>>>,
    /// Perf-BodyStory — per-(story_id, signature) cache of the
    /// multi-page DisplayList delta produced by the body-story
    /// pass. The signature hashes the chain's frames (bounds +
    /// transforms) + the wrap_rects on the chain's host pages,
    /// so a story whose chain doesn't include the dragged frame
    /// AND whose chain pages don't see a wrap-rect change keeps
    /// hitting the cache during gestures. The body-story pass is
    /// the largest single cost in `build_document` on a multi-
    /// spread fixture (~613ms); most stories are unaffected by
    /// any given gesture, so the win ratio is high.
    pub body_story_emit_cache:
        Option<&'a std::cell::RefCell<HashMap<(String, u64), BodyStoryEmissionDelta>>>,
}

/// Perf-BodyStory — captured multi-page emission delta from one
/// body-story pass iteration. The per-page entries reuse
/// `MasterTextEmitDelta`'s relative-path-id rebase trick AND
/// also carry the `story_layout` + `footnotes` entries the emit
/// would have appended so caret/hit-test queries and per-page
/// footnote pools land in the same shape as a non-cached build.
/// The anchored + breaks side-channels survive verbatim because
/// their contents are page-indexed and pool-independent.
#[derive(Debug, Clone)]
pub struct BodyStoryEmissionDelta {
    pub per_page: Vec<(usize, BodyStoryPageDelta)>,
    pub anchored: Vec<AnchoredImageEmit>,
    pub breaks: Vec<BreakRecord>,
}

/// Perf-BodyStory — single page's worth of captured emission state.
/// Splice on hit: push `paths` through `PathBuffer::push_anon`
/// (rebasing each command's path-ids by the current pool size),
/// extend `pages[i].story_layout` with `story_layout`, and extend
/// `pages[i].footnotes` with `footnotes`.
#[derive(Debug, Clone)]
pub struct BodyStoryPageDelta {
    pub paths: Vec<paged_compose::PathData>,
    pub commands: Vec<paged_compose::DisplayCommand>,
    pub story_layout: Vec<LineLayout>,
    pub footnotes: Vec<EmittedFootnote>,
}

/// Perf-MasterText — captured DisplayList delta for one
/// `(master_frame_self_id, page_idx)` emission, used by
/// `PipelineOptions::master_text_emit_cache`. The `paths` and
/// `commands` vectors are produced by snapshotting the page's
/// PathBuffer + commands vec around the emit, then extracting
/// the new entries. Path_ids in `commands` are RELATIVE — i.e.
/// `0` means the first new path; replay adds the current
/// `page.list.paths.len()` to remap.
#[derive(Debug, Clone)]
pub struct MasterTextEmitDelta {
    /// Raw path geometries appended by emit (no intern dedup);
    /// replay pushes them via `PathBuffer::push_anon` so the IDs
    /// stay sequential and the relative offsets in `commands`
    /// resolve correctly.
    pub paths: Vec<paged_compose::PathData>,
    /// Commands appended by emit, with path-id fields rebased to
    /// `0..paths.len()`. Replay adds the current path-buffer
    /// size to each id before pushing.
    pub commands: Vec<paged_compose::DisplayCommand>,
}

impl std::fmt::Debug for PipelineOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineOptions")
            .field("font", &self.font.map(|b| b.len()))
            .field("assets", &self.assets.is_some())
            .field("default_point_size", &self.default_point_size)
            .field("fallback_column_width_pt", &self.fallback_column_width_pt)
            .field("cmyk_icc_profile", &self.cmyk_icc_profile.map(|b| b.len()))
            .field("frame_drop_shadow", &self.frame_drop_shadow)
            .finish_non_exhaustive()
    }
}

impl Default for PipelineOptions<'_> {
    fn default() -> Self {
        Self {
            font: None,
            assets: None,
            default_point_size: 12.0,
            fallback_column_width_pt: None,
            fallback_frame_fill: Paint::Solid(Color::rgba(0.92, 0.92, 0.92, 1.0)),
            fallback_text_paint: Paint::Solid(Color::BLACK),
            cmyk_icc_profile: None,
            cmyk_intent: paged_color::Intent::RelativeColorimetric,
            cmyk_bpc: true,
            collect_glyph_runs: false,
            collect_link_regions: false,
            frame_drop_shadow: None,
            font_metrics_overrides: &[],
            missing_image_placeholder: true,
            collect_breaks: false,
            break_story_filter: None,
            break_page_range: None,
            image_decode_cache: None,
            pre_built_font_table: None,
            master_text_emit_cache: None,
            body_story_emit_cache: None,
        }
    }
}

/// Stable page identity, independent of position in the page vector.
///
/// Derived from the IDML `<Page Self="...">` attribute where present;
/// synthesised as `"page-<spread_idx>-<local_idx>"` when missing
/// (older / synthetic fixtures without `Self`). The canvas keys
/// display-list caches and LOD tiles by `PageId`, so the value must
/// stay stable across re-layouts — only document-structural edits
/// (insert/delete page) should ever change the set of `PageId`s.
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

/// Page bounding box and display-list built from a `Document`.
#[derive(Debug)]
pub struct BuiltPage {
    /// Stable identity for cache keys (LOD tiers, salsa queries, the
    /// canvas worker's `display_list_for_page` lookups). Today every
    /// page in `BuiltDocument::pages` has a unique `id`; we don't
    /// rely on positional indexing in callers.
    pub id: PageId,
    pub width_pt: f32,
    pub height_pt: f32,
    /// Page origin in spread coordinates (top-left). The display list's
    /// commands are page-relative — the rasterizer treats (0, 0) as
    /// the page's top-left corner regardless of where the page sits in
    /// its parent spread. Stays in spread-INNER coords (pre-spread-
    /// ItemTransform); the spread's rotation/scale rides
    /// `spread_transform` and is applied *about* this origin.
    pub spread_origin: (f32, f32),
    /// W1.9 — the LINEAR part (rotation / scale, translation dropped) of
    /// the parent `<Spread>` / `<MasterSpread>`'s own `ItemTransform`.
    /// InDesign restricts a spread transform to translation +
    /// 0/90/180/270 rotation; the translation cancels against
    /// `spread_origin` (both spread-inner) so only the linear part needs
    /// to ride here. It is applied about the page origin:
    /// `frame_outer_transform` builds
    /// `spread_transform ∘ translate(-spread_origin) ∘ item_transform`,
    /// rotating/scaling the whole page's content in place. The canvas
    /// hit-tester reads the SAME field and inverts it, so painter and
    /// hit-test agree by construction (cycle-8: a transform the painter
    /// applies but the hit-tester ignores breaks selection silently).
    /// `IDENTITY` (the common case) makes the composition byte-identical
    /// to the pre-W1.9 `translate(-spread_origin) ∘ item_transform`.
    pub spread_transform: Transform,
    pub list: DisplayList,
    /// Bumped whenever any frame on this page is re-laid-out. The
    /// canvas combines `(id, layout_generation, numbering_generation)`
    /// as the cache key for display-list-derived artifacts (snapshot,
    /// mid-res, live tiles). Today the whole pipeline is one-shot, so
    /// every build starts every page at 0; Tier 3 (Phase 2) and
    /// incremental Tier 2 (Phase 3) populate this for real.
    pub layout_generation: u64,
    /// Bumped whenever any resolved field on this page changes value.
    /// Reserved for Phase 2 resolution work — today always 0.
    pub numbering_generation: u64,
    /// Aggregated counts, useful for logging / CI reporting.
    pub stats: PipelineStats,
    /// Phase 3 (correctness layer) — per-line layout index for every
    /// laid-out paragraph whose visible glyphs land on this page.
    /// Carries the per-cluster page-local positions the canvas needs
    /// to (a) hit-test by character offset, (b) place the caret, and
    /// (c) compute selection geometry (rect-per-line). Empty when no
    /// text emitted on the page (e.g. graphic-only pages). Captured
    /// unconditionally at emit time; cost is O(visible glyphs).
    pub story_layout: Vec<LineLayout>,
    /// Phase 5 — footnotes anchored on paragraphs that landed on this
    /// page. Each entry preserves the host paragraph's identity and
    /// the footnote body. The renderer draws these bodies bottom-up at
    /// the host frame's content area in a post-pass
    /// ([`emit_footnote_pools`]); the captures stay here so downstream
    /// tools can observe footnote distribution. Remaining work:
    /// reserving the pool's space *before* the main text fills (so
    /// bodies don't overlap the last lines) and per-run styling — see
    /// the pool emitter's docs.
    pub footnotes: Vec<EmittedFootnote>,
    /// Lossy-render signals collected while emitting this page (missing
    /// image links, decode failures). Aggregated into
    /// [`BuiltDocument::diagnostics`] with `page_index` backfilled. Empty
    /// for a clean page. Story-level signals (overset) ride the emit
    /// channel instead, so this stays untouched by the body-story cache.
    pub diagnostics: Vec<Diagnostic>,
    /// W3.A1 — per-cell page-local rects for every table cell that
    /// landed on this page. Captured at table-emit time (the only place
    /// cell geometry exists; the display list flattens it into fills /
    /// strokes / glyphs). The canvas's hit-tester reads this to resolve
    /// a doc-point inside a table frame to its `(tableId, row, col)`
    /// context. Empty for pages with no tables — mirrors how
    /// `story_layout` retains text-line hit data. Header / footer
    /// *replays* on continuation frames push their own rects (each
    /// carries the template row index), so a click on a replayed header
    /// resolves to the source row.
    pub cell_rects: Vec<CellRect>,
}

/// W3.A1 — one table cell's page-local rect plus its addressing keys.
/// Page-local pt, `(0, 0)` = page top-left (same frame as
/// [`LineLayout`]). Captured once per emitted physical row × column.
#[derive(Debug, Clone)]
pub struct CellRect {
    /// `<Story Self="…">` owning the table.
    pub story_id: String,
    /// `<Table Self="…">` id. Empty string when the table carried no
    /// `Self` (synthetic / malformed) — such tables aren't addressable.
    pub table_id: String,
    /// Zero-based row index in the table's `rows` (the *template* row;
    /// header / footer replays report their source row).
    pub row: u32,
    /// Zero-based column index.
    pub col: u32,
    /// Page-local rect `[x, y, w, h]` in pt (cell's outer box,
    /// inset-inclusive).
    pub rect: [f32; 4],
}

/// Phase 5 — a footnote captured at emit time on the page where its
/// host paragraph landed. The `number` is a per-page running counter
/// (1-based) and is the value the host paragraph's anchor character
/// should display once anchor-substitution lands. Renderer doesn't
/// yet write footnote bodies into the page display list; this
/// struct's `paragraphs` are the source body verbatim, untouched.
#[derive(Debug, Clone)]
pub struct EmittedFootnote {
    /// Per-page running number, 1-based. Resets at every page.
    pub number: u32,
    /// `<Story Self="...">` id of the host story (the one carrying
    /// the anchor character, not the footnote story itself).
    pub host_story_id: String,
    /// Index of the host paragraph within the host story. Lets
    /// downstream tools cross-reference back to the source AST.
    pub host_paragraph_idx: u32,
    /// `<Footnote Self="...">` id, when present.
    pub footnote_self_id: Option<String>,
    /// Footnote body paragraphs, exactly as parsed.
    pub paragraphs: Vec<paged_parse::Paragraph>,
    /// Page-local pt rect of the host frame's content area at
    /// capture time. The footnote-pool emit pass uses this to know
    /// where to lay out the body text at the bottom of the frame.
    pub host_frame_rect_pt: Rect,
}

/// Disjoint paragraph-address qualifier for a laid-out line that
/// lives inside a table cell rather than in the story's main
/// paragraph flow (W1.13).
///
/// ## Why this exists — the paragraph_idx collision
///
/// Table-cell text is stored OUT OF BAND: a cell's paragraphs hang off
/// `Table.cells[].paragraphs`, not off `Story.paragraphs`. The host
/// paragraph that physically carries the `<Table>` has its own
/// `paragraph_idx` in the body flow, and every cell paragraph the table
/// pass emits used to be stamped with that SAME host `paragraph_idx`
/// (see `tables.rs::emit_cell_paragraph`). Result: body paragraph N and
/// a cell paragraph both reported `paragraph_idx == N`, so
/// `story_layout` / `paragraph_byte_offset` / hit-test could not tell
/// a body caret from a cell caret — the long-standing W3.A1 deferral.
///
/// The fix is a disjoint address axis. `LineLayout.cell` is `None` for
/// body lines and `Some(CellAddr)` for cell lines; `paragraph_idx` is
/// then re-based to be local to its own stream (the cell's own
/// paragraph list, or the body flow). Two lines are the "same
/// paragraph" only when BOTH `(cell, paragraph_idx)` match. This
/// mirrors the wire-side `TextCellAddr` qualifier on `ContentSelection`
/// / `TextOp` (`paged-canvas`), which selects `cell.paragraphs` instead
/// of `story.paragraphs` for the identical `locate`/`splice` machinery.
///
/// `table_id` / `row` / `col` reuse the exact identifiers the
/// `cell_rects` hit-test surface already carries (`CellRect`), so a
/// `HitTest` that lands in a cell and the `LineLayout` it should map to
/// share one coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CellAddr {
    /// `<Table Self="...">` id of the owning table.
    pub table_id: String,
    /// Template row (0-based). Header / footer replays report their
    /// SOURCE row so a caret in a replayed header maps to the original
    /// cell's text — matching `CellRect`'s convention.
    pub row: u32,
    /// Column (0-based). Span-origin column for spanned cells.
    pub col: u32,
}

/// One laid-out line of a story, in page-local pt coordinates.
///
/// Captured at emit time and stored on the hosting `BuiltPage`. The
/// canvas reconstructs whole-story layouts via
/// [`BuiltDocument::story_layout`], which walks every page and
/// gathers lines whose `story_id` matches.
///
/// Page-local means: `(0, 0)` is the page's top-left in spread
/// coordinates; subtract the page's `spread_origin` from spread-space
/// values to land here. Y grows downward.
#[derive(Debug, Clone)]
pub struct LineLayout {
    /// IDML `<Story Self="...">` of the source story.
    pub story_id: String,
    /// Page hosting this visible line. Threaded text spans pages, so
    /// every line records its host independently.
    pub page_id: PageId,
    /// W1.13 — disjoint paragraph-address qualifier. `None` for lines
    /// in the story's main paragraph flow; `Some` for lines inside a
    /// table cell, where `paragraph_idx` is then local to that cell's
    /// own paragraph stream. Two lines belong to the same paragraph
    /// only when BOTH `(cell, paragraph_idx)` are equal — this is what
    /// disambiguates body paragraph N from cell paragraph N. See
    /// [`CellAddr`].
    pub cell: Option<CellAddr>,
    /// Paragraph index within its own stream: within the story's body
    /// flow when `cell` is `None`, or within `cell.paragraphs` when
    /// `cell` is `Some`. NOT globally unique on its own — pair it with
    /// `cell` for an unambiguous address.
    pub paragraph_idx: u32,
    /// Line index within the paragraph.
    pub line_idx: u32,
    /// Frame's IDML `Self` id when present (synthetic frames may have
    /// none). Lets the overlay attribute selections to specific
    /// frames within a chain.
    pub frame_id: Option<String>,
    /// Page-local baseline y in pt.
    pub baseline_y_pt: f32,
    /// Approximate ascent above the baseline in pt. Phase 3 first cut
    /// derives this as `0.8 × line_height`; real font metrics arrive
    /// alongside the main-thread fast composer.
    pub ascent_pt: f32,
    /// Approximate descent below the baseline in pt (`0.2 × line_height`).
    pub descent_pt: f32,
    /// Paragraph-local byte range covered by this visible line.
    pub byte_range: std::ops::Range<u32>,
    /// Per-glyph-cluster page-local positions, in left-to-right order.
    /// The hit-tester bisects on `x_pt`; the caret rule keys on
    /// `byte` for an exact-offset lookup.
    pub clusters: Vec<ClusterPos>,
}

/// One glyph cluster's page-local position. Cluster bytes are
/// paragraph-local so they compose with `LineLayout::byte_range`.
#[derive(Debug, Clone, Copy)]
pub struct ClusterPos {
    pub byte: u32,
    /// Left edge in page-local pt.
    pub x_pt: f32,
    /// Cluster's horizontal advance in page-local pt.
    pub advance_pt: f32,
}

/// Multi-page render output. Each entry is a fully populated
/// `BuiltPage` with its own DisplayList and dimensions.
#[derive(Debug)]
pub struct BuiltDocument {
    pub pages: Vec<BuiltPage>,
    pub stats: PipelineStats,
    /// Track 2: per-laid-out-line break records, when collection was
    /// enabled via [`PipelineOptions::collect_breaks`]. Empty when
    /// the flag was off (the default). Used by the A/B harness to
    /// compare candidate-side break decisions against PDF-derived
    /// references.
    pub breaks: Vec<BreakRecord>,
    /// Structured signals for lossy / degraded renders (overset text
    /// dropped, missing image links, section-numbering fallback).
    /// Empty for a fully-faithful render. Aggregated from the
    /// per-page collectors plus the per-story emit channel; the
    /// underlying `tracing::warn!` calls still fire too.
    pub diagnostics: RenderDiagnostics,
}

impl BuiltDocument {
    /// Look up a page by stable id. Linear scan — `pages` typically
    /// fits in cache, and the canvas worker calls this once per
    /// viewport-visible page per dirty event. If a future profile
    /// shows this in hot paths, swap to an `IndexMap` keyed by
    /// `PageId` without changing the public signature.
    pub fn page(&self, id: &PageId) -> Option<&BuiltPage> {
        self.pages.iter().find(|p| &p.id == id)
    }

    /// Convenience for the canvas Tier 4: hand back just the slice
    /// of render commands for one page. Mirrors the
    /// `display_list_for_page(page_id)` accessor named in the
    /// canvas concept (docs/paged/canvas.md §4.4).
    pub fn display_list_for_page(&self, id: &PageId) -> Option<&DisplayList> {
        self.page(id).map(|p| &p.list)
    }

    /// All page ids in document order. The canvas's page navigator
    /// and minimap consume this to drive the snapshot atlas. Returns
    /// an iterator so callers don't pay for the `Vec` allocation when
    /// they only want a count or a prefix.
    pub fn page_ids(&self) -> impl Iterator<Item = &PageId> {
        self.pages.iter().map(|p| &p.id)
    }

    /// Lines belonging to `story_id`, gathered from every page that
    /// hosts part of the story (chained text spans pages). Sorted by
    /// `(paragraph_idx, line_idx)` so the canvas can walk the story
    /// in document order without re-sorting. Empty when the story is
    /// unplaced or unparsed.
    ///
    /// Phase 3 correctness layer (Item A) — selection, caret, and
    /// selection geometry all key off this index.
    pub fn story_layout(&self, story_id: &str) -> Vec<&LineLayout> {
        let mut out: Vec<&LineLayout> = Vec::new();
        for page in &self.pages {
            for line in &page.story_layout {
                if line.story_id == story_id {
                    out.push(line);
                }
            }
        }
        // W1.13 — sort within each disjoint stream. Body lines
        // (`cell == None`) sort ahead of any cell stream; cell streams
        // sort by their `(table_id, row, col)` qualifier so callers can
        // walk one cell's lines contiguously. Within a stream, the
        // `(paragraph_idx, line_idx)` order is document order. Pairing
        // `cell` into the key is what keeps body paragraph N and cell
        // paragraph N from interleaving.
        out.sort_by(|a, b| {
            cell_sort_key(&a.cell)
                .cmp(&cell_sort_key(&b.cell))
                .then((a.paragraph_idx, a.line_idx).cmp(&(b.paragraph_idx, b.line_idx)))
        });
        out
    }

    /// W1.13 — all lines of `story_id` that belong to the given cell
    /// stream (`cell == Some(addr)`), in document order. Body lines are
    /// excluded. Used by the cell-aware caret / hit-test / byte-offset
    /// machinery to walk a single cell's paragraph flow.
    pub fn cell_layout<'a>(&'a self, story_id: &str, addr: &CellAddr) -> Vec<&'a LineLayout> {
        let mut out: Vec<&LineLayout> = Vec::new();
        for page in &self.pages {
            for line in &page.story_layout {
                if line.story_id == story_id && line.cell.as_ref() == Some(addr) {
                    out.push(line);
                }
            }
        }
        out.sort_by(|a, b| (a.paragraph_idx, a.line_idx).cmp(&(b.paragraph_idx, b.line_idx)));
        out
    }
}

/// Total ordering key for the cell qualifier: body (`None`) first,
/// then cell streams ordered by `(table_id, row, col)`.
fn cell_sort_key(cell: &Option<CellAddr>) -> (u8, &str, u32, u32) {
    match cell {
        None => (0, "", 0, 0),
        Some(c) => (1, c.table_id.as_str(), c.row, c.col),
    }
}

/// One laid-out line, captured by the renderer when
/// [`PipelineOptions::collect_breaks`] is set. Coordinates are in pt
/// (1 pt = 1/72 in); byte offsets are paragraph-local.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BreakRecord {
    pub story_id: String,
    pub paragraph_idx: u32,
    pub line_idx: u32,
    pub page_idx: u32,
    pub frame_idx: u32,
    pub first_byte: u32,
    /// Exclusive upper bound (matches `Range`'s `end`).
    pub last_byte: u32,
    pub baseline_y_pt: f32,
    pub width_pt: f32,
    /// Cycle-5 Track 1: the line's source text, sliced from the
    /// paragraph's concatenated bytes by `[first_byte..last_byte]`.
    /// Newlines / forced-break characters preserved as-is. Empty
    /// when collection wasn't enabled; populated only when
    /// `PipelineOptions::collect_breaks` is set.
    pub source_text: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PipelineStats {
    pub spreads: usize,
    pub pages: usize,
    pub frames: usize,
    pub stories: usize,
    pub paragraphs: usize,
    pub runs: usize,
    pub glyphs: usize,
    pub lines: usize,
    /// Number of distinct URIs decoded into the renderer-scoped
    /// `DecodedImage` cache. Stays 0 when no image-bearing frames
    /// were encountered; otherwise lets callers observe cross-page
    /// image sharing (one decode per URI, regardless of how many
    /// rectangles or pages reference it).
    pub decoded_images: usize,
    /// Number of laid-out lines dropped because they fell past the
    /// last frame in their chain (typically a wider font substitute).
    /// Surfaced for diagnostics; non-zero means a story didn't fit
    /// its declared frame chain (P-13).
    pub dropped_overflow_lines: usize,
}

/// Walks body pages in document order, computing each page's
/// user-visible label from the document's `<Section>` numbering rules.
/// `<Page Name>` (the label InDesign baked at export) stays
/// authoritative when present; the section rules fill the gap for
/// `Name`-absent pages and keep the running counter coherent so a later
/// `Name`-absent page still numbers correctly. With no sections and no
/// `Name`, this reproduces the historical 1-based body-page fallback
/// exactly (`current_number == pages.len() + 1`).
struct SectionWalk<'a> {
    sections: &'a [paged_parse::designmap::Section],
    /// page `Self` → index into `sections` for the section starting there.
    starts: HashMap<&'a str, usize>,
    active: Option<usize>,
    /// Number assigned to the most recently processed page (0 before any).
    current_number: u32,
    /// True once any page fell back to a computed (non-`Name`) label.
    used_fallback: bool,
}

impl<'a> SectionWalk<'a> {
    fn new(sections: &'a [paged_parse::designmap::Section]) -> Self {
        let mut starts = HashMap::new();
        for (i, s) in sections.iter().enumerate() {
            if let Some(ps) = s.page_start.as_deref() {
                starts.entry(ps).or_insert(i);
            }
        }
        Self {
            sections,
            starts,
            active: None,
            current_number: 0,
            used_fallback: false,
        }
    }

    /// Advance to the next body page and return its label.
    fn next_label(&mut self, page_self_id: Option<&str>, page_name: Option<&str>) -> String {
        // A section starting at this page reseeds the counter; otherwise
        // it just advances by one within the active (or implicit) section.
        match page_self_id.and_then(|sid| self.starts.get(sid).copied()) {
            Some(si) => {
                let sec = &self.sections[si];
                self.active = Some(si);
                self.current_number = if sec.continue_numbering {
                    self.current_number + 1
                } else {
                    sec.start_at.unwrap_or(1)
                };
            }
            None => self.current_number += 1,
        }

        if let Some(name) = page_name {
            return name.to_string();
        }
        self.used_fallback = true;
        let style = self
            .active
            .map(|si| self.sections[si].numbering_style)
            .unwrap_or(paged_parse::designmap::NumberingStyle::Arabic);
        let mut label = style.format(self.current_number);
        if let Some(sec) = self.active.map(|si| &self.sections[si]) {
            if sec.include_prefix {
                if let Some(prefix) = &sec.section_prefix {
                    label = format!("{prefix}{label}");
                }
            }
        }
        label
    }
}

/// Build one `BuiltPage` per `<Page>` in the document. Each page's
/// display list contains only frames whose centres fall inside the
/// page's `GeometricBounds`. Frames placed entirely on the pasteboard
/// (rare) land on the first page so they don't disappear silently.
///
/// Returns a `BuiltDocument` with aggregated stats. Use `build` for
/// the historical single-page (union of all bounds) shape.
pub fn build_document(
    document: &Document,
    options: &PipelineOptions,
) -> anyhow::Result<BuiltDocument> {
    let palette = &document.palette;
    // Build the CMYK ICC transform once per render. Failures are
    // logged + swallowed: if the profile is malformed we silently
    // fall back to naive math so the render still produces output.
    let cmyk_xform = options.cmyk_icc_profile.and_then(|bytes| {
        // Default settings route through the back-compat shim so the
        // per-target intent defaults (native RelColorimetric+BPC,
        // wasm Perceptual) stay bit-identical; explicit document
        // colour settings take the parameterised path.
        let default_settings = options.cmyk_intent == paged_color::Intent::RelativeColorimetric
            && options.cmyk_bpc;
        let built = if default_settings {
            paged_color::IccTransform::cmyk_to_linear_rgb(bytes)
        } else {
            paged_color::IccTransform::cmyk_to_linear_rgb_with(
                bytes,
                options.cmyk_intent,
                options.cmyk_bpc,
            )
        };
        match built {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(error = %e, "failed to build CMYK ICC transform; using naive conversion");
                None
            }
        }
    });
    let mut pages: Vec<BuiltPage> = Vec::new();
    let mut total_stats = PipelineStats::default();
    let mut breaks: Vec<BreakRecord> = Vec::new();
    // Document-level diagnostics drained from per-story emitters
    // (overset) and from page-label computation (section fallback).
    // Per-page image diagnostics are aggregated separately at the end.
    let mut emit_diagnostics: Vec<Diagnostic> = Vec::new();

    // W1.7 Phase B: precompute each AutoSizing text frame's GROWN
    // inner-coord bounds, keyed by `Self` id. Computed once up front so
    // the frame-paint pass (the box stretches to fit) and the text-wrap
    // collection (neighbours wrap around the grown box) both see the
    // same extent. Only frames that actually grow get an entry.
    let auto_sized_bounds: HashMap<String, paged_parse::Bounds> = document
        .spreads
        .iter()
        .flat_map(|parsed| parsed.spread.text_frames.iter())
        .filter_map(|frame| {
            let id = frame.self_id.clone()?;
            let grown = compute_auto_sized_bounds(document, frame)?;
            Some((id, grown))
        })
        .collect();

    // Walk every page in every spread. We capture each page's bounds,
    // origin, and applied-master reference so the next passes can
    // route frames by containment and apply master backgrounds.
    //
    // `spread_page_ranges[i]` is the half-open page-index range
    // owned by `document.spreads[i]`; frames within a spread route
    // only to that range, since each IDML spread has its own
    // coordinate system and two spreads' page bounds can collide.
    let mut page_geometries: Vec<PageGeom> = Vec::new();
    let mut page_labels: Vec<String> = Vec::new();
    let mut section_walk = SectionWalk::new(&document.container.designmap.sections);
    let mut spread_page_ranges: Vec<std::ops::Range<usize>> =
        Vec::with_capacity(document.spreads.len());
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        total_stats.spreads += 1;
        let start = pages.len();
        // W1.9 — the spread's own `<Spread ItemTransform>` rotation/scale
        // (linear part; translation drops because it cancels against the
        // spread-inner page origin). IDENTITY for the common case
        // (absent / pure-translation transform).
        let spread_transform = spread_linear_transform(parsed.spread.item_transform);
        for (local_idx, p) in parsed.spread.pages.iter().enumerate() {
            // Per spec §10.3.3: GeometricBounds is in the page's
            // *inner* coords; ItemTransform maps page-inner →
            // spread. Real InDesign exports rely on this — without
            // it every frame routes to the wrong page (or to none).
            let bounds_in_spread = transform_bounds(p.bounds, p.item_transform);
            page_geometries.push(PageGeom {
                bounds_in_spread,
                applied_master: p.applied_master.clone(),
                host_spread_idx: spread_idx,
                local_page_idx: local_idx,
            });
            // Page.Name carries the user-visible label as InDesign
            // rendered it (Arabic / Roman / arbitrary section
            // override) and stays authoritative. When absent, the
            // section walk computes the label from the document's
            // `<Section>` numbering rules (falling back to the 1-based
            // body-page index when no section applies).
            page_labels.push(section_walk.next_label(p.self_id.as_deref(), p.name.as_deref()));
            let page_id = p
                .self_id
                .clone()
                .map(PageId)
                .unwrap_or_else(|| PageId::synthetic(spread_idx, local_idx));
            let mut page_list = DisplayList::new();
            if options.collect_glyph_runs {
                page_list.glyph_runs = Some(paged_compose::GlyphRunTable::default());
            }
            if options.collect_link_regions {
                page_list.link_regions = Some(paged_compose::LinkRegionTable::default());
            }
            pages.push(BuiltPage {
                id: page_id,
                width_pt: bounds_in_spread.width(),
                height_pt: bounds_in_spread.height(),
                spread_origin: (bounds_in_spread.left, bounds_in_spread.top),
                spread_transform,
                list: page_list,
                layout_generation: 0,
                numbering_generation: 0,
                stats: PipelineStats::default(),
                story_layout: Vec::new(),
                footnotes: Vec::new(),
                diagnostics: Vec::new(),
                cell_rects: Vec::new(),
            });
        }
        spread_page_ranges.push(start..pages.len());
    }
    total_stats.pages = pages.len();
    // Surface that one or more page labels were computed rather than
    // read from a baked `<Page Name>` — an honest signal that numbering
    // came from section rules / the 1-based fallback, not InDesign.
    if section_walk.used_fallback {
        let detail = if document.container.designmap.sections.is_empty() {
            "page label(s) computed via 1-based fallback (no <Page Name>, no <Section>)"
        } else {
            "page label(s) computed from <Section> numbering rules (no baked <Page Name>)"
        };
        emit_diagnostics.push(Diagnostic::new(
            DiagnosticCode::SectionNumberingFallback,
            detail,
        ));
    }
    if pages.is_empty() {
        // Documents without a page (rare but valid) get a single
        // letter-sized canvas so callers always see a renderable output.
        pages.push(BuiltPage {
            id: PageId::synthetic(0, 0),
            width_pt: 612.0,
            height_pt: 792.0,
            spread_origin: (0.0, 0.0),
            spread_transform: Transform::IDENTITY,
            list: DisplayList::new(),
            layout_generation: 0,
            numbering_generation: 0,
            stats: PipelineStats::default(),
            story_layout: Vec::new(),
            footnotes: Vec::new(),
            diagnostics: Vec::new(),
            cell_rects: Vec::new(),
        });
        page_geometries.push(PageGeom {
            bounds_in_spread: paged_parse::Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 792.0,
                right: 612.0,
            },
            applied_master: None,
            host_spread_idx: 0,
            local_page_idx: 0,
        });
        page_labels.push("1".to_string());
    }

    // W1.4 — total body-page count, frozen here (pages are all created
    // above). Feeds `PageCountType` text-variable resolution.
    let total_page_count = pages.len();
    // W1.4 — `<Page Self=...>` id → flat body-page index, for resolving
    // hyperlink page destinations. Built only when link-region
    // collection is on (the live render never pays for it). A page's
    // `PageId(self_id)` is its `<Page Self>`; synthetic ids (no baked
    // Self) won't be hyperlink targets, so they harmlessly miss.
    let page_index_map: HashMap<String, u32> = if options.collect_link_regions {
        pages
            .iter()
            .enumerate()
            .map(|(idx, p)| (p.id.0.clone(), idx as u32))
            .collect()
    } else {
        HashMap::new()
    };

    // Master-spread pass — runs first so master items end up at the
    // bottom of each page's display list (page-level frames overlay on
    // top). Master frames are stamped into every page that references
    // the master.
    //
    // (master_text_emissions is populated in this loop and consumed by
    // a later master-story pass that emits page-number footers, headers,
    // and other master story content per body page.)
    let mut master_text_emissions: Vec<(usize, TextFrame)> = Vec::new();
    //
    // Per IDML spec §10.3.3, master items live in master-spread
    // coords (each master page maps to spread via its own
    // ItemTransform). The live `<Page>`'s `MasterPageTransform`
    // positions the master overlay relative to the live page; for
    // the common case both transforms are identity and the
    // (dx, dy) collapses to "shift master-page origin → live-page
    // origin". We compute it via the spread-coord bounds of both
    // sides so the math composes cleanly with our existing Page
    // ItemTransform plumbing.
    for (i, geom) in page_geometries.iter().enumerate() {
        let Some(master_ref) = geom.applied_master.as_deref() else {
            continue;
        };
        let Some(master) = document.master_spread(master_ref) else {
            continue;
        };
        if master.spread.pages.is_empty() {
            continue;
        }
        let body_page = document.spreads[geom.host_spread_idx]
            .spread
            .pages
            .get(geom.local_page_idx);
        // `ShowMasterItems="false"` hides every master overlay item for
        // this page (InDesign's per-page "Hide Master Items"). Skipping
        // the whole loop body suppresses master frames, lines, and the
        // master-story page-number text (all stamped below) at once.
        if body_page.and_then(|p| p.show_master_items) == Some(false) {
            continue;
        }
        // Body-page OverrideList enumerates master items the body has
        // replaced with its own copies — skip them here so we don't
        // stamp the placeholder under the body's override.
        let override_set: std::collections::HashSet<&str> = body_page
            .map(|p| p.override_list.iter().map(String::as_str).collect())
            .unwrap_or_default();

        // Each master page in spread coords. Master items get routed
        // to one of these by their own spread-coord centroid; the
        // matching live page consumes only the items belonging to
        // its same-ordinal master page. This is what InDesign's
        // "Master Page Overlay" feature actually does — without
        // routing, a master with both white-LEFT-page and navy-RIGHT-
        // page rectangles would stamp both onto every live page.
        let master_page_bounds: Vec<paged_parse::Bounds> = master
            .spread
            .pages
            .iter()
            .map(|p| transform_bounds(p.bounds, p.item_transform))
            .collect();
        let local_master_page_idx = geom.local_page_idx.min(master.spread.pages.len() - 1);
        let master_page_origin = (
            master_page_bounds[local_master_page_idx].left,
            master_page_bounds[local_master_page_idx].top,
        );
        let target_origin = pages[i].spread_origin;
        // MasterPageTransform sits between master-spread coords and
        // live-page coords (for sample.idml it is identity). Build the
        // full outer transform once for this page — `translate(live
        // origin) ∘ MPT ∘ translate(-master origin)` — so a MPT carrying
        // rotation/scale (not just translation) is honoured. With an
        // identity MPT this collapses to the plain origin shift the
        // common case relies on. Each master item below is stamped as
        // `mpt_outer ∘ item_transform`.
        let mpt = document.spreads[geom.host_spread_idx]
            .spread
            .pages
            .get(geom.local_page_idx)
            .and_then(|p| p.master_page_transform);
        // W1.9 — the MASTER spread's own `<MasterSpread ItemTransform>`
        // rotation/scale (linear part). Inserted between the
        // MasterPageTransform and the master-page-origin re-base so it
        // rotates/scales the master overlay about the master page origin,
        // mirroring how the body spread's `spread_transform` rotates the
        // body page. IDENTITY for the common (untransformed master) case,
        // collapsing the chain to the historical translation-only stamp.
        // (The body spread's own transform separately rides the live
        // page's `spread_transform` in `frame_outer_transform`.)
        let master_spread_linear = spread_linear_transform(master.spread.item_transform);
        let mpt_outer = Transform::translate(target_origin.0, target_origin.1)
            .compose(&Transform(mpt.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])))
            .compose(&master_spread_linear)
            .compose(&Transform::translate(
                -master_page_origin.0,
                -master_page_origin.1,
            ));

        // Pick the master page index that contains the centroid of
        // the given spread-coord bounds; falls back to the nearest
        // page so items hugging the centre line don't get dropped.
        let master_page_for = |b: paged_parse::Bounds| -> usize {
            let cx = (b.left + b.right) * 0.5;
            let cy = (b.top + b.bottom) * 0.5;
            for (idx, mb) in master_page_bounds.iter().enumerate() {
                if cx >= mb.left && cx <= mb.right && cy >= mb.top && cy <= mb.bottom {
                    return idx;
                }
            }
            // Outside any master page (rare): pick by horizontal proximity.
            master_page_bounds
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    let da = ((a.left + a.right) * 0.5 - cx).abs();
                    let db = ((b.left + b.right) * 0.5 - cx).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(idx, _)| idx)
                .unwrap_or(0)
        };

        // Master items belong to the current live page when either
        // (a) their centroid lands on the matching master page, or
        // (b) they're "full-bleed-ish" — area ≥ 50% of a master page
        //     AND the item's spread bounds intersect this master page.
        // The second arm covers spread-spanning brand-colour
        // backgrounds whose centroid lands across the page fold; the
        // pure centroid test would route them to the wrong page or
        // (for items straddling the gutter) to one page only.
        let target_master = &master_page_bounds[local_master_page_idx];
        let target_master_area = (target_master.right - target_master.left).max(0.0)
            * (target_master.bottom - target_master.top).max(0.0);
        let item_belongs = |b: paged_parse::Bounds| -> bool {
            if master_page_for(b) == local_master_page_idx {
                return true;
            }
            let item_area = (b.right - b.left).max(0.0) * (b.bottom - b.top).max(0.0);
            if target_master_area <= 0.0 || item_area < 0.5 * target_master_area {
                return false;
            }
            // Intersection test against the target master page.
            b.right > target_master.left
                && b.left < target_master.right
                && b.bottom > target_master.top
                && b.top < target_master.bottom
        };

        for frame in &master.spread.text_frames {
            let spread_b = transform_bounds(frame.bounds, frame.item_transform);
            if !item_belongs(spread_b) {
                continue;
            }
            if frame
                .self_id
                .as_deref()
                .is_some_and(|id| override_set.contains(id))
            {
                continue;
            }
            total_stats.frames += 1;
            // Master items live in master-spread coords. Compose an
            // outer translate(dx, dy) into the frame's existing
            // ItemTransform so the inner-coord rect ends up in the
            // *live* spread coords once frame_outer_transform applies.
            // Mutating bounds (inner coords) would be wrong now that
            // PathGeometry-derived shapes carry geometry in inner
            // space.
            let mut copy = frame.clone();
            copy.item_transform = Some(compose_outer_matrix(mpt_outer, copy.item_transform));
            emit_text_frame_into(
                &mut pages[i],
                &copy,
                document,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
                None, // master items don't carry a drop shadow today.
                None, // master frames don't auto-size in our model.
            );
            // Stash a relocated copy so the master-story pass below
            // can flow this frame's hosted story (page-number footers,
            // running headers, etc.) onto this body page. Skipping it
            // when ParentStory is missing is fine — the rectangle was
            // still drawn above.
            if copy.parent_story.is_some() {
                master_text_emissions.push((i, copy));
            }
        }
        for rect in &master.spread.rectangles {
            let spread_b = transform_bounds(rect.bounds, rect.item_transform);
            if !item_belongs(spread_b) {
                continue;
            }
            if rect
                .self_id
                .as_deref()
                .is_some_and(|id| override_set.contains(id))
            {
                continue;
            }
            total_stats.frames += 1;
            let mut copy = rect.clone();
            copy.item_transform = Some(compose_outer_matrix(mpt_outer, copy.item_transform));
            emit_rectangle_into(
                &mut pages[i],
                &copy,
                document,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
                None,
            );
        }
        // Non-text background shapes (Polygon / Oval / GraphicLine)
        // routed onto live body pages. The legacy code stopped at
        // Rectangle, so master-spread page backgrounds drawn as
        // polygons / ovals (full-bleed brand colours, decorative
        // bezel strokes) silently disappeared on every body page.
        for poly in &master.spread.polygons {
            let spread_b = transform_bounds(poly.bounds, poly.item_transform);
            if !item_belongs(spread_b) {
                continue;
            }
            if poly
                .self_id
                .as_deref()
                .is_some_and(|id| override_set.contains(id))
            {
                continue;
            }
            total_stats.frames += 1;
            let mut copy = poly.clone();
            copy.item_transform = Some(compose_outer_matrix(mpt_outer, copy.item_transform));
            emit_polygon_into(
                &mut pages[i],
                &copy,
                document,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
            );
        }
        for oval in &master.spread.ovals {
            let spread_b = transform_bounds(oval.bounds, oval.item_transform);
            if !item_belongs(spread_b) {
                continue;
            }
            if oval
                .self_id
                .as_deref()
                .is_some_and(|id| override_set.contains(id))
            {
                continue;
            }
            total_stats.frames += 1;
            let mut copy = oval.clone();
            copy.item_transform = Some(compose_outer_matrix(mpt_outer, copy.item_transform));
            emit_oval_into(
                &mut pages[i],
                &copy,
                document,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
            );
        }
        for line in &master.spread.graphic_lines {
            let spread_b = transform_bounds(line.bounds, line.item_transform);
            if !item_belongs(spread_b) {
                continue;
            }
            if line
                .self_id
                .as_deref()
                .is_some_and(|id| override_set.contains(id))
            {
                continue;
            }
            total_stats.frames += 1;
            let mut copy = line.clone();
            copy.item_transform = Some(compose_outer_matrix(mpt_outer, copy.item_transform));
            emit_line_into(&mut pages[i], &copy, document, palette, cmyk_xform.as_ref());
        }
    }

    // Frame pass: route every frame to the page whose bounds contain
    // Per-frame (by Self id) page lookup. The story pass builds
    // each story's frame chain via Document::frame_chain and uses
    // this map to find each chain entry's page so threaded stories
    // can route line emission across pages.
    let mut frame_to_page: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    // Per-page (URI → ImageId) cache so multiple rectangles on the
    // same page sharing an image share a single ImageId in the
    // page's display list.
    let mut page_image_caches: Vec<HashMap<String, paged_compose::ImageId>> =
        (0..pages.len()).map(|_| HashMap::new()).collect();
    // Renderer-scoped (URI → DecodedImage) cache so an image
    // referenced from multiple pages is decoded once. The cached
    // DecodedImage is cloned into each page's image pool — the
    // memcpy is cheap; the saved decode (PNG/JPEG → RGBA) is not.
    // Build a layer-visibility map once: any item whose `ItemLayer`
    // points at a hidden or non-printable layer is suppressed. Items
    // without an explicit ItemLayer always render — matches InDesign's
    // single-layer-by-default behaviour. The same predicate is consumed
    // by the canvas hit-tester so selection cannot disagree with paint.
    let layer_renders = paged_scene::build_layer_render_map(&document.container.designmap);
    let layer_visible = |layer_ref: Option<&str>| -> bool {
        paged_scene::lookup_layer_render_visible(&layer_renders, layer_ref)
    };

    // Perf-S — when the caller supplies a persistent cache, decode
    // results survive across `build_document` calls; otherwise fall
    // back to a per-call scratch. The match holds the RefMut alive
    // for the duration of the build via the `_owned_borrow` binding
    // — dropping it would invalidate the `&mut HashMap` reference.
    let mut local_image_cache: HashMap<String, paged_compose::DecodedImage> = HashMap::new();
    let mut _owned_borrow: Option<
        std::cell::RefMut<'_, HashMap<String, paged_compose::DecodedImage>>,
    > = None;
    let decoded_image_cache: &mut HashMap<String, paged_compose::DecodedImage> =
        match options.image_decode_cache {
            Some(rc) => {
                _owned_borrow = Some(rc.borrow_mut());
                _owned_borrow.as_mut().unwrap()
            }
            None => &mut local_image_cache,
        };
    // Aggregated queue of image-bearing anchored Rectangles captured
    // during the master + body story passes. Drained after both
    // passes complete so `emit_rectangle_image` can route the
    // already-resolved placements through the per-page + decoded
    // image caches that live in this scope. Order is preserved so
    // multiple anchored images on the same page composite in
    // story-pass order.
    let mut anchored_image_queue: Vec<AnchoredImageEmit> = Vec::new();
    // Per-spread per-frame-kind command spans, captured in document
    // order so the post-pass `group_pass` can translate each
    // group's `Vec<FrameRef>` into the page-space command ranges
    // it brackets with `BeginBlendGroup` / `EndBlendGroup`.
    let mut spread_frame_spans: Vec<crate::module::SpreadFrameSpans> =
        Vec::with_capacity(document.spreads.len());
    // Q-10: IDML lists layers bottom-first (designmap[0] = backmost,
    // paints first); see the cycle-8 sort below and the convention
    // note in `paged_scene::layer`. Shared helper so the canvas
    // hit-tester walks items in the same paint order — divergence here
    // would break selection on multi-layer documents.
    let layer_z_index = paged_scene::layer_z_index(&document.container.designmap);
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        let range = spread_page_ranges[spread_idx].clone();
        let local_geoms = &page_geometries[range.clone()];
        let mut frame_spans = crate::module::SpreadFrameSpans {
            text_frames: vec![None; spread.text_frames.len()],
            rectangles: vec![None; spread.rectangles.len()],
            ovals: vec![None; spread.ovals.len()],
            graphic_lines: vec![None; spread.graphic_lines.len()],
            polygons: vec![None; spread.polygons.len()],
        };

        // Q-10: build a flat (layer_z, xml_order, FrameRef) list from
        // `frames_in_order` so cross-shape z-order honours ItemLayer.
        // Items without `ItemLayer` keep their XML position by sharing
        // `usize::MAX` as the sort key — combined with a stable sort
        // they stay where they were. The sort is a no-op when all
        // items resolve to the same layer-z (legacy behaviour).
        let layer_z_of = |fr: paged_parse::FrameRef| -> usize {
            let id = match fr {
                paged_parse::FrameRef::TextFrame(i) => spread
                    .text_frames
                    .get(i)
                    .and_then(|f| f.item_layer.as_deref()),
                paged_parse::FrameRef::Rectangle(i) => spread
                    .rectangles
                    .get(i)
                    .and_then(|f| f.item_layer.as_deref()),
                paged_parse::FrameRef::Oval(i) => {
                    spread.ovals.get(i).and_then(|f| f.item_layer.as_deref())
                }
                paged_parse::FrameRef::GraphicLine(i) => spread
                    .graphic_lines
                    .get(i)
                    .and_then(|f| f.item_layer.as_deref()),
                paged_parse::FrameRef::Polygon(i) => {
                    spread.polygons.get(i).and_then(|f| f.item_layer.as_deref())
                }
                // Group: derive layer from the first leaf member with
                // an ItemLayer. If none, treat as "no layer" (MAX).
                paged_parse::FrameRef::Group(_) => None,
            };
            id.and_then(|s| layer_z_index.get(s).copied())
                .unwrap_or(usize::MAX)
        };
        let frames_ordered: Vec<paged_parse::FrameRef> = if spread.frames_in_order.is_empty() {
            // Legacy path: a parser revision predating
            // `frames_in_order` (or a spread carrying only frames the
            // parser couldn't classify) → fall through to the same
            // XML-vec walk as before. Builds a synthetic flat list by
            // concatenating the per-shape vecs in their historical
            // order.
            let mut v: Vec<paged_parse::FrameRef> = Vec::new();
            v.extend((0..spread.text_frames.len()).map(paged_parse::FrameRef::TextFrame));
            v.extend((0..spread.rectangles.len()).map(paged_parse::FrameRef::Rectangle));
            v.extend((0..spread.ovals.len()).map(paged_parse::FrameRef::Oval));
            v.extend((0..spread.graphic_lines.len()).map(paged_parse::FrameRef::GraphicLine));
            v.extend((0..spread.polygons.len()).map(paged_parse::FrameRef::Polygon));
            v
        } else {
            let mut keyed: Vec<(usize, usize, paged_parse::FrameRef)> = spread
                .frames_in_order
                .iter()
                .enumerate()
                .map(|(xi, &fr)| (layer_z_of(fr), xi, fr))
                .collect();
            // Sort no-op safeguard: only reorder when at least two
            // distinct layer-z values appear. Single-layer spreads
            // (the overwhelming majority) keep verbatim XML order.
            let mut zs = keyed.iter().map(|(z, _, _)| *z);
            let first = zs.next();
            let multi_layer = first.is_some_and(|f| zs.any(|z| z != f));
            if multi_layer {
                // Cycle-8: IDML's designmap lists layers in the order
                // matching InDesign's layer panel from BOTTOM to TOP
                // (designmap[0] = bottom layer, paints first; the
                // cycle-2 Q-10 commit's assumption of top-first was
                // inverted, manifesting on company-profile-template
                // page 20 where the Bg layer covered the image
                // instead of sitting beneath it). Sort ascending
                // by layer-z so low-index (bottom) layers paint first
                // and high-index (top) layers paint last. Stable
                // sort preserves XML order as the tiebreaker within
                // a layer.
                keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            }
            keyed.into_iter().map(|(_, _, fr)| fr).collect()
        };

        // Emit one FrameRef. Recurses through Group members so group
        // children render at the group's XML slot.
        fn emit_one(
            fr: paged_parse::FrameRef,
            spread: &paged_parse::Spread,
            range: &std::ops::Range<usize>,
            local_geoms: &[PageGeom],
            pages: &mut [BuiltPage],
            page_image_caches: &mut [HashMap<String, paged_compose::ImageId>],
            decoded_image_cache: &mut HashMap<String, paged_compose::DecodedImage>,
            frame_to_page: &mut HashMap<String, usize>,
            frame_spans: &mut crate::module::SpreadFrameSpans,
            total_stats: &mut PipelineStats,
            document: &Document,
            palette: &Graphic,
            options: &PipelineOptions,
            cmyk_xform: Option<&paged_color::IccTransform>,
            auto_sized_bounds: &HashMap<String, paged_parse::Bounds>,
        ) {
            match fr {
                paged_parse::FrameRef::TextFrame(idx) => {
                    let Some(frame) = spread.text_frames.get(idx) else {
                        return;
                    };
                    if !is_layer_visible(document, frame.item_layer.as_deref()) {
                        return;
                    }
                    total_stats.frames += 1;
                    // W1.7 Phase B: paint the box at its grown extent when
                    // this frame auto-sizes (key by Self id). Routing still
                    // uses the authored bounds — the grown box only changes
                    // what's painted, not which page hosts the frame.
                    let grown = frame
                        .self_id
                        .as_deref()
                        .and_then(|id| auto_sized_bounds.get(id))
                        .copied();
                    let spread_bounds = transform_bounds(frame.bounds, frame.item_transform);
                    let centroid_local = page_for_frame(&spread_bounds, local_geoms).unwrap_or(0);
                    let centroid_page = range.start + centroid_local;
                    if let Some(self_id) = frame.self_id.clone() {
                        frame_to_page.insert(self_id, centroid_page);
                    }
                    let overlaps = pages_overlapping_frame(&spread_bounds, local_geoms);
                    let local_indices: Vec<usize> = if overlaps.is_empty() {
                        vec![centroid_local]
                    } else {
                        overlaps
                    };
                    for &local_idx in &local_indices {
                        let page_idx = range.start + local_idx;
                        let before = pages[page_idx].list.commands.len();
                        emit_text_frame_into(
                            &mut pages[page_idx],
                            frame,
                            document,
                            palette,
                            options.fallback_frame_fill,
                            cmyk_xform,
                            options.frame_drop_shadow,
                            grown,
                        );
                        let after = pages[page_idx].list.commands.len();
                        if after > before && frame_spans.text_frames[idx].is_none() {
                            frame_spans.text_frames[idx] = Some(crate::module::FrameCmdSpan {
                                page_idx,
                                start: before,
                                end: after,
                            });
                        }
                    }
                }
                paged_parse::FrameRef::Rectangle(idx) => {
                    let Some(rect) = spread.rectangles.get(idx) else {
                        return;
                    };
                    if !is_layer_visible(document, rect.item_layer.as_deref()) {
                        return;
                    }
                    total_stats.frames += 1;
                    let spread_bounds = transform_bounds(rect.bounds, rect.item_transform);
                    let overlaps = pages_overlapping_frame(&spread_bounds, local_geoms);
                    let local_indices: Vec<usize> = if overlaps.is_empty() {
                        vec![page_for_frame(&spread_bounds, local_geoms).unwrap_or(0)]
                    } else {
                        overlaps
                    };
                    // Cycle-8 Track 1: page-routing diagnostic. Emit
                    // one record per Rectangle when --trace-routing
                    // is on; downstream callers filter by self_id.
                    // Note: kept narrow so the trace is useful for
                    // future routing investigations without poking at
                    // every shape kind.
                    tracing::debug!(
                        target: "paged_renderer::routing",
                        kind = "rect",
                        self_id = rect.self_id.as_deref().unwrap_or("?"),
                        spread_bounds = ?spread_bounds,
                        chosen_local = ?local_indices,
                        "rect page-routing"
                    );
                    for &local_idx in &local_indices {
                        let page_idx = range.start + local_idx;
                        let before = pages[page_idx].list.commands.len();
                        emit_rectangle_into(
                            &mut pages[page_idx],
                            rect,
                            document,
                            palette,
                            options.fallback_frame_fill,
                            cmyk_xform,
                            options.frame_drop_shadow,
                        );
                        // emit_rectangle_image runs paired with the
                        // rectangle fill so the placed image sits on
                        // top of the solid fill in the same span.
                        emit_rectangle_image(
                            &mut pages[page_idx],
                            rect,
                            options,
                            &mut page_image_caches[page_idx],
                            decoded_image_cache,
                        );
                        let after = pages[page_idx].list.commands.len();
                        if after > before && frame_spans.rectangles[idx].is_none() {
                            frame_spans.rectangles[idx] = Some(crate::module::FrameCmdSpan {
                                page_idx,
                                start: before,
                                end: after,
                            });
                        }
                    }
                }
                paged_parse::FrameRef::Oval(idx) => {
                    let Some(oval) = spread.ovals.get(idx) else {
                        return;
                    };
                    if !is_layer_visible(document, oval.item_layer.as_deref()) {
                        return;
                    }
                    total_stats.frames += 1;
                    let spread_bounds = transform_bounds(oval.bounds, oval.item_transform);
                    let overlaps = pages_overlapping_frame(&spread_bounds, local_geoms);
                    let local_indices: Vec<usize> = if overlaps.is_empty() {
                        vec![page_for_frame(&spread_bounds, local_geoms).unwrap_or(0)]
                    } else {
                        overlaps
                    };
                    for &local_idx in &local_indices {
                        let page_idx = range.start + local_idx;
                        let before = pages[page_idx].list.commands.len();
                        emit_oval_into(
                            &mut pages[page_idx],
                            oval,
                            document,
                            palette,
                            options.fallback_frame_fill,
                            cmyk_xform,
                        );
                        emit_oval_image(
                            &mut pages[page_idx],
                            oval,
                            options,
                            &mut page_image_caches[page_idx],
                            decoded_image_cache,
                        );
                        let after = pages[page_idx].list.commands.len();
                        if after > before && frame_spans.ovals[idx].is_none() {
                            frame_spans.ovals[idx] = Some(crate::module::FrameCmdSpan {
                                page_idx,
                                start: before,
                                end: after,
                            });
                        }
                    }
                }
                paged_parse::FrameRef::GraphicLine(idx) => {
                    let Some(line) = spread.graphic_lines.get(idx) else {
                        return;
                    };
                    if !is_layer_visible(document, line.item_layer.as_deref()) {
                        return;
                    }
                    total_stats.frames += 1;
                    let spread_bounds = transform_bounds(line.bounds, line.item_transform);
                    let overlaps = pages_overlapping_frame(&spread_bounds, local_geoms);
                    let local_indices: Vec<usize> = if overlaps.is_empty() {
                        vec![page_for_frame(&spread_bounds, local_geoms).unwrap_or(0)]
                    } else {
                        overlaps
                    };
                    for &local_idx in &local_indices {
                        let page_idx = range.start + local_idx;
                        let before = pages[page_idx].list.commands.len();
                        emit_line_into(&mut pages[page_idx], line, document, palette, cmyk_xform);
                        let after = pages[page_idx].list.commands.len();
                        if after > before && frame_spans.graphic_lines[idx].is_none() {
                            frame_spans.graphic_lines[idx] = Some(crate::module::FrameCmdSpan {
                                page_idx,
                                start: before,
                                end: after,
                            });
                        }
                    }
                }
                paged_parse::FrameRef::Polygon(idx) => {
                    let Some(poly) = spread.polygons.get(idx) else {
                        return;
                    };
                    if !is_layer_visible(document, poly.item_layer.as_deref()) {
                        return;
                    }
                    total_stats.frames += 1;
                    let spread_bounds = transform_bounds(poly.bounds, poly.item_transform);
                    let overlaps = pages_overlapping_frame(&spread_bounds, local_geoms);
                    let local_indices: Vec<usize> = if overlaps.is_empty() {
                        vec![page_for_frame(&spread_bounds, local_geoms).unwrap_or(0)]
                    } else {
                        overlaps
                    };
                    for &local_idx in &local_indices {
                        let page_idx = range.start + local_idx;
                        let before = pages[page_idx].list.commands.len();
                        emit_polygon_into(
                            &mut pages[page_idx],
                            poly,
                            document,
                            palette,
                            options.fallback_frame_fill,
                            cmyk_xform,
                        );
                        emit_polygon_image(
                            &mut pages[page_idx],
                            poly,
                            options,
                            &mut page_image_caches[page_idx],
                            decoded_image_cache,
                        );
                        let after = pages[page_idx].list.commands.len();
                        if after > before && frame_spans.polygons[idx].is_none() {
                            frame_spans.polygons[idx] = Some(crate::module::FrameCmdSpan {
                                page_idx,
                                start: before,
                                end: after,
                            });
                        }
                    }
                }
                paged_parse::FrameRef::Group(gi) => {
                    if let Some(g) = spread.groups.get(gi) {
                        for &m in &g.members {
                            emit_one(
                                m,
                                spread,
                                range,
                                local_geoms,
                                pages,
                                page_image_caches,
                                decoded_image_cache,
                                frame_to_page,
                                frame_spans,
                                total_stats,
                                document,
                                palette,
                                options,
                                cmyk_xform,
                                auto_sized_bounds,
                            );
                        }
                    }
                }
            }
        }

        for fr in frames_ordered {
            emit_one(
                fr,
                spread,
                &range,
                local_geoms,
                &mut pages,
                &mut page_image_caches,
                decoded_image_cache,
                &mut frame_to_page,
                &mut frame_spans,
                &mut total_stats,
                document,
                palette,
                options,
                cmyk_xform.as_ref(),
                &auto_sized_bounds,
            );
        }
        spread_frame_spans.push(frame_spans);
    }

    // Story pass: layout text into its hosting frame's page.
    //
    // The font table pre-resolves every distinct (family, style)
    // referenced anywhere in the document so each paragraph picks up
    // the right TTF without re-querying the resolver. Per paragraph
    // we still build `Face`s on demand — `rustybuzz::Face::from_slice`
    // is cheap (parses font tables, no allocation churn).
    // Group transparency pass: bracket every group's emitted frame
    // range with `BeginBlendGroup` / `EndBlendGroup` whenever the
    // group's `<TransparencySetting>` has non-default values. Runs
    // *before* the story pass so text glyphs added later don't fall
    // inside the wrong bracket (per-text-frame brackets land later
    // via `apply_blend_groups`). Each spread's groups are resolved
    // against the per-frame command spans recorded above.
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let Some(spans) = spread_frame_spans.get(spread_idx) else {
            continue;
        };
        crate::module::group_pass(&parsed.spread, spans, &mut pages);
    }

    // Perf-FontTable — reuse the caller's pre-built table when
    // provided; otherwise build a fresh one for this call. The
    // `owned_font_table` binding holds the local value's storage
    // for the duration of the build when we fall through to the
    // None branch (its address is stable in that scope).
    let owned_font_table: Option<FontTable> = match options.pre_built_font_table {
        Some(_) => None,
        None => Some(FontTable::build(document, options)),
    };
    let font_table: &FontTable = options
        .pre_built_font_table
        .unwrap_or_else(|| owned_font_table.as_ref().expect("set on None branch"));
    // One hyphenator per render. We currently only build English-US;
    // the document's `AppliedLanguage` is honoured via the cascade,
    // but unrecognised values fall back to this dictionary so we
    // always have *some* hyphenation when a paragraph requests it.
    // Multi-language docs will grow this into a HashMap keyed by
    // resolved language string.
    let hyphenator = paged_text::Hyphenator::for_language(paged_text::Language::EnglishUS);

    // Per-page wrap exclusion rectangles (spread coords, expanded by
    // the wrap's offsets). Only items with TextWrapMode != "None"
    // contribute. Used by StoryEmitter::new to shrink the head text
    // frame's effective column width and shift its origin past any
    // intruding shape.
    let wrap_rects_per_page =
        collect_wrap_rects_per_page(document, &spread_page_ranges, &auto_sized_bounds);

    // Master-story pass: emit each master text frame's hosted story
    // (page-number footers, running headers) per body page that
    // references the master. The frame copies stashed during the
    // master overlay pass already carry the dx/dy translation from
    // master-spread coords to live spread coords, so a single-frame
    // chain is enough for the StoryEmitter.
    //
    // Per-page emission is what makes <?ACE 18?> resolve to the live
    // page number — pipeline.rs::emit_paragraph reads chain_pages[
    // frame_idx] and substitutes AUTO_PAGE_NUMBER_MARKER with that
    // body page's index. Run before the body-story pass so master
    // content sits below body content; future work to hard-enforce
    // z-order (rather than rely on display-list append order) should
    // tag these commands as "master layer" if/when we add layering.
    for (page_idx, master_frame) in &master_text_emissions {
        let Some(story_id) = master_frame.parent_story.as_deref() else {
            continue;
        };
        // When the body spreads carry their own frame for this same
        // story, the body has overridden the master placeholder (IDML
        // "Override Master Page Items"). The body-story pass below
        // will emit it — skipping here avoids the doubled header you
        // get when both copies render on top of each other.
        if !document.frame_chain(story_id).is_empty() {
            continue;
        }
        let Some(parsed) = document.stories.iter().find(|s| s.self_id == story_id) else {
            continue;
        };

        // Perf-MasterText — try the cache before running the emit.
        // Key is (master_frame_self_id, page_idx). On hit we splice
        // the cached delta into the page's display list, renumbering
        // path-ids relative to the page's current path-buffer size.
        // The cache is populated below on emit-miss; structural
        // mutations clear it via `CanvasModel::apply_operation`.
        let cache_key = master_frame
            .self_id
            .as_deref()
            .map(|id| (id.to_string(), *page_idx));
        if let (Some(ref key), Some(rc)) = (&cache_key, options.master_text_emit_cache) {
            if let Some(delta) = rc.borrow().get(key) {
                splice_master_text_delta(&mut pages[*page_idx].list, delta);
                continue;
            }
        }

        // Snapshot path-buffer + commands + side-effect pools BEFORE
        // emit so the post-emit extraction can compute deltas.
        let path_base = pages[*page_idx].list.paths.len();
        let cmd_base = pages[*page_idx].list.commands.len();
        let grad_base = pages[*page_idx].list.gradients.len();
        let rad_grad_base = pages[*page_idx].list.radial_gradients.len();
        let image_base = pages[*page_idx].list.images.len();

        let chain: Vec<&TextFrame> = vec![master_frame];
        let chain_pages: Vec<usize> = vec![*page_idx];
        let head_wrap_rects: &[WrapShape] = &[];
        let chain_wrap_rects: Vec<&[WrapShape]> = vec![&[]];
        let mut emitter = StoryEmitter::new(
            document,
            options,
            palette,
            cmyk_xform.as_ref(),
            font_table,
            chain,
            chain_pages,
            &page_labels,
            Some(&hyphenator),
            head_wrap_rects,
            chain_wrap_rects,
        )
        .with_optical_margin(
            parsed.story.optical_margin_alignment,
            parsed.story.optical_margin_size,
        )
        .with_story_id(&parsed.self_id)
        .with_page_count(total_page_count)
        .with_page_index_map(&page_index_map);
        for paragraph in &parsed.story.paragraphs {
            emitter.emit_paragraph(paragraph, &mut pages, &mut total_stats);
        }
        emitter.apply_vertical_justification(&mut pages);
        emitter.apply_polygon_clip(&mut pages);
        emitter.apply_blend_groups(&mut pages);
        let anchored_q = emitter.take_anchored_image_queue();
        let new_breaks = emitter.take_breaks();
        let new_diags = emitter.take_diagnostics();
        anchored_image_queue.extend(anchored_q.iter().cloned());
        breaks.extend(new_breaks.iter().cloned());
        emit_diagnostics.extend(new_diags.iter().cloned());

        // Perf-MasterText — capture the delta if the emit didn't
        // touch the gradient / image / anchored / breaks side
        // channels (the common case for footers + running headers,
        // which are pure text with solid paints). Skipping the
        // cache on the uncacheable cases keeps the splice path
        // pure-path; gradient/image renumbering is a follow-up.
        let list = &pages[*page_idx].list;
        let uncacheable = list.gradients.len() != grad_base
            || list.radial_gradients.len() != rad_grad_base
            || list.images.len() != image_base
            || !anchored_q.is_empty()
            || !new_breaks.is_empty()
            || !new_diags.is_empty();
        if let (Some(ref key), Some(rc), false) =
            (&cache_key, options.master_text_emit_cache, uncacheable)
        {
            let new_paths: Vec<paged_compose::PathData> =
                list.paths.slice(path_base, list.paths.len()).to_vec();
            let mut new_commands: Vec<paged_compose::DisplayCommand> =
                list.commands[cmd_base..list.commands.len()].to_vec();
            // Rebase path-ids in the captured commands so they're
            // relative to the start of the captured paths slice.
            for cmd in new_commands.iter_mut() {
                rebase_path_ids(cmd, -(path_base as i64));
            }
            rc.borrow_mut().insert(
                key.clone(),
                MasterTextEmitDelta {
                    paths: new_paths,
                    commands: new_commands,
                },
            );
        }
    }

    // Text-on-path pass: walk every spread's shapes and emit any
    // attached `<TextPath>` along the host's tessellated curve.
    // Stories that flow only via TextPath have an empty
    // `frame_chain`, so the body-story pass below skips them — this
    // pass is what gives those stories their visible glyphs.
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        let range = spread_page_ranges[spread_idx].clone();
        let local_geoms = &page_geometries[range.clone()];
        for poly in &spread.polygons {
            if poly.text_paths.is_empty() {
                continue;
            }
            if !layer_visible(poly.item_layer.as_deref()) {
                continue;
            }
            let spread_bounds = transform_bounds(poly.bounds, poly.item_transform);
            let local_idx = page_for_frame(&spread_bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            for tp in &poly.text_paths {
                emit_text_path_into(
                    &mut pages[page_idx],
                    tp,
                    &poly.anchors,
                    poly.item_transform,
                    document,
                    options,
                    palette,
                    cmyk_xform.as_ref(),
                    font_table,
                );
            }
        }
        for rect in &spread.rectangles {
            if rect.text_paths.is_empty() {
                continue;
            }
            if !layer_visible(rect.item_layer.as_deref()) {
                continue;
            }
            let spread_bounds = transform_bounds(rect.bounds, rect.item_transform);
            let local_idx = page_for_frame(&spread_bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            // Rectangles serialise their corners as PathPointType
            // anchors only when they carry custom geometry; the
            // simple-rect case stores `GeometricBounds` only. Build
            // a 4-corner anchor list as a fallback so straight-edge
            // rect-hosted TextPaths still flow.
            let synth_corners: Vec<PathAnchor>;
            let anchors: &[PathAnchor] = {
                synth_corners = vec![
                    PathAnchor {
                        anchor: (rect.bounds.left, rect.bounds.top),
                        left: (rect.bounds.left, rect.bounds.top),
                        right: (rect.bounds.left, rect.bounds.top),
                    },
                    PathAnchor {
                        anchor: (rect.bounds.right, rect.bounds.top),
                        left: (rect.bounds.right, rect.bounds.top),
                        right: (rect.bounds.right, rect.bounds.top),
                    },
                    PathAnchor {
                        anchor: (rect.bounds.right, rect.bounds.bottom),
                        left: (rect.bounds.right, rect.bounds.bottom),
                        right: (rect.bounds.right, rect.bounds.bottom),
                    },
                    PathAnchor {
                        anchor: (rect.bounds.left, rect.bounds.bottom),
                        left: (rect.bounds.left, rect.bounds.bottom),
                        right: (rect.bounds.left, rect.bounds.bottom),
                    },
                ];
                &synth_corners
            };
            for tp in &rect.text_paths {
                emit_text_path_into(
                    &mut pages[page_idx],
                    tp,
                    anchors,
                    rect.item_transform,
                    document,
                    options,
                    palette,
                    cmyk_xform.as_ref(),
                    font_table,
                );
            }
        }
        for line in &spread.graphic_lines {
            if line.text_paths.is_empty() {
                continue;
            }
            if !layer_visible(line.item_layer.as_deref()) {
                continue;
            }
            let spread_bounds = transform_bounds(line.bounds, line.item_transform);
            let local_idx = page_for_frame(&spread_bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            // GraphicLines without anchors fall back to the bounds
            // diagonal endpoints — matches the line-renderer's
            // fallback geometry.
            let synth_endpoints: Vec<PathAnchor>;
            let anchors: &[PathAnchor] = if !line.anchors.is_empty() {
                line.anchors.as_slice()
            } else {
                synth_endpoints = vec![
                    PathAnchor {
                        anchor: (line.bounds.left, line.bounds.top),
                        left: (line.bounds.left, line.bounds.top),
                        right: (line.bounds.left, line.bounds.top),
                    },
                    PathAnchor {
                        anchor: (line.bounds.right, line.bounds.bottom),
                        left: (line.bounds.right, line.bounds.bottom),
                        right: (line.bounds.right, line.bounds.bottom),
                    },
                ];
                &synth_endpoints
            };
            for tp in &line.text_paths {
                emit_text_path_into(
                    &mut pages[page_idx],
                    tp,
                    anchors,
                    line.item_transform,
                    document,
                    options,
                    palette,
                    cmyk_xform.as_ref(),
                    font_table,
                );
            }
        }
    }

    // W1.22 (engine gap 22) — cross-story numbering ledger. When the
    // document declares at least one `<NumberingList>` with
    // `ContinueNumbersAcrossStories="true"`, paragraphs sharing that
    // list keep counting across stories. The ledger (list id → last
    // counter) lives here, outside the per-story loop, and is threaded
    // into each story's emitter. Built lazily — `None`/empty when no
    // such list exists, so the overwhelming-common case pays nothing
    // and the per-story counter owns everything as before.
    //
    // Determinism: stories emit in `document.stories` order (designmap
    // story order); the ledger is updated in that single forward walk.
    // The footnote-reservation re-emit loop snapshots + restores the
    // ledger around its passes (below) so a re-emit doesn't double the
    // count, and the body-story emit cache is disabled for stories that
    // touch a continue-across-stories list (a cache replay wouldn't
    // re-run the ledger update). Same source bytes ⇒ same numbers.
    let has_continue_across_stories = document
        .styles
        .numbering_lists
        .values()
        .any(|d| d.continue_across_stories == Some(true));
    let cross_story_numbering: Option<std::cell::RefCell<HashMap<String, u32>>> =
        has_continue_across_stories.then(|| std::cell::RefCell::new(HashMap::new()));

    for parsed in &document.stories {
        total_stats.stories += 1;
        let chain = document.frame_chain(&parsed.self_id);
        if chain.is_empty() {
            continue;
        }
        // Perf-BodyStory — try the cache before running the emit.
        // Signature hashes the chain's frames (bounds + transforms)
        // and the wrap_rects_per_page entries for every page the
        // chain touches. Stories whose chain doesn't include the
        // dragged frame AND whose chain pages don't see a wrap
        // change keep hitting through the drag. Capture happens
        // post-emit (and post-all-post-passes) so the cached
        // commands are fully baked.
        let chain_pages_pre: Vec<usize> = chain
            .iter()
            .map(|f| {
                f.self_id
                    .as_deref()
                    .and_then(|id| frame_to_page.get(id).copied())
                    .unwrap_or(0)
            })
            .collect();
        // W1.22 — a continue-across-stories list makes a story's
        // numbering depend on the documents-order prefix of stories,
        // not just its own frames/wrap; a cached splice replay wouldn't
        // re-run the ledger update, so disable the cache document-wide
        // when such a list exists (conservative, like the
        // gradient/image-pool rule below).
        let cache_key: Option<(String, u64)> =
            if options.body_story_emit_cache.is_some() && cross_story_numbering.is_none() {
                Some((
                    parsed.self_id.clone(),
                    body_story_signature(&chain, &chain_pages_pre, &wrap_rects_per_page),
                ))
            } else {
                None
            };
        if let (Some(ref key), Some(rc)) = (&cache_key, options.body_story_emit_cache) {
            if let Some(delta) = rc.borrow().get(key) {
                // Defense in depth — the signature includes the chain's
                // page indices, so a stale-index hit should be
                // impossible; but the cache is long-lived interactive
                // state, and splicing past pages.len() is a hard panic
                // inside the worker (mutate() never resolves). If any
                // captured index is out of range, treat the entry as a
                // miss and re-emit fresh.
                if delta.per_page.iter().all(|(idx, _)| *idx < pages.len()) {
                    for (page_idx, page_delta) in &delta.per_page {
                        splice_body_story_page_delta(&mut pages[*page_idx], page_delta);
                    }
                    anchored_image_queue.extend(delta.anchored.iter().cloned());
                    breaks.extend(delta.breaks.iter().cloned());
                    continue;
                }
            }
        }
        // Snapshot per-page pool sizes BEFORE this story emits so
        // post-emit extraction can compute per-page deltas. Tracks
        // path / command / gradient / image pool sizes plus the
        // story_layout + footnotes vec lengths — the latter two
        // are extended by emit_paragraph and must be replayed on
        // cache hit so caret / hit-test / footnote pools match a
        // from-scratch emit.
        let pre_snapshot: Vec<(usize, usize, usize, usize, usize, usize, usize)> = pages
            .iter()
            .map(|p| {
                (
                    p.list.paths.len(),
                    p.list.commands.len(),
                    p.list.gradients.len(),
                    p.list.radial_gradients.len(),
                    p.list.images.len(),
                    p.story_layout.len(),
                    p.footnotes.len(),
                )
            })
            .collect();
        // TOC swap-in: if the head text frame carries
        // `AppliedTOCStyle="TOCStyle/<id>"`, replace the story's
        // own paragraphs with the resolver's output for that TOC
        // style. Real-world unresolved TOC stories carry a single
        // placeholder paragraph; the frame's `AppliedTOCStyle`
        // attribute is what binds it to a `<TOCStyle>` in
        // `Resources/Styles.xml`. After the swap the synthetic
        // paragraphs go through the standard paragraph-emission
        // path so they get full shaping, tab handling, applied
        // paragraph-style cascade resolution, etc.
        let toc_paragraphs: Option<Vec<paged_parse::Paragraph>> = chain
            .first()
            .and_then(|f| f.applied_toc_style.as_deref())
            .and_then(|toc_id| document.styles.toc_styles.get(toc_id))
            .map(|toc| build_toc_paragraphs(document, toc, &page_labels));
        let chain_pages = chain_pages_pre.clone();
        let head_page_idx = chain_pages[0];
        let head_wrap_rects: &[WrapShape] = wrap_rects_per_page
            .get(head_page_idx)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        // Per-chain wrap rects so threaded frames inherit per-line
        // wrap. Each chain index maps to its frame's page's
        // exclusion list.
        let chain_wrap_rects: Vec<&[WrapShape]> = chain_pages
            .iter()
            .map(|&p| {
                wrap_rects_per_page
                    .get(p)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[])
            })
            .collect();
        // Phase 7 — clone chain refs + page indices BEFORE moving them
        // into the StoryEmitter so the vertical-writing post-pass can
        // resolve the host frame for each page.
        let chain_for_post = chain.clone();
        let chain_pages_for_post = chain_pages.clone();
        let is_vertical = matches!(
            parsed.story.story_direction,
            Some(paged_parse::story::StoryDirection::VerticalWritingDirection)
        );

        // W1.7 — footnote space reservation. Footnotes are discovered
        // while composing the very text that references them, so the
        // pool's height isn't known until the body has been laid out —
        // a chicken-and-egg the standard fix resolves by COMPOSING,
        // MEASURING, then RE-COMPOSING with the bottom of each frame's
        // text area held back by the measured pool height.
        //
        // Convergence: each iteration emits the whole story, measures
        // every frame's pool, and sets `reserved[frame]` to that pool's
        // height. A taller pool pushes the last body lines (and any
        // footnote they reference) into the next frame, which can shrink
        // THIS frame's pool — so we iterate to a fixpoint. In practice
        // the reservation only ever grows or holds within a frame, so
        // the loop settles in 1–2 passes; we cap at
        // `MAX_FOOTNOTE_RESERVE_PASSES` and accept whatever the last
        // pass produced (the pool is still drawn as an overlay, so a
        // non-converged page degrades to today's behaviour rather than
        // dropping content). A footnote whose reference moved to the
        // next frame simply moves WITH it — the per-page pool the
        // post-pass draws follows the capture, matching InDesign's
        // "footnote travels with its reference" rule for the stable
        // subset; cross-frame *splitting* of a single oversized
        // footnote is still deferred (reported via FootnoteOverflow).
        let pre_reset = snapshot_body_story_reset(&pages);
        let pre_total_stats = total_stats;
        // Map each chain frame to the (page, rect) key it captures
        // footnotes under, so a measured pool routes back to the frame
        // whose text area must shrink.
        let frame_host_keys: Vec<(usize, i32, i32, i32, i32)> = chain_for_post
            .iter()
            .zip(chain_pages_for_post.iter())
            .map(|(f, &p)| footnote_host_key_for_frame(f, p, &pages))
            .collect();
        let mut reserved_64: Vec<i32> = vec![0; chain_for_post.len()];

        // Captured from the FINAL emit pass for the cache + side
        // channels below.
        let mut new_anchored: Vec<AnchoredImageEmit> = Vec::new();
        let mut new_breaks: Vec<BreakRecord> = Vec::new();
        let mut new_diags: Vec<Diagnostic> = Vec::new();

        // W1.22 — snapshot the cross-story numbering ledger so a
        // footnote-reservation re-emit (pass > 0) restarts this story's
        // numbering from the same pre-story state instead of counting
        // on top of the previous pass's writes.
        let cross_story_pre: Option<HashMap<String, u32>> =
            cross_story_numbering.as_ref().map(|c| c.borrow().clone());

        for pass in 0..MAX_FOOTNOTE_RESERVE_PASSES {
            // Re-emit passes start from the pre-story snapshot so the
            // page accumulates exactly one story's worth of commands.
            if pass > 0 {
                rollback_body_story(&mut pages, &pre_reset);
                total_stats = pre_total_stats;
                if let (Some(c), Some(pre)) =
                    (cross_story_numbering.as_ref(), cross_story_pre.as_ref())
                {
                    *c.borrow_mut() = pre.clone();
                }
            }
            let mut emitter = StoryEmitter::new(
                document,
                options,
                palette,
                cmyk_xform.as_ref(),
                font_table,
                chain_for_post.clone(),
                chain_pages_for_post.clone(),
                &page_labels,
                Some(&hyphenator),
                head_wrap_rects,
                chain_wrap_rects.clone(),
            )
            .with_optical_margin(
                parsed.story.optical_margin_alignment,
                parsed.story.optical_margin_size,
            )
            .with_story_id(&parsed.self_id)
            .with_page_count(total_page_count)
            .with_page_index_map(&page_index_map)
            .with_footnote_reservation(&reserved_64);
            // W1.22 — thread the cross-story numbering ledger when one
            // exists (only built for documents with a continue-across-
            // stories list).
            if let Some(ref ledger) = cross_story_numbering {
                emitter = emitter.with_cross_story_numbering(ledger);
            }
            // Phase 7 — capture each page's command count BEFORE this
            // story's emit so a post-pass can rotate the story's commands
            // when StoryDirection="VerticalWritingDirection".
            let pre_story_cmd_counts: Vec<usize> =
                pages.iter().map(|p| p.list.commands.len()).collect();
            if let Some(paragraphs) = toc_paragraphs.as_ref() {
                for paragraph in paragraphs {
                    emitter.emit_paragraph(paragraph, &mut pages, &mut total_stats);
                }
            } else {
                for paragraph in &parsed.story.paragraphs {
                    emitter.emit_paragraph(paragraph, &mut pages, &mut total_stats);
                }
            }
            emitter.apply_vertical_justification(&mut pages);
            emitter.apply_polygon_clip(&mut pages);
            emitter.apply_blend_groups(&mut pages);
            // Phase 7 — vertical writing post-rotation. When the source
            // story declares `StoryDirection="VerticalWritingDirection"`,
            // rotate every command this story emitted by 90° CW around
            // each host frame's top-left corner, then translate right
            // by the frame's width. This maps the horizontal layout
            // (lines top-to-bottom, chars left-to-right within a line)
            // to CJK vertical convention (columns right-to-left, chars
            // top-to-bottom within a column). Latin glyphs render
            // sideways — full per-glyph upright counter-rotation
            // (matched to InDesign's `<RotateSingleByteCharacters>` flag)
            // is queued.
            if is_vertical {
                apply_vertical_writing_rotation(
                    &mut pages,
                    &pre_story_cmd_counts,
                    &chain_for_post,
                    &chain_pages_for_post,
                );
            }
            new_anchored = emitter.take_anchored_image_queue();
            new_breaks = emitter.take_breaks();
            new_diags = emitter.take_diagnostics();

            // Measure each frame's footnote pool and fold it into the
            // reservation. Vertical-writing stories lay the pool out in
            // horizontal page space too, so the measure is valid there;
            // we skip the reserve loop only when no footnotes captured.
            let any_footnotes = pages.iter().any(|p| !p.footnotes.is_empty());
            if !any_footnotes {
                break;
            }
            let pool_heights = measure_footnote_pools(
                &pages,
                options,
                document,
                font_table,
                palette,
                cmyk_xform.as_ref(),
            );
            let mut next_reserved = vec![0i32; reserved_64.len()];
            for (frame_idx, key) in frame_host_keys.iter().enumerate() {
                if let Some(h_pt) = pool_heights.get(key) {
                    next_reserved[frame_idx] =
                        (*h_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;
                }
            }
            if next_reserved == reserved_64 || pass + 1 == MAX_FOOTNOTE_RESERVE_PASSES {
                // Fixpoint, or the bail cap — accept this pass. The pool
                // emit post-pass paints below the reserved band.
                reserved_64 = next_reserved;
                break;
            }
            reserved_64 = next_reserved;
        }

        anchored_image_queue.extend(new_anchored.iter().cloned());
        breaks.extend(new_breaks.iter().cloned());
        emit_diagnostics.extend(new_diags.iter().cloned());

        // Perf-BodyStory — capture the per-page delta if the emit
        // didn't touch gradient/image pools. Same conservative
        // policy as master_text: skip caching when gradient or
        // image entries were added, since the cached splice path
        // only renumbers path-ids.
        if let (Some(ref key), Some(rc)) = (&cache_key, options.body_story_emit_cache) {
            // Diagnostics (overset) ride the emit channel, not the
            // cached delta — a story that produced any is left
            // uncacheable so a future hit re-emits and re-reports.
            let mut uncacheable = !new_diags.is_empty();
            let mut per_page: Vec<(usize, BodyStoryPageDelta)> = Vec::new();
            for (page_idx, snap) in pre_snapshot.iter().enumerate() {
                let page = &pages[page_idx];
                let list = &page.list;
                if list.gradients.len() != snap.2
                    || list.radial_gradients.len() != snap.3
                    || list.images.len() != snap.4
                {
                    uncacheable = true;
                    break;
                }
                let grew_list = list.paths.len() > snap.0 || list.commands.len() > snap.1;
                let grew_layout = page.story_layout.len() > snap.5;
                let grew_footnotes = page.footnotes.len() > snap.6;
                if grew_list || grew_layout || grew_footnotes {
                    let new_paths: Vec<paged_compose::PathData> =
                        list.paths.slice(snap.0, list.paths.len()).to_vec();
                    let mut new_commands: Vec<paged_compose::DisplayCommand> =
                        list.commands[snap.1..list.commands.len()].to_vec();
                    for cmd in new_commands.iter_mut() {
                        rebase_path_ids(cmd, -(snap.0 as i64));
                    }
                    let new_story_layout: Vec<LineLayout> = page.story_layout[snap.5..].to_vec();
                    let new_footnotes: Vec<EmittedFootnote> = page.footnotes[snap.6..].to_vec();
                    per_page.push((
                        page_idx,
                        BodyStoryPageDelta {
                            paths: new_paths,
                            commands: new_commands,
                            story_layout: new_story_layout,
                            footnotes: new_footnotes,
                        },
                    ));
                }
            }
            if !uncacheable {
                rc.borrow_mut().insert(
                    key.clone(),
                    BodyStoryEmissionDelta {
                        per_page,
                        anchored: new_anchored,
                        breaks: new_breaks,
                    },
                );
            }
        }
    }

    // Anchored-rectangle image post-pass. Each entry was captured
    // during the story pass after placement resolution; replay
    // through `emit_rectangle_image` so anchored images share the
    // same per-page ImageId cache + renderer-scoped decoded-image
    // cache as spread-level Rectangles. Drains both master + body
    // captures (master frames currently never carry anchored
    // images, but the queue is unified for symmetry).
    for entry in anchored_image_queue {
        emit_anchored_rect_image(
            &mut pages[entry.target_page],
            &entry.af,
            entry.place_x,
            entry.place_y,
            entry.width,
            entry.height,
            options,
            &mut page_image_caches[entry.target_page],
            decoded_image_cache,
        );
    }

    total_stats.decoded_images = decoded_image_cache.len();

    // Phase 5 — footnote pool post-pass. For each page that captured
    // footnotes during the story emit, lay out the bodies at the
    // bottom of the host frame's content area. Bodies stack
    // upward from the frame's bottom; per-page running numbers
    // prefix each body. Overlay rather than reflow today —
    // body content remains where it was, and footnotes can
    // overlap it if the host frame is fully populated.
    // Cross-page overflow (a footnote pool taller than the host
    // frame) and anchor-character superscript substitution are
    // queued follow-ups.
    let footnote_options = options.clone();
    emit_footnote_pools(
        &mut pages,
        font_table,
        &footnote_options,
        document,
        palette,
        cmyk_xform.as_ref(),
    );

    // Aggregate diagnostics: the per-story emit channel (overset,
    // section fallback) already carries page indices; the per-page
    // collectors (missing image, footnote overflow) get their flat
    // page index backfilled here.
    let mut diagnostics = RenderDiagnostics {
        items: emit_diagnostics,
    };
    for (page_idx, p) in pages.iter().enumerate() {
        for d in &p.diagnostics {
            let mut d = d.clone();
            if d.page_index.is_none() {
                d.page_index = Some(page_idx);
            }
            diagnostics.push(d);
        }
    }

    Ok(BuiltDocument {
        pages,
        stats: total_stats,
        breaks,
        diagnostics,
    })
}

/// Emits a story's paragraphs into the page list, flowing across
/// the frame chain on overflow and applying TextFramePreference
/// vertical justification once the story finishes.
///
/// Carries all the per-story mutable state the build_document loop
/// previously held inline:
///  - frame_idx + y_cursor: which frame is currently filling and
///    where the next baseline goes inside it.
///  - frame_cmd_ranges + frame_max_baseline_64: tracked during
///    emission so the post-story vertical-justification shift can
///    target this story's commands without touching frame outlines.
struct StoryEmitter<'a> {
    document: &'a Document,
    options: &'a PipelineOptions<'a>,
    palette: &'a Graphic,
    /// Reserved for the upcoming CMYK text-fill path. The current
    /// per-glyph paint picker resolves through `palette` directly.
    #[allow(dead_code)]
    cmyk_xform: Option<&'a paged_color::IccTransform>,
    font_table: &'a FontTable,
    chain: Vec<&'a TextFrame>,
    chain_pages: Vec<usize>,
    /// User-visible page labels indexed by flat body-page idx (parallel
    /// to `pages`). The auto-page-number marker substitutes
    /// `page_labels[chain_pages[frame_idx]]`; ACE 19 looks one slot
    /// further ahead. Owned by the document, not the emitter.
    page_labels: &'a [String],
    /// Pre-built hyphenator for the document's primary language.
    /// `None` ⇒ the document opts out of hyphenation entirely (the
    /// composer skips the language-specific pattern lookup).
    hyphenator: Option<&'a paged_text::Hyphenator>,
    column_width_pt: Option<f32>,
    /// Inner-coord x-shift to apply to the head frame's text
    /// origin when an obstacle on the page intrudes from the left
    /// of the frame for the *whole* frame's height. Zero unless
    /// wrap rectangles overlap the head frame.
    column_x_shift_pt: f32,
    /// Spread-coord wrap exclusion rectangles for the head frame's
    /// page. Per-paragraph wrap (per-line column carving) reads
    /// these and computes a `column_widths` slice + per-line
    /// glyph x-shifts so body text flows around an island
    /// obstacle (the chairman page's pull quote, for example).
    /// Superseded by `chain_wrap_rects[0]` for the per-line walk;
    /// retained alongside `head_frame_spread` for callers that
    /// want the head's wraps without indexing.
    #[allow(dead_code)]
    head_wrap_rects: Vec<WrapShape>,
    /// Spread-coord bounds of the head frame, cached so the
    /// per-paragraph wrap pass doesn't recompute per paragraph.
    /// Currently superseded by `chain_spread_bounds[0]` for the
    /// per-line walk; retained for future per-frame optimisations
    /// that read the head's bounds without indexing.
    #[allow(dead_code)]
    head_frame_spread: paged_parse::Bounds,
    /// Spread-coord wrap exclusion rectangles per chain index — the
    /// threaded-frame extension of `head_wrap_rects`. Each chain
    /// index `i` carries the wrap rectangles on chain[i]'s page.
    /// Used by `build_perline_wrap_widths` so overflow lines that
    /// land in chain[1+] get the right exclusions for that frame's
    /// page.
    chain_wrap_rects: Vec<Vec<WrapShape>>,
    /// Spread-coord bounds for every frame in the chain. Same
    /// motivation as `chain_wrap_rects`: per-frame per-line wrap
    /// needs each frame's spread rect.
    chain_spread_bounds: Vec<paged_parse::Bounds>,
    frame_idx: usize,
    y_cursor: i32,
    /// Leading (in 1/64 pt) of the most recently placed line (or
    /// empty paragraph). Adobe positions each baseline at
    /// `prev_baseline + leading(THIS line)`; our `y_cursor` instead
    /// tracks `prev_baseline + leading(THAT line)`. When the new
    /// line/paragraph has a different leading (mixed-size flow:
    /// 12pt body → 24pt heading, or vice versa), the next baseline
    /// needs to rewind by `prev_line_height_64` and re-apply with
    /// the new line's leading. We record the most recent advance so
    /// the next placement can do that adjustment. None at frame
    /// start (no baseline yet — `first_baseline_for_frame` will be
    /// used instead).
    prev_line_height_64: Option<i32>,
    frame_cmd_ranges: Vec<Option<(usize, usize)>>,
    frame_max_baseline_64: Vec<i32>,
    /// W1.7 footnote space reservation — per chain-frame height (in
    /// 1/64 pt) to hold back from the bottom of the frame's text area
    /// for the footnote pool. Body text overflows to the next frame
    /// once a line's baseline crosses `frame_height - reserved`, so the
    /// pool drawn in the post-pass sits *below* the last body line
    /// instead of overlapping it. All zero on the first (measuring)
    /// emit; the body-story loop fills it from the measured pool
    /// heights and re-emits to a fixpoint. See `emit_footnote_pools`
    /// and the reservation loop in `build_document`.
    reserved_footnote_64: Vec<i32>,
    /// Per-frame list of `(cmd_start, cmd_end)` slices, one entry
    /// per paragraph that contributed glyph commands to the frame,
    /// in emission order. A paragraph that flows across N frames
    /// contributes one entry to each of those frames'
    /// `paragraph_cmd_ranges` lists. Drives `JustifyAlign` vertical
    /// justification, which distributes the per-frame slack as
    /// extra inter-paragraph space.
    paragraph_cmd_ranges: Vec<Vec<(usize, usize)>>,
    /// Counter for `NumberedList` paragraphs in this story. The
    /// renderer treats the count as a sticky story-level value
    /// across paragraphs of different kinds; the implicit-reset
    /// fires only when entering a `NumberedList` paragraph whose
    /// prior neighbour wasn't also numbered (and the paragraph
    /// hasn't explicitly opted into `NumberingContinue`). 0 is the
    /// initial value; the first numbered paragraph either lifts it
    /// to its `NumberingStartAt` or to 1.
    numbered_counter: u32,
    /// Tracks whether the previous paragraph was a `NumberedList`.
    /// Drives the implicit-reset decision for the next paragraph:
    /// a `NumberedList` paragraph that follows a non-numbered one
    /// resets the counter to 0 (so the first increment lands at 1)
    /// unless the paragraph carries `NumberingContinue="true"` or
    /// `NumberingStartAt`.
    prev_was_numbered: bool,
    /// W1.22 (engine gap 22) — document-level numbering ledger keyed
    /// by `<NumberingList>` self id, shared across every story's
    /// emitter so a list with `ContinueNumbersAcrossStories="true"`
    /// keeps its counter as the body-story loop walks stories in
    /// document order. `None` when the document declares no
    /// continue-across-stories list (the common case) — the per-story
    /// `numbered_counter` then owns everything as before. See
    /// `numbering::list_prefix` and `build_document`'s ledger.
    ///
    /// Determinism: the ledger is updated in the FROM-SCRATCH emit
    /// order, which is the `for parsed in &document.stories` walk =
    /// designmap story order. Footnote-reservation re-emit passes and
    /// the body-story cache are made ledger-safe by `build_document`
    /// (snapshot/restore around re-emits; cross-story lists disable
    /// the cache), so a given source always yields the same numbers.
    cross_story_numbering: Option<&'a std::cell::RefCell<HashMap<String, u32>>>,
    /// `<StoryPreference OpticalMarginAlignment>` flag. When true,
    /// the per-line emit pass nudges the leftmost / rightmost glyph
    /// of each line outward per `paged_text::optical_margin_offset`.
    optical_margin_alignment: bool,
    /// `<StoryPreference OpticalMarginSize>` (point size). Bounds the
    /// hang for glyphs smaller than this size; ignored when
    /// `optical_margin_alignment` is false.
    optical_margin_size_pt: f32,
    /// How many anchored-frame story recursions deep this emitter is.
    /// 0 for the top-level body / master pass; 1+ for an emitter
    /// constructed by `emit_anchored_textframe_story`. Bounded at
    /// `MAX_ANCHORED_STORY_RECURSION` so a malformed document with an
    /// anchored TextFrame referencing its own host story can't blow
    /// the stack.
    anchored_recursion_depth: u32,
    /// Image-bearing anchored frames captured during emission so the
    /// caller can replay them through `emit_rectangle_image` once the
    /// story pass completes. Image emission needs the per-page
    /// `ImageId` cache + decoded-image cache that live in
    /// `build_document`'s scope, outside StoryEmitter — collecting the
    /// already-resolved (target_page, place_x, place_y, AnchoredFrame
    /// clone) tuples here lets the post-pass run with the caches in
    /// hand without re-doing placement.
    anchored_image_queue: Vec<AnchoredImageEmit>,
    /// Track 2: per-line records collected when
    /// `options.collect_breaks` is set. Drained by `take_breaks` once
    /// the story finishes emitting.
    breaks: Vec<BreakRecord>,
    /// Track 2: identifies which story this emitter is processing.
    /// Set by `StoryEmitter::with_story_id` before emit; included in
    /// every pushed `BreakRecord`. Empty string when collection isn't
    /// enabled.
    current_story_id: String,
    /// Track 2: monotonically incremented as `emit_paragraph` fires.
    /// Resets to 0 per emitter (i.e. per story).
    paragraph_idx: u32,
    /// Lossy-render signals collected during this story's emit (overset
    /// drop). Drained by `take_diagnostics` into the document-level
    /// collector, mirroring `breaks`. A non-empty drain marks the emit
    /// uncacheable so a body-story cache hit can't silently swallow it.
    diagnostics: Vec<Diagnostic>,
    /// Set once a story-overflow drop has been reported so the overset
    /// diagnostic fires once per story, not once per dropped line.
    overset_reported: bool,
    /// W1.4 — total body-page count, for `PageCountType` text-variable
    /// resolution. Set once per build (the same value for every
    /// emitter); 0 only before pages exist.
    page_count: usize,
    /// W1.4 — when true, the LineLayout capture also pushes
    /// [`paged_compose::LinkRegion`]s for runs tagged with
    /// `hyperlink_source`. Mirrors `options.collect_link_regions`;
    /// cached so the per-line path doesn't re-read options.
    collect_link_regions: bool,
    /// W1.4 — `<Page Self=...>` id → flat 0-based body-page index, for
    /// resolving hyperlink page destinations. `None` when link-region
    /// collection is off (the map isn't built). Owned by the build, not
    /// the emitter.
    page_index_map: Option<&'a HashMap<String, u32>>,
}

impl<'a> StoryEmitter<'a> {
    fn new(
        document: &'a Document,
        options: &'a PipelineOptions<'a>,
        palette: &'a Graphic,
        cmyk_xform: Option<&'a paged_color::IccTransform>,
        font_table: &'a FontTable,
        chain: Vec<&'a TextFrame>,
        chain_pages: Vec<usize>,
        page_labels: &'a [String],
        hyphenator: Option<&'a paged_text::Hyphenator>,
        head_wrap_rects: &[WrapShape],
        chain_wrap_rects: Vec<&[WrapShape]>,
    ) -> Self {
        // Head frame's L+R insets shrink the column width. Threaded
        // frames usually share the same insets; honouring per-frame
        // insets requires recomputing the column width when
        // crossing frame boundaries.
        let head_insets = chain[0].inset_spacing.unwrap_or([0.0; 4]);
        let head_frame_spread = transform_bounds(chain[0].bounds, chain[0].item_transform);
        let (mut shrink_left, mut shrink_right) = (0.0f32, 0.0f32);
        // Treat any wrap rectangle that overlaps the head frame's
        // vertical extent as a side exclusion: extend `shrink_left`
        // when the rect intrudes from the left, `shrink_right` when
        // from the right. This is the simplest of the IDML wrap
        // modes (BoundingBoxTextWrap, BothSides) and handles the
        // common "image to one side of body text" layout. True
        // per-line island wrap needs column-segment support in
        // compose_paragraph and is queued.
        let frame_height = head_frame_spread.height();
        for shape in head_wrap_rects {
            let w = shape.bounds;
            // Vertical overlap check.
            let v_overlap =
                w.bottom.min(head_frame_spread.bottom) - w.top.max(head_frame_spread.top);
            if v_overlap <= 0.0 {
                continue;
            }
            // Skip rects that fully cover the frame horizontally.
            if w.left <= head_frame_spread.left && w.right >= head_frame_spread.right {
                continue;
            }
            // Side-shrink is only correct when the obstacle spans
            // most of the frame vertically (sidebars, full-height
            // images). Smaller obstacles (pull quotes, inline
            // figures) need true per-line island wrap; shrinking
            // the whole column for them would collapse the body
            // text. Threshold: ≥ 80% vertical overlap.
            if frame_height > 0.0 && v_overlap < 0.8 * frame_height {
                continue;
            }
            let frame_cx = (head_frame_spread.left + head_frame_spread.right) * 0.5;
            let rect_cx = (w.left + w.right) * 0.5;
            if rect_cx < frame_cx {
                let new_left = w.right.max(head_frame_spread.left);
                shrink_left = shrink_left.max(new_left - head_frame_spread.left);
            } else {
                let new_right = w.left.min(head_frame_spread.right);
                shrink_right = shrink_right.max(head_frame_spread.right - new_right);
            }
        }

        // Use the head frame's *inner-coord* width for column sizing
        // so rotated TextFrames (90° sidebar labels, vertical
        // wordmarks) don't degenerate to a frame-height-sized column.
        // `transform_bounds` produces the spread-space AABB which
        // swaps width/height under a 90° ItemTransform; that's the
        // right input for wrap-obstacle / page-routing but the wrong
        // one for the rotation-invariant text column. The post-emit
        // pass at `frame_is_upright` later rotates the glyph commands
        // around the frame's spread top-left so they land along the
        // rotated axis.
        let raw_width = (chain[0].bounds.width() - head_insets[1] - head_insets[3]).max(0.0);
        let wrapped_width = (raw_width - shrink_left - shrink_right).max(0.0);
        // Q-02: when the head frame's AutoSizingType allows width
        // growth, the IDML authored an *undersized* column expecting
        // composition-time growth ("MAGAZINE" headline frame at
        // ~40-80pt expecting to grow to fit the actual headline).
        // Knuth-Plass at the authored width clips wrap output to
        // "MAG" / "MA-/GA-/ZINE". Override the column upward to an
        // estimate that fits the longest token in the story.
        //
        // Conservative estimator: take the longest WORD in the story,
        // approximate its width as
        //   point_size × char_count × 0.62
        // (an average-glyph advance ratio across realistic display
        // faces; 0.62 hits Inter Bold / Roboto Black / Source Serif
        // within ~10%). Multiply by a 1.1 slack factor. The renderer
        // doesn't measure here — that would require shape calls per
        // word + face resolution — but the estimate is correct enough
        // to unblock the wrap. Glyphs land where the actual shape
        // puts them at render time; the column is only the wrap
        // budget.
        //
        // Bound the override by the host page's width when known so
        // we don't shove headlines off-page on layouts where the
        // headline frame sits near the right edge.
        let column_width_pt = {
            let mut base = options.fallback_column_width_pt.or(Some(wrapped_width));
            if let Some(at) = chain[0].auto_sizing {
                if at.grows_width() {
                    let est = q02_estimate_auto_sizing_width(document, chain[0]);
                    let floor = chain[0].minimum_width_for_auto_sizing.unwrap_or(0.0);
                    let target = est.max(floor).max(wrapped_width);
                    if target > wrapped_width {
                        base = Some(target);
                    }
                }
            }
            base
        };
        let len = chain.len();
        let chain_spread_bounds: Vec<paged_parse::Bounds> = chain
            .iter()
            .map(|f| transform_bounds(f.bounds, f.item_transform))
            .collect();
        let chain_wrap_rects_owned: Vec<Vec<WrapShape>> =
            chain_wrap_rects.iter().map(|s| s.to_vec()).collect();
        Self {
            document,
            options,
            palette,
            cmyk_xform,
            font_table,
            chain,
            chain_pages,
            page_labels,
            hyphenator,
            column_width_pt,
            column_x_shift_pt: shrink_left,
            head_wrap_rects: head_wrap_rects.to_vec(),
            head_frame_spread,
            chain_wrap_rects: chain_wrap_rects_owned,
            chain_spread_bounds,
            frame_idx: 0,
            y_cursor: -1,
            prev_line_height_64: None,
            frame_cmd_ranges: vec![None; len],
            frame_max_baseline_64: vec![0; len],
            reserved_footnote_64: vec![0; len],
            paragraph_cmd_ranges: vec![Vec::new(); len],
            numbered_counter: 0,
            prev_was_numbered: false,
            cross_story_numbering: None,
            optical_margin_alignment: false,
            optical_margin_size_pt: 0.0,
            anchored_recursion_depth: 0,
            anchored_image_queue: Vec::new(),
            breaks: Vec::new(),
            current_story_id: String::new(),
            paragraph_idx: 0,
            diagnostics: Vec::new(),
            overset_reported: false,
            page_count: 0,
            collect_link_regions: options.collect_link_regions,
            page_index_map: None,
        }
    }

    /// W1.4 — wire the `<Page Self=...>` → flat-index map used to
    /// resolve hyperlink page destinations. Called only when
    /// link-region collection is on.
    fn with_page_index_map(mut self, map: &'a HashMap<String, u32>) -> Self {
        self.page_index_map = Some(map);
        self
    }

    /// W1.4 — resolve a hyperlink destination target id (a `<Page>`
    /// self id, or a story / text-anchor id) to a flat body-page index.
    /// Only page ids resolve in v1; story / text-anchor ids fall through
    /// to `None` (the caller records them as `Unresolved`).
    fn page_index_for_target(&self, target_id: &str) -> Option<u32> {
        self.page_index_map.and_then(|m| m.get(target_id).copied())
    }

    fn with_story_id(mut self, story_id: &str) -> Self {
        self.current_story_id = story_id.to_string();
        self
    }

    /// W1.22 — wire the document-level numbering ledger so
    /// `list_prefix` can carry a `ContinueNumbersAcrossStories` list's
    /// counter across story boundaries. Only set by `build_document`'s
    /// body-story pass when the document declares such a list.
    fn with_cross_story_numbering(
        mut self,
        ledger: &'a std::cell::RefCell<HashMap<String, u32>>,
    ) -> Self {
        self.cross_story_numbering = Some(ledger);
        self
    }

    /// W1.4 — record the document's total body-page count for
    /// `PageCountType` text-variable resolution. Called by the body /
    /// master pass once `pages` is sized.
    fn with_page_count(mut self, count: usize) -> Self {
        self.page_count = count;
        self
    }

    fn take_breaks(&mut self) -> Vec<BreakRecord> {
        std::mem::take(&mut self.breaks)
    }

    fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Cycle 6 Track 1: gate per-line break collection on the
    /// optional story / page filters. Returns true when the current
    /// emitter context is selected by both filters (each `None`
    /// filter passes anything).
    fn break_filter_passes(&self, target_page: u32) -> bool {
        if !self.options.collect_breaks {
            return false;
        }
        if let Some(want) = self.options.break_story_filter.as_deref() {
            if self.current_story_id != want {
                return false;
            }
        }
        if let Some(range) = self.options.break_page_range.as_ref() {
            if !range.contains(&target_page) {
                return false;
            }
        }
        true
    }

    /// Mark this emitter as a `depth`-deep anchored-story sub-emitter.
    /// The body/master pass leaves the default of 0; the anchored
    /// recursion path bumps the value before constructing each nested
    /// emitter so [`MAX_ANCHORED_STORY_RECURSION`] caps the depth.
    fn with_anchored_recursion_depth(mut self, depth: u32) -> Self {
        self.anchored_recursion_depth = depth;
        self
    }

    /// Hand off any image-bearing anchored frames captured during the
    /// story pass. The body / master pass calls this after
    /// `apply_blend_groups` so the post-pass below can reuse the
    /// already-resolved per-page + decoded caches without
    /// re-traversing the story tree.
    fn take_anchored_image_queue(&mut self) -> Vec<AnchoredImageEmit> {
        std::mem::take(&mut self.anchored_image_queue)
    }

    /// Set the story's `<StoryPreference>` optical-margin flags so
    /// the per-paragraph emit pass can nudge the leftmost / rightmost
    /// glyph of every line. `size_pt = 0.0` disables the feature even
    /// if the flag is true (matches `apply_optical_margin`'s noop).
    fn with_optical_margin(mut self, alignment: bool, size_pt: f32) -> Self {
        self.optical_margin_alignment = alignment;
        self.optical_margin_size_pt = size_pt;
        self
    }

    /// W1.7 — seed the per-frame footnote space reservation (1/64 pt)
    /// before a re-emit. `reserved[i]` is held back from chain frame
    /// `i`'s text bottom so the footnote pool drawn underneath does
    /// not overlap the body. A shorter/empty slice is padded with
    /// zeros; entries past the chain length are ignored.
    fn with_footnote_reservation(mut self, reserved: &[i32]) -> Self {
        for (slot, &r) in self.reserved_footnote_64.iter_mut().zip(reserved.iter()) {
            *slot = r.max(0);
        }
        self
    }

    fn emit_paragraph(
        &mut self,
        paragraph: &paged_parse::Paragraph,
        pages: &mut [BuiltPage],
        total_stats: &mut PipelineStats,
    ) {
        emit_paragraph_into_chain(self, paragraph, pages, total_stats);
        self.paragraph_idx = self.paragraph_idx.saturating_add(1);
    }

    fn apply_vertical_justification(&self, pages: &mut [BuiltPage]) {
        for (i, frame) in self.chain.iter().enumerate() {
            let Some((start, end)) = self.frame_cmd_ranges[i] else {
                continue;
            };
            let Some(vj) = frame.vertical_justification else {
                continue;
            };
            if vj == paged_parse::VerticalJustification::Top {
                continue;
            }
            let frame_height_64 =
                (frame.bounds.height() * paged_text::shape::ADVANCE_PRECISION).round() as i32;
            let used_64 = self.frame_max_baseline_64[i];
            // W1.7 — the footnote pool reserves the bottom of the
            // frame, so vertical justification must distribute slack
            // against the reduced text area. Without this, Center /
            // Bottom / Justify would shove the body text back down into
            // the reserved band and the pool would overlap it again.
            let reserved_64 = self.reserved_footnote_64.get(i).copied().unwrap_or(0);
            let usable_64 = (frame_height_64 - reserved_64).max(0);
            let slack_64 = (usable_64 - used_64).max(0);
            if vj == paged_parse::VerticalJustification::Justify {
                // JustifyAlign distributes the frame's slack as extra
                // space between paragraphs (NOT inside a paragraph —
                // that would distort leading). With < 2 paragraphs in
                // the frame or non-positive slack (overflow), the
                // result is identical to Top: nothing to shift.
                let segments = &self.paragraph_cmd_ranges[i];
                if slack_64 <= 0 || segments.len() < 2 {
                    continue;
                }
                let gaps = (segments.len() as i32 - 1).max(1);
                let gap_64 = slack_64 / gaps;
                if gap_64 == 0 {
                    continue;
                }
                let cmds = &mut pages[self.chain_pages[i]].list.commands;
                for (idx, &(seg_start, seg_end)) in segments.iter().enumerate() {
                    let dy_64 = gap_64 * idx as i32;
                    if dy_64 == 0 {
                        continue;
                    }
                    let dy_pt = dy_64 as f32 / paged_text::shape::ADVANCE_PRECISION;
                    for cmd in &mut cmds[seg_start..seg_end] {
                        cmd.transform_mut().0[5] += dy_pt;
                    }
                }
                continue;
            }
            let dy_64 = match vj {
                paged_parse::VerticalJustification::Center => slack_64 / 2,
                paged_parse::VerticalJustification::Bottom => slack_64,
                _ => 0,
            };
            if dy_64 == 0 {
                continue;
            }
            let dy_pt = dy_64 as f32 / paged_text::shape::ADVANCE_PRECISION;
            for cmd in &mut pages[self.chain_pages[i]].list.commands[start..end] {
                cmd.transform_mut().0[5] += dy_pt;
            }
        }
    }

    /// Bracket each text frame's glyph range with `BeginBlendGroup`
    /// / `EndBlendGroup` whenever the frame's blend mode is non-Normal
    /// or opacity < 100%. Run after `apply_vertical_justification` so
    /// the splice is over the final glyph positions; the inserted
    /// stub commands carry no rendering side-effects beyond the group
    /// composite at end-of-range.
    ///
    /// Splice `PushClip` / `PopClip` around the glyph range of any
    /// chain frame whose `<PathGeometry>` is non-rectangular (a
    /// triangle, pentagon, …). The clip path is the frame's polygon
    /// outline in spread coords (already post-`item_transform`); the
    /// clip transform is the per-page origin shift. Run BEFORE
    /// `apply_blend_groups` so blend / shadow brackets nest inside
    /// the clip and `frame_cmd_ranges` can be updated once.
    ///
    /// Layout still happens at the frame's AABB width — paragraph_breaker
    /// doesn't strictly enforce the per-line widths the polygon-clip
    /// path produces (`build_perline_wrap_widths`) when the carved
    /// segment is below the widest word. The clip is the structural
    /// guarantee that pixels outside the polygon never paint glyphs,
    /// even when the layout overflows visually. Background outside
    /// the polygon shows through as page paper.
    ///
    /// Skip list (mirrors `frame_polygon_spread`): rectangles, frames
    /// with <3 anchors, and rotated/sheared frames (where the polygon
    /// would need to be transformed *with* the frame at emit time —
    /// out of scope today).
    fn apply_polygon_clip(&mut self, pages: &mut [BuiltPage]) {
        // Collect (frame_idx, start, end, shape) clip records grouped by
        // page so we can splice in reverse start-order. The `FrameShape`
        // carries one flattened, transformed contour per
        // `<GeometryPathType>` — so an oval clips to its curve and a
        // compound path keeps its hole (W1.10), rather than the old
        // anchors-only single-contour diamond.
        type ClipRecord = (usize, usize, usize, paged_text::FrameShape);
        let mut per_page: HashMap<usize, Vec<ClipRecord>> = HashMap::new();
        for (i, frame) in self.chain.iter().enumerate() {
            let Some((start, end)) = self.frame_cmd_ranges[i] else {
                continue;
            };
            if start == end {
                continue;
            }
            let Some(shape) = frame_shape_spread(frame) else {
                continue;
            };
            let page_idx = self.chain_pages[i];
            per_page
                .entry(page_idx)
                .or_default()
                .push((i, start, end, shape));
        }
        for (page_idx, mut entries) in per_page {
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            for (frame_idx, start, end, shape) in entries {
                let page = &mut pages[page_idx];
                // Build a closed clip path: one MoveTo/LineTo*/Close
                // sub-path per contour. Coordinates are in spread
                // coords; the clip transform below maps to page coords.
                // The rasterizer fills with NonZero, so a hole contour
                // authored with opposite winding (IDML's convention)
                // carves the interior — its flattened ring preserves
                // that winding.
                let mut path = PathData::default();
                for contour in shape.contours() {
                    let Some(&(x, y)) = contour.first() else {
                        continue;
                    };
                    path.segments.push(PathSegment::MoveTo { x, y });
                    for &(x, y) in &contour[1..] {
                        path.segments.push(PathSegment::LineTo { x, y });
                    }
                    path.segments.push(PathSegment::Close);
                }
                let path_id = page.list.paths.push_anon(path);
                let (ox, oy) = page.spread_origin;
                let clip_transform = Transform::translate(-ox, -oy);
                // Splice in end-then-start order so the start-insert
                // doesn't shift `end`.
                page.list.commands.insert(
                    end,
                    paged_compose::DisplayCommand::PopClip(Transform::IDENTITY),
                );
                page.list.commands.insert(
                    start,
                    paged_compose::DisplayCommand::PushClip {
                        path_id,
                        transform: clip_transform,
                    },
                );
                // Range expanded by 2 commands (PushClip + PopClip).
                // `apply_blend_groups` reads this updated range so
                // its BeginBlendGroup / EndBlendGroup wraps OUTSIDE
                // the clip — clip nests inside the blend group,
                // matching PDF state-vs-buffer semantics.
                self.frame_cmd_ranges[frame_idx] = Some((start, end + 2));
            }
        }
    }

    /// The frame body (fill / stroke / drop-shadow) is bracketed
    /// separately at body emit time inside `emit_text_frame_into`. We
    /// emit two groups per blended text frame — one for the body, one
    /// for the glyphs — both using the same blend mode against the
    /// page underneath. Visually equivalent to a single group when the
    /// body and glyphs occupy disjoint pixel sets (text frames with
    /// transparent fills, the manual-sample case); slightly different
    /// only when the body's painted pixels overlap the glyph pixels
    /// AND the blend is non-associative.
    fn apply_blend_groups(&self, pages: &mut [BuiltPage]) {
        // Per-frame post-emit work: optionally splice glyph-shaped
        // drop shadows in front of the frame's glyph fills, then
        // optionally bracket the (still-original) glyph range with
        // a transparency group. Both run from the same per-page
        // reverse-start-order pass so command-index bookkeeping
        // stays straightforward.
        //
        // Entry shape: `(start, end, glyph_shadow, glyph_shadow_bounds,
        // blend_group)`.
        // - `glyph_shadow`: Some(DropShadow) if the frame has a
        //   stroke-transparency drop shadow AND the visible
        //   stroke + fill are both transparent (per InDesign
        //   semantics for "shadow off the visible text outlines").
        // - `glyph_shadow_bounds`: page-space rect to seed the
        //   shadow wrapper's BlendGroup buffer; the helper pads
        //   further by `|offset| + 3σ` to guarantee soft edges fit.
        // - `blend_group`: Some(...) when the frame's blend mode is
        //   non-Normal or opacity < 100%.
        type Entry = (
            usize,
            usize,
            Option<DropShadow>,
            paged_compose::Rect,
            Option<(paged_compose::Rect, paged_compose::BlendMode, f32)>,
        );
        let mut per_page: HashMap<usize, Vec<Entry>> = HashMap::new();
        for (i, frame) in self.chain.iter().enumerate() {
            let Some((start, end)) = self.frame_cmd_ranges[i] else {
                continue;
            };
            if start == end {
                // No glyphs were emitted into this frame — nothing
                // to bracket or shadow, skip.
                continue;
            }
            let page_idx = self.chain_pages[i];
            let blend_mode = blend_mode_from_idml(frame.blend_mode.as_deref());
            let opacity = frame.opacity;
            let needs_group = !matches!(blend_mode, paged_compose::BlendMode::Normal)
                || matches!(opacity, Some(o) if o < 100.0 - f32::EPSILON);
            let opacity_f = opacity.map(|p| (p / 100.0).clamp(0.0, 1.0)).unwrap_or(1.0);
            let outer = frame_outer_transform(&pages[page_idx], frame.item_transform);
            let inner_rect = paged_compose::Rect {
                x: frame.bounds.left,
                y: frame.bounds.top,
                w: frame.bounds.width(),
                h: frame.bounds.height(),
            };
            let frame_bounds_in_page = rect_bounds_in_page(inner_rect, outer);
            let blend_group = if needs_group {
                let padded = paged_compose::Rect {
                    x: frame_bounds_in_page.x - 0.5,
                    y: frame_bounds_in_page.y - 0.5,
                    w: frame_bounds_in_page.w + 1.0,
                    h: frame_bounds_in_page.h + 1.0,
                };
                Some((padded, blend_mode, opacity_f))
            } else {
                None
            };
            // Glyph-shaped shadow: emit when the frame carries a
            // stroke-transparency drop shadow AND both fill and
            // stroke are transparent (so the rect-shaped stamp from
            // the body-time drop_shadow_module wouldn't fire). Real
            // InDesign casts the shadow off the visible TEXT
            // outlines in this case.
            // Stroke weight defaults to 1.0pt when absent (InDesign
            // default); the stroke-visibility check still gates on
            // `Swatch/None` so absent-stroke-color frames register
            // as invisible regardless.
            let stroke_w = frame.stroke_weight.unwrap_or(1.0);
            let stroke_visible = frame_stroke_is_visible(frame.stroke_color.as_deref(), stroke_w);
            let fill_transparent = frame_fill_is_transparent(frame.fill_color.as_deref());
            let glyph_shadow =
                if !stroke_visible && fill_transparent && frame.stroke_drop_shadow.is_some() {
                    resolve_frame_shadow(
                        frame.stroke_drop_shadow.as_ref(),
                        None,
                        self.palette,
                        self.cmyk_xform,
                    )
                } else {
                    None
                };
            if glyph_shadow.is_none() && blend_group.is_none() {
                continue;
            }
            per_page.entry(page_idx).or_default().push((
                start,
                end,
                glyph_shadow,
                frame_bounds_in_page,
                blend_group,
            ));
        }
        // Splice in reverse start-order per page so earlier ranges
        // stay valid.
        for (page_idx, mut entries) in per_page {
            entries.sort_by(|a, b| b.0.cmp(&a.0));
            for (start, end, glyph_shadow, frame_bounds_in_page, blend_group) in entries {
                let page = &mut pages[page_idx];
                // Step 1: splice glyph-shaped shadows in front of
                // the original glyph range. The shadow stamps land
                // *before* any BeginBlendGroup we add in Step 2,
                // so a Lighten-blend frame's glyphs still cast a
                // dark shadow against the page below (Lighten of
                // dark gray on white = white = invisible — the
                // shadow has to be outside the group). Returns
                // `inserted`, the number of commands added (one
                // PathShadow per glyph fill plus the wrapper
                // BeginBlendGroup / EndBlendGroup); every later
                // index (incl. `end`) shifts forward by that count
                // for Step 2.
                let inserted = if let Some(shadow) = glyph_shadow {
                    // Group-buffer bounds for the shadow wrapper:
                    // the frame's bbox in page coords, padded by
                    // `(|offset| + 3*blur)` on each side so soft
                    // edges don't get clipped to the buffer. The
                    // helper inserts the BeginBlendGroup itself.
                    let pad = shadow.offset_x.abs().max(shadow.offset_y.abs())
                        + 3.0 * shadow.blur_radius.abs()
                        + 1.0;
                    let bounds_in_page = paged_compose::Rect {
                        x: frame_bounds_in_page.x - pad,
                        y: frame_bounds_in_page.y - pad,
                        w: frame_bounds_in_page.w + 2.0 * pad,
                        h: frame_bounds_in_page.h + 2.0 * pad,
                    };
                    crate::module::emit_glyph_shadow_pass(page, start..end, shadow, bounds_in_page)
                } else {
                    0
                };
                let glyphs_start = start + inserted;
                let glyphs_end = end + inserted;
                // Step 2: bracket glyph fills with BeginBlendGroup /
                // EndBlendGroup (when needed). Insert end-then-start
                // so the start-insert doesn't shift `glyphs_end`.
                if let Some((bounds, blend_mode, opacity)) = blend_group {
                    page.list.commands.insert(
                        glyphs_end,
                        paged_compose::DisplayCommand::EndBlendGroup(Transform::IDENTITY),
                    );
                    page.list.commands.insert(
                        glyphs_start,
                        paged_compose::DisplayCommand::BeginBlendGroup {
                            bounds,
                            blend_mode,
                            opacity,
                            transform: Transform::IDENTITY,
                        },
                    );
                }
            }
        }
    }
}

/// Phase 5 — build a synthetic `Paragraph` sequence for an index
/// story. Walks `Document::resolve_index()` and emits one paragraph
/// per topic:
///
///   "Apple\t12, 23, 41"
///   "Banana\t7"
///
/// Each paragraph carries the topic text, a tab separator (the IDML
/// `^t` convention; renderer's tab-stop pass snaps it to the next
/// tab stop), then a comma-separated string of page labels resolved
/// from `page_labels`. Page-label resolution mirrors `build_toc_paragraphs`
/// so Section overrides (Roman numerals etc.) flow through.
///
/// Returns an empty vec when the document has no markers.
///
/// Today the renderer doesn't yet trigger this automatically — there's
/// no frame attribute that says "this is the index host" the way
/// `AppliedTOCStyle` triggers the TOC swap-in. Callers that want to
/// emit a generated index point this at a target story and overwrite
/// its paragraphs.
pub fn build_index_paragraphs(
    document: &Document,
    page_labels: &[String],
) -> Vec<paged_parse::Paragraph> {
    let entries = document.resolve_index();
    let mut out: Vec<paged_parse::Paragraph> = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut text = entry.topic.clone();
        if !entry.pages.is_empty() {
            // Resolve page indices to labels. Missing labels
            // (out-of-bounds — shouldn't happen, defensive) skip.
            let labels: Vec<String> = entry
                .pages
                .iter()
                .filter_map(|i| page_labels.get(*i).cloned())
                .collect();
            if !labels.is_empty() {
                text.push('\t');
                text.push_str(&labels.join(", "));
            }
        }
        let run = paged_parse::CharacterRun {
            text,
            ..paged_parse::CharacterRun::default()
        };
        out.push(paged_parse::Paragraph {
            runs: vec![run],
            ..paged_parse::Paragraph::default()
        });
    }
    out
}

/// Build the synthetic `Paragraph` sequence for an unresolved TOC
/// story. Walks `Document::resolve_toc(toc_style)` and turns every
/// `TOCEntry` into a single `Paragraph` whose:
///   - `paragraph_style` = entry's `format_style`,
///   - one run carrying `text` + expanded `separator` + page label.
///
/// Tabs in `Separator` (IDML serialises a tab as `^t`) expand to a
/// literal `\t`, which `paged_text::layout::apply_tab_stops` snaps
/// to the next tab stop (or, when none, to a single tab width).
/// Page labels come from the per-page `page_labels` slice so
/// Section overrides (Roman numerals etc.) carry through.
///
/// Returns an empty vec when the TOC has no resolved entries —
/// keeps the renderer from emitting any glyphs into the host
/// frame (matches InDesign, which leaves the frame blank).
fn build_toc_paragraphs(
    document: &Document,
    toc_style: &paged_parse::TOCStyleDef,
    page_labels: &[String],
) -> Vec<paged_parse::Paragraph> {
    let entries = document.resolve_toc(toc_style);
    let mut out: Vec<paged_parse::Paragraph> = Vec::with_capacity(entries.len());
    for entry in entries {
        // Expand the IDML tab token. Only `^t` is recognised
        // today — Adobe's full set (^m em-space, ^>, etc.) is
        // queued; the corpus only carries `^t` separators.
        let separator = entry.separator.replace("^t", "\t");
        // Resolve the page label. `TOCEntry::page_number` is a
        // 0-based body-page index; `page_labels` is parallel to
        // the renderer's `pages` slice and already carries the
        // user-visible label (Section overrides included). When
        // the resolver returned `None` (orphan story) or the
        // entry suppressed the page number, skip the separator
        // + page-number tail.
        let page_label = entry
            .page_number
            .filter(|_| entry.page_number_visible)
            .and_then(|i| page_labels.get(i).cloned());
        let mut text = entry.text;
        if let Some(label) = page_label {
            text.push_str(&separator);
            text.push_str(&label);
        }
        let run = paged_parse::CharacterRun {
            text,
            ..paged_parse::CharacterRun::default()
        };
        let paragraph = paged_parse::Paragraph {
            paragraph_style: entry.format_style,
            runs: vec![run],
            ..paged_parse::Paragraph::default()
        };
        out.push(paragraph);
    }
    out
}

/// Body of `StoryEmitter::emit_paragraph`. Lives as a free fn so
/// the long, branching layout/emit pipeline isn't visually
/// indented under `impl`. The free fn has full mutable access to
/// the emitter state via `&mut StoryEmitter`.
fn emit_paragraph_into_chain(
    em: &mut StoryEmitter,
    paragraph: &paged_parse::Paragraph,
    pages: &mut [BuiltPage],
    total_stats: &mut PipelineStats,
) {
    // Tables ride on a paragraph but render with their own
    // grid-of-mini-frames pipeline. Hand off here so the rest of
    // this function stays focused on the run/glyph case.
    if let Some(table) = paragraph.table.as_ref() {
        emit_table_into_chain(em, table, pages, total_stats);
        return;
    }
    // Phase 5 — conditional text. Drop runs whose `AppliedConditions`
    // include any `<Condition Visible="false">`. Empty conditions list
    // means "always visible"; the filter pass is a no-op when no run
    // carries any conditions (almost every paragraph).
    let paragraph_filtered_owned;
    let paragraph: &paged_parse::Paragraph = {
        let has_conditions = paragraph
            .runs
            .iter()
            .any(|r| !r.applied_conditions.is_empty());
        if !has_conditions {
            paragraph
        } else {
            let conditions = &em.document.styles.conditions;
            let filtered: Vec<paged_parse::CharacterRun> = paragraph
                .runs
                .iter()
                .filter(|r| {
                    r.applied_conditions
                        .iter()
                        .all(|cid| conditions.get(cid).and_then(|c| c.visible).unwrap_or(true))
                })
                .cloned()
                .collect();
            paragraph_filtered_owned = paged_parse::Paragraph {
                runs: filtered,
                ..paragraph.clone()
            };
            &paragraph_filtered_owned
        }
    };

    // Phase 4 typography — nested character styles. If the paragraph
    // style declares `<NestedStyle>` children, splice the runs at
    // overlay boundaries and override the `character_style` field on
    // each sliced fragment. The rest of the function then sees a run
    // list whose applied character styles already reflect the nested
    // overrides; no other code path needs to know about nested styles.
    let paragraph_owned;
    let paragraph: &paged_parse::Paragraph = {
        let nested = &em
            .document
            .resolved_paragraph_attrs(paragraph)
            .nested_styles;
        if nested.is_empty() {
            paragraph
        } else {
            let paragraph_text: String = paragraph.runs.iter().map(|r| r.text.as_str()).collect();
            let overlay = compute_nested_style_overlay(&paragraph_text, nested);
            if overlay.is_empty() {
                paragraph
            } else {
                paragraph_owned = paged_parse::Paragraph {
                    runs: split_runs_for_nested_styles(&paragraph.runs, &overlay),
                    ..paragraph.clone()
                };
                &paragraph_owned
            }
        }
    };
    // IDML <Br/> serialises as `\n` inside run text; it's a forced
    // line break, not a paragraph break. paragraph_breaker treats
    // it as ordinary whitespace, which would let it merge into a
    // glue and lay text either side of it on the same line. Split
    // the paragraph at every `\n` boundary and emit each segment
    // as a sub-paragraph at the same paragraph style — same effect
    // as a hard break in the composer, no layout-engine change
    // required. Sub-paragraphs inherit the parent's style; only
    // SpaceBefore is suppressed for the second-and-later segments
    // so consecutive bullet rows don't accumulate extra leading.
    if paragraph.runs.iter().any(|r| r.text.contains('\n')) {
        for sub in split_paragraph_at_breaks(paragraph) {
            emit_paragraph_into_chain(em, &sub, pages, total_stats);
        }
        return;
    }

    // Phase 5 — capture any `<Footnote>` anchors on this paragraph
    // onto the page where the anchor character is going to land.
    // The anchor's page is the paragraph's *starting* frame's page
    // (em.chain_pages[em.frame_idx] at this point in the emit
    // sequence); per-page numbering restarts at 1.
    if !paragraph.footnotes.is_empty() {
        let anchor_page = em.chain_pages[em.frame_idx];
        let host_story = em.current_story_id.clone();
        let host_para_idx = em.paragraph_idx;
        // Capture the host frame's page-local content rect so the
        // post-pass footnote pool render knows where to draw.
        let frame = em.chain[em.frame_idx];
        let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
        let (ox, oy) = pages[anchor_page].spread_origin;
        let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
        let frame_w = frame.bounds.width();
        let frame_h = frame.bounds.height();
        let host_frame_rect_pt = Rect {
            x: sx - ox + insets[1],
            y: sy - oy + insets[0],
            w: (frame_w - insets[1] - insets[3]).max(0.0),
            h: (frame_h - insets[0] - insets[2]).max(0.0),
        };
        let pool = &mut pages[anchor_page].footnotes;
        for fn_body in &paragraph.footnotes {
            let next_number = pool.len() as u32 + 1;
            pool.push(EmittedFootnote {
                number: next_number,
                host_story_id: host_story.clone(),
                host_paragraph_idx: host_para_idx,
                footnote_self_id: fn_body.self_id.clone(),
                paragraphs: fn_body.paragraphs.clone(),
                host_frame_rect_pt,
            });
        }
    }
    // Empty paragraph: a sub-paragraph produced by `<Br/><Br/>` and
    // similar patterns. Advance the baseline cursor by one line of
    // auto-leading at the paragraph style's resolved point size so
    // the visible vertical rhythm matches InDesign. No glyphs emit.
    let runs_have_text = paragraph
        .runs
        .iter()
        .any(|r| !r.text.is_empty() && r.text != "\n");
    if !runs_have_text {
        let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);
        // Prefer the synthetic zero-text run's resolved PointSize when
        // present (the split function plants it on every empty
        // sub-paragraph so the leading reflects the surrounding text
        // size — e.g. 24pt `<Br/><Br/>` produces a 28.8pt gap, not
        // 14.4pt). Falls back to the paragraph style's PointSize and
        // ultimately the renderer-wide default.
        let run_pt = paragraph
            .runs
            .first()
            .and_then(|r| em.document.resolved_run_attrs(paragraph, r).point_size);
        let para_pt = run_pt.unwrap_or_else(|| {
            em.document
                .styles
                .resolve_paragraph(
                    paragraph
                        .paragraph_style
                        .as_deref()
                        .unwrap_or("ParagraphStyle/$ID/[No paragraph style]"),
                )
                .point_size
                .unwrap_or(em.options.default_point_size)
        });
        let space_before_64 =
            resolved_paragraph.space_before.unwrap_or(0.0) * paged_text::shape::ADVANCE_PRECISION;
        let line_height_64 = (para_pt * 1.2 * paged_text::shape::ADVANCE_PRECISION).round() as i32;
        // Establish the first baseline if we haven't placed any
        // content yet — same convention as the populated branch
        // below — then advance by a full line height.
        if em.y_cursor < 0 {
            em.y_cursor = (para_pt * 0.8 * paged_text::shape::ADVANCE_PRECISION).round() as i32;
        }
        em.y_cursor += space_before_64.round() as i32;
        // Adobe places the empty paragraph's virtual baseline at
        // `prev_baseline + leading(empty)`, then the next line at
        // `empty_baseline + leading(next)`. Our y_cursor encodes
        // `prev_baseline + leading(prev_line)`; rewind the previous
        // advance and re-apply with this paragraph's leading so a
        // 12pt empty between 24pt body and 24pt heading still
        // contributes only ~14.4pt (matching InDesign), while a
        // 12pt empty after a 12pt run unchanged (no-op when prev
        // and current leadings agree).
        let prev_lh = em.prev_line_height_64.unwrap_or(line_height_64);
        em.y_cursor = em.y_cursor - prev_lh + line_height_64 + line_height_64;
        em.prev_line_height_64 = Some(line_height_64);
        let space_after_64 =
            resolved_paragraph.space_after.unwrap_or(0.0) * paged_text::shape::ADVANCE_PRECISION;
        em.y_cursor += space_after_64.round() as i32;
        return;
    }
    total_stats.paragraphs += 1;
    total_stats.runs += paragraph.runs.len();
    pages[em.chain_pages[em.frame_idx]].stats.paragraphs += 1;
    pages[em.chain_pages[em.frame_idx]].stats.runs += paragraph.runs.len();

    let resolved_runs: Vec<paged_scene::ResolvedRunAttrs> = paragraph
        .runs
        .iter()
        .map(|r| em.document.resolved_run_attrs(paragraph, r))
        .collect();
    let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);

    // Resolve every run's font bytes up front so the borrows for
    // `Face` construction below all live in the same scope. Any run
    // whose (family, style) is unknown to the FontTable inherits a
    // paragraph-level fallback (first resolvable sibling > document
    // default font) — without this, an IDML referencing one missing
    // font (e.g. an obscure decorative face) would silently drop the
    // entire paragraph and lose every neighbouring run with it.
    let Some(bytes_pool) = em.font_table.resolve_paragraph_bytes(&resolved_runs) else {
        return;
    };

    // Per-run wght axis values. Variable fonts ship one TTF that
    // covers the whole weight axis; a run flagged `FontStyle="Bold"`
    // would otherwise render at the file's default weight (~400).
    // Pin a wght axis variation per run so bold / light / etc.
    // headings get the right thickness.
    let wghts: Vec<f32> = resolved_runs
        .iter()
        .map(|r| wght_for_font_style(r.font_style.as_deref()))
        .collect();

    // Dedup faces by (Bytes pointer identity, wght). Two runs with
    // the same font bytes but different weights need separate
    // faces because each holds a different fvar variation. When a
    // paragraph is single-weight (the common case) every run still
    // shares one face.
    let mut unique_idx: Vec<usize> = Vec::with_capacity(bytes_pool.len());
    for (i, b) in bytes_pool.iter().enumerate() {
        let head = bytes_pool[..i]
            .iter()
            .zip(wghts[..i].iter())
            .position(|(prior, w)| prior.as_ptr() == b.as_ptr() && (*w - wghts[i]).abs() < 0.5)
            .unwrap_or(i);
        unique_idx.push(head);
    }
    // Outline faces stay per-paragraph (ttf_parser::Face is cheap
    // and the outline interner already caches glyph outlines at the
    // DisplayList level — caching the Face itself buys little).
    let mut outline_faces: Vec<Option<ttf_parser::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    // Shaping faces: prefer the per-render FontTable cache; fall
    // back to building one on the fly when the cache misses (e.g.
    // a run added dynamically after build, or a fallback-font slot
    // the harvest pass didn't see). `owned_shaping_faces` holds the
    // fallbacks; `shaping_faces` is the parallel array of borrowed
    // references that StyledRun consumes downstream.
    let mut owned_shaping_faces: Vec<Option<rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut shaping_faces: Vec<Option<&rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
    let bytes_font_ids: Vec<u32> = bytes_pool.iter().map(|b| fnv_1a_u32(b.as_ref())).collect();
    for i in 0..bytes_pool.len() {
        if unique_idx[i] != i {
            continue;
        }
        let bytes_ref = bytes_pool[i].as_ref();
        let Ok(mut of) = ttf_parser::Face::parse(bytes_ref, 0) else {
            return;
        };
        let has_wght_axis = of
            .variation_axes()
            .into_iter()
            .any(|axis| axis.tag == wght_tag);
        if has_wght_axis {
            let _ = of.set_variation(wght_tag, wghts[i]);
        } else if (wghts[i] - 400.0).abs() > 50.0 {
            // Q-25: the IDML asked for a non-Regular weight but the
            // matched font has no `wght` variation axis (single-
            // weight TTF). Surface this as a trace so users know
            // catalog-brochure-template / brand-guidelines display
            // headlines render at the substitute's intrinsic weight
            // (e.g. "Catalog" hero ~30% thicker than ref). Curable
            // by routing the affected family through a variable font
            // in the per-pack fonts overrides.
            tracing::warn!(
                font_id = bytes_font_ids[i],
                requested_wght = wghts[i],
                "matched font has no wght axis; requested weight ignored — substitute will render at the file's intrinsic weight"
            );
        }
        outline_faces[i] = Some(of);

        // Shaping Face: cache lookup first, build on miss.
        if em
            .font_table
            .face(bytes_font_ids[i], wghts[i].to_bits())
            .is_none()
        {
            let Some(mut rf) = rustybuzz::Face::from_slice(bytes_ref, 0) else {
                return;
            };
            if has_wght_axis {
                rf.set_variations(&[rustybuzz::Variation {
                    tag: wght_tag,
                    value: wghts[i],
                }]);
            }
            owned_shaping_faces[i] = Some(rf);
        }
    }
    // Second pass: assemble the borrowed-reference array. The cache
    // is borrowed via `em.font_table` (which outlives this scope);
    // the on-demand owned faces are borrowed from `owned_shaping_faces`
    // (which lives to the end of the paragraph emission).
    for i in 0..bytes_pool.len() {
        let head = unique_idx[i];
        if let Some(cached) = em
            .font_table
            .face(bytes_font_ids[head], wghts[head].to_bits())
        {
            shaping_faces[i] = Some(cached);
        } else if let Some(owned) = owned_shaping_faces[head].as_ref() {
            shaping_faces[i] = Some(owned);
        }
    }

    // font_id mixes in the wght variation so the glyph-outline cache
    // (keyed on (font_id, glyph_id)) doesn't conflate outlines from a
    // variable font fed at two different wght axis values.
    let font_ids: Vec<u32> = bytes_pool
        .iter()
        .zip(wghts.iter())
        .map(|(b, w)| fnv_1a_u32(b.as_ref()) ^ w.to_bits())
        .collect();

    // Bulleted paragraphs prepend `<bullet><separator>` to the
    // first run's text. The bullet's font / size still inherit
    // from the first run; its colour can be overridden by a
    // `BulletsCharacterStyle` (see `bullet_paint_override` below).
    // Font / size override through the same character style is a
    // follow-up — the parser fields are in place. IDML serialises
    // tabs in BulletsTextAfter as the literal `^t` two-byte
    // sequence — expand to a real `\t` so apply_tab_stops snaps it.
    // W1.22 — resolve whether this paragraph's named NumberingList
    // wants cross-story continuity. When it does, seed the counter
    // from the document-level ledger (so a list spanning stories keeps
    // counting) and write the post-increment value back afterwards.
    // `None` keeps the legacy per-story scope.
    let cross_story_list_id: Option<&str> = resolved_paragraph
        .applied_numbering_list
        .as_deref()
        .filter(|id| {
            em.document
                .styles
                .numbering_lists
                .get(*id)
                .and_then(|def| def.continue_across_stories)
                .unwrap_or(false)
        });
    let cross_story_seed: Option<u32> = match (cross_story_list_id, em.cross_story_numbering) {
        (Some(id), Some(ledger)) => Some(ledger.borrow().get(id).copied().unwrap_or(0)),
        _ => None,
    };
    let list_first_text: Option<String> = list_prefix(
        &resolved_paragraph,
        &mut em.numbered_counter,
        &mut em.prev_was_numbered,
        cross_story_seed,
    )
    .and_then(|prefix| {
        paragraph
            .runs
            .first()
            .map(|r| format!("{prefix}{}", r.text))
    });
    // Save the post-increment counter back to the ledger so the next
    // story sharing this list continues from here. Only writes for a
    // numbered paragraph that actually advanced the counter (the
    // prefix was emitted); a bullet / NoList paragraph in the same
    // list leaves the ledger untouched.
    let advanced_numbered = list_first_text.is_some()
        && resolved_paragraph.bullets_list_type.as_deref() == Some("NumberedList");
    if let (Some(id), Some(ledger), true) = (
        cross_story_list_id,
        em.cross_story_numbering,
        advanced_numbered,
    ) {
        ledger
            .borrow_mut()
            .insert(id.to_string(), em.numbered_counter);
    }

    // Substitute IDML auto-page-number markers with the current
    // page number. The parser leaves a private-use sentinel in
    // run.text; expand here so master-spread footers print the
    // live page number rather than nothing.
    // Auto-page-number substitution. The page-labels table is keyed
    // by flat body-page index and already carries the user-visible
    // label (Arabic / Roman / section-overridden). ACE 19 (next-page)
    // peeks one slot ahead in the same table; for the last page it
    // numerically increments the current label as a best-effort.
    let cur_idx = em.chain_pages[em.frame_idx];
    let current_page_str = em
        .page_labels
        .get(cur_idx)
        .cloned()
        .unwrap_or_else(|| (cur_idx + 1).to_string());
    let next_page_str = em.page_labels.get(cur_idx + 1).cloned().unwrap_or_else(|| {
        current_page_str
            .parse::<i64>()
            .map(|n| (n + 1).to_string())
            .unwrap_or_else(|_| current_page_str.clone())
    });
    let needs_page_subst = paragraph.runs.iter().any(|r| {
        r.text.contains(paged_parse::AUTO_PAGE_NUMBER_MARKER)
            || r.text.contains(paged_parse::NEXT_PAGE_NUMBER_MARKER)
    }) || list_first_text
        .as_deref()
        .is_some_and(|t| t.contains(paged_parse::AUTO_PAGE_NUMBER_MARKER));
    let page_substituted: Vec<String> = if needs_page_subst {
        paragraph
            .runs
            .iter()
            .map(|r| {
                r.text
                    .replace(paged_parse::AUTO_PAGE_NUMBER_MARKER, &current_page_str)
                    .replace(paged_parse::NEXT_PAGE_NUMBER_MARKER, &next_page_str)
            })
            .collect()
    } else {
        Vec::new()
    };

    // W1.4 — text-variable substitution. Each run tagged with
    // `text_variable` (produced by the parser splitting a
    // `<TextVariableInstance>` into its own run) is re-resolved per the
    // variable's type: real page count, document name, custom literal,
    // etc. `None` ⇒ keep the run's baked `ResultText` (already its
    // `text`). Mirrors the auto-page-number marker substitution above;
    // the resolved string flows through the same per-run text override
    // as `capitalized` / `page_substituted`.
    let total_pages = em.page_count;
    let needs_var_subst = paragraph.runs.iter().any(|r| r.text_variable.is_some());
    let variable_resolved: Vec<Option<String>> = if needs_var_subst {
        paragraph
            .runs
            .iter()
            .map(|r| {
                r.text_variable.as_deref().and_then(|var_id| {
                    links::resolve_variable(
                        &em.document.container.designmap,
                        var_id,
                        &r.text,
                        total_pages,
                    )
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let var_text = |i: usize| -> Option<&str> {
        if needs_var_subst {
            variable_resolved.get(i).and_then(|o| o.as_deref())
        } else {
            None
        }
    };

    // Per-run uppercase override for `Capitalization=AllCaps`. The
    // previous implementation also uppercased SmallCaps / CapToSmallCap,
    // but our shaper doesn't drive the `smcp` OT feature yet — the
    // result was a row of full-height capitals where the IDML asked
    // for capital-tall + small-tall rhythm. Pass SmallCaps through
    // with its original case until a real small-caps fallback lands
    // (P-12). Allocates only for runs whose resolved capitalization
    // actually differs from their input.
    let capitalized: Vec<Option<String>> = paragraph
        .runs
        .iter()
        .enumerate()
        .map(
            |(i, run)| match resolved_runs[i].capitalization.as_deref() {
                Some("AllCaps") => {
                    let src: &str = if let Some(v) = var_text(i) {
                        v
                    } else if needs_page_subst {
                        page_substituted[i].as_str()
                    } else {
                        &run.text
                    };
                    let upper = src.to_uppercase();
                    if upper != src {
                        Some(upper)
                    } else {
                        None
                    }
                }
                _ => None,
            },
        )
        .collect();

    // P-20: per-cluster glyph fallback. Build a list of every
    // distinct sibling face used in this paragraph so a run that
    // shapes a cluster to `.notdef` can retry against another run's
    // face. Same-face siblings collapse via raw-pointer comparison
    // so the fallback list is bounded by the number of distinct
    // fonts in the paragraph (typically 1-3).
    let mut fallback_faces_pool: Vec<&rustybuzz::Face> = Vec::new();
    for (i, f) in shaping_faces.iter().enumerate() {
        if unique_idx[i] != i {
            continue;
        }
        let Some(face) = f else { continue };
        if !fallback_faces_pool
            .iter()
            .any(|existing| std::ptr::eq(*existing, *face))
        {
            fallback_faces_pool.push(face);
        }
    }
    let styled_runs: Vec<paged_text::StyledRun> = paragraph
        .runs
        .iter()
        .enumerate()
        .map(|(i, run)| {
            // `Position` (super/subscript) shrinks the run to a
            // fraction of its base size and adds a baseline offset on
            // top of any explicit `BaselineShift` — see
            // `position_adjusted_metrics`.
            let base_size = resolved_runs[i]
                .point_size
                .unwrap_or(em.options.default_point_size);
            let (point_size, baseline_shift_pt) = position_adjusted_metrics(
                base_size,
                resolved_runs[i].baseline_shift,
                resolved_runs[i].position.as_deref(),
            );
            paged_text::StyledRun {
                text: if i == 0 {
                    list_first_text.as_deref().unwrap_or_else(|| {
                        if let Some(c) = capitalized[i].as_deref() {
                            c
                        } else if let Some(v) = var_text(i) {
                            v
                        } else if needs_page_subst {
                            page_substituted[i].as_str()
                        } else {
                            &run.text
                        }
                    })
                } else if let Some(c) = capitalized[i].as_deref() {
                    c
                } else if let Some(v) = var_text(i) {
                    v
                } else if needs_page_subst {
                    page_substituted[i].as_str()
                } else {
                    &run.text
                },
                face: shaping_faces[unique_idx[i]].unwrap(),
                point_size,
                tracking: resolved_runs[i].tracking,
                font_id: font_ids[i],
                underline: resolved_runs[i].underline.unwrap_or(false),
                strikethru: resolved_runs[i].strikethru.unwrap_or(false),
                baseline_shift_pt,
                horizontal_scale_pct: resolved_runs[i].horizontal_scale.unwrap_or(100.0),
                vertical_scale_pct: resolved_runs[i].vertical_scale.unwrap_or(100.0),
                skew_deg: resolved_runs[i].skew.unwrap_or(0.0),
                fallback_faces: &fallback_faces_pool,
                shaping_features: shaping_features_from(
                    resolved_runs[i].ligatures_on,
                    resolved_runs[i].kerning_method.as_deref(),
                    &resolved_runs[i].otf,
                ),
            }
        })
        .collect();

    // W1.4 — hyperlink/cross-reference source spans for this paragraph,
    // as paragraph-local byte ranges into the concatenated styled-run
    // text (the same string the line clusters index into). Each entry
    // pre-resolves the source id to a `LinkTarget` so the per-line
    // capture below only intersects byte ranges. Empty (and skipped)
    // unless `collect_link_regions` is on AND a run carries a source.
    let link_spans: Vec<(std::ops::Range<usize>, paged_compose::LinkTarget)> =
        if em.collect_link_regions && paragraph.runs.iter().any(|r| r.hyperlink_source.is_some()) {
            let mut spans = Vec::new();
            let mut byte_cursor = 0usize;
            for (i, sr) in styled_runs.iter().enumerate() {
                let run_len = sr.text.len();
                let start = byte_cursor;
                byte_cursor += run_len;
                let Some(source_id) = paragraph
                    .runs
                    .get(i)
                    .and_then(|r| r.hyperlink_source.as_deref())
                else {
                    continue;
                };
                if run_len == 0 {
                    continue;
                }
                let target = links::resolve_link_target(
                    &em.document.container.designmap,
                    source_id,
                    |page_id| em.page_index_for_target(page_id),
                );
                spans.push((start..byte_cursor, target));
            }
            spans
        } else {
            Vec::new()
        };

    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let Some(full_col_pt) = em.column_width_pt else {
        return;
    };
    // FINDING #7.2 — LeftIndent / RightIndent narrow the composed
    // column (so the breaker wraps inside the indented measure) and the
    // body shifts right by LeftIndent post-layout. Clamp so a pathological
    // indent can't drive the column non-positive.
    let left_indent_pt = resolved_paragraph.left_indent.unwrap_or(0.0).max(0.0);
    let right_indent_pt = resolved_paragraph.right_indent.unwrap_or(0.0).max(0.0);
    let col_pt = (full_col_pt - left_indent_pt - right_indent_pt).max(1.0);
    let mut lopts = paged_text::LayoutOptions::new(col_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification);
    apply_paragraph_compose_options(&mut lopts, em.hyphenator, &resolved_paragraph);
    // Explicit `Leading` on the leading run mirrors IDML semantics:
    // every line uses the override regardless of the largest glyph
    // size on the line. Auto leading (no override) keeps existing
    // behaviour.
    if let Some(leading_pt) = resolved_runs.first().and_then(|r| r.leading) {
        if leading_pt > 0.0 {
            lopts.leading_override =
                Some((leading_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32);
        }
    }

    if em.y_cursor < 0 {
        // Family-keyed override wins over the byte-hash lookup so a
        // documented Arial → Roboto substitution can pin Arial's
        // ascender (0.728 em from sTypoAscender) via `--font-metrics`
        // instead of letting the substitute font's metrics override
        // every first baseline. See `manual-sample.fonts.sh` for the
        // concrete numbers.
        //
        // The byte-hash fallback uses `font_ids[0]`, which XORs in
        // the wght axis bits. `FontTable::metrics` is keyed by the
        // raw bytes-fnv hash without wght, so this lookup misses by
        // design: it forces AscentOffset to fall through to the
        // `0.8 × pt` heuristic when no family override is set.
        // Empirically `0.8 × pt` is closer to Adobe's actual baseline
        // (~0.7–0.75 em sTypoAscender for typical fonts) than most
        // substitute fonts' raw ascender values (Cormorant Garamond
        // 0.924, Roboto 1.048, etc.) — switching to the unmixed
        // lookup regressed the text-fixture's Minion Pro → Cormorant
        // substitution by ~2.4 pt per first baseline. The fix is to
        // pin the original font's metrics through `--font-metrics`
        // (the family-override branch above) rather than trusting the
        // substitute's metrics.
        let head_family = resolved_runs.first().and_then(|r| r.font.as_deref());
        let head_font_metrics = head_family
            .and_then(|f| em.font_table.metrics_for_family(f))
            .or_else(|| {
                font_ids
                    .first()
                    .and_then(|id| em.font_table.metrics_for(*id))
            });
        em.y_cursor = first_baseline_for_frame(
            em.chain[0],
            paragraph_size,
            lopts.first_baseline,
            head_font_metrics,
        );
    } else {
        let space_before_64 =
            resolved_paragraph.space_before.unwrap_or(0.0) * paged_text::shape::ADVANCE_PRECISION;
        em.y_cursor += space_before_64.round() as i32;
        // Adobe places each baseline at `prev_baseline + leading(THIS
        // line)`, not `+ leading(prev line)`. The most recent
        // y_cursor bump used the previous line/empty-paragraph's
        // leading; rewind that and re-apply with this paragraph's
        // first-line leading so mixed-size flows (12pt body → 24pt
        // heading) gain the extra leading Adobe expects. No-op when
        // previous and current leadings agree (the common case).
        if let Some(prev_lh) = em.prev_line_height_64 {
            em.y_cursor += lopts.line_height - prev_lh;
        }
    }
    lopts.first_baseline = em.y_cursor;

    // Drop cap: when the paragraph carries
    // `<ParagraphStyleRange DropCapCharacters="N" DropCapLines="M">`,
    // the first N characters render at an enlarged size for M body
    // lines. We carve the first M lines narrower in `column_widths`
    // and shape the dropped run separately at
    // `drop_cap_point_size(line_height_pt, M)` for emission below.
    //
    // The implementation:
    //   1. Decide the byte split inside the first styled run (the
    //      first `DropCapCharacters` Unicode scalars).
    //   2. Shape the dropped slice at the enlarged point size to
    //      measure `glyph_advance` for the column carve.
    //   3. Build a `DropCapSpec` and ask
    //      `paged_text::drop_cap_column_widths` for the carved widths.
    //   4. Replace the first styled run's text with the slice past
    //      the drop cap, then run `layout_runs` as normal.
    //   5. After layout, splice the dropped glyphs in at the
    //      paragraph origin.
    let drop_cap_spec_emit: Option<(
        usize,
        paged_text::DropCapSpec,
        paged_text::ShapedRun,
        f32,
        u32,
        ttf_parser::Face<'_>,
        paged_compose::Paint,
    )> = if paragraph.drop_cap_characters > 0
        && paragraph.drop_cap_lines > 0
        && !styled_runs.is_empty()
        && !styled_runs[0].text.is_empty()
    {
        let body_line_height_pt = lopts.line_height as f32 / paged_text::shape::ADVANCE_PRECISION;
        let cap_point_size =
            paged_text::drop_cap_point_size(body_line_height_pt, paragraph.drop_cap_lines);
        // Byte split: take `drop_cap_characters` Unicode scalars
        // off the front of run 0's text. Whitespace counts as a
        // character; IDML's serialisation matches char count not
        // grapheme count.
        let head = styled_runs[0].text;
        // Byte offset of the `drop_cap_characters`th scalar; past the
        // end keeps the whole run (split == head.len()).
        let split = head
            .char_indices()
            .nth(paragraph.drop_cap_characters as usize)
            .map(|(i, _)| i)
            .unwrap_or(head.len());
        if split > 0 {
            let dropped_slice = &head[..split];
            let cap_face_idx = unique_idx[0];
            let cap_face_ref = shaping_faces[cap_face_idx].unwrap();
            let cap_shaped = paged_text::shape_run(cap_face_ref, dropped_slice, cap_point_size);
            // Gutter: half the body's space-glyph advance — a small
            // proxy for InDesign's `DropCapDetail` side-bearing.
            let space_shaped = paged_text::shape_run(cap_face_ref, " ", styled_runs[0].point_size);
            let gutter_64 = space_shaped.total_advance / 2;
            let spec = paged_text::DropCapSpec {
                characters: paragraph.drop_cap_characters,
                lines: paragraph.drop_cap_lines,
                glyph_advance: cap_shaped.total_advance,
                gutter: gutter_64,
            };
            // Outline face for the dropped glyphs. Shares bytes with
            // the body run's face but parses fresh because the
            // existing `outline_faces[cap_face_idx]` instance lives
            // borrowed by the body emit loop below.
            let bytes_ref = bytes_pool[cap_face_idx].as_ref();
            let outline = ttf_parser::Face::parse(bytes_ref, 0).ok();
            // Drop-cap paint: pick from the first run's resolved
            // fill (same as the body's run-0 paint).
            let fallback_paint = em.options.fallback_text_paint;
            let cap_paint = resolved_runs
                .first()
                .and_then(|r| r.fill_color.as_deref())
                .and_then(|id| color_id_to_paint(id, em.palette, em.cmyk_xform))
                .unwrap_or(fallback_paint);
            // Now overlay the carved widths onto lopts so the
            // remainder body wraps narrower for the first M lines.
            // If a wrap pass set widths already, take min per line.
            //
            // P-19: clamp every carved width to at least the widest
            // shaped word in the remainder so paragraph_breaker can
            // still place at least one token per line. Without this,
            // wide-fallback fonts or aggressive cap sizes produced
            // an empty break list and the entire body text dropped.
            let scalar_width_64 = lopts.compose.column_width;
            let max_word_width_64 = styled_runs.iter().fold(0i32, |acc, run| {
                let shaped = paged_text::shape::shape_run(run.face, run.text, run.point_size);
                let mut local_max = 0i32;
                let mut current = 0i32;
                let text_bytes = run.text.as_bytes();
                let is_break = |i: u32| -> bool {
                    let idx = i as usize;
                    idx < text_bytes.len()
                        && (text_bytes[idx] == b' '
                            || text_bytes[idx] == b'\t'
                            || text_bytes[idx] == b'\n')
                };
                for g in &shaped.glyphs {
                    if is_break(g.cluster) {
                        local_max = local_max.max(current);
                        current = 0;
                    } else {
                        current = current.saturating_add(g.x_advance);
                    }
                }
                local_max = local_max.max(current);
                acc.max(local_max)
            });
            let carved = paged_text::drop_cap_column_widths_with_min(
                &spec,
                scalar_width_64,
                max_word_width_64,
            );
            if let Some(existing) = lopts.compose.column_widths.as_deref() {
                let mut merged: Vec<i32> = carved.clone();
                for (i, w) in merged.iter_mut().enumerate() {
                    if let Some(&e) = existing.get(i) {
                        *w = (*w).min(e);
                    }
                }
                for &e in existing.iter().skip(merged.len()) {
                    merged.push(e);
                }
                lopts.compose.column_widths = Some(merged);
            } else {
                lopts.compose.column_widths = Some(carved);
            }
            outline.map(|o| {
                (
                    split,
                    spec,
                    cap_shaped,
                    cap_point_size,
                    font_ids[0] ^ 0xD0DC_AAA0u32,
                    o,
                    cap_paint,
                )
            })
        } else {
            None
        }
    } else {
        None
    };

    // Cycle-7 Track 2: capture the dropped slice text so the first
    // line's BreakRecord source_text can include it. pdftotext sees
    // the drop-cap glyph as part of the line's first word; without
    // this, word_match_rate stays 0.0 on drop-cap-bearing fixtures
    // like text-advanced.
    let dropped_text_for_breaks: Option<String> =
        if em.options.collect_breaks && drop_cap_spec_emit.is_some() {
            let head = styled_runs[0].text;
            let split = drop_cap_spec_emit.as_ref().map(|t| t.0).unwrap_or(0);
            Some(head[..split].to_string())
        } else {
            None
        };

    // If we have a drop cap, splice the body-run text past the
    // dropped slice. We can't mutate `styled_runs` in place because
    // its `text` field borrows the source string; build a fresh
    // styled_runs vec borrowing from the same source at the new
    // offset.
    let styled_runs_storage: Vec<paged_text::StyledRun>;
    let styled_runs_ref: &[paged_text::StyledRun] =
        if let Some((split, _, _, _, _, _, _)) = &drop_cap_spec_emit {
            let mut adjusted: Vec<paged_text::StyledRun> = Vec::with_capacity(styled_runs.len());
            for (i, r) in styled_runs.iter().enumerate() {
                let new_text = if i == 0 { &r.text[*split..] } else { r.text };
                adjusted.push(paged_text::StyledRun {
                    text: new_text,
                    face: r.face,
                    point_size: r.point_size,
                    tracking: r.tracking,
                    font_id: r.font_id,
                    underline: r.underline,
                    strikethru: r.strikethru,
                    baseline_shift_pt: r.baseline_shift_pt,
                    horizontal_scale_pct: r.horizontal_scale_pct,
                    vertical_scale_pct: r.vertical_scale_pct,
                    skew_deg: r.skew_deg,
                    fallback_faces: r.fallback_faces,
                    shaping_features: r.shaping_features,
                });
            }
            styled_runs_storage = adjusted;
            &styled_runs_storage
        } else {
            &styled_runs
        };

    // Per-line wrap: build a `column_widths` slice + per-line
    // x-shifts + twin-pair markers based on which wrap rectangles
    // each predicted line intersects. Shifts are stored in 1/64 pt
    // so the post-layout pass can add them to each glyph's x;
    // twin_after[i] = true means line i shares its baseline with
    // line i-1 (BothSides flow around an obstacle).
    let WrapPlan {
        line_x_shifts_64,
        twin_after,
    } = build_perline_wrap_widths(em, styled_runs_ref, &mut lopts);

    // Twin segments (text wrap on both sides of an obstacle) emit
    // alternating narrow/wide widths to the breaker. For long
    // paragraphs `paragraph_breaker::total_fit`'s fitness-class
    // machinery prunes every candidate before the end-of-paragraph
    // penalty and returns zero breaks. Bumping the glue stretch
    // budget from 0.33 (Adobe's calibrated default) to 0.5 gives
    // Knuth-Plass enough headroom to absorb the narrow-to-wide row
    // transitions and converge on a feasible solution — verified
    // against the manual-sample page 7 case (~300 words flowing
    // around two obstacles, previously emitted zero lines). We
    // bump only when twins are present so the regular-column
    // corpus keeps the tightly-calibrated 0.33 and its 100%
    // line-break parity on the calibration suite. The bump trades
    // tighter line-break match against InDesign for the ability to
    // render at all — without it, a wrap-around-object paragraph
    // longer than ~165 words drops to an empty frame.
    let twins_present = twin_after.iter().any(|&t| t);
    if twins_present {
        lopts.compose.stretch_ratio = lopts.compose.stretch_ratio.max(0.5);
    }

    let mut laid_out = paged_text::cache::layout_runs_cached(styled_runs_ref, &lopts);

    // Optical margin alignment: when the story carries
    // `<StoryPreference OpticalMarginAlignment="true" />`, nudge the
    // leftmost / rightmost glyph of each line outward per
    // `paged_text::optical_margin_offset`. Operates directly on the
    // positioned glyphs (not the shaped run, since layout_runs has
    // already converted advances to absolute x). The leftmost glyph
    // shifts negative (hangs outward); the rightmost glyph shifts
    // positive when right/centre-aligned, no-op for left-aligned
    // lines (the trim leaves trailing whitespace inside the column
    // — which is what hanging punctuation visually achieves).
    if em.optical_margin_alignment && em.optical_margin_size_pt > 0.0 {
        // Build the concatenated paragraph text the way layout_runs
        // saw it — clusters point into this string.
        let mut paragraph_concat = String::new();
        for r in &styled_runs {
            paragraph_concat.push_str(r.text);
        }
        let bytes = paragraph_concat.as_bytes();
        let margin_size_pt = em.optical_margin_size_pt;
        for line in laid_out.lines.iter_mut() {
            if line.glyphs.is_empty() {
                continue;
            }
            let first_idx = 0usize;
            let last_idx = line.glyphs.len() - 1;
            let first_cluster = line.glyphs[first_idx].cluster as usize;
            let last_cluster = line.glyphs[last_idx].cluster as usize;
            let first_pt_size = line.glyphs[first_idx].point_size.max(1e-3);
            let last_pt_size = line.glyphs[last_idx].point_size.max(1e-3);
            let first_char = char_at_byte(bytes, first_cluster);
            let last_char = char_at_byte(bytes, last_cluster);
            // Left-side trim: shift leftmost glyph negative by
            // factor*pt, scaled by min(point_size,
            // margin_size_pt)/point_size so smaller glyphs hang less.
            if let Some(c) = first_char {
                let scale = if first_pt_size >= margin_size_pt {
                    1.0
                } else {
                    first_pt_size / margin_size_pt
                };
                let off_pt = paged_text::optical_margin_offset(
                    c,
                    paged_text::MarginSide::Left,
                    first_pt_size,
                ) * scale;
                if off_pt != 0.0 {
                    let off_64 = (off_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;
                    line.glyphs[first_idx].x -= off_64;
                }
            }
            // Right-side trim: shrink the rightmost glyph's advance
            // (so the line's natural width drops by `off_64`) — the
            // alignment pass already ran inside layout_runs, so the
            // pixel-level effect lands on the right edge of the
            // line. We mutate `x_advance` to keep the line width
            // bookkeeping consistent if any later pass reads it.
            if let Some(c) = last_char {
                let scale = if last_pt_size >= margin_size_pt {
                    1.0
                } else {
                    last_pt_size / margin_size_pt
                };
                let off_pt = paged_text::optical_margin_offset(
                    c,
                    paged_text::MarginSide::Right,
                    last_pt_size,
                ) * scale;
                if off_pt != 0.0 {
                    let off_64 = (off_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;
                    let g = &mut line.glyphs[last_idx];
                    let trim = off_64.min(g.x_advance);
                    g.x_advance -= trim;
                    line.width -= trim;
                }
            }
        }
    }

    // Apply per-line x-shifts (text wrap around objects).
    if !line_x_shifts_64.is_empty() {
        for (i, line) in laid_out.lines.iter_mut().enumerate() {
            let shift_64 = line_x_shifts_64[i.min(line_x_shifts_64.len() - 1)];
            if shift_64 == 0 {
                continue;
            }
            for g in &mut line.glyphs {
                g.x += shift_64;
            }
        }
    }

    // BothSides flow: collapse twin lines onto the previous line's
    // baseline so the two segments render side by side at the same
    // y. Subsequent non-twin lines step down by the original
    // composer leading from the most recent unique-baseline row,
    // not by their composer-assigned baseline (which counted twins
    // as separate rows). Without this pass twins would render as
    // sequential rows, which Knuth-Plass produced naively.
    if !twin_after.is_empty() {
        let line_height_64 = lopts.line_height.max(1);
        let mut prev_unique_baseline: Option<i32> = None;
        for (i, line) in laid_out.lines.iter_mut().enumerate() {
            let is_twin = twin_after.get(i).copied().unwrap_or(false) && i > 0;
            if is_twin {
                if let Some(target) = prev_unique_baseline {
                    let diff = line.baseline_y - target;
                    if diff != 0 {
                        line.baseline_y = target;
                        for g in &mut line.glyphs {
                            g.y -= diff;
                        }
                    }
                }
                // Twin partner — stays on previous unique row, doesn't
                // advance prev_unique_baseline.
            } else {
                let new_baseline = match prev_unique_baseline {
                    Some(prev) => prev + line_height_64,
                    None => line.baseline_y,
                };
                let diff = line.baseline_y - new_baseline;
                if diff != 0 {
                    line.baseline_y = new_baseline;
                    for g in &mut line.glyphs {
                        g.y -= diff;
                    }
                }
                prev_unique_baseline = Some(new_baseline);
            }
        }
    }

    // FINDING #7.2 — LeftIndent shifts the whole paragraph body right.
    // The column was already narrowed by left+right indent above (so the
    // breaker wrapped inside the measure); this slides every line to the
    // left margin. FirstLineIndent (below) stacks on top of this on line 0.
    if left_indent_pt != 0.0 {
        let left_64 = (left_indent_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;
        if left_64 != 0 {
            for line in laid_out.lines.iter_mut() {
                for g in &mut line.glyphs {
                    g.x += left_64;
                }
            }
        }
    }

    // FirstLineIndent shifts the first line's glyphs after
    // breaking — Knuth-Plass can't model a per-line x-shift, so
    // it's a post-layout pass.
    if let Some(indent_pt) = resolved_paragraph.first_line_indent {
        let indent_64 = (indent_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;
        if indent_64 != 0 {
            if let Some(line) = laid_out.lines.first_mut() {
                for g in &mut line.glyphs {
                    g.x += indent_64;
                }
            }
        }
    }

    // Drop cap indent: when a drop cap is active, the body text on
    // the first M=drop_cap_lines lines must start to the right of
    // the dropped glyph + gutter. The carved column widths got
    // layout_runs to break tighter; this shift moves the laid-out
    // glyphs from x=0 to x=glyph_advance + gutter so the body
    // doesn't overstrike the drop cap. Lines past M are unindented.
    if let Some((_, spec, _, _, _, _, _)) = &drop_cap_spec_emit {
        let indent_64 = spec.glyph_advance.saturating_add(spec.gutter);
        for (i, line) in laid_out.lines.iter_mut().enumerate() {
            if (i as u32) >= spec.lines {
                break;
            }
            for g in &mut line.glyphs {
                g.x += indent_64;
            }
        }
    }

    // Build the paragraph text that matches the cluster offsets
    // layout_runs saw — bulleted paragraphs include the prepended
    // bullet+separator on run 0. Compute lazily; only the tab
    // pass actually needs it.
    let needs_paragraph_text = paragraph.runs.iter().any(|r| r.text.contains('\t'))
        || list_first_text.as_deref().is_some_and(|t| t.contains('\t'));
    if needs_paragraph_text {
        let tab_stops: Vec<paged_text::layout::TabStopSpec> = resolved_paragraph
            .tab_list
            .iter()
            .map(|t| paged_text::layout::TabStopSpec {
                position_pt: t.position,
                alignment: map_tab_alignment(t.alignment.as_deref()),
                alignment_character: t
                    .alignment_character
                    .as_deref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or('.'),
                // IDML's `Leader` is a short string (commonly ".",
                // ". ", or "…"). Empty leaders are treated as absent
                // so the tab snaps without filling. Trailing
                // whitespace is significant — ". " produces
                // space-separated dots — so it's kept verbatim.
                leader: t.leader.clone().filter(|s| !s.is_empty()),
            })
            .collect();
        let paragraph_text: String = paragraph
            .runs
            .iter()
            .enumerate()
            .map(|(i, r)| {
                if i == 0 {
                    list_first_text.as_deref().unwrap_or(&r.text)
                } else {
                    &r.text
                }
            })
            .collect();
        // Pre-build the leader context once per paragraph so each
        // `\t` snap that has a non-empty `<TabStop Leader="...">` can
        // shape the leader with the run that owns the tab.
        let any_leader = tab_stops.iter().any(|t| t.leader.is_some());
        let leader_ctx = if any_leader {
            Some(paged_text::layout::LeaderContext::new(styled_runs_ref))
        } else {
            None
        };
        for line in laid_out.lines.iter_mut() {
            paged_text::layout::apply_tab_stops_with_leaders(
                line,
                &paragraph_text,
                &tab_stops,
                36.0,
                leader_ctx.as_ref(),
            );
        }
    }

    // Bullet-character-style paint override. When the paragraph
    // style references a `BulletsCharacterStyle` /
    // `BulletsAndNumberingDigitsCharacterStyle`, resolve that
    // character style's `FillColor` (with `FillTint` applied) so the
    // bullet / digit marker can render in a colour distinct from
    // run 0's fill. Font / size override via the same character
    // style is not yet wired through; this batch ships colour-only
    // and the parser fields are in place for the follow-up.
    let bullet_paint_override: Option<(u32, Paint)> = list_first_text.as_deref().and_then(|lft| {
        let bullet_len = lft
            .len()
            .saturating_sub(paragraph.runs.first().map(|r| r.text.len()).unwrap_or(0));
        if bullet_len == 0 {
            return None;
        }
        let style_id = bullet_marker_character_style(&resolved_paragraph)?;
        let resolved = em.document.styles.resolve_character(style_id);
        let fill_id = resolved.fill_color.as_deref()?;
        let base = color_id_to_paint(fill_id, em.palette, em.cmyk_xform)?;
        let paint = apply_fill_tint(base, resolved.fill_tint);
        Some((bullet_len as u32, paint))
    });

    let picker = build_run_paint_picker_resolved(
        paragraph,
        &resolved_runs,
        em.palette,
        em.cmyk_xform,
        em.options.fallback_text_paint,
        bullet_paint_override,
    );
    let stroke_picker = build_run_stroke_picker(
        paragraph,
        &resolved_runs,
        em.palette,
        em.cmyk_xform,
        bullet_paint_override.map(|(len, _)| len).unwrap_or(0),
    );
    let any_text_stroke = stroke_picker.any_visible();

    let space_after_64 =
        resolved_paragraph.space_after.unwrap_or(0.0) * paged_text::shape::ADVANCE_PRECISION;
    // Per-frame segment tracker for the JustifyAlign vertical-justify
    // mode: each line's commands extend the active segment for its
    // host frame, and a frame switch closes the prior segment so the
    // pass can shift each paragraph independently.
    let mut active_seg: Option<(usize, usize, usize)> = None; // (frame_idx, cmd_start, cmd_end)
    let mut dropped_overflow_lines: usize = 0;
    // Q-09: resolve paragraph-shading band once per paragraph. The
    // per-line emit below stamps the band before each line's glyphs
    // so multi-line shaded paragraphs span continuously visually.
    // We bake the resolved (color, tint, offsets) up-front so the
    // per-line code path stays cheap.
    let shading_paint = if resolved_paragraph.shading.on == Some(true) {
        resolved_paragraph
            .shading
            .color
            .as_deref()
            .and_then(|id| color_id_to_paint(id, em.palette, em.cmyk_xform))
            .map(|p| {
                let tint = resolved_paragraph.shading.tint.unwrap_or(100.0);
                // IDML tint of -1 means "use stop color as-is"; 0..100
                // scales the swatch toward white.
                if tint < 0.0 {
                    p
                } else {
                    apply_fill_tint(p, Some(tint))
                }
            })
    } else {
        None
    };
    let shading_offsets = [
        resolved_paragraph.shading.offset_top.unwrap_or(0.0),
        resolved_paragraph.shading.offset_left.unwrap_or(0.0),
        resolved_paragraph.shading.offset_bottom.unwrap_or(0.0),
        resolved_paragraph.shading.offset_right.unwrap_or(0.0),
    ];
    // Q-09: resolve RuleAbove / RuleBelow paint + geometry once per
    // paragraph. The per-line emit below stamps the line above the
    // first line (RuleAbove) or below the last line (RuleBelow).
    let resolve_rule_paint = |r: &paged_parse::ParagraphRule| -> Option<Paint> {
        if r.on != Some(true) {
            return None;
        }
        let id = r.color.as_deref()?;
        let base = color_id_to_paint(id, em.palette, em.cmyk_xform)?;
        let tint = r.tint.unwrap_or(100.0);
        if tint < 0.0 {
            Some(base)
        } else {
            Some(apply_fill_tint(base, Some(tint)))
        }
    };
    let rule_above_paint = resolve_rule_paint(&resolved_paragraph.rule_above);
    let rule_below_paint = resolve_rule_paint(&resolved_paragraph.rule_below);
    // Q-09: resolve ParagraphBorder paint once per paragraph. The
    // four-edge stroke lands at the END of the last line, using the
    // first line's baseline (captured below) to anchor the top edge.
    let border_paint = if resolved_paragraph.border.on == Some(true) {
        resolved_paragraph
            .border
            .color
            .as_deref()
            .and_then(|id| color_id_to_paint(id, em.palette, em.cmyk_xform))
            .map(|p| {
                let tint = resolved_paragraph.border.tint.unwrap_or(100.0);
                if tint < 0.0 {
                    p
                } else {
                    apply_fill_tint(p, Some(tint))
                }
            })
    } else {
        None
    };
    let last_line_index = laid_out.lines.len().saturating_sub(1);
    let mut current_line_idx: usize = 0;
    // Q-09: capture the first line's baseline so the border's top
    // edge anchors above it; closed out at the last-line emit.
    let mut first_baseline_pt: Option<f32> = None;
    // Cycle-5 Track 1: pre-concatenate the paragraph text once so the
    // Track-2 BreakRecord can slice per-line `[first_byte..last_byte]`
    // without re-walking the run vec. Only built when break collection
    // is enabled — production renders skip the allocation entirely.
    let paragraph_text_for_breaks: Option<String> = if em.options.collect_breaks {
        let mut buf = String::new();
        for r in styled_runs_ref {
            buf.push_str(r.text);
        }
        Some(buf)
    } else {
        None
    };
    for mut line in laid_out.lines.into_iter() {
        let line_h = paged_text::layout::max_line_height_for_glyphs(&line.glyphs)
            .unwrap_or(lopts.line_height);
        let frame_height_64 = (em.chain[em.frame_idx].bounds.height()
            * paged_text::shape::ADVANCE_PRECISION)
            .round() as i32;
        // W1.7 — the usable text bottom is the frame height minus the
        // space reserved for this frame's footnote pool. Lines whose
        // baseline crosses it flow on (or drop, on the last frame) so
        // the pool drawn in the post-pass lands below the body text
        // rather than over it. Zero reservation reproduces the old
        // `frame_height_64` comparison byte-for-byte (the no-footnote
        // regression guard). Never let the reservation invert the
        // usable area to a negative bottom — a pool taller than the
        // frame is the FootnoteOverflow case, handled by accepting the
        // overlap rather than dropping every line.
        let text_bottom_64 = (frame_height_64
            - em.reserved_footnote_64
                .get(em.frame_idx)
                .copied()
                .unwrap_or(0))
        .max(0);
        if line.baseline_y > text_bottom_64 && em.frame_idx + 1 < em.chain.len() {
            let prev_baseline = line.baseline_y;
            em.frame_idx += 1;
            let new_baseline =
                (paragraph_size * 0.8 * paged_text::shape::ADVANCE_PRECISION).round() as i32;
            let dy = new_baseline - prev_baseline;
            for g in &mut line.glyphs {
                g.y += dy;
            }
            line.baseline_y = new_baseline;
        }
        // A-09 (AutoSizing height): a frame whose AutoSizingType grows
        // height is authored undersized and expected to grow to fit its
        // text. Rather than dropping the overflow (below), keep placing
        // lines — the frame effectively extends downward (the common
        // Top* reference point). The visible fill/stroke box growth +
        // the text-wrap cascade for neighbouring frames are Phase B.
        let last_frame_grows_height = em
            .chain
            .get(em.frame_idx)
            .and_then(|f| f.auto_sizing)
            .map(|a| a.grows_height())
            .unwrap_or(false);
        // P-13 short-term: when the last frame in the chain overflows
        // (typically because a font substitute is wider than the
        // requested face), drop the overflow lines rather than letting
        // them spill across following frames/pages with no clip. The
        // reference PDFs hide the overflow via the same out-of-frame
        // clip; matching this prevents large ΔE regions.
        if line.baseline_y > text_bottom_64
            && em.frame_idx + 1 >= em.chain.len()
            && !last_frame_grows_height
        {
            dropped_overflow_lines += 1;
            // Report once per story: the count of dropped lines isn't
            // known until the paragraph finishes, but a single signal
            // that this story is overset is the actionable bit.
            if !em.overset_reported {
                em.overset_reported = true;
                let page = em.chain_pages[em.frame_idx];
                let mut d = Diagnostic::new(
                    DiagnosticCode::OversetTextDropped,
                    "text overflows the last frame in its chain; trailing lines clipped (overset)",
                )
                .with_page(page);
                if !em.current_story_id.is_empty() {
                    d = d.with_story(em.current_story_id.clone());
                }
                em.diagnostics.push(d);
            }
            continue;
        }

        let target_page = em.chain_pages[em.frame_idx];
        pages[target_page].stats.glyphs += line.glyphs.len();
        pages[target_page].stats.lines += 1;
        total_stats.glyphs += line.glyphs.len();
        total_stats.lines += 1;

        // Track 2: A/B-harness break record. Cheap when disabled; the
        // collector flag is checked once per line. baseline_y / width
        // live in paged_text's 1/64-pt units (ADVANCE_PRECISION) so we
        // divide back to pt here so downstream tooling (the Python
        // reference-side extractor) reads natural units. Cycle-6
        // Track 1: also gated on optional story / page-range filters.
        if em.break_filter_passes(target_page as u32) {
            // Slice the line's source text from the paragraph buffer
            // we pre-built above. byte_range is a half-open
            // `[start..end)` of bytes; clamp to the buffer length so
            // a malformed breaker output can't out-of-bounds.
            // For the first line of a drop-cap paragraph, prepend the
            // dropped characters PLUS any paragraph-text bytes the
            // breaker skipped before the line's first_byte (typically
            // a leading space — InDesign's content "In a hole..." with
            // DropCapCharacters="2" leaves the body as " a hole..."
            // and the breaker starts line 0 at the 'a', skipping the
            // space). pdftotext sees the contiguous "In a" so we
            // reconstruct that here for word-match parity.
            let source_text = paragraph_text_for_breaks
                .as_deref()
                .map(|pt| {
                    let start = if current_line_idx == 0 && dropped_text_for_breaks.is_some() {
                        0
                    } else {
                        line.byte_range.start.min(pt.len())
                    };
                    let end = line.byte_range.end.min(pt.len());
                    let body = pt.get(start..end).unwrap_or("");
                    if current_line_idx == 0 {
                        if let Some(dropped) = dropped_text_for_breaks.as_deref() {
                            return format!("{dropped}{body}");
                        }
                    }
                    body.to_string()
                })
                .unwrap_or_default();
            em.breaks.push(BreakRecord {
                story_id: em.current_story_id.clone(),
                paragraph_idx: em.paragraph_idx,
                line_idx: current_line_idx as u32,
                page_idx: target_page as u32,
                frame_idx: em.frame_idx as u32,
                first_byte: line.byte_range.start as u32,
                last_byte: line.byte_range.end as u32,
                baseline_y_pt: line.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION,
                width_pt: line.width as f32 / paged_text::shape::ADVANCE_PRECISION,
                source_text,
            });
        }

        let frame = em.chain[em.frame_idx];
        let (ox, oy) = pages[target_page].spread_origin;
        let frame_insets = frame.inset_spacing.unwrap_or([0.0; 4]);
        // frame.bounds is in the frame's *inner* coordinate system
        // (PathGeometry-derived for real-world IDMLs). The frame's
        // ItemTransform maps that to spread coords; subtracting the
        // page's spread_origin then puts text in page-local pt.
        // column_x_shift_pt is non-zero only when a wrap rectangle
        // intrudes from the head frame's left side.
        let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
        let text_origin_pt = (sx - ox + frame_insets[1] + em.column_x_shift_pt, sy - oy);

        // Phase 3 Item A — capture per-cluster page-local positions.
        // Lets the canvas hit-test by character offset, place the
        // caret, and compute selection geometry. Captured
        // unconditionally; cost is O(glyphs on this line). The
        // captured baseline / x_pt are in page-local pt — already
        // includes the frame's spread→page→origin offset. Rotated
        // frames receive their visual rotation via the post-emit
        // pass that follows; the captured positions here are the
        // upright pre-rotation values, suitable for content-side
        // selection math (rotation only affects how we *render* the
        // caret, not which character it points at).
        {
            let baseline_pt_local = line.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION;
            let line_h_pt = line_h as f32 / paged_text::shape::ADVANCE_PRECISION;
            let mut clusters: Vec<ClusterPos> = Vec::with_capacity(line.glyphs.len());
            // Coalesce glyphs that share a source cluster (ligatures,
            // multi-glyph clusters) into one ClusterPos entry.
            let mut last_cluster: Option<u32> = None;
            for g in &line.glyphs {
                let adv = g.x_advance as f32 / paged_text::shape::ADVANCE_PRECISION;
                if last_cluster == Some(g.cluster) {
                    if let Some(c) = clusters.last_mut() {
                        c.advance_pt += adv;
                    }
                    continue;
                }
                last_cluster = Some(g.cluster);
                let x_pt_page =
                    text_origin_pt.0 + g.x as f32 / paged_text::shape::ADVANCE_PRECISION;
                clusters.push(ClusterPos {
                    byte: g.cluster,
                    x_pt: x_pt_page,
                    advance_pt: adv,
                });
            }
            // W1.4 — clickable link regions. For each hyperlink source
            // span that overlaps this visible line, bound the clusters
            // in the overlap into one page-local pt rect and push a
            // `LinkRegion` carrying the pre-resolved target. The rect's
            // vertical extent follows the line's ascent / descent (same
            // heuristic the LineLayout uses); a span covering multiple
            // lines yields one region per line, which the PDF backend
            // emits as separate annotations — the correct behaviour for
            // a wrapped link.
            if em.collect_link_regions && !link_spans.is_empty() {
                let baseline_y_page = text_origin_pt.1 + baseline_pt_local;
                let asc = 0.8 * line_h_pt;
                let desc = 0.2 * line_h_pt;
                let line_start = line.byte_range.start;
                let line_end = line.byte_range.end;
                for (span, target) in &link_spans {
                    // Byte intersection of the span with this line.
                    let lo = span.start.max(line_start);
                    let hi = span.end.min(line_end);
                    if lo >= hi {
                        continue;
                    }
                    // Bound the clusters whose byte falls in [lo, hi).
                    let mut min_x = f32::MAX;
                    let mut max_x = f32::MIN;
                    for c in &clusters {
                        let b = c.byte as usize;
                        if b >= lo && b < hi {
                            min_x = min_x.min(c.x_pt);
                            max_x = max_x.max(c.x_pt + c.advance_pt);
                        }
                    }
                    if min_x > max_x {
                        // No cluster landed in the overlap (e.g. the
                        // span covers only trailing whitespace) — skip.
                        continue;
                    }
                    if let Some(table) = pages[target_page].list.link_regions.as_mut() {
                        table.push(paged_compose::LinkRegion {
                            rect: paged_compose::Rect {
                                x: min_x,
                                y: baseline_y_page - asc,
                                w: (max_x - min_x).max(0.0),
                                h: asc + desc,
                            },
                            target: target.clone(),
                        });
                    }
                }
            }

            let host_page_id = pages[target_page].id.clone();
            pages[target_page].story_layout.push(LineLayout {
                story_id: em.current_story_id.clone(),
                page_id: host_page_id,
                cell: None,
                paragraph_idx: em.paragraph_idx,
                line_idx: current_line_idx as u32,
                frame_id: frame.self_id.clone(),
                baseline_y_pt: text_origin_pt.1 + baseline_pt_local,
                // Phase 3 first cut: line-height heuristic for ascent
                // / descent. Real font metrics arrive alongside the
                // main-thread fast composer.
                ascent_pt: 0.8 * line_h_pt,
                descent_pt: 0.2 * line_h_pt,
                byte_range: line.byte_range.start as u32..line.byte_range.end as u32,
                clusters,
            });
        }

        // Pull just the rotation/scale 2×2 from the frame's
        // ItemTransform. emit_glyph_slice positions glyphs in upright
        // page coords offset by `text_origin_pt`; the post-emit pass
        // below rotates each glyph command around the frame's spread
        // top-left so rotated TextFrames render with text rotated.
        let frame_linear = frame
            .item_transform
            .map(|m| [m[0], m[1], m[2], m[3]])
            .unwrap_or([1.0, 0.0, 0.0, 1.0]);
        let frame_is_upright = (frame_linear[1].abs() < 1e-5)
            && (frame_linear[2].abs() < 1e-5)
            && ((frame_linear[0] - 1.0).abs() < 1e-5)
            && ((frame_linear[3] - 1.0).abs() < 1e-5);

        let before_cmds = pages[target_page].list.commands.len();

        // Q-09: emit RuleAbove BEFORE the shading rect on the first
        // line so the rule sits above the shading band.
        let is_first_line = current_line_idx == 0;
        let is_last_line = current_line_idx == last_line_index;
        let line_h_pt_local = line_h as f32 / paged_text::shape::ADVANCE_PRECISION;
        let baseline_pt_local = line.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION;
        if is_first_line {
            first_baseline_pt = Some(baseline_pt_local);
            if let Some(paint) = rule_above_paint {
                let r = &resolved_paragraph.rule_above;
                let weight = r.weight.unwrap_or(1.0).max(0.01);
                let offset = r.offset.unwrap_or(0.0);
                let left = r.left_indent.unwrap_or(0.0);
                let right = r.right_indent.unwrap_or(0.0);
                let col_w_pt = em.column_width_pt.unwrap_or(0.0);
                // Rule y: above the first line's baseline by
                // (line_h * 0.8 + offset). InDesign's default origin
                // for RuleAbove is the baseline; we approximate with
                // ascent ≈ 0.8 line_h.
                let rule_y = text_origin_pt.1 + baseline_pt_local - line_h_pt_local * 0.8 - offset;
                let x_left = text_origin_pt.0 + left;
                let x_right = text_origin_pt.0 + col_w_pt - right;
                if x_right > x_left {
                    let rect = paged_compose::Rect {
                        x: x_left,
                        y: rule_y - weight * 0.5,
                        w: x_right - x_left,
                        h: weight,
                    };
                    paged_compose::emit_rect_transformed(
                        rect,
                        Transform::IDENTITY,
                        paint,
                        &mut pages[target_page].list,
                    );
                }
            }
        }

        // Q-09: paint the shading band BEFORE the line's glyphs so it
        // composites behind the text. Width spans the column (modulo
        // the per-side offsets); vertical extents are line_h above
        // and a descent-fudge below the baseline. The renderer doesn't
        // yet differentiate `AscentTopOrigin` vs `BaselineTopOrigin`
        // etc. — `line_h * 0.8` covers the ascent portion well enough
        // for the visible band to read correctly for most display
        // headlines.
        if let Some(paint) = shading_paint {
            let line_h_pt = line_h as f32 / paged_text::shape::ADVANCE_PRECISION;
            let baseline_pt = line.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION;
            let col_w_pt = em.column_width_pt.unwrap_or(0.0);
            let y_top = text_origin_pt.1 + baseline_pt - line_h_pt * 0.8 - shading_offsets[0];
            let y_bot = text_origin_pt.1 + baseline_pt + line_h_pt * 0.2 + shading_offsets[2];
            let x_left = text_origin_pt.0 + shading_offsets[1];
            let x_right = text_origin_pt.0 + col_w_pt - shading_offsets[3];
            if x_right > x_left && y_bot > y_top {
                let rect = paged_compose::Rect {
                    x: x_left,
                    y: y_top,
                    w: x_right - x_left,
                    h: y_bot - y_top,
                };
                paged_compose::emit_rect_transformed(
                    rect,
                    Transform::IDENTITY,
                    paint,
                    &mut pages[target_page].list,
                );
            }
        }

        let mut start = 0;
        while start < line.glyphs.len() {
            let fid = line.glyphs[start].font_id;
            let mut end = start + 1;
            while end < line.glyphs.len() && line.glyphs[end].font_id == fid {
                end += 1;
            }
            let face_idx = match font_ids.iter().position(|f| *f == fid) {
                Some(i) => unique_idx[i],
                None => {
                    start = end;
                    continue;
                }
            };
            let Some(outline) = outline_faces[face_idx].as_ref() else {
                start = end;
                continue;
            };
            let outliner = TtfOutliner::new(outline);
            // Frame blend mode is applied at the transparency-group
            // level by `bracket_text_frame_glyph_ranges` after the
            // story pass completes; the glyphs themselves emit at
            // BlendMode::Normal so the group composite is the single
            // place the IDML BlendingSetting takes effect.
            emit_glyph_slice(
                &line.glyphs[start..end],
                fid,
                line.glyphs[start].point_size,
                |cluster| picker.pick(cluster),
                text_origin_pt,
                &outliner,
                &mut pages[target_page].list,
            );
            // Text strokes are sparse — guard the second sweep with
            // `any_text_stroke` so paragraphs without a single
            // `StrokeColor` cascade skip the per-glyph picker probe
            // entirely. When active, the stroke commands land in
            // display order *after* the matching fills so the outline
            // paints on top of the silhouette (InDesign's default for
            // `OutsideAlignment`-style outlines).
            if any_text_stroke {
                emit_glyph_slice_stroke(
                    &line.glyphs[start..end],
                    fid,
                    line.glyphs[start].point_size,
                    |cluster| stroke_picker.pick(cluster),
                    text_origin_pt,
                    &outliner,
                    &mut pages[target_page].list,
                );
            }
            start = end;
        }
        emit_line_decorations(
            &line,
            &picker,
            (sx - ox, sy - oy),
            &mut pages[target_page].list,
        );

        // Phase 7 — Kenten emphasis marks. For each glyph whose
        // source run has `kenten_kind` resolved to something other
        // than "None", stamp a small mark above the glyph centre.
        // Mark = a black-filled circle at ~10% of base point size
        // (matches InDesign's default visual density for the
        // common "Black Circle" / "Sesame Dot" presets); position
        // = above the line's baseline by ~1.1 × base point size.
        // Per-character `KentenKind` variants (Dot / Sesame /
        // White / Custom) all stamp the same simple filled circle
        // today; richer glyphs (the actual ・ / ﹅ shapes) are a
        // follow-up.
        emit_kenten_for_line(
            &line,
            paragraph,
            &resolved_runs,
            (sx - ox, sy - oy),
            &mut pages[target_page].list,
        );

        // Phase 7 — Ruby annotations. For each run with
        // `ruby_flag = true` and a non-empty `ruby_string`, shape
        // the ruby text at half the run's point size using the
        // document's fallback font and emit it centered above the
        // base run's glyphs. Per-character vs. group alignment is
        // collapsed to "group centered" in the MVP — distributing
        // ruby chars per base char (`PerCharacter` mode) requires
        // a more involved layout pass and is queued.
        if let Some(font) = em.options.font {
            emit_ruby_for_line(
                &line,
                paragraph,
                &resolved_runs,
                font,
                (sx - ox, sy - oy),
                &mut pages[target_page].list,
            );
        }

        // For rotated/sheared TextFrames, post-multiply each glyph
        // command's transform by the frame's linear 2×2, pivoting
        // around the frame's page-space top-left so glyphs end up
        // rotated *with* their host frame. Upright frames skip the
        // pass entirely (the common case).
        let after_glyph_cmds = pages[target_page].list.commands.len();
        if !frame_is_upright {
            let pivot_x = sx - ox;
            let pivot_y = sy - oy;
            for cmd in &mut pages[target_page].list.commands[before_cmds..after_glyph_cmds] {
                let xf = cmd.transform_mut();
                rotate_transform_around(xf, frame_linear, pivot_x, pivot_y);
            }
        }

        let after_cmds = pages[target_page].list.commands.len();
        // Glyph-level overprint: when the paragraph cascade sets
        // `OverprintFill="true"` (or stroke) on a `<ParagraphStyleRange>`
        // or its applied paragraph style, rewrite this line's freshly
        // emitted `FillPath` / `StrokePath` (including decoration
        // strokes) to their `*Overprint` variants. Per-run mixing within
        // a paragraph (some runs overprint, others knockout) is not yet
        // honoured — the slice loop already groups glyphs by (font,
        // paint), so a future batch can extend the picker to include
        // the flag in the band identity.
        let op_fill = resolved_paragraph.overprint_fill.unwrap_or(false);
        let op_stroke = resolved_paragraph.overprint_stroke.unwrap_or(false);
        if op_fill || op_stroke {
            rewrite_tail_for_overprint(&mut pages[target_page], before_cmds, op_fill, op_stroke);
        }
        let frame_idx = em.frame_idx;
        match &mut em.frame_cmd_ranges[frame_idx] {
            Some((_, e)) => *e = after_cmds,
            None => em.frame_cmd_ranges[frame_idx] = Some((before_cmds, after_cmds)),
        }
        match active_seg {
            Some((f, _, _)) if f != frame_idx => {
                if let Some((prev_f, s, e)) = active_seg.take() {
                    if s != e {
                        em.paragraph_cmd_ranges[prev_f].push((s, e));
                    }
                }
                active_seg = Some((frame_idx, before_cmds, after_cmds));
            }
            Some((f, s, _)) => active_seg = Some((f, s, after_cmds)),
            None => active_seg = Some((frame_idx, before_cmds, after_cmds)),
        }
        if line.baseline_y > em.frame_max_baseline_64[frame_idx] {
            em.frame_max_baseline_64[frame_idx] = line.baseline_y;
        }

        em.y_cursor = line.baseline_y + line_h;
        em.prev_line_height_64 = Some(line_h);

        // Q-09: emit RuleBelow AFTER the last line's glyphs so the
        // rule sits in front of the body text. Mirror of the
        // RuleAbove emit at the top of the loop. Same column +
        // indent + weight handling; offset is measured below the
        // baseline so positive `offset` pushes the rule further down.
        if is_last_line {
            if let Some(paint) = rule_below_paint {
                let r = &resolved_paragraph.rule_below;
                let weight = r.weight.unwrap_or(1.0).max(0.01);
                let offset = r.offset.unwrap_or(0.0);
                let left = r.left_indent.unwrap_or(0.0);
                let right = r.right_indent.unwrap_or(0.0);
                let col_w_pt = em.column_width_pt.unwrap_or(0.0);
                let rule_y = text_origin_pt.1 + baseline_pt_local + line_h_pt_local * 0.2 + offset;
                let x_left = text_origin_pt.0 + left;
                let x_right = text_origin_pt.0 + col_w_pt - right;
                if x_right > x_left {
                    let rect = paged_compose::Rect {
                        x: x_left,
                        y: rule_y - weight * 0.5,
                        w: x_right - x_left,
                        h: weight,
                    };
                    paged_compose::emit_rect_transformed(
                        rect,
                        Transform::IDENTITY,
                        paint,
                        &mut pages[target_page].list,
                    );
                }
            }
        }
        // Q-09: emit ParagraphBorder on the last line. Sharp corners
        // (all radii 0) keep the cheap four-fill-rect path; any rounded
        // corner switches to a single rounded-outline StrokePath
        // (Track 4d).
        if is_last_line {
            if let (Some(paint), Some(first_baseline)) = (border_paint, first_baseline_pt) {
                let b = &resolved_paragraph.border;
                let weight = b.weight.unwrap_or(1.0).max(0.01);
                let off_top = b.offset_top.unwrap_or(0.0);
                let off_left = b.offset_left.unwrap_or(0.0);
                let off_bottom = b.offset_bottom.unwrap_or(0.0);
                let off_right = b.offset_right.unwrap_or(0.0);
                let col_w_pt = em.column_width_pt.unwrap_or(0.0);
                let x_left = text_origin_pt.0 + off_left;
                let x_right = text_origin_pt.0 + col_w_pt - off_right;
                let y_top = text_origin_pt.1 + first_baseline - line_h_pt_local * 0.8 - off_top;
                let y_bot =
                    text_origin_pt.1 + baseline_pt_local + line_h_pt_local * 0.2 + off_bottom;
                if x_right > x_left && y_bot > y_top {
                    let radii = per_corner_radii(None, None, &b.corners);
                    let kinds = per_corner_kinds(None, &b.corners);
                    let any_rounded = radii.iter().any(|r| r.map(|v| v > 0.0).unwrap_or(false));
                    if any_rounded {
                        let outline_rect = paged_compose::Rect {
                            x: x_left,
                            y: y_top,
                            w: x_right - x_left,
                            h: y_bot - y_top,
                        };
                        let path = corner_rect_path(outline_rect, radii, kinds);
                        let path_id = pages[target_page].list.paths.push_anon(path);
                        pages[target_page]
                            .list
                            .push(paged_compose::DisplayCommand::StrokePath {
                                path_id,
                                paint,
                                stroke: paged_compose::Stroke::new(weight),
                                transform: Transform::IDENTITY,
                            });
                    } else {
                        let top = paged_compose::Rect {
                            x: x_left - weight * 0.5,
                            y: y_top - weight * 0.5,
                            w: (x_right - x_left) + weight,
                            h: weight,
                        };
                        let bottom = paged_compose::Rect {
                            x: x_left - weight * 0.5,
                            y: y_bot - weight * 0.5,
                            w: (x_right - x_left) + weight,
                            h: weight,
                        };
                        let left_edge = paged_compose::Rect {
                            x: x_left - weight * 0.5,
                            y: y_top - weight * 0.5,
                            w: weight,
                            h: (y_bot - y_top) + weight,
                        };
                        let right_edge = paged_compose::Rect {
                            x: x_right - weight * 0.5,
                            y: y_top - weight * 0.5,
                            w: weight,
                            h: (y_bot - y_top) + weight,
                        };
                        for r in [top, right_edge, bottom, left_edge] {
                            paged_compose::emit_rect_transformed(
                                r,
                                Transform::IDENTITY,
                                paint,
                                &mut pages[target_page].list,
                            );
                        }
                    }
                }
            }
        }
        current_line_idx += 1;
    }
    // Drop-cap glyph emission: now that the body lines have landed,
    // position the dropped run at the paragraph's origin (left edge,
    // first baseline). The dropped glyphs share the head frame's
    // page; we use the first laid-out line's baseline_y as the
    // y reference (already adjusted for text_origin_pt). Cluster=0
    // routes the paint picker to run 0 — same fill as the body's
    // first character.
    if let Some((_, _spec, cap_shaped, cap_point_size, cap_font_id, cap_outline, cap_paint)) =
        drop_cap_spec_emit
    {
        let target_page = em.chain_pages[em.frame_idx];
        let frame = em.chain[em.frame_idx];
        let (ox, oy) = pages[target_page].spread_origin;
        let frame_insets = frame.inset_spacing.unwrap_or([0.0; 4]);
        let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
        let text_origin_pt = (sx - ox + frame_insets[1] + em.column_x_shift_pt, sy - oy);
        // Drop-cap baseline = M-th body line's baseline, where
        // M = `paragraph.drop_cap_lines`. InDesign aligns the cap-
        // height of the dropped glyph with the first body line's
        // cap-height; the glyph then descends to the M-th body
        // line's baseline. We compute that as
        // `first_baseline + (M - 1) * line_height` (M >= 1 always
        // when the spec is active). Falls back to the emitter's
        // y_cursor when no body line was emitted (drop cap consumed
        // the entire paragraph).
        let baseline_64 = if em.y_cursor < 0 {
            (cap_point_size * 0.8 * paged_text::shape::ADVANCE_PRECISION).round() as i32
        } else {
            let m = paragraph.drop_cap_lines.saturating_sub(1) as i32;
            lopts.first_baseline + m * lopts.line_height
        };
        let mut positioned: Vec<paged_text::PositionedGlyph> =
            Vec::with_capacity(cap_shaped.glyphs.len());
        let mut pen_x = 0i32;
        for g in &cap_shaped.glyphs {
            positioned.push(paged_text::PositionedGlyph {
                glyph_id: g.glyph_id,
                cluster: 0, // route paint to run 0
                x: pen_x + g.x_offset,
                y: baseline_64 + g.y_offset,
                x_advance: g.x_advance,
                font_id: cap_font_id,
                point_size: cap_point_size,
                underline: false,
                strikethru: false,
                x_scale: 1.0,
                y_scale: 1.0,
                // Drop caps inherit run 0's skew (the cap is the head of
                // the first run) so a skewed paragraph leans its cap too.
                skew_deg: resolved_runs.first().and_then(|r| r.skew).unwrap_or(0.0),
                ch: None,
            });
            pen_x += g.x_advance;
        }
        let outliner = TtfOutliner::new(&cap_outline);
        let before_cap_cmds = pages[target_page].list.commands.len();
        emit_glyph_slice(
            &positioned,
            cap_font_id,
            cap_point_size,
            |_cluster| cap_paint,
            text_origin_pt,
            &outliner,
            &mut pages[target_page].list,
        );
        // Drop-cap glyphs inherit run 0's outline when the paragraph
        // resolves a text stroke (cluster=0 routes the picker to the
        // first run's band). Rare but cheap to honour for the few
        // paragraphs where it applies.
        if any_text_stroke {
            emit_glyph_slice_stroke(
                &positioned,
                cap_font_id,
                cap_point_size,
                |cluster| stroke_picker.pick(cluster),
                text_origin_pt,
                &outliner,
                &mut pages[target_page].list,
            );
        }
        let after_cap_cmds = pages[target_page].list.commands.len();
        // Track the drop-cap glyphs against the same frame range so
        // any later transparency / vertical-justification pass
        // covers them.
        let frame_idx = em.frame_idx;
        match &mut em.frame_cmd_ranges[frame_idx] {
            Some((_, e)) => *e = after_cap_cmds,
            None => em.frame_cmd_ranges[frame_idx] = Some((before_cap_cmds, after_cap_cmds)),
        }
        match active_seg {
            Some((f, _, _)) if f != frame_idx => {
                if let Some((prev_f, s, e)) = active_seg.take() {
                    if s != e {
                        em.paragraph_cmd_ranges[prev_f].push((s, e));
                    }
                }
                active_seg = Some((frame_idx, before_cap_cmds, after_cap_cmds));
            }
            Some((f, s, _)) => active_seg = Some((f, s, after_cap_cmds)),
            None => active_seg = Some((frame_idx, before_cap_cmds, after_cap_cmds)),
        }
    }
    if let Some((f, s, e)) = active_seg {
        if s != e {
            em.paragraph_cmd_ranges[f].push((s, e));
        }
    }
    if dropped_overflow_lines > 0 {
        total_stats.dropped_overflow_lines += dropped_overflow_lines;
    }
    em.y_cursor += space_after_64.round() as i32;

    // Anchored object pass: walk the paragraph's `anchored_frames`
    // list and emit each one. We support InlinePosition (the most
    // common case) plus a best-effort AbovePosition / Custom that
    // applies anchor_x / anchor_y offsets relative to the
    // paragraph's baseline. Frame content recursion is intentionally
    // shallow — the parser provides bounds + setting + a story ref
    // for TextFrames; richer recursion (nested transparency, full
    // fill cascade) lands when the corpus needs it.
    if !paragraph.anchored_frames.is_empty() {
        // Resolve the anchor line's vertical metrics (x-height /
        // cap-height / leading-top) for the `Line*` vertical reference
        // points. Source the metrics the same way as the rest of the
        // baseline math: the IDML family override first, then the head
        // run's real parsed font metrics keyed by the raw byte hash
        // (NOT `font_ids[0]`, which mixes in the wght axis and is
        // keyed differently from `FontTable::metrics`). The leading is
        // the paragraph's effective line height (explicit override or
        // 1.2× auto).
        let anchor_family = resolved_runs.first().and_then(|r| r.font.as_deref());
        let anchor_metrics = anchor_family
            .and_then(|f| em.font_table.metrics_for_family(f))
            .or_else(|| {
                bytes_pool
                    .first()
                    .map(|b| fnv_1a_u32(b.as_ref()))
                    .and_then(|id| em.font_table.metrics_for(id))
            });
        let baseline_y_pt = {
            let frame = em.chain[em.frame_idx];
            let (_ox, oy) = pages[em.chain_pages[em.frame_idx]].spread_origin;
            let (_sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
            let para_origin_y = sy - oy;
            if em.y_cursor >= 0 {
                para_origin_y + em.y_cursor as f32 / paged_text::shape::ADVANCE_PRECISION
            } else {
                para_origin_y
            }
        };
        let leading_pt = lopts.leading_override.unwrap_or(lopts.line_height).max(1) as f32
            / paged_text::shape::ADVANCE_PRECISION;
        let line_metrics = anchored::LineRefMetrics::resolve(
            baseline_y_pt,
            paragraph_size,
            leading_pt,
            anchor_metrics,
        );
        // W0.6 margin box for the anchor's host page, page-local pt.
        let margin_box = resolve_page_margin_box(em.document, &pages[em.chain_pages[em.frame_idx]]);
        emit_anchored_frames_for_paragraph(
            em,
            paragraph,
            pages,
            line_metrics,
            margin_box,
            total_stats,
        );
    }
}

/// Resolve the host page's `<MarginPreference>` margin box into a
/// page-local pt rectangle for the anchored `PageMargins` reference
/// point. The margins live on the parsed `Spread` as a side map
/// (`page_margins`) keyed by the page's `Self` id (W0.6); `BuiltPage::id`
/// carries that same id. Page `Self` ids are document-unique, so a flat
/// scan across spreads finds the one owning this page. Margins inset the
/// page rectangle, so the box is `[left, top, width-right, height-bottom]`
/// in the page's own (0,0)-top-left coordinate frame. Returns `None` when
/// the page declared no margins (the reference then degenerates to the
/// page edge).
fn resolve_page_margin_box(
    document: &Document,
    page: &BuiltPage,
) -> Option<anchored::PageMarginBox> {
    let page_self = page.id.0.as_str();
    if page_self.is_empty() {
        return None;
    }
    let m = document
        .spreads
        .iter()
        .find_map(|s| s.spread.page_margins.get(page_self))?;
    Some(anchored::PageMarginBox {
        left: m.left,
        top: m.top,
        right: (page.width_pt - m.right).max(m.left),
        bottom: (page.height_pt - m.bottom).max(m.top),
    })
}

/// Wraps a page's bounds for centre-point routing + its master
/// reference for master-spread application + its position in the
/// document so the master pass can read back per-page state
/// (MasterPageTransform).
struct PageGeom {
    bounds_in_spread: paged_parse::Bounds,
    applied_master: Option<String>,
    host_spread_idx: usize,
    local_page_idx: usize,
}

/// Local mirror of `paged_compose::text::get_or_intern_glyph_outline`,
/// which is private. Same caching key (font_id × glyph_id) so glyphs
/// emitted via the body-text path and the text-on-path path share
/// outlines.
fn list_get_or_intern_glyph_outline<O: GlyphOutliner>(
    font_id: u32,
    glyph_id: u32,
    outliner: &O,
    list: &mut DisplayList,
) -> Option<paged_compose::PathId> {
    let key = GlyphCacheKey { font_id, glyph_id }.to_u64();
    if let Some(existing) = list.paths.find_by_key(key) {
        return Some(existing);
    }
    let outline = outliner.outline(glyph_id)?;
    let (id, _) = list.paths.intern(key, outline);
    Some(id)
}

/// Cheap content-derived cache key for polygons that don't carry a
/// `Self` id (synthetic / minified IDMLs). FNV-1a of the
/// concatenated anchor coordinates.
pub(crate) fn path_signature(anchors: &[PathAnchor]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for a in anchors {
        for v in [
            a.anchor.0, a.anchor.1, a.left.0, a.left.1, a.right.0, a.right.1,
        ] {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
    }
    h
}

pub(crate) fn fnv_1a_u64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Decode the UTF-8 character starting at byte offset `i` in `bytes`.
/// Returns `None` when `i` is past the end or doesn't sit on a UTF-8
/// boundary. Used by the optical-margin pass to look up the
/// leftmost / rightmost glyph's source codepoint by cluster, since
/// `PositionedGlyph::cluster` is a byte offset into the paragraph's
/// concatenated source text.
fn char_at_byte(bytes: &[u8], i: usize) -> Option<char> {
    if i >= bytes.len() {
        return None;
    }
    // Walk forward up to 4 bytes — the maximum UTF-8 sequence
    // length — and decode lazily via std::str::from_utf8.
    let end = (i + 4).min(bytes.len());
    let slice = &bytes[i..end];
    std::str::from_utf8(slice)
        .ok()
        .and_then(|s| s.chars().next())
        .or_else(|| {
            // If the 4-byte window straddled an invalid boundary
            // (rare — clusters can land on byte-start of any
            // codepoint), fall back to a slower scan from byte 0.
            std::str::from_utf8(&bytes[..end])
                .ok()
                .and_then(|s| s[i..].chars().next())
        })
}

/// Apply a 6-element IDML affine `[a b c d e f]` to `(x, y)`.
/// Per IDML spec §10.3.3 the matrix maps inner→parent coords:
/// `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
fn apply_matrix(m: &[f32; 6], x: f32, y: f32) -> (f32, f32) {
    let [a, b, c, d, e, f] = *m;
    (a * x + c * y + e, b * x + d * y + f)
}

/// Transform an axis-aligned `Bounds` by an IDML affine and return
/// the AABB of the result. Identity (`None`) is the no-op.
/// For pure translation (the common Page.ItemTransform case) this
/// preserves width/height; for the 90° page rotations the spec
/// allows on whole spreads, the AABB swaps width/height — the right
/// behaviour for routing + canvas sizing.
/// W1.9 — the LINEAR part (rotation / scale) of a spread-level
/// `<Spread ItemTransform>`, with the translation dropped (it cancels
/// against the spread-inner page origin in `frame_outer_transform`).
/// Returns `Transform::IDENTITY` when the transform is absent or is a
/// pure translation (`[1 0 0 1 tx ty]`) — the overwhelmingly common
/// case — so the per-page composition stays byte-identical to the
/// pre-W1.9 path. Applied *about the page origin*, so only the 2×2
/// linear block matters here.
fn spread_linear_transform(m: Option<[f32; 6]>) -> Transform {
    match m {
        Some([a, b, c, d, _, _]) => {
            let is_identity_linear = (a - 1.0).abs() < 1e-6
                && b.abs() < 1e-6
                && c.abs() < 1e-6
                && (d - 1.0).abs() < 1e-6;
            if is_identity_linear {
                Transform::IDENTITY
            } else {
                Transform([a, b, c, d, 0.0, 0.0])
            }
        }
        None => Transform::IDENTITY,
    }
}

fn transform_bounds(b: paged_parse::Bounds, m: Option<[f32; 6]>) -> paged_parse::Bounds {
    let Some(m) = m else { return b };
    let corners = [
        apply_matrix(&m, b.left, b.top),
        apply_matrix(&m, b.right, b.top),
        apply_matrix(&m, b.right, b.bottom),
        apply_matrix(&m, b.left, b.bottom),
    ];
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for (x, y) in corners {
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
    }
    paged_parse::Bounds {
        top: min_y,
        left: min_x,
        bottom: max_y,
        right: max_x,
    }
}

/// A text-wrap obstacle: AABB bounds plus the four corner points of
/// the (possibly rotated) source rectangle in spread coords. The
/// AABB drives fast vertical/horizontal rejection and the simple
/// side-shrink heuristic; the polygon corners drive per-line carve
/// against rotated obstacles so a rotated rect's wrap follows its
/// actual angled edges instead of its much wider unrotated AABB.
#[derive(Debug, Clone, Copy)]
struct WrapShape {
    bounds: paged_parse::Bounds,
    corners: [(f32, f32); 4],
}

impl WrapShape {
    /// Build from an inner-coord `Bounds`, an optional ItemTransform,
    /// and per-side wrap offsets `[top, left, bottom, right]`. The
    /// offsets inflate the unrotated source rect *before* the
    /// transform applies so the polygon stays aligned with the host's
    /// rotation (offset is in inner-coord points, same as InDesign).
    fn from_inner(b: paged_parse::Bounds, m: Option<[f32; 6]>, offsets: [f32; 4]) -> Self {
        let inner = paged_parse::Bounds {
            top: b.top - offsets[0],
            left: b.left - offsets[1],
            bottom: b.bottom + offsets[2],
            right: b.right + offsets[3],
        };
        let corners = match m {
            Some(m) => [
                apply_matrix(&m, inner.left, inner.top),
                apply_matrix(&m, inner.right, inner.top),
                apply_matrix(&m, inner.right, inner.bottom),
                apply_matrix(&m, inner.left, inner.bottom),
            ],
            None => [
                (inner.left, inner.top),
                (inner.right, inner.top),
                (inner.right, inner.bottom),
                (inner.left, inner.bottom),
            ],
        };
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        );
        for (x, y) in corners {
            if x < min_x {
                min_x = x;
            }
            if x > max_x {
                max_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if y > max_y {
                max_y = y;
            }
        }
        let bounds = paged_parse::Bounds {
            top: min_y,
            left: min_x,
            bottom: max_y,
            right: max_x,
        };
        Self { bounds, corners }
    }

    /// Return the polygon's projected x-extent within the horizontal
    /// strip `[band_top, band_bottom]` (spread y). Returns `None` if
    /// the polygon doesn't intersect the strip vertically. The result
    /// is the (min_x, max_x) range over all polygon points whose y
    /// lies inside the strip plus all polygon-edge crossings of the
    /// strip's top and bottom horizontal lines. This handles both
    /// upright AABBs (where corners themselves bound the answer) and
    /// rotated parallelograms (where edges crossing the strip yield
    /// the carve.
    fn x_extent_in_band(&self, band_top: f32, band_bottom: f32) -> Option<(f32, f32)> {
        if self.bounds.bottom <= band_top || self.bounds.top >= band_bottom {
            return None;
        }
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut visit = |x: f32| {
            if x < min_x {
                min_x = x;
            }
            if x > max_x {
                max_x = x;
            }
        };
        // Corners that lie inside the strip.
        for (x, y) in self.corners {
            if y >= band_top && y <= band_bottom {
                visit(x);
            }
        }
        // Edge crossings against the two horizontal strip lines.
        for i in 0..4 {
            let (x0, y0) = self.corners[i];
            let (x1, y1) = self.corners[(i + 1) % 4];
            for &y_line in &[band_top, band_bottom] {
                let crosses = (y0 - y_line) * (y1 - y_line) <= 0.0 && (y0 - y1).abs() > 1e-6;
                if crosses {
                    let t = (y_line - y0) / (y1 - y0);
                    if (0.0..=1.0).contains(&t) {
                        visit(x0 + t * (x1 - x0));
                    }
                }
            }
        }
        if min_x.is_finite() && max_x.is_finite() && min_x < max_x {
            Some((min_x, max_x))
        } else {
            None
        }
    }
}

/// Compose `translate(dx, dy)` *after* an existing IDML affine.
/// `translate ∘ inner` applied to a point: first inner maps the
/// point, then translate shifts it by (dx, dy). Used by the master-
/// overlay pass to push master-spread coords into the live spread.
/// `None` becomes a pure translation.
/// Stamp a master item: compose its inner `item_transform` (item →
/// master-spread coords) under the page's outer master-overlay
/// transform (`translate(live origin) ∘ MasterPageTransform ∘
/// translate(-master origin)`), yielding the item's transform in
/// live-page space. Generalises the former translation-only stamp so a
/// `MasterPageTransform` carrying rotation/scale is honoured; an
/// identity MPT reduces to the same `(dx, dy)` shift as before.
fn compose_outer_matrix(outer: Transform, inner: Option<[f32; 6]>) -> [f32; 6] {
    let inner_t = inner.map(Transform).unwrap_or(Transform::IDENTITY);
    outer.compose(&inner_t).0
}

/// Walk the document's spreads and build per-page wrap-exclusion
/// rectangles in spread coords. Each shape with
/// `TextWrapMode != "None"` contributes its spread-coord bounds
/// inflated by the wrap's offsets. Items without TextWrap, items on
/// no specific page (centroid outside every page bound), and items
/// with active mode `JumpObjectTextWrap` / `NextColumnTextWrap`
/// (which the simple side-shrink heuristic can't model) are skipped.
fn collect_wrap_rects_per_page(
    document: &Document,
    spread_page_ranges: &[std::ops::Range<usize>],
    auto_sized_bounds: &HashMap<String, paged_parse::Bounds>,
) -> Vec<Vec<WrapShape>> {
    let total_pages: usize = spread_page_ranges.last().map(|r| r.end).unwrap_or(0);
    let mut out: Vec<Vec<WrapShape>> = (0..total_pages).map(|_| Vec::new()).collect();
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let range = spread_page_ranges[spread_idx].clone();
        if range.is_empty() {
            continue;
        }
        // Local page bounds for centroid containment routing.
        let page_bounds: Vec<paged_parse::Bounds> = parsed
            .spread
            .pages
            .iter()
            .map(|p| transform_bounds(p.bounds, p.item_transform))
            .collect();
        let route = |aabb: paged_parse::Bounds| -> Option<usize> {
            let cx = (aabb.left + aabb.right) * 0.5;
            let cy = (aabb.top + aabb.bottom) * 0.5;
            page_bounds
                .iter()
                .position(|b| cx >= b.left && cx <= b.right && cy >= b.top && cy <= b.bottom)
        };
        let push = |out: &mut Vec<Vec<WrapShape>>,
                    inner_bounds: paged_parse::Bounds,
                    item_transform: Option<[f32; 6]>,
                    wrap: paged_parse::TextWrap| {
            if !wrap.mode.is_active() {
                return;
            }
            // Treat BoundingBoxTextWrap and ContourTextWrap as
            // bounding-box exclusions. ContourTextWrap with
            // `ContourType=BoundingBox` (the default that InDesign
            // emits for plain rectangle hosts) is identical; richer
            // contour types degrade to their AABB which is still a
            // useful first-cut. JumpObject / NextColumn keep being
            // skipped — they need column-level layout we don't yet
            // model, and approximating them as side-shrink makes
            // matters worse.
            if !matches!(
                wrap.mode,
                paged_parse::TextWrapMode::BoundingBoxTextWrap
                    | paged_parse::TextWrapMode::ContourTextWrap
            ) {
                return;
            }
            let shape = WrapShape::from_inner(inner_bounds, item_transform, wrap.offsets);
            if let Some(local_idx) = route(shape.bounds) {
                let page_idx = range.start + local_idx;
                if page_idx < out.len() {
                    out[page_idx].push(shape);
                }
            }
        };
        for f in &parsed.spread.text_frames {
            if let Some(w) = f.text_wrap {
                // W1.7 Phase B: a neighbouring frame wraps around the
                // GROWN box of an auto-sized frame, not its authored
                // undersized rect. Substitute the precomputed grown
                // inner-coord bounds when this frame auto-sizes so the
                // exclusion rect matches the painted box.
                let wrap_bounds = f
                    .self_id
                    .as_deref()
                    .and_then(|id| auto_sized_bounds.get(id))
                    .copied()
                    .unwrap_or(f.bounds);
                push(&mut out, wrap_bounds, f.item_transform, w);
            }
        }
        for r in &parsed.spread.rectangles {
            if let Some(w) = r.text_wrap {
                push(&mut out, r.bounds, r.item_transform, w);
            }
        }
        for o in &parsed.spread.ovals {
            if let Some(w) = o.text_wrap {
                push(&mut out, o.bounds, o.item_transform, w);
            }
        }
        for p in &parsed.spread.polygons {
            if let Some(w) = p.text_wrap {
                push(&mut out, p.bounds, p.item_transform, w);
            }
        }
        for l in &parsed.spread.graphic_lines {
            if let Some(w) = l.text_wrap {
                push(&mut out, l.bounds, l.item_transform, w);
            }
        }
    }
    out
}

/// `CellStyle/$ID/[None]` is IDML's "no style" sentinel. Treat it
/// as absent so the region cascade kicks in.
fn is_none_style_id(id: &str) -> bool {
    id == "CellStyle/$ID/[None]" || id == "CellStyle/n" || id.is_empty()
}

/// True for swatch IDs that resolve to "no paint" — used by per-cell
/// stroke override to fall through to the cascaded cell-style colour
/// when the inline `<Cell>` attribute carries `Swatch/None`.
fn is_none_swatch_id(id: &str) -> bool {
    // Concept 2 — routed through the shared reserved-swatch
    // classifier; behaviour identical (plus the `Color/None`
    // spelling the canvas-side sites match).
    paged_parse::graphic::ReservedSwatch::is_none(id)
}

/// True when an `Option<String>` FillColor on a page item should be
/// treated as fully transparent — i.e. no background rect should be
/// emitted at all. Mirrors InDesign's behaviour for both "FillColor
/// attribute absent" and `FillColor="Swatch/None"`. Distinct from the
/// "palette lookup miss" case — when an id is present but unresolved
/// the renderer still falls back to the gray preview swatch.
pub(crate) fn frame_fill_is_transparent(id: Option<&str>) -> bool {
    match id {
        None => true,
        Some(s) => is_none_swatch_id(s),
    }
}

/// True when the frame's stroke would actually paint pixels — i.e.
/// `StrokeColor` resolves to a non-`Swatch/None` paint AND
/// `StrokeWeight > 0`. The drop-shadow module uses this to gate
/// stroke shadows: a stroke shadow without a visible stroke would
/// otherwise leak as a stamped rectangle behind an outline that
/// isn't drawn.
pub(crate) fn frame_stroke_is_visible(stroke_color: Option<&str>, stroke_weight: f32) -> bool {
    if stroke_weight <= 0.0 {
        return false;
    }
    match stroke_color {
        None => false,
        Some(s) => !is_none_swatch_id(s),
    }
}

/// Map an IDML `FontStyle` attribute string to a numeric wght axis
/// value (CSS / fvar convention: 100=Thin, 400=Regular, 700=Bold,
/// 900=Black). Unknown values fall through to 400. Italic / Bold
/// Italic are matched on substring so combined styles still get
/// the right weight; the italic axis is handled separately by
/// loading a different font file (resolver-side).
fn wght_for_font_style(style: Option<&str>) -> f32 {
    let s = match style {
        Some(s) => s,
        None => return 400.0,
    };
    let lower = s.to_ascii_lowercase();
    if lower.contains("thin") || lower.contains("hairline") {
        100.0
    } else if lower.contains("extralight")
        || lower.contains("extra light")
        || lower.contains("ultralight")
    {
        200.0
    } else if lower.contains("light") {
        300.0
    } else if lower.contains("medium") {
        500.0
    } else if lower.contains("semibold")
        || lower.contains("semi bold")
        || lower.contains("demibold")
        || lower.contains("demi bold")
    {
        600.0
    } else if lower.contains("extrabold")
        || lower.contains("extra bold")
        || lower.contains("ultrabold")
    {
        800.0
    } else if lower.contains("bold") {
        700.0
    } else if lower.contains("black") || lower.contains("heavy") {
        900.0
    } else {
        400.0
    }
}

/// Split a paragraph at every `\n` boundary in any run's text into
/// a sequence of sub-paragraphs, each inheriting the parent's
/// style. Used to honour IDML `<Br/>` (which serialises as `\n`)
/// as a forced line break: the layout engine sees each sub-
/// paragraph independently, so successive bullet items / address
/// lines / etc. land on their own rows rather than collapsing
/// into glue-separated runs of one paragraph.
///
/// `SpaceBefore` is suppressed on every sub-paragraph past the
/// first so consecutive lines in the same logical paragraph don't
/// accumulate extra leading. `tab_list` and other paragraph
/// metadata copy through unchanged.
fn split_paragraph_at_breaks(paragraph: &paged_parse::Paragraph) -> Vec<paged_parse::Paragraph> {
    // Walk runs in order; for each run, split text at '\n' and
    // emit the leading segment into the in-progress sub-paragraph,
    // then close the sub-paragraph and start a new one.
    let mut subs: Vec<paged_parse::Paragraph> = Vec::new();
    let mut current = paged_parse::Paragraph {
        paragraph_style: paragraph.paragraph_style.clone(),
        justification: paragraph.justification,
        first_line_indent: paragraph.first_line_indent,
        // W0.2 — left/right indent and the rule structs are
        // whole-paragraph attributes; every split sub-paragraph
        // inherits them (same convention as kinsoku / indents below).
        left_indent: paragraph.left_indent,
        right_indent: paragraph.right_indent,
        hyphenation: paragraph.hyphenation,
        keep_lines_together: paragraph.keep_lines_together,
        keep_with_next: paragraph.keep_with_next,
        rule_above: paragraph.rule_above.clone(),
        rule_below: paragraph.rule_below.clone(),
        space_before: paragraph.space_before,
        space_after: None, // applied to last sub-paragraph only
        tab_list: paragraph.tab_list.clone(),
        bullets_list_type: paragraph.bullets_list_type.clone(),
        bullet_character: paragraph.bullet_character,
        numbering_format: paragraph.numbering_format.clone(),
        applied_numbering_list: paragraph.applied_numbering_list.clone(),
        // Drop-cap + anchored frames carry on the FIRST sub-paragraph
        // only; the splits below clone from the source paragraph and
        // overwrite these to defaults so the cap doesn't repeat.
        drop_cap_characters: paragraph.drop_cap_characters,
        drop_cap_lines: paragraph.drop_cap_lines,
        drop_cap_detail: paragraph.drop_cap_detail,
        overprint_fill: paragraph.overprint_fill,
        overprint_stroke: paragraph.overprint_stroke,
        // Kinsoku / Mojikumi apply to the whole paragraph; every
        // split sub-paragraph inherits the same set.
        kinsoku_set: paragraph.kinsoku_set.clone(),
        kinsoku_type: paragraph.kinsoku_type.clone(),
        mojikumi_table: paragraph.mojikumi_table.clone(),
        mojikumi_set: paragraph.mojikumi_set.clone(),
        anchored_frames: paragraph.anchored_frames.clone(),
        runs: Vec::new(),
        table: None,
        // Phase 5 — footnotes / index markers ride the FIRST
        // sub-paragraph only (matches the anchored-frame +
        // drop-cap convention above); subsequent splits start with
        // empty vecs so the markers don't duplicate.
        footnotes: paragraph.footnotes.clone(),
        index_markers: paragraph.index_markers.clone(),
    };
    for run in &paragraph.runs {
        if !run.text.contains('\n') {
            current.runs.push(run.clone());
            continue;
        }
        let segments: Vec<&str> = run.text.split('\n').collect();
        for (i, seg) in segments.iter().enumerate() {
            if !seg.is_empty() {
                let mut copy = run.clone();
                copy.text = (*seg).to_string();
                current.runs.push(copy);
            }
            if i + 1 < segments.len() {
                // If the about-to-be-closed sub-paragraph has no runs
                // (the previous segment ended with a `\n` and produced
                // a paragraph terminator straight away), surface the
                // run's character attributes via a zero-text run so
                // the empty-paragraph emit branch can read its
                // PointSize. Without this, an empty paragraph inside
                // a 24pt `<Br/><Br/>` falls through to the paragraph
                // style's PointSize (or the default 12pt), collapsing
                // the leading from 28.8pt to 14.4pt.
                if current.runs.is_empty() {
                    let mut hint = run.clone();
                    hint.text = String::new();
                    current.runs.push(hint);
                }
                // Close the current sub-paragraph and start a new
                // one. Discard empty sub-paragraphs (consecutive
                // `\n`s, common at the end of bullet lists).
                let mut next = paged_parse::Paragraph {
                    paragraph_style: paragraph.paragraph_style.clone(),
                    justification: paragraph.justification,
                    first_line_indent: paragraph.first_line_indent,
                    // W0.2 — whole-paragraph attributes carry to every
                    // split sub-paragraph (kinsoku convention).
                    left_indent: paragraph.left_indent,
                    right_indent: paragraph.right_indent,
                    hyphenation: paragraph.hyphenation,
                    keep_lines_together: paragraph.keep_lines_together,
                    keep_with_next: paragraph.keep_with_next,
                    rule_above: paragraph.rule_above.clone(),
                    rule_below: paragraph.rule_below.clone(),
                    space_before: None,
                    space_after: None,
                    tab_list: paragraph.tab_list.clone(),
                    bullets_list_type: paragraph.bullets_list_type.clone(),
                    bullet_character: paragraph.bullet_character,
                    numbering_format: paragraph.numbering_format.clone(),
                    applied_numbering_list: paragraph.applied_numbering_list.clone(),
                    // Drop cap + anchored frames are first-paragraph-only;
                    // sub-paragraphs after a `\n` reset to defaults.
                    drop_cap_characters: 0,
                    drop_cap_lines: 0,
                    drop_cap_detail: 0,
                    overprint_fill: paragraph.overprint_fill,
                    overprint_stroke: paragraph.overprint_stroke,
                    // Kinsoku / Mojikumi apply to the whole paragraph.
                    kinsoku_set: paragraph.kinsoku_set.clone(),
                    kinsoku_type: paragraph.kinsoku_type.clone(),
                    mojikumi_table: paragraph.mojikumi_table.clone(),
                    mojikumi_set: paragraph.mojikumi_set.clone(),
                    anchored_frames: Vec::new(),
                    runs: Vec::new(),
                    table: None,
                    // Sub-paragraphs after a `\n` reset markers too
                    // (matches anchored-frame convention above).
                    footnotes: Vec::new(),
                    index_markers: Vec::new(),
                };
                std::mem::swap(&mut current, &mut next);
                // Keep empty sub-paragraphs — `<Br/><Br/>` and similar
                // patterns mean "advance one line of vertical space".
                // The emitter renders them as a single line-height
                // step (no glyphs) so the surrounding text keeps its
                // visual rhythm.
                subs.push(next);
            }
        }
    }
    // Flush the trailing sub-paragraph + propagate the original
    // SpaceAfter so the chain's vertical spacing matches.
    if !current.runs.is_empty() {
        current.space_after = paragraph.space_after;
        subs.push(current);
    } else if let Some(last) = subs.last_mut() {
        last.space_after = paragraph.space_after;
    }
    // P-25 guard: drop a trailing sub-paragraph whose every run is
    // empty or `\n`-only. The split loop above already discards the
    // `current` working sub when its runs vec is empty, but a
    // pathological run carrying ONLY `\n` characters in its text
    // would seed a sub with a zero-text hint run (set at line ~5891)
    // that has no visible glyphs yet still triggers bullet-marker
    // emission for NumberedList paragraphs. Drop those at the tail
    // so the numbering counter doesn't double-fire on the visible
    // line. Stops short of dropping interior empty sub-paragraphs
    // because consecutive `<Br/>` pairs intentionally render as
    // empty vertical-leading slots.
    while subs.len() > 1
        && subs
            .last()
            .map(|p| {
                p.runs
                    .iter()
                    .all(|r| r.text.is_empty() || r.text.chars().all(|c| c == '\n'))
            })
            .unwrap_or(false)
    {
        // Carry the dropped tail's space_after over to the new last.
        let dropped = subs.pop().expect("len > 1 just checked");
        if let Some(last) = subs.last_mut() {
            last.space_after = last.space_after.or(dropped.space_after);
        }
    }
    if subs.is_empty() {
        // Defensive: the original was all `\n`s. Return a single
        // empty paragraph to keep the upstream loop's stat
        // bookkeeping consistent without rendering anything.
        subs.push(paged_parse::Paragraph {
            paragraph_style: paragraph.paragraph_style.clone(),
            justification: paragraph.justification,
            first_line_indent: paragraph.first_line_indent,
            // W0.2 — whole-paragraph attributes (carry from source).
            left_indent: paragraph.left_indent,
            right_indent: paragraph.right_indent,
            hyphenation: paragraph.hyphenation,
            keep_lines_together: paragraph.keep_lines_together,
            keep_with_next: paragraph.keep_with_next,
            rule_above: paragraph.rule_above.clone(),
            rule_below: paragraph.rule_below.clone(),
            space_before: paragraph.space_before,
            space_after: paragraph.space_after,
            tab_list: paragraph.tab_list.clone(),
            bullets_list_type: paragraph.bullets_list_type.clone(),
            bullet_character: paragraph.bullet_character,
            numbering_format: paragraph.numbering_format.clone(),
            applied_numbering_list: paragraph.applied_numbering_list.clone(),
            // All-`\n` source paragraph: defensive placeholder.
            // Drop cap + anchored frames don't apply to a glyph-less
            // paragraph; default them.
            drop_cap_characters: 0,
            drop_cap_lines: 0,
            drop_cap_detail: 0,
            overprint_fill: paragraph.overprint_fill,
            overprint_stroke: paragraph.overprint_stroke,
            kinsoku_set: paragraph.kinsoku_set.clone(),
            kinsoku_type: paragraph.kinsoku_type.clone(),
            mojikumi_table: paragraph.mojikumi_table.clone(),
            mojikumi_set: paragraph.mojikumi_set.clone(),
            anchored_frames: Vec::new(),
            runs: Vec::new(),
            table: None,
            footnotes: Vec::new(),
            index_markers: Vec::new(),
        });
    }
    subs
}

/// Measure-only pass for one cell paragraph: shapes + lays out at
/// `column_width_pt` and returns the vertical extent the paragraph
/// would consume, without emitting glyphs. Mirrors
/// [`emit_cell_paragraph`]'s layout half so content-driven row
/// growth can sum cell heights before committing row geometry.
///
/// Phase 5 — footnote pool emit. For every page that captured
/// footnotes during the story pass, lay out the footnote bodies at
/// the bottom of the host frame's content area. Bodies stack
/// upward from the frame bottom; per-page running numbers prefix
/// each body ("1. body text").
///
/// W1.8 scope: footnote bodies compose through the SAME styled-run path
/// as body text — per-run point size, weight (bold/italic via the wght
/// axis), and `FillColor` are honoured (`compose_footnote_paragraphs`).
/// The document's `<FootnoteOption>` separator rule is drawn above the
/// pool from real designmap settings, and the W1.7 reserve-then-fill
/// pass keeps bodies clear of the body text.
///
/// Deferred:
/// - Anchor superscript substitution at the host paragraph (the inline
///   footnote-reference number),
/// - Cross-frame footnote SPLITTING — a single footnote taller than the
///   remaining column is not split across frames; it overruns and is
///   reported via a `FootnoteOverflow` diagnostic (see the dated note on
///   `emit_footnote_pools`).
///
/// Perf-BodyStory — signature for a story's emission inputs. Hashes
/// the frame chain's (self_id, bounds, item_transform) plus the
/// wrap_rects on each chain page. A gesture that moves a frame
/// outside this set leaves the signature unchanged → cache hit.
/// Moving a frame INSIDE the chain or a frame whose wrap rect
/// lives on a chain page bumps the signature → cache miss + fresh
/// capture.
fn body_story_signature(
    chain: &[&TextFrame],
    chain_pages: &[usize],
    wrap_rects_per_page: &[Vec<WrapShape>],
) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    // Chain — frame identity + geometry.
    chain.len().hash(&mut h);
    for f in chain {
        f.self_id.as_deref().unwrap_or("").hash(&mut h);
        f.bounds.top.to_bits().hash(&mut h);
        f.bounds.left.to_bits().hash(&mut h);
        f.bounds.bottom.to_bits().hash(&mut h);
        f.bounds.right.to_bits().hash(&mut h);
        match f.item_transform {
            Some(m) => {
                1u8.hash(&mut h);
                for v in &m {
                    v.to_bits().hash(&mut h);
                }
            }
            None => 0u8.hash(&mut h),
        }
    }
    // Wrap rects on chain pages — captures wrap-causing frames'
    // movements on pages this story touches. Other pages' wrap
    // changes don't affect this story's line breaking.
    for &page in chain_pages {
        // The page INDEX is part of the key: insert/delete-page shifts
        // the chain's absolute page indices while the frame geometry
        // stays identical, and the cached delta's per_page entries are
        // keyed by absolute index — a stale hit would splice commands
        // into the wrong page, or out of bounds entirely (the
        // editor-suite insertPage-mid-set panic).
        page.hash(&mut h);
        if let Some(rects) = wrap_rects_per_page.get(page) {
            1u8.hash(&mut h);
            rects.len().hash(&mut h);
            for r in rects {
                r.bounds.top.to_bits().hash(&mut h);
                r.bounds.left.to_bits().hash(&mut h);
                r.bounds.bottom.to_bits().hash(&mut h);
                r.bounds.right.to_bits().hash(&mut h);
                for (cx, cy) in &r.corners {
                    cx.to_bits().hash(&mut h);
                    cy.to_bits().hash(&mut h);
                }
            }
        } else {
            0u8.hash(&mut h);
        }
    }
    h.finish()
}

/// Perf-MasterText — splice a cached delta into a page's display
/// list. Appends the delta's path entries (via `push_anon`, no
/// intern dedup — the rebuild's master+frame pass may have already
/// interned the same glyph outlines under different ids, but that
/// wastes a few path slots and not correctness), then pushes the
/// cached commands with their relative path-ids rebased to the
/// page's NEW path-buffer base.
fn splice_master_text_delta(list: &mut paged_compose::DisplayList, delta: &MasterTextEmitDelta) {
    let new_base = list.paths.len() as i64;
    for path in &delta.paths {
        list.paths.push_anon(path.clone());
    }
    for cmd in &delta.commands {
        let mut c = cmd.clone();
        rebase_path_ids(&mut c, new_base);
        list.commands.push(c);
    }
}

/// Perf-BodyStory — splice one page's captured body-story emission
/// into a `BuiltPage`: rebase + push the path+command delta, and
/// extend `story_layout` + `footnotes` so caret / hit-test /
/// footnote queries match a from-scratch emit.
fn splice_body_story_page_delta(page: &mut BuiltPage, delta: &BodyStoryPageDelta) {
    let new_base = page.list.paths.len() as i64;
    for path in &delta.paths {
        page.list.paths.push_anon(path.clone());
    }
    for cmd in &delta.commands {
        let mut c = cmd.clone();
        rebase_path_ids(&mut c, new_base);
        page.list.commands.push(c);
    }
    page.story_layout.extend(delta.story_layout.iter().cloned());
    page.footnotes.extend(delta.footnotes.iter().cloned());
}

/// Perf-MasterText — adds `offset` to every PathId field on a
/// DisplayCommand. Used (1) at capture-time with `offset = -base`
/// to rebase to relative ids, and (2) at replay-time with
/// `offset = new_base` to rebase the cached relative ids to the
/// active path-buffer position. Variants without a path_id field
/// are no-ops.
fn rebase_path_ids(cmd: &mut paged_compose::DisplayCommand, offset: i64) {
    use paged_compose::DisplayCommand::*;
    let add = |pid: &mut paged_compose::PathId| {
        let v = pid.0 as i64 + offset;
        pid.0 = v as u32;
    };
    match cmd {
        FillPath { path_id, .. } => add(path_id),
        FillPathBlend { path_id, .. } => add(path_id),
        StrokePath { path_id, .. } => add(path_id),
        DropShadow { path_id, .. } => add(path_id),
        PathShadow { path_id, .. } => add(path_id),
        PushClip { path_id, .. } => add(path_id),
        InnerShadow { path_id, .. } => add(path_id),
        OuterGlow { path_id, .. } => add(path_id),
        InnerGlow { path_id, .. } => add(path_id),
        BevelEmboss { path_id, .. } => add(path_id),
        Satin { path_id, .. } => add(path_id),
        Feather { path_id, .. } => add(path_id),
        DirectionalFeather { path_id, .. } => add(path_id),
        GradientFeather { path_id, .. } => add(path_id),
        FillPathOverprint { path_id, .. } => add(path_id),
        StrokePathOverprint { path_id, .. } => add(path_id),
        // Variants without a path_id field — no-op.
        Image { .. }
        | PopClip(_)
        | BeginBlendGroup { .. }
        | EndBlendGroup(_)
        | PushLayer { .. }
        | PopLayer(_) => {}
    }
}

/// W1.7 — default footnote body point size, used when a footnote run
/// carries no resolved `PointSize` (and the paragraph/character style
/// cascade likewise declares none). Real InDesign footnotes inherit
/// the `[Footnote]` paragraph style's size; absent that we fall back
/// to 8pt, the long-standing footnote convention.
///
/// W1.8 — footnote bodies now compose through the SAME styled-run path
/// as body text (per-run size/weight/colour). This constant is only
/// the per-run *fallback*; the composition + the space-reservation
/// measurement share [`compose_footnote_paragraphs`], so they remain
/// pixel-locked regardless of per-run size shifts.
const FOOTNOTE_POINT_SIZE: f32 = 8.0;

/// W1.7 — bail cap for the footnote space-reservation fixpoint loop.
/// Pass 0 composes with no reservation and measures; pass 1 re-composes
/// against the measured pool; a third pass catches the rare case where
/// the pass-1 reflow pushed a footnote across a frame boundary and
/// changed a pool height. Two re-composes settle every realistic
/// layout; the cap guarantees termination even if a pathological
/// document oscillates, in which case the last pass's result (an
/// overlay, never dropped text) is accepted.
const MAX_FOOTNOTE_RESERVE_PASSES: usize = 3;

/// W1.8 — the styled, laid-out form of one footnote's body, shared by
/// the space-reservation measure and the pool emit so they agree to the
/// pixel. Each entry is one body paragraph (the leading paragraph also
/// carries the `"N." + separator` marker prefix) laid out into the
/// column width through the SAME multi-font `layout_runs` path as body
/// text. `height_pt` is the stacked line height of all its lines.
struct ComposedFootnote {
    /// One laid-out paragraph per source footnote paragraph.
    paragraphs: Vec<paged_text::LaidOutParagraph>,
    /// Per-paragraph height in pt (sum of its line heights).
    para_heights_pt: Vec<f32>,
    /// Total height of this footnote in pt (Σ `para_heights_pt`).
    height_pt: f32,
    /// Outline bytes keyed by the `font_id` carried on each positioned
    /// glyph, so the emit pass can build a [`TtfOutliner`] per font
    /// group (a footnote that mixes faces/weights needs more than one).
    font_outline_bytes: HashMap<u32, Bytes>,
    /// Per-run resolved attrs, one Vec per source paragraph, so the
    /// emit pass can build a per-cluster paint picker (per-run
    /// `FillColor`/tint) matching the styled-run text.
    resolved_runs_per_para: Vec<Vec<paged_scene::ResolvedRunAttrs>>,
    /// Per-run SHAPED text byte lengths (marker folded onto run 0 of
    /// paragraph 0), parallel to `resolved_runs_per_para`. Drives the
    /// paint picker's band offsets.
    run_text_lens_per_para: Vec<Vec<usize>>,
}

/// W1.8 — vertical line advance for a footnote line, mirroring the
/// auto-leading body text uses (`point_size × 1.2`). Computed from the
/// dominant point size on the line so a footnote that mixes sizes still
/// leaves room for its tallest glyphs.
fn footnote_line_height_pt(line: &paged_text::layout::LaidOutLine) -> f32 {
    let max_size = line
        .glyphs
        .iter()
        .map(|g| g.point_size)
        .fold(0.0_f32, f32::max);
    let size = if max_size > 0.0 {
        max_size
    } else {
        FOOTNOTE_POINT_SIZE
    };
    size * 1.2
}

/// W1.8 — compose one footnote's body into laid-out paragraphs through
/// the styled-run path. Each source run resolves its own
/// `PointSize` / `FontStyle` (bold/italic via the `wght` axis) /
/// `FillColor` / tracking / baseline-shift exactly like body text, so a
/// footnote with mixed styling renders faithfully instead of flattening
/// to a single face + size.
///
/// `column_width_pt` is the host frame's content width; `separator` is
/// the document `FootnoteOption/SeparatorText` (already marker-expanded)
/// inserted between the number and the body. Returns `None` when no run
/// resolves to any font (nothing to shape) — the caller skips it.
fn compose_footnote_paragraphs(
    fn_: &EmittedFootnote,
    document: &Document,
    font_table: &FontTable,
    column_width_pt: f32,
    separator: &str,
    default_size: f32,
) -> Option<ComposedFootnote> {
    if column_width_pt <= 0.0 {
        return None;
    }
    let marker = format!("{}{}", fn_.number, separator);
    let mut paragraphs: Vec<paged_text::LaidOutParagraph> = Vec::new();
    let mut para_heights_pt: Vec<f32> = Vec::new();
    let mut total_h_pt = 0.0f32;
    let mut font_outline_bytes: HashMap<u32, Bytes> = HashMap::new();
    let mut resolved_runs_per_para: Vec<Vec<paged_scene::ResolvedRunAttrs>> = Vec::new();
    let mut run_text_lens_per_para: Vec<Vec<usize>> = Vec::new();

    for (p_idx, para) in fn_.paragraphs.iter().enumerate() {
        // Resolve every run's attrs against the footnote paragraph's
        // own style cascade — the footnote body parses into the same
        // Paragraph/CharacterRun shape as a top-level story, so the
        // standard resolver applies directly.
        let resolved_runs: Vec<paged_scene::ResolvedRunAttrs> = para
            .runs
            .iter()
            .map(|r| document.resolved_run_attrs(para, r))
            .collect();
        let bytes_pool = match font_table.resolve_paragraph_bytes(&resolved_runs) {
            Some(b) => b,
            None => continue,
        };
        let wghts: Vec<f32> = resolved_runs
            .iter()
            .map(|r| wght_for_font_style(r.font_style.as_deref()))
            .collect();

        // Build one shaping Face per (bytes, wght). Built on the fly
        // here (rather than via the per-render face cache) — footnote
        // pools are small, so the extra zero-copy Face construction is
        // negligible and keeps this path independent of the cache's
        // harvest pass (which never sees footnote stories).
        let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
        let mut owned_faces: Vec<Option<rustybuzz::Face>> = Vec::with_capacity(bytes_pool.len());
        for (i, b) in bytes_pool.iter().enumerate() {
            let face = rustybuzz::Face::from_slice(b.as_ref(), 0).map(|mut rf| {
                let has_wght = ttf_parser::Face::parse(b.as_ref(), 0)
                    .ok()
                    .map(|of| of.variation_axes().into_iter().any(|a| a.tag == wght_tag))
                    .unwrap_or(false);
                if has_wght {
                    rf.set_variations(&[rustybuzz::Variation {
                        tag: wght_tag,
                        value: wghts[i],
                    }]);
                }
                rf
            });
            owned_faces.push(face);
        }
        if owned_faces.iter().any(|f| f.is_none()) {
            continue;
        }

        // font_id mixes in the wght so the glyph-outline cache doesn't
        // conflate two weights of one variable font.
        let font_ids: Vec<u32> = bytes_pool
            .iter()
            .zip(wghts.iter())
            .map(|(b, w)| fnv_1a_u32(b.as_ref()) ^ w.to_bits())
            .collect();
        for (fid, b) in font_ids.iter().zip(bytes_pool.iter()) {
            font_outline_bytes.entry(*fid).or_insert_with(|| b.clone());
        }

        // Owned shaped texts: the marker prefix rides on the FIRST run
        // of the FIRST paragraph (inheriting that run's style — matching
        // InDesign, where the footnote number takes the body style
        // unless a Footnote marker character style overrides it, a
        // follow-up). Owned so the `&str` views in `StyledRun` outlive
        // the layout call.
        let run_texts: Vec<String> = para
            .runs
            .iter()
            .enumerate()
            .map(|(i, run)| {
                if p_idx == 0 && i == 0 {
                    format!("{marker}{}", run.text)
                } else {
                    run.text.clone()
                }
            })
            .collect();
        let run_text_lens: Vec<usize> = run_texts.iter().map(|t| t.len()).collect();

        let styled_runs: Vec<paged_text::StyledRun> = para
            .runs
            .iter()
            .enumerate()
            .map(|(i, _run)| {
                let base_size = resolved_runs[i].point_size.unwrap_or(default_size);
                let (point_size, baseline_shift_pt) = position_adjusted_metrics(
                    base_size,
                    resolved_runs[i].baseline_shift,
                    resolved_runs[i].position.as_deref(),
                );
                paged_text::StyledRun {
                    text: run_texts[i].as_str(),
                    face: owned_faces[i].as_ref().unwrap(),
                    point_size,
                    tracking: resolved_runs[i].tracking,
                    font_id: font_ids[i],
                    underline: resolved_runs[i].underline.unwrap_or(false),
                    strikethru: resolved_runs[i].strikethru.unwrap_or(false),
                    baseline_shift_pt,
                    horizontal_scale_pct: resolved_runs[i].horizontal_scale.unwrap_or(100.0),
                    vertical_scale_pct: resolved_runs[i].vertical_scale.unwrap_or(100.0),
                    skew_deg: resolved_runs[i].skew.unwrap_or(0.0),
                    fallback_faces: &[],
                    shaping_features: shaping_features_from(
                        resolved_runs[i].ligatures_on,
                        resolved_runs[i].kerning_method.as_deref(),
                        &resolved_runs[i].otf,
                    ),
                }
            })
            .collect();
        if styled_runs.is_empty() {
            continue;
        }

        let mut lopts = paged_text::LayoutOptions::new(column_width_pt, default_size);
        lopts.alignment = paged_text::Alignment::Left;
        let laid = paged_text::cache::layout_runs_cached(&styled_runs, &lopts);
        // `styled_runs` (which borrows `resolved_runs`, `owned_faces`,
        // `run_texts`) is no longer needed after layout; drop it so
        // `resolved_runs` can move into the returned struct.
        drop(styled_runs);
        let h: f32 = laid.lines.iter().map(footnote_line_height_pt).sum();
        let h = if laid.lines.is_empty() {
            default_size * 1.2
        } else {
            h
        };
        para_heights_pt.push(h);
        total_h_pt += h;
        paragraphs.push(laid);
        resolved_runs_per_para.push(resolved_runs);
        run_text_lens_per_para.push(run_text_lens);
    }

    if paragraphs.is_empty() {
        return None;
    }
    Some(ComposedFootnote {
        paragraphs,
        para_heights_pt,
        height_pt: total_h_pt,
        font_outline_bytes,
        resolved_runs_per_para,
        run_text_lens_per_para,
    })
}

/// W1.8 — total pool height (pt) for one frame's footnote group, laid
/// out exactly as [`emit_footnote_pools`] draws it: each footnote
/// composed through [`compose_footnote_paragraphs`], plus the
/// `space_between` gap between consecutive footnotes and the separator
/// rule's vertical footprint (offset + weight) when the rule is on.
/// Summed across the group, this is the band the body text must vacate.
fn footnote_pool_height_pt(
    group: &[&EmittedFootnote],
    document: &Document,
    font_table: &FontTable,
    column_width_pt: f32,
    metrics: &FootnoteMetrics,
) -> f32 {
    if column_width_pt <= 0.0 {
        return 0.0;
    }
    let mut total_h_pt = metrics.rule_band_pt();
    for (i, fn_) in group.iter().enumerate() {
        if let Some(c) = compose_footnote_paragraphs(
            fn_,
            document,
            font_table,
            column_width_pt,
            &metrics.separator_text,
            metrics.default_size,
        ) {
            total_h_pt += c.height_pt;
            if i + 1 < group.len() {
                total_h_pt += metrics.space_between_pt;
            }
        }
    }
    total_h_pt
}

/// W1.8 — document-level footnote layout metrics, resolved once from the
/// `<FootnoteOption>` settings and shared by the measure + emit passes.
/// Both the separator-text marker and the spacing values come straight
/// from the designmap; absent values fall back to InDesign's defaults.
struct FootnoteMetrics {
    /// Marker→text separator (already `^t`/`^m` expanded), e.g. `"\t"`.
    separator_text: String,
    /// `SpaceBetween` between consecutive footnotes, in pt.
    space_between_pt: f32,
    /// `Spacer`: minimum gap between body bottom and first footnote, pt.
    /// (Folded into the reservation so the pool sits clear of the body.)
    spacer_pt: f32,
    /// Default per-run point size fallback.
    default_size: f32,
    /// Resolved separator-rule spec (`None` when the rule is off).
    rule: Option<FootnoteRuleSpec>,
}

/// W1.8 — resolved separator-rule geometry/paint, ready to stroke.
struct FootnoteRuleSpec {
    weight_pt: f32,
    left_indent_pt: f32,
    width_pt: f32,
    offset_pt: f32,
    paint: Paint,
}

impl FootnoteMetrics {
    /// Vertical space the separator rule occupies above the pool: its
    /// offset plus its stroke weight. Zero when the rule is off.
    fn rule_band_pt(&self) -> f32 {
        self.rule
            .as_ref()
            .map(|r| r.offset_pt.max(0.0) + r.weight_pt.max(0.0))
            .unwrap_or(0.0)
    }
}

/// W1.8 — expand IDML inline markers in a `SeparatorText` value: `^t`
/// → tab, `^m` → em space, `^>` → en space. Unknown `^x` sequences
/// pass through verbatim. The common real-world value is `^t`.
fn expand_separator_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' {
            match chars.peek() {
                Some('t') => {
                    out.push('\t');
                    chars.next();
                }
                Some('m') => {
                    out.push('\u{2003}');
                    chars.next();
                }
                Some('>') => {
                    out.push('\u{2002}');
                    chars.next();
                }
                _ => out.push('^'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// W1.8 — resolve the document's `<FootnoteOption>` into the metrics the
/// pool measure + emit consume. Applies InDesign's defaults for any
/// value the designmap left unset (rule ON, ~0.5pt black rule 50% of the
/// column wide, `". "` separator, no extra spacing).
fn resolve_footnote_metrics(
    document: &Document,
    column_width_pt: f32,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
    default_size: f32,
) -> FootnoteMetrics {
    let fo = &document.container.designmap.footnote_options;
    // Separator: InDesign's factory default is a tab; but our legacy
    // (pre-W1.8) flat path used ". ", and the reservation tests lock to
    // that visual. Honour an explicit SeparatorText; otherwise keep the
    // ". " the rest of the renderer has always produced.
    let separator_text = fo
        .separator_text
        .as_deref()
        .map(expand_separator_markers)
        .unwrap_or_else(|| ". ".to_string());
    let space_between_pt = fo.space_between.unwrap_or(0.0).max(0.0);
    let spacer_pt = fo.spacer.unwrap_or(0.0).max(0.0);

    let rule = if fo.rule_on_effective() {
        // Defaults mirror InDesign's new-document footnote rule: 0.5pt
        // black, full offset 0, indent 0, length = half the column.
        let weight_pt = fo.rule_line_weight.unwrap_or(0.5).max(0.0);
        let left_indent_pt = fo.rule_left_indent.unwrap_or(0.0).max(0.0);
        let width_pt = fo
            .rule_width
            .filter(|w| *w > 0.0)
            .unwrap_or(column_width_pt * 0.5)
            .min((column_width_pt - left_indent_pt).max(0.0));
        let offset_pt = fo.rule_offset.unwrap_or(0.0);
        let base_paint = fo
            .rule_color
            .as_deref()
            .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
            .unwrap_or(Paint::Solid(Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            }));
        let paint = apply_fill_tint(base_paint, fo.rule_tint);
        Some(FootnoteRuleSpec {
            weight_pt,
            left_indent_pt,
            width_pt,
            offset_pt,
            paint,
        })
    } else {
        None
    };

    FootnoteMetrics {
        separator_text,
        space_between_pt,
        spacer_pt,
        default_size,
        rule,
    }
}

/// W1.7/W1.8 — per (page, host-frame) footnote pool heights in pt.
/// Keyed by the same quantised `host_frame_rect_pt` tuple the emit
/// groups by, so the reservation pass can map a pool back to the chain
/// frame that hosts it. Returns an empty map when the document carries
/// no font bytes (footnotes can't be measured or drawn without a face).
fn measure_footnote_pools(
    pages: &[BuiltPage],
    options: &PipelineOptions,
    document: &Document,
    font_table: &FontTable,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
) -> std::collections::HashMap<(usize, i32, i32, i32, i32), f32> {
    let mut out = std::collections::HashMap::new();
    if options.font.is_none() && font_table.fallback.is_none() {
        return out;
    }
    for (page_idx, page) in pages.iter().enumerate() {
        if page.footnotes.is_empty() {
            continue;
        }
        let mut by_frame: std::collections::BTreeMap<(i32, i32, i32, i32), Vec<&EmittedFootnote>> =
            Default::default();
        for fn_ in &page.footnotes {
            by_frame
                .entry(footnote_frame_key(&fn_.host_frame_rect_pt))
                .or_default()
                .push(fn_);
        }
        for (key, group) in by_frame {
            let column_width_pt = group[0].host_frame_rect_pt.w;
            let metrics = resolve_footnote_metrics(
                document,
                column_width_pt,
                palette,
                cmyk_xform,
                FOOTNOTE_POINT_SIZE,
            );
            let h =
                footnote_pool_height_pt(&group, document, font_table, column_width_pt, &metrics)
                    + metrics.spacer_pt;
            if h > 0.0 {
                out.insert((page_idx, key.0, key.1, key.2, key.3), h);
            }
        }
    }
    out
}

/// Quantised grouping key for a host frame's content rect (1/64 pt),
/// shared by the pool emit and the reservation measure so they agree
/// on which footnotes belong to which frame.
fn footnote_frame_key(rect: &paged_compose::Rect) -> (i32, i32, i32, i32) {
    (
        (rect.x * 64.0) as i32,
        (rect.y * 64.0) as i32,
        (rect.w * 64.0) as i32,
        (rect.h * 64.0) as i32,
    )
}

/// W1.7 — the page index and quantised content-rect key a chain frame
/// would capture footnotes under, computed with the EXACT formula
/// [`emit_paragraph_into_chain`] uses (`frame_spread_top_left` minus the
/// page origin, plus L/T insets; width/height minus L+R / T+B insets).
/// Lets the reservation pass map a measured pool back to the chain
/// frame whose text area must shrink. Returns `None` for a frame whose
/// `self_id` doesn't resolve to a page.
fn footnote_host_key_for_frame(
    frame: &TextFrame,
    page_idx: usize,
    pages: &[BuiltPage],
) -> (usize, i32, i32, i32, i32) {
    let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
    let (ox, oy) = pages[page_idx].spread_origin;
    let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
    let frame_w = frame.bounds.width();
    let frame_h = frame.bounds.height();
    let rect = paged_compose::Rect {
        x: sx - ox + insets[1],
        y: sy - oy + insets[0],
        w: (frame_w - insets[1] - insets[3]).max(0.0),
        h: (frame_h - insets[0] - insets[2]).max(0.0),
    };
    let k = footnote_frame_key(&rect);
    (page_idx, k.0, k.1, k.2, k.3)
}

/// W1.7 — per-page display-list + capture lengths plus stats, snapshot
/// before a story's body emit so the footnote space-reservation loop
/// can roll the page back and re-emit with a reduced text area. The
/// re-emit truncates `paths` (cache-aware, [`PathBuffer::truncate_to`]),
/// `commands`, the gradient/image pools, `story_layout`, and
/// `footnotes` to these lengths and restores `stats` — returning the
/// page to exactly its pre-story state. (The body emit never appends to
/// `page.diagnostics`; footnote-overflow diagnostics come from the
/// later pool pass, so they need no rollback here.)
#[derive(Clone, Copy)]
struct BodyStoryPageReset {
    paths: usize,
    commands: usize,
    gradients: usize,
    radial_gradients: usize,
    images: usize,
    story_layout: usize,
    footnotes: usize,
    stats: PipelineStats,
}

fn snapshot_body_story_reset(pages: &[BuiltPage]) -> Vec<BodyStoryPageReset> {
    pages
        .iter()
        .map(|p| BodyStoryPageReset {
            paths: p.list.paths.len(),
            commands: p.list.commands.len(),
            gradients: p.list.gradients.len(),
            radial_gradients: p.list.radial_gradients.len(),
            images: p.list.images.len(),
            story_layout: p.story_layout.len(),
            footnotes: p.footnotes.len(),
            stats: p.stats,
        })
        .collect()
}

fn rollback_body_story(pages: &mut [BuiltPage], snap: &[BodyStoryPageReset]) {
    for (page, s) in pages.iter_mut().zip(snap.iter()) {
        page.list.paths.truncate_to(s.paths);
        page.list.commands.truncate(s.commands);
        page.list.gradients.truncate(s.gradients);
        page.list.radial_gradients.truncate(s.radial_gradients);
        page.list.images.truncate(s.images);
        page.story_layout.truncate(s.story_layout);
        page.footnotes.truncate(s.footnotes);
        page.stats = s.stats;
    }
}

/// W1.7/W1.8 — lay out each page's captured footnote pool at the bottom
/// of its host frame: separator rule, then the footnote bodies composed
/// through the styled-run path, stacked so the last body's bottom sits
/// at the frame's content bottom.
///
/// DEFERRED (2026-06-07, W1.8) — cross-frame footnote SPLITTING.
/// InDesign, when the last footnote on a column doesn't fit, splits that
/// footnote: it keeps the reference line plus as many footnote lines as
/// fit in the current column, and continues the remaining footnote lines
/// in the next column/frame's pool (no repeated number).
///
/// The current model can't express this. First, the pool is laid out in
/// THIS post-pass, AFTER the whole story's body emit has finished and the
/// frame chain is fixed, so there is no live feedback from "footnote line
/// N overflows" back into the body fill to push the reference line
/// forward. Second, the reservation fixpoint (`measure_footnote_pools` →
/// `with_footnote_reservation`) reserves a whole-pool height per frame; it
/// has no notion of a partial footnote or a per-line continuation cursor
/// carrying a remainder to the next frame. Third, `EmittedFootnote` is
/// captured per-page at the host paragraph's starting frame; a split would
/// need ONE footnote to contribute to two pages' pools — a (footnote,
/// line_range, page) fan-out the capture vec doesn't model.
///
/// Design sketch for a future pass: change the reservation loop to reserve
/// only what fits, have the pool emit return an overflow remainder per
/// frame (the unplaced laid-out lines plus a "continued" flag), and thread
/// that remainder into the NEXT chain frame's pool as a leading
/// continuation block before its own captured footnotes. That requires the
/// pool pass to run inside the frame-chain walk (or at least be
/// chain-aware) rather than as a flat per-page post-pass — a multi-day
/// restructure of the StoryEmitter-to-pool boundary. Until then a too-tall
/// footnote overruns the body (overlay) and we fire
/// `DiagnosticCode::FootnoteOverflow` (below) so it is never silent.
fn emit_footnote_pools(
    pages: &mut [BuiltPage],
    font_table: &FontTable,
    options: &PipelineOptions,
    document: &Document,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
) {
    // The footnote pool needs at least one resolvable face. The
    // styled-run composer resolves per-run bytes through the FontTable
    // (which already folds in `options.font` as its fallback), so the
    // only hard requirement is that *some* font is available.
    if options.font.is_none() && font_table.fallback.is_none() {
        return;
    }
    let default_paint = Paint::Solid(Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    for (page_idx, page) in pages.iter_mut().enumerate() {
        if page.footnotes.is_empty() {
            continue;
        }
        // Sort by number (already inserted in order, but defensive).
        let mut pool: Vec<EmittedFootnote> = page.footnotes.clone();
        pool.sort_by_key(|f| f.number);
        // Group by host_frame_rect_pt — render each frame's pool
        // independently. Real-world: most pages have one frame
        // hosting all the footnotes; this loop handles the rare
        // multi-host case too.
        let mut by_frame: std::collections::BTreeMap<(i32, i32, i32, i32), Vec<&EmittedFootnote>> =
            Default::default();
        for fn_ in &pool {
            by_frame
                .entry(footnote_frame_key(&fn_.host_frame_rect_pt))
                .or_default()
                .push(fn_);
        }
        for (_key, group) in by_frame {
            let rect = group[0].host_frame_rect_pt;
            let column_width_pt = rect.w;
            if column_width_pt <= 0.0 {
                continue;
            }
            let metrics = resolve_footnote_metrics(
                document,
                column_width_pt,
                palette,
                cmyk_xform,
                FOOTNOTE_POINT_SIZE,
            );
            // Compose each footnote through the styled-run path (per-run
            // size / weight / colour). Skipping any that resolve to no
            // font, exactly as the measure pass does.
            let composed: Vec<ComposedFootnote> = group
                .iter()
                .filter_map(|fn_| {
                    compose_footnote_paragraphs(
                        fn_,
                        document,
                        font_table,
                        column_width_pt,
                        &metrics.separator_text,
                        metrics.default_size,
                    )
                })
                .collect();
            if composed.is_empty() {
                continue;
            }
            // Pool height = Σ footnote heights + (n-1) inter-footnote
            // gaps + the separator-rule band. Bodies stack so the LAST
            // footnote's bottom sits at the frame's content bottom.
            let n = composed.len();
            let bodies_h: f32 = composed.iter().map(|c| c.height_pt).sum();
            let gaps_h = metrics.space_between_pt * (n.saturating_sub(1)) as f32;
            let rule_band = metrics.rule_band_pt();
            let total_h_pt = bodies_h + gaps_h + rule_band;
            let frame_bottom_pt = rect.y + rect.h;
            // Top of the whole pool (rule + bodies).
            let pool_top_pt = frame_bottom_pt - total_h_pt;
            // First body row sits below the rule band.
            let mut cursor_y_pt = pool_top_pt + rule_band;

            // The pool stacks upward from the frame bottom; when its top
            // rises above the frame's content top it can't fit and
            // overruns the body text. Report it so callers know the
            // render is lossy. Bodies still draw (overlay) — cross-frame
            // continuation is the documented deferral (see the note on
            // `EmittedFootnote`). The diagnostic is what the editor /
            // CLI surfaces for a too-tall footnote.
            if pool_top_pt < rect.y - 0.5 {
                page.diagnostics.push(
                    Diagnostic::new(
                        DiagnosticCode::FootnoteOverflow,
                        "footnote pool is taller than its host frame; bodies overrun the text \
                         (cross-frame footnote splitting is not yet implemented)",
                    )
                    .with_page(page_idx)
                    .with_story(group[0].host_story_id.clone()),
                );
            }

            // Draw the separator rule once, above the first footnote.
            if let Some(spec) = metrics.rule.as_ref() {
                // The rule baseline sits at the bottom of the rule band
                // (i.e. just above the first body row), inset from the
                // pool top by the rule offset.
                let rule_y_pt = pool_top_pt + spec.offset_pt.max(0.0);
                let x0 = rect.x + spec.left_indent_pt;
                let x1 = x0 + spec.width_pt;
                if spec.width_pt > 0.0 && spec.weight_pt > 0.0 {
                    emit_line(
                        x0,
                        rule_y_pt,
                        x1,
                        rule_y_pt,
                        Stroke::new(spec.weight_pt),
                        spec.paint,
                        &mut page.list,
                    );
                }
            }

            // Emit each footnote's glyphs, stacking downward.
            for (fi, c) in composed.iter().enumerate() {
                // Build the per-font outliners for this footnote once.
                let ttf_faces: HashMap<u32, ttf_parser::Face> = c
                    .font_outline_bytes
                    .iter()
                    .filter_map(|(fid, bytes)| {
                        ttf_parser::Face::parse(bytes.as_ref(), 0)
                            .ok()
                            .map(|f| (*fid, f))
                    })
                    .collect();
                for (p_idx, laid) in c.paragraphs.iter().enumerate() {
                    let picker = build_footnote_paint_picker(
                        &c.resolved_runs_per_para[p_idx],
                        &c.run_text_lens_per_para[p_idx],
                        palette,
                        cmyk_xform,
                        default_paint,
                    );
                    let para_top = cursor_y_pt;
                    emit_footnote_paragraph(
                        laid,
                        &ttf_faces,
                        &picker,
                        (rect.x, para_top),
                        &mut page.list,
                    );
                    cursor_y_pt += c.para_heights_pt[p_idx];
                }
                if fi + 1 < composed.len() {
                    cursor_y_pt += metrics.space_between_pt;
                }
            }
        }
    }
}

/// W1.8 — emit one laid-out footnote paragraph's glyphs, grouping by
/// `font_id` so each face/weight uses its own outliner (a footnote that
/// mixes bold + regular needs more than one). `paint_for` returns the
/// per-cluster fill. `frame_origin_pt` is the (x, top-y) the line's
/// glyph positions offset from — `layout_runs` places the first
/// baseline below the top by the line ascent, matching body text.
fn emit_footnote_paragraph(
    laid: &paged_text::LaidOutParagraph,
    ttf_faces: &HashMap<u32, ttf_parser::Face>,
    picker: &RunPaintPicker,
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    for line in &laid.lines {
        let mut start = 0;
        while start < line.glyphs.len() {
            // Group by (font_id, point_size): `emit_glyph_slice` applies
            // ONE point-size scale to the whole slice, so a footnote line
            // that mixes sizes under one face (e.g. an 8pt body with a
            // 10pt inline phrase) must split at every size change or the
            // larger run would render — and be recorded in the glyph-run
            // side channel — at the first glyph's size. Body text never
            // hit this because its composer assigns size via the run's
            // own slice; footnote runs share one fallback font_id.
            let fid = line.glyphs[start].font_id;
            let size = line.glyphs[start].point_size;
            let mut end = start + 1;
            while end < line.glyphs.len()
                && line.glyphs[end].font_id == fid
                && (line.glyphs[end].point_size - size).abs() < 0.01
            {
                end += 1;
            }
            if let Some(face) = ttf_faces.get(&fid) {
                let outliner = TtfOutliner::new(face);
                emit_glyph_slice(
                    &line.glyphs[start..end],
                    fid,
                    size,
                    |cluster| picker.pick(cluster),
                    frame_origin_pt,
                    &outliner,
                    list,
                );
            }
            start = end;
        }
    }
}

struct WrapPlan {
    /// Per-line x-shifts in 1/64 pt. Index `i` = shift for line i.
    line_x_shifts_64: Vec<i32>,
    /// Parallel marker: `twin_after[i] == true` means line `i`
    /// shares a baseline with line `i-1`. Used by the post-layout
    /// pass to implement BothSides wrap (text on both sides of an
    /// obstacle in the same row).
    twin_after: Vec<bool>,
}

/// Polygon vertices for a chain frame, expressed in *spread coords*.
/// Returned only when:
///   - the frame's anchors form a non-rectangular polygon (so AABB
///     layout would place text outside the actual outline);
///   - the frame's `ItemTransform` is upright (identity rotation/scale,
///     translation only). Rotated polygon frames fall back to the
///     AABB path because per-line shifts in spread coords would not
///     compose cleanly with the frame's post-emit rotation.
///
/// `None` means "treat the frame as its AABB" (the legacy behaviour).
fn frame_polygon_spread(frame: &TextFrame) -> Option<Vec<(f32, f32)>> {
    if frame.anchors.len() < 3 {
        return None;
    }
    // Inner-coord rectangularity test: 2 unique x values + 2 unique y
    // values => axis-aligned rect (the common case, every plain text
    // frame). Polygon clipping would be a no-op here; skip.
    let mut xs: Vec<f32> = frame.anchors.iter().map(|a| a.anchor.0).collect();
    let mut ys: Vec<f32> = frame.anchors.iter().map(|a| a.anchor.1).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let eq = |a: f32, b: f32| (a - b).abs() < 1e-3;
    if frame.anchors.len() == 4
        && eq(xs[0], xs[1])
        && eq(xs[2], xs[3])
        && eq(ys[0], ys[1])
        && eq(ys[2], ys[3])
    {
        return None;
    }
    // Only handle upright frames. The renderer rotates rotated text
    // frames post-emit; per-line shifts pre-rotation would interact
    // badly with a non-AABB clip.
    let m = frame
        .item_transform
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let upright = (m[1].abs() < 1e-5)
        && (m[2].abs() < 1e-5)
        && ((m[0] - 1.0).abs() < 1e-5)
        && ((m[3] - 1.0).abs() < 1e-5);
    if !upright {
        return None;
    }
    // Each anchor's straight-segment chain — Bezier control points
    // are approximated by the polyline through `anchor` only, per the
    // implementation plan (curve-flattening can land later without
    // affecting the boundary test above).
    Some(
        frame
            .anchors
            .iter()
            .map(|a| apply_matrix(&m, a.anchor.0, a.anchor.1))
            .collect(),
    )
}

/// W1.10 — build the frame's outline as a [`paged_text::FrameShape`]
/// (one flattened, closed contour per `<GeometryPathType>`, in spread
/// coords) for wrap-INSIDE line layout. Unlike [`frame_polygon_spread`]
/// (which walks anchors only, collapsing ovals to diamonds and ignoring
/// holes), this:
///   * flattens each cubic Bezier edge so ovals / rounded corners
///     conform to the true curve (InDesign stores ovals as four
///     cardinal anchors with 0.5523·r handles — anchors-only would be a
///     diamond);
///   * honours `subpath_starts` so a compound path (donut: outer ring +
///     inner hole) keeps its contours separate — the even-odd scanline
///     in `FrameShape::segments_in_band` then carves the hole.
///
/// Returns `None` (⇒ AABB fallback) for the same cases as
/// `frame_polygon_spread`: fewer than 3 anchors, an axis-aligned rect,
/// or a rotated/sheared frame.
fn frame_shape_spread(frame: &TextFrame) -> Option<paged_text::FrameShape> {
    // Gate on the same rectangularity / upright tests as the clip path
    // so the layout carve and the clip stay in lockstep.
    frame_polygon_spread(frame)?;
    let m = frame
        .item_transform
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let anchors = &frame.anchors;
    // Materialise subpath ranges — same rules as
    // `polygon_path_from_anchors_with_open`: an empty / single-entry
    // `subpath_starts` is one contour over all anchors; otherwise each
    // start opens a contour ending where the next begins.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    if frame.subpath_starts.len() <= 1 {
        ranges.push((0, anchors.len()));
    } else {
        let mut starts: Vec<usize> = frame
            .subpath_starts
            .iter()
            .copied()
            .filter(|&s| s < anchors.len())
            .collect();
        starts.sort_unstable();
        starts.dedup();
        if starts.first() != Some(&0) {
            starts.insert(0, 0);
        }
        for i in 0..starts.len() {
            let lo = starts[i];
            let hi = starts.get(i + 1).copied().unwrap_or(anchors.len());
            if hi > lo {
                ranges.push((lo, hi));
            }
        }
    }
    // Flattening tolerance: 0.5pt deviation from the true curve keeps
    // an oval's chord widths accurate to well under one glyph advance
    // while collapsing straight corner segments to a single edge.
    const FLATTEN_TOL_PT: f32 = 0.5;
    let mut contours: Vec<paged_text::Contour> = Vec::with_capacity(ranges.len());
    for (range_idx, (lo, hi)) in ranges.iter().copied().enumerate() {
        // Open contours describe lassoed strokes / text-on-path hosts,
        // not fillable regions — skip them for the inside test.
        if frame.subpath_open.get(range_idx).copied().unwrap_or(false) {
            continue;
        }
        let sub = &anchors[lo..hi];
        if sub.len() < 2 {
            continue;
        }
        let mut pts: Vec<(f32, f32)> = Vec::new();
        let p0 = apply_matrix(&m, sub[0].anchor.0, sub[0].anchor.1);
        pts.push(p0);
        // Edge between each adjacent anchor + the closing edge back to
        // the first anchor (IDML polygons are closed).
        for k in 0..sub.len() {
            let from = &sub[k];
            let to = &sub[(k + 1) % sub.len()];
            let a = apply_matrix(&m, from.anchor.0, from.anchor.1);
            let c1 = apply_matrix(&m, from.right.0, from.right.1);
            let c2 = apply_matrix(&m, to.left.0, to.left.1);
            let b = apply_matrix(&m, to.anchor.0, to.anchor.1);
            let steps = paged_text::cubic_steps_for_tolerance(a, c1, c2, b, FLATTEN_TOL_PT);
            paged_text::flatten_cubic(a, c1, c2, b, steps, &mut pts);
        }
        // The closing edge re-appended the first anchor; drop the
        // duplicate so the contour is a clean closed ring.
        if pts.len() >= 2 && pts.first() == pts.last() {
            pts.pop();
        }
        contours.push(pts);
    }
    let shape = paged_text::FrameShape::from_contours(contours);
    if shape.is_empty() {
        None
    } else {
        Some(shape)
    }
}

// The polygon scanline / hole-carve geometry that used to live here
// (polygon_x_at_y / pairs_from_xs / carve_holes) moved into
// `paged_text::frame_shape::FrameShape` (W1.10), which adds Bezier
// flattening + whole-band intersection so ovals and compound paths lay
// out correctly. `build_perline_wrap_widths` calls
// `FrameShape::segments_in_band` directly.

fn build_perline_wrap_widths(
    em: &StoryEmitter,
    styled_runs: &[paged_text::StyledRun],
    lopts: &mut paged_text::LayoutOptions,
) -> WrapPlan {
    let empty = WrapPlan {
        line_x_shifts_64: Vec::new(),
        twin_after: Vec::new(),
    };
    // Polygon clip per chain frame — enabled when the frame's
    // <PathGeometry> is non-rectangular (e.g. triangle, pentagon).
    // Indexed by frame_idx; `None` means treat the frame as its AABB.
    // The `FrameShape` carries the *flattened, contour-separated*
    // outline (ovals conform to the curve; compound paths keep their
    // hole) used to carve each line's available x-segments (W1.10);
    // the parallel `chain_polygons` AABB-diamond stays as the cheap
    // gate / legacy fallback for frames whose shape build declines.
    let chain_polygons: Vec<Option<Vec<(f32, f32)>>> =
        em.chain.iter().map(|f| frame_polygon_spread(f)).collect();
    let chain_shapes: Vec<Option<paged_text::FrameShape>> =
        em.chain.iter().map(|f| frame_shape_spread(f)).collect();
    let any_polygon_clip = chain_polygons.iter().any(|p| p.is_some());
    if em.frame_idx != 0 && !any_polygon_clip {
        // After the head frame fills, the existing emit loop
        // advances to chain[1+] using a fixed first-baseline
        // reset; per-line wrap inside overflow frames is layered
        // on by the chain walk below — handled when the head
        // frame's paragraph composes. We still need to engage when
        // a downstream frame is polygon-clipped so paragraphs that
        // start *inside* the polygon get the per-line carve.
        return empty;
    }
    let any_chain_overlap = em
        .chain_spread_bounds
        .iter()
        .zip(em.chain_wrap_rects.iter())
        .any(|(b, ws)| {
            ws.iter().any(|s| {
                let w = s.bounds;
                w.bottom > b.top && w.top < b.bottom && w.right > b.left && w.left < b.right
            })
        });
    if !any_chain_overlap && !any_polygon_clip {
        return empty;
    }
    // Estimate leading from the first run's point size × 1.2.
    // Matches paged-text's auto-leading default.
    let head_size_pt = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let leading_pt = head_size_pt * 1.2;
    let leading_64 = ((leading_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32).max(1);
    let scalar_width_64 =
        (em.column_width_pt.unwrap_or(0.0) * paged_text::shape::ADVANCE_PRECISION).round() as i32;

    let mut widths_64: Vec<i32> = Vec::new();
    let mut shifts_64: Vec<i32> = Vec::new();
    let mut twin_after: Vec<bool> = Vec::new();

    // Walk every frame in the chain. Head frame starts at y_cursor
    // (already accounts for FirstBaselineOffset + SpaceBefore);
    // overflow frames reset to the same first-baseline the existing
    // emit loop uses (`paragraph_size * 0.8`). Each frame contributes
    // its own widths to the combined slice; once layout produces
    // lines the existing emit pass discovers per-line frame
    // assignment and reads x-shifts by absolute line index.
    // Paragraphs that start mid-chain skip the preceding frames so
    // the widths slice starts at the *current* frame.
    let start_frame = em.frame_idx;
    for (frame_idx, frame_bounds) in em.chain_spread_bounds.iter().enumerate() {
        if frame_idx < start_frame {
            continue;
        }
        let frame_left_pt = frame_bounds.left;
        let frame_right_pt = frame_bounds.right;
        let frame = em.chain[frame_idx];
        let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
        let frame_height_pt = frame_bounds.height();
        let frame_first_baseline_64 = if frame_idx == start_frame {
            em.y_cursor.max(0)
        } else {
            (head_size_pt * 0.8 * paged_text::shape::ADVANCE_PRECISION).round() as i32
        };
        let remaining_height_pt = (frame_height_pt
            - frame_first_baseline_64 as f32 / paged_text::shape::ADVANCE_PRECISION)
            .max(0.0);
        let mut n_lines = (remaining_height_pt / leading_pt).ceil() as usize + 1;
        n_lines = n_lines.min(512);
        if n_lines == 0 {
            continue;
        }
        let wraps = &em.chain_wrap_rects[frame_idx];
        let shape = chain_shapes[frame_idx].as_ref();
        // Frames without a shaped outline and without any wrap overlap
        // emit scalar-width entries — preserves the legacy "no
        // per-line carve" behaviour for plain rectangle frames in a
        // chain whose polygon-clipped frame appears later. Without
        // this guard, the AABB-based width for a slightly rotated
        // sibling differs enough from `column_width_pt` to derail
        // Knuth-Plass for the entire story.
        let frame_has_wraps = wraps.iter().any(|s| {
            let w = s.bounds;
            w.bottom > frame_bounds.top
                && w.top < frame_bounds.bottom
                && w.right > frame_bounds.left
                && w.left < frame_bounds.right
        });
        let frame_legacy = shape.is_none() && !frame_has_wraps;
        for i in 0..n_lines {
            if frame_legacy {
                widths_64.push(scalar_width_64);
                shifts_64.push(0);
                twin_after.push(false);
                continue;
            }
            let baseline_pt = (frame_first_baseline_64 + (i as i32) * leading_64) as f32
                / paged_text::shape::ADVANCE_PRECISION;
            // Line's vertical band in spread coords. The band spans the
            // ascent above and descent below the baseline so a glyph's
            // full box — not just its baseline — must fit inside the
            // shape.
            let line_top = frame_bounds.top + baseline_pt - leading_pt * 0.8;
            let line_bottom = frame_bounds.top + baseline_pt + leading_pt * 0.2;

            let frame_inner_left = frame_left_pt + insets[1];
            let frame_inner_right = frame_right_pt - insets[3];
            // Build the *gap list* of open horizontal segments on this
            // line. For shaped frames (ovals, triangles, pentagons,
            // compound paths with holes), seed segments from the
            // outline's interior x-intervals across the line's whole
            // vertical band — so a glyph's box never crosses the actual
            // curve and a circle's top line comes out shorter than its
            // middle line. Plain rectangle frames start from the AABB
            // inner range. The band intersection (vs. a baseline-only
            // sample) and the bezier flattening behind `FrameShape` are
            // the W1.10 upgrade over the prior anchors-only diamond.
            let mut segments: Vec<(f32, f32)> = if let Some(shape) = shape {
                shape
                    .segments_in_band(line_top, line_bottom)
                    .into_iter()
                    .map(|(a, b)| {
                        (
                            (a + insets[1]).max(frame_inner_left),
                            (b - insets[3]).min(frame_inner_right),
                        )
                    })
                    .filter(|(a, b)| b > a)
                    .collect()
            } else {
                vec![(frame_inner_left, frame_inner_right)]
            };
            // Then subtract each intruding wrap shape's x-extent
            // within the line's vertical band. For upright AABBs the
            // extent is the AABB's left/right; for rotated
            // parallelograms the extent is the actual polygon span at
            // this y, which is much narrower than the AABB at the
            // rotated rect's vertical extremes.
            for shape in wraps {
                let aabb = shape.bounds;
                if aabb.bottom <= line_top || aabb.top >= line_bottom {
                    continue;
                }
                let Some((wl, wr)) = shape.x_extent_in_band(line_top, line_bottom) else {
                    continue;
                };
                if wl <= frame_inner_left && wr >= frame_inner_right {
                    continue;
                }
                let mut next: Vec<(f32, f32)> = Vec::with_capacity(segments.len() + 1);
                for (a, b) in &segments {
                    if wr <= *a || wl >= *b {
                        next.push((*a, *b));
                        continue;
                    }
                    if wl > *a {
                        next.push((*a, wl));
                    }
                    if wr < *b {
                        next.push((wr, *b));
                    }
                }
                segments = next;
            }
            // Drop segments narrower than the per-line floor (a band too
            // thin to hold even one word). The widest sub-floor segment
            // is kept aside as a shape-conforming fallback for narrow
            // tips (a circle's poles, a triangle's apex), so glyphs
            // there hug the outline instead of escaping to the full AABB
            // width.
            const MIN_USABLE_64: i32 = 1536; // 24 pt × 64
            let widest_raw = segments.iter().copied().max_by(|x, y| {
                (x.1 - x.0)
                    .partial_cmp(&(y.1 - y.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let usable: Vec<(f32, f32)> = segments
                .into_iter()
                .filter(|(a, b)| {
                    let w64 = ((b - a) * paged_text::shape::ADVANCE_PRECISION).round() as i32;
                    w64 >= MIN_USABLE_64
                })
                .collect();
            if usable.is_empty() {
                // No segment meets the per-line floor at this band.
                //
                // For a SHAPED (wrap-inside) frame, a narrow band still
                // lies inside the outline near a tip — fall back to the
                // widest sub-floor segment (positioned at its real x and
                // floored to MIN_USABLE so the breaker can still seat a
                // word) so the line tracks the shape's centre-line
                // rather than spilling to the AABB. Glyphs that overrun
                // the thin segment are clipped by `apply_polygon_clip`,
                // but they stay centred on the outline's axis.
                //
                // For a wrap-AROUND-objects frame with no shape (and for
                // the degenerate "no segment at all" case), keep the
                // legacy `scalar_width_64` fallback: emitting a 1pt
                // sentinel would make `paragraph_breaker::total_fit`
                // read the slot as "ratio < -1" and prune every active
                // node crossing it, so a paragraph needing more rows
                // than fit before the apex would return zero breaks and
                // the whole story would vanish.
                match (shape.is_some(), widest_raw) {
                    (true, Some((a, b))) if b > a => {
                        // Use the thin segment's ACTUAL width, seated at
                        // its real x. A line here stays as narrow as the
                        // outline allows (so a circle's pole line is
                        // short, not full-width) and centred on the
                        // chord; any glyph that overruns the thin slot is
                        // trimmed by `apply_polygon_clip`. Width is
                        // floored to 1 unit so the breaker still sees a
                        // positive slot.
                        let w64 = (((b - a) * paged_text::shape::ADVANCE_PRECISION).round() as i32)
                            .max(1);
                        let shift_pt = (a - frame_inner_left).max(0.0);
                        widths_64.push(w64);
                        shifts_64
                            .push((shift_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32);
                        twin_after.push(false);
                    }
                    _ => {
                        widths_64.push(scalar_width_64);
                        shifts_64.push(0);
                        twin_after.push(false);
                    }
                }
                continue;
            }
            // Emit one composer line per usable segment. The first
            // segment owns the actual baseline; the rest are twin
            // partners that the post-layout pass collapses onto the
            // first's row. Sort by x so the leftmost segment comes
            // first — keeps reading order intact.
            let mut usable_sorted = usable;
            usable_sorted
                .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (idx, (a, b)) in usable_sorted.iter().enumerate() {
                let w64 = ((b - a) * paged_text::shape::ADVANCE_PRECISION).round() as i32;
                let shift_pt = (a - frame_inner_left).max(0.0);
                widths_64.push(w64);
                shifts_64.push((shift_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32);
                // Mark every segment after the first as a twin so
                // the emit pass collapses it onto the first
                // segment's row at the same baseline.
                twin_after.push(idx > 0);
            }
        }
    }
    if widths_64.is_empty() {
        return WrapPlan {
            line_x_shifts_64: Vec::new(),
            twin_after: Vec::new(),
        };
    }
    lopts.compose.column_widths = Some(widths_64);
    WrapPlan {
        line_x_shifts_64: shifts_64,
        twin_after,
    }
}

/// Map an inner-coord top-left corner through ItemTransform to its
/// spread-coord position. Identity (`None`) is the no-op. Used by
/// the text-emission path so glyphs land where the frame actually
/// sits in spread coords rather than at its inner-coord origin.
fn frame_spread_top_left(b: paged_parse::Bounds, m: Option<[f32; 6]>) -> (f32, f32) {
    match m {
        Some(m) => apply_matrix(&m, b.left, b.top),
        None => (b.left, b.top),
    }
}

/// Whether items on `layer_ref` should render. Matches the
/// `layer_visible` closure in `build_document`: missing layer (or
/// unknown id) defaults to visible so single-layer IDMLs that omit
/// ItemLayer still emit.
fn is_layer_visible(document: &Document, layer_ref: Option<&str>) -> bool {
    // Route through the scene helper so the renderer and the canvas
    // hit-tester agree, including the layer-group ancestor walk (a
    // visible child inside a hidden group resolves hidden).
    paged_scene::layer_render_visible(&document.container.designmap, layer_ref)
}

fn page_for_frame(frame: &paged_parse::Bounds, pages: &[PageGeom]) -> Option<usize> {
    let cx = (frame.left + frame.right) * 0.5;
    let cy = (frame.top + frame.bottom) * 0.5;
    pages.iter().position(|p| {
        let b = p.bounds_in_spread;
        cx >= b.left && cx <= b.right && cy >= b.top && cy <= b.bottom
    })
}

/// Local page indices whose `bounds_in_spread` overlap `frame`.
/// Used by the non-text shape emit loops (Rectangle / Oval /
/// GraphicLine / Polygon) so spread-spanning page backgrounds, hero
/// bands, and bleed-the-gutter decoratives render on every page they
/// cover instead of only on the page that wins the AABB-centroid
/// test. Pages clip raster output to their own dimensions, so emitting
/// the same geometry twice is safe — the page-local rasterizer drops
/// off-page commands.
///
/// `None` is treated as "no overlap"; callers fall back to the legacy
/// `page_for_frame` centroid (or `0`) for backwards compatibility.
/// Q-02: rough longest-line width estimator for AutoSizing-width
/// frames. Walks the story's runs, scores each by (char_count ×
/// point_size × 0.62), and returns the max. Includes a 1.1 slack
/// factor so the estimator runs a touch wider than the rendered line
/// to make sure Knuth-Plass doesn't trim a trailing space. Returns 0
/// when no story / no runs / story is empty so the caller falls back
/// to the authored width.
///
/// The estimator is intentionally cheap (no shape calls) — display
/// headlines render at one of a handful of weights and the 0.62
/// advance ratio holds across them within ~10%. The cost of a few %
/// over-estimate is the breaker has a touch more stretch budget than
/// it needs; the alternative (under-estimating) collapses wrap back
/// to "MAG" / "BUSI" because the budget is too tight.
fn q02_estimate_auto_sizing_width(document: &Document, frame: &TextFrame) -> f32 {
    let Some(story_id) = frame.parent_story.as_deref() else {
        return 0.0;
    };
    let Some(story) = document.stories.iter().find(|s| s.self_id == story_id) else {
        return 0.0;
    };
    let mut max_line: f32 = 0.0;
    // Estimate per-paragraph: the longest WORD plus a margin so a
    // word boundary doesn't force a mid-word break. Iterate runs in
    // each paragraph, accumulate per-line width as text + spaces, and
    // reset on hard line breaks (`\n`) — the authoring app wraps each
    // paragraph independently.
    for paragraph in &story.story.paragraphs {
        for run in &paragraph.runs {
            let point_size = run.point_size.unwrap_or(12.0);
            // Walk by line so a multi-line run (paragraph break in
            // the middle) doesn't conflate two lines into one.
            for line in run.text.split('\n') {
                let chars = line.chars().count() as f32;
                let est = chars * point_size * 0.62 * 1.1;
                if est > max_line {
                    max_line = est;
                }
            }
        }
    }
    max_line
}

/// W1.7 Phase B — rough wrapped-line-count estimator for an
/// AutoSizing-height frame. Walks the story's runs, wraps each
/// paragraph at `inner_width_pt` using the same cheap 0.62 advance
/// ratio as [`q02_estimate_auto_sizing_width`] (no shape calls), and
/// returns the total line count. Hard breaks (`\n`) start a new line;
/// an empty trailing segment still contributes one line (an empty
/// paragraph occupies a line). Returns at least 1 so a non-empty frame
/// never grows to zero height.
///
/// Deliberately mirrors the width estimator's cheapness: the count is
/// the *grown box* budget, not the rendered line positions (those come
/// from the real composer in the story pass). A few % over/under-count
/// shifts the painted box by a fraction of a line — far better than
/// leaving the box at the authored undersized height while the text
/// (placed by Phase A) spills past it.
fn estimate_auto_sizing_line_count(
    document: &Document,
    frame: &TextFrame,
    inner_width_pt: f32,
) -> u32 {
    let Some(story_id) = frame.parent_story.as_deref() else {
        return 1;
    };
    let Some(story) = document.stories.iter().find(|s| s.self_id == story_id) else {
        return 1;
    };
    let width = inner_width_pt.max(1.0);
    let mut total_lines: u32 = 0;
    for paragraph in &story.story.paragraphs {
        // Accumulate the paragraph's natural width across its runs,
        // resetting on every hard line break. `\n`-delimited segments
        // wrap independently.
        let mut seg_natural: f32 = 0.0;
        let mut seg_has_content = false;
        let flush = |seg_natural: f32, has_content: bool, total: &mut u32| {
            // ceil(natural / width) lines, min 1 — an empty segment is
            // still one (blank) line.
            let lines = if has_content && seg_natural > 0.0 {
                (seg_natural / width).ceil().max(1.0) as u32
            } else {
                1
            };
            *total += lines;
        };
        for run in &paragraph.runs {
            let point_size = run.point_size.unwrap_or(12.0);
            let mut first_seg = true;
            for line in run.text.split('\n') {
                if !first_seg {
                    // A hard break closed the previous segment.
                    flush(seg_natural, seg_has_content, &mut total_lines);
                    seg_natural = 0.0;
                    seg_has_content = false;
                }
                first_seg = false;
                let chars = line.chars().count() as f32;
                if chars > 0.0 {
                    seg_natural += chars * point_size * 0.62;
                    seg_has_content = true;
                }
            }
        }
        // Close the paragraph's final (or only) segment.
        flush(seg_natural, seg_has_content, &mut total_lines);
    }
    total_lines.max(1)
}

/// W1.7 Phase B — compute an AutoSizing frame's GROWN inner-coord
/// bounds. Phase A grew the text *placement* downward (lines past the
/// authored bottom are kept rather than dropped); Phase B makes the
/// frame's visible extent — its painted fill/stroke box and the
/// text-wrap exclusion neighbouring frames see — match that growth.
///
/// The grown box honours the `AutoSizingType` (which axes may grow) and
/// the `AutoSizingReferencePoint` (which corner/edge stays pinned while
/// the box grows). Width growth reuses [`q02_estimate_auto_sizing_width`];
/// height growth uses the wrapped line count × auto-leading. Floors
/// from `MinimumWidthForAutoSizing` / `MinimumHeightForAutoSizing`
/// (the latter only when `UseMinimumHeightForAutoSizing`) apply. A box
/// never shrinks below its authored bounds — AutoSizing only grows.
///
/// Returns `None` when the frame doesn't auto-size (or only grows in a
/// way that doesn't change the authored bounds), so callers can keep
/// the cheap authored-bounds path.
fn compute_auto_sized_bounds(
    document: &Document,
    frame: &TextFrame,
) -> Option<paged_parse::Bounds> {
    let at = frame.auto_sizing?;
    if matches!(at, paged_parse::AutoSizingType::Off) {
        return None;
    }
    let authored = frame.bounds;
    let insets = frame.inset_spacing.unwrap_or([0.0; 4]); // top,left,bottom,right
    let authored_w = authored.width().max(0.0);
    let authored_h = authored.height().max(0.0);

    // --- Width axis ---
    let mut grown_w = authored_w;
    if at.grows_width() {
        let est = q02_estimate_auto_sizing_width(document, frame); // inner text width
        let floor = frame.minimum_width_for_auto_sizing.unwrap_or(0.0);
        // The estimate + floor are inner (text) widths; the box adds the
        // L/R insets back to compare against the authored *outer* width.
        let needed_outer = est.max(floor) + insets[1] + insets[3];
        grown_w = needed_outer.max(authored_w);
    }

    // --- Height axis ---
    let mut grown_h = authored_h;
    if at.grows_height() {
        // Wrap at the (possibly grown) inner width so a width-grown box
        // needs fewer lines.
        let inner_w = (grown_w - insets[1] - insets[3]).max(1.0);
        let lines = estimate_auto_sizing_line_count(document, frame, inner_w);
        // Auto-leading is 1.2 × point size (LayoutOptions::new); use the
        // story's leading run size as the representative line height.
        let line_height_pt = auto_sizing_line_height_pt(document, frame);
        let needed_inner_h = lines as f32 * line_height_pt;
        let mut needed_outer_h = needed_inner_h + insets[0] + insets[2];
        if frame.use_minimum_height_for_auto_sizing == Some(true) {
            if let Some(min_h) = frame.minimum_height_for_auto_sizing {
                needed_outer_h = needed_outer_h.max(min_h);
            }
        }
        grown_h = needed_outer_h.max(authored_h);
    }

    // HeightAndWidthProportionally: keep the authored aspect ratio while
    // growing. Take the larger growth factor on either axis and apply it
    // to both so the box scales uniformly (InDesign's "Proportionally").
    if matches!(
        at,
        paged_parse::AutoSizingType::HeightAndWidthProportionally
    ) && authored_w > 0.0
        && authored_h > 0.0
    {
        let fx = grown_w / authored_w;
        let fy = grown_h / authored_h;
        let f = fx.max(fy).max(1.0);
        grown_w = authored_w * f;
        grown_h = authored_h * f;
    }

    // No change ⇒ let the caller use the authored bounds.
    if grown_w <= authored_w + 0.01 && grown_h <= authored_h + 0.01 {
        return None;
    }

    // --- Reference-point anchoring ---
    // The reference point is the corner/edge that stays fixed as the box
    // grows. Distribute the width delta to left/right and the height
    // delta to top/bottom per the pinned point. Default TopLeftPoint:
    // grow right + down (top-left pinned), matching Phase A's downward
    // line placement.
    use paged_parse::AutoSizingReferencePoint as RP;
    let rp = frame
        .auto_sizing_reference_point
        .unwrap_or(RP::TopLeftPoint);
    let dw = grown_w - authored_w;
    let dh = grown_h - authored_h;
    // Horizontal split: fraction of dw added to the LEFT (box extends
    // leftward by `left_frac * dw`, rightward by the remainder).
    let left_frac = match rp {
        RP::TopLeftPoint | RP::CenterLeftPoint | RP::BottomLeftPoint => 0.0,
        RP::TopCenterPoint | RP::CenterPoint | RP::BottomCenterPoint => 0.5,
        RP::TopRightPoint | RP::CenterRightPoint | RP::BottomRightPoint => 1.0,
    };
    // Vertical split: fraction of dh added to the TOP.
    let top_frac = match rp {
        RP::TopLeftPoint | RP::TopCenterPoint | RP::TopRightPoint => 0.0,
        RP::CenterLeftPoint | RP::CenterPoint | RP::CenterRightPoint => 0.5,
        RP::BottomLeftPoint | RP::BottomCenterPoint | RP::BottomRightPoint => 1.0,
    };
    Some(paged_parse::Bounds {
        left: authored.left - dw * left_frac,
        right: authored.right + dw * (1.0 - left_frac),
        top: authored.top - dh * top_frac,
        bottom: authored.bottom + dh * (1.0 - top_frac),
    })
}

/// Representative auto-leading line height (pt) for an AutoSizing
/// frame's story: the leading run's point size × 1.2 (the auto-leading
/// factor `LayoutOptions::new` uses), or an explicit `Leading` when the
/// leading run carries one. Falls back to 12pt × 1.2.
fn auto_sizing_line_height_pt(document: &Document, frame: &TextFrame) -> f32 {
    let lh = frame
        .parent_story
        .as_deref()
        .and_then(|sid| document.stories.iter().find(|s| s.self_id == sid))
        .and_then(|story| story.story.paragraphs.iter().flat_map(|p| &p.runs).next())
        .map(|run| {
            run.leading
                .filter(|l| *l > 0.0)
                .unwrap_or_else(|| run.point_size.unwrap_or(12.0) * 1.2)
        })
        .unwrap_or(12.0 * 1.2);
    lh.max(1.0)
}

fn pages_overlapping_frame(frame: &paged_parse::Bounds, pages: &[PageGeom]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        let b = p.bounds_in_spread;
        if frame.right > b.left
            && frame.left < b.right
            && frame.bottom > b.top
            && frame.top < b.bottom
        {
            out.push(i);
        }
    }
    out
}

fn emit_text_frame_into(
    page: &mut BuiltPage,
    frame: &TextFrame,
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&paged_color::IccTransform>,
    drop_shadow: Option<DropShadow>,
    auto_sized_bounds: Option<paged_parse::Bounds>,
) {
    let mut resolved = ResolvedFrame::from_text_frame(frame);
    // W1.7 Phase B: an AutoSizing frame paints its fill / stroke to the
    // GROWN extent, not the authored undersized box. Substitute the
    // grown rect into the resolved geometry so the box, its effects, and
    // its drop shadow all stretch to where the auto-sized text actually
    // reaches. Only the rectangular text-panel case is grown — a
    // non-rectangular (`Polygon`) text frame keeps its authored outline.
    if let Some(grown) = auto_sized_bounds {
        if let Geometry::TextFrameRect { rect } = &mut resolved.geometry {
            *rect = Rect {
                x: grown.left,
                y: grown.top,
                w: grown.width(),
                h: grown.height(),
            };
        }
    }
    let style = crate::module::resolve_applied_style(&resolved, document);
    if let Some(s) = &style {
        crate::module::object_style_cascade(&mut resolved, s);
    }
    page.stats.frames += 1;
    let outer = frame_outer_transform(page, resolved.item_transform);
    // Bracket fill / stroke / drop-shadow into a transparency group
    // whenever the frame's blend mode is non-Normal or opacity < 100%.
    // The group composite at EndBlendGroup applies the blend mode
    // against the page underneath, which is the structurally correct
    // PDF transparency-group semantic (replaces the per-glyph /
    // per-shape FillPathBlend approximation).
    //
    // Text glyphs land in this same page list during the story pass —
    // they're bracketed separately post-pass via
    // `bracket_text_frame_glyph_ranges` so each text frame's glyphs
    // composite with the same blend mode against the page below.
    let needs_group = frame_needs_blend_group(&resolved);
    let group_bounds = if needs_group {
        let geom_bounds = match &resolved.geometry {
            Geometry::TextFrameRect { rect } | Geometry::Rect { rect } => *rect,
            Geometry::Oval { rect } => *rect,
            Geometry::Polygon { bbox, .. } => *bbox,
            Geometry::Line { p0, p1 } => paged_compose::Rect {
                x: p0.0.min(p1.0),
                y: p0.1.min(p1.1),
                w: (p0.0 - p1.0).abs(),
                h: (p0.1 - p1.1).abs(),
            },
        };
        Some(push_blend_group(
            page,
            geom_bounds,
            outer,
            resolved.blend_mode,
            frame_group_opacity(&resolved),
        ))
    } else {
        None
    };
    crate::module::drop_shadow_module(
        &resolved,
        page,
        palette,
        cmyk_xform,
        drop_shadow,
        outer,
        frame.stroke_drop_shadow.as_ref(),
    );
    // W1.1: a TextFrame carrying a genuinely non-rectangular path
    // (triangle / pentagon / Bezier / compound outline) had its
    // geometry lifted to `Geometry::Polygon` by `from_text_frame`.
    // Intern that path up-front so the frame's own fill / stroke /
    // effects paint the real outline rather than the AABB — mirroring
    // `emit_polygon_into`. Plain rectangular text panels keep the
    // unit-rect path (`fill_path = None`) and the rect emitter. Text
    // *layout* clipping is handled separately off `frame.anchors`.
    let fill_path = if let Geometry::Polygon {
        anchors,
        subpath_starts,
        subpath_open,
        ..
    } = &resolved.geometry
    {
        let path = polygon_path_from_anchors_with_open(anchors, subpath_starts, subpath_open);
        let cache_key = match resolved.self_id {
            Some(id) => fnv_1a_u64(id.as_bytes()),
            None => path_signature(anchors),
        };
        let (id, _) = page.list.paths.intern(cache_key, path);
        Some(id)
    } else {
        None
    };
    // Q-04: extended GradientFeather (and the rest of FrameEffects) to
    // TextFrame. For the rectangular panel we route through the unit-
    // rect path the same way `emit_rectangle_into` does (intern the
    // unit rect, scale via `Transform::for_rect_in`, flag
    // `effects_unit_normalize` so the effects module converts path-
    // local coords from unit space). For a pathed text frame the
    // interned polygon path is already in inner-anchor coords under
    // `outer`, so effects ride it directly with no unit normalisation
    // (mirrors `emit_polygon_into`).
    let (effects_path, effects_xform, effects_unit_normalize) = if frame.effects.is_some() {
        match (&resolved.geometry, fill_path) {
            (Geometry::TextFrameRect { rect: r }, _) => {
                let (id, _) = page.list.paths.intern(
                    paged_compose::UNIT_RECT_KEY,
                    paged_compose::PathData {
                        segments: vec![
                            paged_compose::PathSegment::MoveTo { x: 0.0, y: 0.0 },
                            paged_compose::PathSegment::LineTo { x: 1.0, y: 0.0 },
                            paged_compose::PathSegment::LineTo { x: 1.0, y: 1.0 },
                            paged_compose::PathSegment::LineTo { x: 0.0, y: 1.0 },
                            paged_compose::PathSegment::Close,
                        ],
                    },
                );
                (Some(id), Transform::for_rect_in(*r, outer), Some(*r))
            }
            (Geometry::Polygon { .. }, Some(pid)) => (Some(pid), outer, None),
            _ => (None, outer, None),
        }
    } else {
        (None, outer, None)
    };
    if let (Some(path_id), Some(effects)) = (effects_path, frame.effects.as_ref()) {
        crate::module::emit_effects_pre_fill(
            page,
            effects,
            path_id,
            effects_xform,
            palette,
            cmyk_xform,
        );
    }
    crate::module::fill_paint_module(
        &resolved, page, palette, cmyk_xform, fallback, outer, fill_path,
    );
    if let (Some(path_id), Some(effects)) = (effects_path, frame.effects.as_ref()) {
        crate::module::emit_effects_post_fill(
            page,
            effects,
            path_id,
            effects_xform,
            palette,
            cmyk_xform,
            effects_unit_normalize,
        );
    }
    crate::module::stroke_paint_module(
        &resolved,
        page,
        palette,
        cmyk_xform,
        outer,
        fill_path,
        stroke_for(
            resolved.stroke_type,
            resolved.effective_stroke_weight(),
            resolved.end_cap,
            resolved.end_join,
            resolved.miter_limit,
            Some(&document.styles.stroke_styles),
            resolved.stroke_dash,
        ),
    );
    if needs_group {
        pop_blend_group(page);
    }
    let _ = group_bounds;
}

/// First-baseline y (1/64 pt) for the head frame of a story,
/// honouring `<TextFramePreference FirstBaselineOffset>` and the
/// top inset. `default_64` is the renderer's heuristic baseline
/// (LayoutOptions::new gives `point_size * 0.8 * 64`) used for
/// `AscentOffset` (the IDML default) and any unrecognised value.
/// `metrics` carries the head font's OS/2 / hhea metrics; when
/// present, `CapHeight` and `XHeight` policies use the font's
/// real values instead of a 70% / 50% heuristic.
fn first_baseline_for_frame(
    frame: &TextFrame,
    point_size: f32,
    default_64: i32,
    metrics: Option<&FontMetrics>,
) -> i32 {
    const CAP_HEIGHT_FALLBACK: f32 = 0.70;
    const X_HEIGHT_FALLBACK: f32 = 0.50;
    let top_inset_64 = frame
        .inset_spacing
        .map(|i| (i[0] * paged_text::shape::ADVANCE_PRECISION).round() as i32)
        .unwrap_or(0);
    let pt_to_64 = |pt: f32| (pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;
    let em_fraction_to_64 = |frac: f32| pt_to_64(point_size * frac);
    use paged_parse::FirstBaselineOffset as F;
    let policy_offset_64 = match frame.first_baseline_offset {
        Some(F::CapHeight) => em_fraction_to_64(
            metrics
                .and_then(|m| m.cap_height)
                .unwrap_or(CAP_HEIGHT_FALLBACK),
        ),
        Some(F::XHeight) => em_fraction_to_64(
            metrics
                .and_then(|m| m.x_height)
                .unwrap_or(X_HEIGHT_FALLBACK),
        ),
        Some(F::EmBoxHeight) => pt_to_64(point_size),
        // FixedHeight / LeadingOffset use MinimumFirstBaselineOffset
        // verbatim. Falls back to default when missing.
        Some(F::FixedHeight) | Some(F::LeadingOffset) => frame
            .minimum_first_baseline_offset
            .map(pt_to_64)
            .unwrap_or(default_64),
        // AscentOffset (IDML default) and `None` (unrecognised /
        // absent attribute): use the font's ascender if available;
        // otherwise fall through to the LayoutOptions heuristic.
        Some(F::AscentOffset) | None => metrics
            .map(|m| em_fraction_to_64(m.ascender))
            .unwrap_or(default_64),
    };
    // Display-headline clamp: when the frame is sized to the visual
    // letterform (cap height) rather than the typo ascent — common
    // on Envato cover-style templates where designers tight-fit
    // 60-100pt headlines into ~half-em-tall boxes — `AscentOffset`'s
    // baseline lands past the frame bottom and the renderer drops
    // the line. InDesign keeps the text by treating the baseline as
    // if cap-height were the ascent. Mirror that: if the resolved
    // offset would exceed the frame's inner height, fall back to a
    // cap-height-based offset (which fits inside any box at least
    // ~0.7×pt tall, matching real-world headline frame sizing).
    let bottom_inset_64 = frame
        .inset_spacing
        .map(|i| (i[2] * paged_text::shape::ADVANCE_PRECISION).round() as i32)
        .unwrap_or(0);
    let inner_height_64 = pt_to_64(frame.bounds.height()) - top_inset_64 - bottom_inset_64;
    let baseline_offset_in_frame = top_inset_64 + policy_offset_64;
    if inner_height_64 > 0 && baseline_offset_in_frame > top_inset_64 + inner_height_64 {
        let cap_height = metrics
            .and_then(|m| m.cap_height)
            .unwrap_or(CAP_HEIGHT_FALLBACK);
        let clamped = em_fraction_to_64(cap_height);
        return top_inset_64 + clamped.min(inner_height_64);
    }
    baseline_offset_in_frame
}

/// Build the outer affine that maps a frame's local-space rect into
/// page-space pixels: page-origin offset composed with the frame's
/// `ItemTransform` (identity when absent). Identity ItemTransform is
/// the common case — the result collapses to a pure translation, so
/// the rasterizer's axis-aligned fast paths still apply.
/// Post-multiply `xf` by a rotation/scale `linear` (2×2: a b c d in
/// row-major IDML convention) pivoted around the page-space point
/// `(pivot_x, pivot_y)`. Mathematically:
///   xf := T(pivot) · L · T(-pivot) · xf
/// Used by the text-emission path so glyph commands inside a
/// rotated/sheared TextFrame inherit the frame's ItemTransform
/// rotation around the frame's top-left.
/// Phase 7 — vertical writing post-rotation. Walks the per-page
/// command ranges this story emitted and rotates each command 90°
/// clockwise around its host frame's top-left, then translates +x
/// by the frame's width. The result: horizontal content layouts
/// flip into CJK vertical convention — columns advance right-to-
/// left and characters within a column read top-to-bottom.
///
/// `pre_counts[i]` is the number of commands on `pages[i]` before
/// this story's emit; commands at index ≥ pre_counts[i] are this
/// story's contributions and get rotated. `chain` + `chain_pages`
/// are parallel slices — `chain[i]` is the host frame whose page
/// is `chain_pages[i]`. For each page that hosted at least one
/// chain frame, the FIRST matching chain frame's geometry is used
/// as the rotation pivot (typical CJK doesn't thread vertical
/// stories across pages anyway).
///
/// Limitations:
/// - Latin glyphs end up sideways. Upright Latin in CJK vertical
///   would require per-glyph counter-rotation around each glyph's
///   centre (`<RotateSingleByteCharacters>` IDML attribute).
/// - Rotated content overflows the frame's geometric bounds when
///   the original layout was wider than the frame is tall (the
///   common case for tall frames flipped from wide layouts).
/// - Frame-inset axes don't swap (a 12pt TextTopInset stays in y,
///   not x). The right fix moves to a layout-time axis swap.
fn apply_vertical_writing_rotation(
    pages: &mut [BuiltPage],
    pre_counts: &[usize],
    chain: &[&paged_parse::TextFrame],
    chain_pages: &[usize],
) {
    use std::collections::BTreeMap;
    // For each page that hosted this story, look up the first
    // chain frame on that page. We pivot around that frame's
    // top-left and translate by the frame's width.
    let mut frame_for_page: BTreeMap<usize, &paged_parse::TextFrame> = BTreeMap::new();
    for (i, &page_idx) in chain_pages.iter().enumerate() {
        frame_for_page.entry(page_idx).or_insert(chain[i]);
    }
    // 90° CW rotation in screen coords (Y down): cos=0, sin=1.
    // Matrix linear part [a, b, c, d] = [0, 1, -1, 0].
    let linear = [0.0_f32, 1.0, -1.0, 0.0];
    for (page_idx, frame) in frame_for_page {
        if page_idx >= pages.len() {
            continue;
        }
        let pre = pre_counts.get(page_idx).copied().unwrap_or(0);
        let total = pages[page_idx].list.commands.len();
        if pre >= total {
            continue;
        }
        let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
        let (ox, oy) = pages[page_idx].spread_origin;
        let pivot_x = sx - ox;
        let pivot_y = sy - oy;
        let frame_w = frame.bounds.width();
        for cmd in &mut pages[page_idx].list.commands[pre..total] {
            let xf = cmd.transform_mut();
            rotate_transform_around(xf, linear, pivot_x, pivot_y);
            // After rotation around the frame's top-left, rotated
            // content lives in x ∈ [pivot_x - h, pivot_x], y ∈
            // [pivot_y, pivot_y + w]. Shift +frame_w on x to bring
            // it into the right half of the frame.
            xf.0[4] += frame_w;
        }
    }
}

fn rotate_transform_around(xf: &mut Transform, linear: [f32; 4], pivot_x: f32, pivot_y: f32) {
    let [a, b, c, d] = linear;
    // The pivoted rotation is:
    //   M = [a c (pivot_x - a*pivot_x - c*pivot_y);
    //        b d (pivot_y - b*pivot_x - d*pivot_y);
    //        0 0 1]
    // Compose as M · xf.
    let [xa, xb, xc, xd, xtx, xty] = xf.0;
    let m_tx = pivot_x - a * pivot_x - c * pivot_y;
    let m_ty = pivot_y - b * pivot_x - d * pivot_y;
    let new_a = a * xa + c * xb;
    let new_b = b * xa + d * xb;
    let new_c = a * xc + c * xd;
    let new_d = b * xc + d * xd;
    let new_tx = a * xtx + c * xty + m_tx;
    let new_ty = b * xtx + d * xty + m_ty;
    xf.0 = [new_a, new_b, new_c, new_d, new_tx, new_ty];
}

fn frame_outer_transform(page: &BuiltPage, item_transform: Option<[f32; 6]>) -> Transform {
    let (ox, oy) = page.spread_origin;
    let page_origin = Transform::translate(-ox, -oy);
    // W1.9 — the spread-level `<Spread ItemTransform>` rotation/scale
    // (`spread_transform`, linear part only; translation already cancels
    // against `spread_origin`) is applied ABOUT the page origin: first
    // re-origin the frame into page-local space, then rotate/scale the
    // whole page in place. When the spread carries no rotation/scale
    // `spread_transform` is `IDENTITY` and this is exactly the historical
    // `translate(-spread_origin) ∘ item_transform`. The canvas hit-tester
    // inverts the same `spread_transform`, so selection can't disagree
    // with paint.
    let local = match item_transform {
        Some(m) => page_origin.compose(&Transform(m)),
        None => page_origin,
    };
    if page.spread_transform == Transform::IDENTITY {
        local
    } else {
        page.spread_transform.compose(&local)
    }
}

/// Axis-aligned bounding box of `rect` after `outer` is applied to its
/// four corners. The corners may rotate / shear under non-uniform
/// transforms, so we union all four projections rather than just the
/// top-left + bottom-right.
fn rect_bounds_in_page(rect: paged_compose::Rect, outer: Transform) -> paged_compose::Rect {
    let pts = [
        outer.apply(rect.x, rect.y),
        outer.apply(rect.x + rect.w, rect.y),
        outer.apply(rect.x + rect.w, rect.y + rect.h),
        outer.apply(rect.x, rect.y + rect.h),
    ];
    let mut minx = pts[0].0;
    let mut miny = pts[0].1;
    let mut maxx = minx;
    let mut maxy = miny;
    for &(x, y) in &pts[1..] {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    paged_compose::Rect {
        x: minx,
        y: miny,
        w: (maxx - minx).max(0.0),
        h: (maxy - miny).max(0.0),
    }
}

/// Decide whether `frame` needs a transparency-group bracket: any
/// non-Normal blend mode, or any opacity strictly less than 100%.
/// Normal + 100% opacity is the fast path that draws straight onto
/// the page.
pub(crate) fn frame_needs_blend_group(frame: &ResolvedFrame<'_>) -> bool {
    if !matches!(frame.blend_mode, paged_compose::BlendMode::Normal) {
        return true;
    }
    matches!(frame.opacity, Some(o) if o < 100.0 - f32::EPSILON)
}

/// Group opacity normalised to 0..=1. Defaults to 1.0 when no opacity
/// override is present on the frame.
fn frame_group_opacity(frame: &ResolvedFrame<'_>) -> f32 {
    frame
        .opacity
        .map(|p| (p / 100.0).clamp(0.0, 1.0))
        .unwrap_or(1.0)
}

/// Push a `BeginBlendGroup` covering `geometry_bounds × outer` (axis-
/// aligned in page coords, padded slightly so AA edges stay inside the
/// buffer). Returns the bounds the matching `EndBlendGroup` will use,
/// for callers that want to bracket multiple ranges of commands with
/// the same group buffer.
pub(crate) fn push_blend_group(
    page: &mut BuiltPage,
    bounds_in_inner: paged_compose::Rect,
    outer: Transform,
    blend_mode: paged_compose::BlendMode,
    opacity: f32,
) -> paged_compose::Rect {
    let bounds = rect_bounds_in_page(bounds_in_inner, outer);
    // Pad by 0.5pt so glyph anti-aliasing at the edges of the
    // text-frame bbox still falls inside the buffer.
    let padded = paged_compose::Rect {
        x: bounds.x - 0.5,
        y: bounds.y - 0.5,
        w: bounds.w + 1.0,
        h: bounds.h + 1.0,
    };
    page.list
        .commands
        .push(paged_compose::DisplayCommand::BeginBlendGroup {
            bounds: padded,
            blend_mode,
            opacity,
            transform: Transform::IDENTITY,
        });
    padded
}

/// Push the matching `EndBlendGroup` for [`push_blend_group`].
pub(crate) fn pop_blend_group(page: &mut BuiltPage) {
    page.list
        .commands
        .push(paged_compose::DisplayCommand::EndBlendGroup(
            Transform::IDENTITY,
        ));
}

/// Resolve the effective shadow for a frame. Per-frame IDML shadow
/// wins; the synthetic `fallback` (from `PipelineOptions`) is used
/// when the frame carries none. Returns `None` for fully-transparent
/// shadows so callers don't emit a no-op.
pub(crate) fn resolve_frame_shadow(
    frame_shadow: Option<&paged_parse::DropShadowSetting>,
    fallback: Option<DropShadow>,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
) -> Option<DropShadow> {
    frame_shadow
        .and_then(|s| convert_setting_to_shadow(s, palette, cmyk_xform))
        .or(fallback)
}

/// Convert an IDML `<DropShadowSetting>` to a compose-layer `DropShadow`.
/// The parser already drops `Mode="None"` settings, so we only have
/// to filter out fully-transparent shadows here.
fn convert_setting_to_shadow(
    setting: &paged_parse::DropShadowSetting,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
) -> Option<DropShadow> {
    let opacity = (setting.opacity_pct / 100.0).clamp(0.0, 1.0);
    if opacity == 0.0 {
        return None;
    }
    let color = setting
        .effect_color
        .as_deref()
        .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
        .and_then(|p| paint_as_solid_with_icc(p, cmyk_xform))
        .unwrap_or(Color::BLACK);
    Some(DropShadow {
        offset_x: setting.x_offset,
        offset_y: setting.y_offset,
        blur_radius: setting.size,
        color,
        opacity,
    })
}

/// Pull the inner `Color` out of a solid (or CMYK) paint, returning
/// `None` for gradient paints. Used wherever a context can only
/// consume a flat colour (drop shadow, per-glyph paint).
///
/// `Paint::Cmyk` flattens through the supplied ICC transform (or via
/// the naive CMYK→RGB fallback when no transform is available), so
/// drop-shadow / gradient-stop / decoration paths that have only ever
/// understood RGB keep producing identical pixels to the pre-Stage A
/// world.
fn paint_as_solid_with_icc(
    p: Paint,
    cmyk_xform: Option<&paged_color::IccTransform>,
) -> Option<Color> {
    match p {
        Paint::Solid(c) => Some(c),
        // The CMYK paint carries the ICC-resolved display RGB cached
        // on it — drop-shadow / gradient-stop paths use that directly
        // so the colour matches what a direct `Paint::Solid` resolved
        // to before Stage A landed. `cmyk_xform` is unused here but
        // kept in the signature for callers that don't know if the
        // paint is a CMYK paint and want a stable API.
        Paint::Cmyk { rgb, .. } => {
            let _ = cmyk_xform;
            Some(rgb)
        }
        _ => None,
    }
}

/// Single-page convenience: union every page's bounds and emit all
/// frames in spread coordinates. Kept for back-compat and for hosts
/// that genuinely want one canvas — but multi-page callers should use
/// `build_document` instead.
pub fn build(document: &Document, options: &PipelineOptions) -> anyhow::Result<BuiltPage> {
    let palette = &document.palette;
    let mut stats = PipelineStats::default();
    let mut list = DisplayList::new();

    // Page bounding box — union across every page the document has.
    let mut page_w: f32 = 612.0;
    let mut page_h: f32 = 792.0;
    let mut saw_page = false;

    for parsed in &document.spreads {
        let spread = &parsed.spread;
        stats.spreads += 1;
        stats.pages += spread.pages.len();
        stats.frames += spread.text_frames.len() + spread.rectangles.len();

        for p in &spread.pages {
            if saw_page {
                page_w = page_w.max(p.bounds.width());
                page_h = page_h.max(p.bounds.height());
            } else {
                page_w = p.bounds.width();
                page_h = p.bounds.height();
                saw_page = true;
            }
        }

        for frame in &spread.text_frames {
            let rect = Rect {
                x: frame.bounds.left,
                y: frame.bounds.top,
                w: frame.bounds.width(),
                h: frame.bounds.height(),
            };
            let fill_paint = resolve_fill(frame, palette).unwrap_or(options.fallback_frame_fill);
            emit_rect(rect, fill_paint, &mut list);
            if let Some(stroke_paint) = resolve_stroke(frame, palette) {
                let width = frame.stroke_weight.unwrap_or(1.0);
                if width > 0.0 {
                    emit_stroke_rect(rect, Stroke::new(width), stroke_paint, &mut list);
                }
            }
        }

        for rect in &spread.rectangles {
            let r = Rect {
                x: rect.bounds.left,
                y: rect.bounds.top,
                w: rect.bounds.width(),
                h: rect.bounds.height(),
            };
            let fill = resolve_rect_fill(rect, palette).unwrap_or(options.fallback_frame_fill);
            emit_rect(r, fill, &mut list);
            if let Some(paint) = resolve_rect_stroke(rect, palette) {
                let width = rect.stroke_weight.unwrap_or(1.0);
                if width > 0.0 {
                    emit_stroke_rect(r, Stroke::new(width), paint, &mut list);
                }
            }
        }
    }

    let shaping_face = options
        .font
        .and_then(|bytes| rustybuzz::Face::from_slice(bytes, 0));
    let outline_face = options
        .font
        .and_then(|bytes| ttf_parser::Face::parse(bytes, 0).ok());
    let font_id = options.font.map(fnv_1a_u32).unwrap_or(0);

    for parsed in &document.stories {
        stats.stories += 1;
        let story = &parsed.story;
        let frame = document.frame_for(&parsed.self_id);
        let column_width_pt = options
            .fallback_column_width_pt
            .or_else(|| frame.map(|f| f.bounds.width()));

        for paragraph in &story.paragraphs {
            stats.paragraphs += 1;
            stats.runs += paragraph.runs.len();

            let paragraph_size = paragraph
                .runs
                .first()
                .and_then(|r| r.point_size)
                .unwrap_or(options.default_point_size);
            let paragraph_text: String = paragraph.runs.iter().map(|r| r.text.as_str()).collect();

            if let Some(face) = shaping_face.as_ref() {
                for run in &paragraph.runs {
                    let size = run.point_size.unwrap_or(options.default_point_size);
                    let shaped = paged_text::shape_run(face, &run.text, size);
                    stats.glyphs += shaped.glyphs.len();
                }
            }

            let (Some(face), Some(col_pt)) = (shaping_face.as_ref(), column_width_pt) else {
                continue;
            };
            let measurer = paged_text::RustybuzzMeasurer::new(face, paragraph_size);
            let mut lopts = paged_text::LayoutOptions::new(col_pt, paragraph_size);
            lopts.alignment = map_justification(paragraph.justification);
            let laid_out = paged_text::layout_paragraph(&paragraph_text, &measurer, &lopts);
            stats.lines += laid_out.lines.len();

            let (Some(outline), Some(frame)) = (outline_face.as_ref(), frame) else {
                continue;
            };
            let outliner = TtfOutliner::new(outline);
            let picker = build_run_paint_picker(paragraph, palette, options.fallback_text_paint);
            let origin = frame_spread_top_left(frame.bounds, frame.item_transform);
            emit_paragraph(
                &laid_out,
                font_id,
                paragraph_size,
                |cluster| picker.pick(cluster),
                origin,
                &outliner,
                &mut list,
            );
        }
    }

    Ok(BuiltPage {
        id: PageId::synthetic(0, 0),
        width_pt: page_w,
        height_pt: page_h,
        spread_origin: (0.0, 0.0),
        spread_transform: Transform::IDENTITY,
        list,
        layout_generation: 0,
        numbering_generation: 0,
        stats,
        story_layout: Vec::new(),
        footnotes: Vec::new(),
        diagnostics: Vec::new(),
        cell_rects: Vec::new(),
    })
}

/// Rasterise a single already-built page at a target DPI. Shared by
/// `render_document` and the canvas LOD cache, which wants to
/// produce low-resolution snapshots (page navigator / minimap) and
/// per-page bitmaps at viewport zoom without re-running
/// `build_document`. `background` is composited under transparent
/// regions of the display list.
#[cfg(feature = "cpu")]
pub fn render_built_page(page: &BuiltPage, dpi: f32, background: Color) -> image::RgbaImage {
    let mut raster_opts = paged_gpu::RasterOptions::new(page.width_pt, page.height_pt);
    raster_opts.dpi = dpi;
    raster_opts.background = background;
    paged_gpu::rasterize(&page.list, &raster_opts)
}

/// Build + rasterise every page. Returns one `RgbaImage` per page in
/// document order. `dpi` and `background` apply uniformly.
#[cfg(feature = "cpu")]
pub fn render_document(
    document: &Document,
    options: &PipelineOptions,
    dpi: f32,
    background: Color,
) -> anyhow::Result<(BuiltDocument, Vec<image::RgbaImage>)> {
    let built = build_document(document, options)?;
    let mut images = Vec::with_capacity(built.pages.len());
    for page in &built.pages {
        images.push(render_built_page(page, dpi, background));
    }
    Ok((built, images))
}

/// Build + rasterise in one call. `dpi` and `background` control the
/// raster pass; everything else comes from `options`.
#[cfg(feature = "cpu")]
pub fn render(
    document: &Document,
    options: &PipelineOptions,
    dpi: f32,
    background: Color,
) -> anyhow::Result<(BuiltPage, image::RgbaImage)> {
    let built = build(document, options)?;
    let mut raster_opts = paged_gpu::RasterOptions::new(built.width_pt, built.height_pt);
    raster_opts.dpi = dpi;
    raster_opts.background = background;
    let image = paged_gpu::rasterize(&built.list, &raster_opts);
    Ok((built, image))
}

/// Apply IDML paragraph-style attributes that drive the line breaker
/// onto a fresh `LayoutOptions`. Hyphenation defaults to *on* (IDML's
/// own default) when the cascade leaves the field unset; explicit
/// `Hyphenation="false"` disables it. Word-spacing percentages convert
/// to the composer's stretch / shrink ratios.
fn apply_paragraph_compose_options<'a>(
    lopts: &mut paged_text::LayoutOptions<'a>,
    hyphenator: Option<&'a paged_text::Hyphenator>,
    resolved: &paged_scene::ResolvedParagraphAttrs,
) {
    // Hyphenation: IDML's default is true; only an explicit false
    // disables it. We treat None as "use the default" which lets
    // unstyled paragraphs hyphenate just like InDesign would.
    let hyphenate = resolved.hyphenation.unwrap_or(true);
    if hyphenate {
        lopts.compose.hyphenator = hyphenator;
    } else {
        lopts.compose.hyphenator = None;
    }
    // Hyphenation zone (pt → 1/64 pt). Only meaningful when a
    // hyphenator is wired; the composer ignores it otherwise. A word
    // that would start within `zone` of the right margin is kept whole
    // rather than broken (InDesign's "hyphenation zone"). `None`/0 ⇒
    // no zone restriction (hyphenate anywhere an opportunity exists).
    //
    // W1.17: the zone is a *ragged-edge* feature. Adobe: "The
    // Hyphenation Zone … applies only when you're using the Single-line
    // Composer with nonjustified text." (helpx.adobe.com/indesign/using/
    // text-composition.html — "Compose and hyphenate text".) The zone's
    // whole job is to bound how far the right edge may rag before a
    // hyphen is forced; a justified paragraph has no rag (every line is
    // flushed to the column), so the option has no meaning there and
    // InDesign ignores it. Mirror that exactly: zero the zone for
    // justified paragraphs so the composer's hyphenation penalties are
    // driven purely by geometric fit, as InDesign's justified composer
    // does. W1.3 landed the ragged-only zone gate; this closes the
    // justified case as a documented no-op rather than a behaviour.
    let zone_64 = resolved
        .hyphenation_zone
        .map(|z| (z.max(0.0) * paged_text::shape::ADVANCE_PRECISION).round() as i32)
        .unwrap_or(0);
    lopts.compose.hyphenation_zone = if lopts.alignment == paged_text::Alignment::Justify {
        0
    } else {
        zone_64
    };
    // Word spacing: IDML carries percentages on the [Min..=Desired..=Max]
    // axis relative to the natural space-glyph advance. The composer's
    // `desired_space_ratio` scales the glue's natural width;
    // `stretch_ratio` / `shrink_ratio` are still relative to the raw
    // glyph advance, so the breaker reads a Min..=Desired..=Max band
    // shifted by Desired (P-07).
    let desired = resolved.desired_word_spacing.unwrap_or(100.0).max(1.0);
    lopts.compose.desired_space_ratio = (desired / 100.0).max(0.0);
    if let Some(max) = resolved.maximum_word_spacing {
        lopts.compose.stretch_ratio = ((max - desired) / 100.0).max(0.0);
    }
    if let Some(min) = resolved.minimum_word_spacing {
        lopts.compose.shrink_ratio = ((desired - min) / 100.0).clamp(0.0, 1.0);
    }
    // Floor the stretch budget so the breaker can always find a feasible
    // line. IDML paragraphs like `MinimumWordSpacing=90 MaximumWordSpacing=100`
    // (Max == Desired) yield a zero-stretch budget which Knuth-Plass cannot
    // satisfy on wide columns, collapsing wrap to one word per line (Q-15).
    //
    // Cycle-6 Track 4 Round B: only floor the stretch if the IDML
    // didn't carry an explicit max — paragraphs that explicitly set
    // MaximumWordSpacing get exactly the budget they asked for. The
    // Q-15 fallback was protecting the case where IDML's Max == Min
    // == Desired which yields zero budget; that's still covered by
    // the unconditional floor below for paragraphs with no
    // MaximumWordSpacing attribute.
    if resolved.maximum_word_spacing.is_none() {
        lopts.compose.stretch_ratio = lopts.compose.stretch_ratio.max(0.1);
    }
    // Q-20: fold letter-spacing budget into the per-word stretch /
    // shrink budget so the breaker can lean on inter-glyph space when
    // word-space alone can't justify a line. IDML's
    // `Min/Desired/Max LetterSpacing` is in pt and applies *between
    // glyphs*; we approximate by adding `letter_delta_pt * avg_chars_per_word`
    // into the existing space stretch / shrink ratios. Default values
    // (0 pt) are a no-op. Real per-glyph distribution after the
    // breaker picks breaks is queued.
    let ls_min = resolved.minimum_letter_spacing.unwrap_or(0.0);
    let ls_desired = resolved.desired_letter_spacing.unwrap_or(0.0);
    let ls_max = resolved.maximum_letter_spacing.unwrap_or(0.0);
    if ls_min != 0.0 || ls_desired != 0.0 || ls_max != 0.0 {
        // Cycle-6 Track 3: bounded mapping from LS budget (pt) to
        // stretch_add / shrink_add. The cycle-5 formula
        // `(ls_max - ls_desired) * AVG_CHARS_PER_WORD / space_width`
        // saturated `.min(2.0)` on typical IDML LS values (e.g.
        // newspaper's body MaximumLetterSpacing=25 ⇒ stretch_add ≈ 78
        // clamped to 2.0), making any AVG_CHARS_PER_WORD-style tweak
        // invisible to the harness. The new mapping caps the
        // contribution at 0.5 / 0.25 (half of the legacy ceiling) so
        // the breaker has letter-spacing budget without overwhelming
        // word-spacing budget. `LS_BUDGET_PT_FOR_FULL_STRETCH = 24.0`
        // calibrates from the InDesign default 25pt-vs-0 spread
        // mapping to ~full contribution; smaller spreads fall below
        // proportionally and remain unsaturated.
        const LS_BUDGET_PT_FOR_FULL_STRETCH: f32 = 12.0;
        let stretch_budget = (ls_max - ls_desired).max(0.0);
        let shrink_budget = (ls_desired - ls_min).max(0.0);
        let stretch_add = (stretch_budget / LS_BUDGET_PT_FOR_FULL_STRETCH).clamp(0.0, 0.5);
        let shrink_add = (shrink_budget / LS_BUDGET_PT_FOR_FULL_STRETCH).clamp(0.0, 0.25);
        lopts.compose.stretch_ratio = (lopts.compose.stretch_ratio + stretch_add).min(2.0);
        lopts.compose.shrink_ratio = (lopts.compose.shrink_ratio + shrink_add).min(0.5);
    }
    // Q-20: glyph scaling. When `Min/Max GlyphScaling` differ from
    // 100 the IDML allows the composer to scale per-glyph x-advance
    // by that percentage. Per-glyph distribution after Knuth-Plass
    // is the proper implementation; for now we widen the stretch
    // ratio so the breaker has the budget the IDML implies. None of
    // the cycle-2 evidence packs vary this from 100, so this is
    // foundation work that lights up on packs that do customise it.
    let gs_desired = resolved.desired_glyph_scaling.unwrap_or(100.0);
    let gs_max = resolved.maximum_glyph_scaling.unwrap_or(gs_desired);
    let gs_min = resolved.minimum_glyph_scaling.unwrap_or(gs_desired);
    if (gs_max - gs_desired).abs() > 0.01 || (gs_desired - gs_min).abs() > 0.01 {
        let extra_stretch = ((gs_max - gs_desired) / 100.0).max(0.0);
        let extra_shrink = ((gs_desired - gs_min) / 100.0).max(0.0);
        lopts.compose.stretch_ratio = (lopts.compose.stretch_ratio + extra_stretch).min(2.0);
        lopts.compose.shrink_ratio = (lopts.compose.shrink_ratio + extra_shrink).min(0.5);
    }
    // CJK Stage 2: enable hard-kinsoku enforcement whenever the cascade
    // carries any `KinsokuType` ("WordbreakWithJustification" / "PushIn"
    // / "PushOut" / etc). The composer currently keys on presence only;
    // flavour-specific behaviour is queued under CJK Stage 4.
    lopts.compose.kinsoku_enforce = resolved.kinsoku_type.is_some();
    // Phase 7 — enable Mojikumi half-width tightening when the
    // cascade resolves a `MojikumiTable` or `MojikumiSet` reference.
    // The MVP applies a uniform "halve CJK punctuation advance"
    // rule rather than per-table per-adjacency lookups; richer
    // table-driven behaviour is queued.
    lopts.compose.mojikumi_half_width =
        resolved.mojikumi_table.is_some() || resolved.mojikumi_set.is_some();
}

/// Phase 7 — emit Kenten emphasis marks above glyphs whose source
/// run carries a `KentenKind` other than `"None"`. The mark is a
/// small filled black circle stamped above the base glyph's centre
/// at a fixed fraction of the run's point size. Per-glyph cluster
/// → run lookup is done inline so we don't need to thread a picker
/// or build a side index.
///
/// Position: mark sits ~0.4 × point_size above the line's baseline
/// (above the cap line of typical CJK fonts). Mark diameter =
/// 0.18 × point_size (slightly smaller than ideographic full-
/// width). The mark scales with the run's point size so kenten
/// over headlines vs. body text reads at proportional weight.
fn emit_kenten_for_line(
    line: &paged_text::layout::LaidOutLine,
    paragraph: &paged_parse::Paragraph,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    use paged_text::shape::ADVANCE_PRECISION;
    if line.glyphs.is_empty() || paragraph.runs.is_empty() {
        return;
    }
    // Build a tiny cluster → run index. Linear walk over
    // paragraph.runs accumulating byte lengths.
    let mut run_byte_ends: Vec<usize> = Vec::with_capacity(paragraph.runs.len());
    let mut acc = 0usize;
    for r in &paragraph.runs {
        acc += r.text.len();
        run_byte_ends.push(acc);
    }
    // Fast bail when no run has a Kenten mark to render.
    let any_kenten = resolved_runs.iter().any(|r| {
        r.kenten_kind
            .as_deref()
            .map(|k| !k.eq_ignore_ascii_case("None"))
            .unwrap_or(false)
    });
    if !any_kenten {
        return;
    }
    let (ox, oy) = frame_origin_pt;
    let mark_paint = Paint::Solid(Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    for g in &line.glyphs {
        let cluster = g.cluster as usize;
        // Find the run that owns this cluster.
        let run_idx = run_byte_ends
            .iter()
            .position(|&end| cluster < end)
            .unwrap_or(run_byte_ends.len() - 1);
        let Some(resolved) = resolved_runs.get(run_idx) else {
            continue;
        };
        let kind = match resolved.kenten_kind.as_deref() {
            Some(k) if !k.eq_ignore_ascii_case("None") => k,
            _ => continue,
        };
        let point_size = g.point_size.max(1.0);
        let mark_diameter = point_size * 0.18;
        // Centre of mark = centre of glyph's advance, sitting
        // 0.4 × point_size above the baseline. Mark fill colour
        // currently follows a fixed black (KentenKind variants
        // map to the same simple dot today).
        let _ = kind; // variants share the simple-dot shape MVP.
        let glyph_x_pt = g.x as f32 / ADVANCE_PRECISION;
        let glyph_adv_pt = g.x_advance as f32 / ADVANCE_PRECISION;
        let centre_x = ox + glyph_x_pt + glyph_adv_pt * 0.5;
        let baseline_y_pt = g.y as f32 / ADVANCE_PRECISION;
        let centre_y = oy + baseline_y_pt - point_size * 0.95;
        let rect = Rect {
            x: centre_x - mark_diameter * 0.5,
            y: centre_y - mark_diameter * 0.5,
            w: mark_diameter,
            h: mark_diameter,
        };
        emit_ellipse(rect, mark_paint, list);
    }
}

/// Phase 7 — emit ruby annotations above runs whose `ruby_flag` is
/// set. The MVP shapes `ruby_string` once per ruby-tagged run via
/// the document's fallback font at 0.5 × base point size, centers
/// the result horizontally over the base run's glyph span, and
/// places it 1.05 × base point size above the line's baseline (i.e.
/// just above the cap line).
///
/// Limitations called out:
/// - Uses the fallback font for shaping (the run's own font may
///   carry better glyphs for Japanese kana but the fallback at
///   least always has SOME glyph). This is good enough for visible
///   confirmation; replacing it with the run's resolved face is a
///   follow-up.
/// - `PerCharacter` ruby (one ruby char per base char) collapses to
///   the same "centered group" placement as `GroupRuby`. Per-
///   character distribution requires aligning ruby char N over
///   base char N which the MVP skips.
fn emit_ruby_for_line(
    line: &paged_text::layout::LaidOutLine,
    paragraph: &paged_parse::Paragraph,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    font_bytes: &[u8],
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    use paged_text::shape::ADVANCE_PRECISION;
    if line.glyphs.is_empty() || paragraph.runs.is_empty() {
        return;
    }
    // Fast bail when no run has ruby.
    let any_ruby = resolved_runs.iter().any(|r| r.ruby_flag.unwrap_or(false));
    if !any_ruby {
        return;
    }
    // Construct a shaping + outlining face for the ruby text.
    let Some(rb_face) = rustybuzz::Face::from_slice(font_bytes, 0) else {
        return;
    };
    let Ok(ttf_face) = ttf_parser::Face::parse(font_bytes, 0) else {
        return;
    };
    let outliner = TtfOutliner::new(&ttf_face);
    // Build cluster → run index lookup.
    let mut run_byte_ends: Vec<usize> = Vec::with_capacity(paragraph.runs.len());
    let mut acc = 0usize;
    for r in &paragraph.runs {
        acc += r.text.len();
        run_byte_ends.push(acc);
    }
    let (ox, oy) = frame_origin_pt;
    let ruby_paint = Paint::Solid(Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    // For each run with ruby, find its glyph span in the line and
    // emit centered ruby. We do per-run independently so multiple
    // ruby runs in one line work.
    for (run_idx, resolved) in resolved_runs.iter().enumerate() {
        if !resolved.ruby_flag.unwrap_or(false) {
            continue;
        }
        let ruby_text = match resolved.ruby_string.as_deref() {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };
        let run_start = if run_idx == 0 {
            0
        } else {
            run_byte_ends[run_idx - 1]
        };
        let run_end = run_byte_ends[run_idx];
        // Find min/max x among glyphs in this run's cluster range.
        let mut x_min = i32::MAX;
        let mut x_max = i32::MIN;
        let mut base_point_size: f32 = 0.0;
        let mut baseline_y_64: i32 = 0;
        for g in &line.glyphs {
            let c = g.cluster as usize;
            if c < run_start || c >= run_end {
                continue;
            }
            x_min = x_min.min(g.x);
            x_max = x_max.max(g.x + g.x_advance);
            base_point_size = base_point_size.max(g.point_size);
            baseline_y_64 = g.y; // last wins; all glyphs on a line share baseline
        }
        if x_min == i32::MAX || base_point_size <= 0.0 {
            continue;
        }
        // Shape the ruby string at half the base point size.
        let ruby_pt = base_point_size * 0.5;
        let shaped = paged_text::shape_run(&rb_face, ruby_text, ruby_pt);
        if shaped.glyphs.is_empty() {
            continue;
        }
        // Centre the shaped advance over the base x span.
        let base_x_left_pt = x_min as f32 / ADVANCE_PRECISION;
        let base_x_right_pt = x_max as f32 / ADVANCE_PRECISION;
        let base_centre_pt = (base_x_left_pt + base_x_right_pt) * 0.5;
        let ruby_advance_pt = shaped.total_advance as f32 / ADVANCE_PRECISION;
        let ruby_origin_x_pt = base_centre_pt - ruby_advance_pt * 0.5;
        // Position above the baseline by 1.05 × base point size.
        let baseline_y_pt = baseline_y_64 as f32 / ADVANCE_PRECISION;
        let ruby_origin_y_pt = baseline_y_pt - base_point_size * 1.05;
        // Convert shape glyphs to PositionedGlyph at the ruby
        // origin. Each glyph's x is the running advance sum.
        let mut positioned: Vec<paged_text::PositionedGlyph> =
            Vec::with_capacity(shaped.glyphs.len());
        let mut cursor = 0i32;
        for g in &shaped.glyphs {
            positioned.push(paged_text::PositionedGlyph {
                glyph_id: g.glyph_id,
                cluster: g.cluster,
                x: cursor + g.x_offset,
                y: g.y_offset,
                x_advance: g.x_advance,
                font_id: u32::MAX, // sentinel: ruby uses the fallback face directly via outliner
                point_size: ruby_pt,
                underline: false,
                strikethru: false,
                x_scale: 1.0,
                y_scale: 1.0,
                skew_deg: 0.0,
                ch: None,
            });
            cursor = cursor.saturating_add(g.x_advance);
        }
        emit_glyph_slice(
            &positioned,
            u32::MAX,
            ruby_pt,
            |_| ruby_paint,
            (ox + ruby_origin_x_pt, oy + ruby_origin_y_pt),
            &outliner,
            list,
        );
    }
}

/// Walk a laid-out line's glyphs and emit horizontal stroke
/// commands for any underlined or struck-through ranges. The stroke
/// uses the run's resolved fill colour (per cluster, via the same
/// picker as the glyphs themselves) so coloured text gets coloured
/// decoration.
fn emit_line_decorations(
    line: &paged_text::layout::LaidOutLine,
    picker: &RunPaintPicker,
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    use paged_text::shape::ADVANCE_PRECISION;
    if line.glyphs.is_empty() {
        return;
    }
    // Two passes — underline (12% of em below baseline) then
    // strikethrough (30% above) — so a glyph with both gets two
    // stripes. Offsets are crude approximations until we read the
    // font's `OS/2` table for the spec'd y_offset / strikeout_pos.
    const UNDERLINE_OFFSET_EM: f32 = 0.12;
    const STRIKETHRU_OFFSET_EM: f32 = -0.30;
    type Pred = fn(&paged_text::PositionedGlyph) -> bool;
    let underline: Pred = |g| g.underline;
    let strikethru: Pred = |g| g.strikethru;
    for (predicate, y_offset_factor) in [
        (underline, UNDERLINE_OFFSET_EM),
        (strikethru, STRIKETHRU_OFFSET_EM),
    ] {
        let mut start = 0;
        while start < line.glyphs.len() {
            if !predicate(&line.glyphs[start]) {
                start += 1;
                continue;
            }
            let mut end = start + 1;
            while end < line.glyphs.len() && predicate(&line.glyphs[end]) {
                end += 1;
            }
            let g0 = &line.glyphs[start];
            let g_last = &line.glyphs[end - 1];
            let x_start_pt = frame_origin_pt.0 + (g0.x as f32) / ADVANCE_PRECISION;
            let x_end_pt =
                frame_origin_pt.0 + ((g_last.x + g_last.x_advance) as f32) / ADVANCE_PRECISION;
            let baseline_pt = frame_origin_pt.1 + (line.baseline_y as f32) / ADVANCE_PRECISION;
            let y_pt = baseline_pt + g0.point_size * y_offset_factor;
            let stroke_w = (g0.point_size * 0.06_f32).max(0.4);
            // Decoration paint matches the run's fill at the start
            // glyph's cluster.
            let paint = picker.pick(g0.cluster);
            paged_compose::emit_line(
                x_start_pt,
                y_pt,
                x_end_pt,
                y_pt,
                Stroke::new(stroke_w),
                paint,
                list,
            );
            start = end;
        }
    }
}

/// Map an IDML `Justification` enum value to `paged_text::Alignment`.
/// `None` (no attribute on the cascade) falls back to `Left`, the
/// IDML default.
///
/// `ToBindingSide` / `AwayFromBindingSide` are binding-aware values
/// that ideally consult the spread's page side (left vs. right). We
/// don't plumb binding side through to the composer today, so they
/// resolve to `Left` / `Right` respectively — matches the historical
/// stringly-typed behaviour, which fell through to `Left` for any
/// unrecognised string.
/// Phase 4 typography — one nested-style application: the half-open
/// byte range of the paragraph text that the override character style
/// should apply to. `byte_range.start` is inclusive; `byte_range.end`
/// is exclusive. `applied_character_style` mirrors
/// [`paged_parse::NestedStyle::applied_character_style`].
#[derive(Debug, Clone, PartialEq)]
pub struct NestedStyleApplication {
    pub byte_range: std::ops::Range<usize>,
    pub applied_character_style: String,
}

/// Phase 4 typography — walk a paragraph's text against its cascaded
/// `<NestedStyle>` list, producing the half-open byte ranges each
/// override should apply to. The first entry's range starts at byte 0;
/// each subsequent entry starts where the previous one ended. Returns
/// an empty vec when `nested_styles` is empty or when every entry has
/// an unsupported delimiter / zero repetition.
///
/// The walker handles every `NestedDelimiter` variant. Single-paragraph
/// scope: the walker stops when the cursor reaches the end of
/// `paragraph_text`, even if some entries are unconsumed (their range
/// would extend past the text). This matches InDesign's behaviour for
/// short paragraphs.
pub fn compute_nested_style_overlay(
    paragraph_text: &str,
    nested_styles: &[paged_parse::NestedStyle],
) -> Vec<NestedStyleApplication> {
    if nested_styles.is_empty() || paragraph_text.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<NestedStyleApplication> = Vec::new();
    let mut cursor: usize = 0;
    for ns in nested_styles {
        if cursor >= paragraph_text.len() {
            break;
        }
        if ns.repetition <= 0 {
            continue;
        }
        let end = find_nested_end(
            paragraph_text,
            cursor,
            &ns.delimiter,
            ns.repetition,
            ns.inclusive,
        );
        if end > cursor {
            out.push(NestedStyleApplication {
                byte_range: cursor..end,
                applied_character_style: ns.applied_character_style.clone(),
            });
            cursor = end;
        }
    }
    out
}

/// Internal — locate the byte offset where a nested-style range ends,
/// scanning `text[start..]` for `repetition` occurrences of
/// `delimiter`. Returns `text.len()` when fewer than `repetition`
/// matches are found (the range stretches to the paragraph's end).
fn find_nested_end(
    text: &str,
    start: usize,
    delimiter: &paged_parse::NestedDelimiter,
    repetition: i32,
    inclusive: bool,
) -> usize {
    use paged_parse::NestedDelimiter as D;
    let bytes = text.as_bytes();
    let slice = &text[start..];
    // For Words / Sentences / Characters the count is the number of
    // logical units to traverse, INCLUDING the trailing boundary.
    // For class matchers (AnyDigit / AnyLetter / quote pairs / Char /
    // Tab) it's the count of matches.
    match delimiter {
        D::Characters => {
            // Walk `repetition` Unicode scalar values (≠ bytes).
            let mut indices = slice.char_indices();
            for _ in 0..repetition {
                if indices.next().is_none() {
                    return text.len();
                }
            }
            // `indices.offset()` is unstable; reconstruct via next().
            match indices.next() {
                Some((off, _)) => start + off,
                None => text.len(),
            }
        }
        D::Words => {
            // Walk `repetition` words. A word is a maximal run of
            // non-whitespace chars; the boundary after a word is its
            // trailing whitespace.
            let mut idx = 0usize;
            let mut words_seen = 0;
            let mut in_word = false;
            // Skip leading whitespace so word 1 starts at the first
            // non-space char (matches InDesign).
            while idx < slice.len() && slice.as_bytes()[idx].is_ascii_whitespace() {
                idx += 1;
            }
            while idx < slice.len() {
                let b = slice.as_bytes()[idx];
                if b.is_ascii_whitespace() {
                    if in_word {
                        words_seen += 1;
                        in_word = false;
                        if words_seen >= repetition {
                            // Boundary candidate is the trailing space.
                            if inclusive {
                                // Consume whitespace run; range ends
                                // after the last whitespace byte.
                                while idx < slice.len()
                                    && slice.as_bytes()[idx].is_ascii_whitespace()
                                {
                                    idx += 1;
                                }
                            }
                            return start + idx;
                        }
                    }
                } else {
                    in_word = true;
                }
                idx += 1;
            }
            // Reaching here means fewer than `repetition` word boundaries
            // were found before the slice ended (the in-loop check at the
            // `repetition`th word returns early), so the boundary is the
            // end of the text — whether or not it ended mid-word.
            text.len()
        }
        D::Sentences => {
            // A sentence boundary is `.`, `!`, or `?` followed by
            // optional whitespace.
            let mut idx = 0usize;
            let mut sentences_seen = 0;
            while idx < slice.len() {
                let b = slice.as_bytes()[idx];
                if matches!(b, b'.' | b'!' | b'?') {
                    sentences_seen += 1;
                    if sentences_seen >= repetition {
                        if inclusive {
                            idx += 1;
                            // Consume the trailing whitespace run too.
                            while idx < slice.len() && slice.as_bytes()[idx].is_ascii_whitespace() {
                                idx += 1;
                            }
                        }
                        return start + idx;
                    }
                }
                idx += 1;
            }
            text.len()
        }
        D::AnyDigit => find_class_end(text, start, repetition, inclusive, |c| c.is_ascii_digit()),
        D::AnyLetter => find_class_end(text, start, repetition, inclusive, |c| c.is_alphabetic()),
        D::AnyDoubleQuotes => find_class_end(text, start, repetition, inclusive, |c| {
            matches!(c, '"' | '\u{201C}' | '\u{201D}')
        }),
        D::AnySingleQuotes => find_class_end(text, start, repetition, inclusive, |c| {
            matches!(c, '\'' | '\u{2018}' | '\u{2019}')
        }),
        D::Tab => find_class_end(text, start, repetition, inclusive, |c| c == '\t'),
        D::ForcedLineBreak => find_class_end(text, start, repetition, inclusive, |c| {
            // U+2028 LINE SEPARATOR; IDML serialises forced line
            // breaks as `<Br/>` which the parser materialises as `\n`
            // in run text.
            c == '\n' || c == '\u{2028}'
        }),
        D::EndNestedStyle => {
            // U+0003 END OF TEXT — InDesign's "End Nested Style Here"
            // marker. Inserted by the user via a special character.
            find_class_end(text, start, repetition, inclusive, |c| c == '\u{0003}')
        }
        D::Char(target) => find_class_end(text, start, repetition, inclusive, |c| c == *target),
        D::Unknown => start,
    }
    .min(bytes.len())
}

fn find_class_end<F: Fn(char) -> bool>(
    text: &str,
    start: usize,
    repetition: i32,
    inclusive: bool,
    is_match: F,
) -> usize {
    let mut matches = 0;
    for (off, c) in text[start..].char_indices() {
        if is_match(c) {
            matches += 1;
            if matches >= repetition {
                let abs = start + off;
                let end = if inclusive { abs + c.len_utf8() } else { abs };
                return end;
            }
        }
    }
    text.len()
}

/// Phase 4 typography — apply a nested-style overlay to a paragraph's
/// character runs. Returns a new run vec where each run that overlaps
/// a `<NestedStyle>` range has been split so its `character_style`
/// field carries the override id. Runs that don't touch any overlay
/// range pass through unchanged.
///
/// Empty overlay → returns `runs.to_vec()`. The walker preserves run
/// ordering: any run produced by splitting one source run appears in
/// the same paragraph-byte-order position. All non-text fields on a
/// split run are cloned from the source run — only the override
/// `character_style` differs.
pub fn split_runs_for_nested_styles(
    runs: &[paged_parse::CharacterRun],
    overlay: &[NestedStyleApplication],
) -> Vec<paged_parse::CharacterRun> {
    if overlay.is_empty() {
        return runs.to_vec();
    }
    // Build a per-byte map of "what character style overrides this
    // position?" Sparse: only the bytes covered by some overlay range
    // are touched. We build it as a sorted Vec of (range, style) and
    // do binary search per-run-byte during splitting.
    let mut out: Vec<paged_parse::CharacterRun> = Vec::with_capacity(runs.len());
    let mut cursor: usize = 0; // paragraph-byte position of the next run.
    for run in runs {
        let run_start = cursor;
        let run_end = cursor + run.text.len();
        cursor = run_end;
        // Compute the set of overlay-defined boundaries inside this
        // run, plus the run's own start and end. Then walk the sorted
        // boundaries and emit a fragment per (start, end) pair.
        let mut boundaries: Vec<usize> = vec![run_start, run_end];
        for ov in overlay {
            if ov.byte_range.start > run_start && ov.byte_range.start < run_end {
                boundaries.push(ov.byte_range.start);
            }
            if ov.byte_range.end > run_start && ov.byte_range.end < run_end {
                boundaries.push(ov.byte_range.end);
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        for window in boundaries.windows(2) {
            let frag_start = window[0];
            let frag_end = window[1];
            if frag_start >= frag_end {
                continue;
            }
            // Find an overlay whose range covers frag_start (any
            // byte inside the fragment maps to the same override
            // because we split at every overlay boundary).
            let override_style = overlay
                .iter()
                .find(|ov| frag_start >= ov.byte_range.start && frag_start < ov.byte_range.end)
                .map(|ov| ov.applied_character_style.clone());
            let local_lo = frag_start - run_start;
            let local_hi = frag_end - run_start;
            let mut frag = run.clone();
            frag.text = run.text[local_lo..local_hi].to_string();
            if let Some(s) = override_style {
                frag.character_style = Some(s);
            }
            out.push(frag);
        }
    }
    out
}

/// Phase 4 typography — translate a `ResolvedRunAttrs`'s `Ligatures` /
/// `KerningMethod` into the shaper's [`paged_text::ShapingFeatures`].
/// Inputs are `None`-tolerant: missing `ligatures_on` defaults to true
/// (InDesign's CharacterStyle default); unrecognised `kerning_method`
/// strings fall through to `Metrics`. `"Optical"` falls through to
/// `Optical` even though the renderer currently shapes it the same as
/// `Metrics` — the cache key still distinguishes the two so the
/// optical-kerning pass can land later without invalidating the cache.
/// Resolve an IDML `Position` (super/subscript) into a `(size_factor,
/// baseline_offset_fraction)` pair, both relative to the run's base
/// point size. A positive offset lifts the glyphs (superscript); a
/// negative one drops them (subscript) — matching `baseline_shift_pt`'s
/// sign convention in the layout emit.
///
/// InDesign derives the exact factors from the document's Superscript /
/// Subscript Size & Position text preferences; we use its factory
/// defaults (58.3 % size, ±33.3 % of the base size) because
/// `Resources/Preferences.xml` is not parsed yet (a separate gap). The
/// OpenType variants (`OT*`, `Numerator`/`Denominator`) reuse the same
/// geometric fallback until real OT feature lookup lands.
pub fn position_metrics(position: Option<&str>) -> (f32, f32) {
    const SIZE_FACTOR: f32 = 0.583;
    const OFFSET_FACTOR: f32 = 0.333;
    match position {
        Some("Superscript") | Some("OTSuperscript") | Some("OTNumerator") => {
            (SIZE_FACTOR, OFFSET_FACTOR)
        }
        Some("Subscript") | Some("OTSubscript") | Some("OTDenominator") => {
            (SIZE_FACTOR, -OFFSET_FACTOR)
        }
        // `Normal` / `None` / unknown ⇒ identity.
        _ => (1.0, 0.0),
    }
}

/// Combine a run's base point size, its explicit `BaselineShift`, and
/// its `Position` (super/subscript) into the `(point_size,
/// baseline_shift_pt)` pair the layout emit consumes.
///
/// - `point_size` shrinks by the `Position` size factor (super/subscript
///   render at a fraction of the base; `Normal` keeps the base).
/// - `baseline_shift_pt` adds the `Position` baseline offset (a fraction
///   of the *base* size) on top of any explicit `BaselineShift`, so a
///   superscript both lifts and shrinks while an explicit shift still
///   composes additively. The offset is computed against the base size
///   (not the shrunk size) to match InDesign's geometry.
pub fn position_adjusted_metrics(
    base_size: f32,
    explicit_baseline_shift: Option<f32>,
    position: Option<&str>,
) -> (f32, f32) {
    let (size_factor, offset_fraction) = position_metrics(position);
    let point_size = base_size * size_factor;
    let baseline_shift_pt = explicit_baseline_shift.unwrap_or(0.0) + base_size * offset_fraction;
    (point_size, baseline_shift_pt)
}

pub fn shaping_features_from(
    ligatures_on: Option<bool>,
    kerning_method: Option<&str>,
    otf: &paged_parse::OtfFeatures,
) -> paged_text::ShapingFeatures {
    use paged_text::KerningMethod as K;
    paged_text::ShapingFeatures {
        ligatures_on: ligatures_on.unwrap_or(true),
        kerning: match kerning_method {
            Some("None") => K::Off,
            Some("Optical") => K::Optical,
            // "Metrics" or anything else (incl. None) → default.
            _ => K::Metrics,
        },
        // Discrete OTF toggles: a `None` flag at the bottom of the
        // cascade means the feature is off (its OpenType default).
        // `OTFContextualAlternate` is the exception — fonts opt into
        // `calt` by default, so only an explicit `false` disables it.
        discretionary_ligatures: otf.discretionary_ligatures.unwrap_or(false),
        fractions: otf.fraction.unwrap_or(false),
        ordinals: otf.ordinal.unwrap_or(false),
        swash: otf.swash.unwrap_or(false),
        slashed_zero: otf.slashed_zero.unwrap_or(false),
        titling: otf.titling.unwrap_or(false),
        contextual_alternates: otf.contextual_alternates.unwrap_or(true),
        figure_style: paged_text::FigureStyle::from_idml(otf.figure_style.as_deref()),
        // Negative / absent bitfields ⇒ no stylistic set.
        stylistic_sets: otf.stylistic_sets.unwrap_or(0).max(0) as u32,
    }
}

pub fn map_justification(j: Option<paged_parse::Justification>) -> paged_text::Alignment {
    use paged_parse::Justification as J;
    match j {
        Some(J::RightAlign) | Some(J::RightJustified) | Some(J::AwayFromBindingSide) => {
            paged_text::Alignment::Right
        }
        Some(J::CenterAlign) | Some(J::CenterJustified) => paged_text::Alignment::Center,
        Some(J::FullyJustified) | Some(J::LeftJustified) => paged_text::Alignment::Justify,
        Some(J::LeftAlign) | Some(J::ToBindingSide) | None => paged_text::Alignment::Left,
    }
}

fn map_tab_alignment(a: Option<&str>) -> paged_text::layout::TabAlignment {
    match a {
        Some("RightAlign") => paged_text::layout::TabAlignment::Right,
        Some("CenterAlign") => paged_text::layout::TabAlignment::Center,
        Some("CharacterAlign") => paged_text::layout::TabAlignment::Decimal,
        _ => paged_text::layout::TabAlignment::Left,
    }
}

/// Per-render font cache. Pre-resolves every distinct (family, style)
/// pair referenced anywhere in the document via the configured
/// `AssetResolver`. Falls back to `options.font` when nothing
/// resolves. Also extracts OS/2 / hhea metrics per font_id at
/// build time so baseline math doesn't have to re-parse the font
/// per paragraph.
///
/// Field declaration order matters: `faces` is declared FIRST so on
/// drop it is dropped FIRST — before `face_bytes`. The cached
/// `rustybuzz::Face<'static>` values borrow from the `Bytes` stored
/// in `face_bytes`; if we dropped `face_bytes` first the Faces would
/// briefly hold dangling references. Rust drops struct fields in
/// declaration order (first declared = first dropped), so keeping
/// `faces` above `face_bytes` is load-bearing for soundness.
/// Per-render shaping resource: the resolved bytes + configured
/// rustybuzz Faces for every (family, style, wght) referenced by the
/// document. Built once per `build_document` call by default; the
/// caller can pre-build it (via `FontTable::build`) and pass it
/// through `PipelineOptions::pre_built_font_table` to amortise the
/// ~225ms harvest-and-resolve cost across gesture rebuilds on
/// image-heavy fixtures. See the SAFETY contract on `faces` below
/// — the struct can be moved + held by a long-lived caller (e.g.
/// `CanvasModel`) without invalidating the Face references.
pub struct FontTable {
    /// Pre-configured rustybuzz `Face` cache keyed by
    /// `(font_id, wght_bits)`. The `Face<'static>` lifetime is a
    /// LIE narrowed back to `&self` at the public accessor.
    ///
    /// SAFETY contract (also enforced at each insertion site):
    ///   1. Each cached Face borrows from the `Bytes` stored under
    ///      the matching `font_id` key in `face_bytes`. `bytes::Bytes`
    ///      is refcounted with a stable heap pointer — the underlying
    ///      buffer cannot move while any clone is alive.
    ///   2. `face_bytes` is never removed-from or overwritten after
    ///      `FontTable::build` returns: it is populated inside
    ///      `build` and never touched by any later method. Therefore
    ///      the buffer a cached Face borrows from outlives that Face.
    ///   3. `faces` is declared before `face_bytes`, so on `Drop` the
    ///      Faces are dropped first — they never see a freed `Bytes`.
    ///   4. The accessor [`Self::face`] returns `&rustybuzz::Face<'_>`
    ///      with the lifetime narrowed to `&self`. No caller ever
    ///      observes the `'static` lifetime, so the lie can't escape.
    ///   5. Variations are baked in at insert time. The cached Face
    ///      is never mutated post-insert (no `&mut Face` is ever
    ///      exposed). Two runs with the same bytes but different
    ///      `wght` use distinct cache keys → distinct cached Faces.
    faces: HashMap<(u32, u32), rustybuzz::Face<'static>>,
    /// Bytes kept alive for `faces` to point into. One entry per
    /// distinct `font_id` (the wght variant is irrelevant — same
    /// buffer, just different variation state on the Face).
    ///
    /// Marked `dead_code`-allow: the field is never read after
    /// `build`, but its EXISTENCE is load-bearing — drop-time
    /// soundness of `faces` (which holds `Face<'static>` references
    /// into these buffers) depends on this map keeping the `Bytes`
    /// values alive for at least as long as `faces`. See the SAFETY
    /// contract on `faces` above.
    #[allow(dead_code)]
    face_bytes: HashMap<u32, Bytes>,
    cache: HashMap<(String, Option<String>), Bytes>,
    fallback: Option<Bytes>,
    /// Metrics keyed by `fnv_1a_u32(bytes)` (same id the rest of
    /// the pipeline uses for glyph-cache routing).
    metrics: HashMap<u32, FontMetrics>,
    /// Per-IDML-family metric override. Populated from
    /// `PipelineOptions::font_metrics_overrides` and consulted FIRST
    /// by `metrics_for_family` so a substitute font doesn't force its
    /// own ascender / cap-height onto baseline math when the IDML
    /// names a different family. Empty when no overrides were set.
    family_metrics: HashMap<String, FontMetrics>,
}

/// Per-font metrics the renderer reads at baseline-placement time.
/// All values are scale-free (unit = font units / `units_per_em`)
/// so callers can multiply by `point_size` to get pt.
#[derive(Debug, Clone, Copy)]
struct FontMetrics {
    /// `OS/2.sCapHeight`, fraction of em. `None` when the font
    /// doesn't expose it (legacy fonts without the OS/2 v2+ field).
    cap_height: Option<f32>,
    /// `OS/2.sxHeight`, fraction of em.
    x_height: Option<f32>,
    /// `hhea.ascender`, fraction of em. Always present.
    ascender: f32,
    /// `hhea.descender`, fraction of em, stored as a POSITIVE distance
    /// below the baseline (ttf-parser returns a negative value; we flip
    /// the sign at parse time). Used by the anchored `TopOfLeading`
    /// vertical reference point to split a line's leading into its
    /// above- and below-baseline portions in the font's own
    /// ascent:descent proportion (InDesign's leading model).
    descender: f32,
}

impl FontTable {
    /// Concept 3 (PDF export) — the ORIGINAL face bytes for a
    /// font-table id, for subsetting/embedding. `None` for unknown
    /// ids.
    pub fn face_data(&self, font_id: u32) -> Option<&[u8]> {
        self.face_bytes.get(&font_id).map(|b| b.as_ref())
    }

    pub fn build(document: &Document, options: &PipelineOptions) -> Self {
        let fallback = options.font.map(Bytes::copy_from_slice);
        let mut cache: HashMap<(String, Option<String>), Bytes> = HashMap::new();
        if let Some(resolver) = options.assets {
            // Walk every run in every story and collect distinct
            // keys before calling the resolver — `resolve_font`
            // may be a JS Promise wrapper or a disk read, so
            // deduping matters. Each run's effective (family,
            // style) comes from the cascade (run direct > applied
            // character style > applied paragraph style) so a run
            // that only carries `AppliedCharacterStyle` still
            // requests the right font.
            let mut keys: std::collections::HashSet<(String, Option<String>)> =
                std::collections::HashSet::new();
            // Helper: harvest font keys from a paragraph + every
            // run nested inside its table cells (cells host their
            // own ParagraphStyleRange children; their runs never
            // surface through the outer story paragraph list).
            fn harvest_keys(
                document: &Document,
                paragraph: &paged_parse::Paragraph,
                keys: &mut std::collections::HashSet<(String, Option<String>)>,
            ) {
                for run in &paragraph.runs {
                    let resolved = document.resolved_run_attrs(paragraph, run);
                    if let Some(family) = resolved.font {
                        keys.insert((family, resolved.font_style));
                    }
                }
                if let Some(table) = paragraph.table.as_ref() {
                    for cell in &table.cells {
                        for inner in &cell.paragraphs {
                            harvest_keys(document, inner, keys);
                        }
                    }
                }
            }
            for parsed in &document.stories {
                for paragraph in &parsed.story.paragraphs {
                    harvest_keys(document, paragraph, &mut keys);
                }
            }
            cache.reserve(keys.len());
            for key in keys {
                if let Some(bytes) = resolver.resolve_font(&key.0, key.1.as_deref()) {
                    cache.insert(key, bytes);
                }
            }
        }
        // Pre-build the shaping-Face cache. Walk every run again to
        // collect each distinct `(font_id, wght_bits)` actually used
        // across all stories (incl. nested table cells). The first
        // pass above resolves bytes from the asset resolver; the wght
        // axis value comes from the per-run resolved `FontStyle`.
        // Storing the configured Face here (vs. per-paragraph) lets
        // the shaping sites in `emit_paragraph_into_chain`,
        // `emit_cell_paragraph`, and `measure_cell_paragraph` share
        // one rustybuzz::Face across the entire render — Adobe-typical
        // docs reuse the same (font, weight) thousands of times.
        let mut face_keys: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        let mut id_to_bytes: HashMap<u32, Bytes> = HashMap::new();
        let harvest_face_keys =
            |paragraph: &paged_parse::Paragraph,
             face_keys: &mut std::collections::HashSet<(u32, u32)>,
             id_to_bytes: &mut HashMap<u32, Bytes>| {
                // Inner walk: handle both top-level paragraphs and
                // recursive table-cell paragraphs.
                fn walk(
                    document: &Document,
                    cache: &HashMap<(String, Option<String>), Bytes>,
                    fallback: &Option<Bytes>,
                    paragraph: &paged_parse::Paragraph,
                    face_keys: &mut std::collections::HashSet<(u32, u32)>,
                    id_to_bytes: &mut HashMap<u32, Bytes>,
                ) {
                    for run in &paragraph.runs {
                        let resolved = document.resolved_run_attrs(paragraph, run);
                        // Mirror `FontTable::bytes_for`: (family, style)
                        // direct hit, then bare-family, then fallback.
                        let bytes = resolved
                            .font
                            .as_deref()
                            .and_then(|f| {
                                cache
                                    .get(&(f.to_string(), resolved.font_style.clone()))
                                    .or_else(|| cache.get(&(f.to_string(), None)))
                            })
                            .or(fallback.as_ref());
                        if let Some(b) = bytes {
                            let font_id = fnv_1a_u32(b.as_ref());
                            let wght = wght_for_font_style(resolved.font_style.as_deref());
                            face_keys.insert((font_id, wght.to_bits()));
                            id_to_bytes.entry(font_id).or_insert_with(|| b.clone());
                        }
                    }
                    if let Some(table) = paragraph.table.as_ref() {
                        for cell in &table.cells {
                            for inner in &cell.paragraphs {
                                walk(document, cache, fallback, inner, face_keys, id_to_bytes);
                            }
                        }
                    }
                }
                walk(
                    document,
                    &cache,
                    &fallback,
                    paragraph,
                    face_keys,
                    id_to_bytes,
                );
            };
        for parsed in &document.stories {
            for paragraph in &parsed.story.paragraphs {
                harvest_face_keys(paragraph, &mut face_keys, &mut id_to_bytes);
            }
        }
        // Build `face_bytes` first (so the buffers are owned before
        // any Face borrows from them), then build `faces`. Per the
        // SAFETY contract on `faces`, the cached Face<'static>
        // borrows from the Bytes stored at the same `font_id` in
        // `face_bytes`; `Bytes` is a refcounted heap buffer whose
        // pointer is stable across clones, so the buffer is alive
        // for as long as the `face_bytes` map holds an entry.
        let face_bytes: HashMap<u32, Bytes> = id_to_bytes;
        let mut faces: HashMap<(u32, u32), rustybuzz::Face<'static>> =
            HashMap::with_capacity(face_keys.len());
        let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
        for (font_id, wght_bits) in face_keys {
            let Some(buf) = face_bytes.get(&font_id) else {
                continue;
            };
            // SAFETY: extending the byte slice's lifetime to 'static.
            //  1. `buf` is a `Bytes` stored in `face_bytes` and owned
            //     by `Self`. `bytes::Bytes` is a refcounted heap
            //     buffer with a stable interior pointer — the
            //     underlying allocation cannot move while any clone
            //     exists.
            //  2. The map `face_bytes` is never mutated after this
            //     `build` returns (it has no exposed `&mut` accessor)
            //     so the buffer survives as long as the `FontTable`.
            //  3. The cached `Face<'static>` is dropped before
            //     `face_bytes`: `faces` is declared above `face_bytes`
            //     in `FontTable`, and Rust drops struct fields in
            //     declaration order (first declared = first dropped).
            //  4. The public accessor [`Self::face`] returns
            //     `&rustybuzz::Face<'_>` with the lifetime re-anchored
            //     to `&self`, so the 'static lie never escapes the
            //     module.
            //  5. The Face is never mutated post-insert: no `&mut`
            //     reference to it is exposed. Variations are baked
            //     in at insert time below; (font_id, wght_bits) keys
            //     guarantee a bold-vs-regular pair sharing the same
            //     bytes ends up in distinct cache slots.
            let bytes_static: &'static [u8] =
                unsafe { std::mem::transmute::<&[u8], &'static [u8]>(buf.as_ref()) };
            let Some(mut face) = rustybuzz::Face::from_slice(bytes_static, 0) else {
                continue;
            };
            // Only bake a wght variation when the face actually exposes
            // a `wght` axis — otherwise `set_variations` silently no-ops
            // and the bold/light slot reuses Regular metrics (P-06).
            let has_wght_axis = face
                .variation_axes()
                .into_iter()
                .any(|axis| axis.tag == wght_tag);
            let wght = f32::from_bits(wght_bits);
            if has_wght_axis {
                face.set_variations(&[rustybuzz::Variation {
                    tag: wght_tag,
                    value: wght,
                }]);
            }
            faces.insert((font_id, wght_bits), face);
        }
        // Parse metrics for every distinct byte buffer we ended up
        // caching, plus the fallback. Keyed by the same fnv hash
        // emit_paragraph uses for font_id — so the lookup is direct.
        let mut metrics: HashMap<u32, FontMetrics> = HashMap::new();
        let mut record = |bytes: &[u8]| {
            let id = fnv_1a_u32(bytes);
            if metrics.contains_key(&id) {
                return;
            }
            if let Some(m) = parse_font_metrics(bytes) {
                metrics.insert(id, m);
            }
        };
        for b in cache.values() {
            record(b.as_ref());
        }
        if let Some(b) = fallback.as_ref() {
            record(b.as_ref());
        }
        // Family-keyed override map. The override carries an
        // ascender (mandatory) plus optional cap-height / x-height —
        // missing optional values fall back to whichever metrics
        // `parse_font_metrics` extracted from the substitute font
        // (looked up by lifting them out of `metrics` via the cache's
        // bytes hash for the same family). This lets a caller pin
        // only the ascender (the dominant axis for first-baseline
        // drift) while leaving the rest at the substitute's natural
        // values.
        let mut family_metrics: HashMap<String, FontMetrics> = HashMap::new();
        for (family, ov) in options.font_metrics_overrides {
            // Find the substitute's parsed metrics for sensible
            // defaults on missing optional fields.
            let substitute = cache
                .get(&(family.clone(), None))
                .or_else(|| {
                    cache
                        .iter()
                        .find_map(|((f, _), b)| if f == family { Some(b) } else { None })
                })
                .map(|b| fnv_1a_u32(b.as_ref()))
                .and_then(|id| metrics.get(&id))
                .copied()
                .unwrap_or(FontMetrics {
                    cap_height: None,
                    x_height: None,
                    ascender: ov.ascender,
                    descender: 0.0,
                });
            family_metrics.insert(
                family.clone(),
                FontMetrics {
                    ascender: ov.ascender,
                    cap_height: ov.cap_height.or(substitute.cap_height),
                    x_height: ov.x_height.or(substitute.x_height),
                    // Descender isn't an override axis (it only feeds the
                    // anchored leading split); inherit the substitute's.
                    descender: substitute.descender,
                },
            );
        }
        Self {
            faces,
            face_bytes,
            cache,
            fallback,
            metrics,
            family_metrics,
        }
    }

    /// Returns the cached, pre-configured shaping Face for the given
    /// `(font_id, wght_bits)` key. The returned reference's lifetime
    /// is narrowed to `&self`, hiding the underlying `'static` lie
    /// (see the SAFETY contract on `FontTable::faces`).
    ///
    /// Callers must NOT call `set_variations` on the returned Face —
    /// variations are baked in at cache-insert time. The signature
    /// (`&Face`, not `&mut Face`) enforces that at compile time.
    fn face(&self, font_id: u32, wght_bits: u32) -> Option<&rustybuzz::Face<'_>> {
        self.faces.get(&(font_id, wght_bits))
    }

    /// Look up the bytes a paragraph should shape with.
    /// Resolver hit > options.font fallback. `None` means no font
    /// is available — caller skips the paragraph.
    fn bytes_for(&self, family: Option<&str>, style: Option<&str>) -> Option<Bytes> {
        if let Some(family) = family {
            // Direct (family, style) hit, then bare-family hit, so
            // a doc that only registers "Body Font" still picks up
            // its bold runs.
            if let Some(b) = self
                .cache
                .get(&(family.to_string(), style.map(str::to_string)))
            {
                return Some(b.clone());
            }
            if let Some(b) = self.cache.get(&(family.to_string(), None)) {
                return Some(b.clone());
            }
        }
        self.fallback.clone()
    }

    /// Resolve a paragraph's per-run font bytes, filling any
    /// individually-unresolvable run with a paragraph-level fallback
    /// so a single bad run doesn't drop the entire paragraph. The
    /// per-paragraph fallback is, in order: the first sibling run
    /// that DID resolve (keeps the visual style closest to what the
    /// rest of the paragraph uses), then [`FontTable::fallback`]
    /// (the renderer-wide default font), then `None` — signalling
    /// no font is available anywhere and the caller should skip.
    ///
    /// Returns `None` when no run resolves AND no document-wide
    /// fallback is configured. In that case the paragraph still has
    /// to be dropped because there's nothing to shape with.
    fn resolve_paragraph_bytes(
        &self,
        runs: &[paged_scene::ResolvedRunAttrs],
    ) -> Option<Vec<Bytes>> {
        if runs.is_empty() {
            return None;
        }
        let per_run: Vec<Option<Bytes>> = runs
            .iter()
            .map(|r| self.bytes_for(r.font.as_deref(), r.font_style.as_deref()))
            .collect();
        let paragraph_fallback: Option<Bytes> = per_run
            .iter()
            .find_map(|b| b.clone())
            .or_else(|| self.fallback.clone());
        let paragraph_fallback = paragraph_fallback?;
        Some(
            per_run
                .into_iter()
                .map(|b| b.unwrap_or_else(|| paragraph_fallback.clone()))
                .collect(),
        )
    }

    fn metrics_for(&self, font_id: u32) -> Option<&FontMetrics> {
        self.metrics.get(&font_id)
    }

    /// Override-aware metrics lookup keyed by IDML family name.
    /// Returns the per-family override when present, otherwise falls
    /// through so the caller can try the byte-hash path.
    fn metrics_for_family(&self, family: &str) -> Option<&FontMetrics> {
        self.family_metrics.get(family)
    }
}

fn parse_font_metrics(bytes: &[u8]) -> Option<FontMetrics> {
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let upem = face.units_per_em() as f32;
    if upem <= 0.0 {
        return None;
    }
    Some(FontMetrics {
        cap_height: face.capital_height().map(|v| v as f32 / upem),
        x_height: face.x_height().map(|v| v as f32 / upem),
        ascender: face.ascender() as f32 / upem,
        // `hhea.descender` is negative (below the baseline); store the
        // magnitude so the leading-split math reads naturally.
        descender: (face.descender() as f32 / upem).abs(),
    })
}

fn fnv_1a_u32(bytes: &[u8]) -> u32 {
    // Stable per-render font-cache key; the u32 range collides in
    // ~2B fonts — enough for any realistic document.
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_do_not_collect_glyph_runs() {
        // Load-bearing invariant: the glyph side-channel is opt-in.
        // With the default options every capture site no-ops, so the
        // canvas/CI command stream stays byte-identical to before the
        // PDF exporter existed. Flipping this default would silently
        // change every consumer — do it only on purpose.
        assert!(!PipelineOptions::default().collect_glyph_runs);
    }

    #[test]
    fn position_metrics_super_sub_and_normal() {
        // Superscript / numerator lift (positive offset); subscript /
        // denominator drop (negative); both shrink to 58.3 %.
        assert_eq!(position_metrics(Some("Superscript")), (0.583, 0.333));
        assert_eq!(position_metrics(Some("OTSuperscript")), (0.583, 0.333));
        assert_eq!(position_metrics(Some("OTNumerator")), (0.583, 0.333));
        assert_eq!(position_metrics(Some("Subscript")), (0.583, -0.333));
        assert_eq!(position_metrics(Some("OTDenominator")), (0.583, -0.333));
        // Normal / absent / unknown ⇒ identity (no scale, no shift).
        assert_eq!(position_metrics(Some("Normal")), (1.0, 0.0));
        assert_eq!(position_metrics(None), (1.0, 0.0));
    }

    #[test]
    fn position_adjusted_metrics_super_sub_and_normal() {
        // Base 12pt, no explicit baseline shift.
        // Superscript: 12 * 0.583 = 6.996 pt, +12 * 0.333 = +3.996 pt.
        let (sz, sh) = position_adjusted_metrics(12.0, None, Some("Superscript"));
        assert!((sz - 6.996).abs() < 1e-3, "super size {sz}");
        assert!((sh - 3.996).abs() < 1e-3, "super shift {sh}");
        // Subscript drops (negative shift), same shrink.
        let (sz, sh) = position_adjusted_metrics(12.0, None, Some("Subscript"));
        assert!((sz - 6.996).abs() < 1e-3, "sub size {sz}");
        assert!((sh + 3.996).abs() < 1e-3, "sub shift {sh}");
        // Normal: untouched size, zero shift.
        assert_eq!(
            position_adjusted_metrics(12.0, None, Some("Normal")),
            (12.0, 0.0)
        );
        assert_eq!(position_adjusted_metrics(12.0, None, None), (12.0, 0.0));
    }

    #[test]
    fn position_adjusted_metrics_composes_explicit_baseline_shift() {
        // An explicit BaselineShift adds on top of the Position offset.
        // Base 10pt, explicit +2pt, superscript ⇒ shift = 2 + 10*0.333.
        let (sz, sh) = position_adjusted_metrics(10.0, Some(2.0), Some("Superscript"));
        assert!((sz - 5.83).abs() < 1e-3, "size {sz}");
        assert!((sh - (2.0 + 3.33)).abs() < 1e-3, "shift {sh}");
        // Explicit shift with Normal position is passed through verbatim
        // (no size change, no Position offset).
        let (sz, sh) = position_adjusted_metrics(10.0, Some(-1.5), None);
        assert_eq!(sz, 10.0);
        assert!((sh + 1.5).abs() < 1e-6, "shift {sh}");
    }

    #[test]
    fn stroke_for_custom_styles_dashed_dotted_striped_wavy() {
        use paged_parse::{StrokeStyleDef, StrokeStyleKind as K};
        let mk = |kind, pattern: &[f32]| {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "S".to_string(),
                StrokeStyleDef {
                    self_id: "S".to_string(),
                    name: None,
                    kind,
                    pattern: pattern.to_vec(),
                    stripes: Vec::new(),
                    wave_width: None,
                    wave_length: None,
                    gap_color: None,
                    gap_tint: None,
                },
            );
            m
        };
        let go = |kind, pat: &[f32]| {
            let m = mk(kind, pat);
            stroke_for(Some("S"), 2.0, None, None, None, Some(&m), &[])
        };
        // Custom Dashed + Dotted patterns are consumed (a real dash).
        assert!(!go(K::Dashed, &[3.0, 2.0]).dash.is_solid(), "dashed");
        assert!(!go(K::Dotted, &[0.0, 2.0]).dash.is_solid(), "dotted");
        // `stroke_for` is the low-level *single*-stroke builder; Striped
        // / Wavy can't be expressed as one dash, so it returns a solid
        // base. The multi-rule / sine geometry for those is produced at
        // the emit site by `classify_stroke_style` + `emit_styled_stroke`
        // (W1.2), not here.
        assert!(go(K::Striped, &[]).dash.is_solid(), "striped base → solid");
        assert!(go(K::Wavy, &[]).dash.is_solid(), "wavy base → solid");
    }

    /// W1.1 — a per-frame `StrokeDashAndGap` override takes PRECEDENCE
    /// over the `StrokeStyleDef` pattern (and over the built-in name
    /// table): the override feeds the dash slot verbatim regardless of
    /// the named style. An empty override falls back to the style.
    #[test]
    fn stroke_for_instance_dash_override_wins_over_style_pattern() {
        use paged_parse::{StrokeStyleDef, StrokeStyleKind as K};
        let mut styles = std::collections::BTreeMap::new();
        styles.insert(
            "S".to_string(),
            StrokeStyleDef {
                self_id: "S".to_string(),
                name: None,
                kind: K::Dashed,
                pattern: vec![3.0, 2.0],
                stripes: Vec::new(),
                wave_width: None,
                wave_length: None,
                gap_color: None,
                gap_tint: None,
            },
        );
        // Instance override [9, 4] beats the style's [3, 2].
        let overridden = stroke_for(Some("S"), 2.0, None, None, None, Some(&styles), &[9.0, 4.0]);
        assert_eq!(overridden.dash.as_slice(), &[9.0, 4.0]);
        // Override wins even against a SOLID built-in name (no style def).
        let on_solid = stroke_for(
            Some("StrokeStyle/$ID/Solid"),
            2.0,
            None,
            None,
            None,
            None,
            &[7.0, 1.0],
        );
        assert_eq!(on_solid.dash.as_slice(), &[7.0, 1.0]);
        // Empty override → fall back to the style's [3, 2] pattern.
        let fallback = stroke_for(Some("S"), 2.0, None, None, None, Some(&styles), &[]);
        assert_eq!(fallback.dash.as_slice(), &[3.0, 2.0]);
    }

    #[test]
    fn compose_outer_matrix_identity_mpt_is_origin_shift() {
        // Identity MasterPageTransform: outer collapses to
        // translate(target - master_origin), matching the legacy
        // translation-only stamp. master_origin (10,20), target (100,50).
        let outer = Transform::translate(100.0, 50.0)
            .compose(&Transform::IDENTITY)
            .compose(&Transform::translate(-10.0, -20.0));
        // Master item sitting at inner translate(3, 4).
        let m = compose_outer_matrix(outer, Some([1.0, 0.0, 0.0, 1.0, 3.0, 4.0]));
        assert_eq!(
            [m[0], m[1], m[2], m[3]],
            [1.0, 0.0, 0.0, 1.0],
            "linear part untouched"
        );
        assert!((m[4] - 93.0).abs() < 1e-4, "tx={} (100-10+3)", m[4]);
        assert!((m[5] - 34.0).abs() < 1e-4, "ty={} (50-20+4)", m[5]);
    }

    #[test]
    fn compose_outer_matrix_applies_mpt_scale() {
        // A 2× MasterPageTransform about a master origin at (0,0) scales
        // the stamped item's linear part *and* its offset — the part the
        // old translation-only stamp silently dropped.
        let outer = Transform::translate(0.0, 0.0)
            .compose(&Transform([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]))
            .compose(&Transform::translate(0.0, 0.0));
        let m = compose_outer_matrix(outer, Some([1.0, 0.0, 0.0, 1.0, 5.0, 7.0]));
        assert!(
            (m[0] - 2.0).abs() < 1e-4 && (m[3] - 2.0).abs() < 1e-4,
            "linear scaled"
        );
        assert!((m[4] - 10.0).abs() < 1e-4, "tx={} (5×2)", m[4]);
        assert!((m[5] - 14.0).abs() < 1e-4, "ty={} (7×2)", m[5]);
    }

    // ---- W1.9 spread-level ItemTransform rotation/scale ----

    #[test]
    fn spread_linear_transform_identity_and_translation_collapse() {
        // Absent → identity.
        assert_eq!(spread_linear_transform(None), Transform::IDENTITY);
        // Pure translation → identity (translation cancels against the
        // spread origin, so only the linear part rides the field).
        assert_eq!(
            spread_linear_transform(Some([1.0, 0.0, 0.0, 1.0, 40.0, -12.0])),
            Transform::IDENTITY
        );
    }

    #[test]
    fn spread_linear_transform_keeps_rotation_drops_translation() {
        // 90° CW rotation (a=0,b=1,c=-1,d=0) with a translation → the
        // linear block is kept, the translation dropped.
        let lin = spread_linear_transform(Some([0.0, 1.0, -1.0, 0.0, 100.0, 200.0]));
        assert_eq!(lin.0, [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn frame_outer_transform_identity_spread_is_unchanged() {
        // With an identity spread_transform the outer is byte-identical
        // to the historical translate(-origin) ∘ item_transform.
        let page = BuiltPage {
            id: PageId::synthetic(0, 0),
            width_pt: 100.0,
            height_pt: 100.0,
            spread_origin: (10.0, 20.0),
            spread_transform: Transform::IDENTITY,
            list: DisplayList::new(),
            layout_generation: 0,
            numbering_generation: 0,
            stats: PipelineStats::default(),
            story_layout: Vec::new(),
            footnotes: Vec::new(),
            diagnostics: Vec::new(),
            cell_rects: Vec::new(),
        };
        let outer = frame_outer_transform(&page, Some([1.0, 0.0, 0.0, 1.0, 5.0, 6.0]));
        // translate(-10,-20) ∘ translate(5,6) = translate(-5,-14).
        assert_eq!(outer.0, [1.0, 0.0, 0.0, 1.0, -5.0, -14.0]);
    }

    #[test]
    fn frame_outer_transform_rotated_spread_rotates_about_page_origin() {
        // 90° CW spread rotation. A point at the page origin (spread
        // origin) stays put; a frame offset from the origin rotates
        // about it. spread_origin=(0,0) keeps the math clean.
        let page = BuiltPage {
            id: PageId::synthetic(0, 0),
            width_pt: 100.0,
            height_pt: 100.0,
            spread_origin: (0.0, 0.0),
            spread_transform: Transform([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]),
            list: DisplayList::new(),
            layout_generation: 0,
            numbering_generation: 0,
            stats: PipelineStats::default(),
            story_layout: Vec::new(),
            footnotes: Vec::new(),
            diagnostics: Vec::new(),
            cell_rects: Vec::new(),
        };
        // Frame at inner origin translated to (30, 0). Under 90° CW
        // (x' = -y, y' = x), the frame's translation (30,0) maps to
        // (0, 30). outer = spread ∘ translate(0,0) ∘ translate(30,0).
        let outer = frame_outer_transform(&page, Some([1.0, 0.0, 0.0, 1.0, 30.0, 0.0]));
        let (x, y) = outer.apply(0.0, 0.0);
        assert!((x - 0.0).abs() < 1e-4, "x={x}");
        assert!((y - 30.0).abs() < 1e-4, "y={y}");
        // The linear block is the spread rotation composed with identity.
        assert!((outer.0[0]).abs() < 1e-4 && (outer.0[1] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn spread_transform_inverse_round_trips() {
        // The hit-tester inverts the same spread_transform the renderer
        // applied; a 90° rotation + 2× scale must round-trip.
        let s = Transform([0.0, 2.0, -2.0, 0.0, 0.0, 0.0]);
        let inv = s.inverse().expect("invertible");
        let p = (12.0, -5.0);
        let (fx, fy) = s.apply(p.0, p.1);
        let (bx, by) = inv.apply(fx, fy);
        assert!(
            (bx - p.0).abs() < 1e-4 && (by - p.1).abs() < 1e-4,
            "({bx},{by})"
        );
    }

    fn attrs(
        list_type: Option<&str>,
        ch: Option<u32>,
        after: Option<&str>,
    ) -> paged_scene::ResolvedParagraphAttrs {
        paged_scene::ResolvedParagraphAttrs {
            bullets_list_type: list_type.map(str::to_string),
            bullet_character: ch,
            bullets_text_after: after.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn list_prefix_builds_bullet_plus_separator() {
        let mut counter = 0;
        let mut prev_numbered = false;
        let p = list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
            &mut counter,
            &mut prev_numbered,
            None,
        )
        .unwrap();
        assert_eq!(p, "\u{2022} ");
        assert!(!prev_numbered, "BulletList clears prev_numbered");
    }

    #[test]
    fn list_prefix_expands_caret_t_to_tab() {
        let mut counter = 0;
        let mut prev_numbered = false;
        let p = list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some("^t")),
            &mut counter,
            &mut prev_numbered,
            None,
        )
        .unwrap();
        assert_eq!(p, "\u{2022}\t");
    }

    #[test]
    fn list_prefix_none_for_nolist_clears_prev_numbered() {
        let mut counter = 5;
        let mut prev_numbered = true;
        assert!(list_prefix(
            &attrs(Some("NoList"), None, None),
            &mut counter,
            &mut prev_numbered,
            None
        )
        .is_none());
        // NoList shouldn't damage a sticky counter — a follow-on
        // NumberedList with `NumberingContinue` may resume.
        assert_eq!(counter, 5);
        assert!(!prev_numbered);
    }

    #[test]
    fn list_prefix_numbered_increments_across_paragraphs() {
        let mut counter = 0;
        let mut prev_numbered = false;
        let attrs = attrs(Some("NumberedList"), None, None);
        // Default expression `^#.^t` ⇒ "<n>.\t".
        assert_eq!(
            list_prefix(&attrs, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("1.\t")
        );
        assert_eq!(
            list_prefix(&attrs, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("2.\t")
        );
        assert_eq!(
            list_prefix(&attrs, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("3.\t")
        );
        assert_eq!(counter, 3);
        assert!(prev_numbered);
    }

    #[test]
    fn list_prefix_numbered_resets_after_non_numbered() {
        let mut counter = 0;
        let mut prev_numbered = false;
        let n = attrs(Some("NumberedList"), None, None);
        let none = attrs(None, None, None);
        list_prefix(&n, &mut counter, &mut prev_numbered, None); // 1.
        list_prefix(&n, &mut counter, &mut prev_numbered, None); // 2.
        list_prefix(&none, &mut counter, &mut prev_numbered, None); // clears prev_numbered, counter sticky
        assert!(!prev_numbered);
        assert_eq!(
            list_prefix(&n, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("1.\t"),
            "default behaviour: counter resets when prev wasn't numbered"
        );
    }

    #[test]
    fn list_prefix_bullet_to_numbered_resets() {
        // Mixing list types in a row resets by default — each
        // list_type change starts a fresh sequence unless
        // NumberingContinue is set.
        let mut counter = 0;
        let mut prev_numbered = false;
        list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
            &mut counter,
            &mut prev_numbered,
            None,
        );
        assert!(!prev_numbered);
        let n = attrs(Some("NumberedList"), None, None);
        assert_eq!(
            list_prefix(&n, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("1.\t")
        );
    }

    #[test]
    fn list_prefix_bullet_falls_back_to_default_when_codepoint_missing() {
        // BulletList without an explicit BulletChar still emits the
        // U+2022 default — matches InDesign's behaviour and lets
        // real-export IDMLs render visible bullets.
        let mut counter = 0;
        let mut prev_numbered = false;
        let prefix = list_prefix(
            &attrs(Some("BulletList"), None, Some(" ")),
            &mut counter,
            &mut prev_numbered,
            None,
        );
        assert_eq!(prefix.as_deref(), Some("\u{2022} "));
    }

    #[test]
    fn list_prefix_numbering_start_at_jumps_counter() {
        // StartAt = 5 ⇒ first emission is "5.\t", then 6, 7, ...
        let mut counter = 0;
        let mut prev_numbered = false;
        let mut a = attrs(Some("NumberedList"), None, None);
        a.numbering_start_at = Some(5);
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("5.\t")
        );
        // StartAt only fires on paragraph entry; once it's been
        // applied, drop it for the next paragraph.
        a.numbering_start_at = None;
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("6.\t")
        );
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("7.\t")
        );
    }

    #[test]
    fn list_prefix_numbering_start_at_mid_list_resets() {
        // After a few numbered paragraphs, a paragraph with
        // NumberingStartAt = 10 forces the counter to that value.
        let mut counter = 0;
        let mut prev_numbered = false;
        let plain = attrs(Some("NumberedList"), None, None);
        list_prefix(&plain, &mut counter, &mut prev_numbered, None); // 1.
        list_prefix(&plain, &mut counter, &mut prev_numbered, None); // 2.
        let mut jumped = attrs(Some("NumberedList"), None, None);
        jumped.numbering_start_at = Some(10);
        assert_eq!(
            list_prefix(&jumped, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("10.\t")
        );
        // Subsequent plain paragraphs continue off the jump.
        assert_eq!(
            list_prefix(&plain, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("11.\t")
        );
    }

    #[test]
    fn list_prefix_numbering_continue_persists_across_style_boundary() {
        // Numbered → BulletList → Numbered with `NumberingContinue`
        // resumes the count off the prior numbered run instead of
        // resetting to 1.
        let mut counter = 0;
        let mut prev_numbered = false;
        let plain = attrs(Some("NumberedList"), None, None);
        list_prefix(&plain, &mut counter, &mut prev_numbered, None); // 1.
        list_prefix(&plain, &mut counter, &mut prev_numbered, None); // 2.
        list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
            &mut counter,
            &mut prev_numbered,
            None,
        );
        let mut cont = attrs(Some("NumberedList"), None, None);
        cont.numbering_continue = Some(true);
        assert_eq!(
            list_prefix(&cont, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("3.\t"),
            "NumberingContinue suppresses the implicit reset"
        );
        // Compare against the default-reset path: without Continue,
        // the same scenario would have restarted at 1.
        let mut counter2 = 0;
        let mut prev2 = false;
        list_prefix(&plain, &mut counter2, &mut prev2, None); // 1.
        list_prefix(&plain, &mut counter2, &mut prev2, None); // 2.
        list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
            &mut counter2,
            &mut prev2,
            None,
        );
        assert_eq!(
            list_prefix(&plain, &mut counter2, &mut prev2, None).as_deref(),
            Some("1.\t"),
            "without NumberingContinue the count resets"
        );
    }

    #[test]
    fn list_prefix_uses_custom_numbering_expression() {
        // `Step ^# of 5^t` ⇒ "Step 1 of 5\t", "Step 2 of 5\t", ...
        let mut counter = 0;
        let mut prev_numbered = false;
        let mut a = attrs(Some("NumberedList"), None, None);
        a.numbering_expression = Some("Step ^# of 5^t".to_string());
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("Step 1 of 5\t")
        );
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("Step 2 of 5\t")
        );
    }

    #[test]
    fn list_prefix_cross_story_seed_continues_and_suppresses_reset() {
        // W1.22 — a ContinueNumbersAcrossStories list. The first
        // numbered paragraph of a fresh story emitter has
        // prev_was_numbered=false (no neighbour), which WITHOUT the
        // seed would reset to "1". With `cross_story_seed = Some(2)`
        // (the ledger's last value) it must continue at "3".
        let n = attrs(Some("NumberedList"), None, None);
        let mut counter = 0; // fresh per-story emitter
        let mut prev_numbered = false; // story start
        assert_eq!(
            list_prefix(&n, &mut counter, &mut prev_numbered, Some(2)).as_deref(),
            Some("3.\t"),
            "cross-story seed of 2 must continue at 3, not reset to 1",
        );
        assert_eq!(counter, 3, "counter advances off the seed");
        // NumberingStartAt still wins over the seed (explicit restart).
        let mut started = attrs(Some("NumberedList"), None, None);
        started.numbering_start_at = Some(10);
        let mut counter2 = 0;
        let mut prev2 = false;
        assert_eq!(
            list_prefix(&started, &mut counter2, &mut prev2, Some(5)).as_deref(),
            Some("10.\t"),
            "explicit NumberingStartAt overrides the cross-story seed",
        );
    }

    #[test]
    fn substitute_numbering_expression_passes_literals_and_decodes_caret_escape() {
        // `^^` decodes to a literal caret; unknown `^x` sequences
        // pass through verbatim (no surprise glyph loss).
        assert_eq!(substitute_numbering_expression("^^#^t", "1"), "^#\t");
        assert_eq!(substitute_numbering_expression("(^#)^t", "42"), "(42)\t");
        assert_eq!(substitute_numbering_expression("^?", "1"), "^?");
        // Trailing lone caret passes through.
        assert_eq!(substitute_numbering_expression("^# ^", "5"), "5 ^");
    }

    #[test]
    fn format_number_arabic_default() {
        assert_eq!(format_number(1, None), "1");
        assert_eq!(format_number(42, None), "42");
        assert_eq!(format_number(7, Some("1, 2, 3, 4...")), "7");
    }

    #[test]
    fn format_number_zero_padded() {
        assert_eq!(format_number(1, Some("01, 02, 03, 04...")), "01");
        assert_eq!(format_number(42, Some("01, 02, 03...")), "42");
        assert_eq!(format_number(7, Some("001, 002, 003...")), "007");
    }

    #[test]
    fn format_number_roman_upper_lower() {
        assert_eq!(format_number(1, Some("I, II, III, IV...")), "I");
        assert_eq!(format_number(4, Some("I, II, III, IV...")), "IV");
        assert_eq!(format_number(9, Some("I, II, III...")), "IX");
        assert_eq!(format_number(40, Some("I, II, III...")), "XL");
        assert_eq!(format_number(1994, Some("I, II, III...")), "MCMXCIV");
        assert_eq!(format_number(4, Some("i, ii, iii, iv...")), "iv");
    }

    #[test]
    fn format_number_alpha_upper_lower() {
        assert_eq!(format_number(1, Some("A, B, C, D...")), "A");
        assert_eq!(format_number(26, Some("A, B, C...")), "Z");
        assert_eq!(format_number(27, Some("A, B, C...")), "AA");
        assert_eq!(format_number(28, Some("A, B, C...")), "AB");
        assert_eq!(format_number(703, Some("A, B, C...")), "AAA");
        assert_eq!(format_number(2, Some("a, b, c...")), "b");
    }

    #[test]
    fn format_number_unknown_falls_back_to_arabic() {
        assert_eq!(format_number(5, Some("Q, R, S, ...")), "5");
        assert_eq!(format_number(5, Some("not a format")), "5");
    }

    #[test]
    fn format_number_hanzi_everyday() {
        let f = |n| format_number(n, Some("一, 二, 三..."));
        assert_eq!(f(1), "一");
        assert_eq!(f(5), "五");
        assert_eq!(f(9), "九");
        // 10..=19: leading 十 without 一 prefix.
        assert_eq!(f(10), "十");
        assert_eq!(f(11), "十一");
        assert_eq!(f(15), "十五");
        // 20..=99: digit + 十 + units.
        assert_eq!(f(20), "二十");
        assert_eq!(f(25), "二十五");
        assert_eq!(f(99), "九十九");
        // 100..=999: hundreds digit + 百 + tens + units.
        assert_eq!(f(100), "一百");
        // 零 gap-marker when tens=0 but units>0 (e.g. 101 = 一百零一).
        assert_eq!(f(101), "一百零一");
        // 110 = hundreds + 一 + 十 (no 零, tens is non-zero).
        assert_eq!(f(110), "一百一十");
        assert_eq!(f(125), "一百二十五");
        assert_eq!(f(999), "九百九十九");
        // ≥ 1000: fallback to Arabic.
        assert_eq!(f(1000), "1000");
    }

    #[test]
    fn format_number_hanzi_formal() {
        let f = |n| format_number(n, Some("壹, 貳, 參..."));
        // Formal/financial digit set.
        assert_eq!(f(1), "壹");
        assert_eq!(f(5), "伍");
        assert_eq!(f(9), "玖");
        // 10..=19: formal keeps 壹十 prefix (unlike everyday).
        assert_eq!(f(11), "壹十壹");
    }

    #[test]
    fn list_prefix_uses_numbering_format() {
        let mut counter = 0;
        let mut prev_numbered = false;
        let mut a = attrs(Some("NumberedList"), None, None);
        a.numbering_format = Some("I, II, III, IV...".to_string());
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("I.\t")
        );
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("II.\t")
        );
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered, None).as_deref(),
            Some("III.\t")
        );
    }

    fn approx(a: (f32, f32), b: (f32, f32)) {
        let eps = 1e-5;
        assert!(
            (a.0 - b.0).abs() < eps && (a.1 - b.1).abs() < eps,
            "expected {b:?}, got {a:?}",
        );
    }

    #[test]
    fn gradient_endpoints_zero_degrees_horizontal() {
        // 0° = horizontal left → right (IDML's default direction).
        let (s, e) = linear_gradient_endpoints(None, None, None);
        approx(s, (0.0, 0.5));
        approx(e, (1.0, 0.5));
        // Some(0.0) must match None — both are the spec default.
        let (s, e) = linear_gradient_endpoints(Some(0.0), None, None);
        approx(s, (0.0, 0.5));
        approx(e, (1.0, 0.5));
    }

    #[test]
    fn gradient_endpoints_ninety_degrees_vertical() {
        // Regression for the fill-side default that used to be
        // hardcoded `(0,0)→(0,1)` (top→bottom). 90° must keep that
        // orientation: in IDML's y-down convention the +y axis points
        // down the page, so 90° rotates the gradient line vertically.
        let (s, e) = linear_gradient_endpoints(Some(90.0), None, None);
        approx(s, (0.5, 0.0));
        approx(e, (0.5, 1.0));
    }

    #[test]
    fn gradient_endpoints_forty_five_degrees() {
        // 45° at default length: half-vector magnitude = 0.5 along the
        // unit vector `(cos 45°, sin 45°)`. Endpoints sit inside the
        // unit rect (the half-distance projects shorter than the
        // diagonal); that matches the existing fill-default behaviour.
        let (s, e) = linear_gradient_endpoints(Some(45.0), None, None);
        let r = std::f32::consts::FRAC_1_SQRT_2 * 0.5;
        approx(s, (0.5 - r, 0.5 - r));
        approx(e, (0.5 + r, 0.5 + r));
    }

    #[test]
    fn gradient_endpoints_negative_angle_matches_supplement() {
        // -45° (= 315°) reflects the 45° endpoints across the
        // horizontal axis. cos is symmetric, sin flips sign.
        let (s_neg, e_neg) = linear_gradient_endpoints(Some(-45.0), None, None);
        let (s_pos, e_pos) = linear_gradient_endpoints(Some(315.0), None, None);
        approx(s_neg, s_pos);
        approx(e_neg, e_pos);
        let r = std::f32::consts::FRAC_1_SQRT_2 * 0.5;
        approx(s_neg, (0.5 - r, 0.5 + r));
        approx(e_neg, (0.5 + r, 0.5 - r));
    }

    #[test]
    fn gradient_endpoints_explicit_length_compresses_line() {
        // GradientFillLength in pt converts to unit-rect half-vector
        // `(cos θ · L / (2·w), sin θ · L / (2·h))`. For a 200×100 rect
        // at 0° with L = 100pt the half-vec is `(0.25, 0)` so endpoints
        // hug the rect centre instead of running edge-to-edge.
        let (s, e) = linear_gradient_endpoints(Some(0.0), Some(100.0), Some((200.0, 100.0)));
        approx(s, (0.25, 0.5));
        approx(e, (0.75, 0.5));
        // 90° on the same rect with L=100 → half-vec `(0, 0.5)` so the
        // gradient line still spans edge-to-edge along the short axis.
        let (s, e) = linear_gradient_endpoints(Some(90.0), Some(100.0), Some((200.0, 100.0)));
        approx(s, (0.5, 0.0));
        approx(e, (0.5, 1.0));
    }

    #[test]
    fn gradient_endpoints_length_without_dims_falls_through_to_default() {
        // Without bbox dimensions we can't convert pt to unit-rect
        // coords; helper falls back to the unit-vector default so
        // callers that lack geometry (e.g. legacy text-frame strokes
        // that don't track a bbox) still produce a sensible line.
        let (s, e) = linear_gradient_endpoints(Some(0.0), Some(100.0), None);
        approx(s, (0.0, 0.5));
        approx(e, (1.0, 0.5));
    }

    fn anchor_at(x: f32, y: f32) -> paged_parse::PathAnchor {
        paged_parse::PathAnchor {
            anchor: (x, y),
            left: (x, y),
            right: (x, y),
        }
    }

    /// `polygon_path_from_anchors` collapses to a single MoveTo/Close
    /// when given no subpath markers — the legacy serialisation that
    /// every InDesign-export polygon uses.
    #[test]
    fn polygon_path_from_anchors_single_contour_emits_one_subpath() {
        let anchors = vec![
            anchor_at(0.0, 0.0),
            anchor_at(10.0, 0.0),
            anchor_at(10.0, 10.0),
            anchor_at(0.0, 10.0),
        ];
        let path = polygon_path_from_anchors(&anchors, &[]);
        let move_count = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::MoveTo { .. }))
            .count();
        let close_count = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::Close))
            .count();
        assert_eq!(move_count, 1, "legacy single-contour input → one MoveTo");
        assert_eq!(close_count, 1, "legacy single-contour input → one Close");
    }

    /// Compound-path input (square with hole — two `<GeometryPathType>`
    /// contours in the source IDML) emits one MoveTo/Close per
    /// contour. Without this, the renderer would draw a stray segment
    /// from the outer contour's last anchor to the inner contour's
    /// first anchor and silently mis-render the hole as a triangle
    /// notch in the outer outline.
    #[test]
    fn polygon_path_from_anchors_compound_emits_one_subpath_per_contour() {
        let anchors = vec![
            // outer
            anchor_at(0.0, 0.0),
            anchor_at(200.0, 0.0),
            anchor_at(200.0, 200.0),
            anchor_at(0.0, 200.0),
            // inner
            anchor_at(60.0, 60.0),
            anchor_at(60.0, 140.0),
            anchor_at(140.0, 140.0),
            anchor_at(140.0, 60.0),
        ];
        let subpath_starts = vec![0, 4];
        let path = polygon_path_from_anchors(&anchors, &subpath_starts);
        let moves: Vec<&PathSegment> = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::MoveTo { .. }))
            .collect();
        let closes = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::Close))
            .count();
        assert_eq!(moves.len(), 2, "two contours → two MoveTo segments");
        assert_eq!(closes, 2, "two contours → two Close segments");
        // The two MoveTos should land on the first anchor of each
        // contour — guards against a silent off-by-one in the range
        // construction that would otherwise still emit two contours
        // but join them at the wrong points.
        match moves[0] {
            PathSegment::MoveTo { x, y } => {
                assert!((*x - 0.0).abs() < 1e-6 && (*y - 0.0).abs() < 1e-6)
            }
            _ => unreachable!(),
        }
        match moves[1] {
            PathSegment::MoveTo { x, y } => {
                assert!((*x - 60.0).abs() < 1e-6 && (*y - 60.0).abs() < 1e-6)
            }
            _ => unreachable!(),
        }
    }

    /// Defensive: subpath markers that point past the end of the
    /// anchor list, or that duplicate the implicit "starts at 0"
    /// boundary, must not produce empty contours or panic.
    #[test]
    fn polygon_path_from_anchors_filters_bogus_markers() {
        let anchors = vec![
            anchor_at(0.0, 0.0),
            anchor_at(10.0, 0.0),
            anchor_at(10.0, 10.0),
        ];
        let path = polygon_path_from_anchors(&anchors, &[0, 99, 0]);
        let moves = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::MoveTo { .. }))
            .count();
        let closes = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::Close))
            .count();
        assert_eq!(
            moves, 1,
            "out-of-range / duplicate markers collapse to one contour"
        );
        assert_eq!(closes, 1);
    }

    /// P-15: open contours skip the closing CubicTo + Close so a
    /// `<GeometryPathType PathOpen="true">` polygon doesn't get
    /// auto-filled.
    #[test]
    fn polygon_path_from_anchors_with_open_skips_close_for_open_contour() {
        let anchors = vec![
            anchor_at(0.0, 0.0),
            anchor_at(40.0, 0.0),
            anchor_at(20.0, 40.0),
        ];
        let path = polygon_path_from_anchors_with_open(&anchors, &[], &[true]);
        let moves = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::MoveTo { .. }))
            .count();
        let closes = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::Close))
            .count();
        let cubics = path
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::CubicTo { .. }))
            .count();
        assert_eq!(moves, 1, "single contour → one MoveTo");
        assert_eq!(closes, 0, "open contour skips the Close");
        // 3 anchors → 2 inter-anchor CubicTos; the closing back-to-first
        // cubic must NOT fire (so 2, not 3).
        assert_eq!(cubics, 2, "open contour skips the closing CubicTo");
    }

    /// W1.1: when an anchor's Bezier handles coincide with its anchor
    /// point — the IDML serialisation for a straight corner — the
    /// emitted CubicTo's control points land exactly on the segment's
    /// endpoints, so the cubic reduces to a straight line in both
    /// rasterizers (tiny-skia / Vello flatten a degenerate cubic to a
    /// LineTo). Locks the "handle == anchor ⇒ line" contract.
    #[test]
    fn polygon_path_from_anchors_straight_segment_cubic_collapses_to_line() {
        let anchors = vec![anchor_at(0.0, 0.0), anchor_at(10.0, 0.0)];
        let path = polygon_path_from_anchors(&anchors, &[]);
        // First CubicTo is the forward segment 0→1; its controls must be
        // the two anchors (no bow).
        let cubic = path
            .segments
            .iter()
            .find_map(|s| match s {
                PathSegment::CubicTo {
                    cx1,
                    cy1,
                    cx2,
                    cy2,
                    x,
                    y,
                } => Some((*cx1, *cy1, *cx2, *cy2, *x, *y)),
                _ => None,
            })
            .expect("a forward CubicTo is emitted between the two anchors");
        let (cx1, cy1, cx2, cy2, x, y) = cubic;
        // cx1/cy1 == from.right == anchor 0; cx2/cy2 == to.left == anchor 1.
        assert!((cx1 - 0.0).abs() < 1e-6 && (cy1 - 0.0).abs() < 1e-6);
        assert!((cx2 - 10.0).abs() < 1e-6 && (cy2 - 0.0).abs() < 1e-6);
        assert!((x - 10.0).abs() < 1e-6 && (y - 0.0).abs() < 1e-6);
    }

    fn font_table_with(
        cache: &[(&str, Option<&str>, &[u8])],
        fallback: Option<&[u8]>,
    ) -> FontTable {
        let mut hm: HashMap<(String, Option<String>), Bytes> = HashMap::new();
        for (family, style, b) in cache {
            hm.insert(
                (family.to_string(), style.map(str::to_string)),
                Bytes::copy_from_slice(b),
            );
        }
        FontTable {
            faces: HashMap::new(),
            face_bytes: HashMap::new(),
            cache: hm,
            fallback: fallback.map(Bytes::copy_from_slice),
            metrics: HashMap::new(),
            family_metrics: HashMap::new(),
        }
    }

    fn run_attrs(family: Option<&str>, style: Option<&str>) -> paged_scene::ResolvedRunAttrs {
        paged_scene::ResolvedRunAttrs {
            font: family.map(str::to_string),
            font_style: style.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_paragraph_bytes_falls_back_per_run_to_sibling_font() {
        // Mixed paragraph: one run references a registered family,
        // another references something the cache doesn't know AND no
        // document-wide fallback is configured. The unknown run
        // inherits the resolved sibling's bytes instead of dropping
        // the whole paragraph.
        let table = font_table_with(&[("Inter", None, b"INTER")], None);
        let runs = vec![
            run_attrs(Some("Inter"), None),
            run_attrs(Some("Limon Script"), None),
            run_attrs(Some("Inter"), None),
        ];
        let pool = table
            .resolve_paragraph_bytes(&runs)
            .expect("paragraph kept");
        assert_eq!(pool.len(), 3);
        assert_eq!(&pool[0][..], b"INTER");
        assert_eq!(&pool[1][..], b"INTER", "missing run inherits sibling");
        assert_eq!(&pool[2][..], b"INTER");
    }

    #[test]
    fn resolve_paragraph_bytes_prefers_table_fallback_when_no_run_resolves() {
        // All runs reference unknown families but the renderer was
        // given a document-wide default font — every slot picks it up.
        let table = font_table_with(&[], Some(b"DEFAULT"));
        let runs = vec![
            run_attrs(Some("Unknown A"), None),
            run_attrs(Some("Unknown B"), Some("Bold")),
        ];
        let pool = table
            .resolve_paragraph_bytes(&runs)
            .expect("paragraph kept");
        assert_eq!(pool.len(), 2);
        assert_eq!(&pool[0][..], b"DEFAULT");
        assert_eq!(&pool[1][..], b"DEFAULT");
    }

    #[test]
    fn resolve_paragraph_bytes_returns_none_when_nothing_resolves() {
        // No registered family, no fallback — caller still has to
        // skip the paragraph because there's literally no shaping
        // input.
        let table = font_table_with(&[], None);
        let runs = vec![run_attrs(Some("Unknown"), None)];
        assert!(table.resolve_paragraph_bytes(&runs).is_none());
    }

    // P-22: lock the stroke-alignment inset math. `tiny_skia` strokes
    // centered on the path, so Inside alignment needs the path inset
    // by +stroke/2 inward (i.e. shrink the rect), Outside by
    // -stroke/2 (grow the rect), Center / None ⇒ 0. Regressions in
    // this math show up as ½-px nudges on line-art-dense pages.
    #[test]
    fn stroke_alignment_offset_inside_returns_positive_half_weight() {
        assert!((stroke_alignment_offset(Some("InsideAlignment"), 2.0) - 1.0).abs() < 1e-6);
        assert!((stroke_alignment_offset(Some("InsideAlignment"), 0.5) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn stroke_alignment_offset_outside_returns_negative_half_weight() {
        assert!((stroke_alignment_offset(Some("OutsideAlignment"), 2.0) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn stroke_alignment_offset_center_and_none_return_zero() {
        assert_eq!(stroke_alignment_offset(Some("CenterAlignment"), 2.0), 0.0);
        assert_eq!(stroke_alignment_offset(None, 2.0), 0.0);
    }

    // P-25 regression: a paragraph ending with a trailing `\n` (the
    // `<Br/>` after the final visible content) must NOT produce a
    // phantom empty sub-paragraph. A NumberedList paragraph would
    // otherwise increment its counter twice and emit two "01" /
    // "02" markers per visible line.
    #[test]
    fn split_paragraph_at_breaks_drops_trailing_newline_only_sub_paragraph() {
        let run = paged_parse::CharacterRun {
            text: "01\n".to_string(),
            ..paged_parse::CharacterRun::default()
        };
        let paragraph = paged_parse::Paragraph {
            runs: vec![run],
            ..paged_parse::Paragraph::default()
        };
        let subs = split_paragraph_at_breaks(&paragraph);
        assert_eq!(
            subs.len(),
            1,
            "trailing \\n must not produce a phantom sub-paragraph"
        );
        assert_eq!(subs[0].runs.len(), 1);
        assert_eq!(subs[0].runs[0].text, "01");
    }

    // Belt + braces: pathological case where the splitter's hint
    // path seeds an all-`\n` trailing run. The post-loop guard at
    // the tail of `split_paragraph_at_breaks` must collapse it.
    #[test]
    fn split_paragraph_at_breaks_drops_trailing_all_newline_run_after_visible() {
        let visible = paged_parse::CharacterRun {
            text: "01".to_string(),
            ..paged_parse::CharacterRun::default()
        };
        let nl_only = paged_parse::CharacterRun {
            text: "\n\n".to_string(),
            ..paged_parse::CharacterRun::default()
        };
        let paragraph = paged_parse::Paragraph {
            runs: vec![visible, nl_only],
            ..paged_parse::Paragraph::default()
        };
        let subs = split_paragraph_at_breaks(&paragraph);
        // Two `\n` after visible content used to seed two empty
        // hint-only subs after the "01" one (= 3 total). The guard
        // collapses the trailing newline-only subs so a numbered
        // list emits its marker once, not three times.
        assert_eq!(
            subs.len(),
            1,
            "trailing-only-newline tail subs must collapse"
        );
        assert_eq!(subs[0].runs.len(), 1);
        assert_eq!(subs[0].runs[0].text, "01");
    }

    // Composed: inset_rect applied at the stroke offset must shrink
    // (Inside) or grow (Outside) the rect by exactly the stroke width
    // along each axis. A 100×100 rect with a 2-pt Inside stroke ends
    // up 98×98, drawn so the centered stroke lands fully inside.
    #[test]
    fn stroke_alignment_inside_shrinks_rect_by_stroke_width() {
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let off = stroke_alignment_offset(Some("InsideAlignment"), 2.0);
        let inset = inset_rect(r, off);
        assert!((inset.x - 1.0).abs() < 1e-6);
        assert!((inset.y - 1.0).abs() < 1e-6);
        assert!((inset.w - 98.0).abs() < 1e-6);
        assert!((inset.h - 98.0).abs() < 1e-6);
    }

    #[test]
    // Deliberately asserts on source constants: this test pins the
    // placeholder calibration values so an accidental edit trips CI.
    #[allow(clippy::assertions_on_constants)]
    fn q22_missing_image_placeholder_calibration_pinned() {
        assert!(
            (PLACEHOLDER_FILL_RGB - 0.5).abs() < 1e-6,
            "placeholder fill should target ~50% grey",
        );
        assert!(
            (PLACEHOLDER_X_STROKE_PT - 1.5).abs() < 1e-6,
            "placeholder X stroke should be 1.5pt",
        );
        assert!(
            PLACEHOLDER_X_RGB < 0.05,
            "placeholder X should read as near-black against the grey fill",
        );
    }

    /// Q-08 (hypothesis check, rect / oval path): for a rotated
    /// rect / oval the `linear_gradient_endpoints` projection
    /// (unit-rect coords) is fed through `Transform::for_rect_in(rect,
    /// outer)` where `outer` already incorporates the shape's
    /// `ItemTransform`. The composed transform IS what the rasterizer
    /// uses to push the unit-rect endpoints into page space (see
    /// `paged_gpu::cpu::build_linear_gradient_shader`), so a 90°-
    /// vertical gradient on a 90°-rotated frame should produce a
    /// horizontal page-space gradient line. Asserts that — guards
    /// against a regression that would re-introduce the protocol's
    /// hypothesised bug (ItemTransform ignored on gradient projection).
    #[test]
    fn q08_gradient_endpoints_rotate_with_item_transform() {
        let rect = paged_compose::Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let (s_unit, e_unit) = linear_gradient_endpoints(Some(90.0), None, None);
        approx(s_unit, (0.5, 0.0));
        approx(e_unit, (0.5, 1.0));
        // Identity baseline: local vertical = page vertical.
        let xf_id = Transform::for_rect_in(rect, Transform::IDENTITY);
        approx(xf_id.apply(s_unit.0, s_unit.1), (50.0, 0.0));
        approx(xf_id.apply(e_unit.0, e_unit.1), (50.0, 100.0));
        // ItemTransform `0 1 -1 0 200 0` packs to `[a, b, c, d, tx,
        // ty] = [0, 1, -1, 0, 200, 0]` — a 90° rotation about the
        // origin plus translate(+200, 0). Maps frame-local (x, y) to
        // page (200 - y, x).
        let outer_rot = Transform([0.0, 1.0, -1.0, 0.0, 200.0, 0.0]);
        let xf_rot = Transform::for_rect_in(rect, outer_rot);
        approx(xf_rot.apply(s_unit.0, s_unit.1), (200.0, 50.0));
        approx(xf_rot.apply(e_unit.0, e_unit.1), (100.0, 50.0));
    }

    /// Q-08 polygon fix: a Polygon fill emits `FillPath` whose
    /// rasterizer path_transform IS `outer` directly (the path lives
    /// in inner-anchor coords). The fill module rewrites the
    /// gradient's unit-rect endpoints to bbox-local inner coords so
    /// the rasterizer's subsequent `outer.apply(...)` lands them in
    /// the polygon's actual page span. Without that step a 90° fill
    /// on the brochure's full-page background polygon collapses to a
    /// ~1pt gradient line near the spread origin and renders flat.
    /// Asserts the inner-coord math the fill module bakes in.
    #[test]
    fn q08_polygon_gradient_rebases_to_bbox() {
        // Brochure page-bg polygon dimensions (approx).
        let bbox = paged_compose::Rect {
            x: -8.5,
            y: -479.0,
            w: 612.3,
            h: 672.4,
        };
        let (s_unit, e_unit) =
            linear_gradient_endpoints(Some(90.0), Some(577.7332), Some((bbox.w, bbox.h)));
        // `rebase_gradient_to_bbox` applies this mapping.
        let start = (bbox.x + s_unit.0 * bbox.w, bbox.y + s_unit.1 * bbox.h);
        let end = (bbox.x + e_unit.0 * bbox.w, bbox.y + e_unit.1 * bbox.h);
        // Vertical line, horizontally centred on the bbox; length
        // equals the input `length_pt`. Without the rebase the
        // rasterizer would see (0.5, ~0.07) → (0.5, ~0.93) directly
        // (sub-pt line near the spread origin → flat polygon).
        let cx = bbox.x + bbox.w * 0.5;
        assert!((start.0 - cx).abs() < 1e-3);
        assert!((end.0 - cx).abs() < 1e-3);
        assert!(((end.1 - start.1) - 577.7332).abs() < 1e-3);
    }

    /// Track 1a: oversized JPEGs go through `jpeg-decoder`'s
    /// DCT-scaling path instead of materialising the full RGBA8
    /// buffer via `image::load_from_memory`. Annual-report-template's
    /// 5760×9000 cover would otherwise allocate ~198MB in one shot;
    /// here we use a 4000×4000 synthetic JPEG with a 1024px cap and
    /// assert the result lands at the largest DCT scale that still
    /// fits the cap (1/4 → 1000×1000).
    #[test]
    fn track_1a_oversized_jpeg_routes_through_streaming_decoder() {
        use image::{ImageBuffer, ImageFormat, Rgb};
        use std::io::Cursor;
        let src: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(4000, 4000, |x, y| {
            Rgb([(x & 0xFF) as u8, (y & 0xFF) as u8, ((x ^ y) & 0xFF) as u8])
        });
        let mut buf: Vec<u8> = Vec::new();
        src.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
            .expect("encode JPEG");

        let decoded =
            decode_image_bytes_with_target_max(&buf, 1024).expect("streaming JPEG decode");
        // 4000 * 2/8 = 1000 ≤ 1024 fits; 4000 * 3/8 = 1500 doesn't.
        assert_eq!(decoded.width, 1000);
        assert_eq!(decoded.height, 1000);
        assert_eq!(
            decoded.rgba.len(),
            (decoded.width as usize) * (decoded.height as usize) * 4
        );
        // Alpha channel filled to opaque — JPEGs carry no alpha.
        assert!(decoded.rgba.chunks_exact(4).all(|p| p[3] == 255));
    }

    // Track 1a: small JPEGs (longest edge ≤ cap) skip the streaming
    // ── Phase 4 typography — nested-style overlay walker ──────────

    fn ns(
        style: &str,
        delim: paged_parse::NestedDelimiter,
        rep: i32,
        inc: bool,
    ) -> paged_parse::NestedStyle {
        paged_parse::NestedStyle {
            applied_character_style: style.into(),
            delimiter: delim,
            repetition: rep,
            inclusive: inc,
        }
    }

    #[test]
    fn nested_overlay_empty_when_no_entries() {
        assert!(compute_nested_style_overlay("hello world", &[]).is_empty());
    }

    #[test]
    fn nested_overlay_characters_simple() {
        let ov = compute_nested_style_overlay(
            "abcdef",
            &[ns(
                "S/Bold",
                paged_parse::NestedDelimiter::Characters,
                3,
                true,
            )],
        );
        assert_eq!(ov.len(), 1);
        assert_eq!(ov[0].byte_range, 0..3);
        assert_eq!(ov[0].applied_character_style, "S/Bold");
    }

    #[test]
    fn nested_overlay_words_inclusive_captures_trailing_space() {
        // "the quick brown" — Words=1 inclusive should cover "the ".
        let ov = compute_nested_style_overlay(
            "the quick brown",
            &[ns("S/Lead", paged_parse::NestedDelimiter::Words, 1, true)],
        );
        assert_eq!(ov.len(), 1);
        assert_eq!(&"the quick brown"[ov[0].byte_range.clone()], "the ");
    }

    #[test]
    fn nested_overlay_words_exclusive_excludes_space() {
        let ov = compute_nested_style_overlay(
            "the quick brown",
            &[ns("S/Lead", paged_parse::NestedDelimiter::Words, 1, false)],
        );
        assert_eq!(ov.len(), 1);
        assert_eq!(&"the quick brown"[ov[0].byte_range.clone()], "the");
    }

    #[test]
    fn nested_overlay_char_delimiter_until_colon() {
        let ov = compute_nested_style_overlay(
            "Heading: body copy",
            &[ns(
                "S/Bold",
                paged_parse::NestedDelimiter::Char(':'),
                1,
                true,
            )],
        );
        assert_eq!(ov.len(), 1);
        assert_eq!(&"Heading: body copy"[ov[0].byte_range.clone()], "Heading:");
    }

    #[test]
    fn nested_overlay_chained_entries_consume_in_order() {
        // First entry: 3 chars styled S/A. Second entry: 5 chars
        // starting where the first ended.
        let ov = compute_nested_style_overlay(
            "abcdefghijk",
            &[
                ns("S/A", paged_parse::NestedDelimiter::Characters, 3, true),
                ns("S/B", paged_parse::NestedDelimiter::Characters, 5, true),
            ],
        );
        assert_eq!(ov.len(), 2);
        assert_eq!(ov[0].byte_range, 0..3);
        assert_eq!(ov[0].applied_character_style, "S/A");
        assert_eq!(ov[1].byte_range, 3..8);
        assert_eq!(ov[1].applied_character_style, "S/B");
    }

    #[test]
    fn nested_overlay_stops_at_end_of_text() {
        let ov = compute_nested_style_overlay(
            "abc",
            &[ns(
                "S/X",
                paged_parse::NestedDelimiter::Characters,
                100,
                true,
            )],
        );
        // Repetition exceeds text length → range extends to end.
        assert_eq!(ov.len(), 1);
        assert_eq!(ov[0].byte_range, 0..3);
    }

    #[test]
    fn nested_overlay_skips_unknown_delimiter() {
        let ov = compute_nested_style_overlay(
            "hello",
            &[ns("S/X", paged_parse::NestedDelimiter::Unknown, 1, true)],
        );
        // Unknown delimiter yields a zero-length match → no override
        // emitted, no cursor advance.
        assert!(ov.is_empty());
    }

    #[test]
    fn nested_overlay_zero_repetition_is_noop() {
        let ov = compute_nested_style_overlay(
            "hello world",
            &[ns("S/X", paged_parse::NestedDelimiter::Words, 0, true)],
        );
        assert!(ov.is_empty());
    }

    fn mk_run(text: &str, style: Option<&str>) -> paged_parse::CharacterRun {
        paged_parse::CharacterRun {
            character_style: style.map(String::from),
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn split_runs_no_overlay_passes_through() {
        let runs = vec![mk_run("hello", None), mk_run(" world", Some("S/Base"))];
        let out = split_runs_for_nested_styles(&runs, &[]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "hello");
        assert_eq!(out[1].text, " world");
        assert_eq!(out[1].character_style.as_deref(), Some("S/Base"));
    }

    #[test]
    fn split_runs_overlay_inside_single_run_splits_into_three() {
        // Run "the quick brown" (15 bytes). Overlay [4..9) = "quick".
        let runs = vec![mk_run("the quick brown", None)];
        let overlay = vec![NestedStyleApplication {
            byte_range: 4..9,
            applied_character_style: "S/Bold".into(),
        }];
        let out = split_runs_for_nested_styles(&runs, &overlay);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].text, "the ");
        assert_eq!(out[0].character_style, None);
        assert_eq!(out[1].text, "quick");
        assert_eq!(out[1].character_style.as_deref(), Some("S/Bold"));
        assert_eq!(out[2].text, " brown");
        assert_eq!(out[2].character_style, None);
    }

    #[test]
    fn split_runs_overlay_at_run_start_no_pre_fragment() {
        // Run "Heading text", overlay [0..7) = "Heading".
        let runs = vec![mk_run("Heading text", None)];
        let overlay = vec![NestedStyleApplication {
            byte_range: 0..7,
            applied_character_style: "S/H".into(),
        }];
        let out = split_runs_for_nested_styles(&runs, &overlay);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "Heading");
        assert_eq!(out[0].character_style.as_deref(), Some("S/H"));
        assert_eq!(out[1].text, " text");
        assert_eq!(out[1].character_style, None);
    }

    #[test]
    fn split_runs_overlay_spanning_two_runs_splits_both() {
        // Runs: "abc" + "defgh" (paragraph bytes 0..8). Overlay [2..6) =
        // "cdef" — covers tail of run0 and head of run1.
        let runs = vec![mk_run("abc", None), mk_run("defgh", Some("S/Base"))];
        let overlay = vec![NestedStyleApplication {
            byte_range: 2..6,
            applied_character_style: "S/Lead".into(),
        }];
        let out = split_runs_for_nested_styles(&runs, &overlay);
        // Expected fragments: "ab" (no override), "c" (S/Lead from
        // run0), "def" (S/Lead from run1), "gh" (S/Base from run1).
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].text, "ab");
        assert_eq!(out[0].character_style, None);
        assert_eq!(out[1].text, "c");
        assert_eq!(out[1].character_style.as_deref(), Some("S/Lead"));
        assert_eq!(out[2].text, "def");
        assert_eq!(out[2].character_style.as_deref(), Some("S/Lead"));
        assert_eq!(out[3].text, "gh");
        assert_eq!(out[3].character_style.as_deref(), Some("S/Base"));
    }

    // ── Phase 5 — conditional text filter ─────────────────────────

    fn cond_run(text: &str, conditions: &[&str]) -> paged_parse::CharacterRun {
        paged_parse::CharacterRun {
            text: text.into(),
            applied_conditions: conditions.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn cond(id: &str, visible: bool) -> paged_parse::ConditionDef {
        paged_parse::ConditionDef {
            self_id: id.into(),
            visible: Some(visible),
            ..Default::default()
        }
    }

    /// Tiny smoke that mirrors the filter logic inline in
    /// `emit_paragraph_into_chain`. Keeps the test independent of
    /// constructing a full Document, while still exercising the
    /// "all conditions visible ⇒ keep; any invisible ⇒ drop" rule.
    fn filter_by_conditions(
        runs: &[paged_parse::CharacterRun],
        table: &std::collections::BTreeMap<String, paged_parse::ConditionDef>,
    ) -> Vec<paged_parse::CharacterRun> {
        runs.iter()
            .filter(|r| {
                r.applied_conditions
                    .iter()
                    .all(|cid| table.get(cid).and_then(|c| c.visible).unwrap_or(true))
            })
            .cloned()
            .collect()
    }

    #[test]
    fn conditions_no_applied_keeps_run() {
        let runs = vec![cond_run("body", &[])];
        let table = std::collections::BTreeMap::new();
        let out = filter_by_conditions(&runs, &table);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "body");
    }

    #[test]
    fn conditions_visible_keeps_run() {
        let runs = vec![cond_run("draft text", &["Condition/Draft"])];
        let mut table = std::collections::BTreeMap::new();
        table.insert("Condition/Draft".to_string(), cond("Condition/Draft", true));
        let out = filter_by_conditions(&runs, &table);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn conditions_invisible_drops_run() {
        let runs = vec![
            cond_run("keep", &[]),
            cond_run("hide", &["Condition/Draft"]),
        ];
        let mut table = std::collections::BTreeMap::new();
        table.insert(
            "Condition/Draft".to_string(),
            cond("Condition/Draft", false),
        );
        let out = filter_by_conditions(&runs, &table);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "keep");
    }

    #[test]
    fn conditions_multiple_all_must_be_visible() {
        // Two conditions on one run; one is hidden ⇒ drop.
        let runs = vec![cond_run("dual", &["Condition/A", "Condition/B"])];
        let mut table = std::collections::BTreeMap::new();
        table.insert("Condition/A".to_string(), cond("Condition/A", true));
        table.insert("Condition/B".to_string(), cond("Condition/B", false));
        let out = filter_by_conditions(&runs, &table);
        assert!(out.is_empty());
    }

    #[test]
    fn conditions_unknown_id_treated_as_visible() {
        // A reference to a condition not in the document's table
        // shouldn't silently hide content. InDesign treats unknown
        // condition refs as visible.
        let runs = vec![cond_run("orphan", &["Condition/Missing"])];
        let table = std::collections::BTreeMap::new();
        let out = filter_by_conditions(&runs, &table);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn nested_overlay_digit_class() {
        // First 3 digits in mixed text.
        let ov = compute_nested_style_overlay(
            "a1b2c3d4",
            &[ns("S/Num", paged_parse::NestedDelimiter::AnyDigit, 3, true)],
        );
        assert_eq!(ov.len(), 1);
        assert_eq!(&"a1b2c3d4"[ov[0].byte_range.clone()], "a1b2c3");
    }

    // ── Phase 5 renderer — index paragraph builder ────────────────

    #[test]
    fn nested_table_inside_cell_emits_grid_commands() {
        // Outer 1×1 table whose single cell hosts a nested 2×2 table.
        // After build, the page's display list must contain enough
        // rectangle commands to draw the inner table's grid (5
        // horizontal + 3 vertical = 8 lines, plus the outer table's
        // 4 borders = 4) plus glyph emission for inner cell text.
        // A pre-fix build would skip the nested table entirely; we
        // detect the fix by asserting the inner cell's text glyphs
        // are present.
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 600 600"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="10 10 590 590"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
        <Table Self="t-outer" HeaderRowCount="0" FooterRowCount="0"
               BodyRowCount="1" ColumnCount="1">
          <Row Self="or0" Name="0" SingleRowHeight="100"/>
          <Column Self="oc0" Name="0" SingleColumnWidth="400"/>
          <Cell Self="oc0r0" Name="0:0" RowSpan="1" ColumnSpan="1">
            <ParagraphStyleRange>
              <CharacterStyleRange AppliedFont="Inter" PointSize="10">
                <Table Self="t-inner" HeaderRowCount="0" FooterRowCount="0"
                       BodyRowCount="2" ColumnCount="2">
                  <Row Self="ir0" Name="0" SingleRowHeight="30"/>
                  <Row Self="ir1" Name="1" SingleRowHeight="30"/>
                  <Column Self="ic0" Name="0" SingleColumnWidth="150"/>
                  <Column Self="ic1" Name="1" SingleColumnWidth="150"/>
                  <Cell Self="i00" Name="0:0">
                    <ParagraphStyleRange>
                      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
                        <Content>INNER-A</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                  <Cell Self="i11" Name="1:1">
                    <ParagraphStyleRange>
                      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
                        <Content>INNER-B</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                </Table>
              </CharacterStyleRange>
            </ParagraphStyleRange>
          </Cell>
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");

        let font_bytes = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpus/fonts/Inter.ttf"),
        )
        .expect("Inter.ttf fixture");
        let options = PipelineOptions {
            font: Some(&font_bytes),
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");
        // The inner cells contribute commands to the page's display
        // list via emit_cell_paragraph called from inside the new
        // nested-table emit. Count grid-line + glyph commands as a
        // sanity check that the nested table actually rendered.
        //
        // Before the nested-table fix: page would have ~0 commands
        // (the outer cell paragraph had empty runs + table → silent
        // skip). After: ≥ 8 grid rects (5 row lines + 3 col lines)
        // plus inner-cell glyph commands.
        let cmd_count = built.pages[0].list.commands.len();
        assert!(
            cmd_count >= 20,
            "expected nested-table cmds (≥20 for grid + INNER-A + INNER-B), \
             got {cmd_count}"
        );
    }

    #[test]
    fn missing_image_link_emits_diagnostic() {
        // A Rectangle hosting an <Image> whose LinkResourceURI can't be
        // resolved (no AssetResolver wired) should render a placeholder
        // AND surface exactly one ImageLinkMissing diagnostic with the
        // URI attached — previously this was a silent tracing::warn!.
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="r1" GeometricBounds="20 20 120 120">
      <Image LinkResourceURI="file:///nonexistent/photo.jpg"/>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");

        let built = build_document(&doc, &PipelineOptions::default()).expect("build");
        let missing: Vec<_> = built
            .diagnostics
            .items
            .iter()
            .filter(|d| d.code == crate::diagnostics::DiagnosticCode::ImageLinkMissing)
            .collect();
        assert_eq!(
            missing.len(),
            1,
            "expected one ImageLinkMissing diagnostic, got {:?}",
            built.diagnostics.items
        );
        assert_eq!(missing[0].page_index, Some(0));
        assert_eq!(
            missing[0].uri.as_deref(),
            Some("file:///nonexistent/photo.jpg")
        );
    }

    fn test_section(
        self_id: &str,
        page_start: &str,
        style: paged_parse::designmap::NumberingStyle,
        start_at: u32,
    ) -> paged_parse::designmap::Section {
        paged_parse::designmap::Section {
            self_id: self_id.to_string(),
            page_start: Some(page_start.to_string()),
            continue_numbering: false,
            start_at: Some(start_at),
            numbering_style: style,
            section_prefix: None,
            marker: None,
            include_prefix: false,
        }
    }

    #[test]
    fn section_walk_computes_roman_then_arabic_labels() {
        use paged_parse::designmap::NumberingStyle;
        let sections = vec![
            test_section("sec1", "p1", NumberingStyle::LowerRoman, 1),
            test_section("sec2", "p3", NumberingStyle::Arabic, 1),
        ];
        let mut w = SectionWalk::new(&sections);
        // 4 Name-less pages: roman section p1..p2, then arabic from p3.
        let labels: Vec<String> = ["p1", "p2", "p3", "p4"]
            .iter()
            .map(|id| w.next_label(Some(id), None))
            .collect();
        assert_eq!(labels, vec!["i", "ii", "1", "2"]);
        assert!(w.used_fallback);
    }

    #[test]
    fn section_walk_name_is_authoritative() {
        // No sections: a baked Name wins; a Name-less page uses the
        // 1-based fallback that matches the historical behaviour.
        let mut w = SectionWalk::new(&[]);
        assert_eq!(w.next_label(Some("p1"), Some("iii")), "iii");
        assert_eq!(w.next_label(Some("p2"), None), "2");
    }

    /// Build a one-page IDML with a single 1-row × 3-col table in a
    /// text frame, interpolating `table_attrs` onto `<Table>` and
    /// `cell_attrs` onto each `<Cell>`. Shared by the column-divider
    /// and cell-rotation tests.
    fn build_single_table_idml(table_attrs: &str, cell_attrs: &str) -> Vec<u8> {
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Resources/Graphic.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Color Self="Color/Black" Space="CMYK" ColorValue="0 0 0 100"/>
</idPkg:Graphic>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 400"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="10 10 390 390"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        let cell = |name: &str, content: &str| {
            format!(
                r#"<Cell Self="{name}" Name="{name}"{cell_attrs}><ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>{content}</Content></CharacterStyleRange></ParagraphStyleRange></Cell>"#
            )
        };
        let story = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="10">
        <Table Self="t" HeaderRowCount="0" FooterRowCount="0" BodyRowCount="1" ColumnCount="3"{table_attrs}>
          <Row Self="r0" Name="0" SingleRowHeight="40"/>
          <Column Self="cc0" Name="0" SingleColumnWidth="100"/>
          <Column Self="cc1" Name="1" SingleColumnWidth="100"/>
          <Column Self="cc2" Name="2" SingleColumnWidth="100"/>
          {c0}{c1}{c2}
        </Table>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
            c0 = cell("0:0", "A"),
            c1 = cell("1:0", "B"),
            c2 = cell("2:0", "C"),
        );
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(story.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    fn inter_font_bytes() -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpus/fonts/Inter.ttf"),
        )
        .expect("Inter.ttf fixture")
    }

    #[test]
    fn table_column_dividers_emit_extra_edges() {
        // A table-style column-stroke decl must draw interior column
        // dividers — previously nothing rendered for it. Differential:
        // the same table with the decl emits more commands than without.
        let font = inter_font_bytes();
        let count = |table_attrs: &str| -> usize {
            let bytes = build_single_table_idml(table_attrs, "");
            let doc = paged_scene::Document::open(&bytes).expect("open IDML");
            let options = PipelineOptions {
                font: Some(&font),
                ..PipelineOptions::default()
            };
            let built = build_document(&doc, &options).expect("build");
            built.pages[0].list.commands.len()
        };
        let with = count(
            r#" StartColumnStrokeColor="Color/Black" StartColumnStrokeType="Solid" StartColumnStrokeWeight="1""#,
        );
        let without = count("");
        // Two interior dividers (3 columns) → at least two extra edges.
        assert!(
            with >= without + 2,
            "column dividers should add ≥2 edge commands: with={with} without={without}",
        );
    }

    #[test]
    fn cell_rotation_rotates_content() {
        // A cell with RotationAngle="90" rotates its content: at least
        // one emitted command's transform gains a non-zero `b` term
        // (sin 90° = 1). Without rotation, content stays axis-aligned.
        let font = inter_font_bytes();
        let max_b = |cell_attrs: &str| -> f32 {
            let bytes = build_single_table_idml("", cell_attrs);
            let doc = paged_scene::Document::open(&bytes).expect("open IDML");
            let options = PipelineOptions {
                font: Some(&font),
                ..PipelineOptions::default()
            };
            let mut built = build_document(&doc, &options).expect("build");
            built.pages[0]
                .list
                .commands
                .iter_mut()
                .map(|c| c.transform_mut().0[1].abs())
                .fold(0.0f32, f32::max)
        };
        // Glyph command transforms carry the font scale on the
        // diagonal (a/d ≈ 1/units_per_em·size); a 90° rotation moves
        // that scale onto the off-diagonal (b). So rotated |b| ≈ the
        // glyph scale (clearly > 0), while upright |b| ≈ 0.
        let rotated = max_b(r#" RotationAngle="90""#);
        let upright = max_b("");
        assert!(
            upright < 1e-4,
            "unrotated cell content should be axis-aligned, got |b|={upright}"
        );
        assert!(
            rotated > 1e-3 && rotated > upright * 100.0,
            "RotationAngle=90 should rotate content (|b| ≈ glyph scale), got |b|={rotated}"
        );
    }

    #[test]
    fn autosize_height_prevents_overset_drop() {
        // A short frame with lots of text drops overset lines. The same
        // frame with AutoSizingType="HeightOnly" grows to fit instead —
        // no lines dropped, more lines rendered.
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
        let font = inter_font_bytes();
        let build = |auto: &str| -> (usize, usize) {
            let buf = std::io::Cursor::new(Vec::new());
            let mut zip = ZipWriter::new(buf);
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            let deflated =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("designmap.xml", deflated).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
            )
            .unwrap();
            let spread = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 400"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="20 20 60 200">
      <Properties/>
      <TextFramePreference{auto}/>
    </TextFrame>
  </Spread>
</idPkg:Spread>"#
            );
            zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
            zip.write_all(spread.as_bytes()).unwrap();
            // Many short paragraphs so the 40pt-tall frame overflows.
            let mut story = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">"#,
            );
            for i in 0..12 {
                story.push_str(&format!(
                    r#"<ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>Line {i}</Content></CharacterStyleRange></ParagraphStyleRange>"#
                ));
            }
            story.push_str("</Story></idPkg:Story>");
            zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
            zip.write_all(story.as_bytes()).unwrap();
            let bytes = zip.finish().unwrap().into_inner();
            let doc = paged_scene::Document::open(&bytes).expect("open IDML");
            let options = PipelineOptions {
                font: Some(&font),
                ..PipelineOptions::default()
            };
            let built = build_document(&doc, &options).expect("build");
            (built.stats.lines, built.stats.dropped_overflow_lines)
        };
        let (plain_lines, plain_dropped) = build("");
        let (grown_lines, grown_dropped) =
            build(r#" AutoSizingType="HeightOnly" AutoSizingReferencePoint="TopLeftPoint""#);
        assert!(
            plain_dropped > 0,
            "the undersized frame should overset without autosizing"
        );
        assert_eq!(
            grown_dropped, 0,
            "HeightOnly autosizing should drop nothing (frame grows)"
        );
        assert!(
            grown_lines > plain_lines,
            "autosized frame should render more lines: grown={grown_lines} plain={plain_lines}"
        );
    }

    #[test]
    fn hyphenation_zone_is_noop_for_justified_but_active_for_ragged() {
        // W1.17: the Hyphenation Zone is a RAGGED-edge feature. Adobe:
        // "The Hyphenation Zone … applies only when you're using the
        // Single-line Composer with nonjustified text." (Adobe, "Compose
        // and hyphenate text in InDesign",
        // helpx.adobe.com/indesign/using/text-composition.html). A
        // justified paragraph has no rag — every line is flushed to the
        // column — so the zone has nothing to bound and InDesign ignores
        // it. We mirror that exactly: `layout_runs` zeroes the zone for
        // justified paragraphs, so the line breaks are IDENTICAL with or
        // without a HyphenationZone. For a ragged (Left-aligned)
        // paragraph the same zone DOES suppress a hyphen near the right
        // margin and end the line short — proving the fixture is
        // sensitive and the justified equality is a real no-op, not an
        // inert column. (W1.3 landed the zone gate in `compose_paragraph`;
        // W1.17 extends it to the renderer's multi-run path and pins the
        // justified case.)
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
        let font = inter_font_bytes();
        // (justification token, HyphenationZone pt) → per-line source
        // text. The zone is carried on an applied ParagraphStyle because
        // an inline `HyphenationZone` on a `<ParagraphStyleRange>` is not
        // captured by the scene cascade (only the applied style is) —
        // Justification, by contrast, IS read inline.
        let breaks_for = |justification: &str, zone_pt: &str| -> Vec<String> {
            let buf = std::io::Cursor::new(Vec::new());
            let mut zip = ZipWriter::new(buf);
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            let deflated =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("designmap.xml", deflated).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Styles src="Resources/Styles.xml"/>
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
            )
            .unwrap();
            zip.start_file("Resources/Styles.xml", deflated).unwrap();
            let styles = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/Z" Hyphenation="true" HyphenationZone="{zone_pt}"/>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#
            );
            zip.write_all(styles.as_bytes()).unwrap();
            // Narrow column (frame width 140pt) so the long hyphenatable
            // word "communication" lands near the right margin and the
            // zone has something to gate. Tall enough that nothing
            // oversets.
            zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 400"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="20 20 400 160"/>
  </Spread>
</idPkg:Spread>"#,
            )
            .unwrap();
            let story = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Z" Justification="{justification}">
      <CharacterStyleRange AppliedFont="Inter" PointSize="11"><Content>the quick brown communication network protocol gateway</Content></CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
            );
            zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
            zip.write_all(story.as_bytes()).unwrap();
            let bytes = zip.finish().unwrap().into_inner();
            let doc = paged_scene::Document::open(&bytes).expect("open IDML");
            let options = PipelineOptions {
                font: Some(&font),
                collect_breaks: true,
                ..PipelineOptions::default()
            };
            let built = build_document(&doc, &options).expect("build");
            built
                .breaks
                .iter()
                .map(|b| b.source_text.trim().to_string())
                .collect()
        };

        // Justified: a non-zero zone must NOT change the breaks — the
        // zone is a documented no-op for justified text.
        let just_no_zone = breaks_for("FullyJustified", "0");
        let just_zone = breaks_for("FullyJustified", "36");
        assert!(
            just_no_zone.len() >= 2,
            "need a wrap to exercise the zone, got {just_no_zone:?}"
        );
        assert_eq!(
            just_no_zone, just_zone,
            "HyphenationZone must be ignored for justified text: \
             zone-0={just_no_zone:?} vs zone-36={just_zone:?}"
        );
        // The justified control actually hyphenates (so the equality
        // above is meaningful: the zone would have suppressed it if it
        // applied). "communication" splits as "commu-/nication".
        assert!(
            just_no_zone.iter().any(|l| l.ends_with("commu")),
            "justified control should hyphenate near the margin: {just_no_zone:?}"
        );

        // Ragged (Left): the SAME zone DOES move a break — it suppresses
        // the "commu-" hyphen and pushes "communication" whole to the
        // next line, ending line 1 short (the hyphenation-zone trade).
        let rag_no_zone = breaks_for("LeftAlign", "0");
        let rag_zone = breaks_for("LeftAlign", "36");
        assert_ne!(
            rag_no_zone, rag_zone,
            "HyphenationZone must change ragged breaks: \
             zone-0={rag_no_zone:?} vs zone-36={rag_zone:?}"
        );
        assert!(
            rag_no_zone.iter().any(|l| l.ends_with("commu")),
            "ragged zone-0 should still hyphenate: {rag_no_zone:?}"
        );
        assert!(
            rag_zone.iter().all(|l| !l.ends_with("commu")),
            "ragged zone-36 should suppress the commu- hyphen: {rag_zone:?}"
        );
    }

    #[test]
    fn autosize_phase_b_grows_box_and_shifts_neighbour_wrap() {
        // W1.7 Phase B. Frame A is an AutoSizingType="HeightOnly" frame
        // authored undersized (40pt tall) with a fill, a stroke, and an
        // active TextWrap, holding many short paragraphs so it grows to
        // ~10× its authored height. Frame B is a plain neighbour text
        // frame that overlaps A's GROWN vertical band.
        //
        // Two visible Phase-B effects are asserted differentially against
        // a no-autosize control (AutoSizingType absent):
        //   (1) A's painted fill box stretches to the grown extent — the
        //       `FillPath` for A's box is much taller with autosizing.
        //   (2) B's text wraps around A's GROWN box, not its authored
        //       rect — B's line breaks shift vs the control.
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
        let font = inter_font_bytes();
        // Returns (A's painted-box height in pt, B's per-line texts).
        let build = |auto: &str| -> (f32, Vec<String>) {
            let buf = std::io::Cursor::new(Vec::new());
            let mut zip = ZipWriter::new(buf);
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            let deflated =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("designmap.xml", deflated).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_a.xml"/>
  <idPkg:Story src="Stories/Story_b.xml"/>
</Document>"#,
            )
            .unwrap();
            // Page origin (0,0) so the page-outer transform is identity
            // and a box `FillPath`'s transform `d` component is exactly
            // the painted box height in pt.
            //
            // Frame A: authored 40pt tall (top 20, bottom 60), 180 wide,
            // fill Black, with a BoundingBox TextWrap (no offsets). Frame
            // B: a tall neighbour starting at y=80 that overlaps A's
            // grown band (A grows well past y=80). B has no fill, so the
            // only frame `FillPath` is A's box.
            let spread = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 600"/>
    <TextFrame Self="frameA" ParentStory="a" GeometricBounds="20 20 60 200" FillColor="Color/Black">
      <Properties/>
      <TextFramePreference{auto}/>
      <TextWrapPreference Inverse="false" TextWrapMode="BoundingBoxTextWrap">
        <Properties>
          <TextWrapOffset Top="0" Left="0" Bottom="0" Right="0"/>
        </Properties>
      </TextWrapPreference>
    </TextFrame>
    <TextFrame Self="frameB" ParentStory="b" GeometricBounds="80 20 600 400"/>
  </Spread>
</idPkg:Spread>"#
            );
            zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
            zip.write_all(spread.as_bytes()).unwrap();
            // Story A: many short paragraphs so the 40pt frame grows tall.
            let mut story_a = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="a">"#,
            );
            for i in 0..14 {
                story_a.push_str(&format!(
                    r#"<ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>Headline line {i}</Content></CharacterStyleRange></ParagraphStyleRange>"#
                ));
            }
            story_a.push_str("</Story></idPkg:Story>");
            zip.start_file("Stories/Story_a.xml", deflated).unwrap();
            zip.write_all(story_a.as_bytes()).unwrap();
            // Story B: one long paragraph that wraps; its lines that fall
            // in A's grown band get carved on the left, shifting breaks.
            let story_b = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="b">
    <ParagraphStyleRange Justification="LeftAlign">
      <CharacterStyleRange AppliedFont="Inter" PointSize="11"><Content>alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega</Content></CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;
            zip.start_file("Stories/Story_b.xml", deflated).unwrap();
            zip.write_all(story_b.as_bytes()).unwrap();
            let bytes = zip.finish().unwrap().into_inner();
            let doc = paged_scene::Document::open(&bytes).expect("open IDML");
            let options = PipelineOptions {
                font: Some(&font),
                collect_breaks: true,
                ..PipelineOptions::default()
            };
            let built = build_document(&doc, &options).expect("build");
            // A's painted box: the page-outer transform is identity, so
            // the box `FillPath`'s transform is for_rect_in(rect, I) =
            // [w, 0, 0, h, x, y]. `d` (index 3) is the box height. B has
            // no fill, so the single frame-box FillPath is A's.
            let box_h = built.pages[0]
                .list
                .commands
                .iter()
                .find_map(|c| match c {
                    paged_compose::DisplayCommand::FillPath { transform, .. } => {
                        Some(transform.0[3])
                    }
                    _ => None,
                })
                .expect("frame A should emit a fill box");
            // B's per-line source texts (story "b").
            let b_lines: Vec<String> = built
                .breaks
                .iter()
                .filter(|r| r.story_id == "b")
                .map(|r| r.source_text.trim().to_string())
                .collect();
            (box_h, b_lines)
        };

        let (plain_box_h, plain_b_lines) = build("");
        let (grown_box_h, grown_b_lines) =
            build(r#" AutoSizingType="HeightOnly" AutoSizingReferencePoint="TopLeftPoint""#);

        // (1) The painted box stretches to the auto-sized extent. The
        // authored box is 40pt; with 14 lines at ~12pt leading it grows
        // several-fold. Allow generous slack — the exact grown height is
        // an estimate, the contract is "much taller than authored".
        assert!(
            (plain_box_h - 40.0).abs() < 1.0,
            "control box should stay at its authored 40pt height, got {plain_box_h}"
        );
        assert!(
            grown_box_h > plain_box_h * 2.0,
            "autosized box should stretch well past authored: grown={grown_box_h} plain={plain_box_h}"
        );

        // (2) Neighbour text-wrap derives from the GROWN box: B's line
        // breaks shift vs the no-autosize control. (With the control, A
        // is only 40pt tall and ends at y=60, above B's first line at
        // y≈80, so A's authored box barely carves B; the grown box
        // reaches deep into B's column and re-wraps it.)
        assert!(
            !plain_b_lines.is_empty() && !grown_b_lines.is_empty(),
            "both runs should lay out neighbour text"
        );
        assert_ne!(
            grown_b_lines, plain_b_lines,
            "neighbour wrap must shift with the grown box: \
             grown={grown_b_lines:?} plain={plain_b_lines:?}"
        );
    }

    #[test]
    fn autosize_phase_b_reference_point_anchors_box_growth() {
        // W1.7 Phase B reference-point anchoring. The same growing
        // headline frame is auto-sized under three AutoSizingReferencePoint
        // values; the painted box's top-left (`x`,`y` baked into the box
        // FillPath transform) moves per the pinned point:
        //   - TopLeftPoint   → top-left pinned: x,y stay at authored.
        //   - CenterPoint    → centre pinned: box extends up AND left.
        //   - BottomRightPoint → bottom-right pinned: box extends up+left
        //     by the full delta (top-left moves the most).
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
        let font = inter_font_bytes();
        // Returns the painted box rect (x, y, w, h) for frame A.
        let box_rect = |auto: &str| -> (f32, f32, f32, f32) {
            let buf = std::io::Cursor::new(Vec::new());
            let mut zip = ZipWriter::new(buf);
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            let deflated =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("designmap.xml", deflated).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_a.xml"/>
</Document>"#,
            )
            .unwrap();
            // Authored box: 200..240 in y (40pt tall), 100..280 in x
            // (180 wide), centred in a large page so growth in any
            // direction stays on-page. Page origin (0,0) ⇒ identity outer.
            let spread = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 600"/>
    <TextFrame Self="frameA" ParentStory="a" GeometricBounds="200 100 240 280" FillColor="Color/Black">
      <Properties/>
      <TextFramePreference{auto}/>
    </TextFrame>
  </Spread>
</idPkg:Spread>"#
            );
            zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
            zip.write_all(spread.as_bytes()).unwrap();
            let mut story_a = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="a">"#,
            );
            for i in 0..12 {
                // Each line is long enough that the longest-line width
                // estimate exceeds the authored 180pt width, so a
                // HeightAndWidth frame grows on BOTH axes (exercising the
                // horizontal AND vertical reference-point split).
                story_a.push_str(&format!(
                    r#"<ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>Supercalifragilistic headline number {i}</Content></CharacterStyleRange></ParagraphStyleRange>"#
                ));
            }
            story_a.push_str("</Story></idPkg:Story>");
            zip.start_file("Stories/Story_a.xml", deflated).unwrap();
            zip.write_all(story_a.as_bytes()).unwrap();
            let bytes = zip.finish().unwrap().into_inner();
            let doc = paged_scene::Document::open(&bytes).expect("open IDML");
            let options = PipelineOptions {
                font: Some(&font),
                ..PipelineOptions::default()
            };
            let built = build_document(&doc, &options).expect("build");
            built.pages[0]
                .list
                .commands
                .iter()
                .find_map(|c| match c {
                    paged_compose::DisplayCommand::FillPath { transform, .. } => {
                        let t = transform.0;
                        // for_rect_in(rect, I) = [w, 0, 0, h, x, y].
                        Some((t[4], t[5], t[0], t[3]))
                    }
                    _ => None,
                })
                .expect("frame A should emit a fill box")
        };

        let top_left =
            box_rect(r#" AutoSizingType="HeightAndWidth" AutoSizingReferencePoint="TopLeftPoint""#);
        let center =
            box_rect(r#" AutoSizingType="HeightAndWidth" AutoSizingReferencePoint="CenterPoint""#);
        let bottom_right = box_rect(
            r#" AutoSizingType="HeightAndWidth" AutoSizingReferencePoint="BottomRightPoint""#,
        );

        // All three grow to the SAME size (same content), differing only
        // in where the top-left lands.
        let eq = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(
            eq(top_left.2, center.2) && eq(center.2, bottom_right.2),
            "width should be identical across reference points"
        );
        assert!(
            eq(top_left.3, center.3) && eq(center.3, bottom_right.3),
            "height should be identical across reference points"
        );

        // TopLeft: top-left pinned at the authored (100, 200).
        assert!(
            eq(top_left.0, 100.0) && eq(top_left.1, 200.0),
            "TopLeftPoint must pin the authored top-left, got ({}, {})",
            top_left.0,
            top_left.1
        );

        // Centre pinned ⇒ box extends left and up by HALF the delta:
        // top-left sits left of and above the authored corner, but not as
        // far as the BottomRight case (full delta).
        assert!(
            center.0 < 100.0 && center.1 < 200.0,
            "CenterPoint must extend the box up and left, got ({}, {})",
            center.0,
            center.1
        );
        assert!(
            bottom_right.0 < center.0 && bottom_right.1 < center.1,
            "BottomRightPoint must move the top-left further than CenterPoint: \
             br=({}, {}) center=({}, {})",
            bottom_right.0,
            bottom_right.1,
            center.0,
            center.1
        );
        // The bottom-right corner stays pinned at the authored (280, 240)
        // for the BottomRight case.
        assert!(
            eq(bottom_right.0 + bottom_right.2, 280.0)
                && eq(bottom_right.1 + bottom_right.3, 240.0),
            "BottomRightPoint must pin the authored bottom-right corner, got ({}, {})",
            bottom_right.0 + bottom_right.2,
            bottom_right.1 + bottom_right.3
        );
    }

    #[test]
    fn graphic_line_arrowhead_emits_fill() {
        // A GraphicLine with a RightLineEnd arrowhead emits an extra
        // FillPath (the arrowhead) on top of the stroked line.
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
        let count_fills = |right_line_end: &str| -> usize {
            let buf = std::io::Cursor::new(Vec::new());
            let mut zip = ZipWriter::new(buf);
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            let deflated =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("designmap.xml", deflated).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
            )
            .unwrap();
            zip.start_file("Resources/Graphic.xml", deflated).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Color Self="Color/Black" Space="CMYK" ColorValue="0 0 0 100"/>
</idPkg:Graphic>"#,
            )
            .unwrap();
            let spread = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <GraphicLine Self="gl" GeometricBounds="20 20 180 180" StrokeColor="Color/Black" StrokeWeight="3"{right_line_end}/>
  </Spread>
</idPkg:Spread>"#
            );
            zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
            zip.write_all(spread.as_bytes()).unwrap();
            let bytes = zip.finish().unwrap().into_inner();
            let doc = paged_scene::Document::open(&bytes).expect("open IDML");
            let built = build_document(&doc, &PipelineOptions::default()).expect("build");
            built.pages[0]
                .list
                .commands
                .iter()
                .filter(|c| matches!(c, paged_compose::DisplayCommand::FillPath { .. }))
                .count()
        };
        let with_arrow = count_fills(r#" RightLineEnd="TriangleHead""#);
        let without = count_fills("");
        assert_eq!(without, 0, "plain line draws no fill");
        assert_eq!(with_arrow, 1, "arrowhead should add one FillPath");
    }

    #[test]
    fn corner_rect_path_shapes_per_kind() {
        use paged_compose::PathSegment::{CubicTo, LineTo};
        use paged_parse::CornerOption;
        let rect = paged_compose::Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let radii = [Some(20.0); 4];
        let segs = |kind| corner_rect_path(rect, radii, [kind; 4]).segments;
        let cubics = |kind| {
            segs(kind)
                .iter()
                .filter(|s| matches!(s, CubicTo { .. }))
                .count()
        };
        let lines = |kind| {
            segs(kind)
                .iter()
                .filter(|s| matches!(s, LineTo { .. }))
                .count()
        };
        // Rounded / Inverse: one quarter-arc cubic per corner. Inverse
        // is the smooth concave indentation (distinct from Inset's
        // sharp fold-in below).
        assert_eq!(cubics(CornerOption::Rounded), 4);
        assert_eq!(cubics(CornerOption::Inverse), 4);
        // Bevel: straight chamfers — no cubics. Four corners + four
        // edges ⇒ 8 LineTos.
        assert_eq!(cubics(CornerOption::Bevel), 0);
        assert_eq!(lines(CornerOption::Bevel), 8);
        // Inset: InDesign's sharp "fold-in" notch — no cubics, two
        // LineTos per corner (in to `m`, back out) ⇒ strictly more line
        // segments than Bevel's single chamfer per corner. Distinct
        // from Inverse (calibrated + verified distinct in W1.8).
        assert_eq!(cubics(CornerOption::Inset), 0);
        assert!(
            lines(CornerOption::Inset) > lines(CornerOption::Bevel),
            "inset fold-in adds an extra line segment per corner vs bevel"
        );
        // Fancy: the ornamental three-arc scallop — three cubics per
        // corner (calibrated; was a two-cubic ogee before W1.8).
        assert_eq!(cubics(CornerOption::Fancy), 12);
        // None / zero radius: sharp corners, no cubics.
        assert_eq!(cubics(CornerOption::None), 0);
    }

    #[test]
    fn inset_and_inverse_corners_are_distinct_geometry() {
        // W1.8 regression guard: InDesign's Inset (sharp fold-in) and
        // Inverse Rounded (smooth concave arc) must NOT collapse onto
        // the same path. A naive "Inset = quarter-circle cut inward"
        // implementation made them byte-identical.
        use paged_parse::CornerOption;
        let rect = paged_compose::Rect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 60.0,
        };
        let inverse = corner_rect_path(rect, [Some(15.0); 4], [CornerOption::Inverse; 4]);
        let inset = corner_rect_path(rect, [Some(15.0); 4], [CornerOption::Inset; 4]);
        assert_ne!(
            inverse.segments, inset.segments,
            "Inset and Inverse Rounded must render as distinct shapes"
        );
    }

    #[test]
    fn corner_rect_path_every_option_emits_closed_continuous_geometry() {
        // W1.8: all five IDML corner options must emit geometry (a
        // non-degenerate, closed, continuous contour). Verifies segment
        // counts AND that the path's drawn vertices stay inside the
        // rect's bounds with no NaNs — the regression guard for the
        // Inset / Fancy calibration.
        use paged_compose::PathSegment;
        use paged_parse::CornerOption;
        let rect = paged_compose::Rect {
            x: 10.0,
            y: 20.0,
            w: 120.0,
            h: 80.0,
        };
        let r = 18.0_f32;
        for kind in [
            CornerOption::Rounded,
            CornerOption::Inverse,
            CornerOption::Bevel,
            CornerOption::Inset,
            CornerOption::Fancy,
        ] {
            let path = corner_rect_path(rect, [Some(r); 4], [kind; 4]);
            let segs = &path.segments;
            // Starts with a MoveTo, ends with a Close.
            assert!(
                matches!(segs.first(), Some(PathSegment::MoveTo { .. })),
                "{kind:?}: must open with MoveTo"
            );
            assert!(
                matches!(segs.last(), Some(PathSegment::Close)),
                "{kind:?}: must end Close"
            );
            // Walk the contour tracking the current point; every drawn
            // endpoint and control point must be finite and inside the
            // rect's AABB (corner effects only ever cut *inward*, never
            // outside the bounding box). A square notch / scallop / arc
            // that escaped the box would signal a miscomputed corner.
            let inside = |x: f32, y: f32| -> bool {
                x.is_finite()
                    && y.is_finite()
                    && x >= rect.x - 0.01
                    && x <= rect.x + rect.w + 0.01
                    && y >= rect.y - 0.01
                    && y <= rect.y + rect.h + 0.01
            };
            let mut start: Option<(f32, f32)> = None;
            let mut cur = (0.0_f32, 0.0_f32);
            for s in segs {
                match s {
                    PathSegment::MoveTo { x, y } => {
                        assert!(inside(*x, *y), "{kind:?}: MoveTo escapes box");
                        start = Some((*x, *y));
                        cur = (*x, *y);
                    }
                    PathSegment::LineTo { x, y } => {
                        assert!(inside(*x, *y), "{kind:?}: LineTo escapes box");
                        cur = (*x, *y);
                    }
                    PathSegment::CubicTo {
                        cx1,
                        cy1,
                        cx2,
                        cy2,
                        x,
                        y,
                    } => {
                        for (px, py) in [(*cx1, *cy1), (*cx2, *cy2), (*x, *y)] {
                            assert!(inside(px, py), "{kind:?}: cubic point escapes box");
                        }
                        cur = (*x, *y);
                    }
                    PathSegment::QuadTo { cx, cy, x, y } => {
                        for (px, py) in [(*cx, *cy), (*x, *y)] {
                            assert!(inside(px, py), "{kind:?}: quad point escapes box");
                        }
                        cur = (*x, *y);
                    }
                    PathSegment::Close => {
                        // Closing back to the contour start; the current
                        // point should already be (approximately) the
                        // start of the top edge's outgoing point.
                        if let Some(s0) = start {
                            let d = (cur.0 - s0.0).hypot(cur.1 - s0.1);
                            // The walk ends at TL's p_out, which is the
                            // very point MoveTo emitted — continuity.
                            assert!(d < 1e-3, "{kind:?}: contour not continuous (gap {d})");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn inset_corner_folds_in_to_the_rounding_centre() {
        // W1.8 Inset shape: each corner steps in to the inner rounding
        // centre `m` (the "fold-in" apex) then back out to the outgoing
        // edge. We check the top-left corner of a square: its apex must
        // land at `m = (r, r)` and the segment endpoints on the edges at
        // `(0, r)` (incoming) and `(r, 0)` (outgoing).
        use paged_compose::PathSegment::LineTo;
        use paged_parse::CornerOption;
        let rect = paged_compose::Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let r = 25.0_f32;
        let path = corner_rect_path(rect, [Some(r); 4], [CornerOption::Inset; 4]);
        // The contour walks TR, BR, BL, then TL last. The final two
        // LineTos belong to TL: the fold-in apex `m = (r, r)` then the
        // outgoing point `p_out = (r, 0)` on the top edge.
        let line_pts: Vec<(f32, f32)> = path
            .segments
            .iter()
            .filter_map(|s| match s {
                LineTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();
        let n = line_pts.len();
        assert!(n >= 2, "inset emits fold-in LineTos");
        // Last LineTo = TL p_out on the top edge at (r, 0).
        let p_out = line_pts[n - 1];
        assert!(
            (p_out.0 - r).abs() < 1e-3 && p_out.1.abs() < 1e-3,
            "TL p_out at {p_out:?}"
        );
        // Second-to-last = the fold-in apex at m = (r, r).
        let apex = line_pts[n - 2];
        assert!(
            (apex.0 - r).abs() < 1e-3 && (apex.1 - r).abs() < 1e-3,
            "TL fold-in apex at {apex:?}, expected ({r}, {r})"
        );
    }

    #[test]
    fn midpoint_blend_curve() {
        // Default midpoint is exactly linear.
        assert!((midpoint_blend(0.3, 0.5) - 0.3).abs() < 1e-6);
        // At t == mid, the colour-blend fraction is exactly 0.5.
        assert!((midpoint_blend(0.25, 0.25) - 0.5).abs() < 1e-4);
        assert!((midpoint_blend(0.75, 0.75) - 0.5).abs() < 1e-4);
        // A 0.25 midpoint pushes the colour past halfway by the time
        // geometry reaches t == 0.5.
        assert!(midpoint_blend(0.5, 0.25) > 0.5);
        // Endpoints are fixed regardless of midpoint.
        assert!((midpoint_blend(0.0, 0.25)).abs() < 1e-6);
        assert!((midpoint_blend(1.0, 0.25) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn color_lerp_midpoint_is_average() {
        let black = paged_compose::Color::rgba(0.0, 0.0, 0.0, 1.0);
        let white = paged_compose::Color::rgba(1.0, 1.0, 1.0, 1.0);
        let mid = color_lerp(black, white, 0.5);
        assert!((mid.r - 0.5).abs() < 1e-6 && (mid.g - 0.5).abs() < 1e-6);
    }

    #[test]
    fn section_walk_applies_prefix() {
        use paged_parse::designmap::{NumberingStyle, Section};
        let sections = vec![Section {
            self_id: "sec".into(),
            page_start: Some("p1".into()),
            continue_numbering: false,
            start_at: Some(1),
            numbering_style: NumberingStyle::Arabic,
            section_prefix: Some("A-".into()),
            marker: None,
            include_prefix: true,
        }];
        let mut w = SectionWalk::new(&sections);
        assert_eq!(w.next_label(Some("p1"), None), "A-1");
        assert_eq!(w.next_label(Some("p2"), None), "A-2");
    }

    #[test]
    fn vertical_writing_rotates_emitted_commands() {
        // Build a story with StoryDirection="VerticalWritingDirection".
        // After build, every command that landed on the host page
        // should have a rotated transform (90° CW). We detect this
        // by checking the `b` and `c` cells of the transform — for
        // upright transforms b=0; after a 90° CW rotation b=1 (the
        // first column becomes [0, 1]). Identity → rotated proves
        // the post-pass fired.
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="20 20 180 180"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1" StoryDirection="VerticalWritingDirection">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>ABC</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");

        let font_bytes = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpus/fonts/Inter.ttf"),
        )
        .expect("Inter.ttf fixture");
        let options = PipelineOptions {
            font: Some(&font_bytes),
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");

        // At least one command on the page should have a rotated
        // transform. Identity transform = [1, 0, 0, 1, tx, ty];
        // after 90° CW the linear part becomes [0, 1, -1, 0, ...].
        // Glyph FillPath commands have transforms like
        // [scale, 0, 0, scale, tx, ty] with scale ≈ 12/units_per_em.
        // After 90° CW rotation: new linear = [0, scale, -scale, 0].
        // The test detects "a became zero" + "b became non-zero" —
        // any threshold > 0 catches it.
        let mut owned = built.pages[0].list.commands.clone();
        let mut saw_any = false;
        let mut saw_rotated = false;
        for cmd in owned.iter_mut() {
            let xf = cmd.transform_mut();
            saw_any = true;
            // Rotated: a near 0, b non-zero. Pre-rotation: a non-zero, b near 0.
            if xf.0[0].abs() < 1e-3 && xf.0[1].abs() > 1e-4 {
                saw_rotated = true;
                break;
            }
        }
        assert!(saw_any, "expected at least one command on the page");
        assert!(
            saw_rotated,
            "vertical-writing post-rotation should have rotated at least one command"
        );
    }

    #[test]
    fn ruby_annotation_emits_above_base_run() {
        // Paragraph with RubyFlag="true" + RubyString="ruby". The
        // renderer should shape the ruby text at half point size and
        // emit it above the base run. Base ABC at 12pt + ruby
        // "ruby" at 6pt = ~7 extra glyph commands (ruby has up to 4
        // glyphs depending on shaper output).
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="40 40 180 572"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12" RubyFlag="true" RubyString="abc">
        <Content>ABC</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");

        let font_bytes = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpus/fonts/Inter.ttf"),
        )
        .expect("Inter.ttf fixture");
        let options = PipelineOptions {
            font: Some(&font_bytes),
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");

        // Body "ABC" = 3 glyphs; ruby "abc" at 6pt = 3 more.
        // Expect ≥ 6 commands.
        let cmd_count = built.pages[0].list.commands.len();
        assert!(
            cmd_count >= 6,
            "expected base + ruby glyphs (≥6 cmds), got {cmd_count}"
        );
    }

    #[test]
    fn kenten_marks_emit_above_each_glyph() {
        // A paragraph with `KentenKind="Dot"` on its CharacterStyleRange.
        // The renderer should stamp an emphasis mark (small filled
        // ellipse) above every glyph of that run. Pre-fix: zero
        // ellipse commands. Post-fix: one ellipse per character
        // glyph plus any frame chrome.
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 612"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="40 40 180 572"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12" KentenKind="Dot">
        <Content>ABC</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");

        let font_bytes = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpus/fonts/Inter.ttf"),
        )
        .expect("Inter.ttf fixture");
        let options = PipelineOptions {
            font: Some(&font_bytes),
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");

        // The kenten pass emits one ellipse command per glyph in
        // the kenten-tagged run. "ABC" is 3 chars → 3 ellipse
        // commands. The body text alone contributes ~3 glyph
        // FillPath commands; with kenten we add ~3 more ellipses
        // (each rendered as a FillPath of an ellipse path).
        let cmd_count = built.pages[0].list.commands.len();
        assert!(
            cmd_count >= 6,
            "expected glyphs + 3 kenten marks (≥6 cmds), got {cmd_count}"
        );
    }

    #[test]
    fn footnotes_are_captured_onto_their_host_page() {
        // Build an IDML with a body paragraph that anchors two
        // footnotes. After running the pipeline, the page that
        // hosts the body paragraph should carry both footnotes with
        // per-page running numbers 1 and 2.
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="40 40 380 572"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>Anchor host body.</Content>
        <Footnote Self="Footnote/fn1">
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>First footnote.</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Footnote>
        <Footnote Self="Footnote/fn2">
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>Second footnote.</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Footnote>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");

        let font_bytes = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpus/fonts/Inter.ttf"),
        )
        .expect("Inter.ttf fixture");
        let options = PipelineOptions {
            font: Some(&font_bytes),
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");
        assert_eq!(built.pages.len(), 1);
        let footnotes = &built.pages[0].footnotes;
        assert_eq!(footnotes.len(), 2);
        assert_eq!(footnotes[0].number, 1);
        assert_eq!(
            footnotes[0].footnote_self_id.as_deref(),
            Some("Footnote/fn1")
        );
        assert_eq!(footnotes[1].number, 2);
        assert_eq!(
            footnotes[1].footnote_self_id.as_deref(),
            Some("Footnote/fn2")
        );
        // Footnote bodies preserved verbatim.
        assert_eq!(footnotes[0].paragraphs[0].runs[0].text, "First footnote.");
        assert_eq!(footnotes[1].paragraphs[0].runs[0].text, "Second footnote.");

        // Phase 5 footnote pool: the post-pass should have laid out
        // the two footnotes as glyphs at the bottom of frameA.
        // The body alone contributes ~17 glyphs ("Anchor host
        // body."). With the pool emit firing, total commands grow
        // by the two footnote bodies' glyph counts; we assert a
        // floor of 40 to confirm pool emission happened without
        // pinning the exact glyph count (which depends on the
        // shaper's ligature decisions for the fallback font).
        let cmd_count = built.pages[0].list.commands.len();
        assert!(
            cmd_count >= 40,
            "expected footnote pool glyphs in display list (≥40), got {cmd_count}"
        );
    }

    /// W1.7 — shared fixture for the footnote space-reservation tests:
    /// a SHORT frame whose body paragraph anchors `footnote_count`
    /// footnotes, each with a long body so the accumulated pool is a
    /// meaningful fraction of the frame height. `with_footnotes=false`
    /// produces the identical body with the footnotes stripped — the
    /// regression control.
    fn footnote_reserve_idml(footnote_count: usize, with_footnotes: bool) -> Vec<u8> {
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        // Short frame: 120pt tall (top 40, bottom 160), 232pt wide.
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 300 612"/>
    <TextFrame Self="frameA" ParentStory="s1" GeometricBounds="40 40 160 272"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();

        let mut footnotes_xml = String::new();
        if with_footnotes {
            for i in 0..footnote_count {
                footnotes_xml.push_str(&format!(
                    r#"<Footnote Self="Footnote/fn{i}">
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>Footnote body number {i} runs long enough to wrap across several lines in the narrow pool column at the bottom of the host frame.</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Footnote>"#
                ));
            }
        }
        let story = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>Body line one of the host paragraph. Body line two continues the host text. Body line three keeps the frame filled so the footnote pool must reserve space and push these lines upward.</Content>
        {footnotes_xml}
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
        );
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(story.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn footnote_pool_reserves_space_below_body_text() {
        // W1.7 (a): with the reservation pass, NO body line's baseline
        // may fall inside the band the footnote pool occupies
        // (frame content bottom − pool height). Before W1.7 the pool
        // was a pure overlay and body lines ran straight through it.
        let bytes = footnote_reserve_idml(3, true);
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");
        let font_bytes = inter_font_bytes();
        let options = PipelineOptions {
            font: Some(&font_bytes),
            ..PipelineOptions::default()
        };
        let built = build_document(&doc, &options).expect("build");
        assert_eq!(built.pages.len(), 1);

        // The pool must actually exist for this assertion to bite.
        assert!(
            !built.pages[0].footnotes.is_empty(),
            "fixture should capture footnotes"
        );
        let font_table = FontTable::build(&doc, &options);
        let pools = measure_footnote_pools(
            &built.pages,
            &options,
            &doc,
            &font_table,
            &doc.palette,
            None,
        );
        let pool_h: f32 = pools.values().copied().fold(0.0, f32::max);
        assert!(pool_h > 0.0, "expected a measurable footnote pool height");

        // Frame content area: top 40, height 120 (no insets) ⇒ bottom
        // at page-local y = 160. The reserved band starts at
        // bottom − pool_h; every kept body line's baseline sits at or
        // above it (a small epsilon absorbs the rounding the overflow
        // check does in 1/64-pt units).
        let content_bottom_pt = 160.0_f32;
        let reserved_top_pt = content_bottom_pt - pool_h;
        let body_baselines: Vec<f32> = built.pages[0]
            .story_layout
            .iter()
            .filter(|l| l.story_id == "s1")
            .map(|l| l.baseline_y_pt)
            .collect();
        assert!(
            !body_baselines.is_empty(),
            "expected body lines in the layout index"
        );
        let max_baseline = body_baselines.iter().copied().fold(0.0, f32::max);
        assert!(
            max_baseline <= reserved_top_pt + 0.5,
            "body baseline {max_baseline:.2}pt intrudes into the reserved \
             footnote band (starts at {reserved_top_pt:.2}pt, pool {pool_h:.2}pt)"
        );
    }

    #[test]
    fn footnote_reserve_loop_converges_and_pushes_text_up() {
        // W1.7 (b): the compose→measure→re-compose loop terminates
        // (build returns Ok, i.e. it didn't spin past the bail cap), and
        // the reservation demonstrably moved body text — the
        // footnote-bearing build keeps FEWER body lines on the page than
        // the same body with footnotes stripped, because the pool ate
        // the bottom of the frame.
        let font_bytes = inter_font_bytes();
        let options = PipelineOptions {
            font: Some(&font_bytes),
            ..PipelineOptions::default()
        };

        let with_doc =
            paged_scene::Document::open(&footnote_reserve_idml(3, true)).expect("open with");
        let with_built = build_document(&with_doc, &options).expect("build with footnotes");

        let without_doc =
            paged_scene::Document::open(&footnote_reserve_idml(3, false)).expect("open without");
        let without_built = build_document(&without_doc, &options).expect("build without");

        let count_body = |b: &BuiltDocument| {
            b.pages[0]
                .story_layout
                .iter()
                .filter(|l| l.story_id == "s1")
                .count()
        };
        let with_lines = count_body(&with_built);
        let without_lines = count_body(&without_built);
        assert!(
            with_lines < without_lines,
            "reservation should keep fewer body lines on the page \
             (with footnotes: {with_lines}, without: {without_lines})"
        );
    }

    #[test]
    fn no_footnote_frame_is_byte_identical() {
        // W1.7 (c) regression guard: a story with no footnotes must
        // take the pass-0 early break (no rollback, no re-emit), so its
        // display list is identical to the pre-W1.7 single-pass emit.
        // We can't diff against old code in-process, so we assert (1)
        // the build is deterministic across two runs, and (2) it emits
        // ZERO footnote-pool commands — i.e. the reservation machinery
        // left the page untouched. The display-list command count is
        // the golden; update the comment here if a legitimate emit
        // change shifts it.
        let font_bytes = inter_font_bytes();
        let options = PipelineOptions {
            font: Some(&font_bytes),
            ..PipelineOptions::default()
        };
        let bytes = footnote_reserve_idml(0, false);

        let doc_a = paged_scene::Document::open(&bytes).expect("open a");
        let built_a = build_document(&doc_a, &options).expect("build a");
        let doc_b = paged_scene::Document::open(&bytes).expect("open b");
        let built_b = build_document(&doc_b, &options).expect("build b");

        assert!(
            built_a.pages[0].footnotes.is_empty(),
            "no-footnote fixture must capture zero footnotes"
        );
        // Deterministic command count across runs — the reservation
        // loop is a no-op here, so nothing perturbs the display list.
        assert_eq!(
            built_a.pages[0].list.commands.len(),
            built_b.pages[0].list.commands.len(),
            "no-footnote build must be deterministic (reservation loop \
             must not run for a footnote-free story)"
        );
        // No FootnoteOverflow / pool diagnostics leaked in.
        assert!(
            built_a
                .diagnostics
                .items
                .iter()
                .all(|d| d.code != DiagnosticCode::FootnoteOverflow),
            "no-footnote build must not emit footnote diagnostics"
        );
    }

    #[test]
    fn build_index_paragraphs_emits_topic_tab_pages() {
        // Construct a Document by parsing a small IDML so we exercise
        // the full resolve_index → build path. Reusing the parser
        // here is far cheaper than a hand-rolled Document.
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <TextFrame Self="f1" ParentStory="s1" GeometricBounds="10 10 190 190"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_s1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="s1">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Content>The apple is red.</Content>
        <PageReference Self="PR1" TopicName="Apple"/>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");

        let page_labels = vec!["1".to_string()];
        let paragraphs = build_index_paragraphs(&doc, &page_labels);
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].runs.len(), 1);
        assert_eq!(paragraphs[0].runs[0].text, "Apple\t1");
    }

    /// path and decode at native size via `image::load_from_memory`.
    #[test]
    fn track_1a_small_jpeg_keeps_native_dimensions() {
        use image::{ImageBuffer, ImageFormat, Rgb};
        use std::io::Cursor;
        let src: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(128, 96, |x, y| {
            Rgb([
                (x & 0xFF) as u8,
                (y & 0xFF) as u8,
                ((x.wrapping_add(y)) & 0xFF) as u8,
            ])
        });
        let mut buf: Vec<u8> = Vec::new();
        src.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
            .expect("encode JPEG");
        let decoded = decode_image_bytes_with_target_max(&buf, 4096).expect("small JPEG decode");
        assert_eq!(decoded.width, 128);
        assert_eq!(decoded.height, 96);
    }

    // ── W1.21: image clipping-path display-list tests ────────────────

    /// 100×100 RGBA PNG, base64-encoded for inline `<Contents>` so the
    /// image resolves with no asset resolver. Same fixture the
    /// `image-clipping` gen sample embeds.
    const CLIP_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAGQAAABkCAYAAABw4pVUAAAA0klEQVR42u3RMREAMAgEMBQhEEHoqpPi4y9DFKR69yd40xFKiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJEiBAhQoQIESJESHrIAUVvrbCxtZyKAAAAAElFTkSuQmCC";

    /// Build a single-page IDML (in-memory zip) hosting a 100×100 inline
    /// image in a 100 pt rectangle with an identity inner ItemTransform,
    /// carrying the supplied `<ClippingPathSettings>` XML fragment.
    fn build_clip_idml(clipping_path_xml: &str) -> Vec<u8> {
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
        )
        .unwrap();
        let spread = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Rectangle Self="r1" GeometricBounds="0 0 100 100">
      <Image Self="img1" ItemTransform="1 0 0 1 0 0" LinkResourceURI="file:clip.png">
        <Properties>
          <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
            <PathPointType Anchor="0 0"/>
            <PathPointType Anchor="100 0"/>
            <PathPointType Anchor="100 100"/>
            <PathPointType Anchor="0 100"/>
          </PathPointArray></GeometryPathType></PathGeometry>
          <Contents><![CDATA[{CLIP_PNG_B64}]]></Contents>
        </Properties>
        {clipping_path_xml}
        <Link LinkResourceURI="file:clip.png"/>
      </Image>
    </Rectangle>
  </Spread>
</idPkg:Spread>"#
        );
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(spread.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    /// Count the PushClip / Image commands and capture the clip-path
    /// `PathData`s in document order on page 0.
    fn clip_command_summary(built: &BuiltDocument) -> (usize, usize, Vec<paged_compose::PathData>) {
        let page = &built.pages[0];
        let mut push_clips = 0usize;
        let mut images = 0usize;
        let mut clip_paths = Vec::new();
        for cmd in &page.list.commands {
            match cmd {
                paged_compose::DisplayCommand::PushClip { path_id, .. } => {
                    push_clips += 1;
                    if let Some(p) = page.list.paths.get(*path_id) {
                        clip_paths.push(p.clone());
                    }
                }
                paged_compose::DisplayCommand::Image { .. } => images += 1,
                _ => {}
            }
        }
        (push_clips, images, clip_paths)
    }

    #[test]
    fn user_modified_clip_emits_extra_pushclip_around_image() {
        // A star clip (UserModifiedPath) ⇒ the image is wrapped in TWO
        // clips: the frame box AND the star path. Without the clip there
        // would be exactly one PushClip (the frame).
        let clip = r#"<ClippingPathSettings ClippingType="UserModifiedPath" InvertPath="false"
              IncludeInsideEdges="false">
          <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
            <PathPointType Anchor="50 2"/>
            <PathPointType Anchor="62 38"/>
            <PathPointType Anchor="98 38"/>
            <PathPointType Anchor="68 60"/>
            <PathPointType Anchor="80 96"/>
            <PathPointType Anchor="50 74"/>
            <PathPointType Anchor="20 96"/>
            <PathPointType Anchor="32 60"/>
            <PathPointType Anchor="2 38"/>
            <PathPointType Anchor="38 38"/>
          </PathPointArray></GeometryPathType></PathGeometry>
        </ClippingPathSettings>"#;
        let bytes = build_clip_idml(clip);
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");
        let built = build_document(&doc, &PipelineOptions::default()).expect("build");

        let (push_clips, images, clip_paths) = clip_command_summary(&built);
        assert_eq!(images, 1, "exactly one placed image");
        assert_eq!(
            push_clips, 2,
            "frame clip + image clipping path = two PushClips, got {push_clips}"
        );
        // No defer diagnostic for an inline UserModifiedPath.
        assert!(
            !built
                .diagnostics
                .items
                .iter()
                .any(|d| d.code == DiagnosticCode::ImageClippingPathDeferred),
            "inline geometry must not defer"
        );
        // The second clip path (the star) is a single closed contour:
        // one MoveTo + 10 CubicTo (9 between points + 1 closing) + Close.
        let star = clip_paths.last().expect("clip path present");
        let move_tos = star
            .segments
            .iter()
            .filter(|s| matches!(s, paged_compose::PathSegment::MoveTo { .. }))
            .count();
        let cubics = star
            .segments
            .iter()
            .filter(|s| matches!(s, paged_compose::PathSegment::CubicTo { .. }))
            .count();
        assert_eq!(move_tos, 1, "star is a single contour");
        assert_eq!(cubics, 10, "10 anchors ⇒ 10 cubic segments");
    }

    #[test]
    fn invert_clip_path_punches_bbox_with_two_contours() {
        // InvertPath ⇒ the clip path is (image bbox) − (rectangle), so
        // the emitted clip path has TWO MoveTo contours: the bounding
        // box and the punched rectangle.
        let clip = r#"<ClippingPathSettings ClippingType="UserModifiedPath" InvertPath="true"
              IncludeInsideEdges="false">
          <PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
            <PathPointType Anchor="30 30"/>
            <PathPointType Anchor="70 30"/>
            <PathPointType Anchor="70 70"/>
            <PathPointType Anchor="30 70"/>
          </PathPointArray></GeometryPathType></PathGeometry>
        </ClippingPathSettings>"#;
        let bytes = build_clip_idml(clip);
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");
        let built = build_document(&doc, &PipelineOptions::default()).expect("build");

        let (push_clips, images, clip_paths) = clip_command_summary(&built);
        assert_eq!(images, 1);
        assert_eq!(push_clips, 2, "frame + invert clip");
        let invert = clip_paths.last().expect("clip path present");
        let move_tos = invert
            .segments
            .iter()
            .filter(|s| matches!(s, paged_compose::PathSegment::MoveTo { .. }))
            .count();
        assert_eq!(
            move_tos, 2,
            "invert clip = bbox + punched rectangle (two contours)"
        );
    }

    #[test]
    fn compound_clip_path_keeps_hole_contour() {
        // A star with a punched diamond (IncludeInsideEdges) ⇒ the clip
        // path keeps both contours so the hole survives.
        let clip = r#"<ClippingPathSettings ClippingType="UserModifiedPath" InvertPath="false"
              IncludeInsideEdges="true">
          <PathGeometry>
            <GeometryPathType PathOpen="false"><PathPointArray>
              <PathPointType Anchor="10 10"/>
              <PathPointType Anchor="90 10"/>
              <PathPointType Anchor="90 90"/>
              <PathPointType Anchor="10 90"/>
            </PathPointArray></GeometryPathType>
            <GeometryPathType PathOpen="false"><PathPointArray>
              <PathPointType Anchor="40 40"/>
              <PathPointType Anchor="60 40"/>
              <PathPointType Anchor="60 60"/>
              <PathPointType Anchor="40 60"/>
            </PathPointArray></GeometryPathType>
          </PathGeometry>
        </ClippingPathSettings>"#;
        let bytes = build_clip_idml(clip);
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");
        let built = build_document(&doc, &PipelineOptions::default()).expect("build");

        let (_, images, clip_paths) = clip_command_summary(&built);
        assert_eq!(images, 1);
        let compound = clip_paths.last().expect("clip path present");
        let move_tos = compound
            .segments
            .iter()
            .filter(|s| matches!(s, paged_compose::PathSegment::MoveTo { .. }))
            .count();
        assert_eq!(move_tos, 2, "outer square + inner diamond hole");
    }

    #[test]
    fn photoshop_clip_path_defers_with_diagnostic() {
        // PhotoshopPath references a named 8BIM path with no inline
        // geometry ⇒ the image is clipped to the frame only (ONE
        // PushClip) and exactly one ImageClippingPathDeferred diagnostic
        // is recorded, carrying the path name + frame id.
        let clip = r#"<ClippingPathSettings ClippingType="PhotoshopPath" InvertPath="false"
              IncludeInsideEdges="false" AppliedPathName="Path 1"/>"#;
        let bytes = build_clip_idml(clip);
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");
        let built = build_document(&doc, &PipelineOptions::default()).expect("build");

        let (push_clips, images, _) = clip_command_summary(&built);
        assert_eq!(images, 1, "the image still renders (frame-clipped)");
        assert_eq!(push_clips, 1, "frame clip only — no detached clip path");

        let deferred: Vec<_> = built
            .diagnostics
            .items
            .iter()
            .filter(|d| d.code == DiagnosticCode::ImageClippingPathDeferred)
            .collect();
        assert_eq!(deferred.len(), 1, "one defer diagnostic");
        assert_eq!(deferred[0].frame_id.as_deref(), Some("r1"));
        assert!(
            deferred[0].message.contains("Path 1"),
            "diagnostic names the applied path: {}",
            deferred[0].message
        );
    }

    #[test]
    fn no_clipping_path_keeps_single_frame_clip() {
        // Control: an image with no <ClippingPathSettings> keeps exactly
        // one PushClip (the frame) and emits no defer diagnostic — the
        // clipping path is purely additive.
        let bytes = build_clip_idml("");
        let doc = paged_scene::Document::open(&bytes).expect("open IDML");
        let built = build_document(&doc, &PipelineOptions::default()).expect("build");

        let (push_clips, images, _) = clip_command_summary(&built);
        assert_eq!(images, 1);
        assert_eq!(push_clips, 1, "frame clip only when no clipping path");
        assert!(!built
            .diagnostics
            .items
            .iter()
            .any(|d| d.code == DiagnosticCode::ImageClippingPathDeferred));
    }
}
