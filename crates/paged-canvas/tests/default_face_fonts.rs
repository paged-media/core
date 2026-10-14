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

//! The default face is no substitute for its own family, and only fonts
//! the text uses are reported.
//!
//! `loadDocument` takes the default face as bytes with no family name, so
//! a document set in that very family (the editor bundles Inter; the
//! `layout` fixture is set in Inter) had every run flagged as substituted
//! and its family reported missing. The two paths also disagreed on where
//! the first baseline sits: a substitute keeps a 0.8 em heuristic, the
//! face itself uses its ascender, as InDesign does. And the fonts list
//! counted every style's font whether any text used it: `layout` sets all
//! its runs in Inter, yet reported the root style's Open Sans as missing.

use std::io::Write;
use std::path::PathBuf;

use paged_canvas::{CanvasModel, CanvasOptions, FontEntry};

fn corpus_font(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn inter() -> Vec<u8> {
    corpus_font("Inter.ttf")
}

/// The `layout` fixture with EVERY run set in Inter: its body already
/// is, and its label lines, which took the root style's Open Sans, name
/// Inter too. The root style still names Open Sans.
fn all_inter_idml() -> Vec<u8> {
    let mut sample = paged_gen::samples::build("layout").expect("sample");
    const PLAIN: &str = r#"<CharacterStyleRange AppliedCharacterStyle="CharacterStyle/$ID/[No character style]"><Content>"#;
    const INTER: &str = r#"<CharacterStyleRange AppliedCharacterStyle="CharacterStyle/$ID/[No character style]"><Properties><AppliedFont type="string">Inter</AppliedFont></Properties><Content>"#;
    let mut patched = 0;
    for (_, xml) in &mut sample.stories {
        let text = String::from_utf8(xml.clone()).expect("utf-8 story");
        if text.contains(PLAIN) {
            patched += 1;
            *xml = text.replace(PLAIN, INTER).into_bytes();
        }
    }
    assert!(patched > 0, "the fixture has runs in the root style's font");
    paged_gen::write_idml(&sample).expect("idml")
}

fn as_default() -> CanvasModel {
    let opts = CanvasOptions {
        fonts: vec![inter()],
        ..CanvasOptions::default()
    };
    CanvasModel::load("default", &all_inter_idml(), opts).expect("load")
}

fn as_registered() -> CanvasModel {
    let opts = CanvasOptions {
        font_registry: vec![FontEntry {
            family: "Inter".to_string(),
            style: None,
            bytes: inter(),
        }],
        ..CanvasOptions::default()
    };
    CanvasModel::load("registered", &all_inter_idml(), opts).expect("load")
}

/// Every laid-out line's (page, story, paragraph, line, baseline).
fn baselines(m: &CanvasModel) -> Vec<(String, String, u32, u32, f32)> {
    m.built()
        .pages
        .iter()
        .flat_map(|p| {
            p.story_layout.iter().map(|l| {
                (
                    l.page_id.as_str().to_string(),
                    l.story_id.clone(),
                    l.paragraph_idx,
                    l.line_idx,
                    l.baseline_y_pt,
                )
            })
        })
        .collect()
}

#[test]
fn the_default_face_lays_out_its_own_family_as_a_registered_face_does() {
    let (d, r) = (as_default(), as_registered());
    let (bd, br) = (baselines(&d), baselines(&r));
    assert!(!bd.is_empty());
    assert_eq!(bd.len(), br.len(), "the same lines");
    let first_diff = bd.iter().zip(&br).find(|(a, b)| (a.4 - b.4).abs() > 1e-3);
    assert!(
        first_diff.is_none(),
        "default face vs registered face, first differing line: {first_diff:?}"
    );
}

#[test]
fn the_default_face_is_not_a_missing_font_for_its_own_family() {
    let m = as_default();
    let inter = m
        .fonts()
        .into_iter()
        .find(|f| f.family == "Inter")
        .expect("the text is set in Inter");
    assert!(!inter.is_missing, "Inter is the default face: {inter:?}");
    let substituted: Vec<String> = m
        .built()
        .diagnostics
        .items
        .iter()
        .map(|d| format!("{d:?}"))
        .filter(|d| d.contains("FontSubstituted") && d.contains("Inter"))
        .collect();
    assert!(substituted.is_empty(), "{substituted:?}");
}

#[test]
fn only_fonts_the_text_uses_are_listed() {
    let m = as_default();
    let fonts = m.fonts();
    let families: Vec<&str> = fonts.iter().map(|f| f.family.as_str()).collect();
    assert_eq!(
        families,
        ["Inter"],
        "the root style names Open Sans but every run is set in Inter: {fonts:?}"
    );
    // The count is the runs set in it.
    let runs: u32 = m
        .scene()
        .stories
        .iter()
        .flat_map(|s| s.story.paragraphs.iter())
        .map(|p| p.runs.len() as u32)
        .sum();
    assert_eq!(fonts[0].reference_count, runs);
}

/// One text frame whose story sets one run per `(family, style)`.
fn styled_runs_idml(runs: &[(&str, Option<&str>)]) -> Vec<u8> {
    let ranges: String = runs
        .iter()
        .map(|(family, style)| {
            let style_attr = style.map_or(String::new(), |s| format!(r#" FontStyle="{s}""#));
            format!(
                r#"<CharacterStyleRange{style_attr}><Properties><AppliedFont type="string">{family}</AppliedFont></Properties><Content>{family} {} </Content></CharacterStyleRange>"#,
                style.unwrap_or("plain")
            )
        })
        .collect();
    let story = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1"><ParagraphStyleRange>{ranges}</ParagraphStyleRange></Story></idPkg:Story>"#
    );
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="72 72 720 540" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(story.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn with_default_face(face: &str, runs: &[(&str, Option<&str>)]) -> CanvasModel {
    let opts = CanvasOptions {
        fonts: vec![corpus_font(face)],
        ..CanvasOptions::default()
    };
    CanvasModel::load("styled", &styled_runs_idml(runs), opts).expect("load")
}

/// The `FontSubstituted` diagnostics' messages.
fn substitutions(m: &CanvasModel) -> Vec<String> {
    m.built()
        .diagnostics
        .items
        .iter()
        .filter(|d| format!("{:?}", d.code).contains("FontSubstituted"))
        .map(|d| d.message.clone())
        .collect()
}

fn is_missing(m: &CanvasModel, family: &str) -> bool {
    m.fonts()
        .into_iter()
        .find(|f| f.family == family)
        .unwrap_or_else(|| panic!("the text is set in {family}"))
        .is_missing
}

/// A STATIC default face is its own family only in the style it has:
/// asked for a weight it lacks, it sets the run in its own weight, which
/// is reported as a substitution like any other stand-in.
#[test]
fn a_static_default_face_is_a_substitute_for_a_weight_it_lacks() {
    let m = with_default_face(
        "Roboto-Regular.ttf",
        &[("Roboto", None), ("Roboto", Some("Bold"))],
    );
    let subs = substitutions(&m);
    assert_eq!(subs.len(), 1, "{subs:?}");
    assert!(subs[0].contains("\"Roboto Bold\""), "{subs:?}");
    assert!(is_missing(&m, "Roboto"), "Roboto Bold is not served");

    let m = with_default_face(
        "Roboto-Regular.ttf",
        &[("Roboto", None), ("Roboto", Some("Regular"))],
    );
    assert!(substitutions(&m).is_empty(), "{:?}", substitutions(&m));
    assert!(!is_missing(&m, "Roboto"), "Regular is the face itself");
}

/// Reported as a substitute, the family's own face still places the first
/// baseline at the family's ascender (the real Bold's), not at the
/// heuristic a foreign stand-in falls back on: the layout is the one the
/// family's face gives.
#[test]
fn a_style_substitute_of_the_default_family_keeps_the_familys_first_baseline() {
    let first_baseline = |style: Option<&str>| {
        let m = with_default_face("Roboto-Regular.ttf", &[("Roboto", style)]);
        baselines(&m).first().expect("a laid-out line").4
    };
    let regular = first_baseline(None);
    let bold = first_baseline(Some("Bold"));
    assert!(
        (regular - bold).abs() < 1e-3,
        "Regular {regular} vs Bold {bold}"
    );
}

/// A VARIABLE default face covers every weight on its `wght` axis, so it
/// stays its own family at any of them.
#[test]
fn a_variable_default_face_is_its_own_family_at_every_weight() {
    let m = with_default_face(
        "Inter.ttf",
        &[
            ("Inter", None),
            ("Inter", Some("Light")),
            ("Inter", Some("Bold")),
            ("Inter", Some("Black")),
        ],
    );
    assert!(substitutions(&m).is_empty(), "{:?}", substitutions(&m));
    assert!(!is_missing(&m, "Inter"));
}
