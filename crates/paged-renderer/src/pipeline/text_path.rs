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
pub(super) fn polygon_path_from_anchors(anchors: &[PathAnchor], subpath_starts: &[usize]) -> PathData {
    polygon_path_from_anchors_with_open(anchors, subpath_starts, &[])
}

/// Same as `polygon_path_from_anchors` but consults a parallel
/// `subpath_open` slice. An open contour skips the closing CubicTo +
/// Close so a hand-drawn lassoed stroke or a `PathOpen="true"` clip
/// path doesn't get auto-filled (P-15). `subpath_open` is interpreted
/// against the indexed order of contours (the `i`th true ⇒ `i`th
/// contour open); a shorter slice / empty slice means every contour
/// is closed (legacy behaviour).
pub(super) fn polygon_path_from_anchors_with_open(
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
pub(super) fn emit_polygon_into(
    page: &mut BuiltPage,
    poly: &Polygon,
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&paged_color::IccTransform>,
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
            _ => paged_compose::Rect {
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
        stroke_for(
            resolved.stroke_type,
            resolved.effective_stroke_weight(),
            resolved.end_cap,
            resolved.end_join,
            resolved.miter_limit,
            Some(&document.styles.stroke_styles),
        ),
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
pub(super) fn emit_text_path_into(
    page: &mut BuiltPage,
    text_path: &TextPath,
    anchors: &[PathAnchor],
    item_transform: Option<[f32; 6]>,
    document: &Document,
    options: &PipelineOptions,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
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
            let mut shaped = paged_text::shape::shape_run(rb_face, &run.text, point_size);
            if let Some(t) = resolved.tracking {
                paged_text::shape::apply_tracking(&mut shaped, t, point_size);
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
        let advance_pt = g.x_advance_64 as f32 / paged_text::shape::ADVANCE_PRECISION;
        let x_off_pt = g.x_offset_64 as f32 / paged_text::shape::ADVANCE_PRECISION;
        let y_off_pt = g.y_offset_64 as f32 / paged_text::shape::ADVANCE_PRECISION;
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
        // Concept 3 — glyph-run side-channel (text-on-path glyphs
        // carry a rotated affine; the exporter reuses it verbatim).
        if let Some(runs) = page.list.glyph_runs.as_mut() {
            runs.push(paged_compose::GlyphRunEntry {
                command_index: page.list.commands.len() as u32,
                font_id: face_font_ids[g.face_idx],
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
}
