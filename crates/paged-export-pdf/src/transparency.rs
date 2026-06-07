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
    // Transform points into page space and find bounds.
    let t = transform.0;
    let map =
        |x: f32, y: f32| -> (f32, f32) { (t[0] * x + t[2] * y + t[4], t[1] * x + t[3] * y + t[5]) };
    let mut min = (f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN);
    let mut consider = |p: (f32, f32)| {
        min.0 = min.0.min(p.0);
        min.1 = min.1.min(p.1);
        max.0 = max.0.max(p.0);
        max.1 = max.1.max(p.1);
    };
    for seg in &path.segments {
        use paged_compose::PathSegment as S;
        match *seg {
            S::MoveTo { x, y } | S::LineTo { x, y } => consider(map(x, y)),
            S::QuadTo { cx, cy, x, y } => {
                consider(map(cx, cy));
                consider(map(x, y));
            }
            S::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => {
                consider(map(cx1, cy1));
                consider(map(cx2, cy2));
                consider(map(x, y));
            }
            S::Close => {}
        }
    }
    if min.0 > max.0 || min.1 > max.1 {
        return None;
    }
    let sigma_pt = blur_radius_pt.max(0.01) * 0.5;
    let pad_pt = sigma_pt * 3.0;
    let origin = (min.0 - pad_pt, min.1 - pad_pt);
    let size_pt = (max.0 - min.0 + pad_pt * 2.0, max.1 - min.1 + pad_pt * 2.0);
    let scale = dpi / 72.0;
    let w = ((size_pt.0 * scale).ceil() as u32).clamp(1, 4096);
    let h = ((size_pt.1 * scale).ceil() as u32).clamp(1, 4096);

    // Scanline fill (NonZero) of the transformed, flattened path.
    let mut alpha = vec![0u8; (w * h) as usize];
    let mut polys: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut current: Vec<(f32, f32)> = Vec::new();
    let to_px =
        |p: (f32, f32)| -> (f32, f32) { ((p.0 - origin.0) * scale, (p.1 - origin.1) * scale) };
    let mut last = (0.0f32, 0.0f32);
    for seg in &path.segments {
        use paged_compose::PathSegment as S;
        match *seg {
            S::MoveTo { x, y } => {
                if current.len() > 2 {
                    polys.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                last = (x, y);
                current.push(to_px(map(x, y)));
            }
            S::LineTo { x, y } => {
                last = (x, y);
                current.push(to_px(map(x, y)));
            }
            S::QuadTo { cx, cy, x, y } => {
                // Flatten with fixed steps — mask quality only.
                for i in 1..=8 {
                    let s = i as f32 / 8.0;
                    let inv = 1.0 - s;
                    let px = inv * inv * last.0 + 2.0 * inv * s * cx + s * s * x;
                    let py = inv * inv * last.1 + 2.0 * inv * s * cy + s * s * y;
                    current.push(to_px(map(px, py)));
                }
                last = (x, y);
            }
            S::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => {
                for i in 1..=12 {
                    let s = i as f32 / 12.0;
                    let inv = 1.0 - s;
                    let px = inv * inv * inv * last.0
                        + 3.0 * inv * inv * s * cx1
                        + 3.0 * inv * s * s * cx2
                        + s * s * s * x;
                    let py = inv * inv * inv * last.1
                        + 3.0 * inv * inv * s * cy1
                        + 3.0 * inv * s * s * cy2
                        + s * s * s * y;
                    current.push(to_px(map(px, py)));
                }
                last = (x, y);
            }
            S::Close => {
                if current.len() > 2 {
                    polys.push(std::mem::take(&mut current));
                }
            }
        }
    }
    if current.len() > 2 {
        polys.push(current);
    }
    for yy in 0..h {
        let sample_y = yy as f32 + 0.5;
        // Even-odd is fine for mask quality on flattened outlines.
        let mut xs: Vec<f32> = Vec::new();
        for poly in &polys {
            for i in 0..poly.len() {
                let a = poly[i];
                let b = poly[(i + 1) % poly.len()];
                if (a.1 <= sample_y && b.1 > sample_y) || (b.1 <= sample_y && a.1 > sample_y) {
                    let t = (sample_y - a.1) / (b.1 - a.1);
                    xs.push(a.0 + t * (b.0 - a.0));
                }
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks_exact(2) {
            let x0 = pair[0].max(0.0) as u32;
            let x1 = (pair[1].min(w as f32 - 1.0)) as u32;
            for xx in x0..=x1.min(w - 1) {
                alpha[(yy * w + xx) as usize] = 255;
            }
        }
    }

    // Separable triple-box blur ≈ Gaussian.
    let sigma_px = sigma_pt * scale;
    let radius = (sigma_px * 1.88 / 3.0).round().max(1.0) as i32;
    for _ in 0..3 {
        box_blur_h(&mut alpha, w, h, radius);
        box_blur_v(&mut alpha, w, h, radius);
    }

    Some(AlphaStamp {
        alpha,
        width_px: w,
        height_px: h,
        origin_pt: origin,
        size_pt,
    })
}

fn box_blur_h(buf: &mut [u8], w: u32, h: u32, r: i32) {
    let w = w as i32;
    let h = h as i32;
    let norm = (2 * r + 1) as u32;
    let mut row = vec![0u8; w as usize];
    for y in 0..h {
        let base = (y * w) as usize;
        let mut acc: u32 = 0;
        for i in -r..=r {
            acc += buf[base + i.clamp(0, w - 1) as usize] as u32;
        }
        for x in 0..w {
            row[x as usize] = (acc / norm) as u8;
            let add = (x + r + 1).clamp(0, w - 1);
            let sub = (x - r).clamp(0, w - 1);
            acc += buf[base + add as usize] as u32;
            acc -= buf[base + sub as usize] as u32;
        }
        buf[base..base + w as usize].copy_from_slice(&row);
    }
}

fn box_blur_v(buf: &mut [u8], w: u32, h: u32, r: i32) {
    let w = w as i32;
    let h = h as i32;
    let norm = (2 * r + 1) as u32;
    let mut col = vec![0u8; h as usize];
    for x in 0..w {
        let mut acc: u32 = 0;
        for i in -r..=r {
            acc += buf[(i.clamp(0, h - 1) * w + x) as usize] as u32;
        }
        for y in 0..h {
            col[y as usize] = (acc / norm) as u8;
            let add = (y + r + 1).clamp(0, h - 1);
            let sub = (y - r).clamp(0, h - 1);
            acc += buf[(add * w + x) as usize] as u32;
            acc -= buf[(sub * w + x) as usize] as u32;
        }
        for y in 0..h {
            buf[(y * w + x) as usize] = col[y as usize];
        }
    }
}

/// Emit a drop shadow as the standard PDF encoding: a coloured rect
/// painted through a blurred-alpha /SMask'd image XObject, offset
/// from the path. Returns the resource (name, ref) used.
#[allow(clippy::too_many_arguments)]
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
