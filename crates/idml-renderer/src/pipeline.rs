//! End-to-end pipeline: `Document` → `DisplayList` → `RgbaImage`.
//!
//! Everything the inspect binary does minus the pretty-printing. This
//! is the thin, reusable top-level Rust API that hosts (WASM binding,
//! native tools, the fidelity harness) call into.
//!
//! The pipeline consumes `&idml_scene::Document` — parsing and resource
//! walking live in that crate so we stay focused on layout + emission.

use std::collections::HashMap;

use bytes::Bytes;
use idml_compose::{
    emit_ellipse, emit_glyph_slice, emit_glyph_slice_stroke, emit_line, emit_paragraph, emit_rect,
    emit_stroke_rect, emit_stroke_rect_transformed, Color, DisplayList, DropShadow, GlyphCacheKey,
    GlyphOutliner, Paint, PathData, PathSegment, Rect, Stroke, Transform, TtfOutliner,
};
use idml_parse::{
    graphic, Graphic, GraphicLine, Oval, PathAnchor, Polygon, Rectangle, TextFrame, TextPath,
};
use idml_scene::Document;

use crate::module::geometry::rewrite_tail_for_overprint;
use crate::module::{Geometry, ResolvedFrame};
use crate::AssetResolver;

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
    /// through ICC instead of the naive math in `idml-parse::graphic`.
    /// None → naive conversion (existing behaviour).
    pub cmyk_icc_profile: Option<&'a [u8]>,
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
}

/// Missing-image placeholder calibration (Q-22). Originally P-02
/// shipped with 0.7-grey + 0.5pt 0.25-grey X, which under-printed
/// against InDesign's reference. Histogramming the reference PNGs for
/// magazine-editorial-layout / catalog / project-case-study-template
/// puts the target at ~50% RGB grey with a 1.5pt near-black stroke.
const PLACEHOLDER_FILL_RGB: f32 = 0.5;
const PLACEHOLDER_X_STROKE_PT: f32 = 1.5;
const PLACEHOLDER_X_RGB: f32 = 0.0;

/// Track 1a: longest-edge cap for raster decode. JPEGs whose declared
/// dimensions exceed this on either axis are decoded through
/// `jpeg-decoder`'s DCT scaling (1/2, 1/4, or 1/8) so we never
/// materialise the full RGBA8 buffer — the annual-report-template
/// cover JPEG is 5760×9000 ≈ 198MB at RGBA8, which the previous
/// `image::load_from_memory` path allocated in one shot. 4096px keeps
/// us safely under one rasteriser tile target while still hitting
/// 300dpi for any frame up to ~13.6" on the longest edge.
const DECODE_MAX_RASTER_PX: u32 = 4096;

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
            frame_drop_shadow: None,
            font_metrics_overrides: &[],
            missing_image_placeholder: true,
        }
    }
}

/// Page bounding box and display-list built from a `Document`.
#[derive(Debug)]
pub struct BuiltPage {
    pub width_pt: f32,
    pub height_pt: f32,
    /// Page origin in spread coordinates (top-left). The display list's
    /// commands are page-relative — the rasterizer treats (0, 0) as
    /// the page's top-left corner regardless of where the page sits in
    /// its parent spread.
    pub spread_origin: (f32, f32),
    pub list: DisplayList,
    /// Aggregated counts, useful for logging / CI reporting.
    pub stats: PipelineStats,
}

/// Multi-page render output. Each entry is a fully populated
/// `BuiltPage` with its own DisplayList and dimensions.
#[derive(Debug)]
pub struct BuiltDocument {
    pub pages: Vec<BuiltPage>,
    pub stats: PipelineStats,
}

#[derive(Debug, Default, Clone)]
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
        match idml_color::IccTransform::cmyk_to_linear_rgb(bytes) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(error = %e, "failed to build CMYK ICC transform; using naive conversion");
                None
            }
        }
    });
    let mut pages: Vec<BuiltPage> = Vec::new();
    let mut total_stats = PipelineStats::default();

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
    let mut spread_page_ranges: Vec<std::ops::Range<usize>> =
        Vec::with_capacity(document.spreads.len());
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        total_stats.spreads += 1;
        let start = pages.len();
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
            // override). Falling back to the 1-based body-page index
            // matches the pre-Section behaviour for IDMLs that omit
            // Name (rare; mostly synthetic test fixtures).
            page_labels.push(
                p.name
                    .clone()
                    .unwrap_or_else(|| (pages.len() + 1).to_string()),
            );
            pages.push(BuiltPage {
                width_pt: bounds_in_spread.width(),
                height_pt: bounds_in_spread.height(),
                spread_origin: (bounds_in_spread.left, bounds_in_spread.top),
                list: DisplayList::new(),
                stats: PipelineStats::default(),
            });
        }
        spread_page_ranges.push(start..pages.len());
    }
    total_stats.pages = pages.len();
    if pages.is_empty() {
        // Documents without a page (rare but valid) get a single
        // letter-sized canvas so callers always see a renderable output.
        pages.push(BuiltPage {
            width_pt: 612.0,
            height_pt: 792.0,
            spread_origin: (0.0, 0.0),
            list: DisplayList::new(),
            stats: PipelineStats::default(),
        });
        page_geometries.push(PageGeom {
            bounds_in_spread: idml_parse::Bounds {
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
        // Body-page OverrideList enumerates master items the body has
        // replaced with its own copies — skip them here so we don't
        // stamp the placeholder under the body's override.
        let body_page = document.spreads[geom.host_spread_idx]
            .spread
            .pages
            .get(geom.local_page_idx);
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
        let master_page_bounds: Vec<idml_parse::Bounds> = master
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
        // live-page coords; for sample.idml this is identity.
        let mpt = document.spreads[geom.host_spread_idx]
            .spread
            .pages
            .get(geom.local_page_idx)
            .and_then(|p| p.master_page_transform);
        let (mpt_tx, mpt_ty) = mpt.map(|m| (m[4], m[5])).unwrap_or((0.0, 0.0));
        let dx = target_origin.0 - master_page_origin.0 + mpt_tx;
        let dy = target_origin.1 - master_page_origin.1 + mpt_ty;

        // Pick the master page index that contains the centroid of
        // the given spread-coord bounds; falls back to the nearest
        // page so items hugging the centre line don't get dropped.
        let master_page_for = |b: idml_parse::Bounds| -> usize {
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
        let item_belongs = |b: idml_parse::Bounds| -> bool {
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
            copy.item_transform = Some(compose_outer_translation(copy.item_transform, dx, dy));
            emit_text_frame_into(
                &mut pages[i],
                &copy,
                document,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
                None, // master items don't carry a drop shadow today.
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
            copy.item_transform = Some(compose_outer_translation(copy.item_transform, dx, dy));
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
            copy.item_transform = Some(compose_outer_translation(copy.item_transform, dx, dy));
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
            copy.item_transform = Some(compose_outer_translation(copy.item_transform, dx, dy));
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
            copy.item_transform = Some(compose_outer_translation(copy.item_transform, dx, dy));
            emit_line_into(
                &mut pages[i],
                &copy,
                document,
                palette,
                cmyk_xform.as_ref(),
            );
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
    let mut page_image_caches: Vec<HashMap<String, idml_compose::ImageId>> =
        (0..pages.len()).map(|_| HashMap::new()).collect();
    // Renderer-scoped (URI → DecodedImage) cache so an image
    // referenced from multiple pages is decoded once. The cached
    // DecodedImage is cloned into each page's image pool — the
    // memcpy is cheap; the saved decode (PNG/JPEG → RGBA) is not.
    // Build a layer-visibility map once: any item whose `ItemLayer`
    // points at a hidden or non-printable layer is suppressed. Items
    // without an explicit ItemLayer always render — matches InDesign's
    // single-layer-by-default behaviour.
    let layer_renders: std::collections::HashMap<&str, bool> = document
        .container
        .designmap
        .layers
        .iter()
        .map(|l| (l.self_id.as_str(), l.visible && l.printable))
        .collect();
    let layer_visible = |layer_ref: Option<&str>| -> bool {
        match layer_ref {
            Some(id) => layer_renders.get(id).copied().unwrap_or(true),
            None => true,
        }
    };

    let mut decoded_image_cache: HashMap<String, idml_compose::DecodedImage> = HashMap::new();
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
    // Q-10: IDML lists layers top-first (layers[0] = topmost). Build a
    // map so cross-shape iteration can paint back-to-front regardless
    // of the per-vec XML order the legacy loop walked.
    let layer_z_index: std::collections::HashMap<&str, usize> = document
        .container
        .designmap
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| (l.self_id.as_str(), i))
        .collect();
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        let range = spread_page_ranges[spread_idx].clone();
        let local_geoms = &page_geometries[range.clone()];
        let mut frame_spans = crate::module::SpreadFrameSpans::default();
        frame_spans.text_frames = vec![None; spread.text_frames.len()];
        frame_spans.rectangles = vec![None; spread.rectangles.len()];
        frame_spans.ovals = vec![None; spread.ovals.len()];
        frame_spans.graphic_lines = vec![None; spread.graphic_lines.len()];
        frame_spans.polygons = vec![None; spread.polygons.len()];

        // Q-10: build a flat (layer_z, xml_order, FrameRef) list from
        // `frames_in_order` so cross-shape z-order honours ItemLayer.
        // Items without `ItemLayer` keep their XML position by sharing
        // `usize::MAX` as the sort key — combined with a stable sort
        // they stay where they were. The sort is a no-op when all
        // items resolve to the same layer-z (legacy behaviour).
        let layer_z_of = |fr: idml_parse::FrameRef| -> usize {
            let id = match fr {
                idml_parse::FrameRef::TextFrame(i) => {
                    spread.text_frames.get(i).and_then(|f| f.item_layer.as_deref())
                }
                idml_parse::FrameRef::Rectangle(i) => {
                    spread.rectangles.get(i).and_then(|f| f.item_layer.as_deref())
                }
                idml_parse::FrameRef::Oval(i) => {
                    spread.ovals.get(i).and_then(|f| f.item_layer.as_deref())
                }
                idml_parse::FrameRef::GraphicLine(i) => {
                    spread.graphic_lines.get(i).and_then(|f| f.item_layer.as_deref())
                }
                idml_parse::FrameRef::Polygon(i) => {
                    spread.polygons.get(i).and_then(|f| f.item_layer.as_deref())
                }
                // Group: derive layer from the first leaf member with
                // an ItemLayer. If none, treat as "no layer" (MAX).
                idml_parse::FrameRef::Group(_) => None,
            };
            id.and_then(|s| layer_z_index.get(s).copied()).unwrap_or(usize::MAX)
        };
        let frames_ordered: Vec<idml_parse::FrameRef> = if spread.frames_in_order.is_empty() {
            // Legacy path: a parser revision predating
            // `frames_in_order` (or a spread carrying only frames the
            // parser couldn't classify) → fall through to the same
            // XML-vec walk as before. Builds a synthetic flat list by
            // concatenating the per-shape vecs in their historical
            // order.
            let mut v: Vec<idml_parse::FrameRef> = Vec::new();
            v.extend((0..spread.text_frames.len()).map(idml_parse::FrameRef::TextFrame));
            v.extend((0..spread.rectangles.len()).map(idml_parse::FrameRef::Rectangle));
            v.extend((0..spread.ovals.len()).map(idml_parse::FrameRef::Oval));
            v.extend((0..spread.graphic_lines.len()).map(idml_parse::FrameRef::GraphicLine));
            v.extend((0..spread.polygons.len()).map(idml_parse::FrameRef::Polygon));
            v
        } else {
            let mut keyed: Vec<(usize, usize, idml_parse::FrameRef)> = spread
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
            let multi_layer = first.map_or(false, |f| zs.any(|z| z != f));
            if multi_layer {
                // Descending layer-z (high index = bottom layer →
                // paint first). Stable sort keeps XML order as the
                // tiebreaker within a layer.
                keyed.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            }
            keyed.into_iter().map(|(_, _, fr)| fr).collect()
        };

        // Emit one FrameRef. Recurses through Group members so group
        // children render at the group's XML slot.
        fn emit_one(
            fr: idml_parse::FrameRef,
            spread: &idml_parse::Spread,
            range: &std::ops::Range<usize>,
            local_geoms: &[PageGeom],
            pages: &mut [BuiltPage],
            page_image_caches: &mut [HashMap<String, idml_compose::ImageId>],
            decoded_image_cache: &mut HashMap<String, idml_compose::DecodedImage>,
            frame_to_page: &mut HashMap<String, usize>,
            frame_spans: &mut crate::module::SpreadFrameSpans,
            total_stats: &mut PipelineStats,
            document: &Document,
            palette: &Graphic,
            options: &PipelineOptions,
            cmyk_xform: Option<&idml_color::IccTransform>,
        ) {
            match fr {
                idml_parse::FrameRef::TextFrame(idx) => {
                    let Some(frame) = spread.text_frames.get(idx) else {
                        return;
                    };
                    if !is_layer_visible(document, frame.item_layer.as_deref()) {
                        return;
                    }
                    total_stats.frames += 1;
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
                idml_parse::FrameRef::Rectangle(idx) => {
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
                idml_parse::FrameRef::Oval(idx) => {
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
                idml_parse::FrameRef::GraphicLine(idx) => {
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
                idml_parse::FrameRef::Polygon(idx) => {
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
                idml_parse::FrameRef::Group(gi) => {
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
                &mut decoded_image_cache,
                &mut frame_to_page,
                &mut frame_spans,
                &mut total_stats,
                document,
                palette,
                options,
                cmyk_xform.as_ref(),
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

    let font_table = FontTable::build(document, options);
    // One hyphenator per render. We currently only build English-US;
    // the document's `AppliedLanguage` is honoured via the cascade,
    // but unrecognised values fall back to this dictionary so we
    // always have *some* hyphenation when a paragraph requests it.
    // Multi-language docs will grow this into a HashMap keyed by
    // resolved language string.
    let hyphenator = idml_text::Hyphenator::for_language(idml_text::Language::EnglishUS);

    // Per-page wrap exclusion rectangles (spread coords, expanded by
    // the wrap's offsets). Only items with TextWrapMode != "None"
    // contribute. Used by StoryEmitter::new to shrink the head text
    // frame's effective column width and shift its origin past any
    // intruding shape.
    let wrap_rects_per_page = collect_wrap_rects_per_page(document, &spread_page_ranges);

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
        let chain: Vec<&TextFrame> = vec![master_frame];
        let chain_pages: Vec<usize> = vec![*page_idx];
        let head_wrap_rects: &[WrapShape] = &[];
        let chain_wrap_rects: Vec<&[WrapShape]> = vec![&[]];
        let mut emitter = StoryEmitter::new(
            document,
            options,
            palette,
            cmyk_xform.as_ref(),
            &font_table,
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
        );
        for paragraph in &parsed.story.paragraphs {
            emitter.emit_paragraph(paragraph, &mut pages, &mut total_stats);
        }
        emitter.apply_vertical_justification(&mut pages);
        emitter.apply_polygon_clip(&mut pages);
        emitter.apply_blend_groups(&mut pages);
        anchored_image_queue.extend(emitter.take_anchored_image_queue());
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
                    &font_table,
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
                    &font_table,
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
                    &font_table,
                );
            }
        }
    }

    for parsed in &document.stories {
        total_stats.stories += 1;
        let chain = document.frame_chain(&parsed.self_id);
        if chain.is_empty() {
            continue;
        }
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
        let toc_paragraphs: Option<Vec<idml_parse::Paragraph>> = chain
            .first()
            .and_then(|f| f.applied_toc_style.as_deref())
            .and_then(|toc_id| document.styles.toc_styles.get(toc_id))
            .map(|toc| build_toc_paragraphs(document, toc, &page_labels));
        let chain_pages: Vec<usize> = chain
            .iter()
            .map(|f| {
                f.self_id
                    .as_deref()
                    .and_then(|id| frame_to_page.get(id).copied())
                    .unwrap_or(0)
            })
            .collect();
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
        let mut emitter = StoryEmitter::new(
            document,
            options,
            palette,
            cmyk_xform.as_ref(),
            &font_table,
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
        );
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
        anchored_image_queue.extend(emitter.take_anchored_image_queue());
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
            &mut decoded_image_cache,
        );
    }

    total_stats.decoded_images = decoded_image_cache.len();

    Ok(BuiltDocument {
        pages,
        stats: total_stats,
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
    cmyk_xform: Option<&'a idml_color::IccTransform>,
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
    hyphenator: Option<&'a idml_text::Hyphenator>,
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
    head_frame_spread: idml_parse::Bounds,
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
    chain_spread_bounds: Vec<idml_parse::Bounds>,
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
    /// `<StoryPreference OpticalMarginAlignment>` flag. When true,
    /// the per-line emit pass nudges the leftmost / rightmost glyph
    /// of each line outward per `idml_text::optical_margin_offset`.
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
}

/// One image-bearing anchored Rectangle captured during the body /
/// master story pass. The post-pass in `build_document` drains
/// these and routes each through `emit_rectangle_image` with the
/// per-page + decoded caches already in scope.
#[derive(Debug, Clone)]
struct AnchoredImageEmit {
    target_page: usize,
    place_x: f32,
    place_y: f32,
    width: f32,
    height: f32,
    /// Cloned so the post-pass doesn't borrow the source
    /// `AnchoredFrame` (which lives inside the parsed Story tree). We
    /// only need image_link / image_item_transform / self_id for the
    /// rectangle synthesis below, so the clone is cheap.
    af: idml_parse::AnchoredFrame,
}

/// Hard cap on `anchored_recursion_depth`. Real-world IDMLs nest at
/// most 1–2 deep (a sidebar with an inline figure containing a caption
/// frame); 4 leaves headroom while still bounding pathological docs.
const MAX_ANCHORED_STORY_RECURSION: u32 = 4;

impl<'a> StoryEmitter<'a> {
    fn new(
        document: &'a Document,
        options: &'a PipelineOptions<'a>,
        palette: &'a Graphic,
        cmyk_xform: Option<&'a idml_color::IccTransform>,
        font_table: &'a FontTable,
        chain: Vec<&'a TextFrame>,
        chain_pages: Vec<usize>,
        page_labels: &'a [String],
        hyphenator: Option<&'a idml_text::Hyphenator>,
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
        let chain_spread_bounds: Vec<idml_parse::Bounds> = chain
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
            paragraph_cmd_ranges: vec![Vec::new(); len],
            numbered_counter: 0,
            prev_was_numbered: false,
            optical_margin_alignment: false,
            optical_margin_size_pt: 0.0,
            anchored_recursion_depth: 0,
            anchored_image_queue: Vec::new(),
        }
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

    fn emit_paragraph(
        &mut self,
        paragraph: &idml_parse::Paragraph,
        pages: &mut [BuiltPage],
        total_stats: &mut PipelineStats,
    ) {
        emit_paragraph_into_chain(self, paragraph, pages, total_stats);
    }

    fn apply_vertical_justification(&self, pages: &mut [BuiltPage]) {
        for (i, frame) in self.chain.iter().enumerate() {
            let Some((start, end)) = self.frame_cmd_ranges[i] else {
                continue;
            };
            let Some(vj) = frame.vertical_justification else {
                continue;
            };
            if vj == idml_parse::VerticalJustification::Top {
                continue;
            }
            let frame_height_64 =
                (frame.bounds.height() * idml_text::shape::ADVANCE_PRECISION).round() as i32;
            let used_64 = self.frame_max_baseline_64[i];
            let slack_64 = (frame_height_64 - used_64).max(0);
            if vj == idml_parse::VerticalJustification::Justify {
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
                    let dy_pt = dy_64 as f32 / idml_text::shape::ADVANCE_PRECISION;
                    for cmd in &mut cmds[seg_start..seg_end] {
                        cmd.transform_mut().0[5] += dy_pt;
                    }
                }
                continue;
            }
            let dy_64 = match vj {
                idml_parse::VerticalJustification::Center => slack_64 / 2,
                idml_parse::VerticalJustification::Bottom => slack_64,
                _ => 0,
            };
            if dy_64 == 0 {
                continue;
            }
            let dy_pt = dy_64 as f32 / idml_text::shape::ADVANCE_PRECISION;
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
        // Collect (frame_idx, page_idx, start, end, verts) tuples,
        // grouped by page so we can splice in reverse start-order.
        let mut per_page: HashMap<usize, Vec<(usize, usize, usize, Vec<(f32, f32)>)>> =
            HashMap::new();
        for (i, frame) in self.chain.iter().enumerate() {
            let Some((start, end)) = self.frame_cmd_ranges[i] else {
                continue;
            };
            if start == end {
                continue;
            }
            let Some(verts) = frame_polygon_spread(frame) else {
                continue;
            };
            let page_idx = self.chain_pages[i];
            per_page
                .entry(page_idx)
                .or_default()
                .push((i, start, end, verts));
        }
        for (page_idx, mut entries) in per_page {
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            for (frame_idx, start, end, verts) in entries {
                let page = &mut pages[page_idx];
                // Build a closed polygon path from the polyline
                // through the anchors. Coordinates are in spread
                // coords; the clip transform below maps to page
                // coords.
                let mut path = PathData::default();
                if let Some(&(x, y)) = verts.first() {
                    path.segments.push(PathSegment::MoveTo { x, y });
                    for &(x, y) in &verts[1..] {
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
                    idml_compose::DisplayCommand::PopClip(Transform::IDENTITY),
                );
                page.list.commands.insert(
                    start,
                    idml_compose::DisplayCommand::PushClip {
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
            idml_compose::Rect,
            Option<(idml_compose::Rect, idml_compose::BlendMode, f32)>,
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
            let needs_group = !matches!(blend_mode, idml_compose::BlendMode::Normal)
                || matches!(opacity, Some(o) if o < 100.0 - f32::EPSILON);
            let opacity_f = opacity.map(|p| (p / 100.0).clamp(0.0, 1.0)).unwrap_or(1.0);
            let outer = frame_outer_transform(&pages[page_idx], frame.item_transform);
            let inner_rect = idml_compose::Rect {
                x: frame.bounds.left,
                y: frame.bounds.top,
                w: frame.bounds.width(),
                h: frame.bounds.height(),
            };
            let frame_bounds_in_page = rect_bounds_in_page(inner_rect, outer);
            let blend_group = if needs_group {
                let padded = idml_compose::Rect {
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
            let stroke_visible =
                frame_stroke_is_visible(frame.stroke_color.as_deref(), stroke_w);
            let fill_transparent = frame_fill_is_transparent(frame.fill_color.as_deref());
            let glyph_shadow = if !stroke_visible
                && fill_transparent
                && frame.stroke_drop_shadow.is_some()
            {
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
                    let bounds_in_page = idml_compose::Rect {
                        x: frame_bounds_in_page.x - pad,
                        y: frame_bounds_in_page.y - pad,
                        w: frame_bounds_in_page.w + 2.0 * pad,
                        h: frame_bounds_in_page.h + 2.0 * pad,
                    };
                    crate::module::emit_glyph_shadow_pass(
                        page,
                        start..end,
                        shadow,
                        bounds_in_page,
                    )
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
                        idml_compose::DisplayCommand::EndBlendGroup(Transform::IDENTITY),
                    );
                    page.list.commands.insert(
                        glyphs_start,
                        idml_compose::DisplayCommand::BeginBlendGroup {
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

/// Build the synthetic `Paragraph` sequence for an unresolved TOC
/// story. Walks `Document::resolve_toc(toc_style)` and turns every
/// `TOCEntry` into a single `Paragraph` whose:
///   - `paragraph_style` = entry's `format_style`,
///   - one run carrying `text` + expanded `separator` + page label.
///
/// Tabs in `Separator` (IDML serialises a tab as `^t`) expand to a
/// literal `\t`, which `idml_text::layout::apply_tab_stops` snaps
/// to the next tab stop (or, when none, to a single tab width).
/// Page labels come from the per-page `page_labels` slice so
/// Section overrides (Roman numerals etc.) carry through.
///
/// Returns an empty vec when the TOC has no resolved entries —
/// keeps the renderer from emitting any glyphs into the host
/// frame (matches InDesign, which leaves the frame blank).
fn build_toc_paragraphs(
    document: &Document,
    toc_style: &idml_parse::TOCStyleDef,
    page_labels: &[String],
) -> Vec<idml_parse::Paragraph> {
    let entries = document.resolve_toc(toc_style);
    let mut out: Vec<idml_parse::Paragraph> = Vec::with_capacity(entries.len());
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
        let run = idml_parse::CharacterRun {
            text,
            ..idml_parse::CharacterRun::default()
        };
        let paragraph = idml_parse::Paragraph {
            paragraph_style: entry.format_style,
            runs: vec![run],
            ..idml_parse::Paragraph::default()
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
    paragraph: &idml_parse::Paragraph,
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
            resolved_paragraph.space_before.unwrap_or(0.0) * idml_text::shape::ADVANCE_PRECISION;
        let line_height_64 = (para_pt * 1.2 * idml_text::shape::ADVANCE_PRECISION).round() as i32;
        // Establish the first baseline if we haven't placed any
        // content yet — same convention as the populated branch
        // below — then advance by a full line height.
        if em.y_cursor < 0 {
            em.y_cursor = (para_pt * 0.8 * idml_text::shape::ADVANCE_PRECISION).round() as i32;
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
            resolved_paragraph.space_after.unwrap_or(0.0) * idml_text::shape::ADVANCE_PRECISION;
        em.y_cursor += space_after_64.round() as i32;
        return;
    }
    total_stats.paragraphs += 1;
    total_stats.runs += paragraph.runs.len();
    pages[em.chain_pages[em.frame_idx]].stats.paragraphs += 1;
    pages[em.chain_pages[em.frame_idx]].stats.runs += paragraph.runs.len();

    let resolved_runs: Vec<idml_scene::ResolvedRunAttrs> = paragraph
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
    let bytes_font_ids: Vec<u32> = bytes_pool
        .iter()
        .map(|b| fnv_1a_u32(b.as_ref()))
        .collect();
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
        if let Some(cached) = em.font_table.face(bytes_font_ids[head], wghts[head].to_bits()) {
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
    let list_first_text: Option<String> = list_prefix(
        &resolved_paragraph,
        &mut em.numbered_counter,
        &mut em.prev_was_numbered,
    )
    .and_then(|prefix| {
        paragraph
            .runs
            .first()
            .map(|r| format!("{prefix}{}", r.text))
    });

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
        r.text.contains(idml_parse::AUTO_PAGE_NUMBER_MARKER)
            || r.text.contains(idml_parse::NEXT_PAGE_NUMBER_MARKER)
    }) || list_first_text
        .as_deref()
        .is_some_and(|t| t.contains(idml_parse::AUTO_PAGE_NUMBER_MARKER));
    let page_substituted: Vec<String> = if needs_page_subst {
        paragraph
            .runs
            .iter()
            .map(|r| {
                r.text
                    .replace(idml_parse::AUTO_PAGE_NUMBER_MARKER, &current_page_str)
                    .replace(idml_parse::NEXT_PAGE_NUMBER_MARKER, &next_page_str)
            })
            .collect()
    } else {
        Vec::new()
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
                    let src: &str = if needs_page_subst {
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
    let styled_runs: Vec<idml_text::StyledRun> = paragraph
        .runs
        .iter()
        .enumerate()
        .map(|(i, run)| idml_text::StyledRun {
            text: if i == 0 {
                list_first_text.as_deref().unwrap_or_else(|| {
                    if let Some(c) = capitalized[i].as_deref() {
                        c
                    } else if needs_page_subst {
                        page_substituted[i].as_str()
                    } else {
                        &run.text
                    }
                })
            } else if let Some(c) = capitalized[i].as_deref() {
                c
            } else if needs_page_subst {
                page_substituted[i].as_str()
            } else {
                &run.text
            },
            face: shaping_faces[unique_idx[i]].unwrap(),
            point_size: resolved_runs[i]
                .point_size
                .unwrap_or(em.options.default_point_size),
            tracking: resolved_runs[i].tracking,
            font_id: font_ids[i],
            underline: resolved_runs[i].underline.unwrap_or(false),
            strikethru: resolved_runs[i].strikethru.unwrap_or(false),
            baseline_shift_pt: resolved_runs[i].baseline_shift.unwrap_or(0.0),
            horizontal_scale_pct: resolved_runs[i].horizontal_scale.unwrap_or(100.0),
            fallback_faces: &fallback_faces_pool,
        })
        .collect();

    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let Some(col_pt) = em.column_width_pt else {
        return;
    };
    let mut lopts = idml_text::LayoutOptions::new(col_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification);
    apply_paragraph_compose_options(&mut lopts, em.hyphenator, &resolved_paragraph);
    // Explicit `Leading` on the leading run mirrors IDML semantics:
    // every line uses the override regardless of the largest glyph
    // size on the line. Auto leading (no override) keeps existing
    // behaviour.
    if let Some(leading_pt) = resolved_runs.first().and_then(|r| r.leading) {
        if leading_pt > 0.0 {
            lopts.leading_override =
                Some((leading_pt * idml_text::shape::ADVANCE_PRECISION).round() as i32);
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
            resolved_paragraph.space_before.unwrap_or(0.0) * idml_text::shape::ADVANCE_PRECISION;
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
    //      `idml_text::drop_cap_column_widths` for the carved widths.
    //   4. Replace the first styled run's text with the slice past
    //      the drop cap, then run `layout_runs` as normal.
    //   5. After layout, splice the dropped glyphs in at the
    //      paragraph origin.
    let drop_cap_spec_emit: Option<(usize, idml_text::DropCapSpec, idml_text::ShapedRun, f32, u32, ttf_parser::Face<'_>, idml_compose::Paint)> = if paragraph.drop_cap_characters > 0
        && paragraph.drop_cap_lines > 0
        && !styled_runs.is_empty()
        && !styled_runs[0].text.is_empty()
    {
        let body_line_height_pt =
            lopts.line_height as f32 / idml_text::shape::ADVANCE_PRECISION;
        let cap_point_size =
            idml_text::drop_cap_point_size(body_line_height_pt, paragraph.drop_cap_lines);
        // Byte split: take `drop_cap_characters` Unicode scalars
        // off the front of run 0's text. Whitespace counts as a
        // character; IDML's serialisation matches char count not
        // grapheme count.
        let head = styled_runs[0].text;
        let mut split = head.len();
        let mut taken = 0u32;
        for (i, _c) in head.char_indices() {
            if taken == paragraph.drop_cap_characters {
                split = i;
                break;
            }
            taken += 1;
        }
        if split > 0 {
            let dropped_slice = &head[..split];
            let cap_face_idx = unique_idx[0];
            let cap_face_ref = shaping_faces[cap_face_idx].unwrap();
            let cap_shaped = idml_text::shape_run(cap_face_ref, dropped_slice, cap_point_size);
            // Gutter: half the body's space-glyph advance — a small
            // proxy for InDesign's `DropCapDetail` side-bearing.
            let space_shaped =
                idml_text::shape_run(cap_face_ref, " ", styled_runs[0].point_size);
            let gutter_64 = space_shaped.total_advance / 2;
            let spec = idml_text::DropCapSpec {
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
            let max_word_width_64 =
                styled_runs.iter().fold(0i32, |acc, run| {
                    let shaped = idml_text::shape::shape_run(
                        run.face,
                        run.text,
                        run.point_size,
                    );
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
            let carved = idml_text::drop_cap_column_widths_with_min(
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

    // If we have a drop cap, splice the body-run text past the
    // dropped slice. We can't mutate `styled_runs` in place because
    // its `text` field borrows the source string; build a fresh
    // styled_runs vec borrowing from the same source at the new
    // offset.
    let styled_runs_storage: Vec<idml_text::StyledRun>;
    let styled_runs_ref: &[idml_text::StyledRun] = if let Some((split, _, _, _, _, _, _)) =
        &drop_cap_spec_emit
    {
        let mut adjusted: Vec<idml_text::StyledRun> = Vec::with_capacity(styled_runs.len());
        for (i, r) in styled_runs.iter().enumerate() {
            let new_text = if i == 0 { &r.text[*split..] } else { r.text };
            adjusted.push(idml_text::StyledRun {
                text: new_text,
                face: r.face,
                point_size: r.point_size,
                tracking: r.tracking,
                font_id: r.font_id,
                underline: r.underline,
                strikethru: r.strikethru,
                baseline_shift_pt: r.baseline_shift_pt,
                horizontal_scale_pct: r.horizontal_scale_pct,
                fallback_faces: r.fallback_faces,
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

    let mut laid_out = idml_text::layout_runs(styled_runs_ref, &lopts);

    // Optical margin alignment: when the story carries
    // `<StoryPreference OpticalMarginAlignment="true" />`, nudge the
    // leftmost / rightmost glyph of each line outward per
    // `idml_text::optical_margin_offset`. Operates directly on the
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
                let off_pt = idml_text::optical_margin_offset(
                    c,
                    idml_text::MarginSide::Left,
                    first_pt_size,
                ) * scale;
                if off_pt != 0.0 {
                    let off_64 =
                        (off_pt * idml_text::shape::ADVANCE_PRECISION).round() as i32;
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
                let off_pt = idml_text::optical_margin_offset(
                    c,
                    idml_text::MarginSide::Right,
                    last_pt_size,
                ) * scale;
                if off_pt != 0.0 {
                    let off_64 =
                        (off_pt * idml_text::shape::ADVANCE_PRECISION).round() as i32;
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

    // FirstLineIndent shifts the first line's glyphs after
    // breaking — Knuth-Plass can't model a per-line x-shift, so
    // it's a post-layout pass.
    if let Some(indent_pt) = resolved_paragraph.first_line_indent {
        let indent_64 = (indent_pt * idml_text::shape::ADVANCE_PRECISION).round() as i32;
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
        let tab_stops: Vec<idml_text::layout::TabStopSpec> = resolved_paragraph
            .tab_list
            .iter()
            .map(|t| idml_text::layout::TabStopSpec {
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
                leader: t
                    .leader
                    .clone()
                    .filter(|s| !s.is_empty()),
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
            Some(idml_text::layout::LeaderContext::new(styled_runs_ref))
        } else {
            None
        };
        for line in laid_out.lines.iter_mut() {
            idml_text::layout::apply_tab_stops_with_leaders(
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
    let bullet_paint_override: Option<(u32, Paint)> = list_first_text
        .as_deref()
        .and_then(|lft| {
            let bullet_len = lft.len().saturating_sub(
                paragraph.runs.first().map(|r| r.text.len()).unwrap_or(0),
            );
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
        resolved_paragraph.space_after.unwrap_or(0.0) * idml_text::shape::ADVANCE_PRECISION;
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
    let resolve_rule_paint = |r: &idml_parse::ParagraphRule| -> Option<Paint> {
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
    for mut line in laid_out.lines.into_iter() {
        let line_h = idml_text::layout::max_line_height_for_glyphs(&line.glyphs)
            .unwrap_or(lopts.line_height);
        let frame_height_64 = (em.chain[em.frame_idx].bounds.height()
            * idml_text::shape::ADVANCE_PRECISION)
            .round() as i32;
        if line.baseline_y > frame_height_64 && em.frame_idx + 1 < em.chain.len() {
            let prev_baseline = line.baseline_y;
            em.frame_idx += 1;
            let new_baseline =
                (paragraph_size * 0.8 * idml_text::shape::ADVANCE_PRECISION).round() as i32;
            let dy = new_baseline - prev_baseline;
            for g in &mut line.glyphs {
                g.y += dy;
            }
            line.baseline_y = new_baseline;
        }
        // P-13 short-term: when the last frame in the chain overflows
        // (typically because a font substitute is wider than the
        // requested face), drop the overflow lines rather than letting
        // them spill across following frames/pages with no clip. The
        // reference PDFs hide the overflow via the same out-of-frame
        // clip; matching this prevents large ΔE regions.
        if line.baseline_y > frame_height_64 && em.frame_idx + 1 >= em.chain.len() {
            dropped_overflow_lines += 1;
            continue;
        }

        let target_page = em.chain_pages[em.frame_idx];
        pages[target_page].stats.glyphs += line.glyphs.len();
        pages[target_page].stats.lines += 1;
        total_stats.glyphs += line.glyphs.len();
        total_stats.lines += 1;

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
        let line_h_pt_local = line_h as f32 / idml_text::shape::ADVANCE_PRECISION;
        let baseline_pt_local = line.baseline_y as f32 / idml_text::shape::ADVANCE_PRECISION;
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
                let rule_y = text_origin_pt.1 + baseline_pt_local - line_h_pt_local * 0.8
                    - offset;
                let x_left = text_origin_pt.0 + left;
                let x_right = text_origin_pt.0 + col_w_pt - right;
                if x_right > x_left {
                    let rect = idml_compose::Rect {
                        x: x_left,
                        y: rule_y - weight * 0.5,
                        w: x_right - x_left,
                        h: weight,
                    };
                    idml_compose::emit_rect_transformed(
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
            let line_h_pt = line_h as f32 / idml_text::shape::ADVANCE_PRECISION;
            let baseline_pt = line.baseline_y as f32 / idml_text::shape::ADVANCE_PRECISION;
            let col_w_pt = em.column_width_pt.unwrap_or(0.0);
            let y_top = text_origin_pt.1 + baseline_pt - line_h_pt * 0.8
                - shading_offsets[0];
            let y_bot = text_origin_pt.1 + baseline_pt + line_h_pt * 0.2
                + shading_offsets[2];
            let x_left = text_origin_pt.0 + shading_offsets[1];
            let x_right = text_origin_pt.0 + col_w_pt - shading_offsets[3];
            if x_right > x_left && y_bot > y_top {
                let rect = idml_compose::Rect {
                    x: x_left,
                    y: y_top,
                    w: x_right - x_left,
                    h: y_bot - y_top,
                };
                idml_compose::emit_rect_transformed(
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
            rewrite_tail_for_overprint(
                &mut pages[target_page],
                before_cmds,
                op_fill,
                op_stroke,
            );
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
                let rule_y = text_origin_pt.1 + baseline_pt_local + line_h_pt_local * 0.2
                    + offset;
                let x_left = text_origin_pt.0 + left;
                let x_right = text_origin_pt.0 + col_w_pt - right;
                if x_right > x_left {
                    let rect = idml_compose::Rect {
                        x: x_left,
                        y: rule_y - weight * 0.5,
                        w: x_right - x_left,
                        h: weight,
                    };
                    idml_compose::emit_rect_transformed(
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
                let y_top =
                    text_origin_pt.1 + first_baseline - line_h_pt_local * 0.8 - off_top;
                let y_bot =
                    text_origin_pt.1 + baseline_pt_local + line_h_pt_local * 0.2 + off_bottom;
                if x_right > x_left && y_bot > y_top {
                    let radii = per_corner_radii(None, None, &b.corners);
                    let any_rounded = radii.iter().any(|r| r.map(|v| v > 0.0).unwrap_or(false));
                    if any_rounded {
                        let outline_rect = idml_compose::Rect {
                            x: x_left,
                            y: y_top,
                            w: x_right - x_left,
                            h: y_bot - y_top,
                        };
                        let path = rounded_rect_path_per_corner(outline_rect, radii);
                        let path_id = pages[target_page].list.paths.push_anon(path);
                        pages[target_page].list.push(
                            idml_compose::DisplayCommand::StrokePath {
                                path_id,
                                paint,
                                stroke: idml_compose::Stroke::new(weight),
                                transform: Transform::IDENTITY,
                            },
                        );
                    } else {
                        let top = idml_compose::Rect {
                            x: x_left - weight * 0.5,
                            y: y_top - weight * 0.5,
                            w: (x_right - x_left) + weight,
                            h: weight,
                        };
                        let bottom = idml_compose::Rect {
                            x: x_left - weight * 0.5,
                            y: y_bot - weight * 0.5,
                            w: (x_right - x_left) + weight,
                            h: weight,
                        };
                        let left_edge = idml_compose::Rect {
                            x: x_left - weight * 0.5,
                            y: y_top - weight * 0.5,
                            w: weight,
                            h: (y_bot - y_top) + weight,
                        };
                        let right_edge = idml_compose::Rect {
                            x: x_right - weight * 0.5,
                            y: y_top - weight * 0.5,
                            w: weight,
                            h: (y_bot - y_top) + weight,
                        };
                        for r in [top, right_edge, bottom, left_edge] {
                            idml_compose::emit_rect_transformed(
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
            (cap_point_size * 0.8 * idml_text::shape::ADVANCE_PRECISION).round() as i32
        } else {
            let m = paragraph.drop_cap_lines.saturating_sub(1) as i32;
            lopts.first_baseline + m * lopts.line_height
        };
        let mut positioned: Vec<idml_text::PositionedGlyph> = Vec::with_capacity(cap_shaped.glyphs.len());
        let mut pen_x = 0i32;
        for g in &cap_shaped.glyphs {
            positioned.push(idml_text::PositionedGlyph {
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
        emit_anchored_frames_for_paragraph(em, paragraph, pages, total_stats);
    }
}

/// Best-effort emission of the paragraph's anchored frames. Supports
/// `InlinePosition` (default) by placing the frame at the
/// paragraph's first-baseline anchor offset by `anchor_x_offset` /
/// `anchor_y_offset`. `AbovePosition` puts it above the paragraph's
/// origin; `Custom` honours the offsets verbatim. Unrecognised
/// positions log a TODO and fall through to InlinePosition placement.
///
/// Anchored TextFrames recurse into their story via the document's
/// frame_chain lookup; anchored Rectangles emit through
/// `emit_rectangle_into` if the parser surfaced bounds for them. We
/// don't yet thread images on anchored rectangles; those land when
/// the parser surfaces image_link on AnchoredFrame.
fn emit_anchored_frames_for_paragraph(
    em: &mut StoryEmitter,
    paragraph: &idml_parse::Paragraph,
    pages: &mut [BuiltPage],
    _total_stats: &mut PipelineStats,
) {
    let target_page = em.chain_pages[em.frame_idx];
    let frame = em.chain[em.frame_idx];
    let (ox, oy) = pages[target_page].spread_origin;
    let frame_insets = frame.inset_spacing.unwrap_or([0.0; 4]);
    let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
    let para_origin_x = sx - ox + frame_insets[1] + em.column_x_shift_pt;
    let para_origin_y = sy - oy;
    // Paragraph baseline (page-local pt). y_cursor is in 1/64 pt
    // relative to the host frame's inner origin, so convert + add
    // the frame's spread top-left to get a page-local baseline.
    let baseline_y_pt = if em.y_cursor >= 0 {
        para_origin_y + em.y_cursor as f32 / idml_text::shape::ADVANCE_PRECISION
    } else {
        para_origin_y
    };

    for af in &paragraph.anchored_frames {
        let setting = af.setting.as_ref();
        let position = setting
            .and_then(|s| s.anchored_position.as_deref())
            .unwrap_or("InlinePosition");
        let (offset_x, offset_y) = setting
            .map(|s| (s.anchor_x_offset, s.anchor_y_offset))
            .unwrap_or((0.0, 0.0));
        let frame_w = af.bounds.map(|b| b.width()).unwrap_or(0.0);
        let frame_h = af.bounds.map(|b| b.height()).unwrap_or(0.0);
        // Anchor reference point on the frame — the corner / edge the
        // AnchoredObjectSetting offset attaches to (`TopLeftAnchor`,
        // `TopRightAnchor`, `CenterAnchor`, …). For inline frames
        // we resolve the *vertical* component of the anchor point
        // strictly: a Top anchor sits the frame's top on the line
        // baseline, a Bottom anchor sits the frame's bottom on the
        // baseline (the legacy default), Center splits the diff.
        // The horizontal component currently degenerates because we
        // don't yet thread the per-anchor advance offset out of the
        // composer — both `BottomLeftAnchor` and `BottomRightAnchor`
        // place the frame at the column-left edge of the paragraph
        // (real InDesign would shift `BottomRightAnchor` by the
        // anchor character's full advance, which equals the frame's
        // own width when the anchor is the lone character on the
        // line). Once the composer surfaces the U+FFFC advance
        // position the horizontal degenerates collapse — see the
        // TODO below.
        let anchor_point = setting
            .and_then(|s| s.anchor_point.as_deref())
            .unwrap_or("BottomLeftAnchor");
        let vertical_corner_dy = anchor_vertical_corner_offset(anchor_point, frame_h);
        // TODO(anchored-position): once paragraph_breaker exposes
        // the anchor character's advance-from-line-start, replace
        // `para_origin_x` with that advance so `InlinePosition` lands
        // at the actual inline position. The horizontal anchor-corner
        // offset (Left / Center / Right) then becomes meaningful too.
        let (place_x, place_y) = match position {
            "InlinePosition" => {
                if frame_w > 0.0 && frame_h > 0.0 {
                    tracing::debug!(
                        target: "idml_renderer::pipeline",
                        anchor_point,
                        "InlinePosition: anchored at paragraph origin (per-anchor advance offset queued)"
                    );
                }
                // Frame top-left placed so the named anchor corner
                // sits at (paragraph origin x, baseline y) plus the
                // anchor offsets. The horizontal anchor-corner
                // component currently collapses to 0 until
                // paragraph_breaker exposes the per-anchor advance
                // position; the vertical component drives Top vs
                // Bottom anchoring.
                (
                    para_origin_x + offset_x,
                    baseline_y_pt + offset_y - vertical_corner_dy,
                )
            }
            // Both `AbovePosition` and the (newer) `AboveLine` enum
            // value place the frame above the host line; treat them
            // identically until line-by-line vertical resolution lands.
            "AbovePosition" | "AboveLine" => (
                para_origin_x + offset_x,
                para_origin_y + offset_y - vertical_corner_dy,
            ),
            // `Custom` / `Anchored` honour the offsets verbatim from
            // the anchor character's baseline. IDML 14+ uses
            // `Anchored` in place of `Custom`; treat them as synonyms.
            "Custom" | "Anchored" => (
                para_origin_x + offset_x,
                baseline_y_pt + offset_y - vertical_corner_dy,
            ),
            _ => {
                tracing::debug!(
                    target: "idml_renderer::pipeline",
                    position = position,
                    "unrecognised anchored position; defaulting to InlinePosition"
                );
                (
                    para_origin_x + offset_x,
                    baseline_y_pt + offset_y - vertical_corner_dy,
                )
            }
        };
        emit_one_anchored_frame(em, af, target_page, place_x, place_y, pages);
    }
}

/// Vertical offset from an anchored frame's top to the reference
/// edge / center named by `anchor_point`. Returns `0` for any
/// `Top*Anchor` (frame's top at the anchor's y), `h/2` for any
/// `*CenterAnchor`, and `h` for any `Bottom*Anchor` / unknown values
/// (frame's bottom at the anchor's y — the legacy default that
/// matched the original anchored-frame placement).
fn anchor_vertical_corner_offset(anchor_point: &str, h: f32) -> f32 {
    match anchor_point {
        "TopLeftAnchor" | "TopCenterAnchor" | "TopRightAnchor" => 0.0,
        "LeftCenterAnchor" | "CenterAnchor" | "RightCenterAnchor" => h * 0.5,
        // Bottom* and unknown values fall through to the legacy
        // bottom-anchored placement (frame's bottom at the anchor y).
        _ => h,
    }
}

/// Emit a single anchored frame (or recurse through a Group). Splits
/// out of `emit_anchored_frames_for_paragraph` so anchored Groups can
/// reuse the same placement logic for each child without duplicating
/// the position-resolution preamble.
///
/// `place_x` / `place_y` are the page-local pt coordinates of the
/// frame's top-left as resolved from the AnchoredObjectSetting
/// (InlinePosition / AbovePosition / Custom). For Group children, the
/// caller offsets these by the child's bounds delta within the group.
fn emit_one_anchored_frame(
    em: &mut StoryEmitter,
    af: &idml_parse::AnchoredFrame,
    target_page: usize,
    place_x: f32,
    place_y: f32,
    pages: &mut [BuiltPage],
) {
    let frame_w = af.bounds.map(|b| b.width()).unwrap_or(0.0);
    let frame_h = af.bounds.map(|b| b.height()).unwrap_or(0.0);
    match af.frame_kind {
        idml_parse::AnchoredFrameKind::Rectangle
        | idml_parse::AnchoredFrameKind::TextFrame => {
            // Rectangles AND TextFrames render the frame's box +
            // fill / stroke through the same `emit_rectangle_into`
            // pipeline used by spread-level Rectangles. TextFrames
            // additionally host a story; the story-recursion layer
            // is queued (anchored.idml's TextFrame variants ship
            // FillColor=Color/Paper which makes the frame visible
            // even without the inner text). The synthesizer below
            // bakes the page-local placement into a Rectangle whose
            // bounds sit in spread coords so `frame_outer_transform`
            // unwinds back to the right page-local position.
            if frame_w > 0.0 && frame_h > 0.0 {
                emit_anchored_rect_via_pipeline(
                    em,
                    af,
                    target_page,
                    place_x,
                    place_y,
                    frame_w,
                    frame_h,
                    pages,
                );
            }
            // Capture image-bearing anchored Rectangles (incl. Group
            // children). Rendering routes through the per-page +
            // decoded-image caches owned by `build_document`, so
            // we record placement here and the post-pass replays via
            // `emit_rectangle_image`. Anchored TextFrames don't carry
            // an `image_link` (the parser only sets it for Rectangles
            // / Groups), but the guard is symmetric for safety.
            if af.image_link.is_some() && frame_w > 0.0 && frame_h > 0.0 {
                em.anchored_image_queue.push(AnchoredImageEmit {
                    target_page,
                    place_x,
                    place_y,
                    width: frame_w,
                    height: frame_h,
                    af: af.clone(),
                });
            }
            if matches!(af.frame_kind, idml_parse::AnchoredFrameKind::TextFrame) {
                if let Some(story_id) = af.parent_story.as_deref() {
                    if frame_w > 0.0 && frame_h > 0.0 {
                        emit_anchored_textframe_story(
                            em,
                            af,
                            story_id,
                            target_page,
                            place_x,
                            place_y,
                            frame_w,
                            frame_h,
                            pages,
                        );
                    }
                }
            }
        }
        idml_parse::AnchoredFrameKind::Group => {
            // Recurse through the group's children. The group's own
            // ItemTransform (typically a pure translate of the form
            // `[1 0 0 1 tx ty]`) shifts every child by `(tx, ty)` in
            // page-local pt. Each child's `bounds.left` /
            // `bounds.top` are relative to the group's inner-coord
            // origin; we offset by the difference between the
            // child's and the group's `bounds` so the children land
            // at the right spot inside the group's placement rect.
            // Image-link emission for Group children is deferred —
            // the per-page image cache lives outside StoryEmitter.
            let (group_tx, group_ty) = af
                .item_transform
                .map(|m| (m[4], m[5]))
                .unwrap_or((0.0, 0.0));
            let (group_bx, group_by) = af
                .bounds
                .map(|b| (b.left, b.top))
                .unwrap_or((0.0, 0.0));
            for child in &af.children {
                // Child's offset within the group's inner coord
                // system is `child.bounds.{left,top} - group.bounds.{left,top}`.
                // Plus the child's own item_transform (translate
                // component) and the group's item_transform.
                let (child_bx, child_by) = child
                    .bounds
                    .map(|b| (b.left, b.top))
                    .unwrap_or((0.0, 0.0));
                let (child_tx, child_ty) = child
                    .item_transform
                    .map(|m| (m[4], m[5]))
                    .unwrap_or((0.0, 0.0));
                let child_place_x =
                    place_x + group_tx + child_tx + (child_bx - group_bx);
                let child_place_y =
                    place_y + group_ty + child_ty + (child_by - group_by);
                emit_one_anchored_frame(
                    em,
                    child,
                    target_page,
                    child_place_x,
                    child_place_y,
                    pages,
                );
            }
        }
    }
}

/// Synthesize a Rectangle for an anchored frame placed at
/// `(place_x, place_y)` page-local pt with size `(w, h)` and route it
/// through `emit_rectangle_into` so fill / stroke / drop-shadow
/// modules emit identically to a spread-level Rectangle. The
/// synthetic Rectangle's bounds sit in spread coords (page-local +
/// spread_origin) so `frame_outer_transform` produces a translate of
/// `-spread_origin` and lands the geometry back on `(place_x, place_y)`.
fn emit_anchored_rect_via_pipeline(
    em: &StoryEmitter,
    af: &idml_parse::AnchoredFrame,
    target_page: usize,
    place_x: f32,
    place_y: f32,
    w: f32,
    h: f32,
    pages: &mut [BuiltPage],
) {
    let (ox, oy) = pages[target_page].spread_origin;
    let bounds = idml_parse::Bounds {
        top: place_y + oy,
        left: place_x + ox,
        bottom: place_y + oy + h,
        right: place_x + ox + w,
    };
    let synthetic = Rectangle {
        self_id: af.self_id.clone(),
        bounds,
        item_transform: None,
        fill_color: af.fill_color.clone(),
        fill_tint: af.fill_tint,
        stroke_color: af.stroke_color.clone(),
        stroke_weight: af.stroke_weight,
        drop_shadow: None,
        stroke_drop_shadow: None,
        // Image emission for anchored Rectangles is deferred — the
        // per-page image cache lives in the pre-pass scope, outside
        // StoryEmitter. The parser still surfaces image_link /
        // image_item_transform on AnchoredFrame so a future renderer
        // pass can pick them up. Today's anchored.idml ships no
        // image-bearing anchored Rectangles.
        image_link: None,
        image_bytes: None,
        has_image_element: false,
        has_inline_pdf: false,
        image_item_transform: None,
        applied_object_style: af.applied_object_style.clone(),
        text_wrap: None,
        frame_fitting: None,
        stroke_type: None,
        stroke_alignment: None,
        end_cap: None,
        end_join: None,
        miter_limit: None,
        item_layer: None,
        corner_radius: None,
        corner_option: None,
        corners: Default::default(),
        is_anchored: true,
        opacity: None,
        blend_mode: None,
        effects: None,
        gradient_fill_angle: af.gradient_fill_angle,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        text_paths: Vec::new(),
        // Anchored frames don't currently carry overprint attrs in our
        // AnchoredFrame mirror; default to knockout (the IDML default).
        overprint_fill: false,
        overprint_stroke: false,
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
    };
    // `emit_rectangle_into` increments `page.stats.frames` internally.
    emit_rectangle_into(
        &mut pages[target_page],
        &synthetic,
        em.document,
        em.palette,
        em.options.fallback_frame_fill,
        em.cmyk_xform,
        None,
    );
}

/// Image-emit pass for an anchored Rectangle whose `image_link` is
/// populated. Synthesises a Rectangle in spread coords (mirroring
/// `emit_anchored_rect_via_pipeline`'s placement math, plus the
/// image fields the parent helper drops) and hands it to
/// `emit_rectangle_image` so the per-page + decoded-image caches in
/// `build_document`'s scope are reused.
///
/// The image stamps *on top* of the rectangle's own fill / stroke
/// emitted earlier by the body / master story pass — same z-order
/// as a spread-level Rectangle whose `<Image>` child overlays the
/// rectangle's solid fill.
fn emit_anchored_rect_image(
    page: &mut BuiltPage,
    af: &idml_parse::AnchoredFrame,
    place_x: f32,
    place_y: f32,
    w: f32,
    h: f32,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, idml_compose::ImageId>,
    decoded_cache: &mut HashMap<String, idml_compose::DecodedImage>,
) {
    let (ox, oy) = page.spread_origin;
    let bounds = idml_parse::Bounds {
        top: place_y + oy,
        left: place_x + ox,
        bottom: place_y + oy + h,
        right: place_x + ox + w,
    };
    let synthetic = Rectangle {
        self_id: af.self_id.clone(),
        bounds,
        item_transform: None,
        fill_color: af.fill_color.clone(),
        fill_tint: af.fill_tint,
        stroke_color: af.stroke_color.clone(),
        stroke_weight: af.stroke_weight,
        drop_shadow: None,
        stroke_drop_shadow: None,
        image_link: af.image_link.clone(),
        has_image_element: af.image_link.is_some(),
        has_inline_pdf: false,
        image_item_transform: af.image_item_transform,
        image_bytes: None,
        applied_object_style: af.applied_object_style.clone(),
        text_wrap: None,
        frame_fitting: None,
        stroke_type: None,
        stroke_alignment: None,
        end_cap: None,
        end_join: None,
        miter_limit: None,
        item_layer: None,
        corner_radius: None,
        corner_option: None,
        corners: Default::default(),
        is_anchored: true,
        opacity: None,
        blend_mode: None,
        effects: None,
        gradient_fill_angle: af.gradient_fill_angle,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        text_paths: Vec::new(),
        // Anchored frames don't currently carry overprint attrs in our
        // AnchoredFrame mirror; default to knockout (the IDML default).
        overprint_fill: false,
        overprint_stroke: false,
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
    };
    emit_rectangle_image(page, &synthetic, options, page_image_cache, decoded_cache);
}

/// Flow the story referenced by an anchored TextFrame into the
/// placed rectangle. Synthesises a single-frame chain whose `bounds`
/// sit in spread coords (so `frame_outer_transform` produces a
/// `-spread_origin` translate that lands the geometry on
/// `(place_x, place_y)`), then runs the existing per-paragraph emit
/// loop on a fresh sub-`StoryEmitter`. The sub-emitter inherits the
/// parent's document / palette / font_table / cmyk / hyphenator
/// borrows so no extra plumbing is needed.
///
/// Recursion is bounded by [`MAX_ANCHORED_STORY_RECURSION`]: an
/// anchored TextFrame inside an anchored TextFrame is fine, but a
/// pathological cycle (anchored TextFrame whose story re-references
/// itself) is short-circuited with a `tracing::warn!`.
///
/// Inset spacing on the synthetic frame is `[0; 4]` because parsed
/// `AnchoredFrame` records don't carry `<TextFramePreference
/// InsetSpacing>` — anchored frames in real-world IDMLs typically
/// rely on the ObjectStyle cascade for insets, which the renderer's
/// `emit_text_frame_into` pre-pass already drew the box from. The
/// inner story flows edge-to-edge inside the frame's bounds.
fn emit_anchored_textframe_story<'a>(
    em: &mut StoryEmitter<'a>,
    af: &idml_parse::AnchoredFrame,
    story_id: &str,
    target_page: usize,
    place_x: f32,
    place_y: f32,
    w: f32,
    h: f32,
    pages: &mut [BuiltPage],
) {
    if em.anchored_recursion_depth >= MAX_ANCHORED_STORY_RECURSION {
        tracing::warn!(
            target: "idml_renderer::pipeline",
            depth = em.anchored_recursion_depth,
            story_id = story_id,
            "anchored TextFrame recursion depth cap hit; skipping inner story"
        );
        return;
    }
    let Some(parsed) = em
        .document
        .stories
        .iter()
        .find(|s| s.self_id == story_id)
    else {
        return;
    };
    // Build the synthetic TextFrame's bounds in spread coords. The
    // sub-emitter's per-line walk transforms `bounds` through the
    // (None) item_transform and subtracts the page's spread_origin —
    // the shape of `emit_anchored_rect_via_pipeline` for fill /
    // stroke, but here driving the StoryEmitter rather than
    // `emit_rectangle_into`.
    let (ox, oy) = pages[target_page].spread_origin;
    let bounds = idml_parse::Bounds {
        top: place_y + oy,
        left: place_x + ox,
        bottom: place_y + oy + h,
        right: place_x + ox + w,
    };
    let synthetic = TextFrame {
        self_id: af.self_id.clone(),
        parent_story: Some(story_id.to_string()),
        bounds,
        item_transform: None,
        fill_color: None,
        fill_tint: None,
        stroke_color: None,
        stroke_weight: None,
        drop_shadow: None,
        stroke_drop_shadow: None,
        next_text_frame: None,
        vertical_justification: None,
        first_baseline_offset: None,
        minimum_first_baseline_offset: None,
        inset_spacing: None,
        auto_sizing: None,
        auto_sizing_reference_point: None,
        minimum_width_for_auto_sizing: None,
        minimum_height_for_auto_sizing: None,
        use_minimum_height_for_auto_sizing: None,
        applied_object_style: af.applied_object_style.clone(),
        text_wrap: None,
        item_layer: None,
        is_anchored: true,
        opacity: None,
        blend_mode: None,
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
        effects: None,
        gradient_fill_angle: None,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        applied_toc_style: None,
        overprint_fill: false,
        overprint_stroke: false,
    };
    // Sub-emitter borrows from the parent's `'a` so the document /
    // palette / font_table refs share lifetimes with the body pass.
    // The synthetic frame lives on this stack frame; the sub-emitter
    // is dropped before this function returns, so the chain's
    // `&TextFrame` borrow is sound.
    let chain: Vec<&TextFrame> = vec![&synthetic];
    let chain_pages: Vec<usize> = vec![target_page];
    let head_wrap_rects: &[WrapShape] = &[];
    let chain_wrap_rects: Vec<&[WrapShape]> = vec![&[]];
    let mut sub = StoryEmitter::new(
        em.document,
        em.options,
        em.palette,
        em.cmyk_xform,
        em.font_table,
        chain,
        chain_pages,
        em.page_labels,
        em.hyphenator,
        head_wrap_rects,
        chain_wrap_rects,
    )
    .with_optical_margin(
        parsed.story.optical_margin_alignment,
        parsed.story.optical_margin_size,
    )
    .with_anchored_recursion_depth(em.anchored_recursion_depth + 1);
    // The story-pass entry point uses a fresh PipelineStats per call
    // for stat aggregation; we accumulate into a discard local rather
    // than the document-wide `total_stats` because anchored stories
    // already counted into `frames` via the synthetic-rect emission.
    // The user-visible counters that matter (paragraphs / runs /
    // glyphs) get added to the page stats by the body emit functions
    // directly.
    let mut sub_stats = PipelineStats::default();
    for paragraph in &parsed.story.paragraphs {
        sub.emit_paragraph(paragraph, pages, &mut sub_stats);
    }
    sub.apply_vertical_justification(pages);
    sub.apply_blend_groups(pages);
}

/// Wraps a page's bounds for centre-point routing + its master
/// reference for master-spread application + its position in the
/// document so the master pass can read back per-page state
/// (MasterPageTransform).
struct PageGeom {
    bounds_in_spread: idml_parse::Bounds,
    applied_master: Option<String>,
    host_spread_idx: usize,
    local_page_idx: usize,
}

/// Build a [`PathData`] from a polygon's parsed Bezier anchors.
/// Each consecutive pair becomes a cubic with the leading point's
/// `right` and the trailing point's `left` as control points. When
/// `right == anchor` and `left == anchor` (the IDML serialisation
/// for straight-line corners), the cubic degenerates and tiny-skia
/// reduces it to a line internally.
///
/// `subpath_starts` carries one entry per `<GeometryPathType>` in
/// the source IDML so compound paths (square-with-hole etc.) emit
/// distinct `MoveTo`/`Close` sequences rather than connecting the
/// inner contour to the outer one with a stray segment. An empty
/// or single-entry slice means "single contour" — the legacy path.
fn polygon_path_from_anchors(anchors: &[PathAnchor], subpath_starts: &[usize]) -> PathData {
    polygon_path_from_anchors_with_open(anchors, subpath_starts, &[])
}

/// Same as `polygon_path_from_anchors` but consults a parallel
/// `subpath_open` slice. An open contour skips the closing CubicTo +
/// Close so a hand-drawn lassoed stroke or a `PathOpen="true"` clip
/// path doesn't get auto-filled (P-15). `subpath_open` is interpreted
/// against the indexed order of contours (the `i`th true ⇒ `i`th
/// contour open); a shorter slice / empty slice means every contour
/// is closed (legacy behaviour).
fn polygon_path_from_anchors_with_open(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
) -> PathData {
    if anchors.is_empty() {
        return PathData {
            segments: Vec::new(),
        };
    }
    // Materialise subpath ranges. Default ([] or [0]) = one contour
    // covering the whole anchor list. Otherwise each entry begins a
    // new contour at that index, ending where the next one starts
    // (or at `anchors.len()` for the last entry). Out-of-range and
    // duplicate offsets are filtered defensively — every contour
    // gets at least one anchor or is dropped.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    if subpath_starts.len() <= 1 {
        ranges.push((0, anchors.len()));
    } else {
        let mut starts: Vec<usize> = subpath_starts
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
    let mut segs = Vec::with_capacity(anchors.len() * 2 + ranges.len() * 2);
    for (range_idx, (lo, hi)) in ranges.iter().copied().enumerate() {
        let sub = &anchors[lo..hi];
        if sub.is_empty() {
            continue;
        }
        let is_open = subpath_open.get(range_idx).copied().unwrap_or(false);
        let (mx, my) = sub[0].anchor;
        segs.push(PathSegment::MoveTo { x: mx, y: my });
        for window in sub.windows(2) {
            let from = &window[0];
            let to = &window[1];
            segs.push(PathSegment::CubicTo {
                cx1: from.right.0,
                cy1: from.right.1,
                cx2: to.left.0,
                cy2: to.left.1,
                x: to.anchor.0,
                y: to.anchor.1,
            });
        }
        // Close the path back to the first anchor through the curve
        // implied by the last point's `right` and the first point's
        // `left` — IDML polygons are otherwise always closed. Single-
        // anchor contours degenerate to a point and skip the closer.
        // Open contours skip the closing curve + Close so the path
        // stays open (P-15).
        if !is_open && sub.len() >= 2 {
            let last = sub.last().unwrap();
            let first = &sub[0];
            segs.push(PathSegment::CubicTo {
                cx1: last.right.0,
                cy1: last.right.1,
                cx2: first.left.0,
                cy2: first.left.1,
                x: first.anchor.0,
                y: first.anchor.1,
            });
        }
        if !is_open {
            segs.push(PathSegment::Close);
        }
    }
    PathData { segments: segs }
}

/// Polygon emit. When the polygon carries `<PathPointType>` anchors
/// (real-world InDesign export shape) we build a curved FillPath
/// from them; otherwise fall back to drawing the AABB so synthetic
/// IDMLs that declare a polygon via `GeometricBounds` still render.
fn emit_polygon_into(
    page: &mut BuiltPage,
    poly: &Polygon,
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    let mut resolved = ResolvedFrame::from_polygon(poly);
    let style = crate::module::resolve_applied_style(&resolved, document);
    if let Some(s) = &style {
        crate::module::object_style_cascade(&mut resolved, s);
    }
    page.stats.frames += 1;
    let outer = frame_outer_transform(page, resolved.item_transform);
    let needs_group = frame_needs_blend_group(&resolved);
    if needs_group {
        let bbox = match &resolved.geometry {
            Geometry::Polygon { bbox, .. } => *bbox,
            Geometry::Rect { rect } => *rect,
            _ => idml_compose::Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
        };
        push_blend_group(
            page,
            bbox,
            outer,
            resolved.blend_mode,
            frame_group_opacity(&resolved),
        );
    }
    // Intern the polygon's path up-front so fill/stroke modules can
    // route through `FillPath{Blend}` / `StrokePath` rather than the
    // unit-rect/ellipse primitives. The adapter collapsed anchor-
    // less polygons into `Geometry::Rect` already, so this only fires
    // for the curved-path case.
    let path_id = if let Geometry::Polygon {
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
    // Q-04: Polygon frame effects (GradientFeather, OuterGlow, etc.)
    // ride the interned polygon path. The path is already in inner-
    // coord space and `outer` carries the frame's ItemTransform plus
    // the page-origin shift, so `effects_unit_normalize = None` (the
    // effects module reads coordinates from the path directly).
    if let (Some(pid), Some(effects)) = (path_id, poly.effects.as_ref()) {
        crate::module::emit_effects_pre_fill(
            page, effects, pid, outer, palette, cmyk_xform,
        );
    }
    crate::module::fill_paint_module(
        &resolved, page, palette, cmyk_xform, fallback, outer, path_id,
    );
    if let (Some(pid), Some(effects)) = (path_id, poly.effects.as_ref()) {
        crate::module::emit_effects_post_fill(
            page, effects, pid, outer, palette, cmyk_xform, None,
        );
    }
    crate::module::stroke_paint_module(
        &resolved,
        page,
        palette,
        cmyk_xform,
        outer,
        path_id,
        Stroke::new(resolved.effective_stroke_weight()),
    );
    if needs_group {
        pop_blend_group(page);
    }
}

/// One sample of a host shape's path in inner coords. `cum` is the
/// cumulative arc length from the path's start (point 0) to this
/// sample, in pt. Built once per emit and indexed binary-search style
/// by `sample_path_at`.
#[derive(Debug, Clone, Copy)]
struct PathSample {
    x: f32,
    y: f32,
    cum: f32,
}

/// Tessellate an IDML path (anchors + Bezier control points) into a
/// dense polyline, sampling each cubic at `samples_per_segment`
/// points so curved paths get a smooth approximation.
///
/// Open paths (GraphicLine / open Polygon) only walk anchor pairs; we
/// don't synthesise a closing segment because a TextPath's text
/// flows from the open path's start to its end. Closed polygons
/// (the manual-sample arch) carry the closing curve in their
/// last→first anchor pair already, so we still tessellate it.
fn tessellate_anchors(anchors: &[PathAnchor], samples_per_segment: u32) -> Vec<PathSample> {
    if anchors.is_empty() {
        return Vec::new();
    }
    let n = samples_per_segment.max(1);
    let mut samples: Vec<PathSample> = Vec::with_capacity(anchors.len() * n as usize + 1);
    let (x0, y0) = anchors[0].anchor;
    samples.push(PathSample {
        x: x0,
        y: y0,
        cum: 0.0,
    });
    let mut cum = 0.0f32;
    for window in anchors.windows(2) {
        let from = &window[0];
        let to = &window[1];
        let (p0x, p0y) = from.anchor;
        let (c1x, c1y) = from.right;
        let (c2x, c2y) = to.left;
        let (p1x, p1y) = to.anchor;
        let mut prev_x = p0x;
        let mut prev_y = p0y;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            // Cubic Bezier evaluation. When the control points
            // collapse onto the anchors (the common straight-line
            // case), this reduces exactly to a linear interpolation
            // — degenerate but correct.
            let x = mt * mt * mt * p0x
                + 3.0 * mt * mt * t * c1x
                + 3.0 * mt * t * t * c2x
                + t * t * t * p1x;
            let y = mt * mt * mt * p0y
                + 3.0 * mt * mt * t * c1y
                + 3.0 * mt * t * t * c2y
                + t * t * t * p1y;
            let dx = x - prev_x;
            let dy = y - prev_y;
            cum += (dx * dx + dy * dy).sqrt();
            samples.push(PathSample { x, y, cum });
            prev_x = x;
            prev_y = y;
        }
    }
    samples
}

/// Find the sample whose cumulative arc length brackets `s`, then
/// linearly interpolate to get `(x, y)` plus the local tangent angle
/// in radians (atan2 of the segment direction). Out-of-range `s`
/// clamps to the nearest endpoint so glyphs that overflow the path
/// pile up at the end rather than disappearing.
fn sample_path_at(samples: &[PathSample], s: f32) -> Option<(f32, f32, f32)> {
    if samples.len() < 2 {
        return None;
    }
    if s <= samples[0].cum {
        let dx = samples[1].x - samples[0].x;
        let dy = samples[1].y - samples[0].y;
        return Some((samples[0].x, samples[0].y, dy.atan2(dx)));
    }
    let last = samples.last().unwrap();
    if s >= last.cum {
        let n = samples.len();
        let dx = samples[n - 1].x - samples[n - 2].x;
        let dy = samples[n - 1].y - samples[n - 2].y;
        return Some((last.x, last.y, dy.atan2(dx)));
    }
    // Binary search for the segment containing `s`. Each window pair
    // is monotonically increasing in `cum` by construction.
    let mut lo = 0usize;
    let mut hi = samples.len() - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if samples[mid].cum <= s {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let a = &samples[lo];
    let b = &samples[hi];
    let span = (b.cum - a.cum).max(1e-6);
    let t = ((s - a.cum) / span).clamp(0.0, 1.0);
    let x = a.x + t * (b.x - a.x);
    let y = a.y + t * (b.y - a.y);
    let angle = (b.y - a.y).atan2(b.x - a.x);
    Some((x, y, angle))
}

/// Emit the glyphs for a `<TextPath>` along the host shape's
/// tessellated curve. Approximates IDML's text-on-path:
///
///   - Concatenates every paragraph's runs into a single styled
///     string and shapes them with rustybuzz, exactly like the body
///     text path. Per-paragraph styles (alignment, leading, tabs)
///     are intentionally ignored — text-on-path is a single
///     baseline, not a multi-line column.
///   - Walks the shape's polyline by cumulative arc length: for
///     each glyph the cursor advances by the glyph's `x_advance` and
///     we look up `(x, y, angle)` at the cursor's midpoint. The
///     glyph is then emitted with a per-glyph rotated transform.
///   - Honours the `flip_path_effect` attribute: `Flipped` reverses
///     the path direction so text reads from end-to-start.
///
/// Path-effect modes (`RainbowPathEffect` / `SkewPathEffect` /
/// `Path3DRibbonEffect` / `StairStepPathEffect` / `GravityPathEffect`)
/// are all rendered as plain rainbow today. The first three look the
/// same on a gentle arch like manual-sample's polygon; the latter
/// two need a per-glyph projection that lands later.
fn emit_text_path_into(
    page: &mut BuiltPage,
    text_path: &TextPath,
    anchors: &[PathAnchor],
    item_transform: Option<[f32; 6]>,
    document: &Document,
    options: &PipelineOptions,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    font_table: &FontTable,
) {
    if anchors.len() < 2 {
        return;
    }
    let Some(parsed_story) = document
        .stories
        .iter()
        .find(|s| s.self_id == text_path.parent_story)
    else {
        return;
    };

    // Build the shape's polyline in inner coords.
    let mut samples = tessellate_anchors(anchors, 8);
    if samples.len() < 2 {
        return;
    }
    // Honour FlipPathEffect by reversing the polyline. Cumulative
    // distances must be recomputed so binary search still works.
    if text_path.flip_path_effect.as_deref() == Some("Flipped") {
        samples.reverse();
        let mut cum = 0.0f32;
        for i in 0..samples.len() {
            if i > 0 {
                let dx = samples[i].x - samples[i - 1].x;
                let dy = samples[i].y - samples[i - 1].y;
                cum += (dx * dx + dy * dy).sqrt();
            }
            samples[i].cum = cum;
        }
    }
    let total_len = samples.last().map(|s| s.cum).unwrap_or(0.0);
    if total_len <= 0.0 {
        return;
    }

    // Resolve every paragraph's runs into face + size + paint. We
    // shape each run separately and concatenate the resulting glyphs;
    // line-breaking and column flow don't apply to text-on-path so
    // the simpler per-run shape suffices.
    struct PathGlyph {
        glyph_id: u32,
        x_advance_64: i32,
        y_offset_64: i32,
        x_offset_64: i32,
        face_idx: usize,
        point_size: f32,
        paint: Paint,
    }
    let mut glyphs: Vec<PathGlyph> = Vec::new();
    // Faces are indexed; outline + font_id parallel arrays.
    let mut face_bytes: Vec<Bytes> = Vec::new();
    let mut face_font_ids: Vec<u32> = Vec::new();

    let find_or_push_face = |bytes: &Bytes,
                              face_bytes: &mut Vec<Bytes>,
                              face_font_ids: &mut Vec<u32>|
     -> usize {
        if let Some(i) = face_bytes
            .iter()
            .position(|b| b.as_ptr() == bytes.as_ptr())
        {
            return i;
        }
        face_bytes.push(bytes.clone());
        face_font_ids.push(fnv_1a_u32(bytes.as_ref()));
        face_bytes.len() - 1
    };

    let default_paint = options.fallback_text_paint;
    for paragraph in &parsed_story.story.paragraphs {
        for run in &paragraph.runs {
            if run.text.is_empty() {
                continue;
            }
            let resolved = document.resolved_run_attrs(paragraph, run);
            // Try the FontTable cache first (built from
            // resolver-resolved (family, style) keys). If that misses
            // — typically because the run's font resolves only via
            // the BasedOn chain and the chain's id form differs from
            // the cache key — fall back to the resolver's
            // `default_font` directly. Without this the text-on-path
            // would silently emit zero glyphs whenever the host
            // story's runs lack a directly-set `AppliedFont`.
            let face_bytes_b = font_table
                .bytes_for(resolved.font.as_deref(), resolved.font_style.as_deref())
                .or_else(|| {
                    options.assets.and_then(|r| {
                        r.resolve_font(
                            resolved.font.as_deref().unwrap_or(""),
                            resolved.font_style.as_deref(),
                        )
                    })
                });
            let Some(face_bytes_b) = face_bytes_b else {
                continue;
            };
            let face_idx = find_or_push_face(&face_bytes_b, &mut face_bytes, &mut face_font_ids);
            let point_size = resolved
                .point_size
                .unwrap_or(options.default_point_size);
            let paint = resolved
                .fill_color
                .as_deref()
                .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
                .map(|p| apply_fill_tint(p, resolved.fill_tint))
                .unwrap_or(default_paint);
            // Pull the pre-configured (wght-baked) Face from the
            // FontTable cache when possible; build on the fly only
            // on a miss (e.g. a run whose bytes resolved through the
            // fallback path that `harvest_face_keys` didn't see).
            let font_id = fnv_1a_u32(face_bytes_b.as_ref());
            let wght_bits = wght_for_font_style(resolved.font_style.as_deref()).to_bits();
            let owned_face: Option<rustybuzz::Face> = if font_table.face(font_id, wght_bits).is_none() {
                let Some(mut rf) = rustybuzz::Face::from_slice(face_bytes_b.as_ref(), 0) else {
                    continue;
                };
                let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
                let has_wght_axis = rf
                    .variation_axes()
                    .into_iter()
                    .any(|axis| axis.tag == wght_tag);
                if has_wght_axis {
                    rf.set_variations(&[rustybuzz::Variation {
                        tag: wght_tag,
                        value: f32::from_bits(wght_bits),
                    }]);
                }
                Some(rf)
            } else {
                None
            };
            let rb_face: &rustybuzz::Face = match font_table.face(font_id, wght_bits) {
                Some(f) => f,
                None => owned_face.as_ref().unwrap(),
            };
            let mut shaped = idml_text::shape::shape_run(rb_face, &run.text, point_size);
            if let Some(t) = resolved.tracking {
                idml_text::shape::apply_tracking(&mut shaped, t, point_size);
            }
            for g in &shaped.glyphs {
                glyphs.push(PathGlyph {
                    glyph_id: g.glyph_id,
                    x_advance_64: g.x_advance,
                    y_offset_64: g.y_offset,
                    x_offset_64: g.x_offset,
                    face_idx,
                    point_size,
                    paint,
                });
            }
        }
    }
    if glyphs.is_empty() {
        return;
    }

    // Build outliners for every face we ended up using. Parallel to
    // `face_bytes` / `face_font_ids` so per-glyph emit can index in
    // O(1).
    let mut outline_faces: Vec<Option<ttf_parser::Face>> = Vec::with_capacity(face_bytes.len());
    for b in &face_bytes {
        outline_faces.push(ttf_parser::Face::parse(b.as_ref(), 0).ok());
    }

    // Total text width in pt (advance precision is 1/64).
    let total_advance_pt: f32 = glyphs
        .iter()
        .map(|g| g.x_advance_64 as f32 / idml_text::shape::ADVANCE_PRECISION)
        .sum();

    // IDML `StartBracket` / `EndBracket` define the arc-length range
    // over which the text flows; outside this range the path is
    // visible but the text doesn't draw. Clamp to the tessellated
    // path so a bogus bracket doesn't shoot glyphs off the end.
    let start_b = text_path.start_bracket.unwrap_or(0.0).clamp(0.0, total_len);
    let end_b = text_path
        .end_bracket
        .unwrap_or(total_len)
        .clamp(start_b, total_len);
    let usable_len = (end_b - start_b).max(0.0);

    // Center the text along the path: matches IDML's default
    // `CenterPathAlignment`. Other alignments fall back to centered
    // for now. Overflowing text (advance > usable_len) starts at
    // `start_b` and runs off the end.
    let start_offset_pt = if total_advance_pt < usable_len {
        start_b + ((usable_len - total_advance_pt) * 0.5)
    } else {
        start_b
    };

    // Outer transform: page origin · ItemTransform. Same composition
    // as every other shape — keeps text-on-path inside the host
    // shape's coordinate system without re-implementing the math.
    let outer = frame_outer_transform(page, item_transform);

    let mut cursor_pt = start_offset_pt;
    for g in &glyphs {
        let advance_pt = g.x_advance_64 as f32 / idml_text::shape::ADVANCE_PRECISION;
        let x_off_pt = g.x_offset_64 as f32 / idml_text::shape::ADVANCE_PRECISION;
        let y_off_pt = g.y_offset_64 as f32 / idml_text::shape::ADVANCE_PRECISION;
        // Place the glyph's baseline-left at the cursor's current
        // arc length (plus its shaping x_offset). The local tangent
        // at that point gives the glyph's rotation. Each glyph
        // advances the cursor by its own advance.
        let s = cursor_pt + x_off_pt;
        cursor_pt += advance_pt;
        let Some((px, py, angle)) = sample_path_at(&samples, s) else {
            continue;
        };
        let Some(outline) = outline_faces[g.face_idx].as_ref() else {
            continue;
        };
        let outliner = TtfOutliner::new(outline);
        let upem = outliner.units_per_em();
        let scale = g.point_size / upem;
        let Some(path_id) = list_get_or_intern_glyph_outline(
            face_font_ids[g.face_idx],
            g.glyph_id,
            &outliner,
            &mut page.list,
        ) else {
            continue;
        };
        // Final 2×3 transform = outer · T_path · R · T_local · S(scale,-scale)
        // where:
        //   S(scale, -scale) maps font-units → pt and flips y (font
        //                    space is y-up, page space y-down).
        //   T_local(0, y_off) carries the glyph's per-shape vertical
        //                    offset.
        //   R(angle)         rotates by the path tangent at `s`.
        //   T_path(px, py)   places the rotated glyph at the path
        //                    sample.
        // Glyph (0, 0) (baseline-left in font space) lands at (px,py).
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        let r = [cos_a, sin_a, -sin_a, cos_a];
        let s_diag = [scale, 0.0, 0.0, -scale];
        // After R · T_local: matrix [r0 r2 r0*tx+r2*ty; r1 r3 r1*tx+r3*ty].
        // local_tx/ty: x_offset already baked into `s` so only y_off
        // applies here.
        let local_tx = 0.0;
        let local_ty = y_off_pt;
        let rtl_tx = r[0] * local_tx + r[2] * local_ty;
        let rtl_ty = r[1] * local_tx + r[3] * local_ty;
        // (R · T_local) · S(scale, -scale): scales the columns.
        let rs_a = r[0] * s_diag[0] + r[2] * s_diag[1];
        let rs_b = r[1] * s_diag[0] + r[3] * s_diag[1];
        let rs_c = r[0] * s_diag[2] + r[2] * s_diag[3];
        let rs_d = r[1] * s_diag[2] + r[3] * s_diag[3];
        let inner = Transform([rs_a, rs_b, rs_c, rs_d, rtl_tx + px, rtl_ty + py]);
        let final_xf = outer.compose(&inner);
        page.list.push(idml_compose::DisplayCommand::FillPath {
            path_id,
            paint: g.paint,
            transform: final_xf,
        });
        page.stats.glyphs += 1;
    }
}

/// Local mirror of `idml_compose::text::get_or_intern_glyph_outline`,
/// which is private. Same caching key (font_id × glyph_id) so glyphs
/// emitted via the body-text path and the text-on-path path share
/// outlines.
fn list_get_or_intern_glyph_outline<O: GlyphOutliner>(
    font_id: u32,
    glyph_id: u32,
    outliner: &O,
    list: &mut DisplayList,
) -> Option<idml_compose::PathId> {
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
fn path_signature(anchors: &[PathAnchor]) -> u64 {
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
fn transform_bounds(b: idml_parse::Bounds, m: Option<[f32; 6]>) -> idml_parse::Bounds {
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
    idml_parse::Bounds {
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
    bounds: idml_parse::Bounds,
    corners: [(f32, f32); 4],
}

impl WrapShape {
    /// Build from an inner-coord `Bounds`, an optional ItemTransform,
    /// and per-side wrap offsets `[top, left, bottom, right]`. The
    /// offsets inflate the unrotated source rect *before* the
    /// transform applies so the polygon stays aligned with the host's
    /// rotation (offset is in inner-coord points, same as InDesign).
    fn from_inner(
        b: idml_parse::Bounds,
        m: Option<[f32; 6]>,
        offsets: [f32; 4],
    ) -> Self {
        let inner = idml_parse::Bounds {
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
        let bounds = idml_parse::Bounds {
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
fn compose_outer_translation(inner: Option<[f32; 6]>, dx: f32, dy: f32) -> [f32; 6] {
    match inner {
        Some([a, b, c, d, e, f]) => [a, b, c, d, e + dx, f + dy],
        None => [1.0, 0.0, 0.0, 1.0, dx, dy],
    }
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
) -> Vec<Vec<WrapShape>> {
    let total_pages: usize = spread_page_ranges.last().map(|r| r.end).unwrap_or(0);
    let mut out: Vec<Vec<WrapShape>> = (0..total_pages).map(|_| Vec::new()).collect();
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let range = spread_page_ranges[spread_idx].clone();
        if range.is_empty() {
            continue;
        }
        // Local page bounds for centroid containment routing.
        let page_bounds: Vec<idml_parse::Bounds> = parsed
            .spread
            .pages
            .iter()
            .map(|p| transform_bounds(p.bounds, p.item_transform))
            .collect();
        let route = |aabb: idml_parse::Bounds| -> Option<usize> {
            let cx = (aabb.left + aabb.right) * 0.5;
            let cy = (aabb.top + aabb.bottom) * 0.5;
            page_bounds
                .iter()
                .position(|b| cx >= b.left && cx <= b.right && cy >= b.top && cy <= b.bottom)
        };
        let push = |out: &mut Vec<Vec<WrapShape>>,
                    inner_bounds: idml_parse::Bounds,
                    item_transform: Option<[f32; 6]>,
                    wrap: idml_parse::TextWrap| {
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
                idml_parse::TextWrapMode::BoundingBoxTextWrap
                    | idml_parse::TextWrapMode::ContourTextWrap
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
                push(&mut out, f.bounds, f.item_transform, w);
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

/// Lay out and emit a `<Table>` at the StoryEmitter's current
/// cursor in the head frame. Treats every cell as a mini-frame:
/// computes its rect from cumulative row heights + column widths,
/// then routes each cell paragraph through `emit_cell_paragraph`
/// which does a self-contained shape → layout → emit at a fixed
/// origin and column width.
///
/// Scope:
/// * Honours per-row `SingleRowHeight`, `MinimumHeight`,
///   `MaximumHeight` (Task T3.2) and per-column `SingleColumnWidth`.
///   Cells with `RowSpan > 1` or `ColumnSpan > 1` widen / lengthen
///   their rect; multi-cell text merging across spans isn't
///   separately modelled.
/// * Cells with content overflow grow their row up to
///   `MaximumHeight` — a top-down pre-measure pass computes per-row
///   required heights, then `row_heights[r] =
///   max(SingleRowHeight, MinimumHeight, max_cell_required) ` clamped
///   to `MaximumHeight`. For RowSpan > 1 cells the constraint is
///   applied to the LAST spanned row only (simpler heuristic; the
///   common case has spans inside header rows that don't grow).
/// * Header rows duplicate at the top of every continuation frame
///   when the table breaks across a NextTextFrame chain; footer
///   rows duplicate at the bottom of every frame except the last
///   (Task T3.1). `RepeatingHeader="false"` / `RepeatingFooter="false"`
///   opt out.
// Range-loops over `row_heights` carry the row index (`r`) as data
// — it doubles as a template index into `table.rows` / `table.cells`
// — so the `needless_range_loop` lint is a false positive here.
#[allow(clippy::needless_range_loop)]
fn emit_table_into_chain(
    em: &mut StoryEmitter,
    table: &idml_parse::Table,
    pages: &mut [BuiltPage],
    total_stats: &mut PipelineStats,
) {
    if table.cells.is_empty() {
        return;
    }
    let col_widths: Vec<f32> = table
        .columns
        .iter()
        .map(|c| c.single_column_width.unwrap_or(0.0))
        .collect();
    let mut col_x: Vec<f32> = Vec::with_capacity(col_widths.len() + 1);
    let mut acc = 0.0f32;
    col_x.push(0.0);
    for w in &col_widths {
        acc += *w;
        col_x.push(acc);
    }
    let total_w = col_x.last().copied().unwrap_or(0.0);

    let resolved_table = table
        .applied_table_style
        .as_deref()
        .map(|id| em.document.styles.resolve_table(id))
        .unwrap_or_default();
    let header_count = table.header_row_count as usize;
    let footer_count = table.footer_row_count as usize;
    let total_rows = table.rows.len();
    let total_cols = col_widths.len();

    // Content-driven row growth. For each row, find the tallest
    // required cell height (sum of per-paragraph consumed heights
    // + top/bottom insets). For span > 1 cells, only the LAST row
    // of the span enforces the shortfall — earlier rows in the
    // span are left at their declared height. This is the simpler
    // heuristic the plan calls out; a smarter distributor would
    // share the slack across the span proportionally.
    //
    // Final height per row =
    //   max(SingleRowHeight, MinimumHeight, content_required)
    // clamped to MaximumHeight (when set; unbounded otherwise).
    let mut row_heights: Vec<f32> = table
        .rows
        .iter()
        .map(|r| {
            r.single_row_height
                .unwrap_or(0.0)
                .max(r.minimum_height.unwrap_or(0.0))
        })
        .collect();
    // Per-cell pre-measured content height, keyed by the cell's
    // starting (col, row) — independent of where the cell lands
    // geometrically. Used both for the row-growth pass and to skip
    // re-laying-out the same cell during emission.
    let mut cell_required: std::collections::HashMap<(u32, u32), f32> =
        std::collections::HashMap::with_capacity(table.cells.len());
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else { continue };
        let (cu, ru) = (c as usize, r as usize);
        if cu >= col_widths.len() || ru >= total_rows {
            continue;
        }
        let span_cols = cell.column_span.max(1) as usize;
        let last_c = (cu + span_cols).min(col_widths.len());
        let inner_w = (col_x[last_c]
            - col_x[cu]
            - cell.text_left_inset
            - cell.text_right_inset)
            .max(0.0);
        let mut paragraph_y = 0.0f32;
        for paragraph in &cell.paragraphs {
            if paragraph.runs.is_empty() {
                continue;
            }
            paragraph_y += measure_cell_paragraph(em, paragraph, inner_w);
        }
        let required = paragraph_y + cell.text_top_inset + cell.text_bottom_inset;
        cell_required.insert((c, r), required);
    }
    // Walk rows top-to-bottom; for each row grow it to fit cells
    // that *end* in this row (span_rows + start_row - 1 == r).
    // We iterate by ending-row, look at all cells with that ending,
    // and bump `row_heights[r]` to cover any shortfall remaining
    // after the prior rows of the span. This way RowSpan > 1
    // cells don't blow up multiple rows.
    for r in 0..total_rows {
        let mut required = row_heights[r];
        for cell in &table.cells {
            let Some((c, sr)) = cell.coords() else { continue };
            let span = cell.row_span.max(1) as usize;
            let (cu, sru) = (c as usize, sr as usize);
            if sru + span - 1 != r {
                continue;
            }
            if cu >= col_widths.len() {
                continue;
            }
            let Some(cell_h) = cell_required.get(&(c, sr)).copied() else {
                continue;
            };
            // Heights already grown for the prior rows of the span.
            let prior: f32 = (sru..r).map(|i| row_heights[i]).sum();
            let shortfall = cell_h - prior;
            if shortfall > required {
                required = shortfall;
            }
        }
        let max_h = table
            .rows
            .get(r)
            .and_then(|tr| tr.maximum_height)
            .unwrap_or(f32::INFINITY);
        row_heights[r] = required.min(max_h);
    }

    let region_cell_style_for = |c: usize, r: usize| -> Option<&str> {
        if r < header_count {
            return resolved_table.header_region_cell_style.as_deref();
        }
        if footer_count > 0 && r + footer_count >= total_rows {
            return resolved_table.footer_region_cell_style.as_deref();
        }
        if c == 0 {
            if let Some(s) = resolved_table.left_column_region_cell_style.as_deref() {
                return Some(s);
            }
        }
        if c + 1 == total_cols {
            if let Some(s) = resolved_table.right_column_region_cell_style.as_deref() {
                return Some(s);
            }
        }
        resolved_table.body_region_cell_style.as_deref()
    };

    // Repeating-header / repeating-footer flags. IDML defaults
    // both to true (the attribute is absent in the common case
    // and the rows *do* repeat); explicit `RepeatingHeader="false"`
    // / `RepeatingFooter="false"` opt out.
    let repeating_header = table.repeating_header.unwrap_or(true) && header_count > 0;
    let repeating_footer = table.repeating_footer.unwrap_or(true) && footer_count > 0;

    // Per-row layout basis: which chain frame the row lives in,
    // page-local row-top y, AND which template row in `table.rows`
    // sources the cells / heights for this row. Body rows have
    // `template_idx == phys_idx_in_source`; header/footer replays
    // reuse a template row's index while sitting at a different
    // geometric position.
    #[derive(Clone, Copy, Debug)]
    #[allow(dead_code)]
    enum RowKind {
        /// Body / header / footer row from the original sequence,
        /// emitted once.
        Original,
        /// Replayed header row at the top of a continuation frame.
        HeaderReplay,
        /// Replayed footer row at the bottom of a non-last frame.
        FooterReplay,
    }
    #[derive(Clone, Copy)]
    struct PhysicalRow {
        /// Index into `table.rows` whose cells / height this
        /// physical row mirrors. Cells look up `table.cells` by
        /// `(col, template_idx)`.
        template_idx: usize,
        height: f32,
        chain_idx: usize,
        target_page: usize,
        table_left_pt: f32,
        /// Page-local y for the top of THIS row.
        row_top_in_page: f32,
        /// Kept for debugging / future per-kind hooks (e.g. when
        /// header replays want a different divider style than the
        /// original header dividers). Not read by current emission.
        #[allow(dead_code)]
        kind: RowKind,
    }
    let frame_basis_for = |chain_idx: usize, x_shift: f32| -> (f32, f32, f32, f32, usize) {
        let frame = em.chain[chain_idx];
        let target_page = em.chain_pages[chain_idx];
        let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
        let (ox, oy) = pages[target_page].spread_origin;
        let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
        let table_left_pt = sx - ox + insets[1] + x_shift;
        let frame_top_in_page = sy - oy;
        let frame_height = frame.bounds.height();
        (
            table_left_pt,
            frame_top_in_page,
            frame_height,
            insets[0],
            target_page,
        )
    };
    let mut chain_idx = em.frame_idx;
    let (mut tab_left, mut frame_top_in_page, mut frame_height, mut top_inset, mut target_page) =
        frame_basis_for(chain_idx, em.column_x_shift_pt);
    let mut row_top_y_in_frame = if em.y_cursor >= 0 {
        em.y_cursor as f32 / idml_text::shape::ADVANCE_PRECISION
            - em.options.default_point_size * 0.8
    } else {
        top_inset
    };
    // Total replayed-footer height we should leave reserved below
    // body rows in any non-last frame. Equals the sum of footer
    // template heights when `repeating_footer` is set.
    let footer_reserved_h: f32 = if repeating_footer {
        (total_rows - footer_count..total_rows)
            .map(|r| row_heights[r])
            .sum()
    } else {
        0.0
    };
    // Same for headers — height of header rows we replay at the
    // top of every continuation frame.
    let header_reserved_h: f32 = if repeating_header {
        (0..header_count).map(|r| row_heights[r]).sum()
    } else {
        0.0
    };

    let mut physical_rows: Vec<PhysicalRow> = Vec::with_capacity(total_rows);
    // Per-frame extent for table-border emission below.
    // Each entry: (chain_idx, target_page, table_left_pt, row_top
    // of the first row in this frame, row_bottom of the last row
    // in this frame).
    let mut frame_extents: Vec<(usize, usize, f32, f32, f32)> = Vec::new();
    let mut current_frame_first_top = frame_top_in_page + row_top_y_in_frame;
    let mut current_frame_last_bottom = current_frame_first_top;

    // Track which "body" rows (rows whose template index falls in
    // `header_count..total_rows - footer_count`) we still need to
    // emit. Header rows are always emitted at the top of frame 1
    // (their position in the original sequence) plus replayed at
    // the top of every continuation frame. Footer rows are emitted
    // at the bottom of the *last* frame in the original sequence
    // position, plus replayed at the bottom of every non-last frame.
    let body_range = header_count..total_rows.saturating_sub(footer_count);

    // Helper closures need to keep the borrow of `em` short, so we
    // pull the frame-advance logic into an inline block. The body
    // of the loop below is mechanical: append the next body row
    // (or first run of original headers / final footers) and check
    // whether we still fit.
    let mut placed_in_frame = 0usize;

    // Emit the original header rows at the start of the head frame
    // (they sit in the natural sequence — no replay).
    for r in 0..header_count {
        let h = row_heights[r];
        // We don't attempt to fit headers across a frame split on
        // their own — if a head frame is too small to hold even
        // the headers we'd loop forever. Leave them in this frame
        // and let the body rows trigger the chain advance instead.
        physical_rows.push(PhysicalRow {
            template_idx: r,
            height: h,
            chain_idx,
            target_page,
            table_left_pt: tab_left,
            row_top_in_page: frame_top_in_page + row_top_y_in_frame,
            kind: RowKind::Original,
        });
        row_top_y_in_frame += h;
        current_frame_last_bottom = frame_top_in_page + row_top_y_in_frame;
        placed_in_frame += 1;
    }

    // Emit body rows. Before placing each row, check whether it
    // (plus the footer-reserve, if any) would overflow the current
    // frame. If so, close out this frame with replayed footers,
    // advance, then prepend replayed headers in the new frame.
    for r in body_range.clone() {
        let h = row_heights[r];
        let need_extra_for_split = footer_reserved_h;
        let would_overflow = row_top_y_in_frame + h + need_extra_for_split > frame_height;
        if would_overflow && chain_idx + 1 < em.chain.len() && placed_in_frame > 0 {
            // Append replayed footers at the bottom of this frame.
            if repeating_footer {
                for fr in (total_rows - footer_count)..total_rows {
                    let fh = row_heights[fr];
                    physical_rows.push(PhysicalRow {
                        template_idx: fr,
                        height: fh,
                        chain_idx,
                        target_page,
                        table_left_pt: tab_left,
                        row_top_in_page: frame_top_in_page + row_top_y_in_frame,
                        kind: RowKind::FooterReplay,
                    });
                    row_top_y_in_frame += fh;
                    current_frame_last_bottom = frame_top_in_page + row_top_y_in_frame;
                }
            }
            // Close out current frame's extent.
            frame_extents.push((
                chain_idx,
                target_page,
                tab_left,
                current_frame_first_top,
                current_frame_last_bottom,
            ));
            chain_idx += 1;
            let (l, ftop, h_next, ti, tp) = frame_basis_for(chain_idx, 0.0);
            tab_left = l;
            frame_top_in_page = ftop;
            frame_height = h_next;
            top_inset = ti;
            target_page = tp;
            row_top_y_in_frame = top_inset;
            current_frame_first_top = frame_top_in_page + row_top_y_in_frame;
            placed_in_frame = 0;
            // Prepend replayed headers at the top of the new frame.
            // (current_frame_last_bottom is updated by the body push
            // immediately following — no need to maintain it here.)
            if repeating_header {
                for hr in 0..header_count {
                    let hh = row_heights[hr];
                    physical_rows.push(PhysicalRow {
                        template_idx: hr,
                        height: hh,
                        chain_idx,
                        target_page,
                        table_left_pt: tab_left,
                        row_top_in_page: frame_top_in_page + row_top_y_in_frame,
                        kind: RowKind::HeaderReplay,
                    });
                    row_top_y_in_frame += hh;
                    placed_in_frame += 1;
                }
                let _ = header_reserved_h;
            }
            // current_frame_last_bottom updates when the next body
            // row pushes below; no need to maintain it here.
        }
        physical_rows.push(PhysicalRow {
            template_idx: r,
            height: h,
            chain_idx,
            target_page,
            table_left_pt: tab_left,
            row_top_in_page: frame_top_in_page + row_top_y_in_frame,
            kind: RowKind::Original,
        });
        row_top_y_in_frame += h;
        current_frame_last_bottom = frame_top_in_page + row_top_y_in_frame;
        placed_in_frame += 1;
    }

    // Original footer rows — emitted on whatever frame the body
    // left off in (= the last frame), in their natural sequence.
    for r in (total_rows - footer_count)..total_rows {
        if footer_count == 0 {
            break;
        }
        let h = row_heights[r];
        physical_rows.push(PhysicalRow {
            template_idx: r,
            height: h,
            chain_idx,
            target_page,
            table_left_pt: tab_left,
            row_top_in_page: frame_top_in_page + row_top_y_in_frame,
            kind: RowKind::Original,
        });
        row_top_y_in_frame += h;
        current_frame_last_bottom = frame_top_in_page + row_top_y_in_frame;
    }
    // Close out the trailing frame extent.
    frame_extents.push((
        chain_idx,
        target_page,
        tab_left,
        current_frame_first_top,
        current_frame_last_bottom,
    ));
    // Track the final frame index + y for the y_cursor advance.
    let final_chain_idx = chain_idx;
    let final_y_in_frame = row_top_y_in_frame;

    // Alternating row fills. The TableStyle cycles between
    // `start_row_fill_color` (count rows) and
    // `end_row_fill_color` (count rows) starting from the first
    // *body* row. Cells with their own cell-style fill paint over
    // the alternating fill.
    let alternating_fill_for_body_row = |body_row_idx: usize| -> Option<(&str, Option<f32>)> {
        let start_n = resolved_table.start_row_fill_count.unwrap_or(0) as usize;
        let end_n = resolved_table.end_row_fill_count.unwrap_or(0) as usize;
        let cycle = start_n + end_n;
        if cycle == 0 {
            return None;
        }
        let pos = body_row_idx % cycle;
        if pos < start_n {
            resolved_table
                .start_row_fill_color
                .as_deref()
                .map(|c| (c, resolved_table.start_row_fill_tint))
        } else {
            resolved_table
                .end_row_fill_color
                .as_deref()
                .map(|c| (c, resolved_table.end_row_fill_tint))
        }
    };
    // Alternating fills iterate the physical-row sequence: replayed
    // headers / footers count from their *original* template index
    // so the visual cycle stays coherent across frame splits.
    for prow in &physical_rows {
        let r = prow.template_idx;
        if r < header_count {
            continue;
        }
        if footer_count > 0 && r + footer_count >= total_rows {
            continue;
        }
        let body_idx = r - header_count;
        let Some((fill_id, tint)) = alternating_fill_for_body_row(body_idx) else {
            continue;
        };
        let Some(paint) = color_id_to_paint(fill_id, em.palette, em.cmyk_xform) else {
            continue;
        };
        let paint = apply_fill_tint(paint, tint);
        let rect = Rect {
            x: prow.table_left_pt,
            y: prow.row_top_in_page,
            w: total_w,
            h: prow.height,
        };
        emit_rect(rect, paint, &mut pages[prow.target_page].list);
    }

    // Iterate physical rows × cells. For each physical row, find
    // the `<Cell>` entries whose template row matches and emit
    // them at the row's actual page-local coordinates. This naturally
    // handles header/footer replays — the same `<Cell>` definition
    // re-renders at the duplicated row's basis.
    //
    // Build a (col, template_row) → cell index map so the inner
    // loop is O(1) per cell rather than O(cells × physical_rows).
    let mut cell_by_origin: std::collections::HashMap<(u32, u32), &idml_parse::TableCell> =
        std::collections::HashMap::with_capacity(table.cells.len());
    for cell in &table.cells {
        if let Some(coords) = cell.coords() {
            cell_by_origin.insert(coords, cell);
        }
    }
    for prow_i in 0..physical_rows.len() {
        let prow = physical_rows[prow_i];
        let r = prow.template_idx;
        for c in 0..col_widths.len() {
            let Some(cell) = cell_by_origin.get(&(c as u32, r as u32)).copied() else {
                continue;
            };
        let target_page = prow.target_page;
        let cell_x_pt = prow.table_left_pt + col_x[c];
        let cell_y_pt = prow.row_top_in_page;
        let last_c = (c + cell.column_span.max(1) as usize).min(col_widths.len());
        // For row spans, accumulate heights of the contiguous
        // *physical* rows that sit in the same frame as this cell's
        // starting row. Spans that would straddle a frame boundary
        // clip to the originating frame's bottom (same conservative
        // policy as before). Walk physical rows starting at the
        // current physical row, advancing while their template_idx
        // is within `[r, r + span)` and `chain_idx` matches.
        let span_rows = cell.row_span.max(1) as usize;
        let mut cell_h_pt = 0.0f32;
        let mut step = 0usize;
        while step < span_rows && prow_i + step < physical_rows.len() {
            let next = &physical_rows[prow_i + step];
            if next.chain_idx != prow.chain_idx {
                break;
            }
            // Only accumulate template rows in [r, r + span). A
            // continuation frame whose first row is a HeaderReplay
            // would otherwise add the header's height to the body
            // cell's span. The replay rows live in a *different*
            // physical row index, so we'd never reach them mid-span
            // anyway — but the explicit range guard makes this
            // robust if the physical-row sequence ever interleaves
            // replays differently.
            let t = next.template_idx;
            if t < r || t >= r + span_rows {
                break;
            }
            cell_h_pt += next.height;
            step += 1;
        }
        if cell_h_pt <= 0.0 {
            cell_h_pt = prow.height;
        }
        let cell_w_pt = col_x[last_c] - col_x[c];

        let inner_left = cell_x_pt + cell.text_left_inset;
        let inner_top = cell_y_pt + cell.text_top_inset;
        let inner_w = (cell_w_pt - cell.text_left_inset - cell.text_right_inset).max(0.0);
        let inner_h = (cell_h_pt - cell.text_top_inset - cell.text_bottom_inset).max(0.0);

        // Resolve the cell's CellStyle. Per-cell AppliedCellStyle
        // wins; fall through to the table-style region default
        // (Header / Body / Footer / left or right column).
        let cell_style_id = cell
            .applied_cell_style
            .as_deref()
            .filter(|id| !is_none_style_id(id))
            .or_else(|| region_cell_style_for(c, r));
        let resolved_cell = cell_style_id
            .map(|id| em.document.styles.resolve_cell(id))
            .unwrap_or_default();

        // Cell fill — drawn before text so glyphs paint on top.
        // Inline FillColor on the <Cell> wins over the cascaded
        // cell-style fill — same precedence as the per-edge stroke
        // overrides above.
        let cell_fill_id = cell
            .fill_color
            .as_deref()
            .filter(|c| !is_none_swatch_id(c))
            .or(resolved_cell.fill_color.as_deref());
        if let Some(fill) =
            cell_fill_id.and_then(|id| color_id_to_paint(id, em.palette, em.cmyk_xform))
        {
            emit_rect(
                Rect {
                    x: cell_x_pt,
                    y: cell_y_pt,
                    w: cell_w_pt,
                    h: cell_h_pt,
                },
                fill,
                &mut pages[target_page].list,
            );
        }
        // Per-edge cell strokes. Each edge gets its own thin rect
        // (filled, since rect-stroke aligns to centerlines and we
        // want the edge to sit precisely on the cell boundary).
        // Per-cell overrides (declared inline on the <Cell> element)
        // win over the cascaded CellStyle — IDML serialises real row
        // dividers there even when AppliedCellStyle is `[None]`.
        let cell_top_color = cell
            .top_edge_stroke_color
            .as_deref()
            .filter(|c| !is_none_swatch_id(c))
            .or(resolved_cell.top_edge_stroke_color.as_deref());
        let cell_top_weight = cell
            .top_edge_stroke_weight
            .or(resolved_cell.top_edge_stroke_weight);
        let cell_bot_color = cell
            .bottom_edge_stroke_color
            .as_deref()
            .filter(|c| !is_none_swatch_id(c))
            .or(resolved_cell.bottom_edge_stroke_color.as_deref());
        let cell_bot_weight = cell
            .bottom_edge_stroke_weight
            .or(resolved_cell.bottom_edge_stroke_weight);
        let edges = [
            (
                cell_top_color,
                cell_top_weight,
                cell.top_edge_stroke_tint,
                cell_x_pt,
                cell_y_pt,
                cell_w_pt,
            ),
            (
                cell_bot_color,
                cell_bot_weight,
                cell.bottom_edge_stroke_tint,
                cell_x_pt,
                cell_y_pt + cell_h_pt,
                cell_w_pt,
            ),
        ];
        for (color, weight, tint, x, y, w) in edges {
            if let (Some(color_id), Some(weight)) = (color, weight) {
                if weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform)
                        .map(|p| apply_fill_tint(p, tint))
                    {
                        emit_rect(
                            Rect {
                                x,
                                y: y - weight * 0.5,
                                w,
                                h: weight,
                            },
                            paint,
                            &mut pages[target_page].list,
                        );
                    }
                }
            }
        }
        let cell_left_color = cell
            .left_edge_stroke_color
            .as_deref()
            .filter(|c| !is_none_swatch_id(c))
            .or(resolved_cell.left_edge_stroke_color.as_deref());
        let cell_left_weight = cell
            .left_edge_stroke_weight
            .or(resolved_cell.left_edge_stroke_weight);
        let cell_right_color = cell
            .right_edge_stroke_color
            .as_deref()
            .filter(|c| !is_none_swatch_id(c))
            .or(resolved_cell.right_edge_stroke_color.as_deref());
        let cell_right_weight = cell
            .right_edge_stroke_weight
            .or(resolved_cell.right_edge_stroke_weight);
        let v_edges = [
            (
                cell_left_color,
                cell_left_weight,
                cell.left_edge_stroke_tint,
                cell_x_pt,
                cell_y_pt,
                cell_h_pt,
            ),
            (
                cell_right_color,
                cell_right_weight,
                cell.right_edge_stroke_tint,
                cell_x_pt + cell_w_pt,
                cell_y_pt,
                cell_h_pt,
            ),
        ];
        for (color, weight, tint, x, y, h) in v_edges {
            if let (Some(color_id), Some(weight)) = (color, weight) {
                if weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform)
                        .map(|p| apply_fill_tint(p, tint))
                    {
                        emit_rect(
                            Rect {
                                x: x - weight * 0.5,
                                y,
                                w: weight,
                                h,
                            },
                            paint,
                            &mut pages[target_page].list,
                        );
                    }
                }
            }
        }

        // Diagonal cell strokes. IDML's "Left" diagonal goes
        // top-left → bottom-right; "Right" goes top-right →
        // bottom-left. Emitted before content as the simpler default;
        // `DiagonalLineInFront=true` semantics (paint over content)
        // are queued — visually this only matters when content
        // overlaps the diagonal, which is rare.
        let diag = &cell.diagonal;
        let diag_emit = |drawn: Option<bool>,
                         color: Option<&str>,
                         weight: Option<f32>,
                         (x1, y1): (f32, f32),
                         (x2, y2): (f32, f32),
                         pages: &mut [BuiltPage]| {
            if drawn != Some(true) {
                return;
            }
            let Some(weight) = weight.filter(|w| *w > 0.0) else {
                return;
            };
            let Some(color_id) = color else {
                return;
            };
            if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
                idml_compose::emit_line(
                    x1,
                    y1,
                    x2,
                    y2,
                    Stroke::new(weight),
                    paint,
                    &mut pages[target_page].list,
                );
            }
        };
        diag_emit(
            diag.left_line_drawn,
            diag.left_line_color.as_deref(),
            diag.left_line_weight,
            (cell_x_pt, cell_y_pt),
            (cell_x_pt + cell_w_pt, cell_y_pt + cell_h_pt),
            pages,
        );
        diag_emit(
            diag.right_line_drawn,
            diag.right_line_color.as_deref(),
            diag.right_line_weight,
            (cell_x_pt + cell_w_pt, cell_y_pt),
            (cell_x_pt, cell_y_pt + cell_h_pt),
            pages,
        );

        // Lay out the cell paragraphs into a working buffer first
        // so we know their total height; then apply vertical
        // justification by shifting all of them by a uniform dy.
        let mut paragraph_y = 0.0f32;
        let mut emitted_extents: Vec<(usize, usize)> = Vec::new();
        for paragraph in &cell.paragraphs {
            if paragraph.runs.is_empty() {
                continue;
            }
            let cmd_start = pages[target_page].list.commands.len();
            let consumed = emit_cell_paragraph(
                em,
                paragraph,
                target_page,
                (inner_left, inner_top),
                inner_w,
                paragraph_y,
                pages,
                total_stats,
            );
            let cmd_end = pages[target_page].list.commands.len();
            if cmd_end > cmd_start {
                emitted_extents.push((cmd_start, cmd_end));
            }
            paragraph_y += consumed;
            if paragraph_y >= inner_h {
                break;
            }
        }
        // Apply CellStyle vertical justification by shifting every
        // glyph command we emitted in this cell by dy = slack/factor.
        // CenterAlign → centre vertically; BottomAlign → push to the
        // bottom inset. Top is the default (no shift).
        let used_h = paragraph_y;
        if used_h > 0.0 && used_h < inner_h {
            let dy = match resolved_cell.vertical_justification.as_deref() {
                Some("CenterAlign") => Some((inner_h - used_h) * 0.5),
                Some("BottomAlign") => Some(inner_h - used_h),
                _ => None,
            };
            if let Some(dy) = dy {
                for (s, e) in &emitted_extents {
                    for cmd in &mut pages[target_page].list.commands[*s..*e] {
                        cmd.transform_mut().0[5] += dy;
                    }
                }
            }
        }
        } // close inner `for c in 0..col_widths.len()`
    } // close outer `for prow_i in 0..physical_rows.len()`

    // Resolve effective outer-border attributes. Direct `<Table>`
    // attributes (e.g. `LeftBorderStrokeColor` on the `<Table>`
    // element itself) win over the AppliedTableStyle's cascaded
    // values; weight defaults to 1pt when both are absent and a
    // colour is present.
    let direct = &table.border;
    let effective_color = |direct: Option<&str>, style: Option<&str>| -> Option<String> {
        match direct {
            Some(s) if !is_none_swatch_id(s) => Some(s.to_string()),
            _ => style.map(|s| s.to_string()),
        }
    };
    let effective_weight = |direct_w: Option<f32>, style_w: Option<f32>, has_color: bool| -> f32 {
        if let Some(w) = direct_w {
            return w;
        }
        if let Some(w) = style_w {
            return w;
        }
        if has_color {
            1.0
        } else {
            0.0
        }
    };
    let top_color = effective_color(
        direct.top_color.as_deref(),
        resolved_table.top_border_stroke_color.as_deref(),
    );
    let top_weight = effective_weight(
        direct.top_weight,
        resolved_table.top_border_stroke_weight,
        top_color.is_some(),
    );
    let top_type = direct.top_type.clone();
    let bot_color = effective_color(
        direct.bottom_color.as_deref(),
        resolved_table.bottom_border_stroke_color.as_deref(),
    );
    let bot_weight = effective_weight(
        direct.bottom_weight,
        resolved_table.bottom_border_stroke_weight,
        bot_color.is_some(),
    );
    let bot_type = direct.bottom_type.clone();
    let left_color = effective_color(
        direct.left_color.as_deref(),
        resolved_table.left_border_stroke_color.as_deref(),
    );
    let left_weight = effective_weight(
        direct.left_weight,
        resolved_table.left_border_stroke_weight,
        left_color.is_some(),
    );
    let left_type = direct.left_type.clone();
    let right_color = effective_color(
        direct.right_color.as_deref(),
        resolved_table.right_border_stroke_color.as_deref(),
    );
    let right_weight = effective_weight(
        direct.right_weight,
        resolved_table.right_border_stroke_weight,
        right_color.is_some(),
    );
    let right_type = direct.right_type.clone();

    // Row separators between rows. IDML serialises divider styles
    // via `StartRowStrokeType` / `EndRowStrokeType` on the `<Table>`.
    // The first `start_count` row separators use the start-stroke
    // style; subsequent dividers fall through to the end-stroke
    // style (alternating). When `start_color` is absent but a type
    // is declared we fall back to black — IDML's documented default.
    let row_decl = &table.row_strokes;
    let row_start_type = row_decl.start_type.clone();
    let row_start_color_raw = row_decl.start_color.clone();
    let has_row_decl = row_start_type.is_some()
        || row_start_color_raw.is_some()
        || row_decl.end_type.is_some()
        || row_decl.end_color.is_some();
    let row_start_color = if has_row_decl && row_start_color_raw.is_none() {
        Some("Color/Black".to_string())
    } else {
        row_start_color_raw
    };
    let row_start_weight = row_decl
        .start_weight
        .unwrap_or(if has_row_decl { 1.0 } else { 0.0 });
    let row_end_type = row_decl
        .end_type
        .clone()
        .or_else(|| row_start_type.clone());
    let row_end_color = row_decl
        .end_color
        .clone()
        .or_else(|| row_start_color.clone());
    let row_end_weight = row_decl.end_weight.unwrap_or(row_start_weight);
    let row_start_count = row_decl.start_count.unwrap_or(0) as usize;
    let row_end_count = row_decl.end_count.unwrap_or(0) as usize;
    let row_cycle = row_start_count + row_end_count;
    let pick_row_stroke = |i: usize| -> (Option<&str>, Option<&str>, f32) {
        if row_cycle == 0 {
            return (
                row_start_type.as_deref(),
                row_start_color.as_deref(),
                row_start_weight,
            );
        }
        let pos = i % row_cycle;
        if pos < row_start_count {
            (
                row_start_type.as_deref(),
                row_start_color.as_deref(),
                row_start_weight,
            )
        } else {
            (
                row_end_type.as_deref(),
                row_end_color.as_deref(),
                row_end_weight,
            )
        }
    };

    // Emit row dividers. A divider sits at the bottom edge of a
    // physical row when the next physical row sits in the same
    // frame. The dividing-stroke pick still cycles via the *template*
    // index so replayed header / footer rows match the original
    // dividers (visually consistent across continuation frames).
    for i in 0..physical_rows.len().saturating_sub(1) {
        let curr = physical_rows[i];
        let next = physical_rows[i + 1];
        if curr.chain_idx != next.chain_idx {
            continue;
        }
        let (stype, scolor, sweight) = pick_row_stroke(curr.template_idx);
        let Some(color_id) = scolor else { continue };
        if sweight <= 0.0 {
            continue;
        }
        let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) else {
            continue;
        };
        let y = curr.row_top_in_page + curr.height;
        emit_table_horizontal_edge(
            curr.table_left_pt,
            y,
            total_w,
            stype,
            sweight,
            paint,
            &mut pages[curr.target_page].list,
        );
    }

    // Table-level borders, drawn per-frame so a threaded table
    // gets a top border at the start of the first frame, a bottom
    // border at the end of the last frame, and full left/right
    // borders inside every frame the table touches.
    for (i, (_chain_idx, fp_target_page, frame_table_left, top_y, bottom_y)) in
        frame_extents.iter().enumerate()
    {
        let is_first = i == 0;
        let is_last = i == frame_extents.len() - 1;
        let target = *fp_target_page;
        if is_first {
            if let Some(color_id) = top_color.as_deref() {
                if top_weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
                        emit_table_horizontal_edge(
                            *frame_table_left,
                            *top_y,
                            total_w,
                            top_type.as_deref(),
                            top_weight,
                            paint,
                            &mut pages[target].list,
                        );
                    }
                }
            }
        }
        if is_last {
            if let Some(color_id) = bot_color.as_deref() {
                if bot_weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
                        emit_table_horizontal_edge(
                            *frame_table_left,
                            *bottom_y,
                            total_w,
                            bot_type.as_deref(),
                            bot_weight,
                            paint,
                            &mut pages[target].list,
                        );
                    }
                }
            }
        }
        // Left/right borders span this frame's portion of the table.
        let segment_h = bottom_y - top_y;
        if let Some(color_id) = left_color.as_deref() {
            if left_weight > 0.0 {
                if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
                    emit_table_vertical_edge(
                        *frame_table_left,
                        *top_y,
                        segment_h,
                        left_type.as_deref(),
                        left_weight,
                        paint,
                        &mut pages[target].list,
                    );
                }
            }
        }
        if let Some(color_id) = right_color.as_deref() {
            if right_weight > 0.0 {
                if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
                    emit_table_vertical_edge(
                        *frame_table_left + total_w,
                        *top_y,
                        segment_h,
                        right_type.as_deref(),
                        right_weight,
                        paint,
                        &mut pages[target].list,
                    );
                }
            }
        }
    }

    // Advance the active frame_idx + y_cursor to the row after the
    // last one we placed. The host emitter loop reads em.frame_idx
    // and em.y_cursor when continuing the surrounding paragraph
    // flow.
    em.frame_idx = final_chain_idx;
    em.y_cursor = ((final_y_in_frame + em.options.default_point_size * 0.8)
        * idml_text::shape::ADVANCE_PRECISION)
        .round() as i32;
    total_stats.paragraphs += 1;
    let stat_page = em.chain_pages[em.frame_idx];
    pages[stat_page].stats.paragraphs += 1;
}

/// Strip `StrokeStyle/$ID/` and an optional leading `Canned ` so the
/// remaining suffix matches the canonical stroke-style name table.
/// Mirrors `stroke_for`'s normalisation for the table-edge emitter.
fn normalise_stroke_type(name: Option<&str>) -> &str {
    let Some(name) = name else { return "Solid" };
    let suffix = name.strip_prefix("StrokeStyle/$ID/").unwrap_or(name);
    suffix.strip_prefix("Canned ").unwrap_or(suffix)
}

/// Emit a horizontal table-edge segment of length `length` starting
/// at `(x, y)` (the centre of the edge, snapped to the cell boundary).
/// Honours a small set of stroke types:
///
/// * `Solid` / unknown → single filled rect of height `weight`.
/// * `ThickThick` → two parallel rects each of height `weight/3`,
///   separated by a `weight/3` gap; the trio spans `weight` total
///   (matches InDesign's preset).
/// * `Dotted` / `Dotted2..8` / `Japanese Dots` → a series of small
///   filled circles of diameter `weight` stamped along the edge.
fn emit_table_horizontal_edge(
    x: f32,
    y: f32,
    length: f32,
    stroke_type: Option<&str>,
    weight: f32,
    paint: Paint,
    list: &mut DisplayList,
) {
    if weight <= 0.0 || length <= 0.0 {
        return;
    }
    let kind = normalise_stroke_type(stroke_type);
    match kind {
        "ThickThick" => {
            let line_w = weight / 3.0;
            let upper_centre = y - weight / 3.0;
            let lower_centre = y + weight / 3.0;
            emit_rect(
                Rect {
                    x,
                    y: upper_centre - line_w * 0.5,
                    w: length,
                    h: line_w,
                },
                paint,
                list,
            );
            emit_rect(
                Rect {
                    x,
                    y: lower_centre - line_w * 0.5,
                    w: length,
                    h: line_w,
                },
                paint,
                list,
            );
        }
        "Dotted" | "Dotted2" | "Dotted4" | "Dotted8" | "Japanese Dots" => {
            let step = match kind {
                "Dotted2" | "Dotted" => 2.0,
                "Dotted4" => 4.0,
                "Dotted8" => 8.0,
                _ => 1.5,
            } * weight.max(0.1);
            let diameter = weight;
            let mut cx = x;
            while cx <= x + length + 0.001 {
                emit_ellipse(
                    Rect {
                        x: cx - diameter * 0.5,
                        y: y - diameter * 0.5,
                        w: diameter,
                        h: diameter,
                    },
                    paint,
                    list,
                );
                cx += step;
            }
        }
        _ => {
            emit_rect(
                Rect {
                    x,
                    y: y - weight * 0.5,
                    w: length,
                    h: weight,
                },
                paint,
                list,
            );
        }
    }
}

/// Vertical analogue of [`emit_table_horizontal_edge`]. `x` is the
/// horizontal centre of the edge; the segment spans `(y, y + length)`.
fn emit_table_vertical_edge(
    x: f32,
    y: f32,
    length: f32,
    stroke_type: Option<&str>,
    weight: f32,
    paint: Paint,
    list: &mut DisplayList,
) {
    if weight <= 0.0 || length <= 0.0 {
        return;
    }
    let kind = normalise_stroke_type(stroke_type);
    match kind {
        "ThickThick" => {
            let line_w = weight / 3.0;
            let left_centre = x - weight / 3.0;
            let right_centre = x + weight / 3.0;
            emit_rect(
                Rect {
                    x: left_centre - line_w * 0.5,
                    y,
                    w: line_w,
                    h: length,
                },
                paint,
                list,
            );
            emit_rect(
                Rect {
                    x: right_centre - line_w * 0.5,
                    y,
                    w: line_w,
                    h: length,
                },
                paint,
                list,
            );
        }
        "Dotted" | "Dotted2" | "Dotted4" | "Dotted8" | "Japanese Dots" => {
            let step = match kind {
                "Dotted2" | "Dotted" => 2.0,
                "Dotted4" => 4.0,
                "Dotted8" => 8.0,
                _ => 1.5,
            } * weight.max(0.1);
            let diameter = weight;
            let mut cy = y;
            while cy <= y + length + 0.001 {
                emit_ellipse(
                    Rect {
                        x: x - diameter * 0.5,
                        y: cy - diameter * 0.5,
                        w: diameter,
                        h: diameter,
                    },
                    paint,
                    list,
                );
                cy += step;
            }
        }
        _ => {
            emit_rect(
                Rect {
                    x: x - weight * 0.5,
                    y,
                    w: weight,
                    h: length,
                },
                paint,
                list,
            );
        }
    }
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
    id == "Swatch/None" || id == "n" || id.is_empty()
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
fn split_paragraph_at_breaks(paragraph: &idml_parse::Paragraph) -> Vec<idml_parse::Paragraph> {
    // Walk runs in order; for each run, split text at '\n' and
    // emit the leading segment into the in-progress sub-paragraph,
    // then close the sub-paragraph and start a new one.
    let mut subs: Vec<idml_parse::Paragraph> = Vec::new();
    let mut current = idml_parse::Paragraph {
        paragraph_style: paragraph.paragraph_style.clone(),
        justification: paragraph.justification,
        first_line_indent: paragraph.first_line_indent,
        space_before: paragraph.space_before,
        space_after: None, // applied to last sub-paragraph only
        tab_list: paragraph.tab_list.clone(),
        bullets_list_type: paragraph.bullets_list_type.clone(),
        bullet_character: paragraph.bullet_character,
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
                let mut next = idml_parse::Paragraph {
                    paragraph_style: paragraph.paragraph_style.clone(),
                    justification: paragraph.justification,
                    first_line_indent: paragraph.first_line_indent,
                    space_before: None,
                    space_after: None,
                    tab_list: paragraph.tab_list.clone(),
                    bullets_list_type: paragraph.bullets_list_type.clone(),
                    bullet_character: paragraph.bullet_character,
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
        subs.push(idml_parse::Paragraph {
            paragraph_style: paragraph.paragraph_style.clone(),
            justification: paragraph.justification,
            first_line_indent: paragraph.first_line_indent,
            space_before: paragraph.space_before,
            space_after: paragraph.space_after,
            tab_list: paragraph.tab_list.clone(),
            bullets_list_type: paragraph.bullets_list_type.clone(),
            bullet_character: paragraph.bullet_character,
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
/// Returns `0.0` when the paragraph is empty or the font assets
/// don't resolve — callers compare against `SingleRowHeight` /
/// `MinimumHeight` so a 0 is safely absorbed.
fn measure_cell_paragraph(
    em: &StoryEmitter,
    paragraph: &idml_parse::Paragraph,
    column_width_pt: f32,
) -> f32 {
    if column_width_pt <= 0.0 || paragraph.runs.is_empty() {
        return 0.0;
    }
    let resolved_runs: Vec<idml_scene::ResolvedRunAttrs> = paragraph
        .runs
        .iter()
        .map(|r| em.document.resolved_run_attrs(paragraph, r))
        .collect();
    // Per-run bytes with per-paragraph fallback for any run whose
    // (family, style) doesn't resolve — keeps height-measurement
    // honest even when one cell run references an absent font.
    let Some(bytes_pool) = em.font_table.resolve_paragraph_bytes(&resolved_runs) else {
        return 0.0;
    };
    let wghts: Vec<f32> = resolved_runs
        .iter()
        .map(|r| wght_for_font_style(r.font_style.as_deref()))
        .collect();
    let mut unique_idx: Vec<usize> = Vec::with_capacity(bytes_pool.len());
    for (i, b) in bytes_pool.iter().enumerate() {
        let head = bytes_pool[..i]
            .iter()
            .zip(wghts[..i].iter())
            .position(|(prior, w)| prior.as_ptr() == b.as_ptr() && (*w - wghts[i]).abs() < 0.5)
            .unwrap_or(i);
        unique_idx.push(head);
    }
    // Shaping faces: prefer the per-render FontTable cache (built
    // from a full harvest of every run, table cells included); fall
    // back to building on demand for runs the cache didn't see.
    let mut owned_shaping_faces: Vec<Option<rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut shaping_faces: Vec<Option<&rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
    let bytes_font_ids: Vec<u32> = bytes_pool
        .iter()
        .map(|b| fnv_1a_u32(b.as_ref()))
        .collect();
    for i in 0..bytes_pool.len() {
        if unique_idx[i] != i {
            continue;
        }
        if em
            .font_table
            .face(bytes_font_ids[i], wghts[i].to_bits())
            .is_none()
        {
            let bytes_ref = bytes_pool[i].as_ref();
            let Some(mut rf) = rustybuzz::Face::from_slice(bytes_ref, 0) else {
                return 0.0;
            };
            let has_wght_axis = rf
                .variation_axes()
                .into_iter()
                .any(|axis| axis.tag == wght_tag);
            if has_wght_axis {
                rf.set_variations(&[rustybuzz::Variation {
                    tag: wght_tag,
                    value: wghts[i],
                }]);
            }
            owned_shaping_faces[i] = Some(rf);
        }
    }
    for i in 0..bytes_pool.len() {
        let head = unique_idx[i];
        if let Some(cached) = em.font_table.face(bytes_font_ids[head], wghts[head].to_bits()) {
            shaping_faces[i] = Some(cached);
        } else if let Some(owned) = owned_shaping_faces[head].as_ref() {
            shaping_faces[i] = Some(owned);
        }
    }
    let font_ids: Vec<u32> = bytes_pool
        .iter()
        .zip(wghts.iter())
        .map(|(b, w)| fnv_1a_u32(b.as_ref()) ^ w.to_bits())
        .collect();
    let styled_runs: Vec<idml_text::StyledRun> = paragraph
        .runs
        .iter()
        .enumerate()
        .map(|(i, run)| idml_text::StyledRun {
            text: &run.text,
            face: shaping_faces[unique_idx[i]].unwrap(),
            point_size: resolved_runs[i]
                .point_size
                .unwrap_or(em.options.default_point_size),
            tracking: resolved_runs[i].tracking,
            font_id: font_ids[i],
            underline: resolved_runs[i].underline.unwrap_or(false),
            strikethru: resolved_runs[i].strikethru.unwrap_or(false),
            baseline_shift_pt: resolved_runs[i].baseline_shift.unwrap_or(0.0),
            horizontal_scale_pct: resolved_runs[i].horizontal_scale.unwrap_or(100.0),
            fallback_faces: &[],
        })
        .collect();
    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);
    let mut lopts = idml_text::LayoutOptions::new(column_width_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification);
    apply_paragraph_compose_options(&mut lopts, em.hyphenator, &resolved_paragraph);
    lopts.first_baseline =
        ((paragraph_size * 0.8) * idml_text::shape::ADVANCE_PRECISION).round() as i32;
    let laid_out = idml_text::layout_runs(&styled_runs, &lopts);
    if laid_out.lines.is_empty() {
        return 0.0;
    }
    let leading_pt = paragraph_size * 1.2;
    let max_baseline_pt = laid_out
        .lines
        .iter()
        .map(|l| l.baseline_y as f32 / idml_text::shape::ADVANCE_PRECISION)
        .fold(0.0f32, f32::max);
    max_baseline_pt + leading_pt * 0.4
}

/// Lay out and emit a single cell paragraph at `(origin_pt.0,
/// origin_pt.1 + paragraph_y)` with `column_width_pt` available.
/// Returns the vertical extent the paragraph consumed so the
/// caller can stack subsequent cell paragraphs underneath.
/// Self-contained shape → layout → emit; no inter-paragraph state.
#[allow(clippy::too_many_arguments)]
fn emit_cell_paragraph(
    em: &StoryEmitter,
    paragraph: &idml_parse::Paragraph,
    target_page: usize,
    origin_pt: (f32, f32),
    column_width_pt: f32,
    paragraph_y: f32,
    pages: &mut [BuiltPage],
    total_stats: &mut PipelineStats,
) -> f32 {
    if column_width_pt <= 0.0 || paragraph.runs.is_empty() {
        return 0.0;
    }
    let resolved_runs: Vec<idml_scene::ResolvedRunAttrs> = paragraph
        .runs
        .iter()
        .map(|r| em.document.resolved_run_attrs(paragraph, r))
        .collect();
    // Per-run bytes with per-paragraph fallback (matches the main
    // emit path). A single unresolvable run no longer takes the
    // whole cell paragraph down with it.
    let Some(bytes_pool) = em.font_table.resolve_paragraph_bytes(&resolved_runs) else {
        return 0.0;
    };
    // Per-run wght axis values, derived from the resolved FontStyle.
    // Identical wiring to the main `emit_paragraph_into_chain` path —
    // table-cell text needs Bold / Light pinning too. Without this,
    // table column labels styled with a Bold paragraph style render
    // at the variable font's default weight (visible regression on
    // any catalog with bold table headers).
    let wghts: Vec<f32> = resolved_runs
        .iter()
        .map(|r| wght_for_font_style(r.font_style.as_deref()))
        .collect();
    // Reuse a shaped face only when both bytes AND weight match; a
    // bold + regular pair sharing the same Inter.ttf bytes still
    // needs two distinct rustybuzz::Face objects so set_variations
    // doesn't fight itself.
    let mut unique_idx: Vec<usize> = Vec::with_capacity(bytes_pool.len());
    for (i, b) in bytes_pool.iter().enumerate() {
        let head = bytes_pool[..i]
            .iter()
            .zip(wghts[..i].iter())
            .position(|(prior, w)| prior.as_ptr() == b.as_ptr() && (*w - wghts[i]).abs() < 0.5)
            .unwrap_or(i);
        unique_idx.push(head);
    }
    // Outline faces stay per-paragraph; shaping faces pull from
    // the per-render FontTable cache (built from a full
    // table-cell-aware harvest at startup) with an on-demand
    // fallback for any (font_id, wght_bits) the cache didn't see.
    let mut outline_faces: Vec<Option<ttf_parser::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut owned_shaping_faces: Vec<Option<rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut shaping_faces: Vec<Option<&rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
    let bytes_font_ids: Vec<u32> = bytes_pool
        .iter()
        .map(|b| fnv_1a_u32(b.as_ref()))
        .collect();
    for i in 0..bytes_pool.len() {
        if unique_idx[i] != i {
            continue;
        }
        let bytes_ref = bytes_pool[i].as_ref();
        let Ok(mut of) = ttf_parser::Face::parse(bytes_ref, 0) else {
            return 0.0;
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

        if em
            .font_table
            .face(bytes_font_ids[i], wghts[i].to_bits())
            .is_none()
        {
            let Some(mut rf) = rustybuzz::Face::from_slice(bytes_ref, 0) else {
                return 0.0;
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
    for i in 0..bytes_pool.len() {
        let head = unique_idx[i];
        if let Some(cached) = em.font_table.face(bytes_font_ids[head], wghts[head].to_bits()) {
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

    let styled_runs: Vec<idml_text::StyledRun> = paragraph
        .runs
        .iter()
        .enumerate()
        .map(|(i, run)| idml_text::StyledRun {
            text: &run.text,
            face: shaping_faces[unique_idx[i]].unwrap(),
            point_size: resolved_runs[i]
                .point_size
                .unwrap_or(em.options.default_point_size),
            tracking: resolved_runs[i].tracking,
            font_id: font_ids[i],
            underline: resolved_runs[i].underline.unwrap_or(false),
            strikethru: resolved_runs[i].strikethru.unwrap_or(false),
            baseline_shift_pt: resolved_runs[i].baseline_shift.unwrap_or(0.0),
            horizontal_scale_pct: resolved_runs[i].horizontal_scale.unwrap_or(100.0),
            fallback_faces: &[],
        })
        .collect();
    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);
    let mut lopts = idml_text::LayoutOptions::new(column_width_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification);
    apply_paragraph_compose_options(&mut lopts, em.hyphenator, &resolved_paragraph);
    lopts.first_baseline =
        ((paragraph_size * 0.8) * idml_text::shape::ADVANCE_PRECISION).round() as i32;

    let laid_out = idml_text::layout_runs(&styled_runs, &lopts);
    if laid_out.lines.is_empty() {
        return 0.0;
    }

    let picker = build_run_paint_picker_resolved(
        paragraph,
        &resolved_runs,
        em.palette,
        em.cmyk_xform,
        em.options.fallback_text_paint,
        None,
    );
    let stroke_picker = build_run_stroke_picker(
        paragraph,
        &resolved_runs,
        em.palette,
        em.cmyk_xform,
        0,
    );
    let any_text_stroke = stroke_picker.any_visible();
    let leading_pt = paragraph_size * 1.2;
    let cell_origin = (origin_pt.0, origin_pt.1 + paragraph_y);
    let list = &mut pages[target_page].list;
    let mut max_baseline_pt = 0.0f32;
    for line in &laid_out.lines {
        let baseline_pt = line.baseline_y as f32 / idml_text::shape::ADVANCE_PRECISION;
        if baseline_pt > max_baseline_pt {
            max_baseline_pt = baseline_pt;
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
            emit_glyph_slice(
                &line.glyphs[start..end],
                fid,
                line.glyphs[start].point_size,
                |cluster| picker.pick(cluster),
                cell_origin,
                &outliner,
                list,
            );
            if any_text_stroke {
                emit_glyph_slice_stroke(
                    &line.glyphs[start..end],
                    fid,
                    line.glyphs[start].point_size,
                    |cluster| stroke_picker.pick(cluster),
                    cell_origin,
                    &outliner,
                    list,
                );
            }
            start = end;
        }
    }
    let glyph_count: usize = laid_out.lines.iter().map(|l| l.glyphs.len()).sum();
    total_stats.paragraphs += 1;
    total_stats.runs += paragraph.runs.len();
    total_stats.glyphs += glyph_count;
    total_stats.lines += laid_out.lines.len();
    pages[target_page].stats.paragraphs += 1;
    pages[target_page].stats.runs += paragraph.runs.len();
    pages[target_page].stats.glyphs += glyph_count;
    pages[target_page].stats.lines += laid_out.lines.len();
    max_baseline_pt + leading_pt * 0.4
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
    let m = frame.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
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

/// Scanline x-intersections of a closed polygon at horizontal line
/// `y`. Edges connect consecutive `verts` plus the wrap-around closing
/// segment. Edges parallel to `y` are skipped (their endpoints are
/// already covered by the neighbouring edges). Returned x values are
/// sorted ascending; pairing them up `[(x0,x1), (x2,x3), …]` yields
/// the inside intervals at this y by the even-odd rule.
fn polygon_x_at_y(verts: &[(f32, f32)], y: f32) -> Vec<f32> {
    let n = verts.len();
    let mut xs: Vec<f32> = Vec::new();
    for i in 0..n {
        let (x0, y0) = verts[i];
        let (x1, y1) = verts[(i + 1) % n];
        // Half-open at the upper endpoint to avoid double-counting
        // shared anchor y values.
        let (lo_y, hi_y, lo_x, hi_x) = if y0 <= y1 {
            (y0, y1, x0, x1)
        } else {
            (y1, y0, x1, x0)
        };
        if (lo_y - hi_y).abs() < 1e-6 {
            continue; // horizontal edge
        }
        if y < lo_y || y >= hi_y {
            continue;
        }
        let t = (y - lo_y) / (hi_y - lo_y);
        xs.push(lo_x + t * (hi_x - lo_x));
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs
}

/// Pair the sorted x intersections into inside intervals `[(x0,x1),
/// (x2,x3), …]`. Odd counts (numerical edge-grazing) drop the last x.
fn pairs_from_xs(xs: &[f32]) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(xs.len() / 2);
    let mut i = 0;
    while i + 1 < xs.len() {
        out.push((xs[i], xs[i + 1]));
        i += 2;
    }
    out
}

/// Subtract the union of `holes` from each `(left, right)` segment.
/// Both inputs are in spread-coord pt; output preserves left-to-right
/// order. Mirrors the wrap-rect carve loop in
/// `build_perline_wrap_widths` but expressed as a free fn so the
/// polygon path can reuse it.
fn carve_holes(mut segments: Vec<(f32, f32)>, holes: &[(f32, f32)]) -> Vec<(f32, f32)> {
    for (hl, hr) in holes {
        let mut next: Vec<(f32, f32)> = Vec::with_capacity(segments.len() + 1);
        for (a, b) in &segments {
            if *hr <= *a || *hl >= *b {
                next.push((*a, *b));
                continue;
            }
            if hl > a {
                next.push((*a, *hl));
            }
            if hr < b {
                next.push((*hr, *b));
            }
        }
        segments = next;
    }
    segments
}

fn build_perline_wrap_widths(
    em: &StoryEmitter,
    styled_runs: &[idml_text::StyledRun],
    lopts: &mut idml_text::LayoutOptions,
) -> WrapPlan {
    let empty = WrapPlan {
        line_x_shifts_64: Vec::new(),
        twin_after: Vec::new(),
    };
    // Polygon clip per chain frame — enabled when the frame's
    // <PathGeometry> is non-rectangular (e.g. triangle, pentagon).
    // Indexed by frame_idx; `None` means treat the frame as its AABB.
    let chain_polygons: Vec<Option<Vec<(f32, f32)>>> =
        em.chain.iter().map(|f| frame_polygon_spread(f)).collect();
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
    // Matches idml-text's auto-leading default.
    let head_size_pt = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let leading_pt = head_size_pt * 1.2;
    let leading_64 = ((leading_pt * idml_text::shape::ADVANCE_PRECISION).round() as i32).max(1);
    let scalar_width_64 =
        (em.column_width_pt.unwrap_or(0.0) * idml_text::shape::ADVANCE_PRECISION).round() as i32;

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
            (head_size_pt * 0.8 * idml_text::shape::ADVANCE_PRECISION).round() as i32
        };
        let remaining_height_pt = (frame_height_pt
            - frame_first_baseline_64 as f32 / idml_text::shape::ADVANCE_PRECISION)
            .max(0.0);
        let mut n_lines = (remaining_height_pt / leading_pt).ceil() as usize + 1;
        n_lines = n_lines.min(512);
        if n_lines == 0 {
            continue;
        }
        let wraps = &em.chain_wrap_rects[frame_idx];
        let poly = chain_polygons[frame_idx].as_deref();
        // Frames without polygon clip and without any wrap overlap
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
        let frame_legacy = poly.is_none() && !frame_has_wraps;
        for i in 0..n_lines {
            if frame_legacy {
                widths_64.push(scalar_width_64);
                shifts_64.push(0);
                twin_after.push(false);
                continue;
            }
            let baseline_pt = (frame_first_baseline_64 + (i as i32) * leading_64) as f32
                / idml_text::shape::ADVANCE_PRECISION;
            // Line's vertical band in spread coords.
            let line_top = frame_bounds.top + baseline_pt - leading_pt * 0.8;
            let line_bottom = frame_bounds.top + baseline_pt + leading_pt * 0.2;

            let frame_inner_left = frame_left_pt + insets[1];
            let frame_inner_right = frame_right_pt - insets[3];
            // Build the *gap list* of open horizontal segments on
            // this line. For polygon-shaped frames (triangles,
            // pentagons), seed segments from the polygon's interior
            // x-intervals at the baseline so glyph advance never
            // crosses the actual outline. Plain rectangle frames
            // start from the AABB inner range.
            let mut segments: Vec<(f32, f32)> = if let Some(verts) = poly {
                let baseline_y = frame_bounds.top + baseline_pt;
                pairs_from_xs(&polygon_x_at_y(verts, baseline_y))
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
            // Drop segments narrower than the per-line floor.
            const MIN_USABLE_64: i32 = 1536; // 24 pt × 64
            let usable: Vec<(f32, f32)> = segments
                .into_iter()
                .filter(|(a, b)| {
                    let w64 = ((b - a) * idml_text::shape::ADVANCE_PRECISION).round() as i32;
                    w64 >= MIN_USABLE_64
                })
                .collect();
            if usable.is_empty() {
                // No usable segment at this line. Previously the
                // polygon-clipped branch emitted a 1pt sentinel
                // intending to mark the line as "infeasible — break
                // here", but `paragraph_breaker::total_fit` reads
                // that as "every line at this index has ratio < -1",
                // pruning every active node that crosses the
                // sentinel slot. For paragraphs whose content needs
                // more rows than fit *before* the apex (the common
                // case for table-cell-style threaded polygons), the
                // breaker can no longer reach the end-of-paragraph
                // penalty and returns zero breaks — leaving the
                // entire story unrendered. Falling back to
                // `scalar_width_64` at apex rows lets the breaker
                // proceed; any glyph laid out at those y positions
                // falls outside the polygon path and is invisible
                // once `apply_polygon_clip` wraps the frame's
                // commands in a clipping path. The cost is a few
                // extra glyph commands the rasteriser discards.
                widths_64.push(scalar_width_64);
                shifts_64.push(0);
                twin_after.push(false);
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
                let w64 = ((b - a) * idml_text::shape::ADVANCE_PRECISION).round() as i32;
                let shift_pt = (a - frame_inner_left).max(0.0);
                widths_64.push(w64);
                shifts_64.push((shift_pt * idml_text::shape::ADVANCE_PRECISION).round() as i32);
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
fn frame_spread_top_left(b: idml_parse::Bounds, m: Option<[f32; 6]>) -> (f32, f32) {
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
    let Some(id) = layer_ref else {
        return true;
    };
    document
        .container
        .designmap
        .layers
        .iter()
        .find(|l| l.self_id == id)
        .map(|l| l.visible && l.printable)
        .unwrap_or(true)
}

fn page_for_frame(frame: &idml_parse::Bounds, pages: &[PageGeom]) -> Option<usize> {
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

fn pages_overlapping_frame(frame: &idml_parse::Bounds, pages: &[PageGeom]) -> Vec<usize> {
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
    cmyk_xform: Option<&idml_color::IccTransform>,
    drop_shadow: Option<DropShadow>,
) {
    let mut resolved = ResolvedFrame::from_text_frame(frame);
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
            Geometry::Line { p0, p1 } => idml_compose::Rect {
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
    // Q-04: extended GradientFeather (and the rest of FrameEffects) to
    // TextFrame. The host geometry is a rectangular text panel, so we
    // route through the unit-rect path the same way `emit_rectangle_into`
    // does for non-rounded rectangles: intern the unit rect, scale via
    // `Transform::for_rect_in`, flag `effects_unit_normalize` so the
    // effects module knows to convert path-local coordinates from unit
    // space into the frame's actual bounds.
    let (effects_path, effects_xform, effects_unit_normalize) =
        if frame.effects.is_some() {
            if let Geometry::TextFrameRect { rect: r } = &resolved.geometry {
                let (id, _) = page
                    .list
                    .paths
                    .intern(idml_compose::UNIT_RECT_KEY, idml_compose::PathData {
                        segments: vec![
                            idml_compose::PathSegment::MoveTo { x: 0.0, y: 0.0 },
                            idml_compose::PathSegment::LineTo { x: 1.0, y: 0.0 },
                            idml_compose::PathSegment::LineTo { x: 1.0, y: 1.0 },
                            idml_compose::PathSegment::LineTo { x: 0.0, y: 1.0 },
                            idml_compose::PathSegment::Close,
                        ],
                    });
                (Some(id), Transform::for_rect_in(*r, outer), Some(*r))
            } else {
                (None, outer, None)
            }
        } else {
            (None, outer, None)
        };
    if let (Some(path_id), Some(effects)) = (effects_path, frame.effects.as_ref()) {
        crate::module::emit_effects_pre_fill(
            page, effects, path_id, effects_xform, palette, cmyk_xform,
        );
    }
    crate::module::fill_paint_module(
        &resolved, page, palette, cmyk_xform, fallback, outer, None,
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
        None,
        Stroke::new(resolved.effective_stroke_weight()),
    );
    if needs_group {
        pop_blend_group(page);
    }
    let _ = group_bounds;
}

fn emit_oval_into(
    page: &mut BuiltPage,
    oval: &Oval,
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    let mut frame = ResolvedFrame::from_oval(oval);
    let style = crate::module::resolve_applied_style(&frame, document);
    if let Some(s) = &style {
        crate::module::object_style_cascade(&mut frame, s);
    }
    page.stats.frames += 1;
    let outer = frame_outer_transform(page, frame.item_transform);
    let needs_group = frame_needs_blend_group(&frame);
    if needs_group {
        if let Geometry::Oval { rect } = &frame.geometry {
            push_blend_group(
                page,
                *rect,
                outer,
                frame.blend_mode,
                frame_group_opacity(&frame),
            );
        }
    }
    crate::module::drop_shadow_module(
        &frame,
        page,
        palette,
        cmyk_xform,
        None,
        outer,
        oval.stroke_drop_shadow.as_ref(),
    );
    // Q-04: extend GradientFeather / OuterGlow / etc. to Oval. The
    // host geometry is the unit-ellipse path scaled to `rect` via the
    // outer affine, mirroring how `emit_ellipse_transformed` builds
    // the fill itself. `effects_unit_normalize = Some(rect)` flags the
    // effects module to treat path-local coords as unit-ellipse space.
    let (effects_path, effects_xform, effects_unit_normalize) =
        if oval.effects.is_some() {
            if let Geometry::Oval { rect: r } = &frame.geometry {
                let (id, _) = page
                    .list
                    .paths
                    .intern(idml_compose::UNIT_ELLIPSE_KEY, idml_compose::unit_ellipse());
                (Some(id), Transform::for_rect_in(*r, outer), Some(*r))
            } else {
                (None, outer, None)
            }
        } else {
            (None, outer, None)
        };
    if let (Some(pid), Some(effects)) = (effects_path, oval.effects.as_ref()) {
        crate::module::emit_effects_pre_fill(
            page, effects, pid, effects_xform, palette, cmyk_xform,
        );
    }
    crate::module::fill_paint_module(&frame, page, palette, cmyk_xform, fallback, outer, None);
    if let (Some(pid), Some(effects)) = (effects_path, oval.effects.as_ref()) {
        crate::module::emit_effects_post_fill(
            page,
            effects,
            pid,
            effects_xform,
            palette,
            cmyk_xform,
            effects_unit_normalize,
        );
    }
    crate::module::stroke_paint_module(
        &frame,
        page,
        palette,
        cmyk_xform,
        outer,
        None,
        Stroke::new(frame.effective_stroke_weight()),
    );
    if needs_group {
        pop_blend_group(page);
    }
}

fn emit_line_into(
    page: &mut BuiltPage,
    line: &GraphicLine,
    document: &Document,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    let mut resolved = ResolvedFrame::from_graphic_line(line);
    let style = crate::module::resolve_applied_style(&resolved, document);
    if let Some(s) = &style {
        crate::module::object_style_cascade(&mut resolved, s);
    }
    page.stats.frames += 1;
    // GraphicLines without an explicit StrokeColor inherit the
    // document cascade default (Color/Black). Falling back here
    // keeps real-InDesign exports rendering with visible lines —
    // those frequently leave StrokeColor implicit.
    //
    // Routes through the `_dir` variant so `GradientStrokeAngle` /
    // `GradientStrokeLength` on a line-stroke gradient still rotate
    // the gradient line. Lines have no rect bbox, so `path_dims` is
    // `None`; the helper falls back to the unit-rect default centred
    // on (0.5, 0.5) — angle still rotates around that centre.
    let stroke_paint = resolved
        .stroke_color
        .and_then(|id| {
            color_id_to_paint_with_list_dir(
                id,
                palette,
                cmyk_xform,
                &mut page.list,
                resolved.gradient_stroke_angle,
                resolved.gradient_stroke_length,
                None,
            )
        })
        .or_else(|| color_id_to_paint("Color/Black", palette, cmyk_xform))
        .unwrap_or(Paint::Solid(Color::BLACK));
    let stroke_width = resolved.effective_stroke_weight();
    if stroke_width <= 0.0 {
        return;
    }
    // GraphicLine.bounds is in inner coords; ItemTransform maps it
    // to spread coords. Without the transform pass the line draws
    // at its untransformed inner-coord origin (typically (0, 0))
    // and disappears off-page when the spread has any origin offset.
    // The adapter packs endpoints into Geometry::Line in inner
    // coords; we reapply the inner→spread→page math here.
    let spread_bounds = transform_bounds(line.bounds, resolved.item_transform);
    let (ox, oy) = page.spread_origin;
    emit_line(
        spread_bounds.left - ox,
        spread_bounds.top - oy,
        spread_bounds.right - ox,
        spread_bounds.bottom - oy,
        Stroke::new(stroke_width),
        stroke_paint,
        &mut page.list,
    );
}

/// Emit a Rectangle whose Q-11 multi-anchor PathGeometry adapter
/// produced Geometry::Polygon. Mirrors `emit_polygon_into`'s post-
/// resolve sequence: intern the curve, push a blend group when the
/// frame's blend mode is non-trivial, then run fill + stroke modules
/// against the interned path. Skips the corner-radius and effects
/// branches the rectangular path runs because those don't apply to a
/// curved outline.
fn emit_rectangle_polygon_path(
    page: &mut BuiltPage,
    resolved: &ResolvedFrame<'_>,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    page.stats.frames += 1;
    let outer = frame_outer_transform(page, resolved.item_transform);
    let needs_group = frame_needs_blend_group(resolved);
    let bbox = match &resolved.geometry {
        Geometry::Polygon { bbox, .. } => *bbox,
        _ => return,
    };
    if needs_group {
        push_blend_group(
            page,
            bbox,
            outer,
            resolved.blend_mode,
            frame_group_opacity(resolved),
        );
    }
    let path_id = if let Geometry::Polygon {
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
    crate::module::fill_paint_module(
        resolved, page, palette, cmyk_xform, fallback, outer, path_id,
    );
    crate::module::stroke_paint_module(
        resolved,
        page,
        palette,
        cmyk_xform,
        outer,
        path_id,
        Stroke::new(resolved.effective_stroke_weight()),
    );
    if needs_group {
        pop_blend_group(page);
    }
}

fn emit_rectangle_into(
    page: &mut BuiltPage,
    rect: &Rectangle,
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
    drop_shadow: Option<DropShadow>,
) {
    let mut resolved = ResolvedFrame::from_rectangle(rect);
    let style = crate::module::resolve_applied_style(&resolved, document);
    if let Some(s) = &style {
        crate::module::object_style_cascade(&mut resolved, s);
    }
    // Q-11: a Rectangle whose PathGeometry carries more than four
    // anchors is rendered as a curved polygon. `from_rectangle` lifts
    // the geometry to Polygon for those cases; the rounded-corner /
    // effect / stroke-alignment apparatus below assumes Rect so we
    // route the polygon case through the same path emit
    // `emit_polygon_into` uses, then return.
    if matches!(resolved.geometry, Geometry::Polygon { .. }) {
        emit_rectangle_polygon_path(page, &resolved, palette, fallback, cmyk_xform);
        return;
    }
    let Geometry::Rect { rect: r } = resolved.geometry else {
        unreachable!("from_rectangle produces Geometry::Rect after polygon branch");
    };
    page.stats.frames += 1;
    let outer = frame_outer_transform(page, resolved.item_transform);
    let needs_group = frame_needs_blend_group(&resolved);
    if needs_group {
        push_blend_group(
            page,
            r,
            outer,
            resolved.blend_mode,
            frame_group_opacity(&resolved),
        );
    }
    crate::module::drop_shadow_module(
        &resolved,
        page,
        palette,
        cmyk_xform,
        drop_shadow,
        outer,
        rect.stroke_drop_shadow.as_ref(),
    );

    // Rounded-corner Rectangles route fill + stroke through interned
    // paths; non-rounded ones use the geometry's natural primitives.
    // The corner_path module returns `(None, None)` when there's no
    // corner radius, so the same module call covers both cases.
    let corner = crate::module::corner_path_module(&resolved, page);

    // Frame effects (`<*Setting>` elements). Resolve the path id +
    // transform that the rasterizer will stamp under: for rounded
    // rects that's the corner-path interned in inner coords (so the
    // path already carries the rect geometry and the transform is just
    // `outer`); for flat rects we intern the unit rect and let
    // `Transform::for_rect_in` handle the rect → page mapping. The
    // `OuterGlow` fragment of the effect set is emitted *before* the
    // fill so the halo lands behind it; the rest stamp *after* the
    // fill so they composite onto the path's interior.
    // `effects_unit_normalize` flags the unit-rect path so effect
    // helpers know to convert IDML path-local coordinates (e.g. a
    // `<GradientFeatherSetting>`'s `GradientStart`) into unit-rect
    // space. The corner-rounded path is already in path-local coords,
    // so it skips the conversion.
    let (effects_path, effects_xform, effects_unit_normalize) = match corner.fill {
        Some(id) => (id, outer, None),
        None => {
            let (id, _) = page
                .list
                .paths
                .intern(idml_compose::UNIT_RECT_KEY, idml_compose::PathData {
                    segments: vec![
                        idml_compose::PathSegment::MoveTo { x: 0.0, y: 0.0 },
                        idml_compose::PathSegment::LineTo { x: 1.0, y: 0.0 },
                        idml_compose::PathSegment::LineTo { x: 1.0, y: 1.0 },
                        idml_compose::PathSegment::LineTo { x: 0.0, y: 1.0 },
                        idml_compose::PathSegment::Close,
                    ],
                });
            (id, Transform::for_rect_in(r, outer), Some(r))
        }
    };
    if let Some(effects) = rect.effects.as_ref() {
        crate::module::emit_effects_pre_fill(
            page, effects, effects_path, effects_xform, palette, cmyk_xform,
        );
    }

    crate::module::fill_paint_module(
        &resolved, page, palette, cmyk_xform, fallback, outer, corner.fill,
    );

    if let Some(effects) = rect.effects.as_ref() {
        crate::module::emit_effects_post_fill(
            page,
            effects,
            effects_path,
            effects_xform,
            palette,
            cmyk_xform,
            effects_unit_normalize,
        );
    }

    // Stroke needs the IDML stroke style (dash pattern, end-cap/join,
    // miter limit) folded into the `Stroke`. For non-rounded
    // rectangles the stroke also rides an `inset_rect` to honour
    // `StrokeAlignment` — which the geometry adapter doesn't know
    // about, so we compute it here and either pre-intern (rounded)
    // or hand a custom rect to the fallback emit (flat).
    let stroke_width = resolved.effective_stroke_weight();
    let stroke = stroke_for(
        resolved.stroke_type,
        stroke_width,
        resolved.end_cap,
        resolved.end_join,
        resolved.miter_limit,
    );
    if corner.stroke.is_some() {
        crate::module::stroke_paint_module(
            &resolved,
            page,
            palette,
            cmyk_xform,
            outer,
            corner.stroke,
            stroke,
        );
        if needs_group {
            pop_blend_group(page);
        }
        return;
    }
    // Flat rectangle — use the inset rect for stroke-alignment.
    let stroke_offset = stroke_alignment_offset(resolved.stroke_alignment, stroke_width);
    if stroke_width > 0.0 {
        if let Some(paint) = resolved.stroke_color.and_then(|id| {
            color_id_to_paint_with_list_dir(
                id,
                palette,
                cmyk_xform,
                &mut page.list,
                resolved.gradient_stroke_angle,
                resolved.gradient_stroke_length,
                Some((r.w, r.h)),
            )
        }) {
            // Frame opacity is applied at the transparency-group
            // level by the orchestrator; per-paint scaling here
            // would double-apply the alpha.
            emit_stroke_rect_transformed(
                inset_rect(r, stroke_offset),
                outer,
                stroke,
                paint,
                &mut page.list,
            );
        }
    }
    if needs_group {
        pop_blend_group(page);
    }
}

/// Half the stroke width to shift the stroke path by, signed so that
/// positive shrinks inward (Inside alignment) and negative grows
/// outward (Outside alignment). `CenterAlignment` and `None` return 0.
pub(crate) fn stroke_alignment_offset(alignment: Option<&str>, stroke_width: f32) -> f32 {
    match alignment {
        Some("InsideAlignment") => stroke_width * 0.5,
        Some("OutsideAlignment") => -stroke_width * 0.5,
        _ => 0.0,
    }
}

/// Map IDML's `<BlendingSetting BlendMode="...">` enum string to the
/// compose-layer `BlendMode`. Unknown / absent values fall back to
/// Normal. Names mirror Adobe's PDF blend-mode catalogue.
pub(crate) fn blend_mode_from_idml(name: Option<&str>) -> idml_compose::BlendMode {
    use idml_compose::BlendMode;
    match name {
        Some("Multiply") => BlendMode::Multiply,
        Some("Screen") => BlendMode::Screen,
        Some("Overlay") => BlendMode::Overlay,
        Some("Darken") => BlendMode::Darken,
        Some("Lighten") => BlendMode::Lighten,
        Some("ColorDodge") => BlendMode::ColorDodge,
        Some("ColorBurn") => BlendMode::ColorBurn,
        Some("HardLight") => BlendMode::HardLight,
        Some("SoftLight") => BlendMode::SoftLight,
        Some("Difference") => BlendMode::Difference,
        Some("Exclusion") => BlendMode::Exclusion,
        Some("Hue") => BlendMode::Hue,
        Some("Saturation") => BlendMode::Saturation,
        Some("Color") => BlendMode::Color,
        Some("Luminosity") => BlendMode::Luminosity,
        _ => BlendMode::Normal,
    }
}

/// Inset (positive) or outset (negative) all four edges of a rect by
/// `delta`. Used for stroke-alignment shifts on rectangles.
pub(crate) fn inset_rect(r: Rect, delta: f32) -> Rect {
    Rect {
        x: r.x + delta,
        y: r.y + delta,
        w: (r.w - 2.0 * delta).max(0.0),
        h: (r.h - 2.0 * delta).max(0.0),
    }
}

/// Scale a paint's alpha by the IDML `Opacity` percentage. `None` ⇒
/// unchanged. Only solid paints get scaled today; gradient stops
/// would need a per-stop pass that we'll add when frame-level
/// opacity meets a gradient fill in real samples.
///
/// Retained for back-compat but no longer called from the live emit
/// path: frame-level opacity is now applied at the transparency-group
/// composite (`BeginBlendGroup` / `EndBlendGroup`), so per-paint
/// alpha scaling would double-apply the value.
#[allow(dead_code)]
pub(crate) fn apply_opacity(paint: Paint, opacity_pct: Option<f32>) -> Paint {
    let Some(o) = opacity_pct else {
        return paint;
    };
    let scale = (o / 100.0).clamp(0.0, 1.0);
    if (scale - 1.0).abs() < f32::EPSILON {
        return paint;
    }
    match paint {
        Paint::Solid(c) => Paint::Solid(Color::rgba(c.r, c.g, c.b, c.a * scale)),
        other => other,
    }
}

/// Effective corner radius for a Rectangle, considering CornerOption.
/// Returns `Some(radius)` only when the corner-option string names a
/// rounding variant and the radius is positive; otherwise `None` so
/// the renderer takes the cheap unit-rect path.
/// Effective corner radius for a Rectangle, considering CornerOption.
/// Reads the already-resolved fields off `ResolvedFrame` so the
/// corner-path module never imports `Rectangle`. Returns
/// `Some(radius)` only when the option names a rounding variant and
/// the radius is positive; otherwise `None` so the renderer takes
/// the cheap unit-rect path.
pub(crate) fn corner_radius_from(radius: Option<f32>, option: Option<&str>) -> Option<f32> {
    let r = radius?;
    if r <= 0.0 {
        return None;
    }
    match option {
        // The decorative variants (Inverse-Rounded, Inset, Bevel, Fancy)
        // currently fall back to plain Rounded. Replace per-corner-option
        // path emission lands later.
        Some("Rounded")
        | Some("InverseRounded")
        | Some("Inset")
        | Some("Bevel")
        | Some("Fancy") => Some(r),
        _ => None,
    }
}

/// Q-16: resolve the 4 per-corner radii for a Rectangle. Per-corner
/// `CornerSpec` wins when set; otherwise fall back to the legacy
/// `corner_radius` / `corner_option` pair. Returns `[tl, tr, br, bl]`
/// — clockwise from top-left to match `rounded_rect_path_per_corner`'s
/// walk. `None` means "this corner is square" (no rounding); a corner
/// with positive radius but a `Some(CornerOption::None)` override
/// also clamps to square.
pub(crate) fn per_corner_radii(
    corner_radius: Option<f32>,
    corner_option: Option<&str>,
    corners: &[idml_parse::CornerSpec; 4],
) -> [Option<f32>; 4] {
    let fallback = corner_radius_from(corner_radius, corner_option);
    let mut out = [None; 4];
    for (i, spec) in corners.iter().enumerate() {
        // Decide rounding-on-off for this corner:
        //   explicit Some(option) wins; absent option falls through to
        //   the global `corner_option`.
        let rounds = match spec.option {
            Some(opt) => opt.rounds(),
            None => corner_option
                .map(|s| !matches!(s, "None" | "Square"))
                .unwrap_or(false),
        };
        if !rounds {
            continue;
        }
        let r = spec
            .radius
            .or(corner_radius)
            .filter(|r| *r > 0.0);
        // When the per-corner spec carries an option but no explicit
        // radius, inherit from the global fallback. When no fallback
        // either, the corner squares back off via `out[i] = None`.
        out[i] = r.or(fallback);
    }
    // Fast path: if no per-corner override touched the array, fall
    // back to the symmetric fallback for all four corners.
    if corners.iter().all(|s| s.option.is_none() && s.radius.is_none()) {
        return [fallback, fallback, fallback, fallback];
    }
    out
}

/// Build a rounded-rect path with cubic-Bezier quarter-circle corners
/// (control offset = `radius * 0.5523`). The path is emitted in the
/// rectangle's *inner* coordinate system (same coords as `rect.x` /
/// `rect.y`); the renderer's `outer` transform handles spread-origin
/// and ItemTransform composition the same way it does for polygons.
/// Walks clockwise from the top edge.
pub(crate) fn rounded_rect_path(rect: Rect, radius: f32) -> idml_compose::PathData {
    rounded_rect_path_per_corner(rect, [Some(radius); 4])
}

/// Q-16: rounded-rect path with per-corner radii. The array order is
/// `[top_left, top_right, bottom_right, bottom_left]` — matches
/// `Rectangle::corners` and the clockwise walk this function emits.
/// `None` (or `Some(0)`) for a corner produces a sharp 90° angle.
/// Each radius is clamped to `min(width/2, height/2)` independently
/// so a tall narrow rect with one large corner still fits.
pub(crate) fn rounded_rect_path_per_corner(
    rect: Rect,
    radii: [Option<f32>; 4],
) -> idml_compose::PathData {
    use idml_compose::PathSegment::*;
    let max_r = rect.w.min(rect.h) * 0.5;
    let r = |i: usize| -> f32 {
        radii[i].map(|v| v.min(max_r).max(0.0)).unwrap_or(0.0)
    };
    let (tl, tr, br, bl) = (r(0), r(1), r(2), r(3));
    let l = rect.x;
    let t = rect.y;
    let right = rect.x + rect.w;
    let bot = rect.y + rect.h;
    // Cubic-Bezier control offset for a quarter-circle: KAPPA × radius.
    const KAPPA: f32 = 0.552_284_8;
    let mut segments = Vec::with_capacity(13);
    // Start at the top edge, just past the top-left corner's rounding.
    segments.push(MoveTo { x: l + tl, y: t });
    // Top edge → top-right corner.
    segments.push(LineTo { x: right - tr, y: t });
    if tr > 0.0 {
        let k = tr * KAPPA;
        segments.push(CubicTo {
            cx1: right - tr + k,
            cy1: t,
            cx2: right,
            cy2: t + tr - k,
            x: right,
            y: t + tr,
        });
    }
    // Right edge → bottom-right corner.
    segments.push(LineTo { x: right, y: bot - br });
    if br > 0.0 {
        let k = br * KAPPA;
        segments.push(CubicTo {
            cx1: right,
            cy1: bot - br + k,
            cx2: right - br + k,
            cy2: bot,
            x: right - br,
            y: bot,
        });
    }
    // Bottom edge → bottom-left corner.
    segments.push(LineTo { x: l + bl, y: bot });
    if bl > 0.0 {
        let k = bl * KAPPA;
        segments.push(CubicTo {
            cx1: l + bl - k,
            cy1: bot,
            cx2: l,
            cy2: bot - bl + k,
            x: l,
            y: bot - bl,
        });
    }
    // Left edge → top-left corner (closes back to MoveTo's point).
    segments.push(LineTo { x: l, y: t + tl });
    if tl > 0.0 {
        let k = tl * KAPPA;
        segments.push(CubicTo {
            cx1: l,
            cy1: t + tl - k,
            cx2: l + tl - k,
            cy2: t,
            x: l + tl,
            y: t,
        });
    }
    segments.push(Close);
    idml_compose::PathData { segments }
}

/// Resolve, decode, and emit a placed image for a rectangle. Skips
/// silently if `assets` is unset, the resolver returns `None`, or
/// decoding fails — IDMLs without their linked assets should still
/// produce a usable render of the surrounding geometry.
///
/// Two placement paths:
///   1. *Inner* `<Image ItemTransform="...">` present (the
///      InDesign-export shape). The image's pixel rect (0..w, 0..h)
///      maps through that transform into the frame's inner coord
///      space, then through the frame's ItemTransform into spread
///      coords. The image is then *clipped* to the frame's path so
///      cropping / partial-frame placements (a thin slice, a square
///      crop, etc.) match InDesign.
///   2. No inner transform (synthetic IDMLs that omit it). Fall
///      back to the legacy "stretch image to frame bounds — minus
///      `<FrameFittingOption>` crops" path. No clip is needed
///      because the image already covers exactly the frame's AABB.
fn emit_rectangle_image(
    page: &mut BuiltPage,
    rect: &Rectangle,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, idml_compose::ImageId>,
    decoded_cache: &mut HashMap<String, idml_compose::DecodedImage>,
) {
    // Routing: a `<Image>`-bearing frame whose link is *missing*
    // gets InDesign's 50% grey + diagonal-X placeholder. A link that
    // resolves but whose payload our decoder can't handle (Q-14) is
    // a different case — InDesign still rasterises it, so we fall
    // through to the frame's intrinsic FillColor instead of stamping
    // a "missing image" badge over what should be real content.
    //
    // Q-03: inline base64 `<Image><Contents>` bytes take precedence
    // over `LinkResourceURI` — when the IDML embeds the JPEG directly
    // we decode straight from those bytes regardless of whether an
    // asset resolver is wired up.
    let resolved = if let Some(bytes) = rect.image_bytes.as_deref() {
        resolve_inline_image_bytes(bytes, &mut page.list, page_image_cache, decoded_cache)
    } else {
        match rect.image_link.as_deref() {
            Some(uri) => resolve_image_id(uri, options, &mut page.list, page_image_cache, decoded_cache),
            None => ImageResolution::LinkMissing,
        }
    };
    let outer = frame_outer_transform(page, rect.item_transform);
    let (id, img_w, img_h) = match resolved {
        ImageResolution::Resolved(id, w, h) => (id, w, h),
        ImageResolution::DecodeFailed => return,
        ImageResolution::LinkMissing => {
            // Q-06: inline `<PDF>` content we can't decode → fall
            // through to the frame's intrinsic FillColor (already
            // emitted by the earlier shape-fill pass) rather than
            // stamping the grey-X missing-image placeholder over it.
            if rect.has_image_element
                && !rect.has_inline_pdf
                && options.missing_image_placeholder
            {
                emit_rectangle_missing_image_placeholder(page, rect, outer);
            }
            return;
        }
    };

    if let Some(image_t) = rect.image_item_transform {
        // Path 1: honour the inner Image[ItemTransform]. The image's
        // natural pixel rect (0..w, 0..h) — IDML treats placed-image
        // pixels as 1pt at 72ppi — maps through `image_t` into
        // frame-inner coords, then through `outer` (= page_origin ·
        // frame.ItemTransform) into spread → page coords.
        //
        // `Transform::for_rect_in(rect, t)` builds
        //   t · scale(rect.w, rect.h) · translate(rect.x, rect.y)
        // so passing rect=(0,0,w,h) plus a composed `outer ∘ image_t`
        // gives us exactly the placement we need.
        let composed = outer.compose(&Transform(image_t));
        let img_rect = Rect {
            x: 0.0,
            y: 0.0,
            w: img_w,
            h: img_h,
        };
        // Clip to the frame's rectangular path (in inner coords).
        // We use the rectangle's `bounds` AABB: IDML rectangles are
        // axis-aligned in inner space by definition, so the AABB
        // equals the path. Any rotation/shear lives on `outer`,
        // which we share with the image emission below. Polygon-
        // hosted images (curved frames) aren't part of this slice.
        let clip_rect = idml_compose::Rect {
            x: rect.bounds.left,
            y: rect.bounds.top,
            w: rect.bounds.width(),
            h: rect.bounds.height(),
        };
        emit_clipped_image(&mut page.list, clip_rect, outer, img_rect, composed, id);
    } else {
        // Path 2: legacy synthetic-IDML placement. No inner
        // transform ⇒ fit the image to the frame's bounds (minus
        // FrameFitting crops). No clip — the image already
        // occupies exactly the rect.
        let frame_left = rect.bounds.left;
        let frame_top = rect.bounds.top;
        let frame_w = rect.bounds.width();
        let frame_h = rect.bounds.height();
        let (left_crop, top_crop, right_crop, bottom_crop) = rect
            .frame_fitting
            .as_ref()
            .map(|f| {
                (
                    f.left_crop.unwrap_or(0.0),
                    f.top_crop.unwrap_or(0.0),
                    f.right_crop.unwrap_or(0.0),
                    f.bottom_crop.unwrap_or(0.0),
                )
            })
            .unwrap_or((0.0, 0.0, 0.0, 0.0));
        let r = Rect {
            x: frame_left + left_crop,
            y: frame_top + top_crop,
            w: (frame_w - left_crop - right_crop).max(0.0),
            h: (frame_h - top_crop - bottom_crop).max(0.0),
        };
        idml_compose::emit_image_at(r, outer, id, &mut page.list);
    }
}

/// Polygon-hosted placed image. Mirrors [`emit_rectangle_image`] but
/// uses the polygon's curved `PathPointType` anchors as the clip
/// shape so the image hugs the polygon's outline rather than its
/// bounding rectangle. When the polygon has no anchors (synthetic
/// IDMLs declaring a polygon via `GeometricBounds` only), the
/// rectangle path falls through to a flat AABB clip — visually
/// identical to the rectangle case.
fn emit_polygon_image(
    page: &mut BuiltPage,
    poly: &Polygon,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, idml_compose::ImageId>,
    decoded_cache: &mut HashMap<String, idml_compose::DecodedImage>,
) {
    let resolved = if let Some(bytes) = poly.image_bytes.as_deref() {
        resolve_inline_image_bytes(bytes, &mut page.list, page_image_cache, decoded_cache)
    } else {
        match poly.image_link.as_deref() {
            Some(uri) => resolve_image_id(uri, options, &mut page.list, page_image_cache, decoded_cache),
            None => ImageResolution::LinkMissing,
        }
    };
    let outer = frame_outer_transform(page, poly.item_transform);
    let (id, img_w, img_h) = match resolved {
        ImageResolution::Resolved(id, w, h) => (id, w, h),
        ImageResolution::DecodeFailed => return,
        ImageResolution::LinkMissing => {
            if poly.has_image_element
                && !poly.has_inline_pdf
                && options.missing_image_placeholder
            {
                emit_polygon_missing_image_placeholder(page, poly, outer);
            }
            return;
        }
    };

    // Build (or reuse) the polygon's clip path. Falls back to the
    // bounds AABB when the polygon carries no Bezier anchors. Honours
    // `subpath_open` so open contours don't get auto-closed when used
    // as an image clip (P-15).
    let clip_path_id = if !poly.anchors.is_empty() {
        let path = polygon_path_from_anchors_with_open(
            &poly.anchors,
            &poly.subpath_starts,
            &poly.subpath_open,
        );
        let cache_key = match poly.self_id.as_deref() {
            Some(sid) => fnv_1a_u64(sid.as_bytes()),
            None => path_signature(&poly.anchors),
        };
        let (id, _) = page.list.paths.intern(cache_key, path);
        id
    } else {
        // Anchor-less polygon: synthesise the AABB unit rect path
        // (same key as rectangles use, so both share one entry).
        const CLIP_UNIT_RECT_KEY: u64 = 0x1d_4c_69_70_5f_72_65_63;
        let path = idml_compose::PathData {
            segments: vec![
                idml_compose::PathSegment::MoveTo { x: 0.0, y: 0.0 },
                idml_compose::PathSegment::LineTo { x: 1.0, y: 0.0 },
                idml_compose::PathSegment::LineTo { x: 1.0, y: 1.0 },
                idml_compose::PathSegment::LineTo { x: 0.0, y: 1.0 },
                idml_compose::PathSegment::Close,
            ],
        };
        let (id, _) = page.list.paths.intern(CLIP_UNIT_RECT_KEY, path);
        id
    };

    // Pick a clip transform: anchor-bearing polygons keep their path
    // in inner coords (already in the right space) so `outer` maps
    // directly. Anchor-less polygons need the unit-rect-to-bounds
    // scale baked in — same shape as `emit_clipped_image`.
    let clip_transform = if !poly.anchors.is_empty() {
        outer
    } else {
        let clip_rect = idml_compose::Rect {
            x: poly.bounds.left,
            y: poly.bounds.top,
            w: poly.bounds.width(),
            h: poly.bounds.height(),
        };
        Transform::for_rect_in(clip_rect, outer)
    };

    let img_rect = Rect {
        x: 0.0,
        y: 0.0,
        w: img_w,
        h: img_h,
    };
    let image_transform = if let Some(image_t) = poly.image_item_transform {
        outer.compose(&Transform(image_t))
    } else {
        // Stretch the image across the polygon's bounds (same fallback
        // as the synthetic rectangle case).
        let frame_rect = Rect {
            x: poly.bounds.left,
            y: poly.bounds.top,
            w: poly.bounds.width(),
            h: poly.bounds.height(),
        };
        // Compose the bounds-scale into outer so emit_image_under_clip
        // ends up with `outer ∘ scale(w,h) ∘ translate(left,top)`.
        Transform::for_rect_in(frame_rect, outer)
    };
    // When no inner image transform is present, `image_transform`
    // already encodes the for_rect_in math; for_rect_in below would
    // double-scale. Branch: pass a unit rect so emit_image_under_clip
    // doesn't multiply by img_w/img_h again.
    if poly.image_item_transform.is_some() {
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            img_rect,
            image_transform,
            id,
        );
    } else {
        let unit = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            unit,
            image_transform,
            id,
        );
    }
}

/// Mirror of `emit_polygon_image` for `<Oval>` frames hosting placed
/// images. The clip path is the unit ellipse (interned at a stable
/// key so multiple ovals share the same path); the image fits the
/// oval's bounds unless the inner `<Image ItemTransform>` overrides
/// it (P-16).
fn emit_oval_image(
    page: &mut BuiltPage,
    oval: &Oval,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, idml_compose::ImageId>,
    decoded_cache: &mut HashMap<String, idml_compose::DecodedImage>,
) {
    let resolved = if let Some(bytes) = oval.image_bytes.as_deref() {
        resolve_inline_image_bytes(bytes, &mut page.list, page_image_cache, decoded_cache)
    } else {
        match oval.image_link.as_deref() {
            Some(uri) => resolve_image_id(uri, options, &mut page.list, page_image_cache, decoded_cache),
            None => ImageResolution::LinkMissing,
        }
    };
    let outer = frame_outer_transform(page, oval.item_transform);
    let (id, img_w, img_h) = match resolved {
        ImageResolution::Resolved(id, w, h) => (id, w, h),
        ImageResolution::DecodeFailed => return,
        ImageResolution::LinkMissing => {
            if oval.has_image_element
                && !oval.has_inline_pdf
                && options.missing_image_placeholder
            {
                emit_oval_missing_image_placeholder(page, oval, outer);
            }
            return;
        }
    };

    // Clip to the oval's parametric ellipse (unit-rect-scaled to the
    // frame's bounds via the outer affine). UNIT_ELLIPSE_KEY is the
    // same interner key the fill / stroke paths use, so the path is
    // shared across all ovals.
    let bounds = oval.bounds;
    let clip_rect = idml_compose::Rect {
        x: bounds.left,
        y: bounds.top,
        w: bounds.width(),
        h: bounds.height(),
    };
    let (clip_path_id, _) = page
        .list
        .paths
        .intern(idml_compose::UNIT_ELLIPSE_KEY, unit_ellipse_path());
    let clip_transform = Transform::for_rect_in(clip_rect, outer);

    let img_rect = Rect {
        x: 0.0,
        y: 0.0,
        w: img_w,
        h: img_h,
    };
    let image_transform = if let Some(image_t) = oval.image_item_transform {
        outer.compose(&Transform(image_t))
    } else {
        Transform::for_rect_in(clip_rect, outer)
    };
    if oval.image_item_transform.is_some() {
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            img_rect,
            image_transform,
            id,
        );
    } else {
        let unit = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        emit_image_under_clip(
            &mut page.list,
            clip_path_id,
            clip_transform,
            unit,
            image_transform,
            id,
        );
    }
}

/// Approximate a unit ellipse with four cubic Bezier curves (the
/// standard 0.5522847 control-point distance for a circle). Returns
/// a `PathData` ready to intern under `UNIT_ELLIPSE_KEY`.
fn unit_ellipse_path() -> idml_compose::PathData {
    use idml_compose::PathSegment;
    // Kappa for circular Bezier approximation.
    const K: f32 = 0.5522847498307933;
    // Unit ellipse in the [0,1]×[0,1] rect: center (0.5, 0.5),
    // radius 0.5. Each quadrant is one CubicTo.
    let cx = 0.5;
    let cy = 0.5;
    let rx = 0.5;
    let ry = 0.5;
    let kx = rx * K;
    let ky = ry * K;
    idml_compose::PathData {
        segments: vec![
            PathSegment::MoveTo {
                x: cx + rx,
                y: cy,
            },
            PathSegment::CubicTo {
                cx1: cx + rx,
                cy1: cy + ky,
                cx2: cx + kx,
                cy2: cy + ry,
                x: cx,
                y: cy + ry,
            },
            PathSegment::CubicTo {
                cx1: cx - kx,
                cy1: cy + ry,
                cx2: cx - rx,
                cy2: cy + ky,
                x: cx - rx,
                y: cy,
            },
            PathSegment::CubicTo {
                cx1: cx - rx,
                cy1: cy - ky,
                cx2: cx - kx,
                cy2: cy - ry,
                x: cx,
                y: cy - ry,
            },
            PathSegment::CubicTo {
                cx1: cx + kx,
                cy1: cy - ry,
                cx2: cx + rx,
                cy2: cy - ky,
                x: cx + rx,
                y: cy,
            },
            PathSegment::Close,
        ],
    }
}

/// Missing-image placeholder for `<Oval>` (P-16). Stamps the 50% grey
/// fill clipped to the oval's ellipse, plus the diagonal-X strokes
/// across the bounding rect — the same visual the Rectangle path
/// emits, with the elliptical clip applied so the placeholder reads
/// as a placeholder oval rather than a placeholder square.
fn emit_oval_missing_image_placeholder(
    page: &mut BuiltPage,
    oval: &Oval,
    outer: Transform,
) {
    let bounds = oval.bounds;
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return;
    }
    let grey = Paint::Solid(Color::rgba(
        PLACEHOLDER_FILL_RGB,
        PLACEHOLDER_FILL_RGB,
        PLACEHOLDER_FILL_RGB,
        1.0,
    ));
    let rect = idml_compose::Rect {
        x: bounds.left,
        y: bounds.top,
        w: bounds.width(),
        h: bounds.height(),
    };
    idml_compose::emit_ellipse_transformed(rect, outer, grey, &mut page.list);
    let stroke = idml_compose::Stroke::new(PLACEHOLDER_X_STROKE_PT);
    let dark = Paint::Solid(Color::rgba(
        PLACEHOLDER_X_RGB,
        PLACEHOLDER_X_RGB,
        PLACEHOLDER_X_RGB,
        1.0,
    ));
    emit_diagonal_under_transform(
        &mut page.list,
        bounds.left,
        bounds.top,
        bounds.right,
        bounds.bottom,
        outer,
        stroke,
        dark,
    );
    emit_diagonal_under_transform(
        &mut page.list,
        bounds.right,
        bounds.top,
        bounds.left,
        bounds.bottom,
        outer,
        stroke,
        dark,
    );
}

/// Push a rectangular clip path, emit an image, then pop the clip.
/// `clip_rect` is the frame's inner-coord AABB; `clip_outer` is the
/// frame's outer transform (page_origin · ItemTransform).
/// `image_rect` is `(0, 0, img_w, img_h)` and `image_transform` is
/// the composed `outer ∘ image_item_transform`. Sharing `outer` for
/// both keeps clip and image in lockstep when the frame rotates —
/// the unit-rect clip turns into the host's rotated quad under
/// `clip_outer`, which is the right behaviour for axis-aligned
/// rectangles regardless of the host's rotation.
fn emit_clipped_image(
    list: &mut idml_compose::DisplayList,
    clip_rect: idml_compose::Rect,
    clip_outer: Transform,
    image_rect: idml_compose::Rect,
    image_transform: Transform,
    image_id: idml_compose::ImageId,
) {
    use idml_compose::PathSegment;
    // Unit-rect path interned under a stable key so multiple clipped-
    // image emissions share the same entry in the path buffer.
    const CLIP_UNIT_RECT_KEY: u64 = 0x1d_4c_69_70_5f_72_65_63; // "idClip_rec"
    let path = idml_compose::PathData {
        segments: vec![
            PathSegment::MoveTo { x: 0.0, y: 0.0 },
            PathSegment::LineTo { x: 1.0, y: 0.0 },
            PathSegment::LineTo { x: 1.0, y: 1.0 },
            PathSegment::LineTo { x: 0.0, y: 1.0 },
            PathSegment::Close,
        ],
    };
    let (clip_path_id, _) = list.paths.intern(CLIP_UNIT_RECT_KEY, path);
    let clip_transform = Transform::for_rect_in(clip_rect, clip_outer);
    emit_image_under_clip(
        list,
        clip_path_id,
        clip_transform,
        image_rect,
        image_transform,
        image_id,
    );
}

/// Push an arbitrary clip path, emit an image, then pop. Splits the
/// PushClip / Image / PopClip emission off `emit_clipped_image` so
/// the polygon-hosted image variant (used when the host is a curved
/// `<Polygon>` frame) can supply its own pre-interned path.
fn emit_image_under_clip(
    list: &mut idml_compose::DisplayList,
    clip_path_id: idml_compose::PathId,
    clip_transform: Transform,
    image_rect: idml_compose::Rect,
    image_transform: Transform,
    image_id: idml_compose::ImageId,
) {
    use idml_compose::DisplayCommand;
    list.push(DisplayCommand::PushClip {
        path_id: clip_path_id,
        transform: clip_transform,
    });
    let img_transform = Transform::for_rect_in(image_rect, image_transform);
    list.push(DisplayCommand::Image {
        image_id,
        transform: img_transform,
    });
    list.push(DisplayCommand::PopClip(Transform::IDENTITY));
}

/// Resolve a `LinkResourceURI` to a renderer `ImageId` plus its
/// natural pixel dimensions, threading through both the per-page
/// `ImageId` cache and the renderer-scoped decoded-bytes cache.
/// Returns `None` for any failure along the resolver / decode chain
/// (no resolver, resolver miss, undecodable bytes, zero-pixel image)
/// so callers can fall back to a missing-image placeholder.
/// Outcome of `resolve_image_id`. `LinkMissing` means the IDML
/// referenced a link the asset resolver couldn't find (typical Envato
/// template placeholder — InDesign stamps a grey-X placeholder over
/// these). `DecodeFailed` means the resolver returned bytes our
/// decoder can't handle (oversized JPEG, unsupported PSD layers,
/// streaming-only formats); in that case InDesign would still
/// rasterise the actual content, so falling back to the
/// missing-image placeholder is worse than emitting the frame's
/// intrinsic FillColor (Q-14).
enum ImageResolution {
    Resolved(idml_compose::ImageId, f32, f32),
    DecodeFailed,
    LinkMissing,
}

fn resolve_image_id(
    uri: &str,
    options: &PipelineOptions,
    list: &mut idml_compose::DisplayList,
    page_image_cache: &mut HashMap<String, idml_compose::ImageId>,
    decoded_cache: &mut HashMap<String, idml_compose::DecodedImage>,
) -> ImageResolution {
    let id = match page_image_cache.get(uri).copied() {
        Some(id) => id,
        None => {
            let decoded = if let Some(d) = decoded_cache.get(uri) {
                d.clone()
            } else {
                let Some(resolver) = options.assets else {
                    return ImageResolution::LinkMissing;
                };
                let Some(bytes) = resolver.resolve_image(uri) else {
                    tracing::warn!(uri, "image resolver returned no bytes; skipping");
                    return ImageResolution::LinkMissing;
                };
                let Some(d) = decode_image_bytes(bytes.as_ref()) else {
                    tracing::warn!(uri, "image decode failed; skipping");
                    return ImageResolution::DecodeFailed;
                };
                decoded_cache.insert(uri.to_string(), d.clone());
                d
            };
            let id = list.push_image(decoded);
            page_image_cache.insert(uri.to_string(), id);
            id
        }
    };
    let (img_w, img_h) = match list.image(id) {
        Some(d) => (d.width as f32, d.height as f32),
        None => return ImageResolution::DecodeFailed,
    };
    if img_w <= 0.0 || img_h <= 0.0 {
        return ImageResolution::DecodeFailed;
    }
    ImageResolution::Resolved(id, img_w, img_h)
}

/// Q-03: route inline base64 image bytes (the `<Contents>` payload
/// captured by the parser) through the same per-page + decoded
/// caches `resolve_image_id` uses. Cache key is the bytes' allocation
/// address — stable across reuses inside a single render pass, and
/// distinct per frame so two Rectangles with the same inline image
/// share the decoded result.
fn resolve_inline_image_bytes(
    bytes: &[u8],
    list: &mut idml_compose::DisplayList,
    page_image_cache: &mut HashMap<String, idml_compose::ImageId>,
    decoded_cache: &mut HashMap<String, idml_compose::DecodedImage>,
) -> ImageResolution {
    let key = format!("inline:{:p}:{}", bytes.as_ptr(), bytes.len());
    let id = match page_image_cache.get(&key).copied() {
        Some(id) => id,
        None => {
            let decoded = if let Some(d) = decoded_cache.get(&key) {
                d.clone()
            } else {
                let Some(d) = decode_image_bytes(bytes) else {
                    tracing::warn!(
                        len = bytes.len(),
                        "inline image decode failed; skipping"
                    );
                    return ImageResolution::DecodeFailed;
                };
                decoded_cache.insert(key.clone(), d.clone());
                d
            };
            let id = list.push_image(decoded);
            page_image_cache.insert(key, id);
            id
        }
    };
    let (img_w, img_h) = match list.image(id) {
        Some(d) => (d.width as f32, d.height as f32),
        None => return ImageResolution::DecodeFailed,
    };
    if img_w <= 0.0 || img_h <= 0.0 {
        return ImageResolution::DecodeFailed;
    }
    ImageResolution::Resolved(id, img_w, img_h)
}

/// 50% grey fill + two 1.5pt diagonal stroke lines stamped over a
/// rectangle's path, matching InDesign's placeholder visual for image
/// frames whose `LinkResourceURI` doesn't resolve. The fill replaces
/// the host frame's normal paint (rectangles already drew their fill
/// in `emit_rectangle_into`; the placeholder paints on top because
/// the missing image would have done the same).
fn emit_rectangle_missing_image_placeholder(
    page: &mut BuiltPage,
    rect: &Rectangle,
    outer: Transform,
) {
    let r = idml_compose::Rect {
        x: rect.bounds.left,
        y: rect.bounds.top,
        w: rect.bounds.width(),
        h: rect.bounds.height(),
    };
    if r.w <= 0.0 || r.h <= 0.0 {
        return;
    }
    let grey = Paint::Solid(Color::rgba(
        PLACEHOLDER_FILL_RGB,
        PLACEHOLDER_FILL_RGB,
        PLACEHOLDER_FILL_RGB,
        1.0,
    ));
    idml_compose::emit_rect_transformed(r, outer, grey, &mut page.list);
    let stroke = idml_compose::Stroke::new(PLACEHOLDER_X_STROKE_PT);
    let dark = Paint::Solid(Color::rgba(
        PLACEHOLDER_X_RGB,
        PLACEHOLDER_X_RGB,
        PLACEHOLDER_X_RGB,
        1.0,
    ));
    // Diagonals drawn in inner coords; `outer` carries the
    // page-origin + frame ItemTransform so they rotate / shear with
    // the host frame.
    emit_diagonal_under_transform(
        &mut page.list,
        rect.bounds.left,
        rect.bounds.top,
        rect.bounds.right,
        rect.bounds.bottom,
        outer,
        stroke,
        dark,
    );
    emit_diagonal_under_transform(
        &mut page.list,
        rect.bounds.right,
        rect.bounds.top,
        rect.bounds.left,
        rect.bounds.bottom,
        outer,
        stroke,
        dark,
    );
}

/// Polygon analogue of [`emit_rectangle_missing_image_placeholder`].
/// Reuses the polygon's curved path (or falls back to AABB when the
/// polygon was declared from `GeometricBounds` only) so the
/// placeholder hugs the polygon outline.
fn emit_polygon_missing_image_placeholder(
    page: &mut BuiltPage,
    poly: &Polygon,
    outer: Transform,
) {
    use idml_compose::DisplayCommand;
    let bounds = poly.bounds;
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return;
    }
    let grey = Paint::Solid(Color::rgba(
        PLACEHOLDER_FILL_RGB,
        PLACEHOLDER_FILL_RGB,
        PLACEHOLDER_FILL_RGB,
        1.0,
    ));
    if !poly.anchors.is_empty() {
        let path = polygon_path_from_anchors_with_open(
            &poly.anchors,
            &poly.subpath_starts,
            &poly.subpath_open,
        );
        let cache_key = match poly.self_id.as_deref() {
            Some(sid) => fnv_1a_u64(sid.as_bytes()),
            None => path_signature(&poly.anchors),
        };
        let (path_id, _) = page.list.paths.intern(cache_key, path);
        page.list.push(DisplayCommand::FillPath {
            path_id,
            paint: grey,
            transform: outer,
        });
    } else {
        let r = idml_compose::Rect {
            x: bounds.left,
            y: bounds.top,
            w: bounds.width(),
            h: bounds.height(),
        };
        idml_compose::emit_rect_transformed(r, outer, grey, &mut page.list);
    }
    let stroke = idml_compose::Stroke::new(PLACEHOLDER_X_STROKE_PT);
    let dark = Paint::Solid(Color::rgba(
        PLACEHOLDER_X_RGB,
        PLACEHOLDER_X_RGB,
        PLACEHOLDER_X_RGB,
        1.0,
    ));
    emit_diagonal_under_transform(
        &mut page.list,
        bounds.left,
        bounds.top,
        bounds.right,
        bounds.bottom,
        outer,
        stroke,
        dark,
    );
    emit_diagonal_under_transform(
        &mut page.list,
        bounds.right,
        bounds.top,
        bounds.left,
        bounds.bottom,
        outer,
        stroke,
        dark,
    );
}

/// Push a `StrokePath` for a single line segment whose endpoints live
/// in inner-frame coords. The segment is interned as an anonymous
/// path (lines aren't naturally interned by [`emit_line`] either)
/// and stamped through `outer` so it picks up the frame's
/// ItemTransform / page-origin shift.
fn emit_diagonal_under_transform(
    list: &mut idml_compose::DisplayList,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    outer: Transform,
    stroke: idml_compose::Stroke,
    paint: Paint,
) {
    use idml_compose::{DisplayCommand, PathData, PathSegment};
    let path = PathData {
        segments: vec![
            PathSegment::MoveTo { x: x1, y: y1 },
            PathSegment::LineTo { x: x2, y: y2 },
        ],
    };
    let path_id = list.paths.push_anon(path);
    list.push(DisplayCommand::StrokePath {
        path_id,
        paint,
        stroke,
        transform: outer,
    });
}

/// Detect PostScript / EPS magic in the first few bytes of a resolved
/// image buffer. EPS streams start with `%!PS-Adobe` (or `%!PS`); the
/// `image` crate can't decode them, so the caller falls back to the
/// missing-image placeholder rather than emit nothing (P-14).
fn is_eps_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%!PS")
}

/// Decode raw image bytes to RGBA8. Routes oversized JPEGs through
/// `jpeg-decoder`'s DCT scaling so we never materialise a multi-
/// hundred-MB RGBA8 buffer; everything else (PNG / WebP / small JPEGs)
/// goes through `image::load_from_memory`. Returns `None` for any
/// decode or buffer-shape failure — including EPS / PostScript
/// streams, which would need a Ghostscript sidecar to rasterise
/// (deferred, see `docs/plan.md` Phase 4).
fn decode_image_bytes(bytes: &[u8]) -> Option<idml_compose::DecodedImage> {
    decode_image_bytes_with_target_max(bytes, DECODE_MAX_RASTER_PX)
}

/// Same as [`decode_image_bytes`] but with a caller-supplied
/// longest-edge cap. Used by the streaming JPEG path and by the
/// fallback retry on decode failure. JPEGs above the cap are
/// decoded via `jpeg-decoder` with DCT scaling chosen so the longest
/// edge ends up ≤ `max_px`; other formats and small JPEGs fall
/// through to `image::load_from_memory`.
fn decode_image_bytes_with_target_max(
    bytes: &[u8],
    max_px: u32,
) -> Option<idml_compose::DecodedImage> {
    if is_eps_magic(bytes) {
        tracing::warn!("EPS / PostScript image detected; emitting missing-image placeholder");
        return None;
    }
    if is_jpeg_magic(bytes) {
        if let Some((src_w, src_h)) = peek_jpeg_dimensions(bytes) {
            if src_w.max(src_h) > max_px {
                if let Some(d) = decode_jpeg_scaled(bytes, max_px) {
                    return Some(d);
                }
                tracing::debug!(
                    src_w,
                    src_h,
                    max_px,
                    "streaming JPEG decoder rejected oversized payload; falling back to image crate"
                );
            }
        }
    }
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(idml_compose::DecodedImage {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

fn is_jpeg_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
}

/// Cheap pre-flight: read just the JPEG headers via `jpeg-decoder` to
/// discover declared dimensions without decoding pixel data. Returns
/// `None` if the headers are malformed or the file isn't a JPEG.
fn peek_jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut decoder = jpeg_decoder::Decoder::new(bytes);
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    Some((info.width as u32, info.height as u32))
}

/// Decode a JPEG via `jpeg-decoder`, using its DCT-scaled output mode
/// to land the longest edge ≤ `max_px`. Scaling is restricted to the
/// JPEG-native factors (1, 1/2, 1/4, 1/8); the decoder picks the
/// smallest factor whose output is ≥ the requested target.
fn decode_jpeg_scaled(bytes: &[u8], max_px: u32) -> Option<idml_compose::DecodedImage> {
    use jpeg_decoder::{Decoder, PixelFormat};
    let mut decoder = Decoder::new(bytes);
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    let src_w = info.width as u32;
    let src_h = info.height as u32;
    if src_w == 0 || src_h == 0 {
        return None;
    }
    let longest = src_w.max(src_h);
    // jpeg-decoder picks the smallest DCT scale `k` (1..=8 in 1/8ths
    // of full resolution) whose output ≥ the requested dimensions,
    // so requesting `max_px` directly would round UP past the cap.
    // Instead, pick the largest `k` where `longest * k / 8 ≤ max_px`
    // ourselves and request the resulting size verbatim — the
    // decoder then returns exactly that scale. When no scale fits
    // (`max_px` < `longest / 8`) we fall back to `k = 1` (1/8 — the
    // smallest DCT-supported output) and accept the cap overshoot.
    let k = if longest <= max_px {
        8
    } else {
        let mut best: u32 = 1;
        for k in 1..=8u32 {
            if longest * k / 8 <= max_px {
                best = k;
            }
        }
        best
    };
    let target_w = (src_w * k / 8).max(1).min(u16::MAX as u32) as u16;
    let target_h = (src_h * k / 8).max(1).min(u16::MAX as u32) as u16;
    let (final_w, final_h) = decoder.scale(target_w, target_h).ok()?;
    let pixels = decoder.decode().ok()?;
    let info_after = decoder.info()?;
    let icc_profile = decoder.icc_profile();
    let w = final_w as u32;
    let h = final_h as u32;
    let rgba = match info_after.pixel_format {
        PixelFormat::L8 => l8_to_rgba(&pixels, w, h)?,
        PixelFormat::L16 => l16_to_rgba(&pixels, w, h)?,
        PixelFormat::RGB24 => rgb24_to_rgba(&pixels, w, h)?,
        PixelFormat::CMYK32 => cmyk32_to_rgba(&pixels, w, h, icc_profile.as_deref())?,
    };
    Some(idml_compose::DecodedImage {
        width: w,
        height: h,
        rgba,
    })
}

fn l8_to_rgba(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let expected = (w as usize).checked_mul(h as usize)?;
    if src.len() != expected {
        return None;
    }
    let mut rgba = Vec::with_capacity(expected.checked_mul(4)?);
    for &g in src {
        rgba.extend_from_slice(&[g, g, g, 255]);
    }
    Some(rgba)
}

fn l16_to_rgba(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let pixels = (w as usize).checked_mul(h as usize)?;
    if src.len() != pixels.checked_mul(2)? {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4)?);
    for chunk in src.chunks_exact(2) {
        // jpeg-decoder writes L16 big-endian.
        let g16 = u16::from_be_bytes([chunk[0], chunk[1]]);
        let g8 = (g16 >> 8) as u8;
        rgba.extend_from_slice(&[g8, g8, g8, 255]);
    }
    Some(rgba)
}

fn rgb24_to_rgba(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let pixels = (w as usize).checked_mul(h as usize)?;
    if src.len() != pixels.checked_mul(3)? {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4)?);
    for chunk in src.chunks_exact(3) {
        rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
    }
    Some(rgba)
}

/// Track 1b dispatcher: route a CMYK JPEG buffer through the embedded
/// ICC profile when present (and the platform supports lcms2),
/// falling back to the Adobe-naive multiplicative form on
/// missing/invalid profiles or wasm32 targets.
fn cmyk32_to_rgba(src: &[u8], w: u32, h: u32, icc_profile: Option<&[u8]>) -> Option<Vec<u8>> {
    if let Some(profile) = icc_profile {
        match idml_color::IccTransform::cmyk_to_linear_rgb(profile) {
            Ok(xform) => {
                if let Some(rgba) = cmyk32_to_rgba_via_icc(src, w, h, &xform) {
                    return Some(rgba);
                }
                tracing::warn!("CMYK JPEG ICC transform produced wrong-shape output; using naive");
            }
            Err(err) => {
                tracing::warn!(error = %err, "CMYK JPEG ICC profile rejected; using naive");
            }
        }
    }
    cmyk32_to_rgba_naive(src, w, h)
}

/// Batch CMYK-8 → sRGB-byte transform via lcms2. Chunked so peak
/// intermediate memory stays bounded (the largest legal output at
/// the decode cap is 4096×4096 ≈ 64MB CMYK input + ~48MB lcms2
/// scratch + 64MB RGBA output; chunking drops the scratch to ~28KB).
fn cmyk32_to_rgba_via_icc(
    src: &[u8],
    w: u32,
    h: u32,
    xform: &idml_color::IccTransform,
) -> Option<Vec<u8>> {
    let pixels = (w as usize).checked_mul(h as usize)?;
    if src.len() != pixels.checked_mul(4)? {
        return None;
    }
    const CHUNK: usize = 4096;
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4)?);
    let mut cmyk_buf: Vec<[u8; 4]> = vec![[0; 4]; CHUNK];
    let mut rgb_buf: Vec<[u8; 3]> = vec![[0; 3]; CHUNK];
    for src_chunk in src.chunks(CHUNK * 4) {
        let n = src_chunk.len() / 4;
        for i in 0..n {
            cmyk_buf[i] = [
                src_chunk[i * 4],
                src_chunk[i * 4 + 1],
                src_chunk[i * 4 + 2],
                src_chunk[i * 4 + 3],
            ];
        }
        xform.cmyk_bytes_to_rgb_bytes(&cmyk_buf[..n], &mut rgb_buf[..n]);
        for i in 0..n {
            rgba.extend_from_slice(&[rgb_buf[i][0], rgb_buf[i][1], rgb_buf[i][2], 255]);
        }
    }
    Some(rgba)
}

/// Naive Adobe-style CMYK → sRGB. The Adobe CMYK-JPEG convention
/// stores channels inverted (byte 255 = no ink) so the multiplicative
/// form simplifies to `R = C_byte * K_byte / 255` etc. Used as the
/// fallback when no ICC profile is present (or on wasm32 where lcms2
/// is unavailable).
fn cmyk32_to_rgba_naive(src: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let pixels = (w as usize).checked_mul(h as usize)?;
    if src.len() != pixels.checked_mul(4)? {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4)?);
    for chunk in src.chunks_exact(4) {
        let c = chunk[0] as u32;
        let m = chunk[1] as u32;
        let y = chunk[2] as u32;
        let k = chunk[3] as u32;
        let r = (c * k / 255) as u8;
        let g = (m * k / 255) as u8;
        let b = (y * k / 255) as u8;
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    Some(rgba)
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
        .map(|i| (i[0] * idml_text::shape::ADVANCE_PRECISION).round() as i32)
        .unwrap_or(0);
    let pt_to_64 = |pt: f32| (pt * idml_text::shape::ADVANCE_PRECISION).round() as i32;
    let em_fraction_to_64 = |frac: f32| pt_to_64(point_size * frac);
    use idml_parse::FirstBaselineOffset as F;
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
    top_inset_64 + policy_offset_64
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
fn rotate_transform_around(
    xf: &mut Transform,
    linear: [f32; 4],
    pivot_x: f32,
    pivot_y: f32,
) {
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
    match item_transform {
        Some(m) => page_origin.compose(&Transform(m)),
        None => page_origin,
    }
}

/// Axis-aligned bounding box of `rect` after `outer` is applied to its
/// four corners. The corners may rotate / shear under non-uniform
/// transforms, so we union all four projections rather than just the
/// top-left + bottom-right.
fn rect_bounds_in_page(rect: idml_compose::Rect, outer: Transform) -> idml_compose::Rect {
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
    idml_compose::Rect {
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
    if !matches!(frame.blend_mode, idml_compose::BlendMode::Normal) {
        return true;
    }
    matches!(frame.opacity, Some(o) if o < 100.0 - f32::EPSILON)
}

/// Group opacity normalised to 0..=1. Defaults to 1.0 when no opacity
/// override is present on the frame.
fn frame_group_opacity(frame: &ResolvedFrame<'_>) -> f32 {
    frame.opacity.map(|p| (p / 100.0).clamp(0.0, 1.0)).unwrap_or(1.0)
}

/// Push a `BeginBlendGroup` covering `geometry_bounds × outer` (axis-
/// aligned in page coords, padded slightly so AA edges stay inside the
/// buffer). Returns the bounds the matching `EndBlendGroup` will use,
/// for callers that want to bracket multiple ranges of commands with
/// the same group buffer.
pub(crate) fn push_blend_group(
    page: &mut BuiltPage,
    bounds_in_inner: idml_compose::Rect,
    outer: Transform,
    blend_mode: idml_compose::BlendMode,
    opacity: f32,
) -> idml_compose::Rect {
    let bounds = rect_bounds_in_page(bounds_in_inner, outer);
    // Pad by 0.5pt so glyph anti-aliasing at the edges of the
    // text-frame bbox still falls inside the buffer.
    let padded = idml_compose::Rect {
        x: bounds.x - 0.5,
        y: bounds.y - 0.5,
        w: bounds.w + 1.0,
        h: bounds.h + 1.0,
    };
    page.list
        .commands
        .push(idml_compose::DisplayCommand::BeginBlendGroup {
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
        .push(idml_compose::DisplayCommand::EndBlendGroup(
            Transform::IDENTITY,
        ));
}

/// Resolve the effective shadow for a frame. Per-frame IDML shadow
/// wins; the synthetic `fallback` (from `PipelineOptions`) is used
/// when the frame carries none. Returns `None` for fully-transparent
/// shadows so callers don't emit a no-op.
pub(crate) fn resolve_frame_shadow(
    frame_shadow: Option<&idml_parse::DropShadowSetting>,
    fallback: Option<DropShadow>,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) -> Option<DropShadow> {
    frame_shadow
        .and_then(|s| convert_setting_to_shadow(s, palette, cmyk_xform))
        .or(fallback)
}

/// Convert an IDML `<DropShadowSetting>` to a compose-layer `DropShadow`.
/// The parser already drops `Mode="None"` settings, so we only have
/// to filter out fully-transparent shadows here.
fn convert_setting_to_shadow(
    setting: &idml_parse::DropShadowSetting,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
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
    cmyk_xform: Option<&idml_color::IccTransform>,
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
                    let shaped = idml_text::shape_run(face, &run.text, size);
                    stats.glyphs += shaped.glyphs.len();
                }
            }

            let (Some(face), Some(col_pt)) = (shaping_face.as_ref(), column_width_pt) else {
                continue;
            };
            let measurer = idml_text::RustybuzzMeasurer::new(face, paragraph_size);
            let mut lopts = idml_text::LayoutOptions::new(col_pt, paragraph_size);
            lopts.alignment = map_justification(paragraph.justification);
            let laid_out = idml_text::layout_paragraph(&paragraph_text, &measurer, &lopts);
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
        width_pt: page_w,
        height_pt: page_h,
        spread_origin: (0.0, 0.0),
        list,
        stats,
    })
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
        let mut raster_opts = idml_gpu::RasterOptions::new(page.width_pt, page.height_pt);
        raster_opts.dpi = dpi;
        raster_opts.background = background;
        images.push(idml_gpu::rasterize(&page.list, &raster_opts));
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
    let mut raster_opts = idml_gpu::RasterOptions::new(built.width_pt, built.height_pt);
    raster_opts.dpi = dpi;
    raster_opts.background = background;
    let image = idml_gpu::rasterize(&built.list, &raster_opts);
    Ok((built, image))
}

/// Pick the paint for a frame from its FillColor attribute.
pub fn resolve_fill(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(frame.fill_color.as_deref()?, palette, None)
}

/// Same, for StrokeColor.
pub fn resolve_stroke(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(frame.stroke_color.as_deref()?, palette, None)
}

/// Rectangle flavour of `resolve_fill` (no ParentStory to consider).
pub fn resolve_rect_fill(rect: &Rectangle, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(rect.fill_color.as_deref()?, palette, None)
}

/// Rectangle flavour of `resolve_stroke`.
pub fn resolve_rect_stroke(rect: &Rectangle, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(rect.stroke_color.as_deref()?, palette, None)
}

/// Solid-paint resolver. Used by per-cluster glyph paint pickers
/// (where embedding gradient stops per glyph would be wasteful) and
/// by callers that don't have a `&mut DisplayList`.
///
/// CMYK swatches resolve to [`Paint::Cmyk`] when the IDML's
/// `Space="CMYK"` (process or spot-with-CMYK-alternate) so per-channel
/// CMYK overprint compositing (Phase 3 Tier 3 #14 Stage A) can read
/// the source ink values directly. The rasterizer ICC-converts to RGB
/// at draw time for ordinary paints; only the overprint path consumes
/// the channels separately.
///
/// When `cmyk_xform` is `None` (wasm32 fallback, hosts without an
/// ICC profile loaded) CMYK swatches collapse to the naive RGB the
/// `graphic::to_linear_rgb` helper produces, matching the prior
/// behaviour — the CMYK path is gated on having a usable ICC transform
/// downstream.
/// Short-term fallback for gradient-painted glyphs (P-11): when a run's
/// `FillColor` resolves to a gradient swatch but the glyph emit path
/// only consumes solid paints, evaluate the gradient at its midpoint
/// and substitute a representative `Paint::Solid` (or `Paint::Cmyk`).
/// Returns `None` for non-gradient ids or when fewer than two stops
/// could be resolved.
pub fn gradient_midpoint_paint(
    id: &str,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) -> Option<Paint> {
    let grad = palette.gradients.get(id)?;
    // Walk the stops in declaration order; interpolate the colour at
    // 50% of the gradient line. Each stop's `StopColor` already routes
    // through the same swatch table the renderer uses elsewhere, so the
    // result respects ICC and tint cascades.
    let resolved: Vec<(f32, Color)> = grad
        .stops
        .iter()
        .filter_map(|s| {
            let p = color_id_to_paint(&s.stop_color, palette, cmyk_xform)?;
            let c = match p {
                Paint::Solid(c) => c,
                Paint::Cmyk { rgb, .. } => rgb,
                _ => return None,
            };
            Some(((s.location_pct / 100.0).clamp(0.0, 1.0), c))
        })
        .collect();
    if resolved.len() < 2 {
        return None;
    }
    let target = 0.5_f32;
    // Find the segment that brackets `target` and linearly interpolate.
    let mut iter = resolved.windows(2);
    let mut color = resolved.last().map(|s| s.1)?;
    for pair in &mut iter {
        let (off_a, ca) = pair[0];
        let (off_b, cb) = pair[1];
        if target <= off_b {
            let span = (off_b - off_a).max(1e-6);
            let t = ((target - off_a) / span).clamp(0.0, 1.0);
            color = Color::rgba(
                ca.r + (cb.r - ca.r) * t,
                ca.g + (cb.g - ca.g) * t,
                ca.b + (cb.b - ca.b) * t,
                ca.a + (cb.a - ca.a) * t,
            );
            break;
        }
    }
    Some(Paint::Solid(color))
}

pub fn color_id_to_paint(
    id: &str,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) -> Option<Paint> {
    let entry = palette.resolve(id)?;
    // Prefer the swatch's *effective* CMYK — it folds Spot →
    // alternate-CMYK resolution and any swatch-level `TintValue`
    // (e.g. "PANTONE 286 C at 50%") into the channels before ICC.
    // Tint scales each channel toward paper white (0,0,0,0) linearly
    // in CMYK space, which is what InDesign does in preview.
    if let (Some(xform), Some([c, m, y, k])) = (cmyk_xform, entry.effective_cmyk()) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // ICC-resolve once at compose time and bake the result into
            // the paint. The rasterizer uses `rgb` for ordinary draws
            // (bit-identical to the pre-Stage-A path) and the
            // C/M/Y/K channels for overprint composition.
            let cmyk = idml_color::Cmyk { c, m, y, k };
            let idml_color::LinearRgb([r, g, b]) = xform.cmyk_percent_to_linear_rgb(cmyk);
            return Some(Paint::Cmyk {
                c: (c / 100.0).clamp(0.0, 1.0),
                m: (m / 100.0).clamp(0.0, 1.0),
                y: (y / 100.0).clamp(0.0, 1.0),
                k: (k / 100.0).clamp(0.0, 1.0),
                rgb: Color::rgba(r, g, b, 1.0),
                // The list-aware wrappers (e.g. `color_id_to_paint_with_list_dir`)
                // re-tag this paint with a `SpotInkId` for `Model="Spot"`
                // swatches; this function lacks a `&mut DisplayList`,
                // so it leaves the field empty and the visible behaviour
                // collapses to the CMYK-alternate path (matching the
                // Stage A/B output).
                spot: None,
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (c, m, y, k);
        }
    }
    let [r, g, b] = graphic::to_linear_rgb(entry)?;
    Some(Paint::Solid(Color::rgba(r, g, b, 1.0)))
}

/// Project an IDML gradient angle / length onto the path's
/// local 0..1 unit rect. Endpoints lie at `(0.5 ± h_x, 0.5 ± h_y)`
/// where the half-vector `(h_x, h_y)` is derived from the angle and
/// length:
///
/// * `angle_deg` — degrees CCW around the rect centre. IDML's
///   convention is 0° horizontal-right, 90° vertical-down (the page
///   y-axis points down, so a CCW rotation in screen-up coords reads
///   as CW on the page). Defaults to 0° when absent.
/// * `length_pt` — page-space length of the gradient line through the
///   centre. When `Some(L)` and a bbox is supplied, the half-vector is
///   `(cos θ · L / (2·w), sin θ · L / (2·h))` so the page-space line
///   length is exactly `L` regardless of the rect's aspect ratio.
///   When `None`, half-vector magnitude in unit-rect coords is `0.5`
///   along the angle direction — gradient runs edge-to-edge along the
///   cardinal axis (matches InDesign's swatch-panel default).
fn linear_gradient_endpoints(
    angle_deg: Option<f32>,
    length_pt: Option<f32>,
    dims_pt: Option<(f32, f32)>,
) -> ((f32, f32), (f32, f32)) {
    let deg = angle_deg.unwrap_or(0.0);
    let rad = deg.to_radians();
    let (sin, cos) = rad.sin_cos();
    let (cx, cy) = (0.5_f32, 0.5_f32);
    let (hx, hy) = match (length_pt, dims_pt) {
        (Some(l), Some((w, h))) if w > 0.0 && h > 0.0 => {
            (cos * l / (2.0 * w), sin * l / (2.0 * h))
        }
        _ => {
            let half = 0.5_f32;
            (cos * half, sin * half)
        }
    };
    ((cx - hx, cy - hy), (cx + hx, cy + hy))
}

/// Resolver that also handles gradient swatches.
///
/// Gradient ids resolve to a `Paint::LinearGradient` whose stops live
/// in `list.gradients`. Solid colours fall through to
/// `color_id_to_paint`. Used for frame fills (which can carry
/// gradient swatches); not used for per-glyph paints.
pub fn color_id_to_paint_with_list(
    id: &str,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    list: &mut DisplayList,
) -> Option<Paint> {
    color_id_to_paint_with_list_dir(id, palette, cmyk_xform, list, None, None, None)
}

/// Like [`color_id_to_paint_with_list`] but takes an explicit
/// `gradient_angle_deg` from the frame's `GradientFillAngle` /
/// `GradientStrokeAngle` attribute (0° horizontal-right; 90°
/// vertical-down — IDML's convention), an explicit
/// `gradient_length_pt` from the matching `GradientFillLength` /
/// `GradientStrokeLength` attribute, and the path's local bbox
/// `(width, height)` in pt.
///
/// The bbox lets the radial-gradient default place its centre at the
/// path's bottom-left corner with radius equal to the diagonal —
/// matching what InDesign emits when `GradientFillStart` /
/// `GradientFillLength` are absent. For linear gradients it converts
/// the page-pt length into unit-rect endpoints (so the same
/// `LinearGradient` reused on rectangles of different sizes still
/// honours the user-specified length).
///
/// Without the bbox or length we fall back to the unit-rect centred
/// default — gradient line through `(0.5, 0.5)` along the angle, with
/// half-vector magnitude `0.5` in unit-rect coords (still serviceable
/// for callers that don't have geometry, e.g. text-frame strokes).
pub fn color_id_to_paint_with_list_dir(
    id: &str,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    list: &mut DisplayList,
    gradient_angle_deg: Option<f32>,
    gradient_length_pt: Option<f32>,
    path_dims_pt: Option<(f32, f32)>,
) -> Option<Paint> {
    if let Some(grad) = palette.gradients.get(id) {
        // Resolve raw stop colors. For CMYK swatches, also keep the
        // raw CMYK percentages so we can interpolate in CMYK space
        // (pdftoppm's behaviour) — interpolating stops in linear-sRGB
        // produces over-saturated mid-tones because CMYK→sRGB is
        // non-linear (e.g. 50% C + 50% M is a duller violet than the
        // average of pure-C-sRGB and pure-M-sRGB).
        struct StopRef {
            offset: f32,
            color: idml_compose::Color,
            // Owned so a spot swatch's tint-scaled alternate CMYK can
            // participate in CMYK-space gradient interpolation just
            // like a process CMYK swatch would.
            cmyk: Option<[f32; 4]>,
        }
        let raw_stops: Vec<StopRef> = grad
            .stops
            .iter()
            .filter_map(|s| {
                let color = color_id_to_paint(&s.stop_color, palette, cmyk_xform)
                    .and_then(|p| paint_as_solid_with_icc(p, cmyk_xform))?;
                let entry = palette.resolve(&s.stop_color);
                let cmyk = entry.and_then(|e| e.effective_cmyk());
                Some(StopRef {
                    offset: (s.location_pct / 100.0).clamp(0.0, 1.0),
                    color,
                    cmyk,
                })
            })
            .collect();
        if raw_stops.len() < 2 {
            return None;
        }
        let stops: Vec<idml_compose::GradientStop> = if cmyk_xform.is_some()
            && raw_stops.iter().all(|s| s.cmyk.is_some())
        {
            // All stops are CMYK swatches — tessellate the gradient in
            // CMYK space and convert each tessellated point through
            // the ICC transform. 16 sub-stops per inter-stop segment is
            // enough to make even cyan↔yellow mid-tones (the most
            // visibly non-linear pair) match pdftoppm within ~1 ΔE.
            const SUB_STOPS: usize = 16;
            let mut out: Vec<idml_compose::GradientStop> = Vec::new();
            let xform = cmyk_xform.unwrap();
            for win in raw_stops.windows(2) {
                let a = &win[0];
                let b = &win[1];
                let cmyk_a = a.cmyk.unwrap();
                let cmyk_b = b.cmyk.unwrap();
                for i in 0..SUB_STOPS {
                    let t = i as f32 / SUB_STOPS as f32;
                    let interp = idml_color::Cmyk {
                        c: cmyk_a[0] * (1.0 - t) + cmyk_b[0] * t,
                        m: cmyk_a[1] * (1.0 - t) + cmyk_b[1] * t,
                        y: cmyk_a[2] * (1.0 - t) + cmyk_b[2] * t,
                        k: cmyk_a[3] * (1.0 - t) + cmyk_b[3] * t,
                    };
                    let idml_color::LinearRgb([r, g, b_]) =
                        xform.cmyk_percent_to_linear_rgb(interp);
                    out.push(idml_compose::GradientStop {
                        offset: a.offset * (1.0 - t) + b.offset * t,
                        color: idml_compose::Color::rgba(r, g, b_, 1.0),
                    });
                }
            }
            // Always include the final stop exactly.
            let last = raw_stops.last().unwrap();
            out.push(idml_compose::GradientStop {
                offset: last.offset,
                color: last.color,
            });
            out
        } else {
            raw_stops
                .iter()
                .map(|s| idml_compose::GradientStop {
                    offset: s.offset,
                    color: s.color,
                })
                .collect()
        };
        // Radial gradients without an explicit `GradientFillStart` /
        // `GradientFillLength` use InDesign's auto-default: centre at
        // the path's BOTTOM-LEFT corner with radius equal to the
        // path's diagonal (verified empirically against an InDesign-
        // exported PDF — see corpus/generated/gradients.pdf p. 3).
        // The renderer's gradient lives in the path's local 0..1
        // unit-rect; the rasterizer derives the actual circle radius
        // by averaging `width * R_unit` and `height * R_unit` (see
        // `idml_gpu::cpu::build_radial_gradient_shader`), so to
        // produce a circle of pt-radius √(w² + h²) we set
        // `R_unit = 2·√(w² + h²) / (w + h)`. When the caller can't
        // supply the bbox (text-frame strokes etc.) we fall back to
        // the legacy centred-on-(0.5, 0.5) / √½ unit-rect default.
        if matches!(grad.kind, idml_parse::GradientKind::Radial) {
            // Centre at (0, 1) of the unit-rect (= bottom-left of
            // the path in InDesign coords) with radius equal to the
            // longer bbox dimension. Empirically matches what
            // pdftoppm renders from an InDesign-exported PDF for a
            // gradient applied via the Swatches panel without manual
            // gradient-tool placement (corpus/generated/gradients
            // page 3): gradient hits pure black at distance ≈ width
            // for a 360×200 rect, *not* at the diagonal.
            let (center, radius) = match path_dims_pt {
                Some((w, h)) if (w + h) > 0.0 => {
                    let r_actual = w.max(h);
                    // Rasterizer averages (a·R, b·R)·hypot and (c·R,
                    // d·R)·hypot to reduce a unit-rect circle to a
                    // single page-space radius (see
                    // `idml_gpu::cpu::build_radial_gradient_shader`).
                    // Compensate so the page-space circle has the
                    // pt-radius we computed above.
                    let r_unit = 2.0 * r_actual / (w + h);
                    ((0.0, 1.0), r_unit)
                }
                _ => ((0.5, 0.5), std::f32::consts::FRAC_1_SQRT_2),
            };
            let id = list.push_radial_gradient(idml_compose::RadialGradient {
                center,
                radius,
                stops,
            });
            return Some(Paint::RadialGradient(id));
        }
        let (start, end) =
            linear_gradient_endpoints(gradient_angle_deg, gradient_length_pt, path_dims_pt);
        let id = list.push_linear_gradient(idml_compose::LinearGradient {
            start,
            end,
            stops,
        });
        return Some(Paint::LinearGradient(id));
    }
    let paint = color_id_to_paint(id, palette, cmyk_xform)?;
    // Stage C: when the swatch is a named-ink spot colour, intern the
    // ink name on the display list and tag the paint with the resulting
    // id. Spot-on-same-spot overprint then composites per-pixel in the
    // spot's own plane (see `idml-gpu::cpu::compose_spot_overprint_via_planes`).
    // Process CMYK swatches and non-CMYK paints pass through unchanged.
    if let Paint::Cmyk {
        c,
        m,
        y,
        k,
        rgb,
        spot: _,
    } = paint
    {
        if let Some(entry) = palette.resolve(id) {
            if entry.model == idml_parse::ColorModel::Spot && entry.effective_cmyk().is_some() {
                let cmyk_alt_unit = entry.effective_cmyk().unwrap();
                let to_8 = |v: f32| (v.clamp(0.0, 100.0) * 2.55).round() as u8;
                let alt_8 = [
                    to_8(cmyk_alt_unit[0]),
                    to_8(cmyk_alt_unit[1]),
                    to_8(cmyk_alt_unit[2]),
                    to_8(cmyk_alt_unit[3]),
                ];
                let spot_id = list.push_spot_ink(idml_compose::SpotInk {
                    name: id.to_string(),
                    cmyk_alternate: alt_8,
                });
                return Some(Paint::Cmyk {
                    c,
                    m,
                    y,
                    k,
                    rgb,
                    spot: Some(spot_id),
                });
            }
        }
    }
    Some(paint)
}

/// Cluster → Paint picker built from a paragraph's run table.
pub struct RunPaintPicker {
    bands: Vec<(u32, Paint)>,
    default: Paint,
}

impl RunPaintPicker {
    pub fn pick(&self, cluster: u32) -> Paint {
        let mut chosen = self.default;
        for (start, paint) in &self.bands {
            if *start <= cluster {
                chosen = *paint;
            } else {
                break;
            }
        }
        chosen
    }
}

/// Per-cluster lookup for a run's text outline (paint + stroke geometry).
/// Constructed once per paragraph alongside `RunPaintPicker`. `pick`
/// returns `None` for clusters whose cascade leaves `StrokeColor`
/// unset (the common case — IDML records a stroke colour on a run
/// only when the author has explicitly assigned one). A `Some` value
/// drives one extra `StrokePath` per glyph in that run.
#[derive(Default)]
pub struct RunStrokePicker {
    /// `(start_cluster, paint_and_stroke_or_none)`. The picker walks
    /// in cluster order so we keep the bands sorted at build time.
    bands: Vec<(u32, Option<(Paint, Stroke)>)>,
}

impl RunStrokePicker {
    pub fn pick(&self, cluster: u32) -> Option<(Paint, Stroke)> {
        let mut chosen: Option<(Paint, Stroke)> = None;
        for (start, entry) in &self.bands {
            if *start <= cluster {
                chosen = *entry;
            } else {
                break;
            }
        }
        chosen
    }

    /// True iff at least one band carries a visible stroke. Lets the
    /// hot per-line emit loop skip the second glyph sweep entirely
    /// for the overwhelming majority of paragraphs.
    pub fn any_visible(&self) -> bool {
        self.bands.iter().any(|(_, e)| e.is_some())
    }
}

pub fn build_run_paint_picker(
    paragraph: &idml_parse::Paragraph,
    palette: &Graphic,
    default: Paint,
) -> RunPaintPicker {
    build_run_paint_picker_with_cmyk(paragraph, palette, None, default)
}

/// Variant of [`build_run_paint_picker`] that routes CMYK swatches
/// through the document's ICC transform when one is available. Without
/// this the per-glyph fill picker would silently fall back to the
/// naive CMYK→sRGB approximation in `graphic::to_linear_rgb`, undoing
/// the work of building the transform.
pub fn build_run_paint_picker_with_cmyk(
    paragraph: &idml_parse::Paragraph,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    default: Paint,
) -> RunPaintPicker {
    let mut bands: Vec<(u32, Paint)> = Vec::with_capacity(paragraph.runs.len());
    let mut cursor: u32 = 0;
    for run in &paragraph.runs {
        // Gradient swatches fall through `color_id_to_paint` (returns
        // None — gradient resolution requires the DisplayList). For
        // glyph paints we don't yet support per-glyph gradient brushes;
        // substitute the gradient's midpoint colour so display titles
        // stop dropping out (P-11).
        let paint = run
            .fill_color
            .as_deref()
            .and_then(|id| {
                color_id_to_paint(id, palette, cmyk_xform)
                    .or_else(|| gradient_midpoint_paint(id, palette, cmyk_xform))
            })
            .unwrap_or(default);
        bands.push((cursor, paint));
        cursor += run.text.len() as u32;
    }
    RunPaintPicker { bands, default }
}

/// Like [`build_run_paint_picker_with_cmyk`] but uses each run's
/// cascaded `fill_color` (so a run that only carries an
/// `AppliedCharacterStyle` still picks up the right paint). Applies
/// the run's resolved `FillTint` after colour conversion.
///
/// `bullet_paint_override` carries `(bullet_byte_len, paint)` when a
/// `BulletsCharacterStyle` / `BulletsAndNumberingDigitsCharacterStyle`
/// resolves a colour that overrides run 0's fill for the list marker
/// only. The picker prepends a band at cursor 0 with the override
/// paint and pushes every content band by `bullet_byte_len` so the
/// bullet glyphs (clusters 0..bullet_byte_len) get the override while
/// the body text past the marker keeps each run's resolved fill.
/// Build a per-cluster stroke picker for a paragraph.
///
/// Each run's cascaded `(stroke_color, stroke_weight)` decides whether
/// glyphs in that run carry an outline. When `stroke_color` resolves
/// to a real paint but `stroke_weight` is `None`, we fall back to 1pt
/// — matching the value the document's `<TextDefault>` records for a
/// fresh InDesign document (the parser doesn't surface TextDefault as
/// its own node yet; 1pt is the InDesign-published default).
fn build_run_stroke_picker(
    paragraph: &idml_parse::Paragraph,
    resolved_runs: &[idml_scene::ResolvedRunAttrs],
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    bullet_byte_offset: u32,
) -> RunStrokePicker {
    let mut bands: Vec<(u32, Option<(Paint, Stroke)>)> =
        Vec::with_capacity(paragraph.runs.len() + 1);
    let mut cursor = bullet_byte_offset;
    if bullet_byte_offset > 0 {
        // The bullet marker carries no per-run stroke today (the parser
        // wires only fill / fill-tint through the bullet character
        // style). Seed a no-stroke band at cluster 0 so the marker
        // stays fill-only.
        bands.push((0, None));
    }
    for (i, run) in paragraph.runs.iter().enumerate() {
        let entry = resolved_runs[i]
            .stroke_color
            .as_deref()
            .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
            .map(|paint| {
                let width = resolved_runs[i].stroke_weight.unwrap_or(1.0);
                (paint, Stroke::new(width))
            });
        bands.push((cursor, entry));
        cursor += run.text.len() as u32;
    }
    RunStrokePicker { bands }
}

fn build_run_paint_picker_resolved(
    paragraph: &idml_parse::Paragraph,
    resolved_runs: &[idml_scene::ResolvedRunAttrs],
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    default: Paint,
    bullet_paint_override: Option<(u32, Paint)>,
) -> RunPaintPicker {
    let mut bands: Vec<(u32, Paint)> = Vec::with_capacity(paragraph.runs.len() + 1);
    // When a bullet character style overrides the marker's paint, the
    // marker text sits at cluster 0..bullet_byte_len; the content
    // runs follow at cluster bullet_byte_len.. so we seed `cursor` at
    // that offset and emit a leading bullet band.
    let mut cursor: u32 = 0;
    if let Some((bullet_len, bullet_paint)) = bullet_paint_override {
        bands.push((0, bullet_paint));
        cursor = bullet_len;
    }
    for (i, run) in paragraph.runs.iter().enumerate() {
        // Resolve the swatch (or fall through to `default`) FIRST,
        // then apply the run's `FillTint`. The tint affects both
        // explicit swatches and the default paint — IDML treats it
        // as a strength-of-current-fill modifier independent of
        // whether the run carries a FillColor attribute.
        // See `build_run_paint_picker_with_cmyk`: gradient swatches
        // resolve via `gradient_midpoint_paint` as a short-term solid
        // substitute (P-11) until per-glyph gradient brushes land.
        let base = resolved_runs[i]
            .fill_color
            .as_deref()
            .and_then(|id| {
                color_id_to_paint(id, palette, cmyk_xform)
                    .or_else(|| gradient_midpoint_paint(id, palette, cmyk_xform))
            })
            .unwrap_or(default);
        let paint = apply_fill_tint(base, resolved_runs[i].fill_tint);
        bands.push((cursor, paint));
        cursor += run.text.len() as u32;
    }
    RunPaintPicker { bands, default }
}

/// Apply IDML paragraph-style attributes that drive the line breaker
/// onto a fresh `LayoutOptions`. Hyphenation defaults to *on* (IDML's
/// own default) when the cascade leaves the field unset; explicit
/// `Hyphenation="false"` disables it. Word-spacing percentages convert
/// to the composer's stretch / shrink ratios.
fn apply_paragraph_compose_options<'a>(
    lopts: &mut idml_text::LayoutOptions<'a>,
    hyphenator: Option<&'a idml_text::Hyphenator>,
    resolved: &idml_scene::ResolvedParagraphAttrs,
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
    lopts.compose.stretch_ratio = lopts.compose.stretch_ratio.max(0.1);
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
        // Approximate "average chars per word" at 5 — typical English
        // word length is 4.7. Multiplied by a 12pt point-size estimate
        // gives a budget in glyph-advance units (1/64 pt) the breaker
        // can consume. The exact value matters less than that the
        // budget exists; the breaker only uses it when KP can't fit
        // on word spacing alone.
        const AVG_CHARS_PER_WORD: f32 = 5.0;
        let space_width = lopts.compose.column_width as f32 / 80.0; // rough natural space
        let stretch_add = ((ls_max - ls_desired) * AVG_CHARS_PER_WORD / space_width).max(0.0);
        let shrink_add = ((ls_desired - ls_min) * AVG_CHARS_PER_WORD / space_width).max(0.0);
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
}

/// Map an IDML `StrokeType` reference to a [`Stroke`] of the given
/// width with the appropriate dash pattern. Recognises the canonical
/// built-in styles (`StrokeStyle/$ID/Solid`, `Dashed`, `Dotted`,
/// `Dashed3-2`, `Dashed4-4`, `Dashed5-5`, `Dotted2`, `Dotted4`,
/// `Dotted8`); custom user-defined `<StrokeStyle>` definitions
/// fall back to `Solid` until full parser support arrives.
///
/// Pattern values are scaled by the stroke width so a heavier stroke
/// looks proportionally heavier — that mirrors InDesign's behaviour
/// where the named built-ins describe a multiple of the line weight,
/// not absolute pt distances.
pub(crate) fn stroke_for(
    stroke_type: Option<&str>,
    width: f32,
    end_cap: Option<&str>,
    end_join: Option<&str>,
    miter_limit: Option<f32>,
) -> Stroke {
    let mut s = Stroke::new(width);
    if let Some(cap) = end_cap_from(end_cap) {
        s.cap = cap;
    }
    if let Some(join) = end_join_from(end_join) {
        s.join = join;
    }
    if let Some(ml) = miter_limit {
        s.miter_limit = ml;
    }
    let Some(name) = stroke_type else {
        return s;
    };
    let suffix = name.strip_prefix("StrokeStyle/$ID/").unwrap_or(name);
    let w = width.max(0.1);
    // IDML's "Canned" prefix denotes built-in user-facing stroke
    // styles InDesign ships in the Stroke panel — InDesign serialises
    // them as `StrokeStyle/$ID/Canned <Name>` references. Map the
    // common ones to the same pattern table the bare names use so
    // real IDMLs render with the right dash/dot style without each
    // sample needing to declare a custom <StrokeStyle>.
    let normalised = suffix
        .strip_prefix("Canned ")
        .unwrap_or(suffix);
    let is_dotted = matches!(
        normalised,
        "Dotted" | "Dotted2" | "Dotted4" | "Dotted8" | "Japanese Dots"
    );
    let pattern: Option<&[f32]> = match normalised {
        "Solid" | "" => None,
        "Dashed" => Some(&[3.0, 2.0]),
        "Dashed3-2" => Some(&[3.0, 2.0]),
        "Dashed4-4" => Some(&[4.0, 4.0]),
        "Dashed5-5" => Some(&[5.0, 5.0]),
        "Dotted" => Some(&[0.0, 2.0]),
        "Dotted2" => Some(&[0.0, 2.0]),
        "Dotted4" => Some(&[0.0, 4.0]),
        "Dotted8" => Some(&[0.0, 8.0]),
        // InDesign's "Japanese Dots" is denser than the standard
        // Dotted (smaller gap, same on-zero-length).
        "Japanese Dots" => Some(&[0.0, 1.5]),
        _ => None,
    };
    if let Some(p) = pattern {
        let scaled: Vec<f32> = p.iter().map(|v| v * w).collect();
        s.dash = idml_compose::DashPattern::from_slice(&scaled);
        // Dotted patterns force round caps when the IDML didn't carry
        // an explicit `EndCap`, otherwise the zero-length on-segment
        // would render as a needle. Adobe previews behave the same.
        if is_dotted && end_cap.is_none() {
            s.cap = idml_compose::LineCap::Round;
        }
    }
    s
}

fn end_cap_from(name: Option<&str>) -> Option<idml_compose::LineCap> {
    match name? {
        "ButtEndCap" => Some(idml_compose::LineCap::Butt),
        "RoundEndCap" => Some(idml_compose::LineCap::Round),
        "ProjectingEndCap" => Some(idml_compose::LineCap::Square),
        _ => None,
    }
}

fn end_join_from(name: Option<&str>) -> Option<idml_compose::LineJoin> {
    match name? {
        "MiterEndJoin" => Some(idml_compose::LineJoin::Miter),
        "RoundEndJoin" => Some(idml_compose::LineJoin::Round),
        "BevelEndJoin" => Some(idml_compose::LineJoin::Bevel),
        _ => None,
    }
}

/// Scale a paint toward paper white per the IDML `FillTint`
/// percentage. `tint = 100` is identity; lower values blend toward
/// white in linear-RGB space, matching InDesign's preview behaviour.
/// `None` returns the input unchanged. Only applied to solid paints
/// today — gradient stops are left as-is until the gradient
/// resolution itself learns about per-stop tints.
///
/// For [`Paint::Cmyk`] the tint scales each channel toward 0 (paper
/// white in CMYK) — matching the swatch-level `TintValue` semantics
/// `ColorEntry::effective_cmyk` already applies before we get here.
/// This keeps run-level `FillTint` tinting consistent across the
/// CMYK and RGB swatch paths.
pub(crate) fn apply_fill_tint(paint: Paint, tint_pct: Option<f32>) -> Paint {
    let Some(t) = tint_pct else {
        return paint;
    };
    let t = (t / 100.0).clamp(0.0, 1.0);
    if (t - 1.0).abs() < f32::EPSILON {
        return paint;
    }
    match paint {
        Paint::Solid(c) => Paint::Solid(Color::rgba(
            1.0 + (c.r - 1.0) * t,
            1.0 + (c.g - 1.0) * t,
            1.0 + (c.b - 1.0) * t,
            c.a,
        )),
        Paint::Cmyk { c, m, y, k, rgb, spot } => Paint::Cmyk {
            c: c * t,
            m: m * t,
            y: y * t,
            k: k * t,
            // Tint the cached display RGB in step — same blend toward
            // paper white as the `Paint::Solid` arm so the visible
            // result for non-overprint draws stays consistent.
            rgb: Color::rgba(
                1.0 + (rgb.r - 1.0) * t,
                1.0 + (rgb.g - 1.0) * t,
                1.0 + (rgb.b - 1.0) * t,
                rgb.a,
            ),
            // Per-use FillTint preserves the spot identity. The spot
            // plane is tinted by the new C/M/Y/K (whose value is
            // already the tint-scaled CMYK alternate) — that's the
            // late-bound "PANTONE at N%" preview path.
            spot,
        },
        other => other,
    }
}

/// Walk a laid-out line's glyphs and emit horizontal stroke
/// commands for any underlined or struck-through ranges. The stroke
/// uses the run's resolved fill colour (per cluster, via the same
/// picker as the glyphs themselves) so coloured text gets coloured
/// decoration.
fn emit_line_decorations(
    line: &idml_text::layout::LaidOutLine,
    picker: &RunPaintPicker,
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    use idml_text::shape::ADVANCE_PRECISION;
    if line.glyphs.is_empty() {
        return;
    }
    // Two passes — underline (12% of em below baseline) then
    // strikethrough (30% above) — so a glyph with both gets two
    // stripes. Offsets are crude approximations until we read the
    // font's `OS/2` table for the spec'd y_offset / strikeout_pos.
    const UNDERLINE_OFFSET_EM: f32 = 0.12;
    const STRIKETHRU_OFFSET_EM: f32 = -0.30;
    type Pred = fn(&idml_text::PositionedGlyph) -> bool;
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
            idml_compose::emit_line(
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

/// Map an IDML `Justification` enum value to `idml_text::Alignment`.
/// `None` (no attribute on the cascade) falls back to `Left`, the
/// IDML default.
///
/// `ToBindingSide` / `AwayFromBindingSide` are binding-aware values
/// that ideally consult the spread's page side (left vs. right). We
/// don't plumb binding side through to the composer today, so they
/// resolve to `Left` / `Right` respectively — matches the historical
/// stringly-typed behaviour, which fell through to `Left` for any
/// unrecognised string.
pub fn map_justification(j: Option<idml_parse::Justification>) -> idml_text::Alignment {
    use idml_parse::Justification as J;
    match j {
        Some(J::RightAlign) | Some(J::RightJustified) | Some(J::AwayFromBindingSide) => {
            idml_text::Alignment::Right
        }
        Some(J::CenterAlign) | Some(J::CenterJustified) => idml_text::Alignment::Center,
        Some(J::FullyJustified) | Some(J::LeftJustified) => idml_text::Alignment::Justify,
        Some(J::LeftAlign) | Some(J::ToBindingSide) | None => idml_text::Alignment::Left,
    }
}

/// Map IDML `<TabStop Alignment="...">` values to the layout
/// crate's `TabAlignment`.
/// Build the list-marker prefix for a paragraph, or `None` when no
/// list applies. Mutates `counter` based on the paragraph's
/// numbering attributes:
///  - BulletList: counter resets to 0 (bullets don't number);
///    returns `<bullet><separator>`.
///  - NumberedList: applies `NumberingStartAt` / `NumberingContinue`
///    overrides to `counter`, then increments and substitutes
///    `NumberingExpression` (default `^#.^t`). Tokens: `^#` → the
///    formatted counter (per `numbering_format`), `^.` → a literal
///    period, `^t` → a literal tab. Literal characters pass through.
///  - NoList / absent: counter resets to 0; returns `None`.
///
/// `^t` substitutions are snapped to the next tab stop by the
/// existing `apply_tab_stops` pass; the default 36 pt grid gives a
/// reasonable hanging indent without explicit `<TabList>`.
fn list_prefix(
    p: &idml_scene::ResolvedParagraphAttrs,
    counter: &mut u32,
    prev_was_numbered: &mut bool,
) -> Option<String> {
    match p.bullets_list_type.as_deref() {
        Some("BulletList") => {
            // Don't touch the counter here — a later NumberedList
            // paragraph with `NumberingContinue` may want to resume
            // off the prior count across an intervening bullet.
            *prev_was_numbered = false;
            // InDesign's default bullet glyph when none is declared
            // is U+2022 (•). Real IDML usually carries an explicit
            // BulletChar, but real-world exports sometimes leave it
            // implicit on the cascade — fall back so visible bullets
            // still appear.
            let cp = p.bullet_character.unwrap_or(0x2022);
            let ch = char::from_u32(cp)?;
            // `^t` in IDML serialises a literal tab in BulletsTextAfter.
            let after = p
                .bullets_text_after
                .as_deref()
                .map(|s| s.replace("^t", "\t"))
                .unwrap_or_else(|| " ".to_string());
            Some(format!("{ch}{after}"))
        }
        Some("NumberedList") => {
            // Decide whether to reset the counter on entry:
            //   1. Explicit `NumberingStartAt` always wins — the
            //      counter jumps to (start - 1) so the increment
            //      below lands on `start`.
            //   2. Otherwise, if the previous paragraph wasn't
            //      numbered AND this paragraph isn't carrying
            //      `NumberingContinue="true"`, reset to 0 so the
            //      increment lands at 1 (a fresh sequence).
            //   3. Otherwise carry the count forward.
            if let Some(start) = p.numbering_start_at {
                // Negative IDML values clamp to 0 (renders as "0" /
                // whatever the format yields for n=0; matches
                // InDesign's UI which disallows entries < 1 but the
                // schema permits them).
                *counter = (start - 1).max(0) as u32;
            } else if !*prev_was_numbered && p.numbering_continue != Some(true) {
                *counter = 0;
            }
            *counter = counter.checked_add(1).unwrap_or(1);
            *prev_was_numbered = true;
            let formatted = format_number(*counter, p.numbering_format.as_deref());
            // IDML default expression is `^#.^t` — `<n>` + period +
            // tab. The tab snaps to a tab stop via `apply_tab_stops`
            // (default 36 pt grid if no <TabList>), giving a
            // hanging indent without explicit setup.
            let expr = p.numbering_expression.as_deref().unwrap_or("^#.^t");
            Some(substitute_numbering_expression(expr, &formatted))
        }
        _ => {
            // NoList / absent. Like BulletList, don't reset the
            // counter — a later NumberedList paragraph with
            // `NumberingContinue` may want to resume.
            *prev_was_numbered = false;
            None
        }
    }
}

/// Pick the cascaded `CharacterStyle/<id>` that styles the list
/// marker, per IDML's two-field convention:
///
/// - `NumberedList` paragraphs read
///   `BulletsAndNumberingDigitsCharacterStyle` (the digits-style).
/// - `BulletList` paragraphs read `BulletsCharacterStyle` if set,
///   otherwise fall back to
///   `BulletsAndNumberingDigitsCharacterStyle` — the InDesign UI
///   exposes a single "Character Style" picker per paragraph style
///   regardless of list kind, and real-world IDML often lands the
///   reference in the digits-style slot even when the paragraph is
///   a bullet list.
///
/// Returns `None` when no override applies (the bullet/marker then
/// inherits the first run's formatting, the historical behaviour).
fn bullet_marker_character_style(
    p: &idml_scene::ResolvedParagraphAttrs,
) -> Option<&str> {
    match p.bullets_list_type.as_deref() {
        Some("NumberedList") => p
            .bullets_and_numbering_digits_character_style
            .as_deref(),
        Some("BulletList") => p
            .bullets_character_style
            .as_deref()
            .or(p.bullets_and_numbering_digits_character_style.as_deref()),
        _ => None,
    }
}

/// Substitute `^#`, `^.`, `^t` tokens in a NumberingExpression
/// template. Anything else (including unknown `^x` sequences) passes
/// through unchanged.
///
/// IDML escapes a literal caret as `^^` (a doubled caret); decode
/// that so styles that want a literal `^` in their template don't
/// accidentally trigger token replacement.
fn substitute_numbering_expression(expr: &str, formatted_counter: &str) -> String {
    let mut out = String::with_capacity(expr.len() + formatted_counter.len());
    let mut chars = expr.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' {
            match chars.peek().copied() {
                Some('#') => {
                    chars.next();
                    out.push_str(formatted_counter);
                }
                Some('.') => {
                    chars.next();
                    out.push('.');
                }
                Some('t') => {
                    chars.next();
                    out.push('\t');
                }
                Some('^') => {
                    chars.next();
                    out.push('^');
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Format a 1-based list counter per IDML's `NumberingFormat`
/// sample string. Reads the prefix before the first comma to
/// pick a style:
///  - "1, 2, 3..."   → Arabic ("1", "2", "3", ...)
///  - "01, 02, 03..." (or "001, ...") → zero-padded Arabic
///  - "I, II, III..." → upper Roman
///  - "i, ii, iii..." → lower Roman
///  - "A, B, C..."   → upper alpha (A..Z, AA..ZZ, ...)
///  - "a, b, c..."   → lower alpha
///
/// Anything else (or `None`) falls through to plain Arabic.
fn format_number(n: u32, format: Option<&str>) -> String {
    let Some(spec) = format else {
        return n.to_string();
    };
    let head = spec.split(',').next().unwrap_or("").trim();
    match head {
        "I" => to_roman(n, false),
        "i" => to_roman(n, true),
        "A" => to_alpha(n, false),
        "a" => to_alpha(n, true),
        s if s.starts_with('0') && s.chars().all(|c| c.is_ascii_digit()) => {
            // Zero-padded Arabic; width = head's length.
            format!("{:0>width$}", n, width = s.len())
        }
        _ => n.to_string(),
    }
}

/// Roman numeral conversion. `n` must be ≥ 1; `n == 0` returns
/// an empty string (lists start at 1, so this is a sanity guard).
fn to_roman(mut n: u32, lower: bool) -> String {
    if n == 0 {
        return String::new();
    }
    const MAP: &[(u32, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for &(value, symbol) in MAP {
        while n >= value {
            out.push_str(symbol);
            n -= value;
        }
    }
    if lower {
        out.make_ascii_lowercase();
    }
    out
}

/// Spreadsheet-column-style alpha encoding: 1→A, 2→B, …, 26→Z,
/// 27→AA, 28→AB, …, 702→ZZ, 703→AAA. Lowercase mode shifts to
/// 'a'..'z'.
fn to_alpha(mut n: u32, lower: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let base_char = if lower { b'a' } else { b'A' };
    let mut chars = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        chars.push(base_char + rem);
        n = (n - 1) / 26;
    }
    chars.reverse();
    String::from_utf8(chars).expect("ascii letters are valid utf-8")
}

fn map_tab_alignment(a: Option<&str>) -> idml_text::layout::TabAlignment {
    match a {
        Some("RightAlign") => idml_text::layout::TabAlignment::Right,
        Some("CenterAlign") => idml_text::layout::TabAlignment::Center,
        Some("CharacterAlign") => idml_text::layout::TabAlignment::Decimal,
        _ => idml_text::layout::TabAlignment::Left,
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
struct FontTable {
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
}

impl FontTable {
    fn build(document: &Document, options: &PipelineOptions) -> Self {
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
                paragraph: &idml_parse::Paragraph,
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
        let mut face_keys: std::collections::HashSet<(u32, u32)> =
            std::collections::HashSet::new();
        let mut id_to_bytes: HashMap<u32, Bytes> = HashMap::new();
        let harvest_face_keys = |paragraph: &idml_parse::Paragraph,
                                 face_keys: &mut std::collections::HashSet<(u32, u32)>,
                                 id_to_bytes: &mut HashMap<u32, Bytes>| {
            // Inner walk: handle both top-level paragraphs and
            // recursive table-cell paragraphs.
            fn walk(
                document: &Document,
                cache: &HashMap<(String, Option<String>), Bytes>,
                fallback: &Option<Bytes>,
                paragraph: &idml_parse::Paragraph,
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
                        id_to_bytes
                            .entry(font_id)
                            .or_insert_with(|| b.clone());
                    }
                }
                if let Some(table) = paragraph.table.as_ref() {
                    for cell in &table.cells {
                        for inner in &cell.paragraphs {
                            walk(
                                document,
                                cache,
                                fallback,
                                inner,
                                face_keys,
                                id_to_bytes,
                            );
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
                });
            family_metrics.insert(
                family.clone(),
                FontMetrics {
                    ascender: ov.ascender,
                    cap_height: ov.cap_height.or(substitute.cap_height),
                    x_height: ov.x_height.or(substitute.x_height),
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
        runs: &[idml_scene::ResolvedRunAttrs],
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

    fn attrs(
        list_type: Option<&str>,
        ch: Option<u32>,
        after: Option<&str>,
    ) -> idml_scene::ResolvedParagraphAttrs {
        idml_scene::ResolvedParagraphAttrs {
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
            &mut prev_numbered
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
            list_prefix(&attrs, &mut counter, &mut prev_numbered).as_deref(),
            Some("1.\t")
        );
        assert_eq!(
            list_prefix(&attrs, &mut counter, &mut prev_numbered).as_deref(),
            Some("2.\t")
        );
        assert_eq!(
            list_prefix(&attrs, &mut counter, &mut prev_numbered).as_deref(),
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
        list_prefix(&n, &mut counter, &mut prev_numbered); // 1.
        list_prefix(&n, &mut counter, &mut prev_numbered); // 2.
        list_prefix(&none, &mut counter, &mut prev_numbered); // clears prev_numbered, counter sticky
        assert!(!prev_numbered);
        assert_eq!(
            list_prefix(&n, &mut counter, &mut prev_numbered).as_deref(),
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
        );
        assert!(!prev_numbered);
        let n = attrs(Some("NumberedList"), None, None);
        assert_eq!(
            list_prefix(&n, &mut counter, &mut prev_numbered).as_deref(),
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
            list_prefix(&a, &mut counter, &mut prev_numbered).as_deref(),
            Some("5.\t")
        );
        // StartAt only fires on paragraph entry; once it's been
        // applied, drop it for the next paragraph.
        a.numbering_start_at = None;
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered).as_deref(),
            Some("6.\t")
        );
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered).as_deref(),
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
        list_prefix(&plain, &mut counter, &mut prev_numbered); // 1.
        list_prefix(&plain, &mut counter, &mut prev_numbered); // 2.
        let mut jumped = attrs(Some("NumberedList"), None, None);
        jumped.numbering_start_at = Some(10);
        assert_eq!(
            list_prefix(&jumped, &mut counter, &mut prev_numbered).as_deref(),
            Some("10.\t")
        );
        // Subsequent plain paragraphs continue off the jump.
        assert_eq!(
            list_prefix(&plain, &mut counter, &mut prev_numbered).as_deref(),
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
        list_prefix(&plain, &mut counter, &mut prev_numbered); // 1.
        list_prefix(&plain, &mut counter, &mut prev_numbered); // 2.
        list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
            &mut counter,
            &mut prev_numbered,
        );
        let mut cont = attrs(Some("NumberedList"), None, None);
        cont.numbering_continue = Some(true);
        assert_eq!(
            list_prefix(&cont, &mut counter, &mut prev_numbered).as_deref(),
            Some("3.\t"),
            "NumberingContinue suppresses the implicit reset"
        );
        // Compare against the default-reset path: without Continue,
        // the same scenario would have restarted at 1.
        let mut counter2 = 0;
        let mut prev2 = false;
        list_prefix(&plain, &mut counter2, &mut prev2); // 1.
        list_prefix(&plain, &mut counter2, &mut prev2); // 2.
        list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
            &mut counter2,
            &mut prev2,
        );
        assert_eq!(
            list_prefix(&plain, &mut counter2, &mut prev2).as_deref(),
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
            list_prefix(&a, &mut counter, &mut prev_numbered).as_deref(),
            Some("Step 1 of 5\t")
        );
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered).as_deref(),
            Some("Step 2 of 5\t")
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
    fn list_prefix_uses_numbering_format() {
        let mut counter = 0;
        let mut prev_numbered = false;
        let mut a = attrs(Some("NumberedList"), None, None);
        a.numbering_format = Some("I, II, III, IV...".to_string());
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered).as_deref(),
            Some("I.\t")
        );
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered).as_deref(),
            Some("II.\t")
        );
        assert_eq!(
            list_prefix(&a, &mut counter, &mut prev_numbered).as_deref(),
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

    fn anchor_at(x: f32, y: f32) -> idml_parse::PathAnchor {
        idml_parse::PathAnchor {
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

    fn run_attrs(family: Option<&str>, style: Option<&str>) -> idml_scene::ResolvedRunAttrs {
        idml_scene::ResolvedRunAttrs {
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
        let pool = table.resolve_paragraph_bytes(&runs).expect("paragraph kept");
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
        let pool = table.resolve_paragraph_bytes(&runs).expect("paragraph kept");
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
        let run = idml_parse::CharacterRun {
            text: "01\n".to_string(),
            ..idml_parse::CharacterRun::default()
        };
        let paragraph = idml_parse::Paragraph {
            runs: vec![run],
            ..idml_parse::Paragraph::default()
        };
        let subs = split_paragraph_at_breaks(&paragraph);
        assert_eq!(subs.len(), 1, "trailing \\n must not produce a phantom sub-paragraph");
        assert_eq!(subs[0].runs.len(), 1);
        assert_eq!(subs[0].runs[0].text, "01");
    }

    // Belt + braces: pathological case where the splitter's hint
    // path seeds an all-`\n` trailing run. The post-loop guard at
    // the tail of `split_paragraph_at_breaks` must collapse it.
    #[test]
    fn split_paragraph_at_breaks_drops_trailing_all_newline_run_after_visible() {
        let visible = idml_parse::CharacterRun {
            text: "01".to_string(),
            ..idml_parse::CharacterRun::default()
        };
        let nl_only = idml_parse::CharacterRun {
            text: "\n\n".to_string(),
            ..idml_parse::CharacterRun::default()
        };
        let paragraph = idml_parse::Paragraph {
            runs: vec![visible, nl_only],
            ..idml_parse::Paragraph::default()
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
    /// `idml_gpu::cpu::build_linear_gradient_shader`), so a 90°-
    /// vertical gradient on a 90°-rotated frame should produce a
    /// horizontal page-space gradient line. Asserts that — guards
    /// against a regression that would re-introduce the protocol's
    /// hypothesised bug (ItemTransform ignored on gradient projection).
    #[test]
    fn q08_gradient_endpoints_rotate_with_item_transform() {
        let rect = idml_compose::Rect {
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
        let bbox = idml_compose::Rect {
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
            Rgb([
                (x & 0xFF) as u8,
                (y & 0xFF) as u8,
                ((x ^ y) & 0xFF) as u8,
            ])
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

    /// Track 1a: small JPEGs (longest edge ≤ cap) skip the streaming
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
}
