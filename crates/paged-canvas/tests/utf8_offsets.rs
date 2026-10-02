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

//! A text offset inside a multi-byte UTF-8 character is refused, not a
//! panic. insertText / deleteRange address bytes; `String::insert_str` and
//! `replace_range` panic off a character boundary, so a stale or hostile
//! offset aborted the engine (thoughts ADR 031: untrusted input never
//! aborts the engine). Found by the ADR 027 digest-gate harness.

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation, PageId};

fn story_with(text: &str) -> (CanvasModel, String) {
    let idml = paged_canvas::blank::blank_idml(612.0, 792.0);
    let mut m = CanvasModel::load("doc", &idml, CanvasOptions::default()).expect("load");
    let page = m.scene().spreads[0].spread.pages[0]
        .self_id
        .clone()
        .expect("page id");
    let out = m
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: PageId(page),
            bounds: (36.0, 36.0, 576.0, 756.0),
        })
        .expect("frame");
    let Some(ElementId::TextFrame(frame)) = out.created_id else {
        panic!("expected a text frame, got {:?}", out.created_id);
    };
    let story = m.scene().spreads[0]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(frame.as_str()))
        .and_then(|f| f.parent_story.clone())
        .expect("minted story");
    m.apply_mutation(&Mutation::InsertText {
        story_id: story.clone(),
        offset: 0,
        text: text.into(),
        cell: None,
    })
    .expect("insert text");
    (m, story)
}

fn text(m: &CanvasModel, story: &str) -> String {
    m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story")
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter().map(|r| r.text.clone()))
        .collect()
}

#[test]
fn an_insert_inside_a_character_is_refused() {
    // "é" is two bytes: offset 2 is inside it.
    let (mut m, story) = story_with("Café au lait");
    let before = text(&m, &story);
    let r = m.apply_mutation(&Mutation::InsertText {
        story_id: story.clone(),
        offset: 4,
        text: "X".into(),
        cell: None,
    });
    assert!(r.is_err(), "offset 4 splits é (bytes 3..5): refused");
    assert_eq!(text(&m, &story), before, "nothing changed");
    // On the boundary after é it works.
    m.apply_mutation(&Mutation::InsertText {
        story_id: story.clone(),
        offset: 5,
        text: "X".into(),
        cell: None,
    })
    .expect("a boundary offset still inserts");
    assert_eq!(text(&m, &story), "CaféX au lait");
}

#[test]
fn a_delete_through_a_character_is_refused() {
    // "€" is three bytes (2..5).
    let (mut m, story) = story_with("5 € only");
    let before = text(&m, &story);
    for (start, end) in [(3, 6), (0, 3), (3, 4)] {
        let r = m.apply_mutation(&Mutation::DeleteRange {
            story_id: story.clone(),
            start,
            end,
            cell: None,
        });
        assert!(r.is_err(), "[{start}, {end}) cuts through € : refused");
        assert_eq!(text(&m, &story), before, "nothing changed");
    }
    m.apply_mutation(&Mutation::DeleteRange {
        story_id: story.clone(),
        start: 2,
        end: 5,
        cell: None,
    })
    .expect("deleting the whole character works");
    assert_eq!(text(&m, &story), "5  only");
}
