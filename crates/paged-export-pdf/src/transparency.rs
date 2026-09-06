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

//! Live PDF transparency (the whole reason to target X-4): blend
//! modes + constant alpha as interned ExtGStates, overprint
//! graphics states, and raster soft-mask stamps for the blur-based
//! effects (drop shadow / glow) — which is exactly what InDesign's
//! own exports do: a pre-blurred raster luminosity mask is the
//! standard PDF encoding of a shadow, NOT a fidelity compromise.

use paged_compose::{BlendMode, Color, DropShadow, PathData, Transform};
use pdf_writer::{Content, Finish, Name, Ref};

use crate::writer::{DocState, PageResources};

fn blend_name(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "Normal",
        BlendMode::Multiply => "Multiply",
        BlendMode::Screen => "Screen",
        BlendMode::Overlay => "Overlay",
        BlendMode::Darken => "Darken",
        BlendMode::Lighten => "Lighten",
        BlendMode::ColorDodge => "ColorDodge",
        BlendMode::ColorBurn => "ColorBurn",
        BlendMode::HardLight => "HardLight",
        BlendMode::SoftLight => "SoftLight",
        BlendMode::Difference => "Difference",
        BlendMode::Exclusion => "Exclusion",
        BlendMode::Hue => "Hue",
        BlendMode::Saturation => "Saturation",
        BlendMode::Color => "Color",
        BlendMode::Luminosity => "Luminosity",
    }
}

/// Intern an ExtGState for (blend, alpha, overprint) and emit the
/// `gs` op. The pool is document-wide; the resource name is derived
/// from the canonical key so it's deterministic.
pub fn apply_gs(
    content: &mut Content,
    state: &mut DocState,
    resources: &mut PageResources,
    blend: Option<BlendMode>,
    alpha: Option<f32>,
    overprint: bool,
) {
    let blend = blend.filter(|b| *b != BlendMode::Normal);
    let alpha = alpha.filter(|a| *a < 0.9999);
    if blend.is_none() && alpha.is_none() && !overprint {
        return;
    }
    let key = format!(
        "B{}A{}O{}",
        blend.map(blend_name).unwrap_or("-"),
        alpha
            .map(|a| format!("{:.3}", a))
            .unwrap_or_else(|| "-".into()),
        overprint as u8,
    );
    let gs_ref = match state.gs_pool.get(&key) {
        Some(r) => *r,
        None => {
            let r = state.refs.alloc();
            let mut gs = state.pdf.ext_graphics(r);
            if let Some(b) = blend {
                gs.blend_mode(pdf_blend(b));
            }
            if let Some(a) = alpha {
                gs.non_stroking_alpha(a);
                gs.stroking_alpha(a);
            }
            if overprint {
                gs.overprint(true);
                gs.overprint_fill(true);
                gs.overprint_mode(pdf_writer::types::OverprintMode::IgnoreZeroChannel);
            }
            gs.finish();
            state.gs_pool.insert(key.clone(), r);
            r
        }
    };
    let name = format!("Gs{}", short_hash(&key));
    resources.ext_g_states.entry(name.clone()).or_insert(gs_ref);
    content.set_parameters(Name(name.as_bytes()));
}

fn pdf_blend(mode: BlendMode) -> pdf_writer::types::BlendMode {
    use pdf_writer::types::BlendMode as P;
    match mode {
        BlendMode::Normal => P::Normal,
        BlendMode::Multiply => P::Multiply,
        BlendMode::Screen => P::Screen,
        BlendMode::Overlay => P::Overlay,
        BlendMode::Darken => P::Darken,
        BlendMode::Lighten => P::Lighten,
        BlendMode::ColorDodge => P::ColorDodge,
        BlendMode::ColorBurn => P::ColorBurn,
        BlendMode::HardLight => P::HardLight,
        BlendMode::SoftLight => P::SoftLight,
        BlendMode::Difference => P::Difference,
        BlendMode::Exclusion => P::Exclusion,
        BlendMode::Hue => P::Hue,
        BlendMode::Saturation => P::Saturation,
        BlendMode::Color => P::Color,
        BlendMode::Luminosity => P::Luminosity,
    }
}

/// The effect-coverage rasters live in `paged-compose` so the PDF
/// lane, the CPU rasterizer and the GPU lane run the same arithmetic;
/// re-exported here because every call site in this crate reads them
/// as part of the transparency machinery.
pub use paged_compose::mask::{
    directional_feather_mask, feather_mask, remap_choke, Grid, MaskRaster,
};

fn short_hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// A rasterised blurred-alpha stamp in the path's page space,
/// expanded by 3σ.
pub struct AlphaStamp {
    pub alpha: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
    pub origin_pt: (f32, f32),
    pub size_pt: (f32, f32),
}

/// Rasterise a blurred alpha stamp for a path (the shadow/glow
/// encoding): scanline-fill the path's alpha at `dpi`, then a
/// separable box-approximated Gaussian (3 passes ≈ true Gaussian).
pub fn blurred_alpha_stamp(
    path: &PathData,
    transform: &Transform,
    blur_radius_pt: f32,
    dpi: f32,
) -> Option<AlphaStamp> {
    // σ = half the IDML `Size`, which is what InDesign itself draws:
    // its shadow reads Φ(−1) at Size/2 outside the outline and Φ(−2)
    // at Size. This lane always had it right; the CPU rasterizer took
    // σ = Size and drew shadows twice as soft, and now shares this.
    let sigma_pt = paged_compose::mask::outer_sigma_pt(blur_radius_pt.max(0.01));
    let grid = Grid::for_path(path, transform, sigma_pt * 3.0, dpi)?;
    let mut mask = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    mask.blur(sigma_pt);
    Some(AlphaStamp {
        alpha: mask.data,
        width_px: grid.width_px,
        height_px: grid.height_px,
        origin_pt: grid.origin_pt,
        size_pt: grid.size_pt,
    })
}

/// Emit a drop shadow as the standard PDF encoding: a coloured rect
/// painted through a blurred-alpha /SMask'd image XObject, offset
/// from the path. Returns the resource (name, ref) used.
#[allow(clippy::too_many_arguments)]
/// Write a mask as an 8-bit DeviceGray Flate image and return its ref.
pub fn write_gray_mask_image(state: &mut DocState, mask: &MaskRaster) -> Ref {
    let compressed = {
        use std::io::Write as _;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = enc.write_all(&mask.data);
        enc.finish().unwrap_or_default()
    };
    let mask_ref = state.refs.alloc();
    let mut x = state.pdf.image_xobject(mask_ref, &compressed);
    x.width(mask.grid.width_px as i32);
    x.height(mask.grid.height_px as i32);
    x.bits_per_component(8);
    x.color_space().device_gray();
    x.filter(pdf_writer::Filter::FlateDecode);
    x.interpolate(true);
    x.finish();
    mask_ref
}

/// A 1×1 solid-colour image wearing `mask_ref` as its `/SMask`, interned
/// on the page. Returns its resource name.
pub fn write_tinted_stamp(
    state: &mut DocState,
    resources: &mut PageResources,
    mask_ref: Ref,
    color: Color,
    xobject_counter: &mut u32,
) -> String {
    let px = [
        (crate::color::linear_to_srgb(color.r) * 255.0) as u8,
        (crate::color::linear_to_srgb(color.g) * 255.0) as u8,
        (crate::color::linear_to_srgb(color.b) * 255.0) as u8,
    ];
    let fill_ref = state.refs.alloc();
    {
        let mut x = state.pdf.image_xobject(fill_ref, &px);
        x.width(1);
        x.height(1);
        x.bits_per_component(8);
        x.color_space().device_rgb();
        x.s_mask(mask_ref);
        x.finish();
    }
    let name = format!("Xs{}", *xobject_counter);
    *xobject_counter += 1;
    resources.x_objects.insert(name.clone(), fill_ref);
    name
}

/// Paint a stamp over its grid's rectangle, offset by `offset_pt`.
///
/// Image XObjects paint into the unit square; this scales to the grid's
/// bounds and flips vertically, because the mask's rows are y-down like
/// the page's content space while the unit square is y-up.
pub fn place_stamp(content: &mut Content, grid: &Grid, name: &str, offset_pt: (f32, f32)) {
    content.save_state();
    content.transform([
        grid.size_pt.0,
        0.0,
        0.0,
        -grid.size_pt.1,
        grid.origin_pt.0 + offset_pt.0,
        grid.origin_pt.1 + offset_pt.1 + grid.size_pt.1,
    ]);
    content.x_object(Name(name.as_bytes()));
    content.restore_state();
}

/// Write a mask, tint it and paint it — the three steps every stamped
/// effect shares.
pub fn stamp_mask(
    content: &mut Content,
    state: &mut DocState,
    resources: &mut PageResources,
    mask: &MaskRaster,
    color: Color,
    offset_pt: (f32, f32),
    xobject_counter: &mut u32,
) {
    if mask.is_empty() {
        return;
    }
    let mask_ref = write_gray_mask_image(state, mask);
    let name = write_tinted_stamp(state, resources, mask_ref, color, xobject_counter);
    place_stamp(content, &mask.grid, &name, offset_pt);
}

pub fn emit_shadow_stamp(
    content: &mut Content,
    state: &mut DocState,
    resources: &mut PageResources,
    path: &PathData,
    transform: &Transform,
    shadow: &DropShadow,
    effect_dpi: f32,
    xobject_counter: &mut u32,
) {
    let Some(AlphaStamp {
        alpha,
        width_px: w,
        height_px: h,
        origin_pt: origin,
        size_pt,
    }) = blurred_alpha_stamp(path, transform, shadow.blur_radius, effect_dpi)
    else {
        return;
    };
    // Modulate by the shadow opacity at stamp level.
    let opacity = shadow.opacity.clamp(0.0, 1.0);
    let data: Vec<u8> = if opacity < 0.999 {
        alpha.iter().map(|a| (*a as f32 * opacity) as u8).collect()
    } else {
        alpha
    };
    let compressed = {
        use std::io::Write as _;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = enc.write_all(&data);
        enc.finish().unwrap_or_default()
    };
    let mask_ref = state.refs.alloc();
    {
        let mut x = state.pdf.image_xobject(mask_ref, &compressed);
        x.width(w as i32);
        x.height(h as i32);
        x.bits_per_component(8);
        x.color_space().device_gray();
        x.filter(pdf_writer::Filter::FlateDecode);
        x.finish();
    }
    // A 1×1 solid-colour image masked by the alpha.
    let color = shadow.color;
    let px = [
        (crate::color::linear_to_srgb(color.r) * 255.0) as u8,
        (crate::color::linear_to_srgb(color.g) * 255.0) as u8,
        (crate::color::linear_to_srgb(color.b) * 255.0) as u8,
    ];
    let fill_ref = state.refs.alloc();
    {
        let mut x = state.pdf.image_xobject(fill_ref, &px);
        x.width(1);
        x.height(1);
        x.bits_per_component(8);
        x.color_space().device_rgb();
        x.s_mask(mask_ref);
        x.finish();
    }
    let name = format!("Xs{}", *xobject_counter);
    *xobject_counter += 1;
    resources.x_objects.insert(name.clone(), fill_ref);

    // Place: image XObjects paint into the unit square; scale to
    // the stamp's bounds at the shadow offset. NOTE: y-down page
    // space, the page CTM flips — the stamp's alpha rows are
    // y-down too, so flip the image vertically within its rect.
    content.save_state();
    content.transform([
        size_pt.0,
        0.0,
        0.0,
        -size_pt.1,
        origin.0 + shadow.offset_x,
        origin.1 + shadow.offset_y + size_pt.1,
    ]);
    content.x_object(Name(name.as_bytes()));
    content.restore_state();
}

#[allow(unused)]
fn _color_check(_: Color, _: Ref) {}

/// Gradient feather — the vector encoding. The rasterizer blends the
/// already-drawn target toward paper inside the path by
/// `1 − aa·(1 − gradient_alpha)` (see `apply_alpha_factor` in
/// paged-gpu); the exact PDF equivalent at the same z-position is a
/// paper-coloured fill of the path under a LUMINOSITY soft mask
/// whose gray value is `1 − gradient_alpha` along the feather axis:
/// overlay opacity = mask luminosity = 1 − gradient_alpha, i.e. the
/// content underneath fades to paper precisely where the canvas
/// fades it. Fully vector — no raster stamp.
pub fn emit_gradient_feather(
    content: &mut Content,
    state: &mut DocState,
    resources: &mut PageResources,
    pending_forms: &mut Vec<crate::writer::PendingForm>,
    path: &PathData,
    transform: &Transform,
    params: &paged_compose::GradientFeather,
) {
    if params.stops.is_empty() {
        return;
    }
    // Everything in page space (the transform applied point-wise, as
    // the rasterizer does for both path and axis).
    let page_path = crate::page::transform_path(path, transform);
    let bbox = crate::page::path_bbox(&page_path);
    let t = transform.0;
    let map =
        |x: f32, y: f32| -> (f32, f32) { (t[0] * x + t[2] * y + t[4], t[1] * x + t[3] * y + t[5]) };
    let (sx, sy) = map(params.start_x, params.start_y);
    let (ex, ey) = map(params.end_x, params.end_y);

    // Sorted (location, mask gray = 1 − alpha) stops.
    let mut stops: Vec<(f32, f32)> = params
        .stops
        .iter()
        .map(|s| (s.location.clamp(0.0, 1.0), 1.0 - s.alpha.clamp(0.0, 1.0)))
        .collect();
    stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // DeviceGray shading along the feather axis. Degenerate axis ⇒
    // uniform first-stop value (rasterizer parity).
    let degenerate = {
        let dx = ex - sx;
        let dy = ey - sy;
        dx * dx + dy * dy < 1e-6
    };
    let fn_ref = state.refs.alloc();
    if degenerate || stops.len() == 1 {
        let g = stops[0].1;
        let mut f = state.pdf.exponential_function(fn_ref);
        f.domain([0.0, 1.0]);
        f.range([0.0, 1.0]);
        f.c0([g]);
        f.c1([g]);
        f.n(1.0);
        f.finish();
    } else if stops.len() == 2 {
        let mut f = state.pdf.exponential_function(fn_ref);
        f.domain([0.0, 1.0]);
        f.range([0.0, 1.0]);
        f.c0([stops[0].1]);
        f.c1([stops[1].1]);
        f.n(1.0);
        f.finish();
    } else {
        let mut seg_refs = Vec::new();
        for pair in stops.windows(2) {
            let seg_ref = state.refs.alloc();
            let mut f = state.pdf.exponential_function(seg_ref);
            f.domain([0.0, 1.0]);
            f.range([0.0, 1.0]);
            f.c0([pair[0].1]);
            f.c1([pair[1].1]);
            f.n(1.0);
            f.finish();
            seg_refs.push(seg_ref);
        }
        let mut f = state.pdf.stitching_function(fn_ref);
        f.domain([0.0, 1.0]);
        f.range([0.0, 1.0]);
        f.functions(seg_refs.iter().copied());
        let bounds: Vec<f32> = stops[1..stops.len() - 1].iter().map(|s| s.0).collect();
        f.bounds(bounds.iter().copied());
        let encode: Vec<f32> = seg_refs.iter().flat_map(|_| [0.0, 1.0]).collect();
        f.encode(encode.iter().copied());
        f.finish();
    }

    let shading_ref = state.refs.alloc();
    {
        let mut sh = state.pdf.function_shading(shading_ref);
        match params.kind {
            paged_compose::GradientFeatherKind::Linear => {
                sh.shading_type(pdf_writer::types::FunctionShadingType::Axial);
                sh.coords([sx, sy, ex, ey]);
            }
            paged_compose::GradientFeatherKind::Radial => {
                let r = ((ex - sx).powi(2) + (ey - sy).powi(2)).sqrt().max(1e-3);
                sh.shading_type(pdf_writer::types::FunctionShadingType::Radial);
                sh.coords([sx, sy, 0.0, sx, sy, r]);
            }
        }
        sh.color_space().device_gray();
        sh.function(fn_ref);
        sh.extend([true, true]);
        sh.finish();
    }
    let sh_name = format!("Sh{}", resources.shadings.len());
    resources.shadings.insert(sh_name.clone(), shading_ref);

    // The mask group: clip to the path, paint the gray shading. It
    // shares the page's /Resources (written by ref at page finish).
    let mut mask = Content::new();
    crate::path::emit_path(&mut mask, &page_path);
    mask.clip_nonzero();
    mask.end_path();
    mask.shading(Name(sh_name.as_bytes()));
    let mask_ref = state.refs.alloc();
    let pad = 1.0;
    pending_forms.push(crate::writer::PendingForm {
        form_ref: mask_ref,
        data: mask.finish().to_vec(),
        bbox: pdf_writer::Rect::new(
            bbox.x - pad,
            bbox.y - pad,
            bbox.x + bbox.w + pad,
            bbox.y + bbox.h + pad,
        ),
        group: crate::writer::PendingFormGroup::LuminosityGray,
    });

    // ExtGState carrying the soft mask (unique per feather — masks
    // aren't poolable by the simple blend/alpha key).
    let gs_ref = state.refs.alloc();
    {
        let mut gs = state.pdf.ext_graphics(gs_ref);
        let mut sm = gs.soft_mask();
        sm.subtype(pdf_writer::types::MaskType::Luminosity);
        sm.group(mask_ref);
        // Outside the BBox the mask evaluates to the backdrop:
        // black ⇒ overlay alpha 0 ⇒ untouched (matches aa = 0).
        sm.backdrop([0.0]);
        sm.finish();
        gs.finish();
    }
    let gs_name = format!("GsSm{}", resources.ext_g_states.len());
    resources.ext_g_states.insert(gs_name.clone(), gs_ref);

    // Paper overlay: 0/0/0/0 CMYK (no ink) under the mask. The gs —
    // and with it the soft mask — dies with the Q.
    content.save_state();
    content.set_parameters(Name(gs_name.as_bytes()));
    content.set_fill_cmyk(0.0, 0.0, 0.0, 0.0);
    crate::path::emit_path(content, &page_path);
    content.fill_nonzero();
    content.restore_state();
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_compose::PathSegment;

    #[test]
    fn blend_names_match_pdf_blend_modes_one_to_one() {
        // The interned gs KEY uses `blend_name` while the written dict
        // uses `pdf_blend` — if the two mappings ever disagree, two
        // different blend modes could share one pooled ExtGState.
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
            BlendMode::Darken,
            BlendMode::Lighten,
            BlendMode::ColorDodge,
            BlendMode::ColorBurn,
            BlendMode::HardLight,
            BlendMode::SoftLight,
            BlendMode::Difference,
            BlendMode::Exclusion,
            BlendMode::Hue,
            BlendMode::Saturation,
            BlendMode::Color,
            BlendMode::Luminosity,
        ];
        let mut names: Vec<&str> = modes.iter().map(|m| blend_name(*m)).collect();
        // PDF's /BM names are exactly these strings (32000-1 Table 136).
        assert_eq!(blend_name(BlendMode::Multiply), "Multiply");
        assert_eq!(blend_name(BlendMode::ColorDodge), "ColorDodge");
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), modes.len(), "blend names must be distinct");
        for m in modes {
            // The pdf_writer enum's Debug names mirror the PDF names.
            assert_eq!(format!("{:?}", pdf_blend(m)), blend_name(m));
        }
    }

    #[test]
    fn short_hash_is_deterministic_and_separates_gs_keys() {
        // Resource NAMES derive from this hash; a collision between
        // two live keys would silently alias two graphics states.
        assert_eq!(
            short_hash("BMultiplyA0.500O0"),
            short_hash("BMultiplyA0.500O0")
        );
        assert_ne!(
            short_hash("BMultiplyA0.500O0"),
            short_hash("BMultiplyA0.500O1")
        );
        assert_ne!(short_hash("B-A0.500O0"), short_hash("BMultiplyA-O0"));
    }

    fn rect_path(w: f32, h: f32) -> PathData {
        PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: w, y: 0.0 },
                PathSegment::LineTo { x: w, y: h },
                PathSegment::LineTo { x: 0.0, y: h },
                PathSegment::Close,
            ],
        }
    }

    #[test]
    fn blurred_alpha_stamp_pads_by_three_sigma_and_blurs_the_edge() {
        let path = rect_path(100.0, 100.0);
        let ident = Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        // blur_radius 4 → σ = 2 pt → pad = 6 pt; at 72 dpi 1 pt = 1 px.
        let stamp = blurred_alpha_stamp(&path, &ident, 4.0, 72.0).expect("stamp");
        assert_eq!(stamp.origin_pt, (-6.0, -6.0));
        assert_eq!(stamp.size_pt, (112.0, 112.0));
        assert_eq!((stamp.width_px, stamp.height_px), (112, 112));
        let at = |x: u32, y: u32| stamp.alpha[(y * stamp.width_px + x) as usize];
        // Deep interior: fully opaque.
        assert!(at(56, 56) >= 250, "interior alpha {}", at(56, 56));
        // Stamp corner (3σ out from the shape): fully faded.
        assert!(at(0, 0) <= 5, "corner alpha {}", at(0, 0));
        // On the shape edge: a genuine gradient, neither 0 nor 255 —
        // this is the blur actually happening.
        let edge = at(6, 56);
        assert!(
            (20..=235).contains(&edge),
            "edge alpha {edge} should be mid-ramp"
        );
    }

    #[test]
    fn blurred_alpha_stamp_applies_the_transform() {
        // The same rect under a (30, 40) translation must move the
        // stamp origin with it — stamps are in PAGE space.
        let path = rect_path(10.0, 10.0);
        let t = Transform([1.0, 0.0, 0.0, 1.0, 30.0, 40.0]);
        let stamp = blurred_alpha_stamp(&path, &t, 4.0, 72.0).expect("stamp");
        assert_eq!(stamp.origin_pt, (24.0, 34.0));
        assert_eq!(stamp.size_pt, (22.0, 22.0));
    }

    #[test]
    fn blurred_alpha_stamp_rejects_empty_paths() {
        let empty = PathData { segments: vec![] };
        let ident = Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert!(blurred_alpha_stamp(&empty, &ident, 4.0, 72.0).is_none());
    }
}
