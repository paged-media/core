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

//! Fill-paint module.
//!
//! Resolves the frame's fill from `FillColor` (with gradient angle,
//! tint, and opacity baked in) and emits the geometry-appropriate
//! primitive through [`super::geometry::emit_filled`]. Skipped when
//! the fill is transparent.

use paged_compose::{BlendMode, Paint, PathId, Rect, Transform};
use paged_model::Graphic;

use super::geometry::{emit_filled, rewrite_tail_for_overprint};
use super::{Geometry, ResolvedFrame};
use crate::pipeline::{
    apply_fill_tint_ctx, color_id_to_paint_with_list_dir, frame_fill_is_transparent, object_tint,
    BuiltPage, ColorCtx,
};

/// Resolve and emit the frame fill. `fill_path`, when `Some`, routes
/// the emit through `FillPath` against the pre-interned path (rounded
/// Rectangle / Polygon).
///
/// The frame's blend mode and opacity are applied at the
/// transparency-group level by the orchestrator (the body+glyphs are
/// bracketed in `BeginBlendGroup` / `EndBlendGroup` when non-trivial).
/// Fill emission therefore always uses `BlendMode::Normal` and skips
/// per-paint opacity scaling — the group composite handles both.
pub(crate) fn fill_paint_module(
    frame: &ResolvedFrame<'_>,
    page: &mut BuiltPage,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    fallback: Paint,
    outer: Transform,
    fill_path: Option<PathId>,
) {
    if frame_fill_is_transparent(frame.fill_color) {
        return;
    }
    // Bbox dims for gradient defaults: pulled from the same Rect the
    // geometry adapter writes (Rectangle/TextFrame/Oval/Polygon all
    // carry one). Lines have no fill so the fall-through `None` is
    // unreachable for them.
    let path_dims = match frame.geometry {
        Geometry::Rect { rect }
        | Geometry::TextFrameRect { rect }
        | Geometry::Oval { rect }
        | Geometry::Polygon { bbox: rect, .. } => Some((rect.w, rect.h)),
        Geometry::Line { .. } => None,
    };
    let fill = frame
        .fill_color
        .and_then(|id| {
            color_id_to_paint_with_list_dir(
                id,
                palette,
                color_ctx,
                &mut page.list,
                frame.gradient_fill_angle,
                frame.gradient_fill_length,
                path_dims,
            )
        })
        .unwrap_or(fallback);
    let fill = apply_fill_tint_ctx(
        fill,
        object_tint(palette, frame.fill_color, frame.fill_tint),
        &color_ctx,
    );
    // Q-08: Polygon fills route through `emit_fill_path`, whose
    // path_transform = `outer` directly (the path lives in inner-
    // anchor coords). A gradient paint's endpoints are stored in
    // unit-rect (0..1) coords, which `for_rect_in` would normally
    // bake into the path's bbox via the rect / oval emit helpers.
    // For polygons that bbox step is absent — the rasterizer applies
    // `outer` to the unit-rect endpoints and the gradient line
    // collapses to ~1pt near the spread origin, making the polygon
    // paint flat. Pre-bake the unit→bbox mapping into the stored
    // gradient endpoints so the downstream `outer` lands them in the
    // polygon's actual page span. Track 4c extends the same rewrite
    // to radial gradients (center + radius in unit-rect coords).
    let fill = match (fill, &frame.geometry) {
        (Paint::LinearGradient(gid), Geometry::Polygon { bbox, .. }) => {
            rebase_gradient_to_bbox(page, gid, *bbox);
            Paint::LinearGradient(gid)
        }
        (Paint::RadialGradient(gid), Geometry::Polygon { bbox, .. }) => {
            rebase_radial_gradient_to_bbox(page, gid, *bbox);
            Paint::RadialGradient(gid)
        }
        _ => fill,
    };
    match fill {
        Paint::LinearGradient(gid) => place_linear_at_start(page, gid, frame),
        Paint::RadialGradient(gid) => place_radial_at_start(page, gid, frame),
        _ => {}
    }
    let start = page.list.commands.len();
    emit_filled(
        &frame.geometry,
        page,
        fill,
        BlendMode::Normal,
        outer,
        fill_path,
    );
    rewrite_tail_for_overprint(page, start, frame.overprint_fill, false);
}

/// Map the linear gradient's unit-rect endpoints onto `bbox` in
/// place. Used for Polygon fills whose `path_transform` is `outer`
/// directly (no `for_rect_in` step). After this rewrite the
/// rasterizer's `outer.apply(...)` lands endpoints inside the
/// polygon's inner-coord bbox, matching what rect / oval gradients
/// produce via `Transform::for_rect_in`.
fn rebase_gradient_to_bbox(page: &mut BuiltPage, gid: paged_compose::GradientId, bbox: Rect) {
    let idx = gid.0 as usize;
    if let Some(g) = page.list.gradients.get_mut(idx) {
        g.start = (bbox.x + g.start.0 * bbox.w, bbox.y + g.start.1 * bbox.h);
        g.end = (bbox.x + g.end.0 * bbox.w, bbox.y + g.end.1 * bbox.h);
    }
}

/// An explicit `GradientFillStart` anchors a linear gradient's axis: it
/// runs from the start (the frame's inner coordinates) for
/// `GradientFillLength` pt in the direction `(cos θ, −sin θ)` — asked of
/// InDesign, `gradient-geometry` cases 19–21: `start 50 80, length 60`
/// at 90° on a 100 × 100 frame ramps from y 80 UP to y 20. A missing or
/// non-positive length is the frame's width, as for the default
/// placement (`pipeline::color_paint::linear_gradient_endpoints`).
///
/// A polygon's gradient is already in inner coordinates (see
/// [`rebase_gradient_to_bbox`]); a rect's, a text frame's and an oval's
/// are in its unit rectangle, which the rasterisers map back to the
/// page by the endpoints alone, so the axis keeps its angle on a
/// non-square frame.
fn place_linear_at_start(
    page: &mut BuiltPage,
    gid: paged_compose::GradientId,
    frame: &ResolvedFrame<'_>,
) {
    let bbox = match frame.geometry {
        Geometry::Rect { rect }
        | Geometry::TextFrameRect { rect }
        | Geometry::Oval { rect }
        | Geometry::Polygon { bbox: rect, .. } => rect,
        Geometry::Line { .. } => return,
    };
    if !(bbox.w > 0.0 && bbox.h > 0.0) {
        return;
    }
    let stated_len = frame
        .gradient_fill_length
        .filter(|l| l.is_finite() && *l > 0.0);
    let ([sx, sy], len) = match frame.gradient_fill_start {
        Some([sx, sy]) if sx.is_finite() && sy.is_finite() => {
            ([sx, sy], stated_len.unwrap_or(bbox.w))
        }
        Some(_) => return,
        // An upright frame keeps the default the paint was built with.
        None => match rotated_linear_default(bbox, frame.item_transform) {
            Some((start, default_len)) => (start, stated_len.unwrap_or(default_len)),
            None => return,
        },
    };
    let (sin, cos) = frame
        .gradient_fill_angle
        .unwrap_or(0.0)
        .to_radians()
        .sin_cos();
    let (ex, ey) = (sx + len * cos, sy - len * sin);
    let Some(g) = page.list.gradients.get_mut(gid.0 as usize) else {
        return;
    };
    match frame.geometry {
        Geometry::Polygon { .. } => {
            g.start = (sx, sy);
            g.end = (ex, ey);
        }
        _ => {
            let unit = |x: f32, y: f32| ((x - bbox.x) / bbox.w, (y - bbox.y) / bbox.h);
            g.start = unit(sx, sy);
            g.end = unit(ex, ey);
        }
    }
}

/// Where InDesign starts a linear gradient on a ROTATED frame that states
/// no `GradientFillStart`, and how long it makes it: `(start, length)` in
/// the frame's inner coordinates, or `None` for a frame at 0°, which keeps
/// the upright default (`pipeline::color_paint::linear_gradient_endpoints`:
/// the inner box's bottom-left corner, the box's width).
///
/// Asked of InDesign 20.0.1 by opening frames turned every 15° and reading
/// back the start and length it writes (ADR 147). Only the rotation θ of
/// the item's transform counts — a scale, shear or flip leaves the answer
/// of the rotation alone (a horizontal flip is θ = 180°). With the box
/// turned by θ into its parent's axes:
///
/// * the length is the width of that turned box;
/// * at exactly 90°, 180° and 270° the start is a corner of the inner box:
///   top-right, bottom-right and bottom-left;
/// * strictly between 0° and 180° it is the left-centre of the turned box,
///   carried back into inner coordinates;
/// * strictly between 180° and 360° it is the inner corner the turn puts
///   furthest left.
///
/// The angle stays the frame's `GradientFillAngle`, in inner coordinates,
/// so the axis turns with the frame from that start.
fn rotated_linear_default(bbox: Rect, item_transform: Option<[f32; 6]>) -> Option<([f32; 2], f32)> {
    let [a, b, ..] = item_transform?;
    if a.hypot(b) < 1e-6 {
        return None;
    }
    let theta = (-b).atan2(a).to_degrees().rem_euclid(360.0);
    const EPS: f32 = 1e-3;
    let near = |t: f32| (theta - t).abs() < EPS;
    if near(0.0) || near(360.0) {
        return None;
    }
    let (sin, cos) = theta.to_radians().sin_cos();
    // Inner → parent, rotation only (an IDML transform maps the inner
    // y-down plane with `b = −sin θ`, `c = sin θ`).
    let turn = |x: f32, y: f32| (cos * x + sin * y, -sin * x + cos * y);
    let (x0, y0, x1, y1) = (bbox.x, bbox.y, bbox.x + bbox.w, bbox.y + bbox.h);
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    let turned = corners.map(|(x, y)| turn(x, y));
    let min_x = turned.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let max_x = turned.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let min_y = turned.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_y = turned.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    let length = max_x - min_x;
    let start = if near(90.0) {
        [x1, y0]
    } else if near(180.0) {
        [x1, y1]
    } else if near(270.0) {
        [x0, y1]
    } else if theta < 180.0 {
        let (px, py) = (min_x, (min_y + max_y) * 0.5);
        [cos * px - sin * py, sin * px + cos * py]
    } else {
        let i = (0..4)
            .min_by(|&i, &j| turned[i].0.total_cmp(&turned[j].0))
            .unwrap_or(0);
        [corners[i].0, corners[i].1]
    };
    Some((start, length))
}

/// An explicit `GradientFillStart` + `GradientFillLength` centres a
/// radial gradient at the start point (the frame's inner coordinates)
/// with the length as its radius, replacing the default placement. A
/// polygon's gradient is already in inner coordinates (see
/// [`rebase_radial_gradient_to_bbox`]); a rect's and an oval's are in
/// its unit rectangle, whose radius the rasteriser reads as the mean of
/// the two axes' — hence `2·R / (w + h)`.
fn place_radial_at_start(
    page: &mut BuiltPage,
    gid: paged_compose::GradientId,
    frame: &ResolvedFrame<'_>,
) {
    let (Some([sx, sy]), Some(len)) = (frame.gradient_fill_start, frame.gradient_fill_length)
    else {
        return;
    };
    if len.is_nan() || len <= 0.0 {
        return;
    }
    let Some(g) = page.list.radial_gradients.get_mut(gid.0 as usize) else {
        return;
    };
    match frame.geometry {
        Geometry::Polygon { .. } => {
            g.center = (sx, sy);
            g.radius = len;
        }
        Geometry::Rect { rect } | Geometry::TextFrameRect { rect } | Geometry::Oval { rect }
            if rect.w > 0.0 && rect.h > 0.0 =>
        {
            g.center = ((sx - rect.x) / rect.w, (sy - rect.y) / rect.h);
            g.radius = 2.0 * len / (rect.w + rect.h);
        }
        _ => {}
    }
}

/// Radial counterpart to [`rebase_gradient_to_bbox`]. Maps the
/// gradient's unit-rect `center` onto `bbox` and scales `radius` by
/// `min(w, h)` — a unit `0.5` radius then covers exactly the inscribed
/// circle, matching what `Transform::for_rect_in` produces for rect /
/// oval radial fills.
fn rebase_radial_gradient_to_bbox(
    page: &mut BuiltPage,
    gid: paged_compose::GradientId,
    bbox: Rect,
) {
    let idx = gid.0 as usize;
    if let Some(g) = page.list.radial_gradients.get_mut(idx) {
        g.center = (bbox.x + g.center.0 * bbox.w, bbox.y + g.center.1 * bbox.h);
        g.radius *= bbox.w.min(bbox.h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_compose::{DisplayList, GradientStop, RadialGradient};

    fn turned(theta: f32, sx: f32, sy: f32) -> [f32; 6] {
        let (sin, cos) = theta.to_radians().sin_cos();
        // R(θ)·diag(sx, sy), as an IDML ItemTransform.
        [cos * sx, -sin * sx, sin * sy, cos * sy, 0.0, 0.0]
    }

    /// Every frame of the recorded probe
    /// (`tools/indesign-export/gradient-defaults-probe.sh`, ADR 147): an
    /// 80 × 60 rectangle turned 0°, 15° … 255° with no gradient start, and
    /// the start and length InDesign 20.0.1 wrote back for it.
    #[test]
    fn a_rotated_frame_starts_its_gradient_where_indesign_does() {
        let probe: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tools/indesign-export/gradient-defaults-probe.json"
        ))
        .expect("probe recording parses");
        let frames = probe["frames"].as_array().expect("frames");
        assert_eq!(frames.len(), 18);
        let num = |v: &serde_json::Value| v.as_f64().expect("number") as f32;
        for f in frames {
            let theta = num(&f["rotation"]);
            let bbox = Rect {
                x: 0.0,
                y: 0.0,
                w: num(&f["box"][0]),
                h: num(&f["box"][1]),
            };
            let (ex, ey, el) = (num(&f["start"][0]), num(&f["start"][1]), num(&f["length"]));
            let ([x, y], len) = rotated_linear_default(bbox, Some(turned(theta, 1.0, 1.0)))
                // Upright: the existing default, (0, h) and the width.
                .unwrap_or(([0.0, bbox.h], bbox.w));
            assert!(
                (x - ex).abs() < 0.05 && (y - ey).abs() < 0.05 && (len - el).abs() < 0.05,
                "{theta}°: start ({x:.2}, {y:.2}) length {len:.2}, InDesign ({ex}, {ey}) {el}"
            );
        }
        let upright = Rect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 60.0,
        };
        assert_eq!(rotated_linear_default(upright, None), None);
    }

    /// Scale, shear and flip leave the rotation's answer alone: InDesign
    /// wrote the same start for 30° scaled 1.5 × 0.5 as for 30°, and a
    /// horizontal flip is 180°.
    #[test]
    fn only_the_rotation_counts() {
        let bbox = Rect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 60.0,
        };
        let plain = rotated_linear_default(bbox, Some(turned(30.0, 1.0, 1.0)));
        assert_eq!(
            rotated_linear_default(bbox, Some(turned(30.0, 1.5, 0.5))),
            plain
        );
        let ([x, y], len) =
            rotated_linear_default(bbox, Some([-1.0, 0.0, 0.0, 1.0, 0.0, 0.0])).unwrap();
        assert!(
            (x - 80.0).abs() < 1e-3 && (y - 60.0).abs() < 1e-3 && (len - 80.0).abs() < 1e-3,
            "flipped: start ({x}, {y}) length {len}"
        );
        // A vertical flip is still 0°.
        assert_eq!(
            rotated_linear_default(bbox, Some([1.0, 0.0, 0.0, -1.0, 0.0, 0.0])),
            None
        );
    }

    fn page_with(list: DisplayList) -> BuiltPage {
        BuiltPage {
            id: crate::pipeline::PageId::synthetic(0, 0),
            width_pt: 0.0,
            height_pt: 0.0,
            spread_origin: (0.0, 0.0),
            spread_transform: paged_compose::Transform::IDENTITY,
            list,
            layout_generation: 0,
            numbering_generation: 0,
            stats: Default::default(),
            story_layout: Vec::new(),
            footnotes: Vec::new(),
            diagnostics: Vec::new(),
            cell_rects: Vec::new(),
            resource_tiles_needed: Vec::new(),
            scene_layer_frames: Vec::new(),
        }
    }

    #[test]
    fn radial_gradient_rebases_center_and_radius_to_polygon_bbox() {
        let mut list = DisplayList::new();
        let gid = list.push_radial_gradient(RadialGradient {
            center: (0.5, 0.5),
            radius: 0.5,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: paged_compose::Color::WHITE,
                },
                GradientStop {
                    offset: 1.0,
                    color: paged_compose::Color::BLACK,
                },
            ],
        });
        let mut page = page_with(list);
        let bbox = Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 200.0,
        };

        rebase_radial_gradient_to_bbox(&mut page, gid, bbox);

        let g = page.list.radial_gradient(gid).expect("gradient survives");
        // bbox center: (10 + 100/2, 20 + 200/2) = (60, 120).
        assert_eq!(g.center, (60.0, 120.0));
        // Inscribed-circle radius: 0.5 * min(w, h) = 0.5 * 100 = 50.
        assert_eq!(g.radius, 50.0);
    }
}
