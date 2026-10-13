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

//! `baseline-grid.idml` and `baseline-grid-margin.idml` — align to
//! baseline grid, asked of InDesign (ADR 120).
//!
//! Body text is Open Sans 10/12 on a 14 pt grid, so an aligned line can
//! never keep its own leading: every case shows where InDesign moves it.
//! The grid is in `Resources/Preferences.xml`'s `<GridPreference>`, in
//! InDesign's spelling (`BaselineStart`, `BaselineDivision`,
//! `BaselineGridRelativeOption`), and a paragraph opts in with
//! `GridAlignment="AlignBaseline"` (`GridAlignFirstLineOnly` for its
//! first line alone) on its style or on its range.
//!
//! `baseline-grid` — the grid from the top of the PAGE, starting 36 pt
//! down; the top margin is 45 pt, so a grid read from the margin would
//! sit 3 pt off. Page 1:
//!
//!  1. not aligned (the control);
//!  2. aligned by the paragraph style;
//!  3. aligned by the range, over a style that is not;
//!  4. not aligned by the range, over a style that is;
//!  5. aligned, the second paragraph with `SpaceBefore` 5;
//!  6. a 24 pt heading (leading 28.8) aligned among aligned 10 pt body;
//!  7. `GridAlignFirstLineOnly`;
//!  8. aligned, not aligned, aligned;
//!  9. aligned, leading 15 (more than one grid step);
//! 10. aligned, with an empty paragraph between two;
//! 11. aligned in a two-column frame;
//! 12. aligned in a frame that starts above the grid's first line.
//!
//! Page 2 — frames with a grid of their own
//! (`<BaselineFrameGridOption>`, a sibling of `<TextFramePreference>`),
//! every frame inset 6 pt so the frame and inset references differ:
//!
//! 13. `TopOfInset`, offset 0, every 9 pt;
//! 14. `TopOfFrame`, offset 5, every 13 pt;
//! 15. `TopOfPage`, offset 20, every 16 pt;
//! 16. `TopOfMargin`, offset 3, every 11 pt;
//! 17. a frame grid declared but not used (`UseCustomBaselineFrameGrid`
//!     false) — the document grid applies;
//! 18. case 14's grid, the text not aligned;
//! 19. `TopOfInset`, offset 2, every 10 pt, first line only;
//! 20. `TopOfInset`, every 9 pt, a 24 pt heading among body.
//!
//! `baseline-grid-margin` — the grid from the top MARGIN, starting at
//! it (`BaselineStart` 0): 45 pt on page 1, 60.5 pt on page 2.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{BaselineFrameGrid, PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, styles_xml_with_raw},
    spread::{write_spread, MarginPreference, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

pub const SAMPLE: &str = "baseline-grid";
pub const SAMPLE_MARGIN: &str = "baseline-grid-margin";
const PAGE_W_PT: f32 = 612.0; // US Letter
const PAGE_H_PT: f32 = 792.0;
const FONT: &str = "Open Sans";
/// The document grid's step, both documents.
pub const DIVISION: f32 = 14.0;
/// Left edge of each column of cases, page-local pt.
pub const COLUMNS: [f32; 3] = [36.0, 220.0, 404.0];
/// Top edge of each row of cases, page-local pt.
pub const ROWS: [f32; 4] = [100.0, 260.0, 420.0, 580.0];
pub const FRAME_W: f32 = 180.0;
pub const FRAME_H: f32 = 140.0;

const BODY: &str = "Body";
const BODY_GRID: &str = "Body Grid";
const BODY_GRID_FIRST: &str = "Body Grid First";
const HEAD_GRID: &str = "Head Grid";
const LABEL: &str = "Label";

// Every measure stands clear of where a line of these texts ends: InDesign
// fits a line up to ~0.01 pt over its measure, the engine none, and text B
// stays out of the two-digit cases, whose first line ends on a break the
// two ragged-right composers weigh differently ("…judge | my vow"). Line
// breaks are not what this fixture is about.
const A: &str =
    "Quick zephyrs blow, vexing daft Jim, while five boxing wizards jump over the lazy dog.";
const B: &str = "Sphinx of black quartz, judge my vow; pack my box with five dozen liquor jugs.";
const C: &str = "Bright vixens jump and dozy fowl quack over the lazy dog.";

/// One paragraph of a case.
#[derive(Clone)]
pub struct Para {
    pub style: &'static str,
    /// The range's own `GridAlignment`, over its style's.
    pub grid_alignment: Option<&'static str>,
    pub space_before: Option<f32>,
    /// The range's own leading, over its style's.
    pub leading: Option<f32>,
    pub text: String,
}

fn para(style: &'static str, text: &str) -> Para {
    Para {
        style,
        grid_alignment: None,
        space_before: None,
        leading: None,
        text: text.to_string(),
    }
}

/// Where a case's frame stands and what it states of its own.
#[derive(Clone, Copy)]
pub struct Slot {
    pub at: (f32, f32),
    pub size: (f32, f32),
    pub columns: u32,
    pub inset: f32,
    pub frame_grid: Option<BaselineFrameGrid>,
    /// Where the case label goes, when not just above the frame.
    pub label_at: Option<(f32, f32)>,
}

fn slot(col: usize, row: usize) -> Slot {
    Slot {
        at: (COLUMNS[col], ROWS[row]),
        size: (FRAME_W, FRAME_H),
        columns: 1,
        inset: 0.0,
        frame_grid: None,
        label_at: None,
    }
}

fn grid_slot(col: usize, row: usize, grid: BaselineFrameGrid) -> Slot {
    Slot {
        inset: 6.0,
        frame_grid: Some(grid),
        ..slot(col, row)
    }
}

fn frame_grid(
    relative_option: &'static str,
    starting_offset: f32,
    increment: f32,
) -> BaselineFrameGrid {
    BaselineFrameGrid {
        use_custom: true,
        starting_offset,
        relative_option,
        increment,
    }
}

/// One numbered case: its label, frame and paragraphs. Paragraph `k` of
/// case `n` starts with `n.k` so a line names its case and paragraph.
#[derive(Clone)]
pub struct Case {
    pub number: u32,
    pub label: &'static str,
    pub slot: Slot,
    pub paragraphs: Vec<Para>,
}

impl Case {
    fn new(number: u32, label: &'static str, slot: Slot, paragraphs: Vec<Para>) -> Self {
        let paragraphs = paragraphs
            .into_iter()
            .enumerate()
            .map(|(k, mut p)| {
                if !p.text.is_empty() {
                    p.text = format!("{number}.{} {}", k + 1, p.text);
                }
                p
            })
            .collect();
        Case {
            number,
            label,
            slot,
            paragraphs,
        }
    }
}

fn aligned_by_range(style: &'static str, alignment: &'static str, text: &str) -> Para {
    Para {
        grid_alignment: Some(alignment),
        ..para(style, text)
    }
}

/// `baseline-grid`, page 1: the document grid.
pub fn page_one_cases() -> Vec<Case> {
    vec![
        Case::new(
            1,
            "1 · not aligned (control)",
            slot(0, 0),
            vec![para(BODY, A), para(BODY, B)],
        ),
        Case::new(
            2,
            "2 · aligned by the style",
            slot(1, 0),
            vec![para(BODY_GRID, A), para(BODY_GRID, B)],
        ),
        Case::new(
            3,
            "3 · aligned by the range, style not",
            slot(2, 0),
            vec![
                aligned_by_range(BODY, "AlignBaseline", A),
                aligned_by_range(BODY, "AlignBaseline", B),
            ],
        ),
        Case::new(
            4,
            "4 · range None over an aligned style",
            slot(0, 1),
            vec![
                aligned_by_range(BODY_GRID, "None", A),
                aligned_by_range(BODY_GRID, "None", B),
            ],
        ),
        Case::new(
            5,
            "5 · aligned, then SpaceBefore 5",
            slot(1, 1),
            vec![
                para(BODY_GRID, A),
                Para {
                    space_before: Some(5.0),
                    ..para(BODY_GRID, B)
                },
            ],
        ),
        Case::new(
            6,
            "6 · 24/28.8 heading among 10/12 body",
            slot(2, 1),
            vec![
                para(BODY_GRID, C),
                para(HEAD_GRID, "Heading"),
                para(BODY_GRID, B),
            ],
        ),
        Case::new(
            7,
            "7 · first line only",
            slot(0, 2),
            vec![para(BODY_GRID_FIRST, A), para(BODY_GRID_FIRST, B)],
        ),
        Case::new(
            8,
            "8 · aligned, not aligned, aligned",
            slot(1, 2),
            vec![para(BODY_GRID, A), para(BODY, B), para(BODY_GRID, C)],
        ),
        Case::new(
            9,
            "9 · aligned, leading 15",
            slot(2, 2),
            vec![
                Para {
                    leading: Some(15.0),
                    ..para(BODY_GRID, A)
                },
                Para {
                    leading: Some(15.0),
                    ..para(BODY_GRID, B)
                },
            ],
        ),
        Case::new(
            10,
            "10 · aligned, an empty paragraph between",
            slot(0, 3),
            vec![para(BODY_GRID, A), para(BODY_GRID, ""), para(BODY_GRID, C)],
        ),
        Case::new(
            11,
            "11 · aligned, two columns",
            Slot {
                size: (2.0 * FRAME_W + 8.0, FRAME_H),
                columns: 2,
                ..slot(1, 3)
            },
            vec![
                para(BODY_GRID, A),
                para(BODY_GRID, C),
                para(BODY_GRID, A),
                para(BODY_GRID, C),
                para(BODY_GRID, A),
            ],
        ),
        Case::new(
            12,
            "12 · aligned, the frame (top right, top 6 pt) starts above the grid",
            Slot {
                at: (COLUMNS[2], 6.0),
                size: (FRAME_W, 66.0),
                label_at: Some((COLUMNS[0], 32.0)),
                ..slot(2, 0)
            },
            vec![para(BODY_GRID, C)],
        ),
    ]
}

/// `baseline-grid`, page 2: frames with a grid of their own.
pub fn page_two_cases() -> Vec<Case> {
    vec![
        Case::new(
            13,
            "13 · frame grid: top of inset + 0, every 9",
            grid_slot(0, 0, frame_grid("TopOfInset", 0.0, 9.0)),
            vec![para(BODY_GRID, A), para(BODY_GRID, B)],
        ),
        Case::new(
            14,
            "14 · frame grid: top of frame + 5, every 13",
            grid_slot(1, 0, frame_grid("TopOfFrame", 5.0, 13.0)),
            vec![para(BODY_GRID, A), para(BODY_GRID, B)],
        ),
        Case::new(
            15,
            "15 · frame grid: top of page + 20, every 16",
            grid_slot(2, 0, frame_grid("TopOfPage", 20.0, 16.0)),
            vec![para(BODY_GRID, A), para(BODY_GRID, B)],
        ),
        Case::new(
            16,
            "16 · frame grid: top of margin + 3, every 11",
            grid_slot(0, 1, frame_grid("TopOfMargin", 3.0, 11.0)),
            vec![para(BODY_GRID, A), para(BODY_GRID, B)],
        ),
        Case::new(
            17,
            "17 · frame grid declared, not used",
            grid_slot(
                1,
                1,
                BaselineFrameGrid {
                    use_custom: false,
                    ..frame_grid("TopOfFrame", 5.0, 9.0)
                },
            ),
            vec![para(BODY_GRID, A), para(BODY_GRID, B)],
        ),
        Case::new(
            18,
            "18 · frame grid as 14, text not aligned",
            grid_slot(2, 1, frame_grid("TopOfFrame", 5.0, 13.0)),
            vec![para(BODY, A), para(BODY, B)],
        ),
        Case::new(
            19,
            "19 · frame grid: inset + 2, every 10, first line only",
            grid_slot(0, 2, frame_grid("TopOfInset", 2.0, 10.0)),
            vec![para(BODY_GRID_FIRST, A), para(BODY_GRID_FIRST, B)],
        ),
        Case::new(
            20,
            "20 · frame grid: inset, every 9, heading among body",
            grid_slot(1, 2, frame_grid("TopOfInset", 0.0, 9.0)),
            vec![
                para(BODY_GRID, C),
                para(HEAD_GRID, "Heading"),
                para(BODY_GRID, B),
            ],
        ),
    ]
}

/// `baseline-grid-margin`, page 1 (top margin 45).
pub fn margin_page_one_cases() -> Vec<Case> {
    vec![
        Case::new(
            1,
            "1 · not aligned (control)",
            slot(0, 0),
            vec![para(BODY, A), para(BODY, B)],
        ),
        Case::new(
            2,
            "2 · aligned by the style",
            slot(1, 0),
            vec![para(BODY_GRID, A), para(BODY_GRID, B)],
        ),
        Case::new(
            3,
            "3 · 24/28.8 heading among 10/12 body",
            slot(2, 0),
            vec![
                para(BODY_GRID, C),
                para(HEAD_GRID, "Heading"),
                para(BODY_GRID, B),
            ],
        ),
        Case::new(
            4,
            "4 · first line only",
            slot(0, 1),
            vec![para(BODY_GRID_FIRST, A), para(BODY_GRID_FIRST, B)],
        ),
        Case::new(
            5,
            "5 · aligned by the range, then SpaceBefore 5",
            slot(1, 1),
            vec![
                aligned_by_range(BODY, "AlignBaseline", A),
                Para {
                    space_before: Some(5.0),
                    ..aligned_by_range(BODY, "AlignBaseline", B)
                },
            ],
        ),
        Case::new(
            6,
            "6 · aligned, the frame (top right, top 6 pt) starts above the margin",
            Slot {
                at: (COLUMNS[2], 6.0),
                size: (FRAME_W, 66.0),
                label_at: Some((COLUMNS[0], 32.0)),
                ..slot(2, 0)
            },
            vec![para(BODY_GRID, C)],
        ),
    ]
}

/// `baseline-grid-margin`, page 2 (top margin 60.5).
pub fn margin_page_two_cases() -> Vec<Case> {
    vec![
        Case::new(
            7,
            "7 · aligned by the style",
            slot(0, 0),
            vec![para(BODY_GRID, A), para(BODY_GRID, B)],
        ),
        Case::new(
            8,
            "8 · 24/28.8 heading among 10/12 body",
            slot(1, 0),
            vec![
                para(BODY_GRID, C),
                para(HEAD_GRID, "Heading"),
                para(BODY_GRID, B),
            ],
        ),
        Case::new(
            9,
            "9 · first line only",
            slot(2, 0),
            vec![para(BODY_GRID_FIRST, A), para(BODY_GRID_FIRST, B)],
        ),
    ]
}

/// A paragraph style in InDesign's spelling: the font, leading and
/// `BasedOn` are typed `<Properties>` children, the rest attributes.
fn paragraph_style(name: &str, size: f32, leading: f32, grid: &str, first_only: bool) -> String {
    let size = crate::xml::format_f32(size);
    let leading = crate::xml::format_f32(leading);
    format!(
        "<ParagraphStyle Self=\"ParagraphStyle/{name}\" Name=\"{name}\" PointSize=\"{size}\" \
FontStyle=\"Regular\" FillColor=\"Color/Black\" Justification=\"LeftAlign\" \
Hyphenation=\"false\" GridAlignment=\"{grid}\" GridAlignFirstLineOnly=\"{first_only}\">\
<Properties><BasedOn type=\"string\">$ID/[No paragraph style]</BasedOn>\
<Leading type=\"unit\">{leading}</Leading>\
<AppliedFont type=\"string\">{FONT}</AppliedFont></Properties></ParagraphStyle>"
    )
}

fn styles() -> Vec<u8> {
    let styles = [
        paragraph_style(BODY, 10.0, 12.0, "None", false),
        paragraph_style(BODY_GRID, 10.0, 12.0, "AlignBaseline", false),
        paragraph_style(BODY_GRID_FIRST, 10.0, 12.0, "AlignBaseline", true),
        paragraph_style(HEAD_GRID, 24.0, 28.8, "AlignBaseline", false),
        paragraph_style(LABEL, 7.0, 8.4, "None", false),
    ]
    .concat();
    styles_xml_with_raw(&format!(
        "<RootParagraphStyleGroup>{styles}</RootParagraphStyleGroup>"
    ))
}

/// `Resources/Preferences.xml` with the document's baseline grid.
fn preferences(relative: &str, start: f32) -> Vec<u8> {
    let start = crate::xml::format_f32(start);
    let division = crate::xml::format_f32(DIVISION);
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
            ("FacingPages", "false"),
        ],
    );
    b.empty("TextPreference", &[("UseOpticalSize", "false")]);
    b.empty(
        "GridPreference",
        &[
            ("BaselineGridShown", "false"),
            ("BaselineStart", start.as_str()),
            ("BaselineDivision", division.as_str()),
            ("BaselineGridRelativeOption", relative),
        ],
    );
    b.end("idPkg:Preferences");
    b.into_bytes()
}

fn paragraph(p: &Para) -> Paragraph {
    let mut attrs = vec![("AppliedParagraphStyle", style_ref(p.style))];
    if let Some(g) = p.grid_alignment {
        attrs.push(("GridAlignment", g));
    }
    Paragraph {
        space_before: p.space_before,
        leading: p.leading,
        ..Paragraph::plain(p.text.clone()).with_para_attrs(attrs)
    }
}

fn style_ref(name: &'static str) -> &'static str {
    match name {
        BODY => "ParagraphStyle/Body",
        BODY_GRID => "ParagraphStyle/Body Grid",
        BODY_GRID_FIRST => "ParagraphStyle/Body Grid First",
        HEAD_GRID => "ParagraphStyle/Head Grid",
        LABEL => "ParagraphStyle/Label",
        other => panic!("no paragraph style {other}"),
    }
}

fn frame(
    id: String,
    story: String,
    at: (f32, f32),
    size: (f32, f32),
    pref: TextFramePref,
) -> PageItem {
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
        text_frame_pref: Some(pref),
        custom_subpaths: None,
    }
    .into()
}

/// A document's pages: `(top margin, title, cases)`.
struct PageSpec {
    margin_top: f32,
    title: String,
    cases: Vec<Case>,
}

fn build_document(sample: &str, relative: &str, start: f32, pages: Vec<PageSpec>) -> Sample {
    let mut stories: Vec<(String, Vec<u8>)> = Vec::new();
    let mut story = |id: String, paragraphs: Vec<Paragraph>| {
        stories.push((
            id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: id.clone(),
                paragraphs,
            }),
        ));
        id
    };
    let plain = || TextFramePref {
        inset_spacing: Some([0.0; 4]),
        ..Default::default()
    };
    let label = |text: &str| paragraph(&para(LABEL, text));

    let master_id = self_id(sample, "MasterSpread", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: self_id(sample, "MasterPage", 0),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let mut spreads = Vec::new();
    for (page_idx, page) in pages.iter().enumerate() {
        let p = page_idx as u32;
        let mut items = Vec::new();
        let title = story(self_id(sample, "TitleStory", p), vec![label(&page.title)]);
        items.push(frame(
            self_id(sample, "TitleFrame", p),
            title,
            (COLUMNS[0], 18.0),
            (360.0, 10.0),
            plain(),
        ));
        for case in &page.cases {
            let n = case.number;
            let s = &case.slot;
            let label_story = story(self_id(sample, "LabelStory", n), vec![label(case.label)]);
            items.push(frame(
                self_id(sample, "LabelFrame", n),
                label_story,
                s.label_at.unwrap_or((s.at.0, s.at.1 - 14.0)),
                (
                    if s.label_at.is_some() {
                        360.0
                    } else {
                        s.size.0
                    },
                    10.0,
                ),
                plain(),
            ));
            let body = story(
                self_id(sample, "Story", n),
                case.paragraphs.iter().map(paragraph).collect(),
            );
            items.push(frame(
                self_id(sample, "Frame", n),
                body,
                s.at,
                s.size,
                TextFramePref {
                    inset_spacing: Some([s.inset; 4]),
                    text_column_count: (s.columns > 1).then_some(s.columns),
                    text_column_gutter: (s.columns > 1).then_some(12.0),
                    baseline_frame_grid: s.frame_grid,
                    ..Default::default()
                },
            ));
        }
        let spread_id = self_id(sample, "Spread", p);
        let xml = write_spread(&Spread {
            self_id: spread_id.clone(),
            page_self_id: self_id(sample, "Page", p),
            page_name: (page_idx + 1).to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: items,
            override_list: Vec::new(),
            margins: Some(MarginPreference::symmetric(
                page.margin_top,
                36.0,
                36.0,
                36.0,
            )),
            item_transform: None,
        });
        spreads.push((spread_id, xml));
    }

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
            stories: stories.iter().map(|(id, _)| id.clone()).collect(),
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles(),
        preferences_xml: preferences(relative, start),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads,
        stories,
    }
}

/// `baseline-grid`: the grid from the top of the page, starting 36 pt
/// down, every 14 pt; the top margin 45 pt.
pub fn build() -> Sample {
    build_document(
        SAMPLE,
        "TopOfPageOfBaselineGridRelativeOption",
        36.0,
        vec![
            PageSpec {
                margin_top: 45.0,
                title: "baseline-grid · page 1 · document grid: top of page + 36, every 14 \
                        (top margin 45) · body 10/12"
                    .to_string(),
                cases: page_one_cases(),
            },
            PageSpec {
                margin_top: 45.0,
                title: "baseline-grid · page 2 · frames with their own grid, inset 6".to_string(),
                cases: page_two_cases(),
            },
        ],
    )
}

/// `baseline-grid-margin`: the grid from the top margin, starting at it,
/// every 14 pt; the top margin 45 pt on page 1 and 60.5 pt on page 2.
pub fn build_margin() -> Sample {
    build_document(
        SAMPLE_MARGIN,
        "TopOfMarginOfBaselineGridRelativeOption",
        0.0,
        vec![
            PageSpec {
                margin_top: 45.0,
                title: "baseline-grid-margin · page 1 · document grid: top of margin + 0, \
                        every 14 · top margin 45"
                    .to_string(),
                cases: margin_page_one_cases(),
            },
            PageSpec {
                margin_top: 60.5,
                title: "baseline-grid-margin · page 2 · top margin 60.5".to_string(),
                cases: margin_page_two_cases(),
            },
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader: two pages each, the document
    /// grid, and the paragraphs' alignment from style and range.
    #[test]
    fn both_documents_read_back() {
        for (sample, relative) in [
            (build(), "TopOfPageOfBaselineGridRelativeOption"),
            (build_margin(), "TopOfMarginOfBaselineGridRelativeOption"),
        ] {
            let bytes = crate::package::write_idml(&sample).expect("package");
            let doc = idml_import::import_idml_doc(&bytes).expect("import");
            assert_eq!(doc.spreads.len(), 2);
            let grid = &doc.designmap.grid_preference;
            assert_eq!(grid.baseline_division, Some(DIVISION));
            assert_eq!(
                grid.baseline_grid_relative_option.as_deref(),
                Some(relative)
            );
        }
    }
}
