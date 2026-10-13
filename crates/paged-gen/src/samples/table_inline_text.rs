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

//! `table-inline-text.idml` — a table that shares a paragraph with text,
//! asked of InDesign.
//!
//! InDesign keeps a table as a CHARACTER of its paragraph (its story
//! holds one where the table stands) and sets it on a line of its own:
//! the text before it ends a line, the text after it starts the next, and
//! both stay lines of the one paragraph. Its own export spells that as
//! text and table sharing one `<CharacterStyleRange>`:
//!
//! ```xml
//! <Content>Before table </Content><Table …>…</Table><Content>After table</Content>
//! ```
//!
//! and two paragraphs with a `<Br/>` (the paragraph mark) between them —
//! `Lead paragraph<Br/><Table …/>After table` is "Lead paragraph" and a
//! paragraph that opens with the table (InDesign 20.0.1, built through its
//! DOM and exported). Every story here is written the way InDesign wrote
//! it, one numbered case per frame:
//!
//!  1. one paragraph: text, table, text;
//!  2. separate paragraphs: text, mark, table, mark, text;
//!  3. the table opens the second paragraph;
//!  4. a numbered paragraph with space before / after and a first-line
//!     indent: the text after the table takes no number, no indent and no
//!     space before, and the next item is number 2;
//!  5. a justified paragraph: the line before the table is justified like
//!     any line that does not end the paragraph;
//!  6. a centred paragraph: the table's line is centred too;
//!  7. the table ends its paragraph;
//!  8. a paragraph mark before the table at the end of the range (the
//!     annual's captioned tables): two paragraphs;
//!  9. a right-aligned paragraph.
//!
//! Inter 10 pt, auto leading, frames 280 pt wide with no inset and an
//! `AscentOffset` first baseline — the geometry the DOM probe measured.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, write_table, Cell, Paragraph, Run, Story, Table},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "table-inline-text";
const PAGE_W_PT: f32 = 700.0;
const PAGE_H_PT: f32 = 1230.0;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: &str = "10";
pub const FRAME_W: f32 = 280.0;
const FRAME_H: f32 = 200.0;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

/// One piece of a range's content, in order.
#[derive(Clone, Copy)]
pub enum Piece {
    Text(&'static str),
    /// `<Br/>`: InDesign's paragraph mark.
    Mark,
    /// The case's next table.
    Table,
}

/// One `<ParagraphStyleRange>` holding one `<CharacterStyleRange>`.
pub struct Range {
    pub attrs: Vec<(&'static str, &'static str)>,
    pub pieces: Vec<Piece>,
}

pub struct Case {
    pub name: &'static str,
    pub ranges: Vec<Range>,
}

fn range(pieces: Vec<Piece>) -> Range {
    Range {
        attrs: Vec::new(),
        pieces,
    }
}

/// The cases, in frame order.
pub fn cases() -> Vec<Case> {
    use Piece::{Mark, Table as T, Text};
    let numbered = vec![
        ("BulletsAndNumberingListType", "NumberedList"),
        ("SpaceBefore", "12"),
        ("SpaceAfter", "6"),
        ("FirstLineIndent", "18"),
    ];
    vec![
        Case {
            name: "1 one paragraph: text, table, text",
            ranges: vec![range(vec![Text("Before table "), T, Text("After table")])],
        },
        Case {
            name: "2 separate paragraphs",
            ranges: vec![range(vec![
                Text("Before table"),
                Mark,
                T,
                Mark,
                Text("After table"),
            ])],
        },
        Case {
            name: "3 the table opens the second paragraph",
            ranges: vec![range(vec![
                Text("Lead paragraph"),
                Mark,
                T,
                Text("After table"),
            ])],
        },
        Case {
            name: "4 numbered, spacing, first indent",
            ranges: vec![
                Range {
                    attrs: numbered.clone(),
                    pieces: vec![Text("Before table "), T, Text("After table"), Mark],
                },
                Range {
                    attrs: numbered,
                    pieces: vec![Text("Second item")],
                },
            ],
        },
        Case {
            name: "5 justified text wraps around the table",
            ranges: vec![Range {
                attrs: vec![("Justification", "LeftJustified")],
                pieces: vec![
                    Text(
                        "Justified text that runs long enough to wrap onto a second \
                         line before the table stands here and ends",
                    ),
                    T,
                    Text(
                        " After table text that also runs long enough to wrap onto a \
                         second line.",
                    ),
                ],
            }],
        },
        Case {
            name: "6 centred paragraph",
            ranges: vec![
                Range {
                    attrs: vec![("Justification", "CenterAlign")],
                    pieces: vec![
                        Text("Centred before table"),
                        T,
                        Text(" centred after table"),
                        Mark,
                    ],
                },
                range(vec![Text("Next paragraph")]),
            ],
        },
        Case {
            name: "7 the table ends its paragraph",
            ranges: vec![
                range(vec![Text("Text before the table "), T, Mark]),
                range(vec![Text("Following paragraph")]),
            ],
        },
        Case {
            name: "8 a paragraph mark, then the table",
            ranges: vec![range(vec![Text("Caption text"), Mark, T])],
        },
        Case {
            name: "9 right-aligned paragraph",
            ranges: vec![
                Range {
                    attrs: vec![("Justification", "RightAlign")],
                    pieces: vec![
                        Text("Right before table"),
                        T,
                        Text(" right after table"),
                        Mark,
                    ],
                },
                range(vec![Text("Next paragraph")]),
            ],
        },
    ]
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// Top-left of case `i`'s body frame, page-local pt.
pub fn frame_origin(i: u32) -> (f32, f32) {
    let col = i % 2;
    let row = i / 2;
    (60.0 + col as f32 * 320.0, 70.0 + row as f32 * 230.0)
}

/// Text of a cell: Inter 10 pt, like the body.
fn cell(text: String) -> Cell {
    let mut c = Cell::plain("");
    c.paragraphs = vec![Paragraph {
        runs: vec![Run {
            point_size: Some(10.0),
            applied_font: Some(BODY_FONT),
            ..Run::with_char_attrs(text, Vec::new())
        }],
        ..Paragraph::plain("")
    }];
    c
}

/// Case `case`'s `n`-th table (both 1-based in its cell text): two
/// columns of 50 pt, two rows of 18 pt.
fn table(case: u32, n: u32) -> Table {
    let id = self_id(SAMPLE, "Table", case * 10 + n);
    let tag = if n == 1 {
        format!("T{case}")
    } else {
        format!("T{case}.{n}")
    };
    Table {
        self_id: id,
        header_row_count: 0,
        footer_row_count: 0,
        body_row_count: 2,
        column_count: 2,
        applied_table_style: None,
        row_heights_pt: vec![18.0, 18.0],
        column_widths_pt: vec![50.0, 50.0],
        // Column-major: A1, A2, B1, B2.
        cells: ["A1", "A2", "B1", "B2"]
            .iter()
            .map(|c| cell(format!("{tag}-{c}")))
            .collect(),
        extra_row_attrs: Vec::new(),
    }
}

/// The case's story, in InDesign's spelling (see the module doc).
fn story_xml(id: &str, case: u32, ranges: &[Range]) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", id)]);
    let mut tables = 0;
    for r in ranges {
        let mut attrs = vec![
            (
                "AppliedParagraphStyle",
                "ParagraphStyle/$ID/[No paragraph style]",
            ),
            ("Hyphenation", "false"),
        ];
        attrs.extend(r.attrs.iter().copied());
        b.start("ParagraphStyleRange", &attrs);
        b.start(
            "CharacterStyleRange",
            &[
                (
                    "AppliedCharacterStyle",
                    "CharacterStyle/$ID/[No character style]",
                ),
                ("FontStyle", "Regular"),
                ("PointSize", POINT_SIZE),
                ("FillColor", "Color/Black"),
            ],
        );
        b.start("Properties", &[]);
        b.start("AppliedFont", &[("type", "string")]);
        b.text(BODY_FONT);
        b.end("AppliedFont");
        b.end("Properties");
        for piece in &r.pieces {
            match piece {
                Piece::Text(t) => {
                    b.start("Content", &[]);
                    b.text(t);
                    b.end("Content");
                }
                Piece::Mark => b.empty("Br", &[]),
                Piece::Table => {
                    tables += 1;
                    write_table(&mut b, &table(case, tables));
                }
            }
        }
        b.end("CharacterStyleRange");
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

fn frame(id: String, story: String, w: f32, h: f32, at: (f32, f32), body: bool) -> PageItem {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
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
        text_frame_pref: body.then(|| TextFramePref {
            inset_spacing: Some([0.0, 0.0, 0.0, 0.0]),
            first_baseline_offset: Some("AscentOffset"),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items = Vec::new();

    for (i, case) in cases().into_iter().enumerate() {
        let seq = i as u32;
        let (x, y) = frame_origin(seq);
        let label_story = self_id(SAMPLE, "LabelStory", seq);
        stories.push((
            label_story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story.clone(),
                paragraphs: vec![Paragraph::plain(case.name)],
            }),
        ));
        story_refs.push(label_story.clone());
        items.push(frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label_story,
            FRAME_W,
            20.0,
            (x, y - 24.0),
            false,
        ));

        let body = body_story_id(seq);
        stories.push((body.clone(), story_xml(&body, seq + 1, &case.ranges)));
        story_refs.push(body.clone());
        items.push(frame(
            self_id(SAMPLE, "Frame", seq),
            body,
            FRAME_W,
            FRAME_H,
            (x, y),
            true,
        ));
    }

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: self_id(SAMPLE, "MasterPage", 0),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: self_id(SAMPLE, "Page", 0),
        page_name: "1".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: items,
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id.clone()],
            stories: story_refs,
        }),
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}
