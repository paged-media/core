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

//! `tab-breaks.idml` — where a line breaks when a tab decides it,
//! against InDesign.
//!
//! Page 1 holds one frame per case, each a short paragraph whose tab
//! lands near the frame's right edge, so the width the tab TAKES (not
//! the tab glyph's own advance) decides whether the next word fits:
//! a left stop with a word that no longer fits after it, a stop so close
//! to the edge that the word after the tab cannot fit, several tabs
//! exceeding the measure (default and explicit stops), right / decimal /
//! center stops near the edge, a tab past the last explicit stop, a stop
//! beyond the frame, and justified text with a tab.
//!
//! Page 2 holds four sweeps, one frame per alignment (Left, Right,
//! Center, Decimal), each thirteen one-line paragraphs `Chapter\t12`
//! (`Chapter\t1.5` for decimal) whose single stop steps across the point
//! where the segment after the tab would meet the pen — so the minimum
//! width each alignment gives a tab reads straight off the export.
//! Below them, edge probes: one-line paragraphs whose segment ends just
//! inside, exactly on, or just past the frame's right edge.
//!
//! Frames have zero insets, no stroke and `LeadingOffset`, so a frame-local x is a
//! tab-stop position and every baseline sits on the 12 pt grid. Text is
//! Inter 10/12, hyphenation off (registered by `tab-breaks.fonts.sh`).

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story, TabStop},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "tab-breaks";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
pub const FRAME_W: f32 = 200.0;

/// One stop: position (pt, frame-local) and IDML alignment.
pub type Stop = (f32, &'static str);

/// One paragraph of a case.
pub struct Para {
    pub text: String,
    pub stops: Vec<Stop>,
}

/// One frame.
pub struct Case {
    pub name: String,
    pub page: u32,
    pub origin: (f32, f32),
    pub height: f32,
    pub justification: Option<&'static str>,
    pub left_indent: f32,
    pub first_line_indent: f32,
    pub paras: Vec<Para>,
}

fn para(text: &str, stops: &[Stop]) -> Para {
    Para {
        text: text.to_string(),
        stops: stops.to_vec(),
    }
}

/// Page-1 case at grid slot `i` (two columns, seven rows).
fn break_case(i: u32, name: &str, paras: Vec<Para>) -> Case {
    Case {
        name: name.to_string(),
        page: 0,
        origin: (72.0 + (i % 2) as f32 * 240.0, 72.0 + (i / 2) as f32 * 104.0),
        height: 60.0,
        justification: None,
        left_indent: 0.0,
        first_line_indent: 0.0,
        paras,
    }
}

/// The stop positions of a sweep, centred on `s0` (where the segment
/// would start exactly at the pen with a zero-width tab), plus one stop
/// behind the pen.
pub fn sweep_stops(s0: f32) -> Vec<f32> {
    let mut v: Vec<f32> = [
        -3.0, -2.0, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0,
    ]
    .iter()
    .map(|d| s0 + d)
    .collect();
    v.push(30.0);
    v
}

/// The sweeps: (alignment, text, s0).
pub const SWEEPS: [(&str, &str, f32); 4] = [
    ("LeftAlign", "Chapter\t12", 38.0),
    ("RightAlign", "Chapter\t12", 48.0),
    ("CenterAlign", "Chapter\t12", 43.0),
    ("CharacterAlign", "Chapter\t1.5", 42.0),
];

/// Every frame, page 1 then page 2.
pub fn cases() -> Vec<Case> {
    let mut v = vec![
        // A left stop near the edge: "gamma" no longer fits after it.
        break_case(
            0,
            "c00 left 150 · next word overflows",
            vec![para("Alpha\tBeta gamma", &[(150.0, "LeftAlign")])],
        ),
        // The word after the tab itself does not fit.
        break_case(
            1,
            "c01 left 185 · word after tab overflows",
            vec![para("Alpha\tBeta gamma", &[(185.0, "LeftAlign")])],
        ),
        // Default 36 pt stops: the seventh column lands at 216.
        break_case(
            2,
            "c02 default stops · seven columns",
            vec![para("a1\tb2\tc3\td4\te5\tf6\tg7", &[])],
        ),
        // Explicit stops: the last column starts at 190.
        break_case(
            3,
            "c03 stops 70 140 190",
            vec![para(
                "One\tTwo\tThree\tFour",
                &[(70.0, "LeftAlign"), (140.0, "LeftAlign"), (190.0, "LeftAlign")],
            )],
        ),
        // A right stop on the edge, then text that runs past where the
        // segment would have to start.
        break_case(
            4,
            "c04 right 200",
            vec![
                para("Chapter one\t12", &[(200.0, "RightAlign")]),
                para(
                    "A long chapter title runs on here, okay\t123",
                    &[(200.0, "RightAlign")],
                ),
            ],
        ),
        // A right stop the pre-tab text already overlaps (not at the edge).
        break_case(
            5,
            "c05 right 160 · overlapped",
            vec![para(
                "A long chapter title runs on here\t123",
                &[(160.0, "RightAlign")],
            )],
        ),
        // A decimal stop whose tail runs past the edge.
        break_case(
            6,
            "c06 decimal 190",
            vec![
                para("Total due\t1234.50", &[(190.0, "CharacterAlign")]),
                para("Total\t12.50", &[(190.0, "CharacterAlign")]),
            ],
        ),
        // A centre stop whose segment runs past the edge.
        break_case(
            7,
            "c07 center 195",
            vec![
                para("Name\tCentered", &[(195.0, "CenterAlign")]),
                para("Name\tMid", &[(195.0, "CenterAlign")]),
            ],
        ),
        // The second tab is past the last explicit stop: next default 216.
        break_case(
            8,
            "c08 past last stop · default 216",
            vec![para(
                "Key\tvalue words that run on and on\tend",
                &[(50.0, "LeftAlign")],
            )],
        ),
        // Justified: a tab with spaces before and after it.
        Case {
            justification: Some("LeftJustified"),
            ..break_case(
                9,
                "c09 justified · stop 60",
                vec![para(
                    "Big term\tdefinition words that wrap onto a second line so the first is set justified",
                    &[(60.0, "LeftAlign")],
                )],
            )
        },
        // Justified, the tab late in the line.
        Case {
            justification: Some("LeftJustified"),
            ..break_case(
                10,
                "c10 justified · stop 100",
                vec![para(
                    "One two three\tfour five six seven eight nine ten eleven twelve",
                    &[(100.0, "LeftAlign")],
                )],
            )
        },
        // A hanging indent: where the wrapped text after the tab starts.
        Case {
            left_indent: 20.0,
            first_line_indent: -20.0,
            ..break_case(
                11,
                "c11 hanging 20 · wrap after tab",
                vec![para(
                    "Head\tbody words that are long enough to wrap to the next line here",
                    &[],
                )],
            )
        },
        // An explicit stop beyond the frame.
        break_case(
            12,
            "c12 left 250 · beyond the frame",
            vec![para("Alpha\tBeta gamma", &[(250.0, "LeftAlign")])],
        ),
        // A space before the tab: break at the space or at the tab?
        break_case(
            13,
            "c13 left 195 · space before tab",
            vec![para("Alpha beta\tGamma", &[(195.0, "LeftAlign")])],
        ),
    ];
    for (i, (align, text, s0)) in SWEEPS.iter().enumerate() {
        let i = i as u32;
        v.push(Case {
            name: format!("s{i} sweep {align} s0 {s0}"),
            page: 1,
            origin: (72.0 + (i % 2) as f32 * 240.0, 72.0 + (i / 2) as f32 * 220.0),
            height: 160.0,
            justification: None,
            left_indent: 0.0,
            first_line_indent: 0.0,
            paras: sweep_stops(*s0)
                .into_iter()
                .map(|s| Para {
                    text: text.to_string(),
                    stops: vec![(s, *align)],
                })
                .collect(),
        });
    }
    // Where "fits" ends: one-line paragraphs whose segment ends just
    // inside or just past the 200 pt edge (or exactly on it).
    let probes: [(&str, Stop); 12] = [
        ("Chapter one\t12", (195.0, "RightAlign")),
        ("Chapter one\t12", (199.5, "RightAlign")),
        ("Chapter one\t12", (199.9, "RightAlign")),
        ("Chapter one\t12", (200.0, "RightAlign")),
        ("Chapter one\t12", (200.1, "RightAlign")),
        ("Alpha\tBeta", (178.6, "LeftAlign")),
        ("Alpha\tBeta", (178.8, "LeftAlign")),
        ("Name\tMid", (191.1, "CenterAlign")),
        ("Name\tMid", (191.3, "CenterAlign")),
        ("Total\t1.5", (191.1, "CharacterAlign")),
        ("Total\t1.5", (191.3, "CharacterAlign")),
        ("Alpha\tBeta", (210.0, "RightAlign")),
    ];
    v.push(Case {
        name: "e0 edge probes".to_string(),
        page: 1,
        origin: (72.0, 512.0),
        height: 300.0,
        justification: None,
        left_indent: 0.0,
        first_line_indent: 0.0,
        paras: probes.iter().map(|(t, s)| para(t, &[*s])).collect(),
    });
    v
}

/// The body story of case `i` (0-based, page 1 then page 2).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

fn paragraph(c: &Case, p: &Para) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        justification: c.justification,
        leading: Some(LEADING),
        first_line_indent: Some(c.first_line_indent),
        left_indent: Some(c.left_indent),
        tab_list: p
            .stops
            .iter()
            .map(|&(position_pt, alignment)| TabStop {
                position_pt,
                alignment,
                leader: None,
            })
            .collect(),
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

fn frame(id: String, story: String, w: f32, h: f32, at: (f32, f32), body: bool) -> PageItem {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(at.0, at.1),
        fill_color: None,
        // No stroke: InDesign insets a frame's text by half a
        // centre-aligned stroke (a 0.25 pt border made the measure
        // 199.75 and moved every line 0.125 right), which would blur
        // exactly the edge this fixture measures.
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
            first_baseline_offset: Some("LeadingOffset"),
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
    let mut items: [Vec<PageItem>; 2] = [Vec::new(), Vec::new()];

    for (i, case) in cases().iter().enumerate() {
        let seq = i as u32;
        let (x, y) = case.origin;
        let label_story = self_id(SAMPLE, "LabelStory", seq);
        stories.push((
            label_story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story.clone(),
                paragraphs: vec![Paragraph::plain(case.name.clone())],
            }),
        ));
        story_refs.push(label_story.clone());
        let page = case.page as usize;
        items[page].push(frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label_story,
            FRAME_W,
            20.0,
            (x, y - 24.0),
            false,
        ));

        let body = body_story_id(seq);
        stories.push((
            body.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: body.clone(),
                paragraphs: case.paras.iter().map(|p| paragraph(case, p)).collect(),
            }),
        ));
        story_refs.push(body.clone());
        items[page].push(frame(
            self_id(SAMPLE, "Frame", seq),
            body,
            FRAME_W,
            case.height,
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
    let mut spreads = Vec::new();
    for (p, page_items) in items.into_iter().enumerate() {
        let spread_id = self_id(SAMPLE, "Spread", p as u32);
        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id,
                page_self_id: self_id(SAMPLE, "Page", p as u32),
                page_name: (p + 1).to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        ));
    }

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
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
        spreads,
        stories,
    }
}
