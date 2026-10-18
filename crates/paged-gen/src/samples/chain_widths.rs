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

//! `chain-widths.idml` — a story threaded through frames of different
//! widths, against InDesign (ADR 179).
//!
//! Two pages, one story each, set in Inter 10/12 pt with numbered
//! sentences so every line reads differently in the PDF:
//!
//! 1. frames of 400, 250 and 400 pt, one under the other: the first
//!    paragraph runs from the wide frame into the narrow one, the second
//!    starts in the narrow frame and runs on into the wide one, the third
//!    starts in the last frame;
//! 2. frames of 250 and 400 pt: a paragraph runs from the narrow frame
//!    into the wide one.
//!
//! Frame heights are whole lines (12 pt leading, a `LeadingOffset` first
//! baseline, no insets), so where a frame ends is where a line ends.

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

const SAMPLE: &str = "chain-widths";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;

/// A case: its frames `(width, lines)`, one under the other, and its
/// paragraphs' sentence counts.
pub struct Case {
    pub name: &'static str,
    pub frames: &'static [(f32, u32)],
    pub paragraphs: &'static [usize],
}

/// The cases, one per page.
pub fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "400 · 250 · 400",
            frames: &[(400.0, 8), (250.0, 10), (400.0, 8)],
            paragraphs: &[18, 9, 6],
        },
        Case {
            name: "250 · 400",
            frames: &[(250.0, 8), (400.0, 10)],
            paragraphs: &[16],
        },
    ]
}

/// The story of case `i` (0-based).
pub fn story_id(i: u32) -> String {
    self_id(SAMPLE, "Story", i)
}

/// Numbered sentences of paragraph `p`: `P2 s3 keeps ...`.
fn sentences(p: usize, n: usize) -> String {
    (1..=n)
        .map(|i| format!("P{p} s{i} keeps the line full of ordinary words."))
        .collect::<Vec<_>>()
        .join(" ")
}

fn paragraph(text: String) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(LEADING),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text,
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
        let story = story_id(seq);
        stories.push((
            story.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: story.clone(),
                paragraphs: case
                    .paragraphs
                    .iter()
                    .enumerate()
                    .map(|(p, &n)| paragraph(sentences(p + 1, n)))
                    .collect(),
            }),
        ));
        story_refs.push(story.clone());
        let ids: Vec<String> = (0..case.frames.len())
            .map(|f| self_id(SAMPLE, &format!("Frame{f}"), seq))
            .collect();
        let mut y = 72.0;
        let mut items: Vec<PageItem> = Vec::new();
        for (f, &(width, lines)) in case.frames.iter().enumerate() {
            // Whole lines: the first baseline one leading down, then a
            // leading per line, and a little room under the last.
            let height = lines as f32 * LEADING + 3.0;
            items.push(
                Rect {
                    self_id: ids[f].clone(),
                    width_pt: width,
                    height_pt: height,
                    item_transform: translate(72.0, y),
                    fill_color: None,
                    stroke_color: None,
                    stroke_weight_pt: None,
                    parent_story: Some(story.clone()),
                    next_text_frame: ids.get(f + 1).cloned(),
                    previous_text_frame: f.checked_sub(1).map(|p| ids[p].clone()),
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
            y += height + 24.0;
        }
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
