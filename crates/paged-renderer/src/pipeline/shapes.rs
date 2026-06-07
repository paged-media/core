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

//! Shape emitters for the non-text page items — ovals, lines (with
//! arrowheads), rectangles (incl. rounded / fancy corners) — plus the
//! shared corner-geometry helpers, blend-mode mapping, and the
//! missing-image placeholder visuals (50% grey + diagonal X).

use super::*;

/// Missing-image placeholder calibration (Q-22). Originally P-02
/// shipped with 0.7-grey + 0.5pt 0.25-grey X, which under-printed
/// against InDesign's reference. Histogramming the reference PNGs for
/// magazine-editorial-layout / catalog / project-case-study-template
/// puts the target at ~50% RGB grey with a 1.5pt near-black stroke.
pub(super) const PLACEHOLDER_FILL_RGB: f32 = 0.5;
pub(super) const PLACEHOLDER_X_STROKE_PT: f32 = 1.5;
pub(super) const PLACEHOLDER_X_RGB: f32 = 0.0;

pub(super) fn emit_oval_into(
    page: &mut BuiltPage,
    oval: &Oval,
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&paged_color::IccTransform>,
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
    let (effects_path, effects_xform, effects_unit_normalize) = if oval.effects.is_some() {
        if let Geometry::Oval { rect: r } = &frame.geometry {
            let (id, _) = page.list.paths.intern(
                paged_compose::UNIT_ELLIPSE_KEY,
                paged_compose::unit_ellipse(),
            );
            (Some(id), Transform::for_rect_in(*r, outer), Some(*r))
        } else {
            (None, outer, None)
        }
    } else {
        (None, outer, None)
    };
    if let (Some(pid), Some(effects)) = (effects_path, oval.effects.as_ref()) {
        crate::module::emit_effects_pre_fill(
            page,
            effects,
            pid,
            effects_xform,
            palette,
            cmyk_xform,
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
        stroke_for(
            frame.stroke_type,
            frame.effective_stroke_weight(),
            frame.end_cap,
            frame.end_join,
            frame.miter_limit,
            Some(&document.styles.stroke_styles),
            frame.stroke_dash,
        ),
    );
    if needs_group {
        pop_blend_group(page);
    }
}

/// Outward (off-the-end-of-the-path) direction at a line endpoint, for
/// orienting its arrowhead. `endpoint` is the terminal anchor;
/// `handle` is that anchor's adjacent Bezier control point (the
/// endpoint's `right` at the start, its `left` at the end);
/// `neighbour` is the next anchor inward. On a curved terminal segment
/// the cubic's tangent at the endpoint runs along `endpoint - handle`,
/// which is the correct arrowhead axis. IDML serialises a straight
/// segment as `handle == endpoint`; that degenerates the tangent, so
/// we fall back to the chord `endpoint - neighbour` (the old
/// straight-line behaviour). Returns an un-normalised direction —
/// `emit_arrowhead` normalises.
fn path_end_outward_dir(
    endpoint: (f32, f32),
    handle: (f32, f32),
    neighbour: (f32, f32),
) -> (f32, f32) {
    let (hx, hy) = (endpoint.0 - handle.0, endpoint.1 - handle.1);
    if hx * hx + hy * hy > 1e-6 {
        (hx, hy)
    } else {
        (endpoint.0 - neighbour.0, endpoint.1 - neighbour.1)
    }
}

/// Emit a filled arrowhead of `kind` at `tip`, pointing along the
/// outward direction `dir` at that line end. Size derives from the
/// stroke weight × `scale_pct`, matching InDesign's stroke-relative
/// arrowheads. Filled with `paint` (the stroke colour). `transform`
/// maps local coords to the page (inner→page for anchored lines,
/// identity for the page-local diagonal fallback). Emitted as a plain
/// `FillPath`, so the CPU and Vello backends both draw it.
#[allow(clippy::too_many_arguments)]
fn emit_arrowhead(
    page: &mut BuiltPage,
    kind: paged_parse::ArrowheadType,
    tip: (f32, f32),
    dir: (f32, f32),
    stroke_width: f32,
    scale_pct: f32,
    paint: Paint,
    transform: Transform,
) {
    use paged_compose::PathSegment::*;
    use paged_parse::ArrowheadType as A;
    if !kind.draws() || stroke_width <= 0.0 {
        return;
    }
    let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
    if len < 1e-6 {
        return;
    }
    let (dx, dy) = (dir.0 / len, dir.1 / len); // unit outward
    let (px, py) = (-dy, dx); // unit perpendicular
    let scale = (scale_pct / 100.0).max(0.05);
    // Arrowheads scale off the stroke weight (InDesign-like).
    let s = stroke_width * 4.0 * scale;
    let mut segs: Vec<paged_compose::PathSegment> = Vec::new();
    match kind {
        A::Triangle | A::TriangleWide | A::Simple | A::Other => {
            let half_w = if matches!(kind, A::TriangleWide) {
                s * 0.8
            } else {
                s * 0.5
            };
            let back = (tip.0 - dx * s, tip.1 - dy * s);
            segs.push(MoveTo { x: tip.0, y: tip.1 });
            segs.push(LineTo {
                x: back.0 + px * half_w,
                y: back.1 + py * half_w,
            });
            segs.push(LineTo {
                x: back.0 - px * half_w,
                y: back.1 - py * half_w,
            });
            segs.push(Close);
        }
        A::Bar => {
            let half_w = s * 0.7; // extent each side, perpendicular
            let half_t = (stroke_width * 0.6).max(0.5); // thickness along line
            let (ax, ay) = (dx * half_t, dy * half_t);
            let (bx, by) = (px * half_w, py * half_w);
            segs.push(MoveTo {
                x: tip.0 + bx + ax,
                y: tip.1 + by + ay,
            });
            segs.push(LineTo {
                x: tip.0 + bx - ax,
                y: tip.1 + by - ay,
            });
            segs.push(LineTo {
                x: tip.0 - bx - ax,
                y: tip.1 - by - ay,
            });
            segs.push(LineTo {
                x: tip.0 - bx + ax,
                y: tip.1 - by + ay,
            });
            segs.push(Close);
        }
        A::CircleSolid => {
            let r = s * 0.5;
            // Cap the line end: centre the disc one radius back.
            let c = (tip.0 - dx * r, tip.1 - dy * r);
            const KAPPA: f32 = 0.552_284_8;
            let k = r * KAPPA;
            segs.push(MoveTo { x: c.0 + r, y: c.1 });
            segs.push(CubicTo {
                cx1: c.0 + r,
                cy1: c.1 + k,
                cx2: c.0 + k,
                cy2: c.1 + r,
                x: c.0,
                y: c.1 + r,
            });
            segs.push(CubicTo {
                cx1: c.0 - k,
                cy1: c.1 + r,
                cx2: c.0 - r,
                cy2: c.1 + k,
                x: c.0 - r,
                y: c.1,
            });
            segs.push(CubicTo {
                cx1: c.0 - r,
                cy1: c.1 - k,
                cx2: c.0 - k,
                cy2: c.1 - r,
                x: c.0,
                y: c.1 - r,
            });
            segs.push(CubicTo {
                cx1: c.0 + k,
                cy1: c.1 - r,
                cx2: c.0 + r,
                cy2: c.1 - k,
                x: c.0 + r,
                y: c.1,
            });
            segs.push(Close);
        }
        A::None => return,
    }
    let path_id = page
        .list
        .paths
        .push_anon(paged_compose::PathData { segments: segs });
    page.list.push(paged_compose::DisplayCommand::FillPath {
        path_id,
        paint,
        transform,
    });
}

pub(super) fn emit_line_into(
    page: &mut BuiltPage,
    line: &GraphicLine,
    document: &Document,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
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
    let stroke = stroke_for(
        resolved.stroke_type,
        stroke_width,
        resolved.end_cap,
        resolved.end_join,
        resolved.miter_limit,
        Some(&document.styles.stroke_styles),
        resolved.stroke_dash,
    );
    // A multi-segment / curved / open line carries real path anchors;
    // stroke the actual outline (mirrors `emit_polygon_into`) instead
    // of the corner-to-corner diagonal of its bounds. The anchor path
    // is in inner coords and `frame_outer_transform` maps inner → page
    // (ItemTransform composed with the page-origin shift) — exactly the
    // mapping the diagonal fallback below gets via `transform_bounds`.
    if line.anchors.len() >= 2 {
        // A GraphicLine is an open path by definition; default any
        // contour the parser didn't explicitly flag to *open* so the
        // builder doesn't synthesise a closing segment back to start.
        let open_flags: Vec<bool> = (0..line.anchors.len())
            .map(|i| line.subpath_open.get(i).copied().unwrap_or(true))
            .collect();
        let path =
            polygon_path_from_anchors_with_open(&line.anchors, &line.subpath_starts, &open_flags);
        let cache_key = match resolved.self_id {
            Some(id) => fnv_1a_u64(id.as_bytes()),
            None => path_signature(&line.anchors),
        };
        let (path_id, _) = page.list.paths.intern(cache_key, path);
        let outer = frame_outer_transform(page, resolved.item_transform);
        page.list.push(paged_compose::DisplayCommand::StrokePath {
            path_id,
            paint: stroke_paint,
            stroke,
            transform: outer,
        });
        // Arrowheads at the first / last anchor, oriented outward along
        // each end's *Bezier tangent*. Built in inner coords and emitted
        // through the same `outer` transform as the stroke. On a curved
        // final segment the chord between the last two anchors points
        // the wrong way; `path_end_outward_dir` uses the endpoint's own
        // control handle (the cubic's true tangent there) and only
        // falls back to the chord when the handle coincides with the
        // anchor (the straight-segment serialisation).
        let n = line.anchors.len();
        if line.start_arrow.draws() {
            let a0 = line.anchors[0].anchor;
            emit_arrowhead(
                page,
                line.start_arrow,
                a0,
                path_end_outward_dir(a0, line.anchors[0].right, line.anchors[1].anchor),
                stroke_width,
                line.start_arrow_scale,
                stroke_paint,
                outer,
            );
        }
        if line.end_arrow.draws() {
            let an = line.anchors[n - 1].anchor;
            emit_arrowhead(
                page,
                line.end_arrow,
                an,
                path_end_outward_dir(an, line.anchors[n - 1].left, line.anchors[n - 2].anchor),
                stroke_width,
                line.end_arrow_scale,
                stroke_paint,
                outer,
            );
        }
        return;
    }
    // Anchorless line (synthetic `GeometricBounds`-only): rasterise the
    // corner-to-corner diagonal. GraphicLine.bounds is in inner coords;
    // ItemTransform maps it to spread coords, then the page subtracts
    // its spread_origin so the endpoints land in page-local coords.
    let spread_bounds = transform_bounds(line.bounds, resolved.item_transform);
    let (ox, oy) = page.spread_origin;
    let (sx, sy) = (spread_bounds.left - ox, spread_bounds.top - oy);
    let (ex, ey) = (spread_bounds.right - ox, spread_bounds.bottom - oy);
    emit_line(sx, sy, ex, ey, stroke, stroke_paint, &mut page.list);
    // Arrowheads at the diagonal's endpoints, in page-local coords
    // (identity transform). Start points back toward (sx,sy); end
    // points forward toward (ex,ey).
    if line.start_arrow.draws() {
        emit_arrowhead(
            page,
            line.start_arrow,
            (sx, sy),
            (sx - ex, sy - ey),
            stroke_width,
            line.start_arrow_scale,
            stroke_paint,
            Transform::IDENTITY,
        );
    }
    if line.end_arrow.draws() {
        emit_arrowhead(
            page,
            line.end_arrow,
            (ex, ey),
            (ex - sx, ey - sy),
            stroke_width,
            line.end_arrow_scale,
            stroke_paint,
            Transform::IDENTITY,
        );
    }
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
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&paged_color::IccTransform>,
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
    let (path_id, base_path, cache_key) = if let Geometry::Polygon {
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
        let (id, _) = page.list.paths.intern(cache_key, path.clone());
        (Some(id), Some(path), cache_key)
    } else {
        (None, None, 0)
    };
    crate::module::fill_paint_module(
        resolved, page, palette, cmyk_xform, fallback, outer, path_id,
    );
    let stroke_width = resolved.effective_stroke_weight();
    let stroke = stroke_for(
        resolved.stroke_type,
        stroke_width,
        resolved.end_cap,
        resolved.end_join,
        resolved.miter_limit,
        Some(&document.styles.stroke_styles),
        resolved.stroke_dash,
    );
    // W1.2: striped / wavy / gap-colour styled stroke on the polygon
    // outline. Polygon stroke alignment stays centred (open contours
    // can't be safely inset/outset; documented in `stroke_geom`).
    let class = classify_stroke_style(
        resolved.stroke_type,
        stroke_width,
        &document.styles.stroke_styles,
    );
    // FINDING #7.5 — a per-frame GapColor turns a dashed/dotted stroke
    // (whose gaps would otherwise show the page) into a gap-filled
    // styled stroke even when the style def declares no gap colour.
    let gap_override = frame_gap_override(resolved, &stroke, &class);
    let styled =
        !matches!(class, StrokeStyleClass::Plain { gap_color: None }) || gap_override.is_some();
    let handled = if styled && stroke_width > 0.0 {
        if let (Some(base), Some(paint)) = (
            base_path.as_ref(),
            resolved.stroke_color.and_then(|id| {
                color_id_to_paint_with_list_dir(
                    id,
                    palette,
                    cmyk_xform,
                    &mut page.list,
                    resolved.gradient_stroke_angle,
                    resolved.gradient_stroke_length,
                    Some((bbox.w, bbox.h)),
                )
            }),
        ) {
            emit_styled_stroke(
                page,
                base,
                cache_key,
                &class,
                &stroke,
                stroke_width,
                paint,
                palette,
                cmyk_xform,
                outer,
                gap_override,
            )
        } else {
            false
        }
    } else {
        false
    };
    if !handled {
        crate::module::stroke_paint_module(
            resolved, page, palette, cmyk_xform, outer, path_id, stroke,
        );
    }
    if needs_group {
        pop_blend_group(page);
    }
}

pub(super) fn emit_rectangle_into(
    page: &mut BuiltPage,
    rect: &Rectangle,
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    cmyk_xform: Option<&paged_color::IccTransform>,
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
        emit_rectangle_polygon_path(page, &resolved, document, palette, fallback, cmyk_xform);
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
            (id, Transform::for_rect_in(r, outer), Some(r))
        }
    };
    if let Some(effects) = rect.effects.as_ref() {
        crate::module::emit_effects_pre_fill(
            page,
            effects,
            effects_path,
            effects_xform,
            palette,
            cmyk_xform,
        );
    }

    crate::module::fill_paint_module(
        &resolved,
        page,
        palette,
        cmyk_xform,
        fallback,
        outer,
        corner.fill,
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
        Some(&document.styles.stroke_styles),
        resolved.stroke_dash,
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
            let inset = inset_rect(r, stroke_offset);
            // W1.2: striped / wavy stroke styles and the gap-colour
            // under-stroke run against a closed path of the (alignment-
            // adjusted) rectangle outline. A Plain style returns false →
            // the cheap axis-aligned rect emit below still runs.
            let class = classify_stroke_style(
                resolved.stroke_type,
                stroke_width,
                &document.styles.stroke_styles,
            );
            // FINDING #7.5 — per-frame GapColor override (see the polygon
            // path for the rationale).
            let gap_override = frame_gap_override(&resolved, &stroke, &class);
            let styled = !matches!(class, StrokeStyleClass::Plain { gap_color: None })
                || gap_override.is_some();
            let handled = if styled {
                let base = axis_rect_path(inset);
                let seed = resolved
                    .self_id
                    .map(|id| fnv_1a_u64(id.as_bytes()))
                    .unwrap_or(0xFEED);
                emit_styled_stroke(
                    page,
                    &base,
                    seed,
                    &class,
                    &stroke,
                    stroke_width,
                    paint,
                    palette,
                    cmyk_xform,
                    outer,
                    gap_override,
                )
            } else {
                false
            };
            if !handled {
                // Frame opacity is applied at the transparency-group
                // level by the orchestrator; per-paint scaling here
                // would double-apply the alpha.
                emit_stroke_rect_transformed(inset, outer, stroke, paint, &mut page.list);
            }
        }
    }
    if needs_group {
        pop_blend_group(page);
    }
}

/// Build a closed, axis-aligned rectangle outline as a [`PathData`] in
/// the rect's own coords. Used by the W1.2 styled-stroke emitter, which
/// needs a polyline-able outline (the cheap `emit_stroke_rect_*`
/// primitive doesn't expose one).
pub(crate) fn axis_rect_path(r: Rect) -> paged_compose::PathData {
    use paged_compose::PathSegment::{Close, LineTo, MoveTo};
    paged_compose::PathData {
        segments: vec![
            MoveTo { x: r.x, y: r.y },
            LineTo {
                x: r.x + r.w,
                y: r.y,
            },
            LineTo {
                x: r.x + r.w,
                y: r.y + r.h,
            },
            LineTo {
                x: r.x,
                y: r.y + r.h,
            },
            Close,
        ],
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
pub(crate) fn blend_mode_from_idml(name: Option<&str>) -> paged_compose::BlendMode {
    use paged_compose::BlendMode;
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

/// Which W1.2 stroke-STYLE family a `StrokeType` reference resolves to.
/// Drives whether the caller emits the default single `StrokePath`
/// (Solid/Dashed/Dotted) or the multi-rule (Striped) / sine (Wavy)
/// variants. `gap_color` carries the resolved gap-fill swatch ref for
/// the dashed/dotted/striped under-stroke second pass.
#[derive(Debug)]
pub(crate) enum StrokeStyleClass<'a> {
    /// Default: a single straight `StrokePath` (the existing emit). Dash
    /// pattern, if any, already lives on the `Stroke`. `gap_color` is
    /// the gap swatch for a patterned (dashed/dotted) stroke, if set.
    Plain { gap_color: Option<&'a str> },
    /// `<StripedStrokeStyle>`: N parallel rules. Each `(centre, weight)`
    /// is the stripe centreline offset from the stroke's upper edge and
    /// its sub-weight, both in pt.
    Striped {
        rules: Vec<(f32, f32)>,
        gap_color: Option<&'a str>,
    },
    /// `<WavyStrokeStyle>`: a sine ribbon. `amplitude` / `period` in pt;
    /// `sub_weight` is the pen width of the wave line.
    Wavy {
        amplitude: f32,
        period: f32,
        sub_weight: f32,
    },
}

/// FINDING #7.5 — the frame's INSTANCE gap colour override, or `None`.
/// Returns `Some((gap_color_id, gap_tint))` only when the frame carries a
/// `GapColor` AND the stroke actually has gaps to fill: either a dash
/// pattern (`stroke.dash`) or a striped/dashed/dotted `class`. A solid
/// stroke has no gaps, so the override is irrelevant there. This value
/// takes precedence over the `StrokeStyleDef`'s gap colour in
/// `emit_styled_stroke`.
fn frame_gap_override<'a>(
    resolved: &super::ResolvedFrame<'a>,
    stroke: &Stroke,
    class: &StrokeStyleClass<'_>,
) -> Option<(&'a str, Option<f32>)> {
    let gc = resolved.stroke_gap_color?;
    let class_has_gaps = matches!(
        class,
        StrokeStyleClass::Striped { .. } | StrokeStyleClass::Plain { gap_color: Some(_) }
    );
    if stroke.dash.len == 0 && !class_has_gaps {
        return None;
    }
    Some((gc, resolved.stroke_gap_tint))
}

/// Classify a frame's `StrokeType` into a [`StrokeStyleClass`] given the
/// total stroke weight and the document's custom-stroke-style table.
///
/// Striped sub-rules are derived from the `<Stripe>` table when present;
/// otherwise from the built-in name (`Thick - Thin`, `Thin - Thick`,
/// `Thick - Thin - Thick`, `Triple`) so a real IDML that references a
/// canned striped style renders without declaring custom stripes. Wavy
/// amplitude/period come from the style's `Width`/`Wavelength` ratios or
/// IDML-sensible defaults.
pub(crate) fn classify_stroke_style<'a>(
    stroke_type: Option<&str>,
    weight: f32,
    stroke_styles: &'a std::collections::BTreeMap<String, paged_parse::StrokeStyleDef>,
) -> StrokeStyleClass<'a> {
    use paged_parse::StrokeStyleKind as K;
    let Some(name) = stroke_type else {
        return StrokeStyleClass::Plain { gap_color: None };
    };
    // Custom `<StrokeStyle>` definition wins over the built-in name.
    if let Some(def) = stroke_styles.get(name) {
        let gap_color = def.gap_color.as_deref();
        match def.kind {
            K::Striped => {
                let rules = stripe_rules_from_def(&def.stripes, weight);
                if !rules.is_empty() {
                    return StrokeStyleClass::Striped { rules, gap_color };
                }
            }
            K::Wavy => {
                let (amplitude, period, sub_weight) =
                    wavy_params(def.wave_width, def.wave_length, weight);
                return StrokeStyleClass::Wavy {
                    amplitude,
                    period,
                    sub_weight,
                };
            }
            // Dashed / Dotted gap colour rides the default single
            // StrokePath; the gap swatch flows through for the
            // under-stroke pass.
            K::Dashed | K::Dotted => return StrokeStyleClass::Plain { gap_color },
        }
    }
    // Built-in names. `Canned ` prefix and the `StrokeStyle/$ID/`
    // namespace are stripped to match the bare style name.
    let suffix = name.strip_prefix("StrokeStyle/$ID/").unwrap_or(name);
    let bare = suffix.strip_prefix("Canned ").unwrap_or(suffix);
    if let Some(fractions) = builtin_stripe_fractions(bare) {
        let rules = stripe_rules_from_def(&fractions, weight);
        if !rules.is_empty() {
            return StrokeStyleClass::Striped {
                rules,
                gap_color: None,
            };
        }
    }
    if bare == "Wavy" {
        let (amplitude, period, sub_weight) = wavy_params(None, None, weight);
        return StrokeStyleClass::Wavy {
            amplitude,
            period,
            sub_weight,
        };
    }
    StrokeStyleClass::Plain { gap_color: None }
}

/// Convert a stripe table (`left`/`width` as 0..1 fractions of the total
/// weight) into `(centre, sub_weight)` pairs in pt, where `centre` is
/// the stripe centreline measured from the stroke's *upper* edge
/// (i.e. `-weight/2` … `+weight/2` once recentred by the caller).
/// Zero-width stripes are dropped.
fn stripe_rules_from_def(stripes: &[paged_parse::StripeDef], weight: f32) -> Vec<(f32, f32)> {
    stripes
        .iter()
        .filter(|s| s.width > 1e-4)
        .map(|s| {
            let sub_weight = s.width * weight;
            let centre_frac = s.left + s.width * 0.5;
            (centre_frac * weight, sub_weight)
        })
        .collect()
}

/// Built-in striped stroke styles InDesign ships, expressed as a stripe
/// table (left/width fractions of the total weight). Ordered top→bottom.
fn builtin_stripe_fractions(name: &str) -> Option<Vec<paged_parse::StripeDef>> {
    use paged_parse::StripeDef as S;
    let v = match name {
        "Thick - Thin" | "Thick-Thin" | "ThickThin" => vec![
            S {
                left: 0.0,
                width: 0.6,
            },
            S {
                left: 0.8,
                width: 0.2,
            },
        ],
        "Thin - Thick" | "Thin-Thick" | "ThinThick" => vec![
            S {
                left: 0.0,
                width: 0.2,
            },
            S {
                left: 0.4,
                width: 0.6,
            },
        ],
        "Thick - Thin - Thick" | "ThickThinThick" => vec![
            S {
                left: 0.0,
                width: 0.25,
            },
            S {
                left: 0.4,
                width: 0.2,
            },
            S {
                left: 0.75,
                width: 0.25,
            },
        ],
        "Thin - Thick - Thin" | "ThinThickThin" => vec![
            S {
                left: 0.0,
                width: 0.2,
            },
            S {
                left: 0.35,
                width: 0.3,
            },
            S {
                left: 0.8,
                width: 0.2,
            },
        ],
        "Triple" => vec![
            S {
                left: 0.0,
                width: 0.25,
            },
            S {
                left: 0.375,
                width: 0.25,
            },
            S {
                left: 0.75,
                width: 0.25,
            },
        ],
        _ => return None,
    };
    Some(v)
}

/// Wavy stroke amplitude / period / pen-width in pt. `width` and
/// `wavelength` are InDesign 0..1 ratios of the total weight; absent
/// values fall back to IDML-sensible defaults (amplitude = the weight,
/// period = 3× the weight, pen = 1/4 the weight, min 0.5pt).
fn wavy_params(width: Option<f32>, wavelength: Option<f32>, weight: f32) -> (f32, f32, f32) {
    let amplitude = width.map(|w| w * weight).unwrap_or(weight).max(0.1);
    let period = wavelength
        .map(|wl| (wl * weight).max(1.0))
        .unwrap_or(weight * 3.0)
        .max(1.0);
    let sub_weight = (weight * 0.25).max(0.5);
    (amplitude, period, sub_weight)
}

/// Emit a styled stroke for `base_path` (in inner coords). Handles the
/// W1.2 stroke-STYLE families: striped (N offset rules), wavy (sine
/// ribbon), and the dashed/dotted/striped gap-colour under-stroke. The
/// default single `StrokePath` (Solid/Dashed/Dotted) is emitted by the
/// caller when this returns `false`.
///
/// `closed` reflects whether `base_path` is a closed outline. The sine
/// flattening always produces an open ribbon (matching InDesign).
///
/// Returns `true` when it fully emitted the stroke (caller must skip its
/// own emit), `false` when the style is Plain and the caller should
/// proceed (after this has already emitted any gap-colour under-stroke).
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_styled_stroke(
    page: &mut BuiltPage,
    base_path: &paged_compose::PathData,
    cache_seed: u64,
    class: &StrokeStyleClass<'_>,
    stroke: &Stroke,
    weight: f32,
    stroke_paint: Paint,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
    outer: Transform,
    // FINDING #7.5 — the frame's INSTANCE `(GapColor, GapTint)`. When
    // present it overrides the `StrokeStyleDef`'s gap colour (W0.3's
    // per-frame mutation must win over the style def's W1.2 gap fill).
    gap_override: Option<(&str, Option<f32>)>,
) -> bool {
    use crate::pipeline::stroke_geom;
    // Resolve a gap-colour paint (if any) so the under-stroke pass can
    // run beneath dashed/dotted/striped patterns.
    let style_gap_color = match class {
        StrokeStyleClass::Plain { gap_color } | StrokeStyleClass::Striped { gap_color, .. } => {
            *gap_color
        }
        StrokeStyleClass::Wavy { .. } => None,
    };
    // Frame instance gap colour wins; fall back to the style def's. The
    // instance tint (if any) rides through `apply_fill_tint`.
    let (gap_color, gap_tint) = match gap_override {
        Some((gc, tint)) => (Some(gc), tint),
        None => (style_gap_color, None),
    };
    // Gap-colour second pass: a continuous solid under-stroke of the full
    // weight in the gap colour, beneath the patterned/striped stroke, so
    // the gaps show the gap colour rather than the page beneath. Emitted
    // first (drawn first ⇒ underneath).
    if let Some(gc) = gap_color {
        if let Some(gap_paint) = color_id_to_paint(gc, palette, cmyk_xform) {
            let gap_paint = crate::pipeline::apply_fill_tint(gap_paint, gap_tint);
            let (gap_path_id, _) = page
                .list
                .paths
                .intern(cache_seed ^ 0x9A7C, base_path.clone());
            page.list.push(paged_compose::DisplayCommand::StrokePath {
                path_id: gap_path_id,
                paint: gap_paint,
                stroke: Stroke::new(weight),
                transform: outer,
            });
        }
    }

    match class {
        StrokeStyleClass::Plain { .. } => false,
        StrokeStyleClass::Striped { rules, .. } => {
            // Flatten the outline once, then offset it per stripe. The
            // stripe centre is measured from the upper edge, so recentre
            // around 0 (the path centreline) by subtracting weight/2.
            let lines = stroke_geom::flatten_path(base_path, stroke_geom::FLATTEN_STEPS);
            for (stripe_idx, &(centre, sub_weight)) in rules.iter().enumerate() {
                let offset = centre - weight * 0.5;
                let mut sub_stroke = *stroke;
                sub_stroke.width = sub_weight.max(0.05);
                sub_stroke.dash = paged_compose::DashPattern::default();
                for (line_idx, line) in lines.iter().enumerate() {
                    let off = stroke_geom::offset_polyline(line, offset);
                    let path = stroke_geom::polyline_to_path(&off);
                    if path.is_empty() {
                        continue;
                    }
                    let key =
                        cache_seed ^ 0x57A1_0000 ^ ((stripe_idx as u64) << 8) ^ (line_idx as u64);
                    let (path_id, _) = page.list.paths.intern(key, path);
                    page.list.push(paged_compose::DisplayCommand::StrokePath {
                        path_id,
                        paint: stroke_paint,
                        stroke: sub_stroke,
                        transform: outer,
                    });
                }
            }
            true
        }
        StrokeStyleClass::Wavy {
            amplitude,
            period,
            sub_weight,
        } => {
            let lines = stroke_geom::flatten_path(base_path, stroke_geom::FLATTEN_STEPS);
            let mut sub_stroke = *stroke;
            sub_stroke.width = sub_weight.max(0.05);
            sub_stroke.dash = paged_compose::DashPattern::default();
            for (line_idx, line) in lines.iter().enumerate() {
                let wave = stroke_geom::sine_polyline(line, *amplitude, *period, 12);
                let path = stroke_geom::polyline_to_path(&wave);
                if path.is_empty() {
                    continue;
                }
                let (path_id, _) = page
                    .list
                    .paths
                    .intern(cache_seed ^ 0x4A7E_0000 ^ line_idx as u64, path);
                page.list.push(paged_compose::DisplayCommand::StrokePath {
                    path_id,
                    paint: stroke_paint,
                    stroke: sub_stroke,
                    transform: outer,
                });
            }
            true
        }
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
    // Normalise through `CornerOption::from_idml` so the full IDML
    // vocabulary is accepted — InDesign serialises these as
    // `RoundedCorner`, `BeveledCorner`, `InverseRoundedCorner`,
    // `InsetCorner`, `FancyCorner` (and the bare aliases). A hand-rolled
    // match on the short enum names silently dropped the radius for any
    // `…Corner`-suffixed value (W1.8: this was why a `Beveled` rect fell
    // back to the plain axis-aligned rect with no corner geometry).
    // Every non-`None` option consumes the radius.
    match option.and_then(paged_parse::CornerOption::from_idml) {
        Some(opt) if opt.rounds() => Some(r),
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
    corners: &[paged_parse::CornerSpec; 4],
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
        let r = spec.radius.or(corner_radius).filter(|r| *r > 0.0);
        // When the per-corner spec carries an option but no explicit
        // radius, inherit from the global fallback. When no fallback
        // either, the corner squares back off via `out[i] = None`.
        out[i] = r.or(fallback);
    }
    // Fast path: if no per-corner override touched the array, fall
    // back to the symmetric fallback for all four corners.
    if corners
        .iter()
        .all(|s| s.option.is_none() && s.radius.is_none())
    {
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
// Uniform-radius convenience over `corner_rect_path`; kept for callers
// even though the per-corner path is the only one wired in today.
#[allow(dead_code)]
pub(crate) fn rounded_rect_path(rect: Rect, radius: f32) -> paged_compose::PathData {
    corner_rect_path(
        rect,
        [Some(radius); 4],
        [paged_parse::CornerOption::Rounded; 4],
    )
}

/// Resolve the per-corner `CornerOption` *kind* (shape), `[tl, tr, br,
/// bl]`, parallel to [`per_corner_radii`]. Per-corner `spec.option`
/// wins; otherwise the global `corner_option` applies; otherwise the
/// corner is square (`None`).
pub(crate) fn per_corner_kinds(
    corner_option: Option<&str>,
    corners: &[paged_parse::CornerSpec; 4],
) -> [paged_parse::CornerOption; 4] {
    let global = corner_option
        .and_then(paged_parse::CornerOption::from_idml)
        .unwrap_or(paged_parse::CornerOption::None);
    let mut out = [paged_parse::CornerOption::None; 4];
    for (i, spec) in corners.iter().enumerate() {
        out[i] = spec.option.unwrap_or(global);
    }
    out
}

/// Rect path with per-corner radius AND per-corner `CornerOption`
/// shape. Walks clockwise from the top-left's top-edge point. Each
/// corner is a sharp 90° when its radius is `None`/`0` or its kind is
/// `None`; otherwise it emits the kind's geometry:
///
/// * `Rounded` — convex quarter-circle (control offset = `r·0.5523`).
/// * `Inverse` (inverse-rounded) — concave quarter-circle cut inward.
/// * `Bevel` — a straight 45° chamfer (the chord).
/// * `Inset` — a square notch stepping inward to the rounding centre.
/// * `Fancy` — an ogee (concave-then-convex double curve); an
///   approximation pending reference-PDF calibration.
///
/// The shape is emitted as backend-agnostic `PathData`, so the CPU and
/// Vello rasterizers both honour it with no per-backend work.
pub(crate) fn corner_rect_path(
    rect: Rect,
    radii: [Option<f32>; 4],
    kinds: [paged_parse::CornerOption; 4],
) -> paged_compose::PathData {
    use paged_compose::PathSegment::*;
    use paged_parse::CornerOption;
    const KAPPA: f32 = 0.552_284_8;
    let max_r = rect.w.min(rect.h) * 0.5;
    // Effective radius: 0 when the corner is square (`None` kind) or
    // its radius is absent / non-positive.
    let eff_r = |i: usize| -> f32 {
        if matches!(kinds[i], CornerOption::None) {
            return 0.0;
        }
        radii[i].map(|v| v.min(max_r).max(0.0)).unwrap_or(0.0)
    };
    let (l, t) = (rect.x, rect.y);
    let (right, bot) = (rect.x + rect.w, rect.y + rect.h);

    // Per corner: incoming edge end `p_in`, outgoing edge start
    // `p_out`, the sharp vertex `c`, and the inner rounding centre `m`.
    // Clockwise order TL, TR, BR, BL.
    let tl = eff_r(0);
    let tr = eff_r(1);
    let br = eff_r(2);
    let bl = eff_r(3);
    let geom = [
        // TL: from left edge to top edge.
        ((l, t + tl), (l + tl, t), (l, t), (l + tl, t + tl)),
        // TR: from top edge to right edge.
        (
            (right - tr, t),
            (right, t + tr),
            (right, t),
            (right - tr, t + tr),
        ),
        // BR: from right edge to bottom edge.
        (
            (right, bot - br),
            (right - br, bot),
            (right, bot),
            (right - br, bot - br),
        ),
        // BL: from bottom edge to left edge.
        ((l + bl, bot), (l, bot - bl), (l, bot), (l + bl, bot - bl)),
    ];

    // Emit one corner's segments (assuming the path's current point is
    // already at `p_in`), ending at `p_out`.
    let emit_corner = |segs: &mut Vec<paged_compose::PathSegment>,
                       kind: CornerOption,
                       r: f32,
                       p_in: (f32, f32),
                       p_out: (f32, f32),
                       c: (f32, f32),
                       m: (f32, f32)| {
        if r <= 0.0 || matches!(kind, CornerOption::None) {
            // Sharp: p_in == p_out == vertex; nothing to add.
            return;
        }
        // Control point a fraction `f` of the way from `p` toward
        // `toward` (the corner vertex `c` for convex, the inner
        // centre `m` for concave).
        let ctl = |p: (f32, f32), toward: (f32, f32), f: f32| {
            (p.0 + (toward.0 - p.0) * f, p.1 + (toward.1 - p.1) * f)
        };
        match kind {
            CornerOption::Rounded => {
                let c1 = ctl(p_in, c, KAPPA);
                let c2 = ctl(p_out, c, KAPPA);
                segs.push(CubicTo {
                    cx1: c1.0,
                    cy1: c1.1,
                    cx2: c2.0,
                    cy2: c2.1,
                    x: p_out.0,
                    y: p_out.1,
                });
            }
            CornerOption::Inverse => {
                // Concave: same endpoints, controls pulled toward
                // the inner centre so the arc bulges inward.
                let c1 = ctl(p_in, m, KAPPA);
                let c2 = ctl(p_out, m, KAPPA);
                segs.push(CubicTo {
                    cx1: c1.0,
                    cy1: c1.1,
                    cx2: c2.0,
                    cy2: c2.1,
                    x: p_out.0,
                    y: p_out.1,
                });
            }
            CornerOption::Bevel => {
                segs.push(LineTo {
                    x: p_out.0,
                    y: p_out.1,
                });
            }
            CornerOption::Inset => {
                // InDesign's Inset is the SHARP "fold-in" corner: the
                // edge steps inward to the inner rounding centre `m`
                // and back out to the outgoing edge — two straight
                // segments forming a right-angle notch. Applied to a
                // square this yields the cross / plus-sign silhouette
                // Adobe documents ("corners folding in on
                // themselves"). It is deliberately NOT a quarter-
                // circle: that is Inverse Rounded (a smooth concave
                // arc), and a circular Inset would collapse the two
                // options onto byte-identical geometry. W1.8
                // calibration verified the two stay visually distinct.
                segs.push(LineTo { x: m.0, y: m.1 });
                segs.push(LineTo {
                    x: p_out.0,
                    y: p_out.1,
                });
            }
            CornerOption::Fancy => {
                // InDesign's Fancy corner is an ornamental scallop:
                // three small arcs running p_in → q1 → q2 → p_out.
                // The two outer arcs are convex quarter-bumps (pulled
                // toward the sharp vertex `c`); the middle arc is a
                // concave notch (pulled toward the inner centre `m`).
                // That concave-between-two-convex rhythm is the
                // decorative three-arc pattern InDesign draws (an
                // honest approximation of the precise ornament, whose
                // exact radii Adobe never published — the segment
                // count, endpoints, and convex/concave rhythm match).
                //
                // q1 / q2 split the corner span into thirds along the
                // straight chord from p_in to p_out; each arc's
                // control points pull a third of the way toward `c`
                // (convex) or `m` (concave).
                let lerp = |a: (f32, f32), b: (f32, f32), f: f32| {
                    (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
                };
                let q1 = lerp(p_in, p_out, 1.0 / 3.0);
                let q2 = lerp(p_in, p_out, 2.0 / 3.0);
                // Arc 1: p_in → q1, convex bump toward the vertex.
                let a1 = ctl(p_in, c, 0.5);
                let a2 = ctl(q1, c, 0.5);
                segs.push(CubicTo {
                    cx1: a1.0,
                    cy1: a1.1,
                    cx2: a2.0,
                    cy2: a2.1,
                    x: q1.0,
                    y: q1.1,
                });
                // Arc 2: q1 → q2, concave notch toward the inner
                // centre (the ornament's central dip).
                let b1 = ctl(q1, m, 0.5);
                let b2 = ctl(q2, m, 0.5);
                segs.push(CubicTo {
                    cx1: b1.0,
                    cy1: b1.1,
                    cx2: b2.0,
                    cy2: b2.1,
                    x: q2.0,
                    y: q2.1,
                });
                // Arc 3: q2 → p_out, convex bump toward the vertex.
                let d1 = ctl(q2, c, 0.5);
                let d2 = ctl(p_out, c, 0.5);
                segs.push(CubicTo {
                    cx1: d1.0,
                    cy1: d1.1,
                    cx2: d2.0,
                    cy2: d2.1,
                    x: p_out.0,
                    y: p_out.1,
                });
            }
            CornerOption::None => {}
        }
    };

    let radius_of = [tl, tr, br, bl];
    let mut segments = Vec::with_capacity(17);
    // Start at TL's outgoing point on the top edge.
    let (_tl_in, tl_out, _tl_c, _tl_m) = geom[0];
    segments.push(MoveTo {
        x: tl_out.0,
        y: tl_out.1,
    });
    // Walk TR → BR → BL, each preceded by the edge LineTo to its p_in,
    // then TL last to close.
    for &i in &[1usize, 2, 3, 0] {
        let (p_in, p_out, c, m) = geom[i];
        segments.push(LineTo {
            x: p_in.0,
            y: p_in.1,
        });
        emit_corner(&mut segments, kinds[i], radius_of[i], p_in, p_out, c, m);
    }
    segments.push(Close);
    paged_compose::PathData { segments }
}

/// Approximate a unit ellipse with four cubic Bezier curves (the
/// standard 0.5522847 control-point distance for a circle). Returns
/// a `PathData` ready to intern under `UNIT_ELLIPSE_KEY`.
pub(super) fn unit_ellipse_path() -> paged_compose::PathData {
    use paged_compose::PathSegment;
    // Kappa for circular Bezier approximation.
    const K: f32 = 0.552_284_8;
    // Unit ellipse in the [0,1]×[0,1] rect: center (0.5, 0.5),
    // radius 0.5. Each quadrant is one CubicTo.
    let cx = 0.5;
    let cy = 0.5;
    let rx = 0.5;
    let ry = 0.5;
    let kx = rx * K;
    let ky = ry * K;
    paged_compose::PathData {
        segments: vec![
            PathSegment::MoveTo { x: cx + rx, y: cy },
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
pub(super) fn emit_oval_missing_image_placeholder(
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
    let rect = paged_compose::Rect {
        x: bounds.left,
        y: bounds.top,
        w: bounds.width(),
        h: bounds.height(),
    };
    paged_compose::emit_ellipse_transformed(rect, outer, grey, &mut page.list);
    let stroke = paged_compose::Stroke::new(PLACEHOLDER_X_STROKE_PT);
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

/// 50% grey fill + two 1.5pt diagonal stroke lines stamped over a
/// rectangle's path, matching InDesign's placeholder visual for image
/// frames whose `LinkResourceURI` doesn't resolve. The fill replaces
/// the host frame's normal paint (rectangles already drew their fill
/// in `emit_rectangle_into`; the placeholder paints on top because
/// the missing image would have done the same).
pub(super) fn emit_rectangle_missing_image_placeholder(
    page: &mut BuiltPage,
    rect: &Rectangle,
    outer: Transform,
) {
    let r = paged_compose::Rect {
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
    paged_compose::emit_rect_transformed(r, outer, grey, &mut page.list);
    let stroke = paged_compose::Stroke::new(PLACEHOLDER_X_STROKE_PT);
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
pub(super) fn emit_polygon_missing_image_placeholder(
    page: &mut BuiltPage,
    poly: &Polygon,
    outer: Transform,
) {
    use paged_compose::DisplayCommand;
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
        let r = paged_compose::Rect {
            x: bounds.left,
            y: bounds.top,
            w: bounds.width(),
            h: bounds.height(),
        };
        paged_compose::emit_rect_transformed(r, outer, grey, &mut page.list);
    }
    let stroke = paged_compose::Stroke::new(PLACEHOLDER_X_STROKE_PT);
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
    list: &mut paged_compose::DisplayList,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    outer: Transform,
    stroke: paged_compose::Stroke,
    paint: Paint,
) {
    use paged_compose::{DisplayCommand, PathData, PathSegment};
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

#[cfg(test)]
mod tests {
    use super::path_end_outward_dir;

    /// W1.1: on a straight terminal segment (IDML serialises straight
    /// segments with the handle coincident with its anchor), the
    /// arrowhead axis falls back to the chord between the endpoint and
    /// its inward neighbour — the legacy behaviour.
    #[test]
    fn arrowhead_dir_straight_segment_uses_chord() {
        // Endpoint (0,0), handle on the anchor, neighbour to the right.
        let dir = path_end_outward_dir((0.0, 0.0), (0.0, 0.0), (10.0, 0.0));
        // Outward = endpoint - neighbour = (-10, 0): points left, away
        // from the line body.
        assert!((dir.0 + 10.0).abs() < 1e-6 && dir.1.abs() < 1e-6);
    }

    /// W1.1: on a curved terminal segment the chord points the wrong
    /// way; the arrowhead must follow the cubic's true tangent, which
    /// runs along `endpoint - handle`.
    #[test]
    fn arrowhead_dir_curved_segment_follows_handle_tangent() {
        // Endpoint (0,0); its control handle sits above-right at (3,3)
        // (the curve leaves the endpoint heading there). The neighbour
        // anchor is straight right at (10,0) — its chord would give a
        // purely horizontal axis, but the real tangent is the
        // (endpoint - handle) = (-3,-3) diagonal.
        let dir = path_end_outward_dir((0.0, 0.0), (3.0, 3.0), (10.0, 0.0));
        assert!(
            (dir.0 + 3.0).abs() < 1e-6 && (dir.1 + 3.0).abs() < 1e-6,
            "curved end must use the handle tangent, got {dir:?}"
        );
    }
}

#[cfg(test)]
mod stroke_style_class_tests {
    use super::*;
    use paged_parse::{StripeDef, StrokeStyleDef, StrokeStyleKind as K};
    use std::collections::BTreeMap;

    fn styles(defs: Vec<StrokeStyleDef>) -> BTreeMap<String, StrokeStyleDef> {
        defs.into_iter().map(|d| (d.self_id.clone(), d)).collect()
    }

    fn def(self_id: &str, kind: K) -> StrokeStyleDef {
        StrokeStyleDef {
            self_id: self_id.to_string(),
            name: None,
            kind,
            pattern: Vec::new(),
            stripes: Vec::new(),
            wave_width: None,
            wave_length: None,
            gap_color: None,
            gap_tint: None,
        }
    }

    #[test]
    fn custom_striped_yields_one_rule_per_stripe_with_scaled_weights() {
        let mut d = def("StrokeStyle/S", K::Striped);
        d.stripes = vec![
            StripeDef {
                left: 0.0,
                width: 0.6,
            },
            StripeDef {
                left: 0.8,
                width: 0.2,
            },
        ];
        let m = styles(vec![d]);
        match classify_stroke_style(Some("StrokeStyle/S"), 10.0, &m) {
            StrokeStyleClass::Striped { rules, .. } => {
                assert_eq!(rules.len(), 2);
                // (centre, sub_weight): centre = (left + width/2)·weight.
                assert!((rules[0].0 - 3.0).abs() < 1e-4, "{:?}", rules[0]);
                assert!((rules[0].1 - 6.0).abs() < 1e-4, "{:?}", rules[0]);
                assert!((rules[1].0 - 9.0).abs() < 1e-4, "{:?}", rules[1]);
                assert!((rules[1].1 - 2.0).abs() < 1e-4, "{:?}", rules[1]);
            }
            other => panic!("expected Striped, got {other:?}"),
        }
    }

    #[test]
    fn builtin_thick_thin_name_resolves_to_striped() {
        let m = BTreeMap::new();
        let class = classify_stroke_style(Some("StrokeStyle/$ID/Canned Thick - Thin"), 8.0, &m);
        assert!(matches!(class, StrokeStyleClass::Striped { .. }));
    }

    #[test]
    fn custom_wavy_uses_width_and_wavelength_ratios() {
        let mut d = def("StrokeStyle/W", K::Wavy);
        d.wave_width = Some(0.5);
        d.wave_length = Some(2.0);
        let m = styles(vec![d]);
        match classify_stroke_style(Some("StrokeStyle/W"), 10.0, &m) {
            StrokeStyleClass::Wavy {
                amplitude, period, ..
            } => {
                assert!((amplitude - 5.0).abs() < 1e-4, "amp={amplitude}");
                assert!((period - 20.0).abs() < 1e-4, "period={period}");
            }
            other => panic!("expected Wavy, got {other:?}"),
        }
    }

    #[test]
    fn dashed_with_gap_color_is_plain_with_gap() {
        let mut d = def("StrokeStyle/G", K::Dashed);
        d.gap_color = Some("Color/Cyan".to_string());
        let m = styles(vec![d]);
        match classify_stroke_style(Some("StrokeStyle/G"), 4.0, &m) {
            StrokeStyleClass::Plain { gap_color } => {
                assert_eq!(gap_color, Some("Color/Cyan"));
            }
            other => panic!("expected Plain with gap, got {other:?}"),
        }
    }

    #[test]
    fn unknown_and_solid_are_plain_without_gap() {
        let m = BTreeMap::new();
        assert!(matches!(
            classify_stroke_style(Some("StrokeStyle/$ID/Solid"), 2.0, &m),
            StrokeStyleClass::Plain { gap_color: None }
        ));
        assert!(matches!(
            classify_stroke_style(None, 2.0, &m),
            StrokeStyleClass::Plain { gap_color: None }
        ));
    }
}
