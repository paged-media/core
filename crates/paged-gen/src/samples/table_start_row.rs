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

//! `table-start-row.idml` — a table row's Start Row option (`Row@StartRow`,
//! ADR 175) against InDesign.
//!
//! Every case is one story holding one table, threaded through the
//! `start-paragraph` fixture's four-page chain:
//!
//! | page | frame | |
//! |---|---|---|
//! | 1 | A | two columns, five rows each |
//! | 1 | B | one column, five rows, below A |
//! | 2 | C | |
//! | 3 | D | |
//! | 4 | E | |
//!
//! Each case starts on an ODD page (cases are four pages long), so where a
//! row lands answers the rule directly: A's second column (next column),
//! B (next frame), C (next page, next even page), D (next odd page). The
//! table is one column of 20 pt rows (each grows to its label), named `R01`, `R02`, ...
//!
//! The last three cases ask what InDesign does when the rule meets a row
//! that already opens a column or the story, and whether a header row
//! follows a row that the rule moves.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Cell, Paragraph, Story, Table},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "table-start-row";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const ROW_H: f32 = 20.0;
/// Five rows per column, with room to spare: a 20 pt row grows to
/// 20.826 pt with its label (4 pt insets around Open Sans 12's 12.826 pt
/// ascent), and five of those with their strokes are 105.13 pt.
const FRAME_H: f32 = 115.0;
const COLUMN_W: f32 = 100.0;
const GUTTER: f32 = 20.0;
const TABLE_W: f32 = 90.0;
/// Pages per case (the chain's length in pages).
pub const PAGES_PER_CASE: usize = 4;

/// A case: its label, whether the table has a header row, the body row
/// count, and which body row (1-based) carries which `StartRow` value.
pub struct Case {
    pub name: &'static str,
    pub header: bool,
    pub rows: u32,
    pub rule_on: u32,
    pub rule: &'static str,
}

/// The cases, in page order.
pub fn cases() -> Vec<Case> {
    let c = |name, header, rows, rule_on, rule| Case {
        name,
        header,
        rows,
        rule_on,
        rule,
    };
    vec![
        c("control · R04 Anywhere", false, 8, 4, "Anywhere"),
        c("R04 NextColumn", false, 8, 4, "NextColumn"),
        c("R04 NextFrame", false, 8, 4, "NextFrame"),
        c("R04 NextPage", false, 8, 4, "NextPage"),
        c("R04 NextOddPage", false, 8, 4, "NextOddPage"),
        c("R04 NextEvenPage", false, 8, 4, "NextEvenPage"),
        // The rule on the table's first row (top of frame A, page 1).
        c(
            "R01 NextPage · first row of the story",
            false,
            8,
            1,
            "NextPage",
        ),
        // R01-R05 fill column one; R06 opens column two anyway.
        c(
            "R06 NextColumn · already opens column two",
            false,
            10,
            6,
            "NextColumn",
        ),
        // Does the header follow the moved row into frame B?
        c("header · R04 NextFrame", true, 8, 4, "NextFrame"),
    ]
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// The table of case `i` (0-based).
pub fn table_id(i: u32) -> String {
    self_id(SAMPLE, "Table", i)
}

fn table(i: u32, case: &Case) -> Table {
    let header = u32::from(case.header);
    let total = header + case.rows;
    let mut cells: Vec<Cell> = Vec::with_capacity(total as usize);
    if case.header {
        cells.push(Cell::plain("head"));
    }
    cells.extend((1..=case.rows).map(|n| Cell::plain(format!("R{n:02}"))));
    let mut t = Table {
        self_id: table_id(i),
        applied_table_style: None,
        header_row_count: header,
        footer_row_count: 0,
        body_row_count: case.rows,
        column_count: 1,
        row_heights_pt: vec![ROW_H; total as usize],
        column_widths_pt: vec![TABLE_W],
        cells,
        extra_row_attrs: Vec::new(),
    };
    t.extra_row_attrs
        .push((header + case.rule_on - 1, "StartRow", case.rule.to_string()));
    t
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut master_spreads = Vec::new();
    let mut spreads = Vec::new();
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut spread_refs = Vec::new();

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    master_spreads.push((
        master_id.clone(),
        write_master(&Master {
            self_id: format!("MasterSpread/{master_id}"),
            page_self_id: self_id(SAMPLE, "MasterPage", 0),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        }),
    ));

    for (i, case) in cases().into_iter().enumerate() {
        let seq = i as u32;
        let label_story_id = self_id(SAMPLE, "LabelStory", seq);
        let body = body_story_id(seq);

        stories.push((
            label_story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story_id.clone(),
                paragraphs: vec![Paragraph::plain(case.name)],
            }),
        ));
        story_refs.push(label_story_id.clone());
        stories.push((
            body.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: body.clone(),
                paragraphs: vec![Paragraph::plain("").with_table(table(seq, &case))],
            }),
        ));
        story_refs.push(body.clone());

        // The chain A → B → C → D → E.
        let ids: Vec<String> = ["A", "B", "C", "D", "E"]
            .iter()
            .map(|f| self_id(SAMPLE, &format!("Frame{f}"), seq))
            .collect();
        let link = |k: usize| ((k > 0).then(|| ids[k - 1].clone()), ids.get(k + 1).cloned());
        let body_frame = |k: usize, columns: u32, y: f32| -> PageItem {
            let (prev, next) = link(k);
            let width = columns as f32 * COLUMN_W + (columns as f32 - 1.0) * GUTTER;
            Rect {
                self_id: ids[k].clone(),
                width_pt: width,
                height_pt: FRAME_H,
                item_transform: translate(72.0, y),
                fill_color: None,
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight_pt: Some(0.25),
                parent_story: Some(body.clone()),
                next_text_frame: next,
                previous_text_frame: prev,
                extra_attrs: Vec::new(),
                blending: None,
                drop_shadow: None,
                placed_image: None,
                text_wrap: None,
                anchored_setting: None,
                frame_effects: Vec::new(),
                text_frame_pref: Some(TextFramePref {
                    inset_spacing: Some([0.0, 0.0, 0.0, 0.0]),
                    text_column_count: (columns > 1).then_some(columns),
                    text_column_gutter: (columns > 1).then_some(GUTTER),
                    ..Default::default()
                }),
                custom_subpaths: None,
            }
            .into()
        };
        let label: PageItem = Rect {
            self_id: self_id(SAMPLE, "LabelFrame", seq),
            width_pt: 460.0,
            height_pt: 24.0,
            item_transform: translate(36.0, 12.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(label_story_id),
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
        .into();

        let pages: Vec<Vec<PageItem>> = vec![
            vec![label, body_frame(0, 2, 72.0), body_frame(1, 1, 300.0)],
            vec![body_frame(2, 1, 72.0)],
            vec![body_frame(3, 1, 72.0)],
            vec![body_frame(4, 1, 72.0)],
        ];
        for (p, items) in pages.into_iter().enumerate() {
            let n = (i * PAGES_PER_CASE + p) as u32;
            let spread_id = self_id(SAMPLE, "Spread", n);
            spreads.push((
                spread_id.clone(),
                write_spread(&Spread {
                    self_id: spread_id.clone(),
                    page_self_id: self_id(SAMPLE, "Page", n),
                    page_name: (n + 1).to_string(),
                    applied_master: format!("MasterSpread/{master_id}"),
                    page_width_pt: PAGE_W_PT,
                    page_height_pt: PAGE_H_PT,
                    page_items: items,
                    override_list: Vec::new(),
                    margins: None,
                    item_transform: None,
                }),
            ));
            spread_refs.push(spread_id);
        }
    }

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id],
            spreads: spread_refs,
            stories: story_refs,
        }),
        graphic_xml: graphic_xml(),
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
