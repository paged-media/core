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

//! A frame of CJK text sets its first baseline where InDesign 20.0.1 does
//! (`tools/indesign-export/cjk-first-baseline-probe`: 30 pt, the first
//! baseline's distance below the frame's top).
//!
//! * Ascent (the default) is not the face's `hhea` ascender for a CJK
//!   face: a static one takes the top of its `d`, a variable one (Noto
//!   Sans JP) one em below the top of its ideographic character face.
//! * Em Box Height is one em above the `BASE` table's ideographic baseline,
//!   or, with no `BASE`, one em centred on the face's 永.
//!
//! Noto Sans JP is in the corpus. Arial Unicode MS and AppleMyungjo, two
//! static CJK faces without a `BASE` table, ship with macOS: their cases
//! run where those files are (macOS, CI's macOS job) and are skipped
//! elsewhere.

use std::io::Write;
use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// The first baseline (pt below the frame's top) of a frame holding
/// `text` in `family` at 30 pt, its first baseline offset `offset` (an
/// IDML `FirstBaselineOffset`, `None` for the default).
fn first_baseline(family: &str, font: &[u8], text: &str, offset: Option<&str>) -> f32 {
    let pref = offset
        .map(|o| format!(r#"<TextFramePreference FirstBaselineOffset="{o}"/>"#))
        .unwrap_or_default();
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    let mut put = |path: &str, body: &str| {
        zip.start_file(path, deflated).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    put(
        "designmap.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<idPkg:Spread src="Spreads/Spread_sp1.xml"/>
<idPkg:Story src="Stories/Story_s1.xml"/>
</Document>"#,
    );
    put(
        "Spreads/Spread_sp1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="sp1">
<Page Self="p1" GeometricBounds="0 0 841.89 595.276"/>
<TextFrame Self="tf1" ParentStory="s1" GeometricBounds="30 40 85 440">{pref}</TextFrame>
</Spread>
</idPkg:Spread>"#
        ),
    );
    put(
        "Stories/Story_s1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0"><Story Self="s1"><ParagraphStyleRange><CharacterStyleRange AppliedFont="{family}" FontStyle="Regular" PointSize="30"><Content>{text}</Content></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>"#
        ),
    );
    let idml = zip.finish().unwrap().into_inner();
    let mut resolver = BytesResolver::new();
    resolver.add_font(family, None, font.to_vec());
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let doc = idml_import::import_idml_doc(&idml).unwrap();
    let built = pipeline::build_document(&doc, &opts).unwrap();
    built.pages[0]
        .list
        .commands
        .iter()
        .find_map(|c| match c {
            DisplayCommand::FillPath { transform, .. } => {
                let [a, b, c2, d, _, ty] = transform.0;
                (b.abs() < 1e-5 && c2.abs() < 1e-5 && a > 0.0 && d < 0.0).then_some(ty - 30.0)
            }
            _ => None,
        })
        .expect("a glyph")
}

/// The face's two first baselines, Ascent and Em Box Height, against
/// InDesign's, to 0.02 pt.
fn assert_as_indesign(family: &str, font: &[u8], text: &str, ascent: f32, em_box: f32) {
    for (offset, want) in [(None, ascent), (Some("EmboxHeight"), em_box)] {
        let got = first_baseline(family, font, text, offset);
        assert!(
            (got - want).abs() < 0.02,
            "{family} {offset:?}: first baseline {got} pt, InDesign {want} pt"
        );
    }
}

#[test]
fn a_variable_cjk_face_sets_its_first_baseline_as_indesign_does() {
    // Noto Sans JP: hhea 1.16 em, but InDesign 24.81 pt (0.827 em, its
    // BASE icft) and its em box 26.4 pt (one em above ideo -120).
    let font = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/NotoSansJP-VF.ttf"),
    )
    .unwrap();
    assert_as_indesign("Noto Sans JP", &font, "漢字かなAx", 24.81, 26.4);
}

#[test]
fn a_static_cjk_face_takes_its_ascent_from_its_d_and_its_em_box_from_its_ideograph() {
    // InDesign: Arial Unicode MS 21.475 pt (its d, 0.716 em; hhea 1.069)
    // and 26.594 pt (an em centred on its 永); AppleMyungjo 21.93 and
    // 25.355 pt.
    for (family, file, ascent, em_box) in [
        ("Arial Unicode MS", "Arial Unicode.ttf", 21.475, 26.594),
        ("AppleMyungjo", "AppleMyungjo.ttf", 21.93, 25.355),
    ] {
        let Ok(font) = std::fs::read(format!("/System/Library/Fonts/Supplemental/{file}")) else {
            eprintln!("skipped: no {file} on this machine");
            continue;
        };
        assert_as_indesign(family, &font, "漢字かなAx", ascent, em_box);
    }
}

#[test]
fn a_latin_face_keeps_its_hhea() {
    // Open Sans: its hhea ascender, 32.065 pt; its em box centred on half
    // its cap height, 25.708 pt.
    let font = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/OpenSans.ttf"),
    )
    .unwrap();
    assert_as_indesign("Open Sans", &font, "Hxg", 32.065, 25.708);
}
