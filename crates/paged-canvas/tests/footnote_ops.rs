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

//! Footnotes inserted and deleted over the wire (ADR 163), on
//! `footnote-numbering` (stories A, B and C; every paragraph carries one
//! note): a note inserted mid-story numbers in its place and undoes; a
//! note deleted takes its reference with it and undo puts both back as
//! they were; a text delete that covers a reference takes its note too.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_gen::samples::footnote_numbering as sample;

const MARK: char = paged_model::FOOTNOTE_REFERENCE_MARKER;

fn model() -> CanvasModel {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    let idml = paged_gen::write_idml(&sample::build(&sample::NUMBERING)).expect("idml");
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

fn story(name: &str) -> String {
    sample::story_id("footnote-numbering", name)
}

/// `(id, label, text)` of a story's notes, in story order.
fn notes(m: &CanvasModel, story_id: &str) -> Vec<(String, String, String)> {
    m.footnotes()
        .into_iter()
        .filter(|f| f.story_id == story_id)
        .map(|f| (f.self_id, f.label.unwrap_or_default(), f.text))
        .collect()
}

/// The story's text as the text ops address it: paragraphs joined by `\n`.
fn text(m: &CanvasModel, story_id: &str) -> String {
    m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == story_id)
        .unwrap()
        .story
        .paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_note_inserted_mid_story_numbers_in_its_place_and_undoes() {
    let mut m = model();
    let b = story("B");
    let before = notes(&m, &b);
    assert_eq!(before.len(), 2);
    // After "B1 The" in B's first paragraph (contiguous char offset 6).
    m.apply_mutation(&Mutation::InsertFootnote {
        story_id: b.clone(),
        offset: 6,
        paragraph: None,
        text: "An inserted note.".into(),
    })
    .expect("insert");
    let after = notes(&m, &b);
    assert_eq!(after.len(), 3, "{after:?}");
    assert_eq!(after[0].2, "An inserted note.");
    assert_eq!(
        after.iter().map(|n| n.1.as_str()).collect::<Vec<_>>(),
        ["[1]", "[2]", "[3]"],
        "it takes number 1 and the others follow"
    );
    assert!(text(&m, &b).starts_with(&format!("B1 The{MARK}")));
    m.undo().expect("undo");
    assert_eq!(notes(&m, &b), before);
    assert!(!text(&m, &b).starts_with(&format!("B1 The{MARK}")));
}

#[test]
fn a_deleted_note_takes_its_reference_and_undo_puts_both_back() {
    let mut m = model();
    let a = story("A");
    let before = notes(&m, &a);
    let text_before = text(&m, &a);
    let (id, ..) = before[1].clone();
    m.apply_mutation(&Mutation::DeleteFootnote {
        story_id: a.clone(),
        footnote_id: id.clone(),
    })
    .expect("delete");
    let after = notes(&m, &a);
    assert_eq!(after.len(), before.len() - 1);
    assert!(after.iter().all(|n| n.0 != id));
    assert_eq!(
        text(&m, &a).matches(MARK).count(),
        text_before.matches(MARK).count() - 1
    );
    m.undo().expect("undo");
    assert_eq!(notes(&m, &a), before);
    assert_eq!(text(&m, &a), text_before);
}

#[test]
fn a_text_delete_over_a_reference_takes_its_note() {
    let mut m = model();
    let c = story("C");
    let before = notes(&m, &c);
    let t = text(&m, &c);
    // The first reference marker and the character before it.
    let at = t.find(MARK).expect("a reference") as u32;
    m.apply_mutation(&Mutation::DeleteRange {
        story_id: c.clone(),
        start: at - 1,
        end: at + MARK.len_utf8() as u32,
        cell: None,
    })
    .expect("delete range");
    let after = notes(&m, &c);
    assert_eq!(after.len(), before.len() - 1, "{after:?}");
    assert_eq!(after[0].0, before[1].0);
    m.undo().expect("undo");
    assert_eq!(notes(&m, &c), before);
    assert_eq!(text(&m, &c), t);
}

/// What an IDML save of the edited document reads back as: each story's
/// notes' texts.
fn saved_notes(m: &CanvasModel, story_id: &str) -> Vec<String> {
    let bytes = m.export_idml().expect("export");
    let doc = idml_import::import_idml_doc(&bytes).expect("re-import");
    doc.stories
        .iter()
        .find(|s| s.self_id == story_id)
        .unwrap()
        .story
        .paragraphs
        .iter()
        .flat_map(|p| &p.footnotes)
        .map(|f| {
            f.paragraphs
                .iter()
                .flat_map(|p| &p.runs)
                .map(|r| r.text.as_str())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn an_inserted_and_a_deleted_note_save_as_idml() {
    let mut m = model();
    let b = story("B");
    let a = story("A");
    m.apply_mutation(&Mutation::InsertFootnote {
        story_id: b.clone(),
        offset: 6,
        paragraph: None,
        text: "An inserted note.".into(),
    })
    .expect("insert");
    let (gone, ..) = notes(&m, &a)[1].clone();
    m.apply_mutation(&Mutation::DeleteFootnote {
        story_id: a.clone(),
        footnote_id: gone,
    })
    .expect("delete");
    let b_saved = saved_notes(&m, &b);
    assert_eq!(b_saved.len(), 3, "{b_saved:?}");
    assert!(b_saved[0].ends_with("An inserted note."), "{b_saved:?}");
    assert_eq!(saved_notes(&m, &a).len(), 3);
}
