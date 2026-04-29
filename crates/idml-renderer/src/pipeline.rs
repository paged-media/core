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
    emit_drop_shadow_rect_transformed, emit_ellipse_transformed, emit_glyph_slice, emit_line,
    emit_paragraph, emit_rect, emit_rect_transformed, emit_stroke_ellipse_transformed,
    emit_stroke_rect, emit_stroke_rect_transformed, Color, DisplayCommand, DisplayList, DropShadow,
    Paint, PathData, PathSegment, Rect, Stroke, Transform, TtfOutliner,
};
use idml_parse::{graphic, Graphic, GraphicLine, Oval, PathAnchor, Polygon, Rectangle, TextFrame};
use idml_scene::Document;

use crate::AssetResolver;

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
            frame_drop_shadow: None,
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

        for frame in &master.spread.text_frames {
            let spread_b = transform_bounds(frame.bounds, frame.item_transform);
            if master_page_for(spread_b) != local_master_page_idx {
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
            let copy = text_frame_with_object_style(copy, document);
            emit_text_frame_into(
                &mut pages[i],
                &copy,
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
            if master_page_for(spread_b) != local_master_page_idx {
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
            let copy = rectangle_with_object_style(copy, document);
            emit_rectangle_into(
                &mut pages[i],
                &copy,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
                None,
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
    for (spread_idx, parsed) in document.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        let range = spread_page_ranges[spread_idx].clone();
        let local_geoms = &page_geometries[range.clone()];
        for frame in &spread.text_frames {
            if !layer_visible(frame.item_layer.as_deref()) {
                continue;
            }
            total_stats.frames += 1;
            // Frame.bounds are in the frame's *inner* coords; route
            // by transforming through ItemTransform first so the
            // centroid lives in spread coords (matching
            // page_geometries).
            let spread_bounds = transform_bounds(frame.bounds, frame.item_transform);
            let local_idx = page_for_frame(&spread_bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            if let Some(self_id) = frame.self_id.clone() {
                frame_to_page.insert(self_id, page_idx);
            }
            let frame = text_frame_with_object_style(frame.clone(), document);
            emit_text_frame_into(
                &mut pages[page_idx],
                &frame,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
                options.frame_drop_shadow,
            );
        }
        for rect in &spread.rectangles {
            if !layer_visible(rect.item_layer.as_deref()) {
                continue;
            }
            total_stats.frames += 1;
            let spread_bounds = transform_bounds(rect.bounds, rect.item_transform);
            let local_idx = page_for_frame(&spread_bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            let rect = rectangle_with_object_style(rect.clone(), document);
            emit_rectangle_into(
                &mut pages[page_idx],
                &rect,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
                options.frame_drop_shadow,
            );
            // The image draws on top of the rectangle's solid fill.
            // Per-page cache: shares ImageId across same-URI
            // rectangles on this page. Renderer-scoped cache:
            // shares the decoded RGBA across pages.
            emit_rectangle_image(
                &mut pages[page_idx],
                &rect,
                options,
                &mut page_image_caches[page_idx],
                &mut decoded_image_cache,
            );
        }
        for oval in &spread.ovals {
            if !layer_visible(oval.item_layer.as_deref()) {
                continue;
            }
            total_stats.frames += 1;
            let spread_bounds = transform_bounds(oval.bounds, oval.item_transform);
            let local_idx = page_for_frame(&spread_bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            let oval = oval_with_object_style(oval.clone(), document);
            emit_oval_into(
                &mut pages[page_idx],
                &oval,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
            );
        }
        for line in &spread.graphic_lines {
            if !layer_visible(line.item_layer.as_deref()) {
                continue;
            }
            total_stats.frames += 1;
            let spread_bounds = transform_bounds(line.bounds, line.item_transform);
            let local_idx = page_for_frame(&spread_bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            let line = line_with_object_style(line.clone(), document);
            emit_line_into(&mut pages[page_idx], &line, palette, cmyk_xform.as_ref());
        }
        for poly in &spread.polygons {
            if !layer_visible(poly.item_layer.as_deref()) {
                continue;
            }
            total_stats.frames += 1;
            let spread_bounds = transform_bounds(poly.bounds, poly.item_transform);
            let local_idx = page_for_frame(&spread_bounds, local_geoms).unwrap_or(0);
            let page_idx = range.start + local_idx;
            let poly = polygon_with_object_style(poly.clone(), document);
            emit_polygon_into(
                &mut pages[page_idx],
                &poly,
                palette,
                options.fallback_frame_fill,
                cmyk_xform.as_ref(),
            );
        }
    }

    // Story pass: layout text into its hosting frame's page.
    //
    // The font table pre-resolves every distinct (family, style)
    // referenced anywhere in the document so each paragraph picks up
    // the right TTF without re-querying the resolver. Per paragraph
    // we still build `Face`s on demand — `rustybuzz::Face::from_slice`
    // is cheap (parses font tables, no allocation churn).
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
        let head_wrap_rects: &[idml_parse::Bounds] = &[];
        let chain_wrap_rects: Vec<&[idml_parse::Bounds]> = vec![&[]];
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
        );
        for paragraph in &parsed.story.paragraphs {
            emitter.emit_paragraph(paragraph, &mut pages, &mut total_stats);
        }
        emitter.apply_vertical_justification(&mut pages);
    }

    for parsed in &document.stories {
        total_stats.stories += 1;
        let chain = document.frame_chain(&parsed.self_id);
        if chain.is_empty() {
            continue;
        }
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
        let head_wrap_rects: &[idml_parse::Bounds] = wrap_rects_per_page
            .get(head_page_idx)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        // Per-chain wrap rects so threaded frames inherit per-line
        // wrap. Each chain index maps to its frame's page's
        // exclusion list.
        let chain_wrap_rects: Vec<&[idml_parse::Bounds]> = chain_pages
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
        );
        for paragraph in &parsed.story.paragraphs {
            emitter.emit_paragraph(paragraph, &mut pages, &mut total_stats);
        }
        emitter.apply_vertical_justification(&mut pages);
    }

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
    head_wrap_rects: Vec<idml_parse::Bounds>,
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
    chain_wrap_rects: Vec<Vec<idml_parse::Bounds>>,
    /// Spread-coord bounds for every frame in the chain. Same
    /// motivation as `chain_wrap_rects`: per-frame per-line wrap
    /// needs each frame's spread rect.
    chain_spread_bounds: Vec<idml_parse::Bounds>,
    frame_idx: usize,
    y_cursor: i32,
    frame_cmd_ranges: Vec<Option<(usize, usize)>>,
    frame_max_baseline_64: Vec<i32>,
    /// Counter for `NumberedList` paragraphs in this story.
    /// 0 means "not currently inside a numbered list" — incremented
    /// to 1 on the first numbered paragraph and reset back to 0 the
    /// first time a non-numbered paragraph is emitted. The reset
    /// matches IDML's `NumberingContinue=true` default behaviour
    /// for adjacent paragraphs.
    numbered_counter: u32,
}

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
        head_wrap_rects: &[idml_parse::Bounds],
        chain_wrap_rects: Vec<&[idml_parse::Bounds]>,
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
        for w in head_wrap_rects {
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

        let raw_width = (head_frame_spread.width() - head_insets[1] - head_insets[3]).max(0.0);
        let wrapped_width = (raw_width - shrink_left - shrink_right).max(0.0);
        let column_width_pt = options.fallback_column_width_pt.or(Some(wrapped_width));
        let len = chain.len();
        let chain_spread_bounds: Vec<idml_parse::Bounds> = chain
            .iter()
            .map(|f| transform_bounds(f.bounds, f.item_transform))
            .collect();
        let chain_wrap_rects_owned: Vec<Vec<idml_parse::Bounds>> =
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
            frame_cmd_ranges: vec![None; len],
            frame_max_baseline_64: vec![0; len],
            numbered_counter: 0,
        }
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
            let dy_64 = match vj {
                idml_parse::VerticalJustification::Center => slack_64 / 2,
                idml_parse::VerticalJustification::Bottom => slack_64,
                // Justify falls through to Top (per-paragraph
                // distribution lands later); Top handled above.
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
        let para_pt = em
            .document
            .styles
            .resolve_paragraph(
                paragraph
                    .paragraph_style
                    .as_deref()
                    .unwrap_or("ParagraphStyle/$ID/[No paragraph style]"),
            )
            .point_size
            .unwrap_or(em.options.default_point_size);
        let space_before_64 =
            resolved_paragraph.space_before.unwrap_or(0.0) * idml_text::shape::ADVANCE_PRECISION;
        let line_height_64 = (para_pt * 1.2 * idml_text::shape::ADVANCE_PRECISION).round() as i32;
        // Establish the first baseline if we haven't placed any
        // content yet — same convention as the populated branch
        // below — then advance by a full line height.
        if em.y_cursor < 0 {
            em.y_cursor = (para_pt * 0.8 * idml_text::shape::ADVANCE_PRECISION).round() as i32;
        }
        em.y_cursor += space_before_64.round() as i32 + line_height_64;
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
    // `Face` construction below all live in the same scope.
    let mut bytes_pool: Vec<bytes::Bytes> = Vec::with_capacity(paragraph.runs.len());
    for resolved in &resolved_runs {
        let Some(b) = em
            .font_table
            .bytes_for(resolved.font.as_deref(), resolved.font_style.as_deref())
        else {
            continue;
        };
        bytes_pool.push(b);
    }
    if bytes_pool.is_empty() || bytes_pool.len() != paragraph.runs.len() {
        return;
    }

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
    let mut shaping_faces: Vec<Option<rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut outline_faces: Vec<Option<ttf_parser::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    for i in 0..bytes_pool.len() {
        if unique_idx[i] != i {
            continue;
        }
        let bytes_ref = bytes_pool[i].as_ref();
        let Some(mut rf) = rustybuzz::Face::from_slice(bytes_ref, 0) else {
            return;
        };
        let Ok(mut of) = ttf_parser::Face::parse(bytes_ref, 0) else {
            return;
        };
        // Set wght variation on both faces. No-op for static fonts
        // (set_variation returns Some only when the axis exists).
        let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
        rf.set_variations(&[rustybuzz::Variation {
            tag: wght_tag,
            value: wghts[i],
        }]);
        let _ = of.set_variation(wght_tag, wghts[i]);
        shaping_faces[i] = Some(rf);
        outline_faces[i] = Some(of);
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
    // first run's text. The bullet picks up the first run's font
    // and size; a future batch can route it through the paragraph
    // style's character formatting instead. IDML serialises tabs
    // in BulletsTextAfter as the literal `^t` two-byte sequence —
    // expand to a real `\t` so apply_tab_stops snaps it.
    let list_first_text: Option<String> =
        list_prefix(&resolved_paragraph, &mut em.numbered_counter).and_then(|prefix| {
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

    // Per-run uppercase override for `Capitalization=AllCaps |
    // SmallCaps | CapToSmallCap`. We don't yet drive an OT smcp
    // lookup, so SmallCaps falls back to AllCaps shaping — the metric
    // gets the right glyph count and width even if the shape isn't
    // optical-size-tuned. Allocates only for runs whose resolved
    // capitalization actually differs from their input.
    let capitalized: Vec<Option<String>> = paragraph
        .runs
        .iter()
        .enumerate()
        .map(
            |(i, run)| match resolved_runs[i].capitalization.as_deref() {
                Some("AllCaps") | Some("SmallCaps") | Some("CapToSmallCap") => {
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
            face: shaping_faces[unique_idx[i]].as_ref().unwrap(),
            point_size: resolved_runs[i]
                .point_size
                .unwrap_or(em.options.default_point_size),
            tracking: resolved_runs[i].tracking,
            font_id: font_ids[i],
            underline: resolved_runs[i].underline.unwrap_or(false),
            strikethru: resolved_runs[i].strikethru.unwrap_or(false),
            baseline_shift_pt: resolved_runs[i].baseline_shift.unwrap_or(0.0),
        })
        .collect();

    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let Some(col_pt) = em.column_width_pt else {
        return;
    };
    let mut lopts = idml_text::LayoutOptions::new(col_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification.as_deref());
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
        let head_font_metrics = font_ids
            .first()
            .and_then(|id| em.font_table.metrics_for(*id));
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
    }
    lopts.first_baseline = em.y_cursor;

    // Per-line wrap: build a `column_widths` slice + per-line
    // x-shifts + twin-pair markers based on which wrap rectangles
    // each predicted line intersects. Shifts are stored in 1/64 pt
    // so the post-layout pass can add them to each glyph's x;
    // twin_after[i] = true means line i shares its baseline with
    // line i-1 (BothSides flow around an obstacle).
    let WrapPlan {
        line_x_shifts_64,
        twin_after,
    } = build_perline_wrap_widths(em, &styled_runs, &mut lopts);

    let mut laid_out = idml_text::layout_runs(&styled_runs, &lopts);

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
    // y. Subsequent lines absorb the saved row height by shifting
    // up cumulatively. Without this pass twins would render as
    // sequential rows, which Knuth-Plass produced naively.
    if !twin_after.is_empty() {
        let mut accumulated_shift = 0i32;
        let mut prev_baseline = 0i32;
        for (i, line) in laid_out.lines.iter_mut().enumerate() {
            let is_twin = twin_after.get(i).copied().unwrap_or(false) && i > 0;
            if is_twin {
                let target = prev_baseline;
                let diff = line.baseline_y - target;
                line.baseline_y = target;
                for g in &mut line.glyphs {
                    g.y -= diff;
                }
                accumulated_shift += diff;
            } else if accumulated_shift > 0 {
                line.baseline_y -= accumulated_shift;
                for g in &mut line.glyphs {
                    g.y -= accumulated_shift;
                }
                prev_baseline = line.baseline_y;
            } else {
                prev_baseline = line.baseline_y;
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
                leader: t
                    .leader
                    .as_deref()
                    .and_then(|s| s.chars().next())
                    .filter(|c| !c.is_whitespace()),
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
        for line in laid_out.lines.iter_mut() {
            idml_text::layout::apply_tab_stops(line, &paragraph_text, &tab_stops, 36.0);
        }
    }

    let picker = build_run_paint_picker_resolved(
        paragraph,
        &resolved_runs,
        em.palette,
        em.cmyk_xform,
        em.options.fallback_text_paint,
    );

    let space_after_64 =
        resolved_paragraph.space_after.unwrap_or(0.0) * idml_text::shape::ADVANCE_PRECISION;
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
                text_origin_pt,
                &outliner,
                &mut pages[target_page].list,
            );
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
        let frame_idx = em.frame_idx;
        match &mut em.frame_cmd_ranges[frame_idx] {
            Some((_, e)) => *e = after_cmds,
            None => em.frame_cmd_ranges[frame_idx] = Some((before_cmds, after_cmds)),
        }
        if line.baseline_y > em.frame_max_baseline_64[frame_idx] {
            em.frame_max_baseline_64[frame_idx] = line.baseline_y;
        }

        em.y_cursor = line.baseline_y + line_h;
    }
    em.y_cursor += space_after_64.round() as i32;
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

/// Apply a frame's `AppliedObjectStyle` cascade (when present) so
/// any unset fill/stroke/weight gets the value the style would
/// have produced. Real-world IDMLs rely almost exclusively on
/// AppliedObjectStyle for these — `FillColor=` attributes are
/// rare on individual page items. Frames already carrying every
/// override are returned unchanged.
fn text_frame_with_object_style(frame: TextFrame, document: &Document) -> TextFrame {
    if frame.fill_color.is_some() && frame.stroke_color.is_some() && frame.stroke_weight.is_some() {
        return frame;
    }
    let Some(id) = frame.applied_object_style.as_deref() else {
        return frame;
    };
    let resolved = document.styles.resolve_object(id);
    TextFrame {
        fill_color: frame.fill_color.or(resolved.fill_color),
        stroke_color: frame.stroke_color.or(resolved.stroke_color),
        stroke_weight: frame.stroke_weight.or(resolved.stroke_weight),
        ..frame
    }
}

fn rectangle_with_object_style(rect: Rectangle, document: &Document) -> Rectangle {
    let Some(id) = rect.applied_object_style.as_deref() else {
        return rect;
    };
    let resolved = document.styles.resolve_object(id);
    Rectangle {
        fill_color: rect.fill_color.or(resolved.fill_color),
        stroke_color: rect.stroke_color.or(resolved.stroke_color),
        stroke_weight: rect.stroke_weight.or(resolved.stroke_weight),
        corner_radius: rect.corner_radius.or(resolved.corner_radius),
        corner_option: rect.corner_option.or(resolved.corner_option),
        ..rect
    }
}

fn oval_with_object_style(oval: Oval, document: &Document) -> Oval {
    if oval.fill_color.is_some() && oval.stroke_color.is_some() && oval.stroke_weight.is_some() {
        return oval;
    }
    let Some(id) = oval.applied_object_style.as_deref() else {
        return oval;
    };
    let resolved = document.styles.resolve_object(id);
    Oval {
        fill_color: oval.fill_color.or(resolved.fill_color),
        stroke_color: oval.stroke_color.or(resolved.stroke_color),
        stroke_weight: oval.stroke_weight.or(resolved.stroke_weight),
        ..oval
    }
}

fn line_with_object_style(line: GraphicLine, document: &Document) -> GraphicLine {
    if line.stroke_color.is_some() && line.stroke_weight.is_some() {
        return line;
    }
    let Some(id) = line.applied_object_style.as_deref() else {
        return line;
    };
    let resolved = document.styles.resolve_object(id);
    GraphicLine {
        stroke_color: line.stroke_color.or(resolved.stroke_color),
        stroke_weight: line.stroke_weight.or(resolved.stroke_weight),
        ..line
    }
}

fn polygon_with_object_style(poly: Polygon, document: &Document) -> Polygon {
    if poly.fill_color.is_some() && poly.stroke_color.is_some() && poly.stroke_weight.is_some() {
        return poly;
    }
    let Some(id) = poly.applied_object_style.as_deref() else {
        return poly;
    };
    let resolved = document.styles.resolve_object(id);
    Polygon {
        fill_color: poly.fill_color.or(resolved.fill_color),
        stroke_color: poly.stroke_color.or(resolved.stroke_color),
        stroke_weight: poly.stroke_weight.or(resolved.stroke_weight),
        ..poly
    }
}

/// Build a [`PathData`] from a polygon's parsed Bezier anchors.
/// Each consecutive pair becomes a cubic with the leading point's
/// `right` and the trailing point's `left` as control points. When
/// `right == anchor` and `left == anchor` (the IDML serialisation
/// for straight-line corners), the cubic degenerates and tiny-skia
/// reduces it to a line internally.
fn polygon_path_from_anchors(anchors: &[PathAnchor]) -> PathData {
    let mut segs = Vec::with_capacity(anchors.len() * 2);
    if let Some(first) = anchors.first() {
        let (mx, my) = first.anchor;
        segs.push(PathSegment::MoveTo { x: mx, y: my });
    }
    for window in anchors.windows(2) {
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
    // `left` — IDML polygons are otherwise always closed.
    if anchors.len() >= 2 {
        let last = anchors.last().unwrap();
        let first = anchors.first().unwrap();
        segs.push(PathSegment::CubicTo {
            cx1: last.right.0,
            cy1: last.right.1,
            cx2: first.left.0,
            cy2: first.left.1,
            x: first.anchor.0,
            y: first.anchor.1,
        });
    }
    segs.push(PathSegment::Close);
    PathData { segments: segs }
}

/// Polygon emit. When the polygon carries `<PathPointType>` anchors
/// (real-world InDesign export shape) we build a curved FillPath
/// from them; otherwise fall back to drawing the AABB so synthetic
/// IDMLs that declare a polygon via `GeometricBounds` still render.
fn emit_polygon_into(
    page: &mut BuiltPage,
    poly: &Polygon,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    page.stats.frames += 1;
    let outer = frame_outer_transform(page, poly.item_transform);
    let fill = poly
        .fill_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        .unwrap_or(fallback);

    if poly.anchors.is_empty() {
        // No anchors: render the bbox.
        let r = Rect {
            x: poly.bounds.left,
            y: poly.bounds.top,
            w: poly.bounds.width(),
            h: poly.bounds.height(),
        };
        emit_rect_transformed(r, outer, fill, &mut page.list);
        if let Some(stroke) = poly
            .stroke_color
            .as_deref()
            .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        {
            let width = poly.stroke_weight.unwrap_or(1.0);
            if width > 0.0 {
                emit_stroke_rect_transformed(r, outer, Stroke::new(width), stroke, &mut page.list);
            }
        }
        return;
    }

    // Curved path. Anchors are in inner coords; the outer transform
    // (page-origin × ItemTransform) maps inner → page-local pt at
    // emit time, so the path data we intern stays in the same
    // coordinate system the parser produced. Cache key uses the
    // polygon's Self id where present so repeated draws of the
    // same shape share the interned path.
    let path = polygon_path_from_anchors(&poly.anchors);
    let cache_key = match &poly.self_id {
        Some(id) => fnv_1a_u64(id.as_bytes()),
        None => path_signature(&poly.anchors),
    };
    let (path_id, _) = page.list.paths.intern(cache_key, path);
    page.list.commands.push(DisplayCommand::FillPath {
        path_id,
        paint: fill,
        transform: outer,
    });
    if let Some(stroke_paint) = poly
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
    {
        let width = poly.stroke_weight.unwrap_or(1.0);
        if width > 0.0 {
            page.list.commands.push(DisplayCommand::StrokePath {
                path_id,
                paint: stroke_paint,
                stroke: Stroke::new(width),
                transform: outer,
            });
        }
    }
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

fn fnv_1a_u64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
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
) -> Vec<Vec<idml_parse::Bounds>> {
    let total_pages: usize = spread_page_ranges.last().map(|r| r.end).unwrap_or(0);
    let mut out: Vec<Vec<idml_parse::Bounds>> = (0..total_pages).map(|_| Vec::new()).collect();
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
        let route = |spread_b: idml_parse::Bounds| -> Option<usize> {
            let cx = (spread_b.left + spread_b.right) * 0.5;
            let cy = (spread_b.top + spread_b.bottom) * 0.5;
            page_bounds
                .iter()
                .position(|b| cx >= b.left && cx <= b.right && cy >= b.top && cy <= b.bottom)
        };
        let push = |out: &mut Vec<Vec<idml_parse::Bounds>>,
                    spread_b: idml_parse::Bounds,
                    wrap: idml_parse::TextWrap| {
            if !wrap.mode.is_active() {
                return;
            }
            // Side-shrink heuristic only handles BoundingBoxTextWrap.
            // Other modes (Contour / Jump / NextColumn) need richer
            // per-line geometry; we ignore them to avoid corrupting
            // layouts that the side-shrink can't represent.
            if !matches!(wrap.mode, idml_parse::TextWrapMode::BoundingBoxTextWrap) {
                return;
            }
            let inflated = idml_parse::Bounds {
                top: spread_b.top - wrap.offsets[0],
                left: spread_b.left - wrap.offsets[1],
                bottom: spread_b.bottom + wrap.offsets[2],
                right: spread_b.right + wrap.offsets[3],
            };
            if let Some(local_idx) = route(spread_b) {
                let page_idx = range.start + local_idx;
                if page_idx < out.len() {
                    out[page_idx].push(inflated);
                }
            }
        };
        for f in &parsed.spread.text_frames {
            if let Some(w) = f.text_wrap {
                push(&mut out, transform_bounds(f.bounds, f.item_transform), w);
            }
        }
        for r in &parsed.spread.rectangles {
            if let Some(w) = r.text_wrap {
                push(&mut out, transform_bounds(r.bounds, r.item_transform), w);
            }
        }
        for o in &parsed.spread.ovals {
            if let Some(w) = o.text_wrap {
                push(&mut out, transform_bounds(o.bounds, o.item_transform), w);
            }
        }
        for p in &parsed.spread.polygons {
            if let Some(w) = p.text_wrap {
                push(&mut out, transform_bounds(p.bounds, p.item_transform), w);
            }
        }
        for l in &parsed.spread.graphic_lines {
            if let Some(w) = l.text_wrap {
                push(&mut out, transform_bounds(l.bounds, l.item_transform), w);
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
/// * Honours per-row `SingleRowHeight` and per-column
///   `SingleColumnWidth`. Cells with `RowSpan > 1` or
///   `ColumnSpan > 1` widen / lengthen their rect; multi-cell text
///   merging across spans isn't separately modelled.
/// * No cell strokes / fills — those live on `<CellStyle>` and
///   `<TableStyle>` definitions in `Resources/Styles.xml` we don't
///   yet resolve.
/// * Threaded overflow of a table across frames is not modeled
///   (rare in real-world IDMLs).
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
    let row_heights: Vec<f32> = table
        .rows
        .iter()
        .map(|r| r.single_row_height.unwrap_or(0.0))
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
    // TODO(T3.1): when chain_idx advances at the frame-boundary check
    // below, re-emit `table.cells` whose row falls in `0..header_count`
    // at the top of the new frame (and `(total_rows - footer_count)..`
    // at the bottom of the previous frame). Requires synthesising
    // RowBasis entries for the duplicates and routing emit_cell_paragraph
    // through them. Deferred until we have a threaded-table sample to
    // test against — Sample-3's tables don't span frames.
    let total_rows = row_heights.len();
    let total_cols = col_widths.len();
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

    // Per-row layout basis: which chain frame the row lives in
    // and the page-local top y for that row. Walks rows
    // top-to-bottom, advancing through the chain when a row would
    // overflow its frame. Cells consult their starting row's basis
    // for positioning; spans clip to the starting frame's bottom
    // rather than splitting across frames.
    #[derive(Clone, Copy)]
    struct RowBasis {
        chain_idx: usize,
        target_page: usize,
        table_left_pt: f32,
        // Page-local y for the top of THIS row.
        row_top_in_page: f32,
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
    let mut row_bases: Vec<RowBasis> = Vec::with_capacity(total_rows);
    // Per-frame extent for table-border emission below.
    // Each entry: (chain_idx, target_page, table_left_pt, row_top
    // of the first row in this frame, row_bottom of the last row
    // in this frame).
    let mut frame_extents: Vec<(usize, usize, f32, f32, f32)> = Vec::new();
    let mut current_frame_first_top = frame_top_in_page + row_top_y_in_frame;
    let mut current_frame_last_bottom = current_frame_first_top;

    for &h in row_heights.iter() {
        // Advance to the next frame if this row would overflow
        // the current frame and we already have at least one row
        // placed in it. Without the "already placed" guard a row
        // taller than the head frame's available space would
        // trigger an infinite handover loop on a single-frame story.
        let already_placed_in_this_frame = row_bases
            .last()
            .map(|b| b.chain_idx == chain_idx)
            .unwrap_or(false);
        if row_top_y_in_frame + h > frame_height
            && chain_idx + 1 < em.chain.len()
            && already_placed_in_this_frame
        {
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
        }
        let row_top_in_page = frame_top_in_page + row_top_y_in_frame;
        row_bases.push(RowBasis {
            chain_idx,
            target_page,
            table_left_pt: tab_left,
            row_top_in_page,
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
    for r in 0..total_rows {
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
        let basis = row_bases[r];
        let rect = Rect {
            x: basis.table_left_pt,
            y: basis.row_top_in_page,
            w: total_w,
            h: row_heights[r],
        };
        emit_rect(rect, paint, &mut pages[basis.target_page].list);
    }

    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else {
            continue;
        };
        let (c, r) = (c as usize, r as usize);
        if c >= col_widths.len() || r >= row_heights.len() {
            continue;
        }
        let basis = row_bases[r];
        let target_page = basis.target_page;
        let cell_x_pt = basis.table_left_pt + col_x[c];
        let cell_y_pt = basis.row_top_in_page;
        let last_c = (c + cell.column_span.max(1) as usize).min(col_widths.len());
        // For row spans, accumulate heights and clip to the
        // starting frame's bottom so spans that would cross a
        // frame boundary don't fly off-page. For body of work in
        // sample.idml all spans stay within their starting frame.
        let span_rows = cell.row_span.max(1) as usize;
        let last_r = (r + span_rows).min(row_heights.len());
        let mut cell_h_pt = 0.0f32;
        for sr in r..last_r {
            // Stop accumulating if a successor row jumped to a new
            // frame — clip the cell to the originating frame.
            if row_bases[sr].chain_idx != basis.chain_idx {
                break;
            }
            cell_h_pt += row_heights[sr];
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
    }

    // Table-level borders, drawn per-frame so a threaded table
    // gets a top border at the start of the first frame, a bottom
    // border at the end of the last frame, and full left/right
    // borders inside every frame the table touches. Each border
    // segment uses the same filled-rect snap-to-boundary trick as
    // the per-cell edge strokes.
    for (i, (_chain_idx, fp_target_page, frame_table_left, top_y, bottom_y)) in
        frame_extents.iter().enumerate()
    {
        let is_first = i == 0;
        let is_last = i == frame_extents.len() - 1;
        let target = *fp_target_page;
        if is_first {
            if let (Some(color_id), Some(w)) = (
                resolved_table.top_border_stroke_color.as_deref(),
                resolved_table.top_border_stroke_weight,
            ) {
                if w > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
                        emit_rect(
                            Rect {
                                x: *frame_table_left,
                                y: *top_y - w * 0.5,
                                w: total_w,
                                h: w,
                            },
                            paint,
                            &mut pages[target].list,
                        );
                    }
                }
            }
        }
        if is_last {
            if let (Some(color_id), Some(w)) = (
                resolved_table.bottom_border_stroke_color.as_deref(),
                resolved_table.bottom_border_stroke_weight,
            ) {
                if w > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
                        emit_rect(
                            Rect {
                                x: *frame_table_left,
                                y: *bottom_y - w * 0.5,
                                w: total_w,
                                h: w,
                            },
                            paint,
                            &mut pages[target].list,
                        );
                    }
                }
            }
        }
        // Left/right borders span this frame's portion of the table.
        let segment_h = bottom_y - top_y;
        if let (Some(color_id), Some(w)) = (
            resolved_table.left_border_stroke_color.as_deref(),
            resolved_table.left_border_stroke_weight,
        ) {
            if w > 0.0 {
                if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
                    emit_rect(
                        Rect {
                            x: *frame_table_left - w * 0.5,
                            y: *top_y,
                            w,
                            h: segment_h,
                        },
                        paint,
                        &mut pages[target].list,
                    );
                }
            }
        }
        if let (Some(color_id), Some(w)) = (
            resolved_table.right_border_stroke_color.as_deref(),
            resolved_table.right_border_stroke_weight,
        ) {
            if w > 0.0 {
                if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
                    emit_rect(
                        Rect {
                            x: *frame_table_left + total_w - w * 0.5,
                            y: *top_y,
                            w,
                            h: segment_h,
                        },
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
        justification: paragraph.justification.clone(),
        first_line_indent: paragraph.first_line_indent,
        space_before: paragraph.space_before,
        space_after: None, // applied to last sub-paragraph only
        tab_list: paragraph.tab_list.clone(),
        bullets_list_type: paragraph.bullets_list_type.clone(),
        bullet_character: paragraph.bullet_character,
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
                // Close the current sub-paragraph and start a new
                // one. Discard empty sub-paragraphs (consecutive
                // `\n`s, common at the end of bullet lists).
                let mut next = idml_parse::Paragraph {
                    paragraph_style: paragraph.paragraph_style.clone(),
                    justification: paragraph.justification.clone(),
                    first_line_indent: paragraph.first_line_indent,
                    space_before: None,
                    space_after: None,
                    tab_list: paragraph.tab_list.clone(),
                    bullets_list_type: paragraph.bullets_list_type.clone(),
                    bullet_character: paragraph.bullet_character,
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
    if subs.is_empty() {
        // Defensive: the original was all `\n`s. Return a single
        // empty paragraph to keep the upstream loop's stat
        // bookkeeping consistent without rendering anything.
        subs.push(idml_parse::Paragraph {
            paragraph_style: paragraph.paragraph_style.clone(),
            justification: paragraph.justification.clone(),
            first_line_indent: paragraph.first_line_indent,
            space_before: paragraph.space_before,
            space_after: paragraph.space_after,
            tab_list: paragraph.tab_list.clone(),
            bullets_list_type: paragraph.bullets_list_type.clone(),
            bullet_character: paragraph.bullet_character,
            runs: Vec::new(),
            table: None,
        });
    }
    subs
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
    let mut bytes_pool: Vec<bytes::Bytes> = Vec::with_capacity(paragraph.runs.len());
    for resolved in &resolved_runs {
        let Some(b) = em
            .font_table
            .bytes_for(resolved.font.as_deref(), resolved.font_style.as_deref())
        else {
            return 0.0;
        };
        bytes_pool.push(b);
    }
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
    let mut shaping_faces: Vec<Option<rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut outline_faces: Vec<Option<ttf_parser::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    for i in 0..bytes_pool.len() {
        if unique_idx[i] != i {
            continue;
        }
        let bytes_ref = bytes_pool[i].as_ref();
        let Some(mut rf) = rustybuzz::Face::from_slice(bytes_ref, 0) else {
            return 0.0;
        };
        let Ok(mut of) = ttf_parser::Face::parse(bytes_ref, 0) else {
            return 0.0;
        };
        let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
        rf.set_variations(&[rustybuzz::Variation {
            tag: wght_tag,
            value: wghts[i],
        }]);
        let _ = of.set_variation(wght_tag, wghts[i]);
        shaping_faces[i] = Some(rf);
        outline_faces[i] = Some(of);
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
            face: shaping_faces[unique_idx[i]].as_ref().unwrap(),
            point_size: resolved_runs[i]
                .point_size
                .unwrap_or(em.options.default_point_size),
            tracking: resolved_runs[i].tracking,
            font_id: font_ids[i],
            underline: resolved_runs[i].underline.unwrap_or(false),
            strikethru: resolved_runs[i].strikethru.unwrap_or(false),
            baseline_shift_pt: resolved_runs[i].baseline_shift.unwrap_or(0.0),
        })
        .collect();
    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);
    let mut lopts = idml_text::LayoutOptions::new(column_width_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification.as_deref());
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
    );
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

fn build_perline_wrap_widths(
    em: &StoryEmitter,
    styled_runs: &[idml_text::StyledRun],
    lopts: &mut idml_text::LayoutOptions,
) -> WrapPlan {
    let empty = WrapPlan {
        line_x_shifts_64: Vec::new(),
        twin_after: Vec::new(),
    };
    if em.frame_idx != 0 {
        // After the head frame fills, the existing emit loop
        // advances to chain[1+] using a fixed first-baseline
        // reset; per-line wrap inside overflow frames is layered
        // on by the chain walk below — handled when the head
        // frame's paragraph composes.
        return empty;
    }
    let any_chain_overlap = em
        .chain_spread_bounds
        .iter()
        .zip(em.chain_wrap_rects.iter())
        .any(|(b, ws)| {
            ws.iter().any(|w| {
                w.bottom > b.top && w.top < b.bottom && w.right > b.left && w.left < b.right
            })
        });
    if !any_chain_overlap {
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
    for (frame_idx, frame_bounds) in em.chain_spread_bounds.iter().enumerate() {
        let frame_left_pt = frame_bounds.left;
        let frame_right_pt = frame_bounds.right;
        let frame = em.chain[frame_idx];
        let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
        let frame_height_pt = frame_bounds.height();
        let frame_first_baseline_64 = if frame_idx == 0 {
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
        for i in 0..n_lines {
            let baseline_pt = (frame_first_baseline_64 + (i as i32) * leading_64) as f32
                / idml_text::shape::ADVANCE_PRECISION;
            // Line's vertical band in spread coords.
            let line_top = frame_bounds.top + baseline_pt - leading_pt * 0.8;
            let line_bottom = frame_bounds.top + baseline_pt + leading_pt * 0.2;

            let frame_inner_left = frame_left_pt + insets[1];
            let frame_inner_right = frame_right_pt - insets[3];
            // Build the *gap list* of open horizontal segments on
            // this line by subtracting each intruding wrap rect
            // from the [frame_inner_left, frame_inner_right] range.
            let mut segments: Vec<(f32, f32)> = vec![(frame_inner_left, frame_inner_right)];
            for w in wraps {
                if w.bottom <= line_top || w.top >= line_bottom {
                    continue;
                }
                if w.left <= frame_inner_left && w.right >= frame_inner_right {
                    continue;
                }
                let mut next: Vec<(f32, f32)> = Vec::with_capacity(segments.len() + 1);
                for (a, b) in &segments {
                    if w.right <= *a || w.left >= *b {
                        next.push((*a, *b));
                        continue;
                    }
                    if w.left > *a {
                        next.push((*a, w.left));
                    }
                    if w.right < *b {
                        next.push((w.right, *b));
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
                // Nothing usable: fall back to scalar width with no
                // shift so this line at least renders something.
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

fn page_for_frame(frame: &idml_parse::Bounds, pages: &[PageGeom]) -> Option<usize> {
    let cx = (frame.left + frame.right) * 0.5;
    let cy = (frame.top + frame.bottom) * 0.5;
    pages.iter().position(|p| {
        let b = p.bounds_in_spread;
        cx >= b.left && cx <= b.right && cy >= b.top && cy <= b.bottom
    })
}

fn emit_text_frame_into(
    page: &mut BuiltPage,
    frame: &TextFrame,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
    drop_shadow: Option<DropShadow>,
) {
    page.stats.frames += 1;
    let r = Rect {
        x: frame.bounds.left,
        y: frame.bounds.top,
        w: frame.bounds.width(),
        h: frame.bounds.height(),
    };
    let outer = frame_outer_transform(page, frame.item_transform);
    if let Some(shadow) =
        resolve_frame_shadow(frame.drop_shadow.as_ref(), drop_shadow, palette, cmyk_xform)
    {
        emit_drop_shadow_rect_transformed(r, outer, shadow, &mut page.list);
    }
    let fill = frame
        .fill_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        .unwrap_or(fallback);
    emit_rect_transformed(r, outer, fill, &mut page.list);
    if let Some(stroke) = frame
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
    {
        let width = frame.stroke_weight.unwrap_or(1.0);
        if width > 0.0 {
            emit_stroke_rect_transformed(r, outer, Stroke::new(width), stroke, &mut page.list);
        }
    }
}

fn emit_oval_into(
    page: &mut BuiltPage,
    oval: &Oval,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    page.stats.frames += 1;
    let r = Rect {
        x: oval.bounds.left,
        y: oval.bounds.top,
        w: oval.bounds.width(),
        h: oval.bounds.height(),
    };
    let outer = frame_outer_transform(page, oval.item_transform);
    // Ovals don't yet have a dedicated shadow primitive — use the
    // bounding-rect stamp as a stopgap. Replace once the rasterizer
    // grows shadowed-ellipse support.
    if let Some(shadow) = resolve_frame_shadow(oval.drop_shadow.as_ref(), None, palette, cmyk_xform)
    {
        emit_drop_shadow_rect_transformed(r, outer, shadow, &mut page.list);
    }
    let fill = oval
        .fill_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        .unwrap_or(fallback);
    emit_ellipse_transformed(r, outer, fill, &mut page.list);
    if let Some(stroke) = oval
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
    {
        let width = oval.stroke_weight.unwrap_or(1.0);
        if width > 0.0 {
            emit_stroke_ellipse_transformed(r, outer, Stroke::new(width), stroke, &mut page.list);
        }
    }
}

fn emit_line_into(
    page: &mut BuiltPage,
    line: &GraphicLine,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    page.stats.frames += 1;
    // GraphicLines without an explicit StrokeColor inherit the
    // document cascade default (Color/Black). Falling back here
    // keeps real-InDesign exports rendering with visible lines —
    // those frequently leave StrokeColor implicit.
    let stroke_paint = line
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        .or_else(|| color_id_to_paint("Color/Black", palette, cmyk_xform))
        .unwrap_or(Paint::Solid(Color::BLACK));
    let width = line.stroke_weight.unwrap_or(1.0);
    if width <= 0.0 {
        return;
    }
    // GraphicLine.bounds is in inner coords; ItemTransform maps it
    // to spread coords. Without the transform pass the line draws
    // at its untransformed inner-coord origin (typically (0, 0))
    // and disappears off-page when the spread has any origin offset.
    let spread_bounds = transform_bounds(line.bounds, line.item_transform);
    let (ox, oy) = page.spread_origin;
    emit_line(
        spread_bounds.left - ox,
        spread_bounds.top - oy,
        spread_bounds.right - ox,
        spread_bounds.bottom - oy,
        Stroke::new(width),
        stroke_paint,
        &mut page.list,
    );
}

fn emit_rectangle_into(
    page: &mut BuiltPage,
    rect: &Rectangle,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&idml_color::IccTransform>,
    drop_shadow: Option<DropShadow>,
) {
    page.stats.frames += 1;
    let r = Rect {
        x: rect.bounds.left,
        y: rect.bounds.top,
        w: rect.bounds.width(),
        h: rect.bounds.height(),
    };
    let outer = frame_outer_transform(page, rect.item_transform);
    if let Some(shadow) =
        resolve_frame_shadow(rect.drop_shadow.as_ref(), drop_shadow, palette, cmyk_xform)
    {
        emit_drop_shadow_rect_transformed(r, outer, shadow, &mut page.list);
    }
    let fill = rect
        .fill_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        .unwrap_or(fallback);
    let fill = apply_fill_tint(fill, rect.fill_tint);
    let fill = apply_opacity(fill, rect.opacity);
    let stroke_paint = rect
        .stroke_color
        .as_deref()
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
        .map(|p| apply_opacity(p, rect.opacity));
    let stroke_width = rect.stroke_weight.unwrap_or(1.0);
    let stroke_offset = stroke_alignment_offset(rect.stroke_alignment.as_deref(), stroke_width);
    let blend_mode = blend_mode_from_idml(rect.blend_mode.as_deref());
    let radius = corner_radius_for(rect);
    if let Some(radius) = radius {
        // Rounded path: build once, intern, fill + (optional) stroke.
        // Cache key includes the radius so different radii on the
        // same Self id get distinct interned paths.
        let path = rounded_rect_path(r, radius);
        let key_bytes = rect
            .self_id
            .as_deref()
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_else(|| format!("{:?}", r).into_bytes());
        let key = fnv_1a_u64(&[key_bytes.as_slice(), &radius.to_bits().to_le_bytes()].concat());
        let (path_id, _) = page.list.paths.intern(key, path);
        if matches!(blend_mode, idml_compose::BlendMode::Normal) {
            page.list.commands.push(DisplayCommand::FillPath {
                path_id,
                paint: fill,
                transform: outer,
            });
        } else {
            page.list.commands.push(DisplayCommand::FillPathBlend {
                path_id,
                paint: fill,
                transform: outer,
                blend_mode,
            });
        }
        if let (Some(stroke), true) = (stroke_paint, stroke_width > 0.0) {
            // Inside/Outside alignment shifts the stroke path inward
            // or outward by W/2, with matching radius adjustment so
            // the corners stay tangent to the geometry.
            let stroke_rect = inset_rect(r, stroke_offset);
            let stroke_radius = (radius - stroke_offset).max(0.0);
            let stroke_path = rounded_rect_path(stroke_rect, stroke_radius);
            let stroke_key = fnv_1a_u64(
                &[
                    key_bytes.as_slice(),
                    &stroke_radius.to_bits().to_le_bytes(),
                    &stroke_offset.to_bits().to_le_bytes(),
                    b"sa",
                ]
                .concat(),
            );
            let (stroke_path_id, _) = page.list.paths.intern(stroke_key, stroke_path);
            page.list.commands.push(DisplayCommand::StrokePath {
                path_id: stroke_path_id,
                paint: stroke,
                stroke: stroke_for(
                    rect.stroke_type.as_deref(),
                    stroke_width,
                    rect.end_cap.as_deref(),
                    rect.end_join.as_deref(),
                    rect.miter_limit,
                ),
                transform: outer,
            });
        }
        return;
    }
    idml_compose::emit_rect_transformed_blend(r, outer, fill, blend_mode, &mut page.list);
    if let (Some(stroke), true) = (stroke_paint, stroke_width > 0.0) {
        emit_stroke_rect_transformed(
            inset_rect(r, stroke_offset),
            outer,
            stroke_for(
                rect.stroke_type.as_deref(),
                stroke_width,
                rect.end_cap.as_deref(),
                rect.end_join.as_deref(),
                rect.miter_limit,
            ),
            stroke,
            &mut page.list,
        );
    }
}

/// Half the stroke width to shift the stroke path by, signed so that
/// positive shrinks inward (Inside alignment) and negative grows
/// outward (Outside alignment). `CenterAlignment` and `None` return 0.
fn stroke_alignment_offset(alignment: Option<&str>, stroke_width: f32) -> f32 {
    match alignment {
        Some("InsideAlignment") => stroke_width * 0.5,
        Some("OutsideAlignment") => -stroke_width * 0.5,
        _ => 0.0,
    }
}

/// Map IDML's `<BlendingSetting BlendMode="...">` enum string to the
/// compose-layer `BlendMode`. Unknown / absent values fall back to
/// Normal. Names mirror Adobe's PDF blend-mode catalogue.
fn blend_mode_from_idml(name: Option<&str>) -> idml_compose::BlendMode {
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
fn inset_rect(r: Rect, delta: f32) -> Rect {
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
fn apply_opacity(paint: Paint, opacity_pct: Option<f32>) -> Paint {
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
fn corner_radius_for(rect: &Rectangle) -> Option<f32> {
    let radius = rect.corner_radius?;
    if radius <= 0.0 {
        return None;
    }
    match rect.corner_option.as_deref() {
        // The decorative variants (Inverse-Rounded, Inset, Bevel, Fancy)
        // currently fall back to plain Rounded. Replace per-corner-option
        // path emission lands later.
        Some("Rounded")
        | Some("InverseRounded")
        | Some("Inset")
        | Some("Bevel")
        | Some("Fancy") => Some(radius),
        _ => None,
    }
}

/// Build a rounded-rect path with cubic-Bezier quarter-circle corners
/// (control offset = `radius * 0.5523`). The path is emitted in the
/// rectangle's *inner* coordinate system (same coords as `rect.x` /
/// `rect.y`); the renderer's `outer` transform handles spread-origin
/// and ItemTransform composition the same way it does for polygons.
/// Walks clockwise from the top edge.
fn rounded_rect_path(rect: Rect, radius: f32) -> idml_compose::PathData {
    use idml_compose::PathSegment::*;
    let r = radius.min(rect.w * 0.5).min(rect.h * 0.5).max(0.0);
    let l = rect.x;
    let t = rect.y;
    let right = rect.x + rect.w;
    let bot = rect.y + rect.h;
    // Cubic-Bezier control offset for a quarter-circle of radius r.
    const KAPPA: f32 = 0.552_284_8;
    let k = r * KAPPA;
    idml_compose::PathData {
        segments: vec![
            MoveTo { x: l + r, y: t },
            LineTo { x: right - r, y: t },
            CubicTo {
                cx1: right - r + k,
                cy1: t,
                cx2: right,
                cy2: t + r - k,
                x: right,
                y: t + r,
            },
            LineTo {
                x: right,
                y: bot - r,
            },
            CubicTo {
                cx1: right,
                cy1: bot - r + k,
                cx2: right - r + k,
                cy2: bot,
                x: right - r,
                y: bot,
            },
            LineTo { x: l + r, y: bot },
            CubicTo {
                cx1: l + r - k,
                cy1: bot,
                cx2: l,
                cy2: bot - r + k,
                x: l,
                y: bot - r,
            },
            LineTo { x: l, y: t + r },
            CubicTo {
                cx1: l,
                cy1: t + r - k,
                cx2: l + r - k,
                cy2: t,
                x: l + r,
                y: t,
            },
            Close,
        ],
    }
}

/// Resolve, decode, and emit a placed image for a rectangle. Skips
/// silently if `assets` is unset, the resolver returns `None`, or
/// decoding fails — IDMLs without their linked assets should still
/// produce a usable render of the surrounding geometry.
fn emit_rectangle_image(
    page: &mut BuiltPage,
    rect: &Rectangle,
    options: &PipelineOptions,
    page_image_cache: &mut HashMap<String, idml_compose::ImageId>,
    decoded_cache: &mut HashMap<String, idml_compose::DecodedImage>,
) {
    let Some(uri) = rect.image_link.as_deref() else {
        return;
    };
    // Default placement = the frame's inner-coord rect. FrameFitting
    // crops grow / shrink the image bounds — negative crops typically
    // come from `FillProportionally` (image overflows the frame). We
    // emit the expanded rect; per-frame clipping will land when the
    // display list grows a Clip primitive.
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
    let outer = frame_outer_transform(page, rect.item_transform);
    let id = if let Some(cached) = page_image_cache.get(uri) {
        *cached
    } else {
        // Cross-page decode cache hit: clone the cached RGBA into
        // this page's image pool.
        let decoded = if let Some(d) = decoded_cache.get(uri) {
            d.clone()
        } else {
            let Some(resolver) = options.assets else {
                return;
            };
            let Some(bytes) = resolver.resolve_image(uri) else {
                tracing::warn!(uri, "image resolver returned no bytes; skipping");
                return;
            };
            let Some(d) = decode_image_bytes(bytes.as_ref()) else {
                tracing::warn!(uri, "image decode failed; skipping");
                return;
            };
            decoded_cache.insert(uri.to_string(), d.clone());
            d
        };
        let id = page.list.push_image(decoded);
        page_image_cache.insert(uri.to_string(), id);
        id
    };
    idml_compose::emit_image_at(r, outer, id, &mut page.list);
}

/// Decode raw image bytes to RGBA8. Format detection is via magic
/// bytes (`image::load_from_memory`). Returns `None` for any decode
/// or buffer-shape failure.
fn decode_image_bytes(bytes: &[u8]) -> Option<idml_compose::DecodedImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(idml_compose::DecodedImage {
        width,
        height,
        rgba: rgba.into_raw(),
    })
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

/// Resolve the effective shadow for a frame. Per-frame IDML shadow
/// wins; the synthetic `fallback` (from `PipelineOptions`) is used
/// when the frame carries none. Returns `None` for fully-transparent
/// shadows so callers don't emit a no-op.
fn resolve_frame_shadow(
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
        .and_then(paint_as_solid)
        .unwrap_or(Color::BLACK);
    Some(DropShadow {
        offset_x: setting.x_offset,
        offset_y: setting.y_offset,
        blur_radius: setting.size,
        color,
        opacity,
    })
}

/// Pull the inner `Color` out of a solid paint, returning `None`
/// for gradient (or future image) paints. Used wherever a context
/// can only consume a flat colour (drop shadow, per-glyph paint).
fn paint_as_solid(p: Paint) -> Option<Color> {
    match p {
        Paint::Solid(c) => Some(c),
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
            lopts.alignment = map_justification(paragraph.justification.as_deref());
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
pub fn color_id_to_paint(
    id: &str,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) -> Option<Paint> {
    let entry = palette.resolve(id)?;
    if let (Some(xform), idml_parse::ColorSpace::Cmyk) = (cmyk_xform, entry.space) {
        if entry.value.len() == 4 {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let cmyk = idml_color::Cmyk {
                    c: entry.value[0],
                    m: entry.value[1],
                    y: entry.value[2],
                    k: entry.value[3],
                };
                let idml_color::LinearRgb([r, g, b]) = xform.cmyk_percent_to_linear_rgb(cmyk);
                return Some(Paint::Solid(Color::rgba(r, g, b, 1.0)));
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = xform;
            }
        }
    }
    let [r, g, b] = graphic::to_linear_rgb(entry)?;
    Some(Paint::Solid(Color::rgba(r, g, b, 1.0)))
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
    if let Some(grad) = palette.gradients.get(id) {
        let stops: Vec<idml_compose::GradientStop> = grad
            .stops
            .iter()
            .filter_map(|s| {
                let color = color_id_to_paint(&s.stop_color, palette, cmyk_xform)
                    .and_then(paint_as_solid)?;
                Some(idml_compose::GradientStop {
                    offset: (s.location_pct / 100.0).clamp(0.0, 1.0),
                    color,
                })
            })
            .collect();
        if stops.len() < 2 {
            return None;
        }
        // Default endpoints: top-to-bottom across the unit square.
        // Frame-level GradientFillStart / Length / Angle attributes
        // override these — that wiring lands when the spread parser
        // captures those fields.
        let id = list.push_linear_gradient(idml_compose::LinearGradient {
            start: (0.0, 0.0),
            end: (0.0, 1.0),
            stops,
        });
        return Some(Paint::LinearGradient(id));
    }
    color_id_to_paint(id, palette, cmyk_xform)
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
        let paint = run
            .fill_color
            .as_deref()
            .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
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
fn build_run_paint_picker_resolved(
    paragraph: &idml_parse::Paragraph,
    resolved_runs: &[idml_scene::ResolvedRunAttrs],
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    default: Paint,
) -> RunPaintPicker {
    let mut bands: Vec<(u32, Paint)> = Vec::with_capacity(paragraph.runs.len());
    let mut cursor: u32 = 0;
    for (i, run) in paragraph.runs.iter().enumerate() {
        // Resolve the swatch (or fall through to `default`) FIRST,
        // then apply the run's `FillTint`. The tint affects both
        // explicit swatches and the default paint — IDML treats it
        // as a strength-of-current-fill modifier independent of
        // whether the run carries a FillColor attribute.
        let base = resolved_runs[i]
            .fill_color
            .as_deref()
            .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
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
    // axis. We translate to the composer's stretch/shrink ratios as
    // (max - desired) / desired and (desired - min) / desired so the
    // breaker gets a relative range matching what InDesign penalises.
    let desired = resolved.desired_word_spacing.unwrap_or(100.0).max(1.0);
    if let Some(max) = resolved.maximum_word_spacing {
        lopts.compose.stretch_ratio = ((max - desired) / desired).max(0.0);
    }
    if let Some(min) = resolved.minimum_word_spacing {
        lopts.compose.shrink_ratio = ((desired - min) / desired).clamp(0.0, 1.0);
    }
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
fn stroke_for(
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
fn apply_fill_tint(paint: Paint, tint_pct: Option<f32>) -> Paint {
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

/// Map IDML `Justification` attribute values to `idml_text::Alignment`.
/// Unknown or missing values fall back to `Left`.
pub fn map_justification(j: Option<&str>) -> idml_text::Alignment {
    match j {
        Some("RightAlign") | Some("RightJustified") => idml_text::Alignment::Right,
        Some("CenterAlign") | Some("CenterJustified") => idml_text::Alignment::Center,
        Some("FullyJustified") | Some("LeftJustified") => idml_text::Alignment::Justify,
        _ => idml_text::Alignment::Left,
    }
}

/// Map IDML `<TabStop Alignment="...">` values to the layout
/// crate's `TabAlignment`.
/// Build the list-marker prefix for a paragraph, or `None` when no
/// list applies. Mutates `counter` per IDML's
/// `NumberingContinue=true` default:
///  - BulletList: counter resets to 0 (bullets don't number);
///    returns `<bullet><separator>`.
///  - NumberedList: counter increments and the marker is
///    `<n>.<tab>` using Arabic numerals.
///  - NoList / absent: counter resets to 0; returns `None`.
///
/// The `\t` after a numbered marker is handled by the existing
/// tab-stop pass — the renderer's default 36 pt grid gives a
/// reasonable hanging indent without explicit `<TabList>`.
///
/// Other NumberingFormat variants (Roman, alpha, zero-padded)
/// fall through to Arabic for now; lands as a follow-up.
fn list_prefix(p: &idml_scene::ResolvedParagraphAttrs, counter: &mut u32) -> Option<String> {
    match p.bullets_list_type.as_deref() {
        Some("BulletList") => {
            *counter = 0;
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
            *counter = counter.checked_add(1).unwrap_or(1);
            let formatted = format_number(*counter, p.numbering_format.as_deref());
            // Two regular spaces after the period — a literal tab
            // would shape via the font's .notdef glyph (tofu) when
            // the run's font lacks a tab mapping. The original
            // intent is "advance past the marker"; two spaces gives
            // a similar visual gap without a missing-glyph rectangle.
            Some(format!("{formatted}.  "))
        }
        _ => {
            *counter = 0;
            None
        }
    }
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
struct FontTable {
    cache: HashMap<(String, Option<String>), Bytes>,
    fallback: Option<Bytes>,
    /// Metrics keyed by `fnv_1a_u32(bytes)` (same id the rest of
    /// the pipeline uses for glyph-cache routing).
    metrics: HashMap<u32, FontMetrics>,
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
            for parsed in &document.stories {
                for paragraph in &parsed.story.paragraphs {
                    for run in &paragraph.runs {
                        let resolved = document.resolved_run_attrs(paragraph, run);
                        let Some(family) = resolved.font else {
                            continue;
                        };
                        keys.insert((family, resolved.font_style));
                    }
                }
            }
            cache.reserve(keys.len());
            for key in keys {
                if let Some(bytes) = resolver.resolve_font(&key.0, key.1.as_deref()) {
                    cache.insert(key, bytes);
                }
            }
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
        Self {
            cache,
            fallback,
            metrics,
        }
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

    fn metrics_for(&self, font_id: u32) -> Option<&FontMetrics> {
        self.metrics.get(&font_id)
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
        let p = list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
            &mut counter,
        )
        .unwrap();
        assert_eq!(p, "\u{2022} ");
        assert_eq!(counter, 0, "BulletList resets counter");
    }

    #[test]
    fn list_prefix_expands_caret_t_to_tab() {
        let mut counter = 0;
        let p = list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some("^t")),
            &mut counter,
        )
        .unwrap();
        assert_eq!(p, "\u{2022}\t");
    }

    #[test]
    fn list_prefix_none_for_nolist_resets_counter() {
        let mut counter = 5;
        assert!(list_prefix(&attrs(Some("NoList"), None, None), &mut counter).is_none());
        assert_eq!(counter, 0);
    }

    #[test]
    fn list_prefix_numbered_increments_across_paragraphs() {
        let mut counter = 0;
        let attrs = attrs(Some("NumberedList"), None, None);
        assert_eq!(list_prefix(&attrs, &mut counter).as_deref(), Some("1.\t"));
        assert_eq!(list_prefix(&attrs, &mut counter).as_deref(), Some("2.\t"));
        assert_eq!(list_prefix(&attrs, &mut counter).as_deref(), Some("3.\t"));
        assert_eq!(counter, 3);
    }

    #[test]
    fn list_prefix_numbered_resets_after_non_numbered() {
        let mut counter = 0;
        let n = attrs(Some("NumberedList"), None, None);
        let none = attrs(None, None, None);
        list_prefix(&n, &mut counter); // 1.
        list_prefix(&n, &mut counter); // 2.
        list_prefix(&none, &mut counter); // resets
        assert_eq!(counter, 0);
        assert_eq!(list_prefix(&n, &mut counter).as_deref(), Some("1.\t"));
    }

    #[test]
    fn list_prefix_bullet_to_numbered_resets() {
        // Mixing list types in a row also resets — each list_type
        // change starts a fresh sequence.
        let mut counter = 0;
        list_prefix(
            &attrs(Some("BulletList"), Some(0x2022), Some(" ")),
            &mut counter,
        );
        assert_eq!(counter, 0);
        let n = attrs(Some("NumberedList"), None, None);
        assert_eq!(list_prefix(&n, &mut counter).as_deref(), Some("1.\t"));
    }

    #[test]
    fn list_prefix_bullet_none_when_codepoint_missing() {
        let mut counter = 0;
        assert!(list_prefix(&attrs(Some("BulletList"), None, Some(" ")), &mut counter).is_none());
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
        let mut a = attrs(Some("NumberedList"), None, None);
        a.numbering_format = Some("I, II, III, IV...".to_string());
        assert_eq!(list_prefix(&a, &mut counter).as_deref(), Some("I.\t"));
        assert_eq!(list_prefix(&a, &mut counter).as_deref(), Some("II.\t"));
        assert_eq!(list_prefix(&a, &mut counter).as_deref(), Some("III.\t"));
    }
}
