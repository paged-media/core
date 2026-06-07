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
//! Rectangles, text frames, and ovals stamp the bbox rect; ovals use
//! it as a stopgap until an elliptical stamp lands. W1.1: Polygons
//! (and pathed Rectangles / TextFrames lifted to `Geometry::Polygon`)
//! cast a *path-shaped* shadow — the frame's real outline is interned
//! and emitted as a `DropShadow { path_id, .. }` (σ-scale 1.0, the
//! frame-body blur, distinct from the wider glyph-shadow `PathShadow`),
//! so a triangle / Bezier frame's shadow hugs the shape rather than
//! its bounding box. Lines still emit no shadow.

use paged_compose::{
    emit_drop_shadow_rect_transformed, DisplayCommand, DropShadow, PathId, Rect, Transform,
};
use paged_parse::{DropShadowSetting, Graphic};

use super::{Geometry, ResolvedFrame};
use crate::pipeline::{
    fnv_1a_u64, frame_fill_is_transparent, frame_stroke_is_visible, path_signature,
    polygon_path_from_anchors_with_open, resolve_frame_shadow, BuiltPage,
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
    cmyk_xform: Option<&paged_color::IccTransform>,
    fallback: Option<DropShadow>,
    outer: Transform,
    stroke_drop_shadow: Option<&DropShadowSetting>,
) {
    // Resolve the shape the shadow stamps under: an axis-aligned rect
    // for Rect / TextFrameRect / Oval, or the frame's real outline
    // (interned once, reused by both fill and stroke shadows) for a
    // pathed Polygon. Lines cast no shadow.
    let target: ShadowTarget = match &frame.geometry {
        Geometry::Rect { rect } | Geometry::TextFrameRect { rect } | Geometry::Oval { rect } => {
            ShadowTarget::Rect(*rect)
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

    // Fill shadow — gated on a visible fill so the stamp doesn't
    // leak a solid backdrop through a transparent frame.
    if !frame_fill_is_transparent(frame.fill_color) {
        if let Some(shadow) = resolve_frame_shadow(frame.drop_shadow, fallback, palette, cmyk_xform)
        {
            emit_shadow(target, outer, shadow, page);
        }
    }

    // Stroke shadow — only when the stroke is actually visible.
    // Resolving via `resolve_frame_shadow(..., None, ...)` so the
    // synthetic fallback only ever supplies the *fill* shadow.
    if frame_stroke_is_visible(frame.stroke_color, frame.effective_stroke_weight()) {
        if let Some(shadow) = resolve_frame_shadow(stroke_drop_shadow, None, palette, cmyk_xform) {
            emit_shadow(target, outer, shadow, page);
        }
    }
}

/// What a frame's drop shadow stamps under: an axis-aligned bounding
/// rect (the common rectangle / text-frame / oval stopgap) or the
/// frame's interned real outline (pathed Polygon).
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
