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

//! Concept 2 — `.ase` import/export + Ink Manager state:
//! ImportSwatchLibrary as ONE undoable operation, export_ase
//! round-trip, ink settings (AC-8: convert-to-process never edits
//! the swatch), and the standard-Lab-for-spots preview preference.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use paged_color::ase::{AseEntry, AseGroup, AseKind, AseLibrary, AseSpace};

fn small_idml() -> Vec<u8> {
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
<Color Self="Color/cyan" Model="Process" Space="CMYK" ColorValue="100 0 0 0" Name="Cyan"/>
<Color Self="Color/pms" Model="Spot" Space="LAB" ColorValue="48 64 47" AlternateSpace="CMYK" AlternateColorValue="0 91 76 0" Name="PANTONE Warm Red C"/>
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

fn hlc_sample_ase() -> Vec<u8> {
    paged_color::ase::write_ase(&AseLibrary {
        groups: vec![AseGroup {
            name: "HLC Colour Atlas".into(),
            entries: vec![
                AseEntry {
                    name: "HLC H010_L20_C010".into(),
                    space: AseSpace::Lab,
                    value: vec![20.0, 9.848, 1.736],
                    kind: AseKind::Global,
                },
                AseEntry {
                    name: "HLC H050_L70_C040".into(),
                    space: AseSpace::Lab,
                    value: vec![70.0, 25.71, 30.64],
                    kind: AseKind::Global,
                },
            ],
        }],
        loose: vec![AseEntry {
            name: "Loose CMYK".into(),
            space: AseSpace::Cmyk,
            value: vec![10.0, 20.0, 30.0, 0.0],
            kind: AseKind::Process,
        }],
    })
}

#[test]
fn import_swatch_library_is_one_undoable_operation() {
    let mut model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    let swatches_before = model.swatches().len();
    let groups_before = model.color_groups().len();
    let log_before = model.applied_log_len();

    model
        .apply_mutation(&Mutation::ImportSwatchLibrary {
            bytes: hlc_sample_ase().into(),
            group_name: Some("Imported".into()),
        })
        .expect("import");
    // 3 colours + the HLC group + the loose-entries group.
    assert_eq!(model.swatches().len(), swatches_before + 3);
    assert_eq!(model.color_groups().len(), groups_before + 2);
    assert_eq!(model.applied_log_len(), log_before + 1, "ONE log entry");
    // Names preserved verbatim (HLC name = provenance).
    assert!(model
        .swatches()
        .iter()
        .any(|s| s.name == "HLC H010_L20_C010"));
    // The HLC Lab entries preview through the analytic Lab path.
    let hlc = model
        .swatches()
        .into_iter()
        .find(|s| s.name == "HLC H010_L20_C010")
        .unwrap();
    let preview = model.color_preview(&hlc.self_id).expect("preview");
    assert_ne!(preview.rgb_hex, "#808080", "Lab resolves, not grey");

    // ONE undo removes the whole import.
    model.undo().expect("undo");
    assert_eq!(model.swatches().len(), swatches_before);
    assert_eq!(model.color_groups().len(), groups_before);
    // Redo restores everything with the same ids.
    model.redo().expect("redo");
    assert_eq!(model.swatches().len(), swatches_before + 3);
    assert_eq!(model.color_groups().len(), groups_before + 2);
}

#[test]
fn export_ase_round_trips_through_the_parser() {
    let mut model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    model
        .apply_mutation(&Mutation::ImportSwatchLibrary {
            bytes: hlc_sample_ase().into(),
            group_name: None,
        })
        .expect("import");
    let bytes = model.export_ase(None);
    let lib = paged_color::ase::parse_ase(&bytes).expect("parse");
    // The HLC group survives with its entries; the fixture's own
    // colours (cyan process + the PANTONE spot) export too.
    let group_names: Vec<&str> = lib.groups.iter().map(|g| g.name.as_str()).collect();
    assert!(group_names.contains(&"HLC Colour Atlas"), "{group_names:?}");
    let all_names: Vec<String> = lib
        .groups
        .iter()
        .flat_map(|g| g.entries.iter().map(|e| e.name.clone()))
        .chain(lib.loose.iter().map(|e| e.name.clone()))
        .collect();
    assert!(all_names.iter().any(|n| n == "HLC H010_L20_C010"));
    assert!(all_names.iter().any(|n| n == "Cyan"));
    let pms = lib
        .groups
        .iter()
        .flat_map(|g| g.entries.iter())
        .chain(lib.loose.iter())
        .find(|e| e.name == "PANTONE Warm Red C")
        .expect("spot exported");
    assert_eq!(pms.kind, AseKind::Spot);
    assert_eq!(pms.space, AseSpace::Lab);
}

#[test]
fn ink_settings_never_touch_the_swatch_identity() {
    let mut model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    // One spot in the fixture => one ink row, defaults off.
    let inks = model.inks();
    assert_eq!(inks.len(), 1);
    assert_eq!(inks[0].spot_id, "Color/pms");
    assert!(!inks[0].convert_to_process);

    model
        .apply_mutation(&Mutation::SetInkSetting {
            spot_id: "Color/pms".into(),
            convert_to_process: true,
            alias_to: None,
        })
        .expect("set ink");
    let inks = model.inks();
    assert!(inks[0].convert_to_process);
    // AC-8 — the swatch itself is untouched: still a Spot, same
    // name, same alternate.
    let preview = model.color_preview("Color/pms").expect("preview");
    assert_eq!(preview.model, "spot");
    assert_eq!(preview.name, "PANTONE Warm Red C");

    // Non-spot targets are rejected.
    let err = model
        .apply_mutation(&Mutation::SetInkSetting {
            spot_id: "Color/cyan".into(),
            convert_to_process: true,
            alias_to: None,
        })
        .expect_err("cyan is process");
    assert!(format!("{err:?}").contains("not a spot"));
}

#[test]
fn standard_lab_for_spots_prefers_the_lab_primary() {
    let mut model =
        CanvasModel::load("doc1", &small_idml(), CanvasOptions::default()).expect("load");
    let via_alternate = model.color_preview("Color/pms").expect("preview").rgb_hex;
    model
        .apply_mutation(&Mutation::SetUseStandardLabForSpots { enabled: true })
        .expect("toggle");
    assert_eq!(model.document_meta().use_standard_lab_for_spots, Some(true));
    let via_lab = model.color_preview("Color/pms").expect("preview").rgb_hex;
    assert_ne!(
        via_alternate, via_lab,
        "Lab primary resolves differently from the naive CMYK alternate"
    );
    // Toggle back restores the alternate-based preview.
    model
        .apply_mutation(&Mutation::SetUseStandardLabForSpots { enabled: false })
        .expect("toggle off");
    assert_eq!(
        model.color_preview("Color/pms").expect("preview").rgb_hex,
        via_alternate
    );
}
