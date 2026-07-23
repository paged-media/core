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

//! Story creation on `InsertNode { NodeSpec::TextFrame }` (v0.42.1 —
//! the K-1 live-validation finding): a spec naming a ParentStory id
//! with no parsed story CREATES the empty story (the wire's
//! InsertTextFrame mapping mints the id), so `hitTest` resolves a
//! story and callers can pour text immediately; the RemoveNode-built
//! spec captures the id so undo of a delete REATTACHES the story.
//! `parent_story: None` attaches nothing — the legacy story-less shape
//! stays byte-identical across remove → undo. Before this, a fresh
//! frame had no story at all — paged.sheet's and paged.data's table
//! lowerings both silently placed empty frames in the live editor.

use std::path::PathBuf;

use paged_mutate::{apply, NodeId, NodeSpec, Operation};
use paged_scene::Document;

fn fixture_bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry-groups.idml");
    std::fs::read(path).expect("read geometry fixture")
}

fn frame_story(doc: &Document, id: &str) -> Option<String> {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some(id))
        .expect("frame present")
        .parent_story
        .clone()
}

fn insert_frame_op(doc: &Document) -> Operation {
    let spread_id = doc.spreads[0]
        .spread
        .self_id
        .clone()
        .expect("fixture spread has a Self id");
    Operation::InsertNode {
        parent: NodeId::Spread(spread_id),
        position: doc.spreads[0].spread.text_frames.len(),
        node: NodeSpec::TextFrame {
            self_id: "TextFrame/mint-test".to_string(),
            bounds: [10.0, 10.0, 110.0, 210.0],
            fill_color: None,
            stroke_color: None,
            stroke_weight: None,
            item_transform: None,
            parent_story: Some("Story/umint".to_string()), // unknown id ⇒ created
        },
        z_slot: None,
    }
}

#[test]
fn fresh_insert_mints_a_parent_story() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let stories_before = doc.stories.len();
    let op = insert_frame_op(&doc);
    apply(&mut doc, &op).expect("insert");

    let sid =
        frame_story(&doc, "TextFrame/mint-test").expect("fresh frame carries its ParentStory");
    assert_eq!(sid, "Story/umint");
    assert_eq!(doc.stories.len(), stories_before + 1, "one new story");
    let story = doc
        .stories
        .iter()
        .find(|s| s.self_id == sid)
        .expect("minted story is in doc.stories");
    // The empty-parsed-story shape the text ops' locate() requires.
    assert_eq!(story.story.paragraphs.len(), 1);
    assert_eq!(story.story.paragraphs[0].runs.len(), 1);
    assert!(story.story.paragraphs[0].runs[0].text.is_empty());
}

#[test]
fn delete_then_undo_reattaches_the_same_story() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let op = insert_frame_op(&doc);
    apply(&mut doc, &op).expect("insert");
    let sid = frame_story(&doc, "TextFrame/mint-test").expect("minted");

    // Delete the frame; the inverse spec must carry the story id.
    let removed = apply(
        &mut doc,
        &Operation::RemoveNode {
            node: NodeId::TextFrame("TextFrame/mint-test".to_string()),
        },
    )
    .expect("remove");
    match &removed.inverse {
        Operation::InsertNode {
            node: NodeSpec::TextFrame { parent_story, .. },
            ..
        } => assert_eq!(parent_story.as_deref(), Some(sid.as_str())),
        other => panic!("inverse is not an InsertNode TextFrame: {other:?}"),
    }

    // Undo (apply the inverse): the SAME story reattaches — no re-mint.
    let stories_before_undo = doc.stories.len();
    apply(&mut doc, &removed.inverse).expect("undo of delete");
    assert_eq!(
        frame_story(&doc, "TextFrame/mint-test").as_deref(),
        Some(sid.as_str()),
        "undo reattaches the original story"
    );
    assert_eq!(doc.stories.len(), stories_before_undo, "no second mint");
}
