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

//! ADR 028 — the keep options (`KeepAllLinesTogether`, `KeepFirstLines`,
//! `KeepLastLines`) and the break-before rule (`StartParagraph`) on the
//! wire. The model carried them and the renderer honoured them, but no
//! surface could set them: a document authored over the wire could never
//! ask for "start in next frame" or widow control. Each path must apply at
//! paragraph AND style level, undo, and — for the start rule — actually
//! move the paragraph in the built layout.

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation};
use paged_gen::ids::self_id;
use paged_gen::samples::start_paragraph::{body_story_id, cases};
use paged_mutate::{PropertyPath as P, StyleCollection, Value as V};

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

fn start_paragraph_model() -> CanvasModel {
    let idml = paged_gen::write_idml(&paged_gen::samples::start_paragraph::build()).expect("idml");
    CanvasModel::load(
        "doc",
        &idml,
        CanvasOptions {
            fonts: vec![inter_font()],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

/// The `[start, end)` character range of paragraph `idx` (0-based) of
/// `story`, in the paragraph setter's own offset convention (run chars,
/// summed over the preceding paragraphs).
fn paragraph_range(m: &CanvasModel, story: &str, idx: usize) -> (u32, u32) {
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .unwrap_or_else(|| panic!("story {story} not in scene"));
    let len = |p: &paged_model::Paragraph| -> u32 {
        p.runs.iter().map(|r| r.text.chars().count() as u32).sum()
    };
    let start: u32 = s.story.paragraphs[..idx].iter().map(len).sum();
    (start, start + len(&s.story.paragraphs[idx]))
}

fn set_paragraph(m: &mut CanvasModel, story: &str, idx: usize, path: P, value: V) {
    let (start, end) = paragraph_range(m, story, idx);
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::StoryRange {
            story_id: story.to_string(),
            start,
            end,
        },
        path,
        value,
    })
    .unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
}

/// The frame the first line of paragraph `idx` (0-based) was laid into.
fn frame_of(m: &CanvasModel, story: &str, idx: usize) -> String {
    m.built()
        .story_layout(story)
        .iter()
        .find(|l| l.paragraph_idx == idx as u32)
        .and_then(|l| l.frame_id.clone())
        .unwrap_or_else(|| panic!("paragraph {idx} of {story} was not laid out"))
}

#[test]
fn start_paragraph_next_frame_over_the_wire_moves_the_paragraph() {
    // Case 0 is the control: eight one-line paragraphs, all in frame A.
    assert!(cases()[0].name.starts_with("control"));
    let mut m = start_paragraph_model();
    let story = body_story_id(0);
    let frame = |f: &str| self_id("start-paragraph", &format!("Frame{f}"), 0);
    // P04 is paragraph index 3.
    assert_eq!(frame_of(&m, &story, 3), frame("A"), "control lands in A");

    set_paragraph(
        &mut m,
        &story,
        3,
        P::ParagraphStartParagraph,
        V::Text("NextFrame".into()),
    );
    assert_eq!(
        frame_of(&m, &story, 3),
        frame("B"),
        "NextFrame moves P04 to frame B"
    );
    assert_eq!(frame_of(&m, &story, 2), frame("A"), "P03 stays in A");

    m.undo().expect("undo");
    assert_eq!(frame_of(&m, &story, 3), frame("A"), "undo puts P04 back");

    // An unknown rule is refused and changes nothing.
    let (start, end) = paragraph_range(&m, &story, 3);
    let err = m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::StoryRange {
            story_id: story.clone(),
            start,
            end,
        },
        path: P::ParagraphStartParagraph,
        value: V::Text("NextSpread".into()),
    });
    assert!(err.is_err(), "unknown StartParagraph must be refused");
    assert_eq!(frame_of(&m, &story, 3), frame("A"));
}

#[test]
fn keep_and_start_paths_read_back_at_paragraph_level() {
    let mut m = start_paragraph_model();
    let story = body_story_id(0);
    let edits = [
        (P::ParagraphKeepAllLinesTogether, V::Bool(true)),
        (P::ParagraphKeepFirstLines, V::Length(Some(3.0))),
        (P::ParagraphKeepLastLines, V::Length(Some(4.0))),
        (P::ParagraphStartParagraph, V::Text("NextColumn".into())),
    ];
    for (path, value) in edits.clone() {
        set_paragraph(&mut m, &story, 1, path, value);
    }
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story");
    let p = &s.story.paragraphs[1];
    assert_eq!(p.keep_all_lines_together, Some(true));
    assert_eq!(p.keep_first_lines, Some(3));
    assert_eq!(p.keep_last_lines, Some(4));
    assert_eq!(
        p.start_paragraph,
        Some(paged_model::StartParagraph::NextColumn)
    );
    for _ in 0..edits.len() {
        m.undo().expect("undo");
    }
    let s = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story");
    let p = &s.story.paragraphs[1];
    assert_eq!(
        p.keep_all_lines_together,
        Some(false),
        "undo → IDML default"
    );
    assert_eq!(p.keep_first_lines, None);
    assert_eq!(p.keep_last_lines, None);
    assert_eq!(p.start_paragraph, None);
}

#[test]
fn paragraph_styles_take_the_keep_and_start_paths() {
    let mut m = start_paragraph_model();
    m.apply_mutation(&Mutation::CreateParagraphStyle {
        self_id: Some("ParagraphStyle/k".into()),
        name: Some("K".into()),
        based_on: None,
    })
    .expect("paragraph style");
    let edits = [
        (P::ParagraphKeepAllLinesTogether, V::Bool(true)),
        (P::ParagraphKeepFirstLines, V::Length(Some(3.0))),
        (P::ParagraphKeepLastLines, V::Length(Some(4.0))),
        (P::ParagraphStartParagraph, V::Text("NextPage".into())),
    ];
    for (path, value) in edits.clone() {
        m.apply_mutation(&Mutation::SetStyleProperty {
            collection: StyleCollection::Paragraph,
            style_id: "ParagraphStyle/k".into(),
            path,
            value,
        })
        .unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
    }
    let def = &m.scene().styles.paragraph_styles["ParagraphStyle/k"];
    assert_eq!(def.keep_all_lines_together, Some(true));
    assert_eq!(def.keep_first_lines, Some(3));
    assert_eq!(def.keep_last_lines, Some(4));
    assert_eq!(
        def.start_paragraph,
        Some(paged_model::StartParagraph::NextPage)
    );

    // An unknown rule is refused at style level too (not silently dropped).
    let err = m.apply_mutation(&Mutation::SetStyleProperty {
        collection: StyleCollection::Paragraph,
        style_id: "ParagraphStyle/k".into(),
        path: P::ParagraphStartParagraph,
        value: V::Text("Sometimes".into()),
    });
    assert!(err.is_err());
    assert_eq!(
        m.scene().styles.paragraph_styles["ParagraphStyle/k"].start_paragraph,
        Some(paged_model::StartParagraph::NextPage)
    );

    for _ in 0..edits.len() {
        m.undo().expect("undo a style edit");
    }
    let def = &m.scene().styles.paragraph_styles["ParagraphStyle/k"];
    assert_eq!(def.keep_all_lines_together, Some(false));
    assert_eq!(def.keep_first_lines, None);
    assert_eq!(def.keep_last_lines, None);
    assert_eq!(def.start_paragraph, None);
}
