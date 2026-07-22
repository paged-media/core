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

//! The document build + story-emission engine: build_document_inner +
//! StoryEmitter (per-story paragraph composition, chain threading) +
//! index/TOC paragraph builders. Extracted from pipeline/mod.rs (1.6b).

use super::*;
use std::collections::HashMap;

use paged_compose::{
    emit_glyph_slice, emit_glyph_slice_stroke, DisplayList, DropShadow, Paint, PathData,
    PathSegment, Rect, Transform, TtfOutliner,
};
use paged_model::{Graphic, PathAnchor, TextFrame};
use paged_scene::Document;

use crate::diagnostics::{Diagnostic, DiagnosticCode, RenderDiagnostics};
use crate::module::geometry::rewrite_tail_for_overprint;

pub(super) fn build_document_inner(
    document: &Document,
    options: &PipelineOptions,
    post: Option<&PostLayoutCtx>,
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

    // W1.18c — the master-text / body-story emit caches are bypassed on
    // the post-layout (second) pass: their deltas were captured during
    // the first pass with the PRE-running-header variable text, so a
    // splice would re-introduce the stale value. The first pass still
    // populates + reads them as before (the shared RefCell survives), so
    // gesture-rebuild callers keep their cache hit on the next build.
    let master_text_emit_cache = if post.is_some() {
        None
    } else {
        options.master_text_emit_cache
    };
    let body_story_emit_cache = if post.is_some() {
        None
    } else {
        options.body_story_emit_cache
    };

    // W1.7 Phase B: precompute each AutoSizing text frame's GROWN
    // inner-coord bounds, keyed by `Self` id. Computed once up front so
    // the frame-paint pass (the box stretches to fit) and the text-wrap
    // collection (neighbours wrap around the grown box) both see the
    // same extent. Only frames that actually grow get an entry.
    let auto_sized_bounds: HashMap<String, paged_model::Bounds> = document
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
    let mut section_walk = SectionWalk::new(&document.designmap.sections);
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
                resource_tiles_needed: Vec::new(),
            });
        }
        spread_page_ranges.push(start..pages.len());
    }
    total_stats.pages = pages.len();
    // Surface that one or more page labels were computed rather than
    // read from a baked `<Page Name>` — an honest signal that numbering
    // came from section rules / the 1-based fallback, not InDesign.
    if section_walk.used_fallback {
        let detail = if document.designmap.sections.is_empty() {
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
            resource_tiles_needed: Vec::new(),
        });
        page_geometries.push(PageGeom {
            bounds_in_spread: paged_model::Bounds {
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
    let mut page_index_map: HashMap<String, u32> = if options.collect_link_regions {
        pages
            .iter()
            .enumerate()
            .map(|(idx, p)| (p.id.0.clone(), idx as u32))
            .collect()
    } else {
        HashMap::new()
    };
    // W1.19 — on the post-layout pass, fold in story / text-anchor →
    // page entries so a cross-reference (or text-anchor hyperlink) whose
    // destination is a story resolves to the page that story landed on.
    // `<Page Self>` entries stay authoritative (a real page id never
    // collides with a story id).
    if let Some(post) = post {
        for (id, idx) in &post.story_page {
            page_index_map.entry(id.clone()).or_insert(*idx);
        }
    }

    // W1.18b — chapter number per flat body-page index, computed once
    // from `<Section>` settings. `<Page Self>` → flat index lets us find
    // which section owns each page. Empty Strings (and an all-empty
    // table) when the document declares no sections, in which case
    // `ChapterNumberType` variables fall back to their baked text.
    let page_starts: HashMap<String, usize> = pages
        .iter()
        .enumerate()
        .map(|(idx, p)| (p.id.0.clone(), idx))
        .collect();
    let sections = &document.designmap.sections;
    let chapter_numbers: Vec<String> = (0..pages.len())
        .map(|idx| links::chapter_number_for_page(sections, &page_starts, idx).unwrap_or_default())
        .collect();
    // W1.18c — the post-layout running-header pickup index, threaded into
    // the emit passes only on the second (post-layout) build. `None` on
    // the first pass — running headers then keep their baked value.
    let running_header_index: Option<&links::RunningHeaderIndex> = post.map(|p| &p.running_headers);

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
        let master_page_bounds: Vec<paged_model::Bounds> = master
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
        let master_page_for = |b: paged_model::Bounds| -> usize {
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
        let item_belongs = |b: paged_model::Bounds| -> bool {
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
    let layer_renders = paged_scene::build_layer_render_map(&document.designmap);
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
    let layer_z_index = paged_scene::layer_z_index(&document.designmap);
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
        let layer_z_of = |fr: paged_model::FrameRef| -> usize {
            let id = match fr {
                paged_model::FrameRef::TextFrame(i) => spread
                    .text_frames
                    .get(i)
                    .and_then(|f| f.item_layer.as_deref()),
                paged_model::FrameRef::Rectangle(i) => spread
                    .rectangles
                    .get(i)
                    .and_then(|f| f.item_layer.as_deref()),
                paged_model::FrameRef::Oval(i) => {
                    spread.ovals.get(i).and_then(|f| f.item_layer.as_deref())
                }
                paged_model::FrameRef::GraphicLine(i) => spread
                    .graphic_lines
                    .get(i)
                    .and_then(|f| f.item_layer.as_deref()),
                paged_model::FrameRef::Polygon(i) => {
                    spread.polygons.get(i).and_then(|f| f.item_layer.as_deref())
                }
                // Group: derive layer from the first leaf member with
                // an ItemLayer. If none, treat as "no layer" (MAX).
                paged_model::FrameRef::Group(_) => None,
            };
            id.and_then(|s| layer_z_index.get(s).copied())
                .unwrap_or(usize::MAX)
        };
        let frames_ordered: Vec<paged_model::FrameRef> = if spread.frames_in_order.is_empty() {
            // Legacy path: a parser revision predating
            // `frames_in_order` (or a spread carrying only frames the
            // parser couldn't classify) → fall through to the same
            // XML-vec walk as before. Builds a synthetic flat list by
            // concatenating the per-shape vecs in their historical
            // order.
            let mut v: Vec<paged_model::FrameRef> = Vec::new();
            v.extend((0..spread.text_frames.len()).map(paged_model::FrameRef::TextFrame));
            v.extend((0..spread.rectangles.len()).map(paged_model::FrameRef::Rectangle));
            v.extend((0..spread.ovals.len()).map(paged_model::FrameRef::Oval));
            v.extend((0..spread.graphic_lines.len()).map(paged_model::FrameRef::GraphicLine));
            v.extend((0..spread.polygons.len()).map(paged_model::FrameRef::Polygon));
            v
        } else {
            let mut keyed: Vec<(usize, usize, paged_model::FrameRef)> = spread
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
            fr: paged_model::FrameRef,
            spread: &paged_model::Spread,
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
            auto_sized_bounds: &HashMap<String, paged_model::Bounds>,
        ) {
            match fr {
                paged_model::FrameRef::TextFrame(idx) => {
                    let Some(frame) = spread.text_frames.get(idx) else {
                        return;
                    };
                    // W2.5 — element-level Visible="false" hides the item
                    // (same skip as a hidden layer). Locked is NOT a render
                    // gate (locked items still paint); selection-gating
                    // lives in the canvas hit-tester.
                    if !is_layer_visible(document, frame.item_layer.as_deref()) || !frame.visible {
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
                        // C-1: a plugin scene layer renders inside this frame.
                        emit_frame_scene_layer(
                            &mut pages[page_idx],
                            frame.self_id.as_deref(),
                            frame.bounds,
                            frame.inset_spacing,
                            frame.item_transform,
                            options.scene_layers,
                            options.font,
                        );
                        // C-6: a claimed image provider assembles pyramid
                        // tiles inside this frame.
                        emit_frame_resource_tiles(
                            &mut pages[page_idx],
                            frame.self_id.as_deref(),
                            frame.bounds,
                            frame.inset_spacing,
                            frame.item_transform,
                            options.resource_providers,
                            options.render_scale,
                        );
                    }
                }
                paged_model::FrameRef::Rectangle(idx) => {
                    let Some(rect) = spread.rectangles.get(idx) else {
                        return;
                    };
                    // W2.5 — element-level Visible="false" hides the item.
                    if !is_layer_visible(document, rect.item_layer.as_deref()) || !rect.visible {
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
                        // C-1: a plugin scene layer renders inside this frame
                        // (a rectangle's content box is its bounds — no inset).
                        emit_frame_scene_layer(
                            &mut pages[page_idx],
                            rect.self_id.as_deref(),
                            rect.bounds,
                            None,
                            rect.item_transform,
                            options.scene_layers,
                            options.font,
                        );
                        // C-6: a claimed image provider assembles pyramid
                        // tiles inside this frame.
                        emit_frame_resource_tiles(
                            &mut pages[page_idx],
                            rect.self_id.as_deref(),
                            rect.bounds,
                            None,
                            rect.item_transform,
                            options.resource_providers,
                            options.render_scale,
                        );
                    }
                }
                paged_model::FrameRef::Oval(idx) => {
                    let Some(oval) = spread.ovals.get(idx) else {
                        return;
                    };
                    // W2.5 — element-level Visible="false" hides the item.
                    if !is_layer_visible(document, oval.item_layer.as_deref()) || !oval.visible {
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
                        // C-1: a plugin scene layer renders inside this frame.
                        emit_frame_scene_layer(
                            &mut pages[page_idx],
                            oval.self_id.as_deref(),
                            oval.bounds,
                            None,
                            oval.item_transform,
                            options.scene_layers,
                            options.font,
                        );
                        // C-6: a claimed image provider assembles pyramid
                        // tiles inside this frame.
                        emit_frame_resource_tiles(
                            &mut pages[page_idx],
                            oval.self_id.as_deref(),
                            oval.bounds,
                            None,
                            oval.item_transform,
                            options.resource_providers,
                            options.render_scale,
                        );
                    }
                }
                paged_model::FrameRef::GraphicLine(idx) => {
                    let Some(line) = spread.graphic_lines.get(idx) else {
                        return;
                    };
                    // W2.5 — element-level Visible="false" hides the item.
                    if !is_layer_visible(document, line.item_layer.as_deref()) || !line.visible {
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
                        // C-1: a plugin scene layer renders inside this frame.
                        emit_frame_scene_layer(
                            &mut pages[page_idx],
                            line.self_id.as_deref(),
                            line.bounds,
                            None,
                            line.item_transform,
                            options.scene_layers,
                            options.font,
                        );
                        // C-6: a claimed image provider assembles pyramid
                        // tiles inside this frame.
                        emit_frame_resource_tiles(
                            &mut pages[page_idx],
                            line.self_id.as_deref(),
                            line.bounds,
                            None,
                            line.item_transform,
                            options.resource_providers,
                            options.render_scale,
                        );
                    }
                }
                paged_model::FrameRef::Polygon(idx) => {
                    let Some(poly) = spread.polygons.get(idx) else {
                        return;
                    };
                    // W2.5 — element-level Visible="false" hides the item.
                    if !is_layer_visible(document, poly.item_layer.as_deref()) || !poly.visible {
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
                        // C-1: a plugin scene layer renders inside this frame.
                        emit_frame_scene_layer(
                            &mut pages[page_idx],
                            poly.self_id.as_deref(),
                            poly.bounds,
                            None,
                            poly.item_transform,
                            options.scene_layers,
                            options.font,
                        );
                        // C-6: a claimed image provider assembles pyramid
                        // tiles inside this frame.
                        emit_frame_resource_tiles(
                            &mut pages[page_idx],
                            poly.self_id.as_deref(),
                            poly.bounds,
                            None,
                            poly.item_transform,
                            options.resource_providers,
                            options.render_scale,
                        );
                    }
                }
                paged_model::FrameRef::Group(gi) => {
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
        if let (Some(ref key), Some(rc)) = (&cache_key, master_text_emit_cache) {
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
        .with_page_index_map(&page_index_map)
        .with_chapter_numbers(&chapter_numbers);
        if let Some(index) = running_header_index {
            emitter = emitter.with_running_headers(index);
        }
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
        if let (Some(ref key), Some(rc), false) = (&cache_key, master_text_emit_cache, uncacheable)
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
            // W2.5 — element-level Visible="false" hides the shape and
            // its text-on-path.
            if !layer_visible(poly.item_layer.as_deref()) || !poly.visible {
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
            // W2.5 — element-level Visible="false" hides the shape and
            // its text-on-path.
            if !layer_visible(rect.item_layer.as_deref()) || !rect.visible {
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
            // W2.5 — element-level Visible="false" hides the shape and
            // its text-on-path.
            if !layer_visible(line.item_layer.as_deref()) || !line.visible {
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
            if body_story_emit_cache.is_some() && cross_story_numbering.is_none() {
                Some((
                    parsed.self_id.clone(),
                    body_story_signature(&chain, &chain_pages_pre, &wrap_rects_per_page),
                ))
            } else {
                None
            };
        if let (Some(ref key), Some(rc)) = (&cache_key, body_story_emit_cache) {
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
        let toc_paragraphs: Option<Vec<paged_model::Paragraph>> = chain
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
            Some(paged_model::StoryDirection::VerticalWritingDirection)
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
            .with_chapter_numbers(&chapter_numbers)
            .with_footnote_reservation(&reserved_64);
            // W1.18c — running-header pickup index on the post-layout pass.
            if let Some(index) = running_header_index {
                emitter = emitter.with_running_headers(index);
            }
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
        if let (Some(ref key), Some(rc)) = (&cache_key, body_story_emit_cache) {
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

    // C-6 — aggregate each page's tile-miss requests verbatim (they carry
    // their own image_id; no page backfill needed).
    let resource_tiles_needed: Vec<_> = pages
        .iter()
        .flat_map(|p| p.resource_tiles_needed.iter().cloned())
        .collect();

    Ok(BuiltDocument {
        pages,
        stats: total_stats,
        breaks,
        diagnostics,
        resource_tiles_needed,
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
pub(super) struct StoryEmitter<'a> {
    pub(super) document: &'a Document,
    pub(super) options: &'a PipelineOptions<'a>,
    pub(super) palette: &'a Graphic,
    /// Reserved for the upcoming CMYK text-fill path. The current
    /// per-glyph paint picker resolves through `palette` directly.
    #[allow(dead_code)]
    pub(super) cmyk_xform: Option<&'a paged_color::IccTransform>,
    pub(super) font_table: &'a FontTable,
    pub(super) chain: Vec<&'a TextFrame>,
    pub(super) chain_pages: Vec<usize>,
    /// User-visible page labels indexed by flat body-page idx (parallel
    /// to `pages`). The auto-page-number marker substitutes
    /// `page_labels[chain_pages[frame_idx]]`; ACE 19 looks one slot
    /// further ahead. Owned by the document, not the emitter.
    pub(super) page_labels: &'a [String],
    /// Pre-built hyphenator for the document's primary language.
    /// `None` ⇒ the document opts out of hyphenation entirely (the
    /// composer skips the language-specific pattern lookup).
    pub(super) hyphenator: Option<&'a paged_text::Hyphenator>,
    pub(super) column_width_pt: Option<f32>,
    /// Inner-coord x-shift to apply to the head frame's text
    /// origin when an obstacle on the page intrudes from the left
    /// of the frame for the *whole* frame's height. Zero unless
    /// wrap rectangles overlap the head frame.
    pub(super) column_x_shift_pt: f32,
    /// Spread-coord wrap exclusion rectangles for the head frame's
    /// page. Per-paragraph wrap (per-line column carving) reads
    /// these and computes a `column_widths` slice + per-line
    /// glyph x-shifts so body text flows around an island
    /// obstacle (the chairman page's pull quote, for example).
    /// Superseded by `chain_wrap_rects[0]` for the per-line walk;
    /// retained alongside `head_frame_spread` for callers that
    /// want the head's wraps without indexing.
    #[allow(dead_code)]
    pub(super) head_wrap_rects: Vec<WrapShape>,
    /// Spread-coord bounds of the head frame, cached so the
    /// per-paragraph wrap pass doesn't recompute per paragraph.
    /// Currently superseded by `chain_spread_bounds[0]` for the
    /// per-line walk; retained for future per-frame optimisations
    /// that read the head's bounds without indexing.
    #[allow(dead_code)]
    pub(super) head_frame_spread: paged_model::Bounds,
    /// Spread-coord wrap exclusion rectangles per chain index — the
    /// threaded-frame extension of `head_wrap_rects`. Each chain
    /// index `i` carries the wrap rectangles on chain[i]'s page.
    /// Used by `build_perline_wrap_widths` so overflow lines that
    /// land in chain[1+] get the right exclusions for that frame's
    /// page.
    pub(super) chain_wrap_rects: Vec<Vec<WrapShape>>,
    /// Spread-coord bounds for every frame in the chain. Same
    /// motivation as `chain_wrap_rects`: per-frame per-line wrap
    /// needs each frame's spread rect.
    pub(super) chain_spread_bounds: Vec<paged_model::Bounds>,
    pub(super) frame_idx: usize,
    pub(super) y_cursor: i32,
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
    pub(super) prev_line_height_64: Option<i32>,
    pub(super) frame_cmd_ranges: Vec<Option<(usize, usize)>>,
    pub(super) frame_max_baseline_64: Vec<i32>,
    /// W1.7 footnote space reservation — per chain-frame height (in
    /// 1/64 pt) to hold back from the bottom of the frame's text area
    /// for the footnote pool. Body text overflows to the next frame
    /// once a line's baseline crosses `frame_height - reserved`, so the
    /// pool drawn in the post-pass sits *below* the last body line
    /// instead of overlapping it. All zero on the first (measuring)
    /// emit; the body-story loop fills it from the measured pool
    /// heights and re-emits to a fixpoint. See `emit_footnote_pools`
    /// and the reservation loop in `build_document`.
    pub(super) reserved_footnote_64: Vec<i32>,
    /// Per-frame list of `(cmd_start, cmd_end)` slices, one entry
    /// per paragraph that contributed glyph commands to the frame,
    /// in emission order. A paragraph that flows across N frames
    /// contributes one entry to each of those frames'
    /// `paragraph_cmd_ranges` lists. Drives `JustifyAlign` vertical
    /// justification, which distributes the per-frame slack as
    /// extra inter-paragraph space.
    pub(super) paragraph_cmd_ranges: Vec<Vec<(usize, usize)>>,
    /// Counter for `NumberedList` paragraphs in this story. The
    /// renderer treats the count as a sticky story-level value
    /// across paragraphs of different kinds; the implicit-reset
    /// fires only when entering a `NumberedList` paragraph whose
    /// prior neighbour wasn't also numbered (and the paragraph
    /// hasn't explicitly opted into `NumberingContinue`). 0 is the
    /// initial value; the first numbered paragraph either lifts it
    /// to its `NumberingStartAt` or to 1.
    pub(super) numbered_counter: u32,
    /// Tracks whether the previous paragraph was a `NumberedList`.
    /// Drives the implicit-reset decision for the next paragraph:
    /// a `NumberedList` paragraph that follows a non-numbered one
    /// resets the counter to 0 (so the first increment lands at 1)
    /// unless the paragraph carries `NumberingContinue="true"` or
    /// `NumberingStartAt`.
    pub(super) prev_was_numbered: bool,
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
    pub(super) cross_story_numbering: Option<&'a std::cell::RefCell<HashMap<String, u32>>>,
    /// `<StoryPreference OpticalMarginAlignment>` flag. When true,
    /// the per-line emit pass nudges the leftmost / rightmost glyph
    /// of each line outward per `paged_text::optical_margin_offset`.
    pub(super) optical_margin_alignment: bool,
    /// `<StoryPreference OpticalMarginSize>` (point size). Bounds the
    /// hang for glyphs smaller than this size; ignored when
    /// `optical_margin_alignment` is false.
    pub(super) optical_margin_size_pt: f32,
    /// How many anchored-frame story recursions deep this emitter is.
    /// 0 for the top-level body / master pass; 1+ for an emitter
    /// constructed by `emit_anchored_textframe_story`. Bounded at
    /// `MAX_ANCHORED_STORY_RECURSION` so a malformed document with an
    /// anchored TextFrame referencing its own host story can't blow
    /// the stack.
    pub(super) anchored_recursion_depth: u32,
    /// Image-bearing anchored frames captured during emission so the
    /// caller can replay them through `emit_rectangle_image` once the
    /// story pass completes. Image emission needs the per-page
    /// `ImageId` cache + decoded-image cache that live in
    /// `build_document`'s scope, outside StoryEmitter — collecting the
    /// already-resolved (target_page, place_x, place_y, AnchoredFrame
    /// clone) tuples here lets the post-pass run with the caches in
    /// hand without re-doing placement.
    pub(super) anchored_image_queue: Vec<AnchoredImageEmit>,
    /// Track 2: per-line records collected when
    /// `options.collect_breaks` is set. Drained by `take_breaks` once
    /// the story finishes emitting.
    pub(super) breaks: Vec<BreakRecord>,
    /// Track 2: identifies which story this emitter is processing.
    /// Set by `StoryEmitter::with_story_id` before emit; included in
    /// every pushed `BreakRecord`. Empty string when collection isn't
    /// enabled.
    pub(super) current_story_id: String,
    /// Track 2: monotonically incremented as `emit_paragraph` fires.
    /// Resets to 0 per emitter (i.e. per story).
    pub(super) paragraph_idx: u32,
    /// Lossy-render signals collected during this story's emit (overset
    /// drop). Drained by `take_diagnostics` into the document-level
    /// collector, mirroring `breaks`. A non-empty drain marks the emit
    /// uncacheable so a body-story cache hit can't silently swallow it.
    pub(super) diagnostics: Vec<Diagnostic>,
    /// Set once a story-overflow drop has been reported so the overset
    /// diagnostic fires once per story, not once per dropped line.
    pub(super) overset_reported: bool,
    /// W1.4 — total body-page count, for `PageCountType` text-variable
    /// resolution. Set once per build (the same value for every
    /// emitter); 0 only before pages exist.
    pub(super) page_count: usize,
    /// W1.4 — when true, the LineLayout capture also pushes
    /// [`paged_compose::LinkRegion`]s for runs tagged with
    /// `hyperlink_source`. Mirrors `options.collect_link_regions`;
    /// cached so the per-line path doesn't re-read options.
    pub(super) collect_link_regions: bool,
    /// W1.4 — `<Page Self=...>` id → flat 0-based body-page index, for
    /// resolving hyperlink page destinations. `None` when link-region
    /// collection is off (the map isn't built). Owned by the build, not
    /// the emitter.
    pub(super) page_index_map: Option<&'a HashMap<String, u32>>,
    /// W1.18b — chapter number per flat body-page index, pre-computed
    /// once per build from `<Section>` settings. Empty (every
    /// `ChapterNumberType` falls back to baked text) when the document
    /// declares no sections. Owned by the build.
    pub(super) chapter_numbers: &'a [String],
    /// W1.18c — post-layout running-header pickup index. `None` on the
    /// first (pre-layout) pass — running headers then keep their baked
    /// value; populated for the re-emit so they resolve to the matching
    /// on-page paragraph. Owned by the build.
    pub(super) running_headers: Option<&'a links::RunningHeaderIndex>,
}

impl<'a> StoryEmitter<'a> {
    pub(super) fn new(
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
        let chain_spread_bounds: Vec<paged_model::Bounds> = chain
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
            chapter_numbers: &[],
            running_headers: None,
        }
    }

    /// W1.4 — wire the `<Page Self=...>` → flat-index map used to
    /// resolve hyperlink page destinations. Called only when
    /// link-region collection is on.
    pub(super) fn with_page_index_map(mut self, map: &'a HashMap<String, u32>) -> Self {
        self.page_index_map = Some(map);
        self
    }

    /// W1.18b — wire the per-page chapter-number table used by
    /// `ChapterNumberType` variables. Empty slice ⇒ no sections.
    pub(super) fn with_chapter_numbers(mut self, numbers: &'a [String]) -> Self {
        self.chapter_numbers = numbers;
        self
    }

    /// W1.18c — wire the post-layout running-header pickup index used by
    /// `RunningHeaderType` variables. Set only on the re-emit pass.
    pub(super) fn with_running_headers(mut self, index: &'a links::RunningHeaderIndex) -> Self {
        self.running_headers = Some(index);
        self
    }

    /// W1.4 — resolve a hyperlink destination target id (a `<Page>`
    /// self id, or a story / text-anchor id) to a flat body-page index.
    /// Only page ids resolve in v1; story / text-anchor ids fall through
    /// to `None` (the caller records them as `Unresolved`).
    pub(super) fn page_index_for_target(&self, target_id: &str) -> Option<u32> {
        self.page_index_map.and_then(|m| m.get(target_id).copied())
    }

    pub(super) fn with_story_id(mut self, story_id: &str) -> Self {
        self.current_story_id = story_id.to_string();
        self
    }

    /// W1.22 — wire the document-level numbering ledger so
    /// `list_prefix` can carry a `ContinueNumbersAcrossStories` list's
    /// counter across story boundaries. Only set by `build_document`'s
    /// body-story pass when the document declares such a list.
    pub(super) fn with_cross_story_numbering(
        mut self,
        ledger: &'a std::cell::RefCell<HashMap<String, u32>>,
    ) -> Self {
        self.cross_story_numbering = Some(ledger);
        self
    }

    /// W1.4 — record the document's total body-page count for
    /// `PageCountType` text-variable resolution. Called by the body /
    /// master pass once `pages` is sized.
    pub(super) fn with_page_count(mut self, count: usize) -> Self {
        self.page_count = count;
        self
    }

    pub(super) fn take_breaks(&mut self) -> Vec<BreakRecord> {
        std::mem::take(&mut self.breaks)
    }

    pub(super) fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Cycle 6 Track 1: gate per-line break collection on the
    /// optional story / page filters. Returns true when the current
    /// emitter context is selected by both filters (each `None`
    /// filter passes anything).
    pub(super) fn break_filter_passes(&self, target_page: u32) -> bool {
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
    pub(super) fn with_anchored_recursion_depth(mut self, depth: u32) -> Self {
        self.anchored_recursion_depth = depth;
        self
    }

    /// Hand off any image-bearing anchored frames captured during the
    /// story pass. The body / master pass calls this after
    /// `apply_blend_groups` so the post-pass below can reuse the
    /// already-resolved per-page + decoded caches without
    /// re-traversing the story tree.
    pub(super) fn take_anchored_image_queue(&mut self) -> Vec<AnchoredImageEmit> {
        std::mem::take(&mut self.anchored_image_queue)
    }

    /// Set the story's `<StoryPreference>` optical-margin flags so
    /// the per-paragraph emit pass can nudge the leftmost / rightmost
    /// glyph of every line. `size_pt = 0.0` disables the feature even
    /// if the flag is true (matches `apply_optical_margin`'s noop).
    pub(super) fn with_optical_margin(mut self, alignment: bool, size_pt: f32) -> Self {
        self.optical_margin_alignment = alignment;
        self.optical_margin_size_pt = size_pt;
        self
    }

    /// W1.7 — seed the per-frame footnote space reservation (1/64 pt)
    /// before a re-emit. `reserved[i]` is held back from chain frame
    /// `i`'s text bottom so the footnote pool drawn underneath does
    /// not overlap the body. A shorter/empty slice is padded with
    /// zeros; entries past the chain length are ignored.
    pub(super) fn with_footnote_reservation(mut self, reserved: &[i32]) -> Self {
        for (slot, &r) in self.reserved_footnote_64.iter_mut().zip(reserved.iter()) {
            *slot = r.max(0);
        }
        self
    }

    pub(super) fn emit_paragraph(
        &mut self,
        paragraph: &paged_model::Paragraph,
        pages: &mut [BuiltPage],
        total_stats: &mut PipelineStats,
    ) {
        emit_paragraph_into_chain(self, paragraph, pages, total_stats);
        self.paragraph_idx = self.paragraph_idx.saturating_add(1);
    }

    pub(super) fn apply_vertical_justification(&self, pages: &mut [BuiltPage]) {
        for (i, frame) in self.chain.iter().enumerate() {
            let Some((start, end)) = self.frame_cmd_ranges[i] else {
                continue;
            };
            let Some(vj) = frame.vertical_justification else {
                continue;
            };
            if vj == paged_model::VerticalJustification::Top {
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
            if vj == paged_model::VerticalJustification::Justify {
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
                paged_model::VerticalJustification::Center => slack_64 / 2,
                paged_model::VerticalJustification::Bottom => slack_64,
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
    pub(super) fn apply_polygon_clip(&mut self, pages: &mut [BuiltPage]) {
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
    pub(super) fn apply_blend_groups(&self, pages: &mut [BuiltPage]) {
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
) -> Vec<paged_model::Paragraph> {
    let entries = document.resolve_index();
    let mut out: Vec<paged_model::Paragraph> = Vec::with_capacity(entries.len());
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
        let run = paged_model::CharacterRun {
            text,
            ..paged_model::CharacterRun::default()
        };
        out.push(paged_model::Paragraph {
            runs: vec![run],
            ..paged_model::Paragraph::default()
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
pub(super) fn build_toc_paragraphs(
    document: &Document,
    toc_style: &paged_model::TOCStyleDef,
    page_labels: &[String],
) -> Vec<paged_model::Paragraph> {
    let entries = document.resolve_toc(toc_style);
    let mut out: Vec<paged_model::Paragraph> = Vec::with_capacity(entries.len());
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
        let run = paged_model::CharacterRun {
            text,
            ..paged_model::CharacterRun::default()
        };
        let paragraph = paged_model::Paragraph {
            paragraph_style: entry.format_style,
            runs: vec![run],
            ..paged_model::Paragraph::default()
        };
        out.push(paragraph);
    }
    out
}

/// Body of `StoryEmitter::emit_paragraph`. Lives as a free fn so
/// the long, branching layout/emit pipeline isn't visually
/// indented under `impl`. The free fn has full mutable access to
/// the emitter state via `&mut StoryEmitter`.
pub(super) fn emit_paragraph_into_chain(
    em: &mut StoryEmitter,
    paragraph: &paged_model::Paragraph,
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
    let paragraph: &paged_model::Paragraph = {
        let has_conditions = paragraph
            .runs
            .iter()
            .any(|r| !r.applied_conditions.is_empty());
        if !has_conditions {
            paragraph
        } else {
            let conditions = &em.document.styles.conditions;
            let filtered: Vec<paged_model::CharacterRun> = paragraph
                .runs
                .iter()
                .filter(|r| {
                    r.applied_conditions
                        .iter()
                        .all(|cid| conditions.get(cid).and_then(|c| c.visible).unwrap_or(true))
                })
                .cloned()
                .collect();
            paragraph_filtered_owned = paged_model::Paragraph {
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
    let paragraph: &paged_model::Paragraph = {
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
                paragraph_owned = paged_model::Paragraph {
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
        r.text.contains(paged_model::AUTO_PAGE_NUMBER_MARKER)
            || r.text.contains(paged_model::NEXT_PAGE_NUMBER_MARKER)
    }) || list_first_text
        .as_deref()
        .is_some_and(|t| t.contains(paged_model::AUTO_PAGE_NUMBER_MARKER));
    let page_substituted: Vec<String> = if needs_page_subst {
        paragraph
            .runs
            .iter()
            .map(|r| {
                r.text
                    .replace(paged_model::AUTO_PAGE_NUMBER_MARKER, &current_page_str)
                    .replace(paged_model::NEXT_PAGE_NUMBER_MARKER, &next_page_str)
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
        // W1.18 — the frame currently filling resolves variables FOR its
        // host page: date variables read the deterministic clock, the
        // chapter number is the section owning this page, and (on the
        // re-emit) running headers pick up the matching paragraph on this
        // very page.
        let host_page = em.chain_pages.get(em.frame_idx).copied().unwrap_or(0);
        let ctx = links::VarResolveCtx {
            designmap: &em.document.designmap,
            total_pages,
            clock: &em.options.document_clock,
            chapter_number: em.chapter_numbers.get(host_page).map(String::as_str),
            page_index: host_page,
            running_headers: em.running_headers,
        };
        paragraph
            .runs
            .iter()
            .map(|r| {
                r.text_variable
                    .as_deref()
                    .and_then(|var_id| links::resolve_variable(&ctx, var_id, &r.text))
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
                let target =
                    links::resolve_link_target(&em.document.designmap, source_id, |page_id| {
                        em.page_index_for_target(page_id)
                    });
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
    let resolve_rule_paint = |r: &paged_model::ParagraphRule| -> Option<Paint> {
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
        if paged_flow::region_overflows(line.baseline_y, text_bottom_64)
            && em.frame_idx + 1 < em.chain.len()
        {
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
        if paged_flow::region_overflows(line.baseline_y, text_bottom_64)
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
                // First-class overset continuation (paged_flow::Overset::
                // Remains): record WHERE the flow overran — this line is the
                // first that didn't fit, so `current_line_idx` (not yet
                // incremented; the increment is at the end of the loop, past
                // this `continue`) is the overset line within the paragraph.
                let mut d = Diagnostic::new(
                    DiagnosticCode::OversetTextDropped,
                    "text overflows the last frame in its chain; trailing lines clipped (overset)",
                )
                .with_page(page)
                .with_overset(em.paragraph_idx, current_line_idx as u32);
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
