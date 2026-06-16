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

//! Frame transparency-group bracketing + drop-shadow resolution —
//! extracted from pipeline/mod.rs (1.6b). Net-zero behaviour.

use super::*;

use paged_compose::{Color, DropShadow, Transform};
use paged_parse::Graphic;

use crate::module::ResolvedFrame;

/// Decide whether `frame` needs a transparency-group bracket: any
/// non-Normal blend mode, or any opacity strictly less than 100%.
/// Normal + 100% opacity is the fast path that draws straight onto
/// the page.
pub(crate) fn frame_needs_blend_group(frame: &ResolvedFrame<'_>) -> bool {
    if !matches!(frame.blend_mode, paged_compose::BlendMode::Normal) {
        return true;
    }
    matches!(frame.opacity, Some(o) if o < 100.0 - f32::EPSILON)
}

/// Group opacity normalised to 0..=1. Defaults to 1.0 when no opacity
/// override is present on the frame.
pub(crate) fn frame_group_opacity(frame: &ResolvedFrame<'_>) -> f32 {
    frame
        .opacity
        .map(|p| (p / 100.0).clamp(0.0, 1.0))
        .unwrap_or(1.0)
}

/// Push a `BeginBlendGroup` covering `geometry_bounds × outer` (axis-
/// aligned in page coords, padded slightly so AA edges stay inside the
/// buffer). Returns the bounds the matching `EndBlendGroup` will use,
/// for callers that want to bracket multiple ranges of commands with
/// the same group buffer.
pub(crate) fn push_blend_group(
    page: &mut BuiltPage,
    bounds_in_inner: paged_compose::Rect,
    outer: Transform,
    blend_mode: paged_compose::BlendMode,
    opacity: f32,
) -> paged_compose::Rect {
    let bounds = rect_bounds_in_page(bounds_in_inner, outer);
    // Pad by 0.5pt so glyph anti-aliasing at the edges of the
    // text-frame bbox still falls inside the buffer.
    let padded = paged_compose::Rect {
        x: bounds.x - 0.5,
        y: bounds.y - 0.5,
        w: bounds.w + 1.0,
        h: bounds.h + 1.0,
    };
    page.list
        .commands
        .push(paged_compose::DisplayCommand::BeginBlendGroup {
            bounds: padded,
            blend_mode,
            opacity,
            transform: Transform::IDENTITY,
        });
    padded
}

/// Push the matching `EndBlendGroup` for [`push_blend_group`].
pub(crate) fn pop_blend_group(page: &mut BuiltPage) {
    page.list
        .commands
        .push(paged_compose::DisplayCommand::EndBlendGroup(
            Transform::IDENTITY,
        ));
}

/// Resolve the effective shadow for a frame. Per-frame IDML shadow
/// wins; the synthetic `fallback` (from `PipelineOptions`) is used
/// when the frame carries none. Returns `None` for fully-transparent
/// shadows so callers don't emit a no-op.
pub(crate) fn resolve_frame_shadow(
    frame_shadow: Option<&paged_parse::DropShadowSetting>,
    fallback: Option<DropShadow>,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
) -> Option<DropShadow> {
    frame_shadow
        .and_then(|s| convert_setting_to_shadow(s, palette, cmyk_xform))
        .or(fallback)
}

/// Convert an IDML `<DropShadowSetting>` to a compose-layer `DropShadow`.
/// The parser already drops `Mode="None"` settings, so we only have
/// to filter out fully-transparent shadows here.
pub(super) fn convert_setting_to_shadow(
    setting: &paged_parse::DropShadowSetting,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
) -> Option<DropShadow> {
    let opacity = (setting.opacity_pct / 100.0).clamp(0.0, 1.0);
    if opacity == 0.0 {
        return None;
    }
    let color = setting
        .effect_color
        .as_deref()
        .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
        .and_then(|p| paint_as_solid_with_icc(p, cmyk_xform))
        .unwrap_or(Color::BLACK);
    Some(DropShadow {
        offset_x: setting.x_offset,
        offset_y: setting.y_offset,
        blur_radius: setting.size,
        color,
        opacity,
    })
}
