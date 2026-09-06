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
    /// choke / spread knobs.
    ///
    /// This moves the boundary by exactly `amount_pt`, by thresholding
    /// the coverage's own distance field. It used to blur by the
    /// amount and threshold the result, which rounds corners and, for
    /// a large choke, loses the shape entirely: a 90 % choke on a
    /// 16 pt inner shadow simply stopped painting.
    pub fn morph(&mut self, amount_pt: f32) {
        if amount_pt.abs() < 0.01 {
            return;
        }
        let sd = signed_distance_pt(self);
        for (v, &d) in self.data.iter_mut().zip(sd.iter()) {
            *v = if d + amount_pt >= 0.0 { 255 } else { 0 };
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

/// InDesign's `Size` is the WIDTH OF THE BAND an effect occupies, not
/// a Gaussian σ — the same discovery the bevel's facet made, and it
/// applies to every soft effect in the dialog.
///
/// Measured (InDesign 20.0.1, 600 dpi JPEGs of size sweeps over a
/// vermilion square on white): a drop shadow or outer glow is the
/// object's coverage blurred with **σ = Size / 2** — its alpha reads
/// Φ(−1) at exactly `Size/2` outside the outline and Φ(−2) at `Size`
/// — while the inner effects, which blur the inverted shape and then
/// clip it back, sit a little tighter at **σ = 0.4 × Size**.
///
/// We used to pass `Size` straight in as σ, so every shadow and glow
/// in every document was twice as soft as InDesign's and reached
/// twice as far.
pub const OUTER_SIGMA_PER_SIZE: f32 = 0.5;
pub const INNER_SIGMA_PER_SIZE: f32 = 0.4;

/// σ for an effect that spills OUTSIDE the object.
pub fn outer_sigma_pt(size_pt: f32) -> f32 {
    size_pt.max(0.0) * OUTER_SIGMA_PER_SIZE
}

/// σ for an effect that stays INSIDE it.
pub fn inner_sigma_pt(size_pt: f32) -> f32 {
    size_pt.max(0.0) * INNER_SIGMA_PER_SIZE
}

/// `Choke` / `Spread` are FRACTIONS OF `Size`, not distances: they
/// move the effect's half-way line that far along the band and give
/// the blur whatever width is left. Measured on an inner shadow with
/// `Size = 16`: `ChokeAmount = 60` puts the fully-dark front at
/// 9.6 pt (= 0.6 × 16) and still reaches nothing at 16 pt.
fn choke_split(size_pt: f32, choke: f32) -> (f32, f32) {
    let c = choke.clamp(0.0, 1.0);
    (size_pt.max(0.0) * c, 1.0 - c)
}

/// Outer glow — the coverage grown and blurred, minus the shape
/// itself, so only what lies outside is painted.
pub fn outer_glow_stamps(
    path: &PathData,
    transform: &Transform,
    glow: &crate::OuterGlow,
    dpi: f32,
) -> Vec<EffectStamp> {
    let (grow, rest) = choke_split(glow.blur_radius, glow.spread);
    let sigma = outer_sigma_pt(glow.blur_radius) * rest;
    let pad = 3.0 * sigma + grow + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, dpi) else {
        return Vec::new();
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut mask = interior.clone();
    mask.morph(grow);
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
    let (choke, rest) = choke_split(shadow.blur_radius, shadow.choke);
    let sigma = inner_sigma_pt(shadow.blur_radius) * rest;
    let pad = 3.0 * sigma + choke + shadow.offset_x.abs().max(shadow.offset_y.abs()) + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, dpi) else {
        return Vec::new();
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut mask = MaskRaster::interior(grid, path, transform, (shadow.offset_x, shadow.offset_y));
    // Choking an INNER effect eats further into the object, so the
    // shape the shadow is the outside of has to SHRINK first.
    mask.morph(-choke);
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
    let (choke, rest) = choke_split(glow.blur_radius, glow.choke);
    let sigma = inner_sigma_pt(glow.blur_radius) * rest;
    let pad = 3.0 * sigma + choke + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, dpi) else {
        return Vec::new();
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut mask = interior.clone();
    mask.invert();
    // Already inverted, so GROWING the outside is what eats inward.
    mask.morph(choke);
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
    let sigma = outer_sigma_pt(satin.blur_radius);
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

/// One-dimensional squared Euclidean distance transform
/// (Felzenszwalb & Huttenlocher 2012), in place over `f`.
///
/// `f` holds 0 at the feature pixels and a large value elsewhere; on
/// return it holds the squared distance, in pixels², to the nearest
/// feature. Separable, so running it over the columns and then the
/// rows gives the exact 2-D transform in O(n).
fn edt_1d(f: &mut [f32], scratch: &mut Edt1dScratch) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let (v, z, d) = (&mut scratch.v, &mut scratch.z, &mut scratch.d);
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    let mut k = 0usize;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32))
                / (2.0 * q as f32 - 2.0 * p as f32);
            if s <= z[k] {
                // `z[0]` is −∞, so this never steps below zero.
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f32::INFINITY;
                break;
            }
        }
    }
    k = 0;
    for (q, slot) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        let dq = q as f32 - p as f32;
        *slot = dq * dq + f[p];
    }
    f.copy_from_slice(&d[..n]);
}

/// Reusable buffers for [`edt_1d`], sized to the longest scanline.
struct Edt1dScratch {
    v: Vec<usize>,
    z: Vec<f32>,
    d: Vec<f32>,
}

impl Edt1dScratch {
    fn new(n: usize) -> Self {
        Self {
            v: vec![0; n + 1],
            z: vec![0.0; n + 2],
            d: vec![0.0; n + 1],
        }
    }
}

/// Distance, in pixels, from every pixel to the nearest pixel where
/// `feature` is true.
fn distance_px(feature: impl Fn(usize) -> bool, w: usize, h: usize) -> Vec<f32> {
    const FAR: f32 = 1.0e12;
    let mut f: Vec<f32> = (0..w * h)
        .map(|i| if feature(i) { 0.0 } else { FAR })
        .collect();
    let mut scratch = Edt1dScratch::new(w.max(h));
    let mut col = vec![0.0f32; h];
    for x in 0..w {
        for (y, slot) in col.iter_mut().enumerate() {
            *slot = f[y * w + x];
        }
        edt_1d(&mut col, &mut scratch);
        for (y, v) in col.iter().enumerate() {
            f[y * w + x] = *v;
        }
    }
    for y in 0..h {
        edt_1d(&mut f[y * w..(y + 1) * w], &mut scratch);
    }
    for v in f.iter_mut() {
        *v = v.max(0.0).sqrt();
    }
    f
}

/// Signed distance to the coverage boundary, in POINTS, positive
/// inside. The half-pixel shift puts the zero on the boundary itself
/// rather than on the first covered pixel's centre.
fn signed_distance_pt(interior: &MaskRaster) -> Vec<f32> {
    let (w, h) = (
        interior.grid.width_px as usize,
        interior.grid.height_px as usize,
    );
    let inside = |i: usize| interior.data[i] >= 128;
    // `&inside` and `|i| !inside(i)` are the covered and uncovered
    // feature sets; the two transforms together make the field signed.
    let d_out = distance_px(inside, w, h);
    let d_in = distance_px(|i: usize| !inside(i), w, h);
    let scale = interior.grid.scale.max(1e-6);
    (0..w * h)
        .map(|i| {
            let px = if inside(i) {
                d_in[i] - 0.5
            } else {
                -(d_out[i] - 0.5)
            };
            px / scale
        })
        .collect()
}

/// The facet's surface slope at the object's edge, in rise over run.
///
/// Measured against InDesign 20.0.1 (24 pt bevels on a vermilion
/// square at angle 180°, so a horizontal scanline reads the facet
/// directly, swept over depth, angle and altitude): the smooth
/// contour's surface starts at a little over 45° on the edge and
/// flattens LINEARLY to nothing at `Size`, while BOTH chisels are one
/// flat facet at about half that slope for the band's whole width.
const SMOOTH_EDGE_SLOPE: f32 = 1.08;
const CHISEL_SLOPE: f32 = 0.54;

/// The facet's HEIGHT `q` of the way from the object's edge (`q = 0`)
/// to `Size` inside it (`q = 1`), in multiples of the band's width.
///
/// This is the INTEGRAL of the slope described above — the shading
/// differentiates the field this builds, so the two are one statement
/// written twice and `the_height_field_integrates_the_facet_slope`
/// holds them together.
fn facet_height(technique: crate::BevelTechnique, q: f32) -> f32 {
    let q = q.clamp(0.0, 1.0);
    match technique {
        crate::BevelTechnique::Smooth => SMOOTH_EDGE_SLOPE * (q - 0.5 * q * q),
        _ => CHISEL_SLOPE * q,
    }
}

/// One sloped band of a bevel: where it sits relative to the object's
/// edge, how wide it is, and whether it rises or falls going inward.
struct BevelBand {
    /// `true` when the band lies inside the object.
    inner: bool,
    width_pt: f32,
    sign: f32,
}

/// The bands a bevel style is made of.
///
/// Measured: an inner bevel is one band `Size` wide inside the edge;
/// an outer bevel one band `Size` wide OUTSIDE it (the old code masked
/// every style to the interior, so an outer bevel drew nothing at
/// all). Emboss straddles the edge with a half-`Size` band on each
/// side, rising inward throughout, and pillow emboss is the same pair
/// with the outer band's slope reversed.
fn bevel_bands(style: crate::BevelStyle, size: f32) -> Vec<BevelBand> {
    use crate::BevelStyle as S;
    match style {
        S::InnerBevel | S::StrokeEmboss => vec![BevelBand {
            inner: true,
            width_pt: size,
            sign: 1.0,
        }],
        S::OuterBevel => vec![BevelBand {
            inner: false,
            width_pt: size,
            sign: 1.0,
        }],
        S::Emboss => vec![
            BevelBand {
                inner: true,
                width_pt: size * 0.5,
                sign: 1.0,
            },
            BevelBand {
                inner: false,
                width_pt: size * 0.5,
                sign: 1.0,
            },
        ],
        S::PillowEmboss => vec![
            BevelBand {
                inner: true,
                width_pt: size * 0.5,
                sign: 1.0,
            },
            BevelBand {
                inner: false,
                width_pt: size * 0.5,
                sign: -1.0,
            },
        ],
    }
}

/// How far outside the object's outline a bevel reaches, in points.
pub fn bevel_outer_reach_pt(bevel: &crate::BevelEmboss) -> f32 {
    bevel_bands(bevel.style, bevel.size.max(0.0))
        .iter()
        .filter(|b| !b.inner)
        .fold(0.0f32, |acc, b| acc.max(b.width_pt))
}

/// `Soften` blurs the shaded facet. Measured: a 12 pt soften on a
/// 24 pt bevel spreads the tail about 6 pt past the band and takes
/// ~15 % off the peak, which is a σ of a quarter the slider.
const SOFTEN_SIGMA: f32 = 0.25;

/// Bevel & emboss — light a height field built from the distance to
/// the object's outline.
///
/// The model is InDesign's, read off real exports rather than guessed
/// (`bev2`/`bev3` probes, InDesign 20.0.1, 600 dpi JPEG):
///
///   * the facet is exactly `Size` points wide, not some multiple of a
///     blur radius;
///   * its surface slope is 45° at the edge and falls linearly to zero
///     at `Size` for the smooth contour, and is one flat half-slope
///     facet for both chisels;
///   * `Depth` steepens that facet rather than scaling the result,
///     and the shading is Lambert against the surface normal —
///     measured, taking depth from 25 % to 120 % barely moves the
///     highlight (the lit face tips PAST the light) while the shadow
///     runs to black, which no linear gain reproduces;
///   * the relief is normalised by the room the light leaves above
///     (`1 − sin altitude`) and below (`sin altitude`) the flat
///     interior, so a bevel is as strong as its altitude allows;
///   * the highlight composites with Screen and the shadow with
///     Multiply (InDesign's defaults, and the only spelling IDML
///     leaves out of the file).
///
/// Known gap: where the band is wider than half the object, InDesign
/// fades the facet out as the two sides' slopes meet at the medial
/// axis; we let them meet at full strength.
pub fn bevel_emboss_stamps(
    path: &PathData,
    transform: &Transform,
    bevel: &crate::BevelEmboss,
    dpi: f32,
) -> Vec<EffectStamp> {
    use crate::BevelDirection;
    let size = bevel.size.max(0.0);
    if size <= 0.0 {
        return Vec::new();
    }
    let bands = bevel_bands(bevel.style, size);
    let soften = bevel.soften.max(0.0);
    let outer = bevel_outer_reach_pt(bevel);
    let pad = outer + soften + 2.0;
    let Some(grid) = Grid::for_path(path, transform, pad, dpi) else {
        return Vec::new();
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let sd = signed_distance_pt(&interior);
    let (w, h) = (grid.width_px as usize, grid.height_px as usize);

    // Height field, in points. Every band is measured from the
    // object's OUTLINE outward or inward — an outer facet is the inner
    // one mirrored, steepest against the edge and flattening away from
    // it, not the other way round.
    let full = facet_height(bevel.technique, 1.0);
    let height: Vec<f32> = sd
        .iter()
        .map(|&s| {
            bands
                .iter()
                .map(|b| {
                    let w = b.width_pt.max(1e-6);
                    let a = w * b.sign;
                    if b.inner {
                        a * facet_height(bevel.technique, s / w)
                    } else {
                        a * (full - facet_height(bevel.technique, -s / w))
                    }
                })
                .sum::<f32>()
        })
        .collect();

    // Light: azimuth around the page (screen-down y), altitude out of
    // it. `lz` is how much of the flat interior the light already
    // reaches, so it is exactly the room a shadow has to take away —
    // and `1 − lz` the room a highlight has to add.
    let az = bevel.angle_deg.to_radians();
    let alt = bevel.altitude_deg.to_radians();
    let cos_alt = alt.cos();
    let lz = alt.sin();
    let (lx, ly) = (az.cos() * cos_alt, -az.sin() * cos_alt);
    let hi_room = (1.0 - lz).max(0.02);
    let sh_room = lz.max(0.02);
    let dir_sign = match bevel.direction {
        BevelDirection::Down => -1.0f32,
        BevelDirection::Up => 1.0,
    };
    let depth = bevel.depth.max(0.0);

    let mut hi = MaskRaster {
        data: vec![0u8; w * h],
        grid,
    };
    let mut sh = MaskRaster {
        data: vec![0u8; w * h],
        grid,
    };
    let grad_scale = 0.5 * grid.scale;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(w - 1);
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(h - 1);
            // Central differences, in points: the pixel step is
            // 1/scale points wide, so the bevel reads the same at
            // every dpi.
            // `Depth` steepens the FACET, it does not scale the
            // output: measured, a 480 % depth increase barely moves
            // the highlight (the lit side tips past the light) while
            // the shadow saturates. Only a real surface normal does
            // that; a linear gain cannot.
            let gx = (height[y * w + xp] - height[y * w + xm]) * grad_scale * depth * dir_sign;
            let gy = (height[yp * w + x] - height[ym * w + x]) * grad_scale * depth * dir_sign;
            // Normal (−gx, −gy, 1) against the light; a flat interior
            // returns exactly `lz`, so the relief is the departure
            // from flat.
            let len = (gx * gx + gy * gy + 1.0).sqrt();
            let relief = (-gx * lx - gy * ly + lz) / len - lz;
            // The shading saturates BEFORE the opacity scales it —
            // a 70 %-opaque shadow over a facet that is already fully
            // dark reads at 70 %, not at 100 %.
            let v = |k: f32, op: f32| -> u8 { (k.clamp(0.0, 1.0) * op * 255.0) as u8 };
            if relief > 0.0 {
                hi.data[i] = v(relief / hi_room, bevel.highlight_opacity);
            } else if relief < 0.0 {
                sh.data[i] = v(-relief / sh_room, bevel.shadow_opacity);
            }
        }
    }
    // Only the bands that live inside the object are clipped to it;
    // an outer bevel's facet is meant to fall on the page.
    let clip = bands.iter().all(|b| b.inner);
    if soften > 0.0 {
        // Renormalised blur: measured, `Soften` drags the facet's tail
        // further into the object without eating the crisp line where
        // it meets the object's edge. A plain blur does eat it,
        // because it averages in the emptiness outside; dividing by
        // the blurred coverage puts that back.
        let mut norm = if clip {
            interior.clone()
        } else {
            MaskRaster {
                data: vec![255u8; w * h],
                grid,
            }
        };
        norm.blur(soften * SOFTEN_SIGMA);
        hi.blur(soften * SOFTEN_SIGMA);
        sh.blur(soften * SOFTEN_SIGMA);
        for m in [&mut hi, &mut sh] {
            for (v, &n) in m.data.iter_mut().zip(norm.data.iter()) {
                if n > 0 {
                    *v = ((*v as u32 * 255) / n as u32).min(255) as u8;
                }
            }
        }
    }
    if clip {
        hi.multiply(&interior);
        sh.multiply(&interior);
    }
    vec![
        EffectStamp {
            mask: hi,
            color: bevel.highlight_color,
            blend_mode: crate::BlendMode::Screen,
            clip_to_path: clip,
        },
        EffectStamp {
            mask: sh,
            color: bevel.shadow_color,
            blend_mode: crate::BlendMode::Multiply,
            clip_to_path: clip,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BevelDirection, BevelEmboss, BevelStyle, BevelTechnique, Color, PathSegment};

    /// A `w × h` point rectangle whose top-left corner sits at
    /// `(x, y)`, as a unit-square path plus the transform that places
    /// it — the shape every emitter hands the mask builders.
    fn rect(x: f32, y: f32, w: f32, h: f32) -> (PathData, Transform) {
        let path = PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: 1.0, y: 0.0 },
                PathSegment::LineTo { x: 1.0, y: 1.0 },
                PathSegment::LineTo { x: 0.0, y: 1.0 },
                PathSegment::Close,
            ],
        };
        (path, Transform([w, 0.0, 0.0, h, x, y]))
    }

    fn bevel(size: f32, depth: f32, angle_deg: f32) -> BevelEmboss {
        BevelEmboss {
            depth,
            size,
            angle_deg,
            altitude_deg: 30.0,
            highlight_color: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            shadow_color: Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            highlight_opacity: 1.0,
            shadow_opacity: 1.0,
            style: BevelStyle::InnerBevel,
            direction: BevelDirection::Up,
            technique: BevelTechnique::Smooth,
            soften: 0.0,
        }
    }

    /// Read a mask at a page-space point.
    fn at(m: &MaskRaster, x_pt: f32, y_pt: f32) -> u8 {
        let g = m.grid;
        let px = ((x_pt - g.origin_pt.0) * g.scale) as i32;
        let py = ((y_pt - g.origin_pt.1) * g.scale) as i32;
        if px < 0 || py < 0 || px >= g.width_px as i32 || py >= g.height_px as i32 {
            return 0;
        }
        m.data[py as usize * g.width_px as usize + px as usize]
    }

    #[test]
    fn the_height_field_integrates_the_facet_slope() {
        // The shading differentiates the height field, so the height
        // MUST be the integral of the slope the model claims — the
        // two are one statement written twice, and a drift between
        // them silently rescales every bevel.
        for technique in [
            BevelTechnique::Smooth,
            BevelTechnique::ChiselSoft,
            BevelTechnique::ChiselHard,
        ] {
            let step = 1.0 / 512.0;
            for k in 0..512 {
                let q = k as f32 * step;
                let numeric =
                    (facet_height(technique, q + step) - facet_height(technique, q)) / step;
                let qm = (q + step * 0.5).clamp(0.0, 1.0);
                let claimed = match technique {
                    BevelTechnique::Smooth => SMOOTH_EDGE_SLOPE * (1.0 - qm),
                    _ => CHISEL_SLOPE,
                };
                assert!(
                    (numeric - claimed).abs() < 1e-3,
                    "{technique:?} at q={q}: d(height) {numeric} vs slope {claimed}"
                );
            }
        }
    }

    #[test]
    fn the_facet_is_exactly_size_points_wide() {
        // Measured against InDesign: a `Size` of 12 lights 12 points
        // of the object's edge and not a point more. The old model
        // blurred the coverage instead, which spread the facet over
        // 1.25 × Size and made the whole thing a smudge.
        let (path, xf) = rect(20.0, 20.0, 90.0, 90.0);
        let stamps = bevel_emboss_stamps(&path, &xf, &bevel(12.0, 0.25, 180.0), 288.0);
        let hi = &stamps[0].mask;
        assert!(at(hi, 20.5, 65.0) > 100, "the edge itself is lit");
        assert!(at(hi, 28.0, 65.0) > 20, "still inside the 12 pt band");
        assert_eq!(at(hi, 34.0, 65.0), 0, "two points past the band, nothing");
    }

    #[test]
    fn a_bevel_lights_the_edge_its_angle_points_at() {
        // Angle 180° is light from the left: the left edge takes the
        // highlight, the right edge the shadow, and the two edges the
        // light only grazes take neither.
        let (path, xf) = rect(20.0, 20.0, 90.0, 90.0);
        let stamps = bevel_emboss_stamps(&path, &xf, &bevel(10.0, 0.25, 180.0), 288.0);
        let (hi, sh) = (&stamps[0].mask, &stamps[1].mask);
        assert!(at(hi, 21.0, 65.0) > 80, "left edge lit");
        assert_eq!(at(sh, 21.0, 65.0), 0, "and not also shadowed");
        assert!(at(sh, 109.0, 65.0) > 80, "right edge shadowed");
        assert_eq!(at(hi, 109.0, 65.0), 0, "and not also lit");
        assert!(at(hi, 65.0, 21.0) < 20, "the top edge is only grazed");
        assert_eq!(at(hi, 65.0, 65.0), 0, "the flat middle is untouched");
    }

    #[test]
    fn depth_steepens_the_facet_rather_than_scaling_it() {
        // The measurement this whole model rests on: taking Depth from
        // 25 % to 120 % runs InDesign's shadow to black while its
        // highlight barely moves, because the lit face tips PAST the
        // light. A gain multiplier — what we used to have — would move
        // both by the same 4.8×.
        let (path, xf) = rect(20.0, 20.0, 90.0, 90.0);
        let low = bevel_emboss_stamps(&path, &xf, &bevel(24.0, 0.25, 120.0), 288.0);
        let high = bevel_emboss_stamps(&path, &xf, &bevel(24.0, 1.2, 120.0), 288.0);
        let hi_low = at(&low[0].mask, 21.0, 65.0) as f32;
        let hi_high = at(&high[0].mask, 21.0, 65.0) as f32;
        let sh_low = at(&low[1].mask, 109.0, 65.0) as f32;
        let sh_high = at(&high[1].mask, 109.0, 65.0) as f32;
        assert!(
            hi_high / hi_low < 2.0,
            "highlight saturates: {hi_low} -> {hi_high}"
        );
        assert!(
            sh_high > 240.0,
            "shadow runs to black: {sh_low} -> {sh_high}"
        );
    }

    #[test]
    fn an_outer_bevel_paints_outside_the_object() {
        // It used to paint nothing at all: every style was masked to
        // the interior, where an outer bevel has no facet.
        let (path, xf) = rect(40.0, 40.0, 60.0, 60.0);
        let mut b = bevel(12.0, 0.5, 180.0);
        b.style = BevelStyle::OuterBevel;
        let stamps = bevel_emboss_stamps(&path, &xf, &b, 288.0);
        assert!(!stamps[0].clip_to_path, "an outer facet is not clipped in");
        let sh = &stamps[1].mask;
        assert!(
            at(sh, 101.0, 70.0) > 40,
            "shadow just outside the right edge"
        );
        assert!(
            at(sh, 101.0, 70.0) > at(sh, 110.0, 70.0),
            "and it fades going outward"
        );
        assert_eq!(at(sh, 70.0, 70.0), 0, "nothing inside the object");
    }

    #[test]
    fn the_signed_distance_reads_zero_on_the_outline() {
        let (path, xf) = rect(10.0, 10.0, 40.0, 40.0);
        let grid = Grid::for_path(&path, &xf, 6.0, 288.0).expect("grid");
        let interior = MaskRaster::interior(grid, &path, &xf, (0.0, 0.0));
        let sd = signed_distance_pt(&interior);
        let read = |x: f32, y: f32| -> f32 {
            let px = ((x - grid.origin_pt.0) * grid.scale) as usize;
            let py = ((y - grid.origin_pt.1) * grid.scale) as usize;
            sd[py * grid.width_px as usize + px]
        };
        assert!(read(30.0, 30.0) > 19.0, "the middle is 20 pt in");
        assert!(read(10.2, 30.0).abs() < 0.5, "the left edge is the zero");
        assert!(read(6.0, 30.0) < -3.0, "outside is negative");
    }

    #[test]
    fn an_outer_effect_blurs_with_half_its_size() {
        // Measured on InDesign 20.0.1: a drop shadow or outer glow of
        // `Size` reads Φ(−1) ≈ 0.16 of its opacity at Size/2 outside
        // the outline and Φ(−2) ≈ 0.02 at Size. We used to pass `Size`
        // in as σ, so every shadow was twice as soft and reached twice
        // as far as InDesign's.
        let (path, xf) = rect(30.0, 30.0, 60.0, 60.0);
        let glow = crate::OuterGlow {
            blur_radius: 16.0,
            color: Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            opacity: 1.0,
            blend_mode: crate::BlendMode::Normal,
            spread: 0.0,
        };
        let stamps = outer_glow_stamps(&path, &xf, &glow, 288.0);
        let m = &stamps[0].mask;
        let at_half = at(m, 98.0, 60.0) as f32 / 255.0; // 8 pt out = σ
        let at_full = at(m, 106.0, 60.0) as f32 / 255.0; // 16 pt out = 2σ
        assert!(
            (at_half - 0.159).abs() < 0.05,
            "Φ(−1) one σ out, got {at_half}"
        );
        assert!(
            (at_full - 0.023).abs() < 0.03,
            "Φ(−2) two σ out, got {at_full}"
        );
    }

    #[test]
    fn choke_walks_the_shadows_front_along_the_band() {
        // `ChokeAmount` is a percentage OF `Size`, not a distance: a
        // 60 % choke on a 16 pt inner shadow puts the fully-dark front
        // 9.6 pt in and still fades out by 16. We used to feed the
        // fraction in as points, so a 60 % choke moved the edge by
        // 0.6 pt — nothing.
        let (path, xf) = rect(20.0, 20.0, 90.0, 90.0);
        let shadow = |choke: f32| crate::InnerShadow {
            offset_x: 0.0,
            offset_y: 0.0,
            blur_radius: 16.0,
            color: Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            opacity: 1.0,
            choke,
            blend_mode: crate::BlendMode::Multiply,
        };
        let none = inner_shadow_stamps(&path, &xf, &shadow(0.0), 288.0);
        let hard = inner_shadow_stamps(&path, &xf, &shadow(0.6), 288.0);
        // 4 pt in: the unchoked shadow has already faded, the choked
        // one is still solid.
        assert!(at(&none[0].mask, 24.0, 65.0) < 100, "unchoked, 4 pt in");
        assert!(at(&hard[0].mask, 24.0, 65.0) > 240, "choked, 4 pt in");
        // The choked front's half-way line sits at 0.6 × 16 = 9.6 pt.
        let half = at(&hard[0].mask, 30.0, 65.0);
        assert!((90..160).contains(&half), "half-way at 10 pt, got {half}");
        // And both are gone by `Size`.
        assert!(at(&hard[0].mask, 34.0, 65.0) < 30, "spent by 14 pt");
        assert_eq!(at(&hard[0].mask, 36.0, 65.0), 0, "nothing at Size");
    }

    #[test]
    fn a_nearly_total_choke_still_paints() {
        // The old blur-and-threshold morph lost the shape entirely at
        // a 90 % choke: InDesign draws a solid 14 pt band, we drew
        // nothing at all.
        let (path, xf) = rect(20.0, 20.0, 90.0, 90.0);
        let stamps = inner_shadow_stamps(
            &path,
            &xf,
            &crate::InnerShadow {
                offset_x: 0.0,
                offset_y: 0.0,
                blur_radius: 16.0,
                color: Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                },
                opacity: 1.0,
                choke: 0.9,
                blend_mode: crate::BlendMode::Multiply,
            },
            288.0,
        );
        assert!(at(&stamps[0].mask, 32.0, 65.0) > 230, "solid 12 pt in");
        assert_eq!(at(&stamps[0].mask, 40.0, 65.0), 0, "nothing 20 pt in");
    }
}
