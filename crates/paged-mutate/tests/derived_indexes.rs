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

//! `apply`'s post-condition: `Document`'s derived indices are fresh.
//!
//! `text_frame_index` / `frame_for_story` / `anchors` are `#[serde(skip)]`
//! caches, functions of `spreads` + `stories`. They were built by
//! `Document::open` and by nothing that ran after a mutation, and the
//! apply layer said so in a comment asking consumers to rebuild
//! themselves. No consumer did — so the moment the WIRE created a frame,
//! `Document::text_frame` could not find it, and every reader that
//! resolves a frame by id silently degraded.
//!
//! The reader that made it visible is `Document::frame_chain`: it
//! collects a story's frames by SCANNING the spreads (always fresh) but
//! follows `NextTextFrame` through `Document::text_frame` (the index).
//! So a `LinkFrames` into a frame `InsertTextFrame` had just minted
//! wrote a perfectly correct pointer into a chain walk that stopped one
//! frame short — the model threaded and the page rendered nothing. That
//! is the shape the two tests below pin, at the layer that owns it.

use std::io::Write;

use paged_mutate::{apply, NodeId, NodeSpec, Operation};
use paged_scene::Document;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// One spread, one page, one text frame `host` carrying `story1` —
/// enough text that a second frame in the chain would have something to
/// receive. Built in memory (the `corpus/generated` fixtures are
/// gitignored, so a test that read one would be a fixture-presence test).
fn idml_one_frame_one_story() -> Vec<u8> {
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
    <TextFrame Self="host" ParentStory="story1" NextTextFrame="n"
               GeometricBounds="40 40 200 300" ItemTransform="1 0 0 1 0 0"/>
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
        <Content>Threading needs both halves: the pointer and the index that resolves it.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
    )
    .unwrap();

    zip.finish().unwrap().into_inner()
}

fn open() -> Document {
    idml_import::import_idml_doc(&idml_one_frame_one_story()).expect("fixture must open")
}

const WIRE_FRAME: &str = "TextFrame/wire";

fn insert_wire_frame(spread_id: &str, position: usize) -> Operation {
    Operation::InsertNode {
        parent: NodeId::Spread(spread_id.to_string()),
        position,
        node: NodeSpec::TextFrame {
            self_id: WIRE_FRAME.to_string(),
            bounds: [40.0, 320.0, 200.0, 580.0],
            fill_color: None,
            stroke_color: None,
            stroke_weight: None,
            item_transform: None,
            parent_story: Some("Story/wire".to_string()),
        },
        z_slot: None,
    }
}

fn spread_id(doc: &Document) -> String {
    doc.spreads[0]
        .spread
        .self_id
        .clone()
        .expect("fixture spread carries a Self id")
}

/// The index itself: a frame the wire created must be resolvable by id.
#[test]
fn a_wire_created_frame_is_resolvable_through_the_frame_index() {
    let mut doc = open();
    assert!(
        doc.text_frame("host").is_some(),
        "the frame the fixture shipped is indexed at open"
    );

    let op = insert_wire_frame(&spread_id(&doc), doc.spreads[0].spread.text_frames.len());
    apply(&mut doc, &op).expect("insert the frame");

    assert!(
        doc.text_frame(WIRE_FRAME).is_some(),
        "a frame created by a mutation must be in `text_frame_index` — \
         `apply` rebuilds the derived caches, and without that every \
         reader that resolves a frame by id (chain walks, composition \
         projection, body-page mapping) silently skips it"
    );
    assert_eq!(
        doc.text_frame(WIRE_FRAME).and_then(|f| f.self_id.clone()),
        Some(WIRE_FRAME.to_string()),
        "and the index entry must point at that frame, not merely exist"
    );
    // Its story index half, too: the frame declared `Story/wire`.
    assert_eq!(
        doc.frame_for("Story/wire")
            .and_then(|f| f.self_id.clone())
            .as_deref(),
        Some(WIRE_FRAME),
        "`frame_for_story` is rebuilt by the same pass"
    );
}

/// The reader that made it visible: `frame_chain` must walk INTO the
/// wire-created frame. This is the `LinkFrames`-renders-nothing defect,
/// one layer below the pixels.
#[test]
fn linking_into_a_wire_created_frame_extends_the_story_chain() {
    let mut doc = open();
    let sid = spread_id(&doc);
    assert_eq!(
        doc.frame_chain("story1").len(),
        1,
        "the fixture starts with a one-frame chain"
    );

    let op = insert_wire_frame(&sid, doc.spreads[0].spread.text_frames.len());
    apply(&mut doc, &op).expect("insert the frame");
    apply(
        &mut doc,
        &Operation::LinkFrames {
            from: "host".to_string(),
            to: WIRE_FRAME.to_string(),
        },
    )
    .expect("link");

    let chain = doc.frame_chain("story1");
    assert_eq!(
        chain.len(),
        2,
        "the chain must reach the linked frame — it is found by following \
         `next_text_frame` through `Document::text_frame`, so a stale index \
         ends the walk at the source frame and the story renders as if it \
         were never threaded"
    );
    assert_eq!(chain[0].self_id.as_deref(), Some("host"));
    assert_eq!(chain[1].self_id.as_deref(), Some(WIRE_FRAME));
}

/// A `Batch` rebuilds ONCE, at the end — so the two halves of a thread
/// authored in a single undo step still leave a fresh index behind.
#[test]
fn a_batch_that_creates_and_links_leaves_a_fresh_index() {
    let mut doc = open();
    let sid = spread_id(&doc);
    let batch = Operation::Batch {
        ops: vec![
            insert_wire_frame(&sid, doc.spreads[0].spread.text_frames.len()),
            Operation::LinkFrames {
                from: "host".to_string(),
                to: WIRE_FRAME.to_string(),
            },
        ],
    };
    apply(&mut doc, &batch).expect("batch");

    assert_eq!(
        doc.frame_chain("story1").len(),
        2,
        "one rebuild at the end of the batch is enough — the children only \
         read the indices for advisory invalidation hints"
    );
}

/// Undo is `apply` too: reversing the link must leave the index
/// describing the document that is actually there.
#[test]
fn undoing_the_insert_drops_the_frame_from_the_index() {
    let mut doc = open();
    let sid = spread_id(&doc);
    let op = insert_wire_frame(&sid, doc.spreads[0].spread.text_frames.len());
    let inserted = apply(&mut doc, &op).expect("insert");
    assert!(doc.text_frame(WIRE_FRAME).is_some());

    apply(&mut doc, &inserted.inverse).expect("undo the insert");
    assert!(
        doc.text_frame(WIRE_FRAME).is_none(),
        "a removed frame must leave the index with it — a stale entry \
         points at a `(spread, frame)` slot now holding a DIFFERENT frame"
    );
}
