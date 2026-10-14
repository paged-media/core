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

//! `table-cell-tints.idml` — a cell's fill tint, on the cell and from
//! its cell style, asked of InDesign (A-14).
//!
//! One table of 90 × 60 pt cells, each case numbered in its cell:
//!
//! * 1a — the cell's own `FillColor` (teal) and `FillTint="40"`;
//! * 1b — cell style `Teal40` (`FillColor` teal, `FillTint="40"`);
//! * 1c — `Teal40` with the cell's `FillTint="70"`;
//! * 1d — style `Black` based on `Teal40` (`FillColor` black, no tint);
//! * 2a — the cell's black at `FillTint="20"`;
//! * 2b — style `Teal` (no tint) with the cell's `FillTint="25"`;
//! * 2c — the cell's teal with no tint;
//! * 2d — `Teal40` with the cell's `FillColor` black and no tint;
//! * 3a — the cell's `FillTint="50"` with no fill at all;
//! * 3b — style `Tint60` (`FillTint="60"`, no colour) with the cell's teal;
//! * 3c — style `Teal40` with the cell's `FillTint="100"`;
//! * 3d — the cell's teal at `FillTint="0"`.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{Rect, TextFramePref},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml_with_raw,
        ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Cell, Paragraph, Story, Table},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "table-cell-tints";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const TEAL: &str = "Color/Teal";
const BLACK: &str = "Color/Black";

/// One case: the cell's style, fill colour and fill tint.
struct Case {
    label: &'static str,
    style: Option<&'static str>,
    fill: Option<&'static str>,
    tint: Option<&'static str>,
}

const fn case(
    label: &'static str,
    style: Option<&'static str>,
    fill: Option<&'static str>,
    tint: Option<&'static str>,
) -> Case {
    Case {
        label,
        style,
        fill,
        tint,
    }
}

/// Row-major: three rows of four.
const CASES: [Case; 12] = [
    case("1a", None, Some(TEAL), Some("40")),
    case("1b", Some("CellStyle/Teal40"), None, None),
    case("1c", Some("CellStyle/Teal40"), None, Some("70")),
    case("1d", Some("CellStyle/Black"), None, None),
    case("2a", None, Some(BLACK), Some("20")),
    case("2b", Some("CellStyle/Teal"), None, Some("25")),
    case("2c", None, Some(TEAL), None),
    case("2d", Some("CellStyle/Teal40"), Some(BLACK), None),
    case("3a", None, None, Some("50")),
    case("3b", Some("CellStyle/Tint60"), Some(TEAL), None),
    case("3c", Some("CellStyle/Teal40"), None, Some("100")),
    case("3d", None, Some(TEAL), Some("0")),
];
const ROWS: usize = 3;
const COLUMNS: usize = 4;
const CELL_W: f32 = 90.0;
const CELL_H: f32 = 60.0;

fn cell(c: &Case) -> Cell {
    let mut cell = Cell::plain(c.label);
    cell.fill_color = c.fill.map(str::to_string);
    if let Some(s) = c.style {
        cell.extra_cell_attrs
            .push(("AppliedCellStyle", s.to_string()));
    }
    if let Some(t) = c.tint {
        cell.extra_cell_attrs.push(("FillTint", t.to_string()));
    }
    cell
}

fn table() -> Table {
    // Column-major, as the builder writes cells.
    let cells = (0..COLUMNS)
        .flat_map(|col| (0..ROWS).map(move |row| cell(&CASES[row * COLUMNS + col])))
        .collect();
    Table {
        self_id: self_id(SAMPLE, "Table", 0),
        header_row_count: 0,
        footer_row_count: 0,
        body_row_count: ROWS as u32,
        column_count: COLUMNS as u32,
        applied_table_style: None,
        row_heights_pt: vec![CELL_H; ROWS],
        column_widths_pt: vec![CELL_W; COLUMNS],
        cells,
        extra_row_attrs: Vec::new(),
    }
}

fn styles() -> Vec<u8> {
    styles_xml_with_raw(
        "<RootCellStyleGroup>\
         <CellStyle Self=\"CellStyle/Teal40\" Name=\"Teal40\" FillColor=\"Color/Teal\" FillTint=\"40\"/>\
         <CellStyle Self=\"CellStyle/Teal\" Name=\"Teal\" FillColor=\"Color/Teal\"/>\
         <CellStyle Self=\"CellStyle/Black\" Name=\"Black\" FillColor=\"Color/Black\">\
         <Properties><BasedOn type=\"object\">CellStyle/Teal40</BasedOn></Properties></CellStyle>\
         <CellStyle Self=\"CellStyle/Tint60\" Name=\"Tint60\" FillTint=\"60\"/>\
         </RootCellStyleGroup>",
    )
}

fn frame(id: String, at: (f32, f32), size: (f32, f32), story: String) -> Rect {
    Rect {
        self_id: id,
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
            ..Default::default()
        }),
        custom_subpaths: None,
    }
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let title_id = self_id(SAMPLE, "TitleStory", 0);
    let story_id = self_id(SAMPLE, "Story", 0);
    let stories = vec![
        (
            title_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: title_id.clone(),
                paragraphs: vec![Paragraph::plain(
                    "table-cell-tints: a cell's fill tint, on the cell and from its style",
                )],
            }),
        ),
        (
            story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: story_id.clone(),
                paragraphs: vec![Paragraph::plain("").with_table(table())],
            }),
        ),
    ];
    let items = vec![
        frame(
            self_id(SAMPLE, "TitleFrame", 0),
            (60.0, 40.0),
            (470.0, 24.0),
            title_id.clone(),
        ),
        frame(
            self_id(SAMPLE, "TextFrame", 0),
            (60.0, 80.0),
            (470.0, 300.0),
            story_id.clone(),
        ),
    ];
    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id.clone()],
            stories: vec![title_id, story_id],
        }),
        graphic_xml: graphic_xml_with_extras(&[ExtraColor {
            self_id: TEAL.to_string(),
            name: "Teal".to_string(),
            space: "CMYK",
            value: "85 10 40 0".to_string(),
        }]),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: self_id(SAMPLE, "MasterPage", 0),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        )],
        spreads: vec![(
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id,
                page_self_id: self_id(SAMPLE, "Page", 0),
                page_name: "1".to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: items.into_iter().map(Into::into).collect(),
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        )],
        stories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader: the cells and styles carry the
    /// tints the cases name.
    #[test]
    fn tints_read_back_on_cells_and_styles() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        let style = doc.styles.resolve_cell("CellStyle/Black");
        assert_eq!(style.fill_color.as_deref(), Some("Color/Black"));
        assert_eq!(style.fill_tint, Some(40.0), "based on Teal40");
        let table = doc
            .stories
            .iter()
            .flat_map(|s| s.story.paragraphs.iter())
            .find_map(|p| p.table.as_ref())
            .expect("table");
        let tints: Vec<Option<f32>> = table.cells.iter().map(|c| c.fill_tint).collect();
        assert!(
            tints.contains(&Some(70.0)) && tints.contains(&Some(0.0)),
            "{tints:?}"
        );
    }
}
