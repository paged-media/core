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

//! Endnotes inserted, deleted and converted from footnotes over the wire
//! (ADR 174), as InDesign 20.0.1 does it (`endnote-probe`): a new note's
//! range goes after the range of the reference before it and the notes
//! number in reference order; a deleted note takes its range; the first
//! endnote of a document makes the endnote story on a page of its own; a
//! footnote converts to an endnote of its text at its reference's place.
//! Every op undoes exactly.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_gen::samples::endnotes as sample;

fn load(idml: Vec<u8>) -> CanvasModel {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
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

fn model() -> CanvasModel {
    load(paged_gen::write_idml(&sample::build()).expect("idml"))
}

/// `(label, text)` of the document's endnotes, in reference order.
fn notes(m: &CanvasModel) -> Vec<(String, String)> {
    m.endnotes()
        .into_iter()
        .map(|e| (e.label.unwrap_or_default(), e.text))
        .collect()
}

/// The endnote story's text.
fn endnote_story(m: &CanvasModel) -> Option<String> {
    m.scene()
        .stories
        .iter()
        .find(|s| s.story.is_endnote_story)
        .map(|s| {
            s.story
                .paragraphs
                .iter()
                .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        })
}

fn s(v: &str) -> String {
    v.to_string()
}

#[test]
fn the_endnotes_read_in_reference_order_with_their_numbers() {
    let m = model();
    assert_eq!(
        notes(&m),
        [
            (s("iii"), s("The first endnote.")),
            (s("iv"), s("The second endnote, a little longer.")),
            (s("v"), s("The third endnote.")),
        ]
    );
}

#[test]
fn an_inserted_endnote_takes_its_place_in_the_order_and_undoes() {
    let mut m = model();
    let before = notes(&m);
    let story_before = endnote_story(&m);
    // In the second claim, before its reference.
    m.apply_mutation(&Mutation::InsertEndnote {
        story_id: sample::story_id("Body"),
        offset: 40,
        paragraph: None,
        text: "Inserted between.".into(),
    })
    .expect("insert");
    assert_eq!(
        notes(&m),
        [
            (s("iii"), s("The first endnote.")),
            (s("iv"), s("Inserted between.")),
            (s("v"), s("The second endnote, a little longer.")),
            (s("vi"), s("The third endnote.")),
        ]
    );
    // Its range stands second in the endnote story.
    let story = endnote_story(&m).unwrap();
    let first = story.find("The first endnote.").unwrap();
    let new = story.find("Inserted between.").unwrap();
    let second = story.find("The second endnote").unwrap();
    assert!(first < new && new < second, "{story:?}");
    m.undo().expect("undo");
    assert_eq!(notes(&m), before);
    assert_eq!(endnote_story(&m), story_before);
}

#[test]
fn a_deleted_endnote_takes_its_range_and_undo_puts_it_back() {
    let mut m = model();
    let before = notes(&m);
    let story_before = endnote_story(&m);
    let id = m.endnotes()[1].id.clone();
    m.apply_mutation(&Mutation::DeleteEndnote { endnote_id: id })
        .expect("delete");
    assert_eq!(
        notes(&m),
        [
            (s("iii"), s("The first endnote.")),
            (s("iv"), s("The third endnote.")),
        ]
    );
    assert!(!endnote_story(&m).unwrap().contains("second"));
    m.undo().expect("undo");
    assert_eq!(notes(&m), before);
    assert_eq!(endnote_story(&m), story_before);
}

#[test]
fn the_first_endnote_makes_the_endnote_story_on_a_page_of_its_own() {
    let mut m = load(paged_gen::write_idml(&paged_gen::samples::notes::build()).expect("idml"));
    let pages = m.page_count();
    assert!(endnote_story(&m).is_none());
    m.apply_mutation(&Mutation::InsertEndnote {
        story_id: paged_gen::samples::notes::story_id(),
        offset: 4,
        paragraph: None,
        text: "A first endnote.".into(),
    })
    .expect("insert");
    assert_eq!(m.page_count(), pages + 1, "a page at the end");
    assert_eq!(
        endnote_story(&m).as_deref(),
        Some("Endnotes\n\u{FEFF}\u{E01E}\tA first endnote.\u{FEFF}")
    );
    assert_eq!(notes(&m), [(s("1"), s("A first endnote."))]);
    // Its frame is the endnote frame, on the new page.
    let last = m.scene().spreads.last().unwrap();
    assert_eq!(last.spread.endnote_frames.len(), 1);
    assert_eq!(last.spread.text_frames.len(), 1);
    m.undo().expect("undo");
    assert_eq!(m.page_count(), pages);
    assert!(endnote_story(&m).is_none());
    assert!(m.endnotes().is_empty());
}

#[test]
fn a_footnote_converts_to_an_endnote_of_its_text_and_undoes() {
    use paged_gen::samples::footnote_numbering as fsample;
    let mut m = load(paged_gen::write_idml(&fsample::build(&fsample::NUMBERING)).expect("idml"));
    let story = fsample::story_id("footnote-numbering", "A");
    let footnotes = m.footnotes().len();
    let note = m
        .footnotes()
        .into_iter()
        .find(|f| f.story_id == story)
        .expect("a footnote");
    m.apply_mutation(&Mutation::ConvertFootnoteToEndnote {
        story_id: story.clone(),
        footnote_id: note.self_id.clone(),
    })
    .expect("convert");
    assert_eq!(m.footnotes().len(), footnotes - 1);
    let endnotes = m.endnotes();
    assert_eq!(endnotes.len(), 1);
    assert_eq!(endnotes[0].text, note.text);
    assert_eq!(endnotes[0].offset, note.offset);
    m.undo().expect("undo");
    assert_eq!(m.footnotes().len(), footnotes);
    assert!(m.endnotes().is_empty());
    assert!(endnote_story(&m).is_none());
}

/// What an IDML save reads back as: the endnotes' texts in reference
/// order, and how many endnote frames the spreads hold.
fn saved(m: &CanvasModel) -> (Vec<String>, usize) {
    let bytes = m.export_idml().expect("export");
    let back = load(bytes);
    let frames = back
        .scene()
        .spreads
        .iter()
        .map(|s| s.spread.endnote_frames.len())
        .sum();
    (
        back.endnotes().into_iter().map(|e| e.text).collect(),
        frames,
    )
}

#[test]
fn inserted_and_deleted_endnotes_save_as_idml() {
    let mut m = model();
    m.apply_mutation(&Mutation::InsertEndnote {
        story_id: sample::story_id("Body"),
        offset: 40,
        paragraph: None,
        text: "Inserted between.".into(),
    })
    .expect("insert");
    let id = m.endnotes()[3].id.clone();
    m.apply_mutation(&Mutation::DeleteEndnote { endnote_id: id })
        .expect("delete");
    assert_eq!(
        saved(&m),
        (
            vec![
                s("The first endnote."),
                s("Inserted between."),
                s("The second endnote, a little longer.")
            ],
            1
        )
    );
}

#[test]
fn a_first_endnote_saves_its_story_and_frame() {
    let mut m = load(paged_gen::write_idml(&paged_gen::samples::notes::build()).expect("idml"));
    m.apply_mutation(&Mutation::InsertEndnote {
        story_id: paged_gen::samples::notes::story_id(),
        offset: 4,
        paragraph: None,
        text: "A first endnote.".into(),
    })
    .expect("insert");
    assert_eq!(saved(&m), (vec![s("A first endnote.")], 1));
}

/// The endnotes saved for InDesign to read back
/// (`PROBE_IN=<path> bash tools/indesign-export/endnote-probe.sh`):
/// `PAGED_ENDNOTES_IDML=<dir> cargo test -p paged-canvas --test
/// endnote_ops -- --ignored` writes `inserted.idml` (the endnotes sample
/// with a note inserted and one deleted) and `first.idml` (a first endnote
/// in a document that had none).
#[test]
#[ignore]
fn writes_the_saved_endnotes_for_indesign() {
    let dir = std::path::PathBuf::from(std::env::var("PAGED_ENDNOTES_IDML").expect("dir"));
    let mut m = model();
    m.apply_mutation(&Mutation::InsertEndnote {
        story_id: sample::story_id("Body"),
        offset: 40,
        paragraph: None,
        text: "Inserted between.".into(),
    })
    .expect("insert");
    let id = m.endnotes()[3].id.clone();
    m.apply_mutation(&Mutation::DeleteEndnote { endnote_id: id })
        .expect("delete");
    std::fs::write(dir.join("inserted.idml"), m.export_idml().expect("export")).expect("write");
    let mut m = load(paged_gen::write_idml(&paged_gen::samples::notes::build()).expect("idml"));
    m.apply_mutation(&Mutation::InsertEndnote {
        story_id: paged_gen::samples::notes::story_id(),
        offset: 4,
        paragraph: None,
        text: "A first endnote.".into(),
    })
    .expect("insert");
    std::fs::write(dir.join("first.idml"), m.export_idml().expect("export")).expect("write");
}
