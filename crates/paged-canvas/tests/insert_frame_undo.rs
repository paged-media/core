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

//! Undoing an `insertTextFrame`, or the typing that followed it,
//! returns the document to exactly where it was.
//!
//! The wire insert mints the frame's `ParentStory` and creates it empty
//! (one paragraph, one empty run). Its undo removed the frame and left
//! the story behind — an empty, frameless story the document never had,
//! which a save writes out as a `Stories/` part. The insert's inverse
//! now removes the story it minted as well (and only one it minted);
//! removing a frame any other way still keeps its story, so undoing a
//! delete re-attaches it.
//!
//! Typing into that fresh story and undoing deleted the text and the
//! empty run it had been typed into, leaving a run-less paragraph.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions, ElementId, PageId};

fn idml() -> Vec<u8> {
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
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn load() -> CanvasModel {
    CanvasModel::load("doc", &idml(), CanvasOptions::default()).expect("load")
}

fn insert_frame() -> Mutation {
    Mutation::InsertTextFrame {
        page_id: PageId("p1".into()),
        bounds: (20.0, 20.0, 300.0, 200.0),
    }
}

fn story_ids(model: &CanvasModel) -> Vec<String> {
    model
        .scene()
        .stories
        .iter()
        .map(|s| s.self_id.clone())
        .collect()
}

#[test]
fn undoing_an_inserted_text_frame_takes_its_empty_story_too() {
    let mut model = load();
    let hash_before = model.current_state_hash();
    let stories_before = story_ids(&model);

    model.apply_mutation(&insert_frame()).expect("insert");
    let hash_after = model.current_state_hash();
    assert_eq!(story_ids(&model).len(), stories_before.len() + 1);

    model.undo().expect("undo");
    assert_eq!(story_ids(&model), stories_before, "no orphan story");
    assert_eq!(model.current_state_hash(), hash_before);

    model.redo().expect("redo");
    assert_eq!(
        model.current_state_hash(),
        hash_after,
        "redo mints it again"
    );
}

/// The same frame inside a batch, undone as one step.
#[test]
fn undoing_a_batch_that_inserted_a_frame_takes_its_story_too() {
    let mut model = load();
    let hash_before = model.current_state_hash();
    model
        .apply_mutation(&Mutation::Batch {
            ops: vec![
                insert_frame(),
                Mutation::BindCreated { handle: "f".into() },
                Mutation::InsertText {
                    story_id: "$h:f".into(),
                    offset: 0,
                    text: "poured".into(),
                    cell: None,
                },
            ],
        })
        .expect("batch");
    let hash_after = model.current_state_hash();
    model.undo().expect("undo");
    assert_eq!(model.current_state_hash(), hash_before);
    model.redo().expect("redo");
    assert_eq!(model.current_state_hash(), hash_after);
}

/// Deleting a frame whose story has text keeps the story: undo of the
/// delete re-attaches it, text and all.
#[test]
fn deleting_a_frame_with_text_keeps_its_story() {
    let mut model = load();
    let out = model.apply_mutation(&insert_frame()).expect("insert");
    let Some(ElementId::TextFrame(frame)) = out.created_id else {
        panic!("a text frame id");
    };
    let story = story_ids(&model).pop().expect("the minted story");
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: "keep me".into(),
            cell: None,
        })
        .expect("type");
    let hash_typed = model.current_state_hash();
    model
        .apply_mutation(&Mutation::DeleteFrame { frame_id: frame })
        .expect("delete");
    assert!(story_ids(&model).contains(&story), "the story stays");
    model.undo().expect("undo delete");
    assert_eq!(model.current_state_hash(), hash_typed);
}

/// Typing into the empty run of a fresh frame and undoing it gives the
/// run back: the paragraph is exactly as the insert left it.
#[test]
fn undoing_typing_into_a_fresh_frame_restores_its_empty_run() {
    let mut model = load();
    model.apply_mutation(&insert_frame()).expect("insert");
    let story = story_ids(&model).pop().expect("the minted story");
    let hash_empty = model.current_state_hash();
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: "typed".into(),
            cell: None,
        })
        .expect("type");
    let hash_typed = model.current_state_hash();
    model.undo().expect("undo typing");
    assert_eq!(
        model.current_state_hash(),
        hash_empty,
        "the empty run is back"
    );
    model.redo().expect("redo typing");
    assert_eq!(model.current_state_hash(), hash_typed);
}

/// The same across a paragraph break.
#[test]
fn undoing_two_typed_paragraphs_restores_the_empty_run() {
    let mut model = load();
    model.apply_mutation(&insert_frame()).expect("insert");
    let story = story_ids(&model).pop().expect("the minted story");
    let hash_empty = model.current_state_hash();
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: story,
            offset: 0,
            text: "one\ntwo".into(),
            cell: None,
        })
        .expect("type");
    model.undo().expect("undo typing");
    assert_eq!(model.current_state_hash(), hash_empty);
}
