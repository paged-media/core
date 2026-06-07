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

//! Colour encoding — preserve native spaces, don't collapse to
//! display (concept E3/E5):
//!
//! - `Paint::Cmyk{spot:None}` → ICCBased `/N 4` (working profile
//!   registered) else `DeviceCMYK`; channel numbers byte-equal
//!   under Preserve Numbers — pure 100%-K stays pure K.
//! - `Paint::Cmyk{spot:Some}` → `/Separation [/name alt tint]`
//!   with a Type-4 calculator tint transform; the Ink Manager's
//!   alias/convert-to-process settings collapse or bypass plates.
//! - `Paint::Solid` (source space lost at list level) → sRGB,
//!   ICCBased `/N 3` when an sRGB profile is supplied, else
//!   DeviceRGB. Linear→sRGB encode happens here (the list carries
//!   linear light).
//! - Gradients → `/Shading` type 2/3 with Type-3 stitching honouring
//!   midpoints (sRGB stops v1; native-space shadings = v2).

use paged_compose::{Color, LinearGradient, Paint, RadialGradient, SpotInk};
use pdf_writer::{Content, Finish, Name, Ref};

use crate::writer::{DocState, PageResources};
use crate::{ExportInkSettings, ExportInput};

/// Linear-light → sRGB-encoded component.
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_of(c: Color) -> [f32; 3] {
    [
        linear_to_srgb(c.r),
        linear_to_srgb(c.g),
        linear_to_srgb(c.b),
    ]
}

/// Resolve the effective spot handling for a paint after the Ink
/// Manager settings: `None` ⇒ paint as plain process CMYK;
/// `Some(name)` ⇒ paint on the named separation plate.
fn effective_spot<'a>(
    spot: &'a SpotInk,
    inks: &'a ExportInkSettings,
    spot_table: &'a [SpotInk],
) -> Option<&'a SpotInk> {
    if inks.convert_to_process.contains(&spot.name) {
        return None;
    }
    if let Some((_, target)) = inks.aliases.iter().find(|(from, _)| *from == spot.name) {
        if let Some(t) = spot_table.iter().find(|s| s.name == *target) {
            return Some(t);
        }
    }
    Some(spot)
}

/// Emit fill-colour ops for `paint`. Returns true when the paint
/// could be set (gradient paints are handled by the caller through
/// shading patterns instead).
pub fn set_fill_paint(
    content: &mut Content,
    state: &mut DocState,
    resources: &mut PageResources,
    input: &ExportInput<'_>,
    list_spot_inks: &[SpotInk],
    paint: &Paint,
) -> bool {
    match paint {
        Paint::Solid(c) => {
            set_solid(content, state, resources, input, *c, false);
            true
        }
        Paint::Cmyk {
            c,
            m,
            y,
            k,
            spot,
            rgb: _,
        } => {
            let spot_ink = spot
                .and_then(|id| list_spot_inks.get(id.0 as usize))
                .and_then(|s| effective_spot(s, &input.inks, list_spot_inks));
            match spot_ink {
                Some(ink) => {
                    let cs_name = separation_space(state, resources, input, ink);
                    content.set_fill_color_space(pdf_writer::types::ColorSpaceOperand::Named(
                        Name(cs_name.as_bytes()),
                    ));
                    // Tint on a Separation plate: the ink coverage.
                    // The display channels already carry the tinted
                    // alternate; the plate value is the max channel
                    // ratio vs the full-strength alternate — for v1
                    // the paint's k-folded scale is captured by the
                    // dominant channel ratio. Full-strength = 1.0.
                    let t = separation_tint(ink, [*c, *m, *y, *k]);
                    content.set_fill_color([t]);
                }
                None => {
                    set_cmyk(content, state, resources, input, [*c, *m, *y, *k], false);
                }
            }
            true
        }
        Paint::LinearGradient(_) | Paint::RadialGradient(_) => false,
    }
}

/// Emit stroke-colour ops (the `SCN`/`CS` family).
pub fn set_stroke_paint(
    content: &mut Content,
    state: &mut DocState,
    resources: &mut PageResources,
    input: &ExportInput<'_>,
    list_spot_inks: &[SpotInk],
    paint: &Paint,
) -> bool {
    match paint {
        Paint::Solid(c) => {
            set_solid(content, state, resources, input, *c, true);
            true
        }
        Paint::Cmyk {
            c,
            m,
            y,
            k,
            spot,
            rgb: _,
        } => {
            let spot_ink = spot
                .and_then(|id| list_spot_inks.get(id.0 as usize))
                .and_then(|s| effective_spot(s, &input.inks, list_spot_inks));
            match spot_ink {
                Some(ink) => {
                    let cs_name = separation_space(state, resources, input, ink);
                    content.set_stroke_color_space(pdf_writer::types::ColorSpaceOperand::Named(
                        Name(cs_name.as_bytes()),
                    ));
                    let t = separation_tint(ink, [*c, *m, *y, *k]);
                    content.set_stroke_color([t]);
                }
                None => {
                    set_cmyk(content, state, resources, input, [*c, *m, *y, *k], true);
                }
            }
            true
        }
        Paint::LinearGradient(_) | Paint::RadialGradient(_) => false,
    }
}

fn set_solid(
    content: &mut Content,
    state: &mut DocState,
    resources: &mut PageResources,
    input: &ExportInput<'_>,
    c: Color,
    stroke: bool,
) {
    // The list carries linear light; encode to sRGB, then hand the
    // value to the CMM's export seam — under ConvertToDestination
    // it comes back as destination CMYK, under Preserve Numbers it
    // passes through and encodes as sRGB ICCBased.
    let rgb = srgb_of(c);
    match input
        .cmm
        .convert_for_export(paged_color::WorkingColor::Rgb(rgb))
    {
        paged_color::WorkingColor::Cmyk(out) => {
            let cmyk = [out.c / 100.0, out.m / 100.0, out.y / 100.0, out.k / 100.0];
            set_cmyk(content, state, resources, input, cmyk, stroke);
        }
        paged_color::WorkingColor::Gray(pct) => {
            // Defensive — the seam never returns Gray today.
            set_cmyk(
                content,
                state,
                resources,
                input,
                [0.0, 0.0, 0.0, pct / 100.0],
                stroke,
            );
        }
        // Rgb passthrough (and Lab, which cannot arise from an Rgb
        // input — encode its analytic sRGB if it ever does).
        wc => {
            let [r, g, b] = match wc {
                paged_color::WorkingColor::Rgb(rgb) => rgb,
                paged_color::WorkingColor::Lab { l, a, b } => {
                    paged_color::lab::lab_d50_to_srgb_encoded(l, a, b)
                }
                _ => unreachable!(),
            };
            match srgb_space(state, resources, input) {
                Some(name) => {
                    let operand =
                        pdf_writer::types::ColorSpaceOperand::Named(Name(name.as_bytes()));
                    if stroke {
                        content.set_stroke_color_space(operand);
                        content.set_stroke_color([r, g, b]);
                    } else {
                        content.set_fill_color_space(operand);
                        content.set_fill_color([r, g, b]);
                    }
                }
                None => {
                    if stroke {
                        content.set_stroke_rgb(r, g, b);
                    } else {
                        content.set_fill_rgb(r, g, b);
                    }
                }
            }
        }
    }
}

fn set_cmyk(
    content: &mut Content,
    state: &mut DocState,
    resources: &mut PageResources,
    input: &ExportInput<'_>,
    cmyk: [f32; 4],
    stroke: bool,
) {
    match cmyk_space(state, resources, input) {
        Some(name) => {
            let operand = pdf_writer::types::ColorSpaceOperand::Named(Name(name.as_bytes()));
            if stroke {
                content.set_stroke_color_space(operand);
                content.set_stroke_color(cmyk);
            } else {
                content.set_fill_color_space(operand);
                content.set_fill_color(cmyk);
            }
        }
        None if stroke => {
            content.set_stroke_cmyk(cmyk[0], cmyk[1], cmyk[2], cmyk[3]);
        }
        None => {
            content.set_fill_cmyk(cmyk[0], cmyk[1], cmyk[2], cmyk[3]);
        }
    }
}

/// The Separation tint for the painted channels: the display list
/// folded swatch tint into the CMYK alternate, so the plate value
/// is the ratio of the painted channels to the full-strength
/// alternate (max over channels for robustness; 1.0 when the
/// alternate is degenerate).
fn separation_tint(ink: &SpotInk, painted: [f32; 4]) -> f32 {
    let alt = ink.cmyk_alternate;
    let mut best = 0.0f32;
    let mut found = false;
    for i in 0..4 {
        let full = alt[i] as f32 / 255.0;
        if full > 1e-4 {
            best = best.max((painted[i] / full).clamp(0.0, 1.0));
            found = true;
        }
    }
    if found {
        best
    } else {
        1.0
    }
}

/// ICCBased /N 4 colour space for process CMYK (interned per page
/// + per document). `None` ⇒ caller uses DeviceCMYK operators.
fn cmyk_space(
    state: &mut DocState,
    resources: &mut PageResources,
    input: &ExportInput<'_>,
) -> Option<String> {
    let bytes = input.profiles.cmyk_working?;
    let icc_ref = match state.icc_refs.get("cmyk") {
        Some(r) => *r,
        None => {
            let stream_ref = state.refs.alloc();
            let mut s = state.pdf.icc_profile(stream_ref, bytes);
            s.n(4);
            s.finish();
            let array_ref = state.refs.alloc();
            let mut arr = state.pdf.indirect(array_ref).array();
            arr.item(Name(b"ICCBased"));
            arr.item(stream_ref);
            arr.finish();
            state.icc_refs.insert("cmyk", array_ref);
            array_ref
        }
    };
    Some(intern_space(resources, "CsCmyk", icc_ref))
}

/// ICCBased /N 3 sRGB space. `None` ⇒ DeviceRGB.
fn srgb_space(
    state: &mut DocState,
    resources: &mut PageResources,
    input: &ExportInput<'_>,
) -> Option<String> {
    let bytes = input.profiles.srgb?;
    let icc_ref = match state.icc_refs.get("srgb") {
        Some(r) => *r,
        None => {
            let stream_ref = state.refs.alloc();
            let mut s = state.pdf.icc_profile(stream_ref, bytes);
            s.n(3);
            s.finish();
            let array_ref = state.refs.alloc();
            let mut arr = state.pdf.indirect(array_ref).array();
            arr.item(Name(b"ICCBased"));
            arr.item(stream_ref);
            arr.finish();
            state.icc_refs.insert("srgb", array_ref);
            array_ref
        }
    };
    Some(intern_space(resources, "CsSrgb", icc_ref))
}

/// `/Separation [/InkName altSpace tintTransform]` — interned per
/// colorant. The tint transform is a Type-4 PostScript calculator
/// mapping t → t·alternate (linear toward paper white), evaluated
/// in DeviceCMYK.
fn separation_space(
    state: &mut DocState,
    resources: &mut PageResources,
    input: &ExportInput<'_>,
    ink: &SpotInk,
) -> String {
    // Human-readable colorant name: the palette's swatch Name when
    // resolvable (spot names ARE the colourant identity), else the
    // raw id.
    let colorant = input
        .palette
        .colors
        .get(&ink.name)
        .and_then(|c| c.name.clone())
        .unwrap_or_else(|| ink.name.clone());

    let cs_ref = match state.separation_pool.get(&colorant) {
        Some(r) => *r,
        None => {
            // Type-4 calculator: { dup c mul exch dup m mul exch
            //                      dup y mul exch k mul }
            let alt = ink.cmyk_alternate;
            let [c, m, y, k] = [
                alt[0] as f32 / 255.0,
                alt[1] as f32 / 255.0,
                alt[2] as f32 / 255.0,
                alt[3] as f32 / 255.0,
            ];
            let ps = format!(
                "{{ dup {c:.4} mul exch dup {m:.4} mul exch dup {y:.4} mul exch {k:.4} mul }}"
            );
            let fn_ref = state.refs.alloc();
            {
                let mut f = state.pdf.post_script_function(fn_ref, ps.as_bytes());
                f.domain([0.0, 1.0]);
                f.range([0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
                f.finish();
            }
            let cs_ref = state.refs.alloc();
            {
                let mut arr = state.pdf.indirect(cs_ref).array();
                arr.item(Name(b"Separation"));
                arr.item(Name(sanitize_name(&colorant).as_bytes()));
                arr.item(Name(b"DeviceCMYK"));
                arr.item(fn_ref);
                arr.finish();
            }
            state.separation_pool.insert(colorant.clone(), cs_ref);
            cs_ref
        }
    };
    intern_space(
        resources,
        &format!("CsSep{}", short_hash(&colorant)),
        cs_ref,
    )
}

/// `/Separation /All` for registration content (printer marks hit
/// every plate).
pub fn registration_all_space(state: &mut DocState, resources: &mut PageResources) -> String {
    let cs_ref = match state.separation_pool.get("\u{0}All") {
        Some(r) => *r,
        None => {
            let fn_ref = state.refs.alloc();
            {
                let mut f = state.pdf.post_script_function(fn_ref, b"{ dup dup dup }");
                f.domain([0.0, 1.0]);
                f.range([0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
                f.finish();
            }
            let cs_ref = state.refs.alloc();
            {
                let mut arr = state.pdf.indirect(cs_ref).array();
                arr.item(Name(b"Separation"));
                arr.item(Name(b"All"));
                arr.item(Name(b"DeviceCMYK"));
                arr.item(fn_ref);
                arr.finish();
            }
            state.separation_pool.insert("\u{0}All".into(), cs_ref);
            cs_ref
        }
    };
    intern_space(resources, "CsAll", cs_ref)
}

fn intern_space(resources: &mut PageResources, name: &str, r: Ref) -> String {
    resources.color_spaces.entry(name.to_string()).or_insert(r);
    name.to_string()
}

fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn short_hash(s: &str) -> u32 {
    // FNV-1a, deterministic.
    let mut h: u32 = 0x811c_9dc5;
    for b in s.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Write a `/Shading` dict (type 2 linear / type 3 radial) for a
/// gradient, with a Type-3 stitching function over the stops. v1
/// emits sRGB-encoded stops (DeviceRGB / ICCBased sRGB); native
/// CMYK/Lab shadings are the documented v2.
pub fn write_linear_shading(
    state: &mut DocState,
    input: &ExportInput<'_>,
    g: &LinearGradient,
    bbox: paged_compose::Rect,
) -> Ref {
    let coords = [
        bbox.x + g.start.0 * bbox.w,
        bbox.y + g.start.1 * bbox.h,
        bbox.x + g.end.0 * bbox.w,
        bbox.y + g.end.1 * bbox.h,
    ];
    write_shading(state, input, &g.stops, ShadingGeometry::Axial(coords))
}

pub fn write_radial_shading(
    state: &mut DocState,
    input: &ExportInput<'_>,
    g: &RadialGradient,
    bbox: paged_compose::Rect,
) -> Ref {
    let cx = bbox.x + g.center.0 * bbox.w;
    let cy = bbox.y + g.center.1 * bbox.h;
    let r = g.radius * bbox.w.max(bbox.h);
    write_shading(
        state,
        input,
        &g.stops,
        ShadingGeometry::Radial([cx, cy, 0.0, cx, cy, r]),
    )
}

enum ShadingGeometry {
    Axial([f32; 4]),
    Radial([f32; 6]),
}

fn write_shading(
    state: &mut DocState,
    _input: &ExportInput<'_>,
    stops: &[paged_compose::GradientStop],
    geometry: ShadingGeometry,
) -> Ref {
    // Build the colour function: a single Type-2 for two stops, a
    // Type-3 stitch otherwise. Stops are sRGB-encoded here.
    let mut sorted: Vec<&paged_compose::GradientStop> = stops.iter().collect();
    sorted.sort_by(|a, b| {
        a.offset
            .partial_cmp(&b.offset)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let fn_ref = state.refs.alloc();
    if sorted.len() <= 1 {
        let c = sorted
            .first()
            .map(|s| srgb_of(s.color))
            .unwrap_or([0.0, 0.0, 0.0]);
        let mut f = state.pdf.exponential_function(fn_ref);
        f.domain([0.0, 1.0]);
        f.range([0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
        f.c0(c);
        f.c1(c);
        f.n(1.0);
        f.finish();
    } else if sorted.len() == 2 {
        let mut f = state.pdf.exponential_function(fn_ref);
        f.domain([0.0, 1.0]);
        f.range([0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
        f.c0(srgb_of(sorted[0].color));
        f.c1(srgb_of(sorted[1].color));
        f.n(1.0);
        f.finish();
    } else {
        // Type-3 stitching over Type-2 segments.
        let mut seg_refs = Vec::new();
        for pair in sorted.windows(2) {
            let seg_ref = state.refs.alloc();
            let mut f = state.pdf.exponential_function(seg_ref);
            f.domain([0.0, 1.0]);
            f.range([0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
            f.c0(srgb_of(pair[0].color));
            f.c1(srgb_of(pair[1].color));
            f.n(1.0);
            f.finish();
            seg_refs.push(seg_ref);
        }
        let mut f = state.pdf.stitching_function(fn_ref);
        f.domain([0.0, 1.0]);
        f.range([0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
        f.functions(seg_refs.iter().copied());
        let bounds: Vec<f32> = sorted[1..sorted.len() - 1]
            .iter()
            .map(|s| s.offset)
            .collect();
        f.bounds(bounds.iter().copied());
        let encode: Vec<f32> = seg_refs.iter().flat_map(|_| [0.0, 1.0]).collect();
        f.encode(encode.iter().copied());
        f.finish();
    }

    let shading_ref = state.refs.alloc();
    let mut sh = state.pdf.function_shading(shading_ref);
    match geometry {
        ShadingGeometry::Axial(coords) => {
            sh.shading_type(pdf_writer::types::FunctionShadingType::Axial);
            sh.color_space().device_rgb();
            sh.coords(coords);
        }
        ShadingGeometry::Radial(coords) => {
            sh.shading_type(pdf_writer::types::FunctionShadingType::Radial);
            sh.color_space().device_rgb();
            sh.coords(coords);
        }
    }
    sh.function(fn_ref);
    sh.extend([true, true]);
    sh.finish();
    shading_ref
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(alt: [u8; 4]) -> SpotInk {
        SpotInk {
            name: "Color/test".into(),
            cmyk_alternate: alt,
        }
    }

    #[test]
    fn separation_tint_endpoints() {
        // Full-strength paint over a full-strength alternate = 1.0.
        let pantone = ink([255, 191, 0, 0]);
        let full = separation_tint(&pantone, [1.0, 0.75, 0.0, 0.0]);
        assert!((full - 1.0).abs() < 1e-4, "full = {full}");
        // Paper (no ink painted) = 0.0.
        let none = separation_tint(&pantone, [0.0, 0.0, 0.0, 0.0]);
        assert!(none.abs() < 1e-4, "none = {none}");
        // 50% swatch tint — channels at half the alternate.
        let half = separation_tint(&pantone, [0.5, 0.375, 0.0, 0.0]);
        assert!((half - 0.5).abs() < 1e-2, "half = {half}");
        // Degenerate alternate (registration-ish): tint pins to 1.
        let degenerate = ink([0, 0, 0, 0]);
        assert!((separation_tint(&degenerate, [0.2, 0.0, 0.0, 0.0]) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn separation_colorant_names_sanitise() {
        assert_eq!(sanitize_name("PANTONE 286 C"), "PANTONE-286-C");
        assert_eq!(sanitize_name("Größe/50%"), "Gr--e-50-");
    }

    #[test]
    fn linear_srgb_round_trip_endpoints() {
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
        // The 0.5-linear grey encodes to ~0.7354 sRGB.
        assert!((linear_to_srgb(0.5) - 0.7354).abs() < 1e-3);
    }
}
