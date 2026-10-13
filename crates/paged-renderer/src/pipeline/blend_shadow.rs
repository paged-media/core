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
use paged_model::Graphic;

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
///
/// The geometry rect is a STARTING POINT, not the final extent. It is
/// what this emitter knows; what actually lands in the group also
/// includes the drop shadow emitted immediately below (offset + 3σ),
/// the stroke's outer half-weight, and any outer glow / feather. Since
/// `bounds` is a clip in both back ends, all of that would be cut here.
/// `paged_compose::fit_transparency_group_bounds`, run once on the
/// finished page list at the end of `build_document_inner`, grows the
/// rect to cover it.
pub(crate) fn push_blend_group(
    page: &mut BuiltPage,
    bounds_in_inner: paged_compose::Rect,
    outer: Transform,
    blend_mode: paged_compose::BlendMode,
    opacity: f32,
) -> paged_compose::Rect {
    let bounds = rect_bounds_in_page(bounds_in_inner, outer);
    // Pad by 0.5pt so glyph anti-aliasing at the edges of the
    // text-frame bbox still falls inside the buffer. The fit pass keeps
    // the same headroom (`paged_compose::GROUP_AA_PAD_PT`), so a group
    // whose contents match its geometry ends up with exactly this rect.
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
            isolated: false,
            knockout: false,
            transform: Transform::IDENTITY,
        });
    padded
}

/// R-10 — open a transparency TARGET's group (the fill's, the
/// stroke's, the content's) when it composites differently from an
/// opaque `Normal` paint. Everything the target paints — its own shadow
/// included, which its opacity fades with it (asked of InDesign,
/// `transparency-targets` case 40) — goes inside; the object's group,
/// when there is one, encloses it. Returns whether a group was pushed,
/// for the matching [`pop_blend_group`].
pub(crate) fn push_target_group(
    page: &mut BuiltPage,
    target: Option<&paged_model::TargetTransparency>,
    bounds_in_inner: paged_compose::Rect,
    outer: Transform,
) -> bool {
    let Some(t) = target.filter(|t| t.is_active()) else {
        return false;
    };
    push_blend_group(
        page,
        bounds_in_inner,
        outer,
        blend_mode_from_idml(t.blend_mode.as_deref()),
        target_opacity(Some(t)),
    );
    true
}

/// A target's opacity, 0..=1; 1 for a missing target.
pub(crate) fn target_opacity(target: Option<&paged_model::TargetTransparency>) -> f32 {
    target
        .and_then(|t| t.opacity)
        .map(|p| (p / 100.0).clamp(0.0, 1.0))
        .unwrap_or(1.0)
}

/// R-11 — the blend mode a shadow's stamp needs a group for: its own,
/// unless that changes nothing (`Normal`, or a black `Multiply` shadow,
/// which multiplies exactly as it paints).
pub(crate) fn shadow_group_blend(shadow: &DropShadow) -> Option<paged_compose::BlendMode> {
    use paged_compose::BlendMode as B;
    let black = shadow.color.r <= 0.0 && shadow.color.g <= 0.0 && shadow.color.b <= 0.0;
    match shadow.blend_mode {
        B::Normal => None,
        B::Multiply | B::Darken if black => None,
        mode => Some(mode),
    }
}

/// R-11 — emit a shadow stamp, bracketed in a group of the shadow's
/// blend mode when that mode changes the result. The group's
/// paper-backdrop rule is what keeps a `Screen` shadow visible on paper
/// (asked of InDesign, `effects-attrs` case 3). The group's bounds are
/// the page; the fit pass at the end of the build sizes it.
pub(crate) fn emit_blended_shadow(
    page: &mut BuiltPage,
    shadow: &DropShadow,
    emit: impl FnOnce(&mut BuiltPage),
) {
    let Some(mode) = shadow_group_blend(shadow) else {
        emit(page);
        return;
    };
    let bounds = paged_compose::Rect {
        x: 0.0,
        y: 0.0,
        w: page.width_pt,
        h: page.height_pt,
    };
    page.list
        .commands
        .push(paged_compose::DisplayCommand::BeginBlendGroup {
            bounds,
            blend_mode: mode,
            opacity: 1.0,
            isolated: false,
            knockout: false,
            transform: Transform::IDENTITY,
        });
    emit(page);
    pop_blend_group(page);
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
    frame_shadow: Option<&paged_model::DropShadowSetting>,
    fallback: Option<DropShadow>,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
) -> Option<DropShadow> {
    frame_shadow
        .and_then(|s| convert_setting_to_shadow(s, palette, color_ctx))
        .or(fallback)
}

/// Convert an IDML `<DropShadowSetting>` to a compose-layer `DropShadow`.
/// The parser already drops `Mode="None"` settings, so we only have
/// to filter out fully-transparent shadows here.
pub(super) fn convert_setting_to_shadow(
    setting: &paged_model::DropShadowSetting,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
) -> Option<DropShadow> {
    let opacity = (setting.opacity_pct / 100.0).clamp(0.0, 1.0);
    if opacity == 0.0 {
        return None;
    }
    let color = setting
        .effect_color
        .as_deref()
        .and_then(|id| color_id_to_paint(id, palette, color_ctx))
        .and_then(|p| paint_as_solid_with_icc(p, color_ctx))
        .unwrap_or(Color::BLACK);
    // R-11: under the document's light the shadow keeps its distance
    // and turns to the light's angle (asked of InDesign, `effects-attrs`
    // cases 13–15).
    let (offset_x, offset_y) = setting.effective_offset(color_ctx.global_light_angle);
    Some(DropShadow {
        offset_x,
        offset_y,
        blur_radius: setting.size,
        color,
        opacity,
        // InDesign's default is `Multiply` (cases 2 and 4 paint alike).
        blend_mode: blend_mode_from_idml(Some(setting.blend_mode.as_deref().unwrap_or("Multiply"))),
        spread: setting
            .spread_pct
            .map(|s| (s / 100.0).clamp(0.0, 0.99))
            .unwrap_or(0.0),
        noise: setting
            .noise_pct
            .map(|n| (n / 100.0).clamp(0.0, 1.0))
            .unwrap_or(0.0),
    })
}
