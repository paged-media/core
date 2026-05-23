//! Fill-paint module.
//!
//! Resolves the frame's fill from `FillColor` (with gradient angle,
//! tint, and opacity baked in) and emits the geometry-appropriate
//! primitive through [`super::geometry::emit_filled`]. Skipped when
//! the fill is transparent.

use idml_compose::{BlendMode, Paint, PathId, Rect, Transform};
use idml_parse::Graphic;

use super::geometry::{emit_filled, rewrite_tail_for_overprint};
use super::{Geometry, ResolvedFrame};
use crate::pipeline::{
    apply_fill_tint, color_id_to_paint_with_list_dir, frame_fill_is_transparent, BuiltPage,
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
    cmyk_xform: Option<&idml_color::IccTransform>,
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
                cmyk_xform,
                &mut page.list,
                frame.gradient_fill_angle,
                frame.gradient_fill_length,
                path_dims,
            )
        })
        .unwrap_or(fallback);
    let fill = apply_fill_tint(fill, frame.fill_tint);
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
    let start = page.list.commands.len();
    emit_filled(&frame.geometry, page, fill, BlendMode::Normal, outer, fill_path);
    rewrite_tail_for_overprint(page, start, frame.overprint_fill, false);
}

/// Map the linear gradient's unit-rect endpoints onto `bbox` in
/// place. Used for Polygon fills whose `path_transform` is `outer`
/// directly (no `for_rect_in` step). After this rewrite the
/// rasterizer's `outer.apply(...)` lands endpoints inside the
/// polygon's inner-coord bbox, matching what rect / oval gradients
/// produce via `Transform::for_rect_in`.
fn rebase_gradient_to_bbox(page: &mut BuiltPage, gid: idml_compose::GradientId, bbox: Rect) {
    let idx = gid.0 as usize;
    if let Some(g) = page.list.gradients.get_mut(idx) {
        g.start = (bbox.x + g.start.0 * bbox.w, bbox.y + g.start.1 * bbox.h);
        g.end = (bbox.x + g.end.0 * bbox.w, bbox.y + g.end.1 * bbox.h);
    }
}

/// Radial counterpart to [`rebase_gradient_to_bbox`]. Maps the
/// gradient's unit-rect `center` onto `bbox` and scales `radius` by
/// `min(w, h)` — a unit `0.5` radius then covers exactly the inscribed
/// circle, matching what `Transform::for_rect_in` produces for rect /
/// oval radial fills.
fn rebase_radial_gradient_to_bbox(
    page: &mut BuiltPage,
    gid: idml_compose::GradientId,
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
    use idml_compose::{DisplayList, GradientStop, RadialGradient};

    fn page_with(list: DisplayList) -> BuiltPage {
        BuiltPage {
            id: crate::pipeline::PageId::synthetic(0, 0),
            width_pt: 0.0,
            height_pt: 0.0,
            spread_origin: (0.0, 0.0),
            list,
            layout_generation: 0,
            numbering_generation: 0,
            stats: Default::default(),
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
                    color: idml_compose::Color::WHITE,
                },
                GradientStop {
                    offset: 1.0,
                    color: idml_compose::Color::BLACK,
                },
            ],
        });
        let mut page = page_with(list);
        let bbox = Rect { x: 10.0, y: 20.0, w: 100.0, h: 200.0 };

        rebase_radial_gradient_to_bbox(&mut page, gid, bbox);

        let g = page.list.radial_gradient(gid).expect("gradient survives");
        // bbox center: (10 + 100/2, 20 + 200/2) = (60, 120).
        assert_eq!(g.center, (60.0, 120.0));
        // Inscribed-circle radius: 0.5 * min(w, h) = 0.5 * 100 = 50.
        assert_eq!(g.radius, 50.0);
    }
}
