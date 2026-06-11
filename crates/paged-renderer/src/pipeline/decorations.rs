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

use paged_compose::{
    emit_ellipse, emit_glyph_slice, Color, DisplayList, Paint, Rect, Stroke, TtfOutliner,
};


/// Phase 7 — emit Kenten emphasis marks above glyphs whose source
/// run carries a `KentenKind` other than `"None"`. The mark is a
/// small filled black circle stamped above the base glyph's centre
/// at a fixed fraction of the run's point size. Per-glyph cluster
/// → run lookup is done inline so we don't need to thread a picker
/// or build a side index.
///
/// Position: mark sits ~0.4 × point_size above the line's baseline
/// (above the cap line of typical CJK fonts). Mark diameter =
/// 0.18 × point_size (slightly smaller than ideographic full-
/// width). The mark scales with the run's point size so kenten
/// over headlines vs. body text reads at proportional weight.
pub(super) fn emit_kenten_for_line(
    line: &paged_text::layout::LaidOutLine,
    paragraph: &paged_parse::Paragraph,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    use paged_text::shape::ADVANCE_PRECISION;
    if line.glyphs.is_empty() || paragraph.runs.is_empty() {
        return;
    }
    // Build a tiny cluster → run index. Linear walk over
    // paragraph.runs accumulating byte lengths.
    let mut run_byte_ends: Vec<usize> = Vec::with_capacity(paragraph.runs.len());
    let mut acc = 0usize;
    for r in &paragraph.runs {
        acc += r.text.len();
        run_byte_ends.push(acc);
    }
    // Fast bail when no run has a Kenten mark to render.
    let any_kenten = resolved_runs.iter().any(|r| {
        r.kenten_kind
            .as_deref()
            .map(|k| !k.eq_ignore_ascii_case("None"))
            .unwrap_or(false)
    });
    if !any_kenten {
        return;
    }
    let (ox, oy) = frame_origin_pt;
    let mark_paint = Paint::Solid(Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    for g in &line.glyphs {
        let cluster = g.cluster as usize;
        // Find the run that owns this cluster.
        let run_idx = run_byte_ends
            .iter()
            .position(|&end| cluster < end)
            .unwrap_or(run_byte_ends.len() - 1);
        let Some(resolved) = resolved_runs.get(run_idx) else {
            continue;
        };
        let kind = match resolved.kenten_kind.as_deref() {
            Some(k) if !k.eq_ignore_ascii_case("None") => k,
            _ => continue,
        };
        let point_size = g.point_size.max(1.0);
        let mark_diameter = point_size * 0.18;
        // Centre of mark = centre of glyph's advance, sitting
        // 0.4 × point_size above the baseline. Mark fill colour
        // currently follows a fixed black (KentenKind variants
        // map to the same simple dot today).
        let _ = kind; // variants share the simple-dot shape MVP.
        let glyph_x_pt = g.x as f32 / ADVANCE_PRECISION;
        let glyph_adv_pt = g.x_advance as f32 / ADVANCE_PRECISION;
        let centre_x = ox + glyph_x_pt + glyph_adv_pt * 0.5;
        let baseline_y_pt = g.y as f32 / ADVANCE_PRECISION;
        let centre_y = oy + baseline_y_pt - point_size * 0.95;
        let rect = Rect {
            x: centre_x - mark_diameter * 0.5,
            y: centre_y - mark_diameter * 0.5,
            w: mark_diameter,
            h: mark_diameter,
        };
        emit_ellipse(rect, mark_paint, list);
    }
}

/// Phase 7 — emit ruby annotations above runs whose `ruby_flag` is
/// set. The MVP shapes `ruby_string` once per ruby-tagged run via
/// the document's fallback font at 0.5 × base point size, centers
/// the result horizontally over the base run's glyph span, and
/// places it 1.05 × base point size above the line's baseline (i.e.
/// just above the cap line).
///
/// Limitations called out:
/// - Uses the fallback font for shaping (the run's own font may
///   carry better glyphs for Japanese kana but the fallback at
///   least always has SOME glyph). This is good enough for visible
///   confirmation; replacing it with the run's resolved face is a
///   follow-up.
/// - `PerCharacter` ruby (one ruby char per base char) collapses to
///   the same "centered group" placement as `GroupRuby`. Per-
///   character distribution requires aligning ruby char N over
///   base char N which the MVP skips.
pub(super) fn emit_ruby_for_line(
    line: &paged_text::layout::LaidOutLine,
    paragraph: &paged_parse::Paragraph,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    font_bytes: &[u8],
    frame_origin_pt: (f32, f32),
    list: &mut DisplayList,
) {
    use paged_text::shape::ADVANCE_PRECISION;
    if line.glyphs.is_empty() || paragraph.runs.is_empty() {
        return;
    }
    // Fast bail when no run has ruby.
    let any_ruby = resolved_runs.iter().any(|r| r.ruby_flag.unwrap_or(false));
    if !any_ruby {
        return;
    }
    // Construct a shaping + outlining face for the ruby text.
    let Some(rb_face) = rustybuzz::Face::from_slice(font_bytes, 0) else {
        return;
    };
    let Ok(ttf_face) = ttf_parser::Face::parse(font_bytes, 0) else {
        return;
    };
    let outliner = TtfOutliner::new(&ttf_face);
    // Build cluster → run index lookup.
    let mut run_byte_ends: Vec<usize> = Vec::with_capacity(paragraph.runs.len());
    let mut acc = 0usize;
    for r in &paragraph.runs {
        acc += r.text.len();
        run_byte_ends.push(acc);
    }
    let (ox, oy) = frame_origin_pt;
    let ruby_paint = Paint::Solid(Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    // For each run with ruby, find its glyph span in the line and
    // emit centered ruby. We do per-run independently so multiple
    // ruby runs in one line work.
    for (run_idx, resolved) in resolved_runs.iter().enumerate() {
        if !resolved.ruby_flag.unwrap_or(false) {
            continue;
        }
        let ruby_text = match resolved.ruby_string.as_deref() {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };
        let run_start = if run_idx == 0 {
            0
        } else {
            run_byte_ends[run_idx - 1]
        };
        let run_end = run_byte_ends[run_idx];
        // Find min/max x among glyphs in this run's cluster range.
        let mut x_min = i32::MAX;
        let mut x_max = i32::MIN;
        let mut base_point_size: f32 = 0.0;
        let mut baseline_y_64: i32 = 0;
        for g in &line.glyphs {
            let c = g.cluster as usize;
            if c < run_start || c >= run_end {
                continue;
            }
            x_min = x_min.min(g.x);
            x_max = x_max.max(g.x + g.x_advance);
            base_point_size = base_point_size.max(g.point_size);
            baseline_y_64 = g.y; // last wins; all glyphs on a line share baseline
        }
        if x_min == i32::MAX || base_point_size <= 0.0 {
            continue;
        }
        // Shape the ruby string at half the base point size.
        let ruby_pt = base_point_size * 0.5;
        let shaped = paged_text::shape_run(&rb_face, ruby_text, ruby_pt);
        if shaped.glyphs.is_empty() {
            continue;
        }
        // Centre the shaped advance over the base x span.
        let base_x_left_pt = x_min as f32 / ADVANCE_PRECISION;
        let base_x_right_pt = x_max as f32 / ADVANCE_PRECISION;
        let base_centre_pt = (base_x_left_pt + base_x_right_pt) * 0.5;
        let ruby_advance_pt = shaped.total_advance as f32 / ADVANCE_PRECISION;
        let ruby_origin_x_pt = base_centre_pt - ruby_advance_pt * 0.5;
        // Position above the baseline by 1.05 × base point size.
        let baseline_y_pt = baseline_y_64 as f32 / ADVANCE_PRECISION;
        let ruby_origin_y_pt = baseline_y_pt - base_point_size * 1.05;
        // Convert shape glyphs to PositionedGlyph at the ruby
        // origin. Each glyph's x is the running advance sum.
        let mut positioned: Vec<paged_text::PositionedGlyph> =
            Vec::with_capacity(shaped.glyphs.len());
        let mut cursor = 0i32;
        for g in &shaped.glyphs {
            positioned.push(paged_text::PositionedGlyph {
                glyph_id: g.glyph_id,
                cluster: g.cluster,
                x: cursor + g.x_offset,
                y: g.y_offset,
                x_advance: g.x_advance,
                font_id: u32::MAX, // sentinel: ruby uses the fallback face directly via outliner
                point_size: ruby_pt,
                underline: false,
                strikethru: false,
                x_scale: 1.0,
                y_scale: 1.0,
                skew_deg: 0.0,
                ch: None,
            });
            cursor = cursor.saturating_add(g.x_advance);
        }
        emit_glyph_slice(
            &positioned,
            u32::MAX,
            ruby_pt,
            |_| ruby_paint,
            (ox + ruby_origin_x_pt, oy + ruby_origin_y_pt),
            &outliner,
            list,
        );
    }
}

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
