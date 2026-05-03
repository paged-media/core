//! Fill-paint module.
//!
//! Resolves the frame's fill from `FillColor` (with gradient angle,
//! tint, and opacity baked in) and emits the geometry-appropriate
//! primitive through [`super::geometry::emit_filled`]. Skipped when
//! the fill is transparent.

use idml_compose::{BlendMode, Paint, PathId, Transform};
use idml_parse::Graphic;

use super::geometry::emit_filled;
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
                path_dims,
            )
        })
        .unwrap_or(fallback);
    let fill = apply_fill_tint(fill, frame.fill_tint);
    emit_filled(&frame.geometry, page, fill, BlendMode::Normal, outer, fill_path);
}
