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
use crate::module::{Geometry, ResolvedFrame};
use crate::AssetResolver;

mod anchored;
mod color_paint;
mod datefmt;
mod image_convert;
mod image_decode;
mod images;
mod links;
mod numbering;
mod shapes;
mod stroke_geom;
mod tables;
mod text_path;
mod footnotes;
mod decorations;
mod nested_styles;
mod metrics;
mod blend_shadow;
mod build_engine;

pub use anchored::AnchoredImageEmit;
use anchored::{emit_anchored_frames_for_paragraph, emit_anchored_rect_image};
pub use datefmt::DateParts;
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
use footnotes::*;
use decorations::*;
pub use nested_styles::*;
pub use metrics::*;
use metrics::map_tab_alignment;
pub(crate) use blend_shadow::{frame_group_opacity, frame_needs_blend_group, pop_blend_group, push_blend_group, resolve_frame_shadow};
use build_engine::*;
pub use build_engine::build_index_paragraphs;

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

/// W1.18a — the document's date clock for `CreationDate` /
/// `ModificationDate` / `OutputDate` text variables.
///
/// **Determinism.** Date variables must NEVER read the wall clock —
/// two renders of the same model on different days would otherwise
/// differ. Every field is an explicit [`DateParts`] supplied by the
/// caller (or defaulted to [`DocumentClock::EPOCH`], a documented
/// constant). `OutputDate` — the moment of *this* render — is the
/// `output` field, defaulting to `modification` so an un-set clock
/// still produces a stable, sensible value rather than "today".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentClock {
    /// `CreationDateType` — when the document was first authored.
    pub creation: DateParts,
    /// `ModificationDateType` — when the document was last edited.
    pub modification: DateParts,
    /// `OutputDateType` — the instant of this render/export. Injected
    /// (never `now()`) so output is reproducible; defaults to
    /// `modification`.
    pub output: DateParts,
}

impl DocumentClock {
    /// A documented, deterministic fallback when the package carries no
    /// metadata and the caller supplies no clock: the Unix epoch
    /// midnight, 1970-01-01 00:00:00. Stable across machines and runs.
    pub const EPOCH: DateParts = DateParts {
        year: 1970,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    };

    /// Build a clock from optional package-derived creation /
    /// modification dates. `output` defaults to `modification` (the
    /// task's documented default) so OutputDate is deterministic even
    /// when the caller doesn't pin a render instant. Absent dates fall
    /// back to [`Self::EPOCH`].
    pub fn from_metadata(creation: Option<DateParts>, modification: Option<DateParts>) -> Self {
        let creation = creation.unwrap_or(Self::EPOCH);
        let modification = modification.unwrap_or(creation);
        Self {
            creation,
            modification,
            output: modification,
        }
    }
}

impl Default for DocumentClock {
    fn default() -> Self {
        Self {
            creation: Self::EPOCH,
            modification: Self::EPOCH,
            output: Self::EPOCH,
        }
    }
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
    /// W1.18a — the date clock for `CreationDate` / `ModificationDate` /
    /// `OutputDate` text variables. Explicit + injectable so date
    /// variables resolve deterministically (never the wall clock). The
    /// caller pins package-derived dates here; the default
    /// ([`DocumentClock::EPOCH`]) renders a stable constant rather than
    /// "today".
    pub document_clock: DocumentClock,
    /// C-1 — plugin scene layers keyed by frame element id (`Self`). When
    /// a body frame's id has an entry, its vector [`SceneLayer`] is lowered
    /// into the frame's content box (transformed by the frame's
    /// `ItemTransform`, clipped to the content box) right after the frame's
    /// own content, so a plugin renders inside a frame through the same
    /// display-list → Vello/tiny-skia path. `None` (the default) is the
    /// no-plugin path and costs nothing.
    pub scene_layers: Option<&'a std::collections::HashMap<String, paged_compose::SceneLayer>>,
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
            document_clock: DocumentClock::default(),
            scene_layers: None,
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

/// W1.18c / W1.19 — the post-layout resolution context handed to the
/// SECOND build pass. Built from the first pass's seated layout: the
/// per-page running-header pickup index, and a story / text-anchor →
/// flat page-index map (cross-reference + text-anchor hyperlink
/// destinations resolve to the page their target story landed on).
///
/// Threaded internally only — `build_document` builds it after the
/// first pass and re-invokes the inner builder with it. Keeping it
/// internal lets `RunningHeaderIndex` stay private to the pipeline
/// module rather than leaking onto the public `PipelineOptions`.
struct PostLayoutCtx {
    running_headers: links::RunningHeaderIndex,
    /// Story `<Self>` id (and any text-anchor id that maps to a story)
    /// → flat 0-based page index of that story's first laid-out line.
    story_page: HashMap<String, u32>,
}

/// Build one `BuiltPage` per `<Page>` in the document. Each page's
/// display list contains only frames whose centres fall inside the
/// page's `GeometricBounds`. Frames placed entirely on the pasteboard
/// (rare) land on the first page so they don't disappear silently.
///
/// Returns a `BuiltDocument` with aggregated stats. Use `build` for
/// the historical single-page (union of all bounds) shape.
///
/// W1.18c / W1.19 — when the document contains running-header variables
/// or cross-reference (text-anchor) destinations, this runs the inner
/// builder TWICE: the first pass seats the text, then we index where
/// each style / story landed and re-run so running headers + page-number
/// cross-references resolve against the CURRENT layout. The re-run is
/// gated; documents without those features build in a single pass.
pub fn build_document(
    document: &Document,
    options: &PipelineOptions,
) -> anyhow::Result<BuiltDocument> {
    let first = build_document_inner(document, options, None)?;

    // Does the document need the post-layout pass? (Same predicate the
    // inner builder uses; cheap to recompute and avoids plumbing a flag
    // back out.)
    let dm = &document.container.designmap;
    let has_running_header = dm.text_variables.iter().any(|v| {
        matches!(
            v.variable_type.as_deref(),
            Some("RunningHeaderType") | Some("RunningHeaderVariableType")
        )
    });
    let has_text_anchor_dest = options.collect_link_regions
        && dm.hyperlink_destinations.iter().any(|d| {
            matches!(
                &d.kind,
                paged_parse::HyperlinkDestinationKind::TextAnchor(_)
            )
        });
    if !has_running_header && !has_text_anchor_dest {
        return Ok(first);
    }

    // Build the resolution context from the first pass's seated layout,
    // then re-run with it in hand. Only one re-run ever happens (the
    // inner builder, given a `post`, never asks for another).
    let post = build_post_layout_ctx(document, &first);
    build_document_inner(document, options, Some(&post))
}

/// W1.18c / W1.19 — derive the running-header pickup index + the
/// story→page map from a first-pass [`BuiltDocument`]. Walks every
/// page's `story_layout`, attributing each line's source paragraph to
/// its applied paragraph style, and records the first / last matching
/// text per (page, style). `fallback` carries forward the most-recent
/// match so a page with no own occurrence inherits the prior page's.
fn build_post_layout_ctx(document: &Document, first: &BuiltDocument) -> PostLayoutCtx {
    let mut running_headers = links::RunningHeaderIndex::default();
    let mut story_page: HashMap<String, u32> = HashMap::new();

    // Pre-index each story's paragraphs by index → (applied style,
    // concatenated text) so we can attribute a laid-out line to its
    // source paragraph's style + read its full text.
    let mut para_style: HashMap<(&str, u32), &str> = HashMap::new();
    let mut para_text: HashMap<(&str, u32), String> = HashMap::new();
    for parsed in &document.stories {
        for (p_idx, para) in parsed.story.paragraphs.iter().enumerate() {
            let key = (parsed.self_id.as_str(), p_idx as u32);
            if let Some(style) = para.paragraph_style.as_deref() {
                para_style.insert(key, style);
            }
            let text: String = para.runs.iter().map(|r| r.text.as_str()).collect();
            para_text.insert(key, text);
        }
    }

    // Walk pages in order; for each line, record where its story landed
    // (first occurrence wins) and, when its paragraph carries a style,
    // the first / last matching text on that page.
    //
    // `seen_on_page` tracks which (page, style) firsts are already set so
    // a later line on the same page only updates `last`. `carry` holds
    // the most-recent matched text per style for the fallback walk.
    let mut carry: HashMap<String, String> = HashMap::new();
    for (page_idx, page) in first.pages.iter().enumerate() {
        // Seed every style's fallback for this page from the carry-over
        // BEFORE processing the page's own lines, so a page with no own
        // match inherits the prior page's value.
        for (style, text) in &carry {
            running_headers
                .fallback
                .insert((page_idx, style.clone()), text.clone());
        }
        for line in &page.story_layout {
            // Story → first page it appears on.
            story_page
                .entry(line.story_id.clone())
                .or_insert(page_idx as u32);
            let key = (line.story_id.as_str(), line.paragraph_idx);
            let Some(style) = para_style.get(&key).copied() else {
                continue;
            };
            let text = para_text.get(&key).cloned().unwrap_or_default();
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }
            let idx_key = (page_idx, style.to_string());
            running_headers
                .first
                .entry(idx_key.clone())
                .or_insert_with(|| trimmed.clone());
            running_headers.last.insert(idx_key, trimmed.clone());
            // Update both the carry-forward AND this page's fallback so
            // a LATER page with no match (and this page, were it queried
            // for fallback) sees the freshest value.
            carry.insert(style.to_string(), trimmed.clone());
            running_headers
                .fallback
                .insert((page_idx, style.to_string()), trimmed);
        }
    }

    PostLayoutCtx {
        running_headers,
        story_page,
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

/// C-1 — splice a plugin [`paged_compose::SceneLayer`] into a frame, right
/// after the frame's own content. Looks the frame's `Self` id up in the
/// registry; on a hit, builds the content-origin → page transform (the
/// frame's `frame_outer_transform` composed with the content-box offset)
/// and lowers the layer clipped to the content box. A no-op when no
/// registry is wired, the frame has no id, or no layer is registered —
/// so the no-plugin render path is untouched. `inset` is the text-frame
/// content inset `[top,left,bottom,right]`; pass `None` for shapes (whose
/// content box is the bounds).
fn emit_frame_scene_layer(
    page: &mut BuiltPage,
    self_id: Option<&str>,
    bounds: paged_parse::Bounds,
    inset: Option<[f32; 4]>,
    item_transform: Option<[f32; 6]>,
    registry: Option<&std::collections::HashMap<String, paged_compose::SceneLayer>>,
    font_bytes: Option<&[u8]>,
) {
    let Some(registry) = registry else { return };
    let Some(id) = self_id else { return };
    let Some(layer) = registry.get(id) else {
        return;
    };
    if layer.items.is_empty() {
        return;
    }
    let outer = frame_outer_transform(page, item_transform);
    let ins = inset.unwrap_or([0.0; 4]);
    let content_left = bounds.left + ins[1];
    let content_top = bounds.top + ins[0];
    let content_w = (bounds.right - bounds.left - ins[1] - ins[3]).max(0.0);
    let content_h = (bounds.bottom - bounds.top - ins[0] - ins[2]).max(0.0);
    let content_outer = outer.compose(&Transform::translate(content_left, content_top));

    // C-1.1 — the default-font shaping face + outliner for `SceneItem::Text`,
    // built once per layer. `None` when the build has no font (text items
    // are then skipped, like the renderer's own no-font text path). v1
    // renders every text run in this default face (the run's `family`/
    // `style` hints are reserved for per-run selection).
    let text_faces = font_bytes.and_then(|b| {
        let rb = rustybuzz::Face::from_slice(b, 0)?;
        let ttf = ttf_parser::Face::parse(b, 0).ok()?;
        Some((rb, ttf))
    });
    let text_outliner = text_faces.as_ref().map(|(_, ttf)| TtfOutliner::new(ttf));

    paged_compose::emit_scene_layer(
        &mut page.list,
        layer,
        content_outer,
        (content_w, content_h),
        |list, t, xf| {
            // Lower a text run: shape with the default face, position glyphs
            // at the transformed baseline (`xf.apply(x, y)`), and emit glyph
            // FillPaths through the standard glyph slice (upright in page
            // space — full per-glyph affine for rotated frames is a
            // follow-on, §8.5).
            let (Some((rb, _)), Some(outliner)) = (text_faces.as_ref(), text_outliner.as_ref())
            else {
                return;
            };
            let shaped = paged_text::shape_run(rb, &t.text, t.size);
            if shaped.glyphs.is_empty() {
                return;
            }
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
                    font_id: u32::MAX,
                    point_size: t.size,
                    underline: false,
                    strikethru: false,
                    x_scale: 1.0,
                    y_scale: 1.0,
                    skew_deg: 0.0,
                    ch: None,
                });
                cursor = cursor.saturating_add(g.x_advance);
            }
            let origin = xf.apply(t.x, t.y);
            let paint = Paint::Solid(paged_compose::scene_paint_to_color(t.paint));
            emit_glyph_slice(
                &positioned,
                u32::MAX,
                t.size,
                |_| paint,
                origin,
                outliner,
                list,
            );
        },
    );
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
mod tests;
