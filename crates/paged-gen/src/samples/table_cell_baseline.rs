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

//! `table-cell-baseline.idml` — where a table cell sets its first
//! baseline, asked of InDesign.
//!
//! Two A4 pages, one table each. Every body row is one first-baseline
//! policy ([`POLICIES`]): `AscentOffset`, `CapHeight`, `XHeight`,
//! `LeadingOffset`, `EmBoxHeight`, `FixedHeight` with
//! `MinimumFirstBaselineOffset` 30, and `AscentOffset` and `CapHeight`
//! with a minimum of 30 (does the minimum floor every policy?). The four data columns
//! ([`COLUMNS`]) cross the cell's top inset {0, 6} with its
//! `VerticalJustification` {`TopAlign`, `CenterAlign`}. Each data cell
//! holds `Hxg` in Open Sans 20 pt; the first column names the row.
//!
//! * page 1 — the policy written on the `<Cell>` itself; below the
//!   table, three text frames ask the em box of a frame
//!   ([`FRAME_CASES`]);
//! * page 2 — the same policy from a `<CellStyle>` the cell applies.
//!
//! The cell's own page comes first so the fidelity gate can hold it
//! alone (`max_pages_with_pdf` gates pages 1..N): the engine's IDML
//! reader does not take a policy off a `<CellStyle>` yet.
//!
//! Rows are 56 pt (a minimum the content never needs to grow), so every
//! cell's top is fixed and each baseline reads as `row top + inset +
//! offset` (top-aligned) or centred in the row.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_raw},
    spread::{write_spread, Spread},
    story::{write_story, Cell, Paragraph, Run, Story, Table},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "table-cell-baseline";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
/// The table frame's top-left, page pt. The frame has no inset, so the
/// table starts at its top.
pub const FRAME_AT: (f32, f32) = (60.0, 80.0);
const FRAME_W_PT: f32 = 470.0;
const FRAME_H_PT: f32 = 560.0;
pub const LABEL_COLUMN_W: f32 = 110.0;
pub const DATA_COLUMN_W: f32 = 90.0;
pub const HEADER_ROW_H: f32 = 24.0;
pub const BODY_ROW_H: f32 = 56.0;
/// The data cells' text and size.
pub const SAMPLE_TEXT: &str = "Hxg";
pub const SAMPLE_SIZE: f32 = 20.0;
const LABEL_SIZE: f32 = 8.0;

/// One first-baseline policy: a body row.
pub struct Policy {
    pub label: &'static str,
    /// `FirstBaselineOffset`.
    pub offset: &'static str,
    /// `MinimumFirstBaselineOffset`, pt.
    pub minimum: Option<&'static str>,
    /// The data cells' face.
    pub font: &'static str,
}

pub const POLICIES: [Policy; 9] = [
    Policy {
        label: "1 Ascent",
        offset: "AscentOffset",
        minimum: None,
        font: "Open Sans",
    },
    Policy {
        label: "2 Cap height",
        offset: "CapHeight",
        minimum: None,
        font: "Open Sans",
    },
    Policy {
        label: "3 x height",
        offset: "XHeight",
        minimum: None,
        font: "Open Sans",
    },
    Policy {
        label: "4 Leading",
        offset: "LeadingOffset",
        minimum: None,
        font: "Open Sans",
    },
    Policy {
        label: "5 Em box",
        // InDesign's spelling (its own export, 2026-10-12): `EmBoxHeight`
        // is ignored and reads as `AscentOffset`.
        offset: "EmboxHeight",
        minimum: None,
        font: "Open Sans",
    },
    Policy {
        label: "6 Fixed, min 30",
        offset: "FixedHeight",
        minimum: Some("30"),
        font: "Open Sans",
    },
    Policy {
        label: "7 Ascent, min 30",
        offset: "AscentOffset",
        minimum: Some("30"),
        font: "Open Sans",
    },
    Policy {
        label: "8 Cap height, min 30",
        offset: "CapHeight",
        minimum: Some("30"),
        font: "Open Sans",
    },
    Policy {
        label: "9 Em box, Lora",
        offset: "EmboxHeight",
        minimum: None,
        font: "Lora",
    },
];

/// One data column: the cell's top inset and vertical justification.
pub struct Column {
    pub label: &'static str,
    pub top_inset: f32,
    pub justification: &'static str,
}

pub const COLUMNS: [Column; 4] = [
    Column {
        label: "a inset 0, top",
        top_inset: 0.0,
        justification: "TopAlign",
    },
    Column {
        label: "b inset 0, centre",
        top_inset: 0.0,
        justification: "CenterAlign",
    },
    Column {
        label: "c inset 6, top",
        top_inset: 6.0,
        justification: "TopAlign",
    },
    Column {
        label: "d inset 6, centre",
        top_inset: 6.0,
        justification: "CenterAlign",
    },
];

/// Text frames below the table on page 1, the em box asked of a frame
/// too: `(FirstBaselineOffset, face)`, each holding [`SAMPLE_TEXT`] at
/// [`SAMPLE_SIZE`] with no inset, at [`FRAME_CASE_AT`] + `i ×`
/// [`FRAME_CASE_STEP`].
pub const FRAME_CASES: [(&str, &str); 3] = [
    ("EmboxHeight", "Open Sans"),
    ("EmboxHeight", "Lora"),
    ("AscentOffset", "Lora"),
];
pub const FRAME_CASE_AT: (f32, f32) = (60.0, 680.0);
pub const FRAME_CASE_STEP: f32 = 160.0;
const FRAME_CASE_SIZE: (f32, f32) = (140.0, 50.0);

/// Where a page's tables take the policy from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    CellStyle,
    Cell,
}

/// The two pages, in order.
pub const PAGES: [Source; 2] = [Source::Cell, Source::CellStyle];

/// The `<CellStyle>` carrying policy `i`.
pub fn cell_style_id(i: usize) -> String {
    format!("CellStyle/FirstBaseline{}", i + 1)
}

fn run(text: &str, size: f32, font: &'static str) -> Run {
    Run {
        text: text.to_string(),
        point_size: Some(size),
        fill_color: Some("Color/Black".to_string()),
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: Some(font),
        anchored_frame: None,
        extra_char_attrs: Vec::new(),
    }
}

fn paragraph(text: &str, size: f32, font: &'static str) -> Paragraph {
    Paragraph {
        runs: vec![run(text, size, font)],
        ..Paragraph::plain("")
    }
}

fn text_cell(text: &str, size: f32, font: &'static str) -> Cell {
    let mut cell = Cell::plain("");
    cell.paragraphs = vec![paragraph(text, size, font)];
    cell
}

fn data_cell(policy: usize, column: &Column, source: Source) -> Cell {
    let p = &POLICIES[policy];
    let mut cell = text_cell(SAMPLE_TEXT, SAMPLE_SIZE, p.font)
        .with_vertical_justification(column.justification);
    cell.extra_cell_attrs
        .push(("TextTopInset", crate::xml::format_f32(column.top_inset)));
    match source {
        Source::CellStyle => cell
            .extra_cell_attrs
            .push(("AppliedCellStyle", cell_style_id(policy))),
        Source::Cell => {
            cell.extra_cell_attrs
                .push(("FirstBaselineOffset", p.offset.to_string()));
            if let Some(m) = p.minimum {
                cell.extra_cell_attrs
                    .push(("MinimumFirstBaselineOffset", m.to_string()));
            }
        }
    }
    cell
}

fn table(page: usize, source: Source) -> Table {
    // Column-major: the label column, then each data column.
    let mut cells = vec![text_cell("", LABEL_SIZE, "Open Sans")];
    cells.extend(
        POLICIES
            .iter()
            .map(|p| text_cell(p.label, LABEL_SIZE, "Open Sans")),
    );
    for column in &COLUMNS {
        cells.push(text_cell(column.label, LABEL_SIZE, "Open Sans"));
        cells.extend((0..POLICIES.len()).map(|i| data_cell(i, column, source)));
    }
    let mut row_heights = vec![HEADER_ROW_H];
    row_heights.extend(std::iter::repeat(BODY_ROW_H).take(POLICIES.len()));
    let mut column_widths = vec![LABEL_COLUMN_W];
    column_widths.extend(std::iter::repeat(DATA_COLUMN_W).take(COLUMNS.len()));
    Table {
        self_id: self_id(SAMPLE, "Table", page as u32),
        header_row_count: 0,
        footer_row_count: 0,
        body_row_count: (POLICIES.len() + 1) as u32,
        column_count: (COLUMNS.len() + 1) as u32,
        applied_table_style: None,
        row_heights_pt: row_heights,
        column_widths_pt: column_widths,
        cells,
        extra_row_attrs: Vec::new(),
    }
}

fn cell_styles_xml() -> Vec<u8> {
    let mut fragment = String::from("<RootCellStyleGroup>");
    for (i, p) in POLICIES.iter().enumerate() {
        let minimum = p
            .minimum
            .map(|m| format!(" MinimumFirstBaselineOffset=\"{m}\""))
            .unwrap_or_default();
        fragment.push_str(&format!(
            "<CellStyle Self=\"{}\" Name=\"First baseline {}\" FirstBaselineOffset=\"{}\"{minimum}/>",
            cell_style_id(i),
            i + 1,
            p.offset,
        ));
    }
    fragment.push_str("</RootCellStyleGroup>");
    styles_xml_with_raw(&fragment)
}

fn frame(
    self_id: String,
    at: (f32, f32),
    size: (f32, f32),
    story: String,
    first_baseline_offset: Option<&'static str>,
) -> Rect {
    Rect {
        self_id,
        width_pt: size.0,
        height_pt: size.1,
        item_transform: translate(at.0, at.1),
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
        text_frame_pref: Some(TextFramePref {
            inset_spacing: Some([0.0; 4]),
            first_baseline_offset,
            ..Default::default()
        }),
        custom_subpaths: None,
    }
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut master_spreads = Vec::new();
    let mut spreads = Vec::new();
    let mut stories = Vec::new();
    let mut master_refs = Vec::new();
    let mut spread_refs = Vec::new();
    let mut story_refs = Vec::new();

    for (page, &source) in PAGES.iter().enumerate() {
        let seq = page as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let story_id = self_id(SAMPLE, "Story", seq);
        let title_id = self_id(SAMPLE, "TitleStory", seq);
        let title = match source {
            Source::CellStyle => "table-cell-baseline: the policy from a cell style",
            Source::Cell => "table-cell-baseline: the policy on the cell",
        };

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: self_id(SAMPLE, "MasterPage", seq),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());

        stories.push((
            title_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: title_id.clone(),
                paragraphs: vec![paragraph(title, 10.0, "Open Sans")],
            }),
        ));
        story_refs.push(title_id.clone());
        stories.push((
            story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: story_id.clone(),
                paragraphs: vec![Paragraph::plain("").with_table(table(page, source))],
            }),
        ));
        story_refs.push(story_id.clone());

        let mut items = vec![
            frame(
                self_id(SAMPLE, "TitleFrame", seq),
                (FRAME_AT.0, 40.0),
                (FRAME_W_PT, 24.0),
                title_id,
                None,
            ),
            frame(
                self_id(SAMPLE, "TextFrame", seq),
                FRAME_AT,
                (FRAME_W_PT, FRAME_H_PT),
                story_id,
                None,
            ),
        ];
        if page == 0 {
            for (i, &(offset, font)) in FRAME_CASES.iter().enumerate() {
                let id = self_id(SAMPLE, "CaseStory", i as u32);
                stories.push((
                    id.clone(),
                    write_story(&Story {
                        extra_story_attrs: Vec::new(),
                        self_id: id.clone(),
                        paragraphs: vec![paragraph(SAMPLE_TEXT, SAMPLE_SIZE, font)],
                    }),
                ));
                story_refs.push(id.clone());
                items.push(frame(
                    self_id(SAMPLE, "CaseFrame", i as u32),
                    (
                        FRAME_CASE_AT.0 + FRAME_CASE_STEP * i as f32,
                        FRAME_CASE_AT.1,
                    ),
                    FRAME_CASE_SIZE,
                    id,
                    Some(offset),
                ));
            }
        }
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(SAMPLE, "Page", seq),
                page_name: title.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: items.into_iter().map(Into::into).collect(),
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
        styles_xml: cell_styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader: page 1's data cells carry the
    /// policy on the cell, page 2's apply their policy's cell style.
    #[test]
    fn cells_read_back_with_their_policy() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        let tables: Vec<_> = doc
            .stories
            .iter()
            .flat_map(|s| s.story.paragraphs.iter())
            .filter_map(|p| p.table.as_ref())
            .collect();
        assert_eq!(tables.len(), PAGES.len());
        for (table, source) in tables.iter().zip(PAGES) {
            for (i, p) in POLICIES.iter().enumerate() {
                for col in 1..=COLUMNS.len() {
                    let name = format!("{col}:{}", i + 1);
                    let cell = table
                        .cells
                        .iter()
                        .find(|c| c.name.as_deref() == Some(name.as_str()))
                        .expect("data cell");
                    match source {
                        Source::CellStyle => {
                            assert_eq!(
                                cell.applied_cell_style.as_deref(),
                                Some(cell_style_id(i).as_str())
                            );
                            assert_eq!(cell.first_baseline_offset, None);
                        }
                        Source::Cell => {
                            assert_eq!(cell.first_baseline_offset.as_deref(), Some(p.offset));
                            assert_eq!(
                                cell.minimum_first_baseline_offset,
                                p.minimum.map(|m| m.parse().unwrap())
                            );
                        }
                    }
                }
            }
        }
    }
}
