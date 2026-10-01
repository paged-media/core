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

//! `reflow.idml` — page growth (thoughts ADR 026), the case InDesign's
//! Smart Text Reflow answers.
//!
//! Two pages, one text frame each, THREADED (Smart Text Reflow only grows a
//! chain; InDesign ignores a lone unthreaded frame), holding 80 numbered
//! one-line paragraphs on an exact 12 pt grid (`LeadingOffset`, zero
//! insets): each 400 pt frame holds 33 lines, so 14 lines are overset and
//! InDesign adds one page. The master carries a text frame of the same
//! geometry.
//!
//! InDesign does not reflow on open; `tools/indesign-export/reflow-probe.jsx`
//! switches Smart Text Reflow on, nudges the story so it reflows, and reports
//! the pages it added, their frames, and which paragraph opens each page.

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

const SAMPLE: &str = "reflow";
pub const PAGE_W_PT: f32 = 372.0;
pub const PAGE_H_PT: f32 = 472.0;
/// The text frame, on the page and on the master alike.
pub const FRAME_X: f32 = 36.0;
pub const FRAME_Y: f32 = 36.0;
pub const FRAME_W: f32 = 300.0;
/// 33 lines of 12 pt (baseline 396 fits, 408 does not).
pub const FRAME_H: f32 = 400.0;
pub const PARAGRAPHS: u32 = 80;
const BODY_FONT: &str = "Inter";

fn line(n: u32) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: vec![("Hyphenation", "false")],
        leading: Some(12.0),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: format!("Paragraph {n:02} of the reflowing story."),
            point_size: Some(10.0),
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

fn text_frame(
    self_id: String,
    story: Option<String>,
    next: Option<String>,
    prev: Option<String>,
) -> PageItem {
    Rect {
        self_id,
        width_pt: FRAME_W,
        height_pt: FRAME_H,
        item_transform: translate(FRAME_X, FRAME_Y),
        fill_color: None,
        stroke_color: Some("Color/Black".to_string()),
        stroke_weight_pt: Some(0.25),
        parent_story: story,
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
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let body_story_id = self_id(SAMPLE, "BodyStory", 0);
    let master_story_id = self_id(SAMPLE, "MasterStory", 0);

    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: self_id(SAMPLE, "MasterPage", 0),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![text_frame(
            self_id(SAMPLE, "MasterFrame", 0),
            Some(master_story_id.clone()),
            None,
            None,
        )],
    });
    let frame_ids: Vec<String> = (0..2).map(|i| self_id(SAMPLE, "BodyFrame", i)).collect();
    let spreads: Vec<(String, Vec<u8>)> = (0..2u32)
        .map(|i| {
            let id = self_id(SAMPLE, "Spread", i);
            let next = (i == 0).then(|| frame_ids[1].clone());
            let prev = (i == 1).then(|| frame_ids[0].clone());
            let bytes = write_spread(&Spread {
                self_id: id.clone(),
                page_self_id: self_id(SAMPLE, "Page", i),
                page_name: (i + 1).to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![text_frame(
                    frame_ids[i as usize].clone(),
                    Some(body_story_id.clone()),
                    next,
                    prev,
                )],
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            });
            (id, bytes)
        })
        .collect();
    let body = write_story(&Story {
        extra_story_attrs: Vec::new(),
        self_id: body_story_id.clone(),
        paragraphs: (1..=PARAGRAPHS).map(line).collect(),
    });
    let master_story = write_story(&Story {
        extra_story_attrs: Vec::new(),
        self_id: master_story_id.clone(),
        paragraphs: vec![Paragraph::plain("")],
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
            stories: vec![body_story_id.clone(), master_story_id.clone()],
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
        stories: vec![(body_story_id, body), (master_story_id, master_story)],
    }
}
