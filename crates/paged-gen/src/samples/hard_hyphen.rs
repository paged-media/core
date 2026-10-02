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

//! `hard-hyphen.idml` — does a line break after a hyphen the text already
//! has? (RFI C-54: Word breaks `two-` / `way`; the engine only broke at
//! spaces, soft hyphens and dictionary points.)
//!
//! A grid on one page: each ROW is a short text, each COLUMN a frame width,
//! so reading the lines of a row left to right shows at which width the
//! compound stops fitting and where InDesign breaks it. All in Inter 10/12,
//! `LeadingOffset`, zero insets.
//!
//! Rows: the same compound with hyphenation off (Paragraph Composer), with
//! the Single-line Composer, with hyphenation on, and justified; a chain of
//! hyphens and a one-letter prefix (`state-of-the-art`, `x-ray`); an en dash
//! and an em dash.

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

const SAMPLE: &str = "hard-hyphen";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
const FRAME_H: f32 = 66.0;
const ROW_PITCH: f32 = 90.0;
const COL_PITCH: f32 = 108.0;

/// The frame widths, left to right.
pub const WIDTHS: [f32; 5] = [50.0, 60.0, 70.0, 80.0, 95.0];

type Attrs = Vec<(&'static str, &'static str)>;

/// The rows: a text and its paragraph attributes.
pub fn rows() -> Vec<(&'static str, Attrs)> {
    let off: Attrs = vec![("Hyphenation", "false")];
    vec![
        ("The two-way street", off.clone()),
        (
            "The two-way street",
            vec![("Hyphenation", "false"), ("Composer", "HL Single")],
        ),
        ("The two-way street", vec![("Hyphenation", "true")]),
        (
            "The two-way street",
            vec![("Hyphenation", "false"), ("Justification", "LeftJustified")],
        ),
        ("A state-of-the-art x-ray", off.clone()),
        ("pages 10\u{2013}20 and so\u{2014}on", off),
    ]
}

/// The story of the frame at (`row`, `col`).
pub fn story_id(row: u32, col: u32) -> String {
    self_id(SAMPLE, "Story", row * WIDTHS.len() as u32 + col)
}

fn paragraph(text: &str, attrs: &Attrs) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: attrs.clone(),
        leading: Some(LEADING),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
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
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut items: Vec<PageItem> = Vec::new();
    for (r, (text, attrs)) in rows().iter().enumerate() {
        for (c, width) in WIDTHS.iter().enumerate() {
            let id = story_id(r as u32, c as u32);
            stories.push((
                id.clone(),
                write_story(&Story {
                    extra_story_attrs: Vec::new(),
                    self_id: id.clone(),
                    paragraphs: vec![paragraph(text, attrs)],
                }),
            ));
            story_refs.push(id.clone());
            items.push(
                Rect {
                    self_id: self_id(SAMPLE, "Frame", r as u32 * WIDTHS.len() as u32 + c as u32),
                    width_pt: *width,
                    height_pt: FRAME_H,
                    item_transform: translate(
                        36.0 + c as f32 * COL_PITCH,
                        48.0 + r as f32 * ROW_PITCH,
                    ),
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
