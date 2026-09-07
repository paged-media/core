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

//! `tables-overset.idml` — what InDesign draws when a table does not
//! fit its frame.
//!
//! The renderer oversets table rows the last frame cannot hold, but
//! only *after* one row has been placed: header rows are emitted
//! unconditionally, and the body-overset test requires
//! `placed_in_frame > 0`. So a frame too short for even its first row
//! still draws that row. Measured on the annual's page 1 (a 48-cell
//! sheet table in a 56 pt frame) the canvas drew a table InDesign
//! shows nothing of.
//!
//! This fixture isolates that boundary. Each page is one table in a
//! frame of a deliberately chosen height, and the reference PDF is
//! InDesign's own answer:
//!
//!   1. every row fits — the control that proves the fixture sound
//!   2. two of four rows fit — the partial overset that already works
//!   3. not even the first body row fits
//!   4. a header row taller than the whole frame
//!   5. the header fits but no body row does
//!
//! Pages 3–5 are the ones that carry the question. The frame itself
//! has no fill and no stroke, so anything at all on those pages below
//! the label is table ink.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Cell, Paragraph, Story, Table},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "tables-overset";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 360.0;
const ROW_H_PT: f32 = 28.0;
const COL_W_PT: f32 = 120.0;
const COLUMNS: usize = 3;

struct Variant {
    name: &'static str,
    /// Frame height in points — the whole point of the fixture.
    frame_h_pt: f32,
    header_rows: usize,
    body_rows: usize,
    /// Explicit stroke weight on every cell edge. `None` leaves the
    /// attributes off entirely, which is InDesign's default (1 pt
    /// black) and the case our two writers actually take. A declared
    /// weight is here to measure how the table's placement moves with
    /// it — see the last variant.
    edge_weight: Option<f32>,
}

/// Row heights are uniform, so each case is stated as "N rows of 28 pt
/// into an H pt frame" and the arithmetic is visible in the name.
fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "overset · 4 rows of 28pt in a 200pt frame · all fit",
            frame_h_pt: 200.0,
            header_rows: 0,
            body_rows: 4,
            edge_weight: None,
        },
        Variant {
            name: "overset · 4 rows of 28pt in a 62pt frame · 2 fit",
            frame_h_pt: 62.0,
            header_rows: 0,
            body_rows: 4,
            edge_weight: None,
        },
        Variant {
            name: "overset · 4 rows of 28pt in a 20pt frame · none fit",
            frame_h_pt: 20.0,
            header_rows: 0,
            body_rows: 4,
            edge_weight: None,
        },
        Variant {
            name: "overset · header + 3 rows in a 20pt frame · header too tall",
            frame_h_pt: 20.0,
            header_rows: 1,
            body_rows: 3,
            edge_weight: None,
        },
        Variant {
            name: "overset · header + 3 rows in a 30pt frame · only the header fits",
            frame_h_pt: 30.0,
            header_rows: 1,
            body_rows: 3,
            edge_weight: None,
        },
        // A heavy declared border, so the fixture carries two stroke
        // weights rather than one. InDesign places the table half its
        // OUTER border below the frame top (measured: a 1 pt default
        // border puts the first row's text at 128.64 pt where we put it
        // at 128.16); with 4 pt here the same rule predicts 2 pt, and a
        // rule tested at one weight is a coincidence.
        Variant {
            name: "overset · 4 rows in a 200pt frame · 4pt cell borders",
            frame_h_pt: 200.0,
            header_rows: 0,
            body_rows: 4,
            edge_weight: Some(4.0),
        },
    ]
}

fn table_for(variant: &Variant, id: &str) -> Table {
    let rows = variant.header_rows + variant.body_rows;
    // Column-major, as IDML orders cells.
    let mut cells = Vec::with_capacity(rows * COLUMNS);
    for c in 0..COLUMNS {
        for r in 0..rows {
            let label = if r < variant.header_rows {
                format!("Hdr {}", c + 1)
            } else {
                format!("R{}C{}", r + 1 - variant.header_rows, c + 1)
            };
            let mut cell = Cell::plain(label);
            if let Some(w) = variant.edge_weight {
                cell.top_edge_stroke_color = Some("Color/Black");
                cell.bottom_edge_stroke_color = Some("Color/Black");
                cell.left_edge_stroke_color = Some("Color/Black");
                cell.right_edge_stroke_color = Some("Color/Black");
                cell.top_edge_stroke_weight = Some(w);
                cell.bottom_edge_stroke_weight = Some(w);
                cell.left_edge_stroke_weight = Some(w);
                cell.right_edge_stroke_weight = Some(w);
            }
            cells.push(cell);
        }
    }
    Table {
        self_id: id.to_string(),
        applied_table_style: None,
        header_row_count: variant.header_rows as u32,
        footer_row_count: 0,
        body_row_count: variant.body_rows as u32,
        column_count: COLUMNS as u32,
        row_heights_pt: vec![ROW_H_PT; rows],
        column_widths_pt: vec![COL_W_PT; COLUMNS],
        cells,
    }
}

pub fn build() -> Sample {
    let variants = variants();

    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut stories = Vec::with_capacity(variants.len());
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());
    let mut story_refs = Vec::with_capacity(variants.len());

    for (i, variant) in variants.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let story_id = self_id(SAMPLE, "Story", seq);
        let label_story_id = self_id(SAMPLE, "LabelStory", seq);
        let label_frame_id = self_id(SAMPLE, "LabelFrame", seq);
        let body_frame_id = self_id(SAMPLE, "TextFrame", seq);
        let table_self_id = self_id(SAMPLE, "Table", seq);

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id.clone(),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());

        stories.push((
            label_story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story_id.clone(),
                paragraphs: vec![Paragraph::plain(variant.name)],
            }),
        ));
        story_refs.push(label_story_id.clone());

        let host_paragraph = Paragraph {
            table: Some(table_for(variant, &table_self_id)),
            ..Paragraph::plain("")
        };
        stories.push((
            story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: story_id.clone(),
                paragraphs: vec![host_paragraph],
            }),
        ));
        story_refs.push(story_id.clone());

        let label_frame = plain_frame(
            label_frame_id,
            480.0,
            24.0,
            translate_m(36.0, 36.0),
            label_story_id,
        );

        // Fixed top edge across the fixture: the frames differ only in
        // height, so a page-to-page comparison isolates that one
        // variable.
        let body_frame = plain_frame(
            body_frame_id,
            FRAME_W_PT,
            variant.frame_h_pt,
            translate_m((PAGE_W_PT - FRAME_W_PT) * 0.5, 120.0),
            story_id.clone(),
        );

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: variant.name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![label_frame.into(), body_frame.into()],
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
            master_spreads: master_refs,
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

/// A text frame with no fill, no stroke and no preferences — so the
/// only ink it can contribute is its story's. That is what makes
/// "InDesign drew nothing here" a measurable statement.
fn plain_frame(
    self_id: String,
    width_pt: f32,
    height_pt: f32,
    item_transform: Matrix,
    parent_story: String,
) -> Rect {
    Rect {
        self_id,
        width_pt,
        height_pt,
        item_transform,
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(parent_story),
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

fn translate_m(tx: f32, ty: f32) -> Matrix {
    let mut m = IDENTITY;
    m[4] = tx;
    m[5] = ty;
    m
}
