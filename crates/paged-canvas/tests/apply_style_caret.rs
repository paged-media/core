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

//! A zero-length `applyStyle` at paragraph scope is a caret: it styles the
//! paragraph the caret stands in. plugin-doc styles every Word paragraph
//! over its character range, and an empty Word paragraph (a blank line)
//! has a zero-length one. The engine refused it ("empty range"), so blank
//! lines kept the default style and leading.
//!
//! Offsets are in ApplyStyle's CONTIGUOUS character space (run text only,
//! no paragraph-break character), where an empty paragraph sits at the
//! offset the next paragraph starts at.

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation, PageId};
use paged_mutate::operation::StyleScope;

const H: &str = "ParagraphStyle/h";

/// A blank document with one minted frame whose story holds `text`
/// (inserted in insertText's own space, where `\n` is a paragraph break).
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

fn apply(m: &mut CanvasModel, story: &str, at: u32, style: &str, scope: StyleScope) -> bool {
    m.apply_mutation(&Mutation::ApplyStyle {
        paragraph: None,
        story_id: story.into(),
        start: at,
        end: at,
        style: style.into(),
        scope,
        cell: None,
    })
    .is_ok()
}

/// Apply H at a caret, assert exactly paragraph `want` changed, undo, and
/// assert everything is back.
fn caret_styles_only(text: &str, at: u32, want: usize) {
    let (mut m, story) = story_with(text);
    let before = styles(&m, &story);
    assert!(
        apply(&mut m, &story, at, H, StyleScope::Paragraph),
        "{text:?} @ {at}: refused"
    );
    let after = styles(&m, &story);
    assert_eq!(after.len(), before.len());
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        if i == want {
            assert_eq!(a.1.as_deref(), Some(H), "{text:?} @ {at}: paragraph {i}");
        } else {
            assert_eq!(a, b, "{text:?} @ {at}: paragraph {i} must not change");
        }
    }
    m.undo().expect("undo");
    assert_eq!(styles(&m, &story), before, "{text:?} @ {at}: undo");
}

#[test]
fn an_empty_paragraph_between_two_takes_the_style() {
    // "A" [0,1)  "" [1,1)  "B" [1,2)
    caret_styles_only("A\n\nB", 1, 1);
}

#[test]
fn an_empty_first_paragraph_takes_the_style() {
    // "" [0,0)  "A" [0,1)
    caret_styles_only("\nA", 0, 0);
}

#[test]
fn an_empty_last_paragraph_takes_the_style() {
    // "A" [0,1)  "" [1,1)
    caret_styles_only("A\n", 1, 1);
}

#[test]
fn a_story_that_is_one_empty_paragraph_takes_the_style() {
    // The Word skeleton's initial state: nothing inserted yet.
    let (m, story) = story_with("");
    assert_eq!(styles(&m, &story).len(), 1, "premise: one empty paragraph");
    caret_styles_only("", 0, 0);
}

#[test]
fn a_caret_inside_text_styles_its_paragraph() {
    caret_styles_only("AB\nCD", 1, 0);
    caret_styles_only("AB\nCD", 3, 1);
    // At the very end of the story: the last paragraph.
    caret_styles_only("AB\nCD", 4, 1);
}

#[test]
fn ranges_around_an_empty_paragraph_leave_it_alone() {
    // plugin-doc styles the neighbours over their own ranges; neither
    // range may reach the blank line between them.
    let (mut m, story) = story_with("A\n\nB");
    for (s, e) in [(0, 1), (1, 2)] {
        m.apply_mutation(&Mutation::ApplyStyle {
            paragraph: None,
            story_id: story.clone(),
            start: s,
            end: e,
            style: H.into(),
            scope: StyleScope::Paragraph,
            cell: None,
        })
        .expect("range");
    }
    let got: Vec<_> = styles(&m, &story).into_iter().map(|p| p.1).collect();
    assert_eq!(got[0].as_deref(), Some(H));
    assert_ne!(got[1].as_deref(), Some(H), "the blank line was reached");
    assert_eq!(got[2].as_deref(), Some(H));
}

#[test]
fn consecutive_blank_lines_share_a_caret() {
    // "A" [0,1)  "" [1,1)  "" [1,1)  "B" [1,2): the two blank lines are
    // the same contiguous offset, so a caret writes both.
    let (mut m, story) = story_with("A\n\n\nB");
    let before = styles(&m, &story);
    assert!(apply(&mut m, &story, 1, H, StyleScope::Paragraph));
    let got: Vec<_> = styles(&m, &story).into_iter().map(|p| p.1).collect();
    assert_eq!(got[1].as_deref(), Some(H));
    assert_eq!(got[2].as_deref(), Some(H));
    assert_eq!(got[0], before[0].1);
    assert_eq!(got[3], before[3].1);
    m.undo().expect("undo");
    assert_eq!(styles(&m, &story), before);
}

#[test]
fn blank_lines_that_disagree_refuse_a_caret_rather_than_flatten_undo() {
    let (mut m, story) = story_with("A\n\n\nB");
    // No range can name one of two blank lines, so author the difference
    // through text: fill the first, style it, empty it again.
    m.apply_mutation(&Mutation::InsertText {
        story_id: story.clone(),
        offset: 2, // byte space: "A" 0..1, break, blank line 1 at byte 2
        text: "x".into(),
        cell: None,
    })
    .expect("x");
    // Contiguous: "A" [0,1) "x" [1,2) "" [2,2) "B" [2,3).
    m.apply_mutation(&Mutation::ApplyStyle {
        paragraph: None,
        story_id: story.clone(),
        start: 1,
        end: 2,
        style: H.into(),
        scope: StyleScope::Paragraph,
        cell: None,
    })
    .expect("style the x line");
    m.apply_mutation(&Mutation::DeleteRange {
        story_id: story.clone(),
        start: 2,
        end: 3,
        cell: None,
    })
    .expect("delete x");
    let before = styles(&m, &story);
    assert_eq!(before[1].0, "", "premise: blank again");
    assert_eq!(before[1].1.as_deref(), Some(H), "premise: styled");
    assert_ne!(before[2].1.as_deref(), Some(H), "premise: the other is not");
    assert!(
        !apply(
            &mut m,
            &story,
            1,
            "ParagraphStyle/$ID/NormalParagraphStyle",
            StyleScope::Paragraph
        ),
        "two blank lines with different styles at one caret must refuse"
    );
    assert_eq!(styles(&m, &story), before, "a refusal changes nothing");
}

#[test]
fn a_caret_at_character_scope_stays_refused() {
    // A character style needs characters. InDesign would set the caret's
    // typing attributes, which the model has no place for, so a silent
    // success would claim an effect that never happens.
    let (mut m, story) = story_with("A\n\nB");
    let before = styles(&m, &story);
    m.apply_mutation(&Mutation::CreateCharacterStyle {
        self_id: Some("CharacterStyle/c".into()),
        name: Some("C".into()),
        based_on: None,
    })
    .expect("char style");
    assert!(!apply(
        &mut m,
        &story,
        1,
        "CharacterStyle/c",
        StyleScope::Character
    ));
    assert_eq!(styles(&m, &story), before);
}

#[test]
fn a_caret_past_the_end_is_refused() {
    let (mut m, story) = story_with("AB");
    assert!(!apply(&mut m, &story, 3, H, StyleScope::Paragraph));
}

#[test]
fn a_range_over_a_blank_line_undoes() {
    // The inverse restores each paragraph at its own range — for the
    // blank line, a caret. That replay was refused before carets were
    // addresses, so undo failed.
    let (mut m, story) = story_with("A\n\nB");
    let before = styles(&m, &story);
    m.apply_mutation(&Mutation::ApplyStyle {
        paragraph: None,
        story_id: story.clone(),
        start: 0,
        end: 2,
        style: H.into(),
        scope: StyleScope::Paragraph,
        cell: None,
    })
    .expect("range");
    assert!(styles(&m, &story).iter().all(|p| p.1.as_deref() == Some(H)));
    m.undo().expect("undo");
    assert_eq!(styles(&m, &story), before);
}
