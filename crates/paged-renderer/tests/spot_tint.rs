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
//! lighter than the same Spot colour at 100% tint — and, under the Ink
//! Manager's "Use Standard Lab Values for Spots", from its Lab primary
//! instead of its CMYK alternate.
//!
//! By default we preview a spot colour via its `AlternateColorValue`
//! (CMYK fallback), the way InDesign does out of the box, and the
//! swatch-level `TintValue` is applied to that alternate in CMYK space
//! *before* the ICC transform: `tinted_cmyk = base_cmyk * (tint / 100)`.
//! That is the same channel-scaling InDesign applies in screen preview,
//! mathematically equivalent to a linear interpolation between the
//! resolved colour and paper white in CMYK.
//!
//! `PipelineOptions::use_standard_lab_for_spots` is the Ink Manager
//! switch that says "trust the measurement instead": the swatch's Lab
//! primary drives the DISPLAY colour (tint interpolated in Lab, toward
//! paper white), while the CMYK alternate stays on the paint's channels
//! so separations and overprint are unaffected.

use std::io::Write;

use paged_compose::{Color, DisplayCommand, Paint};
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

/// sRGB-encode a linear channel the way the rasterizer does, to byte.
fn srgb_byte(linear: f32) -> i32 {
    let s = if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0).round() as i32
}

/// The Ink Manager's "Use Standard Lab Values for Spots" must change
/// what lands on the PAGE, not just what a swatch chip previews.
///
/// The fixture's spot is `Space="LAB" ColorValue="20 25 -70"` with a
/// CMYK alternate of `100 75 0 0` — a genuinely different colour — so
/// "the option is wired to the renderer" and "the option does nothing"
/// have visibly different answers here.
#[test]
fn standard_lab_for_spots_paints_the_lab_primary_not_the_cmyk_alternate() {
    let bytes = build_spot_tint_idml();
    let document = idml_import::import_idml_doc(&bytes).unwrap();

    let (_b, alternate) =
        pipeline::render_document(&document, &PipelineOptions::default(), 72.0, Color::WHITE)
            .unwrap();
    let lab_opts = PipelineOptions {
        use_standard_lab_for_spots: true,
        ..PipelineOptions::default()
    };
    let (_b2, lab) = pipeline::render_document(&document, &lab_opts, 72.0, Color::WHITE).unwrap();

    let via_alternate = alternate[0].get_pixel(100, 100);
    let via_lab = lab[0].get_pixel(100, 100);
    assert_ne!(
        via_alternate.0, via_lab.0,
        "the setting must change the rendered pixel; got {via_lab:?} either way"
    );

    // The standard-Lab pixel is the swatch's measured Lab primary,
    // resolved device-independently (D50 → Bradford → linear sRGB).
    let paged_color::LinearRgb(expected) =
        paged_color::lab::lab_d50_to_linear_srgb(20.0, 25.0, -70.0);
    for (ch, want) in expected.iter().enumerate() {
        assert!(
            (via_lab.0[ch] as i32 - srgb_byte(*want)).abs() <= 2,
            "channel {ch}: rendered {:?}, expected Lab(20 25 -70) → {:?}",
            via_lab,
            expected.map(srgb_byte),
        );
    }

    // Page 2's swatch is the same ink at `TintValue="50"`. A tint mixes
    // toward paper white, so in Lab it lightens L* rather than scaling
    // channels — the half-tint page stays lighter under this setting too.
    let half_via_lab = lab[1].get_pixel(100, 100);
    assert!(
        half_via_lab.0[0] > via_lab.0[0] && half_via_lab.0[1] > via_lab.0[1],
        "50% of a Lab spot must render lighter: half={half_via_lab:?} full={via_lab:?}"
    );
}

/// With an ICC profile loaded, standard-Lab changes the paint's DISPLAY
/// colour and leaves its CMYK channels alone: those channels are the
/// spot's alternate, which is what the spot plane composites and what
/// separates on output. The Ink Manager toggle governs how an ink is
/// shown, not which plate it lands on.
#[test]
fn standard_lab_for_spots_leaves_the_separation_channels_alone() {
    let Some(profile) = paged_color::test_profiles::read_cmyk_profile(env!("CARGO_MANIFEST_DIR"))
    else {
        eprintln!(
            "skipping standard_lab_for_spots_leaves_the_separation_channels_alone: {}",
            paged_color::test_profiles::NO_PROFILE_HINT
        );
        return;
    };
    let bytes = build_spot_tint_idml();
    let document = idml_import::import_idml_doc(&bytes).unwrap();

    let fill_paint = |use_lab: bool| -> Paint {
        let opts = PipelineOptions {
            cmyk_icc_profile: Some(&profile),
            use_standard_lab_for_spots: use_lab,
            ..PipelineOptions::default()
        };
        let built = pipeline::build_document(&document, &opts).unwrap();
        built.pages[0]
            .list
            .commands
            .iter()
            .find_map(|c| match c {
                DisplayCommand::FillPath { paint, .. } => Some(*paint),
                _ => None,
            })
            .expect("the full-tint rectangle fills page 1")
    };

    let (
        Paint::Cmyk {
            c, m, y, k, rgb, ..
        },
        Paint::Cmyk {
            c: c2,
            m: m2,
            y: y2,
            k: k2,
            rgb: rgb2,
            ..
        },
    ) = (fill_paint(false), fill_paint(true))
    else {
        panic!("a spot with a CMYK alternate resolves to a CMYK paint under ICC");
    };
    assert_eq!(
        [c, m, y, k],
        [c2, m2, y2, k2],
        "the CMYK alternate (and therefore the separation) must not move"
    );
    assert_ne!(
        (rgb.r, rgb.g, rgb.b),
        (rgb2.r, rgb2.g, rgb2.b),
        "the displayed colour must move"
    );
    let paged_color::LinearRgb(expected) =
        paged_color::lab::lab_d50_to_linear_srgb(20.0, 25.0, -70.0);
    assert!(
        (rgb2.r - expected[0]).abs() < 1e-3
            && (rgb2.g - expected[1]).abs() < 1e-3
            && (rgb2.b - expected[2]).abs() < 1e-3,
        "standard-Lab display colour should be the Lab primary: got {rgb2:?}, want {expected:?}"
    );
}
