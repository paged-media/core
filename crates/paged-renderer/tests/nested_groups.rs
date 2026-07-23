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

//! W4.11 — display-list assertions over the generated `nested-groups.idml`
//! fixture (group-of-groups, W1.20).
//!
//! The distinctive, exactly-assertable effect is on page 2 (scaled
//! outer): the outer group scales 1.5×, two inner groups translate, and
//! every leaf rect is axis-aligned. The 1.5× outer scale must compose
//! through BOTH inner groups onto every leaf, so each leaf's emitted
//! fill linear block reads 1.5× its authored 60×40 — proof the renderer
//! composes `outer ∘ inner ∘ leaf` over the whole sub-tree, not just one
//! chain. Page 1 (rotated inners) proves the leaves pick up the inner
//! groups' rotation (off-diagonal terms non-zero).

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, PipelineOptions};

const LEAF_W_PT: f32 = 60.0;
const LEAF_H_PT: f32 = 40.0;

const PAGE_ROTATE_INNERS: usize = 0;
const PAGE_SCALE_OUTER: usize = 1;

fn build() -> pipeline::BuiltDocument {
    let bytes =
        paged_gen::write_idml(&paged_gen::samples::nested_groups::build()).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("Document::open");
    // No fonts needed — the only label text would shape away, and the
    // leaf rects (the feature) emit fills regardless.
    let opts = PipelineOptions::default();
    pipeline::build_document(&document, &opts).expect("build_document")
}

/// Every axis-aligned `FillPath` linear block `(|a|, |d|)` on the page
/// (b ≈ c ≈ 0). The leaf rects ride UNIT_RECT so `a`/`d` are the baked
/// width/height.
fn axis_aligned_fill_dims(page: &pipeline::BuiltPage) -> Vec<(f32, f32)> {
    page.list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => {
                let [a, b, c2, d, _, _] = transform.0;
                if b.abs() < 0.01 && c2.abs() < 0.01 {
                    Some((a.abs(), d.abs()))
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect()
}

#[test]
fn scaled_outer_group_scales_every_leaf_through_both_inner_groups() {
    let built = build();
    let page = &built.pages[PAGE_SCALE_OUTER];

    let want_w = LEAF_W_PT * 1.5;
    let want_h = LEAF_H_PT * 1.5;
    let scaled_leaves: Vec<_> = axis_aligned_fill_dims(page)
        .into_iter()
        .filter(|(w, h)| (w - want_w).abs() < 0.5 && (h - want_h).abs() < 0.5)
        .collect();
    // Four leaves (two inner groups × two leaves), each scaled 1.5×.
    assert_eq!(
        scaled_leaves.len(),
        4,
        "all four leaves must read 1.5x ({want_w}x{want_h}) through outer∘inner∘leaf, got {scaled_leaves:?}"
    );
}

#[test]
fn rotated_inner_groups_rotate_their_leaves() {
    let built = build();
    let page = &built.pages[PAGE_ROTATE_INNERS];

    // Each leaf fill on this page rides one of the rotated inner groups
    // (±20°), so its transform must carry non-zero off-diagonal terms.
    // Filter to the leaf-sized fills (column lengths ≈ 60 / 40) and
    // assert every one is rotated.
    let leaf_fills: Vec<[f32; 6]> = page
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => Some(transform.0),
            _ => None,
        })
        .filter(|m| {
            let col0 = (m[0] * m[0] + m[1] * m[1]).sqrt();
            let col1 = (m[2] * m[2] + m[3] * m[3]).sqrt();
            (col0 - LEAF_W_PT).abs() < 1.0 && (col1 - LEAF_H_PT).abs() < 1.0
        })
        .collect();
    assert_eq!(
        leaf_fills.len(),
        4,
        "four leaf fills, got {}",
        leaf_fills.len()
    );
    for m in &leaf_fills {
        assert!(
            m[1].abs() > 1.0 || m[2].abs() > 1.0,
            "each leaf must inherit its inner group's rotation (b/c non-zero), got {m:?}"
        );
    }
}
