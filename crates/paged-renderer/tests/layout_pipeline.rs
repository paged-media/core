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

//! W4.10 — display-list assertions over the generated `layout.idml`
//! fixture. Each test isolates one distinctive *rendered* layout effect:
//!
//!   * the rotate-15 spread page: the body rect's emitted fill transform
//!     carries the 15° spread `ItemTransform` (off-diagonal terms);
//!   * the scale-1.25 spread page: the body rect's fill linear block is
//!     1.25× the authored 120×80 rect;
//!   * the CenterPoint autosize page: the centre-grown box's painted top
//!     rises ABOVE the identically-authored TopLeft control box (the
//!     W1.7 "visible box" grow-direction effect).
//!
//! Pages map 1:1 to the sample's variant declaration order (see
//! `paged_gen::samples::layout`).

use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, PipelineOptions};

const PAGE_ROTATE_15: usize = 4;
const PAGE_SCALE_125: usize = 5;
const PAGE_CENTER_GROW: usize = 3;

const DEMO_W_PT: f32 = 120.0;
const DEMO_H_PT: f32 = 80.0;

fn inter_font() -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

fn build() -> pipeline::BuiltDocument {
    let bytes = paged_gen::write_idml(&paged_gen::samples::layout::build()).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("Document::open");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).expect("build_document")
}

/// Every `FillPath` transform on a page, as `[a, b, c, d, tx, ty]`.
fn fill_transforms(page: &pipeline::BuiltPage) -> Vec<[f32; 6]> {
    page.list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => Some(transform.0),
            _ => None,
        })
        .collect()
}

/// The fill whose baked linear block (|a|,|d|) best matches the demo
/// rect dimensions at `scale` — the 120×80 magenta rect. Returns its
/// full transform.
fn demo_rect_fill(page: &pipeline::BuiltPage, scale: f32) -> [f32; 6] {
    let want_w = DEMO_W_PT * scale;
    let want_h = DEMO_H_PT * scale;
    fill_transforms(page)
        .into_iter()
        .find(|m| {
            // The rect rides UNIT_RECT, so the linear block magnitude is
            // (w, h) possibly rotated; compare the singular-value-ish
            // column lengths against the target dims.
            let col0 = (m[0] * m[0] + m[1] * m[1]).sqrt();
            let col1 = (m[2] * m[2] + m[3] * m[3]).sqrt();
            (col0 - want_w).abs() < 1.0 && (col1 - want_h).abs() < 1.0
        })
        .unwrap_or_else(|| panic!("no demo-rect fill ~{want_w}x{want_h} on the page"))
}

#[test]
fn spread_rotation_rotates_body_rect_fill() {
    let built = build();
    let page = &built.pages[PAGE_ROTATE_15];
    let m = demo_rect_fill(page, 1.0);
    // A 15° rotation composes onto the 120×80 UNIT_RECT scale, so the
    // off-diagonal terms must be clearly non-zero (an identity/translate
    // spread would leave b == c == 0).
    assert!(
        m[1].abs() > 1.0 && m[2].abs() > 1.0,
        "rotate-15 spread must rotate the body rect (b={}, c={}) — full {m:?}",
        m[1],
        m[2]
    );
    // Sanity: the column lengths still encode the authored rect dims
    // (rotation preserves length).
    let col0 = (m[0] * m[0] + m[1] * m[1]).sqrt();
    let col1 = (m[2] * m[2] + m[3] * m[3]).sqrt();
    assert!(
        (col0 - DEMO_W_PT).abs() < 1.0,
        "width preserved, got {col0}"
    );
    assert!(
        (col1 - DEMO_H_PT).abs() < 1.0,
        "height preserved, got {col1}"
    );
}

#[test]
fn spread_scale_scales_body_rect_fill() {
    let built = build();
    let page = &built.pages[PAGE_SCALE_125];
    let m = demo_rect_fill(page, 1.25);
    // A 1.25x uniform spread scale bakes 1.25x the authored 120x80 into
    // the fill's linear block; off-diagonals stay zero (pure scale).
    assert!(
        m[1].abs() < 0.01 && m[2].abs() < 0.01,
        "scale spread keeps the rect axis-aligned (b={}, c={})",
        m[1],
        m[2]
    );
    assert!(
        (m[0] - DEMO_W_PT * 1.25).abs() < 1.0,
        "width scaled 1.25x: expected {}, got {}",
        DEMO_W_PT * 1.25,
        m[0]
    );
    assert!(
        (m[3] - DEMO_H_PT * 1.25).abs() < 1.0,
        "height scaled 1.25x: expected {}, got {}",
        DEMO_H_PT * 1.25,
        m[3]
    );
}

#[test]
fn center_point_autosize_grows_above_top_left_control() {
    // The CenterPoint box and the TopLeft control are authored at the
    // SAME top (200pt) with the same story. CenterPoint grows
    // symmetrically about its centre, so its painted box top rises ABOVE
    // the control's top, which stays pinned at the authored 200pt. We
    // read the painted-box tops from the two cyan fill rects (one per
    // box) — the autosize fill whose baked height exceeds the authored
    // 36pt is the grown box.
    let built = build();
    let page = &built.pages[PAGE_CENTER_GROW];

    // Collect axis-aligned fills that are clearly grown autosize boxes:
    // width ~200pt and height well past the authored 36pt.
    let mut grown: Vec<(f32, f32)> = fill_transforms(page)
        .into_iter()
        .filter(|m| m[1].abs() < 0.01 && m[2].abs() < 0.01)
        .filter(|m| (m[0] - 200.0).abs() < 2.0 && m[3] > 36.0 * 1.5)
        .map(|m| (m[5], m[3])) // (top y, height)
        .collect();
    // Sort by top y so [0] is the higher (smaller y) box.
    grown.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert_eq!(
        grown.len(),
        2,
        "expected two grown autosize boxes (centre + top-left control), got {grown:?}"
    );
    let (center_top, _center_h) = grown[0];
    let (control_top, _control_h) = grown[1];
    // The centre box's painted top must rise strictly above the control's
    // — the visible-box symmetric-grow signature. (TopLeft pins the top
    // at the authored 200pt; CenterPoint lifts it.)
    assert!(
        center_top < control_top - 5.0,
        "CenterPoint box top ({center_top}) must rise above TopLeft control top ({control_top})"
    );
    // The control's top stays at (near) the authored 200pt.
    assert!(
        (control_top - 200.0).abs() < 5.0,
        "TopLeft control top should stay pinned ~200pt, got {control_top}"
    );
}
