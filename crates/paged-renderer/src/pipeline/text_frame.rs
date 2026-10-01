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

//! Text-frame geometry: wrap shapes, per-line wrap widths, auto-sizing, frame transform + scene-layer emission. Extracted from pipeline/mod.rs (1.6b).

use super::*;

use paged_compose::{emit_glyph_slice, Color, DropShadow, Paint, Rect, Transform, TtfOutliner};
use paged_model::{Graphic, TextFrame};
use paged_scene::Document;

use crate::module::{Geometry, ResolvedFrame};

pub(super) struct WrapPlan {
    /// Per-line x-shifts in 1/64 pt. Index `i` = shift for line i.
    pub(super) line_x_shifts_64: Vec<i32>,
    /// Parallel marker: `twin_after[i] == true` means line `i`
    /// shares a baseline with line `i-1`. Used by the post-layout
    /// pass to implement BothSides wrap (text on both sides of an
    /// obstacle in the same row).
    pub(super) twin_after: Vec<bool>,
    /// Per composer line: the band lies outside the frame's shape (an
    /// oval's last line past the far edge, a polygon narrower than the
    /// frame at that height). The composer still gets a width for the
    /// index, but the emitter treats the line as not fitting — it
    /// flows on or oversets, as InDesign does with a line that has no
    /// room inside the outline.
    pub(super) no_room: Vec<bool>,
    /// Points the first baseline must move DOWN, past where a
    /// rectangular frame would have put it, before the shape admits
    /// the paragraph's first word. Zero for every frame that is not
    /// shaped, and for a shape wide enough at its top. See
    /// `shaped_first_baseline_push_pt`.
    pub(super) first_baseline_push_pt: f32,
}

/// Polygon vertices for a chain frame, expressed in *spread coords*.
/// Returned only when:
///   - the frame's anchors form a non-rectangular polygon (so AABB
///     layout would place text outside the actual outline);
///   - the frame's `ItemTransform` is upright (identity rotation/scale,
///     translation only). Rotated polygon frames fall back to the
///     AABB path because per-line shifts in spread coords would not
///     compose cleanly with the frame's post-emit rotation.
///
/// `None` means "treat the frame as its AABB" (the legacy behaviour).
pub(crate) fn frame_polygon_spread(frame: &TextFrame) -> Option<Vec<(f32, f32)>> {
    if frame.anchors.len() < 3 {
        return None;
    }
    // Inner-coord rectangularity test: 2 unique x values + 2 unique y
    // values => axis-aligned rect (the common case, every plain text
    // frame). Polygon clipping would be a no-op here; skip.
    let mut xs: Vec<f32> = frame.anchors.iter().map(|a| a.anchor.0).collect();
    let mut ys: Vec<f32> = frame.anchors.iter().map(|a| a.anchor.1).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let eq = |a: f32, b: f32| (a - b).abs() < 1e-3;
    if frame.anchors.len() == 4
        && eq(xs[0], xs[1])
        && eq(xs[2], xs[3])
        && eq(ys[0], ys[1])
        && eq(ys[2], ys[3])
    {
        return None;
    }
    // Only handle upright frames. The renderer rotates rotated text
    // frames post-emit; per-line shifts pre-rotation would interact
    // badly with a non-AABB clip.
    let m = frame
        .item_transform
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let upright = (m[1].abs() < 1e-5)
        && (m[2].abs() < 1e-5)
        && ((m[0] - 1.0).abs() < 1e-5)
        && ((m[3] - 1.0).abs() < 1e-5);
    if !upright {
        return None;
    }
    // Each anchor's straight-segment chain — Bezier control points
    // are approximated by the polyline through `anchor` only, per the
    // implementation plan (curve-flattening can land later without
    // affecting the boundary test above).
    Some(
        frame
            .anchors
            .iter()
            .map(|a| apply_matrix(&m, a.anchor.0, a.anchor.1))
            .collect(),
    )
}

/// W1.10 — build the frame's outline as a [`paged_text::FrameShape`]
/// (one flattened, closed contour per `<GeometryPathType>`, in spread
/// coords) for wrap-INSIDE line layout. Unlike [`frame_polygon_spread`]
/// (which walks anchors only, collapsing ovals to diamonds and ignoring
/// holes), this:
///   * flattens each cubic Bezier edge so ovals / rounded corners
///     conform to the true curve (InDesign stores ovals as four
///     cardinal anchors with 0.5523·r handles — anchors-only would be a
///     diamond);
///   * honours `subpath_starts` so a compound path (donut: outer ring +
///     inner hole) keeps its contours separate — the even-odd scanline
///     in `FrameShape::segments_in_band` then carves the hole.
///
/// Returns `None` (⇒ AABB fallback) for the same cases as
/// `frame_polygon_spread`: fewer than 3 anchors, an axis-aligned rect,
/// or a rotated/sheared frame.
pub(super) fn frame_shape_spread(frame: &TextFrame) -> Option<paged_text::FrameShape> {
    // Gate on the same rectangularity / upright tests as the clip path
    // so the layout carve and the clip stay in lockstep.
    frame_polygon_spread(frame)?;
    let m = frame
        .item_transform
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let anchors = &frame.anchors;
    // Materialise subpath ranges — same rules as
    // `polygon_path_from_anchors_with_open`: an empty / single-entry
    // `subpath_starts` is one contour over all anchors; otherwise each
    // start opens a contour ending where the next begins.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    if frame.subpath_starts.len() <= 1 {
        ranges.push((0, anchors.len()));
    } else {
        let mut starts: Vec<usize> = frame
            .subpath_starts
            .iter()
            .copied()
            .filter(|&s| s < anchors.len())
            .collect();
        starts.sort_unstable();
        starts.dedup();
        if starts.first() != Some(&0) {
            starts.insert(0, 0);
        }
        for i in 0..starts.len() {
            let lo = starts[i];
            let hi = starts.get(i + 1).copied().unwrap_or(anchors.len());
            if hi > lo {
                ranges.push((lo, hi));
            }
        }
    }
    // Flattening tolerance. 0.5 pt was fine while a chord fed the
    // composer directly, but the band is now snapped to whole points,
    // and near the top of an oval the chord's x moves arbitrarily fast
    // in y: half a point of sagitta cost the first line of a 320 pt
    // circle EIGHT points of measure. 0.02 pt was still a point too
    // narrow one point below a 200 pt circle's top (`shaped-bands`: the
    // chord there moves seven points of x per point of y, so 0.02 of
    // sagitta is 0.14 of edge, across a grid line); 0.002 pt holds the
    // edge to hundredths — ~160 segments per quarter of a 100 pt arc,
    // which the scanline walks per line.
    const FLATTEN_TOL_PT: f32 = 0.002;
    let mut contours: Vec<paged_text::Contour> = Vec::with_capacity(ranges.len());
    for (range_idx, (lo, hi)) in ranges.iter().copied().enumerate() {
        // Open contours describe lassoed strokes / text-on-path hosts,
        // not fillable regions — skip them for the inside test.
        if frame.subpath_open.get(range_idx).copied().unwrap_or(false) {
            continue;
        }
        let sub = &anchors[lo..hi];
        if sub.len() < 2 {
            continue;
        }
        let mut pts: Vec<(f32, f32)> = Vec::new();
        let p0 = apply_matrix(&m, sub[0].anchor.0, sub[0].anchor.1);
        pts.push(p0);
        // Edge between each adjacent anchor + the closing edge back to
        // the first anchor (IDML polygons are closed).
        for k in 0..sub.len() {
            let from = &sub[k];
            let to = &sub[(k + 1) % sub.len()];
            let a = apply_matrix(&m, from.anchor.0, from.anchor.1);
            let c1 = apply_matrix(&m, from.right.0, from.right.1);
            let c2 = apply_matrix(&m, to.left.0, to.left.1);
            let b = apply_matrix(&m, to.anchor.0, to.anchor.1);
            let steps = paged_text::cubic_steps_for_tolerance(a, c1, c2, b, FLATTEN_TOL_PT);
            paged_text::flatten_cubic(a, c1, c2, b, steps, &mut pts);
        }
        // The closing edge re-appended the first anchor; drop the
        // duplicate so the contour is a clean closed ring.
        if pts.len() >= 2 && pts.first() == pts.last() {
            pts.pop();
        }
        contours.push(pts);
    }
    let shape = paged_text::FrameShape::from_contours(contours);
    if shape.is_empty() {
        None
    } else {
        Some(shape)
    }
}

// The polygon scanline / hole-carve geometry that used to live here
// (polygon_x_at_y / pairs_from_xs / carve_holes) moved into
// `paged_text::frame_shape::FrameShape` (W1.10), which adds Bezier
// flattening + whole-band intersection so ovals and compound paths lay
// out correctly. `build_perline_wrap_widths` calls
// `FrameShape::segments_in_band` directly.

/// How far short of a grid point an edge may fall and still count as ON
/// it: an edge InDesign puts exactly on a point — a straight side at
/// x = 272 — comes back from the float arithmetic a hair either side.
/// Kept to a thousandth: a narrow ellipse's chord at 85.992 pt is 85 in
/// InDesign, and a hundredth would round it up.
const BAND_GRID_EPS_PT: f32 = 0.001;

/// A band's ends on InDesign's whole-point grid.
///
/// Measured on InDesign 20.0.1 (`shaped-bands`, and the 23 left edges
/// behind `text_in_shape.rs`): both ends are FLOORED to whole points, and
/// the grid starts at the text area's left edge — the frame's edge plus
/// its inset and its stroke's share — not at the page's or the frame's
/// origin. `text-in-shape`'s donut (frame at x = 137.638, 4 pt inset plus
/// half a 0.75 pt stroke) has its bands at 142.013 / 235.013 / 359.013 /
/// 453.013, every one a whole number of points from 142.013. An end
/// exactly on a grid point stays where it is: a straight side at 272
/// keeps 272, it does not lose a point on the right.
fn snap_band(a: f32, b: f32, grid_origin: f32) -> (f32, f32) {
    let snap = |x: f32| grid_origin + (x - grid_origin + BAND_GRID_EPS_PT).floor();
    (snap(a), snap(b))
}

/// The advance of the paragraph's first word, in pt — what a shaped
/// frame's first line has to find room for.
fn first_word_advance_pt(runs: &[paged_text::StyledRun]) -> f32 {
    let Some(r) = runs.first() else {
        return 0.0;
    };
    let rest = r.text.trim_start();
    let word = match rest.find(char::is_whitespace) {
        Some(i) => &rest[..i],
        None => rest,
    };
    if word.is_empty() {
        return 0.0;
    }
    paged_text::shape_run(r.face, word, r.point_size).total_advance as f32
        / paged_text::shape::ADVANCE_PRECISION
}

/// How far DOWN a shaped frame's first baseline moves before the
/// outline is wide enough to start on.
///
/// A rectangular frame puts the first baseline at
/// `top + inset + ascent`; inside a shape that is narrow up there —
/// the top of an oval, a triangle's apex — no line fits at that
/// height, and InDesign walks the baseline down **one whole point at a
/// time** until the band admits the first word. Measured on InDesign
/// 20.0.1 with a wedge whose band widens 0.1 pt per point of descent,
/// so the stop is bracketed to a tenth of a point: with the first word
/// at 16.84 pt the search stopped where the snapped band reached 17 and
/// not at 16, at 20.37 pt where it reached 21 and not 20, at 2.78 pt
/// where it reached 3 and not 2, and a 33.69 pt word at 22 pt type
/// where it reached 35 and not 34. The band is snapped exactly as a
/// laid-out line's is, so the two agree by construction.
fn shaped_first_baseline_push_pt(
    shape: &paged_text::FrameShape,
    text_top_pt: f32,
    frame_height_pt: f32,
    first_drop_pt: f32,
    inset_pt: f32,
    grid_origin: f32,
    word_pt: f32,
) -> f32 {
    // One point per step, and never past the frame's own bottom: a
    // shape that never opens wide enough has no first line to place,
    // and the caller's `no_room` handling takes it from there. The
    // bound matters — each step is an erosion query, and an unbounded
    // walk on a tall narrow shape would be thousands of them.
    let max_steps = frame_height_pt.max(0.0).ceil() as usize + 1;
    for k in 0..max_steps {
        let step = k as f32;
        // The first line's slug runs from the text area's top (moved
        // with the line) down to its baseline.
        let widest = shape
            .segments_in_band_eroded(
                text_top_pt + step,
                text_top_pt + step + first_drop_pt,
                inset_pt,
            )
            .into_iter()
            .map(|(a, b)| {
                let (a, b) = snap_band(a, b, grid_origin);
                b - a
            })
            .fold(0.0_f32, f32::max);
        if widest >= word_pt {
            return step;
        }
    }
    0.0
}

pub(super) fn build_perline_wrap_widths(
    em: &StoryEmitter,
    styled_runs: &[paged_text::StyledRun],
    lopts: &mut paged_text::LayoutOptions,
    frame_first_paragraph: bool,
) -> WrapPlan {
    let empty = WrapPlan {
        line_x_shifts_64: Vec::new(),
        twin_after: Vec::new(),
        no_room: Vec::new(),
        first_baseline_push_pt: 0.0,
    };
    // Polygon clip per chain frame — enabled when the frame's
    // <PathGeometry> is non-rectangular (e.g. triangle, pentagon).
    // Indexed by frame_idx; `None` means treat the frame as its AABB.
    // The `FrameShape` carries the *flattened, contour-separated*
    // outline (ovals conform to the curve; compound paths keep their
    // hole) used to carve each line's available x-segments (W1.10);
    // the parallel `chain_polygons` AABB-diamond stays as the cheap
    // gate / legacy fallback for frames whose shape build declines.
    let chain_polygons: Vec<Option<Vec<(f32, f32)>>> =
        em.chain.iter().map(|f| frame_polygon_spread(f)).collect();
    let chain_shapes: Vec<Option<paged_text::FrameShape>> =
        em.chain.iter().map(|f| frame_shape_spread(f)).collect();
    let any_polygon_clip = chain_polygons.iter().any(|p| p.is_some());
    if em.frame_idx != 0 && !any_polygon_clip {
        // After the head frame fills, the existing emit loop
        // advances to chain[1+] using a fixed first-baseline
        // reset; per-line wrap inside overflow frames is layered
        // on by the chain walk below — handled when the head
        // frame's paragraph composes. We still need to engage when
        // a downstream frame is polygon-clipped so paragraphs that
        // start *inside* the polygon get the per-line carve.
        return empty;
    }
    let any_chain_overlap = em
        .chain_spread_bounds
        .iter()
        .zip(em.chain_wrap_rects.iter())
        .any(|(b, ws)| {
            ws.iter().any(|s| {
                let w = s.bounds;
                w.bottom > b.top && w.top < b.bottom && w.right > b.left && w.left < b.right
            })
        });
    if !any_chain_overlap && !any_polygon_clip {
        return empty;
    }
    // The paragraph's leading: its own when the caller set one, else
    // the first run's point size × 1.2 (paged-text's auto-leading
    // default).
    let head_size_pt = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let leading_64 = match lopts.leading_override.unwrap_or(lopts.line_height) {
        l if l > 0 => l,
        _ => ((head_size_pt * 1.2 * paged_text::shape::ADVANCE_PRECISION).round() as i32).max(1),
    };
    let leading_pt = leading_64 as f32 / paged_text::shape::ADVANCE_PRECISION;
    let scalar_width_64 =
        (em.column_width_pt.unwrap_or(0.0) * paged_text::shape::ADVANCE_PRECISION).round() as i32;

    let mut widths_64: Vec<i32> = Vec::new();
    let mut shifts_64: Vec<i32> = Vec::new();
    let mut twin_after: Vec<bool> = Vec::new();
    let mut no_room: Vec<bool> = Vec::new();
    // Composer lines up to here sit inside the frame (chain); past it
    // they are overset.
    let mut visible_lines = 0usize;
    let mut last_row_fits = false;

    // Walk every frame in the chain. Head frame starts at y_cursor
    // (already accounts for FirstBaselineOffset + SpaceBefore);
    // overflow frames reset to the same first-baseline the existing
    // emit loop uses (`paragraph_size * 0.8`). Each frame contributes
    // its own widths to the combined slice; once layout produces
    // lines the existing emit pass discovers per-line frame
    // assignment and reads x-shifts by absolute line index.
    // Paragraphs that start mid-chain skip the preceding frames so
    // the widths slice starts at the *current* frame.
    let start_frame = em.frame_idx;
    // The first baseline of a paragraph that OPENS a shaped frame is
    // walked down until the outline has room for its first word.
    let first_baseline_push_pt = if frame_first_paragraph {
        match (
            chain_shapes.get(start_frame).and_then(|s| s.as_ref()),
            em.chain_spread_bounds.get(start_frame),
            em.chain.get(start_frame),
        ) {
            (Some(shape), Some(bounds), Some(frame)) => {
                let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
                let first_baseline_pt =
                    em.y_cursor.max(0) as f32 / paged_text::shape::ADVANCE_PRECISION;
                shaped_first_baseline_push_pt(
                    shape,
                    bounds.top + insets[0],
                    bounds.height(),
                    first_baseline_pt - insets[0],
                    insets.iter().copied().fold(0.0_f32, f32::max),
                    bounds.left + insets[1],
                    first_word_advance_pt(styled_runs),
                )
            }
            _ => 0.0,
        }
    } else {
        0.0
    };
    let push_64 = (first_baseline_push_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;
    for (frame_idx, frame_bounds) in em.chain_spread_bounds.iter().enumerate() {
        if frame_idx < start_frame {
            continue;
        }
        let frame_left_pt = frame_bounds.left;
        let frame_right_pt = frame_bounds.right;
        let frame = em.chain[frame_idx];
        let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
        let frame_height_pt = frame_bounds.height();
        let frame_first_baseline_64 = if frame_idx == start_frame {
            em.y_cursor.max(0) + push_64
        } else {
            (head_size_pt * 0.8 * paged_text::shape::ADVANCE_PRECISION).round() as i32
        };
        let remaining_height_pt = (frame_height_pt
            - frame_first_baseline_64 as f32 / paged_text::shape::ADVANCE_PRECISION)
            .max(0.0);
        let mut n_lines = (remaining_height_pt / leading_pt).ceil() as usize + 1;
        n_lines = n_lines.min(512);
        if n_lines == 0 {
            continue;
        }
        let wraps = &em.chain_wrap_rects[frame_idx];
        let shape = chain_shapes[frame_idx].as_ref();
        // Frames without a shaped outline and without any wrap overlap
        // emit scalar-width entries — preserves the legacy "no
        // per-line carve" behaviour for plain rectangle frames in a
        // chain whose polygon-clipped frame appears later. Without
        // this guard, the AABB-based width for a slightly rotated
        // sibling differs enough from `column_width_pt` to derail
        // Knuth-Plass for the entire story.
        let frame_has_wraps = wraps.iter().any(|s| {
            let w = s.bounds;
            w.bottom > frame_bounds.top
                && w.top < frame_bounds.bottom
                && w.right > frame_bounds.left
                && w.left < frame_bounds.right
        });
        let frame_legacy = shape.is_none() && !frame_has_wraps;
        let frame_first_line = widths_64.len();
        for i in 0..n_lines {
            if last_row_fits {
                visible_lines = widths_64.len();
            }
            let baseline_pt = (frame_first_baseline_64 + (i as i32) * leading_64) as f32
                / paged_text::shape::ADVANCE_PRECISION;
            last_row_fits = baseline_pt <= frame_height_pt - insets[2] + 0.01;
            if frame_legacy {
                widths_64.push(scalar_width_64);
                shifts_64.push(0);
                twin_after.push(false);
                no_room.push(false);
                continue;
            }
            // The line's SLUG in spread coords — the band a shaped
            // outline is measured across. Measured on InDesign 20.0.1
            // (`shaped-bands`, `text_in_shape.rs`): it runs from the
            // PREVIOUS line's baseline down to this one's, whichever
            // paragraph that line belonged to, and the band is the
            // narrowest the outline gets anywhere in it. The frame's
            // first line reaches up to the text area's top (moved down
            // with any first-baseline push): its ascent for
            // `AscentOffset`, its leading for `LeadingOffset`. Nothing
            // below the baseline counts: an apex-down wedge places its
            // lines exactly where the chord at the baseline says.
            let slug_top_rel = if i > 0 {
                baseline_pt - leading_pt
            } else {
                let prev_64 = em
                    .frame_max_baseline_64
                    .get(frame_idx)
                    .copied()
                    .unwrap_or(0);
                if frame_idx == start_frame && !frame_first_paragraph && prev_64 > 0 {
                    prev_64 as f32 / paged_text::shape::ADVANCE_PRECISION
                } else if frame_idx == start_frame {
                    insets[0] + first_baseline_push_pt
                } else {
                    insets[0]
                }
            };
            let line_top = frame_bounds.top + slug_top_rel;
            let line_bottom = frame_bounds.top + baseline_pt;

            let frame_inner_left = frame_left_pt + insets[1];
            let frame_inner_right = frame_right_pt - insets[3];
            // Build the *gap list* of open horizontal segments on this
            // line. For shaped frames (ovals, triangles, pentagons,
            // compound paths with holes), seed segments from the
            // outline's interior x-intervals across the line's whole
            // vertical band — so a glyph's box never crosses the actual
            // curve and a circle's top line comes out shorter than its
            // middle line. Plain rectangle frames start from the AABB
            // inner range. The band intersection (vs. a baseline-only
            // sample) and the bezier flattening behind `FrameShape` are
            // the W1.10 upgrade over the prior anchors-only diamond.
            let mut segments: Vec<(f32, f32)> = if let Some(shape) = shape {
                // A shaped frame's inset erodes the OUTLINE by that
                // distance; it does not slide each chord's ends inward
                // (measured — see `segments_in_band_eroded`). InDesign
                // carries a single inset for a non-rectangular frame,
                // writing it as one scalar; the largest of the four is
                // the reading that never lets text touch the outline.
                let inset = insets.iter().copied().fold(0.0_f32, f32::max);
                shape
                    .segments_in_band_eroded(line_top, line_bottom, inset)
                    .into_iter()
                    // The whole-point grid from the text area's left
                    // edge (`snap_band`).
                    .map(|(a, b)| snap_band(a, b, frame_inner_left))
                    .filter(|(a, b)| b > a)
                    .collect()
            } else {
                vec![(frame_inner_left, frame_inner_right)]
            };
            // Then subtract each intruding wrap shape's x-extent
            // within the line's vertical band. For upright AABBs the
            // extent is the AABB's left/right; for rotated
            // parallelograms the extent is the actual polygon span at
            // this y, which is much narrower than the AABB at the
            // rotated rect's vertical extremes.
            for shape in wraps {
                let aabb = shape.bounds;
                if aabb.bottom <= line_top || aabb.top >= line_bottom {
                    continue;
                }
                let Some((wl, wr)) = shape.x_extent_in_band(line_top, line_bottom) else {
                    continue;
                };
                if wl <= frame_inner_left && wr >= frame_inner_right {
                    continue;
                }
                let mut next: Vec<(f32, f32)> = Vec::with_capacity(segments.len() + 1);
                for (a, b) in &segments {
                    if wr <= *a || wl >= *b {
                        next.push((*a, *b));
                        continue;
                    }
                    if wl > *a {
                        next.push((*a, wl));
                    }
                    if wr < *b {
                        next.push((wr, *b));
                    }
                }
                segments = next;
            }
            // Drop segments narrower than the per-line floor (a band too
            // thin to hold even one word). The widest sub-floor segment
            // is kept aside as a shape-conforming fallback for narrow
            // tips (a circle's poles, a triangle's apex), so glyphs
            // there hug the outline instead of escaping to the full AABB
            // width.
            const MIN_USABLE_64: i32 = 1536; // 24 pt × 64
            let widest_raw = segments.iter().copied().max_by(|x, y| {
                (x.1 - x.0)
                    .partial_cmp(&(y.1 - y.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let usable: Vec<(f32, f32)> = segments
                .into_iter()
                .filter(|(a, b)| {
                    let w64 = ((b - a) * paged_text::shape::ADVANCE_PRECISION).round() as i32;
                    w64 >= MIN_USABLE_64
                })
                .collect();
            if usable.is_empty() {
                // No segment meets the per-line floor at this band.
                //
                // For a SHAPED (wrap-inside) frame, a narrow band still
                // lies inside the outline near a tip — fall back to the
                // widest sub-floor segment (positioned at its real x and
                // floored to MIN_USABLE so the breaker can still seat a
                // word) so the line tracks the shape's centre-line
                // rather than spilling to the AABB. Glyphs that overrun
                // the thin segment are clipped by `apply_polygon_clip`,
                // but they stay centred on the outline's axis.
                //
                // For a wrap-AROUND-objects frame with no shape (and for
                // the degenerate "no segment at all" case), keep the
                // legacy `scalar_width_64` fallback: emitting a 1pt
                // sentinel would make `paragraph_breaker::total_fit`
                // read the slot as "ratio < -1" and prune every active
                // node crossing it, so a paragraph needing more rows
                // than fit before the apex would return zero breaks and
                // the whole story would vanish.
                match (shape.is_some(), widest_raw) {
                    (true, Some((a, b))) if b > a => {
                        // Use the thin segment's ACTUAL width, seated at
                        // its real x. A line here stays as narrow as the
                        // outline allows (so a circle's pole line is
                        // short, not full-width) and centred on the
                        // chord; any glyph that overruns the thin slot is
                        // trimmed by `apply_polygon_clip`. Width is
                        // floored to 1 unit so the breaker still sees a
                        // positive slot.
                        let w64 = (((b - a) * paged_text::shape::ADVANCE_PRECISION).round() as i32)
                            .max(1);
                        let shift_pt = (a - frame_inner_left).max(0.0);
                        widths_64.push(w64);
                        shifts_64
                            .push((shift_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32);
                        twin_after.push(false);
                        no_room.push(false);
                    }
                    _ => {
                        widths_64.push(scalar_width_64);
                        shifts_64.push(0);
                        twin_after.push(false);
                        // A shaped frame with no chord at this height
                        // has no room for the line; a wrapped
                        // rectangle fully covered by its wraps keeps
                        // the full-width fallback it always had.
                        no_room.push(shape.is_some());
                    }
                }
                continue;
            }
            // Emit one composer line per usable segment. The first
            // segment owns the actual baseline; the rest are twin
            // partners that the post-layout pass collapses onto the
            // first's row. Sort by x so the leftmost segment comes
            // first — keeps reading order intact.
            let mut usable_sorted = usable;
            usable_sorted
                .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (idx, (a, b)) in usable_sorted.iter().enumerate() {
                let w64 = ((b - a) * paged_text::shape::ADVANCE_PRECISION).round() as i32;
                let shift_pt = (a - frame_inner_left).max(0.0);
                widths_64.push(w64);
                shifts_64.push((shift_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32);
                // Mark every segment after the first as a twin so
                // the emit pass collapses it onto the first
                // segment's row at the same baseline.
                twin_after.push(idx > 0);
                no_room.push(false);
            }
        }
        if last_row_fits {
            visible_lines = widths_64.len();
        }
        last_row_fits = false;
        // Only the bands past the LAST one with room count as no room:
        // the tip of a shape (bands before the first usable one) keeps
        // the full-width fallback it always had, because the composer
        // cannot yet skip a slot and start lower. A frame with no usable
        // band at all keeps every fallback too.
        let tail = &mut no_room[frame_first_line..];
        match tail.iter().rposition(|n| !*n) {
            Some(last_usable) => {
                for n in &mut tail[..last_usable] {
                    *n = false;
                }
            }
            None => {
                for n in tail.iter_mut() {
                    *n = false;
                }
            }
        }
    }
    if widths_64.is_empty() {
        return WrapPlan {
            line_x_shifts_64: Vec::new(),
            twin_after: Vec::new(),
            no_room: Vec::new(),
            first_baseline_push_pt: 0.0,
        };
    }
    lopts.compose.column_widths = Some(widths_64);
    // A shaped frame's ragged lines break the way InDesign breaks them
    // there: by minimum raggedness (`paged_text` `ragged.rs`).
    if chain_shapes.iter().any(Option::is_some) {
        lopts.compose.minimum_raggedness = true;
        lopts.compose.visible_lines = Some(visible_lines);
        lopts.compose.joined_lines = Some(twin_after.clone());
    }
    WrapPlan {
        line_x_shifts_64: shifts_64,
        twin_after,
        no_room,
        first_baseline_push_pt,
    }
}

/// Map an inner-coord top-left corner through ItemTransform to its
/// spread-coord position. Identity (`None`) is the no-op. Used by
/// the text-emission path so glyphs land where the frame actually
/// sits in spread coords rather than at its inner-coord origin.
pub(super) fn frame_spread_top_left(b: paged_model::Bounds, m: Option<[f32; 6]>) -> (f32, f32) {
    match m {
        Some(m) => apply_matrix(&m, b.left, b.top),
        None => (b.left, b.top),
    }
}

/// Whether items on `layer_ref` should render. Matches the
/// `layer_visible` closure in `build_document`: missing layer (or
/// unknown id) defaults to visible so single-layer IDMLs that omit
/// ItemLayer still emit.
pub(super) fn is_layer_visible(document: &Document, layer_ref: Option<&str>) -> bool {
    // Route through the scene helper so the renderer and the canvas
    // hit-tester agree, including the layer-group ancestor walk (a
    // visible child inside a hidden group resolves hidden).
    paged_scene::layer_render_visible(&document.designmap, layer_ref)
}

/// The box a path-bearing item actually covers: the AABB of its anchors
/// and control handles when it has any, else its stored `bounds`. The
/// stored box is whatever the minting op was handed — a tracer on the
/// annual passed 10 of 94 polygons a two-point box while their paths
/// spanned a page and a half (2026-09-05) — and page routing off that
/// box dropped the gutter-crossing half from the facing page while
/// InDesign, routing off the path, painted it. The path is the truth.
pub(super) fn path_bounds_or(
    bounds: paged_model::Bounds,
    anchors: &[paged_model::PathAnchor],
) -> paged_model::Bounds {
    if anchors.is_empty() {
        return bounds;
    }
    let mut out = paged_model::Bounds {
        top: f32::INFINITY,
        left: f32::INFINITY,
        bottom: f32::NEG_INFINITY,
        right: f32::NEG_INFINITY,
    };
    for a in anchors {
        for (x, y) in [a.anchor, a.left, a.right] {
            out.left = out.left.min(x);
            out.right = out.right.max(x);
            out.top = out.top.min(y);
            out.bottom = out.bottom.max(y);
        }
    }
    if out.left.is_finite() && out.top.is_finite() {
        out
    } else {
        bounds
    }
}

pub(super) fn page_for_frame(frame: &paged_model::Bounds, pages: &[PageGeom]) -> Option<usize> {
    let cx = (frame.left + frame.right) * 0.5;
    let cy = (frame.top + frame.bottom) * 0.5;
    pages.iter().position(|p| {
        let b = p.bounds_in_spread;
        cx >= b.left && cx <= b.right && cy >= b.top && cy <= b.bottom
    })
}

/// Local page indices whose `bounds_in_spread` overlap `frame`.
/// Used by the non-text shape emit loops (Rectangle / Oval /
/// GraphicLine / Polygon) so spread-spanning page backgrounds, hero
/// bands, and bleed-the-gutter decoratives render on every page they
/// cover instead of only on the page that wins the AABB-centroid
/// test. Pages clip raster output to their own dimensions, so emitting
/// the same geometry twice is safe — the page-local rasterizer drops
/// off-page commands.
///
/// `None` is treated as "no overlap"; callers fall back to the legacy
/// `page_for_frame` centroid (or `0`) for backwards compatibility.
pub(super) fn pages_overlapping_frame(
    frame: &paged_model::Bounds,
    pages: &[PageGeom],
) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        let b = p.bounds_in_spread;
        if frame.right > b.left
            && frame.left < b.right
            && frame.bottom > b.top
            && frame.top < b.bottom
        {
            out.push(i);
        }
    }
    out
}

/// How far a stroke of `weight` on a text frame pushes the frame's text
/// in, on every side, for its `StrokeAlignment`.
///
/// Measured against InDesign 20.0.1 (the `stroke-inset` fixture,
/// 2026-10-01; every line read off its DOM): a CENTRE stroke insets the
/// text by half its weight, an INSIDE stroke by all of it, an OUTSIDE
/// stroke not at all — on all four sides, so the first baseline moves
/// down and the last line has that much less room too.
pub(crate) fn stroke_inset_share(alignment: Option<&str>, weight: f32) -> f32 {
    match alignment {
        Some("InsideAlignment") => weight,
        Some("OutsideAlignment") => 0.0,
        _ => weight * 0.5,
    }
}

/// The stroke's share of a text frame's text inset — zero unless the
/// stroke is one the frame actually paints. A weight with no colour
/// (`Swatch/None`) moves nothing; InDesign's default text frame has
/// exactly that, a 1 pt stroke in None.
///
/// Resolved the way `emit_text_frame_into` resolves the stroke it
/// paints (the frame, then its object style), so the text moves exactly
/// when a stroke is drawn, by the share its `StrokeAlignment` gives it
/// (the same alignment `emit_text_frame_into` strokes the outline by).
pub(crate) fn stroke_text_inset(frame: &TextFrame, document: &Document) -> f32 {
    let mut resolved = ResolvedFrame::from_text_frame(frame);
    let style = crate::module::resolve_applied_style(&resolved, document);
    if let Some(s) = &style {
        crate::module::object_style_cascade(&mut resolved, s);
    }
    let weight = resolved.effective_stroke_weight();
    if !frame_stroke_is_visible(resolved.stroke_color, weight) {
        return 0.0;
    }
    stroke_inset_share(resolved.stroke_alignment, weight)
}

/// The frame a story composes into: `frame` with its stroke's share
/// folded into `InsetSpacing`, so every consumer of the text area —
/// the column geometry, the first baseline, the bottom-fit check, a
/// shaped frame's eroded outline, the auto-size fit — sees the one
/// inset InDesign uses. Borrowed unchanged when the stroke moves
/// nothing.
///
/// Fold once, where a frame ENTERS text layout (the story chain, a
/// master frame's copy, the auto-size fit): a frame this returns still
/// carries its stroke, so folding it again would inset twice.
///
/// A shaped frame's outline is eroded by the sum, as InDesign does
/// (`stroke-inset`'s chamfered frames, `shaped-bands`' stroked circle,
/// triangle, donuts and notch, and `text-in-shape`'s donut, whose bands
/// InDesign sets at exactly [142, 235] / [359, 453] — the 4 pt inset
/// plus half the 0.75 pt stroke).
pub(crate) fn text_area_frame<'f>(
    frame: &'f TextFrame,
    document: &Document,
) -> std::borrow::Cow<'f, TextFrame> {
    let share = stroke_text_inset(frame, document);
    if share <= 0.0 {
        return std::borrow::Cow::Borrowed(frame);
    }
    let mut folded = frame.clone();
    folded.inset_spacing = Some(frame.inset_spacing.unwrap_or([0.0; 4]).map(|v| v + share));
    std::borrow::Cow::Owned(folded)
}

pub(super) fn emit_text_frame_into(
    page: &mut BuiltPage,
    frame: &TextFrame,
    document: &Document,
    palette: &Graphic,
    fallback: Paint,
    color_ctx: ColorCtx<'_>,
    drop_shadow: Option<DropShadow>,
    auto_sized_bounds: Option<paged_model::Bounds>,
) {
    let mut resolved = ResolvedFrame::from_text_frame(frame);
    // W1.7 Phase B: an AutoSizing frame paints its fill / stroke to the
    // GROWN extent, not the authored undersized box. Substitute the
    // grown rect into the resolved geometry so the box, its effects, and
    // its drop shadow all stretch to where the auto-sized text actually
    // reaches. Only the rectangular text-panel case is grown — a
    // non-rectangular (`Polygon`) text frame keeps its authored outline.
    if let Some(grown) = auto_sized_bounds {
        if let Geometry::TextFrameRect { rect } = &mut resolved.geometry {
            *rect = Rect {
                x: grown.left,
                y: grown.top,
                w: grown.width(),
                h: grown.height(),
            };
        }
    }
    let style = crate::module::resolve_applied_style(&resolved, document);
    if let Some(s) = &style {
        crate::module::object_style_cascade(&mut resolved, s);
    }
    page.stats.frames += 1;
    let outer = frame_outer_transform(page, resolved.item_transform);
    // Bracket fill / stroke / drop-shadow into a transparency group
    // whenever the frame's blend mode is non-Normal or opacity < 100%.
    // The group composite at EndBlendGroup applies the blend mode
    // against the page underneath, which is the structurally correct
    // PDF transparency-group semantic (replaces the per-glyph /
    // per-shape FillPathBlend approximation).
    //
    // Text glyphs land in this same page list during the story pass —
    // they're bracketed separately post-pass via
    // `bracket_text_frame_glyph_ranges` so each text frame's glyphs
    // composite with the same blend mode against the page below.
    let needs_group = frame_needs_blend_group(&resolved);
    let group_bounds = if needs_group {
        let geom_bounds = match &resolved.geometry {
            Geometry::TextFrameRect { rect } | Geometry::Rect { rect } => *rect,
            Geometry::Oval { rect } => *rect,
            Geometry::Polygon { bbox, .. } => *bbox,
            Geometry::Line { p0, p1 } => paged_compose::Rect {
                x: p0.0.min(p1.0),
                y: p0.1.min(p1.1),
                w: (p0.0 - p1.0).abs(),
                h: (p0.1 - p1.1).abs(),
            },
        };
        Some(push_blend_group(
            page,
            geom_bounds,
            outer,
            resolved.blend_mode,
            frame_group_opacity(&resolved),
        ))
    } else {
        None
    };
    crate::module::drop_shadow_module(
        &resolved,
        page,
        palette,
        color_ctx,
        drop_shadow,
        outer,
        frame.stroke_drop_shadow.as_ref(),
    );
    // W1.1: a TextFrame carrying a genuinely non-rectangular path
    // (triangle / pentagon / Bezier / compound outline) had its
    // geometry lifted to `Geometry::Polygon` by `from_text_frame`.
    // Intern that path up-front so the frame's own fill / stroke /
    // effects paint the real outline rather than the AABB — mirroring
    // `emit_polygon_into`. Plain rectangular text panels keep the
    // unit-rect path (`fill_path = None`) and the rect emitter. Text
    // *layout* clipping is handled separately off `frame.anchors`.
    //
    // C-18: a text frame carries the corner vocabulary too, and unlike
    // an oval / line / group its body is a REAL outline, so the effect
    // renders. The rectangular panel goes through the same
    // `corner_path_module` a `<Rectangle>` uses (all four names address
    // the box's corners); the pathed one folds the uniform corner into
    // the anchor path exactly as `emit_polygon_into` does. Both leave
    // `fill_path = None` when no corner effect resolves, so an ordinary
    // text frame keeps the cheap rect primitive it always used.
    let corner = crate::module::corner_path_module(&resolved, page);
    let stroke_weight = resolved.effective_stroke_weight();
    // Where the stroke paints, for its `StrokeAlignment` — the same
    // geometry a `<Rectangle>` / `<Polygon>` strokes: the outline moved
    // in (Inside) or out (Outside) by half the weight, so the stroke lies
    // wholly inside / outside the frame edge. `None` when the stroke is
    // centred (the outline itself is stroked), and for a corner-effected
    // panel, whose corner module already baked the alignment into
    // `corner.stroke`.
    let mut aligned_stroke = None;
    let fill_path = if let Geometry::Polygon {
        anchors,
        subpath_starts,
        subpath_open,
        ..
    } = &resolved.geometry
    {
        let path = super::text_path::polygon_outline_path(
            anchors,
            subpath_starts,
            subpath_open,
            super::shapes::uniform_corner(
                resolved.corner_radius,
                resolved.corner_option,
                &resolved.corners,
            ),
        );
        let cache_key = match resolved.self_id {
            Some(id) => fnv_1a_u64(id.as_bytes()),
            None => path_signature(anchors),
        };
        aligned_stroke =
            super::shapes::aligned_outline_path(&path, resolved.stroke_alignment, stroke_weight)
                .map(|p| page.list.paths.intern(cache_key ^ 0xA11A_0000, p).0);
        let (id, _) = page.list.paths.intern(cache_key, path);
        Some(id)
    } else {
        corner.fill
    };
    if let (Geometry::TextFrameRect { rect }, None) = (&resolved.geometry, corner.stroke) {
        let offset = stroke_alignment_offset(resolved.stroke_alignment, stroke_weight);
        if offset != 0.0 {
            let inset = super::shapes::inset_rect(*rect, offset);
            let seed = resolved
                .self_id
                .map(|id| fnv_1a_u64(id.as_bytes()))
                .unwrap_or_else(|| fnv_1a_u64(format!("{rect:?}").as_bytes()));
            let key = seed ^ 0xA11A_0000 ^ u64::from(offset.to_bits());
            let (id, _) = page
                .list
                .paths
                .intern(key, super::shapes::axis_rect_path(inset));
            aligned_stroke = Some(id);
        }
    }
    // Q-04: extended GradientFeather (and the rest of FrameEffects) to
    // TextFrame. For the rectangular panel we route through the unit-
    // rect path the same way `emit_rectangle_into` does (intern the
    // unit rect, scale via `Transform::for_rect_in`, flag
    // `effects_unit_normalize` so the effects module converts path-
    // local coords from unit space). For a pathed text frame the
    // interned polygon path is already in inner-anchor coords under
    // `outer`, so effects ride it directly with no unit normalisation
    // (mirrors `emit_polygon_into`).
    // C-18: a CORNER-effected rectangular panel takes the same third
    // route `emit_rectangle_into` does — the corner path is already in
    // inner coords, so effects ride it under plain `outer` with no unit
    // normalisation. Without it the halo/feather would trace the square
    // box while the fill traced the rounded one.
    let (effects_path, effects_xform, effects_unit_normalize) = if frame.effects.is_some() {
        match (&resolved.geometry, fill_path) {
            (Geometry::TextFrameRect { .. }, Some(pid)) => (Some(pid), outer, None),
            (Geometry::TextFrameRect { rect: r }, None) => {
                let (id, _) = page.list.paths.intern(
                    paged_compose::UNIT_RECT_KEY,
                    paged_compose::PathData {
                        segments: vec![
                            paged_compose::PathSegment::MoveTo { x: 0.0, y: 0.0 },
                            paged_compose::PathSegment::LineTo { x: 1.0, y: 0.0 },
                            paged_compose::PathSegment::LineTo { x: 1.0, y: 1.0 },
                            paged_compose::PathSegment::LineTo { x: 0.0, y: 1.0 },
                            paged_compose::PathSegment::Close,
                        ],
                    },
                );
                (Some(id), Transform::for_rect_in(*r, outer), Some(*r))
            }
            (Geometry::Polygon { .. }, Some(pid)) => (Some(pid), outer, None),
            _ => (None, outer, None),
        }
    } else {
        (None, outer, None)
    };
    if let (Some(path_id), Some(effects)) = (effects_path, frame.effects.as_ref()) {
        crate::module::emit_effects_pre_fill(
            page,
            effects,
            path_id,
            effects_xform,
            palette,
            color_ctx,
        );
    }
    crate::module::fill_paint_module(
        &resolved, page, palette, color_ctx, fallback, outer, fill_path,
    );
    if let (Some(path_id), Some(effects)) = (effects_path, frame.effects.as_ref()) {
        crate::module::emit_effects_post_fill(
            page,
            effects,
            path_id,
            effects_xform,
            palette,
            color_ctx,
            effects_unit_normalize,
        );
    }
    crate::module::stroke_paint_module(
        &resolved,
        page,
        palette,
        color_ctx,
        outer,
        // C-18: the corner module returns a SEPARATE stroke path with
        // `StrokeAlignment` baked in, as for `emit_rectangle_into`; an
        // aligned flat panel or pathed frame strokes its moved outline;
        // `.or(fill_path)` is the centred pathed case.
        corner.stroke.or(aligned_stroke).or(fill_path),
        stroke_for(
            resolved.stroke_type,
            stroke_weight,
            resolved.end_cap,
            resolved.end_join,
            resolved.miter_limit,
            Some(&document.styles.stroke_styles),
            resolved.stroke_dash,
        ),
    );
    if needs_group {
        pop_blend_group(page);
    }
    let _ = group_bounds;
}

/// First-baseline y (1/64 pt) for the head frame of a story,
/// honouring `<TextFramePreference FirstBaselineOffset>` and the
/// top inset. `default_64` is the renderer's heuristic baseline
/// (LayoutOptions::new gives `point_size * 0.8 * 64`) used for
/// `AscentOffset` (the IDML default) and any unrecognised value.
/// `metrics` carries the head font's OS/2 / hhea metrics; when
/// present, `CapHeight` and `XHeight` policies use the font's
/// real values instead of a 70% / 50% heuristic.
/// How far below the top of a text area its first baseline sits, for
/// one `FirstBaselineOffset` policy.
///
/// Shared by frames and TABLE CELLS: InDesign applies the same rule in
/// both (measured 2026-09-06 — a top-aligned cell's first line sits at
/// `row_top + top_inset + ascender`, and `FixedHeight` with a minimum
/// of 10 pt puts it at exactly 10). Cells had their own `0.8 × pt`
/// heuristic long after frames adopted the real ascender.
pub(super) fn first_baseline_offset_64(
    policy: Option<paged_model::FirstBaselineOffset>,
    minimum_offset_pt: Option<f32>,
    point_size: f32,
    default_64: i32,
    metrics: Option<&FontMetrics>,
    leading_64: Option<i32>,
) -> i32 {
    const CAP_HEIGHT_FALLBACK: f32 = 0.70;
    const X_HEIGHT_FALLBACK: f32 = 0.50;
    let pt_to_64 = |pt: f32| (pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;
    let em_fraction_to_64 = |frac: f32| pt_to_64(point_size * frac);
    use paged_model::FirstBaselineOffset as F;
    match policy {
        Some(F::CapHeight) => em_fraction_to_64(
            metrics
                .and_then(|m| m.cap_height)
                .unwrap_or(CAP_HEIGHT_FALLBACK),
        ),
        Some(F::XHeight) => em_fraction_to_64(
            metrics
                .and_then(|m| m.x_height)
                .unwrap_or(X_HEIGHT_FALLBACK),
        ),
        Some(F::EmBoxHeight) => pt_to_64(point_size),
        // LeadingOffset: the first line's LEADING, never less than
        // MinimumFirstBaselineOffset. Measured against InDesign 2025 (the
        // `reflow` fixture, 2026-10-01): 12 pt leading puts the first
        // baseline 12 pt below the frame top. It used to take the minimum
        // or the caller's 0.8 x point-size default (8 pt at 10 pt), so every
        // LeadingOffset frame sat 4 pt high. Callers without the leading
        // (`None`) keep the old fallback.
        Some(F::LeadingOffset) => {
            let min_64 = minimum_offset_pt.map(pt_to_64);
            match (leading_64, min_64) {
                (Some(l), Some(m)) => l.max(m),
                (Some(l), None) => l,
                (None, Some(m)) => m,
                (None, None) => default_64,
            }
        }
        // FixedHeight uses MinimumFirstBaselineOffset verbatim. Falls back
        // to default when missing.
        Some(F::FixedHeight) => minimum_offset_pt.map(pt_to_64).unwrap_or(default_64),
        // AscentOffset (IDML default) and `None` (unrecognised /
        // absent attribute): use the font's ascender if available;
        // otherwise fall through to the LayoutOptions heuristic.
        Some(F::AscentOffset) | None => metrics
            .map(|m| em_fraction_to_64(m.ascender))
            .unwrap_or(default_64),
    }
}

pub(super) fn first_baseline_for_frame(
    frame: &TextFrame,
    point_size: f32,
    default_64: i32,
    metrics: Option<&FontMetrics>,
    leading_64: Option<i32>,
) -> i32 {
    const CAP_HEIGHT_FALLBACK: f32 = 0.70;
    let top_inset_64 = frame
        .inset_spacing
        .map(|i| (i[0] * paged_text::shape::ADVANCE_PRECISION).round() as i32)
        .unwrap_or(0);
    let pt_to_64 = |pt: f32| (pt * paged_text::shape::ADVANCE_PRECISION).round() as i32;
    let em_fraction_to_64 = |frac: f32| pt_to_64(point_size * frac);
    let policy_offset_64 = first_baseline_offset_64(
        frame.first_baseline_offset,
        frame.minimum_first_baseline_offset,
        point_size,
        default_64,
        metrics,
        leading_64,
    );
    // Display-headline clamp: when the frame is sized to the visual
    // letterform (cap height) rather than the typo ascent — common
    // on Envato cover-style templates where designers tight-fit
    // 60-100pt headlines into ~half-em-tall boxes — `AscentOffset`'s
    // baseline lands past the frame bottom and the renderer drops
    // the line. InDesign keeps the text by treating the baseline as
    // if cap-height were the ascent. Mirror that: if the resolved
    // offset would exceed the frame's inner height, fall back to a
    // cap-height-based offset (which fits inside any box at least
    // ~0.7×pt tall, matching real-world headline frame sizing).
    let bottom_inset_64 = frame
        .inset_spacing
        .map(|i| (i[2] * paged_text::shape::ADVANCE_PRECISION).round() as i32)
        .unwrap_or(0);
    let inner_height_64 = pt_to_64(frame.bounds.height()) - top_inset_64 - bottom_inset_64;
    let baseline_offset_in_frame = top_inset_64 + policy_offset_64;
    if inner_height_64 > 0 && baseline_offset_in_frame > top_inset_64 + inner_height_64 {
        let cap_height = metrics
            .and_then(|m| m.cap_height)
            .unwrap_or(CAP_HEIGHT_FALLBACK);
        let clamped = em_fraction_to_64(cap_height);
        return top_inset_64 + clamped.min(inner_height_64);
    }
    baseline_offset_in_frame
}

/// Build the outer affine that maps a frame's local-space rect into
/// page-space pixels: page-origin offset composed with the frame's
/// `ItemTransform` (identity when absent). Identity ItemTransform is
/// the common case — the result collapses to a pure translation, so
/// the rasterizer's axis-aligned fast paths still apply.
/// Post-multiply `xf` by a rotation/scale `linear` (2×2: a b c d in
/// row-major IDML convention) pivoted around the page-space point
/// `(pivot_x, pivot_y)`. Mathematically:
///   xf := T(pivot) · L · T(-pivot) · xf
/// Used by the text-emission path so glyph commands inside a
/// rotated/sheared TextFrame inherit the frame's ItemTransform
/// rotation around the frame's top-left.
/// Phase 7 — vertical writing post-rotation. Walks the per-page
/// command ranges this story emitted and rotates each command 90°
/// clockwise around its host frame's top-left, then translates +x
/// by the frame's width. The result: horizontal content layouts
/// flip into CJK vertical convention — columns advance right-to-
/// left and characters within a column read top-to-bottom.
///
/// `pre_counts[i]` is the number of commands on `pages[i]` before
/// this story's emit; commands at index ≥ pre_counts[i] are this
/// story's contributions and get rotated. `chain` + `chain_pages`
/// are parallel slices — `chain[i]` is the host frame whose page
/// is `chain_pages[i]`. For each page that hosted at least one
/// chain frame, the FIRST matching chain frame's geometry is used
/// as the rotation pivot (typical CJK doesn't thread vertical
/// stories across pages anyway).
///
/// Limitations:
/// - Latin glyphs end up sideways. Upright Latin in CJK vertical
///   would require per-glyph counter-rotation around each glyph's
///   centre (`<RotateSingleByteCharacters>` IDML attribute).
/// - Rotated content overflows the frame's geometric bounds when
///   the original layout was wider than the frame is tall (the
///   common case for tall frames flipped from wide layouts).
/// - Frame-inset axes don't swap (a 12pt TextTopInset stays in y,
///   not x). The right fix moves to a layout-time axis swap.
pub(super) fn apply_vertical_writing_rotation(
    pages: &mut [BuiltPage],
    pre_counts: &[usize],
    chain: &[&paged_model::TextFrame],
    chain_pages: &[usize],
) {
    use std::collections::BTreeMap;
    // For each page that hosted this story, look up the first
    // chain frame on that page. We pivot around that frame's
    // top-left and translate by the frame's width.
    let mut frame_for_page: BTreeMap<usize, &paged_model::TextFrame> = BTreeMap::new();
    for (i, &page_idx) in chain_pages.iter().enumerate() {
        frame_for_page.entry(page_idx).or_insert(chain[i]);
    }
    // 90° CW rotation in screen coords (Y down): cos=0, sin=1.
    // Matrix linear part [a, b, c, d] = [0, 1, -1, 0].
    let linear = [0.0_f32, 1.0, -1.0, 0.0];
    for (page_idx, frame) in frame_for_page {
        if page_idx >= pages.len() {
            continue;
        }
        let pre = pre_counts.get(page_idx).copied().unwrap_or(0);
        let total = pages[page_idx].list.commands.len();
        if pre >= total {
            continue;
        }
        let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
        let (ox, oy) = pages[page_idx].spread_origin;
        let pivot_x = sx - ox;
        let pivot_y = sy - oy;
        let frame_w = frame.bounds.width();
        for cmd in &mut pages[page_idx].list.commands[pre..total] {
            let xf = cmd.transform_mut();
            rotate_transform_around(xf, linear, pivot_x, pivot_y);
            // After rotation around the frame's top-left, rotated
            // content lives in x ∈ [pivot_x - h, pivot_x], y ∈
            // [pivot_y, pivot_y + w]. Shift +frame_w on x to bring
            // it into the right half of the frame.
            xf.0[4] += frame_w;
        }
    }
}

pub(super) fn rotate_transform_around(
    xf: &mut Transform,
    linear: [f32; 4],
    pivot_x: f32,
    pivot_y: f32,
) {
    let [a, b, c, d] = linear;
    // The pivoted rotation is:
    //   M = [a c (pivot_x - a*pivot_x - c*pivot_y);
    //        b d (pivot_y - b*pivot_x - d*pivot_y);
    //        0 0 1]
    // Compose as M · xf.
    let [xa, xb, xc, xd, xtx, xty] = xf.0;
    let m_tx = pivot_x - a * pivot_x - c * pivot_y;
    let m_ty = pivot_y - b * pivot_x - d * pivot_y;
    let new_a = a * xa + c * xb;
    let new_b = b * xa + d * xb;
    let new_c = a * xc + c * xd;
    let new_d = b * xc + d * xd;
    let new_tx = a * xtx + c * xty + m_tx;
    let new_ty = b * xtx + d * xty + m_ty;
    xf.0 = [new_a, new_b, new_c, new_d, new_tx, new_ty];
}

pub(super) fn frame_outer_transform(
    page: &BuiltPage,
    item_transform: Option<[f32; 6]>,
) -> Transform {
    let (ox, oy) = page.spread_origin;
    let page_origin = Transform::translate(-ox, -oy);
    // W1.9 — the spread-level `<Spread ItemTransform>` rotation/scale
    // (`spread_transform`, linear part only; translation already cancels
    // against `spread_origin`) is applied ABOUT the page origin: first
    // re-origin the frame into page-local space, then rotate/scale the
    // whole page in place. When the spread carries no rotation/scale
    // `spread_transform` is `IDENTITY` and this is exactly the historical
    // `translate(-spread_origin) ∘ item_transform`. The canvas hit-tester
    // inverts the same `spread_transform`, so selection can't disagree
    // with paint.
    let local = match item_transform {
        Some(m) => page_origin.compose(&Transform(m)),
        None => page_origin,
    };
    if page.spread_transform == Transform::IDENTITY {
        local
    } else {
        page.spread_transform.compose(&local)
    }
}

/// C-1 — splice a plugin [`paged_compose::SceneLayer`] into a frame, right
/// after the frame's own content. Looks the frame's `Self` id up in the
/// registry; on a hit, builds the content-origin → page transform (the
/// frame's `frame_outer_transform` composed with the content-box offset)
/// and lowers the layer clipped to the content box. A no-op when no
/// registry is wired, the frame has no id, or no layer is registered —
/// so the no-plugin render path is untouched. `inset` is the text-frame
/// content inset `[top,left,bottom,right]`; pass `None` for shapes (whose
/// content box is the bounds).
pub(super) fn emit_frame_scene_layer(
    page: &mut BuiltPage,
    self_id: Option<&str>,
    bounds: paged_model::Bounds,
    inset: Option<[f32; 4]>,
    item_transform: Option<[f32; 6]>,
    registry: Option<&std::collections::HashMap<String, paged_compose::SceneLayer>>,
    font_bytes: Option<&[u8]>,
) {
    let Some(registry) = registry else { return };
    let Some(id) = self_id else { return };
    let Some(layer) = registry.get(id) else {
        return;
    };
    if layer.items.is_empty() {
        return;
    }
    let outer = frame_outer_transform(page, item_transform);
    let ins = inset.unwrap_or([0.0; 4]);
    let content_left = bounds.left + ins[1];
    let content_top = bounds.top + ins[0];
    let content_w = (bounds.right - bounds.left - ins[1] - ins[3]).max(0.0);
    let content_h = (bounds.bottom - bounds.top - ins[0] - ins[2]).max(0.0);
    let content_outer = outer.compose(&Transform::translate(content_left, content_top));

    // C-1.1 — the default-font shaping face + outliner for `SceneItem::Text`,
    // built once per layer. `None` when the build has no font (text items
    // are then skipped, like the renderer's own no-font text path). v1
    // renders every text run in this default face (the run's `family`/
    // `style` hints are reserved for per-run selection).
    let text_faces = font_bytes.and_then(|b| {
        let rb = paged_text::Face::from_slice(b, 0)?;
        let ttf = ttf_parser::Face::parse(b, 0).ok()?;
        Some((rb, ttf))
    });
    let text_outliner = text_faces.as_ref().map(|(_, ttf)| TtfOutliner::new(ttf));

    paged_compose::emit_scene_layer(
        &mut page.list,
        layer,
        content_outer,
        (content_w, content_h),
        |list, t, xf| {
            // Lower a text run: shape with the default face, position glyphs
            // at the transformed baseline (`xf.apply(x, y)`), and emit glyph
            // FillPaths through the standard glyph slice (upright in page
            // space — full per-glyph affine for rotated frames is a
            // follow-on, §8.5).
            let (Some((rb, _)), Some(outliner)) = (text_faces.as_ref(), text_outliner.as_ref())
            else {
                return;
            };
            let shaped = paged_text::shape_run(rb, &t.text, t.size);
            if shaped.glyphs.is_empty() {
                return;
            }
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
                    font_id: u32::MAX,
                    point_size: t.size,
                    underline: false,
                    strikethru: false,
                    substituted: false,
                    x_scale: 1.0,
                    y_scale: 1.0,
                    skew_deg: 0.0,
                    ch: None,
                });
                cursor = cursor.saturating_add(g.x_advance);
            }
            let origin = xf.apply(t.x, t.y);
            let paint = Paint::Solid(paged_compose::scene_paint_to_color(t.paint));
            emit_glyph_slice(
                &positioned,
                u32::MAX,
                t.size,
                |_| paint,
                origin,
                outliner,
                list,
            );
        },
    );
}

/// C-6 (I-06) — assemble a claimed image resource's pyramid tiles into a
/// frame, mirroring [`emit_frame_scene_layer`]'s seam. Looks the frame's
/// `Self` id up in the provider registry; on a hit, picks the mip level
/// matching `render_scale`, assembles the cached tiles into the frame's
/// content box (the same content-origin → page transform the scene layer
/// uses), and records any tiles the provider lacked at that level onto
/// `page.resource_tiles_needed` for the host to fill asynchronously.
/// Compose never blocks: cold tiles simply don't paint (the native
/// whole-image lane already drew first paint). A no-op when no registry is
/// wired, the frame has no id, or no provider claimed it — so the
/// no-plugin path is untouched. `inset`/`item_transform` carry the same
/// meaning as in [`emit_frame_scene_layer`].
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_frame_resource_tiles(
    page: &mut BuiltPage,
    self_id: Option<&str>,
    bounds: paged_model::Bounds,
    inset: Option<[f32; 4]>,
    item_transform: Option<[f32; 6]>,
    registry: Option<
        &std::collections::HashMap<String, crate::pipeline::ResourceProviderEntry<'_>>,
    >,
    render_scale: f32,
) {
    let Some(registry) = registry else { return };
    let Some(id) = self_id else { return };
    let Some(entry) = registry.get(id) else {
        return;
    };
    let outer = frame_outer_transform(page, item_transform);
    let ins = inset.unwrap_or([0.0; 4]);
    let content_left = bounds.left + ins[1];
    let content_top = bounds.top + ins[0];
    let content_w = (bounds.right - bounds.left - ins[1] - ins[3]).max(0.0);
    let content_h = (bounds.bottom - bounds.top - ins[0] - ins[2]).max(0.0);
    if content_w <= 0.0 || content_h <= 0.0 {
        return;
    }
    let content_outer = outer.compose(&Transform::translate(content_left, content_top));

    let level =
        crate::resource_provider::mip_level_for_scale(render_scale, entry.pyramid.max_level());
    let missing = crate::resource_provider::assemble_resource_tiles(
        &mut page.list,
        entry.provider,
        entry.image_id,
        &entry.pyramid,
        level,
        content_outer,
        (content_w, content_h),
    );
    if !missing.is_empty() {
        page.resource_tiles_needed
            .push(crate::resource_provider::ResourceTilesNeeded {
                image_id: entry.image_id.to_string(),
                level,
                tiles: missing,
                generation: entry.provider.revision(entry.image_id),
            });
    }
}

/// Axis-aligned bounding box of `rect` after `outer` is applied to its
/// four corners. The corners may rotate / shear under non-uniform
/// transforms, so we union all four projections rather than just the
/// top-left + bottom-right.
pub(super) fn rect_bounds_in_page(
    rect: paged_compose::Rect,
    outer: Transform,
) -> paged_compose::Rect {
    let pts = [
        outer.apply(rect.x, rect.y),
        outer.apply(rect.x + rect.w, rect.y),
        outer.apply(rect.x + rect.w, rect.y + rect.h),
        outer.apply(rect.x, rect.y + rect.h),
    ];
    let mut minx = pts[0].0;
    let mut miny = pts[0].1;
    let mut maxx = minx;
    let mut maxy = miny;
    for &(x, y) in &pts[1..] {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    paged_compose::Rect {
        x: minx,
        y: miny,
        w: (maxx - minx).max(0.0),
        h: (maxy - miny).max(0.0),
    }
}

/// Pull the inner `Color` out of a solid (or CMYK) paint, returning
/// `None` for gradient paints. Used wherever a context can only
/// consume a flat colour (drop shadow, per-glyph paint).
///
/// `Paint::Cmyk` flattens through the supplied ICC transform (or via
/// the naive CMYK→RGB fallback when no transform is available), so
/// drop-shadow / gradient-stop / decoration paths that have only ever
/// understood RGB keep producing identical pixels to the pre-Stage A
/// world.
pub(super) fn paint_as_solid_with_icc(p: Paint, color_ctx: ColorCtx<'_>) -> Option<Color> {
    match p {
        Paint::Solid(c) => Some(c),
        // The CMYK paint carries the ICC-resolved display RGB cached
        // on it — drop-shadow / gradient-stop paths use that directly
        // so the colour matches what a direct `Paint::Solid` resolved
        // to before Stage A landed. `color_ctx` is unused here but
        // kept in the signature for callers that don't know if the
        // paint is a CMYK paint and want a stable API.
        Paint::Cmyk { rgb, .. } => {
            let _ = color_ctx;
            Some(rgb)
        }
        _ => None,
    }
}
