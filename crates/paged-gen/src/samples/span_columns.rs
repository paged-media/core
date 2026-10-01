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

//! `span-columns.idml` — InDesign's span and split columns
//! (`SpanColumnType`, `SpanSplitColumnCount`, `SpanColumnMinSpaceBefore`
//! / `After`, `SplitColumnInsideGutter` / `OutsideGutter`) against
//! InDesign.
//!
//! One case per page. Every case is one story threaded through two
//! frames of the same width and column count: A at the top of the page,
//! B below it, ten 12 pt lines per column each (the `keeps` line grid:
//! Inter 10/12, a `LeadingOffset` first baseline, zero insets). Body
//! paragraphs are one line each, numbered `P01`, `P02`, ... A spanning
//! paragraph reads "Heading spans the columns", which is wider than one
//! column and narrower than two, so where it breaks says which width it
//! was set at.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "span-columns";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
/// Ten 12 pt lines per column (baseline 120 fits, 132 does not).
pub const FRAME_H: f32 = 126.0;
pub const COLUMN_W: f32 = 100.0;
pub const GUTTER: f32 = 20.0;
/// Frame A's and B's left edge and top edges on every page.
pub const FRAME_X: f32 = 72.0;
pub const FRAME_A_Y: f32 = 72.0;
pub const FRAME_B_Y: f32 = 300.0;
/// A split paragraph long enough to take two lines of a split column.
pub const LONG: &str = "Long split paragraph wraps";
/// The spanning paragraph's text.
pub const HEADING: &str = "Heading spans the columns";

/// One paragraph of a case: its text and its span/split attributes.
pub struct Para {
    pub text: String,
    pub attrs: Vec<(&'static str, &'static str)>,
    /// Typed `<Properties>` children, `(name, type, value)`.
    pub props: Vec<(&'static str, &'static str, &'static str)>,
}

/// A paragraph's span/split settings: attributes, and the column count
/// (InDesign reads `SpanSplitColumnCount` only as a typed property).
pub(crate) type Spec = (
    &'static [(&'static str, &'static str)],
    Option<(&'static str, &'static str)>,
);

/// A case: its label, the frames' column count (a one-column frame is
/// still `2 × COLUMN_W + GUTTER` wide, so a split has room) and its
/// paragraphs.
pub struct Case {
    pub name: &'static str,
    pub columns: u32,
    pub paragraphs: Vec<Para>,
}

impl Case {
    /// The frames' width.
    pub fn frame_width(&self) -> f32 {
        let c = self.columns.max(2) as f32;
        c * COLUMN_W + (c - 1.0) * GUTTER
    }
}

pub(crate) fn para(text: String, spec: Spec) -> Para {
    Para {
        text,
        attrs: spec.0.to_vec(),
        props: spec
            .1
            .map(|(ty, v)| vec![("SpanSplitColumnCount", ty, v)])
            .unwrap_or_default(),
    }
}

pub(crate) fn body(range: std::ops::RangeInclusive<u32>, spec: Spec) -> Vec<Para> {
    range.map(|n| para(format!("P{n:02}"), spec)).collect()
}

pub(crate) fn heading(spec: Spec) -> Para {
    para(HEADING.to_string(), spec)
}

pub(crate) const SINGLE: Spec = (&[], None);

pub(crate) const SPAN: &[(&str, &str)] = &[("SpanColumnType", "SpanColumns")];
const SPAN2: Spec = (SPAN, Some(("short", "2")));
pub(crate) const SPAN_ALL: Spec = (SPAN, Some(("enumeration", "All")));
const SPAN_SPACED: Spec = (
    &[
        ("SpanColumnType", "SpanColumns"),
        ("SpanColumnMinSpaceBefore", "6"),
        ("SpanColumnMinSpaceAfter", "10"),
    ],
    Some(("enumeration", "All")),
);
const SPAN_SPACE_BEFORE: Spec = (
    &[
        ("SpanColumnType", "SpanColumns"),
        ("SpaceBefore", "4"),
        ("SpanColumnMinSpaceBefore", "10"),
        ("SpaceAfter", "3"),
    ],
    Some(("enumeration", "All")),
);
const SPAN_SPACE_BEFORE_WINS: Spec = (
    &[
        ("SpanColumnType", "SpanColumns"),
        ("SpaceBefore", "10"),
        ("SpanColumnMinSpaceBefore", "4"),
        ("SpaceAfter", "9"),
        ("SpanColumnMinSpaceAfter", "5"),
    ],
    Some(("enumeration", "All")),
);
const SPAN_CENTRED: Spec = (
    &[
        ("SpanColumnType", "SpanColumns"),
        ("Justification", "CenterAlign"),
    ],
    Some(("enumeration", "All")),
);
pub(crate) const SPLIT: &[(&str, &str)] = &[("SpanColumnType", "SplitColumns")];
pub(crate) const SPLIT2: Spec = (SPLIT, Some(("short", "2")));
const SPLIT2_GUTTERS: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SplitColumnInsideGutter", "20"),
        ("SplitColumnOutsideGutter", "0"),
    ],
    Some(("short", "2")),
);
const SPLIT3: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SplitColumnInsideGutter", "10"),
        ("SplitColumnOutsideGutter", "10"),
    ],
    Some(("short", "3")),
);
const SPLIT2_SPACED: Spec = (
    &[
        ("SpanColumnType", "SplitColumns"),
        ("SplitColumnInsideGutter", "20"),
        ("SplitColumnOutsideGutter", "0"),
        ("SpanColumnMinSpaceBefore", "6"),
        ("SpanColumnMinSpaceAfter", "12"),
    ],
    Some(("short", "2")),
);

/// The cases, in page order.
pub fn cases() -> Vec<Case> {
    let cat = |parts: Vec<Vec<Para>>| parts.into_iter().flatten().collect::<Vec<_>>();
    vec![
        Case {
            name: "span 2 · heading opens the story, 2 columns",
            columns: 2,
            paragraphs: cat(vec![vec![heading(SPAN2)], body(1..=12, SINGLE)]),
        },
        Case {
            name: "span All · mid-story, 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=6, SINGLE),
                vec![heading(SPAN_ALL)],
                body(7..=18, SINGLE),
            ]),
        },
        Case {
            name: "span 2 · mid-story, 3 columns",
            columns: 3,
            paragraphs: cat(vec![
                body(1..=4, SINGLE),
                vec![heading(SPAN2)],
                body(5..=16, SINGLE),
            ]),
        },
        Case {
            name: "span All · min space before 6 after 10",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=4, SINGLE),
                vec![heading(SPAN_SPACED)],
                body(5..=14, SINGLE),
            ]),
        },
        Case {
            name: "span All · space before 4 min 10, after 3",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=4, SINGLE),
                vec![heading(SPAN_SPACE_BEFORE)],
                body(5..=14, SINGLE),
            ]),
        },
        Case {
            name: "span 2 · six above, overflow, 3 columns",
            columns: 3,
            paragraphs: cat(vec![
                body(1..=6, SINGLE),
                vec![heading(SPAN2)],
                body(7..=40, SINGLE),
            ]),
        },
        Case {
            name: "span All · six above, 3 columns",
            columns: 3,
            paragraphs: cat(vec![
                body(1..=6, SINGLE),
                vec![heading(SPAN_ALL)],
                body(7..=12, SINGLE),
            ]),
        },
        Case {
            name: "span All · space before 10 min 4, after 9 min 5",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=4, SINGLE),
                vec![heading(SPAN_SPACE_BEFORE_WINS)],
                body(5..=14, SINGLE),
            ]),
        },
        Case {
            name: "span All · no room left in frame A",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=20, SINGLE),
                vec![heading(SPAN_ALL)],
                body(21..=26, SINGLE),
            ]),
        },
        Case {
            name: "span All · centred heading, 2 columns",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=2, SINGLE),
                vec![heading(SPAN_CENTRED)],
                body(3..=6, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · a two-line paragraph straddles the split",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=3, SPLIT2_GUTTERS),
                vec![para(LONG.to_string(), SPLIT2_GUTTERS)],
                body(5..=5, SPLIT2_GUTTERS),
                body(6..=7, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · six paragraphs, default gutters, 1 column",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=7, SPLIT2),
                body(8..=9, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · five paragraphs, gutters 20/0",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=6, SPLIT2_GUTTERS),
                body(7..=8, SINGLE),
            ]),
        },
        Case {
            name: "split 3 · seven paragraphs, gutters 10/10",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=8, SPLIT3),
                body(9..=10, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · min space before 6 after 12",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2_SPACED),
                body(6..=7, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · sixteen paragraphs cross into frame B",
            columns: 1,
            paragraphs: cat(vec![
                body(1..=6, SINGLE),
                body(7..=22, SPLIT2_GUTTERS),
                body(23..=24, SINGLE),
            ]),
        },
        Case {
            name: "split 2 · inside one column of a 2-column frame",
            columns: 2,
            paragraphs: cat(vec![
                body(1..=1, SINGLE),
                body(2..=5, SPLIT2_GUTTERS),
                body(6..=8, SINGLE),
            ]),
        },
    ]
}

fn paragraph(p: &Para) -> Paragraph {
    let mut attrs: Vec<(&'static str, &'static str)> = vec![("Hyphenation", "false")];
    attrs.extend(p.attrs.iter().copied());
    Paragraph {
        extra_paragraph_attrs: attrs,
        extra_paragraph_props: p.props.clone(),
        leading: Some(LEADING),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: p.text.clone(),
            point_size: Some(POINT_SIZE),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(BODY_FONT),
            anchored_frame: None,
        }],
        ..Paragraph::plain("")
    }
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    body_story_id_in(SAMPLE, i)
}

/// Frame `f` ("A" or "B") of case `i`.
pub fn frame_id(f: &str, i: u32) -> String {
    frame_id_in(SAMPLE, f, i)
}

/// [`body_story_id`] of a sample built by [`build_cases`].
pub fn body_story_id_in(sample: &str, i: u32) -> String {
    self_id(sample, "BodyStory", i)
}

/// [`frame_id`] of a sample built by [`build_cases`].
pub fn frame_id_in(sample: &str, f: &str, i: u32) -> String {
    self_id(sample, &format!("Frame{f}"), i)
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    build_cases(SAMPLE, cases())
}

/// The `span-columns` page layout (one case per page, frames A and B)
/// over another sample's cases.
pub fn build_cases(sample: &str, cases: Vec<Case>) -> Sample {
    let mut spreads = Vec::new();
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut spread_refs = Vec::new();

    let master_id = self_id(sample, "MasterSpread", 0);
    let master_spreads = vec![(
        master_id.clone(),
        write_master(&Master {
            self_id: format!("MasterSpread/{master_id}"),
            page_self_id: self_id(sample, "MasterPage", 0),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        }),
    )];

    for (i, case) in cases.into_iter().enumerate() {
        let seq = i as u32;
        let label_story_id = self_id(sample, "LabelStory", seq);
        let body = body_story_id_in(sample, seq);

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
                paragraphs: case.paragraphs.iter().map(paragraph).collect(),
            }),
        ));
        story_refs.push(body.clone());

        let a = frame_id_in(sample, "A", seq);
        let b = frame_id_in(sample, "B", seq);
        let columns = case.columns;
        let width = case.frame_width();
        let frame = |id: &str, y: f32, prev: Option<String>, next: Option<String>| -> PageItem {
            Rect {
                self_id: id.to_string(),
                width_pt: width,
                height_pt: FRAME_H,
                item_transform: translate(FRAME_X, y),
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
                    first_baseline_offset: Some("LeadingOffset"),
                    text_column_count: (columns > 1).then_some(columns),
                    text_column_gutter: (columns > 1).then_some(GUTTER),
                    ..Default::default()
                }),
                custom_subpaths: None,
            }
            .into()
        };
        let label: PageItem = Rect {
            self_id: self_id(sample, "LabelFrame", seq),
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

        let items = vec![
            label,
            frame(&a, FRAME_A_Y, None, Some(b.clone())),
            frame(&b, FRAME_B_Y, Some(a.clone()), None),
        ];
        let spread_id = self_id(sample, "Spread", seq);
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(sample, "Page", seq),
                page_name: (seq + 1).to_string(),
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
