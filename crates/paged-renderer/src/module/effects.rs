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

//! Frame-effects module.
//!
//! Translates the parser's `<*Setting>` bag (`FrameEffects`) into the
//! compose layer's typed effect parameters and emits one
//! `DisplayCommand::*` per applied effect. The CPU rasterizer already
//! understands every `DisplayCommand::InnerShadow` / `OuterGlow` /
//! `InnerGlow` / `BevelEmboss` / `Satin` / `Feather` variant; this
//! module is the missing parser → compose bridge.
//!
//! Rendering order matches Photoshop / InDesign's layer-effect stack:
//! `OuterGlow` first (the halo lands behind the fill), then
//! `InnerShadow` / `InnerGlow` / `BevelEmboss` / `Satin` / `Feather`
//! after the fill (they composite onto the path's interior). The fill
//! is emitted by `fill_paint_module` between these two groups — the
//! caller bookends the call accordingly.
//!
//! `directional_feather` / `gradient_feather` go through their own
//! parser-to-compose converters (`directional_feather_from_parser` /
//! `gradient_feather_from_parser`) and emit dedicated
//! `DisplayCommand::DirectionalFeather` / `GradientFeather` variants
//! after the fill, alongside the plain `Feather` arm.
//!
//! All four shape kinds (Rectangle / Oval / Polygon / TextFrame)
//! capture the effects bag (parser Q-04) and the pipeline wires this
//! module from each one's emit path, so an IDML `<*Setting>` declared
//! on any of them reaches the rasterizer.

use paged_compose::{
    BevelDirection, BevelEmboss as ComposeBevelEmboss, BevelStyle, BevelTechnique, BlendMode,
    Color, DirectionalFeather as ComposeDirectionalFeather, DisplayCommand,
    Feather as ComposeFeather, FeatherCornerType, GradientFeather as ComposeGradientFeather,
    GradientFeatherKind, GradientFeatherStop as ComposeGradientFeatherStop,
    InnerGlow as ComposeInnerGlow, InnerShadow as ComposeInnerShadow,
    OuterGlow as ComposeOuterGlow, Paint, PathId, Rect, Satin as ComposeSatin, Transform,
};
use paged_model::{
    BevelEmbossParams, DirectionalFeatherParams, FeatherParams, FrameEffects,
    GradientFeatherParams, Graphic, InnerGlowParams, InnerShadowParams, OuterGlowParams,
    SatinParams,
};

use crate::pipeline::{blend_mode_from_idml, color_id_to_paint_with_list, BuiltPage, ColorCtx};

/// Default opacity for shadow/glow/satin effects (75%) — matches
/// InDesign's slider default. Used when the IDML omits `Opacity`.
const DEFAULT_OPACITY: f32 = 0.75;
/// Default blur radius (5pt) for shadow/glow/satin effects. Used when
/// the IDML omits `Size`.
const DEFAULT_BLUR_RADIUS: f32 = 5.0;
/// Default feather width (5pt). Used when the IDML omits `Width`.
const DEFAULT_FEATHER_WIDTH: f32 = 5.0;

/// Emit one `DisplayCommand::*` per applied effect onto `page.list`.
/// `fill_path_id` is the rectangle's fill path (the rounded path from
/// `corner_path_module` for rounded rects; `unit_rect_path_id` for flat
/// ones). `transform` maps that path into page coords — for the unit
/// rect that's `Transform::for_rect_in(rect, outer)`; for the rounded
/// path it's `outer` directly (the path is pre-baked in inner coords).
///
/// Caller convention: emit OuterGlow *before* the fill, the rest *after*
/// the fill. We do not enforce the order here — the caller passes us
/// either of the two helpers below depending on where in the emit
/// sequence it sits.
pub(crate) fn emit_effects_pre_fill(
    page: &mut BuiltPage,
    effects: &FrameEffects,
    fill_path_id: PathId,
    transform: Transform,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
) {
    if let Some(p) = effects.outer_glow.as_ref() {
        let params = outer_glow_from_parser(p, palette, color_ctx, &mut page.list);
        page.list.commands.push(DisplayCommand::OuterGlow {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
}

/// See [`emit_effects_pre_fill`]. Emits the effects that composite
/// *after* the fill: InnerShadow, InnerGlow, BevelEmboss, Satin,
/// Feather. Order mirrors Photoshop's layer-effect stack.
/// `unit_normalize` carries the path-local rect for the frame when
/// `transform` is the "unit-rect → page" composition
/// (`Transform::for_rect_in`). The path under it is the unit rect at
/// `[0,0,1,1]`, so IDML effect coordinates (which live in path-local
/// coords) need to be normalised by this rect before being passed to
/// the rasterizer. Pass `None` when the path is already in path-local
/// coords (rounded-corner path) — coordinates flow through unmodified.
pub(crate) fn emit_effects_post_fill(
    page: &mut BuiltPage,
    effects: &FrameEffects,
    fill_path_id: PathId,
    transform: Transform,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    unit_normalize: Option<Rect>,
) {
    if let Some(p) = effects.inner_shadow.as_ref() {
        let params = inner_shadow_from_parser(p, palette, color_ctx, &mut page.list);
        page.list.commands.push(DisplayCommand::InnerShadow {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
    if let Some(p) = effects.inner_glow.as_ref() {
        let params = inner_glow_from_parser(p, palette, color_ctx, &mut page.list);
        page.list.commands.push(DisplayCommand::InnerGlow {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
    if let Some(p) = effects.bevel.as_ref() {
        let params = bevel_emboss_from_parser(p, palette, color_ctx, &mut page.list);
        page.list.commands.push(DisplayCommand::BevelEmboss {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
    if let Some(p) = effects.satin.as_ref() {
        let params = satin_from_parser(p, palette, color_ctx, &mut page.list);
        page.list.commands.push(DisplayCommand::Satin {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
    if let Some(p) = effects.feather.as_ref() {
        let params = feather_from_parser(p);
        page.list.commands.push(DisplayCommand::Feather {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
    if let Some(p) = effects.directional_feather.as_ref() {
        let params = directional_feather_from_parser(p);
        page.list.commands.push(DisplayCommand::DirectionalFeather {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
    if let Some(p) = effects.gradient_feather.as_ref() {
        // The CPU rasterizer blends the underlying fill toward the
        // page background (see `render_gradient_feather`), so a
        // fading stop list (e.g. 100% → 0%) correctly fades the rect
        // out toward paper rather than to a dark tint. Emit
        // unconditionally — non-fading stop lists become a no-op
        // (factor = 1 leaves pixels untouched).
        let params = gradient_feather_from_parser(p, unit_normalize);
        page.list.commands.push(DisplayCommand::GradientFeather {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
}

/// Resolve a parser color id (e.g. `"Color/Black"`) into a compose
/// `Color`, defaulting to opaque black when the id is absent or
/// unresolvable. Gradient swatches collapse to black — IDML's effect
/// settings only ever reference solid swatches in practice.
fn resolve_effect_color(
    id: Option<&str>,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    list: &mut paged_compose::DisplayList,
) -> Color {
    resolve_effect_color_or(id, Color::BLACK, palette, color_ctx, list)
}

/// As [`resolve_effect_color`] but with a caller-supplied fallback for
/// the `None` / unresolvable case. FINDING #7.4 — glows default to
/// **white**, not black: InDesign's default glow `EffectColor` is a
/// light tint and the default blend is `Screen`, under which a *black*
/// glow is a no-op (`screen(base, 0) = base`) and paints zero pixels.
/// Shadows keep the black default (multiply-ish look).
fn resolve_effect_color_or(
    id: Option<&str>,
    fallback: Color,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    list: &mut paged_compose::DisplayList,
) -> Color {
    let Some(id) = id else {
        return fallback;
    };
    match color_id_to_paint_with_list(id, palette, color_ctx, list) {
        Some(Paint::Solid(c)) => c,
        Some(Paint::Cmyk { rgb, .. }) => rgb,
        _ => fallback,
    }
}

/// Map a 0..=100 percentage to 0..=1, clamped. `None` returns the
/// supplied default. Used for opacity / choke / spread / depth.
fn pct_to_unit(pct: Option<f32>, default: f32) -> f32 {
    pct.map(|p| (p / 100.0).clamp(0.0, 1.0)).unwrap_or(default)
}

/// Map a percentage that is allowed to exceed 100 % to its multiple,
/// clamped at InDesign's own ceiling. `Depth` is the one such knob:
/// its slider runs to 1000 %, and passing it through [`pct_to_unit`]
/// silently pinned every strong bevel — the annual's 120 % included —
/// back to 1.0.
fn pct_to_scale(pct: Option<f32>, default: f32) -> f32 {
    pct.map(|p| (p / 100.0).clamp(0.0, 10.0)).unwrap_or(default)
}

/// Compute `(x_offset, y_offset)` from `(angle_deg, distance)`.
///
/// IDML's `Angle` is where the LIGHT comes from, so the shadow falls
/// the other way: `x = −distance · cos(angle)`, `y = +distance ·
/// sin(angle)` in the page's y-down coordinates. Asked directly,
/// InDesign 20.0.1 computes exactly that — `Angle = 180` (light from
/// the left) with `Distance = 10` reports `xOffset = +10`, and
/// `Angle = 90` (light from above) reports `yOffset = +10`, i.e. the
/// shadow drops down the page.
///
/// This used to be the negative of that on both axes, so every effect
/// whose IDML gives only `Angle`/`Distance` was cast on the wrong side.
/// The corpus never caught it: InDesign writes explicit
/// `XOffset`/`YOffset` for the fixtures' drop and inner shadows, and
/// satin's two offsets are symmetric, so the polar path is only taken
/// by files InDesign wrote — like the annual.
fn polar_to_offset(angle_deg: f32, distance: f32) -> (f32, f32) {
    let rad = angle_deg.to_radians();
    let (sin, cos) = rad.sin_cos();
    (-distance * cos, distance * sin)
}

fn inner_shadow_from_parser(
    p: &InnerShadowParams,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    list: &mut paged_compose::DisplayList,
) -> ComposeInnerShadow {
    let color = resolve_effect_color(p.effect_color.as_deref(), palette, color_ctx, list);
    // Prefer explicit (XOffset, YOffset). Fall back to polar
    // (angle, distance) when only those are set; otherwise (0, 0).
    let (offset_x, offset_y) = match (p.x_offset, p.y_offset, p.angle_deg, p.distance) {
        (Some(x), Some(y), _, _) => (x, y),
        (_, _, Some(angle), Some(dist)) => polar_to_offset(angle, dist),
        _ => (0.0, 0.0),
    };
    ComposeInnerShadow {
        offset_x,
        offset_y,
        blur_radius: p.size.unwrap_or(DEFAULT_BLUR_RADIUS),
        color,
        opacity: pct_to_unit(p.opacity_pct, DEFAULT_OPACITY),
        choke: pct_to_unit(p.choke_pct, 0.0),
        blend_mode: blend_mode_from_idml(p.blend_mode.as_deref())
            .or_default_to(BlendMode::Multiply),
    }
}

fn outer_glow_from_parser(
    p: &OuterGlowParams,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    list: &mut paged_compose::DisplayList,
) -> ComposeOuterGlow {
    // FINDING #7.4 — glow default color is white (visible under Screen).
    let color = resolve_effect_color_or(
        p.effect_color.as_deref(),
        Color::WHITE,
        palette,
        color_ctx,
        list,
    );
    ComposeOuterGlow {
        blur_radius: p.size.unwrap_or(DEFAULT_BLUR_RADIUS),
        color,
        opacity: pct_to_unit(p.opacity_pct, DEFAULT_OPACITY),
        blend_mode: blend_mode_from_idml(p.blend_mode.as_deref()).or_default_to(BlendMode::Screen),
        spread: pct_to_unit(p.spread_pct, 0.0),
    }
}

fn inner_glow_from_parser(
    p: &InnerGlowParams,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    list: &mut paged_compose::DisplayList,
) -> ComposeInnerGlow {
    // FINDING #7.4 — glow default color is white (visible under Screen).
    let color = resolve_effect_color_or(
        p.effect_color.as_deref(),
        Color::WHITE,
        palette,
        color_ctx,
        list,
    );
    ComposeInnerGlow {
        blur_radius: p.size.unwrap_or(DEFAULT_BLUR_RADIUS),
        color,
        opacity: pct_to_unit(p.opacity_pct, DEFAULT_OPACITY),
        blend_mode: blend_mode_from_idml(p.blend_mode.as_deref()).or_default_to(BlendMode::Screen),
        choke: pct_to_unit(p.choke_pct, 0.0),
    }
}

fn bevel_emboss_from_parser(
    p: &BevelEmbossParams,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    list: &mut paged_compose::DisplayList,
) -> ComposeBevelEmboss {
    let highlight_color = p
        .highlight_color
        .as_deref()
        .map(|id| resolve_effect_color(Some(id), palette, color_ctx, list))
        .unwrap_or(Color::WHITE);
    let shadow_color = p
        .shadow_color
        .as_deref()
        .map(|id| resolve_effect_color(Some(id), palette, color_ctx, list))
        .unwrap_or(Color::BLACK);
    // W1.4 parity — map the IDML enum strings onto the compose
    // style/direction/technique knobs the rasterizer now honours.
    let style = match p.style.as_deref() {
        Some("OuterBevel") => BevelStyle::OuterBevel,
        Some("Emboss") => BevelStyle::Emboss,
        Some("PillowEmboss") => BevelStyle::PillowEmboss,
        Some("StrokeEmboss") => BevelStyle::StrokeEmboss,
        // "InnerBevel" and any unrecognised value fall through to the
        // inner bevel (InDesign's default).
        _ => BevelStyle::InnerBevel,
    };
    let direction = match p.direction.as_deref() {
        Some("Down") => BevelDirection::Down,
        _ => BevelDirection::Up,
    };
    let technique = match p.technique.as_deref() {
        Some("ChiselHard") => BevelTechnique::ChiselHard,
        Some("ChiselSoft") => BevelTechnique::ChiselSoft,
        _ => BevelTechnique::Smooth,
    };
    ComposeBevelEmboss {
        // Depth is an IDML percentage that runs to 1000 %; the
        // rasterizer's contrast multiplier is 1.0 at "100% depth".
        depth: pct_to_scale(p.depth_pct, 1.0),
        size: p.size.unwrap_or(DEFAULT_BLUR_RADIUS),
        angle_deg: p.angle_deg.unwrap_or(120.0),
        altitude_deg: p.altitude_deg.unwrap_or(30.0),
        highlight_color,
        shadow_color,
        highlight_opacity: pct_to_unit(p.highlight_opacity_pct, DEFAULT_OPACITY),
        shadow_opacity: pct_to_unit(p.shadow_opacity_pct, DEFAULT_OPACITY),
        style,
        direction,
        technique,
        // `Soften` is an IDML pt value; the rasterizer adds it as an
        // extra Gaussian over the shaded layer.
        soften: p.soften.unwrap_or(0.0),
    }
}

fn satin_from_parser(
    p: &SatinParams,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    list: &mut paged_compose::DisplayList,
) -> ComposeSatin {
    let color = resolve_effect_color(p.effect_color.as_deref(), palette, color_ctx, list);
    ComposeSatin {
        blur_radius: p.size.unwrap_or(DEFAULT_BLUR_RADIUS),
        angle_deg: p.angle_deg.unwrap_or(19.0),
        distance: p.distance.unwrap_or(11.0),
        color,
        opacity: pct_to_unit(p.opacity_pct, 0.5),
        blend_mode: blend_mode_from_idml(p.blend_mode.as_deref())
            .or_default_to(BlendMode::Multiply),
        // W1.4 parity — `Invert` flips the satin wave band (dark
        // centre instead of bright). Absent ⇒ false (InDesign default).
        invert: p.invert.unwrap_or(false),
    }
}

fn feather_from_parser(p: &FeatherParams) -> ComposeFeather {
    let corner_type = match p.corner_type.as_deref() {
        Some("Rounded") => FeatherCornerType::Rounded,
        Some("Diffusion") => FeatherCornerType::Diffusion,
        // "Sharp" and any unrecognised value fall through to Sharp.
        _ => FeatherCornerType::Sharp,
    };
    ComposeFeather {
        width: p.width.unwrap_or(DEFAULT_FEATHER_WIDTH),
        corner_type,
        noise: pct_to_unit(p.noise_pct, 0.0),
        choke: pct_to_unit(p.choke_pct, 0.0),
    }
}

/// Convert a parser `DirectionalFeatherParams` into the compose
/// layer's `DirectionalFeather`. Missing per-edge widths default to
/// 0 (no feather on that side); other knobs follow the plain
/// feather's defaults.
fn directional_feather_from_parser(p: &DirectionalFeatherParams) -> ComposeDirectionalFeather {
    let corner_type = match p.corner_type.as_deref() {
        Some("Rounded") => FeatherCornerType::Rounded,
        Some("Diffusion") => FeatherCornerType::Diffusion,
        _ => FeatherCornerType::Sharp,
    };
    ComposeDirectionalFeather {
        left_width: p.left_width.unwrap_or(0.0),
        right_width: p.right_width.unwrap_or(0.0),
        top_width: p.top_width.unwrap_or(0.0),
        bottom_width: p.bottom_width.unwrap_or(0.0),
        angle_deg: p.angle_deg.unwrap_or(0.0),
        noise: pct_to_unit(p.noise_pct, 0.0),
        choke: pct_to_unit(p.choke_pct, 0.0),
        corner_type,
    }
}

/// Convert a parser `GradientFeatherParams` into the compose layer's
/// `GradientFeather`. Endpoints default to a horizontal axis across
/// the unit rect (`(0, 0.5) → (1, 0.5)`) when the IDML omits both
/// `GradientStart`/`GradientEnd` *and* angle. Stops collapse the
/// parser's 0..100 location/alpha pair into 0..1 floats. The renderer
/// doesn't yet resolve `stop_color` against the palette to extract a
/// per-channel alpha; we use `alpha_pct` directly (matches the IDML
/// convention where the gradient feather's `<GradientStop>` carries
/// the alpha as a separate attribute).
///
/// `unit_normalize` selects the output coordinate space: `Some(rect)`
/// converts the IDML's path-local axis points into the unit rect
/// (`[0,0,1,1]`) — the path the rasterizer stamps when the caller is
/// the unit-rect path; `None` leaves them in path-local coords (the
/// rounded-corner path).
fn gradient_feather_from_parser(
    p: &GradientFeatherParams,
    unit_normalize: Option<Rect>,
) -> ComposeGradientFeather {
    let kind = match p.gradient_type.as_deref() {
        Some("Radial") => GradientFeatherKind::Radial,
        // "Linear" and any unrecognised value fall through to Linear.
        _ => GradientFeatherKind::Linear,
    };
    // Pick the gradient axis. Prefer explicit start/end points; if
    // only an angle is supplied, derive a unit-square axis through
    // the centre at that angle. Otherwise fall back to a horizontal
    // axis across the unit rect.
    //
    // The fallback (angle / no-axis) cases produce coordinates already
    // in unit-rect space, so they bypass `unit_normalize` below.
    let (start, end, already_unit) = match (p.start_point, p.end_point) {
        (Some(s), Some(e)) => (s, e, false),
        _ => match p.angle_deg {
            Some(angle) => {
                let rad = angle.to_radians();
                let (sin, cos) = rad.sin_cos();
                // Sweep from the unit-rect centre to the edge at
                // (cos, -sin) (IDML's screen-down Y convention,
                // matches `polar_to_offset`).
                let cx = 0.5;
                let cy = 0.5;
                let half = 0.5_f32;
                (
                    (cx - half * cos, cy + half * sin),
                    (cx + half * cos, cy - half * sin),
                    true,
                )
            }
            None => ((0.0, 0.5), (1.0, 0.5), true),
        },
    };
    // If the caller's path is the unit rect, normalise IDML path-local
    // coordinates by the rect's bounds so the rasterizer's
    // `transform.apply` maps them to the right page-pt position. The
    // angle / fallback cases are already in unit space — skip them.
    let (start, end) = match (unit_normalize, already_unit) {
        (Some(r), false) => {
            let inv_w = if r.w.abs() > 1e-6 { 1.0 / r.w } else { 0.0 };
            let inv_h = if r.h.abs() > 1e-6 { 1.0 / r.h } else { 0.0 };
            (
                ((start.0 - r.x) * inv_w, (start.1 - r.y) * inv_h),
                ((end.0 - r.x) * inv_w, (end.1 - r.y) * inv_h),
            )
        }
        _ => (start, end),
    };
    // An applied `<GradientFeatherSetting>` with no `<GradientStop>`
    // children is not "no gradient": it is InDesign's DEFAULT gradient
    // feather, white (opaque) to black (transparent) across the axis.
    // InDesign writes the element bare in that case, so a reader that
    // treats the empty list as "nothing to do" draws no fade at all —
    // which is what the annual's gradient-feather bar did next to
    // InDesign's smooth fade to paper.
    let stops: Vec<ComposeGradientFeatherStop> = if p.stops.is_empty() {
        vec![
            ComposeGradientFeatherStop {
                location: 0.0,
                alpha: 1.0,
            },
            ComposeGradientFeatherStop {
                location: 1.0,
                alpha: 0.0,
            },
        ]
    } else {
        p.stops
            .iter()
            .map(|s| ComposeGradientFeatherStop {
                location: (s.location_pct / 100.0).clamp(0.0, 1.0),
                alpha: (s.alpha_pct / 100.0).clamp(0.0, 1.0),
            })
            .collect()
    };
    ComposeGradientFeather {
        kind,
        start_x: start.0,
        start_y: start.1,
        end_x: end.0,
        end_y: end.1,
        stops,
    }
}

/// Tiny helper trait so a `BlendMode::Normal` from the parser maps to
/// a sensible per-effect default (Multiply for shadows / satin, Screen
/// for glows). The parser's `blend_mode_from_idml` returns `Normal`
/// both for "absent" and for an explicit `BlendMode="Normal"` — the
/// per-effect defaults below are what InDesign ships out of the box.
trait BlendModeDefault {
    fn or_default_to(self, default: BlendMode) -> BlendMode;
}

impl BlendModeDefault for BlendMode {
    fn or_default_to(self, default: BlendMode) -> BlendMode {
        match self {
            BlendMode::Normal => default,
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shadow_falls_away_from_the_light_the_angle_names() {
        // Asked directly, InDesign 20.0.1 turns Angle/Distance into
        // xOffset/yOffset like this: 180° (light from the left) gives
        // (+d, 0), and 90° (light from above) gives (0, +d), i.e. the
        // shadow drops DOWN the y-down page. We had both axes negated,
        // so every effect whose IDML omits XOffset/YOffset — which is
        // what InDesign writes for an inner shadow — was cast on the
        // wrong side.
        let near = |a: (f32, f32), b: (f32, f32)| {
            assert!(
                (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3,
                "{a:?} vs {b:?}"
            );
        };
        near(polar_to_offset(180.0, 10.0), (10.0, 0.0));
        near(polar_to_offset(0.0, 10.0), (-10.0, 0.0));
        near(polar_to_offset(90.0, 10.0), (0.0, 10.0));
        near(polar_to_offset(-90.0, 10.0), (0.0, -10.0));
        near(polar_to_offset(135.0, 10.0), (7.071_068, 7.071_068));
    }

    #[test]
    fn depth_is_a_multiple_not_a_fraction() {
        // InDesign's Depth slider runs to 1000 %. Passing it through
        // the 0..=1 percentage helper pinned the annual's 120 % bevel
        // — and every stronger one — back to 100 %.
        assert_eq!(pct_to_scale(Some(120.0), 1.0), 1.2);
        assert_eq!(pct_to_scale(Some(1000.0), 1.0), 10.0);
        assert_eq!(pct_to_unit(Some(120.0), 1.0), 1.0);
    }

    #[test]
    fn a_gradient_feather_with_no_stops_is_indesigns_default_fade() {
        // InDesign writes `<GradientFeatherSetting Applied="true"
        // Type="Linear" Angle="0"/>` with no `<GradientStop>` children
        // when the object carries the default gradient feather, and
        // renders it as opaque-to-transparent along the axis. Reading
        // the empty list as "no stops, nothing to fade" left the
        // annual's feathered bar flat vermilion against InDesign's
        // fade to paper.
        let p = GradientFeatherParams {
            gradient_type: Some("Linear".into()),
            angle_deg: Some(0.0),
            start_point: None,
            end_point: None,
            stops: Vec::new(),
        };
        let g = gradient_feather_from_parser(&p, None);
        assert_eq!(g.stops.len(), 2, "the default gradient has two stops");
        assert_eq!((g.stops[0].location, g.stops[0].alpha), (0.0, 1.0));
        assert_eq!((g.stops[1].location, g.stops[1].alpha), (1.0, 0.0));
    }

    #[test]
    fn declared_gradient_feather_stops_win_over_the_default() {
        let p = GradientFeatherParams {
            gradient_type: Some("Radial".into()),
            angle_deg: None,
            start_point: None,
            end_point: None,
            stops: vec![
                paged_model::GradientFeatherStop {
                    location_pct: 20.0,
                    alpha_pct: 80.0,
                    midpoint_pct: 50.0,
                    stop_color: None,
                },
                paged_model::GradientFeatherStop {
                    location_pct: 90.0,
                    alpha_pct: 10.0,
                    midpoint_pct: 50.0,
                    stop_color: None,
                },
            ],
        };
        let g = gradient_feather_from_parser(&p, None);
        assert_eq!(g.stops.len(), 2);
        assert!((g.stops[0].alpha - 0.8).abs() < 1e-6);
        assert!((g.stops[1].location - 0.9).abs() < 1e-6);
    }
}
