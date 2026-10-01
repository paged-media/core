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

//! Span and split columns (`SpanColumnType`, `SpanSplitColumnCount`,
//! `SpanColumnMinSpaceBefore` / `After`, `SplitColumnInsideGutter` /
//! `OutsideGutter`) on the wire. The model carried them (ef4cb28) and the
//! pipeline honoured them (a3e5a0c), but no surface could set them. Each
//! path must apply at paragraph AND style level, read back, undo, and —
//! for the span itself — actually widen the paragraph's line in the built
//! layout.

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation};
use paged_gen::samples::span_columns::{body_story_id, cases, COLUMN_W, FRAME_X, GUTTER};
use paged_model::{SpanColumnType, SpanColumns, SpanSplitColumnCount};
use paged_mutate::{PropertyPath as P, StyleCollection, Value as V};

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

fn span_columns_model() -> CanvasModel {
    let idml = paged_gen::write_idml(&paged_gen::samples::span_columns::build()).expect("idml");
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
/// `story`, in the paragraph setter's offset convention (run chars,
/// summed over the preceding paragraphs).
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

fn span_of(m: &CanvasModel, story: &str, idx: usize) -> SpanColumns {
    m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .expect("story")
        .story
        .paragraphs[idx]
        .span_columns
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

/// The width of the line box paragraph `idx`'s first line was centred
/// in: with `CenterAlign`, the line's centre sits half the line width
/// from the frame's left edge (the frame has zero insets).
fn centred_line_width(m: &CanvasModel, story: &str, idx: usize) -> f32 {
    let built = m.built();
    let line = built
        .story_layout(story)
        .into_iter()
        .find(|l| l.paragraph_idx == idx as u32)
        .unwrap_or_else(|| panic!("paragraph {idx} of {story} was not laid out"));
    let left = line
        .clusters
        .iter()
        .map(|c| c.x_pt)
        .fold(f32::INFINITY, f32::min);
    let right = line
        .clusters
        .iter()
        .map(|c| c.x_pt + c.advance_pt)
        .fold(f32::NEG_INFINITY, f32::max);
    2.0 * ((left + right) / 2.0 - FRAME_X)
}

#[test]
fn span_columns_over_the_wire_widens_the_line_and_undo_narrows_it() {
    // Case 0: a two-column frame; P01 (paragraph 1) is a plain
    // single-column paragraph.
    assert_eq!(cases()[0].columns, 2);
    let mut m = span_columns_model();
    let story = body_story_id(0);
    assert_eq!(span_of(&m, &story, 1), SpanColumns::default());
    // Centre it, so where its line sits says how wide its line box is.
    set_paragraph(
        &mut m,
        &story,
        1,
        P::ParagraphJustification,
        V::Text("CenterAlign".into()),
    );
    let one_column = centred_line_width(&m, &story, 1);
    assert!(
        (one_column - COLUMN_W).abs() < 1.0,
        "single column: line box {one_column} pt, want {COLUMN_W}"
    );

    // The count alone does nothing while the type is SingleColumn. A
    // bare count arrives as a Length (a script's `2`) and reads back as
    // the IDML text.
    set_paragraph(
        &mut m,
        &story,
        1,
        P::ParagraphSpanSplitColumnCount,
        V::Length(Some(2.0)),
    );
    assert_eq!(
        read_back(&m, &story, 1, P::ParagraphSpanSplitColumnCount),
        Some(V::Text("2".into()))
    );
    assert!((centred_line_width(&m, &story, 1) - one_column).abs() < 0.01);

    set_paragraph(
        &mut m,
        &story,
        1,
        P::ParagraphSpanColumnType,
        V::Text("SpanColumns".into()),
    );
    assert_eq!(
        read_back(&m, &story, 1, P::ParagraphSpanColumnType),
        Some(V::Text("SpanColumns".into()))
    );
    let spanned = centred_line_width(&m, &story, 1);
    let two_columns = 2.0 * COLUMN_W + GUTTER;
    assert!(
        (spanned - two_columns).abs() < 1.0,
        "spanning 2: line box {spanned} pt, want {two_columns}"
    );

    m.undo().expect("undo the span");
    assert_eq!(span_of(&m, &story, 1).column_type, None);
    let restored = centred_line_width(&m, &story, 1);
    assert!(
        (restored - one_column).abs() < 0.01,
        "undo restores the single-column line box: {restored} vs {one_column}"
    );

    // An unknown type is refused and changes nothing.
    let element_id = paragraph_element(&m, &story, 1);
    let err = m.apply_mutation(&Mutation::SetElementProperty {
        element_id,
        path: P::ParagraphSpanColumnType,
        value: V::Text("SpanAll".into()),
    });
    assert!(err.is_err(), "unknown SpanColumnType must be refused");
    assert_eq!(span_of(&m, &story, 1).column_type, None);
}

#[test]
fn span_columns_paths_read_back_and_undo_at_paragraph_level() {
    let mut m = span_columns_model();
    let story = body_story_id(0);
    let edits = [
        (P::ParagraphSpanColumnType, V::Text("SplitColumns".into())),
        (P::ParagraphSpanSplitColumnCount, V::Text("3".into())),
        (P::ParagraphSpanColumnMinSpaceBefore, V::Length(Some(6.0))),
        (P::ParagraphSpanColumnMinSpaceAfter, V::Length(Some(10.0))),
        (P::ParagraphSplitColumnInsideGutter, V::Length(Some(20.0))),
        (P::ParagraphSplitColumnOutsideGutter, V::Length(Some(4.0))),
    ];
    for (path, value) in edits.clone() {
        set_paragraph(&mut m, &story, 2, path, value);
    }
    assert_eq!(
        span_of(&m, &story, 2),
        SpanColumns {
            column_type: Some(SpanColumnType::SplitColumns),
            count: Some(SpanSplitColumnCount::Count(3)),
            min_space_before: Some(6.0),
            min_space_after: Some(10.0),
            inside_gutter: Some(20.0),
            outside_gutter: Some(4.0),
        }
    );
    for (path, value) in edits.clone() {
        assert_eq!(read_back(&m, &story, 2, path), Some(value), "{path:?}");
    }

    // "All" and the empty string (clear) on the count.
    set_paragraph(
        &mut m,
        &story,
        2,
        P::ParagraphSpanSplitColumnCount,
        V::Text("All".into()),
    );
    assert_eq!(
        span_of(&m, &story, 2).count,
        Some(SpanSplitColumnCount::All)
    );
    set_paragraph(
        &mut m,
        &story,
        2,
        P::ParagraphSpanSplitColumnCount,
        V::Text(String::new()),
    );
    assert_eq!(span_of(&m, &story, 2).count, None);
    assert_eq!(
        read_back(&m, &story, 2, P::ParagraphSpanSplitColumnCount),
        Some(V::Text(String::new()))
    );
    m.undo().expect("undo clear");
    assert_eq!(
        span_of(&m, &story, 2).count,
        Some(SpanSplitColumnCount::All)
    );
    m.undo().expect("undo All");
    assert_eq!(
        span_of(&m, &story, 2).count,
        Some(SpanSplitColumnCount::Count(3))
    );

    // Counts that are not a whole number >= 1 are refused, slot untouched.
    for bad in [
        V::Text("0".into()),
        V::Text("two".into()),
        V::Length(Some(0.0)),
        V::Length(Some(2.5)),
        V::Bool(true),
    ] {
        let element_id = paragraph_element(&m, &story, 2);
        let err = m.apply_mutation(&Mutation::SetElementProperty {
            element_id,
            path: P::ParagraphSpanSplitColumnCount,
            value: bad.clone(),
        });
        assert!(err.is_err(), "{bad:?} must be refused");
        assert_eq!(
            span_of(&m, &story, 2).count,
            Some(SpanSplitColumnCount::Count(3))
        );
    }

    for _ in 0..edits.len() {
        m.undo().expect("undo");
    }
    assert_eq!(span_of(&m, &story, 2), SpanColumns::default());
}

#[test]
fn paragraph_styles_take_the_span_columns_paths() {
    let mut m = span_columns_model();
    m.apply_mutation(&Mutation::CreateParagraphStyle {
        self_id: Some("ParagraphStyle/span".into()),
        name: Some("Span".into()),
        based_on: None,
    })
    .expect("paragraph style");
    let set_style = |m: &mut CanvasModel, path: P, value: V| {
        m.apply_mutation(&Mutation::SetStyleProperty {
            collection: StyleCollection::Paragraph,
            style_id: "ParagraphStyle/span".into(),
            path,
            value,
        })
    };
    let edits = [
        (P::ParagraphSpanColumnType, V::Text("SpanColumns".into())),
        (P::ParagraphSpanSplitColumnCount, V::Text("All".into())),
        (P::ParagraphSpanColumnMinSpaceBefore, V::Length(Some(6.0))),
        (P::ParagraphSpanColumnMinSpaceAfter, V::Length(Some(10.0))),
        (P::ParagraphSplitColumnInsideGutter, V::Length(Some(12.0))),
        (P::ParagraphSplitColumnOutsideGutter, V::Length(Some(2.0))),
    ];
    for (path, value) in edits.clone() {
        set_style(&mut m, path, value).unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
    }
    let want = SpanColumns {
        column_type: Some(SpanColumnType::SpanColumns),
        count: Some(SpanSplitColumnCount::All),
        min_space_before: Some(6.0),
        min_space_after: Some(10.0),
        inside_gutter: Some(12.0),
        outside_gutter: Some(2.0),
    };
    let style_span =
        |m: &CanvasModel| m.scene().styles.paragraph_styles["ParagraphStyle/span"].span_columns;
    assert_eq!(style_span(&m), want);

    // Unknown values are refused at style level too (not silently dropped).
    assert!(set_style(&mut m, P::ParagraphSpanColumnType, V::Text("Wide".into())).is_err());
    assert!(set_style(
        &mut m,
        P::ParagraphSpanSplitColumnCount,
        V::Text("0".into())
    )
    .is_err());
    assert_eq!(style_span(&m), want);

    for _ in 0..edits.len() {
        m.undo().expect("undo a style edit");
    }
    assert_eq!(style_span(&m), SpanColumns::default());
}
