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

//! `blank-paragraphs.idml` — how much room does a paragraph with no word
//! take? Found measuring Word documents: a paragraph of nothing but spaces
//! laid out no line in the engine and took no room.
//!
//! One row of frames, each "A", something, "B" in Inter 10/12
//! (`LeadingOffset`, zero insets); B's baseline says how tall the middle
//! is. See [`cases`].

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

const SAMPLE: &str = "blank-paragraphs";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
const FRAME_W: f32 = 70.0;
const FRAME_H: f32 = 90.0;
const COL_PITCH: f32 = 86.0;

/// One frame: its name and the paragraphs between "A" and "B", each a
/// text with its point size and leading.
pub struct Case {
    pub name: &'static str,
    pub middle: Vec<(&'static str, f32, f32)>,
}

pub fn cases() -> Vec<Case> {
    let p = |t: &'static str| (t, POINT_SIZE, LEADING);
    vec![
        Case {
            name: "nothing between",
            middle: vec![],
        },
        Case {
            name: "an empty paragraph",
            middle: vec![p("")],
        },
        Case {
            name: "one space",
            middle: vec![p(" ")],
        },
        Case {
            name: "three spaces",
            middle: vec![p("   ")],
        },
        Case {
            name: "two paragraphs of spaces",
            middle: vec![p(" "), p("  ")],
        },
        Case {
            name: "a space at 20/24",
            middle: vec![(" ", 20.0, 24.0)],
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

fn paragraph(text: &str, size: f32, leading: f32) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(leading),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
            point_size: Some(size),
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
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items: Vec<PageItem> = Vec::new();
    for (i, case) in cases().iter().enumerate() {
        let id = story_id(i as u32);
        let mut paragraphs = vec![paragraph("A", POINT_SIZE, LEADING)];
        for (text, size, leading) in &case.middle {
            paragraphs.push(paragraph(text, *size, *leading));
        }
        paragraphs.push(paragraph("B", POINT_SIZE, LEADING));
        stories.push((
            id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: id.clone(),
                paragraphs,
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
