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

//! `keeps-reflow.idml` — Word's widow control on a GROWING chain (thoughts
//! ADR 026 + 028): the case plugin-doc's lowering produces for every Word
//! document, where widow control is on by default.
//!
//! Two authored pages, one threaded frame each (Smart Text Reflow grows only
//! a chain), on an exact 12 pt grid (`LeadingOffset`, zero insets): each
//! 246 pt frame holds 20 lines. The frame is 100 pt wide and every body
//! paragraph is made of ~80 pt tokens with hyphenation off, so a paragraph
//! of K tokens is exactly K lines (the `keeps` fixture's trick). Ten
//! sections of a one-line heading and three or four body paragraphs make
//! 159 lines: about eight pages, six of them generated.
//!
//! The keeps are plugin-doc's lowering of Word's defaults: every paragraph
//! `KeepLinesTogether` At Start / At End 2 / 2 (widow control), and the
//! headings also `KeepWithNext` 1 (`w:keepNext`). Paragraphs are numbered
//! (`P07head`, `P08lineA…`) so a misplaced page break reads at a glance.
//!
//! InDesign does not reflow on open; `tools/indesign-export/reflow-probe.sh`
//! switches Smart Text Reflow on, nudges the story and reports every page's
//! first and last line.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, MarginPreference, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "keeps-reflow";
/// The frame is the page's margin box (36 pt all round), which is where
/// InDesign puts a generated page's frame.
pub const MARGIN: f32 = 36.0;
pub const FRAME_W: f32 = 100.0;
/// 20 lines of 12 pt (baseline 240 fits, 252 does not).
pub const FRAME_H: f32 = 246.0;
pub const PAGE_W_PT: f32 = FRAME_W + 2.0 * MARGIN;
pub const PAGE_H_PT: f32 = FRAME_H + 2.0 * MARGIN;
const BODY_FONT: &str = "Inter";

/// Each section's body paragraph lengths, in lines (a heading opens each).
pub const SECTIONS: [&[u32]; 10] = [
    &[5, 3, 6],
    &[4, 7, 2],
    &[3, 5, 4, 6],
    &[2, 6, 3],
    &[7, 4, 5],
    &[3, 3, 6, 4],
    &[5, 2, 7],
    &[4, 6, 3, 5],
    &[6, 4, 3],
    &[5, 5, 2, 4],
];

type Attrs = Vec<(&'static str, &'static str)>;

/// Word's widow control, as plugin-doc lowers it.
fn widow_control() -> Attrs {
    vec![
        ("Hyphenation", "false"),
        ("KeepLinesTogether", "true"),
        ("KeepAllLinesTogether", "false"),
        ("KeepFirstLines", "2"),
        ("KeepLastLines", "2"),
    ]
}

fn para(tokens: &[String], attrs: Attrs) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: attrs,
        leading: Some(12.0),
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: tokens.join(" "),
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

/// The story's paragraphs as `(tokens, is_heading)`, in order: what the tests
/// and the probe's report are read against.
pub fn paragraphs() -> Vec<(Vec<String>, bool)> {
    let mut out = Vec::new();
    let mut n = 0u32;
    for body in SECTIONS {
        n += 1;
        out.push((vec![format!("P{n:02}head")], true));
        for &lines in body {
            n += 1;
            let tokens = (0..lines)
                .map(|i| format!("P{n:02}line{}mmmm", (b'A' + i as u8) as char))
                .collect();
            out.push((tokens, false));
        }
    }
    out
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
        item_transform: translate(MARGIN, MARGIN),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
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

    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: self_id(SAMPLE, "MasterPage", 0),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
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
                margins: Some(MarginPreference {
                    top: MARGIN,
                    bottom: MARGIN,
                    left: MARGIN,
                    right: MARGIN,
                    column_count: 1,
                    column_gutter: 12.0,
                }),
                item_transform: None,
            });
            (id, bytes)
        })
        .collect();
    let body = write_story(&Story {
        extra_story_attrs: Vec::new(),
        self_id: body_story_id.clone(),
        paragraphs: paragraphs()
            .into_iter()
            .map(|(tokens, heading)| {
                let mut attrs = widow_control();
                if heading {
                    attrs.push(("KeepWithNext", "1"));
                }
                para(&tokens, attrs)
            })
            .collect(),
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: spreads.iter().map(|(id, _)| id.clone()).collect(),
            stories: vec![body_story_id.clone()],
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
        stories: vec![(body_story_id, body)],
    }
}
