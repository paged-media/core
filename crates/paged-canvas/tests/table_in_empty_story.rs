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

//! A table inserted into an empty story sits on the story's first line,
//! as in InDesign.
//!
//! `InsertTable` always appended a NEW paragraph for the table, so a
//! table placed in a fresh text frame — whose story holds one empty
//! paragraph — started one line down, under an empty lead-in line (the
//! story wrote `<Content></Content><Br/>` before the table). The table
//! now takes the story's last paragraph when that paragraph is empty
//! (the insertion point at the story's end is on an empty line), and the
//! insert still undoes and redoes exactly.

use paged_canvas::channel::Mutation;
use paged_canvas::{blank::blank_idml, CanvasModel, CanvasOptions, PageId};

/// A blank page with one fresh text frame at page-local top 72 pt; the
/// frame's (minted, empty) story id.
fn fresh_frame() -> (CanvasModel, String) {
    let mut model =
        CanvasModel::load("table", &blank_idml(612.0, 792.0), CanvasOptions::default()).unwrap();
    let page = model.handle().page_ids[0].clone();
    model
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: PageId(page.0.clone()),
            bounds: (72.0, 72.0, 400.0, 400.0),
        })
        .expect("insert text frame");
    let story = model.scene().stories.last().unwrap().self_id.clone();
    (model, story)
}

fn insert_table(story_id: &str) -> Mutation {
    Mutation::InsertTable {
        story_id: story_id.to_string(),
        rows: 2,
        cols: 2,
        header_rows: 0,
        footer_rows: 0,
        column_widths: vec![],
        row_heights: vec![],
    }
}

fn story_json(model: &CanvasModel, story_id: &str) -> String {
    let s = model
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story_id)
        .expect("story");
    serde_json::to_string(s).unwrap()
}

#[test]
fn a_table_in_a_fresh_frame_takes_the_first_line() {
    let (mut model, story) = fresh_frame();
    model
        .apply_mutation(&insert_table(&story))
        .expect("insert table");

    let s = model
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .unwrap();
    assert_eq!(
        s.story.paragraphs.len(),
        1,
        "no empty lead-in paragraph before the table"
    );
    assert!(s.story.paragraphs[0].table.is_some());

    // The first row starts at the frame's top, not a line further down.
    let top = model
        .built()
        .pages
        .iter()
        .flat_map(|p| p.cell_rects.iter())
        .filter(|c| c.story_id == story && c.row == 0)
        .map(|c| c.rect[1])
        .fold(f32::INFINITY, f32::min);
    // (72.5: the row's outer box sits half its 1 pt top stroke inside.)
    assert!(
        (72.0..73.0).contains(&top),
        "first row at y = {top}, frame top 72 — a line down means a lead-in"
    );
}

#[test]
fn the_insert_undoes_and_redoes_exactly() {
    let (mut model, story) = fresh_frame();
    let before = story_json(&model, &story);
    model
        .apply_mutation(&insert_table(&story))
        .expect("insert table");
    let after = story_json(&model, &story);

    model.undo().expect("undo");
    assert_eq!(
        story_json(&model, &story),
        before,
        "undo restores the empty story"
    );
    model.redo().expect("redo");
    assert_eq!(
        story_json(&model, &story),
        after,
        "redo puts the table back"
    );
    model.undo().expect("undo again");
    assert_eq!(story_json(&model, &story), before);
}

#[test]
fn after_text_the_table_still_takes_a_paragraph_of_its_own() {
    let (mut model, story) = fresh_frame();
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: "Prices".into(),
            cell: None,
        })
        .expect("insert text");
    model
        .apply_mutation(&insert_table(&story))
        .expect("insert table");
    let s = model
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .unwrap();
    assert_eq!(s.story.paragraphs.len(), 2);
    assert!(s.story.paragraphs[0].table.is_none());
    assert!(s.story.paragraphs[1].table.is_some());
}
