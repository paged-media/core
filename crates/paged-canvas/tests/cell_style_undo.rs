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

//! Undo of a cell-addressed text edit lands in the CELL.
//!
//! plugin-doc found that undoing `applyStyle { cell: Some(..) }` left the
//! cell paragraph's new style in place: the splitter's inverse addressed a
//! `StoryRange`, which has no cell, so the undo restyled the story's BODY
//! paragraphs at the cell-local offsets instead. Every case here edits a
//! cell whose offsets overlap real body text, then checks undo restores
//! both streams exactly and redo re-applies — paragraph scope over a range
//! and at a caret, character scope, and the cell-qualified `insertText` /
//! `deleteRange`.

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation, PageId, TextCellAddr};
use paged_mutate::operation::StyleScope;

const H: &str = "ParagraphStyle/h";
const H2: &str = "ParagraphStyle/h2";
const C: &str = "CharacterStyle/c";

/// One paragraph of a snapshot: its text, its paragraph style, and the
/// character style of each character (split-invariant: an undo may leave
/// runs split that the edit split, and that is not a difference).
type Para = (String, Option<String>, Vec<Option<String>>);

struct Fixture {
    m: CanvasModel,
    story: String,
    cell: TextCellAddr,
}

/// A frame whose story holds two body paragraphs and then a 2x2 table,
/// cell (0,0) holding "One" / "Two".
fn fixture() -> Fixture {
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
        text: "Body one\nBody two".into(),
        cell: None,
    })
    .expect("body text");
    let out = m
        .apply_mutation(&Mutation::InsertTable {
            story_id: story.clone(),
            rows: 2,
            cols: 2,
            header_rows: 0,
            footer_rows: 0,
            column_widths: vec![],
            row_heights: vec![],
        })
        .expect("table");
    let Some(ElementId::Table { table_id, .. }) = out.created_id else {
        panic!("expected a table, got {:?}", out.created_id);
    };
    let cell = TextCellAddr {
        table_id,
        row: 0,
        col: 0,
    };
    m.apply_mutation(&Mutation::InsertText {
        story_id: story.clone(),
        offset: 0,
        text: "One\nTwo".into(),
        cell: Some(cell.clone()),
    })
    .expect("cell text");
    for (id, name) in [(H, "H"), (H2, "H2")] {
        m.apply_mutation(&Mutation::CreateParagraphStyle {
            self_id: Some(id.into()),
            name: Some(name.into()),
            based_on: None,
        })
        .expect("paragraph style");
    }
    m.apply_mutation(&Mutation::CreateCharacterStyle {
        self_id: Some(C.into()),
        name: Some("C".into()),
        based_on: None,
    })
    .expect("character style");
    Fixture { m, story, cell }
}

fn snap(paragraphs: &[paged_model::Paragraph]) -> Vec<Para> {
    paragraphs
        .iter()
        .filter(|p| p.table.is_none())
        .map(|p| {
            let text: String = p.runs.iter().map(|r| r.text.as_str()).collect();
            let chars = p
                .runs
                .iter()
                .flat_map(|r| r.text.chars().map(|_| r.character_style.clone()))
                .collect();
            (text, p.paragraph_style.clone(), chars)
        })
        .collect()
}

impl Fixture {
    /// (body paragraphs, cell (0,0) paragraphs).
    fn state(&self) -> (Vec<Para>, Vec<Para>) {
        let story = &self
            .m
            .scene()
            .stories
            .iter()
            .find(|s| s.self_id == self.story)
            .expect("story")
            .story;
        let table = story
            .paragraphs
            .iter()
            .filter_map(|p| p.table.as_ref())
            .find(|t| t.self_id.as_deref() == Some(self.cell.table_id.as_str()))
            .expect("table");
        let cell = table
            .cells
            .iter()
            .find(|c| c.coords() == Some((0, 0)))
            .expect("cell (0,0)");
        (snap(&story.paragraphs), snap(&cell.paragraphs))
    }

    fn style(&mut self, start: u32, end: u32, style: &str, scope: StyleScope) {
        self.m
            .apply_mutation(&Mutation::ApplyStyle {
                story_id: self.story.clone(),
                start,
                end,
                style: style.into(),
                scope,
                cell: Some(self.cell.clone()),
            })
            .expect("applyStyle in the cell");
    }

    /// Undo restores `before` exactly; redo brings back `after`; a second
    /// undo restores `before` again.
    fn round_trip(&mut self, before: &(Vec<Para>, Vec<Para>)) {
        let after = self.state();
        assert_ne!(&after, before, "the edit changed nothing");
        assert_eq!(after.0, before.0, "a cell edit touched the body");
        self.m.undo().expect("undo");
        assert_eq!(&self.state(), before, "undo restores body and cell");
        self.m.redo().expect("redo");
        assert_eq!(self.state(), after, "redo re-applies the edit");
        self.m.undo().expect("undo again");
        assert_eq!(&self.state(), before, "the second undo restores too");
    }
}

#[test]
fn undoing_a_cell_paragraph_style_restores_the_cell() {
    let mut f = fixture();
    let before = f.state();
    f.style(0, 3, H, StyleScope::Paragraph);
    assert_eq!(f.state().1[0].1.as_deref(), Some(H));
    f.round_trip(&before);
}

#[test]
fn undoing_a_cell_paragraph_style_at_a_caret_restores_the_cell() {
    let mut f = fixture();
    let before = f.state();
    // Contiguous offsets: "One" 0..3, "Two" 3..6 — the caret at 5 stands
    // in the cell's second paragraph.
    f.style(5, 5, H, StyleScope::Paragraph);
    let after = f.state();
    assert_eq!(after.1[1].1.as_deref(), Some(H), "{after:?}");
    assert_eq!(after.1[0], before.1[0]);
    f.round_trip(&before);
}

#[test]
fn undo_restores_a_cell_paragraphs_previous_style_not_none() {
    let mut f = fixture();
    f.style(0, 3, H, StyleScope::Paragraph);
    let before = f.state();
    f.style(0, 6, H2, StyleScope::Paragraph);
    assert_eq!(f.state().1[0].1.as_deref(), Some(H2));
    f.round_trip(&before);
    assert_eq!(f.state().1[0].1.as_deref(), Some(H));
}

#[test]
fn undoing_a_cell_character_style_restores_the_cell() {
    let mut f = fixture();
    let before = f.state();
    // "ne" + "Tw": a range across the cell's paragraph boundary.
    f.style(1, 5, C, StyleScope::Character);
    let after = f.state();
    let c = Some(C.to_string());
    assert_eq!(after.1[0].2, [None, c.clone(), c.clone()], "{after:?}");
    assert_eq!(after.1[1].2, [c.clone(), c, None], "{after:?}");
    f.round_trip(&before);
}

#[test]
fn undoing_cell_insert_text_and_delete_range_restores_the_cell() {
    let mut f = fixture();
    let before = f.state();
    f.m.apply_mutation(&Mutation::InsertText {
        story_id: f.story.clone(),
        offset: 1,
        text: "XY\nZ".into(),
        cell: Some(f.cell.clone()),
    })
    .expect("insert in the cell");
    assert_eq!(f.state().1[0].0, "OXY");
    f.round_trip(&before);

    f.m.apply_mutation(&Mutation::DeleteRange {
        story_id: f.story.clone(),
        start: 1,
        end: 5,
        cell: Some(f.cell.clone()),
    })
    .expect("delete in the cell");
    // insertText's space: "One\nTwo", so [1, 5) takes "ne\nT".
    assert_eq!(f.state().1.len(), 1);
    assert_eq!(f.state().1[0].0, "Owo");
    f.round_trip(&before);
}
