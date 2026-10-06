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

//! A paragraph ADDRESS (wire v65, RFI C-53): `applyStyle` and
//! `insertAnchoredFrame` can name a paragraph by its index. In the
//! contiguous character space these ops address, an empty paragraph has
//! no characters, so several in a row share one offset: a caret styled
//! them all at once (and was refused when they differed), and a picture
//! alone in its paragraph landed in a neighbour. Word documents have
//! both (plugin-doc acceptance, 13 blank lines of different heights in a
//! row; a seal alone on its line).

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation, PageId};
use paged_mutate::operation::StyleScope;
const H: &str = "ParagraphStyle/h";
const K: &str = "ParagraphStyle/k";

fn story_with(text: &str) -> (CanvasModel, String) {
    let idml = paged_canvas::blank::blank_idml(612.0, 792.0);
    let inter = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("read Inter.ttf");
    let opts = CanvasOptions {
        fonts: vec![inter],
        ..CanvasOptions::default()
    };
    let mut m = CanvasModel::load("doc", &idml, opts).expect("load");
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
    if !text.is_empty() {
        m.apply_mutation(&Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: text.into(),
            cell: None,
        })
        .expect("insert text");
    }
    m.apply_mutation(&Mutation::CreateParagraphStyle {
        self_id: Some(H.into()),
        name: Some("H".into()),
        based_on: None,
    })
    .expect("style");
    (m, story)
}

fn styles(m: &CanvasModel, story: &str) -> Vec<(String, Option<String>)> {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story");
    s.story
        .paragraphs
        .iter()
        .map(|p| {
            (
                p.runs.iter().map(|r| r.text.as_str()).collect(),
                p.paragraph_style.clone(),
            )
        })
        .collect()
}

fn style_at(m: &mut CanvasModel, story: &str, paragraph: u32, style: &str) -> bool {
    m.apply_mutation(&Mutation::ApplyStyle {
        story_id: story.into(),
        start: 0,
        end: 0,
        style: style.into(),
        scope: StyleScope::Paragraph,
        cell: None,
        paragraph: Some(paragraph),
    })
    .is_ok()
}

#[test]
fn blank_lines_in_a_row_take_different_styles_and_undo_one_by_one() {
    let (mut m, story) = story_with("A\n\n\n\nB");
    m.apply_mutation(&Mutation::CreateParagraphStyle {
        self_id: Some(K.into()),
        name: Some("K".into()),
        based_on: None,
    })
    .expect("style K");
    let before = styles(&m, &story);
    assert_eq!(before.len(), 5);

    assert!(style_at(&mut m, &story, 1, H), "paragraph 1 takes H");
    assert!(style_at(&mut m, &story, 2, K), "paragraph 2 takes K");
    // The third blank line now differs from its neighbours: a caret could
    // not have named it.
    assert!(style_at(&mut m, &story, 3, H), "paragraph 3 takes H");
    let got: Vec<Option<String>> = styles(&m, &story).into_iter().map(|p| p.1).collect();
    assert_eq!(
        got[1..4],
        [
            Some(H.to_string()),
            Some(K.to_string()),
            Some(H.to_string())
        ]
    );
    assert_eq!(got[0], before[0].1, "A is untouched");
    assert_eq!(got[4], before[4].1, "B is untouched");

    m.undo().expect("undo 3");
    let got: Vec<Option<String>> = styles(&m, &story).into_iter().map(|p| p.1).collect();
    assert_eq!(got[3], before[3].1, "undo restores paragraph 3 alone");
    assert_eq!(got[2], Some(K.to_string()), "paragraph 2 keeps K");
    m.undo().expect("undo 2");
    m.undo().expect("undo 1");
    assert_eq!(styles(&m, &story), before, "all three undone");
}

#[test]
fn a_paragraph_address_past_the_story_or_at_character_scope_is_refused() {
    let (mut m, story) = story_with("A\n\nB");
    assert!(!style_at(&mut m, &story, 3, H), "there is no paragraph 3");
    let refused = m
        .apply_mutation(&Mutation::ApplyStyle {
            story_id: story.clone(),
            start: 0,
            end: 0,
            style: "CharacterStyle/x".into(),
            scope: StyleScope::Character,
            cell: None,
            paragraph: Some(0),
        })
        .is_err();
    assert!(refused, "a paragraph address takes a paragraph style");
}

fn frames_per_paragraph(m: &CanvasModel, story: &str) -> Vec<usize> {
    m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story")
        .story
        .paragraphs
        .iter()
        .map(|p| p.anchored_frames.len())
        .collect()
}

#[test]
fn a_picture_alone_in_its_paragraph_anchors_there_through_undo_and_redo() {
    let (mut m, story) = story_with("A\n\n\nB");
    m.apply_mutation(&Mutation::InsertAnchoredFrame {
        story_id: story.clone(),
        offset: 0,
        width: 40.0,
        height: 30.0,
        image_uri: None,
        paragraph: Some(2),
    })
    .expect("anchor in the second blank paragraph");
    assert_eq!(frames_per_paragraph(&m, &story), [0, 0, 1, 0]);
    m.undo().expect("undo");
    assert_eq!(frames_per_paragraph(&m, &story), [0, 0, 0, 0]);
    m.redo().expect("redo");
    assert_eq!(frames_per_paragraph(&m, &story), [0, 0, 1, 0]);

    // The blank line holding it is as tall as the picture: B moves down.
    let b = |m: &CanvasModel| {
        m.built()
            .story_layout(&story)
            .iter()
            .find(|l| l.paragraph_idx == 3)
            .expect("line B")
            .baseline_y_pt
    };
    let with_picture = b(&m);
    m.undo().expect("undo again");
    let without = b(&m);
    assert!(
        with_picture > without + 10.0,
        "the picture takes room in its own paragraph ({with_picture} vs {without})"
    );
}
