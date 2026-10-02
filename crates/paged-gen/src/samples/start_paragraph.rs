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

//! `start-paragraph.idml` — the break-before rule (`StartParagraph`,
//! ADR 028) against InDesign.
//!
//! Every case is one story threaded through a four-page chain:
//!
//! | page | frame | |
//! |---|---|---|
//! | 1 | A | two columns, ten lines each |
//! | 1 | B | one column, ten lines, below A |
//! | 2 | C | |
//! | 3 | D | |
//! | 4 | E | |
//!
//! Each case starts on an ODD page (cases are four pages long), so where a
//! paragraph lands answers the rule directly: A's second column (next
//! column), B (next frame), C (next page, next even page), D (next odd
//! page). The line grid is the `keeps` fixture's: 12 pt leading, a
//! `LeadingOffset` first baseline and zero insets, so a frame holds exactly
//! ten one-line paragraphs, numbered `P01`, `P02`, ...
//!
//! The last four cases ask what InDesign does when the rule meets a
//! paragraph that already opens a column, frame or page, or the story
//! itself.

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

const SAMPLE: &str = "start-paragraph";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
/// Ten 12 pt lines per column (baseline 120 fits, 132 does not).
const FRAME_H: f32 = 126.0;
const COLUMN_W: f32 = 100.0;
const GUTTER: f32 = 20.0;
/// Pages per case (the chain's length in pages).
pub const PAGES_PER_CASE: usize = 4;

/// A one-line paragraph `P07`, with `start` as its `StartParagraph`.
fn line(n: u32, start: Option<&'static str>) -> Paragraph {
    let mut attrs: Vec<(&'static str, &'static str)> = vec![("Hyphenation", "false")];
    if let Some(s) = start {
        attrs.push(("StartParagraph", s));
    }
    Paragraph {
        extra_paragraph_attrs: attrs,
        leading: Some(LEADING),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: format!("P{n:02}"),
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

/// A case: its label, the paragraph count, and which paragraph carries
/// which `StartParagraph` value.
pub struct Case {
    pub name: &'static str,
    pub paragraphs: u32,
    pub rule_on: u32,
    pub rule: &'static str,
}

/// The cases, in page order.
pub fn cases() -> Vec<Case> {
    let c = |name, paragraphs, rule_on, rule| Case {
        name,
        paragraphs,
        rule_on,
        rule,
    };
    vec![
        c("control · P04 Anywhere", 8, 4, "Anywhere"),
        c("P04 NextColumn", 8, 4, "NextColumn"),
        c("P04 NextFrame", 8, 4, "NextFrame"),
        c("P04 NextPage", 8, 4, "NextPage"),
        c("P04 NextOddPage", 8, 4, "NextOddPage"),
        c("P04 NextEvenPage", 8, 4, "NextEvenPage"),
        // The rule on the story's first paragraph (top of frame A, page 1).
        c("P01 NextPage · story start", 8, 1, "NextPage"),
        c(
            "P01 NextEvenPage · story start on an odd page",
            8,
            1,
            "NextEvenPage",
        ),
        // A fills both columns; P21 opens B anyway. Does it skip B?
        c("P21 NextFrame · already opens frame B", 24, 21, "NextFrame"),
        // P11 opens column two anyway. Does it skip it?
        c(
            "P11 NextColumn · already opens column two",
            14,
            11,
            "NextColumn",
        ),
    ]
}

/// The body story of case `i` (0-based).
pub fn body_story_id(i: u32) -> String {
    self_id(SAMPLE, "BodyStory", i)
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let mut master_spreads = Vec::new();
    let mut spreads = Vec::new();
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    let mut spread_refs = Vec::new();

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    master_spreads.push((
        master_id.clone(),
        write_master(&Master {
            self_id: format!("MasterSpread/{master_id}"),
            page_self_id: self_id(SAMPLE, "MasterPage", 0),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        }),
    ));

    for (i, case) in cases().into_iter().enumerate() {
        let seq = i as u32;
        let label_story_id = self_id(SAMPLE, "LabelStory", seq);
        let body = body_story_id(seq);

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
                paragraphs: (1..=case.paragraphs)
                    .map(|n| line(n, (n == case.rule_on).then_some(case.rule)))
                    .collect(),
            }),
        ));
        story_refs.push(body.clone());

        // The chain A → B → C → D → E.
        let ids: Vec<String> = ["A", "B", "C", "D", "E"]
            .iter()
            .map(|f| self_id(SAMPLE, &format!("Frame{f}"), seq))
            .collect();
        let link = |k: usize| ((k > 0).then(|| ids[k - 1].clone()), ids.get(k + 1).cloned());
        let body_frame = |k: usize, columns: u32, y: f32| -> PageItem {
            let (prev, next) = link(k);
            let width = columns as f32 * COLUMN_W + (columns as f32 - 1.0) * GUTTER;
            Rect {
                self_id: ids[k].clone(),
                width_pt: width,
                height_pt: FRAME_H,
                item_transform: translate(72.0, y),
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
            self_id: self_id(SAMPLE, "LabelFrame", seq),
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

        let pages: Vec<Vec<PageItem>> = vec![
            vec![label, body_frame(0, 2, 72.0), body_frame(1, 1, 300.0)],
            vec![body_frame(2, 1, 72.0)],
            vec![body_frame(3, 1, 72.0)],
            vec![body_frame(4, 1, 72.0)],
        ];
        for (p, items) in pages.into_iter().enumerate() {
            let n = (i * PAGES_PER_CASE + p) as u32;
            let spread_id = self_id(SAMPLE, "Spread", n);
            spreads.push((
                spread_id.clone(),
                write_spread(&Spread {
                    self_id: spread_id.clone(),
                    page_self_id: self_id(SAMPLE, "Page", n),
                    page_name: (n + 1).to_string(),
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
