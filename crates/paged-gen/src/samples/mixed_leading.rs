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

//! `mixed-leading.idml` — which leading does a line take when its
//! characters carry different ones? (RFI C-59: Word sizes every line by the
//! fonts on it; the engine gave every line the paragraph's FIRST run's
//! leading.)
//!
//! One row of frames, Inter 10 pt, each a single paragraph whose lines are
//! fixed by forced line breaks (U+2028) and start with a 10 pt label
//! (`L1`, `L2` ...), so each line's baseline reads off the label. The runs
//! of a line carry the leadings [`cases`] lists.

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

const SAMPLE: &str = "mixed-leading";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const FRAME_W: f32 = 84.0;
const FRAME_H: f32 = 130.0;
const COL_PITCH: f32 = 88.0;

/// U+2028 — the forced line break.
const LSEP: char = '\u{2028}';

/// A piece of a line: its text, point size, and leading (`None` = auto).
pub type Piece = (&'static str, f32, Option<&'static str>);

/// One frame: its name and its lines, each a list of pieces.
pub struct Case {
    pub name: &'static str,
    pub lines: Vec<Vec<Piece>>,
}

pub fn cases() -> Vec<Case> {
    let a = |t: &'static str| (t, 10.0, Some("12"));
    vec![
        Case {
            name: "one leading",
            lines: vec![vec![a("L1 aa")], vec![a("L2 aa")], vec![a("L3 aa")]],
        },
        Case {
            name: "20 from mid line 2 to line 3",
            lines: vec![
                vec![a("L1 aa")],
                vec![a("L2 aa "), ("bb", 10.0, Some("20"))],
                vec![("L3 bb", 10.0, Some("20"))],
                vec![a("L4 aa")],
            ],
        },
        Case {
            name: "a last line at 9",
            lines: vec![
                vec![a("L1 aa")],
                vec![a("L2 aa")],
                vec![("L3 bb", 10.0, Some("9"))],
            ],
        },
        Case {
            name: "a 14 pt auto word on line 2",
            lines: vec![
                vec![a("L1 aa")],
                vec![a("L2 aa "), ("Bb", 14.0, None), a(" aa")],
                vec![a("L3 aa")],
            ],
        },
        Case {
            name: "auto text, one word at 24 on line 2",
            lines: vec![
                vec![("L1 aa", 10.0, None)],
                vec![
                    ("L2 aa ", 10.0, None),
                    ("bb", 10.0, Some("24")),
                    (" aa", 10.0, None),
                ],
                vec![("L3 aa", 10.0, None)],
            ],
        },
        Case {
            name: "18, then a whole line at 12",
            lines: vec![
                vec![("L1 aa", 10.0, Some("18"))],
                vec![("L2 aa", 10.0, Some("18"))],
                vec![a("L3 bb")],
                vec![("L4 aa", 10.0, Some("18"))],
            ],
        },
    ]
}

/// Where frame `i`'s text area starts on the page.
pub fn frame_origin(i: u32) -> (f32, f32) {
    (36.0 + i as f32 * COL_PITCH, 48.0)
}

/// The story of frame `i`.
pub fn story_id(i: u32) -> String {
    self_id(SAMPLE, "Story", i)
}

fn paragraph(case: &Case) -> Paragraph {
    let mut runs = Vec::new();
    for (k, line) in case.lines.iter().enumerate() {
        for (j, (text, size, leading)) in line.iter().enumerate() {
            // The break ending the previous line rides this line's first
            // piece, in that piece's formatting.
            let text = if k > 0 && j == 0 {
                format!("{LSEP}{text}")
            } else {
                text.to_string()
            };
            runs.push(Run {
                extra_char_attrs: leading.map(|l| vec![("Leading", l)]).unwrap_or_default(),
                text,
                point_size: Some(*size),
                fill_color: Some("Color/Black".to_string()),
                font_style: None,
                tracking: None,
                baseline_shift: None,
                underline: None,
                applied_font: Some(BODY_FONT),
                anchored_frame: None,
            });
        }
    }
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: None,
        runs,
        ..Paragraph::plain("")
    }
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items: Vec<PageItem> = Vec::new();
    for (i, case) in cases().iter().enumerate() {
        let id = story_id(i as u32);
        stories.push((
            id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: id.clone(),
                paragraphs: vec![paragraph(case)],
            }),
        ));
        story_refs.push(id.clone());
        let (x, y) = frame_origin(i as u32);
        items.push(
            Rect {
                self_id: self_id(SAMPLE, "Frame", i as u32),
                width_pt: FRAME_W,
                height_pt: FRAME_H,
                item_transform: translate(x, y),
                fill_color: None,
                stroke_color: None,
                stroke_weight_pt: None,
                parent_story: Some(id),
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
                    inset_spacing: Some([0.0, 0.0, 0.0, 0.0]),
                    first_baseline_offset: Some("LeadingOffset"),
                    ..Default::default()
                }),
                custom_subpaths: None,
            }
            .into(),
        );
    }
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
                page_items: items,
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        )],
        stories,
    }
}
