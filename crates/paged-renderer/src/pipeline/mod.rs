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
    emit_stroke_rect, emit_stroke_rect_transformed, Color, DisplayList, DropShadow, GlyphOutliner,
    Paint, PathData, PathSegment, Rect, Stroke, Transform, TtfOutliner,
};
use paged_parse::{
    graphic, Graphic, GraphicLine, Oval, PathAnchor, Polygon, Rectangle, TextFrame, TextPath,
};
use paged_scene::Document;

use crate::diagnostics::{Diagnostic, DiagnosticCode, RenderDiagnostics};
use crate::module::{Geometry, ResolvedFrame};
use crate::AssetResolver;

mod anchored;
mod blend_shadow;
mod build_engine;
mod color_paint;
mod compose_opts;
mod datefmt;
mod decorations;
mod deltas;
mod font_table;
mod footnotes;
mod geom;
mod image_convert;
mod image_decode;
mod images;
mod links;
mod metrics;
mod nested_styles;
mod numbering;
mod shapes;
mod stroke_geom;
mod tables;
mod text_frame;
mod text_path;

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

pub(crate) use blend_shadow::{
    frame_group_opacity, frame_needs_blend_group, pop_blend_group, push_blend_group,
    resolve_frame_shadow,
};
pub use build_engine::build_index_paragraphs;
use build_engine::*;
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
use compose_opts::*;
use decorations::*;
use deltas::*;
pub use font_table::FontTable;
use font_table::*;
use footnotes::*;
use geom::*;
pub(crate) use geom::{
    fnv_1a_u64, frame_fill_is_transparent, frame_stroke_is_visible, path_signature,
    transform_bounds,
};
use image_convert::{cmyk32_to_rgba, l16_to_rgba, l8_to_rgba, rgb24_to_rgba};
use image_decode::decode_image_bytes;
#[cfg(test)]
use image_decode::decode_image_bytes_with_target_max;
use metrics::map_tab_alignment;
pub use metrics::*;
pub use nested_styles::*;
use numbering::{bullet_marker_character_style, list_prefix};
#[cfg(test)]
use numbering::{format_number, substitute_numbering_expression};
use text_frame::*;
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
    /// C-6 (I-06) — image resource providers keyed by frame element id
    /// (`Self`). When a body frame's id has an entry, the renderer pulls
    /// pyramid tiles from the provider at the mip level matching
    /// [`Self::render_scale`] and assembles them into the frame's content
    /// box as ordinary `DisplayCommand::Image` entries (the same lane the
    /// C-1 image scene layer + placed assets use). Missing tiles are
    /// recorded on `BuiltPage::resource_tiles_needed` for the host to fill
    /// asynchronously — compose never blocks. `None` (the default) keeps
    /// the existing whole-image lane and costs nothing. Keyed by frame id
    /// (the worker maps the `x-paged-image:<frame>` claim id back to the
    /// frame before building); the entry's `image_id` is what the
    /// emitted `ResourceTilesNeeded` carries.
    pub resource_providers:
        Option<&'a std::collections::HashMap<String, ResourceProviderEntry<'a>>>,
    /// C-6 — the rasteriser scale (px-per-content-pt) the assembled tiles
    /// will be drawn at, used only to pick the pyramid mip level
    /// ([`crate::resource_provider::mip_level_for_scale`]). Default `1.0`
    /// (full resolution). The display list itself stays
    /// resolution-independent: this is a *hint* for which level of detail
    /// to assemble, not a geometry scale.
    pub render_scale: f32,
}

/// C-6 — one frame's claimed image-resource entry on [`PipelineOptions`]:
/// the provider to pull tiles from, the pyramid geometry, and the
/// provider-claimed `image_id` the [`crate::resource_provider::
/// ResourceTilesNeeded`] signal carries. Borrowed (not owned) so the
/// worker's cache stays the single source of truth across rebuilds.
#[derive(Clone, Copy)]
pub struct ResourceProviderEntry<'a> {
    /// Provider-claimed id (`x-paged-image:<frame>`); echoed in the
    /// `ResourceTilesNeeded` request so the host knows which claim to fill.
    pub image_id: &'a str,
    /// Pyramid geometry (base extent, level count, tile size).
    pub pyramid: crate::resource_provider::ResourcePyramid,
    /// The tile source. One provider may back many claimed images.
    pub provider: &'a dyn crate::resource_provider::ImageResourceProvider,
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
            resource_providers: None,
            render_scale: 1.0,
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
    /// C-6 (I-06) — tiles a claimed image resource lacked at the chosen
    /// mip level while emitting this page. The whole-image lane (or a
    /// coarser cached level) painted in their place; the host fills the
    /// gaps asynchronously and the next build sharpens the frame. Empty
    /// unless `PipelineOptions::resource_providers` is wired and a claimed
    /// frame on this page had a cache miss. Aggregated verbatim into
    /// `BuiltDocument::resource_tiles_needed`.
    pub resource_tiles_needed: Vec<crate::resource_provider::ResourceTilesNeeded>,
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
    /// C-6 (I-06) — every page's tile-miss requests, aggregated. The
    /// worker turns these into `ResourceTilesNeeded` wire messages so the
    /// host fetches + submits the missing tiles. Empty when no resource
    /// provider was wired (the default) or every claimed image was fully
    /// cached at the chosen level.
    pub resource_tiles_needed: Vec<crate::resource_provider::ResourceTilesNeeded>,
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
    let dm = &document.designmap;
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
        resource_tiles_needed: Vec::new(),
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

#[cfg(test)]
mod tests;
