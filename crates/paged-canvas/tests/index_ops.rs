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

//! Generate Index (ADR 168) on `index`: the story on page 4 prints its
//! stale index until the op writes what InDesign 20.0.1 writes on Generate
//! Index, recorded by `index-probe` in
//! `tools/indesign-export/index-probe.json`. The probe host runs InDesign
//! in German: its style names and the reference word are mapped back to
//! the language-neutral `$ID/` names and the English word the engine
//! writes.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_gen::samples::index;

fn model() -> CanvasModel {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    let idml = paged_gen::write_idml(&index::build()).expect("idml");
    CanvasModel::load(
        "doc",
        &idml,
        CanvasOptions {
            fonts: vec![font],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

/// `(paragraph style, [(character style, text)])`, styles by `Self` id.
type Para = (Option<String>, Vec<(Option<String>, String)>);

fn paragraphs(paragraphs: &[paged_model::Paragraph]) -> Vec<Para> {
    paragraphs
        .iter()
        .map(|p| {
            (
                p.paragraph_style.clone(),
                p.runs
                    .iter()
                    .map(|r| (r.character_style.clone(), r.text.clone()))
                    .collect(),
            )
        })
        .collect()
}

fn index_story(m: &CanvasModel) -> Vec<Para> {
    let id = index::story_id("Index");
    paragraphs(
        &m.scene()
            .stories
            .iter()
            .find(|s| s.self_id == id)
            .expect("the index story")
            .story
            .paragraphs,
    )
}

/// InDesign's record, its German names mapped back.
fn indesign() -> Vec<Para> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/indesign-export/index-probe.json");
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("probe record")).expect("json");
    let paragraph_style = |name: &str| {
        Some(
            match name {
                "Indextitel" => "ParagraphStyle/$ID/Index Title",
                "Indexabschnittskopf" => "ParagraphStyle/$ID/Index Section Head",
                "Indexstufe 1" => "ParagraphStyle/$ID/Index Level 1",
                "Indexstufe 2" => "ParagraphStyle/$ID/Index Level 2",
                other => panic!("an index style the test does not map: {other}"),
            }
            .to_string(),
        )
    };
    let character_style = |name: &str| match name {
        "Indexquerverweis" => Some("CharacterStyle/$ID/Index Cross-reference".to_string()),
        n if n.starts_with('[') => None,
        other => panic!("a character style the test does not map: {other}"),
    };
    json["index"]
        .as_array()
        .expect("index")
        .iter()
        .map(|p| {
            let runs = p["ranges"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    let text = r[1]
                        .as_str()
                        .unwrap()
                        .replace("<U+000D>", "")
                        .replace("<U+00A0>", "\u{00A0}")
                        .replace("Siehe", "See");
                    (character_style(r[0].as_str().unwrap()), text)
                })
                .filter(|(_, t)| !t.is_empty())
                .collect();
            (paragraph_style(p["style"].as_str().unwrap()), runs)
        })
        .collect()
}

#[test]
fn a_stale_index_prints_as_stored_until_generated() {
    let m = model();
    let texts: Vec<String> = index_story(&m)
        .into_iter()
        .map(|(_, runs)| runs.into_iter().map(|(_, t)| t).collect())
        .collect();
    assert_eq!(texts, index::STALE);
}

#[test]
fn generate_index_writes_what_indesign_writes_and_undoes() {
    let mut m = model();
    let stale = index_story(&m);
    m.apply_mutation(&Mutation::GenerateIndex {
        story_id: index::story_id("Index"),
    })
    .expect("generate");
    let expected = indesign();
    assert_eq!(expected.len(), 12, "the probe's record: {expected:?}");
    assert_eq!(index_story(&m), expected);

    // The index styles InDesign creates, created with its attributes.
    let styles = &m.scene().styles;
    let level1 = &styles.paragraph_styles["ParagraphStyle/$ID/Index Level 1"];
    assert_eq!(level1.left_indent, Some(28.0));
    assert_eq!(level1.first_line_indent, Some(-28.0));
    let title = &styles.paragraph_styles["ParagraphStyle/$ID/Index Title"];
    assert_eq!(title.point_size, Some(30.0));
    let xref = &styles.character_styles["CharacterStyle/$ID/Index Cross-reference"];
    assert_eq!(xref.font_style.as_deref(), Some("Italic"));

    // One step: undo takes the styles back off as well.
    m.undo().expect("undo");
    assert_eq!(index_story(&m), stale);
    assert!(!m
        .scene()
        .styles
        .paragraph_styles
        .contains_key("ParagraphStyle/$ID/Index Title"));
    m.redo().expect("redo");
    assert_eq!(index_story(&m), expected);
}

#[test]
fn a_generated_index_saves_as_idml() {
    let mut m = model();
    m.apply_mutation(&Mutation::GenerateIndex {
        story_id: index::story_id("Index"),
    })
    .expect("generate");
    let saved = m.export_idml().expect("export");
    let doc = idml_import::import_idml_doc(&saved).expect("re-import");
    let id = index::story_id("Index");
    let story = &doc.stories.iter().find(|s| s.self_id == id).unwrap().story;
    assert_eq!(paragraphs(&story.paragraphs), indesign());
    for style in [
        "ParagraphStyle/$ID/Index Title",
        "ParagraphStyle/$ID/Index Section Head",
        "ParagraphStyle/$ID/Index Level 1",
        "ParagraphStyle/$ID/Index Level 2",
    ] {
        assert!(
            doc.styles.paragraph_styles.contains_key(style),
            "{style} saved"
        );
    }
    assert!(doc
        .styles
        .character_styles
        .contains_key("CharacterStyle/$ID/Index Cross-reference"));
}

#[test]
fn generating_into_an_unknown_story_is_refused() {
    let mut m = model();
    assert!(m
        .apply_mutation(&Mutation::GenerateIndex {
            story_id: "Story/none".into(),
        })
        .is_err());
}
