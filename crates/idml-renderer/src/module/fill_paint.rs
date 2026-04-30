//! Fill-paint module.
//!
//! Resolves the frame's fill from `FillColor` (with gradient angle,
//! tint, and opacity baked in) and emits the geometry-appropriate
//! primitive through [`super::geometry::emit_filled`]. Skipped when
//! the fill is transparent.

use idml_compose::{Paint, PathId, Transform};
use idml_parse::Graphic;

use super::geometry::emit_filled;
use super::ResolvedFrame;
use crate::pipeline::{
    apply_fill_tint, apply_opacity, color_id_to_paint_with_list_dir, frame_fill_is_transparent,
    BuiltPage,
};

/// Resolve and emit the frame fill. `fill_path`, when `Some`, routes
/// the emit through `FillPath{Blend}` against the pre-interned path
/// (rounded Rectangle / Polygon).
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
    let fill = frame
        .fill_color
        .and_then(|id| {
            color_id_to_paint_with_list_dir(
                id,
                palette,
                cmyk_xform,
                &mut page.list,
                frame.gradient_fill_angle,
            )
        })
        .unwrap_or(fallback);
    let fill = apply_fill_tint(fill, frame.fill_tint);
    let fill = apply_opacity(fill, frame.opacity);
    emit_filled(&frame.geometry, page, fill, frame.blend_mode, outer, fill_path);
}
