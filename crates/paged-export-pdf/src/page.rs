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

//! The per-page walk: the same DisplayList the rasterizers consume,
//! emitted as a PDF content stream. One page-level CTM handles the
//! y-down → y-up flip; every leaf primitive wraps its own transform
//! in a private q…Q so clips stay live across siblings (see
//! `gstate`).

use std::collections::HashMap;

use paged_compose::{DisplayCommand, GlyphRunEntry, Paint, Transform};
use pdf_writer::{Content, Finish, Name};

use crate::gstate::{FrameKind, StateStack};
use crate::writer::{DocState, FinishedPage, PageResources};
use crate::{ExportDiagnostic, ExportError, ExportInput};

pub fn export_page(
    state: &mut DocState,
    input: &ExportInput<'_>,
    page: &paged_renderer::BuiltPage,
    diagnostics: &mut Vec<ExportDiagnostic>,
) -> Result<(), ExportError> {
    let list = &page.list;
    let trim_w = page.width_pt;
    let trim_h = page.height_pt;

    // Geometry: bleed from options override or the document's
    // declared preference; marks margin only when marks are on.
    let bleed = input.options.bleed.override_pt.unwrap_or(input.doc_bleed);
    let [bleed_top, bleed_left, bleed_bottom, bleed_right] = bleed;
    let marks_on = input.options.marks.crop_marks
        || input.options.marks.registration_marks
        || input.options.marks.color_bars
        || input.options.marks.page_info;
    let marks_margin = if marks_on {
        let offset = if input.options.marks.offset_pt > 0.0 {
            input.options.marks.offset_pt
        } else {
            6.0
        };
        offset + 18.0 + 6.0
    } else {
        0.0
    };
    let off_left = bleed_left + marks_margin;
    let off_bottom = bleed_bottom + marks_margin;
    let media_w = trim_w + bleed_left + bleed_right + marks_margin * 2.0;
    let media_h = trim_h + bleed_top + bleed_bottom + marks_margin * 2.0;

    // Boxes in PDF (y-up media) coords.
    let trim_box = pdf_writer::Rect::new(
        off_left,
        off_bottom,
        off_left + trim_w,
        off_bottom + trim_h,
    );
    let bleed_box = pdf_writer::Rect::new(
        off_left - bleed_left,
        off_bottom - bleed_bottom,
        off_left + trim_w + bleed_right,
        off_bottom + trim_h + bleed_top,
    );
    let media_box = pdf_writer::Rect::new(0.0, 0.0, media_w, media_h);

    let mut content = Content::new();
    let mut resources = PageResources::default();
    let mut stack = StateStack::new();
    let mut xobject_counter: u32 = 0;
    let mut upem_cache: HashMap<u32, f32> = HashMap::new();

    // Page-level CTM: translate the trim origin into the media box,
    // then flip y (content coordinates are y-down page-local pt).
    content.save_state();
    content.transform([1.0, 0.0, 0.0, -1.0, off_left, off_bottom + trim_h]);

    // Index the glyph side-channel by command index.
    let glyph_by_cmd: HashMap<u32, Vec<&GlyphRunEntry>> = match &list.glyph_runs {
        Some(table) => {
            let mut m: HashMap<u32, Vec<&GlyphRunEntry>> = HashMap::new();
            for e in &table.entries {
                m.entry(e.command_index).or_default().push(e);
            }
            m
        }
        None => HashMap::new(),
    };
    // Fonts that failed the fsType gate stay as outlines.
    let mut outline_font: HashMap<u32, bool> = HashMap::new();

    let commands = &list.commands;
    let mut i = 0usize;
    while i < commands.len() {
        // Glyph-paralleled command? Collect the consecutive slice
        // sharing (font, size, paint) and emit ONE text object at
        // this z-position.
        if let Some(entries) = glyph_by_cmd.get(&(i as u32)) {
            let entry = entries[0];
            let embeddable = *outline_font.entry(entry.font_id).or_insert_with(|| {
                let ok = input
                    .fonts
                    .and_then(|f| f.font_bytes(entry.font_id))
                    .map(|bytes| state.fonts.check_embeddable(entry.font_id, bytes))
                    .unwrap_or(false);
                ok
            });
            if embeddable && input.fonts.is_some() {
                // Gather the consecutive run.
                let mut slice: Vec<&GlyphRunEntry> = Vec::new();
                let mut j = i;
                while j < commands.len() {
                    match glyph_by_cmd.get(&(j as u32)) {
                        Some(es)
                            if es.iter().all(|e| {
                                e.font_id == entry.font_id
                                    && (e.font_size - entry.font_size).abs() < 1e-3
                                    && paint_key(&e.paint) == paint_key(&entry.paint)
                                    && !e.is_stroke
                            }) =>
                        {
                            slice.extend(es.iter().copied());
                            j += 1;
                        }
                        _ => break,
                    }
                }
                if !slice.is_empty() && !entry.is_stroke {
                    let mut font_name = String::new();
                    let mut font_ref = pdf_writer::Ref::new(1);
                    let pairs: Vec<(&GlyphRunEntry, u16)> = slice
                        .iter()
                        .map(|e| {
                            let (name, fref, new_gid) = state.fonts.use_glyph(
                                &mut state.refs,
                                e.font_id,
                                e.glyph_id,
                                e.unicode,
                            );
                            font_name = name;
                            font_ref = fref;
                            (*e, new_gid)
                        })
                        .collect();
                    resources.fonts.entry(font_name.clone()).or_insert(font_ref);
                    let upem = *upem_cache.entry(entry.font_id).or_insert_with(|| {
                        input
                            .fonts
                            .and_then(|f| f.font_bytes(entry.font_id))
                            .and_then(|b| ttf_parser::Face::parse(b, 0).ok())
                            .map(|f| f.units_per_em() as f32)
                            .unwrap_or(1000.0)
                    });
                    content.save_state();
                    crate::color::set_fill_paint(
                        &mut content,
                        state,
                        &mut resources,
                        input,
                        &list.spot_inks,
                        &entry.paint,
                    );
                    crate::text::emit_text_slice(
                        &mut content,
                        &font_name,
                        entry.font_size,
                        upem,
                        &pairs,
                    );
                    content.restore_state();
                    i = j;
                    continue;
                }
            }
            // Not embeddable / no font source → fall through and
            // emit the outline command below (the fallback the
            // concept demands — never silent, the diagnostic lands
            // in write_fonts).
        }

        match &commands[i] {
            DisplayCommand::FillPath { path_id, paint, transform } => {
                emit_fill(state, input, &mut content, &mut resources, list, *path_id, paint, transform, None, false);
            }
            DisplayCommand::FillPathBlend { path_id, paint, transform, blend_mode } => {
                emit_fill(state, input, &mut content, &mut resources, list, *path_id, paint, transform, Some(*blend_mode), false);
            }
            DisplayCommand::FillPathOverprint { path_id, paint, transform } => {
                emit_fill(state, input, &mut content, &mut resources, list, *path_id, paint, transform, None, true);
            }
            DisplayCommand::StrokePath { path_id, paint, stroke, transform } => {
                emit_stroke(state, input, &mut content, &mut resources, list, *path_id, paint, stroke, transform, false);
            }
            DisplayCommand::StrokePathOverprint { path_id, paint, stroke, transform } => {
                emit_stroke(state, input, &mut content, &mut resources, list, *path_id, paint, stroke, transform, true);
            }
            DisplayCommand::PushClip { path_id, transform } => {
                stack.push(&mut content, FrameKind::Clip);
                if let Some(path) = list.paths.get(*path_id) {
                    content.save_state();
                    content.transform(transform.0);
                    crate::path::emit_path(&mut content, path);
                    content.restore_state();
                    // NOTE: W must precede the path-painting no-op n
                    // and the path must be in the CURRENT CTM — we
                    // can't wrap the clip path itself in q/Q because
                    // the clip would die with the Q. Re-emit under
                    // the frame CTM instead:
                }
                // Re-emit correctly: clip path under the live CTM.
                if let Some(path) = list.paths.get(*path_id) {
                    emit_transformed_clip(&mut content, path, transform);
                }
            }
            DisplayCommand::PopClip(_) => {
                stack.pop(&mut content, FrameKind::Clip);
            }
            DisplayCommand::BeginBlendGroup { blend_mode, opacity, .. } => {
                stack.push(&mut content, FrameKind::BlendGroup);
                crate::transparency::apply_gs(
                    &mut content,
                    state,
                    &mut resources,
                    Some(*blend_mode),
                    Some(*opacity),
                    false,
                );
            }
            DisplayCommand::EndBlendGroup(_) => {
                stack.pop(&mut content, FrameKind::BlendGroup);
            }
            DisplayCommand::PushLayer { blend_mode, opacity, .. } => {
                stack.push(&mut content, FrameKind::Layer);
                crate::transparency::apply_gs(
                    &mut content,
                    state,
                    &mut resources,
                    Some(*blend_mode),
                    Some(*opacity),
                    false,
                );
            }
            DisplayCommand::PopLayer(_) => {
                stack.pop(&mut content, FrameKind::Layer);
            }
            DisplayCommand::Image { image_id, transform } => {
                if let Some(img) = list.image(*image_id) {
                    if let Some(img_ref) =
                        crate::image::write_image(state, img, image_id.0, diagnostics)
                    {
                        let name = crate::image::image_resource_name(xobject_counter);
                        xobject_counter += 1;
                        resources.x_objects.insert(name.clone(), img_ref);
                        // Image XObjects paint the unit square with
                        // the FIRST row at the TOP edge (y=1). Our
                        // transform maps a y-DOWN unit square; pre-
                        // compose a unit flip so rows land upright
                        // under the page CTM.
                        let t = transform.compose(&Transform([1.0, 0.0, 0.0, -1.0, 0.0, 1.0]));
                        content.save_state();
                        content.transform(t.0);
                        content.x_object(Name(name.as_bytes()));
                        content.restore_state();
                    }
                }
            }
            DisplayCommand::DropShadow { path_id, transform, shadow }
            | DisplayCommand::PathShadow { path_id, transform, shadow } => {
                if let Some(path) = list.paths.get(*path_id) {
                    crate::transparency::emit_shadow_stamp(
                        &mut content,
                        state,
                        &mut resources,
                        path,
                        transform,
                        shadow,
                        input.options.effect_dpi.max(72.0),
                        &mut xobject_counter,
                    );
                }
            }
            // v1: the remaining blur-based effects are documented
            // gaps (the canvas-side raster look is the reference;
            // shadows — the headline effect — export above).
            DisplayCommand::InnerShadow { .. }
            | DisplayCommand::OuterGlow { .. }
            | DisplayCommand::InnerGlow { .. }
            | DisplayCommand::BevelEmboss { .. }
            | DisplayCommand::Satin { .. }
            | DisplayCommand::Feather { .. }
            | DisplayCommand::DirectionalFeather { .. }
            | DisplayCommand::GradientFeather { .. } => {
                tracing::debug!("paged-export-pdf: effect command not yet exported");
            }
        }
        i += 1;
    }

    stack.flush(&mut content);
    content.restore_state(); // the page CTM save

    // Marks live OUTSIDE the flipped content space (media coords).
    if marks_on {
        let geo = crate::marks::MarkGeometry {
            media_w,
            media_h,
            trim: [off_left, off_bottom, off_left + trim_w, off_bottom + trim_h],
            bleed: [
                off_left - bleed_left,
                off_bottom - bleed_bottom,
                off_left + trim_w + bleed_right,
                off_bottom + trim_h + bleed_top,
            ],
        };
        crate::marks::emit_marks(&mut content, state, &mut resources, &geo, &input.options.marks);
    }

    // Write the content stream (Flate-compressed).
    let data = content.finish();
    let compressed = {
        use std::io::Write as _;
        let mut enc =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = enc.write_all(&data);
        enc.finish().unwrap_or_default()
    };
    let content_ref = state.refs.alloc();
    {
        let mut s = state.pdf.stream(content_ref, &compressed);
        s.filter(pdf_writer::Filter::FlateDecode);
        s.finish();
    }
    let page_ref = state.refs.alloc();
    state.pages.push(FinishedPage {
        page_ref,
        content_ref,
        media_box,
        trim_box,
        bleed_box,
        resources,
    });
    Ok(())
}

/// Clip path emission: transformed point-by-point into the CURRENT
/// CTM (no q/Q — the clip must survive), then `W n`.
fn emit_transformed_clip(
    content: &mut Content,
    path: &paged_compose::PathData,
    transform: &Transform,
) {
    let transformed = transform_path(path, transform);
    crate::path::emit_path(content, &transformed);
    content.clip_nonzero();
    content.end_path();
}

fn transform_path(path: &paged_compose::PathData, t: &Transform) -> paged_compose::PathData {
    use paged_compose::PathSegment as S;
    let m = t.0;
    let map = |x: f32, y: f32| -> (f32, f32) {
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    };
    paged_compose::PathData {
        segments: path
            .segments
            .iter()
            .map(|seg| match *seg {
                S::MoveTo { x, y } => {
                    let (x, y) = map(x, y);
                    S::MoveTo { x, y }
                }
                S::LineTo { x, y } => {
                    let (x, y) = map(x, y);
                    S::LineTo { x, y }
                }
                S::QuadTo { cx, cy, x, y } => {
                    let (cx, cy) = map(cx, cy);
                    let (x, y) = map(x, y);
                    S::QuadTo { cx, cy, x, y }
                }
                S::CubicTo { cx1, cy1, cx2, cy2, x, y } => {
                    let (cx1, cy1) = map(cx1, cy1);
                    let (cx2, cy2) = map(cx2, cy2);
                    let (x, y) = map(x, y);
                    S::CubicTo { cx1, cy1, cx2, cy2, x, y }
                }
                S::Close => S::Close,
            })
            .collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_fill(
    state: &mut DocState,
    input: &ExportInput<'_>,
    content: &mut Content,
    resources: &mut PageResources,
    list: &paged_compose::DisplayList,
    path_id: paged_compose::PathId,
    paint: &Paint,
    transform: &Transform,
    blend: Option<paged_compose::BlendMode>,
    overprint: bool,
) {
    let Some(path) = list.paths.get(path_id) else { return };
    content.save_state();
    if blend.is_some() || (overprint && crate::paint_is_cmyk(paint)) {
        crate::transparency::apply_gs(content, state, resources, blend, None, overprint);
    }
    match paint {
        Paint::LinearGradient(id) => {
            if let Some(g) = list.linear_gradient(*id) {
                // Clip to the path, then paint the shading over the
                // path's bbox in local space.
                content.transform(transform.0);
                crate::path::emit_path(content, path);
                content.clip_nonzero();
                content.end_path();
                let bbox = path_bbox(path);
                let sh_ref = crate::color::write_linear_shading(state, input, g, bbox);
                let name = format!("Sh{}", resources.shadings.len());
                resources.shadings.insert(name.clone(), sh_ref);
                content.shading(Name(name.as_bytes()));
            }
        }
        Paint::RadialGradient(id) => {
            if let Some(g) = list.radial_gradient(*id) {
                content.transform(transform.0);
                crate::path::emit_path(content, path);
                content.clip_nonzero();
                content.end_path();
                let bbox = path_bbox(path);
                let sh_ref = crate::color::write_radial_shading(state, input, g, bbox);
                let name = format!("Sh{}", resources.shadings.len());
                resources.shadings.insert(name.clone(), sh_ref);
                content.shading(Name(name.as_bytes()));
            }
        }
        _ => {
            crate::color::set_fill_paint(content, state, resources, input, &list.spot_inks, paint);
            content.transform(transform.0);
            crate::path::emit_path(content, path);
            content.fill_nonzero();
        }
    }
    content.restore_state();
}

#[allow(clippy::too_many_arguments)]
fn emit_stroke(
    state: &mut DocState,
    input: &ExportInput<'_>,
    content: &mut Content,
    resources: &mut PageResources,
    list: &paged_compose::DisplayList,
    path_id: paged_compose::PathId,
    paint: &Paint,
    stroke: &paged_compose::Stroke,
    transform: &Transform,
    overprint: bool,
) {
    let Some(path) = list.paths.get(path_id) else { return };
    content.save_state();
    if overprint && crate::paint_is_cmyk(paint) {
        crate::transparency::apply_gs(content, state, resources, None, None, true);
    }
    crate::color::set_stroke_paint(content, state, resources, input, &list.spot_inks, paint);
    // Stroke widths are document-space pt: transform the PATH
    // points instead of the CTM so `w` stays in pt.
    let transformed = transform_path(path, transform);
    crate::path::emit_stroke_params(content, stroke);
    crate::path::emit_path(content, &transformed);
    content.stroke();
    content.restore_state();
}

fn path_bbox(path: &paged_compose::PathData) -> paged_compose::Rect {
    use paged_compose::PathSegment as S;
    let mut min = (f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN);
    let mut consider = |x: f32, y: f32| {
        min.0 = min.0.min(x);
        min.1 = min.1.min(y);
        max.0 = max.0.max(x);
        max.1 = max.1.max(y);
    };
    for seg in &path.segments {
        match *seg {
            S::MoveTo { x, y } | S::LineTo { x, y } => consider(x, y),
            S::QuadTo { cx, cy, x, y } => {
                consider(cx, cy);
                consider(x, y);
            }
            S::CubicTo { cx1, cy1, cx2, cy2, x, y } => {
                consider(cx1, cy1);
                consider(cx2, cy2);
                consider(x, y);
            }
            S::Close => {}
        }
    }
    if min.0 > max.0 {
        return paged_compose::Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
    }
    paged_compose::Rect {
        x: min.0,
        y: min.1,
        w: (max.0 - min.0).max(1e-3),
        h: (max.1 - min.1).max(1e-3),
    }
}

fn paint_key(p: &Paint) -> u64 {
    // Cheap grouping key for consecutive text runs.
    match p {
        Paint::Solid(c) => {
            let r = (c.r * 1000.0) as u64;
            let g = (c.g * 1000.0) as u64;
            let b = (c.b * 1000.0) as u64;
            let a = (c.a * 1000.0) as u64;
            1 << 60 | r << 40 | g << 24 | b << 8 | a & 0xFF
        }
        Paint::Cmyk { c, m, y, k, spot, .. } => {
            let cc = (*c * 255.0) as u64;
            let mm = (*m * 255.0) as u64;
            let yy = (*y * 255.0) as u64;
            let kk = (*k * 255.0) as u64;
            let s = spot.map(|s| s.0 as u64 + 1).unwrap_or(0);
            2 << 60 | cc << 44 | mm << 32 | yy << 20 | kk << 8 | s & 0xFF
        }
        Paint::LinearGradient(id) => 3 << 60 | id.0 as u64,
        Paint::RadialGradient(id) => 4 << 60 | id.0 as u64,
    }
}
