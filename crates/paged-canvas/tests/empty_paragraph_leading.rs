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

//! A blank line takes its leading from its paragraph STYLE. A blank line a
//! plugin pours has no run, and the emitter read the leading only from the
//! first run, so a styled blank line kept auto leading (1.2 × size): Word's
//! 24 pt blank line came out 14.4 pt tall (plugin-doc's doc-line-breaks
//! spec, ADR 029).

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation, PageId};
use paged_mutate::operation::StyleScope;
use paged_mutate::{PropertyPath, StyleCollection, Value};

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
    m.apply_mutation(&Mutation::InsertText {
        story_id: story.clone(),
        offset: 0,
        text: text.into(),
        cell: None,
    })
    .expect("insert text");
    (m, story)
}

/// A paragraph style with `leading` pt, applied to the blank line at the
/// caret; returns the baseline of the line after it.
fn baseline_after_blank(leading: f32) -> f32 {
    // "A", a blank line, "B": the blank line sits at caret offset 1.
    let (mut m, story) = story_with("A\n\nB");
    let style = "ParagraphStyle/blank";
    m.apply_mutation(&Mutation::CreateParagraphStyle {
        self_id: Some(style.into()),
        name: Some("Blank".into()),
        based_on: None,
    })
    .expect("style");
    m.apply_mutation(&Mutation::SetStyleProperty {
        collection: StyleCollection::Paragraph,
        style_id: style.into(),
        path: PropertyPath::CharacterLeading,
        value: Value::Length(Some(leading)),
    })
    .expect("leading");
    m.apply_mutation(&Mutation::ApplyStyle {
        story_id: story.clone(),
        start: 1,
        end: 1,
        style: style.into(),
        scope: StyleScope::Paragraph,
        cell: None,
    })
    .expect("caret style on the blank line");
    let blank = &m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story")
        .story
        .paragraphs[1];
    assert!(
        blank.runs.iter().all(|r| r.text.is_empty()),
        "the middle paragraph is blank"
    );
    m.built()
        .story_layout(&story)
        .iter()
        .find(|l| l.paragraph_idx == 2)
        .expect("line B")
        .baseline_y_pt
}

#[test]
fn a_blank_line_takes_its_style_leading() {
    let tall = baseline_after_blank(50.0);
    let short = baseline_after_blank(30.0);
    assert!(
        (tall - short - 20.0).abs() < 0.01,
        "a 50 pt blank line pushes B 20 pt further than a 30 pt one (got {})",
        tall - short
    );
}
