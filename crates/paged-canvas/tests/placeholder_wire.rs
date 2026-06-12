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

//! v43 (D-01) — tagged placeholders through the canvas model + wire:
//! insert via `Mutation::InsertField { field: placeholder }`,
//! enumerate via `document_placeholders` (the
//! `RequestDocumentPlaceholders` read door), re-resolve via
//! `Mutation::SetFieldValue` (one undoable step on the worker's undo
//! log), and the camelCase wire shapes the SDK members bind against.

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

#[test]
fn insert_enumerate_set_value_undo_round_trip() {
    let mut m = model();
    assert!(m.document_placeholders().is_empty(), "none at load");

    // Insert unresolved at offset 6 of story1.
    insert(&mut m, "story1", 6, "price", None);
    let items = m.document_placeholders();
    assert_eq!(
        items,
        vec![PlaceholderItem {
            story_id: "story1".into(),
            offset: 6,
            plugin: PLUGIN.into(),
            key: "price".into(),
            value: None,
        }]
    );

    // Re-resolve at the enumerated offset: ONE mutation, ONE undo step.
    m.apply_mutation(&Mutation::SetFieldValue {
        story_id: "story1".into(),
        offset: items[0].offset,
        value: Some("€ 9,99".into()),
    })
    .expect("set value applies");
    assert_eq!(
        m.document_placeholders()[0].value.as_deref(),
        Some("€ 9,99")
    );

    // Undo #1: back to unresolved.
    m.undo().expect("undo set value");
    assert_eq!(m.document_placeholders()[0].value, None);
    // Undo #2: the field itself is gone.
    m.undo().expect("undo insert");
    assert!(m.document_placeholders().is_empty());
}

#[test]
fn placeholders_enumerate_across_stories_in_story_order() {
    let mut m = model();
    // story2 first by insertion order; enumerate is story order.
    insert(&mut m, "story2", 0, "b", Some("B"));
    insert(&mut m, "story1", 0, "a", None);
    let items = m.document_placeholders();
    assert_eq!(items.len(), 2);
    assert_eq!(
        (items[0].story_id.as_str(), items[0].key.as_str()),
        ("story1", "a")
    );
    assert_eq!(
        (items[1].story_id.as_str(), items[1].key.as_str()),
        ("story2", "b")
    );
    assert_eq!(items[1].value.as_deref(), Some("B"));
}

// ── Wire shapes (what the SDK members serialise/parse) ───────────────

#[test]
fn placeholder_wire_shapes_are_pinned() {
    // Mutation::InsertField with the plugin placeholder kind.
    let insert = Mutation::InsertField {
        story_id: "story1".into(),
        offset: 6,
        field: FieldKind::Placeholder {
            plugin: PLUGIN.into(),
            key: "price".into(),
            value: Some("€ 9,99".into()),
        },
    };
    assert_eq!(
        serde_json::to_value(&insert).unwrap(),
        serde_json::json!({
            "op": "insertField",
            "args": {
                "storyId": "story1",
                "offset": 6,
                "field": { "placeholder": {
                    "plugin": "media.paged.data",
                    "key": "price",
                    "value": "€ 9,99",
                }},
            },
        })
    );
    // `value` omitted ⇒ unresolved (serde default).
    let unresolved: Mutation = serde_json::from_value(serde_json::json!({
        "op": "insertField",
        "args": {
            "storyId": "story1",
            "offset": 0,
            "field": { "placeholder": { "plugin": PLUGIN, "key": "k" } },
        },
    }))
    .expect("value is optional on the wire");
    match unresolved {
        Mutation::InsertField {
            field: FieldKind::Placeholder { value, .. },
            ..
        } => assert_eq!(value, None),
        other => panic!("unexpected: {other:?}"),
    }

    // Mutation::SetFieldValue.
    let set = Mutation::SetFieldValue {
        story_id: "story1".into(),
        offset: 6,
        value: None,
    };
    assert_eq!(
        serde_json::to_value(&set).unwrap(),
        serde_json::json!({
            "op": "setFieldValue",
            "args": { "storyId": "story1", "offset": 6, "value": null },
        })
    );

    // RequestDocumentPlaceholders → DocumentPlaceholders reply.
    let req = MainToWorkerKind::RequestDocumentPlaceholders;
    assert_eq!(
        serde_json::to_value(&req).unwrap(),
        serde_json::json!({ "kind": "requestDocumentPlaceholders" })
    );
    let reply = WorkerToMainKind::DocumentPlaceholders {
        items: vec![PlaceholderItem {
            story_id: "story1".into(),
            offset: 6,
            plugin: PLUGIN.into(),
            key: "price".into(),
            value: Some("€ 9,99".into()),
        }],
    };
    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        serde_json::json!({
            "kind": "documentPlaceholders",
            "payload": { "items": [{
                "storyId": "story1",
                "offset": 6,
                "plugin": "media.paged.data",
                "key": "price",
                "value": "€ 9,99",
            }]},
        })
    );
}
