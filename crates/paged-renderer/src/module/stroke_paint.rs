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

use paged_compose::{PathId, Stroke, Transform};
use paged_model::Graphic;

use super::geometry::{emit_stroked, rewrite_tail_for_overprint};
use super::{Geometry, ResolvedFrame};
use crate::pipeline::{color_id_to_paint_with_list_dir, BuiltPage};

/// Resolve and emit the frame stroke. `stroke_path`, when `Some`,
/// routes through `StrokePath` against the pre-interned offset path
/// (rounded Rectangle with stroke alignment) or the polygon path.
///
/// Frame opacity is applied at the transparency-group level by the
/// orchestrator (the body+glyphs are bracketed in
/// `BeginBlendGroup` / `EndBlendGroup` when non-trivial). Stroke
/// emission therefore skips per-paint opacity scaling — the group
/// composite handles it.
pub(crate) fn stroke_paint_module(
    frame: &ResolvedFrame<'_>,
    page: &mut BuiltPage,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
    outer: Transform,
    stroke_path: Option<PathId>,
    stroke: Stroke,
) {
    if frame.effective_stroke_weight() <= 0.0 {
        return;
    }
    // Bbox dims for gradient defaults; mirrors `fill_paint_module`.
    // Lines have no rect bbox so the dims fall through to `None` —
    // the stroke gradient's unit-rect default still serviceable.
    let path_dims = match frame.geometry {
        Geometry::Rect { rect }
        | Geometry::TextFrameRect { rect }
        | Geometry::Oval { rect }
        | Geometry::Polygon { bbox: rect, .. } => Some((rect.w, rect.h)),
        Geometry::Line { .. } => None,
    };
    let Some(paint) = frame.stroke_color.and_then(|id| {
        color_id_to_paint_with_list_dir(
            id,
            palette,
            cmyk_xform,
            &mut page.list,
            frame.gradient_stroke_angle,
            frame.gradient_stroke_length,
            path_dims,
        )
    }) else {
        return;
    };
    let start = page.list.commands.len();
    emit_stroked(&frame.geometry, page, paint, stroke, outer, stroke_path);
    rewrite_tail_for_overprint(page, start, false, frame.overprint_stroke);
}
