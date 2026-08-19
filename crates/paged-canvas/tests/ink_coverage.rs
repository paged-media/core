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

//! §21 advanced prepress — ink coverage.
//!
//! Two readings, deliberately independent:
//!
//!  * `SwatchSummary::total_area_coverage_pct` — exact palette
//!    arithmetic. No render, no profile, no resolution.
//!  * the `inkCoverage` collection — the rendered per-page separation,
//!    read off the CPU rasterizer's ink planes. Needs an active CMYK
//!    working profile to exist at all, and only covers pixels the
//!    raster lane could decompose.
//!
//! The tests below pin the boundary between them, because the failure
//! mode this feature must not have is a confident zero.

use std::io::Write;

use paged_canvas::{channel::CollectionName, CanvasModel, CanvasOptions};

/// Minimal one-page IDML with a palette that exercises every
/// total-area-coverage branch, plus rectangles that put the rich black
/// and the RGB blue on the page.
fn prepress_idml(rect_fill: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Graphic src="Resources/Graphic.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Resources/Graphic.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Color Self="Color/richblack" Model="Process" Space="CMYK" ColorValue="60 40 40 100" Name="Rich Black"/>
<Color Self="Color/flatk" Model="Process" Space="CMYK" ColorValue="0 0 0 100" Name="Flat Black"/>
<Color Self="Color/pms" Model="Spot" Space="LAB" ColorValue="48 64 47" AlternateSpace="CMYK" AlternateColorValue="0 91 76 0" Name="PANTONE Warm Red C"/>
<Color Self="Color/rgbblue" Model="Process" Space="RGB" ColorValue="0 0 255" Name="RGB Blue"/>
</idPkg:Graphic>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 400 400" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" FillColor="{rect_fill}" GeometricBounds="0 0 200 400" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

/// A valid CMYK ICC profile can't be synthesised inline, so the
/// profile-dependent tests locate one exactly the way
/// `paged-color/tests/parity.rs` does — `PAGED_CMYK_PROFILE`, then
/// `corpus/profiles/*.icc` (private corpus), then Adobe's recommended
/// install. They skip when none is reachable; the *branch* that
/// matters most (no profile ⇒ no ink lane) is pinned unconditionally
/// by `ink_coverage_without_a_cmyk_profile_says_so_instead_of_reporting_zero`.
fn cmyk_profile() -> Option<Vec<u8>> {
    // ONE resolver for the whole workspace (env → corpus/profiles →
    // local Adobe install). Three hand-rolled copies used to disagree,
    // and ~8 colour tests skipped silently as a result — see
    // `paged_color::test_profiles`.
    paged_color::test_profiles::read_cmyk_profile(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn swatch_total_area_coverage_is_exact_and_needs_no_profile() {
    let model = CanvasModel::load(
        "doc-1",
        &prepress_idml("Color/richblack"),
        CanvasOptions::default(),
    )
    .expect("load");
    let swatches = model.swatches();
    let by_id = |id: &str| {
        swatches
            .iter()
            .find(|s| s.self_id == id)
            .unwrap_or_else(|| panic!("swatch {id}"))
    };

    // Process CMYK sums its four channels.
    let rich = by_id("Color/richblack").total_area_coverage_pct.unwrap();
    assert!((rich - 240.0).abs() < 0.01, "rich black TAC = {rich}");
    let flat = by_id("Color/flatk").total_area_coverage_pct.unwrap();
    assert!((flat - 100.0).abs() < 0.01, "flat black TAC = {flat}");

    // A SPOT is one plate at its own tint — NOT the sum of its CMYK
    // alternate (0/91/76/0 would read 167% and flag every duotone).
    let spot = by_id("Color/pms").total_area_coverage_pct.unwrap();
    assert!((spot - 100.0).abs() < 0.01, "spot TAC = {spot}");

    // RGB has no ink decomposition here — it separates at the RIP.
    // `None`, never `Some(0.0)`: a UI must show a blank, not a zero.
    assert_eq!(by_id("Color/rgbblue").total_area_coverage_pct, None);
}

#[test]
fn ink_coverage_without_a_cmyk_profile_says_so_instead_of_reporting_zero() {
    // The engine's ink lane only exists when a CMYK working profile is
    // active: without one, `color_id_to_paint` cannot build a
    // `Paint::Cmyk` and every swatch — including the rich black —
    // resolves to display RGB. The collection must make that legible,
    // because "no profile" and "all-RGB artwork" both bottom out at
    // 0% separated and mean completely different things.
    let model = CanvasModel::load(
        "doc-1",
        &prepress_idml("Color/richblack"),
        CanvasOptions::default(),
    )
    .expect("load");
    let rows = model.ink_coverage();
    assert_eq!(rows.len(), 1);
    assert!(
        !rows[0].separation_available,
        "no profile was supplied, so no ink lane exists"
    );
    assert_eq!(rows[0].separated_pixels, 0);
    assert!(rows[0].total_pixels > 0, "the page still has pixels");

    // The swatch audit is unaffected — it never needed the profile.
    let rich = model
        .swatches()
        .into_iter()
        .find(|s| s.self_id == "Color/richblack")
        .unwrap()
        .total_area_coverage_pct
        .unwrap();
    assert!((rich - 240.0).abs() < 0.01);
}

#[test]
fn ink_coverage_measures_a_rich_black_when_a_profile_is_active() {
    let Some(profile) = cmyk_profile() else {
        eprintln!("skipping: {}", paged_color::test_profiles::NO_PROFILE_HINT);
        return;
    };
    let model = CanvasModel::load(
        "doc-1",
        &prepress_idml("Color/richblack"),
        CanvasOptions {
            cmyk_icc_profile: Some(profile),
            ..CanvasOptions::default()
        },
    )
    .expect("load");
    let rows = model.ink_coverage();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert!(row.separation_available);

    // The rectangle covers the top half of the page, so about half the
    // pixels separate. The report carries that denominator so nobody
    // reads maxTac as a whole-page verdict.
    assert!(
        (40.0..=60.0).contains(&row.separated_pct),
        "half-page fill separated_pct = {}",
        row.separated_pct
    );
    assert!(
        (row.max_tac_pct - 240.0).abs() < 2.0,
        "rich-black page max TAC = {}",
        row.max_tac_pct
    );
    // 240% is inside the 300% default limit.
    assert_eq!(row.over_limit_pixels, 0);
    assert_eq!(row.limit_pct, 300.0);

    // Four process plates, named the way the Ink Manager names inks.
    assert_eq!(row.plates.len(), 4);
    let ids: Vec<&str> = row.plates.iter().map(|p| p.ink_id.as_str()).collect();
    assert_eq!(ids, vec!["cyan", "magenta", "yellow", "black"]);
    assert!(row.plates.iter().all(|p| !p.is_spot));
    let k = row.plates.iter().find(|p| p.ink_id == "black").unwrap();
    assert!(
        (k.max_tint_pct - 100.0).abs() < 1.0,
        "K plate max tint = {}",
        k.max_tint_pct
    );
    let c = row.plates.iter().find(|p| p.ink_id == "cyan").unwrap();
    assert!(
        (c.max_tint_pct - 60.0).abs() < 1.5,
        "C plate max tint = {}",
        c.max_tint_pct
    );

    // The histogram accounts for exactly the separated pixels, so a
    // panel can re-threshold the ink limit without a re-render.
    let counted: u32 = row.histogram.iter().sum();
    assert_eq!(counted, row.separated_pixels);
    assert_eq!(row.histogram.len(), paged_renderer::TAC_BUCKETS);
}

#[test]
fn rgb_artwork_separates_nothing_even_with_a_profile_active() {
    let Some(profile) = cmyk_profile() else {
        eprintln!("skipping: {}", paged_color::test_profiles::NO_PROFILE_HINT);
        return;
    };
    let model = CanvasModel::load(
        "doc-1",
        &prepress_idml("Color/rgbblue"),
        CanvasOptions {
            cmyk_icc_profile: Some(profile),
            ..CanvasOptions::default()
        },
    )
    .expect("load");
    let rows = model.ink_coverage();
    let row = &rows[0];
    // The profile IS active — so this zero means "the artwork carries
    // no ink decomposition", not "activate a profile". The two states
    // are distinguishable, which is the whole point of the flag.
    assert!(row.separation_available);
    assert_eq!(row.separated_pixels, 0);
    assert_eq!(row.max_tac_pct, 0.0);
}

#[test]
fn ink_coverage_routes_through_the_collection_dispatcher() {
    let model = CanvasModel::load(
        "doc-1",
        &prepress_idml("Color/richblack"),
        CanvasOptions::default(),
    )
    .expect("load");
    let via_named = serde_json::to_value(model.ink_coverage()).unwrap();
    let via_dispatch = model.collection(CollectionName::InkCoverage);
    assert_eq!(via_named, via_dispatch);
    assert_eq!(
        CollectionName::from_str("inkCoverage"),
        Some(CollectionName::InkCoverage)
    );
    assert_eq!(CollectionName::InkCoverage.as_str(), "inkCoverage");
    // Memoised: a second read is the same data, not a fresh render.
    assert_eq!(
        serde_json::to_value(model.ink_coverage()).unwrap(),
        serde_json::to_value(model.ink_coverage()).unwrap()
    );
}
