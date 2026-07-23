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

//! W1.8 — every IDML corner option emits real corner geometry end to
//! end. Renders the generated `corners` sample (one page per option:
//! Rounded, Inverse Rounded, Bevel, Inset, Fancy) and asserts each page
//! interns a fill path whose contour is the calibrated corner shape
//! (not the plain four-corner axis-aligned rect).

use paged_compose::{DisplayCommand, PathSegment};

fn build_corners() -> paged_renderer::pipeline::BuiltDocument {
    let sample = paged_gen::samples::corners::build();
    let bytes = paged_gen::write_idml(&sample).expect("write corners idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("open corners idml");
    let options = paged_renderer::pipeline::PipelineOptions::default();
    paged_renderer::pipeline::build_document(&doc, &options).expect("build corners")
}

/// Collect the segment kinds of every `FillPath` path on the page.
fn fill_path_segments(page: &paged_renderer::pipeline::BuiltPage) -> Vec<&[PathSegment]> {
    page.list
        .commands
        .iter()
        .filter_map(|cmd| match cmd {
            DisplayCommand::FillPath { path_id, .. } => {
                page.list.paths.get(*path_id).map(|p| p.segments.as_slice())
            }
            _ => None,
        })
        .collect()
}

fn count_cubics(segs: &[PathSegment]) -> usize {
    segs.iter()
        .filter(|s| matches!(s, PathSegment::CubicTo { .. }))
        .count()
}

fn count_lines(segs: &[PathSegment]) -> usize {
    segs.iter()
        .filter(|s| matches!(s, PathSegment::LineTo { .. }))
        .count()
}

#[test]
fn all_five_corner_options_emit_geometry() {
    let built = build_corners();
    // Page order mirrors the sample's `variants()`:
    //   0 Rounded, 1 Inverse, 2 Bevel, 3 Inset, 4 Fancy.
    assert_eq!(built.pages.len(), 5, "one page per corner option");

    // Helper: does ANY fill path on the page satisfy `pred`?
    let any_fill = |page_idx: usize, pred: &dyn Fn(&[PathSegment]) -> bool| -> bool {
        fill_path_segments(&built.pages[page_idx])
            .iter()
            .any(|s| pred(s))
    };

    // Rounded: 4 convex quarter-arc cubics.
    assert!(
        any_fill(0, &|s| count_cubics(s) == 4),
        "rounded page should emit a 4-cubic corner fill path"
    );
    // Inverse Rounded: 4 concave quarter-arc cubics.
    assert!(
        any_fill(1, &|s| count_cubics(s) == 4),
        "inverse-rounded page should emit a 4-cubic corner fill path"
    );
    // Bevel: chamfers — no cubics, eight LineTos (one chamfer + one
    // edge per corner).
    assert!(
        any_fill(2, &|s| count_cubics(s) == 0 && count_lines(s) == 8),
        "bevel page should emit a chamfered (line-only) corner fill path"
    );
    // Inset: InDesign's sharp fold-in notch — no cubics, twelve LineTos
    // (two fold-in segments + one edge per corner). Strictly more lines
    // than Bevel, and distinct from Inverse Rounded's smooth arc.
    assert!(
        any_fill(3, &|s| count_cubics(s) == 0 && count_lines(s) == 12),
        "inset page should emit a sharp fold-in (line-only) corner fill path"
    );
    // Fancy: the three-arc ornamental scallop — 12 cubics (3 per
    // corner; the W1.8 calibration, previously 8).
    assert!(
        any_fill(4, &|s| count_cubics(s) == 12),
        "fancy page should emit a 12-cubic ornamental corner fill path"
    );
}
