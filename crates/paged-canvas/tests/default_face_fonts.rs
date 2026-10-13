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

use std::path::PathBuf;

use paged_canvas::{CanvasModel, CanvasOptions, FontEntry};

fn inter() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
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
