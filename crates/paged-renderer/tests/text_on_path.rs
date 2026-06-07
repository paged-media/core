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

//! Emission tests for text-on-path rendering.
//!
//! Builds the `paged-gen` `text-on-path.idml` fixture — three pages: an
//! open Bézier arch (Baseline alignment), a circle (CenterPathType),
//! and a short segment that overflows. Asserts that glyphs emit along
//! the host path: positions advance monotonically along the curve,
//! per-glyph rotations track the local tangent, the bracket window is
//! honoured, and the short path fires an `OversetTextDropped`
//! diagnostic.
//!
//! A font resolver (Open Sans, the family the sample declares) is
//! required so shaping produces glyphs at all.

use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{
    diagnostics::DiagnosticCode, pipeline, BytesResolver, Document, PipelineOptions,
};

const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;

fn read_font(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read font fixture {}: {e}", p.display()))
}

/// One emitted glyph fill, reduced to its placement: page-local origin
/// `(tx, ty)` and rotation angle (radians) recovered from the affine's
/// upper-left 2×2. A text-on-path glyph carries
/// `R(angle) · S(scale, -scale)`, so `atan2(b, a)` yields the tangent
/// angle directly (the `-scale` flips the d/c column, not a/b).
#[derive(Debug, Clone, Copy)]
struct GlyphPlacement {
    tx: f32,
    ty: f32,
    angle: f32,
}

/// Pull every glyph fill on a page, in emission order. Glyph fills are
/// the `FillPath`s with a non-trivial 2×2 (rotation or sub-point
/// scale); the host polygon's stroke is a `StrokePath`, not a
/// `FillPath`, so nothing else slips through.
fn glyph_placements(cmds: &[DisplayCommand]) -> Vec<GlyphPlacement> {
    cmds.iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => {
                let [a, b, _c, _d, tx, ty] = transform.0;
                Some(GlyphPlacement {
                    tx,
                    ty,
                    angle: b.atan2(a),
                })
            }
            _ => None,
        })
        .collect()
}

fn build() -> pipeline::BuiltDocument {
    let sample = paged_gen::samples::text_on_path::build();
    let bytes = paged_gen::write_idml(&sample).expect("write_idml");
    let document = Document::open(&bytes).expect("Document::open");

    let mut resolver = BytesResolver::new();
    resolver.add_font("Open Sans", None, read_font("OpenSans.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).expect("build_document")
}

#[test]
fn arch_glyphs_ride_the_path_with_tangent_rotation() {
    let built = build();
    assert_eq!(built.pages.len(), 3, "three single-page spreads");

    // Page 0 — the open arch. Glyphs centre along the arc and rotate
    // to the local tangent.
    let glyphs = glyph_placements(&built.pages[0].list.commands);
    assert!(
        glyphs.len() >= 5,
        "expected several arch glyphs, got {}",
        glyphs.len()
    );

    // The arch climbs left→right over its first half, so successive
    // glyph origins advance in +x (monotonic arc progression). We
    // check the run is non-decreasing across the leading glyphs.
    let lead = &glyphs[..glyphs.len().min(6)];
    for w in lead.windows(2) {
        assert!(
            w[1].tx >= w[0].tx - 0.5,
            "glyph x should advance along the arch: {} then {}",
            w[0].tx,
            w[1].tx
        );
    }

    // Tangent check: the angle each glyph carries should match the
    // direction implied by the step to the *next* glyph (the path
    // tangent we shaped against). Compare a couple of interior glyphs
    // where the chord between neighbours is a good tangent estimate.
    for i in 1..lead.len().saturating_sub(1) {
        let here = lead[i];
        let next = lead[i + 1];
        let dx = next.tx - here.tx;
        let dy = next.ty - here.ty;
        if (dx * dx + dy * dy).sqrt() < 1.0 {
            continue; // too short a chord to estimate direction
        }
        let chord_angle = dy.atan2(dx);
        let mut diff = (here.angle - chord_angle).abs();
        if diff > std::f32::consts::PI {
            diff = std::f32::consts::TAU - diff;
        }
        assert!(
            diff < 0.35, // ~20°: tangent ≈ chord over one advance
            "glyph {i} rotation {:.3} should track the chord tangent {:.3} (diff {:.3})",
            here.angle,
            chord_angle,
            diff
        );
    }

    // The arch is centred near the page centre (polygon ItemTransform
    // translates by (W/2, H/2)); glyph origins land in the page's
    // right-of-centre band, not at the origin.
    let cx = PAGE_W_PT * 0.5;
    let cy = PAGE_H_PT * 0.5;
    assert!(
        glyphs
            .iter()
            .all(|g| g.tx > cx - 50.0 && g.ty > cy && g.ty < cy + 200.0),
        "arch glyphs should sit in the centred path band"
    );
}

#[test]
fn circle_center_alignment_emits_glyphs() {
    let built = build();

    // Page 1 — the circle, CenterPathType. The whole sentence fits
    // the ring (circumference ≈ 2π·140 ≈ 880 pt ≫ the text advance),
    // so every glyph emits and nothing is overset.
    let glyphs = glyph_placements(&built.pages[1].list.commands);
    assert!(
        glyphs.len() >= 10,
        "expected the ring sentence's glyphs, got {}",
        glyphs.len()
    );

    // No overset diagnostic for the circle page (page index 1).
    assert!(
        !built
            .diagnostics
            .items
            .iter()
            .any(|d| d.code == DiagnosticCode::OversetTextDropped && d.page_index == Some(1)),
        "circle page should not be overset"
    );

    // Glyphs ring the centre: their origins span both sides of the
    // page centre x (text wraps over the top of the circle).
    let cx = PAGE_W_PT * 0.5;
    let any_left = glyphs.iter().any(|g| g.tx < cx);
    let any_right = glyphs.iter().any(|g| g.tx > cx);
    assert!(
        any_left && any_right,
        "ring glyphs should straddle the circle centre"
    );
}

#[test]
fn short_path_overflows_and_fires_overset_diagnostic() {
    let built = build();

    // Page 2 — the 80 pt segment with a long sentence. Some glyphs
    // fit; the tail drops and reports overset.
    let glyphs = glyph_placements(&built.pages[2].list.commands);
    assert!(
        !glyphs.is_empty(),
        "the head of the overset text should still draw"
    );

    let overset: Vec<_> = built
        .diagnostics
        .items
        .iter()
        .filter(|d| d.code == DiagnosticCode::OversetTextDropped && d.page_index == Some(2))
        .collect();
    assert_eq!(
        overset.len(),
        1,
        "exactly one text-on-path overset diagnostic on the short-path page"
    );
    // It carries the host story id and is page-tagged via backfill.
    assert!(
        overset[0].story_id.is_some(),
        "overset diagnostic should name the host story"
    );

    // Every drawn glyph stays within the bracket window (≤ 80 pt of
    // arc from the path start). The segment runs along +x from the
    // polygon origin (page centre), so the rightmost drawn glyph must
    // not exceed start_x + 80 + one advance of slack.
    let start_x = PAGE_W_PT * 0.5;
    let max_x = glyphs.iter().map(|g| g.tx).fold(f32::MIN, f32::max);
    assert!(
        max_x <= start_x + 80.0 + 20.0,
        "drawn glyphs must stay inside the 80 pt bracket window (max_x {max_x}, start {start_x})"
    );
}
