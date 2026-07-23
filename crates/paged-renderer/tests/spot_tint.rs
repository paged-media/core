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

//! End-to-end: a Spot colour with `TintValue="50"` renders distinctly
//! lighter than the same Spot colour at 100% tint.
//!
//! Spot ink names (e.g. PANTONE 286) are NOT supported in this
//! renderer — we always preview spot colours via their
//! `AlternateColorValue` (CMYK fallback). The swatch-level
//! `TintValue` is applied to that alternate in CMYK space *before*
//! the ICC transform: `tinted_cmyk = base_cmyk * (tint / 100)`. The
//! result is the same channel-scaling InDesign applies in screen
//! preview, mathematically equivalent to a linear interpolation
//! between the resolved colour and paper white in CMYK.

use std::io::Write;

use paged_compose::Color;
use paged_renderer::{pipeline, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// Build a synthetic two-page IDML where:
///   * page 1 has a rectangle filled with `Color/PantoneFull` (spot,
///     CMYK alternate `100 75 0 0`, no TintValue → 100% tint).
///   * page 2 has a rectangle filled with `Color/PantoneHalf` (same
///     swatch but `TintValue="50"` → 50% tint).
fn build_spot_tint_idml() -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();

    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
</Document>"#,
    )
    .unwrap();

    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/PantoneFull" Name="PANTONE 286 C" Model="Spot"
           Space="LAB" ColorValue="20 25 -70"
           AlternateSpace="CMYK" AlternateColorValue="100 75 0 0"/>
    <Color Self="Color/PantoneHalf" Name="PANTONE 286 C 50%" Model="Spot"
           Space="LAB" ColorValue="20 25 -70"
           AlternateSpace="CMYK" AlternateColorValue="100 75 0 0"
           TintValue="50"/>
  </Graphic>
</idPkg:Graphic>"#,
    )
    .unwrap();

    // Two pages side by side. `GeometricBounds` is `(top left bottom
    // right)` in spread coords; each rectangle fills its page.
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 200 200"/>
    <Page Self="p2" GeometricBounds="0 200 200 400"/>
    <Rectangle Self="rFull" GeometricBounds="0 0 200 200"
               FillColor="Color/PantoneFull" StrokeWeight="0"/>
    <Rectangle Self="rHalf" GeometricBounds="0 200 200 400"
               FillColor="Color/PantoneHalf" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

#[test]
fn spot_color_at_half_tint_renders_lighter_than_full_tint() {
    let bytes = build_spot_tint_idml();
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let (_built, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();

    assert_eq!(images.len(), 2, "two pages → two rasters");
    let full = images[0].get_pixel(100, 100);
    let half = images[1].get_pixel(100, 100);

    // Naive CMYK→linear-RGB (no ICC profile in this test):
    //   full tint: C=1.00, M=0.75, K=0 → r=0,    g=0.25, b=1
    //   half tint: C=0.50, M=0.375    → r=0.5,   g=0.625, b=1
    // After sRGB encode the half-tint pixel is *visibly* lighter and
    // pinker. We assert directional inequalities (the precise byte
    // values can drift if the linear→sRGB path is retuned).
    assert!(
        half.0[0] > full.0[0] + 20,
        "half-tint R should be markedly higher than full-tint R: half={:?} full={:?}",
        half,
        full,
    );
    assert!(
        half.0[1] > full.0[1] + 20,
        "half-tint G should be higher than full-tint G: half={:?} full={:?}",
        half,
        full,
    );
    // Blue is saturated in both — pin equality within 2 LSBs of
    // sRGB encoding noise.
    assert!(
        (half.0[2] as i32 - full.0[2] as i32).abs() <= 2,
        "blue should be ~identical: half={:?} full={:?}",
        half,
        full,
    );
    // Both must be opaque.
    assert_eq!(full.0[3], 255);
    assert_eq!(half.0[3], 255);
}
