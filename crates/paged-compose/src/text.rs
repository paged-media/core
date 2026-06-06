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

//! Convert a laid-out paragraph into display-list commands.
//!
//! Produces one `FillPath` command per glyph. Glyph outlines are
//! interned in the list's `PathBuffer` via a `(font_id, glyph_id)`
//! cache key so repeated glyphs (the common case) share tessellated
//! data.
//!
//! Coordinate system:
//! - Font outlines are y-up with the baseline at y=0.
//! - IDML pages are y-down with the top-left at (0, 0).
//! - The per-glyph transform scales by `point_size / units_per_em`
//!   and flips y, then translates to the glyph's (x, y) position on
//!   the page (in pt).
//!
//! All text input positions are in 1/64 pt, as produced by
//! `paged_text::layout`; we divide by 64 at the emit boundary.

use paged_text::layout::{LaidOutParagraph, PositionedGlyph};

use crate::display_list::{
    DisplayCommand, DisplayList, GlyphCacheKey, Paint, PathId, Stroke, Transform,
};
use crate::glyph::GlyphOutliner;

/// Advance precision used by `paged_text::layout`: positions are in
/// 1/64 pt. We divide by this when converting to float pt.
const ADVANCE_PRECISION: f32 = 64.0;

/// Emit `FillPath` commands for every glyph in `laid_out`.
///
/// - `font_id` identifies the font for glyph caching; callers pick a
///   scheme (hash of the font bytes, index into a font table, etc.)
///   and keep it stable for a single render.
/// - `point_size` is the em size the glyphs were shaped at.
/// - `paint_for(cluster)` returns the paint for the glyph at `cluster`
///   (byte offset into the source paragraph). Single-colour callers
///   pass `|_| Paint::Solid(my_color)`.
/// - `frame_origin_pt` is the page-space position of the frame's
///   top-left corner. Glyph positions are offset by it so the
///   commands live in page coordinates.
pub fn emit_paragraph<O, F>(
    laid_out: &LaidOutParagraph,
    font_id: u32,
    point_size: f32,
    paint_for: F,
    frame_origin_pt: (f32, f32),
    outliner: &O,
    list: &mut DisplayList,
) where
    O: GlyphOutliner,
    F: Fn(u32) -> Paint,
{
    emit_paragraph_blend(
        laid_out,
        font_id,
        point_size,
        paint_for,
        frame_origin_pt,
        outliner,
        list,
        crate::display_list::BlendMode::Normal,
    );
}

/// Like [`emit_paragraph`] but composites every glyph fill with
/// `blend_mode`. Normal (the default in [`emit_paragraph`]) keeps the
/// fast `FillPath` path; non-Normal modes route through
/// `FillPathBlend` so the rasterizer stamps each glyph through an
/// offscreen scratch and composites with the requested mode. Used to
/// honour a TextFrame's `<BlendingSetting>` on its body text.
pub fn emit_paragraph_blend<O, F>(
    laid_out: &LaidOutParagraph,
    font_id: u32,
    point_size: f32,
    paint_for: F,
    frame_origin_pt: (f32, f32),
    outliner: &O,
    list: &mut DisplayList,
    blend_mode: crate::display_list::BlendMode,
) where
    O: GlyphOutliner,
    F: Fn(u32) -> Paint,
{
    for line in &laid_out.lines {
        emit_glyph_slice_blend(
            &line.glyphs,
            font_id,
            point_size,
            &paint_for,
            frame_origin_pt,
            outliner,
            list,
            blend_mode,
        );
    }
}

/// Emit `FillPath` commands for a contiguous slice of glyphs that all
/// share `font_id` (and therefore one outliner). Multi-font callers
/// group glyphs by `glyph.font_id` and call this once per group.
pub fn emit_glyph_slice<O, F>(
    glyphs: &[PositionedGlyph],
    font_id: u32,
    point_size: f32,
    paint_for: F,
    frame_origin_pt: (f32, f32),
    outliner: &O,
    list: &mut DisplayList,
) where
    O: GlyphOutliner,
    F: Fn(u32) -> Paint,
{
    emit_glyph_slice_blend(
        glyphs,
        font_id,
        point_size,
        paint_for,
        frame_origin_pt,
        outliner,
        list,
        crate::display_list::BlendMode::Normal,
    );
}

/// Like [`emit_glyph_slice`] but emits `FillPathBlend` for non-Normal
/// blend modes.
pub fn emit_glyph_slice_blend<O, F>(
    glyphs: &[PositionedGlyph],
    font_id: u32,
    point_size: f32,
    paint_for: F,
    frame_origin_pt: (f32, f32),
    outliner: &O,
    list: &mut DisplayList,
    blend_mode: crate::display_list::BlendMode,
) where
    O: GlyphOutliner,
    F: Fn(u32) -> Paint,
{
    let upem = outliner.units_per_em();
    let scale = point_size / upem;
    let (ox, oy) = frame_origin_pt;
    let normal = matches!(blend_mode, crate::display_list::BlendMode::Normal);
    for g in glyphs {
        let Some(path_id) = get_or_intern_glyph_outline(font_id, g.glyph_id, outliner, list) else {
            continue;
        };
        let gx = ox + g.x as f32 / ADVANCE_PRECISION;
        let gy = oy + g.y as f32 / ADVANCE_PRECISION;
        let paint = paint_for(g.cluster);
        // Column-major 2×3 as `[a b c d tx ty]`: scale by (scale,
        // scale) and flip y by negating the y-axis scale. Then
        // translate to (gx, gy). `x_scale` folds IDML `HorizontalScale`
        // into the glyph affine (P-08); the breaker already accounted
        // for the advance, so glyphs are merely stretched in place.
        let sx = scale * g.x_scale;
        // `y_scale` folds IDML `VerticalScale` into the glyph affine's
        // y-axis (the `-scale` y-flip term), scaling glyph height about
        // the baseline (`gy`) without touching the advance or leading.
        let sy = scale * g.y_scale;
        let transform = Transform([sx, 0.0, 0.0, -sy, gx, gy]);
        // Concept 3 — glyph-run side-channel (collect_glyph_runs
        // builds only): record the parallel text-run entry BEFORE
        // pushing, so command_index points at the outline command.
        if let Some(runs) = list.glyph_runs.as_mut() {
            runs.push(crate::display_list::GlyphRunEntry {
                command_index: list.commands.len() as u32,
                font_id,
                glyph_id: g.glyph_id,
                font_size: point_size,
                transform,
                paint,
                unicode: g.ch,
                is_stroke: false,
            });
        }
        if normal {
            list.push(DisplayCommand::FillPath {
                path_id,
                paint,
                transform,
            });
        } else {
            list.push(DisplayCommand::FillPathBlend {
                path_id,
                paint,
                transform,
                blend_mode,
            });
        }
    }
}

/// Emit `StrokePath` commands for glyphs whose run cascade resolves a
/// visible text stroke. `stroke_for(cluster)` returns `Some((paint,
/// stroke))` for clusters that should be outlined and `None` for
/// clusters that should remain fill-only — the typical sparse case
/// where only a handful of runs carry a `StrokeColor`. The stroke
/// commands are appended *after* the matching fills so they land on
/// top of the glyph in display order (matching InDesign's default
/// `OutsideAlignment` look: fill underneath, stroke around the
/// silhouette).
pub fn emit_glyph_slice_stroke<O, S>(
    glyphs: &[PositionedGlyph],
    font_id: u32,
    point_size: f32,
    stroke_for: S,
    frame_origin_pt: (f32, f32),
    outliner: &O,
    list: &mut DisplayList,
) where
    O: GlyphOutliner,
    S: Fn(u32) -> Option<(Paint, Stroke)>,
{
    let upem = outliner.units_per_em();
    let scale = point_size / upem;
    let (ox, oy) = frame_origin_pt;
    for g in glyphs {
        let Some((paint, stroke)) = stroke_for(g.cluster) else {
            continue;
        };
        let Some(path_id) = get_or_intern_glyph_outline(font_id, g.glyph_id, outliner, list) else {
            continue;
        };
        let gx = ox + g.x as f32 / ADVANCE_PRECISION;
        let gy = oy + g.y as f32 / ADVANCE_PRECISION;
        // Same column-major 2×3 as `emit_glyph_slice`: y-flip + scale,
        // then translate to (gx, gy). Stroke widths are document-space
        // pt so the rasterizer reads `stroke.width` directly rather
        // than transforming through `scale`. `x_scale` mirrors the fill
        // path so a stretched run keeps its stroke aligned (P-08).
        let sx = scale * g.x_scale;
        // `y_scale` folds IDML `VerticalScale` into the glyph affine's
        // y-axis (the `-scale` y-flip term), scaling glyph height about
        // the baseline (`gy`) without touching the advance or leading.
        let sy = scale * g.y_scale;
        let transform = Transform([sx, 0.0, 0.0, -sy, gx, gy]);
        // Concept 3 — glyph-run side-channel (stroked text keeps
        // is_stroke so the exporter falls back to the outline: PDF
        // text render mode 1 strokes are a refinement).
        if let Some(runs) = list.glyph_runs.as_mut() {
            runs.push(crate::display_list::GlyphRunEntry {
                command_index: list.commands.len() as u32,
                font_id,
                glyph_id: g.glyph_id,
                font_size: point_size,
                transform,
                paint,
                unicode: g.ch,
                is_stroke: true,
            });
        }
        list.push(DisplayCommand::StrokePath {
            path_id,
            paint,
            stroke,
            transform,
        });
    }
}

fn get_or_intern_glyph_outline(
    font_id: u32,
    glyph_id: u32,
    outliner: &impl GlyphOutliner,
    list: &mut DisplayList,
) -> Option<PathId> {
    let key = GlyphCacheKey { font_id, glyph_id }.to_u64();
    // `PathBuffer::intern` already treats a repeated key as a cache
    // hit and does not store a second copy. Build the outline only on
    // a miss by probing the cache first.
    if let Some(existing) = list.paths.find_by_key(key) {
        return Some(existing);
    }
    let outline = outliner.outline(glyph_id)?;
    let (id, _fresh) = list.paths.intern(key, outline);
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_list::Color;
    use crate::glyph::UnitSquareOutliner;
    use paged_text::compose::{ComposeOptions, MonospaceMeasurer};
    use paged_text::layout::{layout_paragraph, LayoutOptions};

    fn laid_out(text: &str) -> LaidOutParagraph {
        let shaper = MonospaceMeasurer::new(500, 500);
        let opts = LayoutOptions {
            compose: ComposeOptions {
                column_width: 500 * 8,
                column_widths: None,
                tolerance: 10.0,
                stretch_ratio: 1.0,
                shrink_ratio: 0.5,
                desired_space_ratio: 1.0,
                looseness: 0,
                hyphenator: None,
                hyphen_penalty: 50,
                hyphenation_zone: 0,
                kinsoku_enforce: false,
                mojikumi_half_width: false,
            },
            line_height: 64 * 14,
            first_baseline: 64 * 10,
            alignment: paged_text::Alignment::Left,
            leading_override: None,
        };
        layout_paragraph(text, &shaper, &opts)
    }

    #[test]
    fn emit_produces_one_command_per_glyph() {
        let p = laid_out("hello world foo bar");
        let mut list = DisplayList::new();
        emit_paragraph(
            &p,
            1,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        let total_glyphs: usize = p.lines.iter().map(|l| l.glyphs.len()).sum();
        assert_eq!(list.commands.len(), total_glyphs);
    }

    #[test]
    fn repeated_glyph_shares_path_id() {
        // "aaaa" — every glyph id identical → every FillPath reuses
        // the same interned path.
        let p = laid_out("aaaa aaaa");
        let mut list = DisplayList::new();
        emit_paragraph(
            &p,
            1,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        // Path buffer holds one outline for 'a' plus one for ' '.
        // (MonospaceMeasurer issues a real glyph per space too.)
        assert!(
            list.paths.len() <= 2,
            "expected ≤ 2 unique paths, got {}",
            list.paths.len()
        );
    }

    #[test]
    fn glyph_positions_are_offset_by_frame_origin() {
        let p = laid_out("abc");
        let mut list = DisplayList::new();
        emit_paragraph(
            &p,
            1,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (100.0, 200.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        let first = match &list.commands[0] {
            DisplayCommand::FillPath { transform, .. } => transform.0,
            other => panic!("expected FillPath, got {other:?}"),
        };
        // tx = 100 + glyph.x/64 = 100 + 0 = 100 (first glyph at x=0).
        assert!((first[4] - 100.0).abs() < 1e-4, "tx = {}", first[4]);
        // ty should be 200 + baseline_y/64 = 200 + 10 = 210.
        assert!((first[5] - 210.0).abs() < 1e-4, "ty = {}", first[5]);
    }

    #[test]
    fn paint_picker_receives_cluster_byte_offset() {
        // "ab" with MonospaceMeasurer → 2 glyphs at clusters 0 and 1.
        let p = laid_out("ab");
        let mut list = DisplayList::new();
        let red = Paint::Solid(Color::rgba(1.0, 0.0, 0.0, 1.0));
        let blue = Paint::Solid(Color::rgba(0.0, 0.0, 1.0, 1.0));
        emit_paragraph(
            &p,
            1,
            12.0,
            |c| if c == 0 { red } else { blue },
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        assert_eq!(list.commands.len(), 2);
        let paints: Vec<Paint> = list
            .commands
            .iter()
            .map(|c| match c {
                DisplayCommand::FillPath { paint, .. } => *paint,
                other => panic!("expected FillPath, got {other:?}"),
            })
            .collect();
        assert_eq!(paints[0], red);
        assert_eq!(paints[1], blue);
    }

    #[test]
    fn y_axis_is_flipped_by_transform_matrix() {
        let p = laid_out("x");
        let mut list = DisplayList::new();
        emit_paragraph(
            &p,
            1,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        let m = match &list.commands[0] {
            DisplayCommand::FillPath { transform, .. } => transform.0,
            other => panic!("expected FillPath, got {other:?}"),
        };
        // d (y-scale) must be negative — fonts are y-up, pages y-down.
        assert!(m[3] < 0.0, "y-scale not flipped: {:?}", m);
    }

    #[test]
    fn vertical_scale_scales_glyph_affine_y_axis() {
        // `y_scale` folds IDML VerticalScale into the affine's d term
        // (the y-flip), independent of `x_scale` (HorizontalScale → a).
        let glyph = |x_scale: f32, y_scale: f32| PositionedGlyph {
            glyph_id: 65,
            cluster: 0,
            x: 0,
            y: 0,
            x_advance: 0,
            font_id: 0,
            point_size: 0.0,
            underline: false,
            strikethru: false,
            x_scale,
            y_scale,
            ch: None,
        };
        let mut list = DisplayList::new();
        for g in [glyph(1.0, 1.0), glyph(1.0, 2.0)] {
            emit_glyph_slice(
                &[g],
                1,
                12.0,
                |_| Paint::Solid(Color::BLACK),
                (0.0, 0.0),
                &UnitSquareOutliner::default(),
                &mut list,
            );
        }
        let aff = |i: usize| match &list.commands[i] {
            DisplayCommand::FillPath { transform, .. } => transform.0,
            other => panic!("expected FillPath, got {other:?}"),
        };
        let (m1, m2) = (aff(0), aff(1));
        // d (y-scale) doubles with VerticalScale=200%; a (x-scale) is
        // untouched. Both d's stay negative (the page-down y-flip).
        assert!(m1[3] < 0.0 && m2[3] < 0.0);
        assert!((m2[3] - 2.0 * m1[3]).abs() < 1e-4, "d1={} d2={}", m1[3], m2[3]);
        assert!((m1[0] - m2[0]).abs() < 1e-4, "x-scale must not change");
    }

    #[test]
    fn horizontal_scale_scales_glyph_affine_x_axis_only() {
        // HorizontalScale → the affine's `a` term (x-scale). Doubling
        // x_scale doubles `a` and leaves `d` (y-scale) untouched —
        // mirror of the VerticalScale test.
        let glyph = |x_scale: f32, y_scale: f32| PositionedGlyph {
            glyph_id: 65,
            cluster: 0,
            x: 0,
            y: 0,
            x_advance: 0,
            font_id: 0,
            point_size: 0.0,
            underline: false,
            strikethru: false,
            x_scale,
            y_scale,
            ch: None,
        };
        let mut list = DisplayList::new();
        for g in [glyph(1.0, 1.0), glyph(2.0, 1.0)] {
            emit_glyph_slice(
                &[g],
                1,
                12.0,
                |_| Paint::Solid(Color::BLACK),
                (0.0, 0.0),
                &UnitSquareOutliner::default(),
                &mut list,
            );
        }
        let aff = |i: usize| match &list.commands[i] {
            DisplayCommand::FillPath { transform, .. } => transform.0,
            other => panic!("expected FillPath, got {other:?}"),
        };
        let (m1, m2) = (aff(0), aff(1));
        assert!((m2[0] - 2.0 * m1[0]).abs() < 1e-4, "a1={} a2={}", m1[0], m2[0]);
        assert!((m1[3] - m2[3]).abs() < 1e-4, "y-scale must not change");
    }

    #[test]
    fn baseline_shift_offsets_glyph_y_via_positioned_y() {
        // Super/subscript baseline shift reaches the affine as the
        // glyph's `y` (1/64 pt, frame-relative): a lifted glyph has a
        // smaller `ty` than an unshifted one (page y grows downward, so
        // "up" = a more negative y offset added to the origin). This
        // pins that the `y` field flows straight into the affine's `ty`.
        let glyph = |y: i32| PositionedGlyph {
            glyph_id: 65,
            cluster: 0,
            x: 0,
            y,
            x_advance: 0,
            font_id: 0,
            point_size: 0.0,
            underline: false,
            strikethru: false,
            x_scale: 1.0,
            y_scale: 1.0,
            ch: None,
        };
        let mut list = DisplayList::new();
        // y = 0 (baseline) vs y = -64 (lifted one pt above baseline).
        for g in [glyph(0), glyph(-64)] {
            emit_glyph_slice(
                &[g],
                1,
                12.0,
                |_| Paint::Solid(Color::BLACK),
                (0.0, 100.0),
                &UnitSquareOutliner::default(),
                &mut list,
            );
        }
        let ty = |i: usize| match &list.commands[i] {
            DisplayCommand::FillPath { transform, .. } => transform.0[5],
            other => panic!("expected FillPath, got {other:?}"),
        };
        // Origin oy = 100; baseline glyph lands at 100, lifted glyph at
        // 100 + (-64/64) = 99 (one pt higher up the page).
        assert!((ty(0) - 100.0).abs() < 1e-4, "baseline ty={}", ty(0));
        assert!((ty(1) - 99.0).abs() < 1e-4, "lifted ty={}", ty(1));
    }

    #[test]
    fn emit_glyph_slice_caches_per_font_id() {
        // Two glyph slices with the same glyph_id but different
        // font_ids must intern two distinct outlines — otherwise a
        // 'B' in font A would steal the path of a 'B' in font B.
        let mut list = DisplayList::new();
        let glyphs_a = vec![PositionedGlyph {
            glyph_id: 65,
            cluster: 0,
            x: 0,
            y: 0,
            x_advance: 0,
            font_id: 0,
            point_size: 0.0,
            underline: false,
            strikethru: false,
            x_scale: 1.0,
            y_scale: 1.0,
            ch: None,
        }];
        let glyphs_b = vec![PositionedGlyph {
            glyph_id: 65,
            cluster: 0,
            x: 0,
            y: 0,
            x_advance: 0,
            font_id: 0,
            point_size: 0.0,
            underline: false,
            strikethru: false,
            x_scale: 1.0,
            y_scale: 1.0,
            ch: None,
        }];
        emit_glyph_slice(
            &glyphs_a,
            111,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        emit_glyph_slice(
            &glyphs_b,
            222,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        // Two FillPath commands, two distinct paths in the buffer.
        assert_eq!(list.commands.len(), 2);
        assert_eq!(
            list.paths.len(),
            2,
            "different font_ids must intern distinct outlines"
        );
    }
}
