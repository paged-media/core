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

//! Emission test for Custom / `Anchored`-position anchored frames.
//!
//! Builds the `paged-gen` `anchored.idml` fixture (which carries a
//! deterministic `custom · textframe-top-right` variant) and asserts
//! that the anchored frame's emitted fill rect lands where the
//! `HorizontalReferencePoint=TextFrame` + `RightAlign` +
//! `TopRightAnchor` semantics demand: the frame's top-right corner on
//! the host text frame's top-right edge.
//!
//! The host body frame is placed by the sample at
//! `translate((PAGE_W - BODY_W)/2, 80)` with size `BODY_W × BODY_H`, on
//! single-page spreads (so the page's spread-origin is `(0, 0)` and
//! page-local == spread coords). The expected anchored placement is
//! therefore fully determined:
//!   body_right = (PAGE_W - BODY_W)/2 + BODY_W
//!   anchor_left = body_right - ANCHOR_W   (TopRightAnchor + RightAlign)
//!   anchor_top  = 80                      (TextFrame top + TopAlign)
//!
//! A font resolver (Open Sans, the family the sample declares) is
//! required: the anchored-frame emit runs per *laid-out* paragraph, so
//! without glyph layout the host story never reaches the anchor.

use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, BytesResolver, Document, PipelineOptions};

// Geometry constants mirrored from `paged-gen`'s anchored sample.
const PAGE_W_PT: f32 = 595.276;
const BODY_W_PT: f32 = 460.0;
const ANCHOR_W_PT: f32 = 60.0;
const ANCHOR_H_PT: f32 = 36.0;
const BODY_TOP_PT: f32 = 80.0;

fn read_font(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read font fixture {}: {e}", p.display()))
}

/// Page-local top-left + scale of every axis-aligned `FillPath` on a
/// page. The display list stores a `FillPath` transform as
/// `[a, b, c, d, tx, ty]`; for a unit-rect frame fill emitted via
/// `Transform::for_rect_in` with a pure-translate outer, `a`/`d` are
/// the frame's width/height and `tx`/`ty` its page-local top-left.
/// Glyph fills carry non-zero `b`/`c` (rotation/shear) or sub-point
/// scales and are filtered out by the size match below.
fn axis_aligned_fills(cmds: &[DisplayCommand]) -> Vec<(f32, f32, f32, f32)> {
    cmds.iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => {
                let [a, b, c2, d, tx, ty] = transform.0;
                if b.abs() < 1e-3 && c2.abs() < 1e-3 {
                    Some((tx, ty, a, d))
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect()
}

#[test]
fn custom_anchored_frame_snaps_to_textframe_top_right() {
    let sample = paged_gen::samples::anchored::build();
    let bytes = paged_gen::write_idml(&sample).expect("write_idml");
    let document = Document::open(&bytes).expect("Document::open");

    let mut resolver = BytesResolver::new();
    resolver.add_font("Open Sans", None, read_font("OpenSans.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).expect("build_document");

    // The custom · textframe-top-right variant is the 4th page (after
    // inline, above-line, custom-x-y). Variants map 1:1 to pages in
    // declaration order.
    const CUSTOM_PAGE: usize = 3;
    assert!(
        built.pages.len() > CUSTOM_PAGE,
        "expected at least {} pages, got {}",
        CUSTOM_PAGE + 1,
        built.pages.len()
    );
    let page = &built.pages[CUSTOM_PAGE];
    assert_eq!(
        page.spread_origin,
        (0.0, 0.0),
        "single-page spread: page-local == spread coords"
    );

    // The anchored frame is the only ANCHOR_W × ANCHOR_H axis-aligned
    // fill on the page (the host body frame has no fill colour, so it
    // emits glyphs only).
    let mut anchored = axis_aligned_fills(&page.list.commands)
        .into_iter()
        .filter(|(_, _, a, d)| {
            (a - ANCHOR_W_PT).abs() < 0.5 && (d - ANCHOR_H_PT).abs() < 0.5
        });
    let (anchor_x, anchor_y, _, _) = anchored
        .next()
        .expect("anchored frame fill (60x36) present on the custom page");
    assert!(
        anchored.next().is_none(),
        "expected exactly one 60x36 anchored fill on the page"
    );

    // TextFrame reference + RightAlign + TopRightAnchor:
    let body_right = (PAGE_W_PT - BODY_W_PT) * 0.5 + BODY_W_PT;
    let expected_x = body_right - ANCHOR_W_PT;
    let expected_y = BODY_TOP_PT;
    assert!(
        (anchor_x - expected_x).abs() < 0.5,
        "anchored frame x: expected {expected_x} (body_right - anchor_w), got {anchor_x}"
    );
    assert!(
        (anchor_y - expected_y).abs() < 0.5,
        "anchored frame y: expected {expected_y} (text frame top), got {anchor_y}"
    );
}
