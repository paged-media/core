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

//! v68 (paged.data Wave 8) — text typed at a placeholder field's
//! boundary lands OUTSIDE the field. InDesign treats a text variable as
//! one atomic character: a caret at either edge of it inserts beside it,
//! never into it. Before v68 `insertText` at the field's start (when the
//! field opened its paragraph) or at its end grew the field's own run, so
//! the next `setFieldValue` refresh overwrote what the user had typed.

use std::io::Write;

use paged_canvas::{
    channel::{MainToWorkerKind, Mutation, PlaceholderItem, WorkerToMainKind},
    CanvasModel, CanvasOptions,
};
use paged_mutate::operation::FieldKind;

const PLUGIN: &str = "media.paged.data";

/// One page, two framed stories: `story1` = "Story one body text",
/// `story2` = "Story two body".
fn small_idml() -> Vec<u8> {
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
<Document DOMVersion="13.1" Self="d1" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<idPkg:Spread src="Spreads/Spread_s1.xml"/>
<idPkg:Story src="Stories/Story_story1.xml"/>
<idPkg:Story src="Stories/Story_story2.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tfA" ParentStory="story1" GeometricBounds="100 100 200 300" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tfB" ParentStory="story2" GeometricBounds="300 100 400 300" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        for (id, text) in [
            ("story1", "Story one body text"),
            ("story2", "Story two body"),
        ] {
            zip.start_file(format!("Stories/Story_{id}.xml"), opts)
                .unwrap();
            zip.write_all(
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="{id}">
<ParagraphStyleRange>
<CharacterStyleRange><Content>{text}</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#
                )
                .as_bytes(),
            )
            .unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("d01", &small_idml(), CanvasOptions::default()).expect("load")
}

fn insert(m: &mut CanvasModel, story_id: &str, offset: u32, key: &str, value: Option<&str>) {
    m.apply_mutation(&Mutation::InsertField {
        story_id: story_id.into(),
        offset,
        field: FieldKind::Placeholder {
            plugin: PLUGIN.into(),
            key: key.into(),
            value: value.map(str::to_string),
        },
    })
    .expect("insert placeholder applies");
}

fn story_text(m: &CanvasModel, story_id: &str) -> String {
    let story = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story_id)
        .expect("story");
    story
        .story
        .paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn type_at(m: &mut CanvasModel, story_id: &str, offset: u32, text: &str) {
    m.apply_mutation(&Mutation::InsertText {
        story_id: story_id.into(),
        offset,
        text: text.into(),
        cell: None,
    })
    .expect("insertText applies");
}

fn refresh(m: &mut CanvasModel, value: &str) {
    let items = m.document_placeholders();
    assert_eq!(items.len(), 1, "exactly one field");
    m.apply_mutation(&Mutation::SetFieldValue {
        story_id: items[0].story_id.clone(),
        offset: items[0].offset,
        value: Some(value.into()),
    })
    .expect("setFieldValue applies");
}

#[test]
fn text_typed_at_a_fields_start_survives_a_refresh() {
    let mut m = model();
    insert(&mut m, "story1", 0, "name", Some("Ada"));
    assert_eq!(story_text(&m, "story1"), "AdaStory one body text");
    // The caret sits before the field (story start) — type there.
    type_at(&mut m, "story1", 0, "Dear ");
    assert_eq!(story_text(&m, "story1"), "Dear AdaStory one body text");
    assert_eq!(
        m.document_placeholders()[0].offset,
        5,
        "the field moved right"
    );
    assert_eq!(m.document_placeholders()[0].value.as_deref(), Some("Ada"));
    refresh(&mut m, "Grace");
    assert_eq!(
        story_text(&m, "story1"),
        "Dear GraceStory one body text",
        "the refresh replaced the field, not what was typed before it"
    );
}

#[test]
fn text_typed_at_a_fields_end_survives_a_refresh() {
    let mut m = model();
    // "Story " + field + "one body text"
    insert(&mut m, "story1", 6, "name", Some("Ada"));
    assert_eq!(story_text(&m, "story1"), "Story Adaone body text");
    type_at(&mut m, "story1", 9, ", ");
    assert_eq!(story_text(&m, "story1"), "Story Ada, one body text");
    refresh(&mut m, "Grace");
    assert_eq!(story_text(&m, "story1"), "Story Grace, one body text");
}

#[test]
fn text_typed_after_a_field_that_ends_the_story_survives_a_refresh() {
    let mut m = model();
    let len = "Story two body".len() as u32;
    insert(&mut m, "story2", len, "n", Some("1"));
    type_at(&mut m, "story2", len + 1, "!");
    assert_eq!(story_text(&m, "story2"), "Story two body1!");
    refresh(&mut m, "22");
    assert_eq!(story_text(&m, "story2"), "Story two body22!");
}

#[test]
fn typing_beside_a_field_undoes_to_the_field_alone() {
    let mut m = model();
    insert(&mut m, "story1", 0, "name", Some("Ada"));
    type_at(&mut m, "story1", 0, "Dear ");
    m.undo().expect("undo the typing");
    assert_eq!(story_text(&m, "story1"), "AdaStory one body text");
    let items = m.document_placeholders();
    assert_eq!(
        (items[0].offset, items[0].value.as_deref()),
        (0, Some("Ada"))
    );
}
