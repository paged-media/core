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

//! Editorial notes over the wire (ADR 172), on `notes` (one story, two
//! paragraphs, four notes as InDesign 20.0.1 writes them): the notes read
//! as InDesign reads them (`notes-probe` with `PROBE_IN`); a note
//! inserted, edited or deleted undoes exactly and saves as IDML; Convert
//! to Text turns a note into the story's text as InDesign converts one; a
//! text delete over a note's place takes the note with it.

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_gen::samples::notes as sample;

const NOTE: char = paged_model::NOTE_MARKER;

fn model() -> CanvasModel {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    let idml = paged_gen::write_idml(&sample::build()).expect("idml");
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

/// `(id, offset, text)` of the document's notes, in story order.
fn notes(m: &CanvasModel) -> Vec<(String, u32, String)> {
    m.notes()
        .into_iter()
        .map(|n| (n.id, n.offset, n.text))
        .collect()
}

/// The story's text as the text ops address it: paragraphs joined by `\n`.
fn text(m: &CanvasModel) -> String {
    m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == sample::story_id())
        .unwrap()
        .story
        .paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_notes_read_as_indesign_reads_them() {
    let m = model();
    let read = m.notes();
    assert_eq!(
        read.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
        [
            "Check this claim.",
            "A remark between words.",
            "First line of a note.\rIts second paragraph.",
            "Last one."
        ]
    );
    // InDesign's story offsets: the story's paragraph break stands inside
    // one range (`<Br/>`), a character of its text as InDesign counts it.
    assert_eq!(
        read.iter().map(|n| n.offset).collect::<Vec<_>>(),
        [15, 26, 57, 74]
    );
    assert!(read
        .iter()
        .all(|n| n.user_name.as_deref() == Some("Unbekannter Benutzername")));
    assert_eq!(
        read[0].creation_date.as_deref(),
        Some("2026-10-16T10:16:46")
    );
    assert_eq!(
        text(&m),
        format!("Text with a not{NOTE}e here and{NOTE} more text.\nA second paragraph{NOTE} ends the story.{NOTE}")
    );
    // Ids are the document's.
    let mut ids: Vec<_> = read.iter().map(|n| n.id.clone()).collect();
    ids.dedup();
    assert_eq!(ids.len(), 4);
}

#[test]
fn an_inserted_note_stands_in_its_place_and_undoes() {
    let mut m = model();
    let before = notes(&m);
    let text_before = text(&m);
    m.apply_mutation(&Mutation::InsertNote {
        story_id: sample::story_id(),
        offset: 4,
        paragraph: None,
        text: "Inserted.".into(),
        user: Some("Reviewer".into()),
        date: Some("2026-10-17T09:00:00".into()),
    })
    .expect("insert");
    let after = m.notes();
    assert_eq!(after.len(), 5);
    assert_eq!((after[0].offset, after[0].text.as_str()), (4, "Inserted."));
    assert_eq!(after[0].user_name.as_deref(), Some("Reviewer"));
    assert_eq!(
        after[0].creation_date.as_deref(),
        Some("2026-10-17T09:00:00")
    );
    // Every later note moved on by its one character.
    assert_eq!(after[1].offset, 16);
    assert!(text(&m).starts_with(&format!("Text{NOTE} with")));
    m.undo().expect("undo");
    assert_eq!(notes(&m), before);
    assert_eq!(text(&m), text_before);
}

#[test]
fn an_edited_note_keeps_its_date_unless_given_one_and_undoes() {
    let mut m = model();
    let before = m.notes();
    let id = before[1].id.clone();
    m.apply_mutation(&Mutation::EditNote {
        note_id: id.clone(),
        text: "A sharper remark.".into(),
        date: None,
    })
    .expect("edit");
    let after = m.notes();
    assert_eq!(after[1].text, "A sharper remark.");
    assert_eq!(after[1].modification_date, before[1].modification_date);
    m.apply_mutation(&Mutation::EditNote {
        note_id: id,
        text: "Sharper still.".into(),
        date: Some("2026-10-17T10:00:00".into()),
    })
    .expect("edit with a date");
    assert_eq!(
        m.notes()[1].modification_date.as_deref(),
        Some("2026-10-17T10:00:00")
    );
    m.undo().expect("undo");
    m.undo().expect("undo");
    assert_eq!(notes(&m), notes_of(&before));
    assert_eq!(m.notes()[1].modification_date, before[1].modification_date);
}

fn notes_of(read: &[paged_canvas::channel::NoteSummary]) -> Vec<(String, u32, String)> {
    read.iter()
        .map(|n| (n.id.clone(), n.offset, n.text.clone()))
        .collect()
}

#[test]
fn a_deleted_note_takes_its_place_and_undo_puts_it_back() {
    let mut m = model();
    let before = notes(&m);
    let text_before = text(&m);
    m.apply_mutation(&Mutation::DeleteNote {
        note_id: before[2].0.clone(),
    })
    .expect("delete");
    let after = notes(&m);
    assert_eq!(after.len(), 3);
    assert_eq!(after[2].1, before[3].1 - 1);
    assert_eq!(text(&m).matches(NOTE).count(), 3);
    m.undo().expect("undo");
    assert_eq!(notes(&m), before);
    assert_eq!(text(&m), text_before);
}

#[test]
fn convert_to_text_puts_a_space_and_the_notes_text_in_its_place() {
    let mut m = model();
    let before = notes(&m);
    let text_before = text(&m);
    m.apply_mutation(&Mutation::ConvertNoteToText {
        note_id: before[0].0.clone(),
    })
    .expect("convert");
    assert!(
        text(&m).starts_with(&format!(
            "Text with a not Check this claim.e here and{NOTE}"
        )),
        "{}",
        text(&m)
    );
    // A note of two paragraphs makes the story's paragraph break.
    m.apply_mutation(&Mutation::ConvertNoteToText {
        note_id: before[2].0.clone(),
    })
    .expect("convert two");
    assert_eq!(
        text(&m),
        format!(
            "Text with a not Check this claim.e here and{NOTE} more text.\n\
A second paragraph First line of a note.\nIts second paragraph. ends the story.{NOTE}"
        )
    );
    assert_eq!(m.notes().len(), 2);
    m.undo().expect("undo");
    m.undo().expect("undo");
    assert_eq!(notes(&m), before);
    assert_eq!(text(&m), text_before);
}

#[test]
fn a_text_delete_over_a_notes_place_takes_the_note() {
    let mut m = model();
    let before = notes(&m);
    let t = text(&m);
    // "e here and" and the note after it.
    let at = t.find("e here and").unwrap() as u32;
    let end = t.find(" more").unwrap() as u32;
    m.apply_mutation(&Mutation::DeleteRange {
        story_id: sample::story_id(),
        start: at,
        end,
        cell: None,
    })
    .expect("delete range");
    let after = notes(&m);
    assert_eq!(after.len(), 3, "{after:?}");
    assert!(after.iter().all(|n| n.0 != before[1].0));
    m.undo().expect("undo");
    assert_eq!(notes(&m), before);
    assert_eq!(text(&m), t);
}

/// What an IDML save of the edited document reads back as: its notes'
/// texts.
fn saved_notes(m: &CanvasModel) -> Vec<String> {
    let bytes = m.export_idml().expect("export");
    let doc = idml_import::import_idml_doc(&bytes).expect("re-import");
    doc.stories
        .iter()
        .flat_map(|s| &s.story.paragraphs)
        .flat_map(|p| &p.notes)
        .map(|n| n.text.clone())
        .collect()
}

#[test]
fn inserted_edited_and_deleted_notes_save_as_idml() {
    let mut m = model();
    assert_eq!(saved_notes(&m).len(), 4);
    let ids: Vec<String> = m.notes().into_iter().map(|n| n.id).collect();
    m.apply_mutation(&Mutation::InsertNote {
        story_id: sample::story_id(),
        offset: 0,
        paragraph: None,
        text: "At the start.".into(),
        user: None,
        date: None,
    })
    .expect("insert");
    m.apply_mutation(&Mutation::EditNote {
        note_id: ids[0].clone(),
        text: "Edited.".into(),
        date: None,
    })
    .expect("edit");
    m.apply_mutation(&Mutation::DeleteNote {
        note_id: ids[3].clone(),
    })
    .expect("delete");
    assert_eq!(
        saved_notes(&m),
        [
            "At the start.",
            "Edited.",
            "A remark between words.",
            "First line of a note.\rIts second paragraph."
        ]
    );
}
