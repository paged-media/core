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

//! Undoing the typing into a fresh text frame returns the document to
//! exactly where it was.
//!
//! The wire insert creates the frame's story as one paragraph with one
//! empty run. Typing fills that run; deleting the text (the undo) dropped
//! the emptied run with it, leaving a run-less paragraph — a different
//! state hash than before the typing.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions, PageId};

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
