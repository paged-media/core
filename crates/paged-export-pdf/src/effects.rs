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

use paged_compose::{BevelEmboss, InnerGlow, InnerShadow, OuterGlow, PathData, Satin, Transform};
use pdf_writer::Content;

use crate::transparency::stamp_mask;
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
/// Paint one effect's stamps. The masks come from
/// `paged_compose::mask`, so this lane and the GPU lane draw the same
/// arithmetic; all that differs is how a tinted mask reaches the page.
fn emit_stamps(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    stamps: Vec<paged_compose::mask::EffectStamp>,
) {
    for stamp in stamps {
        if stamp.mask.is_empty() {
            continue;
        }
        let paint = |content: &mut Content, ctx: &mut EffectCtx<'_>| {
            stamp_mask(
                content,
                ctx.state,
                ctx.resources,
                &stamp.mask,
                stamp.color,
                (0.0, 0.0),
                ctx.xobject_counter,
            );
        };
        if stamp.clip_to_path {
            clipped(content, ctx, path, transform, stamp.blend_mode, paint);
        } else {
            with_blend(content, ctx, stamp.blend_mode, paint);
        }
    }
}

pub fn emit_outer_glow(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    glow: &OuterGlow,
) {
    let stamps = paged_compose::mask::outer_glow_stamps(path, transform, glow, ctx.effect_dpi);
    emit_stamps(content, ctx, path, transform, stamps);
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
    let stamps = paged_compose::mask::inner_shadow_stamps(path, transform, shadow, ctx.effect_dpi);
    emit_stamps(content, ctx, path, transform, stamps);
}

/// Inner glow — the outside blurred inward, kept inside the shape.
pub fn emit_inner_glow(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    glow: &InnerGlow,
) {
    let stamps = paged_compose::mask::inner_glow_stamps(path, transform, glow, ctx.effect_dpi);
    emit_stamps(content, ctx, path, transform, stamps);
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
    let stamps = paged_compose::mask::satin_stamps(path, transform, satin, ctx.effect_dpi);
    emit_stamps(content, ctx, path, transform, stamps);
}

/// Bevel & emboss — light a height field built from the blurred
/// coverage. Positive slope toward the light becomes the highlight
/// mask, negative the shadow mask; both are clipped to the shape and
/// painted in one clip so the two halves cannot separate.
pub fn emit_bevel_emboss(
    content: &mut Content,
    ctx: &mut EffectCtx<'_>,
    path: &PathData,
    transform: &Transform,
    bevel: &BevelEmboss,
) {
    let stamps = paged_compose::mask::bevel_emboss_stamps(path, transform, bevel, ctx.effect_dpi);
    clip_to_path(content, path, transform, |content| {
        for stamp in stamps {
            if stamp.mask.is_empty() {
                continue;
            }
            stamp_mask(
                content,
                ctx.state,
                ctx.resources,
                &stamp.mask,
                stamp.color,
                (0.0, 0.0),
                ctx.xobject_counter,
            );
        }
    });
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
