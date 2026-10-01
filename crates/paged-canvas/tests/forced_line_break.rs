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

//! A plugin inserts a forced line break (Word's plain `<w:br/>`) by
//! sending U+2028 inside `insertText`. `\n` is a paragraph break there;
//! U+2028 is stored as text, so the paragraph stays ONE paragraph for
//! `applyStyle`, read-back and keeps, while layout breaks the line, and
//! IDML export writes it back as InDesign's own forced line break.
//!
//! Offsets: U+2028 is 3 bytes in insertText's byte space and ONE
//! character in applyStyle's contiguous character space.

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation, PageId};
use paged_mutate::operation::StyleScope;
use paged_mutate::{PropertyPath as P, StyleCollection, Value as V};

const TEXT: &str = "Alpha one\u{2028}Bravo two\nCharlie";

fn inter() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/fonts/Inter.ttf"
    ))
    .expect("Inter.ttf")
}

struct Doc {
    m: CanvasModel,
    page: PageId,
    story: String,
}

fn doc() -> Doc {
    let idml = paged_canvas::blank::blank_idml(612.0, 792.0);
    let mut m = CanvasModel::load(
        "doc",
        &idml,
        CanvasOptions {
            fonts: vec![inter()],
            ..Default::default()
        },
    )
    .expect("load");
    let page = PageId(
        m.scene().spreads[0].spread.pages[0]
            .self_id
            .clone()
            .expect("page id"),
    );
    let out = m
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: page.clone(),
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
        text: TEXT.into(),
        cell: None,
    })
    .expect("insert text");
    Doc { m, page, story }
}

fn paragraphs(d: &Doc) -> Vec<(String, Option<String>)> {
    d.m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == d.story)
        .expect("story")
        .story
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

/// `(paragraph_idx, line_idx, first x, baseline)` per laid-out line.
fn lines(d: &Doc) -> Vec<(u32, u32, f32, f32)> {
    d.m.built()
        .story_layout(&d.story)
        .iter()
        .map(|l| {
            (
                l.paragraph_idx,
                l.line_idx,
                l.clusters.first().map(|c| c.x_pt).unwrap_or(f32::NAN),
                l.baseline_y_pt,
            )
        })
        .collect()
}

#[test]
fn u2028_keeps_one_paragraph_and_breaks_the_line() {
    let d = doc();
    let paras = paragraphs(&d);
    assert_eq!(
        paras.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(),
        ["Alpha one\u{2028}Bravo two", "Charlie"],
        "U+2028 is text; only \\n splits the paragraph"
    );
    let l = lines(&d);
    assert_eq!(l.len(), 3, "{l:?}");
    assert_eq!((l[0].0, l[0].1), (0, 0));
    assert_eq!(
        (l[1].0, l[1].1),
        (0, 1),
        "line two belongs to paragraph 0: {l:?}"
    );
    assert_eq!((l[2].0, l[2].1), (1, 0));
    assert!(l[1].3 > l[0].3 + 1.0, "line two is below line one: {l:?}");
    assert_eq!(l[0].2, l[1].2, "both lines start at the frame's left edge");
    // Line two's bytes are the paragraph's: "Alpha one" (9) + U+2028 (3).
    let layout = d.m.built().story_layout(&d.story);
    assert_eq!(layout[1].byte_range.start, 12);
}

#[test]
fn a_click_on_the_broken_line_lands_after_the_break() {
    let d = doc();
    let l = lines(&d);
    let hit = d.m.hit_test(&d.page, (l[1].2 + 0.5, l[1].3 - 3.0));
    assert_eq!(
        hit.offset_within_story,
        Some(12),
        "the caret before 'Bravo' is story byte 12"
    );
}

#[test]
fn apply_style_over_the_paragraph_counts_u2028_as_one_character() {
    let mut d = doc();
    d.m.apply_mutation(&Mutation::CreateParagraphStyle {
        self_id: Some("ParagraphStyle/indented".into()),
        name: Some("Indented".into()),
        based_on: None,
    })
    .expect("style");
    d.m.apply_mutation(&Mutation::SetStyleProperty {
        collection: StyleCollection::Paragraph,
        style_id: "ParagraphStyle/indented".into(),
        path: P::ParagraphFirstLineIndent,
        value: V::Length(Some(24.0)),
    })
    .expect("indent");
    // Contiguous: "Alpha one" 9 + U+2028 1 + "Bravo two" 9 = [0, 19).
    d.m.apply_mutation(&Mutation::ApplyStyle {
        story_id: d.story.clone(),
        start: 0,
        end: 19,
        style: "ParagraphStyle/indented".into(),
        scope: StyleScope::Paragraph,
        cell: None,
    })
    .expect("apply");
    let paras = paragraphs(&d);
    assert_eq!(paras[0].1.as_deref(), Some("ParagraphStyle/indented"));
    assert_ne!(
        paras[1].1.as_deref(),
        Some("ParagraphStyle/indented"),
        "[0,19) stops before the next paragraph"
    );
    let l = lines(&d);
    assert!(
        (l[0].2 - (l[1].2 + 24.0)).abs() < 0.01,
        "the first-line indent is the first line's only: {l:?}"
    );
}

#[test]
fn idml_export_writes_the_forced_line_break_inside_content() {
    let d = doc();
    let bytes = d.m.export_idml().expect("export");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut found = None;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).expect("entry");
        if !f.name().starts_with("Stories/") {
            continue;
        }
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut f, &mut xml).expect("utf8");
        if xml.contains("Alpha one") {
            found = Some(xml);
        }
    }
    let xml = found.expect("the story part");
    assert!(
        xml.contains("<Content>Alpha one\u{2028}Bravo two</Content>"),
        "{xml}"
    );
    assert_eq!(xml.matches("<Br").count(), 1, "one paragraph mark: {xml}");
}

#[test]
fn undo_removes_the_inserted_text() {
    let mut d = doc();
    d.m.undo().expect("undo");
    assert!(paragraphs(&d).iter().all(|p| p.0.is_empty()));
}
