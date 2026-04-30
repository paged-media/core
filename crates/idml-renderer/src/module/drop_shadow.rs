//! Drop-shadow module.
//!
//! Resolves a frame's `<DropShadowSetting>` (or the document-wide
//! fallback from `PipelineOptions::frame_drop_shadow`) into a
//! [`DropShadow`] paint and emits the rectangular stamp behind the
//! frame's bounding rect. Skipped when the frame's fill is
//! transparent — InDesign casts no shadow off a `Swatch/None` fill,
//! and emitting the rect-stamp anyway leaks a solid backdrop through
//! the otherwise invisible frame (see commit 9f98738 / 2c33465).
//!
//! Shadows currently only render against an axis-aligned bounding
//! rect — Ovals are stamped with the bbox as a stopgap and Polygons
//! / Lines emit no shadow at all. Replace the rect-stamp emit with a
//! geometry-shaped stamp once the rasterizer grows path-shaped
//! shadow support; the module's interface won't change.

use idml_compose::{emit_drop_shadow_rect_transformed, DropShadow, Transform};
use idml_parse::Graphic;

use super::{Geometry, ResolvedFrame};
use crate::pipeline::{frame_fill_is_transparent, resolve_frame_shadow, BuiltPage};

/// Emit the drop-shadow stamp for a frame. Returns silently when the
/// frame has no shadow setting, when the resolved fallback is
/// `None`, or when the fill is transparent.
pub(crate) fn drop_shadow_module(
    frame: &ResolvedFrame<'_>,
    page: &mut BuiltPage,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    fallback: Option<DropShadow>,
    outer: Transform,
) {
    if frame_fill_is_transparent(frame.fill_color) {
        return;
    }
    let Some(shadow) = resolve_frame_shadow(frame.drop_shadow, fallback, palette, cmyk_xform)
    else {
        return;
    };
    // The compose layer only carries a rect-shaped shadow primitive
    // today; pick the bounding rect off the geometry.
    let rect = match &frame.geometry {
        Geometry::Rect { rect } | Geometry::TextFrameRect { rect } | Geometry::Oval { rect } => {
            *rect
        }
        Geometry::Polygon { bbox, .. } => *bbox,
        Geometry::Line { .. } => return,
    };
    emit_drop_shadow_rect_transformed(rect, outer, shadow, &mut page.list);
}
