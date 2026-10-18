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

//! `table-nested.idml` — a table inside a table cell, against InDesign.
//!
//! One case per page: a one-cell host table (300 pt wide) whose cell holds
//! a nested table, with a plain host row below it, so a nested table that
//! comes out too tall or too short shows as that row moving. Each case
//! asks InDesign one question:
//!
//!   1. a plain 3 × 3 nested table — its default strokes;
//!   2. a column span (A1 over two columns) and a row span (C2 over two
//!      rows);
//!   3. cell fills and per-edge strokes (3 pt red, 2 pt blue);
//!   4. a header row;
//!   5. columns narrower than the host cell (the table sits at its left);
//!   6. columns wider than the host cell (400 pt in a 300 pt cell).
//!
//! Every nested cell names itself (`A1`, `B2`, …), so a misplaced cell
//! reads off the diff.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Cell, Paragraph, Story, Table},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "table-nested";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const HOST_W: f32 = 300.0;
const ROW_H: f32 = 20.0;
const COL_W: f32 = 80.0;

/// One case: its label and the nested table it puts in the host cell.
pub struct Case {
    pub name: &'static str,
    pub nested: fn(&str) -> Table,
}

/// A nested cell named after its column and row (`B2`).
fn named(col: u32, row: u32) -> Cell {
    Cell::plain(format!("{}{}", (b'A' + col as u8) as char, row + 1))
}

/// A `cols` × `rows` table of named cells, column-major as IDML orders
/// them, every column `widths[c]` wide.
fn grid(id: &str, header: u32, widths: Vec<f32>, rows: u32) -> Table {
    let cols = widths.len() as u32;
    let mut cells = Vec::new();
    for c in 0..cols {
        for r in 0..rows {
            cells.push(named(c, r));
        }
    }
    Table {
        self_id: id.to_string(),
        applied_table_style: None,
        header_row_count: header,
        footer_row_count: 0,
        body_row_count: rows - header,
        column_count: cols,
        row_heights_pt: vec![ROW_H; rows as usize],
        column_widths_pt: widths,
        cells,
        extra_row_attrs: Vec::new(),
    }
}

fn plain(id: &str) -> Table {
    grid(id, 0, vec![COL_W; 3], 3)
}

fn spans(id: &str) -> Table {
    let mut t = grid(id, 0, vec![COL_W; 3], 3);
    // Column-major: index = col * 3 + row. A1 spans A1:B1, so B1 goes;
    // C2 spans C2:C3, so C3 goes.
    t.cells[0].column_span = 2;
    t.cells[0].paragraphs = vec![Paragraph::plain("A1 · two columns")];
    t.cells[2 * 3 + 1].row_span = 2;
    t.cells[2 * 3 + 1].paragraphs = vec![Paragraph::plain("C2 · two rows")];
    let gone = [3, 2 * 3 + 2];
    t.cells = t
        .cells
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !gone.contains(i))
        .map(|(_, c)| c)
        .collect();
    t
}

fn fills_and_strokes(id: &str) -> Table {
    let mut t = grid(id, 0, vec![COL_W; 3], 3);
    // A1 filled red; B2 filled blue with a 3 pt red top edge; C3 a 2 pt
    // blue left edge.
    t.cells[0].fill_color = Some("Color/Red".to_string());
    t.cells[3 + 1].fill_color = Some("Color/Blue".to_string());
    t.cells[3 + 1].top_edge_stroke_color = Some("Color/Red");
    t.cells[3 + 1].top_edge_stroke_weight = Some(3.0);
    t.cells[2 * 3 + 2].left_edge_stroke_color = Some("Color/Blue");
    t.cells[2 * 3 + 2].left_edge_stroke_weight = Some(2.0);
    t
}

fn header(id: &str) -> Table {
    grid(id, 1, vec![COL_W; 3], 4)
}

fn narrow(id: &str) -> Table {
    grid(id, 0, vec![50.0; 2], 2)
}

fn wide(id: &str) -> Table {
    grid(id, 0, vec![100.0; 4], 2)
}

/// The cases, in page order.
pub fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "nested · 3 x 3 · default strokes",
            nested: plain,
        },
        Case {
            name: "nested · column span A1:B1 · row span C2:C3",
            nested: spans,
        },
        Case {
            name: "nested · fills and per-edge strokes",
            nested: fills_and_strokes,
        },
        Case {
            name: "nested · a header row",
            nested: header,
        },
        Case {
            name: "nested · narrower than its cell",
            nested: narrow,
        },
        Case {
            name: "nested · wider than its cell (400 in 300)",
            nested: wide,
        },
    ]
}

/// The story of case `i`'s host table (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// The host table of case `i`.
pub fn host_table_id(i: u32) -> String {
    self_id(SAMPLE, "Host", i)
}

/// The nested table of case `i`.
pub fn nested_table_id(i: u32) -> String {
    self_id(SAMPLE, "Nested", i)
}

fn host(i: u32, case: &Case) -> Table {
    let mut holder = Cell::plain("");
    holder.paragraphs = vec![Paragraph::plain("").with_table((case.nested)(&nested_table_id(i)))];
    Table {
        self_id: host_table_id(i),
        applied_table_style: None,
        header_row_count: 0,
        footer_row_count: 0,
        body_row_count: 2,
        column_count: 1,
        row_heights_pt: vec![ROW_H; 2],
        column_widths_pt: vec![HOST_W],
        cells: vec![holder, Cell::plain("the host row below")],
        extra_row_attrs: Vec::new(),
    }
}

fn frame(self_id: String, (w, h): (f32, f32), (x, y): (f32, f32), story: String) -> Rect {
    Rect {
        self_id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(x, y),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(story),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    }
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut spreads = Vec::new();
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut spread_refs = Vec::new();

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_spreads = vec![(
        master_id.clone(),
        write_master(&Master {
            self_id: format!("MasterSpread/{master_id}"),
            page_self_id: self_id(SAMPLE, "MasterPage", 0),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        }),
    )];

    for (i, case) in cases().iter().enumerate() {
        let seq = i as u32;
        let label = self_id(SAMPLE, "LabelStory", seq);
        let body = body_story_id(seq);
        stories.push((
            label.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label.clone(),
                paragraphs: vec![Paragraph::plain(case.name)],
            }),
        ));
        story_refs.push(label.clone());
        stories.push((
            body.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: body.clone(),
                paragraphs: vec![Paragraph::plain("").with_table(host(seq, case))],
            }),
        ));
        story_refs.push(body.clone());

        let spread_id = self_id(SAMPLE, "Spread", seq);
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(SAMPLE, "Page", seq),
                page_name: (seq + 1).to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![
                    frame(
                        self_id(SAMPLE, "LabelFrame", seq),
                        (460.0, 24.0),
                        (36.0, 36.0),
                        label,
                    )
                    .into(),
                    frame(
                        self_id(SAMPLE, "BodyFrame", seq),
                        (480.0, 400.0),
                        (72.0, 120.0),
                        body,
                    )
                    .into(),
                ],
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        ));
        spread_refs.push(spread_id);
    }

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id],
            spreads: spread_refs,
            stories: story_refs,
        }),
        graphic_xml: graphic_xml_with_extras(&[
            ExtraColor {
                self_id: "Color/Red".to_string(),
                name: "Red".to_string(),
                space: "CMYK",
                value: "0 100 100 0".to_string(),
            },
            ExtraColor {
                self_id: "Color/Blue".to_string(),
                name: "Blue".to_string(),
                space: "CMYK",
                value: "100 60 0 0".to_string(),
            },
        ]),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}
