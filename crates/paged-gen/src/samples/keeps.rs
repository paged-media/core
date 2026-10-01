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

//! `keeps.idml` — paragraph keep options at a column break (ADR 028).
//!
//! Every page is one two-column text frame on an EXACT line grid, so the
//! column break falls on a known line and each keep rule is visible as
//! "which line opens column two":
//!
//! - 12 pt leading on every paragraph, `FirstBaselineOffset="LeadingOffset"`
//!   and zero insets put baseline *k* at `12·k` pt. The frame is 126 pt
//!   tall, so each column holds exactly ten lines.
//! - Columns are 100 pt wide and every multi-line paragraph is made of
//!   ~80 pt tokens with hyphenation off, so a paragraph of K tokens is
//!   exactly K lines — in InDesign and in the engine alike, without forced
//!   line breaks (which the importer turns into sub-paragraphs).
//! - Paragraphs are NUMBERED so a misplaced break is obvious in a diff.
//!
//! The pages pair each rule with a no-keep control. InDesign exports the
//! reference (`tools/indesign-export`): it is the oracle for the open
//! questions, notably whether keep-with-next moves a multi-line paragraph
//! WHOLE or only its last line, and how first/last-line keeps resolve.

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

const SAMPLE: &str = "keeps";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const BODY_FONT: &str = "Inter";
const POINT_SIZE: f32 = 10.0;
const LEADING: f32 = 12.0;
/// Ten 12 pt lines per column (baseline 120 fits, 132 does not).
const FRAME_H: f32 = 126.0;
const COLUMN_W: f32 = 100.0;
const GUTTER: f32 = 20.0;

type Attrs = Vec<(&'static str, &'static str)>;

/// One paragraph whose lines are `tokens` (one token per line).
fn para(tokens: &[String], attrs: Attrs) -> Paragraph {
    let mut extra: Attrs = vec![("Hyphenation", "false")];
    extra.extend(attrs);
    Paragraph {
        extra_paragraph_attrs: extra,
        leading: Some(LEADING),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: tokens.join(" "),
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

/// A one-line numbered paragraph, `P07`.
fn line(n: u32) -> Paragraph {
    para(&[format!("P{n:02}")], Vec::new())
}

/// An `lines`-line paragraph numbered `n`: tokens `P08lineAmmmm`, `P08lineBmmmm`…
/// (~80 pt each, so two never share a 100 pt line).
fn multi(n: u32, lines: u32, attrs: Attrs) -> Paragraph {
    let tokens: Vec<String> = (0..lines)
        .map(|i| format!("P{n:02}line{}mmmm", (b'A' + i as u8) as char))
        .collect();
    para(&tokens, attrs)
}

/// One-line paragraphs numbered `from..=to`.
fn lines(from: u32, to: u32) -> Vec<Paragraph> {
    (from..=to).map(line).collect()
}

/// The pages: a name and the story's paragraphs.
fn cases() -> Vec<(&'static str, Vec<Paragraph>)> {
    let kwn2: Attrs = vec![("KeepWithNext", "2")];
    let kwn1: Attrs = vec![("KeepWithNext", "1")];
    let keep_all: Attrs = vec![
        ("KeepLinesTogether", "true"),
        ("KeepAllLinesTogether", "true"),
    ];
    let keep_2_2 = || -> Attrs {
        vec![
            ("KeepLinesTogether", "true"),
            ("KeepAllLinesTogether", "false"),
            ("KeepFirstLines", "2"),
            ("KeepLastLines", "2"),
        ]
    };
    let with = |mut v: Vec<Paragraph>, p: Paragraph, rest: Vec<Paragraph>| {
        v.push(p);
        v.extend(rest);
        v
    };
    vec![
        // Line 10 (the last of column one) is a one-line paragraph.
        (
            "keeps · control · one-line paragraph at the column foot",
            with(lines(1, 9), line(10), lines(11, 16)),
        ),
        (
            "keeps · KeepWithNext=2 · one-line paragraph at the column foot",
            with(lines(1, 9), para(&["P10".to_string()], kwn2), lines(11, 16)),
        ),
        // A three-line paragraph ends exactly at the column foot (lines
        // 8-10), so the next paragraph opens column two. WHOLE or LAST LINE?
        (
            "keeps · control · three-line paragraph ends at the column foot",
            with(lines(1, 7), multi(8, 3, Vec::new()), lines(9, 14)),
        ),
        (
            "keeps · KeepWithNext=1 · three-line paragraph ends at the column foot",
            with(lines(1, 7), multi(8, 3, kwn1), lines(9, 14)),
        ),
        // A four-line paragraph straddles the break 2 | 2 (lines 9-12).
        (
            "keeps · control · four-line paragraph straddles 2|2",
            with(lines(1, 8), multi(9, 4, Vec::new()), lines(10, 14)),
        ),
        (
            "keeps · KeepAllLinesTogether · four-line paragraph straddles 2|2",
            with(lines(1, 8), multi(9, 4, keep_all), lines(10, 14)),
        ),
        // First/last-line keeps (2 / 2): 1 | 3 breaks the first-lines
        // keep, 3 | 1 breaks the last-lines keep.
        (
            "keeps · KeepFirstLines=2 · four-line paragraph straddles 1|3",
            with(lines(1, 9), multi(10, 4, keep_2_2()), lines(11, 14)),
        ),
        (
            "keeps · KeepLastLines=2 · four-line paragraph straddles 3|1",
            with(lines(1, 7), multi(8, 4, keep_2_2()), lines(9, 14)),
        ),
    ]
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let cases = cases();
    let mut master_spreads = Vec::with_capacity(cases.len());
    let mut spreads = Vec::with_capacity(cases.len());
    let mut stories = Vec::with_capacity(cases.len() * 2);
    let mut master_refs = Vec::with_capacity(cases.len());
    let mut spread_refs = Vec::with_capacity(cases.len());
    let mut story_refs = Vec::with_capacity(cases.len() * 2);

    for (i, (name, paragraphs)) in cases.into_iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let label_story_id = self_id(SAMPLE, "LabelStory", seq);
        let body_story_id = self_id(SAMPLE, "BodyStory", seq);

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: self_id(SAMPLE, "MasterPage", seq),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());

        stories.push((
            label_story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story_id.clone(),
                paragraphs: vec![Paragraph::plain(name)],
            }),
        ));
        story_refs.push(label_story_id.clone());
        stories.push((
            body_story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: body_story_id.clone(),
                paragraphs,
            }),
        ));
        story_refs.push(body_story_id.clone());

        let label: PageItem = frame(
            self_id(SAMPLE, "LabelFrame", seq),
            label_story_id,
            460.0,
            24.0,
            translate(36.0, 12.0),
            None,
        );
        let body: PageItem = frame(
            self_id(SAMPLE, "BodyFrame", seq),
            body_story_id,
            2.0 * COLUMN_W + GUTTER,
            FRAME_H,
            translate(72.0, 72.0),
            Some(TextFramePref {
                inset_spacing: Some([0.0, 0.0, 0.0, 0.0]),
                first_baseline_offset: Some("LeadingOffset"),
                text_column_count: Some(2),
                text_column_gutter: Some(GUTTER),
                ..Default::default()
            }),
        );

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: self_id(SAMPLE, "Page", seq),
                page_name: name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![label, body],
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
            master_spreads: master_refs,
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

/// A text frame (thin grey stroke so the frame edge is visible).
fn frame(
    self_id: String,
    story_id: String,
    width_pt: f32,
    height_pt: f32,
    item_transform: crate::geometry::Matrix,
    text_frame_pref: Option<TextFramePref>,
) -> PageItem {
    Rect {
        self_id,
        width_pt,
        height_pt,
        item_transform,
        fill_color: None,
        stroke_color: Some("Color/Black".to_string()),
        stroke_weight_pt: Some(0.25),
        parent_story: Some(story_id),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref,
        custom_subpaths: None,
    }
    .into()
}
