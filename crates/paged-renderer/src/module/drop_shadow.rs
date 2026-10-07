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

//! Drop-shadow module.
//!
//! Resolves a frame's `<DropShadowSetting>` (or the document-wide
//! fallback from `PipelineOptions::frame_drop_shadow`) into a
//! [`DropShadow`] paint and emits the stamp behind what the frame
//! paints. A frame with a transparent fill never stamps its fill
//! outline — that would leak a solid backdrop through the otherwise
//! invisible frame (see commit 9f98738 / 2c33465) — only its stroke
//! band, when the stroke is visible.
//!
//! Stroke shadows (`<StrokeTransparencySetting><DropShadowSetting>`)
//! are emitted only when the frame's stroke is actually visible
//! (`StrokeColor != Swatch/None` AND `StrokeWeight > 0`). InDesign's
//! stroke shadow is a blurred outline of the stroke path; we
//! approximate with the same rect-stamp the fill-shadow uses,
//! which is correct for opaque-stroked rectangles and a close
//! visual match for fill-less / open-frame variants until path-
//! shaped shadow support lands.
//!
//! A plain rectangle or text frame stamps its rect, a rounded one its
//! corner outline; an oval stamps its ellipse. W1.1: Polygons
//! (and pathed Rectangles / TextFrames lifted to `Geometry::Polygon`)
//! cast a *path-shaped* shadow — the frame's real outline is interned
//! and emitted as a `DropShadow { path_id, .. }` (σ-scale 1.0, the
//! frame-body blur, distinct from the wider glyph-shadow `PathShadow`),
//! so a triangle / Bezier frame's shadow hugs the shape rather than
//! its bounding box. A line has no fill, so its shadow is cast by its
//! stroke: [`line_drop_shadow_module`] strokes the centreline into its
//! outline band and stamps that.
//!
//! InDesign casts an object's shadow from what the object paints, not
//! from its fill alone (measured on InDesign 20.0.1 by the
//! `line-shadows` fixture): an oval or a polygon with no fill and a
//! visible stroke — a pen path, a stroke-only triangle — casts the
//! shadow of its stroke band, exactly as a line does. The engine stamps
//! that band, and a filled, stroked shape stamps its outline pushed out
//! by the stroke's outer part. The `rect-shadows` fixture measured the
//! same rule for rectangles and text frames, rounded corners included;
//! a text frame with no fill also shadows its text, which
//! `BuildEngine::apply_blend_groups` casts glyph by glyph.

use paged_compose::{
    emit_drop_shadow_rect_transformed, DisplayCommand, DropShadow, LineCap, LineJoin, PathData,
    PathId, PathSegment, Rect, Stroke, Transform,
};
use paged_model::{DropShadowSetting, Graphic};

use super::{Geometry, ResolvedFrame};
use crate::pipeline::{
    aligned_outline_path, corner_rect_path, ellipse_outline_path, fnv_1a_u64,
    frame_fill_is_transparent, frame_stroke_is_visible, inset_rect, path_signature,
    per_corner_kinds, per_corner_radii, polygon_path_from_anchors_with_open, resolve_frame_shadow,
    stroke_alignment_offset, stroke_for, BuiltPage, ColorCtx,
};

/// Emit the drop-shadow stamp(s) for a frame. The object shadow stamps
/// what the frame paints (its fill outline grown by the stroke's outer
/// part, or its stroke band when unfilled); the stroke-transparency
/// shadow stamps the frame's outline when the stroke is visible
/// (`StrokeColor != Swatch/None` AND `StrokeWeight > 0`).
pub(crate) fn drop_shadow_module(
    frame: &ResolvedFrame<'_>,
    page: &mut BuiltPage,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    fallback: Option<DropShadow>,
    outer: Transform,
    stroke_drop_shadow: Option<&DropShadowSetting>,
) {
    // Resolve the shape the shadow stamps under: an axis-aligned rect
    // for a plain Rect / TextFrameRect, or the frame's real outline
    // (interned once, reused by both fill and stroke shadows) for a
    // rounded one, an Oval or a pathed Polygon. Lines cast theirs in
    // `line_drop_shadow_module`.
    let cache_key = frame.self_id.map(|id| fnv_1a_u64(id.as_bytes()));
    let target: ShadowTarget = match &frame.geometry {
        Geometry::Rect { rect } | Geometry::TextFrameRect { rect } => {
            match rect_outline(frame, *rect, 0.0) {
                RectOutline::Plain(rect) => ShadowTarget::Rect(rect),
                RectOutline::Path(path) => {
                    let key = cache_key.unwrap_or_else(|| path_signature_of(frame))
                        ^ 0x5AD0_C022_0000_0000;
                    ShadowTarget::Path(page.list.paths.intern(key, path).0)
                }
            }
        }
        Geometry::Oval { rect } => {
            let key = cache_key.unwrap_or_else(|| path_signature_of(frame)) ^ 0x5AD0_E111_0000_0000;
            let (path_id, _) = page.list.paths.intern(key, ellipse_outline_path(*rect));
            ShadowTarget::Path(path_id)
        }
        Geometry::Polygon {
            anchors,
            subpath_starts,
            subpath_open,
            bbox,
        } => {
            if anchors.is_empty() {
                ShadowTarget::Rect(*bbox)
            } else {
                let path =
                    polygon_path_from_anchors_with_open(anchors, subpath_starts, subpath_open);
                let cache_key = match frame.self_id {
                    Some(id) => fnv_1a_u64(id.as_bytes()),
                    None => path_signature(anchors),
                };
                let (path_id, _) = page.list.paths.intern(cache_key, path);
                ShadowTarget::Path(path_id)
            }
        }
        Geometry::Line { .. } => return,
    };

    // Object shadow, cast from what the object paints. With a visible
    // fill it stamps the fill outline (never a solid backdrop through a
    // transparent frame); with no fill but a visible stroke it stamps
    // the stroke band, as InDesign does for every closed shape.
    let stroke_visible =
        frame_stroke_is_visible(frame.stroke_color, frame.effective_stroke_weight());
    if !frame_fill_is_transparent(frame.fill_color) {
        if let Some(shadow) = resolve_frame_shadow(frame.drop_shadow, fallback, palette, color_ctx)
        {
            // Filled AND stroked: the fill plus the stroke's outer part.
            let painted = stroke_visible
                .then(|| filled_and_stroked_outline_of(frame))
                .flatten()
                .map(|outline| match outline {
                    RectOutline::Plain(rect) => ShadowTarget::Rect(rect),
                    RectOutline::Path(outline) => {
                        let key = cache_key.unwrap_or_else(|| path_signature_of(frame))
                            ^ 0x5AD0_F111_0000_0000;
                        ShadowTarget::Path(page.list.paths.intern(key, outline).0)
                    }
                });
            emit_shadow(painted.unwrap_or(target), outer, shadow, page);
        }
    } else if stroke_visible {
        if let Some(shadow) = resolve_frame_shadow(frame.drop_shadow, None, palette, color_ctx) {
            if let Some(band) = stroke_band_of(frame) {
                let key =
                    cache_key.unwrap_or_else(|| path_signature_of(frame)) ^ 0x5AD0_B4ED_0000_0000;
                let (path_id, _) = page.list.paths.intern(key, band);
                emit_shadow(ShadowTarget::Path(path_id), outer, shadow, page);
            }
        }
    }

    // Stroke shadow — only when the stroke is actually visible.
    // Resolving via `resolve_frame_shadow(..., None, ...)` so the
    // synthetic fallback only ever supplies the *fill* shadow.
    if stroke_visible {
        if let Some(shadow) = resolve_frame_shadow(stroke_drop_shadow, None, palette, color_ctx) {
            emit_shadow(target, outer, shadow, page);
        }
    }
}

/// The stroke band of a frame's outline in inner coordinates: the
/// outline (offset for Inside / Outside alignment, as the stroke itself
/// is) stroked at the frame's weight, cap, join and miter limit. A dash
/// is not cut out. `None` for a line.
fn stroke_band_of(frame: &ResolvedFrame<'_>) -> Option<PathData> {
    let weight = frame.effective_stroke_weight();
    let outline = match &frame.geometry {
        Geometry::Rect { rect } | Geometry::TextFrameRect { rect } => {
            let offset = stroke_alignment_offset(frame.stroke_alignment, weight);
            rect_outline(frame, *rect, offset).into_path()
        }
        _ => {
            let outline = outline_of(frame)?;
            aligned_outline_path(&outline, frame.stroke_alignment, weight).unwrap_or(outline)
        }
    };
    let stroke = stroke_for(
        None,
        weight,
        frame.end_cap,
        frame.end_join,
        frame.miter_limit,
        None,
        &[],
    );
    let band = stroke_outline(&outline, &stroke);
    (!band.segments.is_empty()).then_some(band)
}

/// The outline of what a filled, stroked frame paints: its outline
/// pushed out by the part of the stroke that lies outside it (half the
/// weight centred, all of it Outside, none Inside), on every closed
/// contour. `None` for a line and for Inside alignment, where the fill
/// outline already is the painted outline.
fn filled_and_stroked_outline_of(frame: &ResolvedFrame<'_>) -> Option<RectOutline> {
    let outside = match frame.stroke_alignment {
        Some("InsideAlignment") => return None,
        Some("OutsideAlignment") => frame.effective_stroke_weight(),
        _ => frame.effective_stroke_weight() * 0.5,
    };
    if let Geometry::Rect { rect } | Geometry::TextFrameRect { rect } = &frame.geometry {
        return Some(rect_outline(frame, *rect, -outside));
    }
    let outline = outline_of(frame)?;
    // `aligned_outline_path` offsets each closed contour by HALF the
    // weight it is given.
    aligned_outline_path(&outline, Some("OutsideAlignment"), outside * 2.0).map(RectOutline::Path)
}

/// A rectangular frame's outline: the plain rect, or the corner path
/// when a corner effect resolves.
enum RectOutline {
    Plain(Rect),
    Path(PathData),
}

impl RectOutline {
    fn into_path(self) -> PathData {
        match self {
            RectOutline::Path(path) => path,
            RectOutline::Plain(r) => PathData {
                segments: vec![
                    PathSegment::MoveTo { x: r.x, y: r.y },
                    PathSegment::LineTo {
                        x: r.x + r.w,
                        y: r.y,
                    },
                    PathSegment::LineTo {
                        x: r.x + r.w,
                        y: r.y + r.h,
                    },
                    PathSegment::LineTo {
                        x: r.x,
                        y: r.y + r.h,
                    },
                    PathSegment::Close,
                ],
            },
        }
    }
}

/// A rectangle's or a text frame's outline moved in by `offset` (out
/// when negative), each corner radius adjusted to stay concentric —
/// the geometry `corner_path_module` strokes.
fn rect_outline(frame: &ResolvedFrame<'_>, rect: Rect, offset: f32) -> RectOutline {
    let radii = per_corner_radii(frame.corner_radius, frame.corner_option, &frame.corners);
    let moved = inset_rect(rect, offset);
    if radii.iter().all(Option::is_none) {
        return RectOutline::Plain(moved);
    }
    let radii = radii.map(|r| r.map(|r| (r - offset).max(0.0)));
    let kinds = per_corner_kinds(frame.corner_option, &frame.corners);
    RectOutline::Path(corner_rect_path(moved, radii, kinds))
}

/// An oval's or a polygon's outline in inner coordinates, as its
/// stroke follows it; `None` for the rectangular kinds and lines.
fn outline_of(frame: &ResolvedFrame<'_>) -> Option<PathData> {
    match &frame.geometry {
        Geometry::Oval { rect } => Some(ellipse_outline_path(*rect)),
        Geometry::Polygon {
            anchors,
            subpath_starts,
            subpath_open,
            ..
        } if !anchors.is_empty() => Some(polygon_path_from_anchors_with_open(
            anchors,
            subpath_starts,
            subpath_open,
        )),
        _ => None,
    }
}

/// A key for an anonymous frame's outline (one with no `Self`).
fn path_signature_of(frame: &ResolvedFrame<'_>) -> u64 {
    match &frame.geometry {
        Geometry::Polygon { anchors, .. } => path_signature(anchors),
        Geometry::Oval { rect } | Geometry::Rect { rect } | Geometry::TextFrameRect { rect } => {
            fnv_1a_u64(format!("{} {} {} {}", rect.x, rect.y, rect.w, rect.h).as_bytes())
        }
        _ => 0,
    }
}

/// What a frame's drop shadow stamps under: an axis-aligned rect (a
/// plain rectangle / text frame) or an interned path (a rounded
/// frame's, an oval's or a polygon's outline, or a stroke band).
#[derive(Clone, Copy)]
enum ShadowTarget {
    Rect(Rect),
    Path(PathId),
}

fn emit_shadow(target: ShadowTarget, outer: Transform, shadow: DropShadow, page: &mut BuiltPage) {
    match target {
        ShadowTarget::Rect(rect) => {
            emit_drop_shadow_rect_transformed(rect, outer, shadow, &mut page.list);
        }
        // The interned polygon path is already in inner-anchor coords;
        // `outer` carries the page-origin + ItemTransform. Use the
        // frame-body `DropShadow` variant (σ-scale 1.0) rather than the
        // wider glyph-shadow `PathShadow`.
        ShadowTarget::Path(path_id) => {
            page.list.push(DisplayCommand::DropShadow {
                path_id,
                transform: outer,
                shadow,
            });
        }
    }
}

/// Emit a line's drop shadow: the stroke's outline band (the centreline
/// stroked at the line's width, cap, join and miter limit; a dash is
/// not cut out — the shadow is the solid band) stamped as a
/// path-shaped `DropShadow` under `transform`, the transform the
/// stroke itself is drawn with.
#[allow(clippy::too_many_arguments)]
pub(crate) fn line_drop_shadow_module(
    page: &mut BuiltPage,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    setting: Option<&DropShadowSetting>,
    centreline: &PathData,
    stroke: &Stroke,
    cache_key: u64,
    transform: Transform,
) {
    let Some(shadow) = resolve_frame_shadow(setting, None, palette, color_ctx) else {
        return;
    };
    if stroke.width <= 0.0 {
        return;
    }
    let band = stroke_outline(centreline, stroke);
    if band.segments.is_empty() {
        return;
    }
    // Salted so the band never collides with the centreline the stroke
    // interned under the line's own key.
    let (path_id, _) = page
        .list
        .paths
        .intern(cache_key ^ 0x5AD0_57A0_0000_0000, band);
    page.list.push(DisplayCommand::DropShadow {
        path_id,
        transform,
        shadow,
    });
}

/// The closed outline of `path` stroked with `stroke` (no dash).
fn stroke_outline(path: &PathData, stroke: &Stroke) -> PathData {
    use kurbo::{BezPath, Cap, Join, PathEl, Point, StrokeOpts};
    let p = |x: f32, y: f32| Point::new(f64::from(x), f64::from(y));
    let mut centre = BezPath::new();
    for seg in &path.segments {
        match *seg {
            PathSegment::MoveTo { x, y } => centre.move_to(p(x, y)),
            PathSegment::LineTo { x, y } => centre.line_to(p(x, y)),
            PathSegment::QuadTo { cx, cy, x, y } => centre.quad_to(p(cx, cy), p(x, y)),
            PathSegment::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => centre.curve_to(p(cx1, cy1), p(cx2, cy2), p(x, y)),
            PathSegment::Close => centre.close_path(),
        }
    }
    let style = kurbo::Stroke::new(f64::from(stroke.width))
        .with_caps(match stroke.cap {
            LineCap::Butt => Cap::Butt,
            LineCap::Round => Cap::Round,
            LineCap::Square => Cap::Square,
        })
        .with_join(match stroke.join {
            LineJoin::Miter => Join::Miter,
            LineJoin::Round => Join::Round,
            LineJoin::Bevel => Join::Bevel,
        })
        .with_miter_limit(f64::from(stroke.miter_limit));
    let band = kurbo::stroke(centre, &style, &StrokeOpts::default(), 0.05);
    let f = |pt: Point| (pt.x as f32, pt.y as f32);
    let segments = band
        .elements()
        .iter()
        .map(|el| match *el {
            PathEl::MoveTo(a) => {
                let (x, y) = f(a);
                PathSegment::MoveTo { x, y }
            }
            PathEl::LineTo(a) => {
                let (x, y) = f(a);
                PathSegment::LineTo { x, y }
            }
            PathEl::QuadTo(c, a) => {
                let ((cx, cy), (x, y)) = (f(c), f(a));
                PathSegment::QuadTo { cx, cy, x, y }
            }
            PathEl::CurveTo(c1, c2, a) => {
                let ((cx1, cy1), (cx2, cy2), (x, y)) = (f(c1), f(c2), f(a));
                PathSegment::CubicTo {
                    cx1,
                    cy1,
                    cx2,
                    cy2,
                    x,
                    y,
                }
            }
            PathEl::ClosePath => PathSegment::Close,
        })
        .collect();
    PathData { segments }
}
