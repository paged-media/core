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

//! The list-marker attributes (`BulletsTextAfter`, `NumberingExpression`,
//! `NumberingStartAt`, `NumberingContinue`, `BulletsCharacterStyle`,
//! `NumberingCharacterStyle`) on the wire. The paragraph carried them
//! (d10ffc9) and the pipeline honoured them (6192c72), but no surface
//! could set them. Each path must apply at paragraph AND style level,
//! read back, undo, and — for the text after a bullet — actually move the
//! text in the built layout.

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation};
use paged_gen::samples::list_markers::{body_story_id, cases, frame_origin, STYLE_AFTER_TAB};
use paged_mutate::{PropertyPath as P, StyleCollection, Value as V};

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

fn list_markers_model() -> CanvasModel {
    let idml = paged_gen::write_idml(&paged_gen::samples::list_markers::build()).expect("idml");
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

/// The `StoryRange` element covering paragraph `idx` (0-based) of
/// `story`, in the paragraph setter's offset convention.
fn paragraph_element(m: &CanvasModel, story: &str, idx: usize) -> ElementId {
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
    ElementId::StoryRange {
        story_id: story.to_string(),
        start,
        end: start + len(&s.story.paragraphs[idx]),
    }
}

fn set_paragraph(m: &mut CanvasModel, story: &str, idx: usize, path: P, value: V) {
    let element_id = paragraph_element(m, story, idx);
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id,
        path,
        value,
    })
    .unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
}

fn paragraph<'a>(m: &'a CanvasModel, story: &str, idx: usize) -> &'a paged_model::Paragraph {
    &m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story")
        .story
        .paragraphs[idx]
}

fn read_back(m: &CanvasModel, story: &str, idx: usize, path: P) -> Option<V> {
    let props = m
        .element_properties(&paragraph_element(m, story, idx))
        .expect("story-range properties");
    props
        .entries
        .iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("no {path:?} entry"))
        .value
        .clone()
}

/// Frame-local pen x of the case tag (the first word after the marker)
/// on paragraph `idx`'s line. Each paragraph is `"<marker><tag> one|two"`
/// on one line, so the tag sits 7 bytes before the line's end.
fn tag_x(m: &CanvasModel, case: u32, idx: usize) -> f32 {
    let (fx, _) = frame_origin(case);
    let built = m.built();
    let line = built
        .story_layout(&body_story_id(case))
        .into_iter()
        .find(|l| l.paragraph_idx == idx as u32)
        .unwrap_or_else(|| panic!("paragraph {idx} of case {case} was not laid out"));
    let byte = line.byte_range.end - 7;
    line.clusters
        .iter()
        .find(|c| c.byte == byte)
        .map(|c| c.x_pt - fx)
        .unwrap_or_else(|| panic!("no cluster at byte {byte}"))
}

#[test]
fn bullets_text_after_over_the_wire_moves_the_text_and_undo_restores_it() {
    // Case 1 (c01): a bullet under a style that says
    // `BulletsTextAfter="^t"`, hanging indent 18 — the text starts at 18.
    let c = cases();
    assert_eq!(c[1].style, Some(STYLE_AFTER_TAB));
    let mut m = list_markers_model();
    let story = body_story_id(1);
    let at_tab = tag_x(&m, 1, 0);
    assert!(
        (at_tab - 18.0).abs() < 0.5,
        "^t from the style: the text starts at the indent, got {at_tab}"
    );

    // A local space beats the style's tab: the text follows the bullet's
    // advance plus one space (InDesign: 8.437, see list_markers_pipeline).
    set_paragraph(
        &mut m,
        &story,
        0,
        P::ParagraphBulletsTextAfter,
        V::Text(" ".into()),
    );
    assert_eq!(
        read_back(&m, &story, 0, P::ParagraphBulletsTextAfter),
        Some(V::Text(" ".into()))
    );
    let at_space = tag_x(&m, 1, 0);
    assert!(
        (at_space - 8.437).abs() < 0.5,
        "a space after the bullet: the text moves left, got {at_space}"
    );
    // Only the paragraph that was set moved.
    assert!((tag_x(&m, 1, 1) - at_tab).abs() < 0.01);

    m.undo().expect("undo");
    assert_eq!(paragraph(&m, &story, 0).bullets_text_after, None);
    let restored = tag_x(&m, 1, 0);
    assert!(
        (restored - at_tab).abs() < 0.01,
        "undo restores the tab: {restored} vs {at_tab}"
    );
}

#[test]
fn list_marker_paths_read_back_and_undo_at_paragraph_level() {
    let mut m = list_markers_model();
    // Case 8 (c08): a numbered list.
    let story = body_story_id(8);
    let edits = [
        (P::ParagraphBulletsTextAfter, V::Text("^t".into())),
        (P::ParagraphNumberingExpression, V::Text("(^#)^t".into())),
        (P::ParagraphNumberingStartAt, V::Length(Some(5.0))),
        (P::ParagraphNumberingContinue, V::Bool(false)),
        (
            P::ParagraphBulletsCharacterStyle,
            V::Text("CharacterStyle/Bullet".into()),
        ),
        (
            P::ParagraphNumberingCharacterStyle,
            V::Text("CharacterStyle/Digits".into()),
        ),
    ];
    // Unset slots read back in the setter's clear shapes.
    for (path, _) in &edits {
        let want = match path {
            P::ParagraphNumberingStartAt => V::Length(None),
            _ => V::Text(String::new()),
        };
        assert_eq!(read_back(&m, &story, 0, *path), Some(want), "{path:?}");
    }
    for (path, value) in edits.clone() {
        set_paragraph(&mut m, &story, 0, path, value);
    }
    {
        let p = paragraph(&m, &story, 0);
        assert_eq!(p.bullets_text_after.as_deref(), Some("^t"));
        assert_eq!(p.numbering_expression.as_deref(), Some("(^#)^t"));
        assert_eq!(p.numbering_start_at, Some(5));
        assert_eq!(p.numbering_continue, Some(false));
        assert_eq!(
            p.bullets_character_style.as_deref(),
            Some("CharacterStyle/Bullet")
        );
        assert_eq!(
            p.bullets_and_numbering_digits_character_style.as_deref(),
            Some("CharacterStyle/Digits")
        );
    }
    for (path, value) in edits.clone() {
        assert_eq!(read_back(&m, &story, 0, path), Some(value), "{path:?}");
    }

    // A start that is not a whole number >= 1 is refused, slot untouched.
    for bad in [
        V::Length(Some(0.0)),
        V::Length(Some(-1.0)),
        V::Length(Some(2.5)),
    ] {
        let element_id = paragraph_element(&m, &story, 0);
        let err = m.apply_mutation(&Mutation::SetElementProperty {
            element_id,
            path: P::ParagraphNumberingStartAt,
            value: bad.clone(),
        });
        assert!(err.is_err(), "{bad:?} must be refused");
        assert_eq!(paragraph(&m, &story, 0).numbering_start_at, Some(5));
    }

    for _ in 0..edits.len() {
        m.undo().expect("undo");
    }
    let p = paragraph(&m, &story, 0);
    assert_eq!(p.bullets_text_after, None);
    assert_eq!(p.numbering_expression, None);
    assert_eq!(p.numbering_start_at, None);
    // Unset, not `Some(true)`: the two count differently.
    assert_eq!(p.numbering_continue, None);
    assert_eq!(p.bullets_character_style, None);
    assert_eq!(p.bullets_and_numbering_digits_character_style, None);
}

#[test]
fn paragraph_styles_take_the_list_marker_paths() {
    let mut m = list_markers_model();
    m.apply_mutation(&Mutation::CreateParagraphStyle {
        self_id: Some("ParagraphStyle/list".into()),
        name: Some("List".into()),
        based_on: None,
    })
    .expect("paragraph style");
    let set_style = |m: &mut CanvasModel, path: P, value: V| {
        m.apply_mutation(&Mutation::SetStyleProperty {
            collection: StyleCollection::Paragraph,
            style_id: "ParagraphStyle/list".into(),
            path,
            value,
        })
    };
    let edits = [
        (P::ParagraphBulletsTextAfter, V::Text(" ".into())),
        (P::ParagraphNumberingExpression, V::Text("^#)^t".into())),
        (P::ParagraphNumberingStartAt, V::Length(Some(3.0))),
        (P::ParagraphNumberingContinue, V::Bool(false)),
        (
            P::ParagraphBulletsCharacterStyle,
            V::Text("CharacterStyle/Bullet".into()),
        ),
        (
            P::ParagraphNumberingCharacterStyle,
            V::Text("CharacterStyle/Digits".into()),
        ),
    ];
    for (path, value) in edits.clone() {
        set_style(&mut m, path, value).unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
    }
    let snapshot = |m: &CanvasModel| {
        let d = &m.scene().styles.paragraph_styles["ParagraphStyle/list"];
        format!(
            "{:?} {:?} {:?} {:?} {:?} {:?}",
            d.bullets_text_after,
            d.numbering_expression,
            d.numbering_start_at,
            d.numbering_continue,
            d.bullets_character_style,
            d.bullets_and_numbering_digits_character_style
        )
    };
    let want = "Some(\" \") Some(\"^#)^t\") Some(3) Some(false) \
                Some(\"CharacterStyle/Bullet\") Some(\"CharacterStyle/Digits\")";
    assert_eq!(snapshot(&m), want);

    // Bad values are refused at style level too (not clamped or dropped).
    assert!(set_style(&mut m, P::ParagraphNumberingStartAt, V::Length(Some(0.0))).is_err());
    assert!(set_style(&mut m, P::ParagraphNumberingStartAt, V::Length(Some(1.5))).is_err());
    assert!(set_style(&mut m, P::ParagraphNumberingContinue, V::Text("no".into())).is_err());
    assert!(set_style(&mut m, P::ParagraphBulletsTextAfter, V::Bool(true)).is_err());
    assert_eq!(snapshot(&m), want);

    for _ in 0..edits.len() {
        m.undo().expect("undo a style edit");
    }
    assert_eq!(snapshot(&m), "None None None None None None");
}
