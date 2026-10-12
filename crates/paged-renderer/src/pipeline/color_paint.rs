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

//! Paint resolution: swatch / gradient / tint / overprint lookup from
//! the IDML palette (`color_id_to_paint*`), run-level paint and stroke
//! pickers, and the named stroke-style dash table (`stroke_for`).

use super::*;

/// How this build resolves a swatch into paint: the document's CMYK
/// display transform plus the Ink Manager policy that decides what a
/// spot ink's colour even *is*.
///
/// Built once per render in `build_document_inner` from the colour
/// fields of [`PipelineOptions`] (profile, intent, black-point
/// compensation, standard-Lab-for-spots) and threaded to every paint
/// resolver, so the whole page agrees on one answer. Copy, so passing
/// it costs the same as the `Option<&IccTransform>` it replaced.
#[derive(Clone, Copy, Default)]
pub struct ColorCtx<'a> {
    /// CMYK → linear-RGB display transform, built from the document's
    /// working profile. `None` on hosts with no profile loaded (and on
    /// wasm32 without lcms2), where CMYK collapses to the naive RGB
    /// `graphic::to_linear_rgb` produces.
    pub icc: Option<&'a paged_color::IccTransform>,
    /// Ink Manager — "Use Standard Lab Values for Spots". A spot swatch
    /// whose PRIMARY space is Lab carries measured, device-independent
    /// values; its CMYK alternate is only the printing approximation
    /// InDesign falls back to. With this on, such a swatch is DISPLAYED
    /// from its Lab primary. See [`spot_lab_display_rgb`].
    pub standard_lab_for_spots: bool,
}

impl<'a> ColorCtx<'a> {
    /// The transform-only context — the shape every caller that has no
    /// ink-manager state (tests, the `&Graphic`-only public resolvers)
    /// wants.
    pub fn icc(icc: Option<&'a paged_color::IccTransform>) -> Self {
        Self {
            icc,
            standard_lab_for_spots: false,
        }
    }
}

/// Build the document's CMYK display transform. Failures are logged and
/// swallowed: with a malformed profile the render falls back to naive
/// math and still produces output.
pub(super) fn build_cmyk_transform(
    profile: &[u8],
    intent: paged_color::Intent,
    bpc: bool,
) -> Option<paged_color::IccTransform> {
    // Default settings route through the back-compat shim so the
    // per-target intent defaults (native RelColorimetric+BPC, wasm
    // Perceptual) stay bit-identical; explicit document colour settings
    // take the parameterised path.
    let built = if intent == paged_color::Intent::RelativeColorimetric && bpc {
        paged_color::IccTransform::cmyk_to_linear_rgb(profile)
    } else {
        paged_color::IccTransform::cmyk_to_linear_rgb_with(profile, intent, bpc)
    };
    match built {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::warn!(error = %e, "failed to build CMYK ICC transform; using naive conversion");
            None
        }
    }
}

/// The CMYK display transform kept across builds (ADR 027 plan step 2).
/// Building it costs tens of milliseconds per build pass on a real
/// profile, while the inputs (profile bytes, intent, black-point
/// compensation) change only with the colour settings. A caller that
/// builds the same document repeatedly (`CanvasModel`) owns one and
/// passes it as [`PipelineOptions::cmyk_transform_cache`]; the entry is
/// rebuilt whenever any of the three inputs differs from the cached one,
/// so a proof or colour-settings change can never be served a stale
/// transform.
#[derive(Default)]
pub struct CmykTransformCache {
    slot: std::cell::RefCell<Option<CachedCmykTransform>>,
}

struct CachedCmykTransform {
    profile: Vec<u8>,
    intent: paged_color::Intent,
    bpc: bool,
    transform: Option<paged_color::IccTransform>,
}

/// A transform borrowed from a [`CmykTransformCache`], or built for this
/// build alone when the cache is busy.
pub(super) enum CmykTransformRef<'a> {
    Cached(std::cell::Ref<'a, paged_color::IccTransform>),
    Owned(paged_color::IccTransform),
}

impl std::ops::Deref for CmykTransformRef<'_> {
    type Target = paged_color::IccTransform;
    fn deref(&self) -> &paged_color::IccTransform {
        match self {
            CmykTransformRef::Cached(r) => r,
            CmykTransformRef::Owned(t) => t,
        }
    }
}

impl CmykTransformCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn get(
        &self,
        profile: &[u8],
        intent: paged_color::Intent,
        bpc: bool,
    ) -> Option<CmykTransformRef<'_>> {
        let hit = matches!(
            &*self.slot.borrow(),
            Some(c) if c.intent == intent && c.bpc == bpc && c.profile == profile
        );
        if !hit {
            let Ok(mut slot) = self.slot.try_borrow_mut() else {
                // An enclosing build still holds the other entry.
                return build_cmyk_transform(profile, intent, bpc).map(CmykTransformRef::Owned);
            };
            *slot = Some(CachedCmykTransform {
                profile: profile.to_vec(),
                intent,
                bpc,
                transform: build_cmyk_transform(profile, intent, bpc),
            });
        }
        std::cell::Ref::filter_map(self.slot.borrow(), |s| {
            s.as_ref().and_then(|c| c.transform.as_ref())
        })
        .ok()
        .map(CmykTransformRef::Cached)
    }
}

impl std::fmt::Debug for CmykTransformCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let slot = self.slot.try_borrow().ok();
        let entry = slot.as_ref().and_then(|s| s.as_ref());
        f.debug_struct("CmykTransformCache")
            .field("profile_len", &entry.map(|c| c.profile.len()))
            .field("intent", &entry.map(|c| c.intent))
            .field("bpc", &entry.map(|c| c.bpc))
            .finish()
    }
}

/// Pick the paint for a frame from its FillColor attribute.
pub fn resolve_fill(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(frame.fill_color.as_deref()?, palette, ColorCtx::default())
}

/// Same, for StrokeColor.
pub fn resolve_stroke(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(frame.stroke_color.as_deref()?, palette, ColorCtx::default())
}

/// Rectangle flavour of `resolve_fill` (no ParentStory to consider).
pub fn resolve_rect_fill(rect: &Rectangle, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(rect.fill_color.as_deref()?, palette, ColorCtx::default())
}

/// Rectangle flavour of `resolve_stroke`.
pub fn resolve_rect_stroke(rect: &Rectangle, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(rect.stroke_color.as_deref()?, palette, ColorCtx::default())
}

/// Solid-paint resolver. Used by per-cluster glyph paint pickers
/// (where embedding gradient stops per glyph would be wasteful) and
/// by callers that don't have a `&mut DisplayList`.
///
/// CMYK swatches resolve to [`Paint::Cmyk`] when the IDML's
/// `Space="CMYK"` (process or spot-with-CMYK-alternate) so per-channel
/// CMYK overprint compositing (Phase 3 Tier 3 #14 Stage A) can read
/// the source ink values directly. The rasterizer ICC-converts to RGB
/// at draw time for ordinary paints; only the overprint path consumes
/// the channels separately.
///
/// When `color_ctx.icc` is `None` (wasm32 fallback, hosts without an
/// ICC profile loaded) CMYK swatches collapse to the naive RGB the
/// `graphic::to_linear_rgb` helper produces, matching the prior
/// behaviour — the CMYK path is gated on having a usable ICC transform
/// downstream.
/// Short-term fallback for gradient-painted glyphs (P-11): when a run's
/// `FillColor` resolves to a gradient swatch but the glyph emit path
/// only consumes solid paints, evaluate the gradient at its midpoint
/// and substitute a representative `Paint::Solid` (or `Paint::Cmyk`).
/// Returns `None` for non-gradient ids or when fewer than two stops
/// could be resolved.
pub fn gradient_midpoint_paint(
    id: &str,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
) -> Option<Paint> {
    let grad = palette.gradients.get(id)?;
    // Walk the stops in declaration order; interpolate the colour at
    // 50% of the gradient line. Each stop's `StopColor` already routes
    // through the same swatch table the renderer uses elsewhere, so the
    // result respects ICC and tint cascades.
    let resolved: Vec<(f32, Color)> = grad
        .stops
        .iter()
        .filter_map(|s| {
            let p = color_id_to_paint(&s.stop_color, palette, color_ctx)?;
            let c = match p {
                Paint::Solid(c) => c,
                Paint::Cmyk { rgb, .. } => rgb,
                _ => return None,
            };
            Some(((s.location_pct / 100.0).clamp(0.0, 1.0), c))
        })
        .collect();
    if resolved.len() < 2 {
        return None;
    }
    let target = 0.5_f32;
    // Find the segment that brackets `target` and linearly interpolate.
    let mut iter = resolved.windows(2);
    let mut color = resolved.last().map(|s| s.1)?;
    for pair in &mut iter {
        let (off_a, ca) = pair[0];
        let (off_b, cb) = pair[1];
        if target <= off_b {
            let span = (off_b - off_a).max(1e-6);
            let t = ((target - off_a) / span).clamp(0.0, 1.0);
            color = Color::rgba(
                ca.r + (cb.r - ca.r) * t,
                ca.g + (cb.g - ca.g) * t,
                ca.b + (cb.b - ca.b) * t,
                ca.a + (cb.a - ca.a) * t,
            );
            break;
        }
    }
    Some(Paint::Solid(color))
}

pub fn color_id_to_paint(id: &str, palette: &Graphic, color_ctx: ColorCtx<'_>) -> Option<Paint> {
    let entry = palette.resolve(id)?;
    // Ink Manager — "Use Standard Lab Values for Spots". Off (the
    // default, and InDesign's) this is `None` and every line below is
    // the historical path.
    let lab_display = color_ctx
        .standard_lab_for_spots
        .then(|| spot_lab_display_rgb(entry))
        .flatten();
    // Prefer the swatch's *effective* CMYK — it folds Spot →
    // alternate-CMYK resolution and any swatch-level `TintValue`
    // (e.g. "PANTONE 286 C at 50%") into the channels before ICC.
    // Tint scales each channel toward paper white (0,0,0,0) linearly
    // in CMYK space, which is what InDesign does in preview.
    if let (Some(xform), Some([c, m, y, k])) = (color_ctx.icc, entry.effective_cmyk()) {
        {
            // ICC-resolve once at compose time and bake the result into
            // the paint. The rasterizer uses `rgb` for ordinary draws
            // (bit-identical to the pre-Stage-A path) and the
            // C/M/Y/K channels for overprint composition.
            // Backend: lcms2 on native, qcms on wasm32 (both behind
            // the `paged_color::IccTransform` shim).
            let rgb = lab_display.unwrap_or_else(|| {
                let cmyk = paged_color::Cmyk { c, m, y, k };
                let paged_color::LinearRgb([r, g, b]) = xform.cmyk_percent_to_linear_rgb(cmyk);
                Color::rgba(r, g, b, 1.0)
            });
            return Some(Paint::Cmyk {
                // The CHANNELS keep the CMYK alternate even under
                // standard-Lab: they are what the spot plane composites
                // and what separates on output, and the Ink Manager
                // toggle governs how the ink is *shown*, not which
                // plate it lands on.
                c: (c / 100.0).clamp(0.0, 1.0),
                m: (m / 100.0).clamp(0.0, 1.0),
                y: (y / 100.0).clamp(0.0, 1.0),
                k: (k / 100.0).clamp(0.0, 1.0),
                rgb,
                // The list-aware wrappers (e.g. `color_id_to_paint_with_list_dir`)
                // re-tag this paint with a `SpotInkId` for `Model="Spot"`
                // swatches; this function lacks a `&mut DisplayList`,
                // so it leaves the field empty and the visible behaviour
                // collapses to the CMYK-alternate path (matching the
                // Stage A/B output).
                spot: None,
            });
        }
    }
    // No ICC transform: the CMYK alternate would collapse to naive RGB
    // below, so a Lab-primary spot under standard-Lab is better served
    // by its own measured values, which need no profile at all.
    if let Some(rgb) = lab_display {
        return Some(Paint::Solid(rgb));
    }
    if let Some([r, g, b]) = to_linear_rgb(entry) {
        return Some(Paint::Solid(Color::rgba(r, g, b, 1.0)));
    }
    // Concept 2 — Lab swatches. `to_linear_rgb` is the parse
    // layer's no-ICC stopgap and returns None for Lab; resolve here
    // analytically (Lab is device-independent: D50→D65 Bradford →
    // linear sRGB, no profile needed for display). Previously these
    // swatches dropped out entirely and rendered as missing paint.
    lab_entry_to_paint(entry)
}

/// The display colour of a spot ink under the Ink Manager's "Use
/// Standard Lab Values for Spots", or `None` when the setting has
/// nothing to say about this swatch.
///
/// IDML gives a spot swatch two descriptions: its PRIMARY value (what
/// the ink measures — a Lab reading, for every modern library swatch)
/// and an `AlternateColorValue` (how to fake it in process ink). The
/// renderer previews spots through the alternate because that is what
/// InDesign does by default. Standard-Lab is the opposite instruction:
/// trust the measurement. Lab is device-independent, so this needs no
/// profile — D50 → Bradford → linear sRGB and we are done.
///
/// Only Lab-primary spots are affected. A spot described in CMYK or RGB
/// has no measurement to prefer, and a process swatch is not an ink.
fn spot_lab_display_rgb(entry: &paged_model::ColorEntry) -> Option<Color> {
    if entry.model != paged_model::ColorModel::Spot {
        return None;
    }
    let [l, a, b] = entry.effective_lab()?;
    let paged_color::LinearRgb([r, g, bl]) = paged_color::lab::lab_d50_to_linear_srgb(l, a, b);
    Some(Color::rgba(r, g, bl, 1.0))
}

/// Lab(D50) swatch → solid paint, or None when the entry isn't a
/// 3-channel Lab colour. Shared by the direct resolver above and
/// the canvas-side preview path.
pub fn lab_entry_to_paint(entry: &paged_model::ColorEntry) -> Option<Paint> {
    if entry.space != paged_model::ColorSpace::Lab || entry.value.len() != 3 {
        return None;
    }
    let paged_color::LinearRgb([r, g, b]) =
        paged_color::lab::lab_d50_to_linear_srgb(entry.value[0], entry.value[1], entry.value[2]);
    Some(Paint::Solid(Color::rgba(r, g, b, 1.0)))
}

/// A linear gradient's axis in the path's local 0..1 unit rect, placed
/// where InDesign places it when the frame carries no
/// `GradientFillStart`.
///
/// Asked of InDesign 20.0.1 (`corpus/generated/gradient-geometry`,
/// ADR 120), and confirmed by what it writes back when it re-exports
/// such a frame:
///
/// * the angle turns COUNTER-clockwise as seen on the page — the y-up
///   convention — so the axis direction in the frame's (y-down) inner
///   coordinates is `(cos θ, −sin θ)`. 90° runs from the bottom to the
///   top, 135° up and to the left, −90° downwards;
/// * with no start the axis begins at the inner bounding box's
///   BOTTOM-LEFT corner (InDesign writes `GradientFillStart="0 h"`);
/// * with no length it is as long as the box is WIDE (it writes
///   `GradientFillLength="w"`) whatever the angle — so a 120 × 60
///   rectangle at 90° shows only the first half of its ramp, and one
///   at 180° or −90° only the first stop.
///
/// `length_pt` is `GradientFillLength`; zero or less counts as absent.
/// `dims_pt` is the box's `(w, h)` in pt; without it the box is taken
/// as square. An explicit `GradientFillStart` moves the axis afterwards
/// (`module::fill_paint::place_linear_at_start`), since it is in inner
/// coordinates and needs the box's origin.
pub(super) fn linear_gradient_endpoints(
    angle_deg: Option<f32>,
    length_pt: Option<f32>,
    dims_pt: Option<(f32, f32)>,
) -> ((f32, f32), (f32, f32)) {
    let (sin, cos) = angle_deg.unwrap_or(0.0).to_radians().sin_cos();
    let length = length_pt.filter(|l| l.is_finite() && *l > 0.0);
    let start = (0.0_f32, 1.0_f32);
    let (dx, dy) = match dims_pt {
        Some((w, h)) if w > 0.0 && h > 0.0 => {
            let l = length.unwrap_or(w);
            (cos * l / w, -sin * l / h)
        }
        _ => (cos, -sin),
    };
    (start, (start.0 + dx, start.1 + dy))
}

/// Resolver that also handles gradient swatches.
///
/// Gradient ids resolve to a `Paint::LinearGradient` whose stops live
/// in `list.gradients`. Solid colours fall through to
/// `color_id_to_paint`. Used for frame fills (which can carry
/// gradient swatches); not used for per-glyph paints.
pub fn color_id_to_paint_with_list(
    id: &str,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    list: &mut DisplayList,
) -> Option<Paint> {
    color_id_to_paint_with_list_dir(id, palette, color_ctx, list, None, None, None)
}

/// Like [`color_id_to_paint_with_list`] but takes an explicit
/// `gradient_angle_deg` from the frame's `GradientFillAngle` /
/// `GradientStrokeAngle` attribute (counter-clockwise on the page: 0°
/// left → right, 90° bottom → top — see [`linear_gradient_endpoints`]),
/// an explicit
/// `gradient_length_pt` from the matching `GradientFillLength` /
/// `GradientStrokeLength` attribute, and the path's local bbox
/// `(width, height)` in pt.
///
/// The bbox lets the radial-gradient default place its centre at the
/// path's bottom-left corner with radius equal to its longer side —
/// matching what InDesign emits when `GradientFillStart` /
/// `GradientFillLength` are absent. For linear gradients it converts
/// the page-pt length into unit-rect endpoints (so the same
/// `LinearGradient` reused on rectangles of different sizes still
/// honours the user-specified length).
///
/// Without the bbox or length we fall back to the unit-rect centred
/// default — gradient line through `(0.5, 0.5)` along the angle, with
/// half-vector magnitude `0.5` in unit-rect coords (still serviceable
/// for callers that don't have geometry, e.g. text-frame strokes).
/// InDesign gradient midpoint remap. Given the linear parameter `t`
/// across a stop-to-next-stop segment and the segment's midpoint `mid`
/// (0..1; 0.5 = linear), return the colour-blend fraction `f` so that
/// `f == 0.5` exactly at `t == mid` — the *colour* follows this curve
/// while the stop's geometric `offset` stays linear in `t`.
///
/// The curve is InDesign's own, read off its PDF export
/// (`corpus/generated/gradient-geometry` cases 23 and 24): each segment
/// is one exponential function with `N = ln 0.5 / ln m ≥ 1`, where `m`
/// is the midpoint or its mirror `1 − mid`, whichever is above ½. A
/// midpoint of 80 is `f = t^3.10628`; a midpoint of 20 is the same
/// function run backwards (`Encode [1 0]`), `f = 1 − (1 − t)^3.10628`
/// — not `t^0.43`, which also crosses ½ at 0.2 but bows the other way.
pub(super) fn midpoint_blend(t: f32, mid: f32) -> f32 {
    if (mid - 0.5).abs() < 1e-4 {
        return t;
    }
    if mid > 0.5 {
        t.powf(0.5f32.ln() / mid.ln())
    } else {
        1.0 - (1.0 - t).powf(0.5f32.ln() / (1.0 - mid).ln())
    }
}

pub(super) fn color_lerp(
    a: paged_compose::Color,
    b: paged_compose::Color,
    f: f32,
) -> paged_compose::Color {
    paged_compose::Color::rgba(
        a.r * (1.0 - f) + b.r * f,
        a.g * (1.0 - f) + b.g * f,
        a.b * (1.0 - f) + b.b * f,
        a.a * (1.0 - f) + b.a * f,
    )
}

pub fn color_id_to_paint_with_list_dir(
    id: &str,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    list: &mut DisplayList,
    gradient_angle_deg: Option<f32>,
    gradient_length_pt: Option<f32>,
    path_dims_pt: Option<(f32, f32)>,
) -> Option<Paint> {
    if let Some(grad) = palette.gradients.get(id) {
        // Resolve raw stop colors. For CMYK swatches, also keep the
        // raw CMYK percentages so we can interpolate in CMYK space
        // (pdftoppm's behaviour) — interpolating stops in linear-sRGB
        // produces over-saturated mid-tones because CMYK→sRGB is
        // non-linear (e.g. 50% C + 50% M is a duller violet than the
        // average of pure-C-sRGB and pure-M-sRGB).
        struct StopRef {
            offset: f32,
            color: paged_compose::Color,
            // Owned so a spot swatch's tint-scaled alternate CMYK can
            // participate in CMYK-space gradient interpolation just
            // like a process CMYK swatch would.
            cmyk: Option<[f32; 4]>,
            /// This stop's `Midpoint` (0..1; 0.5 = linear, the default
            /// when omitted). It bends the segment that ENDS at this
            /// stop: asked of InDesign (`gradient-geometry` cases 23–24),
            /// `Midpoint="20"` on stop 1 puts the half-way colour at 20 %
            /// of the segment from stop 0, and `Midpoint="80"` on the
            /// last stop bends the segment before it. A first stop's
            /// midpoint governs nothing.
            midpoint: f32,
        }
        let raw_stops: Vec<StopRef> = grad
            .stops
            .iter()
            .filter_map(|s| {
                let color = color_id_to_paint(&s.stop_color, palette, color_ctx)
                    .and_then(|p| paint_as_solid_with_icc(p, color_ctx))?;
                let entry = palette.resolve(&s.stop_color);
                let cmyk = entry.and_then(|e| e.effective_cmyk());
                Some(StopRef {
                    offset: (s.location_pct / 100.0).clamp(0.0, 1.0),
                    color,
                    cmyk,
                    midpoint: s
                        .midpoint_pct
                        .map(|m| (m / 100.0).clamp(0.01, 0.99))
                        .unwrap_or(0.5),
                })
            })
            .collect();
        let has_midpoint = raw_stops
            .iter()
            .skip(1)
            .any(|s| (s.midpoint - 0.5).abs() > 1e-4);
        if raw_stops.len() < 2 {
            return None;
        }
        let cmyk_tessellate_xform = color_ctx
            .icc
            .filter(|_| raw_stops.iter().any(|s| s.cmyk.is_some()));
        let stops: Vec<paged_compose::GradientStop> = if let Some(xform) = cmyk_tessellate_xform {
            // At least one stop carries CMYK (process or spot-with-
            // CMYK-alternate). Tessellate the gradient in CMYK space
            // and convert each tessellated point through the ICC
            // transform. 16 sub-stops per inter-stop segment is enough
            // to make even cyan↔yellow mid-tones (the most visibly
            // non-linear pair) match pdftoppm within ~1 ΔE.
            //
            // Track 1c: stops without a CMYK alternate (RGB / LAB /
            // Gray swatches, or spot-with-non-CMYK-alternate) get a
            // naive sRGB→CMYK approximation. Required for the
            // boundary case the cycle-2 work missed: an RGB stop
            // mixed with a CMYK / Pantone stop previously fell out
            // of the CMYK-space path entirely (because the
            // `all(|s| s.cmyk.is_some())` guard tripped) and
            // collapsed to sRGB-linear blending, producing a duller
            // mid-tone than InDesign's preview CMYK.
            fn rgb_to_cmyk_naive(c: paged_compose::Color) -> [f32; 4] {
                let r = c.r.clamp(0.0, 1.0);
                let g = c.g.clamp(0.0, 1.0);
                let b = c.b.clamp(0.0, 1.0);
                let k = 1.0 - r.max(g).max(b);
                let denom = 1.0 - k;
                let (cy, m, y) = if denom <= f32::EPSILON {
                    (0.0, 0.0, 0.0)
                } else {
                    (
                        (1.0 - r - k) / denom,
                        (1.0 - g - k) / denom,
                        (1.0 - b - k) / denom,
                    )
                };
                [cy * 100.0, m * 100.0, y * 100.0, k * 100.0]
            }
            const SUB_STOPS: usize = 16;
            let mut out: Vec<paged_compose::GradientStop> = Vec::new();
            for win in raw_stops.windows(2) {
                let a = &win[0];
                let b = &win[1];
                let cmyk_a = a.cmyk.unwrap_or_else(|| rgb_to_cmyk_naive(a.color));
                let cmyk_b = b.cmyk.unwrap_or_else(|| rgb_to_cmyk_naive(b.color));
                for i in 0..SUB_STOPS {
                    let t = i as f32 / SUB_STOPS as f32;
                    // Geometry stays linear in `t`; the CMYK channel
                    // blend follows the midpoint curve.
                    let f = midpoint_blend(t, b.midpoint);
                    let interp = paged_color::Cmyk {
                        c: cmyk_a[0] * (1.0 - f) + cmyk_b[0] * f,
                        m: cmyk_a[1] * (1.0 - f) + cmyk_b[1] * f,
                        y: cmyk_a[2] * (1.0 - f) + cmyk_b[2] * f,
                        k: cmyk_a[3] * (1.0 - f) + cmyk_b[3] * f,
                    };
                    let paged_color::LinearRgb([r, g, b_]) =
                        xform.cmyk_percent_to_linear_rgb(interp);
                    out.push(paged_compose::GradientStop {
                        offset: a.offset * (1.0 - t) + b.offset * t,
                        color: paged_compose::Color::rgba(r, g, b_, 1.0),
                    });
                }
            }
            // Always include the final stop exactly.
            let last = raw_stops.last().unwrap();
            out.push(paged_compose::GradientStop {
                offset: last.offset,
                color: last.color,
            });
            out
        } else if has_midpoint {
            // sRGB blend with a non-default midpoint on at least one
            // segment: tessellate so the colour follows the midpoint
            // power curve. Offset stays linear in `t`. Segments whose
            // midpoint is the default 0.5 tessellate to a straight ramp
            // (midpoint_blend is the identity there), so the extra
            // sub-stops are harmless.
            const SUB_STOPS: usize = 16;
            let mut out: Vec<paged_compose::GradientStop> = Vec::new();
            for win in raw_stops.windows(2) {
                let a = &win[0];
                let b = &win[1];
                for i in 0..SUB_STOPS {
                    let t = i as f32 / SUB_STOPS as f32;
                    let f = midpoint_blend(t, b.midpoint);
                    out.push(paged_compose::GradientStop {
                        offset: a.offset * (1.0 - t) + b.offset * t,
                        color: color_lerp(a.color, b.color, f),
                    });
                }
            }
            let last = raw_stops.last().unwrap();
            out.push(paged_compose::GradientStop {
                offset: last.offset,
                color: last.color,
            });
            out
        } else {
            raw_stops
                .iter()
                .map(|s| paged_compose::GradientStop {
                    offset: s.offset,
                    color: s.color,
                })
                .collect()
        };
        // Radial gradients without an explicit `GradientFillStart` /
        // `GradientFillLength` use InDesign's auto-default: centre at
        // the path's BOTTOM-LEFT corner with radius equal to the
        // path's diagonal (verified empirically against an InDesign-
        // exported PDF — see corpus/generated/gradients.pdf p. 3).
        // The renderer's gradient lives in the path's local 0..1
        // unit-rect; the rasterizer derives the actual circle radius
        // by averaging `width * R_unit` and `height * R_unit` (see
        // `paged_gpu::cpu::build_radial_gradient_shader`), so to
        // produce a circle of pt-radius √(w² + h²) we set
        // `R_unit = 2·√(w² + h²) / (w + h)`. When the caller can't
        // supply the bbox (text-frame strokes etc.) we fall back to
        // the legacy centred-on-(0.5, 0.5) / √½ unit-rect default.
        if matches!(grad.kind, paged_model::GradientKind::Radial) {
            // Centre at (0, 1) of the unit-rect (= bottom-left of
            // the path in InDesign coords) with radius equal to the
            // longer bbox dimension. Empirically matches what
            // pdftoppm renders from an InDesign-exported PDF for a
            // gradient applied via the Swatches panel without manual
            // gradient-tool placement (corpus/generated/gradients
            // page 3): gradient hits pure black at distance ≈ width
            // for a 360×200 rect, *not* at the diagonal.
            let (center, radius) = match path_dims_pt {
                Some((w, h)) if (w + h) > 0.0 => {
                    let r_actual = w.max(h);
                    // Rasterizer averages (a·R, b·R)·hypot and (c·R,
                    // d·R)·hypot to reduce a unit-rect circle to a
                    // single page-space radius (see
                    // `paged_gpu::cpu::build_radial_gradient_shader`).
                    // Compensate so the page-space circle has the
                    // pt-radius we computed above.
                    let r_unit = 2.0 * r_actual / (w + h);
                    ((0.0, 1.0), r_unit)
                }
                _ => ((0.5, 0.5), std::f32::consts::FRAC_1_SQRT_2),
            };
            let id = list.push_radial_gradient(paged_compose::RadialGradient {
                center,
                radius,
                stops,
            });
            return Some(Paint::RadialGradient(id));
        }
        let (start, end) =
            linear_gradient_endpoints(gradient_angle_deg, gradient_length_pt, path_dims_pt);
        let id = list.push_linear_gradient(paged_compose::LinearGradient { start, end, stops });
        return Some(Paint::LinearGradient(id));
    }
    let paint = color_id_to_paint(id, palette, color_ctx)?;
    // Stage C: when the swatch is a named-ink spot colour, intern the
    // ink name on the display list and tag the paint with the resulting
    // id. Spot-on-same-spot overprint then composites per-pixel in the
    // spot's own plane (see `paged-gpu::cpu::compose_spot_overprint_via_planes`).
    // Process CMYK swatches and non-CMYK paints pass through unchanged.
    if let Paint::Cmyk {
        c,
        m,
        y,
        k,
        rgb,
        spot: _,
    } = paint
    {
        if let Some(entry) = palette.resolve(id) {
            if entry.model == paged_model::ColorModel::Spot && entry.effective_cmyk().is_some() {
                let cmyk_alt_unit = entry.effective_cmyk().unwrap();
                let to_8 = |v: f32| (v.clamp(0.0, 100.0) * 2.55).round() as u8;
                let alt_8 = [
                    to_8(cmyk_alt_unit[0]),
                    to_8(cmyk_alt_unit[1]),
                    to_8(cmyk_alt_unit[2]),
                    to_8(cmyk_alt_unit[3]),
                ];
                let spot_id = list.push_spot_ink(paged_compose::SpotInk {
                    name: id.to_string(),
                    cmyk_alternate: alt_8,
                });
                return Some(Paint::Cmyk {
                    c,
                    m,
                    y,
                    k,
                    rgb,
                    spot: Some(spot_id),
                });
            }
        }
    }
    Some(paint)
}

/// Cluster → Paint picker built from a paragraph's run table.
pub struct RunPaintPicker {
    bands: Vec<(u32, Paint)>,
    default: Paint,
}

impl RunPaintPicker {
    pub fn pick(&self, cluster: u32) -> Paint {
        let mut chosen = self.default;
        for (start, paint) in &self.bands {
            if *start <= cluster {
                chosen = *paint;
            } else {
                break;
            }
        }
        chosen
    }
}

/// Per-cluster lookup for a run's text outline (paint + stroke geometry).
/// Constructed once per paragraph alongside `RunPaintPicker`. `pick`
/// returns `None` for clusters whose cascade leaves `StrokeColor`
/// unset (the common case — IDML records a stroke colour on a run
/// only when the author has explicitly assigned one). A `Some` value
/// drives one extra `StrokePath` per glyph in that run.
#[derive(Default)]
pub struct RunStrokePicker {
    /// `(start_cluster, paint_and_stroke_or_none)`. The picker walks
    /// in cluster order so we keep the bands sorted at build time.
    bands: Vec<(u32, Option<(Paint, Stroke)>)>,
}

impl RunStrokePicker {
    pub fn pick(&self, cluster: u32) -> Option<(Paint, Stroke)> {
        let mut chosen: Option<(Paint, Stroke)> = None;
        for (start, entry) in &self.bands {
            if *start <= cluster {
                chosen = *entry;
            } else {
                break;
            }
        }
        chosen
    }

    /// True iff at least one band carries a visible stroke. Lets the
    /// hot per-line emit loop skip the second glyph sweep entirely
    /// for the overwhelming majority of paragraphs.
    pub fn any_visible(&self) -> bool {
        self.bands.iter().any(|(_, e)| e.is_some())
    }
}

pub fn build_run_paint_picker(
    paragraph: &paged_model::Paragraph,
    palette: &Graphic,
    default: Paint,
) -> RunPaintPicker {
    build_run_paint_picker_with_cmyk(paragraph, palette, ColorCtx::default(), default)
}

/// Variant of [`build_run_paint_picker`] that routes CMYK swatches
/// through the document's ICC transform when one is available. Without
/// this the per-glyph fill picker would silently fall back to the
/// naive CMYK→sRGB approximation in `graphic::to_linear_rgb`, undoing
/// the work of building the transform.
pub fn build_run_paint_picker_with_cmyk(
    paragraph: &paged_model::Paragraph,
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    default: Paint,
) -> RunPaintPicker {
    let mut bands: Vec<(u32, Paint)> = Vec::with_capacity(paragraph.runs.len());
    let mut cursor: u32 = 0;
    for run in &paragraph.runs {
        // Gradient swatches fall through `color_id_to_paint` (returns
        // None — gradient resolution requires the DisplayList). For
        // glyph paints we don't yet support per-glyph gradient brushes;
        // substitute the gradient's midpoint colour so display titles
        // stop dropping out (P-11).
        let paint = run
            .fill_color
            .as_deref()
            .and_then(|id| {
                color_id_to_paint(id, palette, color_ctx)
                    .or_else(|| gradient_midpoint_paint(id, palette, color_ctx))
            })
            .unwrap_or(default);
        bands.push((cursor, paint));
        cursor += run.text.len() as u32;
    }
    RunPaintPicker { bands, default }
}

/// Like [`build_run_paint_picker_with_cmyk`] but uses each run's
/// cascaded `fill_color` (so a run that only carries an
/// `AppliedCharacterStyle` still picks up the right paint). Applies
/// the run's resolved `FillTint` after colour conversion.
///
/// `bullet_paint_override` carries `(bullet_byte_len, paint)` when a
/// `BulletsCharacterStyle` / `BulletsAndNumberingDigitsCharacterStyle`
/// resolves a colour that overrides run 0's fill for the list marker
/// only. The picker prepends a band at cursor 0 with the override
/// paint and pushes every content band by `bullet_byte_len` so the
/// bullet glyphs (clusters 0..bullet_byte_len) get the override while
/// the body text past the marker keeps each run's resolved fill.
/// Build a per-cluster stroke picker for a paragraph.
///
/// Each run's cascaded `(stroke_color, stroke_weight)` decides whether
/// glyphs in that run carry an outline. When `stroke_color` resolves
/// to a real paint but `stroke_weight` is `None`, we fall back to 1pt
/// — matching the value the document's `<TextDefault>` records for a
/// fresh InDesign document (the parser doesn't surface TextDefault as
/// its own node yet; 1pt is the InDesign-published default).
pub(super) fn build_run_stroke_picker(
    paragraph: &paged_model::Paragraph,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    bullet_byte_offset: u32,
) -> RunStrokePicker {
    let mut bands: Vec<(u32, Option<(Paint, Stroke)>)> =
        Vec::with_capacity(paragraph.runs.len() + 1);
    let mut cursor = bullet_byte_offset;
    if bullet_byte_offset > 0 {
        // The bullet marker carries no per-run stroke today (the parser
        // wires only fill / fill-tint through the bullet character
        // style). Seed a no-stroke band at cluster 0 so the marker
        // stays fill-only.
        bands.push((0, None));
    }
    for (i, run) in paragraph.runs.iter().enumerate() {
        let entry = resolved_runs[i]
            .stroke_color
            .as_deref()
            .and_then(|id| color_id_to_paint(id, palette, color_ctx))
            .map(|paint| {
                let width = resolved_runs[i].stroke_weight.unwrap_or(1.0);
                (paint, Stroke::new(width))
            });
        bands.push((cursor, entry));
        cursor += run.text.len() as u32;
    }
    RunStrokePicker { bands }
}

pub(super) fn build_run_paint_picker_resolved(
    paragraph: &paged_model::Paragraph,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    default: Paint,
    bullet_paint_override: Option<(u32, Paint)>,
) -> RunPaintPicker {
    let mut bands: Vec<(u32, Paint)> = Vec::with_capacity(paragraph.runs.len() + 1);
    // When a bullet character style overrides the marker's paint, the
    // marker text sits at cluster 0..bullet_byte_len; the content
    // runs follow at cluster bullet_byte_len.. so we seed `cursor` at
    // that offset and emit a leading bullet band.
    let mut cursor: u32 = 0;
    if let Some((bullet_len, bullet_paint)) = bullet_paint_override {
        bands.push((0, bullet_paint));
        cursor = bullet_len;
    }
    for (i, run) in paragraph.runs.iter().enumerate() {
        // Resolve the swatch (or fall through to `default`) FIRST,
        // then apply the run's `FillTint`. The tint affects both
        // explicit swatches and the default paint — IDML treats it
        // as a strength-of-current-fill modifier independent of
        // whether the run carries a FillColor attribute.
        // See `build_run_paint_picker_with_cmyk`: gradient swatches
        // resolve via `gradient_midpoint_paint` as a short-term solid
        // substitute (P-11) until per-glyph gradient brushes land.
        let base = resolved_runs[i]
            .fill_color
            .as_deref()
            .and_then(|id| {
                color_id_to_paint(id, palette, color_ctx)
                    .or_else(|| gradient_midpoint_paint(id, palette, color_ctx))
            })
            .unwrap_or(default);
        let tint = object_tint(
            palette,
            resolved_runs[i].fill_color.as_deref(),
            resolved_runs[i].fill_tint,
        );
        let paint = apply_fill_tint_ctx(base, tint, &color_ctx);
        bands.push((cursor, paint));
        cursor += run.text.len() as u32;
    }
    RunPaintPicker { bands, default }
}

/// W1.8 — build a cluster→paint picker for ONE footnote paragraph's
/// runs. `run_text_lens[i]` is the byte length of run `i`'s SHAPED text
/// (i.e. including the synthesised `"N." + separator` marker folded onto
/// run 0 of the first paragraph), so band offsets line up with the
/// styled-run text the footnote composer actually shaped. The marker
/// inherits run 0's paint. Per-run `FillColor` + `FillTint` resolve
/// exactly as for body text.
pub(super) fn build_footnote_paint_picker(
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    run_text_lens: &[usize],
    palette: &Graphic,
    color_ctx: ColorCtx<'_>,
    default: Paint,
) -> RunPaintPicker {
    let mut bands: Vec<(u32, Paint)> = Vec::with_capacity(resolved_runs.len() + 1);
    let mut cursor: u32 = 0;
    for (i, attrs) in resolved_runs.iter().enumerate() {
        let base = attrs
            .fill_color
            .as_deref()
            .and_then(|id| {
                color_id_to_paint(id, palette, color_ctx)
                    .or_else(|| gradient_midpoint_paint(id, palette, color_ctx))
            })
            .unwrap_or(default);
        let tint = object_tint(palette, attrs.fill_color.as_deref(), attrs.fill_tint);
        let paint = apply_fill_tint_ctx(base, tint, &color_ctx);
        bands.push((cursor, paint));
        cursor += run_text_lens.get(i).copied().unwrap_or(0) as u32;
    }
    RunPaintPicker { bands, default }
}

/// Map an IDML `StrokeType` reference to a [`Stroke`] of the given
/// width with the appropriate dash pattern. Recognises the canonical
/// built-in styles (`StrokeStyle/$ID/Solid`, `Dashed`, `Dotted`,
/// `Dashed3-2`, `Dashed4-4`, `Dashed5-5`, `Dotted2`, `Dotted4`,
/// `Dotted8`); custom user-defined `<StrokeStyle>` definitions
/// fall back to `Solid` until full parser support arrives.
///
/// Pattern values are scaled by the stroke width so a heavier stroke
/// looks proportionally heavier — that mirrors InDesign's behaviour
/// where the named built-ins describe a multiple of the line weight,
/// not absolute pt distances.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stroke_for(
    stroke_type: Option<&str>,
    width: f32,
    end_cap: Option<&str>,
    end_join: Option<&str>,
    miter_limit: Option<f32>,
    stroke_styles: Option<&std::collections::BTreeMap<String, paged_model::StrokeStyleDef>>,
    dash_override: &[f32],
) -> Stroke {
    let mut s = Stroke::new(width);
    if let Some(cap) = end_cap_from(end_cap) {
        s.cap = cap;
    }
    if let Some(join) = end_join_from(end_join) {
        s.join = join;
    }
    if let Some(ml) = miter_limit {
        s.miter_limit = ml;
    }
    // W1.1 — a per-frame `StrokeDashAndGap` override wins over BOTH the
    // custom `<StrokeStyle>` definition and the built-in name table.
    // IDML serialises the per-instance dash in absolute pt, so it feeds
    // the dash slot directly (no `* width` scaling). An empty override
    // ⇒ fall through to the stroke-style resolution below.
    if !dash_override.is_empty() {
        s.dash = paged_compose::DashPattern::from_slice(dash_override);
        return s;
    }
    let Some(name) = stroke_type else {
        return s;
    };
    let w = width.max(0.1);
    // Track 4a: a user-defined `<DashedStrokeStyle>` lookup wins over
    // the built-in name table. The IDML's `Pattern` attribute is
    // already in absolute pt (unlike the named built-ins, which are
    // multiples of the line weight), so the pattern feeds the dash
    // slot directly without the `* w` scaling below.
    if let Some(styles) = stroke_styles {
        if let Some(def) = styles.get(name) {
            use paged_model::StrokeStyleKind as K;
            match def.kind {
                K::Dashed if !def.pattern.is_empty() => {
                    s.dash = paged_compose::DashPattern::from_slice(&def.pattern);
                    return s;
                }
                K::Dotted if !def.pattern.is_empty() => {
                    // A custom `<DottedStrokeStyle>` carries its on/off
                    // pattern in absolute pt just like Dashed; honour it
                    // directly. Round caps render the zero-length "on"
                    // as a dot (matching the built-in Dotted handling).
                    s.dash = paged_compose::DashPattern::from_slice(&def.pattern);
                    if end_cap.is_none() {
                        s.cap = paged_compose::LineCap::Round;
                    }
                    return s;
                }
                // Striped (parallel rules) and Wavy (sine) cannot be
                // expressed by the single-line `Stroke` model (width +
                // cap/join + dash). They intentionally render as a solid
                // stroke of the declared width — a reasonable footprint —
                // until a dedicated multi-line / sine stroke capability
                // lands in the rasterizer (tracked in the internal renderer gap list).
                // Returning here also stops the built-in name table below
                // from mis-mapping a same-named custom style.
                K::Striped | K::Wavy => return s,
                _ => {}
            }
        }
    }
    let suffix = name.strip_prefix("StrokeStyle/$ID/").unwrap_or(name);
    // IDML's "Canned" prefix denotes built-in user-facing stroke
    // styles InDesign ships in the Stroke panel — InDesign serialises
    // them as `StrokeStyle/$ID/Canned <Name>` references. Map the
    // common ones to the same pattern table the bare names use so
    // real IDMLs render with the right dash/dot style without each
    // sample needing to declare a custom <StrokeStyle>.
    let normalised = suffix.strip_prefix("Canned ").unwrap_or(suffix);
    let is_dotted = matches!(
        normalised,
        "Dotted" | "Dotted2" | "Dotted4" | "Dotted8" | "Japanese Dots"
    );
    let pattern: Option<&[f32]> = match normalised {
        "Solid" | "" => None,
        "Dashed" => Some(&[3.0, 2.0]),
        "Dashed3-2" => Some(&[3.0, 2.0]),
        "Dashed4-4" => Some(&[4.0, 4.0]),
        "Dashed5-5" => Some(&[5.0, 5.0]),
        "Dotted" => Some(&[0.0, 2.0]),
        "Dotted2" => Some(&[0.0, 2.0]),
        "Dotted4" => Some(&[0.0, 4.0]),
        "Dotted8" => Some(&[0.0, 8.0]),
        // InDesign's "Japanese Dots" is denser than the standard
        // Dotted (smaller gap, same on-zero-length).
        "Japanese Dots" => Some(&[0.0, 1.5]),
        // Built-in Striped ("Thick - Thin", "Triple", …) and "Wavy"
        // names land here → no dash → a solid stroke of the declared
        // width. True multi-line / sine rendering needs a new rasterizer
        // capability (deferred; see the internal renderer gap list).
        _ => None,
    };
    if let Some(p) = pattern {
        let scaled: Vec<f32> = p.iter().map(|v| v * w).collect();
        s.dash = paged_compose::DashPattern::from_slice(&scaled);
        // Dotted patterns force round caps when the IDML didn't carry
        // an explicit `EndCap`, otherwise the zero-length on-segment
        // would render as a needle. Adobe previews behave the same.
        if is_dotted && end_cap.is_none() {
            s.cap = paged_compose::LineCap::Round;
        }
    }
    s
}

fn end_cap_from(name: Option<&str>) -> Option<paged_compose::LineCap> {
    match name? {
        "ButtEndCap" => Some(paged_compose::LineCap::Butt),
        "RoundEndCap" => Some(paged_compose::LineCap::Round),
        "ProjectingEndCap" => Some(paged_compose::LineCap::Square),
        _ => None,
    }
}

fn end_join_from(name: Option<&str>) -> Option<paged_compose::LineJoin> {
    match name? {
        "MiterEndJoin" => Some(paged_compose::LineJoin::Miter),
        "RoundEndJoin" => Some(paged_compose::LineJoin::Round),
        "BevelEndJoin" => Some(paged_compose::LineJoin::Bevel),
        _ => None,
    }
}

/// The tint to apply over swatch `id`'s resolved paint for an object's
/// tint of `tint_pct` (a `FillTint`, `StrokeTint`, rule or cell tint).
///
/// An object's tint is measured against the BASE colour of a `<Tint>`
/// swatch, not against the tint: asked of InDesign (`tints` case 59),
/// the teal 50 % `<Tint>` swatch at `FillTint="50"` exports teal at
/// 50 % (`0.425 0.05 0.2 0 k` for teal `85 10 40 0`), not at 25 %, and
/// its IDML re-export keeps `FillColor="Tint/Brand Teal 50%"
/// FillTint="50"`. The swatch's own value applies only when the object
/// carries no tint. Since the swatch's value is already folded into its
/// resolved paint ([`paged_model::ColorEntry::effective_cmyk`]), the
/// object's tint becomes the ratio of the two — above 100 when it is
/// the stronger.
pub(crate) fn object_tint(
    palette: &Graphic,
    id: Option<&str>,
    tint_pct: Option<f32>,
) -> Option<f32> {
    let t = tint_pct?;
    let swatch = id.and_then(|id| palette.resolve(id)).and_then(|e| e.tint);
    match swatch {
        Some(st) if st > 0.0 && st < 100.0 => Some(t * 100.0 / st),
        _ => Some(t),
    }
}

/// Scale a paint toward paper white per the IDML `FillTint`
/// percentage, with no colour context — the no-ICC path. See
/// [`apply_fill_tint_ctx`]; without a transform the CMYK arm keeps its
/// cached display colour's blend toward white.
#[cfg(test)]
pub(crate) fn apply_fill_tint(paint: Paint, tint_pct: Option<f32>) -> Paint {
    apply_fill_tint_ctx(paint, tint_pct, &ColorCtx::default())
}

/// Scale a paint toward paper white per the IDML `FillTint` /
/// `StrokeTint` / rule / shading tint percentage. `tint = 100` and
/// `None` are identity.
///
/// A tint of a CMYK colour is a tint of its INKS, and it prints the
/// same whichever way the document spells it. Asked of InDesign
/// (`corpus/generated/tints`, ADR 120): the object's `FillTint="40"` on
/// a full-strength swatch and a `<Tint TintValue="40">` swatch applied
/// at 100 % export the identical device colour — the base's CMYK
/// percentages scaled by the tint — for every process colour, rich
/// black and a spot's CMYK alternate. The page colour is that scaled
/// ink through the document CMYK profile. The `<Tint>` swatch already
/// took that route ([`paged_model::ColorEntry::effective_cmyk`] before
/// the profile); this function now does too, so the two agree.
///
/// The CMYK arm used to blend the cached DISPLAY colour toward white in
/// linear RGB — a different curve from the profile's, and visibly too
/// light in the mid tints under FOGRA39.
///
/// Without a transform (`color_ctx.icc == None`) CMYK swatches resolve
/// to [`Paint::Solid`] in the first place, and a stray
/// [`Paint::Cmyk`] keeps the display-colour blend: there is no
/// profile to send the scaled inks through.
pub(crate) fn apply_fill_tint_ctx(
    paint: Paint,
    tint_pct: Option<f32>,
    color_ctx: &ColorCtx<'_>,
) -> Paint {
    let Some(t) = tint_pct else {
        return paint;
    };
    // Above 1 only when [`object_tint`] lifts a tint swatch back toward
    // its base; the inks it scales never pass the base's.
    let t = (t / 100.0).max(0.0);
    if !t.is_finite() || (t - 1.0).abs() < f32::EPSILON {
        return paint;
    }
    match paint {
        // A tint is a percentage of INK on paper, and InDesign ramps it
        // in a perceptual space: measured on a Lab spot at
        // `FillTint="50"`, its export gives exactly the Lab midpoint
        // between the ink and paper (L=100, a=0, b=0).
        //
        //   ink Lab(30, 40, -55) -> sRGB (91, 44, 159)
        //   InDesign at 50%                 (175, 146, 207)
        //   Lab midpoint                    (175, 146, 207)   exact
        //   sRGB-device midpoint            (173, 150, 207)
        //   LINEAR-RGB midpoint             (196, 190, 214)   what we did
        //
        // Blending toward white in the pipeline's linear working space
        // is a full ΔE 8.6 out because linear RGB is the one space the
        // ramp is definitely not in.
        Paint::Solid(c) => Paint::Solid(lab_tint(c, t)),
        Paint::Cmyk {
            c,
            m,
            y,
            k,
            rgb,
            spot,
        } => {
            let rgb = match color_ctx.icc {
                // Under "Use Standard Lab Values for Spots" a Lab spot's
                // display colour is its measured Lab, not the alternate
                // through the profile; it tints in Lab like any other
                // measured colour.
                Some(xform) if !shows_its_lab(xform, color_ctx, [c, m, y, k], rgb) => {
                    let ink = |v: f32| (v * t).clamp(0.0, 1.0) * 100.0;
                    let paged_color::LinearRgb([r, g, b]) =
                        xform.cmyk_percent_to_linear_rgb(paged_color::Cmyk {
                            c: ink(c),
                            m: ink(m),
                            y: ink(y),
                            k: ink(k),
                        });
                    Color::rgba(r, g, b, rgb.a)
                }
                Some(_) => lab_tint(rgb, t),
                None => Color::rgba(
                    (1.0 + (rgb.r - 1.0) * t).clamp(0.0, 1.0),
                    (1.0 + (rgb.g - 1.0) * t).clamp(0.0, 1.0),
                    (1.0 + (rgb.b - 1.0) * t).clamp(0.0, 1.0),
                    rgb.a,
                ),
            };
            Paint::Cmyk {
                c: (c * t).clamp(0.0, 1.0),
                m: (m * t).clamp(0.0, 1.0),
                y: (y * t).clamp(0.0, 1.0),
                k: (k * t).clamp(0.0, 1.0),
                rgb,
                // Per-use FillTint preserves the spot identity. The spot
                // plane is tinted by the new C/M/Y/K (whose value is
                // already the tint-scaled CMYK alternate) — that's the
                // late-bound "PANTONE at N%" preview path.
                spot,
            }
        }
        other => other,
    }
}

/// `c` mixed toward paper white in Lab: paper is L=100, a=b=0; `t = 0`
/// is paper, `1` the colour.
fn lab_tint(c: Color, t: f32) -> Color {
    let [l, a, b] = paged_color::lab::linear_srgb_to_lab_d50([c.r, c.g, c.b]);
    let paged_color::LinearRgb([r, g, bl]) = paged_color::lab::lab_d50_to_linear_srgb(
        (100.0 + (l - 100.0) * t).clamp(0.0, 100.0),
        a * t,
        b * t,
    );
    Color::rgba(r, g, bl, c.a)
}

/// Whether a CMYK paint's cached display colour is NOT its inks through
/// the profile — the standard-Lab spot case, the only one
/// [`color_id_to_paint`] produces. Checked only with the Ink Manager
/// setting on, so the ordinary path pays nothing.
fn shows_its_lab(
    xform: &paged_color::IccTransform,
    color_ctx: &ColorCtx<'_>,
    [c, m, y, k]: [f32; 4],
    rgb: Color,
) -> bool {
    if !color_ctx.standard_lab_for_spots {
        return false;
    }
    let paged_color::LinearRgb([r, g, b]) = xform.cmyk_percent_to_linear_rgb(paged_color::Cmyk {
        c: c * 100.0,
        m: m * 100.0,
        y: y * 100.0,
        k: k * 100.0,
    });
    (r - rgb.r).abs() + (g - rgb.g).abs() + (b - rgb.b).abs() > 1e-3
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concept 2's load-bearing invariant: `PipelineOptions::default()`
    /// must reproduce the pre-Concept-2 hardcoded colour behaviour
    /// exactly (the fidelity corpus thresholds depend on it). The
    /// default intent/BPC route the transform build through the
    /// back-compat shim path.
    #[test]
    fn default_pipeline_options_preserve_the_legacy_transform_inputs() {
        let opts = crate::PipelineOptions::default();
        assert_eq!(opts.cmyk_intent, paged_color::Intent::RelativeColorimetric);
        assert!(opts.cmyk_bpc);
        assert!(opts.cmyk_icc_profile.is_none());
    }

    fn cmyk_entry(id: &str, cmyk: [f32; 4], tint: Option<f32>) -> paged_model::ColorEntry {
        paged_model::ColorEntry {
            self_id: id.into(),
            name: None,
            space: paged_model::ColorSpace::Cmyk,
            value: cmyk.to_vec(),
            model: paged_model::ColorModel::Process,
            alternate_space: None,
            alternate_value: vec![],
            tint,
            alpha: None,
        }
    }

    fn cmyk_parts(p: Paint) -> ([f32; 4], Color) {
        match p {
            Paint::Cmyk {
                c, m, y, k, rgb, ..
            } => ([c, m, y, k], rgb),
            other => panic!("expected a CMYK paint, got {other:?}"),
        }
    }

    /// Asked of InDesign (`corpus/generated/tints`, cases 1–54): an
    /// object's `FillTint` on a full-strength CMYK swatch and a `<Tint>`
    /// swatch of the same value export the identical device colour —
    /// the base's inks scaled by the tint — for process C, M, Y, K, a
    /// rich black and brand colours, at every value from 80 to 5. Under
    /// a profile the page colour is that scaled ink through it, so the
    /// two must paint identically. The `FillTint` path used to blend the
    /// full-strength DISPLAY colour toward white in linear RGB instead,
    /// which matched neither the swatch nor InDesign.
    #[test]
    fn fill_tint_equals_tint_swatch_under_icc() {
        let Some(profile) =
            paged_color::test_profiles::read_cmyk_profile(env!("CARGO_MANIFEST_DIR"))
        else {
            eprintln!("{}", paged_color::test_profiles::NO_PROFILE_HINT);
            return;
        };
        let xform = paged_color::IccTransform::cmyk_to_linear_rgb(&profile).expect("transform");
        let ctx = ColorCtx::icc(Some(&xform));
        let bases: [(&str, [f32; 4]); 6] = [
            ("Color/C", [100.0, 0.0, 0.0, 0.0]),
            ("Color/M", [0.0, 100.0, 0.0, 0.0]),
            ("Color/Y", [0.0, 0.0, 100.0, 0.0]),
            ("Color/K", [0.0, 0.0, 0.0, 100.0]),
            ("Color/Rich", [60.0, 40.0, 40.0, 100.0]),
            ("Color/Teal", [85.0, 10.0, 40.0, 0.0]),
        ];
        let mut palette = Graphic::default();
        for (id, cmyk) in bases {
            palette.colors.insert(id.into(), cmyk_entry(id, cmyk, None));
            for t in [80.0, 60.0, 40.0, 20.0, 5.0] {
                let tid = format!("{id}T{t}");
                palette
                    .colors
                    .insert(tid.clone(), cmyk_entry(&tid, cmyk, Some(t)));
            }
        }
        for (id, cmyk) in bases {
            let full = color_id_to_paint(id, &palette, ctx).expect("base");
            for t in [80.0, 60.0, 40.0, 20.0, 5.0] {
                let by_fill_tint =
                    apply_fill_tint_ctx(full, object_tint(&palette, Some(id), Some(t)), &ctx);
                let swatch = color_id_to_paint(&format!("{id}T{t}"), &palette, ctx).expect("tint");
                let (inks_a, rgb_a) = cmyk_parts(by_fill_tint);
                let (inks_b, rgb_b) = cmyk_parts(swatch);
                for ch in 0..4 {
                    assert!(
                        (inks_a[ch] - cmyk[ch] * t / 10_000.0).abs() < 1e-5,
                        "{id} at {t}: inks {inks_a:?}"
                    );
                    assert!((inks_a[ch] - inks_b[ch]).abs() < 1e-5);
                }
                for (a, b) in [(rgb_a.r, rgb_b.r), (rgb_a.g, rgb_b.g), (rgb_a.b, rgb_b.b)] {
                    assert!((a - b).abs() < 1e-5, "{id} at {t}: {rgb_a:?} vs {rgb_b:?}");
                }
            }
        }
    }

    /// An object's tint over a `<Tint>` swatch is measured against the
    /// swatch's BASE: InDesign exports the teal 50 % `<Tint>` at
    /// `FillTint="50"` as teal at 50 %, not 25 % (`tints` case 59), and
    /// at a stronger object tint the colour comes back toward the base.
    #[test]
    fn object_tint_over_a_tint_swatch_is_relative_to_its_base() {
        let teal = [85.0, 10.0, 40.0, 0.0];
        let mut palette = Graphic::default();
        palette
            .colors
            .insert("Color/Teal".into(), cmyk_entry("Color/Teal", teal, None));
        palette.colors.insert(
            "Color/T50".into(),
            cmyk_entry("Color/T50", teal, Some(50.0)),
        );
        assert_eq!(
            object_tint(&palette, Some("Color/T50"), Some(50.0)),
            Some(100.0)
        );
        assert_eq!(
            object_tint(&palette, Some("Color/T50"), Some(80.0)),
            Some(160.0)
        );
        assert_eq!(
            object_tint(&palette, Some("Color/Teal"), Some(80.0)),
            Some(80.0)
        );
        assert_eq!(object_tint(&palette, Some("Color/T50"), None), None);
        let Some(profile) =
            paged_color::test_profiles::read_cmyk_profile(env!("CARGO_MANIFEST_DIR"))
        else {
            eprintln!("{}", paged_color::test_profiles::NO_PROFILE_HINT);
            return;
        };
        let xform = paged_color::IccTransform::cmyk_to_linear_rgb(&profile).expect("transform");
        let ctx = ColorCtx::icc(Some(&xform));
        let swatch = color_id_to_paint("Color/T50", &palette, ctx).expect("tint");
        let at_80 = apply_fill_tint_ctx(
            swatch,
            object_tint(&palette, Some("Color/T50"), Some(80.0)),
            &ctx,
        );
        let (inks, _) = cmyk_parts(at_80);
        for ch in 0..4 {
            assert!((inks[ch] - teal[ch] * 0.8 / 100.0).abs() < 1e-5, "{inks:?}");
        }
    }

    /// Without a profile there are no inks to scale: a solid paint keeps
    /// the Lab ramp toward paper.
    #[test]
    fn fill_tint_without_a_profile_keeps_the_lab_ramp() {
        let ink = Color::rgba(0.1, 0.2, 0.6, 1.0);
        let Paint::Solid(half) = apply_fill_tint(Paint::Solid(ink), Some(50.0)) else {
            panic!("solid");
        };
        let [l0, a0, b0] = paged_color::lab::linear_srgb_to_lab_d50([ink.r, ink.g, ink.b]);
        let [l, a, b] = paged_color::lab::linear_srgb_to_lab_d50([half.r, half.g, half.b]);
        assert!((l - (100.0 + (l0 - 100.0) * 0.5)).abs() < 0.05);
        assert!((a - a0 * 0.5).abs() < 0.05 && (b - b0 * 0.5).abs() < 0.05);
        assert!(matches!(
            apply_fill_tint(Paint::Solid(ink), None),
            Paint::Solid(c) if c == ink
        ));
    }

    /// Lab swatches resolve analytically at the chokepoint — and
    /// every previously-resolvable space is untouched by the new
    /// fallthrough.
    #[test]
    fn lab_entry_resolves_to_a_solid_paint() {
        let entry = paged_model::ColorEntry {
            self_id: "Color/lab".into(),
            name: None,
            space: paged_model::ColorSpace::Lab,
            value: vec![50.0, 29.5, 5.2],
            model: paged_model::ColorModel::Process,
            alternate_space: None,
            alternate_value: vec![],
            tint: None,
            alpha: None,
        };
        let paint = lab_entry_to_paint(&entry).expect("lab resolves");
        match paint {
            Paint::Solid(c) => {
                assert!(c.r > 0.0 && c.r < 1.0, "in-gamut mid tone: {c:?}");
            }
            other => panic!("expected solid, got {other:?}"),
        }
        // Non-Lab entries fall through (the pre-existing paths own them).
        let cmyk = paged_model::ColorEntry {
            space: paged_model::ColorSpace::Cmyk,
            value: vec![0.0, 0.0, 0.0, 100.0],
            ..entry
        };
        assert!(lab_entry_to_paint(&cmyk).is_none());
    }
}
