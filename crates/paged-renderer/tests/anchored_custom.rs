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
const PAGE_H_PT: f32 = 841.890;
const BODY_W_PT: f32 = 460.0;
const ANCHOR_W_PT: f32 = 60.0;
const ANCHOR_H_PT: f32 = 36.0;
const BODY_TOP_PT: f32 = 80.0;

// Margin box (pt) the page-margins variant declares — mirror of the
// sample's MARGIN_* constants. The bottom-right anchor only exercises
// the right + bottom insets; left/top are proved by the resolver's box
// construction (it subtracts all four edges).
const MARGIN_BOTTOM_PT: f32 = 48.0;
const MARGIN_RIGHT_PT: f32 = 60.0;

// Page indices map 1:1 to declaration order in `variants()`.
const PAGE_LINE_BASELINE: usize = 7;
const PAGE_LINE_CAP_HEIGHT: usize = 8;
const PAGE_LINE_TOP_OF_LEADING: usize = 9;
const PAGE_PAGE_MARGINS: usize = 10;
// W1.16 LineXHeight seat — appended last in the sample's variant list.
const PAGE_LINE_X_HEIGHT: usize = 11;

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
        .filter(|(_, _, a, d)| (a - ANCHOR_W_PT).abs() < 0.5 && (d - ANCHOR_H_PT).abs() < 0.5);
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

/// The single 60×36 anchored fill's page-local top-left on a given page.
fn anchored_fill_top_left(
    built: &paged_renderer::pipeline::BuiltDocument,
    page: usize,
) -> (f32, f32) {
    assert!(
        built.pages.len() > page,
        "expected at least {} pages, got {}",
        page + 1,
        built.pages.len()
    );
    let p = &built.pages[page];
    assert_eq!(p.spread_origin, (0.0, 0.0), "single-page spread");
    let mut it = axis_aligned_fills(&p.list.commands)
        .into_iter()
        .filter(|(_, _, a, d)| (a - ANCHOR_W_PT).abs() < 0.5 && (d - ANCHOR_H_PT).abs() < 0.5);
    let (x, y, _, _) = it.next().expect("anchored 60x36 fill present");
    assert!(it.next().is_none(), "exactly one 60x36 fill per page");
    (x, y)
}

/// Open Sans cap-height / x-height / ascent / descent as em-fractions,
/// read straight from the font the test resolves — so the expected
/// vertical-reference deltas track the real metrics rather than baked
/// magic numbers.
fn open_sans_metrics() -> (f32, f32, f32, f32) {
    let bytes = read_font("OpenSans.ttf");
    let face = ttf_parser::Face::parse(&bytes, 0).expect("parse OpenSans");
    let upem = face.units_per_em() as f32;
    let cap = face
        .capital_height()
        .map(|v| v as f32 / upem)
        .unwrap_or(0.70);
    let xh = face.x_height().map(|v| v as f32 / upem).unwrap_or(0.50);
    let asc = face.ascender() as f32 / upem;
    let desc = (face.descender() as f32 / upem).abs();
    (cap, xh, asc, desc)
}

fn build_anchored() -> paged_renderer::pipeline::BuiltDocument {
    let sample = paged_gen::samples::anchored::build();
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
fn line_vertical_reference_points_resolve_real_metrics() {
    let built = build_anchored();

    // All three line-relative variants share identical horizontal
    // placement (TextFrame + RightAlign + TopRightAnchor) and the SAME
    // anchor line, so their X is equal and their Y differs by exactly
    // the font-metric distance above the baseline.
    let (bx, baseline_top) = anchored_fill_top_left(&built, PAGE_LINE_BASELINE);
    let (cx, cap_top) = anchored_fill_top_left(&built, PAGE_LINE_CAP_HEIGHT);
    let (lx, leading_top) = anchored_fill_top_left(&built, PAGE_LINE_TOP_OF_LEADING);

    // Horizontal placement is the deterministic TextFrame-right snap,
    // identical on every line variant.
    let expected_x = (PAGE_W_PT - BODY_W_PT) * 0.5 + BODY_W_PT - ANCHOR_W_PT;
    for (label, x) in [("baseline", bx), ("cap", cx), ("leading", lx)] {
        assert!(
            (x - expected_x).abs() < 0.5,
            "{label} variant x: expected {expected_x}, got {x}"
        );
    }

    // Default point size (no PointSize attr ⇒ 12pt) and auto leading
    // (1.2× ⇒ 14.4pt). The Y of each reference is the anchor line's
    // baseline minus the metric distance.
    const PT: f32 = 12.0;
    const LEADING: f32 = PT * 1.2;
    let (cap_em, _xh_em, asc, desc) = open_sans_metrics();

    // LineBaseline variant: anchor top sits ON the baseline (TopAlign +
    // TopRightAnchor). LineCapHeight sits cap_height·pt above it.
    let cap_delta = baseline_top - cap_top;
    assert!(
        (cap_delta - cap_em * PT).abs() < 0.2,
        "cap-height delta: expected {} (cap_em·pt), got {cap_delta}",
        cap_em * PT
    );

    // TopOfLeading: leading split in the font's ascent:descent ratio.
    let leading_above = LEADING * asc / (asc + desc);
    let leading_delta = baseline_top - leading_top;
    assert!(
        (leading_delta - leading_above).abs() < 0.2,
        "top-of-leading delta: expected {leading_above}, got {leading_delta}"
    );

    // Strict ordering above the baseline: leading-top is highest, then
    // cap-height, then the baseline itself.
    assert!(
        leading_top < cap_top && cap_top < baseline_top,
        "ordering leading({leading_top}) < cap({cap_top}) < baseline({baseline_top})"
    );
}

#[test]
fn line_x_height_reference_seats_between_baseline_and_cap_height() {
    // W1.16 LineXHeight seat: the frame's top lands x_height·pt above the
    // anchor line's baseline — strictly between the baseline (0 above)
    // and the cap-height (cap_height·pt above, cap > x-height for Latin
    // fonts). Shares the deterministic horizontal snap with the other
    // line variants, so X is identical and only the vertical reference
    // moves.
    let built = build_anchored();
    let (xx, xheight_top) = anchored_fill_top_left(&built, PAGE_LINE_X_HEIGHT);
    let (_, baseline_top) = anchored_fill_top_left(&built, PAGE_LINE_BASELINE);
    let (_, cap_top) = anchored_fill_top_left(&built, PAGE_LINE_CAP_HEIGHT);

    let expected_x = (PAGE_W_PT - BODY_W_PT) * 0.5 + BODY_W_PT - ANCHOR_W_PT;
    assert!(
        (xx - expected_x).abs() < 0.5,
        "x-height variant x: expected {expected_x}, got {xx}"
    );

    const PT: f32 = 12.0;
    let (_cap_em, xh_em, _asc, _desc) = open_sans_metrics();
    // The x-height top sits x_height·pt above the baseline.
    let xh_delta = baseline_top - xheight_top;
    assert!(
        (xh_delta - xh_em * PT).abs() < 0.2,
        "x-height delta: expected {} (xh_em·pt), got {xh_delta}",
        xh_em * PT
    );
    // Strict ordering above the baseline: cap-height higher than
    // x-height, x-height higher than baseline.
    assert!(
        cap_top < xheight_top && xheight_top < baseline_top,
        "ordering cap({cap_top}) < x-height({xheight_top}) < baseline({baseline_top})"
    );
}

#[test]
fn page_margins_reference_snaps_to_margin_box_not_page_edge() {
    let built = build_anchored();
    let (x, y) = anchored_fill_top_left(&built, PAGE_PAGE_MARGINS);

    // BottomRightAnchor + RightAlign + BottomAlign against the margin
    // box: frame bottom-right on the margin's bottom-right corner.
    let margin_right_x = PAGE_W_PT - MARGIN_RIGHT_PT;
    let margin_bottom_y = PAGE_H_PT - MARGIN_BOTTOM_PT;
    let expected_x = margin_right_x - ANCHOR_W_PT;
    let expected_y = margin_bottom_y - ANCHOR_H_PT;
    assert!(
        (x - expected_x).abs() < 0.5,
        "page-margins x: expected {expected_x} (margin right - anchor_w), got {x}"
    );
    assert!(
        (y - expected_y).abs() < 0.5,
        "page-margins y: expected {expected_y} (margin bottom - anchor_h), got {y}"
    );

    // Divergence proof: had margins degenerated to the page edge, x
    // would be PAGE_W - ANCHOR_W and y PAGE_H - ANCHOR_H. The margin
    // insets push the frame strictly inward on both axes.
    assert!(
        x < PAGE_W_PT - ANCHOR_W_PT - 1.0,
        "x must be inside the page-edge placement (margins honoured)"
    );
    assert!(
        y < PAGE_H_PT - ANCHOR_H_PT - 1.0,
        "y must be inside the page-edge placement (margins honoured)"
    );
}
