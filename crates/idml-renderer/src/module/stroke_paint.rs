//! Stroke-paint module.
//!
//! Resolves the frame's stroke from `StrokeColor` (with opacity,
//! alignment, dash pattern, end-cap / end-join all baked in) and
//! emits through [`super::geometry::emit_stroked`]. Skipped when no
//! stroke colour resolves or `StrokeWeight` is non-positive.
//!
//! GraphicLines are not routed through this module — they emit
//! directly in `pipeline::emit_line_into` because their endpoint
//! math (transform_bounds + spread origin) doesn't match the
//! geometry adapter's unit-rect convention.

use idml_compose::{PathId, Stroke, Transform};
use idml_parse::Graphic;

use super::geometry::emit_stroked;
use super::ResolvedFrame;
use crate::pipeline::{apply_opacity, color_id_to_paint_with_list, BuiltPage};

/// Resolve and emit the frame stroke. `stroke_path`, when `Some`,
/// routes through `StrokePath` against the pre-interned offset path
/// (rounded Rectangle with stroke alignment) or the polygon path.
pub(crate) fn stroke_paint_module(
    frame: &ResolvedFrame<'_>,
    page: &mut BuiltPage,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    outer: Transform,
    stroke_path: Option<PathId>,
    stroke: Stroke,
) {
    if frame.stroke_weight <= 0.0 {
        return;
    }
    let Some(paint) = frame
        .stroke_color
        .and_then(|id| color_id_to_paint_with_list(id, palette, cmyk_xform, &mut page.list))
    else {
        return;
    };
    let paint = apply_opacity(paint, frame.opacity);
    emit_stroked(&frame.geometry, page, paint, stroke, outer, stroke_path);
}
