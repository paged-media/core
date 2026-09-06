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

//! Effect coverage rasters — the shared geometry behind object
//! effects.
//!
//! InDesign's object effects are all one construction: take the
//! object's coverage, move / grow / blur / invert it, combine two of
//! them, and paint a colour through the result. This module owns that
//! pipeline so every lane runs the SAME arithmetic: the PDF exporter
//! paints its stamps through these masks, and the GPU lane uploads
//! them as alpha images instead of approximating a soft edge with
//! concentric stamps. It has no rasterizer and no writer dependency —
//! only [`PathData`] and [`Transform`] — so it compiles into the wasm
//! build that has no tiny-skia.

use crate::{DirectionalFeather, Feather, FeatherCornerType, PathData, Transform};

/// The pixel grid one effect rasterises on: a page-space rectangle
/// plus its resolution. Every mask for one effect shares a grid, which
/// is what lets them be combined pixel for pixel.
#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub origin_pt: (f32, f32),
    pub size_pt: (f32, f32),
    pub width_px: u32,
    pub height_px: u32,
    /// Pixels per point.
    pub scale: f32,
}

/// Largest mask we will rasterise, whatever the dpi asks for. A
/// full-page feather at 150 dpi is ~2 Mpx; beyond this the dpi is
/// scaled back rather than the effect dropped, and the blur makes the
/// resampling invisible.
const MAX_MASK_PX: u64 = 4_000_000;

impl Grid {
    /// The grid covering `path` under `transform`, inflated by `pad_pt`
    /// on every side. `None` for a degenerate path.
    pub fn for_path(path: &PathData, transform: &Transform, pad_pt: f32, dpi: f32) -> Option<Grid> {
        let (min, max) = path_bounds_in_page(path, transform)?;
        let pad = pad_pt.max(0.0);
        let origin_pt = (min.0 - pad, min.1 - pad);
        let size_pt = (
            (max.0 - min.0 + pad * 2.0).max(0.01),
            (max.1 - min.1 + pad * 2.0).max(0.01),
        );
        let mut scale = dpi.max(1.0) / 72.0;
        let px = |sc: f32| -> u64 {
            let w = (size_pt.0 * sc).ceil().max(1.0) as u64;
            let h = (size_pt.1 * sc).ceil().max(1.0) as u64;
            w * h
        };
        if px(scale) > MAX_MASK_PX {
            let shrink = (MAX_MASK_PX as f32 / px(scale) as f32).sqrt();
            scale = (scale * shrink).max(0.5);
        }
        let width_px = ((size_pt.0 * scale).ceil() as u32).clamp(1, 4096);
        let height_px = ((size_pt.1 * scale).ceil() as u32).clamp(1, 4096);
        Some(Grid {
            origin_pt,
            size_pt,
            width_px,
            height_px,
            scale,
        })
    }
}

/// One 8-bit coverage raster on a [`Grid`], in page space.
///
/// InDesign's object effects are all the same construction: take the
/// object's coverage, move / grow / blur / invert it, combine two of
/// them, and paint a colour through the result. Rather than write that
/// pipeline seven times, each effect composes these operations — and
/// they are ports of `paged-gpu`'s CPU rasterizer, so the PDF and the
/// canvas agree by construction rather than by eye.
#[derive(Clone)]
pub struct MaskRaster {
    pub data: Vec<u8>,
    pub grid: Grid,
}

impl MaskRaster {
    /// Coverage of `path` under `transform`, offset by `offset_pt` in
    /// page space. Scanline fill of the flattened outline.
    pub fn interior(
        grid: Grid,
        path: &PathData,
        transform: &Transform,
        offset_pt: (f32, f32),
    ) -> Self {
        let polys = flatten_path_px(path, transform, &grid, offset_pt);
        let (w, h) = (grid.width_px, grid.height_px);
        let mut data = vec![0u8; (w as usize) * (h as usize)];
        for yy in 0..h {
            let sample_y = yy as f32 + 0.5;
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
                if pair[1] < 0.0 || pair[0] > w as f32 {
                    continue;
                }
                let x0 = pair[0].max(0.0) as u32;
                let x1 = pair[1].min(w as f32 - 1.0).max(0.0) as u32;
                for xx in x0..=x1.min(w - 1) {
                    data[(yy * w + xx) as usize] = 255;
                }
            }
        }
        Self { data, grid }
    }

    /// Separable triple-box blur ≈ Gaussian of `sigma_pt`.
    ///
    /// Three boxes of width `w` give σ² = (w² − 1)/4, so the radius
    /// that realises a target σ is `(√(4σ² + 1) − 1)/2`. The older
    /// `σ·1.88/3` here produced roughly ⅔ of the σ it was asked for,
    /// which is why the PDF's shadows were visibly crisper than the
    /// canvas's.
    pub fn blur(&mut self, sigma_pt: f32) {
        let sigma_px = sigma_pt.max(0.0) * self.grid.scale;
        if sigma_px < 0.35 {
            return;
        }
        let radius = (((4.0 * sigma_px * sigma_px + 1.0).sqrt() - 1.0) / 2.0)
            .round()
            .max(1.0) as i32;
        for _ in 0..3 {
            box_blur_h(
                &mut self.data,
                self.grid.width_px,
                self.grid.height_px,
                radius,
            );
            box_blur_v(
                &mut self.data,
                self.grid.width_px,
                self.grid.height_px,
                radius,
            );
        }
    }

    /// 255 − v everywhere: the outside of the shape.
    pub fn invert(&mut self) {
        for v in self.data.iter_mut() {
            *v = 255 - *v;
        }
    }

    /// Grow (`amount_pt` > 0) or shrink (< 0) the covered area — the
    /// choke / spread knobs. Blur-then-threshold, the same
    /// approximation the CPU rasterizer uses, so the two agree.
    pub fn morph(&mut self, amount_pt: f32) {
        if amount_pt.abs() < 0.01 {
            return;
        }
        let grow = amount_pt > 0.0;
        if !grow {
            self.invert();
        }
        self.blur(amount_pt.abs());
        for v in self.data.iter_mut() {
            *v = if *v > 64 { 255 } else { 0 };
        }
        if !grow {
            self.invert();
        }
    }

    /// Keep only what both masks cover.
    pub fn multiply(&mut self, other: &Self) {
        for (a, b) in self.data.iter_mut().zip(other.data.iter()) {
            *a = ((*a as u16 * *b as u16) / 255) as u8;
        }
    }

    /// Remove what `other` covers (`max(a − b, 0)`).
    pub fn subtract(&mut self, other: &Self) {
        for (a, b) in self.data.iter_mut().zip(other.data.iter()) {
            *a = a.saturating_sub(*b);
        }
    }

    /// `|a − b|` — the satin wave between two offset copies.
    pub fn abs_diff(&mut self, other: &Self) {
        for (a, b) in self.data.iter_mut().zip(other.data.iter()) {
            *a = a.abs_diff(*b);
        }
    }

    /// Scale every value by `k` (0..=1), i.e. the effect's opacity.
    pub fn scale(&mut self, k: f32) {
        let k = k.clamp(0.0, 1.0);
        for v in self.data.iter_mut() {
            *v = (*v as f32 * k).round() as u8;
        }
    }

    /// True when nothing is covered — the caller can then skip the
    /// image entirely rather than write a blank XObject.
    pub fn is_empty(&self) -> bool {
        self.data.iter().all(|&v| v == 0)
    }
}

/// Page-space bounds of a path under a transform (control points
/// included, so a curve never escapes the box).
fn path_bounds_in_page(path: &PathData, transform: &Transform) -> Option<((f32, f32), (f32, f32))> {
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
        use crate::PathSegment as S;
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
    (min.0 <= max.0 && min.1 <= max.1).then_some((min, max))
}

/// Flatten a path into grid-pixel polygons, translated by `offset_pt`.
fn flatten_path_px(
    path: &PathData,
    transform: &Transform,
    grid: &Grid,
    offset_pt: (f32, f32),
) -> Vec<Vec<(f32, f32)>> {
    let t = transform.0;
    let map =
        |x: f32, y: f32| -> (f32, f32) { (t[0] * x + t[2] * y + t[4], t[1] * x + t[3] * y + t[5]) };
    let to_px = |p: (f32, f32)| -> (f32, f32) {
        (
            (p.0 + offset_pt.0 - grid.origin_pt.0) * grid.scale,
            (p.1 + offset_pt.1 - grid.origin_pt.1) * grid.scale,
        )
    };
    let mut polys: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut current: Vec<(f32, f32)> = Vec::new();
    let mut last = (0.0f32, 0.0f32);
    for seg in &path.segments {
        use crate::PathSegment as S;
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
    polys
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

pub fn feather_mask(
    path: &PathData,
    transform: &Transform,
    feather: &Feather,
    effect_dpi: f32,
) -> Option<MaskRaster> {
    let width = feather.width.max(0.0);
    let sigma = width
        * match feather.corner_type {
            FeatherCornerType::Sharp => 0.5,
            FeatherCornerType::Rounded => 0.75,
            FeatherCornerType::Diffusion => 1.0,
        };
    let grid = Grid::for_path(path, transform, 3.0 * width + 1.0, effect_dpi)?;
    let mut mask = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    mask.blur(sigma);
    remap_choke(&mut mask, feather.choke);
    Some(mask)
}

/// Directional feather — per-side ramps, so only the sides given a
/// width fade. Also a mask over the object's own paint.
pub fn directional_feather_mask(
    path: &PathData,
    transform: &Transform,
    feather: &DirectionalFeather,
    effect_dpi: f32,
) -> Option<MaskRaster> {
    let max_w = feather
        .left_width
        .max(feather.right_width)
        .max(feather.top_width)
        .max(feather.bottom_width)
        .max(0.0);
    let grid = Grid::for_path(path, transform, 3.0 * max_w + 1.0, effect_dpi)?;
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    // The object's own box in page space, from the coverage itself, so
    // the ramps measure from the real edges rather than the padded grid.
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let w = grid.width_px;
    for (i, &v) in interior.data.iter().enumerate() {
        if v == 0 {
            continue;
        }
        let (x, y) = ((i as u32) % w, (i as u32) / w);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if x0 == u32::MAX {
        return None;
    }
    let px = grid.scale.max(0.0001);
    let ramp = |d: f32, width_pt: f32| -> f32 {
        if width_pt <= 0.0 {
            1.0
        } else {
            (d / (width_pt * px)).clamp(0.0, 1.0)
        }
    };
    let mut mask = interior.clone();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let i = (y * w + x) as usize;
            if mask.data[i] == 0 {
                continue;
            }
            let f = ramp((x - x0) as f32, feather.left_width)
                .min(ramp((x1 - x) as f32, feather.right_width))
                .min(ramp((y - y0) as f32, feather.top_width))
                .min(ramp((y1 - y) as f32, feather.bottom_width));
            mask.data[i] = (mask.data[i] as f32 * f) as u8;
        }
    }
    let corner_sigma = max_w
        * match feather.corner_type {
            FeatherCornerType::Sharp => 0.0,
            FeatherCornerType::Rounded => 0.25,
            FeatherCornerType::Diffusion => 0.5,
        };
    if corner_sigma > 0.0 {
        mask.blur(corner_sigma);
        mask.multiply(&interior);
    }
    remap_choke(&mut mask, feather.choke);
    Some(mask)
}

/// Choke pushes the half-way point of the ramp toward the edge, so a
/// feather can be soft or nearly hard at the same width.
pub fn remap_choke(mask: &mut MaskRaster, choke_pct: f32) {
    let c = (choke_pct / 100.0).clamp(0.0, 0.95);
    if c <= 0.0 {
        return;
    }
    let inv = 1.0 / (1.0 - c);
    for v in mask.data.iter_mut() {
        *v = (((*v as f32 / 255.0) * inv).clamp(0.0, 1.0) * 255.0) as u8;
    }
}

/// Pad, in pt, that a feather command needs around its path. Every
/// lane computes its grid from this one figure, so a backdrop captured
/// before the object paints lines up with the mask applied after it.
pub fn feather_pad_pt(cmd: &crate::DisplayCommand) -> Option<f32> {
    match cmd {
        crate::DisplayCommand::Feather { params, .. } => Some(params.width.abs() * 3.0 + 1.0),
        crate::DisplayCommand::DirectionalFeather { params, .. } => {
            let max_w = params
                .left_width
                .max(params.right_width)
                .max(params.top_width)
                .max(params.bottom_width)
                .max(0.0);
            Some(max_w * 3.0 + 1.0)
        }
        // The gradient feather only ever touches the path's interior,
        // so a 1 pt pad for the antialiased edge is enough.
        crate::DisplayCommand::GradientFeather { .. } => Some(1.0),
        _ => None,
    }
}

/// The `(path_id, transform)` an effect or paint command addresses.
/// Effects are emitted as a contiguous run sharing both with the fill
/// they decorate, which is what lets a walk find where an object's
/// paint began.
pub fn cmd_path_and_transform(cmd: &crate::DisplayCommand) -> Option<(crate::PathId, &Transform)> {
    use crate::DisplayCommand as C;
    match cmd {
        C::FillPath {
            path_id, transform, ..
        }
        | C::FillPathBlend {
            path_id, transform, ..
        }
        | C::StrokePath {
            path_id, transform, ..
        }
        | C::DropShadow {
            path_id, transform, ..
        }
        | C::PathShadow {
            path_id, transform, ..
        }
        | C::InnerShadow {
            path_id, transform, ..
        }
        | C::OuterGlow {
            path_id, transform, ..
        }
        | C::InnerGlow {
            path_id, transform, ..
        }
        | C::BevelEmboss {
            path_id, transform, ..
        }
        | C::Satin {
            path_id, transform, ..
        }
        | C::Feather {
            path_id, transform, ..
        }
        | C::DirectionalFeather {
            path_id, transform, ..
        }
        | C::GradientFeather {
            path_id, transform, ..
        } => Some((*path_id, transform)),
        _ => None,
    }
}

/// Map "index of the first command of an effect run" → "index of the
/// feather in it", for every run that carries one.
///
/// A feather takes opacity away from the object's OWN paint. A lane
/// that applies it to the page instead erases whatever else happens to
/// sit inside the feather's padded rectangle, so both raster lanes
/// isolate the run: the CPU photographs the target at the run start,
/// the GPU opens a layer there.
pub fn feather_run_starts(list: &crate::DisplayList) -> std::collections::HashMap<usize, usize> {
    let mut out = std::collections::HashMap::new();
    for (i, cmd) in list.commands.iter().enumerate() {
        if feather_pad_pt(cmd).is_none() {
            continue;
        }
        let Some((pid, xf)) = cmd_path_and_transform(cmd) else {
            continue;
        };
        let mut start = i;
        while start > 0 {
            match cmd_path_and_transform(&list.commands[start - 1]) {
                Some((p, t)) if p == pid && t == xf => start -= 1,
                _ => break,
            }
        }
        out.entry(start).or_insert(i);
    }
    out
}

/// The coverage mask a feather command applies, whichever kind it is.
pub fn feather_command_mask(
    cmd: &crate::DisplayCommand,
    path: &PathData,
    transform: &Transform,
    dpi: f32,
) -> Option<MaskRaster> {
    match cmd {
        crate::DisplayCommand::Feather { params, .. } => feather_mask(path, transform, params, dpi),
        crate::DisplayCommand::DirectionalFeather { params, .. } => {
            directional_feather_mask(path, transform, params, dpi)
        }
        crate::DisplayCommand::GradientFeather { params, .. } => {
            gradient_feather_mask(path, transform, params, dpi)
        }
        _ => None,
    }
}

/// One tinted layer an effect paints: a coverage mask, the colour that
/// goes through it, the blend mode, and whether the paint is confined
/// to the object's own outline.
///
/// Every raster lane consumes these: the PDF exporter writes each as a
/// gray-masked image XObject, the GPU lane uploads each as an alpha
/// image. Building them here is what keeps the two from drifting — the
/// GPU lane used to approximate glows and satin with concentric
/// stamps and skip the bevel entirely.
pub struct EffectStamp {
    pub mask: MaskRaster,
    pub color: crate::Color,
    pub blend_mode: crate::BlendMode,
    /// True when the paint must be clipped to `path` — the interior
    /// effects, whose masks can bleed past the outline after a blur.
    pub clip_to_path: bool,
}

/// Outer glow — the coverage grown and blurred, minus the shape
/// itself, so only what lies outside is painted.
pub fn outer_glow_stamps(
    path: &PathData,
    transform: &Transform,
    glow: &crate::OuterGlow,
    dpi: f32,
) -> Vec<EffectStamp> {
    let sigma = glow.blur_radius.max(0.0);
    let pad = 3.0 * sigma + glow.spread.abs() + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, dpi) else {
        return Vec::new();
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut mask = interior.clone();
    mask.morph(glow.spread);
    mask.blur(sigma);
    mask.subtract(&interior);
    mask.scale(glow.opacity);
    vec![EffectStamp {
        mask,
        color: glow.color,
        blend_mode: glow.blend_mode,
        clip_to_path: false,
    }]
}

/// Inner shadow — the offset OUTSIDE of the shape, blurred and kept
/// only where the shape is, so the edge it darkens is the inside one.
pub fn inner_shadow_stamps(
    path: &PathData,
    transform: &Transform,
    shadow: &crate::InnerShadow,
    dpi: f32,
) -> Vec<EffectStamp> {
    let sigma = shadow.blur_radius.max(0.0);
    let pad =
        3.0 * sigma + shadow.choke.abs() + shadow.offset_x.abs().max(shadow.offset_y.abs()) + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, dpi) else {
        return Vec::new();
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut mask = MaskRaster::interior(grid, path, transform, (shadow.offset_x, shadow.offset_y));
    mask.morph(shadow.choke);
    mask.invert();
    mask.blur(sigma);
    mask.multiply(&interior);
    mask.scale(shadow.opacity);
    vec![EffectStamp {
        mask,
        color: shadow.color,
        blend_mode: shadow.blend_mode,
        clip_to_path: true,
    }]
}

/// Inner glow — the outside blurred inward, kept inside the shape.
pub fn inner_glow_stamps(
    path: &PathData,
    transform: &Transform,
    glow: &crate::InnerGlow,
    dpi: f32,
) -> Vec<EffectStamp> {
    let sigma = glow.blur_radius.max(0.0);
    let pad = 3.0 * sigma + glow.choke.abs() + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, dpi) else {
        return Vec::new();
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut mask = interior.clone();
    mask.invert();
    mask.morph(glow.choke);
    mask.blur(sigma);
    mask.multiply(&interior);
    mask.scale(glow.opacity);
    vec![EffectStamp {
        mask,
        color: glow.color,
        blend_mode: glow.blend_mode,
        clip_to_path: true,
    }]
}

/// Satin — the difference between two copies of the shape offset in
/// opposite directions along the angle, blurred; a soft interference
/// band that follows the outline.
pub fn satin_stamps(
    path: &PathData,
    transform: &Transform,
    satin: &crate::Satin,
    dpi: f32,
) -> Vec<EffectStamp> {
    let sigma = satin.blur_radius.max(0.0);
    let pad = 3.0 * sigma + satin.distance.abs() + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, dpi) else {
        return Vec::new();
    };
    let rad = satin.angle_deg.to_radians();
    let (dx, dy) = (
        satin.distance * 0.5 * rad.cos(),
        -satin.distance * 0.5 * rad.sin(),
    );
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut a = MaskRaster::interior(grid, path, transform, (dx, dy));
    let mut b = MaskRaster::interior(grid, path, transform, (-dx, -dy));
    a.blur(sigma);
    b.blur(sigma);
    a.abs_diff(&b);
    if satin.invert {
        a.invert();
    }
    a.multiply(&interior);
    a.scale(satin.opacity);
    vec![EffectStamp {
        mask: a,
        color: satin.color,
        blend_mode: satin.blend_mode,
        clip_to_path: true,
    }]
}

/// Bevel & emboss — light a height field built from the blurred
/// coverage. Positive slope toward the light becomes the highlight
/// mask, negative the shadow mask; both are clipped to the shape.
pub fn bevel_emboss_stamps(
    path: &PathData,
    transform: &Transform,
    bevel: &crate::BevelEmboss,
    dpi: f32,
) -> Vec<EffectStamp> {
    use crate::{BevelDirection, BevelStyle, BevelTechnique};
    let technique_scale = match bevel.technique {
        BevelTechnique::Smooth => 0.5,
        BevelTechnique::ChiselSoft => 0.25,
        BevelTechnique::ChiselHard => 0.1,
    };
    let pad = 3.0 * bevel.size.max(0.0) + 2.0;
    let Some(grid) = Grid::for_path(path, transform, pad, dpi) else {
        return Vec::new();
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut height = interior.clone();
    height.blur(bevel.size.max(0.0) * technique_scale);

    let (w, h) = (grid.width_px as usize, grid.height_px as usize);
    // Light direction: angle around the page, altitude out of it.
    let a = bevel.angle_deg.to_radians();
    let alt = bevel.altitude_deg.to_radians();
    let (lx, ly, lz) = (
        a.cos() * alt.cos(),
        -a.sin() * alt.cos(),
        alt.sin().max(0.05),
    );
    // Emboss inverts the surface; Down flips the light.
    let style_sign = match bevel.style {
        BevelStyle::Emboss | BevelStyle::PillowEmboss => -1.0f32,
        _ => 1.0,
    };
    let dir_sign = match bevel.direction {
        BevelDirection::Down => -1.0f32,
        BevelDirection::Up => 1.0,
    };
    let gain = bevel.depth.max(0.0) * 4.0 * style_sign * dir_sign;

    let mut hi = MaskRaster {
        data: vec![0u8; w * h],
        grid,
    };
    let mut sh = MaskRaster {
        data: vec![0u8; w * h],
        grid,
    };
    let at = |x: usize, y: usize| -> f32 { height.data[y * w + x] as f32 / 255.0 };
    for y in 0..h {
        for x in 0..w {
            if interior.data[y * w + x] == 0 {
                continue;
            }
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(w - 1);
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(h - 1);
            let gx = (at(xp, y) - at(xm, y)) * 0.5 * gain;
            let gy = (at(x, yp) - at(x, ym)) * 0.5 * gain;
            // Surface normal (-gx, -gy, 1) against the light vector.
            let len = (gx * gx + gy * gy + 1.0).sqrt();
            let dot = (-gx * lx - gy * ly + lz) / len;
            // A flat interior has dot ≈ lz; the lit RELIEF is the
            // departure from flat, which is what the edges carry.
            let relief = dot - lz;
            let slope = (gx * gx + gy * gy).sqrt().min(1.0);
            let v = |k: f32| -> u8 { (k.clamp(0.0, 1.0) * 255.0) as u8 };
            if relief > 0.0 {
                hi.data[y * w + x] = v(relief * slope * bevel.highlight_opacity);
            } else {
                sh.data[y * w + x] = v(-relief * slope * bevel.shadow_opacity);
            }
        }
    }
    let soften = bevel.soften.max(0.0);
    if soften > 0.0 {
        hi.blur(soften);
        sh.blur(soften);
        hi.multiply(&interior);
        sh.multiply(&interior);
    }
    vec![
        EffectStamp {
            mask: hi,
            color: bevel.highlight_color,
            blend_mode: crate::BlendMode::Normal,
            clip_to_path: true,
        },
        EffectStamp {
            mask: sh,
            color: bevel.shadow_color,
            blend_mode: crate::BlendMode::Normal,
            clip_to_path: true,
        },
    ]
}

/// The stamps one effect command paints, whichever effect it is.
/// Feathers are absent on purpose: they take alpha away from the
/// object rather than adding a layer, and go through
/// [`feather_command_mask`].
pub fn effect_command_stamps(
    cmd: &crate::DisplayCommand,
    path: &PathData,
    transform: &Transform,
    dpi: f32,
) -> Vec<EffectStamp> {
    use crate::DisplayCommand as C;
    match cmd {
        C::OuterGlow { params, .. } => outer_glow_stamps(path, transform, params, dpi),
        C::InnerShadow { params, .. } => inner_shadow_stamps(path, transform, params, dpi),
        C::InnerGlow { params, .. } => inner_glow_stamps(path, transform, params, dpi),
        C::Satin { params, .. } => satin_stamps(path, transform, params, dpi),
        C::BevelEmboss { params, .. } => bevel_emboss_stamps(path, transform, params, dpi),
        _ => Vec::new(),
    }
}

/// Sample an alpha stop list at `t` (0..=1), linearly between stops.
fn sample_gradient_alpha(stops: &[(f32, f32)], t: f32) -> f32 {
    if stops.is_empty() {
        return 1.0;
    }
    if t <= stops[0].0 {
        return stops[0].1;
    }
    if t >= stops[stops.len() - 1].0 {
        return stops[stops.len() - 1].1;
    }
    for w in stops.windows(2) {
        let (l0, a0) = w[0];
        let (l1, a1) = w[1];
        if t >= l0 && t <= l1 {
            let span = (l1 - l0).max(1e-6);
            return a0 + (a1 - a0) * ((t - l0) / span);
        }
    }
    stops[stops.len() - 1].1
}

/// Gradient feather — the alpha the object KEEPS, sampled from a 1-D
/// gradient along the axis and confined to the path.
///
/// Outside the path the mask is 255 (untouched); inside it is the
/// gradient's alpha. That is the shape a lane can hand straight to a
/// `DestIn` composite or a `/SMask`, and it is why this belongs beside
/// the other feathers rather than in a lane.
pub fn gradient_feather_mask(
    path: &PathData,
    transform: &Transform,
    params: &crate::GradientFeather,
    dpi: f32,
) -> Option<MaskRaster> {
    if params.stops.is_empty() {
        return None;
    }
    let grid = Grid::for_path(path, transform, 1.0, dpi)?;
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let stops: Vec<(f32, f32)> = params
        .stops
        .iter()
        .map(|s| (s.location.clamp(0.0, 1.0), s.alpha.clamp(0.0, 1.0)))
        .collect();
    let (sx, sy) = transform.apply(params.start_x, params.start_y);
    let (ex, ey) = transform.apply(params.end_x, params.end_y);
    let (dx, dy) = (ex - sx, ey - sy);
    let len_sq = dx * dx + dy * dy;
    let degenerate = len_sq < 1e-9;
    let inv_len_sq = if degenerate { 0.0 } else { 1.0 / len_sq };
    let radius = len_sq.sqrt().max(1e-6);
    let inv_scale = 1.0 / grid.scale.max(1e-6);
    let (w, h) = (grid.width_px, grid.height_px);
    let mut data = vec![255u8; (w as usize) * (h as usize)];
    for j in 0..h {
        for i in 0..w {
            let idx = (j * w + i) as usize;
            let aa = interior.data[idx];
            if aa == 0 {
                continue;
            }
            let px_pt = grid.origin_pt.0 + (i as f32 + 0.5) * inv_scale;
            let py_pt = grid.origin_pt.1 + (j as f32 + 0.5) * inv_scale;
            let alpha = if degenerate {
                stops[0].1
            } else {
                let t = match params.kind {
                    crate::GradientFeatherKind::Linear => {
                        (((px_pt - sx) * dx + (py_pt - sy) * dy) * inv_len_sq).clamp(0.0, 1.0)
                    }
                    crate::GradientFeatherKind::Radial => {
                        ((px_pt - sx).hypot(py_pt - sy) / radius).clamp(0.0, 1.0)
                    }
                };
                sample_gradient_alpha(&stops, t)
            };
            let aa_unit = aa as f32 / 255.0;
            let keep = 1.0 - aa_unit * (1.0 - alpha);
            data[idx] = (keep.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    Some(MaskRaster { data, grid })
}
