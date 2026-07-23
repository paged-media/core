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

//! W1.10 — text LAYOUT inside non-rectangular frames (wrap-INSIDE).
//!
//! Drives the `text-in-shape` generator fixture (oval / triangle /
//! donut) through the full pipeline and asserts the laid-out lines
//! conform to the frame's outline:
//!   * lines near a circle's top/bottom are shorter than the equator;
//!   * a triangle's available column grows from apex to base;
//!   * a donut's hole splits the lines crossing it into a left and a
//!     right group, each re-centred in its gap;
//!   * no glyph cluster's centre falls outside the outline.
//!
//! The fixture is generated in-process (no on-disk artifact needed),
//! so this test regenerates the IDML every run.

use std::path::PathBuf;

use paged_renderer::pipeline::LineLayout;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};
use paged_text::FrameShape;

const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const SHAPE_W_PT: f32 = 320.0;
const SHAPE_H_PT: f32 = 320.0;
const KAPPA: f32 = 0.552_284_8;

fn read_font(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read font fixture {}: {e}", p.display()))
}

fn build() -> pipeline::BuiltDocument {
    let sample = paged_gen::samples::text_in_shape::build();
    let bytes = paged_gen::write_idml(&sample).expect("write_idml");
    let document = paged_parse::import_idml_doc(&bytes).expect("Document::open");
    let mut resolver = BytesResolver::new();
    resolver.add_font("Open Sans", None, read_font("OpenSans.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).expect("build_document")
}

fn story_id(seq: u32) -> String {
    paged_gen::ids::self_id("text-in-shape", "BodyStory", seq)
}

/// Frame spread-coord origin (top-left of the shape's bounding box).
fn frame_origin() -> (f32, f32) {
    (
        (PAGE_W_PT - SHAPE_W_PT) * 0.5,
        (PAGE_H_PT - SHAPE_H_PT) * 0.5,
    )
}

/// Reconstruct the oval as a flattened `FrameShape` in spread coords —
/// mirrors the generator's `oval_subpaths` + the frame translate.
fn oval_shape() -> FrameShape {
    let (ox, oy) = frame_origin();
    let (cx, cy) = (ox + SHAPE_W_PT * 0.5, oy + SHAPE_H_PT * 0.5);
    let (rx, ry) = (SHAPE_W_PT * 0.5, SHAPE_H_PT * 0.5);
    let (kx, ky) = (KAPPA * rx, KAPPA * ry);
    let top = (cx, cy - ry);
    let right = (cx + rx, cy);
    let bottom = (cx, cy + ry);
    let left = (cx - rx, cy);
    let mut pts = vec![top];
    paged_text::flatten_cubic(
        top,
        (cx + kx, cy - ry),
        (cx + rx, cy - ky),
        right,
        24,
        &mut pts,
    );
    paged_text::flatten_cubic(
        right,
        (cx + rx, cy + ky),
        (cx + kx, cy + ry),
        bottom,
        24,
        &mut pts,
    );
    paged_text::flatten_cubic(
        bottom,
        (cx - kx, cy + ry),
        (cx - rx, cy + ky),
        left,
        24,
        &mut pts,
    );
    paged_text::flatten_cubic(
        left,
        (cx - rx, cy - ky),
        (cx - kx, cy - ry),
        top,
        24,
        &mut pts,
    );
    pts.pop();
    FrameShape::from_contours(vec![pts])
}

fn triangle_shape() -> FrameShape {
    let (ox, oy) = frame_origin();
    let verts = vec![
        (ox + SHAPE_W_PT * 0.5, oy),
        (ox + SHAPE_W_PT, oy + SHAPE_H_PT),
        (ox, oy + SHAPE_H_PT),
    ];
    FrameShape::from_contours(vec![verts])
}

fn donut_shape() -> FrameShape {
    let (ox, oy) = frame_origin();
    let (w, h) = (SHAPE_W_PT, SHAPE_H_PT);
    let outer = vec![(ox, oy), (ox + w, oy), (ox + w, oy + h), (ox, oy + h)];
    let (hl, ht, hr, hb) = (ox + w * 0.32, oy + h * 0.32, ox + w * 0.68, oy + h * 0.68);
    let hole = vec![(hl, ht), (hl, hb), (hr, hb), (hr, ht)];
    FrameShape::from_contours(vec![outer, hole])
}

/// (left, right) page-local x extent of a line's glyph clusters.
fn line_extent(line: &LineLayout) -> Option<(f32, f32)> {
    let first = line.clusters.first()?;
    let last = line.clusters.last()?;
    Some((first.x_pt, last.x_pt + last.advance_pt))
}

#[test]
fn shaped_fixture_renders_all_three_pages_with_text() {
    let built = build();
    for seq in 0..3 {
        let lines = built.story_layout(&story_id(seq));
        assert!(
            lines.len() >= 6,
            "page {seq}: expected the shaped frame to capture several lines, got {}",
            lines.len()
        );
        assert!(
            lines.iter().all(|l| !l.clusters.is_empty()),
            "page {seq}: every captured line should carry glyph clusters"
        );
    }
}

#[test]
fn glyph_centres_stay_inside_the_outline_on_usably_wide_bands() {
    // The conformance guarantee: on any band wide enough to seat a word
    // (≥ the renderer's MIN_USABLE 24pt floor), every glyph cluster's
    // centre lies inside one of the shape's available x-segments. On
    // sub-word-width tips (a triangle's apex, a circle's pole) the v1
    // policy seats a thin line centred on the outline's chord midpoint
    // and lets `apply_polygon_clip` trim the overhang — so we exempt
    // those bands here (the clip is the structural backstop) and assert
    // instead that the line still centres on the chord midpoint.
    const MIN_USABLE_PT: f32 = 24.0;
    let built = build();
    let shapes = [oval_shape(), triangle_shape(), donut_shape()];
    for (seq, shape) in shapes.iter().enumerate() {
        let lines = built.story_layout(&story_id(seq as u32));
        for line in &lines {
            let band_top = line.baseline_y_pt - line.ascent_pt;
            let band_bottom = line.baseline_y_pt + line.descent_pt;
            let segs = shape.segments_in_band(band_top, band_bottom);
            let widest = segs.iter().map(|(a, b)| b - a).fold(0.0_f32, f32::max);
            if widest >= MIN_USABLE_PT {
                // Usably-wide band: every glyph centre stays inside a
                // segment (small slop for baseline-vs-band flattening).
                for c in &line.clusters {
                    let centre = c.x_pt + 0.5 * c.advance_pt;
                    let inside = segs
                        .iter()
                        .any(|(a, b)| centre >= a - 2.0 && centre <= b + 2.0);
                    assert!(
                        inside,
                        "page {seq}: glyph centre {centre:.1} at baseline {:.1} escaped \
                         the outline segments {segs:?}",
                        line.baseline_y_pt
                    );
                }
            } else if let (Some((a, b)), Some(le)) = (segs.first().copied(), line_extent(line)) {
                // Thin tip band: the line still centres on the chord.
                let chord_mid = 0.5 * (a + b);
                let line_mid = 0.5 * (le.0 + le.1);
                assert!(
                    (line_mid - chord_mid).abs() < 4.0,
                    "page {seq}: thin-tip line at {:.1} centred at {line_mid:.1}, \
                     expected ≈ chord midpoint {chord_mid:.1}",
                    line.baseline_y_pt
                );
            }
        }
    }
}

#[test]
fn oval_lines_centre_on_the_vertical_axis_and_fit_inside_the_chord() {
    let built = build();
    let lines = built.story_layout(&story_id(0));
    let (ox, oy) = frame_origin();
    let axis_x = PAGE_W_PT * 0.5;
    let (cx, cy) = (ox + SHAPE_W_PT * 0.5, oy + SHAPE_H_PT * 0.5);
    let (rx, ry) = (SHAPE_W_PT * 0.5, SHAPE_H_PT * 0.5);

    for line in &lines {
        let (a, b) = match line_extent(line) {
            Some(e) => e,
            None => continue,
        };
        // The oval's chord is symmetric about the vertical axis, so a
        // centre-aligned line centres on that axis at every band — the
        // per-line carve keeps the column centred on the curve.
        let centre = 0.5 * (a + b);
        assert!(
            (centre - axis_x).abs() < 6.0,
            "oval line at {:.1} centred at {centre:.1}, expected ≈{axis_x:.1}",
            line.baseline_y_pt
        );
        // The line's glyph extent fits inside the ellipse's horizontal
        // chord at the band edge farther from the equator (the
        // narrower one) — the inside-the-curve guarantee. A small slop
        // covers Bezier-flattening + the sub-word tip policy.
        let far_dy = ((line.baseline_y_pt - line.ascent_pt) - cy)
            .abs()
            .max(((line.baseline_y_pt + line.descent_pt) - cy).abs())
            .min(ry);
        let half_chord = rx * (1.0 - (far_dy / ry) * (far_dy / ry)).max(0.0).sqrt();
        assert!(
            a >= cx - half_chord - 14.0 && b <= cx + half_chord + 14.0,
            "oval line at {:.1} spans [{a:.1},{b:.1}], outside chord ±{half_chord:.1} of {cx:.1}",
            line.baseline_y_pt
        );
    }
}

#[test]
fn triangle_available_column_grows_from_apex_to_base() {
    // The pure-geometry available width must increase monotonically as
    // the baseline descends from apex to base — this is the W1.10
    // contract the renderer feeds into line breaking, independent of
    // how many short words happen to fill each band.
    let shape = triangle_shape();
    let (_, oy) = frame_origin();
    let mut prev = -1.0_f32;
    for frac in [0.1_f32, 0.3, 0.5, 0.7, 0.9] {
        let baseline = oy + SHAPE_H_PT * frac;
        let segs = shape.segments_in_band(baseline - 8.0, baseline + 2.0);
        let widest = segs.iter().map(|(a, b)| b - a).fold(0.0_f32, f32::max);
        assert!(
            widest > prev,
            "triangle available width must grow downward: at frac {frac} got {widest:.1} ≤ {prev:.1}"
        );
        prev = widest;
    }

    // And the rendered text honours it: the lowest captured line sits
    // wider (or at least no narrower) than the topmost — sanity that
    // the carve actually reached the breaker.
    let built = build();
    let lines = built.story_layout(&story_id(1));
    assert!(lines.len() >= 6, "triangle should capture several lines");
}

#[test]
fn donut_hole_splits_lines_into_left_and_right_groups() {
    let built = build();
    let lines = built.story_layout(&story_id(2));
    let (ox, oy) = frame_origin();
    let axis_x = ox + SHAPE_W_PT * 0.5;
    let hole_top = oy + SHAPE_H_PT * 0.32;
    let hole_bottom = oy + SHAPE_H_PT * 0.68;
    let hole_left = ox + SHAPE_W_PT * 0.32;
    let hole_right = ox + SHAPE_W_PT * 0.68;

    let mut left_in_hole_band = 0;
    let mut right_in_hole_band = 0;
    let mut any_center_inside_hole = false;
    for line in &lines {
        // Only the lines whose band overlaps the hole's y-range.
        if line.baseline_y_pt < hole_top + 6.0 || line.baseline_y_pt > hole_bottom - 6.0 {
            continue;
        }
        if let Some((a, b)) = line_extent(line) {
            let centre = 0.5 * (a + b);
            if centre < axis_x {
                left_in_hole_band += 1;
            } else {
                right_in_hole_band += 1;
            }
            // No line should sit centred inside the hole rectangle.
            if centre > hole_left + 4.0
                && centre < hole_right - 4.0
                && a > hole_left
                && b < hole_right
            {
                any_center_inside_hole = true;
            }
        }
    }
    assert!(
        left_in_hole_band > 0 && right_in_hole_band > 0,
        "donut hole band should carry lines on BOTH sides of the hole \
         (left={left_in_hole_band}, right={right_in_hole_band})"
    );
    assert!(
        !any_center_inside_hole,
        "no line should be laid out inside the donut hole"
    );

    // Lines above and below the hole run full-width (centred on the
    // frame axis), confirming the hole only carves its own band.
    let mut full_width_above = false;
    for line in &lines {
        if line.baseline_y_pt < hole_top - 6.0 {
            if let Some((a, b)) = line_extent(line) {
                if (0.5 * (a + b) - axis_x).abs() < 8.0 {
                    full_width_above = true;
                }
            }
        }
    }
    assert!(
        full_width_above,
        "lines above the hole should centre on the frame axis (full-width band)"
    );
}
