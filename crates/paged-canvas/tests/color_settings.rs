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

//! Concept 2 — colour settings + colour compute integration tests:
//! the profile registry, `SetColorSettings` (AC-3: switching the
//! CMYK working space visibly changes resolution), the unknown-
//! profile guard, `RequestColorCompute`'s gamut verdict, and the
//! default-path-unchanged invariant.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions, ColorProfileEntry};

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
<idPkg:Graphic src="Resources/Graphic.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Resources/Graphic.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Color Self="Color/cyan" Model="Process" Space="CMYK" ColorValue="100 0 0 0" Name="Cyan"/>
<Color Self="Color/labvivid" Model="Process" Space="LAB" ColorValue="50 85 -90" Name="Lab Vivid"/>
</idPkg:Graphic>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" FillColor="Color/cyan" GeometricBounds="50 50 200 200" ItemTransform="1 0 0 1 0 0"/>
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
fn set_color_settings_switches_the_working_space_and_back() {
    let Some(profile) = find_profile() else {
        eprintln!("color_settings: no CMYK profile available — skipping");
        return;
    };
    let opts = CanvasOptions {
        color_profiles: vec![ColorProfileEntry {
            name: "Test CMYK Profile 2".into(),
            bytes: profile,
        }],
        ..CanvasOptions::default()
    };
    let mut model = CanvasModel::load("doc1", &small_idml(), opts).expect("load");
    // No active profile (the registered name differs from the
    // designmap's) — pure cyan previews via the naive math.
    let naive_hex = model.color_preview("Color/cyan").expect("preview").rgb_hex;

    // Activate the registered profile — resolution must CHANGE
    // (AC-3) and the mutation must repaint every page.
    let outcome = model
        .apply_mutation(&Mutation::SetColorSettings {
            cmyk_profile_name: Some("Test CMYK Profile 2".into()),
            rgb_policy: None,
            intent: None,
            bpc: None,
        })
        .expect("set settings");
    assert!(!outcome.page_ids.is_empty(), "settings change repaints");
    let icc_hex = model.color_preview("Color/cyan").expect("preview").rgb_hex;
    assert_ne!(naive_hex, icc_hex, "ICC working space changes pure cyan");

    // Clearing the name restores the load-time (no-profile) path.
    model
        .apply_mutation(&Mutation::SetColorSettings {
            cmyk_profile_name: None,
            rgb_policy: None,
            intent: None,
            bpc: None,
        })
        .expect("clear settings");
    let back_hex = model.color_preview("Color/cyan").expect("preview").rgb_hex;
    assert_eq!(naive_hex, back_hex, "clearing restores the default path");
}

#[test]
fn designmap_profile_name_activates_a_matching_registered_profile() {
    let Some(profile) = find_profile() else {
        eprintln!("color_settings: no CMYK profile available — skipping");
        return;
    };
    // The fixture's designmap declares CMYKProfile="Test CMYK
    // Profile"; registering bytes under exactly that name activates
    // them at load (no explicit cmyk_icc_profile needed).
    let opts = CanvasOptions {
        color_profiles: vec![ColorProfileEntry {
            name: "Test CMYK Profile".into(),
            bytes: profile,
        }],
        ..CanvasOptions::default()
    };
    let model = CanvasModel::load("doc1", &small_idml(), opts).expect("load");
    let meta = model.document_meta();
    assert_eq!(meta.cmyk_profile_name.as_deref(), Some("Test CMYK Profile"));
    // And the preview takes the ICC path (differs from naive).
    let bare =
        CanvasModel::load("doc2", &small_idml(), CanvasOptions::default()).expect("load");
    assert_ne!(
        model.color_preview("Color/cyan").unwrap().rgb_hex,
        bare.color_preview("Color/cyan").unwrap().rgb_hex,
    );
}

#[test]
fn unknown_profile_name_fails_the_mutation() {
    let mut model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    let err = model
        .apply_mutation(&Mutation::SetColorSettings {
            cmyk_profile_name: Some("No Such Profile".into()),
            rgb_policy: None,
            intent: None,
            bpc: None,
        })
        .expect_err("unknown profile");
    let msg = format!("{err:?}");
    assert!(msg.contains("No Such Profile"), "got: {msg}");
}

#[test]
fn color_compute_resolves_and_flags_gamut() {
    let Some(profile) = find_profile() else {
        eprintln!("color_settings: no CMYK profile available — skipping");
        return;
    };
    let mut model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    // Without a working space nothing is "out of gamut".
    let (_, _, oog) = model.color_compute("LAB", &[50.0, 120.0, -100.0], None, None, None, None);
    assert!(!oog, "no working space => no gamut verdict");

    model.register_color_profile("P".into(), profile);
    model
        .apply_mutation(&Mutation::SetColorSettings {
            cmyk_profile_name: Some("P".into()),
            rgb_policy: None,
            intent: None,
            bpc: None,
        })
        .expect("activate");
    // A screaming Lab green-violet far outside any coated CMYK space.
    let (hex, _, oog) =
        model.color_compute("LAB", &[50.0, 120.0, -100.0], None, None, None, None);
    assert!(oog, "vivid Lab must flag out-of-gamut against coated CMYK");
    assert!(hex.starts_with('#') && hex.len() == 7);
    // A neutral mid grey is comfortably inside.
    let (_, _, oog) = model.color_compute("LAB", &[50.0, 0.0, 0.0], None, None, None, None);
    assert!(!oog, "neutral grey is in gamut");
    // CMYK input is in-gamut by definition; effective CMYK echoes
    // the channels (percent units preserved — the preview-readout
    // regression).
    let (_, cmyk, oog) =
        model.color_compute("CMYK", &[0.0, 50.0, 100.0, 0.0], None, None, None, None);
    assert!(!oog);
    assert_eq!(cmyk, Some([0.0, 50.0, 100.0, 0.0]));
}

#[test]
fn swatch_tint_folds_into_compute() {
    let model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    // 50% tint of pure cyan = CMYK(50,0,0,0) after the swatch-level
    // tint fold.
    let (_, cmyk, _) =
        model.color_compute("CMYK", &[100.0, 0.0, 0.0, 0.0], Some(50.0), None, None, None);
    assert_eq!(cmyk, Some([50.0, 0.0, 0.0, 0.0]));
}
