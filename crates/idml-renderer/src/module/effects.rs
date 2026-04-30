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
//! Today only Rectangle's parser arm captures the effects bag; the
//! pipeline calls this module from `emit_rectangle_into`. TextFrame /
//! Oval / Polygon need their parser arms extended before the renderer
//! can wire them up.

use idml_compose::{
    BevelEmboss as ComposeBevelEmboss, BlendMode, Color,
    DirectionalFeather as ComposeDirectionalFeather, DisplayCommand, Feather as ComposeFeather,
    FeatherCornerType, GradientFeather as ComposeGradientFeather, GradientFeatherKind,
    GradientFeatherStop as ComposeGradientFeatherStop, InnerGlow as ComposeInnerGlow,
    InnerShadow as ComposeInnerShadow, OuterGlow as ComposeOuterGlow, Paint, PathId,
    Satin as ComposeSatin, Transform,
};
use idml_parse::{
    spread::{
        BevelEmbossParams, DirectionalFeatherParams, FeatherParams, FrameEffects,
        GradientFeatherParams, InnerGlowParams, InnerShadowParams, OuterGlowParams, SatinParams,
    },
    Graphic,
};

use crate::pipeline::{blend_mode_from_idml, color_id_to_paint_with_list, BuiltPage};

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
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    if let Some(p) = effects.outer_glow.as_ref() {
        let params = outer_glow_from_parser(p, palette, cmyk_xform, &mut page.list);
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
pub(crate) fn emit_effects_post_fill(
    page: &mut BuiltPage,
    effects: &FrameEffects,
    fill_path_id: PathId,
    transform: Transform,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
) {
    if let Some(p) = effects.inner_shadow.as_ref() {
        let params = inner_shadow_from_parser(p, palette, cmyk_xform, &mut page.list);
        page.list.commands.push(DisplayCommand::InnerShadow {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
    if let Some(p) = effects.inner_glow.as_ref() {
        let params = inner_glow_from_parser(p, palette, cmyk_xform, &mut page.list);
        page.list.commands.push(DisplayCommand::InnerGlow {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
    if let Some(p) = effects.bevel.as_ref() {
        let params = bevel_emboss_from_parser(p, palette, cmyk_xform, &mut page.list);
        page.list.commands.push(DisplayCommand::BevelEmboss {
            path_id: fill_path_id,
            transform,
            params,
        });
    }
    if let Some(p) = effects.satin.as_ref() {
        let params = satin_from_parser(p, palette, cmyk_xform, &mut page.list);
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
        // Emit GradientFeather only when the gradient *isn't* an
        // alpha fade. The CPU rasterizer's current approximation
        // paints a 50%-black tinted stamp — same trick as the plain
        // `Feather` arm — so a fading stop list (e.g. 100% → 0%)
        // would *darken* the rect rather than fade it out, the
        // opposite of InDesign's intent.
        //
        // For the common case where every stop is fully opaque we
        // still emit (the gradient becomes a no-op or a uniform
        // tint, harmless). When any stop's alpha drops below 100%,
        // skip emission until the rasterizer learns to modulate the
        // underlying fill's alpha instead of stamping its own tint.
        let any_fade = p.stops.iter().any(|s| s.alpha_pct < 100.0);
        if !any_fade {
            let params = gradient_feather_from_parser(p);
            page.list.commands.push(DisplayCommand::GradientFeather {
                path_id: fill_path_id,
                transform,
                params,
            });
        }
    }
}

/// Resolve a parser color id (e.g. `"Color/Black"`) into a compose
/// `Color`, defaulting to opaque black when the id is absent or
/// unresolvable. Gradient swatches collapse to black — IDML's effect
/// settings only ever reference solid swatches in practice.
fn resolve_effect_color(
    id: Option<&str>,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    list: &mut idml_compose::DisplayList,
) -> Color {
    let Some(id) = id else {
        return Color::BLACK;
    };
    match color_id_to_paint_with_list(id, palette, cmyk_xform, list) {
        Some(Paint::Solid(c)) => c,
        _ => Color::BLACK,
    }
}

/// Map a 0..=100 percentage to 0..=1, clamped. `None` returns the
/// supplied default. Used for opacity / choke / spread / depth.
fn pct_to_unit(pct: Option<f32>, default: f32) -> f32 {
    pct.map(|p| (p / 100.0).clamp(0.0, 1.0)).unwrap_or(default)
}

/// Compute `(x_offset, y_offset)` from `(angle_deg, distance)` using
/// IDML's screen-down Y convention: `x = distance * cos(angle)`,
/// `y = -distance * sin(angle)` (a 90° angle points up the page, so
/// the Y component flips relative to math convention).
fn polar_to_offset(angle_deg: f32, distance: f32) -> (f32, f32) {
    let rad = angle_deg.to_radians();
    let (sin, cos) = rad.sin_cos();
    (distance * cos, -distance * sin)
}

fn inner_shadow_from_parser(
    p: &InnerShadowParams,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    list: &mut idml_compose::DisplayList,
) -> ComposeInnerShadow {
    let color = resolve_effect_color(p.effect_color.as_deref(), palette, cmyk_xform, list);
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
    cmyk_xform: Option<&idml_color::IccTransform>,
    list: &mut idml_compose::DisplayList,
) -> ComposeOuterGlow {
    let color = resolve_effect_color(p.effect_color.as_deref(), palette, cmyk_xform, list);
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
    cmyk_xform: Option<&idml_color::IccTransform>,
    list: &mut idml_compose::DisplayList,
) -> ComposeInnerGlow {
    let color = resolve_effect_color(p.effect_color.as_deref(), palette, cmyk_xform, list);
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
    cmyk_xform: Option<&idml_color::IccTransform>,
    list: &mut idml_compose::DisplayList,
) -> ComposeBevelEmboss {
    let highlight_color = p
        .highlight_color
        .as_deref()
        .map(|id| resolve_effect_color(Some(id), palette, cmyk_xform, list))
        .unwrap_or(Color::WHITE);
    let shadow_color = p
        .shadow_color
        .as_deref()
        .map(|id| resolve_effect_color(Some(id), palette, cmyk_xform, list))
        .unwrap_or(Color::BLACK);
    ComposeBevelEmboss {
        // Depth is a 0..=100 IDML percentage; the rasterizer's bump
        // strength is a 0..=1 multiplier (1.0 = "100% depth").
        depth: pct_to_unit(p.depth_pct, 1.0),
        size: p.size.unwrap_or(DEFAULT_BLUR_RADIUS),
        angle_deg: p.angle_deg.unwrap_or(120.0),
        altitude_deg: p.altitude_deg.unwrap_or(30.0),
        highlight_color,
        shadow_color,
        highlight_opacity: pct_to_unit(p.highlight_opacity_pct, DEFAULT_OPACITY),
        shadow_opacity: pct_to_unit(p.shadow_opacity_pct, DEFAULT_OPACITY),
        // `style` (OuterBevel / InnerBevel / Emboss / PillowEmboss /
        // StrokeEmboss), `direction` (Up / Down), `technique`
        // (Smooth / ChiselHard / ChiselSoft) and `soften` are not
        // consumed by the rasterizer's Lambertian today — Down vs Up
        // would flip the light's altitude sign, but the harness's
        // current Lambertian samples the highlight + shadow tints
        // symmetrically, so the inversion lands in a follow-up.
    }
}

fn satin_from_parser(
    p: &SatinParams,
    palette: &Graphic,
    cmyk_xform: Option<&idml_color::IccTransform>,
    list: &mut idml_compose::DisplayList,
) -> ComposeSatin {
    let color = resolve_effect_color(p.effect_color.as_deref(), palette, cmyk_xform, list);
    ComposeSatin {
        blur_radius: p.size.unwrap_or(DEFAULT_BLUR_RADIUS),
        angle_deg: p.angle_deg.unwrap_or(19.0),
        distance: p.distance.unwrap_or(11.0),
        color,
        opacity: pct_to_unit(p.opacity_pct, 0.5),
        blend_mode: blend_mode_from_idml(p.blend_mode.as_deref())
            .or_default_to(BlendMode::Multiply),
    }
    // `invert` is captured by the parser but unconsumed — flipping the
    // wave mask is a rasterizer follow-up.
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
fn gradient_feather_from_parser(p: &GradientFeatherParams) -> ComposeGradientFeather {
    let kind = match p.gradient_type.as_deref() {
        Some("Radial") => GradientFeatherKind::Radial,
        // "Linear" and any unrecognised value fall through to Linear.
        _ => GradientFeatherKind::Linear,
    };
    // Pick the gradient axis. Prefer explicit start/end points; if
    // only an angle is supplied, derive a unit-square axis through
    // the centre at that angle. Otherwise fall back to a horizontal
    // axis across the unit rect.
    let (start, end) = match (p.start_point, p.end_point) {
        (Some(s), Some(e)) => (s, e),
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
                )
            }
            None => ((0.0, 0.5), (1.0, 0.5)),
        },
    };
    let stops = p
        .stops
        .iter()
        .map(|s| ComposeGradientFeatherStop {
            location: (s.location_pct / 100.0).clamp(0.0, 1.0),
            alpha: (s.alpha_pct / 100.0).clamp(0.0, 1.0),
        })
        .collect();
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
