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

/// Pick the paint for a frame from its FillColor attribute.
pub fn resolve_fill(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(frame.fill_color.as_deref()?, palette, None)
}

/// Same, for StrokeColor.
pub fn resolve_stroke(frame: &TextFrame, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(frame.stroke_color.as_deref()?, palette, None)
}

/// Rectangle flavour of `resolve_fill` (no ParentStory to consider).
pub fn resolve_rect_fill(rect: &Rectangle, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(rect.fill_color.as_deref()?, palette, None)
}

/// Rectangle flavour of `resolve_stroke`.
pub fn resolve_rect_stroke(rect: &Rectangle, palette: &Graphic) -> Option<Paint> {
    color_id_to_paint(rect.stroke_color.as_deref()?, palette, None)
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
/// When `cmyk_xform` is `None` (wasm32 fallback, hosts without an
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
    cmyk_xform: Option<&paged_color::IccTransform>,
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
            let p = color_id_to_paint(&s.stop_color, palette, cmyk_xform)?;
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

pub fn color_id_to_paint(
    id: &str,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
) -> Option<Paint> {
    let entry = palette.resolve(id)?;
    // Prefer the swatch's *effective* CMYK — it folds Spot →
    // alternate-CMYK resolution and any swatch-level `TintValue`
    // (e.g. "PANTONE 286 C at 50%") into the channels before ICC.
    // Tint scales each channel toward paper white (0,0,0,0) linearly
    // in CMYK space, which is what InDesign does in preview.
    if let (Some(xform), Some([c, m, y, k])) = (cmyk_xform, entry.effective_cmyk()) {
        {
            // ICC-resolve once at compose time and bake the result into
            // the paint. The rasterizer uses `rgb` for ordinary draws
            // (bit-identical to the pre-Stage-A path) and the
            // C/M/Y/K channels for overprint composition.
            // Backend: lcms2 on native, qcms on wasm32 (both behind
            // the `paged_color::IccTransform` shim).
            let cmyk = paged_color::Cmyk { c, m, y, k };
            let paged_color::LinearRgb([r, g, b]) = xform.cmyk_percent_to_linear_rgb(cmyk);
            return Some(Paint::Cmyk {
                c: (c / 100.0).clamp(0.0, 1.0),
                m: (m / 100.0).clamp(0.0, 1.0),
                y: (y / 100.0).clamp(0.0, 1.0),
                k: (k / 100.0).clamp(0.0, 1.0),
                rgb: Color::rgba(r, g, b, 1.0),
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
    if let Some([r, g, b]) = graphic::to_linear_rgb(entry) {
        return Some(Paint::Solid(Color::rgba(r, g, b, 1.0)));
    }
    // Concept 2 — Lab swatches. `to_linear_rgb` is the parse
    // layer's no-ICC stopgap and returns None for Lab; resolve here
    // analytically (Lab is device-independent: D50→D65 Bradford →
    // linear sRGB, no profile needed for display). Previously these
    // swatches dropped out entirely and rendered as missing paint.
    lab_entry_to_paint(entry)
}

/// Lab(D50) swatch → solid paint, or None when the entry isn't a
/// 3-channel Lab colour. Shared by the direct resolver above and
/// the canvas-side preview path.
pub fn lab_entry_to_paint(entry: &paged_parse::graphic::ColorEntry) -> Option<Paint> {
    if entry.space != paged_parse::graphic::ColorSpace::Lab || entry.value.len() != 3 {
        return None;
    }
    let paged_color::LinearRgb([r, g, b]) = paged_color::lab::lab_d50_to_linear_srgb(
        entry.value[0],
        entry.value[1],
        entry.value[2],
    );
    Some(Paint::Solid(Color::rgba(r, g, b, 1.0)))
}

/// Project an IDML gradient angle / length onto the path's
/// local 0..1 unit rect. Endpoints lie at `(0.5 ± h_x, 0.5 ± h_y)`
/// where the half-vector `(h_x, h_y)` is derived from the angle and
/// length:
///
/// * `angle_deg` — degrees CCW around the rect centre. IDML's
///   convention is 0° horizontal-right, 90° vertical-down (the page
///   y-axis points down, so a CCW rotation in screen-up coords reads
///   as CW on the page). Defaults to 0° when absent.
/// * `length_pt` — page-space length of the gradient line through the
///   centre. When `Some(L)` and a bbox is supplied, the half-vector is
///   `(cos θ · L / (2·w), sin θ · L / (2·h))` so the page-space line
///   length is exactly `L` regardless of the rect's aspect ratio.
///   When `None`, half-vector magnitude in unit-rect coords is `0.5`
///   along the angle direction — gradient runs edge-to-edge along the
///   cardinal axis (matches InDesign's swatch-panel default).
pub(super) fn linear_gradient_endpoints(
    angle_deg: Option<f32>,
    length_pt: Option<f32>,
    dims_pt: Option<(f32, f32)>,
) -> ((f32, f32), (f32, f32)) {
    let deg = angle_deg.unwrap_or(0.0);
    let rad = deg.to_radians();
    let (sin, cos) = rad.sin_cos();
    let (cx, cy) = (0.5_f32, 0.5_f32);
    let (hx, hy) = match (length_pt, dims_pt) {
        (Some(l), Some((w, h))) if w > 0.0 && h > 0.0 => {
            (cos * l / (2.0 * w), sin * l / (2.0 * h))
        }
        _ => {
            let half = 0.5_f32;
            (cos * half, sin * half)
        }
    };
    ((cx - hx, cy - hy), (cx + hx, cy + hy))
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
    cmyk_xform: Option<&paged_color::IccTransform>,
    list: &mut DisplayList,
) -> Option<Paint> {
    color_id_to_paint_with_list_dir(id, palette, cmyk_xform, list, None, None, None)
}

/// Like [`color_id_to_paint_with_list`] but takes an explicit
/// `gradient_angle_deg` from the frame's `GradientFillAngle` /
/// `GradientStrokeAngle` attribute (0° horizontal-right; 90°
/// vertical-down — IDML's convention), an explicit
/// `gradient_length_pt` from the matching `GradientFillLength` /
/// `GradientStrokeLength` attribute, and the path's local bbox
/// `(width, height)` in pt.
///
/// The bbox lets the radial-gradient default place its centre at the
/// path's bottom-left corner with radius equal to the diagonal —
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
/// `f == 0.5` exactly at `t == mid`. This is the standard PostScript /
/// SVG midpoint emulation `f = t^(ln 0.5 / ln mid)` — the *colour*
/// follows this curve while the stop's geometric `offset` stays linear
/// in `t`. A near-0.5 midpoint short-circuits to linear so the common
/// (no-midpoint) path is unchanged.
pub(super) fn midpoint_blend(t: f32, mid: f32) -> f32 {
    if (mid - 0.5).abs() < 1e-4 {
        return t;
    }
    let exponent = (0.5f32).ln() / mid.ln();
    t.powf(exponent)
}

pub(super) fn color_lerp(a: paged_compose::Color, b: paged_compose::Color, f: f32) -> paged_compose::Color {
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
    cmyk_xform: Option<&paged_color::IccTransform>,
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
            /// Midpoint (0..1) governing the blend curve toward the
            /// NEXT stop. 0.5 = linear (the default when omitted).
            midpoint: f32,
        }
        let raw_stops: Vec<StopRef> = grad
            .stops
            .iter()
            .filter_map(|s| {
                let color = color_id_to_paint(&s.stop_color, palette, cmyk_xform)
                    .and_then(|p| paint_as_solid_with_icc(p, cmyk_xform))?;
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
        let has_midpoint = raw_stops.iter().any(|s| (s.midpoint - 0.5).abs() > 1e-4);
        if raw_stops.len() < 2 {
            return None;
        }
        let stops: Vec<paged_compose::GradientStop> = if cmyk_xform.is_some()
            && raw_stops.iter().any(|s| s.cmyk.is_some())
        {
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
            let xform = cmyk_xform.unwrap();
            for win in raw_stops.windows(2) {
                let a = &win[0];
                let b = &win[1];
                let cmyk_a = a.cmyk.unwrap_or_else(|| rgb_to_cmyk_naive(a.color));
                let cmyk_b = b.cmyk.unwrap_or_else(|| rgb_to_cmyk_naive(b.color));
                for i in 0..SUB_STOPS {
                    let t = i as f32 / SUB_STOPS as f32;
                    // Geometry stays linear in `t`; the CMYK channel
                    // blend follows the midpoint curve.
                    let f = midpoint_blend(t, a.midpoint);
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
                    let f = midpoint_blend(t, a.midpoint);
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
        if matches!(grad.kind, paged_parse::GradientKind::Radial) {
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
        let id = list.push_linear_gradient(paged_compose::LinearGradient {
            start,
            end,
            stops,
        });
        return Some(Paint::LinearGradient(id));
    }
    let paint = color_id_to_paint(id, palette, cmyk_xform)?;
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
            if entry.model == paged_parse::ColorModel::Spot && entry.effective_cmyk().is_some() {
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
    paragraph: &paged_parse::Paragraph,
    palette: &Graphic,
    default: Paint,
) -> RunPaintPicker {
    build_run_paint_picker_with_cmyk(paragraph, palette, None, default)
}

/// Variant of [`build_run_paint_picker`] that routes CMYK swatches
/// through the document's ICC transform when one is available. Without
/// this the per-glyph fill picker would silently fall back to the
/// naive CMYK→sRGB approximation in `graphic::to_linear_rgb`, undoing
/// the work of building the transform.
pub fn build_run_paint_picker_with_cmyk(
    paragraph: &paged_parse::Paragraph,
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
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
                color_id_to_paint(id, palette, cmyk_xform)
                    .or_else(|| gradient_midpoint_paint(id, palette, cmyk_xform))
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
    paragraph: &paged_parse::Paragraph,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
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
            .and_then(|id| color_id_to_paint(id, palette, cmyk_xform))
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
    paragraph: &paged_parse::Paragraph,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    palette: &Graphic,
    cmyk_xform: Option<&paged_color::IccTransform>,
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
                color_id_to_paint(id, palette, cmyk_xform)
                    .or_else(|| gradient_midpoint_paint(id, palette, cmyk_xform))
            })
            .unwrap_or(default);
        let paint = apply_fill_tint(base, resolved_runs[i].fill_tint);
        bands.push((cursor, paint));
        cursor += run.text.len() as u32;
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
pub(crate) fn stroke_for(
    stroke_type: Option<&str>,
    width: f32,
    end_cap: Option<&str>,
    end_join: Option<&str>,
    miter_limit: Option<f32>,
    stroke_styles: Option<&std::collections::BTreeMap<String, paged_parse::StrokeStyleDef>>,
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
            use paged_parse::StrokeStyleKind as K;
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
                // lands in the rasterizer (tracked in renderer-gaps.md).
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
    let normalised = suffix
        .strip_prefix("Canned ")
        .unwrap_or(suffix);
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
        // capability (deferred; see renderer-gaps.md).
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

/// Scale a paint toward paper white per the IDML `FillTint`
/// percentage. `tint = 100` is identity; lower values blend toward
/// white in linear-RGB space, matching InDesign's preview behaviour.
/// `None` returns the input unchanged. Only applied to solid paints
/// today — gradient stops are left as-is until the gradient
/// resolution itself learns about per-stop tints.
///
/// For [`Paint::Cmyk`] the tint scales each channel toward 0 (paper
/// white in CMYK) — matching the swatch-level `TintValue` semantics
/// `ColorEntry::effective_cmyk` already applies before we get here.
/// This keeps run-level `FillTint` tinting consistent across the
/// CMYK and RGB swatch paths.
pub(crate) fn apply_fill_tint(paint: Paint, tint_pct: Option<f32>) -> Paint {
    let Some(t) = tint_pct else {
        return paint;
    };
    let t = (t / 100.0).clamp(0.0, 1.0);
    if (t - 1.0).abs() < f32::EPSILON {
        return paint;
    }
    match paint {
        Paint::Solid(c) => Paint::Solid(Color::rgba(
            1.0 + (c.r - 1.0) * t,
            1.0 + (c.g - 1.0) * t,
            1.0 + (c.b - 1.0) * t,
            c.a,
        )),
        Paint::Cmyk { c, m, y, k, rgb, spot } => Paint::Cmyk {
            c: c * t,
            m: m * t,
            y: y * t,
            k: k * t,
            // Tint the cached display RGB in step — same blend toward
            // paper white as the `Paint::Solid` arm so the visible
            // result for non-overprint draws stays consistent.
            rgb: Color::rgba(
                1.0 + (rgb.r - 1.0) * t,
                1.0 + (rgb.g - 1.0) * t,
                1.0 + (rgb.b - 1.0) * t,
                rgb.a,
            ),
            // Per-use FillTint preserves the spot identity. The spot
            // plane is tinted by the new C/M/Y/K (whose value is
            // already the tint-scaled CMYK alternate) — that's the
            // late-bound "PANTONE at N%" preview path.
            spot,
        },
        other => other,
    }
}
