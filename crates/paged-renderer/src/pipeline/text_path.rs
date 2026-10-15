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

//! Path geometry for polygons and text-on-a-path: Bezier anchor →
//! [`PathData`] conversion (compound-path aware), arc-length
//! tessellation, and the text-on-path glyph emitter.

use super::*;

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
///
/// Only the pipeline tests call this no-`subpath_open` convenience
/// wrapper; production code goes through `..._with_open` directly.
#[cfg(test)]
pub(super) fn polygon_path_from_anchors(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
) -> PathData {
    polygon_path_from_anchors_with_open(anchors, subpath_starts, &[])
}

/// Same as `polygon_path_from_anchors` but consults a parallel
/// `subpath_open` slice. An open contour skips the closing CubicTo +
/// Close so a hand-drawn lassoed stroke or a `PathOpen="true"` clip
/// path doesn't get auto-filled (P-15). `subpath_open` is interpreted
/// against the indexed order of contours (the `i`th true ⇒ `i`th
/// contour open); a shorter slice / empty slice means every contour
/// is closed (legacy behaviour).
/// Materialise a path's contour ranges from its `subpath_starts`.
/// Default (`[]` or `[0]`) = one contour covering the whole anchor
/// list. Otherwise each entry begins a new contour at that index,
/// ending where the next one starts (or at `len` for the last entry).
/// Out-of-range and duplicate offsets are filtered defensively — every
/// contour gets at least one anchor or is dropped.
///
/// Shared by the plain path builder and B-23's corner-effected polygon
/// builder so the two can never disagree about where a contour begins.
pub(crate) fn contour_ranges(len: usize, subpath_starts: &[usize]) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    if subpath_starts.len() <= 1 {
        ranges.push((0, len));
        return ranges;
    }
    let mut starts: Vec<usize> = subpath_starts
        .iter()
        .copied()
        .filter(|&s| s < len)
        .collect();
    starts.sort_unstable();
    starts.dedup();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    for i in 0..starts.len() {
        let lo = starts[i];
        let hi = starts.get(i + 1).copied().unwrap_or(len);
        if hi > lo {
            ranges.push((lo, hi));
        }
    }
    ranges
}

pub(crate) fn polygon_path_from_anchors_with_open(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
) -> PathData {
    if anchors.is_empty() {
        return PathData {
            segments: Vec::new(),
        };
    }
    let ranges = contour_ranges(anchors.len(), subpath_starts);
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

/// B-23 — the outline a polygon actually paints: its anchor path with
/// the uniform corner effect applied when one is resolved, otherwise the
/// plain anchor path. One helper so the fill, the stroke-alignment base
/// and the B-18 container clip can never disagree about the outline.
pub(crate) fn polygon_outline_path(
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
    corner: Option<(paged_model::CornerOption, f32)>,
) -> PathData {
    if let Some((kind, radius)) = corner {
        if let Some(p) =
            super::shapes::corner_polygon_path(anchors, subpath_starts, subpath_open, kind, radius)
        {
            return p;
        }
    }
    polygon_path_from_anchors_with_open(anchors, subpath_starts, subpath_open)
}

/// Polygon emit. When the polygon carries `<PathPointType>` anchors
/// (real-world InDesign export shape) we build a curved FillPath
/// from them; otherwise fall back to drawing the AABB so synthetic
/// IDMLs that declare a polygon via `GeometricBounds` still render.
pub(super) fn emit_polygon_into(
    page: &mut BuiltPage,
    poly: &Polygon,
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    color_ctx: ColorCtx<'_>,
    content: super::shapes::ContentPainter<'_>,
) {
    let mut resolved = ResolvedFrame::from_polygon(poly);
    let style = crate::module::resolve_applied_style(&resolved, document);
    if let Some(s) = &style {
        crate::module::object_style_cascade(&mut resolved, s);
    }
    page.stats.frames += 1;
    let outer = frame_outer_transform(page, resolved.item_transform);
    let needs_group = frame_needs_blend_group(&resolved);
    let bbox = match &resolved.geometry {
        Geometry::Polygon { bbox, .. } => *bbox,
        Geometry::Rect { rect } => *rect,
        _ => paged_compose::Rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        },
    };
    if needs_group {
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
    // B-23: the polygon's corner effect (if any) is resolved once and
    // reused by the fill path, the stroke-alignment base path, and the
    // effects path — they must all describe the same outline.
    let corner = super::shapes::uniform_corner(&resolved.corners);
    let path_id = if let Geometry::Polygon {
        anchors,
        subpath_starts,
        subpath_open,
        ..
    } = &resolved.geometry
    {
        let path = polygon_outline_path(anchors, subpath_starts, subpath_open, corner);
        let cache_key = match resolved.self_id {
            Some(id) => fnv_1a_u64(id.as_bytes()),
            None => path_signature(anchors),
        };
        let (id, _) = page.list.paths.intern(cache_key, path);
        Some(id)
    } else {
        None
    };
    // The shadow stamps under the polygon's real outline (the module
    // interns it under the same key as `path_id`), behind the fill.
    crate::module::drop_shadow_module(&resolved, page, palette, color_ctx, None, outer);
    // Q-04: Polygon frame effects (GradientFeather, OuterGlow, etc.)
    // ride the interned polygon path. The path is already in inner-
    // coord space and `outer` carries the frame's ItemTransform plus
    // the page-origin shift, so `effects_unit_normalize = None` (the
    // effects module reads coordinates from the path directly).
    if let (Some(pid), Some(effects)) = (path_id, poly.effects.as_ref()) {
        crate::module::emit_effects_pre_fill(page, effects, pid, outer, palette, color_ctx);
    }
    let fill_group = push_target_group(page, resolved.fill_target, bbox, outer);
    crate::module::fill_shadow_module(&resolved, page, palette, color_ctx, outer);
    crate::module::fill_paint_module(
        &resolved, page, palette, color_ctx, fallback, outer, path_id,
    );
    if fill_group {
        pop_blend_group(page);
    }
    if let (Some(pid), Some(effects)) = (path_id, poly.effects.as_ref()) {
        crate::module::emit_effects_post_fill(
            page, effects, pid, outer, palette, color_ctx, None, 0.0,
        );
    }
    // W1.5 — Inside/Outside StrokeAlignment on the polygon outline:
    // offset each closed contour inward / outward by weight/2 and
    // stroke that offset path instead of the centred `path_id`. Centre
    // alignment (or an open / anchorless polygon) keeps `path_id`.
    let poly_weight = resolved.effective_stroke_weight();
    let aligned_poly_id = if let Geometry::Polygon {
        anchors,
        subpath_starts,
        subpath_open,
        ..
    } = &resolved.geometry
    {
        let base = polygon_outline_path(anchors, subpath_starts, subpath_open, corner);
        super::shapes::aligned_outline_path(&base, resolved.stroke_alignment, poly_weight).map(
            |p| {
                let seed = resolved
                    .self_id
                    .map(|id| fnv_1a_u64(id.as_bytes()))
                    .unwrap_or(0xC0DE)
                    ^ 0xA11A_0000;
                page.list.paths.intern(seed, p).0
            },
        )
    } else {
        None
    };
    // The picture, between the fill and the stroke, in its own target.
    let content_group = push_target_group(page, resolved.content_target, bbox, outer);
    content(page);
    if content_group {
        pop_blend_group(page);
    }
    let stroke_group = push_target_group(page, resolved.stroke_target, bbox, outer);
    crate::module::stroke_paint_module(
        &resolved,
        page,
        palette,
        color_ctx,
        outer,
        aligned_poly_id.or(path_id),
        stroke_for(
            resolved.stroke_type,
            poly_weight,
            resolved.end_cap,
            resolved.end_join,
            resolved.miter_limit,
            Some(&document.styles.stroke_styles),
            resolved.stroke_dash,
        ),
    );
    // C-62 — line ends on a pen / pencil path: every OPEN contour takes
    // `LeftLineEnd` at its first anchor and `RightLineEnd` at its last,
    // the way a `<GraphicLine>` does, painted with the stroke's paint. A
    // closed contour has no end, so it draws none. Inside the blend
    // group, so the arrowheads fade with the stroke they finish.
    let ends = super::shapes::LineEnds::of_polygon(poly);
    if let Geometry::Polygon {
        anchors,
        subpath_starts,
        subpath_open,
        bbox,
    } = &resolved.geometry
    {
        let open_runs: Vec<(usize, usize)> = contour_ranges(anchors.len(), subpath_starts)
            .into_iter()
            .enumerate()
            .filter(|(i, _)| subpath_open.get(*i).copied().unwrap_or(false))
            .map(|(_, range)| range)
            .collect();
        let paint = if ends.draws() && poly_weight > 0.0 && !open_runs.is_empty() {
            resolved.stroke_color.and_then(|id| {
                color_id_to_paint_with_list_dir(
                    id,
                    palette,
                    color_ctx,
                    &mut page.list,
                    resolved.gradient_stroke_angle,
                    resolved.gradient_stroke_length,
                    Some((bbox.w, bbox.h)),
                )
            })
        } else {
            None
        };
        if let Some(paint) = paint {
            for (lo, hi) in open_runs {
                super::shapes::emit_path_line_ends(
                    page,
                    &anchors[lo..hi],
                    ends,
                    poly_weight,
                    paint,
                    outer,
                );
            }
        }
    }
    if stroke_group {
        pop_blend_group(page);
    }
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

/// The glyph baseline offset, in *font units*, that seats a glyph on the
/// path for a `TextAlignment` value (InDesign's attribute) or the
/// `PathTypeAlignment` spelling older Paged builds wrote. The renderer
/// flips font space (y-up) to page space (y-down) via the `-scale` term,
/// so a *positive* return here moves the glyph below the path.
///
/// InDesign 20.0.1 measures from the font's bounding box (`head` yMax /
/// yMin), not its ascent and descent: type on a straight line, Open Sans
/// 24 pt, put the baseline 25.125 pt below the line for Ascender (yMax
/// 25.148) and 6.562 pt above it for Descender (yMin −6.527), where the
/// ascent and descent would give 25.652 and 7.031.
///
///   - `BaselineTextAlignment` (the default) — baseline on the path ⇒ 0.
///   - `AscenderTextAlignment` — the box's top on the path ⇒ `y_max`.
///   - `DescenderTextAlignment` — the box's bottom on the path ⇒ `y_min`
///     (negative, so the baseline rises above the path).
///   - `CenterTextAlignment` — the box's centre on the path.
fn text_alignment_baseline_offset_units(alignment: Option<&str>, y_max: i16, y_min: i16) -> f32 {
    let top = y_max as f32;
    let bottom = y_min as f32;
    match alignment {
        Some("CenterTextAlignment" | "CenterPathType") => (top + bottom) * 0.5,
        Some("AscenderTextAlignment" | "AscenderPathType") => top,
        Some("DescenderTextAlignment" | "DescenderPathType") => bottom,
        // Baseline (the default) and any unknown value seat the baseline
        // on the path.
        _ => 0.0,
    }
}

/// How far `PathAlignment` moves the text off the path's centre line, in
/// pt, positive downward: InDesign 20.0.1 sets text on the top edge of the
/// stroke for `TopPathAlignment` and on its bottom edge for
/// `BottomPathAlignment`, half the stroke weight either way (a 6 pt stroke
/// put the baseline 3 pt above and 3 pt below the line).
fn path_alignment_shift_pt(alignment: Option<&str>, stroke_weight: f32) -> f32 {
    let half = stroke_weight.max(0.0) / 2.0;
    match alignment {
        Some("TopPathAlignment") => -half,
        Some("BottomPathAlignment") => half,
        _ => 0.0,
    }
}

/// Emit the glyphs for a `<TextPath>` along the host shape's
/// tessellated curve. Approximates IDML's text-on-path:
///
///   - Concatenates every paragraph's runs into a single styled
///     string and shapes them with harfrust, exactly like the body
///     text path. Per-paragraph styles (alignment, leading, tabs)
///     are intentionally ignored — text-on-path is a single
///     baseline, not a multi-line column.
///   - Walks the shape's polyline by cumulative arc length: for
///     each glyph the cursor advances by the glyph's `x_advance` and
///     we look up `(x, y, angle)` at the cursor's midpoint. The
///     glyph is then emitted with a per-glyph rotated transform
///     (Rainbow path effect — the IDML default).
///   - Honours `TextAlignment` (the glyph's vertical seat on the path):
///     Baseline / Center / Ascender / Descender via
///     `text_alignment_baseline_offset_units`, and `PathAlignment` (which
///     edge of the host's stroke the text rides) via
///     `path_alignment_shift_pt`.
///   - Places the text along the path as its first paragraph's
///     justification says: left from `StartBracket` (InDesign's default),
///     centred, or right against `EndBracket`.
///   - Honours the `flip_path_effect` attribute: `Flipped` reverses
///     the path direction so text reads from end-to-start.
///   - Honours `StartBracket` / `EndBracket`: text flows only within
///     that arc-length window; glyphs whose advance carries them past
///     `EndBracket` are dropped and an `OversetTextDropped` diagnostic
///     fires once (matching the body-text overset contract).
///
/// DEFERRED (parsed, not yet rendered): path-effect modes beyond
/// Rainbow — `SkewPathEffect` / `Path3DRibbonEffect` /
/// `StairStepPathEffect` / `GravityPathEffect` all render as Rainbow
/// today. Skew/3D-Ribbon look the same as Rainbow on a gentle arch;
/// Stair-Step and Gravity need a per-glyph projection that lands
/// later. Justified text on a path (spread along it) sets as left for
/// now, and side/flip options beyond `Flipped` are also future work.
pub(super) fn emit_text_path_into(
    page: &mut BuiltPage,
    page_idx: usize,
    breaks: &mut Vec<BreakRecord>,
    text_path: &TextPath,
    anchors: &[PathAnchor],
    item_transform: Option<[f32; 6]>,
    stroke_weight: f32,
    document: &Document,
    options: &PipelineOptions,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
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
        /// The glyph's cluster: its byte in `text`.
        byte: usize,
        glyph_id: u32,
        x_advance_64: i32,
        y_offset_64: i32,
        x_offset_64: i32,
        face_idx: usize,
        point_size: f32,
        paint: Paint,
    }
    let mut glyphs: Vec<PathGlyph> = Vec::new();
    // The text the glyphs were shaped from, run after run.
    let mut text = String::new();
    // Faces are indexed; outline + font_id parallel arrays.
    let mut face_bytes: Vec<Bytes> = Vec::new();
    let mut face_font_ids: Vec<u32> = Vec::new();

    let find_or_push_face =
        |bytes: &Bytes, face_bytes: &mut Vec<Bytes>, face_font_ids: &mut Vec<u32>| -> usize {
            if let Some(i) = face_bytes.iter().position(|b| b.as_ptr() == bytes.as_ptr()) {
                return i;
            }
            face_bytes.push(bytes.clone());
            face_font_ids.push(font_id(bytes));
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
            // Text-on-path doesn't render the substituted highlight
            // (it has no line pass), so the A1 flag is dropped here.
            let face_bytes_b = font_table
                .bytes_for(resolved.font.as_deref(), resolved.font_style.as_deref())
                .map(|(bytes, _substituted)| bytes)
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
            let point_size = resolved.point_size.unwrap_or(options.default_point_size);
            let paint = resolved
                .fill_color
                .as_deref()
                .and_then(|id| color_id_to_paint(id, palette, color_ctx))
                .map(|p| {
                    apply_fill_tint_ctx(
                        p,
                        object_tint(palette, resolved.fill_color.as_deref(), resolved.fill_tint),
                        &color_ctx,
                    )
                })
                .unwrap_or(default_paint);
            // Pull the pre-configured (wght-baked) Face from the
            // FontTable cache when possible; build on the fly only
            // on a miss (e.g. a run whose bytes resolved through the
            // fallback path that `harvest_face_keys` didn't see).
            let font_id = font_id(&face_bytes_b);
            let wght_bits = wght_for_font_style(resolved.font_style.as_deref()).to_bits();
            let owned_face: Option<paged_text::Face> =
                if font_table.face(font_id, wght_bits).is_none() {
                    let Some(mut rf) = paged_text::Face::from_slice(face_bytes_b.as_ref(), 0)
                    else {
                        continue;
                    };
                    let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
                    let has_wght_axis = rf
                        .variation_axes()
                        .into_iter()
                        .any(|axis| axis.tag == wght_tag);
                    if has_wght_axis {
                        rf.set_variations(&[paged_text::Variation {
                            tag: wght_tag,
                            value: f32::from_bits(wght_bits),
                        }]);
                    }
                    Some(rf)
                } else {
                    None
                };
            let rb_face: &paged_text::Face = match font_table.face(font_id, wght_bits) {
                Some(f) => f,
                None => owned_face.as_ref().unwrap(),
            };
            let mut shaped = paged_text::shape::shape_run(rb_face, &run.text, point_size);
            if let Some(t) = resolved.tracking {
                paged_text::shape::apply_tracking(&mut shaped, t, point_size);
            }
            let run_start = text.len();
            text.push_str(&run.text);
            for g in &shaped.glyphs {
                glyphs.push(PathGlyph {
                    byte: run_start + g.cluster as usize,
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
        .map(|g| g.x_advance_64 as f32 / paged_text::shape::ADVANCE_PRECISION)
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

    // Along the path the text sits as its first paragraph's justification
    // says, like a one-line paragraph: InDesign 20.0.1 starts left-aligned
    // text (its default) at `StartBracket`. Overflowing text (advance >
    // usable_len) starts at `start_b` and runs off `end_b`; those
    // trailing glyphs are dropped and reported as overset below.
    let alignment = parsed_story
        .story
        .paragraphs
        .first()
        .map(|p| {
            map_justification(
                document.resolved_paragraph_attrs(p).justification,
                page.binding_edge(),
            )
            .1
        })
        .unwrap_or(paged_text::Alignment::Left);
    let slack = usable_len - total_advance_pt;
    let start_offset_pt = if slack > 0.0 {
        match alignment {
            paged_text::Alignment::Center => start_b + slack * 0.5,
            paged_text::Alignment::Right => start_b + slack,
            // Left, and justified text (not spread along a path yet).
            _ => start_b,
        }
    } else {
        start_b
    };

    // Outer transform: page origin · ItemTransform. Same composition
    // as every other shape — keeps text-on-path inside the host
    // shape's coordinate system without re-implementing the math.
    let outer = frame_outer_transform(page, item_transform);

    // `TextAlignment` seats the glyph vertically on the path, and
    // `PathAlignment` picks the stroke edge it rides; each is a single
    // value for the whole TextPath. `Flipped` already mirrors the path
    // direction; we keep the seat as authored.
    let text_alignment = text_path.path_type_alignment.as_deref();
    let path_shift_pt = path_alignment_shift_pt(text_path.path_alignment.as_deref(), stroke_weight);

    let mut cursor_pt = start_offset_pt;
    // Count glyphs whose advance carries their baseline-origin past
    // `end_b` (the overset tail). Reported once after the loop, like
    // the body-text overset contract.
    let mut overset_dropped = 0usize;
    // The text that fits ends where the first overset glyph's does.
    let mut set_end = text.len();
    // The path's line for the break records: where on the page it starts
    // and the arc length its glyphs take.
    let mut line_start_y: Option<f32> = None;
    let mut set_width_pt = 0.0f32;
    for g in &glyphs {
        let advance_pt = g.x_advance_64 as f32 / paged_text::shape::ADVANCE_PRECISION;
        let x_off_pt = g.x_offset_64 as f32 / paged_text::shape::ADVANCE_PRECISION;
        let y_off_pt = g.y_offset_64 as f32 / paged_text::shape::ADVANCE_PRECISION;
        // Place the glyph's baseline-left at the cursor's current
        // arc length (plus its shaping x_offset). The local tangent
        // at that point gives the glyph's rotation. Each glyph
        // advances the cursor by its own advance.
        let s = cursor_pt + x_off_pt;
        cursor_pt += advance_pt;
        // Overset: once a glyph's origin lands past `end_b` it (and
        // every glyph after it, since the cursor only grows) is beyond
        // the path and dropped. `sample_path_at` would clamp it to the
        // end and pile glyphs up; instead we skip + count so the
        // diagnostic matches InDesign's overset behaviour.
        if s > end_b + 1e-3 {
            if overset_dropped == 0 {
                set_end = g.byte.min(set_end);
            }
            overset_dropped += 1;
            continue;
        }
        set_width_pt += advance_pt;
        let Some((px, py, angle)) = sample_path_at(&samples, s) else {
            continue;
        };
        let Some(outline) = outline_faces[g.face_idx].as_ref() else {
            continue;
        };
        let outliner = TtfOutliner::new(outline);
        let upem = outliner.units_per_em();
        let scale = g.point_size / upem;
        // Vertical seat offset (font units → pt). 0 for Baseline.
        let bbox = outline.global_bounding_box();
        let seat_units =
            text_alignment_baseline_offset_units(text_alignment, bbox.y_max, bbox.y_min);
        let seat_pt = seat_units * scale + path_shift_pt;
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
        // local_tx/ty operate in page-oriented pt space (S applies
        // first). x_offset is already baked into `s`; the per-glyph
        // shaping y_offset plus the path-type vertical seat apply here.
        // `seat_pt > 0` (Center/Ascender) shifts the glyph down in page
        // space so its centre/top — not its baseline — rides the path.
        let local_tx = 0.0;
        let local_ty = y_off_pt + seat_pt;
        let rtl_tx = r[0] * local_tx + r[2] * local_ty;
        let rtl_ty = r[1] * local_tx + r[3] * local_ty;
        // (R · T_local) · S(scale, -scale): scales the columns.
        let rs_a = r[0] * s_diag[0] + r[2] * s_diag[1];
        let rs_b = r[1] * s_diag[0] + r[3] * s_diag[1];
        let rs_c = r[0] * s_diag[2] + r[2] * s_diag[3];
        let rs_d = r[1] * s_diag[2] + r[3] * s_diag[3];
        let inner = Transform([rs_a, rs_b, rs_c, rs_d, rtl_tx + px, rtl_ty + py]);
        let final_xf = outer.compose(&inner);
        line_start_y.get_or_insert(final_xf.0[5]);
        // Concept 3 — glyph-run side-channel (text-on-path glyphs
        // carry a rotated affine; the exporter reuses it verbatim).
        if let Some(runs) = page.list.glyph_runs.as_mut() {
            runs.push(paged_compose::GlyphRunEntry {
                command_index: page.list.commands.len() as u32,
                font_id: face_font_ids[g.face_idx],
                // text-on-path never mixes a wght into its ids, so the
                // outline id IS the byte-resolvable one.
                face_id: Some(face_font_ids[g.face_idx]),
                glyph_id: g.glyph_id,
                font_size: g.point_size,
                transform: final_xf,
                paint: g.paint,
                unicode: None,
                is_stroke: false,
            });
        }
        page.list.push(paged_compose::DisplayCommand::FillPath {
            path_id,
            paint: g.paint,
            transform: final_xf,
        });
        page.stats.glyphs += 1;
    }

    // The path's one line joins the break records, as the text that fits
    // the bracket window.
    let wanted = options.collect_breaks
        && options
            .break_page_range
            .as_ref()
            .is_none_or(|r| r.contains(&(page_idx as u32)))
        && options
            .break_story_filter
            .as_deref()
            .is_none_or(|want| want == text_path.parent_story);
    if wanted && set_end > 0 {
        breaks.push(BreakRecord {
            story_id: text_path.parent_story.clone(),
            paragraph_idx: 0,
            line_idx: 0,
            page_idx: page_idx as u32,
            frame_idx: 0,
            first_byte: 0,
            last_byte: set_end as u32,
            baseline_y_pt: line_start_y.unwrap_or(0.0),
            width_pt: set_width_pt,
            source_text: text.get(..set_end).unwrap_or("").to_string(),
            generated: Vec::new(),
            text_path: Some(text_path.self_id.clone().unwrap_or_default()),
        });
    }

    // Overset on a path: text that didn't fit the bracket window was
    // dropped. Report it once with the host story id; `page_index` is
    // backfilled by the build aggregator. Same severity / contract as
    // the body-text `OversetTextDropped` so the editor Preflight and
    // paged-inspect surface text-on-path overflow the same way.
    if overset_dropped > 0 {
        page.diagnostics.push(
            crate::diagnostics::Diagnostic::new(
                crate::diagnostics::DiagnosticCode::OversetTextDropped,
                format!(
                    "text-on-path overset: {overset_dropped} glyph(s) past the path length were dropped"
                ),
            )
            .with_story(text_path.parent_story.clone()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Open Sans's bounding box in its 2048-upem face (`head` yMax / yMin),
    // the metric InDesign seats type on a path by.
    const TOP: i16 = 2146;
    const BOTTOM: i16 = -557;

    #[test]
    fn text_alignment_offsets() {
        // Baseline (the default) seats the baseline on the path ⇒ 0.
        assert_eq!(text_alignment_baseline_offset_units(None, TOP, BOTTOM), 0.0);
        assert_eq!(
            text_alignment_baseline_offset_units(Some("BaselineTextAlignment"), TOP, BOTTOM),
            0.0
        );
        // Unknown values fall back to Baseline (no panic, no offset).
        assert_eq!(
            text_alignment_baseline_offset_units(Some("WatTextAlignment"), TOP, BOTTOM),
            0.0
        );
        // InDesign's spelling and the one older Paged builds wrote agree.
        for (indesign, legacy) in [
            ("CenterTextAlignment", "CenterPathType"),
            ("AscenderTextAlignment", "AscenderPathType"),
            ("DescenderTextAlignment", "DescenderPathType"),
        ] {
            assert_eq!(
                text_alignment_baseline_offset_units(Some(indesign), TOP, BOTTOM),
                text_alignment_baseline_offset_units(Some(legacy), TOP, BOTTOM)
            );
        }
        // InDesign 20.0.1, Open Sans 24 pt on a straight line: the baseline
        // 25.125 pt below the line for Ascender, 6.562 pt above it for
        // Descender (measured at 1/16 pt).
        let pt =
            |a: &str| text_alignment_baseline_offset_units(Some(a), TOP, BOTTOM) * 24.0 / 2048.0;
        assert!((pt("AscenderTextAlignment") - 25.125).abs() < 0.07);
        assert!((pt("DescenderTextAlignment") + 6.562).abs() < 0.07);
        let center = pt("CenterTextAlignment");
        assert!(pt("DescenderTextAlignment") < center && center < pt("AscenderTextAlignment"));
    }

    #[test]
    fn path_alignment_rides_the_stroke_edge() {
        // InDesign 20.0.1: a 6 pt stroke put the baseline 3 pt above the
        // line for Top and 3 pt below it for Bottom.
        assert_eq!(path_alignment_shift_pt(Some("TopPathAlignment"), 6.0), -3.0);
        assert_eq!(
            path_alignment_shift_pt(Some("BottomPathAlignment"), 6.0),
            3.0
        );
        assert_eq!(
            path_alignment_shift_pt(Some("CenterPathAlignment"), 6.0),
            0.0
        );
        assert_eq!(path_alignment_shift_pt(None, 6.0), 0.0);
    }

    #[test]
    fn arc_length_sampler_is_monotonic_and_tangent_correct() {
        // A straight horizontal segment from (0,0) to (10,0): every
        // sample's cumulative length equals its x, the tangent angle is
        // 0, and out-of-range `s` clamps to the endpoints.
        let anchors = vec![
            PathAnchor {
                anchor: (0.0, 0.0),
                left: (0.0, 0.0),
                right: (0.0, 0.0),
            },
            PathAnchor {
                anchor: (10.0, 0.0),
                left: (10.0, 0.0),
                right: (10.0, 0.0),
            },
        ];
        let samples = tessellate_anchors(&anchors, 8);
        assert!(samples.len() >= 2);
        for w in samples.windows(2) {
            assert!(w[1].cum >= w[0].cum, "cum must be non-decreasing");
        }
        let total = samples.last().unwrap().cum;
        assert!((total - 10.0).abs() < 1e-3, "straight length is the chord");

        let (x, y, angle) = sample_path_at(&samples, 5.0).unwrap();
        assert!((x - 5.0).abs() < 1e-3 && y.abs() < 1e-3);
        assert!(angle.abs() < 1e-3, "horizontal tangent ≈ 0 rad");

        // Clamp past the end.
        let (xe, _, _) = sample_path_at(&samples, 1000.0).unwrap();
        assert!((xe - 10.0).abs() < 1e-3, "overshoot clamps to the end");
    }
}
