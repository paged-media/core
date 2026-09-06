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

//! InDesign's object effects, exported to PDF.
//!
//! The exporter used to draw only the drop shadow and the gradient
//! feather; inner shadow, both glows, bevel & emboss, satin and the two
//! feathers hit one collapsed arm that logged a debug line and drew
//! nothing. The canvas drew them, so every exported PDF disagreed with
//! the screen — and, on the annual's effects chapter, with InDesign.
//!
//! Each effect here is a port of the matching `render_*` in
//! `paged-gpu`'s CPU rasterizer: same σ, same choke/spread semantics,
//! same light model for the bevel. That is deliberate — two independent
//! implementations of "roughly a glow" agree only by luck, and the
//! exporter-vs-canvas lane compares them pixel for pixel.
//!
//! The PDF encoding is the one the drop shadow already used: an 8-bit
//! DeviceGray image as an `/SMask` on a 1×1 colour image, painted over
//! the effect's rectangle, with the blend mode carried by an ExtGState.
//! Interior effects clip to the path first. The two feathers are the
//! exception — they mask the object's OWN paint rather than adding ink,
//! so they ride a luminosity soft mask (see `page.rs`).

use paged_compose::{
    BevelDirection, BevelEmboss, BevelStyle, BevelTechnique, DirectionalFeather, Feather,
    FeatherCornerType, InnerGlow, InnerShadow, OuterGlow, PathData, Satin, Transform,
};
use pdf_writer::Content;

use crate::transparency::{stamp_mask, Grid, MaskRaster};
use crate::writer::{DocState, PageResources};

/// Everything an emitter needs besides its own parameters.
pub struct EffectCtx<'a> {
    pub state: &'a mut DocState,
    pub resources: &'a mut PageResources,
    pub effect_dpi: f32,
    pub xobject_counter: &'a mut u32,
}

/// Outer glow — the object's coverage grown by `spread`, blurred, with
/// the object itself punched back out so the glow only shows outside.
/// Emitted before the fill, which is where the walk meets it.
pub fn emit_outer_glow(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    glow: &OuterGlow,
) {
    let sigma = glow.blur_radius.max(0.0);
    let pad = 3.0 * sigma + glow.spread.abs() + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, ctx.effect_dpi) else {
        return;
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut mask = interior.clone();
    mask.morph(glow.spread);
    mask.blur(sigma);
    mask.subtract(&interior);
    mask.scale(glow.opacity);
    with_blend(content, ctx, glow.blend_mode, |content, ctx| {
        stamp_mask(
            content,
            ctx.state,
            ctx.resources,
            &mask,
            glow.color,
            (0.0, 0.0),
            ctx.xobject_counter,
        );
    });
}

/// Inner shadow — the offset OUTSIDE of the shape, blurred and kept
/// only where the shape is, so the edge it darkens is the inside one.
pub fn emit_inner_shadow(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    shadow: &InnerShadow,
) {
    let sigma = shadow.blur_radius.max(0.0);
    let pad =
        3.0 * sigma + shadow.choke.abs() + shadow.offset_x.abs().max(shadow.offset_y.abs()) + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, ctx.effect_dpi) else {
        return;
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut mask = MaskRaster::interior(grid, path, transform, (shadow.offset_x, shadow.offset_y));
    mask.morph(shadow.choke);
    mask.invert();
    mask.blur(sigma);
    mask.multiply(&interior);
    mask.scale(shadow.opacity);
    clipped(
        content,
        ctx,
        path,
        transform,
        shadow.blend_mode,
        |content, ctx| {
            stamp_mask(
                content,
                ctx.state,
                ctx.resources,
                &mask,
                shadow.color,
                (0.0, 0.0),
                ctx.xobject_counter,
            );
        },
    );
}

/// Inner glow — the outside blurred inward, kept inside the shape.
pub fn emit_inner_glow(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    glow: &InnerGlow,
) {
    let sigma = glow.blur_radius.max(0.0);
    let pad = 3.0 * sigma + glow.choke.abs() + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, ctx.effect_dpi) else {
        return;
    };
    let interior = MaskRaster::interior(grid, path, transform, (0.0, 0.0));
    let mut mask = interior.clone();
    mask.invert();
    mask.morph(glow.choke);
    mask.blur(sigma);
    mask.multiply(&interior);
    mask.scale(glow.opacity);
    clipped(
        content,
        ctx,
        path,
        transform,
        glow.blend_mode,
        |content, ctx| {
            stamp_mask(
                content,
                ctx.state,
                ctx.resources,
                &mask,
                glow.color,
                (0.0, 0.0),
                ctx.xobject_counter,
            );
        },
    );
}

/// Satin — the difference between two copies of the shape offset in
/// opposite directions along the angle, blurred; a soft interference
/// band that follows the outline.
pub fn emit_satin(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    satin: &Satin,
) {
    let sigma = satin.blur_radius.max(0.0);
    let pad = 3.0 * sigma + satin.distance.abs() + 1.0;
    let Some(grid) = Grid::for_path(path, transform, pad, ctx.effect_dpi) else {
        return;
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
    clipped(
        content,
        ctx,
        path,
        transform,
        satin.blend_mode,
        |content, ctx| {
            stamp_mask(
                content,
                ctx.state,
                ctx.resources,
                &a,
                satin.color,
                (0.0, 0.0),
                ctx.xobject_counter,
            );
        },
    );
}

/// Bevel & emboss — light a height field built from the blurred
/// coverage. Positive slope toward the light becomes the highlight
/// mask, negative the shadow mask; both are clipped to the shape.
pub fn emit_bevel_emboss(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    bevel: &BevelEmboss,
) {
    let technique_scale = match bevel.technique {
        BevelTechnique::Smooth => 0.5,
        BevelTechnique::ChiselSoft => 0.25,
        BevelTechnique::ChiselHard => 0.1,
    };
    let pad = 3.0 * bevel.size.max(0.0) + 2.0;
    let Some(grid) = Grid::for_path(path, transform, pad, ctx.effect_dpi) else {
        return;
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
    clip_to_path(content, path, transform, |content| {
        stamp_mask(
            content,
            ctx.state,
            ctx.resources,
            &hi,
            bevel.highlight_color,
            (0.0, 0.0),
            ctx.xobject_counter,
        );
        stamp_mask(
            content,
            ctx.state,
            ctx.resources,
            &sh,
            bevel.shadow_color,
            (0.0, 0.0),
            ctx.xobject_counter,
        );
    });
}

/// Basic feather — the coverage blurred at the corner type's σ and
/// choked. Returned rather than painted: a feather masks the object's
/// own paint, so `page.rs` hangs it on the fill as a soft mask.
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
fn remap_choke(mask: &mut MaskRaster, choke_pct: f32) {
    let c = (choke_pct / 100.0).clamp(0.0, 0.95);
    if c <= 0.0 {
        return;
    }
    let inv = 1.0 / (1.0 - c);
    for v in mask.data.iter_mut() {
        *v = (((*v as f32 / 255.0) * inv).clamp(0.0, 1.0) * 255.0) as u8;
    }
}

/// Run `f` with the path clipped in and the blend mode set.
fn clipped(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    blend: paged_compose::BlendMode,
    f: impl FnOnce(&mut Content, &mut EffectCtx<'_>),
) {
    content.save_state();
    crate::page::emit_transformed_clip(content, path, transform);
    crate::transparency::apply_gs(content, ctx.state, ctx.resources, Some(blend), None, false);
    f(content, ctx);
    content.restore_state();
}

/// Run `f` with only the blend mode set (no clip).
fn with_blend(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    blend: paged_compose::BlendMode,
    f: impl FnOnce(&mut Content, &mut EffectCtx<'_>),
) {
    content.save_state();
    crate::transparency::apply_gs(content, ctx.state, ctx.resources, Some(blend), None, false);
    f(content, ctx);
    content.restore_state();
}

/// Run `f` with the path clipped in, blend mode untouched.
fn clip_to_path(
    content: &mut Content,
    path: &PathData,
    transform: &Transform,
    f: impl FnOnce(&mut Content),
) {
    content.save_state();
    crate::page::emit_transformed_clip(content, path, transform);
    f(content);
    content.restore_state();
}
