//! Drop-shadow module.
//!
//! Resolves a frame's `<DropShadowSetting>` (or the document-wide
//! fallback from `PipelineOptions::frame_drop_shadow`) into a
//! [`DropShadow`] paint and emits the rectangular stamp behind the
//! frame's bounding rect. The fill-shadow is skipped when the
//! frame's fill is transparent — InDesign casts no shadow off a
//! `Swatch/None` fill, and emitting the rect-stamp anyway leaks a
//! solid backdrop through the otherwise invisible frame (see commit
//! 9f98738 / 2c33465).
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
//! Shadows currently only render against an axis-aligned bounding
//! rect — Ovals are stamped with the bbox as a stopgap and Polygons
//! / Lines emit no shadow at all. Replace the rect-stamp emit with a
//! geometry-shaped stamp once the rasterizer grows path-shaped
//! shadow support; the module's interface won't change.

use idml_compose::{emit_drop_shadow_rect_transformed, DropShadow, Rect, Transform};
use idml_parse::{DropShadowSetting, Graphic};

use super::{Geometry, ResolvedFrame};
use crate::pipeline::{
    frame_fill_is_transparent, frame_stroke_is_visible, resolve_frame_shadow, BuiltPage,
};

/// Emit the drop-shadow stamp(s) for a frame. The fill-shadow stamps
/// when the frame has a visible fill; the stroke-shadow stamps when
/// the frame has a visible stroke (`StrokeColor != Swatch/None` AND
/// `StrokeWeight > 0`). Both stamps share the frame's bounding rect
/// today; emitting two when both are visible isn't typical IDML
/// content, so we keep the geometry simple.
pub(crate) fn drop_shadow_module(
    frame: &ResolvedFrame<'_>,
    page: &mut BuiltPage,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    fallback: Option<DropShadow>,
    outer: Transform,
    stroke_drop_shadow: Option<&DropShadowSetting>,
) {
    let rect = match &frame.geometry {
        Geometry::Rect { rect } | Geometry::TextFrameRect { rect } | Geometry::Oval { rect } => {
            Some(*rect)
        }
        Geometry::Polygon { bbox, .. } => Some(*bbox),
        Geometry::Line { .. } => None,
    };
    let Some(rect) = rect else {
        return;
    };

    // Fill shadow — gated on a visible fill so the stamp doesn't
    // leak a solid backdrop through a transparent frame.
    if !frame_fill_is_transparent(frame.fill_color) {
        if let Some(shadow) =
            resolve_frame_shadow(frame.drop_shadow, fallback, palette, cmyk_xform)
        {
            emit_shadow_rect(rect, outer, shadow, page);
        }
    }

    // Stroke shadow — only when the stroke is actually visible.
    // Resolving via `resolve_frame_shadow(..., None, ...)` so the
    // synthetic fallback only ever supplies the *fill* shadow.
    if frame_stroke_is_visible(frame.stroke_color, frame.effective_stroke_weight()) {
        if let Some(shadow) =
            resolve_frame_shadow(stroke_drop_shadow, None, palette, cmyk_xform)
        {
            emit_shadow_rect(rect, outer, shadow, page);
        }
    }
}

fn emit_shadow_rect(rect: Rect, outer: Transform, shadow: DropShadow, page: &mut BuiltPage) {
    emit_drop_shadow_rect_transformed(rect, outer, shadow, &mut page.list);
}
