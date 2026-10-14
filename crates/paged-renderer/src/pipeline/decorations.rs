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

//! Line-level text decorations — kenten (emphasis marks), ruby, and
//! underline/strike decorations — extracted from pipeline/mod.rs (audit
//! 1.6b). Net-zero behaviour.

use super::*;

use paged_compose::{Color, DisplayList, Paint, Rect, Stroke};

/// Walk a laid-out line's glyphs and emit horizontal stroke
/// commands for any underlined or struck-through ranges. The stroke
/// uses the run's resolved fill colour (per cluster, via the same
/// picker as the glyphs themselves) so coloured text gets coloured
/// decoration.
pub(super) fn emit_line_decorations(
    line: &paged_text::layout::LaidOutLine,
    picker: &RunPaintPicker,
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    use paged_text::shape::ADVANCE_PRECISION;
    if line.glyphs.is_empty() {
        return;
    }
    // Two passes — underline (12% of em below baseline) then
    // strikethrough (30% above) — so a glyph with both gets two
    // stripes. Offsets are crude approximations until we read the
    // font's `OS/2` table for the spec'd y_offset / strikeout_pos.
    const UNDERLINE_OFFSET_EM: f32 = 0.12;
    const STRIKETHRU_OFFSET_EM: f32 = -0.30;
    type Pred = fn(&paged_text::PositionedGlyph) -> bool;
    let underline: Pred = |g| g.underline;
    let strikethru: Pred = |g| g.strikethru;
    for (predicate, y_offset_factor) in [
        (underline, UNDERLINE_OFFSET_EM),
        (strikethru, STRIKETHRU_OFFSET_EM),
    ] {
        let mut start = 0;
        while start < line.glyphs.len() {
            if !predicate(&line.glyphs[start]) {
                start += 1;
                continue;
            }
            let mut end = start + 1;
            while end < line.glyphs.len() && predicate(&line.glyphs[end]) {
                end += 1;
            }
            let g0 = &line.glyphs[start];
            let g_last = &line.glyphs[end - 1];
            let x_start_pt = frame_origin_pt.0 + (g0.x as f32) / ADVANCE_PRECISION;
            let x_end_pt =
                frame_origin_pt.0 + ((g_last.x + g_last.x_advance) as f32) / ADVANCE_PRECISION;
            let baseline_pt = frame_origin_pt.1 + (line.baseline_y as f32) / ADVANCE_PRECISION;
            let y_pt = baseline_pt + g0.point_size * y_offset_factor;
            let stroke_w = (g0.point_size * 0.06_f32).max(0.4);
            // Decoration paint matches the run's fill at the start
            // glyph's cluster.
            let paint = picker.pick(g0.cluster);
            paged_compose::emit_line(
                x_start_pt,
                y_pt,
                x_end_pt,
                y_pt,
                Stroke::new(stroke_w),
                paint,
                list,
            );
            start = end;
        }
    }
}

/// A3 — pink "this text renders in a substituted font" highlight.
/// Same contiguous-segment walk as [`emit_line_decorations`], keyed on
/// [`paged_text::PositionedGlyph::substituted`]: per substituted
/// segment, one `FillPath` rect spanning the segment horizontally and
/// ascent→descent vertically. `metrics_for_glyph` maps a glyph's
/// `font_id` to the face's parsed metrics (the caller resolves the
/// wght-mixed glyph id back to the `FontTable::metrics_for` key); a
/// miss falls back to the pipeline's usual 0.8 / 0.2 em split.
///
/// Emitted BEFORE the line's glyph fills so the pink composites behind
/// the text, and gated on `PipelineOptions::degraded_asset_markers` at
/// the call site (default off — exports stay byte-identical).
pub(super) fn emit_substituted_highlight_for_line(
    line: &paged_text::layout::LaidOutLine,
    metrics_for_glyph: &dyn Fn(u32) -> Option<FontMetrics>,
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    use paged_text::shape::ADVANCE_PRECISION;
    if line.glyphs.is_empty() || !line.glyphs.iter().any(|g| g.substituted) {
        return;
    }
    let pink = Paint::Solid(Color {
        r: SUBSTITUTED_HIGHLIGHT_R,
        g: SUBSTITUTED_HIGHLIGHT_G,
        b: SUBSTITUTED_HIGHLIGHT_B,
        a: 1.0,
    });
    let mut start = 0;
    while start < line.glyphs.len() {
        if !line.glyphs[start].substituted {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < line.glyphs.len() && line.glyphs[end].substituted {
            end += 1;
        }
        let g0 = &line.glyphs[start];
        let g_last = &line.glyphs[end - 1];
        let x_start_pt = frame_origin_pt.0 + (g0.x as f32) / ADVANCE_PRECISION;
        let x_end_pt =
            frame_origin_pt.0 + ((g_last.x + g_last.x_advance) as f32) / ADVANCE_PRECISION;
        let baseline_pt = frame_origin_pt.1 + (line.baseline_y as f32) / ADVANCE_PRECISION;
        let (ascender, descender) = metrics_for_glyph(g0.font_id)
            .map(|m| (m.ascender, m.descender))
            .unwrap_or((0.8, 0.2));
        let point_size = g0.point_size.max(1.0);
        let rect = Rect {
            x: x_start_pt,
            y: baseline_pt - ascender * point_size,
            w: x_end_pt - x_start_pt,
            h: (ascender + descender) * point_size,
        };
        if rect.w > 0.0 && rect.h > 0.0 {
            paged_compose::emit_rect_transformed(rect, Transform::IDENTITY, pink, list);
        }
        start = end;
    }
}
