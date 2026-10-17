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

//! `table-fit.idml` — how many table rows a frame holds, at the edge.
//!
//! One case per page: frame A, of the case's height, threaded to frame B
//! beside it, holds a table of eight fixed 20 pt rows with 1 pt strokes —
//! 120 pt of rows, 121 with the table's outer strokes. Asked of InDesign
//! 20.0.1 first (`table-fit-probe`): six rows fit when the text area
//! holds the rows and half the strokes above and below them. Each pair of
//! cases sits either side of that edge:
//!
//!   1-2. a plain frame, 120.9 and 121 pt;
//!   3-4. a 4 pt bottom inset, 124.9 and 125 pt;
//!   5-6. a 1 pt black frame stroke, centred, 121.9 and 122 pt.

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

const SAMPLE: &str = "table-fit";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const ROWS: u32 = 8;

/// A case: its label, frame A's height, its bottom inset and its stroke,
/// and how many rows InDesign sets in it.
pub struct Case {
    pub name: &'static str,
    pub height: f32,
    pub bottom_inset: f32,
    pub stroke: Option<f32>,
    pub rows_in_a: u32,
}

/// The cases, in page order.
pub fn cases() -> Vec<Case> {
    let c = |name, height, bottom_inset, stroke, rows_in_a| Case {
        name,
        height,
        bottom_inset,
        stroke,
        rows_in_a,
    };
    vec![
        c("plain · 120.9 pt", 120.9, 0.0, None, 5),
        c("plain · 121 pt", 121.0, 0.0, None, 6),
        c("bottom inset 4 · 124.9 pt", 124.9, 4.0, None, 5),
        c("bottom inset 4 · 125 pt", 125.0, 4.0, None, 6),
        c("1 pt frame stroke · 121.9 pt", 121.9, 0.0, Some(1.0), 5),
        c("1 pt frame stroke · 122 pt", 122.0, 0.0, Some(1.0), 6),
    ]
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// The table of case `i`.
pub fn table_id(i: u32) -> String {
    self_id(SAMPLE, "Table", i)
}

/// Frame A of case `i`.
pub fn frame_a_id(i: u32) -> String {
    self_id(SAMPLE, "FrameA", i)
}

fn table(i: u32) -> Table {
    let mut t = Table {
        self_id: table_id(i),
        applied_table_style: None,
        header_row_count: 0,
        footer_row_count: 0,
        body_row_count: ROWS,
        column_count: 1,
        row_heights_pt: vec![20.0; ROWS as usize],
        column_widths_pt: vec![90.0],
        cells: (1..=ROWS)
            .map(|n| {
                // No top or bottom inset, so each 12 pt label (12.826 pt
                // of ascent) sits inside its fixed 20 pt row: an overset
                // cell has no frame to report in InDesign's DOM.
                let mut cell = Cell::plain(format!("R{n}"));
                cell.extra_cell_attrs = vec![
                    ("TextTopInset", "0".to_string()),
                    ("TextBottomInset", "0".to_string()),
                ];
                cell
            })
            .collect(),
        extra_row_attrs: Vec::new(),
    };
    // Fixed rows: each stays exactly 20 pt whatever its label needs.
    for r in 0..ROWS {
        t.extra_row_attrs.push((r, "AutoGrow", "false".to_string()));
    }
    t
}

fn frame(
    id: String,
    (x, h): (f32, f32),
    story: String,
    (prev, next): (Option<String>, Option<String>),
    bottom_inset: f32,
    stroke: Option<f32>,
) -> PageItem {
    Rect {
        self_id: id,
        width_pt: 120.0,
        height_pt: h,
        item_transform: translate(x, 72.0),
        fill_color: None,
        stroke_color: stroke.map(|_| "Color/Black".to_string()),
        stroke_weight_pt: stroke,
        parent_story: Some(story),
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
            inset_spacing: Some([0.0, 0.0, bottom_inset, 0.0]),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
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
                paragraphs: vec![Paragraph::plain("").with_table(table(seq))],
            }),
        ));
        story_refs.push(body.clone());
        let a = frame_a_id(seq);
        let b = self_id(SAMPLE, "FrameB", seq);
        let label_frame: PageItem = Rect {
            self_id: self_id(SAMPLE, "LabelFrame", seq),
            width_pt: 460.0,
            height_pt: 24.0,
            item_transform: translate(36.0, 24.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(label),
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
                    label_frame,
                    frame(
                        a.clone(),
                        (72.0, case.height),
                        body.clone(),
                        (None, Some(b.clone())),
                        case.bottom_inset,
                        case.stroke,
                    ),
                    frame(b, (300.0, 300.0), body, (Some(a), None), 0.0, None),
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
