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

//! `justification.idml` — every paragraph `Justification`, asked of
//! InDesign (ADR 120).
//!
//! A justified paragraph has two alignments: the one its lines are set
//! to (stretched to the measure) and the one its LAST line takes. The
//! nine IDML values are crossed with four last-line shapes:
//!
//! * column 1 — a paragraph of several lines ending in a short line of a
//!   few words;
//! * column 2 — a paragraph of several lines whose last line is ONE word;
//! * column 3 — a paragraph whose first line ends in a forced line break
//!   (U+2028), then runs on to a last line;
//! * column 4 — a paragraph of one word on one line.
//!
//! The binding-side values (`ToBindingSide`, `AwayFromBindingSide`) need
//! a page side, so the same matrix stands on three pages of a facing
//! document: page 1 a recto alone in its spread (the first page of a
//! facing document), pages 2 and 3 the verso and recto of one reader
//! spread (spine at spread x = 0, the verso at negative x).
//!
//! Every case is a 126 pt frame with zero insets in Open Sans 9/11, no
//! hyphenation; a numbered label above it names the page, the case, the
//! value and the shape ("2.19 · CenterJustified · forced break"). The
//! frames carry no stroke: pdftoppm draws a 0.25 pt hairline one pixel
//! wide and near black where the engine spreads it grey over two, and on
//! 108 frames a page that residue drowned out the text the fixture is
//! about (mean ΔE 1.50, of which the glyphs are a fraction).

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_facing_master, FacingMaster},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, styles_xml},
    spread::{write_facing_spread, write_spread, FacingPage, FacingSpread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "justification";
const PAGE_W_PT: f32 = 612.0; // US Letter
const PAGE_H_PT: f32 = 792.0;
const BODY_FONT: &str = "Open Sans";
const POINT_SIZE: f32 = 9.0;
const LEADING: f32 = 11.0;
const LABEL_SIZE: f32 = 6.0;
/// Left edge of each shape's column, page-local pt.
pub const COLUMNS: [f32; 4] = [36.0, 174.0, 312.0, 450.0];
/// Top edge of the first row; each value's row is [`ROW_PITCH`] below
/// the one before.
pub const ROW_TOP: f32 = 72.0;
pub const ROW_PITCH: f32 = 78.0;
pub const FRAME_W: f32 = 126.0;
const FRAME_H: f32 = 56.0;

/// U+2028 — the forced line break.
pub const LSEP: char = '\u{2028}';

/// The nine IDML `Justification` values, one per row.
pub const JUSTIFICATIONS: [&str; 9] = [
    "LeftAlign",
    "CenterAlign",
    "RightAlign",
    "LeftJustified",
    "CenterJustified",
    "RightJustified",
    "FullyJustified",
    "ToBindingSide",
    "AwayFromBindingSide",
];

/// The four last-line shapes, one per column: `(label, text)`, `|`
/// standing for a forced line break.
pub const SHAPES: [(&str, &str); 4] = [
    (
        "short last line",
        "Quick zephyrs blow, vexing daft Jim, and the five boxing wizards jump over the fence at dawn",
    ),
    (
        "one-word last line",
        "Sphinx of black quartz, judge my vow uncharacteristically extraordinarily",
    ),
    (
        "forced break",
        "Bright vixens|jump and dozy fowl quack over the lazy dog",
    ),
    ("single word", "Typography"),
];

/// Which page of the facing document, and the side InDesign sets it on.
pub struct PageSpec {
    pub name: &'static str,
    pub side: &'static str,
}

pub const PAGES: [PageSpec; 3] = [
    PageSpec {
        name: "1",
        side: "recto, alone in its spread",
    },
    PageSpec {
        name: "2",
        side: "verso",
    },
    PageSpec {
        name: "3",
        side: "recto",
    },
];

/// Case `n` (1-based, row-major: row = value, column = shape) of a page.
pub fn case_number(row: usize, column: usize) -> usize {
    row * SHAPES.len() + column + 1
}

/// The label printed above a case.
pub fn case_label(page: &PageSpec, row: usize, column: usize) -> String {
    format!(
        "{}.{} · {} · {}",
        page.name,
        case_number(row, column),
        JUSTIFICATIONS[row],
        SHAPES[column].0
    )
}

/// The story of a case on page `page_idx` (0-based).
pub fn body_story_id(page_idx: usize, row: usize, column: usize) -> String {
    self_id(
        SAMPLE,
        "Story",
        (page_idx * 100 + case_number(row, column)) as u32,
    )
}

/// Top-left of a case's frame, page-local pt.
pub fn frame_origin(row: usize, column: usize) -> (f32, f32) {
    (COLUMNS[column], ROW_TOP + row as f32 * ROW_PITCH)
}

fn run(text: String, size: f32) -> Run {
    Run {
        extra_char_attrs: Vec::new(),
        text,
        point_size: Some(size),
        fill_color: Some("Color/Black".to_string()),
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: Some(BODY_FONT),
        anchored_frame: None,
    }
}

fn case_paragraph(justification: &'static str, text: &str) -> Paragraph {
    let text: String = text
        .chars()
        .map(|c| if c == '|' { LSEP } else { c })
        .collect();
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(LEADING),
        justification: Some(justification),
        runs: vec![run(text, POINT_SIZE)],
        ..Paragraph::plain("")
    }
}

fn label_paragraph(text: String, size: f32) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(size * 1.2),
        justification: Some("LeftAlign"),
        runs: vec![run(text, size)],
        ..Paragraph::plain("")
    }
}

fn frame(id: String, story: String, size: (f32, f32), at: (f32, f32)) -> PageItem {
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
    .into()
}

/// One story holding `paragraph`, registered in `stories`.
fn story(stories: &mut Vec<(String, Vec<u8>)>, id: String, paragraph: Paragraph) -> String {
    stories.push((
        id.clone(),
        write_story(&Story {
            extra_story_attrs: Vec::new(),
            self_id: id.clone(),
            paragraphs: vec![paragraph],
        }),
    ));
    id
}

/// The page's frames, shifted by `dx` into spread coordinates (the
/// verso of a reader spread sits at negative x).
fn page_items(
    page_idx: usize,
    page: &PageSpec,
    dx: f32,
    stories: &mut Vec<(String, Vec<u8>)>,
) -> Vec<PageItem> {
    let mut items = Vec::new();
    let base = (page_idx * 100) as u32;
    let title = story(
        stories,
        self_id(SAMPLE, "TitleStory", page_idx as u32),
        label_paragraph(
            format!(
                "justification · page {} · {} · columns: short last line, one-word last line, \
                 forced break, single word",
                page.name, page.side
            ),
            7.0,
        ),
    );
    items.push(frame(
        self_id(SAMPLE, "TitleFrame", page_idx as u32),
        title,
        (540.0, 12.0),
        (36.0 + dx, 36.0),
    ));
    for (row, justification) in JUSTIFICATIONS.iter().enumerate() {
        for (column, (_, text)) in SHAPES.iter().enumerate() {
            let seq = base + case_number(row, column) as u32;
            let (x, y) = frame_origin(row, column);
            let label = story(
                stories,
                self_id(SAMPLE, "LabelStory", seq),
                label_paragraph(case_label(page, row, column), LABEL_SIZE),
            );
            items.push(frame(
                self_id(SAMPLE, "LabelFrame", seq),
                label,
                (FRAME_W, 9.0),
                (x + dx, y - 10.0),
            ));
            let body = story(
                stories,
                body_story_id(page_idx, row, column),
                case_paragraph(justification, text),
            );
            items.push(frame(
                self_id(SAMPLE, "Frame", seq),
                body,
                (FRAME_W, FRAME_H),
                (x + dx, y),
            ));
        }
    }
    items
}

/// `Resources/Preferences.xml` stating the facing document outright:
/// the binding side is what this sample asks about, so it must not rest
/// on InDesign's new-document default. No `PagesPerDocument`: with it
/// InDesign 20.0.1 opened the file with two blank pages ahead of the
/// three the spreads hold.
fn preferences() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Preferences",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.empty(
        "DocumentPreference",
        &[
            ("PageHeight", "792"),
            ("PageWidth", "612"),
            ("FacingPages", "true"),
            ("PageBinding", "LeftToRight"),
        ],
    );
    b.empty("TextPreference", &[("UseOpticalSize", "false")]);
    b.end("idPkg:Preferences");
    b.into_bytes()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master = write_facing_master(&FacingMaster {
        self_id: format!("MasterSpread/{master_id}"),
        name_prefix: "A".to_string(),
        base_name: "Master".to_string(),
        verso_page_self_id: self_id(SAMPLE, "MasterPage", 0),
        recto_page_self_id: self_id(SAMPLE, "MasterPage", 1),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let applied_master = format!("MasterSpread/{master_id}");
    let page_id = |i: u32| self_id(SAMPLE, "Page", i);

    // Spread 0 — page 1, the recto that opens a facing document.
    let p1_items = page_items(0, &PAGES[0], 0.0, &mut stories);
    let spread0 = self_id(SAMPLE, "Spread", 0);
    let spread0_xml = write_spread(&Spread {
        self_id: spread0.clone(),
        page_self_id: page_id(0),
        page_name: PAGES[0].name.to_string(),
        applied_master: applied_master.clone(),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: p1_items,
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    // Spread 1 — pages 2 (verso) and 3 (recto) of one reader spread.
    let mut p23_items = page_items(1, &PAGES[1], -PAGE_W_PT, &mut stories);
    p23_items.extend(page_items(2, &PAGES[2], 0.0, &mut stories));
    let spread1 = self_id(SAMPLE, "Spread", 1);
    let side = |i: usize| FacingPage {
        self_id: page_id(i as u32),
        name: PAGES[i].name.to_string(),
        applied_master: applied_master.clone(),
        override_list: Vec::new(),
        margins: None,
    };
    let spread1_xml = write_facing_spread(&FacingSpread {
        self_id: spread1.clone(),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        verso: side(1),
        recto: side(2),
        page_items: p23_items,
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread0.clone(), spread1.clone()],
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread0, spread0_xml), (spread1, spread1_xml)],
        stories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader: three pages, the verso left of
    /// the spine, and every value on every page.
    #[test]
    fn three_pages_every_value_on_each() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        assert_eq!(doc.spreads.len(), 2);
        assert_eq!(doc.spreads[0].spread.pages.len(), 1);
        assert_eq!(doc.spreads[1].spread.pages.len(), 2);
        let verso = &doc.spreads[1].spread.pages[0];
        assert_eq!(verso.item_transform.map(|m| m[4]), Some(-PAGE_W_PT));
        let values: Vec<_> = doc
            .stories
            .iter()
            .flat_map(|s| s.story.paragraphs.iter())
            .filter_map(|p| p.justification.map(|j| j.as_idml()))
            .collect();
        for j in JUSTIFICATIONS {
            assert_eq!(
                values.iter().filter(|v| **v == j).count(),
                PAGES.len() * SHAPES.len() + usize::from(j == "LeftAlign") * PAGES.len() * 37,
                "{j}"
            );
        }
    }
}
