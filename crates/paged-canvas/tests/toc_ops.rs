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

//! Update Table of Contents (ADR 165) on `toc`, whose TOC story is out of
//! date. Until the update it prints its stored text; the update writes
//! what InDesign 20.0.1 writes, recorded by `toc-update-probe` in
//! `tools/indesign-export/toc-update-probe.json`.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_gen::samples::toc;

fn model_of(idml: &[u8]) -> CanvasModel {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    CanvasModel::load(
        "doc",
        idml,
        CanvasOptions {
            fonts: vec![font],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

fn model() -> CanvasModel {
    model_of(&paged_gen::write_idml(&toc::build()).expect("idml"))
}

/// A paragraph as the probe records one: its style's name and its runs as
/// `(character style name, text)`, empty for no character style.
type Para = (String, Vec<(String, String)>);

fn style_name(id: Option<&str>) -> String {
    id.and_then(|s| s.rsplit('/').next())
        .filter(|s| !s.starts_with('['))
        .unwrap_or("")
        .to_string()
}

fn paragraphs(paragraphs: &[paged_model::Paragraph]) -> Vec<Para> {
    paragraphs
        .iter()
        .map(|p| {
            let mut runs: Vec<(String, String)> = Vec::new();
            for r in &p.runs {
                let style = style_name(r.character_style.as_deref());
                match runs.last_mut() {
                    Some(last) if last.0 == style => last.1.push_str(&r.text),
                    _ => runs.push((style, r.text.clone())),
                }
            }
            (style_name(p.paragraph_style.as_deref()), runs)
        })
        .collect()
}

fn toc_story(m: &CanvasModel) -> Vec<Para> {
    let id = toc::story_id("Contents");
    paragraphs(
        &m.scene()
            .stories
            .iter()
            .find(|s| s.self_id == id)
            .expect("the TOC story")
            .story
            .paragraphs,
    )
}

/// What InDesign wrote on Update, from the probe's record. Its ranges cross
/// paragraph ends (`\r`), so the text is re-split there.
fn indesign_after() -> Vec<Para> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/indesign-export/toc-update-probe.json");
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("probe record")).expect("json");
    let decode = |s: &str| s.replace("<U+0009>", "\t");
    let none = |s: &str| {
        if s.starts_with('[') {
            String::new()
        } else {
            s.to_string()
        }
    };
    json["after"]
        .as_array()
        .expect("after")
        .iter()
        .map(|p| {
            let mut runs: Vec<(String, String)> = Vec::new();
            for r in p["ranges"].as_array().unwrap() {
                let style = none(r[0].as_str().unwrap());
                // The paragraph's own text ends at its return.
                let text = decode(r[1].as_str().unwrap());
                let text = text.split("<U+000D>").next().unwrap_or("").to_string();
                if text.is_empty() {
                    continue;
                }
                match runs.last_mut() {
                    Some(last) if last.0 == style => last.1.push_str(&text),
                    _ => runs.push((style, text)),
                }
            }
            (p["style"].as_str().unwrap().to_string(), runs)
        })
        .collect()
}

fn stale() -> Vec<Para> {
    toc::STALE
        .iter()
        .map(|(style, ranges)| {
            (
                style.to_string(),
                ranges
                    .iter()
                    .map(|(c, t)| (c.to_string(), t.to_string()))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn a_stale_toc_prints_its_stored_entries_until_updated() {
    let m = model();
    assert_eq!(toc_story(&m), stale());
}

#[test]
fn the_update_writes_what_indesign_writes_and_undoes() {
    let mut m = model();
    m.apply_mutation(&Mutation::UpdateTableOfContents {
        story_id: toc::story_id("Contents"),
    })
    .expect("update");
    let expected = indesign_after();
    assert_eq!(expected.len(), 6, "the probe's record: {expected:?}");
    assert_eq!(toc_story(&m), expected);

    m.undo().expect("undo");
    assert_eq!(toc_story(&m), stale());
    m.redo().expect("redo");
    assert_eq!(toc_story(&m), expected);
}

/// In one batch with an edit that renames a heading, the update sees the
/// edit: the layout a batch owes is settled before the entries are read.
#[test]
fn an_update_batched_after_an_edit_lists_the_edit() {
    let mut m = model();
    let body = toc::story_id("Body");
    // "Going Further" opens paragraph 4; replace "Further" (6..13 of it).
    // The text ops address the story's text with a `\n` per paragraph end.
    let start: u32 = toc::BODY[..4]
        .iter()
        .map(|(_, t)| t.trim_start_matches('>').chars().count() as u32 + 1)
        .sum();
    m.apply_mutation(&Mutation::Batch {
        ops: vec![
            Mutation::DeleteRange {
                story_id: body.clone(),
                start: start + 6,
                end: start + 13,
                cell: None,
            },
            Mutation::InsertText {
                story_id: body,
                offset: start + 6,
                text: "Beyond".into(),
                cell: None,
            },
            Mutation::UpdateTableOfContents {
                story_id: toc::story_id("Contents"),
            },
        ],
    })
    .expect("batch");
    let entries = toc_story(&m);
    assert_eq!(
        entries[3].1[0].1, "Going Beyond\t",
        "the renamed heading: {entries:?}"
    );
}

#[test]
fn an_updated_toc_saves_as_idml() {
    let mut m = model();
    m.apply_mutation(&Mutation::UpdateTableOfContents {
        story_id: toc::story_id("Contents"),
    })
    .expect("update");
    let saved = m.export_idml().expect("export");
    let doc = idml_import::import_idml_doc(&saved).expect("re-import");
    let id = toc::story_id("Contents");
    let story = &doc.stories.iter().find(|s| s.self_id == id).unwrap().story;
    assert_eq!(story.applied_toc_style.as_deref(), Some(toc::TOC_STYLE));
    assert_eq!(paragraphs(&story.paragraphs), indesign_after());
}

#[test]
fn a_story_that_is_not_a_toc_is_refused() {
    let mut m = model();
    assert!(m
        .apply_mutation(&Mutation::UpdateTableOfContents {
            story_id: toc::story_id("Body"),
        })
        .is_err());
}
