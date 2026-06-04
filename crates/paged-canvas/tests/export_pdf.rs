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

//! Concept 3 — the worker-side export session, exercised headlessly
//! (no wasm): begin → one page per call → finish over a synthetic
//! IDML, plus the X-4 profile gate and the begin-time option
//! validation. The wasm dispatch is a thin map around exactly these
//! calls; the Playwright layer re-tests it through the real wire.

use std::io::Write;

use paged_canvas::channel::ExportPdfWireOptions;
use paged_canvas::export::CanvasExportSession;
use paged_canvas::{CanvasModel, CanvasOptions, ColorProfileEntry};

fn small_idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package").unwrap();
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
<Document DOMVersion="13.1" Self="d1" CMYKProfile="Test CMYK Profile">
<DocumentPreference DocumentBleedTopOffset="8.5" DocumentBleedBottomOffset="8.5" DocumentBleedInsideOrLeftOffset="8.5" DocumentBleedOutsideOrRightOffset="8.5"/>
<idPkg:Graphic src="Resources/Graphic.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Spread src="Spreads/Spread_s2.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Resources/Graphic.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Color Self="Color/cyan" Model="Process" Space="CMYK" ColorValue="100 0 0 0" Name="Cyan"/>
<Color Self="Color/spotorange" Model="Spot" Space="LAB" ColorValue="65 40 80" AlternateSpace="CMYK" AlternateColorValue="0 60 100 0" Name="Spot Orange"/>
</idPkg:Graphic>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" FillColor="Color/cyan" StrokeColor="Swatch/None" StrokeWeight="0" ItemTransform="1 0 0 1 50 50" Visible="true"><Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray><PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/><PathPointType Anchor="0 150" LeftDirection="0 150" RightDirection="0 150"/><PathPointType Anchor="150 150" LeftDirection="150 150" RightDirection="150 150"/><PathPointType Anchor="150 0" LeftDirection="150 0" RightDirection="150 0"/></PathPointArray></GeometryPathType></PathGeometry></Properties></Rectangle>
<Rectangle Self="r2" FillColor="Color/spotorange" StrokeColor="Swatch/None" StrokeWeight="0" ItemTransform="1 0 0 1 50 250" Visible="true"><Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray><PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/><PathPointType Anchor="0 150" LeftDirection="0 150" RightDirection="0 150"/><PathPointType Anchor="150 150" LeftDirection="150 150" RightDirection="150 150"/><PathPointType Anchor="150 0" LeftDirection="150 0" RightDirection="150 0"/></PathPointArray></GeometryPathType></PathGeometry></Properties></Rectangle>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s2.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s2" PageCount="1">
<Page Self="p2" Name="2" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r3" FillColor="Color/cyan" StrokeColor="Swatch/None" StrokeWeight="0" ItemTransform="1 0 0 1 100 100" Visible="true"><Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray><PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/><PathPointType Anchor="0 200" LeftDirection="0 200" RightDirection="0 200"/><PathPointType Anchor="200 200" LeftDirection="200 200" RightDirection="200 200"/><PathPointType Anchor="200 0" LeftDirection="200 0" RightDirection="200 0"/></PathPointArray></GeometryPathType></PathGeometry></Properties></Rectangle>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn find_profile() -> Option<Vec<u8>> {
    if let Ok(p) = std::env::var("PAGED_CMYK_PROFILE") {
        if let Ok(bytes) = std::fs::read(&p) {
            return Some(bytes);
        }
    }
    let manifest = env!("CARGO_MANIFEST_DIR");
    let corpus = std::path::Path::new(manifest).join("../../corpus/profiles");
    if let Ok(entries) = std::fs::read_dir(&corpus) {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("icc")) {
                if let Ok(bytes) = std::fs::read(&path) {
                    return Some(bytes);
                }
            }
        }
    }
    let adobe = "/Library/Application Support/Adobe/Color/Profiles/Recommended/CoatedFOGRA39.icc";
    std::fs::read(adobe).ok()
}

#[test]
fn session_exports_page_by_page() {
    let model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    let (mut session, pages) =
        CanvasExportSession::begin(&model, &ExportPdfWireOptions::default()).expect("begin");
    assert_eq!(pages, 2);
    assert_eq!(session.pages_done(), 0);

    // Progress is monotone, one page per call.
    let (done, total) = session.export_next_page().expect("page 1");
    assert_eq!((done, total), (1, 2));
    let (done, total) = session.export_next_page().expect("page 2");
    assert_eq!((done, total), (2, 2));
    // Driving past the end is a session-state error, not a panic.
    assert!(session.export_next_page().is_err());

    let (bytes, diagnostics) = session.finish().expect("finish");
    assert!(bytes.starts_with(b"%PDF-1.7"), "plain export is PDF 1.7");
    assert!(bytes.windows(5).rev().take(64).any(|w| w == b"%%EOF"));
    assert!(diagnostics.is_empty(), "unexpected diagnostics: {diagnostics:?}");

    // Without a CMYK working profile the pipeline collapses swatches
    // to RGB solids (Stage A/B display behaviour) — no Separation to
    // assert here; the profile-gated X-4 test below covers the spot
    // plate. Just pin that the pages actually painted.
    assert!(bytes.len() > 900, "suspiciously empty export: {} bytes", bytes.len());
}

#[test]
fn finishing_early_is_an_error() {
    let model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    let (session, _pages) =
        CanvasExportSession::begin(&model, &ExportPdfWireOptions::default()).expect("begin");
    assert!(session.finish().is_err(), "finish with pages remaining must fail");
}

#[test]
fn x4_requires_a_profile_and_honours_a_registered_one() {
    // Without any profile: X-4 must refuse at begin.
    let model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    let wire = ExportPdfWireOptions {
        standard: Some("pdfx4".into()),
        ..Default::default()
    };
    assert!(CanvasExportSession::begin(&model, &wire).is_err());

    // With a registered profile resolved BY NAME: X-4 exports.
    let Some(profile) = find_profile() else {
        eprintln!("export_pdf: no CMYK profile available — skipping the X-4 half");
        return;
    };
    // Registered under the NAME the designmap declares, so it
    // activates as the working space at load — swatches then carry
    // native CMYK + spot identity into the display list.
    let opts = CanvasOptions {
        color_profiles: vec![ColorProfileEntry {
            name: "Test CMYK Profile".into(),
            bytes: profile,
        }],
        ..CanvasOptions::default()
    };
    let model = CanvasModel::load("doc1", &small_idml(), opts).expect("load");
    let wire = ExportPdfWireOptions {
        standard: Some("pdfx4".into()),
        output_intent_profile: Some("Test CMYK Profile".into()),
        output_condition: Some("Test Condition".into()),
        ..Default::default()
    };
    let (mut session, pages) = CanvasExportSession::begin(&model, &wire).expect("begin x4");
    for _ in 0..pages {
        session.export_next_page().expect("page");
    }
    let (bytes, _) = session.finish().expect("finish");
    assert!(bytes.starts_with(b"%PDF-1.6"), "X-4 is PDF 1.6");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("GTS_PDFX"), "missing OutputIntent");
    // The spot swatch keeps its plate: /Separation with the swatch
    // name as the colourant.
    assert!(text.contains("/Separation"), "spot ink lost its plate");
    assert!(text.contains("Spot-Orange"), "colourant name missing");
}

#[test]
fn unknown_profile_name_fails_at_begin() {
    let model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    let wire = ExportPdfWireOptions {
        output_intent_profile: Some("No Such Profile".into()),
        ..Default::default()
    };
    let err = match CanvasExportSession::begin(&model, &wire) {
        Err(e) => e,
        Ok(_) => panic!("begin must fail for an unknown profile name"),
    };
    assert!(err.contains("not registered"), "got: {err}");
}

#[test]
fn document_bleed_grows_the_media_box() {
    // The synthetic designmap declares 8.5pt bleed on all sides; the
    // exported MediaBox must be trim + 2x8.5 in each axis.
    let model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    let (mut session, pages) =
        CanvasExportSession::begin(&model, &ExportPdfWireOptions::default()).expect("begin");
    for _ in 0..pages {
        session.export_next_page().expect("page");
    }
    let (bytes, _) = session.finish().expect("finish");
    let text = String::from_utf8_lossy(&bytes);
    // Page is 612x792 (GeometricBounds are TLBR: 0 0 792 612).
    assert!(
        text.contains("/MediaBox [0 0 629 809]"),
        "MediaBox missing the 8.5pt document bleed"
    );
}
