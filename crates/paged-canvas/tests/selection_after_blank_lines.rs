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

//! A paragraph's story offset counts the blank paragraphs before it. The
//! offset of a line's paragraph was summed from the paragraphs that HAVE
//! lines, so every blank paragraph before it made the selection, the caret
//! and a click land one character early. Found measuring Word documents
//! (plugin-doc acceptance round 9): each paragraph's selection began on
//! the previous paragraph's last line.

use paged_canvas::{
    caret_geometry, selection_geometry, CanvasModel, CanvasOptions, ContentSelection, ElementId,
    Mutation, PageId,
};

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

fn range(story: &str, start: u32, end: u32) -> ContentSelection {
    ContentSelection {
        story_id: story.into(),
        start,
        end,
        affinity: false,
        cell: None,
    }
}

/// "Abc", two blank lines, "Def": D sits at offset 6.
#[test]
fn a_selection_after_blank_lines_starts_on_its_own_line() {
    let (m, story) = story_with("Abc\n\n\nDef");
    let built = m.built();
    let line = |idx: u32| {
        built
            .story_layout(&story)
            .into_iter()
            .find(|l| l.paragraph_idx == idx)
            .map(|l| l.baseline_y_pt - l.ascent_pt)
    };
    let top_a = line(0).expect("line Abc");
    let top_d = line(3).expect("line Def");
    assert!(top_d > top_a + 20.0, "Def sits below two blank lines");

    let rects = selection_geometry(built, &range(&story, 6, 9));
    assert_eq!(rects.len(), 1, "Def is one line: {rects:?}");
    assert!(
        (rects[0].top_pt - top_d).abs() < 0.01,
        "the selection of Def is on Def's line ({} vs {top_d})",
        rects[0].top_pt
    );
    let whole = selection_geometry(built, &range(&story, 0, 3));
    assert_eq!(whole.len(), 1, "Abc is one line: {whole:?}");
    let d_only = selection_geometry(built, &range(&story, 6, 7));
    let def_line_left = selection_geometry(built, &range(&story, 0, 9))
        .last()
        .expect("a rect on Def's line")
        .left_pt;
    assert!(
        (rects[0].left_pt - def_line_left).abs() < 0.01,
        "Def's selection starts at its line's left edge ({} vs {def_line_left})",
        rects[0].left_pt
    );
    assert!(
        rects[0].width_pt > d_only[0].width_pt * 2.0,
        "Def's selection covers three letters ({} vs D alone {})",
        rects[0].width_pt,
        d_only[0].width_pt
    );

    // The caret before D is at the line's left edge, on Def's line.
    let caret = caret_geometry(built, &range(&story, 6, 6)).expect("caret");
    assert!((caret.top_pt - top_d).abs() < 0.01, "caret on Def's line");
    assert!(
        (caret.x_pt - rects[0].left_pt).abs() < 0.01,
        "caret before D is where its selection starts"
    );
    // And the caret on the second blank line is between the two.
    let blank = caret_geometry(built, &range(&story, 5, 5)).expect("caret on a blank line");
    assert!(
        blank.top_pt > top_a + 1.0 && blank.top_pt < top_d - 1.0,
        "the caret at offset 5 is on the second blank line ({} between {top_a} and {top_d})",
        blank.top_pt
    );
}

/// A list marker is not a character of the story: the caret before the
/// first letter of a bulleted paragraph stands after the marker, and the
/// paragraph after it starts on its own line.
#[test]
fn a_list_marker_does_not_shift_the_offsets() {
    use paged_mutate::operation::StyleScope;
    use paged_mutate::{PropertyPath, StyleCollection, Value};
    let (mut m, story) = story_with("Abc\nDef");
    let style = "ParagraphStyle/bullet";
    m.apply_mutation(&Mutation::CreateParagraphStyle {
        self_id: Some(style.into()),
        name: Some("Bullet".into()),
        based_on: None,
    })
    .expect("style");
    for (path, value) in [
        (
            PropertyPath::ParagraphListType,
            Value::Text("BulletList".into()),
        ),
        (
            PropertyPath::ParagraphBulletCharacter,
            Value::Text("\u{2022}".into()),
        ),
        (PropertyPath::ParagraphLeftIndent, Value::Length(Some(18.0))),
        (
            PropertyPath::ParagraphFirstLineIndent,
            Value::Length(Some(-18.0)),
        ),
    ] {
        m.apply_mutation(&Mutation::SetStyleProperty {
            collection: StyleCollection::Paragraph,
            style_id: style.into(),
            path,
            value,
        })
        .expect("style property");
    }
    let plain_a = selection_geometry(m.built(), &range(&story, 0, 3))[0].clone();
    m.apply_mutation(&Mutation::ApplyStyle {
        story_id: story.clone(),
        start: 0,
        end: 3,
        style: style.into(),
        scope: StyleScope::Paragraph,
        cell: None,
    })
    .expect("bullet on Abc");
    let built = m.built();
    let abc = selection_geometry(built, &range(&story, 0, 3));
    assert_eq!(abc.len(), 1);
    assert!(
        (abc[0].width_pt - plain_a.width_pt).abs() < 0.5,
        "Abc's selection is the three letters, not the marker ({} vs {})",
        abc[0].width_pt,
        plain_a.width_pt
    );
    assert!(
        abc[0].left_pt > plain_a.left_pt + 10.0,
        "and it starts after the marker, at the indent"
    );
    let def = selection_geometry(built, &range(&story, 4, 7));
    assert_eq!(def.len(), 1, "Def is one line: {def:?}");
    assert!(
        def[0].top_pt > abc[0].top_pt + 5.0,
        "Def's selection is on Def's line"
    );
    assert!(
        (def[0].left_pt - plain_a.left_pt).abs() < 0.01,
        "from its left edge ({} vs {})",
        def[0].left_pt,
        plain_a.left_pt
    );
}
