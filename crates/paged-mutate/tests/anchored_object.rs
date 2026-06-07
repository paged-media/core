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

//! W1.16 — anchored-object setting mutation round-trips. An anchored
//! frame is addressed by its OWN page-item `NodeId` (the anchored
//! `<TextFrame>`'s `Self` id); the apply layer locates its
//! `AnchoredObjectSetting` inside the story runs rather than the spread
//! page-item vecs. One round-trip per `Value` type: Text
//! (`AnchoredPosition`), Length (`AnchoredXOffset`), Bool
//! (`AnchoredLockPosition`).

use std::io::Write;

use paged_mutate::{apply, NodeId, Operation, PropertyPath, Value};
use paged_scene::Document;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// A one-page IDML whose story `story1` carries a single paragraph
/// with an anchored `<TextFrame Self="anchor1">` nested under its
/// `<CharacterStyleRange>`, with a populated `<AnchoredObjectSetting>`.
fn idml_with_anchored_frame() -> Vec<u8> {
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
</Document>"#,
    )
    .unwrap();

    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1" PageCount="1">
    <Page Self="p1" GeometricBounds="0 0 400 612" ItemTransform="1 0 0 1 0 0"/>
    <TextFrame Self="host" ParentStory="story1" GeometricBounds="40 40 380 572" ItemTransform="1 0 0 1 0 0"/>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();

    zip.start_file("Stories/Story_story1.xml", deflated)
        .unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="story1">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Content>Before</Content>
        <TextFrame Self="anchor1" ParentStory="story1"
                   GeometricBounds="0 0 50 80" ItemTransform="1 0 0 1 5 7">
          <AnchoredObjectSetting AnchoredPosition="InlinePosition"
                                 SpineRelative="false"
                                 AnchorXoffset="0"
                                 AnchorYoffset="-2"
                                 AnchorPoint="TopLeftAnchor"
                                 LockPosition="false"/>
        </TextFrame>
        <Content>After</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

fn open_doc() -> Document {
    Document::open(&idml_with_anchored_frame()).expect("fixture must open")
}

/// Read the anchored frame's `AnchoredObjectSetting` back out of the
/// document for assertions.
fn setting(doc: &Document) -> &paged_parse::AnchoredObjectSetting {
    doc.stories
        .iter()
        .flat_map(|s| s.story.paragraphs.iter())
        .flat_map(|p| p.anchored_frames.iter())
        .find(|f| f.self_id.as_deref() == Some("anchor1"))
        .and_then(|f| f.setting.as_ref())
        .expect("anchor1 carries a setting")
}

fn set(path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty {
        node: NodeId::TextFrame("anchor1".into()),
        path,
        value,
    }
}

#[test]
fn anchored_position_text_round_trips() {
    let mut doc = open_doc();
    assert_eq!(
        setting(&doc).anchored_position.as_deref(),
        Some("InlinePosition")
    );

    let applied = apply(
        &mut doc,
        &set(PropertyPath::AnchoredPosition, Value::Text("Custom".into())),
    )
    .expect("apply AnchoredPosition");
    assert_eq!(setting(&doc).anchored_position.as_deref(), Some("Custom"));
    // The setting reflows its host line.
    assert!(!applied.invalidation.text_reflow.is_empty());

    // Inverse restores the prior value exactly (bytewise round-trip).
    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert_eq!(
        setting(&doc).anchored_position.as_deref(),
        Some("InlinePosition")
    );
}

#[test]
fn anchored_position_empty_clears_then_restores() {
    let mut doc = open_doc();
    // The empty string clears the override back to None.
    let applied = apply(
        &mut doc,
        &set(PropertyPath::AnchoredPosition, Value::Text(String::new())),
    )
    .expect("clear AnchoredPosition");
    assert_eq!(setting(&doc).anchored_position, None);
    // Undo restores the original "InlinePosition".
    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert_eq!(
        setting(&doc).anchored_position.as_deref(),
        Some("InlinePosition")
    );
}

#[test]
fn anchored_x_offset_length_round_trips() {
    let mut doc = open_doc();
    assert_eq!(setting(&doc).anchor_x_offset, 0.0);

    let applied = apply(
        &mut doc,
        &set(PropertyPath::AnchoredXOffset, Value::Length(Some(12.5))),
    )
    .expect("apply AnchoredXOffset");
    assert_eq!(setting(&doc).anchor_x_offset, 12.5);
    assert!(!applied.invalidation.text_reflow.is_empty());

    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert_eq!(setting(&doc).anchor_x_offset, 0.0);
}

#[test]
fn anchored_y_offset_length_none_resets_to_zero() {
    let mut doc = open_doc();
    // Seed a non-zero Y offset, then `Length(None)` resets it to 0.
    apply(
        &mut doc,
        &set(PropertyPath::AnchoredYOffset, Value::Length(Some(9.0))),
    )
    .expect("seed AnchoredYOffset");
    assert_eq!(setting(&doc).anchor_y_offset, 9.0);

    let applied = apply(
        &mut doc,
        &set(PropertyPath::AnchoredYOffset, Value::Length(None)),
    )
    .expect("reset AnchoredYOffset");
    assert_eq!(setting(&doc).anchor_y_offset, 0.0);
    // Inverse restores the seeded 9.0.
    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert_eq!(setting(&doc).anchor_y_offset, 9.0);
}

#[test]
fn anchored_lock_position_bool_round_trips() {
    let mut doc = open_doc();
    assert!(!setting(&doc).lock_position);

    let applied = apply(
        &mut doc,
        &set(PropertyPath::AnchoredLockPosition, Value::Bool(true)),
    )
    .expect("apply AnchoredLockPosition");
    assert!(setting(&doc).lock_position);
    assert!(!applied.invalidation.text_reflow.is_empty());

    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert!(!setting(&doc).lock_position);
}

#[test]
fn anchored_spine_relative_bool_round_trips() {
    let mut doc = open_doc();
    assert!(!setting(&doc).spine_relative);

    let applied = apply(
        &mut doc,
        &set(PropertyPath::AnchoredSpineRelative, Value::Bool(true)),
    )
    .expect("apply AnchoredSpineRelative");
    assert!(setting(&doc).spine_relative);

    apply(&mut doc, &applied.inverse).expect("apply inverse");
    assert!(!setting(&doc).spine_relative);
}

#[test]
fn anchored_write_to_unknown_frame_errors() {
    let mut doc = open_doc();
    let op = Operation::SetProperty {
        node: NodeId::TextFrame("does-not-exist".into()),
        path: PropertyPath::AnchoredPosition,
        value: Value::Text("Custom".into()),
    };
    let err = apply(&mut doc, &op).expect_err("unknown frame must error");
    // The standard NodeNotFound contract (not a silent no-op).
    assert!(
        matches!(err, paged_mutate::OperationError::NodeNotFound(_)),
        "expected NodeNotFound, got {err:?}"
    );
}
