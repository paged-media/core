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

//! v43 (D-01) — tagged-placeholder kernel round-trips: insert (run
//! split + display text), `SetFieldValue` re-resolution as ONE
//! undoable step, delete-after-re-resolution undo fidelity, and
//! placeholders across multiple stories.

use std::io::Write;

use paged_mutate::{apply, FieldKind, Operation};
use paged_scene::Document;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const PLUGIN: &str = "media.paged.data";

/// Two stories: `story1` = "Story one body text" (one run),
/// `story2` = "Story two body".
fn two_story_idml() -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" Self="d1">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_story1.xml"/>
  <idPkg:Story src="Stories/Story_story2.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" PageCount="1">
    <Page Self="p1" GeometricBounds="0 0 400 612" ItemTransform="1 0 0 1 0 0"/>
    <TextFrame Self="tfA" ParentStory="story1" GeometricBounds="40 40 200 572" ItemTransform="1 0 0 1 0 0"/>
    <TextFrame Self="tfB" ParentStory="story2" GeometricBounds="220 40 380 572" ItemTransform="1 0 0 1 0 0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    for (id, text) in [
        ("story1", "Story one body text"),
        ("story2", "Story two body"),
    ] {
        zip.start_file(format!("Stories/Story_{id}.xml"), deflated)
            .unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
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
    zip.finish().unwrap().into_inner()
}

fn open_doc() -> Document {
    Document::open(&two_story_idml()).expect("open synthetic IDML")
}

/// The story's flattened char-space text (sum of run texts — the same
/// space field offsets address).
fn flat_text(doc: &Document, story_id: &str) -> String {
    doc.stories
        .iter()
        .find(|s| s.self_id == story_id)
        .expect("story present")
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.as_str())
        .collect()
}

fn placeholder_runs(doc: &Document, story_id: &str) -> Vec<(String, String, Option<String>)> {
    doc.stories
        .iter()
        .find(|s| s.self_id == story_id)
        .expect("story present")
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .filter_map(|r| {
            r.placeholder
                .as_ref()
                .map(|t| (t.plugin.clone(), t.key.clone(), t.value.clone()))
        })
        .collect()
}

fn insert_op(story_id: &str, offset: u32, key: &str, value: Option<&str>) -> Operation {
    Operation::InsertField {
        story_id: story_id.to_string(),
        offset,
        field: FieldKind::Placeholder {
            plugin: PLUGIN.to_string(),
            key: key.to_string(),
            value: value.map(str::to_string),
        },
    }
}

#[test]
fn unresolved_placeholder_displays_key_token_and_undo_round_trips() {
    let mut doc = open_doc();
    // Mid-run offset 6 splits "Story one body text" around the field.
    let applied = apply(&mut doc, &insert_op("story1", 6, "price", None)).expect("insert");
    assert_eq!(flat_text(&doc, "story1"), "Story <price>one body text");
    assert_eq!(
        placeholder_runs(&doc, "story1"),
        vec![(PLUGIN.to_string(), "price".to_string(), None)]
    );

    // Undo removes the whole tagged run; the split halves keep the
    // original char content.
    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(flat_text(&doc, "story1"), "Story one body text");
    assert!(placeholder_runs(&doc, "story1").is_empty());
}

#[test]
fn resolved_placeholder_displays_cached_value() {
    let mut doc = open_doc();
    apply(&mut doc, &insert_op("story1", 0, "price", Some("€ 9,99"))).expect("insert");
    assert_eq!(flat_text(&doc, "story1"), "€ 9,99Story one body text");
}

#[test]
fn set_field_value_is_one_undoable_step_and_normalises_to_run_start() {
    let mut doc = open_doc();
    apply(&mut doc, &insert_op("story1", 6, "total", None)).expect("insert");
    // "<total>" occupies [6, 13); address it mid-run to prove the
    // echoed op + inverse normalise to the run start.
    let applied = apply(
        &mut doc,
        &Operation::SetFieldValue {
            story_id: "story1".to_string(),
            offset: 9,
            value: Some("1.234,00".to_string()),
        },
    )
    .expect("set value");
    assert_eq!(flat_text(&doc, "story1"), "Story 1.234,00one body text");
    assert_eq!(
        applied.op,
        Operation::SetFieldValue {
            story_id: "story1".to_string(),
            offset: 6,
            value: Some("1.234,00".to_string()),
        }
    );
    assert_eq!(
        applied.inverse,
        Operation::SetFieldValue {
            story_id: "story1".to_string(),
            offset: 6,
            value: None,
        }
    );

    // ONE inverse application returns the unresolved token display.
    apply(&mut doc, &applied.inverse).expect("undo");
    assert_eq!(flat_text(&doc, "story1"), "Story <total>one body text");
    // Redo (the captured forward op) re-resolves.
    apply(&mut doc, &applied.op).expect("redo");
    assert_eq!(flat_text(&doc, "story1"), "Story 1.234,00one body text");
}

#[test]
fn delete_after_re_resolution_undoes_to_current_value() {
    let mut doc = open_doc();
    apply(&mut doc, &insert_op("story1", 0, "name", Some("old"))).expect("insert");
    apply(
        &mut doc,
        &Operation::SetFieldValue {
            story_id: "story1".to_string(),
            offset: 0,
            value: Some("new".to_string()),
        },
    )
    .expect("re-resolve");

    let deleted = apply(
        &mut doc,
        &Operation::DeleteField {
            story_id: "story1".to_string(),
            offset: 0,
            // The caller's stale view may still carry the old value;
            // identity is (plugin, key) + offset.
            field: FieldKind::Placeholder {
                plugin: PLUGIN.to_string(),
                key: "name".to_string(),
                value: Some("old".to_string()),
            },
        },
    )
    .expect("delete");
    assert!(placeholder_runs(&doc, "story1").is_empty());

    // The inverse re-inserts what was actually displayed: "new".
    apply(&mut doc, &deleted.inverse).expect("undo delete");
    assert_eq!(flat_text(&doc, "story1"), "newStory one body text");
    assert_eq!(
        placeholder_runs(&doc, "story1"),
        vec![(
            PLUGIN.to_string(),
            "name".to_string(),
            Some("new".to_string())
        )]
    );
}

#[test]
fn placeholders_land_in_their_own_stories() {
    let mut doc = open_doc();
    apply(&mut doc, &insert_op("story1", 0, "a", Some("A"))).expect("insert a");
    // story2 = "Story two body" (14 chars) — append at the end.
    apply(&mut doc, &insert_op("story2", 14, "b", None)).expect("insert b");
    assert_eq!(
        placeholder_runs(&doc, "story1"),
        vec![(PLUGIN.to_string(), "a".to_string(), Some("A".to_string()))]
    );
    assert_eq!(
        placeholder_runs(&doc, "story2"),
        vec![(PLUGIN.to_string(), "b".to_string(), None)]
    );
    assert_eq!(flat_text(&doc, "story2"), "Story two body<b>");
}

#[test]
fn set_field_value_on_plain_text_offset_is_rejected() {
    let mut doc = open_doc();
    let err = apply(
        &mut doc,
        &Operation::SetFieldValue {
            story_id: "story1".to_string(),
            offset: 3,
            value: Some("x".to_string()),
        },
    );
    assert!(err.is_err(), "no placeholder at offset 3 must error");
}
